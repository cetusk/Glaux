//! 曲から起こす機能の評価: 曲の骨組み(拍・小節頭・テンポ・調・和音・チューニング)の当たり具合を測る。
//!
//! 正解の分かっている曲を Glaux で組み立てて書き出し(テンポの変化・拍子・調・和音・編成・チューニングのずれ・
//! 冒頭の無音を散らす)、`glaux_mcp::recon` の解析を当てて、研究と同じ指標(`glaux_engine::mir_eval`)で数える。
//! 実在の録音の評価セットがあれば `--dataset` で同じ指標を出せる。
//!
//! ```text
//! cargo run --release -p glaux-mcp --example recon_eval -- [オプション]
//!   --cases N        組み立てる曲の数(既定 24)
//!   --seed N         乱数の種(既定 1)
//!   --sf2 FILE       GM の SoundFont で鳴らす曲も混ぜる(ピアノ・ギター・弦・ドラムキット)
//!   --dataset DIR    実在の録音の評価セット(音声と同じ名前の .beats / .lab / .key)
//!   --project DIR    Glaux の曲(.glaux のフォルダ。それを並べたフォルダでもよい)を書き出して、曲の中身(テンポ・拍子・音符)を正解に測る。
//!                    調・和音の正解は曲の音符から求めた小節ごとの和音。パートの正解はトラックごと(ドラムはまとめて 1 つ)。
//!                    --cases 0 と組み合わせるとその曲だけ
//!   --beat M         拍の求め方(dbn〈既定〉/ minimal)
//!   --chord M        和音の求め方(nnls〈既定〉/ perbar)
//!   --chord-sharp X / --chord-bass X / --chord-change 小節頭,中ほど,ほか / --triads / --no-inversions
//!                    和音の認識の設定(`glaux_engine::tonal::ChordOptions`)を変えて比べる
//!   --separate DIR   (--parts と)曲を Demucs で 歌・ドラム・ベース・その他 に分けて起こす経路も測る(作業場の「分離ツールで分ける」と同じ割り当て。
//!                    分けた音は DIR に置き、次からは使い回す)。旋律は歌・「その他」のどちらかに決めて起こした場合も測る
//!   --notes DIR      (--parts と)ほかの道具が曲全体から起こした音符(DIR/<曲の名前>.json。onset・offset・pitch・program・is_drum・velocity の並び)
//!                    も同じ方法で採点する(方式の名前は Ext。音程のある音は Glaux と同じく声部に分ける)
//!   --labels FILE    (--separate と)楽器の見分けも測る。FILE はトラックの楽器の種類の正解({"曲": {"トラック": "pad", ...}})。
//!                    分けた「その他」から旋律・和音の楽器を、「歌」から声を、区間(10 秒)ごとに当てる。
//!                    --protos FILE で、楽器の見本(instrument_protos で作る)と比べる方式も測る
//!   --save DIR       組み立てた曲の音声(wav)と正解(json)を書き出す(聴いて確かめる用)
//!   --json FILE      曲ごとの結果を JSON で書き出す
//! ```
//!
//! 評価セットの形式: `.beats` は 1 行に「時刻 [拍の位置]」(位置 1 が小節頭。madmom などと同じ)、
//! `.lab` は「開始 終了 和音」(Harte の記法 C:maj / A:min7 / G:7/3、N は和音なし)、
//! `.key` は「C major」「A:minor」など 1 行。

use glaux_core::time::{TempoEvent, TempoMap, TimeSigEvent, PPQ};
use glaux_core::{
    chord, Clip, ClipContent, ClipId, Device, Note, NoteId, PluginSource, Project, Tick, Track,
    TrackId, TrackKind,
};
use glaux_engine::mir_eval::{self, ChordSpan, ChordVocab, NoteSpan};
use glaux_mcp::recon::{self, BeatMethod, ChordMethod, SkeletonOptions};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// ---- 乱数(再現できるように自前) ----------------------------------------

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn f(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.f()
    }
    fn int(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn chance(&mut self, p: f64) -> bool {
        self.f() < p
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.int(v.len())]
    }
}

// ---- 正解 --------------------------------------------------------------

#[derive(Clone, Debug, serde::Serialize)]
struct Truth {
    beats: Vec<f64>,
    downbeats: Vec<f64>,
    beats_per_bar: Option<u32>,
    /// 調("C major" など)
    key: Option<String>,
    chords: Vec<ChordSpan>,
    tuning_cents: Option<f64>,
    /// 8 分の裏の位置(%)。8 分の裏が無ければ None
    swing_8th_pct: Option<f64>,
    /// 人の揺れがあるか
    human: Option<bool>,
}

impl Truth {
    /// 拍の間隔の中央値から求めたテンポ
    fn bpm(&self) -> Option<f64> {
        let mut ibi: Vec<f64> = self.beats.windows(2).map(|w| w[1] - w[0]).collect();
        if ibi.is_empty() {
            return None;
        }
        ibi.sort_by(f64::total_cmp);
        Some(60.0 / ibi[ibi.len() / 2])
    }
}

struct Case {
    name: String,
    /// 分類(集計の切り口)。"style:band" / "tempo:step" など
    tags: Vec<String>,
    frames: Vec<f32>,
    /// 左右の差((L − R) / 2。ステレオのときだけ。分離ツールにはステレオで渡す)
    side: Option<Vec<f32>>,
    sample_rate: f32,
    truth: Truth,
    /// パートごとの正解の音符(`--parts` のとき。曲そのものから起こして測る)
    parts: Vec<PartCase>,
    /// トラックごとの正解の音符(Glaux の曲のとき。楽器の見分けの答え合わせに使う)
    tracks: Vec<(String, Vec<NoteSpan>)>,
}

/// パート 1 つ(正解の音符)。
///
/// トラックごとに書き出した音(分離が完全にできたときの音)は持たない。実際には曲しか無いので、
/// それで測った点は使い方の再現にならず、いずれその点で満足してしまうため、評価に使うことを禁じている
/// (2026-10-15、ユーザーと決定。調査文書 13.12 節)。分けて起こす経路は、曲から分離ツールで分けた音で測る
struct PartCase {
    /// 鳴らした音源(調べる用)
    device: String,
    kind: recon::PartKind,
    truth: Vec<NoteSpan>,
}

// ---- 曲の組み立て ------------------------------------------------------

const NAMES: [&str; 12] = [
    "C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
];

