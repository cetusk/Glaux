//! 旋律の動機を作る(MCP の write_melody の中身)。
//!
//! 動機のリズムはジャンルのリズムの型(1 小節ぶんの 16 分の格子)から選び、2 小節の動機では
//! 1 小節目を動く型、2 小節目を伸ばす型にして、動機の中でリズムに対比を作る。
//! 音は輪郭の型(弧・上行・下行・谷・同じ音のフック)に沿わせ、強拍は和音の音、弱拍は音階の音で、
//! 順次進行を主にし、跳躍の後は戻る。展開は `motif::develop`、選ぶのは点検(`melody::critique`)の点数。

use crate::chord::{Chord, Key};
use crate::motif::{MotifNote, Op};

/// 1 小節のリズムの型。16 分 1 つが 1 文字: x = 音の頭 / - = 前を続ける(音なら伸ばし、休みなら休み)/ . = 休み。
/// 歌・管のジャンルの伸ばす型は、2 拍以上伸ばした後に 8 分休む(息継ぎ。点検の句の区切りと同じ条件)
#[derive(Debug)]
pub struct Cell {
    pub steps: &'static str,
    /// 伸ばして終わる型(2 小節の動機の 2 小節目に使う)
    pub held: bool,
}

const fn c(steps: &'static str) -> Cell {
    Cell { steps, held: false }
}

const fn h(steps: &'static str) -> Cell {
    Cell { steps, held: true }
}

/// ジャンルのリズムの語彙
pub struct Vocab {
    pub genre: &'static str,
    pub cells: &'static [Cell],
    /// 強拍の音を 8 分前へ食わせる割合の既定
    pub anticipate: f64,
}

pub const VOCABS: &[Vocab] = &[
    Vocab {
        genre: "pop",
        cells: &[
            c("x-x-x-x-x---x---"),
            c("x---x-x-x---x-x-"),
            c("x-x-x---x-x-x---"),
            c("..x-x-x-x---x---"),
            c("x--x--x-x-x-x---"),
            c("x-x-x-x-x-------"),
            c("x---x---x-x-x---"),
            c("..x-x-x-x-x-x---"),
            h("x-----x-------.."),
            h("x---x-x-------.."),
            h("x-x-x---------.."),
            h("x---------..x-x-"),
            h("x---x---------.."),
        ],
        anticipate: 0.2,
    },
    Vocab {
        genre: "edm",
        // 伸ばしてつなぐ型だけでなく、短く切って休む型(プラック・スタブ)を多めに。トレシーロ(3-3-2)・
        // 裏拍・問いと答え・8 分の途中で割る型(docs の実践の調査)
        cells: &[
            c("x--x--x---x--x--"),
            c("x-.x-.x.x-.x-.x."),
            c("x..x..x.x..x..x."),
            c("..x...x...x.x-.."),
            c("x-x-x-x-xxx-x-x-"),
            c("x.x..x.x..x.x-.."),
            c("..x--x--x-x-x-.."),
            h("x--x--x-------.."),
            h("x..x..x.x-......"),
            h("....x.x.x.x.x---"),
        ],
        anticipate: 0.35,
    },
    Vocab {
        genre: "trap",
        cells: &[
            c("x---x-x---x-----"),
            c("x-x-..x-x---..x-"),
            c("x-----x---x-x---"),
            c("..x-x---x---x---"),
            h("x-------x-----.."),
            h("x-x-x---------.."),
            h("x-----------x---"),
        ],
        anticipate: 0.15,
    },
    Vocab {
        genre: "lofi",
        cells: &[
            c("..x-x---x-----.."),
            c("x-----x-x-------"),
            c("..x-x-x---x-----"),
            c("x---..x-x---x---"),
            h("x-------------.."),
            h("..x-----------.."),
            h("x-----x-------.."),
        ],
        anticipate: 0.1,
    },
    Vocab {
        genre: "jazz",
        cells: &[
            c("x-x-x-x-x-x-x-x-"),
            c("..x-x-x-x-x-x-x-"),
            c("x-x-x-x-x---x-x-"),
            c("x---x-x-x-x-x---"),
            h("x-x-x-x-------.."),
            h("x---x-x-------.."),
            h("x-------..x-x-x-"),
        ],
        anticipate: 0.1,
    },
    Vocab {
        genre: "funk",
        cells: &[
            c("x..x..x...x.x..."),
            c("x.x...x..x..x..."),
            c("..x.x..x..x.x..."),
            c("x..x.x...x.x..x."),
            h("x..x..x---......"),
            h("x.x...x-------.."),
        ],
        anticipate: 0.3,
    },
];

