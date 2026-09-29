//! MusicXML(partwise 4.0)への書き出し(MCP の export_musicxml)。
//!
//! 譜面ソフト(MuseScore・Dorico など)で開くための書き出し。MIDI(SMF)は拍子の分子と分母しか持てないが、
//! MusicXML は拍のまとまりを `<beats>2+2+3</beats>` で書ける。
//! - 位置と長さは 32 分の格子にそろえる(`divisions` = 4 分あたり 8)。3 連などの細かいずれは近い格子へ
//! - 1 トラック = 1 パート・1 声部。同じ位置の音は和音(`<chord/>`)、重なって続く音は次の音の頭で切る
//! - 小節をまたぐ音・定番でない長さはタイでつないだ定番の音価に分ける。休みも同じ
//! - キーは曲の音から推定して調号に、テンポは小節の頭のメトロノーム記号に

use crate::midi::{played_notes, RawNote};
use glaux_core::meter::{bar_meters, BarMeter};
use glaux_core::{Project, TrackKind};
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// 4 分あたりの divisions(32 分 = 1)
const DIV: u64 = 8;
/// 32 分の tick
const GRID: u64 = glaux_core::PPQ / DIV;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// 32 分の数 → (音符の種類, 付点か)。定番でなければ None
fn note_type(units: u64) -> Option<(&'static str, bool)> {
    Some(match units {
        32 => ("whole", false),
        24 => ("half", true),
        16 => ("half", false),
        12 => ("quarter", true),
        8 => ("quarter", false),
        6 => ("eighth", true),
        4 => ("eighth", false),
        3 => ("16th", true),
        2 => ("16th", false),
        1 => ("32nd", false),
        _ => return None,
    })
}

/// 長さを定番の音価に分ける(大きい順。拍の頭をまたがない細かさは求めない)
fn split_units(mut units: u64) -> Vec<u64> {
    const PARTS: [u64; 10] = [32, 24, 16, 12, 8, 6, 4, 3, 2, 1];
    let mut out = Vec::new();
    while units > 0 {
        let p = PARTS.iter().copied().find(|&p| p <= units).unwrap_or(1);
        out.push(p);
        units -= p;
    }
    out
}

/// 調号(五度圏の数)。長調の主音 → fifths
fn key_fifths(project: &Project) -> i32 {
    let notes: Vec<glaux_core::melody::MelNote> = project
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Midi)
        .flat_map(played_notes)
        .map(|(s, e, p, _)| glaux_core::melody::MelNote {
            pos: s,
            dur: e - s,
            pitch: p,
        })
        .collect();
    if notes.is_empty() {
        return 0;
    }
    let key = glaux_core::melody::guess_key(&notes);
    let major_tonic = if key.minor {
        (key.tonic + 3) % 12
    } else {
        key.tonic % 12
    };
    // C G D A E B F#/Gb Db Ab Eb Bb F
    const FIFTHS: [i32; 12] = [0, -5, 2, -3, 4, -1, 6, 1, -4, 3, -2, 5];
    FIFTHS[major_tonic as usize]
}

/// 音程 → (step, alter, octave)。調号がシャープ系ならシャープ、フラット系ならフラットで書く
fn spell(pitch: u8, fifths: i32) -> (&'static str, i32, i32) {
    const SHARP: [(&str, i32); 12] = [
        ("C", 0),
        ("C", 1),
        ("D", 0),
        ("D", 1),
        ("E", 0),
        ("F", 0),
        ("F", 1),
        ("G", 0),
        ("G", 1),
        ("A", 0),
        ("A", 1),
        ("B", 0),
    ];
    const FLAT: [(&str, i32); 12] = [
        ("C", 0),
        ("D", -1),
        ("D", 0),
        ("E", -1),
        ("E", 0),
        ("F", 0),
        ("G", -1),
        ("G", 0),
        ("A", -1),
        ("A", 0),
        ("B", -1),
        ("B", 0),
    ];
    let (step, alter) = if fifths < 0 { FLAT } else { SHARP }[(pitch % 12) as usize];
    (step, alter, pitch as i32 / 12 - 1)
}

/// 1 声部の出来事(32 分の単位。曲の頭から)
#[derive(Clone, Debug, PartialEq)]
struct Event {
    start: u64,
    len: u64,
    /// 空なら休み
    pitches: Vec<u8>,
}

