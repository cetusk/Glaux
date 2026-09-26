//! 先読み(ルックアヘッド)の True Peak リミッタ。
//!
//! 入力を 4 倍にオーバーサンプリングした補間点も含めて山を測り(サンプルの間の山も見落とさない)、
//! 必要な減衰を「先読みの幅の最小値 → 同じ幅の移動平均」で滑らかにしてから、同じだけ遅らせた音に掛ける。
//! 移動平均が山に届く前に下がりきるので、上限(ceiling)を超える前に音量が下がり、歪み(クリップ)が出ない。
//! 戻りはリリースの時定数でゆっくり。
//!
//! 遅れ = 補間の片側の幅(6)+ 先読みの幅 − 1 サンプル(48kHz で約 1.1ms)。エンジンの遅延補正に申告する。
//! 状態は固定長の配列だけ(192kHz まで)。アロケーションなし。

/// 補間の片側の幅(元のサンプル数)。1 位相あたり 2 × HALF タップ
pub const TP_HALF: usize = 6;
/// オーバーサンプリングの倍率
pub const TP_OS: usize = 4;

/// True Peak の補間係数(窓付き sinc、Kaiser β=7、ITU-R BS.1770 附属書 2 と同じ 4 倍)。
/// 位相 p(1..4)の係数を x[n - HALF + 1 ..= n + HALF] に掛けると x(n + p/4) になる
pub fn true_peak_kernel() -> [[f32; 2 * TP_HALF]; TP_OS - 1] {
    const BETA: f64 = 7.0;
    fn bessel_i0(x: f64) -> f64 {
        let mut sum = 1.0;
        let mut term = 1.0;
        for k in 1..30 {
            term *= (x / (2.0 * k as f64)).powi(2);
            sum += term;
        }
        sum
    }
    let mut out = [[0.0f32; 2 * TP_HALF]; TP_OS - 1];
    for (pi, phase) in out.iter_mut().enumerate() {
        let frac = (pi + 1) as f64 / TP_OS as f64;
        let mut taps = [0.0f64; 2 * TP_HALF];
        for (k, t) in taps.iter_mut().enumerate() {
            let u = (k as f64 - (TP_HALF as f64 - 1.0)) - frac;
            let sinc = if u.abs() < 1e-12 {
                1.0
            } else {
                (std::f64::consts::PI * u).sin() / (std::f64::consts::PI * u)
            };
            let r = u / TP_HALF as f64;
            let w = if r.abs() >= 1.0 {
                0.0
            } else {
                bessel_i0(BETA * (1.0 - r * r).sqrt()) / bessel_i0(BETA)
            };
            *t = sinc * w;
        }
        // 直流の利得を 1 に
        let sum: f64 = taps.iter().sum();
        for (d, s) in phase.iter_mut().zip(taps) {
            *d = (s / sum) as f32;
        }
    }
    out
}

/// 先読みの幅の上限(192kHz で 1ms)
const MAX_WINDOW: usize = 256;
/// 音を遅らせる輪の長さ(先読み + 補間の幅より長く)
const DELAY_CAP: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LimiterParams {
    /// 入力のゲイン(リニア)。上げると上限に張り付いて音圧が上がる
    pub input: f32,
    /// 上限(リニア、True Peak)
    pub ceiling: f32,
    /// 戻りの 1 サンプルあたりの割合
    pub release: f32,
    /// 先読みの幅(サンプル)
    pub window: usize,
    pub input_db: f32,
    pub ceiling_db: f32,
    pub release_ms: f32,
    pub sample_rate: f32,
}

impl LimiterParams {
    pub fn new(input_db: f32, ceiling_db: f32, release_ms: f32, sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        let input_db = input_db.clamp(0.0, 24.0);
        let ceiling_db = ceiling_db.clamp(-12.0, 0.0);
        let release_ms = release_ms.clamp(5.0, 2000.0);
        LimiterParams {
            input: 10.0_f32.powf(input_db / 20.0),
            ceiling: 10.0_f32.powf(ceiling_db / 20.0),
            release: 1.0 - (-1.0 / (release_ms * 0.001 * sr)).exp(),
            window: ((0.001 * sr) as usize).clamp(4, MAX_WINDOW),
            input_db,
            ceiling_db,
            release_ms,
            sample_rate: sr,
        }
    }

    /// 遅れ(サンプル)
    pub fn latency(&self) -> u32 {
        (TP_HALF + self.window - 1) as u32
    }
}

#[derive(Clone, Debug)]
pub struct LimiterState {
    kernel: [[f32; 2 * TP_HALF]; TP_OS - 1],
    /// 入力の履歴(補間用、左右)
    hist: [[f32; 2 * TP_HALF]; 2],
    hist_pos: usize,
    /// 1 つ前のフレームの True Peak(サンプルとその次の間の山)
    prev_tp: f32,
    /// 遅らせる音(左右)
    delay: [[f32; DELAY_CAP]; 2],
    delay_pos: usize,
    /// 必要な減衰の履歴(先読みの幅の最小を求める用)
    req: [f32; MAX_WINDOW],
    req_pos: usize,
    /// 最小値の履歴(移動平均用)と、その和
    mins: [f32; MAX_WINDOW],
    mins_sum: f64,
    /// いま掛けている減衰
    gain: f32,
    /// 窓の幅が変わったら履歴を入れ直す
    window: usize,
}

impl Default for LimiterState {
    fn default() -> Self {
        LimiterState {
            kernel: true_peak_kernel(),
            hist: [[0.0; 2 * TP_HALF]; 2],
            hist_pos: 0,
            prev_tp: 0.0,
            delay: [[0.0; DELAY_CAP]; 2],
            delay_pos: 0,
            req: [1.0; MAX_WINDOW],
            req_pos: 0,
            mins: [1.0; MAX_WINDOW],
            mins_sum: 0.0,
            gain: 1.0,
            window: 0,
        }
    }
}

