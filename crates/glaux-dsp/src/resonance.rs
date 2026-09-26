//! 共鳴抑制(耳につく共鳴 = スペクトルの細い出っ張りだけを、鳴っている間だけ下げる)。
//!
//! 短時間フーリエ変換(1024 点、1/4 ずつずらす、平方根 Hann 窓で分析・合成)で、時間でなめらかにした
//! (約 50ms)パワーと、それを対数周波数で幅 `width` オクターブ平均した包絡を求め、
//! 包絡より `threshold_db` 以上出ている分だけ(最大 `depth_db`)下げる。左右は同じだけ下げる(定位を保つ)。
//! 下げる量は、上げるときは速く・戻すときは `release_ms` でゆっくり。
//!
//! 遅れは 1 フレーム(1024 サンプル。48kHz で約 21ms)。原音も同じだけ遅らせて混ぜる(遅延補正に申告)。
//! バッファ・FFT の計画は状態を作るときに用意する(オーディオスレッドではアロケーションしない)。

use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::sync::Arc;

/// フレームの長さ
pub const FRAME: usize = 1024;
/// ずらす量
const HOP: usize = FRAME / 4;
const BINS: usize = FRAME / 2 + 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResonanceParams {
    pub depth_db: f32,
    pub threshold_db: f32,
    pub low_hz: f32,
    pub high_hz: f32,
    pub release_ms: f32,
    pub mix: f32,
    /// 包絡をなめらかにする幅(オクターブ)
    pub width: f32,
    pub sample_rate: f32,
    /// フレームごとの戻りの係数
    release_coef: f32,
}

impl ResonanceParams {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        depth_db: f32,
        threshold_db: f32,
        low_hz: f32,
        high_hz: f32,
        release_ms: f32,
        mix: f32,
        width: f32,
        sample_rate: f32,
    ) -> Self {
        let sr = sample_rate.max(1.0);
        let release_ms = release_ms.clamp(10.0, 1000.0);
        let hop_secs = HOP as f32 / sr;
        ResonanceParams {
            depth_db: depth_db.clamp(0.0, 18.0),
            threshold_db: threshold_db.clamp(0.0, 12.0),
            low_hz: low_hz.clamp(20.0, 20_000.0),
            high_hz: high_hz.clamp(20.0, 20_000.0),
            release_ms,
            mix: mix.clamp(0.0, 1.0),
            width: width.clamp(0.1, 2.0),
            sample_rate: sr,
            release_coef: (-hop_secs / (release_ms * 0.001)).exp(),
        }
    }

    pub fn latency(&self) -> u32 {
        FRAME as u32
    }
}

#[derive(Clone)]
pub struct ResonanceState {
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    /// 入力の輪(左右、FRAME)
    input: [Vec<f32>; 2],
    /// 出力の重ね足し(左右、FRAME)
    output: [Vec<f32>; 2],
    /// 原音の遅れ(左右、FRAME)
    dry: [Vec<f32>; 2],
    pos: usize,
    hop_count: usize,
    spec: [Vec<Complex<f32>>; 2],
    scratch: Vec<Complex<f32>>,
    /// 時間でなめらかにしたパワー(雑音のフレームごとの揺れを共鳴と取り違えないように)、
    /// その累積和(包絡用)、対数振幅、いま下げている量(dB)
    power: Vec<f32>,
    prefix: Vec<f64>,
    logmag: Vec<f32>,
    reduction: Vec<f32>,
}

