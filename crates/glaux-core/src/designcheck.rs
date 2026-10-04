//! 曲の設計データ(計画)と実際の音を並べて比べる(曲の設計データの段階 0。MCP の get_design、設計画面の土台)。
//!
//! - 区間: 計画の盛り上がり(形の平均)と、測った盛り上がり([`crate::critique`] の区間の energy)
//! - パート × 区間: 計画の存在の段階・音域の帯と、測った値(鳴っているか・音の数・音量から見た段階・
//!   実際の音域〈下 10%〜上 90%〉・密度)
//! - ずれ: 計画と実際の食い違い。直すのは計画か音のどちらか(人と AI に同じものを見せる)
//! - クリップの状態: 計画どおり / 計画が先に進んだ、手で直した小節、固定の音の数
//!
//! 推定した計画(state: estimated)は参考として並べるが、ずれの基準にはしない

use crate::id::{PlanId, TrackId};
use crate::model::{Project, SectionJoin, Track};
use crate::plan::{PartPlan, PlanSet, SongPlan, PRESENCE};
use serde::Serialize;

/// 区間 1 つ
#[derive(Clone, Debug, Serialize)]
pub struct SectionView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    /// 始まりの小節(1 始まり)と長さ(小節)
    pub start_bar: usize,
    pub bars: usize,
    /// 計画の盛り上がり(形の平均。形が無ければ energy)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub planned: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub curve: Vec<[f32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub join: Option<SectionJoin>,
    /// 測った盛り上がり 0〜10
    pub measured: f64,
}

/// パート(トラック)× 区間の 1 マス
#[derive(Clone, Debug, Serialize)]
pub struct CellView {
    /// 区間の ID(無い曲は区間の名前)
    pub section: String,
    /// 計画の存在の段階・働き・音域の帯(パートの計画が無ければ省略)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub planned: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub planned_register: Option<[u8; 2]>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
    /// 測った存在の段階(鳴っているか・音量の順位から。0 = 鳴っていない)
    pub measured: u8,
    pub notes: usize,
    /// 実際の音域(下 10%〜上 90%)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub register: Option<[u8; 2]>,
    /// 密度 0〜1(1 小節 16 音で 1)
    pub density: f32,
}

/// パート(トラック)1 つ
#[derive(Clone, Debug, Serialize)]
pub struct PartView {
    pub track_id: TrackId,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// パートの計画(無ければ省略)と、その既定の働き
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_id: Option<PlanId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    /// 計画が推定(まだ人が確かめていない)なら true
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub estimated: bool,
    pub cells: Vec<CellView>,
}

/// 計画と実際のずれ 1 つ
#[derive(Clone, Debug, Serialize)]
pub struct Deviation {
    /// "warn"(食い違っている)/ "info"(検討)
    pub severity: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track: Option<String>,
    pub what: String,
    /// 直し方(計画を実際に合わせる / 音を計画に合わせる)
    pub fix: String,
}

