//! 言葉で音を追い込む: トラックの内蔵エフェクトのつまみを、CLAP の音色語(辞書)に近づく方へ CMA-ES で動かす。
//!
//! 1. トラックだけを、エフェクトを外して描き出す(最初に音が鳴る所から最大 8 秒)。これを 1 回だけ
//! 2. 候補のつまみでエフェクトを順に通し、音量を元にそろえてから(大きいほど印象が変わるのを避ける)
//!    CLAP の埋め込みを求め、「近づけたい語の近さの平均 − 遠ざけたい語の近さの平均」を上げる
//! 3. 元のつまみから離れすぎないよう、動かした量の小さな罰則を足す
//!
//! 近さは語ごとに「いろいろな音に対して出す値」と比べた値(z)なので、語どうしで比べられる。
//! CLAP の埋め込みは 1 回に 1 秒ほどかかるので、評価の回数は数十回にとどめる。

use glaux_core::{Effect, FxId, ParamRange, ParamValue, Project, TrackId};
use std::path::Path;

const RATE: f64 = 48_000.0;
/// 描き出す長さの上限(秒)
const MAX_SECS: f64 = 8.0;
/// 動かすつまみの数の上限
const MAX_DIMS: usize = 10;

/// 動かすつまみ 1 つ
#[derive(Clone, Debug)]
struct Knob {
    fx: FxId,
    name: String,
    min: f64,
    max: f64,
    /// 周波数など、比で動かすもの
    log: bool,
    start: f64,
}

impl Knob {
    fn to_unit(&self, v: f64) -> f64 {
        if self.log {
            (v / self.min).ln() / (self.max / self.min).ln()
        } else {
            (v - self.min) / (self.max - self.min)
        }
        .clamp(0.0, 1.0)
    }

    fn value_at(&self, x: f64) -> f64 {
        let x = x.clamp(0.0, 1.0);
        if self.log {
            self.min * (self.max / self.min).powf(x)
        } else {
            self.min + (self.max - self.min) * x
        }
    }
}

/// 変えたつまみ
#[derive(Clone, Debug, serde::Serialize)]
pub struct KnobChange {
    pub fx_id: String,
    pub effect: String,
    pub param: String,
    pub before: f64,
    pub after: f64,
}

