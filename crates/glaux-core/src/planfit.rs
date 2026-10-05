//! 作る道具(write_*)がパートの計画を目安にする。
//!
//! トラックに採用済みのパートの計画があれば、区間ごとに:
//! - 鳴らさない区間(存在の段階 0)と固定の区間には書かない
//! - 音域の帯から外れた音を帯へ寄せる(まず区間の音をまとめてオクターブで動かして旋律の形を保ち、
//!   それでも外れる音だけを 1 つずつオクターブで折り返す)
//!
//! 推定の計画(state: estimated)と案(state: proposal)は目安にしない(人が確かめた計画だけ)。

use crate::model::{Note, Project};
use crate::plan::{PartPlan, Plan, PlanSet};
use serde::Serialize;

/// 区間 1 つぶんの合わせた結果
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SectionFit {
    /// 区間の名前
    pub section: String,
    /// 外した音の数(鳴らさない区間・固定の区間)
    #[serde(skip_serializing_if = "is_zero")]
    pub removed: usize,
    /// 外した理由("鳴らさない" / "固定")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
    /// 区間の音をまとめて動かしたオクターブ(+ で上)
    #[serde(skip_serializing_if = "is_zero_i")]
    pub octave: i32,
    /// 1 つずつ折り返した音の数
    #[serde(skip_serializing_if = "is_zero")]
    pub folded: usize,
}

/// 計画に合わせた結果(応答に載せる)
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PlanFit {
    pub plan_id: String,
    pub plan: String,
    /// 何か変えた区間だけ
    pub sections: Vec<SectionFit>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

fn is_zero_i(n: &i32) -> bool {
    *n == 0
}

/// トラック `track_id` の、採用済みのパートの計画
pub fn part_plan_for<'a>(plans: &'a PlanSet, track_id: &str) -> Option<(&'a Plan, PartPlan)> {
    plans
        .plans
        .values()
        .filter(|p| p.kind == "part" && p.state.is_none())
        .find_map(|p| {
            let pp: PartPlan = serde_json::from_value(p.body.clone()).ok()?;
            (pp.track.as_deref() == Some(track_id)).then_some((p, pp))
        })
}

/// 区間の ID → 曲の上の範囲 [始まり, 終わり)(最後の区間は曲の終わりまで)
fn section_ranges(project: &Project) -> Vec<(String, String, u64, u64)> {
    let mut secs: Vec<_> = project
        .sections
        .iter()
        .filter_map(|s| Some((s.id.as_ref()?.to_string(), s.name.clone(), s.tick.0)))
        .collect();
    secs.sort_by_key(|s| s.2);
    (0..secs.len())
        .map(|i| {
            let end = secs.get(i + 1).map_or(u64::MAX, |n| n.2);
            (secs[i].0.clone(), secs[i].1.clone(), secs[i].2, end)
        })
        .collect()
}

/// 曲の範囲 [start, end) にかかる区間の計画の音域の帯をまとめたもの(いちばん低い下端・いちばん高い上端)。
/// 範囲を指定されなかった道具が、作るときの音域に使う
pub fn register_for_range(
    project: &Project,
    pp: &PartPlan,
    start: u64,
    end: u64,
) -> Option<(u8, u8)> {
    let ranges = section_ranges(project);
    let mut out: Option<(u8, u8)> = None;
    for s in &pp.sections {
        let Some([lo, hi]) = s.register else { continue };
        if s.presence == 0 || lo > hi {
            continue;
        }
        let Some((_, _, a, b)) = ranges.iter().find(|r| r.0 == s.section) else {
            continue;
        };
        if *a < end && *b > start {
            out = Some(out.map_or((lo, hi), |(l, h)| (l.min(lo), h.max(hi))));
        }
    }
    out
}

