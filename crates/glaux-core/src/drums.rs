//! ドラムの型(MCP の write_drums の中身)。ジャンルの型を、音の多さ・小節ごとの変化・フィル・ビルドのロールと一緒に置く。
//!
//! 型は 4/4 の 16 ステップの文字列: `X` = 強く、`x` = 普通、`g` = ゴースト(ごく弱く)、`r` = 32 分 2 つ、
//! `t` = 3 連の 32 分(トラップのハットのロール)、`.` = 休み。他の拍子の小節は、1 小節を 16 等分して当てる。
//! 音程は GM(キック 36・スネア 38・クラップ 39・リム 37・クローズハット 42・オープンハット 46・ライド 51・
//! クラッシュ 49・タム 50 / 47 / 45 / 43)。乱数は `seed` から作る(同じ入力なら同じ結果)。

/// 楽器ごとの型
pub struct Style {
    pub name: &'static str,
    /// 目安のテンポ
    pub bpm: u32,
    pub note: &'static str,
    /// キック(最初が基本。ほかは小節ごとの変化)
    pub kick: &'static [&'static str],
    /// スネアかクラップの線 (音程, 型)
    pub snare: &'static [(u8, &'static str)],
    /// ハット(音の少ない順に 3 段階)
    pub hats: [&'static str; 3],
    /// オープンハット(音の多さが中以上で)
    pub open_hat: Option<&'static str>,
}

