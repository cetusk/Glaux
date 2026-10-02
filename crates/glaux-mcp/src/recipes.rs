//! 定番の手順(レシピ)を、今のプロジェクトから絶対値のコマンド列に組み立てる(MCP の apply_recipe)。
//!
//! プロのミックス・音作りで毎回同じように組む手順を、AI が 10〜20 手を毎回組み立てずに 1 回で呼べるようにする。
//! 呼び出し側(MCP)が Batch 1 件として適用する(1 回の undo で戻る)。新規 ID はここ(コマンドを作る側)で振る。
//!
//! - `send_reverb`: リバーブのバス(返りは 300Hz〜8kHz に絞り、プリディレイ)を作り(あれば使い回し)、トラックから送る
//! - `send_delay`: テンポに合わせたディレイのバス(ピンポン、返りは 400Hz〜6kHz)を作り、トラックから送る
//! - `kick_bass`: キックとベースのすみ分け。ベースにキックで沈むサイドチェイン(または低域だけ沈むダイナミック EQ)
//! - `vocal_chain`: ボーカル(歌・語り)の定番の挿し方: ハイパス → 濁りを削る EQ → コンプ → 歯擦音を抑える(ダイナミック EQ)
//!   → 軽いテープの飽和、プレートのリバーブへ送る
//! - `supersaw`: 7 声を左右に広げたスーパーソー + 1 オクターブ上の層、ホールのリバーブと付点 8 分のディレイへ送る
//! - `parallel_drums`: ドラムの並列コンプ(強く潰したバスを原音に混ぜて、アタックを残したまま太く)

use glaux_core::{
    Command, Device, Effect, FxId, Layer, ParamValue, PluginSource, Project, Track, TrackId,
    TrackKind,
};

/// 組み立てた結果
pub struct Recipe {
    pub label: String,
    pub commands: Vec<Command>,
    /// 何をしたか(人と AI に返す説明)
    pub steps: Vec<String>,
}

pub const RECIPES: &[&str] = &[
    "send_reverb",
    "send_delay",
    "kick_bass",
    "vocal_chain",
    "supersaw",
    "parallel_drums",
];

/// レシピの引数(使わないものは無視)
#[derive(Default, Clone, Debug)]
pub struct RecipeArgs {
    /// 対象のトラック(ID か名前)
    pub tracks: Vec<String>,
    /// send_reverb の響き: room / hall / plate
    pub space: Option<String>,
    /// 送る量(dB)
    pub level_db: Option<f32>,
    /// send_delay の音符("1/8d"・"1/4" など)
    pub note: Option<String>,
    /// kick_bass の方式: duck / dynamic_eq
    pub mode: Option<String>,
    /// kick_bass のキックとベースのトラック(省略で推定)
    pub kick: Option<String>,
    pub bass: Option<String>,
}

fn fx(name: &str, params: &[(&str, f64)]) -> Effect {
    let mut e = Effect::builtin(FxId::new(), name);
    for (k, v) in params {
        e.params.insert((*k).to_owned(), ParamValue::Float(*v));
    }
    e
}

fn fx_enum(mut e: Effect, k: &str, v: &str) -> Effect {
    e.params
        .insert(k.to_owned(), ParamValue::Enum(v.to_owned()));
    e
}

/// ID か名前でトラックを探す
pub fn find_track<'a>(project: &'a Project, key: &str) -> Option<&'a Track> {
    TrackId::parse(key)
        .ok()
        .and_then(|id| project.track(&id))
        .or_else(|| project.tracks.iter().find(|t| t.name == key))
        .or_else(|| {
            let k = key.to_lowercase();
            project
                .tracks
                .iter()
                .find(|t| t.name.to_lowercase().contains(&k))
        })
}

fn builtin_name(t: &Track) -> Option<&str> {
    match &t.device.as_ref()?.source {
        PluginSource::Builtin { name } => Some(name.as_str()),
        _ => None,
    }
}

