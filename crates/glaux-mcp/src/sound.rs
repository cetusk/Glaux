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
