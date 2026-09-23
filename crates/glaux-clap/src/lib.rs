//! # glaux-clap
//!
//! CLAP プラグイン(MIT ライセンスのオープンなプラグイン規格)を Glaux から使うためのホスト。
//! バインディングは `clack-host`(MIT OR Apache-2.0)。
//!
//! - [`scan()`] … インストール済みプラグインを探して一覧にする
//! - [`ClapPlugin`] … インスタンスの生成・起動・状態の保存と復元(メインスレッド側)
//! - [`ClapProcessor`] … 音声処理(オーディオスレッド側。アロケーションなし)
//!
//! スレッドの約束(CLAP の仕様): `ClapPlugin` は作ったスレッドから動かさない。
//! `ClapProcessor` はオーディオスレッドへ渡して使い、止めるときはメインスレッドへ戻して
//! [`ClapPlugin::deactivate`] に渡す。

mod host;
mod plugin;
mod presets;
mod scan;
#[cfg(windows)]
mod window;

pub use host::{mark_audio_thread, mark_main_thread};
pub use plugin::{ClapPlugin, ClapProcessor, GuiEvent, NoteMsg, ParamInfo, MAX_EVENTS, MAX_FRAMES};
pub use presets::{list_presets, PresetEntry, PresetLocation};
pub use scan::{default_search_paths, describe, scan, PluginInfo};

/// プラグインの画面のためのウィンドウメッセージを処理する(プラグインのメインスレッドで
/// こまめに呼ぶ。Windows 以外では何もしない)。
pub fn pump_gui_events() {
    #[cfg(windows)]
    window::pump_messages();
}

use clack_host::prelude::PluginEntry;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, thiserror::Error)]
pub enum ClapError {
    #[error("プラグインを読み込めません: {0}")]
    Load(String),
    #[error("プラグインを起動できません: {0}")]
    Activate(String),
    #[error("プラグインの状態を扱えません: {0}")]
    State(String),
    #[error("プラグインの画面を開けません: {0}")]
    Gui(String),
}

/// 読み込んだ `.clap`(DLL)をプロセス内で使い回す(同じファイルを何度も開かない)。
fn load_entry(path: &Path) -> Result<PluginEntry, ClapError> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, PluginEntry>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(e) = map.get(path) {
        return Ok(e.clone());
    }
    // SAFETY: CLAP プラグインのエントリを読み込む(プラグインのコードを実行する)。
    // 読み込む対象はユーザーがインストールした CLAP ファイルに限る
    let entry = unsafe { PluginEntry::load(path) }
        .map_err(|e| ClapError::Load(format!("{}: {e}", path.display())))?;
    map.insert(path.to_owned(), entry.clone());
    Ok(entry)
}

#[cfg(test)]
mod tests;
