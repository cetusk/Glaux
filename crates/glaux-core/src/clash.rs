//! 音のぶつかり(トラックの間で同時に鳴る音の濁り)を、ノートの上で(音を描き出さずに)調べる。
//! 人(ピアノロール・タイムラインの印)と AI(MCP の `check_clashes`)が同じ判定を使う。
//!
//! 調べること:
//! - **半音のぶつかり**: 別のトラックの音と短 2 度・短 9 度で重なる(伴奏と旋律がぶつかって変な和音に聞こえる、の元)
//! - **低い音域の渋滞**: C3 より下で、別のトラックの音と 3 半音以内で重なる(濁って音程が分からなくなる)
//! - **耳ざわりな所**: 鳴っている音どうしの「うなり」(Plomp-Levelt の不協和の曲線を倍音ごとに足した粗さ)が、
//!   曲のふつうの所よりはっきり大きい所。和音の名前に表れない濁り(音域・積み方・重ね方)も拾う
//!
//! ぶつかりごとに、その所の和音(鳴っている音全部から推定)と、どちらの音が和音の外かを添える。
//! 直すなら和音の外の方を動かすのが自然なため。意図した不協和(経過音・テンション)もあるので、短い音は info にとどめる。

use crate::harmony::{chord_from_histogram, chord_pitch_classes, key_from_histogram};
use crate::id::{NoteId, TrackId};
use crate::model::{ClipContent, Project, Track};
use crate::time::PPQ;
use crate::tr;
use serde::Serialize;
use std::collections::HashMap;

/// 一緒に鳴っているとみなす最短の重なり(16 分音符)
const MIN_OVERLAP: u64 = PPQ / 4;
/// 低い音域の境(C3)
const LOW_LIMIT: u8 = 48;
/// 知らせの上限(同じ組の繰り返しはまとめたうえで)
const MAX_CLASHES: usize = 80;
const MAX_HARSH: usize = 24;
/// 粗さを測る刻み(16 分音符)
const SLICE: u64 = PPQ / 4;

/// 調べ方
#[derive(Clone, Debug, Default)]
pub struct ClashOptions {
    /// 調べるトラック(省略で全部。ドラムは常に除く)
    pub tracks: Option<Vec<TrackId>>,
    /// 範囲(tick、終わりは含まない)
    pub range: Option<(u64, u64)>,
    /// ミュートのトラックも入れる(既定は聞こえている音だけ)
    pub include_muted: bool,
    /// まとめた組の、出てくる所すべて(`Clash::spans`)も返す(画面の印に使う。AI には要らない)
    pub spans: bool,
}

/// ぶつかりが出てくる所 1 つ(画面の印用)
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ClashSpan {
    pub tick: u64,
    pub len: u64,
    pub a_clip: String,
    pub b_clip: String,
}

/// ぶつかりの片方の音
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ClashNote {
    pub track_id: String,
    pub track: String,
    pub clip_id: String,
    pub note_id: String,
    pub pitch: u8,
    /// 例: "F#4"
    pub name: String,
    /// 曲の頭からの tick
    pub start: u64,
    pub end: u64,
}

/// ぶつかり 1 組(同じトラック・同じ高さの組の繰り返しは 1 つにまとめ、`bars` に並べる)
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Clash {
    /// warn = 聞いて分かる濁り(長く重なる・拍の頭)/ info = 短い経過の重なり
    pub severity: &'static str,
    /// semitone(短 2 度)/ minor_ninth(短 9 度)/ low_mud(低い音域の渋滞)
    pub kind: &'static str,
    /// 最初の所: 小節(1 始まり)と拍(1 始まり、小数で拍の中)
    pub bar: usize,
    pub beat: f64,
    /// 最初の所の重なりの頭と長さ(tick)
    pub tick: u64,
    pub len: u64,
    /// 動かす候補を先に(和音の外の音・旋律側)
    pub a: ClashNote,
    pub b: ClashNote,
    /// その所の和音の推定(伴奏 = 和音を弾くトラックとベースから。伴奏が無い所は鳴っている音全部から)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chord: Option<String>,
    /// 和音の外の音: "a" / "b" / "both"(両方とも和音の音なら None = 積み方の問題)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub off_chord: Option<&'static str>,
    /// この 2 音のうなりの強さ(0〜1。1 = 中音域で同じ強さの短 2 度くらい)
    pub roughness: f64,
    /// 同じ組が出てくる回数と小節(最大 32 個)
    pub count: usize,
    pub bars: Vec<usize>,
    /// 出てくる所すべて(`ClashOptions::spans` のときだけ。最大 256)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<ClashSpan>,
    /// 人に見せる文(言語の設定に従う)
    pub message: String,
    pub hint: String,
}

/// 耳ざわりな所(粗さが曲のふつうの所より大きい区間)
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HarshSpot {
    pub bar: usize,
    pub beat: f64,
    pub tick: u64,
    pub len: u64,
    /// 粗さ(曲の中央値に対する倍率)
    pub relative: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chord: Option<String>,
    /// 鳴っている音(「トラック 音名」)
    pub notes: Vec<String>,
    /// いちばん粗さを生んでいる 2 音(「トラック 音名」)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worst_pair: Option<(String, String)>,
    pub message: String,
}

