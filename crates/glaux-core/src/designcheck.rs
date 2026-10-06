//! 曲の設計データ(計画)と実際の音を並べて比べる(曲の設計データの段階 0。MCP の get_design、設計画面の土台)。
//!
//! - 区間: 計画の盛り上がり(形の平均)と、測った盛り上がり([`crate::critique`] の区間の energy)
//! - パート × 区間: 計画の存在の段階・音域の帯と、測った値(鳴っているか・音の数・音量から見た段階・
//!   実際の音域〈下 10%〜上 90%〉・密度)
//! - ずれ: 計画と実際の食い違い。直すのは計画か音のどちらか(人と AI に同じものを見せる)
//! - クリップの状態: 計画どおり / 計画が先に進んだ、手で直した小節、固定の音の数
//!
//! 推定した計画(state: estimated)は参考として並べるが、ずれの基準にはしない

use crate::i18n::{is_en, t};
use crate::id::{PlanId, TrackId};
use crate::model::{Project, SectionJoin, Track};
use crate::plan::{PartPlan, PlanSet, SongPlan, PRESENCE};
use serde::Serialize;

/// 区間 1 つ
#[derive(Clone, Debug, Serialize)]
pub struct SectionView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    /// 始まりの小節(1 始まり)と長さ(小節)
    pub start_bar: usize,
    pub bars: usize,
    /// 計画の盛り上がり(形の平均。形が無ければ energy)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub planned: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub curve: Vec<[f32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub join: Option<SectionJoin>,
    /// 測った盛り上がり 0〜10
    pub measured: f64,
}

/// パート(トラック)× 区間の 1 マス
#[derive(Clone, Debug, Serialize)]
pub struct CellView {
    /// 区間の ID(無い曲は区間の名前)
    pub section: String,
    /// 計画の存在の段階・働き・音域の帯(パートの計画が無ければ省略)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub planned: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub planned_register: Option<[u8; 2]>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
    /// 測った存在の段階(鳴っているか・音量の順位から。0 = 鳴っていない)
    pub measured: u8,
    pub notes: usize,
    /// 実際の音域(下 10%〜上 90%)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub register: Option<[u8; 2]>,
    /// 密度 0〜1(1 小節 16 音で 1)
    pub density: f32,
}

/// パート(トラック)1 つ
#[derive(Clone, Debug, Serialize)]
pub struct PartView {
    pub track_id: TrackId,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// パートの計画(無ければ省略)と、その既定の働き
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_id: Option<PlanId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    /// 計画が推定(まだ人が確かめていない)なら true
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub estimated: bool,
    pub cells: Vec<CellView>,
}

/// 計画と実際のずれ 1 つ
#[derive(Clone, Debug, Serialize)]
pub struct Deviation {
    /// "warn"(食い違っている)/ "info"(検討)
    pub severity: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track: Option<String>,
    pub what: String,
    /// 直し方(計画を実際に合わせる / 音を計画に合わせる)
    pub fix: String,
}

/// 2 つの曲の音の違い(案の「変わる所」)。曲の頭からの範囲 [a, b) で持つ: ノートの違い、
/// トラックの設定(音色・エフェクト・つまみ・オートメーション・音量など)の違いはそのトラックが鳴っている所。
/// マスター・テンポ・拍子・音の無いトラック(バス)の違いは曲全体に効くので `whole`。
/// 鳴っていないトラック([`silent_tracks`])の違いは数えない
#[derive(Clone, Debug, Default, Serialize)]
pub struct SongDiff {
    pub ranges: Vec<(u64, u64)>,
    pub whole: bool,
    /// 違いはあるが、鳴っていないトラック(ミュート中・ほかのトラックのソロ中など)だけ。聴き比べても違いが無い
    pub silent_only: bool,
}

/// 鳴っていないトラックの ID: ミュート中、出力先のバスがミュート中(送りも無い)、ほかのトラックがソロ中
/// (バスと、ソロのバスへ出す・送るトラックは鳴る)。エンジンの判定(`TrackMix::audible`)に合わせた近似
pub fn silent_tracks(p: &Project) -> std::collections::BTreeSet<String> {
    use crate::model::TrackKind;
    let any_solo = p.tracks.iter().any(|t| t.solo);
    let by_id = |id: &TrackId| p.tracks.iter().find(|t| &t.id == id);
    // 出力先の連なり(壊れた輪でも止まるように 16 段まで)
    let outputs = |t: &Track| -> Vec<&Track> {
        let mut v = Vec::new();
        let mut cur = t.output.as_ref();
        while let Some(b) = cur.and_then(by_id) {
            if v.len() >= 16 {
                break;
            }
            v.push(b);
            cur = b.output.as_ref();
        }
        v
    };
    p.tracks
        .iter()
        .filter(|t| {
            let outs = outputs(t);
            let muted = t.mute || (outs.iter().any(|b| b.mute) && t.sends.is_empty());
            let solo_ok = !any_solo
                || t.solo
                || t.kind == TrackKind::Bus
                || outs.iter().any(|b| b.solo)
                || t.sends
                    .iter()
                    .any(|s| by_id(&s.target).is_some_and(|b| b.solo));
            muted || !solo_ok
        })
        .map(|t| t.id.to_string())
        .collect()
}

