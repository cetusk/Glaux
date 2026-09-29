//! アルペジオ(`write_arpeggio`)。
//!
//! 積んだ和音の音を、決まった刻み(rate)で 1 つずつ鳴らす。並べ方は Ableton Live のアルペジエーターの
//! 18 の型に合わせる。強弱の列(accents)とリズムの列(rhythm)は音の並びと別の長さで回るので、
//! 長さを変えるとポリメーター(3 音の並びを 4 つの強弱で回す、など)になる。
//! 1 回りするたびに音程をずらす(shift × shift_steps)こともできる(Live の Transpose の段)。

use crate::comp::{CompNote, Span};

/// 並べ方
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// 下から上へ
    Up,
    /// 上から下へ
    Down,
    /// 上って下る(両端を繰り返さない: 1 2 3 4 3 2)
    UpDown,
    /// 下って上る(4 3 2 1 2 3)
    DownUp,
    /// 上って下る(両端を繰り返す: 1 2 3 4 4 3 2 1)
    UpAndDown,
    /// 下って上る(4 3 2 1 1 2 3 4)
    DownAndUp,
    /// 外から内へ(1 4 2 3)
    Converge,
    /// 内から外へ(3 2 4 1)
    Diverge,
    /// 外から内へ、また外へ(1 4 2 3 2 4)
    ConDiverge,
    /// 一番上の音を挟みながら上る(1 4 2 4 3 4)
    PinkyUp,
    /// 一番上の音を挟みながら上って下る(1 4 2 4 3 4 2 4)
    PinkyUpDown,
    /// 一番下の音を挟みながら上る(1 2 1 3 1 4)
    ThumbUp,
    /// 一番下の音を挟みながら上って下る(1 2 1 3 1 4 1 3)
    ThumbUpDown,
    /// 和音の音の役割の順(根音・3 度・5 度・7 度)
    PlayOrder,
    /// 和音をまとめて刻む
    Chord,
    /// 毎回でたらめ
    Random,
    /// でたらめだが、全部の音を 1 回ずつ使い切ってから次の並び
    RandomOther,
    /// 1 回決めたでたらめの並びを繰り返す
    RandomOnce,
}

pub const STYLE_NAMES: &[&str] = &[
    "up",
    "down",
    "up_down",
    "down_up",
    "up_and_down",
    "down_and_up",
    "converge",
    "diverge",
    "con_diverge",
    "pinky_up",
    "pinky_up_down",
    "thumb_up",
    "thumb_up_down",
    "play_order",
    "chord",
    "random",
    "random_other",
    "random_once",
];

impl Style {
    pub fn parse(s: &str) -> Option<Style> {
        use Style::*;
        // "Up & Down" / "up-down" / "UpDown" などの書き方をそろえる
        let key = s
            .to_ascii_lowercase()
            .replace('&', " and ")
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
            .join("_");
        Some(match key.as_str() {
            "up" => Up,
            "down" => Down,
            "up_down" | "updown" => UpDown,
            "down_up" | "downup" => DownUp,
            "up_and_down" => UpAndDown,
            "down_and_up" => DownAndUp,
            "converge" => Converge,
            "diverge" => Diverge,
            "con_diverge" | "converge_diverge" | "con_and_diverge" => ConDiverge,
            "pinky_up" => PinkyUp,
            "pinky_up_down" | "pinky_updown" => PinkyUpDown,
            "thumb_up" => ThumbUp,
            "thumb_up_down" | "thumb_updown" => ThumbUpDown,
            "play_order" | "order" => PlayOrder,
            "chord" | "chord_trigger" => Chord,
            "random" => Random,
            "random_other" => RandomOther,
            "random_once" => RandomOnce,
            _ => return None,
        })
    }
}

