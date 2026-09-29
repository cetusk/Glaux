//! 拍子と拍のまとまり(変拍子)。どの道具も「16 分の格子 × まとまり」から拍を引く。
//!
//! 変拍子の大半は 2 と 3 のまとまりの足し算(7/8 = 2+2+3 など)。拍子記号の分子と分母だけでは
//! まとまりが決まらないので、`TimeSigEvent.grouping` に持ち、無ければ [`default_grouping`] で決める。
//! 1 小節の 16 分のステップ数は `num × 16 / den`(7/8 = 14、5/4 = 20)。「1 小節を 16 等分」はしない。

use crate::model::Project;
use crate::time::{TimeSigEvent, PPQ};

/// 16 分 1 つの tick
pub const STEP: u64 = PPQ / 4;

/// まとまりの既定値(分母の音符の数)。
/// - 分母 4 以下: 4 分ずつが拍(全部 1)。5/4 は 3+2、7/4 は 4+3、6/4 は 3+3 を上の段として持つ
/// - 分母 8 以上で分子が 3 の倍数: 3 ずつ(複合拍子。6/8 = 3+3、12/8 = 3+3+3+3)
/// - 分母 8 以上の奇数: 2 を並べて最後に 3(5/8 = 2+3、7/8 = 2+2+3、11/8 = 2+2+2+2+3。
///   ユークリッドリズム E(⌊n/2⌋, n) の間隔と同じ)
/// - 分母 8 以上の偶数: 2 ずつ
pub fn default_grouping(num: u8, den: u8) -> Vec<u8> {
    let num = num.max(1);
    if den <= 4 {
        return match num {
            5 => vec![3, 2],
            6 => vec![3, 3],
            7 => vec![4, 3],
            n => vec![1; n as usize],
        };
    }
    if num % 3 == 0 {
        return vec![3; (num / 3) as usize];
    }
    if num == 1 {
        return vec![1];
    }
    if num % 2 == 1 {
        let mut v = vec![2; ((num - 3) / 2) as usize];
        v.push(3);
        return v;
    }
    vec![2; (num / 2) as usize]
}

/// まとまりの検査(和が分子と同じ・0 が無い)
pub fn check_grouping(num: u8, grouping: &[u8]) -> Result<(), String> {
    if grouping.is_empty() || grouping.contains(&0) {
        return Err("grouping は 1 以上の数の並び".to_owned());
    }
    let sum: u32 = grouping.iter().map(|&g| g as u32).sum();
    if sum != num as u32 {
        return Err(format!(
            "grouping の和({sum})が拍子の分子({num})と合いません"
        ));
    }
    Ok(())
}

/// "2+2+3" / "2,2,3" / "223" を読む
pub fn parse_grouping(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    let parts: Vec<&str> = if s.contains(['+', ',', ' ']) {
        s.split(['+', ',', ' ']).filter(|p| !p.is_empty()).collect()
    } else {
        s.split("").filter(|p| !p.is_empty()).collect()
    };
    parts
        .iter()
        .map(|p| {
            p.parse::<u8>()
                .ok()
                .filter(|&n| n >= 1)
                .ok_or_else(|| format!("grouping が読めません: {s}(例 \"2+2+3\")"))
        })
        .collect()
}

pub fn grouping_text(g: &[u8]) -> String {
    g.iter()
        .map(|x| x.to_string())
        .collect::<Vec<_>>()
        .join("+")
}

/// 1 小節の拍子
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarMeter {
    /// 小節の頭(tick)と長さ
    pub start: u64,
    pub len: u64,
    pub num: u8,
    pub den: u8,
    /// まとまり(分母の音符の数)
    pub grouping: Vec<u8>,
}

impl BarMeter {
    pub fn from_sig(sig: &TimeSigEvent, start: u64, len: u64) -> BarMeter {
        let grouping = sig
            .grouping
            .clone()
            .filter(|g| check_grouping(sig.num, g).is_ok())
            .unwrap_or_else(|| default_grouping(sig.num, sig.den));
        BarMeter {
            start,
            len,
            num: sig.num,
            den: sig.den,
            grouping,
        }
    }

