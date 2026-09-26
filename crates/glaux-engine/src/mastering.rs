//! マスタリングの助手と、参照曲に寄せる。
//!
//! 曲を 1 回だけ描き出し、その音に対してマスターの最後に足すエフェクト(EQ → コンプ → 幅 → リミッタ)の
//! つまみを決める。
//!
//! 1. 音色の釣り合い(EQ): 1/3 オクターブの釣り合いの「目標との差」に、EQ の周波数特性が合うよう
//!    CMA-ES で当てはめる。特性は計算で求める([`glaux_dsp::EqParams::magnitude_db`])ので、音を通さず速い。
//!    目標は、参照曲があればその釣り合い、無ければ曲自身の傾きの直線(出っ張り・へこみだけを均す)
//! 2. ダイナミクス(コンプ): 大きい所で 2dB ほど効く、ゆるいコンプ(2:1、RMS、検出の低域は切る)
//! 3. 幅: 参照曲とサイドの量の差から。参照曲の低域が中央に集まっていれば、低域をモノに
//! 4. 音量(リミッタ): 上限 -1 dBTP。目標のラウドネス(参照曲、または配信先)になるよう入力を二分探索で決める。
//!    曲より大きくする必要が無ければ、マスター音量を下げる
//!
//! 音を変えすぎないよう、EQ は ±6dB、幅は 0.6〜1.4 に抑える。

use crate::analyze::{integrated_lufs, stereo_info, tonal_balance};
use glaux_core::{Effect, FxId, ParamMap, ParamValue};
use serde::Serialize;

const SR: f32 = 48_000.0;

/// 目標のラウドネスの選び方
#[derive(Clone, Copy, Debug)]
pub enum LoudnessTarget {
    /// 参照曲と同じ
    Reference,
    /// 指定の値(LUFS)
    Lufs(f64),
}

/// 音の要約(比べる用)
#[derive(Clone, Debug, Serialize)]
pub struct MasterMetrics {
    pub loudness_lufs: f64,
    pub true_peak_dbtp: f64,
    /// True Peak − 統合ラウドネス
    pub plr_db: f64,
    /// 50Hz〜10kHz の傾き(dB/oct)
    pub tonal_slope_db_per_oct: f64,
    /// 中・高域のサイドとミッドの比(dB)
    pub side_to_mid_db: f64,
    /// 250Hz 以下の左右の相関
    pub low_correlation: f64,
}

/// 足すエフェクト 1 つ
#[derive(Clone, Debug, Serialize)]
pub struct PlannedEffect {
    pub name: &'static str,
    pub label: &'static str,
    pub params: Vec<(String, f64)>,
    /// 選択肢のつまみ(コンプの detector など)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<(String, &'static str)>,
}

/// マスタリングの案
#[derive(Clone, Debug, Serialize)]
pub struct MasterPlan {
    /// マスターの最後に足すエフェクト(この順)
    pub effects: Vec<PlannedEffect>,
    /// マスター音量(dB)。曲が目標より大きいときだけ下げる
    pub master_volume_db: f64,
    pub before: MasterMetrics,
    pub after: MasterMetrics,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<MasterMetrics>,
    /// 1/3 オクターブの釣り合いの目標とのずれ(RMS、dB)。EQ の前と後
    pub tonal_error_db: (f64, f64),
    pub notes: Vec<String>,
}

impl PlannedEffect {
    /// プロジェクトに足すエフェクト(新しい ID)
    pub fn to_effect(&self) -> Effect {
        let mut e = Effect::builtin(FxId::new(), self.name);
        e.params = self.param_map();
        e.ui.label = Some(self.label.to_owned());
        e
    }

    fn param_map(&self) -> ParamMap {
        let mut m = ParamMap::new();
        for (k, v) in &self.params {
            m.insert(k.clone(), ParamValue::Float(*v));
        }
        for (k, v) in &self.choices {
            m.insert(k.clone(), ParamValue::Enum((*v).to_owned()));
        }
        m
    }
}

