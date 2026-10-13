//! 動作の記録(ログ)。標準エラーと `<設定フォルダ>/glaux/logs/glaux.log` の両方に書く
//! (配る版は窓を出さず標準エラーが見えないため、ファイルにも残す)。
//!
//! 記録の詳しさは設定の「一般 → 記録の詳しさ」で選び、`logs/level.txt` に残して次の起動でも使う。
//! 既定は「警告まで」(部品のつなぎの記録などは出さない)。環境変数 `GLAUX_LOG` があればそちらを優先する
//! (`GLAUX_LOG=info,rmcp=warn` のように細かく書ける。開発・調べもの用)。

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use tracing_subscriber::prelude::*;
use tracing_subscriber::{fmt, reload, EnvFilter, Registry};

/// 設定で選べる記録の詳しさ(詳しくない順)
pub const LEVELS: [&str; 4] = ["error", "warn", "info", "debug"];
/// 既定: 警告まで
pub const DEFAULT_LEVEL: &str = "warn";

static HANDLE: OnceLock<reload::Handle<EnvFilter, Registry>> = OnceLock::new();

/// 記録のファイル
pub fn log_path() -> PathBuf {
    crate::projects::config_dir().join("logs").join("glaux.log")
}

fn level_path() -> PathBuf {
    crate::projects::config_dir().join("logs").join("level.txt")
}

/// 詳しさの名前を、記録を選ぶ条件にする。「詳しく」は Glaux の部品だけ debug にする
/// (ほかの部品まで debug にすると、窓・通信の記録で埋まって読めない)
fn directives(level: &str) -> String {
    match level {
        "debug" => [
            "info",
            "glaux_app=debug",
            "glaux_core=debug",
            "glaux_mcp=debug",
            "glaux_engine=debug",
            "glaux_dsp=debug",
            "glaux_ml=debug",
            "glaux_clap=debug",
        ]
        .join(","),
        l if LEVELS.contains(&l) => l.to_owned(),
        _ => DEFAULT_LEVEL.to_owned(),
    }
}

/// 残してある詳しさ(無い・読めない・知らない名前なら既定)
pub fn saved_level() -> String {
    std::fs::read_to_string(level_path())
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| LEVELS.contains(&s.as_str()))
        .unwrap_or_else(|| DEFAULT_LEVEL.to_owned())
}

/// 環境変数 `GLAUX_LOG` で詳しさを決めているとき、その中身(設定の画面で「こちらが優先」と見せる)
pub fn env_override() -> Option<String> {
    std::env::var("GLAUX_LOG")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

/// 記録を始める(起動の最初に 1 回)
pub fn init() {
    let filter = env_override()
        .and_then(|s| EnvFilter::try_new(s).ok())
        .unwrap_or_else(|| EnvFilter::new(directives(&saved_level())));
    let (filter, handle) = reload::Layer::new(filter);
    let _ = HANDLE.set(handle);
    let file = open_log_file().map(|f| fmt::layer().with_ansi(false).with_writer(Mutex::new(f)));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr))
        .with(file)
        .init();
}

/// 記録のファイルを開く(大きくなりすぎたら 1 世代だけ残して作り直す)。開けなければ標準エラーだけにする
fn open_log_file() -> Option<std::fs::File> {
    let path = log_path();
    std::fs::create_dir_all(path.parent()?).ok()?;
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) {
        let _ = std::fs::rename(&path, path.with_extension("old.log"));
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()
}

/// 詳しさを変えて残す(すぐ効く)。`GLAUX_LOG` があるときは残すだけで、今の記録はそちらのまま
pub fn set_level(level: &str) -> Result<(), String> {
    if !LEVELS.contains(&level) {
        return Err(format!("知らない詳しさです: {level}"));
    }
    let path = level_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, level).map_err(|e| e.to_string())?;
    if env_override().is_none() {
        if let Some(h) = HANDLE.get() {
            h.reload(EnvFilter::new(directives(level)))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_level_makes_a_valid_filter() {
        for l in LEVELS {
            assert!(EnvFilter::try_new(directives(l)).is_ok(), "{l}");
        }
        // 知らない名前は既定に倒す
        assert_eq!(directives("loud"), DEFAULT_LEVEL);
        assert_eq!(directives("warn"), "warn");
    }
}
