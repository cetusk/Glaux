//! CLAP プラグインのプリセット(音色の保存データ)の一覧と読み込み。UI(Tauri)と MCP が共用する。
//!
//! 読み込みは「プロジェクトの状態 → プリセット」を一時的なプラグインで適用して新しい状態を作り、
//! 音源は `set_device` 1 件、エフェクトは `set_effect_state` と上書き値の削除のまとめ 1 件で書く
//! (取り消しで元の音色に戻る)。再生中のプラグインは同期で状態を読み込み直す。
//! プリセットは音色一式なので、CLAP パラメータの上書き値(`device/clap:<id>` / `fx/<id>/clap:<id>`)は消す。
//! 読み込んだプリセット名は `params.preset` に残す。

use glaux_core::{Command, ParamPath, ParamValue, PluginSource, Project, TrackId};
use glaux_engine::plugins::PluginOwner;
use serde_json::{json, Value};

/// device.params に残すプリセット名のキー
pub const PRESET_NAME_KEY: &str = "preset";

pub(crate) fn clap_track<'a>(
    project: &'a Project,
    track_id: &TrackId,
) -> Result<
    (
        &'a glaux_core::Track,
        &'a glaux_core::Device,
        &'a str,
        Option<&'a str>,
    ),
    String,
> {
    let track = project
        .track(track_id)
        .ok_or_else(|| format!("トラックが見つかりません: {track_id}"))?;
    let device = track
        .device
        .as_ref()
        .ok_or_else(|| format!("「{}」に音源がありません", track.name))?;
    let PluginSource::Clap { plugin_id, state } = &device.source else {
        return Err(format!(
            "「{}」の音源は CLAP プラグインではありません",
            track.name
        ));
    };
    Ok((track, device, plugin_id, state.as_deref()))
}

/// CLAP プラグインの載っている所の情報。
pub(crate) struct ClapSite<'a> {
    pub plugin_id: &'a str,
    pub state: Option<&'a str>,
    pub params: &'a glaux_core::ParamMap,
    /// 表示用の場所(トラック名 / 「マスター」)
    pub place: String,
    /// エフェクトならそのトラック(マスターのエフェクトは None)
    pub fx_track: Option<TrackId>,
}

/// 音源(トラック)またはエフェクトの CLAP プラグインを引く。
pub(crate) fn clap_site<'a>(
    project: &'a Project,
    owner: &PluginOwner,
) -> Result<ClapSite<'a>, String> {
    match owner {
        PluginOwner::Track(tid) => {
            let (track, device, plugin_id, state) = clap_track(project, tid)?;
            Ok(ClapSite {
                plugin_id,
                state,
                params: &device.params,
                place: track.name.clone(),
                fx_track: None,
            })
        }
        PluginOwner::Effect(fx) => {
            let (effect, place, fx_track) =
                match project.master.effects.iter().find(|e| &e.id == fx) {
                    Some(e) => (e, "マスター".to_owned(), None),
                    None => project
                        .tracks
                        .iter()
                        .find_map(|t| {
                            t.effects
                                .iter()
                                .find(|e| &e.id == fx)
                                .map(|e| (e, t.name.clone(), Some(t.id.clone())))
                        })
                        .ok_or_else(|| format!("エフェクトが見つかりません: {fx}"))?,
                };
            let PluginSource::Clap { plugin_id, state } = &effect.source else {
                return Err(format!("{fx} は CLAP プラグインのエフェクトではありません"));
            };
            Ok(ClapSite {
                plugin_id,
                state: state.as_deref(),
                params: &effect.params,
                place,
                fx_track,
            })
        }
    }
}

fn preset_json(p: &glaux_clap::PresetEntry) -> Value {
    json!({
        "id": p.id(),
        "name": p.name,
        "category": p.category,
        "collection": p.collection,
        "factory": p.factory,
        "creators": p.creators,
        "description": p.description,
        "features": p.features,
    })
}

