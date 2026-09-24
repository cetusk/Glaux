//! `MySong.glaux/` フォルダの読み書き。
//!
//! - `project.json`: 現在状態のスナップショット(人間が読める・Git で差分が取れる)
//! - `history.jsonl`: コマンドログ(1 行 1 エントリ)。空プロジェクトからの全履歴で、
//!   `Session::replay` すると `project.json` と一致するのが正常な状態。
//!
//! 起動時は `history.jsonl` からの再構築を試みる(過去セッションの履歴の上で
//! undo / revert ができる)。保存の途中で落ちたときの食い違い(末尾の切れた行、
//! 履歴が 1〜2 手先にある、compaction の途中)は直して読み込む(`try_replay`)。
//! それでも再構築結果が `project.json` と一致しないときは `project.json` を正として採用し、
//! 既存の履歴は `history.jsonl.orphan` に退避する。
//!
//! 書き込みは一時ファイル + fsync + rename(追記は fsync)。保存の順序は、追記では
//! 「履歴 → project.json」、全書き換えでは「project.json → 履歴」なので、どこで落ちても
//! 履歴は project.json と同じか、数手先にある。
//!
//! 履歴が [`COMPACT_AT`] 件を超えたら compaction する: 直近 [`COMPACT_KEEP`] 件だけ
//! `history.jsonl` に残し、その起点となる状態を `history.base.json` に書く
//! (再構築は base + history)。捨てた分は `history.archive.jsonl` に追記して
//! 記録としては残す(undo の対象からは外れる)。

use anyhow::{Context, Result};
use glaux_core::{History, Project, Session};
use std::cell::Cell;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// この件数を超えたら履歴を compaction する
pub const COMPACT_AT: usize = 3000;
/// compaction 後に残す直近の件数(= 起動後に undo できる上限)
pub const COMPACT_KEEP: usize = 1500;

pub struct Store {
    dir: PathBuf,
    /// `history.jsonl` に書き込み済みの適用エントリ数。
    /// 追記高速パス(apply 1 回 = 1 行 append)の判定に使う。
    saved_entries: Cell<usize>,
    /// 版数(`project_version`)。編集・undo・redo のたびに増え、undo でも戻らない
    /// (AI が「自分の把握は古いか」を判断するため)。開いた時点の履歴の件数から始める
    revision: Cell<Option<usize>>,
}

impl Store {
    /// プロジェクトフォルダを開く。無ければ新規作成する。
    pub fn open_or_create(dir: impl Into<PathBuf>) -> Result<(Store, Session)> {
        let (store, session) = Self::open_or_create_inner(dir.into())?;
        // 版数は開いた時点の履歴の件数から始める
        store.revision.set(Some(session.history().len()));
        Ok((store, session))
    }

    fn open_or_create_inner(dir: PathBuf) -> Result<(Store, Session)> {
        fs::create_dir_all(&dir)
            .with_context(|| format!("プロジェクトフォルダを作成できません: {}", dir.display()))?;
        acquire_lock(&dir)?;
        let store = Store {
            dir,
            saved_entries: Cell::new(0),
            revision: Cell::new(None),
        };
        let project_path = store.project_path();

        if !project_path.exists() {
            fs::create_dir_all(&store.dir).with_context(|| {
                format!(
                    "プロジェクトフォルダを作成できません: {}",
                    store.dir.display()
                )
            })?;
            let title = store
                .dir
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Untitled".to_owned());
            let session = Session::new(Project::new(title));
            store.save(&session)?;
            tracing::info!("新規プロジェクトを作成しました: {}", store.dir.display());
            return Ok((store, session));
        }

        let json = fs::read_to_string(&project_path)
            .with_context(|| format!("project.json を読めません: {}", project_path.display()))?;
        let project = Project::from_json(&json).context("project.json のパースに失敗")?;

        for issue in project.validate() {
            tracing::warn!("project.json の検査: {issue:?}");
        }

        let session = match store.try_replay(&project) {
            Some((session, repaired)) => {
                if repaired {
                    // 履歴の末尾の食い違いを直したので、履歴を今の状態で書き直す
                    store.save(&session)?;
                }
                session
            }
            None => Session::new(project),
        };
        store.saved_entries.set(session.history().len());
        Ok((store, session))
    }

