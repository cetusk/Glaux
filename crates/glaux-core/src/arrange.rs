//! 構成の編集(小節の挿入・削除、クリップの複製)を [`Command`] の列に組み立てる。
//!
//! AI がノートやクリップを 1 つずつ組み立てると、呼び出し回数・ID の書き損じ・トークンが増える。
//! ここで「今のプロジェクト」から絶対値のコマンド列を作り、呼び出し側(MCP)が Batch 1 件として
//! 適用する(1 回の undo で戻る)。新規 ID はここ(コマンドを作る側)で振る。

use crate::command::Command;
use crate::id::{ClipId, NoteId};
use crate::model::{AutomationPoint, ClipContent, Project, SectionMarker};
use crate::time::{Tick, TimeSigEvent};

/// (小節頭 tick, 小節長) の列を拍子マップから作る(`end_tick` を越えるまで)。
pub fn bar_grid(project: &Project, end_tick: u64) -> Vec<(u64, u64)> {
    let ppq = crate::time::PPQ;
    let mut sigs: Vec<TimeSigEvent> = project.time_sig_map.clone();
    sigs.sort_by_key(|s| s.tick);
    if sigs.is_empty() {
        sigs.push(TimeSigEvent {
            tick: Tick(0),
            num: 4,
            den: 4,
        });
    }
    let mut out = Vec::new();
    for (i, sig) in sigs.iter().enumerate() {
        let seg_end = sigs.get(i + 1).map(|s| s.tick.0).unwrap_or(u64::MAX);
        let bar_len = (ppq * 4 * sig.num as u64 / sig.den.max(1) as u64).max(1);
        let mut t = sig.tick.0;
        while t < seg_end {
            let len = bar_len.min(seg_end - t);
            out.push((t, len));
            t += len;
            if seg_end == u64::MAX && t >= end_tick {
                return out;
            }
            if out.len() > 100_000 {
                return out; // 安全弁
            }
        }
    }
    out
}

/// 小節(1 始まり)`bar` から `count` 小節ぶんの (開始 tick, 長さ)。曲の終わりより後ろでもよい。
pub fn bar_range(project: &Project, bar: u32, count: u32) -> Option<(u64, u64)> {
    if bar == 0 || count == 0 {
        return None;
    }
    let first = bar as usize - 1;
    let last = first + count as usize; // この小節の頭まで
                                       // 必要な小節数まで伸ばして求める(end_tick は十分大きく)
    let mut end = project.end().0.max(3840) * 2;
    loop {
        let grid = bar_grid(project, end);
        if grid.len() > last {
            let start = grid[first].0;
            return Some((start, grid[last].0 - start));
        }
        if grid.len() >= 100_000 {
            return None;
        }
        end = end.saturating_mul(2);
    }
}

fn shift_points(
    points: &[AutomationPoint],
    f: impl Fn(u64) -> Option<u64>,
) -> Vec<AutomationPoint> {
    points
        .iter()
        .filter_map(|p| {
            f(p.tick.0).map(|t| AutomationPoint {
                tick: Tick(t),
                ..p.clone()
            })
        })
        .collect()
}

