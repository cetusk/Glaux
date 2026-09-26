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
    /// `track` がこの帯域で鳴っている時間のうち、`masked_by` が 6dB 以上大きい時間の割合
    pub time_ratio: f64,
    /// この帯域が `track` のエネルギーに占める割合(大きいほど、その音の主要な帯域)
    pub band_share: f64,
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
    let offset_seconds = range.map_or(0.0, |(start, _)| project.tempo_map.tick_to_seconds(start));
    let sliced: &[f32] = &stereo;
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
    let stereo = stereo_info(sliced);

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
    })
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

fn stereo_info(stereo: &[f32]) -> StereoInfo {
    let (mut ll, mut rr, mut lr) = (0.0f64, 0.0f64, 0.0f64);
    let (mut lo_ll, mut lo_rr, mut lo_lr) = (0.0f64, 0.0f64, 0.0f64);
    let (mut mid, mut side) = (0.0f64, 0.0f64);
    let (mut fl, mut fr) = (lowpass(250.0), lowpass(250.0));
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
    }
}

/// 聴覚の帯域に近い区切り(Zwicker の臨界帯域の境界、Hz)
const BARK_EDGES: [f64; 25] = [
    20.0, 100.0, 200.0, 300.0, 400.0, 510.0, 630.0, 770.0, 920.0, 1080.0, 1270.0, 1480.0, 1720.0,
    2000.0, 2320.0, 2700.0, 3150.0, 3700.0, 4400.0, 5300.0, 6400.0, 7700.0, 9500.0, 12000.0,
    15500.0,
];

/// モノラルを 100ms ごとの臨界帯域エネルギー(dB)の列にする。
fn band_frames(mono: &[f32]) -> Vec<[f64; 24]> {
    const N: usize = 4096;
    let hop = (0.1 * SAMPLE_RATE) as usize;
    let mut planner = FftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(N);
    let hann: Vec<f64> = (0..N)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / N as f64).cos())
        .collect();
    let bin_hz = SAMPLE_RATE / N as f64;
    let mut out = Vec::new();
    let mut buf = vec![Complex::new(0.0, 0.0); N];
    let mut pos = 0;
    while pos + N <= mono.len() {
        for i in 0..N {
            buf[i] = Complex::new(mono[pos + i] as f64 * hann[i], 0.0);
        }
        fft.process(&mut buf);
        let mut bands = [0.0f64; 24];
        for (k, c) in buf[..N / 2].iter().enumerate() {
            let f = k as f64 * bin_hz;
            if let Some(b) = BARK_EDGES.windows(2).position(|w| f >= w[0] && f < w[1]) {
                bands[b] += c.norm_sqr();
            }
        }
        out.push(bands.map(|p| 10.0 * p.max(1e-12).log10()));
        pos += hop;
    }
    out
}

/// トラックごとにソロでレンダし、要約とトラック間のかぶり(マスキング)を求める。
///
/// かぶりの判定(簡易な心理音響モデル): 臨界帯域ごとに、あるトラックが鳴っている
/// (その帯域での自分の最大から -30dB 以内の)時間のうち、別のトラックが同じ帯域で
/// 6dB 以上大きい時間の割合。その帯域が自分のエネルギーの 8% 以上を占めるものだけを数える。
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
    let mut frames: Vec<(String, Vec<[f64; 24]>)> = Vec::new();
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
    let mut masking = Vec::new();
    for (bi, (b_name, b)) in frames.iter().enumerate() {
        // 帯域ごとのエネルギーの割合と、帯域ごとの最大
        let mut share = [0.0f64; 24];
        let mut maxes = [-120.0f64; 24];
        for f in b {
            for k in 0..24 {
                share[k] += 10f64.powf(f[k] / 10.0);
                maxes[k] = maxes[k].max(f[k]);
            }
        }
        let total: f64 = share.iter().sum::<f64>().max(1e-12);
        for (ai, (a_name, a)) in frames.iter().enumerate() {
            if ai == bi {
                continue;
            }
            for k in 0..24 {
                let s = share[k] / total;
                if s < 0.08 {
                    continue;
                }
                let n = b.len().min(a.len());
                let mut active = 0;
                let mut masked = 0;
                for t in 0..n {
                    if b[t][k] > maxes[k] - 30.0 {
                        active += 1;
                        if a[t][k] >= b[t][k] + 6.0 {
                            masked += 1;
                        }
                    }
                }
                if active >= 5 && masked as f64 / active as f64 >= 0.3 {
                    masking.push(MaskingIssue {
                        track: b_name.clone(),
                        masked_by: a_name.clone(),
                        band_hz: (BARK_EDGES[k] as u32, BARK_EDGES[k + 1] as u32),
                        time_ratio: (masked as f64 / active as f64 * 100.0).round() / 100.0,
                        band_share: (s * 100.0).round() / 100.0,
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
