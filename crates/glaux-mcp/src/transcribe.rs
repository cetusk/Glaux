//! 音声クリップ → MIDI クリップ(譜起こし)のコマンド組み立て。
//! 解析本体は `glaux_engine::transcribe`。UI(Tauri)と MCP ツールが共用する。

use glaux_core::{Clip, ClipId, Command, Project, Track, TrackId, TrackKind};
use glaux_engine::transcribe::{to_clip_notes, transcribe_mono, TranscribeOptions};
use std::path::Path;

pub struct Transcribed {
    pub commands: Vec<Command>,
    pub clip_id: ClipId,
    pub track_id: TrackId,
    pub note_count: usize,
    /// 新しく MIDI トラックを作ったか
    pub created_track: bool,
}

/// `clip_id`(音声クリップ)を譜起こしし、同じ位置・長さの MIDI クリップを置くコマンドを作る。
/// `dest_track` が None なら音声トラックの直後に MIDI トラックを新設する。
pub fn transcribe_clip_commands(
    project: &Project,
    project_dir: &Path,
    clip_id: &ClipId,
    dest_track: Option<&TrackId>,
    quantize_ticks: u64,
    opts: &TranscribeOptions,
) -> Result<Transcribed, String> {
    let (src_index, src_clip) = project
        .tracks
        .iter()
        .enumerate()
        .find_map(|(i, t)| t.clips.iter().find(|c| &c.id == clip_id).map(|c| (i, c)))
        .ok_or_else(|| format!("クリップが見つかりません: {clip_id}"))?;
    let glaux_core::ClipContent::Audio {
        asset,
        offset_samples,
        ..
    } = &src_clip.content
    else {
        return Err(format!("{clip_id} は音声クリップではありません"));
    };
    let asset_meta = project
        .assets
        .get(asset)
        .ok_or_else(|| format!("アセットが見つかりません: {asset}"))?;
    let mut data = glaux_engine::load_wav_mono(&project_dir.join(&asset_meta.path))?;
    // クリップが参照している範囲だけを解析する
    let start_sec = project.tempo_map.tick_to_seconds(src_clip.start);
    let end_sec = project
        .tempo_map
        .tick_to_seconds(src_clip.start + src_clip.length);
    let from = (*offset_samples as usize).min(data.frames.len());
    let to =
        (from + ((end_sec - start_sec) * data.sample_rate as f64) as usize).min(data.frames.len());
    data.frames = data.frames[from..to].to_vec();

    let notes = transcribe_mono(&data, opts);
    let clip_notes = to_clip_notes(
        &notes,
        &project.tempo_map,
        src_clip.start,
        src_clip.length,
        quantize_ticks,
    );
    let note_count = clip_notes.len();
    if note_count == 0 {
        return Err("音程のある音を検出できませんでした(音量が小さい、または無音)".to_owned());
    }

    let mut commands = Vec::new();
    let mut created_track = false;
    let track_id = match dest_track {
        Some(id) => {
            let t = project
                .track(id)
                .ok_or_else(|| format!("トラックが見つかりません: {id}"))?;
            if t.kind != TrackKind::Midi {
                return Err(format!("「{}」は MIDI トラックではありません", t.name));
            }
            id.clone()
        }
        None => {
            let id = TrackId::new();
            let src_name = &project.tracks[src_index].name;
            commands.push(Command::AddTrack {
                track: Track::new(id.clone(), format!("{src_name} MIDI"), TrackKind::Midi),
                index: Some(src_index + 1),
            });
            created_track = true;
            id
        }
    };
    let new_clip_id = ClipId::new();
    let mut clip = Clip::new_midi(
        new_clip_id.clone(),
        format!("{} (MIDI)", src_clip.name),
        src_clip.start,
        src_clip.length,
    );
    if let Some(n) = clip.notes_mut() {
        *n = clip_notes;
    }
    commands.push(Command::AddClip {
        track: track_id.clone(),
        clip,
    });
    Ok(Transcribed {
        commands,
        clip_id: new_clip_id,
        track_id,
        note_count,
        created_track,
    })
}

/// 音声クリップの波形ピーク(表示用)。クリップが参照している範囲を `buckets` 個に分ける。
pub fn clip_peaks(
    project: &Project,
    project_dir: &Path,
    clip_id: &ClipId,
    buckets: usize,
) -> Result<Vec<(f32, f32)>, String> {
    let clip = project
        .tracks
        .iter()
        .find_map(|t| t.clips.iter().find(|c| &c.id == clip_id))
        .ok_or_else(|| format!("クリップが見つかりません: {clip_id}"))?;
    let glaux_core::ClipContent::Audio {
        asset,
        offset_samples,
        ..
    } = &clip.content
    else {
        return Err(format!("{clip_id} は音声クリップではありません"));
    };
    let meta = project
        .assets
        .get(asset)
        .ok_or_else(|| format!("アセットが見つかりません: {asset}"))?;
    let data = glaux_engine::load_wav_mono(&project_dir.join(&meta.path))?;
    let secs = project.tempo_map.tick_to_seconds(clip.start + clip.length)
        - project.tempo_map.tick_to_seconds(clip.start);
    let from = (*offset_samples as usize).min(data.frames.len());
    let to = (from + (secs * data.sample_rate as f64) as usize).min(data.frames.len());
    Ok(glaux_engine::wave_peaks(&data.frames[from..to], buckets))
}
