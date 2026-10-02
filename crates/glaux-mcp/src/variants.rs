//! 変種の自動生成(MCP の mutate_sound)。今の音色から少しずつ違う変種を作り、互いに違うものを選んで、違いを言葉で添える。
//!
//! 「良い音」を一発で当てるより、人の耳で選んでもらう方が確実(旋律で点数と耳が合わなかった経験から)。
//! 選んだ変種を当てて、もう一度呼べば「これの方向にもっと」の次の世代になる(遺伝的な探し方と同じ)。
//!
//! - 候補: 内蔵音源のつまみ(と空間系のエフェクトの mix)を、決まった種で揺らす。周波数・時間のつまみは比で、ほかは差で
//! - 測る: 1 音だけ描き出し([`glaux_engine::export::render_track_note`])、音色の記述子([`glaux_engine::timbre`])を取る
//! - 選ぶ: 元の音を出発点に、選んだものから最も遠い候補を順に足す(互いに違う変種がそろう)
//! - 言葉: 明るさ・立ち上がり・伸び・ざらつきの違いと、広がり・揺らぎ・歪みのつまみの違い

use glaux_core::{Device, ParamRange, ParamValue, PluginSource, Project, Track};

/// 揺らさないつまみ(音量・声部数・選択肢)
const FIXED: &[&str] = &["gain_db", "unison", "root", "kit"];

/// 1 つの変種
#[derive(Clone, Debug)]
pub struct Variant {
    pub device: Device,
    /// (エフェクト ID, つまみ名, 値)
    pub effect_params: Vec<(glaux_core::FxId, String, f64)>,
    /// 元からの違い(つまみ名, 前, 後)
    pub changes: Vec<(String, f64, f64)>,
    /// 違いの言葉
    pub words: Vec<String>,
}

