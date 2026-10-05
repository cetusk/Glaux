//! 今の音から計画を推定する(計画の無い曲で、設計画面をすぐ使い始められるように)。
//!
//! 計画の無いパートごとに、区間ごとの存在の段階と音域の帯を測った値から決め、働きを音源・音の高さ・同時に鳴る音の数から推し量る。
//! 曲全体の計画が無ければ、盛り上がりの型(区間ごとの実測)とキーを推定する。どれも「推定」(state: estimated)として保存し、
//! 人が確かめて採用するまで AI は参考としてだけ使う。区間のマーカー(ID 付き)が無い曲では推定しない。

use crate::designcheck::design_view;
use crate::model::{Project, Track};
use crate::plan::{PartPlan, PartSectionPlan, PlanSet, SongPlan};

/// 推定した計画
#[derive(Clone, Debug, PartialEq)]
pub struct Estimate {
    /// 曲全体の計画(すでにあれば None)
    pub song: Option<SongPlan>,
    /// (トラックの名前, パートの計画)。計画の無いパートだけ
    pub parts: Vec<(String, PartPlan)>,
}

fn is_drum(t: &Track) -> bool {
    t.device.as_ref().is_some_and(|d| d.source.is_drum_kit())
}

/// トラックの名前から働きを推し量る(名前で分からなければ None)
fn function_by_name(name: &str) -> Option<&'static str> {
    let n = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if has(&[
        "kick",
        "snare",
        "clap",
        "hat",
        "perc",
        "drum",
        "crash",
        "cymbal",
        "ride",
        "tom",
        "shaker",
        "キック",
        "スネア",
        "クラップ",
        "ハット",
        "ドラム",
        "パーカッション",
    ]) {
        Some("beat")
    } else if has(&[
        "riser",
        "sweep",
        "impact",
        "uplift",
        "downlift",
        "transition",
        "ライザー",
        "スイープ",
    ]) {
        Some("transition")
    } else if has(&[
        "lead",
        "melody",
        "vocal",
        "vox",
        "リード",
        "メロ",
        "歌",
        "ボーカル",
    ]) {
        Some("lead")
    } else if has(&["arp", "アルペ"]) {
        Some("rhythm")
    } else if has(&["sub"]) {
        Some("sub")
    } else if has(&["bass", "ベース", "808"]) {
        Some("bass")
    } else {
        None
    }
}

/// 音の様子: (音の高さの中央値, 音の頭で同時に鳴っている音の数の平均, 長さの中央値〈tick〉, 音の数)
fn pitch_stats(t: &Track) -> Option<(u8, f64, u64, usize)> {
    let mut notes: Vec<(u64, u64, u8)> = t
        .clips
        .iter()
        .flat_map(|c| {
            c.playback_notes()
                .into_iter()
                .map(move |n| (c.start.0 + n.pos.0, c.start.0 + n.pos.0 + n.dur.0, n.pitch))
        })
        .collect();
    if notes.is_empty() {
        return None;
    }
    notes.sort_unstable();
    let mut pitches: Vec<u8> = notes.iter().map(|n| n.2).collect();
    pitches.sort_unstable();
    let median = pitches[pitches.len() / 2];
    let sounding: usize = notes
        .iter()
        .map(|&(at, _, _)| notes.iter().filter(|o| o.0 <= at && at < o.1).count())
        .sum();
    let mut durs: Vec<u64> = notes.iter().map(|n| n.1 - n.0).collect();
    durs.sort_unstable();
    Some((
        median,
        sounding as f64 / notes.len() as f64,
        durs[durs.len() / 2],
        notes.len(),
    ))
}

/// 区間ごとの盛り上がり(実測)から、盛り上がりの型([`crate::plan::ARCS`] のキー)
fn arc_of(e: &[f64]) -> Option<&'static str> {
    if e.len() < 2 {
        return None;
    }
    let (lo, hi) = e
        .iter()
        .fold((f64::MAX, f64::MIN), |(l, h), &x| (l.min(x), h.max(x)));
    if hi - lo < 1.5 {
        return Some("flat");
    }
    if e.windows(2).all(|w| w[1] >= w[0] - 0.5) {
        return Some("rise");
    }
    if e.windows(2).all(|w| w[1] <= w[0] + 0.5) {
        return Some("sink");
    }
    // 山の数: 両隣より 1 以上高い区間(端は片側だけ見る)
    let peaks = (0..e.len())
        .filter(|&i| {
            let l = if i > 0 {
                e[i] - e[i - 1]
            } else {
                f64::INFINITY
            };
            let r = if i + 1 < e.len() {
                e[i] - e[i + 1]
            } else {
                f64::INFINITY
            };
            l >= 1.0 && r >= 1.0
        })
        .count();
    Some(if peaks >= 2 { "waves" } else { "peak" })
}

