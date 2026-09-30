//! `MySong.glaux/` フォルダの読み書き。
//!
//! - `project.json`: 現在状態のスナップショット(人間が読める・Git で差分が取れる)
//! - `history.jsonl`: コマンドログ(1 行 1 エントリ)。空プロジェクトからの全履歴で、
//!   `Session::replay` すると `project.json` と一致するのが正常な状態。
//!
//! 起動時は `history.jsonl` からの再構築を試みる(過去セッションの履歴の上で
//! undo / revert ができる)。保存の途中で落ちたときの食い違い(末尾の切れた行、
//! 履歴が数手先にある、compaction の途中)は直して読み込む(`try_replay`)。
//! それでも再構築結果が `project.json` と一致しないときは `project.json` を正として採用し、
//! 既存の履歴は `history.jsonl.orphan` に退避する。
//!
//! 書き込みは一時ファイル + fsync + rename(追記は fsync)。保存の順序は、追記では
//! 「履歴 → project.json」、全書き換えでは「project.json → 履歴」なので、どこで落ちても
//! 履歴は project.json と同じか、数手先にある。
//!
//! アプリ・MCP サーバー(アクター)から使うときは、`project.json` は専用のスレッドで最新の状態だけを書く
//! (編集の応答を書き込みで待たせない)。履歴への追記は応答の前に fsync するので、落ちたときに
//! project.json が数手遅れていても、読み込みで上のとおり直す。undo・全書き換え・compaction の前と
//! 終了時には、裏の書き込みを書き終えてから進める(履歴が project.json より遅れないように)。
//!
//! 履歴が [`COMPACT_AT`] 件を超えたら compaction する: 直近 [`COMPACT_KEEP`] 件だけ
//! `history.jsonl` に残し、その起点となる状態を `history.base.json` に書く
//! (再構築は base + history)。捨てた分は `history.archive.jsonl` に追記して
//! 記録としては残す(undo の対象からは外れる)。件数が少なくても、`history.jsonl` が
//! [`COMPACT_BYTES`] を超えたら同じように compaction する。

use anyhow::{Context, Result};
use glaux_core::{History, Project, Session};
use std::cell::{Cell, RefCell};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// この件数を超えたら履歴を compaction する
pub const COMPACT_AT: usize = 3000;
/// compaction 後に残す直近の件数(= 起動後に undo できる上限)
pub const COMPACT_KEEP: usize = 1500;
/// 件数に関わらず、`history.jsonl` がこの大きさを超えたら compaction する
/// (トラックやクリップを消した逆コマンドは中身を丸ごと持つので、大きな編集が続くと件数の割に大きくなる)。
/// 残すのは後ろからこの半分の大きさまで
pub const COMPACT_BYTES: u64 = 64 << 20;

pub struct Store {
    dir: PathBuf,
    /// `history.jsonl` に書き込み済みの適用エントリ数。
    /// 追記高速パス(apply 1 回 = 1 行 append)の判定に使う。
    saved_entries: Cell<usize>,
    /// 書き込み済みの各行の (エントリ ID, 行の終わりのバイト位置)。undo のときにファイルを切り詰めるだけで
    /// 済ませるのに使う(開いた直後など、分からなければ None で、そのときは全書き換え)
    saved_lines: RefCell<Option<Vec<(glaux_core::EntryId, u64)>>>,
    /// `project.json` を裏で書く係(アクターが使うときだけ。無ければその場で書く)
    writer: Option<ProjectWriter>,
    /// 今の状態の読み取り用の複製(変更のたびに 1 回だけ作り、読み手と裏の書き込みで共有する)
    snapshot: RefCell<Option<std::sync::Arc<Project>>>,
    /// 版数(`project_version`)。編集・undo・redo のたびに増え、undo でも戻らない
    /// (AI が「自分の把握は古いか」を判断するため)。開いた時点の履歴の件数から始める
    revision: Cell<Option<usize>>,
}

