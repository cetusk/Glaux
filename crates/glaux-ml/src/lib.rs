//! # glaux-ml
//!
//! 学習済みモデルによる音声解析。いまは **basic-pitch**(Spotify、Apache-2.0)による
//! 和音対応の譜起こし(ポリフォニック)だけを持つ。
//!
//! - モデル(`models/basic_pitch_nmp.onnx`、約 230KB)はバイナリに埋め込む
//! - 推論は pure Rust の ONNX ランタイム `tract`(ネイティブ DLL 不要)
//! - 前処理・後処理は公式実装(`basic_pitch/inference.py`・`note_creation.py`)を移植:
//!   22.05kHz へのリサンプル → 2 秒窓(重なり 30 フレーム)で推論 → 窓の端を捨てて連結 →
//!   onset のピーク + note の持続でノートを組み立て、残ったエネルギーからも拾う(melodia trick)
//!
//! 単旋律の鼻歌は glaux-engine の YIN(`transcribe`)の方が細かく調整してある。
//! こちらはピアノ・ギターの和音や、伴奏入りの素材から音を拾う用途。

pub mod beats;
pub mod clap;
pub mod pitch;

use std::sync::OnceLock;
use tract_onnx::prelude::*;

static MODEL_BYTES: &[u8] = include_bytes!("../models/basic_pitch_nmp.onnx");

/// モデルの入力サンプルレート
pub const MODEL_SAMPLE_RATE: f32 = 22_050.0;
const FFT_HOP: usize = 256;
/// 1 窓のサンプル数(2 秒 - 1 ホップ)
const AUDIO_N_SAMPLES: usize = 43_844;
/// 1 窓の出力フレーム数
const ANNOT_N_FRAMES: usize = 172;
/// 1 秒あたりのフレーム数(公式は整数 86 で扱う)
const ANNOTATIONS_FPS: f64 = 86.0;
/// 窓の重なり(フレーム)。両端の半分ずつを捨てて連結する
const N_OVERLAP_FRAMES: usize = 30;
const N_PITCHES: usize = 88;
/// 出力の 0 番 = MIDI 21(A0)
const MIDI_OFFSET: u8 = 21;

#[derive(Debug, thiserror::Error)]
pub enum MlError {
    #[error("モデルを読み込めません: {0}")]
    Model(String),
    #[error("推論に失敗しました: {0}")]
    Inference(String),
}

/// 譜起こしのパラメータ(既定値は basic-pitch と同じ)。
#[derive(Clone, Copy, Debug)]
pub struct PolyOptions {
    /// 音の立ち上がりとみなす onset 活性のしきい値(0〜1。下げると音が増える)
    pub onset_threshold: f32,
    /// 音が続いているとみなす note 活性のしきい値(0〜1。下げると音が長く・多くなる)
    pub frame_threshold: f32,
    /// これより短い音は捨てる(秒)
    pub min_note_secs: f32,
    /// 検出する音域(MIDI ノート番号)
    pub min_pitch: u8,
    pub max_pitch: u8,
}

impl Default for PolyOptions {
    fn default() -> Self {
        PolyOptions {
            onset_threshold: 0.5,
            frame_threshold: 0.3,
            min_note_secs: 0.1277,
            min_pitch: MIDI_OFFSET,
            max_pitch: MIDI_OFFSET + N_PITCHES as u8 - 1,
        }
    }
}

/// 検出したノート(秒)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PolyNote {
    pub start_sec: f64,
    pub end_sec: f64,
    pub pitch: u8,
    /// 0〜1(note 活性の平均。ベロシティの目安)
    pub amplitude: f32,
}

/// フレームごとの活性(時間 × 88 鍵)。
pub struct Activations {
    pub note: Vec<[f32; N_PITCHES]>,
    pub onset: Vec<[f32; N_PITCHES]>,
}

struct Model {
    plan: std::sync::Arc<TypedSimplePlan>,
    note_idx: usize,
    onset_idx: usize,
}

fn model() -> Result<&'static Model, MlError> {
    static MODEL: OnceLock<Result<Model, String>> = OnceLock::new();
    MODEL
        .get_or_init(|| load_model().map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|e| MlError::Model(e.clone()))
}

