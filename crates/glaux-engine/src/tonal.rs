//! 音の高さの解析: チューニングのずれ・音名の強さ(NNLS クロマ)・和音・調。曲から起こす機能の「骨組み」の一部。
//!
//! - チューニング: スペクトルの山の周波数を細かく求め、最寄りの半音からのずれ(セント)を円周上で平均する
//! - 音の強さ: Mauch と Dixon(2010)の NNLS クロマに倣う。1/3 半音刻みの対数周波数のスペクトルを、
//!   チューニングに合わせて作り、1 オクターブの移動平均・標準偏差で白色化してから、「倍音を含んだ音の形」
//!   (h 倍音の強さ s^(h-1))の非負の重ね合わせで説明して、88 鍵それぞれの強さを求める。倍音を音の形が
//!   引き受けるので、単純なクロマより「C の和音の E の倍音の B」に惑わされにくい
//! - 和音: 拍ごとの高音側・低音側のクロマを、和音の形(根音・種類・最低音)に当てた点数を出し、
//!   隠れマルコフモデル(ビタビ)で滑らかにつなぐ。和音は小節頭・小節の中ほどで変わりやすいとする
//! - 調: 曲全体の音の強さの分布を、調ごとの重み(Krumhansl)に当てる
//!
//! すべてオフライン(UI・MCP のスレッドで呼ぶ)。

use crate::mir_eval::ChordSpan;
use rustfft::{num_complex::Complex, FftPlanner};

/// 対数周波数の 1 半音あたりの数
const BPS: usize = 3;
/// 88 鍵(A0 = 21 〜 C8 = 108)
const LOW: f64 = 21.0;
pub const NOTES: usize = 88;
const LOG_BINS: usize = NOTES * BPS;

const NAMES: [&str; 12] = [
    "C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
];

/// 解析の窓(約 0.34 秒。低音の半音を分けるため長め)と刻み
fn frame_size(sr: f32) -> (usize, usize) {
    let n = ((sr as f64 * 0.34) as usize).next_power_of_two();
    (n, n / 4)
}

/// 窓を掛けた振幅スペクトルを、刻みごとに `f(フレーム番号, 振幅)` へ渡す
fn for_each_spectrum(frames: &[f32], sr: f32, mut f: impl FnMut(usize, &[f32])) {
    let (n, hop) = frame_size(sr);
    if frames.len() < n {
        return;
    }
    let fft = FftPlanner::<f32>::new().plan_fft_forward(n);
    let hann: Vec<f32> = (0..n)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos())
        .collect();
    let mut buf = vec![Complex::new(0.0f32, 0.0); n];
    let mut mag = vec![0.0f32; n / 2];
    let mut start = 0;
    let mut idx = 0;
    while start + n <= frames.len() {
        for (i, c) in buf.iter_mut().enumerate() {
            *c = Complex::new(frames[start + i] * hann[i], 0.0);
        }
        fft.process(&mut buf);
        for (m, c) in mag.iter_mut().zip(&buf) {
            *m = c.norm();
        }
        f(idx, &mag);
        start += hop;
        idx += 1;
    }
}

/// チューニングのずれ(セント、A = 440Hz からの。-50〜+50)を推定する。音程のある音が無ければ 0
pub fn estimate_tuning(frames: &[f32], sr: f32) -> f64 {
    let (n, _) = frame_size(sr);
    let bin_hz = sr as f64 / n as f64;
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for_each_spectrum(frames, sr, |_, mag| {
        let lo = (80.0 / bin_hz) as usize;
        let hi = ((2000.0 / bin_hz) as usize).min(mag.len() - 2);
        let max = mag[lo..hi].iter().cloned().fold(0.0f32, f32::max);
        if max <= 1e-6 {
            return;
        }
        for k in lo.max(1)..hi {
            let m = mag[k];
            if m < max * 0.1 || m < mag[k - 1] || m <= mag[k + 1] {
                continue;
            }
            // 対数振幅の放物線で山の位置を細かく
            let (a, b, c) = (
                (mag[k - 1] as f64 + 1e-9).ln(),
                (m as f64 + 1e-9).ln(),
                (mag[k + 1] as f64 + 1e-9).ln(),
            );
            let den = a - 2.0 * b + c;
            let delta = if den.abs() > 1e-12 {
                (0.5 * (a - c) / den).clamp(-0.5, 0.5)
            } else {
                0.0
            };
            let f = (k as f64 + delta) * bin_hz;
            let p = 69.0 + 12.0 * (f / 440.0).log2();
            let ang = std::f64::consts::TAU * (p - p.round());
            let w = m as f64;
            re += w * ang.cos();
            im += w * ang.sin();
        }
    });
    if re == 0.0 && im == 0.0 {
        return 0.0;
    }
    im.atan2(re) / std::f64::consts::TAU * 100.0
}