pub fn vocab(genre: &str) -> &'static Vocab {
    let g = crate::melody::genre(genre).map_or("pop", |g| g.name);
    VOCABS.iter().find(|v| v.genre == g).unwrap_or(&VOCABS[0])
}

/// 輪郭の型
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Contour {
    Arch,
    Rise,
    Fall,
    Valley,
    /// 同じ音を叩くフック
    FlatHook,
}

impl Contour {
    pub fn parse(s: &str) -> Option<Contour> {
        Some(match s.trim().to_lowercase().as_str() {
            "arch" => Contour::Arch,
            "rise" => Contour::Rise,
            "fall" => Contour::Fall,
            "valley" => Contour::Valley,
            "flat_hook" | "flat" | "hook" => Contour::FlatHook,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Contour::Arch => "arch",
            Contour::Rise => "rise",
            Contour::Fall => "fall",
            Contour::Valley => "valley",
            Contour::FlatHook => "flat_hook",
        }
    }

    /// 動機の中の位置 t(0〜1)での、始まりの音からの高さ(振れ幅 1 あたり)
    fn at(self, t: f64) -> f64 {
        let s = (std::f64::consts::PI * t).sin();
        match self {
            Contour::Arch => s,
            Contour::Rise => t,
            Contour::Fall => -t,
            Contour::Valley => -s,
            // 前半は同じ高さを叩き、後半で上がって戻る
            Contour::FlatHook => {
                if t < 0.45 {
                    0.0
                } else {
                    0.5 * (std::f64::consts::PI * (t - 0.45) / 0.55).sin()
                }
            }
        }
    }
}

/// 役割ごとの既定
#[derive(Debug)]
pub struct Role {
    pub name: &'static str,
    pub forms: &'static [&'static str],
    pub contours: &'static [Contour],
    /// 既定の音域
    pub range: (u8, u8),
    /// 動機の振れ幅(半音)
    pub span: f64,
    /// 始まりの音の高さ(音域の下からの割合)
    pub start: f64,
    /// 1 小節の音の数の上限(伸ばす型は除く)
    pub max_onsets: usize,
    pub note: &'static str,
}

pub const ROLES: &[Role] = &[
    Role {
        name: "verse",
        forms: &["period", "call_response"],
        contours: &[Contour::Fall, Contour::Arch, Contour::Valley],
        range: (60, 74),
        span: 6.0,
        start: 0.35,
        max_onsets: 6,
        note: "A メロ: 低く、音を少なく、語るように。問い → 答え",
    },
    Role {
        name: "pre",
        forms: &["sentence", "aab"],
        contours: &[Contour::Rise, Contour::Arch],
        range: (62, 77),
        span: 8.0,
        start: 0.4,
        max_onsets: 7,
        note: "B メロ: 上がっていき、サビの前で溜める",
    },
    Role {
        name: "chorus",
        forms: &["sentence", "aab", "period"],
        contours: &[Contour::Arch, Contour::FlatHook, Contour::Rise],
        range: (64, 79),
        span: 9.0,
        start: 0.55,
        max_onsets: 8,
        note: "サビ: 高く、伸ばす音とフックの繰り返し",
    },
    Role {
        name: "hook",
        forms: &["loop", "sentence"],
        contours: &[Contour::FlatHook, Contour::Arch, Contour::Fall],
        range: (64, 79),
        span: 5.0,
        start: 0.5,
        max_onsets: 8,
        note: "フック: 短い動機を繰り返し、最後だけ変える",
    },
    Role {
        name: "lead",
        forms: &["loop", "sentence"],
        contours: &[Contour::Arch, Contour::Fall, Contour::FlatHook],
        range: (72, 86),
        span: 7.0,
        start: 0.5,
        max_onsets: 9,
        note: "シンセのリード: 高め、ループ",
    },
];

