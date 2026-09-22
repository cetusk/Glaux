//! 音声アセットの取り込み。
//!
//! WAV を内容ハッシュ(sha256)名でプロジェクトの `audio/` にコピーし、
//! `Asset` メタデータを返す。同じ内容は同じ ID になるので重複コピーしない。

use glaux_core::{Asset, AssetId, Clip, ClipId, Command, Project, Tick, TrackId, TrackKind};
use std::path::Path;

pub struct ImportedSample {
    pub id: AssetId,
    pub asset: Asset,
    /// 既にプロジェクトに同内容のファイルがあった
    pub already_present: bool,
}

/// WAV ファイルをプロジェクトへ取り込む。
pub fn import_wav(project_dir: &Path, src: &Path) -> Result<ImportedSample, String> {
    let bytes =
        std::fs::read(src).map_err(|e| format!("読み込めません({}): {e}", src.display()))?;

    // メタデータの検証(WAV 以外はここで弾く)
    let reader = hound::WavReader::new(std::io::Cursor::new(&bytes))
        .map_err(|e| format!("WAV として読めません({}): {e}", src.display()))?;
    let spec = reader.spec();
    let frames = reader.duration() as u64;
    if frames == 0 {
        return Err("空の WAV です".to_owned());
    }

    use sha2::Digest;
    let hex = format!("{:x}", sha2::Sha256::digest(&bytes));
    let id = AssetId::from_sha256_hex(&hex).map_err(|e| e.to_string())?;
    let rel_path = format!("audio/{hex}.wav");
    let dest = project_dir.join(&rel_path);

    let already_present = dest.exists();
    if !already_present {
        std::fs::create_dir_all(project_dir.join("audio")).map_err(|e| e.to_string())?;
        std::fs::write(&dest, &bytes).map_err(|e| e.to_string())?;
    }

    Ok(ImportedSample {
        id,
        asset: Asset {
            path: rel_path,
            sample_rate: spec.sample_rate,
            channels: spec.channels,
            frames,
        },
        already_present,
    })
}

/// 取り込んだ WAV を音声トラックのクリップとして置くコマンド列を作る
/// (アセット未登録なら add_asset も含む)。長さは WAV の秒数を `start` 位置の
/// テンポで tick に換算したもの。MIDI トラックに置こうとするとエラー。
pub fn audio_clip_commands(
    project: &Project,
    track_id: &TrackId,
    imported: &ImportedSample,
    clip_id: ClipId,
    start: Tick,
    name: &str,
) -> Result<Vec<Command>, String> {
    let track = project
        .track(track_id)
        .ok_or_else(|| format!("トラックが見つかりません: {track_id}"))?;
    if track.kind != TrackKind::Audio {
        return Err(format!(
            "「{}」は MIDI トラックです。音声クリップは kind: \"audio\" のトラックに置いてください\
             (add_track で作成できます)",
            track.name
        ));
    }
    let secs = imported.asset.frames as f64 / imported.asset.sample_rate.max(1) as f64;
    let start_secs = project.tempo_map.tick_to_seconds(start);
    let end_tick = project.tempo_map.seconds_to_tick(start_secs + secs);
    let length = Tick(end_tick.0.saturating_sub(start.0).max(1));

    let mut cmds = Vec::new();
    if !project.assets.contains_key(&imported.id) {
        cmds.push(Command::AddAsset {
            id: imported.id.clone(),
            asset: imported.asset.clone(),
        });
    }
    cmds.push(Command::AddClip {
        track: track_id.clone(),
        clip: Clip::new_audio(clip_id, name, start, length, imported.id.clone()),
    });
    Ok(cmds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::Track;

    #[test]
    fn audio_clip_commands_convert_length_by_tempo() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("take.wav");
        write_test_wav(&src); // 4800 frames @48k = 0.1 秒
        let imported = import_wav(tmp.path(), &src).unwrap();

        let mut project = Project::new("p");
        let audio = TrackId::new();
        let midi = TrackId::new();
        project
            .tracks
            .push(Track::new(audio.clone(), "Gt", TrackKind::Audio));
        project
            .tracks
            .push(Track::new(midi.clone(), "Syn", TrackKind::Midi));

        // 120bpm: 0.1 秒 = 192 tick
        let cmds = audio_clip_commands(
            &project,
            &audio,
            &imported,
            ClipId::new(),
            Tick(960),
            "take",
        )
        .unwrap();
        assert_eq!(cmds.len(), 2, "add_asset + add_clip");
        let Command::AddClip { clip, .. } = &cmds[1] else {
            panic!("add_clip");
        };
        assert_eq!(clip.start, Tick(960));
        assert_eq!(clip.length, Tick(192));
        assert!(!clip.is_midi());

        // MIDI トラックには置けない
        assert!(
            audio_clip_commands(&project, &midi, &imported, ClipId::new(), Tick(0), "x").is_err()
        );
    }

    fn write_test_wav(path: &Path) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for i in 0..4800 {
            let s = ((i as f32 * 0.05).sin() * 20_000.0) as i16;
            w.write_sample(s).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn imports_and_dedupes_by_content() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("guitar.wav");
        write_test_wav(&src);
        let proj = tmp.path().join("Song.glaux");
        std::fs::create_dir_all(&proj).unwrap();

        let a = import_wav(&proj, &src).unwrap();
        assert!(!a.already_present);
        assert!(proj.join(&a.asset.path).exists());
        assert_eq!(a.asset.sample_rate, 48_000);
        assert_eq!(a.asset.frames, 4800);

        // 同じ内容は同じ ID・コピーなし
        let b = import_wav(&proj, &src).unwrap();
        assert!(b.already_present);
        assert_eq!(a.id, b.id);
    }

    #[test]
    fn rejects_non_wav() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("not_audio.wav");
        std::fs::write(&src, b"hello").unwrap();
        assert!(import_wav(tmp.path(), &src).is_err());
    }
}
