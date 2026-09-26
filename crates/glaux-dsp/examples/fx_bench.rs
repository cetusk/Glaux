//! 内蔵エフェクトの重さの計測: 既定のつまみで 10 秒のステレオを通し、1 サンプル(左右 1 組)あたりの時間を出す。
//! `cargo run --release -p glaux-dsp --example fx_bench`
//!
//! 48kHz では 1 サンプルの持ち時間が約 20800ns。1 つのエフェクトが 100ns なら、その 0.5%。
//! (2026-09-26 の計測: いちばん重い multiband で約 80ns。種類の判定をブロックに 1 回にしても
//! 速さは変わらなかった = サンプルごとの分岐は重さの元ではない)

use glaux_core::{Effect, FxId};
use std::time::Instant;

fn main() {
    let sr = 48_000.0f32;
    let n = (sr * 10.0) as usize;
    let input: Vec<(f32, f32)> = (0..n)
        .map(|i| {
            let t = i as f32 / sr;
            let x = (t * 220.0 * std::f32::consts::TAU).sin() * 0.3
                + (t * 3311.0 * std::f32::consts::TAU).sin() * 0.1;
            (x, x * 0.8)
        })
        .collect();
    let names: Vec<&str> = glaux_dsp::effect_catalog().iter().map(|e| e.name).collect();
    println!(
        "{:<12} {:>10}  (1 サンプルあたり、既定のつまみ)",
        "effect", "ns"
    );
    for name in names {
        let e = Effect::builtin(FxId::new(), name);
        let Some(p) = glaux_dsp::bake_effect(&e, sr, &|_| None) else {
            continue;
        };
        let mut st = glaux_dsp::EffectState::default();
        st.ensure_kind(&p);
        let mut sink = 0.0f32;
        let t = Instant::now();
        for &(l, r) in &input {
            let (a, b) = st.process(&p, l, r, 0.0);
            sink += a + b;
        }
        let ns = t.elapsed().as_nanos() as f64 / n as f64;
        println!(
            "{name:<12} {ns:>10.1}  {}",
            if sink.is_nan() { "!" } else { "" }
        );
    }
}
