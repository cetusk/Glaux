//! 譜起こしの診断: `cargo run -p glaux-engine --example transcribe_check -- <wav> [bpm] [frames]`
//! ノート列(時刻と拍位置)と、`frames` 指定時はフレームごとのピッチ・音量を表示する。
use glaux_engine::transcribe::{debug_frames, transcribe_mono, TranscribeOptions};

fn name(p: f32) -> String {
    const N: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let r = p.round() as i32;
    format!("{}{}", N[(r.rem_euclid(12)) as usize], r / 12 - 1)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("wav");
    let bpm: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(120.0);
    let show_frames = args.next().is_some();
    let data = glaux_engine::load_wav_mono(std::path::Path::new(&path)).unwrap();
    let opts = TranscribeOptions::default();
    let beat = 60.0 / bpm;
    let pos = |t: f64| {
        let b = t / beat;
        format!(
            "{}小節 {:.2}拍",
            (b / 4.0).floor() as i64 + 1,
            b % 4.0 + 1.0
        )
    };
    if show_frames {
        for (t, m, db) in debug_frames(&data, &opts) {
            let bar = "#".repeat(((db + 60.0).max(0.0) / 2.0) as usize);
            println!(
                "{t:6.2}s {:>5} {:>6} {db:6.1}dB {bar}",
                m.map(name).unwrap_or_else(|| "-".into()),
                m.map(|m| format!("{m:.2}")).unwrap_or_default()
            );
        }
    }
    let notes = transcribe_mono(&data, &opts);
    println!("---- {} ノート ----", notes.len());
    for n in &notes {
        println!(
            "{:6.2}〜{:6.2}s ({:4.0}ms) {:>4} vel{:3} {}{}",
            n.start_sec,
            n.end_sec,
            (n.end_sec - n.start_sec) * 1000.0,
            name(n.pitch as f32),
            n.vel,
            pos(n.start_sec),
            if n.reattack { " [再アタック]" } else { "" }
        );
    }
}
