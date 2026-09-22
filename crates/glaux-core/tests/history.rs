//! 核となる性質のテスト:
//! 1. 可逆性: apply(cmd) → apply(inverse) で元に戻る(ランダムなコマンド列で検証)
//! 2. リプレイ: 空プロジェクトに履歴を順に適用すると現在の Project と一致する
//! 3. 履歴操作: undo/redo, checkpoint/revert_to, revert(途中エントリ)+衝突検出

use glaux_core::*;
use rand::rngs::StdRng;
use rand::{
    seq::{IteratorRandom, SliceRandom},
    Rng, SeedableRng,
};

fn seed_project() -> Project {
    let mut p = Project::new("seed");
    let asset_id = AssetId::from_sha256_hex("deadbeef").unwrap();
    p.apply(&Command::AddAsset {
        id: asset_id.clone(),
        asset: Asset {
            path: "audio/a.wav".into(),
            sample_rate: 48000,
            channels: 2,
            frames: 48000 * 10,
        },
    })
    .unwrap();

    for i in 0..3 {
        let tid = TrackId::new();
        let mut t = Track::new(tid.clone(), format!("Synth {i}"), TrackKind::Midi);
        let mut dev = Device::builtin("subtractive");
        dev.params.insert("filter.cutoff".into(), 800.0.into());
        t.device = Some(dev);
        t.effects.push(Effect::builtin(FxId::new(), "compressor"));
        p.apply(&Command::AddTrack {
            track: t,
            index: None,
        })
        .unwrap();
        for c in 0..2 {
            let mut clip =
                Clip::new_midi(ClipId::new(), format!("c{c}"), Tick(c * 3840), Tick(3840));
            let notes = clip.notes_mut().unwrap();
            for n in 0..8u64 {
                notes.push(Note {
                    id: NoteId::new(),
                    pos: Tick(n * 480),
                    dur: Tick(480),
                    pitch: 48 + n as u8,
                    vel: 100,
                });
            }
            p.apply(&Command::AddClip {
                track: tid.clone(),
                clip,
            })
            .unwrap();
        }
    }
    let tid = TrackId::new();
    p.apply(&Command::AddTrack {
        track: Track::new(tid.clone(), "Audio", TrackKind::Audio),
        index: None,
    })
    .unwrap();
    p.apply(&Command::AddClip {
        track: tid,
        clip: Clip::new_audio(ClipId::new(), "take", Tick(0), Tick(7680), asset_id),
    })
    .unwrap();
    p
}

