//! 伴奏のリズム(MCP の write_chords の中身の後半)。積んだ和音(`voicing`)を、リズムの型に沿ってノートにする。
//!
//! 型は 1 小節を等分したステップの文字列: `x` = 打つ、`-` = 前の音を伸ばす、`.` = 休み。名前付きの型もある。
//! 和音が変わる所で音が伸びていたら、そこで新しい和音を弾き直す(レガートのコードチェンジ)。

/// 名前付きのリズム(4/4 の 16 ステップ)
pub const RHYTHMS: &[(&str, &str, &str)] = &[
    (
        "sustain",
        "",
        "和音ごとに 1 回、次の和音まで伸ばす(パッド・ストリングス)",
    ),
    ("whole", "x---------------", "小節ごとに弾き直す全音符"),
    ("half", "x-------x-------", "2 分音符"),
    ("quarter", "x--.x--.x--.x--.", "4 分の刻み(少し切る)"),
    (
        "eighth",
        "x.x.x.x.x.x.x.x.",
        "8 分の刻み(ロック・ポップのピアノ)",
    ),
    (
        "offbeat",
        "..x...x...x...x.",
        "8 分の裏(ハウスのスタブ・スカ)",
    ),
    (
        "charleston",
        "x-----x.........",
        "1 拍目と 2 拍目の裏(スウィング・ジャズのコンピング)",
    ),
    (
        "backbeat",
        "....x-......x-..",
        "2・4 拍(レゲエ・ファンクのギター)",
    ),
    (
        "syncopated",
        "x..x..x...x..x..",
        "3-3-2 のシンコペーション(ポップ・EDM のスタブ)",
    ),
];

/// リズムの型を読む(名前か、x - . の文字列)。None は sustain
pub fn parse_rhythm(s: &str) -> Result<Option<Vec<char>>, String> {
    let t = s.trim();
    if let Some((_, pat, _)) = RHYTHMS.iter().find(|(n, _, _)| n.eq_ignore_ascii_case(t)) {
        return Ok(if pat.is_empty() {
            None
        } else {
            Some(pat.chars().collect())
        });
    }
    let steps: Vec<char> = t
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '|')
        .collect();
    if steps.is_empty() || steps.len() > 64 {
        return Err(format!(
            "rhythm は名前({})か、x(打つ)- (伸ばす). (休み)の 1〜64 文字",
            RHYTHMS.iter().map(|r| r.0).collect::<Vec<_>>().join(" / ")
        ));
    }
    if let Some(c) = steps.iter().find(|c| !matches!(c, 'x' | 'X' | '-' | '.')) {
        return Err(format!("rhythm に使えない文字「{c}」(x - . だけ)"));
    }
    if steps[0] == '-' {
        return Err("rhythm は - で始められません(伸ばす前の音が無い)".to_owned());
    }
    Ok(Some(steps.iter().map(|c| c.to_ascii_lowercase()).collect()))
}

/// 和音 1 つが鳴る区間(クリップの頭からの tick)。`chord` は積んだ和音の番号、None は休み
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub start: u64,
    pub len: u64,
    pub chord: Option<usize>,
}

/// 伴奏のノート 1 つ(クリップの頭からの tick)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompNote {
    pub pos: u64,
    pub dur: u64,
    pub pitch: u8,
    pub vel: u8,
}

/// ノートの作り方
#[derive(Clone, Debug)]
pub struct CompOptions {
    /// 伸ばさない音の長さの割合(ステップに対して)
    pub gate: f64,
    pub velocity: u8,
    /// 下の音から順に少しずつ遅らせる(tick。ギターのストローク・ハープ)
    pub strum_ticks: u64,
}

