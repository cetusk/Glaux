//! ベースライン(MCP の write_bassline の中身)。コード進行の区間(`comp::Span`)に沿って、型のとおりにベースの音を置く。
//!
//! 根音(分数コードなら最低音)のオクターブは、進行全体で動きが小さい並びを選ぶ(`voicing::bass_line`)。
//! 型は 1 小節を等分したステップの文字列で、音の選び方も書ける: `x` / `r` = 根音、`o` = 1 オクターブ上、`5` = 5 度、
//! `3` = 3 度、`7` = 7 度(無ければ 1 オクターブ上)、`-` = 伸ばす、`.` = 休み。名前付きの型もある。

use crate::chord::{Chord, Role};
use crate::comp::Span;

/// 名前付きの型(4/4 の 16 ステップ)
pub const PATTERNS: &[(&str, &str, &str)] = &[
    ("root", "", "和音ごとに根音を伸ばす(パッド的・バラード)"),
    (
        "root8",
        "x.x.x.x.x.x.x.x.",
        "根音の 8 分(ハウス・ロック・ポップ)",
    ),
    (
        "offbeat",
        "..x...x...x...x.",
        "8 分の裏(トランス・ハウスのうねり)",
    ),
    (
        "octave",
        "x.o.x.o.x.o.x.o.",
        "オクターブの跳躍(ディスコ・ファンク)",
    ),
    (
        "tresillo",
        "x-----x-----x---",
        "3-3-2(ラテン・レゲトン・トラップ)",
    ),
    (
        "808",
        "x-----x-----x---",
        "トラップの 808: 3-3-2 で伸ばし、音が変わる所は滑らせる",
    ),
    (
        "funk",
        "x..x..o.x.x..5.7",
        "16 分のシンコペーション(ファンク)",
    ),
    (
        "walking",
        "",
        "4 分で歩く(ジャズ・ローファイ): 根音 → 和音の音 → 次の根音へ半音で近づく",
    ),
];

/// 型
#[derive(Clone, Debug, PartialEq)]
pub enum Pattern {
    /// 和音ごとに根音を 1 つ
    Sustain,
    /// 4 分で歩く
    Walking,
    Steps(Vec<char>),
}

/// 型を読む。返り値の bool は 808(伸ばして滑らせる)
pub fn parse_pattern(s: &str) -> Result<(Pattern, bool), String> {
    let t = s.trim();
    if let Some((name, pat, _)) = PATTERNS.iter().find(|(n, _, _)| n.eq_ignore_ascii_case(t)) {
        return Ok(match *name {
            "root" => (Pattern::Sustain, false),
            "walking" => (Pattern::Walking, false),
            "808" => (Pattern::Steps(pat.chars().collect()), true),
            _ => (Pattern::Steps(pat.chars().collect()), false),
        });
    }
    let steps: Vec<char> = t
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '|')
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if steps.is_empty() || steps.len() > 64 {
        return Err(format!(
            "pattern は名前({})か、x o 5 3 7 - . の 1〜64 文字",
            PATTERNS.iter().map(|p| p.0).collect::<Vec<_>>().join(" / ")
        ));
    }
    if let Some(c) = steps
        .iter()
        .find(|c| !matches!(c, 'x' | 'r' | 'o' | '5' | '3' | '7' | '-' | '.'))
    {
        return Err(format!("pattern に使えない文字「{c}」(x o 5 3 7 - . だけ)"));
    }
    if steps[0] == '-' {
        return Err("pattern は - で始められません".to_owned());
    }
    Ok((Pattern::Steps(steps), false))
}

/// 次の和音の根音へ近づく経過音
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Approach {
    None,
    /// 半音上か下から
    Chromatic,
    /// 音階の隣の音から(`scale` を渡したとき)
    Scale,
}

