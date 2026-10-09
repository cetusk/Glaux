//! Glaux デスクトップアプリ(Tauri)。
//!
//! - `Session` は glaux-mcp の [`SessionHandle`](glaux_mcp::actor::SessionHandle)
//!   アクターが所有し、UI(Tauri コマンド)と AI(アプリ内 HTTP MCP サーバー)が
//!   **同じキュー**を通して編集する(docs/HANDOFF.md のプロセス構成)。
//! - MCP サーバーは同一プロセス内で `http://127.0.0.1:<port>/mcp` に立つ。
//!   Claude Code からは `claude mcp add --transport http glaux http://127.0.0.1:<port>/mcp`。
//! - アクターの変更通知(broadcast)を Tauri イベント `project-changed` として
//!   フロントエンドへ転送する。UI はイベントを受けたら全体を取得し直す。
//!
//! プロジェクトパスの決定順: コマンドライン引数 → `GLAUX_PROJECT` 環境変数 →
//! `<ホーム>/Music/GlauxDemo.glaux`(自動作成)。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod chat;
mod projects;

use anyhow::{Context, Result};
use chat::ChatManager;
use glaux_core::i18n::t;
use glaux_core::{Author, Command, EntryId, Tick};
use glaux_engine::EngineHandle;
use glaux_mcp::actor::SessionHandle;
use glaux_mcp::server::GlauxServer;
use glaux_mcp::store::Store;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpService,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::{Emitter, State};

const DEFAULT_MCP_PORT: u16 = 41920;
/// 窓口のポートが使われていたら、この数だけ次のポートを試す(Glaux を複数起動したとき)
const MCP_PORT_TRIES: u16 = 20;

struct AppState {
    handle: SessionHandle,
    /// 現在のプロジェクトフォルダ(プロジェクト切り替えで変わる)
    project_dir: std::sync::Mutex<String>,
    mcp_url: String,
    /// 起動時に知らせること(開こうとした曲が別の Glaux で開かれていた、窓口のポートを変えた、など)
    /// 起動時の知らせ(日本語, 英語)。言語の設定は起動後に画面から届くので、両方持って読むときに選ぶ
    startup_notice: Option<(String, String)>,
    chat: Arc<ChatManager>,
    /// オーディオデバイスが無い環境では None(再生なしで動作を続ける)
    engine: Option<EngineHandle>,
    /// 遅延の較正中の状態(元の再生位置と、録音先頭からの各拍の時刻)
    calib: std::sync::Mutex<Option<(Tick, Vec<f64>)>>,
}

impl AppState {
    fn project_dir(&self) -> String {
        self.project_dir.lock().expect("project_dir lock").clone()
    }
}

impl AppState {
    fn engine(&self) -> Result<&EngineHandle, String> {
        self.engine.as_ref().ok_or_else(|| {
            t(
                "オーディオデバイスが利用できません",
                "Audio device is not available",
            )
            .to_owned()
        })
    }
}

// ---- Tauri コマンド(UI からの読み取り・操作) ---------------------------

/// 画面へ送るトラックから CLAP の状態(プラグインの設定の base64。大きい)を省く。画面は使わない
/// (トラックの複製はバックエンドの duplicate_track で行うので、送り返されることもない)
fn strip_plugin_state(track: &mut Value) {
    if let Some(d) = track.get_mut("device").and_then(|d| d.as_object_mut()) {
        d.remove("state");
    }
    if let Some(fx) = track.get_mut("effects").and_then(|f| f.as_array_mut()) {
        for e in fx {
            if let Some(o) = e.as_object_mut() {
                o.remove("state");
            }
        }
    }
}

/// CLAP の状態を持っているか(画面へ送る前に省く必要があるか)
fn has_plugin_state(source: &glaux_core::PluginSource) -> bool {
    matches!(
        source,
        glaux_core::PluginSource::Clap { state: Some(_), .. }
    )
}

/// [`strip_plugin_state`] と同じことを型の上で(直列化の前に)
fn strip_plugin_state_typed(
    device: Option<&mut glaux_core::Device>,
    effects: &mut [glaux_core::Effect],
) {
    let sources = device
        .map(|d| &mut d.source)
        .into_iter()
        .chain(effects.iter_mut().map(|e| &mut e.source));
    for source in sources {
        if let glaux_core::PluginSource::Clap { state, .. } = source {
            *state = None;
        }
    }
}

/// 曲の全体。`serde_json::Value` の木を作らずに直接 JSON の文字列にして返す(大きな曲で軽い)
#[tauri::command]
async fn get_project(state: State<'_, AppState>) -> Result<tauri::ipc::Response, String> {
    let (project, version) = state.handle.get_project_shared().await?;
    tokio::task::spawn_blocking(move || {
        let has_state = project.tracks.iter().any(|t| {
            t.device
                .as_ref()
                .is_some_and(|d| has_plugin_state(&d.source))
                || t.effects.iter().any(|e| has_plugin_state(&e.source))
        }) || project
            .master
            .effects
            .iter()
            .any(|e| has_plugin_state(&e.source));
        // CLAP の状態があるときだけ複製して省く(無ければそのまま直列化する)
        let stripped;
        let p: &glaux_core::Project = if has_state {
            let mut c = (*project).clone();
            for t in &mut c.tracks {
                strip_plugin_state_typed(t.device.as_mut(), &mut t.effects);
            }
            strip_plugin_state_typed(None, &mut c.master.effects);
            stripped = c;
            &stripped
        } else {
            &project
        };
        #[derive(serde::Serialize)]
        struct Out<'a> {
            project_version: usize,
            project: &'a glaux_core::Project,
        }
        serde_json::to_string(&Out {
            project_version: version,
            project: p,
        })
        .map(tauri::ipc::Response::new)
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// トラックを複製して、元のすぐ下に置く(CLAP の状態・エフェクトのつながり・変調・マクロも写す)
#[tauri::command]
async fn duplicate_track(state: State<'_, AppState>, track_id: String) -> Result<Value, String> {
    let (project, _) = state.handle.get_project_shared().await?;
    let tid = glaux_core::TrackId::parse(&track_id).map_err(|e| e.to_string())?;
    let index = project
        .tracks
        .iter()
        .position(|t| t.id == tid)
        .ok_or_else(|| t("トラックが見つかりません", "Track not found").to_owned())?;
    let src = &project.tracks[index];
    let copy = glaux_mcp::server::duplicate_track(
        src,
        glaux_core::tr!("{} のコピー", "{} copy", src.name),
    );
    let label = glaux_core::tr!("{} を複製", "Duplicate {}", src.name);
    let command = Command::AddTrack {
        track: copy,
        index: Some(index + 1),
    };
    let (entry_id, m) = state
        .handle
        .apply(command, Author::Human, label)
        .await?
        .map_err(|e| e.to_string())?;
    Ok(json!({ "entry_id": entry_id, "project_version": m.project_version }))
}

/// 指定したトラックだけを返す(変更がトラックの中だけのとき、画面が全体を取り直さずに済むように)。
/// 見つからない ID は含めない(画面はそのとき全体を取り直す)。
#[tauri::command]
async fn get_tracks(state: State<'_, AppState>, ids: Vec<String>) -> Result<Value, String> {
    let (project, version) = state.handle.get_project_shared().await?;
    let tracks: Vec<Value> = project
        .tracks
        .iter()
        .filter(|t| ids.iter().any(|i| i == t.id.as_str()))
        .filter_map(|t| serde_json::to_value(t).ok())
        .map(|mut v| {
            strip_plugin_state(&mut v);
            v
        })
        .collect();
    Ok(json!({ "project_version": version, "tracks": tracks }))
}

#[tauri::command]
async fn get_history(state: State<'_, AppState>, limit: Option<usize>) -> Result<Value, String> {
    let page = state
        .handle
        .get_history(None, None, limit)
        .await?
        .map_err(|e| e.to_string())?;
    serde_json::to_value(page).map_err(|e| e.to_string())
}

#[tauri::command]
async fn undo(state: State<'_, AppState>) -> Result<Value, String> {
    let (undone, m) = state.handle.undo(1).await?.map_err(|e| e.to_string())?;
    Ok(json!({ "undone": undone, "project_version": m.project_version }))
}

#[tauri::command]
async fn redo(state: State<'_, AppState>) -> Result<Value, String> {
    let (redone, m) = state.handle.redo(1).await?.map_err(|e| e.to_string())?;
    Ok(json!({ "redone": redone, "project_version": m.project_version }))
}

#[tauri::command]
fn app_info(state: State<'_, AppState>) -> Value {
    json!({
        "project_dir": state.project_dir(),
        "mcp_url": state.mcp_url,
        "startup_notice": state
            .startup_notice
            .as_ref()
            .map(|(ja, en)| if glaux_core::i18n::is_en() { en } else { ja }),
    })
}

// ---- プロジェクト管理 ------------------------------------------------------

#[tauri::command]
fn list_recent_projects(state: State<'_, AppState>) -> Value {
    let current = state.project_dir();
    let list: Vec<Value> = projects::load_recent()
        .into_iter()
        .map(|r| {
            let exists = std::path::Path::new(&r.path).join("project.json").exists();
            json!({
                "path": r.path,
                "title": r.title,
                "last_opened": r.last_opened,
                "exists": exists,
                "current": r.path == current,
            })
        })
        .collect();
    json!({ "recent": list, "default_dir": projects::default_projects_dir() })
}

/// 同期のコマンドは画面のスレッドで動くので、ファイルを読む重い処理は別のスレッドで行う
async fn off_thread<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())
}

/// フォルダの中の Glaux の曲を探す(フォルダ自体が曲ならそれ 1 つ)。
/// (曲の project.json を最大 50 個読む。別のスレッドで)
#[tauri::command]
async fn find_projects(dir: String) -> Result<Value, String> {
    off_thread(move || json!({ "projects": projects::find_projects(&dir) })).await
}

/// 既定の作業フォルダ(新規プロジェクトの作成先)を変更する。
#[tauri::command]
fn set_projects_dir(path: String) -> Result<Value, String> {
    glaux_mcp::store::check_project_parent(std::path::Path::new(&path))?;
    projects::set_projects_dir(&path)?;
    Ok(json!({ "default_dir": path }))
}

