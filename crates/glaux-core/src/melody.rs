//! 旋律の点検(MCP の critique_melody の中身)。旋律の「センス」のうち数えられる性質を測り、直し方と一緒に返す。
//!
//! 研究で確かめられた性質(docs の調査の 3 章):
//! - 順次進行が多い。大きい跳躍(7 半音以上)の約 72% は向きを変えて戻る(von Hippel & Huron 2000)
//! - 最高音は山の区間で。句の終わりは長く伸ばす(Tierney ら 2011)
//! - 強拍は和音の音。ロックの歌では 8 分の裏へ食う音が約 23%(Tan・Lustig・Temperley 2019)
//! - 覚えやすさは反復の多さと「ありふれた輪郭 + 局所の驚き」(Jakubowski 2017、Van Balen 2015)
//! - 音ごとの驚き(情報量)は Temperley 2008 の式で、学習データ無しに計算できる
//!   (音域の分散 29.0・前の音との近さの分散 7.2・キーの分布)
//!
//! ジャンルごとのしきい値は経験則の初期値(ベンチマークで調整する)。

use crate::chord::Key;
use crate::critique::Finding;
use crate::model::Project;
use serde::Serialize;

/// 旋律の 1 音(絶対 tick)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MelNote {
    pub pos: u64,
    pub dur: u64,
    pub pitch: u8,
}

/// ジャンルごとのしきい値
#[derive(Debug)]
pub struct Genre {
    pub name: &'static str,
    /// 音域(半音。区間の中)の目安 (下限, 上限)
    pub range: (u8, u8),
    /// 順次進行(2 半音以下)の割合の目安(参考の数値として返す)
    pub step: (f64, f64),
    /// 跳躍(5 半音以上)の割合の上限
    pub leap_max: f64,
    /// 8 分の裏へ食う音の割合の目安
    pub syncopation: (f64, f64),
    /// 動機の反復率の上限(これを超えると繰り返しすぎ)
    pub repeat_max: f64,
    /// 強拍の和音の音の割合の下限
    pub strong_chord_tone: f64,
    /// 息継ぎ(4 小節を超えて休みが無い句)を点検するか(歌・管の旋律。シンセのリードは続けてよい)
    pub breath: bool,
    pub note: &'static str,
}

pub const GENRES: &[Genre] = &[
    Genre {
        name: "pop",
        range: (12, 17),
        step: (0.55, 0.85),
        leap_max: 0.3,
        syncopation: (0.1, 0.35),
        repeat_max: 0.85,
        strong_chord_tone: 0.5,
        breath: true,
        note:
            "ポップス・J-POP: 8 分主体 + 伸ばし。サビで最高音、同じ高い音の連打はフックとしてよい",
    },
    Genre {
        name: "edm",
        range: (6, 14),
        step: (0.4, 0.8),
        leap_max: 0.4,
        syncopation: (0.2, 0.5),
        repeat_max: 0.95,
        strong_chord_tone: 0.5,
        breath: false,
        note:
            "EDM のリード: 1〜2 小節の動機を繰り返し、最後だけ変える。16 分の裏に食う。音域は狭く",
    },
    Genre {
        name: "trap",
        range: (5, 12),
        step: (0.5, 0.9),
        leap_max: 0.3,
        syncopation: (0.1, 0.4),
        repeat_max: 0.95,
        strong_chord_tone: 0.4,
        breath: false,
        note: "トラップ: 短音階・和声的短音階・フリギアの短いループ。休符多め",
    },
    Genre {
        name: "lofi",
        range: (7, 12),
        step: (0.5, 0.85),
        leap_max: 0.3,
        syncopation: (0.1, 0.3),
        repeat_max: 0.8,
        strong_chord_tone: 0.5,
        breath: true,
        note: "ローファイ: 少ない音、ペンタトニック + 7 度・9 度、後ろにずらす",
    },
    Genre {
        name: "jazz",
        range: (15, 24),
        step: (0.6, 0.9),
        leap_max: 0.25,
        syncopation: (0.1, 0.3),
        repeat_max: 0.5,
        strong_chord_tone: 0.7,
        breath: true,
        note: "ジャズ: 8 分の連続、強拍に和声音(3 度・7 度)、エンクロージャ・半音の接近",
    },
    Genre {
        name: "funk",
        range: (5, 12),
        step: (0.3, 0.7),
        leap_max: 0.45,
        syncopation: (0.3, 0.6),
        repeat_max: 0.9,
        strong_chord_tone: 0.5,
        breath: true,
        note: "ファンクのホーン・リフ: 16 分の短いキメ、休符が多い、1 拍目の強調",
    },
];

pub fn genre(name: &str) -> Option<&'static Genre> {
    let n = name.trim().to_lowercase();
    let n = match n.as_str() {
        "jpop" | "j-pop" | "rock" | "ballad" | "anime" => "pop",
        "house" | "trance" | "techno" | "futurebass" => "edm",
        "hiphop" | "drill" => "trap",
        "chill" | "neosoul" => "lofi",
        "bebop" | "citypop" => "jazz",
        "disco" | "soul" => "funk",
        other => other,
    };
    GENRES.iter().find(|g| g.name == n)
}

/// 区間(点検で最高音の位置を見る)
#[derive(Clone, Debug)]
pub struct SectionSpan {
    pub name: String,
    pub start: u64,
    pub end: u64,
    /// 計画の盛り上がり(無ければ None)
    pub energy: Option<f32>,
}

