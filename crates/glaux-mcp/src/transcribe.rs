//! 音声クリップ → MIDI クリップ(譜起こし)のコマンド組み立て。
//! 解析本体は単旋律が `glaux_engine::transcribe`(YIN)、和音が `glaux_ml`(basic-pitch)。
//! UI(Tauri)と MCP ツールが共用する。

use glaux_core::{Clip, ClipId, Command, Project, Track, TrackId, TrackKind};
use glaux_engine::transcribe::{
    to_clip_notes, to_clip_notes_poly, transcribe_mono, TranscribeOptions, TranscribedNote,
};

/// 譜起こしの方式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TranscribeMode {
    /// 単旋律(鼻歌・歌・単音)。YIN。しゃくれ・細切れ対策が効く
    #[default]
    Melody,
    /// 和音(ピアノ・ギター・伴奏入りの素材)。basic-pitch
    Poly,
}

impl TranscribeMode {
    pub fn parse(s: Option<&str>) -> Result<Self, String> {
        match s.unwrap_or("melody") {
            "melody" | "" => Ok(TranscribeMode::Melody),
            "poly" | "chords" => Ok(TranscribeMode::Poly),
            other => Err(format!("mode は melody か poly です: {other}")),
        }
    }
}
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
    mode: TranscribeMode,
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
        stretch,
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
    // テンポ追従中の素材は元テンポ一定で tick に対応する(素材の秒 ⇔ tick が比例)
    let tempo = match stretch {
        glaux_core::Stretch::Follow { original_bpm } => {
            glaux_core::TempoMap::new(vec![glaux_core::TempoEvent {
                tick: glaux_core::Tick::ZERO,
                bpm: *original_bpm,
            }])
            .map_err(|e| e.to_string())?
        }
        glaux_core::Stretch::None => project.tempo_map.clone(),
    };
    // クリップが参照している範囲だけを解析する
    let secs = tempo.tick_to_seconds(src_clip.start + src_clip.length)
        - tempo.tick_to_seconds(src_clip.start);
    let from = (*offset_samples as usize).min(data.frames.len());
    let to = (from + (secs * data.sample_rate as f64) as usize).min(data.frames.len());
    data.frames = data.frames[from..to].to_vec();

    let clip_notes = match mode {
        TranscribeMode::Melody => {
            let notes = transcribe_mono(&data, opts);
            to_clip_notes(
                &notes,
                &tempo,
                src_clip.start,
                src_clip.length,
                quantize_ticks,
            )
        }
        TranscribeMode::Poly => {
            // 最短音長は明示されたときだけ使う(既定は basic-pitch の 128ms)
            let mut poly_opts = glaux_ml::PolyOptions::default();
            if opts.min_note_ms != TranscribeOptions::default().min_note_ms {
                poly_opts.min_note_secs = (opts.min_note_ms / 1000.0).max(0.05);
            }
            let notes: Vec<TranscribedNote> =
                glaux_ml::transcribe_poly(&data.frames, data.sample_rate, &poly_opts)
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .map(|n| TranscribedNote {
                        start_sec: n.start_sec,
                        end_sec: n.end_sec,
                        pitch: n.pitch,
                        vel: (n.amplitude * 127.0).round().clamp(1.0, 127.0) as u8,
                        reattack: false,
                    })
                    .collect();
            to_clip_notes_poly(
                &notes,
                &tempo,
                src_clip.start,
                src_clip.length,
                quantize_ticks,
            )
        }
    };
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
        gain_db,
        stretch,
        ..
    } = &clip.content
    else {
        return Err(format!("{clip_id} は音声クリップではありません"));
    };
    let gain = 10f32.powf(gain_db / 20.0);
    let meta = project
        .assets
        .get(asset)
        .ok_or_else(|| format!("アセットが見つかりません: {asset}"))?;
    let data = glaux_engine::load_wav_mono(&project_dir.join(&meta.path))?;
    // テンポ追従中は素材の秒数が tick に比例する(タイムライン上の見た目と一致)
    let secs = stretch
        .follow_seconds(clip.length.0 as f64)
        .unwrap_or_else(|| {
            project.tempo_map.tick_to_seconds(clip.start + clip.length)
                - project.tempo_map.tick_to_seconds(clip.start)
        });
    let from = (*offset_samples as usize).min(data.frames.len());
    let to = (from + (secs * data.sample_rate as f64) as usize).min(data.frames.len());
    // 表示はクリップの音量(自動音量調整を含む)を掛けた後の大きさにする
    Ok(glaux_engine::wave_peaks(&data.frames[from..to], buckets)
        .into_iter()
        .map(|(lo, hi)| ((lo * gain).max(-1.0), (hi * gain).min(1.0)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::{ClipContent, Tick};

    /// 和音(C4 E4 G4、1.2 秒)の WAV を置いた音声クリップを持つプロジェクト
    fn chord_project(dir: &Path) -> (Project, ClipId) {
        let sr = 44_100u32;
        let wav = dir.join("chord.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: sr,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&wav, spec).unwrap();
        for i in 0..(2 * sr) {
            let t = i as f32 / sr as f32;
            let mut x = 0.0;
            if t < 1.2 {
                for p in [60.0f32, 64.0, 67.0] {
                    let f0 = 440.0 * 2f32.powf((p - 69.0) / 12.0);
                    let env = (-t * 1.5).exp() * (1.0 - (-t * 400.0).exp());
                    for h in 1..=6 {
                        x += 0.2 * env * (std::f32::consts::TAU * f0 * h as f32 * t).sin()
                            / (h * h) as f32;
                    }
                }
            }
            w.write_sample((x * 20_000.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        let imported = crate::assets::import_wav(dir, &wav).unwrap();
        let mut project = Project::new("t");
        let track = Track::new(TrackId::new(), "Piano", TrackKind::Audio);
        let tid = track.id.clone();
        project.tracks.push(track);
        let clip_id = ClipId::new();
        for c in crate::assets::audio_clip_commands(
            &project,
            &tid,
            &imported,
            clip_id.clone(),
            Tick(0),
            "chord",
        )
        .unwrap()
        {
            project.apply(&c).unwrap();
        }
        (project, clip_id)
    }

    #[test]
    fn poly_mode_keeps_chord_tones_overlapping() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut project, clip_id) = chord_project(tmp.path());
        let t = transcribe_clip_commands(
            &project,
            tmp.path(),
            &clip_id,
            None,
            240,
            &TranscribeOptions::default(),
            TranscribeMode::Poly,
        )
        .unwrap();
        for c in &t.commands {
            project.apply(c).unwrap();
        }
        let (_, clip) = project.clip(&t.clip_id).unwrap();
        let ClipContent::Midi { notes, .. } = &clip.content else {
            panic!("MIDI クリップのはず");
        };
        for p in [60, 64, 67] {
            let n = notes
                .iter()
                .find(|n| n.pitch == p)
                .unwrap_or_else(|| panic!("{p} が無い: {notes:?}"));
            assert_eq!(n.pos, Tick(0), "和音は同時に始まる");
            assert!(n.dur.0 >= 1920, "和音の音は重なったまま残る: {n:?}");
        }
    }

    #[test]
    fn mode_parses() {
        assert_eq!(TranscribeMode::parse(None), Ok(TranscribeMode::Melody));
        assert_eq!(
            TranscribeMode::parse(Some("poly")),
            Ok(TranscribeMode::Poly)
        );
        assert!(TranscribeMode::parse(Some("drums")).is_err());
    }
}
