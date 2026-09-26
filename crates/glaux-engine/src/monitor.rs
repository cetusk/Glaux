//! 聴き方の切り替え(モニター)と、ステレオの見え方(相関・ゴニオメーター)。
//!
//! - モニターの切り替え(モノ・サイド・左右入れ替え・クロスフィード)は、出力デバイスへ送る直前だけに掛ける。
//!   書き出し・フリーズ・Godot はそれぞれ自分の [`crate::render::Shared`] を持つので、ここの設定は入らない
//! - 相関とゴニオメーターの点は、マスターの音量の後(クリップ防止の前)の音で測る
//!
//! オーディオスレッドはアトミックに書くだけ(アロケーション・ロックなし)。

use std::sync::atomic::{AtomicU32, Ordering};

/// ゴニオメーターに残す点の数(2 サンプルに 1 点。48kHz で約 43ms 分)
pub const SCOPE_LEN: usize = 1024;
/// ゴニオメーターの間引き(何サンプルに 1 点残すか)
const SCOPE_DECIM: u32 = 2;
/// 相関の平均の時定数(秒)
const CORR_SECS: f32 = 0.3;
/// クロスフィード: 反対側へ回す低域の境目(Hz)と、低域での直接音と回り込みの差(dB)。bs2b の既定に近い値
const XFEED_HZ: f32 = 700.0;
const XFEED_DB: f32 = 4.5;

/// 聴き方
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MonitorMode {
    /// そのまま
    Stereo,
    /// 左右を足して両耳へ(モノラルで再生されたときの確認。位相の打ち消しが分かる)
    Mono,
    /// 左右の差(サイド)だけ(広がり・リバーブ・位相のずれの確認)
    Side,
    /// 左右を入れ替える(片耳の聞こえ方の癖を打ち消して、偏りを確かめる)
    Swap,
}

impl MonitorMode {
    pub fn name(self) -> &'static str {
        match self {
            MonitorMode::Stereo => "stereo",
            MonitorMode::Mono => "mono",
            MonitorMode::Side => "side",
            MonitorMode::Swap => "swap",
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        Some(match s {
            "stereo" => MonitorMode::Stereo,
            "mono" => MonitorMode::Mono,
            "side" => MonitorMode::Side,
            "swap" => MonitorMode::Swap,
            _ => return None,
        })
    }

    fn bits(self) -> u32 {
        match self {
            MonitorMode::Stereo => 0,
            MonitorMode::Mono => 1,
            MonitorMode::Side => 2,
            MonitorMode::Swap => 3,
        }
    }

    fn from_bits(b: u32) -> Self {
        match b & 3 {
            1 => MonitorMode::Mono,
            2 => MonitorMode::Side,
            3 => MonitorMode::Swap,
            _ => MonitorMode::Stereo,
        }
    }
}

const XFEED_BIT: u32 = 1 << 8;

/// UI とオーディオスレッドで共有する部分
pub struct MonitorShared {
    /// 下位 2 ビット = 聴き方、8 ビット目 = クロスフィード
    settings: AtomicU32,
    /// 相関(f32 のビット列。NaN = ほぼ無音で測れない)
    correlation: AtomicU32,
    /// ゴニオメーターの点(左, 右 の f32 ビット列を交互に)。リングバッファ
    points: [AtomicU32; SCOPE_LEN * 2],
    /// 次に書く点の位置
    write: AtomicU32,
}

impl Default for MonitorShared {
    fn default() -> Self {
        MonitorShared {
            settings: AtomicU32::new(0),
            correlation: AtomicU32::new(f32::NAN.to_bits()),
            points: std::array::from_fn(|_| AtomicU32::new(0)),
            write: AtomicU32::new(0),
        }
    }
}

impl MonitorShared {
    pub fn set(&self, mode: MonitorMode, crossfeed: bool) {
        let bits = mode.bits() | if crossfeed { XFEED_BIT } else { 0 };
        self.settings.store(bits, Ordering::Release);
    }

    pub fn get(&self) -> (MonitorMode, bool) {
        let b = self.settings.load(Ordering::Acquire);
        (MonitorMode::from_bits(b), b & XFEED_BIT != 0)
    }

    /// 相関(-1 = 逆相、0 = 無関係、+1 = モノラル)。ほぼ無音なら None
    pub fn correlation(&self) -> Option<f32> {
        let v = f32::from_bits(self.correlation.load(Ordering::Relaxed));
        v.is_finite().then_some(v)
    }

    /// ゴニオメーターの点(古い順の (左, 右))。書き込み中の点が混ざることがあるが、表示には問題ない
    pub fn scope_points(&self) -> Vec<(f32, f32)> {
        let w = self.write.load(Ordering::Acquire) as usize;
        (0..SCOPE_LEN)
            .map(|k| {
                let i = (w + k) % SCOPE_LEN;
                (
                    f32::from_bits(self.points[i * 2].load(Ordering::Relaxed)),
                    f32::from_bits(self.points[i * 2 + 1].load(Ordering::Relaxed)),
                )
            })
            .collect()
    }
}

/// オーディオスレッド側の状態
#[derive(Default)]
pub struct MonitorState {
    lr: f32,
    ll: f32,
    rr: f32,
    decim: u32,
    write: usize,
    /// クロスフィードのローパス(左, 右)
    lp: (f32, f32),
}

impl MonitorState {
    /// マスターの音を 1 サンプル測る(相関とゴニオメーター)
    pub fn measure(&mut self, shared: &MonitorShared, l: f32, r: f32, sr: f32) {
        let a = 1.0 - (-1.0 / (CORR_SECS * sr.max(1.0))).exp();
        self.lr += (l * r - self.lr) * a;
        self.ll += (l * l - self.ll) * a;
        self.rr += (r * r - self.rr) * a;
        self.decim += 1;
        if self.decim >= SCOPE_DECIM {
            self.decim = 0;
            shared.points[self.write * 2].store(l.to_bits(), Ordering::Relaxed);
            shared.points[self.write * 2 + 1].store(r.to_bits(), Ordering::Relaxed);
            self.write = (self.write + 1) % SCOPE_LEN;
        }
    }

