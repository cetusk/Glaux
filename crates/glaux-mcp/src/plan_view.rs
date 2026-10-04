//! 設計画面(アプリ)のための、計画と実際のまとめと計画の履歴。アプリの Tauri コマンドから呼ぶ
//! (MCP の get_design・plan_log と同じ中身を、画面で使う形で返す)。

use crate::actor::SessionHandle;
use glaux_core::plan::PlanCommand;
use glaux_core::HistoryEntry;
use serde_json::{json, Value};

/// 計画の履歴の 1 件(MCP の plan_log と同じ項目)
pub fn plan_entry_json(e: &HistoryEntry<PlanCommand>) -> Value {
    let (op, rev, paths): (&str, Option<u64>, Vec<&str>) = match &e.forward {
        PlanCommand::Create { plan } => ("create", Some(plan.rev), vec![]),
        PlanCommand::Replace { plan } => ("replace", Some(plan.rev), vec![]),
        PlanCommand::Delete { .. } => ("delete", None, vec![]),
        PlanCommand::Edit { rev, ops, .. } => {
            ("edit", Some(*rev), ops.iter().map(|o| o.path()).collect())
        }
    };
    let mut v = json!({
        "entry_id": e.id,
        "time": e.timestamp,
        "author": e.author,
        "subject": e.label,
        "plan_id": e.forward.plan_id(),
        "op": op,
    });
    if let Some(r) = rev {
        v["rev"] = json!(r);
    }
    if !paths.is_empty() {
        v["paths"] = json!(paths);
    }
    if let Some(n) = &e.note {
        if !n.why.is_empty() {
            v["why"] = json!(n.why);
        }
        if let Some(t) = &n.trigger {
            v["trigger"] = json!(t);
        }
        if !n.measures.is_empty() {
            v["measures"] = json!(n.measures);
        }
    }
    if let Some(r) = &e.reverts {
        v["reverts"] = json!(r);
    }
    v
}

/// 設計画面の中身: 計画と実際(designcheck::design_view)、計画の一覧、計画の履歴(新しい順、`limit` 件)
pub async fn design(handle: &SessionHandle, limit: usize) -> Result<Value, String> {
    let (project, version) = handle.get_project_shared().await?;
    let plans = handle.get_plans().await?;
    let doc = plans.doc().clone();
    let list: Vec<Value> = doc
        .plans
        .values()
        .map(|p| {
            json!({ "plan_id": p.id, "name": p.name, "kind": p.kind, "rev": p.rev,
                    "state": p.state, "derived_from": p.derived_from,
                    // 画面が直すときに丸ごと書き戻すため、旋律以外は中身も渡す(旋律の計画は大きい)
                    "body": (p.kind != "melody").then_some(&p.body) })
        })
        .collect();
    let history: Vec<Value> = plans
        .history()
        .applied()
        .iter()
        .rev()
        .take(limit.clamp(1, 500))
        .map(plan_entry_json)
        .collect();
    let total = plans.history().applied().len();
    let redoable = plans.history().redoable().len();
    let view =
        tokio::task::spawn_blocking(move || glaux_core::designcheck::design_view(&project, &doc))
            .await
            .map_err(|e| e.to_string())?;
    let mut v = json!(view);
    v["project_version"] = json!(version);
    v["plans"] = json!(list);
    v["history"] = json!(history);
    v["history_total"] = json!(total);
    v["redoable"] = json!(redoable);
    Ok(v)
}

/// タイムラインのクリップの印(計画どおり / 計画が先に進んだ・手で直した小節・固定の音の数)
pub async fn clip_states(handle: &SessionHandle) -> Result<Value, String> {
    let (project, version) = handle.get_project_shared().await?;
    let plans = handle.get_plans().await?;
    let doc = plans.doc().clone();
    let states =
        tokio::task::spawn_blocking(move || glaux_core::designcheck::clip_states(&project, &doc))
            .await
            .map_err(|e| e.to_string())?;
    Ok(json!({ "project_version": version, "clips": states }))
}

// ---------------------------------------------------------------- 画面からの計画の編集(人の操作)

fn note(why: &str) -> glaux_core::EntryNote {
    glaux_core::EntryNote {
        why: why.to_owned(),
        trigger: None,
        measures: vec![],
    }
}

fn parse_plan_id(s: &str) -> Result<glaux_core::PlanId, String> {
    glaux_core::PlanId::parse(s).map_err(|e| e.to_string())
}

