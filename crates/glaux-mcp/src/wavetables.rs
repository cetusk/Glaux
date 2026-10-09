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
        // 内蔵のテーブル(analog・growl など)を枚数ぶん並べる(位置の間は内蔵と同じに補う)
        "builtin" => {
            let name = src
                .get("name")
                .and_then(Value::as_str)
                .ok_or("builtin の source には name を")?;
            let i = glaux_dsp::TABLE_NAMES
                .iter()
                .position(|n| *n == name)
                .ok_or_else(|| format!("内蔵のテーブルにありません: {name}"))?;
            let frames = frames.clamp(2, wtedit::MAX_FRAMES);
            let mut out = Vec::with_capacity(frames * CYCLE);
            for k in 0..frames {
                out.extend(glaux_dsp::builtin_cycle(i, k as f32 / (frames - 1) as f32));
            }
            Ok(out)
        }
        // 曲の中の音声(素材)から 1 周期ずつ切り出す。from / to は使う範囲(素材の長さの割合)
        "audio_asset" => {
            let id = src
                .get("id")
                .and_then(Value::as_str)
                .ok_or("audio_asset の source には id(sha256:…)を")?;
            let id = AssetId::parse(id).map_err(|e| e.to_string())?;
            let a = ctx
                .project
                .assets
                .get(&id)
                .ok_or_else(|| format!("素材がありません: {id}"))?;
            let d = glaux_engine::load_wav_mono(&ctx.project_dir.join(&a.path))?;
            let n = d.frames.len();
            let from = src.get("from").and_then(Value::as_f64).unwrap_or(0.0).clamp(0.0, 1.0);
            let to = src.get("to").and_then(Value::as_f64).unwrap_or(1.0).clamp(0.0, 1.0);
            let (a0, b0) = if from <= to { (from, to) } else { (to, from) };
            let (i0, i1) = ((a0 * n as f64) as usize, ((b0 * n as f64) as usize).max((a0 * n as f64) as usize + 1).min(n));
            glaux_dsp::cycles_from_audio(&d.frames[i0..i1], d.sample_rate, frames)
        }
        // 数式(x = 1 周期の中の位置 0〜1、t = テーブルの位置 0〜1)
        "expr" => {
            let f = src
                .get("formula")
                .and_then(Value::as_str)
                .ok_or("expr の source には formula(例「sin(2*pi*x) + t*0.5*saw(3*x)」)を")?;
            wtedit::expr_frames(f, frames)
        }
        other => Err(format!(
            "source の kind は shape / harmonics / expr / audio / library / current / recipe / asset / builtin / audio_asset(got: {other})"
        )),
    }
}

/// 加工の手順を当てる(mix / concat はほかのテーブルを読む)
pub fn apply_steps(cycles: &mut Cycles, steps: &[Value], ctx: &Ctx) -> Result<(), String> {
    for (i, step) in steps.iter().enumerate() {
        // 切った加工(音色エディタの入り切り)は飛ばす
        if step.get("on").and_then(Value::as_bool) == Some(false) {
            continue;
        }
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
                         phase / smooth / reverse / resize / select / saturate / fold / formant_shift / spectral_blur / \
                         phase_distort / sync / mix / concat"
                    ))
                })?;
                match fade_of(step).map_err(err)? {
                    // 位置ごとに効き方を変える: 加工した並びと元の並びを、位置 0 で a・位置 1 で b の割合で混ぜる
                    Some((a, b)) => {
                        let mut done = cycles.clone();
                        wtedit::apply_edits(&mut done, &[e]).map_err(err)?;
                        let n = wtedit::frame_count(cycles);
                        if wtedit::frame_count(&done) != n {
                            return Err(err(
                                "枚数が変わる加工(resize・select)には fade を付けられません"
                                    .to_owned(),
                            ));
                        }
                        for k in 0..n {
                            let t = if n > 1 {
                                k as f32 / (n - 1) as f32
                            } else {
                                0.0
                            };
                            let w = a + (b - a) * t;
                            let (o, d) = (
                                &mut cycles[k * CYCLE..(k + 1) * CYCLE],
                                &done[k * CYCLE..(k + 1) * CYCLE],
                            );
                            for (x, y) in o.iter_mut().zip(d) {
                                *x += (*y - *x) * w;
                            }
                        }
                    }
                    None => wtedit::apply_edits(cycles, &[e]).map_err(err)?,
                }
            }
        }
    }
    Ok(())
}

