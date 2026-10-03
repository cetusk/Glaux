//! 音声アセットの取り込み。
//!
//! 音声ファイルを内容ハッシュ(sha256)名でプロジェクトの `audio/` に置き、
//! `Asset` メタデータを返す。同じ内容は同じ ID になるので重複コピーしない。
//! WAV はそのままコピー、mp3 / flac / ogg / m4a などは symphonia でデコードして
//! 32bit float WAV に変換して置く(エンジンは WAV だけ読めばよい)。

use glaux_core::{Asset, AssetId, Clip, ClipId, Command, Project, Tick, TrackId, TrackKind};
use std::path::Path;

pub struct ImportedSample {
    pub id: AssetId,
    pub asset: Asset,
    /// 既にプロジェクトに同内容のファイルがあった
    pub already_present: bool,
}

/// 取り込める拡張子(小文字)。UI のファイル選択フィルタと揃えること。
pub const AUDIO_EXTENSIONS: &[&str] = &["wav", "mp3", "flac", "ogg", "m4a", "aac"];

/// 音声ファイルをプロジェクトへ取り込む(WAV 以外は WAV に変換)。
pub fn import_audio(project_dir: &Path, src: &Path) -> Result<ImportedSample, String> {
    let ext = src
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if ext == "wav" {
        return import_wav(project_dir, src);
    }
    let bytes =
        std::fs::read(src).map_err(|e| format!("読み込めません({}): {e}", src.display()))?;
    use sha2::Digest;
    let hex = format!("{:x}", sha2::Sha256::digest(&bytes));
    let id = AssetId::from_sha256_hex(&hex).map_err(|e| e.to_string())?;
    let rel_path = format!("audio/{hex}.wav");
    let dest = project_dir.join(&rel_path);

    let already_present = dest.exists();
    let (channels, sample_rate, frames) = if already_present {
        let r = hound::WavReader::open(&dest).map_err(|e| e.to_string())?;
        (r.spec().channels, r.spec().sample_rate, r.duration() as u64)
    } else {
        let (samples, channels, sample_rate) = decode_audio(bytes, &ext)?;
        std::fs::create_dir_all(project_dir.join("audio")).map_err(|e| e.to_string())?;
        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let tmp = dest.with_extension("tmp");
        let mut w = hound::WavWriter::create(&tmp, spec).map_err(|e| e.to_string())?;
        for v in &samples {
            w.write_sample(*v).map_err(|e| e.to_string())?;
        }
        w.finalize().map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
        (
            channels,
            sample_rate,
            samples.len() as u64 / channels.max(1) as u64,
        )
    };
    if frames == 0 {
        return Err("音声データが空です".to_owned());
    }
    Ok(ImportedSample {
        id,
        asset: Asset {
            path: rel_path,
            sample_rate,
            channels,
            frames,
        },
        already_present,
    })
}

pub use glaux_engine::data::decode_audio;

/// 音声ファイル(WAV / MP3 / FLAC など)をモノラルの波形とサンプルレートにする
pub fn decode_mono(src: &Path) -> Result<(Vec<f32>, f32), String> {
    let ext = src
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if ext == "wav" {
        let d = glaux_engine::load_wav(src)?;
        return Ok((d.frames, d.sample_rate));
    }
    let bytes =
        std::fs::read(src).map_err(|e| format!("読み込めません({}): {e}", src.display()))?;
    let (samples, channels, sr) = decode_audio(bytes, &ext)?;
    let ch = channels.max(1) as usize;
    let mono = samples
        .chunks(ch)
        .map(|c| c.iter().sum::<f32>() / ch as f32)
        .collect();
    Ok((mono, sr as f32))
}

/// 音声から作ったウェーブテーブルの取り込み結果
pub struct ImportedTable {
    pub imported: ImportedSample,
    /// フレーム数(1 周期 2048 点の枚数)
    pub frames: usize,
    /// 素材がすでにウェーブテーブルの形(2048 の倍数)だった
    pub as_is: bool,
}

/// 音声ファイルをウェーブテーブル(1 周期 2048 点 × N 枚を並べた浮動小数のモノラル WAV)にして
/// プロジェクトの audio/ に取り込む。元のファイルは取り込まない(テーブルにしたものだけ)。
/// 長さが 2048 の倍数の素材はそのまま、それ以外は音の高さを見つけて `frames` 箇所で 1 周期ずつ切り出す
pub fn import_wavetable(
    project_dir: &Path,
    src: &Path,
    frames: usize,
) -> Result<ImportedTable, String> {
    let (mono, sample_rate) = decode_mono(src)?;
    let as_is = mono.len() >= 2048
        && mono.len() % 2048 == 0
        && mono.len() / 2048 <= glaux_dsp::MAX_USER_FRAMES;
    let cycles = glaux_dsp::cycles_from_audio(&mono, sample_rate, frames)?;
    let n = cycles.len() / 2048;
    // 鳴らせる形か先に確かめる(無音など)
    glaux_dsp::UserTable::from_cycles(&cycles).ok_or("ウェーブテーブルにできません(無音)")?;
    let tmp_dir = project_dir.join("cache");
    std::fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;
    let tmp = tmp_dir.join(format!("wavetable-{}.wav", std::process::id()));
    {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut w = hound::WavWriter::create(&tmp, spec).map_err(|e| e.to_string())?;
        for v in &cycles {
            w.write_sample(*v).map_err(|e| e.to_string())?;
        }
        w.finalize().map_err(|e| e.to_string())?;
    }
    let imported = import_wav(project_dir, &tmp);
    let _ = std::fs::remove_file(&tmp);
    Ok(ImportedTable {
        imported: imported?,
        frames: n,
        as_is,
    })
}

