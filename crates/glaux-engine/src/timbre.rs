//! 単音の音色記述子 — AI が「1 つの音」を細かく知覚するための数値化。
//!
//! 仕様は Timbre Toolbox(Peeters ら 2011, JASA)の記述子群を下敷きにし、音色の知覚研究で
//! 重要とされる次元(立ち上がり時間・スペクトル重心・スペクトルの細かな凹凸。Caclin ら 2005)を
//! 優先して、シンセの音作りに翻訳しやすい形(ADSR・波形の推定・フィルタの動き)で返す。
//!
//! - 包絡: 立ち上がり(10%→90%)・減衰・持続レベル・余韻・形の粗い曲線
//! - 音程: 基本周波数・ずれ(セント)・安定度・しゃくり(冒頭の音程差)・ビブラート(速さと深さ)
//! - スペクトル: 重心・広がり・平坦さ・ロールオフ・フラックス、重心の時間変化(フィルタの開閉)
//! - 倍音: 16 次までの振幅・奇数/偶数倍音比・tristimulus・非調和性・倍音とノイズの比(HNR)・波形の推定
//! - 言葉のラベル: 上の数値を「アタックが鋭い」「矩形波寄り」などに言い換えたもの
//!
//! 音程は外から渡すこともできる([`PitchFrame`]。例えば学習済みモデルの推定)。無ければ内蔵の YIN を使う。

use rustfft::{num_complex::Complex, FftPlanner};
use serde::Serialize;

/// 解析する最大の長さ(秒)。サンプル音源の単音を想定
const MAX_SECONDS: f32 = 8.0;
/// 包絡・音程の刻み(秒)。外から渡す音程もこの刻みにそろえる([`resample_pitch`])
pub const ENV_HOP: f32 = 0.005;
/// 倍音を数える数
const N_HARMONICS: usize = 16;