/// 2 つの曲の音の違いを調べる(案を当てる前と後)。鳴っていないトラックの違いは数えない
pub fn song_diff(before: &Project, after: &Project) -> SongDiff {
    let d = song_diff_inner(before, after, true);
    if d.ranges.is_empty() && !d.whole {
        let all = song_diff_inner(before, after, false);
        return SongDiff {
            silent_only: !all.ranges.is_empty() || all.whole,
            ..d
        };
    }
    d
}

fn song_diff_inner(before: &Project, after: &Project, audible_only: bool) -> SongDiff {
    use std::collections::{BTreeMap, BTreeSet};
    let silent = |p: &Project| {
        if audible_only {
            silent_tracks(p)
        } else {
            BTreeSet::new()
        }
    };
    let (silent_a, silent_b) = (silent(before), silent(after));
    // トラックごとの、鳴る音(ID を除いた中身と曲の頭からの位置)の数え上げ
    fn sounding(
        p: &Project,
        silent: &BTreeSet<String>,
    ) -> BTreeMap<String, BTreeMap<String, (u64, u64, i64)>> {
        let mut out: BTreeMap<String, BTreeMap<String, (u64, u64, i64)>> = BTreeMap::new();
        for t in &p.tracks {
            let m = out.entry(t.id.to_string()).or_default();
            if silent.contains(t.id.as_str()) {
                continue;
            }
            for c in &t.clips {
                // 音声クリップ: 中身(素材・位置・長さ・音量・フェード・伸縮など。ID と名前・色は除く)で数える
                if c.notes().is_none() {
                    let mut v = serde_json::to_value(c).unwrap_or_default();
                    if let Some(o) = v.as_object_mut() {
                        o.remove("id");
                        o.remove("name");
                        o.remove("color");
                    }
                    let key = format!("audio:{v}");
                    let e = m.entry(key).or_insert((c.start.0, c.end().0, 0));
                    e.2 += 1;
                    continue;
                }
                for n in c.playback_notes() {
                    let at = c.start.0 + n.pos.0;
                    let mut k = n.clone();
                    k.id = crate::id::NoteId::parse("nt_x").expect("固定の ID");
                    k.locked = false;
                    k.pos = crate::time::Tick(at);
                    let key = serde_json::to_string(&k).unwrap_or_default();
                    let e = m.entry(key).or_insert((at, at + n.dur.0, 0));
                    e.2 += 1;
                }
            }
        }
        out
    }
    let (a, b) = (sounding(before, &silent_a), sounding(after, &silent_b));
    let mut ranges = Vec::new();
    let tracks: std::collections::BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    let empty = BTreeMap::new();
    for t in tracks {
        let (x, y) = (a.get(t).unwrap_or(&empty), b.get(t).unwrap_or(&empty));
        let keys: std::collections::BTreeSet<&String> = x.keys().chain(y.keys()).collect();
        for k in keys {
            let (cx, cy) = (x.get(k).map_or(0, |v| v.2), y.get(k).map_or(0, |v| v.2));
            if cx != cy {
                let (s, e, _) = x.get(k).or(y.get(k)).copied().unwrap_or_default();
                ranges.push((s, e.max(s + 1)));
            }
        }
    }
    let mut merged = ranges;
    // トラックの設定(クリップ以外)の違い: そのトラックが鳴っている所(前と後のどちらかで鳴る音の範囲)が変わる
    let settings = |t: &crate::model::Track| -> serde_json::Value {
        let mut v = serde_json::to_value(t).unwrap_or_default();
        if let Some(o) = v.as_object_mut() {
            o.remove("clips");
            o.remove("name");
            o.remove("color");
        }
        v
    };
    let mut whole = serde_json::to_value(&before.master).ok()
        != serde_json::to_value(&after.master).ok()
        || before.tempo_map != after.tempo_map
        || before.time_sig_map != after.time_sig_map;
    // 足した・変えたトラックと、消したトラック(前の曲だけにある)。どちらの曲でも鳴っていなければ数えない
    let removed = before
        .tracks
        .iter()
        .filter(|t| after.track(&t.id).is_none())
        .map(|t| (Some(t), None));
    let kept = after.tracks.iter().map(|t| (before.track(&t.id), Some(t)));
    for (old, new) in kept.chain(removed) {
        if let (Some(o), Some(n)) = (old, new) {
            if settings(o) == settings(n) {
                continue;
            }
        }
        let heard_a = old.filter(|t| !silent_a.contains(t.id.as_str()));
        let heard_b = new.filter(|t| !silent_b.contains(t.id.as_str()));
        if heard_a.is_none() && heard_b.is_none() {
            continue;
        }
        let mut spans: Vec<(u64, u64)> = Vec::new();
        for tr in [heard_a, heard_b].into_iter().flatten() {
            for c in &tr.clips {
                for n in c.playback_notes() {
                    let at = c.start.0 + n.pos.0;
                    spans.push((at, at + n.dur.0.max(1)));
                }
            }
        }
        if spans.is_empty() {
            // 音の無いトラック(バス・空のトラック)の違いは、送ってくる音のどこに効くか分からないので曲全体
            whole = true;
        }
        merged.extend(spans);
    }
    // 重なる・1 拍より近い範囲はつなぐ(音ごとに細切れにしない)
    merged.sort_unstable();
    let mut ranges: Vec<(u64, u64)> = Vec::new();
    for (s, e) in merged {
        match ranges.last_mut() {
            Some(l) if s <= l.1 + crate::time::PPQ => l.1 = l.1.max(e),
            _ => ranges.push((s, e)),
        }
    }
    SongDiff {
        ranges,
        whole,
        silent_only: false,
    }
}

