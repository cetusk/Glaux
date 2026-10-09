//! 計画の保存(plans.json / plans.history.jsonl / plans.base.json)の開き直しと復旧。

use glaux_core::plan::{Plan, PlanCommand, PlanSet};
use glaux_core::{Author, EntryNote, PlanId};
use glaux_mcp::plan_store::{PlanStore, PLANS_BASE_FILE, PLANS_FILE, PLANS_HISTORY_FILE};
use serde_json::json;
use std::fs;

fn plan(id: &PlanId, name: &str) -> Plan {
    Plan {
        patch_base: Default::default(),
        patch: vec![],
        group: None,
        state: None,
        id: id.clone(),
        name: name.into(),
        kind: "melody".into(),
        rev: 1,
        derived_from: None,
        body: json!({ "genre": "house" }),
    }
}

fn note(why: &str) -> EntryNote {
    EntryNote {
        why: why.into(),
        ..Default::default()
    }
}

fn edit(id: &PlanId, rev: u64, seed: u64) -> PlanCommand {
    PlanCommand::Edit {
        id: id.clone(),
        rev,
        ops: vec![
            serde_json::from_value(json!({ "op": "set", "path": "/seed", "value": seed })).unwrap(),
        ],
    }
}

#[test]
fn a_song_without_plans_gets_no_files() {
    let tmp = tempfile::tempdir().unwrap();
    let (_, s) = PlanStore::open(tmp.path()).unwrap();
    assert!(s.doc().plans.is_empty());
    assert!(!tmp.path().join(PLANS_FILE).exists());
    assert!(!tmp.path().join(PLANS_HISTORY_FILE).exists());
}

#[test]
fn plans_and_history_survive_reopening_and_undo() {
    let tmp = tempfile::tempdir().unwrap();
    let id = PlanId::new();
    let (mut store, mut s) = PlanStore::open(tmp.path()).unwrap();
    s.apply_with_note(
        PlanCommand::Create {
            plan: plan(&id, "a"),
        },
        Author::Human,
        "作る",
        note("始める"),
    )
    .unwrap();
    store.save(&s).unwrap();
    for k in 0..3 {
        s.apply_with_note(edit(&id, 2 + k, k), Author::Human, "seed", note("試す"))
            .unwrap();
        store.save(&s).unwrap();
    }
    // undo の後に新しい変更: 履歴は書き直される
    s.undo().unwrap();
    store.save(&s).unwrap();
    s.apply_with_note(edit(&id, 4, 9), Author::Human, "seed", note("別の値"))
        .unwrap();
    store.save(&s).unwrap();
    let (_, again) = PlanStore::open(tmp.path()).unwrap();
    assert_eq!(again.doc(), s.doc());
    assert_eq!(again.history().applied().len(), 4);
    assert_eq!(
        again.history().applied()[3].note.as_ref().unwrap().why,
        "別の値"
    );
    assert_eq!(again.doc().plans[&id].body["seed"], 9);
}

#[test]
fn history_ahead_of_plans_json_wins_and_a_torn_last_line_is_dropped() {
    let tmp = tempfile::tempdir().unwrap();
    let id = PlanId::new();
    let (mut store, mut s) = PlanStore::open(tmp.path()).unwrap();
    s.apply_with_note(
        PlanCommand::Create {
            plan: plan(&id, "a"),
        },
        Author::Human,
        "作る",
        note("x"),
    )
    .unwrap();
    store.save(&s).unwrap();
    let old_plans = fs::read(tmp.path().join(PLANS_FILE)).unwrap();
    s.apply_with_note(edit(&id, 2, 5), Author::Human, "seed", note("y"))
        .unwrap();
    store.save(&s).unwrap();
    // plans.json を書く前に止まった(古いまま)+ 次の行を書きかけで止まった
    fs::write(tmp.path().join(PLANS_FILE), &old_plans).unwrap();
    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(tmp.path().join(PLANS_HISTORY_FILE))
        .unwrap();
    std::io::Write::write_all(&mut f, b"{\"id\":\"hst_zz").unwrap();
    drop(f);
    let (_, again) = PlanStore::open(tmp.path()).unwrap();
    assert_eq!(again.doc(), s.doc());
    let on_disk: PlanSet =
        serde_json::from_str(&fs::read_to_string(tmp.path().join(PLANS_FILE)).unwrap()).unwrap();
    assert_eq!(&on_disk, s.doc());
}

#[test]
fn a_broken_history_is_set_aside_and_plans_json_becomes_the_start() {
    let tmp = tempfile::tempdir().unwrap();
    let id = PlanId::new();
    let (mut store, mut s) = PlanStore::open(tmp.path()).unwrap();
    s.apply_with_note(
        PlanCommand::Create {
            plan: plan(&id, "a"),
        },
        Author::Human,
        "作る",
        note("x"),
    )
    .unwrap();
    s.apply_with_note(edit(&id, 2, 5), Author::Human, "seed", note("y"))
        .unwrap();
    store.save(&s).unwrap();
    for round in 1..=2 {
        // 途中の行が壊れている
        let path = tmp.path().join(PLANS_HISTORY_FILE);
        fs::write(&path, "garbage\n{}\n").unwrap();
        let (mut store2, mut again) = PlanStore::open(tmp.path()).unwrap();
        assert_eq!(again.doc(), s.doc(), "round {round}");
        assert!(again.history().applied().is_empty());
        assert!(tmp.path().join(PLANS_BASE_FILE).exists());
        let orphan = if round == 1 {
            format!("{PLANS_HISTORY_FILE}.orphan")
        } else {
            format!("{PLANS_HISTORY_FILE}.orphan.2")
        };
        assert!(tmp.path().join(&orphan).exists(), "round {round}");
        // 起点から続けられる
        again
            .apply_with_note(edit(&id, 3, 7), Author::Human, "seed", note("z"))
            .unwrap();
        store2.save(&again).unwrap();
        let (_, third) = PlanStore::open(tmp.path()).unwrap();
        assert_eq!(third.doc(), again.doc());
        assert_eq!(third.history().applied().len(), 1);
        s = again;
    }
}
