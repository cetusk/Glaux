//! ノート編集の計算量と ID の検査。
//! 以前は add / remove / update_notes が入れ子の線形探索で、1 万ノートの一括編集に
//! 0.2〜0.9 秒かかり、その間セッションが止まっていた。

use glaux_core::*;
use std::time::Instant;

fn note(i: u64) -> Note {
    Note {
        // 連番(ランダムな 6 桁だと、2 万個で約 9% の確率でどこかが重複してテストが揺れる)
        id: NoteId::parse(&format!("nt_{i:06}")).unwrap(),
        pos: Tick(i * 120),
        dur: Tick(120),
        pitch: 36 + (i % 60) as u8,
        vel: 100,
        articulation: Articulation::Normal,
        pitch_curve: vec![],
        glide_ms: None,
    }
}

fn project_with_clip(len: u64) -> (Project, ClipId) {
    let mut p = Project::new("notes");
    let tid = TrackId::new();
    p.apply(&Command::AddTrack {
        track: Track::new(tid.clone(), "t", TrackKind::Midi),
        index: None,
    })
    .unwrap();
    let cid = ClipId::new();
    p.apply(&Command::AddClip {
        track: tid,
        clip: Clip::new_midi(cid.clone(), "c", Tick(0), Tick(len)),
    })
    .unwrap();
    (p, cid)
}

#[test]
fn bulk_note_edits_are_fast() {
    const N: u64 = 20_000;
    let (mut p, cid) = project_with_clip(N * 120);
    let notes: Vec<Note> = (0..N).map(note).collect();
    let ids: Vec<NoteId> = notes.iter().map(|n| n.id.clone()).collect();

    let t = Instant::now();
    p.apply(&Command::AddNotes {
        clip: cid.clone(),
        notes,
    })
    .unwrap();
    let changes: Vec<NoteChange> = ids
        .iter()
        .map(|id| {
            let mut c = NoteChange::new(id.clone());
            c.vel = Some(80);
            c
        })
        .collect();
    let applied = p
        .apply(&Command::UpdateNotes {
            clip: cid.clone(),
            changes,
        })
        .unwrap();
    p.apply(&applied.inverse).unwrap();
    p.apply(&Command::RemoveNotes {
        clip: cid.clone(),
        ids,
    })
    .unwrap();
    let elapsed = t.elapsed();
    // 線形探索の入れ子だと 2 万ノートで数秒かかる。ハッシュ引きなら数十 ms
    assert!(elapsed.as_secs_f64() < 1.0, "{elapsed:?}");
}

#[test]
fn duplicate_ids_within_one_add_are_rejected() {
    let (mut p, cid) = project_with_clip(3840);
    let a = note(0);
    let mut b = note(1);
    b.id = a.id.clone();
    let before = p.clone();
    let err = p
        .apply(&Command::AddNotes {
            clip: cid,
            notes: vec![a, b],
        })
        .unwrap_err();
    assert!(matches!(err, CoreError::DuplicateId(_)), "{err}");
    assert_eq!(p, before);
}
