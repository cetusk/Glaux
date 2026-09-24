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

/// 記録からパスを取り除く(プロジェクト移動後の旧パス掃除に使う)。
pub fn remove_recent(path: &str) {
    let mut list = load_recent();
    let before = list.len();
    list.retain(|r| r.path != path);
    if list.len() != before {
        if let Ok(json) = serde_json::to_string_pretty(&list) {
            let _ = std::fs::write(recent_file(), json);
        }
    }
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

fn settings_file() -> PathBuf {
    config_dir().join("settings.json")
}

#[derive(Default, Serialize, Deserialize)]
struct AppSettings {
    /// ユーザーが選んだ既定の作業(プロジェクト作成)フォルダ
    #[serde(default, skip_serializing_if = "Option::is_none")]
    projects_dir: Option<String>,
}

fn load_settings() -> AppSettings {
    std::fs::read_to_string(settings_file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_settings(s: &AppSettings) -> Result<(), String> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(settings_file(), json).map_err(|e| e.to_string())
}

/// 既定の作業フォルダを保存する(存在しないフォルダはエラー)。
pub fn set_projects_dir(path: &str) -> Result<(), String> {
    if !std::path::Path::new(path).is_dir() {
        return Err(format!("フォルダが見つかりません: {path}"));
    }
    let mut s = load_settings();
    s.projects_dir = Some(path.to_owned());
    save_settings(&s)
}

/// 新規プロジェクトの既定の親フォルダ。
/// ユーザーが保存した作業フォルダがあればそれを、なければ `<ホーム>/Music/Glaux`。
pub fn default_projects_dir() -> String {
    if let Some(dir) = load_settings().projects_dir {
        if std::path::Path::new(&dir).is_dir() {
            return dir;
        }
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_owned());
    PathBuf::from(home)
        .join("Music")
        .join("Glaux")
        .to_string_lossy()
        .into_owned()
}

/// フォルダの中で見つかった Glaux の曲。
#[derive(Clone, Debug, Serialize)]
pub struct FoundProject {
    pub path: String,
    pub title: String,
}

fn project_title(dir: &std::path::Path) -> Option<String> {
    let json = std::fs::read_to_string(dir.join("project.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&json).ok()?;
    Some(
        v.pointer("/meta/title")
            .and_then(|t| t.as_str())
            .unwrap_or("(無題)")
            .to_owned(),
    )
}

/// `dir` が Glaux の曲ならそれ 1 つ、そうでなければ中(2 段下まで)にある曲の一覧(パスの大小文字を区別しない順、最大 50)。
/// 「曲をまとめたフォルダ」(ゲームの songs/ など)を選んだときに、どの曲を開くか選べるようにする。
pub fn find_projects(dir: &str) -> Vec<FoundProject> {
    let root = std::path::Path::new(dir);
    if let Some(title) = project_title(root) {
        return vec![FoundProject {
            path: dir.to_owned(),
            title,
        }];
    }
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0u8)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            if let Some(title) = project_title(&p) {
                out.push(FoundProject {
                    path: p.to_string_lossy().into_owned(),
                    title,
                });
            } else if depth < 1 {
                stack.push((p, depth + 1));
            }
        }
        if out.len() >= 50 {
            break;
        }
    }
    out.sort_by_key(|f| f.path.to_lowercase());
    out.truncate(50);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_songs_inside_a_folder() {
        let tmp = std::env::temp_dir().join(format!("glaux-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let song = |rel: &str, title: &str| {
            let d = tmp.join(rel);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(
                d.join("project.json"),
                format!(r#"{{"meta":{{"title":"{title}"}}}}"#),
            )
            .unwrap();
        };
        song("songs/Stage1.glaux", "Stage 1");
        song("songs/boss/Boss.glaux", "Boss");
        song("songs/a/b/TooDeep.glaux", "Deep");
        std::fs::create_dir_all(tmp.join("songs/empty")).unwrap();
        let songs = tmp.join("songs");
        let found = find_projects(&songs.to_string_lossy());
        let titles: Vec<&str> = found.iter().map(|f| f.title.as_str()).collect();
        assert_eq!(titles, ["Boss", "Stage 1"], "2 段下まで・パス順");
        // 曲そのものを選んだらそれだけ
        let one = find_projects(&songs.join("Stage1.glaux").to_string_lossy());
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].title, "Stage 1");
        assert!(find_projects(&tmp.join("songs/empty").to_string_lossy()).is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