/// 音程の推定結果 1 フレーム分(`f0 <= 0` は無声)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PitchFrame {
    pub time: f32,
    pub f0: f32,
    pub confidence: f32,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Envelope {
    /// 鳴り始め(-40dB を超えた所)からの、振幅 10% → 90% の時間
    pub attack_ms: f32,
    /// 鳴り始めから最大音量までの時間
    pub peak_ms: f32,
    /// 最大音量から持続レベル(+3dB 以内)に落ち着くまでの時間
    pub decay_ms: f32,
    /// 持続レベル(最大音量からの dB。減衰し続ける音では大きく負になる)
    pub sustain_db: f32,
    /// 持続の終わりから -40dB まで落ちる時間
    pub release_ms: f32,
    /// 最大音量の後、音量が落ち続ける(持続しない。プラック・打楽器・ピアノ的)
    pub decays_continuously: bool,
    /// 鳴っている長さ(-40dB を超えている区間)
    pub active_ms: f32,
    /// 鳴っている区間を 20 等分した各点の音量(最大音量からの dB)
    pub curve_db: Vec<f32>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Pitch {
    pub f0_hz: f32,
    /// 最も近い MIDI ノート番号と、そこからのずれ(セント)
    pub midi: u8,
    pub cents: f32,
    /// 音程が取れたフレームの割合(0〜1。低いと音程の無い音・ノイズ)
    pub voiced_ratio: f32,
    /// 持続部の音程のばらつき(セントの標準偏差)
    pub stability_cents: f32,
    /// 冒頭(最初の 60ms)と持続部の音程差(セント。負 = 下からしゃくる、正 = 上から下がる)
    pub glide_cents: f32,
    /// ビブラートの速さ(Hz。無ければ 0)と深さ(± セント)
    pub vibrato_rate_hz: f32,
    pub vibrato_depth_cents: f32,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Spectrum {
    pub centroid_hz: f32,
    pub spread_hz: f32,
    /// 平坦さ(0 = 純音的、1 = ノイズ的)
    pub flatness: f32,
    /// エネルギーの 85% が収まる周波数
    pub rolloff_hz: f32,
    /// 隣り合うフレームのスペクトルの変化量(平均。0〜)
    pub flux: f32,
    /// 重心の時間変化: 鳴り始め(最初の 15%)・中ほど(40〜60%)・終わり(70% 以降)の平均
    pub centroid_start_hz: f32,
    pub centroid_mid_hz: f32,
    pub centroid_end_hz: f32,
    /// 重心の推移(鳴っている区間を 8 等分した各区間の平均)
    pub centroid_curve_hz: Vec<f32>,
    /// 帯域エネルギーの割合(低 <250Hz / 中 / 高 >4kHz)
    pub band_low: f32,
    pub band_mid: f32,
    pub band_high: f32,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Harmonics {
    /// 1〜16 次の倍音の振幅(最も大きい倍音からの dB。見つからなければ -80)
    pub amplitudes_db: Vec<f32>,
    /// 奇数倍音(3,5,7..)と偶数倍音(2,4,6..)のエネルギー比(dB。大きいほど矩形波・三角波寄り)
    pub odd_even_db: f32,
    /// tristimulus: 基音 / 2〜4 次 / 5 次以上 の割合(合計 1)
    pub tristimulus: [f32; 3],
    /// 倍音の位置の、整数倍からのずれの平均(0 = 完全な整数倍。ベル・金属系で大きい)
    pub inharmonicity: f32,
    /// 倍音とそれ以外(ノイズ)のエネルギー比(dB)
    pub hnr_db: f32,
    /// 倍音の減り方(1 オクターブあたりの dB。ノコギリ・矩形 ≈ -6、三角 ≈ -12)
    pub slope_db_per_octave: f32,
    /// 波形の推定: sine / saw / square / triangle / noise / complex
    pub waveform_guess: String,
}

/// 単音の音色記述子。
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct SoundDescriptors {
    pub duration_ms: f32,
    pub peak_db: f32,
    pub rms_db: f32,
    pub envelope: Envelope,
    /// 音程の無い音(打楽器・ノイズ)では None
    pub pitch: Option<Pitch>,
    pub spectrum: Spectrum,
    /// 音程が取れたときだけ
    pub harmonics: Option<Harmonics>,
    /// 数値を言葉に言い換えたもの(AI・人間向け)
    pub labels: Vec<String>,
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

fn pow_db(p: f32) -> f32 {
    10.0 * p.max(1e-18).log10()
}

/// 音程のフレーム列を内蔵の YIN で求める(16kHz 程度に間引いてから。5ms 刻み)。
pub fn pitch_track(frames: &[f32], sr: f32) -> Vec<PitchFrame> {
    let step = ((sr / 16_000.0).round() as usize).max(1);
    let dsr = sr / step as f32;
    // 簡易な低域通過(移動平均)をかけて間引く
    let down: Vec<f32> = frames
        .chunks(step)
        .map(|c| c.iter().sum::<f32>() / c.len() as f32)
        .collect();
    let tau_min = (dsr / 2000.0) as usize; // 上限 2kHz
    let tau_max = (dsr / 40.0) as usize; // 下限 40Hz
    let win = tau_max * 2;
    let hop = ((ENV_HOP * dsr) as usize).max(1);
    let mut out = Vec::new();
    let mut i = 0;
    while i + win < down.len() {
        let buf = &down[i..i + win];
        let energy = buf.iter().map(|v| v * v).sum::<f32>() / win as f32;
        let t = (i + win / 2) as f32 / dsr;
        let pf = if energy < 1e-7 {
            PitchFrame {
                time: t,
                f0: 0.0,
                confidence: 0.0,
            }
        } else {
            match crate::transcribe::yin_pitch(buf, dsr, tau_min.max(2), tau_max) {
                Some((f, c)) if c > 0.5 => PitchFrame {
                    time: t,
                    f0: f,
                    confidence: c,
                },
                Some((_, c)) => PitchFrame {
                    time: t,
                    f0: 0.0,
                    confidence: c,
                },
                None => PitchFrame {
                    time: t,
                    f0: 0.0,
                    confidence: 0.0,
                },
            }
        };
        out.push(pf);
        i += hop;
    }
    out
}

/// 別の刻みで求めた音程(例: 学習済みモデルの 16ms ごとの推定)を [`ENV_HOP`] の刻みにそろえる。
/// 隣り合う 2 フレームがともに有声なら線形補間、片方だけなら近い方、どちらも無声なら無声。
pub fn resample_pitch(track: &[PitchFrame], duration: f32) -> Vec<PitchFrame> {
    if track.is_empty() {
        return Vec::new();
    }
    let n = (duration / ENV_HOP).ceil() as usize;
    let mut j = 0;
    (0..n)
        .map(|i| {
            let t = i as f32 * ENV_HOP;
            while j + 1 < track.len() && track[j + 1].time <= t {
                j += 1;
            }
            let a = track[j];
            let b = track.get(j + 1).copied().unwrap_or(a);
            let (f0, confidence) = if a.f0 > 0.0 && b.f0 > 0.0 && b.time > a.time {
                let r = ((t - a.time) / (b.time - a.time)).clamp(0.0, 1.0);
                // 音程は対数で補間する
                (
                    a.f0 * (b.f0 / a.f0).powf(r),
                    a.confidence + (b.confidence - a.confidence) * r,
                )
            } else {
                let near = if (t - a.time).abs() <= (b.time - t).abs() {
                    a
                } else {
                    b
                };
                (near.f0, near.confidence)
            };
            PitchFrame {
                time: t,
                f0,
                confidence,
            }
        })
        .collect()
}

/// 音程が取れない音(ベル・鐘など非調和な音)の代わりの音の高さ: 鳴り始め 0.5 秒のスペクトルで、
/// いちばん強い成分の 30% 以上ある最も低いピーク(40〜4000Hz)を MIDI ノート番号にする。
pub fn dominant_pitch(frames: &[f32], sr: f32) -> Option<u8> {
    const N: usize = 8192;
    let start = frames.iter().position(|v| v.abs() > 1e-3)?;
    let seg: Vec<f32> = frames[start..].iter().take(N).copied().collect();
    if seg.len() < 1024 {
        return None;
    }
    let fft = rustfft::FftPlanner::<f32>::new().plan_fft_forward(N);
    let window = hann(N);
    let mag = magnitude(&seg, N, &fft, &window);
    let bin_hz = sr / N as f32;
    let lo = (40.0 / bin_hz) as usize;
    let hi = ((4000.0 / bin_hz) as usize).min(mag.len() - 2);
    let max = mag[lo..hi].iter().cloned().fold(0.0f32, f32::max);
    if max <= 0.0 {
        return None;
    }
    let k = (lo.max(1)..hi)
        .find(|&k| mag[k] >= max * 0.3 && mag[k] >= mag[k - 1] && mag[k] >= mag[k + 1])?;
    let f = k as f32 * bin_hz;
    let midi = 69.0 + 12.0 * (f / 440.0).log2();
    Some(midi.round().clamp(0.0, 127.0) as u8)
}

/// 周波数 `freq` の成分の強さ(鳴り始めから最大 0.5 秒、Hann 窓の DFT の振幅)
fn partial_level(frames: &[f32], sr: f32, freq: f32) -> f32 {
    let Some(start) = frames.iter().position(|v| v.abs() > 1e-3) else {
        return 0.0;
    };
    let seg = &frames[start..frames.len().min(start + (sr * 0.5) as usize)];
    let n = seg.len() as f32;
    let (mut re, mut im) = (0.0f32, 0.0f32);
    for (k, v) in seg.iter().enumerate() {
        let w = 0.5 - 0.5 * (std::f32::consts::TAU * k as f32 / n).cos();
        let ph = k as f32 * freq * std::f32::consts::TAU / sr;
        re += v * w * ph.cos();
        im += v * w * ph.sin();
    }
    (re * re + im * im).sqrt()
}

/// 音程の推定がベルのような非調和な音で「成分の無い低い音」(基音の欠けた見かけの音程)になったとき、
/// 実際に鳴っているオクターブへ上げる。推定の音にも、その 3 倍にもほとんど成分が無く、
/// 2 倍(1 オクターブ上)に成分があるときだけ上げる(倍音が並ぶふつうの音で基音が弱いだけなら、
/// 3 倍に成分があるので上げない)
pub fn lift_missing_fundamental(frames: &[f32], sr: f32, midi: u8) -> u8 {
    let mut m = midi;
    for _ in 0..2 {
        let f = 440.0 * 2f32.powf((m as f32 - 69.0) / 12.0);
        if f * 2.0 > sr * 0.45 {
            break;
        }
        let (e1, e2, e3) = (
            partial_level(frames, sr, f),
            partial_level(frames, sr, f * 2.0),
            partial_level(frames, sr, f * 3.0),
        );
        if e2 > 0.0 && e1 < 0.1 * e2 && e3 < 0.1 * e2 && m <= 115 {
            m += 12;
        } else {
            break;
        }
    }
    m
}

/// 単音を解析する。`pitch` を渡せばそれを使い、無ければ内蔵の YIN で求める。
pub fn describe(frames: &[f32], sr: f32, pitch: Option<&[PitchFrame]>) -> SoundDescriptors {
    let n = frames.len().min((MAX_SECONDS * sr) as usize);
    let x = &frames[..n];
    let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let rms = (x.iter().map(|v| v * v).sum::<f32>() / n.max(1) as f32).sqrt();

    let envelope = envelope(x, sr);
    let own_track;
    let track: &[PitchFrame] = match pitch {
        Some(p) => p,
        None => {
            own_track = pitch_track(x, sr);
            &own_track
        }
    };
    // 鳴っている区間(秒)
    let hop = ENV_HOP;
    let (act_start, act_end) = active_range(x, sr);
    let (a0, a1) = (act_start as f32 / sr, act_end as f32 / sr);
    let pitch = describe_pitch(track, a0, a1, &envelope, hop);
    let spectrum = describe_spectrum(x, sr, act_start, act_end);
    let harmonics = pitch
        .as_ref()
        .map(|p| describe_harmonics(x, sr, act_start, act_end, &envelope, p.f0_hz));
    let mut d = SoundDescriptors {
        duration_ms: n as f32 / sr * 1000.0,
        peak_db: db(peak),
        rms_db: db(rms),
        envelope,
        pitch,
        spectrum,
        harmonics,
        labels: vec![],
    };
    d.labels = labels(&d);
    d
}

/// 鳴っている区間(-40dB を超えている最初と最後のサンプル)。
fn active_range(x: &[f32], sr: f32) -> (usize, usize) {
    let hop = ((ENV_HOP * sr) as usize).max(1);
    let env: Vec<f32> = x
        .chunks(hop)
        .map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt())
        .collect();
    let peak = env.iter().fold(0.0f32, |m, v| m.max(*v));
    let th = peak * 0.01;
    let first = env.iter().position(|v| *v > th).unwrap_or(0);
    let last = env
        .iter()
        .rposition(|v| *v > th)
        .unwrap_or(env.len().saturating_sub(1));
    (first * hop, ((last + 1) * hop).min(x.len()))
}

fn envelope(x: &[f32], sr: f32) -> Envelope {
    let hop = ((ENV_HOP * sr) as usize).max(1);
    let win = hop * 2;
    let mut env = Vec::new();
    let mut i = 0;
    while i < x.len() {
        let end = (i + win).min(x.len());
        let c = &x[i..end];
        env.push((c.iter().map(|v| v * v).sum::<f32>() / c.len().max(1) as f32).sqrt());
        i += hop;
    }
    let ms = |frames: usize| frames as f32 * ENV_HOP * 1000.0;
    let peak = env.iter().fold(0.0f32, |m, v| m.max(*v)).max(1e-9);
    let pi = env.iter().position(|v| *v >= peak).unwrap_or(0);
    let start = env.iter().position(|v| *v > peak * 0.01).unwrap_or(0);
    let end = env
        .iter()
        .rposition(|v| *v > peak * 0.01)
        .unwrap_or(0)
        .max(start);
    let t10 = env[start..=pi]
        .iter()
        .position(|v| *v >= peak * 0.1)
        .map_or(start, |p| start + p);
    let t90 = env[start..=pi]
        .iter()
        .position(|v| *v >= peak * 0.9)
        .map_or(pi, |p| start + p);
    let dbs: Vec<f32> = env.iter().map(|v| db(*v / peak)).collect();
    // 持続部の判定: 最大音量の後で、音量の傾きが緩い(±6dB/秒 以内。50ms 幅で見る)フレーム。
    // 持続する音(オルガン・シンセのサステイン)はこれが多く、減衰し続ける音(プラック・ピアノ)はほぼ無い
    let span = ((0.05 / ENV_HOP) as usize).max(1);
    let flat: Vec<usize> = (pi..end.saturating_sub(span))
        .filter(|&k| {
            let slope = (dbs[k + span] - dbs[k]) / (span as f32 * ENV_HOP);
            slope.abs() < 6.0 && dbs[k] > -40.0
        })
        .collect();
    let post = end.saturating_sub(pi).max(1);
    let decays_continuously = (flat.len() as f32) < post as f32 * 0.2;
    let (sustain_db, sus_end) = if decays_continuously || flat.is_empty() {
        // 持続しない: 中ほどの音量を持続レベルとし、余韻は最大音量から数える
        (dbs[(pi + post / 2).min(dbs.len() - 1)], pi)
    } else {
        let mut lv: Vec<f32> = flat.iter().map(|&k| dbs[k]).collect();
        lv.sort_by(f32::total_cmp);
        (
            lv[lv.len() / 2],
            (*flat.last().unwrap_or(&pi) + span).min(end),
        )
    };
    let decay_end = dbs[pi..=end]
        .iter()
        .position(|v| *v <= sustain_db + 3.0)
        .map_or(end, |p| pi + p);
    let curve_db = (0..20)
        .map(|k| {
            let idx = start + (end - start) * k / 19;
            dbs[idx.min(dbs.len() - 1)]
        })
        .collect();
    Envelope {
        attack_ms: ms(t90.saturating_sub(t10)),
        peak_ms: ms(pi.saturating_sub(start)),
        decay_ms: ms(decay_end.saturating_sub(pi)),
        sustain_db,
        release_ms: ms(end.saturating_sub(sus_end)),
        decays_continuously,
        active_ms: ms(end.saturating_sub(start)),
        curve_db,
    }
}

fn describe_pitch(
    track: &[PitchFrame],
    a0: f32,
    a1: f32,
    env: &Envelope,
    _hop: f32,
) -> Option<Pitch> {
    let active: Vec<&PitchFrame> = track
        .iter()
        .filter(|p| p.time >= a0 && p.time <= a1)
        .collect();
    if active.is_empty() {
        return None;
    }
    let voiced: Vec<&PitchFrame> = active.iter().copied().filter(|p| p.f0 > 0.0).collect();
    let voiced_ratio = voiced.len() as f32 / active.len() as f32;
    if voiced.len() < 4 || voiced_ratio < 0.3 {
        return None;
    }
    let to_midi = |f: f32| 69.0 + 12.0 * (f / 440.0).log2();
    // 持続部: 最大音量の後〜持続の終わり(なければ中央の 60%)
    let len = a1 - a0;
    let s0 = a0 + (env.peak_ms / 1000.0).max(len * 0.2);
    let s1 = a0 + len * 0.85;
    let sustain: Vec<f32> = voiced
        .iter()
        .filter(|p| p.time >= s0 && p.time <= s1)
        .map(|p| to_midi(p.f0))
        .collect();
    let body = if sustain.len() >= 4 {
        sustain
    } else {
        voiced.iter().map(|p| to_midi(p.f0)).collect()
    };
    let mut sorted = body.clone();
    sorted.sort_by(f32::total_cmp);
    let med = sorted[sorted.len() / 2];
    let mean = body.iter().sum::<f32>() / body.len() as f32;
    let std = (body.iter().map(|m| (m - mean).powi(2)).sum::<f32>() / body.len() as f32).sqrt();
    // しゃくり: 最初の 3 フレーム(有声)の中央値と持続部の差。
    // 鳴り始めから 150ms 以上たってから音程が取れた場合は判定しない
    let head: Vec<f32> = voiced.iter().take(3).map(|p| to_midi(p.f0)).collect();
    let mut hs = head.clone();
    hs.sort_by(f32::total_cmp);
    let glide = if hs.is_empty() || voiced[0].time > a0 + 0.15 {
        0.0
    } else {
        (hs[hs.len() / 2] - med) * 100.0
    };
    // ビブラート: 持続部の音程(セント)の揺れから、ゼロ交差で速さ、振幅で深さ
    let (rate, depth) = vibrato(&body, ENV_HOP);
    let midi = med.round().clamp(0.0, 127.0);
    Some(Pitch {
        f0_hz: 440.0 * 2f32.powf((med - 69.0) / 12.0),
        midi: midi as u8,
        cents: (med - midi) * 100.0,
        voiced_ratio,
        stability_cents: std * 100.0,
        glide_cents: glide,
        vibrato_rate_hz: rate,
        vibrato_depth_cents: depth,
    })
}

/// 音程の列(MIDI 小数)からビブラートの速さ(Hz)と深さ(± セント)を求める。
fn vibrato(midi: &[f32], hop: f32) -> (f32, f32) {
    if midi.len() < 40 {
        return (0.0, 0.0);
    }
    // ゆっくりした変化(しゃくり・下がり)を除くため、約 0.25 秒の移動平均を引く
    let w = ((0.25 / hop) as usize).max(3);
    let dev: Vec<f32> = (0..midi.len())
        .map(|i| {
            let lo = i.saturating_sub(w / 2);
            let hi = (i + w / 2 + 1).min(midi.len());
            let m = midi[lo..hi].iter().sum::<f32>() / (hi - lo) as f32;
            (midi[i] - m) * 100.0
        })
        .collect();
    let rms = (dev.iter().map(|v| v * v).sum::<f32>() / dev.len() as f32).sqrt();
    if rms < 4.0 {
        return (0.0, 0.0);
    }
    let crossings = dev.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
    let secs = dev.len() as f32 * hop;
    let rate = crossings as f32 / secs;
    if !(2.5..=12.0).contains(&rate) {
        return (0.0, 0.0);
    }
    (rate, rms * std::f32::consts::SQRT_2)
}

/// ハン窓付きの振幅スペクトル。
fn magnitude(
    x: &[f32],
    n: usize,
    fft: &std::sync::Arc<dyn rustfft::Fft<f32>>,
    window: &[f32],
) -> Vec<f32> {
    let mut buf: Vec<Complex<f32>> = (0..n)
        .map(|i| Complex::new(x.get(i).copied().unwrap_or(0.0) * window[i], 0.0))
        .collect();
    fft.process(&mut buf);
    buf[..n / 2 + 1].iter().map(|c| c.norm()).collect()
}

fn hann(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos())
        .collect()
}

fn describe_spectrum(x: &[f32], sr: f32, s: usize, e: usize) -> Spectrum {
    const N: usize = 2048;
    let hop = N / 4;
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N);
    let w = hann(N);
    let bin_hz = sr / N as f32;
    // (パワー合計, [重心, 広がり, 平坦さ, ロールオフ], 帯域エネルギー, 振幅スペクトル)
    type FrameStats = (f32, [f32; 4], [f32; 3], Vec<f32>);
    let mut frames: Vec<FrameStats> = Vec::new();
    let mut i = s;
    while i + N / 2 < e.max(s + 1) || frames.is_empty() {
        let mag = magnitude(&x[i.min(x.len())..], N, &fft, &w);
        let power: Vec<f32> = mag.iter().map(|m| m * m).collect();
        let total: f32 = power.iter().sum::<f32>().max(1e-18);
        let centroid = power
            .iter()
            .enumerate()
            .map(|(k, p)| k as f32 * bin_hz * p)
            .sum::<f32>()
            / total;
        let spread = (power
            .iter()
            .enumerate()
            .map(|(k, p)| (k as f32 * bin_hz - centroid).powi(2) * p)
            .sum::<f32>()
            / total)
            .sqrt();
        let geo = (mag.iter().skip(1).map(|m| (m + 1e-12).ln()).sum::<f32>()
            / (mag.len() - 1) as f32)
            .exp();
        let arith = mag.iter().skip(1).sum::<f32>() / (mag.len() - 1) as f32;
        let flatness = (geo / arith.max(1e-12)).clamp(0.0, 1.0);
        let mut acc = 0.0;
        let mut rolloff = 0.0;
        for (k, p) in power.iter().enumerate() {
            acc += p;
            if acc >= total * 0.85 {
                rolloff = k as f32 * bin_hz;
                break;
            }
        }
        let mut bands = [0.0f32; 3];
        for (k, p) in power.iter().enumerate() {
            let f = k as f32 * bin_hz;
            let b = if f < 250.0 {
                0
            } else if f < 4000.0 {
                1
            } else {
                2
            };
            bands[b] += p;
        }
        frames.push((total, [centroid, spread, flatness, rolloff], bands, mag));
        i += hop;
        if i >= x.len() {
            break;
        }
    }
    // フラックス(正規化したスペクトルの差)
    let mut flux_sum = 0.0;
    for k in 1..frames.len() {
        let a = &frames[k - 1].3;
        let b = &frames[k].3;
        let na = a.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-9);
        let nb = b.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-9);
        let d: f32 = a
            .iter()
            .zip(b)
            .map(|(p, q)| (q / nb - p / na).powi(2))
            .sum::<f32>()
            .sqrt();
        flux_sum += d;
    }
    let flux = if frames.len() > 1 {
        flux_sum / (frames.len() - 1) as f32
    } else {
        0.0
    };
    // エネルギーで重み付けした平均
    let wsum: f32 = frames.iter().map(|f| f.0).sum::<f32>().max(1e-18);
    let avg = |j: usize| frames.iter().map(|f| f.1[j] * f.0).sum::<f32>() / wsum;
    // 区間 [lo, hi)(割合)の重心の平均(エネルギーで重み付け)
    let seg_centroid = |lo: f32, hi: f32| {
        let n = frames.len();
        let a = ((n as f32 * lo) as usize).min(n - 1);
        let b = ((n as f32 * hi).ceil() as usize).clamp(a + 1, n);
        let seg = &frames[a..b];
        let ws: f32 = seg.iter().map(|f| f.0).sum::<f32>().max(1e-18);
        seg.iter().map(|f| f.1[0] * f.0).sum::<f32>() / ws
    };
    let bands: [f32; 3] = [0, 1, 2].map(|b| frames.iter().map(|f| f.2[b]).sum::<f32>());
    let btot = bands.iter().sum::<f32>().max(1e-18);
    Spectrum {
        centroid_hz: avg(0),
        spread_hz: avg(1),
        flatness: avg(2),
        rolloff_hz: avg(3),
        flux,
        centroid_start_hz: seg_centroid(0.0, 0.15),
        centroid_mid_hz: seg_centroid(0.4, 0.6),
        centroid_end_hz: seg_centroid(0.7, 1.0),
        centroid_curve_hz: (0..8)
            .map(|k| seg_centroid(k as f32 / 8.0, (k + 1) as f32 / 8.0))
            .collect(),
        band_low: bands[0] / btot,
        band_mid: bands[1] / btot,
        band_high: bands[2] / btot,
    }
}

