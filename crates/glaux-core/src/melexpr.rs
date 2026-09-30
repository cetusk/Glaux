//! 旋律の表情(強さ・切り方・ビブラート・グライド・わずかなタイミング)と、その一様さの測定。
//!
//! 計画から作った旋律は、強さが全部同じ・全部が格子ちょうど・切り方が 2 通り・表情が無し、で機械的に聞こえた
//! (Testv2_rev3)。調査の結論「表情はすべての音に同じにしない」(docs の practice)に沿って、句の中の位置と
//! 骨格かどうかで音ごとに変える。どれも seed で決まる。
//!
//! - 強さ: 句の弧(句の山へ強く、終わりで抜く)、高い音ほど少し強く(Friberg らの規則の high-loud)、拍の位置、
//!   食った音のアクセント、骨格の音、同じ音の連打は少しずつ弱く、ごく小さな揺れ
//! - 切り方: 順次進行はレガートでつなぐ(奏法 legato で弾き直さない)、連打と跳躍の前は短く、1 拍以上の音と
//!   句の終わりは伸ばし切る
//! - ビブラート: 1 拍以上の音だけ、音の長さに合わせて遅らせて掛ける(短い音には掛けない)
//! - グライド: 句の山・長い音への 5 半音以上の上向きの跳躍だけ
//! - タイミング: 裏拍の音だけ、ノリ(tight / laid_back / push)でわずかにずらす

use crate::melody::MelNote;
use crate::model::{Articulation, Note, Vibrato};
use crate::time::PPQ;
use serde::{Deserialize, Serialize};

/// 表情の指定(計画の `expression`)
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Expression {
    /// 量 0〜1(0 で表情無し = 以前と同じ一律)
    #[serde(default = "default_amount")]
    pub amount: f64,
    /// ノリ: tight(格子どおり)/ laid_back(裏拍を少し遅らせる)/ push(裏拍を少し早める)
    #[serde(default = "default_feel")]
    pub feel: String,
    /// 伸ばす音のビブラート
    #[serde(default = "yes")]
    pub vibrato: bool,
    /// 山への跳躍のグライド
    #[serde(default = "yes")]
    pub glide: bool,
    /// 強さの中心(既定 90)
    #[serde(default = "default_vel")]
    pub velocity: u8,
}

fn default_amount() -> f64 {
    0.6
}
fn default_feel() -> String {
    "tight".to_owned()
}
fn yes() -> bool {
    true
}
fn default_vel() -> u8 {
    90
}

impl Default for Expression {
    fn default() -> Self {
        Expression {
            amount: default_amount(),
            feel: default_feel(),
            vibrato: true,
            glide: true,
            velocity: default_vel(),
        }
    }
}

pub const FEELS: &[&str] = &["tight", "laid_back", "push"];

/// 表情を付けた 1 音
#[derive(Clone, PartialEq, Debug)]
pub struct ExprNote {
    pub pos: u64,
    pub dur: u64,
    pub pitch: u8,
    pub vel: u8,
    pub articulation: Articulation,
    pub glide_ms: Option<f32>,
    pub vibrato: Option<Vibrato>,
}

fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// -1〜1 の決まった揺れ(音の位置と seed から)
fn jitter(seed: u64, pos: u64) -> f64 {
    (mix(seed ^ pos.wrapping_mul(0x2545_F491_4F6C_DD1D)) >> 11) as f64 / (1u64 << 52) as f64 - 1.0
}

