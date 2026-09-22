//! 和声の記号的分析(AI の「音楽理論の目」)。
//!
//! オーディオではなく **ノートデータから** キーと小節ごとのコードを推定する。
//! - キー: Krumhansl-Schmuckler 法(音価で重み付けしたピッチクラス分布を
//!   長調・短調のプロファイルと相関して 24 キーから選ぶ)
//! - コード: 小節ごとに鳴っている音の分布をコードテンプレートと照合
//!
//! 推定なので confidence を必ず添える。AI はこれを「参考値」として使い、
//! 意図的な転調・借用和音を壊さないよう判断する。

use crate::model::{ClipContent, Project};
use crate::time::Tick;
use serde::Serialize;

const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// Krumhansl-Kessler のキープロファイル。
const MAJOR_PROFILE: [f64; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const MINOR_PROFILE: [f64; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

/// コード品質のテンプレート(ルートからの半音, 重み)。
const CHORD_TEMPLATES: &[(&str, &[(usize, f64)])] = &[
    ("", &[(0, 1.0), (4, 0.85), (7, 0.6)]),            // メジャー
    ("m", &[(0, 1.0), (3, 0.85), (7, 0.6)]),           // マイナー
    ("7", &[(0, 1.0), (4, 0.8), (7, 0.5), (10, 0.7)]), // ドミナント 7th
    ("maj7", &[(0, 1.0), (4, 0.8), (7, 0.5), (11, 0.7)]), // メジャー 7th
    ("m7", &[(0, 1.0), (3, 0.8), (7, 0.5), (10, 0.7)]), // マイナー 7th
    ("dim", &[(0, 1.0), (3, 0.85), (6, 0.7)]),         // ディミニッシュ
    ("sus4", &[(0, 1.0), (5, 0.85), (7, 0.6)]),        // sus4
];

#[derive(Clone, Debug, Serialize)]
pub struct KeyEstimate {
    /// 例: "A minor" / "C major"
    pub name: String,
    /// トニックのピッチクラス 0..12(C=0)
    pub tonic: u8,
    pub mode: &'static str,
    /// 0..1(1 位と 2 位の相関の差から算出)
    pub confidence: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct BarChord {
    /// 1 始まりの小節番号
    pub bar: usize,
    /// 小節頭の tick
    pub tick: u64,
    /// 例: "Am" / "G7" / "N.C."(ノートなし)
    pub chord: String,
    /// 0..1
    pub confidence: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct HarmonyAnalysis {
    pub key: Option<KeyEstimate>,
    pub chords: Vec<BarChord>,
    /// キーのスケールに含まれない音の割合(0..1。転調・borrowed の目安)
    pub out_of_key_ratio: f64,
    /// 分析に使ったノート数
    pub note_count: usize,
}

/// (小節頭 tick, 小節長) の列を拍子マップから作る。
fn bars(project: &Project, end_tick: u64) -> Vec<(u64, u64)> {
    let ppq = crate::time::PPQ;
    let mut sigs: Vec<_> = project.time_sig_map.clone();
    sigs.sort_by_key(|s| s.tick);
    if sigs.is_empty() {
        sigs.push(crate::time::TimeSigEvent {
            tick: Tick(0),
            num: 4,
            den: 4,
        });
    }
    let mut out = Vec::new();
    for (i, sig) in sigs.iter().enumerate() {
        let seg_end = sigs.get(i + 1).map(|s| s.tick.0).unwrap_or(u64::MAX);
        let bar_len = (ppq * 4 * sig.num as u64 / sig.den as u64).max(1);
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

/// ピアソン相関。
fn correlate(hist: &[f64; 12], profile: &[f64; 12]) -> f64 {
    let mh = hist.iter().sum::<f64>() / 12.0;
    let mp = profile.iter().sum::<f64>() / 12.0;
    let mut num = 0.0;
    let mut dh = 0.0;
    let mut dp = 0.0;
    for i in 0..12 {
        let a = hist[i] - mh;
        let b = profile[i] - mp;
        num += a * b;
        dh += a * a;
        dp += b * b;
    }
    if dh <= 0.0 || dp <= 0.0 {
        return 0.0;
    }
    num / (dh * dp).sqrt()
}

fn scale_pcs(tonic: u8, mode: &str) -> [bool; 12] {
    let steps: &[u8] = if mode == "major" {
        &[0, 2, 4, 5, 7, 9, 11]
    } else {
        &[0, 2, 3, 5, 7, 8, 10] // ナチュラルマイナー
    };
    let mut out = [false; 12];
    for s in steps {
        out[((tonic + s) % 12) as usize] = true;
    }
    out
}

/// プロジェクト(または範囲)の和声を分析する。
/// `track_ids` を指定するとそのトラックだけ(例: ドラムを除外したいとき)。
pub fn analyze(
    project: &Project,
    track_ids: Option<&[crate::id::TrackId]>,
    range: Option<(Tick, Tick)>,
) -> HarmonyAnalysis {
    // (絶対 tick 開始, 終了, ピッチ, 音価重み) を収集。ドラムトラックは除外
    let mut events: Vec<(u64, u64, u8)> = Vec::new();
    for t in &project.tracks {
        if let Some(ids) = track_ids {
            if !ids.contains(&t.id) {
                continue;
            }
        }
        // ドラム(音程情報がない)は和声分析から除く
        if let Some(d) = &t.device {
            if matches!(&d.source, crate::model::PluginSource::Builtin { name } if name == "drum") {
                continue;
            }
            if let crate::model::PluginSource::Sf2 { bank, .. } = &d.source {
                if *bank == 128 {
                    continue;
                }
            }
        }
        for c in &t.clips {
            let ClipContent::Midi { notes, .. } = &c.content else {
                continue;
            };
            for n in notes {
                let start = c.start.0 + n.pos.0;
                let end = start + n.dur.0.min(c.length.0.saturating_sub(n.pos.0));
                events.push((start, end, n.pitch));
            }
        }
    }
    if let Some((s, e)) = range {
        events.retain(|(start, end, _)| *end > s.0 && *start < e.0);
        for ev in &mut events {
            ev.0 = ev.0.max(s.0);
            ev.1 = ev.1.min(e.0);
        }
    }

    let note_count = events.len();
    if note_count == 0 {
        return HarmonyAnalysis {
            key: None,
            chords: Vec::new(),
            out_of_key_ratio: 0.0,
            note_count: 0,
        };
    }

    // ---- キー推定(音価重み付きピッチクラス分布) ----
    let mut hist = [0.0f64; 12];
    for (start, end, pitch) in &events {
        hist[(*pitch % 12) as usize] += end.saturating_sub(*start) as f64;
    }
    let mut best = (0u8, "major", f64::MIN);
    let mut second = f64::MIN;
    for tonic in 0..12u8 {
        for (mode, profile) in [("major", &MAJOR_PROFILE), ("minor", &MINOR_PROFILE)] {
            // プロファイルをトニックに合わせて回転
            let mut rotated = [0.0f64; 12];
            for (i, r) in rotated.iter_mut().enumerate() {
                *r = profile[(i + 12 - tonic as usize) % 12];
            }
            let corr = correlate(&hist, &rotated);
            if corr > best.2 {
                second = best.2;
                best = (tonic, mode, corr);
            } else if corr > second {
                second = corr;
            }
        }
    }
    let (tonic, mode, corr) = best;
    let confidence = ((corr - second.max(0.0)) * 3.0).clamp(0.05, 1.0);
    let key = KeyEstimate {
        name: format!(
            "{} {}",
            NOTE_NAMES[tonic as usize],
            if mode == "major" { "major" } else { "minor" }
        ),
        tonic,
        mode,
        confidence,
    };

    // スケール外音の割合(音価重み)
    let in_scale = scale_pcs(tonic, mode);
    let total: f64 = hist.iter().sum();
    let out_of_key: f64 = hist
        .iter()
        .enumerate()
        .filter(|(pc, _)| !in_scale[*pc])
        .map(|(_, w)| *w)
        .sum();
    let out_of_key_ratio = if total > 0.0 { out_of_key / total } else { 0.0 };

    // ---- 小節ごとのコード推定 ----
    let range_start = range.map(|(s, _)| s.0).unwrap_or(0);
    let range_end = range
        .map(|(_, e)| e.0)
        .unwrap_or_else(|| events.iter().map(|(_, e, _)| *e).max().unwrap_or(0));
    let mut chords = Vec::new();
    for (bar_idx, (bar_tick, bar_len)) in bars(project, range_end).iter().enumerate() {
        let (bs, be) = (*bar_tick, bar_tick + bar_len);
        if be <= range_start || bs >= range_end {
            continue;
        }
        // 小節内の音価重み付きピッチクラス分布
        let mut w = [0.0f64; 12];
        let mut bass: Option<(u8, u64)> = None; // 最低音(ルート判定の補助)
        for (start, end, pitch) in &events {
            let overlap = (*end).min(be).saturating_sub((*start).max(bs));
            if overlap == 0 {
                continue;
            }
            w[(*pitch % 12) as usize] += overlap as f64;
            match bass {
                Some((p, _)) if *pitch >= p => {}
                _ => bass = Some((*pitch, overlap)),
            }
        }
        let total: f64 = w.iter().sum();
        if total <= 0.0 {
            chords.push(BarChord {
                bar: bar_idx + 1,
                tick: bs,
                chord: "N.C.".to_owned(),
                confidence: 1.0,
            });
            continue;
        }
        // 全ルート × 全品質でスコアリング
        let mut best_score = f64::MIN;
        let mut second_score = f64::MIN;
        let mut best_name = String::new();
        for root in 0..12usize {
            for (suffix, tmpl) in CHORD_TEMPLATES {
                let mut score = 0.0;
                for (offset, weight) in tmpl.iter() {
                    score += w[(root + offset) % 12] * weight;
                }
                // テンプレート外の音はペナルティ
                let tmpl_pcs: Vec<usize> = tmpl.iter().map(|(o, _)| (root + o) % 12).collect();
                let outside: f64 = w
                    .iter()
                    .enumerate()
                    .filter(|(pc, _)| !tmpl_pcs.contains(pc))
                    .map(|(_, x)| *x)
                    .sum();
                score -= outside * 0.35;
                // 最低音がルートなら加点(転回形の誤判定を減らす)
                if let Some((bp, _)) = bass {
                    if (bp % 12) as usize == root {
                        score += total * 0.08;
                    }
                }
                if score > best_score {
                    second_score = best_score;
                    best_score = score;
                    best_name = format!("{}{}", NOTE_NAMES[root], suffix);
                } else if score > second_score {
                    second_score = score;
                }
            }
        }
        let conf = if best_score <= 0.0 {
            0.1
        } else {
            (((best_score - second_score.max(0.0)) / best_score) + 0.3).clamp(0.1, 1.0)
        };
        chords.push(BarChord {
            bar: bar_idx + 1,
            tick: bs,
            chord: best_name,
            confidence: conf,
        });
    }

    HarmonyAnalysis {
        key: Some(key),
        chords,
        out_of_key_ratio,
        note_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::{ClipId, NoteId, TrackId};
    use crate::model::{Articulation, Clip, Note, Track, TrackKind};
    use crate::Command;

    /// 指定したピッチ列を各小節に和音として置いたプロジェクト
    fn project_with_bars(bars: &[&[u8]]) -> Project {
        let mut p = Project::new("h");
        let tid = TrackId::new();
        p.apply(&Command::AddTrack {
            track: Track::new(tid.clone(), "Keys", TrackKind::Midi),
            index: None,
        })
        .unwrap();
        let mut clip = Clip::new_midi(
            ClipId::new(),
            "prog",
            Tick(0),
            Tick(3840 * bars.len() as u64),
        );
        let notes = clip.notes_mut().unwrap();
        for (i, chord) in bars.iter().enumerate() {
            for &pitch in chord.iter() {
                notes.push(Note {
                    id: NoteId::new(),
                    pos: Tick(i as u64 * 3840),
                    dur: Tick(3840),
                    pitch,
                    vel: 100,
                    articulation: Articulation::Normal,
                });
            }
        }
        p.apply(&Command::AddClip { track: tid, clip }).unwrap();
        p
    }

    #[test]
    fn detects_c_major_progression() {
        // C → F → G7 → C
        let p = project_with_bars(&[
            &[60, 64, 67],     // C
            &[53, 57, 60, 65], // F(F3 が最低音)
            &[55, 59, 62, 65], // G7
            &[48, 60, 64, 67], // C
        ]);
        let a = analyze(&p, None, None);
        let key = a.key.unwrap();
        assert_eq!(key.name, "C major", "confidence={}", key.confidence);
        assert!(a.out_of_key_ratio < 0.05);
        let names: Vec<&str> = a.chords.iter().map(|c| c.chord.as_str()).collect();
        assert_eq!(names[0], "C");
        assert_eq!(names[1], "F");
        assert_eq!(names[2], "G7");
        assert_eq!(names[3], "C");
    }

    #[test]
    fn detects_a_minor() {
        // Am → Dm → E → Am
        let p = project_with_bars(&[
            &[57, 60, 64],
            &[50, 53, 57],
            &[52, 56, 59],
            &[45, 57, 60, 64],
        ]);
        let a = analyze(&p, None, None);
        let key = a.key.unwrap();
        assert_eq!(key.name, "A minor", "confidence={}", key.confidence);
        assert_eq!(a.chords[0].chord, "Am");
        assert_eq!(a.chords[1].chord, "Dm");
    }

    #[test]
    fn empty_bar_is_no_chord_and_empty_project_has_no_key() {
        // 2 小節目が空(コード → 休み → コード)
        let p = project_with_bars(&[&[60, 64, 67], &[], &[60, 64, 67]]);
        let a = analyze(&p, None, None);
        assert_eq!(a.chords[1].chord, "N.C.");

        let empty = Project::new("e");
        let a = analyze(&empty, None, None);
        assert!(a.key.is_none());
        assert_eq!(a.note_count, 0);
    }
}