/// 語ごとの近さ(前と後)
#[derive(Clone, Debug, serde::Serialize)]
pub struct WordShift {
    pub word: String,
    pub ja: String,
    pub before: f64,
    pub after: f64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct RefineOutcome {
    pub changes: Vec<KnobChange>,
    /// 近づけたい語の近さの平均 − 遠ざけたい語の近さの平均(前と後)
    pub score_before: f64,
    pub score_after: f64,
    pub toward: Vec<WordShift>,
    pub away: Vec<WordShift>,
    pub evaluations: usize,
    pub notes: Vec<String>,
}

/// トラックのエフェクトの並びが「入力 → 1 列 → 出口」か(分岐・合流があると、この方法では通せない)
fn serial_chain(track: &glaux_core::Track) -> Result<Vec<Effect>, String> {
    use glaux_core::model::routing::{effective_links, processing_order};
    let links = effective_links(&track.effects, track.fx_links.as_deref());
    let order = processing_order(&track.effects, &links);
    let mut prev = glaux_core::FxNode::Input;
    for id in order
        .iter()
        .map(|i| glaux_core::FxNode::Fx(i.clone()))
        .chain([glaux_core::FxNode::Output])
    {
        if !links.iter().any(|l| l.from == prev && l.to == id) {
            return Err(
                "エフェクトが分岐・合流しているトラックは、言葉での追い込みに対応していません"
                    .to_owned(),
            );
        }
        prev = id;
    }
    if links.len() != order.len() + 1 {
        return Err(
            "エフェクトが分岐・合流しているトラックは、言葉での追い込みに対応していません"
                .to_owned(),
        );
    }
    Ok(order
        .iter()
        .filter_map(|id| track.effects.iter().find(|e| &e.id == id).cloned())
        .collect())
}

fn builtin_name(e: &Effect) -> Option<&str> {
    match &e.source {
        glaux_core::PluginSource::Builtin { name } => Some(name.as_str()),
        _ => None,
    }
}

/// 動かすつまみを集める。`only` があればその名前(`<fx_id>/<name>` か `<name>`)だけ
fn knobs(chain: &[Effect], only: &[String]) -> Vec<Knob> {
    let mut out = Vec::new();
    for e in chain.iter().filter(|e| !e.bypass) {
        let Some(name) = builtin_name(e) else {
            continue;
        };
        let Some(specs) = glaux_dsp::effect_params_spec(name) else {
            continue;
        };
        for s in specs {
            let ParamRange::Float {
                min, max, default, ..
            } = s.range
            else {
                continue;
            };
            if !only.is_empty()
                && !only
                    .iter()
                    .any(|o| o == s.name || *o == format!("{}/{}", e.id, s.name))
            {
                continue;
            }
            let start = match e.params.get(s.name) {
                Some(ParamValue::Float(v)) => *v,
                Some(ParamValue::Int(v)) => *v as f64,
                _ => default,
            };
            out.push(Knob {
                fx: e.id.clone(),
                name: s.name.to_owned(),
                min,
                max,
                log: min > 0.0 && max / min >= 20.0,
                start: start.clamp(min, max),
            });
        }
    }
    out
}

/// エフェクトを順に通す(候補のつまみを当てて)
fn process(dry: &[f32], chain: &[Effect], knobs: &[Knob], x: &[f64]) -> Vec<f32> {
    let baked: Vec<glaux_dsp::EffectParams> = chain
        .iter()
        .filter(|e| !e.bypass)
        .filter_map(|e| {
            let mut e = e.clone();
            for (k, v) in knobs.iter().zip(x) {
                if k.fx == e.id {
                    e.params
                        .insert(k.name.clone(), ParamValue::Float(k.value_at(*v)));
                }
            }
            glaux_dsp::bake_effect(&e, RATE as f32, &|_| None)
        })
        .collect();
    let mut states: Vec<glaux_dsp::EffectState> = baked
        .iter()
        .map(|p| {
            let mut s = glaux_dsp::EffectState::default();
            s.ensure_kind(p);
            s
        })
        .collect();
    let mut out = Vec::with_capacity(dry.len() / 2);
    for c in dry.as_chunks::<2>().0 {
        let (mut l, mut r) = (c[0], c[1]);
        for (p, st) in baked.iter().zip(states.iter_mut()) {
            (l, r) = st.process(p, l, r, 0.0);
        }
        out.push((l + r) * 0.5);
    }
    out
}

/// 小数第 2 位までの f64(f32 のまま JSON にすると桁が崩れて見える)
fn r2(v: f32) -> f64 {
    (v as f64 * 100.0).round() / 100.0
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

/// 語を辞書で探す(英語か日本語)
fn words(list: &[String]) -> Result<Vec<&'static glaux_ml::clap::VocabWord>, String> {
    list.iter()
        .map(|w| {
            glaux_ml::clap::find_word(w).ok_or_else(|| {
                let known: Vec<String> = glaux_ml::clap::vocab()
                    .iter()
                    .filter(|v| v.category != "instrument")
                    .map(|v| format!("{}({})", v.en, v.ja))
                    .collect();
                format!(
                    "音色語「{w}」は辞書にありません。使える語: {}",
                    known.join("、")
                )
            })
        })
        .collect()
}

/// トラックの内蔵エフェクトのつまみを、言葉に近づく方へ動かす(プロジェクトは変えない。変える案を返す)
#[allow(clippy::too_many_arguments)]
pub fn refine_by_words(
    project: &Project,
    dir: &Path,
    track_id: &TrackId,
    toward: &[String],
    away: &[String],
    only: &[String],
    max_evals: usize,
) -> Result<RefineOutcome, String> {
    use cmaes::{CMAESOptions, DVector};
    if !glaux_ml::clap::available() {
        return Err(
            "音色語のモデル(CLAP)が入っていません。analyze_sound の案内に従って取得してください"
                .to_owned(),
        );
    }
    if toward.is_empty() && away.is_empty() {
        return Err(
            "toward(近づけたい語)か away(遠ざけたい語)を 1 つ以上指定してください".to_owned(),
        );
    }
    let (tw, aw) = (words(toward)?, words(away)?);
    let track = project
        .track(track_id)
        .ok_or_else(|| format!("トラックが見つかりません: {track_id}"))?;
    let chain = serial_chain(track)?;
    let ks = knobs(&chain, only);
    if ks.is_empty() {
        return Err("動かせる内蔵エフェクトのつまみがありません(先に add_effect で eq などを足してください)".to_owned());
    }
    let mut notes = Vec::new();
    if chain.iter().any(|e| builtin_name(e).is_none()) {
        notes.push("CLAP のエフェクトは通さずに評価した(内蔵エフェクトだけで判断)".to_owned());
    }
    let ks: Vec<Knob> = if ks.len() > MAX_DIMS {
        notes.push(format!(
            "つまみが多いので最初の {MAX_DIMS} 個だけ動かした(only で選べる)"
        ));
        ks.into_iter().take(MAX_DIMS).collect()
    } else {
        ks
    };

    // エフェクトを外したトラックだけを、最初に鳴る所から描き出す
    let start_tick = track
        .clips
        .iter()
        .map(|c| c.start)
        .min()
        .ok_or("トラックにクリップがありません")?;
    let mut solo = project.clone();
    solo.tracks.retain(|t| &t.id == track_id);
    for t in &mut solo.tracks {
        t.effects.clear();
        t.fx_links = None;
        t.solo = false;
        t.mute = false;
        t.volume_db = 0.0;
        t.pan = 0.0;
    }
    solo.master.effects.clear();
    solo.master.fx_links = None;
    solo.master.volume_db = 0.0;
    let s0 = project.tempo_map.tick_to_seconds(start_tick);
    let bank = glaux_engine::SampleBank::for_offline(&solo, dir);
    let dry = glaux_engine::export::render_project_range(&solo, RATE, &bank, s0, s0 + MAX_SECS)
        .map_err(|e| format!("描き出せません: {e}"))?;
    let dry_mono: Vec<f32> = dry
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| (c[0] + c[1]) * 0.5)
        .collect();
    let dry_rms = rms(&dry_mono);
    if dry_rms < 1e-5 {
        return Err("トラックの音が小さすぎて判断できません".to_owned());
    }