/// ドラムのトラックか(内蔵の drum、名前)
pub fn is_drum(t: &Track) -> bool {
    let n = t.name.to_lowercase();
    builtin_name(t) == Some("drum")
        || [
            "drum",
            "ドラム",
            "kick",
            "キック",
            "snare",
            "スネア",
            "hat",
            "ハット",
            "perc",
        ]
        .iter()
        .any(|k| n.contains(k))
}

/// MIDI のノートの高さの中央値
fn median_pitch(t: &Track) -> Option<u8> {
    let mut p: Vec<u8> = t
        .clips
        .iter()
        .filter_map(|c| c.notes())
        .flat_map(|ns| ns.iter().map(|n| n.pitch))
        .collect();
    if p.is_empty() {
        return None;
    }
    p.sort_unstable();
    Some(p[p.len() / 2])
}

/// キックのトラックを推定する(名前に kick / キック、無ければ内蔵の drum 音源か名前に drum / ドラム。
/// 打楽器一般〈perc〉は含めない: 管弦楽の打楽器をキックと取り違えないように)
pub fn guess_kick(project: &Project) -> Option<&Track> {
    let tracks = || project.tracks.iter().filter(|t| t.kind == TrackKind::Midi);
    tracks()
        .find(|t| {
            let n = t.name.to_lowercase();
            n.contains("kick") || n.contains("キック")
        })
        .or_else(|| tracks().find(|t| builtin_name(t) == Some("drum")))
        .or_else(|| {
            tracks().find(|t| {
                let n = t.name.to_lowercase();
                n.contains("drum") || n.contains("ドラム")
            })
        })
}

/// ベースのトラックを推定する(名前に bass / ベース、無ければドラム以外で最も低いもの)
pub fn guess_bass(project: &Project) -> Option<&Track> {
    let tracks = || {
        project
            .tracks
            .iter()
            .filter(|t| t.kind != TrackKind::Bus && !is_drum(t))
    };
    tracks()
        .find(|t| {
            let n = t.name.to_lowercase();
            n.contains("bass") || n.contains("ベース")
        })
        .or_else(|| {
            tracks()
                .filter_map(|t| median_pitch(t).map(|p| (p, t)))
                .filter(|(p, _)| *p < 52)
                .min_by_key(|(p, _)| *p)
                .map(|(_, t)| t)
        })
}

/// 名前のバスを探し、無ければ作るコマンドを足す。返り値はバスの ID
fn ensure_bus(
    project: &Project,
    name: &str,
    effects: Vec<Effect>,
    cmds: &mut Vec<Command>,
    steps: &mut Vec<String>,
) -> TrackId {
    if let Some(b) = project
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Bus && t.name == name)
    {
        steps.push(format!("既存のバス「{name}」を使う"));
        return b.id.clone();
    }
    let mut bus = Track::new(TrackId::new(), name, TrackKind::Bus);
    bus.effects = effects;
    let id = bus.id.clone();
    steps.push(format!("バス「{name}」を作る"));
    cmds.push(Command::AddTrack {
        track: bus,
        index: None,
    });
    id
}

fn send(cmds: &mut Vec<Command>, steps: &mut Vec<String>, from: &Track, to: &TrackId, db: f32) {
    cmds.push(Command::SetSend {
        track: from.id.clone(),
        target: to.clone(),
        level_db: Some(db),
        pre_fader: false,
    });
    steps.push(format!("「{}」から {db:.0}dB 送る", from.name));
}

fn targets<'a>(project: &'a Project, args: &RecipeArgs) -> Result<Vec<&'a Track>, String> {
    let mut out = Vec::new();
    for k in &args.tracks {
        let t = find_track(project, k).ok_or_else(|| format!("トラックが見つかりません: {k}"))?;
        if t.kind == TrackKind::Bus {
            return Err(format!("「{}」はバスです(バスからは送れない)", t.name));
        }
        out.push(t);
    }
    Ok(out)
}