    /// 4/4 の小節
    pub fn common(start: u64) -> BarMeter {
        BarMeter::from_sig(
            &TimeSigEvent::new(crate::time::Tick(start), 4, 4),
            start,
            PPQ * 4,
        )
    }

    /// 分母の音符 1 つの tick(8 分 = 480)
    pub fn unit(&self) -> u64 {
        (PPQ * 4 / self.den.max(1) as u64).max(1)
    }

    /// 16 分のステップ数(7/8 = 14)。途中で切れた小節(拍子の変わり目の前)は、その長さぶん
    pub fn steps(&self) -> u64 {
        self.len.div_ceil(STEP)
    }

    /// 4/4 か(今までの型をそのまま使える)
    pub fn is_common(&self) -> bool {
        self.num == 4 && self.den == 4 && self.len == PPQ * 4
    }

    /// まとまりの頭(小節の頭からの tick)と長さ
    pub fn groups(&self) -> Vec<(u64, u64)> {
        let u = self.unit();
        let mut out = Vec::new();
        let mut t = 0u64;
        for &g in &self.grouping {
            if t >= self.len {
                break;
            }
            let l = (g as u64 * u).min(self.len - t);
            out.push((t, l));
            t += g as u64 * u;
        }
        out
    }

    /// 小節の頭からの `rel` の拍の強さ: 0 = 小節の頭 / 1 = まとまりの頭 / 2 = 8 分 / 3 = 16 分 / 4 = それより細かい。
    /// 4/4(まとまりが 4 分ずつ)なら 1 = 4 分、2 = 8 分の裏
    pub fn level(&self, rel: u64) -> u8 {
        if rel == 0 {
            return 0;
        }
        if self.groups().iter().any(|g| g.0 == rel) {
            return 1;
        }
        if rel % (PPQ / 2) == 0 {
            2
        } else if rel % STEP == 0 {
            3
        } else {
            4
        }
    }

    /// 拍の並び(小節の頭からの tick と強さ)。16 分ごと
    pub fn beats(&self) -> Vec<Beat> {
        (0..self.steps())
            .map(|i| {
                let t = i * STEP;
                Beat {
                    tick: t,
                    len: STEP.min(self.len - t),
                    level: self.level(t),
                }
            })
            .collect()
    }

    /// 強拍(小節の頭からの tick)。4 分ずつの拍子(4/4・3/4 など)は小節の頭と、偶数拍子の半ば。
    /// まとまりのある拍子(7/8・6/8・5/4 など)はまとまりの頭
    pub fn strong_ticks(&self) -> Vec<u64> {
        if self.grouping.iter().all(|&g| g == 1) {
            let mut v = vec![0];
            if self.num % 2 == 0 && self.num >= 4 {
                v.push(self.len / 2);
            }
            return v;
        }
        self.groups().iter().map(|g| g.0).collect()
    }

    /// 小節の中の 16 分の番号 `idx` を、4/4 の型の 16 か所のどれに当てるか。
    /// 4 分ずつの拍子は 16 で割った余り、まとまりのある分母 8 以上の拍子は [`fit_pattern`] と同じ当て方
    /// (まとまり i に 4/4 の i 拍目、3 のまとまりの 5・6 番目は 8 分の後半を繰り返す)
    pub fn slot16(&self, idx: i64) -> usize {
        if self.is_common() || self.den <= 4 {
            return idx.rem_euclid(16) as usize;
        }
        let k = idx.rem_euclid(self.steps().max(1) as i64) as u64 * STEP;
        let groups = self.groups();
        let gi = groups.iter().rposition(|g| g.0 <= k).unwrap_or(0);
        let j = ((k - groups[gi].0) / STEP) as usize;
        (gi % 4) * 4 + if j < 4 { j } else { 2 + (j - 4) % 2 }
    }