/// 対数周波数(1/3 半音刻み、88 鍵)のスペクトルを、刻みごとに白色化して返す。(フレームごとの値, 刻みの秒)
fn log_spectrogram(frames: &[f32], sr: f32, tuning_cents: f64) -> (Vec<Vec<f32>>, f64) {
    let (n, hop) = frame_size(sr);
    let bin_hz = sr as f64 / n as f64;
    // 対数の区切り b の中心の音高: LOW + b / BPS(b = 0 が A0 の中心)
    let hz = |pitch: f64| 440.0 * 2f64.powf((pitch - 69.0 + tuning_cents / 100.0) / 12.0);
    let ranges: Vec<(f64, f64, f64)> = (0..LOG_BINS)
        .map(|b| {
            let p = LOW + b as f64 / BPS as f64;
            let half = 0.5 / BPS as f64;
            (hz(p - half) / bin_hz, hz(p) / bin_hz, hz(p + half) / bin_hz)
        })
        .collect();
    let mut out = Vec::new();
    for_each_spectrum(frames, sr, |_, mag| {
        let mut v = vec![0.0f32; LOG_BINS];
        for (o, &(lo, mid, hi)) in v.iter_mut().zip(&ranges) {
            let (a, b) = (lo.ceil() as usize, hi.floor() as usize);
            *o = if b >= a && b < mag.len() {
                // 区切りの中にビンがある(高い方): 最大
                mag[a..=b].iter().cloned().fold(0.0, f32::max)
            } else {
                // 区切りが細かい(低い方): 線形補間
                let k = mid.floor() as usize;
                if k + 1 >= mag.len() {
                    0.0
                } else {
                    let t = (mid - k as f64) as f32;
                    mag[k] * (1.0 - t) + mag[k + 1] * t
                }
            };
        }
        out.push(whiten(&v));
    });
    (out, hop as f64 / sr as f64)
}

/// 1 オクターブの移動平均を引き、移動標準偏差で割る(負は 0)。弱い音・強い倍音の差をならす
fn whiten(v: &[f32]) -> Vec<f32> {
    let half = 12 * BPS / 2;
    let n = v.len();
    let mut out = vec![0.0f32; n];
    for i in 0..n {
        let (a, b) = (i.saturating_sub(half), (i + half + 1).min(n));
        let w = &v[a..b];
        let mean = w.iter().sum::<f32>() / w.len() as f32;
        let var = w.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / w.len() as f32;
        let sd = var.sqrt();
        out[i] = if sd > 1e-9 {
            ((v[i] - mean) / sd).max(0.0)
        } else {
            0.0
        };
    }
    out
}

/// 「音の形」の辞書(列 = 88 鍵、行 = 対数周波数)。h 倍音の強さ s^(h-1)。列は長さ 1 にそろえる
fn dictionary() -> Vec<Vec<f32>> {
    const S: f64 = 0.7;
    const HARMONICS: usize = 20;
    (0..NOTES)
        .map(|note| {
            let mut col = vec![0.0f32; LOG_BINS];
            for h in 1..=HARMONICS {
                let x = (note as f64 + 12.0 * (h as f64).log2()) * BPS as f64;
                let k = x.floor() as usize;
                if k + 1 >= LOG_BINS {
                    break;
                }
                let t = x - k as f64;
                let amp = S.powi(h as i32 - 1);
                col[k] += (amp * (1.0 - t)) as f32;
                col[k + 1] += (amp * t) as f32;
            }
            let norm = col.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
            col.iter_mut().for_each(|x| *x /= norm);
            col
        })
        .collect()
}

/// 非負最小二乗(乗法更新)。辞書 `a`(列ごと)で `y` を説明する非負の重み
fn nnls(a: &[Vec<f32>], gram: &[Vec<f32>], y: &[f32]) -> Vec<f32> {
    let aty: Vec<f32> = a
        .iter()
        .map(|col| col.iter().zip(y).map(|(c, v)| c * v).sum())
        .collect();
    let mut x: Vec<f32> = aty.iter().map(|v| v.max(0.0)).collect();
    if x.iter().all(|v| *v <= 0.0) {
        return x;
    }
    let n = x.len();
    let mut gx = vec![0.0f32; n];
    for _ in 0..100 {
        for i in 0..n {
            gx[i] = gram[i].iter().zip(&x).map(|(g, v)| g * v).sum();
        }
        for i in 0..n {
            x[i] *= aty[i].max(0.0) / (gx[i] + 1e-9);
        }
    }
    x
}