pub fn role(name: &str) -> Option<&'static Role> {
    let n = name.trim().to_lowercase();
    let n = match n.as_str() {
        "a" | "amelo" | "a_melo" | "verse" => "verse",
        "b" | "bmelo" | "b_melo" | "prechorus" | "pre_chorus" | "pre-chorus" | "build" => "pre",
        "sabi" | "chorus" | "drop" => "chorus",
        other => other,
    };
    ROLES.iter().find(|r| r.name == n)
}

/// 16 分の格子の文字列を読む("|" と空白は読み飛ばす)。音の (頭からの位置, 長さ) を返す
pub fn parse_grid(s: &str, step: u64) -> Result<(Vec<(u64, u64)>, u64), String> {
    let chars: Vec<char> = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '|')
        .collect();
    if chars.is_empty() || chars.len() > 128 {
        return Err("rhythm は x - . の 1〜128 文字(16 分 1 つが 1 文字)".to_owned());
    }
    let mut out: Vec<(u64, u64)> = Vec::new();
    let mut sounding = false;
    for (i, ch) in chars.iter().enumerate() {
        let t = i as u64 * step;
        match ch {
            'x' | 'X' => {
                out.push((t, step));
                sounding = true;
            }
            '-' => {
                if sounding {
                    if let Some(last) = out.last_mut() {
                        last.1 += step;
                    }
                }
            }
            '.' => sounding = false,
            _ => return Err(format!("rhythm に使えない文字「{ch}」(x - . だけ)")),
        }
    }
    if out.is_empty() {
        return Err("rhythm に音の頭(x)がありません".to_owned());
    }
    Ok((out, chars.len() as u64 * step))
}

/// 小節に合わせる。4/4 以外は拍のまとまりに当てる([`crate::meter::fit_pattern`]。息継ぎの休みは残す)。
/// 当てられない長さの小節は、はみ出す音を落として長さを切る(足りなければ最後の音を伸ばす)
fn fit_cell(cell: &str, meter: &crate::meter::BarMeter) -> Vec<(u64, u64)> {
    let src: Vec<char> = cell.chars().collect();
    if let Some(mut v) = crate::meter::fit_pattern(&src, Some(meter)) {
        let n = v.len();
        if n >= 2 && src.ends_with(&['.', '.']) {
            v[n - 2] = '.';
            v[n - 1] = '.';
        }
        let text: String = v.into_iter().collect();
        if let Ok((notes, _)) = parse_grid(&text, crate::meter::STEP) {
            return notes;
        }
    }
    let bar_len = meter.len;
    let step = bar_len.min(3840) / 16;
    let (mut notes, len) = parse_grid(cell, step).unwrap_or_default();
    notes.retain(|n| n.0 < bar_len);
    for n in &mut notes {
        n.1 = n.1.min(bar_len - n.0);
    }
    if len < bar_len {
        if let Some(last) = notes.last_mut() {
            last.1 = bar_len - last.0;
        }
    }
    notes
}