    /// スウィングの組(長さ `pair`)の頭。組はまとまりの頭から数え、まとまりに収まらない余り
    /// (3 のまとまりの最後の 8 分など)は None(ハネさせない)
    pub fn pair_start(&self, rel: u64, pair: u64) -> Option<u64> {
        let pair = pair.max(1);
        let &(h, l) = self.groups().iter().rev().find(|g| g.0 <= rel)?;
        let ps = h + (rel - h) / pair * pair;
        (ps + pair <= h + l).then_some(ps)
    }

    /// "7/8 (2+2+3)" の形(4 分ずつの拍子は "4/4" だけ)
    pub fn label(&self) -> String {
        if self.grouping.iter().all(|&g| g == 1) {
            return format!("{}/{}", self.num, self.den);
        }
        format!(
            "{}/{} ({})",
            self.num,
            self.den,
            grouping_text(&self.grouping)
        )
    }
}

/// 型の文字列を小節に当てる(返り値: 16 分のステップの型。None なら今までどおり小節を等分する)。
/// - 4/4 の小節・型の長さが 16 でも小節のステップ数でもない: None
/// - 型の長さが小節のステップ数と同じ(7/8 なら 14 文字): そのまま
/// - 4/4 の 16 ステップの型: 分母 4 以下は 16 分の格子で切るか延ばす(3/4 = 前の 12 ステップ、5/4 = 16 + 前の 4)。
///   分母 8 以上は、4/4 の 1 拍ぶん(4 ステップ)を各まとまりに当て、3 のまとまりは 8 分の後半を繰り返して延ばす
///   (8 分の刻み x.x. → x.x.x.、裏 ..x. → ..x.x.、伸ばし x--- → x-----)
pub fn fit_pattern(pat: &[char], m: Option<&BarMeter>) -> Option<Vec<char>> {
    let m = m.filter(|m| !m.is_common())?;
    let steps = m.steps() as usize;
    if pat.len() == steps {
        return Some(pat.to_vec());
    }
    if pat.len() != 16 {
        return None;
    }
    if m.den <= 4 {
        return Some((0..steps).map(|k| pat[k % 16]).collect());
    }
    let mut out = vec!['.'; steps];
    for (gi, (h, l)) in m.groups().iter().enumerate() {
        let h = (h / STEP) as usize;
        let l = l.div_ceil(STEP) as usize;
        let cell = &pat[(gi % 4) * 4..(gi % 4) * 4 + 4];
        for j in 0..l {
            if h + j >= steps {
                break;
            }
            out[h + j] = if j < 4 {
                cell[j]
            } else {
                cell[2 + (j - 4) % 2]
            };
        }
    }
    Some(out)
}

/// 拍 1 つ(小節の頭からの tick)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Beat {
    pub tick: u64,
    pub len: u64,
    pub level: u8,
}

/// 小節ごとの拍子(`end_tick` を越えるまで)
pub fn bar_meters(project: &Project, end_tick: u64) -> Vec<BarMeter> {
    let grid = crate::arrange::bar_grid(project, end_tick);
    let mut sigs = project.time_sig_map.clone();
    sigs.sort_by_key(|s| s.tick);
    let default = TimeSigEvent::new(crate::time::Tick(0), 4, 4);
    grid.iter()
        .map(|&(start, len)| {
            let sig = sigs
                .iter()
                .rev()
                .find(|s| s.tick.0 <= start)
                .unwrap_or(&default);
            BarMeter::from_sig(sig, start, len)
        })
        .collect()
}

/// `tick` を含む小節の拍子
pub fn meter_at(project: &Project, tick: u64) -> BarMeter {
    bar_meters(project, tick + 1)
        .into_iter()
        .rev()
        .find(|m| m.start <= tick)
        .unwrap_or_else(|| BarMeter::common(0))
}