fn load_model() -> TractResult<Model> {
    let inference = tract_onnx::onnx()
        .model_for_read(&mut std::io::Cursor::new(MODEL_BYTES))?
        .with_input_fact(0, f32::fact([1, AUDIO_N_SAMPLES, 1]).into())?;
    // 出力の名前で note / onset を特定する(公式: :1 = note、:2 = onset、:0 = contour)
    let labels: Vec<String> = inference
        .output_outlets()?
        .iter()
        .map(|o| inference.outlet_label(*o).unwrap_or_default().to_owned())
        .collect();
    let find = |suffix: &str| -> TractResult<usize> {
        labels
            .iter()
            .position(|l| l.ends_with(suffix))
            .ok_or_else(|| TractError::msg(format!("出力 {suffix} が見つかりません: {labels:?}")))
    };
    let note_idx = find(":1")?;
    let onset_idx = find(":2")?;
    let plan = inference.into_optimized()?.into_runnable()?;
    Ok(Model {
        plan,
        note_idx,
        onset_idx,
    })
}

/// 22.05kHz のモノラル音声から活性を求める。
pub fn infer(audio: &[f32]) -> Result<Activations, MlError> {
    let m = model()?;
    let overlap_len = N_OVERLAP_FRAMES * FFT_HOP;
    let hop = AUDIO_N_SAMPLES - overlap_len;
    // 先頭に重なりの半分の無音を足し、窓ごとに切り出す(最後の窓は無音で埋める)
    let mut padded = vec![0.0f32; overlap_len / 2];
    padded.extend_from_slice(audio);
    let n_trim = N_OVERLAP_FRAMES / 2;
    let mut note = Vec::new();
    let mut onset = Vec::new();
    let mut start = 0;
    while start < padded.len() {
        let mut window = vec![0.0f32; AUDIO_N_SAMPLES];
        let end = (start + AUDIO_N_SAMPLES).min(padded.len());
        window[..end - start].copy_from_slice(&padded[start..end]);
        let input: Tensor = tract_ndarray::Array3::from_shape_vec((1, AUDIO_N_SAMPLES, 1), window)
            .map_err(|e| MlError::Inference(e.to_string()))?
            .into();
        let out = m
            .plan
            .run(tvec!(input.into()))
            .map_err(|e| MlError::Inference(e.to_string()))?;
        for (idx, dest) in [(m.note_idx, &mut note), (m.onset_idx, &mut onset)] {
            let view = out[idx]
                .to_plain_array_view::<f32>()
                .map_err(|e| MlError::Inference(e.to_string()))?;
            let frames = view.shape()[1];
            for f in n_trim..frames.saturating_sub(n_trim) {
                let mut row = [0.0f32; N_PITCHES];
                for (p, v) in row.iter_mut().enumerate() {
                    *v = view[[0, f, p]];
                }
                dest.push(row);
            }
        }
        start += hop;
    }
    let n_frames = (audio.len() as f64 * ANNOTATIONS_FPS / MODEL_SAMPLE_RATE as f64) as usize;
    note.truncate(n_frames);
    onset.truncate(n_frames);
    Ok(Activations { note, onset })
}

/// 任意のサンプルレートのモノラル音声を譜起こしする。
pub fn transcribe_poly(
    frames: &[f32],
    sample_rate: f32,
    opts: &PolyOptions,
) -> Result<Vec<PolyNote>, MlError> {
    let audio = resample(frames, sample_rate, MODEL_SAMPLE_RATE);
    let act = infer(&audio)?;
    Ok(notes_from_activations(&act, opts))
}

/// フレーム番号 → 秒(窓ごとの端数補正込み。公式 `model_frames_to_time`)。
fn frame_to_secs(frame: usize) -> f64 {
    let hop_secs = FFT_HOP as f64 / MODEL_SAMPLE_RATE as f64;
    let window_offset =
        hop_secs * (ANNOT_N_FRAMES as f64 - AUDIO_N_SAMPLES as f64 / FFT_HOP as f64) + 0.0018;
    let window_number = (frame / ANNOT_N_FRAMES) as f64;
    frame as f64 * hop_secs - window_offset * window_number
}

