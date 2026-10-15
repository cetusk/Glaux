//! ドラムの譜起こしの調整用: recon_eval が書き出したドラムの音(RECON_DRUMS)と正解で、種類ごとの F 値を出す。
//! `cargo run --release -p glaux-engine --example drum_tune -- <フォルダ> [threshold] [snare_vs_hat_db] [cymbal_secs]`
//! 環境変数 DRUM_MIX=1 で曲そのもの(分けていない音)から、DRUM_SHOW=曲名 で 1 曲の正解と推定を並べる。

use glaux_engine::drum_transcribe::{transcribe, DrumKind, DrumOptions};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(&args[0]);
    let mixed = std::env::var("DRUM_MIX").is_ok();
    let mut opts = if mixed {
        DrumOptions::mixed()
    } else {
        DrumOptions::default()
    };
    if let Some(v) = args.get(1).and_then(|v| v.parse().ok()) {
        opts.threshold = v;
    }
    if let Some(v) = args.get(2).and_then(|v| v.parse().ok()) {
        opts.snare_vs_hat_db = v;
    }
    if let Some(v) = args.get(3).and_then(|v| v.parse().ok()) {
        opts.cymbal_secs = v;
    }
    let classes: [(&str, &[u8], &[DrumKind]); 4] = [
        ("kick", &[35, 36], &[DrumKind::Kick]),
        ("snare", &[37, 38, 39, 40], &[DrumKind::Snare]),
        (
            "hihat",
            &[42, 44, 46],
            &[DrumKind::ClosedHat, DrumKind::OpenHat],
        ),
        ("cymbal", &[49, 51, 52, 55, 57, 59], &[DrumKind::Cymbal]),
    ];
    let mut sums: std::collections::BTreeMap<String, (f64, usize, usize, usize, usize)> =
        Default::default();
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().is_some_and(|e| e == "wav") && !p.to_string_lossy().ends_with(".mix.wav")
        })
        .collect();
    files.sort();
    for wav in files {
        let truth: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(wav.with_extension("json")).unwrap())
                .unwrap();
        let device = truth["device"].as_str().unwrap_or("").to_owned();
        let group = if device.starts_with("sf2") {
            "sf2"
        } else {
            "builtin"
        };
        // DRUM_MIX があれば、ドラムだけの音の代わりに曲そのもの(<名前>.mix.wav)から起こす
        let src = if mixed {
            wav.with_extension("mix.wav")
        } else {
            wav.clone()
        };
        let d = glaux_engine::load_wav_mono(&src).unwrap();
        // 本番と同じく、打楽器の成分を取り出してから
        let perc = glaux_engine::separate::hpss(&d.frames).percussive;
        let hits = transcribe(&perc, d.sample_rate, &opts);
        if std::env::var("DRUM_SHOW").is_ok_and(|v| wav.to_string_lossy().contains(&v)) {
            let mut t: Vec<(f64, u64)> = truth["truth"]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| (n["onset"].as_f64().unwrap(), n["pitch"].as_u64().unwrap()))
                .collect();
            t.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (time, p) in t.iter().filter(|x| x.0 < 60.0) {
                println!("正解 {time:.3} {p}");
            }
            for h in hits.iter().filter(|h| h.time < 60.0) {
                println!("推定 {:.3} {:?} v{}", h.time, h.kind, h.vel);
            }
        }
        for (name, pitches, kinds) in classes {
            let mut t: Vec<f64> = truth["truth"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|n| pitches.contains(&(n["pitch"].as_u64().unwrap() as u8)))
                .map(|n| n["onset"].as_f64().unwrap())
                .collect();
            if t.is_empty() {
                continue;
            }
            t.sort_by(f64::total_cmp);
            t.dedup_by(|a, b| (*a - *b).abs() < 0.01);
            let e: Vec<f64> = hits
                .iter()
                .filter(|h| kinds.contains(&h.kind))
                .map(|h| h.time)
                .collect();
            let hit = glaux_engine::mir_eval::match_events(&t, &e, 0.05);
            let f = 2.0 * hit as f64 / (t.len() + e.len()) as f64;
            for key in [format!("{name} {group}"), format!("{name} all")] {
                let s = sums.entry(key).or_default();
                s.0 += f;
                s.1 += 1;
                s.2 += t.len();
                s.3 += e.len();
                s.4 += hit;
            }
        }
    }
    for (k, (f, n, t, e, h)) in sums {
        println!(
            "{k:<16} F {:.3}  (n {n}, 正解 {t}, 推定 {e}, 当たり {h})",
            f / n as f64
        );
    }
}