/// 表情を付ける。`phrases` は句の (最初の音の頭, 終わり) の列、`anchors` は骨格の音の位置(tick)、
/// `bar_of(t)` はその小節の (頭, 長さ)、`bpm` はミリ秒の計算に使う
pub fn express(
    notes: &[MelNote],
    phrases: &[(u64, u64)],
    anchors: &[u64],
    bar_of: &dyn Fn(u64) -> (u64, u64),
    bpm: f64,
    e: &Expression,
    seed: u64,
) -> Vec<ExprNote> {
    let amt = e.amount.clamp(0.0, 1.0);
    let ms_per_tick = 60_000.0 / bpm.max(20.0) / PPQ as f64;
    let mut out: Vec<ExprNote> = notes
        .iter()
        .map(|n| ExprNote {
            pos: n.pos,
            dur: n.dur,
            pitch: n.pitch,
            vel: e.velocity,
            articulation: Articulation::Normal,
            glide_ms: None,
            vibrato: None,
        })
        .collect();
    if amt <= 0.0 || notes.is_empty() {
        return out;
    }
    let phrase_of = |t: u64| {
        phrases
            .iter()
            .position(|&(a, b)| a <= t && t < b.max(a + 1))
    };
    for (i, n) in notes.iter().enumerate() {
        let prev = i.checked_sub(1).map(|j| notes[j]);
        let next = notes.get(i + 1).copied();
        let gap = next.map_or(n.dur, |x| x.pos.saturating_sub(n.pos)).max(1);
        let ph = phrase_of(n.pos);
        let same_phrase_next = next.is_some_and(|x| phrase_of(x.pos) == ph && ph.is_some());
        let same_phrase_prev = prev.is_some_and(|x| phrase_of(x.pos) == ph && ph.is_some());
        let (bs, bl) = bar_of(n.pos);
        let rel = n.pos.saturating_sub(bs);
        let half = (bl / 2).max(1);
        // ---- 強さ
        let mut v = e.velocity as f64;
        if let Some(k) = ph {
            let (a, b) = phrases[k];
            let inside: Vec<&MelNote> = notes.iter().filter(|x| a <= x.pos && x.pos < b).collect();
            let top = inside.iter().map(|x| x.pitch).max().unwrap_or(n.pitch);
            let mean =
                inside.iter().map(|x| x.pitch as f64).sum::<f64>() / inside.len().max(1) as f64;
            let x = (n.pos - a) as f64 / (b - a).max(1) as f64;
            // 句の弧: 半ばから山へ強く、終わりで抜く
            v += 6.0 * (std::f64::consts::PI * x.min(1.0)).sin() - 3.0;
            // 高い音ほど少し強く(句の平均から)
            v += ((n.pitch as f64 - mean) * 0.8).clamp(-6.0, 6.0);
            if n.pitch == top {
                v += 4.0;
            }
            if !same_phrase_next {
                // 句の終わりの音は抜く(伸ばした音が大きく残らない)
                v -= 5.0;
            }
        }
        if rel % bl == 0 {
            v += 4.0;
        } else if rel % half == 0 {
            v += 2.0;
        } else if rel % (PPQ / 2) != 0 {
            v -= 4.0;
        }
        // 裏から入って伸ばす音(食い)はアクセント
        if rel % PPQ != 0 && n.dur >= PPQ {
            v += 5.0;
        }
        if anchors.contains(&n.pos) {
            v += 3.0;
        }
        // 同じ音の連打は少しずつ弱く
        if prev.is_some_and(|p| p.pitch == n.pitch) && same_phrase_prev {
            v -= 3.0;
        }
        v += 3.0 * jitter(seed, n.pos);
        // 量 0.6 で上の幅そのまま。量に比例させる(最大 1.6 倍)
        let base = e.velocity as f64;
        let v = base + (v - base) * (amt / 0.6).min(1.6);
        out[i].vel = v.round().clamp(30.0, 127.0) as u8;
        // ---- 切り方
        let step = next.map(|x| (x.pitch as i32 - n.pitch as i32).abs());
        let dur = if !same_phrase_next || n.dur >= PPQ {
            // 伸ばし切る(句の終わり・1 拍以上)。句の終わりは元の長さのまま(息継ぎを保つ)
            if same_phrase_next {
                gap.saturating_sub(24).max(n.dur)
            } else {
                n.dur
            }
        } else {
            match step {
                Some(0) => gap / 2,               // 連打は短く、弾き直しを聞かせる
                Some(1..=2) => gap,               // 順次進行はつなぐ(次の音を legato に)
                Some(s) if s >= 5 => gap * 3 / 4, // 跳躍の前は少し切る
                _ => (gap as f64 * (0.82 + 0.1 * jitter(seed ^ 7, n.pos))) as u64,
            }
        };
        out[i].dur = dur.clamp(PPQ / 8, gap.max(PPQ / 8));
        if same_phrase_prev {
            if let Some(p) = prev {
                let d = (n.pitch as i32 - p.pitch as i32).abs();
                if (1..=2).contains(&d) && p.dur < PPQ && n.pos.saturating_sub(p.pos) <= PPQ {
                    out[i].articulation = Articulation::Legato;
                }
                // 山・長い音への上向きの跳躍はグライド
                let up = n.pitch as i32 - p.pitch as i32;
                let is_top = ph.is_some_and(|k| {
                    let (a, b) = phrases[k];
                    notes
                        .iter()
                        .filter(|x| a <= x.pos && x.pos < b)
                        .all(|x| x.pitch <= n.pitch)
                });
                if e.glide && up >= 5 && (is_top || n.dur >= PPQ) {
                    out[i].articulation = Articulation::Portamento;
                    out[i].glide_ms = Some((40.0 + 50.0 * amt) as f32);
                }
            }
        }
        // ---- ビブラート(1 拍以上の音だけ、長さに合わせて遅らせる)
        let len_ms = n.dur as f64 * ms_per_tick;
        if e.vibrato && n.dur >= PPQ && len_ms >= 300.0 {
            let long = (n.dur as f64 / (2 * PPQ) as f64).min(1.5);
            out[i].vibrato = Some(Vibrato {
                rate_hz: (5.2 + 0.4 * jitter(seed ^ 11, n.pos)) as f32,
                depth_cents: ((18.0 + 22.0 * long) * amt / 0.6).clamp(8.0, 60.0) as f32,
                delay_ms: (len_ms * 0.35).min(450.0) as f32,
                fade_in_ms: (len_ms * 0.3).min(400.0) as f32,
                fade_out_ms: 60.0,
                rate_end_hz: None,
            });
        }
        // ---- タイミング(裏拍だけ)
        if rel % PPQ != 0 {
            let shift = match e.feel.as_str() {
                "laid_back" => 14.0,
                "push" => -10.0,
                _ => 0.0,
            } * amt
                + 4.0 * amt * jitter(seed ^ 3, n.pos);
            let p = (n.pos as f64 + shift).round().max(0.0) as u64;
            out[i].pos = p;
        }
    }
    // ずらした後で重ならないよう、長さを次の音までに収める(レガートの重なりは奏法に任せる)
    for i in 0..out.len().saturating_sub(1) {
        let room = out[i + 1].pos.saturating_sub(out[i].pos).max(1);
        if out[i].dur > room {
            out[i].dur = room;
        }
    }
    out
}

