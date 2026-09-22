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
pub fn analyze_project_tracks(
    project: &Project,
    range: Option<(Tick, Tick)>,
    bank: &crate::data::SampleBank,
) -> Vec<TrackAnalysis> {
    project
        .tracks
        .iter()
        .filter_map(|t| {
            let a =
                analyze_project(project, Some(std::slice::from_ref(&t.id)), range, bank).ok()?;
            Some(TrackAnalysis {
                track_id: t.id.to_string(),
                name: t.name.clone(),
                loudness_lufs: a.loudness_lufs,
                rms_db: a.rms_db,
                peak_db: a.peak_db,
                spectral_centroid_hz: a.spectral_centroid_hz,
                band_energy: a.band_energy,
            })
        })
        .collect()
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
    let stereo = render_project(&target, SAMPLE_RATE, bank)?;

    // tick 範囲 → サンプル範囲でスライス
    let (offset_seconds, sliced): (f64, &[f32]) = match range {
        Some((start, end)) => {
            let s0 = project.tempo_map.tick_to_seconds(start);
            let s1 = project.tempo_map.tick_to_seconds(end);
            let i0 = ((s0 * SAMPLE_RATE) as usize * 2).min(stereo.len());
            let i1 = ((s1 * SAMPLE_RATE) as usize * 2).min(stereo.len());
            (s0, &stereo[i0..i1.max(i0)])
        }
        None => (0.0, &stereo[..]),
    };
    if sliced.len() < 4096 {
        return Err(ExportError::Empty);
    }

    let frames = sliced.len() / 2;
    let mono: Vec<f32> = sliced
        .chunks_exact(2)
        .map(|c| (c[0] + c[1]) * 0.5)
        .collect();

    // ---- レベル系 ----
    let peak = sliced.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let mean_sq = sliced.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / sliced.len() as f64;
    let peak_db = amp_db(peak as f64);
    let rms_db = 10.0 * mean_sq.max(1e-12).log10();
    let clipped = peak >= 0.999;

    let loudness_lufs = integrated_lufs(sliced);

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
    })
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
fn integrated_lufs(stereo: &[f32]) -> f64 {
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
                    id: NoteId::new(),
                    pos: Tick(pos),
                    dur: Tick(dur),
                    pitch,
                    vel,
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
    fn empty_selection_is_error() {
        let project = Project::new("t");
        assert!(analyze_project(&project, None, None, &Default::default()).is_err());
    }
}