/// 動機のリズムを選ぶ。1 小節目は動く型、2 小節目は伸ばす型(EDM などのループは動く型どうし)
pub fn pick_rhythm(
    vocab: &Vocab,
    role: &Role,
    bars: u64,
    meter: &crate::meter::BarMeter,
    vocal: bool,
    seed: u64,
) -> (Vec<(u64, u64)>, String) {
    let bar_len = meter.len;
    let mut st = mix(seed ^ 0x2B);
    let onsets = |c: &Cell| c.steps.chars().filter(|&ch| ch == 'x').count();
    let busy: Vec<&Cell> = vocab
        .cells
        .iter()
        .filter(|c| !c.held && onsets(c) <= role.max_onsets)
        .collect();
    let busy: Vec<&Cell> = if busy.is_empty() {
        vocab.cells.iter().filter(|c| !c.held).collect()
    } else {
        busy
    };
    let held: Vec<&Cell> = vocab.cells.iter().filter(|c| c.held).collect();
    let pick = |list: &[&'static Cell], st: &mut u64| -> &'static Cell {
        let i = (next_unit(st) * list.len() as f64) as usize;
        list[i.min(list.len() - 1)]
    };
    let mut out = Vec::new();
    let mut names = Vec::new();
    let mut prev: Option<&str> = None;
    for b in 0..bars {
        let last = b + 1 == bars && bars > 1;
        let cell = if last && vocal && !held.is_empty() {
            pick(&held, &mut st)
        } else {
            // 前の小節と同じ型は避ける
            let mut c = pick(&busy, &mut st);
            if prev == Some(c.steps) && busy.len() > 1 {
                c = pick(&busy, &mut st);
            }
            c
        };
        prev = Some(cell.steps);
        names.push(cell.steps);
        out.extend(
            fit_cell(cell.steps, meter)
                .into_iter()
                .map(|(o, d)| (b * bar_len + o, d)),
        );
    }
    (out, names.join("|"))
}

/// 動機の音を選ぶ条件
pub struct MotifSpec<'a> {
    pub key: Key,
    pub contour: Contour,
    /// 作業する音域(動機はこの中に作り、展開で音域に合わせ直す)
    pub low: u8,
    pub high: u8,
    /// 始まりの音の目安
    pub start: u8,
    /// 振れ幅(半音)
    pub span: f64,
    pub bar_len: u64,
    /// 強拍(小節の頭からの tick)。空なら小節の頭と半ば
    pub strong: Vec<u64>,
    /// 動機の頭からの tick → 和音
    pub chord_at: &'a dyn Fn(u64) -> Option<Chord>,
    pub seed: u64,
}

/// リズムに音を当てて動機にする
pub fn make_motif(rhythm: &[(u64, u64)], spec: &MotifSpec) -> Vec<MotifNote> {
    let scale_pcs = crate::harmony::scale_pitch_classes(
        spec.key.tonic,
        if spec.key.minor { "minor" } else { "major" },
    );
    let tonic_triad: Vec<u8> = [0usize, 2, 4].iter().map(|&i| scale_pcs[i]).collect();
    let beat = crate::time::PPQ;
    let half = (spec.bar_len / 2).max(1);
    let end = rhythm.iter().map(|r| r.0 + r.1).max().unwrap_or(1).max(1);
    let mut st = mix(spec.seed ^ 0x3C);
    let candidates: Vec<i32> = (spec.low as i32..=spec.high as i32)
        .filter(|p| scale_pcs.contains(&(p.rem_euclid(12) as u8)))
        .collect();
    let mut out: Vec<MotifNote> = Vec::new();
    for (i, &(off, dur)) in rhythm.iter().enumerate() {
        let tones = (spec.chord_at)(off).map_or_else(|| tonic_triad.clone(), |c| c.pitch_classes());
        let t = off as f64 / end as f64;
        let target = spec.start as f64 + spec.span * spec.contour.at(t);
        let strong = if spec.strong.is_empty() {
            off % half == 0
        } else {
            spec.strong.contains(&(off % spec.bar_len.max(1)))
        };
        let on_beat = off % beat == 0;
        let last = i + 1 == rhythm.len();
        let prev = out.last().map(|n| n.pitch as i32);
        // 同じ音が 2 回続いた後か
        let twice = out.len() >= 2 && out[out.len() - 1].pitch == out[out.len() - 2].pitch;
        let prev_step = if out.len() >= 2 {
            Some(out[out.len() - 1].pitch as i32 - out[out.len() - 2].pitch as i32)
        } else {
            None
        };
        let cost = |p: i32, jitter: f64| -> f64 {
            let pc = p.rem_euclid(12) as u8;
            let chord_tone = tones.contains(&pc);
            let mut c = (p as f64 - target).abs() * 0.5 + jitter;
            if let Some(q) = prev {
                let iv = (p - q).abs();
                c += match iv {
                    0 if twice => 3.0,
                    0 => {
                        if spec.contour == Contour::FlatHook {
                            -0.6
                        } else {
                            1.2
                        }
                    }
                    1 | 2 => 0.0,
                    3 | 4 => 0.9,
                    5 | 7 => 2.2,
                    6 => 4.0,
                    8..=9 => 3.5,
                    _ => 6.0 + iv as f64,
                };
                // 隣の音を行き来するだけ(A–B–A)にしない。順次で進んできたら同じ向きに続けやすく(step inertia)
                if out.len() >= 2 {
                    let back = out[out.len() - 2].pitch as i32;
                    if p == back && p != q {
                        c += 1.4;
                    }
                }
                if let Some(d) = prev_step {
                    if (1..=2).contains(&d.abs()) && p != q {
                        c += if (p - q).signum() == d.signum() {
                            -0.3
                        } else {
                            0.3
                        };
                    }
                }
                // 跳躍の後は向きを変えて順次に
                if let Some(d) = prev_step {
                    if d.abs() >= 5 {
                        let dir = (p - q).signum();
                        if dir == d.signum() || dir == 0 {
                            c += 3.0;
                        } else if iv <= 2 {
                            c -= 0.8;
                        }
                    }
                }
            }
            if !chord_tone {
                if strong {
                    c += 4.0;
                } else if on_beat || dur >= beat {
                    c += 1.6;
                }
            }
            if last {
                if !chord_tone {
                    c += 3.0;
                }
                // 句の終わりは下がる
                if prev.is_some_and(|q| p > q) {
                    c += 0.6;
                }
            }
            c
        };
        let jitters: Vec<f64> = candidates
            .iter()
            .map(|_| next_unit(&mut st) * 1.4)
            .collect();
        let best = candidates
            .iter()
            .zip(&jitters)
            .min_by(|a, b| cost(*a.0, *a.1).total_cmp(&cost(*b.0, *b.1)))
            .map_or(spec.start as i32, |(p, _)| *p);
        out.push(MotifNote {
            offset: off,
            dur,
            pitch: best.clamp(0, 127) as u8,
        });
    }
    out
}

