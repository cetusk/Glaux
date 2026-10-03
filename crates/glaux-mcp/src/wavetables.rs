//! ウェーブテーブルを作り込む道具の中身(make_wavetable / describe_wavetable / wavetable_library)。
//!
//! - 元(source): 定番の変化・倍音の設計図・音声・棚・トラックの今のテーブル・素材
//! - 加工(edits): [`glaux_dsp::wtedit::EditOp`] と、ほかのテーブルを混ぜる・つなぐ(mix / concat)
//! - 作ったテーブルは浮動小数のモノラル WAV(1 周期 2048 点の並び)としてプロジェクトの audio/ に入れ、
//!   wavetable の table に素材の ID を入れる(取り消せる)。作り方の手順は素材の隣の `.recipe.json` に残す
//! - 棚: 全プロジェクト共通の置き場(`%APPDATA%\glaux\wavetables` / `~/.config/glaux/wavetables`)。
//!   `<名前>.wav`(配布形式。Serum などでも読める)と `<名前>.json`(手順・メモ)

use glaux_core::{AssetId, Command, Device, ParamValue, PluginSource, Project, Track, TrackKind};
use glaux_dsp::wtedit::{self, Cycles, EditOp, CYCLE};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// 元を読むときに使うもの
pub struct Ctx<'a> {
    pub project: &'a Project,
    pub project_dir: &'a Path,
    /// 「今のテーブル」を読むトラック
    pub track: Option<&'a Track>,
}

/// 今のテーブルが内蔵のものだったときに並べる枚数
const BUILTIN_FRAMES: usize = 16;

/// 元を読む。`frames` は作る元(shape / harmonics / audio)の枚数
pub fn resolve_source(src: &Value, frames: usize, ctx: &Ctx) -> Result<Cycles, String> {
    let kind = src
        .get("kind")
        .and_then(Value::as_str)
        .ok_or("source には kind(shape / harmonics / audio / library / current / asset)を")?;
    match kind {
        "shape" | "harmonics" => {
            let s: wtedit::TableSource = serde_json::from_value(src.clone())
                .map_err(|e| format!("source が読めません: {e}"))?;
            wtedit::generate(&s, frames)
        }
        "audio" => {
            let path = src
                .get("path")
                .and_then(Value::as_str)
                .ok_or("audio の source には path(音声ファイルの絶対パス)を")?;
            let (mono, sr) = crate::assets::decode_mono(Path::new(path))?;
            glaux_dsp::cycles_from_audio(&mono, sr, frames)
        }
        "library" => {
            let name = src
                .get("name")
                .and_then(Value::as_str)
                .ok_or("library の source には name を")?;
            load_library(name).map(|(c, _)| c)
        }
        "current" => {
            let t = ctx.track.ok_or("current にはトラックが要ります")?;
            current_cycles(t, ctx.project, ctx.project_dir)
        }
        "asset" => {
            let id = src
                .get("id")
                .and_then(Value::as_str)
                .ok_or("asset の source には id(sha256:…)を")?;
            let id = AssetId::parse(id).map_err(|e| e.to_string())?;
            asset_cycles(&id, ctx.project, ctx.project_dir)
        }
        other => Err(format!(
            "source の kind は shape / harmonics / audio / library / current / asset(got: {other})"
        )),
    }
}

/// 加工の手順を当てる(mix / concat はほかのテーブルを読む)
pub fn apply_steps(cycles: &mut Cycles, steps: &[Value], ctx: &Ctx) -> Result<(), String> {
    for (i, step) in steps.iter().enumerate() {
        let op = step.get("op").and_then(Value::as_str).unwrap_or("");
        let err = |e: String| format!("edits[{i}]({op}): {e}");
        match op {
            "mix" | "concat" => {
                let with = step
                    .get("with")
                    .ok_or_else(|| err("with(元)を".to_owned()))?;
                let other =
                    resolve_source(with, wtedit::frame_count(cycles).max(2), ctx).map_err(err)?;
                *cycles = if op == "mix" {
                    let amount = step.get("amount").and_then(Value::as_f64).unwrap_or(0.5) as f32;
                    wtedit::mix(cycles, &other, amount)
                } else {
                    wtedit::concat(cycles, &other)
                };
            }
            _ => {
                let e: EditOp = serde_json::from_value(step.clone()).map_err(|e| {
                    err(format!(
                        "読めません({e})。op は normalize / remove_dc / tilt / odd_even / band / lowpass / \
                         phase / smooth / reverse / resize / select / saturate / fold / mix / concat"
                    ))
                })?;
                wtedit::apply_edits(cycles, &[e]).map_err(err)?;
            }
        }
    }
    Ok(())
}

