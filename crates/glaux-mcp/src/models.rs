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
    if path.is_file() {
        return Ok(path);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("フォルダを作れません({}): {e}", dir.display()))?;
    }
    let part = path.with_extension("onnx.part");
    let mut res = ureq::get(MODEL_URL)
        .call()
        .map_err(|e| format!("モデルを取得できません: {e}"))?;
    let total = res
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(MODEL_BYTES);
    let mut reader = res.body_mut().with_config().limit(MODEL_BYTES * 2).reader();
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
    if hash != MODEL_SHA256 {
        let _ = std::fs::remove_file(&part);
        return Err(format!(
            "取得したモデルが壊れています(SHA-256 が一致しません: {hash})"
        ));
    }
    std::fs::rename(&part, &path).map_err(|e| format!("置き換えられません: {e}"))?;
    Ok(path)
}
