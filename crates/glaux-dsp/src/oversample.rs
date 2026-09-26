//! 歪みの折り返し(エイリアシング)対策。
//!
//! 歪ませると元の音の何倍もの周波数の倍音が出る。ナイキスト(サンプルレートの半分)を超えた倍音は
//! 可聴域へ折り返して、元の音と関係のない濁った音になる(高い音ほど目立つ)。対策は 2 つ:
//!
//! - [`Halfband`]: 2 倍オーバーサンプリング。2 本のオールパスの並び(多相 IIR のハーフバンド)で
//!   アップ・ダウンする。通過域は平ら(リップルなし)、阻止域は約 -75dB、遅れは低域で約 1.4 サンプル(往復で約 2.7)。
//!   係数は de Soras の設計法(楕円フィルタの閉じた式)で求めた値
//! - [`AdaaTanh`]: 1 次の不定積分によるアンチエイリアス(ADAA)。tanh の代わりに
//!   「不定積分 ln cosh の差分 ÷ 入力の差分」を出す。オーバーサンプリングなしで折り返しが減る。
//!   遅れは 0.5 サンプルで、並列に混ぜても打ち消しがほとんど出ない。代わりに、ほぼ線形な
//!   小さい音では高域がやや落ちる(10kHz で約 -2dB)
//!
//! どちらもアロケーションなし(`Copy` の状態だけ)。

/// ハーフバンドの係数(6 本。遷移帯域 0.04、阻止域 約 -75dB)。偶数番目が経路 0、奇数番目が経路 1
const HB_COEFS: [f32; 6] = [
    0.068_204_08,
    0.240_270_36,
    0.448_676_24,
    0.641_122_37,
    0.799_997_56,
    0.934_482_24,
];
const HB_N: usize = HB_COEFS.len();

/// 1 本のオールパス(元のレートで動く 1 次のオールパスの並び)の状態
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct AllpassChain {
    x1: [f32; HB_N / 2],
    y1: [f32; HB_N / 2],
}

impl AllpassChain {
    #[inline]
    fn process(&mut self, mut x: f32, path: usize) -> f32 {
        for k in 0..HB_N / 2 {
            let a = HB_COEFS[k * 2 + path];
            let y = a * (x - self.y1[k]) + self.x1[k];
            self.x1[k] = x;
            self.y1[k] = y;
            x = y;
        }
        x
    }
}

/// 2 倍のアップ・ダウン(1 チャンネル分)。アップとダウンで別の状態を持つ
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Halfband {
    up: [AllpassChain; 2],
    down: [AllpassChain; 2],
}

impl Halfband {
    /// 1 サンプルを 2 サンプル(先, 後)にする
    #[inline]
    pub fn up(&mut self, x: f32) -> (f32, f32) {
        (self.up[0].process(x, 0), self.up[1].process(x, 1))
    }

    /// 2 サンプル(先, 後)を 1 サンプルにする
    #[inline]
    pub fn down(&mut self, first: f32, second: f32) -> f32 {
        0.5 * (self.down[0].process(second, 0) + self.down[1].process(first, 1))
    }

    /// `f` を 2 倍のレートで掛ける(アップ → f を 2 回 → ダウン)
    #[inline]
    pub fn run(&mut self, x: f32, mut f: impl FnMut(f32) -> f32) -> f32 {
        let (a, b) = self.up(x);
        let (a, b) = (f(a), f(b));
        self.down(a, b)
    }
}

/// tanh の 1 次 ADAA(1 チャンネル分)
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AdaaTanh {
    x1: f64,
    f1: f64,
}

/// tanh の不定積分 ln cosh x(大きな x でもあふれない形)
#[inline]
fn ln_cosh(x: f64) -> f64 {
    let a = x.abs();
    a + (-2.0 * a).exp().ln_1p() - std::f64::consts::LN_2
}

impl AdaaTanh {
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let x = x as f64;
        let f = ln_cosh(x);
        let dx = x - self.x1;
        let y = if dx.abs() < 1e-5 {
            // 差がほぼ 0 なら中点の tanh(0 ÷ 0 を避ける)
            (0.5 * (x + self.x1)).tanh()
        } else {
            (f - self.f1) / dx
        };
        self.x1 = x;
        self.f1 = f;
        y as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// de Soras の設計法で係数を求め直し、表の値と一致することを確かめる(表の出どころの記録)
    #[test]
    fn coefficients_match_the_design() {
        let tbw = 0.04f64;
        let order = HB_N * 2 + 1;
        let mut k = ((1.0 - tbw * 2.0) * std::f64::consts::FRAC_PI_4).tan();
        k *= k;
        let kk = (1.0 - k * k).powf(0.25);
        let e = 0.5 * (1.0 - kk) / (1.0 + kk);
        let e4 = e.powi(4);
        let q = e * (1.0 + e4 * (2.0 + e4 * (15.0 + 150.0 * e4)));
        for (idx, want) in HB_COEFS.iter().enumerate() {
            let c = (idx + 1) as f64;
            let pi = std::f64::consts::PI;
            let (mut num, mut den) = (0.0, 0.0);
            for i in 0..20 {
                let s = if i % 2 == 0 { 1.0 } else { -1.0 };
                num += s
                    * q.powi((i * (i + 1)) as i32)
                    * ((i as f64 * 2.0 + 1.0) * c * pi / order as f64).sin();
            }
            for i in 1..20 {
                let s = if i % 2 == 1 { -1.0 } else { 1.0 };
                den += s * q.powi((i * i) as i32) * (i as f64 * 2.0 * c * pi / order as f64).cos();
            }
            let ww = num * q.powf(0.25) / (den + 0.5);
            let w2 = ww * ww;
            let x = ((1.0 - w2 * k) * (1.0 - w2 / k)).sqrt() / (1.0 + w2);
            let coef = (1.0 - x) / (1.0 + x);
            assert!((coef as f32 - want).abs() < 1e-6, "{idx}: {coef} vs {want}");
        }
    }

