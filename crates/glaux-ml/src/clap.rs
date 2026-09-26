//! 音を言葉で捉える: LAION-CLAP(`laion/larger_clap_music_and_speech`、Apache-2.0)。
//!
//! 音と言葉を同じ 512 次元の空間に写すモデルの **音声側だけ** を tract で動かし、言葉側は
//! 音色語の辞書(`data/clap_vocab.json`、`scripts/clap_vocab.py` で事前計算)を使う。
//! 音声側のモデルは約 280MB あるので同梱せず、初回に取得したファイルを読む([`model_path`])。
//!
//! 前処理は Hugging Face の ClapFeatureExtractor(truncation = "rand_trunc"、padding = "repeatpad")と同じ:
//! 48kHz、10 秒に満たなければ繰り返して 10 秒に(端数は無音)、中心合わせ・反射パディングの STFT
//! (n_fft 1024、hop 480、周期ハン窓、パワー)、Slaney のメル尺度 50〜14000Hz 64 帯域(面積正規化あり)、
//! 10·log10。10 秒より長い音は、公式のランダムな切り出しの代わりに先頭・中央・末尾の 10 秒の平均を使う。

use rustfft::{num_complex::Complex, FftPlanner};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use tract_onnx::prelude::*;

pub const SAMPLE_RATE: f32 = 48_000.0;
pub const DIM: usize = 512;
const N_FFT: usize = 1024;
const HOP: usize = 480;
const N_MELS: usize = 64;
const F_MIN: f64 = 50.0;
const F_MAX: f64 = 14_000.0;
const MAX_SAMPLES: usize = 480_000;
const N_FRAMES: usize = 1 + MAX_SAMPLES / HOP;

/// 音声側モデルのファイル名(設定ディレクトリの `models/` に置く)。
pub const MODEL_FILE: &str = "clap_audio.onnx";
/// 取得元(Xenova による ONNX 変換。リビジョン固定)と、その SHA-256・大きさ。
pub const MODEL_URL: &str = "https://huggingface.co/Xenova/larger_clap_music_and_speech/resolve/e9fd5ac1dbf3280936a7fc3ec8a020453ff184db/onnx/audio_model.onnx";
pub const MODEL_SHA256: &str = "3ecc72d27740e2a09ced20cf22fd6244122e5e506008763a0f368b3b4ff6eac8";
pub const MODEL_BYTES: u64 = 281_749_092;

static VOCAB_JSON: &str = include_str!("../data/clap_vocab.json");

type Plan = Arc<TypedSimplePlan>;

/// 音声側モデルの置き場所。`GLAUX_CLAP_MODEL` があればそのファイル、
/// 無ければ設定ディレクトリ(`%APPDATA%\glaux\models` / `~/.config/glaux/models`)。
pub fn model_path() -> PathBuf {
    if let Some(p) = std::env::var_os("GLAUX_CLAP_MODEL") {
        return PathBuf::from(p);
    }
    let base = std::env::var("APPDATA")
        .or_else(|_| std::env::var("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("USERPROFILE")
                .or_else(|_| std::env::var("HOME"))
                .unwrap_or_else(|_| ".".to_owned());
            PathBuf::from(home).join(".config")
        });
    base.join("glaux").join("models").join(MODEL_FILE)
}

/// 音声側モデルが使えるか(ファイルがあるか)。
pub fn available() -> bool {
    model_path().is_file()
}

fn plan() -> Result<Plan, crate::MlError> {
    // 置き場所が変わったら(テストで環境変数を切り替えた等)読み直す
    static CACHE: OnceLock<Mutex<Option<(PathBuf, Plan)>>> = OnceLock::new();
    let path = model_path();
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| crate::MlError::Model("CLAP モデルの読み込み中に失敗しました".into()))?;
    if let Some((p, plan)) = cache.as_ref() {
        if *p == path {
            return Ok(plan.clone());
        }
    }
    if !path.is_file() {
        return Err(crate::MlError::Model(format!(
            "CLAP の音声モデルがありません({})",
            path.display()
        )));
    }
    let plan = load_plan(&path).map_err(|e| crate::MlError::Model(e.to_string()))?;
    *cache = Some((path, plan.clone()));
    Ok(plan)
}

