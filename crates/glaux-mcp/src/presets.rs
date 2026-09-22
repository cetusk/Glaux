//! 音色プリセットライブラリ。
//!
//! トラックの「音源(device)+ エフェクトチェーン」をひとまとめのパッチとして
//! `<設定ディレクトリ>/glaux/presets/<名前>.json` に保存し、**曲プロジェクトを
//! またいで**再利用する(docs/HANDOFF.md §8-6)。
//!
//! - エフェクトの `FxId` はプロジェクト固有なので保存しない。適用時に新しい ID を
//!   生成する(コマンドは決定的: ID は呼び出し側 = ここで生成して Command に渡す)。
//! - 適用は 1 つの `Batch`(set_device + 既存エフェクト削除 + 追加)= 1 回の undo。

use glaux_core::{Command, Device, Effect, FxId, ParamMap, PluginSource, Track};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const PRESET_FORMAT: &str = "glaux-preset";
pub const PRESET_VERSION: u32 = 1;

/// 保存されるプリセット本体。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Preset {
    pub format: String,
    pub version: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 音源(内蔵楽器 + パラメータ)
    pub device: Device,
    /// エフェクトチェーン(ID なし。適用時に採番)
    #[serde(default)]
    pub effects: Vec<PresetEffect>,
    /// RFC3339
    pub created: String,
}

/// プリセット内のエフェクト(`FxId` を持たない以外は `Effect` と同じ)。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PresetEffect {
    #[serde(flatten)]
    pub source: PluginSource,
    #[serde(default)]
    pub bypass: bool,
    #[serde(default)]
    pub params: ParamMap,
}

/// 一覧表示用の要約。
#[derive(Clone, Debug, Serialize)]
pub struct PresetInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 音源名(subtractive / drum)
    pub instrument: String,
    /// エフェクト名の一覧(順番どおり)
    pub effects: Vec<String>,
    pub created: String,
}

/// 既定のプリセット置き場(`%APPDATA%\glaux\presets` / `~/.config/glaux/presets`)。
pub fn default_dir() -> PathBuf {
    let base = std::env::var("APPDATA")
        .or_else(|_| std::env::var("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("USERPROFILE")
                .or_else(|_| std::env::var("HOME"))
                .unwrap_or_else(|_| ".".to_owned());
            PathBuf::from(home).join(".config")
        });
    base.join("glaux").join("presets")
}

fn source_name(source: &PluginSource) -> String {
    match source {
        PluginSource::Builtin { name } => name.clone(),
        other => format!("{other:?}"),
    }
}

/// ファイル名として安全な名前か検証する。
fn validate_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("プリセット名が空です".to_owned());
    }
    if name.starts_with('.') {
        return Err("プリセット名を . で始めることはできません".to_owned());
    }
    if name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
        return Err(format!(
            "プリセット名に使えない文字が含まれています: {name}"
        ));
    }
    Ok(())
}

fn preset_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.json"))
}

