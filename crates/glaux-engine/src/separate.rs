//! 音源分離(内蔵): HPSS(Harmonic / Percussive Source Separation、Fitzgerald 2010)。
//!
//! スペクトログラム上で「時間方向に伸びる成分(音程のある持続音)」と「周波数方向に伸びる成分
//! (打楽器の瞬間的な音)」をメディアンフィルタで見分け、ソフトマスク(ウィーナー型)で
//! 振り分ける。学習モデルを使わないので軽く常に使えるが、分けられるのは
//! 「打楽器 / 音程のある楽器」の 2 つまで(ボーカルとギター等は分けられない)。
//! 2 つのマスクの和は 1 なので、分けた 2 本を足すと元に戻る。
//!
//! オフライン処理(UI・MCP のスレッドで呼ぶ)。

use rustfft::{num_complex::Complex, FftPlanner};

const N_FFT: usize = 2048;
const HOP: usize = 512;
/// メディアンフィルタの長さ(フレーム数 / ビン数。奇数)
const KERNEL: usize = 17;

/// 分離結果(入力と同じ長さ・サンプルレート)。
pub struct HpssResult {
    /// 音程のある持続音(ボーカル・ベース・和音楽器など)
    pub harmonic: Vec<f32>,
    /// 打楽器(ドラム・パーカッション・子音のアタック)
    pub percussive: Vec<f32>,
}

/// モノラル音声を打楽器 / 音程楽器に分ける。
pub fn hpss(input: &[f32]) -> HpssResult {
    hpss_channels(&[input]).pop().unwrap_or(HpssResult {
        harmonic: vec![],
        percussive: vec![],
    })
}