/// 句 1 つ
#[derive(Clone, Debug, Serialize)]
pub struct Phrase {
    pub start_bar: usize,
    pub bars: f64,
    pub notes: usize,
    /// 句の最後の音の長さ ÷ 句の音の長さの中央値
    pub ending_ratio: f64,
    /// 句の驚き(情報量、ビット)の平均と最大
    pub surprise_mean: f64,
    pub surprise_max: f64,
}

/// 数値
#[derive(Clone, Debug, Default, Serialize)]
pub struct Metrics {
    pub notes: usize,
    /// 2 半音以下の音程の割合(同じ音の連続は除く)
    pub step_ratio: f64,
    /// 5 半音以上の音程の割合(同じ音の連続は除く)
    pub leap_ratio: f64,
    /// 同じ音の連続の割合
    pub repeat_ratio: f64,
    /// 7 半音以上の跳躍の数と、その後に向きを変えて小さい音程が来た割合
    pub leaps: usize,
    pub leap_recovery: Option<f64>,
    pub range: u8,
    pub lowest: u8,
    pub highest: u8,
    pub median_pitch: u8,
    /// 最高音の回数と、初めて出る小節
    pub peak_count: usize,
    pub peak_bar: usize,
    /// 強拍(小節頭・半ば)で始まる音のうち、和音の音の割合(和音が分からなければ None)
    pub strong_chord_tone: Option<f64>,
    /// 最も多い音価の割合
    pub top_duration_share: f64,
    pub duration_kinds: usize,
    /// 打点の型が他の小節と一致する小節の割合
    pub rhythm_reuse: f64,
    /// 4 音の形(音程と長さの比)が 2 回以上出る部分に入る音の割合
    pub motif_coverage: f64,
    /// 8 分の裏で始まり次の拍まで続く音の割合と、そのうち 1・3 拍目の直前の割合
    pub syncopation: f64,
    pub anticipation: f64,
    /// 音ごとの驚き(Temperley の式、ビット)の平均
    pub surprise_mean: f64,
}

/// 点検の結果
#[derive(Clone, Debug, Serialize)]
pub struct MelodyCritique {
    pub genre: &'static str,
    pub key: String,
    pub metrics: Metrics,
    pub phrases: Vec<Phrase>,
    pub findings: Vec<Finding>,
    /// 0〜100(指摘が少ないほど高い。複数の案を比べるときに使う)
    pub score: u32,
}

/// Temperley(2007)のエッセン民謡集からのキーの分布(主音から、長調・短調)
const KEY_MAJOR: [f64; 12] = [
    0.184, 0.001, 0.155, 0.003, 0.191, 0.109, 0.005, 0.214, 0.001, 0.078, 0.004, 0.055,
];
const KEY_MINOR: [f64; 12] = [
    0.192, 0.005, 0.149, 0.179, 0.002, 0.144, 0.002, 0.201, 0.038, 0.012, 0.053, 0.022,
];
const VAR_RANGE: f64 = 29.0;
const VAR_PROX: f64 = 7.2;

fn profile(key: Key) -> &'static [f64; 12] {
    if key.minor {
        &KEY_MINOR
    } else {
        &KEY_MAJOR
    }
}

/// 旋律の音から、キーの分布に最もよく合うキー
pub fn guess_key(notes: &[MelNote]) -> Key {
    let mut best = (
        f64::MIN,
        Key {
            tonic: 0,
            minor: false,
        },
    );
    for minor in [false, true] {
        for tonic in 0..12u8 {
            let k = Key { tonic, minor };
            let p = profile(k);
            let s: f64 = notes
                .iter()
                .map(|n| {
                    p[((n.pitch as i32 - tonic as i32).rem_euclid(12)) as usize].ln() * n.dur as f64
                })
                .sum();
            if s > best.0 {
                best = (s, k);
            }
        }
    }
    best.1
}

/// 音ごとの驚き(情報量、ビット)。Temperley の RPK モデル: P(音) ∝ 音域 × 近さ × キー
pub fn surprise(notes: &[MelNote], key: Key) -> Vec<f64> {
    if notes.is_empty() {
        return vec![];
    }
    let center = notes.iter().map(|n| n.pitch as f64).sum::<f64>() / notes.len() as f64;
    let prof = profile(key);
    let mut prev: Option<f64> = None;
    notes
        .iter()
        .map(|n| {
            let weight = |p: f64| {
                let r = (-(p - center).powi(2) / (2.0 * VAR_RANGE)).exp();
                let x = prev.map_or(1.0, |q| (-(p - q).powi(2) / (2.0 * VAR_PROX)).exp());
                let k = prof[((p as i32 - key.tonic as i32).rem_euclid(12)) as usize];
                r * x * k
            };
            let z: f64 = (0..128).map(|p| weight(p as f64)).sum();
            let w = weight(n.pitch as f64);
            prev = Some(n.pitch as f64);
            -(w / z.max(1e-300)).max(1e-12).log2()
        })
        .collect()
}