// ---------------------------------------------------------------- 案の編集が触る所の指紋

fn fnv_hex(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// 指紋の鍵(ノートはクリップごと比べるので鍵にしない)
fn target_key(t: &crate::command::Target) -> Option<String> {
    use crate::command::Target as T;
    Some(match t {
        T::Track(id) => format!("track:{id}"),
        T::Clip(id) => format!("clip:{id}"),
        T::Effect(id) => format!("fx:{id}"),
        T::Asset(id) => format!("asset:{id}"),
        T::Tempo => "tempo".to_owned(),
        T::TimeSig => "time_sig".to_owned(),
        T::Master => "master".to_owned(),
        T::Meta => "meta".to_owned(),
        T::Sections => "sections".to_owned(),
        T::Note(_) => return None,
    })
}

/// 値の中の `locked`(音の固定)を外す。固定しただけでは中身は変わらないので、指紋に入れない
fn strip_locked(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(o) => {
            o.remove("locked");
            o.values_mut().for_each(strip_locked);
        }
        serde_json::Value::Array(a) => a.iter_mut().for_each(strip_locked),
        _ => {}
    }
}

/// 鍵の指す所の中身(無ければ None)
fn key_value(project: &Project, key: &str) -> Option<serde_json::Value> {
    let (kind, id) = key.split_once(':').unwrap_or((key, ""));
    let mut v = match kind {
        "track" => {
            let t = project.tracks.iter().find(|t| t.id.to_string() == id)?;
            let mut v = serde_json::to_value(t).ok()?;
            // トラックの設定だけ(クリップは別の鍵で比べる)
            v.as_object_mut()?.remove("clips");
            v
        }
        "clip" => serde_json::to_value(
            project
                .tracks
                .iter()
                .flat_map(|t| &t.clips)
                .find(|c| c.id.to_string() == id)?,
        )
        .ok()?,
        "fx" => serde_json::to_value(
            project
                .tracks
                .iter()
                .flat_map(|t| &t.effects)
                .chain(project.master.effects.iter())
                .find(|e| e.id.to_string() == id)?,
        )
        .ok()?,
        "asset" => serde_json::to_value(
            project
                .assets
                .iter()
                .find(|(k, _)| k.to_string() == id)
                .map(|(_, a)| a)?,
        )
        .ok()?,
        "tempo" => serde_json::to_value(&project.tempo_map).ok()?,
        "time_sig" => serde_json::to_value(&project.time_sig_map).ok()?,
        "master" => serde_json::to_value(&project.master).ok()?,
        "meta" => serde_json::to_value(&project.meta).ok()?,
        "sections" => serde_json::to_value(&project.sections).ok()?,
        _ => return None,
    };
    strip_locked(&mut v);
    Some(v)
}

fn key_digest(project: &Project, key: &str) -> String {
    key_value(project, key).map_or_else(|| "-".to_owned(), |v| fnv_hex(&v.to_string()))
}

/// 案を出すときに記録する、案の編集 `patch` が触る所の中身の指紋(まだ無い所は "-")
pub fn patch_base(
    project: &Project,
    patch: &[crate::command::Command],
) -> std::collections::BTreeMap<String, String> {
    patch
        .iter()
        .flat_map(|c| c.targets())
        .filter_map(|t| target_key(&t))
        .map(|k| {
            let d = key_digest(project, &k);
            (k, d)
        })
        .collect()
}

/// 指紋を記録した後に中身が変わった所(人に見せる名前。変わっていなければ空)
pub fn patch_base_changed(
    project: &Project,
    base: &std::collections::BTreeMap<String, String>,
) -> Vec<String> {
    base.iter()
        .filter(|(k, d)| key_digest(project, k) != **d)
        .map(|(k, _)| {
            let (kind, id) = k.split_once(':').unwrap_or((k, ""));
            match kind {
                "track" => {
                    let name = project
                        .tracks
                        .iter()
                        .find(|t| t.id.to_string() == id)
                        .map_or(id, |t| t.name.as_str());
                    crate::tr!("トラック「{name}」", "Track \"{name}\"")
                }
                "clip" => project
                    .tracks
                    .iter()
                    .find_map(|t| {
                        t.clips.iter().find(|c| c.id.to_string() == id).map(|c| {
                            crate::tr!(
                                "「{}」のクリップ「{}」",
                                "Clip \"{}\" on \"{}\"",
                                t.name,
                                c.name
                            )
                        })
                    })
                    .unwrap_or_else(|| crate::tr!("クリップ({id})", "Clip ({id})")),
                "fx" => t("エフェクト", "Effect").to_owned(),
                "asset" => t("素材", "Asset").to_owned(),
                "tempo" => t("テンポ", "Tempo").to_owned(),
                "time_sig" => t("拍子", "Time signature").to_owned(),
                "master" => t("マスター", "Master").to_owned(),
                "meta" => t("曲の情報", "Song info").to_owned(),
                "sections" => t("区間", "Sections").to_owned(),
                _ => k.clone(),
            }
        })
        .collect()
}