/// 活性からノートを組み立てる(公式 `output_to_notes_polyphonic`、melodia trick あり)。
pub fn notes_from_activations(act: &Activations, opts: &PolyOptions) -> Vec<PolyNote> {
    let n = act.note.len();
    if n < 3 {
        return vec![];
    }
    let frames = &act.note;
    let lo = opts.min_pitch.saturating_sub(MIDI_OFFSET) as usize;
    let hi = (opts.max_pitch.saturating_sub(MIDI_OFFSET) as usize).min(N_PITCHES - 1);
    let in_range = |p: usize| p >= lo && p <= hi;
    let min_len = (opts.min_note_secs as f64 * ANNOTATIONS_FPS).round() as usize;
    let energy_tol = 11;
    let frame_thresh = opts.frame_threshold;

    // onset の推定を補う: note 活性の立ち上がり(1・2 フレーム差の小さい方)を onset の最大値に合わせる
    let mut onsets = act.onset.clone();
    let mut diff = vec![[0.0f32; N_PITCHES]; n];
    let mut diff_max = 0.0f32;
    for t in 2..n {
        for p in 0..N_PITCHES {
            let d = (frames[t][p] - frames[t - 1][p]).min(frames[t][p] - frames[t - 2][p]);
            let d = d.max(0.0);
            diff[t][p] = d;
            diff_max = diff_max.max(d);
        }
    }
    let onset_max = onsets.iter().flatten().fold(0.0f32, |m, v| m.max(*v));
    if diff_max > 0.0 {
        for t in 0..n {
            for p in 0..N_PITCHES {
                onsets[t][p] = onsets[t][p].max(onset_max * diff[t][p] / diff_max);
            }
        }
    }

    // 時間方向の極大で、しきい値を超えたものを onset とする(新しい順に処理)
    let mut starts: Vec<(usize, usize)> = Vec::new();
    for t in 1..n - 1 {
        for (p, &v) in onsets[t].iter().enumerate() {
            if v >= opts.onset_threshold
                && v > onsets[t - 1][p]
                && v > onsets[t + 1][p]
                && in_range(p)
            {
                starts.push((t, p));
            }
        }
    }
    starts.sort_by(|a, b| b.cmp(a));

    let mut remaining: Vec<[f32; N_PITCHES]> = frames.clone();
    let mut out = Vec::new();
    let mean = |a: usize, b: usize, p: usize| -> f32 {
        if b <= a {
            return 0.0;
        }
        frames[a..b].iter().map(|r| r[p]).sum::<f32>() / (b - a) as f32
    };
    let clear = |remaining: &mut Vec<[f32; N_PITCHES]>, t: usize, p: usize| {
        remaining[t][p] = 0.0;
        if p + 1 < N_PITCHES {
            remaining[t][p + 1] = 0.0;
        }
        if p > 0 {
            remaining[t][p - 1] = 0.0;
        }
    };
    for (start, p) in starts {
        if start >= n - 1 {
            continue;
        }
        let mut i = start + 1;
        let mut k = 0;
        while i < n - 1 && k < energy_tol {
            if remaining[i][p] < frame_thresh {
                k += 1;
            } else {
                k = 0;
            }
            i += 1;
        }
        i -= k;
        if i - start <= min_len {
            continue;
        }
        for t in start..i {
            clear(&mut remaining, t, p);
        }
        out.push((start, i, p, mean(start, i, p)));
    }

    // melodia trick: onset の無い持続音(タイ・スラー等)も、残ったエネルギーの山から拾う
    loop {
        let mut best = (0usize, 0usize, 0.0f32);
        for (t, row) in remaining.iter().enumerate() {
            for (p, v) in row.iter().enumerate() {
                if *v > best.2 && in_range(p) {
                    best = (t, p, *v);
                }
            }
        }
        let (mid, p, v) = best;
        if v <= frame_thresh {
            break;
        }
        remaining[mid][p] = 0.0;
        let mut i = mid + 1;
        let mut k = 0;
        while i < n - 1 && k < energy_tol {
            if remaining[i][p] < frame_thresh {
                k += 1;
            } else {
                k = 0;
            }
            clear(&mut remaining, i, p);
            i += 1;
        }
        let end = i - 1 - k;
        let mut i = mid as isize - 1;
        let mut k = 0isize;
        while i > 0 && k < energy_tol as isize {
            let t = i as usize;
            if remaining[t][p] < frame_thresh {
                k += 1;
            } else {
                k = 0;
            }
            clear(&mut remaining, t, p);
            i -= 1;
        }
        let start = (i + 1 + k).max(0) as usize;
        if end <= start || end - start <= min_len {
            continue;
        }
        out.push((start, end, p, mean(start, end, p)));
    }

    let mut notes: Vec<PolyNote> = out
        .into_iter()
        .map(|(s, e, p, amp)| PolyNote {
            start_sec: frame_to_secs(s),
            end_sec: frame_to_secs(e),
            pitch: p as u8 + MIDI_OFFSET,
            amplitude: amp,
        })
        .collect();
    notes.sort_by(|a, b| {
        a.start_sec
            .total_cmp(&b.start_sec)
            .then(a.pitch.cmp(&b.pitch))
    });
    notes
}

