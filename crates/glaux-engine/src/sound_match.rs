//! 似た音を作る: 音色の距離と、内蔵 subtractive のつまみの自動合わせ。
//!
//! 距離(`distance`)は、音量をそろえた 2 音の「多重解像度の対数メルスペクトログラムの差」
//! (auraloss の multi-resolution STFT 距離に倣い、約 11 / 21 / 43ms の 3 つの窓で、対数の L1 と
//! スペクトル収束度)と「音量の包絡(5ms ごとの dB)の差」の和。
//! 窓の長さは秒で決めるので、サンプルレートの違う音どうしも比べられる。
//!
//! 自動合わせ(`fit_instrument`。subtractive / fm、リバーブ込みも)は Instrumental(2026)の構成に倣い、
//! CMA-ES(微分を使わない進化的な最適化)で音源の連続つまみを探す。初期値は音の記述子(立ち上がり・減衰・明るさ・
//! ノイズっぽさ・波形の推定)から決め、波形(4 種)は有望な 2 つだけを探す。候補の音はボイスを直接
//! 鳴らして作る(エフェクト・トラック音量は通さない)ので 1 回数 ms で済み、CMA-ES の 1 世代を並列に評価する。

use crate::timbre::SoundDescriptors;
use glaux_core::{Device, ParamMap, ParamValue};
use glaux_dsp::{bake_instrument, VoiceState};
use rustfft::{num_complex::Complex, FftPlanner};

/// 比べる長さの上限(秒。鳴り始めから)
const MAX_SECS: f32 = 3.0;
const N_MELS: usize = 48;
const F_MIN: f32 = 40.0;
/// 窓の長さ(秒)
const WINDOWS: [f32; 3] = [0.0107, 0.0213, 0.0427];
const ENV_HOP: f32 = 0.005;

/// 比較の準備が済んだ音の特徴。
#[derive(Clone, Debug)]
pub struct Features {
    /// 解像度ごとのメルスペクトログラム(フレーム × N_MELS、音量はそろえ済み)
    mels: Vec<Vec<[f32; N_MELS]>>,
    /// 5ms ごとの音量(dB、最大 0、下限 -60)
    env_db: Vec<f32>,
}

/// 2 音の距離(0 で同じ。1 前後で「かなり違う」)。
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct Distance {
    pub total: f32,
    /// 音色(スペクトルの形とその時間変化)の差
    pub spectral: f32,
    /// 音量の時間変化(立ち上がり・減衰・長さ)の差
    pub envelope: f32,
}

/// 鳴り始め(最大から -40dB を超えた所)から最大 3 秒を切り出す。
pub fn trim_onset(x: &[f32], sr: f32) -> &[f32] {
    let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if peak <= 0.0 {
        return &x[..0];
    }
    let th = peak * 0.01;
    let start = x.iter().position(|v| v.abs() >= th).unwrap_or(0);
    // 立ち上がりの手前を少し残す(5ms)
    let start = start.saturating_sub((0.005 * sr) as usize);
    let end = (start + (MAX_SECS * sr) as usize).min(x.len());
    &x[start..end]
}

/// 特徴を求める(`x` は `trim_onset` 済みを想定)。`f_max` はメル帯域の上限(両者のナイキスト未満にそろえる)。
pub fn features(x: &[f32], sr: f32, f_max: f32) -> Features {
    // 音量は「いちばん大きい 5ms」でそろえる(全体の RMS だと余韻の長さで変わってしまう)
    let hop = ((ENV_HOP * sr) as usize).max(1);
    let loudest = x
        .chunks(hop)
        .map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt())
        .fold(0.0f32, f32::max);
    let g = if loudest > 0.0 { 0.1 / loudest } else { 0.0 };
    let mels = WINDOWS
        .iter()
        .map(|w| {
            // 窓の長さを秒でそろえる(2 のべき乗に丸めるとサンプルレートで窓の長さが変わる)
            let n = ((w * sr).round() as usize).max(64);
            mel_frames(x, g, n, sr, f_max)
        })
        .collect();
    let mut env: Vec<f32> = x
        .chunks(hop)
        .map(|c| {
            let r = (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt();
            20.0 * r.max(1e-9).log10()
        })
        .collect();
    let max = env.iter().cloned().fold(-200.0f32, f32::max);
    for e in env.iter_mut() {
        *e = (*e - max).max(-60.0);
    }
    Features { mels, env_db: env }
}

fn mel_frames(x: &[f32], gain: f32, n: usize, sr: f32, f_max: f32) -> Vec<[f32; N_MELS]> {
    let hop = n / 4;
    let fft = FftPlanner::<f32>::new().plan_fft_forward(n);
    let window: Vec<f32> = (0..n)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos())
        .collect();
    // 振幅を窓の和で割り、窓の長さによらない大きさにする
    let wsum: f32 = window.iter().sum();
    let fb = mel_bank(n, sr, f_max);
    let mut buf = vec![Complex::new(0.0f32, 0.0); n];
    let frames = x.len().div_ceil(hop).max(1);
    (0..frames)
        .map(|t| {
            for (i, b) in buf.iter_mut().enumerate() {
                let v = x.get(t * hop + i).copied().unwrap_or(0.0) * gain;
                *b = Complex::new(v * window[i], 0.0);
            }
            fft.process(&mut buf);
            let mut row = [0.0f32; N_MELS];
            for (r, (lo, w)) in row.iter_mut().zip(&fb) {
                *r = w
                    .iter()
                    .zip(&buf[*lo..])
                    .map(|(w, c)| w * c.norm())
                    .sum::<f32>()
                    / wsum;
            }
            row
        })
        .collect()
}

