//! 構成の編集(小節の挿入・削除、クリップの複製)を [`Command`] の列に組み立てる。
//!
//! AI がノートやクリップを 1 つずつ組み立てると、呼び出し回数・ID の書き損じ・トークンが増える。
//! ここで「今のプロジェクト」から絶対値のコマンド列を作り、呼び出し側(MCP)が Batch 1 件として
//! 適用する(1 回の undo で戻る)。新規 ID はここ(コマンドを作る側)で振る。

use crate::command::Command;
use crate::id::{ClipId, NoteId};
use crate::model::{AutomationPoint, Clip, ClipContent, Project, SectionMarker};
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
            grouping: None,
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
                ..e.clone()
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

/// `from`〜`to`(絶対 tick)を無音にするコマンド(ドロップ・サビの直前の無音)。その範囲で始まる音は消し、
/// かかる音は手前で切る。ループのクリップは繰り返しを書き出してから切る。`keep` のトラックは触らない。
/// 返り値: (コマンド, 消した・切った音の数, 切れなかった音声のクリップの名前)
pub fn silence_range(
    project: &Project,
    from: u64,
    to: u64,
    keep: &[crate::id::TrackId],
) -> (Vec<Command>, usize, Vec<String>) {
    let mut cmds = Vec::new();
    let mut touched = 0usize;
    let mut audio = Vec::new();
    for t in project.tracks.iter().filter(|t| !keep.contains(&t.id)) {
        for c in &t.clips {
            if c.start.0 >= to || c.end().0 <= from {
                continue;
            }
            if !c.is_midi() {
                audio.push(c.name.clone());
                continue;
            }
            let base = unroll_loop(c).unwrap_or_else(|| c.clone());
            let mut out = base.clone();
            let mut changed = base.loop_len() != c.loop_len();
            if let Some(ns) = out.notes_mut() {
                let before = ns.len();
                ns.retain(|n| {
                    let at = c.start.0 + n.pos.0;
                    !(from..to).contains(&at)
                });
                let mut cut = before - ns.len();
                for n in ns.iter_mut() {
                    let at = c.start.0 + n.pos.0;
                    if at < from && at + n.dur.0 > from {
                        n.dur = Tick(from - at);
                        cut += 1;
                    }
                }
                touched += cut;
                changed |= cut > 0;
            }
            if changed {
                cmds.push(Command::ReplaceClip {
                    id: c.id.clone(),
                    clip: out,
                });
            }
        }
    }
    (cmds, touched, audio)
}

/// 曲の計画書の 1 区間(MCP の set_song_plan の引数)
#[derive(Clone, Debug, Default)]
pub struct PlanSection {
    pub name: String,
    /// 小節数
    pub bars: u32,
    /// 盛り上がり 0〜10
    pub energy: Option<f32>,
    /// この区間で鳴らすトラックの名前
    pub tracks: Vec<String>,
    /// 役割・意図
    pub note: Option<String>,
}

/// 計画書の区間を、小節の頭に置く区間のマーカーにする(拍子の変化に沿って小節を数える)。
/// 返り値は (マーカー, 始まりの小節, 終わりの tick) の並び。`start_bar` は最初の区間の小節(1 始まり)
pub fn plan_markers(
    project: &Project,
    start_bar: u32,
    plan: &[PlanSection],
) -> Result<Vec<(SectionMarker, u32, u64)>, String> {
    if plan.is_empty() || plan.len() > 64 {
        return Err("区間は 1〜64 個".to_owned());
    }
    let mut bar = start_bar.max(1);
    let mut out = Vec::with_capacity(plan.len());
    for s in plan {
        let name = s.name.trim();
        if name.is_empty() {
            return Err("区間の名前が空です".to_owned());
        }
        if s.bars == 0 || s.bars > 512 {
            return Err(format!("「{name}」の小節数は 1〜512"));
        }
        if let Some(e) = s.energy {
            if !(0.0..=10.0).contains(&e) {
                return Err(format!("「{name}」の energy は 0〜10"));
            }
        }
        let (at, len) = bar_range(project, bar, s.bars)
            .ok_or_else(|| format!("「{name}」の小節を数えられません"))?;
        out.push((
            SectionMarker {
                tick: Tick(at),
                name: name.to_owned(),
                energy: s.energy,
                tracks: s
                    .tracks
                    .iter()
                    .map(|t| t.trim().to_owned())
                    .filter(|t| !t.is_empty())
                    .collect(),
                note: s
                    .note
                    .as_ref()
                    .map(|n| n.trim().to_owned())
                    .filter(|n| !n.is_empty()),
            },
            bar,
            at + len,
        ));
        bar += s.bars;
    }
    Ok(out)
}

