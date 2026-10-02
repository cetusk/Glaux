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
                    articulation: Articulation::Normal,
                    pitch_curve: vec![],
                    glide_ms: None,
                    vibrato: None,
                    volume_curve: vec![],
                    brightness_curve: vec![],
                    condition: None,
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
    // センドの送り先になるバス
    p.apply(&Command::AddTrack {
        track: Track::new(TrackId::new(), "Reverb Bus", TrackKind::Bus),
        index: None,
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
        // 0..=29 は単体コマンド、30 以上は Batch(入れ子は 1 段まで)
        let choice = if depth == 0 {
            rng.gen_range(0..31)
        } else {
            rng.gen_range(0..30)
        };
        match choice {
            0 => {
                // センド(バス以外 → バス。外す・量を変える)
                let buses: Vec<&&Track> =
                    tracks.iter().filter(|t| t.kind == TrackKind::Bus).collect();
                if rng.gen_bool(0.4) && !buses.is_empty() {
                    let src = *midi_tracks.choose(rng).unwrap();
                    let bus = buses.choose(rng).unwrap();
                    return Command::SetSend {
                        track: src.id.clone(),
                        target: bus.id.clone(),
                        level_db: rng.gen_bool(0.8).then(|| rng.gen_range(-40.0..6.0)),
                        pre_fader: rng.gen(),
                    };
                }
                // 出力先(グループ)。輪になる組み合わせは失敗する(可逆性は成功したものだけ見る)
                if rng.gen_bool(0.15) && !buses.is_empty() {
                    let src = *midi_tracks.choose(rng).unwrap();
                    let bus = buses.choose(rng).unwrap();
                    return Command::SetTrackProp {
                        id: src.id.clone(),
                        prop: TrackProp::Output(rng.gen_bool(0.7).then(|| bus.id.clone())),
                    };
                }
                return Command::SetTrackProp {
                    id: pick_track(rng),
                    prop: match rng.gen_range(0..7) {
                        0 => TrackProp::Mute(rng.gen()),
                        1 => TrackProp::VolumeDb(rng.gen_range(-30.0..6.0)),
                        2 => TrackProp::Name(format!("name{}", rng.gen_range(0..100))),
                        3 => TrackProp::Modulators(
                            (0..rng.gen_range(0..3))
                                .map(|i| Modulator {
                                    target: ParamPath::device(if i == 0 {
                                        "cutoff"
                                    } else {
                                        "resonance"
                                    }),
                                    shape: *[LfoShape::Sine, LfoShape::SawDown, LfoShape::Random]
                                        .choose(rng)
                                        .unwrap(),
                                    sync: rng.gen_bool(0.5).then(|| "1/8".to_owned()),
                                    rate_hz: 2.0,
                                    depth: rng.gen_range(0.0..500.0),
                                    phase: 0.0,
                                    range: None,
                                    center: None,
                                })
                                .collect(),
                        ),
                        4 => TrackProp::Layers(
                            (0..rng.gen_range(0..3))
                                .map(|i| {
                                    let mut l =
                                        glaux_core::Layer::new(glaux_core::Device::builtin(
                                            if i == 0 { "fm" } else { "subtractive" },
                                        ));
                                    l.transpose = rng.gen_range(-12..=12);
                                    l.volume_db = rng.gen_range(-12.0..0.0);
                                    l.key_hi = rng.gen_range(60..=127);
                                    l
                                })
                                .collect(),
                        ),
                        5 => TrackProp::Macros(
                            (0..rng.gen_range(0..3))
                                .map(|i| glaux_core::Macro {
                                    name: format!("m{i}"),
                                    value: rng.gen_range(0.0..1.0),
                                    targets: vec![glaux_core::MacroTarget {
                                        target: ParamPath::device("cutoff"),
                                        min: 200.0,
                                        max: rng.gen_range(500.0..5000.0),
                                        curve: rng.gen_range(-1.0..1.0),
                                    }],
                                })
                                .collect(),
                        ),
                        _ => TrackProp::Pan(rng.gen_range(-1.0..1.0)),
                    },
                };
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
                        articulation: *[
                            Articulation::Normal,
                            Articulation::PalmMute,
                            Articulation::Staccato,
                            Articulation::Accent,
                            Articulation::Vibrato,
                            Articulation::Bend,
                            Articulation::Legato,
                            Articulation::Portamento,
                        ]
                        .choose(rng)
                        .unwrap(),
                        pitch_curve: vec![],
                        glide_ms: None,
                        vibrato: None,
                        volume_curve: vec![],
                        brightness_curve: vec![],
                        condition: None,
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
                        if rng.gen() {
                            ch = ch.articulation(
                                *[
                                    Articulation::Normal,
                                    Articulation::PalmMute,
                                    Articulation::Staccato,
                                    Articulation::Accent,
                                    Articulation::Vibrato,
                                    Articulation::Bend,
                                    Articulation::Legato,
                                    Articulation::Portamento,
                                ]
                                .choose(rng)
                                .unwrap(),
                            );
                        }
                        if rng.gen() {
                            let n = rng.gen_range(0..4);
                            let curve = (0..n)
                                .map(|i| PitchPoint {
                                    tick: Tick(i * 120),
                                    cents: rng.gen_range(-200.0..200.0),
                                    shape: *[
                                        CurveShape::Linear,
                                        CurveShape::EaseOut,
                                        CurveShape::Hold,
                                    ]
                                    .choose(rng)
                                    .unwrap(),
                                })
                                .collect();
                            ch = ch.pitch_curve(curve);
                        }
                        if rng.gen_bool(0.3) {
                            // 0 は個別指定の解除
                            ch = ch.glide_ms(*[0.0, 40.0, 150.0, 800.0].choose(rng).unwrap());
                        }
                        if rng.gen_bool(0.3) {
                            // 音量・明るさの曲線(空は削除)
                            let n = rng.gen_range(0..4u64);
                            let curve: Vec<CurvePoint> = (0..n)
                                .map(|i| {
                                    CurvePoint::new(
                                        Tick(i * 240),
                                        rng.gen_range(-1.0..1.0),
                                        CurveShape::EaseOut,
                                    )
                                })
                                .collect();
                            ch = if rng.gen() {
                                ch.volume_curve(
                                    curve
                                        .iter()
                                        .map(|p| CurvePoint {
                                            value: p.value * 12.0,
                                            ..*p
                                        })
                                        .collect(),
                                )
                            } else {
                                ch.brightness_curve(curve)
                            };
                        }
                        if rng.gen_bool(0.2) {
                            // いつも鳴る条件は外す
                            ch = ch.condition(NoteCondition {
                                probability: *[1.0, 0.5, 0.25].choose(rng).unwrap(),
                                every: rng.gen_bool(0.5).then_some([1, 2]),
                            });
                        }
                        if rng.gen_bool(0.3) {
                            // 深さ 0 はビブラートの解除
                            ch = ch.vibrato(Vibrato {
                                rate_hz: rng.gen_range(4.0..7.0),
                                depth_cents: *[0.0, 30.0, 60.0].choose(rng).unwrap(),
                                delay_ms: 200.0,
                                fade_in_ms: 150.0,
                                fade_out_ms: if rng.gen() { 0.0 } else { 100.0 },
                                rate_end_hz: rng.gen_bool(0.3).then_some(7.0),
                            });
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
                // トラックのつなぎの設定(レガート / ポルタメント)。未設定との行き来も試す
                if rng.gen_bool(0.25) {
                    let name = *["glide_ms", "legato_ms"].choose(rng).unwrap();
                    if rng.gen_bool(0.3) {
                        return Command::UnsetParam {
                            track: t.id.clone(),
                            path: ParamPath::track(name),
                        };
                    }
                    let v = if name == "glide_ms" {
                        rng.gen_range(10.0..2000.0)
                    } else {
                        rng.gen_range(5.0..200.0)
                    };
                    return Command::SetParam {
                        track: t.id.clone(),
                        path: ParamPath::track(name),
                        value: v.into(),
                    };
                }
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
                // CLAP エフェクトなら状態の差し替えも試す
                if matches!(e.source, PluginSource::Clap { .. }) && rng.gen_bool(0.5) {
                    return Command::SetEffectState {
                        id: e.id.clone(),
                        state: rng
                            .gen_bool(0.7)
                            .then(|| format!("c3RhdGU{}", rng.gen_range(0..100))),
                    };
                }
                return Command::SetEffectBypass {
                    id: e.id.clone(),
                    bypass: rng.gen(),
                };
            }
            13 => {
                let t = pick_track(rng);
                let effect = if rng.gen_bool(0.3) {
                    Effect {
                        id: FxId::new(),
                        source: PluginSource::Clap {
                            plugin_id: "com.example.fx".into(),
                            state: None,
                        },
                        bypass: false,
                        params: Default::default(),
                        ui: Default::default(),
                    }
                } else {
                    Effect::builtin(FxId::new(), "eq")
                };
                return Command::AddEffect {
                    track: t,
                    effect,
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
            17 => {
                if rng.gen_bool(0.5) {
                    // 拍子(変拍子のまとまりも付けたり付けなかったり)
                    let (num, den, grouping) = [
                        (4u8, 4u8, None),
                        (3, 4, None),
                        (7, 8, None),
                        (7, 8, Some(vec![3u8, 2, 2])),
                        (5, 4, Some(vec![2, 3])),
                        (6, 8, None),
                    ]
                    .choose(rng)
                    .unwrap()
                    .clone();
                    let mut events = vec![TimeSigEvent {
                        tick: Tick(0),
                        num,
                        den,
                        grouping: grouping.clone(),
                    }];
                    if rng.gen_bool(0.5) {
                        events.push(TimeSigEvent::new(Tick(rng.gen_range(1..8) * 3840), 4, 4));
                    }
                    return Command::SetTimeSig { events };
                }
                return Command::SetTitle {
                    title: format!("title{}", rng.gen_range(0..100)),
                };
            }
            18 => {
                let sections = (0..rng.gen_range(0..4))
                    .map(|i| SectionMarker {
                        tick: Tick(rng.gen_range(0..8) * 3840),
                        name: format!("sec{i}"),
                        // 計画書の項目(set_song_plan)も付けたり付けなかったり
                        energy: rng.gen_bool(0.5).then(|| rng.gen_range(0..=10) as f32),
                        tracks: if rng.gen_bool(0.5) {
                            vec![format!("t{}", rng.gen_range(0..3))]
                        } else {
                            vec![]
                        },
                        note: rng.gen_bool(0.3).then(|| format!("note{i}")),
                    })
                    .collect();
                return Command::SetSections { sections };
            }
            20 => {
                return Command::AddMasterEffect {
                    effect: Effect::builtin(FxId::new(), "compressor"),
                    index: None,
                };
            }
            21 => {
                let Some(e) = p.master.effects.choose(rng) else {
                    continue;
                };
                return Command::SetMasterParam {
                    path: ParamPath::effect(e.id.clone(), "ratio"),
                    value: ParamValue::Float(rng.gen_range(1.0..8.0)),
                };
            }
            22 => {
                let Some(e) = p.master.effects.choose(rng) else {
                    continue;
                };
                if e.params.is_empty() {
                    continue;
                }
                return Command::UnsetMasterParam {
                    path: ParamPath::effect(e.id.clone(), e.params.keys().next().unwrap().clone()),
                };
            }
            24 => {
                let audio: Vec<&Clip> = all_clips
                    .iter()
                    .map(|(_, c)| *c)
                    .filter(|c| !c.is_midi())
                    .collect();
                let Some(c) = audio.choose(rng) else {
                    continue;
                };
                let stretch = if rng.gen_bool(0.7) {
                    Stretch::Follow {
                        original_bpm: rng.gen_range(60.0..180.0),
                    }
                } else {
                    Stretch::None
                };
                return Command::SetClipStretch {
                    id: c.id.clone(),
                    stretch,
                };
            }
            23 => {
                // マスター音量か、マスターのエフェクトのパラメータのレーン
                let target = match p.master.effects.choose(rng) {
                    Some(e) if rng.gen_bool(0.5) => ParamPath::effect(e.id.clone(), "mix"),
                    _ => ParamPath::track("volume_db"),
                };
                let points = (0..rng.gen_range(0..4))
                    .map(|i| AutomationPoint {
                        tick: Tick(i * 960),
                        value: rng.gen_range(-12.0..0.0),
                        curve: Curve::Linear,
                    })
                    .collect();
                return Command::SetMasterAutomationPoints { target, points };
            }
            25 => {
                // エフェクトの並べ替え(トラックかマスター。同じ列の中で)
                let lists: Vec<&Vec<Effect>> = tracks
                    .iter()
                    .map(|t| &t.effects)
                    .chain(std::iter::once(&p.master.effects))
                    .filter(|l| !l.is_empty())
                    .collect();
                let Some(list) = lists.choose(rng) else {
                    continue;
                };
                return Command::MoveEffect {
                    id: list.choose(rng).unwrap().id.clone(),
                    to_index: rng.gen_range(0..list.len()),
                };
            }
            26 => {
                // エフェクトの表示・置き場所(トラックかマスター)
                let all: Vec<&Effect> = tracks
                    .iter()
                    .flat_map(|t| t.effects.iter())
                    .chain(p.master.effects.iter())
                    .collect();
                let Some(e) = all.choose(rng) else {
                    continue;
                };
                let prop = match rng.gen_range(0..4) {
                    0 => EffectProp::Label(
                        rng.gen_bool(0.7)
                            .then(|| format!("fx{}", rng.gen_range(0..100))),
                    ),
                    1 => EffectProp::Parked(rng.gen()),
                    2 => EffectProp::Note(rng.gen_bool(0.5).then(|| "memo".to_owned())),
                    _ => EffectProp::Pos(
                        rng.gen_bool(0.7)
                            .then(|| [rng.gen_range(0.0..800.0), rng.gen_range(0.0..300.0)]),
                    ),
                };
                return Command::SetEffectProp {
                    id: e.id.clone(),
                    prop,
                };
            }
            27 => {
                // エフェクトのつながり(分岐・合流・つながっていないものを含む。ときどき直列に戻す)
                let target = if rng.gen_bool(0.25) {
                    None
                } else {
                    Some(tracks.choose(rng).unwrap())
                };
                let effects = match target {
                    Some(t) => &t.effects,
                    None => &p.master.effects,
                };
                let links = if rng.gen_bool(0.2) || effects.is_empty() {
                    None
                } else {
                    // 並び順の部分列を直列に並べ(= 輪にならない)、ときどき飛ばす線と音量を足す
                    let mut chain: Vec<FxNode> = vec![FxNode::Input];
                    chain.extend(
                        effects
                            .iter()
                            .filter(|_| rng.gen_bool(0.7))
                            .map(|e| FxNode::Fx(e.id.clone())),
                    );
                    chain.push(FxNode::Output);
                    let mut l: Vec<FxLink> = chain
                        .windows(2)
                        .map(|w| FxLink::new(w[0].clone(), w[1].clone()))
                        .collect();
                    if chain.len() > 3 && rng.gen_bool(0.5) {
                        l.push(FxLink {
                            from: chain[0].clone(),
                            to: chain[chain.len() - 1].clone(),
                            gain_db: rng.gen_range(-12.0..0.0),
                        });
                    }
                    if rng.gen_bool(0.3) {
                        l.pop();
                    }
                    Some(l)
                };
                return Command::SetFxLinks {
                    track: target.map(|t| t.id.clone()),
                    links,
                };
            }
            28 => {
                // ノード表示の入力と出口の置き場所
                let track = rng.gen_bool(0.7).then(|| pick_track(rng));
                let pos = rng.gen_bool(0.8).then(|| FxIoPos {
                    input: [rng.gen_range(0.0..300.0), rng.gen_range(0.0..300.0)],
                    output: [rng.gen_range(300.0..1200.0), rng.gen_range(0.0..300.0)],
                });
                return Command::SetFxIoPos { track, pos };
            }
            29 => {
                // クリップの計画の参照(付ける・外す)
                let Some((_, c)) = all_clips.choose(rng) else {
                    continue;
                };
                let plan = rng.gen_bool(0.7).then(|| glaux_core::plan::PlanRef {
                    id: PlanId::new(),
                    rev: rng.gen_range(1..10),
                    digest: if rng.gen_bool(0.5) {
                        format!("{:016x}", rng.gen::<u64>())
                    } else {
                        String::new()
                    },
                });
                return Command::SetClipPlan {
                    clip: c.id.clone(),
                    plan,
                };
            }
            19 => {
                let Some(c) = midi_clips.choose(rng) else {
                    continue;
                };
                let loop_len = rng.gen_bool(0.7).then(|| Tick(rng.gen_range(1..5) * 960));
                return Command::SetClipLoop {
                    id: c.id.clone(),
                    loop_len,
                };
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
fn compact_keeps_recent_entries_and_replayable_base() {
    let mut rng = StdRng::seed_from_u64(11);
    let seed = seed_project();
    let mut s = Session::new(seed.clone());
    for i in 0..200 {
        let cmd = random_command(s.project(), &mut rng, 0);
        let _ = s.apply(cmd, Author::Human, format!("step {i}"));
    }
    let before = s.project().clone();
    let total = s.history().len();
    assert!(total > 50);
    s.checkpoint("old"); // 切り捨て後は消える
                         // 途中のチェックポイントも作っておく(切り捨て境界より後なら生き残る)
    let keep = 30;

    let (base, dropped) = s.compact(keep).unwrap().expect("compact されるはず");
    assert_eq!(dropped.len(), total - keep);
    assert_eq!(s.history().len(), keep);
    assert_eq!(s.project(), &before, "現在状態は変わらない");
    // 起点 + 残した履歴 = 現在状態
    let entries = s.history().applied().to_vec();
    let rebuilt = Session::replay(base.clone(), entries).unwrap();
    assert_eq!(rebuilt.project(), &before);
    // 捨てた履歴を元の起点に適用すると base になる
    let front = Session::replay(seed, dropped).unwrap();
    assert_eq!(front.project(), &base);
    // 末尾に付けたチェックポイントは index が詰められて残る
    assert_eq!(s.history().checkpoints().get("old"), Some(&keep));
    // 残った分は undo できる
    assert!(s.can_undo());
    s.undo().unwrap().unwrap();
    assert_eq!(s.history().len(), keep - 1);

    // redo スタックがある間は compact しない
    assert!(s.compact(5).unwrap().is_none());
    s.redo().unwrap().unwrap();
    // keep 以上のときも何もしない
    assert!(s.compact(keep).unwrap().is_none());
}

#[test]
fn loop_clip_expands_notes_for_playback() {
    let mut clip = Clip::new_midi(ClipId::new(), "drums", Tick(0), Tick(3840 * 4));
    let n = |pos: u64, dur: u64| Note {
        id: NoteId::new(),
        pos: Tick(pos),
        dur: Tick(dur),
        pitch: 36,
        vel: 100,
        articulation: Articulation::Normal,
        pitch_curve: vec![],
        glide_ms: None,
        vibrato: None,
        volume_curve: vec![],
        brightness_curve: vec![],
        condition: None,
    };
    // 1 小節パターン: 頭と、ループ境界をまたぐ音と、ループ外の音
    *clip.notes_mut().unwrap() = vec![n(0, 480), n(3600, 480), n(5000, 480)];
    // ループなし: クリップ内のノートがそのまま
    assert_eq!(clip.playback_notes().len(), 3);

    let mut p = Project::new("loop");
    let tid = TrackId::new();
    p.apply(&Command::AddTrack {
        track: Track::new(tid.clone(), "D", TrackKind::Midi),
        index: None,
    })
    .unwrap();
    let cid = clip.id.clone();
    p.apply(&Command::AddClip { track: tid, clip }).unwrap();
    let applied = p
        .apply(&Command::SetClipLoop {
            id: cid.clone(),
            loop_len: Some(Tick(3840)),
        })
        .unwrap();
    let (_, clip) = p.clip(&cid).unwrap();
    let played = clip.playback_notes();
    // 4 小節 × 2 音(ループ外の 5000 は鳴らない)
    assert_eq!(played.len(), 8);
    assert_eq!(played[2].pos, Tick(3840));
    // 境界をまたぐ音はループ境界で切れる
    assert_eq!(played[1].dur, Tick(240));
    // 逆コマンドで元に戻る
    p.apply(&applied.inverse).unwrap();
    assert_eq!(p.clip(&cid).unwrap().1.loop_len(), None);
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
fn project_at_rebuilds_past_states_without_touching_session() {
    // 聴き比べ用: 過去の地点のプロジェクトを、今のセッションを変えずに作る
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
    let after_mute = s.project().clone();
    let mute_entry = s.history().applied()[0].id.clone();
    s.apply(
        Command::SetMasterVolume { volume_db: -6.0 },
        Author::Human,
        "master",
    )
    .unwrap();
    let now = s.project().clone();

    let at = |p: HistoryPoint| s.project_at(s.resolve_point(&p).unwrap()).unwrap();
    assert_eq!(at(HistoryPoint::Checkpoint("before".into())), start);
    assert_eq!(at(HistoryPoint::BeforeEntry(mute_entry)), start);
    assert_eq!(at(HistoryPoint::Back(1)), after_mute);
    assert_eq!(at(HistoryPoint::Back(0)), now);
    assert_eq!(at(HistoryPoint::Back(99)), start);
    assert_eq!(s.project(), &now, "セッションは変わらない");
    assert!(s
        .resolve_point(&HistoryPoint::Checkpoint("no_such".into()))
        .is_err());
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
            articulation: Articulation::Normal,
            pitch_curve: vec![],
            glide_ms: None,
            vibrato: None,
            volume_curve: vec![],
            brightness_curve: vec![],
            condition: None,
        }, // 左に残る
        Note {
            id: ids[1].clone(),
            pos: Tick(1680),
            dur: Tick(480),
            pitch: 62,
            vel: 100,
            articulation: Articulation::Normal,
            pitch_curve: vec![],
            glide_ms: None,
            vibrato: None,
            volume_curve: vec![],
            brightness_curve: vec![],
            condition: None,
        }, // 分割点(1920)をまたぐ → 切り詰め
        Note {
            id: ids[2].clone(),
            pos: Tick(2880),
            dur: Tick(480),
            pitch: 64,
            vel: 100,
            articulation: Articulation::Normal,
            pitch_curve: vec![],
            glide_ms: None,
            vibrato: None,
            volume_curve: vec![],
            brightness_curve: vec![],
            condition: None,
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

#[test]
fn sends_go_only_to_buses_and_undo_restores() {
    let mut p = Project::new("send");
    let (a, bus, bus2) = (TrackId::new(), TrackId::new(), TrackId::new());
    for (id, kind) in [
        (&a, TrackKind::Midi),
        (&bus, TrackKind::Bus),
        (&bus2, TrackKind::Bus),
    ] {
        p.apply(&Command::AddTrack {
            track: Track::new(id.clone(), "t", kind),
            index: None,
        })
        .unwrap();
    }
    let send = |track: &TrackId, target: &TrackId, db: Option<f32>| Command::SetSend {
        track: track.clone(),
        target: target.clone(),
        level_db: db,
        pre_fader: false,
    };
    // バス以外 → バス はよい。量の変更・取り消しで前の値に戻る
    p.apply(&send(&a, &bus, Some(-6.0))).unwrap();
    let applied = p.apply(&send(&a, &bus, Some(-12.0))).unwrap();
    assert_eq!(p.track(&a).unwrap().sends[0].level_db, -12.0);
    p.apply(&applied.inverse).unwrap();
    assert_eq!(p.track(&a).unwrap().sends[0].level_db, -6.0);
    // 外す → 取り消しで戻る
    let applied = p.apply(&send(&a, &bus, None)).unwrap();
    assert!(p.track(&a).unwrap().sends.is_empty());
    p.apply(&applied.inverse).unwrap();
    assert_eq!(p.track(&a).unwrap().sends.len(), 1);
    // バス → バスはよい(輪になるのは不可)、バス以外への送り、範囲外は不可
    p.apply(&send(&bus, &bus2, Some(0.0))).unwrap();
    assert!(p.apply(&send(&bus2, &bus, Some(0.0))).is_err(), "輪");
    assert_eq!(p.track(&bus2).unwrap().sends.len(), 0, "輪の送りは入らない");
    assert!(p.apply(&send(&bus2, &a, Some(0.0))).is_err());
    // 出力先(グループ): バスだけ、輪にならないこと、取り消しで戻る
    let out = |id: &TrackId, to: Option<&TrackId>| Command::SetTrackProp {
        id: id.clone(),
        prop: glaux_core::TrackProp::Output(to.cloned()),
    };
    let applied = p.apply(&out(&a, Some(&bus))).unwrap();
    assert_eq!(p.track(&a).unwrap().output.as_ref(), Some(&bus));
    p.apply(&applied.inverse).unwrap();
    assert!(p.track(&a).unwrap().output.is_none());
    assert!(
        p.apply(&out(&bus2, Some(&bus))).is_err(),
        "bus → bus2 があるので輪"
    );
    assert!(p.apply(&out(&a, Some(&a))).is_err());
    assert!(
        p.track(&bus2).unwrap().output.is_none(),
        "失敗したら変えない"
    );
    assert!(p.apply(&send(&a, &bus, Some(40.0))).is_err());
    // バスにクリップは置けない
    let clip = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(480));
    assert!(p
        .apply(&Command::AddClip {
            track: bus.clone(),
            clip
        })
        .is_err());
}

#[test]
fn update_notes_changes_curve_and_glide_and_track_legato_settings() {
    let mut p = seed_project();
    let (tid, cid, nid) = {
        let t = &p.tracks[0];
        let c = &t.clips[0];
        (t.id.clone(), c.id.clone(), c.notes().unwrap()[1].id.clone())
    };
    let before = p.clone();
    let curve = vec![
        PitchPoint {
            tick: Tick(0),
            cents: -300.0,
            shape: Default::default(),
        },
        PitchPoint {
            tick: Tick(240),
            cents: 0.0,
            shape: Default::default(),
        },
    ];
    let applied = p
        .apply(&Command::UpdateNotes {
            clip: cid.clone(),
            changes: vec![NoteChange::new(nid.clone())
                .pitch_curve(curve.clone())
                .glide_ms(400.0)],
        })
        .unwrap();
    let n = p.clip(&cid).unwrap().1.notes().unwrap()[1].clone();
    assert_eq!(n.pitch_curve, curve, "ピッチカーブが実際に変わる");
    assert_eq!(n.glide_ms, Some(400.0));
    p.apply(&applied.inverse).unwrap();
    assert_eq!(p, before);

    // 範囲外は拒否(カーブ ±2400 超・点が多すぎる・滑る時間)
    let bad = |ch: NoteChange| Command::UpdateNotes {
        clip: cid.clone(),
        changes: vec![ch],
    };
    let too_far = vec![PitchPoint {
        tick: Tick(0),
        cents: 3000.0,
        shape: Default::default(),
    }];
    assert!(p
        .apply(&bad(NoteChange::new(nid.clone()).pitch_curve(too_far)))
        .is_err());
    let too_many = (0..17)
        .map(|i| PitchPoint {
            tick: Tick(i * 10),
            cents: 0.0,
            shape: Default::default(),
        })
        .collect();
    assert!(p
        .apply(&bad(NoteChange::new(nid.clone()).pitch_curve(too_many)))
        .is_err());
    assert!(p
        .apply(&bad(NoteChange::new(nid.clone()).glide_ms(5000.0)))
        .is_err());
    let vib = Vibrato {
        rate_hz: 5.5,
        depth_cents: 40.0,
        delay_ms: 200.0,
        fade_in_ms: 200.0,
        fade_out_ms: 0.0,
        rate_end_hz: None,
    };
    assert!(p
        .apply(&bad(NoteChange::new(nid.clone()).vibrato(Vibrato {
            rate_hz: 40.0,
            ..vib
        })))
        .is_err());
    // ビブラートを付けて、深さ 0 で消す(どちらも undo で戻る)
    let inv = p
        .apply(&bad(NoteChange::new(nid.clone()).vibrato(vib)))
        .unwrap()
        .inverse;
    let vib_of = |p: &Project| p.clip(&cid).unwrap().1.notes().unwrap()[1].vibrato;
    assert_eq!(vib_of(&p), Some(vib));
    p.apply(&inv).unwrap();
    assert_eq!(vib_of(&p), None);
    // 音量の曲線: 範囲外・点が多すぎるのは拒否、差し替えは undo で戻る
    let fade = vec![
        CurvePoint::new(Tick(0), 0.0, CurveShape::Linear),
        CurvePoint::new(Tick(480), -24.0, CurveShape::EaseIn),
    ];
    assert!(p
        .apply(&bad(NoteChange::new(nid.clone()).volume_curve(vec![
            CurvePoint::new(Tick(0), 40.0, CurveShape::Linear)
        ])))
        .is_err());
    assert!(p
        .apply(&bad(NoteChange::new(nid.clone()).brightness_curve(
            (0..9)
                .map(|i| CurvePoint::new(Tick(i * 10), 0.0, CurveShape::Linear))
                .collect()
        )))
        .is_err());
    let inv = p
        .apply(&bad(NoteChange::new(nid.clone()).volume_curve(fade.clone())))
        .unwrap()
        .inverse;
    assert_eq!(
        p.clip(&cid).unwrap().1.notes().unwrap()[1].volume_curve,
        fade
    );
    p.apply(&inv).unwrap();
    assert!(p.clip(&cid).unwrap().1.notes().unwrap()[1]
        .volume_curve
        .is_empty());

    // トラックのつなぎの設定: 設定 → 取り消しで未設定に戻る。範囲外は拒否
    let set = p
        .apply(&Command::SetParam {
            track: tid.clone(),
            path: ParamPath::track("glide_ms"),
            value: 300.0.into(),
        })
        .unwrap();
    assert_eq!(p.track(&tid).unwrap().glide_ms, Some(300.0));
    p.apply(&set.inverse).unwrap();
    assert_eq!(p.track(&tid).unwrap().glide_ms, None);
    assert!(p
        .apply(&Command::SetParam {
            track: tid.clone(),
            path: ParamPath::track("legato_ms"),
            value: 500.0.into(),
        })
        .is_err());
}
