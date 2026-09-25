//! MIDI ファイル(.mid)の読み込みと書き出し(アプリと MCP の import_midi / export_midi で共通)。
//!
//! 読み込み: テンポ・拍子・マーカー・トラック(チャンネルごとに 1 本)・ノート・最初の音色(GM プログラム)・
//! 音量(CC7)とパン(CC10)を移す。分解能は 960 に換算する。音源は GM の分類から内蔵の楽器を選ぶ
//! (SoundFont を指定すればそのプリセット)。全体で 1 件の履歴。
//! 書き出し: SMF 1(トラックごと)。ループのクリップは展開する。ピッチカーブ・奏法・オートメーションは移さない。

use glaux_core::{
    arrange, Clip, ClipId, Command, Device, Note, NoteId, PluginSource, Project, SectionMarker,
    TempoEvent, Tick, TimeSigEvent, Track, TrackId, TrackKind, MAX_TICK, PPQ,
};
use midly::num::{u15, u24, u28, u4, u7};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// ノート: (開始, 長さ か 終了, 音高, ベロシティ)
type RawNote = (u64, u64, u8, u8);

/// ドラムのチャンネル(0 始まり。GM の 10ch)
const DRUM_CH: u8 = 9;

/// 読み込みの依頼。
#[derive(Clone, Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct ImportMidiRequest {
    /// .mid ファイルの絶対パス
    pub path: String,
    /// 置く位置(tick)。省略で曲の頭
    #[serde(default)]
    pub start_tick: Option<u64>,
    /// ファイルのテンポと拍子をプロジェクトに使うか。省略時は、プロジェクトにまだクリップが無いときだけ使う
    #[serde(default)]
    pub set_tempo: Option<bool>,
    /// 音源に使う SoundFont(list_soundfonts のファイル名)。省略で内蔵の楽器(GM の分類で選ぶ)
    #[serde(default)]
    pub soundfont: Option<String>,
}

/// 読み込んだ 1 パート(元のトラック × チャンネル)。tick は 960 換算済み
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Part {
    pub name: String,
    pub channel: u8,
    pub program: u8,
    /// (開始, 長さ, 音高, ベロシティ)
    pub notes: Vec<RawNote>,
    pub volume: Option<u8>,
    pub pan: Option<u8>,
}

impl Part {
    pub fn is_drum(&self) -> bool {
        self.channel == DRUM_CH
    }
}

/// MIDI ファイルの中身(Glaux に移せる部分)。
#[derive(Clone, Debug, Default)]
pub struct MidiSong {
    pub tempos: Vec<TempoEvent>,
    pub sigs: Vec<TimeSigEvent>,
    pub markers: Vec<SectionMarker>,
    pub parts: Vec<Part>,
}

#[derive(Default)]
struct PartBuilder {
    program: Option<u8>,
    notes: Vec<RawNote>,
    volume: Option<u8>,
    pan: Option<u8>,
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).trim().to_owned()
}

