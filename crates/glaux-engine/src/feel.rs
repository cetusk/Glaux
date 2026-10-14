//! 曲のノリの解析: 音の立ち上がりが拍の中のどこに来るか(ハネ具合・細かさ)、格子からのずれ(打ち込みか人の演奏か)、
//! 帯域ごとの音の密度。参考曲の「言葉で伝えにくい感じ」を AI が読めるようにする(曲から起こす機能の一部)。
//!
//! 立ち上がりは帯域(低・中・高)ごとのスペクトルの増え方(スペクトル流束)の山で拾う(約 5ms 刻み)。
//! 拍の時刻(拍の推定の結果)で区切り、拍の中の位置(0〜1)の分布から:
//!
//! - 8 分の裏の位置(0.5 = イーブン、0.67 = 3 連のハネ)
//! - 16 分の裏の位置(半拍の中で。0.5 = イーブン)
//! - 格子(上の位置で作った 16 分の格子)からのずれのばらつき(ms。打ち込みは数 ms、人の演奏は 10〜25ms)
//! - 細かさ(8 分まで / 16 分 / 3 連)
//!
//! を求める。

use rustfft::{num_complex::Complex, FftPlanner};
use serde::Serialize;

/// 帯域の境目(Hz)
const LOW_HI: f64 = 200.0;
const MID_HI: f64 = 2500.0;

/// 立ち上がり 1 つ
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Onset {
    pub time: f64,
    /// 強さ(流束の山の高さ。帯域の中で最大が 1)
    pub strength: f32,
    /// 0 = 低, 1 = 中, 2 = 高
    pub band: u8,
}

/// 帯域ごとの立ち上がりを拾う(モノラル音声)
pub fn onsets(frames: &[f32], sr: f32) -> Vec<Onset> {
    let n = ((sr as f64 * 0.023) as usize).next_power_of_two();
    let hop = n / 4;
    if frames.len() < n * 2 {
        return Vec::new();
    }
    let fft = FftPlanner::<f32>::new().plan_fft_forward(n);
    let hann: Vec<f32> = (0..n)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos())
        .collect();
    let bin_hz = sr as f64 / n as f64;
    let band_of = |k: usize| -> usize {
        let f = k as f64 * bin_hz;
        if f < LOW_HI {
            0
        } else if f < MID_HI {
            1
        } else {
            2
        }
    };
    let bands: Vec<usize> = (0..n / 2).map(band_of).collect();
    let mut buf = vec![Complex::new(0.0f32, 0.0); n];
    let mut prev = vec![0.0f32; n / 2];
    let mut flux: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut start = 0;
    while start + n <= frames.len() {
        for (i, c) in buf.iter_mut().enumerate() {
            *c = Complex::new(frames[start + i] * hann[i], 0.0);
        }
        fft.process(&mut buf);
        let mut f = [0.0f32; 3];
        for k in 1..n / 2 {
            let m = (1.0 + 100.0 * buf[k].norm()).ln();
            let d = m - prev[k];
            if d > 0.0 {
                f[bands[k]] += d;
            }
            prev[k] = m;
        }
        for b in 0..3 {
            flux[b].push(f[b]);
        }
        start += hop;
    }
    let hop_s = hop as f64 / sr as f64;
    // 窓の中心の時刻
    let center = n as f64 / 2.0 / sr as f64;
    let mut out = Vec::new();
    for (b, fl) in flux.iter().enumerate() {
        let max = fl.iter().cloned().fold(0.0f32, f32::max);
        if max <= 1e-6 {
            continue;
        }
        // 山: 前後 ±30ms で最大、かつ前後 ±150ms の平均 + 最大の 7% を超える
        let w = (0.03 / hop_s).round().max(1.0) as usize;
        let wa = (0.15 / hop_s).round() as usize;
        let mut last = f64::NEG_INFINITY;
        for i in 1..fl.len() {
            let v = fl[i];
            let (a, z) = (i.saturating_sub(w), (i + w + 1).min(fl.len()));
            if fl[a..z].iter().any(|x| *x > v) {
                continue;
            }
            let (a2, z2) = (i.saturating_sub(wa), (i + wa + 1).min(fl.len()));
            let mean = fl[a2..z2].iter().sum::<f32>() / (z2 - a2) as f32;
            if v < mean + 0.07 * max {
                continue;
            }
            // 山の位置を放物線で細かく
            let (y0, y1, y2) = (fl[i - 1], v, *fl.get(i + 1).unwrap_or(&v));
            let den = y0 - 2.0 * y1 + y2;
            let d = if den.abs() > 1e-9 {
                (0.5 * (y0 - y2) / den).clamp(-0.5, 0.5)
            } else {
                0.0
            } as f64;
            // 流束は窓の中で音が増えた所 → 窓の中心より少し前が立ち上がり(窓の 1/4 を引く)
            let t = (i as f64 + d) * hop_s + center - n as f64 / 4.0 / sr as f64;
            if t - last < 0.03 {
                continue;
            }
            last = t;
            out.push(Onset {
                time: t.max(0.0),
                strength: v / max,
                band: b as u8,
            });
        }
    }
    out.sort_by(|a, b| a.time.total_cmp(&b.time));
    out
}

