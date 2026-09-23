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

struct AppState {
    handle: SessionHandle,
    /// 現在のプロジェクトフォルダ(プロジェクト切り替えで変わる)
    project_dir: std::sync::Mutex<String>,
    mcp_url: String,
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
        self.engine
            .as_ref()
            .ok_or_else(|| "オーディオデバイスが利用できません".to_owned())
    }
}

// ---- Tauri コマンド(UI からの読み取り・操作) ---------------------------

#[tauri::command]
async fn get_project(state: State<'_, AppState>) -> Result<Value, String> {
    let (project, version) = state.handle.get_project().await?;
    Ok(json!({ "project_version": version, "project": project }))
}

#[tauri::command]
async fn get_history(state: State<'_, AppState>) -> Result<Value, String> {
    let (entries, version) = state
        .handle
        .get_history(None, None, None)
        .await?
        .map_err(|e| e.to_string())?;
    Ok(json!({ "project_version": version, "entries": entries }))
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
    json!({ "project_dir": state.project_dir(), "mcp_url": state.mcp_url })
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

/// 既定の作業フォルダ(新規プロジェクトの作成先)を変更する。
#[tauri::command]
fn set_projects_dir(path: String) -> Result<Value, String> {
    projects::set_projects_dir(&path)?;
    Ok(json!({ "default_dir": path }))
}

/// WAV をプロジェクトに取り込み、トラックの音源を sampler にする(音作りビュー用)。
#[tauri::command]
async fn import_sample(
    state: State<'_, AppState>,
    track_id: String,
    path: String,
) -> Result<Value, String> {
    let tid = glaux_core::TrackId::parse(&track_id).map_err(|e| e.to_string())?;
    let (project, _) = state.handle.get_project().await?;
    let track = project
        .track(&tid)
        .ok_or_else(|| format!("トラックが見つかりません: {track_id}"))?;
    let dir = state.project_dir();
    let imported =
        glaux_mcp::assets::import_audio(std::path::Path::new(&dir), std::path::Path::new(&path))?;

    let mut cmds = Vec::new();
    if !project.assets.contains_key(&imported.id) {
        cmds.push(Command::AddAsset {
            id: imported.id.clone(),
            asset: imported.asset.clone(),
        });
    }
    cmds.push(Command::SetDevice {
        track: tid,
        device: Some(glaux_core::Device {
            source: glaux_core::PluginSource::Sampler {
                asset: imported.id.clone(),
            },
            params: glaux_core::ParamMap::new(),
        }),
    });
    let file_name = std::path::Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "sample".to_owned());
    let label = format!("{} にサンプル「{file_name}」を設定", track.name);
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
    let (project, _) = state.handle.get_project().await?;
    let dir = state.project_dir();
    let imported =
        glaux_mcp::assets::import_audio(std::path::Path::new(&dir), std::path::Path::new(&path))?;
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
    let label = format!("{track_name} に音声クリップ「{name}」を配置");
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
    let (project, _) = state.handle.get_project().await?;
    let dir = state.project_dir();
    let buckets = buckets.clamp(1, 4096) as usize;
    let peaks = tokio::task::spawn_blocking(move || {
        glaux_mcp::transcribe::clip_peaks(&project, std::path::Path::new(&dir), &cid, buckets)
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(json!({ "peaks": peaks }))
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
    let (project, _) = state.handle.get_project().await?;
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
    let label = format!("音声クリップを譜起こし({} ノート)", t.note_count);
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
) -> Result<Value, String> {
    let engine = state.engine()?.clone();
    let dir = state.project_dir();
    let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let path = std::path::PathBuf::from(&dir).join(format!("audio/rec_{stamp}.wav"));
    // カウントインの長さ: 現在位置の拍子で bars 小節
    let (project, _) = state.handle.get_project().await?;
    let count_in =
        Tick(bar_ticks_at(&project, engine.playhead_tick()) * count_in_bars.unwrap_or(1) as u64);
    let clip_start = engine
        .start_recording(
            path,
            count_in,
            latency_ms.unwrap_or(0.0).clamp(0.0, 1000.0) / 1000.0,
            metronome.unwrap_or(true),
        )
        .map_err(|e| e.to_string())?;
    engine.play();
    engine.mark_play_started();
    Ok(json!({ "clip_start": clip_start, "count_in_ticks": count_in }))
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
    let (project, _) = state.handle.get_project().await?;
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
        return Err("カウントインより後に弾かれたノートがありませんでした".to_owned());
    }
    let (project, _) = state.handle.get_project().await?;
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
                track: glaux_core::Track::new(id.clone(), "MIDI 録音", glaux_core::TrackKind::Midi),
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
    let name = format!("MIDI 録音 {}", chrono::Local::now().format("%H:%M"));
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
    let label = format!("{name}(ノート {count} 個)を配置");
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
        return Err("カウントインより後に録音データがありません".to_owned());
    }
    if result.frames == 0 {
        let _ = std::fs::remove_file(&result.path);
        return Err("録音データが空でした(入力デバイスの設定を確認してください)".to_owned());
    }
    let dir = state.project_dir();
    let imported = glaux_mcp::assets::import_wav(std::path::Path::new(&dir), &result.path)?;
    // 一時ファイルはハッシュ名でコピー済みなので消す
    let _ = std::fs::remove_file(&result.path);

    let (project, _) = state.handle.get_project().await?;
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
                    track: glaux_core::Track::new(id.clone(), "録音", glaux_core::TrackKind::Audio),
                    index: None,
                });
                id
            }
        },
    };
    // 新設トラックはまだ project に無いので、仮に足したコピーでコマンドを組む
    let mut project_view = project.clone();
    if let Some(Command::AddTrack { track, .. }) = cmds.first() {
        project_view.tracks.push(track.clone());
    }
    let clip_id = glaux_core::ClipId::new();
    let name = format!("録音 {}", chrono::Local::now().format("%H:%M"));
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
        if let Ok(data) = glaux_engine::load_wav_mono(&wav) {
            let from = (offset as usize).min(data.frames.len());
            let peak = data.frames[from..]
                .iter()
                .fold(0.0f32, |m, v| m.max(v.abs()));
            if peak > 1e-4 {
                gain_db = (-6.0 - 20.0 * peak.log10()).clamp(0.0, 30.0);
            }
        }
        for c in &mut cmds {
            if let Command::AddClip { clip, .. } = c {
                if let glaux_core::ClipContent::Audio { gain_db: g, .. } = &mut clip.content {
                    *g = gain_db;
                }
            }
        }
    }
    let label = format!("{name}(録音)を配置");
    let (_, m) = state
        .handle
        .apply(Command::batch(label.clone(), cmds), Author::Human, label)
        .await?
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "clip_id": clip_id,
        "track_id": tid,
        "seconds": (result.frames - offset) as f64 / result.sample_rate as f64,
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
    json!({
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
    let (project, _) = state.handle.get_project().await?;
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
        return Err("録音中は較正できません".to_owned());
    }
    let (project, _) = state.handle.get_project().await?;
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
    let clip_start = match engine.start_recording(path, count_in, 0.0, true) {
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
        return Err("較正を開始していません".to_owned());
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
/// 追加できるエフェクトのカタログも返す。
#[tauri::command]
async fn get_track_params(state: State<'_, AppState>, track_id: String) -> Result<Value, String> {
    let tid = glaux_core::TrackId::parse(&track_id).map_err(|e| e.to_string())?;
    let (project, version) = state.handle.get_project().await?;
    let track = project
        .track(&tid)
        .ok_or_else(|| format!("トラックが見つかりません: {track_id}"))?;
    let mut v = glaux_mcp::server::track_params_json(track)?;
    v["project_version"] = json!(version);
    v["track_id"] = json!(track_id);
    v["available_effects"] =
        serde_json::to_value(glaux_dsp::effect_catalog()).unwrap_or(Value::Null);
    Ok(v)
}

/// 音作りビュー(マスター)用: マスターバスのエフェクトチェーン。
#[tauri::command]
async fn get_master_params(state: State<'_, AppState>) -> Result<Value, String> {
    let (project, version) = state.handle.get_project().await?;
    Ok(json!({
        "track_id": "__master__",
        "device": { "name": "master", "is_default_fallback": false },
        "params": [],
        "effects": glaux_mcp::server::effects_json(&project.master.effects),
        "available_effects": serde_json::to_value(glaux_dsp::effect_catalog()).unwrap_or(Value::Null),
        "project_version": version,
    }))
}

// ---- SoundFont ------------------------------------------------------------

#[tauri::command]
fn list_soundfonts() -> Value {
    let dir = glaux_engine::sf2::default_dir();
    json!({
        "dir": dir.to_string_lossy(),
        "files": glaux_engine::sf2::list_files(&dir),
    })
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
        .ok_or_else(|| "ファイル名が取れません".to_owned())?;
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

#[tauri::command]
fn list_presets() -> Value {
    json!({ "presets": glaux_mcp::presets::list(&glaux_mcp::presets::default_dir()) })
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
    let (project, _) = state.handle.get_project().await?;
    let track = project
        .track(&tid)
        .ok_or_else(|| format!("トラックが見つかりません: {track_id}"))?;
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
    let (project, _) = state.handle.get_project().await?;
    let track = project
        .track(&tid)
        .ok_or_else(|| format!("トラックが見つかりません: {track_id}"))?;
    let preset = glaux_mcp::presets::load(&glaux_mcp::presets::default_dir(), &name)?;
    let label = format!("{} にプリセット「{}」を適用", track.name, preset.name);
    let cmds = glaux_mcp::presets::apply_commands(track, &preset);
    let (_, m) = state
        .handle
        .apply(Command::batch(label.clone(), cmds), Author::Human, label)
        .await?
        .map_err(|e| e.to_string())?;
    Ok(json!({ "applied": preset.name, "project_version": m.project_version }))
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
        return Err("パスが空です".to_owned());
    }
    let p = std::path::Path::new(&path);
    if !create && !p.join("project.json").exists() {
        return Err(format!(
            "Glaux プロジェクトではありません(project.json が見つかりません): {path}"
        ));
    }

    if let Some(engine) = &state.engine {
        engine.stop();
        engine.clear_loop(); // ループ区間は前のプロジェクトの tick なので持ち越さない
    }
    let (title, version) = state.handle.switch_project(path.clone()).await?;
    state.chat.switch_project(path.clone());
    *state.project_dir.lock().expect("project_dir lock") = path.clone();
    projects::push_recent(&path, &title);
    set_window_title(&app, &title);
    Ok(json!({ "title": title, "project_version": version, "path": path }))
}

/// 現在のプロジェクトを移動 / 名前変更する。
/// `dest_parent` 省略で場所は今のまま、`new_name` 省略で名前は今のまま。
/// 名前を変えた場合はタイトル(meta.title)も追従させる(SetTitle コマンド、author: system)。
#[tauri::command]
async fn move_project(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    dest_parent: Option<String>,
    new_name: Option<String>,
) -> Result<Value, String> {
    if state.chat.is_running() {
        return Err("AI が作業中は移動できません。完了を待つか停止してください".to_owned());
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
        None => cur
            .parent()
            .map(|p| p.to_path_buf())
            .ok_or_else(|| "現在のプロジェクトの親フォルダが分かりません".to_owned())?,
    };
    let name = match new_name
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
    {
        Some(n) => {
            if n.contains(['/', '\\', ':']) {
                return Err("プロジェクト名に使えない文字が含まれています".to_owned());
            }
            n
        }
        None => cur_stem.clone(),
    };
    let dest = parent.join(format!("{name}.glaux"));
    let dest_str = dest.to_string_lossy().into_owned();
    if dest == cur {
        return Ok(json!({ "path": current, "moved": false }));
    }

    if let Some(engine) = &state.engine {
        engine.stop();
    }
    // 同一プロジェクトの移動なのでループ区間はそのまま有効
    let (mut title, mut version) = state.handle.move_project(dest_str.clone()).await?;

    // フォルダ名を変えたらタイトルも合わせる(履歴に載るので undo 可)
    if name != cur_stem && title != name {
        let cmd = glaux_core::Command::SetTitle {
            title: name.clone(),
        };
        match state
            .handle
            .apply(
                cmd,
                glaux_core::Author::System,
                format!("プロジェクト名を「{name}」に変更"),
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

    state.chat.switch_project(dest_str.clone());
    *state.project_dir.lock().expect("project_dir lock") = dest_str.clone();
    projects::remove_recent(&current);
    projects::push_recent(&dest_str, &title);
    set_window_title(&app, &title);
    Ok(json!({ "path": dest_str, "title": title, "project_version": version, "moved": true }))
}

/// 新規プロジェクトを作成して開く。`parent_dir/name.glaux` に作られる。
#[tauri::command]
async fn create_project(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    parent_dir: String,
    name: String,
) -> Result<Value, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("プロジェクト名が空です".to_owned());
    }
    if name.contains(['/', '\\', ':']) {
        return Err("プロジェクト名に使えない文字が含まれています".to_owned());
    }
    let parent = if parent_dir.trim().is_empty() {
        projects::default_projects_dir()
    } else {
        parent_dir.trim().to_owned()
    };
    let dir = std::path::Path::new(&parent).join(format!("{name}.glaux"));
    if dir.join("project.json").exists() {
        return Err(format!("既に存在します: {}", dir.display()));
    }
    open_project(app, state, dir.to_string_lossy().into_owned(), true).await
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
        return Err("commands が空です".to_owned());
    }
    let mut parsed = Vec::with_capacity(commands.len());
    for (i, value) in commands.into_iter().enumerate() {
        let cmd: Command = serde_json::from_value(value)
            .map_err(|e| format!("commands[{i}] を Command として解釈できません: {e}"))?;
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

// ---- エクスポート ----------------------------------------------------------

/// プロジェクトを WAV に書き出す(`<プロジェクト>/export/` 配下、48kHz/16bit)。
/// 再生と同じレンダラを使うので聴こえている音がそのまま書き出される。
#[tauri::command]
async fn export_project_wav(state: State<'_, AppState>) -> Result<Value, String> {
    let (project, _) = state.handle.get_project().await?;
    let file_name = format!(
        "{}_{}.wav",
        sanitize_file_name(&project.meta.title),
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    );
    let path = std::path::Path::new(&state.project_dir())
        .join("export")
        .join(file_name);

    // レンダリングは CPU バウンドなのでブロッキングスレッドで
    let path_for_render = path.clone();
    let project_dir = state.project_dir();
    let seconds = tauri::async_runtime::spawn_blocking(move || {
        let bank = glaux_engine::SampleBank::load(&project, std::path::Path::new(&project_dir));
        glaux_engine::export_wav(&project, &path_for_render, 48_000.0, &bank)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    Ok(json!({ "path": path.to_string_lossy(), "seconds": seconds }))
}

fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "untitled".to_owned()
    } else {
        cleaned
    }
}

// ---- トランスポート(再生) ----------------------------------------------

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
    let (project, _) = state.handle.get_project().await?;
    let index = project
        .track_index(&track_id)
        .ok_or_else(|| format!("track not found: {track_id}"))?;
    engine.preview_note(index, pitch.min(127), 100, 250);
    Ok(())
}

// ---- チャット(UI → AI 指示) --------------------------------------------

/// 前回のターン以降に人間が行った編集をまとめた、AI 向けのコンテキスト文を作る。
/// あわせて「AI に見せた最新エントリ」を更新する。注入するものが無ければ None。
async fn build_chat_context(state: &AppState) -> Option<String> {
    let since_raw = state.chat.last_seen_entry();
    let since = since_raw.as_deref().and_then(|s| EntryId::parse(s).ok());

    // 初回(記録なし)はコンテキスト不要。現在位置だけ記録する
    let Some(since) = since else {
        if let Ok(Ok((entries, _))) = state.handle.get_history(None, None, Some(1)).await {
            state
                .chat
                .set_last_seen_entry(entries.last().map(|e| e.id.to_string()));
        }
        return None;
    };

    let mut rolled_back = false;
    let (entries, version) = match state.handle.get_history(None, Some(since), None).await {
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
) -> Result<(), String> {
    state.chat.set_model(model)?;
    let prompt = prompt.trim().to_owned();
    if prompt.is_empty() {
        return Err("指示が空です".to_owned());
    }
    if state.chat.is_running() {
        return Err("前の指示がまだ実行中です".to_owned());
    }
    let full_prompt = match build_chat_context(&state).await {
        Some(ctx) => format!("{ctx}\n{prompt}"),
        None => prompt,
    };
    let mgr = state.chat.clone();
    tauri::async_runtime::spawn(chat::run_turn(app, mgr, full_prompt));
    Ok(())
}

#[tauri::command]
fn cancel_chat(state: State<'_, AppState>) {
    state.chat.cancel();
}

/// 会話をリセットする(次の送信が新しいセッションになる)。
#[tauri::command]
fn reset_chat(state: State<'_, AppState>) {
    state.chat.reset();
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

fn mcp_port() -> u16 {
    std::env::var("GLAUX_MCP_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_MCP_PORT)
}

/// アプリ内 MCP サーバー(streamable HTTP)。UI と同じ SessionHandle を共有する。
async fn serve_mcp(handle: SessionHandle, port: u16) -> Result<()> {
    let service: StreamableHttpService<GlauxServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(GlauxServer::new(handle.clone())),
            Default::default(),
            Default::default(),
        );
    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| format!("MCP ポート {port} を bind できません(既に起動中?)"))?;
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

    let project_dir = resolve_project_dir();
    let port = mcp_port();
    let (store, session) =
        Store::open_or_create(&project_dir).context("プロジェクトを開けません")?;
    tracing::info!(
        "プロジェクトを開きました: {}(履歴 {} エントリ)",
        store.dir().display(),
        session.history().len()
    );
    let project_title = session.project().meta.title.clone();
    projects::push_recent(&project_dir, &project_title);
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
        chat: Arc::new(ChatManager::new(mcp_url, project_dir.clone())),
        engine: engine.clone(),
        calib: std::sync::Mutex::new(None),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .setup(move |app| {
            set_window_title(app.handle(), &project_title);

            // アプリ内 MCP サーバー
            let mcp_handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = serve_mcp(mcp_handle, port).await {
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

            // プロジェクトの変更をエンジンの再生データに反映する。
            // AI の連続編集で毎回全再構築しないよう、短い静穏時間でイベントを合流させる
            if let Some(engine) = engine.clone() {
                let session = handle.clone();
                tauri::async_runtime::spawn(async move {
                    // プロジェクト移動・切り替えに追従するため dir は毎回引く
                    let sync = |project: glaux_core::Project, dir: String| {
                        let engine = engine.clone();
                        async move {
                            tauri::async_runtime::spawn_blocking(move || {
                                engine.set_project(&project, std::path::Path::new(&dir));
                            })
                            .await
                            .ok();
                        }
                    };
                    if let (Ok((project, _)), Ok(dir)) =
                        (session.get_project().await, session.project_dir().await)
                    {
                        sync(project, dir).await;
                    }
                    let mut rx = session.subscribe();
                    loop {
                        use tokio::sync::broadcast::error::{RecvError, TryRecvError};
                        match rx.recv().await {
                            Ok(_) | Err(RecvError::Lagged(_)) => {
                                // 40ms 待って、その間に来たイベントをまとめて捨てる
                                tokio::time::sleep(std::time::Duration::from_millis(40)).await;
                                while matches!(rx.try_recv(), Ok(_) | Err(TryRecvError::Lagged(_)))
                                {
                                }
                                if let (Ok((project, _)), Ok(dir)) =
                                    (session.get_project().await, session.project_dir().await)
                                {
                                    sync(project, dir).await;
                                }
                            }
                            Err(RecvError::Closed) => break,
                        }
                    }
                });
            }

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
            get_history,
            undo,
            redo,
            app_info,
            list_recent_projects,
            set_projects_dir,
            open_project,
            move_project,
            list_presets,
            save_preset,
            load_preset,
            get_track_params,
            import_sample,
            list_soundfonts,
            list_soundfont_presets,
            add_soundfont,
            create_project,
            export_project_wav,
            apply_edit,
            revert_entry,
            import_audio_clip,
            clip_peaks,
            transcribe_clip,
            record_start,
            record_stop,
            midi_inputs,
            set_midi_input,
            set_live_target,
            midi_record_start,
            midi_record_stop,
            audio_devices,
            get_master_params,
            set_output_device,
            set_input_device,
            input_monitor,
            calibrate_start,
            calibrate_stop,
            transport_set_metronome,
            send_chat,
            cancel_chat,
            reset_chat,
            transport_state,
            transport_set_loop,
            transport_clear_loop,
            transport_play,
            transport_pause,
            transport_stop,
            transport_seek,
            preview_note
        ])
        .run(tauri::generate_context!())
        .context("Tauri の起動に失敗")?;
    Ok(())
}
