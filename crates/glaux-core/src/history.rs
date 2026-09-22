//! Git ライクな履歴。
//!
//! - `undo` / `redo`: 通常の直前操作の取り消し(`git reset` 相当、ただし redo 可能)
//! - `checkpoint` / `revert_to`: 名前を付けた地点に戻る。AI の試行錯誤の足場。
//! - `revert(entry)`: **途中の** エントリだけを取り消す。逆コマンドを新しいエントリとして
//!   末尾に積む(`git revert` 相当)。後続エントリと対象が重なるときは衝突候補を返す。
//! - `replay`: 空プロジェクトに履歴を順に適用して再構築する。
//!
//! [`HistoryEntry`] は `author` を持つので「AI の変更だけ一覧して個別に却下」ができる。
//! 1 行 1 エントリの JSONL(`history.jsonl`)としてそのまま保存できる。

use crate::apply::Change;
use crate::command::{Command, Target};
use crate::error::{CoreError, Result};
use crate::id::EntryId;
use crate::model::Project;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Author {
    Human,
    Ai { model: String },
    System,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: EntryId,
    pub author: Author,
    pub timestamp: DateTime<Utc>,
    pub label: String,
    pub forward: Command,
    pub inverse: Command,
    /// 触った対象。revert の衝突判定に使う。
    pub targets: BTreeSet<Target>,
    /// このエントリが `revert()` で作られたものなら、取り消した元エントリ
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverts: Option<EntryId>,
}

/// 履歴本体。`entries[..cursor]` が適用済み、`entries[cursor..]` が redo 可能。
#[derive(Clone, PartialEq, Debug, Default)]
pub struct History {
    entries: Vec<HistoryEntry>,
    cursor: usize,
    checkpoints: BTreeMap<String, usize>,
}

impl History {
    pub fn applied(&self) -> &[HistoryEntry] {
        &self.entries[..self.cursor]
    }

    pub fn redoable(&self) -> &[HistoryEntry] {
        &self.entries[self.cursor..]
    }

    pub fn len(&self) -> usize {
        self.cursor
    }

    pub fn is_empty(&self) -> bool {
        self.cursor == 0
    }

    pub fn checkpoints(&self) -> &BTreeMap<String, usize> {
        &self.checkpoints
    }

    pub fn entry(&self, id: &EntryId) -> Option<&HistoryEntry> {
        self.applied().iter().find(|e| &e.id == id)
    }

    /// 作者で絞り込む(AI の作業一覧など)
    pub fn by_author<'a>(
        &'a self,
        pred: impl Fn(&Author) -> bool + 'a,
    ) -> impl Iterator<Item = &'a HistoryEntry> + 'a {
        self.applied().iter().filter(move |e| pred(&e.author))
    }

    /// 適用済みエントリを JSONL に書き出す(redo スタックは含めない)
    pub fn to_jsonl(&self) -> serde_json::Result<String> {
        let mut out = String::new();
        for e in self.applied() {
            out.push_str(&serde_json::to_string(e)?);
            out.push('\n');
        }
        Ok(out)
    }

    pub fn entries_from_jsonl(s: &str) -> serde_json::Result<Vec<HistoryEntry>> {
        s.lines()
            .filter(|l| !l.trim().is_empty())
            .map(serde_json::from_str)
            .collect()
    }

    fn push(&mut self, entry: HistoryEntry) {
        // 新しい操作で redo スタックと、その先のチェックポイントを捨てる
        self.entries.truncate(self.cursor);
        self.checkpoints.retain(|_, &mut idx| idx <= self.cursor);
        self.entries.push(entry);
        self.cursor += 1;
    }
}

/// `revert()` の結果。
#[derive(Clone, Debug)]
pub struct RevertResult {
    /// 新しく積まれた revert エントリ
    pub entry: EntryId,
    pub changes: Vec<Change>,
    /// 取り消したエントリより後で、同じ対象を触っている適用済みエントリ。
    /// 空でなければ「意図しない結果」になっている可能性がある。
    pub conflicts: Vec<EntryId>,
}