/// 計画から作った・AI が作ったクリップの状態
#[derive(Clone, Debug, Serialize)]
pub struct ClipView {
    pub clip_id: crate::id::ClipId,
    pub track: String,
    /// in_sync(計画どおり)/ ahead(計画が先に進んだ。作り直せる)/ missing(計画が無い)。計画の参照が無ければ省略
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<&'static str>,
    /// 人が手で直した小節(1 始まり、[最初, 最後])
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub edited_bars: Vec<[u32; 2]>,
    /// 固定の音の数
    #[serde(skip_serializing_if = "is_zero")]
    pub locked_notes: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// 計画と実際をまとめたもの
#[derive(Clone, Debug, Serialize)]
pub struct DesignView {
    /// 曲全体の計画(無ければ省略)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub song: Option<SongPlan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub song_plan_id: Option<PlanId>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub song_estimated: bool,
    pub sections: Vec<SectionView>,
    pub parts: Vec<PartView>,
    pub deviations: Vec<Deviation>,
    pub clips: Vec<ClipView>,
}

/// 区間の小節の範囲 [b0, b1)(0 始まり)と、その tick の範囲
struct Span {
    b0: usize,
    b1: usize,
    t0: u64,
    t1: u64,
}

fn percentile(sorted: &[u8], q: f64) -> u8 {
    if sorted.is_empty() {
        return 0;
    }
    let i = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

/// 存在の段階の表示名(人に見せる。PRESENCE と同じ並び。英語は画面の表示名と揃える)
fn presence_label(v: u8) -> &'static str {
    const EN: [&str; 6] = ["Off", "Hint", "Background", "Support", "Front", "Lead"];
    let i = (v as usize).min(PRESENCE.len() - 1);
    if is_en() {
        EN[i]
    } else {
        PRESENCE[i]
    }
}

fn note_name(p: u8) -> String {
    const N: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!("{}{}", N[(p % 12) as usize], p as i32 / 12 - 1)
}

fn is_drum(t: &Track) -> bool {
    t.device.as_ref().is_some_and(|d| d.source.is_drum_kit())
}

/// 計画と実際を並べ、ずれを出す
pub fn design_view(project: &Project, plans: &PlanSet) -> DesignView {
    let critique = crate::critique::critique(project);
    let end = project.end().0;
    let grid = crate::arrange::bar_grid(project, end.max(1));
    let bar_of = |t: u64| grid.partition_point(|(s, _)| *s <= t).saturating_sub(1);
    let song_bars = grid.iter().filter(|(s, _)| *s < end).count().max(1);
    let tick_of = |b: usize| grid.get(b).map_or(end, |g| g.0);
    let mut marks: Vec<_> = project.sections.iter().collect();
    marks.sort_by_key(|m| m.tick);
    // 区間が無い曲は曲全体を 1 区間として扱う
    let spans: Vec<Span> = if marks.is_empty() {
        vec![Span {
            b0: 0,
            b1: song_bars,
            t0: 0,
            t1: end,
        }]
    } else {
        marks
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let b0 = bar_of(m.tick.0);
                let b1 = marks
                    .get(i + 1)
                    .map_or(song_bars, |n| bar_of(n.tick.0))
                    .max(b0 + 1);
                Span {
                    b0,
                    b1,
                    t0: m.tick.0,
                    t1: marks.get(i + 1).map_or(tick_of(b1).max(end), |n| n.tick.0),
                }
            })
            .collect()
    };
    let section_key = |i: usize| -> String {
        marks
            .get(i)
            .map(|m| {
                m.id.as_ref()
                    .map_or_else(|| m.name.clone(), |id| id.to_string())
            })
            .unwrap_or_else(|| "song".to_owned())
    };
    let section_name = |i: usize| -> String {
        marks
            .get(i)
            .map_or_else(|| t("曲全体", "Whole song").to_owned(), |m| m.name.clone())
    };
    let sections: Vec<SectionView> = spans
        .iter()
        .enumerate()
        .map(|(i, s)| SectionView {
            id: marks
                .get(i)
                .and_then(|m| m.id.as_ref().map(|x| x.to_string())),
            name: section_name(i),
            start_bar: s.b0 + 1,
            bars: s.b1 - s.b0,
            planned: marks.get(i).and_then(|m| m.energy_mean()),
            curve: marks.get(i).map(|m| m.curve.clone()).unwrap_or_default(),
            join: marks.get(i).and_then(|m| m.join),
            measured: critique.sections.get(i).map_or(0.0, |c| c.energy),
        })
        .collect();

    // 計画(曲全体・パート)。曲全体は区間の設計を重ねるときと同じ選び方
    let current = crate::plan::current_song_plan(plans);
    let song_estimated = current.as_ref().is_some_and(|(p, _)| p.state.is_some());
    let song_plan_id = current.as_ref().map(|(p, _)| p.id.clone());
    // 区間ごとの設計は区間の欄(planned・curve・join・note)に並ぶので、曲全体の計画からは外して見せる(二重にしない)
    let song: Option<SongPlan> = current.map(|(_, s)| SongPlan {
        sections: vec![],
        ..s
    });
    let mut part_plans: Vec<(&PlanId, PartPlan, bool)> = Vec::new();
    for (id, p) in &plans.plans {
        // 案(枝)は今の計画ではないので並べない(案は計画の一覧から聴き比べて採用する)
        if p.state.as_deref() == Some("proposal") {
            continue;
        }
        let estimated = p.state.as_deref() == Some("estimated");
        if p.kind == "part" {
            if let Ok(pp) = serde_json::from_value::<PartPlan>(p.body.clone()) {
                part_plans.push((id, pp, estimated));
            }
        }
    }

    // パート × 区間を測る
    struct Raw {
        notes: usize,
        level: f64,
        pitches: Vec<u8>,
    }
    let mut parts: Vec<PartView> = Vec::new();
    let mut raws: Vec<Vec<Raw>> = Vec::new();
    for t in &project.tracks {
        if t.kind != crate::model::TrackKind::Midi {
            continue;
        }
        let mut per: Vec<Raw> = spans
            .iter()
            .map(|_| Raw {
                notes: 0,
                level: f64::NEG_INFINITY,
                pitches: vec![],
            })
            .collect();
        let mut vel_sum = vec![0f64; spans.len()];
        for c in &t.clips {
            for n in c.playback_notes() {
                let at = c.start.0 + n.pos.0;
                let Some(i) = spans.iter().position(|s| s.t0 <= at && at < s.t1) else {
                    continue;
                };
                per[i].notes += 1;
                per[i].pitches.push(n.pitch);
                vel_sum[i] += n.vel as f64;
            }
        }
        for (i, r) in per.iter_mut().enumerate() {
            r.pitches.sort_unstable();
            if r.notes > 0 && !t.mute {
                let mean_vel = vel_sum[i] / r.notes as f64;
                r.level = t.volume_db as f64 + 20.0 * (mean_vel / 127.0).max(1e-3).log10();
            }
        }
        // このトラックの計画(採用済みを先に)
        let plan = part_plans
            .iter()
            .filter(|(_, pp, _)| pp.track.as_deref() == Some(t.id.as_str()))
            .min_by_key(|(_, _, est)| *est);
        parts.push(PartView {
            track_id: t.id.clone(),
            name: t.name.clone(),
            color: t.color.clone(),
            plan_id: plan.map(|(id, _, _)| (*id).clone()),
            function: plan.and_then(|(_, pp, _)| pp.function.clone()),
            estimated: plan.is_some_and(|(_, _, e)| *e),
            cells: vec![],
        });
        raws.push(per);
    }
    // 区間ごとに、鳴っているトラックを音量の順に並べて段階を決める
    for (si, span) in spans.iter().enumerate() {
        let mut order: Vec<(usize, f64)> = raws
            .iter()
            .enumerate()
            .filter(|(_, r)| r[si].notes > 0 && r[si].level.is_finite())
            .map(|(pi, r)| (pi, r[si].level))
            .collect();
        order.sort_by(|a, b| b.1.total_cmp(&a.1));
        let bars = (span.b1 - span.b0).max(1) as f32;
        for (pi, part) in parts.iter_mut().enumerate() {
            let r = &raws[pi][si];
            let rank = order.iter().position(|(k, _)| *k == pi);
            let drum = project.track(&part.track_id).is_some_and(is_drum);
            let measured = match rank {
                None => 0,
                Some(_) if r.level < -30.0 => 1,
                Some(_) if r.level < -20.0 => 2,
                Some(0) if !drum => 5,
                Some(k) if k <= 2 => 4,
                Some(_) => 3,
            };
            let key = section_key(si);
            let plan = part_plans
                .iter()
                .filter(|(_, pp, _)| pp.track.as_deref() == Some(part.track_id.as_str()))
                .min_by_key(|(_, _, est)| *est)
                .and_then(|(_, pp, _)| pp.section(&key).cloned());
            part.cells.push(CellView {
                section: key,
                planned: plan.as_ref().map(|p| p.presence),
                function: plan.as_ref().and_then(|p| p.function.clone()),
                planned_register: plan.as_ref().and_then(|p| p.register),
                locked: plan.as_ref().is_some_and(|p| p.locked),
                measured,
                notes: r.notes,
                register: (!r.pitches.is_empty())
                    .then(|| [percentile(&r.pitches, 0.1), percentile(&r.pitches, 0.9)]),
                density: ((r.notes as f32 / bars / 16.0).min(1.0) * 100.0).round() / 100.0,
            });
        }
    }

    // ずれ
    let mut deviations = Vec::new();
    for (i, s) in sections.iter().enumerate() {
        if let Some(p) = s.planned {
            let d = s.measured - p as f64;
            if d.abs() >= 1.5 {
                deviations.push(Deviation {
                    severity: "warn",
                    section: Some(section_key(i)),
                    track: None,
                    what: if is_en() {
                        format!(
                            "Energy of \"{}\" is {} than planned (plan {:.1} / actual {:.1})",
                            s.name,
                            if d > 0.0 { "higher" } else { "lower" },
                            p,
                            s.measured
                        )
                    } else {
                        format!(
                            "「{}」の盛り上がりが計画より{}(計画 {:.1} / 実際 {:.1})",
                            s.name,
                            if d > 0.0 { "高い" } else { "低い" },
                            p,
                            s.measured
                        )
                    },
                    fix: t(
                        "計画の曲線を実際に合わせる(set_song_plan の energy・curve)か、パートの出入り・音の数を計画に合わせる",
                        "Match the planned curve to the actual one (energy/curve in set_song_plan), or match parts entering/leaving and note counts to the plan",
                    )
                    .to_owned(),
                });
            }
        }
    }
    for part in &parts {
        if part.estimated {
            continue;
        }
        for (i, c) in part.cells.iter().enumerate() {
            let Some(planned) = c.planned else {
                continue;
            };
            let at = |what: String, fix: &str, severity: &'static str| Deviation {
                severity,
                section: Some(c.section.clone()),
                track: Some(part.name.clone()),
                what,
                fix: fix.to_owned(),
            };
            let name = &sections[i].name;
            if planned == 0 && c.notes > 0 {
                deviations.push(at(
                    crate::tr!(
                        "「{name}」の {} は鳴らさない計画なのに鳴っている({} 音)",
                        "{} plays in \"{name}\" though planned to be off ({} notes)",
                        part.name,
                        c.notes
                    ),
                    t(
                        "計画の段階を上げるか、その区間の音を消す",
                        "Raise the planned level, or remove the notes in that section",
                    ),
                    "warn",
                ));
            } else if planned > 0 && c.notes == 0 {
                deviations.push(at(
                    crate::tr!(
                        "「{name}」の {} は「{}」の計画なのに鳴っていない",
                        "{} is silent in \"{name}\" though planned as \"{}\"",
                        part.name,
                        presence_label(planned)
                    ),
                    t(
                        "その区間に書く(write_* の道具)か、計画を「鳴らさない」にする",
                        "Write that section (write_* tools), or set the plan to \"Off\"",
                    ),
                    "warn",
                ));
            } else if planned > 0 && (planned as i32 - c.measured as i32).abs() >= 2 {
                deviations.push(at(
                    crate::tr!(
                        "「{name}」の {} は計画「{}」に対して、実際は「{}」くらい",
                        "{} in \"{name}\" is planned as \"{}\" but sounds about \"{}\"",
                        part.name,
                        presence_label(planned),
                        presence_label(c.measured)
                    ),
                    t(
                        "音量・音の数・音域で前後を整えるか、計画の段階を実際に合わせる",
                        "Adjust depth with volume, note count and register, or match the planned level to the actual",
                    ),
                    "info",
                ));
            }
            if let (Some([lo, hi]), Some([a, b])) = (c.planned_register, c.register) {
                if (a as i32) < lo as i32 - 2 || (b as i32) > hi as i32 + 2 {
                    deviations.push(at(
                        crate::tr!(
                            "「{name}」の {} が音域の帯から外れている(計画 {}〜{} / 実際 {}〜{})",
                            "{} in \"{name}\" is outside its register band (plan {}–{} / actual {}–{})",
                            part.name,
                            note_name(lo),
                            note_name(hi),
                            note_name(a),
                            note_name(b)
                        ),
                        t(
                            "音を帯の中へ移す(transpose_notes)か、計画の帯を広げる",
                            "Move the notes into the band (transpose_notes), or widen the planned band",
                        ),
                        "warn",
                    ));
                }
            }
        }
    }
    for (i, s) in sections.iter().enumerate() {
        let front = parts
            .iter()
            .filter(|p| !p.estimated)
            .filter(|p| {
                p.cells
                    .get(i)
                    .and_then(|c| c.planned)
                    .is_some_and(|v| v >= 4)
            })
            .count();
        if front >= 5 {
            deviations.push(Deviation {
                severity: "info",
                section: Some(section_key(i)),
                track: None,
                what: crate::tr!(
                    "「{}」で前面・主役の計画のパートが {front} つ(同時に前に出るのは 3〜4 つまでが目安)",
                    "{front} parts are planned as front/lead in \"{}\" (3–4 at once is a good maximum)",
                    s.name
                ),
                fix: t(
                    "いくつかを「支え」「背景」に下げる",
                    "Lower some of them to \"Support\" or \"Background\"",
                )
                .to_owned(),
            });
        }
    }

    let clips = clip_states(project, plans);

    DesignView {
        song,
        song_plan_id,
        song_estimated,
        sections,
        parts,
        deviations,
        clips,
    }
}