/// トラックの現在の音をプリセットとして保存する。
/// `overwrite: false` で同名が既にあればエラー。
pub fn save(
    dir: &Path,
    track: &Track,
    name: &str,
    description: Option<String>,
    overwrite: bool,
) -> Result<Preset, String> {
    validate_name(name)?;
    let device = track
        .device
        .clone()
        .unwrap_or_else(|| Device::builtin(glaux_dsp::DEFAULT_INSTRUMENT));
    if !matches!(device.source, PluginSource::Builtin { .. }) {
        return Err("内蔵音源のトラックのみプリセット保存できます".to_owned());
    }
    let preset = Preset {
        format: PRESET_FORMAT.to_owned(),
        version: PRESET_VERSION,
        name: name.to_owned(),
        description,
        device,
        effects: track
            .effects
            .iter()
            .map(|e| PresetEffect {
                source: e.source.clone(),
                bypass: e.bypass,
                params: e.params.clone(),
            })
            .collect(),
        created: chrono::Local::now().to_rfc3339(),
    };

    let path = preset_path(dir, name);
    if path.exists() && !overwrite {
        return Err(format!(
            "同名のプリセットが既にあります(上書きは overwrite: true): {name}"
        ));
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(&preset).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    Ok(preset)
}

/// プリセットを読み込む。
pub fn load(dir: &Path, name: &str) -> Result<Preset, String> {
    validate_name(name)?;
    let path = preset_path(dir, name);
    let text = std::fs::read_to_string(&path)
        .map_err(|_| format!("プリセットが見つかりません: {name}"))?;
    let preset: Preset = serde_json::from_str(&text)
        .map_err(|e| format!("プリセットを読み込めません({name}): {e}"))?;
    if preset.format != PRESET_FORMAT {
        return Err(format!("プリセット形式ではありません: {name}"));
    }
    Ok(preset)
}

/// 保存済みプリセットの一覧(名前順)。壊れたファイルは黙って飛ばす。
pub fn list(dir: &Path) -> Vec<PresetInfo> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PresetInfo> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            let text = std::fs::read_to_string(e.path()).ok()?;
            let p: Preset = serde_json::from_str(&text).ok()?;
            (p.format == PRESET_FORMAT).then(|| PresetInfo {
                name: p.name,
                description: p.description,
                instrument: source_name(&p.device.source),
                effects: p.effects.iter().map(|f| source_name(&f.source)).collect(),
                created: p.created,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// 出荷時プリセット。初回起動時に一度だけ書き込む(ユーザーが削除したら復活させない)。
fn factory_presets() -> Vec<Preset> {
    let float = |v: f64| glaux_core::ParamValue::Float(v);
    let device = |name: &str, params: &[(&str, f64)]| {
        let mut d = Device::builtin(name);
        for (k, v) in params {
            d.params.insert((*k).to_owned(), float(*v));
        }
        d
    };
    let fx = |name: &str, params: &[(&str, f64)]| PresetEffect {
        source: PluginSource::Builtin {
            name: name.to_owned(),
        },
        bypass: false,
        params: params
            .iter()
            .map(|(k, v)| ((*k).to_owned(), float(*v)))
            .collect(),
    };
    let preset = |name: &str, desc: &str, device: Device, effects: Vec<PresetEffect>| Preset {
        format: PRESET_FORMAT.to_owned(),
        version: PRESET_VERSION,
        name: name.to_owned(),
        description: Some(desc.to_owned()),
        device,
        effects,
        created: chrono::Local::now().to_rfc3339(),
    };

    vec![
        preset(
            "アコースティックギター",
            "pluck 素の弦。アルペジオやストローク系のバッキングに",
            device(
                "pluck",
                &[("decay", 3.5), ("brightness", 0.45), ("pick", 0.45)],
            ),
            vec![fx("reverb", &[("mix", 0.15), ("size", 0.4)])],
        ),
        preset(
            "クリーンエレキ",
            "pluck + アンプ(低ゲイン)。カッティングやクリーントーンのリフに",
            device(
                "pluck",
                &[("decay", 2.2), ("brightness", 0.65), ("pick", 0.7)],
            ),
            vec![fx(
                "amp",
                &[("gain_db", 12.0), ("tone", 0.6), ("level_db", -8.0)],
            )],
        ),
        preset(
            "クランチギター",
            "pluck + アンプ(中ゲイン)。ロックのバッキングに",
            device(
                "pluck",
                &[("decay", 1.8), ("brightness", 0.65), ("pick", 0.8)],
            ),
            vec![fx(
                "amp",
                &[("gain_db", 24.0), ("tone", 0.55), ("level_db", -12.0)],
            )],
        ),
        preset(
            "メタルギター",
            "pluck + アンプ(ハイゲイン)。低音の刻みは palm_mute ノートと組み合わせる",
            device(
                "pluck",
                &[("decay", 1.6), ("brightness", 0.7), ("pick", 0.9)],
            ),
            vec![fx(
                "amp",
                &[
                    ("gain_db", 44.0),
                    ("tone", 0.5),
                    ("presence", 0.45),
                    ("level_db", -16.0),
                ],
            )],
        ),
    ]
}

/// 出荷時プリセットの版。上げると次回起動時に同名の出荷時プリセットを更新する
/// (ユーザーが独自に作った別名のプリセットには触れない)。
const FACTORY_VERSION: &str = "v2";

/// 出荷時プリセットを導入・更新する(アプリ起動時に呼ぶ)。
/// - マーカーが現行版: 何もしない(ユーザーが削除したものを復活させない)
/// - マーカーが旧版: 同名の出荷時プリセットを新定義で上書きして版を上げる
/// - マーカーなし(初回): 同名の既存ファイルがあれば尊重して残す
pub fn ensure_factory(dir: &Path) {
    let marker = dir.join(".factory-installed");
    let installed = std::fs::read_to_string(&marker).unwrap_or_default();
    if installed.trim() == FACTORY_VERSION {
        return;
    }
    let fresh_install = installed.trim().is_empty();
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    for p in factory_presets() {
        let path = preset_path(dir, &p.name);
        if path.exists() && fresh_install {
            continue; // 初回導入で同名がある = ユーザー作かもしれないので触らない
        }
        if let Ok(json) = serde_json::to_string_pretty(&p) {
            let _ = std::fs::write(&path, json);
        }
    }
    let _ = std::fs::write(&marker, format!("{FACTORY_VERSION}\n"));
}

/// プリセットを削除する。
pub fn remove(dir: &Path, name: &str) -> Result<(), String> {
    validate_name(name)?;
    std::fs::remove_file(preset_path(dir, name))
        .map_err(|_| format!("プリセットが見つかりません: {name}"))
}

/// プリセットをトラックに適用するコマンド列を作る(1 Batch で適用すること)。
/// 音源を差し替え、既存のエフェクトチェーンをプリセットの内容に置き換える。
pub fn apply_commands(track: &Track, preset: &Preset) -> Vec<Command> {
    let mut cmds = vec![Command::SetDevice {
        track: track.id.clone(),
        device: Some(preset.device.clone()),
    }];
    for e in &track.effects {
        cmds.push(Command::RemoveEffect { id: e.id.clone() });
    }
    for f in &preset.effects {
        cmds.push(Command::AddEffect {
            track: track.id.clone(),
            effect: Effect {
                id: FxId::new(),
                source: f.source.clone(),
                bypass: f.bypass,
                params: f.params.clone(),
            },
            index: None,
        });
    }
    cmds
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::{TrackId, TrackKind};

    fn track_with_patch() -> Track {
        let mut t = Track::new(TrackId::new(), "Lead", TrackKind::Midi);
        let mut device = Device::builtin("subtractive");
        device
            .params
            .insert("cutoff".to_owned(), glaux_core::ParamValue::Float(1200.0));
        t.device = Some(device);
        t.effects.push(Effect {
            id: FxId::new(),
            source: PluginSource::Builtin {
                name: "distortion".to_owned(),
            },
            bypass: false,
            params: ParamMap::new(),
        });
        t
    }

    #[test]
    fn save_list_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let track = track_with_patch();
        save(
            tmp.path(),
            &track,
            "メタルリード",
            Some("刻み用".into()),
            false,
        )
        .unwrap();

        let infos = list(tmp.path());
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].name, "メタルリード");
        assert_eq!(infos[0].instrument, "subtractive");
        assert_eq!(infos[0].effects, vec!["distortion".to_owned()]);

        let p = load(tmp.path(), "メタルリード").unwrap();
        assert_eq!(p.device, track.device.clone().unwrap());
        assert_eq!(p.effects.len(), 1);

        // 上書き保護
        assert!(save(tmp.path(), &track, "メタルリード", None, false).is_err());
        assert!(save(tmp.path(), &track, "メタルリード", None, true).is_ok());

        remove(tmp.path(), "メタルリード").unwrap();
        assert!(list(tmp.path()).is_empty());
    }

    #[test]
    fn rejects_bad_names() {
        let tmp = tempfile::tempdir().unwrap();
        let track = track_with_patch();
        for bad in ["", "  ", "a/b", "a\\b", "c:", ".hidden", "a*b"] {
            assert!(save(tmp.path(), &track, bad, None, false).is_err(), "{bad}");
        }
        assert!(load(tmp.path(), "../etc/passwd").is_err());
    }

    #[test]
    fn apply_commands_replace_device_and_effects() {
        let tmp = tempfile::tempdir().unwrap();
        let track = track_with_patch();
        let preset = save(tmp.path(), &track, "p", None, false).unwrap();

        // 適用先: 別のエフェクトを 2 つ持つトラック
        let mut dest = track_with_patch();
        dest.effects.push(Effect {
            id: FxId::new(),
            source: PluginSource::Builtin {
                name: "reverb".to_owned(),
            },
            bypass: false,
            params: ParamMap::new(),
        });

        let cmds = apply_commands(&dest, &preset);
        // set_device + 既存 2 削除 + プリセット 1 追加
        assert_eq!(cmds.len(), 4);
        assert!(matches!(cmds[0], Command::SetDevice { .. }));
        assert!(matches!(cmds[1], Command::RemoveEffect { .. }));
        assert!(matches!(cmds[2], Command::RemoveEffect { .. }));
        assert!(matches!(cmds[3], Command::AddEffect { .. }));
    }
}
