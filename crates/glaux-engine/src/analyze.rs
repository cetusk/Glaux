//! 音声解析 — AI の「耳」。
//!
//! LLM は生音声を扱えないので、オフラインレンダした結果を数値に要約して返す。
//! MCP の `analyze_audio` ツールの実体。返す指標は「AI がミックス判断に使えるか」で選ぶ:
//!
//! - `loudness_lufs`: ITU-R BS.1770 の K 特性 + ゲーティングによる統合ラウドネス
//! - `peak_db` / `rms_db` / `crest_factor_db`: 音量とダイナミクス
//! - `spectral_centroid_hz`: 明るさの重心
//! - `band_energy`: low(<250Hz) / mid / high(>4kHz) のエネルギー比率
//! - `onsets_ticks`: 発音タイミング(tick)。リズムの確認用
//! - `clipped`: クリップ(振り切れ)の有無
//!
//! 解析は常に 48kHz でレンダする(K 特性フィルタ係数が 48kHz 定義のため)。

use crate::export::{render_project, ExportError};
use glaux_core::{Project, Tick, TrackId};
use rustfft::{num_complex::Complex, FftPlanner};
use serde::Serialize;

const SAMPLE_RATE: f64 = 48_000.0;

#[derive(Clone, Debug, Serialize)]
pub struct BandEnergy {
    /// < 250 Hz(キック・ベースの帯域)
    pub low: f64,
    /// 250 Hz 〜 4 kHz(メロディ・コードの主戦場)
    pub mid: f64,
    /// > 4 kHz(ハット・空気感)
    pub high: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Analysis {
    pub duration_seconds: f64,
    /// 統合ラウドネス(LUFS)。ストリーミング配信の目安は -14 前後
    pub loudness_lufs: f64,
    pub peak_db: f64,
    pub rms_db: f64,
    /// ピークと RMS の差。小さいと潰れ気味、大きいとダイナミック
    pub crest_factor_db: f64,
    pub spectral_centroid_hz: f64,
    pub band_energy: BandEnergy,
    /// 発音位置(絶対 tick、最大 200 個)
    pub onsets_ticks: Vec<u64>,
    pub onset_count: usize,
    /// サンプルが振り切れている(歪んでいる可能性)
    pub clipped: bool,
    /// ラウドネスレンジ(LU。EBU R128 の LRA)。曲中の音量の起伏の大きさ(小さい = 平板)
    pub loudness_range_lu: f64,
    /// True Peak(dBTP。サンプル間のピークも含む。配信の目安は -1 以下)
    pub true_peak_dbtp: f64,
    /// 短期ラウドネス(3 秒窓)の 1 秒ごとの推移(LUFS、最大 180 点)。展開・盛り上がりの把握用
    pub short_term_lufs: Vec<f64>,
    /// 短期ラウドネスの最大値(いちばん大きい所)
    pub max_short_term_lufs: f64,
    /// PLR(True Peak − 統合ラウドネス、dB)。小さいほどピークが潰れている(目安: 8 を大きく下回ると潰しすぎ)
    pub plr_db: f64,
    /// PSR(3 秒窓の True Peak − 短期ラウドネス)の最小値(dB)。いちばん詰まった所の余裕。
    /// 実務の目安は 8 を下回らない(Ian Shepherd。規格ではない)。測れなければ null
    pub psr_min_db: Option<f64>,
    /// 配信サービスで再生されたときの音量の調整の予測(Spotify・Apple Music・YouTube・AES77)
    pub streaming: Vec<crate::loudness::StreamingPreview>,
    /// ステレオの広がり
    pub stereo: StereoInfo,
    /// 音色の釣り合い(1/3 オクターブの長時間平均と、その傾き・出っ張り)
    pub tonal_balance: TonalBalance,
}

/// 音色の釣り合い。
#[derive(Clone, Debug, Serialize)]
pub struct TonalBalance {
    /// 1/3 オクターブ帯域ごとの量(全体に対する dB)。[中心 Hz, dB] の並び
    pub third_octave: Vec<(u32, f64)>,
    /// 50Hz〜10kHz の傾き(dB/oct、帯域ごとのエネルギーで)。0 = ピンクノイズと同じ(どのオクターブも同じ量)、
    /// マイナスほど高域が少ない(暗い)、プラスほど明るい
    pub slope_db_per_oct: f64,
    /// 傾きの直線から ±3dB 以上ずれた帯域(40Hz〜16kHz)。プラス = 出っ張り(こもり・刺さりの候補)、マイナス = へこみ
    pub deviations: Vec<(u32, f64)>,
}

/// 帯域ごとのステレオの広がり。
#[derive(Clone, Debug, Serialize)]
pub struct StereoBand {
    /// "low"(250Hz 未満)/ "mid"(250Hz〜4kHz)/ "high"(4kHz 以上)
    pub band: &'static str,
    pub correlation: f64,
    pub side_to_mid_db: f64,
}

/// ステレオの広がり・位相。
#[derive(Clone, Debug, Serialize)]
pub struct StereoInfo {
    /// 左右の相関(1 = モノラル、0 = 無相関で広い、負 = 逆相でモノラル再生時に打ち消し合う)
    pub correlation: f64,
    /// 250Hz 以下の左右の相関(低域は 1 に近い = 中央に集まっているのが望ましい)
    pub low_correlation: f64,
    /// サイド(左右の差)とミッド(和)のエネルギー比(dB。-∞ に近い = モノラル、0 付近 = 非常に広い)
    pub side_to_mid_db: f64,
    /// 左右の音量差(dB。正 = 右が大きい)
    pub balance_db: f64,
    /// 帯域ごとの相関と広がり(低い帯域ほど中央に集まり、高い帯域ほど広いのがふつう)
    pub bands: Vec<StereoBand>,
    /// 音が鳴っている 100ms の区間のうち、相関がマイナス(逆相)だった割合
    pub negative_correlation_ratio: f64,
    /// モノラルにしたときの統合ラウドネスの変化(dB)。左右が同じなら 0、無相関で約 -3、
    /// それより大きく下がるなら逆相の成分がモノラルで消えている
    pub mono_loudness_change_db: f64,
}

/// トラック間の周波数のかぶり(マスキング)。
#[derive(Clone, Debug, Serialize)]
pub struct MaskingIssue {
    /// かぶって聞こえにくくなっている側
    pub track: String,
    /// かぶせている側
    pub masked_by: String,
    /// 帯域(聴覚の帯域幅に近い区切り、Hz)
    pub band_hz: (u32, u32),
    /// `track` がこの帯域で聞こえるはずの時間のうち、`masked_by` のマスキングのしきい値より下にいる
    /// (覆われて聞こえない)時間の割合
    pub time_ratio: f64,
    /// この帯域が `track` のエネルギーに占める割合(大きいほど、その音の主要な帯域)
    pub band_share: f64,
    /// 覆われている時間に、しきい値より何 dB 下にいるか(中央値)。`masked_by` をこの帯域でこれだけ下げる
    /// (か `track` を上げる)と聞こえてくる目安
    pub suggest_cut_db: f64,
}

/// トラックごとの要約と、トラック間のかぶり。
#[derive(Clone, Debug, Serialize)]
pub struct MixAnalysis {
    pub tracks: Vec<TrackAnalysis>,
    /// 深刻な順(最大 12 件)
    pub masking: Vec<MaskingIssue>,
}

/// 1 トラックぶんの要約(ミックスバランスの比較用)。
#[derive(Clone, Debug, Serialize)]
pub struct TrackAnalysis {
    pub track_id: String,
    pub name: String,
    pub loudness_lufs: f64,
    pub rms_db: f64,
    pub peak_db: f64,
    pub spectral_centroid_hz: f64,
    pub band_energy: BandEnergy,
}

/// 各トラックをソロでレンダして要約を返す(音が出ないトラックは省く)。
/// 「リードが埋もれている」のようなトラック間の相対バランスを判断する材料。
/// かぶり(マスキング)も欲しければ [`analyze_mix`]。
pub fn analyze_project_tracks(
    project: &Project,
    range: Option<(Tick, Tick)>,
    bank: &crate::data::SampleBank,
) -> Vec<TrackAnalysis> {
    analyze_mix(project, range, bank).tracks
}

/// プロジェクトを解析する。`track_ids` で対象トラックを、`range` で tick 範囲を絞れる。
pub fn analyze_project(
    project: &Project,
    track_ids: Option<&[TrackId]>,
    range: Option<(Tick, Tick)>,
    bank: &crate::data::SampleBank,
) -> Result<Analysis, ExportError> {
    // 対象トラックだけ残したコピーを作ってレンダする
    let mut target = project.clone();
    if let Some(ids) = track_ids {
        target.tracks.retain(|t| ids.contains(&t.id));
        // solo が対象外トラックに付いていた場合の影響を避ける
        for t in &mut target.tracks {
            t.solo = false;
        }
    }
    let stereo = render_for_analysis(&target, range, bank)?;
    analyze_stereo(project, range, &stereo)
}

/// レンダ済みのステレオ(48kHz、インターリーブ)を解析する
fn analyze_stereo(
    project: &Project,
    range: Option<(Tick, Tick)>,
    stereo: &[f32],
) -> Result<Analysis, ExportError> {
    let offset_seconds = range.map_or(0.0, |(start, _)| project.tempo_map.tick_to_seconds(start));
    let sliced: &[f32] = stereo;
    if sliced.len() < 4096 {
        return Err(ExportError::Empty);
    }

    let frames = sliced.len() / 2;
    let mono: Vec<f32> = sliced
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| (c[0] + c[1]) * 0.5)
        .collect();