/// 旋律の線だけを残す(同じ時に始まる音は一番高い音。ただし全部がオクターブ違いの同じ音なら、前の音に近い方)
pub fn top_line(mut notes: Vec<MelNote>) -> Vec<MelNote> {
    notes.sort_by_key(|n| (n.pos, std::cmp::Reverse(n.pitch)));
    let mut out: Vec<MelNote> = Vec::new();
    let mut i = 0;
    while i < notes.len() {
        let mut j = i + 1;
        while j < notes.len() && notes[j].pos.abs_diff(notes[i].pos) < 30 {
            j += 1;
        }
        let group = &notes[i..j];
        let octaves = group.iter().all(|n| n.pitch % 12 == group[0].pitch % 12);
        let pick = match out.last() {
            Some(prev) if octaves && group.len() > 1 => *group
                .iter()
                .min_by_key(|n| (n.pitch as i32 - prev.pitch as i32).abs())
                .expect("空ではない"),
            _ => group[0],
        };
        out.push(pick);
        i = j;
    }
    out
}

fn median_u64(v: &mut [u64]) -> u64 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    v[v.len() / 2]
}

/// 点検の文脈
pub struct Context<'a> {
    pub project: &'a Project,
    /// その時に鳴っている和音の構成音(ピッチクラス)。分からなければ None
    pub chord_at: &'a dyn Fn(u64) -> Option<Vec<u8>>,
    pub key: Option<Key>,
    pub genre: &'static Genre,
    pub sections: Vec<SectionSpan>,
    /// 指摘の対象の名前(トラック名)
    pub target: String,
}

