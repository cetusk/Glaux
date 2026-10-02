//! 楽器のボイス 1 個の処理時間(ns/サンプル)。`cargo run --release -p glaux-dsp --example voice_bench`

use glaux_core::{Articulation, Device, ParamValue};
use glaux_dsp::{bake_instrument, VoiceState};
use std::time::Instant;

fn bench(label: &str, device: Device, pitch: u8) {
    let (_, params) = bake_instrument(Some(&device));
    bench_params(label, params, pitch);
}

fn bench_params(label: &str, params: glaux_dsp::InstrumentParams, pitch: u8) {
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
            acc += v.next_stereo(&params).0;
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
    bench("fm4", with("fm4", &[]), 60);
    bench(
        "fm4 直列+feedback",
        with("fm4", &[("algorithm", 1.0), ("feedback", 0.5)]),
        60,
    );
    bench("additive 16", with("additive", &[("partials", 16.0)]), 48);
    bench("additive 32", with("additive", &[]), 48);
    bench("additive 64", with("additive", &[("partials", 64.0)]), 48);
    bench(
        "additive 64 全部入り",
        with(
            "additive",
            &[
                ("partials", 64.0),
                ("formant_db", 12.0),
                ("damping", 0.5),
                ("inharmonic", 0.3),
                ("wobble", 0.5),
            ],
        ),
        48,
    );
    // 素材を使う音源: 2 秒のステレオの合成音
    let sr = 48_000.0f32;
    let l: Vec<f32> = (0..96_000)
        .map(|i| (i as f32 * 220.0 * std::f32::consts::TAU / sr).sin() * 0.5)
        .collect();
    let r: Vec<f32> = (0..96_000)
        .map(|i| (i as f32 * 331.0 * std::f32::consts::TAU / sr).sin() * 0.5)
        .collect();
    let data = std::sync::Arc::new(glaux_dsp::SampleData::stereo(&l, &r, sr));
    let map = |kv: &[(&str, ParamValue)]| {
        let mut m = glaux_core::ParamMap::new();
        for (k, v) in kv {
            m.insert((*k).into(), v.clone());
        }
        m
    };
    bench_params(
        "sampler",
        glaux_dsp::InstrumentParams::Sampler(glaux_dsp::bake_sampler(&map(&[]), data.clone(), sr)),
        67,
    );
    bench_params(
        "sampler ループ+フィルタ+ステレオ",
        glaux_dsp::InstrumentParams::Sampler(glaux_dsp::bake_sampler(
            &map(&[
                ("loop", ParamValue::Bool(true)),
                ("loop_start", ParamValue::Float(0.2)),
                ("loop_end", ParamValue::Float(0.4)),
                ("filter_type", ParamValue::Enum("lp24".into())),
                ("cutoff", ParamValue::Float(2000.0)),
                ("stereo", ParamValue::Bool(true)),
            ]),
            data.clone(),
            sr,
        )),
        67,
    );
    bench_params(
        "granular(既定)",
        glaux_dsp::InstrumentParams::Granular(glaux_dsp::bake_granular(
            &map(&[]),
            Some(data.clone()),
        )),
        67,
    );
    bench_params(
        "granular 密(200/s・150ms)",
        glaux_dsp::InstrumentParams::Granular(glaux_dsp::bake_granular(
            &map(&[
                ("density", ParamValue::Float(200.0)),
                ("grain_ms", ParamValue::Float(150.0)),
            ]),
            Some(data),
        )),
        67,
    );
    bench("drum kick", with("drum", &[]), 36);
    bench("drum snare", with("drum", &[]), 38);
    bench("drum hat", with("drum", &[]), 42);
    bench("drum clap", with("drum", &[]), 39);
}