/// 複数チャンネルを同じマスクで分ける(マスクは最初のチャンネルで決める)。
/// ステレオは M(左右の平均)と S(左右差)を渡すと、分けた後も定位が保たれる。
pub fn hpss_channels(channels: &[&[f32]]) -> Vec<HpssResult> {
    let Some(first) = channels.first() else {
        return Vec::new();
    };
    let len = first.len();
    if len == 0 {
        return channels
            .iter()
            .map(|_| HpssResult {
                harmonic: vec![],
                percussive: vec![],
            })
            .collect();
    }
    let window: Vec<f32> = (0..N_FFT)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / N_FFT as f32).cos())
        .collect();
    let bins = N_FFT / 2 + 1;
    // 先頭・末尾に窓の半分の無音を足し、端も完全に再構成できるようにする
    let pad = N_FFT / 2;
    let padded_len = len + 2 * pad + N_FFT;
    let n_frames = (len + 2 * pad) / HOP + 1;

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N_FFT);
    let ifft = planner.plan_fft_inverse(N_FFT);
    let mut buf = vec![Complex::new(0.0, 0.0); N_FFT];

    // STFT(片側スペクトル)
    let mut stft = |x: &[f32]| -> Vec<Vec<Complex<f32>>> {
        let mut spec = Vec::with_capacity(n_frames);
        for f in 0..n_frames {
            let s = f * HOP;
            for i in 0..N_FFT {
                // 先頭に pad の無音があるとみなす
                let v = (s + i)
                    .checked_sub(pad)
                    .and_then(|k| x.get(k))
                    .copied()
                    .unwrap_or(0.0);
                buf[i] = Complex::new(v * window[i], 0.0);
            }
            fft.process(&mut buf);
            spec.push(buf[..bins].to_vec());
        }
        spec
    };
    let specs: Vec<Vec<Vec<Complex<f32>>>> = channels.iter().map(|x| stft(x)).collect();
    let mag: Vec<Vec<f32>> = specs[0]
        .iter()
        .map(|row| row.iter().map(|c| c.norm()).collect())
        .collect();

    // 時間方向のメディアン = 持続成分、周波数方向のメディアン = 打楽器成分
    let half = KERNEL / 2;
    let mut scratch = Vec::with_capacity(KERNEL);
    let mut median = |values: &mut dyn Iterator<Item = f32>| -> f32 {
        scratch.clear();
        scratch.extend(values);
        let mid = scratch.len() / 2;
        *scratch.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1
    };
    // 持続成分のソフトマスク(打楽器側は 1 − これ)
    let mut mask_h = vec![vec![0.5f32; bins]; n_frames];
    for f in 0..n_frames {
        let (t0, t1) = (f.saturating_sub(half), (f + half + 1).min(n_frames));
        for b in 0..bins {
            let h = median(&mut (t0..t1).map(|t| mag[t][b]));
            let (b0, b1) = (b.saturating_sub(half), (b + half + 1).min(bins));
            let p = median(&mut mag[f][b0..b1].iter().copied());
            let (h2, p2) = (h * h, p * p);
            let total = h2 + p2;
            if total > 1e-20 {
                mask_h[f][b] = h2 / total;
            }
        }
    }

    // ソフトマスクを掛けて逆変換(窓の 2 乗和で正規化する重ね合わせ)
    let mut norm = vec![0.0f32; padded_len];
    for f in 0..n_frames {
        let s = f * HOP;
        for i in 0..N_FFT {
            norm[s + i] += window[i] * window[i];
        }
    }
    let finish = |v: Vec<f32>| -> Vec<f32> {
        (0..len)
            .map(|i| {
                let n = norm[pad + i];
                if n > 1e-6 {
                    v[pad + i] / n
                } else {
                    0.0
                }
            })
            .collect()
    };
    specs
        .iter()
        .map(|spec| {
            let mut out_h = vec![0.0f32; padded_len];
            let mut out_p = vec![0.0f32; padded_len];
            for f in 0..n_frames {
                let s = f * HOP;
                for (dest, harmonic_part) in [(&mut out_h, true), (&mut out_p, false)] {
                    for b in 0..bins {
                        let m = if harmonic_part {
                            mask_h[f][b]
                        } else {
                            1.0 - mask_h[f][b]
                        };
                        buf[b] = spec[f][b] * m;
                    }
                    // 実信号なので負の周波数は共役で埋める
                    for b in bins..N_FFT {
                        buf[b] = buf[N_FFT - b].conj();
                    }
                    ifft.process(&mut buf);
                    for i in 0..N_FFT {
                        dest[s + i] += buf[i].re / N_FFT as f32 * window[i];
                    }
                }
            }
            HpssResult {
                harmonic: finish(out_h),
                percussive: finish(out_p),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    /// 持続するサイン波 + 0.25 秒ごとのクリック(打楽器)
    fn mix(sr: f32, secs: f32) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
        let n = (sr * secs) as usize;
        let tone: Vec<f32> = (0..n)
            .map(|i| 0.3 * (std::f32::consts::TAU * 330.0 * i as f32 / sr).sin())
            .collect();
        let mut clicks = vec![0.0f32; n];
        let every = (sr * 0.25) as usize;
        let mut seed = 1u32;
        for start in (every / 2..n).step_by(every) {
            for i in 0..200.min(n - start) {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (seed >> 9) as f32 / (1u32 << 23) as f32 * 2.0 - 1.0;
                clicks[start + i] = 0.8 * noise * (-(i as f32) / 40.0).exp();
            }
        }
        let sum = tone.iter().zip(&clicks).map(|(a, b)| a + b).collect();
        (sum, tone, clicks)
    }

    #[test]
    fn parts_add_back_to_the_input() {
        let (x, _, _) = mix(22_050.0, 1.0);
        let r = hpss(&x);
        assert_eq!(r.harmonic.len(), x.len());
        let err: Vec<f32> = x
            .iter()
            .zip(r.harmonic.iter().zip(&r.percussive))
            .map(|(a, (h, p))| a - h - p)
            .collect();
        assert!(rms(&err) < 1e-4, "足すと元に戻る: {}", rms(&err));
    }

    #[test]
    fn separates_tone_from_clicks() {
        let (x, tone, clicks) = mix(22_050.0, 2.0);
        let r = hpss(&x);
        // 持続音側はサイン波に近く、打楽器側はクリックに近い
        let err_h: Vec<f32> = r.harmonic.iter().zip(&tone).map(|(a, b)| a - b).collect();
        let err_p: Vec<f32> = r
            .percussive
            .iter()
            .zip(&clicks)
            .map(|(a, b)| a - b)
            .collect();
        assert!(
            rms(&err_h) < rms(&tone) * 0.35,
            "持続音側: 誤差 {} / 元 {}",
            rms(&err_h),
            rms(&tone)
        );
        assert!(
            rms(&err_p) < rms(&clicks) * 0.6,
            "打楽器側: 誤差 {} / 元 {}",
            rms(&err_p),
            rms(&clicks)
        );
    }
}