/// 窓付き sinc 補間によるリサンプル(オフライン用)。下げるときは折り返しを防ぐため
/// 低い方のナイキストで帯域制限する。
pub fn resample(input: &[f32], from: f32, to: f32) -> Vec<f32> {
    if (from - to).abs() < 0.5 || input.is_empty() {
        return input.to_vec();
    }
    const ZEROS: f64 = 16.0;
    let ratio = to as f64 / from as f64;
    let cutoff = ratio.min(1.0) * 0.95;
    let half = (ZEROS / cutoff).ceil() as isize;
    let out_len = (input.len() as f64 * ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for j in 0..out_len {
        let center = j as f64 / ratio;
        let c = center.floor() as isize;
        let mut acc = 0.0f64;
        for i in (c - half + 1)..=(c + half) {
            if i < 0 || i as usize >= input.len() {
                continue;
            }
            let x = (i as f64 - center) * cutoff;
            let sinc = if x.abs() < 1e-9 {
                1.0
            } else {
                (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
            };
            // ハン窓(±ZEROS 零交差)
            let w = 0.5 + 0.5 * (std::f64::consts::PI * x / ZEROS).cos();
            if x.abs() < ZEROS {
                acc += input[i as usize] as f64 * sinc * w * cutoff;
            }
        }
        out.push(acc as f32);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 倍音を含み減衰する「ピアノっぽい」音(1 音)
    fn tone(buf: &mut [f32], sr: f32, pitch: u8, start: f32, dur: f32) {
        let f0 = 440.0 * 2f32.powf((pitch as f32 - 69.0) / 12.0);
        let s = (start * sr) as usize;
        let e = ((start + dur) * sr) as usize;
        let e = e.min(buf.len());
        for (i, v) in buf[s..e].iter_mut().enumerate() {
            let t = i as f32 / sr;
            let env = (-t * 1.5).exp() * (1.0 - (-t * 400.0).exp());
            let mut x = 0.0;
            for h in 1..=6 {
                x += (std::f32::consts::TAU * f0 * h as f32 * t).sin() / (h * h) as f32;
            }
            *v += 0.25 * env * x;
        }
    }

    #[test]
    fn resample_keeps_frequency_and_length() {
        let sr = 48_000.0;
        let x: Vec<f32> = (0..48_000)
            .map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / sr).sin())
            .collect();
        let y = resample(&x, sr, MODEL_SAMPLE_RATE);
        assert_eq!(y.len(), 22_050);
        let body = &y[1000..21_000];
        let crossings = (1..body.len())
            .filter(|&i| body[i - 1] < 0.0 && body[i] >= 0.0)
            .count();
        let freq = crossings as f32 * MODEL_SAMPLE_RATE / body.len() as f32;
        assert!((freq - 1000.0).abs() < 5.0, "{freq}");
        let rms = (body.iter().map(|v| v * v).sum::<f32>() / body.len() as f32).sqrt();
        assert!((rms - 0.707).abs() < 0.02, "{rms}");
    }

    #[test]
    fn transcribes_a_chord_and_a_following_note() {
        let sr = 44_100.0;
        let mut buf = vec![0.0f32; (3.0 * sr) as usize];
        // 0〜1.2 秒: C メジャー(C4 E4 G4)、1.4〜2.4 秒: A4 単音
        for p in [60, 64, 67] {
            tone(&mut buf, sr, p, 0.0, 1.2);
        }
        tone(&mut buf, sr, 69, 1.4, 1.0);
        let notes = transcribe_poly(&buf, sr, &PolyOptions::default()).unwrap();
        let chord: Vec<u8> = notes
            .iter()
            .filter(|n| n.start_sec < 0.3)
            .map(|n| n.pitch)
            .collect();
        for p in [60, 64, 67] {
            assert!(chord.contains(&p), "和音の {p} を拾うはず: {notes:?}");
        }
        let a4 = notes
            .iter()
            .find(|n| n.pitch == 69 && (n.start_sec - 1.4).abs() < 0.1)
            .unwrap_or_else(|| panic!("A4 を拾うはず: {notes:?}"));
        assert!(a4.end_sec > 1.8, "A4 はしばらく続く: {a4:?}");
        // 和音の音が後半まで伸びて A4 と重なったりはしない
        assert!(
            notes.iter().all(|n| n.pitch == 69 || n.start_sec < 1.3),
            "余計な音: {notes:?}"
        );
    }
}