/// トラックの組ごとのぶつかりの数
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PairCount {
    pub a: String,
    pub b: String,
    pub warn: usize,
    pub info: usize,
}

/// 調べた結果
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ClashReport {
    /// 調べたトラック・除いたトラック(ミュート・ドラム・ノートの無いもの)
    pub tracks: Vec<String>,
    /// 調べたトラックの ID(`tracks` と同じ順)
    pub track_ids: Vec<String>,
    pub skipped: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub clashes: Vec<Clash>,
    pub harsh: Vec<HarshSpot>,
    pub pairs: Vec<PairCount>,
    /// 小節ごとの粗さ(曲でいちばん粗い小節 = 1)。盛り上がりで増えるのはふつう。周りより飛び出た小節を見る
    pub roughness_by_bar: Vec<f64>,
    /// まとめで省いた数
    #[serde(skip_serializing_if = "is_zero")]
    pub omitted: usize,
    pub summary: String,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

impl ClashReport {
    pub fn warn_count(&self) -> usize {
        self.clashes.iter().filter(|c| c.severity == "warn").count()
    }
}

/// 鳴る音 1 つ
#[derive(Clone, Copy)]
struct Sound {
    start: u64,
    end: u64,
    pitch: u8,
    vel: u8,
    track: usize,
    clip: usize,
    note: usize,
}

fn is_drum(t: &Track) -> bool {
    t.device.as_ref().is_some_and(|d| d.source.is_drum_kit())
}

pub fn pitch_name(p: u8) -> String {
    const N: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!("{}{}", N[(p % 12) as usize], p as i32 / 12 - 1)
}

/// 2 つの純音のうなりの強さ(Plomp-Levelt の曲線、Sethares の近似。最大で 1 になるよう割る)
fn pl(f1: f64, f2: f64) -> f64 {
    let (lo, hi) = if f1 < f2 { (f1, f2) } else { (f2, f1) };
    let s = 0.24 / (0.0207 * lo + 18.96);
    let x = s * (hi - lo);
    ((-3.5 * x).exp() - (-5.75 * x).exp()) / 0.1807
}

/// 倍音の数と強さ(ふつうの楽器のおおまかな形)
const PARTIALS: usize = 6;
fn partial_amp(k: usize) -> f64 {
    0.88f64.powi(k as i32)
}

/// 2 音(倍音込み)のうなり。強さは掛けない。中音域の短 2 度でおよそ 1 になるよう割る
struct RoughTable {
    t: Vec<f64>,
}
impl RoughTable {
    fn new() -> Self {
        let raw = |a: u8, b: u8| -> f64 {
            let fa = 440.0 * 2f64.powf((a as f64 - 69.0) / 12.0);
            let fb = 440.0 * 2f64.powf((b as f64 - 69.0) / 12.0);
            let mut r = 0.0;
            for i in 0..PARTIALS {
                for j in 0..PARTIALS {
                    r += partial_amp(i).min(partial_amp(j))
                        * pl(fa * (i + 1) as f64, fb * (j + 1) as f64);
                }
            }
            r
        };
        let norm = raw(60, 61).max(1e-9);
        let mut t = vec![0.0; 128 * 128];
        for a in 0..128u8 {
            for b in a..128u8 {
                let v = if a == b { 0.0 } else { raw(a, b) / norm };
                t[a as usize * 128 + b as usize] = v;
                t[b as usize * 128 + a as usize] = v;
            }
        }
        Self { t }
    }
    fn get(&self, a: u8, b: u8) -> f64 {
        self.t[(a.min(127) as usize) * 128 + b.min(127) as usize]
    }
}

fn rough_table() -> &'static RoughTable {
    static T: std::sync::OnceLock<RoughTable> = std::sync::OnceLock::new();
    T.get_or_init(RoughTable::new)
}

/// 2 音のうなり(強さの小さい方で重み付け)
fn pair_roughness(a: &Sound, b: &Sound) -> f64 {
    rough_table().get(a.pitch, b.pitch) * (a.vel.min(b.vel) as f64 / 127.0)
}

