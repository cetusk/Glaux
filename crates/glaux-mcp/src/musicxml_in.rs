//! MusicXML(partwise)の読み込み(MCP の import_musicxml とアプリの読み込み)。
//!
//! MIDI(SMF)より情報が多い: パート名・強弱記号・スタッカートやアクセント・スラー・移調楽器の指定・
//! リハーサルマーク・拍子のまとまり。読んだものは MIDI の読み込みと同じ形([`MidiSong`])に直し、
//! トラックの作り方・音源の選び方・テンポと拍子の扱いを共通にする。
//!
//! - 1 パート = 1 トラック。段が複数あるパート(ピアノの右手・左手など)は段ごとに分ける(2 段なら上段・下段)。
//!   声部は分けない。パート名が無ければ略称 → 楽器名 → 音色(GM)の名前。`<backup>` / `<forward>` で声部の位置を戻す・進める
//! - 高さ: step + alter + octave に、移調楽器の `<transpose>`(chromatic・octave-change)を足して実音にする。
//!   打楽器(unpitched)は、その音の楽器の `<midi-unpitched>`(GM のドラムの鍵盤)、無ければ表示の高さ
//! - タイはつないで 1 音に。和音(`<chord/>`)は前の音と同じ位置
//! - 強さ: 強弱記号(pp〜ff)と `<sound dynamics>`(100 = MIDI の 90)。sf・sfz・fz は次の音だけ強く
//! - 奏法: スタッカート系 → staccato、アクセント系 → accent、スラーの中の 2 音目から → legato
//! - テンポ: `<sound tempo>`、無ければメトロノーム記号(拍の単位を 4 分に直す)。拍子は最初のパートから
//! - 目印: リハーサルマーク → 区間の目印
//! - 装飾音符とキュー音符は飛ばす。繰り返し記号は展開しない(報告する)。.mxl(zip)も読む

use crate::midi::{ImportMidiRequest, Imported, MidiSong, Part, RawNote};
use glaux_core::{Articulation, SectionMarker, TempoEvent, Tick, TimeSigEvent, PPQ};
use roxmltree::{Document, Node};
use std::collections::{BTreeMap, HashMap};
use std::io::Read;

/// 読み込んだ結果と、移せなかったことの報告
pub struct ScoreRead {
    pub song: MidiSong,
    pub notes: Vec<String>,
    pub title: Option<String>,
}

/// .mxl(zip)なら中の楽譜を取り出す。そうでなければそのまま文字列に
pub fn read_text(bytes: &[u8]) -> Result<String, String> {
    if bytes.starts_with(b"PK") {
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes))
            .map_err(|e| format!(".mxl を開けません: {e}"))?;
        // META-INF/container.xml の rootfile、無ければ最初の .xml / .musicxml
        let mut root: Option<String> = None;
        if let Ok(mut f) = z.by_name("META-INF/container.xml") {
            let mut s = String::new();
            let _ = f.read_to_string(&mut s);
            let opt = roxmltree::ParsingOptions {
                allow_dtd: true,
                ..Default::default()
            };
            if let Ok(doc) = Document::parse_with_options(&s, opt) {
                root = doc
                    .descendants()
                    .find(|n| n.has_tag_name("rootfile"))
                    .and_then(|n| n.attribute("full-path"))
                    .map(str::to_owned);
            }
        }
        let name = match root {
            Some(r) => r,
            None => (0..z.len())
                .filter_map(|i| z.by_index(i).ok().map(|f| f.name().to_owned()))
                .find(|n| {
                    !n.starts_with("META-INF") && (n.ends_with(".xml") || n.ends_with(".musicxml"))
                })
                .ok_or(".mxl の中に楽譜がありません")?,
        };
        let mut f = z
            .by_name(&name)
            .map_err(|e| format!(".mxl の中の {name} を読めません: {e}"))?;
        let mut s = String::new();
        f.read_to_string(&mut s)
            .map_err(|e| format!(".mxl の中の {name} を読めません: {e}"))?;
        return Ok(s);
    }
    let s = String::from_utf8_lossy(bytes).into_owned();
    // UTF-16 などの宣言は roxmltree が読めないので、BOM だけ外す
    Ok(s.trim_start_matches('\u{feff}').to_owned())
}

fn child<'a>(n: Node<'a, 'a>, name: &str) -> Option<Node<'a, 'a>> {
    n.children().find(|c| c.has_tag_name(name))
}