/// 現在のプロジェクト状態に対して(ほぼ)有効なランダムコマンドを作る。
fn random_command(p: &Project, rng: &mut StdRng, depth: u8) -> Command {
    let tracks: Vec<&Track> = p.tracks.iter().collect();
    let midi_tracks: Vec<&Track> = tracks
        .iter()
        .copied()
        .filter(|t| t.kind == TrackKind::Midi)
        .collect();
    let all_clips: Vec<(&Track, &Clip)> = tracks
        .iter()
        .flat_map(|t| t.clips.iter().map(move |c| (*t, c)))
        .collect();
    let midi_clips: Vec<&Clip> = all_clips
        .iter()
        .map(|(_, c)| *c)
        .filter(|c| c.is_midi())
        .collect();
    let effects: Vec<&Effect> = tracks.iter().flat_map(|t| t.effects.iter()).collect();

    let pick_track = |rng: &mut StdRng| tracks.choose(rng).unwrap().id.clone();

    loop {
        let choice = if depth == 0 {
            rng.gen_range(0..17)
        } else {
            rng.gen_range(0..18)
        };
        match choice {
            0 => {
                return Command::SetTrackProp {
                    id: pick_track(rng),
                    prop: match rng.gen_range(0..4) {
                        0 => TrackProp::Mute(rng.gen()),
                        1 => TrackProp::VolumeDb(rng.gen_range(-30.0..6.0)),
                        2 => TrackProp::Name(format!("name{}", rng.gen_range(0..100))),
                        _ => TrackProp::Pan(rng.gen_range(-1.0..1.0)),
                    },
                }
            }
            1 => {
                return Command::MoveTrack {
                    id: pick_track(rng),
                    to_index: rng.gen_range(0..tracks.len()),
                }
            }
            2 => {
                let Some(t) = midi_tracks.choose(rng) else {
                    continue;
                };
                return Command::AddClip {
                    track: t.id.clone(),
                    clip: Clip::new_midi(
                        ClipId::new(),
                        "new",
                        Tick(rng.gen_range(0..20) * 960),
                        Tick(rng.gen_range(1..8) * 960),
                    ),
                };
            }
            3 => {
                if midi_clips.len() < 3 {
                    continue;
                }
                return Command::RemoveClip {
                    id: midi_clips.choose(rng).unwrap().id.clone(),
                };
            }
            4 => {
                let Some((_, c)) = all_clips.choose(rng) else {
                    continue;
                };
                let dest = tracks
                    .iter()
                    .filter(|t| {
                        t.kind
                            == (if c.is_midi() {
                                TrackKind::Midi
                            } else {
                                TrackKind::Audio
                            })
                    })
                    .choose(rng);
                return Command::MoveClip {
                    id: c.id.clone(),
                    start: Tick(rng.gen_range(0..20) * 480),
                    track: dest.map(|t| t.id.clone()),
                };
            }
            5 => {
                let Some((_, c)) = all_clips.choose(rng) else {
                    continue;
                };
                return Command::ResizeClip {
                    id: c.id.clone(),
                    length: Tick(rng.gen_range(1..10) * 960),
                };
            }
            6 => {
                let Some((_, c)) = all_clips
                    .iter()
                    .filter(|(_, c)| c.length.0 >= 2)
                    .choose(rng)
                else {
                    continue;
                };
                let at = Tick(c.start.0 + rng.gen_range(1..c.length.0));
                return Command::SplitClip {
                    id: c.id.clone(),
                    at,
                    new_id: ClipId::new(),
                };
            }
            7 => {
                let Some(c) = midi_clips.choose(rng) else {
                    continue;
                };
                let notes = (0..rng.gen_range(1..4))
                    .map(|_| Note {
                        id: NoteId::new(),
                        pos: Tick(rng.gen_range(0..8) * 480),
                        dur: Tick(240),
                        pitch: rng.gen_range(36..84),
                        vel: rng.gen_range(1..=127),
                    })
                    .collect();
                return Command::AddNotes {
                    clip: c.id.clone(),
                    notes,
                };
            }
            8 => {
                let Some(c) = midi_clips
                    .iter()
                    .filter(|c| !c.notes().unwrap().is_empty())
                    .choose(rng)
                else {
                    continue;
                };
                let k = rng.gen_range(1..=2);
                let ids = c
                    .notes()
                    .unwrap()
                    .choose_multiple(rng, k)
                    .map(|n| n.id.clone())
                    .collect();
                return Command::RemoveNotes {
                    clip: c.id.clone(),
                    ids,
                };
            }
            9 => {
                let Some(c) = midi_clips
                    .iter()
                    .filter(|c| !c.notes().unwrap().is_empty())
                    .choose(rng)
                else {
                    continue;
                };
                let k = rng.gen_range(1..=3);
                let picked: Vec<&Note> = c.notes().unwrap().choose_multiple(rng, k).collect();
                let changes = picked
                    .into_iter()
                    .map(|n| {
                        let mut ch = NoteChange::new(n.id.clone());
                        if rng.gen() {
                            ch = ch.pos(Tick(rng.gen_range(0..8) * 480));
                        }
                        if rng.gen() {
                            ch = ch.pitch(rng.gen_range(36..84));
                        }
                        if rng.gen() {
                            ch = ch.vel(rng.gen_range(1..=127));
                        }
                        if rng.gen() {
                            ch = ch.dur(Tick(rng.gen_range(1..4) * 240));
                        }
                        ch
                    })
                    .collect();
                return Command::UpdateNotes {
                    clip: c.id.clone(),
                    changes,
                };
            }
            10 => {
                let Some(t) = midi_tracks.choose(rng) else {
                    continue;
                };
                let name = ["filter.cutoff", "filter.resonance", "osc1.wave"]
                    .choose(rng)
                    .unwrap();
                let value: ParamValue = if *name == "osc1.wave" {
                    "square".into()
                } else {
                    rng.gen_range(0.0..1000.0).into()
                };
                return Command::SetParam {
                    track: t.id.clone(),
                    path: ParamPath::device(*name),
                    value,
                };
            }
            11 => {
                let Some(t) = midi_tracks
                    .iter()
                    .filter(|t| t.device.as_ref().is_some_and(|d| !d.params.is_empty()))
                    .choose(rng)
                else {
                    continue;
                };
                let name = t
                    .device
                    .as_ref()
                    .unwrap()
                    .params
                    .keys()
                    .choose(rng)
                    .unwrap()
                    .clone();
                return Command::UnsetParam {
                    track: t.id.clone(),
                    path: ParamPath::device(name),
                };
            }
            12 => {
                let Some(e) = effects.choose(rng) else {
                    continue;
                };
                return Command::SetEffectBypass {
                    id: e.id.clone(),
                    bypass: rng.gen(),
                };
            }
            13 => {
                let t = pick_track(rng);
                return Command::AddEffect {
                    track: t,
                    effect: Effect::builtin(FxId::new(), "eq"),
                    index: None,
                };
            }
            14 => {
                if effects.len() < 2 {
                    continue;
                }
                return Command::RemoveEffect {
                    id: effects.choose(rng).unwrap().id.clone(),
                };
            }
            15 => {
                let t = pick_track(rng);
                let points = (0..rng.gen_range(0..4))
                    .map(|i| AutomationPoint {
                        tick: Tick(i * 960),
                        value: rng.gen_range(0.0..1.0),
                        curve: Curve::Linear,
                    })
                    .collect();
                return Command::SetAutomationPoints {
                    track: t,
                    target: ParamPath::track("volume_db"),
                    points,
                };
            }
            16 => {
                return Command::SetMasterVolume {
                    volume_db: rng.gen_range(-12.0..0.0),
                }
            }
            _ => {
                // Batch: 2〜4 個を順に生成(前のコマンドの結果に依存する可能性があるので仮適用しながら作る)
                let mut scratch = p.clone();
                let mut cmds = Vec::new();
                for _ in 0..rng.gen_range(2..=4) {
                    let c = random_command(&scratch, rng, depth + 1);
                    if scratch.apply(&c).is_ok() {
                        cmds.push(c);
                    }
                }
                if cmds.is_empty() {
                    continue;
                }
                return Command::batch("batch", cmds);
            }
        }
    }
}

