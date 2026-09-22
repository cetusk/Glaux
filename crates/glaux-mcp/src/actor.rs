//! Session アクター。
//!
//! `Session` は専用スレッドが 1 つだけ所有し、他のスレッド(MCP ハンドラ、将来の
//! Tauri UI)は [`SessionHandle`] 経由でリクエストを送る。全編集が 1 本のキューを
//! 通るので、Undo 履歴の直列化が自然に保たれる(`docs/HANDOFF.md` のアクター方式)。
//!
//! 変更が成功するたびに [`Store`](crate::store::Store) へ保存する。保存失敗は
//! メモリ上の状態を巻き戻さず、警告としてレスポンスに載せる。

use crate::store::Store;
use glaux_core::{Author, Change, Command, CoreError, EntryId, HistoryEntry, Project, Session};
use tokio::sync::{broadcast, mpsc, oneshot};

/// 状態が変わったことの通知。UI(Tauri)がこれを購読して画面を更新する。
/// checkpoint のようにプロジェクト本体が変わらない操作でも送る(changes は空)。
#[derive(Clone, Debug, serde::Serialize)]
pub struct ProjectChanged {
    pub project_version: usize,
    pub changes: Vec<Change>,
}

/// AI(MCP クライアント)のツール呼び出し状況。UI の「AI 作業中」表示に使う。
/// `busy` は進行中の呼び出しが 1 つ以上あるか。`tool` はこのイベントを起こした呼び出し。
#[derive(Clone, Debug, serde::Serialize)]
pub struct AiActivity {
    pub tool: String,
    pub busy: bool,
}

/// 履歴一覧用の軽量ビュー(forward/inverse は含めない)。
#[derive(Clone, Debug, serde::Serialize)]
pub struct EntrySummary {
    pub id: EntryId,
    pub author: Author,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub label: String,
    pub targets: Vec<glaux_core::Target>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reverts: Option<EntryId>,
}

impl From<&HistoryEntry> for EntrySummary {
    fn from(e: &HistoryEntry) -> Self {
        EntrySummary {
            id: e.id.clone(),
            author: e.author.clone(),
            timestamp: e.timestamp,
            label: e.label.clone(),
            targets: e.targets.iter().cloned().collect(),
            reverts: e.reverts.clone(),
        }
    }
}

/// 変更系リクエストの共通レスポンス部品。
#[derive(Clone, Debug)]
pub struct Mutated {
    pub changes: Vec<Change>,
    /// 適用済み履歴エントリ数。AI が自分の把握が古いか判断するための版数。
    pub project_version: usize,
    /// 保存に失敗したときのエラーメッセージ(状態はメモリ上では反映済み)
    pub save_error: Option<String>,
}

pub enum Request {
    GetProject {
        reply: oneshot::Sender<(Project, usize)>,
    },
    Apply {
        /// `Command` は最大バリアントが大きいので Box で持つ(clippy::large_enum_variant)
        command: Box<Command>,
        author: Author,
        label: String,
        reply: oneshot::Sender<Result<(EntryId, Mutated), CoreError>>,
    },
    Undo {
        n: usize,
        reply: oneshot::Sender<Result<(usize, Mutated), CoreError>>,
    },
    Redo {
        n: usize,
        reply: oneshot::Sender<Result<(usize, Mutated), CoreError>>,
    },
    Checkpoint {
        label: String,
        reply: oneshot::Sender<Mutated>,
    },
    RevertTo {
        label: String,
        reply: oneshot::Sender<Result<Mutated, CoreError>>,
    },
    GetHistory {
        /// `Author` の種別名("human" | "ai" | "system")。None なら全部。
        author_kind: Option<String>,
        /// このエントリ ID より後(そのエントリ自体は含まない)
        since: Option<EntryId>,
        /// 末尾(最新)から最大件数
        limit: Option<usize>,
        reply: oneshot::Sender<Result<(Vec<EntrySummary>, usize), CoreError>>,
    },
    /// 別のプロジェクトフォルダに切り替える。アクターが Session + Store を
    /// 丸ごと差し替えるので、ハンドルを持つ全員(UI / MCP / エンジン)は
    /// そのまま新しいプロジェクトに追従する。成功時は (タイトル, バージョン)。
    SwitchProject {
        dir: String,
        reply: oneshot::Sender<Result<(String, usize), String>>,
    },
    /// 現在のプロジェクトフォルダを別の場所へ移動して開き直す。
    /// フォルダ移動をアクター内で行うことで、進行中の保存と直列化される
    /// (移動中に古い場所へ書き込まれる競合が起きない)。成功時は (タイトル, バージョン)。
    MoveProject {
        dest: String,
        reply: oneshot::Sender<Result<(String, usize), String>>,
    },
}