/// 作り方の手順(元・枚数・加工)からテーブルを作る
pub fn build(recipe: &Value, ctx: &Ctx) -> Result<Cycles, String> {
    let frames = recipe
        .get("frames")
        .and_then(Value::as_u64)
        .unwrap_or(64)
        .clamp(2, wtedit::MAX_FRAMES as u64) as usize;
    let src = recipe.get("source").ok_or("source を")?;
    let mut c = resolve_source(src, frames, ctx)?;
    if let Some(steps) = recipe.get("edits").and_then(Value::as_array) {
        apply_steps(&mut c, steps, ctx)?;
    }
    if c.iter().any(|v| !v.is_finite()) {
        return Err("作ったテーブルに不正な値があります".to_owned());
    }
    glaux_dsp::UserTable::from_cycles(&c).ok_or("鳴らせる形になりません(無音)")?;
    Ok(c)
}

/// トラックの音源が wavetable なら、その table の値(内蔵の名前か素材の ID)
fn table_value(t: &Track) -> Option<String> {
    let d = t.device.as_ref()?;
    match (&d.source, d.params.get("table")) {
        (PluginSource::Builtin { name }, Some(ParamValue::Enum(v))) if name == "wavetable" => {
            Some(v.clone())
        }
        (PluginSource::Builtin { name }, None) if name == "wavetable" => Some("analog".to_owned()),
        _ => None,
    }
}

/// トラックの今のテーブル
pub fn current_cycles(t: &Track, project: &Project, dir: &Path) -> Result<Cycles, String> {
    let v =
        table_value(t).ok_or_else(|| format!("「{}」の音源は wavetable ではありません", t.name))?;
    if let Some(i) = glaux_dsp::TABLE_NAMES.iter().position(|n| *n == v) {
        let mut out = Vec::with_capacity(BUILTIN_FRAMES * CYCLE);
        for k in 0..BUILTIN_FRAMES {
            out.extend(glaux_dsp::builtin_cycle(
                i,
                k as f32 / (BUILTIN_FRAMES - 1) as f32,
            ));
        }
        return Ok(out);
    }
    let id = AssetId::parse(&v).map_err(|e| format!("table が読めません({v}): {e}"))?;
    asset_cycles(&id, project, dir)
}

/// 素材のテーブル(1 周期 2048 点の並び)
pub fn asset_cycles(id: &AssetId, project: &Project, dir: &Path) -> Result<Cycles, String> {
    let a = project
        .assets
        .get(id)
        .ok_or_else(|| format!("素材がありません: {id}"))?;
    let d = glaux_engine::load_wav_mono(&dir.join(&a.path))?;
    if d.frames.len() < CYCLE || d.frames.len() % CYCLE != 0 {
        return Err("素材がウェーブテーブルの形(2048 点の倍数)ではありません".to_owned());
    }
    Ok(d.frames)
}

/// テーブルを素材として書き、手順を隣に残す
pub fn write_asset(
    dir: &Path,
    cycles: &[f32],
    recipe: Option<&Value>,
) -> Result<crate::assets::ImportedSample, String> {
    let tmp_dir = dir.join("cache");
    std::fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;
    let tmp = tmp_dir.join(format!(
        "wavetable-{}-{}.wav",
        std::process::id(),
        cycles.len()
    ));
    std::fs::write(&tmp, wtedit::to_wav_bytes(cycles)).map_err(|e| e.to_string())?;
    let imported = crate::assets::import_wav(dir, &tmp);
    let _ = std::fs::remove_file(&tmp);
    let imported = imported?;
    if let Some(r) = recipe {
        let p = recipe_path(dir, &imported.asset.path);
        let _ = std::fs::write(p, serde_json::to_vec_pretty(r).unwrap_or_default());
    }
    Ok(imported)
}

fn recipe_path(dir: &Path, asset_path: &str) -> PathBuf {
    dir.join(asset_path).with_extension("recipe.json")
}