/// ユークリッドリズム E(k, n): n ステップに k 個の打点をなるべく均等に(Toussaint)。
/// `rotation` で左へ回す。E(3,8) = x..x..x.、E(3,7) = x.x.x..
pub fn euclid(k: usize, n: usize, rotation: usize) -> Vec<bool> {
    if n == 0 {
        return vec![];
    }
    let k = k.min(n);
    let mut out: Vec<bool> = if k == 0 || k == n {
        vec![k == n; n]
    } else {
        // Bjorklund の手順: 打点の列と休みの列を、余りが 1 つ以下になるまで後ろにつなぐ
        let mut a: Vec<Vec<bool>> = vec![vec![true]; k];
        let mut b: Vec<Vec<bool>> = vec![vec![false]; n - k];
        while b.len() > 1 {
            let m = a.len().min(b.len());
            let joined: Vec<Vec<bool>> = (0..m)
                .map(|i| {
                    let mut x = a[i].clone();
                    x.extend(&b[i]);
                    x
                })
                .collect();
            let rest = if a.len() > m {
                a[m..].to_vec()
            } else {
                b[m..].to_vec()
            };
            a = joined;
            b = rest;
        }
        a.into_iter().chain(b).flatten().collect()
    };
    out.rotate_left(rotation % n);
    out
}

/// "E(5,16)" / "E(3,8,2)" を読む
pub fn parse_euclid(s: &str) -> Option<Vec<bool>> {
    let t = s.trim();
    let inner = t
        .strip_prefix("E(")
        .or_else(|| t.strip_prefix("e("))?
        .strip_suffix(')')?;
    let nums: Vec<usize> = inner
        .split(',')
        .map(|x| x.trim().parse().ok())
        .collect::<Option<_>>()?;
    match nums.as_slice() {
        [k, n] if *n >= 1 && *n <= 128 => Some(euclid(*k, *n, 0)),
        [k, n, r] if *n >= 1 && *n <= 128 => Some(euclid(*k, *n, *r)),
        _ => None,
    }
}

/// ポリリズム: 長さ `span` を `count` 等分した位置(tick に丸める)と、丸めの誤差の最大(tick、小数)。
/// 3 連の 4 分(2 拍 = 1920 を 3 等分)= 640 は割り切れる
pub fn polyrhythm(span: u64, count: u32) -> (Vec<u64>, f64) {
    let count = count.max(1) as u64;
    let mut err = 0.0f64;
    let pos = (0..count)
        .map(|k| {
            let exact = k as f64 * span as f64 / count as f64;
            let r = exact.round() as u64;
            err = err.max((exact - r as f64).abs());
            r
        })
        .collect();
    (pos, err)
}

/// ポリメーターの型("X..x.." か "E(5,16)")を読む。'X' = 強く、'x' = 普通、'.' = 休み
pub fn parse_cycle(s: &str) -> Result<Vec<char>, String> {
    if let Some(v) = parse_euclid(s) {
        return Ok(v
            .iter()
            .enumerate()
            .map(|(i, &b)| match (b, i) {
                (true, 0) => 'X',
                (true, _) => 'x',
                _ => '.',
            })
            .collect());
    }
    let v: Vec<char> = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '|')
        .collect();
    if v.is_empty() || v.len() > 64 {
        return Err("pattern は X x . の 1〜64 文字か \"E(5,16)\"".to_owned());
    }
    if let Some(c) = v.iter().find(|c| !matches!(c, 'X' | 'x' | '.')) {
        return Err(format!("pattern に使えない文字「{c}」(X x . だけ)"));
    }
    if !v.iter().any(|c| *c != '.') {
        return Err("pattern に打つ所(X か x)がありません".to_owned());
    }
    Ok(v)
}

/// 最大公約数・最小公倍数
pub fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

pub fn lcm(a: u64, b: u64) -> u64 {
    if a == 0 || b == 0 {
        0
    } else {
        a / gcd(a, b) * b
    }
}