fn text_of(n: Node, name: &str) -> Option<String> {
    child(n, name)
        .and_then(|c| c.text())
        .map(|t| t.trim().to_owned())
}

fn num<T: std::str::FromStr>(n: Node, name: &str) -> Option<T> {
    text_of(n, name).and_then(|t| t.parse().ok())
}

/// 強弱記号 → 強さ
fn dynamic_vel(name: &str) -> Option<u8> {
    Some(match name {
        "pppp" => 22,
        "ppp" => 30,
        "pp" => 42,
        "p" => 55,
        "mp" => 68,
        "mf" => 80,
        "f" => 96,
        "ff" => 110,
        "fff" => 120,
        "ffff" => 126,
        _ => return None,
    })
}

/// 拍の単位 → 4 分に対する倍率
fn beat_unit(unit: &str, dots: usize) -> f64 {
    let base = match unit {
        "whole" => 4.0,
        "half" => 2.0,
        "quarter" => 1.0,
        "eighth" => 0.5,
        "16th" => 0.25,
        "32nd" => 0.125,
        _ => 1.0,
    };
    let mut m = base;
    let mut add = base / 2.0;
    for _ in 0..dots {
        m += add;
        add /= 2.0;
    }
    m
}

/// パートの情報(part-list から)
#[derive(Default, Clone)]
struct PartInfo {
    name: String,
    channel: Option<u8>,
    program: Option<u8>,
    volume: Option<u8>,
    pan: Option<u8>,
    /// 楽器 ID → GM の鍵盤(打楽器)
    unpitched: HashMap<String, u8>,
}

