//! 単音の音程推定: SwiftF0(MIT、約 9.6 万パラメータ)。
//!
//! 16kHz のモノラル音声から、16ms(256 サンプル)ごとに音程(Hz、46.875〜2093.75)と
//! 確からしさ(0〜1。0.5 以上を有声とみなすよう較正済み)を返す。CPU で非常に速く、
//! 雑音下でも YIN より頑健。モデルは可変長入力だが、tract で 1 回だけ最適化するため
//! 固定長(2 秒 + 前後の文脈)に区切って推論し、つなぎ合わせる(有声フレームは一括推論と一致)。

use std::sync::OnceLock;
use tract_onnx::prelude::*;

static MODEL_BYTES: &[u8] = include_bytes!("../models/swiftf0.onnx");

pub const SAMPLE_RATE: f32 = 16_000.0;
const HOP: usize = 256;
/// 1 回に推論する長さ(サンプル)と、前後に付ける文脈(モデルが見る範囲)
const CHUNK: usize = 32_000;
const LEFT: usize = 11 * HOP;
const RIGHT: usize = 10 * HOP;
const FMIN: f32 = 46.875;
const FMAX: f32 = 2093.75;
/// 無音とみなすピーク
const SILENCE_PEAK: f32 = 1e-3;

/// 1 フレーム(16ms)の推定。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PitchEstimate {
    /// フレームの時刻(秒)
    pub time: f32,
    pub f0: f32,
    /// 確からしさ(0.5 以上で有声)
    pub confidence: f32,
}

type Plan = std::sync::Arc<TypedSimplePlan>;

fn model() -> Result<&'static Plan, crate::MlError> {
    static M: OnceLock<Result<Plan, String>> = OnceLock::new();
    M.get_or_init(|| {
        (|| -> TractResult<Plan> {
            tract_onnx::onnx()
                .model_for_read(&mut std::io::Cursor::new(MODEL_BYTES))?
                .with_input_fact(0, f32::fact([1, LEFT + CHUNK + RIGHT]).into())?
                .with_input_fact(1, f32::fact(Vec::<usize>::new()).into())?
                .with_input_fact(2, f32::fact(Vec::<usize>::new()).into())?
                .into_optimized()?
                .into_runnable()
        })()
        .map_err(|e| e.to_string())
    })
    .as_ref()
    .map_err(|e| crate::MlError::Model(e.clone()))
}

/// 任意のサンプルレートのモノラル音声の音程を推定する。
pub fn track(frames: &[f32], sample_rate: f32) -> Result<Vec<PitchEstimate>, crate::MlError> {
    let audio = crate::resample(frames, sample_rate, SAMPLE_RATE);
    track_16k(&audio)
}

/// 16kHz のモノラル音声の音程を推定する。
pub fn track_16k(audio: &[f32]) -> Result<Vec<PitchEstimate>, crate::MlError> {
    let m = model()?;
    let n_frames = audio.len().div_ceil(HOP);
    let mut padded = vec![0.0f32; LEFT];
    padded.extend_from_slice(audio);
    padded.resize(LEFT + audio.len() + CHUNK + RIGHT, 0.0);
    let per_chunk = CHUNK / HOP;
    let mut out = Vec::with_capacity(n_frames);
    let mut start = 0;
    while start < audio.len() {
        let seg = padded[start..start + LEFT + CHUNK + RIGHT].to_vec();
        let input: Tensor = tract_ndarray::Array2::from_shape_vec((1, seg.len()), seg)
            .map_err(|e| crate::MlError::Inference(e.to_string()))?
            .into();
        let fmin: Tensor = tract_ndarray::arr0(FMIN).into();
        let fmax: Tensor = tract_ndarray::arr0(FMAX).into();
        let r = m
            .run(tvec!(input.into(), fmin.into(), fmax.into()))
            .map_err(|e| crate::MlError::Inference(e.to_string()))?;
        // pitch は float64 で出てくる
        let pitch = r[0]
            .cast_to::<f32>()
            .map_err(|e| crate::MlError::Inference(e.to_string()))?;
        let pitch = pitch
            .to_plain_array_view::<f32>()
            .map_err(|e| crate::MlError::Inference(e.to_string()))?;
        let conf = r[1]
            .to_plain_array_view::<f32>()
            .map_err(|e| crate::MlError::Inference(e.to_string()))?;
        let skip = LEFT / HOP;
        for k in 0..per_chunk {
            let idx = out.len();
            if idx >= n_frames {
                break;
            }
            let hop_start = idx * HOP;
            let hop_end = (hop_start + HOP).min(audio.len());
            let silent = audio[hop_start..hop_end]
                .iter()
                .all(|v| v.abs() < SILENCE_PEAK);
            out.push(PitchEstimate {
                time: idx as f32 * HOP as f32 / SAMPLE_RATE,
                f0: pitch[[0, skip + k]],
                confidence: if silent { 0.0 } else { conf[[0, skip + k]] },
            });
        }
        start += CHUNK;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_reference_on_vibrato_tone() {
        // 220Hz に 5Hz・±30 セントのビブラート、3 秒(2 チャンクにまたがる)。
        // 一括推論(onnxruntime)との一致はモデルの同梱時に確認済み
        let sr = 16_000.0f32;
        let n = (sr * 3.0) as usize;
        let mut phase = 0.0f64;
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f64 / sr as f64;
                let f = 220.0 * 2f64.powf(30.0 * (std::f64::consts::TAU * 5.0 * t).sin() / 1200.0);
                phase += std::f64::consts::TAU * f / sr as f64;
                // 倍音を含む音(1/k で減る 6 倍音まで)
                (0.2 * (1..=6)
                    .map(|k| (phase * k as f64).sin() / k as f64)
                    .sum::<f64>()) as f32
            })
            .collect();
        let est = track_16k(&x).unwrap();
        assert_eq!(est.len(), n.div_ceil(HOP));
        let voiced: Vec<&PitchEstimate> = est.iter().filter(|e| e.confidence >= 0.5).collect();
        assert!(voiced.len() > est.len() * 8 / 10, "ほとんど有声");
        for e in &voiced {
            let cents = 1200.0 * (e.f0 / 220.0).log2();
            assert!(cents.abs() < 45.0, "ビブラートの範囲内: {} Hz", e.f0);
        }
        // ±30 セントの山と谷(223.9Hz / 216.2Hz)が捉えられている
        let first: Vec<f32> = est[1..20].iter().map(|e| e.f0).collect();
        let hi = first.iter().cloned().fold(0.0, f32::max);
        let lo = first.iter().cloned().fold(f32::MAX, f32::min);
        assert!(
            (hi - 223.9).abs() < 1.5 && (lo - 216.2).abs() < 1.5,
            "{lo}〜{hi}"
        );
    }
}
