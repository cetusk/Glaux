//! 歪み・押さえ込みの測り方(MCP の `check_distortion` の実体)。AI が「歪ませて潰れていないか」を数で掴むため。
//!
//! - エフェクトの効き目([`fx_impact`]): トラックをソロで、エフェクトありとエフェクトを外したものの 2 つを描き出し、
//!   クレスト・PLR・立ち上がりの強さ・4 kHz 以上の量・平坦さ(雑音っぽさ)を比べる。立ち上がりは、外した音で
//!   見つけた音の頭の同じ位置で測る(両方で同じ音を比べる)
//! - 試験の音([`fx_probe`]): トラックのエフェクトの列に、1 音と 2 音(長 3 度)を音量を変えて通し、
//!   歪み率(THD)・相互変調(IMD)・押さえ込み(小さい音に対して大きい音がどれだけ増えないか)を測る。
//!   音の高さはトラックのノートの音域(真ん中の音)に合わせる(ベースに 440 Hz を通すと、EQ で削られて比べられない)
//!
//! どちらも 48 kHz。エフェクトの並列のつなぎ方(fx_links)は、試験の音では並びの順の直列として測る。

use crate::analyze::{integrated_lufs, render_for_analysis, welch_power};
use crate::export::ExportError;
use glaux_core::{Project, Tick, TrackId};
use serde::Serialize;

const SR: f64 = 48_000.0;

fn db(a: f64) -> f64 {
    20.0 * a.max(1e-9).log10()
}
fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// 音 1 つぶんの指標
#[derive(Clone, Debug, Serialize)]
pub struct Levels {
    pub loudness_lufs: f64,
    pub crest_factor_db: f64,
    pub plr_db: f64,
    /// 音の頭の強さ(頭 10 ms のピーク − その後 30〜130 ms の RMS、dB。音の頭ごとの平均)。下がると立ち上がりが丸く・平らに
    pub attack_db: Option<f64>,
    /// 4 kHz 以上のエネルギーの割合(dB。全体に対して)
    pub high_db: f64,
    /// スペクトルの平坦さ 0〜1(100 Hz〜16 kHz。1 に近いほど雑音っぽい。歪みで倍音が密になると上がる)
    pub flatness: f64,
}

/// エフェクトの効き目(エフェクトありとエフェクトを外した音の比較)
#[derive(Clone, Debug, Serialize)]
pub struct FxImpact {
    pub with_fx: Levels,
    pub without_fx: Levels,
    /// あり − なし
    pub loudness_change_db: f64,
    pub crest_change_db: f64,
    pub attack_change_db: Option<f64>,
    pub high_change_db: f64,
    pub flatness_change: f64,
    /// 比べて分かったこと(日本語の短い文)
    pub notes: Vec<String>,
}

/// 音の頭(サンプルの位置)。5 ms ごとのエネルギーが、直前 50 ms の平均より 6 dB 以上上がり、-50 dBFS を超えた所。
/// 近すぎる頭(80 ms 以内)は 1 つにまとめる。最大 64 個
fn onsets(mono: &[f32]) -> Vec<usize> {
    let hop = (SR * 0.005) as usize;
    let env: Vec<f64> = mono
        .chunks(hop)
        .map(|c| c.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / c.len().max(1) as f64)
        .collect();
    let mut out: Vec<usize> = Vec::new();
    for i in 10..env.len() {
        let prev = env[i - 10..i].iter().sum::<f64>() / 10.0;
        let e = env[i];
        if 10.0 * e.max(1e-12).log10() > -50.0 && e > prev * 4.0 {
            let pos = i * hop;
            if out.last().is_none_or(|&l| pos - l > (SR * 0.08) as usize) {
                out.push(pos);
                if out.len() >= 64 {
                    break;
                }
            }
        }
    }
    out
}