/// プロジェクトと履歴を束ねた編集セッション。UI も MCP もこれを通す。
#[derive(Clone, Debug, Default)]
pub struct Session {
    project: Project,
    history: History,
}

impl Session {
    pub fn new(project: Project) -> Self {
        Session {
            project,
            history: History::default(),
        }
    }

    /// 空プロジェクトに履歴を順に適用して再構築する。
    pub fn replay(base: Project, entries: Vec<HistoryEntry>) -> Result<Self> {
        let mut s = Session::new(base);
        for e in entries {
            let applied = s.project.apply(&e.forward)?;
            s.history.push(HistoryEntry {
                inverse: applied.inverse,
                ..e
            });
        }
        Ok(s)
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    /// コマンドを適用して履歴に積む。
    pub fn apply(
        &mut self,
        cmd: Command,
        author: Author,
        label: impl Into<String>,
    ) -> Result<(EntryId, Vec<Change>)> {
        self.apply_inner(cmd, author, label.into(), None)
    }

    fn apply_inner(
        &mut self,
        cmd: Command,
        author: Author,
        label: String,
        reverts: Option<EntryId>,
    ) -> Result<(EntryId, Vec<Change>)> {
        let applied = self.project.apply(&cmd)?;
        let id = EntryId::new();
        self.history.push(HistoryEntry {
            id: id.clone(),
            author,
            timestamp: Utc::now(),
            label,
            targets: cmd.targets(),
            forward: cmd,
            inverse: applied.inverse,
            reverts,
        });
        Ok((id, applied.changes))
    }

    pub fn can_undo(&self) -> bool {
        self.history.cursor > 0
    }

    pub fn can_redo(&self) -> bool {
        self.history.cursor < self.history.entries.len()
    }

    pub fn undo(&mut self) -> Result<Option<Vec<Change>>> {
        if !self.can_undo() {
            return Ok(None);
        }
        let idx = self.history.cursor - 1;
        let applied = self.project.apply(&self.history.entries[idx].inverse)?;
        self.history.cursor = idx;
        Ok(Some(applied.changes))
    }

    pub fn redo(&mut self) -> Result<Option<Vec<Change>>> {
        if !self.can_redo() {
            return Ok(None);
        }
        let idx = self.history.cursor;
        let applied = self.project.apply(&self.history.entries[idx].forward)?;
        // 逆コマンドは再計算したものに更新しておく(内容は同じはず)
        self.history.entries[idx].inverse = applied.inverse;
        self.history.cursor = idx + 1;
        Ok(Some(applied.changes))
    }

    /// 現在位置に名前を付ける。
    pub fn checkpoint(&mut self, label: impl Into<String>) {
        self.history
            .checkpoints
            .insert(label.into(), self.history.cursor);
    }

    /// チェックポイントまで undo を繰り返す。
    pub fn revert_to(&mut self, label: &str) -> Result<Vec<Change>> {
        let target = *self
            .history
            .checkpoints
            .get(label)
            .ok_or_else(|| CoreError::CheckpointNotFound(label.to_owned()))?;
        let mut changes = Vec::new();
        while self.history.cursor > target {
            if let Some(c) = self.undo()? {
                changes.extend(c);
            }
        }
        Ok(changes)
    }

    /// 履歴の途中のエントリを取り消す(`git revert`)。
    /// 逆コマンドを新しいエントリとして積むので、取り消した事実も履歴に残る。
    pub fn revert(&mut self, id: &EntryId, author: Author) -> Result<RevertResult> {
        let idx = self
            .history
            .applied()
            .iter()
            .position(|e| &e.id == id)
            .ok_or_else(|| CoreError::EntryNotFound(id.clone()))?;
        let entry = self.history.entries[idx].clone();

        let conflicts: Vec<EntryId> = self.history.applied()[idx + 1..]
            .iter()
            .filter(|later| !later.targets.is_disjoint(&entry.targets))
            .map(|e| e.id.clone())
            .collect();

        let (new_id, changes) = self.apply_inner(
            entry.inverse,
            author,
            format!("revert: {}", entry.label),
            Some(id.clone()),
        )?;
        Ok(RevertResult {
            entry: new_id,
            changes,
            conflicts,
        })
    }
}