pub const STYLES: &[Style] = &[
    Style {
        name: "house",
        bpm: 124,
        note: "4 つ打ち・2 と 4 にクラップ・裏のオープンハット",
        kick: &["X...X...X...X..."],
        snare: &[(39, "....X.......X...")],
        hats: ["................", "x.x.x.x.x.x.x.x.", "xxxxxxxxxxxxxxxx"],
        open_hat: Some("..x...x...x...x."),
    },
    Style {
        name: "techno",
        bpm: 130,
        note: "4 つ打ち・16 分のハット・裏のオープンハット",
        kick: &["X...X...X...X...", "X...X...X...X.g.", "X...X...X..gX..."],
        snare: &[(37, "....x.......x...")],
        hats: ["..x...x...x...x.", "x.x.x.x.x.x.x.x.", "xxxxxxxxxxxxxxxx"],
        open_hat: Some("..x...x...x...x."),
    },
    Style {
        name: "trap",
        bpm: 140,
        note: "ハーフタイム(スネアは 3 拍目)・ハットの連打",
        kick: &["X......X..X.....", "X.....X...X...X.", "X..X......X....."],
        snare: &[(38, "........X.......")],
        hats: ["x.x.x.x.x.x.x.x.", "xxxxxxxxxxxxxtxx", "xxxxrxxxxxxtxxrx"],
        open_hat: None,
    },
    Style {
        name: "hiphop",
        bpm: 90,
        note: "ブームバップ: 2 と 4 のスネア・8 分のハット",
        kick: &["X.........X.X...", "X......X..X.....", "X.......X.X....."],
        snare: &[(38, "....X.......X...")],
        hats: ["x...x...x...x...", "x.x.x.x.x.x.x.x.", "x.xxx.x.x.xxx.x."],
        open_hat: None,
    },
    Style {
        name: "lofi",
        bpm: 80,
        note: "ゆったり・少し欠けたハット(スウィングを後から)",
        kick: &["X.......x.X.....", "X......x..X.....", "X.........x.X..."],
        snare: &[(38, "....X.......X...")],
        hats: ["x...x...x...x...", "x.x.x.x.x.x.xxx.", "x.xxx.x.x.xxx.xx"],
        open_hat: None,
    },
    Style {
        name: "funk",
        bpm: 105,
        note: "16 分のシンコペーションのキック・ゴースト入りのスネア",
        kick: &["X..X..X...X..X..", "X..X...X..X..X..", "X.X...X...X..X.."],
        snare: &[(38, "....X..g.g..X..g")],
        hats: ["X.x.X.x.X.x.X.x.", "XxXxXxXxXxXxXxXx", "XxXxXxXxXxXxXxXx"],
        open_hat: Some("..............x."),
    },
    Style {
        name: "rock",
        bpm: 120,
        note: "8 ビート",
        kick: &["X.......X.X.....", "X.X.....X.......", "X......XX.X....."],
        snare: &[(38, "....X.......X...")],
        hats: ["X...X...X...X...", "X.x.X.x.X.x.X.x.", "XxxxXxxxXxxxXxxx"],
        open_hat: None,
    },
    Style {
        name: "pop",
        bpm: 100,
        note: "ポップの 8 ビート",
        kick: &["X.....X.X.......", "X.....X.X.X.....", "X.......X.....X."],
        snare: &[(38, "....X.......X...")],
        hats: ["x...x...x...x...", "x.x.x.x.x.x.x.x.", "xxxxxxxxxxxxxxxx"],
        open_hat: Some("..............x."),
    },
    Style {
        name: "dnb",
        bpm: 174,
        note: "ドラムンベース: 2 と 4 のスネア・細かいキック",
        kick: &["X.........X.....", "X.X.......X.....", "X.........XX...."],
        snare: &[(38, "....X.......X...")],
        hats: ["x.x.x.x.x.x.x.x.", "xxxxxxxxxxxxxxxx", "xxxxxxxxxxxxxxxx"],
        open_hat: None,
    },
    Style {
        name: "disco",
        bpm: 120,
        note: "4 つ打ち・2 と 4 のスネア・裏のオープンハット",
        kick: &["X...X...X...X..."],
        snare: &[(38, "....X.......X...")],
        hats: ["x.x.x.x.x.x.x.x.", "x.x.x.x.x.x.x.x.", "xxxxxxxxxxxxxxxx"],
        open_hat: Some("..X...X...X...X."),
    },
    Style {
        name: "reggaeton",
        bpm: 95,
        note: "デンボウ: 4 つ打ちと 3-3-2 のスネア",
        kick: &["X...X...X...X..."],
        snare: &[(38, "...X..X....X..X.")],
        hats: ["x.x.x.x.x.x.x.x.", "x.x.x.x.x.x.x.x.", "xxxxxxxxxxxxxxxx"],
        open_hat: None,
    },
    Style {
        name: "halftime",
        bpm: 140,
        note: "ハーフタイム(ダブステップ・フューチャーベース): スネアは 3 拍目",
        kick: &["X.........X.....", "X......X..X.....", "X.........X...x."],
        snare: &[(38, "........X.......")],
        hats: ["x...x...x...x...", "x.x.x.x.x.x.x.x.", "xxxxxxxxxxxxxxxx"],
        open_hat: None,
    },
    Style {
        name: "2step",
        bpm: 130,
        note: "2 ステップ: 2・4 拍目のキックを抜いた 4 つ打ち(スウィングを後から)",
        kick: &["X.........X..X..", "X.......X.X.....", "X.........X....."],
        snare: &[(38, "....X.......X...")],
        hats: ["..x...x...x...x.", "x.x.x.x.x.x.x.x.", "xxxxxxxxxxxxxxxx"],
        open_hat: None,
    },
];

pub fn style(name: &str) -> Option<&'static Style> {
    STYLES
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(name.trim()))
}

/// フィルの種類
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fill {
    None,
    /// 3・4 拍目にスネアの 16 分(だんだん強く)
    Snare,
    /// 3・4 拍目にタムを高い方から
    Toms,
}

/// 置き方
#[derive(Clone, Debug)]
pub struct Options {
    /// 音の多さ 0〜1(ハットの細かさ・オープンハット)
    pub intensity: f64,
    /// 小節ごとの変化の度合い 0〜1(キックの型を入れ替える割合)
    pub variation: f64,
    pub fill: Fill,
    /// 何小節ごとにフィルを入れるか(0 で入れない)
    pub fill_every: u32,
    /// 最初の小節の頭と、フィルの次の小節の頭にクラッシュ
    pub crash: bool,
    /// 最後の何小節をスネアのロールのビルドにするか(0 で無し)
    pub build_bars: u32,
    /// 最後の何拍を無音にするか(ドロップ直前の無音)
    pub gap_beats: u32,
    pub seed: u64,
}

