//! 旋律の計画の保存(曲のフォルダの中の `plans.json` / `plans.history.jsonl` / `plans.base.json`)。
//!
//! 曲の保存([`crate::store`])と同じ考え方で、計画だけの別の履歴を持つ。
//! - `plans.json`: 今の全計画(作業ツリー)
//! - `plans.history.jsonl`: 計画のコマンドと逆コマンド、経緯(1 行 1 件、コミットの列)
//! - `plans.base.json`: 履歴の起点(無ければ空)。履歴を読めないときは今の `plans.json` を起点にし直す
//!
//! 計画は小さいので、書き込みはその場で行う(裏の書き込み・履歴の切り詰めは持たない)。
//! 書く順は「履歴 → plans.json」。途中で止まったら、開くときに履歴の再生の結果を正とする。
//! 計画が 1 つも無い曲ではファイルを作らない。

use anyhow::{Context, Result};
use glaux_core::plan::{PlanCommand, PlanSet};
use glaux_core::{EntryId, History, HistoryEntry, Session};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const PLANS_FILE: &str = "plans.json";
pub const PLANS_HISTORY_FILE: &str = "plans.history.jsonl";
pub const PLANS_BASE_FILE: &str = "plans.base.json";

pub type PlanSession = Session<PlanSet>;

pub struct PlanStore {
    dir: PathBuf,
    /// 履歴のファイルに書いてあるエントリの ID(先頭から順)。これと今の適用済みの列の先頭が同じなら追記で済む
    saved: Vec<EntryId>,
}

impl PlanStore {
    /// 曲のフォルダから計画を開く。無ければ空
    pub fn open(dir: impl Into<PathBuf>) -> Result<(PlanStore, PlanSession)> {
        let dir = dir.into();
        let plans_path = dir.join(PLANS_FILE);
        let history_path = dir.join(PLANS_HISTORY_FILE);
        let base_path = dir.join(PLANS_BASE_FILE);
        let current: Option<PlanSet> = match fs::read_to_string(&plans_path) {
            Ok(t) => Some(
                serde_json::from_str(&t)
                    .with_context(|| format!("{} を読めません", plans_path.display()))?,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(e).with_context(|| format!("{} を読めません", plans_path.display()))
            }
        };
        let base: Option<PlanSet> = match fs::read_to_string(&base_path) {
            Ok(t) => serde_json::from_str(&t).ok(),
            Err(_) => None,
        };
        let text = fs::read_to_string(&history_path).unwrap_or_default();
        // 最後の行が書きかけなら捨てる
        let mut entries: Vec<HistoryEntry<PlanCommand>> = Vec::new();
        let mut broken = None;
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        for (i, l) in lines.iter().enumerate() {
            match serde_json::from_str(l) {
                Ok(e) => entries.push(e),
                Err(e) if i + 1 == lines.len() => {
                    tracing::warn!("{PLANS_HISTORY_FILE} の最後の行を読めないので捨てます: {e}")
                }
                Err(e) => {
                    broken = Some(format!("{} 行目を読めない: {e}", i + 1));
                    break;
                }
            }
        }
        let base_ok = base.is_some() || !base_path.exists();
        let replayed = match (&broken, base_ok) {
            (None, true) => Session::replay(base.clone().unwrap_or_default(), entries.clone()).ok(),
            _ => None,
        };
        let mut store = PlanStore {
            dir: dir.clone(),
            saved: vec![],
        };
        match replayed {
            Some(s) => {
                store.saved = s.history().applied().iter().map(|e| e.id.clone()).collect();
                if current.as_ref() != Some(s.doc()) && (current.is_some() || !entries.is_empty()) {
                    // 履歴の方が先に進んでいる(plans.json を書く前に止まった)。履歴を正として書き直す
                    if current.is_some() {
                        tracing::warn!(
                            "{PLANS_FILE} が履歴と合わないので、履歴の再生の結果で書き直します"
                        );
                    }
                    store.write_plans(s.doc())?;
                }
                Ok((store, s))
            }
            None => {
                // 履歴を使えない: 今の plans.json を起点にし直し、履歴は退避する(上書きしない)
                let reason = broken.unwrap_or_else(|| "起点または再生に失敗".to_owned());
                let slot = (1..)
                    .map(|k: u32| {
                        if k == 1 {
                            ".orphan".to_owned()
                        } else {
                            format!(".orphan.{k}")
                        }
                    })
                    .find(|suf| !dir.join(format!("{PLANS_HISTORY_FILE}{suf}")).exists())
                    .unwrap_or_else(|| ".orphan".to_owned());
                tracing::warn!(
                    "{PLANS_HISTORY_FILE} を使えません({reason})。{PLANS_HISTORY_FILE}{slot} に退避します"
                );
                if history_path.exists() {
                    fs::rename(
                        &history_path,
                        dir.join(format!("{PLANS_HISTORY_FILE}{slot}")),
                    )
                    .context("計画の履歴の退避に失敗")?;
                }
                if base_path.exists() {
                    let _ = fs::rename(&base_path, dir.join(format!("{PLANS_BASE_FILE}{slot}")));
                }
                let start = current.unwrap_or_default();
                crate::store::write_atomic(&base_path, &serde_json::to_vec_pretty(&start)?)?;
                Ok((store, Session::new(start)))
            }
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn write_plans(&self, plans: &PlanSet) -> Result<()> {
        crate::store::write_atomic(
            &self.dir.join(PLANS_FILE),
            &serde_json::to_vec_pretty(plans)?,
        )
    }

    /// 変更のあとに保存する。履歴は先頭が同じなら追記、違えば(undo の後の新しい変更・undo)書き直す
    pub fn save(&mut self, session: &PlanSession) -> Result<()> {
        let applied = session.history().applied();
        let history_path = self.dir.join(PLANS_HISTORY_FILE);
        let same_prefix = applied.len() >= self.saved.len()
            && applied.iter().zip(&self.saved).all(|(e, id)| &e.id == id);
        if same_prefix {
            if applied.len() > self.saved.len() {
                let mut f = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&history_path)
                    .with_context(|| format!("書き込み失敗: {}", history_path.display()))?;
                let mut buf = String::new();
                for e in &applied[self.saved.len()..] {
                    buf.push_str(&serde_json::to_string(e)?);
                    buf.push('\n');
                }
                f.write_all(buf.as_bytes())
                    .and_then(|_| f.sync_all())
                    .with_context(|| format!("書き込み失敗: {}", history_path.display()))?;
            }
        } else {
            let text = session.history().to_jsonl()?;
            crate::store::write_atomic(&history_path, text.as_bytes())?;
        }
        self.saved = applied.iter().map(|e| e.id.clone()).collect();
        self.write_plans(session.doc())
    }
}

/// 履歴の JSONL を読む(試験・調査用)
pub fn read_entries(dir: &Path) -> Result<Vec<HistoryEntry<PlanCommand>>> {
    let text = fs::read_to_string(dir.join(PLANS_HISTORY_FILE)).unwrap_or_default();
    Ok(History::<PlanCommand>::entries_from_jsonl(&text)?)
}