/// 素材のテーブルの作り方(残っていれば)
pub fn asset_recipe(project: &Project, dir: &Path, id: &AssetId) -> Option<Value> {
    let a = project.assets.get(id)?;
    let bytes = std::fs::read(recipe_path(dir, &a.path)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// トラックの音源のテーブルを素材に差し替えるコマンド(素材が未登録なら登録も)。
/// 音源が wavetable なら他のつまみはそのまま、そうでなければ wavetable にする
pub fn set_table_commands(
    project: &Project,
    track: &Track,
    imported: &crate::assets::ImportedSample,
) -> Result<Vec<Command>, String> {
    if track.kind != TrackKind::Midi {
        return Err(format!("「{}」は MIDI トラックではありません", track.name));
    }
    let mut device = match &track.device {
        Some(d) if matches!(&d.source, PluginSource::Builtin { name } if name == "wavetable") => {
            d.clone()
        }
        _ => {
            let mut d = Device::builtin("wavetable");
            // 新しく選んだときと同じ「生きた音」寄りの初期値
            d.params.insert("analog".to_owned(), ParamValue::Float(0.2));
            d.params.insert("spread".to_owned(), ParamValue::Float(0.5));
            d
        }
    };
    device.params.insert(
        "table".to_owned(),
        ParamValue::Enum(imported.id.to_string()),
    );
    let mut cmds = Vec::new();
    if !project.assets.contains_key(&imported.id) {
        cmds.push(Command::AddAsset {
            id: imported.id.clone(),
            asset: imported.asset.clone(),
        });
    }
    cmds.push(Command::SetDevice {
        track: track.id.clone(),
        device: Some(device),
    });
    Ok(cmds)
}

/// 要約を JSON に
pub fn summary_json(cycles: &[f32], rows: usize) -> Value {
    serde_json::to_value(wtedit::describe(cycles, rows)).unwrap_or(Value::Null)
}

// ---- 棚 ----

/// 棚の置き場
pub fn library_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("GLAUX_WAVETABLE_DIR") {
        return PathBuf::from(d);
    }
    crate::presets::default_dir()
        .parent()
        .map(|p| p.join("wavetables"))
        .unwrap_or_else(|| PathBuf::from("wavetables"))
}

/// 棚の 1 つの情報
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LibraryEntry {
    pub name: String,
    pub frames: usize,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// 作り方(残っていれば)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe: Option<Value>,
    #[serde(default)]
    pub created: String,
}

fn check_name(name: &str) -> Result<(), String> {
    let n = name.trim();
    if n.is_empty() || n.len() > 80 {
        return Err("name は 1〜80 文字".to_owned());
    }
    if n.chars().any(|c| {
        matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()
    }) || n.starts_with('.')
    {
        return Err(format!("name にファイル名に使えない文字があります: {n}"));
    }
    Ok(())
}

/// 棚に置く(同じ名前は上書き)
pub fn save_library(
    name: &str,
    cycles: &[f32],
    note: &str,
    recipe: Option<Value>,
) -> Result<LibraryEntry, String> {
    check_name(name)?;
    let dir = library_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(
        dir.join(format!("{name}.wav")),
        wtedit::to_wav_bytes(cycles),
    )
    .map_err(|e| e.to_string())?;
    let entry = LibraryEntry {
        name: name.to_owned(),
        frames: wtedit::frame_count(cycles),
        note: note.to_owned(),
        recipe,
        created: chrono_now(),
    };
    std::fs::write(
        dir.join(format!("{name}.json")),
        serde_json::to_vec_pretty(&entry).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(entry)
}

fn chrono_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}

/// 棚から読む
pub fn load_library(name: &str) -> Result<(Cycles, Option<LibraryEntry>), String> {
    check_name(name)?;
    let dir = library_dir();
    let wav = dir.join(format!("{name}.wav"));
    if !wav.is_file() {
        return Err(format!(
            "棚にありません: {name}(wavetable_library の list で確かめる)"
        ));
    }
    let d = glaux_engine::load_wav_mono(&wav)?;
    if d.frames.len() < CYCLE || d.frames.len() % CYCLE != 0 {
        return Err(format!(
            "{name} はウェーブテーブルの形(2048 点の倍数)ではありません"
        ));
    }
    let meta = std::fs::read(dir.join(format!("{name}.json")))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    Ok((d.frames, meta))
}

/// 棚の一覧(名前順。WAV だけ置かれたもの = 配布のテーブルを置いたものも並べる)
pub fn list_library() -> Vec<LibraryEntry> {
    let dir = library_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return vec![];
    };
    let mut out: Vec<LibraryEntry> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension()?.to_str()? != "wav" {
                return None;
            }
            let name = p.file_stem()?.to_string_lossy().into_owned();
            let meta: Option<LibraryEntry> = std::fs::read(dir.join(format!("{name}.json")))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok());
            Some(match meta {
                Some(mut m) => {
                    m.recipe = None;
                    m
                }
                None => {
                    let frames = e
                        .metadata()
                        .map(|m| (m.len() as usize).saturating_sub(44) / 4 / CYCLE)
                        .unwrap_or(0);
                    LibraryEntry {
                        name,
                        frames,
                        note: String::new(),
                        recipe: None,
                        created: String::new(),
                    }
                }
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// 棚から消す
pub fn delete_library(name: &str) -> Result<(), String> {
    check_name(name)?;
    let dir = library_dir();
    let wav = dir.join(format!("{name}.wav"));
    if !wav.is_file() {
        return Err(format!("棚にありません: {name}"));
    }
    std::fs::remove_file(&wav).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(dir.join(format!("{name}.json")));
    Ok(())
}

/// 配布形式の WAV に書き出す
pub fn export_wav(cycles: &[f32], path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, wtedit::to_wav_bytes(cycles))
        .map_err(|e| format!("書き出せません({}): {e}", path.display()))
}

/// 道具の説明に使う、定番の変化の一覧
pub fn shapes_json() -> Value {
    json!(wtedit::SHAPES
        .iter()
        .map(|(n, d)| json!({ "name": n, "description": d }))
        .collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(p: &'a Project, dir: &'a Path, t: Option<&'a Track>) -> Ctx<'a> {
        Ctx {
            project: p,
            project_dir: dir,
            track: t,
        }
    }

    #[test]
    fn recipe_with_mix_and_edits_builds_and_writes_an_asset() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Project::new("t");
        let r = json!({
            "source": { "kind": "shape", "name": "growl" },
            "frames": 32,
            "edits": [
                { "op": "mix", "with": { "kind": "shape", "name": "fm" }, "amount": 0.3 },
                { "op": "smooth", "amount": 0.5 },
                { "op": "tilt", "db_per_octave": -1.5 },
                { "op": "normalize", "per_frame": true }
            ]
        });
        let c = build(&r, &ctx(&p, tmp.path(), None)).unwrap();
        assert_eq!(wtedit::frame_count(&c), 32);
        let imported = write_asset(tmp.path(), &c, Some(&r)).unwrap();
        assert!(tmp.path().join(&imported.asset.path).is_file());
        // 素材を登録すると、手順と中身を読み戻せる
        let mut p2 = p.clone();
        p2.assets
            .insert(imported.id.clone(), imported.asset.clone());
        assert_eq!(asset_recipe(&p2, tmp.path(), &imported.id).unwrap(), r);
        let back = asset_cycles(&imported.id, &p2, tmp.path()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn bad_steps_say_which_one() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Project::new("t");
        let r = json!({ "source": { "kind": "shape", "name": "fm" }, "edits": [ { "op": "tilt", "db_per_octave": 1 }, { "op": "explode" } ] });
        let e = build(&r, &ctx(&p, tmp.path(), None)).unwrap_err();
        assert!(e.contains("edits[1]"), "{e}");
        let r = json!({ "source": { "kind": "nope" } });
        assert!(build(&r, &ctx(&p, tmp.path(), None)).is_err());
    }

    #[test]
    fn current_table_of_a_builtin_track_is_read_as_cycles() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Project::new("t");
        let mut t = Track::new(glaux_core::TrackId::new(), "W", TrackKind::Midi);
        let mut d = Device::builtin("wavetable");
        d.params
            .insert("table".into(), ParamValue::Enum("sync".into()));
        t.device = Some(d);
        let c = current_cycles(&t, &p, tmp.path()).unwrap();
        assert_eq!(wtedit::frame_count(&c), BUILTIN_FRAMES);
        // 音源が wavetable でなければ失敗
        let plain = Track::new(glaux_core::TrackId::new(), "S", TrackKind::Midi);
        assert!(current_cycles(&plain, &p, tmp.path()).is_err());
    }

    #[test]
    fn library_saves_lists_loads_and_deletes() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("GLAUX_WAVETABLE_DIR", tmp.path().join("lib"));
        let c = wtedit::generate(&wtedit::TableSource::Shape { name: "pwm".into() }, 8).unwrap();
        save_library("細いパルス", &c, "PWM の練習", Some(json!({"x": 1}))).unwrap();
        assert!(save_library("../x", &c, "", None).is_err());
        let l = list_library();
        assert_eq!(l.len(), 1);
        assert_eq!((l[0].name.as_str(), l[0].frames), ("細いパルス", 8));
        let (back, meta) = load_library("細いパルス").unwrap();
        assert_eq!(back, c);
        assert_eq!(meta.unwrap().recipe.unwrap(), json!({"x": 1}));
        delete_library("細いパルス").unwrap();
        assert!(list_library().is_empty());
        assert!(load_library("細いパルス").is_err());
    }
}