/// 小節の格子から (小節番号 1 始まり, 拍 1 始まり)
struct Grid {
    bars: Vec<(u64, u64)>,
    beat_len: Vec<u64>,
}
impl Grid {
    fn new(project: &Project, end: u64) -> Self {
        let bars = crate::arrange::bar_grid(project, end.max(1));
        // 拍の長さ: 拍子の分母から(6/8 は 8 分音符を 1 拍)
        let mut sigs = project.time_sig_map.clone();
        sigs.sort_by_key(|s| s.tick);
        let beat_len = bars
            .iter()
            .map(|(t, _)| {
                let den = sigs
                    .iter()
                    .rev()
                    .find(|s| s.tick.0 <= *t)
                    .map_or(4, |s| s.den.max(1) as u64);
                (PPQ * 4 / den).max(1)
            })
            .collect();
        Self { bars, beat_len }
    }
    fn index(&self, tick: u64) -> usize {
        match self.bars.binary_search_by(|(t, _)| t.cmp(&tick)) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        }
    }
    fn locate(&self, tick: u64) -> (usize, f64) {
        if self.bars.is_empty() {
            return (1, 1.0 + tick as f64 / PPQ as f64);
        }
        let i = self.index(tick);
        let (bs, _) = self.bars[i];
        let beat = 1.0 + tick.saturating_sub(bs) as f64 / self.beat_len[i] as f64;
        (i + 1, (beat * 100.0).round() / 100.0)
    }
    /// 拍の頭に当たるか
    fn on_beat(&self, tick: u64) -> bool {
        if self.bars.is_empty() {
            return tick % PPQ == 0;
        }
        let i = self.index(tick);
        tick.saturating_sub(self.bars[i].0) % self.beat_len[i] == 0
    }
    /// tick を含む 2 拍の窓(和音の推定に使う)
    fn window(&self, tick: u64) -> (u64, u64) {
        if self.bars.is_empty() {
            let s = tick / (PPQ * 2) * PPQ * 2;
            return (s, s + PPQ * 2);
        }
        let i = self.index(tick);
        let (bs, bl) = self.bars[i];
        let w = (self.beat_len[i] * 2).min(bl).max(1);
        let s = bs + (tick.saturating_sub(bs)) / w * w;
        (s, (s + w).min(bs + bl).max(s + 1))
    }
}