/// 計画から作った・AI が作ったクリップの状態(計画どおり / 先に進んだ・手で直した小節・固定の音の数)。
/// 何も無いクリップは含めない。タイムラインのクリップの印に使う(盛り上がりなどは測らないので軽い)
pub fn clip_states(project: &Project, plans: &PlanSet) -> Vec<ClipView> {
    let mut clips = Vec::new();
    for t in &project.tracks {
        for c in &t.clips {
            let locked_notes = c
                .notes()
                .map_or(0, |ns| ns.iter().filter(|n| n.locked).count());
            let plan = project
                .plan_refs
                .get(&c.id)
                .map(|r| match plans.plans.get(&r.id) {
                    None => "missing",
                    Some(p)
                        if p.digest() == r.digest || (r.digest.is_empty() && p.rev == r.rev) =>
                    {
                        "in_sync"
                    }
                    Some(_) => "ahead",
                });
            let edited_bars: Vec<[u32; 2]> = crate::made::edited_spans(project, &c.id)
                .into_iter()
                .map(|(a, b)| crate::made::bars_of(project, a, b))
                .collect();
            if plan.is_none() && edited_bars.is_empty() && locked_notes == 0 {
                continue;
            }
            clips.push(ClipView {
                clip_id: c.id.clone(),
                track: t.name.clone(),
                plan,
                edited_bars,
                locked_notes,
            });
        }
    }
    clips
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Command;
    use crate::history::Document;
    use crate::id::{ClipId, NoteId, SectionId};
    use crate::model::{Clip, Note, SectionMarker, TrackKind};
    use crate::plan::{Plan, PlanCommand};
    use crate::time::Tick;
    use serde_json::json;

    fn note(pos: u64, pitch: u8) -> Note {
        Note {
            id: NoteId::new(),
            pos: Tick(pos),
            dur: Tick(480),
            pitch,
            vel: 100,
            articulation: Default::default(),
            pitch_curve: vec![],
            glide_ms: None,
            vibrato: None,
            volume_curve: vec![],
            brightness_curve: vec![],
            condition: None,
            locked: false,
        }
    }

    #[test]
    fn plans_and_reality_are_compared() {
        let mut p = Project::new("t");
        let (s1, s2) = (SectionId::new(), SectionId::new());
        p.apply(&Command::SetSections {
            sections: vec![
                SectionMarker {
                    id: Some(s1.clone()),
                    tick: Tick(0),
                    name: "intro".into(),
                    energy: Some(2.0),
                    ..Default::default()
                },
                SectionMarker {
                    id: Some(s2.clone()),
                    tick: Tick(3840 * 4),
                    name: "drop".into(),
                    curve: vec![[0.0, 8.0], [1.0, 10.0]],
                    ..Default::default()
                },
            ],
        })
        .unwrap();
        let tid = TrackId::new();
        let mut t = Track::new(tid.clone(), "Lead", TrackKind::Midi);
        let mut c = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(3840 * 8));
        if let Some(ns) = c.notes_mut() {
            // イントロにも鳴っている(計画は鳴らさない)、ドロップは C5 付近(計画の帯は C3〜C4)
            ns.push(note(0, 60));
            for b in 4..8u64 {
                ns.push(note(b * 3840, 72));
                ns.push(note(b * 3840 + 960, 74));
            }
            ns[1].locked = true;
        }
        t.clips.push(c);
        p.tracks.push(t);
        let mut plans = PlanSet::default();
        plans
            .apply_command(&PlanCommand::Create {
                plan: Plan {
                    patch_base: Default::default(),
                    patch: vec![],
                    id: PlanId::new(),
                    name: "Lead".into(),
                    kind: "part".into(),
                    rev: 1,
                    derived_from: None,
                    state: None,
                    body: json!({
                        "track": tid.to_string(),
                        "function": "lead",
                        "sections": [
                            { "section": s1.to_string(), "presence": 0 },
                            { "section": s2.to_string(), "presence": 5, "register": [48, 60] }
                        ]
                    }),
                },
            })
            .unwrap();
        let v = design_view(&p, &plans);
        assert_eq!(v.sections.len(), 2);
        assert_eq!(v.sections[1].planned, Some(9.0));
        assert_eq!(v.parts.len(), 1);
        let cells = &v.parts[0].cells;
        assert_eq!(cells[0].planned, Some(0));
        assert_eq!(cells[0].notes, 1);
        assert_eq!(cells[1].measured, 5);
        assert_eq!(cells[1].register, Some([72, 74]));
        let text: Vec<&str> = v.deviations.iter().map(|d| d.what.as_str()).collect();
        assert!(
            text.iter()
                .any(|t| t.contains("鳴らさない計画なのに鳴っている")),
            "{text:?}"
        );
        assert!(
            text.iter().any(|t| t.contains("音域の帯から外れている")),
            "{text:?}"
        );
        // 固定の音のあるクリップは状態に出る
        assert_eq!(v.clips.len(), 1);
        assert_eq!(v.clips[0].locked_notes, 1);
        // 推定の計画はずれの基準にしない
        let id = v.parts[0].plan_id.clone().unwrap();
        let mut est = plans.plans[&id].clone();
        est.state = Some("estimated".into());
        plans
            .apply_command(&PlanCommand::Replace { plan: est })
            .unwrap();
        let v = design_view(&p, &plans);
        assert!(v.parts[0].estimated);
        assert!(
            v.deviations.iter().all(|d| d.track.is_none()),
            "{:?}",
            v.deviations
        );
    }

    #[test]
    fn song_diff_finds_changed_ranges_and_whole_song_changes() {
        let mut p = Project::new("t");
        let tid = TrackId::new();
        let mut t = Track::new(tid.clone(), "b", TrackKind::Midi);
        let mut c = Clip::new_midi(ClipId::new(), "c", Tick(3840), Tick(3840 * 4));
        if let Some(ns) = c.notes_mut() {
            ns.push(note(0, 40));
            ns.push(note(3840 * 2, 43));
        }
        t.clips.push(c);
        p.tracks.push(t);
        let mut q = p.clone();
        let n2 = q.tracks[0].clips[0].notes().unwrap()[1].id.clone();
        q.apply(&Command::UpdateNotes {
            clip: q.tracks[0].clips[0].id.clone(),
            changes: vec![crate::command::NoteChange::new(n2).pitch(31)],
        })
        .unwrap();
        let d = song_diff(&p, &q);
        assert_eq!(d.ranges, vec![(3840 * 3, 3840 * 3 + 480)]);
        assert!(!d.whole);
        // ID が変わっただけ(中身が同じ)なら違いなし
        assert!(song_diff(&p, &p.clone()).ranges.is_empty());
        // 音色・音量の違いは、そのトラックが鳴っている所(曲全体ではない)
        let mut v = p.clone();
        v.tracks[0].volume_db = -6.0;
        let d = song_diff(&p, &v);
        assert!(!d.whole);
        assert_eq!(
            d.ranges,
            vec![(3840, 3840 + 480), (3840 * 3, 3840 * 3 + 480)]
        );
        // 1 拍より近い音どうしの範囲はつなぐ
        let mut p2 = p.clone();
        if let Some(ns) = p2.tracks[0].clips[0].notes_mut() {
            ns.push(note(720, 45));
        }
        let mut v2 = p2.clone();
        v2.tracks[0].volume_db = -6.0;
        assert_eq!(
            song_diff(&p2, &v2).ranges,
            vec![(3840, 3840 + 720 + 480), (3840 * 3, 3840 * 3 + 480)]
        );
        // マスターの違いは曲全体
        let mut m = p.clone();
        m.master.volume_db = -3.0;
        assert!(song_diff(&p, &m).whole);
    }

    #[test]
    fn song_diff_ignores_tracks_that_are_not_heard() {
        let mut p = Project::new("t");
        for name in ["lead", "pad"] {
            let mut t = Track::new(TrackId::new(), name, TrackKind::Midi);
            let mut c = Clip::new_midi(ClipId::new(), "c", Tick(3840), Tick(3840 * 2));
            if let Some(ns) = c.notes_mut() {
                ns.push(note(0, 60));
            }
            t.clips.push(c);
            p.tracks.push(t);
        }
        p.tracks[0].mute = true;
        // ミュート中のトラックの音色を変えても、ミュートのトラックを足しても、鳴る音は変わらない
        let mut q = p.clone();
        q.tracks[0].volume_db = -6.0;
        let mut extra = p.tracks[0].clone();
        extra.id = TrackId::new();
        q.tracks.push(extra);
        let d = song_diff(&p, &q);
        assert!(d.ranges.is_empty() && !d.whole);
        assert!(d.silent_only);
        // ミュートを外すと、そのトラックが鳴る所が変わる
        let mut u = p.clone();
        u.tracks[0].mute = false;
        let d = song_diff(&p, &u);
        assert_eq!(d.ranges, vec![(3840, 3840 + 480)]);
        assert!(!d.silent_only);
        // ほかのトラックのソロ中は、ソロでないトラックの違いは聞こえない
        let mut s = p.clone();
        s.tracks[0].mute = false;
        s.tracks[0].solo = true;
        let mut s2 = s.clone();
        s2.tracks[1].volume_db = -6.0;
        let d = song_diff(&s, &s2);
        assert!(d.ranges.is_empty() && d.silent_only);
        // 同じ曲なら違いも印も無い
        let d = song_diff(&p, &p.clone());
        assert!(d.ranges.is_empty() && !d.whole && !d.silent_only);
        // 鳴っているトラックを消すと、その所が変わる
        let mut r = p.clone();
        r.tracks.remove(1);
        assert_eq!(song_diff(&p, &r).ranges, vec![(3840, 3840 + 480)]);
    }

    #[test]
    fn song_diff_sees_audio_clips() {
        let mut p = Project::new("t");
        let mut t = Track::new(TrackId::new(), "vo", TrackKind::Audio);
        let cid = ClipId::new();
        t.clips.push(Clip::new_audio(
            cid.clone(),
            "a",
            Tick(3840),
            Tick(3840 * 2),
            crate::id::AssetId::parse("sha256:ab12").unwrap(),
        ));
        p.tracks.push(t);
        let mut q = p.clone();
        q.apply(&Command::MoveClip {
            id: cid,
            start: Tick(3840 * 3),
            track: None,
        })
        .unwrap();
        let d = song_diff(&p, &q);
        assert!(!d.whole);
        // 前の位置と後の位置(続いているので 1 つにつなぐ)
        assert_eq!(d.ranges, vec![(3840, 3840 * 5)]);
        assert!(song_diff(&p, &p.clone()).ranges.is_empty());
    }

    #[test]
    fn patch_base_notices_edits_to_what_the_patch_touches() {
        let mut p = Project::new("t");
        let tid = TrackId::new();
        let mut t = Track::new(tid.clone(), "b", TrackKind::Midi);
        let (c1, c2) = (ClipId::new(), ClipId::new());
        for (id, at) in [(&c1, 0), (&c2, 3840 * 4)] {
            let mut c = Clip::new_midi(id.clone(), "c", Tick(at), Tick(3840 * 2));
            if let Some(ns) = c.notes_mut() {
                ns.push(note(0, 40));
            }
            t.clips.push(c);
        }
        p.tracks.push(t);
        let n1 = p.tracks[0].clips[0].notes().unwrap()[0].id.clone();
        let patch = vec![Command::UpdateNotes {
            clip: c1.clone(),
            changes: vec![crate::command::NoteChange::new(n1.clone()).pitch(28)],
        }];
        let base = patch_base(&p, &patch);
        assert!(base.contains_key(&format!("clip:{c1}")));
        // 別のクリップを直しても、固定しただけでも、案の触る所は変わらない
        let mut q = p.clone();
        let n2 = q.tracks[0].clips[1].notes().unwrap()[0].id.clone();
        q.apply(&Command::UpdateNotes {
            clip: c2,
            changes: vec![crate::command::NoteChange::new(n2).pitch(45)],
        })
        .unwrap();
        q.apply(&Command::UpdateNotes {
            clip: c1.clone(),
            changes: vec![crate::command::NoteChange::new(n1.clone()).locked(true)],
        })
        .unwrap();
        assert!(patch_base_changed(&q, &base).is_empty());
        // 案の触るクリップを人が直した → 変わった所として名前が出る
        q.apply(&Command::UpdateNotes {
            clip: c1,
            changes: vec![crate::command::NoteChange::new(n1).vel(60)],
        })
        .unwrap();
        assert_eq!(patch_base_changed(&q, &base), vec!["「b」のクリップ「c」"]);
    }
}
