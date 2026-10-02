//! どの音色にも効く共通の大きなつまみ(MCP の set_character)。
//!
//! 「明るさ・太さ・動き・広がり・空間・アタック・歪み」の 7 つを、トラックのマクロ(set_macro と同じ仕組み)として作る。
//! 0〜100 で、50 が「作ったときの音」。AI は「もう少し明るく・遠く」を 30 個のつまみから探さずに 1〜2 個で指示でき、
//! 人には簡単な取っ手になり、オートメーション(macro/N)も 1 本で済む。
//!
//! 割り当て先は音源・エフェクトの種類ごとの表([`feel_targets`])。50 のときに今の値になるよう、マクロの曲線を合わせる
//! ([`centered_target`])。周波数・時間のつまみは比で、ほかは差で動かす。CLAP 音源はつまみの名前(cutoff・drive・reverb…)
//! から推定する。

use glaux_core::{
    Effect, Macro, MacroTarget, ParamPath, ParamRange, ParamValue, PluginSource, Track,
};

/// 7 つのつまみ: (キー, 名前, 説明)
pub const FEELS: [(&str, &str, &str); 7] = [
    (
        "brightness",
        "明るさ",
        "上げると明るく抜ける、下げるとこもって暗い",
    ),
    ("body", "太さ", "上げると低域と厚みが増す、下げると細く軽い"),
    (
        "motion",
        "動き",
        "上げると揺らぎ・うねりが増える、下げると止まった音",
    ),
    (
        "width",
        "広がり",
        "上げると左右に広がる、下げると中央に集まる",
    ),
    (
        "space",
        "空間",
        "上げると響きが増えて遠く、下げると近く乾いた音",
    ),
    (
        "attack",
        "アタック",
        "上げると立ち上がりが速く鋭い、下げるとふわっと柔らかい",
    ),
    (
        "grit",
        "歪み",
        "上げると倍音が増えてざらつく・攻撃的、下げるとクリーン",
    ),
];

/// 動かし方
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Span {
    /// 比で ±2^oct 倍(周波数・時間)
    Ratio(f64),
    /// 差で ±amount
    Add(f64),
}

/// 1 つの割り当て先の素(どのつまみを、どちら向きに、どれだけ)
#[derive(Clone, Debug, PartialEq)]
pub struct FeelTarget {
    pub path: ParamPath,
    pub span: Span,
    /// false なら逆向き(つまみを上げると値が下がる。アタックの attack など)
    pub up: bool,
}

/// 内蔵の音源ごとの表: (つまみのキー, 音源のつまみ名, 動かし方, 向き)
fn instrument_table(name: &str) -> &'static [(&'static str, &'static str, Span, bool)] {
    match name {
        "subtractive" => &[
            ("brightness", "cutoff", Span::Ratio(1.5), true),
            ("brightness", "filter_env", Span::Add(0.25), true),
            ("body", "sub", Span::Add(0.4), true),
            ("body", "detune", Span::Add(10.0), true),
            ("motion", "analog", Span::Add(0.35), true),
            ("motion", "lfo1_depth", Span::Add(0.2), true),
            ("width", "spread", Span::Add(0.5), true),
            ("attack", "attack", Span::Ratio(3.0), false),
            ("attack", "filter_attack", Span::Ratio(1.5), false),
            ("grit", "drive", Span::Add(0.45), true),
        ],
        "wavetable" => &[
            ("brightness", "cutoff", Span::Ratio(1.5), true),
            ("brightness", "position", Span::Add(0.2), true),
            ("body", "detune", Span::Add(10.0), true),
            ("motion", "analog", Span::Add(0.35), true),
            ("motion", "lfo_depth", Span::Add(0.3), true),
            ("width", "spread", Span::Add(0.5), true),
            ("attack", "attack", Span::Ratio(3.0), false),
            ("grit", "drive", Span::Add(0.45), true),
        ],
        "fm" => &[
            ("brightness", "index", Span::Ratio(1.0), true),
            ("body", "index_sustain", Span::Add(0.25), true),
            ("attack", "attack", Span::Ratio(3.0), false),
            ("grit", "feedback", Span::Add(0.35), true),
        ],
        "pluck" => &[
            ("brightness", "brightness", Span::Add(0.3), true),
            ("attack", "pick", Span::Add(0.3), true),
        ],
        "drum" => &[
            ("brightness", "tone", Span::Add(0.3), true),
            ("body", "decay", Span::Ratio(0.6), true),
            ("attack", "kick_punch", Span::Add(0.35), true),
        ],
        _ => &[],
    }
}

