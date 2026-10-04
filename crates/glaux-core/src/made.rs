//! AI が作ったときの中身の指紋と、手直し・固定の見分け(曲の設計データの段階 0)。
//!
//! - AI の編集のたびに、ノートが変わったクリップの中身を小節ごとの指紋([`Made`])にして、同じ履歴の 1 件に記録する
//!   ([`for_ai`]。セッションのアクターが AI の編集に掛ける)。AI が中身を変えなかった小節は前の指紋を引き継ぐので、
//!   人が手で直した小節は、AI が別の所を直した後も「手で直した」のまま残る
//! - 今の中身と指紋が違う小節 = 人が手で直した所([`edited_spans`])。指紋の記録が無いクリップ
//!   (この仕組みより前の曲・人が作ったクリップ)は見分けられないので、手直しは無いものとして扱う
//! - 固定の音(`Note::locked`)は、AI の編集で変えない([`guard_locks`]。消す・変える・クリップごと消す・切り詰めるを外す)
//! - 作り直し(クリップの差し替え)では、手で直した小節の音を残す([`protect_edits`]。既定。
//!   「手直しも含めて上書き」のときは使わない)

use crate::command::Command;
use crate::id::{ClipId, NoteId};
use crate::model::{Clip, Note, Project};
use crate::time::Tick;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// クリップ 1 つの、AI が作った・直したときの中身の指紋
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct Made {
    /// 小節ごとの指紋(位置はクリップの頭から。曲の小節の境目で切る)
    pub spans: Vec<MadeSpan>,
}

/// 1 小節ぶんの指紋
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct MadeSpan {
    pub start: Tick,
    pub end: Tick,
    /// その小節で始まる音の中身(ID と固定は含めない。位置は小節の頭から)の指紋。16 桁の 16 進
    pub hash: String,
}