/// 三角フィルタ(HTK のメル尺度、面積正規化なし)。帯域ごとに (最初のビン, 重み)。
fn mel_bank(n: usize, sr: f32, f_max: f32) -> Vec<(usize, Vec<f32>)> {
    let mel = |f: f32| 2595.0 * (1.0 + f / 700.0).log10();
    let hz = |m: f32| 700.0 * (10f32.powf(m / 2595.0) - 1.0);
    let (m0, m1) = (mel(F_MIN), mel(f_max.min(sr / 2.0)));
    let pts: Vec<f32> = (0..N_MELS + 2)
        .map(|i| hz(m0 + (m1 - m0) * i as f32 / (N_MELS + 1) as f32))
        .collect();
    let bin_hz = sr / n as f32;
    (0..N_MELS)
        .map(|m| {
            let lo = (pts[m] / bin_hz).floor() as usize;
            let hi = ((pts[m + 2] / bin_hz).ceil() as usize).min(n / 2);
            let w: Vec<f32> = (lo..=hi.max(lo))
                .map(|k| {
                    let f = k as f32 * bin_hz;
                    let down = (f - pts[m]) / (pts[m + 1] - pts[m]).max(1e-6);
                    let up = (pts[m + 2] - f) / (pts[m + 2] - pts[m + 1]).max(1e-6);
                    down.min(up).max(0.0)
                })
                .collect();
            (lo, w)
        })
        .collect()
}

/// 2 つの特徴の距離。音色(spectral)は重なっている時間だけで比べ、長さの違いは音量の時間変化
/// (envelope。短い方を無音として扱う)に出す。
pub fn distance(a: &Features, b: &Features) -> Distance {
    const EPS: f32 = 1e-3;
    let mut spectral = 0.0;
    for (ma, mb) in a.mels.iter().zip(&b.mels) {
        let n = ma.len().min(mb.len());
        let zero = [0.0f32; N_MELS];
        let (mut l1, mut diff2, mut ref2) = (0.0f32, 0.0f32, 0.0f32);
        for t in 0..n {
            let ra = ma.get(t).unwrap_or(&zero);
            let rb = mb.get(t).unwrap_or(&zero);
            for k in 0..N_MELS {
                l1 += ((ra[k] + EPS).ln() - (rb[k] + EPS).ln()).abs();
                diff2 += (ra[k] - rb[k]).powi(2);
                ref2 += ra[k] * ra[k];
            }
        }
        let log_l1 = l1 / (n * N_MELS).max(1) as f32;
        let convergence = (diff2 / ref2.max(1e-12)).sqrt();
        // 対数の差は無音部で大きくなりやすいので半分の重み
        spectral += 0.5 * log_l1 + convergence;
    }
    spectral /= a.mels.len().max(1) as f32;
    let n = a.env_db.len().max(b.env_db.len());
    let env: f32 = (0..n)
        .map(|t| {
            let x = a.env_db.get(t).copied().unwrap_or(-60.0);
            let y = b.env_db.get(t).copied().unwrap_or(-60.0);
            (x - y).abs()
        })
        .sum::<f32>()
        / n.max(1) as f32
        / 20.0;
    Distance {
        total: spectral + env,
        spectral,
        envelope: env,
    }
}

/// 2 音を比べる(それぞれ鳴り始めからそろえる)。
pub fn compare(a: &[f32], sr_a: f32, b: &[f32], sr_b: f32) -> Distance {
    let f_max = (sr_a.min(sr_b) / 2.0 * 0.9).min(16_000.0);
    let fa = features(trim_onset(a, sr_a), sr_a, f_max);
    let fb = features(trim_onset(b, sr_b), sr_b, f_max);
    distance(&fa, &fb)
}

// ---- 小さな要約(プリセット検索の索引用) ----

/// 要約の時間区間(秒。鳴り始めから)
const SUMMARY_SEGMENTS: [(f32, f32); 4] = [(0.0, 0.1), (0.1, 0.4), (0.4, 1.0), (1.0, 2.0)];
/// 要約の音量の推移の点数(50ms ごと、2 秒)
const SUMMARY_ENV: usize = 40;
/// 要約の長さ(区間ごとの対数メル + 音量の推移)
pub const SUMMARY_LEN: usize = SUMMARY_SEGMENTS.len() * N_MELS + SUMMARY_ENV;

/// 音を小さな数値列にまとめる(区間ごとの対数メルの平均と、音量の推移)。
/// 何千ものプリセットを覚えておき、目標に近い候補をすばやく絞るのに使う(最終的な比較は [`compare`])。
pub fn summary(x: &[f32], sr: f32) -> Vec<f32> {
    let x = trim_onset(x, sr);
    let f = features(x, sr, (sr / 2.0 * 0.9).min(16_000.0));
    let mel = &f.mels[1];
    let hop_sec = (WINDOWS[1] * sr).round() / 4.0 / sr;
    let mut out = Vec::with_capacity(SUMMARY_LEN);
    for (a, b) in SUMMARY_SEGMENTS {
        let (fa, fb) = ((a / hop_sec) as usize, (b / hop_sec) as usize);
        let rows: Vec<&[f32; N_MELS]> = mel.iter().skip(fa).take(fb.saturating_sub(fa)).collect();
        for k in 0..N_MELS {
            let m = if rows.is_empty() {
                0.0
            } else {
                rows.iter().map(|r| r[k]).sum::<f32>() / rows.len() as f32
            };
            out.push((m + 1e-3).ln());
        }
    }
    let per = (0.05 / ENV_HOP) as usize;
    for i in 0..SUMMARY_ENV {
        let seg: Vec<f32> = f.env_db.iter().skip(i * per).take(per).copied().collect();
        let v = if seg.is_empty() {
            -60.0
        } else {
            seg.iter().sum::<f32>() / seg.len() as f32
        };
        out.push(v / 20.0);
    }
    out
}

