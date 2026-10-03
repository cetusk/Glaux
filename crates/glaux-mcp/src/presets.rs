//! 音色プリセットライブラリ。
//!
//! トラックの「音源(device)+ エフェクトチェーン」をひとまとめのパッチとして
//! `<設定ディレクトリ>/glaux/presets/<名前>.json` に保存し、**曲プロジェクトを
//! またいで**再利用する(docs/HANDOFF.md §8-6)。
//!
//! - エフェクトの `FxId` はプロジェクト固有なので保存しない。適用時に新しい ID を
//!   生成する(コマンドは決定的: ID は呼び出し側 = ここで生成して Command に渡す)。
//! - 適用は 1 つの `Batch`(set_device + 既存エフェクト削除 + 追加 + 層 + マクロ)= 1 回の undo。
//! - 層(重ねる音源)とマクロも持つ。マクロがエフェクトを指すときは、保存時に `fx_slot<番号>`
//!   (プリセットのエフェクトの並びの番号)に置き換え、適用時に新しい ID に戻す。

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
    /// 重ねる音源
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<glaux_core::Layer>,
    /// マクロ(エフェクトの先は `fx_slot<番号>`)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub macros: Vec<glaux_core::Macro>,
    /// RFC3339
    pub created: String,
}

/// プリセットの中のエフェクトの番号を表す仮の ID
fn slot_id(i: usize) -> FxId {
    FxId::parse(&format!("fx_slot{i}")).unwrap_or_else(|_| FxId::new())
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
    // 鳴っているエフェクトだけを処理の順に(線から外したもの・つながっていないものは音に入らない)。
    // 分岐・合流のつながりは持たず、処理の順の直列として保存する
    let order: Vec<&Effect> =
        glaux_core::model::routing::processing_order(&track.effects, &track.links())
            .iter()
            .filter_map(|id| track.effects.iter().find(|e| &e.id == id))
            .collect();
    // マクロのエフェクトの先は並びの番号に。保存しないエフェクトを指す割り当ては落とす
    let macros: Vec<glaux_core::Macro> = track
        .macros
        .iter()
        .filter_map(|m| {
            let targets: Vec<glaux_core::MacroTarget> = m
                .targets
                .iter()
                .filter_map(|t| {
                    let mut t = t.clone();
                    if let glaux_core::ParamPath::Effect { id, .. } = &mut t.target {
                        let i = order.iter().position(|e| &e.id == id)?;
                        *id = slot_id(i);
                    }
                    Some(t)
                })
                .collect();
            (!targets.is_empty()).then(|| glaux_core::Macro {
                targets,
                ..m.clone()
            })
        })
        .collect();
    let preset = Preset {
        format: PRESET_FORMAT.to_owned(),
        version: PRESET_VERSION,
        name: name.to_owned(),
        description,
        device,
        effects: order
            .iter()
            .map(|e| PresetEffect {
                source: e.source.clone(),
                bypass: e.bypass,
                params: e.params.clone(),
            })
            .collect(),
        layers: track.layers.clone(),
        macros,
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
    let wavetable = |table: &str, params: &[(&str, f64)]| {
        let mut d = device("wavetable", params);
        d.params.insert(
            "table".to_owned(),
            glaux_core::ParamValue::Enum(table.to_owned()),
        );
        d
    };
    // wavetable に選択肢のつまみ(変形・LFO の行き先など)も入れる
    let wavetable_with = |table: &str, params: &[(&str, f64)], enums: &[(&str, &str)]| {
        let mut d = wavetable(table, params);
        for (k, v) in enums {
            d.params.insert(
                (*k).to_owned(),
                glaux_core::ParamValue::Enum((*v).to_owned()),
            );
        }
        d
    };
    // 雑音だけの subtractive(波形を消して、雑音の色を選ぶ)
    let noise_dev = |color: &str, params: &[(&str, f64)]| {
        let mut d = device("subtractive", params);
        d.params.insert("osc_level".to_owned(), float(0.0));
        d.params.insert(
            "noise_color".to_owned(),
            glaux_core::ParamValue::Enum(color.to_owned()),
        );
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
        layers: vec![],
        macros: vec![],
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
        preset(
            "ウォブルベース",
            "wavetable(sync)の position を LFO で揺らすダブステップ/ベースミュージックのうねり。\
             lfo_rate をテンポに合わせる(8 分 = BPM/30 Hz)",
            wavetable(
                "sync",
                &[
                    ("position", 0.35),
                    ("lfo_rate", 4.0),
                    ("lfo_depth", 0.6),
                    ("unison", 3.0),
                    ("detune", 10.0),
                    ("cutoff", 6000.0),
                    ("attack", 0.003),
                    ("sustain", 1.0),
                    ("release", 0.08),
                    ("gain_db", -10.0),
                ],
            ),
            vec![fx(
                "distortion",
                &[("drive_db", 10.0), ("mix", 0.5), ("level_db", -4.0)],
            )],
        ),
        preset(
            "グロウルベース",
            "うなる母音のテーブル(growl)を position と変形(FM)の 2 本の動きで揺らすダブステップのグロウル。\
             position の LFO と LFO2(warp)の速さをテンポに合わせる(8 分 = BPM/30 Hz)。オートメーションで position を動かしても",
            wavetable_with(
                "growl",
                &[
                    ("position", 0.35),
                    ("lfo_rate", 3.0),
                    ("lfo_depth", 0.5),
                    ("warp_amount", 0.25),
                    ("lfo2_rate", 1.5),
                    ("lfo2_depth", 0.5),
                    ("unison", 2.0),
                    ("detune", 8.0),
                    ("cutoff", 4000.0),
                    ("attack", 0.003),
                    ("sustain", 1.0),
                    ("release", 0.08),
                    ("gain_db", -10.0),
                ],
                &[("warp", "fm"), ("lfo2_target", "warp")],
            ),
            vec![fx(
                "distortion",
                &[("drive_db", 8.0), ("mix", 0.45), ("level_db", -4.0)],
            )],
        ),
        preset(
            "リースベース",
            "わずかにずらしたノコギリ波を重ねてうねらせる、ドラムンベース・ベースミュージックの太い低音。\
             低めのカットオフをゆっくり揺らす。左右には広げない(低音は真ん中に)",
            wavetable(
                "analog",
                &[
                    ("position", 0.66),
                    ("unison", 3.0),
                    ("detune", 18.0),
                    ("cutoff", 900.0),
                    ("resonance", 0.15),
                    ("lfo1_rate", 0.3),
                    ("lfo1_depth", 0.2),
                    ("attack", 0.005),
                    ("sustain", 1.0),
                    ("release", 0.12),
                    ("gain_db", -10.0),
                ],
            ),
            vec![fx(
                "distortion",
                &[("drive_db", 6.0), ("mix", 0.35), ("level_db", -3.0)],
            )],
        ),
        preset(
            "ニューロベース",
            "ウェーブフォールド(fold)にシンクの変形を掛け、LFO2 で変形の量を細かく揺らす荒れたベース(ニューロファンク)。\
             position をオートメーションで動かすと表情が変わる",
            wavetable_with(
                "fold",
                &[
                    ("position", 0.5),
                    ("lfo_rate", 2.0),
                    ("lfo_depth", 0.3),
                    ("warp_amount", 0.3),
                    ("lfo2_rate", 4.0),
                    ("lfo2_depth", 0.4),
                    ("cutoff", 8000.0),
                    ("attack", 0.002),
                    ("sustain", 1.0),
                    ("release", 0.06),
                    ("gain_db", -12.0),
                ],
                &[("warp", "sync"), ("lfo2_target", "warp"), ("lfo2_shape", "square")],
            ),
            vec![fx(
                "distortion",
                &[("drive_db", 12.0), ("mix", 0.55), ("level_db", -6.0)],
            )],
        ),
        preset(
            "デジタルリード",
            "量子化したノコギリ(digital)を曲げる変形(bend)で鋭くした、粗いデジタルのリード。左右に広げて付点 8 分のディレイ",
            wavetable_with(
                "digital",
                &[
                    ("position", 0.4),
                    ("warp_amount", 0.3),
                    ("unison", 3.0),
                    ("detune", 12.0),
                    ("spread", 0.6),
                    ("cutoff", 7000.0),
                    ("attack", 0.005),
                    ("sustain", 0.85),
                    ("release", 0.25),
                    ("gain_db", -14.0),
                ],
                &[("warp", "bend")],
            ),
            vec![
                fx("delay", &[("time_ms", 375.0), ("feedback", 0.3), ("mix", 0.18), ("ping_pong", 1.0)]),
                fx("reverb", &[("mix", 0.15), ("size", 0.5)]),
            ],
        ),
        preset(
            "母音パッド",
            "wavetable(vocal)を LFO でゆっくり動かす、しゃべるように変化するパッド",
            wavetable(
                "vocal",
                &[
                    ("position", 0.4),
                    ("lfo_rate", 0.15),
                    ("lfo_depth", 0.5),
                    ("unison", 5.0),
                    ("detune", 18.0),
                    ("attack", 0.6),
                    ("sustain", 0.9),
                    ("release", 1.5),
                    ("gain_db", -12.0),
                ],
            ),
            vec![
                fx("chorus", &[("mix", 0.4)]),
                fx("reverb", &[("mix", 0.35), ("size", 0.8)]),
            ],
        ),
        preset(
            "シンクリード",
            "wavetable(sync)+ pos_env で鳴り始めがギラッと鋭い EDM のリード",
            wavetable(
                "sync",
                &[
                    ("position", 0.2),
                    ("pos_env", 0.5),
                    ("pos_decay", 0.25),
                    ("unison", 5.0),
                    ("detune", 20.0),
                    ("sustain", 0.8),
                    ("release", 0.2),
                    ("gain_db", -12.0),
                ],
            ),
            vec![fx(
                "delay",
                &[("time_ms", 375.0), ("feedback", 0.35), ("mix", 0.25)],
            )],
        ),
        preset(
            "Lo-fi エレピ",
            "fm のエレピ + chorus + tape(レコードのパチパチ込み)。チルホップ/ Lo-fi Hip Hop の揺れてくすんだ鍵盤",
            device(
                "fm",
                &[
                    ("ratio", 1.0),
                    ("index", 2.5),
                    ("index_decay", 0.3),
                    ("decay", 2.0),
                    ("sustain", 0.2),
                    ("release", 0.4),
                ],
            ),
            vec![
                fx(
                    "chorus",
                    &[("rate_hz", 0.6), ("depth_ms", 2.0), ("mix", 0.35)],
                ),
                fx(
                    "tape",
                    &[
                        ("wow", 0.4),
                        ("flutter", 0.25),
                        ("saturation", 0.4),
                        ("tone", 5000.0),
                        ("hiss", 0.25),
                        ("crackle", 0.25),
                    ],
                ),
            ],
        ),
        preset(
            "レコードノイズ",
            "雑音だけ(ピンクノイズを少し + パチパチ)。ローファイの地の音。1 つの長い音(C4 など)を曲の長さぶん置き、\
             音量は -18〜-12dB。低いゴロゴロが要るなら noise_color を brown に",
            noise_dev(
                "pink",
                &[
                    ("noise", 0.12),
                    ("crackle", 0.35),
                    ("cutoff", 7000.0),
                    ("resonance", 0.0),
                    ("attack", 0.05),
                    ("sustain", 1.0),
                    ("release", 0.8),
                    ("filter_env", 0.0),
                    ("gain_db", -6.0),
                ],
            ),
            vec![fx("tape", &[("wow", 0.2), ("saturation", 0.2), ("tone", 6000.0)])],
        ),
        preset(
            "ノイズのライザー",
            "ホワイトノイズの音が立ち上がるにつれてフィルタが開く(ビルドの「シューッ」)。ビルドの長さの音を 1 つ置く。\
             長さに合わせて attack を 2〜8 秒に。止めるときはドロップの頭で切る",
            noise_dev(
                "white",
                &[
                    ("noise", 0.8),
                    ("cutoff", 250.0),
                    ("resonance", 0.35),
                    ("attack", 4.0),
                    ("decay", 3.0),
                    ("sustain", 1.0),
                    ("release", 0.3),
                    ("filter_env", 1.0),
                    ("gain_db", -6.0),
                ],
            ),
            vec![fx("reverb", &[("mix", 0.3), ("size", 0.8)])],
        ),
        preset(
            "風",
            "ブラウンノイズ + 共振で、ヒューと鳴る風・波の環境音。cutoff をオートメーションで動かすと吹き方が変わる",
            noise_dev(
                "brown",
                &[
                    ("noise", 1.0),
                    ("cutoff", 900.0),
                    ("resonance", 0.7),
                    ("attack", 1.5),
                    ("sustain", 1.0),
                    ("release", 2.0),
                    ("filter_env", 0.0),
                    ("gain_db", -3.0),
                ],
            ),
            vec![fx("reverb", &[("mix", 0.35), ("size", 0.9)])],
        ),
        preset(
            "ポンピングパッド",
            "厚いのこぎり波のパッドを 4 分ごとに沈める(volume_shaper。サイドチェイン無しで 4 つ打ちに合わせて呼吸する)。EDM・ハウスのコードに",
            device(
                "subtractive",
                &[
                    ("unison", 5.0),
                    ("detune", 22.0),
                    ("cutoff", 3500.0),
                    ("attack", 0.05),
                    ("sustain", 0.9),
                    ("release", 0.4),
                    ("filter_env", 0.1),
                    ("gain_db", -12.0),
                ],
            ),
            vec![
                fx("volume_shaper", &[("depth_db", 14.0), ("release", 0.55)]),
                fx("reverb", &[("mix", 0.25), ("size", 0.7)]),
            ],
        ),
        preset(
            "トランスゲートのパッド",
            "パッドを 16 分の型(trance)で刻む。伸ばしのコードを置くだけでトランスのリズムになる",
            device(
                "subtractive",
                &[
                    ("unison", 5.0),
                    ("detune", 18.0),
                    ("cutoff", 5000.0),
                    ("attack", 0.01),
                    ("sustain", 1.0),
                    ("release", 0.3),
                    ("filter_env", 0.0),
                    ("gain_db", -12.0),
                ],
            ),
            vec![
                fx("trance_gate", &[("depth", 1.0), ("smooth_ms", 3.0)]),
                fx("delay", &[("time_ms", 375.0), ("feedback", 0.3), ("mix", 0.2), ("duck_db", 4.0)]),
                fx("reverb", &[("mix", 0.2), ("size", 0.6)]),
            ],
        ),
        preset(
            "80 年代のドラム",
            "ドラムに大きく短く切ったリバーブ(ゲートリバーブ)とクリッパー。80 年代のスネアの「バシャッ」",
            device("drum", &[]),
            vec![
                fx("reverb", &[("mix", 0.35), ("size", 0.85), ("gate_ms", 220.0)]),
                fx("clipper", &[("drive_db", 4.0)]),
            ],
        ),
        preset(
            "オートワウ・ギター",
            "pluck のギター + アンプ + 弾いた強さで開くバンドパス(auto_filter の env_amount)。ファンクのカッティングに",
            device(
                "pluck",
                &[("decay", 1.2), ("brightness", 0.7), ("pick", 0.8)],
            ),
            vec![
                fx("amp", &[("gain_db", 14.0), ("tone", 0.6), ("level_db", -9.0)]),
                fx(
                    "auto_filter",
                    &[("cutoff", 500.0), ("resonance", 0.6), ("depth", 0.0), ("env_amount", 2.5)],
                ),
            ],
        ),
        preset(
            "ビットクラッシュ・リード",
            "矩形波のリードをビットとサンプルレートで荒らしたゲーム機風の音",
            {
                let mut d = device(
                    "subtractive",
                    &[("cutoff", 9000.0), ("sustain", 0.8), ("release", 0.1), ("filter_env", 0.2), ("gain_db", -12.0)],
                );
                d.params.insert(
                    "waveform".to_owned(),
                    glaux_core::ParamValue::Enum("square".to_owned()),
                );
                d
            },
            vec![fx("bitcrush", &[("bits", 6.0), ("downsample", 4.0), ("mix", 1.0)])],
        ),
        // ---- 生きた音の手本(広がり・揺らぎ・フィルタの種類とエンベロープ・LFO) ----
        preset(
            "ワイドなスーパーソー",
            "7 声のスーパーソーを左右に広げ(spread)、揺らぎ(analog)で生きた厚みに。モノで聴いても痩せない。\
            EDM・トランスのコード・リード。付点 8 分のディレイとホールの空間付き",
            device(
                "subtractive",
                &[
                    ("unison", 7.0),
                    ("detune", 30.0),
                    ("spread", 0.8),
                    ("analog", 0.3),
                    ("cutoff", 6000.0),
                    ("filter_env", 0.1),
                    ("key_track", 0.4),
                    ("sustain", 0.85),
                    ("release", 0.35),
                    ("gain_db", -15.0),
                ],
            ),
            vec![
                fx("delay", &[("time_ms", 375.0), ("feedback", 0.3), ("mix", 0.18), ("ping_pong", 1.0)]),
                fx("reverb", &[("mix", 0.2), ("size", 0.7)]),
            ],
        ),
        preset(
            "アナログベース",
            "24dB のローパスとドライブで太く、フィルタのエンベロープで頭だけ「ブッ」と開くベース。\
            強く弾くほど明るい。キーで明るさが付いていくので、どの音域でも同じ太さ",
            {
                let mut d = device(
                    "subtractive",
                    &[
                        ("cutoff", 280.0),
                        ("resonance", 0.25),
                        ("drive", 0.35),
                        ("filter_env", 0.65),
                        ("filter_decay", 0.22),
                        ("filter_sustain", 0.1),
                        ("vel_cutoff", 0.4),
                        ("key_track", 0.5),
                        ("sub", 0.35),
                        ("analog", 0.2),
                        ("sustain", 0.9),
                        ("release", 0.08),
                        ("gain_db", -10.0),
                    ],
                );
                d.params.insert(
                    "filter_type".to_owned(),
                    glaux_core::ParamValue::Enum("lp24".to_owned()),
                );
                d
            },
            vec![],
        ),
        preset(
            "プラック",
            "音量は少し伸ばしたまま、フィルタだけ素早く閉じる「ポロン」としたシンセのプラック。\
            ハウス・フューチャーベースのコードやアルペジオに。弱く弾くと丸い",
            device(
                "subtractive",
                &[
                    ("unison", 3.0),
                    ("detune", 14.0),
                    ("spread", 0.5),
                    ("analog", 0.2),
                    ("cutoff", 450.0),
                    ("filter_env", 0.85),
                    ("filter_decay", 0.16),
                    ("vel_cutoff", 0.5),
                    ("key_track", 0.5),
                    ("decay", 0.6),
                    ("sustain", 0.25),
                    ("release", 0.25),
                    ("gain_db", -13.0),
                ],
            ),
            vec![fx("reverb", &[("mix", 0.22), ("size", 0.55)])],
        ),
        preset(
            "ビンテージ・パッド",
            "広げた 5 声のパッドを、揺らぎとゆっくりした LFO(カットオフ)で常に少しずつ動かす。\
            止まった音にならない、古いアナログシンセのような温かいパッド",
            {
                let mut d = device(
                    "subtractive",
                    &[
                        ("unison", 5.0),
                        ("detune", 18.0),
                        ("spread", 0.75),
                        ("analog", 0.45),
                        ("cutoff", 1600.0),
                        ("resonance", 0.1),
                        ("filter_env", 0.15),
                        ("key_track", 0.4),
                        ("lfo1_rate", 0.25),
                        ("lfo1_depth", 0.18),
                        ("attack", 0.7),
                        ("sustain", 0.9),
                        ("release", 1.4),
                        ("gain_db", -15.0),
                    ],
                );
                d.params.insert(
                    "lfo1_target".to_owned(),
                    glaux_core::ParamValue::Enum("cutoff".to_owned()),
                );
                d.params.insert(
                    "lfo1_shape".to_owned(),
                    glaux_core::ParamValue::Enum("triangle".to_owned()),
                );
                d
            },
            vec![fx("chorus", &[("mix", 0.25)]), fx("reverb", &[("mix", 0.3), ("size", 0.8)])],
        ),
        preset(
            "ビブラートのリード",
            "矩形波のリードに軽いドライブと、少し遅れて効くようなビブラート(LFO で音程を 5.5Hz)。\
            歌うような単音のメロディーに",
            {
                let mut d = device(
                    "subtractive",
                    &[
                        ("cutoff", 3800.0),
                        ("drive", 0.25),
                        ("filter_env", 0.2),
                        ("vel_cutoff", 0.3),
                        ("analog", 0.25),
                        ("lfo1_rate", 5.5),
                        ("lfo1_depth", 0.05),
                        ("sustain", 0.85),
                        ("release", 0.15),
                        ("gain_db", -13.0),
                    ],
                );
                d.params.insert(
                    "waveform".to_owned(),
                    glaux_core::ParamValue::Enum("square".to_owned()),
                );
                d.params.insert(
                    "lfo1_target".to_owned(),
                    glaux_core::ParamValue::Enum("pitch".to_owned()),
                );
                d
            },
            vec![fx("delay", &[("time_ms", 300.0), ("feedback", 0.25), ("mix", 0.15)])],
        ),
        preset(
            "動くウェーブテーブル・パッド",
            "wavetable(analog)を左右に広げ、2 本の LFO で position と左右をゆっくり動かすパッド。アンビエント・映画の背景に",
            {
                let mut d = wavetable(
                    "analog",
                    &[
                        ("position", 0.35),
                        ("unison", 5.0),
                        ("detune", 16.0),
                        ("spread", 0.8),
                        ("analog", 0.35),
                        ("cutoff", 5000.0),
                        ("lfo1_rate", 0.12),
                        ("lfo1_depth", 0.5),
                        ("lfo2_rate", 0.07),
                        ("lfo2_depth", 0.35),
                        ("attack", 1.2),
                        ("sustain", 0.9),
                        ("release", 2.0),
                        ("gain_db", -14.0),
                    ],
                );
                for (k, v) in [
                    ("lfo1_target", "position"),
                    ("lfo1_shape", "sine"),
                    ("lfo2_target", "pan"),
                    ("lfo2_shape", "triangle"),
                ] {
                    d.params
                        .insert(k.to_owned(), glaux_core::ParamValue::Enum(v.to_owned()));
                }
                d
            },
            vec![fx("reverb", &[("mix", 0.35), ("size", 0.9)])],
        ),
        // ---- 層とマクロの手本 ----
        {
            let mut p = preset(
                "スーパーソウ + サブ",
                "7 声のスーパーソウに 1 オクターブ下のサインを重ねる(層)。サビのリード・コードを太く。マクロ 1「明るさ」でフィルタ、2「厚み」で声の広がり",
                device(
                    "subtractive",
                    &[
                        ("unison", 7.0),
                        ("detune", 25.0),
                        ("cutoff", 5000.0),
                        ("attack", 0.005),
                        ("sustain", 0.85),
                        ("release", 0.3),
                        ("filter_env", 0.15),
                        ("gain_db", -14.0),
                    ],
                ),
                vec![fx("reverb", &[("mix", 0.18), ("size", 0.6)])],
            );
            let mut sub = glaux_core::Layer::new({
                let mut d = device("subtractive", &[("sustain", 1.0), ("release", 0.2), ("gain_db", -12.0)]);
                d.params.insert(
                    "waveform".to_owned(),
                    glaux_core::ParamValue::Enum("sine".to_owned()),
                );
                d
            });
            sub.name = "サブ".to_owned();
            sub.transpose = -12;
            sub.volume_db = -9.0;
            p.layers = vec![sub];
            p.macros = vec![
                glaux_core::Macro {
                    name: "明るさ".to_owned(),
                    value: 0.6,
                    targets: vec![glaux_core::MacroTarget {
                        target: glaux_core::ParamPath::device("cutoff"),
                        min: 800.0,
                        max: 9000.0,
                        curve: 0.4,
                    }],
                },
                glaux_core::Macro {
                    name: "厚み".to_owned(),
                    value: 0.6,
                    targets: vec![glaux_core::MacroTarget {
                        target: glaux_core::ParamPath::device("detune"),
                        min: 5.0,
                        max: 40.0,
                        curve: 0.0,
                    }],
                },
            ];
            p
        },
        {
            let mut p = preset(
                "キックにサブ",
                "ドラムのキック(35・36)にだけ、短いサインのサブ(約 49Hz)を重ねる(層)。クラブの低域を足したいときに。マクロ 1「サブの量」",
                device("drum", &[("gain_db", -6.0), ("kick_decay", 0.8)]),
                vec![],
            );
            let mut sub = glaux_core::Layer::new({
                let mut d = device(
                    "subtractive",
                    &[("attack", 0.002), ("decay", 0.35), ("sustain", 0.0), ("release", 0.08), ("gain_db", -4.0)],
                );
                d.params.insert(
                    "waveform".to_owned(),
                    glaux_core::ParamValue::Enum("sine".to_owned()),
                );
                d
            });
            sub.name = "サブ".to_owned();
            sub.transpose = -5;
            sub.key_lo = 35;
            sub.key_hi = 36;
            p.layers = vec![sub];
            p.macros = vec![glaux_core::Macro {
                name: "キックの長さ".to_owned(),
                value: 0.4,
                targets: vec![glaux_core::MacroTarget {
                    target: glaux_core::ParamPath::device("kick_decay"),
                    min: 0.4,
                    max: 2.0,
                    curve: 0.3,
                }],
            }];
            p
        },
        {
            let mut p = preset(
                "エレピ + パッド",
                "FM のエレピに、ゆっくり立ち上がるパッドを薄く重ねる(層)。バラード・チルの伴奏に。強く弾いたときだけ明るい鐘の音も鳴る",
                device("fm", &[("ratio", 1.0), ("index", 2.0), ("index_decay", 0.6), ("gain_db", -10.0)]),
                vec![fx("chorus", &[("mix", 0.3)]), fx("reverb", &[("mix", 0.2)])],
            );
            let mut pad = glaux_core::Layer::new(device(
                "subtractive",
                &[("unison", 3.0), ("detune", 12.0), ("cutoff", 1800.0), ("attack", 0.6), ("sustain", 0.8), ("release", 0.8), ("gain_db", -14.0)],
            ));
            pad.name = "パッド".to_owned();
            pad.volume_db = -8.0;
            let mut bell = glaux_core::Layer::new(device(
                "fm",
                &[("ratio", 3.5), ("index", 3.0), ("index_decay", 0.3), ("decay", 0.8), ("sustain", 0.0), ("gain_db", -16.0)],
            ));
            bell.name = "鐘(強いときだけ)".to_owned();
            bell.transpose = 12;
            bell.vel_lo = 100;
            bell.volume_db = -6.0;
            p.layers = vec![pad, bell];
            p
        },
    ]
}

/// 出荷時プリセットの版。上げると次回起動時に同名の出荷時プリセットを更新する
/// (ユーザーが独自に作った別名のプリセットには触れない)。
const FACTORY_VERSION: &str = "v9";

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
    let mut new_ids = Vec::new();
    for f in &preset.effects {
        let id = FxId::new();
        new_ids.push(id.clone());
        cmds.push(Command::AddEffect {
            track: track.id.clone(),
            effect: Effect {
                id,
                source: f.source.clone(),
                bypass: f.bypass,
                params: f.params.clone(),
                ui: Default::default(),
            },
            index: None,
        });
    }
    // 層とマクロも丸ごと入れ替える(プリセットに無ければ外す)
    if preset.layers != track.layers {
        cmds.push(Command::SetTrackProp {
            id: track.id.clone(),
            prop: glaux_core::TrackProp::Layers(preset.layers.clone()),
        });
    }
    let macros: Vec<glaux_core::Macro> = preset
        .macros
        .iter()
        .map(|m| {
            let mut m = m.clone();
            for t in &mut m.targets {
                if let glaux_core::ParamPath::Effect { id, .. } = &mut t.target {
                    if let Some(i) = (0..new_ids.len()).find(|&i| *id == slot_id(i)) {
                        *id = new_ids[i].clone();
                    }
                }
            }
            m
        })
        .collect();
    if macros != track.macros {
        cmds.push(Command::SetTrackProp {
            id: track.id.clone(),
            prop: glaux_core::TrackProp::Macros(macros),
        });
        // 前のマクロのオートメーション(macro/N)は意味が変わるので消す
        for lane in &track.automation {
            if matches!(lane.target, glaux_core::ParamPath::Macro { .. }) {
                cmds.push(Command::SetAutomationPoints {
                    track: track.id.clone(),
                    target: lane.target.clone(),
                    points: vec![],
                });
            }
        }
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
            ui: Default::default(),
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
            ui: Default::default(),
        });

        let cmds = apply_commands(&dest, &preset);
        // set_device + 既存 2 削除 + プリセット 1 追加
        assert_eq!(cmds.len(), 4);
        assert!(matches!(cmds[0], Command::SetDevice { .. }));
        assert!(matches!(cmds[1], Command::RemoveEffect { .. }));
        assert!(matches!(cmds[2], Command::RemoveEffect { .. }));
        assert!(matches!(cmds[3], Command::AddEffect { .. }));
    }

    #[test]
    fn noise_presets_bake_to_noise_only_subtractive() {
        let all = factory_presets();
        for name in ["レコードノイズ", "ノイズのライザー", "風"] {
            let p = all.iter().find(|p| p.name == name).expect(name);
            let (_, params) = glaux_dsp::bake_instrument(Some(&p.device));
            let glaux_dsp::InstrumentParams::Subtractive(sp) = params else {
                panic!("{name} は subtractive");
            };
            assert_eq!(sp.osc_level, 0.0, "{name}");
            assert!(sp.noise > 0.0);
        }
        let vinyl = all.iter().find(|p| p.name == "レコードノイズ").unwrap();
        let (_, params) = glaux_dsp::bake_instrument(Some(&vinyl.device));
        let glaux_dsp::InstrumentParams::Subtractive(sp) = params else {
            unreachable!()
        };
        assert_eq!(sp.noise_color, glaux_dsp::NoiseColor::Pink);
        assert!(sp.crackle > 0.2);
        // ライザーは長い立ち上がり(上限 10 秒に広げた)
        let riser = all.iter().find(|p| p.name == "ノイズのライザー").unwrap();
        let (_, params) = glaux_dsp::bake_instrument(Some(&riser.device));
        let glaux_dsp::InstrumentParams::Subtractive(sp) = params else {
            unreachable!()
        };
        assert_eq!(sp.attack, 4.0);
    }

    /// ウェーブテーブルの出荷時プリセットは、低い音で鳴らしても無音にならず、割れもしない
    #[test]
    fn wavetable_factory_presets_sound() {
        use glaux_core::{Project, TrackId, TrackKind};
        for preset in factory_presets().into_iter().filter(
            |p| matches!(&p.device.source, PluginSource::Builtin { name } if name == "wavetable"),
        ) {
            let mut project = Project::new("t");
            let track = Track::new(TrackId::new(), "W", TrackKind::Midi);
            project.tracks.push(track.clone());
            for c in apply_commands(&track, &preset) {
                project
                    .apply(&c)
                    .unwrap_or_else(|e| panic!("{}: {e}", preset.name));
            }
            let out = glaux_engine::export::render_track_note(
                &project,
                &track.id,
                45,
                110,
                1.0,
                48_000.0,
                &Default::default(),
            )
            .unwrap();
            let body = &out[out.len() / 8..out.len() / 2];
            let rms = (body.iter().map(|v| v * v).sum::<f32>() / body.len() as f32).sqrt();
            let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(rms > 0.01, "{}: 小さすぎる {rms}", preset.name);
            assert!(peak < 1.0, "{}: 割れる {peak}", preset.name);
        }
    }

    #[test]
    fn factory_presets_use_known_effects_and_params() {
        for p in factory_presets() {
            for fx in &p.effects {
                let PluginSource::Builtin { name } = &fx.source else {
                    continue;
                };
                let specs = glaux_dsp::effect_params_spec(name)
                    .unwrap_or_else(|| panic!("{}: 知らないエフェクト {name}", p.name));
                for key in fx.params.keys() {
                    assert!(
                        specs.iter().any(|s| s.name == key.as_str()),
                        "{}: {name} に {key} は無い",
                        p.name
                    );
                }
            }
            // 本体と層の音源のつまみ
            for d in std::iter::once(&p.device).chain(p.layers.iter().map(|l| &l.device)) {
                if let PluginSource::Builtin { name } = &d.source {
                    let specs = glaux_dsp::instrument_params(name).expect("音源");
                    for key in d.params.keys() {
                        assert!(
                            specs.iter().any(|s| s.name == key.as_str()),
                            "{}: {name} に {key} は無い",
                            p.name
                        );
                    }
                }
            }
            glaux_core::check_layers(&p.layers).unwrap_or_else(|e| panic!("{}: {e}", p.name));
            if !p.macros.is_empty() {
                glaux_core::check_macros(&p.macros).unwrap_or_else(|e| panic!("{}: {e}", p.name));
            }
            // マクロの先は本体の音源のつまみ
            for m in &p.macros {
                for t in &m.targets {
                    if let glaux_core::ParamPath::Device { name: key } = &t.target {
                        let PluginSource::Builtin { name } = &p.device.source else {
                            panic!("{}", p.name)
                        };
                        let specs = glaux_dsp::instrument_params(name).unwrap();
                        assert!(
                            specs.iter().any(|s| s.name == key.as_str()),
                            "{}: マクロの先 {key} は {name} に無い",
                            p.name
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn layers_and_macros_travel_with_presets() {
        let tmp = tempfile::tempdir().unwrap();
        let mut t = track_with_patch();
        let fx_id = t.effects[0].id.clone();
        t.layers = vec![glaux_core::Layer::new(Device::builtin("fm"))];
        t.macros = vec![glaux_core::Macro {
            name: "歪み".into(),
            value: 0.3,
            targets: vec![
                glaux_core::MacroTarget {
                    target: glaux_core::ParamPath::effect(fx_id.clone(), "drive"),
                    min: 0.0,
                    max: 1.0,
                    curve: 0.0,
                },
                glaux_core::MacroTarget {
                    target: glaux_core::ParamPath::device("cutoff"),
                    min: 500.0,
                    max: 5000.0,
                    curve: 0.0,
                },
            ],
        }];
        let saved = save(tmp.path(), &t, "層つき", None, false).unwrap();
        // エフェクトの先は並びの番号に
        assert_eq!(
            saved.macros[0].targets[0].target.to_string(),
            "fx/fx_slot0/drive"
        );
        let loaded = load(tmp.path(), "層つき").unwrap();
        let other = Track::new(
            glaux_core::TrackId::new(),
            "Other",
            glaux_core::TrackKind::Midi,
        );
        let cmds = apply_commands(&other, &loaded);
        let new_fx = cmds
            .iter()
            .find_map(|c| match c {
                Command::AddEffect { effect, .. } => Some(effect.id.clone()),
                _ => None,
            })
            .unwrap();
        let macros = cmds
            .iter()
            .find_map(|c| match c {
                Command::SetTrackProp {
                    prop: glaux_core::TrackProp::Macros(m),
                    ..
                } => Some(m.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            macros[0].targets[0].target,
            glaux_core::ParamPath::effect(new_fx, "drive"),
            "新しいエフェクトの ID に戻る"
        );
        assert!(cmds.iter().any(|c| matches!(
            c,
            Command::SetTrackProp {
                prop: glaux_core::TrackProp::Layers(l),
                ..
            } if l.len() == 1
        )));
        // 層もマクロも無いプリセットを、層のあるトラックに当てると外れる
        let plain = save(tmp.path(), &track_with_patch(), "素", None, false).unwrap();
        let cmds = apply_commands(&t, &plain);
        assert!(cmds.iter().any(|c| matches!(
            c,
            Command::SetTrackProp {
                prop: glaux_core::TrackProp::Layers(l),
                ..
            } if l.is_empty()
        )));
    }
}