/// 曲名から、曲のフォルダ名(`.glaux` の前)を作る。曲名そのものは変えず、ファイル名としてだけ整える。
/// - 空白は `_` に(続く空白・`_` は 1 つに)。Windows で使えない記号(`\ / : * ? " < > |`)と制御文字も `_` に
/// - 日本語などの文字・数字・`-` `_` `.` `(` `)` `&` `+` `,` `'` はそのまま。先頭と末尾の `_` `.` は落とす
/// - Windows の予約名(CON・NUL・COM1 など)は末尾に `_`、空になったら `Untitled`、長すぎたら 80 文字で切る
pub fn folder_name(title: &str) -> String {
    let mut out = String::new();
    for c in title.trim().chars() {
        let keep = (c.is_alphanumeric() || "-_.()&+,'".contains(c)) && !c.is_control();
        let ch = if keep { c } else { '_' };
        if ch == '_' && out.ends_with('_') {
            continue;
        }
        out.push(ch);
    }
    let name: String = out
        .trim_matches(|c| c == '_' || c == '.')
        .chars()
        .take(80)
        .collect();
    let mut name = name.trim_end_matches(['_', '.']).to_owned();
    if name.is_empty() {
        return "Untitled".to_owned();
    }
    let upper = name.to_ascii_uppercase();
    let base = upper.split('.').next().unwrap_or("");
    let reserved = matches!(base, "CON" | "PRN" | "AUX" | "NUL")
        || (base.len() == 4
            && (base.starts_with("COM") || base.starts_with("LPT"))
            && base.as_bytes()[3].is_ascii_digit());
    if reserved {
        name.push('_');
    }
    name
}

/// `parent` の中で、まだ使われていない `<name>.glaux` を返す(あれば `<name>-2.glaux`、`-3` …)
pub fn unique_project_dir(parent: &Path, name: &str) -> PathBuf {
    let first = parent.join(format!("{name}.glaux"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|i| parent.join(format!("{name}-{i}.glaux")))
        .find(|p| !p.exists())
        .expect("空いている名前は必ず見つかる")
}

/// Glaux の曲のフォルダか(`project.json` の先頭に `"format": "glaux"` がある、または名前が `.glaux` で終わる)
pub fn is_project_folder(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    if dir
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("glaux"))
    {
        return true;
    }
    let mut head = [0u8; 256];
    let n = fs::File::open(dir.join("project.json"))
        .and_then(|mut f| std::io::Read::read(&mut f, &mut head))
        .unwrap_or(0);
    let head = String::from_utf8_lossy(&head[..n]);
    head.contains("\"format\"") && head.contains("\"glaux\"")
}

/// `dir`(まだ無くてもよい)自身か、その上のどこかが曲のフォルダなら、その曲のフォルダ。
/// 曲の中に別の曲を作ったり移したりすると入れ子になり、曲のフォルダを動かしたときに壊れるので、作る・移す前に断る
pub fn enclosing_project(dir: &Path) -> Option<PathBuf> {
    let dir = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    dir.ancestors()
        .find(|a| is_project_folder(a))
        .map(Path::to_path_buf)
}

/// 新しい曲を置けない場所なら、その理由(曲のフォルダの中)
pub fn check_project_parent(parent: &Path) -> std::result::Result<(), String> {
    match enclosing_project(parent) {
        Some(p) => Err(format!(
            "曲のフォルダ({})の中には、別の曲を作ったり移したりできません。曲のフォルダの外を選んでください",
            p.display()
        )),
        None => Ok(()),
    }
}

/// 新しい曲を、曲名を付けて作る(project.json と空の履歴)。開くのは呼び出し側
pub fn create_project(dir: &Path, title: &str) -> Result<()> {
    if dir.join("project.json").exists() {
        anyhow::bail!("既に存在します: {}", dir.display());
    }
    fs::create_dir_all(dir)
        .with_context(|| format!("プロジェクトフォルダを作成できません: {}", dir.display()))?;
    let store = Store {
        dir: dir.to_path_buf(),
        saved_entries: Cell::new(0),
        saved_lines: RefCell::new(None),
        writer: None,
        snapshot: RefCell::new(None),
        revision: Cell::new(None),
    };
    store.save(&Session::new(Project::new(title.trim())))
}

