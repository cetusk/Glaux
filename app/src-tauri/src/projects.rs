//! 最近使ったプロジェクトの記録。
//!
//! 保存先は OS 標準の設定ディレクトリ配下の `glaux/recent.json`
//! (Windows: `%APPDATA%\glaux`、他: `$XDG_CONFIG_HOME` または `~/.config`)。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const MAX_RECENT: usize = 15;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecentProject {
    pub path: String,
    pub title: String,
    /// RFC3339
    pub last_opened: String,
}

fn config_dir() -> PathBuf {
    let base = std::env::var("APPDATA")
        .or_else(|_| std::env::var("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("USERPROFILE")
                .or_else(|_| std::env::var("HOME"))
                .unwrap_or_else(|_| ".".to_owned());
            PathBuf::from(home).join(".config")
        });
    base.join("glaux")
}

fn recent_file() -> PathBuf {
    config_dir().join("recent.json")
}

pub fn load_recent() -> Vec<RecentProject> {
    std::fs::read_to_string(recent_file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// プロジェクトを開いた記録を残す(先頭に移動、上限 15 件)。
pub fn push_recent(path: &str, title: &str) {
    let mut list = load_recent();
    list.retain(|r| r.path != path);
    list.insert(
        0,
        RecentProject {
            path: path.to_owned(),
            title: title.to_owned(),
            last_opened: chrono::Local::now().to_rfc3339(),
        },
    );
    list.truncate(MAX_RECENT);
    let dir = config_dir();
    if std::fs::create_dir_all(&dir).is_ok() {
        if let Ok(json) = serde_json::to_string_pretty(&list) {
            if let Err(e) = std::fs::write(recent_file(), json) {
                tracing::warn!("recent.json を保存できません: {e}");
            }
        }
    }
}

/// 新規プロジェクトの既定の親フォルダ(`<ホーム>/Music/Glaux`)。
pub fn default_projects_dir() -> String {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_owned());
    PathBuf::from(home)
        .join("Music")
        .join("Glaux")
        .to_string_lossy()
        .into_owned()
}
