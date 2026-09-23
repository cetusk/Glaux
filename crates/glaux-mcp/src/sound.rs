//! 音色の解析・比較の対象になる「音」を用意する(UI と MCP で共用)。
//!
//! 対象は 3 種類:
//! - 音声クリップ(クリップが参照している範囲。テンポ追従中も素材の範囲を正しく切り出す)
//! - 音声ファイル(WAV / MP3 / FLAC / OGG / M4A)
//! - トラックの音源で鳴らした 1 音(`render_track_note`。CLAP プラグインも鳴る)

use glaux_core::{ClipContent, ClipId, Project, TrackId};
use std::path::{Path, PathBuf};

/// 解析・比較の対象。
#[derive(Clone, Debug)]
pub enum SoundSource {
    Clip(ClipId),
    File(PathBuf),
    TrackNote {
        track: TrackId,
        pitch: u8,
        velocity: u8,
        seconds: f64,
    },
}

/// 読み込んだ音(モノラル)。
pub struct LoadedSound {
    pub frames: Vec<f32>,
    pub sample_rate: f32,
    /// 何の音か(表示用)
    pub label: String,
}

/// 試し鳴らしのサンプルレート
pub const RENDER_RATE: f64 = 48_000.0;

/// 音声クリップが参照している範囲をモノラルで読む。
pub fn load_clip(project: &Project, dir: &Path, clip_id: &ClipId) -> Result<LoadedSound, String> {
    let clip = project
        .tracks
        .iter()
        .find_map(|t| t.clips.iter().find(|c| &c.id == clip_id))
        .ok_or_else(|| format!("クリップが見つかりません: {clip_id}"))?;
    let ClipContent::Audio {
        asset,
        offset_samples,
        stretch,
        ..
    } = &clip.content
    else {
        return Err(format!("{clip_id} は音声クリップではありません"));
    };
    let meta = project
        .assets
        .get(asset)
        .ok_or_else(|| format!("アセットが見つかりません: {asset}"))?;
    let data = glaux_engine::load_wav_mono(&dir.join(&meta.path))?;
    let secs = stretch
        .follow_seconds(clip.length.0 as f64)
        .unwrap_or_else(|| {
            project.tempo_map.tick_to_seconds(clip.start + clip.length)
                - project.tempo_map.tick_to_seconds(clip.start)
        });
    let from = (*offset_samples as usize).min(data.frames.len());
    let to = (from + (secs * data.sample_rate as f64) as usize).min(data.frames.len());
    Ok(LoadedSound {
        frames: data.frames[from..to].to_vec(),
        sample_rate: data.sample_rate,
        label: format!("音声クリップ「{}」", clip.name),
    })
}

/// 音声ファイルをモノラルで読む。
pub fn load_file(path: &Path) -> Result<LoadedSound, String> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let label = format!(
        "ファイル「{}」",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    );
    if ext == "wav" {
        let d = glaux_engine::load_wav_mono(path)?;
        return Ok(LoadedSound {
            frames: d.frames,
            sample_rate: d.sample_rate,
            label,
        });
    }
    let bytes =
        std::fs::read(path).map_err(|e| format!("読み込めません({}): {e}", path.display()))?;
    let (inter, ch, sr) = crate::assets::decode_audio(bytes, &ext)?;
    let ch = ch.max(1) as usize;
    let frames = inter
        .chunks(ch)
        .map(|c| c.iter().sum::<f32>() / c.len() as f32)
        .collect();
    Ok(LoadedSound {
        frames,
        sample_rate: sr as f32,
        label,
    })
}

/// トラックの音源で 1 音鳴らす。
pub fn render_note(
    project: &Project,
    dir: &Path,
    track: &TrackId,
    pitch: u8,
    velocity: u8,
    seconds: f64,
) -> Result<LoadedSound, String> {
    let t = project
        .track(track)
        .ok_or_else(|| format!("トラックが見つかりません: {track}"))?;
    let bank = glaux_engine::SampleBank::load(project, dir);
    let frames = glaux_engine::render_track_note(
        project,
        track,
        pitch,
        velocity,
        seconds,
        RENDER_RATE,
        &bank,
    )
    .map_err(|e| format!("「{}」の音を鳴らせません(MIDI トラックのみ): {e}", t.name))?;
    Ok(LoadedSound {
        frames,
        sample_rate: RENDER_RATE as f32,
        label: format!(
            "トラック「{}」の音(MIDI {pitch}、ベロシティ {velocity})",
            t.name
        ),
    })
}

pub fn load(project: &Project, dir: &Path, source: &SoundSource) -> Result<LoadedSound, String> {
    let s = match source {
        SoundSource::Clip(id) => load_clip(project, dir, id)?,
        SoundSource::File(p) => load_file(p)?,
        SoundSource::TrackNote {
            track,
            pitch,
            velocity,
            seconds,
        } => render_note(project, dir, track, *pitch, *velocity, *seconds)?,
    };
    if s.frames.iter().all(|v| v.abs() < 1e-6) {
        return Err(format!("{} が無音です", s.label));
    }
    Ok(s)
}