/// 計画から作った・AI が作ったクリップの状態
#[derive(Clone, Debug, Serialize)]
pub struct ClipView {
    pub clip_id: crate::id::ClipId,
    pub track: String,
    /// in_sync(計画どおり)/ ahead(計画が先に進んだ。作り直せる)/ missing(計画が無い)。計画の参照が無ければ省略
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<&'static str>,
    /// 人が手で直した小節(1 始まり、[最初, 最後])
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub edited_bars: Vec<[u32; 2]>,
    /// 固定の音の数
    #[serde(skip_serializing_if = "is_zero")]
    pub locked_notes: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// 計画と実際をまとめたもの
#[derive(Clone, Debug, Serialize)]
pub struct DesignView {
    /// 曲全体の計画(無ければ省略)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub song: Option<SongPlan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub song_plan_id: Option<PlanId>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub song_estimated: bool,
    pub sections: Vec<SectionView>,
    pub parts: Vec<PartView>,
    pub deviations: Vec<Deviation>,
    pub clips: Vec<ClipView>,
}

/// 区間の小節の範囲 [b0, b1)(0 始まり)と、その tick の範囲
struct Span {
    b0: usize,
    b1: usize,
    t0: u64,
    t1: u64,
}

fn percentile(sorted: &[u8], q: f64) -> u8 {
    if sorted.is_empty() {
        return 0;
    }
    let i = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

fn note_name(p: u8) -> String {
    const N: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!("{}{}", N[(p % 12) as usize], p as i32 / 12 - 1)
}

fn is_drum(t: &Track) -> bool {
    t.device.as_ref().is_some_and(|d| d.source.is_drum_kit())
}

/// 計画と実際を並べ、ずれを出す
pub fn design_view(project: &Project, plans: &PlanSet) -> DesignView {
    let critique = crate::critique::critique(project);
    let end = project.end().0;
    let grid = crate::arrange::bar_grid(project, end.max(1));
    let bar_of = |t: u64| grid.partition_point(|(s, _)| *s <= t).saturating_sub(1);
    let song_bars = grid.iter().filter(|(s, _)| *s < end).count().max(1);
    let tick_of = |b: usize| grid.get(b).map_or(end, |g| g.0);
    let mut marks: Vec<_> = project.sections.iter().collect();
    marks.sort_by_key(|m| m.tick);
    // 区間が無い曲は曲全体を 1 区間として扱う
    let spans: Vec<Span> = if marks.is_empty() {
        vec![Span {
            b0: 0,
            b1: song_bars,
            t0: 0,
            t1: end,
        }]
    } else {
        marks
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let b0 = bar_of(m.tick.0);
                let b1 = marks
                    .get(i + 1)
                    .map_or(song_bars, |n| bar_of(n.tick.0))
                    .max(b0 + 1);
                Span {
                    b0,
                    b1,
                    t0: m.tick.0,
                    t1: marks.get(i + 1).map_or(tick_of(b1).max(end), |n| n.tick.0),
                }
            })
            .collect()
    };
    let section_key = |i: usize| -> String {
        marks
            .get(i)
            .map(|m| {
                m.id.as_ref()
                    .map_or_else(|| m.name.clone(), |id| id.to_string())
            })
            .unwrap_or_else(|| "song".to_owned())
    };
    let section_name = |i: usize| -> String {
        marks
            .get(i)
            .map_or_else(|| "曲全体".to_owned(), |m| m.name.clone())
    };
    let sections: Vec<SectionView> = spans
        .iter()
        .enumerate()
        .map(|(i, s)| SectionView {
            id: marks
                .get(i)
                .and_then(|m| m.id.as_ref().map(|x| x.to_string())),
            name: section_name(i),
            start_bar: s.b0 + 1,
            bars: s.b1 - s.b0,
            planned: marks.get(i).and_then(|m| m.energy_mean()),
            curve: marks.get(i).map(|m| m.curve.clone()).unwrap_or_default(),
            join: marks.get(i).and_then(|m| m.join),
            measured: critique.sections.get(i).map_or(0.0, |c| c.energy),
        })
        .collect();

    // 計画(曲全体・パート)
    let mut song = None;
    let mut song_plan_id = None;
    let mut song_estimated = false;
    let mut part_plans: Vec<(&PlanId, PartPlan, bool)> = Vec::new();
    for (id, p) in &plans.plans {
        let estimated = p.state.as_deref() == Some("estimated");
        match p.kind.as_str() {
            "song" if song.is_none() || (song_estimated && !estimated) => {
                if let Ok(s) = serde_json::from_value::<SongPlan>(p.body.clone()) {
                    song = Some(s);
                    song_plan_id = Some(id.clone());
                    song_estimated = estimated;
                }
            }
            "part" => {
                if let Ok(pp) = serde_json::from_value::<PartPlan>(p.body.clone()) {
                    part_plans.push((id, pp, estimated));
                }
            }
            _ => {}
        }
    }