/// 表情の一様さ(機械的に聞こえる兆候)の測定
#[derive(Clone, Debug, Default, Serialize)]
pub struct Uniformity {
    pub notes: usize,
    /// 強さの標準偏差と種類の数
    pub velocity_sd: f64,
    pub velocity_kinds: usize,
    /// 長さ ÷ 次の音までの比(0.05 刻み)の種類の数と、最も多い比の割合
    pub cut_kinds: usize,
    pub cut_top_share: f64,
    /// 16 分の格子ちょうどに頭がある割合
    pub on_grid: f64,
    /// ビブラート・グライド・ベンド・奏法・音量の曲線のどれかがある音の割合
    pub expressive: f64,
    /// 1 拍以上の音のうちビブラートがある割合と、1 拍未満の音のうちビブラートがある割合
    pub vibrato_long: f64,
    pub vibrato_short: f64,
}

/// ノートの表情の一様さを測る(`bar_start` はクリップの頭の tick。格子の判定に使う)
pub fn uniformity(notes: &[Note], clip_start: u64) -> Uniformity {
    let n = notes.len();
    if n == 0 {
        return Uniformity::default();
    }
    let mut ns: Vec<&Note> = notes.iter().collect();
    ns.sort_by_key(|x| x.pos);
    let vels: Vec<f64> = ns.iter().map(|x| x.vel as f64).collect();
    let mean = vels.iter().sum::<f64>() / n as f64;
    let sd = (vels.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64).sqrt();
    let mut vk: Vec<u8> = ns.iter().map(|x| x.vel).collect();
    vk.sort_unstable();
    vk.dedup();
    let mut cuts: std::collections::BTreeMap<u32, usize> = Default::default();
    for w in ns.windows(2) {
        let gap = w[1].pos.0.saturating_sub(w[0].pos.0);
        if gap > 0 {
            let r = ((w[0].dur.0 as f64 / gap as f64) / 0.05).round() as u32;
            *cuts.entry(r).or_default() += 1;
        }
    }
    let total_cuts: usize = cuts.values().sum();
    let expressive = ns
        .iter()
        .filter(|x| {
            x.vibrato.is_some()
                || x.glide_ms.is_some()
                || !x.pitch_curve.is_empty()
                || !x.volume_curve.is_empty()
                || x.articulation != Articulation::Normal
        })
        .count();
    let long: Vec<&&Note> = ns.iter().filter(|x| x.dur.0 >= PPQ).collect();
    let short: Vec<&&Note> = ns.iter().filter(|x| x.dur.0 < PPQ).collect();
    let vib = |v: &[&&Note]| {
        v.iter()
            .filter(|x| x.vibrato.is_some() || x.articulation == Articulation::Vibrato)
            .count() as f64
            / v.len().max(1) as f64
    };
    Uniformity {
        notes: n,
        velocity_sd: (sd * 10.0).round() / 10.0,
        velocity_kinds: vk.len(),
        cut_kinds: cuts.len(),
        cut_top_share: ((cuts.values().max().copied().unwrap_or(0) as f64
            / total_cuts.max(1) as f64)
            * 100.0)
            .round()
            / 100.0,
        on_grid: ((ns
            .iter()
            .filter(|x| (clip_start + x.pos.0) % 240 == 0)
            .count() as f64
            / n as f64)
            * 100.0)
            .round()
            / 100.0,
        expressive: ((expressive as f64 / n as f64) * 100.0).round() / 100.0,
        vibrato_long: (vib(&long) * 100.0).round() / 100.0,
        vibrato_short: (vib(&short) * 100.0).round() / 100.0,
    }
}