/// 並びの 1 回り分(音の番号の列。`n` 音、低い順に 0..n)。Chord は空(まとめて鳴らす)。
/// でたらめの型は `seq_random` で作る
pub fn pattern(style: Style, n: usize) -> Vec<usize> {
    use Style::*;
    if n == 0 {
        return Vec::new();
    }
    let up: Vec<usize> = (0..n).collect();
    let down: Vec<usize> = (0..n).rev().collect();
    let inner = |v: &[usize]| -> Vec<usize> {
        // 両端を除いた逆向き(上り下りのつなぎ)
        if v.len() <= 2 {
            Vec::new()
        } else {
            v[1..v.len() - 1].iter().rev().copied().collect()
        }
    };
    let converge = || {
        let (mut lo, mut hi) = (0usize, n - 1);
        let mut v = Vec::with_capacity(n);
        while lo <= hi {
            v.push(lo);
            if hi != lo {
                v.push(hi);
            }
            lo += 1;
            if hi == 0 {
                break;
            }
            hi -= 1;
        }
        v
    };
    match style {
        Up | PlayOrder | Random | RandomOther | RandomOnce => up,
        Down => down,
        UpDown => [up.clone(), inner(&up)].concat(),
        DownUp => [down.clone(), inner(&down)].concat(),
        UpAndDown => [up.clone(), down].concat(),
        DownAndUp => [down.clone(), up].concat(),
        Converge => converge(),
        Diverge => converge().into_iter().rev().collect(),
        ConDiverge => {
            let c = converge();
            let back = inner(&c);
            [c, back].concat()
        }
        PinkyUp | PinkyUpDown => {
            if n == 1 {
                return vec![0];
            }
            let top = n - 1;
            let mut rise: Vec<usize> = (0..top).collect();
            if style == PinkyUpDown {
                let mut fall: Vec<usize> = (1..top.saturating_sub(1).max(1)).rev().collect();
                if top < 3 {
                    fall.clear();
                }
                rise.extend(fall);
            }
            rise.into_iter().flat_map(|i| [i, top]).collect()
        }
        ThumbUp | ThumbUpDown => {
            if n == 1 {
                return vec![0];
            }
            let mut rise: Vec<usize> = (1..n).collect();
            if style == ThumbUpDown && n > 3 {
                rise.extend((2..n - 1).rev());
            }
            rise.into_iter().flat_map(|i| [0, i]).collect()
        }
        Chord => Vec::new(),
    }
}

/// 決まった種からの乱数(xorshift)
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn shuffle(&mut self, v: &mut [usize]) {
        for i in (1..v.len()).rev() {
            let j = self.below(i + 1);
            v.swap(i, j);
        }
    }
}

/// リズムの列の 1 ステップ
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// 鳴らす
    Hit,
    /// 休む
    Rest,
    /// 前の音を伸ばす
    Tie,
}

/// "x x . x -" のような列を読む(x = 鳴らす、. = 休む、- = 前の音を伸ばす。空白は無視)
pub fn parse_rhythm(s: &str) -> Result<Vec<Step>, String> {
    let v: Vec<Step> = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '|')
        .map(|c| match c {
            'x' | 'X' | '1' => Ok(Step::Hit),
            '.' | '0' | '_' => Ok(Step::Rest),
            '-' => Ok(Step::Tie),
            c => Err(format!(
                "rhythm に使えない文字です: {c}(x = 鳴らす、. = 休む、- = 伸ばす)"
            )),
        })
        .collect::<Result<_, _>>()?;
    if v.is_empty() || !v.contains(&Step::Hit) {
        return Err("rhythm には x を 1 つ以上入れてください".to_owned());
    }
    Ok(v)
}

/// アルペジオの作り方
#[derive(Clone, Debug)]
pub struct Options {
    pub style: Style,
    /// 1 ステップの長さ(tick)
    pub step: u64,
    /// 音の長さ(ステップに対する割合。1 を超えると次の音に重なる)
    pub gate: f64,
    /// 何オクターブに広げるか(1 = 積んだ和音のまま)
    pub octaves: u8,
    /// 強弱の列(ステップごとに回る)。空なら `velocity` と拍の頭の強調
    pub accents: Vec<u8>,
    pub velocity: u8,
    /// リズムの列(ステップごとに回る)。空ならすべて鳴らす
    pub rhythm: Vec<Step>,
    /// 和音が変わるたびに並びを頭からやり直す
    pub retrigger: bool,
    /// 1 回りするごとにずらす半音と、その段数(0 = ずらさない)
    pub shift: i32,
    pub shift_steps: u8,
    pub seed: u64,
}