fn load_plan(path: &Path) -> TractResult<Plan> {
    tract_onnx::onnx()
        .model_for_path(path)?
        .with_input_fact(0, f32::fact([1, 1, N_FRAMES, N_MELS]).into())?
        .into_optimized()?
        .into_runnable()
}

/// 音の埋め込み(512 次元、長さ 1)。
pub fn embed(frames: &[f32], sample_rate: f32) -> Result<Vec<f32>, crate::MlError> {
    let audio = crate::resample(frames, sample_rate, SAMPLE_RATE);
    if audio.is_empty() {
        return Err(crate::MlError::Inference("音が空です".into()));
    }
    let windows: Vec<&[f32]> = if audio.len() <= MAX_SAMPLES {
        vec![&audio[..]]
    } else {
        let last = audio.len() - MAX_SAMPLES;
        let mut starts = vec![0, last / 2, last];
        starts.dedup();
        starts
            .into_iter()
            .map(|s| &audio[s..s + MAX_SAMPLES])
            .collect()
    };
    let plan = plan()?;
    let mut sum = vec![0.0f32; DIM];
    for w in windows {
        let mel = log_mel(&repeat_pad(w));
        let input = Tensor::from_shape(&[1, 1, N_FRAMES, N_MELS], &mel)
            .map_err(|e| crate::MlError::Inference(e.to_string()))?;
        let out = plan
            .run(tvec!(input.into()))
            .map_err(|e| crate::MlError::Inference(e.to_string()))?;
        let v = out[0]
            .to_plain_array_view::<f32>()
            .map_err(|e| crate::MlError::Inference(e.to_string()))?;
        let e: Vec<f32> = v.iter().copied().collect();
        let n = norm(&e);
        for (s, x) in sum.iter_mut().zip(&e) {
            *s += x / n;
        }
    }
    let n = norm(&sum);
    Ok(sum.into_iter().map(|x| x / n).collect())
}

fn norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12)
}

/// 2 つの埋め込みのコサイン類似度(-1〜1。同じ音で 1)。
pub fn similarity(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>() / (norm(a) * norm(b))
}

/// 10 秒に満たなければ丸ごと繰り返し、残りを無音で埋める(公式の repeatpad)。
fn repeat_pad(x: &[f32]) -> Vec<f32> {
    let mut out = Vec::with_capacity(MAX_SAMPLES);
    let n = MAX_SAMPLES / x.len();
    for _ in 0..n.max(1) {
        out.extend_from_slice(x);
    }
    out.truncate(MAX_SAMPLES);
    out.resize(MAX_SAMPLES, 0.0);
    out
}

/// 対数メルスペクトログラム([フレーム × 64] を平たく並べたもの)。
fn log_mel(audio: &[f32]) -> Vec<f32> {
    let pad = N_FFT / 2;
    let n = audio.len() as isize;
    let reflect = |i: isize| -> f64 {
        let period = 2 * (n - 1);
        let mut k = i.rem_euclid(period.max(1));
        if k >= n {
            k = period - k;
        }
        audio[k as usize] as f64
    };
    let window: Vec<f64> = (0..N_FFT)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / N_FFT as f64).cos())
        .collect();
    let fb = mel_filterbank();
    let fft = FftPlanner::<f64>::new().plan_fft_forward(N_FFT);
    let n_frames = 1 + audio.len() / HOP;
    let mut buf = vec![Complex::new(0.0f64, 0.0); N_FFT];
    let mut power = vec![0.0f64; N_FFT / 2 + 1];
    let mut out = Vec::with_capacity(n_frames * N_MELS);
    for t in 0..n_frames {
        let start = (t * HOP) as isize - pad as isize;
        for (i, b) in buf.iter_mut().enumerate() {
            *b = Complex::new(reflect(start + i as isize) * window[i], 0.0);
        }
        fft.process(&mut buf);
        for (p, c) in power.iter_mut().zip(&buf) {
            *p = c.norm_sqr();
        }
        for (lo, weights) in &fb {
            let e: f64 = weights.iter().zip(&power[*lo..]).map(|(w, p)| w * p).sum();
            out.push((10.0 * e.max(1e-10).log10()) as f32);
        }
    }
    out
}