impl Default for ResonanceState {
    fn default() -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FRAME);
        let ifft = planner.plan_fft_inverse(FRAME);
        let scratch = fft
            .get_inplace_scratch_len()
            .max(ifft.get_inplace_scratch_len());
        // 平方根 Hann(分析と合成で 2 回掛けると Hann。1/4 ずつずらして重ねると 2 倍になるので後で割る)
        let window = (0..FRAME)
            .map(|i| (0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / FRAME as f32).cos()).sqrt())
            .collect();
        ResonanceState {
            fft,
            ifft,
            window,
            input: [vec![0.0; FRAME], vec![0.0; FRAME]],
            output: [vec![0.0; FRAME], vec![0.0; FRAME]],
            dry: [vec![0.0; FRAME], vec![0.0; FRAME]],
            pos: 0,
            hop_count: 0,
            spec: [
                vec![Complex::new(0.0, 0.0); FRAME],
                vec![Complex::new(0.0, 0.0); FRAME],
            ],
            scratch: vec![Complex::new(0.0, 0.0); scratch],
            power: vec![0.0; BINS],
            prefix: vec![0.0; BINS + 1],
            logmag: vec![0.0; BINS],
            reduction: vec![0.0; BINS],
        }
    }
}

impl ResonanceState {
    pub fn reset(&mut self) {
        for c in 0..2 {
            self.input[c].fill(0.0);
            self.output[c].fill(0.0);
            self.dry[c].fill(0.0);
        }
        self.reduction.fill(0.0);
        self.power.fill(0.0);
        self.pos = 0;
        self.hop_count = 0;
    }

    /// 1 サンプル(左右)
    #[inline]
    pub fn process(&mut self, p: &ResonanceParams, l: f32, r: f32) -> (f32, f32) {
        let i = self.pos;
        // 出力(1 フレーム遅れ): 重ね足しの先頭と、同じだけ遅らせた原音
        let (wl, wr) = (self.output[0][i], self.output[1][i]);
        let (dl, dr) = (self.dry[0][i], self.dry[1][i]);
        self.output[0][i] = 0.0;
        self.output[1][i] = 0.0;
        self.dry[0][i] = l;
        self.dry[1][i] = r;
        self.input[0][i] = l;
        self.input[1][i] = r;
        self.pos = (i + 1) % FRAME;
        self.hop_count += 1;
        if self.hop_count >= HOP {
            self.hop_count = 0;
            self.frame(p);
        }
        (
            dl * (1.0 - p.mix) + wl * p.mix,
            dr * (1.0 - p.mix) + wr * p.mix,
        )
    }