/// 置き方
#[derive(Clone, Debug)]
pub struct Options {
    /// 音域(MIDI 番号、両端を含む)
    pub low: u8,
    pub high: u8,
    /// 伸ばさない音の長さの割合
    pub gate: f64,
    pub velocity: u8,
    pub approach: Approach,
    /// キーの音階(ピッチクラス)。Scale の経過音とウォーキングの経過に使う
    pub scale: Option<Vec<u8>>,
    /// 808: 音を次の音まで伸ばし、音が変わる所は滑らせる
    pub slide: bool,
    /// 1 拍の長さ(tick)
    pub beat: u64,
}

/// ベースのノート 1 つ(クリップの頭から)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BassNote {
    pub pos: u64,
    pub dur: u64,
    pub pitch: u8,
    pub vel: u8,
    /// 直前の音から滑らせる
    pub slide: bool,
}

/// `root` を基準に、和音の `pc` の音を 1 オクターブ内で上に取る
fn above(root: u8, pc: u8) -> u8 {
    root + ((pc as i32 - root as i32).rem_euclid(12)) as u8
}

/// ステップの文字の音
fn degree(c: char, chord: &Chord, root: u8) -> u8 {
    let tone = |roles: &[Role]| {
        chord
            .tones
            .iter()
            .find(|t| roles.contains(&t.role))
            .map(|t| above(root, (chord.root + t.interval) % 12))
    };
    match c {
        'o' => root + 12,
        '5' => tone(&[Role::Fifth]).unwrap_or(root + 7),
        '3' => tone(&[Role::Third, Role::Sus]).unwrap_or(root),
        '7' => tone(&[Role::Seventh, Role::Sixth]).unwrap_or(root + 12),
        _ => root,
    }
}

/// `to` へ近づく経過音(`from` に近い側から)
fn approach_note(from: u8, to: u8, how: Approach, scale: Option<&[u8]>) -> u8 {
    let up = from > to;
    match (how, scale) {
        (Approach::Scale, Some(sc)) => {
            // 音階の隣の音(上から近づくなら上の隣、下からなら下の隣)
            let mut p = to as i32;
            loop {
                p += if up { 1 } else { -1 };
                if sc.contains(&(p.rem_euclid(12) as u8)) || (p - to as i32).abs() >= 2 {
                    break;
                }
            }
            p.clamp(0, 127) as u8
        }
        _ => {
            if up {
                to + 1
            } else {
                to.saturating_sub(1)
            }
        }
    }
}