#[test]
fn every_command_is_invertible() {
    let mut rng = StdRng::seed_from_u64(42);
    let mut p = seed_project();
    let mut applied = 0;
    for _ in 0..1500 {
        let cmd = random_command(&p, &mut rng, 0);
        let before = p.clone();
        let mut trial = p.clone();
        match trial.apply(&cmd) {
            Ok(a) => {
                trial
                    .apply(&a.inverse)
                    .unwrap_or_else(|e| panic!("inverse failed: {e}\ncmd={cmd:?}"));
                assert_eq!(trial, before, "inverse did not restore state for {cmd:?}");
                p.apply(&cmd).unwrap();
                applied += 1;
            }
            Err(_) => {
                // 失敗時はプロジェクトが変わっていないこと
                assert_eq!(trial, before, "failed apply mutated project: {cmd:?}");
            }
        }
        assert!(p.is_valid(), "{:?}", p.validate());
    }
    assert!(applied > 1000, "too few commands applied: {applied}");
}

#[test]
fn batch_rolls_back_on_failure() {
    let mut p = seed_project();
    let before = p.clone();
    let tid = p.tracks[0].id.clone();
    let bad = Command::batch(
        "partial",
        vec![
            Command::SetTrackProp {
                id: tid.clone(),
                prop: TrackProp::Mute(true),
            },
            Command::SetMasterVolume { volume_db: -3.0 },
            Command::RemoveClip { id: ClipId::new() }, // 存在しない → 失敗
        ],
    );
    let err = p.apply(&bad).unwrap_err();
    assert!(matches!(err, CoreError::Batch { index: 2, .. }));
    assert_eq!(p, before);
}

#[test]
fn replay_reconstructs_project() {
    let mut rng = StdRng::seed_from_u64(7);
    let base = seed_project();
    let mut s = Session::new(base.clone());
    for i in 0..300 {
        let cmd = random_command(s.project(), &mut rng, 0);
        let author = if i % 3 == 0 {
            Author::Ai {
                model: "test".into(),
            }
        } else {
            Author::Human
        };
        let _ = s.apply(cmd, author, format!("step {i}"));
    }
    let jsonl = s.history().to_jsonl().unwrap();
    let entries = History::entries_from_jsonl(&jsonl).unwrap();
    assert_eq!(entries.len(), s.history().len());

    let rebuilt = Session::replay(base, entries).unwrap();
    assert_eq!(rebuilt.project(), s.project());

    // project.json の往復もついでに
    let json = s.project().to_json().unwrap();
    assert_eq!(&Project::from_json(&json).unwrap(), s.project());

    let ai_edits = s
        .history()
        .by_author(|a| matches!(a, Author::Ai { .. }))
        .count();
    assert!(ai_edits > 0);
}

#[test]
fn undo_redo_and_checkpoints() {
    let mut s = Session::new(seed_project());
    let start = s.project().clone();
    let tid = s.project().tracks[0].id.clone();

    s.checkpoint("before");
    s.apply(
        Command::SetTrackProp {
            id: tid.clone(),
            prop: TrackProp::Mute(true),
        },
        Author::Human,
        "mute",
    )
    .unwrap();
    s.apply(
        Command::SetMasterVolume { volume_db: -6.0 },
        Author::Human,
        "master",
    )
    .unwrap();
    assert!(s.project().track(&tid).unwrap().mute);

    s.undo().unwrap();
    assert_eq!(s.project().master.volume_db, 0.0);
    s.redo().unwrap();
    assert_eq!(s.project().master.volume_db, -6.0);

    s.revert_to("before").unwrap();
    assert_eq!(s.project(), &start);
    assert!(s.can_redo());

    // 新しい操作で redo は捨てられる
    s.apply(
        Command::SetMasterVolume { volume_db: -1.0 },
        Author::Human,
        "x",
    )
    .unwrap();
    assert!(!s.can_redo());
}