/// 音を帯 [lo, hi] へオクターブで折り返す(帯が 1 オクターブより狭くて入らなければ、いちばん近い所)
fn fold_into(pitch: u8, lo: u8, hi: u8) -> u8 {
    let mut p = pitch as i32;
    while p < lo as i32 && p + 12 <= 127 {
        p += 12;
    }
    while p > hi as i32 && p - 12 >= 0 {
        p -= 12;
    }
    if p < lo as i32 || p > hi as i32 {
        // 入らないときは、上下のどちらか近い方の候補
        let up = (p + 12).min(127);
        let dist = |q: i32| (q - lo as i32).max(0) + (q - hi as i32).max(0);
        if dist(up) < dist(p) {
            p = up;
        }
    }
    p.clamp(0, 127) as u8
}

/// クリップの頭が `clip_start` の音 `notes`(位置はクリップの頭から)を、パートの計画に合わせる。
/// `register` が false なら音域は動かさない(ドラムなど)
pub fn fit_notes(
    project: &Project,
    plan: &Plan,
    pp: &PartPlan,
    clip_start: u64,
    notes: &mut Vec<Note>,
    register: bool,
) -> PlanFit {
    let ranges = section_ranges(project);
    let mut sections = Vec::new();
    for s in &pp.sections {
        let Some((_, name, a, b)) = ranges.iter().find(|r| r.0 == s.section) else {
            continue;
        };
        let inside = |n: &Note| (*a..*b).contains(&(clip_start + n.pos.0));
        let mut fit = SectionFit {
            section: name.clone(),
            removed: 0,
            reason: None,
            octave: 0,
            folded: 0,
        };
        if s.presence == 0 || s.locked {
            let before = notes.len();
            notes.retain(|n| !inside(n));
            fit.removed = before - notes.len();
            fit.reason = Some(if s.locked {
                "固定"
            } else {
                "鳴らさない"
            });
        } else if let (true, Some([lo, hi])) = (register, s.register) {
            if lo <= hi {
                let idx: Vec<usize> = (0..notes.len()).filter(|&i| inside(&notes[i])).collect();
                // まとめて動かすオクターブ: 帯に入る音がいちばん多いもの(同じなら動かさない方へ)
                let count = |k: i32| {
                    idx.iter()
                        .filter(|&&i| {
                            let p = notes[i].pitch as i32 + 12 * k;
                            p >= lo as i32 && p <= hi as i32
                        })
                        .count()
                };
                let best = (-3..=3).max_by_key(|&k| (count(k), -k.abs())).unwrap_or(0);
                if best != 0 && count(best) > count(0) {
                    for &i in &idx {
                        notes[i].pitch = (notes[i].pitch as i32 + 12 * best).clamp(0, 127) as u8;
                    }
                    fit.octave = best;
                }
                for &i in &idx {
                    let p = notes[i].pitch;
                    if p < lo || p > hi {
                        let q = fold_into(p, lo, hi);
                        if q != p {
                            notes[i].pitch = q;
                            fit.folded += 1;
                        }
                    }
                }
            }
        }
        if fit.removed > 0 || fit.octave != 0 || fit.folded > 0 {
            sections.push(fit);
        }
    }
    // 折り返しで同じ位置・同じ高さに重なった音は 1 つにする
    notes.sort_by_key(|n| (n.pos, n.pitch));
    notes.dedup_by(|x, y| x.pos == y.pos && x.pitch == y.pitch);
    PlanFit {
        plan_id: plan.id.to_string(),
        plan: plan.name.clone(),
        sections,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::{NoteId, SectionId};
    use crate::model::SectionMarker;
    use crate::plan::PartSectionPlan;
    use crate::time::Tick;

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

    fn song() -> (Project, Vec<String>) {
        let mut p = Project::new("t");
        let ids: Vec<SectionId> = (0..3).map(|_| SectionId::new()).collect();
        for (i, (id, name)) in ids.iter().zip(["A", "B", "C"]).enumerate() {
            p.sections.push(SectionMarker {
                id: Some(id.clone()),
                tick: Tick(3840 * 4 * i as u64),
                name: name.to_owned(),
                energy: None,
                tracks: vec![],
                note: None,
                curve: vec![],
                join: None,
            });
        }
        (p, ids.iter().map(|i| i.to_string()).collect())
    }

    fn plan(ids: &[String]) -> (Plan, PartPlan) {
        let pp = PartPlan {
            track: Some("trk_bass01".to_owned()),
            function: Some("bass".to_owned()),
            note: None,
            sections: vec![
                PartSectionPlan {
                    section: ids[0].clone(),
                    presence: 0,
                    ..Default::default()
                },
                PartSectionPlan {
                    section: ids[1].clone(),
                    presence: 3,
                    register: Some([28, 48]),
                    ..Default::default()
                },
                PartSectionPlan {
                    section: ids[2].clone(),
                    presence: 4,
                    register: Some([28, 40]),
                    locked: true,
                    ..Default::default()
                },
            ],
        };
        let p = Plan {
            id: crate::id::PlanId::new(),
            name: "Bass".to_owned(),
            kind: "part".to_owned(),
            rev: 1,
            derived_from: None,
            state: None,
            body: serde_json::to_value(&pp).unwrap(),
            patch: vec![],
            patch_base: Default::default(),
        };
        (p, pp)
    }

    #[test]
    fn silent_and_locked_sections_are_left_empty_and_notes_fit_the_band() {
        let (project, ids) = song();
        let (pl, pp) = plan(&ids);
        // 区間 A(鳴らさない)に 2 音、B に 1 オクターブ高すぎる旋律 3 音 + 1 つだけ飛び出す音、C(固定)に 1 音
        let mut notes = vec![
            note(0, 40),
            note(3840, 43),
            note(15360, 57),
            note(15360 + 960, 59),
            note(15360 + 1920, 55),
            note(15360 + 2880, 81),
            note(30720, 40),
        ];
        let fit = fit_notes(&project, &pl, &pp, 0, &mut notes, true);
        let pitches: Vec<(u64, u8)> = notes.iter().map(|n| (n.pos.0, n.pitch)).collect();
        // A と C の音は無くなり、B はまとめて 1 オクターブ下へ(形を保つ)、残りの 1 音は折り返し
        assert_eq!(
            pitches,
            vec![
                (15360, 45),
                (15360 + 960, 47),
                (15360 + 1920, 43),
                (15360 + 2880, 45)
            ]
        );
        let by: std::collections::BTreeMap<&str, &SectionFit> = fit
            .sections
            .iter()
            .map(|s| (s.section.as_str(), s))
            .collect();
        assert_eq!((by["A"].removed, by["A"].reason), (2, Some("鳴らさない")));
        assert_eq!((by["B"].octave, by["B"].folded), (-1, 1));
        assert_eq!((by["C"].removed, by["C"].reason), (1, Some("固定")));
        // 音域を動かさない(ドラム)ときは、区間で外すだけ
        let mut drums = vec![note(0, 36), note(15360, 36)];
        let fit = fit_notes(&project, &pl, &pp, 0, &mut drums, false);
        assert_eq!(drums.len(), 1);
        assert_eq!(fit.sections.len(), 1);
    }

    #[test]
    fn the_plan_register_covers_the_sections_in_range() {
        let (project, ids) = song();
        let (_, pp) = plan(&ids);
        // B だけ → B の帯。B と C → 両方をまとめた帯。A(鳴らさない)だけ → 無し
        assert_eq!(
            register_for_range(&project, &pp, 15360, 30720),
            Some((28, 48))
        );
        assert_eq!(
            register_for_range(&project, &pp, 15360, 46080),
            Some((28, 48))
        );
        assert_eq!(register_for_range(&project, &pp, 0, 15360), None);
    }

    #[test]
    fn only_adopted_part_plans_are_followed() {
        let (_, ids) = song();
        let (mut pl, _) = plan(&ids);
        let mut set = PlanSet::default();
        set.plans.insert(pl.id.clone(), pl.clone());
        assert!(part_plan_for(&set, "trk_bass01").is_some());
        assert!(part_plan_for(&set, "trk_other1").is_none());
        for st in ["estimated", "proposal"] {
            pl.state = Some(st.to_owned());
            set.plans.insert(pl.id.clone(), pl.clone());
            assert!(part_plan_for(&set, "trk_bass01").is_none());
        }
    }
}