    // パート × 区間を測る
    struct Raw {
        notes: usize,
        level: f64,
        pitches: Vec<u8>,
    }
    let mut parts: Vec<PartView> = Vec::new();
    let mut raws: Vec<Vec<Raw>> = Vec::new();
    for t in &project.tracks {
        if t.kind != crate::model::TrackKind::Midi {
            continue;
        }
        let mut per: Vec<Raw> = spans
            .iter()
            .map(|_| Raw {
                notes: 0,
                level: f64::NEG_INFINITY,
                pitches: vec![],
            })
            .collect();
        let mut vel_sum = vec![0f64; spans.len()];
        for c in &t.clips {
            for n in c.playback_notes() {
                let at = c.start.0 + n.pos.0;
                let Some(i) = spans.iter().position(|s| s.t0 <= at && at < s.t1) else {
                    continue;
                };
                per[i].notes += 1;
                per[i].pitches.push(n.pitch);
                vel_sum[i] += n.vel as f64;
            }
        }
        for (i, r) in per.iter_mut().enumerate() {
            r.pitches.sort_unstable();
            if r.notes > 0 && !t.mute {
                let mean_vel = vel_sum[i] / r.notes as f64;
                r.level = t.volume_db as f64 + 20.0 * (mean_vel / 127.0).max(1e-3).log10();
            }
        }
        // このトラックの計画(採用済みを先に)
        let plan = part_plans
            .iter()
            .filter(|(_, pp, _)| pp.track.as_deref() == Some(t.id.as_str()))
            .min_by_key(|(_, _, est)| *est);
        parts.push(PartView {
            track_id: t.id.clone(),
            name: t.name.clone(),
            color: t.color.clone(),
            plan_id: plan.map(|(id, _, _)| (*id).clone()),
            function: plan.and_then(|(_, pp, _)| pp.function.clone()),
            estimated: plan.is_some_and(|(_, _, e)| *e),
            cells: vec![],
        });
        raws.push(per);
    }
    // 区間ごとに、鳴っているトラックを音量の順に並べて段階を決める
    for (si, span) in spans.iter().enumerate() {
        let mut order: Vec<(usize, f64)> = raws
            .iter()
            .enumerate()
            .filter(|(_, r)| r[si].notes > 0 && r[si].level.is_finite())
            .map(|(pi, r)| (pi, r[si].level))
            .collect();
        order.sort_by(|a, b| b.1.total_cmp(&a.1));
        let bars = (span.b1 - span.b0).max(1) as f32;
        for (pi, part) in parts.iter_mut().enumerate() {
            let r = &raws[pi][si];
            let rank = order.iter().position(|(k, _)| *k == pi);
            let drum = project.track(&part.track_id).is_some_and(is_drum);
            let measured = match rank {
                None => 0,
                Some(_) if r.level < -30.0 => 1,
                Some(_) if r.level < -20.0 => 2,
                Some(0) if !drum => 5,
                Some(k) if k <= 2 => 4,
                Some(_) => 3,
            };
            let key = section_key(si);
            let plan = part_plans
                .iter()
                .filter(|(_, pp, _)| pp.track.as_deref() == Some(part.track_id.as_str()))
                .min_by_key(|(_, _, est)| *est)
                .and_then(|(_, pp, _)| pp.section(&key).cloned());
            part.cells.push(CellView {
                section: key,
                planned: plan.as_ref().map(|p| p.presence),
                function: plan.as_ref().and_then(|p| p.function.clone()),
                planned_register: plan.as_ref().and_then(|p| p.register),
                locked: plan.as_ref().is_some_and(|p| p.locked),
                measured,
                notes: r.notes,
                register: (!r.pitches.is_empty())
                    .then(|| [percentile(&r.pitches, 0.1), percentile(&r.pitches, 0.9)]),
                density: ((r.notes as f32 / bars / 16.0).min(1.0) * 100.0).round() / 100.0,
            });
        }
    }

