//! ステレオの幅(M/S)。
//!
//! - 幅: ミッド(左右の和)とサイド(左右の差)に分け、サイドの量を変える(0 = モノラル、1 = そのまま、2 = 広く)
//! - 低域のモノ化: サイドだけに 4 次のハイパス(LR4)を掛け、指定より低い音を中央に集める
//!   (キック・ベースの位置が定まり、モノラルで再生しても低音が痩せない)
//! - 広げる(デコリレーション): ミッドを velvet noise(まばらな ±1 の列)で畳み込んだ音をサイドに足し、
//!   モノラルの音にも左右の違いを作る。サイドに足すだけなので、モノラルにすると消えて元の音に戻る。
//!   打点(トランジェント)は畳み込むとにじむので、打点の間は足す量を減らす
//!
//! 状態は固定長の配列だけ(アロケーションなし)。

use crate::effects::{time_coef, SvfCoeffs, SvfState};

/// velvet noise の長さの上限(サンプル。192kHz で約 21ms)
const VELVET_CAP: usize = 4096;
/// velvet noise のインパルスの数
const VELVET_TAPS: usize = 24;
/// 長さ(秒)
const VELVET_SECS: f32 = 0.02;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WidthParams {
    /// サイドの量(0..2)
    pub width: f32,
    /// 低域のモノ化(None で掛けない)
    pub mono_hp: Option<SvfCoeffs>,
    pub mono_below_hz: f32,
    /// デコリレーションの量(0..1)
    pub decorrelate: f32,
    /// velvet noise のインパルスの位置(サンプル)と重み(符号・減衰込み)
    taps: [(u16, f32); VELVET_TAPS],
    /// 打点の検出の係数(速い立ち上がり・遅い立ち上がり・戻り)
    att_fast: f32,
    att_slow: f32,
    rel: f32,
    pub sample_rate: f32,
}

impl WidthParams {
    pub fn new(width: f32, mono_below_hz: f32, decorrelate: f32, sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        let len = ((VELVET_SECS * sr) as usize).clamp(VELVET_TAPS * 2, VELVET_CAP - 1);
        // 位置・符号は決まった乱数で(毎回同じ音になるように)。区間ごとに 1 つ置く
        let mut rng: u32 = 0x2545_F491;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            rng
        };
        let seg = len / VELVET_TAPS;
        let mut taps = [(0u16, 0.0f32); VELVET_TAPS];
        let mut norm = 0.0f32;
        for (k, t) in taps.iter_mut().enumerate() {
            let pos = k * seg + (next() as usize % seg.max(1));
            let sign = if next() & 1 == 0 { 1.0 } else { -1.0 };
            // 後ろほど小さく(約 -30dB まで)
            let w = sign * (-3.4 * k as f32 / VELVET_TAPS as f32).exp();
            *t = (pos.max(1) as u16, w);
            norm += w * w;
        }
        // エネルギーを 1 に
        let n = norm.sqrt().max(1e-6);
        for t in &mut taps {
            t.1 /= n;
        }
        let hz = mono_below_hz.clamp(20.0, 500.0);
        WidthParams {
            width: width.clamp(0.0, 2.0),
            mono_hp: (hz > 20.0).then(|| SvfCoeffs::high_pass(sr, hz)),
            mono_below_hz: hz,
            decorrelate: decorrelate.clamp(0.0, 1.0),
            taps,
            att_fast: time_coef(0.5, sr),
            att_slow: time_coef(20.0, sr),
            rel: time_coef(60.0, sr),
            sample_rate: sr,
        }
    }
}

#[derive(Clone, Debug)]
pub struct WidthState {
    hp: [SvfState; 2],
    ring: [f32; VELVET_CAP],
    pos: usize,
    fast: f32,
    slow: f32,
}

impl Default for WidthState {
    fn default() -> Self {
        WidthState {
            hp: Default::default(),
            ring: [0.0; VELVET_CAP],
            pos: 0,
            fast: 0.0,
            slow: 0.0,
        }
    }
}

