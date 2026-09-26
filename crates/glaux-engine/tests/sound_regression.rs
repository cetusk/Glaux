//! 音の回帰テスト: 内蔵の音源・エフェクトごとに決まった短い曲を描き出し、「音の指紋」
//! (統合ラウドネス・True Peak・1/3 オクターブの釣り合い)を保存してある基準と比べる。
//! 信号処理を直したときに、意図せず音が変わっていないかを捕まえる。
//!
//! 意図して音を変えたときは、基準を作り直す:
//! `GLAUX_UPDATE_GOLDEN=1 cargo test -p glaux-engine --test sound_regression`
//! (基準は tests/golden/sound.json。差分をコミットの前に確かめること)

use glaux_core::{
    Clip, ClipContent, ClipId, Device, Effect, FxId, Note, NoteId, Project, Tick, Track, TrackId,
    TrackKind,
};
use std::collections::BTreeMap;

const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/sound.json");

/// 比べるときの許し(dB)。浮動小数の計算順の違い(CPU・コンパイラ)で出る細かな差は許す
const TOL_LOUDNESS: f64 = 0.5;
const TOL_BAND: f64 = 1.5;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct Fingerprint {
    loudness_lufs: f64,
    true_peak_dbtp: f64,
    /// 1/3 オクターブ(中心 Hz → dB。全体に対する)
    bands: BTreeMap<u32, f64>,
}

fn phrase(device: &str) -> Track {
    let mut t = Track::new(TrackId::new(), device, TrackKind::Midi);
    t.device = Some(Device::builtin(device));
    let mut clip = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(3840));
    if let ClipContent::Midi { notes, .. } = &mut clip.content {
        let pitches: &[u8] = if device == "drum" {
            &[36, 42, 38, 42, 36, 42, 38, 46]
        } else {
            &[48, 55, 60, 64, 67, 64, 60, 55]
        };
        for (i, p) in pitches.iter().enumerate() {
            notes.push(Note {
                id: NoteId::new(),
                pos: Tick(i as u64 * 480),
                dur: Tick(420),
                pitch: *p,
                vel: 100,
                articulation: Default::default(),
                pitch_curve: vec![],
                glide_ms: None,
            });
        }
    }
    t.clips.push(clip);
    t
}

/// 決まった曲たち(名前 → 曲)
fn cases() -> Vec<(String, Project)> {
    let mut out = Vec::new();
    for dev in ["subtractive", "fm", "wavetable", "drum", "pluck"] {
        let mut p = Project::new(dev);
        p.tracks.push(phrase(dev));
        out.push((format!("instrument/{dev}"), p));
    }
    let effects = [
        "eq",
        "dynamic_eq",
        "resonance",
        "virtual_bass",
        "compressor",
        "multiband",
        "transient",
        "limiter",
        "width",
        "reverb",
        "distortion",
        "amp",
        "delay",
        "chorus",
        "tape",
    ];
    for fx in effects {
        let mut p = Project::new(fx);
        let mut t = phrase("subtractive");
        let mut e = Effect::builtin(FxId::new(), fx);
        // 効きが分かるよう、いくつかは既定より強めに
        let set = |e: &mut Effect, k: &str, v: f64| {
            e.params.insert(k.into(), glaux_core::ParamValue::Float(v));
        };
        match fx {
            "eq" => set(&mut e, "high_gain_db", 6.0),
            "compressor" => set(&mut e, "threshold_db", -30.0),
            "limiter" => set(&mut e, "input_db", 9.0),
            "reverb" => set(&mut e, "mix", 0.5),
            "tape" => {
                set(&mut e, "hiss", 0.0);
                set(&mut e, "crackle", 0.0);
            }
            _ => {}
        }
        t.effects.push(e);
        p.tracks.push(t);
        out.push((format!("effect/{fx}"), p));
    }
    out
}

fn fingerprint(p: &Project) -> Fingerprint {
    let a = glaux_engine::analyze_project(p, None, None, &Default::default()).expect("解析できる");
    let r1 = |v: f64| (v * 10.0).round() / 10.0;
    Fingerprint {
        loudness_lufs: r1(a.loudness_lufs),
        true_peak_dbtp: r1(a.true_peak_dbtp),
        bands: a
            .tonal_balance
            .third_octave
            .iter()
            .map(|(hz, db)| (*hz, r1(*db)))
            .collect(),
    }
}

#[test]
fn sounds_match_the_golden_fingerprints() {
    let got: BTreeMap<String, Fingerprint> = cases()
        .into_iter()
        .map(|(name, p)| (name, fingerprint(&p)))
        .collect();
    if std::env::var_os("GLAUX_UPDATE_GOLDEN").is_some() {
        let json = serde_json::to_string_pretty(&got).unwrap();
        std::fs::write(GOLDEN, json + "\n").unwrap();
        eprintln!("基準を書き直しました: {GOLDEN}");
        return;
    }
    let want: BTreeMap<String, Fingerprint> = serde_json::from_str(
        &std::fs::read_to_string(GOLDEN).expect("基準がある(GLAUX_UPDATE_GOLDEN=1 で作る)"),
    )
    .unwrap();
    let mut diffs = Vec::new();
    for (name, w) in &want {
        let Some(g) = got.get(name) else {
            diffs.push(format!("{name}: 描き出せなかった"));
            continue;
        };
        if (g.loudness_lufs - w.loudness_lufs).abs() > TOL_LOUDNESS {
            diffs.push(format!(
                "{name}: ラウドネス {} → {} LUFS",
                w.loudness_lufs, g.loudness_lufs
            ));
        }
        if (g.true_peak_dbtp - w.true_peak_dbtp).abs() > TOL_LOUDNESS {
            diffs.push(format!(
                "{name}: True Peak {} → {} dBTP",
                w.true_peak_dbtp, g.true_peak_dbtp
            ));
        }
        for (hz, wdb) in &w.bands {
            // 無音に近い帯域は比べない
            if *wdb < -60.0 {
                continue;
            }
            let gdb = g.bands.get(hz).copied().unwrap_or(-100.0);
            if (gdb - wdb).abs() > TOL_BAND {
                diffs.push(format!("{name}: {hz}Hz 帯 {wdb} → {gdb} dB"));
            }
        }
    }
    for name in got.keys().filter(|n| !want.contains_key(*n)) {
        diffs.push(format!("{name}: 基準に無い(GLAUX_UPDATE_GOLDEN=1 で足す)"));
    }
    assert!(
        diffs.is_empty(),
        "音が基準と変わった(意図したものなら GLAUX_UPDATE_GOLDEN=1 で基準を作り直す):\n{}",
        diffs.join("\n")
    );
}