/// 積んだ和音(`chords[i]` = i 番目の和音の音。`order[i]` = 役割の順に並べた同じ音)を、区間に沿ってアルペジオにする。
/// ステップはクリップの頭からの格子に置く(区間の途中から始まる和音も格子に合う)。音は区間の終わりで切る
/// (gate が 1 を超えて重ねるときも、次の和音には持ち越さない)
pub fn render(spans: &[Span], chords: &[Vec<u8>], order: &[Vec<u8>], o: &Options) -> Vec<CompNote> {
    let step = o.step.max(1);
    let mut out: Vec<CompNote> = Vec::new();
    let mut rng = Rng(o.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    // 通しのステップの番号(retrigger しないときの並びの位置、強弱・リズムの列の位置)
    let mut pat_pos: usize = 0;
    let mut once_perm: Option<Vec<usize>> = None;
    let mut other_bag: Vec<usize> = Vec::new();
    for sp in spans {
        let Some(ci) = sp.chord else { continue };
        let base = match (o.style, order.get(ci)) {
            (Style::PlayOrder, Some(v)) if !v.is_empty() => v.clone(),
            _ => chords.get(ci).cloned().unwrap_or_default(),
        };
        if base.is_empty() {
            continue;
        }
        // オクターブに広げる(並べ方の順は保つ)
        let mut notes: Vec<u8> = Vec::new();
        for k in 0..o.octaves.max(1) {
            for &p in &base {
                let q = p as i32 + 12 * k as i32;
                if (0..=127).contains(&q) {
                    notes.push(q as u8);
                }
            }
        }
        if o.style != Style::PlayOrder {
            notes.sort_unstable();
            notes.dedup();
        }
        let n = notes.len();
        let pat = pattern(o.style, n);
        if o.retrigger {
            pat_pos = 0;
            other_bag.clear();
        }
        if o.style == Style::RandomOnce && once_perm.as_ref().map_or(true, |p| p.len() != n) {
            let mut p: Vec<usize> = (0..n).collect();
            rng.shuffle(&mut p);
            once_perm = Some(p);
        }
        let end = sp.start + sp.len;
        let first = sp.start.div_ceil(step) * step;
        let mut t = first;
        let mut last_idx: Option<usize> = None;
        while t < end {
            let global = (t / step) as usize;
            let rhythm = if o.rhythm.is_empty() {
                Step::Hit
            } else {
                o.rhythm[global % o.rhythm.len()]
            };
            let vel = if o.accents.is_empty() {
                // 拍の頭(4 分)を少し強く
                let beat = crate::PPQ;
                if t % beat == 0 {
                    o.velocity.saturating_add(10).min(127)
                } else {
                    o.velocity
                }
            } else {
                o.accents[global % o.accents.len()]
            }
            .clamp(1, 127);
            match rhythm {
                Step::Tie => {
                    // 前の音(同じ和音の中のもの)を 1 ステップ伸ばす
                    if let Some(i) = last_idx {
                        for note in out[i..].iter_mut() {
                            note.dur = (note.dur + step).min(end - note.pos);
                        }
                    }
                }
                Step::Rest => {
                    last_idx = None;
                }
                Step::Hit => {
                    // 1 回りごとのずらし
                    let cycle_len = if o.style == Style::Chord {
                        1
                    } else {
                        match o.style {
                            Style::Random | Style::RandomOther | Style::RandomOnce => n,
                            _ => pat.len().max(1),
                        }
                    };
                    let cycle = pat_pos / cycle_len;
                    let shift = if o.shift_steps > 0 && o.shift != 0 {
                        o.shift * (cycle % (o.shift_steps as usize + 1)) as i32
                    } else {
                        0
                    };
                    let pitches: Vec<u8> = match o.style {
                        Style::Chord => notes.clone(),
                        Style::Random => vec![notes[rng.below(n)]],
                        Style::RandomOther => {
                            if other_bag.is_empty() {
                                other_bag = (0..n).collect();
                                rng.shuffle(&mut other_bag);
                            }
                            vec![notes[other_bag.pop().unwrap_or(0).min(n - 1)]]
                        }
                        Style::RandomOnce => {
                            let perm = once_perm.as_deref().unwrap_or(&[]);
                            vec![
                                notes[perm
                                    .get(pat_pos % n.max(1))
                                    .copied()
                                    .unwrap_or(0)
                                    .min(n - 1)],
                            ]
                        }
                        _ => vec![notes[pat[pat_pos % pat.len()].min(n - 1)]],
                    };
                    pat_pos += 1;
                    let dur = ((step as f64 * o.gate).round() as u64).max(1).min(end - t);
                    last_idx = Some(out.len());
                    for p in pitches {
                        let q = p as i32 + shift;
                        if (0..=127).contains(&q) {
                            out.push(CompNote {
                                pos: t,
                                dur,
                                pitch: q as u8,
                                vel,
                            });
                        }
                    }
                }
            }
            t += step;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_follow_live() {
        use Style::*;
        let p = |s| pattern(s, 4);
        assert_eq!(p(Up), [0, 1, 2, 3]);
        assert_eq!(p(Down), [3, 2, 1, 0]);
        assert_eq!(p(UpDown), [0, 1, 2, 3, 2, 1]);
        assert_eq!(p(DownUp), [3, 2, 1, 0, 1, 2]);
        assert_eq!(p(UpAndDown), [0, 1, 2, 3, 3, 2, 1, 0]);
        assert_eq!(p(DownAndUp), [3, 2, 1, 0, 0, 1, 2, 3]);
        assert_eq!(p(Converge), [0, 3, 1, 2]);
        assert_eq!(p(Diverge), [2, 1, 3, 0]);
        assert_eq!(p(ConDiverge), [0, 3, 1, 2, 1, 3]);
        assert_eq!(p(PinkyUp), [0, 3, 1, 3, 2, 3]);
        assert_eq!(p(PinkyUpDown), [0, 3, 1, 3, 2, 3, 1, 3]);
        assert_eq!(p(ThumbUp), [0, 1, 0, 2, 0, 3]);
        assert_eq!(p(ThumbUpDown), [0, 1, 0, 2, 0, 3, 0, 2]);
        assert_eq!(pattern(Converge, 3), [0, 2, 1]);
        assert_eq!(pattern(UpDown, 2), [0, 1]);
        assert_eq!(pattern(PinkyUp, 1), [0]);
        for name in STYLE_NAMES {
            assert!(Style::parse(name).is_some(), "{name}");
        }
        assert_eq!(Style::parse("Up & Down"), Some(UpAndDown));
    }

    fn opts(style: Style) -> Options {
        Options {
            style,
            step: 240,
            gate: 0.5,
            octaves: 1,
            accents: vec![],
            velocity: 80,
            rhythm: vec![],
            retrigger: true,
            shift: 0,
            shift_steps: 0,
            seed: 1,
        }
    }

    fn one_bar_c() -> (Vec<Span>, Vec<Vec<u8>>) {
        (
            vec![Span {
                start: 0,
                len: 3840,
                chord: Some(0),
            }],
            vec![vec![60, 64, 67]],
        )
    }

    #[test]
    fn up_in_sixteenths_over_two_octaves() {
        let (spans, chords) = one_bar_c();
        let o = Options {
            octaves: 2,
            ..opts(Style::Up)
        };
        let n = render(&spans, &chords, &chords, &o);
        assert_eq!(n.len(), 16);
        let pitches: Vec<u8> = n.iter().take(7).map(|x| x.pitch).collect();
        assert_eq!(pitches, [60, 64, 67, 72, 76, 79, 60]);
        assert_eq!(n[1].pos, 240);
        assert_eq!(n[0].dur, 120, "gate 0.5");
        assert_eq!(n[0].vel, 90, "拍の頭は少し強い");
        assert_eq!(n[1].vel, 80);
    }

    #[test]
    fn rhythm_and_accents_cycle_independently() {
        // 3 音の上りを、4 つの強弱と "x x - ." で回す(ポリメーター)
        let (spans, chords) = one_bar_c();
        let o = Options {
            accents: vec![100, 60, 80, 60],
            rhythm: parse_rhythm("x x - .").unwrap(),
            ..opts(Style::Up)
        };
        let n = render(&spans, &chords, &chords, &o);
        // 16 ステップのうち x は 8 つ
        assert_eq!(n.len(), 8);
        assert_eq!(n[0].pitch, 60);
        assert_eq!(n[1].pitch, 64);
        assert_eq!(n[1].dur, 120 + 240, "- で 1 ステップ伸びる");
        assert_eq!(n[2].pitch, 67, "休みの後も並びは続く");
        assert_eq!(n[2].pos, 240 * 4);
        assert_eq!(n[0].vel, 100);
        assert_eq!(n[1].vel, 60);
    }

    #[test]
    fn retrigger_restarts_at_chord_change() {
        let spans = vec![
            Span {
                start: 0,
                len: 720,
                chord: Some(0),
            },
            Span {
                start: 720,
                len: 720,
                chord: Some(1),
            },
        ];
        let chords = vec![vec![60, 64, 67], vec![57, 60, 64]];
        let n = render(&spans, &chords, &chords, &opts(Style::Up));
        let p: Vec<u8> = n.iter().map(|x| x.pitch).collect();
        assert_eq!(p, [60, 64, 67, 57, 60, 64]);
        let o = Options {
            retrigger: false,
            ..opts(Style::Up)
        };
        // 並びの位置を持ち越す: 最初の和音で 2 ステップ進んだので、次の和音は 3 番目の音から
        let spans2 = vec![
            Span {
                start: 0,
                len: 480,
                chord: Some(0),
            },
            Span {
                start: 480,
                len: 720,
                chord: Some(1),
            },
        ];
        let chords2 = vec![vec![60, 64], vec![57, 60, 64]];
        let n = render(&spans2, &chords2, &chords2, &o);
        let p: Vec<u8> = n.iter().map(|x| x.pitch).collect();
        assert_eq!(p, [60, 64, 64, 57, 60]);
    }

    #[test]
    fn chord_trigger_shift_and_random() {
        let (spans, chords) = one_bar_c();
        let n = render(&spans, &chords, &chords, &opts(Style::Chord));
        assert_eq!(n.len(), 48, "16 ステップ × 3 音");
        // 1 回りごとに +12 を 1 段(0, +12, 0, +12 …)
        let o = Options {
            shift: 12,
            shift_steps: 1,
            ..opts(Style::Up)
        };
        let n = render(&spans, &chords, &chords, &o);
        let p: Vec<u8> = n.iter().take(7).map(|x| x.pitch).collect();
        assert_eq!(p, [60, 64, 67, 72, 76, 79, 60]);
        // random_other: 3 つずつ区切ると、どの 3 つも全部の音を 1 回ずつ
        let n = render(&spans, &chords, &chords, &opts(Style::RandomOther));
        for chunk in n.chunks(3).filter(|c| c.len() == 3) {
            let mut v: Vec<u8> = chunk.iter().map(|x| x.pitch).collect();
            v.sort();
            assert_eq!(v, [60, 64, 67]);
        }
        // random_once: 3 つの並びを繰り返す
        let n = render(&spans, &chords, &chords, &opts(Style::RandomOnce));
        assert_eq!(n[0].pitch, n[3].pitch);
        assert_eq!(n[1].pitch, n[4].pitch);
        // 同じ種なら同じ結果
        let a = render(&spans, &chords, &chords, &opts(Style::Random));
        let b = render(&spans, &chords, &chords, &opts(Style::Random));
        assert_eq!(a, b);
    }

    #[test]
    fn play_order_uses_chord_roles() {
        // C/E(E G C)の役割の順は C E G
        let (spans, _) = one_bar_c();
        let voiced = vec![vec![64, 67, 72]];
        let order = vec![vec![72, 64, 67]];
        let n = render(&spans, &voiced, &order, &opts(Style::PlayOrder));
        let p: Vec<u8> = n.iter().take(4).map(|x| x.pitch).collect();
        assert_eq!(p, [72, 64, 67, 72]);
    }

    #[test]
    fn notes_do_not_cross_the_chord_change() {
        let spans = vec![
            Span {
                start: 0,
                len: 600,
                chord: Some(0),
            },
            Span {
                start: 600,
                len: 600,
                chord: Some(0),
            },
        ];
        let chords = vec![vec![60, 64, 67]];
        let o = Options {
            gate: 1.5,
            ..opts(Style::Up)
        };
        let n = render(&spans, &chords, &chords, &o);
        // 480 の音は 600 で切れる(360 ではなく 120)
        let at480 = n.iter().find(|x| x.pos == 480).unwrap();
        assert_eq!(at480.dur, 120);
        // 区間の途中(600)は格子(720)から始まる
        assert!(n.iter().any(|x| x.pos == 720));
        assert!(!n.iter().any(|x| x.pos == 600));
    }
}