/// 形式をスロットの数に合わせる。多ければ繰り返し(途中の終止は問いに、2 回目の頭はリズムを変えて)、
/// 少なければ切って最後を終止に
pub fn fit_form(form: &[Vec<Op>], slots: usize) -> Vec<Vec<Op>> {
    let n = form.len().max(1);
    if slots == n {
        return form.to_vec();
    }
    let mut out: Vec<Vec<Op>> = Vec::new();
    for i in 0..slots {
        let rep = i / n;
        let mut ops = form[i % n].clone();
        let last_of_rep = i % n == n - 1 || i + 1 == slots;
        if i + 1 < slots && last_of_rep {
            ops.retain(|o| *o != Op::Cadence);
            if !ops.contains(&Op::Tail) {
                ops.push(Op::Tail);
            }
        }
        if rep > 0 && i % n == 0 {
            ops = vec![Op::Adapt, Op::Vary];
        }
        if i + 1 == slots {
            ops.retain(|o| *o != Op::Tail);
            if !ops.contains(&Op::Cadence) {
                if !ops.contains(&Op::Adapt) {
                    ops.push(Op::Adapt);
                }
                ops.push(Op::Cadence);
            }
        }
        out.push(ops);
    }
    out
}

/// 動機を "E5:q D5:e r:e ..." の形に書く(develop_motif にそのまま渡せる)
pub fn format_motif(notes: &[MotifNote]) -> String {
    let len = |d: u64| -> String {
        match d {
            3840 => "w".into(),
            2880 => "h.".into(),
            1920 => "h".into(),
            1440 => "q.".into(),
            960 => "q".into(),
            720 => "e.".into(),
            480 => "e".into(),
            360 => "s.".into(),
            240 => "s".into(),
            120 => "t".into(),
            other => other.to_string(),
        }
    };
    let mut parts = Vec::new();
    let mut t = 0u64;
    for n in notes {
        if n.offset > t {
            parts.push(format!("r:{}", len(n.offset - t)));
        }
        parts.push(format!(
            "{}:{}",
            crate::chord::note_name(n.pitch),
            len(n.dur)
        ));
        t = n.offset + n.dur;
    }
    parts.join(" ")
}