fn describe_harmonics(
    x: &[f32],
    sr: f32,
    s: usize,
    e: usize,
    env: &Envelope,
    f0: f32,
) -> Harmonics {
    // 持続部(立ち上がりの後〜鳴っている区間の 85%)の平均スペクトル。分解能のため長めの窓
    const N: usize = 8192;
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N);
    let w = hann(N);
    let bin_hz = sr / N as f32;
    let start = s + ((env.peak_ms / 1000.0) * sr) as usize;
    let end = (s + ((e - s) as f32 * 0.85) as usize).max(start + 1);
    let mut avg = vec![0.0f32; N / 2 + 1];
    let mut count = 0;
    let mut i = start.min(x.len().saturating_sub(1));
    loop {
        let mag = magnitude(&x[i..], N, &fft, &w);
        for (a, m) in avg.iter_mut().zip(&mag) {
            *a += m * m;
        }
        count += 1;
        i += N / 2;
        if i + N / 2 >= end || i >= x.len() {
            break;
        }
    }
    for a in avg.iter_mut() {
        *a /= count as f32;
    }
    let nyq = sr / 2.0;
    let mut amps = [0.0f32; N_HARMONICS]; // パワー
    let mut devs = Vec::new();
    let mut harm_power = 0.0;
    let mut harmonic_bins = vec![false; avg.len()];
    for k in 1..=40usize {
        let target = f0 * k as f32;
        if target >= nyq * 0.95 {
            break;
        }
        let tb = target / bin_hz;
        let tol = (tb * 0.03).max(2.0);
        let lo = (tb - tol).floor().max(1.0) as usize;
        let hi = ((tb + tol).ceil() as usize).min(avg.len() - 2);
        if lo >= hi {
            continue;
        }
        let pk = (lo..=hi)
            .max_by(|a, b| avg[*a].total_cmp(&avg[*b]))
            .unwrap_or(lo);
        // ピークの周辺 ±2 ビンを倍音のエネルギーとして数える
        let mut p = 0.0;
        for b in pk.saturating_sub(2)..=(pk + 2).min(avg.len() - 1) {
            p += avg[b];
            harmonic_bins[b] = true;
        }
        harm_power += p;
        if k <= N_HARMONICS {
            amps[k - 1] = p;
            if p > 0.0 {
                // 放物線補間でピーク周波数を詰める
                let (a, b, c) = (
                    avg[pk - 1].max(1e-18).ln(),
                    avg[pk].max(1e-18).ln(),
                    avg[pk + 1].max(1e-18).ln(),
                );
                let den = a - 2.0 * b + c;
                let off = if den.abs() > 1e-9 {
                    0.5 * (a - c) / den
                } else {
                    0.0
                };
                let fk = (pk as f32 + off) * bin_hz;
                devs.push(((fk - target) / target).abs());
            }
        }
    }
    let band_top = ((f0 * 40.0).min(nyq) / bin_hz) as usize;
    let total: f32 = avg[1..band_top.min(avg.len())]
        .iter()
        .sum::<f32>()
        .max(1e-18);
    let noise = (total - harm_power).max(total * 1e-4);
    let max_amp = amps.iter().fold(0.0f32, |m, v| m.max(*v)).max(1e-18);
    let amplitudes_db: Vec<f32> = amps
        .iter()
        .map(|p| pow_db(p / max_amp).max(-80.0))
        .collect();
    let odd: f32 = amps
        .iter()
        .enumerate()
        .filter(|(i, _)| (i + 1) % 2 == 1 && *i >= 2)
        .map(|(_, p)| p)
        .sum();
    let even: f32 = amps
        .iter()
        .enumerate()
        .filter(|(i, _)| (i + 1) % 2 == 0)
        .map(|(_, p)| p)
        .sum();
    let a: Vec<f32> = amps.iter().map(|p| p.sqrt()).collect();
    let asum = a.iter().sum::<f32>().max(1e-12);
    let tristimulus = [
        a[0] / asum,
        (a[1] + a[2] + a[3]) / asum,
        a[4..].iter().sum::<f32>() / asum,
    ];
    // 倍音の減り方: 見えている倍音(-60dB 以上)の振幅の、log2(次数) に対する回帰の傾き
    let pts: Vec<(f32, f32)> = amplitudes_db
        .iter()
        .enumerate()
        .filter(|(_, d)| **d > -60.0)
        .map(|(i, d)| (((i + 1) as f32).log2(), *d))
        .collect();
    let slope = if pts.len() >= 3 {
        let mx = pts.iter().map(|p| p.0).sum::<f32>() / pts.len() as f32;
        let my = pts.iter().map(|p| p.1).sum::<f32>() / pts.len() as f32;
        let num: f32 = pts.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
        let den: f32 = pts.iter().map(|p| (p.0 - mx).powi(2)).sum();
        if den > 0.0 {
            num / den
        } else {
            0.0
        }
    } else {
        -40.0
    };
    let hnr_db = pow_db(harm_power / noise);
    let odd_even_db = pow_db(odd.max(1e-18) / even.max(1e-18)).clamp(-60.0, 60.0);
    let inharmonicity = if devs.is_empty() {
        0.0
    } else {
        devs.iter().sum::<f32>() / devs.len() as f32
    };
    let waveform_guess = if hnr_db < 3.0 {
        "noise"
    } else if tristimulus[0] > 0.85 || amplitudes_db[1..].iter().all(|d| *d < -30.0) {
        "sine"
    } else if inharmonicity > 0.02 {
        "complex"
    } else if odd_even_db > 12.0 {
        if slope < -9.0 {
            "triangle"
        } else {
            "square"
        }
    } else {
        "saw"
    }
    .to_owned();
    Harmonics {
        amplitudes_db,
        odd_even_db,
        tristimulus,
        inharmonicity,
        hnr_db,
        slope_db_per_octave: slope,
        waveform_guess,
    }
}