/// 内蔵のエフェクトごとの表
fn effect_table(name: &str) -> &'static [(&'static str, &'static str, Span, bool)] {
    match name {
        "eq" => &[
            ("brightness", "high_gain_db", Span::Add(5.0), true),
            ("body", "low_gain_db", Span::Add(4.0), true),
        ],
        "reverb" => &[
            ("space", "mix", Span::Add(0.25), true),
            ("space", "size", Span::Add(0.25), true),
        ],
        "convolution" => &[("space", "mix", Span::Add(0.25), true)],
        "delay" => &[("space", "mix", Span::Add(0.2), true)],
        "width" => &[("width", "width", Span::Add(0.6), true)],
        "chorus" => &[
            ("motion", "mix", Span::Add(0.3), true),
            ("width", "mix", Span::Add(0.2), true),
        ],
        "distortion" => &[("grit", "drive_db", Span::Add(12.0), true)],
        "amp" => &[("grit", "gain_db", Span::Add(12.0), true)],
        "tape" => &[
            ("grit", "saturation", Span::Add(0.3), true),
            ("motion", "wow", Span::Add(0.3), true),
        ],
        "transient" => &[("attack", "attack_db", Span::Add(6.0), true)],
        _ => &[],
    }
}

/// CLAP のつまみの名前から推定する表: (キー, 名前に含む語, 向き)
const CLAP_WORDS: &[(&str, &[&str], bool)] = &[
    (
        "brightness",
        &["cutoff", "filter freq", "brightness", "tone"],
        true,
    ),
    ("body", &["sub", "detune", "thick"], true),
    (
        "motion",
        &["lfo amount", "lfo depth", "mod depth", "vibrato"],
        true,
    ),
    ("width", &["width", "spread", "stereo"], true),
    ("space", &["reverb", "room", "space", "fx mix"], true),
    ("attack", &["attack"], false),
    ("grit", &["drive", "dist", "saturat", "crush"], true),
];

/// ParamSpec の (最小, 最大, 既定)。数のつまみでなければ None
fn spec_range(r: &ParamRange) -> Option<(f64, f64, f64)> {
    match *r {
        ParamRange::Float {
            min, max, default, ..
        } => Some((min, max, default)),
        ParamRange::Int { min, max, default } => Some((min as f64, max as f64, default as f64)),
        _ => None,
    }
}

/// 50(マクロの値 0.5)で `cur` になる割り当て先を作る。範囲の端にある値は片側だけ動く
pub fn centered_target(
    path: ParamPath,
    cur: f64,
    lo: f64,
    hi: f64,
    span: Span,
    up: bool,
) -> MacroTarget {
    let cur = cur.clamp(lo, hi);
    let (mut a, mut b) = match span {
        Span::Ratio(oct) if cur > 0.0 => (cur / 2f64.powf(oct), cur * 2f64.powf(oct)),
        Span::Ratio(oct) => (lo, (lo + (hi - lo) * oct.min(1.0) * 0.5).max(cur)),
        Span::Add(d) => (cur - d, cur + d),
    };
    a = a.clamp(lo, hi);
    b = b.clamp(lo, hi);
    // 0.5 で cur になる曲線(r = 中心の位置)。map は min + (max − min) × shaped(0.5)
    let (min, max) = if up { (a, b) } else { (b, a) };
    let r = if (max - min).abs() < 1e-12 {
        0.5
    } else {
        ((cur - min) / (max - min)).clamp(1e-3, 1.0 - 1e-3)
    };
    let curve = if r < 0.5 {
        ((1.0 / r).log2() - 1.0) / 3.0
    } else {
        (1.0 - (1.0 / (1.0 - r)).log2()) / 3.0
    }
    .clamp(-1.0, 1.0);
    MacroTarget {
        target: path,
        min,
        max,
        curve: (curve * 1000.0).round() / 1000.0,
    }
}

