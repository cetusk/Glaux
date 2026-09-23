//! プラグインのプリセット(音色の保存データ)の一覧。CLAP の preset-discovery を使う。
//!
//! プラグインが「プリセットの置き場所(フォルダ)」と「ファイルの種類(拡張子)」を宣言し、
//! ホストがフォルダをたどってファイルごとにメタデータ(名前・作者など)を問い合わせる。
//! カテゴリはプラグインが返さないことがある(Surge XT は返さない)ので、置き場所からの
//! フォルダ名で補う。読み込みは [`crate::ClapPlugin::load_preset`]。

use clack_extensions::preset_discovery::indexer::IndexerImpl;
use clack_extensions::preset_discovery::metadata_receiver::MetadataReceiverImpl;
use clack_extensions::preset_discovery::prelude::PresetDiscoveryFactory;
use clack_extensions::preset_discovery::preset_data::{
    FileType, Flags, Location, LocationInfo, Soundpack,
};
use clack_extensions::preset_discovery::provider::Provider;
use clack_host::prelude::*;
use clack_host::utils::{Timestamp, UniversalPluginId};
use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};

/// プリセットの在りか。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PresetLocation {
    /// ファイル(`.fxp` など)
    File(PathBuf),
    /// プラグイン本体に入っているもの
    Plugin,
}

/// プリセット 1 つ分。
#[derive(Clone, Debug, PartialEq)]
pub struct PresetEntry {
    pub name: String,
    pub location: PresetLocation,
    /// ファイル内の位置など(プラグインが解釈する。無ければ None)
    pub load_key: Option<String>,
    /// 置き場所からのフォルダ名(例 `Pads`、`Leads/Mono`)。プラグインが返すカテゴリがあればそれ
    pub category: String,
    /// 置き場所の名前(例 `Surge XT Factory Presets`)
    pub collection: String,
    pub factory: bool,
    pub creators: Vec<String>,
    pub description: String,
    /// プラグインが付けたタグ(`bass` / `pad` など)
    pub features: Vec<String>,
}

impl PresetEntry {
    /// AI・UI が指定に使う識別子(ファイルならパス、本体内なら `plugin:<load_key>`)。
    pub fn id(&self) -> String {
        match &self.location {
            PresetLocation::File(p) => match &self.load_key {
                Some(k) if !k.is_empty() => format!("{}#{k}", p.display()),
                _ => p.display().to_string(),
            },
            PresetLocation::Plugin => format!("plugin:{}", self.load_key.as_deref().unwrap_or("")),
        }
    }

    /// [`id`](Self::id) から在りかと load_key に戻す。
    pub fn parse_id(id: &str) -> (PresetLocation, Option<String>) {
        if let Some(key) = id.strip_prefix("plugin:") {
            return (PresetLocation::Plugin, Some(key.to_owned()));
        }
        match id.rsplit_once('#') {
            // パスに # が含まれることもあるので、そのファイルが存在しない場合だけ分ける
            Some((p, k)) if !Path::new(id).exists() && Path::new(p).exists() => {
                (PresetLocation::File(PathBuf::from(p)), Some(k.to_owned()))
            }
            _ => (PresetLocation::File(PathBuf::from(id)), None),
        }
    }
}

#[derive(Default)]
struct Indexer {
    extensions: Vec<String>,
    locations: Vec<(String, bool, Option<PathBuf>)>,
}

impl IndexerImpl for Indexer {
    fn declare_filetype(&mut self, f: FileType) -> Result<(), HostError> {
        if let Some(ext) = f.file_extension {
            let ext = ext.to_string_lossy().trim_start_matches('.').to_lowercase();
            if !ext.is_empty() {
                self.extensions.push(ext);
            }
        }
        Ok(())
    }

    fn declare_location(&mut self, l: LocationInfo) -> Result<(), HostError> {
        let path = match l.location {
            Location::Plugin => None,
            Location::File { path } => Some(PathBuf::from(path.to_string_lossy().into_owned())),
        };
        self.locations.push((
            l.name.to_string_lossy().into_owned(),
            l.flags.contains(Flags::IS_FACTORY_CONTENT),
            path,
        ));
        Ok(())
    }

    fn declare_soundpack(&mut self, _s: Soundpack) -> Result<(), HostError> {
        Ok(())
    }
}

/// 1 ファイル(か本体)ぶんのメタデータを受け取る。
struct Receiver<'a> {
    plugin_id: &'a str,
    presets: Vec<Received>,
}

#[derive(Default)]
struct Received {
    name: String,
    load_key: Option<String>,
    plugin_ids: Vec<String>,
    creators: Vec<String>,
    description: String,
    features: Vec<String>,
    factory: bool,
}