/// 区間ごとの 88 鍵の強さ。`bounds` は区間の (開始, 終了) 秒。区間の中のスペクトルを平均してから分解する
pub fn note_activations(
    frames: &[f32],
    sr: f32,
    tuning_cents: f64,
    bounds: &[(f64, f64)],
) -> Vec<Vec<f32>> {
    let (spec, hop) = log_spectrogram(frames, sr, tuning_cents);
    let (n, _) = frame_size(sr);
    let center = n as f64 / 2.0 / sr as f64;
    let dict = dictionary();
    let gram: Vec<Vec<f32>> = dict
        .iter()
        .map(|a| {
            dict.iter()
                .map(|b| a.iter().zip(b).map(|(x, y)| x * y).sum())
                .collect()
        })
        .collect();
    bounds
        .iter()
        .map(|&(s, e)| {
            let mut avg = vec![0.0f32; LOG_BINS];
            let mut cnt = 0;
            for (i, row) in spec.iter().enumerate() {
                let t = i as f64 * hop + center;
                if t >= s && t < e {
                    for (a, v) in avg.iter_mut().zip(row) {
                        *a += v;
                    }
                    cnt += 1;
                }
            }
            if cnt == 0 {
                // 区間が刻みより短い: いちばん近いフレーム
                let i = (((s + e) / 2.0 - center) / hop).round().max(0.0) as usize;
                if let Some(row) = spec.get(i.min(spec.len().saturating_sub(1))) {
                    avg.copy_from_slice(row);
                    cnt = 1;
                }
            }
            if cnt > 1 {
                avg.iter_mut().for_each(|v| *v /= cnt as f32);
            }
            nnls(&dict, &gram, &avg)
        })
        .collect()
}

/// 高音側・低音側のクロマ(音名ごとの強さ。和が 1、音が無ければ 0)
pub fn chromas(notes: &[f32]) -> ([f32; 12], [f32; 12]) {
    let mut treble = [0.0f32; 12];
    let mut bass = [0.0f32; 12];
    for (i, &a) in notes.iter().enumerate() {
        let m = LOW + i as f64;
        // 高音側: 48〜84 を中心に、36・96 へ向けて弱める。低音側: 28〜52 を中心に、24・60 へ
        let wt = ramp(m, 36.0, 48.0, 84.0, 96.0);
        let wb = ramp(m, 24.0, 28.0, 52.0, 60.0);
        let pc = (m as usize) % 12;
        treble[pc] += a * wt as f32;
        bass[pc] += a * wb as f32;
    }
    for c in [&mut treble, &mut bass] {
        let s: f32 = c.iter().sum();
        if s > 1e-9 {
            c.iter_mut().for_each(|v| *v /= s);
        }
    }
    (treble, bass)
}

fn ramp(x: f64, a: f64, b: f64, c: f64, d: f64) -> f64 {
    if x <= a || x >= d {
        0.0
    } else if x < b {
        (x - a) / (b - a)
    } else if x <= c {
        1.0
    } else {
        (d - x) / (d - c)
    }
}

/// 和音の種類(記号の後ろ, 根音からの半音, 事前の重み〈対数〉)
const QUALITIES: &[(&str, &[u8], f32)] = &[
    ("", &[0, 4, 7], 0.0),
    ("m", &[0, 3, 7], 0.0),
    ("7", &[0, 4, 7, 10], -0.15),
    ("maj7", &[0, 4, 7, 11], -0.15),
    ("m7", &[0, 3, 7, 10], -0.15),
    ("dim", &[0, 3, 6], -0.4),
    ("aug", &[0, 4, 8], -0.6),
    ("sus4", &[0, 5, 7], -0.4),
];

/// 和音の認識の設定
#[derive(Clone, Debug)]
pub struct ChordOptions {
    /// 四和音(7・maj7・m7)を候補に入れる
    pub sevenths: bool,
    /// 転回形(最低音が根音でない)を候補に入れる
    pub inversions: bool,
    /// 点数の鋭さ(大きいほど各拍の当てはまりを重く見る)
    pub sharpness: f32,
    /// 低音側の重み
    pub bass_weight: f32,
    /// 拍の位置ごとの、和音が変わる確率(小節頭, 小節の中ほど, ほか)
    pub change: [f32; 3],
}