/// ドラムのノート 1 つ(クリップの頭から)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub pos: u64,
    pub dur: u64,
    pub pitch: u8,
    pub vel: u8,
}

/// 同じ seed から同じ列を作る小さな乱数
struct Rng(u64);
impl Rng {
    fn unit(&mut self) -> f64 {
        let mut x = self.0.max(1);
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// 1 小節の型の文字列をノートにする(`from`〜`to` のステップだけ)
fn line(
    out: &mut Vec<Hit>,
    pat: &str,
    pitch: u8,
    bar: u64,
    len: u64,
    range: std::ops::Range<usize>,
) {
    let chars: Vec<char> = pat.chars().collect();
    let n = chars.len().max(1) as u64;
    let at = |k: f64| bar + (k * len as f64 / n as f64).round() as u64;
    for (k, &c) in chars.iter().enumerate() {
        if !range.contains(&k) {
            continue;
        }
        let k = k as f64;
        let mut push = |pos: u64, vel: u8| {
            out.push(Hit {
                pos,
                dur: (len / n / 2).max(30),
                pitch,
                vel,
            })
        };
        match c {
            'X' => push(at(k), 112),
            'x' => push(at(k), 88),
            'g' => push(at(k), 34),
            'r' => {
                push(at(k), 76);
                push(at(k + 0.5), 62);
            }
            't' => {
                push(at(k), 76);
                push(at(k + 1.0 / 3.0), 64);
                push(at(k + 2.0 / 3.0), 70);
            }
            _ => {}
        }
    }
}

/// `bars`(クリップの頭からの (小節の頭, 小節の長さ))に、型を置く
pub fn render(style: &Style, bars: &[(u64, u64)], opts: &Options) -> Vec<Hit> {
    let mut rng = Rng(opts.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD5A1);
    let level = if opts.intensity < 0.34 {
        0
    } else if opts.intensity < 0.67 {
        1
    } else {
        2
    };
    let n = bars.len();
    let build_from = n.saturating_sub(opts.build_bars as usize);
    let mut out = Vec::new();
    for (i, &(bar, len)) in bars.iter().enumerate() {
        let in_build = opts.build_bars > 0 && i >= build_from;
        // フィル: fill_every 小節ごとの最後の小節の後半(ビルドの小節には入れない)
        let fill_here = opts.fill != Fill::None
            && opts.fill_every > 0
            && (i + 1) % opts.fill_every as usize == 0
            && !in_build;
        let body = if fill_here { 0..8 } else { 0..16 };
        // キック: 小節ごとに変化(最初の小節と区切りの次は基本の型)
        let kick = if i > 0 && style.kick.len() > 1 && rng.unit() < opts.variation {
            style.kick
                [1 + (rng.unit() * (style.kick.len() - 1) as f64) as usize % (style.kick.len() - 1)]
        } else {
            style.kick[0]
        };
        line(
            &mut out,
            kick,
            36,
            bar,
            len,
            if fill_here { 0..12 } else { 0..16 },
        );
        if in_build {
            continue;
        }
        for &(pitch, pat) in style.snare {
            line(&mut out, pat, pitch, bar, len, body.clone());
        }
        line(&mut out, style.hats[level], 42, bar, len, body.clone());
        if level >= 1 {
            if let Some(oh) = style.open_hat {
                line(&mut out, oh, 46, bar, len, body.clone());
            }
        }
        // 変化: 奇数小節の最後の裏にオープンハット(ハットがある型だけ)
        if opts.variation > 0.0 && i % 2 == 1 && !fill_here && rng.unit() < opts.variation * 0.5 {
            line(&mut out, "..............x.", 46, bar, len, 0..16);
        }
        if fill_here {
            let at = |k: u64| bar + k * len / 16;
            match opts.fill {
                Fill::Snare => {
                    for (j, k) in (8..16).enumerate() {
                        if k == 9 || k == 11 {
                            continue;
                        }
                        out.push(Hit {
                            pos: at(k),
                            dur: len / 32,
                            pitch: 38,
                            vel: (70 + j * 6).min(120) as u8,
                        });
                    }
                }
                Fill::Toms => {
                    for (j, (k, pitch)) in [
                        (8, 50),
                        (9, 50),
                        (10, 47),
                        (11, 47),
                        (12, 45),
                        (13, 45),
                        (14, 43),
                        (15, 43),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        out.push(Hit {
                            pos: at(k),
                            dur: len / 32,
                            pitch,
                            vel: (80 + j * 5).min(120) as u8,
                        });
                    }
                }
                Fill::None => {}
            }
        }
    }
    // ビルド: スネアのロール
    if opts.build_bars > 0 {
        out.extend(snare_roll(&bars[build_from..]));
    }
    // クラッシュ: 最初と、フィルの次の小節の頭
    if opts.crash {
        for (i, &(bar, len)) in bars.iter().enumerate() {
            let after_fill = i > 0
                && opts.fill != Fill::None
                && opts.fill_every > 0
                && i % opts.fill_every as usize == 0;
            // ビルドの小節には置かない(build_from はビルドが無ければ小節の数)
            if (i == 0 || after_fill) && i < build_from {
                out.push(Hit {
                    pos: bar,
                    dur: len / 2,
                    pitch: 49,
                    vel: 100,
                });
            }
        }
    }
    // ドロップ直前の無音
    if opts.gap_beats > 0 {
        if let Some(&(last, len)) = bars.last() {
            let end = last + len;
            let cut = end.saturating_sub(opts.gap_beats as u64 * len / 4);
            out.retain(|h| h.pos < cut);
            for h in &mut out {
                if h.pos + h.dur > cut {
                    h.dur = cut - h.pos;
                }
            }
        }
    }
    out.sort_by_key(|h| (h.pos, h.pitch));
    out.dedup_by(|a, b| a.pos == b.pos && a.pitch == b.pitch);
    out
}

/// スネアのロール(ビルド・区間のつなぎ)。小節ごとに細かく(最後の 4 小節が 4 分 → 8 分 → 16 分 → 32 分)、
/// だんだん強く。`bars` はロールにする小節(クリップの頭から)
pub fn snare_roll(bars: &[(u64, u64)]) -> Vec<Hit> {
    let n = bars.len();
    let divs = [4u64, 8, 16, 32];
    let mut out = Vec::new();
    for (i, &(bar, len)) in bars.iter().enumerate() {
        // 最後の小節が 32 分になるように、後ろからそろえる
        let div = divs[(divs.len() + i).saturating_sub(n).min(3)];
        for k in 0..div {
            let progress = i as f64 + k as f64 / div as f64;
            let vel = 50.0 + 70.0 * progress / n.max(1) as f64;
            out.push(Hit {
                pos: bar + k * len / div,
                dur: (len / div / 2).max(20),
                pitch: 38,
                vel: vel.round().clamp(1.0, 127.0) as u8,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAR: u64 = 3840;

    fn bars(n: u64) -> Vec<(u64, u64)> {
        (0..n).map(|b| (b * BAR, BAR)).collect()
    }

    fn opts() -> Options {
        Options {
            intensity: 0.6,
            variation: 0.0,
            fill: Fill::Snare,
            fill_every: 4,
            crash: true,
            build_bars: 0,
            gap_beats: 0,
            seed: 1,
        }
    }

    fn at(h: &[Hit], pitch: u8) -> Vec<u64> {
        h.iter()
            .filter(|x| x.pitch == pitch)
            .map(|x| x.pos)
            .collect()
    }

    #[test]
    fn every_style_parses_and_has_16_steps() {
        for s in STYLES {
            for p in s.kick.iter().chain(s.hats.iter()).chain(s.open_hat.iter()) {
                assert_eq!(p.chars().count(), 16, "{} {p}", s.name);
                assert!(p.chars().all(|c| "Xxgrt.".contains(c)), "{} {p}", s.name);
            }
            for (_, p) in s.snare {
                assert_eq!(p.chars().count(), 16, "{}", s.name);
            }
        }
        assert!(style("House").is_some());
        assert!(style("polka").is_none());
    }

    #[test]
    fn house_four_on_the_floor_with_fills_and_crashes() {
        let h = render(style("house").unwrap(), &bars(4), &opts());
        // 4 つ打ち(4 小節目の後半はフィルなのでキックは 3 拍目まで)
        let kicks = at(&h, 36);
        assert_eq!(kicks.len(), 4 + 4 + 4 + 3);
        // クラップは 2・4 拍(フィルの小節は 2 拍目だけ)
        assert_eq!(at(&h, 39).len(), 2 + 2 + 2 + 1);
        // 4 小節目の後半にスネアのフィル
        assert!(at(&h, 38).iter().all(|p| *p >= 3 * BAR + BAR / 2));
        assert!(at(&h, 38).len() >= 5);
        // クラッシュは最初だけ(4 小節で次が無い)
        assert_eq!(at(&h, 49), vec![0]);
        // 同じ入力なら同じ結果
        assert_eq!(h, render(style("house").unwrap(), &bars(4), &opts()));
    }

    #[test]
    fn intensity_variation_and_rolls() {
        // 音の多さで、ハットが 8 分 → 16 分
        let mut o = opts();
        o.fill = Fill::None;
        o.intensity = 0.5;
        let mid = at(&render(style("rock").unwrap(), &bars(1), &o), 42).len();
        o.intensity = 0.9;
        let high = at(&render(style("rock").unwrap(), &bars(1), &o), 42).len();
        assert!(high > mid, "{mid} {high}");
        // 変化: キックの型が小節で変わる
        o.variation = 1.0;
        let h = render(style("rock").unwrap(), &bars(8), &o);
        let per_bar: Vec<Vec<u64>> = (0..8)
            .map(|b| {
                at(&h, 36)
                    .into_iter()
                    .filter(|p| p / BAR == b)
                    .map(|p| p % BAR)
                    .collect()
            })
            .collect();
        assert!(
            per_bar.iter().skip(1).any(|k| *k != per_bar[0]),
            "{per_bar:?}"
        );
        // トラップのハットのロール(3 連の 32 分)
        o.intensity = 0.9;
        let t = render(style("trap").unwrap(), &bars(1), &o);
        let hats = at(&t, 42);
        assert!(hats.windows(2).any(|w| w[1] - w[0] < 100), "{hats:?}");
    }

    #[test]
    fn build_roll_and_gap_before_the_drop() {
        let mut o = opts();
        o.build_bars = 4;
        o.gap_beats = 1;
        o.fill = Fill::None;
        let h = render(style("house").unwrap(), &bars(4), &o);
        // ロール: 1 小節目は 4 分、4 小節目は 32 分(最後の 1 拍は無音)
        let snares = at(&h, 38);
        assert_eq!(snares.iter().filter(|p| **p < BAR).count(), 4);
        assert!(snares.iter().filter(|p| **p >= 3 * BAR).count() >= 24);
        // だんだん強く
        let v: Vec<u8> = h.iter().filter(|x| x.pitch == 38).map(|x| x.vel).collect();
        assert!(v.last().unwrap() > v.first().unwrap());
        // 最後の 1 拍は何も鳴らない
        assert!(h.iter().all(|x| x.pos < 4 * BAR - BAR / 4));
        // タムのフィル
        let mut o = opts();
        o.fill = Fill::Toms;
        let h = render(style("rock").unwrap(), &bars(4), &o);
        assert_eq!(at(&h, 50).len(), 2);
        assert_eq!(at(&h, 43).len(), 2);
    }
}
