//! オートメーション。トラック単位で、ターゲットは [`ParamPath`]。

use super::ParamPath;
use crate::time::Tick;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Curve {
    #[default]
    Linear,
    Hold,
    Exponential,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct AutomationPoint {
    pub tick: Tick,
    pub value: f64,
    #[serde(default, skip_serializing_if = "is_default_curve")]
    pub curve: Curve,
}

fn is_default_curve(c: &Curve) -> bool {
    *c == Curve::Linear
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct AutomationLane {
    pub target: ParamPath,
    /// tick 昇順を保つ
    pub points: Vec<AutomationPoint>,
}

impl AutomationLane {
    /// 区分線形補間で値を求める(Hold は前の値を保持)。
    pub fn value_at(&self, tick: Tick) -> Option<f64> {
        let pts = &self.points;
        if pts.is_empty() {
            return None;
        }
        let idx = pts.partition_point(|p| p.tick <= tick);
        if idx == 0 {
            return Some(pts[0].value);
        }
        let a = &pts[idx - 1];
        let Some(b) = pts.get(idx) else {
            return Some(a.value);
        };
        let span = (b.tick.0 - a.tick.0) as f64;
        if span == 0.0 {
            return Some(a.value);
        }
        let t = (tick.0 - a.tick.0) as f64 / span;
        Some(match a.curve {
            Curve::Hold => a.value,
            Curve::Linear => a.value + (b.value - a.value) * t,
            Curve::Exponential => a.value + (b.value - a.value) * t * t,
        })
    }
}