/// SMF を読む。
pub fn parse(bytes: &[u8]) -> Result<MidiSong, String> {
    let smf = Smf::parse(bytes).map_err(|e| format!("MIDI ファイルを読めません: {e}"))?;
    let ppq = match smf.header.timing {
        Timing::Metrical(t) => t.as_int() as u64,
        Timing::Timecode(..) => {
            return Err("SMPTE の時間で書かれた MIDI ファイルには対応していません".to_owned())
        }
    };
    if ppq == 0 {
        return Err("分解能が 0 の MIDI ファイルです".to_owned());
    }
    let conv = |t: u64| ((t as u128 * PPQ as u128 + ppq as u128 / 2) / ppq as u128) as u64;

    let mut song = MidiSong::default();
    // チャンネルごとの最初の音色(トラックをまたいで使う。音色を別のトラックに置くファイルがある)
    let mut first_program: BTreeMap<u8, u8> = BTreeMap::new();
    let mut per_track: Vec<(Option<String>, BTreeMap<u8, PartBuilder>)> = Vec::new();
    for track in &smf.tracks {
        let mut abs = 0u64;
        let mut name = None;
        let mut builders: BTreeMap<u8, PartBuilder> = BTreeMap::new();
        let mut open: BTreeMap<(u8, u8), Vec<(u64, u8)>> = BTreeMap::new();
        for ev in track {
            abs += ev.delta.as_int() as u64;
            match ev.kind {
                TrackEventKind::Meta(MetaMessage::Tempo(us)) if us.as_int() > 0 => {
                    song.tempos.push(TempoEvent {
                        tick: Tick(conv(abs)),
                        // 4 分音符のマイクロ秒なので、90 が 89.99995 のようになる。小数 3 桁に丸める
                        bpm: (60_000_000_000.0 / us.as_int() as f64).round() / 1000.0,
                    });
                }
                TrackEventKind::Meta(MetaMessage::TimeSignature(num, pow, _, _))
                    if num > 0 && pow <= 6 =>
                {
                    song.sigs.push(TimeSigEvent {
                        tick: Tick(conv(abs)),
                        num,
                        den: 1 << pow,
                    });
                }
                TrackEventKind::Meta(MetaMessage::Marker(b)) if !text(b).is_empty() => {
                    song.markers.push(SectionMarker {
                        tick: Tick(conv(abs)),
                        name: text(b),
                    });
                }
                TrackEventKind::Meta(MetaMessage::TrackName(b))
                    if name.is_none() && !text(b).is_empty() =>
                {
                    name = Some(text(b));
                }
                TrackEventKind::Midi { channel, message } => {
                    let ch = channel.as_int();
                    match message {
                        MidiMessage::NoteOn { key, vel } if vel.as_int() > 0 => {
                            builders.entry(ch).or_default();
                            open.entry((ch, key.as_int()))
                                .or_default()
                                .push((abs, vel.as_int()));
                        }
                        MidiMessage::NoteOn { key, .. } | MidiMessage::NoteOff { key, .. } => {
                            let k = key.as_int();
                            if let Some(stack) = open.get_mut(&(ch, k)) {
                                if !stack.is_empty() {
                                    let (s, v) = stack.remove(0);
                                    builders.entry(ch).or_default().notes.push((s, abs, k, v));
                                }
                            }
                        }
                        MidiMessage::ProgramChange { program } => {
                            let p = program.as_int();
                            first_program.entry(ch).or_insert(p);
                            let b = builders.entry(ch).or_default();
                            if b.program.is_none() {
                                b.program = Some(p);
                            }
                        }
                        MidiMessage::Controller { controller, value } => {
                            let b = builders.entry(ch).or_default();
                            match controller.as_int() {
                                7 if b.volume.is_none() => b.volume = Some(value.as_int()),
                                10 if b.pan.is_none() => b.pan = Some(value.as_int()),
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        // 終わりの無いノートはトラックの終わりまで
        for ((ch, k), stack) in open {
            for (s, v) in stack {
                builders.entry(ch).or_default().notes.push((s, abs, k, v));
            }
        }
        per_track.push((name, builders));
    }

    for (name, builders) in per_track {
        let with_notes: Vec<(u8, PartBuilder)> = builders
            .into_iter()
            .filter(|(_, b)| !b.notes.is_empty())
            .collect();
        let several = with_notes.len() > 1;
        for (ch, b) in with_notes {
            let program = b.program.or(first_program.get(&ch).copied()).unwrap_or(0);
            let inst = if ch == DRUM_CH {
                "Drums"
            } else {
                GM_NAMES[program as usize & 127]
            };
            let name = match &name {
                Some(n) if several => format!("{n} {inst}"),
                Some(n) => n.clone(),
                None => inst.to_owned(),
            };
            let mut notes: Vec<RawNote> = b
                .notes
                .into_iter()
                .map(|(s, e, k, v)| {
                    let (s, e) = (conv(s), conv(e));
                    (s, e.saturating_sub(s).max(1), k, v.clamp(1, 127))
                })
                .collect();
            notes.sort_unstable();
            song.parts.push(Part {
                name,
                channel: ch,
                program,
                notes,
                volume: b.volume,
                pan: b.pan,
            });
        }
    }

    // テンポ・拍子は同じ位置なら後のものを使い、頭に無ければ既定(120 / 4/4)を置く
    song.tempos.sort_by_key(|e| e.tick);
    song.tempos.reverse();
    song.tempos.dedup_by_key(|e| e.tick);
    song.tempos.reverse();
    if song.tempos.first().is_none_or(|e| e.tick != Tick::ZERO) {
        song.tempos.insert(
            0,
            TempoEvent {
                tick: Tick::ZERO,
                bpm: 120.0,
            },
        );
    }
    song.sigs.sort_by_key(|e| e.tick);
    song.sigs.reverse();
    song.sigs.dedup_by_key(|e| e.tick);
    song.sigs.reverse();
    if song.sigs.first().is_none_or(|e| e.tick != Tick::ZERO) {
        song.sigs.insert(
            0,
            TimeSigEvent {
                tick: Tick::ZERO,
                num: 4,
                den: 4,
            },
        );
    }
    song.markers.sort_by_key(|m| m.tick);
    Ok(song)
}

/// 読み込みの結果。
pub struct Imported {
    pub commands: Vec<Command>,
    pub label: String,
    /// (トラック ID, 名前, ノート数)
    pub tracks: Vec<(TrackId, String, usize)>,
    pub tempo_set: bool,
}

/// GM の音色から内蔵の楽器を選ぶ
fn builtin_for(part: &Part) -> &'static str {
    if part.is_drum() {
        return "drum";
    }
    match part.program {
        0..=15 => "fm",                 // ピアノ・鍵盤打楽器
        24..=31 | 104..=107 => "pluck", // ギター・シタール・バンジョー・三味線・琴
        _ => "subtractive",
    }
}

/// 音源の選び方。SoundFont を使うときは、そのフォントにあるプリセット (bank, preset) の一覧を渡す
pub enum Instruments<'a> {
    Builtin,
    Soundfont {
        file: &'a str,
        presets: &'a [(u16, u16)],
    },
}

fn device_for(part: &Part, inst: &Instruments) -> Device {
    if let Instruments::Soundfont { file, presets } = inst {
        let want = if part.is_drum() {
            (128, 0)
        } else {
            (0, part.program as u16)
        };
        if presets.contains(&want) {
            return Device {
                source: PluginSource::Sf2 {
                    soundfont: (*file).to_owned(),
                    bank: want.0,
                    preset: want.1,
                },
                params: Default::default(),
            };
        }
    }
    Device::builtin(builtin_for(part))
}

/// 読み込んだ曲をプロジェクトに足すコマンドを作る(新しいトラックを末尾に足す)。
pub fn import_commands(
    project: &Project,
    song: &MidiSong,
    file_name: &str,
    start_tick: Option<u64>,
    set_tempo: Option<bool>,
    inst: &Instruments,
) -> Result<Imported, String> {
    if song.parts.is_empty() {
        return Err("ノートがありません".to_owned());
    }
    let offset = start_tick.unwrap_or(0);
    let tempo_set = set_tempo.unwrap_or_else(|| project.tracks.iter().all(|t| t.clips.is_empty()));
    let mut commands = Vec::new();
    let mut after = project.clone();

    if tempo_set {
        let shifted_before =
            |before: Vec<TempoEvent>| before.into_iter().filter(|e| e.tick.0 < offset);
        let mut tempos: Vec<TempoEvent> =
            shifted_before(project.tempo_map.events().to_vec()).collect();
        tempos.extend(song.tempos.iter().map(|e| TempoEvent {
            tick: Tick(e.tick.0 + offset),
            bpm: e.bpm,
        }));
        let mut sigs: Vec<TimeSigEvent> = project
            .time_sig_map
            .iter()
            .filter(|e| e.tick.0 < offset)
            .cloned()
            .collect();
        sigs.extend(song.sigs.iter().map(|e| TimeSigEvent {
            tick: Tick(e.tick.0 + offset),
            ..*e
        }));
        for c in [
            Command::SetTempo { events: tempos },
            Command::SetTimeSig { events: sigs },
        ] {
            after.apply(&c).map_err(|e| e.to_string())?;
            commands.push(c);
        }
    }
    if !song.markers.is_empty() {
        let mut sections: Vec<SectionMarker> = project.sections.clone();
        for m in &song.markers {
            let tick = Tick(m.tick.0 + offset);
            sections.retain(|s| s.tick != tick);
            sections.push(SectionMarker {
                tick,
                name: m.name.clone(),
            });
        }
        sections.sort_by_key(|m| m.tick);
        let c = Command::SetSections { sections };
        after.apply(&c).map_err(|e| e.to_string())?;
        commands.push(c);
    }

    // クリップの終わりは、最後のノートの後の小節の頭
    let last = song
        .parts
        .iter()
        .flat_map(|p| p.notes.iter().map(|n| n.0 + n.1))
        .max()
        .unwrap_or(0)
        + offset;
    if last > MAX_TICK.0 {
        return Err("曲が長すぎます".to_owned());
    }
    let end = arrange::bar_grid(&after, last)
        .iter()
        .map(|(s, len)| s + len)
        .find(|e| *e >= last)
        .unwrap_or(last);

    let mut tracks = Vec::new();
    for part in &song.parts {
        let id = TrackId::new();
        let mut t = Track::new(id.clone(), part.name.clone(), TrackKind::Midi);
        t.device = Some(device_for(part, inst));
        if let Some(v) = part.volume {
            // GM の音量カーブ(40 log10)
            t.volume_db = if v == 0 {
                -60.0
            } else {
                40.0 * (v as f32 / 127.0).log10()
            };
        }
        if let Some(p) = part.pan {
            t.pan = ((p as f32 - 64.0) / 63.0).clamp(-1.0, 1.0);
        }
        let mut clip = Clip::new_midi(
            ClipId::new(),
            part.name.clone(),
            Tick(offset),
            Tick((end - offset).max(1)),
        );
        if let Some(notes) = clip.notes_mut() {
            notes.extend(part.notes.iter().map(|&(pos, dur, pitch, vel)| Note {
                id: NoteId::new(),
                pos: Tick(pos),
                dur: Tick(dur),
                pitch,
                vel,
                articulation: Default::default(),
                pitch_curve: vec![],
                glide_ms: None,
            }));
            notes.sort_by(|a, b| (a.pos, a.pitch, &a.id).cmp(&(b.pos, b.pitch, &b.id)));
        }
        t.clips.push(clip);
        tracks.push((id, part.name.clone(), part.notes.len()));
        commands.push(Command::AddTrack {
            track: t,
            index: None,
        });
    }
    Ok(Imported {
        commands,
        label: format!(
            "MIDI ファイル「{file_name}」を読み込み(トラック {} 本)",
            song.parts.len()
        ),
        tracks,
        tempo_set,
    })
}

/// ファイルを読んで、読み込みのコマンドを作る(SoundFont の確認を含む。ブロッキング)
pub fn import_file(project: &Project, req: &ImportMidiRequest) -> Result<Imported, String> {
    let path = Path::new(&req.path);
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", req.path))?;
    let song = parse(&bytes)?;
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let presets: Vec<(u16, u16)>;
    let inst = match &req.soundfont {
        Some(file) => {
            let font = glaux_engine::sf2::load_font(&glaux_engine::sf2::default_dir().join(file))?;
            presets = glaux_engine::sf2::list_presets(&font)
                .into_iter()
                .map(|m| (m.bank, m.preset))
                .collect();
            Instruments::Soundfont {
                file,
                presets: &presets,
            }
        }
        None => Instruments::Builtin,
    };
    import_commands(
        project,
        &song,
        &file_name,
        req.start_tick,
        req.set_tempo,
        &inst,
    )
}

// ============================== 書き出し ==============================

/// 書き出すときの GM の音色(チャンネル 10 のドラムなら None)
fn program_for(track: &Track, avg_pitch: f64) -> Option<u8> {
    match track.device.as_ref().map(|d| &d.source) {
        Some(PluginSource::Sf2 { bank: 128, .. }) => None,
        Some(PluginSource::Sf2 { preset, .. }) => Some((*preset).min(127) as u8),
        Some(PluginSource::Builtin { name }) if name == "drum" => None,
        Some(PluginSource::Builtin { name }) if name == "pluck" => Some(25),
        Some(PluginSource::Builtin { name }) if name == "fm" => Some(4),
        Some(PluginSource::Builtin { name }) if name == "wavetable" => Some(89),
        _ if avg_pitch < 48.0 => Some(38), // シンセベース
        _ => Some(81),                     // シンセリード(ノコギリ)
    }
}

/// トラックの鳴るノート(ループを展開し、クリップの終わりで切る)。(開始, 終了, 音高, ベロシティ)
fn played_notes(track: &Track) -> Vec<RawNote> {
    let mut out = Vec::new();
    for clip in &track.clips {
        let Some(notes) = clip.notes() else { continue };
        let len = clip.length.0;
        let (period, reps) = match clip.loop_len() {
            Some(l) => (l.0, len.div_ceil(l.0)),
            None => (len, 1),
        };
        for k in 0..reps {
            let base = k * period;
            for n in notes.iter().filter(|n| n.pos.0 < period) {
                let pos = base + n.pos.0;
                if pos >= len {
                    continue;
                }
                let end = (pos + n.dur.0).min(len);
                out.push((
                    clip.start.0 + pos,
                    clip.start.0 + end,
                    n.pitch,
                    n.vel.clamp(1, 127),
                ));
            }
        }
    }
    out.sort_unstable();
    out
}

/// プロジェクトを SMF 1 のバイト列にする。
pub fn export_smf(project: &Project) -> Result<Vec<u8>, String> {
    // 文字列は先に持っておく(イベントはそれを借りる)
    let title = project.meta.title.clone().into_bytes();
    let markers: Vec<(u64, Vec<u8>)> = project
        .sections
        .iter()
        .map(|m| (m.tick.0, m.name.clone().into_bytes()))
        .collect();
    let parts: Vec<(&Track, Vec<u8>, Vec<RawNote>)> = project
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Midi)
        .map(|t| (t, t.name.clone().into_bytes(), played_notes(t)))
        .filter(|(_, _, n)| !n.is_empty())
        .collect();
    if parts.is_empty() {
        return Err("書き出せるノートがありません".to_owned());
    }

    // (tick, 同じ tick での順番, イベント) を並べてから差分にする
    fn to_track(mut evs: Vec<(u64, u8, TrackEventKind<'_>)>) -> Vec<TrackEvent<'_>> {
        evs.sort_by_key(|e| (e.0, e.1));
        let mut prev = 0;
        let mut out: Vec<TrackEvent> = evs
            .into_iter()
            .map(|(t, _, kind)| {
                let delta = u28::new((t - prev).min(u28::max_value().as_int() as u64) as u32);
                prev = t;
                TrackEvent { delta, kind }
            })
            .collect();
        out.push(TrackEvent {
            delta: u28::new(0),
            kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
        });
        out
    }

    let mut tracks = Vec::new();
    let mut head: Vec<(u64, u8, TrackEventKind)> =
        vec![(0, 0, TrackEventKind::Meta(MetaMessage::TrackName(&title)))];
    for e in project.tempo_map.events() {
        let us = (60_000_000.0 / e.bpm)
            .round()
            .clamp(1.0, u24::max_value().as_int() as f64) as u32;
        head.push((
            e.tick.0,
            1,
            TrackEventKind::Meta(MetaMessage::Tempo(u24::new(us))),
        ));
    }
    for s in &project.time_sig_map {
        head.push((
            s.tick.0,
            1,
            TrackEventKind::Meta(MetaMessage::TimeSignature(
                s.num,
                s.den.trailing_zeros() as u8,
                24,
                8,
            )),
        ));
    }
    for (t, name) in &markers {
        head.push((*t, 2, TrackEventKind::Meta(MetaMessage::Marker(name))));
    }
    tracks.push(to_track(head));

    let mut next_ch = 0u8;
    for (track, name, notes) in &parts {
        let avg = notes.iter().map(|n| n.2 as f64).sum::<f64>() / notes.len() as f64;
        let program = program_for(track, avg);
        let ch = match program {
            None => DRUM_CH,
            Some(_) => {
                let c = next_ch;
                next_ch = (next_ch + 1) % 16;
                if next_ch == DRUM_CH {
                    next_ch += 1;
                }
                c
            }
        };
        let channel = u4::new(ch);
        let midi = |message| TrackEventKind::Midi { channel, message };
        let mut evs = vec![(0, 0, TrackEventKind::Meta(MetaMessage::TrackName(name)))];
        if let Some(p) = program {
            evs.push((
                0,
                1,
                midi(MidiMessage::ProgramChange {
                    program: u7::new(p),
                }),
            ));
        }
        let vol = (127.0 * 10f32.powf(track.volume_db / 40.0))
            .round()
            .clamp(0.0, 127.0) as u8;
        let pan = (64.0 + track.pan * 63.0).round().clamp(0.0, 127.0) as u8;
        evs.push((
            0,
            1,
            midi(MidiMessage::Controller {
                controller: u7::new(7),
                value: u7::new(vol),
            }),
        ));
        evs.push((
            0,
            1,
            midi(MidiMessage::Controller {
                controller: u7::new(10),
                value: u7::new(pan),
            }),
        ));
        for &(s, e, k, v) in notes {
            // 同じ位置では、前の音を止めてから次を鳴らす
            evs.push((
                e,
                2,
                midi(MidiMessage::NoteOff {
                    key: u7::new(k),
                    vel: u7::new(0),
                }),
            ));
            evs.push((
                s,
                3,
                midi(MidiMessage::NoteOn {
                    key: u7::new(k),
                    vel: u7::new(v),
                }),
            ));
        }
        tracks.push(to_track(evs));
    }

    let mut smf = Smf::new(Header::new(
        Format::Parallel,
        Timing::Metrical(u15::new(PPQ as u16)),
    ));
    smf.tracks = tracks;
    let mut out = Vec::new();
    smf.write_std(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// 書き出す。`path` 省略でプロジェクトの export/ に日時付きの名前
pub fn export_file(
    project: &Project,
    project_dir: &Path,
    path: Option<&str>,
) -> Result<Value, String> {
    let bytes = export_smf(project)?;
    let path = path.map(PathBuf::from).unwrap_or_else(|| {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        project_dir.join("export").join(format!(
            "{}_{stamp}.mid",
            crate::export::sanitize(&project.meta.title)
        ))
    });
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let tracks = project
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Midi && !played_notes(t).is_empty())
        .count();
    Ok(json!({ "path": path.to_string_lossy(), "tracks": tracks }))
}

/// GM の音色名(トラック名に使う)
const GM_NAMES: [&str; 128] = [
    "Acoustic Grand Piano",
    "Bright Acoustic Piano",
    "Electric Grand Piano",
    "Honky-tonk Piano",
    "Electric Piano 1",
    "Electric Piano 2",
    "Harpsichord",
    "Clavinet",
    "Celesta",
    "Glockenspiel",
    "Music Box",
    "Vibraphone",
    "Marimba",
    "Xylophone",
    "Tubular Bells",
    "Dulcimer",
    "Drawbar Organ",
    "Percussive Organ",
    "Rock Organ",
    "Church Organ",
    "Reed Organ",
    "Accordion",
    "Harmonica",
    "Tango Accordion",
    "Nylon Guitar",
    "Steel Guitar",
    "Jazz Guitar",
    "Clean Guitar",
    "Muted Guitar",
    "Overdriven Guitar",
    "Distortion Guitar",
    "Guitar Harmonics",
    "Acoustic Bass",
    "Finger Bass",
    "Pick Bass",
    "Fretless Bass",
    "Slap Bass 1",
    "Slap Bass 2",
    "Synth Bass 1",
    "Synth Bass 2",
    "Violin",
    "Viola",
    "Cello",
    "Contrabass",
    "Tremolo Strings",
    "Pizzicato Strings",
    "Orchestral Harp",
    "Timpani",
    "String Ensemble 1",
    "String Ensemble 2",
    "Synth Strings 1",
    "Synth Strings 2",
    "Choir Aahs",
    "Voice Oohs",
    "Synth Voice",
    "Orchestra Hit",
    "Trumpet",
    "Trombone",
    "Tuba",
    "Muted Trumpet",
    "French Horn",
    "Brass Section",
    "Synth Brass 1",
    "Synth Brass 2",
    "Soprano Sax",
    "Alto Sax",
    "Tenor Sax",
    "Baritone Sax",
    "Oboe",
    "English Horn",
    "Bassoon",
    "Clarinet",
    "Piccolo",
    "Flute",
    "Recorder",
    "Pan Flute",
    "Blown Bottle",
    "Shakuhachi",
    "Whistle",
    "Ocarina",
    "Square Lead",
    "Saw Lead",
    "Calliope Lead",
    "Chiff Lead",
    "Charang Lead",
    "Voice Lead",
    "Fifths Lead",
    "Bass + Lead",
    "New Age Pad",
    "Warm Pad",
    "Polysynth Pad",
    "Choir Pad",
    "Bowed Pad",
    "Metallic Pad",
    "Halo Pad",
    "Sweep Pad",
    "Rain",
    "Soundtrack",
    "Crystal",
    "Atmosphere",
    "Brightness",
    "Goblins",
    "Echoes",
    "Sci-fi",
    "Sitar",
    "Banjo",
    "Shamisen",
    "Koto",
    "Kalimba",
    "Bagpipe",
    "Fiddle",
    "Shanai",
    "Tinkle Bell",
    "Agogo",
    "Steel Drums",
    "Woodblock",
    "Taiko Drum",
    "Melodic Tom",
    "Synth Drum",
    "Reverse Cymbal",
    "Guitar Fret Noise",
    "Breath Noise",
    "Seashore",
    "Bird Tweet",
    "Telephone Ring",
    "Helicopter",
    "Applause",
    "Gunshot",
];

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::{ClipContent, Tick};

    fn note(pos: u64, dur: u64, pitch: u8) -> Note {
        Note {
            id: NoteId::new(),
            pos: Tick(pos),
            dur: Tick(dur),
            pitch,
            vel: 100,
            articulation: Default::default(),
            pitch_curve: vec![],
            glide_ms: None,
        }
    }

    fn song() -> Project {
        let mut p = Project::new("曲");
        p.apply(&Command::SetTempo {
            events: vec![
                TempoEvent {
                    tick: Tick(0),
                    bpm: 100.0,
                },
                TempoEvent {
                    tick: Tick(7680),
                    bpm: 140.0,
                },
            ],
        })
        .unwrap();
        p.apply(&Command::SetTimeSig {
            events: vec![TimeSigEvent {
                tick: Tick(0),
                num: 3,
                den: 4,
            }],
        })
        .unwrap();
        p.sections = vec![SectionMarker {
            tick: Tick(2880),
            name: "サビ".into(),
        }];
        // ドラム(内蔵)
        let mut d = Track::new(TrackId::new(), "Drums", TrackKind::Midi);
        d.device = Some(Device::builtin("drum"));
        let mut c = Clip::new_midi(ClipId::new(), "d", Tick(0), Tick(2880));
        c.notes_mut()
            .unwrap()
            .extend([note(0, 240, 36), note(960, 240, 38)]);
        d.clips.push(c);
        // ベース: 1 拍のループを 3 拍ぶん(ノートは 3 回鳴る。ループの外のノートは鳴らない)
        let mut b = Track::new(TrackId::new(), "Bass", TrackKind::Midi);
        b.volume_db = -6.0;
        let mut c = Clip::new_midi(ClipId::new(), "b", Tick(2880), Tick(2880));
        if let ClipContent::Midi {
            notes,
            looped,
            loop_len,
        } = &mut c.content
        {
            notes.extend([note(0, 480, 40), note(1200, 480, 43)]);
            *looped = true;
            *loop_len = Some(Tick(960));
        }
        b.clips.push(c);
        p.tracks.extend([d, b]);
        p
    }

    #[test]
    fn export_then_import_keeps_tempo_notes_and_drums() {
        let p = song();
        let bytes = export_smf(&p).unwrap();
        let s = parse(&bytes).unwrap();
        assert_eq!(s.tempos.len(), 2);
        assert!((s.tempos[1].bpm - 140.0).abs() < 0.01 && s.tempos[1].tick == Tick(7680));
        assert_eq!((s.sigs[0].num, s.sigs[0].den), (3, 4));
        assert_eq!(s.markers[0].name, "サビ");
        assert_eq!(s.parts.len(), 2);
        assert!(s.parts[0].is_drum());
        assert_eq!(
            s.parts[0].notes,
            vec![(0, 240, 36, 100), (960, 240, 38, 100)]
        );
        let bass = &s.parts[1];
        assert!(!bass.is_drum());
        assert_eq!(bass.program, 38); // 低い音の内蔵シンセはシンセベース
        assert_eq!(
            bass.notes.iter().map(|n| n.0).collect::<Vec<_>>(),
            vec![2880, 3840, 4800],
            "ループが展開される"
        );
        assert_eq!(bass.volume, Some(90)); // -6dB → 127 × 10^(-6/40)

        // 空のプロジェクトに読み込む → テンポ・拍子・マーカーも移る
        let mut q = Project::new("新規");
        let imp = import_commands(&q, &s, "a.mid", None, None, &Instruments::Builtin).unwrap();
        assert!(imp.tempo_set);
        q.apply(&Command::batch(imp.label.clone(), imp.commands))
            .unwrap();
        assert_eq!(q.tempo_map.events().len(), 2);
        assert_eq!(q.time_sig_map[0].num, 3);
        assert_eq!(q.sections.len(), 1);
        assert_eq!(q.tracks.len(), 2);
        assert!(
            matches!(&q.tracks[0].device.as_ref().unwrap().source, PluginSource::Builtin { name } if name == "drum")
        );
        assert!((q.tracks[1].volume_db + 6.0).abs() < 0.1);
        // クリップは最後のノートの後の小節の頭まで(3/4 で 5760)
        assert_eq!(q.tracks[1].clips[0].length, Tick(5760));
    }

    #[test]
    fn imports_type0_with_other_resolution_and_keeps_tempo_of_a_busy_project() {
        // 480 分解能、1 トラックに 2 チャンネル(ピアノとドラム)
        let ch = |c| u4::new(c);
        let ev = |delta, kind| TrackEvent {
            delta: u28::new(delta),
            kind,
        };
        let on = |c, k, v| TrackEventKind::Midi {
            channel: ch(c),
            message: MidiMessage::NoteOn {
                key: u7::new(k),
                vel: u7::new(v),
            },
        };
        let track = vec![
            ev(
                0,
                TrackEventKind::Meta(MetaMessage::Tempo(u24::new(500_000))),
            ),
            ev(
                0,
                TrackEventKind::Midi {
                    channel: ch(0),
                    message: MidiMessage::ProgramChange {
                        program: u7::new(0),
                    },
                },
            ),
            ev(0, on(0, 60, 90)),
            ev(0, on(9, 36, 100)),
            ev(240, on(9, 36, 0)),
            ev(240, on(0, 60, 0)),
            ev(0, on(0, 64, 80)), // 終わりの無いノート
            ev(480, TrackEventKind::Meta(MetaMessage::EndOfTrack)),
        ];
        let mut smf = Smf::new(Header::new(
            Format::SingleTrack,
            Timing::Metrical(u15::new(480)),
        ));
        smf.tracks.push(track);
        let mut bytes = Vec::new();
        smf.write_std(&mut bytes).unwrap();

        let s = parse(&bytes).unwrap();
        assert_eq!(s.parts.len(), 2);
        assert_eq!(s.parts[0].name, "Acoustic Grand Piano");
        assert_eq!(s.parts[0].notes, vec![(0, 960, 60, 90), (960, 960, 64, 80)]);
        assert_eq!(s.parts[1].notes, vec![(0, 480, 36, 100)]);

        // クリップがあるプロジェクトではテンポを変えない。置く位置をずらせる
        let mut p = song();
        let before = p.tempo_map.clone();
        let imp =
            import_commands(&p, &s, "b.mid", Some(3840), None, &Instruments::Builtin).unwrap();
        assert!(!imp.tempo_set);
        p.apply(&Command::batch(imp.label, imp.commands)).unwrap();
        assert_eq!(p.tempo_map, before);
        let piano = &p.tracks[2];
        assert!(
            matches!(&piano.device.as_ref().unwrap().source, PluginSource::Builtin { name } if name == "fm")
        );
        assert_eq!(piano.clips[0].start, Tick(3840));

        // SoundFont にプリセットがあればそれ、無ければ内蔵
        let presets = [(0u16, 0u16)];
        let imp = import_commands(
            &Project::new("x"),
            &s,
            "b.mid",
            None,
            None,
            &Instruments::Soundfont {
                file: "gm.sf2",
                presets: &presets,
            },
        )
        .unwrap();
        let devices: Vec<_> = imp
            .commands
            .iter()
            .filter_map(|c| match c {
                Command::AddTrack { track, .. } => track.device.clone(),
                _ => None,
            })
            .collect();
        assert!(matches!(
            devices[0].source,
            PluginSource::Sf2 {
                preset: 0,
                bank: 0,
                ..
            }
        ));
        assert!(matches!(&devices[1].source, PluginSource::Builtin { name } if name == "drum"));

        assert!(parse(b"not a midi file").is_err());
    }
}
