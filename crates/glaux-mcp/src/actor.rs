//! Session アクター。
//!
//! `Session` は専用スレッドが 1 つだけ所有し、他のスレッド(MCP ハンドラ、将来の
//! Tauri UI)は [`SessionHandle`] 経由でリクエストを送る。全編集が 1 本のキューを
//! 通るので、Undo 履歴の直列化が自然に保たれる(`docs/HANDOFF.md` のアクター方式)。
//!
//! 変更が成功するたびに [`Store`](crate::store::Store) へ保存する。保存失敗は
//! メモリ上の状態を巻き戻さず、警告としてレスポンスに載せる。

use crate::plan_store::{PlanSession, PlanStore};
use crate::store::Store;
use glaux_core::plan::PlanCommand;
use glaux_core::{
    Author, Change, Command, CoreError, EntryId, EntryNote, HistoryEntry, Project, Session,
};
use tokio::sync::{broadcast, mpsc, oneshot};

/// 状態が変わったことの通知。UI(Tauri)がこれを購読して画面を更新する。
/// checkpoint のようにプロジェクト本体が変わらない操作でも送る(changes は空)。
#[derive(Clone, Debug, serde::Serialize)]
pub struct ProjectChanged {
    pub project_version: usize,
    pub changes: Vec<Change>,
    /// 保存に失敗したときのエラー(状態はメモリ上では反映済み)。UI が警告を出すのに使う
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_error: Option<String>,
    /// 曲の中身は変わらず、履歴だけが変わった(チェックポイント)。保存・再生データの作り直し・画面の全体取得は要らない
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub history_only: bool,
    /// 計画(plans.json)が変わったときの計画の版(この曲を開いてからの変更の回数。計画の変更のときだけ)。
    /// 曲の中身は変わらないので history_only も立てる
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plans_version: Option<u64>,
}

/// AI(MCP クライアント)のツール呼び出し状況。UI の「AI 作業中」表示に使う。
/// `busy` は進行中の呼び出しが 1 つ以上あるか。`tool` はこのイベントを起こした呼び出し。
#[derive(Clone, Debug, serde::Serialize)]
pub struct AiActivity {
    pub tool: String,
    pub busy: bool,
}

/// 履歴の一覧の 1 ページ。
#[derive(Clone, Debug, serde::Serialize)]
pub struct HistoryPage {
    pub project_version: usize,
    /// 条件(作者・since)に合うエントリの総数(limit で切る前)
    pub total: usize,
    pub entries: Vec<EntrySummary>,
    /// やり直せる(undo 済みの)エントリ。次に redo されるものが先頭。最大 20 件
    pub redoable: Vec<EntrySummary>,
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
    /// 版数(編集・undo・redo のたびに増え、undo でも戻らない)。AI が自分の把握が古いか判断するのに使う。
    pub project_version: usize,
    /// 保存に失敗したときのエラーメッセージ(状態はメモリ上では反映済み)
    pub save_error: Option<String>,
    /// 編集の後の確認(AI の道具の編集だけ。サーバーが付ける。glaux_core::aftercare)
    pub aftercare: Option<serde_json::Value>,
    /// 固定の音を守るために外した編集(AI の編集だけ。glaux_core::made::guard_locks)
    pub locks: Vec<glaux_core::made::LockHit>,
}

/// `RevertEntry` の返り値: (revert エントリ, 衝突エントリ一覧, Mutated)。
pub type RevertOutcome = (EntryId, Vec<EntryId>, Mutated);

/// 旋律の計画への要求(曲とは別の文書・別の履歴。曲のフォルダが変わったら開き直す)
pub enum PlanRequest {
    /// 計画のセッションの複製(計画と履歴。小さいので丸ごと渡し、読み取りはサーバー側で行う)
    Get {
        reply: oneshot::Sender<Result<PlanSession, String>>,
    },
    /// 経緯付きで適用して保存する
    Apply {
        command: Box<PlanCommand>,
        author: Author,
        label: String,
        note: EntryNote,
        reply: oneshot::Sender<Result<EntryId, String>>,
    },
    /// 計画の undo(`redo` なら redo)を n 回。返り値は実際に動かした数
    Step {
        n: usize,
        redo: bool,
        reply: oneshot::Sender<Result<usize, String>>,
    },
    /// 途中の変更の取り消し(git revert)。返り値は (新しいエントリ, 後で同じ計画を変えたエントリ)
    Revert {
        id: EntryId,
        author: Author,
        note: EntryNote,
        reply: oneshot::Sender<Result<(EntryId, Vec<EntryId>), String>>,
    },
}