impl Default for ChordOptions {
    fn default() -> Self {
        ChordOptions {
            sevenths: true,
            inversions: true,
            sharpness: 8.0,
            bass_weight: 2.0,
            change: [0.5, 0.25, 0.04],
        }
    }
}

/// 候補の和音
struct Candidate {
    symbol: String,
    /// 構成音(ピッチクラス)の印
    tones: [bool; 12],
    bass: usize,
    prior: f32,
}

fn candidates(opts: &ChordOptions) -> Vec<Candidate> {
    let mut out = Vec::new();
    for root in 0..12usize {
        for &(suffix, ivs, prior) in QUALITIES {
            if !opts.sevenths && ivs.len() > 3 {
                continue;
            }
            let mut tones = [false; 12];
            for &iv in ivs {
                tones[(root + iv as usize) % 12] = true;
            }
            let basses: Vec<usize> =
                if opts.inversions && !suffix.starts_with("sus") && suffix != "aug" {
                    ivs.iter().map(|&iv| (root + iv as usize) % 12).collect()
                } else {
                    vec![root]
                };
            for (i, &b) in basses.iter().enumerate() {
                let mut symbol = format!("{}{suffix}", NAMES[root]);
                if b != root {
                    symbol = format!("{symbol}/{}", NAMES[b]);
                }
                out.push(Candidate {
                    symbol,
                    tones,
                    bass: b,
                    prior: prior - if i == 0 { 0.0 } else { 0.5 },
                });
            }
        }
    }
    out
}

/// 拍ごとの区間の和音を求める。`beat_pos` は区間の頭が小節の何拍目か(0 始まり。分からなければ None)、
/// `beats_per_bar` は拍子。`notes` は区間ごとの 88 鍵の強さ([`note_activations`])
pub fn recognize_chords(
    notes: &[Vec<f32>],
    bounds: &[(f64, f64)],
    beat_pos: &[Option<u32>],
    beats_per_bar: u32,
    opts: &ChordOptions,
) -> Vec<ChordSpan> {
    let cands = candidates(opts);
    let n = notes.len().min(bounds.len());
    if n == 0 {
        return Vec::new();
    }
    // 区間の音の量(和音なしの判定)
    let energy: Vec<f32> = notes.iter().map(|v| v.iter().sum()).collect();
    let loud = energy.iter().cloned().fold(0.0f32, f32::max);
    let n_states = cands.len() + 1; // 最後が「和音なし」
    let emissions: Vec<Vec<f32>> = (0..n)
        .map(|i| {
            let (treble, bass) = chromas(&notes[i]);
            let quiet = energy[i] < loud * 0.02;
            let tnorm = treble.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
            let mut e: Vec<f32> = cands
                .iter()
                .map(|c| {
                    let k = c.tones.iter().filter(|t| **t).count() as f32;
                    let dot: f32 = (0..12).filter(|&p| c.tones[p]).map(|p| treble[p]).sum();
                    let cos = dot / (tnorm * k.sqrt());
                    opts.sharpness * cos + opts.bass_weight * (bass[c.bass] + 0.02).ln() + c.prior
                })
                .collect();
            let best = e.iter().cloned().fold(f32::MIN, f32::max);
            e.push(if quiet { best + 5.0 } else { best - 6.0 });
            e
        })
        .collect();
    // ビタビ(和音が変わる確率は拍の位置で決める。変わる先は一様)
    let change_p = |i: usize| -> f32 {
        match beat_pos.get(i).copied().flatten() {
            Some(0) => opts.change[0],
            Some(p) if beats_per_bar >= 2 && p == beats_per_bar / 2 => opts.change[1],
            Some(_) => opts.change[2],
            None => opts.change[1],
        }
    };
    let mut cur: Vec<f32> = emissions[0].clone();
    let mut back: Vec<Vec<u16>> = Vec::with_capacity(n);
    back.push(vec![0; n_states]);
    for (i, em) in emissions.iter().enumerate().skip(1) {
        let p = change_p(i).clamp(1e-4, 0.999);
        let stay = (1.0 - p).ln();
        let switch = (p / (n_states - 1) as f32).ln();
        let (best_s, best_v) = cur
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(s, v)| (s, *v))
            .unwrap_or((0, 0.0));
        let mut next = vec![0.0f32; n_states];
        let mut bp = vec![0u16; n_states];
        for s in 0..n_states {
            let keep = cur[s] + stay;
            let jump = best_v + switch;
            if keep >= jump || best_s == s {
                next[s] = keep + em[s];
                bp[s] = s as u16;
            } else {
                next[s] = jump + em[s];
                bp[s] = best_s as u16;
            }
        }
        back.push(bp);
        cur = next;
    }
    let mut s = cur
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(s, _)| s)
        .unwrap_or(0);
    let mut path = vec![0usize; n];
    for i in (0..n).rev() {
        path[i] = s;
        s = back[i][s] as usize;
    }
    // 同じ和音の続きをまとめる
    let mut out: Vec<ChordSpan> = Vec::new();
    for (i, &st) in path.iter().enumerate() {
        let symbol = if st == cands.len() {
            "N".to_owned()
        } else {
            cands[st].symbol.clone()
        };
        match out.last_mut() {
            Some(last) if last.symbol == symbol && (last.end - bounds[i].0).abs() < 1e-6 => {
                last.end = bounds[i].1;
            }
            _ => out.push(ChordSpan {
                start: bounds[i].0,
                end: bounds[i].1,
                symbol,
            }),
        }
    }
    out
}