/// 調の中の和音(主音からの半音, 種類, 最低音〈根音からの半音。0 = 基本形〉)
type Deg = (u8, &'static str, u8);

/// 和音の進行と、調がはっきりするか(主和音を含み、主和音から始まる。vi から始まる進行などは
/// 平行調とも聞けるので、調の正解を付けない)
const MAJOR_PROGS: &[(&[Deg], bool)] = &[
    (&[(0, "", 0), (7, "", 0), (9, "m", 0), (5, "", 0)], true),
    (&[(0, "", 0), (9, "m", 0), (5, "", 0), (7, "", 0)], true),
    (&[(9, "m", 0), (5, "", 0), (0, "", 0), (7, "", 0)], false),
    (
        &[(2, "m7", 0), (7, "7", 0), (0, "maj7", 0), (0, "maj7", 0)],
        true,
    ),
    (&[(0, "", 0), (5, "", 0), (7, "", 0), (5, "", 0)], true),
    (
        &[(5, "maj7", 0), (7, "", 0), (4, "m7", 0), (9, "m", 0)],
        false,
    ),
    (&[(0, "", 0), (7, "", 4), (9, "m", 0), (0, "", 7)], true),
    (&[(0, "", 0), (10, "", 0), (5, "", 0), (0, "", 0)], false),
    (&[(5, "", 0), (7, "", 0), (4, "m", 0), (9, "m", 0)], false),
    (&[(0, "", 0), (4, "m", 0), (5, "", 0), (7, "sus4", 0)], true),
];

const MINOR_PROGS: &[(&[Deg], bool)] = &[
    (&[(0, "m", 0), (8, "", 0), (3, "", 0), (10, "", 0)], false),
    (&[(0, "m", 0), (5, "m", 0), (7, "", 0), (0, "m", 0)], true),
    (&[(0, "m", 0), (10, "", 0), (8, "", 0), (7, "", 0)], true),
    (&[(8, "", 0), (10, "", 0), (0, "m", 0), (0, "m", 0)], false),
    (
        &[(0, "m7", 0), (5, "m7", 0), (10, "7", 0), (3, "maj7", 0)],
        false,
    ),
    (&[(0, "m", 0), (7, "7", 11), (0, "m", 3), (5, "m", 0)], true),
];

/// 和音の記号と、鳴らすピッチクラス(根音から)と最低音のピッチクラス
fn realize(tonic: u8, d: Deg) -> (String, Vec<u8>, u8) {
    let root = (tonic + d.0) % 12;
    let bass = (root + d.2) % 12;
    let mut name = format!("{}{}", NAMES[root as usize], d.1);
    if d.2 != 0 {
        name = format!("{name}/{}", NAMES[bass as usize]);
    }
    let c = chord::parse(&name)
        .ok()
        .flatten()
        .unwrap_or_else(|| panic!("読めない和音: {name}"));
    (c.name.clone(), c.pitch_classes(), bass)
}

fn note(pos: i64, dur: i64, pitch: i32, vel: i32) -> Note {
    Note {
        locked: false,
        id: NoteId::new(),
        pos: Tick(pos.max(0) as u64),
        dur: Tick(dur.max(30) as u64),
        pitch: pitch.clamp(0, 127) as u8,
        vel: vel.clamp(1, 127) as u8,
        articulation: Default::default(),
        pitch_curve: vec![],
        glide_ms: None,
        vibrato: None,
        volume_curve: vec![],
        brightness_curve: vec![],
        condition: None,
    }
}

/// 音源の決め方(内蔵か SoundFont か)
#[derive(Clone, Copy)]
enum Voice {
    Drums,
    Bass,
    Comp,
    Pad,
    Lead,
    Piano,
}

fn device(v: Voice, sf2: Option<&str>, rng: &mut Rng, electro: bool) -> Device {
    use glaux_core::ParamValue as P;
    if let Some(sf) = sf2 {
        let (bank, preset) = match v {
            Voice::Drums => (128, 0),
            Voice::Bass => (0, *rng.pick(&[33u16, 34, 38])),
            Voice::Comp => (0, *rng.pick(&[25u16, 27, 4, 0])),
            Voice::Pad => (0, *rng.pick(&[48u16, 49, 89, 19])),
            Voice::Lead => (0, *rng.pick(&[73u16, 65, 80, 56])),
            Voice::Piano => (0, *rng.pick(&[0u16, 1, 4])),
        };
        return Device {
            source: PluginSource::Sf2 {
                soundfont: sf.to_owned(),
                bank,
                preset,
            },
            params: Default::default(),
        };
    }
    let mut d;
    match v {
        Voice::Drums => {
            d = Device::builtin("drum");
            let kit = if electro {
                *rng.pick(&["808", "909"])
            } else {
                *rng.pick(&["modern", "909"])
            };
            d.params.insert("kit".into(), P::Enum(kit.into()));
        }
        Voice::Bass => {
            d = Device::builtin("subtractive");
            d.params.insert(
                "waveform".into(),
                P::Enum((*rng.pick(&["saw", "square"])).into()),
            );
            d.params
                .insert("cutoff".into(), P::Float(rng.range(400.0, 1500.0)));
            d.params.insert("release".into(), P::Float(0.08));
        }
        Voice::Comp => {
            d = Device::builtin(*rng.pick(&["pluck", "fm", "additive"]));
        }
        Voice::Pad => {
            d = Device::builtin("subtractive");
            d.params.insert("waveform".into(), P::Enum("saw".into()));
            d.params
                .insert("cutoff".into(), P::Float(rng.range(1500.0, 4000.0)));
            d.params
                .insert("attack".into(), P::Float(rng.range(0.1, 0.4)));
            d.params.insert("release".into(), P::Float(0.6));
            d.params.insert("unison".into(), P::Int(3));
        }
        Voice::Lead => {
            d = Device::builtin(*rng.pick(&["subtractive", "fm", "wavetable"]));
        }
        Voice::Piano => {
            d = Device::builtin(*rng.pick(&["fm", "additive", "pluck"]));
        }
    }
    d
}

fn track(name: &str, dev: Device, notes: Vec<Note>, end: u64, volume_db: f32) -> Track {
    let mut t = Track::new(TrackId::new(), name, TrackKind::Midi);
    t.device = Some(dev);
    t.volume_db = volume_db;
    let mut clip = Clip::new_midi(ClipId::new(), name, Tick(0), Tick(end));
    if let ClipContent::Midi { notes: n, .. } = &mut clip.content {
        *n = notes;
        n.sort_by_key(|x| (x.pos, x.pitch));
    }
    t.clips.push(clip);
    t
}

/// 組み立てた曲と、tick で書いた正解
struct Song {
    project: Project,
    tags: Vec<String>,
    bpb: u32,
    bars: u32,
    /// 調(はっきりしない進行なら None)
    key: Option<chord::Key>,
    /// (開始 tick, 終了 tick, 記号)
    chords: Vec<(u64, u64, String)>,
    swing: f64,
    human: bool,
}

fn make_song(rng: &mut Rng, sf2: Option<&str>) -> Song {
    let mut tags = Vec::new();
    let style = *rng.pick(&["band", "band", "piano", "electro"]);
    tags.push(format!("style:{style}"));
    tags.push(format!(
        "sound:{}",
        if sf2.is_some() { "sf2" } else { "builtin" }
    ));
    let bpb: u32 = if rng.chance(0.25) { 3 } else { 4 };
    tags.push(format!("meter:{bpb}"));
    let bars: u32 = 24;
    let bar_ticks = bpb as u64 * PPQ;
    let end = bars as u64 * bar_ticks;
    let minor = rng.chance(0.35);
    let tonic = rng.int(12) as u8;
    let key = chord::Key { tonic, minor };
    tags.push(format!("mode:{}", if minor { "minor" } else { "major" }));

    // テンポ
    let bpm = match style {
        "electro" => rng.range(110.0, 140.0),
        "piano" => rng.range(66.0, 110.0),
        _ => rng.range(80.0, 150.0),
    };
    let kind = *rng.pick(&["const", "const", "const", "step", "rit", "drift"]);
    tags.push(format!("tempo:{kind}"));
    let mut events = vec![TempoEvent { tick: Tick(0), bpm }];
    match kind {
        "step" => {
            let f = if rng.chance(0.5) { 1.0 } else { -1.0 } * rng.range(0.08, 0.2);
            events.push(TempoEvent {
                tick: Tick(16 * bar_ticks),
                bpm: bpm * (1.0 + f),
            });
        }
        "rit" => {
            // 最後の 4 小節で 25% まで遅く(拍ごと)
            let from = (bars - 4) as u64 * bpb as u64;
            let n = 4 * bpb as u64;
            for k in 0..n {
                events.push(TempoEvent {
                    tick: Tick((from + k) * PPQ),
                    bpm: bpm * (1.0 - 0.25 * (k + 1) as f64 / n as f64),
                });
            }
        }
        "drift" => {
            for b in 1..bars {
                events.push(TempoEvent {
                    tick: Tick(b as u64 * bar_ticks),
                    bpm: bpm * (1.0 + rng.range(-0.03, 0.03)),
                });
            }
        }
        _ => {}
    }
    let human = rng.chance(0.4);
    if human {
        tags.push("timing:human".into());
    }

    // 和音: A(8 小節)B(8)A(8)。1 小節に 1 つか、4 拍子なら 2 つ
    let progs = if minor { MINOR_PROGS } else { MAJOR_PROGS };
    let (pa, clear_a) = *rng.pick(progs);
    let (pb, clear_b) = *rng.pick(progs);
    let clear_key = clear_a && clear_b;
    if !clear_key {
        tags.push("key:ambiguous".into());
    }
    let per_bar: u64 = if bpb == 4 && rng.chance(0.3) { 2 } else { 1 };
    let span = bar_ticks / per_bar;
    let mut chords: Vec<(u64, u64, String, Vec<u8>, u8)> = Vec::new();
    for i in 0..(bars as u64 * per_bar) {
        let bar = i / per_bar;
        let prog = if (8..16).contains(&bar) { pb } else { pa };
        let d = prog[(i % 4) as usize];
        let (name, pcs, bass) = realize(tonic, d);
        chords.push((i * span, (i + 1) * span, name, pcs, bass));
    }

    let jit = |rng: &mut Rng| -> i64 {
        if human {
            (rng.range(-1.0, 1.0) * 0.025 * PPQ as f64) as i64
        } else {
            0
        }
    };
    let vel = |rng: &mut Rng, v: i32| -> i32 { v + (rng.range(-10.0, 10.0) as i32) };

    let mut tracks = Vec::new();
    let electro = style == "electro";
    let pc_near = |pc: u8, lo: i32| -> i32 { lo + ((pc as i32 - lo).rem_euclid(12)) };

    // ドラム(ピアノの曲には無い。バンドは 4 小節目から入ることがある)
    if style != "piano" {
        let enter = if rng.chance(0.3) { 4u64 } else { 0 };
        let sixteenth_hats = rng.chance(0.4);
        let mut ns = Vec::new();
        for bar in enter..bars as u64 {
            let b0 = bar * bar_ticks;
            let fill = bar % 8 == 7;
            for beat in 0..bpb as u64 {
                let t = b0 + beat * PPQ;
                let kick = if bpb == 4 {
                    beat == 0 || beat == 2 || (electro && true)
                } else {
                    beat == 0
                };
                if kick {
                    ns.push(note(t as i64 + jit(rng), 240, 36, vel(rng, 110)));
                }
                let snare = if bpb == 4 {
                    beat == 1 || beat == 3
                } else {
                    beat != 0
                };
                if snare && !(fill && beat == bpb as u64 - 1) {
                    let v = if bpb == 3 { 70 } else { 105 };
                    ns.push(note(
                        t as i64 + jit(rng),
                        240,
                        if electro { 39 } else { 38 },
                        vel(rng, v),
                    ));
                }
                let subdiv = if sixteenth_hats { 4 } else { 2 };
                for s in 0..subdiv {
                    let ht = t + s * PPQ / subdiv;
                    let v = if s == 0 { 85 } else { 60 };
                    ns.push(note(ht as i64 + jit(rng), 120, 42, vel(rng, v)));
                }
            }
            if fill {
                let t = b0 + (bpb as u64 - 1) * PPQ;
                for s in 0..4 {
                    ns.push(note(
                        (t + s * PPQ / 4) as i64 + jit(rng),
                        120,
                        38,
                        vel(rng, 80 + s as i32 * 10),
                    ));
                }
            }
            if bar % 8 == 0 && bar > 0 {
                ns.push(note(b0 as i64, PPQ as i64 * 2, 49, 100));
            }
        }
        tracks.push(track(
            "Drums",
            device(Voice::Drums, sf2, rng, electro),
            ns,
            end,
            -4.0,
        ));
    }

    // ベース
    {
        let mut ns = Vec::new();
        let eighths = electro || rng.chance(0.4);
        for (s, e, _, pcs, bass) in &chords {
            let root = pc_near(*bass, 33);
            let step = if eighths { PPQ / 2 } else { PPQ };
            let mut t = *s;
            let mut k = 0;
            while t < *e {
                // たまに 5 度
                let p = if !eighths && k % 4 == 2 && rng.chance(0.3) {
                    pc_near((pcs[0] + 7) % 12, root)
                } else {
                    root
                };
                ns.push(note(
                    t as i64 + jit(rng),
                    step as i64 - 40,
                    p,
                    vel(rng, 100),
                ));
                t += step;
                k += 1;
            }
        }
        let v = if style == "piano" {
            Voice::Piano
        } else {
            Voice::Bass
        };
        tracks.push(track("Bass", device(v, sf2, rng, electro), ns, end, -3.0));
    }

    // 和音(刻む・伸ばす・分散のどれか)
    {
        let pattern = *rng.pick(&["stab", "sustain", "arp"]);
        let mut ns = Vec::new();
        for (s, e, _, pcs, _) in &chords {
            let mut voicing: Vec<i32> = pcs.iter().map(|&pc| pc_near(pc, 55)).collect();
            voicing.sort();
            match pattern {
                "sustain" => {
                    for &p in &voicing {
                        ns.push(note(
                            *s as i64 + jit(rng),
                            (*e - *s) as i64 - 20,
                            p,
                            vel(rng, 80),
                        ));
                    }
                }
                "stab" => {
                    let mut t = *s;
                    while t < *e {
                        for &p in &voicing {
                            ns.push(note(t as i64 + jit(rng), PPQ as i64 / 2, p, vel(rng, 85)));
                        }
                        t += PPQ;
                    }
                }
                _ => {
                    let mut t = *s;
                    let mut k = 0;
                    while t < *e {
                        let p = voicing[k % voicing.len()]
                            + if (k / voicing.len()) % 2 == 1 { 12 } else { 0 };
                        ns.push(note(t as i64 + jit(rng), PPQ as i64 / 2, p, vel(rng, 80)));
                        t += PPQ / 2;
                        k += 1;
                    }
                }
            }
        }
        let v = if style == "piano" {
            Voice::Piano
        } else if pattern == "sustain" {
            Voice::Pad
        } else {
            Voice::Comp
        };
        tracks.push(track("Chords", device(v, sf2, rng, electro), ns, end, -6.0));
    }

    // 旋律(拍頭は和音の音、ほかは音階の音を順次進行で)
    if style != "piano" || rng.chance(0.5) {
        let scale =
            glaux_core::harmony::scale_pitch_classes(tonic, if minor { "minor" } else { "major" });
        let mut ns = Vec::new();
        let mut cur = pc_near(tonic, 67);
        let mut t = 0u64;
        while t < end {
            let (_, _, _, pcs, _) = chords
                .iter()
                .find(|c| c.0 <= t && t < c.1)
                .expect("和音の範囲");
            let on_beat = t.is_multiple_of(PPQ);
            let len = if rng.chance(0.5) { PPQ } else { PPQ / 2 };
            if rng.chance(0.15) {
                t += len;
                continue;
            }
            let target = if on_beat {
                let pc = *rng.pick(pcs);
                pc_near(pc, cur - 6)
            } else {
                let dir = if rng.chance(0.5) { 1 } else { -1 };
                let mut p = cur + dir;
                while !scale.contains(&((p.rem_euclid(12)) as u8)) {
                    p += dir;
                }
                p
            };
            cur = target.clamp(62, 84);
            ns.push(note(
                t as i64 + jit(rng),
                len as i64 - 30,
                cur,
                vel(rng, 95),
            ));
            t += len;
        }
        tracks.push(track(
            "Lead",
            device(Voice::Lead, sf2, rng, electro),
            ns,
            end,
            -5.0,
        ));
    }

    // ハネ(8 分の裏を遅らせる。16 分のハットの曲には付けない)
    let swing = if style != "piano" && rng.chance(0.3) {
        rng.range(0.56, 0.68)
    } else {
        0.5
    };
    if swing > 0.5 {
        tags.push("groove:swing".into());
        let q = PPQ as i64;
        for t in tracks.iter_mut() {
            for c in t.clips.iter_mut() {
                if let ClipContent::Midi { notes, .. } = &mut c.content {
                    for n in notes.iter_mut() {
                        let r = n.pos.0 as i64 % q;
                        if (r - q / 2).abs() < q / 8 {
                            n.pos =
                                Tick((n.pos.0 as i64 + ((swing - 0.5) * q as f64) as i64) as u64);
                            n.dur =
                                Tick(n.dur.0.min((q as f64 * (1.0 - swing)) as u64 - 20).max(30));
                        } else if r == q / 4 || r == 3 * q / 4 {
                            // 16 分の裏は消す(ハネた 8 分と重ならないように)
                            n.vel = 1;
                        }
                    }
                    notes.retain(|n| n.vel > 1);
                }
            }
        }
    }

    let q = PPQ as i64;
    let has_offbeat = tracks.iter().any(|t| {
        t.clips.iter().any(|c| match &c.content {
            ClipContent::Midi { notes, .. } => notes.iter().any(|n| {
                let r = (n.pos.0 as i64 % q - q / 2).abs();
                r < q / 8
                    || (swing > 0.5
                        && (n.pos.0 as i64 % q - (swing * q as f64) as i64).abs() < q / 8)
            }),
            _ => false,
        })
    });

    let mut project = Project::new("eval");
    project.tempo_map = TempoMap::new(events).expect("テンポ");
    project.time_sig_map = vec![TimeSigEvent::new(Tick(0), bpb as u8, 4)];
    project.tracks = tracks;
    Song {
        project,
        tags,
        bpb,
        bars,
        key: clear_key.then_some(key),
        swing: if has_offbeat { swing } else { -1.0 },
        human,
        chords: chords
            .into_iter()
            .map(|(s, e, n, _, _)| (s, e, n))
            .collect(),
    }
}

/// `frames` を `cents` だけ高く(速く)する。返り値は (音, 時刻の倍率)
fn shift_tuning(frames: &[f32], cents: f64) -> (Vec<f32>, f64) {
    let r = 2f64.powf(cents / 1200.0);
    let n = ((frames.len() - 1) as f64 / r) as usize;
    let out = (0..n)
        .map(|i| {
            let x = i as f64 * r;
            let k = x as usize;
            let t = (x - k as f64) as f32;
            frames[k] * (1.0 - t) + frames[(k + 1).min(frames.len() - 1)] * t
        })
        .collect();
    (out, 1.0 / r)
}

fn synth_case(i: usize, seed: u64, sf2: Option<&Path>, with_parts: bool) -> Case {
    let mut rng = Rng::new(seed * 1000 + i as u64);
    let sf2_name = sf2
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned());
    let song = make_song(&mut rng, sf2_name.as_deref());
    let mut bank = glaux_engine::data::SampleBank::default();
    if let Some(dir) = sf2.and_then(|p| p.parent()) {
        bank = bank.with_sf2_dir(dir);
    }
    // SoundFont を読み込む(読まないと、SoundFont の音色は内蔵の subtractive で代わりに鳴る。
    // 2026-10-15 まではこれが無く、「GM の SoundFont で鳴らした曲」は実際には subtractive で鳴っていた)
    bank.sync(&song.project, std::path::Path::new("."));
    let sr = 48_000.0;
    let stereo = glaux_engine::export::render_project(&song.project, sr, &bank).expect("書き出し");
    let mut frames: Vec<f32> = stereo.chunks(2).map(|c| (c[0] + c[1]) * 0.5).collect();
    let mut side: Vec<f32> = stereo.chunks(2).map(|c| (c[0] - c[1]) * 0.5).collect();
    let mut tags = song.tags.clone();

    // 冒頭の無音
    let lead = if rng.chance(0.5) {
        rng.range(0.2, 1.5)
    } else {
        0.0
    };
    let pad = (lead * sr) as usize;
    if pad > 0 {
        let mut v = vec![0.0; pad];
        v.extend_from_slice(&frames);
        frames = v;
        let mut v = vec![0.0; pad];
        v.extend_from_slice(&side);
        side = v;
    }
    // チューニングのずれ
    let cents = if rng.chance(0.35) {
        tags.push("tuning:off".into());
        rng.range(-40.0, 40.0)
    } else {
        0.0
    };
    let side = if cents != 0.0 {
        shift_tuning(&side, cents).0
    } else {
        side
    };
    let (frames, scale) = if cents != 0.0 {
        shift_tuning(&frames, cents)
    } else {
        (frames, 1.0)
    };

    let tm = &song.project.tempo_map;
    let sec = |tick: u64| (tm.tick_to_seconds(Tick(tick)) + lead) * scale;
    // パートごとの正解の音符
    let mut parts = Vec::new();
    if with_parts {
        for (name, kind) in [
            ("Lead", recon::PartKind::Melody),
            ("Bass", recon::PartKind::Bass),
            ("Chords", recon::PartKind::Poly),
            ("Drums", recon::PartKind::Drums),
        ] {
            let Some(track) = song.project.tracks.iter().find(|t| t.name == name) else {
                continue;
            };
            let truth = track
                .clips
                .iter()
                .flat_map(|c| c.notes().unwrap_or(&[]).iter().map(move |n| (c.start.0, n)))
                .map(|(base, n)| NoteSpan {
                    onset: sec(base + n.pos.0),
                    offset: sec(base + n.pos.0 + n.dur.0),
                    pitch: n.pitch,
                })
                .collect();
            let device = match track.device.as_ref().map(|d| &d.source) {
                Some(glaux_core::PluginSource::Builtin { name }) => name.clone(),
                Some(glaux_core::PluginSource::Sf2 { preset, .. }) => format!("sf2:{preset}"),
                _ => String::new(),
            };
            parts.push(PartCase {
                device,
                kind,
                truth,
            });
        }
    }
    let mut beats = Vec::new();
    let mut downbeats = Vec::new();
    for b in 0..(song.bars * song.bpb) as u64 {
        let t = sec(b * PPQ);
        beats.push(t);
        if b % song.bpb as u64 == 0 {
            downbeats.push(t);
        }
    }
    let chords = song
        .chords
        .iter()
        .map(|(s, e, n)| ChordSpan {
            start: sec(*s),
            end: sec(*e),
            symbol: n.clone(),
        })
        .collect();
    Case {
        name: format!("synth-{i:02}"),
        tags,
        frames,
        side: Some(side),
        tracks: Vec::new(),
        sample_rate: sr as f32,
        truth: Truth {
            beats,
            downbeats,
            beats_per_bar: Some(song.bpb),
            key: song.key.map(|k| {
                format!(
                    "{} {}",
                    NAMES[k.tonic as usize],
                    if k.minor { "minor" } else { "major" }
                )
            }),
            chords,
            tuning_cents: Some(cents),
            swing_8th_pct: (song.swing > 0.0).then_some(song.swing * 100.0),
            human: Some(song.human),
        },
        parts,
    }
}