    fn sine(freq: f32, i: usize, sr: f32) -> f32 {
        (i as f32 * freq * std::f32::consts::TAU / sr).sin()
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn up_then_down_is_transparent_in_the_audio_band() {
        for freq in [100.0, 1000.0, 10_000.0, 18_000.0] {
            let mut hb = Halfband::default();
            let out: Vec<f32> = (0..9600)
                .map(|i| hb.run(sine(freq, i, 48_000.0), |x| x))
                .collect();
            let r = rms(&out[4800..]) * std::f32::consts::SQRT_2;
            assert!((r - 1.0).abs() < 0.01, "{freq}Hz: {r}");
        }
    }

    #[test]
    fn upsampling_suppresses_the_image() {
        // 18kHz を 2 倍にしたとき、鏡像(48k - 18k の折り返し = 30kHz)がほとんど出ない
        let mut hb = Halfband::default();
        let mut hi = Vec::new();
        for i in 0..4800 {
            let (a, b) = hb.up(sine(18_000.0, i, 48_000.0));
            hi.push(a);
            hi.push(b);
        }
        let dft = |f: f32| {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (n, v) in hi[2000..].iter().enumerate() {
                let ph = n as f64 * f as f64 * std::f64::consts::TAU / 96_000.0;
                re += *v as f64 * ph.cos();
                im += *v as f64 * ph.sin();
            }
            (re * re + im * im).sqrt()
        };
        let db = 20.0 * (dft(30_000.0) / dft(18_000.0)).log10();
        assert!(db < -60.0, "鏡像: {db:.1} dB");
    }

    /// 高い音を強く歪ませたときの折り返し(倍音の並びにない周波数の成分)が、そのままより減る
    #[test]
    fn oversampling_and_adaa_reduce_aliasing() {
        let sr = 48_000.0;
        let f0 = 4_700.0; // 倍音 3・5・7… の多くがナイキストを超えて折り返す
        let n = 48_000;
        let naive: Vec<f32> = (0..n).map(|i| (sine(f0, i, sr) * 8.0).tanh()).collect();
        let mut hb = Halfband::default();
        let os: Vec<f32> = (0..n)
            .map(|i| hb.run(sine(f0, i, sr), |x| (x * 8.0).tanh()))
            .collect();
        let mut ad = AdaaTanh::default();
        let adaa: Vec<f32> = (0..n).map(|i| ad.process(sine(f0, i, sr) * 8.0)).collect();
        // 倍音(f0 の整数倍)以外の成分のエネルギーを、全体に対する dB で
        let alias_db = |v: &[f32]| {
            let v = &v[4800..];
            let mut total = 0.0f64;
            let mut alias = 0.0f64;
            let bins = 400;
            for b in 1..bins {
                let f = b as f64 * sr as f64 / 2.0 / bins as f64;
                let (mut re, mut im) = (0.0f64, 0.0f64);
                let len = v.len() as f64;
                for (k, s) in v.iter().enumerate() {
                    // 漏れを抑える Hann 窓
                    let w = 0.5 - 0.5 * (std::f64::consts::TAU * k as f64 / len).cos();
                    let ph = k as f64 * f * std::f64::consts::TAU / sr as f64;
                    re += *s as f64 * w * ph.cos();
                    im += *s as f64 * w * ph.sin();
                }
                let p = re * re + im * im;
                total += p;
                let h = f / f0 as f64;
                if (h - h.round()).abs() * f0 as f64 > 120.0 {
                    alias += p;
                }
            }
            10.0 * (alias / total).log10()
        };
        let (a0, a1, a2) = (alias_db(&naive), alias_db(&os), alias_db(&adaa));
        assert!(a1 < a0 - 6.0, "2 倍: {a1:.1} dB(そのまま {a0:.1} dB)");
        assert!(a2 < a0 - 6.0, "ADAA: {a2:.1} dB(そのまま {a0:.1} dB)");
    }

    #[test]
    fn adaa_matches_tanh_for_slow_signals() {
        let mut ad = AdaaTanh::default();
        let mut worst = 0.0f32;
        for i in 0..4800 {
            let x = sine(50.0, i, 48_000.0) * 3.0;
            let y = ad.process(x);
            if i > 10 {
                worst = worst.max((y - x.tanh()).abs());
            }
        }
        assert!(worst < 0.01, "{worst}");
    }
}