/// 計画の無い所を、今の音から推定する
pub fn estimate(project: &Project, plans: &PlanSet) -> Estimate {
    let view = design_view(project, plans);
    // 区間の ID が無い(マーカーが無い)曲では推定しない
    if view.sections.is_empty() || view.sections.iter().any(|s| s.id.is_none()) {
        return Estimate {
            song: None,
            parts: vec![],
        };
    }
    // 音の無い曲(作り始めたばかり)では推定しない
    if !view
        .parts
        .iter()
        .any(|p| p.cells.iter().any(|c| c.notes > 0))
    {
        return Estimate {
            song: None,
            parts: vec![],
        };
    }
    let song = view.song.is_none().then(|| {
        let energies: Vec<f64> = view.sections.iter().map(|s| s.measured).collect();
        let key = crate::harmony::analyze(project, None, None)
            .key
            .filter(|k| k.confidence >= 0.1)
            .map(|k| k.name);
        SongPlan {
            key,
            arc: arc_of(&energies).map(str::to_owned),
            ..Default::default()
        }
    });
    // 働き: ドラム → ビート、名前(Lead・Arp・Riser・Bass など)→ その働き、音がごく少ない → つなぎ、低い → ベース、
    // 和音が多い → 和音、細かい音が並ぶ → 刻み、ほかの旋律のうち最も前に出ているもの → 主役(名前で主役が決まっていなければ)、残りは合いの手
    struct Cand {
        name: String,
        plan: PartPlan,
        melodic: bool,
        presence: u32,
    }
    let mut cands: Vec<Cand> = Vec::new();
    for part in view.parts.iter().filter(|p| p.plan_id.is_none()) {
        let Some(track) = project.track(&part.track_id) else {
            continue;
        };
        let Some((median, poly, dur, count)) = pitch_stats(track) else {
            continue;
        };
        let (function, melodic) = if is_drum(track) {
            ("beat", false)
        } else if let Some(f) = function_by_name(&track.name) {
            (f, false)
        } else if count <= 4 {
            ("transition", false)
        } else if median < 50 {
            ("bass", false)
        } else if poly >= 2.5 {
            ("harmony", false)
        } else if dur <= crate::PPQ / 4 && count >= 32 {
            ("rhythm", false)
        } else {
            ("lead", true)
        };
        let sections: Vec<PartSectionPlan> = part
            .cells
            .iter()
            .map(|c| PartSectionPlan {
                section: c.section.clone(),
                presence: c.measured,
                register: (c.notes > 0 && function != "beat")
                    .then_some(c.register)
                    .flatten(),
                ..Default::default()
            })
            .collect();
        cands.push(Cand {
            name: part.name.clone(),
            presence: part.cells.iter().map(|c| c.measured as u32).sum(),
            plan: PartPlan {
                track: Some(part.track_id.to_string()),
                function: Some(function.to_owned()),
                note: None,
                sections,
            },
            melodic,
        });
    }
    // 主役は 1 つ(採用済みの主役か、名前で主役と分かるパートがいれば、ほかの旋律は合いの手にする)
    let has_lead = plans
        .plans
        .values()
        .filter(|p| p.kind == "part" && p.state.is_none())
        .filter_map(|p| serde_json::from_value::<PartPlan>(p.body.clone()).ok())
        .any(|pp| pp.function.as_deref() == Some("lead"))
        || cands
            .iter()
            .any(|c| !c.melodic && c.plan.function.as_deref() == Some("lead"));
    let lead = (!has_lead)
        .then(|| {
            cands
                .iter()
                .enumerate()
                .filter(|(_, c)| c.melodic)
                .max_by_key(|(_, c)| c.presence)
                .map(|(i, _)| i)
        })
        .flatten();
    let parts = cands
        .into_iter()
        .enumerate()
        .map(|(i, mut c)| {
            if c.melodic && Some(i) != lead {
                c.plan.function = Some("answer".to_owned());
            }
            (c.name, c.plan)
        })
        .collect();
    Estimate { song, parts }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::{ClipId, NoteId, SectionId, TrackId};
    use crate::model::{Clip, Note, SectionMarker, TrackKind};
    use crate::time::Tick;

    fn note(pos: u64, pitch: u8, dur: u64) -> Note {
        Note {
            id: NoteId::new(),
            pos: Tick(pos),
            dur: Tick(dur),
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

    fn track(name: &str, notes: Vec<Note>, start: u64, len: u64) -> Track {
        let mut t = Track::new(TrackId::new(), name, TrackKind::Midi);
        let mut c = Clip::new_midi(ClipId::new(), "c", Tick(start), Tick(len));
        *c.notes_mut().unwrap() = notes;
        t.clips.push(c);
        t
    }

    /// 2 区間(各 2 小節)。ベースは後半だけ、和音とリード 2 本は両方
    fn song() -> Project {
        let mut p = Project::new("t");
        for (i, name) in ["Verse", "Chorus"].iter().enumerate() {
            p.sections.push(SectionMarker {
                id: Some(SectionId::new()),
                tick: Tick(7680 * i as u64),
                name: (*name).to_owned(),
                energy: None,
                tracks: vec![],
                note: None,
                curve: vec![],
                join: None,
            });
        }
        let bar = 3840;
        p.tracks.push(track(
            "Bass",
            (0..8)
                .map(|i| note(i * 960, 36 + (i % 2) as u8 * 7, 900))
                .collect(),
            7680,
            7680,
        ));
        let mut pad = vec![];
        for b in 0..4 {
            for pc in [57u8, 60, 64] {
                pad.push(note(b * bar, pc, bar));
            }
        }
        p.tracks.push(track("Pad", pad, 0, 15360));
        p.tracks.push(track(
            "Lead",
            (0..16)
                .map(|i| note(i * 960, 69 + (i % 5) as u8, 900))
                .collect(),
            0,
            15360,
        ));
        let mut quiet = track(
            "Answer",
            (0..8).map(|i| note(i * 1920, 76, 480)).collect(),
            0,
            15360,
        );
        quiet.volume_db = -12.0;
        p.tracks.push(quiet);
        p
    }

    #[test]
    fn parts_without_a_plan_get_an_estimated_role_presence_and_register() {
        let p = song();
        let e = estimate(&p, &PlanSet::default());
        let by: std::collections::BTreeMap<&str, &PartPlan> =
            e.parts.iter().map(|(n, pp)| (n.as_str(), pp)).collect();
        assert_eq!(by["Bass"].function.as_deref(), Some("bass"));
        assert_eq!(by["Pad"].function.as_deref(), Some("harmony"));
        // 旋律のうち前に出ている方が主役、もう一方は応答
        assert_eq!(by["Lead"].function.as_deref(), Some("lead"));
        assert_eq!(by["Answer"].function.as_deref(), Some("answer"));
        // ベースは前半では鳴っていない(存在 0・帯なし)、後半は鳴っていて実際の音域が帯
        let bass = &by["Bass"].sections;
        assert_eq!((bass[0].presence, bass[0].register), (0, None));
        assert!(bass[1].presence > 0);
        assert_eq!(bass[1].register, Some([36, 43]));
        // 曲全体: キーと盛り上がりの型
        let song = e.song.unwrap();
        assert!(song.key.is_some());
        assert!(song.arc.is_some());
    }

    #[test]
    fn songs_without_section_ids_or_notes_are_not_estimated() {
        let mut p = song();
        p.sections[0].id = None;
        let e = estimate(&p, &PlanSet::default());
        assert!(e.song.is_none() && e.parts.is_empty());
        // 区間はあるが音が無い(作り始めたばかり)
        let mut p = song();
        p.tracks.clear();
        let e = estimate(&p, &PlanSet::default());
        assert!(e.song.is_none() && e.parts.is_empty());
    }

    #[test]
    fn track_names_and_note_shapes_guide_the_role() {
        // 名前: Arp(音が多く前に出ていても刻み)と Lead(主役)、Riser(つなぎ)
        let mut p = song();
        p.tracks[3].name = "Lead Synth".to_owned();
        p.tracks[2].name = "Arp".to_owned();
        let e = estimate(&p, &PlanSet::default());
        let by: std::collections::BTreeMap<&str, Option<&str>> = e
            .parts
            .iter()
            .map(|(n, pp)| (n.as_str(), pp.function.as_deref()))
            .collect();
        assert_eq!(by["Arp"], Some("rhythm"));
        assert_eq!(by["Lead Synth"], Some("lead"));
        assert_eq!(function_by_name("Riser FX"), Some("transition"));
        assert_eq!(function_by_name("ベース"), Some("bass"));
        assert_eq!(function_by_name("Sub Bass"), Some("sub"));
        assert_eq!(function_by_name("Strings"), None);
        // 名前が無くても、音がごく少ないものはつなぎ
        let mut p = song();
        p.tracks[3].clips[0].notes_mut().unwrap().truncate(2);
        let e = estimate(&p, &PlanSet::default());
        let ans = e.parts.iter().find(|(n, _)| n == "Answer").unwrap();
        assert_eq!(ans.1.function.as_deref(), Some("transition"));
    }

    #[test]
    fn arcs_from_measured_energy() {
        assert_eq!(arc_of(&[3.0, 5.0, 8.0]), Some("rise"));
        assert_eq!(arc_of(&[8.0, 5.0, 2.0]), Some("sink"));
        assert_eq!(arc_of(&[3.0, 8.0, 4.0]), Some("peak"));
        assert_eq!(arc_of(&[3.0, 8.0, 4.0, 9.0, 2.0]), Some("waves"));
        assert_eq!(arc_of(&[5.0, 5.5, 5.2]), Some("flat"));
        assert_eq!(arc_of(&[5.0]), None);
    }
}
