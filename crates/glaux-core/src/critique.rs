//! 編曲の点検(MCP の `critique_arrangement` の中身)。楽譜だけで分かる「機械的すぎる・平板すぎる・破綻している」を
//! 数値で見つけ、直し方(使う道具)と一緒に返す。AI は完了を報告する前にこれで確かめ、指摘を直す。
//! ベンチマーク(同じ依頼を作らせて前後を比べる)の自動の指標にも使う。
//!
//! しきい値は経験則(研究・製品の調査から置いた初期値)。音そのものの点検(音量・帯域・かぶり)は analyze_audio が受け持つ。

use crate::model::{Clip, ClipContent, Project, Track};
use serde::Serialize;

const SIXTEENTH: u64 = crate::time::PPQ / 4;

/// トラックごとの数値
#[derive(Clone, Debug, Serialize)]
pub struct TrackMetrics {
    pub track_id: String,
    pub name: String,
    pub drums: bool,
    pub notes: usize,
    /// 16 分の格子ちょうどにある割合
    pub on_grid: f64,
    /// ベロシティの標準偏差
    pub velocity_sd: f64,
    pub lowest: u8,
    pub highest: u8,
    /// 奏法・ピッチカーブ・ノートごとの滑る時間を使っている音の割合
    pub expressive: f64,
    pub automation_lanes: usize,
    /// 鳴っている小節のうち、中身(音の並び)が違う小節の数
    pub distinct_bars: usize,
    pub sounding_bars: usize,
}

/// 区間(セクションマーカー)ごとの数値
#[derive(Clone, Debug, Serialize)]
pub struct SectionMetrics {
    pub name: String,
    pub start_bar: usize,
    pub bars: usize,
    /// 鳴っているトラックの数
    pub active_tracks: usize,
    /// 1 小節あたりの音の数
    pub notes_per_bar: f64,
    /// 盛り上がりの目安 0〜10(鳴っているトラックの割合・音の密度・音域の広さ)
    pub energy: f64,
}