    // ずれ
    let mut deviations = Vec::new();
    for (i, s) in sections.iter().enumerate() {
        if let Some(p) = s.planned {
            let d = s.measured - p as f64;
            if d.abs() >= 1.5 {
                deviations.push(Deviation {
                    severity: "warn",
                    section: Some(section_key(i)),
                    track: None,
                    what: format!(
                        "「{}」の盛り上がりが計画より{}(計画 {:.1} / 実際 {:.1})",
                        s.name,
                        if d > 0.0 { "高い" } else { "低い" },
                        p,
                        s.measured
                    ),
                    fix: "計画の曲線を実際に合わせる(set_song_plan の energy・curve)か、パートの出入り・音の数を計画に合わせる"
                        .to_owned(),
                });
            }
        }
    }
    for part in &parts {
        if part.estimated {
            continue;
        }
        for (i, c) in part.cells.iter().enumerate() {
            let Some(planned) = c.planned else {
                continue;
            };
            let at = |what: String, fix: &str, severity: &'static str| Deviation {
                severity,
                section: Some(c.section.clone()),
                track: Some(part.name.clone()),
                what,
                fix: fix.to_owned(),
            };
            let name = &sections[i].name;
            if planned == 0 && c.notes > 0 {
                deviations.push(at(
                    format!(
                        "「{name}」の {} は鳴らさない計画なのに鳴っている({} 音)",
                        part.name, c.notes
                    ),
                    "計画の段階を上げるか、その区間の音を消す",
                    "warn",
                ));
            } else if planned > 0 && c.notes == 0 {
                deviations.push(at(
                    format!(
                        "「{name}」の {} は「{}」の計画なのに鳴っていない",
                        part.name, PRESENCE[planned as usize]
                    ),
                    "その区間に書く(write_* の道具)か、計画を「鳴らさない」にする",
                    "warn",
                ));
            } else if planned > 0 && (planned as i32 - c.measured as i32).abs() >= 2 {
                deviations.push(at(
                    format!(
                        "「{name}」の {} は計画「{}」に対して、実際は「{}」くらい",
                        part.name, PRESENCE[planned as usize], PRESENCE[c.measured as usize]
                    ),
                    "音量・音の数・音域で前後を整えるか、計画の段階を実際に合わせる",
                    "info",
                ));
            }
            if let (Some([lo, hi]), Some([a, b])) = (c.planned_register, c.register) {
                if (a as i32) < lo as i32 - 2 || (b as i32) > hi as i32 + 2 {
                    deviations.push(at(
                        format!(
                            "「{name}」の {} が音域の帯から外れている(計画 {}〜{} / 実際 {}〜{})",
                            part.name,
                            note_name(lo),
                            note_name(hi),
                            note_name(a),
                            note_name(b)
                        ),
                        "音を帯の中へ移す(transpose_notes)か、計画の帯を広げる",
                        "warn",
                    ));
                }
            }
        }
    }
    for (i, s) in sections.iter().enumerate() {
        let front = parts
            .iter()
            .filter(|p| !p.estimated)
            .filter(|p| {
                p.cells
                    .get(i)
                    .and_then(|c| c.planned)
                    .is_some_and(|v| v >= 4)
            })
            .count();
        if front >= 5 {
            deviations.push(Deviation {
                severity: "info",
                section: Some(section_key(i)),
                track: None,
                what: format!(
                    "「{}」で前面・主役の計画のパートが {front} つ(同時に前に出るのは 3〜4 つまでが目安)",
                    s.name
                ),
                fix: "いくつかを「支え」「背景」に下げる".to_owned(),
            });
        }
    }

    let clips = clip_states(project, plans);

    DesignView {
        song,
        song_plan_id,
        song_estimated,
        sections,
        parts,
        deviations,
        clips,
    }
}