/// ノリの解析結果
#[derive(Clone, Debug, Serialize)]
pub struct Groove {
    /// 8 分の裏の位置(拍の中の割合、%)。50 = イーブン、58〜62 = 軽いハネ、67 = 3 連のハネ。測れなければ null
    pub swing_8th_pct: Option<f64>,
    /// 16 分の裏の位置(半拍の中の割合、%)。50 = イーブン
    pub swing_16th_pct: Option<f64>,
    /// 格子からの立ち上がりのずれのばらつき(ms。頑健な標準偏差)。8 未満 = 打ち込みのように揃っている
    pub timing_spread_ms: Option<f64>,
    /// 細かさ: "8分" / "16分" / "3連"
    pub subdivision: &'static str,
    /// 1 拍あたりの立ち上がりの数(低・中・高の帯域)
    pub onsets_per_beat: [f64; 3],
    /// 言葉での要約
    pub summary: String,
}

/// 拍の中の位置の分布から、山の位置を探す(`lo..hi`、重み付きのヒストグラムを 0.01 刻み・幅 0.02 でならす)
fn peak_in(phases: &[(f64, f32)], lo: f64, hi: f64) -> Option<(f64, f64)> {
    const BINS: usize = 100;
    let mut h = [0.0f64; BINS];
    for &(p, w) in phases {
        let i = ((p * BINS as f64) as usize).min(BINS - 1);
        h[i] += w as f64;
    }
    let total: f64 = h.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let smooth: Vec<f64> = (0..BINS)
        .map(|i| {
            (-2i64..=2)
                .map(|d| {
                    let j = (i as i64 + d).rem_euclid(BINS as i64) as usize;
                    h[j] * (1.0 - d.abs() as f64 / 3.0)
                })
                .sum()
        })
        .collect();
    let (a, b) = (
        (lo * BINS as f64) as usize,
        ((hi * BINS as f64) as usize).min(BINS),
    );
    let (i, v) = (a..b)
        .map(|i| (i, smooth[i]))
        .max_by(|x, y| x.1.total_cmp(&y.1))?;
    // 山の重さ(全体に対する割合)
    let mass: f64 = (i.saturating_sub(3)..(i + 4).min(BINS))
        .map(|j| h[j])
        .sum::<f64>()
        / total;
    // 隣のビンとの重心で細かく
    let (l, r) = (smooth[i.saturating_sub(1)], smooth[(i + 1).min(BINS - 1)]);
    let den = l - 2.0 * v + r;
    let d = if den.abs() > 1e-12 {
        (0.5 * (l - r) / den).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    Some(((i as f64 + 0.5 + d) / BINS as f64, mass))
}

/// ノリを解析する。`beats` は拍の時刻(秒、昇順)
pub fn analyze(onsets: &[Onset], beats: &[f64]) -> Option<Groove> {
    if beats.len() < 8 {
        return None;
    }
    // 拍の推定は 20ms 刻みなので、前後 4 拍の直線で滑らかにしてから測る(刻みの誤差を揺れと取り違えない)
    let smooth: Vec<f64> = (0..beats.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(4), (i + 5).min(beats.len()));
            let n = (b - a) as f64;
            let mx = (a..b).map(|k| k as f64).sum::<f64>() / n;
            let my = beats[a..b].iter().sum::<f64>() / n;
            let (mut sxy, mut sxx) = (0.0, 0.0);
            for (k, t) in beats.iter().enumerate().take(b).skip(a) {
                sxy += (k as f64 - mx) * (t - my);
                sxx += (k as f64 - mx).powi(2);
            }
            if sxx > 0.0 {
                my + sxy / sxx * (i as f64 - mx)
            } else {
                beats[i]
            }
        })
        .collect();
    let beats = &smooth[..];
    // 拍の中の位置(0〜1)と、拍の長さ
    let mut raw: Vec<(f64, f64, f32)> = Vec::new(); // (位置, 拍の長さ, 強さ)
    let mut counts = [0usize; 3];
    let mut i = 0;
    for o in onsets {
        while i + 1 < beats.len() && beats[i + 1] <= o.time {
            i += 1;
        }
        if i + 1 >= beats.len() || o.time < beats[i] {
            continue;
        }
        let len = beats[i + 1] - beats[i];
        raw.push(((o.time - beats[i]) / len, len, o.strength));
        counts[o.band as usize] += 1;
    }
    // 拍の推定と立ち上がりの検出の、決まったずれを除く: 拍の頭の近く(±12%)の立ち上がりの位置の中央値を 0 にする
    let mut near: Vec<f64> = raw
        .iter()
        .map(|x| if x.0 > 0.5 { x.0 - 1.0 } else { x.0 })
        .filter(|p| p.abs() <= 0.12)
        .collect();
    near.sort_by(f64::total_cmp);
    let p0 = near.get(near.len() / 2).copied().unwrap_or(0.0);
    let located: Vec<(f64, f64, f32)> = raw
        .iter()
        .map(|&(p, len, w)| {
            let q = p - p0;
            // 次の拍の頭の直前に来た音は、次の拍の頭(位置 0)として数える
            let q = if q > 0.97 { q - 1.0 } else { q };
            (q, len, w)
        })
        .collect();
    let phases: Vec<(f64, f32)> = located.iter().map(|x| (x.0.rem_euclid(1.0), x.2)).collect();
    let n_beats = (beats.len() - 1) as f64;
    let total_w: f32 = phases.iter().map(|x| x.1).sum();
    if total_w <= 0.0 {
        return None;
    }
    // 8 分の裏: 0.4〜0.75 の山(重さが 5% 以上あるとき)
    let p8 = peak_in(&phases, 0.4, 0.75).filter(|(_, m)| *m >= 0.05);
    let s8 = p8.map(|(p, _)| p);
    // 16 分の裏: 拍の頭から 8 分の裏までの間の山
    let half = s8.unwrap_or(0.5);
    let p16 = peak_in(&phases, 0.12, half - 0.08).filter(|(_, m)| *m >= 0.04);
    let s16 = p16.map(|(p, _)| p / half);
    // 3 連: 1/3 の位置に山があり、8 分の裏が 2/3 付近
    let p3 = peak_in(&phases, 0.28, 0.4).filter(|(_, m)| *m >= 0.05);
    let triplet = p3.is_some_and(|(p, _)| (p - 1.0 / 3.0).abs() < 0.04)
        && s8.is_some_and(|p| (p - 2.0 / 3.0).abs() < 0.05);
    let subdivision = if triplet {
        "3連"
    } else if s16.is_some() {
        "16分"
    } else {
        "8分"
    };
    // 格子: 0、16 分の裏、8 分の裏、その後ろの 16 分
    let mut grid = vec![0.0, 1.0];
    if let Some(p) = s8 {
        grid.push(p);
        if let Some(q) = s16 {
            grid.push(q * p);
            grid.push(p + q * (1.0 - p));
        }
    }
    if triplet {
        grid.push(1.0 / 3.0);
    }
    let mut dev: Vec<f64> = located
        .iter()
        .filter(|x| x.2 >= 0.15)
        .map(|&(p, len, _)| {
            grid.iter()
                .map(|g| (p - g) * len * 1000.0)
                .min_by(|a, b| a.abs().total_cmp(&b.abs()))
                .unwrap_or(0.0)
        })
        .collect();
    let spread = if dev.len() >= 16 {
        dev.sort_by(f64::total_cmp);
        let med = dev[dev.len() / 2];
        let mut ad: Vec<f64> = dev.iter().map(|d| (d - med).abs()).collect();
        ad.sort_by(f64::total_cmp);
        Some(ad[ad.len() / 2] * 1.4826)
    } else {
        None
    };
    let r1 = |v: f64| (v * 10.0).round() / 10.0;
    let per_beat = counts.map(|c| r1(c as f64 / n_beats));
    let mut words = Vec::new();
    match s8 {
        Some(p) if p >= 0.63 => words.push("8 分が 3 連寄りに大きくハネる(シャッフル)".to_owned()),
        Some(p) if p >= 0.56 => {
            words.push(format!("8 分が軽くハネる(スウィング {:.0}%)", p * 100.0))
        }
        Some(_) => words.push("8 分はイーブン".to_owned()),
        None => {}
    }
    if let Some(q) = s16 {
        if q >= 0.56 {
            words.push(format!("16 分がハネる(スウィング {:.0}%)", q * 100.0));
        } else {
            words.push("16 分のノリ".to_owned());
        }
    }
    match spread {
        Some(s) if s < 8.0 => words.push("音の頭が格子にぴったり揃う(打ち込み的)".to_owned()),
        Some(s) if s < 15.0 => words.push("音の頭が少し揺れる(人の演奏らしい)".to_owned()),
        Some(_) => words.push("音の頭が大きく揺れる(ゆったりした演奏・ルバート)".to_owned()),
        None => {}
    }
    Some(Groove {
        swing_8th_pct: s8.map(|p| r1(p * 100.0)),
        swing_16th_pct: s16.map(|q| r1(q * 100.0)),
        timing_spread_ms: spread.map(r1),
        subdivision,
        onsets_per_beat: per_beat,
        summary: words.join("。"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 拍と、8 分の裏を `swing` の位置に置いたクリック(高い帯域)
    fn clicks(bpm: f64, swing: f64, secs: f64, sr: f32, jitter_ms: f64) -> (Vec<f32>, Vec<f64>) {
        let n = (secs * sr as f64) as usize;
        let mut x = vec![0.0f32; n];
        let ibi = 60.0 / bpm;
        let mut beats = Vec::new();
        let mut k = 0usize;
        let mut seed = 1u64;
        let mut rnd = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 33) as f64 / (1u64 << 31) as f64) * 2.0 - 1.0
        };
        loop {
            let t = 0.5 + k as f64 * ibi;
            if t + ibi > secs {
                break;
            }
            beats.push(t);
            for (pos, amp) in [(0.0, 1.0f32), (swing, 0.7)] {
                let at = t + pos * ibi + rnd() * jitter_ms / 1000.0;
                let s = (at * sr as f64) as usize;
                for i in 0..(0.03 * sr as f64) as usize {
                    if s + i < n {
                        let env = (-(i as f32) / (0.004 * sr)).exp();
                        // 高い音のノイズ風(決まった列)
                        let v = ((i as f32 * 2.7).sin() * (i as f32 * 1.3).cos()) * env * amp;
                        x[s + i] += v;
                    }
                }
            }
            k += 1;
        }
        (x, beats)
    }

    #[test]
    fn swing_and_tightness() {
        let sr = 44_100.0;
        for swing in [0.5, 0.6, 0.667] {
            let (x, beats) = clicks(110.0, swing, 20.0, sr, 0.0);
            let on = onsets(&x, sr);
            let g = analyze(&on, &beats).expect("ノリ");
            let s = g.swing_8th_pct.expect("8 分の裏") / 100.0;
            assert!((s - swing).abs() < 0.02, "{swing} → {s}");
            assert!(g.timing_spread_ms.unwrap_or(99.0) < 8.0, "{g:?}");
        }
        // 人の揺れ(±20ms)
        let (x, beats) = clicks(100.0, 0.5, 20.0, sr, 20.0);
        let g = analyze(&onsets(&x, sr), &beats).expect("ノリ");
        assert!(g.timing_spread_ms.unwrap_or(0.0) > 8.0, "{g:?}");
    }
}