// ---- Glaux の曲 -----------------------------------------------------------

/// Glaux の曲を書き出して、曲の中身を正解にした評価の材料にする
fn project_case(dir: &Path, with_parts: bool) -> Result<Case, String> {
    use recon::PartKind;
    let json = std::fs::read_to_string(dir.join("project.json")).map_err(|e| e.to_string())?;
    let project = Project::from_json(&json).map_err(|e| e.to_string())?;
    let name = dir
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".into());
    let mut bank =
        glaux_engine::data::SampleBank::default().with_sf2_dir(glaux_engine::sf2::default_dir());
    bank.sync(&project, dir);
    let sr = 48_000.0f32;
    let st = glaux_engine::export::render_project(&project, sr as f64, &bank)
        .map_err(|e| e.to_string())?;
    let frames: Vec<f32> = st.chunks(2).map(|c| (c[0] + c[1]) * 0.5).collect();
    let side: Vec<f32> = st.chunks(2).map(|c| (c[0] - c[1]) * 0.5).collect();
    let tm = &project.tempo_map;
    let sec = |tick: u64| tm.tick_to_seconds(Tick(tick));

    // 鳴るトラック(ミュート・ソロを反映)と、その鳴る音符(ループの繰り返しを含む、曲の中の tick)
    let any_solo = project.tracks.iter().any(|t| t.solo);
    let audible = |t: &Track| !t.mute && (!any_solo || t.solo) && !t.reference;
    let notes_of = |t: &Track| -> Vec<(u64, u64, u8)> {
        t.clips
            .iter()
            .flat_map(|c| {
                c.playback_notes()
                    .into_iter()
                    .map(move |n| (c.start.0 + n.pos.0, c.start.0 + n.pos.0 + n.dur.0, n.pitch))
            })
            .collect()
    };
    let end_tick = project
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter().map(|c| c.start.0 + c.length.0))
        .max()
        .unwrap_or(0);

    // 拍・小節頭(拍子の分母に合わせた拍)
    let mut beats = Vec::new();
    let mut downbeats = Vec::new();
    let mut bpb_seen = Vec::new();
    for (start, len) in glaux_core::arrange::bar_grid(&project, end_tick) {
        if start >= end_tick {
            break;
        }
        let den = project
            .time_sig_map
            .iter()
            .filter(|s| s.tick.0 <= start)
            .max_by_key(|s| s.tick.0)
            .map_or(4, |s| s.den as u64);
        let beat = 4 * PPQ / den.max(1);
        let n = (len / beat).max(1);
        bpb_seen.push(n as u32);
        downbeats.push(sec(start));
        for k in 0..n {
            beats.push(sec(start + k * beat));
        }
    }
    bpb_seen.dedup();
    // 調・和音: 曲の音符から(ドラムは除く)
    let harmony = glaux_core::harmony::analyze(&project, None, None);
    let bars = glaux_core::arrange::bar_grid(&project, end_tick);
    let chords = harmony
        .chords
        .iter()
        .filter_map(|c| {
            let (start, len) = *bars.get(c.bar.checked_sub(1)?)?;
            Some(ChordSpan {
                start: sec(start),
                end: sec(start + len),
                symbol: if c.chord == "N.C." {
                    "N".into()
                } else {
                    c.chord.clone()
                },
            })
        })
        .collect();

    // パートの正解: ドラムのトラックはまとめて 1 つ、ほかはトラックごと(音符が少ないもの〈効果音など〉は除く)。
    // 重なる音が 2 割を超えれば和音、そうでなければ音の高さの中央値で旋律かベースか
    let mut parts = Vec::new();
    if with_parts {
        let is_bus = |t: &Track| t.kind == TrackKind::Bus;
        let is_drum = |t: &Track| t.device.as_ref().is_some_and(|d| d.source.is_drum_kit());
        let mut groups: Vec<(String, PartKind, Vec<TrackId>)> = Vec::new();
        let drums: Vec<TrackId> = project
            .tracks
            .iter()
            .filter(|t| audible(t) && is_drum(t))
            .map(|t| t.id.clone())
            .collect();
        if !drums.is_empty() {
            groups.push(("Drums".into(), PartKind::Drums, drums));
        }
        for t in project
            .tracks
            .iter()
            .filter(|t| audible(t) && !is_drum(t) && !is_bus(t) && t.kind == TrackKind::Midi)
        {
            let mut ns = notes_of(t);
            if ns.len() < 16 {
                continue;
            }
            ns.sort();
            let overlap =
                ns.windows(2).filter(|w| w[1].0 + 30 < w[0].1).count() as f64 / ns.len() as f64;
            let mut pitches: Vec<u8> = ns.iter().map(|n| n.2).collect();
            pitches.sort();
            let kind = if overlap > 0.2 {
                PartKind::Poly
            } else if pitches[pitches.len() / 2] < 52 {
                PartKind::Bass
            } else {
                PartKind::Melody
            };
            groups.push((t.name.clone(), kind, vec![t.id.clone()]));
        }
        for (pname, kind, ids) in groups {
            let truth: Vec<NoteSpan> = project
                .tracks
                .iter()
                .filter(|t| ids.contains(&t.id))
                .flat_map(notes_of)
                .map(|(a, b, p)| NoteSpan {
                    onset: sec(a),
                    offset: sec(b),
                    pitch: p,
                })
                .collect();
            let device = project
                .tracks
                .iter()
                .find(|t| t.id == ids[0])
                .and_then(|t| t.device.as_ref())
                .map(|d| match &d.source {
                    PluginSource::Builtin { name } => name.clone(),
                    PluginSource::Sf2 { preset, .. } => format!("sf2:{preset}"),
                    _ => "plugin".into(),
                })
                .unwrap_or_default();
            eprintln!("  パート {pname}: {kind:?}・音符 {}", truth.len());
            parts.push(PartCase {
                device,
                kind,
                truth,
            });
        }
    }
    // トラックごとの正解の音符(鳴るトラックすべて)
    let tracks: Vec<(String, Vec<NoteSpan>)> = project
        .tracks
        .iter()
        .filter(|t| audible(t) && t.kind == TrackKind::Midi)
        .map(|t| {
            (
                t.name.clone(),
                notes_of(t)
                    .into_iter()
                    .map(|(a, b, p)| NoteSpan {
                        onset: sec(a),
                        offset: sec(b),
                        pitch: p,
                    })
                    .collect(),
            )
        })
        .collect();
    Ok(Case {
        tracks,
        tags: vec!["sound:project".into(), format!("song:{name}")],
        name,
        frames,
        side: Some(side),
        sample_rate: sr,
        truth: Truth {
            beats,
            downbeats,
            beats_per_bar: (bpb_seen.len() == 1).then(|| bpb_seen[0]),
            key: harmony.key.map(|k| k.name),
            chords,
            tuning_cents: Some(0.0),
            swing_8th_pct: None,
            human: None,
        },
        parts,
    })
}

