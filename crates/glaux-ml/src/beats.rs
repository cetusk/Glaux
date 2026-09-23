//! 音声のビート・小節頭・テンポの推定: Beat This!(Foscarin ら、ISMIR 2024。コード・重みとも MIT)。
//!
//! 22.05kHz のモノラル音声から対数メルスペクトログラム(50 フレーム/秒、128 帯域)を作り、
//! 小型モデル(small1、beat-this-rs が ONNX 化したもの)でフレームごとのビート・小節頭の
//! ロジットを出し、公式の「最小の後処理」(7 フレームの極大 + ロジット > 0)で時刻にする。
//! 長い音声は 1500 フレーム(30 秒)ずつ、両端 6 フレームを重ねて推論する(公式と同じ区切り方)。

use rustfft::{num_complex::Complex, FftPlanner};
use std::sync::{Arc, OnceLock};
use tract_onnx::prelude::*;

static MODEL_BYTES: &[u8] = include_bytes!("../models/beat_this_small.onnx");

pub const SAMPLE_RATE: f32 = 22_050.0;
/// 1 秒あたりのフレーム数
pub const FPS: f32 = 50.0;
const N_FFT: usize = 1024;
const HOP: usize = 441;
const N_MELS: usize = 128;
const F_MIN: f32 = 30.0;
const F_MAX: f32 = 11_000.0;
const LOG_MULTIPLIER: f32 = 1000.0;
const CHUNK: usize = 1500;
const BORDER: usize = 6;

type Plan = Arc<TypedSimplePlan>;

/// 推定結果。
#[derive(Clone, Debug, PartialEq)]
pub struct BeatTrack {
    /// ビートの時刻(秒)
    pub beats: Vec<f32>,
    /// 小節頭の時刻(秒。どれもいずれかのビートに一致する)
    pub downbeats: Vec<f32>,
}

impl BeatTrack {
    /// テンポ(BPM)。ビートの時刻を「何拍目か」に対して直線で当てはめた傾きから求める
    /// (フレームが 20ms 刻みなので、間隔の中央値より精度が高い。抜けたビートも拍数で数える)。
    /// ビートが 4 つ未満なら `None`。
    pub fn bpm(&self) -> Option<f32> {
        let (slope, _) = self.fit(0, self.beats.len())?;
        Some(60.0 / slope)
    }

    /// テンポの揺れ: 16 拍ずつの区間で直線からのずれ(RMS)を拍の長さで割り、その中央値。
    /// 打ち込みなら 0.015 未満(フレームの刻みによる誤差を含む)、人の演奏は 0.02〜0.05 程度。
    pub fn tempo_variation(&self) -> Option<f32> {
        const W: usize = 16;
        let n = self.beats.len();
        if n < 4 {
            return None;
        }
        let mut v: Vec<f32> = (0..n.saturating_sub(W).max(1))
            .step_by(W / 2)
            .filter_map(|s| {
                let (slope, rms) = self.fit(s, (s + W).min(n))?;
                Some(rms / slope)
            })
            .collect();
        if v.is_empty() {
            return None;
        }
        Some(median(&mut v))
    }

    /// `beats[from..to]` を拍番号に対する直線で当てはめ、(1 拍の秒数, ずれの RMS 秒) を返す。
    fn fit(&self, from: usize, to: usize) -> Option<(f32, f32)> {
        let t = &self.beats[from..to];
        if t.len() < 4 {
            return None;
        }
        let mut ibi: Vec<f32> = t.windows(2).map(|w| w[1] - w[0]).collect();
        let m = median(&mut ibi);
        if m <= 0.0 {
            return None;
        }
        // 拍番号は直前のビートからの間隔で数える(抜けたビートは 2 拍ぶんとして数える)
        let mut k = vec![0.0f64; t.len()];
        for i in 1..t.len() {
            k[i] = k[i - 1] + ((t[i] - t[i - 1]) / m).round().max(1.0) as f64;
        }
        let n = t.len() as f64;
        let (mk, mt) = (
            k.iter().sum::<f64>() / n,
            t.iter().map(|&x| x as f64).sum::<f64>() / n,
        );
        let (mut sxy, mut sxx) = (0.0, 0.0);
        for (ki, &ti) in k.iter().zip(t) {
            sxy += (ki - mk) * (ti as f64 - mt);
            sxx += (ki - mk) * (ki - mk);
        }
        if sxx <= 0.0 {
            return None;
        }
        let slope = sxy / sxx;
        let rms = (k
            .iter()
            .zip(t)
            .map(|(ki, &ti)| (ti as f64 - (mt + slope * (ki - mk))).powi(2))
            .sum::<f64>()
            / n)
            .sqrt();
        (slope > 0.0).then_some((slope as f32, rms as f32))
    }