/// 加工の `fade: [a, b]`(位置 0 での効き方 a、位置 1 での効き方 b。0〜1)。無ければ None
fn fade_of(step: &Value) -> Result<Option<(f32, f32)>, String> {
    match step.get("fade") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(v)) if v.len() == 2 => {
            let f = |x: &Value| x.as_f64().map(|v| v.clamp(0.0, 1.0) as f32);
            match (f(&v[0]), f(&v[1])) {
                (Some(a), Some(b)) => Ok(Some((a, b))),
                _ => {
                    Err("fade は [位置 0 での効き方, 位置 1 での効き方](0〜1 の数 2 つ)".to_owned())
                }
            }
        }
        _ => Err("fade は [位置 0 での効き方, 位置 1 での効き方](0〜1 の数 2 つ)".to_owned()),
    }
}

/// 手で編集した波形 1 つ: {pos, amps, phases?}(倍音)か {pos, points: [[x, y], …]}(1 周期を点の並びで。x 0〜1・y −1〜1)
pub fn key_of(k: &Value) -> Result<wtedit::HarmonicKey, String> {
    if let Some(pts) = k.get("points") {
        let pos = k.get("pos").and_then(Value::as_f64).unwrap_or(0.0) as f32;
        let pts: Vec<(f32, f32)> = serde_json::from_value::<Vec<(f32, f32)>>(pts.clone())
            .map_err(|e| format!("points は [[x, y], …](x 0〜1・y −1〜1): {e}"))?;
        return wtedit::key_from_points(pos, &pts);
    }
    serde_json::from_value::<wtedit::HarmonicKey>(k.clone())
        .map_err(|e| format!("keys が読めません: {e}"))
}

/// 手で編集した波形(手順の keys)。位相は回転数(0〜1)
fn recipe_keys(recipe: &Value) -> Result<Vec<wtedit::HarmonicKey>, String> {
    match recipe.get("keys") {
        Some(Value::Array(a)) if !a.is_empty() => a.iter().map(key_of).collect(),
        _ => Ok(vec![]),
    }
}

/// 加工の前の表: 元(source)に、手で編集した波形(keys)を重ねる。
/// 倍音から作る(source が harmonics で keys を持たない)ときは、keys がそのまま倍音の設計図になる
/// (編集した波形の間を混ぜてつなぐ。無ければサイン波から始める)。ほかの元は、なじませる幅(blend)で前後を寄せる
pub fn build_pre(recipe: &Value, ctx: &Ctx) -> Result<Cycles, String> {
    let frames = recipe
        .get("frames")
        .and_then(Value::as_u64)
        .unwrap_or(64)
        .clamp(2, wtedit::MAX_FRAMES as u64) as usize;
    let src = recipe.get("source").ok_or("source を")?;
    let keys = recipe_keys(recipe)?;
    let harmonics = src.get("kind").and_then(Value::as_str) == Some("harmonics");
    if harmonics && src.get("keys").is_none() {
        let keys = if keys.is_empty() {
            vec![wtedit::HarmonicKey {
                pos: 0.0,
                amps: vec![1.0],
                phases: None,
            }]
        } else {
            keys
        };
        return wtedit::generate(&wtedit::TableSource::Harmonics { keys }, frames);
    }
    let mut c = resolve_source(src, frames, ctx)?;
    if !keys.is_empty() {
        let blend = recipe.get("blend").and_then(Value::as_f64).unwrap_or(0.15) as f32;
        wtedit::overlay_keys(&mut c, &keys, blend.clamp(0.0, 0.5));
    }
    Ok(c)
}