/// 16 bit のモノラルの wav を書き出す
fn write_wav(path: &Path, frames: &[f32], sr: f32) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: sr as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    if let Ok(mut w) = hound::WavWriter::create(path, spec) {
        for &x in frames {
            w.write_sample((x.clamp(-1.0, 1.0) * 32767.0) as i16).ok();
        }
        w.finalize().ok();
    }
}

// ---- 実在の録音の評価セット ----------------------------------------------

/// Harte の記法(C:maj7 / A:min / G:7/3 / N / X)を Glaux の記号へ。X(分からない)は None
fn harte_to_symbol(label: &str) -> Option<String> {
    let l = label.trim();
    if l == "N" {
        return Some("N".into());
    }
    if l == "X" || l.is_empty() {
        return None;
    }
    let (body, bass) = match l.split_once('/') {
        Some((b, s)) => (b, Some(s)),
        None => (l, None),
    };
    let (root, quality) = match body.split_once(':') {
        Some((r, q)) => (r, q),
        None => (body, "maj"),
    };
    // 括弧の追加音(C:maj(9))は落とす
    let quality = quality.split('(').next().unwrap_or("");
    let q = match quality {
        "" | "maj" => "",
        "min" => "m",
        "7" => "7",
        "maj7" => "maj7",
        "min7" => "m7",
        "dim" => "dim",
        "aug" => "aug",
        "sus4" => "sus4",
        "sus2" => "sus2",
        "hdim7" => "m7b5",
        "dim7" => "dim7",
        "maj6" => "6",
        "min6" => "m6",
        "9" => "9",
        "maj9" => "maj9",
        "min9" => "m9",
        "minmaj7" => "mMaj7",
        "1" | "5" => "5",
        _ => return None,
    };
    let (rpc, _) = chord::parse_pitch_class(root)?;
    let mut s = format!("{}{q}", NAMES[rpc as usize]);
    if let Some(b) = bass {
        let flats = b.matches('b').count() as i32;
        let sharps = b.matches('#').count() as i32;
        let deg: usize = b.trim_matches(|c| c == 'b' || c == '#').parse().ok()?;
        let steps: [i32; 10] = [0, 0, 2, 4, 5, 7, 9, 11, 12, 14];
        let iv = *steps.get(deg)? - flats + sharps;
        if iv.rem_euclid(12) != 0 {
            s = format!("{s}/{}", NAMES[((rpc as i32 + iv).rem_euclid(12)) as usize]);
        }
    }
    Some(s)
}

fn dataset_cases(dir: &Path) -> Vec<Case> {
    let mut out = Vec::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|r| r.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    files.sort();
    for p in files {
        let ext = p
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if !["wav", "mp3", "flac", "ogg", "m4a"].contains(&ext.as_str()) {
            continue;
        }
        let stem = p.with_extension("");
        let read = |e: &str| std::fs::read_to_string(stem.with_extension(e)).ok();
        let mut beats = Vec::new();
        let mut downbeats = Vec::new();
        let mut has_pos = false;
        if let Some(t) = read("beats") {
            for line in t.lines() {
                let mut it = line.split_whitespace();
                let Some(time) = it.next().and_then(|x| x.parse::<f64>().ok()) else {
                    continue;
                };
                beats.push(time);
                if let Some(pos) = it.next().and_then(|x| x.parse::<f64>().ok()) {
                    has_pos = true;
                    if pos == 1.0 {
                        downbeats.push(time);
                    }
                }
            }
        }
        let mut chords = Vec::new();
        if let Some(t) = read("lab") {
            for line in t.lines() {
                let v: Vec<&str> = line.split_whitespace().collect();
                if v.len() < 3 {
                    continue;
                }
                let (Ok(s), Ok(e)) = (v[0].parse::<f64>(), v[1].parse::<f64>()) else {
                    continue;
                };
                if let Some(sym) = harte_to_symbol(v[2]) {
                    chords.push(ChordSpan {
                        start: s,
                        end: e,
                        symbol: sym,
                    });
                }
            }
        }
        let key = read("key").and_then(|t| {
            let t = t
                .trim()
                .replace(':', " ")
                .replace("min", "minor")
                .replace("maj", "major");
            let t = t.replace("minoror", "minor").replace("majoror", "major");
            chord::Key::parse(&t).map(|k| {
                format!(
                    "{} {}",
                    NAMES[k.tonic as usize],
                    if k.minor { "minor" } else { "major" }
                )
            })
        });
        let sound = match glaux_mcp::sound::load_file(&p) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("読めません: {e}");
                continue;
            }
        };
        let bpb = if has_pos && downbeats.len() >= 2 {
            let n = beats
                .iter()
                .filter(|&&b| b >= downbeats[0] && b < downbeats[1])
                .count();
            Some(n as u32)
        } else {
            None
        };
        out.push(Case {
            name: stem
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            tags: vec!["set:dataset".into()],
            frames: sound.frames,
            side: None,
            tracks: Vec::new(),
            sample_rate: sound.sample_rate,
            truth: Truth {
                beats,
                downbeats: if has_pos { downbeats } else { Vec::new() },
                beats_per_bar: bpb,
                key,
                chords,
                tuning_cents: None,
                swing_8th_pct: None,
                human: None,
            },
            parts: Vec::new(),
        });
    }
    out
}

