//! インストール済みの CLAP プラグインを探す。
//!
//! 探す場所は CLAP 仕様の標準パス(`CLAP_PATH` 環境変数 → OS ごとの既定フォルダ)と、
//! 呼び出し側が足すフォルダ。`.clap` ファイルを再帰的に探し、中の記述子を読んで一覧にする。
//! 読み込みはプラグインのコード(DLL)を実行するので、壊れたプラグインがあると
//! 巻き込まれうる(将来は別プロセスでスキャンする)。

use std::path::{Path, PathBuf};

/// 見つかったプラグイン 1 つ分。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginInfo {
    /// プラグイン ID(例 `org.surge-synth-team.surge-xt`)。プロジェクトにはこれを保存する
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    /// `.clap` ファイルの場所
    pub path: PathBuf,
    /// CLAP の特徴タグ(`instrument` / `audio-effect` / `synthesizer` など)
    pub features: Vec<String>,
}

impl PluginInfo {
    /// 音源(ノートを受けて音を出す)か
    pub fn is_instrument(&self) -> bool {
        self.features.iter().any(|f| f == "instrument")
    }

    /// エフェクト(音声を受けて加工する)か
    pub fn is_effect(&self) -> bool {
        self.features.iter().any(|f| f == "audio-effect")
    }
}

/// CLAP 仕様の標準の探し場所。
pub fn default_search_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(p) = std::env::var_os("CLAP_PATH") {
        out.extend(std::env::split_paths(&p));
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(common) = std::env::var_os("COMMONPROGRAMFILES") {
            out.push(PathBuf::from(common).join("CLAP"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            out.push(
                PathBuf::from(local)
                    .join("Programs")
                    .join("Common")
                    .join("CLAP"),
            );
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            out.push(PathBuf::from(home).join("Library/Audio/Plug-Ins/CLAP"));
        }
        out.push(PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(home) = std::env::var_os("HOME") {
            out.push(PathBuf::from(home).join(".clap"));
        }
        out.push(PathBuf::from("/usr/lib/clap"));
    }
    out
}

/// `.clap` ファイル(macOS ではバンドルのフォルダ)を再帰的に集める。
fn collect_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 6 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let path = e.path();
        let is_clap = path
            .extension()
            .is_some_and(|x| x.eq_ignore_ascii_case("clap"));
        if is_clap {
            out.push(path);
        } else if path.is_dir() {
            collect_files(&path, depth + 1, out);
        }
    }
}

/// `.clap` ファイル 1 つに入っているプラグインの一覧。
pub fn describe(path: &Path) -> Result<Vec<PluginInfo>, crate::ClapError> {
    let entry = crate::load_entry(path)?;
    let factory = entry.get_plugin_factory().ok_or_else(|| {
        crate::ClapError::Load(format!("{}: plugin factory がありません", path.display()))
    })?;
    let text = |c: Option<&std::ffi::CStr>| {
        c.map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    Ok(factory
        .plugin_descriptors()
        .filter_map(|d| {
            let id = text(d.id());
            (!id.is_empty()).then(|| PluginInfo {
                id,
                name: text(d.name()),
                vendor: text(d.vendor()),
                version: text(d.version()),
                path: path.to_owned(),
                features: d
                    .features()
                    .map(|f| f.to_string_lossy().into_owned())
                    .collect(),
            })
        })
        .collect())
}

/// 指定フォルダ群からプラグインを探す(読めないファイルは飛ばしてログに残す)。
/// 同じ ID が複数あれば先に見つかった方を使う。
pub fn scan(dirs: &[PathBuf]) -> Vec<PluginInfo> {
    let mut files = Vec::new();
    for d in dirs {
        collect_files(d, 0, &mut files);
    }
    let mut out: Vec<PluginInfo> = Vec::new();
    for f in files {
        match describe(&f) {
            Ok(list) => {
                for p in list {
                    if !out.iter().any(|q| q.id == p.id) {
                        out.push(p);
                    }
                }
            }
            Err(e) => tracing::warn!("CLAP を読めません: {e}"),
        }
    }
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}
