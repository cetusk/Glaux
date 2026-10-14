//! Godot で鳴らして録った音(`godot/demo/tools/record_song.gd`)と、アプリの書き出しと同じ描き出しを比べる。
//! 同じ曲がゲームでもアプリと同じ音で鳴るかの確かめ(godot/check_render.sh から使う)。
//!
//! `cargo run -p glaux-engine --release --example render_compare -- <曲のフォルダ> <録った WAV> [--save <描き出した WAV>]`
//!
//! ゲームと同じ条件にするため、CLAP の音源のトラックは無音にし、ソロは外して描き出す(Glaux for Godot の読み込みと同じ)。
//! 頭の位置は、音の出だしと相互相関で合わせる。差の大きさ(dB、元の音に対する差の比)を全体と 1 秒ごとの最悪で出す。
//! Godot は再生の頭を約 12ms かけて小さい音から始めるので、頭の 1024 フレームは比べない。
//! 合格の目安は全体 -40 dB 以下(16bit で録るので、まったく同じでも -90 dB 前後の差は残る)

use glaux_core::{PluginSource, Project};
use glaux_engine::export::render_project;
use glaux_engine::SampleBank;
use std::path::Path;

/// 合格とみなす差の大きさ(dB)
const PASS_DB: f64 = -40.0;
/// 比べ始める位置(フレーム)。Godot の再生の頭の小さい音から始まる所を外す
const START_FADE: usize = 1024;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("使い方: render_compare <曲のフォルダ> <録った WAV> [--save <描き出した WAV>]");
        std::process::exit(2);
    }
    let dir = Path::new(&args[0]);
    let save = args
        .iter()
        .position(|a| a == "--save")
        .and_then(|i| args.get(i + 1));

    let (godot, rate) = read_wav(Path::new(&args[1]));
    let json =
        std::fs::read_to_string(dir.join("project.json")).expect("project.json を読めません");
    let mut project = Project::from_json(&json).expect("project.json の形が違います");
    for t in &mut project.tracks {
        t.solo = false;
        if t.device
            .as_ref()
            .is_some_and(|d| matches!(d.source, PluginSource::Clap { .. }))
        {
            t.mute = true;
        }
    }
    let mut bank = SampleBank::default();
    bank.sync(&project, dir);
    let app = match render_project(&project, rate as f64, &bank) {
        Ok(a) => a,
        Err(glaux_engine::export::ExportError::Empty) => {
            println!("RESULT skip 曲にノート・音声が無いので比べません(効果音用の曲など)");
            return;
        }
        Err(e) => panic!("描き出せません: {e}"),
    };
    if let Some(p) = save {
        write_wav(Path::new(p), &app, rate);
    }

    // 音の出だしで大まかに合わせ、相互相関で細かく合わせる
    let (ga, aa) = (onset(&godot), onset(&app));
    let rough = ga as i64 - aa as i64;
    let lag = refine(&godot, &app, rough, 2048, rate as usize * 2);
    let start_g = lag.max(0) as usize;
    let start_a = (-lag).max(0) as usize;
    let n =
        ((godot.len() / 2).saturating_sub(start_g)).min((app.len() / 2).saturating_sub(start_a));
    if n < rate as usize / 2 {
        println!(
            "RESULT ng 重なる長さが短すぎます(Godot {:.1}s / アプリ {:.1}s)",
            godot.len() as f64 / 2.0 / rate as f64,
            app.len() as f64 / 2.0 / rate as f64
        );
        std::process::exit(1);
    }
    let (mut sig, mut diff, mut peak) = (0.0f64, 0.0f64, 0.0f64);
    let sec = rate as usize;
    let mut worst = (f64::NEG_INFINITY, 0usize);
    // Godot は再生の頭を約 512 サンプルかけて小さい音から始める(プチッを防ぐ Godot の動き)ので、そこは比べない
    for s0 in (START_FADE..n).step_by(sec) {
        let s1 = (s0 + sec).min(n);
        let (mut ss, mut dd) = (0.0f64, 0.0f64);
        for i in s0..s1 {
            for c in 0..2 {
                let g = godot[(start_g + i) * 2 + c] as f64;
                let a = app[(start_a + i) * 2 + c] as f64;
                ss += a * a;
                dd += (g - a) * (g - a);
                peak = peak.max((g - a).abs());
            }
        }
        sig += ss;
        diff += dd;
        // ごく小さい音(-40 dBFS 未満。余韻の終わりなど)は 16bit の粗さが目立つので、1 秒ごとの最悪には入れない
        if ss / ((s1 - s0) * 2) as f64 > 1e-4 {
            let db = 10.0 * (dd.max(1e-20) / ss).log10();
            if db > worst.0 {
                worst = (db, s0 / sec);
            }
        }
    }
    let total_db = 10.0 * (diff.max(1e-20) / sig.max(1e-20)).log10();
    let ok = total_db <= PASS_DB && worst.0 <= PASS_DB + 10.0;
    println!(
        "RESULT {} 差 {:.1} dB(1 秒ごとの最悪 {:.1} dB @ {} 秒)・最大の差 {:.5}・比べた長さ {:.1} 秒・ずれ {} サンプル・{} Hz",
        if ok { "ok" } else { "ng" },
        total_db,
        worst.0,
        worst.1,
        peak,
        n as f64 / rate as f64,
        lag,
        rate
    );
    if !ok {
        std::process::exit(1);
    }
}

/// 最初に音が出た位置(フレーム)
fn onset(x: &[f32]) -> usize {
    x.chunks(2)
        .position(|f| f[0].abs().max(f[1].abs()) > 1e-3)
        .unwrap_or(0)
}

/// `around` の前後 `span` フレームで、`len` フレームぶんの相互相関がいちばん大きいずれ(godot の位置 - app の位置)
fn refine(godot: &[f32], app: &[f32], around: i64, span: i64, len: usize) -> i64 {
    let mono = |x: &[f32], i: i64| -> f64 {
        if i < 0 || (i as usize) * 2 + 1 >= x.len() {
            0.0
        } else {
            (x[i as usize * 2] + x[i as usize * 2 + 1]) as f64
        }
    };
    let a0 = onset(app) as i64;
    let mut best = (f64::NEG_INFINITY, around);
    for lag in around - span..=around + span {
        let mut s = 0.0;
        for i in (0..len as i64).step_by(2) {
            s += mono(app, a0 + i) * mono(godot, a0 + i + lag);
        }
        if s > best.0 {
            best = (s, lag);
        }
    }
    best.1
}

fn read_wav(path: &Path) -> (Vec<f32>, u32) {
    let mut r = hound::WavReader::open(path).expect("WAV を読めません");
    let spec = r.spec();
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().map(|s| s.unwrap_or(0.0)).collect(),
        hound::SampleFormat::Int => {
            let full = (1i64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>()
                .map(|s| s.unwrap_or(0) as f32 / full)
                .collect()
        }
    };
    let out = if spec.channels == 1 {
        raw.iter().flat_map(|&v| [v, v]).collect()
    } else {
        raw.chunks(spec.channels as usize)
            .flat_map(|f| [f[0], f[1.min(f.len() - 1)]])
            .collect()
    };
    (out, spec.sample_rate)
}

fn write_wav(path: &Path, x: &[f32], rate: u32) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec).expect("WAV を書けません");
    for &s in x {
        w.write_sample(s).expect("WAV を書けません");
    }
    w.finalize().expect("WAV を書けません");
}