/// 指摘 1 つ
#[derive(Clone, Debug, Serialize)]
pub struct Finding {
    /// "warn"(直した方がよい)/ "info"(検討)
    pub severity: &'static str,
    /// 対象(トラック名・区間名。曲全体なら空)
    pub target: String,
    pub what: String,
    /// 直し方(使う道具)
    pub fix: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Critique {
    pub findings: Vec<Finding>,
    pub tracks: Vec<TrackMetrics>,
    pub sections: Vec<SectionMetrics>,
    pub song_bars: usize,
    pub automation_lanes: usize,
}

/// トラックの小節ごとの数値(ドラムか, 音の数, (最低音, 最高音))
type BarStats = (bool, Vec<usize>, Vec<(u8, u8)>);

fn is_drum(track: &Track) -> bool {
    use crate::model::PluginSource;
    match track.device.as_ref().map(|d| &d.source) {
        Some(PluginSource::Builtin { name }) => name == "drum",
        Some(PluginSource::Sf2 { bank, .. }) => *bank == 128,
        _ => false,
    }
}

/// クリップの鳴る音を (絶対 tick, 長さ, 音程, 強さ, 表情の有無) で(ループは繰り返しを展開)
fn sounding(clip: &Clip) -> Vec<(u64, u64, u8, u8, bool)> {
    let ClipContent::Midi {
        notes,
        looped,
        loop_len,
    } = &clip.content
    else {
        return vec![];
    };
    let period = if *looped {
        loop_len.map(|l| l.0).filter(|l| *l > 0)
    } else {
        None
    };
    let mut out = Vec::new();
    let reps = period.map_or(1, |p| clip.length.0.div_ceil(p));
    for r in 0..reps {
        let off = period.map_or(0, |p| p * r);
        for n in notes {
            if period.is_some_and(|p| n.pos.0 >= p) {
                continue;
            }
            let pos = n.pos.0 + off;
            if pos >= clip.length.0 {
                continue;
            }
            let expressive = n.articulation != Default::default()
                || !n.pitch_curve.is_empty()
                || n.glide_ms.is_some();
            out.push((clip.start.0 + pos, n.dur.0, n.pitch, n.vel, expressive));
        }
    }
    out
}

pub fn critique(project: &Project) -> Critique {
    let end = project.end().0;
    let grid = crate::arrange::bar_grid(project, end.max(1));
    let bar_of = |t: u64| grid.partition_point(|(s, _)| *s <= t).saturating_sub(1);
    let song_bars = grid.iter().filter(|(s, _)| *s < end).count();
    let mut findings = Vec::new();
    let mut tracks = Vec::new();
    // トラックごとの、小節ごとの音の数(区間の数値に使う)
    let mut per_bar: Vec<BarStats> = Vec::new();
    for t in &project.tracks {
        let drums = is_drum(t);
        let mut notes: Vec<(u64, u64, u8, u8, bool)> = t.clips.iter().flat_map(sounding).collect();
        notes.sort_by_key(|n| (n.0, n.2));
        let mut counts = vec![0usize; song_bars.max(1)];
        let mut ranges = vec![(127u8, 0u8); song_bars.max(1)];
        // 小節ごとの中身(小節頭からの相対位置と音程)
        let mut patterns: Vec<Vec<(u64, u8)>> = vec![Vec::new(); song_bars.max(1)];
        for n in &notes {
            let b = bar_of(n.0).min(counts.len() - 1);
            counts[b] += 1;
            ranges[b] = (ranges[b].0.min(n.2), ranges[b].1.max(n.2));
            patterns[b].push((n.0 - grid.get(b).map_or(0, |g| g.0), n.2));
        }
        per_bar.push((drums, counts.clone(), ranges));
        if notes.is_empty() {
            continue;
        }
        let n = notes.len() as f64;
        let on_grid = notes.iter().filter(|x| x.0 % SIXTEENTH == 0).count() as f64 / n;
        let mean_v = notes.iter().map(|x| x.3 as f64).sum::<f64>() / n;
        let sd_v = (notes
            .iter()
            .map(|x| (x.3 as f64 - mean_v).powi(2))
            .sum::<f64>()
            / n)
            .sqrt();
        let expressive = notes.iter().filter(|x| x.4).count() as f64 / n;
        let mut distinct: Vec<&Vec<(u64, u8)>> =
            patterns.iter().filter(|p| !p.is_empty()).collect();
        let sounding_bars = distinct.len();
        distinct.sort();
        distinct.dedup();
        let m = TrackMetrics {
            track_id: t.id.to_string(),
            name: t.name.clone(),
            drums,
            notes: notes.len(),
            on_grid: (on_grid * 1000.0).round() / 1000.0,
            velocity_sd: (sd_v * 10.0).round() / 10.0,
            lowest: notes.iter().map(|x| x.2).min().unwrap_or(0),
            highest: notes.iter().map(|x| x.2).max().unwrap_or(0),
            expressive: (expressive * 1000.0).round() / 1000.0,
            automation_lanes: t.automation.len(),
            distinct_bars: distinct.len(),
            sounding_bars,
        };
        let name = t.name.clone();
        // 伸ばしのパッド(音の長さの中央値が 2 分音符以上)は、格子どおりでも不自然ではない
        let mut durs: Vec<u64> = notes.iter().map(|x| x.1).collect();
        durs.sort_unstable();
        let sustained = durs[durs.len() / 2] >= crate::time::PPQ * 2;
        if m.notes >= 16 && m.on_grid > 0.95 && !sustained {
            findings.push(Finding {
                severity: "warn",
                target: name.clone(),
                what: format!("{:.0}% の音が 16 分の格子ちょうどにあり、機械的に聞こえやすい", m.on_grid * 100.0),
                fix: "apply_groove(ドラムは型の style、ベースは as_part: \"kick\"、コードは \"hat\")で楽器ごとのずれと強弱を付ける".to_owned(),
            });
        }
        if m.notes >= 16 && m.velocity_sd < 6.0 {
            findings.push(Finding {
                severity: "warn",
                target: name.clone(),
                what: format!("強弱がほぼ平ら(ベロシティの標準偏差 {:.1})", m.velocity_sd),
                fix: "apply_groove の velocity(拍の位置のアクセント)、フレーズの山に向けた scale_velocity".to_owned(),
            });
        }
        if !drums && m.notes >= 16 && m.expressive == 0.0 && m.highest > 60 && sounding_bars >= 8 {
            findings.push(Finding {
                severity: "info",
                target: name.clone(),
                what: "奏法・ピッチカーブを 1 つも使っていない".to_owned(),
                fix: "リード・弦・管のつながったフレーズに legato / portamento、伸ばしに vibrato(get_guide expression)".to_owned(),
            });
        }
        if sounding_bars >= 16 && m.distinct_bars <= 1 && m.automation_lanes == 0 {
            findings.push(Finding {
                severity: "warn",
                target: name.clone(),
                what: format!("{sounding_bars} 小節ずっと同じ小節の繰り返しで、変化が無い"),
                fix: "区間の終わりにフィル、区間ごとに抜き差し、shape_automation でフィルタや音量を動かす".to_owned(),
            });
        }
        if m.lowest < 24 || m.highest > 108 {
            findings.push(Finding {
                severity: "info",
                target: name.clone(),
                what: format!("音域の端の音がある({}〜{})", m.lowest, m.highest),
                fix: "楽器の出る音域か確かめる(transpose_notes でオクターブを動かす)".to_owned(),
            });
        }
        tracks.push(m);
    }
    // 低域の渋滞: ドラム以外で、C3(48)より下で同時に鳴る音の間隔が 3 半音以内
    let mut low: Vec<(u64, u64, u8, String)> = Vec::new();
    for t in project.tracks.iter().filter(|t| !is_drum(t) && !t.mute) {
        for c in &t.clips {
            for n in sounding(c).into_iter().filter(|n| n.2 < 48) {
                low.push((n.0, n.0 + n.1, n.2, t.name.clone()));
            }
        }
    }
    low.sort_by_key(|x| x.0);
    let mut clash = 0usize;
    let mut pair: Option<(String, String)> = None;
    for (i, a) in low.iter().enumerate() {
        for b in low[i + 1..].iter().take_while(|b| b.0 < a.1) {
            if a.3 != b.3 && a.2.abs_diff(b.2) <= 3 && a.2 != b.2 {
                clash += 1;
                pair.get_or_insert((a.3.clone(), b.3.clone()));
            }
        }
    }
    if clash >= 4 {
        let (x, y) = pair.unwrap_or_default();
        findings.push(Finding {
            severity: "warn",
            target: format!("{x} / {y}"),
            what: format!("低い音域(C3 より下)で、別のトラックの音が 3 半音以内でぶつかる所が {clash} か所ある(濁る)"),
            fix: "片方をオクターブ上げる(transpose_notes)、低音はベースに任せて他は C3 より上に置く".to_owned(),
        });
    }
    // オートメーション
    let automation_lanes: usize = project
        .tracks
        .iter()
        .map(|t| t.automation.len())
        .sum::<usize>()
        + project.master.automation.len();
    if song_bars >= 32 && automation_lanes == 0 {
        findings.push(Finding {
            severity: "warn",
            target: String::new(),
            what: format!("{song_bars} 小節の曲でオートメーションが 1 本も無い(時間の中で音が動かない)"),
            fix: "shape_automation でビルドアップ(フィルタを exp で開く)・区間の頭の音量の出し入れ・パッドのスウェルを書く".to_owned(),
        });
    }
    // 区間
    let mut sections = Vec::new();
    let mut marks: Vec<_> = project.sections.iter().collect();
    marks.sort_by_key(|m| m.tick);
    let total_tracks = per_bar
        .iter()
        .filter(|p| p.1.iter().any(|c| *c > 0))
        .count()
        .max(1);
    for (i, m) in marks.iter().enumerate() {
        let b0 = bar_of(m.tick.0);
        let b1 = marks
            .get(i + 1)
            .map_or(song_bars, |n| bar_of(n.tick.0))
            .max(b0 + 1)
            .min(song_bars.max(b0 + 1));
        let bars = b1 - b0;
        let mut active = 0;
        let mut notes = 0usize;
        let (mut lo, mut hi) = (127u8, 0u8);
        for (_, counts, ranges) in &per_bar {
            let c: usize = counts
                .get(b0..b1.min(counts.len()))
                .map_or(0, |s| s.iter().sum());
            if c > 0 {
                active += 1;
                notes += c;
                for r in ranges.get(b0..b1.min(ranges.len())).unwrap_or(&[]) {
                    if r.0 <= r.1 {
                        lo = lo.min(r.0);
                        hi = hi.max(r.1);
                    }
                }
            }
        }
        let npb = notes as f64 / bars.max(1) as f64;
        let span = if hi >= lo { (hi - lo) as f64 } else { 0.0 };
        let energy = 10.0
            * (0.45 * active as f64 / total_tracks as f64
                + 0.35 * (npb / 64.0).min(1.0)
                + 0.2 * (span / 60.0).min(1.0));
        sections.push(SectionMetrics {
            name: m.name.clone(),
            start_bar: b0 + 1,
            bars,
            active_tracks: active,
            notes_per_bar: (npb * 10.0).round() / 10.0,
            energy: (energy * 10.0).round() / 10.0,
        });
    }
    if sections.is_empty() && song_bars >= 32 {
        findings.push(Finding {
            severity: "info",
            target: String::new(),
            what: "構成のマーカー(セクション)が無い".to_owned(),
            fix: "set_sections で intro / verse / chorus / drop などを置く(区間ごとの起伏を点検できるようになる)".to_owned(),
        });
    }
    if sections.len() >= 3 {
        let peak = sections.iter().map(|s| s.energy).fold(0.0, f64::max);
        let first = &sections[0];
        if peak > 0.0 && first.energy >= peak * 0.8 && first.bars >= 4 {
            findings.push(Finding {
                severity: "info",
                target: first.name.clone(),
                what: format!(
                    "最初の区間がすでに山とほぼ同じ厚さ(energy {:.1} / 山 {:.1})。入りから全部鳴ると、山で広がりが出にくい",
                    first.energy, peak
                ),
                fix: "イントロは楽器を絞る(ドラムかコードだけ・フィルタを閉じる)、区切りに向けて足していく".to_owned(),
            });
        }
        let es: Vec<f64> = sections.iter().map(|s| s.energy).collect();
        let spread = es.iter().cloned().fold(f64::MIN, f64::max)
            - es.iter().cloned().fold(f64::MAX, f64::min);
        if spread < 1.5 {
            findings.push(Finding {
                severity: "warn",
                target: String::new(),
                what: format!("区間ごとの盛り上がりの差が小さい(energy の幅 {spread:.1})。平板に聞こえやすい"),
                fix: "静かな区間はトラックを抜く・音を減らす、山の区間は足す・音域を広げる。区切りの前にフィル・ライザー(shape_automation)".to_owned(),
            });
        }
    }
    Critique {
        findings,
        tracks,
        sections,
        song_bars,
        automation_lanes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::{ClipId, NoteId, TrackId};
    use crate::model::{Articulation, Note, SectionMarker, TrackKind};
    use crate::time::Tick;

    fn note(pos: u64, pitch: u8, vel: u8) -> Note {
        Note {
            id: NoteId::new(),
            pos: Tick(pos),
            dur: Tick(200),
            pitch,
            vel,
            articulation: Articulation::Normal,
            pitch_curve: vec![],
            glide_ms: None,
        }
    }

    fn song(bars: u64, jitter: bool) -> Project {
        let mut p = Project::new("t");
        for (name, pitch) in [("Bass", 40u8), ("Keys", 64u8)] {
            let mut t = Track::new(TrackId::new(), name, TrackKind::Midi);
            let mut c = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(3840 * bars));
            if let ClipContent::Midi { notes, .. } = &mut c.content {
                for i in 0..bars * 8 {
                    let off = if jitter { i * 7 % 11 } else { 0 };
                    let vel = if jitter {
                        70 + (i * 13 % 40) as u8
                    } else {
                        100
                    };
                    notes.push(note(i * 480 + off, pitch, vel));
                }
            }
            t.clips.push(c);
            p.tracks.push(t);
        }
        p
    }

    #[test]
    fn flags_a_mechanical_static_song() {
        let c = critique(&song(40, false));
        let whats: Vec<&str> = c.findings.iter().map(|f| f.what.as_str()).collect();
        assert!(
            whats.iter().any(|w| w.contains("格子ちょうど")),
            "{whats:?}"
        );
        assert!(whats.iter().any(|w| w.contains("平ら")));
        assert!(whats
            .iter()
            .any(|w| w.contains("オートメーションが 1 本も無い")));
        assert!(whats.iter().any(|w| w.contains("同じ小節の繰り返し")));
        assert!(whats.iter().any(|w| w.contains("マーカー")));
        assert_eq!(c.song_bars, 40);
        assert_eq!(c.tracks[0].on_grid, 1.0);
    }

    #[test]
    fn a_humanized_song_is_not_called_mechanical() {
        let c = critique(&song(8, true));
        assert!(
            !c.findings
                .iter()
                .any(|f| f.what.contains("格子ちょうど") || f.what.contains("平ら")),
            "{:?}",
            c.findings
        );
    }

    #[test]
    fn sections_and_low_clashes() {
        let mut p = song(24, true);
        p.sections = vec![
            SectionMarker {
                tick: Tick(0),
                name: "intro".into(),
            },
            SectionMarker {
                tick: Tick(3840 * 8),
                name: "verse".into(),
            },
            SectionMarker {
                tick: Tick(3840 * 16),
                name: "chorus".into(),
            },
        ];
        let c = critique(&p);
        assert_eq!(c.sections.len(), 3);
        assert_eq!(c.sections[1].start_bar, 9);
        // 全区間で同じ密度 → 平板の指摘
        assert!(
            c.findings
                .iter()
                .any(|f| f.what.contains("盛り上がりの差が小さい")),
            "{:?}",
            c.findings
        );
        // 低域: Keys を E2(40)の 2 半音上(42)に下げると、Bass とぶつかる
        let mut p2 = song(8, true);
        if let ClipContent::Midi { notes, .. } = &mut p2.tracks[1].clips[0].content {
            notes.iter_mut().for_each(|n| n.pitch = 42);
        }
        assert!(critique(&p2)
            .findings
            .iter()
            .any(|f| f.what.contains("低い音域")));
    }
}
