//! トラックを音声にする(フリーズ / バウンス)。
//!
//! 対象のトラックを、自分のエフェクト・音量・パン・オートメーションと、センドしているバスの響きまで込みで
//! 描き出し(マスターのエフェクトとクリップ防止は通さない)、新しい音声トラックとして直後に置いて、
//! 元のトラックはミュートする。全体で 1 件の履歴(1 回の undo で戻る)。
//!
//! 用途: CLAP プラグインの音源はゲーム(Godot)で鳴らないので音声にしておく / 重いトラックを軽くする /
//! 音声として切り貼りする。

use crate::assets::{audio_clip_commands, import_wav};
use glaux_core::{ClipId, Command, Project, Tick, Track, TrackId, TrackKind, TrackProp};
use std::path::Path;

/// 描き出しのサンプルレート
const RATE: u32 = 48_000;

/// 音声にした結果。
pub struct Bounced {
    /// 適用するコマンド(音声トラックの追加・音声の登録とクリップ・元のトラックのミュート)
    pub commands: Vec<Command>,
    pub new_track: TrackId,
    pub seconds: f64,
    pub label: String,
}

/// 描き出す用のプロジェクト: 対象のトラックと、それがセンドしているバスだけを残す。
/// マスターのエフェクト・オートメーションは外し、音量は 0dB にする
pub fn stem_project(project: &Project, track: &Track) -> Project {
    let buses: Vec<TrackId> = track.sends.iter().map(|s| s.target.clone()).collect();
    let mut p = project.clone();
    p.tracks
        .retain(|t| t.id == track.id || buses.contains(&t.id));
    for t in &mut p.tracks {
        t.solo = false;
        if t.id == track.id {
            t.mute = false;
        }
    }
    p.master.effects.clear();
    p.master.fx_links = None;
    p.master.automation.clear();
    p.master.volume_db = 0.0;
    p
}