/// CLAP のつまみ 1 つ: (id, 名前, 最小, 最大, 今の値)
pub type ClapParam = (u32, String, f64, f64, f64);

/// トラックの、つまみのキーごとの割り当て先(今の値を中心に)。CLAP 音源なら `clap` にそのつまみの一覧を渡す
pub fn feel_targets(track: &Track, clap: &[ClapParam]) -> Vec<(&'static str, MacroTarget)> {
    let mut out = Vec::new();
    // 音源
    if let Some(d) = &track.device {
        match &d.source {
            PluginSource::Builtin { name } => {
                let specs = glaux_dsp::instrument_params(name).unwrap_or(&[]);
                for (key, pname, span, up) in instrument_table(name) {
                    let Some(spec) = specs.iter().find(|s| s.name == *pname) else {
                        continue;
                    };
                    let Some((lo, hi, def)) = spec_range(&spec.range) else {
                        continue;
                    };
                    let cur = d
                        .params
                        .get(*pname)
                        .and_then(ParamValue::as_f64)
                        .unwrap_or(def);
                    out.push((
                        *key,
                        centered_target(ParamPath::device(*pname), cur, lo, hi, *span, *up),
                    ));
                }
            }
            PluginSource::Clap { .. } => {
                for (key, words, up) in CLAP_WORDS {
                    let hit = clap.iter().find(|(_, n, ..)| {
                        let n = n.to_lowercase();
                        words.iter().any(|w| n.contains(w))
                    });
                    if let Some((id, _, lo, hi, cur)) = hit {
                        let span = Span::Add((hi - lo) * 0.25);
                        out.push((
                            *key,
                            centered_target(
                                ParamPath::device(format!("clap:{id}")),
                                *cur,
                                *lo,
                                *hi,
                                span,
                                *up,
                            ),
                        ));
                    }
                }
            }
            _ => {}
        }
    } else {
        // 音源が未設定のトラックは既定の subtractive で鳴る
        let mut t = track.clone();
        t.device = Some(glaux_core::Device::builtin(glaux_dsp::DEFAULT_INSTRUMENT));
        return feel_targets(&t, clap);
    }
    // エフェクト(内蔵、外していないもの)
    for e in track.effects.iter().filter(|e| !e.bypass && !e.ui.parked) {
        let PluginSource::Builtin { name } = &e.source else {
            continue;
        };
        let specs = glaux_dsp::effect_params_spec(name).unwrap_or(&[]);
        for (key, pname, span, up) in effect_table(name) {
            let Some(spec) = specs.iter().find(|s| s.name == *pname) else {
                continue;
            };
            let Some((lo, hi, def)) = spec_range(&spec.range) else {
                continue;
            };
            let cur = e
                .params
                .get(*pname)
                .and_then(ParamValue::as_f64)
                .unwrap_or(def);
            out.push((
                *key,
                centered_target(
                    ParamPath::effect(e.id.clone(), *pname),
                    cur,
                    lo,
                    hi,
                    *span,
                    *up,
                ),
            ));
        }
    }
    out
}

/// 空間のつまみの行き先が無いトラックに足すリバーブ(響きは控えめに始める)
pub fn space_reverb() -> Effect {
    let mut e = Effect::builtin(glaux_core::FxId::new(), "reverb");
    e.params.insert("mix".into(), ParamValue::Float(0.15));
    e.params.insert("size".into(), ParamValue::Float(0.5));
    e.ui.label = Some("空間".to_owned());
    e
}

/// [`build_macros`] の結果: (新しいマクロの並び, 作ったつまみのキー, 行き先が無くて作れなかったキー)
pub type Built = (Vec<Macro>, Vec<&'static str>, Vec<&'static str>);