fn attack_db(mono: &[f32], heads: &[usize]) -> Option<f64> {
    let ms = |m: f64| (SR * m / 1000.0) as usize;
    let vals: Vec<f64> = heads
        .iter()
        .filter(|&&o| o + ms(130.0) <= mono.len())
        .filter_map(|&o| {
            let peak = mono[o..o + ms(10.0)]
                .iter()
                .fold(0.0f32, |m, s| m.max(s.abs())) as f64;
            let tail = &mono[o + ms(30.0)..o + ms(130.0)];
            let rms =
                (tail.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / tail.len() as f64).sqrt();
            (peak > 1e-4).then(|| db(peak) - db(rms))
        })
        .collect();
    (!vals.is_empty()).then(|| r1(vals.iter().sum::<f64>() / vals.len() as f64))
}

fn levels(stereo: &[f32], heads: &[usize]) -> Levels {
    let mono: Vec<f32> = stereo
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| (c[0] + c[1]) * 0.5)
        .collect();
    let peak = stereo.iter().fold(0.0f32, |m, s| m.max(s.abs())) as f64;
    let ms = stereo.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / stereo.len().max(1) as f64;
    let rms_db = 10.0 * ms.max(1e-12).log10();
    let lufs = integrated_lufs(stereo);
    let n = 4096;
    let power = welch_power(&mono, n);
    let bin = SR / n as f64;
    let total: f64 = power.iter().sum::<f64>().max(1e-18);
    let high: f64 = power
        .iter()
        .enumerate()
        .filter(|(i, _)| *i as f64 * bin >= 4000.0)
        .map(|(_, p)| p)
        .sum();
    let band: Vec<f64> = power
        .iter()
        .enumerate()
        .filter(|(i, _)| (100.0..16_000.0).contains(&(*i as f64 * bin)))
        .map(|(_, p)| p.max(1e-20))
        .collect();
    let flatness = if band.is_empty() {
        0.0
    } else {
        let g = (band.iter().map(|p| p.ln()).sum::<f64>() / band.len() as f64).exp();
        let a = band.iter().sum::<f64>() / band.len() as f64;
        g / a.max(1e-20)
    };
    Levels {
        loudness_lufs: r1(lufs),
        crest_factor_db: r1(db(peak) - rms_db),
        plr_db: if lufs.is_finite() {
            r1(crate::loudness::true_peak_db(stereo) - lufs)
        } else {
            f64::NAN
        },
        attack_db: attack_db(&mono, heads),
        high_db: r1(10.0 * (high / total).max(1e-12).log10()),
        flatness: (flatness * 1000.0).round() / 1000.0,
    }
}