/// 数値を言葉に言い換える(音作りの手がかり)。
fn labels(d: &SoundDescriptors) -> Vec<String> {
    let mut out = Vec::new();
    let e = &d.envelope;
    out.push(
        if e.attack_ms < 6.0 {
            "アタックが鋭い"
        } else if e.attack_ms < 40.0 {
            "アタックは普通"
        } else if e.attack_ms < 150.0 {
            "やや遅く立ち上がる"
        } else {
            "ゆっくり立ち上がる(パッド・ストリングス的)"
        }
        .to_owned(),
    );
    if e.decays_continuously {
        out.push("減衰し続ける(プラック・打楽器・ピアノ的)".into());
    } else if e.sustain_db > -6.0 {
        out.push("音量が持続する(オルガン・シンセリード的)".into());
    } else if e.sustain_db > -20.0 {
        out.push("減衰して一定の音量に落ち着く".into());
    } else {
        out.push("減衰し続ける(プラック・打楽器・ピアノ的)".into());
    }
    if e.release_ms > 300.0 {
        out.push(format!("余韻が長い({:.0}ms)", e.release_ms));
    }
    match &d.pitch {
        None => out.push("はっきりした音程が無い(打楽器・ノイズ的)".into()),
        Some(p) => {
            if p.glide_cents < -40.0 {
                out.push(format!(
                    "下から音程がせり上がる(しゃくり {:.0} セント)",
                    p.glide_cents
                ));
            } else if p.glide_cents > 40.0 {
                out.push(format!(
                    "上から音程が下がる({:+.0} セント。808 のピッチ下降など)",
                    p.glide_cents
                ));
            }
            if p.vibrato_rate_hz > 0.0 {
                out.push(format!(
                    "ビブラートあり({:.1}Hz、±{:.0} セント)",
                    p.vibrato_rate_hz, p.vibrato_depth_cents
                ));
            }
            if p.stability_cents > 25.0 && p.vibrato_rate_hz == 0.0 {
                out.push("音程が揺れている・不安定".into());
            }
        }
    }
    if let Some(h) = &d.harmonics {
        out.push(
            match h.waveform_guess.as_str() {
                "sine" => "ほぼ基音だけ(サイン波的。丸く柔らかい)",
                "saw" => "偶数・奇数の倍音がそろう(ノコギリ波寄り。明るくブラス・ストリングス的)",
                "square" => "奇数倍音が多い(矩形波寄り。中空でクラリネット・チップチューン的)",
                "triangle" => "奇数倍音が少しだけ(三角波寄り。柔らかい)",
                "complex" => "倍音が整数倍からずれる(ベル・金属・FM 的)",
                _ => "ノイズ成分が主体",
            }
            .to_owned(),
        );
        if h.hnr_db < 10.0 && h.waveform_guess != "noise" {
            out.push("息・ノイズ成分が多い".into());
        }
    }
    let s = &d.spectrum;
    let bright = match &d.pitch {
        Some(p) => s.centroid_hz / p.f0_hz.max(1.0),
        None => s.centroid_hz / 500.0,
    };
    out.push(
        if s.centroid_hz > 3500.0 || bright > 8.0 {
            "明るい・きらびやか"
        } else if s.centroid_hz < 600.0 || bright < 2.0 {
            "暗い・こもった"
        } else {
            "明るさは中くらい"
        }
        .to_owned(),
    );
    if s.centroid_start_hz > s.centroid_end_hz * 1.4 {
        out.push(
            "鳴り始めが明るく、だんだん暗くなる(フィルタが閉じていく・フィルタエンベロープ)".into(),
        );
    } else if s.centroid_end_hz > s.centroid_start_hz * 1.4 {
        out.push("だんだん明るくなる(フィルタが開いていく)".into());
    }
    if s.band_low > 0.6 {
        out.push("低域が中心".into());
    } else if s.band_high > 0.3 {
        out.push("高域が多い".into());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// 倍音の振幅を指定して合成する(ADSR つき)
    fn synth(
        f0: f32,
        harm: impl Fn(usize) -> f32,
        secs: f32,
        attack: f32,
        release: f32,
    ) -> Vec<f32> {
        let n = (secs * SR) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / SR;
                let env = (t / attack).min(1.0) * ((secs - t) / release).clamp(0.0, 1.0);
                let mut v = 0.0;
                for k in 1..40 {
                    let f = f0 * k as f32;
                    if f > SR / 2.0 {
                        break;
                    }
                    v += harm(k) * (std::f32::consts::TAU * f * t).sin();
                }
                0.3 * env * v
            })
            .collect()
    }

    #[test]
    fn distinguishes_waveforms() {
        let saw = synth(220.0, |k| 1.0 / k as f32, 1.0, 0.005, 0.05);
        let square = synth(
            220.0,
            |k| if k % 2 == 1 { 1.0 / k as f32 } else { 0.0 },
            1.0,
            0.005,
            0.05,
        );
        let triangle = synth(
            220.0,
            |k| {
                if k % 2 == 1 {
                    1.0 / (k * k) as f32
                } else {
                    0.0
                }
            },
            1.0,
            0.005,
            0.05,
        );
        let sine = synth(220.0, |k| if k == 1 { 1.0 } else { 0.0 }, 1.0, 0.005, 0.05);
        for (x, want) in [
            (saw, "saw"),
            (square, "square"),
            (triangle, "triangle"),
            (sine, "sine"),
        ] {
            let d = describe(&x, SR, None);
            let h = d.harmonics.as_ref().expect("倍音が取れる");
            eprintln!(
                "{want}: odd/even {:.1}dB slope {:.1} T={:?}",
                h.odd_even_db, h.slope_db_per_octave, h.tristimulus
            );
            assert_eq!(h.waveform_guess, want);
            let p = d.pitch.as_ref().unwrap();
            assert_eq!(p.midi, 57, "A3");
            assert!(p.cents.abs() < 10.0);
        }
    }

    #[test]
    fn measures_envelope() {
        // 立ち上がり 200ms の持続音(パッド)と、鋭く立ち上がって減衰し続ける音(プラック)
        let pad = synth(330.0, |k| 1.0 / k as f32, 1.5, 0.2, 0.3);
        let d = describe(&pad, SR, None);
        eprintln!("pad {:?}", d.envelope);
        assert!(
            (120.0..220.0).contains(&d.envelope.attack_ms),
            "{}",
            d.envelope.attack_ms
        );
        assert!(d.envelope.sustain_db > -6.0);
        assert!(d
            .labels
            .iter()
            .any(|l| l.contains("ゆっくり") || l.contains("やや遅く")));

        let n = SR as usize;
        let pluck: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / SR;
                0.5 * (-t * 6.0).exp() * (std::f32::consts::TAU * 330.0 * t).sin()
            })
            .collect();
        let d = describe(&pluck, SR, None);
        eprintln!("pluck {:?}", d.envelope);
        assert!(d.envelope.attack_ms < 10.0);
        assert!(d.labels.iter().any(|l| l.contains("減衰し続ける")));
    }

    #[test]
    fn detects_vibrato_and_glide() {
        let n = (2.0 * SR) as usize;
        let mut phase = 0.0f32;
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / SR;
                // 冒頭 80ms は 1 半音下からせり上がり、その後 5.5Hz・±40 セントのビブラート
                let glide = if t < 0.08 {
                    -100.0 * (1.0 - t / 0.08)
                } else {
                    0.0
                };
                let vib = 40.0 * (std::f32::consts::TAU * 5.5 * t).sin();
                let f = 440.0 * 2f32.powf((glide + vib) / 1200.0);
                phase += std::f32::consts::TAU * f / SR;
                0.3 * phase.sin()
            })
            .collect();
        let d = describe(&x, SR, None);
        let p = d.pitch.unwrap();
        eprintln!("{p:?}");
        assert_eq!(p.midi, 69);
        assert!(
            (4.5..6.5).contains(&p.vibrato_rate_hz),
            "{}",
            p.vibrato_rate_hz
        );
        assert!(
            (20.0..60.0).contains(&p.vibrato_depth_cents),
            "{}",
            p.vibrato_depth_cents
        );
        assert!(p.glide_cents < -30.0, "{}", p.glide_cents);
    }

    #[test]
    fn missing_fundamental_is_lifted_only_for_inharmonic_sounds() {
        let sr = 48_000.0;
        let tone = |parts: &[(f32, f32)]| -> Vec<f32> {
            (0..24_000)
                .map(|i| {
                    let t = i as f32 / sr;
                    parts
                        .iter()
                        .map(|(f, a)| a * (std::f32::consts::TAU * f * t).sin())
                        .sum()
                })
                .collect()
        };
        // ベル: 523Hz と、その 3.5 倍(見かけの基音は 262Hz = MIDI 60)→ 72 に直す
        let bell = tone(&[(523.25, 1.0), (523.25 * 3.5, 0.6)]);
        assert_eq!(lift_missing_fundamental(&bell, sr, 60), 72);
        // 基音の弱い倍音列(262Hz の 2・3・4 倍)は 60 のまま(3 倍に成分がある)
        let harm = tone(&[(523.25, 1.0), (784.9, 0.8), (1046.5, 0.5)]);
        assert_eq!(lift_missing_fundamental(&harm, sr, 60), 60);
        // ふつうの音(基音あり)は変えない
        let saw = tone(&[(261.6, 1.0), (523.25, 0.5), (784.9, 0.33)]);
        assert_eq!(lift_missing_fundamental(&saw, sr, 60), 60);
    }

    #[test]
    fn dominant_pitch_finds_the_lowest_strong_partial() {
        let sr = 48_000.0;
        // 523Hz(C5)と、その 3.5 倍・4.7 倍の非調和な成分
        let x: Vec<f32> = (0..24_000)
            .map(|i| {
                let t = i as f32 / sr;
                (std::f32::consts::TAU * 523.25 * t).sin()
                    + 0.6 * (std::f32::consts::TAU * 523.25 * 3.5 * t).sin()
                    + 0.4 * (std::f32::consts::TAU * 523.25 * 4.7 * t).sin()
            })
            .collect();
        assert_eq!(dominant_pitch(&x, sr), Some(72));
    }

    #[test]
    fn resampled_pitch_interpolates_and_keeps_unvoiced() {
        let f = |time, f0| PitchFrame {
            time,
            f0,
            confidence: if f0 > 0.0 { 0.9 } else { 0.1 },
        };
        let track = [f(0.0, 0.0), f(0.016, 220.0), f(0.032, 440.0), f(0.048, 0.0)];
        let r = resample_pitch(&track, 0.064);
        assert_eq!(r.len(), 13);
        assert_eq!(r[0].f0, 0.0);
        // 0.016〜0.032 の中間(0.024 付近)は対数で補間されて約 311Hz
        let mid = r.iter().find(|p| (p.time - 0.025).abs() < 1e-4).unwrap();
        assert!(
            (mid.f0 - 220.0 * 2f32.powf(0.5625)).abs() < 1.0,
            "{}",
            mid.f0
        );
        assert_eq!(r.last().unwrap().f0, 0.0);
    }

    #[test]
    fn noise_has_no_pitch() {
        let mut seed = 1u32;
        let x: Vec<f32> = (0..(SR as usize / 2))
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((seed >> 9) as f32 / (1u32 << 23) as f32 * 2.0 - 1.0) * 0.3
            })
            .collect();
        let d = describe(&x, SR, None);
        assert!(d.pitch.is_none());
        assert!(d.spectrum.flatness > 0.5, "{}", d.spectrum.flatness);
    }

    #[test]
    fn filter_sweep_is_labelled() {
        // 鳴り始めは倍音が多く、だんだん基音だけになる
        let n = (1.5 * SR) as usize;
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / SR;
                let bright = (-t * 1.5).exp();
                let mut v = 0.0;
                for k in 1..30 {
                    v += (bright.powf((k - 1) as f32) / k as f32)
                        * (std::f32::consts::TAU * 110.0 * k as f32 * t).sin();
                }
                0.3 * v
            })
            .collect();
        let d = describe(&x, SR, None);
        eprintln!("{:?}", d.spectrum);
        assert!(d.labels.iter().any(|l| l.contains("だんだん暗く")));
    }
}
