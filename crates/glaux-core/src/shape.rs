//! オートメーションを「区間と形」で書くための計算(MCP の `shape_automation` の中身)。
//!
//! AI がビルドアップ 1 つのために、フィルタの点を十数個、正しい tick で並べるのは手間が大きく、
//! 実際に AI が作った曲ではオートメーションが 1 本も使われていなかった。ここでは
//! 「どこからどこまで・どんな形で・どの値からどの値へ」から点の列を作り、既存のレーンに差し込む。
//! 形は細かい直線の点で近似する(エンジンの点の曲線は線形・保持・指数の 3 種なので)。

use crate::model::{AutomationPoint, Curve, Project};
use crate::time::{Tick, PPQ};

/// 形。`from` → `to` の変化のしかた
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// 一定の速さで
    Linear,
    /// 初めゆっくり・終わりで急に(ビルドアップのフィルタ・ライザー)
    Exp,
    /// 初め急に・終わりでゆっくり(フェードアウト)
    Log,
    /// なめらかに始まってなめらかに終わる(音量の出し入れ)
    SCurve,
    /// `to` へ膨らんで `from` へ戻る(スウェル)
    Swell,
    /// `to` へ沈んで `from` へ戻る(一瞬抜く)
    Dip,
    /// 区間の頭で `to` に切り替えて保つ
    Step,
    /// 周期 `period` で `from` と `to` の間を揺れる
    Lfo(LfoShape),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LfoShape {
    Sine,
    Triangle,
    /// 周期の頭で `to`、周期の終わりで `from` に戻る(拍ごとのポンピング: from = 0dB, to = -8dB)
    SawDown,
    /// 周期の頭で `from`、終わりで `to`
    SawUp,
    Square,
}

impl Shape {
    pub fn parse(name: &str) -> Option<Shape> {
        Some(match name {
            "linear" => Shape::Linear,
            "exp" => Shape::Exp,
            "log" => Shape::Log,
            "s_curve" => Shape::SCurve,
            "swell" => Shape::Swell,
            "dip" => Shape::Dip,
            "step" => Shape::Step,
            "sine" => Shape::Lfo(LfoShape::Sine),
            "triangle" => Shape::Lfo(LfoShape::Triangle),
            "saw_down" | "pump" => Shape::Lfo(LfoShape::SawDown),
            "saw_up" => Shape::Lfo(LfoShape::SawUp),
            "square" => Shape::Lfo(LfoShape::Square),
            _ => return None,
        })
    }

    /// 区間の中の位置 x(0〜1)での、from → to の割合(0 = from、1 = to)
    fn amount(self, x: f64, period_frac: f64) -> f64 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Shape::Linear => x,
            // 指数・対数は曲がり具合を固定(4 倍の曲がり)。音量・周波数の聞こえ方に近い
            Shape::Exp => ((4.0 * x).exp() - 1.0) / (4f64.exp() - 1.0),
            Shape::Log => 1.0 - ((4.0 * (1.0 - x)).exp() - 1.0) / (4f64.exp() - 1.0),
            Shape::SCurve => 0.5 - 0.5 * (std::f64::consts::PI * x).cos(),
            Shape::Swell | Shape::Dip => (std::f64::consts::PI * x).sin(),
            Shape::Step => 1.0,
            Shape::Lfo(l) => {
                let p = if period_frac > 0.0 {
                    (x / period_frac).fract()
                } else {
                    0.0
                };
                match l {
                    LfoShape::Sine => 0.5 - 0.5 * (std::f64::consts::TAU * p).cos(),
                    LfoShape::Triangle => 1.0 - (2.0 * p - 1.0).abs(),
                    LfoShape::SawDown => 1.0 - p,
                    LfoShape::SawUp => p,
                    LfoShape::Square => {
                        if p < 0.5 {
                            1.0
                        } else {
                            0.0
                        }
                    }
                }
            }
        }
    }
}

/// 点を置く間隔の目安(16 分 = 240 tick)。LFO は周期を 16 に分ける
const STEP: u64 = PPQ / 4;
/// 1 回で作る点の上限(長い区間・速い LFO でもレーンが巨大にならないように)
pub const MAX_POINTS: usize = 1024;