    // ---- レベル系 ----
    let peak = sliced.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let mean_sq = sliced.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / sliced.len() as f64;
    let peak_db = amp_db(peak as f64);
    let rms_db = 10.0 * mean_sq.max(1e-12).log10();
    let clipped = peak >= 0.999;

    let loudness_lufs = integrated_lufs(sliced);
    let r128 = r128_stats(sliced);
    let mut stereo = stereo_info(sliced);
    if loudness_lufs.is_finite() {
        let mono_dup: Vec<f32> = sliced
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|c| {
                let m = (c[0] + c[1]) * 0.5;
                [m, m]
            })
            .collect();
        let mono_lufs = integrated_lufs(&mono_dup);
        stereo.mono_loudness_change_db = if mono_lufs.is_finite() {
            ((mono_lufs - loudness_lufs) * 10.0).round() / 10.0
        } else {
            -70.0
        };
    }
    let tonal_balance = tonal_balance(&mono);

    // ---- スペクトル系(Welch 平均) ----
    let (spectral_centroid_hz, band_energy) = spectrum_stats(&mono);

    // ---- オンセット ----
    let onsets_ticks = detect_onsets(&mono, offset_seconds, &project.tempo_map);
    let onset_count = onsets_ticks.len();

    Ok(Analysis {
        duration_seconds: frames as f64 / SAMPLE_RATE,
        loudness_lufs,
        peak_db,
        rms_db,
        crest_factor_db: peak_db - rms_db,
        spectral_centroid_hz,
        band_energy,
        onsets_ticks: onsets_ticks.into_iter().take(200).collect(),
        onset_count,
        clipped,
        loudness_range_lu: r128.0,
        true_peak_dbtp: r128.1,
        short_term_lufs: r128.2,
        max_short_term_lufs: r128.3,
        plr_db: if loudness_lufs.is_finite() {
            ((r128.1 - loudness_lufs) * 10.0).round() / 10.0
        } else {
            f64::NAN
        },
        psr_min_db: r128.4.map(|v| (v * 10.0).round() / 10.0),
        streaming: crate::loudness::streaming_previews(loudness_lufs, r128.1),
        stereo,
        tonal_balance,
    })
}

/// オクターブ帯域ごとの音の量(全体に対する dB)。音量に左右されない「音色の釣り合い」
#[derive(Clone, Debug, Serialize)]
pub struct OctaveLevel {
    /// 中心周波数(Hz)
    pub hz: u32,
    pub before_db: f64,
    pub after_db: f64,
    /// after − before(dB)。プラスはその帯域が相対的に増えた
    pub diff_db: f64,
}

/// 比べるときの要約(前・後それぞれ)
#[derive(Clone, Debug, Serialize)]
pub struct CompareSide {
    pub loudness_lufs: f64,
    pub true_peak_dbtp: f64,
    pub plr_db: f64,
    pub psr_min_db: Option<f64>,
    pub loudness_range_lu: f64,
    pub crest_factor_db: f64,
    pub spectral_centroid_hz: f64,
    pub band_energy: BandEnergy,
    pub stereo: StereoInfo,
}

impl From<&Analysis> for CompareSide {
    fn from(a: &Analysis) -> Self {
        CompareSide {
            loudness_lufs: a.loudness_lufs,
            true_peak_dbtp: a.true_peak_dbtp,
            plr_db: a.plr_db,
            psr_min_db: a.psr_min_db,
            loudness_range_lu: a.loudness_range_lu,
            crest_factor_db: a.crest_factor_db,
            spectral_centroid_hz: a.spectral_centroid_hz,
            band_energy: a.band_energy.clone(),
            stereo: a.stereo.clone(),
        }
    }
}

/// 編集の前後の比較。音量の差と、音量をそろえたうえでの違いを分けて返す
#[derive(Clone, Debug, Serialize)]
pub struct Comparison {
    pub before: CompareSide,
    pub after: CompareSide,
    /// 統合ラウドネスの差(after − before、dB)。大きい方が良く聞こえる錯覚の元
    pub loudness_diff_db: f64,
    /// after にこれを掛けると before と同じ音量になる(dB)。聴き比べるときの補正量
    pub match_gain_db: f64,
    /// オクターブ帯域ごとの釣り合い(それぞれの全体に対する dB。音量差の影響を受けない)
    pub tonal_balance: Vec<OctaveLevel>,
    /// 目立つ違いの要約(日本語)
    pub notes: Vec<String>,
}

/// 2 つのプロジェクト(編集の前と後)を同じ条件でレンダして比べる。
/// 音量が違うと大きい方が良く聞こえるので、音色・広がり・ダイナミクスは音量に左右されない指標で比べる
pub fn compare_projects(
    before: &Project,
    after: &Project,
    track_ids: Option<&[TrackId]>,
    range: Option<(Tick, Tick)>,
    bank_before: &crate::data::SampleBank,
    bank_after: &crate::data::SampleBank,
) -> Result<Comparison, ExportError> {
    let render = |p: &Project, bank| -> Result<(Analysis, Vec<f64>), ExportError> {
        let mut target = p.clone();
        if let Some(ids) = track_ids {
            target.tracks.retain(|t| ids.contains(&t.id));
            for t in &mut target.tracks {
                t.solo = false;
            }
        }
        let stereo = render_for_analysis(&target, range, bank)?;
        let a = analyze_stereo(p, range, &stereo)?;
        let mono: Vec<f32> = stereo
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| (c[0] + c[1]) * 0.5)
            .collect();
        Ok((a, octave_levels(&mono)))
    };
    let (a0, o0) = render(before, bank_before)?;
    let (a1, o1) = render(after, bank_after)?;
    let r1 = |v: f64| (v * 10.0).round() / 10.0;
    let diff = if a0.loudness_lufs.is_finite() && a1.loudness_lufs.is_finite() {
        r1(a1.loudness_lufs - a0.loudness_lufs)
    } else {
        0.0
    };
    let tonal: Vec<OctaveLevel> = OCTAVES
        .iter()
        .zip(o0.iter().zip(&o1))
        .map(|(hz, (b, a))| OctaveLevel {
            hz: *hz,
            before_db: r1(*b),
            after_db: r1(*a),
            diff_db: r1(a - b),
        })
        .collect();
    let notes = compare_notes(&a0, &a1, diff, &tonal);
    Ok(Comparison {
        before: (&a0).into(),
        after: (&a1).into(),
        loudness_diff_db: diff,
        match_gain_db: -diff,
        tonal_balance: tonal,
        notes,
    })
}

