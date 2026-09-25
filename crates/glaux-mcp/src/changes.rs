//! 履歴の差分の要約(MCP の `get_changes` と、チャットに添える「人間が変えたこと」)。
//!
//! 以前 AI に渡せたのは履歴のラベルの羅列だけで、「どのクリップのどのノートが変わったか」は
//! get_project を読み直して自分で比べるしかなかった。ここでは履歴のコマンドをたどって、
//! クリップごとのノートの増減・変更と、トラック・エフェクト・曲全体の操作をまとめる。

use glaux_core::{Command, HistoryEntry, Project, Target};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// 1 クリップあたり返すノート ID の上限(多いときは数だけ)
const MAX_IDS: usize = 30;

#[derive(Default)]
struct ClipChanges {
    added: Vec<String>,
    removed: Vec<String>,
    changed: BTreeSet<String>,
    ops: BTreeSet<String>,
}

#[derive(Default)]
struct Acc {
    clips: BTreeMap<String, ClipChanges>,
    tracks: BTreeMap<String, BTreeSet<String>>,
    effects: BTreeMap<String, BTreeSet<String>>,
    global: BTreeSet<String>,
}

fn op_name(cmd: &Command) -> String {
    serde_json::to_value(cmd)
        .ok()
        .and_then(|v| v.get("op").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| "edit".to_owned())
}

fn walk(cmd: &Command, acc: &mut Acc) {
    match cmd {
        Command::Batch { commands, .. } => {
            for c in commands {
                walk(c, acc);
            }
        }
        Command::AddNotes { clip, notes } => {
            let c = acc.clips.entry(clip.to_string()).or_default();
            c.added.extend(notes.iter().map(|n| n.id.to_string()));
        }
        Command::RemoveNotes { clip, ids } => {
            let c = acc.clips.entry(clip.to_string()).or_default();
            c.removed.extend(ids.iter().map(|n| n.to_string()));
        }
        Command::UpdateNotes { clip, changes } => {
            let c = acc.clips.entry(clip.to_string()).or_default();
            c.changed.extend(changes.iter().map(|n| n.id.to_string()));
        }
        other => {
            let op = op_name(other);
            for t in other.targets() {
                match t {
                    Target::Track(id) => {
                        acc.tracks
                            .entry(id.to_string())
                            .or_default()
                            .insert(op.clone());
                    }
                    Target::Clip(id) => {
                        acc.clips
                            .entry(id.to_string())
                            .or_default()
                            .ops
                            .insert(op.clone());
                    }
                    Target::Effect(id) => {
                        acc.effects
                            .entry(id.to_string())
                            .or_default()
                            .insert(op.clone());
                    }
                    Target::Note(_) | Target::Asset(_) => {}
                    Target::Tempo
                    | Target::TimeSig
                    | Target::Master
                    | Target::Meta
                    | Target::Sections => {
                        acc.global.insert(op.clone());
                    }
                }
            }
        }
    }
}

fn ids(v: &[String]) -> Value {
    if v.len() > MAX_IDS {
        json!(v[v.len() - MAX_IDS..])
    } else {
        json!(v)
    }
}

/// 履歴エントリ(古い → 新しい)の変更を要約する。名前は今のプロジェクトから引く
/// (消えたクリップ・トラックは名前なし)。
pub fn summarize(entries: &[&HistoryEntry], project: &Project) -> Value {
    let mut acc = Acc::default();
    for e in entries {
        walk(&e.forward, &mut acc);
    }
    let clip_info = |id: &str| {
        project.tracks.iter().find_map(|t| {
            t.clips
                .iter()
                .find(|c| c.id.as_str() == id)
                .map(|c| (t.name.clone(), c.name.clone(), c.start.0))
        })
    };
    let clips: Vec<Value> = acc
        .clips
        .iter()
        .map(|(id, c)| {
            let mut v = json!({ "clip_id": id });
            match clip_info(id) {
                Some((track, name, start)) => {
                    v["track"] = json!(track);
                    v["name"] = json!(name);
                    v["start_tick"] = json!(start);
                }
                None => v["exists"] = json!(false),
            }
            if !c.added.is_empty() {
                v["notes_added"] = json!(c.added.len());
                v["added_ids"] = ids(&c.added);
            }
            if !c.removed.is_empty() {
                v["notes_removed"] = json!(c.removed.len());
                v["removed_ids"] = ids(&c.removed);
            }
            if !c.changed.is_empty() {
                let changed: Vec<String> = c.changed.iter().cloned().collect();
                v["notes_changed"] = json!(changed.len());
                v["changed_ids"] = ids(&changed);
            }
            if !c.ops.is_empty() {
                v["ops"] = json!(c.ops);
            }
            v
        })
        .collect();
    let track_name = |id: &str| {
        project
            .tracks
            .iter()
            .find(|t| t.id.as_str() == id)
            .map(|t| t.name.clone())
    };
    let tracks: Vec<Value> = acc
        .tracks
        .iter()
        .map(|(id, ops)| json!({ "track_id": id, "name": track_name(id), "ops": ops }))
        .collect();
    let effects: Vec<Value> = acc
        .effects
        .iter()
        .map(|(id, ops)| json!({ "effect_id": id, "ops": ops }))
        .collect();
    json!({
        "entries": entries.iter().map(|e| json!({
            "id": e.id,
            "author": e.author,
            "label": e.label,
        })).collect::<Vec<_>>(),
        "clips": clips,
        "tracks": tracks,
        "effects": effects,
        "global": acc.global,
    })
}