fn fnv(parts: &[String]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for &b in p.as_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// 曲の頭からの区間 [a, b) で始まる音の中身の指紋(`notes` の位置は曲の頭から)
fn hash_range(notes: &[(u64, &Note)], a: u64, b: u64) -> String {
    let mut parts: Vec<String> = notes
        .iter()
        .filter(|(t, _)| *t >= a && *t < b)
        .map(|(t, n)| {
            let mut n = (*n).clone();
            n.id = NoteId::parse("nt_x").expect("固定の ID");
            n.locked = false;
            n.pos = Tick(t - a);
            serde_json::to_string(&n).unwrap_or_default()
        })
        .collect();
    parts.sort();
    fnv(&parts)
}

/// クリップの鳴る音(位置は曲の頭から。クリップの長さより後ろの音は鳴らないので含めない)
fn abs_notes(clip: &Clip) -> Vec<(u64, &Note)> {
    clip.notes()
        .unwrap_or(&[])
        .iter()
        .filter(|n| n.pos < clip.length)
        .map(|n| (clip.start.0 + n.pos.0, n))
        .collect()
}

/// クリップの範囲を曲の小節の境目で切る(曲の頭からの [a, b))
fn bar_cuts(project: &Project, clip: &Clip) -> Vec<(u64, u64)> {
    let (s, e) = (clip.start.0, clip.end().0);
    if e <= s {
        return vec![];
    }
    let grid = crate::arrange::bar_grid(project, e);
    let mut out = Vec::new();
    for (bs, len) in grid {
        let (a, b) = (bs.max(s), (bs + len).min(e));
        if a < b {
            out.push((a, b));
        }
        if bs + len >= e {
            break;
        }
    }
    if out.is_empty() {
        out.push((s, e));
    }
    out
}

/// 今のクリップの中身から指紋を作る(MIDI クリップでなければ None)
pub fn fingerprint(project: &Project, clip: &Clip) -> Option<Made> {
    clip.notes()?;
    let notes = abs_notes(clip);
    Some(Made {
        spans: bar_cuts(project, clip)
            .into_iter()
            .map(|(a, b)| MadeSpan {
                start: Tick(a - clip.start.0),
                end: Tick(b - clip.start.0),
                hash: hash_range(&notes, a, b),
            })
            .collect(),
    })
}

/// 人が手で直した範囲(曲の頭からの [a, b))。指紋の記録が無いクリップは空。
/// 記録より後ろ(人がクリップを伸ばして書き足した所)に音があれば、そこも手で直した所
pub fn edited_spans(project: &Project, clip_id: &ClipId) -> Vec<(u64, u64)> {
    let (Some(made), Some((_, clip))) = (project.made.get(clip_id), project.clip(clip_id)) else {
        return vec![];
    };
    if clip.notes().is_none() {
        return vec![];
    }
    let notes = abs_notes(clip);
    let base = clip.start.0;
    let mut out: Vec<(u64, u64)> = made
        .spans
        .iter()
        .filter(|s| s.start < clip.length)
        .filter_map(|s| {
            let (a, b) = (base + s.start.0, base + s.end.0.min(clip.length.0));
            (hash_range(&notes, a, base + s.end.0) != s.hash).then_some((a, b))
        })
        .collect();
    let covered = made.spans.iter().map(|s| s.end.0).max().unwrap_or(0);
    if covered < clip.length.0 {
        let (a, b) = (base + covered, clip.end().0);
        if notes.iter().any(|(t, _)| *t >= a && *t < b) {
            out.push((a, b));
        }
    }
    merge(out)
}

fn merge(mut v: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    v.sort_unstable();
    let mut out: Vec<(u64, u64)> = Vec::new();
    for (a, b) in v {
        match out.last_mut() {
            Some(l) if a <= l.1 => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// AI が直した後の指紋。AI が中身を変えなかった小節は前の記録を引き継ぐ(手で直した小節は手で直したまま)。
/// 前の記録が無いクリップは、全部の小節を今の中身で記録する
fn merged_fingerprint(before: &Project, after: &Project, clip_id: &ClipId) -> Option<Made> {
    let (_, clip) = after.clip(clip_id)?;
    let fresh = fingerprint(after, clip)?;
    let (Some(old), Some((_, old_clip))) = (before.made.get(clip_id), before.clip(clip_id)) else {
        return Some(fresh);
    };
    let old_notes = abs_notes(old_clip);
    // 前の記録を曲の頭からの位置で引けるように
    let old_spans: BTreeMap<(u64, u64), &str> = old
        .spans
        .iter()
        .map(|s| {
            (
                (old_clip.start.0 + s.start.0, old_clip.start.0 + s.end.0),
                s.hash.as_str(),
            )
        })
        .collect();
    let new_notes = abs_notes(clip);
    let spans = fresh
        .spans
        .into_iter()
        .map(|s| {
            let (a, b) = (clip.start.0 + s.start.0, clip.start.0 + s.end.0);
            let unchanged = hash_range(&old_notes, a, b) == hash_range(&new_notes, a, b);
            match old_spans.get(&(a, b)) {
                Some(h) if unchanged => MadeSpan {
                    hash: (*h).to_owned(),
                    ..s
                },
                _ => s,
            }
        })
        .collect();
    Some(Made { spans })
}

// ---------------------------------------------------------------- 固定

/// 固定の音を守るために外した編集
#[derive(Clone, PartialEq, Debug, Serialize)]
pub struct LockHit {
    pub clip: ClipId,
    /// 守った固定の音
    pub notes: Vec<NoteId>,
    /// 外した編集(remove_notes / update_notes / replace_clip / remove_clip / remove_track / resize_clip / split_clip)
    pub op: &'static str,
    pub what: String,
}

/// 曲の中の固定の音(クリップごと)
fn locked_notes(project: &Project) -> BTreeMap<ClipId, Vec<&Note>> {
    let mut out: BTreeMap<ClipId, Vec<&Note>> = BTreeMap::new();
    for t in &project.tracks {
        for c in &t.clips {
            let l: Vec<&Note> = c
                .notes()
                .unwrap_or(&[])
                .iter()
                .filter(|n| n.locked)
                .collect();
            if !l.is_empty() {
                out.insert(c.id.clone(), l);
            }
        }
    }
    out
}

/// AI の編集から、固定の音を変える部分を外す。全部外れたら None。
/// 固定の音を足す(locked: true にする)編集はそのまま通す。外す(locked: false)編集は外す
pub fn guard_locks(project: &Project, cmd: Command) -> (Option<Command>, Vec<LockHit>) {
    let locked = locked_notes(project);
    let mut hits = Vec::new();
    if locked.is_empty() {
        return (Some(cmd), hits);
    }
    let out = guard_one(project, &locked, cmd, &mut hits);
    (out, hits)
}

fn guard_one(
    project: &Project,
    locked: &BTreeMap<ClipId, Vec<&Note>>,
    cmd: Command,
    hits: &mut Vec<LockHit>,
) -> Option<Command> {
    let ids_of = |clip: &ClipId| -> BTreeSet<NoteId> {
        locked
            .get(clip)
            .map(|v| v.iter().map(|n| n.id.clone()).collect())
            .unwrap_or_default()
    };
    match cmd {
        Command::Batch { commands, label } => {
            let kept: Vec<Command> = commands
                .into_iter()
                .filter_map(|c| guard_one(project, locked, c, hits))
                .collect();
            (!kept.is_empty()).then_some(Command::Batch {
                commands: kept,
                label,
            })
        }
        Command::RemoveNotes { clip, ids } => {
            let l = ids_of(&clip);
            let (keep, cut): (Vec<NoteId>, Vec<NoteId>) =
                ids.into_iter().partition(|i| !l.contains(i));
            if !cut.is_empty() {
                hits.push(LockHit {
                    clip: clip.clone(),
                    what: format!("固定の音 {} 個は消さなかった", cut.len()),
                    notes: cut,
                    op: "remove_notes",
                });
            }
            (!keep.is_empty()).then_some(Command::RemoveNotes { clip, ids: keep })
        }
        Command::UpdateNotes { clip, changes } => {
            let l = ids_of(&clip);
            let mut cut = Vec::new();
            let keep: Vec<_> = changes
                .into_iter()
                .filter(|c| {
                    if !l.contains(&c.id) {
                        return true;
                    }
                    // 固定のままにする変更(locked: true だけ)は通す
                    let only_lock = {
                        let mut probe = crate::command::NoteChange::new(c.id.clone());
                        probe.locked = c.locked;
                        &probe == c && c.locked != Some(false)
                    };
                    if !only_lock {
                        cut.push(c.id.clone());
                    }
                    only_lock
                })
                .collect();
            if !cut.is_empty() {
                hits.push(LockHit {
                    clip: clip.clone(),
                    what: format!("固定の音 {} 個は変えなかった", cut.len()),
                    notes: cut,
                    op: "update_notes",
                });
            }
            (!keep.is_empty()).then_some(Command::UpdateNotes {
                clip,
                changes: keep,
            })
        }
        Command::ReplaceClip { id, mut clip } => {
            let (Some(l), Some((_, old))) = (locked.get(&id), project.clip(&id)) else {
                return Some(Command::ReplaceClip { id, clip });
            };
            let kept = keep_notes(&mut clip, old, l.iter().copied());
            if kept > 0 {
                hits.push(LockHit {
                    clip: id.clone(),
                    notes: l.iter().map(|n| n.id.clone()).collect(),
                    op: "replace_clip",
                    what: format!("差し替えでも固定の音 {kept} 個はそのまま残した"),
                });
            }
            Some(Command::ReplaceClip { id, clip })
        }
        Command::RemoveClip { id } => match locked.get(&id) {
            Some(l) => {
                hits.push(LockHit {
                    clip: id.clone(),
                    notes: l.iter().map(|n| n.id.clone()).collect(),
                    op: "remove_clip",
                    what: format!("固定の音が {} 個あるクリップは消さなかった", l.len()),
                });
                None
            }
            None => Some(Command::RemoveClip { id }),
        },
        Command::RemoveTrack { id } => {
            let blocked: Vec<(ClipId, Vec<NoteId>)> = project
                .track(&id)
                .map(|t| {
                    t.clips
                        .iter()
                        .filter_map(|c| {
                            locked
                                .get(&c.id)
                                .map(|l| (c.id.clone(), l.iter().map(|n| n.id.clone()).collect()))
                        })
                        .collect()
                })
                .unwrap_or_default();
            if blocked.is_empty() {
                return Some(Command::RemoveTrack { id });
            }
            for (clip, notes) in blocked {
                hits.push(LockHit {
                    clip,
                    what: "固定の音があるトラックは消さなかった".to_owned(),
                    notes,
                    op: "remove_track",
                });
            }
            None
        }
        Command::ResizeClip { id, length } => {
            let cut: Vec<NoteId> = locked
                .get(&id)
                .map(|l| {
                    l.iter()
                        .filter(|n| n.end() > length)
                        .map(|n| n.id.clone())
                        .collect()
                })
                .unwrap_or_default();
            if cut.is_empty() {
                return Some(Command::ResizeClip { id, length });
            }
            hits.push(LockHit {
                clip: id.clone(),
                what: "固定の音が切れるので、クリップを縮めなかった".to_owned(),
                notes: cut,
                op: "resize_clip",
            });
            None
        }
        Command::SplitClip { id, at, new_id } => {
            let start = project.clip(&id).map_or(0, |(_, c)| c.start.0);
            let cut: Vec<NoteId> = locked
                .get(&id)
                .map(|l| {
                    l.iter()
                        .filter(|n| start + n.pos.0 < at.0 && start + n.end().0 > at.0)
                        .map(|n| n.id.clone())
                        .collect()
                })
                .unwrap_or_default();
            if cut.is_empty() {
                return Some(Command::SplitClip { id, at, new_id });
            }
            hits.push(LockHit {
                clip: id.clone(),
                what: "分ける位置が固定の音の途中なので、分けなかった".to_owned(),
                notes: cut,
                op: "split_clip",
            });
            None
        }
        other => Some(other),
    }
}

/// 差し替えのクリップ `clip` に、元のクリップ `old` の音 `keep` をそのまま入れる(位置は曲の上で同じ所)。
/// 入れる音と同じ高さで重なる新しい音は外す。クリップが入れる音を覆わなければ広げる。入れた数を返す
fn keep_notes<'a>(clip: &mut Clip, old: &Clip, keep: impl Iterator<Item = &'a Note>) -> usize {
    let keep: Vec<(u64, Note)> = keep.map(|n| (old.start.0 + n.pos.0, n.clone())).collect();
    if keep.is_empty() || clip.notes().is_none() {
        return 0;
    }
    // 覆わなければ広げる
    let lo = keep.iter().map(|(t, _)| *t).min().unwrap_or(clip.start.0);
    let hi = keep
        .iter()
        .map(|(t, n)| t + n.dur.0)
        .max()
        .unwrap_or(clip.end().0);
    if lo < clip.start.0 {
        let d = clip.start.0 - lo;
        if let Some(ns) = clip.notes_mut() {
            for n in ns.iter_mut() {
                n.pos = Tick(n.pos.0 + d);
            }
        }
        clip.start = Tick(lo);
        clip.length = Tick(clip.length.0 + d);
    }
    if hi > clip.end().0 {
        clip.length = Tick(hi - clip.start.0);
    }
    let start = clip.start.0;
    let ids: BTreeSet<NoteId> = keep.iter().map(|(_, n)| n.id.clone()).collect();
    let Some(ns) = clip.notes_mut() else {
        return 0;
    };
    ns.retain(|n| {
        if ids.contains(&n.id) {
            return false;
        }
        let (a, b) = (start + n.pos.0, start + n.end().0);
        !keep
            .iter()
            .any(|(t, k)| k.pitch == n.pitch && a < t + k.dur.0 && *t < b)
    });
    for (t, mut n) in keep.iter().cloned() {
        n.pos = Tick(t - start);
        ns.push(n);
    }
    crate::model::sort_notes(ns);
    keep.len()
}

// ---------------------------------------------------------------- 手直しを残す作り直し

/// 作り直しで残した、人が手で直した所
#[derive(Clone, PartialEq, Debug, Serialize)]
pub struct Protected {
    pub clip: ClipId,
    /// 残した小節(1 始まり、[最初, 最後])
    pub bars: Vec<[u32; 2]>,
    /// 残した音の数
    pub notes: usize,
}

/// 作り直し(クリップの差し替え)で、人が手で直した小節の音を残す。
/// 差し替えの新しい音のうち、手で直した小節で始まる音を外し、元の音を入れる
pub fn protect_edits(project: &Project, cmd: Command) -> (Command, Vec<Protected>) {
    let mut out = Vec::new();
    let cmd = protect_one(project, cmd, &mut out);
    (cmd, out)
}

fn protect_one(project: &Project, cmd: Command, out: &mut Vec<Protected>) -> Command {
    match cmd {
        Command::Batch { commands, label } => Command::Batch {
            commands: commands
                .into_iter()
                .map(|c| protect_one(project, c, out))
                .collect(),
            label,
        },
        Command::ReplaceClip { id, mut clip } => {
            let spans = edited_spans(project, &id);
            let Some((_, old)) = project.clip(&id) else {
                return Command::ReplaceClip { id, clip };
            };
            if spans.is_empty() || clip.notes().is_none() {
                return Command::ReplaceClip { id, clip };
            }
            let inside = |t: u64| spans.iter().any(|&(a, b)| a <= t && t < b);
            // 新しい音のうち、手で直した小節で始まるものを外す
            let start = clip.start.0;
            if let Some(ns) = clip.notes_mut() {
                ns.retain(|n| !inside(start + n.pos.0));
            }
            let keep: Vec<&Note> = old
                .notes()
                .unwrap_or(&[])
                .iter()
                .filter(|n| n.pos < old.length && inside(old.start.0 + n.pos.0))
                .collect();
            let n = keep_notes(&mut clip, old, keep.into_iter());
            out.push(Protected {
                clip: id.clone(),
                bars: spans.iter().map(|&(a, b)| bars_of(project, a, b)).collect(),
                notes: n,
            });
            Command::ReplaceClip { id, clip }
        }
        other => other,
    }
}

/// 曲の頭からの [a, b) を小節(1 始まり、[最初, 最後])に
pub fn bars_of(project: &Project, a: u64, b: u64) -> [u32; 2] {
    let grid = crate::arrange::bar_grid(project, b.max(a + 1));
    let bar = |t: u64| grid.partition_point(|(s, _)| *s <= t).max(1) as u32;
    [bar(a), bar(b.saturating_sub(1).max(a))]
}

// ---------------------------------------------------------------- AI の編集に掛ける

/// AI の編集の結果
pub struct AiEdit {
    /// 適用するコマンド(固定を守った上で、指紋の記録を足したもの)。全部が固定に当たって外れたら None
    pub command: Option<Command>,
    pub locks: Vec<LockHit>,
}

/// AI の編集に掛ける: 固定の音を守り、ノートが変わったクリップの指紋を同じ 1 件に記録する。
/// 指紋は、曲の複製に一度当ててみた結果から作る(当てられないコマンドはそのまま返し、本当の適用で失敗させる)
pub fn for_ai(project: &Project, cmd: Command) -> AiEdit {
    let (cmd, locks) = guard_locks(project, cmd);
    let Some(cmd) = cmd else {
        return AiEdit {
            command: None,
            locks,
        };
    };
    let mut sim = project.clone();
    let Ok(applied) = sim.apply(&cmd) else {
        return AiEdit {
            command: Some(cmd),
            locks,
        };
    };
    let mut clips: BTreeSet<ClipId> = BTreeSet::new();
    for c in &applied.changes {
        match c {
            crate::apply::Change::NotesChanged { clip } => {
                clips.insert(clip.clone());
            }
            crate::apply::Change::ClipsChanged { track } => {
                for p in [project, &sim] {
                    if let Some(t) = p.track(track) {
                        clips.extend(t.clips.iter().map(|c| c.id.clone()));
                    }
                }
            }
            _ => {}
        }
    }
    let mut extra = Vec::new();
    for id in clips {
        let Some((_, c)) = sim.clip(&id) else {
            continue;
        };
        if c.notes().is_none() {
            continue;
        }
        // 中身が前と同じクリップ(同じトラックのほかのクリップ)は、記録が無ければ付けない
        let same = project.clip(&id).is_some_and(|(_, o)| o == c);
        if same && !project.made.contains_key(&id) {
            continue;
        }
        let made = merged_fingerprint(project, &sim, &id);
        if made.as_ref() != sim.made.get(&id) {
            extra.push(Command::SetClipMade {
                clip: id.clone(),
                made,
            });
        }
    }
    if extra.is_empty() {
        return AiEdit {
            command: Some(cmd),
            locks,
        };
    }
    let label = match &cmd {
        Command::Batch { label, .. } => label.clone(),
        _ => String::new(),
    };
    let mut commands = match cmd {
        Command::Batch { commands, .. } => commands,
        c => vec![c],
    };
    commands.extend(extra);
    AiEdit {
        command: Some(Command::Batch { commands, label }),
        locks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::NoteChange;
    use crate::id::TrackId;
    use crate::model::{Track, TrackKind};

    fn note(pos: u64, pitch: u8) -> Note {
        Note {
            id: NoteId::new(),
            pos: Tick(pos),
            dur: Tick(240),
            pitch,
            vel: 100,
            articulation: Default::default(),
            pitch_curve: vec![],
            glide_ms: None,
            vibrato: None,
            volume_curve: vec![],
            brightness_curve: vec![],
            condition: None,
            locked: false,
        }
    }

    /// 4 小節(1 小節 3840 tick)のクリップ 1 つの曲。小節ごとに 2 音
    fn song() -> (Project, ClipId) {
        let mut p = Project::new("t");
        let tid = TrackId::new();
        let mut t = Track::new(tid.clone(), "lead", TrackKind::Midi);
        let cid = ClipId::new();
        let mut c = Clip::new_midi(cid.clone(), "c", Tick(3840), Tick(3840 * 4));
        if let Some(ns) = c.notes_mut() {
            for b in 0..4u64 {
                ns.push(note(b * 3840, 60 + b as u8));
                ns.push(note(b * 3840 + 1920, 64 + b as u8));
            }
        }
        t.clips.push(c);
        p.tracks.push(t);
        (p, cid)
    }

    fn ai(p: &mut Project, cmd: Command) -> Vec<LockHit> {
        let e = for_ai(p, cmd);
        if let Some(c) = e.command {
            p.apply(&c).unwrap();
        }
        e.locks
    }

    fn notes_of(p: &Project, c: &ClipId) -> Vec<Note> {
        p.clip(c).unwrap().1.notes().unwrap().to_vec()
    }

    #[test]
    fn ai_edits_are_fingerprinted_and_hand_edits_show_up() {
        let (mut p, cid) = song();
        // AI が 1 音変える → クリップ全体の指紋ができる(記録が無かったので全部の小節)
        let n0 = notes_of(&p, &cid)[0].id.clone();
        ai(
            &mut p,
            Command::UpdateNotes {
                clip: cid.clone(),
                changes: vec![NoteChange::new(n0).vel(90)],
            },
        );
        assert_eq!(p.made[&cid].spans.len(), 4);
        assert!(edited_spans(&p, &cid).is_empty());
        // 人が 3 小節目の音を変える → そこだけ手で直した所
        let n = notes_of(&p, &cid)[4].id.clone();
        p.apply(&Command::UpdateNotes {
            clip: cid.clone(),
            changes: vec![NoteChange::new(n).pitch(70)],
        })
        .unwrap();
        let e = edited_spans(&p, &cid);
        assert_eq!(e, vec![(3840 * 3, 3840 * 4)], "{e:?}");
        assert_eq!(bars_of(&p, e[0].0, e[0].1), [4, 4]);
        // AI が 1 小節目を直しても、3 小節目は手で直したまま
        let n1 = notes_of(&p, &cid)[1].id.clone();
        ai(
            &mut p,
            Command::UpdateNotes {
                clip: cid.clone(),
                changes: vec![NoteChange::new(n1).pitch(65)],
            },
        );
        assert_eq!(edited_spans(&p, &cid), vec![(3840 * 3, 3840 * 4)]);
        // クリップを動かしても(中身は同じ)手直しの見分けは変わらない
        p.apply(&Command::MoveClip {
            id: cid.clone(),
            start: Tick(3840 * 2),
            track: None,
        })
        .unwrap();
        assert_eq!(edited_spans(&p, &cid), vec![(3840 * 4, 3840 * 5)]);
    }

    #[test]
    fn regenerating_keeps_hand_edited_bars() {
        let (mut p, cid) = song();
        let (_, c) = p.clip(&cid).unwrap();
        let made = fingerprint(&p, c);
        p.apply(&Command::SetClipMade {
            clip: cid.clone(),
            made,
        })
        .unwrap();
        // 人が 2 小節目に音を足す
        let added = note(3840 + 960, 72);
        p.apply(&Command::AddNotes {
            clip: cid.clone(),
            notes: vec![added.clone()],
        })
        .unwrap();
        // AI が作り直す: 全部の音を別の高さに
        let mut new = p.clip(&cid).unwrap().1.clone();
        if let Some(ns) = new.notes_mut() {
            *ns = (0..4u64).map(|b| note(b * 3840, 48)).collect();
        }
        let (cmd, prot) = protect_edits(
            &p,
            Command::ReplaceClip {
                id: cid.clone(),
                clip: new,
            },
        );
        assert_eq!(prot.len(), 1);
        assert_eq!(prot[0].bars, vec![[3, 3]]);
        assert_eq!(prot[0].notes, 3);
        ai(&mut p, cmd);
        let ns = notes_of(&p, &cid);
        // 2 小節目(クリップの中の位置 3840〜7680)は人が直したまま
        let bar2: Vec<u8> = ns
            .iter()
            .filter(|n| (3840..7680).contains(&n.pos.0))
            .map(|n| n.pitch)
            .collect();
        assert_eq!(bar2, vec![61, 72, 65]);
        // ほかの小節は作り直した
        assert!(ns
            .iter()
            .filter(|n| !(3840..7680).contains(&n.pos.0))
            .all(|n| n.pitch == 48));
        // 作り直した後も、残した小節は手で直した所のまま
        assert_eq!(edited_spans(&p, &cid), vec![(3840 * 2, 3840 * 3)]);
    }

    #[test]
    fn locked_notes_survive_ai_edits() {
        let (mut p, cid) = song();
        let ns = notes_of(&p, &cid);
        let (a, b) = (ns[0].id.clone(), ns[1].id.clone());
        p.apply(&Command::UpdateNotes {
            clip: cid.clone(),
            changes: vec![NoteChange {
                locked: Some(true),
                ..NoteChange::new(a.clone())
            }],
        })
        .unwrap();
        // 消す・変える: 固定の音だけ外れる
        let hits = ai(
            &mut p,
            Command::RemoveNotes {
                clip: cid.clone(),
                ids: vec![a.clone(), b.clone()],
            },
        );
        assert_eq!(hits.len(), 1);
        assert!(notes_of(&p, &cid).iter().any(|n| n.id == a));
        assert!(!notes_of(&p, &cid).iter().any(|n| n.id == b));
        let e = for_ai(
            &p,
            Command::UpdateNotes {
                clip: cid.clone(),
                changes: vec![NoteChange::new(a.clone()).pitch(30)],
            },
        );
        assert!(e.command.is_none());
        // クリップごと消す・固定の音を切る縮め方はしない
        assert!(for_ai(&p, Command::RemoveClip { id: cid.clone() })
            .command
            .is_none());
        assert!(for_ai(
            &p,
            Command::ResizeClip {
                id: cid.clone(),
                length: Tick(100)
            }
        )
        .command
        .is_none());
        // 差し替えでも残る(同じ高さで重なる新しい音は外す)
        let mut new = p.clip(&cid).unwrap().1.clone();
        if let Some(ns) = new.notes_mut() {
            *ns = vec![note(0, 60), note(0, 50)];
        }
        let hits = ai(
            &mut p,
            Command::ReplaceClip {
                id: cid.clone(),
                clip: new,
            },
        );
        assert_eq!(hits[0].op, "replace_clip");
        let ns = notes_of(&p, &cid);
        assert_eq!(ns.len(), 2, "{ns:?}");
        assert!(ns.iter().any(|n| n.id == a && n.locked && n.pitch == 60));
        // 人は固定の音も変えられる(AI の編集でなければ通る)
        p.apply(&Command::UpdateNotes {
            clip: cid.clone(),
            changes: vec![NoteChange::new(a.clone()).pitch(61)],
        })
        .unwrap();
        // AI が固定を外すことはできない。固定にすることはできる
        let other = notes_of(&p, &cid).into_iter().find(|n| n.id != a).unwrap();
        let e = for_ai(
            &p,
            Command::UpdateNotes {
                clip: cid.clone(),
                changes: vec![
                    NoteChange {
                        locked: Some(false),
                        ..NoteChange::new(a.clone())
                    },
                    NoteChange {
                        locked: Some(true),
                        ..NoteChange::new(other.id.clone())
                    },
                ],
            },
        );
        assert_eq!(e.locks.len(), 1);
        p.apply(&e.command.unwrap()).unwrap();
        assert!(notes_of(&p, &cid).iter().all(|n| n.locked));
    }
}