/// 音を 32 分の格子にそろえて、1 声部の並び(和音 + 休み)にする
fn voice(notes: &[RawNote], end_units: u64) -> Vec<Event> {
    let q = |t: u64| (t + GRID / 2) / GRID;
    let mut groups: std::collections::BTreeMap<u64, (u64, Vec<u8>)> = Default::default();
    for &(s, e, p, _) in notes {
        let a = q(s);
        let b = q(e).max(a + 1);
        let g = groups.entry(a).or_insert((0, Vec::new()));
        g.0 = g.0.max(b);
        if !g.1.contains(&p) {
            g.1.push(p);
        }
    }
    let starts: Vec<u64> = groups.keys().copied().collect();
    let mut out = Vec::new();
    let mut t = 0u64;
    for (i, (&a, (b, ps))) in groups.iter().enumerate() {
        if a > t {
            out.push(Event {
                start: t,
                len: a - t,
                pitches: vec![],
            });
        }
        let next = starts.get(i + 1).copied().unwrap_or(u64::MAX);
        let e = (*b).min(next).max(a + 1);
        let mut ps = ps.clone();
        ps.sort_unstable();
        out.push(Event {
            start: a,
            len: e - a,
            pitches: ps,
        });
        t = e;
    }
    if end_units > t {
        out.push(Event {
            start: t,
            len: end_units - t,
            pitches: vec![],
        });
    }
    out
}

fn clef(notes: &[RawNote]) -> (&'static str, u8) {
    let mut ps: Vec<u8> = notes.iter().map(|n| n.2).collect();
    ps.sort_unstable();
    let median = ps.get(ps.len() / 2).copied().unwrap_or(60);
    if median < 57 {
        ("F", 4)
    } else {
        ("G", 2)
    }
}

fn time_xml(m: &BarMeter) -> String {
    let beats = if m.grouping.iter().all(|&g| g == 1) {
        m.num.to_string()
    } else {
        glaux_core::meter::grouping_text(&m.grouping)
    };
    format!(
        "<time><beats>{beats}</beats><beat-type>{}</beat-type></time>",
        m.den
    )
}

/// プロジェクトを MusicXML の文字列にする
pub fn to_musicxml(project: &Project) -> Result<(String, usize), String> {
    let tracks: Vec<(&glaux_core::Track, Vec<RawNote>)> = project
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Midi)
        .map(|t| (t, played_notes(t)))
        .filter(|(_, n)| !n.is_empty())
        .collect();
    if tracks.is_empty() {
        return Err("書き出す MIDI の音がありません".to_owned());
    }
    let end = tracks
        .iter()
        .flat_map(|(_, n)| n.iter().map(|x| x.1))
        .max()
        .unwrap_or(0);
    let mut meters = bar_meters(project, end);
    meters.retain(|m| m.start < end);
    if meters.is_empty() {
        return Err("小節を数えられません".to_owned());
    }
    let end_units = meters.last().map_or(0, |m| (m.start + m.len) / GRID);
    let fifths = key_fifths(project);
    let tempos = project.tempo_map.events();
    let mut x = String::new();
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n");
    x.push_str("<!DOCTYPE score-partwise PUBLIC \"-//Recordare//DTD MusicXML 4.0 Partwise//EN\" \"http://www.musicxml.org/dtds/partwise.dtd\">\n");
    x.push_str("<score-partwise version=\"4.0\">\n");
    let _ = writeln!(
        x,
        "  <work><work-title>{}</work-title></work>",
        esc(&project.meta.title)
    );
    x.push_str(
        "  <identification><encoding><software>Glaux</software></encoding></identification>\n",
    );
    x.push_str("  <part-list>\n");
    for (i, (t, _)) in tracks.iter().enumerate() {
        let _ = writeln!(
            x,
            "    <score-part id=\"P{}\"><part-name>{}</part-name></score-part>",
            i + 1,
            esc(&t.name)
        );
    }
    x.push_str("  </part-list>\n");
    for (pi, (_, notes)) in tracks.iter().enumerate() {
        let _ = writeln!(x, "  <part id=\"P{}\">", pi + 1);
        let events = voice(notes, end_units);
        let (sign, line) = clef(notes);
        let mut ei = 0usize;
        // 出来事の中で、今の小節より前に書いた長さ(小節をまたぐ音の残り)
        let mut carried = 0u64;
        for (mi, m) in meters.iter().enumerate() {
            let m0 = m.start / GRID;
            let m1 = (m.start + m.len) / GRID;
            let _ = writeln!(x, "    <measure number=\"{}\">", mi + 1);
            let changed = mi == 0
                || meters[mi - 1].num != m.num
                || meters[mi - 1].den != m.den
                || meters[mi - 1].grouping != m.grouping;
            if mi == 0 {
                let _ = writeln!(
                    x,
                    "      <attributes><divisions>{DIV}</divisions><key><fifths>{fifths}</fifths></key>{}<clef><sign>{sign}</sign><line>{line}</line></clef></attributes>",
                    time_xml(m)
                );
            } else if changed {
                let _ = writeln!(x, "      <attributes>{}</attributes>", time_xml(m));
            }
            // この小節の中で始まるテンポの変更(小節の頭に書く)
            if let Some(te) = tempos
                .iter()
                .rev()
                .find(|e| e.tick.0 >= m.start && e.tick.0 < m.start + m.len)
            {
                if pi == 0 {
                    let bpm = (te.bpm * 100.0).round() / 100.0;
                    let _ = writeln!(
                        x,
                        "      <direction placement=\"above\"><direction-type><metronome><beat-unit>quarter</beat-unit><per-minute>{bpm}</per-minute></metronome></direction-type><sound tempo=\"{bpm}\"/></direction>"
                    );
                }
            }
            let mut t = m0;
            while t < m1 && ei < events.len() {
                let e = &events[ei];
                let e_end = e.start + e.len;
                let seg_end = e_end.min(m1);
                let seg = seg_end - t;
                let tie_before = carried > 0;
                let tie_after = seg_end < e_end;
                let parts = split_units(seg);
                for (k, &u) in parts.iter().enumerate() {
                    let start_tie = tie_before || k > 0;
                    let stop_tie = tie_after || k + 1 < parts.len();
                    let (ty, dot) = note_type(u).unwrap_or(("32nd", false));
                    if e.pitches.is_empty() {
                        let _ = write!(
                            x,
                            "      <note><rest/><duration>{u}</duration><type>{ty}</type>"
                        );
                        if dot {
                            x.push_str("<dot/>");
                        }
                        x.push_str("</note>\n");
                        continue;
                    }
                    for (ci, &p) in e.pitches.iter().enumerate() {
                        let (step, alter, oct) = spell(p, fifths);
                        x.push_str("      <note>");
                        if ci > 0 {
                            x.push_str("<chord/>");
                        }
                        let _ = write!(x, "<pitch><step>{step}</step>");
                        if alter != 0 {
                            let _ = write!(x, "<alter>{alter}</alter>");
                        }
                        let _ = write!(x, "<octave>{oct}</octave></pitch><duration>{u}</duration>");
                        if start_tie {
                            x.push_str("<tie type=\"stop\"/>");
                        }
                        if stop_tie {
                            x.push_str("<tie type=\"start\"/>");
                        }
                        let _ = write!(x, "<type>{ty}</type>");
                        if dot {
                            x.push_str("<dot/>");
                        }
                        if start_tie || stop_tie {
                            x.push_str("<notations>");
                            if start_tie {
                                x.push_str("<tied type=\"stop\"/>");
                            }
                            if stop_tie {
                                x.push_str("<tied type=\"start\"/>");
                            }
                            x.push_str("</notations>");
                        }
                        x.push_str("</note>\n");
                    }
                }
                t = seg_end;
                if seg_end >= e_end {
                    ei += 1;
                    carried = 0;
                } else {
                    carried += seg;
                }
            }
            x.push_str("    </measure>\n");
        }
        x.push_str("  </part>\n");
    }
    x.push_str("</score-partwise>\n");
    Ok((x, tracks.len()))
}