/// テンポ・拍子・セクション・オートメーションの点を `f`(新しい tick。None なら消す)で動かすコマンド。
/// 変わるものだけ出す
fn shift_global(project: &Project, f: &dyn Fn(u64) -> Option<u64>) -> Vec<Command> {
    let mut out = Vec::new();
    // 先頭(tick 0)のテンポ・拍子は動かさない
    let keep0 = |t: u64| if t == 0 { Some(0) } else { f(t) };

    let tempo: Vec<_> = project
        .tempo_map
        .events()
        .iter()
        .filter_map(|e| {
            keep0(e.tick.0).map(|t| crate::time::TempoEvent {
                tick: Tick(t),
                bpm: e.bpm,
            })
        })
        .collect();
    if tempo.as_slice() != project.tempo_map.events() {
        out.push(Command::SetTempo { events: tempo });
    }
    let sigs: Vec<_> = project
        .time_sig_map
        .iter()
        .filter_map(|e| {
            keep0(e.tick.0).map(|t| TimeSigEvent {
                tick: Tick(t),
                ..*e
            })
        })
        .collect();
    if sigs != project.time_sig_map {
        out.push(Command::SetTimeSig { events: sigs });
    }
    let sections: Vec<SectionMarker> = project
        .sections
        .iter()
        .filter_map(|m| {
            f(m.tick.0).map(|t| SectionMarker {
                tick: Tick(t),
                ..m.clone()
            })
        })
        .collect();
    if sections != project.sections {
        out.push(Command::SetSections { sections });
    }
    for t in &project.tracks {
        for lane in &t.automation {
            let points = shift_points(&lane.points, f);
            if points != lane.points {
                out.push(Command::SetAutomationPoints {
                    track: t.id.clone(),
                    target: lane.target.clone(),
                    points,
                });
            }
        }
    }
    for lane in &project.master.automation {
        let points = shift_points(&lane.points, f);
        if points != lane.points {
            out.push(Command::SetMasterAutomationPoints {
                target: lane.target.clone(),
                points,
            });
        }
    }
    out
}

/// `at` に長さ `len` の空白を挿入する。`at` 以降のクリップ・テンポ・拍子・セクション・オートメーションを
/// 後ろへずらし、`at` をまたぐクリップは `at` で分割して後ろ半分をずらす。
pub fn insert_time(project: &Project, at: u64, len: u64) -> Vec<Command> {
    let mut splits = Vec::new();
    let mut moves = Vec::new();
    for t in &project.tracks {
        for c in &t.clips {
            let (s, e) = (c.start.0, c.start.0 + c.length.0);
            if s >= at {
                moves.push(Command::MoveClip {
                    id: c.id.clone(),
                    start: Tick(s + len),
                    track: None,
                });
            } else if e > at {
                let right = ClipId::new();
                splits.push(Command::SplitClip {
                    id: c.id.clone(),
                    at: Tick(at),
                    new_id: right.clone(),
                });
                moves.push(Command::MoveClip {
                    id: right,
                    start: Tick(at + len),
                    track: None,
                });
            }
        }
    }
    // 後ろのものから動かす(重なりを避ける)
    moves.reverse();
    let mut out = splits;
    out.extend(moves);
    out.extend(shift_global(project, &|t| {
        Some(if t >= at { t + len } else { t })
    }));
    out
}

/// `[from, from + len)` を削除して後ろを詰める。範囲内のクリップは消し、範囲にかかるクリップは
/// 範囲の外側だけ残す(分割・切り詰め)。範囲内のテンポ・拍子・セクション・オートメーションの点は消す。
pub fn delete_time(project: &Project, from: u64, len: u64) -> Vec<Command> {
    let to = from + len;
    let mut out = Vec::new();
    let mut moves = Vec::new();
    for t in &project.tracks {
        for c in &t.clips {
            let (s, e) = (c.start.0, c.start.0 + c.length.0);
            if e <= from {
                continue; // 範囲より前
            }
            if s >= to {
                moves.push(Command::MoveClip {
                    id: c.id.clone(),
                    start: Tick(s - len),
                    track: None,
                });
            } else if s >= from && e <= to {
                out.push(Command::RemoveClip { id: c.id.clone() });
            } else if s < from && e <= to {
                // 後ろが範囲にかかる → 切り詰め
                out.push(Command::ResizeClip {
                    id: c.id.clone(),
                    length: Tick(from - s),
                });
            } else if s >= from {
                // 前が範囲にかかる → 範囲の終わりで分けて前を消し、後ろを詰める
                let right = ClipId::new();
                out.push(Command::SplitClip {
                    id: c.id.clone(),
                    at: Tick(to),
                    new_id: right.clone(),
                });
                out.push(Command::RemoveClip { id: c.id.clone() });
                moves.push(Command::MoveClip {
                    id: right,
                    start: Tick(from),
                    track: None,
                });
            } else {
                // 範囲をまたぐ → 範囲の頭と終わりで分けて真ん中を消し、後ろを詰める
                let mid = ClipId::new();
                let right = ClipId::new();
                out.push(Command::SplitClip {
                    id: c.id.clone(),
                    at: Tick(from),
                    new_id: mid.clone(),
                });
                out.push(Command::SplitClip {
                    id: mid.clone(),
                    at: Tick(to),
                    new_id: right.clone(),
                });
                out.push(Command::RemoveClip { id: mid });
                moves.push(Command::MoveClip {
                    id: right,
                    start: Tick(from),
                    track: None,
                });
            }
        }
    }
    // 前のものから詰める(重なりを避ける)
    out.extend(moves);
    out.extend(shift_global(project, &|t| {
        if t < from {
            Some(t)
        } else if t < to {
            None
        } else {
            Some(t - len)
        }
    }));
    out
}