/// 目立つ違いを文章にする(しきい値は「聞いて分かる」程度の目安)
fn compare_notes(a0: &Analysis, a1: &Analysis, diff: f64, tonal: &[OctaveLevel]) -> Vec<String> {
    let mut n = Vec::new();
    if diff.abs() >= 0.5 {
        n.push(format!(
            "後の方が {:.1} dB {}。音量の差は良し悪しの判断を狂わせる(大きい方が良く聞こえる)ので、\
             以下の音色・広がり・ダイナミクスの違いで判断する",
            diff.abs(),
            if diff > 0.0 { "大きい" } else { "小さい" }
        ));
    }
    let bands: Vec<String> = tonal
        .iter()
        .filter(|o| o.diff_db.abs() >= 1.5 && o.before_db.max(o.after_db) > -40.0)
        .map(|o| format!("{}Hz 帯 {:+.1} dB", o.hz, o.diff_db))
        .collect();
    if !bands.is_empty() {
        n.push(format!(
            "音色の釣り合い(音量をそろえた比較): {}",
            bands.join("、")
        ));
    }
    if a0.plr_db.is_finite() && a1.plr_db.is_finite() && a1.plr_db - a0.plr_db <= -1.5 {
        n.push(format!(
            "ピークと平均の差(PLR)が {:.1} → {:.1} dB に縮んだ(潰れた・迫力が減った可能性)",
            a0.plr_db, a1.plr_db
        ));
    }
    if let (Some(p0), Some(p1)) = (a0.psr_min_db, a1.psr_min_db) {
        if p1 < 8.0 && p1 < p0 - 1.0 {
            n.push(format!(
                "いちばん詰まった所の PSR が {p0:.1} → {p1:.1} dB(8 を下回ると潰しすぎの目安)"
            ));
        }
    }
    if a1.true_peak_dbtp > -1.0 && a1.true_peak_dbtp > a0.true_peak_dbtp + 0.3 {
        n.push(format!(
            "True Peak が {:.1} dBTP に上がった(配信は -1 以下が目安)",
            a1.true_peak_dbtp
        ));
    }
    let (s0, s1) = (&a0.stereo, &a1.stereo);
    if s1.correlation < s0.correlation - 0.15 {
        n.push(format!(
            "左右の相関が {:.2} → {:.2} に下がった(広がった。0 を下回るとモノラルで消える)",
            s0.correlation, s1.correlation
        ));
    } else if s1.correlation > s0.correlation + 0.15 {
        n.push(format!(
            "左右の相関が {:.2} → {:.2} に上がった(狭くなった)",
            s0.correlation, s1.correlation
        ));
    }
    if s1.low_correlation < 0.7 && s1.low_correlation < s0.low_correlation - 0.1 {
        n.push(format!(
            "低域(250Hz 以下)の相関が {:.2} に下がった(低音は中央に集めるのが無難)",
            s1.low_correlation
        ));
    }
    if (s1.balance_db - s0.balance_db).abs() >= 1.0 {
        n.push(format!(
            "左右の偏りが {:+.1} → {:+.1} dB(正 = 右)",
            s0.balance_db, s1.balance_db
        ));
    }
    if a0.loudness_range_lu > 0.0 && a1.loudness_range_lu < a0.loudness_range_lu - 2.0 {
        n.push(format!(
            "曲中の音量の起伏(LRA)が {:.1} → {:.1} LU に減った(平板になった可能性)",
            a0.loudness_range_lu, a1.loudness_range_lu
        ));
    }
    if n.is_empty() {
        n.push("聞いて分かるほどの違いは測れなかった".to_owned());
    }
    n
}

/// オクターブ帯域の中心周波数
const OCTAVES: [u32; 10] = [31, 63, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];

/// オクターブ帯域ごとの音の量(全体に対する dB)。Welch 平均のパワースペクトルから
fn octave_levels(mono: &[f32]) -> Vec<f64> {
    const N: usize = 8192;
    const HOP: usize = 4096;
    let mut planner = FftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(N);
    let hann: Vec<f64> = (0..N)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / N as f64).cos())
        .collect();
    let mut power = vec![0.0f64; N / 2];
    let mut buf = vec![Complex::new(0.0, 0.0); N];
    let mut pos = 0;
    while pos + N <= mono.len() {
        for i in 0..N {
            buf[i] = Complex::new(mono[pos + i] as f64 * hann[i], 0.0);
        }
        fft.process(&mut buf);
        for (i, p) in power.iter_mut().enumerate() {
            *p += buf[i].norm_sqr();
        }
        pos += HOP;
    }
    let bin_hz = SAMPLE_RATE / N as f64;
    let total: f64 = power.iter().sum::<f64>().max(1e-20);
    OCTAVES
        .iter()
        .map(|&c| {
            let (lo, hi) = (
                c as f64 / std::f64::consts::SQRT_2,
                c as f64 * std::f64::consts::SQRT_2,
            );
            let e: f64 = power
                .iter()
                .enumerate()
                .filter(|(i, _)| {
                    let f = *i as f64 * bin_hz;
                    f >= lo && f < hi
                })
                .map(|(_, p)| p)
                .sum();
            (10.0 * (e / total).max(1e-10).log10()).max(-100.0)
        })
        .collect()
}

/// EBU R128: (LRA, True Peak dBTP, 短期ラウドネスの 1 秒ごとの推移, その最大, PSR の最小)。
fn r128_stats(stereo: &[f32]) -> (f64, f64, Vec<f64>, f64, Option<f64>) {
    use ebur128::{EbuR128, Mode};
    let Ok(mut m) = EbuR128::new(2, SAMPLE_RATE as u32, Mode::LRA | Mode::TRUE_PEAK | Mode::S)
    else {
        return (0.0, -120.0, vec![], -70.0, None);
    };
    let sec = SAMPLE_RATE as usize * 2;
    let mut timeline = Vec::new();
    for chunk in stereo.chunks(sec) {
        if m.add_frames_f32(chunk).is_err() {
            break;
        }
        let st = m.loudness_shortterm().unwrap_or(f64::NEG_INFINITY);
        timeline.push(if st.is_finite() {
            (st * 10.0).round() / 10.0
        } else {
            -70.0
        });
    }
    let lra = m.loudness_range().unwrap_or(0.0);
    let tp = (0..2)
        .filter_map(|c| m.true_peak(c).ok())
        .fold(0.0f64, f64::max);
    let max_st = timeline.iter().copied().fold(-70.0f64, f64::max);
    let psr = crate::loudness::psr_min_db(stereo, SAMPLE_RATE, &timeline);
    // 長い曲は間引いて 180 点以内に
    let step = timeline.len().div_ceil(180).max(1);
    let timeline: Vec<f64> = timeline.iter().step_by(step).copied().collect();
    (lra, amp_db(tp), timeline, max_st, psr)
}

/// RBJ の 2 次ローパス(48kHz)。
fn lowpass(fc: f64) -> Biquad {
    let w0 = std::f64::consts::TAU * fc / SAMPLE_RATE;
    let alpha = w0.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
    let cos = w0.cos();
    let a0 = 1.0 + alpha;
    Biquad::new(
        [
            (1.0 - cos) / 2.0 / a0,
            (1.0 - cos) / a0,
            (1.0 - cos) / 2.0 / a0,
        ],
        [-2.0 * cos / a0, (1.0 - alpha) / a0],
    )
}

/// RBJ の 2 次ハイパス(48kHz)。
fn highpass(fc: f64) -> Biquad {
    let w0 = std::f64::consts::TAU * fc / SAMPLE_RATE;
    let alpha = w0.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
    let cos = w0.cos();
    let a0 = 1.0 + alpha;
    Biquad::new(
        [
            (1.0 + cos) / 2.0 / a0,
            -(1.0 + cos) / a0,
            (1.0 + cos) / 2.0 / a0,
        ],
        [-2.0 * cos / a0, (1.0 - alpha) / a0],
    )
}

/// 1/3 オクターブの中心(Hz)
const THIRD_OCTAVES: [u32; 30] = [
    25, 31, 40, 50, 63, 80, 100, 125, 160, 200, 250, 315, 400, 500, 630, 800, 1000, 1250, 1600,
    2000, 2500, 3150, 4000, 5000, 6300, 8000, 10000, 12500, 16000, 20000,
];