#[derive(Clone)]
pub struct SessionHandle {
    tx: mpsc::UnboundedSender<Request>,
    events: broadcast::Sender<ProjectChanged>,
    activity: broadcast::Sender<AiActivity>,
    active_calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

/// ツール呼び出し 1 件の進行を表す RAII ガード。
/// 生成時に「開始」、drop 時に「終了」を購読者へ通知する。
pub struct ActivityGuard {
    tool: String,
    activity: broadcast::Sender<AiActivity>,
    active_calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        let remaining = self.active_calls.fetch_sub(1, Ordering::SeqCst) - 1;
        let _ = self.activity.send(AiActivity {
            tool: self.tool.clone(),
            busy: remaining > 0,
        });
    }
}

/// アクタースレッドが落ちた(通常は起こらない)ときのエラー文言。
const ACTOR_GONE: &str = "session actor is gone";

impl SessionHandle {
    /// アクタースレッドを起動してハンドルを返す。
    pub fn spawn(session: Session, store: Store) -> SessionHandle {
        let (tx, rx) = mpsc::unbounded_channel();
        let (events, _) = broadcast::channel(256);
        let (activity, _) = broadcast::channel(256);
        let events_tx = events.clone();
        std::thread::Builder::new()
            .name("glaux-session".into())
            .spawn(move || actor_loop(session, store, rx, events_tx))
            .expect("session actor spawn");
        SessionHandle {
            tx,
            events,
            activity,
            active_calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// 状態変更の通知を購読する。受信が追いつかない場合は古い通知が落ちる
    /// (`Lagged`)ので、その際は全体を取得し直すこと。
    pub fn subscribe(&self) -> broadcast::Receiver<ProjectChanged> {
        self.events.subscribe()
    }

    /// AI のツール呼び出し状況(開始/終了)を購読する。UI の「AI 作業中」表示用。
    pub fn subscribe_activity(&self) -> broadcast::Receiver<AiActivity> {
        self.activity.subscribe()
    }

    /// ツール呼び出しの開始を記録する。返り値のガードを呼び出しの間保持し、
    /// drop で終了が通知される(MCP ツール実装が呼ぶ)。
    pub fn begin_activity(&self, tool: &str) -> ActivityGuard {
        use std::sync::atomic::Ordering;
        self.active_calls.fetch_add(1, Ordering::SeqCst);
        let _ = self.activity.send(AiActivity {
            tool: tool.to_owned(),
            busy: true,
        });
        ActivityGuard {
            tool: tool.to_owned(),
            activity: self.activity.clone(),
            active_calls: self.active_calls.clone(),
        }
    }

    async fn request<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<T>) -> Request,
    ) -> Result<T, String> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(make(reply))
            .map_err(|_| ACTOR_GONE.to_owned())?;
        rx.await.map_err(|_| ACTOR_GONE.to_owned())
    }

    pub async fn get_project(&self) -> Result<(Project, usize), String> {
        self.request(|reply| Request::GetProject { reply }).await
    }

    pub async fn apply(
        &self,
        command: Command,
        author: Author,
        label: String,
    ) -> Result<Result<(EntryId, Mutated), CoreError>, String> {
        self.request(|reply| Request::Apply {
            command: Box::new(command),
            author,
            label,
            reply,
        })
        .await
    }

    pub async fn undo(&self, n: usize) -> Result<Result<(usize, Mutated), CoreError>, String> {
        self.request(|reply| Request::Undo { n, reply }).await
    }

    pub async fn redo(&self, n: usize) -> Result<Result<(usize, Mutated), CoreError>, String> {
        self.request(|reply| Request::Redo { n, reply }).await
    }

    pub async fn checkpoint(&self, label: String) -> Result<Mutated, String> {
        self.request(|reply| Request::Checkpoint { label, reply })
            .await
    }

    pub async fn revert_to(&self, label: String) -> Result<Result<Mutated, CoreError>, String> {
        self.request(|reply| Request::RevertTo { label, reply })
            .await
    }

    pub async fn get_history(
        &self,
        author_kind: Option<String>,
        since: Option<EntryId>,
        limit: Option<usize>,
    ) -> Result<Result<(Vec<EntrySummary>, usize), CoreError>, String> {
        self.request(|reply| Request::GetHistory {
            author_kind,
            since,
            limit,
            reply,
        })
        .await
    }

    /// 別のプロジェクトに切り替える。成功時は (タイトル, project_version)。
    pub async fn switch_project(&self, dir: String) -> Result<(String, usize), String> {
        self.request(|reply| Request::SwitchProject { dir, reply })
            .await?
    }

    /// 現在のプロジェクトフォルダを `dest` へ移動して開き直す。
    pub async fn move_project(&self, dest: String) -> Result<(String, usize), String> {
        self.request(|reply| Request::MoveProject { dest, reply })
            .await?
    }
}