    /// 1 小節のビート数(小節頭の間のビート数の最頻値)。小節頭が 2 つ未満なら `None`。
    pub fn beats_per_bar(&self) -> Option<u32> {
        if self.downbeats.len() < 2 {
            return None;
        }
        let mut counts = [0u32; 17];
        for w in self.downbeats.windows(2) {
            let n = self
                .beats
                .iter()
                .filter(|&&b| b >= w[0] - 1e-3 && b < w[1] - 1e-3)
                .count();
            if (1..counts.len()).contains(&n) {
                counts[n] += 1;
            }
        }
        let (n, c) = counts
            .iter()
            .enumerate()
            .max_by_key(|(_, c)| **c)
            .unwrap_or((0, &0));
        (*c > 0).then_some(n as u32)
    }
}

fn median(v: &mut [f32]) -> f32 {
    v.sort_by(f32::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

fn load_plan(frames: usize) -> TractResult<Plan> {
    tract_onnx::onnx()
        .model_for_read(&mut std::io::Cursor::new(MODEL_BYTES))?
        .with_input_fact(0, f32::fact([1, frames, N_MELS]).into())?
        .into_optimized()?
        .into_runnable()
}

/// 長さごとの推論計画。30 秒の窓は使い回し、それより短い窓(短い音声・最後の窓)はその都度作る。
fn plan(frames: usize) -> Result<Plan, crate::MlError> {
    if frames == CHUNK {
        static FULL: OnceLock<Result<Plan, String>> = OnceLock::new();
        return FULL
            .get_or_init(|| load_plan(CHUNK).map_err(|e| e.to_string()))
            .clone()
            .map_err(crate::MlError::Model);
    }
    load_plan(frames).map_err(|e| crate::MlError::Model(e.to_string()))
}

/// 任意のサンプルレートのモノラル音声のビート・小節頭を推定する。
pub fn track(frames: &[f32], sample_rate: f32) -> Result<BeatTrack, crate::MlError> {
    let audio = crate::resample(frames, sample_rate, SAMPLE_RATE);
    let mel = log_mel(&audio);
    let (beat, downbeat) = logits(&mel)?;
    let beats = peaks(&beat);
    let mut downbeats = peaks(&downbeat);
    // 小節頭はいちばん近いビートに合わせる(公式と同じ)
    if !beats.is_empty() {
        for d in downbeats.iter_mut() {
            *d = *beats
                .iter()
                .min_by(|a, b| (*a - *d).abs().total_cmp(&(*b - *d).abs()))
                .unwrap_or(d);
        }
        downbeats.dedup();
    }
    Ok(BeatTrack { beats, downbeats })
}

/// 対数メルスペクトログラム(torchaudio の MelSpectrogram と同じ設定: 中心合わせ・反射パディング、
/// 周期ハン窓、√フレーム長で割った振幅、Slaney のメル尺度(面積正規化なし)、log1p(1000x))。
pub fn log_mel(audio: &[f32]) -> Vec<[f32; N_MELS]> {
    if audio.is_empty() {
        return Vec::new();
    }
    let pad = N_FFT / 2;
    // 反射パディング(端の値を含めずに折り返す)
    let reflect = |i: isize| -> f32 {
        let n = audio.len() as isize;
        if n == 1 {
            return audio[0];
        }
        let period = 2 * (n - 1);
        let mut k = i.rem_euclid(period);
        if k >= n {
            k = period - k;
        }
        audio[k as usize]
    };
    let window: Vec<f32> = (0..N_FFT)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / N_FFT as f32).cos())
        .collect();
    // normalized="frame_length" は torch.stft の normalized=True と同じで √n_fft で割る
    let norm = (N_FFT as f32).sqrt();
    let fb = mel_filterbank();
    let fft = FftPlanner::new().plan_fft_forward(N_FFT);
    let n_frames = 1 + audio.len() / HOP;
    let mut buf = vec![Complex::new(0.0f32, 0.0); N_FFT];
    let mut mag = vec![0.0f32; N_FFT / 2 + 1];
    (0..n_frames)
        .map(|t| {
            let start = (t * HOP) as isize - pad as isize;
            for (i, b) in buf.iter_mut().enumerate() {
                *b = Complex::new(reflect(start + i as isize) * window[i], 0.0);
            }
            fft.process(&mut buf);
            for (m, c) in mag.iter_mut().zip(&buf) {
                *m = c.norm() / norm;
            }
            let mut row = [0.0f32; N_MELS];
            for (r, (lo, weights)) in row.iter_mut().zip(&fb) {
                let e: f32 = weights.iter().zip(&mag[*lo..]).map(|(w, m)| w * m).sum();
                *r = (LOG_MULTIPLIER * e).ln_1p();
            }
            row
        })
        .collect()
}