/// 作り方の手順(元・手で編集した波形・枚数・加工)からテーブルを作る
pub fn build(recipe: &Value, ctx: &Ctx) -> Result<Cycles, String> {
    let mut c = build_pre(recipe, ctx)?;
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
    let rc = recipe.map(|r| serde_json::to_vec(r).unwrap_or_default());
    std::fs::write(
        &tmp,
        wtedit::to_wav_bytes_with_recipe(cycles, rc.as_deref()),
    )
    .map_err(|e| e.to_string())?;
    let imported = crate::assets::import_wav(dir, &tmp);
    let _ = std::fs::remove_file(&tmp);
    let imported = imported?;
    if let Some(r) = recipe {
        let p = recipe_path(dir, &imported.asset.path);
        let _ = std::fs::write(p, serde_json::to_vec_pretty(r).unwrap_or_default());
    }
    Ok(imported)
}

/// 試聴用の仮のテーブル(音色エディタでドラッグしている間)を cache/ に書き、素材として登録する形を返す。
/// 履歴には残さない(preview_edit で当てるだけ)。古い仮のテーブルは新しい 4 つを残して消す
pub fn write_preview(dir: &Path, cycles: &[f32]) -> Result<(AssetId, glaux_core::Asset), String> {
    use sha2::Digest;
    let bytes = wtedit::to_wav_bytes(cycles);
    let hex = format!("{:x}", sha2::Sha256::digest(&bytes));
    let id = AssetId::from_sha256_hex(&hex).map_err(|e| e.to_string())?;
    let rel = format!("cache/wt-preview-{hex}.wav");
    let cache = dir.join("cache");
    std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    let dest = dir.join(&rel);
    if !dest.is_file() {
        std::fs::write(&dest, &bytes).map_err(|e| e.to_string())?;
    }
    if let Ok(rd) = std::fs::read_dir(&cache) {
        let mut olds: Vec<(std::time::SystemTime, PathBuf)> = rd
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("wt-preview-"))
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .collect();
        olds.sort_by_key(|o| std::cmp::Reverse(o.0));
        for (_, p) in olds.into_iter().skip(4) {
            if p != dest {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    Ok((
        id,
        glaux_core::Asset {
            path: rel,
            sample_rate: 48_000,
            channels: 1,
            frames: cycles.len() as u64,
        },
    ))
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

/// 音色エディタの絵の材料: 重ねた絵(加工の後の `stack` 枚を `stack_points` 点に間引く)と、
/// 選んだ 1 枚の加工の前(`pos_src` の位置)と後(`pos_out`)の波形(`points` 点)と倍音(`harm` 本の振幅と位相〈ラジアン〉)。
/// どの波形も、いちばん大きい所を 0.9 にそろえて返す(形を見るため)
#[allow(clippy::too_many_arguments)]
pub fn view(
    recipe: &Value,
    ctx: &Ctx,
    pos_out: f32,
    pos_src: f32,
    stack: usize,
    stack_points: usize,
    points: usize,
    harm: usize,
) -> Result<Value, String> {
    let pre = build_pre(recipe, ctx)?;
    let mut post = pre.clone();
    if let Some(steps) = recipe.get("edits").and_then(Value::as_array) {
        apply_steps(&mut post, steps, ctx)?;
    }
    let fp = wtedit::frame_count(&pre).max(1);
    let fo = wtedit::frame_count(&post).max(1);
    let frame = |c: &[f32], f: usize, pos: f32| -> Vec<f32> {
        let i = ((pos.clamp(0.0, 1.0) * (f - 1) as f32).round() as usize).min(f - 1);
        let cyc = &c[i * CYCLE..(i + 1) * CYCLE];
        let peak = cyc.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let k = if peak > 1e-6 { 0.9 / peak } else { 1.0 };
        cyc.iter().map(|v| v * k).collect()
    };
    let decimate =
        |cyc: &[f32], n: usize| -> Vec<f32> { (0..n).map(|j| cyc[j * CYCLE / n.max(1)]).collect() };
    let one = |cyc: Vec<f32>| {
        let (amps, phases) = wtedit::harmonics_of(&cyc, harm);
        json!({ "wave": decimate(&cyc, points), "amps": amps, "phases": phases })
    };
    let show = stack.clamp(1, fo);
    let stack_waves: Vec<Vec<f32>> = (0..show)
        .map(|j| {
            let t = if show > 1 {
                j as f32 / (show - 1) as f32
            } else {
                0.0
            };
            decimate(&frame(&post, fo, t), stack_points)
        })
        .collect();
    Ok(json!({
        "frames": fo,
        "pre_frames": fp,
        "stack": stack_waves,
        "pre": one(frame(&pre, fp, pos_src)),
        "post": one(frame(&post, fo, pos_out)),
    }))
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

#[cfg(test)]
mod editor_recipe_tests {
    use super::*;

    fn ctx(p: &Project) -> Ctx<'_> {
        Ctx {
            project: p,
            project_dir: Path::new("."),
            track: None,
        }
    }

    /// よくある形の上に手で編集した波形を重ねると、その位置の 1 枚が編集した形になる。切った加工は掛からない
    #[test]
    fn keys_overlay_shape_and_off_edits_are_skipped() {
        let p = Project::new("t");
        let key = json!({ "pos": 0.5, "amps": [0.0, 1.0] });
        let r = json!({ "source": { "kind": "shape", "name": "sine_to_saw" }, "frames": 9,
                        "keys": [key], "blend": 0.2,
                        "edits": [{ "op": "band", "from": 2, "to": 2, "gain_db": -100.0, "on": false, "who": "you", "id": 1 }] });
        let c = build(&r, &ctx(&p)).unwrap();
        let mid = &c[4 * CYCLE..5 * CYCLE];
        let (a, _) = wtedit::harmonics_of(mid, 3);
        assert!(a[0] < 1e-3 && a[1] > 0.5, "編集した形(2 倍音だけ): {a:?}");
        // 加工を入れると 2 倍音が消える
        let mut r2 = r.clone();
        r2["edits"][0]["on"] = json!(true);
        let c2 = build(&r2, &ctx(&p)).unwrap();
        let (a2, _) = wtedit::harmonics_of(&c2[4 * CYCLE..5 * CYCLE], 3);
        assert!(a2[1] < 1e-3, "{a2:?}");
    }

    /// 式の元・点で描いた形・位置ごとの効き方(fade)
    #[test]
    fn expr_source_point_keys_and_fade() {
        let p = Project::new("t");
        // 式: t で 3 倍音が増える
        let r = json!({ "source": { "kind": "expr", "formula": "sin(2*pi*x) + t*sin(6*pi*x)" }, "frames": 5 });
        let c = build(&r, &ctx(&p)).unwrap();
        let (a, _) = wtedit::harmonics_of(&c[4 * CYCLE..], 4);
        assert!(a[2] > 0.3 * a[0], "{a:?}");
        assert!(build(
            &json!({ "source": { "kind": "expr", "formula": "sin(" } }),
            &ctx(&p)
        )
        .is_err());
        // 点で描いた矩形を真ん中に重ねる
        let r = json!({ "source": { "kind": "shape", "name": "sine_to_saw" }, "frames": 9, "blend": 0.1,
                        "keys": [{ "pos": 0.5, "points": [[0.0, 1.0], [0.499, 1.0], [0.5, -1.0], [0.999, -1.0]] }] });
        let c = build(&r, &ctx(&p)).unwrap();
        let (a, _) = wtedit::harmonics_of(&c[4 * CYCLE..5 * CYCLE], 4);
        assert!(
            a[2] > 0.25 * a[0] && a[1] < 0.05 * a[0],
            "矩形(奇数だけ): {a:?}"
        );
        // fade [0, 1]: 位置 0 では効かず、位置 1 では効く(2 倍音を消す)
        let r = json!({ "source": { "kind": "shape", "name": "analog" }, "frames": 5,
                        "edits": [{ "op": "lowpass", "max": 2, "fade": [0.0, 1.0] }] });
        let c = build(&r, &ctx(&p)).unwrap();
        let plain = build(
            &json!({ "source": { "kind": "shape", "name": "analog" }, "frames": 5 }),
            &ctx(&p),
        )
        .unwrap();
        assert_eq!(&c[..CYCLE], &plain[..CYCLE], "位置 0 は元のまま");
        let (end, _) = wtedit::harmonics_of(&c[4 * CYCLE..], 4);
        assert!(end[2] < 0.02 * end[0], "位置 1 は加工の後: {end:?}");
        let bad = json!({ "source": { "kind": "shape", "name": "analog" }, "edits": [{ "op": "resize", "frames": 8, "fade": [0, 1] }] });
        assert!(build(&bad, &ctx(&p)).is_err());
    }

    /// 倍音から作る(keys が設計図)。keys が無ければサイン波。内蔵のテーブルも元にできる
    #[test]
    fn harmonics_from_keys_and_builtin_source() {
        let p = Project::new("t");
        let r = json!({ "source": { "kind": "harmonics" }, "frames": 4 });
        let c = build(&r, &ctx(&p)).unwrap();
        let (a, _) = wtedit::harmonics_of(&c[..CYCLE], 3);
        assert!(a[0] > 0.5 && a[1] < 1e-3, "{a:?}");
        let r = json!({ "source": { "kind": "builtin", "name": "analog" }, "frames": 8 });
        assert_eq!(wtedit::frame_count(&build(&r, &ctx(&p)).unwrap()), 8);
    }

    /// 絵の材料: 重ねた絵の枚数・点の数、選んだ 1 枚の加工の前と後
    #[test]
    fn view_returns_stack_and_selected_frames() {
        let p = Project::new("t");
        let r = json!({ "source": { "kind": "shape", "name": "analog" }, "frames": 16,
                        "edits": [{ "op": "lowpass", "max": 3 }] });
        let v = view(&r, &ctx(&p), 1.0, 1.0, 40, 64, 128, 16).unwrap();
        assert_eq!(v["frames"], 16);
        assert_eq!(v["stack"].as_array().unwrap().len(), 16);
        assert_eq!(v["stack"][0].as_array().unwrap().len(), 64);
        assert_eq!(v["pre"]["wave"].as_array().unwrap().len(), 128);
        let pre: Vec<f32> = serde_json::from_value(v["pre"]["amps"].clone()).unwrap();
        let post: Vec<f32> = serde_json::from_value(v["post"]["amps"].clone()).unwrap();
        // 位置 1 の analog(矩形)には 5 倍音がある。ローパス(3 番目まで)の後は無い
        assert!(pre[4] > 0.05, "{pre:?}");
        assert!(post[4] < pre[4] * 0.2, "{post:?}");
    }
}

#[cfg(test)]
mod recipe_identity_tests {
    use super::*;

    /// 同じ波形になる 2 つの手順は、別の素材になり、それぞれの手順が残る(隣の手順のファイルを取り合わない)
    #[test]
    fn same_cycles_with_different_recipes_become_different_assets() {
        let dir = std::env::temp_dir().join(format!("glaux-wt-id-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("audio")).unwrap();
        let p = Project::new("t");
        let ctx = Ctx {
            project: &p,
            project_dir: &dir,
            track: None,
        };
        let a = json!({ "source": { "kind": "shape", "name": "pwm" }, "frames": 4 });
        let b = json!({ "source": { "kind": "shape", "name": "pwm" }, "frames": 4,
                        "edits": [{ "op": "lowpass", "max": 24, "on": false, "who": "you", "id": 1 }] });
        let ca = build(&a, &ctx).unwrap();
        let cb = build(&b, &ctx).unwrap();
        assert_eq!(ca, cb, "切った加工は掛からない(同じ波形)");
        let ia = write_asset(&dir, &ca, Some(&a)).unwrap();
        let ib = write_asset(&dir, &cb, Some(&b)).unwrap();
        assert_ne!(ia.id, ib.id);
        let mut q = Project::new("t");
        q.assets.insert(ia.id.clone(), ia.asset.clone());
        q.assets.insert(ib.id.clone(), ib.asset.clone());
        assert_eq!(asset_recipe(&q, &dir, &ia.id), Some(a));
        assert_eq!(asset_recipe(&q, &dir, &ib.id), Some(b));
        // 手順の塊が入っていても、テーブルとして読める
        assert_eq!(asset_cycles(&ib.id, &q, &dir).unwrap().len(), ca.len());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