/// 中身のあるプロジェクト(デモ曲など)を、履歴が空の新しいプロジェクトとして `dir` に作る。
pub fn create_project_from(dir: &Path, project: Project) -> Result<()> {
    if dir.join("project.json").exists() {
        anyhow::bail!("既に存在します: {}", dir.display());
    }
    fs::create_dir_all(dir)
        .with_context(|| format!("プロジェクトフォルダを作成できません: {}", dir.display()))?;
    let store = Store {
        dir: dir.to_path_buf(),
        saved_entries: Cell::new(0),
        saved_lines: RefCell::new(None),
        writer: None,
        snapshot: RefCell::new(None),
        revision: Cell::new(None),
    };
    // 履歴は空の曲からではなく、この中身から始まる(起点を書かないと、開き直したときに再生できない)
    store.write_start(&project)?;
    store.save(&Session::new(project))
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
            saved_lines: RefCell::new(None),
            writer: None,
            snapshot: RefCell::new(None),
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

        // 同じ種類の指摘は 1 行にまとめる(ノートごとに 1 行ずつ出すと何百行にもなる)
        for (severity, line) in Project::summarize_issues(&project.validate()) {
            match severity {
                glaux_core::validate::Severity::Info => {
                    tracing::info!("project.json の検査: {line}")
                }
                _ => tracing::warn!("project.json の検査: {line}"),
            }
        }

        let session = match store.try_replay(&project) {
            Some((session, repaired)) => {
                if repaired {
                    // 履歴の末尾の食い違いを直したので、履歴を今の状態で書き直す
                    store.save(&session)?;
                }
                session
            }
            None => {
                // 履歴が無い(か使えずに退避した)ので、今の曲を起点に新しい履歴を始める。
                // 起点を書いておかないと、次に開いたとき空の曲から再生しようとして失敗し、また退避してしまう
                store.write_start(&project)?;
                Session::new(project)
            }
        };
        store.saved_entries.set(session.history().len());
        if store.saved_lines.borrow().is_none() {
            *store.saved_lines.borrow_mut() = store.scan_lines(&session);
        }
        Ok((store, session))
    }

    /// 読み込んだ `history.jsonl` の各行の終わりの位置を、適用済みのエントリと対応づける
    /// (行の数が合わなければ None。そのときの undo は全書き換えになる)
    fn scan_lines(&self, session: &Session) -> Option<Vec<(glaux_core::EntryId, u64)>> {
        let bytes = fs::read(self.history_path()).ok()?;
        let mut ends = Vec::new();
        let mut start = 0usize;
        for (i, b) in bytes.iter().enumerate() {
            if *b == b'\n' {
                if !bytes[start..i].iter().all(u8::is_ascii_whitespace) {
                    ends.push(i as u64 + 1);
                }
                start = i + 1;
            }
        }
        let applied = session.history().applied();
        (start == bytes.len() && ends.len() == applied.len()).then(|| {
            applied
                .iter()
                .zip(ends)
                .map(|(e, end)| (e.id.clone(), end))
                .collect()
        })
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
        let had_base = base_path.exists();
        let orphan = |reason: &str| {
            // 前の退避を上書きしない(history.jsonl.orphan、.orphan.2、…)
            let slot = (1..)
                .map(|k: u32| {
                    if k == 1 {
                        ".orphan".to_owned()
                    } else {
                        format!(".orphan.{k}")
                    }
                })
                .find(|suf| !self.dir.join(format!("history.jsonl{suf}")).exists())
                .unwrap_or_else(|| ".orphan".to_owned());
            tracing::warn!(
                "history.jsonl を再構築に使えません({reason})。history.jsonl{slot} に退避します"
            );
            let _ = fs::rename(&history_path, self.dir.join(format!("history.jsonl{slot}")));
            if base_path.exists() {
                let _ = fs::rename(
                    &base_path,
                    self.dir.join(format!("history.base.json{slot}")),
                );
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

        let replayed = Session::replay(base, entries.clone());
        let reason = match replayed {
            Ok(session) => match settle(session, expected) {
                Some((session, step)) => {
                    if step > 0 {
                        tracing::warn!(
                            "履歴が project.json より {step} 手先にありました(保存の途中で落ちた)。\
                             その手は「やり直し」で戻せます"
                        );
                        repaired = true;
                    }
                    return Some((session, repaired));
                }
                None => "再構築結果が project.json と一致しない".to_owned(),
            },
            Err(e) => format!("リプレイ失敗: {e}"),
        };
        // 起点が書かれていない(中身のある曲から始まった履歴。デモ曲・曲の写しなど)なら、今の project.json から
        // 各編集の逆コマンドを後ろから当てて起点を逆算し、そこから再生して一致すれば履歴を生かす(起点を書いて直す)
        if !had_base {
            if let Some((session, step, start)) = reverse_start(expected, &entries) {
                let first = entries.first().map(|e| e.id.to_string());
                match write_base(&start, first.as_deref())
                    .and_then(|b| write_atomic(&base_path, b.as_bytes()))
                {
                    Ok(()) => {
                        tracing::warn!(
                            "履歴の起点が無かったので、project.json から逆算して history.base.json に書きました"
                        );
                        return Some((session, repaired || step > 0));
                    }
                    Err(e) => tracing::warn!("history.base.json を書けません: {e:#}"),
                }
            }
        }
        orphan(&reason);
        None
    }

    /// 履歴の起点を書く(履歴が空のまま、この曲の中身から始まるとき)
    fn write_start(&self, project: &Project) -> Result<()> {
        let base_json = write_base(project, None)?;
        write_atomic(&self.base_path(), base_json.as_bytes())
    }

    /// `project.json` と `history.jsonl` を保存する(temp + rename で原子的に)。
    pub fn save(&self, session: &Session) -> Result<()> {
        // 裏で書きかけの project.json を先に済ませる(後から古い状態で上書きされないように)
        self.flush()?;
        let project_json = session
            .project()
            .to_json_compact()
            .context("project.json のシリアライズに失敗")?;
        // History::to_jsonl と同じ形(1 行 1 エントリ)。行の終わりの位置も覚える
        let mut history_jsonl = String::new();
        let mut lines = Vec::with_capacity(session.history().len());
        for e in session.history().applied() {
            history_jsonl
                .push_str(&serde_json::to_string(e).context("history.jsonl のシリアライズに失敗")?);
            history_jsonl.push('\n');
            lines.push((e.id.clone(), history_jsonl.len() as u64));
        }
        write_atomic(&self.project_path(), project_json.as_bytes())?;
        write_atomic(&self.history_path(), history_jsonl.as_bytes())?;
        self.saved_entries.set(session.history().len());
        *self.saved_lines.borrow_mut() = Some(lines);
        Ok(())
    }

    /// undo(適用済みが書き込み済みの先頭の一部に減った)なら、`history.jsonl` を切り詰めるだけで保存する。
    /// project.json を先に書く(途中で落ちても、履歴は project.json と同じか先にある)。できなければ false
    fn save_truncated(&self, session: &Session) -> Result<bool> {
        let n = session.history().len();
        let end = {
            let lines = self.saved_lines.borrow();
            let Some(lines) = lines.as_ref() else {
                return Ok(false);
            };
            let applied = session.history().applied();
            if n >= lines.len()
                || lines[..n]
                    .iter()
                    .zip(applied)
                    .any(|((id, _), e)| *id != e.id)
            {
                return Ok(false);
            }
            n.checked_sub(1).map_or(0, |i| lines[i].1)
        };
        self.flush()?;
        let project_json = session
            .project()
            .to_json_compact()
            .context("project.json のシリアライズに失敗")?;
        write_atomic(&self.project_path(), project_json.as_bytes())?;
        let cut = fs::OpenOptions::new()
            .write(true)
            .open(self.history_path())
            .and_then(|f| {
                f.set_len(end)?;
                f.sync_data()
            });
        if let Err(e) = cut {
            tracing::warn!("history.jsonl の切り詰めに失敗({e})。全書き換えにフォールバック");
            return Ok(false);
        }
        self.saved_entries.set(n);
        if let Some(lines) = self.saved_lines.borrow_mut().as_mut() {
            lines.truncate(n);
        }
        Ok(true)
    }

    /// 変更後の保存。可能なら `history.jsonl` へ追記だけで済ませる高速パス。
    ///
    /// - apply 直後(エントリが 1 つ増えただけ)→ 末尾 1 行を append
    /// - エントリ数が変わらない(checkpoint 等)→ project.json のみ
    /// - undo(書き込み済みの先頭の一部に減った)→ 切り詰めるだけ
    /// - それ以外(再構築など)→ 全書き換え
    ///
    /// 履歴が長くなると全書き換えは O(履歴長) なので、編集のたびに払うのを避ける。
    pub fn save_after_change(&self, session: &Session) -> Result<()> {
        self.save_after_change_shared(session, None)
    }

    /// [`Self::save_after_change`] の、今の状態の複製(`snapshot`)を渡せる版。裏で書く係があれば、
    /// 追記・チェックポイントの後の `project.json` はその複製を裏で書く(応答を待たせない。
    /// 正本は fsync 済みの履歴。落ちたときに project.json が遅れていても、読み込みで履歴から直す)
    pub fn save_after_change_shared(
        &self,
        session: &Session,
        snapshot: Option<&std::sync::Arc<Project>>,
    ) -> Result<()> {
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
                    if let Some(v) = self.saved_lines.borrow_mut().as_mut() {
                        let prev = v.last().map_or(0, |(_, end)| *end);
                        v.push((last.id.clone(), prev + line.len() as u64 + 1));
                    }
                    return self.write_project(session, snapshot);
                }
                Err(e) => {
                    tracing::warn!("history.jsonl への追記に失敗({e})。全書き換えにフォールバック");
                }
            }
        } else if n == saved {
            return self.write_project(session, snapshot);
        } else if n < saved && self.save_truncated(session)? {
            return Ok(());
        }

        self.save(session)
    }

    /// `project.json` を書く。裏で書く係と複製があれば裏へ渡し、無ければその場で書く。
    /// 裏で前に失敗した書き込みがあれば、そのエラーを返す(次の書き込みは渡してある)
    fn write_project(
        &self,
        session: &Session,
        snapshot: Option<&std::sync::Arc<Project>>,
    ) -> Result<()> {
        if let (Some(w), Some(snap)) = (&self.writer, snapshot) {
            w.enqueue(self.project_path(), snap.clone());
            return match w.take_error() {
                Some(e) => Err(anyhow::anyhow!(e)),
                None => Ok(()),
            };
        }
        self.flush()?;
        let project_json = session
            .project()
            .to_json_compact()
            .context("project.json のシリアライズに失敗")?;
        write_atomic(&self.project_path(), project_json.as_bytes())
    }

    /// 今の状態の読み取り用の複製。変わっていなければ前に作ったものを返す
    /// (変わったら [`Self::save_after_change_snapshot`] か [`Self::clear_snapshot`] で作り直させる)
    pub fn snapshot(&self, session: &Session) -> std::sync::Arc<Project> {
        self.snapshot
            .borrow_mut()
            .get_or_insert_with(|| std::sync::Arc::new(session.project().clone()))
            .clone()
    }

    /// 読み取り用の複製を捨てる(状態が変わったかもしれないとき)
    pub fn clear_snapshot(&self) {
        *self.snapshot.borrow_mut() = None;
    }

    /// 変更後の保存を、新しい読み取り用の複製を作って行う(複製は裏の書き込みと読み手で共有する)
    pub fn save_after_change_snapshot(&self, session: &Session) -> Result<()> {
        let snap = std::sync::Arc::new(session.project().clone());
        *self.snapshot.borrow_mut() = Some(snap.clone());
        self.save_after_change_shared(session, Some(&snap))
    }

    /// `project.json` を裏で書くようにする(アクターが使う)
    pub fn enable_background_writes(&mut self) {
        if self.writer.is_none() {
            self.writer = Some(ProjectWriter::spawn());
        }
    }

    /// 裏で書きかけの `project.json` を書き終えるまで待つ(フォルダの移動・読み直しの前など)。
    /// 裏での書き込みの失敗があれば返す
    pub fn flush(&self) -> Result<()> {
        match self.writer.as_ref().and_then(ProjectWriter::flush) {
            Some(e) => Err(anyhow::anyhow!(e)),
            None => Ok(()),
        }
    }

    /// 履歴が長くなりすぎていれば compaction する(既定のしきい値)。
    /// 戻り値は compaction したかどうか。
    pub fn maybe_compact(&self, session: &mut Session) -> Result<bool> {
        if let Some(keep) = self.keep_for_bytes(COMPACT_BYTES) {
            if keep < session.history().len() {
                return self.maybe_compact_with(session, keep, keep);
            }
        }
        self.maybe_compact_with(session, COMPACT_AT, COMPACT_KEEP)
    }

    /// `history.jsonl` が `limit` バイトを超えていれば、後ろから `limit / 2` バイトまでに収まる件数
    /// (1 件以上、[`COMPACT_KEEP`] 以下)。超えていない・大きさが分からなければ None
    fn keep_for_bytes(&self, limit: u64) -> Option<usize> {
        let lines = self.saved_lines.borrow();
        let lines = lines.as_ref()?;
        let total = lines.last()?.1;
        if total <= limit {
            return None;
        }
        // 終わりの位置が cut 以下の行(= 前の方)を捨てる
        let cut = total - limit / 2;
        let first_kept = lines.partition_point(|(_, end)| *end <= cut);
        Some((lines.len() - first_kept).clamp(1, COMPACT_KEEP))
    }

    /// 大きさのしきい値を指定して compaction する(テスト用)。
    #[doc(hidden)]
    pub fn maybe_compact_by_bytes(&self, session: &mut Session, limit: u64) -> Result<bool> {
        match self.keep_for_bytes(limit) {
            Some(keep) if keep < session.history().len() => {
                self.maybe_compact_with(session, keep, keep)
            }
            _ => Ok(false),
        }
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
/// 曲が別の Glaux(アプリか MCP サーバー)で開かれていて、ロックを取れない。
/// 起動時はこれを見分けて、終了せずに一時的な曲で立ち上げる
#[derive(Debug, thiserror::Error)]
#[error(
    "このプロジェクトは別の Glaux(アプリか MCP サーバー)で開かれています。\
     同じ曲を 2 か所で同時に開くと編集が失われるので、もう一方を閉じてください: {}",
    .0.display()
)]
pub struct ProjectLocked(pub PathBuf);