/// 区間と型に沿ってベースのノートを作る。`kick` はキックの位置(クリップの頭から。渡すとその位置で鳴らす)
pub fn render(
    spans: &[Span],
    chords: &[Chord],
    bars: &[(u64, u64)],
    pattern: &Pattern,
    opts: &Options,
    kick: Option<&[u64]>,
) -> Result<Vec<BassNote>, String> {
    let uses_octave = matches!(pattern, Pattern::Steps(s) if s.contains(&'o') || s.contains(&'7'));
    let root_high = if uses_octave {
        opts.high.saturating_sub(12)
    } else {
        opts.high
    };
    if root_high < opts.low + 11 {
        return Err("音域が狭すぎます(オクターブの跳躍には 2 オクターブ要る)".to_owned());
    }
    let roots = crate::voicing::bass_line(chords, opts.low, root_high);
    let span_at = |t: u64| {
        spans
            .iter()
            .position(|s| s.start <= t && t < s.start + s.len)
    };
    // 区間の次の根音(経過音用)
    let next_root = |si: usize| -> Option<u8> {
        spans[si + 1..]
            .iter()
            .find_map(|s| s.chord)
            .map(|c| roots[c])
    };
    let mut out: Vec<BassNote> = Vec::new();
    let vel = |accent: i32| (opts.velocity as i32 + accent).clamp(1, 127) as u8;
    match (pattern, kick) {
        (_, Some(kicks)) => {
            // キックに合わせる: キックの位置で根音を鳴らし、次のキックまで(区間の変わり目では弾き直す)
            let mut starts: Vec<u64> = kicks.to_vec();
            starts.extend(spans.iter().filter(|s| s.chord.is_some()).map(|s| s.start));
            starts.sort_unstable();
            starts.dedup_by(|a, b| a.abs_diff(*b) < 30);
            for (i, &t) in starts.iter().enumerate() {
                let Some(si) = span_at(t) else { continue };
                let Some(ci) = spans[si].chord else { continue };
                let span_end = spans[si].start + spans[si].len;
                let next = starts.get(i + 1).copied().unwrap_or(span_end).min(span_end);
                let len = ((next - t) as f64 * opts.gate).round().max(1.0) as u64;
                let on_beat = t % opts.beat == 0;
                out.push(BassNote {
                    pos: t,
                    dur: len,
                    pitch: roots[ci],
                    vel: vel(if on_beat { 4 } else { 0 }),
                    slide: false,
                });
            }
        }
        (Pattern::Sustain, None) => {
            for s in spans {
                if let Some(ci) = s.chord {
                    out.push(BassNote {
                        pos: s.start,
                        dur: s.len,
                        pitch: roots[ci],
                        vel: vel(0),
                        slide: false,
                    });
                }
            }
        }
        (Pattern::Walking, None) => {
            for (si, s) in spans.iter().enumerate() {
                let Some(ci) = s.chord else { continue };
                let root = roots[ci];
                let beats = (s.len / opts.beat).max(1);
                let chord = &chords[ci];
                // 和音の音(根音の上の 3 度・5 度・オクターブ)を順に上がり、音域を越えるなら下がる
                let tones: Vec<u8> = ['3', '5', 'o']
                    .iter()
                    .map(|c| degree(*c, chord, root))
                    .collect();
                let mut prev = root;
                for b in 0..beats {
                    let pos = s.start + b * opts.beat;
                    let pitch = if b == 0 {
                        root
                    } else if b == beats - 1 && beats >= 2 {
                        match next_root(si) {
                            Some(nr) => approach_note(
                                prev,
                                nr,
                                if opts.approach == Approach::None {
                                    Approach::Chromatic
                                } else {
                                    opts.approach
                                },
                                opts.scale.as_deref(),
                            ),
                            None => tones[0],
                        }
                    } else {
                        let t = tones[((b - 1) as usize).min(tones.len() - 1)];
                        if t > opts.high {
                            t - 12
                        } else {
                            t
                        }
                    };
                    let pitch = pitch.clamp(opts.low.saturating_sub(2), opts.high);
                    out.push(BassNote {
                        pos,
                        dur: ((opts.beat as f64) * opts.gate).round() as u64,
                        pitch,
                        vel: vel(if b == 0 { 6 } else { 0 }),
                        slide: false,
                    });
                    prev = pitch;
                }
            }
        }
        (Pattern::Steps(pat), None) => {
            let n = pat.len() as u64;
            for &(bar, len) in bars {
                let step = len as f64 / n as f64;
                let at = |k: u64| bar + (k as f64 * step).round() as u64;
                let mut k = 0u64;
                while k < n {
                    let c = pat[k as usize];
                    if matches!(c, '-' | '.') {
                        k += 1;
                        continue;
                    }
                    let mut end = k + 1;
                    while end < n && pat[end as usize] == '-' {
                        end += 1;
                    }
                    let pos = at(k);
                    let held = end > k + 1 || opts.slide;
                    let full = at(end) - pos;
                    let Some(si) = span_at(pos) else {
                        k = end;
                        continue;
                    };
                    let Some(ci) = spans[si].chord else {
                        k = end;
                        continue;
                    };
                    let span_end = spans[si].start + spans[si].len;
                    let mut dur = if held {
                        full
                    } else {
                        ((full as f64) * opts.gate).round().max(1.0) as u64
                    };
                    dur = dur.min(span_end - pos);
                    let beat = (n / 4).max(1);
                    let accent = if k == 0 {
                        8
                    } else if k % beat == 0 {
                        3
                    } else if c == 'o' {
                        -4
                    } else {
                        0
                    };
                    let mut pitch = degree(c, &chords[ci], roots[ci]);
                    if pitch > opts.high {
                        pitch -= 12;
                    }
                    // 経過音: 区間の最後の 8 分に打つ音を、次の根音へ近づける音に
                    if opts.approach != Approach::None && pos + opts.beat / 2 >= span_end {
                        if let Some(nr) = next_root(si).filter(|nr| *nr != roots[ci]) {
                            pitch = approach_note(pitch, nr, opts.approach, opts.scale.as_deref());
                        }
                    }
                    out.push(BassNote {
                        pos,
                        dur,
                        pitch,
                        vel: vel(accent),
                        slide: false,
                    });
                    k = end;
                }
            }
            // 伸ばしの途中で和音が変わる所に打つ音が無ければ、新しい根音を弾き直す
            for s in spans {
                let Some(ci) = s.chord else { continue };
                if out.iter().any(|n| n.pos == s.start) {
                    continue;
                }
                if let Some(prev) = out
                    .iter_mut()
                    .filter(|n| n.pos < s.start && n.pos + n.dur > s.start)
                    .max_by_key(|n| n.pos)
                {
                    prev.dur = s.start - prev.pos;
                    out.push(BassNote {
                        pos: s.start,
                        dur: s.len.min(opts.beat * 4),
                        pitch: roots[ci],
                        vel: vel(3),
                        slide: false,
                    });
                }
            }
            out.sort_by_key(|n| n.pos);
            // 808: 次の音まで伸ばし、音が変わる所は滑らせる
            if opts.slide {
                for i in 1..out.len() {
                    let (a, b) = (out[i - 1], out[i]);
                    if a.pos + a.dur >= b.pos && a.pitch != b.pitch {
                        out[i].slide = true;
                    }
                }
            }
        }
    }
    out.sort_by_key(|n| (n.pos, n.pitch));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chord::parse;

    const BAR: u64 = 3840;

    fn opts() -> Options {
        Options {
            low: 28,
            high: 52,
            gate: 0.85,
            velocity: 96,
            approach: Approach::None,
            scale: None,
            slide: false,
            beat: 960,
        }
    }

    fn setup(names: &[&str]) -> (Vec<Span>, Vec<Chord>, Vec<(u64, u64)>) {
        let chords: Vec<Chord> = names.iter().map(|n| parse(n).unwrap().unwrap()).collect();
        let spans = (0..names.len())
            .map(|i| Span {
                start: i as u64 * BAR,
                len: BAR,
                chord: Some(i),
            })
            .collect();
        let bars = (0..names.len() as u64).map(|b| (b * BAR, BAR)).collect();
        (spans, chords, bars)
    }

    #[test]
    fn patterns_parse() {
        assert_eq!(parse_pattern("root").unwrap().0, Pattern::Sustain);
        assert_eq!(parse_pattern("walking").unwrap().0, Pattern::Walking);
        assert!(parse_pattern("808").unwrap().1);
        assert!(matches!(parse_pattern("x.o5-").unwrap().0, Pattern::Steps(s) if s.len() == 5));
        assert!(parse_pattern("x.q").is_err());
        assert!(parse_pattern("-x").is_err());
    }

    #[test]
    fn root_eighths_follow_the_chords_in_a_smooth_register() {
        let (spans, chords, bars) = setup(&["Am", "F", "C", "G"]);
        let (pat, _) = parse_pattern("root8").unwrap();
        let out = render(&spans, &chords, &bars, &pat, &opts(), None).unwrap();
        assert_eq!(out.len(), 32);
        let roots: Vec<u8> = (0..4)
            .map(|b| out.iter().find(|n| n.pos == b * BAR).unwrap().pitch)
            .collect();
        assert_eq!(
            roots.iter().map(|p| p % 12).collect::<Vec<_>>(),
            vec![9, 5, 0, 7]
        );
        // 根音どうしは大きく跳ばない
        assert!(
            roots
                .windows(2)
                .all(|w| (w[0] as i32 - w[1] as i32).abs() <= 7),
            "{roots:?}"
        );
        assert!(out.iter().all(|n| (28..=52).contains(&n.pitch)));
        // 拍の頭が強い
        assert!(out[0].vel > out[1].vel);
    }

    #[test]
    fn octaves_fifths_and_slash_chords() {
        let (spans, chords, bars) = setup(&["C/E", "G7"]);
        let (pat, _) = parse_pattern("x.o.5.7.").unwrap();
        let out = render(&spans, &chords, &bars, &pat, &opts(), None).unwrap();
        // C/E: 最低音は E、1 オクターブ上も E、5 度は G(C の 5 度)
        let bar0: Vec<u8> = out
            .iter()
            .filter(|n| n.pos < BAR)
            .map(|n| n.pitch)
            .collect();
        assert_eq!(bar0[0] % 12, 4);
        assert_eq!(bar0[1], bar0[0] + 12);
        assert_eq!(bar0[2] % 12, 7);
        // G7 の 7 度は F
        let bar1: Vec<u8> = out
            .iter()
            .filter(|n| n.pos >= BAR)
            .map(|n| n.pitch)
            .collect();
        assert_eq!(bar1[3] % 12, 5);
    }

    #[test]
    fn walking_approaches_the_next_root_chromatically() {
        let (spans, chords, bars) = setup(&["Dm7", "G7", "Cmaj7"]);
        let out = render(&spans, &chords, &bars, &Pattern::Walking, &opts(), None).unwrap();
        assert_eq!(out.len(), 12);
        // 1 小節目の 4 拍目は、次の根音 G の半音隣
        let g = out.iter().find(|n| n.pos == BAR).unwrap().pitch;
        let fourth = out.iter().find(|n| n.pos == 3 * 960).unwrap().pitch;
        assert_eq!((fourth as i32 - g as i32).abs(), 1);
        assert!(out.iter().all(|n| n.pos % 960 == 0));
    }

    #[test]
    fn approach_notes_slides_and_kick_following() {
        // 経過音: 区間の最後の 8 分が次の根音の隣に
        let (spans, chords, bars) = setup(&["C", "F"]);
        let mut o = opts();
        o.approach = Approach::Chromatic;
        let (pat, _) = parse_pattern("root8").unwrap();
        let out = render(&spans, &chords, &bars, &pat, &o, None).unwrap();
        let last = out.iter().rfind(|n| n.pos < BAR).unwrap();
        let f = out.iter().find(|n| n.pos == BAR).unwrap().pitch;
        assert_eq!((last.pitch as i32 - f as i32).abs(), 1, "{out:?}");
        // 808: 伸ばして、音が変わる所で滑る
        let (spans, chords, bars) = setup(&["Am", "F"]);
        let (pat, slide) = parse_pattern("808").unwrap();
        let mut o = opts();
        o.slide = slide;
        let out = render(&spans, &chords, &bars, &pat, &o, None).unwrap();
        assert!(out.iter().any(|n| n.slide && n.pitch % 12 == 5), "{out:?}");
        assert!(out.windows(2).all(|w| w[0].pos + w[0].dur >= w[1].pos));
        // キックに合わせる: キックの位置で根音
        let (spans, chords, bars) = setup(&["Am"]);
        let kicks = [0, 960, 1680, 2880];
        let out = render(
            &spans,
            &chords,
            &bars,
            &Pattern::Sustain,
            &opts(),
            Some(&kicks),
        )
        .unwrap();
        assert_eq!(
            out.iter().map(|n| n.pos).collect::<Vec<_>>(),
            kicks.to_vec()
        );
        assert!(out.iter().all(|n| n.pitch % 12 == 9));
        assert!(out[1].dur < 720);
    }
}