/// 区間 `[start, start + len)` を形で埋める点の列(区間の頭と終わりの点を含む)。
/// `period` は LFO の周期(tick)。終わりの点は、LFO なら周期の頭の値、それ以外は `to`
/// (スウェル・ディップは `from`)。
pub fn shape_points(
    start: u64,
    len: u64,
    from: f64,
    to: f64,
    shape: Shape,
    period: u64,
) -> Vec<AutomationPoint> {
    let len = len.max(1);
    let value = |a: f64| from + (to - from) * a;
    let pt = |tick: u64, v: f64, curve: Curve| AutomationPoint {
        tick: Tick(tick),
        value: v,
        curve,
    };
    match shape {
        Shape::Step => vec![pt(start, to, Curve::Hold), pt(start + len, to, Curve::Hold)],
        Shape::Linear => vec![
            pt(start, from, Curve::Linear),
            pt(start + len, to, Curve::Linear),
        ],
        Shape::Lfo(LfoShape::Square) => {
            // 保持の点で半周期ごとに切り替える
            let half = (period.max(2) / 2).max(1);
            let n = ((len / half) as usize).min(MAX_POINTS);
            let mut out: Vec<AutomationPoint> = (0..n)
                .map(|i| {
                    let t = start + i as u64 * half;
                    pt(t, if i % 2 == 0 { to } else { from }, Curve::Hold)
                })
                .collect();
            out.push(pt(start + len, from, Curve::Hold));
            out
        }
        Shape::Lfo(LfoShape::SawDown) | Shape::Lfo(LfoShape::SawUp) => {
            // 周期の頭で跳び、周期の中は直線(点は周期ごとに 2 つ)
            let period = period.max(1);
            let (head, tail) = if shape == Shape::Lfo(LfoShape::SawDown) {
                (to, from)
            } else {
                (from, to)
            };
            let mut out = Vec::new();
            let mut t = start;
            while t < start + len && out.len() + 2 <= MAX_POINTS {
                out.push(pt(t, head, Curve::Linear));
                let end = (t + period).min(start + len);
                // 次の周期の頭の直前まで直線で戻す(1 tick 手前)
                out.push(pt(end.saturating_sub(1).max(t), tail, Curve::Linear));
                t += period;
            }
            out
        }
        _ => {
            let period_frac = period as f64 / len as f64;
            let step = match shape {
                Shape::Lfo(_) => (period / 16).max(1),
                _ => STEP,
            };
            let n = ((len / step) as usize).clamp(2, MAX_POINTS - 1);
            let mut out: Vec<AutomationPoint> = (0..=n)
                .map(|i| {
                    let x = i as f64 / n as f64;
                    let tick = start + (len as f64 * x).round() as u64;
                    let a = shape.amount(x, period_frac);
                    pt(tick, value(a), Curve::Linear)
                })
                .collect();
            out.dedup_by_key(|p| p.tick);
            out
        }
    }
}

/// 既存の点の列に、区間 `[start, end]` を差し替える形で新しい点を入れる(区間の外の点は残す)。
/// 区間の外の続きがなめらかにつながるよう、区間の直前・直後の値はそのまま(新しい点の最初と最後が境目になる)。
pub fn merge_points(
    existing: &[AutomationPoint],
    new_points: Vec<AutomationPoint>,
    start: u64,
    end: u64,
) -> Vec<AutomationPoint> {
    let mut out: Vec<AutomationPoint> = existing
        .iter()
        .filter(|p| p.tick.0 < start || p.tick.0 > end)
        .cloned()
        .collect();
    out.extend(new_points);
    out.sort_by_key(|p| p.tick);
    out.dedup_by_key(|p| p.tick);
    out
}

