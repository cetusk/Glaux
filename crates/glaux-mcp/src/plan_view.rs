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
        // 曲の編集と一組の変更(案の採用など。計画の側だけでは取り消せない)
        if let Some(s) = &n.song_entry {
            v["song_entry"] = json!(s);
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
                    "state": p.state, "derived_from": p.derived_from, "edits": p.patch.len(),
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
    // 案ごとの「変わる所」: 案の音を今の曲に当てた曲との違い(区間の番号・範囲・曲全体に効くか・当てられないか)
    // 案ごとの変わる所は重い(曲の複製と、全部の音の比べ)ので別のスレッドで計算し、同じ曲・同じ案なら使い回す
    let props: Vec<glaux_core::plan::Plan> = doc
        .plans
        .values()
        .filter(|p| p.state.as_deref() == Some("proposal"))
        .cloned()
        .collect();
    let cache_key = format!(
        "{}|{version}|{}",
        handle.project_dir().await.unwrap_or_default(),
        props
            .iter()
            .map(|p| format!("{}@{}", p.id, p.rev))
            .collect::<Vec<_>>()
            .join(",")
    );
    let cached = CHANGES_CACHE.lock().ok().and_then(|c| {
        c.as_ref()
            .filter(|(k, _)| *k == cache_key)
            .map(|(_, v)| v.clone())
    });
    let changes = match cached {
        Some(c) => c,
        None => {
            let pr = project.clone();
            let c = tokio::task::spawn_blocking(move || proposal_changes(&pr, &props))
                .await
                .map_err(|e| e.to_string())?;
            if let Ok(mut slot) = CHANGES_CACHE.lock() {
                *slot = Some((cache_key, c.clone()));
            }
            c
        }
    };
    let view =
        tokio::task::spawn_blocking(move || glaux_core::designcheck::design_view(&project, &doc))
            .await
            .map_err(|e| e.to_string())?;
    let mut v = json!(view);
    v["project_version"] = json!(version);
    v["plans"] = json!(list);
    v["history"] = json!(history);
    v["history_total"] = json!(total);
    v["proposal_changes"] = Value::Object(changes);
    v["redoable"] = json!(redoable);
    Ok(v)
}

/// 案ごとの「変わる所」の使い回し(曲のフォルダ・曲の版・案の ID と版が同じ間)
#[allow(clippy::type_complexity)]
static CHANGES_CACHE: std::sync::Mutex<Option<(String, serde_json::Map<String, Value>)>> =
    std::sync::Mutex::new(None);

/// 案ごとの「変わる所」: 案の音を今の曲に当てた曲との違い(区間の番号・範囲・曲全体に効くか・当てられないか〈理由〉)
fn proposal_changes(
    project: &glaux_core::Project,
    props: &[glaux_core::plan::Plan],
) -> serde_json::Map<String, Value> {
    let mut changes = serde_json::Map::new();
    for p in props {
        let v = if p.patch.is_empty() {
            json!({ "sections": [], "ranges": [], "whole": false })
        } else {
            match apply_proposal(project, p) {
                Err(why) => json!({ "stale": true, "why": why }),
                Ok(alt) => {
                    let diff = glaux_core::designcheck::song_diff(project, &alt);
                    let mut marks: Vec<u64> = project.sections.iter().map(|m| m.tick.0).collect();
                    marks.sort_unstable();
                    let sections: Vec<usize> = (0..marks.len())
                        .filter(|&i| {
                            let (a, b) = (marks[i], marks.get(i + 1).copied().unwrap_or(u64::MAX));
                            diff.ranges.iter().any(|&(s, e)| s < b && e > a)
                        })
                        .collect();
                    json!({ "sections": sections, "ranges": diff.ranges, "whole": diff.whole })
                }
            }
        };
        changes.insert(p.id.to_string(), v);
    }
    changes
}

