//! 本物のエンジン(出力は ALSA の null など)に、アプリが送る操作と問い合わせをでたらめな順で送り続け、
//! debug ビルドで panic しないかを確かめる。
//!
//! `cargo run -p glaux-engine --example engine_fuzz -- <曲.glaux>... [--secs N] [--seed S]`

use glaux_core::Project;
use glaux_engine::ab::{AbClip, AbSide};
use glaux_engine::monitor::{MonitorMode, Speaker};
use std::path::PathBuf;
use std::time::{Duration, Instant};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn chance(&mut self, p: f64) -> bool {
        (self.next() % 10_000) as f64 / 10_000.0 < p
    }
}

fn main() {
    let mut songs = Vec::new();
    let mut secs = 30u64;
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--secs" => secs = args.next().and_then(|s| s.parse().ok()).unwrap_or(secs),
            "--seed" => seed = args.next().and_then(|s| s.parse().ok()).unwrap_or(seed),
            _ => songs.push(PathBuf::from(a)),
        }
    }
    // panic はどのスレッドでも即座に知らせて終える(オーディオスレッドの panic を見逃さない)
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        prev(info);
        eprintln!("FUZZ-PANIC");
        std::process::exit(101);
    }));

    let projects: Vec<(Project, PathBuf)> = songs
        .iter()
        .map(|d| {
            let json = std::fs::read_to_string(d.join("project.json")).expect("project.json");
            (Project::from_json(&json).expect("読めない曲"), d.clone())
        })
        .collect();
    let engine = glaux_engine::start_engine().expect("エンジンを起動できない");
    println!(
        "出力: {} / {} Hz",
        engine.output_device(),
        engine.sample_rate()
    );

    let mut rng = Rng(seed | 1);
    let start = Instant::now();
    let mut ops = 0u64;
    let mut cur = 0usize;
    let mut tracks = 0usize;
    let mut log: Vec<String> = Vec::new();
    while start.elapsed() < Duration::from_secs(secs) {
        let op = rng.below(26);
        let tick = |rng: &mut Rng| match rng.below(4) {
            0 => 0,
            1 => rng.below(4),
            2 => rng.below(200_000),
            _ => rng.below(u32::MAX as u64 * 4),
        };
        let name = match op {
            0 if !projects.is_empty() => {
                cur = rng.below(projects.len() as u64) as usize;
                let (p, d) = &projects[cur];
                tracks = p.tracks.len();
                engine.set_project(p, d);
                "set_project"
            }
            1 => {
                engine.play();
                "play"
            }
            2 => {
                engine.pause();
                "pause"
            }
            3 => {
                engine.stop();
                "stop"
            }
            4 => {
                engine.seek_tick(glaux_core::Tick(tick(&mut rng)));
                "seek"
            }
            5 => {
                let (a, b) = (tick(&mut rng), tick(&mut rng));
                let _ = engine.set_loop(glaux_core::Tick(a), glaux_core::Tick(b));
                "set_loop"
            }
            6 => {
                engine.clear_loop();
                "clear_loop"
            }
            7 => {
                let t = rng.below(tracks as u64 + 3) as usize;
                let dur = [0u16, 1, 200, u16::MAX][rng.below(4) as usize];
                engine.preview_note(t, rng.below(128) as u8, rng.below(128) as u8, dur);
                "preview_note"
            }
            8 => {
                engine.live_note_on(
                    rng.below(tracks as u64 + 3) as usize,
                    rng.below(128) as u8,
                    rng.below(128) as u8,
                );
                "live_note_on"
            }
            9 => {
                engine.live_note_off(rng.below(128) as u8);
                "live_note_off"
            }
            10 => {
                engine.live_all_off();
                "live_all_off"
            }
            11 => {
                let m = [
                    MonitorMode::Stereo,
                    MonitorMode::Mono,
                    MonitorMode::Side,
                    MonitorMode::Swap,
                ][rng.below(4) as usize];
                engine.set_monitor(m, rng.chance(0.5));
                "set_monitor"
            }
            12 => {
                let s = [
                    Speaker::Off,
                    Speaker::Phone,
                    Speaker::Laptop,
                    Speaker::Front,
                ][rng.below(4) as usize];
                engine.set_speaker(s);
                "set_speaker"
            }
            13 => {
                let _ = engine.loudness();
                "loudness"
            }
            14 => {
                let _ = engine.spectrum();
                "spectrum"
            }
            15 => {
                let _ = engine.scope_points();
                let _ = engine.correlation();
                "scope"
            }
            16 => {
                engine.reset_loudness();
                "reset_loudness"
            }
            17 => {
                let _ = engine.take_levels();
                let _ = engine.take_loads();
                let _ = engine.take_stats();
                "levels"
            }
            18 => {
                let _ = engine.playhead_tick();
                let _ = engine.is_playing();
                let _ = engine.loop_region();
                "playhead"
            }
            19 => {
                engine.set_metronome(rng.chance(0.5));
                engine.set_click_only(rng.chance(0.1));
                "metronome"
            }
            20 => {
                let n = rng.below(4) as usize;
                let len = [0usize, 1, 4800, 96_000][rng.below(4) as usize];
                let clip = AbClip {
                    start: tick(&mut rng),
                    takes: (0..n).map(|_| vec![0.1; len * 2]).collect(),
                    gains: vec![1.0; n],
                };
                engine.set_ab_clip(if rng.chance(0.3) { None } else { Some(clip) });
                "set_ab_clip"
            }
            21 => {
                let s = if rng.chance(0.3) {
                    AbSide::Off
                } else {
                    AbSide::Take(rng.below(7) as u8)
                };
                engine.set_ab_side(s);
                "set_ab_side"
            }
            22 => {
                engine.set_output_volume_db([-120.0, -6.0, 0.0, 12.0][rng.below(4) as usize]);
                "volume"
            }
            23 if rng.chance(0.2) => {
                // 出力を開き直す(デバイスの切り替え・Bluetooth のつなぎ直しと同じく、レンダラが作り直される)。
                // 直後の問い合わせは、まだ数えた量が少ない
                let f = [32u32, 128, 512, 4096][rng.below(4) as usize];
                let _ = engine.set_buffer_frames(f);
                let _ = engine.spectrum();
                let _ = engine.loudness();
                let _ = engine.scope_points();
                "set_buffer_frames"
            }
            24 if rng.chance(0.1) && !projects.is_empty() => {
                // ミキサーの操作のような小さな変更(ミュート・ソロ・音量)を当てて組み直す
                let (p, d) = &projects[cur];
                let mut p = p.clone();
                for t in p.tracks.iter_mut() {
                    t.mute = rng.chance(0.2);
                    t.solo = rng.chance(0.1);
                    t.volume_db = [-60.0, -6.0, 0.0, 6.0][rng.below(4) as usize];
                }
                engine.set_project(&p, d);
                "mix_change"
            }
            _ => {
                std::thread::sleep(Duration::from_millis(rng.below(30)));
                "sleep"
            }
        };
        ops += 1;
        log.push(name.into());
        if log.len() > 40 {
            log.remove(0);
        }
        if rng.chance(0.2) {
            std::thread::sleep(Duration::from_millis(rng.below(15)));
        }
    }
    let st = engine.take_stats();
    println!("OK {ops} 回 / {:?} / 直近: {}", st, log.join(" "));
}