/// 「小節:拍」(1 始まり。拍は小数も可。例 "9"・"9:3"・"12:2.5")を tick にする。拍の長さは拍子の分母で決まる
pub fn position_to_tick(project: &Project, pos: &str) -> Result<u64, String> {
    let (bar_s, beat_s) = match pos.split_once(':') {
        Some((b, t)) => (b.trim(), Some(t.trim())),
        None => (pos.trim(), None),
    };
    let bar: u32 = bar_s
        .parse()
        .map_err(|_| format!("位置「{pos}」の小節が数ではありません(例 \"9\"・\"9:3\")"))?;
    let beat: f64 = match beat_s {
        Some(s) => s
            .parse()
            .map_err(|_| format!("位置「{pos}」の拍が数ではありません"))?,
        None => 1.0,
    };
    if bar == 0 || beat < 1.0 {
        return Err(format!("位置「{pos}」: 小節・拍は 1 から数えます"));
    }
    let (bar_start, bar_len) =
        crate::arrange::bar_range(project, bar, 1).ok_or("小節が求められません")?;
    // 拍の長さ = 小節長 / 分子(拍子の分母の音符 1 つ分)
    let sig = project
        .time_sig_map
        .iter()
        .filter(|s| s.tick.0 <= bar_start)
        .max_by_key(|s| s.tick)
        .map(|s| (s.num.max(1), s.den.max(1)))
        .unwrap_or((4, 4));
    let beat_len = PPQ as f64 * 4.0 / sig.1 as f64;
    let offset = ((beat - 1.0) * beat_len).round() as u64;
    if offset >= bar_len {
        return Err(format!(
            "位置「{pos}」: {bar} 小節目は {} 拍までです",
            sig.0
        ));
    }
    Ok(bar_start + offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(points: &[AutomationPoint]) -> Vec<(u64, f64)> {
        points.iter().map(|p| (p.tick.0, p.value)).collect()
    }

    #[test]
    fn curves_start_and_end_at_the_right_values() {
        for s in ["linear", "exp", "log", "s_curve"] {
            let p = shape_points(0, 3840 * 8, 200.0, 8000.0, Shape::parse(s).unwrap(), 0);
            assert_eq!(p.first().unwrap().value, 200.0, "{s}");
            assert!((p.last().unwrap().value - 8000.0).abs() < 1e-6, "{s}");
            assert_eq!(p.last().unwrap().tick.0, 3840 * 8);
            // 単調に上がる
            assert!(p.windows(2).all(|w| w[1].value >= w[0].value - 1e-9), "{s}");
        }
        // 指数は前半がゆっくり、対数は前半が急
        let exp = shape_points(0, 3840, 0.0, 1.0, Shape::Exp, 0);
        let log = shape_points(0, 3840, 0.0, 1.0, Shape::Log, 0);
        let mid = |p: &[AutomationPoint]| p.iter().find(|x| x.tick.0 >= 1920).unwrap().value;
        assert!(
            mid(&exp) < 0.2 && mid(&log) > 0.8,
            "{} {}",
            mid(&exp),
            mid(&log)
        );
    }

    #[test]
    fn swell_returns_and_pump_repeats_every_beat() {
        let s = shape_points(0, 3840, -20.0, 0.0, Shape::Swell, 0);
        assert_eq!(s.first().unwrap().value, -20.0);
        assert!((s.last().unwrap().value + 20.0).abs() < 1e-6);
        assert!(s.iter().any(|p| p.value > -0.1));
        // 拍ごとのポンピング: 4 拍で 8 点、各拍の頭で沈む
        let p = shape_points(0, 3840, 0.0, -8.0, Shape::Lfo(LfoShape::SawDown), 960);
        assert_eq!(p.len(), 8);
        assert_eq!(v(&p)[0], (0, -8.0));
        assert_eq!(v(&p)[2], (960, -8.0));
        assert_eq!(v(&p)[1], (959, 0.0));
    }

    #[test]
    fn merging_keeps_points_outside_the_range() {
        let old = vec![
            AutomationPoint {
                tick: Tick(0),
                value: 1.0,
                curve: Curve::Linear,
            },
            AutomationPoint {
                tick: Tick(4000),
                value: 2.0,
                curve: Curve::Linear,
            },
            AutomationPoint {
                tick: Tick(9000),
                value: 3.0,
                curve: Curve::Linear,
            },
        ];
        let new = shape_points(3840, 3840, 10.0, 20.0, Shape::Linear, 0);
        let m = merge_points(&old, new, 3840, 7680);
        assert_eq!(
            v(&m),
            vec![(0, 1.0), (3840, 10.0), (7680, 20.0), (9000, 3.0)]
        );
    }

    #[test]
    fn positions_follow_the_time_signature() {
        let mut p = Project::new("t");
        assert_eq!(position_to_tick(&p, "1").unwrap(), 0);
        assert_eq!(position_to_tick(&p, "2:3").unwrap(), 3840 + 1920);
        assert_eq!(position_to_tick(&p, "1:1.5").unwrap(), 480);
        assert!(position_to_tick(&p, "1:5").is_err());
        assert!(position_to_tick(&p, "0").is_err());
        // 3 小節目から 6/8: 拍 = 8 分
        p.time_sig_map = vec![
            crate::time::TimeSigEvent {
                tick: Tick(0),
                num: 4,
                den: 4,
            },
            crate::time::TimeSigEvent {
                tick: Tick(7680),
                num: 6,
                den: 8,
            },
        ];
        assert_eq!(position_to_tick(&p, "3:4").unwrap(), 7680 + 3 * 480);
        assert_eq!(position_to_tick(&p, "4").unwrap(), 7680 + 2880);
    }
}