/// フォルダを移動する。同一ボリュームなら rename、失敗したらコピー + 削除。
fn move_dir(from: &std::path::Path, to: &std::path::Path) -> Result<(), String> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    // 別ドライブなどで rename できない場合のフォールバック
    copy_dir_recursive(from, to).map_err(|e| format!("コピーに失敗しました: {e}"))?;
    std::fs::remove_dir_all(from)
        .map_err(|e| format!("移動元の削除に失敗しました(コピーは完了): {e}"))
}

fn copy_dir_recursive(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
}

fn actor_loop(
    mut session: Session,
    mut store: Store,
    mut rx: mpsc::UnboundedReceiver<Request>,
    events: broadcast::Sender<ProjectChanged>,
) {
    while let Some(req) = rx.blocking_recv() {
        handle(&mut session, &mut store, &events, req);
    }
    tracing::info!("session actor: 全ハンドルが閉じたので終了します");
}

fn version(session: &Session) -> usize {
    session.history().len()
}

/// 変更後の保存・購読者への通知・レスポンス部品の組み立て。
fn mutated(
    session: &Session,
    store: &Store,
    events: &broadcast::Sender<ProjectChanged>,
    changes: Vec<Change>,
) -> Mutated {
    let save_error = store.save_after_change(session).err().map(|e| {
        tracing::error!("保存に失敗しました: {e:#}");
        format!("{e:#}")
    });
    // 購読者ゼロは正常(MCP 単体起動時)なのでエラーは無視
    let _ = events.send(ProjectChanged {
        project_version: version(session),
        changes: changes.clone(),
    });
    Mutated {
        changes,
        project_version: version(session),
        save_error,
    }
}

