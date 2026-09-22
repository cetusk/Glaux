//! オフラインレンダリングと WAV 書き出し。
//!
//! リアルタイム再生と同じ [`Renderer`](crate::render::Renderer) を使うので、
//! 「聴こえている音がそのまま書き出される」ことが保証される。
//! オーディオデバイスは不要(解析やヘッドレス環境でも使える)。

use crate::data::build_playback_data;
use crate::render::{Renderer, Shared};
use glaux_core::Project;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("プロジェクトにノートがありません")]
    Empty,
    #[error("WAV の書き込みに失敗: {0}")]
    Wav(#[from] hound::Error),
    #[error("書き込み先を作成できません: {0}")]
    Io(#[from] std::io::Error),
}

/// プロジェクト全体をステレオ・インターリーブの f32 にレンダリングする。
/// 終端はレンダラの自動停止(余韻込み)に任せ、末尾の無音は切り詰める。
pub fn render_project(
    project: &Project,
    sample_rate: f64,
    bank: &crate::data::SampleBank,
) -> Result<Vec<f32>, ExportError> {
    let data = build_playback_data(project, sample_rate, bank);
    if data.events.is_empty() && data.audio_events.is_empty() {
        return Err(ExportError::Empty);
    }
    let shared = Arc::new(Shared::new(data));
    shared.playing.store(true, Ordering::Release);
    let mut renderer = Renderer::new(shared.clone());

    const BLOCK: usize = 4096;
    // 安全上限: 曲の終端 + 10 秒(自動停止が先に来るのが通常)
    let cap = {
        let d = shared.data.load();
        (d.end_sample + (10.0 * sample_rate) as u64) as usize
    };

    let mut out: Vec<f32> = Vec::new();
    let mut buf = [0.0f32; BLOCK * 2];
    while shared.playing.load(Ordering::Acquire) && out.len() / 2 < cap {
        renderer.process(&mut buf, 2);
        out.extend_from_slice(&buf);
    }

    // 末尾の無音を切り詰める(+0.5 秒の余白を残す)
    let last_audible = out
        .iter()
        .rposition(|s| s.abs() > 1e-4)
        .unwrap_or(out.len().saturating_sub(1));
    let keep = (last_audible / 2 + 1 + (0.5 * sample_rate) as usize) * 2;
    out.truncate(keep.min(out.len()));
    Ok(out)
}

/// プロジェクトを 16bit ステレオ WAV に書き出す。返り値は書き出した秒数。
pub fn export_wav(
    project: &Project,
    path: &Path,
    sample_rate: f64,
    bank: &crate::data::SampleBank,
) -> Result<f64, ExportError> {
    let samples = render_project(project, sample_rate, bank)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: sample_rate as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for s in &samples {
        writer.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    writer.finalize()?;
    Ok(samples.len() as f64 / 2.0 / sample_rate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::{Clip, ClipContent, ClipId, Note, NoteId, Tick, Track, TrackId, TrackKind};

    fn test_project() -> Project {
        let mut project = Project::new("Export");
        let mut track = Track::new(TrackId::new(), "T", TrackKind::Midi);
        let mut clip = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(1920));
        if let ClipContent::Midi { notes, .. } = &mut clip.content {
            notes.push(Note {
                articulation: Default::default(),
                pitch_curve: vec![],
                id: NoteId::new(),
                pos: Tick(0),
                dur: Tick(960),
                pitch: 60,
                vel: 100,
            });
        }
        track.clips.push(clip);
        project.tracks.push(track);
        project
    }

    #[test]
    fn renders_project_offline() {
        let samples = render_project(&test_project(), 48_000.0, &Default::default()).unwrap();
        // 0.5 秒のノート + 余韻。ステレオなので偶数長
        assert!(samples.len() % 2 == 0);
        assert!(samples.len() as f64 / 2.0 / 48_000.0 > 0.5);
        let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
        assert!(rms > 0.01, "音が入っているはず");
    }

    #[test]
    fn writes_valid_wav() {
        let dir = std::env::temp_dir().join("glaux-export-test");
        let path = dir.join("out.wav");
        let seconds = export_wav(&test_project(), &path, 48_000.0, &Default::default()).unwrap();
        assert!(seconds > 0.5);

        let reader = hound::WavReader::open(&path).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 2);
        assert_eq!(spec.sample_rate, 48_000);
        assert_eq!(reader.duration() as f64 / 48_000.0, seconds);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_project_is_an_error() {
        let project = Project::new("Empty");
        assert!(matches!(
            render_project(&project, 48_000.0, &Default::default()),
            Err(ExportError::Empty)
        ));
    }
}
