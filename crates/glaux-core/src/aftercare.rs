//! 編集の後の確認(アフターケア)。編集の前と後の曲を比べ、変わったノートの周りで起きやすい問題を、
//! ノートの上だけで(音を描き出さずに)調べる。AI が曲の一部を直したとき、直した所の外への影響に気づけるように。
//!
//! 調べること:
//! - **ぶつかり**: 変えた音が、同じ時刻に鳴るほかのパートの音と半音(短 2 度・短 9 度)でぶつかる
//! - **調の外**: 変えた音が曲の調の外(長めの音だけ。意図した経過音もあるので知らせるだけ)
//! - **つなぎ目**: 変えた範囲の前後の小節との間で、音程が大きく跳ぶ・強さが段差になる
//! - **繰り返しの食い違い**: 変える前と同じ中身だった別のクリップ・同じ名前の区間が、古いまま残っている
//! - **はみ出し**: クリップの終わりを越える音(切れる・鳴らない)
//! - **厚み**: 変えた範囲の音の数が大きく増えた・減った(音量・ミックスの見直しの合図)
//! - **同じ音程の重なり**: 同じクリップの中で同じ音程の音が重なっている

use crate::id::{ClipId, TrackId};
use crate::model::{Clip, Project, Track};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// 知らせ 1 つ
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AftercareIssue {
    /// warn = 直した方がよい / info = 意図していなければ見直す
    pub severity: &'static str,
    /// clash / out_of_key / leap / velocity_step / stale_copy / stale_section / beyond_clip / denser / thinner / overlap
    pub kind: &'static str,
    pub track: String,
    /// 小節(1 始まり)
    pub bar: usize,
    pub message: String,
    pub hint: String,
}

/// 変わった範囲(トラックごと)
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChangedRange {
    pub track_id: String,
    pub track: String,
    pub from_bar: usize,
    pub to_bar: usize,
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
}

/// 確認の結果
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Aftercare {
    pub ranges: Vec<ChangedRange>,
    /// 調べた項目(報告に使う)
    pub checked: Vec<&'static str>,
    pub issues: Vec<AftercareIssue>,
    /// 知らせが多すぎて省いた数
    #[serde(skip_serializing_if = "is_zero")]
    pub omitted: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

impl Aftercare {
    /// 変わったノートが無い
    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    pub fn warn_count(&self) -> usize {
        self.issues.iter().filter(|i| i.severity == "warn").count()
    }
}

/// 知らせの上限(多すぎると読まれない)
const MAX_ISSUES: usize = 16;

/// 鳴る音 1 つ(絶対 tick)
#[derive(Clone, Copy, Debug, PartialEq)]
struct Sound {
    start: u64,
    end: u64,
    pitch: u8,
    vel: u8,
}

fn is_drum(t: &Track) -> bool {
    t.device.as_ref().is_some_and(|d| d.source.is_drum_kit())
}

/// クリップの鳴る音(ループは展開)
fn sounds(clip: &Clip) -> Vec<Sound> {
    clip.playback_notes()
        .iter()
        .map(|n| Sound {
            start: clip.start.0 + n.pos.0,
            end: clip.start.0 + n.pos.0 + n.dur.0.max(1),
            pitch: n.pitch,
            vel: n.vel,
        })
        .collect()
}

/// クリップの中身の指紋(相対位置・長さ・音程・強さ・奏法。クリップの長さとループも)
fn content_key(clip: &Clip) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let notes = clip.notes()?;
    if notes.is_empty() {
        return None;
    }
    let mut v: Vec<(u64, u64, u8, u8, String)> = notes
        .iter()
        .map(|n| {
            (
                n.pos.0,
                n.dur.0,
                n.pitch,
                n.vel,
                format!("{:?}", n.articulation),
            )
        })
        .collect();
    v.sort();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut h);
    clip.length.0.hash(&mut h);
    Some(h.finish())
}

/// 小節の表(小節の頭の tick)
struct Bars(Vec<u64>);

impl Bars {
    fn new(project: &Project, end: u64) -> Self {
        Bars(
            crate::arrange::bar_grid(project, end + crate::time::PPQ * 8)
                .iter()
                .map(|(s, _)| *s)
                .collect(),
        )
    }