    /// 直近 FRAME サンプルを処理して、出力の重ね足しに足す
    fn frame(&mut self, p: &ResonanceParams) {
        let bin_hz = p.sample_rate / FRAME as f32;
        // 分析(輪の中は古い順に pos から)
        for c in 0..2 {
            for k in 0..FRAME {
                let v = self.input[c][(self.pos + k) % FRAME] * self.window[k];
                self.spec[c][k] = Complex::new(v, 0.0);
            }
            self.fft
                .process_with_scratch(&mut self.spec[c], &mut self.scratch);
        }
        // 左右の大きい方のパワーを時間でなめらかにし(約 50ms)、その対数と累積和
        let smooth = (-(HOP as f32 / p.sample_rate) / 0.05).exp();
        self.prefix[0] = 0.0;
        for k in 0..BINS {
            let m = self.spec[0][k].norm_sqr().max(self.spec[1][k].norm_sqr());
            self.power[k] = smooth * self.power[k] + (1.0 - smooth) * m;
            self.logmag[k] = 10.0 * self.power[k].max(1e-20).log10();
            self.prefix[k + 1] = self.prefix[k] + self.power[k] as f64;
        }
        let half = 2f32.powf(p.width * 0.5);
        for k in 1..BINS {
            let f = k as f32 * bin_hz;
            let want = if f < p.low_hz || f > p.high_hz {
                0.0
            } else {
                // 対数周波数で ±width/2 オクターブのパワーの平均(自分より少し広い範囲 = 包絡)
                let lo = ((k as f32 / half) as usize).max(1);
                let hi = ((k as f32 * half).ceil() as usize)
                    .min(BINS - 1)
                    .max(lo + 2);
                let mean = (self.prefix[hi + 1] - self.prefix[lo]) / (hi + 1 - lo) as f64;
                let env = 10.0 * (mean as f32).max(1e-20).log10();
                (self.logmag[k] - env - p.threshold_db).clamp(0.0, p.depth_db)
            };
            // 上げるのは速く、戻すのはゆっくり
            let cur = self.reduction[k];
            self.reduction[k] = if want > cur {
                want
            } else {
                p.release_coef * cur + (1.0 - p.release_coef) * want
            };
        }
        // 下げてから合成(負の周波数側も同じ量)
        let norm = 1.0 / (FRAME as f32 * 2.0);
        for c in 0..2 {
            for k in 0..BINS {
                let g = 10f32.powf(-self.reduction[k] / 20.0);
                self.spec[c][k] *= g;
                if k > 0 && k < FRAME - k {
                    self.spec[c][FRAME - k] *= g;
                }
            }
            self.ifft
                .process_with_scratch(&mut self.spec[c], &mut self.scratch);
            for k in 0..FRAME {
                let idx = (self.pos + k) % FRAME;
                self.output[c][idx] += self.spec[c][k].re * self.window[k] * norm;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(depth: f32) -> ResonanceParams {
        ResonanceParams::new(depth, 3.0, 100.0, 16_000.0, 80.0, 1.0, 0.5, 48_000.0)
    }

    fn noise(n: usize) -> Vec<f32> {
        let mut r: u32 = 5;
        (0..n)
            .map(|_| {
                r ^= r << 13;
                r ^= r >> 17;
                r ^= r << 5;
                (r as f32 / u32::MAX as f32 - 0.5) * 0.2
            })
            .collect()
    }

    /// 周波数 f の成分の大きさ(Hann 窓の DFT)
    fn level(x: &[f32], f: f32) -> f32 {
        let (mut re, mut im) = (0.0f32, 0.0f32);
        let n = x.len() as f32;
        for (k, v) in x.iter().enumerate() {
            let w = 0.5 - 0.5 * (std::f32::consts::TAU * k as f32 / n).cos();
            let ph = k as f32 * f * std::f32::consts::TAU / 48_000.0;
            re += v * w * ph.cos();
            im += v * w * ph.sin();
        }
        (re * re + im * im).sqrt()
    }

    #[test]
    fn passes_through_when_depth_is_zero() {
        // 下げなければ、1 フレーム遅れた元の音に戻る(分析・合成で音が崩れない)
        let x = noise(8192);
        let p = params(0.0);
        let mut st = ResonanceState::default();
        let out: Vec<f32> = x.iter().map(|v| st.process(&p, *v, *v).0).collect();
        for n in 2 * FRAME..8192 {
            assert!(
                (out[n] - x[n - FRAME]).abs() < 1e-4,
                "{n}: {} / {}",
                out[n],
                x[n - FRAME]
            );
        }
        assert_eq!(p.latency(), FRAME as u32);
    }

    #[test]
    fn lowers_a_narrow_resonance_and_keeps_the_rest() {
        // 雑音に 3kHz の強い共鳴(サイン波)を足す
        let n = 48_000;
        let base = noise(n);
        let x: Vec<f32> = base
            .iter()
            .enumerate()
            .map(|(i, v)| v + (i as f32 * 3000.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.3)
            .collect();
        let run = |depth: f32| {
            let p = params(depth);
            let mut st = ResonanceState::default();
            let out: Vec<f32> = x.iter().map(|v| st.process(&p, *v, *v).0).collect();
            out[24_000..].to_vec()
        };
        let (off, on) = (run(0.0), run(12.0));
        let db = |a: f32, b: f32| 20.0 * (a / b).log10();
        let res = db(level(&on, 3000.0), level(&off, 3000.0));
        let broad = db(level(&on, 1234.0), level(&off, 1234.0));
        assert!(res < -6.0, "共鳴は下がる: {res:.1} dB");
        assert!(broad.abs() < 1.5, "ほかはそのまま: {broad:.1} dB");
    }
}