fn reverb_bus_effects(space: &str) -> Vec<Effect> {
    let (size, damping, predelay, character) = match space {
        "room" => (0.35, 0.45, 12.0, "room"),
        "plate" => (0.6, 0.3, 25.0, "plate"),
        _ => (0.8, 0.35, 30.0, "room"),
    };
    vec![
        // 返りは低域と高域を削る(濁り・耳に痛い残響を抑える)
        fx("eq", &[("hp_freq", 300.0), ("lp_freq", 8000.0)]),
        fx_enum(
            fx(
                "reverb",
                &[
                    ("mix", 1.0),
                    ("size", size),
                    ("damping", damping),
                    ("predelay_ms", predelay),
                ],
            ),
            "character",
            character,
        ),
    ]
}

fn reverb_bus_name(space: &str) -> String {
    match space {
        "room" => "リバーブ(ルーム)".to_owned(),
        "plate" => "リバーブ(プレート)".to_owned(),
        _ => "リバーブ(ホール)".to_owned(),
    }
}

/// ディレイの時間(ms)。音符は "1/8d" など(曲の頭のテンポで)
fn delay_ms(project: &Project, note: &str) -> Result<f64, String> {
    let ticks = glaux_core::meter::sync_ticks(note);
    if ticks <= 0.0 {
        return Err(format!(
            "note が読めません: {note}(\"1/8d\"・\"1/4\"・\"1/8t\" など)"
        ));
    }
    let bpm = project.tempo_map.bpm_at(glaux_core::Tick(0));
    Ok(ticks / glaux_core::PPQ as f64 * 60_000.0 / bpm)
}