fn metrics(stereo: &[f32]) -> MasterMetrics {
    let lufs = integrated_lufs(stereo);
    let tp = crate::loudness::true_peak_db(stereo);
    let mono: Vec<f32> = stereo
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| (c[0] + c[1]) * 0.5)
        .collect();
    let tb = tonal_balance(&mono);
    let st = stereo_info(stereo);
    let side = st
        .bands
        .iter()
        .filter(|b| b.band != "low")
        .map(|b| b.side_to_mid_db)
        .sum::<f64>()
        / 2.0;
    let r1 = |v: f64| (v * 10.0).round() / 10.0;
    MasterMetrics {
        loudness_lufs: r1(lufs),
        true_peak_dbtp: r1(tp),
        plr_db: r1(tp - lufs),
        tonal_slope_db_per_oct: tb.slope_db_per_oct,
        side_to_mid_db: r1(side),
        low_correlation: st.low_correlation,
    }
}

/// エフェクトを順に通す(オフライン)
fn apply(stereo: &[f32], chain: &[PlannedEffect]) -> Vec<f32> {
    let baked: Vec<glaux_dsp::EffectParams> = chain
        .iter()
        .filter_map(|p| {
            let mut e = Effect::builtin(FxId::new(), p.name);
            e.params = p.param_map();
            glaux_dsp::bake_effect(&e, SR, &|_| None)
        })
        .collect();
    let mut states: Vec<glaux_dsp::EffectState> = baked
        .iter()
        .map(|p| {
            let mut s = glaux_dsp::EffectState::without_delay_buffers();
            s.ensure_kind(p);
            s
        })
        .collect();
    let mut out = Vec::with_capacity(stereo.len());
    for c in stereo.as_chunks::<2>().0 {
        let (mut l, mut r) = (c[0], c[1]);
        for (p, st) in baked.iter().zip(states.iter_mut()) {
            (l, r) = st.process(p, l, r, 0.0);
        }
        out.push(l);
        out.push(r);
    }
    // リミッタの遅れの分だけ先頭がずれるが、測るだけなので気にしない
    out
}

/// 1/3 オクターブの釣り合い(全体に対する dB)
fn tonal(stereo: &[f32]) -> Vec<(u32, f64)> {
    let mono: Vec<f32> = stereo
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| (c[0] + c[1]) * 0.5)
        .collect();
    tonal_balance(&mono).third_octave
}

/// 当てはめる帯域の重み(40Hz〜16kHz。両端は軽く)
fn weight(hz: u32) -> f64 {
    match hz {
        0..=39 => 0.0,
        40..=63 => 0.5,
        64..=12_500 => 1.0,
        12_501..=16_000 => 0.5,
        _ => 0.0,
    }
}

/// EQ のつまみ(6 次元、0..1 に正規化)→ パラメータ
fn eq_params(x: &[f64]) -> Vec<(String, f64)> {
    let c = |v: f64| v.clamp(0.0, 1.0);
    let log = |v: f64, lo: f64, hi: f64| lo * (hi / lo).powf(c(v));
    let gain = |v: f64| (c(v) - 0.5) * 12.0;
    vec![
        ("low_freq".into(), log(x[0], 60.0, 300.0)),
        ("low_gain_db".into(), gain(x[1])),
        ("mid_freq".into(), log(x[2], 200.0, 6000.0)),
        ("mid_gain_db".into(), gain(x[3])),
        ("mid_q".into(), 0.8),
        ("high_freq".into(), log(x[4], 3000.0, 12_000.0)),
        ("high_gain_db".into(), gain(x[5])),
    ]
}

/// EQ の特性(dB)を帯域の中心で
fn eq_response(params: &[(String, f64)], bands: &[u32]) -> Vec<f64> {
    let mut e = Effect::builtin(FxId::new(), "eq");
    for (k, v) in params {
        e.params.insert(k.clone(), ParamValue::Float(*v));
    }
    match glaux_dsp::bake_effect(&e, SR, &|_| None) {
        Some(glaux_dsp::EffectParams::Eq(eq)) => bands
            .iter()
            .map(|f| eq.magnitude_db(SR, *f as f32) as f64)
            .collect(),
        _ => vec![0.0; bands.len()],
    }
}

/// 重み付きのずれ(全体の上下は音量で合わせるので、平均を引いてから)の RMS
fn tonal_error(diff: &[f64], bands: &[u32]) -> f64 {
    let (mut sw, mut s) = (0.0, 0.0);
    for (d, b) in diff.iter().zip(bands) {
        sw += weight(*b);
        s += weight(*b) * d;
    }
    let mean = s / sw.max(1e-9);
    let e: f64 = diff
        .iter()
        .zip(bands)
        .map(|(d, b)| weight(*b) * (d - mean).powi(2))
        .sum();
    (e / sw.max(1e-9)).sqrt()
}

