//! `MySong.glaux/` フォルダの読み書き。
//!
//! - `project.json`: 現在状態のスナップショット(人間が読める・Git で差分が取れる)
//! - `history.jsonl`: コマンドログ(1 行 1 エントリ)。空プロジェクトからの全履歴で、
//!   `Session::replay` すると `project.json` と一致するのが正常な状態。
//!
//! 起動時は `history.jsonl` からの再構築を試みる(過去セッションの履歴の上で
//! undo / revert ができる)。再構築結果が `project.json` と一致しないときは
//! `project.json` を正として採用し、既存の履歴は `history.jsonl.orphan` に退避する。
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
}

impl Store {
    /// プロジェクトフォルダを開く。無ければ新規作成する。
    pub fn open_or_create(dir: impl Into<PathBuf>) -> Result<(Store, Session)> {
        let store = Store {
            dir: dir.into(),
            saved_entries: Cell::new(0),
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

        let session = match store.try_replay(&project) {
            Some(session) => session,
            None => Session::new(project),
        };
        store.saved_entries.set(session.history().len());
        Ok((store, session))
    }

    /// `history.jsonl` から Session を再構築する。
    /// 履歴が無い・壊れている・`project.json` と一致しない場合は `None`。
    fn try_replay(&self, expected: &Project) -> Option<Session> {
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

        let entries = match History::entries_from_jsonl(&text) {
            Ok(e) => e,
            Err(e) => {
                orphan(&format!("パース失敗: {e}"));
                return None;
            }
        };

        // 起点: compaction 済みなら history.base.json、そうでなければ
        // 「メタ情報だけ引き継いだ空プロジェクト」からの全記録という前提
        let base = if base_path.exists() {
            match fs::read_to_string(&base_path)
                .ok()
                .and_then(|t| Project::from_json(&t).ok())
            {
                Some(b) => b,
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

        match Session::replay(base, entries) {
            Ok(session) if session.project() == expected => Some(session),
            Ok(_) => {
                orphan("再構築結果が project.json と一致しない");
                None
            }
            Err(e) => {
                orphan(&format!("リプレイ失敗: {e}"));
                None
            }
        }
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
                .and_then(|mut f| writeln!(f, "{line}"));
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
        let base_json = base
            .to_json()
            .context("history.base.json のシリアライズに失敗")?;
        write_atomic(&self.base_path(), base_json.as_bytes())?;
        // 捨てた分は記録として残す(失敗しても compaction 自体は成立させる)
        let archive = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("history.archive.jsonl"))
            .and_then(|mut f| {
                for e in &dropped {
                    let line = serde_json::to_string(e)
                        .map_err(std::io::Error::other)?;
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

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).with_context(|| format!("書き込み失敗: {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("rename 失敗: {}", path.display()))?;
    Ok(())
}