// ---- 評価 --------------------------------------------------------------

#[derive(Clone, Debug, Default, serde::Serialize)]
struct Scores {
    beat_f: Option<f64>,
    downbeat_f: Option<f64>,
    tempo_acc1: Option<f64>,
    tempo_acc2: Option<f64>,
    /// テンポの並びから作った格子の拍の F 値(曲の格子として使えるか)
    grid_f: Option<f64>,
    meter: Option<f64>,
    key: Option<f64>,
    chord_root: Option<f64>,
    chord_majmin: Option<f64>,
    chord_majmin_inv: Option<f64>,
    chord_sevenths: Option<f64>,
    tuning_err_cents: Option<f64>,
    /// 8 分の裏の位置の誤差(% ポイント)
    swing_err: Option<f64>,
    /// 打ち込みか人の演奏かの見分けが合ったか
    human_ok: Option<f64>,
    /// 格子を曲に写したとき、正解の最初の小節頭が 1 小節目の頭(±70ms)に来たか(評価用の曲に弱起は無い)
    bar1_ok: Option<f64>,
}

const METRICS: [&str; 15] = [
    "beat_f",
    "downbeat_f",
    "tempo_acc1",
    "tempo_acc2",
    "grid_f",
    "meter",
    "key",
    "chord_root",
    "chord_majmin",
    "chord_majmin_inv",
    "chord_sevenths",
    "tuning_err_cents",
    "swing_err",
    "human_ok",
    "bar1_ok",
];

impl Scores {
    fn get(&self, m: &str) -> Option<f64> {
        match m {
            "beat_f" => self.beat_f,
            "downbeat_f" => self.downbeat_f,
            "tempo_acc1" => self.tempo_acc1,
            "tempo_acc2" => self.tempo_acc2,
            "grid_f" => self.grid_f,
            "meter" => self.meter,
            "key" => self.key,
            "chord_root" => self.chord_root,
            "chord_majmin" => self.chord_majmin,
            "chord_majmin_inv" => self.chord_majmin_inv,
            "chord_sevenths" => self.chord_sevenths,
            "tuning_err_cents" => self.tuning_err_cents,
            "swing_err" => self.swing_err,
            "human_ok" => self.human_ok,
            "bar1_ok" => self.bar1_ok,
            _ => None,
        }
    }
}

/// 調整用: 推定した小節頭が正解の拍の何拍目に乗ったか(正解の小節頭 = 0)の数
fn down_debug(name: &str, tags: &[String], truth: &Truth, est: &recon::Skeleton) {
    let Some(bpb) = truth.beats_per_bar else {
        return;
    };
    let near = |t: f64| -> Option<usize> {
        let (j, d) = truth
            .beats
            .iter()
            .enumerate()
            .map(|(j, b)| (j, (b - t).abs()))
            .min_by(|a, b| a.1.total_cmp(&b.1))?;
        (d <= 0.07).then_some(j)
    };
    let Some(d0) = truth.downbeats.first().and_then(|&d| near(d)) else {
        return;
    };
    let mut hist = vec![0usize; bpb as usize + 1];
    for &d in &est.downbeats {
        match near(d) {
            Some(j) => hist[(j + bpb as usize * 64 - d0) % bpb as usize] += 1,
            None => hist[bpb as usize] += 1,
        }
    }
    println!(
        "小節頭 {name} {} 拍子 正解 {bpb} 推定 {:?} 何拍目 {:?}(最後は拍に乗らない数)",
        tags.join(","),
        est.beats_per_bar,
        hist
    );
}

fn score(truth: &Truth, est: &recon::Skeleton, frames: &[f32], sr: f32) -> Scores {
    let mut s = Scores::default();
    if let (Some(&d), Some((plan, _))) = (
        truth.downbeats.first(),
        recon::resolve_grid(est, frames, sr, &Default::default()),
    ) {
        // 正解の最初の小節頭の、曲の中の時刻(1 小節目の頭 = 0 秒)
        let secs = plan.audio_start_sec + d;
        s.bar1_ok = Some((secs.abs() <= 0.07) as u8 as f64);
    }
    if !truth.beats.is_empty() {
        s.beat_f = Some(mir_eval::beat_f(&truth.beats, &est.beats).f);
        if let (Some(r), Some(e)) = (truth.bpm(), est.bpm) {
            let (a1, a2) = mir_eval::tempo_accuracy(r, e);
            s.tempo_acc1 = Some(a1 as u8 as f64);
            s.tempo_acc2 = Some(a2 as u8 as f64);
        } else {
            s.tempo_acc1 = Some(0.0);
            s.tempo_acc2 = Some(0.0);
        }
    }
    if !truth.beats.is_empty() {
        let n = est.beats.len();
        let grid = recon::grid_beats(&est.tempo_map, n);
        s.grid_f = Some(mir_eval::beat_f(&truth.beats, &grid).f);
    }
    if !truth.downbeats.is_empty() {
        s.downbeat_f = Some(mir_eval::beat_f(&truth.downbeats, &est.downbeats).f);
    }
    if let Some(b) = truth.beats_per_bar {
        s.meter = Some((est.beats_per_bar == Some(b)) as u8 as f64);
    }
    if let Some(k) = truth.key.as_deref().and_then(chord::Key::parse) {
        s.key = Some(
            est.key
                .as_ref()
                .map(|e| {
                    mir_eval::key_score(
                        k,
                        chord::Key {
                            tonic: e.tonic,
                            minor: e.minor,
                        },
                    )
                })
                .unwrap_or(0.0),
        );
    }
    if !truth.chords.is_empty() {
        s.chord_root = mir_eval::chord_wcsr(&truth.chords, &est.chords, ChordVocab::Root);
        s.chord_majmin = mir_eval::chord_wcsr(&truth.chords, &est.chords, ChordVocab::MajMin);
        s.chord_majmin_inv =
            mir_eval::chord_wcsr(&truth.chords, &est.chords, ChordVocab::MajMinInv);
        s.chord_sevenths = mir_eval::chord_wcsr(&truth.chords, &est.chords, ChordVocab::Sevenths);
    }
    if let Some(sw) = truth.swing_8th_pct {
        let est_sw = est.groove.as_ref().and_then(|g| g.swing_8th_pct);
        s.swing_err = Some(est_sw.map_or(50.0, |e| (e - sw).abs()));
    }
    if let Some(h) = truth.human {
        let spread = est.groove.as_ref().and_then(|g| g.timing_spread_ms);
        s.human_ok = spread.map(|sp| ((sp >= 8.0) == h) as u8 as f64);
    }
    if let Some(c) = truth.tuning_cents {
        s.tuning_err_cents = Some(mir_eval::tuning_error_cents(c, est.tuning_cents));
    }
    s
}