/// 曲(または範囲)のぶつかりを調べる
pub fn check(project: &Project, opts: &ClashOptions) -> ClashReport {
    let mut report = ClashReport::default();
    // ---- 鳴る音を集める ----
    let mut sounds: Vec<Sound> = Vec::new();
    for (ti, t) in project.tracks.iter().enumerate() {
        if let Some(ids) = &opts.tracks {
            if !ids.contains(&t.id) {
                continue;
            }
        }
        let has_notes = t
            .clips
            .iter()
            .any(|c| matches!(c.content, ClipContent::Midi { .. }));
        if !has_notes {
            continue;
        }
        if is_drum(t) || (t.mute && !opts.include_muted) {
            report.skipped.push(t.name.clone());
            continue;
        }
        report.tracks.push(t.name.clone());
        report.track_ids.push(t.id.to_string());
        for (ci, c) in t.clips.iter().enumerate() {
            if !matches!(c.content, ClipContent::Midi { .. }) {
                continue;
            }
            let ids: HashMap<NoteId, usize> = c
                .notes()
                .map(|ns| {
                    ns.iter()
                        .enumerate()
                        .map(|(i, n)| (n.id.clone(), i))
                        .collect()
                })
                .unwrap_or_default();
            for n in c.playback_notes() {
                let mut start = c.start.0 + n.pos.0;
                let mut end = start + n.dur.0;
                if let Some((rs, re)) = opts.range {
                    if end <= rs || start >= re {
                        continue;
                    }
                    start = start.max(rs);
                    end = end.min(re);
                }
                if end <= start {
                    continue;
                }
                sounds.push(Sound {
                    start,
                    end,
                    pitch: n.pitch,
                    vel: n.vel.max(1),
                    track: ti,
                    clip: ci,
                    note: ids.get(&n.id).copied().unwrap_or(usize::MAX),
                });
            }
        }
    }
    sounds.sort_by_key(|s| (s.start, s.pitch));
    let end = sounds.iter().map(|s| s.end).max().unwrap_or(0);
    let grid = Grid::new(project, end);

    if sounds.is_empty() {
        report.summary = tr!(
            "調べる音がありません(ドラム・ミュートのトラックは除いています)",
            "No notes to check (drum and muted tracks are excluded)"
        );
        return report;
    }

    // キー(和音の外を選ぶときの参考)
    let mut hist = [0.0f64; 12];
    for s in &sounds {
        hist[(s.pitch % 12) as usize] += (s.end - s.start) as f64;
    }
    report.key = Some(key_from_histogram(&hist).name);

    // 和音を担うトラック(伴奏): 自分の中で音が重なる(和音を弾く)トラックと、低い音域のトラック(ベース)。
    // 和音は伴奏だけから推定する。旋律の音まで入れると、はみ出した音も和音の一部に読めてしまう
    // (伴奏 Am に旋律の F が乗ると、全部で Fmaj7 と読めて、ぶつかりの元が分からなくなる)
    let carrier = carriers(project, &sounds);
    // 和音の推定(2 拍の窓ごとに覚える)。伴奏が鳴っていない所は、鳴っている音全部から
    let mut chord_cache: HashMap<u64, Option<(String, Vec<u8>)>> = HashMap::new();
    let mut chord_at = |tick: u64| -> Option<(String, Vec<u8>)> {
        let (ws, we) = grid.window(tick);
        chord_cache
            .entry(ws)
            .or_insert_with(|| {
                let histogram = |only_carriers: bool| {
                    let mut w = [0.0f64; 12];
                    let mut bass: Option<u8> = None;
                    for s in sounds.iter().take_while(|s| s.start < we) {
                        let ov = s.end.min(we).saturating_sub(s.start.max(ws));
                        if ov == 0 || (only_carriers && !carrier[s.track]) {
                            continue;
                        }
                        w[(s.pitch % 12) as usize] += ov as f64;
                        bass = Some(bass.map_or(s.pitch, |b| b.min(s.pitch)));
                    }
                    (w, bass)
                };
                let (mut w, mut bass) = histogram(true);
                if w.iter().sum::<f64>() <= 0.0 {
                    (w, bass) = histogram(false);
                }
                if w.iter().sum::<f64>() <= 0.0 {
                    return None;
                }
                let (name, _) = chord_from_histogram(&w, bass.map(|b| b % 12), true);
                let pcs = chord_pitch_classes(&name)?;
                Some((name, pcs))
            })
            .clone()
    };

    let note_of = |s: &Sound| -> ClashNote {
        let t = &project.tracks[s.track];
        let c = &t.clips[s.clip];
        ClashNote {
            track_id: t.id.to_string(),
            track: t.name.clone(),
            clip_id: c.id.to_string(),
            note_id: c
                .notes()
                .and_then(|ns| ns.get(s.note))
                .map(|n| n.id.to_string())
                .unwrap_or_default(),
            pitch: s.pitch,
            name: pitch_name(s.pitch),
            start: s.start,
            end: s.end,
        }
    };

    // ---- 2 音のぶつかり(別のトラックどうし) ----
    struct Group {
        clash: Clash,
        score: f64,
    }
    let mut groups: Vec<Group> = Vec::new();
    let mut index: HashMap<(usize, usize, u8, u8, &'static str), usize> = HashMap::new();
    let mut active: Vec<usize> = Vec::new();
    for (i, s) in sounds.iter().enumerate() {
        active.retain(|&j| sounds[j].end > s.start);
        for &j in &active {
            let o = &sounds[j];
            if o.track == s.track {
                continue;
            }
            let ov_start = s.start.max(o.start);
            let ov = s.end.min(o.end).saturating_sub(ov_start);
            if ov < MIN_OVERLAP {
                continue;
            }
            let d = s.pitch.abs_diff(o.pitch);
            let kind = match d {
                1 => "semitone",
                13 => "minor_ninth",
                2 | 3 if s.pitch < LOW_LIMIT && o.pitch < LOW_LIMIT => "low_mud",
                _ => continue,
            };
            let chord = chord_at(ov_start);
            let out = |p: u8| {
                chord
                    .as_ref()
                    .is_some_and(|(_, pcs)| !pcs.contains(&(p % 12)))
            };
            let (s_out, o_out) = (out(s.pitch), out(o.pitch));
            // 動かす候補を a に: 和音の外の方。どちらも同じなら高い方(旋律の側)
            let (a, b, a_out, b_out) = if s_out != o_out {
                if s_out {
                    (s, o, s_out, o_out)
                } else {
                    (o, s, o_out, s_out)
                }
            } else if s.pitch >= o.pitch {
                (s, o, s_out, o_out)
            } else {
                (o, s, o_out, s_out)
            };
            let off_chord = match (a_out, b_out) {
                (true, true) => Some("both"),
                (true, false) => Some("a"),
                (false, true) => Some("b"),
                (false, false) => None,
            };
            let long = ov >= PPQ || (ov >= PPQ / 2 && grid.on_beat(ov_start));
            // 和音の音どうしの半音(maj7 の 7 度と根音など)は積み方の色。濁りが目立つのは低めの音域で長く重なるときだけ
            let voicing_only = off_chord.is_none() && kind != "low_mud";
            let severity = if voicing_only {
                if ov >= PPQ * 2 && a.pitch.min(b.pitch) < 60 {
                    "warn"
                } else {
                    "info"
                }
            } else if long {
                "warn"
            } else {
                "info"
            };
            let rough = pair_roughness(a, b);
            let key = (a.track, b.track, a.pitch, b.pitch, kind);
            let (bar, beat) = grid.locate(ov_start);
            let span = || ClashSpan {
                tick: ov_start,
                len: ov,
                a_clip: project.tracks[a.track].clips[a.clip].id.to_string(),
                b_clip: project.tracks[b.track].clips[b.clip].id.to_string(),
            };
            if let Some(&gi) = index.get(&key) {
                let g = &mut groups[gi];
                if opts.spans && g.clash.spans.len() < 256 {
                    g.clash.spans.push(span());
                }
                g.clash.count += 1;
                if g.clash.bars.last() != Some(&bar) && g.clash.bars.len() < 32 {
                    g.clash.bars.push(bar);
                }
                if severity == "warn" && g.clash.severity != "warn" {
                    g.clash.severity = "warn";
                }
                g.score += rough * ov as f64 / PPQ as f64;
                continue;
            }
            let (an, bn) = (note_of(a), note_of(b));
            let chord_name = chord.as_ref().map(|(n, _)| n.clone());
            let iv = match kind {
                "semitone" => crate::i18n::t("半音(短 2 度)", "a semitone (minor 2nd)"),
                "minor_ninth" => crate::i18n::t("短 9 度", "a minor 9th"),
                _ => crate::i18n::t("低い音域の近い音程", "a close interval in the low register"),
            };
            let message = tr!(
                "「{}」の {} と「{}」の {} が{}でぶつかる",
                "\"{}\" {} clashes with \"{}\" {} by {}",
                an.track,
                an.name,
                bn.track,
                bn.name,
                iv
            );
            let hint = hint_for(kind, off_chord, &an, &bn, chord.as_ref());
            index.insert(key, groups.len());
            groups.push(Group {
                clash: Clash {
                    severity,
                    kind,
                    bar,
                    beat,
                    tick: ov_start,
                    len: ov,
                    a: an,
                    b: bn,
                    chord: chord_name,
                    off_chord,
                    roughness: (rough * 100.0).round() / 100.0,
                    count: 1,
                    bars: vec![bar],
                    spans: if opts.spans { vec![span()] } else { Vec::new() },
                    message,
                    hint,
                },
                score: rough * ov as f64 / PPQ as f64,
            });
        }
        active.push(i);
    }
    // トラックの組ごとの数
    let mut pairs: Vec<PairCount> = Vec::new();
    for g in &groups {
        let (a, b) = if g.clash.a.track <= g.clash.b.track {
            (&g.clash.a.track, &g.clash.b.track)
        } else {
            (&g.clash.b.track, &g.clash.a.track)
        };
        let p = match pairs.iter_mut().find(|p| &p.a == a && &p.b == b) {
            Some(p) => p,
            None => {
                pairs.push(PairCount {
                    a: a.clone(),
                    b: b.clone(),
                    warn: 0,
                    info: 0,
                });
                pairs.last_mut().expect("直前に足した")
            }
        };
        if g.clash.severity == "warn" {
            p.warn += g.clash.count;
        } else {
            p.info += g.clash.count;
        }
    }
    pairs.sort_by_key(|p| std::cmp::Reverse(p.warn * 4 + p.info));
    report.pairs = pairs;
    // warn を先に、聞こえる量(うなり × 長さ × 回数)の大きい順
    groups.sort_by(|x, y| {
        (y.clash.severity == "warn")
            .cmp(&(x.clash.severity == "warn"))
            .then(y.score.total_cmp(&x.score))
    });
    if groups.len() > MAX_CLASHES {
        report.omitted += groups.len() - MAX_CLASHES;
        groups.truncate(MAX_CLASHES);
    }
    report.clashes = groups.into_iter().map(|g| g.clash).collect();

    // ---- 粗さ(16 分ごと)----
    let first = sounds.first().map_or(0, |s| s.start) / SLICE * SLICE;
    let n_slices = ((end - first).div_ceil(SLICE)) as usize;
    let mut rough = vec![0.0f64; n_slices];
    let mut worst: Vec<Option<(usize, usize, f64)>> = vec![None; n_slices];
    {
        let mut active: Vec<usize> = Vec::new();
        let mut next = 0usize;
        for (k, r) in rough.iter_mut().enumerate() {
            let (ss, se) = (first + k as u64 * SLICE, first + (k as u64 + 1) * SLICE);
            while next < sounds.len() && sounds[next].start < se {
                active.push(next);
                next += 1;
            }
            active.retain(|&j| sounds[j].end > ss);
            // 半分以上鳴っている音だけ
            let on: Vec<usize> = active
                .iter()
                .copied()
                .filter(|&j| {
                    sounds[j]
                        .end
                        .min(se)
                        .saturating_sub(sounds[j].start.max(ss))
                        * 2
                        >= SLICE
                })
                .collect();
            for (x, &i) in on.iter().enumerate() {
                for &j in &on[x + 1..] {
                    let v = pair_roughness(&sounds[i], &sounds[j]);
                    *r += v;
                    if worst[k].map_or(true, |w| v > w.2) {
                        worst[k] = Some((i, j, v));
                    }
                }
            }
            // 音の数で割る(音が多い・厚いだけの所を「濁る」と取り違えない。1 音あたりのうなり)
            if !on.is_empty() {
                *r /= on.len() as f64;
            }
        }
    }
    // 小節ごと(曲でいちばん粗い小節 = 1)
    let mut by_bar = vec![(0.0f64, 0usize); grid.bars.len().max(1)];
    for (k, r) in rough.iter().enumerate() {
        let b = grid.index(first + k as u64 * SLICE).min(by_bar.len() - 1);
        by_bar[b].0 += r;
        by_bar[b].1 += 1;
    }
    let lo_bar = opts.range.map_or(0, |(s, _)| grid.index(s));
    let bars: Vec<f64> = by_bar[lo_bar..].iter().map(|(s, _)| *s).collect();
    let max_bar = bars.iter().cloned().fold(0.0, f64::max);
    report.roughness_by_bar = bars
        .iter()
        .map(|v| {
            if max_bar > 0.0 {
                (v / max_bar * 100.0).round() / 100.0
            } else {
                0.0
            }
        })
        .collect();

    // 耳ざわりな所: 鳴っている所の中央値の 2 倍以上が 1 拍以上続く(1 音あたりのうなりが中音域の短 2 度の 1/4 以上)
    let mut nonzero: Vec<f64> = rough.iter().copied().filter(|v| *v > 0.0).collect();
    nonzero.sort_by(|a, b| a.total_cmp(b));
    let median = nonzero.get(nonzero.len() / 2).copied().unwrap_or(0.0);
    let threshold = (median * 2.0).max(0.25);
    let mut harsh: Vec<(HarshSpot, f64)> = Vec::new();
    let mut k = 0usize;
    while k < n_slices {
        if rough[k] < threshold {
            k += 1;
            continue;
        }
        let s0 = k;
        let mut peak = s0;
        while k < n_slices && rough[k] >= threshold {
            if rough[k] > rough[peak] {
                peak = k;
            }
            k += 1;
        }
        let (t0, t1) = (first + s0 as u64 * SLICE, first + k as u64 * SLICE);
        if t1 - t0 < PPQ {
            continue;
        }
        let tp = first + peak as u64 * SLICE;
        let notes: Vec<String> = sounds
            .iter()
            .filter(|s| s.start <= tp && s.end > tp)
            .map(|s| format!("{} {}", project.tracks[s.track].name, pitch_name(s.pitch)))
            .collect();
        let worst_pair = worst[peak].map(|(i, j, _)| {
            let f = |s: &Sound| format!("{} {}", project.tracks[s.track].name, pitch_name(s.pitch));
            (f(&sounds[i]), f(&sounds[j]))
        });
        let (bar, beat) = grid.locate(t0);
        let relative = if median > 0.0 {
            rough[peak] / median
        } else {
            0.0
        };
        let chord = chord_at(tp).map(|(n, _)| n);
        let message = match &worst_pair {
            Some((x, y)) => tr!(
                "曲のふつうの所の {:.1} 倍濁る(いちばんの元は {} と {})",
                "{:.1}x rougher than the rest of the song (mostly {} against {})",
                relative,
                x,
                y
            ),
            None => tr!(
                "曲のふつうの所の {:.1} 倍濁る",
                "{:.1}x rougher than the rest of the song",
                relative
            ),
        };
        harsh.push((
            HarshSpot {
                bar,
                beat,
                tick: t0,
                len: t1 - t0,
                relative: (relative * 10.0).round() / 10.0,
                chord,
                notes,
                worst_pair,
                message,
            },
            rough[peak] * (t1 - t0) as f64,
        ));
    }
    harsh.sort_by(|a, b| b.1.total_cmp(&a.1));
    if harsh.len() > MAX_HARSH {
        report.omitted += harsh.len() - MAX_HARSH;
        harsh.truncate(MAX_HARSH);
    }
    let mut harsh: Vec<HarshSpot> = harsh.into_iter().map(|(h, _)| h).collect();
    harsh.sort_by_key(|h| h.tick);
    report.harsh = harsh;

    let warn = report.warn_count();
    let info = report.clashes.len() - warn;
    report.summary = if report.clashes.is_empty() && report.harsh.is_empty() {
        tr!(
            "{} トラックを調べた。ぶつかり・耳ざわりな所は見つからない",
            "Checked {} tracks. No clashes or harsh spots found",
            report.tracks.len()
        )
    } else {
        tr!(
            "{} トラックを調べた。直したいぶつかり {} 組・短い経過のぶつかり {} 組・耳ざわりな所 {} か所",
            "Checked {} tracks. {} clashes to fix, {} brief passing clashes, {} harsh spots",
            report.tracks.len(),
            warn,
            info,
            report.harsh.len()
        )
    };
    report
}

/// トラックごとに、和音を担うか(伴奏)。音が重なって鳴る時間が 3 割以上(和音を弾く)か、
/// 音の高さの中央値が E3 より下(ベース)
fn carriers(project: &Project, sounds: &[Sound]) -> Vec<bool> {
    let n = project.tracks.len();
    let mut total = vec![0u64; n];
    let mut stacked = vec![0u64; n];
    let mut pitches: Vec<Vec<u8>> = vec![Vec::new(); n];
    let mut last_end = vec![0u64; n];
    for s in sounds {
        // 同じトラックの前の音と重なる分(始まりの順に並んでいる)
        let ov = last_end[s.track].min(s.end).saturating_sub(s.start);
        stacked[s.track] += ov;
        total[s.track] += s.end - s.start;
        last_end[s.track] = last_end[s.track].max(s.end);
        pitches[s.track].push(s.pitch);
    }
    (0..n)
        .map(|t| {
            if total[t] == 0 {
                return false;
            }
            let mut p = std::mem::take(&mut pitches[t]);
            p.sort_unstable();
            let median = p[p.len() / 2];
            stacked[t] * 10 >= total[t] * 3 || median < 52
        })
        .collect()
}

/// 直し方の案
fn hint_for(
    kind: &str,
    off: Option<&str>,
    a: &ClashNote,
    b: &ClashNote,
    chord: Option<&(String, Vec<u8>)>,
) -> String {
    if kind == "low_mud" {
        return tr!(
            "低い音はベースに任せ、「{}」をオクターブ上げる(低い音域で近い音程は濁る)",
            "Leave the low end to the bass and move \"{}\" up an octave (close intervals in the low register get muddy)",
            a.track
        );
    }
    let nearest = |p: u8, pcs: &[u8]| -> String {
        let mut c: Vec<(u8, i32)> = (1..=4i32)
            .flat_map(|d| [d, -d])
            .filter_map(|d| {
                let q = p as i32 + d;
                (0..128)
                    .contains(&q)
                    .then_some(q as u8)
                    .filter(|q| pcs.contains(&(q % 12)))
                    .map(|q| (q, d.abs()))
            })
            .collect();
        c.sort_by_key(|x| x.1);
        c.dedup_by_key(|x| x.0);
        c.iter()
            .take(2)
            .map(|x| pitch_name(x.0))
            .collect::<Vec<_>>()
            .join(crate::i18n::t(" か ", " or "))
    };
    match (off, chord) {
        (Some("a"), Some((name, pcs))) => tr!(
            "この所の和音は {}。「{}」の {} が和音の外なので、{} へ寄せる(わざとの経過音なら短くする)",
            "The chord here is {}. \"{}\" {} is outside it; move it to {} (if it is a deliberate passing tone, shorten it)",
            name,
            a.track,
            a.name,
            nearest(a.pitch, pcs)
        ),
        (Some("b"), Some((name, pcs))) => tr!(
            "この所の和音は {}。「{}」の {} が和音の外なので、{} へ寄せる",
            "The chord here is {}. \"{}\" {} is outside it; move it to {}",
            name,
            b.track,
            b.name,
            nearest(b.pitch, pcs)
        ),
        (Some("both"), Some((name, _))) => tr!(
            "この所の和音は {} で、どちらも和音の外。旋律か伴奏のどちらが正しい和音か決めて、もう片方を合わせる",
            "The chord here is {} and both notes are outside it. Decide which part carries the intended harmony and match the other to it",
            name
        ),
        _ => tr!(
            "どちらも和音の音だが、隣り合って濁る。「{}」をオクターブ動かすか、伴奏の積み方からその音を抜く",
            "Both are chord tones but sit a semitone apart. Move \"{}\" by an octave or drop that tone from the accompaniment voicing",
            b.track
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::ClipId;
    use crate::model::{Articulation, Clip, Note, TrackKind};
    use crate::time::Tick;

    fn track(name: &str, notes: &[(u64, u64, u8)]) -> Track {
        let mut t = Track::new(TrackId::new(), name, TrackKind::Midi);
        let mut clip = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(PPQ * 16));
        let ns = clip.notes_mut().expect("MIDI");
        for &(pos, dur, pitch) in notes {
            ns.push(Note {
                locked: false,
                id: NoteId::new(),
                pos: Tick(pos),
                dur: Tick(dur),
                pitch,
                vel: 100,
                articulation: Articulation::Normal,
                pitch_curve: vec![],
                glide_ms: None,
                vibrato: None,
                volume_curve: vec![],
                brightness_curve: vec![],
                condition: None,
            });
        }
        t.clips.push(clip);
        t
    }

    fn song(tracks: Vec<Track>) -> Project {
        let mut p = Project::new("t");
        p.tracks = tracks;
        p
    }

    #[test]
    fn a_melody_note_off_the_chord_is_named_as_the_one_to_move() {
        // 伴奏: C メジャー(C4 E4 G4)を 1 小節。旋律: F4 を 1 拍目から 2 拍(E4 と半音)
        let keys = track(
            "Keys",
            &[(0, PPQ * 4, 60), (0, PPQ * 4, 64), (0, PPQ * 4, 67)],
        );
        let lead = track("Lead", &[(0, PPQ * 2, 65)]);
        let r = check(&song(vec![keys, lead]), &ClashOptions::default());
        let c = r.clashes.first().expect("ぶつかりがある");
        assert_eq!(c.severity, "warn");
        assert_eq!(c.kind, "semitone");
        assert_eq!(c.a.track, "Lead");
        assert_eq!(c.a.name, "F4");
        assert_eq!(c.off_chord, Some("a"));
        assert_eq!(c.chord.as_deref(), Some("C"));
        assert_eq!((c.bar, c.beat), (1, 1.0));
        assert!(c.hint.contains("E4") || c.hint.contains("G4"), "{}", c.hint);
    }

    #[test]
    fn a_short_passing_note_is_info_and_consonant_parts_are_clean() {
        let keys = track(
            "Keys",
            &[(0, PPQ * 4, 60), (0, PPQ * 4, 64), (0, PPQ * 4, 67)],
        );
        // 裏の 16 分の経過音 F4 → info
        let lead = track("Lead", &[(PPQ / 4 * 3, PPQ / 4, 65), (PPQ, PPQ * 3, 67)]);
        let r = check(&song(vec![keys.clone(), lead]), &ClashOptions::default());
        assert!(
            r.clashes.iter().all(|c| c.severity == "info"),
            "{:?}",
            r.clashes
        );
        assert_eq!(r.warn_count(), 0);
        // 和音の音だけの旋律ならぶつかり無し
        let lead = track("Lead", &[(0, PPQ * 4, 72)]);
        let r = check(&song(vec![keys, lead]), &ClashOptions::default());
        assert!(r.clashes.is_empty(), "{:?}", r.clashes);
        assert!(r.harsh.is_empty(), "{:?}", r.harsh);
    }

    #[test]
    fn repeats_are_grouped_and_muted_tracks_are_skipped() {
        // 伴奏 Am(A3 C4 E4)に旋律の F4 が 4 小節とも乗る。全部の音で読むと Fmaj7 になってしまうが、
        // 伴奏の和音(Am)と照らして F4 を和音の外と見る。同じぶつかりは 1 組にまとまる
        let bars: Vec<(u64, u64, u8)> = (0..4)
            .flat_map(|b| [57u8, 60, 64].map(|p| (b * PPQ * 4, PPQ * 4, p)))
            .collect();
        let keys = track("Keys", &bars);
        let lead: Vec<(u64, u64, u8)> = (0..4).map(|b| (b * PPQ * 4, PPQ * 2, 65)).collect();
        let mut lead = track("Lead", &lead);
        let r = check(
            &song(vec![keys.clone(), lead.clone()]),
            &ClashOptions::default(),
        );
        assert_eq!(r.clashes.len(), 1, "{:?}", r.clashes);
        assert_eq!(r.clashes[0].severity, "warn");
        assert_eq!(r.clashes[0].chord.as_deref(), Some("Am"));
        assert_eq!(
            (r.clashes[0].a.track.as_str(), r.clashes[0].off_chord),
            ("Lead", Some("a"))
        );
        assert_eq!(r.clashes[0].count, 4);
        assert_eq!(r.clashes[0].bars, vec![1, 2, 3, 4]);
        assert_eq!(r.pairs[0].warn, 4);
        lead.mute = true;
        let r = check(
            &song(vec![keys.clone(), lead.clone()]),
            &ClashOptions::default(),
        );
        assert!(r.clashes.is_empty());
        assert_eq!(r.skipped, vec!["Lead".to_owned()]);
        let r = check(
            &song(vec![keys, lead]),
            &ClashOptions {
                include_muted: true,
                ..Default::default()
            },
        );
        assert_eq!(r.clashes.len(), 1);
    }

    #[test]
    fn low_register_mud_and_harsh_spots() {
        // ベース E1 と シンセ G1(短 3 度、どちらも C3 より下)
        let bass = track("Bass", &[(0, PPQ * 4, 28)]);
        let synth = track("Synth", &[(0, PPQ * 4, 31)]);
        let r = check(&song(vec![bass, synth]), &ClashOptions::default());
        assert!(
            r.clashes.iter().any(|c| c.kind == "low_mud"),
            "{:?}",
            r.clashes
        );

        // 1〜3 小節はきれいな C、4 小節目だけ中音域で半音が 3 つ重なる → 4 小節目が耳ざわり
        let mut keys = Vec::new();
        for b in 0..3u64 {
            keys.extend([
                (b * PPQ * 4, PPQ * 4, 60u8),
                (b * PPQ * 4, PPQ * 4, 64),
                (b * PPQ * 4, PPQ * 4, 67),
            ]);
        }
        keys.extend([
            (PPQ * 12, PPQ * 4, 60),
            (PPQ * 12, PPQ * 4, 64),
            (PPQ * 12, PPQ * 4, 67),
        ]);
        let pad = track(
            "Pad",
            &[
                (PPQ * 12, PPQ * 4, 61),
                (PPQ * 12, PPQ * 4, 66),
                (PPQ * 12, PPQ * 4, 68),
            ],
        );
        let r = check(
            &song(vec![track("Keys", &keys), pad]),
            &ClashOptions::default(),
        );
        assert!(r.harsh.iter().any(|h| h.bar == 4), "{:?}", r.harsh);
        assert_eq!(r.roughness_by_bar.len(), 4);
        assert_eq!(r.roughness_by_bar[3], 1.0);
        assert!(r.roughness_by_bar[0] < 0.5, "{:?}", r.roughness_by_bar);
    }

    #[test]
    fn roughness_follows_the_ear() {
        let t = rough_table();
        // 中音域: 短 2 度 > 長 2 度 > 長 3 度 > 完全 5 度 > オクターブ
        let (m2, maj2, maj3, p5, oct) = (
            t.get(60, 61),
            t.get(60, 62),
            t.get(60, 64),
            t.get(60, 67),
            t.get(60, 72),
        );
        assert!(
            m2 > maj2 && maj2 > maj3 && maj3 > p5 && p5 > oct,
            "{m2} {maj2} {maj3} {p5} {oct}"
        );
        // 同じ長 3 度でも低いほど濁る
        assert!(t.get(36, 40) > t.get(60, 64) * 1.5);
    }

    #[test]
    fn range_and_empty() {
        let keys = track("Keys", &[(0, PPQ * 4, 64), (PPQ * 8, PPQ * 4, 64)]);
        let lead = track("Lead", &[(0, PPQ * 2, 65), (PPQ * 8, PPQ * 2, 65)]);
        let p = song(vec![keys, lead]);
        let r = check(
            &p,
            &ClashOptions {
                range: Some((PPQ * 8, PPQ * 12)),
                ..Default::default()
            },
        );
        assert_eq!(r.clashes.len(), 1);
        assert_eq!(r.clashes[0].bars, vec![3]);
        let r = check(&song(vec![]), &ClashOptions::default());
        assert!(r.clashes.is_empty() && !r.summary.is_empty());
    }
}