/// 音色の釣り合い(1/3 オクターブの長時間平均、傾き、直線からのずれ)
pub(crate) fn tonal_balance(mono: &[f32]) -> TonalBalance {
    const N: usize = 8192;
    const HOP: usize = 4096;
    let mut planner = FftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(N);
    let hann: Vec<f64> = (0..N)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / N as f64).cos())
        .collect();
    let mut power = vec![0.0f64; N / 2];
    let mut buf = vec![Complex::new(0.0, 0.0); N];
    let mut pos = 0;
    while pos + N <= mono.len() {
        for i in 0..N {
            buf[i] = Complex::new(mono[pos + i] as f64 * hann[i], 0.0);
        }
        fft.process(&mut buf);
        for (i, p) in power.iter_mut().enumerate() {
            *p += buf[i].norm_sqr();
        }
        pos += HOP;
    }
    let bin_hz = SAMPLE_RATE / N as f64;
    let total: f64 = power.iter().sum::<f64>().max(1e-20);
    let edge = 2f64.powf(1.0 / 6.0);
    let r1 = |v: f64| (v * 10.0).round() / 10.0;
    let levels: Vec<(u32, f64)> = THIRD_OCTAVES
        .iter()
        .map(|&c| {
            let (lo, hi) = (c as f64 / edge, c as f64 * edge);
            let e: f64 = power
                .iter()
                .enumerate()
                .filter(|(i, _)| {
                    let f = *i as f64 * bin_hz;
                    f >= lo && f < hi
                })
                .map(|(_, p)| p)
                .sum();
            (c, (10.0 * (e / total).max(1e-10).log10()).max(-100.0))
        })
        .collect();
    // 50Hz〜10kHz で dB = a + b·log2(f) を最小 2 乗で当てはめる
    let pts: Vec<(f64, f64)> = levels
        .iter()
        .filter(|(c, _)| (50..=10_000).contains(c))
        .map(|(c, db)| ((*c as f64).log2(), *db))
        .collect();
    let n = pts.len() as f64;
    let (sx, sy) = pts.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
    let (mx, my) = (sx / n, sy / n);
    let sxx: f64 = pts.iter().map(|p| (p.0 - mx).powi(2)).sum();
    let sxy: f64 = pts.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
    let slope = if sxx > 0.0 { sxy / sxx } else { 0.0 };
    let line = |c: u32| my + slope * ((c as f64).log2() - mx);
    let deviations = levels
        .iter()
        .filter(|(c, db)| (40..=16_000).contains(c) && *db > -80.0)
        .map(|(c, db)| (*c, r1(db - line(*c))))
        .filter(|(_, d)| d.abs() >= 3.0)
        .collect();
    TonalBalance {
        third_octave: levels.into_iter().map(|(c, db)| (c, r1(db))).collect(),
        slope_db_per_oct: (slope * 100.0).round() / 100.0,
        deviations,
    }
}

/// 100ms ごとの相関がマイナスだった区間の割合(ほぼ無音の区間は数えない)
fn negative_correlation_ratio(stereo: &[f32]) -> f64 {
    let win = (0.1 * SAMPLE_RATE) as usize;
    let (mut neg, mut all) = (0usize, 0usize);
    for w in stereo.chunks(win * 2) {
        let (mut ll, mut rr, mut lr) = (0.0f64, 0.0f64, 0.0f64);
        for c in w.as_chunks::<2>().0 {
            let (l, r) = (c[0] as f64, c[1] as f64);
            ll += l * l;
            rr += r * r;
            lr += l * r;
        }
        let frames = (w.len() / 2).max(1) as f64;
        // -50dBFS 程度より小さい区間は数えない
        if (ll + rr) / frames < 1e-5 {
            continue;
        }
        all += 1;
        if lr < 0.0 {
            neg += 1;
        }
    }
    if all == 0 {
        0.0
    } else {
        ((neg as f64 / all as f64) * 1000.0).round() / 1000.0
    }
}

pub(crate) fn stereo_info(stereo: &[f32]) -> StereoInfo {
    let (mut ll, mut rr, mut lr) = (0.0f64, 0.0f64, 0.0f64);
    let (mut lo_ll, mut lo_rr, mut lo_lr) = (0.0f64, 0.0f64, 0.0f64);
    let (mut mid, mut side) = (0.0f64, 0.0f64);
    let (mut fl, mut fr) = (lowpass(250.0), lowpass(250.0));
    // 帯域ごと: 低 = 250Hz 未満、中 = 250Hz〜4kHz、高 = 4kHz 以上([ll, rr, lr, mid, side])
    let mut band_acc = [[0.0f64; 5]; 3];
    let mut split = [
        [
            lowpass(250.0),
            highpass(250.0),
            lowpass(4000.0),
            highpass(4000.0),
        ],
        [
            lowpass(250.0),
            highpass(250.0),
            lowpass(4000.0),
            highpass(4000.0),
        ],
    ];
    for c in stereo.as_chunks::<2>().0 {
        let (l, r) = (c[0] as f64, c[1] as f64);
        ll += l * l;
        rr += r * r;
        lr += l * r;
        mid += (l + r).powi(2);
        side += (l - r).powi(2);
        let (a, b) = (fl.next(l), fr.next(r));
        lo_ll += a * a;
        lo_rr += b * b;
        lo_lr += a * b;
        let mut bands_of = |ch: usize, x: f64| {
            let s = &mut split[ch];
            let low = s[0].next(x);
            let rest = s[1].next(x);
            [low, s[2].next(rest), s[3].next(rest)]
        };
        let (bl, br) = (bands_of(0, l), bands_of(1, r));
        for b in 0..3 {
            let acc = &mut band_acc[b];
            acc[0] += bl[b] * bl[b];
            acc[1] += br[b] * br[b];
            acc[2] += bl[b] * br[b];
            acc[3] += (bl[b] + br[b]).powi(2);
            acc[4] += (bl[b] - br[b]).powi(2);
        }
    }
    let corr = |xy: f64, xx: f64, yy: f64| {
        let d = (xx * yy).sqrt();
        if d > 1e-12 {
            (xy / d).clamp(-1.0, 1.0)
        } else {
            1.0
        }
    };
    let r3 = |v: f64| (v * 1000.0).round() / 1000.0;
    StereoInfo {
        correlation: r3(corr(lr, ll, rr)),
        low_correlation: r3(corr(lo_lr, lo_ll, lo_rr)),
        side_to_mid_db: (10.0 * (side.max(1e-12) / mid.max(1e-12)).log10()).max(-60.0),
        balance_db: 10.0 * (rr.max(1e-12) / ll.max(1e-12)).log10(),
        bands: ["low", "mid", "high"]
            .iter()
            .zip(band_acc)
            .map(|(name, a)| StereoBand {
                band: name,
                correlation: r3(corr(a[2], a[0], a[1])),
                side_to_mid_db: ((10.0 * (a[4].max(1e-12) / a[3].max(1e-12)).log10()).max(-60.0)
                    * 10.0)
                    .round()
                    / 10.0,
            })
            .collect(),
        negative_correlation_ratio: negative_correlation_ratio(stereo),
        mono_loudness_change_db: 0.0,
    }
}

/// 聴覚の帯域に近い区切り(Zwicker の臨界帯域の境界、Hz)
const BARK_EDGES: [f64; 25] = [
    20.0, 100.0, 200.0, 300.0, 400.0, 510.0, 630.0, 770.0, 920.0, 1080.0, 1270.0, 1480.0, 1720.0,
    2000.0, 2320.0, 2700.0, 3150.0, 3700.0, 4400.0, 5300.0, 6400.0, 7700.0, 9500.0, 12000.0,
    15500.0,
];

/// 100ms ごとの臨界帯域の音の量(dBFS。フルスケールのサイン波が 0dB)と、帯域ごとの音のらしさ(0 = 雑音、1 = 純音)
struct BandFrame {
    db: [f64; 24],
    tonality: [f64; 24],
}