/// 音声ファイルの取り込み(読み込み・変換・ハッシュ・コピー)を、画面の応答を止めないよう別のスレッドで行う
async fn import_audio_off_thread(
    dir: String,
    path: String,
) -> Result<glaux_mcp::assets::ImportedSample, String> {
    tokio::task::spawn_blocking(move || {
        glaux_mcp::assets::import_audio(std::path::Path::new(&dir), std::path::Path::new(&path))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 素材の波形(最小・最大を交互に並べた 2 × buckets 個)と長さ(秒)。音色エディタの素材の絵
#[tauri::command]
async fn asset_peaks(
    state: State<'_, AppState>,
    asset_id: String,
    buckets: u32,
    from_sec: Option<f64>,
    to_sec: Option<f64>,
) -> Result<Value, String> {
    let id = glaux_core::AssetId::parse(&asset_id).map_err(|e| e.to_string())?;
    let (project, _) = state.handle.get_project_shared().await?;
    let dir = state.project_dir();
    let buckets = buckets.clamp(1, 8192) as usize;
    let (peaks, secs) = tokio::task::spawn_blocking(move || {
        glaux_mcp::assets::asset_peaks(
            &project,
            std::path::Path::new(&dir),
            &id,
            buckets,
            from_sec.zip(to_sec),
        )
    })
    .await
    .map_err(|e| e.to_string())??;
    let flat: Vec<f32> = peaks.into_iter().flat_map(|(lo, hi)| [lo, hi]).collect();
    Ok(json!({ "peaks": flat, "seconds": secs }))
}

/// SFZ の楽器の中身(鍵盤ごとの強さの段・ラウンドロビン・効く CC と、調整つまみ)。音色エディタの SFZ
#[tauri::command]
async fn sfz_inspect(
    instrument: String,
    cc: std::collections::BTreeMap<u8, u8>,
) -> Result<glaux_engine::sfz::Inspect, String> {
    tokio::task::spawn_blocking(move || {
        glaux_engine::sfz::inspect(&glaux_engine::sfz::default_dir(), &instrument, &cc)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// SFZ の鍵盤をその強さで弾いたとき(rr 回目。0 始まり)に鳴る録音の波形。音色エディタの SFZ
#[tauri::command]
async fn sfz_key_wave(
    instrument: String,
    cc: std::collections::BTreeMap<u8, u8>,
    key: u8,
    vel: u8,
    rr: u8,
    buckets: usize,
) -> Result<glaux_engine::sfz::KeyWave, String> {
    tokio::task::spawn_blocking(move || {
        glaux_engine::sfz::key_wave(
            &glaux_engine::sfz::default_dir(),
            &instrument,
            &cc,
            key,
            vel,
            rr,
            buckets,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 音の頭で自動に引く区分の線(長さの割合。使う所の中だけ、区分 1 の頭を除く)。音色エディタのサンプラー
#[tauri::command]
async fn sampler_auto_cuts(
    state: State<'_, AppState>,
    asset_id: String,
    start: f64,
    end: f64,
) -> Result<Vec<f64>, String> {
    let id = glaux_core::AssetId::parse(&asset_id).map_err(|e| e.to_string())?;
    let (project, _) = state.handle.get_project_shared().await?;
    let dir = state.project_dir();
    tokio::task::spawn_blocking(move || {
        glaux_mcp::assets::auto_slice_points(
            &project,
            std::path::Path::new(&dir),
            &id,
            start,
            end,
            64,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// WAV をプロジェクトに取り込み、トラックの音源を sampler にする(音作りビュー用)。
/// `instrument` が "granular" なら、音源はそのままで粒を取り出す素材(sample)にする
#[tauri::command]
async fn import_sample(
    state: State<'_, AppState>,
    track_id: String,
    path: String,
    instrument: Option<String>,
) -> Result<Value, String> {
    let tid = glaux_core::TrackId::parse(&track_id).map_err(|e| e.to_string())?;
    let (project, _) = state.handle.get_project_shared().await?;
    let track = project.track(&tid).ok_or_else(|| {
        glaux_core::tr!(
            "トラックが見つかりません: {track_id}",
            "Track not found: {track_id}"
        )
    })?;
    let dir = state.project_dir();
    let imported = import_audio_off_thread(dir.clone(), path.clone()).await?;

    let mut cmds = Vec::new();
    if !project.assets.contains_key(&imported.id) {
        cmds.push(Command::AddAsset {
            id: imported.id.clone(),
            asset: imported.asset.clone(),
        });
    }
    if instrument.as_deref() == Some("granular") {
        cmds.push(Command::SetParam {
            track: tid.clone(),
            path: glaux_core::ParamPath::parse("device/sample").map_err(|e| e.to_string())?,
            value: glaux_core::ParamValue::Enum(imported.id.to_string()),
        });
        let file_name = std::path::Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "sample".to_owned());
        let label = glaux_core::tr!(
            "{} の素材を「{file_name}」に",
            "Set the source of {} to \"{file_name}\"",
            track.name
        );
        let (_, m) = state
            .handle
            .apply(Command::batch(label.clone(), cmds), Author::Human, label)
            .await?
            .map_err(|e| e.to_string())?;
        return Ok(json!({ "asset_id": imported.id, "project_version": m.project_version }));
    }
    // ステレオの素材は左右のまま鳴らす(つまみの既定はモノラルに合算 = 以前に取り込んだ音源の音を変えない)
    let mut params = glaux_core::ParamMap::new();
    if imported.asset.channels >= 2 {
        params.insert("stereo".to_owned(), glaux_core::ParamValue::Bool(true));
    }
    cmds.push(Command::SetDevice {
        track: tid,
        device: Some(glaux_core::Device {
            source: glaux_core::PluginSource::Sampler {
                asset: imported.id.clone(),
            },
            params,
        }),
    });
    let file_name = std::path::Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "sample".to_owned());
    let label = glaux_core::tr!(
        "{} にサンプル「{file_name}」を設定",
        "Set sample \"{file_name}\" on {}",
        track.name
    );
    let (_, m) = state
        .handle
        .apply(Command::batch(label.clone(), cmds), Author::Human, label)
        .await?
        .map_err(|e| e.to_string())?;
    Ok(json!({ "asset_id": imported.id, "project_version": m.project_version }))
}

/// 畳み込みリバーブの響き(IR)を音声ファイルから取り込み、そのエフェクトの ir に設定する(履歴 1 件)。
/// `track_id` が無ければマスターのエフェクト
#[tauri::command]
async fn import_ir(
    state: State<'_, AppState>,
    track_id: Option<String>,
    fx_id: String,
    path: String,
) -> Result<Value, String> {
    let fx = glaux_core::FxId::parse(&fx_id).map_err(|e| e.to_string())?;
    let (project, _) = state.handle.get_project_shared().await?;
    let dir = state.project_dir();
    let imported = import_audio_off_thread(dir.clone(), path.clone()).await?;
    let mut cmds = Vec::new();
    if !project.assets.contains_key(&imported.id) {
        cmds.push(Command::AddAsset {
            id: imported.id.clone(),
            asset: imported.asset.clone(),
        });
    }
    let param = glaux_core::ParamPath::effect(fx, "ir");
    let value = glaux_core::ParamValue::Enum(imported.id.to_string());
    cmds.push(match &track_id {
        Some(t) => Command::SetParam {
            track: glaux_core::TrackId::parse(t).map_err(|e| e.to_string())?,
            path: param,
            value,
        },
        None => Command::SetMasterParam { path: param, value },
    });
    let file_name = std::path::Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "IR".to_owned());
    let label = glaux_core::tr!(
        "畳み込みリバーブの響きを「{file_name}」に",
        "Set convolution reverb IR to \"{file_name}\""
    );
    let (_, m) = state
        .handle
        .apply(Command::batch(label.clone(), cmds), Author::Human, label)
        .await?
        .map_err(|e| e.to_string())?;
    Ok(json!({ "asset_id": imported.id, "project_version": m.project_version }))
}

/// WAV を音声クリップとして音声トラックに置く(履歴 1 件)。
#[tauri::command]
async fn import_audio_clip(
    state: State<'_, AppState>,
    track_id: String,
    path: String,
    start_tick: Option<u64>,
) -> Result<Value, String> {
    let tid = glaux_core::TrackId::parse(&track_id).map_err(|e| e.to_string())?;
    let (project, _) = state.handle.get_project_shared().await?;
    let dir = state.project_dir();
    let imported = import_audio_off_thread(dir.clone(), path.clone()).await?;
    let name = std::path::Path::new(&path)
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "audio".to_owned());
    let clip_id = glaux_core::ClipId::new();
    let cmds = glaux_mcp::assets::audio_clip_commands(
        &project,
        &tid,
        &imported,
        clip_id.clone(),
        Tick(start_tick.unwrap_or(0)),
        &name,
    )?;
    let track_name = project
        .track(&tid)
        .map(|t| t.name.clone())
        .unwrap_or_default();
    let label = glaux_core::tr!(
        "{track_name} に音声クリップ「{name}」を配置",
        "Place audio clip \"{name}\" on {track_name}"
    );
    let (_, m) = state
        .handle
        .apply(Command::batch(label.clone(), cmds), Author::Human, label)
        .await?
        .map_err(|e| e.to_string())?;
    Ok(json!({ "clip_id": clip_id, "asset_id": imported.id, "project_version": m.project_version }))
}

/// 音声クリップの波形ピーク(表示用)。
#[tauri::command]
async fn clip_peaks(
    state: State<'_, AppState>,
    clip_id: String,
    buckets: u32,
) -> Result<Value, String> {
    let cid = glaux_core::ClipId::parse(&clip_id).map_err(|e| e.to_string())?;
    let (project, _) = state.handle.get_project_shared().await?;
    let dir = state.project_dir();
    let buckets = buckets.clamp(1, 4096) as usize;
    let peaks = tokio::task::spawn_blocking(move || {
        glaux_mcp::transcribe::clip_peaks(&project, std::path::Path::new(&dir), &cid, buckets)
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(json!({ "peaks": peaks }))
}

/// 音声クリップの音に似せた内蔵シンセ(subtractive)のトラックを作る(つまみは自動で探す。履歴 1 件)。
#[tauri::command]
async fn match_clip_sound(state: State<'_, AppState>, clip_id: String) -> Result<Value, String> {
    let cid = glaux_core::ClipId::parse(&clip_id).map_err(|e| e.to_string())?;
    let (project, _) = state.handle.get_project_shared().await?;
    let dir = state.project_dir();
    let m = tokio::task::spawn_blocking(move || {
        glaux_mcp::sound::match_clip_commands(&project, std::path::Path::new(&dir), &cid, 30.0)
    })
    .await
    .map_err(|e| e.to_string())??;
    let label = glaux_core::tr!(
        "「{}」を作る(音色を自動で合わせる)",
        "Create \"{}\" (auto-matched sound)",
        m.track_name
    );
    let (_, applied) = state
        .handle
        .apply(
            Command::batch(label.clone(), m.commands),
            Author::Human,
            label,
        )
        .await?
        .map_err(|e| e.to_string())?;
    let mut v = glaux_mcp::sound::match_json(&m.outcome);
    v["track_id"] = json!(m.track_id.to_string());
    v["track_name"] = json!(m.track_name);
    v["project_version"] = json!(applied.project_version);
    Ok(v)
}

/// 音声クリップの音に近い CLAP 音源のプリセットを探す(track_id の CLAP 音源のプリセットから)。
/// 索引(プリセットを 1 音ずつ鳴らした記録)は index_seconds(既定 90 秒)まで作り足し、
/// 途中経過は `preset-index` イベント({total, indexed, added})で届く。
#[tauri::command]
async fn find_similar_clap_presets(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    clip_id: String,
    track_id: String,
    category: Option<String>,
    index_seconds: Option<u64>,
) -> Result<Value, String> {
    let cid = glaux_core::ClipId::parse(&clip_id).map_err(|e| e.to_string())?;
    let tid = glaux_core::TrackId::parse(&track_id).map_err(|e| e.to_string())?;
    let (project, _) = state.handle.get_project_shared().await?;
    let dir = state.project_dir();
    tokio::task::spawn_blocking(move || {
        glaux_mcp::preset_index::similar_json(
            &project,
            std::path::Path::new(&dir),
            &glaux_mcp::sound::SoundSource::Clip(cid),
            &tid,
            category.as_deref(),
            8,
            std::time::Duration::from_secs(index_seconds.unwrap_or(90).min(600)),
            &mut |p| {
                let _ = app.emit("preset-index", &p);
            },
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// CLAP 音源のつまみを音声クリップの音に自動で合わせる(今の音色から出発。履歴 1 件)。
#[tauri::command]
async fn refine_clap_params(
    state: State<'_, AppState>,
    clip_id: String,
    track_id: String,
    max_seconds: Option<f32>,
) -> Result<Value, String> {
    let cid = glaux_core::ClipId::parse(&clip_id).map_err(|e| e.to_string())?;
    let tid = glaux_core::TrackId::parse(&track_id).map_err(|e| e.to_string())?;
    let (project, _) = state.handle.get_project_shared().await?;
    let dir = state.project_dir();
    let refined = tokio::task::spawn_blocking(move || {
        let target = glaux_mcp::sound::load(
            &project,
            std::path::Path::new(&dir),
            &glaux_mcp::sound::SoundSource::Clip(cid),
        )?;
        glaux_mcp::preset_index::refine_params(
            &project,
            &tid,
            &target,
            &[],
            max_seconds.unwrap_or(20.0).clamp(5.0, 120.0),
        )
    })
    .await
    .map_err(|e| e.to_string())??;
    let mut v = refined.json;
    if refined.commands.is_empty() {
        return Ok(v);
    }
    let label = glaux_core::tr!(
        "「{}」の CLAP のつまみを音声クリップの音に合わせる({} 個)",
        "Match CLAP parameters of \"{}\" to the audio clip ({})",
        v["track"].as_str().unwrap_or(""),
        refined.commands.len()
    );
    let (_, applied) = state
        .handle
        .apply(
            Command::batch(label.clone(), refined.commands),
            Author::Human,
            label,
        )
        .await?
        .map_err(|e| e.to_string())?;
    v["project_version"] = json!(applied.project_version);
    Ok(v)
}

/// 追加モデル(CLAP の音声側)の状態。
#[tauri::command]
fn model_status() -> Value {
    json!({ "clap": glaux_mcp::models::clap_status() })
}

/// CLAP の音声側モデル(約 280MB)を取得する。進捗は `model-download` イベント({got, total})で届く。
#[tauri::command]
async fn download_clap_model(app: tauri::AppHandle) -> Result<Value, String> {
    let path = tokio::task::spawn_blocking(move || {
        glaux_mcp::models::download_clap(&mut |got, total| {
            let _ = app.emit("model-download", json!({ "got": got, "total": total }));
        })
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(json!({ "path": path.to_string_lossy() }))
}

/// 初回の案内の確認: 音の出力・AI の CLI(Claude Code / Codex CLI)・SoundFont のライブラリ。
#[tauri::command]
fn setup_status(state: State<'_, AppState>) -> Value {
    json!({
        "audio_output": state.engine.as_ref().map(|e| e.output_device()),
        "claude": chat::find_cli(chat::Provider::Claude),
        "codex": chat::find_cli(chat::Provider::Codex),
        "soundfont": glaux_mcp::models::soundfont_status(),
    })
}

/// GM 音源一式の SoundFont(GeneralUser GS、約 32MB)をライブラリへ取得する。
/// 進捗は `soundfont-download` イベント({got, total})で届く。
#[tauri::command]
async fn download_soundfont(app: tauri::AppHandle) -> Result<Value, String> {
    let path = tokio::task::spawn_blocking(move || {
        glaux_mcp::models::download_soundfont(&mut |got, total| {
            let _ = app.emit("soundfont-download", json!({ "got": got, "total": total }));
        })
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(json!({ "path": path.to_string_lossy() }))
}

/// 同梱のデモ曲(CyberNeon。内蔵の音源とエフェクトだけで鳴る)を、曲のフォルダに写して開く。
/// 開くたびに新しい写しを作る(前に開いたデモを直していても上書きしない)。
#[tauri::command]
async fn open_demo_song(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    const DEMO: &str = include_str!("../demo/CyberNeon.json");
    let project: glaux_core::Project = serde_json::from_str(DEMO)
        .map_err(|e| glaux_core::tr!("デモ曲を読めません: {e}", "Can't read the demo song: {e}"))?;
    let parent = projects::default_projects_dir();
    let dir = glaux_mcp::store::unique_project_dir(
        std::path::Path::new(&parent),
        &glaux_mcp::store::folder_name(&project.meta.title),
    );
    glaux_mcp::store::create_project_from(&dir, project).map_err(|e| e.to_string())?;
    open_project(app, state, dir.to_string_lossy().into_owned(), false).await
}

/// 音声クリップの元のテンポ・拍子を検出する(テンポ追従の original_bpm 用。速さのため先頭 60 秒)。
#[tauri::command]
async fn detect_clip_tempo(state: State<'_, AppState>, clip_id: String) -> Result<Value, String> {
    let cid = glaux_core::ClipId::parse(&clip_id).map_err(|e| e.to_string())?;
    let (project, _) = state.handle.get_project_shared().await?;
    let dir = state.project_dir();
    let r = tokio::task::spawn_blocking(move || {
        let mut sound = glaux_mcp::sound::load_clip(&project, std::path::Path::new(&dir), &cid)?;
        let max = (60.0 * sound.sample_rate) as usize;
        sound.frames.truncate(max);
        glaux_mcp::sound::beats(&sound)
    })
    .await
    .map_err(|e| e.to_string())??;
    serde_json::to_value(r).map_err(|e| e.to_string())
}

/// MIDI クリップのノートにスウィングを掛ける(`note_ids` 省略で全ノート。履歴 1 件)。
/// `swing` は 0.5(ストレート)〜0.8、`grid` は裏拍の単位(480 = 8 分、240 = 16 分)。
#[tauri::command]
async fn swing_clip(
    state: State<'_, AppState>,
    clip_id: String,
    note_ids: Option<Vec<String>>,
    grid: u64,
    swing: f64,
) -> Result<Value, String> {
    let cid = glaux_core::ClipId::parse(&clip_id).map_err(|e| e.to_string())?;
    let (project, _) = state.handle.get_project_shared().await?;
    let (_, clip) = project.clip(&cid).ok_or_else(|| {
        glaux_core::tr!(
            "クリップが見つかりません: {clip_id}",
            "Clip not found: {clip_id}"
        )
    })?;
    let notes = clip
        .notes()
        .ok_or_else(|| t("MIDI クリップではありません", "Not a MIDI clip").to_owned())?;
    let chosen: Vec<glaux_core::Note> = match &note_ids {
        Some(ids) if !ids.is_empty() => notes
            .iter()
            .filter(|n| ids.iter().any(|i| i == n.id.as_str()))
            .cloned()
            .collect(),
        _ => notes.to_vec(),
    };
    let changes: Vec<glaux_core::NoteChange> = glaux_core::rhythm::swing_positions(
        &chosen,
        clip.start.0,
        clip.length.0,
        grid.max(60),
        swing,
        1.0,
        &glaux_core::meter::bar_meters(&project, clip.start.0 + clip.length.0),
    )
    .into_iter()
    .map(|(id, pos)| glaux_core::NoteChange::new(id).pos(Tick(pos)))
    .collect();
    let n = changes.len();
    if n == 0 {
        return Ok(json!({ "changed": 0 }));
    }
    let label = glaux_core::tr!(
        "スウィング {:.0}%(1/{}、{n} ノート)",
        "Swing {:.0}% (1/{}, {n} notes)",
        swing * 100.0,
        3840 / grid.max(60)
    );
    let (_, m) = state
        .handle
        .apply(
            Command::UpdateNotes { clip: cid, changes },
            Author::Human,
            label,
        )
        .await?
        .map_err(|e| e.to_string())?;
    Ok(json!({ "changed": n, "project_version": m.project_version }))
}

/// 音声クリップ(単旋律)を譜起こしして MIDI クリップを作る(履歴 1 件)。
#[tauri::command]
async fn transcribe_clip(
    state: State<'_, AppState>,
    clip_id: String,
    dest_track_id: Option<String>,
    quantize_ticks: Option<u64>,
    mode: Option<String>,
) -> Result<Value, String> {
    let cid = glaux_core::ClipId::parse(&clip_id).map_err(|e| e.to_string())?;
    let mode = glaux_mcp::transcribe::TranscribeMode::parse(mode.as_deref())?;
    let dest = match dest_track_id {
        Some(id) => Some(glaux_core::TrackId::parse(&id).map_err(|e| e.to_string())?),
        None => None,
    };
    let (project, _) = state.handle.get_project_shared().await?;
    let dir = state.project_dir();
    let q = quantize_ticks.unwrap_or(240);
    let t = tokio::task::spawn_blocking(move || {
        glaux_mcp::transcribe::transcribe_clip_commands(
            &project,
            std::path::Path::new(&dir),
            &cid,
            dest.as_ref(),
            q,
            &glaux_engine::transcribe::TranscribeOptions::default(),
            mode,
        )
    })
    .await
    .map_err(|e| e.to_string())??;
    let label = glaux_core::tr!(
        "音声クリップを譜起こし({} ノート)",
        "Transcribe audio clip ({} notes)",
        t.note_count
    );
    let (_, m) = state
        .handle
        .apply(
            Command::batch(label.clone(), t.commands),
            Author::Human,
            label,
        )
        .await?
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "clip_id": t.clip_id,
        "track_id": t.track_id,
        "note_count": t.note_count,
        "created_track": t.created_track,
        "project_version": m.project_version,
    }))
}

// ---- 録音 ------------------------------------------------------------------

/// 録音を開始する(既定の入力デバイス)。再生も同時に始める(伴奏を聴きながら録る)。
/// 一時ファイルは `<プロジェクト>/audio/rec_<時刻>.wav`。停止時に内容ハッシュ名で登録し直す。
/// `count_in_bars` 小節ぶんメトロノームでカウントインしてからクリップ位置になる。
/// `latency_ms` は出力レイテンシ補正(聴いて歌う分の遅れ。設定値)。
#[tauri::command]
async fn record_start(
    state: State<'_, AppState>,
    count_in_bars: Option<u32>,
    latency_ms: Option<f64>,
    metronome: Option<bool>,
    stereo: Option<bool>,
) -> Result<Value, String> {
    let engine = state.engine()?.clone();
    let dir = state.project_dir();
    let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let path = std::path::PathBuf::from(&dir).join(format!("audio/rec_{stamp}.wav"));
    // カウントインの長さ: 現在位置の拍子で bars 小節
    let (project, _) = state.handle.get_project_shared().await?;
    let count_in =
        Tick(bar_ticks_at(&project, engine.playhead_tick()) * count_in_bars.unwrap_or(1) as u64);
    let clip_start = engine
        .start_recording(
            path,
            count_in,
            latency_ms.unwrap_or(0.0).clamp(0.0, 1000.0) / 1000.0,
            metronome.unwrap_or(true),
            stereo.unwrap_or(false),
        )
        .map_err(|e| e.to_string())?;
    engine.play();
    engine.mark_play_started();
    Ok(json!({ "clip_start": clip_start, "count_in_ticks": count_in }))
}

/// 音声クリップをパートに分離し、パートごとの音声トラックに置く(履歴 1 件)。
/// method: "builtin"(打楽器 / 音程楽器)/ "demucs"(ボーカル / ドラム / ベース / その他、要インストール)。
#[tauri::command]
async fn separate_clip(
    state: State<'_, AppState>,
    clip_id: String,
    method: Option<String>,
) -> Result<Value, String> {
    let cid = glaux_core::ClipId::parse(&clip_id).map_err(|e| e.to_string())?;
    let method = glaux_mcp::stems::SeparateMethod::parse(method.as_deref())?;
    let (project, _) = state.handle.get_project_shared().await?;
    let dir = state.project_dir();
    let s = tokio::task::spawn_blocking(move || {
        glaux_mcp::stems::separate_clip_commands(&project, std::path::Path::new(&dir), &cid, method)
    })
    .await
    .map_err(|e| e.to_string())??;
    let names: Vec<String> = s.tracks.iter().map(|(n, _)| n.clone()).collect();
    let label = glaux_core::tr!(
        "音声クリップをパートに分離({})",
        "Separate audio clip into parts ({})",
        names.join(" / ")
    );
    let (_, m) = state
        .handle
        .apply(
            Command::batch(label.clone(), s.commands),
            Author::Human,
            label,
        )
        .await?
        .map_err(|e| e.to_string())?;
    Ok(json!({ "parts": names, "project_version": m.project_version }))
}

// ---- CLAP プラグイン ----

/// インストール済みの CLAP プラグイン一覧(rescan で探し直す)と、探している場所。
#[tauri::command]
async fn clap_plugins(rescan: Option<bool>) -> Result<Value, String> {
    let list = tokio::task::spawn_blocking(move || {
        if rescan.unwrap_or(false) {
            glaux_engine::plugins::rescan()
        } else {
            glaux_engine::plugins::catalog()
        }
    })
    .await
    .map_err(|e| e.to_string())?;
    let plugins: Vec<Value> = list
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "vendor": p.vendor,
                "version": p.version,
                "path": p.path.to_string_lossy(),
                "instrument": p.is_instrument(),
                "effect": p.is_effect(),
            })
        })
        .collect();
    let dirs: Vec<String> = glaux_engine::plugins::search_paths()
        .iter()
        .map(|d| d.to_string_lossy().into_owned())
        .collect();
    Ok(json!({ "plugins": plugins, "dirs": dirs }))
}

/// CLAP プラグインの持ち主(トラックの音源なら track_id、エフェクトなら fx_id)。
fn plugin_owner(
    track_id: Option<String>,
    fx_id: Option<String>,
) -> Result<glaux_engine::plugins::PluginOwner, String> {
    use glaux_engine::plugins::PluginOwner;
    match (fx_id, track_id) {
        (Some(f), _) => Ok(PluginOwner::Effect(
            glaux_core::FxId::parse(&f).map_err(|e| e.to_string())?,
        )),
        (None, Some(t)) => Ok(PluginOwner::Track(
            glaux_core::TrackId::parse(&t).map_err(|e| e.to_string())?,
        )),
        (None, None) => Err(t(
            "track_id か fx_id を指定してください",
            "Specify track_id or fx_id",
        )
        .to_owned()),
    }
}

/// CLAP プラグイン(音源・エフェクト)の今の状態をプロジェクトに保存する(変わっていなければ何もしない)。
#[tauri::command]
async fn clap_save_state(
    state: State<'_, AppState>,
    track_id: Option<String>,
    fx_id: Option<String>,
) -> Result<Value, String> {
    save_clap_state(&state, plugin_owner(track_id, fx_id)?).await
}

/// CLAP プラグイン(音源は track_id、エフェクトは fx_id)のプリセット一覧(UI 用に全件)。
#[tauri::command]
async fn clap_presets(
    state: State<'_, AppState>,
    track_id: Option<String>,
    fx_id: Option<String>,
    rescan: Option<bool>,
) -> Result<Value, String> {
    let owner = plugin_owner(track_id, fx_id)?;
    let (project, _) = state.handle.get_project_shared().await?;
    tokio::task::spawn_blocking(move || {
        glaux_mcp::clap_presets::list(
            &project,
            &owner,
            None,
            None,
            usize::MAX,
            rescan.unwrap_or(false),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// CLAP プラグイン(音源は track_id、エフェクトは fx_id)にプリセットを読み込む(履歴 1 件)。
#[tauri::command]
async fn clap_load_preset(
    state: State<'_, AppState>,
    track_id: Option<String>,
    fx_id: Option<String>,
    preset: String,
) -> Result<Value, String> {
    let owner = plugin_owner(track_id, fx_id)?;
    let (project, _) = state.handle.get_project_shared().await?;
    let (command, label, name) = tokio::task::spawn_blocking(move || {
        glaux_mcp::clap_presets::load_command(&project, &owner, &preset)
    })
    .await
    .map_err(|e| e.to_string())??;
    let (_, m) = state
        .handle
        .apply(command, Author::Human, label)
        .await?
        .map_err(|e| e.to_string())?;
    Ok(json!({ "preset": name, "project_version": m.project_version }))
}

/// プラグインの画面を開く(開いていれば前面へ)。音源なら track_id、エフェクトなら fx_id。
#[tauri::command]
async fn clap_open_gui(
    state: State<'_, AppState>,
    track_id: Option<String>,
    fx_id: Option<String>,
) -> Result<(), String> {
    use glaux_engine::plugins::PluginOwner;
    let owner = plugin_owner(track_id, fx_id)?;
    let engine = state.engine()?.clone();
    let (project, _) = state.handle.get_project_shared().await?;
    let (source, place) = match &owner {
        PluginOwner::Track(tid) => {
            let track = project.track(tid).ok_or_else(|| {
                glaux_core::tr!("トラックが見つかりません: {tid}", "Track not found: {tid}")
            })?;
            (
                track.device.as_ref().map(|d| d.source.clone()),
                track.name.clone(),
            )
        }
        PluginOwner::Effect(fx) => {
            let (effect, place) = find_effect(&project, fx)?;
            (Some(effect.source.clone()), place)
        }
    };
    let plugin_name = match source {
        Some(glaux_core::PluginSource::Clap { plugin_id, .. }) => {
            glaux_engine::plugins::find(&plugin_id)
                .map(|p| p.name)
                .unwrap_or(plugin_id)
        }
        _ => return Err(t("CLAP プラグインではありません", "Not a CLAP plugin").to_owned()),
    };
    let title = format!("{plugin_name} — {place}");
    tokio::task::spawn_blocking(move || engine.open_plugin_gui(&owner, &title))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn clap_close_gui(
    state: State<'_, AppState>,
    track_id: Option<String>,
    fx_id: Option<String>,
) -> Result<(), String> {
    state
        .engine()?
        .close_plugin_gui(&plugin_owner(track_id, fx_id)?);
    Ok(())
}

/// エフェクトと、その場所の名前(トラック名 / マスター)。
fn find_effect<'a>(
    project: &'a glaux_core::Project,
    fx: &glaux_core::FxId,
) -> Result<(&'a glaux_core::Effect, String), String> {
    if let Some(e) = project.master.effects.iter().find(|e| &e.id == fx) {
        return Ok((e, t("マスター", "Master").to_owned()));
    }
    project
        .tracks
        .iter()
        .find_map(|t| {
            t.effects
                .iter()
                .find(|e| &e.id == fx)
                .map(|e| (e, t.name.clone()))
        })
        .ok_or_else(|| {
            glaux_core::tr!("エフェクトが見つかりません: {fx}", "Effect not found: {fx}")
        })
}

/// プラグインの今の状態をプロジェクトに書く(履歴 1 件。変化が無ければ何もしない)。
/// 音源は set_device、エフェクトは set_effect_state + 上書きしているつまみの値。
async fn save_clap_state(
    state: &AppState,
    owner: glaux_engine::plugins::PluginOwner,
) -> Result<Value, String> {
    use glaux_engine::plugins::PluginOwner;
    let engine = state.engine()?.clone();
    let (saved, values) = {
        let e = engine.clone();
        let o = owner.clone();
        tokio::task::spawn_blocking(move || e.save_plugin_state(&o))
            .await
            .map_err(|e| e.to_string())??
    };
    let (project, _) = state.handle.get_project_shared().await?;
    // 上書きしているパラメータは今の値に揃える(画面で動かした値を上書きで戻さないように)
    let changed_params = |params: &glaux_core::ParamMap| -> Vec<(String, glaux_core::ParamValue)> {
        values
            .iter()
            .map(|(id, v)| {
                (
                    glaux_engine::plugins::param_key(*id),
                    glaux_core::ParamValue::Float(*v),
                )
            })
            .filter(|(k, v)| params.get(k) != Some(v))
            .collect()
    };
    let (command, label) = match &owner {
        PluginOwner::Track(tid) => {
            let track = project.track(tid).ok_or_else(|| {
                glaux_core::tr!("トラックが見つかりません: {tid}", "Track not found: {tid}")
            })?;
            let Some(mut device) = track.device.clone() else {
                return Err(t("音源がありません", "No instrument").to_owned());
            };
            let glaux_core::PluginSource::Clap { state: cur, .. } = &mut device.source else {
                return Err(t(
                    "CLAP プラグインの音源ではありません",
                    "Not a CLAP instrument",
                )
                .to_owned());
            };
            let state_changed = cur.as_deref() != Some(saved.as_str());
            *cur = Some(saved.clone());
            let params = changed_params(&device.params);
            if !state_changed && params.is_empty() {
                return Ok(json!({ "changed": false }));
            }
            device.params.extend(params);
            (
                Command::SetDevice {
                    track: tid.clone(),
                    device: Some(device),
                },
                glaux_core::tr!(
                    "{} のプラグインの設定を保存",
                    "Save plugin settings of {}",
                    track.name
                ),
            )
        }
        PluginOwner::Effect(fx) => {
            let (effect, place) = find_effect(&project, fx)?;
            let glaux_core::PluginSource::Clap { state: cur, .. } = &effect.source else {
                return Err(t(
                    "CLAP プラグインのエフェクトではありません",
                    "Not a CLAP effect",
                )
                .to_owned());
            };
            let state_changed = cur.as_deref() != Some(saved.as_str());
            let params = changed_params(&effect.params);
            if !state_changed && params.is_empty() {
                return Ok(json!({ "changed": false }));
            }
            let on_master = project.master.effects.iter().any(|e| &e.id == fx);
            let track_of = project
                .tracks
                .iter()
                .find(|t| t.effects.iter().any(|e| &e.id == fx))
                .map(|t| t.id.clone());
            let mut cmds = vec![Command::SetEffectState {
                id: fx.clone(),
                state: Some(saved.clone()),
            }];
            for (k, v) in params {
                let path = glaux_core::ParamPath::effect(fx.clone(), k);
                cmds.push(match (&track_of, on_master) {
                    (Some(t), false) => Command::SetParam {
                        track: t.clone(),
                        path,
                        value: v,
                    },
                    _ => Command::SetMasterParam { path, value: v },
                });
            }
            let label = glaux_core::tr!(
                "{place} のエフェクトの設定を保存",
                "Save effect settings on {place}"
            );
            (Command::batch(label.clone(), cmds), label)
        }
    };
    engine.note_plugin_state_saved(&owner, &saved, &values);
    let (_, m) = state
        .handle
        .apply(command, Author::Human, label)
        .await?
        .map_err(|e| e.to_string())?;
    Ok(json!({ "changed": true, "project_version": m.project_version }))
}

/// `at` の位置の拍子での 1 小節の長さ(tick)。
fn bar_ticks_at(project: &glaux_core::Project, at: Tick) -> u64 {
    project
        .time_sig_map
        .iter()
        .rev()
        .find(|e| e.tick <= at)
        .or_else(|| project.time_sig_map.first())
        .map(|e| 3840 * e.num as u64 / e.den.max(1) as u64)
        .unwrap_or(3840)
}

// ---- MIDI キーボード ----

/// MIDI 入力ポートの一覧と接続中のポート。
#[tauri::command]
fn midi_inputs(state: State<'_, AppState>) -> Value {
    json!({
        "inputs": glaux_engine::list_midi_inputs(),
        "current": state.engine.as_ref().and_then(|e| e.midi_input()),
    })
}

/// MIDI 入力に接続する(name 省略・空 = 切断)。
#[tauri::command]
fn set_midi_input(state: State<'_, AppState>, name: Option<String>) -> Result<Value, String> {
    let engine = state.engine()?;
    engine
        .set_midi_input(name.filter(|n| !n.is_empty()))
        .map_err(|e| e.to_string())?;
    Ok(json!({ "current": engine.midi_input() }))
}

/// MIDI キーボードで鳴らすトラック(省略 = 既定音色)。
#[tauri::command]
fn set_live_target(state: State<'_, AppState>, track_id: Option<String>) -> Result<(), String> {
    let id = track_id
        .filter(|s| !s.is_empty())
        .map(|s| glaux_core::TrackId::parse(&s).map_err(|e| e.to_string()))
        .transpose()?;
    state.engine()?.set_live_target(id);
    Ok(())
}

/// MIDI 録音を開始する(count_in_bars 小節のカウントイン後の位置にクリップを置く)。
#[tauri::command]
async fn midi_record_start(
    state: State<'_, AppState>,
    count_in_bars: Option<u32>,
    metronome: Option<bool>,
) -> Result<Value, String> {
    let engine = state.engine()?.clone();
    let (project, _) = state.handle.get_project_shared().await?;
    let count_in =
        Tick(bar_ticks_at(&project, engine.playhead_tick()) * count_in_bars.unwrap_or(1) as u64);
    let clip_start = engine
        .start_midi_recording(count_in, metronome.unwrap_or(true))
        .map_err(|e| e.to_string())?;
    engine.play();
    Ok(json!({ "clip_start": clip_start, "count_in_ticks": count_in }))
}

/// MIDI 録音を止めて、弾いたノートを MIDI クリップとして置く(履歴 1 件)。
/// `track_id` 省略時はライブ演奏の送り先 → 最初の MIDI トラック → 新設の順。
/// `quantize_ticks` > 0 なら開始位置をそのグリッドに丸める。
#[tauri::command]
async fn midi_record_stop(
    state: State<'_, AppState>,
    track_id: Option<String>,
    quantize_ticks: Option<u64>,
) -> Result<Value, String> {
    let engine = state.engine()?.clone();
    engine.pause();
    let outcome = engine.stop_midi_recording().map_err(|e| e.to_string())?;
    let notes = glaux_engine::midi::take_to_notes(
        &outcome.notes,
        outcome.clip_start,
        quantize_ticks.unwrap_or(0),
    );
    if notes.is_empty() {
        return Err(t(
            "カウントインより後に弾かれたノートがありませんでした",
            "No notes were played after the count-in",
        )
        .to_owned());
    }
    let (project, _) = state.handle.get_project_shared().await?;
    let is_midi = |id: &glaux_core::TrackId| {
        project
            .tracks
            .iter()
            .any(|t| &t.id == id && t.kind == glaux_core::TrackKind::Midi)
    };
    let mut cmds = Vec::new();
    let requested = track_id
        .filter(|s| !s.is_empty())
        .map(|s| glaux_core::TrackId::parse(&s).map_err(|e| e.to_string()))
        .transpose()?;
    let tid = match requested
        .or_else(|| engine.live_target())
        .filter(|id| is_midi(id))
        .or_else(|| {
            project
                .tracks
                .iter()
                .find(|t| t.kind == glaux_core::TrackKind::Midi)
                .map(|t| t.id.clone())
        }) {
        Some(id) => id,
        None => {
            let id = glaux_core::TrackId::new();
            cmds.push(Command::AddTrack {
                track: glaux_core::Track::new(
                    id.clone(),
                    t("MIDI 録音", "MIDI Recording"),
                    glaux_core::TrackKind::Midi,
                ),
                index: None,
            });
            id
        }
    };
    // クリップ長: 最後のノートの終わり(と停止位置)を小節単位に切り上げ
    let bar = bar_ticks_at(&project, outcome.clip_start).max(1);
    let last_end = notes.iter().map(|n| n.end().0).max().unwrap_or(0);
    let played = outcome.stop_tick.0.saturating_sub(outcome.clip_start.0);
    let length = Tick(last_end.max(played).div_ceil(bar).max(1) * bar);
    let clip_id = glaux_core::ClipId::new();
    let name = glaux_core::tr!(
        "MIDI 録音 {}",
        "MIDI Recording {}",
        chrono::Local::now().format("%H:%M")
    );
    let count = notes.len();
    let mut clip =
        glaux_core::Clip::new_midi(clip_id.clone(), name.clone(), outcome.clip_start, length);
    if let Some(ns) = clip.notes_mut() {
        *ns = notes;
    }
    cmds.push(Command::AddClip {
        track: tid.clone(),
        clip,
    });
    let label = glaux_core::tr!(
        "{name}(ノート {count} 個)を配置",
        "Place {name} ({count} notes)"
    );
    let (_, m) = state
        .handle
        .apply(Command::batch(label.clone(), cmds), Author::Human, label)
        .await?
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "clip_id": clip_id,
        "track_id": tid,
        "notes": count,
        "project_version": m.project_version,
    }))
}

/// 録音を止めて WAV を確定し、音声トラックのクリップとして置く(履歴 1 件)。
/// `track_id` 省略時は最初の音声トラック、無ければ「録音」トラックを新設する。
#[tauri::command]
async fn record_stop(
    state: State<'_, AppState>,
    track_id: Option<String>,
    auto_gain: Option<bool>,
) -> Result<Value, String> {
    let engine = state.engine()?.clone();
    engine.pause();
    let outcome = engine.stop_recording().map_err(|e| e.to_string())?;
    let (result, start_tick, offset) = (outcome.result, outcome.clip_start, outcome.offset_samples);
    if result.frames <= offset {
        let _ = std::fs::remove_file(&result.path);
        return Err(t(
            "カウントインより後に録音データがありません",
            "No recorded audio after the count-in",
        )
        .to_owned());
    }
    if result.frames == 0 {
        let _ = std::fs::remove_file(&result.path);
        return Err(t(
            "録音データが空でした(入力デバイスの設定を確認してください)",
            "The recording was empty (check the input device settings)",
        )
        .to_owned());
    }
    let dir = state.project_dir();
    // 取り込み(ハッシュとコピー。1 分で約 23MB)は画面の応答を止めないよう別のスレッドで
    let imported = {
        let (dir, tmp) = (dir.clone(), result.path.clone());
        tokio::task::spawn_blocking(move || {
            let r = glaux_mcp::assets::import_wav(std::path::Path::new(&dir), &tmp);
            // 一時ファイルはハッシュ名でコピー済みなので消す
            let _ = std::fs::remove_file(&tmp);
            r
        })
        .await
        .map_err(|e| e.to_string())??
    };

    let (project, _) = state.handle.get_project_shared().await?;
    let mut cmds = Vec::new();
    let tid = match track_id {
        Some(id) => glaux_core::TrackId::parse(&id).map_err(|e| e.to_string())?,
        None => match project
            .tracks
            .iter()
            .find(|t| t.kind == glaux_core::TrackKind::Audio)
        {
            Some(t) => t.id.clone(),
            None => {
                let id = glaux_core::TrackId::new();
                cmds.push(Command::AddTrack {
                    track: glaux_core::Track::new(
                        id.clone(),
                        t("録音", "Recording"),
                        glaux_core::TrackKind::Audio,
                    ),
                    index: None,
                });
                id
            }
        },
    };
    // 新設トラックはまだ project に無いので、仮に足したコピーでコマンドを組む
    let mut project_view = (*project).clone();
    if let Some(Command::AddTrack { track, .. }) = cmds.first() {
        project_view.tracks.push(track.clone());
    }
    let clip_id = glaux_core::ClipId::new();
    let name = glaux_core::tr!(
        "録音 {}",
        "Recording {}",
        chrono::Local::now().format("%H:%M")
    );
    cmds.extend(glaux_mcp::assets::audio_clip_commands_with_offset(
        &project_view,
        &tid,
        &imported,
        clip_id.clone(),
        start_tick,
        &name,
        offset,
    )?);
    // 自動音量調整: 使う範囲のピークが -6dBFS になるようクリップの音量で持ち上げる
    // (元の波形は変えない。下げはしない)
    let mut gain_db = 0.0f32;
    if auto_gain.unwrap_or(true) {
        let wav = std::path::Path::new(&dir).join(&imported.asset.path);
        // 録った音を全部読んでピークを測る(長い録音では重いので別のスレッドで)
        let peak = tokio::task::spawn_blocking(move || {
            let data = glaux_engine::load_wav(&wav).ok()?;
            // ステレオは左右それぞれのピーク
            let (l, r) = data.left_right();
            let from = (offset as usize).min(l.len());
            Some(
                l[from..]
                    .iter()
                    .chain(&r[from..])
                    .fold(0.0f32, |m, v| m.max(v.abs())),
            )
        })
        .await
        .map_err(|e| e.to_string())?;
        if let Some(peak) = peak.filter(|p| *p > 1e-4) {
            gain_db = (-6.0 - 20.0 * peak.log10()).clamp(0.0, 30.0);
        }
        for c in &mut cmds {
            if let Command::AddClip { clip, .. } = c {
                if let glaux_core::ClipContent::Audio { gain_db: g, .. } = &mut clip.content {
                    *g = gain_db;
                }
            }
        }
    }
    let label = glaux_core::tr!("{name}(録音)を配置", "Place {name} (recording)");
    let (_, m) = state
        .handle
        .apply(Command::batch(label.clone(), cmds), Author::Human, label)
        .await?
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "clip_id": clip_id,
        "track_id": tid,
        "seconds": (result.frames - offset) as f64 / result.sample_rate as f64,
        "channels": result.channels,
        "clipped": result.clipped,
        "dropped": result.dropped,
        "gain_db": gain_db,
        "project_version": m.project_version,
    }))
}

// ---- オーディオデバイス ----------------------------------------------------

/// 認識しているデバイスの一覧と、使用中の出力・入力。
#[tauri::command]
fn audio_devices(state: State<'_, AppState>) -> Value {
    let list = glaux_engine::list_devices();
    let (output, input, sample_rate) = match &state.engine {
        Some(e) => (Some(e.output_device()), e.input_device(), e.sample_rate()),
        None => (None, list.default_input.clone(), 0.0),
    };
    let buffer = state.engine.as_ref().map(|e| {
        let b = e.buffer_info();
        json!({
            "requested": b.requested,
            "applied": b.applied,
            "min": b.min,
            "max": b.max,
            "block": e.block_frames(),
        })
    });
    json!({
        "buffer": buffer,
        "outputs": list.outputs,
        "inputs": list.inputs,
        "default_output": list.default_output,
        "default_input": list.default_input,
        "current_output": output,
        "current_input": input,
        "sample_rate": sample_rate,
    })
}

/// 出力デバイスを切り替える(name 省略 = OS 既定)。サンプルレートが変わりうるので
/// 再生データを作り直す。
#[tauri::command]
async fn set_output_device(
    state: State<'_, AppState>,
    name: Option<String>,
) -> Result<Value, String> {
    let engine = state.engine()?.clone();
    let name = name.filter(|n| !n.is_empty());
    let result = {
        let engine = engine.clone();
        tokio::task::spawn_blocking(move || engine.set_output_device(name))
            .await
            .map_err(|e| e.to_string())?
    };
    let (project, _) = state.handle.get_project_shared().await?;
    let dir = state.project_dir();
    {
        let engine = engine.clone();
        tokio::task::spawn_blocking(move || {
            engine.set_project(&project, std::path::Path::new(&dir))
        })
        .await
        .map_err(|e| e.to_string())?;
    }
    result.map_err(|e| e.to_string())?;
    Ok(json!({ "current_output": engine.output_device(), "sample_rate": engine.sample_rate() }))
}

/// 出力バッファの大きさ(フレーム)を変えて開き直す。0 なら既定(1024)
#[tauri::command]
async fn set_buffer_size(state: State<'_, AppState>, frames: u32) -> Result<Value, String> {
    let engine = state.engine()?.clone();
    let frames = if frames == 0 {
        glaux_engine::output::DEFAULT_BUFFER_FRAMES
    } else {
        frames
    };
    {
        let engine = engine.clone();
        tokio::task::spawn_blocking(move || engine.set_buffer_frames(frames))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
    }
    let b = engine.buffer_info();
    Ok(json!({ "requested": b.requested, "applied": b.applied, "min": b.min, "max": b.max }))
}

/// 録音に使う入力デバイス(name 省略 = OS 既定)。
#[tauri::command]
fn set_input_device(state: State<'_, AppState>, name: Option<String>) -> Result<Value, String> {
    let engine = state.engine()?;
    engine
        .set_input_device(name.filter(|n| !n.is_empty()))
        .map_err(|e| e.to_string())?;
    Ok(json!({ "current_input": engine.input_device() }))
}

/// 入力テスト(録音せずに入力レベルだけ測る)の開始・停止。
#[tauri::command]
fn input_monitor(state: State<'_, AppState>, on: bool) -> Result<(), String> {
    state
        .engine()?
        .set_input_monitor(on)
        .map_err(|e| e.to_string())
}

/// 遅延の較正を開始する: 曲頭からメトロノームだけを鳴らし、1 小節のカウントイン後の
/// 8 拍を録音する。戻り値の秒数が経ったら calibrate_stop を呼ぶ。
#[tauri::command]
async fn calibrate_start(state: State<'_, AppState>) -> Result<Value, String> {
    let engine = state.engine()?.clone();
    if engine.is_recording() {
        return Err(t("録音中は較正できません", "Can't calibrate while recording").to_owned());
    }
    let (project, _) = state.handle.get_project_shared().await?;
    let saved = engine.playhead_tick();
    engine.pause();
    engine.seek_tick(Tick(0));
    engine.set_click_only(true);
    let sig = project.time_sig_map.first();
    let (num, den) = sig
        .map(|e| (e.num as u64, e.den.max(1) as u64))
        .unwrap_or((4, 4));
    let beat_ticks = 3840 / den;
    let count_in = Tick(beat_ticks * num);
    let path = std::env::temp_dir().join(format!(
        "glaux_calib_{}.wav",
        chrono::Local::now().format("%H%M%S")
    ));
    let clip_start = match engine.start_recording(path, count_in, 0.0, true, false) {
        Ok(t) => t,
        Err(e) => {
            engine.set_click_only(false);
            engine.seek_tick(saved);
            return Err(e.to_string());
        }
    };
    const BEATS: u64 = 8;
    let tempo = &project.tempo_map;
    let base = tempo.tick_to_seconds(clip_start);
    let beats: Vec<f64> = (0..BEATS)
        .map(|k| tempo.tick_to_seconds(clip_start + Tick(k * beat_ticks)) - base)
        .collect();
    let total = tempo.tick_to_seconds(clip_start + Tick(BEATS * beat_ticks)) + 0.4;
    *state.calib.lock().expect("calib lock") = Some((saved, beats));
    engine.play();
    engine.mark_play_started();
    Ok(json!({
        "total_secs": total,
        "count_in_secs": tempo.tick_to_seconds(count_in),
        "beats": BEATS,
    }))
}

/// 較正を終えて遅延を推定する(失敗しても再生状態は元に戻す)。
#[tauri::command]
async fn calibrate_stop(state: State<'_, AppState>) -> Result<Value, String> {
    let engine = state.engine()?.clone();
    engine.pause();
    let taken = state.calib.lock().expect("calib lock").take();
    let outcome = engine.stop_recording();
    engine.set_click_only(false);
    let Some((saved, beats)) = taken else {
        return Err(t("較正を開始していません", "Calibration hasn't started").to_owned());
    };
    engine.seek_tick(saved);
    let outcome = outcome.map_err(|e| e.to_string())?;
    let path = outcome.result.path.clone();
    let est = tokio::task::spawn_blocking(move || {
        let mut data = glaux_engine::load_wav_mono(&path)?;
        let from = (outcome.offset_samples as usize).min(data.frames.len());
        data.frames.drain(..from);
        let _ = std::fs::remove_file(&path);
        glaux_engine::calibrate::estimate_latency(&data, &beats)
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(json!(est))
}

/// 音作りビュー用: トラックの音源・エフェクトの spec + 現在値 + path。
/// 追加できるエフェクトのカタログは [`get_effect_catalog`] で別に取る。
#[tauri::command]
async fn get_track_params(state: State<'_, AppState>, track_id: String) -> Result<Value, String> {
    let tid = glaux_core::TrackId::parse(&track_id).map_err(|e| e.to_string())?;
    let (project, version) = state.handle.get_project_shared().await?;
    if project.track(&tid).is_none() {
        return Err(glaux_core::tr!(
            "トラックが見つかりません: {track_id}",
            "Track not found: {track_id}"
        ));
    }
    // CLAP のつまみは初めてのとき、プラグインを読み込んで一覧を作るので別のスレッドで
    let mut v = off_thread(move || {
        let track = project.track(&tid).expect("確かめ済み");
        glaux_mcp::server::track_params_json(track)
    })
    .await??;
    v["project_version"] = json!(version);
    v["track_id"] = json!(track_id);
    Ok(v)
}

/// 追加できるエフェクトのカタログ(変わらないので、作るのは 1 回だけ。フロントも 1 回だけ取る)
#[tauri::command]
fn get_effect_catalog() -> Value {
    static CATALOG: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    CATALOG
        .get_or_init(|| serde_json::to_value(glaux_dsp::effect_catalog()).unwrap_or(Value::Null))
        .clone()
}

/// 音作りビュー(マスター)用: マスターバスのエフェクトチェーン。
#[tauri::command]
async fn get_master_params(state: State<'_, AppState>) -> Result<Value, String> {
    let (project, version) = state.handle.get_project_shared().await?;
    // CLAP のエフェクトのつまみは初めてのとき、プラグインを読み込んで一覧を作るので別のスレッドで
    off_thread(move || {
        json!({
            "track_id": "__master__",
            "device": { "name": "master", "is_default_fallback": false },
            "params": [],
            "effects": glaux_mcp::server::effects_json(&project.master.effects, project.master.fx_links.as_deref()),
            "fx_links": project.master.fx_links,
            "project_version": version,
        })
    })
    .await
}

// ---- SoundFont ------------------------------------------------------------

/// SoundFont・SFZ の一覧(SFZ のフォルダを 4 段まで見て回るので、別のスレッドで)
#[tauri::command]
async fn list_soundfonts() -> Result<Value, String> {
    off_thread(|| {
        let dir = glaux_engine::sf2::default_dir();
        let sfz_dir = glaux_engine::sfz::default_dir();
        json!({
            "dir": dir.to_string_lossy(),
            "files": glaux_engine::sf2::list_files(&dir),
            "sfz_dir": sfz_dir.to_string_lossy(),
            "sfz": glaux_engine::sfz::list_files(&sfz_dir),
            "packs": glaux_mcp::sfz_packs::status(&sfz_dir),
        })
    })
    .await
}

/// 無料の SFZ 音源を取得して SFZ ライブラリに入れる(利用者の操作で)。
/// 進捗は `sfz-download` イベント({id, got, total})で届く。
#[tauri::command]
async fn download_sfz_pack(app: tauri::AppHandle, id: String) -> Result<Value, String> {
    let instruments = tokio::task::spawn_blocking(move || {
        let lib = glaux_engine::sfz::default_dir();
        glaux_mcp::sfz_packs::download(&id, &lib, &mut |got, total| {
            let _ = app.emit(
                "sfz-download",
                json!({ "id": id, "got": got, "total": total }),
            );
        })
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(json!({ "instruments": instruments }))
}

/// .sf2 のプリセット一覧(重いのでブロッキングスレッドで)。
#[tauri::command]
async fn list_soundfont_presets(file: String) -> Result<Value, String> {
    let path = glaux_engine::sf2::default_dir().join(&file);
    let presets = tauri::async_runtime::spawn_blocking(move || -> Result<_, String> {
        let font = glaux_engine::sf2::load_font(&path)?;
        Ok(glaux_engine::sf2::list_presets(&font))
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(json!({ "file": file, "presets": presets }))
}

/// .sf2 をライブラリフォルダへコピーして登録する。
#[tauri::command]
async fn add_soundfont(path: String) -> Result<Value, String> {
    let src = std::path::PathBuf::from(&path);
    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| t("ファイル名が取れません", "Can't get the file name").to_owned())?;
    let dir = glaux_engine::sf2::default_dir();
    let dest = dir.join(&name);
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        // 検証を兼ねて一度パースする
        glaux_engine::sf2::load_font(&src)?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::copy(&src, &dest).map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(json!({ "file": name }))
}

// ---- 音色プリセット(glaux-mcp の presets モジュールを共用) ---------------

/// 音色プリセットの一覧(プリセットのファイルを全部読むので、別のスレッドで)
#[tauri::command]
async fn list_presets() -> Result<Value, String> {
    off_thread(
        || json!({ "presets": glaux_mcp::presets::list(&glaux_mcp::presets::default_dir()) }),
    )
    .await
}

/// トラックの現在の音(音源 + エフェクトチェーン)をプリセット保存する。
#[tauri::command]
async fn save_preset(
    state: State<'_, AppState>,
    track_id: String,
    name: String,
    overwrite: bool,
) -> Result<Value, String> {
    let tid = glaux_core::TrackId::parse(&track_id).map_err(|e| e.to_string())?;
    let (project, _) = state.handle.get_project_shared().await?;
    let track = project.track(&tid).ok_or_else(|| {
        glaux_core::tr!(
            "トラックが見つかりません: {track_id}",
            "Track not found: {track_id}"
        )
    })?;
    let preset = glaux_mcp::presets::save(
        &glaux_mcp::presets::default_dir(),
        track,
        &name,
        None,
        overwrite,
    )?;
    Ok(json!({ "saved": preset.name }))
}

/// プリセットをトラックに適用する(音源差し替え + エフェクト置換。1 undo)。
#[tauri::command]
async fn load_preset(
    state: State<'_, AppState>,
    track_id: String,
    name: String,
) -> Result<Value, String> {
    let tid = glaux_core::TrackId::parse(&track_id).map_err(|e| e.to_string())?;
    let (project, _) = state.handle.get_project_shared().await?;
    let track = project.track(&tid).ok_or_else(|| {
        glaux_core::tr!(
            "トラックが見つかりません: {track_id}",
            "Track not found: {track_id}"
        )
    })?;
    let preset = glaux_mcp::presets::load(&glaux_mcp::presets::default_dir(), &name)?;
    let label = glaux_core::tr!(
        "{} にプリセット「{}」を適用",
        "Apply preset \"{1}\" to {0}",
        track.name,
        preset.name
    );
    let cmds = glaux_mcp::presets::apply_commands(track, &preset);
    let (_, m) = state
        .handle
        .apply(Command::batch(label.clone(), cmds), Author::Human, label)
        .await?
        .map_err(|e| e.to_string())?;
    Ok(json!({ "applied": preset.name, "project_version": m.project_version }))
}

// ---- エフェクトのプリセット(glaux-mcp の fx_presets モジュールを共用) -------

#[tauri::command]
async fn list_fx_presets() -> Result<Value, String> {
    // ファイルを読むので、メインスレッドを止めないよう別のスレッドで
    tokio::task::spawn_blocking(
        || json!({ "presets": glaux_mcp::fx_presets::list(&glaux_mcp::fx_presets::default_dir()) }),
    )
    .await
    .map_err(|e| e.to_string())
}

/// エフェクト 1 つを名前を付けて保存する。`target` はトラック ID か "master"
#[tauri::command]
async fn save_fx_preset(
    state: State<'_, AppState>,
    target: String,
    fx_id: String,
    name: String,
    note: Option<String>,
    overwrite: bool,
) -> Result<Value, String> {
    let target = glaux_mcp::fx_presets::Target::parse(&target)?;
    let fx_id = glaux_core::FxId::parse(&fx_id).map_err(|e| e.to_string())?;
    let (project, _) = state.handle.get_project_shared().await?;
    let (effect, owner) = glaux_mcp::fx_presets::find_effect(&project, &target, &fx_id)?;
    let preset = glaux_mcp::fx_presets::save(
        &glaux_mcp::fx_presets::default_dir(),
        effect,
        &name,
        note,
        Some(owner),
        overwrite,
    )?;
    Ok(json!({ "saved": preset.name }))
}

/// エフェクトのプリセットを足す。`parked` なら、つながずに `pos` に置く。
/// `split: [from, to]` なら、その線の間に入れる(1 回の undo)
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn apply_fx_preset(
    state: State<'_, AppState>,
    target: String,
    name: String,
    index: Option<usize>,
    parked: bool,
    pos: Option<[f32; 2]>,
    split: Option<[String; 2]>,
) -> Result<Value, String> {
    let target = glaux_mcp::fx_presets::Target::parse(&target)?;
    let (project, _) = state.handle.get_project_shared().await?;
    let preset = glaux_mcp::fx_presets::load(&glaux_mcp::fx_presets::default_dir(), &name)?;
    let (command, fx_id) = glaux_mcp::fx_presets::add_command(
        &project,
        &target,
        &preset,
        index,
        parked || split.is_some(),
        pos,
    )?;
    let command = match split {
        None => command,
        Some([from, to]) => {
            glaux_mcp::fx_presets::split_command(&project, &target, command, &fx_id, &from, &to)?
        }
    };
    let label = glaux_core::tr!(
        "エフェクトのプリセット「{}」を追加",
        "Add effect preset \"{}\"",
        preset.name
    );
    let (_, m) = state
        .handle
        .apply(command, Author::Human, label)
        .await?
        .map_err(|e| e.to_string())?;
    Ok(json!({ "fx_id": fx_id, "project_version": m.project_version }))
}

#[tauri::command]
fn delete_fx_preset(name: String) -> Result<Value, String> {
    glaux_mcp::fx_presets::remove(&glaux_mcp::fx_presets::default_dir(), &name)?;
    Ok(json!({ "deleted": name }))
}

fn set_window_title(app: &tauri::AppHandle, title: &str) {
    use tauri::Manager;
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_title(&format!("Glaux — {title}"));
    }
}

/// プロジェクトを開く / 作成する(インプロセス切り替え)。
/// アクターが Session を差し替えるので、MCP・チャット・エンジンはそのまま追従する。
#[tauri::command]
async fn open_project(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    path: String,
    create: bool,
) -> Result<Value, String> {
    let path = path.trim().trim_end_matches(['/', '\\']).to_owned();
    if path.is_empty() {
        return Err(t("パスが空です", "The path is empty").to_owned());
    }
    let p = std::path::Path::new(&path);
    if !create && !p.join("project.json").exists() {
        return Err(glaux_core::tr!(
            "Glaux の曲のフォルダではありません(project.json が見つかりません): {path}\n\
             曲のフォルダ(例 MySong.glaux)そのものを選んでください",
            "Not a Glaux song folder (project.json not found): {path}\n\
             Select the song folder itself (e.g. MySong.glaux)"
        ));
    }

    if let Some(engine) = &state.engine {
        engine.stop();
        engine.clear_loop(); // ループ区間は前のプロジェクトの tick なので持ち越さない
        engine.set_ab_clip(None); // 聴き比べも前のプロジェクトのもの
    }
    let (title, version) = state.handle.switch_project(path.clone()).await?;
    state.chat.switch_project(path.clone());
    *state.project_dir.lock().expect("project_dir lock") = path.clone();
    projects::push_recent(&path, &title);
    set_window_title(&app, &title);
    Ok(json!({ "title": title, "project_version": version, "path": path }))
}

/// 現在のプロジェクトを移動 / 曲名を変更する(フォルダ名は曲名から作る)。
/// `dest_parent` 省略で場所は今のまま、`new_name` 省略で名前は今のまま。
/// `new_name` は曲名(meta.title。SetTitle コマンド、author: system)で、フォルダ名は曲名をファイル名として整えたもの。
#[tauri::command]
async fn move_project(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    dest_parent: Option<String>,
    new_name: Option<String>,
) -> Result<Value, String> {
    if state.chat.is_running() {
        return Err(t(
            "AI が作業中は移動できません。完了を待つか停止してください",
            "Can't move while the AI is working. Wait for it to finish or stop it",
        )
        .to_owned());
    }
    let current = state.project_dir();
    let cur = std::path::PathBuf::from(&current);
    let cur_stem = cur
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let cur_stem = cur_stem
        .strip_suffix(".glaux")
        .unwrap_or(&cur_stem)
        .to_owned();

    let parent = match dest_parent
        .map(|s| s.trim().trim_end_matches(['/', '\\']).to_owned())
        .filter(|s| !s.is_empty())
    {
        Some(p) => std::path::PathBuf::from(p),
        None => cur.parent().map(|p| p.to_path_buf()).ok_or_else(|| {
            t(
                "現在のプロジェクトの親フォルダが分かりません",
                "Can't determine the current project's parent folder",
            )
            .to_owned()
        })?,
    };
    // new_name は曲名。フォルダ名は曲名から作る(空白は _ など。同じ名前があれば -2 …)
    let new_title = new_name
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let folder = new_title
        .as_deref()
        .map(glaux_mcp::store::folder_name)
        .unwrap_or_else(|| cur_stem.clone());
    let same_place = cur.parent() == Some(parent.as_path()) && folder == cur_stem;
    if !same_place {
        // 自分の中や、別の曲のフォルダの中へは移さない(自分の中へ移すと複製が止まらず入れ子になった)
        glaux_mcp::store::check_project_parent(&parent)?;
    }
    let dest = if same_place {
        cur.clone()
    } else {
        glaux_mcp::store::unique_project_dir(&parent, &folder)
    };
    let dest_str = dest.to_string_lossy().into_owned();
    let (cur_title, _) = {
        let (p, v) = state.handle.get_project_shared().await?;
        (p.meta.title.clone(), v)
    };
    let retitle = new_title.as_ref().filter(|t| **t != cur_title).cloned();
    if dest == cur && retitle.is_none() {
        return Ok(json!({ "path": current, "moved": false }));
    }

    let moved = dest != cur;
    // 同一プロジェクトの移動なのでループ区間はそのまま有効
    let (mut title, mut version) = if moved {
        if let Some(engine) = &state.engine {
            engine.stop();
        }
        state.handle.move_project(dest_str.clone()).await?
    } else {
        (cur_title, 0)
    };

    // 曲名を変えたらタイトルを変える(履歴に載るので undo 可)
    if let Some(name) = retitle {
        let cmd = glaux_core::Command::SetTitle {
            title: name.clone(),
        };
        match state
            .handle
            .apply(
                cmd,
                glaux_core::Author::System,
                glaux_core::tr!(
                    "プロジェクト名を「{name}」に変更",
                    "Rename project to \"{name}\""
                ),
            )
            .await
        {
            Ok(Ok((_, m))) => {
                title = name.clone();
                version = m.project_version;
            }
            Ok(Err(e)) => tracing::warn!("タイトル変更に失敗: {e}"),
            Err(e) => tracing::warn!("タイトル変更に失敗: {e}"),
        }
    }

    if moved {
        state.chat.switch_project(dest_str.clone());
        *state.project_dir.lock().expect("project_dir lock") = dest_str.clone();
        projects::remove_recent(&current);
    }
    projects::push_recent(&dest_str, &title);
    set_window_title(&app, &title);
    Ok(json!({ "path": dest_str, "title": title, "project_version": version, "moved": moved }))
}

/// 新しい曲(または移動)で作られるフォルダを前もって返す(画面で「-2 になります」「ここには作れません」を先に見せる)。
/// `name` は曲名。`current` は移動のとき今の曲のフォルダ(同じ場所・同じ名前なら動かないので番号を付けない)。
#[tauri::command]
fn preview_project_dir(parent_dir: String, name: String, current: Option<String>) -> Value {
    let parent = if parent_dir.trim().is_empty() {
        projects::default_projects_dir()
    } else {
        parent_dir.trim().trim_end_matches(['/', '\\']).to_owned()
    };
    let parent = std::path::Path::new(&parent);
    let folder = glaux_mcp::store::folder_name(name.trim());
    let wanted = parent.join(format!("{folder}.glaux"));
    if current.is_some_and(|c| std::path::Path::new(&c) == wanted) {
        return json!({ "path": wanted.to_string_lossy(), "folder": format!("{folder}.glaux"), "renamed": false });
    }
    if let Err(e) = glaux_mcp::store::check_project_parent(parent) {
        return json!({ "error": e });
    }
    if !parent.is_dir() {
        return json!({ "error": glaux_core::tr!("フォルダが見つかりません: {}", "Folder not found: {}", parent.display()) });
    }
    let dir = glaux_mcp::store::unique_project_dir(parent, &folder);
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    json!({ "path": dir.to_string_lossy(), "folder": name, "renamed": dir != wanted })
}

/// 新規プロジェクトを作成して開く。`parent_dir/name.glaux` に作られる。
#[tauri::command]
async fn create_project(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    parent_dir: String,
    name: String,
) -> Result<Value, String> {
    let title = name.trim();
    if title.is_empty() {
        return Err(t("曲名が空です", "The song name is empty").to_owned());
    }
    let parent = if parent_dir.trim().is_empty() {
        projects::default_projects_dir()
    } else {
        parent_dir.trim().to_owned()
    };
    glaux_mcp::store::check_project_parent(std::path::Path::new(&parent))?;
    // 曲名はそのまま、フォルダ名だけファイル名として整える(空白は _ など。同じ名前があれば -2 …)
    let dir = glaux_mcp::store::unique_project_dir(
        std::path::Path::new(&parent),
        &glaux_mcp::store::folder_name(title),
    );
    glaux_mcp::store::create_project(&dir, title).map_err(|e| e.to_string())?;
    open_project(app, state, dir.to_string_lossy().into_owned(), false).await
}

/// UI からの編集。MCP と同じく `Command` JSON を受け、author を `human` として適用する。
/// (原則: UI も AI も同じ Command API を通る。これにより UI 操作も履歴に載り、
/// AI が get_history でキャッチアップできる)
#[tauri::command]
async fn apply_edit(
    state: State<'_, AppState>,
    commands: Vec<Value>,
    label: String,
) -> Result<Value, String> {
    if commands.is_empty() {
        return Err(t("commands が空です", "commands is empty").to_owned());
    }
    let mut parsed = Vec::with_capacity(commands.len());
    for (i, value) in commands.into_iter().enumerate() {
        let cmd: Command = serde_json::from_value(value).map_err(|e| {
            glaux_core::tr!(
                "commands[{i}] を Command として解釈できません: {e}",
                "Can't parse commands[{i}] as a Command: {e}"
            )
        })?;
        parsed.push(cmd);
    }
    let command = if parsed.len() == 1 {
        parsed.pop().expect("len checked")
    } else {
        Command::batch(label.clone(), parsed)
    };
    let (entry_id, m) = state
        .handle
        .apply(command, Author::Human, label)
        .await?
        .map_err(|e| e.to_string())?;
    Ok(json!({ "entry_id": entry_id, "project_version": m.project_version }))
}

/// スライダーをドラッグしている間の試聴: いまのプロジェクトの複製にコマンドを当てて、エンジンにだけ渡す。
/// 履歴にもプロジェクトにも残らない(離したときに `apply_edit` で 1 回だけ確定する。
/// 確定すると、いつもどおりプロジェクトの変更としてエンジンが作り直される)。
#[tauri::command]
async fn preview_edit(state: State<'_, AppState>, commands: Vec<Value>) -> Result<(), String> {
    let engine = state.engine()?.clone();
    let (mut project, _) = state.handle.get_project().await?;
    for (i, value) in commands.into_iter().enumerate() {
        let cmd: Command = serde_json::from_value(value).map_err(|e| {
            glaux_core::tr!(
                "commands[{i}] を Command として解釈できません: {e}",
                "Can't parse commands[{i}] as a Command: {e}"
            )
        })?;
        project.apply(&cmd).map_err(|e| e.to_string())?;
    }
    let dir = state.project_dir();
    tokio::task::spawn_blocking(move || engine.set_project(&project, std::path::Path::new(&dir)))
        .await
        .map_err(|e| e.to_string())
}

/// 履歴の途中のエントリを 1 件だけ取り消す(`git revert` 相当)。
/// 逆コマンドが新エントリとして積まれるので、取り消し自体も undo できる。
#[tauri::command]
async fn revert_entry(state: State<'_, AppState>, entry_id: String) -> Result<Value, String> {
    let id = EntryId::parse(&entry_id).map_err(|e| e.to_string())?;
    let (entry, conflicts, m) = state
        .handle
        .revert_entry(id, Author::Human)
        .await?
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "entry_id": entry,
        "conflicts": conflicts,
        "project_version": m.project_version,
    }))
}

/// 書き出し(形式・範囲・音量の目標・トラックごとを選べる)。結果は書いたファイルと測定値
#[tauri::command]
async fn export_audio(
    state: State<'_, AppState>,
    request: glaux_mcp::export::ExportRequest,
) -> Result<Value, String> {
    let (project, _) = state.handle.get_project_shared().await?;
    let dir = state.project_dir();
    tauri::async_runtime::spawn_blocking(move || {
        let dir = std::path::Path::new(&dir);
        let bank = glaux_engine::SampleBank::for_offline(&project, dir);
        glaux_mcp::export::run(&project, dir, &request, &bank)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// MIDI ファイル・MusicXML を読み込む(パートごとに新しいトラック。1 件の履歴)
#[tauri::command]
async fn import_midi(
    state: State<'_, AppState>,
    request: glaux_mcp::midi::ImportMidiRequest,
) -> Result<Value, String> {
    let (project, _) = state.handle.get_project_shared().await?;
    // 楽譜(MusicXML / .mxl)なら MusicXML として読む(強弱・奏法・パート名なども移る)
    let (imp, report) = tauri::async_runtime::spawn_blocking(move || {
        if glaux_mcp::musicxml_in::is_score_file(&request.path) {
            glaux_mcp::musicxml_in::import_file(&project, &request)
        } else {
            glaux_mcp::midi::import_file(&project, &request).map(|i| (i, vec![]))
        }
    })
    .await
    .map_err(|e| e.to_string())??;
    let (entry_id, m) = state
        .handle
        .apply(
            Command::batch(imp.label.clone(), imp.commands),
            Author::Human,
            imp.label,
        )
        .await?
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "entry_id": entry_id,
        "tracks": imp.tracks.len(),
        "notes": imp.tracks.iter().map(|t| t.2).sum::<usize>(),
        "tempo_set": imp.tempo_set,
        "report": report,
        "project_version": m.project_version,
    }))
}

/// MIDI ファイルに書き出す(`path` 省略でプロジェクトの export/)
#[tauri::command]
async fn export_midi(state: State<'_, AppState>, path: Option<String>) -> Result<Value, String> {
    let (project, _) = state.handle.get_project_shared().await?;
    let dir = state.project_dir();
    off_thread(move || {
        glaux_mcp::midi::export_file(&project, std::path::Path::new(&dir), path.as_deref())
    })
    .await?
}

/// キーと小節ごとのコード(ノートからの推定)と、キーのスケールの音(ピッチクラス)。画面の表示用
#[tauri::command]
async fn harmony(state: State<'_, AppState>) -> Result<Value, String> {
    let (project, _) = state.handle.get_project_shared().await?;
    // 曲全体の和音の推定(編集のたびに画面から呼ばれる。大きな曲では重いので別のスレッドで)
    off_thread(move || {
        let a = glaux_core::harmony::analyze(&project, None, None);
        let scale = a
            .key
            .as_ref()
            .map(|k| glaux_core::harmony::scale_pitch_classes(k.tonic, k.mode))
            .unwrap_or_default();
        let mut v = serde_json::to_value(&a).map_err(|e| e.to_string())?;
        v["scale"] = json!(scale);
        Ok(v)
    })
    .await?
}

/// 設計画面の中身: 曲の計画と実際の音(区間の盛り上がり・パート × 区間・ずれ・クリップの状態)、計画の一覧と計画の履歴
#[tauri::command]
async fn get_design(state: State<'_, AppState>, limit: Option<usize>) -> Result<Value, String> {
    glaux_mcp::plan_view::design(&state.handle, limit.unwrap_or(100)).await
}

/// 画面から計画を保存する(作る・丸ごと置き換える。人の操作として計画の履歴に残る)
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn plan_save(
    state: State<'_, AppState>,
    plan_id: Option<String>,
    name: Option<String>,
    kind: String,
    body: Value,
    plan_state: Option<String>,
    label: String,
) -> Result<Value, String> {
    glaux_mcp::plan_view::save(
        &state.handle,
        plan_id.as_deref(),
        name.as_deref(),
        &kind,
        body,
        plan_state.as_deref(),
        &label,
    )
    .await
}

#[tauri::command]
async fn plan_delete(
    state: State<'_, AppState>,
    plan_id: String,
    label: String,
) -> Result<Value, String> {
    glaux_mcp::plan_view::delete(&state.handle, &plan_id, &label).await
}

/// 計画の取り消し・やり直し
#[tauri::command]
async fn plan_step(
    state: State<'_, AppState>,
    n: Option<usize>,
    redo: bool,
) -> Result<Value, String> {
    glaux_mcp::plan_view::step(&state.handle, n.unwrap_or(1), redo).await
}

/// 計画の途中の変更だけを取り消す
#[tauri::command]
async fn plan_revert(state: State<'_, AppState>, entry_id: String) -> Result<Value, String> {
    glaux_mcp::plan_view::revert(&state.handle, &entry_id).await
}

/// 計画を前の版の中身に戻す
#[tauri::command]
async fn plan_restore(
    state: State<'_, AppState>,
    plan_id: String,
    rev: u64,
) -> Result<Value, String> {
    glaux_mcp::plan_view::restore(&state.handle, &plan_id, rev).await
}

/// 推定した計画をまとめて採用する・捨てる
#[tauri::command]
async fn plan_settle_estimated(state: State<'_, AppState>, adopt: bool) -> Result<Value, String> {
    glaux_mcp::plan_view::settle_estimated(&state.handle, adopt).await
}

/// タイムラインのクリップの印(計画どおり / 計画が先に進んだ・手で直した小節・固定の音の数)
#[tauri::command]
async fn clip_states(state: State<'_, AppState>) -> Result<Value, String> {
    glaux_mcp::plan_view::clip_states(&state.handle).await
}

/// トラックを音声にする(フリーズ)。描き出して直後に音声トラックとして置き、元はミュート(1 件の履歴)
#[tauri::command]
async fn bounce_track(state: State<'_, AppState>, track_id: String) -> Result<Value, String> {
    let id = glaux_core::TrackId::parse(&track_id).map_err(|e| e.to_string())?;
    let (project, _) = state.handle.get_project_shared().await?;
    let dir = state.project_dir();
    let b = tauri::async_runtime::spawn_blocking(move || {
        let dir = std::path::Path::new(&dir);
        let bank = glaux_engine::SampleBank::for_offline(&project, dir);
        glaux_mcp::bounce::bounce_track(&project, dir, &id, &bank)
    })
    .await
    .map_err(|e| e.to_string())??;
    let (entry_id, m) = state
        .handle
        .apply(
            Command::batch(b.label.clone(), b.commands),
            Author::Human,
            b.label,
        )
        .await?
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "entry_id": entry_id,
        "new_track": b.new_track,
        "seconds": b.seconds,
        "project_version": m.project_version,
    }))
}

// ---- チャットの 1 ターン分の編集 ------------------------------------------

/// `since`(ターンの開始前の最後の履歴エントリ。None なら先頭から)より後の、AI の編集
async fn ai_entries_since(
    state: &AppState,
    since: Option<String>,
) -> Result<Vec<glaux_core::HistoryEntry>, String> {
    let since = match since {
        Some(s) => Some(EntryId::parse(&s).map_err(|e| e.to_string())?),
        None => None,
    };
    let entries = state
        .handle
        .get_entries(since, 1000)
        .await?
        .map_err(|e| e.to_string())?;
    Ok(entries
        .into_iter()
        .filter(|e| matches!(e.author, Author::Ai { .. }))
        .collect())
}

/// チャットの 1 ターンで AI が行った編集の要約(件数・変わったクリップとノート)。
/// 画面で「このターンを取り消す」を出し、変わった所を縁取りするのに使う
#[tauri::command]
async fn turn_changes(state: State<'_, AppState>, since: Option<String>) -> Result<Value, String> {
    let entries = ai_entries_since(&state, since).await?;
    let refs: Vec<&glaux_core::HistoryEntry> = entries.iter().collect();
    let (project, _) = state.handle.get_project_shared().await?;
    let mut v = glaux_mcp::changes::summarize(&refs, &project);
    v["entry_ids"] = json!(entries.iter().map(|e| e.id.to_string()).collect::<Vec<_>>());
    Ok(v)
}

/// 送った指示を直して送り直すときに戻す所: 曲の履歴の `since` より後の項目(AI も人も)と、計画の履歴のうち
/// その時刻より後で、曲の編集と一組でないもの(一組のものは曲を戻すと一緒に戻る)
async fn rewind_targets(
    state: &AppState,
    since: Option<String>,
    after_ms: Option<f64>,
) -> Result<(Vec<glaux_core::HistoryEntry>, Vec<(EntryId, bool)>), String> {
    let since_id = match &since {
        Some(s) => Some(EntryId::parse(s).map_err(|e| e.to_string())?),
        None => None,
    };
    let song = state
        .handle
        .get_entries(since_id.clone(), 1000)
        .await?
        .map_err(|e| e.to_string())?;
    // 計画は時刻で切る(指示を送った時刻。無ければ since の項目の時刻、それも無ければ全部)
    let after: Option<chrono::DateTime<chrono::Utc>> = match after_ms {
        Some(ms) => chrono::DateTime::from_timestamp_millis(ms as i64),
        None => match since_id {
            Some(id) => state
                .handle
                .get_entries(None, 100_000)
                .await?
                .map_err(|e| e.to_string())?
                .into_iter()
                .find(|e| e.id == id)
                .map(|e| e.timestamp),
            None => None,
        },
    };
    let plans = state.handle.get_plans().await?;
    let plan: Vec<(EntryId, bool)> = plans
        .history()
        .applied()
        .iter()
        .filter(|e| after.is_none_or(|t| e.timestamp >= t))
        .filter(|e| !e.note.as_ref().is_some_and(|n| n.song_entry.is_some()))
        .map(|e| (e.id.clone(), matches!(e.author, Author::Human)))
        .collect();
    Ok((song, plan))
}

/// 送った指示を直して送り直す前に、戻る編集の数(人の分も)を数える(確かめの表示用)
#[tauri::command]
async fn chat_rewind_preview(
    state: State<'_, AppState>,
    since: Option<String>,
    after_ms: Option<f64>,
) -> Result<Value, String> {
    let (song, plan) = rewind_targets(&state, since, after_ms).await?;
    let human = song
        .iter()
        .filter(|e| matches!(e.author, Author::Human))
        .count()
        + plan.iter().filter(|(_, h)| *h).count();
    Ok(json!({ "song": song.len(), "plan": plan.len(), "human": human }))
}

/// 送った指示を直して送り直す: その指示より後の編集を、AI も人も新しい順に取り消し(取り消しも履歴に残る)、
/// 会話もその指示の前まで戻す(Claude は一つ前のターンの終わり `chain_end` から再開。無ければ新しい会話)
#[tauri::command]
async fn chat_rewind(
    state: State<'_, AppState>,
    since: Option<String>,
    after_ms: Option<f64>,
    chain_end: Option<String>,
) -> Result<Value, String> {
    if state.chat.is_running() {
        return Err(glaux_core::i18n::t(
            "AI が作業中です。止めてから送り直してください",
            "The AI is working. Stop it before resending",
        )
        .to_owned());
    }
    // 計画は先に数えておく(曲を戻すと、一組の計画の変更が新しい項目として積まれるため)
    let (song, plan) = rewind_targets(&state, since, after_ms).await?;
    for e in song.iter().rev() {
        state
            .handle
            .revert_entry(e.id.clone(), Author::Human)
            .await?
            .map_err(|err| {
                glaux_core::tr!(
                    "「{}」を取り消せませんでした: {err}",
                    "Couldn't undo \"{}\": {err}",
                    e.label
                )
            })?;
    }
    let note = glaux_core::EntryNote {
        why: glaux_core::i18n::t(
            "送った指示を直して送り直すため、その指示より後の変更を戻す",
            "Rewinding to resend an edited instruction",
        )
        .to_owned(),
        ..Default::default()
    };
    for (id, _) in plan.iter().rev() {
        // 前に戻した項目などで取り消せないものは飛ばす
        let _ = state
            .handle
            .revert_plan(id.clone(), Author::Human, note.clone())
            .await;
    }
    // 会話を巻き戻す。戻した編集(人の取り消し)は AI に「人の編集」として見せない
    state.chat.rewind(chain_end);
    let latest = state
        .handle
        .get_entries(None, 100_000)
        .await?
        .map_err(|e| e.to_string())?
        .last()
        .map(|e| e.id.to_string());
    state.chat.set_last_seen_entry(latest);
    Ok(json!({ "song": song.len(), "plan": plan.len() }))
}

/// チャットの 1 ターンで AI が行った編集を、新しい順に取り消す(revert。途中の人間の編集は残る)。
/// 取り消し自体も履歴に載るので undo できる
#[tauri::command]
async fn revert_turn(state: State<'_, AppState>, since: Option<String>) -> Result<Value, String> {
    let entries = ai_entries_since(&state, since).await?;
    let mut reverted = 0usize;
    let mut conflicts: Vec<String> = Vec::new();
    for e in entries.iter().rev() {
        let (_, c, _) = state
            .handle
            .revert_entry(e.id.clone(), Author::Human)
            .await?
            .map_err(|err| {
                glaux_core::tr!(
                    "「{}」を取り消せませんでした: {err}",
                    "Couldn't undo \"{}\": {err}",
                    e.label
                )
            })?;
        conflicts.extend(c.into_iter().map(|x| x.to_string()));
        reverted += 1;
    }
    Ok(json!({ "reverted": reverted, "conflicts": conflicts }))
}

// ---- トランスポート(再生) ----------------------------------------------

/// ミキサーのメーター: トラック(プロジェクトの並び)とマスターの直近のピーク(dBFS)
fn levels_json(e: &EngineHandle) -> Value {
    let (tracks, master) = e.take_levels();
    let (loads, master_load) = e.take_loads();
    json!({
        "tracks": tracks,
        "master": master,
        "correlation": e.correlation(),
        "loads": loads,
        "master_load": master_load,
    })
}

#[tauri::command]
fn transport_state(state: State<'_, AppState>) -> Value {
    match &state.engine {
        Some(e) => json!({
            "available": true,
            "playing": e.is_playing(),
            "recording": e.is_recording() || e.is_midi_recording(),
            "midi_recording": e.is_midi_recording(),
            "midi_idle_ms": e.midi_idle_ms(),
            "metronome": e.metronome(),
            "input_peak_db": e.take_input_peak_db(),
            "input_monitor": e.input_monitoring(),
            "dsp": e.take_stats(),
            "levels": levels_json(e),
            "monitor": {
                "mode": e.monitor().0.name(),
                "crossfeed": e.monitor().1,
                "speaker": e.speaker().name(),
            },
            "loudness": e.loudness(),
            "tick": e.playhead_tick(),
            "loop": e.loop_region().map(|(s, gl_end)| json!([s, gl_end])),
        }),
        None => {
            json!({ "available": false, "playing": false, "recording": false, "metronome": false, "tick": 0, "loop": null })
        }
    }
}

/// ループ区間を設定する(tick)。再生位置が終端に達すると区間頭へ戻る。
#[tauri::command]
fn transport_set_loop(
    state: State<'_, AppState>,
    start_tick: u64,
    end_tick: u64,
) -> Result<(), String> {
    state.engine()?.set_loop(Tick(start_tick), Tick(end_tick))
}

#[tauri::command]
fn transport_clear_loop(state: State<'_, AppState>) -> Result<(), String> {
    state.engine()?.clear_loop();
    Ok(())
}

/// 聴き方(stereo / mono / side / swap)とクロスフィード。書き出しには入らない
#[tauri::command]
fn transport_set_monitor(
    state: State<'_, AppState>,
    mode: String,
    crossfeed: bool,
) -> Result<(), String> {
    let mode = glaux_engine::monitor::MonitorMode::from_name(&mode).ok_or_else(|| {
        glaux_core::tr!(
            "聴き方「{mode}」は分かりません",
            "Unknown monitor mode \"{mode}\""
        )
    })?;
    state.engine()?.set_monitor(mode, crossfeed);
    Ok(())
}

/// マスターの直近のスペクトル(1/3 オクターブ。帯域の中心 Hz と dB)
#[tauri::command]
fn transport_spectrum(state: State<'_, AppState>) -> Result<Value, String> {
    let e = state.engine()?;
    Ok(json!({ "bands": glaux_engine::monitor::SPECTRUM_BANDS, "db": e.spectrum() }))
}

/// 聴き比べる範囲の上限(秒)。音の数だけ書き出すので、長すぎると待たされる
const AB_MAX_SECS: f64 = 90.0;

/// 案(設計データの枝)の聴き比べを用意する: A = 今の曲、B・C … = 今の曲にそれぞれの案の音を当てたもの
/// (いちばん小さい音量にそろえる。案は 4 つまで)。用意できたら B(最初の案)を鳴らす状態にする
#[tauri::command]
async fn ab_prepare_proposals(
    state: State<'_, AppState>,
    plan_ids: Vec<String>,
    start_tick: u64,
    end_tick: u64,
) -> Result<Value, String> {
    if end_tick <= start_tick {
        return Err(t("聴き比べる範囲がありません", "No range to compare").into());
    }
    if plan_ids.is_empty() || plan_ids.len() >= glaux_engine::ab::MAX_TAKES {
        return Err(glaux_core::tr!(
            "いっしょに聴き比べられる案は 1〜{} 個です",
            "You can compare 1 to {} proposals at once",
            glaux_engine::ab::MAX_TAKES - 1
        ));
    }
    // 曲は 1 回だけ取り、どの案も同じ曲に当てる(取る合間に曲が変わって、A と B・C の元が食い違わないように)
    let (now, alts) =
        glaux_mcp::plan_view::proposal_projects_many(&state.handle, &plan_ids).await?;
    let engine = state.engine()?.clone();
    let sr = engine.sample_rate();
    let dir = state.handle.project_dir().await?;
    let from = now.tempo_map.tick_to_seconds(Tick(start_tick));
    let to = now
        .tempo_map
        .tick_to_seconds(Tick(end_tick))
        .min(from + AB_MAX_SECS);
    let end_tick = now.tempo_map.seconds_to_tick(to).0;
    let (clip, info) = tokio::task::spawn_blocking(move || {
        let dir = std::path::Path::new(&dir);
        let songs: Vec<glaux_core::Project> = std::iter::once(now).chain(alts).collect();
        let banks: Vec<glaux_engine::SampleBank> = songs
            .iter()
            .map(|p| glaux_engine::SampleBank::for_offline(p, dir))
            .collect();
        let pairs: Vec<_> = songs.iter().zip(banks.iter()).collect();
        glaux_engine::ab::prepare_many(&pairs, sr, from, to)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| {
        glaux_core::tr!(
            "聴き比べを用意できません: {e}",
            "Can't prepare the comparison: {e}"
        )
    })?;
    engine.set_ab_clip(Some(clip));
    engine.set_ab_side(glaux_engine::ab::AbSide::B);
    let mut v = serde_json::to_value(&info).map_err(|e| e.to_string())?;
    v["end_tick"] = json!(end_tick);
    v["plan_ids"] = json!(plan_ids);
    Ok(v)
}

/// 計画の無い所を、今の音から推定して保存する(推定 = 未確認)
#[tauri::command]
async fn plan_estimate(state: State<'_, AppState>) -> Result<Value, String> {
    glaux_mcp::plan_view::estimate(&state.handle).await
}

/// 計画の履歴のいちばん新しい側の項目の ID と、やり直せる項目の ID(設計画面の Ctrl+Z の確かめ)
#[tauri::command]
async fn plan_head(
    state: State<'_, AppState>,
    n: usize,
    ids: Option<Vec<String>>,
) -> Result<Value, String> {
    glaux_mcp::plan_view::plan_head(&state.handle, n.clamp(1, 64), &ids.unwrap_or_default()).await
}

/// 案を採用する(案の音を曲に当て、案の計画を今の計画にする)
#[tauri::command]
async fn plan_adopt_proposal(
    state: State<'_, AppState>,
    plan_id: String,
    also_discard: Option<Vec<String>>,
) -> Result<Value, String> {
    glaux_mcp::plan_view::adopt_proposal(&state.handle, &plan_id, &also_discard.unwrap_or_default())
        .await
}

/// 人に見せる文(履歴の名前・計画と実際のずれの説明・エラーなど、裏側で作る文)の言語を設定する(設定の「言語」)
#[tauri::command]
fn set_ui_language(lang: String) {
    glaux_core::i18n::set_english(lang == "en");
}

/// 設計画面で区間を直す: 区間の位置・名前・数が変わったら曲の区間を置き換え(曲の履歴)、盛り上がり・形・境目・
/// 鳴らすトラック・メモは曲全体の計画に書く(計画の履歴。曲も変えたなら一組)。返り値は書いた履歴の項目
#[tauri::command]
async fn design_edit_sections(
    state: State<'_, AppState>,
    sections: Vec<glaux_core::SectionMarker>,
    label: String,
) -> Result<Value, String> {
    let (song, plan) = glaux_mcp::plan_view::edit_sections(
        &state.handle,
        glaux_core::Author::Human,
        sections,
        &label,
    )
    .await?;
    Ok(json!({ "entry_id": song, "plan_entry_id": plan }))
}

/// 履歴の編集 `before_entry` の前と今の、音の違う範囲(tick)と、曲全体に効く違いがあるか(チャットのターンを聴き比べる範囲)
#[tauri::command]
async fn change_ranges(state: State<'_, AppState>, before_entry: String) -> Result<Value, String> {
    let id = EntryId::parse(&before_entry).map_err(|e| e.to_string())?;
    let (before, after, _, _) = state
        .handle
        .project_at(glaux_core::HistoryPoint::BeforeEntry(id))
        .await?
        .map_err(|e| e.to_string())?;
    let d =
        tokio::task::spawn_blocking(move || glaux_core::designcheck::song_diff(&before, &after))
            .await
            .map_err(|e| e.to_string())?;
    Ok(json!({ "ranges": d.ranges, "whole": d.whole, "silent_only": d.silent_only }))
}

/// 音量をそろえた A/B の聴き比べを用意する: 履歴のある地点(既定は 1 つ前の編集の前)と今の、
/// 同じ範囲を書き出して統合ラウドネスをそろえる。用意できたら B(今)を鳴らす状態にする
#[tauri::command]
async fn ab_prepare(
    state: State<'_, AppState>,
    before_entry: Option<String>,
    checkpoint: Option<String>,
    start_tick: u64,
    end_tick: u64,
) -> Result<Value, String> {
    let point = match (checkpoint, before_entry) {
        (Some(c), _) => glaux_core::HistoryPoint::Checkpoint(c),
        (None, Some(e)) => {
            glaux_core::HistoryPoint::BeforeEntry(EntryId::parse(&e).map_err(|e| e.to_string())?)
        }
        (None, None) => glaux_core::HistoryPoint::Back(1),
    };
    if end_tick <= start_tick {
        return Err(t("聴き比べる範囲がありません", "No range to compare").into());
    }
    let (before, after, _version, back) = state
        .handle
        .project_at(point)
        .await?
        .map_err(|e| e.to_string())?;
    if back == 0 {
        return Err(t(
            "比べる編集がありません(その地点は今と同じです)",
            "No edits to compare (that point is the same as now)",
        )
        .into());
    }
    let engine = state.engine()?.clone();
    let sr = engine.sample_rate();
    let dir = state.handle.project_dir().await?;
    let from = after.tempo_map.tick_to_seconds(Tick(start_tick));
    let to = after
        .tempo_map
        .tick_to_seconds(Tick(end_tick))
        .min(from + AB_MAX_SECS);
    let end_tick = after.tempo_map.seconds_to_tick(to).0;
    let (clip, info) = tokio::task::spawn_blocking(move || {
        let dir = std::path::Path::new(&dir);
        let bank_a = glaux_engine::SampleBank::for_offline(&before, dir);
        let bank_b = glaux_engine::SampleBank::for_offline(&after, dir);
        glaux_engine::ab::prepare(&before, &bank_a, &after, &bank_b, sr, from, to)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| {
        glaux_core::tr!(
            "聴き比べを用意できません: {e}",
            "Can't prepare the comparison: {e}"
        )
    })?;
    engine.set_ab_clip(Some(clip));
    engine.set_ab_side(glaux_engine::ab::AbSide::B);
    let mut v = serde_json::to_value(&info).map_err(|e| e.to_string())?;
    v["edits_compared"] = json!(back);
    v["end_tick"] = json!(end_tick);
    Ok(v)
}

/// 聴き比べでどれを鳴らすか("a" = 前・今、"b"〜"e" = 後・案、"off" = ふつうの再生)
#[tauri::command]
fn ab_set_side(state: State<'_, AppState>, side: String) -> Result<(), String> {
    let side = match side.as_bytes() {
        b"off" => glaux_engine::ab::AbSide::Off,
        [c @ b'a'..=b'e'] => glaux_engine::ab::AbSide::Take(c - b'a'),
        _ => return Err(format!("side は a〜e / off(got: {side})")),
    };
    state.engine()?.set_ab_side(side);
    Ok(())
}

/// 聴き比べを終える(用意した音を片付け、ふつうの再生に戻す)
#[tauri::command]
fn ab_clear(state: State<'_, AppState>) -> Result<(), String> {
    state.engine()?.set_ab_clip(None);
    Ok(())
}

/// ラウドネスメーターの統合値と True Peak の最大を測り直す
#[tauri::command]
fn transport_reset_loudness(state: State<'_, AppState>) -> Result<(), String> {
    state.engine()?.reset_loudness();
    Ok(())
}

/// 小さなスピーカーのシミュレーション(off / phone / laptop)。書き出しには入らない
#[tauri::command]
fn transport_set_speaker(state: State<'_, AppState>, speaker: String) -> Result<(), String> {
    let sp = glaux_engine::monitor::Speaker::from_name(&speaker).ok_or_else(|| {
        glaux_core::tr!(
            "スピーカー「{speaker}」は分かりません",
            "Unknown speaker \"{speaker}\""
        )
    })?;
    state.engine()?.set_speaker(sp);
    Ok(())
}

/// アプリから鳴る音の音量(dB)。聴く音量だけで、曲・メーター・書き出しには入らない
#[tauri::command]
fn transport_set_output_volume(state: State<'_, AppState>, db: f32) -> Result<(), String> {
    state.engine()?.set_output_volume_db(db);
    Ok(())
}

/// ゴニオメーターの点(古い順の [左, 右])
#[tauri::command]
fn transport_scope(state: State<'_, AppState>) -> Result<Vec<[f32; 2]>, String> {
    Ok(state
        .engine()?
        .scope_points()
        .into_iter()
        .map(|(l, r)| [l, r])
        .collect())
}

#[tauri::command]
fn transport_set_metronome(state: State<'_, AppState>, on: bool) -> Result<(), String> {
    state.engine()?.set_metronome(on);
    Ok(())
}

#[tauri::command]
fn transport_play(state: State<'_, AppState>) -> Result<(), String> {
    state.engine()?.play();
    Ok(())
}

#[tauri::command]
fn transport_pause(state: State<'_, AppState>) -> Result<(), String> {
    state.engine()?.pause();
    Ok(())
}

#[tauri::command]
fn transport_stop(state: State<'_, AppState>) -> Result<(), String> {
    state.engine()?.stop();
    Ok(())
}

#[tauri::command]
fn transport_seek(state: State<'_, AppState>, tick: u64) -> Result<(), String> {
    state.engine()?.seek_tick(Tick(tick));
    Ok(())
}

/// ノートを 1 音だけ試聴する(ピアノロール編集のフィードバック)。
#[tauri::command]
async fn preview_note(
    state: State<'_, AppState>,
    track_id: String,
    pitch: u8,
) -> Result<(), String> {
    let engine = state.engine()?.clone();
    let track_id = glaux_core::TrackId::parse(&track_id).map_err(|e| e.to_string())?;
    let (project, _) = state.handle.get_project_shared().await?;
    let index = project
        .track_index(&track_id)
        .ok_or_else(|| format!("track not found: {track_id}"))?;
    engine.preview_note(index, pitch.min(127), 100, 250);
    Ok(())
}

/// 音色エディタの試しの鍵盤: 押している間だけ鳴らす(離すのは live_note_off)。`vel` は 1〜127(既定 100)
#[tauri::command]
async fn live_note_on(
    state: State<'_, AppState>,
    track_id: String,
    pitch: u8,
    vel: Option<u8>,
) -> Result<(), String> {
    let engine = state.engine()?.clone();
    let track_id = glaux_core::TrackId::parse(&track_id).map_err(|e| e.to_string())?;
    let (project, _) = state.handle.get_project_shared().await?;
    let index = project
        .track_index(&track_id)
        .ok_or_else(|| format!("track not found: {track_id}"))?;
    engine.live_note_on(index, pitch, vel.unwrap_or(100));
    Ok(())
}

#[tauri::command]
fn live_note_off(state: State<'_, AppState>, pitch: u8) -> Result<(), String> {
    state.engine()?.live_note_off(pitch);
    Ok(())
}

#[tauri::command]
fn live_all_off(state: State<'_, AppState>) -> Result<(), String> {
    state.engine()?.live_all_off();
    Ok(())
}

/// 音色エディタの「出口の音の倍音」: トラックの音源(`device` を渡せばその音源。ドラッグ中の値)で 1 音を鳴らし、
/// いちばん大きくなった所から `times` 秒後の、鍵盤の高さの倍数ごとの強さ(`step` 刻み、`max_mult` 倍まで)を返す。
/// トラックのエフェクトは通さない(音源そのものの音)
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn render_note_harmonics(
    state: State<'_, AppState>,
    track_id: String,
    device: Option<Value>,
    pitch: u8,
    vel: Option<u8>,
    times: Vec<f64>,
    step: Option<f64>,
    max_mult: Option<f64>,
) -> Result<Vec<Vec<f32>>, String> {
    let track_id = glaux_core::TrackId::parse(&track_id).map_err(|e| e.to_string())?;
    let (shared, _) = state.handle.get_project_shared().await?;
    let mut project = (*shared).clone();
    let device: Option<glaux_core::Device> = device
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| e.to_string())?;
    {
        let t = project
            .tracks
            .iter_mut()
            .find(|t| t.id == track_id)
            .ok_or_else(|| format!("track not found: {track_id}"))?;
        if let Some(d) = device {
            t.device = Some(d);
        }
        t.effects.clear();
        t.fx_links = None;
    }
    let dir = state.project_dir();
    let pitch = pitch.min(127);
    tauri::async_runtime::spawn_blocking(move || {
        const RATE: f64 = 48_000.0;
        let dir = std::path::Path::new(&dir);
        let bank = glaux_engine::SampleBank::for_offline(&project, dir);
        let longest = times.iter().cloned().fold(0.0f64, f64::max);
        let x = glaux_engine::render_track_note(
            &project,
            &track_id,
            pitch,
            vel.unwrap_or(100),
            longest + 1.0,
            RATE,
            &bank,
        )
        .map_err(|e| e.to_string())?;
        let f0 = 440.0 * 2f64.powf((pitch as f64 - 69.0) / 12.0);
        Ok(glaux_engine::note_harmonics(
            &x,
            RATE,
            f0,
            &times,
            step.unwrap_or(0.5),
            max_mult.unwrap_or(40.0),
        ))
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---- 音色エディタのウェーブテーブル --------------------------------------
// 作り方の手順(元・手で編集した波形・なじませる幅・加工)から、絵の材料・試聴の仮のテーブル・確定のテーブルを作る

/// 手順を組み立てる場所(曲・フォルダ・トラック)。手順は重いので別のスレッドで
async fn wt_run<T: Send + 'static>(
    state: &AppState,
    track_id: &str,
    f: impl FnOnce(&glaux_mcp::wavetables::Ctx, &std::path::Path) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let tid = glaux_core::TrackId::parse(track_id).map_err(|e| e.to_string())?;
    let (project, _) = state.handle.get_project_shared().await?;
    let dir = state.project_dir();
    tokio::task::spawn_blocking(move || {
        let dir = std::path::Path::new(&dir);
        let ctx = glaux_mcp::wavetables::Ctx {
            project: &project,
            project_dir: dir,
            track: project.track(&tid),
        };
        f(&ctx, dir)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 絵の材料: 重ねた絵と、選んだ 1 枚の加工の前・後の波形と倍音
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn wavetable_view(
    state: State<'_, AppState>,
    track_id: String,
    recipe: Value,
    pos_out: f32,
    pos_src: f32,
    stack: Option<usize>,
    stack_points: Option<usize>,
    points: Option<usize>,
    harm: Option<usize>,
) -> Result<Value, String> {
    wt_run(&state, &track_id, move |ctx, _| {
        glaux_mcp::wavetables::view(
            &recipe,
            ctx,
            pos_out,
            pos_src,
            stack.unwrap_or(40),
            stack_points.unwrap_or(128),
            points.unwrap_or(512),
            harm.unwrap_or(256),
        )
    })
    .await
}

/// ドラッグ中の試聴: 手順からテーブルを作って cache/ に書き、試聴に当てる編集(素材の登録 + table)を返す
#[tauri::command]
async fn wavetable_preview(
    state: State<'_, AppState>,
    track_id: String,
    recipe: Value,
) -> Result<Value, String> {
    let tid = track_id.clone();
    wt_run(&state, &track_id, move |ctx, dir| {
        let cycles = glaux_mcp::wavetables::build(&recipe, ctx)?;
        let (id, asset) = glaux_mcp::wavetables::write_preview(dir, &cycles)?;
        let mut cmds = vec![];
        if !ctx.project.assets.contains_key(&id) {
            cmds.push(json!({ "op": "add_asset", "id": id, "asset": asset }));
        }
        cmds.push(json!({ "op": "set_param", "track": tid, "path": "device/table", "value": id }));
        Ok(json!(cmds))
    })
    .await
}

/// 確定: 手順からテーブルを作って audio/ に書き(手順は隣の .recipe.json)、table を差し替える(+ ほかのつまみ。1 件の編集)
#[tauri::command]
async fn wavetable_commit(
    state: State<'_, AppState>,
    track_id: String,
    recipe: Value,
    label: String,
    extra: Option<Vec<Value>>,
) -> Result<Value, String> {
    let tid = glaux_core::TrackId::parse(&track_id).map_err(|e| e.to_string())?;
    let r2 = recipe.clone();
    let (imported, has) = wt_run(&state, &track_id, move |ctx, dir| {
        let cycles = glaux_mcp::wavetables::build(&r2, ctx)?;
        let imported = glaux_mcp::wavetables::write_asset(dir, &cycles, Some(&r2))?;
        let has = ctx.project.assets.contains_key(&imported.id);
        Ok((imported, has))
    })
    .await?;
    let mut cmds = Vec::new();
    if !has {
        cmds.push(Command::AddAsset {
            id: imported.id.clone(),
            asset: imported.asset.clone(),
        });
    }
    cmds.push(Command::SetParam {
        track: tid,
        path: glaux_core::ParamPath::parse("device/table").map_err(|e| e.to_string())?,
        value: glaux_core::ParamValue::Enum(imported.id.to_string()),
    });
    for (i, v) in extra.unwrap_or_default().into_iter().enumerate() {
        cmds.push(serde_json::from_value(v).map_err(|e| {
            glaux_core::tr!(
                "extra[{i}] を Command として解釈できません: {e}",
                "Can't parse extra[{i}] as a Command: {e}"
            )
        })?);
    }
    let (entry, m) = state
        .handle
        .apply(Command::batch(label.clone(), cmds), Author::Human, label)
        .await?
        .map_err(|e| e.to_string())?;
    Ok(json!({ "entry_id": entry, "asset_id": imported.id, "project_version": m.project_version }))
}

/// 素材のテーブルの作り方の手順(残っていれば)
#[tauri::command]
async fn wavetable_recipe(state: State<'_, AppState>, asset_id: String) -> Result<Value, String> {
    let id = glaux_core::AssetId::parse(&asset_id).map_err(|e| e.to_string())?;
    let (project, _) = state.handle.get_project_shared().await?;
    let dir = state.project_dir();
    Ok(
        glaux_mcp::wavetables::asset_recipe(&project, std::path::Path::new(&dir), &id)
            .unwrap_or(Value::Null),
    )
}

/// 左の列の小さな絵: 元ごとの位置 0・0.5・1 の 3 周期(`points` 点に間引く)
#[tauri::command]
async fn wavetable_thumbs(
    state: State<'_, AppState>,
    track_id: String,
    sources: Vec<Value>,
    points: Option<usize>,
) -> Result<Value, String> {
    let n = points.unwrap_or(64).clamp(8, 512);
    wt_run(&state, &track_id, move |ctx, _| {
        let out: Vec<Value> = sources
            .iter()
            .map(
                |src| match glaux_mcp::wavetables::resolve_source(src, 3, ctx) {
                    Ok(c) => json!((0..3)
                        .map(|k| {
                            let f = glaux_dsp::wtedit::frame_count(&c).max(1);
                            let i = (k * (f - 1) / 2).min(f - 1);
                            let cyc = &c
                                [i * glaux_dsp::wtedit::CYCLE..(i + 1) * glaux_dsp::wtedit::CYCLE];
                            let peak = cyc.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
                            (0..n)
                                .map(|j| cyc[j * glaux_dsp::wtedit::CYCLE / n] / peak)
                                .collect::<Vec<f32>>()
                        })
                        .collect::<Vec<_>>()),
                    Err(_) => Value::Null,
                },
            )
            .collect();
        Ok(json!(out))
    })
    .await
}

/// 保存した波形(全部の曲で使える棚)の一覧
#[tauri::command]
async fn wavetable_library() -> Result<Value, String> {
    off_thread(|| json!(glaux_mcp::wavetables::list_library())).await
}

/// 棚に保存する(作り方の手順も一緒に)
#[tauri::command]
async fn wavetable_library_save(
    state: State<'_, AppState>,
    track_id: String,
    name: String,
    recipe: Value,
) -> Result<Value, String> {
    wt_run(&state, &track_id, move |ctx, _| {
        let cycles = glaux_mcp::wavetables::build(&recipe, ctx)?;
        glaux_mcp::wavetables::save_library(&name, &cycles, "", Some(recipe))?;
        Ok(json!({ "saved": name }))
    })
    .await
}

/// WAV に書き出す(1 周期 2048 点・Serum などで読める)
#[tauri::command]
async fn wavetable_export(
    state: State<'_, AppState>,
    track_id: String,
    recipe: Value,
    path: String,
) -> Result<Value, String> {
    wt_run(&state, &track_id, move |ctx, _| {
        let cycles = glaux_mcp::wavetables::build(&recipe, ctx)?;
        glaux_mcp::wavetables::export_wav(&cycles, std::path::Path::new(&path))?;
        Ok(json!({ "path": path }))
    })
    .await
}

// ---- チャット(UI → AI 指示) --------------------------------------------

/// 前回のターン以降に人間が行った編集をまとめた、AI 向けのコンテキスト文を作る。
/// あわせて「AI に見せた最新エントリ」を更新する。注入するものが無ければ None。
async fn build_chat_context(state: &AppState) -> Option<String> {
    let since_raw = state.chat.last_seen_entry();
    let since = since_raw.as_deref().and_then(|s| EntryId::parse(s).ok());

    // 初回(記録なし)はコンテキスト不要。現在位置だけ記録する
    let Some(since) = since else {
        if let Ok(Ok(page)) = state.handle.get_history(None, None, Some(1)).await {
            state
                .chat
                .set_last_seen_entry(page.entries.last().map(|e| e.id.to_string()));
        }
        return None;
    };

    let mut rolled_back = false;
    let page = match state
        .handle
        .get_history(None, Some(since.clone()), None)
        .await
    {
        Ok(Ok(r)) => r,
        // since のエントリが undo / revert_to で消えている
        Ok(Err(_)) => {
            rolled_back = true;
            match state.handle.get_history(None, None, Some(5)).await {
                Ok(Ok(r)) => r,
                _ => return None,
            }
        }
        Err(_) => return None,
    };
    let (entries, version) = (page.entries, page.project_version);

    match entries.last() {
        Some(last) => state.chat.set_last_seen_entry(Some(last.id.to_string())),
        None if rolled_back => state.chat.set_last_seen_entry(None),
        None => {}
    }

    let human_labels: Vec<String> = entries
        .iter()
        .filter(|e| matches!(e.author, Author::Human))
        .map(|e| format!("- {}", e.label))
        .collect();
    // 人間の編集の中身の要約(どのクリップのノートが何個増えた・変わった等)
    let detail: Vec<String> = if rolled_back || human_labels.is_empty() {
        vec![]
    } else {
        match (
            state.handle.get_entries(Some(since.clone()), 1000).await,
            state.handle.get_project_shared().await,
        ) {
            (Ok(Ok(full)), Ok((project, _))) => {
                let human: Vec<&glaux_core::HistoryEntry> = full
                    .iter()
                    .filter(|e| matches!(e.author, Author::Human))
                    .collect();
                let summary = glaux_mcp::changes::summarize(&human, &project);
                glaux_mcp::changes::summary_lines(&summary, 8)
            }
            _ => vec![],
        }
    };

    if human_labels.is_empty() && !rolled_back {
        return None;
    }

    let mut ctx = String::from("【システム情報(アプリが自動付与)】\n");
    if rolled_back {
        ctx.push_str("履歴が巻き戻されています(undo などが実行された)。会話の記憶は当てにせず、get_project で状態を確認してください。\n");
    }
    if !human_labels.is_empty() {
        ctx.push_str("前回のターン以降に、人間(ユーザー)が UI から以下の編集を行いました:\n");
        // 多すぎる場合は新しい側を優先
        let skip = human_labels.len().saturating_sub(10);
        if skip > 0 {
            ctx.push_str(&format!("(古い {skip} 件を省略)\n"));
        }
        for label in &human_labels[skip..] {
            ctx.push_str(label);
            ctx.push('\n');
        }
    }
    if !detail.is_empty() {
        ctx.push_str("変更の中身(詳しくは get_changes / get_project の clip_ids で):\n");
        for line in &detail {
            ctx.push_str(line);
            ctx.push('\n');
        }
    }
    ctx.push_str(&format!(
        "現在の project_version: {version}。必要に応じて get_project で最新状態を確認してから作業してください。\n"
    ));
    Some(ctx)
}

/// 指示を 1 つ実行する。進捗は Tauri イベント `chat-event` で届くので即座に返る。
#[tauri::command]
async fn send_chat(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    prompt: String,
    model: Option<String>,
    provider: Option<String>,
    effort: Option<String>,
    language: Option<String>,
) -> Result<(), String> {
    let provider = chat::Provider::parse(provider.as_deref())?;
    let prompt = prompt.trim().to_owned();
    if prompt.is_empty() {
        return Err(t("指示が空です", "The instruction is empty").to_owned());
    }
    if state.chat.is_running() {
        return Err(t(
            "前の指示がまだ実行中です",
            "The previous instruction is still running",
        )
        .to_owned());
    }
    state.chat.set_model(model)?;
    state.chat.set_effort(effort)?;
    state.chat.set_provider(provider);
    state
        .chat
        .set_language(chat::ReplyLang::parse(language.as_deref()));
    let full_prompt = match build_chat_context(&state).await {
        Some(ctx) => format!("{ctx}\n{prompt}"),
        None => prompt,
    };
    let mgr = state.chat.clone();
    tauri::async_runtime::spawn(chat::run_turn(app, mgr, full_prompt));
    Ok(())
}

/// (Windows ではプロセスツリーを止める taskkill の終わりを待つので、別のスレッドで)
#[tauri::command]
async fn cancel_chat(state: State<'_, AppState>) -> Result<(), String> {
    let chat = state.chat.clone();
    off_thread(move || chat.cancel()).await
}

/// 会話をリセットする(次の送信が新しいセッションになる)。
#[tauri::command]
fn reset_chat(state: State<'_, AppState>) {
    state.chat.reset();
}

/// 画面の会話ログを読む(再起動・プロジェクトの切り替えのあとに表示を戻す)。
#[derive(serde::Serialize)]
struct ChatLog {
    /// 読んだプロジェクトのフォルダ(保存のときにそのまま返してもらう)
    dir: String,
    log: Option<String>,
}

#[tauri::command]
async fn load_chat_log(state: State<'_, AppState>) -> Result<ChatLog, String> {
    let chat = state.chat.clone();
    off_thread(move || {
        let (dir, log) = chat.load_log();
        ChatLog { dir, log }
    })
    .await
}

/// 画面の会話ログを保存する。読んだときとプロジェクトが変わっていたら保存しない(false)。
/// (会話が長いと大きくなるので、別のスレッドで書く)
#[tauri::command]
async fn save_chat_log(
    state: State<'_, AppState>,
    dir: String,
    log: String,
) -> Result<bool, String> {
    let chat = state.chat.clone();
    off_thread(move || chat.save_log(&dir, &log)).await
}

// ---- 起動 -----------------------------------------------------------------

fn resolve_project_dir() -> String {
    if let Some(arg) = std::env::args().nth(1) {
        return arg;
    }
    if let Ok(env) = std::env::var("GLAUX_PROJECT") {
        if !env.is_empty() {
            return env;
        }
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_owned());
    format!("{home}/Music/GlauxDemo.glaux")
}

/// タスクバーのアイコン(大きさ = 24px × 表示倍率)。`scripts/make_icons.py` が作る RGBA の生データ。
/// Tauri の既定ではウィンドウのアイコンに ICO の 1 枚だけが使われ、Windows がタスクバーの大きさへ
/// 拡大・縮小してぼやけるので、表示倍率に合う大きさの画像をウィンドウに設定する。
const TASKBAR_ICONS: [(u32, &[u8]); 7] = [
    (24, include_bytes!("../icons/taskbar/taskbar-24.rgba")),
    (30, include_bytes!("../icons/taskbar/taskbar-30.rgba")),
    (36, include_bytes!("../icons/taskbar/taskbar-36.rgba")),
    (48, include_bytes!("../icons/taskbar/taskbar-48.rgba")),
    (60, include_bytes!("../icons/taskbar/taskbar-60.rgba")),
    (72, include_bytes!("../icons/taskbar/taskbar-72.rgba")),
    (96, include_bytes!("../icons/taskbar/taskbar-96.rgba")),
];

/// 表示倍率 `scale`(1.0 = 100%)のタスクバーに合う大きさ(24px × 倍率以上で最小のもの)。
fn taskbar_icon_size(scale: f64) -> (u32, &'static [u8]) {
    let want = (24.0 * scale).round() as u32;
    TASKBAR_ICONS
        .iter()
        .find(|(s, _)| *s >= want)
        .copied()
        .unwrap_or(TASKBAR_ICONS[TASKBAR_ICONS.len() - 1])
}

fn taskbar_icon(scale: f64) -> tauri::image::Image<'static> {
    let (size, rgba) = taskbar_icon_size(scale);
    tauri::image::Image::new(rgba, size, size)
}

fn mcp_port() -> u16 {
    std::env::var("GLAUX_MCP_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_MCP_PORT)
}

/// 窓口のポートを開く。使われていたら(Glaux を複数起動したとき)次のポートを順に試す。
/// 開けたポートをチャットと画面の表示に使うので、後から起動した Glaux の AI が先の Glaux の曲を編集することはない
fn bind_mcp(preferred: u16) -> Option<std::net::TcpListener> {
    (0..MCP_PORT_TRIES)
        .filter_map(|i| preferred.checked_add(i))
        .find_map(|p| std::net::TcpListener::bind(("127.0.0.1", p)).ok())
}

/// アプリ内 MCP サーバー(streamable HTTP)。UI と同じ SessionHandle を共有する。
async fn serve_mcp(
    handle: SessionHandle,
    listener: std::net::TcpListener,
    chat_model: glaux_mcp::server::ChatModel,
) -> Result<()> {
    let service: StreamableHttpService<GlauxServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(GlauxServer::new(handle.clone()).with_chat_model(chat_model.clone())),
            Default::default(),
            Default::default(),
        );
    let router = axum::Router::new().nest_service("/mcp", service);
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let listener = tokio::net::TcpListener::from_std(listener)?;
    tracing::info!("MCP サーバー: http://127.0.0.1:{port}/mcp");
    axum::serve(listener, router).await?;
    Ok(())
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("GLAUX_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let mut project_dir = resolve_project_dir();
    let mut notices = Vec::new();
    let (store, session) = match Store::open_or_create(&project_dir) {
        Ok(x) => x,
        // 別の Glaux が開いている曲なら、終了せずに一時的な空の曲で起動する(ユーザーが曲を選び直す)
        Err(e) if glaux_mcp::store::ProjectLocked::is(&e) => {
            tracing::warn!("{e:#}");
            let temp = glaux_mcp::store::unique_project_dir(&std::env::temp_dir(), "Glaux_temp");
            let opened = Store::open_or_create(&temp).context("一時的な曲を作れません")?;
            let name = std::path::Path::new(&project_dir)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| project_dir.clone());
            notices.push((
                format!(
                    "「{name}」は別の Glaux で開かれているので、一時的な空の曲で起動しました。上の曲名のメニューから、別の曲を開くか新しく作ってください"
                ),
                format!(
                    "\"{name}\" is open in another Glaux, so a temporary empty song was opened. Open another song or create a new one from the song title menu above"
                ),
            ));
            project_dir = temp.to_string_lossy().into_owned();
            opened
        }
        Err(e) => return Err(e.context("プロジェクトを開けません")),
    };
    let temp_project = !notices.is_empty();
    let preferred_port = mcp_port();
    let mcp_listener = bind_mcp(preferred_port);
    let port = match &mcp_listener {
        Some(l) => l.local_addr().map(|a| a.port()).unwrap_or(preferred_port),
        None => {
            tracing::error!(
                "MCP ポート {preferred_port}〜 がすべて使われています。AI とつなぐ窓口を開けません"
            );
            notices.push((
                "AI とつなぐ窓口(MCP)を開けませんでした。Glaux をいくつも起動していないか確かめてください".to_owned(),
                "Couldn't open the AI connection (MCP). Check that Glaux isn't running multiple times".to_owned(),
            ));
            // 別の Glaux の窓口につながないよう、つながらない URL にしておく
            0
        }
    };
    if port != preferred_port && port != 0 {
        notices.push((
            format!(
                "ほかの Glaux が起動しているため、AI とつなぐ窓口(MCP)は http://127.0.0.1:{port}/mcp です(アプリ内のチャットはそのまま使えます)"
            ),
            format!(
                "Another Glaux is running, so the AI connection (MCP) is http://127.0.0.1:{port}/mcp (the in-app chat works as usual)"
            ),
        ));
    }
    tracing::info!(
        "プロジェクトを開きました: {}(履歴 {} エントリ)",
        store.dir().display(),
        session.history().len()
    );
    let project_title = session.project().meta.title.clone();
    if !temp_project {
        projects::push_recent(&project_dir, &project_title);
    }
    // 出荷時プリセット(ギター系など)を初回のみ導入
    glaux_mcp::presets::ensure_factory(&glaux_mcp::presets::default_dir());
    let handle = SessionHandle::spawn(session, store);

    let engine = match glaux_engine::start_engine() {
        Ok(e) => Some(e),
        Err(err) => {
            tracing::warn!("オーディオエンジンを起動できません({err})。再生なしで続行します");
            None
        }
    };

    let mcp_url = format!("http://127.0.0.1:{port}/mcp");
    let state = AppState {
        handle: handle.clone(),
        project_dir: std::sync::Mutex::new(project_dir.clone()),
        mcp_url: mcp_url.clone(),
        startup_notice: (!notices.is_empty()).then(|| {
            let (ja, en): (Vec<String>, Vec<String>) = notices.into_iter().unzip();
            (ja.join("\n"), en.join("\n"))
        }),
        chat: Arc::new(ChatManager::new(mcp_url, project_dir.clone())),
        engine: engine.clone(),
        calib: std::sync::Mutex::new(None),
    };
    // チャットの AI のモデル名(履歴の作者名)を MCP サーバーと共有する
    let chat_model = state.chat.chat_model.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .setup(move |app| {
            set_window_title(app.handle(), &project_title);
            // タスクバーのアイコンを表示倍率に合う大きさに(ぼやけないように)
            if let Some(w) = tauri::Manager::get_webview_window(app, "main") {
                if let Ok(scale) = w.scale_factor() {
                    let _ = w.set_icon(taskbar_icon(scale));
                }
            }

            // アプリ内 MCP サーバー
            let mcp_handle = handle.clone();
            let mcp_chat_model = chat_model.clone();
            tauri::async_runtime::spawn(async move {
                let Some(listener) = mcp_listener else {
                    return;
                };
                if let Err(e) = serve_mcp(mcp_handle, listener, mcp_chat_model).await {
                    tracing::error!("MCP サーバーが停止しました: {e:#}");
                }
            });

            // 変更通知 → フロントエンドイベント
            let app_handle = app.handle().clone();
            let mut rx = handle.subscribe();
            tauri::async_runtime::spawn(async move {
                loop {
                    use tokio::sync::broadcast::error::RecvError;
                    match rx.recv().await {
                        Ok(ev) => {
                            let _ = app_handle.emit("project-changed", &ev);
                        }
                        // 取りこぼしても UI はイベントごとに全取得するので続行でよい
                        Err(RecvError::Lagged(_)) => continue,
                        Err(RecvError::Closed) => break,
                    }
                }
            });

            // CLAP プラグインの画面での操作をプロジェクトに保存する(状態が変わった・画面を閉じた)。
            // つまみを回している間に履歴が細かく増えすぎないよう、1.5 秒ごとにまとめて保存する
            if engine.is_some() {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                        use tauri::Manager;
                        let st = app_handle.state::<AppState>();
                        let Some(engine) = st.engine.clone() else {
                            break;
                        };
                        let mut owners: Vec<glaux_engine::plugins::PluginOwner> = Vec::new();
                        for (owner, _) in engine.take_plugin_events() {
                            if !owners.contains(&owner) {
                                owners.push(owner);
                            }
                        }
                        for owner in owners {
                            if let Err(e) = save_clap_state(&st, owner).await {
                                tracing::warn!("プラグインの状態を保存できません: {e}");
                            }
                        }
                    }
                });
            }

            // プロジェクトの変更をエンジンの再生データに反映する。
            // AI の連続編集で毎回全再構築しないよう、短い静穏時間でイベントを合流させる
            if let Some(engine) = engine.clone() {
                let session = handle.clone();
                tauri::async_runtime::spawn(async move {
                    // プロジェクト移動・切り替えに追従するため dir は毎回引く
                    let sync = |project: Arc<glaux_core::Project>, dir: String| {
                        let engine = engine.clone();
                        async move {
                            tauri::async_runtime::spawn_blocking(move || {
                                engine.set_project(&project, std::path::Path::new(&dir));
                            })
                            .await
                            .ok();
                        }
                    };
                    if let (Ok((project, _)), Ok(dir)) = (
                        session.get_project_shared().await,
                        session.project_dir().await,
                    ) {
                        sync(project, dir).await;
                    }
                    let mut rx = session.subscribe();
                    loop {
                        use tokio::sync::broadcast::error::{RecvError, TryRecvError};
                        match rx.recv().await {
                            Ok(first) => {
                                // 40ms 待って、その間に来たイベントをまとめる。曲の中身が変わらない
                                // (チェックポイントだけの)ときは作り直さない
                                let mut changed = !first.history_only;
                                tokio::time::sleep(std::time::Duration::from_millis(40)).await;
                                loop {
                                    match rx.try_recv() {
                                        Ok(ev) => changed |= !ev.history_only,
                                        Err(TryRecvError::Lagged(_)) => changed = true,
                                        Err(_) => break,
                                    }
                                }
                                if !changed {
                                    continue;
                                }
                                if let (Ok((project, _)), Ok(dir)) = (
                                    session.get_project_shared().await,
                                    session.project_dir().await,
                                ) {
                                    sync(project, dir).await;
                                }
                            }
                            Err(RecvError::Lagged(_)) => {
                                tokio::time::sleep(std::time::Duration::from_millis(40)).await;
                                while matches!(rx.try_recv(), Ok(_) | Err(TryRecvError::Lagged(_)))
                                {
                                }
                                if let (Ok((project, _)), Ok(dir)) = (
                                    session.get_project_shared().await,
                                    session.project_dir().await,
                                ) {
                                    sync(project, dir).await;
                                }
                            }
                            Err(RecvError::Closed) => break,
                        }
                    }
                });
            }

            // AI から人への質問(ask_user)→ フロントエンドイベント(チャットの質問のカード)
            let app_handle = app.handle().clone();
            let mut question_rx = handle.subscribe_questions();
            tauri::async_runtime::spawn(async move {
                loop {
                    use tokio::sync::broadcast::error::RecvError;
                    match question_rx.recv().await {
                        Ok(q) => {
                            let _ = app_handle.emit("ai-question", &q);
                        }
                        Err(RecvError::Lagged(_)) => continue,
                        Err(RecvError::Closed) => break,
                    }
                }
            });

            // AI のツール呼び出し状況 → フロントエンドイベント(「AI 作業中」表示)
            let app_handle = app.handle().clone();
            let mut activity_rx = handle.subscribe_activity();
            tauri::async_runtime::spawn(async move {
                loop {
                    use tokio::sync::broadcast::error::RecvError;
                    match activity_rx.recv().await {
                        Ok(ev) => {
                            let _ = app_handle.emit("ai-activity", &ev);
                        }
                        Err(RecvError::Lagged(_)) => continue,
                        Err(RecvError::Closed) => break,
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_project,
            get_tracks,
            get_history,
            undo,
            redo,
            app_info,
            list_recent_projects,
            find_projects,
            set_projects_dir,
            open_project,
            move_project,
            list_presets,
            list_fx_presets,
            save_fx_preset,
            apply_fx_preset,
            delete_fx_preset,
            save_preset,
            load_preset,
            get_track_params,
            import_sample,
            import_ir,
            list_soundfonts,
            download_sfz_pack,
            list_soundfont_presets,
            add_soundfont,
            create_project,
            preview_project_dir,
            apply_edit,
            duplicate_track,
            preview_edit,
            revert_entry,
            turn_changes,
            revert_turn,
            chat_rewind,
            chat_rewind_preview,
            bounce_track,
            import_midi,
            export_midi,
            harmony,
            get_design,
            clip_states,
            plan_save,
            plan_delete,
            plan_step,
            plan_revert,
            plan_restore,
            plan_settle_estimated,
            ab_prepare_proposals,
            plan_adopt_proposal,
            design_edit_sections,
            set_ui_language,
            plan_head,
            plan_estimate,
            change_ranges,
            export_audio,
            import_audio_clip,
            clip_peaks,
            transcribe_clip,
            swing_clip,
            detect_clip_tempo,
            model_status,
            match_clip_sound,
            find_similar_clap_presets,
            refine_clap_params,
            download_clap_model,
            setup_status,
            download_soundfont,
            open_demo_song,
            record_start,
            record_stop,
            midi_inputs,
            separate_clip,
            clap_plugins,
            clap_save_state,
            clap_open_gui,
            clap_close_gui,
            clap_presets,
            clap_load_preset,
            set_midi_input,
            set_live_target,
            live_note_on,
            live_note_off,
            sampler_auto_cuts,
            sfz_inspect,
            sfz_key_wave,
            wavetable_view,
            wavetable_preview,
            wavetable_commit,
            wavetable_recipe,
            wavetable_thumbs,
            wavetable_library,
            wavetable_library_save,
            wavetable_export,
            asset_peaks,
            live_all_off,
            render_note_harmonics,
            midi_record_start,
            midi_record_stop,
            audio_devices,
            get_master_params,
            get_effect_catalog,
            set_output_device,
            set_buffer_size,
            set_input_device,
            input_monitor,
            calibrate_start,
            calibrate_stop,
            transport_set_metronome,
            transport_set_monitor,
            transport_scope,
            transport_spectrum,
            transport_set_speaker,
            transport_set_output_volume,
            transport_reset_loudness,
            ab_prepare,
            ab_set_side,
            ab_clear,
            send_chat,
            cancel_chat,
            reset_chat,
            load_chat_log,
            save_chat_log,
            transport_state,
            transport_set_loop,
            transport_clear_loop,
            transport_play,
            transport_pause,
            transport_stop,
            transport_seek,
            preview_note
        ])
        .on_window_event(|window, event| {
            // 表示倍率の違うモニターへ移ったら、タスクバーのアイコンをその倍率の大きさに替える
            if let tauri::WindowEvent::ScaleFactorChanged { scale_factor, .. } = event {
                let _ = window.set_icon(taskbar_icon(*scale_factor));
            }
        })
        .build(tauri::generate_context!())
        .context("Tauri の起動に失敗")?
        .run(|app, event| {
            // 終わる前に、裏で書きかけの project.json を書き終える
            if let tauri::RunEvent::Exit = event {
                use tauri::Manager;
                if let Some(st) = app.try_state::<AppState>() {
                    let handle = st.handle.clone();
                    if let Err(e) = tauri::async_runtime::block_on(handle.flush()) {
                        tracing::error!("終了時の保存に失敗しました: {e}");
                    }
                }
            }
        });
    Ok(())
}

#[cfg(test)]
mod plugin_state_tests {
    use super::*;

    #[test]
    fn typed_strip_matches_the_json_strip() {
        use glaux_core::{Device, Effect, FxId, PluginSource, Project, Track, TrackId, TrackKind};
        let clap = |state: &str| PluginSource::Clap {
            plugin_id: "org.example.synth".into(),
            state: Some(state.into()),
        };
        let mut p = Project::new("t");
        let mut t = Track::new(TrackId::new(), "A", TrackKind::Midi);
        t.device = Some(Device {
            source: clap("AAAA"),
            params: Default::default(),
        });
        let mut fx = Effect::builtin(FxId::new(), "eq");
        fx.source = clap("BBBB");
        t.effects.push(fx.clone());
        t.effects.push(Effect::builtin(FxId::new(), "reverb"));
        p.tracks.push(t);
        p.master.effects.push(fx);
        // JSON の木から消したもの
        let mut want = serde_json::to_value(&p).unwrap();
        for t in want["tracks"].as_array_mut().unwrap() {
            strip_plugin_state(t);
        }
        strip_plugin_state(&mut want["master"]);
        // 型の上で消してから直列化したもの
        let mut c = p.clone();
        for t in &mut c.tracks {
            strip_plugin_state_typed(t.device.as_mut(), &mut t.effects);
        }
        strip_plugin_state_typed(None, &mut c.master.effects);
        assert_eq!(serde_json::to_value(&c).unwrap(), want);
        assert!(!serde_json::to_string(&c).unwrap().contains("AAAA"));
    }
}

#[cfg(test)]
mod taskbar_icon_tests {
    use super::*;

    #[test]
    fn taskbar_icons_match_their_sizes_and_follow_the_display_scale() {
        for (size, rgba) in TASKBAR_ICONS {
            assert_eq!(
                rgba.len(),
                (size * size * 4) as usize,
                "{size}px の生データの長さ"
            );
        }
        // 100% / 125% / 150% / 200% / 300% はちょうどの大きさ、半端な倍率は一つ大きいもの
        assert_eq!(taskbar_icon_size(1.0).0, 24);
        assert_eq!(taskbar_icon_size(1.25).0, 30);
        assert_eq!(taskbar_icon_size(1.5).0, 36);
        assert_eq!(taskbar_icon_size(1.75).0, 48);
        assert_eq!(taskbar_icon_size(2.0).0, 48);
        assert_eq!(taskbar_icon_size(3.0).0, 72);
        assert_eq!(taskbar_icon_size(8.0).0, 96, "大きすぎる倍率は最大のもの");
    }

    #[test]
    fn demo_song_loads_and_validates() {
        // 同梱のデモ曲が今の形式で読め、検証で問題が出ず、外部のファイル(SoundFont・音声・CLAP)を使わない
        let p: glaux_core::Project =
            serde_json::from_str(include_str!("../demo/CyberNeon.json")).unwrap();
        assert!(p.validate().is_empty(), "{:?}", p.validate());
        assert!(p.assets.is_empty());
        assert!(p.tracks.len() >= 5);
        assert!(p.tracks.iter().all(|t| t
            .device
            .as_ref()
            .is_none_or(|d| matches!(d.source, glaux_core::PluginSource::Builtin { .. }))));
    }
}

#[cfg(test)]
mod mcp_port_tests {
    use super::*;

    /// 窓口のポートが使われていたら、次の空いているポートを開く(Glaux を複数起動したとき)
    #[test]
    fn a_taken_port_moves_to_the_next_free_one() {
        let first = bind_mcp(47_310).expect("開ける");
        let taken = first.local_addr().unwrap().port();
        let second = bind_mcp(taken).expect("次を開ける");
        let next = second.local_addr().unwrap().port();
        assert_ne!(next, taken);
        assert!(next > taken && next < taken + MCP_PORT_TRIES);
    }
}