impl ProjectLocked {
    /// エラーの連なりのどこかが「別の Glaux で開かれている」か
    pub fn is(e: &anyhow::Error) -> bool {
        e.chain()
            .any(|c| c.downcast_ref::<ProjectLocked>().is_some())
    }
}

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
        Err(fs::TryLockError::WouldBlock) => return Err(ProjectLocked(dir.to_path_buf()).into()),
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
/// 履歴は変更ごとに書くが、`project.json` は裏で最新の状態だけを書くので、落ちたときには
/// 書いている間に進んだ数手ぶん遅れていることがある
const RECOVER_STEPS: usize = 32;

/// `project.json` を裏で書く係。最新の状態だけを書く(書いている間に来た古い状態は飛ばす)
struct ProjectWriter {
    shared: std::sync::Arc<(std::sync::Mutex<WriterState>, std::sync::Condvar)>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[derive(Default)]
struct WriterState {
    pending: Option<(PathBuf, std::sync::Arc<Project>)>,
    busy: bool,
    stop: bool,
    error: Option<String>,
}

impl ProjectWriter {
    fn spawn() -> ProjectWriter {
        let shared: std::sync::Arc<(std::sync::Mutex<WriterState>, std::sync::Condvar)> =
            Default::default();
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("glaux-project-writer".into())
            .spawn(move || {
                let (lock, cv) = &*s;
                loop {
                    let job = {
                        let mut st = lock.lock().unwrap_or_else(|e| e.into_inner());
                        while st.pending.is_none() && !st.stop {
                            st = cv.wait(st).unwrap_or_else(|e| e.into_inner());
                        }
                        let Some(job) = st.pending.take() else {
                            return; // 止める合図で、書くものも残っていない
                        };
                        st.busy = true;
                        job
                    };
                    let (path, project) = job;
                    let result = project
                        .to_json_compact()
                        .context("project.json のシリアライズに失敗")
                        .and_then(|json| write_atomic(&path, json.as_bytes()));
                    let mut st = lock.lock().unwrap_or_else(|e| e.into_inner());
                    st.busy = false;
                    if let Err(e) = result {
                        tracing::error!("project.json の保存に失敗しました: {e:#}");
                        st.error = Some(format!("{e:#}"));
                    }
                    cv.notify_all();
                }
            })
            .ok();
        ProjectWriter { shared, thread }
    }