/// 計画から作った・AI が作ったクリップの状態(計画どおり / 先に進んだ・手で直した小節・固定の音の数)。
/// 何も無いクリップは含めない。タイムラインのクリップの印に使う(盛り上がりなどは測らないので軽い)
pub fn clip_states(project: &Project, plans: &PlanSet) -> Vec<ClipView> {
    let mut clips = Vec::new();
    for t in &project.tracks {
        for c in &t.clips {
            let locked_notes = c
                .notes()
                .map_or(0, |ns| ns.iter().filter(|n| n.locked).count());
            let plan = project
                .plan_refs
                .get(&c.id)
                .map(|r| match plans.plans.get(&r.id) {
                    None => "missing",
                    Some(p)
                        if p.digest() == r.digest || (r.digest.is_empty() && p.rev == r.rev) =>
                    {
                        "in_sync"
                    }
                    Some(_) => "ahead",
                });
            let edited_bars: Vec<[u32; 2]> = crate::made::edited_spans(project, &c.id)
                .into_iter()
                .map(|(a, b)| crate::made::bars_of(project, a, b))
                .collect();
            if plan.is_none() && edited_bars.is_empty() && locked_notes == 0 {
                continue;
            }
            clips.push(ClipView {
                clip_id: c.id.clone(),
                track: t.name.clone(),
                plan,
                edited_bars,
                locked_notes,
            });
        }
    }
    clips
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Command;
    use crate::history::Document;
    use crate::id::{ClipId, NoteId, SectionId};
    use crate::model::{Clip, Note, SectionMarker, TrackKind};
    use crate::plan::{Plan, PlanCommand};
    use crate::time::Tick;
    use serde_json::json;

    fn note(pos: u64, pitch: u8) -> Note {
        Note {
            id: NoteId::new(),
            pos: Tick(pos),
            dur: Tick(480),
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

    #[test]
    fn plans_and_reality_are_compared() {
        let mut p = Project::new("t");
        let (s1, s2) = (SectionId::new(), SectionId::new());
        p.apply(&Command::SetSections {
            sections: vec![
                SectionMarker {
                    id: Some(s1.clone()),
                    tick: Tick(0),
                    name: "intro".into(),
                    energy: Some(2.0),
                    ..Default::default()
                },
                SectionMarker {
                    id: Some(s2.clone()),
                    tick: Tick(3840 * 4),
                    name: "drop".into(),
                    curve: vec![[0.0, 8.0], [1.0, 10.0]],
                    ..Default::default()
                },
            ],
        })
        .unwrap();
        let tid = TrackId::new();
        let mut t = Track::new(tid.clone(), "Lead", TrackKind::Midi);
        let mut c = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(3840 * 8));
        if let Some(ns) = c.notes_mut() {
            // イントロにも鳴っている(計画は鳴らさない)、ドロップは C5 付近(計画の帯は C3〜C4)
            ns.push(note(0, 60));
            for b in 4..8u64 {
                ns.push(note(b * 3840, 72));
                ns.push(note(b * 3840 + 960, 74));
            }
            ns[1].locked = true;
        }
        t.clips.push(c);
        p.tracks.push(t);
        let mut plans = PlanSet::default();
        plans
            .apply_command(&PlanCommand::Create {
                plan: Plan {
                    id: PlanId::new(),
                    name: "Lead".into(),
                    kind: "part".into(),
                    rev: 1,
                    derived_from: None,
                    state: None,
                    body: json!({
                        "track": tid.to_string(),
                        "function": "lead",
                        "sections": [
                            { "section": s1.to_string(), "presence": 0 },
                            { "section": s2.to_string(), "presence": 5, "register": [48, 60] }
                        ]
                    }),
                },
            })
            .unwrap();
        let v = design_view(&p, &plans);
        assert_eq!(v.sections.len(), 2);
        assert_eq!(v.sections[1].planned, Some(9.0));
        assert_eq!(v.parts.len(), 1);
        let cells = &v.parts[0].cells;
        assert_eq!(cells[0].planned, Some(0));
        assert_eq!(cells[0].notes, 1);
        assert_eq!(cells[1].measured, 5);
        assert_eq!(cells[1].register, Some([72, 74]));
        let text: Vec<&str> = v.deviations.iter().map(|d| d.what.as_str()).collect();
        assert!(
            text.iter()
                .any(|t| t.contains("鳴らさない計画なのに鳴っている")),
            "{text:?}"
        );
        assert!(
            text.iter().any(|t| t.contains("音域の帯から外れている")),
            "{text:?}"
        );
        // 固定の音のあるクリップは状態に出る
        assert_eq!(v.clips.len(), 1);
        assert_eq!(v.clips[0].locked_notes, 1);
        // 推定の計画はずれの基準にしない
        let id = v.parts[0].plan_id.clone().unwrap();
        let mut est = plans.plans[&id].clone();
        est.state = Some("estimated".into());
        plans
            .apply_command(&PlanCommand::Replace { plan: est })
            .unwrap();
        let v = design_view(&p, &plans);
        assert!(v.parts[0].estimated);
        assert!(
            v.deviations.iter().all(|d| d.track.is_none()),
            "{:?}",
            v.deviations
        );
    }
}