    /// ブロックの終わりに、相関とゴニオメーターの書き込み位置を公開する
    pub fn publish(&self, shared: &MonitorShared) {
        let energy = (self.ll * self.rr).sqrt();
        // -70 dBFS 程度より小さければ測れない扱い
        let corr = if energy > 1e-7 {
            (self.lr / energy).clamp(-1.0, 1.0)
        } else {
            f32::NAN
        };
        shared.correlation.store(corr.to_bits(), Ordering::Relaxed);
        shared.write.store(self.write as u32, Ordering::Release);
    }

    /// 出力デバイスへ送る直前の聴き方の切り替え
    pub fn apply(
        &mut self,
        mode: MonitorMode,
        crossfeed: bool,
        l: f32,
        r: f32,
        sr: f32,
    ) -> (f32, f32) {
        let (l, r) = match mode {
            MonitorMode::Stereo => (l, r),
            MonitorMode::Mono => {
                let m = 0.5 * (l + r);
                (m, m)
            }
            MonitorMode::Side => {
                let s = 0.5 * (l - r);
                (s, s)
            }
            MonitorMode::Swap => (r, l),
        };
        if !crossfeed {
            return (l, r);
        }
        // 反対側の低域を回り込ませ(ヘッドホンでの極端な左右の分離をやわらげる)、
        // 直接音の高域を少し持ち上げて、モノラルの音の高さの釣り合い(周波数特性)は平らに保つ。
        // 低域: 直接 a・回り込み c(c / a = -4.5dB、a + c = 1)。高域: 直接 a + c = 1
        let c = 1.0 / (1.0 + 10.0_f32.powf(XFEED_DB / 20.0));
        let a = 1.0 - c;
        let k = 1.0 - (-std::f32::consts::TAU * XFEED_HZ / sr.max(1.0)).exp();
        self.lp.0 += (l - self.lp.0) * k;
        self.lp.1 += (r - self.lp.1) * k;
        let (hl, hr) = (l - self.lp.0, r - self.lp.1);
        (
            a * l + c * hl + c * self.lp.1,
            a * r + c * hr + c * self.lp.0,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corr_of(f: impl Fn(usize) -> (f32, f32)) -> Option<f32> {
        let shared = MonitorShared::default();
        let mut st = MonitorState::default();
        for i in 0..48_000 {
            let (l, r) = f(i);
            st.measure(&shared, l, r, 48_000.0);
        }
        st.publish(&shared);
        shared.correlation()
    }

    #[test]
    fn correlation_reads_mono_wide_and_inverted() {
        let s = |i: usize| (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5;
        let c = |i: usize| (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).cos() * 0.5;
        assert!(corr_of(|i| (s(i), s(i))).unwrap() > 0.99, "モノラルは +1");
        assert!(corr_of(|i| (s(i), -s(i))).unwrap() < -0.99, "逆相は -1");
        assert!(
            corr_of(|i| (s(i), c(i))).unwrap().abs() < 0.05,
            "90° ずれは 0"
        );
        assert!(corr_of(|_| (0.0, 0.0)).is_none(), "無音は測れない");
    }

    #[test]
    fn scope_keeps_recent_points_in_order() {
        let shared = MonitorShared::default();
        let mut st = MonitorState::default();
        for i in 0..(SCOPE_LEN * 3) {
            st.measure(&shared, i as f32, -(i as f32), 48_000.0);
        }
        st.publish(&shared);
        let pts = shared.scope_points();
        assert_eq!(pts.len(), SCOPE_LEN);
        assert!(pts.windows(2).all(|w| w[1].0 > w[0].0), "古い順");
        assert_eq!(pts.last().unwrap().0, (SCOPE_LEN * 3 - 1) as f32);
    }

    #[test]
    fn monitor_modes_and_flat_crossfeed() {
        let mut st = MonitorState::default();
        assert_eq!(
            st.apply(MonitorMode::Mono, false, 1.0, 0.0, 48_000.0),
            (0.5, 0.5)
        );
        assert_eq!(
            st.apply(MonitorMode::Side, false, 1.0, 1.0, 48_000.0),
            (0.0, 0.0)
        );
        assert_eq!(
            st.apply(MonitorMode::Swap, false, 1.0, 0.0, 48_000.0),
            (0.0, 1.0)
        );
        // クロスフィード: モノラルの音はそのまま、片側だけの低音は反対側へ -4.5dB 回り込む
        let mut st = MonitorState::default();
        for i in 0..4800 {
            let x = (i as f32 * 100.0 * std::f32::consts::TAU / 48_000.0).sin();
            let (l, r) = st.apply(MonitorMode::Stereo, true, x, x, 48_000.0);
            assert!((l - x).abs() < 1e-4 && (r - x).abs() < 1e-4);
        }
        let mut st = MonitorState::default();
        let (mut pl, mut pr) = (0.0f32, 0.0f32);
        for i in 0..9600 {
            let x = (i as f32 * 60.0 * std::f32::consts::TAU / 48_000.0).sin();
            let (l, r) = st.apply(MonitorMode::Stereo, true, x, 0.0, 48_000.0);
            if i > 4800 {
                pl = pl.max(l.abs());
                pr = pr.max(r.abs());
            }
        }
        let diff = 20.0 * (pl / pr).log10();
        assert!((diff - XFEED_DB).abs() < 0.5, "低音の左右差: {diff:.2} dB");
    }
}