pub fn build(project: &Project, recipe: &str, args: &RecipeArgs) -> Result<Recipe, String> {
    let mut cmds = Vec::new();
    let mut steps = Vec::new();
    let label = match recipe {
        "send_reverb" => {
            let ts = targets(project, args)?;
            if ts.is_empty() {
                return Err("tracks に送るトラックを指定してください".into());
            }
            let space = args.space.as_deref().unwrap_or("hall");
            if !["room", "hall", "plate"].contains(&space) {
                return Err(format!("space は room / hall / plate(got: {space})"));
            }
            let bus = ensure_bus(
                project,
                &reverb_bus_name(space),
                reverb_bus_effects(space),
                &mut cmds,
                &mut steps,
            );
            for t in ts {
                send(
                    &mut cmds,
                    &mut steps,
                    t,
                    &bus,
                    args.level_db.unwrap_or(-12.0),
                );
            }
            format!("センドのリバーブ({space})")
        }
        "send_delay" => {
            let ts = targets(project, args)?;
            if ts.is_empty() {
                return Err("tracks に送るトラックを指定してください".into());
            }
            let note = args.note.as_deref().unwrap_or("1/8d");
            let ms = delay_ms(project, note)?.clamp(10.0, 1000.0);
            let bus = ensure_bus(
                project,
                &format!("ディレイ({note})"),
                vec![
                    fx(
                        "delay",
                        &[
                            ("time_ms", ms.round()),
                            ("feedback", 0.35),
                            ("mix", 1.0),
                            ("ping_pong", 1.0),
                        ],
                    ),
                    fx("eq", &[("hp_freq", 400.0), ("lp_freq", 6000.0)]),
                ],
                &mut cmds,
                &mut steps,
            );
            steps.push(format!("ディレイの時間 {ms:.0}ms({note}、曲の頭のテンポ)"));
            for t in ts {
                send(
                    &mut cmds,
                    &mut steps,
                    t,
                    &bus,
                    args.level_db.unwrap_or(-16.0),
                );
            }
            format!("センドのディレイ({note})")
        }
        "kick_bass" => {
            let kick = match &args.kick {
                Some(k) => find_track(project, k),
                None => guess_kick(project),
            }
            .ok_or("キックのトラックが見つかりません(kick で指定してください)")?;
            let bass = match &args.bass {
                Some(k) => find_track(project, k),
                None => guess_bass(project),
            }
            .ok_or("ベースのトラックが見つかりません(bass で指定してください)")?;
            if kick.id == bass.id {
                return Err("キックとベースが同じトラックです".into());
            }
            let mode = args.mode.as_deref().unwrap_or("duck");
            let e = match mode {
                // キックが鳴るたびにベース全体を短く沈める(EDM・ハウスのポンプ感も少し出る)
                "duck" => fx_enum(
                    fx(
                        "sidechain",
                        &[
                            ("threshold_db", -30.0),
                            ("duck_db", -8.0),
                            ("attack_ms", 1.0),
                            ("release_ms", 110.0),
                        ],
                    ),
                    "source",
                    &kick.id.to_string(),
                ),
                // キックが鳴る間だけ、ベースの 60Hz 付近だけ下げる(ベースの上の倍音は残る。自然なすみ分け)
                "dynamic_eq" => fx_enum(
                    fx(
                        "dynamic_eq",
                        &[
                            ("freq", 60.0),
                            ("q", 1.2),
                            ("threshold_db", -30.0),
                            ("ratio", 4.0),
                            ("range_db", -9.0),
                            ("attack_ms", 2.0),
                            ("release_ms", 90.0),
                        ],
                    ),
                    "source",
                    &kick.id.to_string(),
                ),
                other => return Err(format!("mode は duck / dynamic_eq(got: {other})")),
            };
            steps.push(format!(
                "「{}」に、キック「{}」で沈む {}",
                bass.name,
                kick.name,
                if mode == "duck" {
                    "サイドチェイン(-8dB、110ms で戻る)"
                } else {
                    "ダイナミック EQ(60Hz を最大 -9dB)"
                }
            ));
            cmds.push(Command::AddEffect {
                track: bass.id.clone(),
                effect: e,
                index: None,
            });
            "キックとベースのすみ分け".to_owned()
        }
        "vocal_chain" => {
            let ts = targets(project, args)?;
            let [t] = ts.as_slice() else {
                return Err("tracks にボーカルのトラックを 1 つ指定してください".into());
            };
            let chain = vec![
                fx(
                    "eq",
                    &[
                        ("hp_freq", 90.0),
                        ("low_gain_db", 0.0),
                        ("mid_freq", 300.0),
                        ("mid_gain_db", -2.5),
                        ("mid_q", 1.0),
                        ("high_freq", 10_000.0),
                        ("high_gain_db", 2.0),
                    ],
                ),
                fx(
                    "compressor",
                    &[
                        ("threshold_db", -20.0),
                        ("ratio", 3.0),
                        ("attack_ms", 8.0),
                        ("release_ms", 90.0),
                        ("makeup_db", 4.0),
                    ],
                ),
                // 歯擦音(サ行の刺さり)だけを抑える
                fx(
                    "dynamic_eq",
                    &[
                        ("freq", 6500.0),
                        ("q", 2.0),
                        ("threshold_db", -28.0),
                        ("ratio", 4.0),
                        ("range_db", -6.0),
                        ("attack_ms", 1.0),
                        ("release_ms", 60.0),
                    ],
                ),
                fx(
                    "tape",
                    &[
                        ("saturation", 0.2),
                        ("wow", 0.0),
                        ("flutter", 0.0),
                        ("hiss", 0.0),
                        ("crackle", 0.0),
                    ],
                ),
            ];
            for e in chain {
                cmds.push(Command::AddEffect {
                    track: t.id.clone(),
                    effect: e,
                    index: None,
                });
            }
            steps.push(format!(
                "「{}」に ハイパス 90Hz・300Hz を -2.5dB・高域 +2dB の EQ → コンプ(3:1)→ 歯擦音の抑え(6.5kHz)→ 軽いテープ",
                t.name
            ));
            let bus = ensure_bus(
                project,
                &reverb_bus_name("plate"),
                reverb_bus_effects("plate"),
                &mut cmds,
                &mut steps,
            );
            send(
                &mut cmds,
                &mut steps,
                t,
                &bus,
                args.level_db.unwrap_or(-14.0),
            );
            "ボーカルのチェーン".to_owned()
        }
        "supersaw" => {
            let ts = targets(project, args)?;
            let [t] = ts.as_slice() else {
                return Err("tracks にスーパーソーにするトラックを 1 つ指定してください".into());
            };
            if is_drum(t) {
                return Err(format!("「{}」はドラムのトラックです", t.name));
            }
            let mut d = Device::builtin("subtractive");
            for (k, v) in [
                ("unison", 7.0),
                ("detune", 30.0),
                ("spread", 0.8),
                ("analog", 0.3),
                ("cutoff", 6000.0),
                ("filter_env", 0.1),
                ("key_track", 0.4),
                ("sustain", 0.85),
                ("release", 0.35),
                ("gain_db", -15.0),
            ] {
                d.params.insert(k.to_owned(), ParamValue::Float(v));
            }
            cmds.push(Command::SetDevice {
                track: t.id.clone(),
                device: Some(d.clone()),
            });
            // 1 オクターブ上を -6dB で重ねる(きらびやかさ)
            let mut up = Layer::new(d);
            up.name = "1 オクターブ上".to_owned();
            up.transpose = 12;
            up.volume_db = -6.0;
            cmds.push(Command::SetTrackProp {
                id: t.id.clone(),
                prop: glaux_core::TrackProp::Layers(vec![up]),
            });
            steps.push(format!(
                "「{}」を 7 声・広がり 0.8・揺らぎ 0.3 のスーパーソーにし、1 オクターブ上を -6dB で重ねる",
                t.name
            ));
            let rev = ensure_bus(
                project,
                &reverb_bus_name("hall"),
                reverb_bus_effects("hall"),
                &mut cmds,
                &mut steps,
            );
            send(&mut cmds, &mut steps, t, &rev, -12.0);
            let ms = delay_ms(project, "1/8d")?.clamp(10.0, 1000.0);
            let del = ensure_bus(
                project,
                "ディレイ(1/8d)",
                vec![
                    fx(
                        "delay",
                        &[
                            ("time_ms", ms.round()),
                            ("feedback", 0.35),
                            ("mix", 1.0),
                            ("ping_pong", 1.0),
                        ],
                    ),
                    fx("eq", &[("hp_freq", 400.0), ("lp_freq", 6000.0)]),
                ],
                &mut cmds,
                &mut steps,
            );
            send(&mut cmds, &mut steps, t, &del, -18.0);
            "スーパーソー".to_owned()
        }
        "parallel_drums" => {
            let mut ts = targets(project, args)?;
            if ts.is_empty() {
                ts = project
                    .tracks
                    .iter()
                    .filter(|t| t.kind != TrackKind::Bus && is_drum(t))
                    .collect();
            }
            if ts.is_empty() {
                return Err("ドラムのトラックが見つかりません(tracks で指定してください)".into());
            }
            let bus = ensure_bus(
                project,
                "ドラムの並列コンプ",
                vec![
                    fx(
                        "compressor",
                        &[
                            ("threshold_db", -32.0),
                            ("ratio", 8.0),
                            ("attack_ms", 3.0),
                            ("release_ms", 80.0),
                            ("makeup_db", 10.0),
                        ],
                    ),
                    fx(
                        "eq",
                        &[
                            ("low_gain_db", 2.0),
                            ("high_gain_db", 2.0),
                            ("hp_freq", 40.0),
                        ],
                    ),
                ],
                &mut cmds,
                &mut steps,
            );
            for t in ts {
                send(
                    &mut cmds,
                    &mut steps,
                    t,
                    &bus,
                    args.level_db.unwrap_or(-8.0),
                );
            }
            "ドラムの並列コンプ".to_owned()
        }
        other => return Err(format!("recipe は {}(got: {other})", RECIPES.join(" / "))),
    };
    Ok(Recipe {
        label,
        commands: cmds,
        steps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::{Clip, ClipId, Note, NoteId, Session, Tick};

    fn note(pitch: u8) -> Note {
        Note {
            id: NoteId::new(),
            pos: Tick(0),
            dur: Tick(960),
            pitch,
            vel: 100,
            articulation: Default::default(),
            pitch_curve: vec![],
            glide_ms: None,
            vibrato: None,
            volume_curve: vec![],
            brightness_curve: vec![],
            condition: None,
        }
    }

    fn song() -> Project {
        let mut p = Project::new("t");
        let mut kick = Track::new(TrackId::new(), "Kick", TrackKind::Midi);
        kick.device = Some(Device::builtin("drum"));
        let mut bass = Track::new(TrackId::new(), "Sub", TrackKind::Midi);
        let mut c = Clip::new_midi(ClipId::new(), "b", Tick(0), Tick(3840));
        c.notes_mut().unwrap().push(note(36));
        bass.clips.push(c);
        let lead = Track::new(TrackId::new(), "Lead", TrackKind::Midi);
        p.tracks = vec![kick, bass, lead];
        p
    }

    fn apply(p: &Project, r: Recipe) -> Project {
        let mut s = Session::new(p.clone());
        s.apply(
            Command::batch(r.label.clone(), r.commands),
            glaux_core::Author::Human,
            r.label,
        )
        .unwrap();
        s.project().clone()
    }

    #[test]
    fn kick_and_bass_are_guessed_and_separated() {
        let p = song();
        let r = build(&p, "kick_bass", &RecipeArgs::default()).unwrap();
        let after = apply(&p, r);
        let bass = after.tracks.iter().find(|t| t.name == "Sub").unwrap();
        let sc = bass.effects.last().unwrap();
        let kick_id = after.tracks[0].id.to_string();
        assert_eq!(sc.params.get("source"), Some(&ParamValue::Enum(kick_id)));
    }

    #[test]
    fn send_reverb_makes_one_bus_and_reuses_it() {
        let p = song();
        let args = RecipeArgs {
            tracks: vec!["Lead".into()],
            ..Default::default()
        };
        let p1 = apply(&p, build(&p, "send_reverb", &args).unwrap());
        let buses = |p: &Project| p.tracks.iter().filter(|t| t.kind == TrackKind::Bus).count();
        assert_eq!(buses(&p1), 1);
        let lead = p1.tracks.iter().find(|t| t.name == "Lead").unwrap();
        assert_eq!(lead.sends.len(), 1);
        // もう一度呼んでも同じバスを使う
        let args2 = RecipeArgs {
            tracks: vec!["Sub".into()],
            ..Default::default()
        };
        let p2 = apply(&p1, build(&p1, "send_reverb", &args2).unwrap());
        assert_eq!(buses(&p2), 1);
    }

    #[test]
    fn supersaw_and_delay_follow_the_tempo() {
        let p = song();
        let args = RecipeArgs {
            tracks: vec!["Lead".into()],
            ..Default::default()
        };
        let after = apply(&p, build(&p, "supersaw", &args).unwrap());
        let lead = after.tracks.iter().find(|t| t.name == "Lead").unwrap();
        assert_eq!(lead.layers.len(), 1);
        assert_eq!(lead.sends.len(), 2);
        // 120 BPM の付点 8 分 = 375ms
        let del = after
            .tracks
            .iter()
            .find(|t| t.name == "ディレイ(1/8d)")
            .unwrap();
        assert_eq!(
            del.effects[0].params.get("time_ms"),
            Some(&ParamValue::Float(375.0))
        );
    }

    #[test]
    fn every_recipe_builds_on_a_simple_song() {
        let p = song();
        for r in RECIPES {
            let args = RecipeArgs {
                tracks: vec!["Lead".into()],
                ..Default::default()
            };
            let built = build(&p, r, &args).unwrap_or_else(|e| panic!("{r}: {e}"));
            assert!(!built.commands.is_empty(), "{r}");
            apply(&p, built);
        }
        assert!(build(&p, "nope", &RecipeArgs::default()).is_err());
    }
}