/// クリップを複製する(新しい ID。ノートの ID も振り直す)。`offset` だけ後ろ(負で前)に置く。
/// `track` を指定するとそのトラックへ(種類が違うとエラーは apply で返る)。
pub fn duplicate_clips(
    project: &Project,
    clip_ids: &[ClipId],
    offset: i64,
    track: Option<&crate::id::TrackId>,
) -> Result<Vec<(ClipId, Command)>, String> {
    let mut out = Vec::new();
    for id in clip_ids {
        let (t, c) = project
            .tracks
            .iter()
            .find_map(|t| t.clips.iter().find(|c| &c.id == id).map(|c| (t, c)))
            .ok_or_else(|| format!("クリップが見つかりません: {id}"))?;
        let start = c
            .start
            .checked_add_signed(offset)
            .ok_or_else(|| format!("{id} を曲の頭より前には置けません"))?;
        let mut copy = c.clone();
        copy.id = ClipId::new();
        copy.start = start;
        if let ClipContent::Midi { notes, .. } = &mut copy.content {
            for n in notes {
                n.id = NoteId::new();
            }
        }
        out.push((
            copy.id.clone(),
            Command::AddClip {
                track: track.cloned().unwrap_or_else(|| t.id.clone()),
                clip: copy,
            },
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Articulation, Clip, Note, ParamPath, Track, TrackKind};
    use crate::{Curve, TrackId};

    const BAR: u64 = 3840;

    /// 4 小節のクリップ(各小節の頭に 1 音)とセクション・オートメーションを持つ曲
    fn song() -> Project {
        let mut p = Project::new("s");
        let tid = TrackId::new();
        p.apply(&Command::AddTrack {
            track: Track::new(tid.clone(), "t", TrackKind::Midi),
            index: None,
        })
        .unwrap();
        let mut c = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(BAR * 4));
        if let ClipContent::Midi { notes, .. } = &mut c.content {
            for i in 0..4 {
                notes.push(Note {
                    id: NoteId::new(),
                    pos: Tick(i * BAR),
                    dur: Tick(480),
                    pitch: 60 + i as u8,
                    vel: 100,
                    articulation: Articulation::Normal,
                    pitch_curve: vec![],
                    glide_ms: None,
                });
            }
        }
        p.apply(&Command::AddClip {
            track: tid.clone(),
            clip: c,
        })
        .unwrap();
        let later = Clip::new_midi(ClipId::new(), "later", Tick(BAR * 6), Tick(BAR));
        p.apply(&Command::AddClip {
            track: tid.clone(),
            clip: later,
        })
        .unwrap();
        p.apply(&Command::SetSections {
            sections: vec![
                SectionMarker {
                    tick: Tick(0),
                    name: "A".into(),
                },
                SectionMarker {
                    tick: Tick(BAR * 2),
                    name: "B".into(),
                },
            ],
        })
        .unwrap();
        p.apply(&Command::SetAutomationPoints {
            track: tid,
            target: ParamPath::track("volume_db"),
            points: vec![
                AutomationPoint {
                    tick: Tick(0),
                    value: -6.0,
                    curve: Curve::Linear,
                },
                AutomationPoint {
                    tick: Tick(BAR * 3),
                    value: 0.0,
                    curve: Curve::Linear,
                },
            ],
        })
        .unwrap();
        p
    }

    /// 鳴るノートの (絶対 tick, 音高)
    fn played(p: &Project) -> Vec<(u64, u8)> {
        let mut v: Vec<_> = p
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .flat_map(|c| {
                c.playback_notes()
                    .into_iter()
                    .map(move |n| (c.start.0 + n.pos.0, n.pitch))
            })
            .collect();
        v.sort();
        v
    }

    fn apply_all(p: &mut Project, cmds: Vec<Command>) {
        p.apply(&Command::batch("x", cmds)).unwrap();
    }

    #[test]
    fn bar_range_follows_time_signature_changes() {
        let mut p = song();
        p.apply(&Command::SetTimeSig {
            events: vec![
                TimeSigEvent {
                    tick: Tick(0),
                    num: 4,
                    den: 4,
                },
                TimeSigEvent {
                    tick: Tick(BAR * 2),
                    num: 3,
                    den: 4,
                },
            ],
        })
        .unwrap();
        assert_eq!(bar_range(&p, 1, 2), Some((0, BAR * 2)));
        assert_eq!(bar_range(&p, 3, 2), Some((BAR * 2, 2880 * 2)));
        assert_eq!(bar_range(&p, 0, 1), None);
    }

    #[test]
    fn insert_bars_pushes_everything_after_and_splits_across() {
        let mut p = song();
        let before = p.clone();
        let (at, len) = bar_range(&p, 3, 2).unwrap(); // 3 小節目の頭に 2 小節
        let cmds = insert_time(&p, at, len);
        apply_all(&mut p, cmds);
        assert_eq!(
            played(&p),
            vec![(0, 60), (BAR, 61), (BAR * 4, 62), (BAR * 5, 63)]
        );
        // 後ろのクリップ・セクション・オートメーションもずれる
        assert!(p.tracks[0]
            .clips
            .iter()
            .any(|c| c.name == "later" && c.start.0 == BAR * 8));
        assert_eq!(p.sections[1].tick.0, BAR * 4);
        assert_eq!(p.tracks[0].automation[0].points[1].tick.0, BAR * 5);
        // 1 回の undo(逆コマンド)で戻る
        let mut q = before.clone();
        let applied = q
            .apply(&Command::batch("x", insert_time(&before, at, len)))
            .unwrap();
        q.apply(&applied.inverse).unwrap();
        assert_eq!(q, before);
    }

    #[test]
    fn delete_bars_removes_and_pulls_back() {
        let mut p = song();
        let (from, len) = bar_range(&p, 2, 2).unwrap(); // 2〜3 小節目を削除
        let cmds = delete_time(&p, from, len);
        apply_all(&mut p, cmds);
        assert_eq!(played(&p), vec![(0, 60), (BAR, 63)]);
        assert!(p.tracks[0]
            .clips
            .iter()
            .any(|c| c.name == "later" && c.start.0 == BAR * 4));
        // セクション B(3 小節目)は範囲内なので消える。オートメーションの 4 小節目の点は 2 小節目へ
        assert_eq!(p.sections.len(), 1);
        assert_eq!(p.tracks[0].automation[0].points[1].tick.0, BAR);
    }

    #[test]
    fn duplicate_gives_new_ids() {
        let p = song();
        let id = p.tracks[0].clips[0].id.clone();
        let out = duplicate_clips(&p, std::slice::from_ref(&id), (BAR * 8) as i64, None).unwrap();
        let mut q = p.clone();
        q.apply(&Command::batch(
            "dup",
            out.into_iter().map(|(_, c)| c).collect(),
        ))
        .unwrap();
        assert_eq!(q.tracks[0].clips.len(), 3);
        let copy = q.tracks[0]
            .clips
            .iter()
            .find(|c| c.start.0 == BAR * 8)
            .unwrap();
        assert_ne!(copy.id, id);
        assert!(duplicate_clips(&p, &[id], -1, None).is_err());
    }
}