/// まとまり 1 つの写し方: ((元の頭, 元の長さ), (新しい頭, 新しい長さ))。小節の頭からの tick
pub type FeelSegment = ((u64, u64), (u64, u64));

/// アクサクの揺れ: まとまり 2 と 3 の長さの比を変えた、小節の中の区間の写し方
/// (元の頭・長さ → 新しい頭・長さ)。`long_ratio` = 3 のまとまりの長さ ÷ 2 のまとまりの長さ(1.5 で変えない)、
/// `short_skew` = 小節の 2 つ目以降の 2 のまとまりの長さ ÷ 最初の 2 のまとまりの長さ(1.0 で変えない)。
/// 小節の長さは変えない。まとまりが 2 と 3 だけで、両方あるか 2 が 2 つ以上ある小節だけ(ほかは None)
pub fn feel_segments(m: &BarMeter, long_ratio: f64, short_skew: f64) -> Option<Vec<FeelSegment>> {
    let groups = m.groups();
    if groups.len() < 2 || m.grouping.iter().any(|&g| g != 2 && g != 3) {
        return None;
    }
    let mut seen_short = false;
    let weights: Vec<f64> = m
        .grouping
        .iter()
        .map(|&g| {
            if g == 3 {
                long_ratio
            } else if seen_short {
                short_skew
            } else {
                seen_short = true;
                1.0
            }
        })
        .collect();
    let total: f64 = weights.iter().sum();
    let mut t = 0.0f64;
    let mut out = Vec::new();
    for (g, w) in groups.iter().zip(&weights) {
        let a = t.round() as u64;
        t += w / total * m.len as f64;
        let b = (t.round() as u64).min(m.len);
        out.push((*g, (a, b - a)));
    }
    Some(out)
}

