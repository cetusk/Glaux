//! 時間表現。
//!
//! プロジェクト内の位置・長さはすべて [`Tick`](整数、PPQ=960)。
//! 秒への変換は [`TempoMap`] が担い、音声クリップの再生位置計算やエンジンで使う。
//! `bar:beat:tick` 表記は UI 側で計算するだけで、ファイルには書かない。

use serde::{Deserialize, Serialize};
use std::ops::{Add, Sub};

/// 四分音符あたりの Tick 数。
pub const PPQ: u64 = 960;

/// コマンドで受け付ける位置・長さの上限(4/4 で 10 万小節。120 BPM で約 55 時間)。
/// これを超える値は [`crate::Project::apply`] が拒否する(桁あふれや、終端が遠すぎて
/// オフラインレンダがメモリを使い果たすのを防ぐ)
pub const MAX_TICK: Tick = Tick(PPQ * 4 * 100_000);

#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Tick(pub u64);

impl Tick {
    pub const ZERO: Tick = Tick(0);

    pub fn from_beats(beats: f64) -> Tick {
        Tick((beats * PPQ as f64).round().max(0.0) as u64)
    }

    pub fn beats(self) -> f64 {
        self.0 as f64 / PPQ as f64
    }

    pub fn checked_add_signed(self, delta: i64) -> Option<Tick> {
        self.0.checked_add_signed(delta).map(Tick)
    }

    pub fn saturating_sub(self, other: Tick) -> Tick {
        Tick(self.0.saturating_sub(other.0))
    }
}

// 足し算・引き算は飽和させる(不正な入力で panic してセッションを止めないため。
// 通常の値は MAX_TICK で検査済みなので、飽和するのは壊れた入力のときだけ)
impl Add for Tick {
    type Output = Tick;
    fn add(self, rhs: Tick) -> Tick {
        Tick(self.0.saturating_add(rhs.0))
    }
}