/// 2 つの要約の距離(0 で同じ)。
pub fn summary_distance(a: &[f32], b: &[f32]) -> f32 {
    let spec = SUMMARY_SEGMENTS.len() * N_MELS;
    let n = a.len().min(b.len());
    if n < spec {
        return f32::MAX;
    }
    let s: f32 = a[..spec]
        .iter()
        .zip(&b[..spec])
        .map(|(x, y)| (x - y).abs())
        .sum::<f32>()
        / spec as f32;
    let e: f32 = a[spec..n]
        .iter()
        .zip(&b[spec..n])
        .map(|(x, y)| (x - y).abs())
        .sum::<f32>()
        / (n - spec).max(1) as f32;
    0.5 * s + e
}

// ---- 内蔵音源の自動合わせ ----

const WAVEFORMS: [&str; 4] = ["saw", "square", "triangle", "sine"];
const UNISON: [i64; 4] = [1, 3, 5, 7];
/// wavetable のテーブル(glaux_dsp の TABLE_NAMES と同じ)
const WT_TABLES: [&str; 5] = ["analog", "pulse", "vocal", "sync", "organ"];
/// fm の周波数比の出発点(整数比 = 楽器らしい、非整数 = 金属的)
const FM_RATIOS: [&str; 4] = ["1", "2", "3.5", "1.41"];

/// 自動合わせの対象の内蔵音源。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FitInstrument {
    Subtractive,
    Fm,
    Wavetable,
}

fn log_map(v: f64, lo: f64, hi: f64) -> f64 {
    lo * (hi / lo).powf(v.clamp(0.0, 1.0))
}

fn inv_log(v: f64, lo: f64, hi: f64) -> f64 {
    ((v.clamp(lo, hi) / lo).ln() / (hi / lo).ln()).clamp(0.0, 1.0)
}

/// 記述子から包絡の初期値(attack, decay, sustain, release を 0〜1 で)。上限は音源ごと。
fn envelope_guess(d: &SoundDescriptors, decay_max: f64, release_max: f64) -> [f64; 4] {
    let e = &d.envelope;
    let sustain = if e.decays_continuously {
        0.0
    } else {
        10f64.powf(e.sustain_db as f64 / 20.0).clamp(0.0, 1.0)
    };
    [
        inv_log(e.attack_ms as f64 / 1000.0, 0.001, 2.0),
        inv_log((e.decay_ms as f64 / 1000.0).max(0.01), 0.01, decay_max),
        sustain,
        inv_log((e.release_ms as f64 / 1000.0).max(0.01), 0.01, release_max),
    ]
}

