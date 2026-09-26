//! True Peak(サンプルの間のピーク)と、配信サービスでの音量の調整の予測。
//!
//! - True Peak は ITU-R BS.1770 の附属書 2 と同じく 4 倍にオーバーサンプリングして測る
//!   (サンプル値のピークだけでは、D/A 変換やコーデックの後に出る「サンプルの間の山」を見落とす)。
//!   補間は窓付き sinc(1 位相あたり 12 タップ、Kaiser 窓)。4 倍での見落としは最悪でも約 0.7 dB
//! - 配信サービスの目標値は docs/DSP_RESEARCH.md §3.1 の表のとおり(Apple・YouTube は二次情報)

use serde::Serialize;

// 補間の半分の幅と係数は、リアルタイムのリミッタと共通(glaux_dsp::limiter)
use glaux_dsp::limiter::{true_peak_kernel as kernel, TP_HALF as HALF};

/// フレームごとの True Peak(左右の大きい方、リニア)。
/// フレーム i の値は、そのサンプルと、次のサンプルまでの間の補間点(1/4・2/4・3/4)の絶対値の最大
pub fn true_peak_frames(stereo: &[f32]) -> Vec<f32> {
    let n = stereo.len() / 2;
    let k = kernel();
    let at = |c: usize, i: isize| -> f32 {
        if i < 0 || i as usize >= n {
            0.0
        } else {
            stereo[i as usize * 2 + c]
        }
    };
    (0..n)
        .map(|i| {
            let mut peak = stereo[i * 2].abs().max(stereo[i * 2 + 1].abs());
            for c in 0..2 {
                for phase in &k {
                    let mut acc = 0.0f32;
                    for (t, coef) in phase.iter().enumerate() {
                        acc += coef * at(c, i as isize + t as isize - HALF as isize + 1);
                    }
                    peak = peak.max(acc.abs());
                }
            }
            peak
        })
        .collect()
}

/// 全体の True Peak(dBTP)
pub fn true_peak_db(stereo: &[f32]) -> f64 {
    let p = true_peak_frames(stereo).into_iter().fold(0.0f32, f32::max);
    20.0 * (p as f64).max(1e-9).log10()
}

/// PSR(ピークと短期ラウドネスの差)の最小値(dB)。短期ラウドネスは 1 秒ごと(3 秒窓)の値、
/// ピークは同じ 3 秒窓の True Peak。無音に近い所(-60 LUFS 未満)は除く。測れなければ None
pub fn psr_min_db(stereo: &[f32], sample_rate: f64, short_term_lufs: &[f64]) -> Option<f64> {
    let tp = true_peak_frames(stereo);
    let sec = sample_rate as usize;
    short_term_lufs
        .iter()
        .enumerate()
        .filter(|(_, st)| **st > -60.0)
        .filter_map(|(k, st)| {
            let end = ((k + 1) * sec).min(tp.len());
            let start = end.saturating_sub(3 * sec);
            let p = tp[start..end].iter().copied().fold(0.0f32, f32::max);
            (p > 0.0).then(|| 20.0 * (p as f64).log10() - st)
        })
        .reduce(f64::min)
}

/// 配信サービスで再生されたときの音量の調整の予測
#[derive(Clone, Debug, Serialize)]
pub struct StreamingPreview {
    pub service: &'static str,
    /// 目標(LUFS)
    pub target_lufs: f64,
    /// 掛かると見込まれるゲイン(dB。マイナスは下げられる)
    pub gain_db: f64,
    /// 調整後の統合ラウドネス(LUFS)
    pub result_lufs: f64,
    /// 補足(持ち上げない・ピークで制限される・二次情報など)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'static str>,
}

/// 統合ラウドネスと True Peak から、主な配信サービスでの調整を予測する。
/// - Spotify(通常): -14。大きい曲は下げ、小さい曲は True Peak が -1 dBTP を超えない所まで上げる
/// - Spotify(Loud): -11。上げるときは -1 dB のリミッタで抑える
/// - Apple Music: -16(二次情報)。持ち上げ方は未確認なので Spotify と同じとみなす
/// - YouTube: -14。下げるだけで上げない(二次情報)
/// - AES77(曲単位): -16。推奨値との差だけを示す
pub fn streaming_previews(integrated_lufs: f64, true_peak_dbtp: f64) -> Vec<StreamingPreview> {
    if !integrated_lufs.is_finite() {
        return vec![];
    }
    let round = |v: f64| (v * 10.0).round() / 10.0;
    let make = |service, target: f64, gain: f64, note| StreamingPreview {
        service,
        target_lufs: target,
        gain_db: round(gain),
        result_lufs: round(integrated_lufs + gain),
        note,
    };
    // 上げるときは True Peak が -1 dBTP を超えない所まで
    let headroom = (-1.0 - true_peak_dbtp).max(0.0);
    let capped = |target: f64| -> (f64, Option<&'static str>) {
        let want = target - integrated_lufs;
        if want > headroom {
            (headroom, Some("ピークの余裕が足りず、目標まで上がらない"))
        } else {
            (want, None)
        }
    };
    let (sp, sp_note) = capped(-14.0);
    let (ap, ap_note) = capped(-16.0);
    vec![
        make("Spotify", -14.0, sp, sp_note),
        make(
            "Spotify(Loud)",
            -11.0,
            -11.0 - integrated_lufs,
            Some("上げるときは -1 dB のリミッタが掛かる"),
        ),
        make(
            "Apple Music",
            -16.0,
            ap,
            ap_note.or(Some("目標値は二次情報")),
        ),
        make(
            "YouTube",
            -14.0,
            (-14.0 - integrated_lufs).min(0.0),
            Some("下げるだけで上げない(二次情報)"),
        ),
        make(
            "AES77(曲単位の推奨)",
            -16.0,
            -16.0 - integrated_lufs,
            Some("推奨値との差"),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// fs/4 のサイン波を 45° ずらすと、サンプルは ±0.707 だがサンプルの間の山は 1.0
    #[test]
    fn finds_the_peak_between_samples() {
        let n = 4800;
        let stereo: Vec<f32> = (0..n)
            .flat_map(|i| {
                let v =
                    (std::f32::consts::FRAC_PI_2 * i as f32 + std::f32::consts::FRAC_PI_4).sin();
                [v, v]
            })
            .collect();
        let sample_peak = stereo.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((sample_peak - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
        let tp = true_peak_db(&stereo);
        assert!(
            tp > -0.2 && tp < 0.2,
            "サンプルの間の山(0 dBTP)を見つける: {tp:.2}"
        );
    }

    #[test]
    fn predicts_streaming_gain() {
        // 大きいマスター(-8 LUFS / -0.1 dBTP): Spotify で 6 dB 下がる、YouTube も下がる
        let p = streaming_previews(-8.0, -0.1);
        let sp = p.iter().find(|x| x.service == "Spotify").unwrap();
        assert_eq!(sp.gain_db, -6.0);
        let yt = p.iter().find(|x| x.service == "YouTube").unwrap();
        assert_eq!(yt.gain_db, -6.0);
        // 小さいマスター(-20 LUFS / -3 dBTP): Spotify はピークの余裕 2 dB までしか上げない。YouTube は上げない
        let p = streaming_previews(-20.0, -3.0);
        let sp = p.iter().find(|x| x.service == "Spotify").unwrap();
        assert_eq!(sp.gain_db, 2.0);
        assert!(sp.note.is_some());
        let yt = p.iter().find(|x| x.service == "YouTube").unwrap();
        assert_eq!(yt.gain_db, 0.0);
    }
}
