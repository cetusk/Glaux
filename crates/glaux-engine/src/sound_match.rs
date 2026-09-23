//! 似た音を作る: 音色の距離と、内蔵 subtractive のつまみの自動合わせ。
//!
//! 距離(`distance`)は、音量をそろえた 2 音の「多重解像度の対数メルスペクトログラムの差」
//! (auraloss の multi-resolution STFT 距離に倣い、約 11 / 21 / 43ms の 3 つの窓で、対数の L1 と
//! スペクトル収束度)と「音量の包絡(5ms ごとの dB)の差」の和。
//! 窓の長さは秒で決めるので、サンプルレートの違う音どうしも比べられる。
//!
//! 自動合わせ(`fit_subtractive`)は Instrumental(2026)の構成に倣い、CMA-ES(微分を使わない進化的な
//! 最適化)で subtractive の連続つまみ 11 個を探す。初期値は音の記述子(立ち上がり・減衰・明るさ・
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

// ---- subtractive の自動合わせ ----

const WAVEFORMS: [&str; 4] = ["saw", "square", "triangle", "sine"];
const UNISON: [i64; 4] = [1, 3, 5, 7];
/// 探す連続つまみの数(cutoff, resonance, attack, decay, sustain, release, filter_env, detune, sub, noise, unison)
const DIM: usize = 11;

/// 0〜1 の探索空間 ↔ つまみの値。
fn to_params(x: &[f64], waveform: &str) -> ParamMap {
    let c = |i: usize| x[i].clamp(0.0, 1.0);
    let log = |v: f64, lo: f64, hi: f64| lo * (hi / lo).powf(v);
    let mut m = ParamMap::new();
    m.insert("waveform".into(), ParamValue::Enum(waveform.to_owned()));
    m.insert(
        "cutoff".into(),
        ParamValue::Float(log(c(0), 40.0, 12_000.0)),
    );
    m.insert("resonance".into(), ParamValue::Float(c(1) * 0.9));
    m.insert("attack".into(), ParamValue::Float(log(c(2), 0.001, 2.0)));
    m.insert("decay".into(), ParamValue::Float(log(c(3), 0.01, 3.0)));
    m.insert("sustain".into(), ParamValue::Float(c(4)));
    m.insert("release".into(), ParamValue::Float(log(c(5), 0.01, 4.0)));
    m.insert("filter_env".into(), ParamValue::Float(c(6)));
    m.insert("detune".into(), ParamValue::Float(c(7) * 60.0));
    m.insert("sub".into(), ParamValue::Float(c(8)));
    m.insert("noise".into(), ParamValue::Float(c(9)));
    let u = ((c(10) * 4.0) as usize).min(3);
    m.insert("unison".into(), ParamValue::Int(UNISON[u]));
    m
}

/// 記述子から探索の初期値を決める。
fn initial_guess(d: &SoundDescriptors) -> [f64; DIM] {
    let inv_log =
        |v: f64, lo: f64, hi: f64| ((v.clamp(lo, hi) / lo).ln() / (hi / lo).ln()).clamp(0.0, 1.0);
    let e = &d.envelope;
    let s = &d.spectrum;
    let attack = inv_log(e.attack_ms as f64 / 1000.0, 0.001, 2.0);
    let decay = inv_log((e.decay_ms as f64 / 1000.0).max(0.01), 0.01, 3.0);
    let sustain = if e.decays_continuously {
        0.0
    } else {
        10f64.powf(e.sustain_db as f64 / 20.0).clamp(0.0, 1.0)
    };
    let release = inv_log((e.release_ms as f64 / 1000.0).max(0.01), 0.01, 4.0);
    // 明るさの中心の 3 倍あたりにカットオフ。鳴り始めの方が明るければフィルタエンベロープ
    let cutoff = inv_log((s.centroid_hz as f64 * 3.0).max(80.0), 40.0, 12_000.0);
    let filter_env = if s.centroid_start_hz > s.centroid_end_hz * 1.4 {
        0.5
    } else {
        0.15
    };
    let noise = if s.flatness > 0.3 { 0.6 } else { 0.05 };
    [
        cutoff, 0.15, attack, decay, sustain, release, filter_env, 0.2, 0.0, noise, 0.0,
    ]
}

/// 記述子の波形の推定から、探す波形の順番を決める(先頭ほど有望)。
fn waveform_order(d: &SoundDescriptors) -> Vec<&'static str> {
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