impl FitInstrument {
    pub fn name(self) -> &'static str {
        match self {
            FitInstrument::Subtractive => "subtractive",
            FitInstrument::Fm => "fm",
            FitInstrument::Wavetable => "wavetable",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "subtractive" => Some(FitInstrument::Subtractive),
            "fm" => Some(FitInstrument::Fm),
            "wavetable" => Some(FitInstrument::Wavetable),
            _ => None,
        }
    }

    /// 探す連続つまみの数
    fn dim(self) -> usize {
        match self {
            // cutoff, resonance, attack, decay, sustain, release, filter_env, detune, sub, noise, unison
            FitInstrument::Subtractive => 11,
            // ratio, index, index_decay, index_sustain, feedback, attack, decay, sustain, release
            FitInstrument::Fm => 9,
            // position, pos_env, pos_decay, cutoff, resonance, attack, decay, sustain, release, detune, unison
            FitInstrument::Wavetable => 11,
        }
    }

    /// 0〜1 の探索空間 → つまみの値。`variant` は subtractive なら波形、fm なら比の出発点(値は x で決まる)。
    fn to_params(self, x: &[f64], variant: &str) -> ParamMap {
        let c = |i: usize| x[i].clamp(0.0, 1.0);
        let mut m = ParamMap::new();
        let f = |m: &mut ParamMap, k: &str, v: f64| {
            m.insert(k.into(), ParamValue::Float(v));
        };
        match self {
            FitInstrument::Subtractive => {
                m.insert("waveform".into(), ParamValue::Enum(variant.to_owned()));
                f(&mut m, "cutoff", log_map(c(0), 40.0, 12_000.0));
                f(&mut m, "resonance", c(1) * 0.9);
                f(&mut m, "attack", log_map(c(2), 0.001, 2.0));
                f(&mut m, "decay", log_map(c(3), 0.01, 3.0));
                f(&mut m, "sustain", c(4));
                f(&mut m, "release", log_map(c(5), 0.01, 4.0));
                f(&mut m, "filter_env", c(6));
                f(&mut m, "detune", c(7) * 60.0);
                f(&mut m, "sub", c(8));
                f(&mut m, "noise", c(9));
                let u = ((c(10) * 4.0) as usize).min(3);
                m.insert("unison".into(), ParamValue::Int(UNISON[u]));
            }
            FitInstrument::Fm => {
                f(&mut m, "ratio", log_map(c(0), 0.5, 16.0));
                f(&mut m, "index", c(1) * 12.0);
                f(&mut m, "index_decay", log_map(c(2), 0.005, 4.0));
                f(&mut m, "index_sustain", c(3));
                f(&mut m, "feedback", c(4));
                f(&mut m, "attack", log_map(c(5), 0.001, 2.0));
                f(&mut m, "decay", log_map(c(6), 0.01, 6.0));
                f(&mut m, "sustain", c(7));
                f(&mut m, "release", log_map(c(8), 0.01, 6.0));
            }
            FitInstrument::Wavetable => {
                m.insert("table".into(), ParamValue::Enum(variant.to_owned()));
                f(&mut m, "position", c(0));
                f(&mut m, "pos_env", c(1) * 2.0 - 1.0);
                f(&mut m, "pos_decay", log_map(c(2), 0.005, 4.0));
                f(&mut m, "cutoff", log_map(c(3), 40.0, 20_000.0));
                f(&mut m, "resonance", c(4) * 0.9);
                f(&mut m, "attack", log_map(c(5), 0.001, 4.0));
                f(&mut m, "decay", log_map(c(6), 0.01, 6.0));
                f(&mut m, "sustain", c(7));
                f(&mut m, "release", log_map(c(8), 0.01, 8.0));
                f(&mut m, "detune", c(9) * 60.0);
                let u = ((c(10) * 4.0) as usize).min(3);
                m.insert("unison".into(), ParamValue::Int(UNISON[u]));
            }
        }
        m
    }

    /// 記述子から探索の初期値を決める。
    fn initial_guess(self, d: &SoundDescriptors, variant: &str) -> Vec<f64> {
        let s = &d.spectrum;
        let opening = s.centroid_start_hz > s.centroid_end_hz * 1.4;
        match self {
            FitInstrument::Subtractive => {
                let [a, dcy, sus, rel] = envelope_guess(d, 3.0, 4.0);
                // 明るさの中心の 3 倍あたりにカットオフ。鳴り始めの方が明るければフィルタエンベロープ
                let cutoff = inv_log((s.centroid_hz as f64 * 3.0).max(80.0), 40.0, 12_000.0);
                let filter_env = if opening { 0.5 } else { 0.15 };
                let noise = if s.flatness > 0.3 { 0.6 } else { 0.05 };
                vec![
                    cutoff, 0.15, a, dcy, sus, rel, filter_env, 0.2, 0.0, noise, 0.0,
                ]
            }
            FitInstrument::Fm => {
                let [a, dcy, sus, rel] = envelope_guess(d, 6.0, 6.0);
                let ratio: f64 = variant.parse().unwrap_or(1.0);
                // 明るい音ほど変調を深く。鳴り始めだけ明るければ深さを減衰させる
                let index = if s.centroid_hz > 1500.0 { 0.45 } else { 0.25 };
                let (idx_decay, idx_sus) = if opening {
                    (inv_log(0.3, 0.005, 4.0), 0.2)
                } else {
                    (inv_log(1.0, 0.005, 4.0), 0.8)
                };
                let feedback = if s.flatness > 0.2 { 0.3 } else { 0.0 };
                vec![
                    inv_log(ratio, 0.5, 16.0),
                    index,
                    idx_decay,
                    idx_sus,
                    feedback,
                    a,
                    dcy,
                    sus,
                    rel,
                ]
            }
            FitInstrument::Wavetable => {
                let [a, dcy, sus, rel] = envelope_guess(d, 6.0, 8.0);
                // 明るい音ほど position を先へ。鳴り始めだけ明るければ pos_env で掃く
                let position = if s.centroid_hz > 1500.0 { 0.7 } else { 0.4 };
                let pos_env = if opening { 0.75 } else { 0.5 };
                let cutoff = inv_log((s.centroid_hz as f64 * 6.0).max(200.0), 40.0, 20_000.0);
                vec![
                    position,
                    pos_env,
                    inv_log(0.3, 0.005, 4.0),
                    cutoff,
                    0.1,
                    a,
                    dcy,
                    sus,
                    rel,
                    0.2,
                    0.0,
                ]
            }
        }
    }

    /// 候補(先頭ほど有望)。
    fn variants(self, d: &SoundDescriptors) -> Vec<&'static str> {
        match self {
            FitInstrument::Subtractive => {
                let first = match d.harmonics.as_ref().map(|h| h.waveform_guess.as_str()) {
                    Some("sine") => "sine",
                    Some("square") => "square",
                    Some("triangle") => "triangle",
                    _ => "saw",
                };
                let mut v = vec![first];
                v.extend(WAVEFORMS.iter().copied().filter(|w| *w != first));
                v
            }
            FitInstrument::Fm => {
                // 非調和な音なら非整数比から
                let inharmonic = d.harmonics.as_ref().is_some_and(|h| h.inharmonicity > 0.02)
                    || d.pitch.is_none();
                if inharmonic {
                    vec!["3.5", "1.41", "1", "2"]
                } else {
                    FM_RATIOS.to_vec()
                }
            }
            // テーブルは全部を初期値で比べて、近い 2 つを探す
            FitInstrument::Wavetable => WT_TABLES.to_vec(),
        }
    }
}

/// 候補のつまみで 1 音鳴らす(ボイスを直接使う。`hold` 秒で離し、全体で `len` サンプル)。
pub fn render_instrument(
    instrument: &str,
    params: &ParamMap,
    pitch: u8,
    hold: f32,
    len: usize,
    sr: f32,
) -> Vec<f32> {
    let mut device = Device::builtin(instrument);
    device.params = params.clone();
    let (_, ip) = bake_instrument(Some(&device));
    let freq = 440.0 * 2f32.powf((pitch as f32 - 69.0) / 12.0);
    let mut v = VoiceState::start(&ip, freq, pitch, 0.8, Default::default(), sr);
    let off = (hold * sr) as usize;
    (0..len)
        .map(|i| {
            if i == off {
                v.note_off();
            }
            v.next(&ip)
        })
        .collect()
}