/// メルフィルタバンク(帯域ごとに、最初のビン番号と重みの列)。torchaudio の melscale_fbanks と同じ。
fn mel_filterbank() -> Vec<(usize, Vec<f32>)> {
    let hz_to_mel = |f: f64| {
        if f < 1000.0 {
            3.0 * f / 200.0
        } else {
            15.0 + (f / 1000.0).ln() * 27.0 / 6.4f64.ln()
        }
    };
    let mel_to_hz = |m: f64| {
        if m < 15.0 {
            200.0 * m / 3.0
        } else {
            1000.0 * ((m - 15.0) * 6.4f64.ln() / 27.0).exp()
        }
    };
    let n_freqs = N_FFT / 2 + 1;
    let freqs: Vec<f64> = (0..n_freqs)
        .map(|i| i as f64 * (SAMPLE_RATE as f64 / 2.0) / (n_freqs - 1) as f64)
        .collect();
    let (m0, m1) = (hz_to_mel(F_MIN as f64), hz_to_mel(F_MAX as f64));
    let pts: Vec<f64> = (0..N_MELS + 2)
        .map(|i| mel_to_hz(m0 + (m1 - m0) * i as f64 / (N_MELS + 1) as f64))
        .collect();
    (0..N_MELS)
        .map(|m| {
            let w: Vec<f32> = freqs
                .iter()
                .map(|&f| {
                    let down = (f - pts[m]) / (pts[m + 1] - pts[m]);
                    let up = (pts[m + 2] - f) / (pts[m + 2] - pts[m + 1]);
                    down.min(up).max(0.0) as f32
                })
                .collect();
            let lo = w.iter().position(|v| *v > 0.0).unwrap_or(0);
            let hi = w.iter().rposition(|v| *v > 0.0).map_or(lo, |h| h + 1);
            (lo, w[lo..hi].to_vec())
        })
        .collect()
}

/// フレームごとのビート・小節頭のロジット。
fn logits(mel: &[[f32; N_MELS]]) -> Result<(Vec<f32>, Vec<f32>), crate::MlError> {
    let total = mel.len();
    let mut beat = vec![-1000.0f32; total];
    let mut down = vec![-1000.0f32; total];
    if total == 0 {
        return Ok((beat, down));
    }
    // 窓の開始位置(公式の split_predict_aggregate と同じ。最後の窓は末尾にそろえる)
    let stride = (CHUNK - 2 * BORDER) as isize;
    let mut starts: Vec<isize> = Vec::new();
    let mut pos = -(BORDER as isize);
    while pos < total as isize - BORDER as isize {
        starts.push(pos);
        pos += stride;
    }
    if total > stride as usize {
        if let Some(last) = starts.last_mut() {
            *last = total as isize - (CHUNK - BORDER) as isize;
        }
    }
    // 窓ごとの推論は独立なので並列に行う(tract は 1 スレッドで動くため)
    let run = |start: isize| -> Result<(isize, Vec<f32>, Vec<f32>), crate::MlError> {
        let from = start.max(0) as usize;
        let to = ((start + CHUNK as isize) as usize).min(total);
        let pad_left = (-start).max(0) as usize;
        let pad_right =
            (start + CHUNK as isize - total as isize).clamp(0, BORDER as isize) as usize;
        let len = pad_left + (to - from) + pad_right;
        let mut data = vec![0.0f32; len * N_MELS];
        for (k, row) in mel[from..to].iter().enumerate() {
            let d = (pad_left + k) * N_MELS;
            data[d..d + N_MELS].copy_from_slice(row);
        }
        let input: Tensor = tract_ndarray::Array3::from_shape_vec((1, len, N_MELS), data)
            .map_err(|e| crate::MlError::Inference(e.to_string()))?
            .into();
        let out = plan(len)?
            .run(tvec!(input.into()))
            .map_err(|e| crate::MlError::Inference(e.to_string()))?;
        let view = |i: usize| -> Result<Vec<f32>, crate::MlError> {
            Ok(out[i]
                .to_plain_array_view::<f32>()
                .map_err(|e| crate::MlError::Inference(e.to_string()))?
                .iter()
                .copied()
                .collect())
        };
        Ok((start, view(0)?, view(1)?))
    };
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 8);
    let mut results = Vec::with_capacity(starts.len());
    for group in starts.chunks(threads) {
        let part: Vec<_> = std::thread::scope(|sc| {
            let handles: Vec<_> = group.iter().map(|&st| sc.spawn(move || run(st))).collect();
            handles
                .into_iter()
                .map(|h| {
                    h.join().unwrap_or_else(|_| {
                        Err(crate::MlError::Inference("推論が中断しました".into()))
                    })
                })
                .collect()
        });
        results.extend(part);
    }
    // 後ろの窓から書き、前の窓で上書きする(公式と同じ優先順位)
    for r in results.into_iter().rev() {
        let (start, b, d) = r?;
        for k in BORDER..b.len() - BORDER {
            let dest = start + k as isize;
            if (0..total as isize).contains(&dest) {
                beat[dest as usize] = b[k];
                down[dest as usize] = d[k];
            }
        }
    }
    Ok((beat, down))
}

