//! リズム・グルーヴの記号的分析(AI の「リズム感」)。
//!
//! ノートの発音位置から、スウィング・シンコペーション・グリッド(ストレート/3 連)・
//! ヒューマナイズ量を推定する。和声([`crate::harmony`])と対になる分析で、
//! 「既存のノリに合わせてフレーズを足す」判断材料になる。
//! ドラムを含む全トラックが対象(リズムはドラムこそ主役)。

use crate::model::{ClipContent, Project};
use crate::time::Tick;
use serde::Serialize;

const BEAT: u64 = 960; // PPQ = 960 の 4 分音符

#[derive(Clone, Debug, Serialize)]
pub struct RhythmAnalysis {
    /// 裏 8 分の平均位置 / 480。1.0 = ストレート、1.33 ≈ 3 連スウィング。
    /// 裏 8 分が少ない場合は None
    pub swing_ratio: Option<f64>,
    /// 発音のうち 4 分の表拍に乗っている割合(0..1)
    pub onbeat_ratio: f64,
    /// シンコペーション度: 8 分の裏 + 16 分の裏に乗る発音の割合(0..1)
    pub syncopation: f64,
    /// ストレート 16 分グリッドと 3 連グリッドのどちらが近いか
    pub grid: &'static str, // "straight" | "triplet"
    /// 最も近いグリッドからの平均ずれ(tick)。0 = 完全クオンタイズ、
    /// 15〜40 ≈ ヒューマナイズ、それ以上はルーズ
    pub avg_deviation_ticks: f64,
    /// 小節あたりの平均発音数
    pub density_per_bar: f64,
    pub note_count: usize,
}

fn nearest_dist(pos: u64, grid: u64) -> u64 {
    let r = pos % grid;
    r.min(grid - r)
}

/// プロジェクト(または範囲・トラック指定)のリズムを分析する。
pub fn analyze(
    project: &Project,
    track_ids: Option<&[crate::id::TrackId]>,
    range: Option<(Tick, Tick)>,
) -> RhythmAnalysis {
    // 発音位置(絶対 tick)を収集
    let mut onsets: Vec<u64> = Vec::new();
    let mut max_end = 0u64;
    for t in &project.tracks {
        if let Some(ids) = track_ids {
            if !ids.contains(&t.id) {
                continue;
            }
        }
        for c in &t.clips {
            if !matches!(c.content, ClipContent::Midi { .. }) {
                continue;
            }
            // ループクリップの繰り返しも含めた「実際に鳴るノート」
            for n in c.playback_notes() {
                let start = c.start.0 + n.pos.0;
                onsets.push(start);
                max_end = max_end.max(start + n.dur.0);
            }
        }
    }
    if let Some((s, e)) = range {
        onsets.retain(|o| *o >= s.0 && *o < e.0);
    }
    let note_count = onsets.len();
    if note_count == 0 {
        return RhythmAnalysis {
            swing_ratio: None,
            onbeat_ratio: 0.0,
            syncopation: 0.0,
            grid: "straight",
            avg_deviation_ticks: 0.0,
            density_per_bar: 0.0,
            note_count: 0,
        };
    }

    // ---- グリッド判定とずれ(16 分 = 240 vs 8 分 3 連 = 320) ----
    let dev_straight: f64 = onsets
        .iter()
        .map(|o| nearest_dist(*o, 240) as f64)
        .sum::<f64>()
        / note_count as f64;
    let dev_triplet: f64 = onsets
        .iter()
        .map(|o| nearest_dist(*o, 320) as f64)
        .sum::<f64>()
        / note_count as f64;
    let (grid, avg_deviation_ticks) = if dev_triplet + 2.0 < dev_straight {
        ("triplet", dev_triplet)
    } else {
        ("straight", dev_straight)
    };

    // ---- 表拍・裏の割合 ----
    let tol = 40u64; // 判定許容(±40 tick ≈ 1/96 音符)
    let mut onbeat = 0usize;
    let mut off8 = 0usize;
    let mut off16 = 0usize;
    let mut off8_positions: Vec<f64> = Vec::new();
    for o in &onsets {
        let in_beat = o % BEAT;
        if nearest_dist(*o, BEAT) <= tol {
            onbeat += 1;
        } else if (240..800).contains(&in_beat) {
            // 「裏 8 分ゾーン」(ストレート 480〜3 連 640 を含む広め)
            off8 += 1;
            off8_positions.push(in_beat as f64);
        } else if nearest_dist(*o, 240) <= tol {
            off16 += 1;
        }
    }
    let onbeat_ratio = onbeat as f64 / note_count as f64;
    let syncopation = (off8 + off16) as f64 / note_count as f64;
    let swing_ratio = if off8_positions.len() >= 4 {
        Some(off8_positions.iter().sum::<f64>() / off8_positions.len() as f64 / 480.0)
    } else {
        None
    };

    // ---- 密度(4/4 換算の小節あたり) ----
    let span_bars = ((max_end.max(1)) as f64 / 3840.0).max(1.0);
    let density_per_bar = note_count as f64 / span_bars;

    RhythmAnalysis {
        swing_ratio,
        onbeat_ratio,
        syncopation,
        grid,
        avg_deviation_ticks,
        density_per_bar,
        note_count,
    }
}