/// モノラルを 100ms ごとの臨界帯域の [`BandFrame`] の列にする。
fn band_frames(mono: &[f32]) -> Vec<BandFrame> {
    const N: usize = 4096;
    let hop = (0.1 * SAMPLE_RATE) as usize;
    let mut planner = FftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(N);
    let hann: Vec<f64> = (0..N)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / N as f64).cos())
        .collect();
    let bin_hz = SAMPLE_RATE / N as f64;
    // Hann 窓の山の高さ(N/4)の 2 乗 × 等価雑音帯域(1.5 ビン)で割ると、サイン波が 0dB
    let norm = (N as f64 / 4.0).powi(2) * 1.5;
    let band_of: Vec<Option<usize>> = (0..N / 2)
        .map(|k| {
            let f = k as f64 * bin_hz;
            BARK_EDGES.windows(2).position(|w| f >= w[0] && f < w[1])
        })
        .collect();
    let mut out = Vec::new();
    let mut buf = vec![Complex::new(0.0, 0.0); N];
    let mut pos = 0;
    while pos + N <= mono.len() {
        for i in 0..N {
            buf[i] = Complex::new(mono[pos + i] as f64 * hann[i], 0.0);
        }
        fft.process(&mut buf);
        let mut power = [0.0f64; 24];
        // 帯域の中で、隣り合う 3 ビンの和のいちばん大きいもの(純音なら窓の山がほぼ全部入る)
        let mut peak3 = [0.0f64; 24];
        for (k, c) in buf[..N / 2].iter().enumerate() {
            if let Some(b) = band_of[k] {
                power[b] += c.norm_sqr();
                if k >= 1 && k + 1 < N / 2 {
                    let three = buf[k - 1].norm_sqr() + c.norm_sqr() + buf[k + 1].norm_sqr();
                    peak3[b] = peak3[b].max(three);
                }
            }
        }
        // 音のらしさ: 3 ビンの山が帯域の音のほとんどを占めれば純音(1)、雑音なら 3 / ビン数 程度(0)。
        // 臨界帯域のビン数は 7〜数百と少ないので、スペクトルの平坦さ(SFM)より安定する
        let tonality: [f64; 24] = std::array::from_fn(|b| {
            if power[b] > 1e-20 {
                ((peak3[b] / power[b] - 0.5) / 0.45).clamp(0.0, 1.0)
            } else {
                0.0
            }
        });
        out.push(BandFrame {
            db: power.map(|p| (10.0 * (p / norm).max(1e-14).log10()).max(-140.0)),
            tonality,
        });
        pos += hop;
    }
    out
}

/// 臨界帯域 k の中心(Bark)
fn bark_center(k: usize) -> f64 {
    k as f64 + 0.5
}

/// 絶対閾(Terhardt、dB SPL)を dBFS に(フルスケールのサイン波 = 96dB SPL として)
fn hearing_threshold_dbfs(k: usize) -> f64 {
    let f = ((BARK_EDGES[k] * BARK_EDGES[k + 1]).sqrt() / 1000.0).max(0.02);
    let spl = 3.64 * f.powf(-0.8) - 6.5 * (-0.6 * (f - 3.3).powi(2)).exp() + 1e-3 * f.powi(4);
    spl - 96.0
}

/// 広がり関数(Schroeder、dB)。`dz` = 聞く側の帯域 − 覆う側の帯域(Bark)
fn spreading_db(dz: f64) -> f64 {
    15.81 + 7.5 * (dz + 0.474) - 17.5 * (1.0 + (dz + 0.474).powi(2)).sqrt()
}

/// 前向きのマスキングが 100ms ごとに弱まる量(dB)
const FORWARD_DECAY_DB: f64 = 15.0;

/// あるトラックが作るマスキングのしきい値(dBFS、100ms ごと × 臨界帯域)。
/// 帯域ごとの音を広がり関数で周りの帯域へ広げ、音のらしさで決まる分だけ下げる
/// (純音は覆う力が弱い: 14.5 + z dB、雑音は強い: 5.5 dB。Johnston)。
/// 大きな音の直後もしばらく覆う(前向きのマスキング)
fn masking_threshold(frames: &[BandFrame]) -> Vec<[f64; 24]> {
    let mut out: Vec<[f64; 24]> = Vec::with_capacity(frames.len());
    for f in frames {
        let mut t = [-140.0f64; 24];
        for (k, tk) in t.iter_mut().enumerate() {
            let mut sum = 0.0f64;
            for j in 0..24 {
                if f.db[j] <= -130.0 {
                    continue;
                }
                let a = f.tonality[j];
                let offset = a * (14.5 + bark_center(j)) + (1.0 - a) * 5.5;
                let level = f.db[j] + spreading_db(bark_center(k) - bark_center(j)) - offset;
                sum += 10f64.powf(level / 10.0);
            }
            *tk = 10.0 * sum.max(1e-14).log10();
        }
        if let Some(prev) = out.last() {
            for k in 0..24 {
                t[k] = t[k].max(prev[k] - FORWARD_DECAY_DB);
            }
        }
        out.push(t);
    }
    out
}

/// トラックごとにソロでレンダし、要約とトラック間のかぶり(マスキング)を求める。
///
/// かぶりの判定(心理音響モデル): 臨界帯域ごとに、あるトラックが聞こえるはずの
/// (絶対閾より上で、その帯域での自分の最大から -30dB 以内の)時間のうち、別のトラックの
/// マスキングのしきい値([`masking_threshold`])より下にいる時間の割合。
/// その帯域が自分のエネルギーの 8% 以上を占めるものだけを数える。
pub fn analyze_mix(
    project: &Project,
    range: Option<(Tick, Tick)>,
    bank: &crate::data::SampleBank,
) -> MixAnalysis {
    // トラックごとのソロのレンダは互いに独立なので並列にする(CLAP を含むものは、プラグインの
    // インスタンスを作るので 1 つずつ)。再生スレッドと競合しないよう、コア数より少し少なくする
    let solo = |t: &glaux_core::Track| {
        let mut target = project.clone();
        target.tracks.retain(|x| x.id == t.id);
        for x in &mut target.tracks {
            x.solo = false;
        }
        target
    };
    let rendered: Vec<Option<Vec<f32>>> = {
        let n = project.tracks.len();
        let targets: Vec<Project> = project.tracks.iter().map(solo).collect();
        let has_plugins: Vec<bool> = targets
            .iter()
            .map(|t| !crate::plugins::project_plugins(t).is_empty())
            .collect();
        let mut out: Vec<Option<Vec<f32>>> = vec![None; n];
        let workers = std::thread::available_parallelism()
            .map_or(1, |p| p.get())
            .saturating_sub(2)
            .max(1);
        let next = std::sync::atomic::AtomicUsize::new(0);
        let results = std::sync::Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for _ in 0..workers.min(n) {
                scope.spawn(|| loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    if has_plugins[i] {
                        continue;
                    }
                    let r = render_for_analysis(&targets[i], range, bank).ok();
                    results
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push((i, r));
                });
            }
        });
        for (i, r) in results.into_inner().unwrap_or_else(|e| e.into_inner()) {
            out[i] = r;
        }
        for i in (0..n).filter(|&i| has_plugins[i]) {
            out[i] = render_for_analysis(&targets[i], range, bank).ok();
        }
        out
    };

    let mut tracks = Vec::new();
    let mut frames: Vec<(String, Vec<BandFrame>)> = Vec::new();
    for (t, stereo) in project.tracks.iter().zip(rendered) {
        let Some(stereo) = stereo else {
            continue;
        };
        let sliced: &[f32] = &stereo;
        if sliced.len() < 8192 {
            continue;
        }
        let mono: Vec<f32> = sliced
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| (c[0] + c[1]) * 0.5)
            .collect();
        let peak = sliced.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let mean_sq = sliced.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / sliced.len() as f64;
        let (centroid, band_energy) = spectrum_stats(&mono);
        tracks.push(TrackAnalysis {
            track_id: t.id.to_string(),
            name: t.name.clone(),
            loudness_lufs: integrated_lufs(sliced),
            rms_db: 10.0 * mean_sq.max(1e-12).log10(),
            peak_db: amp_db(peak as f64),
            spectral_centroid_hz: centroid,
            band_energy,
        });
        frames.push((t.name.clone(), band_frames(&mono)));
    }
    let thresholds: Vec<Vec<[f64; 24]>> =
        frames.iter().map(|(_, f)| masking_threshold(f)).collect();
    let ath: [f64; 24] = std::array::from_fn(hearing_threshold_dbfs);
    let mut masking = Vec::new();
    for (bi, (b_name, b)) in frames.iter().enumerate() {
        // 帯域ごとのエネルギーの割合と、帯域ごとの最大
        let mut share = [0.0f64; 24];
        let mut maxes = [-140.0f64; 24];
        for f in b {
            for k in 0..24 {
                share[k] += 10f64.powf(f.db[k] / 10.0);
                maxes[k] = maxes[k].max(f.db[k]);
            }
        }
        let total: f64 = share.iter().sum::<f64>().max(1e-14);
        for (ai, (a_name, _)) in frames.iter().enumerate() {
            if ai == bi {
                continue;
            }
            let thr = &thresholds[ai];
            for k in 0..24 {
                let s = share[k] / total;
                if s < 0.08 {
                    continue;
                }
                let n = b.len().min(thr.len());
                let mut active = 0;
                let mut margins = Vec::new();
                for t in 0..n {
                    let level = b[t].db[k];
                    if level > maxes[k] - 30.0 && level > ath[k] {
                        active += 1;
                        if level < thr[t][k] {
                            margins.push(thr[t][k] - level);
                        }
                    }
                }
                let masked = margins.len();
                if active >= 5 && masked as f64 / active as f64 >= 0.3 {
                    margins.sort_by(f64::total_cmp);
                    let median = margins[margins.len() / 2];
                    masking.push(MaskingIssue {
                        track: b_name.clone(),
                        masked_by: a_name.clone(),
                        band_hz: (BARK_EDGES[k] as u32, BARK_EDGES[k + 1] as u32),
                        time_ratio: (masked as f64 / active as f64 * 100.0).round() / 100.0,
                        band_share: (s * 100.0).round() / 100.0,
                        suggest_cut_db: ((median + 1.0).min(24.0) * 10.0).round() / 10.0,
                    });
                }
            }
        }
    }
    masking.sort_by(|x, y| (y.time_ratio * y.band_share).total_cmp(&(x.time_ratio * x.band_share)));
    masking.truncate(12);
    MixAnalysis { tracks, masking }
}

