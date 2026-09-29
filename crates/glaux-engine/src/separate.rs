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
///
/// スペクトログラム全体は持たない: フレームを順に STFT し、メディアンに要る前後 `KERNEL / 2` フレームだけを
/// 輪に置いて、マスクを掛けて逆変換したものを出力へ重ね足す。フレームの区間ごとに並列に処理し
/// (区間の端では前後のフレームを STFT し直す)、区間ごとの出力を最後に足し合わせる
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
    // 先頭・末尾に窓の半分の無音を足し、端も完全に再構成できるようにする
    let pad = N_FFT / 2;
    let padded_len = len + 2 * pad + N_FFT;
    let n_frames = (len + 2 * pad) / HOP + 1;

    let workers = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, 8)
        .min(n_frames.div_ceil(256).max(1));
    let chunk = n_frames.div_ceil(workers).max(1);
    let parts: Vec<(usize, Vec<Split>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..n_frames)
            .step_by(chunk)
            .map(|lo| {
                let window = &window;
                let hi = (lo + chunk).min(n_frames);
                scope.spawn(move || (lo, hpss_frames(channels, window, lo, hi, n_frames)))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
            .collect()
    });
    let mut outs: Vec<(Vec<f32>, Vec<f32>)> = channels
        .iter()
        .map(|_| (vec![0.0f32; padded_len], vec![0.0f32; padded_len]))
        .collect();
    for (lo, part) in parts {
        let at = lo * HOP;
        for ((out_h, out_p), (h, p)) in outs.iter_mut().zip(part) {
            for (d, v) in out_h[at..at + h.len()].iter_mut().zip(&h) {
                *d += v;
            }
            for (d, v) in out_p[at..at + p.len()].iter_mut().zip(&p) {
                *d += v;
            }
        }
    }
    // 窓の 2 乗和で正規化する(位置 x に掛かるフレームの窓の 2 乗を、フレームの順に足す)
    let norm = |x: usize| -> f32 {
        let f_lo = (x + 1).saturating_sub(N_FFT).div_ceil(HOP);
        let f_hi = (x / HOP).min(n_frames - 1);
        (f_lo..=f_hi)
            .map(|f| window[x - f * HOP] * window[x - f * HOP])
            .fold(0.0f32, |acc, w| acc + w)
    };
    let finish = |v: Vec<f32>| -> Vec<f32> {
        (0..len)
            .map(|i| {
                let n = norm(pad + i);
                if n > 1e-6 {
                    v[pad + i] / n
                } else {
                    0.0
                }
            })
            .collect()
    };
    outs.into_iter()
        .map(|(h, p)| HpssResult {
            harmonic: finish(h),
            percussive: finish(p),
        })
        .collect()
}

/// 分けた (持続音, 打楽器) の重ね足しの途中
type Split = (Vec<f32>, Vec<f32>);