fn mean(v: &[f64]) -> Option<f64> {
    (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
}

fn print_table(rows: &[(String, Vec<&Scores>)]) {
    let short = [
        "beat", "down", "bpm1", "bpm2", "grid", "meter", "key", "root", "majmin", "inv", "7th",
        "tune¢", "swing", "human", "bar1",
    ];
    print!("{:<22}{:>4}", "", "n");
    for s in short {
        print!("{s:>8}");
    }
    println!();
    for (name, list) in rows {
        print!("{name:<22}{:>4}", list.len());
        for m in METRICS {
            let v: Vec<f64> = list.iter().filter_map(|s| s.get(m)).collect();
            match mean(&v) {
                Some(x) if m == "tuning_err_cents" || m == "swing_err" => print!("{x:>8.1}"),
                Some(x) => print!("{x:>8.3}"),
                None => print!("{:>8}", "-"),
            }
        }
        println!();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |name: &str| -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1).cloned())
    };
    let n_cases: usize = arg("--cases").and_then(|v| v.parse().ok()).unwrap_or(24);
    let seed: u64 = arg("--seed").and_then(|v| v.parse().ok()).unwrap_or(1);
    let sf2 = arg("--sf2").map(PathBuf::from);
    let opts = SkeletonOptions {
        beat: match arg("--beat").as_deref() {
            Some("minimal") => BeatMethod::Minimal,
            None | Some("dbn") => BeatMethod::Dbn,
            Some(m) => panic!("拍の求め方が分かりません: {m}"),
        },
        chord: match arg("--chord").as_deref() {
            Some("perbar") => ChordMethod::PerBar,
            None | Some("nnls") => ChordMethod::Nnls,
            Some(m) => panic!("和音の求め方が分かりません: {m}"),
        },
        chord_opts: {
            let mut o = glaux_engine::tonal::ChordOptions::default();
            if let Some(v) = arg("--chord-sharp").and_then(|v| v.parse().ok()) {
                o.sharpness = v;
            }
            if let Some(v) = arg("--chord-bass").and_then(|v| v.parse().ok()) {
                o.bass_weight = v;
            }
            if let Some(v) = arg("--chord-change") {
                let p: Vec<f32> = v.split(',').filter_map(|x| x.parse().ok()).collect();
                if p.len() == 3 {
                    o.change = [p[0], p[1], p[2]];
                }
            }
            if args.iter().any(|a| a == "--triads") {
                o.sevenths = false;
            }
            if args.iter().any(|a| a == "--no-inversions") {
                o.inversions = false;
            }
            o
        },
    };
    let save = arg("--save").map(PathBuf::from);
    let with_parts = args.iter().any(|a| a == "--parts");

    let started = std::time::Instant::now();
    let mut cases: Vec<Case> = Vec::new();
    for i in 0..n_cases {
        // SoundFont があれば半分をそれで鳴らす
        let use_sf2 = sf2.as_deref().filter(|_| i % 2 == 1);
        cases.push(synth_case(i, seed, use_sf2, with_parts));
    }
    if let Some(d) = arg("--dataset") {
        cases.extend(dataset_cases(Path::new(&d)));
    }
    if let Some(d) = arg("--project") {
        // 曲のフォルダ 1 つか、曲のフォルダ(*.glaux)を並べたフォルダ
        let d = PathBuf::from(d);
        let mut dirs: Vec<PathBuf> = if d.join("project.json").is_file() {
            vec![d]
        } else {
            std::fs::read_dir(&d)
                .map(|r| r.filter_map(|e| e.ok().map(|e| e.path())).collect())
                .unwrap_or_default()
        };
        dirs.retain(|p| p.join("project.json").is_file());
        dirs.sort();
        for p in dirs {
            match project_case(&p, with_parts) {
                Ok(c) => cases.push(c),
                Err(e) => eprintln!("{}: 曲を読めません: {e}", p.display()),
            }
        }
    }
    eprintln!(
        "{} 曲を用意({:.1} 秒)",
        cases.len(),
        started.elapsed().as_secs_f64()
    );

    let mut results: Vec<(String, Vec<String>, Scores, recon::Skeleton)> = Vec::new();
    let mut analysis_secs = 0.0;
    let mut audio_secs = 0.0;
    for c in &cases {
        if let Some(dir) = &save {
            std::fs::create_dir_all(dir).ok();
            write_wav(
                &dir.join(format!("{}.wav", c.name)),
                &c.frames,
                c.sample_rate,
            );
            std::fs::write(
                dir.join(format!("{}.json", c.name)),
                serde_json::to_string_pretty(
                    &serde_json::json!({"tags": c.tags, "truth": c.truth}),
                )
                .unwrap_or_default(),
            )
            .ok();
        }
        let t0 = std::time::Instant::now();
        let est = match recon::analyze(&c.frames, c.sample_rate, opts.clone()) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("{}: 解析できません: {e}", c.name);
                continue;
            }
        };
        analysis_secs += t0.elapsed().as_secs_f64();
        audio_secs += c.frames.len() as f64 / c.sample_rate as f64;
        let s = score(&c.truth, &est, &c.frames, c.sample_rate);
        if std::env::var("RECON_DOWN_DEBUG").is_ok() {
            down_debug(&c.name, &c.tags, &c.truth, &est);
        }
        results.push((c.name.clone(), c.tags.clone(), s, est));
    }

    // 曲ごと
    println!("== 曲ごと ==");
    let rows: Vec<(String, Vec<&Scores>)> = results
        .iter()
        .map(|(n, tags, s, _)| {
            let short: Vec<&str> = tags
                .iter()
                .filter_map(|t| t.split_once(':').map(|x| x.1))
                .collect();
            (
                format!("{n} {}", short.join(","))
                    .chars()
                    .take(21)
                    .collect(),
                vec![s],
            )
        })
        .collect();
    print_table(&rows);

    // 分類ごと
    println!("\n== 分類ごと ==");
    let mut groups: BTreeMap<String, Vec<&Scores>> = BTreeMap::new();
    for (_, tags, s, _) in &results {
        groups.entry("all".into()).or_default().push(s);
        for t in tags {
            groups.entry(t.clone()).or_default().push(s);
        }
    }
    let rows: Vec<(String, Vec<&Scores>)> = groups.into_iter().collect();
    print_table(&rows);
    println!(
        "\n解析の速さ: 音声 {audio_secs:.0} 秒を {analysis_secs:.1} 秒(実時間の {:.1}%)",
        analysis_secs / audio_secs.max(1e-9) * 100.0
    );

    if let (Some(labels), Some(sep)) = (arg("--labels"), arg("--separate")) {
        eval_instruments(
            &cases,
            Path::new(&sep),
            Path::new(&labels),
            arg("--protos").as_deref().map(Path::new),
        );
    }
    if with_parts {
        eval_parts(
            &cases,
            arg("--separate").as_deref().map(Path::new),
            arg("--notes").as_deref().map(Path::new),
        );
    }

    if let Some(path) = arg("--json") {
        let v: Vec<serde_json::Value> = results
            .iter()
            .map(|(n, tags, s, est)| serde_json::json!({"name": n, "tags": tags, "scores": s, "estimate": est}))
            .collect();
        std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap_or_default()).ok();
    }
}

/// 楽器の見分けを測る: 区間ごとに、正解のトラックが鳴っている種類と、推定した種類を比べる。
/// 方式は 2 つ: 言葉(音色語の辞書との近さの z 値がしきい値を超える)と、見本(--protos。GM の音色で鳴らしたフレーズの埋め込みに、
/// 区間がいちばん近い種類から差がしきい値以内の種類)
fn eval_instruments(cases: &[Case], sep: &Path, labels: &Path, protos: Option<&Path>) {
    use glaux_mcp::instruments::{
        prototype_scores, window_embeddings, word_scores, InstrumentClass as C, Prototype,
        WindowEmbedding, WindowScores, CLASSES,
    };
    let labels: serde_json::Value = std::fs::read_to_string(labels)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let protos: Vec<Prototype> = protos
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let class_of = |s: &str| -> Option<C> { serde_json::from_value(serde_json::json!(s)).ok() };
    // 分けた「その他」から当てる種類(歌は別に「歌」から)
    let from_other = [
        C::Pad,
        C::Strings,
        C::Piano,
        C::Epiano,
        C::Organ,
        C::Pluck,
        C::Bell,
        C::Lead,
        C::Guitar,
        C::Brass,
        C::Wind,
        C::Fx,
    ];
    let word_th = [0.0f32, 1.0, 2.0, 3.0];
    let proto_th = [0.0f32, 0.01, 0.02, 0.03, 0.05, 0.08];
    // (方式, 種類, 音の素, しきい値の番号) → (当たり, 推定, 正解)
    type Counts = BTreeMap<(&'static str, C, &'static str, usize), (usize, usize, usize)>;
    let mut counts: Counts = BTreeMap::new();
    let mut song_rows: Vec<String> = Vec::new();
    for c in cases {
        let Some(lab) = labels.get(&c.name) else {
            continue;
        };
        let work = sep.join(format!("{}-{}", c.name, c.frames.len()));
        // 区間の埋め込み(分けた音の置き場所に残して使い回す)
        let emb = |src: &str| -> Option<Vec<WindowEmbedding>> {
            let path = work.join(format!("emb-{src}.json"));
            if let Some(v) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
            {
                return Some(v);
            }
            let (frames, sr) = if src == "mix" {
                (c.frames.clone(), c.sample_rate)
            } else {
                let d = glaux_engine::load_wav_mono(&work.join(format!("{src}.wav"))).ok()?;
                (d.frames, d.sample_rate)
            };
            let v = window_embeddings(&frames, sr).ok()?;
            std::fs::write(&path, serde_json::to_string(&v).ok()?).ok();
            Some(v)
        };
        let (Some(e_other), Some(e_vocals), Some(e_mix)) =
            (emb("other"), emb("vocals"), emb("mix"))
        else {
            eprintln!(
                "{}: 分けた音か埋め込みがありません(先に --separate で分ける)",
                c.name
            );
            continue;
        };
        // 正解: 区間に、その種類のトラックの音が合わせて 1 秒以上鳴っている
        let active = |cls: C, a: f64, b: f64| -> bool {
            let mut secs = 0.0;
            for (name, notes) in &c.tracks {
                if lab.get(name).and_then(|v| v.as_str()).and_then(class_of) != Some(cls) {
                    continue;
                }
                for n in notes {
                    secs += (n.offset.min(b) - n.onset.max(a)).max(0.0);
                }
            }
            secs >= 1.0
        };
        let present: Vec<&str> = from_other
            .iter()
            .copied()
            .chain([C::Voice])
            .filter(|cls| {
                c.tracks.iter().any(|(name, _)| {
                    lab.get(name).and_then(|v| v.as_str()).and_then(class_of) == Some(*cls)
                })
            })
            .map(|cls| cls.ja())
            .collect();
        // 推定: 方式ごとに、区間ごとの「鳴っている」と見なす種類
        type Decide<'a> = Box<dyn Fn(&WindowScores, C, f32) -> bool + 'a>;
        let by_word: Decide = Box::new(|w, cls, th| w.score(cls) > th);
        let by_proto: Decide = Box::new(|w, cls, th| {
            let best = from_other
                .iter()
                .map(|&k| w.score(k))
                .fold(f32::MIN, f32::max);
            best > f32::MIN && w.score(cls) >= best - th
        });
        // (方式, 「その他」の点数, 曲全体の点数, 決め方, しきい値)
        type Method<'a, 'b> = (
            &'static str,
            Vec<WindowScores>,
            Vec<WindowScores>,
            &'a Decide<'b>,
            &'a [f32],
        );
        let mut methods: Vec<Method> = vec![(
            "言葉",
            word_scores(&e_other),
            word_scores(&e_mix),
            &by_word,
            &word_th[..],
        )];
        if !protos.is_empty() {
            methods.push((
                "見本",
                prototype_scores(&e_other, &protos),
                prototype_scores(&e_mix, &protos),
                &by_proto,
                &proto_th[..],
            ));
        }
        for (m, w_other, w_mix, decide, ths) in &methods {
            // 曲ごとに、推定した区間の割合が多い種類(中ほどのしきい値で)
            let th_mid = ths[ths.len() / 2];
            let mut top: Vec<(f32, &str)> = from_other
                .iter()
                .map(|&cls| {
                    let n = w_other.iter().filter(|w| decide(w, cls, th_mid)).count();
                    (n as f32 / w_other.len().max(1) as f32, cls.ja())
                })
                .collect();
            top.sort_by(|a, b| b.0.total_cmp(&a.0));
            song_rows.push(format!(
                "{} [{m}]: 正解 [{}] / 推定の多い順 [{}]",
                c.name,
                present.join("・"),
                top.iter()
                    .filter(|x| x.0 > 0.0)
                    .take(6)
                    .map(|(r, n)| format!("{n} {:.0}%", r * 100.0))
                    .collect::<Vec<_>>()
                    .join("・")
            ));
            for (src, ws) in [("その他", w_other), ("曲全体", w_mix)] {
                for w in ws.iter() {
                    for &cls in &from_other {
                        let t = active(cls, w.start, w.end);
                        for (k, th) in ths.iter().enumerate() {
                            let e = decide(w, cls, *th);
                            let s = counts.entry((m, cls, src, k)).or_default();
                            s.0 += (t && e) as usize;
                            s.1 += e as usize;
                            s.2 += t as usize;
                        }
                    }
                }
            }
        }
        // 声: 分けた「歌」から(言葉で)
        for w in word_scores(&e_vocals) {
            let t = active(C::Voice, w.start, w.end);
            for (k, th) in word_th.iter().enumerate() {
                let e = w.score(C::Voice) > *th;
                let s = counts.entry(("言葉", C::Voice, "歌", k)).or_default();
                s.0 += (t && e) as usize;
                s.1 += e as usize;
                s.2 += t as usize;
            }
        }
    }
    println!("\n== 楽器の見分け(区間ごと)==");
    for r in &song_rows {
        println!("{r}");
    }
    for (m, ths) in [("言葉", &word_th[..]), ("見本", &proto_th[..])] {
        println!("\n-- {m}(F 値 / 推定の区間の数。しきい値ごと)--");
        print!("{:<16}{:<8}{:>6}", "種類", "音の素", "正解");
        for th in ths {
            print!("{th:>13.2}");
        }
        println!();
        for (cls, _, _) in CLASSES.iter() {
            for src in ["その他", "曲全体", "歌"] {
                let Some(&(_, _, n_true)) = counts.get(&(m, *cls, src, 0)) else {
                    continue;
                };
                print!("{:<16}{:<8}{:>6}", cls.ja(), src, n_true);
                for k in 0..ths.len() {
                    let (hit, est, tru) = counts[&(m, *cls, src, k)];
                    let f = if est + tru == 0 {
                        0.0
                    } else {
                        2.0 * hit as f64 / (est + tru) as f64
                    };
                    print!("{:>8.2}/{est:<4}", f);
                }
                println!();
            }
        }
    }
}