    /// 1 始まりの小節番号
    fn bar(&self, tick: u64) -> usize {
        self.0.partition_point(|s| *s <= tick).max(1)
    }

    fn start(&self, bar: usize) -> u64 {
        self.0.get(bar.saturating_sub(1)).copied().unwrap_or(0)
    }
}

/// 区間の名前から番号・空白を除いたもの(「サビ 2」と「サビ」を同じ種類とみなす)
fn section_kind(name: &str) -> String {
    name.trim()
        .trim_end_matches(|c: char| {
            c.is_ascii_digit() || c.is_whitespace() || "０１２３４５６７８９".contains(c)
        })
        .to_lowercase()
}

/// 編集の前 `before` と後 `after` を比べて確かめる。`clips` を渡すとそのクリップだけを比べる(無ければ全部)
pub fn check(before: &Project, after: &Project, clips: Option<&[ClipId]>) -> Aftercare {
    let mut out = Aftercare::default();
    let only: Option<HashSet<&ClipId>> = clips.map(|c| c.iter().collect());
    let want = |id: &ClipId| only.as_ref().map_or(true, |s| s.contains(id));

    // ---- 変わったクリップと、トラックごとの変わったノート ----
    let index = |p: &Project| -> HashMap<ClipId, (usize, Clip)> {
        let mut m = HashMap::new();
        for (ti, t) in p.tracks.iter().enumerate() {
            for c in &t.clips {
                m.insert(c.id.clone(), (ti, c.clone()));
            }
        }
        m
    };
    let (bi, ai) = (index(before), index(after));
    let mut ids: Vec<ClipId> = bi
        .keys()
        .chain(ai.keys())
        .filter(|id| want(id))
        .cloned()
        .collect();
    ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    ids.dedup();

    struct TrackChange {
        added: Vec<Sound>,
        removed: Vec<Sound>,
        changed: usize,
        clips_after: Vec<ClipId>,
        clips_before_keys: Vec<(ClipId, u64)>,
    }
    let mut per_track: HashMap<TrackId, TrackChange> = HashMap::new();
    for id in &ids {
        let b = bi.get(id);
        let a = ai.get(id);
        let same = match (b, a) {
            (Some((_, cb)), Some((_, ca))) => cb == ca,
            _ => false,
        };
        if same {
            continue;
        }
        let (Some(track), sb, sa) = (
            a.map(|(ti, _)| &after.tracks[*ti])
                .or_else(|| b.map(|(ti, _)| &before.tracks[*ti])),
            b.map(|(_, c)| sounds(c)).unwrap_or_default(),
            a.map(|(_, c)| sounds(c)).unwrap_or_default(),
        ) else {
            continue;
        };
        if a.is_some_and(|(_, c)| c.notes().is_none())
            && b.map_or(true, |(_, c)| c.notes().is_none())
        {
            continue; // 音声クリップ
        }
        let e = per_track
            .entry(track.id.clone())
            .or_insert_with(|| TrackChange {
                added: vec![],
                removed: vec![],
                changed: 0,
                clips_after: vec![],
                clips_before_keys: vec![],
            });
        // 鳴る音で比べる(ループの繰り返しも含めて、増えた音・消えた音)
        let mut rest: Vec<Sound> = sb.clone();
        for s in &sa {
            if let Some(i) = rest.iter().position(|x| x == s) {
                rest.swap_remove(i);
            } else {
                e.added.push(*s);
            }
        }
        e.removed.extend(rest);
        if let (Some((_, cb)), Some((_, ca))) = (b, a) {
            let nb: HashMap<_, _> = cb
                .notes()
                .unwrap_or(&[])
                .iter()
                .map(|n| (n.id.clone(), n))
                .collect();
            e.changed += ca
                .notes()
                .unwrap_or(&[])
                .iter()
                .filter(|n| nb.get(&n.id).is_some_and(|o| *o != *n))
                .count();
        }
        if a.is_some() {
            e.clips_after.push(id.clone());
        }
        if let Some((_, cb)) = b {
            if let Some(k) = content_key(cb) {
                e.clips_before_keys.push((id.clone(), k));
            }
        }
    }
    if per_track.is_empty() {
        return out;
    }

    let end = after
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .map(|c| c.start.0 + c.length.0)
        .chain(
            before
                .tracks
                .iter()
                .flat_map(|t| t.clips.iter())
                .map(|c| c.start.0 + c.length.0),
        )
        .max()
        .unwrap_or(0);
    let bars = Bars::new(after, end);
    out.checked = vec![
        "clash",
        "out_of_key",
        "boundary",
        "stale_copy",
        "beyond_clip",
        "density",
        "overlap",
    ];

    // 曲の調(変えた後)
    let key = crate::harmony::analyze(after, None, None).key;
    let scale: Option<[bool; 12]> = key.as_ref().map(|k| {
        let mut s = [false; 12];
        for pc in crate::harmony::scale_pitch_classes(k.tonic, k.mode) {
            s[pc as usize % 12] = true;
        }
        s
    });

    // 各トラックの鳴る音(変えた後・前)
    let all_sounds = |p: &Project| -> HashMap<TrackId, Vec<Sound>> {
        p.tracks
            .iter()
            .map(|t| (t.id.clone(), t.clips.iter().flat_map(sounds).collect()))
            .collect()
    };
    let (sounds_after, sounds_before) = (all_sounds(after), all_sounds(before));
    let track_after = |id: &TrackId| after.tracks.iter().find(|t| &t.id == id);
    let mut issues: Vec<AftercareIssue> = Vec::new();
    let mut seen: HashSet<(String, &'static str, usize)> = HashSet::new();
    let mut push = |i: AftercareIssue| {
        if seen.insert((i.track.clone(), i.kind, i.bar)) {
            issues.push(i);
        }
    };

    let mut order: Vec<&TrackId> = per_track.keys().collect();
    order.sort_by_key(|id| {
        after
            .tracks
            .iter()
            .position(|t| &t.id == *id)
            .unwrap_or(usize::MAX)
    });
    for tid in order {
        let ch = &per_track[tid];
        let Some(track) = track_after(tid).or_else(|| before.tracks.iter().find(|t| &t.id == tid))
        else {
            continue;
        };
        let drum = is_drum(track);
        let ticks: Vec<u64> = ch
            .added
            .iter()
            .chain(&ch.removed)
            .flat_map(|s| [s.start, s.end.saturating_sub(1)])
            .collect();
        let (Some(&lo), Some(&hi)) = (ticks.iter().min(), ticks.iter().max()) else {
            continue;
        };
        let (from_bar, to_bar) = (bars.bar(lo), bars.bar(hi));
        out.ranges.push(ChangedRange {
            track_id: tid.to_string(),
            track: track.name.clone(),
            from_bar,
            to_bar,
            added: ch.added.len(),
            removed: ch.removed.len(),
            changed: ch.changed,
        });
        let name = track.name.clone();
        let mine = sounds_after.get(tid).cloned().unwrap_or_default();

        // ---- ぶつかり・調の外(変えて増えた音だけ) ----
        if !drum {
            for s in &ch.added {
                for other in after
                    .tracks
                    .iter()
                    .filter(|t| &t.id != tid && !is_drum(t) && !t.mute)
                {
                    let Some(os) = sounds_after.get(&other.id) else {
                        continue;
                    };
                    let hit = os.iter().find(|o| {
                        let overlap = s.end.min(o.end).saturating_sub(s.start.max(o.start));
                        let d = (s.pitch as i32 - o.pitch as i32).abs();
                        overlap >= 240 && (d == 1 || d == 13)
                    });
                    if let Some(o) = hit {
                        push(AftercareIssue {
                            severity: "warn",
                            kind: "clash",
                            track: name.clone(),
                            bar: bars.bar(s.start),
                            message: format!(
                                "{} の音 {} が「{}」の {} と半音でぶつかる",
                                name,
                                pitch_name(s.pitch),
                                other.name,
                                pitch_name(o.pitch)
                            ),
                            hint: "意図した濁りでなければ、どちらかを和音の音に寄せる(analyze_harmony でその小節の和音を見る)"
                                .to_owned(),
                        });
                    }
                }
                if let Some(sc) = &scale {
                    if s.end - s.start >= 480 && !sc[(s.pitch % 12) as usize] {
                        push(AftercareIssue {
                            severity: "info",
                            kind: "out_of_key",
                            track: name.clone(),
                            bar: bars.bar(s.start),
                            message: format!(
                                "{} が曲の調({})の外",
                                pitch_name(s.pitch),
                                key.as_ref().map_or("", |k| k.name.as_str())
                            ),
                            hint: "転調・借用和音のつもりでなければ、調の音に寄せる".to_owned(),
                        });
                    }
                }
            }
        }

        // ---- つなぎ目(変えた範囲の頭と終わり) ----
        let range_lo = bars.start(from_bar);
        let range_hi = bars.start(to_bar + 1).max(range_lo + 1);
        let window = crate::time::PPQ * 4;
        let top_line = |v: &[Sound]| -> Vec<Sound> {
            // いちばん上の線(和音は最高音)で見る
            let mut by_start: HashMap<u64, Sound> = HashMap::new();
            for s in v {
                by_start
                    .entry(s.start)
                    .and_modify(|x| {
                        if s.pitch > x.pitch {
                            *x = *s
                        }
                    })
                    .or_insert(*s);
            }
            let mut l: Vec<Sound> = by_start.into_values().collect();
            l.sort_by_key(|s| s.start);
            l
        };
        let line = top_line(&mine);
        let edge = |at: u64| -> Option<(Sound, Sound)> {
            let prev = line
                .iter()
                .rev()
                .find(|s| s.start < at && s.end + window >= at)?;
            let next = line
                .iter()
                .find(|s| s.start >= at && s.start < at + window)?;
            Some((*prev, *next))
        };
        for (at, label) in [(range_lo, "頭"), (range_hi, "終わり")] {
            // 前と同じつなぎ目なら知らせない
            let before_line = top_line(sounds_before.get(tid).map_or(&[][..], |v| v.as_slice()));
            let same_before = |p: &Sound, n: &Sound| {
                before_line.iter().any(|s| s == p) && before_line.iter().any(|s| s == n)
            };
            let Some((p, n)) = edge(at) else {
                continue;
            };
            if same_before(&p, &n) {
                continue;
            }
            let leap = (n.pitch as i32 - p.pitch as i32).abs();
            if !drum && leap > 12 {
                push(AftercareIssue {
                    severity: "info",
                    kind: "leap",
                    track: name.clone(),
                    bar: bars.bar(at),
                    message: format!(
                        "直した範囲の{label}で {leap} 半音跳ぶ({} → {})",
                        pitch_name(p.pitch),
                        pitch_name(n.pitch)
                    ),
                    hint: "前後の小節とつながるよう、オクターブを寄せるか、つなぎの音を足す"
                        .to_owned(),
                });
            }
            let step = (n.vel as i32 - p.vel as i32).abs();
            if step > 35 {
                push(AftercareIssue {
                    severity: "info",
                    kind: "velocity_step",
                    track: name.clone(),
                    bar: bars.bar(at),
                    message: format!(
                        "直した範囲の{label}で強さが {} → {} と段差になる",
                        p.vel, n.vel
                    ),
                    hint: "前後の強さに合わせる(scale_velocity)か、意図した変化なら そのまま"
                        .to_owned(),
                });
            }
        }

        // ---- 繰り返しの食い違い(同じ中身だったクリップ) ----
        let changed_ids: HashSet<&ClipId> = per_track
            .values()
            .flat_map(|c| c.clips_after.iter())
            .collect();
        for (cid, k) in &ch.clips_before_keys {
            for t in &after.tracks {
                for c in &t.clips {
                    if &c.id == cid || changed_ids.contains(&c.id) {
                        continue;
                    }
                    if content_key(c) == Some(*k) {
                        push(AftercareIssue {
                            severity: "warn",
                            kind: "stale_copy",
                            track: t.name.clone(),
                            bar: bars.bar(c.start.0),
                            message: format!(
                                "直す前と同じ中身だったクリップ「{}」({} 小節目)は古いまま",
                                c.name,
                                bars.bar(c.start.0)
                            ),
                            hint: "繰り返しにも同じ直しを当てるなら、直したクリップを duplicate_clips で置き直すか同じ編集をする。\
                                   わざと変えたなら そのまま"
                                .to_owned(),
                        });
                    }
                }
            }
        }

        // ---- 同じ名前の区間(サビ・サビ 2 など)の食い違い ----
        let mut secs: Vec<(u64, u64, String)> = Vec::new();
        let mut sorted = after.sections.clone();
        sorted.sort_by_key(|s| s.tick);
        for (i, s) in sorted.iter().enumerate() {
            let e = sorted
                .get(i + 1)
                .map_or(end.max(s.tick.0 + 1), |n| n.tick.0);
            secs.push((s.tick.0, e, section_kind(&s.name)));
        }
        let rel = |v: &[Sound], a: u64, b: u64| -> Vec<(u64, u64, u8, u8)> {
            let mut r: Vec<_> = v
                .iter()
                .filter(|s| s.start >= a && s.start < b)
                .map(|s| (s.start - a, s.end - s.start, s.pitch, s.vel))
                .collect();
            r.sort();
            r
        };
        let before_mine = sounds_before.get(tid).cloned().unwrap_or_default();
        for (a, b, kind) in &secs {
            if kind.is_empty() || !(lo < *b && hi >= *a) {
                continue;
            }
            let old = rel(&before_mine, *a, *b);
            if old.is_empty() || old == rel(&mine, *a, *b) {
                continue;
            }
            for (a2, b2, k2) in &secs {
                if (a2, b2) == (a, b) || k2 != kind || (b2 - a2) != (b - a) {
                    continue;
                }
                // 別の区間が、直す前のこの区間と同じ中身のまま(しかも今は違う)
                if rel(&mine, *a2, *b2) == old {
                    push(AftercareIssue {
                        severity: "warn",
                        kind: "stale_section",
                        track: name.clone(),
                        bar: bars.bar(*a2),
                        message: format!(
                            "同じ種類の区間({} 小節目から)の{}は、直す前と同じ中身のまま",
                            bars.bar(*a2),
                            name
                        ),
                        hint: "その区間にも同じ直しを当てるか、わざと変えたなら そのまま"
                            .to_owned(),
                    });
                }
            }
        }

        // ---- はみ出し・同じ音程の重なり(変えたクリップ) ----
        for cid in &ch.clips_after {
            let Some((_, c)) = ai.get(cid) else {
                continue;
            };
            let Some(notes) = c.notes() else {
                continue;
            };
            if c.loop_len().is_none() {
                let over = notes
                    .iter()
                    .filter(|n| n.pos.0 + n.dur.0 > c.length.0)
                    .count();
                if over > 0 {
                    push(AftercareIssue {
                        severity: "warn",
                        kind: "beyond_clip",
                        track: name.clone(),
                        bar: bars.bar(c.start.0 + c.length.0),
                        message: format!(
                            "クリップ「{}」の終わりを越える音が {over} 個(越えた所は鳴らない)",
                            c.name
                        ),
                        hint: "クリップを伸ばす(resize_clip)か、音を短くする".to_owned(),
                    });
                }
            }
            let mut v: Vec<&crate::model::Note> = notes.iter().collect();
            v.sort_by_key(|n| (n.pitch, n.pos.0));
            if let Some(w) = v
                .windows(2)
                .find(|w| w[0].pitch == w[1].pitch && w[0].pos.0 + w[0].dur.0 > w[1].pos.0)
            {
                push(AftercareIssue {
                    severity: "info",
                    kind: "overlap",
                    track: name.clone(),
                    bar: bars.bar(c.start.0 + w[1].pos.0),
                    message: format!("同じ音程 {} の音が重なっている", pitch_name(w[1].pitch)),
                    hint: "前の音を短くする(重なると音が切れたり二重に鳴ったりする)".to_owned(),
                });
            }
        }

        // ---- 厚み(変えた範囲の音の数) ----
        let count = |v: &[Sound]| {
            v.iter()
                .filter(|s| s.start >= range_lo && s.start < range_hi)
                .count()
        };
        let (nb, na) = (count(&before_mine), count(&mine));
        if na >= nb + 8 && na * 2 >= nb * 3 {
            push(AftercareIssue {
                severity: "info",
                kind: "denser",
                track: name.clone(),
                bar: from_bar,
                message: format!("直した範囲の音が {nb} → {na} 個に増えた"),
                hint: "厚くなった分、音量・ほかのパートとの住み分けを見直す(critique_mix・compare_mix)".to_owned(),
            });
        } else if nb >= na + 8 && nb >= na * 2 {
            push(AftercareIssue {
                severity: "info",
                kind: "thinner",
                track: name.clone(),
                bar: from_bar,
                message: format!("直した範囲の音が {nb} → {na} 個に減った"),
                hint: "薄くなった分、区間の盛り上がり(critique_arrangement の energy)が計画に合うか見る".to_owned(),
            });
        }
    }
    // warn を先に、小節順に
    issues.sort_by_key(|i| (i.severity != "warn", i.bar));
    if issues.len() > MAX_ISSUES {
        out.omitted = issues.len() - MAX_ISSUES;
        issues.truncate(MAX_ISSUES);
    }
    out.issues = issues;
    out
}

/// 音名(C4 = 60)
pub fn pitch_name(p: u8) -> String {
    const N: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!("{}{}", N[(p % 12) as usize], p as i32 / 12 - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::NoteId;
    use crate::model::{ClipContent, Note, Track, TrackKind};
    use crate::time::Tick;

    fn note(pos: u64, dur: u64, pitch: u8) -> Note {
        Note {
            id: NoteId::new(),
            pos: Tick(pos),
            dur: Tick(dur),
            pitch,
            vel: 100,
            articulation: Default::default(),
            pitch_curve: vec![],
            glide_ms: None,
            vibrato: None,
            volume_curve: vec![],
            brightness_curve: vec![],
            condition: None,
        }
    }

    fn clip(name: &str, start: u64, notes: Vec<Note>) -> Clip {
        let mut c = Clip::new_midi(ClipId::new(), name, Tick(start), Tick(3840));
        if let ClipContent::Midi { notes: n, .. } = &mut c.content {
            *n = notes;
        }
        c
    }

    /// コード(C・E・G を 4 小節)と旋律 2 本(同じ中身のクリップが 1 小節目と 3 小節目)
    fn song() -> Project {
        let mut p = Project::new("t");
        let mut chords = Track::new(TrackId::new(), "Chords", TrackKind::Midi);
        chords.clips.push(clip(
            "c",
            0,
            vec![note(0, 3840, 60), note(0, 3840, 64), note(0, 3840, 67)],
        ));
        chords.clips[0].length = Tick(3840 * 4);
        chords.clips[0]
            .notes_mut()
            .unwrap()
            .iter_mut()
            .for_each(|n| n.dur = Tick(3840 * 4));
        let mut lead = Track::new(TrackId::new(), "Lead", TrackKind::Midi);
        let phrase = || vec![note(0, 960, 72), note(960, 960, 74), note(1920, 1920, 76)];
        lead.clips.push(clip("A", 0, phrase()));
        lead.clips.push(clip("A'", 7680, phrase()));
        p.tracks = vec![chords, lead];
        p
    }

    #[test]
    fn unchanged_project_has_nothing_to_say() {
        let p = song();
        let a = check(&p, &p, None);
        assert!(a.is_empty() && a.issues.is_empty());
    }

    #[test]
    fn a_new_clash_and_a_stale_copy_are_reported() {
        let before = song();
        let mut after = before.clone();
        // 1 小節目の旋律の 2 つ目を D → C#(コードの C・E と半音でぶつかる)
        after.tracks[1].clips[0].notes_mut().unwrap()[1].pitch = 73;
        let a = check(&before, &after, None);
        assert_eq!(a.ranges.len(), 1);
        assert_eq!(
            (a.ranges[0].track.as_str(), a.ranges[0].from_bar),
            ("Lead", 1)
        );
        assert!(a.issues.iter().any(|i| i.kind == "clash"), "{:?}", a.issues);
        // 3 小節目の同じ中身のクリップは古いまま
        let stale: Vec<_> = a.issues.iter().filter(|i| i.kind == "stale_copy").collect();
        assert_eq!(stale.len(), 1, "{:?}", a.issues);
        assert_eq!(stale[0].bar, 3);
        assert!(a.warn_count() >= 2);
        // 両方に当てれば繰り返しの知らせは消える
        after.tracks[1].clips[1].notes_mut().unwrap()[1].pitch = 73;
        let a = check(&before, &after, None);
        assert!(!a.issues.iter().any(|i| i.kind == "stale_copy"));
    }

    #[test]
    fn boundary_leaps_and_beyond_clip_are_reported() {
        let before = song();
        let mut after = before.clone();
        // 3 小節目の頭を 2 オクターブ上げ、最後の音をクリップの外へ伸ばす
        {
            let n = after.tracks[1].clips[1].notes_mut().unwrap();
            n[0].pitch = 96;
            n[2].dur = Tick(3840);
        }
        let a = check(&before, &after, None);
        assert!(a.issues.iter().any(|i| i.kind == "leap"), "{:?}", a.issues);
        assert!(
            a.issues.iter().any(|i| i.kind == "beyond_clip"),
            "{:?}",
            a.issues
        );
    }

    #[test]
    fn denser_ranges_and_stale_sections_are_reported() {
        let mut before = song();
        before.sections = vec![
            crate::model::SectionMarker {
                tick: Tick(0),
                name: "サビ".into(),
                energy: None,
                tracks: vec![],
                note: None,
            },
            crate::model::SectionMarker {
                tick: Tick(7680),
                name: "サビ 2".into(),
                energy: None,
                tracks: vec![],
                note: None,
            },
        ];
        // 区間ごとの比較にするため、旋律を 1 本のクリップにまとめる
        let mut after = before.clone();
        // 1 小節目に 16 分を 12 個足す
        let added: Vec<Note> = (0..12).map(|i| note(i * 240, 120, 79)).collect();
        after.tracks[1].clips[0].notes_mut().unwrap().extend(added);
        let a = check(&before, &after, None);
        assert!(
            a.issues.iter().any(|i| i.kind == "denser"),
            "{:?}",
            a.issues
        );
        assert!(
            a.issues
                .iter()
                .any(|i| i.kind == "stale_section" && i.bar == 3),
            "{:?}",
            a.issues
        );
    }

    #[test]
    fn big_songs_stay_fast() {
        // 12 トラック × 16 クリップ × 64 音(約 1 万 2 千音)。1 クリップを丸ごと書き換える
        let mut p = Project::new("big");
        p.sections = (0..8)
            .map(|i| crate::model::SectionMarker {
                tick: Tick(i * 3840 * 8),
                name: if i % 2 == 0 {
                    "A".into()
                } else {
                    "サビ".into()
                },
                energy: None,
                tracks: vec![],
                note: None,
            })
            .collect();
        for t in 0..12u8 {
            let mut tr = Track::new(TrackId::new(), format!("T{t}"), TrackKind::Midi);
            for c in 0..16u64 {
                let notes = (0..64u64)
                    .map(|i| note(i * 240, 240, 48 + t * 2 + (i % 12) as u8))
                    .collect();
                tr.clips.push(clip("c", c * 3840 * 4, notes));
                tr.clips.last_mut().unwrap().length = Tick(3840 * 4);
            }
            p.tracks.push(tr);
        }
        let mut after = p.clone();
        for n in after.tracks[3].clips[5].notes_mut().unwrap() {
            n.pitch += 1;
        }
        let ids = vec![after.tracks[3].clips[5].id.clone()];
        let t0 = std::time::Instant::now();
        let a = check(&p, &after, Some(&ids));
        let dt = t0.elapsed();
        assert!(!a.is_empty());
        assert!(dt.as_millis() < 1500, "{dt:?}");
        eprintln!("1 万 2 千音の曲で {dt:?}");
    }

    #[test]
    fn only_listed_clips_are_compared() {
        let before = song();
        let mut after = before.clone();
        after.tracks[1].clips[1].notes_mut().unwrap()[0].pitch = 71;
        let other = after.tracks[1].clips[0].id.clone();
        assert!(check(&before, &after, Some(&[other])).is_empty());
    }
}