/// トラックをソロで、エフェクトありとエフェクトを外したもので描き出して比べる
pub fn fx_impact(
    project: &Project,
    track: &TrackId,
    range: Option<(Tick, Tick)>,
    bank: &crate::data::SampleBank,
) -> Result<FxImpact, String> {
    let mut wet = project.clone();
    wet.tracks.retain(|t| &t.id == track);
    let Some(t) = wet.tracks.first_mut() else {
        return Err(format!("track not found: {track}"));
    };
    t.solo = false;
    t.mute = false;
    if t.effects.iter().all(|e| e.bypass || e.ui.parked) {
        return Err("このトラックには効いているエフェクトがありません".to_owned());
    }
    let mut dry = wet.clone();
    dry.tracks[0].effects.clear();
    dry.tracks[0].fx_links = None;
    let render = |p: &Project| -> Result<Vec<f32>, String> {
        render_for_analysis(p, range, bank).map_err(|e: ExportError| e.to_string())
    };
    let (a, b) = std::thread::scope(|s| {
        let h = s.spawn(|| render(&wet));
        (
            h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)),
            render(&dry),
        )
    });
    let (a, b) = (a?, b?);
    let dry_mono: Vec<f32> = b
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| (c[0] + c[1]) * 0.5)
        .collect();
    let heads = onsets(&dry_mono);
    let (w, d) = (levels(&a, &heads), levels(&b, &heads));
    let crest = r1(w.crest_factor_db - d.crest_factor_db);
    let attack = match (w.attack_db, d.attack_db) {
        (Some(x), Some(y)) => Some(r1(x - y)),
        _ => None,
    };
    let high = r1(w.high_db - d.high_db);
    let flat = ((w.flatness - d.flatness) * 1000.0).round() / 1000.0;
    let mut notes = Vec::new();
    if crest <= -3.0 {
        notes.push(format!("クレストが {:.1} dB 下がった: ピークが押さえ込まれて潰れている(打楽器は芯が無くなり、持続音は平たくなる)", -crest));
    }
    if let Some(a) = attack {
        if a <= -3.0 {
            notes.push(format!("音の頭が {:.1} dB 弱まった: 立ち上がりが丸く・平らになっている(アタックが前に出ない)", -a));
        } else if a >= 3.0 {
            notes.push(format!(
                "音の頭が {a:.1} dB 強まった: 立ち上がりが立っている"
            ));
        }
    }
    if high >= 3.0 {
        notes.push(format!("4 kHz 以上が {high:.1} dB 増えた: 歪みの倍音が足されて明るく・ざらついている(多すぎると耳に刺さる)"));
    }
    if flat >= 0.05 {
        notes.push(format!(
            "平坦さが {flat:.3} 上がった: 倍音が密になって雑音っぽく・濁っている"
        ));
    }
    if notes.is_empty() {
        notes.push(
            "エフェクトを外した音と比べて、潰れ・ざらつきの大きな変化は見当たらない".to_owned(),
        );
    }
    Ok(FxImpact {
        loudness_change_db: r1(w.loudness_lufs - d.loudness_lufs),
        crest_change_db: crest,
        attack_change_db: attack,
        high_change_db: high,
        flatness_change: flat,
        with_fx: w,
        without_fx: d,
        notes,
    })
}

/// 試験の音 1 つの入力の大きさでの結果
#[derive(Clone, Debug, Serialize)]
pub struct ProbePoint {
    /// 入力のピーク(dBFS)
    pub input_db: f64,
    /// 1 音の出力の基音の大きさ − 入力(dB)
    pub gain_db: f64,
    /// 一番小さい入力のときの gain_db からどれだけ下がったか(dB)。大きいほど押さえ込まれている
    pub compression_db: f64,
    /// 歪み率(2〜10 倍音の和 / 基音、%)
    pub thd_pct: f64,
    /// 偶数倍音 − 奇数倍音(dB、±60 まで)。プラス = 偶数が多い(温かい・真空管・テープ寄り)、
    /// マイナス = 奇数が多い(硬い・ファズ・クリップ寄り)。歪みがごく小さい(0.3 % 未満)ときは null
    pub even_minus_odd_db: Option<f64>,
    /// 2 音(1 音の高さと、その長 3 度上)の相互変調(差音・和音など 6 つの和 / 2 音、%)。高いと和音が濁る
    pub imd_pct: f64,
}

/// 試験の音の結果
#[derive(Clone, Debug, Serialize)]
pub struct FxProbe {
    /// 通したエフェクト(並びの順。内蔵のものだけ)
    pub chain: Vec<String>,
    /// 試験の音の高さ(Hz。トラックのノートの真ん中の音。ノートが無ければ 220)
    pub tone_hz: f64,
    pub points: Vec<ProbePoint>,
    /// 分かったこと(日本語の短い文)
    pub notes: Vec<String>,
}

/// 周波数 f の成分の大きさ(Goertzel。窓を掛けた振幅)
fn tone(x: &[f32], f: f64) -> f64 {
    let n = x.len();
    let w = std::f64::consts::TAU * f / SR;
    let (mut re, mut im, mut wsum) = (0.0f64, 0.0f64, 0.0f64);
    for (i, v) in x.iter().enumerate() {
        let hann = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
        let a = i as f64 * w;
        re += *v as f64 * hann * a.cos();
        im += *v as f64 * hann * a.sin();
        wsum += hann;
    }
    2.0 * (re * re + im * im).sqrt() / wsum.max(1e-9)
}