/// ループのクリップを、繰り返しを書き出した普通のクリップにする(`ReplaceClip` に渡す)。ループでなければ None。
/// 1 回目のノートは元の ID のまま、2 回目以降は新しい ID。ループの境目・クリップの終わりをまたぐ音は切り詰める
/// (鳴り方は変わらない。ループの長さより後ろにある鳴らない音は消える)。
/// 繰り返しごとに違う揺れを付けるときに使う(apply_groove の unroll_loop)
pub fn unroll_loop(clip: &Clip) -> Option<Clip> {
    clip.loop_len()?;
    let mut seen = std::collections::HashSet::new();
    let mut notes = clip.playback_notes();
    for n in &mut notes {
        if !seen.insert(n.id.clone()) {
            n.id = NoteId::new();
        }
    }
    notes.sort_by(|a, b| (a.pos, a.pitch, &a.id).cmp(&(b.pos, b.pitch, &b.id)));
    let mut out = clip.clone();
    out.content = ClipContent::Midi {
        notes,
        looped: false,
        loop_len: None,
    };
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Articulation, Note, ParamPath, Track, TrackKind};
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
                    ..Default::default()
                },
                SectionMarker {
                    tick: Tick(BAR * 2),
                    name: "B".into(),
                    ..Default::default()
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
                    grouping: None,
                },
                TimeSigEvent {
                    tick: Tick(BAR * 2),
                    num: 3,
                    den: 4,
                    grouping: None,
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

    #[test]
    fn silence_range_cuts_and_removes_notes_and_unrolls_loops() {
        let mut p = song();
        // 1 小節のループ(4 分)を 4 小節ぶんのクリップも足す
        let tid = p.tracks[0].id.clone();
        let mut c = Clip::new_midi(ClipId::new(), "loop", Tick(0), Tick(BAR * 4));
        if let ClipContent::Midi {
            notes,
            looped,
            loop_len,
        } = &mut c.content
        {
            for k in 0..4 {
                notes.push(Note {
                    id: NoteId::new(),
                    pos: Tick(k * 960),
                    dur: Tick(900),
                    pitch: 42,
                    vel: 90,
                    articulation: Articulation::Normal,
                    pitch_curve: vec![],
                    glide_ms: None,
                });
            }
            *looped = true;
            *loop_len = Some(Tick(BAR));
        }
        p.apply(&Command::AddClip {
            track: tid.clone(),
            clip: c,
        })
        .unwrap();
        // 4 小節目の最後の 1 拍(+ その前から伸びる音の切り詰め)
        let from = BAR * 4 - 960;
        let (cmds, touched, audio) = silence_range(&p, from, BAR * 4, &[]);
        assert!(audio.is_empty());
        assert!(touched >= 1);
        apply_all(&mut p, cmds);
        for c in &p.tracks[0].clips {
            for n in c.playback_notes() {
                let at = c.start.0 + n.pos.0;
                assert!(!(from..BAR * 4).contains(&at), "{at}");
                if at < from {
                    assert!(at + n.dur.0 <= from, "{at} {}", n.dur.0);
                }
            }
        }
        // ほかの範囲の音は残る(ループは書き出されて 3 小節分 + 4 小節目の 3 拍)
        let lp = p.tracks[0].clips.iter().find(|c| c.name == "loop").unwrap();
        assert!(lp.loop_len().is_none());
        assert_eq!(lp.notes().unwrap().len(), 15);
        // keep のトラックは触らない
        let (cmds, _, _) = silence_range(&p, 0, BAR * 4, &[tid]);
        assert!(cmds.is_empty());
    }

    #[test]
    fn plan_markers_count_bars_across_time_signature_changes() {
        let mut p = Project::new("p");
        // 9 小節目から 3/4
        p.time_sig_map.push(TimeSigEvent {
            tick: Tick(BAR * 8),
            num: 3,
            den: 4,
            grouping: None,
        });
        let plan = vec![
            PlanSection {
                name: "intro".into(),
                bars: 8,
                energy: Some(3.0),
                tracks: vec!["Kick".into(), " ".into()],
                note: Some("  ".into()),
            },
            PlanSection {
                name: "waltz".into(),
                bars: 4,
                ..Default::default()
            },
        ];
        let m = plan_markers(&p, 1, &plan).unwrap();
        assert_eq!(m[0].0.tick.0, 0);
        assert_eq!(m[0].0.tracks, vec!["Kick".to_owned()]);
        assert_eq!(m[0].0.note, None);
        assert_eq!((m[1].0.tick.0, m[1].1), (BAR * 8, 9));
        // 3/4 の小節 4 つ
        assert_eq!(m[1].2, BAR * 8 + 2880 * 4);
        // 範囲の外はエラー
        let bad = vec![PlanSection {
            name: "x".into(),
            bars: 4,
            energy: Some(11.0),
            ..Default::default()
        }];
        assert!(plan_markers(&p, 1, &bad).is_err());
    }

    #[test]
    fn unroll_loop_writes_out_repeats_and_sounds_the_same() {
        let mut p = song();
        let id = p.tracks[0].clips[0].id.clone();
        // 1 小節のループを 2.5 小節ぶん
        p.apply(&Command::SetClipLoop {
            id: id.clone(),
            loop_len: Some(Tick(BAR)),
        })
        .unwrap();
        p.apply(&Command::ResizeClip {
            id: id.clone(),
            length: Tick(BAR * 5 / 2),
        })
        .unwrap();
        let before = played(&p);
        let clip = p.tracks[0].clips[0].clone();
        let flat = unroll_loop(&clip).expect("ループ");
        assert!(flat.loop_len().is_none());
        let notes = flat.notes().unwrap();
        assert_eq!(notes.len(), 3);
        // 1 回目は元の ID、2 回目以降は新しい ID
        assert_eq!(notes[0].id, clip.notes().unwrap()[0].id);
        assert_ne!(notes[1].id, notes[0].id);
        p.apply(&Command::ReplaceClip { id, clip: flat }).unwrap();
        assert_eq!(played(&p), before);
        // ループでなければ None
        assert!(unroll_loop(&p.tracks[0].clips[0]).is_none());
    }
}