/// 解析用に描き出す。範囲の指定があれば、その範囲だけ(曲全体を描き出してから切るより速い)
fn render_for_analysis(
    project: &Project,
    range: Option<(Tick, Tick)>,
    bank: &crate::data::SampleBank,
) -> Result<Vec<f32>, ExportError> {
    match range {
        Some((start, end)) => {
            let s0 = project.tempo_map.tick_to_seconds(start);
            let s1 = project.tempo_map.tick_to_seconds(end);
            crate::export::render_project_range(project, SAMPLE_RATE, bank, s0, s1)
        }
        None => render_project(project, SAMPLE_RATE, bank),
    }
}

fn amp_db(a: f64) -> f64 {
    20.0 * a.max(1e-6).log10()
}

/// 双二次フィルタ(Direct Form 1)。K 特性用。
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    x: [f64; 2],
    y: [f64; 2],
}

impl Biquad {
    fn new(b: [f64; 3], a: [f64; 2]) -> Self {
        Biquad {
            b,
            a,
            x: [0.0; 2],
            y: [0.0; 2],
        }
    }
    fn next(&mut self, x0: f64) -> f64 {
        let y0 = self.b[0] * x0 + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[0] * self.y[0]
            - self.a[1] * self.y[1];
        self.x = [x0, self.x[0]];
        self.y = [y0, self.y[0]];
        y0
    }
}

/// ITU-R BS.1770-4 の統合ラウドネス(48kHz 固定係数)。
pub(crate) fn integrated_lufs(stereo: &[f32]) -> f64 {
    // K 特性: 高域シェルフ + ハイパス(チャンネルごと)
    let mut filters: Vec<(Biquad, Biquad)> = (0..2)
        .map(|_| {
            (
                Biquad::new(
                    [1.53512485958697, -2.69169618940638, 1.19839281085285],
                    [-1.69065929318241, 0.73248077421585],
                ),
                Biquad::new([1.0, -2.0, 1.0], [-1.99004745483398, 0.99007225036621]),
            )
        })
        .collect();

    let frames = stereo.len() / 2;
    let mut weighted = vec![0.0f64; frames]; // チャンネル合算の 2 乗値
    for i in 0..frames {
        let mut sum = 0.0;
        for (ch, (shelf, hp)) in filters.iter_mut().enumerate() {
            let v = hp.next(shelf.next(stereo[i * 2 + ch] as f64));
            sum += v * v;
        }
        weighted[i] = sum;
    }

    // 400ms ブロック、100ms ホップでゲーティング
    let block = (0.4 * SAMPLE_RATE) as usize;
    let hop = (0.1 * SAMPLE_RATE) as usize;
    if frames < block {
        let mean = weighted.iter().sum::<f64>() / frames as f64;
        return -0.691 + 10.0 * mean.max(1e-12).log10();
    }
    let block_powers: Vec<f64> = (0..=(frames - block) / hop)
        .map(|k| {
            let s = k * hop;
            weighted[s..s + block].iter().sum::<f64>() / block as f64
        })
        .collect();
    let block_lufs = |p: f64| -0.691 + 10.0 * p.max(1e-12).log10();

    // 絶対ゲート -70 LUFS
    let abs_gated: Vec<f64> = block_powers
        .iter()
        .copied()
        .filter(|&p| block_lufs(p) > -70.0)
        .collect();
    if abs_gated.is_empty() {
        return -70.0;
    }
    // 相対ゲート(平均 -10)
    let mean = abs_gated.iter().sum::<f64>() / abs_gated.len() as f64;
    let threshold = block_lufs(mean) - 10.0;
    let rel_gated: Vec<f64> = abs_gated
        .into_iter()
        .filter(|&p| block_lufs(p) > threshold)
        .collect();
    if rel_gated.is_empty() {
        return -70.0;
    }
    block_lufs(rel_gated.iter().sum::<f64>() / rel_gated.len() as f64)
}

/// Welch 平均パワースペクトルから重心と帯域比を求める。
fn spectrum_stats(mono: &[f32]) -> (f64, BandEnergy) {
    const N: usize = 4096;
    const HOP: usize = 2048;
    let mut planner = FftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(N);

    let hann: Vec<f64> = (0..N)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / N as f64).cos())
        .collect();

    let mut power = vec![0.0f64; N / 2];
    let mut windows = 0usize;
    let mut buf = vec![Complex::new(0.0, 0.0); N];
    let mut pos = 0;
    while pos + N <= mono.len() {
        for i in 0..N {
            buf[i] = Complex::new(mono[pos + i] as f64 * hann[i], 0.0);
        }
        fft.process(&mut buf);
        for (i, p) in power.iter_mut().enumerate() {
            *p += buf[i].norm_sqr();
        }
        windows += 1;
        pos += HOP;
    }
    if windows == 0 {
        return (
            0.0,
            BandEnergy {
                low: 0.0,
                mid: 0.0,
                high: 0.0,
            },
        );
    }

    let bin_hz = SAMPLE_RATE / N as f64;
    let total: f64 = power.iter().sum::<f64>().max(1e-12);
    let centroid = power
        .iter()
        .enumerate()
        .map(|(i, p)| i as f64 * bin_hz * p)
        .sum::<f64>()
        / total;

    let sum_range = |lo: f64, hi: f64| -> f64 {
        power
            .iter()
            .enumerate()
            .filter(|(i, _)| {
                let f = *i as f64 * bin_hz;
                f >= lo && f < hi
            })
            .map(|(_, p)| p)
            .sum::<f64>()
            / total
    };

    (
        centroid,
        BandEnergy {
            low: sum_range(0.0, 250.0),
            mid: sum_range(250.0, 4000.0),
            high: sum_range(4000.0, SAMPLE_RATE / 2.0),
        },
    )
}