/// アクターが持つ計画の状態(曲のフォルダごと)
struct Plans {
    store: PlanStore,
    session: PlanSession,
    /// 計画の版(開いてから変えた回数。変更の通知に載せる)
    version: u64,
}

pub enum Request {
    GetProject {
        reply: oneshot::Sender<(std::sync::Arc<Project>, usize)>,
    },
    /// テスト用: 処理中に panic させる(アクターが止まらずに復帰することの確認)
    #[cfg(test)]
    Panic { reply: oneshot::Sender<()> },
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
    /// 履歴の途中のエントリを個別に取り消す(`git revert` 相当)。
    /// 成功時は (新しく積まれた revert エントリ, 衝突エントリ一覧, Mutated)。
    RevertEntry {
        id: EntryId,
        author: Author,
        reply: oneshot::Sender<Result<RevertOutcome, CoreError>>,
    },
    /// 履歴エントリ本体(コマンド込み)。`since` より後、最新側から `limit` 件(古い → 新しい)
    GetEntries {
        since: Option<EntryId>,
        limit: usize,
        reply: oneshot::Sender<Result<Vec<HistoryEntry>, CoreError>>,
    },
    /// 履歴の地点でのプロジェクト(今のセッションは変えない)。聴き比べ用。
    /// 返り値は (その地点のプロジェクト, 今のプロジェクト, 今のバージョン, 戻した編集の数)
    ProjectAt {
        point: glaux_core::HistoryPoint,
        reply: oneshot::Sender<Result<(Project, Project, usize, usize), CoreError>>,
    },
    GetHistory {
        /// `Author` の種別名("human" | "ai" | "system")。None なら全部。
        author_kind: Option<String>,
        /// このエントリ ID より後(そのエントリ自体は含まない)
        since: Option<EntryId>,
        /// 末尾(最新)から最大件数
        limit: Option<usize>,
        reply: oneshot::Sender<Result<HistoryPage, CoreError>>,
    },
    /// 別のプロジェクトフォルダに切り替える。アクターが Session + Store を
    /// 丸ごと差し替えるので、ハンドルを持つ全員(UI / MCP / エンジン)は
    /// そのまま新しいプロジェクトに追従する。成功時は (タイトル, バージョン)。
    SwitchProject {
        dir: String,
        reply: oneshot::Sender<Result<(String, usize), String>>,
    },
    /// 現在のプロジェクトフォルダの絶対パスを返す。
    ProjectDir { reply: oneshot::Sender<String> },
    /// 裏で書きかけの `project.json` を書き終えるまで待つ(アプリ・サーバーの終了時)
    Flush {
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// 現在のプロジェクトフォルダを別の場所へ移動して開き直す。
    /// フォルダ移動をアクター内で行うことで、進行中の保存と直列化される
    /// (移動中に古い場所へ書き込まれる競合が起きない)。成功時は (タイトル, バージョン)。
    MoveProject {
        dest: String,
        reply: oneshot::Sender<Result<(String, usize), String>>,
    },
    /// 旋律の計画(曲とは別の履歴)
    Plan(PlanRequest),
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

/// アクターが要求に応答しなかった(内部エラーで処理を打ち切った、またはスレッドが無い)ときのエラー文言。
const ACTOR_GONE: &str =
    "セッションが要求を処理できませんでした(内部エラー。直前の保存済みの状態に戻しました)";

impl SessionHandle {
    /// アクタースレッドを起動してハンドルを返す。
    pub fn spawn(session: Session, store: Store) -> SessionHandle {
        let (tx, rx) = mpsc::unbounded_channel();
        let (events, _) = broadcast::channel(256);
        let (activity, _) = broadcast::channel(256);
        let events_tx = events.clone();
        std::thread::Builder::new()
            .name("glaux-session".into())
            .spawn(move || {
                let mut store = store;
                store.enable_background_writes();
                actor_loop(session, store, rx, events_tx)
            })
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

    /// 今のプロジェクトの複製(書き換えて使う用。読むだけなら [`Self::get_project_shared`])
    pub async fn get_project(&self) -> Result<(Project, usize), String> {
        let (p, v) = self.get_project_shared().await?;
        Ok((std::sync::Arc::unwrap_or_clone(p), v))
    }

    /// 今のプロジェクト(読み取り用。変更のたびに 1 回だけ作られた複製を共有する)
    pub async fn get_project_shared(&self) -> Result<(std::sync::Arc<Project>, usize), String> {
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

    /// 曲の undo を n 回。取り消した編集と一組の計画の変更(案の採用など)も戻す
    pub async fn undo(&self, n: usize) -> Result<Result<(usize, Mutated), CoreError>, String> {
        let before = self.song_ids(n).await.0;
        let r = self.request(|reply| Request::Undo { n, reply }).await?;
        if let Ok((k, _)) = &r {
            // 取り消した編集(新しい順)
            for id in before.iter().rev().take(*k) {
                self.flip_linked_plan(id, id).await;
            }
        }
        Ok(r)
    }

    /// 曲の redo を n 回。やり直した編集と一組の計画の変更も、もう一度効かせる
    pub async fn redo(&self, n: usize) -> Result<Result<(usize, Mutated), CoreError>, String> {
        let before = self.song_ids(n).await.1;
        let r = self.request(|reply| Request::Redo { n, reply }).await?;
        if let Ok((k, _)) = &r {
            for id in before.iter().take(*k) {
                self.flip_linked_plan(id, id).await;
            }
        }
        Ok(r)
    }

    /// 曲の履歴の (効いている編集の ID〈新しい側から `n` 件、古い順〉, やり直せる編集の ID〈次にやり直す順〉)
    async fn song_ids(&self, n: usize) -> (Vec<EntryId>, Vec<EntryId>) {
        match self.get_history(None, None, Some(n.max(1))).await {
            Ok(Ok(page)) => (
                page.entries.into_iter().map(|e| e.id).collect(),
                page.redoable.into_iter().map(|e| e.id).collect(),
            ),
            _ => (vec![], vec![]),
        }
    }

    /// 曲の編集 `song` と一組の計画の変更(計画の経緯の song_entry が `song`)を、今と逆の向きにする:
    /// 効いていれば取り消し、取り消してあれば戻す。どちらも計画の「この変更だけ取り消す」で行う
    /// (後から計画に入った別の変更は残し、戻したことも履歴に残る)。戻した記録は `link`(その操作で動いた
    /// 曲の履歴の項目)と一組にする。一組の変更が無ければ何もしない。戻せなかったときは記録だけ残す
    async fn flip_linked_plan(&self, song: &EntryId, link: &EntryId) {
        let Ok(plans) = self.get_plans().await else {
            return;
        };
        let applied = plans.history().applied();
        let linked: Vec<_> = applied
            .iter()
            .filter(|e| e.note.as_ref().and_then(|n| n.song_entry.as_ref()) == Some(song))
            .collect();
        // 今の向きを決めている項目: 一組の中で、まだ別の項目に取り消されていないもの
        let live: Vec<EntryId> = linked
            .iter()
            .filter(|e| !linked.iter().any(|o| o.reverts.as_ref() == Some(&e.id)))
            .map(|e| e.id.clone())
            .collect();
        for id in live.into_iter().rev() {
            let note = EntryNote {
                why: "一組の曲の編集を取り消した・やり直したので、計画の変更も合わせる".to_owned(),
                song_entry: Some(link.clone()),
                ..Default::default()
            };
            if let Err(e) = self.revert_plan(id, Author::System, note).await {
                tracing::warn!("曲の編集と一組の計画の変更を戻せませんでした: {e}");
            }
        }
    }

    pub async fn checkpoint(&self, label: String) -> Result<Mutated, String> {
        self.request(|reply| Request::Checkpoint { label, reply })
            .await
    }

    /// チェックポイントまで戻す。戻した編集と一組の計画の変更も戻す
    pub async fn revert_to(&self, label: String) -> Result<Result<Mutated, CoreError>, String> {
        let before = self.song_ids(usize::MAX / 2).await.0;
        let r = self
            .request(|reply| Request::RevertTo { label, reply })
            .await?;
        if r.is_ok() {
            let after = self.song_ids(usize::MAX / 2).await.0;
            for id in before.iter().rev().filter(|id| !after.contains(id)) {
                self.flip_linked_plan(id, id).await;
            }
        }
        Ok(r)
    }

    /// 履歴の途中のエントリを個別に取り消す。
    pub async fn revert_entry(
        &self,
        id: EntryId,
        author: Author,
    ) -> Result<Result<RevertOutcome, CoreError>, String> {
        let r = self
            .request(|reply| Request::RevertEntry {
                id: id.clone(),
                author,
                reply,
            })
            .await?;
        // 取り消した編集と一組の計画の変更も戻す(戻した記録は、取り消しの新しい項目と一組にする)
        if let Ok((new_id, _, _)) = &r {
            self.flip_linked_plan(&id, new_id).await;
        }
        Ok(r)
    }

    /// 履歴エントリ本体(コマンド込み)。`since` より後、最新側から `limit` 件
    pub async fn get_entries(
        &self,
        since: Option<EntryId>,
        limit: usize,
    ) -> Result<Result<Vec<HistoryEntry>, CoreError>, String> {
        self.request(|reply| Request::GetEntries {
            since,
            limit,
            reply,
        })
        .await
    }

    /// 履歴の地点でのプロジェクトと今のプロジェクト(今のセッションは変えない)
    pub async fn project_at(
        &self,
        point: glaux_core::HistoryPoint,
    ) -> Result<Result<(Project, Project, usize, usize), CoreError>, String> {
        self.request(|reply| Request::ProjectAt { point, reply })
            .await
    }

    pub async fn get_history(
        &self,
        author_kind: Option<String>,
        since: Option<EntryId>,
        limit: Option<usize>,
    ) -> Result<Result<HistoryPage, CoreError>, String> {
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

    /// 現在のプロジェクトフォルダの絶対パス。
    /// 裏で書きかけの `project.json` を書き終えるまで待つ(終了の前に呼ぶ)
    pub async fn flush(&self) -> Result<(), String> {
        self.request(|reply| Request::Flush { reply }).await?
    }

    /// 計画と計画の履歴(複製)
    pub async fn get_plans(&self) -> Result<PlanSession, String> {
        self.request(|reply| Request::Plan(PlanRequest::Get { reply }))
            .await?
    }

    /// 計画のコマンドを経緯付きで適用して保存する
    pub async fn apply_plan(
        &self,
        command: PlanCommand,
        author: Author,
        label: String,
        note: EntryNote,
    ) -> Result<EntryId, String> {
        self.request(|reply| {
            Request::Plan(PlanRequest::Apply {
                command: Box::new(command),
                author,
                label,
                note,
                reply,
            })
        })
        .await?
    }

    /// 計画の undo / redo を n 回。曲の編集と一組の計画の変更(案の採用など)は、計画の側だけで動かすと
    /// 曲と計画が食い違うので動かさない(曲の側の取り消し・やり直しで一緒に戻る)
    pub async fn step_plan(&self, n: usize, redo: bool) -> Result<usize, String> {
        let plans = self.get_plans().await?;
        let linked = if redo {
            plans
                .history()
                .redoable()
                .iter()
                .take(n)
                .any(|e| e.note.as_ref().is_some_and(|n| n.song_entry.is_some()))
        } else {
            plans
                .history()
                .applied()
                .iter()
                .rev()
                .take(n)
                .any(|e| e.note.as_ref().is_some_and(|n| n.song_entry.is_some()))
        };
        if linked {
            return Err("曲の編集と一組の計画の変更(案の採用など)は、計画の側だけでは動かせません。\
                曲の側で取り消す・やり直すと、計画も一緒に戻ります(Ctrl+Z・履歴パネル・undo / redo)"
                .to_owned());
        }
        self.request(|reply| Request::Plan(PlanRequest::Step { n, redo, reply }))
            .await?
    }

    /// 計画の途中の変更を取り消す
    pub async fn revert_plan(
        &self,
        id: EntryId,
        author: Author,
        note: EntryNote,
    ) -> Result<(EntryId, Vec<EntryId>), String> {
        self.request(|reply| {
            Request::Plan(PlanRequest::Revert {
                id,
                author,
                note,
                reply,
            })
        })
        .await?
    }

    pub async fn project_dir(&self) -> Result<String, String> {
        self.request(|reply| Request::ProjectDir { reply }).await
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
    // 写し先が写し元の中にあるとき、写し先そのものは写さない(写すたびに増えて終わらなくなる)
    let to_real = std::fs::canonicalize(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        if std::fs::canonicalize(entry.path()).is_ok_and(|p| p == to_real) {
            continue;
        }
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
    let mut plans: Option<Plans> = None;
    while let Some(req) = rx.blocking_recv() {
        if let Request::Plan(req) = req {
            // 計画は曲とは別。曲のフォルダ(切り替え・移動・読み直しで変わる)に合わせて開く
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                handle_plan(&mut plans, store.dir(), req)
            }));
            match result {
                // 計画が変わったことを画面(と購読者)に知らせる。曲の中身は変わらない
                Ok(Some(v)) => {
                    let _ = events.send(ProjectChanged {
                        project_version: version(&session, &store),
                        changes: vec![],
                        save_error: None,
                        history_only: true,
                        plans_version: Some(v),
                    });
                }
                Ok(None) => {}
                Err(_) => {
                    tracing::error!("計画の要求の処理中に panic しました。次の要求で読み直します");
                    plans = None;
                }
            }
            continue;
        }
        // 1 件の要求の panic(コマンドの不具合など)でアクターごと止まると、以後は再起動まで
        // 編集も保存もできなくなる。受け止めて、保存済みの状態から読み直して動き続ける
        // (適用の途中で止まったセッションは信用できないため)。要求の応答は届かず、
        // 呼び出し側には ACTOR_GONE のエラーが返る
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            handle(&mut session, &mut store, &events, req)
        }));
        if result.is_err() {
            tracing::error!("要求の処理中に panic しました。保存済みの状態から読み直します");
            let previous = version(&session, &store);
            // 裏で書きかけの project.json を済ませてから読み直す
            if let Err(e) = store.flush() {
                tracing::error!("読み直しの前の保存に失敗しました: {e:#}");
            }
            match Store::open_or_create(store.dir()) {
                Ok((mut new_store, new_session)) => {
                    new_store.enable_background_writes();
                    new_store.continue_revision_after(previous);
                    store = new_store;
                    session = new_session;
                    let _ = events.send(ProjectChanged {
                        project_version: version(&session, &store),
                        changes: vec![],
                        save_error: None,
                        history_only: false,
                        plans_version: None,
                    });
                }
                Err(e) => {
                    tracing::error!("読み直しに失敗しました(メモリ上の状態で続けます): {e:#}")
                }
            }
        }
    }
    tracing::info!("session actor: 全ハンドルが閉じたので終了します");
}

/// 計画の要求を処理する。曲のフォルダと違うフォルダの計画を開いていたら開き直す。
/// 計画が変わったら、新しい計画の版を返す
fn handle_plan(plans: &mut Option<Plans>, dir: &std::path::Path, req: PlanRequest) -> Option<u64> {
    if plans.as_ref().is_some_and(|p| p.store.dir() != dir) {
        *plans = None;
    }
    if plans.is_none() {
        match PlanStore::open(dir) {
            Ok((store, session)) => {
                *plans = Some(Plans {
                    store,
                    session,
                    version: 0,
                })
            }
            Err(e) => {
                let msg = format!("計画を開けません: {e:#}");
                match req {
                    PlanRequest::Get { reply } => drop(reply.send(Err(msg))),
                    PlanRequest::Apply { reply, .. } => drop(reply.send(Err(msg))),
                    PlanRequest::Step { reply, .. } => drop(reply.send(Err(msg))),
                    PlanRequest::Revert { reply, .. } => drop(reply.send(Err(msg))),
                }
                return None;
            }
        }
    }
    let p = plans.as_mut()?;
    let mut changed = false;
    let save = |p: &mut Plans| {
        p.store
            .save(&p.session)
            .map_err(|e| format!("計画の保存に失敗しました(メモリ上は反映済み): {e:#}"))
    };
    match req {
        PlanRequest::Get { reply } => {
            let _ = reply.send(Ok(p.session.clone()));
        }
        PlanRequest::Apply {
            command,
            author,
            label,
            note,
            reply,
        } => {
            let result = match p.session.apply_with_note(*command, author, label, note) {
                Ok((id, _)) => {
                    changed = true;
                    save(p).map(|_| id)
                }
                Err(e) => Err(e.to_string()),
            };
            let _ = reply.send(result);
        }
        PlanRequest::Step { n, redo, reply } => {
            let mut done = 0;
            let mut err = None;
            for _ in 0..n {
                let r = if redo {
                    p.session.redo()
                } else {
                    p.session.undo()
                };
                match r {
                    Ok(Some(_)) => done += 1,
                    Ok(None) => break,
                    Err(e) => {
                        err = Some(e.to_string());
                        break;
                    }
                }
            }
            changed = done > 0;
            let result = match (err, done) {
                (Some(e), _) => Err(e),
                (None, 0) => Ok(0),
                (None, d) => save(p).map(|_| d),
            };
            let _ = reply.send(result);
        }
        PlanRequest::Revert {
            id,
            author,
            note,
            reply,
        } => {
            let result = match p.session.revert_with_note(&id, author, Some(note)) {
                Ok(r) => {
                    changed = true;
                    save(p).map(|_| (r.entry, r.conflicts))
                }
                Err(e) => Err(e.to_string()),
            };
            let _ = reply.send(result);
        }
    }
    if changed {
        p.version += 1;
        Some(p.version)
    } else {
        None
    }
}

fn version(session: &Session, store: &Store) -> usize {
    store.revision(session)
}

/// 変更後の保存・購読者への通知・レスポンス部品の組み立て。
fn mutated(
    session: &Session,
    store: &Store,
    events: &broadcast::Sender<ProjectChanged>,
    changes: Vec<Change>,
) -> Mutated {
    store.bump_revision(session);
    let save_error = store.save_after_change_snapshot(session).err().map(|e| {
        tracing::error!("保存に失敗しました: {e:#}");
        format!("{e:#}")
    });
    // 購読者ゼロは正常(MCP 単体起動時)なのでエラーは無視
    let _ = events.send(ProjectChanged {
        project_version: version(session, store),
        changes: changes.clone(),
        save_error: save_error.clone(),
        history_only: false,
        plans_version: None,
    });
    Mutated {
        changes,
        project_version: version(session, store),
        save_error,
        aftercare: None,
        locks: vec![],
    }
}

fn handle(
    session: &mut Session,
    store: &mut Store,
    events: &broadcast::Sender<ProjectChanged>,
    req: Request,
) {
    // 状態を変えうる要求の前に、読み取り用の複製を捨てる(変えたら作り直す)
    if !matches!(
        req,
        Request::GetProject { .. }
            | Request::GetEntries { .. }
            | Request::ProjectAt { .. }
            | Request::GetHistory { .. }
            | Request::ProjectDir { .. }
            | Request::Flush { .. }
    ) {
        store.clear_snapshot();
    }
    match req {
        Request::GetProject { reply } => {
            let _ = reply.send((store.snapshot(session), version(session, store)));
        }
        #[cfg(test)]
        Request::Panic { reply } => {
            // 途中まで壊したセッションを模す(読み直しで元に戻ることを確かめる)
            // (保存せずにメモリ上だけ変える)
            let _ = session.apply(
                Command::SetTitle {
                    title: "壊れた状態".into(),
                },
                Author::Human,
                "保存されない編集".to_owned(),
            );
            drop(reply);
            panic!("テスト用の panic");
        }
        Request::Apply {
            command,
            author,
            label,
            reply,
        } => {
            // 区間の置き換えは、ID の無い区間に今の区間の ID を引き継がせる(計画が区間を ID で指すため)
            let command = glaux_core::arrange::keep_section_ids(session.project(), *command);
            // AI の編集: 固定の音を守り、ノートが変わったクリップの指紋を同じ 1 件に記録する(手で直した所を後で見分ける)
            let (command, locks) = if matches!(author, Author::Ai { .. }) {
                let e = glaux_core::made::for_ai(session.project(), command);
                match e.command {
                    Some(c) => (c, e.locks),
                    None => {
                        let what: Vec<String> = e.locks.iter().map(|l| l.what.clone()).collect();
                        let _ = reply.send(Err(CoreError::Locked(what.join(" / "))));
                        return;
                    }
                }
            } else {
                (command, vec![])
            };
            let result = session.apply(command, author, label).map(|(id, changes)| {
                // 履歴が長くなりすぎたら切り詰める(保存は mutated 内の全書き換えで行われる)
                if let Err(e) = store.maybe_compact(session) {
                    tracing::warn!("履歴の compaction に失敗: {e:#}");
                }
                let mut m = mutated(session, store, events, changes);
                m.locks = locks;
                (id, m)
            });
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
            // 曲の中身は変わらないので保存しない(チェックポイントは保存の対象でもない)。版だけ進めて履歴の表示を更新させる
            store.bump_revision(session);
            let _ = events.send(ProjectChanged {
                project_version: version(session, store),
                changes: vec![],
                save_error: None,
                history_only: true,
                plans_version: None,
            });
            let _ = reply.send(Mutated {
                changes: vec![],
                project_version: version(session, store),
                save_error: None,
                aftercare: None,
                locks: vec![],
            });
        }
        Request::RevertTo { label, reply } => {
            let result = session
                .revert_to(&label)
                .map(|changes| mutated(session, store, events, changes));
            let _ = reply.send(result);
        }
        Request::RevertEntry { id, author, reply } => {
            let result = session.revert(&id, author).map(|r| {
                let m = mutated(session, store, events, r.changes);
                (r.entry, r.conflicts, m)
            });
            let _ = reply.send(result);
        }
        Request::GetEntries {
            since,
            limit,
            reply,
        } => {
            let applied = session.history().applied();
            let result = match &since {
                Some(id) => applied
                    .iter()
                    .position(|e| &e.id == id)
                    .map(|i| i + 1)
                    .ok_or_else(|| CoreError::EntryNotFound(id.clone())),
                None => Ok(0),
            }
            .map(|start| {
                let tail = &applied[start..];
                tail[tail.len().saturating_sub(limit)..].to_vec()
            });
            let _ = reply.send(result);
        }
        Request::ProjectAt { point, reply } => {
            let v = version(session, store);
            let result = session.resolve_point(&point).and_then(|at| {
                let back = session.history().applied().len() - at;
                Ok((session.project_at(at)?, session.project().clone(), v, back))
            });
            let _ = reply.send(result);
        }
        Request::GetHistory {
            author_kind,
            since,
            limit,
            reply,
        } => {
            let _ = reply.send(history_view(session, store, author_kind, since, limit));
        }
        Request::SwitchProject { dir, reply } => {
            if let Err(e) = store.flush() {
                tracing::error!("切り替えの前の保存に失敗しました: {e:#}");
            }
            let result = match Store::open_or_create(&dir) {
                Ok((mut new_store, new_session)) => {
                    new_store.enable_background_writes();
                    *store = new_store;
                    *session = new_session;
                    let version = version(session, store);
                    // 購読者(UI 再取得・エンジン再構築)に全体更新を促す
                    let _ = events.send(ProjectChanged {
                        project_version: version,
                        changes: vec![],
                        save_error: None,
                        history_only: false,
                        plans_version: None,
                    });
                    tracing::info!("プロジェクトを切り替えました: {dir}");
                    Ok((session.project().meta.title.clone(), version))
                }
                Err(e) => Err(format!("{e:#}")),
            };
            let _ = reply.send(result);
        }
        Request::ProjectDir { reply } => {
            let _ = reply.send(store.dir().to_string_lossy().into_owned());
        }
        Request::Flush { reply } => {
            let _ = reply.send(store.flush().map_err(|e| format!("{e:#}")));
        }
        // 計画の要求は actor_loop で先に処理している
        Request::Plan(_) => {}
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
                    // 自分の中や、別の曲のフォルダの中へは移さない
                    crate::store::check_project_parent(parent)?;
                }
                let previous = version(session, store);
                // 裏で書きかけの project.json を元の場所で済ませてから動かす
                store
                    .flush()
                    .map_err(|e| format!("移動の前の保存に失敗しました: {e:#}"))?;
                // 開いているロックファイルを含むフォルダは Windows では動かせない
                crate::store::release_lock(&from);
                if let Err(e) = move_dir(&from, &to) {
                    let _ = crate::store::acquire_lock(&from);
                    return Err(e);
                }
                match Store::open_or_create(&dest) {
                    Ok((mut new_store, new_session)) => {
                        new_store.enable_background_writes();
                        new_store.continue_revision_after(previous);
                        *store = new_store;
                        *session = new_session;
                        let version = version(session, store);
                        let _ = events.send(ProjectChanged {
                            project_version: version,
                            changes: vec![],
                            save_error: None,
                            history_only: false,
                            plans_version: None,
                        });
                        tracing::info!("プロジェクトを移動しました: {} → {dest}", from.display());
                        Ok((session.project().meta.title.clone(), version))
                    }
                    Err(e) => {
                        // 開き直しに失敗したら元の場所へ戻して被害を抑える
                        crate::store::release_lock(&to);
                        let _ = move_dir(&to, &from);
                        let _ = crate::store::acquire_lock(&from);
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
            project_version: version(session, store),
            save_error: None,
            aftercare: None,
            locks: vec![],
        }
    };
    Ok((done, m))
}

fn history_view(
    session: &Session,
    store: &Store,
    author_kind: Option<String>,
    since: Option<EntryId>,
    limit: Option<usize>,
) -> Result<HistoryPage, CoreError> {
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

    let matches = |e: &&HistoryEntry| match author_kind.as_deref() {
        None => true,
        Some("human") => matches!(e.author, Author::Human),
        Some("ai") => matches!(e.author, Author::Ai { .. }),
        Some("system") => matches!(e.author, Author::System),
        Some(_) => false,
    };
    let total = applied[start..].iter().filter(matches).count();
    // 最新側から limit 件だけ要約を作る(返却順は古い→新しい)。全件の要約を作ってから切ると、
    // 履歴が長いときに毎回重い
    let mut entries: Vec<EntrySummary> = applied[start..]
        .iter()
        .rev()
        .filter(matches)
        .take(limit.unwrap_or(usize::MAX))
        .map(EntrySummary::from)
        .collect();
    entries.reverse();
    Ok(HistoryPage {
        project_version: version(session, store),
        total,
        entries,
        redoable: session
            .history()
            .redoable()
            .iter()
            .take(20)
            .map(EntrySummary::from)
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::{Track, TrackId, TrackKind};

    #[tokio::test]
    async fn actor_survives_a_panic_and_reloads_the_saved_state() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("Song.glaux");
        let (store, session) = Store::open_or_create(dir.to_str().unwrap()).unwrap();
        let handle = SessionHandle::spawn(session, store);

        let tid = TrackId::new();
        let (_, applied) = handle
            .apply(
                Command::AddTrack {
                    track: Track::new(tid.clone(), "Bass", TrackKind::Midi),
                    index: None,
                },
                Author::Human,
                "トラック追加".into(),
            )
            .await
            .unwrap()
            .unwrap();
        let before = applied.project_version;

        let err = handle
            .request(|reply| Request::Panic { reply })
            .await
            .unwrap_err();
        assert_eq!(err, ACTOR_GONE);

        // アクターは動き続け、保存済みの状態(トラックあり・壊す前の題名)に戻っている
        let (project, version) = handle.get_project().await.unwrap();
        assert!(project.track(&tid).is_some());
        assert_eq!(project.meta.title, "Song");
        // 版数は読み直しても戻らない
        assert!(version > before, "{version} > {before}");
        // 以後の編集もできる
        handle.undo(1).await.unwrap().unwrap();
        let (project, _) = handle.get_project().await.unwrap();
        assert!(project.track(&tid).is_none());
    }

    #[tokio::test]
    async fn moving_a_song_into_itself_is_refused_and_leaves_nothing_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("Song.glaux");
        let (store, session) = Store::open_or_create(dir.to_str().unwrap()).unwrap();
        let handle = SessionHandle::spawn(session, store);
        // 自分の中へ(以前は複製が止まらず、同じ名前のフォルダが何百段も入れ子になった)
        let inner = dir.join("Song.glaux");
        let err = handle
            .move_project(inner.to_string_lossy().into_owned())
            .await
            .unwrap_err();
        assert!(err.contains("曲のフォルダ"), "{err}");
        assert!(!inner.exists());
        // 別の曲の中へも移さない
        let other = tmp.path().join("Other.glaux");
        crate::store::create_project(&other, "Other").unwrap();
        let err = handle
            .move_project(other.join("Song.glaux").to_string_lossy().into_owned())
            .await
            .unwrap_err();
        assert!(err.contains("曲のフォルダ"), "{err}");
        // 外へは移せる
        let outside = tmp.path().join("Moved.glaux");
        handle
            .move_project(outside.to_string_lossy().into_owned())
            .await
            .unwrap();
        assert!(outside.join("project.json").exists() && !dir.exists());
    }

    #[test]
    fn copying_a_folder_into_itself_stops() {
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("a");
        std::fs::create_dir_all(from.join("sub")).unwrap();
        std::fs::write(from.join("x.txt"), "x").unwrap();
        let to = from.join("copy");
        copy_dir_recursive(&from, &to).unwrap();
        assert!(to.join("x.txt").exists() && to.join("sub").is_dir());
        assert!(!to.join("copy").exists());
    }
}