/// ロジット > 0 かつ前後 3 フレームの最大の点を拾い、隣り合う点はまとめて平均する(秒)。
fn peaks(logits: &[f32]) -> Vec<f32> {
    let n = logits.len();
    let idx: Vec<usize> = (0..n)
        .filter(|&i| {
            logits[i] > 0.0
                && logits[i.saturating_sub(3)..(i + 4).min(n)]
                    .iter()
                    .all(|v| *v <= logits[i])
        })
        .collect();
    let mut out = Vec::new();
    let mut it = idx.into_iter();
    let Some(first) = it.next() else {
        return out;
    };
    let (mut p, mut c) = (first as f64, 1.0f64);
    for q in it {
        let q = q as f64;
        if q - p <= 1.0 {
            c += 1.0;
            p += (q - p) / c;
        } else {
            out.push((p / FPS as f64) as f32);
            p = q;
            c = 1.0;
        }
    }
    out.push((p / FPS as f64) as f32);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 120BPM・4/4 のドラム風の音(小節頭にキック、2・4 拍にスネア、8 分でハイハット)
    fn drum_loop(bpm: f32, bars: usize, sr: f32) -> Vec<f32> {
        let beat = 60.0 / bpm;
        let n = (bars as f32 * 4.0 * beat * sr) as usize;
        let mut x = vec![0.0f32; n];
        let mut seed = 1u32;
        let mut noise = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        };
        for k in 0..bars * 8 {
            let s = (k as f32 * beat / 2.0 * sr) as usize;
            let on_beat = k % 2 == 0;
            let pos = (k / 2) % 4;
            for i in 0..(0.25 * sr) as usize {
                if s + i >= n {
                    break;
                }
                let t = i as f32 / sr;
                let mut v = 0.15 * noise() * (-t * 60.0).exp(); // ハイハット
                if on_beat && pos == 0 {
                    // キック: 下がっていくサイン
                    v += 0.9
                        * (std::f32::consts::TAU * (50.0 + 100.0 * (-t * 30.0).exp()) * t).sin()
                        * (-t * 12.0).exp();
                }
                if on_beat && (pos == 1 || pos == 3) {
                    v += 0.5 * noise() * (-t * 20.0).exp(); // スネア
                }
                if on_beat && pos == 2 {
                    v += 0.6 * (std::f32::consts::TAU * 60.0 * t).sin() * (-t * 12.0).exp();
                }
                x[s + i] += v;
            }
        }
        x
    }

    #[test]
    fn detects_tempo_of_drum_loop() {
        // 20ms 刻みに乗らないテンポでも小数まで合う
        let x = drum_loop(128.0, 12, 48_000.0);
        let t = track(&x, 48_000.0).unwrap();
        let bpm = t.bpm().unwrap();
        assert!((bpm - 128.0).abs() < 0.3, "{bpm} {:?}", t.beats);
        assert!(
            t.tempo_variation().unwrap() < 0.015,
            "{:?}",
            t.tempo_variation()
        );
        eprintln!("128: 小節 {:?} {:?}", t.beats_per_bar(), t.downbeats);

        let x = drum_loop(120.0, 12, 44_100.0);
        let t = track(&x, 44_100.0).unwrap();
        let bpm = t.bpm().unwrap();
        assert!((bpm - 120.0).abs() < 0.3, "{bpm} {:?}", t.beats);
        // ビートは 0.5 秒の格子の上
        for b in &t.beats {
            let off = (b / 0.5 - (b / 0.5).round()).abs() * 0.5;
            assert!(off < 0.04, "{b}");
        }
    }

    #[test]
    fn empty_and_silent_audio_have_no_beats() {
        assert!(track(&[], 44_100.0).unwrap().beats.is_empty());
        let t = track(&vec![0.0; 44_100 * 3], 44_100.0).unwrap();
        assert!(t.beats.len() < 2);
        assert_eq!(t.bpm(), None);
    }

    #[test]
    fn peaks_merge_neighbors() {
        let mut l = vec![-5.0f32; 40];
        l[10] = 2.0;
        l[11] = 2.0;
        l[30] = 1.0;
        let p = peaks(&l);
        assert_eq!(p.len(), 2);
        assert!((p[0] - 10.5 / FPS).abs() < 1e-6);
    }
}