    /// `history.jsonl` から Session を再構築する。
    /// 履歴が無い・壊れている・`project.json` と一致しない場合は `None`。
    ///
    /// 保存の途中で落ちた場合の食い違いは直す(戻り値の bool = 直したか):
    /// - 末尾の行が途中で切れている → その行を捨てる
    /// - 履歴が `project.json` より最大 [`RECOVER_STEPS`] 手ぶん先にある(追記の直後、または
    ///   undo の全書き換えの途中で落ちた)→ その手を undo した状態で一致させ、redo できる形で残す
    /// - compaction の途中で落ちて、履歴に起点より前のエントリが残っている → それを飛ばす
    fn try_replay(&self, expected: &Project) -> Option<(Session, bool)> {
        let history_path = self.history_path();
        let text = fs::read_to_string(&history_path).ok()?;
        if text.trim().is_empty() {
            return None;
        }

        let base_path = self.base_path();
        let orphan = |reason: &str| {
            tracing::warn!(
                "history.jsonl を再構築に使えません({reason})。history.jsonl.orphan に退避します"
            );
            let _ = fs::rename(&history_path, self.dir.join("history.jsonl.orphan"));
            if base_path.exists() {
                let _ = fs::rename(&base_path, self.dir.join("history.base.json.orphan"));
            }
        };

        let mut repaired = false;
        let mut entries = match History::entries_from_jsonl(&text) {
            Ok(e) => e,
            Err(e) => {
                // 末尾の 1 行だけが壊れている(書き込みの途中で落ちた)なら、その行を捨てる
                let body = text.trim_end_matches('\n');
                match body
                    .rfind('\n')
                    .map(|i| History::entries_from_jsonl(&body[..i]))
                {
                    Some(Ok(e)) => {
                        tracing::warn!("history.jsonl の末尾の壊れた行を捨てました");
                        repaired = true;
                        e
                    }
                    _ => {
                        orphan(&format!("パース失敗: {e}"));
                        return None;
                    }
                }
            }
        };

        // 起点: compaction 済みなら history.base.json、そうでなければ
        // 「メタ情報だけ引き継いだ空プロジェクト」からの全記録という前提
        let base = if base_path.exists() {
            match fs::read_to_string(&base_path)
                .ok()
                .and_then(|t| read_base(&t))
            {
                Some((b, first)) => {
                    // 起点より前のエントリ(compaction で起点に畳んだもの)が残っていれば飛ばす
                    if let Some(first) = first {
                        match entries.iter().position(|e| e.id.to_string() == first) {
                            Some(0) | None => {}
                            Some(k) => {
                                tracing::warn!(
                                    "compaction 済みのエントリ {k} 件が残っていたので飛ばします"
                                );
                                entries.drain(..k);
                                repaired = true;
                            }
                        }
                    }
                    b
                }
                None => {
                    orphan("history.base.json を読めない");
                    return None;
                }
            }
        } else {
            let mut base = Project::new(expected.meta.title.clone());
            base.meta = expected.meta.clone();
            base
        };

        let mut session = match Session::replay(base, entries) {
            Ok(s) => s,
            Err(e) => {
                orphan(&format!("リプレイ失敗: {e}"));
                return None;
            }
        };
        for step in 0..=RECOVER_STEPS {
            if session.project() == expected {
                if step > 0 {
                    tracing::warn!(
                        "履歴が project.json より {step} 手先にありました(保存の途中で落ちた)。\
                         その手は「やり直し」で戻せます"
                    );
                    repaired = true;
                }
                return Some((session, repaired));
            }
            if step == RECOVER_STEPS || !matches!(session.undo(), Ok(Some(_))) {
                break;
            }
        }
        orphan("再構築結果が project.json と一致しない");
        None
    }

    /// `project.json` と `history.jsonl` を保存する(temp + rename で原子的に)。
    pub fn save(&self, session: &Session) -> Result<()> {
        let project_json = session
            .project()
            .to_json()
            .context("project.json のシリアライズに失敗")?;
        let history_jsonl = session
            .history()
            .to_jsonl()
            .context("history.jsonl のシリアライズに失敗")?;
        write_atomic(&self.project_path(), project_json.as_bytes())?;
        write_atomic(&self.history_path(), history_jsonl.as_bytes())?;
        self.saved_entries.set(session.history().len());
        Ok(())
    }