/// つまみのマクロを作る(既にある同名のマクロは作り直す = 今の音を 50 にする)。
/// 戻り値は (新しいマクロの並び, 作ったつまみのキー, 行き先が無くて作れなかったキー)
pub fn build_macros(track: &Track, clap: &[ClapParam], keys: &[&str]) -> Result<Built, String> {
    let targets = feel_targets(track, clap);
    let mut macros: Vec<Macro> = track.macros.clone();
    let mut made = Vec::new();
    let mut missing = Vec::new();
    for (key, name, _) in FEELS.iter().filter(|(k, ..)| keys.contains(k)) {
        let ts: Vec<MacroTarget> = targets
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, t)| t.clone())
            .take(16)
            .collect();
        if ts.is_empty() {
            missing.push(*key);
            continue;
        }
        let m = Macro {
            name: (*name).to_owned(),
            value: 0.5,
            targets: ts,
        };
        match macros.iter_mut().find(|x| x.name == *name) {
            Some(x) => *x = m,
            None => macros.push(m),
        }
        made.push(*key);
    }
    if macros.len() > glaux_core::MAX_MACROS {
        return Err(format!(
            "マクロが {} 個になり、上限 {} 個を超えます(作るつまみを絞るか、使っていないマクロを set_macro の remove で外してください)",
            macros.len(),
            glaux_core::MAX_MACROS
        ));
    }
    Ok((macros, made, missing))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::{Device, TrackId, TrackKind};

    #[test]
    fn centered_targets_hit_the_current_value_at_fifty() {
        for (cur, lo, hi, span, up) in [
            (1000.0, 40.0, 12000.0, Span::Ratio(1.5), true),
            (0.2, 0.0, 1.0, Span::Add(0.4), true),
            (0.0, 0.0, 1.0, Span::Add(0.4), true),
            (0.005, 0.001, 10.0, Span::Ratio(2.0), false),
            (0.95, 0.0, 1.0, Span::Add(0.3), true),
        ] {
            let t = centered_target(ParamPath::device("x"), cur, lo, hi, span, up);
            let mid = t.map(0.5);
            // 端の値(0.0 など)は片側にしか動けないので、中心からのずれを許す
            let tol = if cur == lo || cur == hi {
                0.05 * (hi - lo)
            } else {
                0.02 * cur.abs().max(1e-3) + 1e-6
            };
            assert!((mid - cur).abs() <= tol, "{cur}: {mid} {t:?}");
            assert!(t.min >= lo - 1e-9 && t.max <= hi + 1e-9 && t.min.min(t.max) >= lo - 1e-9);
            // 向き
            if up {
                assert!(t.map(1.0) >= t.map(0.0));
            } else {
                assert!(t.map(1.0) <= t.map(0.0));
            }
        }
    }

    #[test]
    fn every_feel_has_targets_on_a_synth_with_effects() {
        let mut t = Track::new(TrackId::new(), "Lead", TrackKind::Midi);
        t.device = Some(Device::builtin("subtractive"));
        t.effects
            .push(Effect::builtin(glaux_core::FxId::new(), "reverb"));
        let keys: Vec<&str> = FEELS.iter().map(|f| f.0).collect();
        let (macros, made, missing) = build_macros(&t, &[], &keys).unwrap();
        assert_eq!(made.len(), 7, "{missing:?}");
        assert_eq!(macros.len(), 7);
        glaux_core::check_macros(&macros).unwrap();
        // 明るさは cutoff を含み、50 で今の値(既定 8000)
        let b = macros.iter().find(|m| m.name == "明るさ").unwrap();
        let cutoff = b
            .targets
            .iter()
            .find(|x| x.target == ParamPath::device("cutoff"))
            .unwrap();
        assert!((cutoff.map(0.5) - 8000.0).abs() < 100.0);
        // 作り直しても数は増えない
        let mut t2 = t.clone();
        t2.macros = macros;
        let (m2, ..) = build_macros(&t2, &[], &["brightness"]).unwrap();
        assert_eq!(m2.len(), 7);
    }

    #[test]
    fn clap_params_are_guessed_by_name() {
        let mut t = Track::new(TrackId::new(), "Synth", TrackKind::Midi);
        t.device = Some(Device {
            source: PluginSource::Clap {
                plugin_id: "x".into(),
                state: None,
            },
            params: Default::default(),
        });
        let params = vec![
            (3, "Filter 1 Cutoff".to_owned(), 0.0, 1.0, 0.6),
            (9, "Amp Attack".to_owned(), 0.0, 1.0, 0.1),
        ];
        let ts = feel_targets(&t, &params);
        assert!(ts
            .iter()
            .any(|(k, x)| *k == "brightness" && x.target == ParamPath::device("clap:3")));
        assert!(ts
            .iter()
            .any(|(k, x)| *k == "attack" && x.target == ParamPath::device("clap:9")));
    }
}