/// 種を混ぜる(splitmix64。近い種でも離れた列になる)
fn mix(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn next_unit(state: &mut u64) -> f64 {
    let mut x = (*state).max(1);
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *state = x;
    (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chord::parse;

    #[test]
    fn grid_reads_holds_and_rests() {
        let (n, len) = parse_grid("x-x.|x---", 240).unwrap();
        assert_eq!(n, vec![(0, 480), (480, 240), (960, 960)]);
        assert_eq!(len, 1920);
        assert!(parse_grid("x-q", 240).is_err());
        assert!(parse_grid("....", 240).is_err());
    }

    #[test]
    fn every_cell_is_one_bar() {
        for v in VOCABS {
            for c in v.cells {
                assert_eq!(c.steps.len(), 16, "{} {}", v.genre, c.steps);
                assert!(parse_grid(c.steps, 240).is_ok(), "{}", c.steps);
            }
            assert!(v.cells.iter().any(|c| c.held), "{}", v.genre);
        }
    }

    #[test]
    fn two_bar_rhythm_contrasts() {
        let r = role("chorus").unwrap();
        for seed in 1..10 {
            let (notes, name) = pick_rhythm(
                vocab("pop"),
                r,
                2,
                &crate::meter::BarMeter::common(0),
                true,
                seed,
            );
            let (a, b) = name.split_once('|').unwrap();
            assert_ne!(a, b);
            let first = notes.iter().filter(|n| n.0 < 3840).count();
            let second = notes.len() - first;
            assert!(first > second, "{name}");
        }
        // 3/4・7/8 の小節でもはみ出さず、16 分の格子の上
        for (num, den) in [(3u8, 4u8), (7, 8), (5, 8)] {
            let sig = crate::time::TimeSigEvent::new(crate::time::Tick(0), num, den);
            let len = 3840 * num as u64 / den as u64;
            let m = crate::meter::BarMeter::from_sig(&sig, 0, len);
            for seed in 1..6 {
                let (notes, _) = pick_rhythm(vocab("pop"), r, 2, &m, true, seed);
                assert!(!notes.is_empty());
                assert!(
                    notes
                        .iter()
                        .all(|n| (n.0 % len) + n.1 <= len && n.0 % 240 == 0),
                    "{num}/{den} {notes:?}"
                );
            }
        }
    }

    #[test]
    fn motif_puts_chord_tones_on_strong_beats() {
        let chords = [parse("C").unwrap().unwrap(), parse("Am").unwrap().unwrap()];
        let at = |t: u64| Some(chords[(t / 3840).min(1) as usize].clone());
        let r = role("chorus").unwrap();
        for seed in 1..8 {
            let (rhythm, _) = pick_rhythm(
                vocab("pop"),
                r,
                2,
                &crate::meter::BarMeter::common(0),
                true,
                seed,
            );
            let spec = MotifSpec {
                key: Key {
                    tonic: 0,
                    minor: false,
                },
                contour: Contour::Arch,
                low: 60,
                high: 81,
                start: 72,
                span: 7.0,
                bar_len: 3840,
                strong: vec![],
                chord_at: &at,
                seed,
            };
            let m = make_motif(&rhythm, &spec);
            assert_eq!(m.len(), rhythm.len());
            for n in &m {
                assert!([0, 2, 4, 5, 7, 9, 11].contains(&(n.pitch % 12)));
                if n.offset % 1920 == 0 {
                    let c = at(n.offset).unwrap();
                    assert!(c.pitch_classes().contains(&(n.pitch % 12)), "{m:?}");
                }
            }
            // 大きい跳躍は無い
            for w in m.windows(2) {
                assert!((w[1].pitch as i32 - w[0].pitch as i32).abs() <= 9, "{m:?}");
            }
        }
    }

    #[test]
    fn forms_fit_the_slot_count() {
        let f = crate::motif::form("sentence").unwrap();
        let eight = fit_form(&f, 8);
        assert_eq!(eight.len(), 8);
        assert!(eight[3].contains(&Op::Tail) && !eight[3].contains(&Op::Cadence));
        assert!(eight[4].contains(&Op::Vary));
        assert!(eight[7].contains(&Op::Cadence));
        let two = fit_form(&f, 2);
        assert!(two[1].contains(&Op::Cadence));
        assert_eq!(fit_form(&f, 4), f);
    }

    #[test]
    fn motif_text_round_trips() {
        let m = vec![
            MotifNote {
                offset: 480,
                dur: 480,
                pitch: 76,
            },
            MotifNote {
                offset: 960,
                dur: 1440,
                pitch: 74,
            },
        ];
        let s = format_motif(&m);
        assert_eq!(s, "r:e E5:e D5:q.");
        assert_eq!(crate::motif::parse_motif(&s).unwrap(), m);
    }
}