/// subtractive で 1 音鳴らす([`render_instrument`] の短縮形)。
pub fn render_subtractive(
    params: &ParamMap,
    pitch: u8,
    hold: f32,
    len: usize,
    sr: f32,
) -> Vec<f32> {
    render_instrument("subtractive", params, pitch, hold, len, sr)
}

/// 内蔵リバーブ(mix / size)を通す(モノラル → 左右の平均)。
pub fn apply_reverb(x: &mut [f32], mix: f32, size: f32, sr: f32) {
    let mut e = glaux_core::Effect::builtin(glaux_core::FxId::new(), "reverb");
    e.params.insert("mix".into(), ParamValue::Float(mix as f64));
    e.params
        .insert("size".into(), ParamValue::Float(size as f64));
    let Some(p) = glaux_dsp::bake_effect(&e, sr, &|_| None) else {
        return;
    };
    let mut st = glaux_dsp::EffectState::without_delay_buffers();
    st.ensure_kind(&p);
    for v in x.iter_mut() {
        let (l, r) = st.process(&p, *v, *v, 0.0);
        *v = (l + r) * 0.5;
    }
}

/// 評価回数の目安: 1 秒あたりに鳴らせる目標の長さ(秒)。これと max_seconds・目標の長さから
/// 評価回数の上限を決める(時間では打ち切らないので、同じ入力なら負荷に関係なく同じ結果になる)
const AUDIO_SECONDS_PER_SECOND: f32 = 1200.0;
/// 安全のための時間の上限(max_seconds の何倍まで待つか。遅い機械・高負荷時だけ効く)
const TIME_CAP_FACTOR: f32 = 4.0;

/// 自動合わせの設定。
#[derive(Clone, Copy, Debug)]
pub struct FitOptions {
    /// 候補 1 つあたりの世代数の上限
    pub generations: usize,
    /// 1 世代の候補数
    pub population: usize,
    /// 時間の目安(秒)。評価回数の上限に換算する(実際の時間は機械の速さで前後する)
    pub max_seconds: f32,
    pub seed: u64,
    /// リバーブ(mix / size)も一緒に探す
    pub reverb: bool,
}

impl Default for FitOptions {
    fn default() -> Self {
        FitOptions {
            generations: 150,
            population: 16,
            max_seconds: 20.0,
            seed: 1,
            reverb: false,
        }
    }
}

/// 自動合わせの結果。
#[derive(Clone, Debug)]
pub struct FitResult {
    pub instrument: FitInstrument,
    /// 音源のつまみ(gain_db は含まない。今の値を保つ)
    pub params: ParamMap,
    /// リバーブも探したとき: (mix, size)
    pub reverb: Option<(f32, f32)>,
    pub distance: Distance,
    /// 初期値(記述子からの推定)での距離
    pub initial_distance: Distance,
    /// 試した候補(波形 / 比の出発点)ごとの最良の距離
    pub tried: Vec<(String, f32)>,
    pub evaluations: usize,
    pub seconds: f32,
}

/// 目標の音(モノラル、`sr`)に内蔵音源のつまみを合わせる。`pitch` は目標の音の高さ、
/// `hold` は鍵盤を押している秒数、`d` は目標の記述子(初期値に使う)。
pub fn fit_instrument(
    target: &[f32],
    sr: f32,
    pitch: u8,
    hold: f32,
    d: &SoundDescriptors,
    instrument: FitInstrument,
    opts: FitOptions,
) -> FitResult {
    use cmaes::{CMAESOptions, DVector};
    let started = std::time::Instant::now();
    let target = trim_onset(target, sr);
    let len = target.len().max(1);
    let f_max = (sr / 2.0 * 0.9).min(16_000.0);
    let tf = features(target, sr, f_max);
    let dim = instrument.dim();
    // リバーブの 2 次元(mix, size)は音源のつまみの後ろ
    let reverb_of = |x: &[f64]| -> Option<(f32, f32)> {
        opts.reverb.then(|| {
            (
                x[dim].clamp(0.0, 1.0) as f32,
                x[dim + 1].clamp(0.0, 1.0) as f32,
            )
        })
    };
    let eval = |x: &[f64], variant: &str| -> Distance {
        let mut y = render_instrument(
            instrument.name(),
            &instrument.to_params(x, variant),
            pitch,
            hold,
            len,
            sr,
        );
        if let Some((mix, size)) = reverb_of(x) {
            apply_reverb(&mut y, mix, size, sr);
        }
        distance(&tf, &features(trim_onset(&y, sr), sr, f_max))
    };
    // 範囲外に出た分は罰則(CMA-ES は制約なしなので、0〜1 に引き戻す)
    let penalty = |x: &[f64]| -> f64 {
        x.iter()
            .map(|v| (v - v.clamp(0.0, 1.0)).powi(2))
            .sum::<f64>()
            * 10.0
    };
    let variants = instrument.variants(d);
    let init_for = |variant: &str| -> Vec<f64> {
        let mut v = instrument.initial_guess(d, variant);
        if opts.reverb {
            // 余韻が長い音はリバーブ多めから
            let wet = if d.envelope.release_ms > 600.0 {
                0.3
            } else {
                0.1
            };
            v.extend([wet, 0.5]);
        }
        v
    };
    let initial_distance = eval(&init_for(variants[0]), variants[0]);
    // 候補ごとに初期値で評価し、有望な 2 つを探す
    let mut first: Vec<(&str, f32)> = variants
        .iter()
        .map(|w| (*w, eval(&init_for(w), w).total))
        .collect();
    first.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut evaluations = variants.len() + 1;
    let mut best: (f64, Vec<f64>, &str) = (f64::MAX, init_for(first[0].0), first[0].0);
    let mut tried = Vec::new();
    let per_variant = opts.max_seconds / 2.0;
    // 評価回数の上限(目標が長いほど 1 回が重いので減らす)。世代数の上限とどちらか小さい方で止まる
    let target_seconds = (len as f32 / sr).max(0.05);
    let max_evals = ((per_variant * AUDIO_SECONDS_PER_SECOND / target_seconds) as usize)
        .max(opts.population * 5);
    for (variant, _) in first.iter().take(2) {
        let init = init_for(variant);
        let objective = |x: &DVector<f64>| -> f64 {
            eval(x.as_slice(), variant).total as f64 + penalty(x.as_slice())
        };
        let mut cma = match CMAESOptions::new(init.clone(), 0.2)
            .population_size(opts.population)
            .max_generations(opts.generations)
            .max_function_evals(max_evals)
            .max_time(std::time::Duration::from_secs_f32(
                per_variant * TIME_CAP_FACTOR,
            ))
            .seed(opts.seed)
            .build(objective)
        {
            Ok(c) => c,
            Err(_) => continue,
        };
        let r = cma.run_parallel();
        evaluations += cma.function_evals();
        if let Some(b) = r.overall_best {
            tried.push((variant.to_string(), b.value as f32));
            if b.value < best.0 {
                best = (b.value, b.point.as_slice().to_vec(), variant);
            }
        }
    }
    let params = instrument.to_params(&best.1, best.2);
    let distance = eval(&best.1, best.2);
    FitResult {
        instrument,
        params,
        reverb: reverb_of(&best.1),
        distance,
        initial_distance,
        tried,
        evaluations,
        seconds: started.elapsed().as_secs_f32(),
    }
}

