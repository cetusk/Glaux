//! 音声アセットの取り込み。
//!
//! WAV を内容ハッシュ(sha256)名でプロジェクトの `audio/` にコピーし、
//! `Asset` メタデータを返す。同じ内容は同じ ID になるので重複コピーしない。

use glaux_core::{Asset, AssetId};
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

#[cfg(test)]
mod tests {
    use super::*;

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