/// トラックのプラグインのプリセット一覧。`filter` は名前・カテゴリ・作者・タグの部分一致、
/// `category` はカテゴリの前方一致(大文字小文字を区別しない)。
pub fn list(
    project: &Project,
    owner: &PluginOwner,
    filter: Option<&str>,
    category: Option<&str>,
    limit: usize,
    rescan: bool,
) -> Result<Value, String> {
    let site = clap_site(project, owner)?;
    let plugin_id = site.plugin_id;
    let all = glaux_engine::plugins::presets(plugin_id, rescan)?;
    let needle = filter.map(str::to_lowercase).filter(|s| !s.is_empty());
    let cat = category.map(str::to_lowercase).filter(|s| !s.is_empty());
    let matched: Vec<&glaux_clap::PresetEntry> = all
        .iter()
        .filter(|p| {
            cat.as_ref()
                .is_none_or(|c| p.category.to_lowercase().starts_with(c.as_str()))
        })
        .filter(|p| {
            needle.as_ref().is_none_or(|n| {
                p.name.to_lowercase().contains(n.as_str())
                    || p.category.to_lowercase().contains(n.as_str())
                    || p.creators
                        .iter()
                        .any(|c| c.to_lowercase().contains(n.as_str()))
                    || p.features
                        .iter()
                        .any(|f| f.to_lowercase().contains(n.as_str()))
            })
        })
        .collect();
    // カテゴリごとの件数(絞り込みの手がかり)
    let mut categories: Vec<(String, usize)> = Vec::new();
    for p in all.iter() {
        match categories.iter_mut().find(|(c, _)| *c == p.category) {
            Some((_, n)) => *n += 1,
            None => categories.push((p.category.clone(), 1)),
        }
    }
    categories.sort();
    Ok(json!({
        "plugin_id": plugin_id,
        "current_preset": site.params.get(PRESET_NAME_KEY).and_then(|v| match v {
            ParamValue::Enum(s) => Some(s.clone()),
            _ => None,
        }),
        "total": all.len(),
        "matched": matched.len(),
        "categories": categories
            .iter()
            .map(|(c, n)| json!({ "name": c, "count": n }))
            .collect::<Vec<_>>(),
        "presets": matched.into_iter().take(limit).map(preset_json).collect::<Vec<_>>(),
    }))
}

/// プリセットを読み込むコマンドと、履歴のラベル・プリセット名。
/// 一時的にプラグインを作るので、呼んだスレッドをしばらく占有する(別スレッドで呼ぶこと)。
pub fn load_command(
    project: &Project,
    owner: &PluginOwner,
    preset_id: &str,
) -> Result<(Command, String, String), String> {
    let site = clap_site(project, owner)?;
    let plugin_id = site.plugin_id;
    let name = glaux_engine::plugins::presets(plugin_id, false)
        .ok()
        .and_then(|l| {
            l.iter()
                .find(|p| p.id() == preset_id)
                .map(|p| p.name.clone())
        })
        .unwrap_or_else(|| {
            std::path::Path::new(preset_id.split('#').next().unwrap_or(preset_id))
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| preset_id.to_owned())
        });
    let new_state = glaux_engine::plugins::state_with_preset(plugin_id, site.state, preset_id)?;
    let label = format!("{} の音色をプリセット「{name}」に", site.place);
    let command = match owner {
        PluginOwner::Track(tid) => {
            let (_, device, _, _) = clap_track(project, tid)?;
            let mut device = device.clone();
            device.source = PluginSource::Clap {
                plugin_id: plugin_id.to_owned(),
                state: Some(new_state),
            };
            // プリセットは音色一式: つまみの上書き値は消し、読み込んだ名前を残す
            device
                .params
                .retain(|k, _| glaux_engine::plugins::parse_param_key(k).is_none());
            device
                .params
                .insert(PRESET_NAME_KEY.to_owned(), ParamValue::Enum(name.clone()));
            Command::SetDevice {
                track: tid.clone(),
                device: Some(device),
            }
        }
        PluginOwner::Effect(fx) => {
            let mut cmds = vec![Command::SetEffectState {
                id: fx.clone(),
                state: Some(new_state),
            }];
            let path = |k: &str| ParamPath::effect(fx.clone(), k);
            for k in site
                .params
                .keys()
                .filter(|k| glaux_engine::plugins::parse_param_key(k).is_some())
            {
                cmds.push(match &site.fx_track {
                    Some(t) => Command::UnsetParam {
                        track: t.clone(),
                        path: path(k),
                    },
                    None => Command::UnsetMasterParam { path: path(k) },
                });
            }
            let value = ParamValue::Enum(name.clone());
            cmds.push(match &site.fx_track {
                Some(t) => Command::SetParam {
                    track: t.clone(),
                    path: path(PRESET_NAME_KEY),
                    value,
                },
                None => Command::SetMasterParam {
                    path: path(PRESET_NAME_KEY),
                    value,
                },
            });
            Command::batch(label.clone(), cmds)
        }
    };
    Ok((command, label, name))
}