/// 目標とのずれ(目標 − 曲)を、両隣と平均してなめらかに(細かい山谷まで追わない)
fn target_delta(mix: &[(u32, f64)], target: &[f64]) -> Vec<f64> {
    let raw: Vec<f64> = mix.iter().zip(target).map(|((_, m), t)| t - m).collect();
    (0..raw.len())
        .map(|i| {
            let lo = i.saturating_sub(1);
            let hi = (i + 1).min(raw.len() - 1);
            raw[lo..=hi].iter().sum::<f64>() / (hi - lo + 1) as f64
        })
        .collect()
}

/// EQ を当てはめる。戻り値は (つまみ, 当てはめる前のずれ, 後のずれ)
fn fit_eq(delta: &[f64], bands: &[u32]) -> (Vec<(String, f64)>, f64, f64) {
    use cmaes::{CMAESOptions, DVector};
    let before = tonal_error(delta, bands);
    let cost = |x: &DVector<f64>| -> f64 {
        let p = eq_params(x.as_slice());
        let resp = eq_response(&p, bands);
        let diff: Vec<f64> = delta.iter().zip(&resp).map(|(d, r)| d - r).collect();
        // 範囲外の罰則と、上げ下げの量の小さな罰則(同じくらい合うなら控えめな方)
        let out: f64 = x
            .iter()
            .map(|v| (v - v.clamp(0.0, 1.0)).powi(2))
            .sum::<f64>()
            * 100.0;
        let gains: f64 = [1usize, 3, 5].iter().map(|&i| p[i].1.powi(2)).sum::<f64>();
        tonal_error(&diff, bands).powi(2) + out + gains * 0.002
    };
    let init = vec![0.5, 0.5, 0.5, 0.5, 0.5, 0.5];
    let best = CMAESOptions::new(init.clone(), 0.25)
        .population_size(12)
        .max_function_evals(3000)
        .seed(1)
        .build(cost)
        .ok()
        .and_then(|mut c| c.run().overall_best)
        .map(|b| b.point.as_slice().to_vec())
        .unwrap_or(init);
    let p = eq_params(&best);
    let resp = eq_response(&p, bands);
    let after: Vec<f64> = delta.iter().zip(&resp).map(|(d, r)| d - r).collect();
    let r2 = |v: f64| (v * 100.0).round() / 100.0;
    let p = p
        .into_iter()
        .map(|(k, v)| (k, (v * 10.0).round() / 10.0))
        .collect();
    (p, r2(before), r2(tonal_error(&after, bands)))
}

/// いちばん大きい 20 秒(短い曲は全体)
fn loudest_excerpt(stereo: &[f32]) -> &[f32] {
    let win = (20.0 * SR) as usize * 2;
    if stereo.len() <= win {
        return stereo;
    }
    let step = (SR as usize) * 2;
    let mut best = (0usize, -1.0f64);
    let mut i = 0;
    while i + win <= stereo.len() {
        let e: f64 = stereo[i..i + win]
            .iter()
            .step_by(7)
            .map(|v| (*v as f64).powi(2))
            .sum();
        if e > best.1 {
            best = (i, e);
        }
        i += step;
    }
    &stereo[best.0..best.0 + win]
}