/// エネルギー変化(フラックス)によるオンセット検出。絶対 tick で返す。
fn detect_onsets(mono: &[f32], offset_seconds: f64, tempo: &glaux_core::TempoMap) -> Vec<u64> {
    const FRAME: usize = 1024;
    const HOP: usize = 512;
    if mono.len() < FRAME * 2 {
        return vec![];
    }
    let energies: Vec<f64> = (0..(mono.len() - FRAME) / HOP)
        .map(|k| {
            let s = k * HOP;
            mono[s..s + FRAME]
                .iter()
                .map(|v| (*v as f64).powi(2))
                .sum::<f64>()
        })
        .collect();
    // 先頭に暗黙の無音フレームを置く(曲頭・範囲頭ちょうどの発音も検出できるように)
    let flux: Vec<f64> = std::iter::once(energies[0])
        .chain(energies.windows(2).map(|w| (w[1] - w[0]).max(0.0)))
        .collect();
    let mean = flux.iter().sum::<f64>() / flux.len().max(1) as f64;
    let std =
        (flux.iter().map(|f| (f - mean).powi(2)).sum::<f64>() / flux.len().max(1) as f64).sqrt();
    let threshold = mean + 1.5 * std;

    let min_gap = (0.09 * SAMPLE_RATE) as usize / HOP; // 90ms
    let mut onsets = Vec::new();
    let mut last: Option<usize> = None;
    for (k, &f) in flux.iter().enumerate() {
        if f > threshold && !last.is_some_and(|l| k - l < min_gap) {
            let seconds = offset_seconds + (k * HOP) as f64 / SAMPLE_RATE;
            onsets.push(tempo.seconds_to_tick(seconds).0);
            last = Some(k);
        }
    }
    onsets
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::{Clip, ClipContent, ClipId, Device, Note, NoteId, Track, TrackKind};

    fn midi_track(name: &str, notes: Vec<(u64, u64, u8, u8)>) -> Track {
        let mut track = Track::new(TrackId::new(), name, TrackKind::Midi);
        let mut clip = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(3840 * 2));
        if let ClipContent::Midi { notes: n, .. } = &mut clip.content {
            for (pos, dur, pitch, vel) in notes {
                n.push(Note {
                    articulation: Default::default(),
                    pitch_curve: vec![],
                    id: NoteId::new(),
                    pos: Tick(pos),
                    dur: Tick(dur),
                    pitch,
                    vel,
                    glide_ms: None,
                });
            }
        }
        track.clips.push(clip);
        track
    }

    #[test]
    fn analyzes_bass_note_as_low_heavy() {
        let mut project = Project::new("t");
        // A1 (55Hz) を 2 秒
        project
            .tracks
            .push(midi_track("Bass", vec![(0, 3840, 33, 110)]));
        let a = analyze_project(&project, None, None, &Default::default()).unwrap();
        assert!(a.duration_seconds > 1.5);
        assert!(
            a.band_energy.low > 0.5,
            "低域中心のはず: {:?}",
            a.band_energy
        );
        assert!(a.spectral_centroid_hz < 1500.0);
        assert!(a.loudness_lufs < 0.0 && a.loudness_lufs > -60.0);
        assert!(a.crest_factor_db > 0.0);
    }

    fn noise(seed: u32, n: usize) -> Vec<f32> {
        let mut r = seed.max(1);
        (0..n)
            .map(|_| {
                r ^= r << 13;
                r ^= r >> 17;
                r ^= r << 5;
                (r as f32 / u32::MAX as f32 - 0.5) * 0.4
            })
            .collect()
    }

    #[test]
    fn stereo_bands_and_mono_loudness() {
        let p = Project::new("t");
        let n = 96_000;
        let (a, b) = (noise(1, n), noise(2, n));
        let make = |f: &dyn Fn(usize) -> (f32, f32)| -> Vec<f32> {
            (0..n)
                .flat_map(|i| {
                    let (l, r) = f(i);
                    [l, r]
                })
                .collect()
        };
        // 左右が同じ: モノにしても変わらない
        let same = analyze_stereo(&p, None, &make(&|i| (a[i], a[i]))).unwrap();
        assert!(same.stereo.mono_loudness_change_db.abs() < 0.2);
        assert!(same.stereo.bands.iter().all(|b| b.correlation > 0.99));
        // 無相関: 約 -3dB
        let wide = analyze_stereo(&p, None, &make(&|i| (a[i], b[i]))).unwrap();
        let d = wide.stereo.mono_loudness_change_db;
        assert!((d + 3.0).abs() < 0.5, "{d}");
        // 逆相: 大きく下がり、ほぼ全区間が逆相
        let inv = analyze_stereo(&p, None, &make(&|i| (a[i], -a[i] * 0.9))).unwrap();
        assert!(inv.stereo.mono_loudness_change_db < -15.0);
        assert!(inv.stereo.negative_correlation_ratio > 0.95);
    }

    #[test]
    fn tonal_balance_slope_and_bumps() {
        // 白色雑音は帯域ごとのエネルギーが高域ほど増える(+3dB/oct)
        let n = 96_000;
        let a = noise(3, n);
        let t = tonal_balance(&a);
        assert!(
            (t.slope_db_per_oct - 3.0).abs() < 0.5,
            "{}",
            t.slope_db_per_oct
        );
        // 1kHz のサイン波を強く足すと、1kHz の帯が出っ張りとして出る
        let bumped: Vec<f32> = a
            .iter()
            .enumerate()
            .map(|(i, v)| {
                v * 0.2 + (i as f32 * 1000.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.1
            })
            .collect();
        let t = tonal_balance(&bumped);
        assert!(
            t.deviations.iter().any(|(hz, d)| *hz == 1000 && *d > 3.0),
            "{:?}",
            t.deviations
        );
    }

    #[test]
    fn compare_separates_loudness_from_tone() {
        // 和音を 2 秒。音量だけ上げた版と、EQ で低域を削った版を比べる
        let mut before = Project::new("t");
        before.tracks.push(midi_track(
            "Pad",
            vec![(0, 3840, 45, 100), (0, 3840, 57, 100), (0, 3840, 64, 100)],
        ));
        let mut louder = before.clone();
        louder.tracks[0].volume_db = 6.0;
        let bank = crate::data::SampleBank::default();
        let c = compare_projects(&before, &louder, None, None, &bank, &bank).unwrap();
        assert!(
            (c.loudness_diff_db - 6.0).abs() < 0.3,
            "{}",
            c.loudness_diff_db
        );
        assert!((c.match_gain_db + 6.0).abs() < 0.3);
        assert!(
            c.tonal_balance
                .iter()
                .all(|o| o.diff_db.abs() < 0.3 || o.before_db < -60.0),
            "音量だけなら釣り合いは変わらない: {:?}",
            c.tonal_balance
        );
        assert!(c.notes[0].contains("大きい"));

        let mut cut = before.clone();
        let mut eq = glaux_core::Effect::builtin(glaux_core::FxId::new(), "eq");
        eq.params.insert("low_gain_db".into(), (-12.0).into());
        cut.tracks[0].effects.push(eq);
        let c = compare_projects(&before, &cut, None, None, &bank, &bank).unwrap();
        let at = |hz: u32| c.tonal_balance.iter().find(|o| o.hz == hz).unwrap().diff_db;
        assert!(at(125) < -3.0, "低域が相対的に減る: {:?}", c.tonal_balance);
        assert!(
            at(2000) > 0.0,
            "高めの帯域は相対的に増える: {:?}",
            c.tonal_balance
        );
        assert!(c.notes.iter().any(|n| n.contains("125Hz")), "{:?}", c.notes);
    }

    #[test]
    fn hat_pattern_is_bright_with_onsets() {
        let mut project = Project::new("t");
        let mut track = midi_track("Hats", (0..8).map(|i| (i * 480, 120, 42, 100)).collect());
        track.device = Some(Device::builtin("drum"));
        project.tracks.push(track);
        let a = analyze_project(&project, None, None, &Default::default()).unwrap();
        assert!(
            a.band_energy.high > 0.3,
            "ハットは高域寄り: {:?}",
            a.band_energy
        );
        // 8 打 ± 数個(検出誤差)
        assert!(
            (5..=11).contains(&a.onset_count),
            "onsets={}",
            a.onset_count
        );
        // オンセットはおおむね 480 tick 間隔
        assert!(a.onsets_ticks[0] < 240);
    }

    #[test]
    fn track_filter_and_range_work() {
        let mut project = Project::new("t");
        let bass = midi_track("Bass", vec![(0, 3840, 33, 110)]);
        let bass_id = bass.id.clone();
        project.tracks.push(bass);
        let mut hats = midi_track("Hats", vec![(0, 120, 42, 100)]);
        hats.device = Some(Device::builtin("drum"));
        project.tracks.push(hats);

        // Bass だけ解析 → 低域寄り
        let a = analyze_project(&project, Some(&[bass_id]), None, &Default::default()).unwrap();
        assert!(a.band_energy.low > 0.5);

        // 範囲指定(後半 1 小節 = ハットは冒頭のみなので無音に近い…ではなく Bass が続く)
        let a = analyze_project(
            &project,
            None,
            Some((Tick(3840), Tick(7680))),
            &Default::default(),
        )
        .unwrap();
        assert!(a.duration_seconds < 2.5);
    }

    /// 範囲だけの描き出しが、曲全体を描き出して切り出したものと同じ音になること
    /// (範囲の頭で鳴り続けている長い音・残響も含めて)
    #[test]
    fn range_render_matches_slice_of_full_render() {
        let rms = |x: &[f32]| {
            (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
        };
        for with_reverb in [false, true] {
            let mut project = Project::new("t");
            // 頭から 8 小節鳴り続けるパッド + 6 小節目の短い音(範囲は 6〜7 小節目 = 10〜12 秒)
            let mut t = midi_track(
                "Pad",
                vec![(0, 3840 * 8, 60, 100), (3840 * 5, 480, 72, 110)],
            );
            t.clips[0].length = Tick(3840 * 8);
            if with_reverb {
                t.effects.push(glaux_core::Effect::builtin(
                    glaux_core::FxId::new(),
                    "reverb",
                ));
            }
            project.tracks.push(t);
            let bank = Default::default();
            let full = render_project(&project, SAMPLE_RATE, &bank).unwrap();
            let s0 = project.tempo_map.tick_to_seconds(Tick(3840 * 5));
            let s1 = project.tempo_map.tick_to_seconds(Tick(3840 * 6));
            let expected = &full[(s0 * SAMPLE_RATE) as usize * 2..(s1 * SAMPLE_RATE) as usize * 2];
            let got =
                crate::export::render_project_range(&project, SAMPLE_RATE, &bank, s0, s1).unwrap();
            assert_eq!(got.len(), expected.len());
            let diff: Vec<f32> = got.iter().zip(expected).map(|(a, b)| a - b).collect();
            let ratio = rms(&diff) / rms(expected);
            // 鳴り続けている音は頭から鳴らすので、同じ音(-60dB 以下の差)
            assert!(ratio < 1e-3, "reverb={with_reverb}: 差 {ratio}");
        }
    }

    #[test]
    fn per_track_analysis_reveals_balance() {
        let mut project = Project::new("t");
        let mut quiet = midi_track("Lead", vec![(0, 3840, 72, 100)]);
        quiet.volume_db = -18.0;
        project.tracks.push(quiet);
        let loud = midi_track("Bass", vec![(0, 3840, 33, 110)]);
        project.tracks.push(loud);

        let tracks = analyze_project_tracks(&project, None, &Default::default());
        assert_eq!(tracks.len(), 2);
        let lead = tracks.iter().find(|t| t.name == "Lead").unwrap();
        let bass = tracks.iter().find(|t| t.name == "Bass").unwrap();
        assert!(
            bass.loudness_lufs > lead.loudness_lufs + 6.0,
            "音量差が数値に出るはず: bass={} lead={}",
            bass.loudness_lufs,
            lead.loudness_lufs
        );
        assert!(bass.band_energy.low > lead.band_energy.low);
    }

    /// 帯域 k のしきい値と、聞く側の音の量(最後のフレーム)
    fn masked_margin(masker: &[f32], target: &[f32], k: usize) -> f64 {
        let thr = masking_threshold(&band_frames(masker));
        let tgt = band_frames(target);
        let t = thr.len().min(tgt.len()) - 1;
        thr[t][k] - tgt[t].db[k]
    }

    fn band_of_hz(f: f64) -> usize {
        BARK_EDGES
            .windows(2)
            .position(|w| f >= w[0] && f < w[1])
            .unwrap()
    }

    #[test]
    fn noise_masks_more_than_a_tone_and_far_bands_do_not_mask() {
        let n = 48_000;
        let sine = |f: f32, amp: f32| -> Vec<f32> {
            (0..n)
                .map(|i| (i as f32 * f * std::f32::consts::TAU / 48_000.0).sin() * amp)
                .collect()
        };
        // 1kHz の帯域(920〜1080Hz)に絞った雑音: 白色雑音を 2 次のバンドパスで
        let band_noise = |amp: f32| -> Vec<f32> {
            let mut r: u32 = 9;
            let (mut lp, mut hp) = (lowpass(1080.0), highpass(920.0));
            let raw: Vec<f32> = (0..n)
                .map(|_| {
                    r ^= r << 13;
                    r ^= r >> 17;
                    r ^= r << 5;
                    let x = (r as f64 / u32::MAX as f64 - 0.5) * 2.0;
                    lp.next(hp.next(x)) as f32
                })
                .collect();
            let rms = (raw.iter().map(|v| v * v).sum::<f32>() / n as f32).sqrt();
            raw.iter()
                .map(|v| v / rms * amp * std::f32::consts::FRAC_1_SQRT_2)
                .collect()
        };
        let k = band_of_hz(1000.0);
        // 聞く側は約 -30dBFS。同じ大きさ(約 -10dBFS)でも、雑音は純音よりずっと強く覆う
        // (帯域に絞った雑音は一部が帯域の外へ漏れるので、差は理論値の 17.5dB より小さく出る)
        let target = sine(1000.0, 0.03);
        let by_tone = masked_margin(&sine(1000.0, 0.3), &target, k);
        let by_noise = masked_margin(&band_noise(0.3), &target, k);
        assert!(
            by_noise > by_tone + 6.0,
            "雑音 {by_noise:.1} / 純音 {by_tone:.1}"
        );
        assert!(by_noise > 0.0, "雑音には覆われる: {by_noise:.1}");
        assert!(by_tone < 0.0, "純音には覆われない: {by_tone:.1}");
        // 離れた帯域(4kHz)は 500Hz の大きな雑音に覆われない
        let far = masked_margin(&sine(500.0, 0.5), &sine(4000.0, 0.03), band_of_hz(4000.0));
        assert!(far < -20.0, "{far:.1}");
    }

    #[test]
    fn masking_lingers_shortly_after_a_loud_sound() {
        // 大きな雑音の 100ms 後でも、しきい値はすぐには下がらない(前向きのマスキング)
        let frames = vec![
            BandFrame {
                db: [-10.0; 24],
                tonality: [0.0; 24],
            },
            BandFrame {
                db: [-140.0; 24],
                tonality: [0.0; 24],
            },
        ];
        let t = masking_threshold(&frames);
        assert!(t[1][10] > t[0][10] - FORWARD_DECAY_DB - 0.01);
        assert!(t[1][10] > -60.0);
    }

    #[test]
    fn masking_and_stereo_and_loudness_are_reported() {
        let mut project = Project::new("t");
        // 同じ音域でずっと大きい Pad が、小さい Lead を覆う
        let mut lead = midi_track("Lead", vec![(0, 7680, 69, 60)]);
        lead.volume_db = -18.0;
        lead.pan = -1.0;
        let pad = midi_track("Pad", vec![(0, 7680, 69, 120)]);
        project.tracks.push(lead);
        project.tracks.push(pad);
        let mix = analyze_mix(&project, None, &Default::default());
        eprintln!("{:?}", mix.masking);
        assert!(
            mix.masking
                .iter()
                .any(|m| m.track == "Lead" && m.masked_by == "Pad"),
            "Lead が Pad に覆われている: {:?}",
            mix.masking
        );
        assert!(
            !mix.masking
                .iter()
                .any(|m| m.track == "Pad" && m.masked_by == "Lead"),
            "逆は起きていない"
        );

        let a = analyze_project(&project, None, None, &Default::default()).unwrap();
        eprintln!(
            "{:?} LRA {} TP {} ST {:?}",
            a.stereo, a.loudness_range_lu, a.true_peak_dbtp, a.short_term_lufs
        );
        assert!(a.stereo.balance_db < 0.0 || a.stereo.correlation < 1.0);
        assert!(a.true_peak_dbtp > -60.0 && a.true_peak_dbtp >= a.peak_db - 0.5);
        assert!(!a.short_term_lufs.is_empty());
        assert!(a.max_short_term_lufs > -70.0);

        // 片側に寄せると左右の差が出る
        let mut left = Project::new("l");
        let mut t = midi_track("L", vec![(0, 3840, 60, 100)]);
        t.pan = -1.0;
        left.tracks.push(t);
        let a = analyze_project(&left, None, None, &Default::default()).unwrap();
        assert!(a.stereo.balance_db < -10.0, "{:?}", a.stereo);
    }

    #[test]
    fn empty_selection_is_error() {
        let project = Project::new("t");
        assert!(analyze_project(&project, None, None, &Default::default()).is_err());
    }
}
