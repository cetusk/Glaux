//! 位置・長さの上限(MAX_TICK)の検査。
//! 以前は巨大な開始位置のクリップを置いて分割すると、Tick の加算が桁あふれで panic し、
//! セッションのアクターが止まっていた。

use glaux_core::*;

fn project_with_track() -> (Project, TrackId) {
    let mut p = Project::new("limits");
    let tid = TrackId::new();
    p.apply(&Command::AddTrack {
        track: Track::new(tid.clone(), "t", TrackKind::Midi),
        index: None,
    })
    .unwrap();
    (p, tid)
}

fn note(pos: u64, dur: u64) -> Note {
    Note {
        id: NoteId::new(),
        pos: Tick(pos),
        dur: Tick(dur),
        pitch: 60,
        vel: 100,
        articulation: Articulation::Normal,
        pitch_curve: vec![],
        glide_ms: None,
    }
}

#[test]
fn huge_positions_are_rejected_without_changing_the_project() {
    let (mut p, tid) = project_with_track();
    let before = p.clone();

    // 報告された再現手順の値
    let clip = Clip::new_midi(ClipId::new(), "c", Tick(18446744073709550000), Tick(3840));
    let err = p
        .apply(&Command::AddClip {
            track: tid.clone(),
            clip,
        })
        .unwrap_err();
    assert!(matches!(err, CoreError::OutOfRange(_)), "{err}");
    assert_eq!(p, before);

    // 開始と長さはそれぞれ上限内でも、終わりが上限を超えるものは拒否
    let clip = Clip::new_midi(ClipId::new(), "c", MAX_TICK, Tick(1));
    assert!(p
        .apply(&Command::AddClip {
            track: tid.clone(),
            clip
        })
        .is_err());

    // Batch の中にあっても拒否され、途中まで適用した分は巻き戻る
    let ok = Clip::new_midi(ClipId::new(), "ok", Tick(0), Tick(3840));
    let bad = Clip::new_midi(ClipId::new(), "bad", Tick(u64::MAX - 10), Tick(3840));
    let batch = Command::batch(
        "x",
        vec![
            Command::AddClip {
                track: tid.clone(),
                clip: ok,
            },
            Command::AddClip {
                track: tid.clone(),
                clip: bad,
            },
        ],
    );
    assert!(p.apply(&batch).is_err());
    assert_eq!(p, before);
}

#[test]
fn huge_note_and_edit_values_are_rejected() {
    let (mut p, tid) = project_with_track();
    let cid = ClipId::new();
    p.apply(&Command::AddClip {
        track: tid,
        clip: Clip::new_midi(cid.clone(), "c", Tick(0), Tick(3840)),
    })
    .unwrap();

    for cmd in [
        Command::AddNotes {
            clip: cid.clone(),
            notes: vec![note(u64::MAX - 5, 480)],
        },
        Command::MoveClip {
            id: cid.clone(),
            start: Tick(u64::MAX),
            track: None,
        },
        Command::ResizeClip {
            id: cid.clone(),
            length: Tick(u64::MAX),
        },
        Command::SplitClip {
            id: cid.clone(),
            at: Tick(u64::MAX),
            new_id: ClipId::new(),
        },
    ] {
        assert!(
            matches!(p.apply(&cmd), Err(CoreError::OutOfRange(_))),
            "{cmd:?}"
        );
    }

    // 上限ちょうどまでの普通の編集は通る
    p.apply(&Command::MoveClip {
        id: cid,
        start: Tick(MAX_TICK.0 - 3840),
        track: None,
    })
    .unwrap();
}

#[test]
fn tick_arithmetic_saturates_instead_of_panicking() {
    assert_eq!(Tick(u64::MAX) + Tick(10), Tick(u64::MAX));
    assert_eq!(Tick(5) - Tick(10), Tick::ZERO);
}
