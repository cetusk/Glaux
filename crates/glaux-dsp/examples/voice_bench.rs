//! 楽器のボイス 1 個の処理時間(ns/サンプル)。`cargo run --release -p glaux-dsp --example voice_bench`

use glaux_core::{Articulation, Device, ParamValue};
use glaux_dsp::{bake_instrument, VoiceState};
use std::time::Instant;

fn bench(label: &str, device: Device, pitch: u8) {
    let (_, params) = bake_instrument(Some(&device));
    let sr = 48_000.0;
    let n = 48_000 * 4;
    let freq = 440.0 * 2f32.powf((pitch as f32 - 69.0) / 12.0);
    let mut acc = 0.0f32;
    let mut best = f64::MAX;
    for _ in 0..5 {
        let mut v = VoiceState::start(&params, freq, pitch, 0.8, Articulation::Normal, sr);
        let t = Instant::now();
        for i in 0..n {
            if i == n * 3 / 4 {
                v.note_off();
            }
            acc += v.next(&params);
        }
        best = best.min(t.elapsed().as_nanos() as f64 / n as f64);
    }
    println!("{label:<22} {best:6.1} ns/サンプル  ({acc:.1})");
}

fn with(name: &str, kv: &[(&str, f64)]) -> Device {
    let mut d = Device::builtin(name);
    for (k, v) in kv {
        d.params.insert((*k).into(), ParamValue::Float(*v));
    }
    d
}

fn main() {
    bench("subtractive", with("subtractive", &[]), 60);
    bench(
        "subtractive unison7",
        with("subtractive", &[("unison", 7.0)]),
        60,
    );
    bench(
        "subtractive 生きた音",
        with(
            "subtractive",
            &[
                ("unison", 7.0),
                ("spread", 0.8),
                ("analog", 0.4),
                ("drive", 0.3),
                ("lfo1_depth", 0.2),
                ("lfo2_depth", 0.2),
                ("filter_decay", 0.3),
            ],
        ),
        60,
    );
    bench("wavetable", with("wavetable", &[]), 60);
    bench(
        "wavetable unison7",
        with("wavetable", &[("unison", 7.0)]),
        60,
    );
    bench("fm", with("fm", &[]), 60);
    bench("drum kick", with("drum", &[]), 36);
    bench("drum snare", with("drum", &[]), 38);
    bench("drum hat", with("drum", &[]), 42);
    bench("drum clap", with("drum", &[]), 39);
}