/// マスタリングの案を作る。`mix` と `reference` はステレオ 48kHz(インターリーブ)。
/// `mix` はマスター音量 0dB で描き出したもの
pub fn plan_master(mix: &[f32], reference: Option<&[f32]>, target: LoudnessTarget) -> MasterPlan {
    let mut notes = Vec::new();
    let before = metrics(mix);
    let ref_metrics = reference.map(metrics);
    let mix_tonal = tonal(mix);
    let bands: Vec<u32> = mix_tonal.iter().map(|(b, _)| *b).collect();

    // 1. EQ
    let target_curve: Vec<f64> = match reference {
        Some(r) => tonal(r).into_iter().map(|(_, d)| d).collect(),
        None => {
            // 曲自身の傾きの直線: 出っ張り・へこみだけを均す
            let mono: Vec<f32> = mix
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| (c[0] + c[1]) * 0.5)
                .collect();
            let tb = tonal_balance(&mono);
            let pts: Vec<(f64, f64)> = mix_tonal
                .iter()
                .filter(|(b, _)| (50..=10_000).contains(b))
                .map(|(b, d)| ((*b as f64).log2(), *d))
                .collect();
            let mx = pts.iter().map(|p| p.0).sum::<f64>() / pts.len().max(1) as f64;
            let my = pts.iter().map(|p| p.1).sum::<f64>() / pts.len().max(1) as f64;
            mix_tonal
                .iter()
                .map(|(b, d)| {
                    let line = my + tb.slope_db_per_oct * ((*b as f64).log2() - mx);
                    // 2dB 未満のずれは目標にしない(曲の個性を残す)
                    if (d - line).abs() < 2.0 {
                        *d
                    } else {
                        line + (d - line).signum() * 2.0
                    }
                })
                .collect()
        }
    };
    let delta = target_delta(&mix_tonal, &target_curve);
    let (eq, err0, err1) = fit_eq(&delta, &bands);
    let mut chain = vec![PlannedEffect {
        name: "eq",
        label: "マスター EQ",
        params: eq,
        choices: vec![],
    }];

    // 2. コンプ: 大きい所の RMS の 4dB 下から、2:1 でゆるく
    let loud = loudest_excerpt(mix);
    let rms_db = 10.0
        * (loud.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / loud.len().max(1) as f64)
            .max(1e-12)
            .log10();
    chain.push(PlannedEffect {
        name: "compressor",
        label: "マスターのコンプ",
        params: vec![
            (
                "threshold_db".into(),
                ((rms_db - 4.0).clamp(-40.0, 0.0) * 10.0).round() / 10.0,
            ),
            ("ratio".into(), 2.0),
            ("knee_db".into(), 6.0),
            ("attack_ms".into(), 30.0),
            ("release_ms".into(), 150.0),
            ("makeup_db".into(), 0.0),
            ("sc_hpf_hz".into(), 100.0),
        ],
        choices: vec![("detector".into(), "rms")],
    });
    // 3. 幅
    if let Some(r) = &ref_metrics {
        let width = 10f64
            .powf((r.side_to_mid_db - before.side_to_mid_db) / 20.0)
            .clamp(0.6, 1.4);
        let mono_low = r.low_correlation > 0.9 && before.low_correlation < 0.9;
        if (width - 1.0).abs() > 0.05 || mono_low {
            chain.push(PlannedEffect {
                name: "width",
                label: "マスターの幅",
                params: vec![
                    ("width".into(), (width * 100.0).round() / 100.0),
                    ("mono_below_hz".into(), if mono_low { 120.0 } else { 20.0 }),
                ],
                choices: vec![],
            });
        }
    }

    // 4. 音量: リミッタの入力を二分探索(いちばん大きい 20 秒で。曲全体とのずれは前もって測って補う)
    let target_lufs = match (target, &ref_metrics) {
        (LoudnessTarget::Reference, Some(r)) => r.loudness_lufs,
        (LoudnessTarget::Lufs(v), _) => v,
        (LoudnessTarget::Reference, None) => -14.0,
    };
    let pre = apply(mix, &chain);
    let pre_loud = loudest_excerpt(&pre).to_vec();
    let offset = integrated_lufs(&pre_loud) - integrated_lufs(&pre);
    let limiter = |input: f64| PlannedEffect {
        name: "limiter",
        label: "マスターのリミッタ",
        params: vec![
            ("input_db".into(), (input * 10.0).round() / 10.0),
            ("ceiling_db".into(), -1.0),
            ("release_ms".into(), 100.0),
        ],
        choices: vec![],
    };
    let lufs_with = |input: f64| integrated_lufs(&apply(&pre_loud, &[limiter(input)])) - offset;
    let mut master_volume_db = 0.0;
    let at0 = lufs_with(0.0);
    let input = if at0 >= target_lufs {
        // すでに目標より大きい: リミッタは上限を守るだけにして、マスター音量で下げる
        master_volume_db = ((target_lufs - at0) * 10.0).round() / 10.0;
        0.0
    } else {
        let (mut lo, mut hi) = (0.0f64, 18.0f64);
        if lufs_with(hi) < target_lufs {
            notes.push(format!(
                "目標 {target_lufs:.1} LUFS まで上げるには、リミッタを 18dB より強く掛ける必要がある(18dB で止めた)"
            ));
            lo = hi;
        }
        for _ in 0..10 {
            if hi - lo < 0.1 {
                break;
            }
            let mid = 0.5 * (lo + hi);
            if lufs_with(mid) < target_lufs {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        hi.max(lo)
    };
    chain.push(limiter(input));

    let mut out = apply(mix, &chain);
    if master_volume_db != 0.0 {
        let g = 10f32.powf(master_volume_db as f32 / 20.0);
        out.iter_mut().for_each(|v| *v *= g);
    }
    let after = metrics(&out);
    if after.plr_db < 8.0 && after.plr_db < before.plr_db - 1.0 {
        notes.push(format!(
            "PLR が {:.1} → {:.1} dB に縮んだ(8 を下回ると潰しすぎの目安)。目標のラウドネスを下げるか、元のミックスのピークを整えるとよい",
            before.plr_db, after.plr_db
        ));
    }
    if input > 8.0 {
        notes.push(format!(
            "リミッタで {input:.1} dB 持ち上げている。配信では正規化されるので、無理に大きくしなくてよい"
        ));
    }
    MasterPlan {
        effects: chain,
        master_volume_db,
        before,
        after,
        reference: ref_metrics,
        tonal_error_db: (err0, err1),
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 低域を強くした雑音(ピンクに近いもの)を作る
    fn tilted_noise(seed: u32, n: usize, low_boost: f32, amp: f32) -> Vec<f32> {
        let mut r = seed;
        let mut lp = 0.0f32;
        (0..n)
            .flat_map(|_| {
                r ^= r << 13;
                r ^= r >> 17;
                r ^= r << 5;
                let x = r as f32 / u32::MAX as f32 - 0.5;
                lp += (x - lp) * 0.02;
                let v = (x + lp * low_boost) * amp;
                [v, v]
            })
            .collect()
    }

    #[test]
    fn matches_reference_tone_and_loudness() {
        let n = 48_000 * 6;
        // 参照: 低域が強く大きい。曲: 低域が弱く小さい
        let reference = tilted_noise(1, n, 6.0, 0.25);
        let mix = tilted_noise(2, n, 1.0, 0.1);
        let plan = plan_master(&mix, Some(&reference), LoudnessTarget::Reference);
        let r = plan.reference.as_ref().unwrap();
        eprintln!("{:#?}", plan);
        assert!(
            plan.tonal_error_db.1 < plan.tonal_error_db.0 * 0.6,
            "釣り合いが近づく"
        );
        let low = plan.effects[0]
            .params
            .iter()
            .find(|(k, _)| k == "low_gain_db")
            .unwrap()
            .1;
        assert!(low > 2.0, "低域を持ち上げる: {low}");
        assert!(
            (plan.after.loudness_lufs - r.loudness_lufs).abs() < 1.0,
            "音量が参照に合う: {} / {}",
            plan.after.loudness_lufs,
            r.loudness_lufs
        );
        assert!(
            plan.after.true_peak_dbtp <= -0.8,
            "{}",
            plan.after.true_peak_dbtp
        );
    }

    #[test]
    fn without_reference_reaches_the_streaming_target_and_lowers_loud_mixes() {
        let n = 48_000 * 6;
        let quiet = tilted_noise(3, n, 2.0, 0.05);
        let plan = plan_master(&quiet, None, LoudnessTarget::Lufs(-14.0));
        assert!(
            (plan.after.loudness_lufs + 14.0).abs() < 1.0,
            "{}",
            plan.after.loudness_lufs
        );
        assert_eq!(plan.master_volume_db, 0.0);
        // すでに大きい曲はマスター音量で下げる
        let loud = tilted_noise(4, n, 2.0, 0.9);
        let plan = plan_master(&loud, None, LoudnessTarget::Lufs(-14.0));
        assert!(plan.master_volume_db < 0.0, "{}", plan.master_volume_db);
        assert!(
            (plan.after.loudness_lufs + 14.0).abs() < 1.0,
            "{}",
            plan.after.loudness_lufs
        );
    }
}
