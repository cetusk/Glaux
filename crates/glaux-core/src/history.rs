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
//!
//! 履歴は文書の型([`Document`])について汎用。既定は曲([`Project`] と [`Command`])で、
//! 旋律の計画([`crate::plan::PlanSet`] と [`crate::plan::PlanCommand`])も同じ仕組みで別の履歴を持つ。

use crate::apply::Change;
use crate::command::{Command, Target};
use crate::error::{CoreError, Result};
use crate::id::EntryId;
use crate::model::Project;
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;

/// 履歴に積めるコマンド。対象(revert の衝突判定に使う)を答える
pub trait HistoryCommand: Clone + PartialEq + Debug + Serialize + DeserializeOwned {
    type Target: Clone + Ord + Debug + Serialize + DeserializeOwned;
    fn targets(&self) -> BTreeSet<Self::Target>;
}

/// 履歴で管理する文書。コマンドを適用して、逆コマンドと変更の通知を返す。
/// 失敗したときは文書を変えない
pub trait Document: Clone + Debug + Default {
    type Command: HistoryCommand;
    type Change: Clone + Debug;
    fn apply_command(&mut self, cmd: &Self::Command) -> Result<(Self::Command, Vec<Self::Change>)>;
}

impl HistoryCommand for Command {
    type Target = Target;
    fn targets(&self) -> BTreeSet<Target> {
        Command::targets(self)
    }
}

impl Document for Project {
    type Command = Command;
    type Change = Change;
    fn apply_command(&mut self, cmd: &Command) -> Result<(Command, Vec<Change>)> {
        let a = self.apply(cmd)?;
        Ok((a.inverse, a.changes))
    }
}