/// 計画を保存する(`plan_id` が無ければ作る、あれば丸ごと置き換える。版は今の版 + 1)。
/// `state`: "estimated" / "adopted"(採用。状態を外す)/ 省略(作るときは採用、置き換えは今の状態のまま)
pub async fn save(
    handle: &SessionHandle,
    plan_id: Option<&str>,
    name: Option<&str>,
    kind: &str,
    body: Value,
    state: Option<&str>,
    label: &str,
) -> Result<Value, String> {
    use glaux_core::plan::Plan;
    let plans = handle.get_plans().await?;
    let command = match plan_id {
        Some(pid) => {
            let id = parse_plan_id(pid)?;
            let old = plans
                .doc()
                .plans
                .get(&id)
                .ok_or_else(|| format!("計画が見つかりません: {pid}"))?;
            PlanCommand::Replace {
                plan: Plan {
                    id,
                    name: name.map_or_else(|| old.name.clone(), str::to_owned),
                    kind: kind.to_owned(),
                    rev: old.rev + 1,
                    derived_from: old.derived_from.clone(),
                    state: match state {
                        None => old.state.clone(),
                        Some("adopted") => None,
                        Some(s) => Some(s.to_owned()),
                    },
                    body,
                },
            }
        }
        None => PlanCommand::Create {
            plan: Plan {
                id: glaux_core::PlanId::new(),
                name: name.unwrap_or(kind).to_owned(),
                kind: kind.to_owned(),
                rev: 1,
                derived_from: None,
                state: match state {
                    None | Some("adopted") => None,
                    Some(s) => Some(s.to_owned()),
                },
                body,
            },
        },
    };
    let id = command.plan_id().clone();
    let entry = handle
        .apply_plan(
            command,
            glaux_core::Author::Human,
            label.to_owned(),
            note(label),
        )
        .await?;
    Ok(json!({ "entry_id": entry, "plan_id": id }))
}

/// 計画を消す
pub async fn delete(handle: &SessionHandle, plan_id: &str, label: &str) -> Result<Value, String> {
    let id = parse_plan_id(plan_id)?;
    let entry = handle
        .apply_plan(
            PlanCommand::Delete { id },
            glaux_core::Author::Human,
            label.to_owned(),
            note(label),
        )
        .await?;
    Ok(json!({ "entry_id": entry }))
}

/// 計画の取り消し・やり直し(n 回)
pub async fn step(handle: &SessionHandle, n: usize, redo: bool) -> Result<Value, String> {
    let done = handle.step_plan(n.max(1), redo).await?;
    Ok(json!({ "done": done }))
}

/// 計画の途中の変更だけを取り消す(後の変更は残す)
pub async fn revert(handle: &SessionHandle, entry_id: &str) -> Result<Value, String> {
    let id = glaux_core::EntryId::parse(entry_id).map_err(|e| e.to_string())?;
    let (entry, conflicts) = handle
        .revert_plan(
            id,
            glaux_core::Author::Human,
            note("画面で「この変更だけ取り消す」"),
        )
        .await?;
    Ok(json!({ "entry_id": entry, "conflicts": conflicts }))
}

/// 計画を前の版の中身に戻す(戻したことも新しい版として残る。壊さない)
pub async fn restore(handle: &SessionHandle, plan_id: &str, rev: u64) -> Result<Value, String> {
    let id = parse_plan_id(plan_id)?;
    let plans = handle.get_plans().await?;
    let old = glaux_core::plan::plan_at_rev(&plans, &id, rev)
        .ok_or_else(|| format!("版 {rev} が履歴に見つかりません"))?;
    let cur_rev = plans.doc().plans.get(&id).map(|p| p.rev);
    let label = format!("「{}」を版 {rev} に戻す", old.name);
    let command = match cur_rev {
        Some(r) => PlanCommand::Replace {
            plan: glaux_core::plan::Plan { rev: r + 1, ..old },
        },
        // 消した計画を戻す
        None => PlanCommand::Create { plan: old },
    };
    let entry = handle
        .apply_plan(
            command,
            glaux_core::Author::Human,
            label.clone(),
            note(&label),
        )
        .await?;
    Ok(json!({ "entry_id": entry }))
}

/// 推定した計画(state: estimated)をまとめて採用する(adopt)か捨てる
pub async fn settle_estimated(handle: &SessionHandle, adopt: bool) -> Result<Value, String> {
    let plans = handle.get_plans().await?;
    let targets: Vec<glaux_core::plan::Plan> = plans
        .doc()
        .plans
        .values()
        .filter(|p| p.state.as_deref() == Some("estimated"))
        .cloned()
        .collect();
    let mut entries = Vec::new();
    for p in targets {
        let (command, label) = if adopt {
            let label = format!("推定した計画「{}」を採用", p.name);
            (
                PlanCommand::Replace {
                    plan: glaux_core::plan::Plan {
                        rev: p.rev + 1,
                        state: None,
                        ..p
                    },
                },
                label,
            )
        } else {
            let label = format!("推定した計画「{}」を捨てる", p.name);
            (PlanCommand::Delete { id: p.id }, label)
        };
        entries.push(
            handle
                .apply_plan(
                    command,
                    glaux_core::Author::Human,
                    label.clone(),
                    note(&label),
                )
                .await?,
        );
    }
    Ok(json!({ "entries": entries }))
}