/// フレーム `lo..hi` を分けて逆変換した (持続音, 打楽器) を、チャンネルごとに返す。
/// 返す列はサンプル `lo * HOP` から始まり、窓を重ね足しただけのもの(正規化は呼ぶ側)
fn hpss_frames(
    channels: &[&[f32]],
    window: &[f32],
    lo: usize,
    hi: usize,
    n_frames: usize,
) -> Vec<Split> {
    let bins = N_FFT / 2 + 1;
    let pad = N_FFT / 2;
    let half = KERNEL / 2;
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N_FFT);
    let ifft = planner.plan_fft_inverse(N_FFT);
    let zero = Complex::new(0.0f32, 0.0);
    let mut buf = vec![zero; N_FFT];
    // 直近 KERNEL フレームの片側スペクトル(チャンネルごと)と、最初のチャンネルの振幅
    let mut spec_ring = vec![vec![vec![zero; bins]; KERNEL]; channels.len()];
    let mut mag_ring = vec![vec![0.0f32; bins]; KERNEL];
    let span = (hi - lo - 1) * HOP + N_FFT;
    let mut outs: Vec<(Vec<f32>, Vec<f32>)> = channels
        .iter()
        .map(|_| (vec![0.0f32; span], vec![0.0f32; span]))
        .collect();
    let mut scratch = Vec::with_capacity(KERNEL);
    let mut median = |values: &mut dyn Iterator<Item = f32>| -> f32 {
        scratch.clear();
        scratch.extend(values);
        let mid = scratch.len() / 2;
        *scratch.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1
    };
    let mut mask_h = vec![0.5f32; bins];
    let mut next_in = lo.saturating_sub(half);
    for f in lo..hi {
        // メディアンに要る先のフレームまで STFT して輪に入れる
        let need = (f + half + 1).min(n_frames);
        while next_in < need {
            let slot = next_in % KERNEL;
            let s = next_in * HOP;
            for (c, x) in channels.iter().enumerate() {
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
                spec_ring[c][slot].copy_from_slice(&buf[..bins]);
                if c == 0 {
                    for (m, v) in mag_ring[slot].iter_mut().zip(&buf[..bins]) {
                        *m = v.norm();
                    }
                }
            }
            next_in += 1;
        }
        // 時間方向のメディアン = 持続成分、周波数方向のメディアン = 打楽器成分。
        // 持続成分のソフトマスク(打楽器側は 1 − これ)
        let (t0, t1) = (f.saturating_sub(half), (f + half + 1).min(n_frames));
        let row = &mag_ring[f % KERNEL];
        for b in 0..bins {
            let h = median(&mut (t0..t1).map(|t| mag_ring[t % KERNEL][b]));
            let (b0, b1) = (b.saturating_sub(half), (b + half + 1).min(bins));
            let p = median(&mut row[b0..b1].iter().copied());
            let (h2, p2) = (h * h, p * p);
            let total = h2 + p2;
            mask_h[b] = if total > 1e-20 { h2 / total } else { 0.5 };
        }
        // ソフトマスクを掛けて逆変換し、重ね足す
        let at = (f - lo) * HOP;
        for (c, (out_h, out_p)) in outs.iter_mut().enumerate() {
            let spec = &spec_ring[c][f % KERNEL];
            for (dest, harmonic_part) in [(&mut *out_h, true), (&mut *out_p, false)] {
                for b in 0..bins {
                    let m = if harmonic_part {
                        mask_h[b]
                    } else {
                        1.0 - mask_h[b]
                    };
                    buf[b] = spec[b] * m;
                }
                // 実信号なので負の周波数は共役で埋める
                for b in bins..N_FFT {
                    buf[b] = buf[N_FFT - b].conj();
                }
                ifft.process(&mut buf);
                for i in 0..N_FFT {
                    dest[at + i] += buf[i].re / N_FFT as f32 * window[i];
                }
            }
        }
    }
    outs
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

    /// 以前の(スペクトログラム全体を持つ)実装。流しながら・並列に処理しても同じ結果になることを確かめる
    /// 複数チャンネルを同じマスクで分ける(マスクは最初のチャンネルで決める)。
    /// ステレオは M(左右の平均)と S(左右差)を渡すと、分けた後も定位が保たれる。
    fn hpss_reference(channels: &[&[f32]]) -> Vec<HpssResult> {
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

    #[test]
    fn streaming_matches_the_whole_spectrogram_version() {
        // 区間が 3 つ以上に分かれる長さ(15 秒)。M と S の 2 チャンネル
        let (m, _, _) = mix(22_050.0, 15.0);
        let side: Vec<f32> = m
            .iter()
            .enumerate()
            .map(|(i, v)| v * ((i % 7) as f32 / 7.0 - 0.5))
            .collect();
        let got = hpss_channels(&[&m, &side]);
        let want = hpss_reference(&[&m, &side]);
        for (g, w) in got.iter().zip(&want) {
            for (a, b) in g
                .harmonic
                .iter()
                .zip(&w.harmonic)
                .chain(g.percussive.iter().zip(&w.percussive))
            {
                assert!((a - b).abs() < 1e-5, "{a} / {b}");
            }
        }
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
