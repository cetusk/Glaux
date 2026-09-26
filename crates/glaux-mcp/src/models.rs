//! 同梱しない学習済みモデル(大きいもの)の取得。今は CLAP の音声側(約 280MB)だけ。

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::PathBuf;

/// CLAP の音声側モデルの状態。
#[derive(Clone, Debug, serde::Serialize)]
pub struct ModelStatus {
    pub available: bool,
    pub path: String,
    /// 取得する大きさ(バイト)
    pub bytes: u64,
}

pub fn clap_status() -> ModelStatus {
    let path = glaux_ml::clap::model_path();
    ModelStatus {
        available: path.is_file(),
        path: path.to_string_lossy().into_owned(),
        bytes: glaux_ml::clap::MODEL_BYTES,
    }
}

/// CLAP の音声側モデルを取得する(すでにあれば何もしない)。`progress(受信済み, 全体)` を 1MB ごとに呼ぶ。
/// 一時ファイルに書いて SHA-256 を確かめてから置き換える。
pub fn download_clap(progress: &mut dyn FnMut(u64, u64)) -> Result<PathBuf, String> {
    use glaux_ml::clap::{MODEL_BYTES, MODEL_SHA256, MODEL_URL};
    let path = glaux_ml::clap::model_path();
    download_verified(MODEL_URL, &path, MODEL_BYTES, MODEL_SHA256, progress)
        .map_err(|e| format!("モデルを取得できません: {e}"))?;
    Ok(path)
}

// ---- SoundFont(GM 音源一式) ----

/// 初回の案内で取得する SoundFont(GeneralUser GS v2.0.3。S. Christian Collins 作、GeneralUser GS License v2.0:
/// 音楽制作に私用・商用とも自由に使え、ソフトウェアに組み込んでよい)。作者の求めに従い、作者の配布ファイルへ
/// 直接つながず、Glaux のリリースに置いた写しから取得する
pub const SOUNDFONT_URL: &str =
    "https://github.com/cetusk/Glaux/releases/download/soundfonts/GeneralUser-GS-v2.0.3.sf2";
pub const SOUNDFONT_FILE: &str = "GeneralUser-GS.sf2";
pub const SOUNDFONT_BYTES: u64 = 32_319_396;
pub const SOUNDFONT_SHA256: &str =
    "9575028c7a1f589f5770fccc8cff2734566af40cd26ed836944e9a5152688cfe";

/// SoundFont のライブラリの状態(入っている .sf2 と、取得できるもの)。
#[derive(Clone, Debug, serde::Serialize)]
pub struct SoundFontStatus {
    pub dir: String,
    pub files: Vec<String>,
    /// 取得できる SoundFont のファイル名と大きさ(バイト)
    pub download_file: String,
    pub download_bytes: u64,
}

pub fn soundfont_status() -> SoundFontStatus {
    let dir = glaux_engine::sf2::default_dir();
    SoundFontStatus {
        files: glaux_engine::sf2::list_files(&dir),
        dir: dir.to_string_lossy().into_owned(),
        download_file: SOUNDFONT_FILE.to_owned(),
        download_bytes: SOUNDFONT_BYTES,
    }
}

/// GM 音源一式の SoundFont をライブラリへ取得する(すでにあれば何もしない)。
pub fn download_soundfont(progress: &mut dyn FnMut(u64, u64)) -> Result<PathBuf, String> {
    let path = glaux_engine::sf2::default_dir().join(SOUNDFONT_FILE);
    download_verified(
        SOUNDFONT_URL,
        &path,
        SOUNDFONT_BYTES,
        SOUNDFONT_SHA256,
        progress,
    )
    .map_err(|e| format!("SoundFont を取得できません: {e}"))?;
    Ok(path)
}

/// `url` を `path` へ取得する(すでにあれば何もしない)。一時ファイル(`.part`)に書き、
/// SHA-256 が `sha256` と一致したら置き換える。`progress(受信済み, 全体)` を 1MB ごとに呼ぶ
fn download_verified(
    url: &str,
    path: &std::path::Path,
    bytes: u64,
    sha256: &str,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<(), String> {
    if path.is_file() {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("フォルダを作れません({}): {e}", dir.display()))?;
    }
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let mut res = ureq::get(url).call().map_err(|e| e.to_string())?;
    let total = res
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(bytes);
    let mut reader = res.body_mut().with_config().limit(bytes * 2).reader();
    let mut file = std::fs::File::create(&part)
        .map_err(|e| format!("書き込めません({}): {e}", part.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let (mut got, mut last) = (0u64, 0u64);
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("取得が途中で止まりました: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])
            .map_err(|e| format!("書き込めません: {e}"))?;
        got += n as u64;
        if got - last >= 1 << 20 {
            last = got;
            progress(got, total);
        }
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);
    progress(got, total);
    let hash: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if hash != sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(format!(
            "取得したファイルが壊れています(SHA-256 が一致しません: {hash})"
        ));
    }
    std::fs::rename(&part, path).map_err(|e| format!("置き換えられません: {e}"))?;
    Ok(())
}