impl MetadataReceiverImpl for Receiver<'_> {
    fn on_error(&mut self, _code: i32, msg: Option<&CStr>) {
        tracing::debug!("プリセットを読めません: {msg:?}");
    }

    fn begin_preset(&mut self, name: Option<&CStr>, key: Option<&CStr>) -> Result<(), HostError> {
        self.presets.push(Received {
            name: name
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            load_key: key
                .map(|k| k.to_string_lossy().into_owned())
                .filter(|k| !k.is_empty()),
            ..Default::default()
        });
        Ok(())
    }

    fn add_plugin_id(&mut self, id: UniversalPluginId) {
        if let Some(p) = self.presets.last_mut() {
            p.plugin_ids.push(id.id.to_string_lossy().into_owned());
        }
    }

    fn set_soundpack_id(&mut self, _id: &CStr) {}

    fn set_flags(&mut self, f: Flags) {
        if let Some(p) = self.presets.last_mut() {
            p.factory = f.contains(Flags::IS_FACTORY_CONTENT);
        }
    }

    fn add_creator(&mut self, c: &CStr) {
        if let Some(p) = self.presets.last_mut() {
            p.creators.push(c.to_string_lossy().into_owned());
        }
    }

    fn set_description(&mut self, d: &CStr) {
        if let Some(p) = self.presets.last_mut() {
            p.description = d.to_string_lossy().into_owned();
        }
    }

    fn set_timestamps(&mut self, _c: Option<Timestamp>, _m: Option<Timestamp>) {}

    fn add_feature(&mut self, f: &CStr) {
        if let Some(p) = self.presets.last_mut() {
            p.features.push(f.to_string_lossy().into_owned());
        }
    }

    fn add_extra_info(&mut self, _k: &CStr, _v: &CStr) {}
}

impl Receiver<'_> {
    /// このプラグイン向けのもの(対象の指定が無いものも含む)だけを残す
    fn take_for_plugin(&mut self) -> Vec<Received> {
        let id = self.plugin_id;
        std::mem::take(&mut self.presets)
            .into_iter()
            .filter(|p| p.plugin_ids.is_empty() || p.plugin_ids.iter().any(|x| x == id))
            .collect()
    }
}

/// フォルダを再帰的にたどって、指定の拡張子のファイルを集める(深さ・数に上限)。
fn crawl(dir: &Path, exts: &[String], depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 8 || out.len() >= 20_000 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            crawl(&path, exts, depth + 1, out);
        } else if path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .is_some_and(|e| exts.is_empty() || exts.contains(&e))
        {
            out.push(path);
        }
    }
}

/// `plugin_path` の `.clap` に入っている `plugin_id` のプラグインのプリセット一覧。
/// プリセットの一覧に対応していなければ空。
pub fn list_presets(
    plugin_path: &Path,
    plugin_id: &str,
) -> Result<Vec<PresetEntry>, crate::ClapError> {
    let entry = crate::load_entry(plugin_path)?;
    let Some(factory) = entry.get_factory::<PresetDiscoveryFactory>() else {
        return Ok(vec![]);
    };
    let host = HostInfo::new(
        "Glaux",
        "Glaux",
        "https://github.com/cetusk/Glaux",
        env!("CARGO_PKG_VERSION"),
    )
    .map_err(|e| crate::ClapError::Load(e.to_string()))?;
    let provider_ids: Vec<CString> = factory
        .provider_descriptors()
        .filter_map(|d| d.id().map(|i| i.to_owned()))
        .collect();
    let mut out = Vec::new();
    for pid in provider_ids {
        let mut provider = match Provider::instantiate(Indexer::default(), &entry, &pid, &host) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("プリセットの一覧を作れません: {e:?}");
                continue;
            }
        };
        let exts = provider.indexer().extensions.clone();
        let locations = provider.indexer().locations.clone();
        for (collection, factory_loc, root) in locations {
            let mut receiver = Receiver {
                plugin_id,
                presets: Vec::new(),
            };
            match root {
                None => {
                    provider.get_metadata(Location::Plugin, &mut receiver);
                    for r in receiver.take_for_plugin() {
                        out.push(to_entry(
                            r,
                            PresetLocation::Plugin,
                            String::new(),
                            &collection,
                            factory_loc,
                        ));
                    }
                }
                Some(root) => {
                    let mut files = Vec::new();
                    if root.is_file() {
                        files.push(root.clone());
                    } else {
                        crawl(&root, &exts, 0, &mut files);
                    }
                    for file in files {
                        let Ok(c) = CString::new(file.to_string_lossy().as_bytes()) else {
                            continue;
                        };
                        provider.get_metadata(Location::File { path: &c }, &mut receiver);
                        let category = file
                            .parent()
                            .and_then(|p| p.strip_prefix(&root).ok())
                            .map(|p| p.to_string_lossy().replace('\\', "/"))
                            .unwrap_or_default();
                        for r in receiver.take_for_plugin() {
                            out.push(to_entry(
                                r,
                                PresetLocation::File(file.clone()),
                                category.clone(),
                                &collection,
                                factory_loc,
                            ));
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

fn to_entry(
    r: Received,
    location: PresetLocation,
    category: String,
    collection: &str,
    factory_loc: bool,
) -> PresetEntry {
    PresetEntry {
        name: r.name,
        location,
        load_key: r.load_key,
        category,
        collection: collection.to_owned(),
        factory: r.factory || factory_loc,
        creators: r.creators,
        description: r.description,
        features: r.features,
    }
}