    /// 変更後の保存。可能なら `history.jsonl` へ追記だけで済ませる高速パス。
    ///
    /// - apply 直後(エントリが 1 つ増えただけ)→ 末尾 1 行を append
    /// - エントリ数が変わらない(checkpoint 等)→ project.json のみ
    /// - それ以外(undo / redo / revert_to / 再構築)→ 全書き換え
    ///
    /// 履歴が長くなると全書き換えは O(履歴長) なので、編集のたびに払うのを避ける。
    pub fn save_after_change(&self, session: &Session) -> Result<()> {
        let n = session.history().len();
        let saved = self.saved_entries.get();

        if n == saved + 1 {
            let last = session
                .history()
                .applied()
                .last()
                .expect("len == saved+1 なので必ずある");
            let line =
                serde_json::to_string(last).context("history エントリのシリアライズに失敗")?;
            let append = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.history_path())
                .and_then(|mut f| {
                    writeln!(f, "{line}")?;
                    f.sync_data()
                });
            match append {
                Ok(()) => {
                    self.saved_entries.set(n);
                    let project_json = session
                        .project()
                        .to_json()
                        .context("project.json のシリアライズに失敗")?;
                    write_atomic(&self.project_path(), project_json.as_bytes())?;
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!("history.jsonl への追記に失敗({e})。全書き換えにフォールバック");
                }
            }
        } else if n == saved {
            let project_json = session
                .project()
                .to_json()
                .context("project.json のシリアライズに失敗")?;
            write_atomic(&self.project_path(), project_json.as_bytes())?;
            return Ok(());
        }

        self.save(session)
    }

    /// 履歴が長くなりすぎていれば compaction する(既定のしきい値)。
    /// 戻り値は compaction したかどうか。
    pub fn maybe_compact(&self, session: &mut Session) -> Result<bool> {
        self.maybe_compact_with(session, COMPACT_AT, COMPACT_KEEP)
    }

    /// しきい値を指定して compaction する(テスト用にも公開)。
    /// 起点を `history.base.json` に書いてから履歴を全書き換えし、
    /// 捨てたエントリは `history.archive.jsonl` に追記する。
    pub fn maybe_compact_with(
        &self,
        session: &mut Session,
        at: usize,
        keep: usize,
    ) -> Result<bool> {
        if session.history().len() <= at {
            return Ok(false);
        }
        let Some((base, dropped)) = session.compact(keep).context("履歴の compaction に失敗")?
        else {
            return Ok(false);
        };
        // 起点には「最初に残すエントリ」の ID も書く(履歴を書き直す前に落ちても、
        // 起点に畳んだエントリを二重に適用しないように)
        let first = session
            .history()
            .applied()
            .first()
            .map(|e| e.id.to_string());
        let base_json = write_base(&base, first.as_deref())?;
        write_atomic(&self.base_path(), base_json.as_bytes())?;
        // 捨てた分は記録として残す(失敗しても compaction 自体は成立させる)
        let archive = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("history.archive.jsonl"))
            .and_then(|mut f| {
                for e in &dropped {
                    let line = serde_json::to_string(e).map_err(std::io::Error::other)?;
                    writeln!(f, "{line}")?;
                }
                Ok(())
            });
        if let Err(e) = archive {
            tracing::warn!("history.archive.jsonl への追記に失敗: {e}");
        }
        self.save(session)?;
        tracing::info!(
            "履歴を compaction しました({} 件を退避、{} 件を保持)",
            dropped.len(),
            session.history().len()
        );
        Ok(true)
    }

    /// 今の版数
    pub fn revision(&self, session: &Session) -> usize {
        self.revision.get().unwrap_or(session.history().len())
    }

    /// 変更のたびに呼ぶ(版数を 1 進める)
    pub fn bump_revision(&self, session: &Session) -> usize {
        let r = self.revision(session) + 1;
        self.revision.set(Some(r));
        r
    }

    /// 同じプロジェクトを開き直したとき(移動・読み直し)に、版数を前より後ろから続ける
    pub fn continue_revision_after(&self, previous: usize) {
        self.revision.set(Some(previous + 1));
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn base_path(&self) -> PathBuf {
        self.dir.join("history.base.json")
    }

    fn project_path(&self) -> PathBuf {
        self.dir.join("project.json")
    }

    fn history_path(&self) -> PathBuf {
        self.dir.join("history.jsonl")
    }
}

// ---- 二重に開くことの防止 -----------------------------------------------------