/// スウィングを掛けたノートの新しい位置(クリップ先頭からの tick)。
///
/// `grid` は裏拍の単位(480 = 8 分、240 = 16 分)。拍の組(2 × grid)のうち「裏」
/// (組の頭から grid/2 以上 3/2·grid 未満)にある音を、組の頭から `2·grid·swing` の位置へ
/// `strength`(0〜1)だけ寄せる。`swing` は 0.5 = ストレート、0.667 ≈ 3 連、0.75 = 付点。
/// 表の音は動かさない。寄せ先は絶対値なので、同じ設定で何度掛けても結果は同じ。
/// 位置は曲頭からの拍で判定する(`clip_start` を足してから測る)。
/// 戻り値は (ノート ID, 新しい位置)。位置が変わらない音は含めない。
pub fn swing_positions(
    notes: &[crate::Note],
    clip_start: u64,
    clip_len: u64,
    grid: u64,
    swing: f64,
    strength: f64,
) -> Vec<(crate::NoteId, u64)> {
    let grid = grid.max(2);
    let pair = grid * 2;
    let swing = swing.clamp(0.5, 0.8);
    let strength = strength.clamp(0.0, 1.0);
    let target = (pair as f64 * swing).round() as i64;
    notes
        .iter()
        .filter_map(|n| {
            let abs = clip_start + n.pos.0;
            let off = (abs % pair) as i64;
            if off < (grid / 2) as i64 || off >= (grid * 3 / 2) as i64 {
                return None; // 表の音
            }
            let moved = off + ((target - off) as f64 * strength).round() as i64;
            let new_abs = abs as i64 + (moved - off);
            let new_pos =
                (new_abs - clip_start as i64).clamp(0, clip_len.saturating_sub(1) as i64) as u64;
            (new_pos != n.pos.0).then(|| (n.id.clone(), new_pos))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::{ClipId, NoteId, TrackId};
    use crate::model::{Articulation, Clip, Note, Track, TrackKind};
    use crate::Command;

    fn project_with_onsets(onsets: &[u64]) -> Project {
        let mut p = Project::new("r");
        let tid = TrackId::new();
        p.apply(&Command::AddTrack {
            track: Track::new(tid.clone(), "Hat", TrackKind::Midi),
            index: None,
        })
        .unwrap();
        let end = onsets.iter().max().unwrap_or(&0) + 3840;
        let mut clip = Clip::new_midi(ClipId::new(), "beat", Tick(0), Tick(end));
        let notes = clip.notes_mut().unwrap();
        for &pos in onsets {
            notes.push(Note {
                id: NoteId::new(),
                pos: Tick(pos),
                dur: Tick(120),
                pitch: 42,
                vel: 100,
                articulation: Articulation::Normal,
                pitch_curve: vec![],
            });
        }
        p.apply(&Command::AddClip { track: tid, clip }).unwrap();
        p
    }

    #[test]
    fn straight_eighths_are_not_swung() {
        // 2 小節ぶんのストレート 8 分
        let onsets: Vec<u64> = (0..16).map(|i| i * 480).collect();
        let a = analyze(&project_with_onsets(&onsets), None, None);
        let swing = a.swing_ratio.unwrap();
        assert!((swing - 1.0).abs() < 0.05, "ストレートのはず: {swing}");
        assert_eq!(a.grid, "straight");
        assert!(a.avg_deviation_ticks < 1.0);
        assert!((a.onbeat_ratio - 0.5).abs() < 0.1);
    }

    #[test]
    fn swung_eighths_are_detected() {
        // 裏 8 分を 640(3 連位置)に置いたシャッフル
        let mut onsets = Vec::new();
        for beat in 0..8u64 {
            onsets.push(beat * 960);
            onsets.push(beat * 960 + 640);
        }
        let a = analyze(&project_with_onsets(&onsets), None, None);
        let swing = a.swing_ratio.unwrap();
        assert!(
            (swing - 640.0 / 480.0).abs() < 0.05,
            "3 連スウィングのはず: {swing}"
        );
        assert_eq!(a.grid, "triplet");
    }

    #[test]
    fn syncopated_pattern_scores_high() {
        // 裏ばかりのパターン
        let onsets: Vec<u64> = (0..8).map(|i| i * 960 + 480).collect();
        let a = analyze(&project_with_onsets(&onsets), None, None);
        assert!(a.syncopation > 0.9, "シンコペーション度: {}", a.syncopation);
        assert!(a.onbeat_ratio < 0.1);

        // 表ばかりのパターン
        let onsets: Vec<u64> = (0..8).map(|i| i * 960).collect();
        let a = analyze(&project_with_onsets(&onsets), None, None);
        assert!(a.syncopation < 0.1);
        assert!(a.onbeat_ratio > 0.9);
    }

    #[test]
    fn humanized_deviation_is_measured() {
        // 16 分グリッドから ±20 tick ずらした「人間らしい」打ち込み
        let onsets: Vec<u64> = (0..16)
            .map(|i| {
                let base: u64 = i * 240;
                if i % 2 == 0 {
                    base + 20
                } else {
                    base.saturating_sub(18)
                }
            })
            .collect();
        let a = analyze(&project_with_onsets(&onsets), None, None);
        assert!(
            (a.avg_deviation_ticks - 19.0).abs() < 3.0,
            "ずれ量: {}",
            a.avg_deviation_ticks
        );
    }

    #[test]
    fn swing_moves_offbeats_and_is_idempotent() {
        use crate::{Note, NoteId, Tick};
        let n = |pos: u64| Note {
            id: NoteId::new(),
            pos: Tick(pos),
            dur: Tick(120),
            pitch: 60,
            vel: 100,
            articulation: Default::default(),
            pitch_curve: vec![],
        };
        // ストレートの 8 分(0, 480, 960, 1440)
        let notes = vec![n(0), n(480), n(960), n(1440)];
        let moved = super::swing_positions(&notes, 0, 3840, 480, 2.0 / 3.0, 1.0);
        let pos: Vec<u64> = moved.iter().map(|(_, p)| *p).collect();
        assert_eq!(pos, vec![640, 1600], "裏だけ 3 連の位置へ");
        // 同じ設定で掛け直しても変わらない
        let again: Vec<Note> = notes
            .iter()
            .map(|x| {
                let mut y = x.clone();
                if let Some((_, p)) = moved.iter().find(|(id, _)| *id == x.id) {
                    y.pos = Tick(*p);
                }
                y
            })
            .collect();
        assert!(super::swing_positions(&again, 0, 3840, 480, 2.0 / 3.0, 1.0).is_empty());
        // ストレートに戻す・半分だけ寄せる・クリップの位置を考慮
        let back = super::swing_positions(&again, 0, 3840, 480, 0.5, 1.0);
        assert_eq!(
            back.iter().map(|(_, p)| *p).collect::<Vec<_>>(),
            vec![480, 1440]
        );
        let half = super::swing_positions(&notes, 0, 3840, 480, 0.75, 0.5);
        assert_eq!(half[0].1, 480 + 120);
        // クリップが 1 拍ずれた所(960)から始まっても、曲の拍で判定する
        let shifted = super::swing_positions(&[n(480)], 960, 3840, 480, 2.0 / 3.0, 1.0);
        assert_eq!(shifted[0].1, 640);
        // 16 分
        let s16 = super::swing_positions(&[n(0), n(240)], 0, 3840, 240, 2.0 / 3.0, 1.0);
        assert_eq!(s16[0].1, 320);
    }
}