/// subtractive に合わせる([`fit_instrument`] の短縮形)。
pub fn fit_subtractive(
    target: &[f32],
    sr: f32,
    pitch: u8,
    hold: f32,
    d: &SoundDescriptors,
    opts: FitOptions,
) -> FitResult {
    fit_instrument(target, sr, pitch, hold, d, FitInstrument::Subtractive, opts)
}

// ---- CLAP 音源のつまみの自動合わせ ----

/// 自動合わせで動かすプラグインのつまみ。
#[derive(Clone, Debug)]
pub struct PluginFitParam {
    pub id: u32,
    pub name: String,
    pub min: f64,
    pub max: f64,
    /// 探索の出発点(今の値)
    pub start: f64,
    pub stepped: bool,
}

/// 動かすつまみを名前で選ぶ(`module/name` を小文字にして判定)。`wanted` が空なら一般的なシンセの主要なつまみ
/// (フィルター 1 のカットオフ・レゾナンス・エンベロープ量、アンプエンベロープの ADSR、フィルターエンベロープの
/// ディケイ、ユニゾンのデチューン)を 1 つずつ。`wanted` は部分一致の語の並び(例 "cutoff"、"amp eg attack")。
pub fn choose_plugin_params(
    params: &[(glaux_clap::ParamInfo, f64)],
    wanted: &[String],
    limit: usize,
) -> Vec<PluginFitParam> {
    let full = |p: &glaux_clap::ParamInfo| format!("{}/{}", p.module, p.name).to_lowercase();
    let excluded = ["shape", "lfo", "mute", "solo", "route", "link"];
    let usable = |p: &glaux_clap::ParamInfo| {
        let n = full(p);
        !excluded.iter().any(|x| n.contains(x)) && p.max > p.min
    };
    // 語の並びの候補(先頭ほど優先)。どれか 1 つが当たれば採る
    let default_groups: Vec<Vec<&str>> = vec![
        vec!["filter 1 cutoff", "cutoff"],
        vec!["filter 1 resonance", "resonance"],
        vec![
            "filter 1 feg mod amount",
            "env amount",
            "eg amount",
            "env mod",
        ],
        vec!["amp eg attack", "amp attack", "attack"],
        vec!["amp eg decay", "amp decay", "decay"],
        vec!["amp eg sustain", "amp sustain", "sustain"],
        vec!["amp eg release", "amp release", "release"],
        vec!["filter eg decay", "filter decay"],
        vec!["unison detune", "detune"],
    ];
    let groups: Vec<Vec<String>> = if wanted.is_empty() {
        default_groups
            .iter()
            .map(|g| g.iter().map(|s| s.to_string()).collect())
            .collect()
    } else {
        wanted.iter().map(|w| vec![w.to_lowercase()]).collect()
    };
    let mut out: Vec<PluginFitParam> = Vec::new();
    for g in groups {
        let found = g.iter().find_map(|word| {
            params.iter().find(|(p, _)| {
                usable(p) && full(p).contains(word.as_str()) && !out.iter().any(|o| o.id == p.id)
            })
        });
        if let Some((p, v)) = found {
            out.push(PluginFitParam {
                id: p.id,
                name: {
                    let m = p.module.trim_matches('/');
                    if m.is_empty() {
                        p.name.clone()
                    } else {
                        format!("{m} / {}", p.name)
                    }
                },
                min: p.min,
                max: p.max,
                start: v.clamp(p.min, p.max),
                stepped: p.stepped,
            });
        }
        if out.len() >= limit {
            break;
        }
    }
    out
}

/// CLAP 音源のつまみの自動合わせの結果。
#[derive(Clone, Debug)]
pub struct PluginFit {
    /// つまみの ID と合わせた値(プラグインの単位)
    pub values: Vec<(u32, f64)>,
    pub distance: Distance,
    pub initial_distance: Distance,
    pub evaluations: usize,
    pub seconds: f32,
}