/// Slaney のメル尺度・面積正規化のフィルタバンク(librosa / transformers の mel_filter_bank と同じ)。
fn mel_filterbank() -> Vec<(usize, Vec<f64>)> {
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
    let (m0, m1) = (hz_to_mel(F_MIN), hz_to_mel(F_MAX));
    let pts: Vec<f64> = (0..N_MELS + 2)
        .map(|i| mel_to_hz(m0 + (m1 - m0) * i as f64 / (N_MELS + 1) as f64))
        .collect();
    (0..N_MELS)
        .map(|m| {
            let enorm = 2.0 / (pts[m + 2] - pts[m]);
            let w: Vec<f64> = freqs
                .iter()
                .map(|&f| {
                    let down = (f - pts[m]) / (pts[m + 1] - pts[m]);
                    let up = (pts[m + 2] - f) / (pts[m + 2] - pts[m + 1]);
                    down.min(up).max(0.0) * enorm
                })
                .collect();
            let lo = w.iter().position(|v| *v > 0.0).unwrap_or(0);
            let hi = w.iter().rposition(|v| *v > 0.0).map_or(lo, |h| h + 1);
            (lo, w[lo..hi].to_vec())
        })
        .collect()
}

// ---- 音色語 ----

/// 音色語の辞書の 1 語。
pub struct VocabWord {
    /// instrument / tone / texture / envelope / movement / space / mood
    pub category: String,
    pub en: String,
    pub ja: String,
    embedding: Vec<f32>,
    ref_mean: f32,
    ref_std: f32,
}

#[derive(serde::Deserialize)]
struct VocabFile {
    entries: Vec<VocabEntry>,
}

#[derive(serde::Deserialize)]
struct VocabEntry {
    category: String,
    en: String,
    ja: String,
    ref_mean: f32,
    ref_std: f32,
    scale: f32,
    q: String,
}

/// 同梱の音色語の辞書。
pub fn vocab() -> &'static [VocabWord] {
    static V: OnceLock<Vec<VocabWord>> = OnceLock::new();
    V.get_or_init(|| {
        use base64::Engine;
        let file: VocabFile = match serde_json::from_str(VOCAB_JSON) {
            Ok(f) => f,
            Err(_) => return Vec::new(),
        };
        file.entries
            .into_iter()
            .filter_map(|e| {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(e.q.as_bytes())
                    .ok()?;
                let v: Vec<f32> = bytes.iter().map(|b| *b as i8 as f32 * e.scale).collect();
                let n = norm(&v);
                (v.len() == DIM).then(|| VocabWord {
                    category: e.category,
                    en: e.en,
                    ja: e.ja,
                    embedding: v.into_iter().map(|x| x / n).collect(),
                    ref_mean: e.ref_mean,
                    ref_std: e.ref_std.max(1e-3),
                })
            })
            .collect()
    })
}

/// 音色語との近さ。
#[derive(Clone, Debug, serde::Serialize)]
pub struct WordScore {
    pub category: String,
    pub en: String,
    pub ja: String,
    /// コサイン類似度
    pub similarity: f32,
    /// その語が「いろいろな音」に対して出す値と比べた近さ(標準偏差の何倍か)。並べ替えはこちら
    pub z: f32,
}

/// 音色語を英語か日本語で探す(大文字小文字は区別しない)。無ければ None
pub fn find_word(word: &str) -> Option<&'static VocabWord> {
    let w = word.trim().to_lowercase();
    vocab()
        .iter()
        .find(|v| v.en.to_lowercase() == w || v.ja == word.trim())
}