/// 要約を短い文にする(チャットに添える用。1 行 1 項目、最大 `max` 行)
pub fn summary_lines(summary: &Value, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    for c in summary["clips"].as_array().into_iter().flatten() {
        let name = match (c["track"].as_str(), c["name"].as_str()) {
            (Some(t), Some(n)) => format!("「{t}」のクリップ「{n}」"),
            _ => "(消えた)クリップ".to_owned(),
        };
        let mut parts = Vec::new();
        for (key, word) in [
            ("notes_added", "ノート追加"),
            ("notes_removed", "ノート削除"),
            ("notes_changed", "ノート変更"),
        ] {
            if let Some(n) = c[key].as_u64() {
                parts.push(format!("{word} {n}"));
            }
        }
        if let Some(ops) = c["ops"].as_array() {
            parts.extend(ops.iter().filter_map(Value::as_str).map(str::to_owned));
        }
        out.push(format!(
            "- {name}({}): {}",
            c["clip_id"].as_str().unwrap_or(""),
            parts.join(" / ")
        ));
    }
    for t in summary["tracks"].as_array().into_iter().flatten() {
        let ops: Vec<&str> = t["ops"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        out.push(format!(
            "- トラック「{}」({}): {}",
            t["name"].as_str().unwrap_or("(消えた)"),
            t["track_id"].as_str().unwrap_or(""),
            ops.join(" / ")
        ));
    }
    let global: Vec<&str> = summary["global"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if !global.is_empty() {
        out.push(format!("- 曲全体: {}", global.join(" / ")));
    }
    if out.len() > max {
        let rest = out.len() - max;
        out.truncate(max);
        out.push(format!("- ほか {rest} 件"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::{
        Articulation, Author, Clip, ClipId, Note, NoteChange, NoteId, Session, Tick, Track,
        TrackId, TrackKind,
    };

    #[test]
    fn summarizes_note_and_track_changes() {
        let mut s = Session::new(Project::new("t"));
        let tid = TrackId::new();
        let cid = ClipId::new();
        s.apply(
            Command::AddTrack {
                track: Track::new(tid.clone(), "Lead", TrackKind::Midi),
                index: None,
            },
            Author::Human,
            "トラック",
        )
        .unwrap();
        s.apply(
            Command::AddClip {
                track: tid.clone(),
                clip: Clip::new_midi(cid.clone(), "サビ", Tick(0), Tick(3840)),
            },
            Author::Human,
            "クリップ",
        )
        .unwrap();
        let since = s.history().applied().last().unwrap().id.clone();
        let n = Note {
            id: NoteId::new(),
            pos: Tick(0),
            dur: Tick(480),
            pitch: 60,
            vel: 100,
            articulation: Articulation::Normal,
            pitch_curve: vec![],
            glide_ms: None,
        };
        let nid = n.id.clone();
        s.apply(
            Command::AddNotes {
                clip: cid.clone(),
                notes: vec![n],
            },
            Author::Human,
            "音",
        )
        .unwrap();
        let mut ch = NoteChange::new(nid);
        ch.pitch = Some(62);
        s.apply(
            Command::UpdateNotes {
                clip: cid.clone(),
                changes: vec![ch],
            },
            Author::Human,
            "移調",
        )
        .unwrap();
        s.apply(
            Command::SetTrackProp {
                id: tid,
                prop: glaux_core::TrackProp::VolumeDb(-3.0),
            },
            Author::Human,
            "音量",
        )
        .unwrap();

        let applied = s.history().applied();
        let k = applied.iter().position(|e| e.id == since).unwrap() + 1;
        let entries: Vec<&HistoryEntry> = applied[k..].iter().collect();
        let v = summarize(&entries, s.project());
        assert_eq!(v["entries"].as_array().unwrap().len(), 3);
        let clip = &v["clips"][0];
        assert_eq!(clip["name"], "サビ");
        assert_eq!(clip["notes_added"], 1);
        assert_eq!(clip["notes_changed"], 1);
        assert_eq!(v["tracks"][0]["name"], "Lead");
        let lines = summary_lines(&v, 10);
        assert!(lines[0].contains("「Lead」のクリップ「サビ」"), "{lines:?}");
        assert!(lines[0].contains("ノート追加 1"), "{lines:?}");
    }
}