/// 目標の音に、プラグインのつまみ `params` を合わせる(CMA-ES、プラグインは 1 つを持ち回って逐次評価)。
/// `renderer` には合わせる元の状態(プリセット等)を読み込んでおくこと。
#[allow(clippy::too_many_arguments)]
pub fn fit_plugin(
    renderer: &mut crate::plugins::PluginRenderer,
    target: &[f32],
    sr: f32,
    pitch: u8,
    hold: f32,
    params: &[PluginFitParam],
    max_seconds: f32,
    seed: u64,
) -> Result<PluginFit, String> {
    use cmaes::{CMAESOptions, DVector};
    let started = std::time::Instant::now();
    let target = trim_onset(target, sr);
    let len = target.len().max(1);
    let f_max = (sr / 2.0 * 0.9).min(16_000.0);
    let tf = features(target, sr, f_max);
    let spec = crate::plugins::PresetRenderSpec {
        pitch,
        velocity: 0.8,
        hold: hold as f64,
        total: len as f64 / sr as f64,
        sample_rate: sr as f64,
    };
    let to_values = |x: &[f64]| -> Vec<(u32, f64)> {
        params
            .iter()
            .zip(x)
            .map(|(p, v)| {
                let mut val = p.min + v.clamp(0.0, 1.0) * (p.max - p.min);
                if p.stepped {
                    val = val.round();
                }
                (p.id, val)
            })
            .collect()
    };
    let mut evaluations = 0usize;
    let mut eval = |x: &[f64]| -> Distance {
        evaluations += 1;
        match renderer.render(&to_values(x), spec) {
            Ok(y) => distance(&tf, &features(trim_onset(&y, sr), sr, f_max)),
            Err(_) => Distance {
                total: 10.0,
                spectral: 10.0,
                envelope: 0.0,
            },
        }
    };
    let init: Vec<f64> = params
        .iter()
        .map(|p| ((p.start - p.min) / (p.max - p.min)).clamp(0.0, 1.0))
        .collect();
    let initial_distance = eval(&init);
    if params.is_empty() {
        return Ok(PluginFit {
            values: vec![],
            distance: initial_distance,
            initial_distance,
            evaluations,
            seconds: started.elapsed().as_secs_f32(),
        });
    }
    let penalty = |x: &[f64]| -> f64 {
        x.iter()
            .map(|v| (v - v.clamp(0.0, 1.0)).powi(2))
            .sum::<f64>()
            * 10.0
    };
    let mut best = (initial_distance.total as f64, init.clone());
    {
        let objective = |x: &DVector<f64>| -> f64 {
            let d = eval(x.as_slice()).total as f64 + penalty(x.as_slice());
            if d < best.0 {
                best = (d, x.as_slice().to_vec());
            }
            d
        };
        let mut cma = CMAESOptions::new(init.clone(), 0.15)
            .population_size(10)
            .max_generations(200)
            .max_time(std::time::Duration::from_secs_f32(max_seconds.max(1.0)))
            .seed(seed)
            .build(objective)
            .map_err(|e| format!("{e:?}"))?;
        cma.run();
    }
    let values = to_values(&best.1);
    let distance = eval(&best.1);
    Ok(PluginFit {
        values,
        distance,
        initial_distance,
        evaluations,
        seconds: started.elapsed().as_secs_f32(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn param(pairs: &[(&str, ParamValue)]) -> ParamMap {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn distance_is_zero_for_same_and_grows_with_difference() {
        let sr = 32_000.0;
        let base = param(&[("waveform", ParamValue::Enum("saw".into()))]);
        let a = render_subtractive(&base, 57, 0.6, 32_000, sr);
        let same = compare(&a, sr, &a, sr);
        assert!(same.total < 1e-4, "{same:?}");
        let mut dark = base.clone();
        dark.insert("cutoff".into(), ParamValue::Float(400.0));
        let b = render_subtractive(&dark, 57, 0.6, 32_000, sr);
        let mut slow = base.clone();
        slow.insert("attack".into(), ParamValue::Float(0.5));
        let c = render_subtractive(&slow, 57, 0.6, 32_000, sr);
        let d_dark = compare(&a, sr, &b, sr);
        let d_slow = compare(&a, sr, &c, sr);
        assert!(d_dark.spectral > 0.2, "{d_dark:?}");
        assert!(d_slow.envelope > d_dark.envelope, "{d_slow:?} {d_dark:?}");
        // サンプルレートが違っても同じ音なら近い
        let a48 = render_subtractive(&base, 57, 0.6, 48_000, 48_000.0);
        let x = compare(&a, sr, &a48, 48_000.0);
        assert!(x.total < d_dark.total * 0.5, "{x:?} vs {d_dark:?}");
    }

    #[test]
    fn summary_ranks_similar_sounds_closer() {
        let sr = 48_000.0;
        let p = |w: &str, cutoff: f64, attack: f64| {
            param(&[
                ("waveform", ParamValue::Enum(w.into())),
                ("cutoff", ParamValue::Float(cutoff)),
                ("attack", ParamValue::Float(attack)),
            ])
        };
        let a = summary(
            &render_subtractive(&p("saw", 3000.0, 0.005), 60, 1.0, 96_000, sr),
            sr,
        );
        let near = summary(
            &render_subtractive(&p("saw", 2500.0, 0.01), 60, 1.0, 96_000, sr),
            sr,
        );
        let far = summary(
            &render_subtractive(&p("sine", 300.0, 0.8), 60, 1.0, 96_000, sr),
            sr,
        );
        assert_eq!(a.len(), SUMMARY_LEN);
        assert!(summary_distance(&a, &a) < 1e-6);
        assert!(summary_distance(&a, &near) < summary_distance(&a, &far));
    }

    #[test]
    fn fm_fits_a_bell_better_than_subtractive() {
        // 目標: 非整数比の FM ベル(減衰し続ける)
        let sr = 32_000.0;
        let bell = param(&[
            ("ratio", ParamValue::Float(3.5)),
            ("index", ParamValue::Float(5.0)),
            ("index_decay", ParamValue::Float(0.6)),
            ("index_sustain", ParamValue::Float(0.1)),
            ("attack", ParamValue::Float(0.002)),
            ("decay", ParamValue::Float(1.5)),
            ("sustain", ParamValue::Float(0.0)),
            ("release", ParamValue::Float(1.0)),
        ]);
        let target = render_instrument("fm", &bell, 69, 1.2, (sr * 1.2) as usize, sr);
        let d = crate::timbre::describe(&target, sr, None);
        let opts = FitOptions {
            max_seconds: 16.0,
            ..Default::default()
        };
        let fm = fit_instrument(&target, sr, 69, 1.2, &d, FitInstrument::Fm, opts);
        let sub = fit_instrument(&target, sr, 69, 1.2, &d, FitInstrument::Subtractive, opts);
        eprintln!(
            "fm {:?} ({:?}) / subtractive {:?}",
            fm.distance,
            fm.params.get("ratio"),
            sub.distance
        );
        assert!(
            fm.distance.total < sub.distance.total,
            "ベルは fm の方が近い"
        );
        assert!(fm.distance.total < 0.3, "{:?}", fm.distance);
    }

    #[test]
    fn reverb_is_found_when_the_target_has_it() {
        let sr = 32_000.0;
        let dry = param(&[
            ("waveform", ParamValue::Enum("saw".into())),
            ("cutoff", ParamValue::Float(2000.0)),
            ("decay", ParamValue::Float(0.2)),
            ("sustain", ParamValue::Float(0.0)),
            ("release", ParamValue::Float(0.1)),
        ]);
        let mut target = render_subtractive(&dry, 57, 0.3, (sr * 1.5) as usize, sr);
        apply_reverb(&mut target, 0.5, 0.8, sr);
        let d = crate::timbre::describe(&target, sr, None);
        let opts = FitOptions {
            max_seconds: 10.0,
            ..Default::default()
        };
        let without = fit_subtractive(&target, sr, 57, 0.3, &d, opts);
        let with = fit_subtractive(
            &target,
            sr,
            57,
            0.3,
            &d,
            FitOptions {
                reverb: true,
                ..opts
            },
        );
        eprintln!(
            "リバーブなし {:?} / あり {:?} {:?}",
            without.distance, with.distance, with.reverb
        );
        assert!(with.distance.total < without.distance.total);
        assert!(with.reverb.is_some_and(|(mix, _)| mix > 0.15));
    }

    #[test]
    fn fit_recovers_a_known_patch() {
        // 目標: 暗めの矩形波のプラック(フィルタエンベロープ付き)
        let sr = 32_000.0;
        let target_params = param(&[
            ("waveform", ParamValue::Enum("square".into())),
            ("cutoff", ParamValue::Float(900.0)),
            ("resonance", ParamValue::Float(0.3)),
            ("attack", ParamValue::Float(0.002)),
            ("decay", ParamValue::Float(0.35)),
            ("sustain", ParamValue::Float(0.1)),
            ("release", ParamValue::Float(0.2)),
            ("filter_env", ParamValue::Float(0.6)),
        ]);
        let target = render_subtractive(&target_params, 52, 0.5, (sr * 1.2) as usize, sr);
        let d = crate::timbre::describe(&target, sr, None);
        let r = fit_subtractive(
            &target,
            sr,
            52,
            0.5,
            &d,
            FitOptions {
                max_seconds: 30.0,
                ..Default::default()
            },
        );
        eprintln!(
            "{:?} → {:?}、{} 回、{:.1} 秒、{:?}\n{:?}",
            r.initial_distance, r.distance, r.evaluations, r.seconds, r.tried, r.params
        );
        assert!(r.distance.total < r.initial_distance.total * 0.6);
        assert!(r.distance.total < 0.25, "{:?}", r.distance);
    }

    #[test]
    fn wavetable_fit_recovers_a_vocal_patch_and_is_deterministic() {
        let sr = 32_000.0;
        let target_params = param(&[
            ("table", ParamValue::Enum("vocal".into())),
            ("position", ParamValue::Float(0.6)),
            ("attack", ParamValue::Float(0.02)),
            ("decay", ParamValue::Float(0.5)),
            ("sustain", ParamValue::Float(0.7)),
            ("release", ParamValue::Float(0.3)),
        ]);
        let target = render_instrument(
            "wavetable",
            &target_params,
            48,
            0.6,
            (sr * 1.0) as usize,
            sr,
        );
        let d = crate::timbre::describe(&target, sr, None);
        let opts = FitOptions {
            max_seconds: 6.0,
            ..Default::default()
        };
        let a = fit_instrument(&target, sr, 48, 0.6, &d, FitInstrument::Wavetable, opts);
        eprintln!(
            "{:?} → {:?}、{} 回、{:.1} 秒、{:?}\n{:?}",
            a.initial_distance, a.distance, a.evaluations, a.seconds, a.tried, a.params
        );
        assert_eq!(
            a.params.get("table"),
            Some(&ParamValue::Enum("vocal".into()))
        );
        assert!(a.distance.total < 0.2, "{:?}", a.distance);
        // 評価回数で区切るので、もう一度やっても同じ結果
        let b = fit_instrument(&target, sr, 48, 0.6, &d, FitInstrument::Wavetable, opts);
        assert_eq!(a.params, b.params);
        assert_eq!(a.evaluations, b.evaluations);
    }
}