impl WidthState {
    #[inline]
    pub fn process(&mut self, p: &WidthParams, l: f32, r: f32) -> (f32, f32) {
        let m = 0.5 * (l + r);
        let mut s = 0.5 * (l - r) * p.width;
        if p.decorrelate > 0.0 {
            self.ring[self.pos] = m;
            let mut d = 0.0;
            for &(off, w) in &p.taps {
                d += w * self.ring[(self.pos + VELVET_CAP - off as usize) % VELVET_CAP];
            }
            self.pos = (self.pos + 1) % VELVET_CAP;
            // 打点の間は足す量を減らす(速い包絡が、それを立ち上がりだけ遅く追う包絡を大きく上回る間。
            // 遅い包絡を波形から直接作ると、波形の山と谷の揺れを打点と取り違える)
            let x = m.abs();
            let c = if x > self.fast { p.att_fast } else { p.rel };
            self.fast = c * self.fast + (1.0 - c) * x;
            self.slow = if self.fast > self.slow {
                p.att_slow * self.slow + (1.0 - p.att_slow) * self.fast
            } else {
                self.fast
            };
            let transient = ((self.fast / self.slow.max(1e-6) - 1.0) * 2.0).clamp(0.0, 1.0);
            s += d * p.decorrelate * (1.0 - transient);
        }
        if let Some(hp) = &p.mono_hp {
            s = self.hp[0].process(hp, 1.0, s);
            s = self.hp[1].process(hp, 1.0, s);
        }
        (m + s, m - s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(p: &WidthParams, input: impl Fn(usize) -> (f32, f32), n: usize) -> Vec<(f32, f32)> {
        let mut st = WidthState::default();
        (0..n)
            .map(|i| {
                let (l, r) = input(i);
                st.process(p, l, r)
            })
            .collect()
    }

    fn sine(f: f32, i: usize) -> f32 {
        (i as f32 * f * std::f32::consts::TAU / 48_000.0).sin()
    }

    fn corr(v: &[(f32, f32)]) -> f32 {
        let (mut lr, mut ll, mut rr) = (0.0, 0.0, 0.0);
        for (l, r) in v {
            lr += l * r;
            ll += l * l;
            rr += r * r;
        }
        lr / (ll * rr).sqrt().max(1e-9)
    }

    #[test]
    fn width_scales_the_side() {
        let input = |i: usize| (sine(440.0, i), sine(660.0, i));
        let mono = run(&WidthParams::new(0.0, 20.0, 0.0, 48_000.0), input, 4800);
        assert!(mono.iter().all(|(l, r)| (l - r).abs() < 1e-6));
        let same = run(&WidthParams::new(1.0, 20.0, 0.0, 48_000.0), input, 4800);
        for (i, (l, r)) in same.iter().enumerate() {
            let (a, b) = input(i);
            assert!((l - a).abs() < 1e-5 && (r - b).abs() < 1e-5);
        }
    }

    #[test]
    fn low_side_is_made_mono() {
        // 左右で逆相の 60Hz(サイドだけ)は消え、2kHz のサイドは残る
        let p = WidthParams::new(1.0, 150.0, 0.0, 48_000.0);
        let low = run(&p, |i| (sine(60.0, i), -sine(60.0, i)), 24_000);
        let hi = run(&p, |i| (sine(2000.0, i), -sine(2000.0, i)), 24_000);
        let rms =
            |v: &[(f32, f32)]| (v.iter().map(|x| x.0 * x.0).sum::<f32>() / v.len() as f32).sqrt();
        assert!(rms(&low[12_000..]) < 0.1 * rms(&hi[12_000..]));
    }

    #[test]
    fn decorrelation_widens_mono_and_vanishes_in_mono() {
        // モノラルの音(和音)を広げる: 左右の相関が下がり、左右を足すと元の音に戻る
        let chord = |i: usize| {
            let x = (sine(220.0, i) + sine(277.0, i) + sine(330.0, i)) * 0.2;
            (x, x)
        };
        let out = run(&WidthParams::new(1.0, 20.0, 1.0, 48_000.0), chord, 24_000);
        let c = corr(&out[4800..]);
        assert!(c < 0.8, "相関 {c}");
        for (i, (l, r)) in out.iter().enumerate() {
            assert!((0.5 * (l + r) - chord(i).0).abs() < 1e-5);
        }
    }
}