#[test]
fn revert_middle_entry_detects_conflicts() {
    let mut s = Session::new(seed_project());
    let ai = Author::Ai {
        model: "test".into(),
    };
    let t0 = s.project().tracks[0].id.clone();
    let t1 = s.project().tracks[1].id.clone();

    // ① t0 にクリップ追加(AI)
    let clip_id = ClipId::new();
    let (e1, _) = s
        .apply(
            Command::AddClip {
                track: t0.clone(),
                clip: Clip::new_midi(clip_id.clone(), "ai", Tick(0), Tick(960)),
            },
            ai.clone(),
            "add clip",
        )
        .unwrap();
    // ② t1 をミュート(無関係)
    let (_e2, _) = s
        .apply(
            Command::SetTrackProp {
                id: t1.clone(),
                prop: TrackProp::Mute(true),
            },
            Author::Human,
            "mute",
        )
        .unwrap();
    // ③ ①のクリップを伸ばす(依存あり)
    let (e3, _) = s
        .apply(
            Command::ResizeClip {
                id: clip_id.clone(),
                length: Tick(1920),
            },
            ai.clone(),
            "resize",
        )
        .unwrap();

    // ①を revert → ③が衝突候補として報告される
    let r = s.revert(&e1, Author::Human).unwrap();
    assert_eq!(r.conflicts, vec![e3.clone()]);
    assert!(s.project().clip(&clip_id).is_none());
    assert!(
        s.project().track(&t1).unwrap().mute,
        "unrelated edit survives"
    );
    assert_eq!(
        s.history().entry(&r.entry).unwrap().reverts,
        Some(e1.clone())
    );

    // 履歴は 4 エントリ(revert も残る)。undo すれば revert を取り消せる。
    assert_eq!(s.history().len(), 4);
    s.undo().unwrap();
    assert!(s.project().clip(&clip_id).is_some());

    // AI の作業一覧
    let ai_entries: Vec<_> = s
        .history()
        .by_author(|a| matches!(a, Author::Ai { .. }))
        .map(|e| e.label.clone())
        .collect();
    assert_eq!(ai_entries, vec!["add clip", "resize"]);
}

#[test]
fn split_midi_clip_moves_and_truncates_notes() {
    let mut p = Project::new("split");
    let tid = TrackId::new();
    p.apply(&Command::AddTrack {
        track: Track::new(tid.clone(), "t", TrackKind::Midi),
        index: None,
    })
    .unwrap();
    let cid = ClipId::new();
    let mut clip = Clip::new_midi(cid.clone(), "c", Tick(960), Tick(3840));
    let ids: Vec<NoteId> = (0..3).map(|_| NoteId::new()).collect();
    clip.notes_mut().unwrap().extend([
        Note {
            id: ids[0].clone(),
            pos: Tick(0),
            dur: Tick(480),
            pitch: 60,
            vel: 100,
        }, // 左に残る
        Note {
            id: ids[1].clone(),
            pos: Tick(1680),
            dur: Tick(480),
            pitch: 62,
            vel: 100,
        }, // 分割点(1920)をまたぐ → 切り詰め
        Note {
            id: ids[2].clone(),
            pos: Tick(2880),
            dur: Tick(480),
            pitch: 64,
            vel: 100,
        }, // 右へ移動
    ]);
    p.apply(&Command::AddClip { track: tid, clip }).unwrap();

    let new_id = ClipId::new();
    let a = p
        .apply(&Command::SplitClip {
            id: cid.clone(),
            at: Tick(960 + 1920),
            new_id: new_id.clone(),
        })
        .unwrap();

    let (_, left) = p.clip(&cid).unwrap();
    let (_, right) = p.clip(&new_id).unwrap();
    assert_eq!(left.length, Tick(1920));
    assert_eq!(right.start, Tick(2880));
    assert_eq!(right.length, Tick(1920));
    let ln = left.notes().unwrap();
    assert_eq!(ln.len(), 2);
    assert_eq!(ln[1].dur, Tick(240), "straddling note truncated");
    let rn = right.notes().unwrap();
    assert_eq!(rn.len(), 1);
    assert_eq!(rn[0].pos, Tick(960), "moved note is relative to new clip");

    let snapshot_after_split = p.clone();
    p.apply(&a.inverse).unwrap();
    assert_eq!(p.clip(&cid).unwrap().1.notes().unwrap().len(), 3);
    assert!(p.clip(&new_id).is_none());
    assert_ne!(p, snapshot_after_split);
}