impl LimiterState {
    #[inline]
    pub fn process(&mut self, p: &LimiterParams, l: f32, r: f32) -> (f32, f32) {
        let w = p.window;
        if self.window != w {
            self.window = w;
            self.req = [1.0; MAX_WINDOW];
            self.mins = [1.0; MAX_WINDOW];
            self.mins_sum = w as f64;
            self.req_pos = 0;
        }
        let (l, r) = (l * p.input, r * p.input);
        // 補間用の履歴(古い順に読めるよう、2 × HALF の輪)
        self.hist[0][self.hist_pos] = l;
        self.hist[1][self.hist_pos] = r;
        self.hist_pos = (self.hist_pos + 1) % (2 * TP_HALF);
        // 輪の中の「中央」のサンプル m(HALF サンプル前)と、m と m+1 の間の補間点の山
        let n = 2 * TP_HALF;
        let at = |c: usize, k: usize| self.hist[c][(self.hist_pos + k) % n];
        let mut tp = 0.0f32;
        for c in 0..2 {
            tp = tp.max(at(c, TP_HALF - 1).abs());
            for phase in &self.kernel {
                let mut acc = 0.0;
                for (k, coef) in phase.iter().enumerate() {
                    acc += coef * at(c, k);
                }
                tp = tp.max(acc.abs());
            }
        }
        // m の前後の区間の山で決める(m の前の区間は 1 つ前のフレームの値)
        let peak = tp.max(self.prev_tp);
        self.prev_tp = tp;
        let need = if peak > p.ceiling {
            p.ceiling / peak
        } else {
            1.0
        };
        // 先読みの幅の最小値(幅は小さいので素直に数える)
        self.req[self.req_pos] = need;
        self.req_pos = (self.req_pos + 1) % w;
        let min = self.req[..w].iter().copied().fold(1.0f32, f32::min);
        // その移動平均(山の手前から少しずつ下げる)
        let old = self.mins[self.req_pos];
        self.mins[self.req_pos] = min;
        self.mins_sum += min as f64 - old as f64;
        let smooth = (self.mins_sum / w as f64) as f32;
        // 下げるのはすぐ、戻すのはリリースで
        self.gain = if smooth < self.gain {
            smooth
        } else {
            self.gain + (smooth - self.gain) * p.release
        };
        // 音を遅らせて掛ける。減衰は m − (w − 1) 番目のサンプルの分(m は HALF サンプル前)なので、
        // 遅れは HALF + w − 1
        self.delay[0][self.delay_pos] = l;
        self.delay[1][self.delay_pos] = r;
        let d = TP_HALF + w - 1;
        let i = (self.delay_pos + DELAY_CAP - d) % DELAY_CAP;
        self.delay_pos = (self.delay_pos + 1) % DELAY_CAP;
        (self.delay[0][i] * self.gain, self.delay[1][i] * self.gain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 出力の True Peak(4 倍の補間で)
    fn true_peak(v: &[(f32, f32)]) -> f32 {
        let k = true_peak_kernel();
        let mut peak = 0.0f32;
        for c in 0..2 {
            let x: Vec<f32> = v.iter().map(|s| if c == 0 { s.0 } else { s.1 }).collect();
            for i in TP_HALF..x.len() - TP_HALF {
                peak = peak.max(x[i].abs());
                for phase in &k {
                    let acc: f32 = phase
                        .iter()
                        .enumerate()
                        .map(|(t, c)| c * x[i + t + 1 - TP_HALF])
                        .sum();
                    peak = peak.max(acc.abs());
                }
            }
        }
        peak
    }

    #[test]
    fn keeps_true_peak_under_the_ceiling() {
        // fs/4 を 45° ずらした大きな音(サンプルは ±0.707 × 2、サンプルの間の山は 2.0)と、急な打撃
        let p = LimiterParams::new(0.0, -1.0, 100.0, 48_000.0);
        let mut st = LimiterState::default();
        let out: Vec<(f32, f32)> = (0..24_000)
            .map(|i| {
                let x = if i < 12_000 {
                    2.0 * (std::f32::consts::FRAC_PI_2 * i as f32 + std::f32::consts::FRAC_PI_4)
                        .sin()
                } else if i % 4800 < 10 {
                    1.8
                } else {
                    0.05
                };
                st.process(&p, x, -x)
            })
            .collect();
        let db = 20.0 * true_peak(&out).log10();
        assert!(db <= -0.9, "True Peak {db:.2} dBTP");
    }

    #[test]
    fn passes_quiet_audio_delayed_and_unchanged() {
        let p = LimiterParams::new(0.0, -1.0, 100.0, 48_000.0);
        let mut st = LimiterState::default();
        let x: Vec<f32> = (0..4800)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.3)
            .collect();
        let out: Vec<f32> = x.iter().map(|v| st.process(&p, *v, *v).0).collect();
        let d = p.latency() as usize;
        for i in d..4800 {
            assert!((out[i] - x[i - d]).abs() < 1e-6, "{i}");
        }
        assert_eq!(p.latency(), 6 + 48 - 1);
    }

    #[test]
    fn input_gain_pushes_level_to_the_ceiling() {
        let p = LimiterParams::new(12.0, -1.0, 50.0, 48_000.0);
        let mut st = LimiterState::default();
        let out: Vec<(f32, f32)> = (0..48_000)
            .map(|i| {
                let x = (i as f32 * 100.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5;
                st.process(&p, x, x)
            })
            .collect();
        let tp = 20.0 * true_peak(&out[24_000..]).log10();
        assert!(tp <= -0.9 && tp > -2.0, "{tp:.2}");
    }
}
