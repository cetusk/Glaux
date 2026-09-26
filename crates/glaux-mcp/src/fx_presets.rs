//! エフェクトのプリセット(エフェクト 1 つ分)。
//!
//! 音色のプリセット([`crate::presets`]。音源 + エフェクト一式)とは別に、よくできたエフェクト 1 つを
//! `<設定ディレクトリ>/glaux/effect_presets/<名前>.json` に保存し、**トラック・曲をまたいで**使う。
//! ノード表示の「わき」(トラックの中だけ)に置いておくより広く使い回したいものを入れる所。
//!
//! - `FxId` は保存しない。足すときに新しい ID を作る(ID はコマンドを作る側 = ここで生成)
//! - 足すと、プリセットの名前を表示名に、メモもそのまま付ける

use crate::presets::PresetEffect;
use glaux_core::{Command, Effect, EffectUi, FxId, PluginSource, Project, TrackId};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const FX_PRESET_FORMAT: &str = "glaux-fx-preset";
pub const FX_PRESET_VERSION: u32 = 1;

/// 保存されるエフェクトのプリセット。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FxPreset {
    pub format: String,
    pub version: u32,
    pub name: String,
    /// メモ(なぜ取っておいたか・どんな音か)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// どのトラックから保存したか(表示用)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    pub effect: PresetEffect,
    /// RFC3339
    pub created: String,
}

/// 一覧表示用の要約。
#[derive(Clone, Debug, Serialize)]
pub struct FxPresetInfo {
    pub name: String,
    /// 種類(内蔵エフェクト名、CLAP は "clap")
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    pub created: String,
}

/// 置き場(`%APPDATA%\glaux\effect_presets` / `~/.config/glaux/effect_presets`)。
pub fn default_dir() -> PathBuf {
    crate::presets::default_dir()
        .parent()
        .map(|p| p.join("effect_presets"))
        .unwrap_or_else(|| PathBuf::from("effect_presets"))
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("プリセット名が空です".to_owned());
    }
    if name.starts_with('.') || name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
        return Err(format!(
            "プリセット名に使えない文字が含まれています: {name}"
        ));
    }
    Ok(())
}

fn path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.json"))
}

fn kind_of(source: &PluginSource) -> (String, Option<String>) {
    match source {
        PluginSource::Builtin { name } => (name.clone(), None),
        PluginSource::Clap { plugin_id, .. } => ("clap".to_owned(), Some(plugin_id.clone())),
        other => (format!("{other:?}"), None),
    }
}

/// エフェクトを名前を付けて保存する。
pub fn save(
    dir: &Path,
    effect: &Effect,
    name: &str,
    note: Option<String>,
    origin: Option<String>,
    overwrite: bool,
) -> Result<FxPreset, String> {
    validate_name(name)?;
    let p = path(dir, name);
    if p.exists() && !overwrite {
        return Err(format!(
            "同じ名前のプリセットがあります: {name}(上書きするなら overwrite: true)"
        ));
    }
    let preset = FxPreset {
        format: FX_PRESET_FORMAT.to_owned(),
        version: FX_PRESET_VERSION,
        name: name.to_owned(),
        note: note
            .or_else(|| effect.ui.note.clone())
            .filter(|n| !n.trim().is_empty()),
        origin,
        effect: PresetEffect {
            source: effect.source.clone(),
            bypass: false,
            params: effect.params.clone(),
        },
        created: chrono::Local::now().to_rfc3339(),
    };
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(&preset).map_err(|e| e.to_string())?;
    std::fs::write(&p, text).map_err(|e| format!("保存できません({name}): {e}"))?;
    Ok(preset)
}

pub fn load(dir: &Path, name: &str) -> Result<FxPreset, String> {
    validate_name(name)?;
    let text = std::fs::read_to_string(path(dir, name))
        .map_err(|_| format!("エフェクトのプリセットが見つかりません: {name}"))?;
    let p: FxPreset =
        serde_json::from_str(&text).map_err(|e| format!("読み込めません({name}): {e}"))?;
    if p.format != FX_PRESET_FORMAT {
        return Err(format!(
            "エフェクトのプリセットの形式ではありません: {name}"
        ));
    }
    Ok(p)
}

/// 一覧(新しい順)。壊れたファイルは飛ばす。
pub fn list(dir: &Path) -> Vec<FxPresetInfo> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<FxPresetInfo> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            let p: FxPreset =
                serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
            (p.format == FX_PRESET_FORMAT).then(|| {
                let (kind, plugin_id) = kind_of(&p.effect.source);
                FxPresetInfo {
                    name: p.name,
                    kind,
                    plugin_id,
                    note: p.note,
                    origin: p.origin,
                    created: p.created,
                }
            })
        })
        .collect();
    out.sort_by(|a, b| b.created.cmp(&a.created));
    out
}