/// 音程を推定する。SwiftF0(学習済みモデル)を使い、失敗したら `None`(呼び出し側で内蔵の YIN に任せる)。
pub fn pitch_track(sound: &LoadedSound) -> Option<Vec<glaux_engine::timbre::PitchFrame>> {
    use glaux_engine::timbre::{resample_pitch, PitchFrame};
    let est = glaux_ml::pitch::track(&sound.frames, sound.sample_rate).ok()?;
    let frames: Vec<PitchFrame> = est
        .iter()
        .map(|e| PitchFrame {
            time: e.time,
            f0: if e.confidence >= 0.5 { e.f0 } else { 0.0 },
            confidence: e.confidence,
        })
        .collect();
    Some(resample_pitch(
        &frames,
        sound.frames.len() as f32 / sound.sample_rate,
    ))
}

/// 音色記述子を求める(音程は SwiftF0。使えなければ内蔵の YIN)。
pub fn describe(sound: &LoadedSound) -> glaux_engine::timbre::SoundDescriptors {
    let pitch = pitch_track(sound);
    glaux_engine::timbre::describe(&sound.frames, sound.sample_rate, pitch.as_deref())
}

/// 音声のビート・小節頭・テンポ(Beat This!)。
#[derive(Clone, Debug, serde::Serialize)]
pub struct BeatReport {
    pub source: String,
    pub duration_sec: f64,
    /// 平均テンポ(BPM、小数 2 桁)。ビートが少なすぎれば null
    pub bpm: Option<f64>,
    /// 半分・倍のテンポ(ビートの取り方の解釈違いの候補)
    pub bpm_alternatives: Vec<f64>,
    /// テンポの揺れ(拍の長さに対する割合)。0.015 未満は打ち込み・クリックに合わせた演奏
    pub tempo_variation: Option<f64>,
    /// 1 小節のビート数(拍子の分子の推定)
    pub beats_per_bar: Option<u32>,
    /// 最初の小節頭の時刻(秒。素材の先頭から)
    pub first_downbeat_sec: Option<f64>,
    pub beat_count: usize,
    pub downbeat_count: usize,
    /// ビートの時刻(秒。先頭から最大 256 個)
    pub beats: Vec<f64>,
    /// 小節頭の時刻(秒。先頭から最大 128 個)
    pub downbeats: Vec<f64>,
    /// 言葉での要約
    pub summary: String,
}

/// ビート・小節頭・テンポを推定する。
pub fn beats(sound: &LoadedSound) -> Result<BeatReport, String> {
    let t = glaux_ml::beats::track(&sound.frames, sound.sample_rate).map_err(|e| e.to_string())?;
    // JSON に f32 の誤差(0.0199999…)が出ないよう f64 で丸める
    let r2 = |v: f32| (v as f64 * 100.0).round() / 100.0;
    let r3 = |v: f32| (v as f64 * 1000.0).round() / 1000.0;
    let bpm = t.bpm().map(r2);
    let variation = t.tempo_variation().map(r3);
    let bpb = t.beats_per_bar();
    let summary = match bpm {
        None => "ビートが見つかりません(拍のはっきりしない音か、短すぎます)".to_owned(),
        Some(b) => {
            let feel = match variation {
                Some(v) if v < 0.015 => "テンポは一定(打ち込み・クリックに合わせた演奏)",
                Some(v) if v < 0.04 => "テンポに人の演奏らしい揺れがある",
                Some(_) => "テンポが大きく揺れる(ルバート・テンポの変化があるか、拍が取りにくい)",
                None => "",
            };
            let meter = bpb.map(|n| format!("、1 小節 {n} 拍")).unwrap_or_default();
            format!("約 {b} BPM{meter}。{feel}")
        }
    };
    Ok(BeatReport {
        source: sound.label.clone(),
        duration_sec: r2(sound.frames.len() as f32 / sound.sample_rate),
        bpm,
        bpm_alternatives: bpm
            .map(|b| {
                [b / 2.0, b * 2.0]
                    .into_iter()
                    .filter(|v| (40.0..=240.0).contains(v))
                    .map(|v| (v * 100.0).round() / 100.0)
                    .collect()
            })
            .unwrap_or_default(),
        tempo_variation: variation,
        beats_per_bar: bpb,
        first_downbeat_sec: t.downbeats.first().copied().map(r3),
        beat_count: t.beats.len(),
        downbeat_count: t.downbeats.len(),
        beats: t.beats.iter().take(256).copied().map(r3).collect(),
        downbeats: t.downbeats.iter().take(128).copied().map(r3).collect(),
        summary,
    })
}