/// 変更の経緯(コミットメッセージの本文)。件名は [`HistoryEntry::label`]
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct EntryNote {
    /// なぜ変えたか
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub why: String,
    /// きっかけ(作曲者の言葉・点検の指摘・聴き比べの結果)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<Trigger>,
    /// 変更の前後の測定(例: 区間の音域の幅 8 → 13 半音)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measures: Vec<Measure>,
    /// この計画の変更と一組の、曲の履歴の項目(案の採用など)。曲の側でその編集を取り消す・やり直す・
    /// 「この変更だけ取り消す」と、どの画面・道具からでも、計画の側の一組の変更も一緒に戻す(戻したことも計画の履歴に残る)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub song_entry: Option<EntryId>,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Trigger {
    /// "user"(作曲者の言葉)/ "finding"(点検の指摘)/ "listening"(聴き比べ)/ "other"
    pub kind: String,
    pub text: String,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Measure {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<f64>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Author {
    Human,
    Ai { model: String },
    System,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(bound = "")]
pub struct HistoryEntry<C: HistoryCommand = Command> {
    pub id: EntryId,
    pub author: Author,
    pub timestamp: DateTime<Utc>,
    pub label: String,
    pub forward: C,
    pub inverse: C,
    /// 触った対象。revert の衝突判定に使う。
    pub targets: BTreeSet<C::Target>,
    /// このエントリが `revert()` で作られたものなら、取り消した元エントリ
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverts: Option<EntryId>,
    /// 変更の経緯(計画の履歴では必ず持つ。曲の履歴では今は持たない)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<EntryNote>,
}

/// 履歴本体。`entries[..cursor]` が適用済み、`entries[cursor..]` が redo 可能。
#[derive(Clone, PartialEq, Debug)]
pub struct History<C: HistoryCommand = Command> {
    entries: Vec<HistoryEntry<C>>,
    cursor: usize,
    checkpoints: BTreeMap<String, usize>,
}

impl<C: HistoryCommand> Default for History<C> {
    fn default() -> Self {
        History {
            entries: Vec::new(),
            cursor: 0,
            checkpoints: BTreeMap::new(),
        }
    }
}

impl<C: HistoryCommand> History<C> {
    pub fn applied(&self) -> &[HistoryEntry<C>] {
        &self.entries[..self.cursor]
    }

    pub fn redoable(&self) -> &[HistoryEntry<C>] {
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

    pub fn entry(&self, id: &EntryId) -> Option<&HistoryEntry<C>> {
        self.applied().iter().find(|e| &e.id == id)
    }

    /// 作者で絞り込む(AI の作業一覧など)
    pub fn by_author<'a>(
        &'a self,
        pred: impl Fn(&Author) -> bool + 'a,
    ) -> impl Iterator<Item = &'a HistoryEntry<C>> + 'a {
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

    pub fn entries_from_jsonl(s: &str) -> serde_json::Result<Vec<HistoryEntry<C>>> {
        s.lines()
            .filter(|l| !l.trim().is_empty())
            .map(serde_json::from_str)
            .collect()
    }

    fn push(&mut self, entry: HistoryEntry<C>) {
        // 新しい操作で redo スタックと、その先のチェックポイントを捨てる
        self.entries.truncate(self.cursor);
        self.checkpoints.retain(|_, &mut idx| idx <= self.cursor);
        self.entries.push(entry);
        self.cursor += 1;
    }
}

/// `revert()` の結果。
#[derive(Clone, Debug)]
pub struct RevertResult<Ch = Change> {
    /// 新しく積まれた revert エントリ
    pub entry: EntryId,
    pub changes: Vec<Ch>,
    /// 取り消したエントリより後で、同じ対象を触っている適用済みエントリ。
    /// 空でなければ「意図しない結果」になっている可能性がある。
    pub conflicts: Vec<EntryId>,
}

/// 履歴の中の地点(聴き比べの「前」を指す)。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HistoryPoint {
    /// 名前を付けたチェックポイント
    Checkpoint(String),
    /// このエントリを適用する直前
    BeforeEntry(EntryId),
    /// 最新から n 個の編集を戻した所
    Back(usize),
}

/// プロジェクトと履歴を束ねた編集セッション。UI も MCP もこれを通す。
#[derive(Clone, Debug, Default)]
pub struct Session<D: Document = Project> {
    project: D,
    history: History<D::Command>,
}

type Cmd<D> = <D as Document>::Command;
type Ch<D> = <D as Document>::Change;

impl<D: Document> Session<D> {
    pub fn new(project: D) -> Self {
        Session {
            project,
            history: History::default(),
        }
    }

    /// 空プロジェクトに履歴を順に適用して再構築する。
    pub fn replay(base: D, entries: Vec<HistoryEntry<Cmd<D>>>) -> Result<Self> {
        let mut s = Session::new(base);
        for e in entries {
            let (inverse, _) = s.project.apply_command(&e.forward)?;
            s.history.push(HistoryEntry { inverse, ..e });
        }
        Ok(s)
    }

    /// 文書(曲なら [`Project`])
    pub fn project(&self) -> &D {
        &self.project
    }

    /// [`Session::project`] と同じ(計画など、曲以外の文書で読みやすい名前)
    pub fn doc(&self) -> &D {
        &self.project
    }

    pub fn history(&self) -> &History<D::Command> {
        &self.history
    }

    /// 履歴の地点を、適用済みのエントリ数(0 = 最初)に直す。
    pub fn resolve_point(&self, point: &HistoryPoint) -> Result<usize> {
        let cursor = self.history.cursor;
        match point {
            HistoryPoint::Checkpoint(label) => {
                let at = *self
                    .history
                    .checkpoints
                    .get(label)
                    .ok_or_else(|| CoreError::CheckpointNotFound(label.clone()))?;
                // redo 側にあるチェックポイントは、今の位置より先なので扱わない
                Ok(at.min(cursor))
            }
            HistoryPoint::BeforeEntry(id) => self
                .history
                .applied()
                .iter()
                .position(|e| &e.id == id)
                .ok_or_else(|| CoreError::EntryNotFound(id.clone())),
            HistoryPoint::Back(n) => Ok(cursor.saturating_sub(*n)),
        }
    }

    /// 履歴の地点(適用済みのエントリ数)でのプロジェクトを、今のセッションを変えずに作る。
    /// 聴き比べ(編集の前後の比較)に使う
    pub fn project_at(&self, at: usize) -> Result<D> {
        let mut p = self.project.clone();
        for e in self.history.applied()[at.min(self.history.cursor)..]
            .iter()
            .rev()
        {
            p.apply_command(&e.inverse)?;
        }
        Ok(p)
    }

    /// コマンドを適用して履歴に積む。
    pub fn apply(
        &mut self,
        cmd: Cmd<D>,
        author: Author,
        label: impl Into<String>,
    ) -> Result<(EntryId, Vec<Ch<D>>)> {
        self.apply_inner(cmd, author, label.into(), None, None)
    }

    /// 経緯(なぜ・きっかけ・前後の測定)付きで適用して履歴に積む
    pub fn apply_with_note(
        &mut self,
        cmd: Cmd<D>,
        author: Author,
        label: impl Into<String>,
        note: EntryNote,
    ) -> Result<(EntryId, Vec<Ch<D>>)> {
        self.apply_inner(cmd, author, label.into(), None, Some(note))
    }

    fn apply_inner(
        &mut self,
        cmd: Cmd<D>,
        author: Author,
        label: String,
        reverts: Option<EntryId>,
        note: Option<EntryNote>,
    ) -> Result<(EntryId, Vec<Ch<D>>)> {
        let (inverse, changes) = self.project.apply_command(&cmd)?;
        let id = EntryId::new();
        self.history.push(HistoryEntry {
            id: id.clone(),
            author,
            timestamp: Utc::now(),
            label,
            targets: cmd.targets(),
            forward: cmd,
            inverse,
            reverts,
            note,
        });
        Ok((id, changes))
    }

    /// 履歴に積まずに当てる(redo の並びも捨てない)。曲の聴き方(ソロ・画面のボタンからのミュート)のように、
    /// 取り消し・やり直しの対象にしない変更に使う。呼び出し側は、ほかの編集の逆コマンドとぶつからない変更だけを渡すこと
    /// (曲の [`Command`] なら [`Command::is_listen_only`])
    pub fn apply_unrecorded(&mut self, cmd: &Cmd<D>) -> Result<Vec<Ch<D>>> {
        let (_, changes) = self.project.apply_command(cmd)?;
        Ok(changes)
    }

    pub fn can_undo(&self) -> bool {
        self.history.cursor > 0
    }

    pub fn can_redo(&self) -> bool {
        self.history.cursor < self.history.entries.len()
    }

    pub fn undo(&mut self) -> Result<Option<Vec<Ch<D>>>> {
        if !self.can_undo() {
            return Ok(None);
        }
        let idx = self.history.cursor - 1;
        let (_, changes) = self
            .project
            .apply_command(&self.history.entries[idx].inverse)?;
        self.history.cursor = idx;
        Ok(Some(changes))
    }

    pub fn redo(&mut self) -> Result<Option<Vec<Ch<D>>>> {
        if !self.can_redo() {
            return Ok(None);
        }
        let idx = self.history.cursor;
        let (inverse, changes) = self
            .project
            .apply_command(&self.history.entries[idx].forward)?;
        // 逆コマンドは再計算したものに更新しておく(内容は同じはず)
        self.history.entries[idx].inverse = inverse;
        self.history.cursor = idx + 1;
        Ok(Some(changes))
    }

    /// 現在位置に名前を付ける。
    pub fn checkpoint(&mut self, label: impl Into<String>) {
        self.history
            .checkpoints
            .insert(label.into(), self.history.cursor);
    }

    /// チェックポイントまで undo を繰り返す。
    pub fn revert_to(&mut self, label: &str) -> Result<Vec<Ch<D>>> {
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
    pub fn revert(&mut self, id: &EntryId, author: Author) -> Result<RevertResult<Ch<D>>> {
        self.revert_with_note(id, author, None)
    }

    /// 経緯付きの revert(計画の履歴で「なぜ戻したか」を残す)
    pub fn revert_with_note(
        &mut self,
        id: &EntryId,
        author: Author,
        note: Option<EntryNote>,
    ) -> Result<RevertResult<Ch<D>>> {
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
            note,
        )?;
        Ok(RevertResult {
            entry: new_id,
            changes,
            conflicts,
        })
    }

    /// 履歴の compaction: 古いエントリを捨てて直近 `keep` 件だけ残す。
    ///
    /// 戻り値は (残した履歴の起点となるプロジェクト状態, 捨てたエントリ)。
    /// 起点は「現在状態に、残すエントリの逆コマンドを新しい順に適用したもの」で、
    /// `Session::replay(起点, 残したエントリ)` が現在状態を再現する。
    /// redo スタックがある間・件数が `keep` 以下のときは何もしない(Ok(None))。
    /// 逆コマンドが適用できない(履歴が壊れている)場合はエラーで、状態は変えない。
    #[allow(clippy::type_complexity)]
    pub fn compact(&mut self, keep: usize) -> Result<Option<(D, Vec<HistoryEntry<Cmd<D>>>)>> {
        let n = self.history.entries.len();
        if self.history.cursor != n || n <= keep {
            return Ok(None);
        }
        let drop = n - keep;
        let mut base = self.project.clone();
        for e in self.history.entries[drop..].iter().rev() {
            base.apply_command(&e.inverse)?;
        }
        let dropped: Vec<HistoryEntry<Cmd<D>>> = self.history.entries.drain(..drop).collect();
        self.history.cursor -= drop;
        self.history.checkpoints.retain(|_, idx| *idx >= drop);
        for idx in self.history.checkpoints.values_mut() {
            *idx -= drop;
        }
        Ok(Some((base, dropped)))
    }
}