/// 積んだ和音(`notes[i]` = i 番目の和音の音。低い順)を、区間とリズムに沿ってノートにする。
/// `bars` はクリップの頭からの (小節の頭, 小節の長さ)、`meters` は小節ごとの拍子(同じ順。4/4 以外の小節は
/// 型を 16 分の格子と拍のまとまりに当てる。[`crate::meter::fit_pattern`])
pub fn render(
    spans: &[Span],
    notes: &[Vec<u8>],
    rhythm: Option<&[char]>,
    bars: &[(u64, u64)],
    meters: &[crate::meter::BarMeter],
    opts: &CompOptions,
) -> Vec<CompNote> {
    // 打つ位置と長さ(クリップの頭から)
    let mut hits: Vec<(u64, u64, u8)> = Vec::new(); // (pos, len, 強さの加減)
    match rhythm {
        None => {
            for s in spans {
                hits.push((s.start, s.len, 0));
            }
        }
        Some(pat0) => {
            for (bi, &(bar, len)) in bars.iter().enumerate() {
                let meter = meters.get(bi);
                let fitted = crate::meter::fit_pattern(pat0, meter);
                let pat: &[char] = fitted.as_deref().unwrap_or(pat0);
                let n = pat.len() as u64;
                let step = if fitted.is_some() {
                    crate::meter::STEP as f64
                } else {
                    len as f64 / n as f64
                };
                let at = |k: u64| bar + ((k as f64 * step).round() as u64).min(len);
                let mut k = 0u64;
                while k < n {
                    if pat[k as usize] != 'x' {
                        k += 1;
                        continue;
                    }
                    let mut end = k + 1;
                    while end < n && pat[end as usize] == '-' {
                        end += 1;
                    }
                    let held = end > k + 1;
                    let full = at(end) - at(k);
                    let len = if held {
                        full
                    } else {
                        ((full as f64) * opts.gate).round().max(1.0) as u64
                    };
                    // 拍の頭(変拍子はまとまりの頭)は少し強く、裏は少し弱く
                    let beat = n / 4;
                    let accent = if k == 0 {
                        8
                    } else if fitted.is_some() {
                        match meter.map(|m| m.level(k * crate::meter::STEP)) {
                            Some(1) => 3,
                            _ => 0,
                        }
                    } else if beat > 0 && k % beat == 0 {
                        3
                    } else {
                        0
                    };
                    hits.push((at(k), len, accent));
                    k = end;
                }
            }
        }
    }
    let span_at = |t: u64| spans.iter().find(|s| s.start <= t && t < s.start + s.len);
    let mut out = Vec::new();
    let mut emit = |pos: u64, end: u64, chord: usize, accent: u8| {
        for (v, &p) in notes[chord].iter().enumerate() {
            let delay = opts.strum_ticks * v as u64;
            let start = pos + delay;
            if start >= end {
                continue;
            }
            out.push(CompNote {
                pos: start,
                dur: end - start,
                pitch: p,
                vel: (opts.velocity as i32 + accent as i32).clamp(1, 127) as u8,
            });
        }
    };
    for &(pos, len, accent) in &hits {
        let Some(s) = span_at(pos) else { continue };
        let end = pos + len;
        // 伸ばしている途中で和音が変わる所では切って、新しい和音を弾き直す
        let mut t = pos;
        let mut cur = *s;
        loop {
            let seg_end = end.min(cur.start + cur.len);
            if let Some(c) = cur.chord {
                emit(t, seg_end, c, if t == pos { accent } else { 0 });
            }
            if seg_end >= end {
                break;
            }
            t = seg_end;
            // 次の打つ位置がちょうどここなら、そちらに任せる
            if hits.iter().any(|h| h.0 == t) {
                break;
            }
            match span_at(t) {
                Some(n) => cur = *n,
                None => break,
            }
        }
    }
    out.sort_by_key(|n| (n.pos, n.pitch));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAR: u64 = 3840;

    fn opts() -> CompOptions {
        CompOptions {
            gate: 0.9,
            velocity: 90,
            strum_ticks: 0,
        }
    }

    #[test]
    fn rhythms_parse() {
        assert_eq!(parse_rhythm("sustain").unwrap(), None);
        assert_eq!(parse_rhythm("Offbeat").unwrap().unwrap().len(), 16);
        assert_eq!(
            parse_rhythm("x-.x").unwrap().unwrap(),
            vec!['x', '-', '.', 'x']
        );
        assert!(parse_rhythm("x-y").is_err());
        assert!(parse_rhythm("-x").is_err());
        assert!(parse_rhythm("").is_err());
    }

    #[test]
    fn sustain_plays_each_chord_once() {
        let spans = [
            Span {
                start: 0,
                len: BAR * 2,
                chord: Some(0),
            },
            Span {
                start: BAR * 2,
                len: BAR,
                chord: None,
            },
            Span {
                start: BAR * 3,
                len: BAR,
                chord: Some(1),
            },
        ];
        let notes = vec![vec![60, 64, 67], vec![62, 65, 69]];
        let bars: Vec<(u64, u64)> = (0..4).map(|b| (b * BAR, BAR)).collect();
        let out = render(&spans, &notes, None, &bars, &[], &opts());
        assert_eq!(out.len(), 6);
        assert!(out.iter().take(3).all(|n| n.pos == 0 && n.dur == BAR * 2));
        assert!(out.iter().skip(3).all(|n| n.pos == BAR * 3));
    }

    #[test]
    fn patterns_split_at_chord_changes_and_strum() {
        // 1 小節に 2 和音、whole(全音符)→ 和音の変わる所で弾き直す
        let spans = [
            Span {
                start: 0,
                len: BAR / 2,
                chord: Some(0),
            },
            Span {
                start: BAR / 2,
                len: BAR / 2,
                chord: Some(1),
            },
        ];
        let notes = vec![vec![60, 64, 67], vec![59, 62, 67]];
        let bars = vec![(0, BAR)];
        let pat = parse_rhythm("whole").unwrap().unwrap();
        let out = render(&spans, &notes, Some(&pat), &bars, &[], &opts());
        assert_eq!(out.len(), 6);
        assert!(out.iter().any(|n| n.pos == BAR / 2 && n.pitch == 59));
        assert!(out.iter().all(|n| n.dur == BAR / 2));
        // 裏拍の刻み: 4 回、短く、弱い拍
        let pat = parse_rhythm("offbeat").unwrap().unwrap();
        let spans = [Span {
            start: 0,
            len: BAR,
            chord: Some(0),
        }];
        let out = render(&spans, &notes, Some(&pat), &bars, &[], &opts());
        let starts: Vec<u64> = out
            .iter()
            .filter(|n| n.pitch == 60)
            .map(|n| n.pos)
            .collect();
        assert_eq!(starts, vec![480, 1440, 2400, 3360]);
        assert!(out.iter().all(|n| n.dur == 216 && n.vel == 90));
        // ストローク: 下から順に遅れる
        let mut o = opts();
        o.strum_ticks = 20;
        let out = render(
            &spans,
            &notes,
            Some(&parse_rhythm("whole").unwrap().unwrap()),
            &bars,
            &[],
            &o,
        );
        let first: Vec<u64> = out.iter().take(3).map(|n| n.pos).collect();
        assert_eq!(first, vec![0, 20, 40]);
        // 1 拍目は強い
        let out = render(
            &spans,
            &notes,
            Some(&parse_rhythm("quarter").unwrap().unwrap()),
            &bars,
            &[],
            &opts(),
        );
        assert!(
            out.iter().find(|n| n.pos == 0).unwrap().vel
                > out.iter().find(|n| n.pos == 960).unwrap().vel
        );
    }

    #[test]
    fn odd_bars_divide_the_pattern_evenly() {
        // 3/4 の小節(2880 tick)に 16 ステップ = 180 tick ずつ
        let spans = [Span {
            start: 0,
            len: 2880,
            chord: Some(0),
        }];
        let notes = vec![vec![60]];
        let pat = parse_rhythm("eighth").unwrap().unwrap();
        let out = render(&spans, &notes, Some(&pat), &[(0, 2880)], &[], &opts());
        assert_eq!(out.len(), 8);
        assert_eq!(out[1].pos, 360);
    }
}