pub fn parse(xml: &str) -> Result<ScoreRead, String> {
    // MusicXML はほぼ必ず DOCTYPE(外部の DTD の参照)を持つ。DTD は許すが外へは取りに行かない
    let opt = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let doc = Document::parse_with_options(xml, opt)
        .map_err(|e| format!("MusicXML を読めません: {e}"))?;
    let root = doc.root_element();
    if root.has_tag_name("score-timewise") {
        return Err("score-timewise の MusicXML には対応していません(譜面ソフトで partwise に書き出してください)".to_owned());
    }
    if !root.has_tag_name("score-partwise") {
        return Err(format!(
            "MusicXML ではありません(ルートの要素が {})",
            root.tag_name().name()
        ));
    }
    let title = child(root, "work")
        .and_then(|w| text_of(w, "work-title"))
        .or_else(|| text_of(root, "movement-title"));
    // part-list
    let mut infos: HashMap<String, PartInfo> = HashMap::new();
    if let Some(pl) = child(root, "part-list") {
        for sp in pl.children().filter(|c| c.has_tag_name("score-part")) {
            let id = sp.attribute("id").unwrap_or_default().to_owned();
            // 名前: パート名 → 略称 → 楽器名(空の名前は飛ばす)。どれも無ければ後で音色(GM)の名前
            let named = |tag: &str| text_of(sp, tag).filter(|t| !t.is_empty());
            let name = named("part-name")
                .or_else(|| named("part-abbreviation"))
                .or_else(|| {
                    sp.descendants()
                        .find(|c| c.has_tag_name("instrument-name"))
                        .and_then(|c| c.text())
                        .map(|t| t.trim().to_owned())
                        .filter(|t| !t.is_empty())
                })
                .unwrap_or_default();
            let mut info = PartInfo {
                name,
                ..Default::default()
            };
            for mi in sp.children().filter(|c| c.has_tag_name("midi-instrument")) {
                let inst = mi.attribute("id").unwrap_or_default().to_owned();
                if let Some(ch) = num::<u8>(mi, "midi-channel") {
                    info.channel.get_or_insert(ch.saturating_sub(1).min(15));
                }
                if let Some(pg) = num::<u8>(mi, "midi-program") {
                    info.program.get_or_insert(pg.saturating_sub(1).min(127));
                }
                if let Some(v) = num::<f64>(mi, "volume") {
                    info.volume
                        .get_or_insert((v.clamp(0.0, 100.0) * 1.27).round() as u8);
                }
                if let Some(p) = num::<f64>(mi, "pan") {
                    info.pan
                        .get_or_insert(((p.clamp(-90.0, 90.0) / 90.0) * 63.0 + 64.0).round() as u8);
                }
                if let Some(u) = num::<u8>(mi, "midi-unpitched") {
                    info.unpitched.insert(inst, u.saturating_sub(1).min(127));
                }
            }
            infos.insert(id, info);
        }
    }
    let mut song = MidiSong::default();
    let mut report: Vec<String> = Vec::new();
    let mut graces = 0usize;
    let mut cues = 0usize;
    let mut repeats = 0usize;
    let mut names_seen: HashMap<String, usize> = HashMap::new();
    for (pi, part) in root
        .children()
        .filter(|c| c.has_tag_name("part"))
        .enumerate()
    {
        let id = part.attribute("id").unwrap_or_default();
        let info = infos.get(id).cloned().unwrap_or_else(|| PartInfo {
            name: id.to_owned(),
            ..Default::default()
        });
        let first = pi == 0;
        let mut divisions = 1.0f64;
        let mut transpose = 0i32;
        let mut vel: u8 = 80;
        let mut sforzando = false;
        let mut pos: f64 = 0.0; // tick(小数のまま進め、音の頭で丸める)
        let mut measure_start: f64 = 0.0;
        // 段ごとの音(ピアノの右手・左手など。段が 1 つなら段 1 だけ)
        let mut staves: BTreeMap<u32, (Vec<RawNote>, BTreeMap<usize, Articulation>)> =
            BTreeMap::new();
        // タイの途中の音((段, 音高) → その段の notes の番号)
        let mut open_ties: HashMap<(u32, u8), usize> = HashMap::new();
        // スラーの中か(番号ごと)
        let mut slurs: HashMap<String, bool> = HashMap::new();
        let mut last_onset: f64 = 0.0;
        let mut any_unpitched = false;
        for m in part.children().filter(|c| c.has_tag_name("measure")) {
            let mut furthest = pos;
            for e in m.children().filter(|c| c.is_element()) {
                match e.tag_name().name() {
                    "attributes" => {
                        if let Some(d) = num::<f64>(e, "divisions") {
                            if d > 0.0 {
                                divisions = d;
                            }
                        }
                        if let Some(t) = child(e, "transpose") {
                            transpose = num::<i32>(t, "chromatic").unwrap_or(0)
                                + 12 * num::<i32>(t, "octave-change").unwrap_or(0);
                        }
                        if first {
                            if let Some(t) = child(e, "time") {
                                let beats = text_of(t, "beats").unwrap_or_default();
                                let den: u8 = num(t, "beat-type").unwrap_or(4);
                                let parts: Vec<u8> = beats
                                    .split('+')
                                    .filter_map(|x| x.trim().parse().ok())
                                    .collect();
                                let n: u32 = parts.iter().map(|&x| x as u32).sum();
                                if n > 0 && n <= 64 {
                                    song.sigs.push(TimeSigEvent {
                                        tick: Tick(pos.round() as u64),
                                        num: n as u8,
                                        den,
                                        grouping: (parts.len() > 1).then_some(parts),
                                    });
                                }
                            }
                        }
                    }
                    "direction" | "sound" => {
                        let at = pos
                            + child(e, "offset")
                                .and_then(|o| o.text())
                                .and_then(|t| t.trim().parse::<f64>().ok())
                                .map_or(0.0, |o| o * PPQ as f64 / divisions);
                        let sound = if e.has_tag_name("sound") {
                            Some(e)
                        } else {
                            child(e, "sound")
                        };
                        let mut tempo = sound
                            .and_then(|s| s.attribute("tempo"))
                            .and_then(|t| t.parse::<f64>().ok());
                        if let Some(d) = sound
                            .and_then(|s| s.attribute("dynamics"))
                            .and_then(|t| t.parse::<f64>().ok())
                        {
                            vel = (d * 0.9).round().clamp(1.0, 127.0) as u8;
                        }
                        for dt in e.children().filter(|c| c.has_tag_name("direction-type")) {
                            for x in dt.children().filter(|c| c.is_element()) {
                                match x.tag_name().name() {
                                    "dynamics" => {
                                        for d in x.children().filter(|c| c.is_element()) {
                                            let nm = d.tag_name().name();
                                            if let Some(v) = dynamic_vel(nm) {
                                                vel = v;
                                            } else if matches!(
                                                nm,
                                                "sf" | "sfz" | "fz" | "sfp" | "rfz" | "sffz"
                                            ) {
                                                sforzando = true;
                                            }
                                        }
                                    }
                                    "metronome" if tempo.is_none() => {
                                        let unit = text_of(x, "beat-unit").unwrap_or_default();
                                        let dots = x
                                            .children()
                                            .filter(|c| c.has_tag_name("beat-unit-dot"))
                                            .count();
                                        if let Some(pm) = num::<f64>(x, "per-minute") {
                                            tempo = Some(pm * beat_unit(&unit, dots));
                                        }
                                    }
                                    "rehearsal" if first => {
                                        if let Some(t) = x.text() {
                                            song.markers.push(SectionMarker {
                                                tick: Tick(at.round() as u64),
                                                name: t.trim().to_owned(),
                                                ..Default::default()
                                            });
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        if let Some(bpm) = tempo.filter(|b| *b > 0.0 && *b < 1000.0) {
                            let tick = Tick(at.round() as u64);
                            song.tempos.retain(|t| t.tick != tick);
                            song.tempos.push(TempoEvent { tick, bpm });
                        }
                    }
                    "backup" => {
                        let d = num::<f64>(e, "duration").unwrap_or(0.0) * PPQ as f64 / divisions;
                        pos = (pos - d).max(measure_start);
                    }
                    "forward" => {
                        let d = num::<f64>(e, "duration").unwrap_or(0.0) * PPQ as f64 / divisions;
                        pos += d;
                        furthest = furthest.max(pos);
                    }
                    "barline" => {
                        if child(e, "repeat").is_some() {
                            repeats += 1;
                        }
                    }
                    "note" => {
                        if child(e, "grace").is_some() {
                            graces += 1;
                            continue;
                        }
                        if child(e, "cue").is_some() {
                            cues += 1;
                            continue;
                        }
                        let dur = num::<f64>(e, "duration").unwrap_or(0.0) * PPQ as f64 / divisions;
                        let chord = child(e, "chord").is_some();
                        let onset = if chord { last_onset } else { pos };
                        if !chord {
                            last_onset = pos;
                        }
                        let pitch: Option<i32> = if let Some(p) = child(e, "pitch") {
                            let step = match text_of(p, "step").as_deref() {
                                Some("C") => 0,
                                Some("D") => 2,
                                Some("E") => 4,
                                Some("F") => 5,
                                Some("G") => 7,
                                Some("A") => 9,
                                Some("B") => 11,
                                _ => 0,
                            };
                            let alter = num::<f64>(p, "alter").unwrap_or(0.0).round() as i32;
                            let oct = num::<i32>(p, "octave").unwrap_or(4);
                            Some((oct + 1) * 12 + step + alter + transpose)
                        } else if let Some(u) = child(e, "unpitched") {
                            any_unpitched = true;
                            let inst = child(e, "instrument").and_then(|i| i.attribute("id"));
                            inst.and_then(|i| info.unpitched.get(i).map(|&k| k as i32))
                                .or_else(|| {
                                    let step = match text_of(u, "display-step").as_deref() {
                                        Some("C") => 0,
                                        Some("D") => 2,
                                        Some("E") => 4,
                                        Some("F") => 5,
                                        Some("G") => 7,
                                        Some("A") => 9,
                                        Some("B") => 11,
                                        _ => 0,
                                    };
                                    num::<i32>(u, "display-octave").map(|o| (o + 1) * 12 + step)
                                })
                        } else {
                            None // 休み
                        };
                        if let Some(p) = pitch.filter(|p| (0..=127).contains(p)) {
                            let p = p as u8;
                            let staff: u32 = num(e, "staff").unwrap_or(1);
                            let (notes, arts) = staves.entry(staff).or_default();
                            let ties: Vec<&str> = e
                                .children()
                                .filter(|c| c.has_tag_name("tie"))
                                .filter_map(|c| c.attribute("type"))
                                .collect();
                            let start = onset.round() as u64;
                            let len = dur.round().max(1.0) as u64;
                            let continued =
                                ties.contains(&"stop") && open_ties.contains_key(&(staff, p));
                            if continued {
                                // タイでつなぐ: 前の音を伸ばす
                                let i = open_ties[&(staff, p)];
                                let n = notes[i];
                                notes[i] = (n.0, (start + len).saturating_sub(n.0), n.2, n.3);
                                if !ties.contains(&"start") {
                                    open_ties.remove(&(staff, p));
                                }
                            } else {
                                let v = if sforzando {
                                    sforzando = false;
                                    vel.saturating_add(15).min(127)
                                } else {
                                    vel
                                };
                                notes.push((start, len, p, v.max(1)));
                                let i = notes.len() - 1;
                                if ties.contains(&"start") {
                                    open_ties.insert((staff, p), i);
                                }
                                // 奏法とスラー
                                let mut art = None;
                                let in_slur = slurs.values().any(|&x| x);
                                if in_slur {
                                    art = Some(Articulation::Legato);
                                }
                                if let Some(nt) = child(e, "notations") {
                                    if let Some(a) = child(nt, "articulations") {
                                        for x in a.children().filter(|c| c.is_element()) {
                                            match x.tag_name().name() {
                                                "staccato" | "staccatissimo" | "spiccato" => {
                                                    art = Some(Articulation::Staccato)
                                                }
                                                "accent" | "strong-accent" => {
                                                    art = Some(Articulation::Accent)
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                    for s in nt.children().filter(|c| c.has_tag_name("slur")) {
                                        let n = s.attribute("number").unwrap_or("1").to_owned();
                                        match s.attribute("type") {
                                            Some("start") => {
                                                slurs.insert(n, true);
                                            }
                                            Some("stop") => {
                                                slurs.insert(n, false);
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                if let Some(a) = art {
                                    arts.insert(i, a);
                                }
                            }
                        }
                        if !chord {
                            pos += dur;
                            furthest = furthest.max(pos);
                        }
                    }
                    _ => {}
                }
            }
            // 次の小節の頭: この小節で最も進んだ所(弱起の小節は短い)
            pos = furthest.max(measure_start);
            measure_start = pos;
        }
        if staves.values().all(|(n, _)| n.is_empty()) {
            report.push(format!(
                "パート「{}」には音がありません(読み込まない)",
                if info.name.is_empty() { id } else { &info.name }
            ));
            continue;
        }
        let base_name = if info.name.is_empty() {
            // パート名が無ければ音色(GM)の名前、それも無ければ番号
            match info.program {
                Some(pg) => crate::midi::GM_NAMES[pg as usize & 127].to_owned(),
                None => format!("パート {}", pi + 1),
            }
        } else {
            info.name.clone()
        };
        let drum = info.channel == Some(9) || (any_unpitched && info.channel.is_none());
        let n_staves = staves.values().filter(|(n, _)| !n.is_empty()).count();
        let mut k_staff = 0;
        for (staff, (notes, arts)) in staves {
            if notes.is_empty() {
                continue;
            }
            k_staff += 1;
            // 段が複数あれば段ごとにトラック(2 段なら上段・下段)
            let label = match (n_staves, k_staff) {
                (1, _) => base_name.clone(),
                (2, 1) => format!("{base_name}(上段)"),
                (2, _) => format!("{base_name}(下段)"),
                _ => format!("{base_name}({staff} 段目)"),
            };
            // 同じ名前のトラックは番号を付ける
            let count = names_seen.entry(label.clone()).or_insert(0);
            *count += 1;
            let name = if *count > 1 {
                format!("{label} {count}")
            } else {
                label
            };
            // 位置でそろえる(奏法の番号も並べ替えに合わせる)
            let mut order: Vec<usize> = (0..notes.len()).collect();
            order.sort_by_key(|&i| (notes[i].0, notes[i].2));
            let sorted: Vec<RawNote> = order.iter().map(|&i| notes[i]).collect();
            let arts_sorted: BTreeMap<usize, Articulation> = order
                .iter()
                .enumerate()
                .filter_map(|(new, &old)| arts.get(&old).map(|a| (new, *a)))
                .collect();
            song.parts.push(Part {
                name,
                channel: if drum {
                    9
                } else {
                    info.channel.filter(|&c| c != 9).unwrap_or(pi as u8 % 16)
                },
                program: info.program.unwrap_or(0),
                notes: sorted,
                volume: info.volume,
                pan: info.pan,
                articulations: arts_sorted,
            });
        }
    }
    if graces > 0 {
        report.push(format!("装飾音符 {graces} 個は飛ばしました"));
    }
    if cues > 0 {
        report.push(format!("キュー音符 {cues} 個は飛ばしました"));
    }
    if repeats > 0 {
        report.push(format!(
            "繰り返し記号({repeats} か所)は展開していません(書かれた順に 1 回ずつ並べています)"
        ));
    }
    // テンポ・拍子は頭に無ければ既定、同じ位置は後のもの
    song.tempos.sort_by_key(|t| t.tick);
    if song.tempos.first().is_none_or(|t| t.tick.0 > 0) {
        song.tempos.insert(
            0,
            TempoEvent {
                tick: Tick(0),
                bpm: 120.0,
            },
        );
    }
    song.sigs.sort_by_key(|s| s.tick);
    song.sigs.dedup_by(|b, a| {
        if a.tick == b.tick {
            *a = b.clone();
            true
        } else {
            false
        }
    });
    // 同じ拍子が続くものは 1 つに
    song.sigs
        .dedup_by(|b, a| a.num == b.num && a.den == b.den && a.grouping == b.grouping);
    if song.sigs.first().is_none_or(|s| s.tick.0 > 0) {
        song.sigs.insert(0, TimeSigEvent::new(Tick(0), 4, 4));
    }
    song.markers.sort_by_key(|m| m.tick);
    Ok(ScoreRead {
        song,
        notes: report,
        title,
    })
}

/// 楽譜のファイル(.musicxml / .xml / .mxl)かどうか(拡張子で見る)
pub fn is_score_file(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.ends_with(".musicxml") || p.ends_with(".mxl") || p.ends_with(".xml")
}

/// ファイルを読んで、読み込みのコマンドを作る(MIDI の読み込みと同じ置き方。ブロッキング)。
/// 返り値の 2 つ目は移せなかったことの報告
pub fn import_file(
    project: &glaux_core::Project,
    req: &ImportMidiRequest,
) -> Result<(Imported, Vec<String>), String> {
    let path = std::path::Path::new(&req.path);
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", req.path))?;
    let text = read_text(&bytes)?;
    let r = parse(&text)?;
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
            crate::midi::Instruments::Soundfont {
                file,
                presets: &presets,
            }
        }
        None => crate::midi::Instruments::Builtin,
    };
    let mut imp = crate::midi::import_commands(
        project,
        &r.song,
        &file_name,
        req.start_tick,
        req.set_tempo,
        &inst,
    )?;
    imp.label = format!(
        "MusicXML「{}」を読み込み(トラック {} 本)",
        r.title.as_deref().unwrap_or(&file_name),
        imp.tracks.len()
    );
    Ok((imp, r.notes))
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<score-partwise version="4.0">
  <work><work-title>Test</work-title></work>
  <part-list>
    <score-part id="P1"><part-name>Flute</part-name>
      <midi-instrument id="P1-I1"><midi-channel>1</midi-channel><midi-program>74</midi-program><volume>80</volume><pan>-45</pan></midi-instrument>
    </score-part>
    <score-part id="P2"><part-name>Clarinet in Bb</part-name></score-part>
    <score-part id="P3"><part-name>Drums</part-name>
      <score-instrument id="P3-I36"><instrument-name>Kick</instrument-name></score-instrument>
      <midi-instrument id="P3-I36"><midi-channel>10</midi-channel><midi-unpitched>37</midi-unpitched></midi-instrument>
    </score-part>
  </part-list>
  <part id="P1">
    <measure number="1">
      <attributes><divisions>2</divisions><time><beats>3+2</beats><beat-type>8</beat-type></time></attributes>
      <direction><direction-type><rehearsal>A</rehearsal></direction-type></direction>
      <direction><direction-type><metronome><beat-unit>quarter</beat-unit><per-minute>100</per-minute></metronome></direction-type></direction>
      <direction><direction-type><dynamics><p/></dynamics></direction-type></direction>
      <note><pitch><step>C</step><octave>5</octave></pitch><duration>1</duration><notations><slur type="start" number="1"/></notations></note>
      <note><pitch><step>D</step><octave>5</octave></pitch><duration>1</duration><notations><articulations><staccato/></articulations></notations></note>
      <note><pitch><step>E</step><alter>-1</alter><octave>5</octave></pitch><duration>1</duration><notations><slur type="stop" number="1"/></notations></note>
      <note><grace/><pitch><step>F</step><octave>5</octave></pitch></note>
      <note><pitch><step>G</step><octave>5</octave></pitch><duration>2</duration><tie type="start"/></note>
    </measure>
    <measure number="2">
      <direction><direction-type><dynamics><f/></dynamics></direction-type><sound tempo="120"/></direction>
      <note><pitch><step>G</step><octave>5</octave></pitch><duration>2</duration><tie type="stop"/></note>
      <note><pitch><step>C</step><octave>6</octave></pitch><duration>2</duration><notations><articulations><accent/></articulations></notations></note>
      <note><chord/><pitch><step>E</step><octave>6</octave></pitch><duration>2</duration></note>
      <note><rest/><duration>1</duration></note>
    </measure>
  </part>
  <part id="P2">
    <measure number="1">
      <attributes><divisions>1</divisions><transpose><diatonic>-1</diatonic><chromatic>-2</chromatic></transpose></attributes>
      <note><pitch><step>D</step><octave>4</octave></pitch><duration>1</duration><voice>1</voice></note>
      <backup><duration>1</duration></backup>
      <note><pitch><step>B</step><octave>3</octave></pitch><duration>1</duration><voice>2</voice></note>
    </measure>
  </part>
  <part id="P3">
    <measure number="1">
      <attributes><divisions>1</divisions></attributes>
      <note><unpitched><display-step>F</display-step><display-octave>4</display-octave></unpitched><duration>1</duration><instrument id="P3-I36"/></note>
    </measure>
  </part>
</score-partwise>"#;

    #[test]
    fn a_score_becomes_parts_with_dynamics_articulations_and_tempo() {
        let r = parse(XML).unwrap();
        assert_eq!(r.title.as_deref(), Some("Test"));
        let s = &r.song;
        // パート名・音色(1 始まり → 0 始まり)・音量・パン
        let fl = &s.parts[0];
        assert_eq!(fl.name, "Flute");
        assert_eq!(fl.program, 73);
        assert_eq!(fl.volume, Some(102));
        assert!(fl.pan.unwrap() < 64);
        // 8 分(divisions 2 → 480 tick)。C5 D5 Eb5、G5 はタイで 4 分 + 4 分 = 1920、和音 C6・E6
        let pitches: Vec<(u64, u64, u8, u8)> = fl.notes.clone();
        assert_eq!(pitches[0], (0, 480, 72, 55));
        assert_eq!(pitches[1].2, 74);
        assert_eq!(pitches[2].2, 75);
        let g = pitches.iter().find(|n| n.2 == 79).unwrap();
        assert_eq!((g.0, g.1), (1440, 1920));
        // 2 小節目の頭は 5/8 の後(1440 + 960 = 2400 tick。3+2 の 8 分 5 つ)
        let c6 = pitches.iter().find(|n| n.2 == 84).unwrap();
        let e6 = pitches.iter().find(|n| n.2 == 88).unwrap();
        assert_eq!(c6.0, 2400 + 960);
        assert_eq!(e6.0, c6.0);
        assert_eq!(c6.3, 96, "f の強さ");
        // 奏法: スラーの 2 音目は legato のはずがスタッカートが優先、3 音目は legato、アクセント
        assert_eq!(fl.articulations.get(&1), Some(&Articulation::Staccato));
        assert_eq!(fl.articulations.get(&2), Some(&Articulation::Legato));
        let c6_i = fl.notes.iter().position(|n| n.2 == 84).unwrap();
        assert_eq!(fl.articulations.get(&c6_i), Some(&Articulation::Accent));
        // 装飾音符は飛ばす
        assert!(!pitches.iter().any(|n| n.2 == 77));
        // テンポ(メトロノーム 100 → 2 小節目の sound 120)・拍子のまとまり・リハーサルマーク
        assert_eq!(s.tempos[0].bpm, 100.0);
        assert_eq!(s.tempos[1].bpm, 120.0);
        assert_eq!(s.tempos[1].tick.0, 2400);
        assert_eq!((s.sigs[0].num, s.sigs[0].den), (5, 8));
        assert_eq!(s.sigs[0].grouping, Some(vec![3, 2]));
        assert_eq!(s.markers[0].name, "A");
        // 移調楽器(B♭ クラリネット: 書かれた D4 → 実音 C4)と 2 声部
        let cl = &s.parts[1];
        assert_eq!(
            cl.notes.iter().map(|n| n.2).collect::<Vec<_>>(),
            vec![57, 60]
        );
        assert_eq!(cl.notes[0].0, cl.notes[1].0);
        // 打楽器は GM の鍵盤(midi-unpitched 37 → 36)、10ch
        let dr = &s.parts[2];
        assert!(dr.is_drum());
        assert_eq!(dr.notes[0].2, 36);
        assert!(r.notes.iter().any(|n| n.contains("装飾音符")));
    }

    #[test]
    fn timewise_and_non_musicxml_are_rejected() {
        assert!(parse("<score-timewise/>").is_err());
        assert!(parse("<html/>").is_err());
        assert!(parse("not xml").is_err());
    }

    #[test]
    fn a_doctype_and_an_mxl_zip_are_read() {
        let with_dtd = XML.replacen(
            "<score-partwise",
            "<!DOCTYPE score-partwise PUBLIC \"-//Recordare//DTD MusicXML 4.0 Partwise//EN\" \"http://www.musicxml.org/dtds/partwise.dtd\">\n<score-partwise",
            1,
        );
        assert_eq!(parse(&with_dtd).unwrap().song.parts.len(), 3);
        // .mxl: META-INF/container.xml が中の楽譜を指す
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let o = zip::write::SimpleFileOptions::default();
            w.start_file("META-INF/container.xml", o).unwrap();
            std::io::Write::write_all(
                &mut w,
                br#"<container><rootfiles><rootfile full-path="score/song.musicxml"/></rootfiles></container>"#,
            )
            .unwrap();
            w.start_file("score/song.musicxml", o).unwrap();
            std::io::Write::write_all(&mut w, with_dtd.as_bytes()).unwrap();
            w.finish().unwrap();
        }
        let text = read_text(buf.get_ref()).unwrap();
        assert_eq!(parse(&text).unwrap().song.parts[0].name, "Flute");
    }

    #[test]
    fn what_glaux_writes_comes_back() {
        use glaux_core::{Clip, ClipId, Note, NoteId, Project, Track, TrackId, TrackKind};
        let mut p = Project::new("往復");
        let mut t = Track::new(TrackId::new(), "Lead", TrackKind::Midi);
        let mut c = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(3840 * 2));
        let seq = [
            (0u64, 960u64, 72u8),
            (960, 480, 74),
            (1440, 480, 76),
            (1920, 1920, 79),
            (3840, 960, 77),
            (4800, 2880, 76),
        ];
        if let Some(ns) = c.notes_mut() {
            for (pos, dur, pitch) in seq {
                ns.push(Note {
                    id: NoteId::new(),
                    pos: Tick(pos),
                    dur: Tick(dur),
                    pitch,
                    vel: 90,
                    articulation: Default::default(),
                    pitch_curve: vec![],
                    glide_ms: None,
                    vibrato: None,
                    volume_curve: vec![],
                    brightness_curve: vec![],
                    condition: None,
                });
            }
        }
        t.clips.push(c);
        p.tracks.push(t);
        let (xml, _) = crate::musicxml::to_musicxml(&p).unwrap();
        let r = parse(&xml).unwrap();
        let back: Vec<(u64, u64, u8)> = r.song.parts[0]
            .notes
            .iter()
            .map(|n| (n.0, n.1, n.2))
            .collect();
        assert_eq!(back, seq.to_vec());
    }

    #[test]
    fn a_two_staff_part_without_a_name_becomes_two_tracks() {
        let xml = r#"<score-partwise version="4.0">
  <part-list><score-part id="P1"><part-name print-object="no"></part-name>
    <midi-instrument id="P1-I1"><midi-program>1</midi-program></midi-instrument></score-part></part-list>
  <part id="P1">
    <measure number="1">
      <attributes><divisions>1</divisions><staves>2</staves></attributes>
      <note><pitch><step>E</step><octave>5</octave></pitch><duration>2</duration><tie type="start"/><staff>1</staff></note>
      <note><pitch><step>E</step><octave>5</octave></pitch><duration>2</duration><tie type="stop"/><staff>1</staff></note>
      <backup><duration>4</duration></backup>
      <note><pitch><step>C</step><octave>3</octave></pitch><duration>4</duration><staff>2</staff></note>
      <note><chord/><pitch><step>G</step><octave>3</octave></pitch><duration>4</duration><staff>2</staff></note>
    </measure>
  </part>
</score-partwise>"#;
        let r = parse(xml).unwrap();
        let names: Vec<&str> = r.song.parts.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["Acoustic Grand Piano(上段)", "Acoustic Grand Piano(下段)"]
        );
        // 上段はタイで 1 音、下段は和音
        assert_eq!(r.song.parts[0].notes, vec![(0, 3840, 76, 80)]);
        assert_eq!(r.song.parts[1].notes.len(), 2);
    }
}
