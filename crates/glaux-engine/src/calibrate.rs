//! 録音遅延の較正: メトロノームに合わせたタップ(手拍子・「タッ」という発声)を録音し、
//! 各拍に対する立ち上がりの遅れの中央値を「往復遅延」として推定する。
//!
//! スピーカーで鳴らしていればクリック音自体がマイクに回り込むので、それが最初の
//! 立ち上がりとして検出され、純粋なシステム遅延が測れる(タップ不要)。

use glaux_dsp::SampleData;

/// 較正の結果。
#[derive(Clone, Debug, serde::Serialize)]
pub struct LatencyEstimate {
    /// 推定した遅延(ms)。録音のレイテンシ補正にそのまま使う
    pub latency_ms: f64,
    /// 各拍の遅れのばらつき(中央絶対偏差、ms)。大きいと信頼できない
    pub spread_ms: f64,
    /// 立ち上がりを検出できた拍の数
    pub detected: usize,
    pub beats: usize,
}

const HOP: f64 = 0.002;

fn envelope(data: &SampleData) -> Vec<f32> {
    let sr = data.sample_rate as f64;
    let hop = (HOP * sr).round().max(1.0) as usize;
    let win = (0.004 * sr).round().max(1.0) as usize;
    let x = &data.frames;
    (0..x.len())
        .step_by(hop)
        .map(|p| {
            let e = (p + win).min(x.len());
            let rms = (x[p..e].iter().map(|v| v * v).sum::<f32>() / (e - p).max(1) as f32).sqrt();
            20.0 * rms.max(1e-9).log10()
        })
        .collect()
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[v.len() / 2]
}

/// `beats` は録音先頭からの各拍の時刻(秒)。各拍の直前 80ms 〜 次の拍の手前までで
/// 最初の立ち上がり(静かな状態から大きく立ち上がった点)を探し、遅れを集計する。
pub fn estimate_latency(data: &SampleData, beats: &[f64]) -> Result<LatencyEstimate, String> {
    let env = envelope(data);
    if env.is_empty() || beats.len() < 2 {
        return Err("録音が短すぎます".to_owned());
    }
    let mut sorted = env.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let floor = sorted[sorted.len() / 5];
    let peak = sorted[sorted.len() - 1];
    if peak - floor < 15.0 {
        return Err(
            "音が小さくて立ち上がりを検出できませんでした(手拍子か「タッ」をはっきりと)".to_owned(),
        );
    }
    // 立ち上がりのしきい値: 雑音より十分上、かつピークから 30dB 以内
    let thresh = (floor + 15.0).max(peak - 30.0);
    // 立ち上がり点: しきい値を上回り、その直前 30ms がしきい値 -6dB を下回っていた点
    let quiet = (0.03 / HOP) as usize;
    let onsets: Vec<f64> = (quiet..env.len())
        .filter(|&k| env[k] >= thresh && env[k - quiet..k].iter().all(|&v| v < thresh - 6.0))
        .map(|k| k as f64 * HOP)
        .collect();

    let beat_len = (beats[1] - beats[0]).max(0.1);
    let mut delays: Vec<f64> = beats
        .iter()
        .filter_map(|&b| {
            onsets
                .iter()
                .find(|&&o| o >= b - 0.08 && o < b + beat_len * 0.9)
                .map(|&o| o - b)
        })
        .collect();
    let detected = delays.len();
    if detected < beats.len().div_ceil(2) {
        return Err(format!(
            "立ち上がりを {detected}/{} 拍しか検出できませんでした。クリックに合わせてはっきり叩いてください",
            beats.len()
        ));
    }
    let med = median(&mut delays.clone());
    let mut dev: Vec<f64> = delays.iter_mut().map(|d| (*d - med).abs()).collect();
    let spread = median(&mut dev);
    Ok(LatencyEstimate {
        latency_ms: (med * 1000.0).clamp(0.0, 1000.0),
        spread_ms: spread * 1000.0,
        detected,
        beats: beats.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 拍ごとに `delay` 秒遅れて手拍子(短いノイズバースト)が入った録音
    fn claps(beats: &[f64], delay: &[f64], sr: f32) -> SampleData {
        let len = ((beats.last().unwrap() + 1.0) * sr as f64) as usize;
        let mut frames = vec![0.0f32; len];
        // 小さな背景雑音
        for (i, v) in frames.iter_mut().enumerate() {
            *v = ((i * 7919 % 1000) as f32 / 1000.0 - 0.5) * 0.001;
        }
        for (b, d) in beats.iter().zip(delay) {
            let s = ((b + d) * sr as f64) as usize;
            for k in 0..(0.03 * sr) as usize {
                let env = (-(k as f32) / (0.006 * sr)).exp();
                let noise = ((k * 104729 % 997) as f32 / 997.0 - 0.5) * 2.0;
                frames[s + k] += 0.5 * env * noise;
            }
        }
        SampleData {
            frames,
            sample_rate: sr,
            side: None,
            mips: Default::default(),
        }
    }

    #[test]
    fn estimates_constant_delay() {
        let beats: Vec<f64> = (0..8).map(|k| 0.5 + k as f64 * 0.5).collect();
        let delays = [0.24, 0.26, 0.25, 0.23, 0.27, 0.25, 0.24, 0.26];
        let est = estimate_latency(&claps(&beats, &delays, 48_000.0), &beats).unwrap();
        assert_eq!(est.detected, 8);
        assert!((est.latency_ms - 250.0).abs() < 12.0, "{est:?}");
        assert!(est.spread_ms < 20.0, "{est:?}");
    }

    #[test]
    fn tolerates_missed_beats() {
        let beats: Vec<f64> = (0..8).map(|k| 0.5 + k as f64 * 0.5).collect();
        let delays = [0.1; 8];
        let mut data = claps(&beats, &delays, 48_000.0);
        // 2 拍分の手拍子を消す
        for b in [1.0, 2.5] {
            let s = ((b + 0.1) * 48_000.0) as usize;
            for v in &mut data.frames[s..s + 2000] {
                *v = 0.0;
            }
        }
        let est = estimate_latency(&data, &beats).unwrap();
        assert_eq!(est.detected, 6);
        assert!((est.latency_ms - 100.0).abs() < 10.0, "{est:?}");
    }

    #[test]
    fn silence_is_an_error() {
        let beats: Vec<f64> = (0..8).map(|k| 0.5 + k as f64 * 0.5).collect();
        let data = SampleData {
            frames: vec![0.0; 48_000 * 5],
            sample_rate: 48_000.0,
            side: None,
            mips: Default::default(),
        };
        assert!(estimate_latency(&data, &beats).is_err());
    }
}
