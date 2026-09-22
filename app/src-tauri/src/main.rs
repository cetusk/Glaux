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
    }
    let (title, version) = state.handle.switch_project(path.clone()).await?;
    state.chat.switch_project(path.clone());
    *state.project_dir.lock().expect("project_dir lock") = path.clone();
    projects::push_recent(&path, &title);
    set_window_title(&app, &title);
    Ok(json!({ "title": title, "project_version": version, "path": path }))
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
    let seconds = tauri::async_runtime::spawn_blocking(move || {
        glaux_engine::export_wav(&project, &path_for_render, 48_000.0)
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
            "tick": e.playhead_tick(),
        }),
        None => json!({ "available": false, "playing": false, "tick": 0 }),
    }
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
) -> Result<(), String> {
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
                    if let Ok((project, _)) = session.get_project().await {
                        engine.set_project(&project);
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
                                if let Ok((project, _)) = session.get_project().await {
                                    engine.set_project(&project);
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
            open_project,
            create_project,
            export_project_wav,
            apply_edit,
            send_chat,
            cancel_chat,
            reset_chat,
            transport_state,
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