/// 調の推定(主音, 短調か, 確からしさ)。区間ごとの 88 鍵の強さを、区間の長さで重みを付けて合わせる
pub fn estimate_key(notes: &[Vec<f32>], bounds: &[(f64, f64)]) -> Option<(u8, bool, f64)> {
    let mut hist = [0.0f64; 12];
    for (v, &(s, e)) in notes.iter().zip(bounds) {
        let (t, b) = chromas(v);
        let w = (e - s).max(0.0) * v.iter().sum::<f32>() as f64;
        for p in 0..12 {
            hist[p] += w * (t[p] as f64 + 0.5 * b[p] as f64);
        }
    }
    if hist.iter().sum::<f64>() <= 0.0 {
        return None;
    }
    let k = glaux_core::harmony::key_from_histogram(&hist);
    Some((k.tonic, k.mode == "minor", k.confidence))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 倍音を含む音(周波数のずれ `cents`)を重ねた音
    fn tones(midis: &[f64], cents: f64, secs: f64, sr: f32) -> Vec<f32> {
        let n = (secs * sr as f64) as usize;
        let mut out = vec![0.0f32; n];
        for &m in midis {
            let f = 440.0 * 2f64.powf((m - 69.0 + cents / 100.0) / 12.0);
            for h in 1..=6 {
                let a = 0.6f64.powi(h - 1) * 0.1;
                for (i, o) in out.iter_mut().enumerate() {
                    *o += (a * (std::f64::consts::TAU * f * h as f64 * i as f64 / sr as f64).sin())
                        as f32;
                }
            }
        }
        out
    }

    #[test]
    fn tuning_is_found() {
        let sr = 22_050.0;
        for c in [0.0, 25.0, -30.0] {
            let x = tones(&[57.0, 64.0, 69.0], c, 3.0, sr);
            let t = estimate_tuning(&x, sr);
            assert!((t - c).abs() < 4.0, "{c} → {t}");
        }
    }

    #[test]
    fn notes_and_chords_are_found() {
        let sr = 22_050.0;
        // C/E(E2・C4・E4・G4)→ Am(A2・A3・C4・E4)、どちらも 25 セント高い
        let mut x = tones(&[40.0, 60.0, 64.0, 67.0], 25.0, 2.0, sr);
        x.extend(tones(&[45.0, 57.0, 60.0, 64.0], 25.0, 2.0, sr));
        let tuning = estimate_tuning(&x, sr);
        let bounds: Vec<(f64, f64)> = (0..8)
            .map(|i| (i as f64 * 0.5, i as f64 * 0.5 + 0.5))
            .collect();
        let notes = note_activations(&x, sr, tuning, &bounds);
        // 最初の区間でいちばん強い高音側の音は C・E・G のどれか
        let (treble, bass) = chromas(&notes[1]);
        let top = (0..12)
            .max_by(|&a, &b| treble[a].total_cmp(&treble[b]))
            .unwrap();
        assert!([0, 4, 7].contains(&top), "{treble:?}");
        assert_eq!(
            (0..12)
                .max_by(|&a, &b| bass[a].total_cmp(&bass[b]))
                .unwrap(),
            4,
            "{bass:?}"
        );
        let pos: Vec<Option<u32>> = (0..8).map(|i| Some(i % 4)).collect();
        let chords = recognize_chords(&notes, &bounds, &pos, 4, &ChordOptions::default());
        let syms: Vec<&str> = chords.iter().map(|c| c.symbol.as_str()).collect();
        assert_eq!(syms, ["C/E", "Am"], "{chords:?}");
        let key = estimate_key(&notes, &bounds).unwrap();
        // C 長調か A 短調
        assert!(matches!(key, (0, false, _) | (9, true, _)), "{key:?}");
    }
}
