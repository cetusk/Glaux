//! 再生負荷の計測: 現実的な編成を 16 小節レンダリングし、1 ブロックあたりの処理時間を
//! 実時間(ブロックの長さ)と比べる。`cargo run -p glaux-engine --example render_bench`
//!
//! 出力の「負荷」はオーディオコールバックに使える時間のうち何 % を使ったか
//! (平均と最悪ブロック)。最悪が 100% に近いと音切れ・カクつきになる。

use glaux_core::{
    Clip, ClipId, Device, Effect, FxId, Note, NoteId, PluginSource, Project, Tick, Track, TrackId,
    TrackKind,
};
use glaux_engine::render::{Renderer, Shared};
use glaux_engine::{build_playback_data, SampleBank};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

const BARS: u64 = 16;
const BLOCK: usize = 1024;
const SR: f64 = 48_000.0;

fn note(pos: u64, dur: u64, pitch: u8, vel: u8) -> Note {
    Note {
        id: NoteId::new(),
        pos: Tick(pos),
        dur: Tick(dur),
        pitch,
        vel,
        articulation: Default::default(),
        pitch_curve: vec![],
    }
}

fn track(name: &str, device: Device, notes: Vec<Note>, fx: &[&str]) -> Track {
    let mut t = Track::new(TrackId::new(), name, TrackKind::Midi);
    t.device = Some(device);
    for f in fx {
        t.effects.push(Effect::builtin(FxId::new(), *f));
    }
    let mut clip = Clip::new_midi(ClipId::new(), name, Tick(0), Tick(3840 * BARS));
    *clip.notes_mut().unwrap() = notes;
    t.clips.push(clip);
    t
}

fn sf2(bank: u16, preset: u16) -> Device {
    Device {
        source: PluginSource::Sf2 {
            soundfont: "FluidR3_GM.sf2".into(),
            bank,
            preset,
        },
        params: Default::default(),
    }
}

fn project(which: &str) -> Project {
    let mut p = Project::new("bench");
    let bars = 0..BARS;
    let chords = [
        [60u8, 64, 67, 71],
        [57, 60, 64, 67],
        [53, 57, 60, 64],
        [55, 59, 62, 65],
    ];
    let chord_notes = |dur: u64| -> Vec<Note> {
        bars.clone()
            .flat_map(|b| {
                chords[(b % 4) as usize]
                    .iter()
                    .map(move |&k| note(b * 3840, dur, k, 90))
            })
            .collect()
    };
    let eighths = |base: u8| -> Vec<Note> {
        bars.clone()
            .flat_map(|b| {
                (0..8).map(move |i| note(b * 3840 + i * 480, 440, base + (i % 3) as u8, 100))
            })
            .collect()
    };
    let drums: Vec<Note> = bars
        .clone()
        .flat_map(|b| {
            let mut v = Vec::new();
            for i in 0..8 {
                v.push(note(b * 3840 + i * 480, 200, 42, 80));
            }
            v.push(note(b * 3840, 400, 36, 110));
            v.push(note(b * 3840 + 1920, 400, 36, 110));
            v.push(note(b * 3840 + 960, 400, 38, 100));
            v.push(note(b * 3840 + 2880, 400, 38, 100));
            v
        })
        .collect();

    let use_sf2 = which == "all" || which == "sf2";
    let use_synth = which == "all" || which == "synth";
    if use_sf2 {
        p.tracks
            .push(track("Piano", sf2(0, 0), chord_notes(3600), &["reverb"]));
        p.tracks
            .push(track("Strings", sf2(0, 48), chord_notes(3840), &["reverb"]));
        p.tracks
            .push(track("Bass", sf2(0, 33), eighths(36), &["compressor"]));
        p.tracks
            .push(track("Drums", sf2(128, 0), drums.clone(), &["compressor"]));
    }
    if use_synth {
        let mut saw = Device::builtin("subtractive");
        saw.params.insert("unison".into(), 7.0.into());
        saw.params.insert("detune".into(), 25.0.into());
        p.tracks
            .push(track("Supersaw", saw, chord_notes(3600), &["eq", "reverb"]));
        p.tracks.push(track(
            "Guitar",
            Device::builtin("pluck"),
            eighths(52),
            &["amp", "reverb"],
        ));
        p.tracks
            .push(track("Kit", Device::builtin("drum"), drums, &[]));
    }
    p
}

fn bench(label: &str, p: &Project, bank: &SampleBank, metronome: bool) {
    let data = build_playback_data(p, SR, bank);
    let total = data.end_sample as usize;
    let shared = Arc::new(Shared::new(data));
    shared.playing.store(true, Ordering::Release);
    shared.metronome.store(metronome, Ordering::Release);
    let mut r = Renderer::new(shared);
    let mut buf = vec![0.0f32; BLOCK * 2];
    let budget = BLOCK as f64 / SR;
    let (mut sum, mut worst, mut n, mut worst_at) = (0.0f64, 0.0f64, 0usize, 0usize);
    let mut spikes = 0usize;
    let mut rendered = 0;
    while rendered < total {
        let t = Instant::now();
        r.process(&mut buf, 2);
        let dt = t.elapsed().as_secs_f64();
        sum += dt;
        if dt > worst {
            worst = dt;
            worst_at = n;
        }
        if dt > budget * 0.5 {
            spikes += 1;
        }
        n += 1;
        rendered += BLOCK;
    }
    println!(
        "{label:<28} 平均 {:>5.1}%  最悪 {:>6.1}%(#{worst_at})  50% 超 {spikes} 回  ({} ブロック)",
        sum / n as f64 / budget * 100.0,
        worst / budget * 100.0,
        n
    );
}

fn main() {
    let arg = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "SoundFonts".into());
    // 実プロジェクト(.glaux フォルダ)を渡したら全体とトラック別に測る
    if arg.ends_with(".glaux") || arg.ends_with(".glaux/") {
        let sf2_dir = std::env::args()
            .nth(2)
            .unwrap_or_else(|| "SoundFonts".into());
        let dir = std::path::Path::new(&arg);
        let json = std::fs::read_to_string(dir.join("project.json")).expect("project.json");
        let p = Project::from_json(&json).expect("parse");
        let mut bank = SampleBank::default().with_sf2_dir(&sf2_dir);
        bank.sync(&p, dir);
        println!("{} トラック", p.tracks.len());
        bench("(全体)", &p, &bank, false);
        for i in 0..p.tracks.len() {
            let mut solo = p.clone();
            let t = solo.tracks[i].clone();
            solo.tracks = vec![t];
            let name = format!(
                "{} [{}] fx{}",
                solo.tracks[0].name,
                solo.tracks[0]
                    .device
                    .as_ref()
                    .map(|d| match &d.source {
                        PluginSource::Builtin { name } => name.clone(),
                        PluginSource::Sf2 { preset, bank, .. } => format!("sf2 {bank}:{preset}"),
                        _ => "other".into(),
                    })
                    .unwrap_or_else(|| "subtractive*".into()),
                solo.tracks[0].effects.len()
            );
            bench(&name, &solo, &bank, false);
        }
        return;
    }
    for which in ["synth", "sf2", "all"] {
        let p = project(which);
        let mut bank = SampleBank::default().with_sf2_dir(&arg);
        bank.sync(&p, std::path::Path::new("."));
        bench(which, &p, &bank, false);
        if which == "all" {
            bench("all + metronome", &p, &bank, true);
        }
    }
}