    fn enqueue(&self, path: PathBuf, project: std::sync::Arc<Project>) {
        let (lock, cv) = &*self.shared;
        lock.lock().unwrap_or_else(|e| e.into_inner()).pending = Some((path, project));
        cv.notify_all();
    }

    fn take_error(&self) -> Option<String> {
        self.shared
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .error
            .take()
    }

    /// 書きかけ・未着手のものを書き終えるまで待つ。失敗があれば返す
    fn flush(&self) -> Option<String> {
        let (lock, cv) = &*self.shared;
        let mut st = lock.lock().unwrap_or_else(|e| e.into_inner());
        // 係のスレッドが起動できなかったときは、ここで書く
        if self.thread.is_none() {
            if let Some((path, project)) = st.pending.take() {
                if let Err(e) = project
                    .to_json_compact()
                    .context("project.json のシリアライズに失敗")
                    .and_then(|json| write_atomic(&path, json.as_bytes()))
                {
                    st.error = Some(format!("{e:#}"));
                }
            }
        }
        while st.pending.is_some() || st.busy {
            st = cv.wait(st).unwrap_or_else(|e| e.into_inner());
        }
        st.error.take()
    }
}

impl Drop for ProjectWriter {
    fn drop(&mut self) {
        // 残っているものを書き終えてから止める
        {
            let (lock, cv) = &*self.shared;
            lock.lock().unwrap_or_else(|e| e.into_inner()).stop = true;
            cv.notify_all();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// `history.base.json` の中身(起点の Project と、最初に残すエントリの ID)。
/// 以前は Project そのものを書いていたので、その形式も読む
#[derive(serde::Serialize, serde::Deserialize)]
struct BaseFile {
    first_entry: Option<String>,
    project: serde_json::Value,
}

/// 再生した結果を project.json に合わせる: 履歴が最大 [`RECOVER_STEPS`] 手先にあるなら、その手を undo して
/// 一致させる(redo できる形で残す)。一致した手数を返す
fn settle(mut session: Session, expected: &Project) -> Option<(Session, usize)> {
    for step in 0..=RECOVER_STEPS {
        if session.project() == expected {
            return Some((session, step));
        }
        if step == RECOVER_STEPS || !matches!(session.undo(), Ok(Some(_))) {
            break;
        }
    }
    None
}

/// 起点の逆算: `expected`(project.json)から、履歴の各編集の逆コマンドを後ろから当てて起点を求め、そこから全部を
/// 再生して project.json と一致するか確かめる。project.json が最後の k 手ぶん遅れている(保存の途中で落ちた)
/// 場合も、k を 0 から [`RECOVER_STEPS`] まで試す。一致すれば (セッション, k, 起点)
fn reverse_start(
    expected: &Project,
    entries: &[glaux_core::HistoryEntry],
) -> Option<(Session, usize, Project)> {
    let n = entries.len();
    for k in 0..=RECOVER_STEPS.min(n) {
        let mut start = expected.clone();
        if entries[..n - k]
            .iter()
            .rev()
            .any(|e| start.apply(&e.inverse).is_err())
        {
            continue;
        }
        let Ok(session) = Session::replay(start.clone(), entries.to_vec()) else {
            continue;
        };
        if let Some((session, step)) = settle(session, expected) {
            return Some((session, step, start));
        }
    }
    None
}

fn write_base(base: &Project, first_entry: Option<&str>) -> Result<String> {
    let project: serde_json::Value = serde_json::from_str(
        &base
            .to_json_compact()
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
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_names_are_safe_but_keep_the_title_readable() {
        assert_eq!(folder_name("Night Drive"), "Night_Drive");
        assert_eq!(folder_name("  夏の終わり  Ver.2 "), "夏の終わり_Ver.2");
        assert_eq!(folder_name("A/B: C*?"), "A_B_C");
        assert_eq!(folder_name("Rock 'n' Roll (Live)"), "Rock_'n'_Roll_(Live)");
        assert_eq!(folder_name("..."), "Untitled");
        assert_eq!(folder_name("   "), "Untitled");
        assert_eq!(folder_name("con"), "con_");
        assert_eq!(folder_name("COM1"), "COM1_");
        assert_eq!(folder_name("Community"), "Community");
        assert_eq!(folder_name(&"あ".repeat(100)).chars().count(), 80);
    }

    #[test]
    fn create_project_keeps_the_title_and_avoids_existing_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = unique_project_dir(tmp.path(), &folder_name("My Song"));
        assert_eq!(dir, tmp.path().join("My_Song.glaux"));
        create_project(&dir, "My Song").unwrap();
        let (_, session) = Store::open_or_create(&dir).unwrap();
        assert_eq!(session.project().meta.title, "My Song");
        assert!(
            create_project(&dir, "My Song").is_err(),
            "同じ場所には作らない"
        );
        // 同じ名前のフォルダがあれば番号を付ける
        assert_eq!(
            unique_project_dir(tmp.path(), "My_Song"),
            tmp.path().join("My_Song-2.glaux")
        );
    }

    #[test]
    fn a_song_held_by_another_glaux_is_reported_as_locked() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("Held.glaux");
        create_project(&dir, "Held").unwrap();
        // 別のプロセスが持っているロックの代わりに、ここでファイルをロックしておく
        let f = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(LOCK_FILE))
            .unwrap();
        f.try_lock().unwrap();
        let e = Store::open_or_create(&dir).err().expect("開けない");
        assert!(ProjectLocked::is(&e), "{e:#}");
        assert!(format!("{e:#}").contains("別の Glaux"));
        // ほかの失敗はロックとみなさない
        assert!(!ProjectLocked::is(&anyhow::anyhow!("x")));
    }

    #[test]
    fn a_song_folder_cannot_hold_another_song() {
        let tmp = tempfile::tempdir().unwrap();
        let song = tmp.path().join("My_Song.glaux");
        create_project(&song, "My Song").unwrap();
        assert!(is_project_folder(&song));
        assert!(!is_project_folder(tmp.path()));
        // 曲のフォルダ自身・その中(まだ無いフォルダも)は断る。外は良い
        assert_eq!(enclosing_project(&song), Some(song.clone()));
        assert_eq!(
            enclosing_project(&song.join("sub").join("x")),
            Some(song.clone())
        );
        assert!(check_project_parent(&song).is_err());
        assert!(check_project_parent(tmp.path()).is_ok());
        // 名前が .glaux でなくても、中身が Glaux の曲なら曲のフォルダ
        let plain = tmp.path().join("plain");
        create_project(&plain, "Plain").unwrap();
        assert!(is_project_folder(&plain));
        // ほかの道具の project.json は曲とみなさない
        let other = tmp.path().join("other");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("project.json"), r#"{"name": "web"}"#).unwrap();
        assert!(!is_project_folder(&other));
    }
}
