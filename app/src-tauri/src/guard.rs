//! 画面からのコマンドの panic を受け止める仕組みと、panic の記録。
//!
//! 同期の `#[tauri::command] fn` は、Windows では WebView2 のコールバックの中(画面のスレッド)で動く。
//! そこで panic すると unwind できず「panic in a function that cannot unwind」でアプリごと落ちる
//! (音色エディタの鍵盤で起きた、スペクトルの桁あふれ)。コマンドの受け口を 1 か所で包んで受け止め、
//! 画面へはエラーとして返す。async のコマンドは tokio の中で動くので、ここを通る前に spawn されて
//! アプリは落ちない(その invoke は返らない)。

use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use tauri::ipc::Invoke;
use tauri::Runtime;

/// `invoke_handler` を包む。panic したコマンドは「内部エラー」で返し、アプリは動き続ける
pub fn guarded<R: Runtime>(
    handler: impl Fn(Invoke<R>) -> bool + Send + Sync + 'static,
) -> impl Fn(Invoke<R>) -> bool + Send + Sync + 'static {
    move |invoke| {
        let cmd = invoke.message.command().to_owned();
        let resolver = invoke.resolver.clone();
        match catch_unwind(AssertUnwindSafe(|| handler(invoke))) {
            Ok(handled) => handled,
            Err(p) => {
                tracing::error!("コマンド {cmd} で panic: {}", panic_message(&*p));
                // もう返事をしていた場合は reject 自体が panic するので、それも受け止める
                let _ = catch_unwind(AssertUnwindSafe(|| {
                    resolver.reject(format!("内部エラー({cmd})。ログ: {}", log_path().display()))
                }));
                true
            }
        }
    }
}

fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(不明)".to_owned())
}

/// panic の記録(`<設定フォルダ>/glaux/logs/panic.log`)
pub fn log_path() -> std::path::PathBuf {
    crate::projects::config_dir().join("logs").join("panic.log")
}

/// どのスレッドの panic も、時刻・場所・中身・呼び出しの流れをファイルに残す(release では標準エラーが見えないため)。
/// 既定の表示(標準エラー)もそのまま出す
pub fn install_panic_log() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        prev(info);
        let path = log_path();
        let _ = (|| -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            // 大きくなりすぎたら 1 世代だけ残して作り直す
            if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) {
                let _ = std::fs::rename(&path, path.with_extension("old.log"));
            }
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)?;
            writeln!(
                f,
                "---- {} [{}] {}\n{}",
                chrono::Local::now().to_rfc3339(),
                std::thread::current().name().unwrap_or("?"),
                env!("CARGO_PKG_VERSION"),
                info
            )?;
            writeln!(f, "{}", std::backtrace::Backtrace::force_capture())
        })();
    }));
}
