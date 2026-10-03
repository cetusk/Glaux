//! ワブルの「速さのリズム」(write_wobble)の中身。
//!
//! ワブルベースは、1 拍(や半拍)ごとに揺れの速さを 1/4 → 1/8 → 1/16t … と切り替えて「しゃべる」。
//! 声ごとのモジュレーター(`mod1_rate`。拍あたりの回数)のオートメーションを、拍に合わせて段々(hold)に置く。

use glaux_core::{AutomationPoint, Curve, Tick};

/// 速さの書き方を、拍(4 分音符)あたりの回数にする。
/// "1/4" = 1、"1/8" = 2、"1/16" = 4、"1/32" = 8、"1/2" = 0.5、"1/1" = 0.25、"2/1" = 0.125。
/// 末尾の t は 3 連(×1.5)、d は付点(÷1.5)。数だけ("3")ならそのまま拍あたりの回数
pub fn parse_rate(s: &str) -> Option<f64> {
    let s = s.trim();
    let (body, mul) = if let Some(b) = s.strip_suffix('t') {
        (b, 1.5)
    } else if let Some(b) = s.strip_suffix('d') {
        (b, 1.0 / 1.5)
    } else {
        (s, 1.0)
    };
    let v = match body.split_once('/') {
        Some((n, d)) => {
            let n: f64 = n.trim().parse().ok()?;
            let d: f64 = d.trim().parse().ok()?;
            if n <= 0.0 || d <= 0.0 {
                return None;
            }
            // 1/8 = 8 分音符 1 つで 1 周期 → 拍(4 分)あたり d / (4 n) 回
            d / (4.0 * n)
        }
        None => body.parse::<f64>().ok()?,
    } * mul;
    (v.is_finite() && (1.0 / 32.0..=32.0).contains(&v)).then_some(v)
}

/// 区間 `[start, end)` を `step` tick ごとに区切り、`pattern` の速さを順に(繰り返して)置く段々の点。
/// "." は前と同じ(つなげる)。最初の要素が "." なら `before` の値を使う。
/// 返すのは区間の点と、区間の後に戻す値の点(`after` があれば)
pub fn pattern_points(
    start: u64,
    end: u64,
    step: u64,
    pattern: &[String],
    before: f64,
    after: Option<f64>,
) -> Result<Vec<AutomationPoint>, String> {
    if pattern.is_empty() {
        return Err("pattern が空です(例 [\"1/8\", \"1/16\", \"1/4\", \"1/8t\"])".to_owned());
    }
    if step == 0 || end <= start {
        return Err("区間か刻みが空です".to_owned());
    }
    let rates: Vec<Option<f64>> = pattern
        .iter()
        .map(|p| {
            if p.trim() == "." {
                Ok(None)
            } else {
                parse_rate(p).map(Some).ok_or_else(|| {
                    format!("速さが読めません: {p}(\"1/4\"・\"1/8\"・\"1/8t\"・\"1/16d\" か拍あたりの回数)")
                })
            }
        })
        .collect::<Result<_, _>>()?;
    let mut out = Vec::new();
    let mut prev = before;
    let mut i = 0usize;
    let mut t = start;
    while t < end {
        let v = rates[i % rates.len()].unwrap_or(prev);
        // 同じ値が続くなら点を増やさない
        if out.is_empty() || v != prev {
            out.push(AutomationPoint {
                tick: Tick(t),
                value: v,
                curve: Curve::Hold,
            });
        }
        prev = v;
        i += 1;
        t += step;
    }
    if let Some(a) = after {
        out.push(AutomationPoint {
            tick: Tick(end),
            value: a,
            curve: Curve::Hold,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_read_as_cycles_per_beat() {
        assert_eq!(parse_rate("1/4"), Some(1.0));
        assert_eq!(parse_rate("1/8"), Some(2.0));
        assert_eq!(parse_rate("1/16"), Some(4.0));
        assert_eq!(parse_rate("1/32"), Some(8.0));
        assert_eq!(parse_rate("1/2"), Some(0.5));
        assert_eq!(parse_rate("1/1"), Some(0.25));
        assert_eq!(parse_rate("2/1"), Some(0.125));
        assert!((parse_rate("1/8t").unwrap() - 3.0).abs() < 1e-9);
        assert!((parse_rate("1/16t").unwrap() - 6.0).abs() < 1e-9);
        assert!((parse_rate("1/8d").unwrap() - 4.0 / 3.0).abs() < 1e-9);
        assert_eq!(parse_rate("3"), Some(3.0));
        assert_eq!(parse_rate("1/0"), None);
        assert_eq!(parse_rate("abc"), None);
        assert_eq!(parse_rate("1/512"), None);
    }

    #[test]
    fn pattern_steps_repeat_and_hold() {
        let pat: Vec<String> = ["1/8", "1/16", ".", "1/4"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // 2 小節(8 拍)、拍ごと。戻す値 2
        let pts = pattern_points(0, 7680, 960, &pat, 2.0, Some(2.0)).unwrap();
        let v: Vec<(u64, f64)> = pts.iter().map(|p| (p.tick.0, p.value)).collect();
        assert_eq!(
            v,
            vec![
                (0, 2.0),
                (960, 4.0),
                (2880, 1.0),
                (3840, 2.0),
                (4800, 4.0),
                (6720, 1.0),
                (7680, 2.0),
            ]
        );
        assert!(pts.iter().all(|p| p.curve == Curve::Hold));
        assert!(pattern_points(0, 960, 960, &[], 2.0, None).is_err());
        assert!(pattern_points(0, 960, 960, &["fast".to_string()], 2.0, None).is_err());
    }
}