pub fn remove(dir: &Path, name: &str) -> Result<(), String> {
    validate_name(name)?;
    std::fs::remove_file(path(dir, name))
        .map_err(|_| format!("エフェクトのプリセットが見つかりません: {name}"))
}

/// 足す先: トラックかマスター
pub enum Target {
    Track(TrackId),
    Master,
}

impl Target {
    /// "master" / "__master__" ならマスター、それ以外はトラック ID
    pub fn parse(s: &str) -> Result<Target, String> {
        if s == "master" || s == "__master__" {
            Ok(Target::Master)
        } else {
            TrackId::parse(s)
                .map(Target::Track)
                .map_err(|e| e.to_string())
        }
    }
}

/// プリセットのエフェクトを足すコマンド(1 件)。`parked` ならわきに置いた状態で、`pos` はその位置
pub fn add_command(
    project: &Project,
    target: &Target,
    preset: &FxPreset,
    index: Option<usize>,
    parked: bool,
    pos: Option<[f32; 2]>,
) -> Result<(Command, FxId), String> {
    let id = FxId::new();
    let effect = Effect {
        id: id.clone(),
        source: preset.effect.source.clone(),
        bypass: false,
        params: preset.effect.params.clone(),
        ui: EffectUi {
            label: Some(preset.name.clone()),
            parked,
            note: preset.note.clone(),
            pos: if parked { pos } else { None },
        },
    };
    let cmd = match target {
        Target::Master => Command::AddMasterEffect { effect, index },
        Target::Track(t) => {
            let track = project
                .track(t)
                .ok_or_else(|| format!("トラックが見つかりません: {t}"))?;
            if index.is_some_and(|i| i > track.effects.len()) {
                return Err("index がエフェクトの数を超えています".to_owned());
            }
            Command::AddEffect {
                track: t.clone(),
                effect,
                index,
            }
        }
    };
    Ok((cmd, id))
}

/// トラックかマスターのエフェクトを探す
pub fn find_effect<'a>(
    project: &'a Project,
    target: &Target,
    fx_id: &FxId,
) -> Result<(&'a Effect, String), String> {
    let (effects, owner) = match target {
        Target::Master => (&project.master.effects, "マスター".to_owned()),
        Target::Track(t) => {
            let track = project
                .track(t)
                .ok_or_else(|| format!("トラックが見つかりません: {t}"))?;
            (&track.effects, track.name.clone())
        }
    };
    effects
        .iter()
        .find(|e| &e.id == fx_id)
        .map(|e| (e, owner))
        .ok_or_else(|| format!("エフェクトが見つかりません: {fx_id}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::{Track, TrackKind};

    #[test]
    fn saves_lists_and_adds_with_name_and_note() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let mut eq = Effect::builtin(FxId::new(), "eq");
        eq.params.insert("high_db".into(), 3.0.into());
        eq.ui.note = Some("こもりを削る".into());
        let saved = save(dir, &eq, "ボーカル用 EQ", None, Some("Voice".into()), false).unwrap();
        assert_eq!(saved.note.as_deref(), Some("こもりを削る"));
        assert!(
            save(dir, &eq, "ボーカル用 EQ", None, None, false).is_err(),
            "同じ名前は上書きを頼まない限り断る"
        );
        let list = list(dir);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].kind, "eq");
        assert_eq!(list[0].origin.as_deref(), Some("Voice"));

        // 別の曲のトラックへ(わきに置いた状態で)
        let mut p = Project::new("別の曲");
        let tid = TrackId::new();
        p.tracks
            .push(Track::new(tid.clone(), "Lead", TrackKind::Midi));
        let preset = load(dir, "ボーカル用 EQ").unwrap();
        let (cmd, id) = add_command(
            &p,
            &Target::Track(tid.clone()),
            &preset,
            None,
            true,
            Some([300.0, 280.0]),
        )
        .unwrap();
        p.apply(&cmd).unwrap();
        let e = &p.tracks[0].effects[0];
        assert_eq!(e.id, id);
        assert_eq!(e.ui.label.as_deref(), Some("ボーカル用 EQ"));
        assert!(e.ui.parked);
        assert_eq!(e.ui.pos, Some([300.0, 280.0]));
        assert_eq!(e.params.get("high_db"), eq.params.get("high_db"));

        // マスターへ
        let (cmd, _) = add_command(
            &p,
            &Target::parse("master").unwrap(),
            &preset,
            Some(0),
            false,
            None,
        )
        .unwrap();
        p.apply(&cmd).unwrap();
        assert_eq!(p.master.effects.len(), 1);
        assert!(!p.master.effects[0].ui.parked);

        remove(dir, "ボーカル用 EQ").unwrap();
        assert!(super::list(dir).is_empty());
    }
}