pub fn critique(notes: &[MelNote], ctx: &Context) -> MelodyCritique {
    let notes = top_line(notes.to_vec());
    let key = ctx.key.unwrap_or_else(|| guess_key(&notes));
    let key_name = format!(
        "{} {}",
        ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"][key.tonic as usize],
        if key.minor { "minor" } else { "major" }
    );
    let g = ctx.genre;
    let mut m = Metrics {
        notes: notes.len(),
        ..Default::default()
    };
    let mut findings: Vec<Finding> = Vec::new();
    let mut warn = |sev: &'static str, what: String, fix: &str| {
        findings.push(Finding {
            severity: sev,
            target: ctx.target.clone(),
            what,
            fix: fix.to_owned(),
        })
    };
    if notes.len() < 4 {
        return MelodyCritique {
            genre: g.name,
            key: key_name,
            metrics: m,
            phrases: vec![],
            findings: vec![Finding {
                severity: "info",
                target: ctx.target.clone(),
                what: "音が少なすぎて点検できない(4 音以上)".to_owned(),
                fix: String::new(),
            }],
            score: 0,
        };
    }
    let end = notes.iter().map(|n| n.pos + n.dur).max().unwrap_or(0);
    let grid = crate::arrange::bar_grid(ctx.project, end.max(1));
    let bar_of = |t: u64| grid.partition_point(|(s, _)| *s <= t).saturating_sub(1);
    let sig_at = |t: u64| {
        let mut sigs = ctx.project.time_sig_map.clone();
        sigs.sort_by_key(|s| s.tick);
        sigs.iter()
            .rev()
            .find(|s| s.tick.0 <= t)
            .map_or((4u8, 4u8), |s| (s.num, s.den))
    };
    // 拍の位置: (小節の中の拍の番号, 拍の頭か, 拍の長さ)
    let beat_info = |t: u64| {
        let b = bar_of(t);
        let (bar, _) = grid.get(b).copied().unwrap_or((0, 3840));
        let (num, den) = sig_at(t);
        let beat = crate::time::PPQ * 4 / den.max(1) as u64;
        let rel = t - bar;
        let idx = (rel + 60) / beat;
        let on = (rel + 60) % beat < 120;
        (b, idx, on, beat, num)
    };
    // 音程
    let ivs: Vec<i32> = notes
        .windows(2)
        .map(|w| w[1].pitch as i32 - w[0].pitch as i32)
        .collect();
    let moving: Vec<i32> = ivs.iter().copied().filter(|i| *i != 0).collect();
    m.step_ratio = if moving.is_empty() {
        0.0
    } else {
        moving.iter().filter(|i| i.abs() <= 2).count() as f64 / moving.len() as f64
    };
    m.leap_ratio = if moving.is_empty() {
        0.0
    } else {
        moving.iter().filter(|i| i.abs() >= 5).count() as f64 / moving.len() as f64
    };
    m.repeat_ratio = ivs.iter().filter(|i| **i == 0).count() as f64 / ivs.len().max(1) as f64;
    // 跳躍の後の戻り(Schellenberg: 7 半音以上の跳躍の後、向きを変えて小さい音程)
    let mut unrecovered = Vec::new();
    for (i, w) in ivs.windows(2).enumerate() {
        if w[0].abs() >= 7 {
            m.leaps += 1;
            let ok = w[1] != 0 && w[1].signum() != w[0].signum() && w[1].abs() < w[0].abs();
            if !ok {
                unrecovered.push(bar_of(notes[i + 1].pos) + 1);
            }
        }
    }
    if m.leaps > 0 {
        m.leap_recovery = Some(1.0 - unrecovered.len() as f64 / m.leaps as f64);
    }
    // 音域・最高音
    m.lowest = notes.iter().map(|n| n.pitch).min().unwrap_or(0);
    m.highest = notes.iter().map(|n| n.pitch).max().unwrap_or(0);
    // 音域は区間ごとの最大(区間が無ければ全体)。曲全体ではオクターブ上げの区間なども入って広くなる
    m.range = if ctx.sections.is_empty() {
        m.highest - m.lowest
    } else {
        ctx.sections
            .iter()
            .filter_map(|s| {
                let ps: Vec<u8> = notes
                    .iter()
                    .filter(|n| s.start <= n.pos && n.pos < s.end)
                    .map(|n| n.pitch)
                    .collect();
                Some(ps.iter().max()? - ps.iter().min()?)
            })
            .max()
            .unwrap_or(m.highest - m.lowest)
    };
    let mut ps: Vec<u64> = notes.iter().map(|n| n.pitch as u64).collect();
    m.median_pitch = median_u64(&mut ps) as u8;
    m.peak_count = notes.iter().filter(|n| n.pitch == m.highest).count();
    let first_peak = notes
        .iter()
        .find(|n| n.pitch == m.highest)
        .map_or(0, |n| n.pos);
    m.peak_bar = bar_of(first_peak) + 1;
    // 強拍の和音の音(変拍子はまとまりの頭も強拍)
    let meters = crate::meter::bar_meters(ctx.project, end.max(1));
    let group_head = |t: u64| {
        meters
            .get(bar_of(t))
            .filter(|m| m.grouping.iter().any(|&g| g != 1))
            .is_some_and(|m| {
                m.strong_ticks()
                    .iter()
                    .any(|&s| (t - m.start.min(t)).abs_diff(s) < 60)
            })
    };
    let mut strong = 0usize;
    let mut strong_ct = 0usize;
    let mut unresolved = Vec::new();
    for (i, n) in notes.iter().enumerate() {
        let (b, idx, on, _, num) = beat_info(n.pos);
        let is_strong = (on && (idx == 0 || (num % 2 == 0 && num >= 4 && idx == num as u64 / 2)))
            || group_head(n.pos);
        if !is_strong {
            continue;
        }
        let Some(pcs) = (ctx.chord_at)(n.pos) else {
            continue;
        };
        strong += 1;
        if pcs.contains(&(n.pitch % 12)) {
            strong_ct += 1;
        } else {
            // 次の音へ順次で動けば解決(倚音・掛留)
            let resolves = notes
                .get(i + 1)
                .is_some_and(|nx| (1..=2).contains(&(nx.pitch as i32 - n.pitch as i32).abs()));
            if !resolves {
                unresolved.push(b + 1);
            }
        }
    }
    if strong > 0 {
        m.strong_chord_tone = Some(strong_ct as f64 / strong as f64);
    }
    // リズム
    let mut dur_hist: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    for n in &notes {
        *dur_hist.entry(((n.dur + 60) / 120).max(1)).or_default() += 1;
    }
    m.duration_kinds = dur_hist.len();
    m.top_duration_share = *dur_hist.values().max().unwrap_or(&0) as f64 / notes.len() as f64;
    let mut masks: std::collections::BTreeMap<usize, u64> = std::collections::BTreeMap::new();
    for n in &notes {
        let b = bar_of(n.pos);
        let bar = grid.get(b).map_or(0, |g| g.0);
        let step = ((n.pos - bar + 120) / 240).min(63);
        *masks.entry(b).or_default() |= 1u64 << step;
    }
    let bars_with: Vec<u64> = masks
        .values()
        .copied()
        .filter(|m| m.count_ones() >= 2)
        .collect();
    if !bars_with.is_empty() {
        let reused = bars_with
            .iter()
            .enumerate()
            .filter(|(i, m)| {
                bars_with
                    .iter()
                    .enumerate()
                    .any(|(j, o)| j != *i && o == *m)
            })
            .count();
        m.rhythm_reuse = reused as f64 / bars_with.len() as f64;
    }
    // 動機の反復率: 4 音の形(3 つの音程と長さの比)が 2 回以上出る部分。
    // 音程は向きと大きさの段階(同音・順次・3 度・4〜5 度・それ以上)で比べ、音階でずらした繰り返しや
    // 和音に合わせて音を変えた繰り返し(長 2 度 ↔ 短 2 度など)も同じ形とみなす
    let class = |d: i32| {
        d.signum()
            * match d.abs() {
                0 => 0,
                1 | 2 => 1,
                3 | 4 => 2,
                5..=7 => 3,
                _ => 4,
            }
    };
    let tokens: Vec<(i32, i32)> = notes
        .windows(2)
        .map(|w| {
            let iv = class(w[1].pitch as i32 - w[0].pitch as i32);
            let r = ((w[1].dur.max(1) as f64 / w[0].dur.max(1) as f64).log2() * 2.0).round() as i32;
            (iv, r.clamp(-4, 4))
        })
        .collect();
    let mut covered = vec![false; notes.len()];
    if tokens.len() >= 3 {
        let mut seen: std::collections::HashMap<&[(i32, i32)], Vec<usize>> =
            std::collections::HashMap::new();
        for i in 0..=tokens.len() - 3 {
            seen.entry(&tokens[i..i + 3]).or_default().push(i);
        }
        for starts in seen.values().filter(|v| v.len() >= 2) {
            for &s in starts {
                for c in covered.iter_mut().skip(s).take(4) {
                    *c = true;
                }
            }
        }
    }
    m.motif_coverage = covered.iter().filter(|c| **c).count() as f64 / notes.len() as f64;
    // シンコペーション: 8 分の裏で始まり、次の拍の頭に次の音が無い(拍をまたいで伸ばすか、拍の頭が休み)。
    // 8 分の連続の裏の音(次の拍に音がある)は数えない
    let mut sync = 0usize;
    let mut antic = 0usize;
    for (i, n) in notes.iter().enumerate() {
        let (_, idx, _, beat, num) = beat_info(n.pos);
        let b = bar_of(n.pos);
        let bar = grid.get(b).map_or(0, |g| g.0);
        let rel = (n.pos - bar) % beat;
        if rel.abs_diff(beat / 2) > 60 {
            continue;
        }
        let next_beat = n.pos + beat / 2;
        let held = notes.get(i + 1).map_or(true, |nx| nx.pos > next_beat + 60);
        if held && n.dur + 60 >= beat / 2 {
            sync += 1;
            let nb = idx + 1;
            if nb % num as u64 == 0 || (num == 4 && nb % 4 == 2) {
                antic += 1;
            }
        }
    }
    m.syncopation = sync as f64 / notes.len() as f64;
    m.anticipation = if sync == 0 {
        0.0
    } else {
        antic as f64 / sync as f64
    };
    // 驚き
    let info = surprise(&notes, key);
    m.surprise_mean = info.iter().sum::<f64>() / info.len() as f64;
    // 句: 1 拍以上の休み、または長い音(2 拍以上)の後の 8 分以上の休みで区切る
    let mut phrases = Vec::new();
    let mut start = 0usize;
    for i in 0..notes.len() {
        let last = i + 1 == notes.len();
        let gap = notes.get(i + 1).map_or(u64::MAX, |nx| {
            nx.pos.saturating_sub(notes[i].pos + notes[i].dur)
        });
        let long = notes[i].dur >= crate::time::PPQ * 2;
        if last || gap >= crate::time::PPQ || (long && gap >= crate::time::PPQ / 2) {
            let seg = &notes[start..=i];
            let mut durs: Vec<u64> = seg.iter().map(|n| n.dur).collect();
            let med = median_u64(&mut durs).max(1);
            let beats = (seg.last().map_or(0, |n| n.pos + n.dur) - seg[0].pos) as f64
                / crate::time::PPQ as f64;
            let inf = &info[start..=i];
            phrases.push(Phrase {
                start_bar: bar_of(seg[0].pos) + 1,
                bars: (beats / 4.0 * 10.0).round() / 10.0,
                notes: seg.len(),
                ending_ratio: ((seg[seg.len() - 1].dur as f64 / med as f64) * 100.0).round()
                    / 100.0,
                surprise_mean: (inf.iter().sum::<f64>() / inf.len() as f64 * 10.0).round() / 10.0,
                surprise_max: (inf.iter().cloned().fold(0.0, f64::max) * 10.0).round() / 10.0,
            });
            start = i + 1;
        }
    }
    // ---- 指摘 ----
    let pct = |x: f64| (x * 100.0).round();
    if m.leap_ratio > g.leap_max {
        warn(
            "warn",
            format!("跳躍が多すぎる(5 半音以上の動きが {}%、{} の目安 {}% まで)", pct(m.leap_ratio), g.name, pct(g.leap_max)),
            "隣の音へ動く(2 半音以内)音を増やす。跳躍は句の頭か山の直前に 1 つだけ(develop_motif の fill、transform_notes)",
        );
    } else if m.step_ratio > g.step.1 && m.range < 5 {
        warn(
            "info",
            format!(
                "順次進行ばかりで音域も狭い(順次 {}%、音域 {} 半音)。平板に聞こえやすい",
                pct(m.step_ratio),
                m.range
            ),
            "句の頭か山の直前に 5〜8 半音の跳躍を 1 つ入れ、その後は逆向きに戻す",
        );
    }
    if let Some(r) = m.leap_recovery {
        if m.leaps >= 2 && r < 0.6 {
            unrecovered.dedup();
            warn(
                "warn",
                format!(
                    "大きい跳躍の後に戻っていない(戻り {}%、研究では約 72%。{} 小節目など)",
                    pct(r),
                    unrecovered
                        .iter()
                        .take(4)
                        .map(|b| b.to_string())
                        .collect::<Vec<_>>()
                        .join("・")
                ),
                "7 半音以上跳んだら、次の音は逆向きに 2 度か 3 度(develop_motif の fill)",
            );
        }
    }
    if m.range > g.range.1 + 2 {
        warn(
            "warn",
            format!("音域が広すぎる(区間の中で {} 半音、{} の目安 {} 半音まで)", m.range, g.name, g.range.1),
            "山の区間以外は音域を狭くする。歌なら 19 半音以内(transpose_notes でオクターブを動かす)",
        );
    } else if m.range < g.range.0.saturating_sub(3) {
        warn(
            "info",
            format!(
                "音域が狭い({} 半音、{} の目安 {}〜{} 半音)",
                m.range, g.name, g.range.0, g.range.1
            ),
            "サビ・山では上へ広げる(ヴァースより 2〜5 半音高い中心)",
        );
    }
    // 最高音の位置: 計画の山(盛り上がりが最も高い区間)より前に出ていないか
    let peak_sec = ctx
        .sections
        .iter()
        .filter(|s| s.energy.is_some())
        .max_by(|a, b| {
            a.energy
                .partial_cmp(&b.energy)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    // 旋律が山の区間まで届かないとき(ヴァースだけ書いたとき)は比べられないので見ない
    let mel_end = notes.iter().map(|n| n.pos + n.dur).max().unwrap_or(0);
    if let Some(ps) = peak_sec.filter(|ps| mel_end > ps.start) {
        if first_peak < ps.start {
            let sec = ctx
                .sections
                .iter()
                .find(|s| s.start <= first_peak && first_peak < s.end)
                .map_or("前の区間".to_owned(), |s| format!("「{}」", s.name));
            warn(
                "warn",
                format!(
                    "最高音({})が山の区間「{}」より前の {} ({} 小節目)で出ている。山で新しい高さが出ない",
                    crate::chord::note_name(m.highest),
                    ps.name,
                    sec,
                    m.peak_bar
                ),
                "山より前の区間の最高音を下げる(transpose_notes・transform_notes)か、山の区間で 1〜3 半音高い音を出す",
            );
        }
    }
    if let Some(r) = m.strong_chord_tone {
        if r < g.strong_chord_tone {
            warn(
                "warn",
                format!(
                    "強拍に和音の外の音が多い(和音の音 {}%、目安 {}% 以上)",
                    pct(r),
                    pct(g.strong_chord_tone)
                ),
                "小節の頭・半ばの音を和音の音(1・3・5・7 度)に。外の音を置くなら次の音で 2 度で解決する",
            );
        }
        unresolved.dedup();
        if !unresolved.is_empty() && r >= g.strong_chord_tone {
            warn(
                "info",
                format!(
                    "強拍の和音の外の音が解決していない所がある({} 小節目)",
                    unresolved
                        .iter()
                        .take(5)
                        .map(|b| b.to_string())
                        .collect::<Vec<_>>()
                        .join("・")
                ),
                "次の音を 2 度上か下の和音の音へ",
            );
        }
    }
    if m.top_duration_share > 0.8 && m.notes >= 12 {
        warn(
            "warn",
            format!("リズムが単調(同じ長さの音が {}%)", pct(m.top_duration_share)),
            "句の終わりを長く伸ばす、付点・休符・食いを入れる(develop_motif の cadence・anticipate)",
        );
    }
    if bars_with.len() >= 4 {
        if m.rhythm_reuse < 0.3 {
            warn(
                "info",
                format!(
                    "リズムの型が小節ごとにばらばら(同じ型の小節 {}%)。動機が伝わりにくい",
                    pct(m.rhythm_reuse)
                ),
                "動機のリズムを決めて使い回す(develop_motif は動機のリズムを保って展開する)",
            );
        } else if g.breath && m.rhythm_reuse >= 0.85 {
            // 歌・管の旋律: 小節のリズムがほぼ同じだと単調(ループが基本の EDM・トラップは除く)
            warn(
                "warn",
                format!("小節のリズムがほぼ同じ(同じ型の小節 {}%)。単調に聞こえやすい", pct(m.rhythm_reuse)),
                "繰り返しの 2 回目以降のリズムを変える: develop_motif の vary(割る・付点・まとめる・休む)・\
                 diminish(速く 2 回)・augment(ゆっくり)・displace(n)(ずらして入る)。句の終わりは伸ばす",
            );
        } else if m.rhythm_reuse > 0.95 {
            warn(
                "info",
                "すべての小節が同じリズム".to_owned(),
                "区切りの前の小節だけリズムを変える(develop_motif の vary・displace)",
            );
        }
    }
    if m.notes >= 8 {
        if m.motif_coverage < 0.3 {
            warn(
                "warn",
                format!("繰り返される形が少ない(動機の反復率 {}%、目安 30% 以上)。覚えにくい", pct(m.motif_coverage)),
                "短い動機(1〜2 小節)を決め、移調・反復進行・和音への合わせ込みで繰り返す(develop_motif)",
            );
        } else if m.motif_coverage > g.repeat_max {
            warn(
                "warn",
                format!("同じ形の繰り返しが多すぎる(反復率 {}%、{} の上限 {}%)", pct(m.motif_coverage), g.name, pct(g.repeat_max)),
                "繰り返しの最後の 1 回だけ変える、断片化して終止へ向かう(develop_motif の sentence)",
            );
        }
    }
    if m.notes >= 16 && m.syncopation == 0.0 && g.syncopation.0 > 0.0 {
        warn(
            "info",
            "すべての音が表拍で始まる(食いが無い)".to_owned(),
            "1・3 拍目の音のいくつかを 8 分前へ食わせる(develop_motif の anticipate 0.2)",
        );
    } else if m.syncopation > g.syncopation.1 + 0.1 {
        warn(
            "info",
            format!(
                "食いが多すぎる(裏で始まる音 {}%、{} の目安 {}% まで)",
                pct(m.syncopation),
                g.name,
                pct(g.syncopation.1)
            ),
            "句の頭と終わりは表拍に置く",
        );
    }
    let long_phrases: Vec<usize> = phrases
        .iter()
        .filter(|p| p.bars > 4.2)
        .map(|p| p.start_bar)
        .collect();
    if g.breath && !long_phrases.is_empty() {
        warn(
            "warn",
            format!(
                "4 小節を超えて休み無しに続く句がある({} 小節目から)。息継ぎができない",
                long_phrases
                    .iter()
                    .take(3)
                    .map(|b| b.to_string())
                    .collect::<Vec<_>>()
                    .join("・")
            ),
            "2〜4 小節ごとに 8 分以上の休符を置く",
        );
    }
    let real: Vec<&Phrase> = phrases.iter().filter(|p| p.notes >= 3).collect();
    if real.len() >= 2 {
        let short = real.iter().filter(|p| p.ending_ratio < 1.5).count();
        if short * 2 > real.len() {
            warn(
                "warn",
                format!(
                    "句の終わりが伸びていない({} 句中 {} 句)。区切りが聞こえにくい",
                    real.len(),
                    short
                ),
                "句の最後の音を他の音の 1.5 倍以上に伸ばす(develop_motif の cadence)",
            );
        }
        // 驚き: 8 小節以上の旋律で、どの句にも目立つ驚きが無い / 平均が高すぎる
        let total_bars: f64 = phrases.iter().map(|p| p.bars).sum();
        let max = real.iter().map(|p| p.surprise_max).fold(0.0, f64::max);
        if total_bars >= 8.0 && max < 5.0 {
            warn(
                "info",
                format!("予想外の音が無い(驚きの最大 {max:.1} ビット)。無難だが印象に残りにくい"),
                "山の直前か句の頭に、跳躍・和音の外の音・シンコペーションを 1 つだけ置く",
            );
        }
        if m.surprise_mean > 5.5 {
            warn(
                "info",
                format!(
                    "予想しにくい音が多い(驚きの平均 {:.1} ビット)。まとまりが無く聞こえやすい",
                    m.surprise_mean
                ),
                "音階の音・隣の音への動きを増やし、驚きは 4 小節に 1〜2 か所にする",
            );
        }
    }
    let round3 = |x: f64| (x * 1000.0).round() / 1000.0;
    m.step_ratio = round3(m.step_ratio);
    m.leap_ratio = round3(m.leap_ratio);
    m.repeat_ratio = round3(m.repeat_ratio);
    m.leap_recovery = m.leap_recovery.map(round3);
    m.strong_chord_tone = m.strong_chord_tone.map(round3);
    m.top_duration_share = round3(m.top_duration_share);
    m.rhythm_reuse = round3(m.rhythm_reuse);
    m.motif_coverage = round3(m.motif_coverage);
    m.syncopation = round3(m.syncopation);
    m.anticipation = round3(m.anticipation);
    m.surprise_mean = (m.surprise_mean * 10.0).round() / 10.0;
    let warns = findings.iter().filter(|f| f.severity == "warn").count() as i32;
    let infos = findings.len() as i32 - warns;
    MelodyCritique {
        genre: g.name,
        key: key_name,
        metrics: m,
        phrases,
        findings,
        score: (100 - 12 * warns - 4 * infos).clamp(0, 100) as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chord::parse;

    const BAR: u64 = 3840;

    fn n(pos: u64, dur: u64, pitch: u8) -> MelNote {
        MelNote { pos, dur, pitch }
    }

    /// 1 小節ごとに和音(C | F | G | C の繰り返し)
    fn chords(t: u64) -> Option<Vec<u8>> {
        let names = ["C", "F", "G", "C"];
        Some(
            parse(names[((t / BAR) % 4) as usize])
                .unwrap()
                .unwrap()
                .pitch_classes(),
        )
    }

    fn ctx<'a>(p: &'a Project, f: &'a dyn Fn(u64) -> Option<Vec<u8>>) -> Context<'a> {
        Context {
            project: p,
            chord_at: f,
            key: Key::parse("C major"),
            genre: genre("pop").unwrap(),
            sections: vec![],
            target: "Lead".into(),
        }
    }

    /// 良い旋律の見本(C | F | G | C の上の 8 小節): 1・5 小節目は同じ動機、3 小節目はそのリズムで下へ、
    /// 強拍は和音の音、2 小節ごとの句の終わりを伸ばす、山(C5)は 6 小節目に 1 回
    fn good() -> Vec<MelNote> {
        let q = 960;
        let e = 480;
        let bars: [&[(u64, u64, u8)]; 8] = [
            &[
                (0, q, 64),
                (q, e, 62),
                (q + e, e, 60),
                (2 * q, q, 64),
                (3 * q, q, 67),
            ],
            &[(0, 3 * q, 69)],
            &[
                (0, q, 62),
                (q, e, 60),
                (q + e, e, 59),
                (2 * q, q, 62),
                (3 * q, q, 67),
            ],
            &[(0, 3 * q, 64)],
            &[
                (0, q, 64),
                (q, e, 62),
                (q + e, e, 60),
                (2 * q, q, 64),
                (3 * q, q, 67),
            ],
            &[(0, q, 72), (q, q, 69), (2 * q, 2 * q, 65)],
            &[
                (0, q, 71),
                (q, e, 69),
                (q + e, e, 67),
                (2 * q, q, 62),
                (3 * q, q, 59),
            ],
            &[(0, 4 * q, 60)],
        ];
        bars.iter()
            .enumerate()
            .flat_map(|(b, notes)| {
                notes
                    .iter()
                    .map(move |&(p, d, pitch)| n(b as u64 * BAR + p, d, pitch))
            })
            .collect()
    }

    #[test]
    fn a_shaped_motif_scores_well() {
        let p = Project::new("m");
        let f = chords;
        let c = critique(&good(), &ctx(&p, &f));
        assert!(c.metrics.motif_coverage > 0.3, "{:?}", c.metrics);
        assert!(c.metrics.leap_ratio < 0.3, "{:?}", c.metrics);
        assert_eq!((c.metrics.peak_count, c.metrics.peak_bar), (1, 6));
        assert_eq!(c.metrics.strong_chord_tone, Some(1.0));
        assert!(c.phrases.len() >= 3, "{:?}", c.phrases);
        assert!(
            c.phrases.iter().all(|p| p.ending_ratio >= 1.5),
            "{:?}",
            c.phrases
        );
        assert!(
            !c.findings.iter().any(|f| f.severity == "warn"),
            "{:?}",
            c.findings
        );
        assert!(c.score >= 80, "{}", c.score);
    }

    #[test]
    fn a_random_jumpy_monotone_line_is_flagged() {
        // 8 分の同じ長さで、大きく跳び回り、戻らず、休みも無い
        let pitches = [
            60u8, 71, 62, 74, 64, 76, 57, 69, 59, 72, 61, 75, 58, 70, 63, 77,
        ];
        let v: Vec<MelNote> = (0..64)
            .map(|i| n(i as u64 * 480, 480, pitches[(i * 7) % 16] + (i as u8 % 3)))
            .collect();
        let p = Project::new("m");
        let f = chords;
        let c = critique(&v, &ctx(&p, &f));
        let whats: Vec<&str> = c.findings.iter().map(|f| f.what.as_str()).collect();
        assert!(
            whats.iter().any(|w| w.contains("跳躍が多すぎる")),
            "{whats:?}"
        );
        assert!(
            whats.iter().any(|w| w.contains("リズムが単調")),
            "{whats:?}"
        );
        assert!(whats.iter().any(|w| w.contains("息継ぎ")), "{whats:?}");
        assert!(c.score < 60, "{}", c.score);
    }

    #[test]
    fn leaps_peaks_strong_beats_and_syncopation() {
        // 跳躍の後に戻らない(7 半音以上跳んで同じ向きに進む)
        let v = vec![
            n(0, 480, 60),
            n(480, 480, 67),
            n(960, 480, 69),
            n(1440, 480, 72),
            n(1920, 480, 79),
            n(2400, 480, 81),
            n(2880, 960, 84),
        ];
        let p = Project::new("m");
        let f = chords;
        let c = critique(&v, &ctx(&p, &f));
        assert_eq!(c.metrics.leaps, 2);
        assert_eq!(c.metrics.leap_recovery, Some(0.0));
        assert!(c.findings.iter().any(|f| f.what.contains("戻っていない")));
        // 最高音が山の区間より前
        let mut cx = ctx(&p, &f);
        cx.sections = vec![
            SectionSpan {
                name: "verse".into(),
                start: 0,
                end: 4 * BAR,
                energy: Some(4.0),
            },
            SectionSpan {
                name: "chorus".into(),
                start: 4 * BAR,
                end: 8 * BAR,
                energy: Some(8.0),
            },
        ];
        let mut w = good();
        w.push(n(BAR + 2880, 240, 88));
        let c = critique(&w, &cx);
        assert!(
            c.findings
                .iter()
                .any(|f| f.what.contains("山の区間「chorus」より前")),
            "{:?}",
            c.findings
        );
        // 強拍に和音の外の音(C の和音の上で D・F・B を 1 拍目に)
        let v: Vec<MelNote> = (0..8)
            .map(|i| n(i * BAR, 1920, [62u8, 71, 64, 65][i as usize % 4]))
            .collect();
        let c = critique(&v, &ctx(&p, &f));
        assert!(
            c.metrics.strong_chord_tone.unwrap() < 0.5,
            "{:?}",
            c.metrics
        );
        // 食い: 1 拍目の 8 分前(4 拍目の裏)で始まり伸ばす
        let v = vec![
            n(0, 960, 64),
            n(960, 960, 62),
            n(1920, 960, 60),
            n(2880 + 480, 1440, 67),
            n(BAR + 960, 960, 65),
            n(BAR + 1920, 1920, 64),
        ];
        let c = critique(&v, &ctx(&p, &f));
        assert!(c.metrics.syncopation > 0.0);
        assert_eq!(c.metrics.anticipation, 1.0);
    }

    #[test]
    fn surprise_is_lower_for_scale_steps_than_chromatic_leaps() {
        let key = Key::parse("C major").unwrap();
        let steps: Vec<MelNote> = [60u8, 62, 64, 65, 67, 65, 64, 62]
            .iter()
            .enumerate()
            .map(|(i, p)| n(i as u64 * 480, 480, *p))
            .collect();
        let leaps: Vec<MelNote> = [60u8, 73, 58, 78, 61, 70, 55, 75]
            .iter()
            .enumerate()
            .map(|(i, p)| n(i as u64 * 480, 480, *p))
            .collect();
        let a: f64 = surprise(&steps, key).iter().sum();
        let b: f64 = surprise(&leaps, key).iter().sum();
        assert!(b > a * 1.5, "{a} {b}");
        // キーの推定
        assert_eq!(guess_key(&steps), key);
        // 同時の音は一番高い音だけ
        let t = top_line(vec![n(0, 480, 60), n(0, 480, 72), n(480, 480, 64)]);
        assert_eq!(t.iter().map(|x| x.pitch).collect::<Vec<_>>(), vec![72, 64]);
        // オクターブ重ねは前の音に近い方
        let t = top_line(vec![
            n(0, 480, 64),
            n(480, 480, 65),
            n(480, 480, 77),
            n(960, 480, 67),
            n(960, 480, 79),
        ]);
        assert_eq!(
            t.iter().map(|x| x.pitch).collect::<Vec<_>>(),
            vec![64, 65, 67]
        );
    }
}