    let x0: Vec<f64> = ks.iter().map(|k| k.to_unit(k.start)).collect();
    let score = |x: &[f64]| -> Result<(f32, Vec<f32>, Vec<f32>), String> {
        let mut y = process(&dry, &chain, &ks, x);
        // 音量を元にそろえる(大きく・小さくするだけで印象を動かさないように)
        let g = dry_rms / rms(&y).max(1e-9);
        y.iter_mut().for_each(|v| *v *= g);
        let e = glaux_ml::clap::embed(&y, RATE as f32).map_err(|e| e.to_string())?;
        let t: Vec<f32> = tw.iter().map(|w| w.z(&e)).collect();
        let a: Vec<f32> = aw.iter().map(|w| w.z(&e)).collect();
        let mean = |v: &[f32]| {
            if v.is_empty() {
                0.0
            } else {
                v.iter().sum::<f32>() / v.len() as f32
            }
        };
        Ok((mean(&t) - mean(&a), t, a))
    };
    let (s_before, t_before, a_before) = score(&x0)?;
    let objective = |x: &DVector<f64>| -> f64 {
        let s = score(x.as_slice()).map(|r| r.0).unwrap_or(-10.0) as f64;
        let out: f64 = x
            .iter()
            .map(|v| (v - v.clamp(0.0, 1.0)).powi(2))
            .sum::<f64>()
            * 50.0;
        let moved: f64 = x.iter().zip(&x0).map(|(a, b)| (a - b).powi(2)).sum();
        -s + out + moved * 0.5
    };
    let evals = max_evals.clamp(8, 120);
    let mut cma = CMAESOptions::new(x0.clone(), 0.15)
        .population_size(6)
        .max_function_evals(evals)
        .seed(1)
        .build(objective)
        .map_err(|e| format!("{e:?}"))?;
    let result = cma.run();
    let evaluations = cma.function_evals();
    let mut best = result
        .overall_best
        .map(|b| b.point.as_slice().to_vec())
        .unwrap_or_else(|| x0.clone());
    best.iter_mut().for_each(|v| *v = v.clamp(0.0, 1.0));
    let (mut s_after, mut t_after, mut a_after) = score(&best)?;
    if s_after <= s_before {
        // 良くならなければ変えない
        best = x0.clone();
        (s_after, t_after, a_after) = (s_before, t_before.clone(), a_before.clone());
        notes.push(
            "言葉に近づくつまみが見つからなかった(エフェクトを足すか、別の語で試す)".to_owned(),
        );
    }
    let r3 = |v: f64| (v * 1000.0).round() / 1000.0;
    let changes = ks
        .iter()
        .zip(&best)
        .filter_map(|(k, x)| {
            let after = k.value_at(*x);
            let effect = chain
                .iter()
                .find(|e| e.id == k.fx)
                .and_then(builtin_name)
                .unwrap_or("")
                .to_owned();
            ((after - k.start).abs() > (k.max - k.min) * 0.005).then(|| KnobChange {
                fx_id: k.fx.to_string(),
                effect,
                param: k.name.clone(),
                before: r3(k.start),
                after: r3(after),
            })
        })
        .collect();
    let shifts = |ws: &[&glaux_ml::clap::VocabWord], b: &[f32], a: &[f32]| -> Vec<WordShift> {
        ws.iter()
            .zip(b.iter().zip(a))
            .map(|(w, (b, a))| WordShift {
                word: w.en.clone(),
                ja: w.ja.clone(),
                before: r2(*b),
                after: r2(*a),
            })
            .collect()
    };
    Ok(RefineOutcome {
        changes,
        score_before: r2(s_before),
        score_after: r2(s_after),
        toward: shifts(&tw, &t_before, &t_after),
        away: shifts(&aw, &a_before, &a_after),
        evaluations,
        notes,
    })
}