/// 一時ファイルに書いて確定させてから rename する(途中で落ちても最終名に壊れたファイルを残さない)。
fn write_file_atomic(dest: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let tmp = dest.with_extension(format!("tmp{}", std::process::id()));
    let written = std::fs::File::create(&tmp).and_then(|mut f| {
        f.write_all(bytes)?;
        f.sync_all()
    });
    if let Err(e) = written.and_then(|_| std::fs::rename(&tmp, dest)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("書き込めません({}): {e}", dest.display()));
    }
    Ok(())
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

    // 名前は中身のハッシュなので、同じ大きさのファイルがあれば同じもの。
    // 大きさが違うのは、以前の書き込みが途中で止まった壊れたファイル
    let already_present = std::fs::metadata(&dest).is_ok_and(|m| m.len() == bytes.len() as u64);
    if !already_present {
        std::fs::create_dir_all(project_dir.join("audio")).map_err(|e| e.to_string())?;
        write_file_atomic(&dest, &bytes)?;
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
    audio_clip_commands_with_offset(project, track_id, imported, clip_id, start, name, 0)
}

/// `offset_samples` ぶん波形の頭を飛ばして置く版(録音のカウントイン・レイテンシ補正)。
pub fn audio_clip_commands_with_offset(
    project: &Project,
    track_id: &TrackId,
    imported: &ImportedSample,
    clip_id: ClipId,
    start: Tick,
    name: &str,
    offset_samples: u64,
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
    let offset_samples = offset_samples.min(imported.asset.frames.saturating_sub(1));
    let secs =
        (imported.asset.frames - offset_samples) as f64 / imported.asset.sample_rate.max(1) as f64;
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
    let mut clip = Clip::new_audio(clip_id, name, start, length, imported.id.clone());
    if let glaux_core::ClipContent::Audio {
        offset_samples: off,
        ..
    } = &mut clip.content
    {
        *off = offset_samples;
    }
    cmds.push(Command::AddClip {
        track: track_id.clone(),
        clip,
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
    fn decodes_other_formats_via_symphonia() {
        // symphonia 経由の経路を、拡張子を偽った WAV で通す(形式はプローブで判定される)
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("take.wav");
        write_test_wav(&src);
        let disguised = tmp.path().join("take.flac");
        std::fs::copy(&src, &disguised).unwrap();
        let proj = tmp.path().join("Song.glaux");
        std::fs::create_dir_all(&proj).unwrap();

        let a = import_audio(&proj, &disguised).unwrap();
        assert_eq!(a.asset.frames, 4800);
        assert_eq!(a.asset.sample_rate, 48_000);
        assert!(a.asset.path.ends_with(".wav"));
        let r = hound::WavReader::open(proj.join(&a.asset.path)).unwrap();
        assert_eq!(r.spec().sample_format, hound::SampleFormat::Float);
        // 2 回目は変換済みファイルを再利用
        let b = import_audio(&proj, &disguised).unwrap();
        assert!(b.already_present);
        assert_eq!(b.asset.frames, 4800);
    }

    #[test]
    fn rejects_non_wav() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("not_audio.wav");
        std::fs::write(&src, b"hello").unwrap();
        assert!(import_wav(tmp.path(), &src).is_err());
    }

    #[test]
    fn audio_file_becomes_a_wavetable_asset() {
        let tmp = tempfile::tempdir().unwrap();
        // 220Hz のノコギリ波 1 秒(WAV、44.1kHz)
        let src = tmp.path().join("take.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&src, spec).unwrap();
        for i in 0..44_100 {
            let ph = (i as f32 * 220.0 / 44_100.0).fract();
            w.write_sample(((1.0 - 2.0 * ph) * 12_000.0) as i16)
                .unwrap();
        }
        w.finalize().unwrap();
        let t = import_wavetable(tmp.path(), &src, 8).unwrap();
        assert_eq!(t.frames, 8);
        assert!(!t.as_is);
        assert_eq!(t.imported.asset.frames, 8 * 2048);
        assert!(tmp.path().join(&t.imported.asset.path).exists());
        // 取り込んだテーブルをもう一度取り込むと、そのまま(2048 の倍数)
        let again =
            import_wavetable(tmp.path(), &tmp.path().join(&t.imported.asset.path), 16).unwrap();
        assert!(again.as_is);
        assert_eq!(again.frames, 8);
        assert_eq!(again.imported.id, t.imported.id, "同じ中身は同じ素材");
    }
}