fn rng_next(s: &mut u64) -> f64 {
    // splitmix64 → 0..1
    *s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *s;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

/// 正規分布に近い乱数(4 つの和)
fn gauss(s: &mut u64) -> f64 {
    (0..4).map(|_| rng_next(s)).sum::<f64>() - 2.0
}

/// 比で動かすつまみか(周波数・時間)
fn is_ratio(name: &str, spec: &glaux_core::ParamSpec) -> bool {
    matches!(spec.range, ParamRange::Float { skew: Some(_), min, .. } if min > 0.0)
        || [
            "cutoff", "attack", "decay", "release", "rate", "freq", "time",
        ]
        .iter()
        .any(|k| name.contains(k))
}

/// 1 つのつまみを揺らす
fn perturb(
    v: f64,
    spec: &glaux_core::ParamSpec,
    name: &str,
    amount: f64,
    s: &mut u64,
) -> Option<f64> {
    let (lo, hi) = match spec.range {
        ParamRange::Float { min, max, .. } => (min, max),
        ParamRange::Int { min, max, .. } => (min as f64, max as f64),
        _ => return None,
    };
    let g = gauss(s) * amount;
    let out = if is_ratio(name, spec) && v > 0.0 && lo > 0.0 {
        // ±amount で最大 ±3 オクターブほど
        v * 2f64.powf(g * 3.0)
    } else {
        v + g * (hi - lo) * 0.5
    };
    let out = out.clamp(lo, hi);
    Some(
        if matches!(spec.range, ParamRange::Int { .. }) || out.abs() >= 100.0 {
            out.round()
        } else {
            (out * 1000.0).round() / 1000.0
        },
    )
}

/// 候補を作る(内蔵音源のトラックだけ)
pub fn candidates(track: &Track, seed: u64, amount: f64, n: usize) -> Result<Vec<Variant>, String> {
    let device = track
        .device
        .clone()
        .unwrap_or_else(|| Device::builtin(glaux_dsp::DEFAULT_INSTRUMENT));
    let PluginSource::Builtin { name } = &device.source else {
        return Err("変種を作れるのは内蔵の音源だけです(CLAP 音源は find_similar_presets・refine_plugin_params)".into());
    };
    let specs = glaux_dsp::instrument_params(name).ok_or("知らない音源です")?;
    let mut s = seed.wrapping_mul(0x2545_F491_4F6C_DD1D) ^ 0x5DEE_CE66;
    let mut out = Vec::new();
    for _ in 0..n {
        let mut d = device.clone();
        let mut changes = Vec::new();
        for spec in specs.iter().filter(|sp| !FIXED.contains(&sp.name)) {
            if rng_next(&mut s) > 0.4 {
                continue;
            }
            let cur = d
                .params
                .get(spec.name)
                .and_then(ParamValue::as_f64)
                .or(match spec.range {
                    ParamRange::Float { default, .. } => Some(default),
                    ParamRange::Int { default, .. } => Some(default as f64),
                    _ => None,
                });
            let Some(cur) = cur else { continue };
            if let Some(v) = perturb(cur, spec, spec.name, amount, &mut s) {
                if (v - cur).abs() > 1e-9 {
                    d.params.insert(spec.name.to_owned(), ParamValue::Float(v));
                    changes.push((spec.name.to_owned(), cur, v));
                }
            }
        }
        // 空間系のエフェクトの mix も少し
        let mut effect_params = Vec::new();
        for e in &track.effects {
            let PluginSource::Builtin { name } = &e.source else {
                continue;
            };
            if !["reverb", "delay", "chorus"].contains(&name.as_str()) || rng_next(&mut s) > 0.5 {
                continue;
            }
            let Some(spec) = glaux_dsp::effect_params_spec(name)
                .and_then(|ss| ss.iter().find(|x| x.name == "mix"))
            else {
                continue;
            };
            let cur =
                e.params
                    .get("mix")
                    .and_then(ParamValue::as_f64)
                    .unwrap_or(match spec.range {
                        ParamRange::Float { default, .. } => default,
                        _ => 0.3,
                    });
            if let Some(v) = perturb(cur, spec, "mix", amount * 0.6, &mut s) {
                effect_params.push((e.id.clone(), "mix".to_owned(), v));
                changes.push((format!("{name}.mix"), cur, v));
            }
        }
        if changes.len() >= 2 {
            out.push(Variant {
                device: d,
                effect_params,
                changes,
                words: vec![],
            });
        }
    }
    Ok(out)
}

/// 測る音の高さ: トラックのノートの中央値(無ければ C4)
pub fn audition_pitch(track: &Track) -> u8 {
    let mut p: Vec<u8> = track
        .clips
        .iter()
        .filter_map(|c| c.notes())
        .flat_map(|ns| ns.iter().map(|n| n.pitch))
        .collect();
    p.sort_unstable();
    p.get(p.len() / 2).copied().unwrap_or(60)
}

/// 音色の記述子(比べる用の数の並び)
#[derive(Clone, Debug)]
pub struct Desc {
    pub centroid: f64,
    pub attack_ms: f64,
    pub sustain_db: f64,
    pub flatness: f64,
    pub rms_db: f64,
}

impl Desc {
    fn vector(&self) -> [f64; 4] {
        [
            self.centroid.max(20.0).log2(),
            (self.attack_ms.max(0.5)).log2() * 0.5,
            self.sustain_db / 12.0,
            self.flatness * 5.0,
        ]
    }
}

/// 変種をトラックに当てたプロジェクト(1 トラックだけ)を描き出して測る
pub fn measure(
    project: &Project,
    track: &Track,
    v: Option<&Variant>,
    bank: &glaux_engine::SampleBank,
) -> Result<Desc, String> {
    let mut p = project.clone();
    let mut t = track.clone();
    if let Some(v) = v {
        t.device = Some(v.device.clone());
        for (id, k, val) in &v.effect_params {
            if let Some(e) = t.effects.iter_mut().find(|e| &e.id == id) {
                e.params.insert(k.clone(), ParamValue::Float(*val));
            }
        }
    }
    p.tracks = vec![t.clone()];
    let y = glaux_engine::export::render_track_note(
        &p,
        &t.id,
        audition_pitch(track),
        100,
        1.2,
        48_000.0,
        bank,
    )
    .map_err(|e| e.to_string())?;
    let d = glaux_engine::timbre::describe(&y, 48_000.0, None);
    Ok(Desc {
        centroid: d.spectrum.centroid_hz as f64,
        attack_ms: d.envelope.attack_ms as f64,
        sustain_db: d.envelope.sustain_db as f64,
        flatness: d.spectrum.flatness as f64,
        rms_db: d.rms_db as f64,
    })
}

/// 元の音と選んだものから最も遠い候補を順に `count` 個(互いに違う変種)
pub fn pick_diverse(orig: &Desc, cands: &[Desc], count: usize) -> Vec<usize> {
    let dist = |a: &[f64; 4], b: &[f64; 4]| {
        a.iter()
            .zip(b)
            .map(|(x, y)| (x - y).powi(2))
            .sum::<f64>()
            .sqrt()
    };
    let mut chosen: Vec<usize> = Vec::new();
    let mut refs: Vec<[f64; 4]> = vec![orig.vector()];
    while chosen.len() < count.min(cands.len()) {
        let best = (0..cands.len())
            .filter(|i| !chosen.contains(i))
            // 無音・壊れた候補(音量が大きく落ちた)は選ばない
            .filter(|&i| cands[i].rms_db > orig.rms_db - 18.0)
            .map(|i| {
                let v = cands[i].vector();
                (i, refs.iter().map(|r| dist(r, &v)).fold(f64::MAX, f64::min))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((i, _)) => {
                refs.push(cands[i].vector());
                chosen.push(i);
            }
            None => break,
        }
    }
    chosen
}

/// 元との違いを言葉に
pub fn words(orig: &Desc, d: &Desc, changes: &[(String, f64, f64)]) -> Vec<String> {
    let mut out = Vec::new();
    let r = d.centroid / orig.centroid.max(1.0);
    if r > 1.15 {
        out.push(format!("明るい(重心 {:.0}%)", r * 100.0));
    } else if r < 0.87 {
        out.push(format!("暗い(重心 {:.0}%)", r * 100.0));
    }
    let a = d.attack_ms / orig.attack_ms.max(0.5);
    if a < 0.6 {
        out.push("立ち上がりが速い".to_owned());
    } else if a > 1.6 {
        out.push("ふわっと立ち上がる".to_owned());
    }
    let sus = d.sustain_db - orig.sustain_db;
    if sus > 3.0 {
        out.push("よく伸びる".to_owned());
    } else if sus < -3.0 {
        out.push("短く減衰する".to_owned());
    }
    if d.flatness - orig.flatness > 0.04 {
        out.push("ざらつく・息っぽい".to_owned());
    } else if orig.flatness - d.flatness > 0.04 {
        out.push("澄んだ".to_owned());
    }
    let ch = |k: &str| changes.iter().find(|c| c.0 == k).map(|c| c.2 - c.1);
    if let Some(x) = ch("spread") {
        out.push(if x > 0.0 { "広い" } else { "狭い" }.to_owned());
    }
    if ch("analog").is_some_and(|x| x > 0.1) || ch("lfo1_depth").is_some_and(|x| x > 0.05) {
        out.push("揺れが多い".to_owned());
    }
    if let Some(x) = ch("drive") {
        if x.abs() > 0.1 {
            out.push(
                if x > 0.0 {
                    "歪みが多い"
                } else {
                    "歪みが少ない"
                }
                .to_owned(),
            );
        }
    }
    if out.is_empty() {
        out.push("わずかな違い".to_owned());
    }
    out
}

/// 変種を作って選ぶ一式(MCP の mutate_sound の中身。重いので spawn_blocking の中で呼ぶ)
pub fn generate(
    project: &Project,
    track: &Track,
    seed: u64,
    amount: f64,
    count: usize,
    bank: &glaux_engine::SampleBank,
) -> Result<(Desc, Vec<Variant>), String> {
    let orig = measure(project, track, None, bank)?;
    let cands = candidates(track, seed, amount, count * 3)?;
    let descs: Vec<Desc> = cands
        .iter()
        .map(|v| measure(project, track, Some(v), bank))
        .collect::<Result<_, _>>()?;
    let picked = pick_diverse(&orig, &descs, count);
    let out = picked
        .into_iter()
        .map(|i| {
            let mut v = cands[i].clone();
            v.words = words(&orig, &descs[i], &v.changes);
            v
        })
        .collect();
    Ok((orig, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::{TrackId, TrackKind};

    fn synth() -> Track {
        let mut t = Track::new(TrackId::new(), "Lead", TrackKind::Midi);
        t.device = Some(Device::builtin("subtractive"));
        t
    }

    #[test]
    fn candidates_are_repeatable_and_in_range() {
        let t = synth();
        let a = candidates(&t, 3, 0.3, 6).unwrap();
        let b = candidates(&t, 3, 0.3, 6).unwrap();
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.device, y.device, "同じ種なら同じ候補");
        }
        let c = candidates(&t, 4, 0.3, 6).unwrap();
        assert_ne!(a[0].device, c[0].device);
        let specs = glaux_dsp::instrument_params("subtractive").unwrap();
        for v in &a {
            assert!(v.changes.len() >= 2);
            for (k, _, after) in &v.changes {
                let sp = specs.iter().find(|s| s.name == k).unwrap();
                if let ParamRange::Float { min, max, .. } = sp.range {
                    assert!((min..=max).contains(after), "{k} {after}");
                }
            }
        }
    }

    #[test]
    fn diverse_picks_spread_out() {
        let d = |c: f64, a: f64| Desc {
            centroid: c,
            attack_ms: a,
            sustain_db: -6.0,
            flatness: 0.1,
            rms_db: -20.0,
        };
        let orig = d(1000.0, 5.0);
        let cands = vec![
            d(1010.0, 5.0),
            d(4000.0, 5.0),
            d(4100.0, 5.0),
            d(250.0, 5.0),
            d(1000.0, 200.0),
        ];
        let picked = pick_diverse(&orig, &cands, 3);
        // ほぼ元と同じ候補(0)や、同じ方向の 2 つ目(1 と 2 のどちらか)は後回し
        assert!(!picked.contains(&0), "{picked:?}");
        assert!(picked.contains(&3) && picked.contains(&4), "{picked:?}");
    }

    #[test]
    fn clap_tracks_are_refused() {
        let mut t = synth();
        t.device = Some(Device {
            source: PluginSource::Clap {
                plugin_id: "x".into(),
                state: None,
            },
            params: Default::default(),
        });
        assert!(candidates(&t, 1, 0.3, 4).is_err());
    }

    #[test]
    fn generate_measures_and_describes() {
        let mut p = Project::new("t");
        let t = synth();
        p.tracks.push(t.clone());
        let (_, vs) = generate(&p, &t, 1, 0.4, 3, &glaux_engine::SampleBank::default()).unwrap();
        assert_eq!(vs.len(), 3);
        assert!(vs.iter().all(|v| !v.words.is_empty()));
    }
}