impl VocabWord {
    /// 埋め込みとこの語の近さ(いろいろな音に対して出す値と比べた、標準偏差の何倍か)
    pub fn z(&self, embedding: &[f32]) -> f32 {
        (similarity(embedding, &self.embedding) - self.ref_mean) / self.ref_std
    }
}

/// 埋め込みを音色語の辞書と比べ、カテゴリごとに近い順に `per_category` 語ずつ返す。
pub fn describe(embedding: &[f32], per_category: usize) -> Vec<WordScore> {
    let mut scores: Vec<WordScore> = vocab()
        .iter()
        .map(|w| {
            let s = similarity(embedding, &w.embedding);
            WordScore {
                category: w.category.clone(),
                en: w.en.clone(),
                ja: w.ja.clone(),
                similarity: s,
                z: (s - w.ref_mean) / w.ref_std,
            }
        })
        .collect();
    scores.sort_by(|a, b| a.category.cmp(&b.category).then(b.z.total_cmp(&a.z)));
    let mut out: Vec<WordScore> = Vec::new();
    for s in scores {
        if out.iter().filter(|o| o.category == s.category).count() < per_category {
            out.push(s);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vocab_is_complete() {
        let v = vocab();
        assert!(v.len() >= 100, "{}", v.len());
        for w in v {
            assert!((norm(&w.embedding) - 1.0).abs() < 1e-4);
        }
        // 違う語は違う向き
        let kick = v.iter().find(|w| w.en == "a kick drum").unwrap();
        let flute = v.iter().find(|w| w.en == "a flute").unwrap();
        assert!(similarity(&kick.embedding, &flute.embedding) < 0.9);
    }

    #[test]
    fn mel_has_expected_shape_and_range() {
        let sr = SAMPLE_RATE as usize;
        let x: Vec<f32> = (0..sr)
            .map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / SAMPLE_RATE).sin() * 0.5)
            .collect();
        let m = log_mel(&repeat_pad(&x));
        assert_eq!(m.len(), N_FRAMES * N_MELS);
        // 1kHz の帯域がいちばん大きい
        let row = &m[10 * N_MELS..11 * N_MELS];
        let peak = row
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        let (m0, m1) = (0.75f64, 15.0 + (14.0f64).ln() * 27.0 / 6.4f64.ln());
        let center_mel = |k: usize| m0 + (m1 - m0) * (k + 1) as f64 / 65.0;
        let mel_1k = 15.0;
        assert!((center_mel(peak) - mel_1k).abs() < 0.6, "{peak}");
    }

    /// 実モデルで確かめる(`GLAUX_CLAP_MODEL` 未設定なら何もしない)。
    #[test]
    fn real_model_names_basic_sounds() {
        if std::env::var_os("GLAUX_CLAP_MODEL").is_none() {
            eprintln!("GLAUX_CLAP_MODEL が未設定のためスキップ");
            return;
        }
        let sr = SAMPLE_RATE;
        let n = (sr * 2.0) as usize;
        let kick: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / sr;
                (std::f32::consts::TAU * (50.0 * t + 100.0 * (1.0 - (-t * 30.0).exp()) / 30.0))
                    .sin()
                    * (-t * 8.0).exp()
            })
            .collect();
        let e = embed(&kick, sr).unwrap();
        assert_eq!(e.len(), DIM);
        let words = describe(&e, 3);
        let inst: Vec<&str> = words
            .iter()
            .filter(|w| w.category == "instrument")
            .map(|w| w.ja.as_str())
            .collect();
        eprintln!("{inst:?}");
        assert!(inst.contains(&"キック"), "{inst:?}");
        // 同じ音は類似度 1、違う音は低い
        let sine: Vec<f32> = (0..n)
            .map(|i| (std::f32::consts::TAU * 880.0 * i as f32 / sr).sin() * 0.3)
            .collect();
        let e2 = embed(&sine, sr).unwrap();
        assert!(similarity(&e, &e) > 0.999);
        assert!(similarity(&e, &e2) < 0.9);
    }
}