/// このプロセスが開いているプロジェクトのロック(フォルダ → ロックしたファイル)。
/// 同じプロセスが開き直す(切り替えて戻る・読み直す)ときは使い回す。
/// ロックはプロセスが終わるか [`release_lock`] で外れる
static LOCKS: std::sync::Mutex<Vec<(PathBuf, fs::File)>> = std::sync::Mutex::new(Vec::new());

/// プロジェクトフォルダのロックファイル
pub const LOCK_FILE: &str = ".glaux.lock";

fn lock_key(dir: &Path) -> PathBuf {
    fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())
}

/// 同じ曲を別のプロセス(アプリと stdio の MCP サーバーなど)で同時に開くと、後から保存した側が
/// 相手の編集を上書きしてしまう。OS の排他ロックで、別のプロセスが開いていれば断る
pub fn acquire_lock(dir: &Path) -> Result<()> {
    let key = lock_key(dir);
    let mut locks = LOCKS.lock().unwrap_or_else(|e| e.into_inner());
    if locks.iter().any(|(d, _)| d == &key) {
        return Ok(());
    }
    let path = dir.join(LOCK_FILE);
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("ロックファイルを作れません: {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => anyhow::bail!(
            "このプロジェクトは別の Glaux(アプリか MCP サーバー)で開かれています。\
             同じ曲を 2 か所で同時に開くと編集が失われるので、もう一方を閉じてください: {}",
            dir.display()
        ),
        // ロックに対応しないファイルシステム(一部のネットワークドライブ)では、防げないが開く
        Err(fs::TryLockError::Error(e)) => {
            tracing::warn!("ロックできません({e})。二重に開くことは防げません")
        }
    }
    locks.push((key, file));
    Ok(())
}

/// ロックを外す(フォルダを移動する前など。Windows では開いているファイルを含むフォルダは動かせない)
pub fn release_lock(dir: &Path) {
    let key = lock_key(dir);
    let mut locks = LOCKS.lock().unwrap_or_else(|e| e.into_inner());
    locks.retain(|(d, _)| d != &key);
}

/// 読み込み時に食い違いを直す、履歴の末尾からの手数の上限。
/// 保存は 1 回の変更ごとなので、落ちて食い違うのは 1 手ぶんまでのはず
const RECOVER_STEPS: usize = 2;

/// `history.base.json` の中身(起点の Project と、最初に残すエントリの ID)。
/// 以前は Project そのものを書いていたので、その形式も読む
#[derive(serde::Serialize, serde::Deserialize)]
struct BaseFile {
    first_entry: Option<String>,
    project: serde_json::Value,
}

fn write_base(base: &Project, first_entry: Option<&str>) -> Result<String> {
    let project: serde_json::Value = serde_json::from_str(
        &base
            .to_json()
            .context("history.base.json のシリアライズに失敗")?,
    )?;
    Ok(serde_json::to_string(&BaseFile {
        first_entry: first_entry.map(str::to_owned),
        project,
    })?)
}

fn read_base(text: &str) -> Option<(Project, Option<String>)> {
    if let Ok(b) = serde_json::from_str::<BaseFile>(text) {
        let project = Project::from_json(&b.project.to_string()).ok()?;
        return Some((project, b.first_entry));
    }
    Project::from_json(text).ok().map(|p| (p, None))
}

/// 一時ファイルに書いて確定(fsync)させてから rename する。
/// 一時ファイル名はプロセスごとに分ける(同じ曲を別プロセスで開いたときに混ざらないように)。
/// Windows では OneDrive やウイルス対策がファイルを掴んでいて rename が一時的に失敗しがちなので、
/// 少し待って何度か試す
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    {
        let mut f =
            fs::File::create(&tmp).with_context(|| format!("書き込み失敗: {}", tmp.display()))?;
        f.write_all(bytes)
            .and_then(|_| f.sync_all())
            .with_context(|| format!("書き込み失敗: {}", tmp.display()))?;
    }
    let mut attempt = 0;
    loop {
        match fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(e) if attempt < 5 => {
                attempt += 1;
                tracing::warn!("rename に失敗({e})。再試行します({attempt}/5)");
                std::thread::sleep(std::time::Duration::from_millis(40 * attempt));
            }
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                return Err(e).with_context(|| format!("rename 失敗: {}", path.display()));
            }
        }
    }
}