fn handle(
    session: &mut Session,
    store: &mut Store,
    events: &broadcast::Sender<ProjectChanged>,
    req: Request,
) {
    match req {
        Request::GetProject { reply } => {
            let _ = reply.send((session.project().clone(), version(session)));
        }
        Request::Apply {
            command,
            author,
            label,
            reply,
        } => {
            let result = session
                .apply(*command, author, label)
                .map(|(id, changes)| (id, mutated(session, store, events, changes)));
            let _ = reply.send(result);
        }
        Request::Undo { n, reply } => {
            let _ = reply.send(undo_redo(session, store, events, n, Session::undo));
        }
        Request::Redo { n, reply } => {
            let _ = reply.send(undo_redo(session, store, events, n, Session::redo));
        }
        Request::Checkpoint { label, reply } => {
            session.checkpoint(label);
            let _ = reply.send(mutated(session, store, events, vec![]));
        }
        Request::RevertTo { label, reply } => {
            let result = session
                .revert_to(&label)
                .map(|changes| mutated(session, store, events, changes));
            let _ = reply.send(result);
        }
        Request::GetHistory {
            author_kind,
            since,
            limit,
            reply,
        } => {
            let _ = reply.send(history_view(session, author_kind, since, limit));
        }
        Request::SwitchProject { dir, reply } => {
            let result = match Store::open_or_create(&dir) {
                Ok((new_store, new_session)) => {
                    *store = new_store;
                    *session = new_session;
                    let version = version(session);
                    // 購読者(UI 再取得・エンジン再構築)に全体更新を促す
                    let _ = events.send(ProjectChanged {
                        project_version: version,
                        changes: vec![],
                    });
                    tracing::info!("プロジェクトを切り替えました: {dir}");
                    Ok((session.project().meta.title.clone(), version))
                }
                Err(e) => Err(format!("{e:#}")),
            };
            let _ = reply.send(result);
        }
        Request::MoveProject { dest, reply } => {
            let from = store.dir().to_path_buf();
            let to = std::path::PathBuf::from(&dest);
            let result = (|| {
                if to.exists() {
                    return Err(format!("移動先が既に存在します: {dest}"));
                }
                if let Some(parent) = to.parent() {
                    if !parent.is_dir() {
                        return Err(format!(
                            "移動先のフォルダがありません: {}",
                            parent.display()
                        ));
                    }
                }
                move_dir(&from, &to)?;
                match Store::open_or_create(&dest) {
                    Ok((new_store, new_session)) => {
                        *store = new_store;
                        *session = new_session;
                        let version = version(session);
                        let _ = events.send(ProjectChanged {
                            project_version: version,
                            changes: vec![],
                        });
                        tracing::info!("プロジェクトを移動しました: {} → {dest}", from.display());
                        Ok((session.project().meta.title.clone(), version))
                    }
                    Err(e) => {
                        // 開き直しに失敗したら元の場所へ戻して被害を抑える
                        let _ = move_dir(&to, &from);
                        Err(format!("移動先で開けません(元に戻しました): {e:#}"))
                    }
                }
            })();
            let _ = reply.send(result);
        }
    }
}

fn undo_redo(
    session: &mut Session,
    store: &Store,
    events: &broadcast::Sender<ProjectChanged>,
    n: usize,
    step: fn(&mut Session) -> glaux_core::error::Result<Option<Vec<Change>>>,
) -> Result<(usize, Mutated), CoreError> {
    let mut all_changes = Vec::new();
    let mut done = 0;
    for _ in 0..n {
        match step(session)? {
            Some(changes) => {
                all_changes.extend(changes);
                done += 1;
            }
            None => break,
        }
    }
    // 1 歩も動いていなければ保存も不要
    let m = if done > 0 {
        mutated(session, store, events, all_changes)
    } else {
        Mutated {
            changes: vec![],
            project_version: version(session),
            save_error: None,
        }
    };
    Ok((done, m))
}

fn history_view(
    session: &Session,
    author_kind: Option<String>,
    since: Option<EntryId>,
    limit: Option<usize>,
) -> Result<(Vec<EntrySummary>, usize), CoreError> {
    let applied = session.history().applied();

    let start = match &since {
        Some(id) => {
            let idx = applied
                .iter()
                .position(|e| &e.id == id)
                .ok_or_else(|| CoreError::EntryNotFound(id.clone()))?;
            idx + 1
        }
        None => 0,
    };

    let mut list: Vec<EntrySummary> = applied[start..]
        .iter()
        .filter(|e| match author_kind.as_deref() {
            None => true,
            Some("human") => matches!(e.author, Author::Human),
            Some("ai") => matches!(e.author, Author::Ai { .. }),
            Some("system") => matches!(e.author, Author::System),
            Some(_) => false,
        })
        .map(EntrySummary::from)
        .collect();

    // 最新側を優先して limit 件(返却順は古い→新しいのまま)
    if let Some(limit) = limit {
        if list.len() > limit {
            list.drain(..list.len() - limit);
        }
    }
    Ok((list, version(session)))
}
