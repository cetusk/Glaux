//! 楽器の見分けの見本を作る: GM の SoundFont の音色で短いフレーズを鳴らし、CLAP の埋め込みを求めて JSON に書き出す。
//! `cargo run --release -p glaux-mcp --example instrument_protos -- <出力の json>`
//! (GM の SoundFont〈GeneralUser GS〉を Glaux の SoundFont の置き場所に、CLAP の音声モデルを models に置いておく)
//!
//! フレーズは種類ごとに変える: 伸ばす楽器(パッド・ストリングス・オルガン・声)は和音を 2 拍ずつ、
//! 旋律の楽器(リード・ブラス・管)は 8 分の旋律、弾く楽器(ピアノ・エレピ・ギター・プラック・ベル)は 8 分の分散和音。
//! 音域は C3〜C6。曲の音と比べるので、和音は C・Am・F・G を 2 小節ずつ。

use glaux_core::time::{TempoEvent, TempoMap, PPQ};
use glaux_core::{
    Clip, ClipContent, ClipId, Device, Note, NoteId, PluginSource, Project, Tick, Track, TrackId,
    TrackKind,
};
use glaux_mcp::instruments::InstrumentClass as C;

fn note(pos: u64, dur: u64, pitch: u8, vel: u8) -> Note {
    Note {
        locked: false,
        id: NoteId::new(),
        pos: Tick(pos),
        dur: Tick(dur.max(30)),
        pitch,
        vel,
        articulation: Default::default(),
        pitch_curve: vec![],
        glide_ms: None,
        vibrato: None,
        volume_curve: vec![],
        brightness_curve: vec![],
        condition: None,
    }
}

/// GM の音色の番号 → 種類(打楽器・効果音の一部は除く)
fn class_of(p: u16) -> Option<C> {
    Some(match p {
        0..=3 | 6 => C::Piano,
        4 | 5 => C::Epiano,
        7 => C::Pluck,
        8..=14 => C::Bell,
        16..=21 | 23 => C::Organ,
        22 => C::Wind,
        24..=31 | 46 | 104..=107 => C::Guitar,
        32..=39 => C::Bass,
        40..=44 | 48..=51 => C::Strings,
        45 => C::Pluck,
        52..=54 => C::Voice,
        55 | 96..=103 | 120..=127 => C::Fx,
        56..=63 => C::Brass,
        64..=79 => C::Wind,
        80..=87 => C::Lead,
        88..=95 => C::Pad,
        _ => return None,
    })
}

fn phrase(c: C) -> Vec<Note> {
    let q = PPQ;
    let chords: [[u8; 4]; 4] = [
        [48, 60, 64, 67],
        [45, 57, 60, 64],
        [41, 57, 60, 65],
        [43, 55, 59, 62],
    ];
    let mut v = Vec::new();
    for bar in 0..8u64 {
        let ch = chords[(bar / 2) as usize % 4];
        let t0 = bar * 4 * q;
        match c {
            C::Pad | C::Strings | C::Organ | C::Voice => {
                for half in 0..2 {
                    for &p in &ch[1..] {
                        v.push(note(t0 + half * 2 * q, 2 * q - 20, p, 90));
                    }
                }
            }
            C::Lead | C::Brass | C::Wind => {
                let line = [
                    ch[3] + 12,
                    ch[2] + 12,
                    ch[1] + 12,
                    ch[3] + 12,
                    ch[3] + 14,
                    ch[3] + 12,
                    ch[2] + 12,
                    ch[1] + 12,
                ];
                for (i, p) in line.iter().enumerate() {
                    v.push(note(t0 + i as u64 * q / 2, q / 2 - 20, (*p).min(84), 100));
                }
            }
            C::Bass => {
                for i in 0..8 {
                    v.push(note(t0 + i * q / 2, q / 2 - 40, ch[0] - 12, 100));
                }
            }
            _ => {
                // 弾く楽器: 分散和音
                let arp = [
                    ch[1],
                    ch[2],
                    ch[3],
                    ch[2] + 12,
                    ch[3],
                    ch[2],
                    ch[1] + 12,
                    ch[3],
                ];
                for (i, p) in arp.iter().enumerate() {
                    v.push(note(t0 + i as u64 * q / 2, q / 2, *p, 95));
                }
            }
        }
    }
    v
}

fn main() {
    let out = std::env::args().nth(1).expect("出力の json を指定");
    let sf = "GeneralUser-GS.sf2";
    let mut rows = Vec::new();
    for p in 0..128u16 {
        let Some(c) = class_of(p) else {
            continue;
        };
        let mut project = Project::new("proto");
        project.tempo_map = TempoMap::new(vec![TempoEvent {
            tick: Tick(0),
            bpm: 120.0,
        }])
        .expect("テンポ");
        let mut t = Track::new(TrackId::new(), "proto", TrackKind::Midi);
        t.device = Some(Device {
            source: PluginSource::Sf2 {
                soundfont: sf.to_owned(),
                bank: 0,
                preset: p,
            },
            params: Default::default(),
        });
        let mut clip = Clip::new_midi(ClipId::new(), "proto", Tick(0), Tick(8 * 4 * PPQ));
        if let ClipContent::Midi { notes, .. } = &mut clip.content {
            *notes = phrase(c);
        }
        t.clips.push(clip);
        project.tracks.push(t);
        let mut bank = glaux_engine::data::SampleBank::default()
            .with_sf2_dir(glaux_engine::sf2::default_dir());
        bank.sync(&project, std::path::Path::new("."));
        let st = glaux_engine::export::render_project(&project, 48_000.0, &bank).expect("書き出し");
        let mono: Vec<f32> = st.chunks(2).map(|c| (c[0] + c[1]) * 0.5).collect();
        // 頭の 10 秒(和音が 2 つ半)と、後ろの 10 秒の 2 つ
        for (k, seg) in [
            &mono[..mono.len().min(480_000)],
            &mono[mono.len().saturating_sub(480_000)..],
        ]
        .iter()
        .enumerate()
        {
            let e = glaux_ml::clap::embed(seg, 48_000.0).expect("埋め込み");
            rows.push(serde_json::json!({"class": c, "program": p, "part": k, "embedding": e}));
        }
        eprintln!("{p:3} {c:?}");
    }
    std::fs::write(&out, serde_json::to_string(&rows).expect("json")).expect("書き出し");
    eprintln!("{} 個 → {out}", rows.len());
}