/// `feel_segments` で小節の中の位置 `rel` を写す(区間の中は比例で)
pub fn feel_map(segs: &[FeelSegment], rel: u64) -> u64 {
    let Some(&((h, l), (nh, nl))) = segs.iter().rev().find(|s| s.0 .0 <= rel) else {
        return rel;
    };
    nh + ((rel - h) as f64 * nl as f64 / l.max(1) as f64).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::Tick;

    #[test]
    fn default_groupings() {
        assert_eq!(default_grouping(4, 4), vec![1, 1, 1, 1]);
        assert_eq!(default_grouping(3, 4), vec![1, 1, 1]);
        assert_eq!(default_grouping(5, 4), vec![3, 2]);
        assert_eq!(default_grouping(7, 4), vec![4, 3]);
        assert_eq!(default_grouping(6, 8), vec![3, 3]);
        assert_eq!(default_grouping(12, 8), vec![3, 3, 3, 3]);
        assert_eq!(default_grouping(5, 8), vec![2, 3]);
        assert_eq!(default_grouping(7, 8), vec![2, 2, 3]);
        assert_eq!(default_grouping(11, 8), vec![2, 2, 2, 2, 3]);
        assert_eq!(default_grouping(13, 16), vec![2, 2, 2, 2, 2, 3]);
        assert_eq!(default_grouping(10, 8), vec![2, 2, 2, 2, 2]);
        for (n, d) in [(1, 4), (2, 2), (9, 8), (15, 16), (4, 8)] {
            assert!(
                check_grouping(n, &default_grouping(n, d)).is_ok(),
                "{n}/{d}"
            );
        }
    }

    #[test]
    fn groupings_parse_and_check() {
        assert_eq!(parse_grouping("2+2+3").unwrap(), vec![2, 2, 3]);
        assert_eq!(parse_grouping("3,2,2").unwrap(), vec![3, 2, 2]);
        assert_eq!(parse_grouping("223").unwrap(), vec![2, 2, 3]);
        assert!(parse_grouping("2+x").is_err());
        assert!(check_grouping(7, &[2, 2, 3]).is_ok());
        assert!(check_grouping(7, &[2, 2, 2]).is_err());
        assert!(check_grouping(7, &[]).is_err());
    }

    #[test]
    fn seven_eight_has_fourteen_steps_and_group_heads() {
        let mut sig = TimeSigEvent::new(Tick(0), 7, 8);
        let m = BarMeter::from_sig(&sig, 0, 3360);
        assert_eq!(m.steps(), 14);
        assert_eq!(m.groups(), vec![(0, 960), (960, 960), (1920, 1440)]);
        assert_eq!(m.level(0), 0);
        assert_eq!(m.level(960), 1);
        assert_eq!(m.level(1920), 1);
        assert_eq!(m.level(2400), 2);
        assert_eq!(m.level(240), 3);
        assert_eq!(m.label(), "7/8 (2+2+3)");
        sig.grouping = Some(vec![3, 2, 2]);
        let m = BarMeter::from_sig(&sig, 0, 3360);
        assert_eq!(m.groups(), vec![(0, 1440), (1440, 960), (2400, 960)]);
        // 和の合わないまとまりは既定値に
        sig.grouping = Some(vec![3, 3]);
        assert_eq!(BarMeter::from_sig(&sig, 0, 3360).grouping, vec![2, 2, 3]);
        assert_eq!(m.strong_ticks(), vec![0, 1440, 2400]);
        // 4/4 は 4 分ずつ。強拍は頭と半ば
        let c = BarMeter::common(0);
        assert_eq!(c.strong_ticks(), vec![0, 1920]);
        assert!(c.is_common());
        assert_eq!(c.groups().len(), 4);
        assert_eq!(c.beats().len(), 16);
    }

    #[test]
    fn meters_follow_the_time_signature_map() {
        let mut p = Project::new("t");
        p.time_sig_map = vec![
            TimeSigEvent::new(Tick(0), 4, 4),
            TimeSigEvent {
                tick: Tick(3840),
                num: 7,
                den: 8,
                grouping: Some(vec![3, 2, 2]),
            },
        ];
        let ms = bar_meters(&p, 3840 + 3360 * 2);
        assert_eq!(ms[0].num, 4);
        assert_eq!((ms[1].start, ms[1].len), (3840, 3360));
        assert_eq!(ms[2].grouping, vec![3, 2, 2]);
        assert_eq!(meter_at(&p, 3840 + 3360 + 5).start, 3840 + 3360);
    }

    #[test]
    fn patterns_fit_odd_bars() {
        let p = |s: &str| s.chars().collect::<Vec<char>>();
        let st = |v: Vec<char>| v.into_iter().collect::<String>();
        let seven = BarMeter::from_sig(&TimeSigEvent::new(Tick(0), 7, 8), 0, 3360);
        assert_eq!(
            st(fit_pattern(&p("x.x.x.x.x.x.x.x."), Some(&seven)).unwrap()),
            "x.x.x.x.x.x.x."
        );
        assert_eq!(
            st(fit_pattern(&p("x-------x-------"), Some(&seven)).unwrap()),
            "x-------x-----"
        );
        assert_eq!(
            st(fit_pattern(&p("..x...x...x...x."), Some(&seven)).unwrap()),
            "..x...x...x.x."
        );
        // 小節のステップ数と同じ長さはそのまま
        assert_eq!(
            st(fit_pattern(&p("x...x...x....."), Some(&seven)).unwrap()),
            "x...x...x....."
        );
        // 3/4 は切る、5/4 は延ばす
        let three = BarMeter::from_sig(&TimeSigEvent::new(Tick(0), 3, 4), 0, 2880);
        assert_eq!(
            st(fit_pattern(&p("x...x...x...x..."), Some(&three)).unwrap()),
            "x...x...x..."
        );
        let five = BarMeter::from_sig(&TimeSigEvent::new(Tick(0), 5, 4), 0, 4800);
        assert_eq!(
            fit_pattern(&p("x...x...x...x..."), Some(&five))
                .unwrap()
                .len(),
            20
        );
        // 4/4・長さの合わない型は None(等分)
        assert!(fit_pattern(&p("x.x."), Some(&seven)).is_none());
        assert!(fit_pattern(&p("x.x.x.x.x.x.x.x."), Some(&BarMeter::common(0))).is_none());
        assert!(fit_pattern(&p("x.x.x.x.x.x.x.x."), None).is_none());
    }

    #[test]
    fn slots_and_swing_pairs() {
        let seven = BarMeter::from_sig(&TimeSigEvent::new(Tick(0), 7, 8), 0, 3360);
        let slots: Vec<usize> = (0..14).map(|i| seven.slot16(i)).collect();
        assert_eq!(slots, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 10, 11]);
        let c = BarMeter::common(0);
        assert_eq!(c.slot16(17), 1);
        // 8 分のスウィング(組 = 960): 3 のまとまりの最後の 8 分は組にならない
        assert_eq!(seven.pair_start(1440, 960), Some(960));
        assert_eq!(seven.pair_start(2400, 960), Some(1920));
        assert_eq!(seven.pair_start(2880, 960), None);
        assert_eq!(c.pair_start(2400, 960), Some(1920));
    }

    #[test]
    fn polyrhythm_polymeter_and_feel() {
        // 2 拍に 3 つ = 640 ごと(割り切れる)。1 小節に 5 つは割り切れず誤差が出る
        assert_eq!(polyrhythm(1920, 3), (vec![0, 640, 1280], 0.0));
        let (p, e) = polyrhythm(3840, 7);
        assert_eq!(p.len(), 7);
        assert!(e > 0.0 && e <= 0.5);
        assert_eq!(parse_cycle("X..x..").unwrap().len(), 6);
        assert_eq!(
            parse_cycle("E(3,8)").unwrap().iter().collect::<String>(),
            "X..x..x."
        );
        assert!(parse_cycle("x-q").is_err());
        assert!(parse_cycle("....").is_err());
        assert_eq!(lcm(3, 16), 48);
        // 7/8 (2+2+3) の長い拍を 1.4 倍に(1.5 より短く): 小節の長さは同じ、3 のまとまりが縮む
        let m = BarMeter::from_sig(&TimeSigEvent::new(Tick(0), 7, 8), 0, 3360);
        let segs = feel_segments(&m, 1.4, 1.0).unwrap();
        let total: u64 = segs.iter().map(|s| s.1 .1).sum();
        assert_eq!(total, 3360);
        assert!(segs[2].1 .1 < 1440);
        assert_eq!(feel_map(&segs, 0), 0);
        assert!(feel_map(&segs, 960) > 960);
        // 1.5・1.0 なら変わらない
        let same = feel_segments(&m, 1.5, 1.0).unwrap();
        assert!(same.iter().all(|s| s.0 == s.1));
        // 4/4 は対象外
        assert!(feel_segments(&BarMeter::common(0), 1.4, 1.0).is_none());
    }

    #[test]
    fn euclidean_rhythms() {
        let s = |v: Vec<bool>| {
            v.iter()
                .map(|&b| if b { 'x' } else { '.' })
                .collect::<String>()
        };
        assert_eq!(s(euclid(3, 8, 0)), "x..x..x.");
        assert_eq!(s(euclid(3, 7, 0)), "x.x.x..");
        assert_eq!(s(euclid(5, 8, 0)), "x.xx.xx.");
        assert_eq!(s(euclid(4, 9, 0)), "x.x.x.x..");
        assert_eq!(s(euclid(4, 16, 0)), "x...x...x...x...");
        assert_eq!(s(euclid(3, 8, 1)), "..x..x.x");
        assert_eq!(s(euclid(8, 8, 0)), "xxxxxxxx");
        assert_eq!(s(parse_euclid("E(3,8)").unwrap()), "x..x..x.");
        assert!(parse_euclid("E(3)").is_none());
        assert_eq!(euclid(0, 4, 0), vec![false; 4]);
    }
}
