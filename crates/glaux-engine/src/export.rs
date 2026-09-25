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
    render_inner(project, sample_rate, bank, None)
}

/// 範囲の手前から描き出す長さ(残響・リリース・コンプの立ち上がりの分)
const PREROLL_SECS: f64 = 3.0;
/// 範囲の頭で鳴っている長い音のために遡る上限
const MAX_PREROLL_SECS: f64 = 30.0;

/// 秒の範囲 `[from, to)` だけを描き出す(解析用。曲全体を描き出すより速い)。
/// 範囲の少し手前から描き出して、残響やリリースを含めた「範囲の頭で聞こえている音」にする。
/// 範囲の頭で鳴っているノート・音声クリップは、その頭から鳴らす(最大 30 秒遡る)。
/// 返すのは範囲の分だけ(ステレオ・インターリーブ)。末尾の無音は切り詰めない
pub fn render_project_range(
    project: &Project,
    sample_rate: f64,
    bank: &crate::data::SampleBank,
    from_secs: f64,
    to_secs: f64,
) -> Result<Vec<f32>, ExportError> {
    let from = (from_secs.max(0.0) * sample_rate) as u64;
    let to = ((to_secs * sample_rate) as u64).max(from);
    render_inner(project, sample_rate, bank, Some((from, to)))
}

fn render_inner(
    project: &Project,
    sample_rate: f64,
    bank: &crate::data::SampleBank,
    range: Option<(u64, u64)>,
) -> Result<Vec<f32>, ExportError> {
    let slots = Arc::new(crate::plugins::new_slots());
    // CLAP の音源・エフェクトがあれば、このスレッドで書き出し専用のインスタンスを作る
    let has_plugins = !crate::plugins::project_plugins(project).is_empty();
    let (offline, data) = if has_plugins {
        let (offline, map) = crate::plugins::OfflinePlugins::create(project, sample_rate, &slots);
        let mut bank = bank.clone();
        bank.plugin_slots = map;
        (
            Some(offline),
            build_playback_data(project, sample_rate, &bank),
        )
    } else {
        (None, build_playback_data(project, sample_rate, bank))
    };
    if data.events.is_empty() && data.audio_events.is_empty() {
        if let Some(o) = offline {
            o.finish(slots.iter().filter_map(|s| s.take_incoming()).collect());
        }
        return Err(ExportError::Empty);
    }
    // 範囲指定なら、描き出しの開始位置(範囲の頭で鳴っている音の頭まで遡る)
    let start = range.map(|(from, _)| {
        let preroll = (PREROLL_SECS * sample_rate) as u64;
        let limit = from.saturating_sub((MAX_PREROLL_SECS * sample_rate) as u64);
        let sounding = data
            .events
            .iter()
            .filter(|e| e.start < from && e.end > from)
            .map(|e| e.start)
            .chain(
                data.audio_events
                    .iter()
                    .filter(|a| a.start < from && a.end > from)
                    .map(|a| a.start),
            )
            .min()
            .unwrap_or(from);
        sounding.min(from.saturating_sub(preroll)).max(limit)
    });
    let mut shared = Shared::new(data);
    shared.plugin_slots = slots;
    let shared = Arc::new(shared);
    shared.playing.store(true, Ordering::Release);
    if let Some(start) = start {
        shared.seek.store(start, Ordering::Release);
    }
    let mut renderer = Renderer::new(shared.clone());

    const BLOCK: usize = 4096;
    // 安全上限: 曲の終端 + 10 秒(自動停止が先に来るのが通常)。範囲指定なら範囲の終わりまで
    let cap = match (range, start) {
        (Some((_, to)), Some(start)) => (to - start) as usize,
        _ => {
            let d = shared.data.load();
            (d.end_sample + (10.0 * sample_rate) as u64) as usize
        }
    };

    let mut out: Vec<f32> = Vec::new();
    let mut buf = [0.0f32; BLOCK * 2];
    while shared.playing.load(Ordering::Acquire) && out.len() / 2 < cap {
        renderer.process(&mut buf, 2);
        out.extend_from_slice(&buf);
    }
    if let Some(o) = offline {
        o.finish(renderer.take_plugins());
    }

    if let (Some((from, to)), Some(start)) = (range, start) {
        // 手前に描き出した分を捨て、範囲の分だけにする(自動停止で足りなければ無音で埋める)
        let skip = ((from - start) as usize * 2).min(out.len());
        out.drain(..skip);
        out.resize((to - from) as usize * 2, 0.0);
        return Ok(out);
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

/// トラックの音源(とエフェクト)で 1 音だけ鳴らした音(モノラル)。音色の解析・比較用。
/// トラックの他のクリップ・オートメーション・音量・パン、マスターのエフェクトは使わない。
/// `seconds` は鍵盤を押している長さで、余韻の分だけ後ろに伸びる(最大 +2 秒程度)。
pub fn render_track_note(
    project: &Project,
    track_id: &glaux_core::TrackId,
    pitch: u8,
    velocity: u8,
    seconds: f64,
    sample_rate: f64,
    bank: &crate::data::SampleBank,
) -> Result<Vec<f32>, ExportError> {
    use glaux_core::{
        Clip, ClipContent, ClipId, Note, NoteId, TempoEvent, TempoMap, Tick, TrackKind,
    };
    let track = project
        .track(track_id)
        .filter(|t| t.kind == TrackKind::Midi)
        .ok_or(ExportError::Empty)?;
    let mut p = project.clone();
    // 120 BPM 固定(1 秒 = 1920 tick)
    p.tempo_map = TempoMap::new(vec![TempoEvent {
        tick: Tick::ZERO,
        bpm: 120.0,
    }])
    .map_err(|_| ExportError::Empty)?;
    p.master.effects.clear();
    p.master.automation.clear();
    p.master.volume_db = 0.0;
    let mut t = track.clone();
    t.automation.clear();
    t.mute = false;
    t.solo = false;
    t.volume_db = 0.0;
    t.pan = 0.0;
    let dur = ((seconds.max(0.05) * 1920.0) as u64).max(1);
    let mut clip = Clip::new_midi(ClipId::new(), "test", Tick::ZERO, Tick(dur + 1920));
    if let ClipContent::Midi { notes, .. } = &mut clip.content {
        notes.push(Note {
            id: NoteId::new(),
            pos: Tick::ZERO,
            dur: Tick(dur),
            pitch: pitch.min(127),
            vel: velocity.clamp(1, 127),
            articulation: Default::default(),
            pitch_curve: vec![],
            glide_ms: None,
        });
    }
    t.clips = vec![clip];
    p.tracks = vec![t];
    let stereo = render_project(&p, sample_rate, bank)?;
    Ok(stereo
        .chunks(2)
        .map(|c| (c[0] + c.get(1).copied().unwrap_or(c[0])) * 0.5)
        .collect())
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
                glide_ms: None,
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
        assert!(samples.len().is_multiple_of(2));
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