/// 一様さの指摘(analyze_melody の [表情])
pub fn uniformity_findings(u: &Uniformity) -> Vec<(&'static str, String, &'static str)> {
    let mut v = Vec::new();
    if u.notes < 8 {
        return v;
    }
    if u.velocity_sd < 2.0 {
        v.push((
            "warn",
            format!("強さが一様(標準偏差 {:.1}、{} 種類)。機械的に聞こえる", u.velocity_sd, u.velocity_kinds),
            "句の山へ強く・終わりで抜く、拍の位置、食った音のアクセントで強さを変える(realize_melody の expression)",
        ));
    }
    if u.cut_top_share >= 0.7 {
        v.push((
            "warn",
            format!(
                "音の切り方がほぼ同じ({:.0}% が同じ比)",
                u.cut_top_share * 100.0
            ),
            "順次進行はつなぎ、連打・跳躍の前は短く、長い音と句の終わりは伸ばし切る",
        ));
    }
    if u.expressive < 0.05 {
        v.push((
            "info",
            "ビブラート・グライドなどの表情がほとんど無い".to_owned(),
            "1 拍以上の音にだけ遅らせたビブラート、山への跳躍にグライド",
        ));
    }
    if u.vibrato_short > 0.3 {
        v.push((
            "warn",
            format!(
                "短い音にもビブラートが掛かっている({:.0}%)",
                u.vibrato_short * 100.0
            ),
            "ビブラートは 1 拍以上の音だけ",
        ));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar_of(t: u64) -> (u64, u64) {
        (t / 3840 * 3840, 3840)
    }

    fn line() -> Vec<MelNote> {
        // 2 小節の句 2 つ: 8 分の順次進行、裏から入る長い音、跳躍して山、連打、句の終わり
        let mut v = vec![];
        let mut t = 0;
        for (d, p) in [
            (480, 69),
            (480, 71),
            (480, 72),
            (1440, 74),
            (480, 72),
            (480, 72),
            (480, 71),
            (3360, 69),
            (480, 64),
            (480, 69),
            (960, 76),
            (480, 74),
            (480, 72),
            (480, 72),
            (3360, 69),
        ] {
            v.push(MelNote {
                pos: t,
                dur: d,
                pitch: p,
            });
            t += d;
        }
        v
    }

    fn to_notes(e: &[ExprNote]) -> Vec<Note> {
        e.iter()
            .map(|x| Note {
                id: crate::NoteId::new(),
                pos: crate::time::Tick(x.pos),
                dur: crate::time::Tick(x.dur),
                pitch: x.pitch,
                vel: x.vel,
                articulation: x.articulation,
                pitch_curve: vec![],
                glide_ms: x.glide_ms,
                vibrato: x.vibrato,
                volume_curve: vec![],
                brightness_curve: vec![],
                condition: None,
            })
            .collect()
    }

    #[test]
    fn expression_varies_by_position_and_is_measured() {
        let notes = line();
        let phrases = [(0, 7680), (7680, 15360)];
        let flat = express(
            &notes,
            &phrases,
            &[0, 1440],
            &bar_of,
            124.0,
            &Expression {
                amount: 0.0,
                ..Default::default()
            },
            1,
        );
        let u0 = uniformity(&to_notes(&flat), 0);
        assert_eq!(u0.velocity_kinds, 1);
        assert!(!uniformity_findings(&u0).is_empty());
        let e = express(
            &notes,
            &phrases,
            &[0, 1440],
            &bar_of,
            124.0,
            &Expression::default(),
            1,
        );
        let u = uniformity(&to_notes(&e), 0);
        assert!(u.velocity_sd >= 3.0, "{u:?}");
        assert!(u.cut_top_share < 0.7, "{u:?}");
        assert_eq!(u.vibrato_short, 0.0, "{u:?}");
        assert!(u.vibrato_long > 0.5, "{u:?}");
        // 句の終わりの長い音は抜く(句の山より弱い)
        let top = e
            .iter()
            .filter(|x| x.pos < 7680)
            .max_by_key(|x| x.pitch)
            .unwrap();
        let end = e.iter().rfind(|x| x.pos < 7680).unwrap();
        assert!(end.vel < top.vel, "{} {}", end.vel, top.vel);
        // 順次進行はレガート、64 → 69 → 76 の跳躍で山へはグライド
        assert!(e.iter().any(|x| x.articulation == Articulation::Legato));
        assert!(e
            .iter()
            .any(|x| x.pitch == 76 && x.articulation == Articulation::Portamento));
        // 句の終わりの息継ぎ(句の終わりの音の長さ)は保つ
        assert_eq!(end.dur, 3360);
        assert!(
            uniformity_findings(&u).iter().all(|f| f.0 != "warn"),
            "{:?}",
            uniformity_findings(&u)
        );
    }

    #[test]
    fn laid_back_delays_only_offbeats() {
        let notes = line();
        let phrases = [(0, 7680), (7680, 15360)];
        let e = express(
            &notes,
            &phrases,
            &[],
            &bar_of,
            124.0,
            &Expression {
                feel: "laid_back".into(),
                amount: 1.0,
                ..Default::default()
            },
            2,
        );
        for (a, b) in notes.iter().zip(&e) {
            if a.pos % PPQ == 0 {
                assert_eq!(a.pos, b.pos);
            } else {
                assert!(b.pos > a.pos, "{} {}", a.pos, b.pos);
            }
        }
    }
}
