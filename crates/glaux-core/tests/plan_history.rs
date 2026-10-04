//! 計画の履歴の可逆性: ランダムな計画の編集を積み、undo で最初に、redo で最後に戻ることを確かめる
//! (曲の tests/history.rs と同じ考え方)。

use glaux_core::plan::{Plan, PlanCommand, PlanOp, PlanSet};
use glaux_core::{Author, History, PlanId, Session};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use serde_json::{json, Value};

fn new_plan(rng: &mut StdRng) -> Plan {
    let sections: Vec<Value> = (0..rng.gen_range(0..3))
        .map(|i| {
            json!({
                "name": format!("sec{i}"),
                "start_bar": 1 + i * 8,
                "bars": 8,
                "phrases": (0..rng.gen_range(0..3)).map(|k| json!({ "label": format!("P{k}"), "bars": 2 })).collect::<Vec<_>>()
            })
        })
        .collect();
    Plan {
        state: None,
        id: PlanId::new(),
        name: format!("plan{}", rng.gen_range(0..100)),
        kind: "melody".into(),
        rev: 1,
        derived_from: None,
        body: json!({ "genre": "house", "sections": sections }),
    }
}

/// 計画の中の道を 1 つ選ぶ(オブジェクトの項目・配列の要素)
fn paths(v: &Value, at: String, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                let p = format!("{at}/{k}");
                out.push(p.clone());
                paths(x, p, out);
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                let p = format!("{at}/{i}");
                out.push(p.clone());
                paths(x, p, out);
            }
        }
        _ => {}
    }
}

fn random_command(doc: &PlanSet, rng: &mut StdRng) -> PlanCommand {
    let plans: Vec<&Plan> = doc.plans.values().collect();
    let Some(p) = plans.choose(rng).copied().filter(|_| rng.gen_bool(0.85)) else {
        return PlanCommand::Create {
            plan: new_plan(rng),
        };
    };
    match rng.gen_range(0..10) {
        0 => PlanCommand::Delete { id: p.id.clone() },
        1 => {
            let mut q = new_plan(rng);
            q.id = p.id.clone();
            q.rev = p.rev + 1;
            PlanCommand::Replace { plan: q }
        }
        _ => {
            let mut ps = Vec::new();
            paths(&p.body, String::new(), &mut ps);
            let ops = (0..rng.gen_range(1..4))
                .map(|_| match rng.gen_range(0..6) {
                    0 => PlanOp::Set {
                        path: "/intent".into(),
                        value: json!(format!("i{}", rng.gen_range(0..9))),
                    },
                    1 => PlanOp::Set {
                        path: "/sections/-".into(),
                        value: json!({ "name": "new", "start_bar": 1, "bars": 4 }),
                    },
                    2 => PlanOp::Insert {
                        path: "/sections/0".into(),
                        value: json!({ "name": "ins", "start_bar": 2, "bars": 2 }),
                    },
                    3 => PlanOp::Set {
                        path: "/sections/0/energy".into(),
                        value: json!(rng.gen_range(0..=10)),
                    },
                    _ => match ps.choose(rng) {
                        // 必須の項目を消すと形に合わず失敗する(失敗も試験の一部)
                        Some(path) => PlanOp::Remove { path: path.clone() },
                        None => PlanOp::Set {
                            path: "/seed".into(),
                            value: json!(rng.gen_range(0..9)),
                        },
                    },
                })
                .collect();
            PlanCommand::Edit {
                id: p.id.clone(),
                rev: p.rev + 1,
                ops,
            }
        }
    }
}

#[test]
fn random_plan_edits_are_reversible() {
    for seed in 0..40u64 {
        let mut rng = StdRng::seed_from_u64(seed);
        let mut s: Session<PlanSet> = Session::new(PlanSet::default());
        let mut states = vec![s.doc().clone()];
        let mut applied = 0;
        for _ in 0..60 {
            let cmd = random_command(s.doc(), &mut rng);
            let before = s.doc().clone();
            match s.apply(cmd, Author::Human, "r") {
                Ok(_) => {
                    applied += 1;
                    states.push(s.doc().clone());
                }
                // 失敗した編集は何も変えない
                Err(_) => assert_eq!(s.doc(), &before, "seed {seed}"),
            }
        }
        assert!(applied > 20, "seed {seed}: {applied}");
        // 保存して読み戻しても同じ
        let text = s.history().to_jsonl().unwrap();
        let replayed = Session::replay(
            PlanSet::default(),
            History::<PlanCommand>::entries_from_jsonl(&text).unwrap(),
        )
        .unwrap();
        assert_eq!(replayed.doc(), s.doc(), "seed {seed}");
        for want in states.iter().rev().skip(1) {
            s.undo().unwrap();
            assert_eq!(s.doc(), want, "seed {seed}");
        }
        assert!(!s.can_undo());
        while s.can_redo() {
            s.redo().unwrap();
        }
        assert_eq!(s.doc(), states.last().unwrap(), "seed {seed}");
    }
}