impl Sub for Tick {
    type Output = Tick;
    fn sub(self, rhs: Tick) -> Tick {
        Tick(self.0.saturating_sub(rhs.0))
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TimeError {
    #[error("tempo map must start at tick 0")]
    TempoMapMustStartAtZero,
    #[error("tempo map ticks must be strictly increasing")]
    TempoMapNotSorted,
    #[error("bpm must be positive and finite (got {0})")]
    InvalidBpm(f64),
    #[error("time signature denominator must be a power of two (got {0})")]
    InvalidTimeSig(u8),
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct TempoEvent {
    pub tick: Tick,
    pub bpm: f64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct TimeSigEvent {
    pub tick: Tick,
    pub num: u8,
    pub den: u8,
}

/// テンポマップ。イベントは tick 昇順で先頭は tick 0 であることを保証する。
/// JSON 上はイベント配列そのもの。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(try_from = "Vec<TempoEvent>", into = "Vec<TempoEvent>")]
pub struct TempoMap {
    events: Vec<TempoEvent>,
}

impl Default for TempoMap {
    fn default() -> Self {
        TempoMap {
            events: vec![TempoEvent {
                tick: Tick::ZERO,
                bpm: 120.0,
            }],
        }
    }
}

impl TempoMap {
    pub fn new(mut events: Vec<TempoEvent>) -> Result<Self, TimeError> {
        events.sort_by_key(|e| e.tick);
        match events.first() {
            Some(e) if e.tick == Tick::ZERO => {}
            _ => return Err(TimeError::TempoMapMustStartAtZero),
        }
        for w in events.windows(2) {
            if w[0].tick >= w[1].tick {
                return Err(TimeError::TempoMapNotSorted);
            }
        }
        for e in &events {
            if !(e.bpm.is_finite() && e.bpm > 0.0) {
                return Err(TimeError::InvalidBpm(e.bpm));
            }
        }
        Ok(TempoMap { events })
    }

    pub fn constant(bpm: f64) -> Result<Self, TimeError> {
        Self::new(vec![TempoEvent {
            tick: Tick::ZERO,
            bpm,
        }])
    }

    pub fn events(&self) -> &[TempoEvent] {
        &self.events
    }

    pub fn bpm_at(&self, tick: Tick) -> f64 {
        let idx = self.events.partition_point(|e| e.tick <= tick);
        self.events[idx.saturating_sub(1)].bpm
    }

    fn seconds_per_tick(bpm: f64) -> f64 {
        60.0 / (bpm * PPQ as f64)
    }

    pub fn tick_to_seconds(&self, tick: Tick) -> f64 {
        let mut secs = 0.0;
        for (i, ev) in self.events.iter().enumerate() {
            if tick <= ev.tick {
                break;
            }
            let end = match self.events.get(i + 1) {
                Some(next) if next.tick < tick => next.tick,
                _ => tick,
            };
            secs += (end.0 - ev.tick.0) as f64 * Self::seconds_per_tick(ev.bpm);
        }
        secs
    }

    /// [`seconds_to_tick`](Self::seconds_to_tick) の小数版(丸めない)。
    pub fn seconds_to_tick_f64(&self, seconds: f64) -> f64 {
        let mut remaining = seconds.max(0.0);
        for (i, ev) in self.events.iter().enumerate() {
            let spt = Self::seconds_per_tick(ev.bpm);
            match self.events.get(i + 1) {
                Some(next) => {
                    let seg_secs = (next.tick.0 - ev.tick.0) as f64 * spt;
                    if remaining < seg_secs {
                        return ev.tick.0 as f64 + remaining / spt;
                    }
                    remaining -= seg_secs;
                }
                None => return ev.tick.0 as f64 + remaining / spt,
            }
        }
        0.0
    }

    pub fn seconds_to_tick(&self, seconds: f64) -> Tick {
        let mut remaining = seconds.max(0.0);
        for (i, ev) in self.events.iter().enumerate() {
            let spt = Self::seconds_per_tick(ev.bpm);
            match self.events.get(i + 1) {
                Some(next) => {
                    let seg_secs = (next.tick.0 - ev.tick.0) as f64 * spt;
                    if remaining < seg_secs {
                        return Tick(ev.tick.0 + (remaining / spt).round() as u64);
                    }
                    remaining -= seg_secs;
                }
                None => return Tick(ev.tick.0 + (remaining / spt).round() as u64),
            }
        }
        Tick::ZERO
    }
}

impl TryFrom<Vec<TempoEvent>> for TempoMap {
    type Error = TimeError;
    fn try_from(v: Vec<TempoEvent>) -> Result<Self, TimeError> {
        TempoMap::new(v)
    }
}

impl From<TempoMap> for Vec<TempoEvent> {
    fn from(m: TempoMap) -> Self {
        m.events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_tempo_conversion() {
        let m = TempoMap::constant(120.0).unwrap();
        // 120bpm: 1 beat = 0.5s, 1 bar(4/4) = 3840 ticks = 2s
        assert!((m.tick_to_seconds(Tick(3840)) - 2.0).abs() < 1e-9);
        assert_eq!(m.seconds_to_tick(2.0), Tick(3840));
    }

    #[test]
    fn tempo_change_conversion() {
        let m = TempoMap::new(vec![
            TempoEvent {
                tick: Tick(0),
                bpm: 120.0,
            },
            TempoEvent {
                tick: Tick(3840),
                bpm: 60.0,
            },
        ])
        .unwrap();
        // 2s @120 + 1 beat @60 (=1s) = 3s
        assert!((m.tick_to_seconds(Tick(3840 + 960)) - 3.0).abs() < 1e-9);
        assert_eq!(m.seconds_to_tick(3.0), Tick(3840 + 960));
        assert_eq!(m.bpm_at(Tick(100)), 120.0);
        assert_eq!(m.bpm_at(Tick(3840)), 60.0);
    }

    #[test]
    fn rejects_bad_maps() {
        assert!(TempoMap::new(vec![TempoEvent {
            tick: Tick(10),
            bpm: 120.0
        }])
        .is_err());
        assert!(TempoMap::new(vec![TempoEvent {
            tick: Tick(0),
            bpm: -1.0
        }])
        .is_err());
    }
}