/// 候補のつまみで 1 音鳴らす(ボイスを直接使う。`hold` 秒で離し、全体で `len` サンプル)。
pub fn render_subtractive(
    params: &ParamMap,
    pitch: u8,
    hold: f32,
    len: usize,
    sr: f32,
) -> Vec<f32> {
    let mut device = Device::builtin("subtractive");
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

/// 自動合わせの設定。
#[derive(Clone, Copy, Debug)]
pub struct FitOptions {
    /// 波形 1 つあたりの世代数の上限
    pub generations: usize,
    /// 1 世代の候補数
    pub population: usize,
    /// 全体の時間の上限(秒)
    pub max_seconds: f32,
    pub seed: u64,
}

impl Default for FitOptions {
    fn default() -> Self {
        FitOptions {
            generations: 150,
            population: 16,
            max_seconds: 20.0,
            seed: 1,
        }
    }
}

/// 自動合わせの結果。
#[derive(Clone, Debug)]
pub struct FitResult {
    /// subtractive のつまみ(gain_db は含まない。今の値を保つ)
    pub params: ParamMap,
    pub distance: Distance,
    /// 初期値(記述子からの推定)での距離
    pub initial_distance: Distance,
    /// 試した波形ごとの最良の距離
    pub tried: Vec<(String, f32)>,
    pub evaluations: usize,
    pub seconds: f32,
}

/// 目標の音(モノラル、`sr`)に subtractive のつまみを合わせる。`pitch` は目標の音の高さ、
/// `hold` は鍵盤を押している秒数、`d` は目標の記述子(初期値に使う)。
pub fn fit_subtractive(
    target: &[f32],
    sr: f32,
    pitch: u8,
    hold: f32,
    d: &SoundDescriptors,
    opts: FitOptions,
) -> FitResult {
    use cmaes::{CMAESOptions, DVector};
    let started = std::time::Instant::now();
    let target = trim_onset(target, sr);
    let len = target.len().max(1);
    let f_max = (sr / 2.0 * 0.9).min(16_000.0);
    let tf = features(target, sr, f_max);
    let eval = |x: &[f64], wf: &str| -> Distance {
        let y = render_subtractive(&to_params(x, wf), pitch, hold, len, sr);
        distance(&tf, &features(trim_onset(&y, sr), sr, f_max))
    };
    // 範囲外に出た分は罰則(CMA-ES は制約なしなので、0〜1 に引き戻す)
    let penalty = |x: &[f64]| -> f64 {
        x.iter()
            .map(|v| (v - v.clamp(0.0, 1.0)).powi(2))
            .sum::<f64>()
            * 10.0
    };
    let init = initial_guess(d);
    let order = waveform_order(d);
    let initial_distance = eval(&init, order[0]);
    // 波形ごとに初期値で評価し、有望な 2 つを探す
    let mut first: Vec<(&str, f32)> = order.iter().map(|w| (*w, eval(&init, w).total)).collect();
    first.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut evaluations = WAVEFORMS.len() + 1;
    let mut best: (f64, Vec<f64>, &str) = (f64::MAX, init.to_vec(), first[0].0);
    let mut tried = Vec::new();
    let per_waveform = opts.max_seconds / 2.0;
    for (wf, _) in first.iter().take(2) {
        let t0 = std::time::Instant::now();
        let objective = |x: &DVector<f64>| -> f64 {
            eval(x.as_slice(), wf).total as f64 + penalty(x.as_slice())
        };
        let mut cma = match CMAESOptions::new(init.to_vec(), 0.2)
            .population_size(opts.population)
            .max_generations(opts.generations)
            .max_time(std::time::Duration::from_secs_f32(per_waveform))
            .seed(opts.seed)
            .build(objective)
        {
            Ok(c) => c,
            Err(_) => continue,
        };
        let r = cma.run_parallel();
        evaluations += cma.function_evals();
        if let Some(b) = r.overall_best {
            tried.push((wf.to_string(), b.value as f32));
            if b.value < best.0 {
                best = (b.value, b.point.as_slice().to_vec(), wf);
            }
        }
        let _ = t0;
    }
    let params = to_params(&best.1, best.2);
    let distance = eval(&best.1, best.2);
    FitResult {
        params,
        distance,
        initial_distance,
        tried,
        evaluations,
        seconds: started.elapsed().as_secs_f32(),
    }
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
}
