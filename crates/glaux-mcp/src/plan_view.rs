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
                    "state": p.state, "derived_from": p.derived_from })
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