/// `track_id` のトラックを音声にするコマンドを作る(描き出した WAV はプロジェクトの audio/ に取り込む)。
pub fn bounce_track(
    project: &Project,
    project_dir: &Path,
    track_id: &TrackId,
    bank: &glaux_engine::SampleBank,
) -> Result<Bounced, String> {
    let index = project
        .tracks
        .iter()
        .position(|t| &t.id == track_id)
        .ok_or_else(|| format!("トラックが見つかりません: {track_id}"))?;
    let track = &project.tracks[index];
    if track.kind == TrackKind::Bus {
        return Err("バスは音声にできません(送っているトラックを音声にしてください)".to_owned());
    }

    let stem = stem_project(project, track);
    let stereo = glaux_engine::render_stem(&stem, RATE as f64, bank)
        .map_err(|e| format!("「{}」を描き出せません: {e}", track.name))?;

    // 32bit 浮動小数のステレオ WAV にして取り込む(0dBFS を超える音も潰さずに残す)
    let tmp_dir = project_dir.join("cache");
    std::fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;
    let tmp = tmp_dir.join(format!("bounce-{track_id}.wav"));
    {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: RATE,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut w = hound::WavWriter::create(&tmp, spec).map_err(|e| e.to_string())?;
        for s in &stereo {
            w.write_sample(*s).map_err(|e| e.to_string())?;
        }
        w.finalize().map_err(|e| e.to_string())?;
    }
    let imported = import_wav(project_dir, &tmp);
    let _ = std::fs::remove_file(&tmp);
    let imported = imported?;

    let new_id = TrackId::new();
    let name = format!("{}(音声)", track.name);
    let mut new_track = Track::new(new_id.clone(), name.clone(), TrackKind::Audio);
    new_track.color = track.color.clone();
    // 左右が同じ音(中央に置いたモノラルの音源)は、読み込み時にモノラルとして扱われ、中央でもパンの
    // 計算で -3dB になる。描き出した音には元のトラックの中央の -3dB が既に入っているので、音量を √2 倍
    // (+3.01dB)にして二重に下がるのを打ち消す(読み込み側の判定と同じ条件)
    let is_mono = stereo
        .as_chunks::<2>()
        .0
        .iter()
        .all(|c| (c[0] - c[1]).abs() < 1e-6);
    if is_mono {
        new_track.volume_db = 20.0 * std::f32::consts::SQRT_2.log10();
    }
    let add_track = Command::AddTrack {
        track: new_track,
        index: Some(index + 1),
    };
    // 音声クリップのコマンドは、置き先のトラックがある状態で作る
    let mut with_track = project.clone();
    with_track.apply(&add_track).map_err(|e| e.to_string())?;
    let mut commands = vec![add_track];
    commands.extend(audio_clip_commands(
        &with_track,
        &new_id,
        &imported,
        ClipId::new(),
        Tick::ZERO,
        &name,
    )?);
    if !track.mute {
        commands.push(Command::SetTrackProp {
            id: track_id.clone(),
            prop: TrackProp::Mute(true),
        });
    }
    Ok(Bounced {
        commands,
        new_track: new_id,
        seconds: stereo.len() as f64 / 2.0 / RATE as f64,
        label: format!("「{}」を音声にする(元のトラックはミュート)", track.name),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::{Articulation, Clip, ClipContent, Note, NoteId};

    #[test]
    fn bounced_audio_sounds_like_the_track() {
        // 中央(左右が同じ = モノラル扱い)とパンを振った場合(ステレオ扱い)の両方
        for pan in [0.0f32, -0.6] {
            check_bounce(pan);
        }
    }

    fn check_bounce(pan: f32) {
        let tmp = tempfile::tempdir().unwrap();
        let mut p = Project::new("b");
        let tid = TrackId::new();
        let mut t = Track::new(tid.clone(), "Lead", TrackKind::Midi);
        t.volume_db = -6.0;
        t.pan = pan;
        let mut c = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(3840));
        if let ClipContent::Midi { notes, .. } = &mut c.content {
            notes.push(Note {
                id: NoteId::new(),
                pos: Tick(0),
                dur: Tick(1920),
                pitch: 60,
                vel: 100,
                articulation: Articulation::Normal,
                pitch_curve: vec![],
                glide_ms: None,
            });
        }
        t.clips.push(c);
        p.tracks.push(t);
        // マスターの音量はフリーズに入れない
        p.master.volume_db = -20.0;

        let b = bounce_track(&p, tmp.path(), &tid, &Default::default()).unwrap();
        let mut after = p.clone();
        after
            .apply(&Command::batch(b.label.clone(), b.commands))
            .unwrap();
        // 音声トラックが直後にでき、元はミュート
        assert_eq!(after.tracks.len(), 2);
        assert_eq!(after.tracks[1].id, b.new_track);
        assert_eq!(after.tracks[1].kind, TrackKind::Audio);
        assert!(after.tracks[0].mute);
        assert!(b.seconds > 1.0);

        // 元のトラック(ミュート前)と、音声にしたトラックの鳴り方が同じ
        let bank = glaux_engine::SampleBank::load(&after, tmp.path());
        let original = glaux_engine::render_project(&p, 48_000.0, &Default::default()).unwrap();
        let bounced = glaux_engine::render_project(&after, 48_000.0, &bank).unwrap();
        let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
        let n = original.len().min(bounced.len()).min(48_000);
        for ch in 0..2 {
            let pick = |x: &[f32]| {
                x[..n]
                    .iter()
                    .skip(ch)
                    .step_by(2)
                    .copied()
                    .collect::<Vec<f32>>()
            };
            let (a, b2) = (rms(&pick(&original)), rms(&pick(&bounced)));
            assert!(
                (a / b2 - 1.0).abs() < 0.05,
                "pan {pan} ch {ch}: 元 {a} / 音声 {b2}"
            );
        }
    }

    #[test]
    fn bus_cannot_be_bounced() {
        let tmp = tempfile::tempdir().unwrap();
        let mut p = Project::new("b");
        let tid = TrackId::new();
        p.tracks
            .push(Track::new(tid.clone(), "Rev", TrackKind::Bus));
        assert!(bounce_track(&p, tmp.path(), &tid, &Default::default()).is_err());
    }
}