/// パートの種類の表の名前(評価用の曲のトラックの名前と同じ)
fn kind_label(k: &recon::PartKind) -> String {
    match k {
        recon::PartKind::Melody => "Lead",
        recon::PartKind::Bass => "Bass",
        recon::PartKind::Poly => "Chords",
        recon::PartKind::Drums => "Drums",
    }
    .to_owned()
}

/// ドラムの種類と GM の番号
const DRUM_CLASSES: [(&str, &[u8]); 4] = [
    ("kick", &[35, 36]),
    ("snare", &[37, 38, 39, 40]),
    ("hihat", &[42, 44, 46]),
    ("cymbal", &[49, 51, 52, 55, 57, 59]),
];

/// パートの譜起こしを、曲そのもの(作業場の「参考曲そのまま」)から起こして測る。
/// トラックごとに書き出した音では測らない(`PartCase` の説明)
fn eval_parts(cases: &[Case], separate: Option<&Path>, ext_notes: Option<&Path>) {
    use recon::{NoteMethod, PartKind};
    // (パート, 方式) → (立ち上がりの F 値, 終わりも見た F 値) の並び
    let mut rows: BTreeMap<(String, String), Vec<(f64, f64)>> = BTreeMap::new();
    let to_spans = |v: &[recon::RawNote]| -> Vec<NoteSpan> {
        v.iter()
            .map(|n| NoteSpan {
                onset: n.start,
                offset: n.end,
                pitch: n.pitch,
            })
            .collect()
    };
    let mut secs = 0.0;
    for c in cases {
        if c.parts.is_empty() {
            continue;
        }
        let t0 = std::time::Instant::now();
        let tuning = glaux_engine::tonal::estimate_tuning(&c.frames, c.sample_rate);
        // 集計の切り口: 全体と、曲の分類(音源・曲名など)ごと
        let keys = |label: &str| -> Vec<String> {
            std::iter::once(format!("{label} all"))
                .chain(c.tags.iter().map(|t| format!("{label} {t}")))
                .collect()
        };
        for p in c.parts.iter().filter(|p| p.kind == PartKind::Drums) {
            // 調整用: 曲そのものとドラムの正解を書き出す(RECON_DRUMS=フォルダ。drum_tune で使う)
            if let Ok(dir) = std::env::var("RECON_DRUMS") {
                let dir = std::path::Path::new(&dir);
                std::fs::create_dir_all(dir).ok();
                let spec = hound::WavSpec {
                    channels: 1,
                    sample_rate: c.sample_rate as u32,
                    bits_per_sample: 32,
                    sample_format: hound::SampleFormat::Float,
                };
                if let Ok(mut w) =
                    hound::WavWriter::create(dir.join(format!("{}.wav", c.name)), spec)
                {
                    for &x in &c.frames {
                        w.write_sample(x).ok();
                    }
                    w.finalize().ok();
                }
                std::fs::write(
                    dir.join(format!("{}.json", c.name)),
                    serde_json::json!({"truth": p.truth, "device": p.device}).to_string(),
                )
                .ok();
            }
            // ドラム: 種類ごと(キック・スネア・ハイハット・シンバル)の立ち上がりの F 値
            let Ok(est) = recon::transcribe_part(
                &c.frames,
                c.sample_rate,
                PartKind::Drums,
                NoteMethod::DrumsMix,
                0.0,
            ) else {
                continue;
            };
            for (class, pitches) in DRUM_CLASSES {
                let mut t: Vec<f64> = p
                    .truth
                    .iter()
                    .filter(|n| pitches.contains(&n.pitch))
                    .map(|n| n.onset)
                    .collect();
                if t.is_empty() {
                    continue;
                }
                let mut e: Vec<f64> = est
                    .iter()
                    .filter(|n| pitches.contains(&n.pitch))
                    .map(|n| n.start)
                    .collect();
                e.sort_by(f64::total_cmp);
                t.sort_by(f64::total_cmp);
                let hit = mir_eval::match_events(&t, &e, 0.05);
                let f = 2.0 * hit as f64 / (t.len() + e.len()) as f64;
                for key in keys(&format!("Drums {class}")) {
                    rows.entry((key, "FromMix".into()))
                        .or_default()
                        .push((f, f));
                }
            }
        }
        // 音程のあるパート: 1 回の譜起こしを声部に分ける(作業場の「参考曲そのまま」)。
        // 種類(旋律・ベース・和音)ごとに 1 つ。同じ種類のトラックが複数あれば正解をまとめる
        let mut kinds: Vec<PartKind> = Vec::new();
        for p in c.parts.iter().filter(|p| p.kind != PartKind::Drums) {
            if !kinds.contains(&p.kind) {
                kinds.push(p.kind);
            }
        }
        let from_mix = recon::transcribe_mix_parts(&c.frames, c.sample_rate, &kinds, tuning)
            .unwrap_or_default();
        for (k, est) in kinds.iter().zip(&from_mix) {
            let same: Vec<&PartCase> = c.parts.iter().filter(|p| p.kind == *k).collect();
            let label = kind_label(k);
            let truth: Vec<NoteSpan> = same.iter().flat_map(|p| p.truth.iter().copied()).collect();
            let est = to_spans(est);
            if let Ok(dir) = std::env::var("RECON_DUMP") {
                let path = std::path::Path::new(&dir).join(format!("{}-{label}.json", c.name));
                std::fs::write(
                    path,
                    serde_json::json!({"truth": truth, "est": est, "tags": c.tags}).to_string(),
                )
                .ok();
            }
            let f1 = mir_eval::note_f(&truth, &est, false).f;
            let f2 = mir_eval::note_f(&truth, &est, true).f;
            for key in keys(&label) {
                rows.entry((key, "FromMix".into()))
                    .or_default()
                    .push((f1, f2));
            }
        }
        // 上の声部(旋律 + 和音)をまとめて(旋律と和音の分け方に左右されない比べ)
        let upper_truth: Vec<NoteSpan> = c
            .parts
            .iter()
            .filter(|p| matches!(p.kind, PartKind::Melody | PartKind::Poly))
            .flat_map(|p| p.truth.iter().copied())
            .collect();
        let upper_of = |est: &[Vec<recon::RawNote>]| -> Vec<NoteSpan> {
            kinds
                .iter()
                .zip(est)
                .filter(|(k, _)| matches!(k, PartKind::Melody | PartKind::Poly))
                .flat_map(|(_, v)| to_spans(v))
                .collect()
        };
        if !upper_truth.is_empty() {
            let est = upper_of(&from_mix);
            let f1 = mir_eval::note_f(&upper_truth, &est, false).f;
            let f2 = mir_eval::note_f(&upper_truth, &est, true).f;
            for key in keys("Upper") {
                rows.entry((key, "FromMix".into()))
                    .or_default()
                    .push((f1, f2));
            }
            // 和声の一致(鳴っている音名)。2 つの列に同じ値を入れる
            let h = mir_eval::chroma_f(&upper_truth, &est).f;
            for key in keys("Upper(和声)") {
                rows.entry((key, "FromMix".into()))
                    .or_default()
                    .push((h, h));
            }
            // 推定した和音から伴奏を組み立てたとき(音符の一致と和声の一致)
            if let Ok(sk) = recon::analyze(&c.frames, c.sample_rate, Default::default()) {
                let est = to_spans(&recon::chord_part_notes(&sk));
                let f1 = mir_eval::note_f(&upper_truth, &est, false).f;
                let f2 = mir_eval::note_f(&upper_truth, &est, true).f;
                for key in keys("Upper") {
                    rows.entry((key, "FromChords".into()))
                        .or_default()
                        .push((f1, f2));
                }
                let h = mir_eval::chroma_f(&upper_truth, &est).f;
                for key in keys("Upper(和声)") {
                    rows.entry((key, "FromChords".into()))
                        .or_default()
                        .push((h, h));
                }
            }
        }
        // ほかの道具が曲全体から起こした音符(--notes)
        if let Some(dir) = ext_notes {
            let path = dir.join(format!("{}.json", c.name));
            let ext: Vec<serde_json::Value> = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default();
            if !ext.is_empty() {
                let note = |v: &serde_json::Value| recon::RawNote {
                    start: v["onset"].as_f64().unwrap_or(0.0),
                    end: v["offset"].as_f64().unwrap_or(0.0),
                    pitch: v["pitch"].as_u64().unwrap_or(0) as u8,
                    vel: v["velocity"].as_u64().unwrap_or(100) as u8,
                };
                let pitched: Vec<recon::RawNote> = ext
                    .iter()
                    .filter(|v| !v["is_drum"].as_bool().unwrap_or(false))
                    .map(note)
                    .collect();
                let split = recon::split_voices(&pitched, &kinds);
                let mut score = |label: String, truth: &[NoteSpan], est: &[NoteSpan], m: &str| {
                    let f1 = mir_eval::note_f(truth, est, false).f;
                    let f2 = mir_eval::note_f(truth, est, true).f;
                    for key in keys(&label) {
                        rows.entry((key, m.to_owned())).or_default().push((f1, f2));
                    }
                };
                for (k, est) in kinds.iter().zip(&split) {
                    let truth: Vec<NoteSpan> = c
                        .parts
                        .iter()
                        .filter(|p| p.kind == *k)
                        .flat_map(|p| p.truth.iter().copied())
                        .collect();
                    score(kind_label(k), &truth, &to_spans(est), "Ext");
                }
                // 和声の一致は score と同じ表に入れるので、ここでは値だけ求めて最後に入れる
                let mut ext_harmony = None;
                if !upper_truth.is_empty() {
                    let est = upper_of(&split);
                    score("Upper".into(), &upper_truth, &est, "Ext");
                    ext_harmony = Some(mir_eval::chroma_f(&upper_truth, &est).f);
                }
                // ベースを楽器の番号(32〜39)で選んだとき
                let bass_truth: Vec<NoteSpan> = c
                    .parts
                    .iter()
                    .filter(|p| p.kind == PartKind::Bass)
                    .flat_map(|p| p.truth.iter().copied())
                    .collect();
                if !bass_truth.is_empty() {
                    let est: Vec<recon::RawNote> = ext
                        .iter()
                        .filter(|v| {
                            !v["is_drum"].as_bool().unwrap_or(false)
                                && (32..40).contains(&v["program"].as_u64().unwrap_or(0))
                        })
                        .map(note)
                        .collect();
                    score("Bass".into(), &bass_truth, &to_spans(&est), "Ext(楽器)");
                }
                let all_truth: Vec<NoteSpan> = c
                    .parts
                    .iter()
                    .filter(|p| p.kind != PartKind::Drums)
                    .flat_map(|p| p.truth.iter().copied())
                    .collect();
                score("Mix".into(), &all_truth, &to_spans(&pitched), "Ext");
                if let Some(h) = ext_harmony {
                    for key in keys("Upper(和声)") {
                        rows.entry((key, "Ext".into())).or_default().push((h, h));
                    }
                }
                // ドラム
                if let Some(p) = c.parts.iter().find(|p| p.kind == PartKind::Drums) {
                    for (class, pitches) in DRUM_CLASSES {
                        let mut t: Vec<f64> = p
                            .truth
                            .iter()
                            .filter(|n| pitches.contains(&n.pitch))
                            .map(|n| n.onset)
                            .collect();
                        if t.is_empty() {
                            continue;
                        }
                        let mut e: Vec<f64> = ext
                            .iter()
                            .filter(|v| {
                                v["is_drum"].as_bool().unwrap_or(false)
                                    && pitches.contains(&(v["pitch"].as_u64().unwrap_or(0) as u8))
                            })
                            .map(|v| v["onset"].as_f64().unwrap_or(0.0))
                            .collect();
                        e.sort_by(f64::total_cmp);
                        t.sort_by(f64::total_cmp);
                        let hit = mir_eval::match_events(&t, &e, 0.05);
                        let f = 2.0 * hit as f64 / (t.len() + e.len()) as f64;
                        for key in keys(&format!("Drums {class}")) {
                            rows.entry((key, "Ext".into())).or_default().push((f, f));
                        }
                    }
                }
            }
        }
        // 音程のあるパートを全部まとめて
        let truth: Vec<NoteSpan> = c
            .parts
            .iter()
            .filter(|p| p.kind != PartKind::Drums)
            .flat_map(|p| p.truth.iter().copied())
            .collect();
        if let Ok(est) = recon::transcribe_part(
            &c.frames,
            c.sample_rate,
            PartKind::Poly,
            NoteMethod::BasicPitch,
            tuning,
        ) {
            let est = to_spans(&est);
            let f1 = mir_eval::note_f(&truth, &est, false).f;
            let f2 = mir_eval::note_f(&truth, &est, true).f;
            for key in keys("Mix") {
                rows.entry((key, "BasicPitch".into()))
                    .or_default()
                    .push((f1, f2));
            }
        }
        // 曲から分離ツールで分けた音から起こす(入力は曲だけから作られているのでよい)
        if let Some(cache) = separate {
            let work = cache.join(format!("{}-{}", c.name, c.frames.len()));
            let names = ["vocals", "drums", "bass", "other"];
            let stems: Option<Vec<Vec<f32>>> = if names
                .iter()
                .all(|n| work.join(format!("{n}.wav")).is_file())
            {
                names
                    .iter()
                    .map(|n| {
                        glaux_engine::load_wav_mono(&work.join(format!("{n}.wav")))
                            .ok()
                            .map(|d| d.frames)
                    })
                    .collect()
            } else {
                match glaux_mcp::stems::separate_mid(
                    &c.frames,
                    c.side.as_deref(),
                    c.sample_rate,
                    &work,
                ) {
                    Ok(v) => {
                        for (n, m) in &v {
                            write_wav(&work.join(format!("{n}.wav")), m, c.sample_rate);
                        }
                        let _ = std::fs::remove_dir_all(work.join("demucs_out"));
                        Some(v.into_iter().map(|x| x.1).collect())
                    }
                    Err(e) => {
                        eprintln!("{}: 分けられません: {e}", c.name);
                        None
                    }
                }
            };
            if let Some(stems) = stems {
                let stem = |n: &str| &stems[names.iter().position(|x| *x == n).unwrap_or(3)];
                // (種類, 分けた音, 名前): 作業場と同じ割り当て(旋律は歌、楽器だけの曲なら「その他」)と、
                // 旋律を歌・「その他」のどちらかに決めて起こした場合
                let melody_src = if recon::melody_from_other(stem("vocals"), stem("other")) {
                    "other"
                } else {
                    "vocals"
                };
                let mut jobs: Vec<(PartKind, &str, String)> = Vec::new();
                for k in &kinds {
                    let src = match k {
                        PartKind::Melody => melody_src,
                        PartKind::Bass => "bass",
                        _ => "other",
                    };
                    jobs.push((*k, src, "Sep".into()));
                    if *k == PartKind::Melody {
                        jobs.push((*k, "vocals", "Sep(歌)".into()));
                        jobs.push((*k, "other", "Sep(その他)".into()));
                    }
                }
                for (k, src, method) in jobs {
                    let same: Vec<&PartCase> = c.parts.iter().filter(|p| p.kind == k).collect();
                    let label = kind_label(&k);
                    let truth: Vec<NoteSpan> =
                        same.iter().flat_map(|p| p.truth.iter().copied()).collect();
                    let Ok(est) = recon::transcribe_part(
                        stem(src),
                        c.sample_rate,
                        k,
                        k.default_method(),
                        tuning,
                    ) else {
                        continue;
                    };
                    let est = to_spans(&est);
                    let f1 = mir_eval::note_f(&truth, &est, false).f;
                    let f2 = mir_eval::note_f(&truth, &est, true).f;
                    for key in keys(&label) {
                        rows.entry((key, method.clone()))
                            .or_default()
                            .push((f1, f2));
                    }
                    if k == PartKind::Poly && method == "Sep" && !upper_truth.is_empty() {
                        let h = mir_eval::chroma_f(&upper_truth, &est).f;
                        for key in keys("Upper(和声)") {
                            rows.entry((key, "Sep".into())).or_default().push((h, h));
                        }
                    }
                }
                // ドラム
                if let Some(p) = c.parts.iter().find(|p| p.kind == PartKind::Drums) {
                    if let Ok(est) = recon::transcribe_part(
                        stem("drums"),
                        c.sample_rate,
                        PartKind::Drums,
                        NoteMethod::Drums,
                        0.0,
                    ) {
                        for (class, pitches) in DRUM_CLASSES {
                            let mut t: Vec<f64> = p
                                .truth
                                .iter()
                                .filter(|n| pitches.contains(&n.pitch))
                                .map(|n| n.onset)
                                .collect();
                            if t.is_empty() {
                                continue;
                            }
                            let mut e: Vec<f64> = est
                                .iter()
                                .filter(|n| pitches.contains(&n.pitch))
                                .map(|n| n.start)
                                .collect();
                            e.sort_by(f64::total_cmp);
                            t.sort_by(f64::total_cmp);
                            let hit = mir_eval::match_events(&t, &e, 0.05);
                            let f = 2.0 * hit as f64 / (t.len() + e.len()) as f64;
                            for key in keys(&format!("Drums {class}")) {
                                rows.entry((key, "Sep".into())).or_default().push((f, f));
                            }
                        }
                    }
                }
            }
        }
        secs += t0.elapsed().as_secs_f64();
    }
    println!("\n== パートの譜起こし(音符の F 値: 立ち上がり / 終わりも)==");
    println!(
        "{:<22}{:<12}{:>4}{:>9}{:>9}",
        "パート", "方式", "n", "onset", "offset"
    );
    for ((part, m), v) in &rows {
        let n = v.len() as f64;
        let a = v.iter().map(|x| x.0).sum::<f64>() / n;
        let b = v.iter().map(|x| x.1).sum::<f64>() / n;
        println!("{part:<22}{m:<12}{:>4}{a:>9.3}{b:>9.3}", v.len());
    }
    println!("譜起こしにかかった時間: {secs:.1} 秒");
}