/// 書き出して、パスとパートの数を返す
pub fn export_file(
    project: &Project,
    project_dir: &Path,
    path: Option<&str>,
) -> Result<Value, String> {
    let (xml, parts) = to_musicxml(project)?;
    let path = path.map(PathBuf::from).unwrap_or_else(|| {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        project_dir.join("export").join(format!(
            "{}_{stamp}.musicxml",
            crate::export::sanitize(&project.meta.title)
        ))
    });
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, xml.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(json!({ "path": path.to_string_lossy(), "parts": parts }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_split_into_standard_values() {
        assert_eq!(split_units(8), vec![8]);
        assert_eq!(split_units(5), vec![4, 1]);
        assert_eq!(split_units(28), vec![24, 4]);
        assert_eq!(note_type(12), Some(("quarter", true)));
        assert!(note_type(5).is_none());
    }

    #[test]
    fn voice_quantizes_chords_and_rests() {
        // C E を 1 拍、休み 1 拍、G を 2 拍(少しずれた位置)
        let notes: Vec<RawNote> = vec![(0, 960, 60, 90), (5, 950, 64, 90), (1930, 3840, 67, 90)];
        let v = voice(&notes, 32);
        assert_eq!(
            v,
            vec![
                Event {
                    start: 0,
                    len: 8,
                    pitches: vec![60, 64]
                },
                Event {
                    start: 8,
                    len: 8,
                    pitches: vec![]
                },
                Event {
                    start: 16,
                    len: 16,
                    pitches: vec![67]
                },
            ]
        );
    }

    #[test]
    fn spells_with_the_key() {
        assert_eq!(spell(61, 2), ("C", 1, 4));
        assert_eq!(spell(61, -3), ("D", -1, 4));
        assert_eq!(spell(60, 0), ("C", 0, 4));
    }
}