/// トラックのエフェクトの列に試験の音を通す
pub fn fx_probe(track: &glaux_core::Track) -> FxProbe {
    let sr = SR as f32;
    let chain: Vec<(String, glaux_dsp::EffectParams)> = track
        .effects
        .iter()
        .filter(|e| !e.bypass && !e.ui.parked)
        .filter_map(|e| {
            let name = match &e.source {
                glaux_core::PluginSource::Builtin { name } => name.clone(),
                _ => return None,
            };
            glaux_dsp::bake_effect(e, sr, &|_| None).map(|p| (name, p))
        })
        .collect();
    let run = |input: &[f32]| -> Vec<f32> {
        let mut states: Vec<glaux_dsp::EffectState> = chain
            .iter()
            .map(|_| glaux_dsp::EffectState::default())
            .collect();
        for (st, (_, p)) in states.iter_mut().zip(&chain) {
            st.ensure_kind(p);
        }
        input
            .iter()
            .map(|&x| {
                let (mut l, mut r) = (x, x);
                for (st, (_, p)) in states.iter_mut().zip(&chain) {
                    (l, r) = st.process(p, l, r, 0.0);
                }
                (l + r) * 0.5
            })
            .collect()
    };
    let len = (SR * 1.0) as usize;
    let skip = (SR * 0.5) as usize;
    // 試験の音の高さ: トラックのノートの真ん中の音(40〜1000 Hz)
    let mut pitches: Vec<u8> = track
        .clips
        .iter()
        .filter_map(|c| c.notes())
        .flat_map(|ns| ns.iter().map(|n| n.pitch))
        .collect();
    pitches.sort_unstable();
    let f0 = pitches
        .get(pitches.len() / 2)
        .map_or(220.0, |&m| 440.0 * 2f64.powf((m as f64 - 69.0) / 12.0))
        .clamp(40.0, 1000.0);
    let (f1, f2) = (f0, f0 * 2f64.powf(4.0 / 12.0));
    let mut points: Vec<ProbePoint> = Vec::new();
    for input_db in [-30.0f64, -24.0, -18.0, -12.0, -6.0, 0.0] {
        let amp = 10f64.powf(input_db / 20.0);
        let one: Vec<f32> = (0..len)
            .map(|i| (amp * (std::f64::consts::TAU * f0 * i as f64 / SR).sin()) as f32)
            .collect();
        let y = run(&one);
        let y = &y[skip..];
        let a1 = tone(y, f0);
        let hs: Vec<f64> = (2..=10)
            .map(|k| f0 * k as f64)
            .filter(|f| *f < SR * 0.45)
            .map(|f| tone(y, f))
            .collect();
        let thd = hs.iter().map(|h| h * h).sum::<f64>().sqrt() / a1.max(1e-9);
        let even = hs.iter().step_by(2).map(|h| h * h).sum::<f64>();
        let odd = hs.iter().skip(1).step_by(2).map(|h| h * h).sum::<f64>();
        // 2 音: 合わせたピークが入力の大きさになるように半分ずつ
        let two: Vec<f32> = (0..len)
            .map(|i| {
                let t = i as f64 / SR;
                (amp * 0.5
                    * ((std::f64::consts::TAU * f1 * t).sin()
                        + (std::f64::consts::TAU * f2 * t).sin())) as f32
            })
            .collect();
        let z = run(&two);
        let z = &z[skip..];
        let base = (tone(z, f1).powi(2) + tone(z, f2).powi(2)).sqrt();
        let prods = [
            f2 - f1,
            2.0 * f1 - f2,
            2.0 * f2 - f1,
            f1 + f2,
            3.0 * f1 - 2.0 * f2,
            3.0 * f2 - 2.0 * f1,
        ];
        let imd = prods
            .iter()
            .map(|&f| tone(z, f).powi(2))
            .sum::<f64>()
            .sqrt()
            / base.max(1e-9);
        points.push(ProbePoint {
            input_db,
            gain_db: r1(db(a1) - db(amp)),
            compression_db: 0.0,
            thd_pct: r1(thd * 100.0),
            even_minus_odd_db: (thd >= 0.003).then(|| {
                let floor = (a1 * 1e-4).powi(2);
                r1((10.0 * (even.max(floor) / odd.max(floor)).log10()).clamp(-60.0, 60.0))
            }),
            imd_pct: r1(imd * 100.0),
        });
    }
    let g0 = points.first().map_or(0.0, |p| p.gain_db);
    for p in &mut points {
        p.compression_db = r1(g0 - p.gain_db);
    }
    let mut notes = Vec::new();
    if chain.is_empty() {
        notes.push("内蔵のエフェクトが無い(または全部外してある)ので、試験の音は素通り".to_owned());
    } else {
        if let Some(p) = points.iter().find(|p| p.thd_pct >= 3.0) {
            notes.push(format!(
                "入力 {:.0} dBFS から歪みが 3 % を超える(そこより大きい音が歪む)",
                p.input_db
            ));
        }
        if let Some(p) = points.iter().find(|p| p.compression_db >= 3.0) {
            notes.push(format!(
                "入力 {:.0} dBFS で 3 dB 以上押さえ込まれる(それより大きい音は大きくならない = 潰れ)",
                p.input_db
            ));
        }
        if let Some(p) = points.iter().max_by(|a, b| a.imd_pct.total_cmp(&b.imd_pct)) {
            if p.imd_pct >= 5.0 {
                notes.push(format!(
                    "2 音の相互変調が最大 {:.1} %(入力 {:.0} dBFS): 和音・ベースの重なりが濁る。和音のパートには掛けすぎない",
                    p.imd_pct, p.input_db
                ));
            }
        }
        if let Some(eo) = points
            .iter()
            .rfind(|p| p.thd_pct >= 1.0)
            .and_then(|p| p.even_minus_odd_db)
        {
            notes.push(if eo >= 0.0 {
                "偶数倍音が多い(温かい・太い歪み)".to_owned()
            } else {
                "奇数倍音が多い(硬い・ざらついた歪み)".to_owned()
            });
        }
        if notes.is_empty() {
            notes.push("どの大きさでも歪み・押さえ込みは小さい".to_owned());
        }
    }
    FxProbe {
        chain: chain.into_iter().map(|(n, _)| n).collect(),
        tone_hz: r1(f0),
        points,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 歪みのエフェクトを通すと、大きい音ほど歪み率と押さえ込みが増え、相互変調も出る
    #[test]
    fn probe_sees_distortion_grow_with_level() {
        let mut t =
            glaux_core::Track::new(glaux_core::TrackId::new(), "t", glaux_core::TrackKind::Midi);
        let mut e = glaux_core::Effect::builtin(glaux_core::FxId::new(), "distortion");
        e.params
            .insert("drive".into(), glaux_core::ParamValue::Float(0.8));
        t.effects.push(e);
        let p = fx_probe(&t);
        assert_eq!(p.chain, vec!["distortion"]);
        let lo = &p.points[0];
        let hi = p.points.last().unwrap();
        assert!(hi.thd_pct > lo.thd_pct + 1.0, "{p:?}");
        assert!(hi.imd_pct > 1.0, "{p:?}");
        // 何も無ければ素通り
        let q = fx_probe(&glaux_core::Track::new(
            glaux_core::TrackId::new(),
            "u",
            glaux_core::TrackKind::Midi,
        ));
        assert!(
            q.points
                .iter()
                .all(|x| x.thd_pct < 0.1 && x.compression_db.abs() < 0.1),
            "{q:?}"
        );
    }
}