/// 案を今の曲に当てた曲。案を出した後に、案が触る所(トラック・クリップ・エフェクトなど)が直されていたら当てない
/// (人の手直しを案で上書きしないため)。当てられないときも理由を返す
fn apply_proposal(
    project: &glaux_core::Project,
    p: &glaux_core::plan::Plan,
) -> Result<glaux_core::Project, String> {
    let changed = glaux_core::designcheck::patch_base_changed(project, &p.patch_base);
    if !changed.is_empty() {
        return Err(format!(
            "案を出した後に{}が直されたので、案の音を当てません(直した所を案で上書きしないため)。\
             AI に案を作り直してもらってください",
            changed.join("・")
        ));
    }
    let mut alt = project.clone();
    if !p.patch.is_empty() {
        alt.apply(&glaux_core::Command::batch("案", p.patch.clone()))
            .map_err(|e| {
                format!("案を作った後に曲が変わったので、案の音を当てられません({e})。AI に案を作り直してもらってください")
            })?;
    }
    Ok(alt)
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
        song_entry: None,
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
            let state = match state {
                None => old.state.clone(),
                Some("adopted") => None,
                Some(s) => Some(s.to_owned()),
            };
            // 案のまま直すなら、案の音(編集の列と、出したときの指紋)を引き継ぐ
            let still_proposal = state.as_deref() == Some("proposal");
            PlanCommand::Replace {
                plan: Plan {
                    patch_base: if still_proposal {
                        old.patch_base.clone()
                    } else {
                        Default::default()
                    },
                    patch: if still_proposal {
                        old.patch.clone()
                    } else {
                        vec![]
                    },
                    id,
                    name: name.map_or_else(|| old.name.clone(), str::to_owned),
                    kind: kind.to_owned(),
                    rev: old.rev + 1,
                    derived_from: old.derived_from.clone(),
                    state,
                    body,
                },
            }
        }
        None => PlanCommand::Create {
            plan: Plan {
                patch_base: Default::default(),
                patch: vec![],
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

// ---------------------------------------------------------------- 案(枝)

/// AI の案を出す: 元の計画(あれば)から派生した計画(状態 proposal)と、今の曲に当てると案の音になる編集の列(patch)。
/// 今の計画にも曲にも効かない。編集の列は今の曲に試しに当てて確かめ、固定の音は守る(外した編集を返す)
#[allow(clippy::too_many_arguments)]
pub async fn propose(
    handle: &SessionHandle,
    author: glaux_core::Author,
    base_plan_id: Option<&str>,
    name: &str,
    kind: Option<&str>,
    body: Option<Value>,
    commands: Vec<glaux_core::Command>,
    note: glaux_core::EntryNote,
) -> Result<Value, String> {
    use glaux_core::plan::Plan;
    let plans = handle.get_plans().await?;
    let base = match base_plan_id {
        Some(pid) => {
            let id = parse_plan_id(pid)?;
            Some(
                plans
                    .doc()
                    .plans
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| format!("元の計画が見つかりません: {pid}"))?,
            )
        }
        None => None,
    };
    if base
        .as_ref()
        .is_some_and(|b| b.state.as_deref() == Some("proposal"))
    {
        return Err("案から案は作れません(元にするのは今の計画)".to_owned());
    }
    let kind = kind
        .map(str::to_owned)
        .or_else(|| base.as_ref().map(|b| b.kind.clone()))
        .ok_or("kind が要ります(元の計画が無いとき)")?;
    let body = body
        .or_else(|| base.as_ref().map(|b| b.body.clone()))
        .unwrap_or_else(|| json!({}));
    // 案の音: 固定の音を守り、今の曲に当てられることを確かめる
    let (project, _) = handle.get_project_shared().await?;
    let mut locks = Vec::new();
    let patch: Vec<glaux_core::Command> = if commands.is_empty() {
        vec![]
    } else {
        let (guarded, hits) =
            glaux_core::made::guard_locks(&project, glaux_core::Command::batch("案", commands));
        locks = hits;
        let guarded =
            guarded.ok_or("案の編集は全部が固定の音への変更でした(固定は人が外すまで変えない)")?;
        let mut sim = (*project).clone();
        sim.apply(&guarded)
            .map_err(|e| format!("案の編集を今の曲に当てられません: {e}"))?;
        // 案の音が今の曲と同じなら、聴き比べても違いが無い(同じ編集をすでに曲に当てた、など)。案として受け付けない
        let diff = glaux_core::designcheck::song_diff(&project, &sim);
        if diff.ranges.is_empty() && !diff.whole {
            return Err("案の音が今の曲と同じです(聴き比べても違いがありません)。同じ編集をすでに曲に当てていないか確かめ、\
                曲は変えずに、案で変える編集を commands に入れてください"
                .to_owned());
        }
        match guarded {
            glaux_core::Command::Batch { commands, .. } => commands,
            c => vec![c],
        }
    };
    let plan = Plan {
        id: glaux_core::PlanId::new(),
        name: name.to_owned(),
        kind,
        rev: 1,
        derived_from: base.as_ref().map(|b| b.reference()),
        state: Some("proposal".to_owned()),
        body,
        patch_base: glaux_core::designcheck::patch_base(&project, &patch),
        patch,
    };
    let id = plan.id.clone();
    let edits = plan.patch.len();
    let entry = handle
        .apply_plan(
            PlanCommand::Create { plan },
            author,
            format!("案「{name}」"),
            note,
        )
        .await?;
    let mut v = json!({ "entry_id": entry, "plan_id": id, "edits": edits });
    if !locks.is_empty() {
        v["kept_locked"] = json!(locks);
    }
    Ok(v)
}

/// 案を聴き比べるための (今の曲, 案を当てた曲)
pub async fn proposal_projects(
    handle: &SessionHandle,
    plan_id: &str,
) -> Result<(glaux_core::Project, glaux_core::Project), String> {
    let (now, mut alts) = proposal_projects_many(handle, &[plan_id.to_owned()]).await?;
    let alt = alts.pop().ok_or("案が見つかりません")?;
    Ok((now, alt))
}

/// 複数の案を聴き比べるための (今の曲, それぞれの案を当てた曲)。曲は 1 回だけ取り、どの案も同じ曲に当てる
pub async fn proposal_projects_many(
    handle: &SessionHandle,
    plan_ids: &[String],
) -> Result<(glaux_core::Project, Vec<glaux_core::Project>), String> {
    let plans = handle.get_plans().await?;
    let (project, _) = handle.get_project_shared().await?;
    let now = (*project).clone();
    let mut alts = Vec::with_capacity(plan_ids.len());
    for pid in plan_ids {
        let id = parse_plan_id(pid)?;
        let p = plans
            .doc()
            .plans
            .get(&id)
            .filter(|p| p.state.as_deref() == Some("proposal"))
            .ok_or_else(|| format!("案が見つかりません: {pid}"))?;
        alts.push(apply_proposal(&now, p).map_err(|e| format!("案「{}」: {e}", p.name))?);
    }
    Ok((now, alts))
}

/// 計画の履歴の、いちばん新しい側の項目の ID(新しい順に `n` 件)と、やり直せる項目の ID(次にやり直す順に `n` 件)。
/// 画面の Ctrl+Z が、戻そうとしている操作がまだいちばん新しいかを確かめるのに使う
///
/// `ids` を渡すと、そのうち今も効いている(取り消されていない)項目も返す(present)
pub async fn plan_head(handle: &SessionHandle, n: usize, ids: &[String]) -> Result<Value, String> {
    let plans = handle.get_plans().await?;
    let present: Vec<&String> = ids
        .iter()
        .filter(|id| {
            plans
                .history()
                .applied()
                .iter()
                .any(|e| e.id.to_string() == **id)
        })
        .collect();
    let applied: Vec<String> = plans
        .history()
        .applied()
        .iter()
        .rev()
        .take(n)
        .map(|e| e.id.to_string())
        .collect();
    let redoable: Vec<String> = plans
        .history()
        .redoable()
        .iter()
        .take(n)
        .map(|e| e.id.to_string())
        .collect();
    Ok(json!({ "applied": applied, "redoable": redoable, "present": present }))
}

/// 案を採用する: 案の音を曲に当て(1 件の編集)、案の計画を今の計画にする(元の計画があれば中身を置き換えて案を消す)
///
/// 採用したら、ほかの案のうち同じ組のもの(同じ元の計画から出た案〈元の計画が変わったので古くなる〉と、
/// 同じきっかけ〈人の同じ言葉〉で出された案)と `also_discard`(いっしょに聴き比べていた案など)を捨てる
/// (計画の履歴に 1 件ずつ残るので、取り消せる)
pub async fn adopt_proposal(
    handle: &SessionHandle,
    plan_id: &str,
    also_discard: &[String],
) -> Result<Value, String> {
    use glaux_core::plan::Plan;
    let id = parse_plan_id(plan_id)?;
    let plans = handle.get_plans().await?;
    let p = plans
        .doc()
        .plans
        .get(&id)
        .filter(|p| p.state.as_deref() == Some("proposal"))
        .cloned()
        .ok_or("案が見つかりません")?;
    // 案を出した後に、案が触る所が直されていたら当てない(手直しを上書きしない)。当てられるかもここで確かめる
    {
        let (project, _) = handle.get_project_shared().await?;
        apply_proposal(&project, &p)?;
    }
    let label = format!("案「{}」を採用", p.name);
    let mut song_entry = None;
    let mut kept_locked = Vec::new();
    if !p.patch.is_empty() {
        // 案の音は AI が作ったもの。人が採用したことを作者の名前に残す(指紋も AI の編集として記録する)。
        // AI の編集なので固定の音は守られる(外した編集は kept_locked で知らせる)
        let r = handle
            .apply(
                glaux_core::Command::batch(label.clone(), p.patch.clone()),
                glaux_core::Author::Ai {
                    model: "案(人が採用)".to_owned(),
                },
                label.clone(),
            )
            .await?;
        match r {
            Ok((e, m)) => {
                song_entry = Some(e);
                kept_locked = m.locks;
            }
            Err(glaux_core::CoreError::Locked(what)) => {
                return Err(format!(
                    "案の編集は全部が固定の音に当たるので、当てられません({what})。固定は人が外すまで変えません"
                ));
            }
            Err(e) => {
                return Err(format!(
                    "案を作った後に曲が変わったので、案の音を当てられません({e})。AI に案を作り直してもらってください"
                ));
            }
        }
    }
    let base = p
        .derived_from
        .as_ref()
        .and_then(|r| plans.doc().plans.get(&r.id))
        .cloned();
    let human = glaux_core::Author::Human;
    // 計画の変更は、曲に当てた採用の編集と一組にする(曲の側でどの画面から取り消しても、計画も一緒に戻るように)
    let linked = |why: &str| glaux_core::EntryNote {
        song_entry: song_entry.clone(),
        ..note(why)
    };
    // 計画の履歴に書いた項目(画面の Ctrl+Z が採用をまとめて戻し、それがまだ新しいかを確かめるのに使う)
    let mut plan_entry_ids = Vec::new();
    match base {
        Some(b) => {
            plan_entry_ids.push(
                handle
                    .apply_plan(
                        PlanCommand::Replace {
                            plan: Plan {
                                rev: b.rev + 1,
                                body: p.body.clone(),
                                state: None,
                                ..b
                            },
                        },
                        human.clone(),
                        label.clone(),
                        linked(&label),
                    )
                    .await?,
            );
            plan_entry_ids.push(
                handle
                    .apply_plan(
                        PlanCommand::Delete { id: p.id.clone() },
                        human,
                        format!("採用した案「{}」をしまう", p.name),
                        linked(&label),
                    )
                    .await?,
            );
        }
        None => {
            plan_entry_ids.push(
                handle
                    .apply_plan(
                        PlanCommand::Replace {
                            plan: Plan {
                                rev: p.rev + 1,
                                state: None,
                                patch: vec![],
                                patch_base: Default::default(),
                                ..p.clone()
                            },
                        },
                        human,
                        label.clone(),
                        linked(&label),
                    )
                    .await?,
            );
        }
    }
    // ほかの案を捨てる
    let extra: Vec<_> = also_discard
        .iter()
        .filter_map(|s| parse_plan_id(s).ok())
        .collect();
    let base_id = p.derived_from.as_ref().map(|r| r.id.clone());
    // 案を出したときのきっかけと時刻(最初の履歴の項目)。AI が 1 つの頼みで並べて出した案は、同じきっかけで、続けて出る
    let created = |id: &glaux_core::PlanId| {
        plans
            .history()
            .applied()
            .iter()
            .find(|e| e.forward.plan_id() == id)
            .and_then(|e| Some((e.note.as_ref()?.trigger.clone()?, e.timestamp)))
    };
    let mine = created(&p.id);
    let same_ask = |q: &glaux_core::PlanId| same_ask(mine.as_ref(), created(q).as_ref());
    let others: Vec<(glaux_core::PlanId, String)> = plans
        .doc()
        .plans
        .values()
        .filter(|q| q.id != p.id && q.state.as_deref() == Some("proposal"))
        .filter(|q| {
            extra.contains(&q.id)
                || (base_id.is_some() && q.derived_from.as_ref().map(|r| &r.id) == base_id.as_ref())
                || same_ask(&q.id)
        })
        .map(|q| (q.id.clone(), q.name.clone()))
        .collect();
    let mut discarded = Vec::new();
    for (id, name) in others {
        let l = format!("案「{}」を採用したので、案「{name}」を捨てる", p.name);
        plan_entry_ids.push(
            handle
                .apply_plan(
                    PlanCommand::Delete { id },
                    glaux_core::Author::Human,
                    l.clone(),
                    linked(&l),
                )
                .await?,
        );
        discarded.push(name);
    }
    let mut v = json!({ "entry_id": song_entry, "plan_entry_id": plan_entry_ids.first(), "discarded": discarded,
                        "plan_entries": plan_entry_ids.len(), "plan_entry_ids": plan_entry_ids });
    if !kept_locked.is_empty() {
        v["kept_locked"] = json!(kept_locked);
    }
    Ok(v)
}

/// 同じきっかけの案を「同じ頼みの組」とみなす、出した時刻の近さ(分)
const SAME_ASK_MINUTES: i64 = 15;

/// 2 つの案が同じ頼みから出たか: きっかけ(種類と言葉)が同じで、言葉が空でなく、出した時刻が近い
/// (別の時に同じ言葉で頼んだ案は別の組)
fn same_ask(
    a: Option<&(glaux_core::Trigger, chrono::DateTime<chrono::Utc>)>,
    b: Option<&(glaux_core::Trigger, chrono::DateTime<chrono::Utc>)>,
) -> bool {
    match (a, b) {
        (Some((t, at)), Some((u, bt))) => {
            t == u
                && !t.text.trim().is_empty()
                && (*at - *bt).num_minutes().abs() <= SAME_ASK_MINUTES
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_ask_needs_the_same_words_close_in_time() {
        let t = |text: &str| glaux_core::Trigger {
            kind: "user".to_owned(),
            text: text.to_owned(),
        };
        let at = chrono::Utc::now();
        let near = at + chrono::Duration::minutes(2);
        let far = at + chrono::Duration::minutes(60);
        assert!(same_ask(Some(&(t("3 案"), at)), Some(&(t("3 案"), near))));
        // 別の時に同じ言葉で頼んだ案は別の組
        assert!(!same_ask(Some(&(t("3 案"), at)), Some(&(t("3 案"), far))));
        assert!(!same_ask(Some(&(t("3 案"), at)), Some(&(t("別"), near))));
        // 言葉が空・きっかけが無いものは組にしない
        assert!(!same_ask(Some(&(t(" "), at)), Some(&(t(" "), near))));
        assert!(!same_ask(None, Some(&(t("3 案"), near))));
    }
}
