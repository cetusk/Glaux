//! 電子音楽の役割の見分けと、役割ごとのパートの組み立て(曲から起こす機能)。
//!
//! 混ざったシンセの音は楽器の名前では見分けられない(調査文書 13.17 節)ので、役割で捉える:
//! - 伸ばす(パッド・ストリングス・オルガン・ドローン): ある高さの音が 1.5 拍以上続く
//! - 刻む(スタブ・ピアノの刻み): 短い和音(同時に立ち上がる 2 音以上)
//! - 線(アルペジオ・旋律・ベル): 単音の短い音
//!
//! 小節ごとに、どの役割が鳴っているかを決め、鳴っている小節にだけ役割ごとの音符を置く。音符は、伸ばす = 推定した和音を広く積んで伸ばす、
//! 刻む = 推定した和音を聞こえた刻みの位置で鳴らす、線 = 起こした単音をその時の和音の音へ寄せる。

use crate::recon::{RawNote, Skeleton};
use serde::Serialize;

/// 役割
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Sustain,
    Stab,
    Line,
}

/// 小節 1 つの役割の手がかりと判定
#[derive(Clone, Debug, Serialize)]
pub struct BarRoles {
    pub start: f64,
    pub end: f64,
    /// 伸ばす音が鳴っている時間の割合(0〜1)
    pub sustain_ratio: f64,
    /// 短い和音の数
    pub stabs: usize,
    /// 単音の短い音の数
    pub line_notes: usize,
    pub sustain: bool,
    pub stab: bool,
    pub line: bool,
}

impl BarRoles {
    pub fn has(&self, r: Role) -> bool {
        match r {
            Role::Sustain => self.sustain,
            Role::Stab => self.stab,
            Role::Line => self.line,
        }
    }
}

/// 判定のしきい値(小節あたり)
#[derive(Clone, Copy, Debug)]
pub struct RoleOptions {
    pub sustain_ratio: f64,
    pub stabs: usize,
    pub line_notes: usize,
}

impl Default for RoleOptions {
    fn default() -> Self {
        RoleOptions {
            sustain_ratio: 0.25,
            stabs: 4,
            line_notes: 4,
        }
    }
}

/// 小節の境目(秒): 骨組みの小節頭。最初の小節頭の前と、最後の小節頭の後(曲の終わりまで)も小節として足す
pub fn bar_bounds(sk: &Skeleton) -> Vec<(f64, f64)> {
    let mut d = sk.downbeats.clone();
    if d.is_empty() {
        return vec![(0.0, sk.duration_sec)];
    }
    let bar = d
        .windows(2)
        .map(|w| w[1] - w[0])
        .fold(0.0f64, f64::max)
        .max(0.5);
    while d[0] > 0.05 {
        d.insert(0, (d[0] - bar).max(0.0));
    }
    while d[d.len() - 1] < sk.duration_sec - 0.05 {
        let last = d[d.len() - 1];
        d.push((last + bar).min(sk.duration_sec));
    }
    d.windows(2)
        .map(|w| (w[0], w[1]))
        .filter(|(a, b)| b - a > 0.05)
        .collect()
}

/// 小節ごとの役割を当てる。`audio` は分けた「その他」か曲全体、`notes` はそれを basic-pitch で起こした音符
pub fn detect(
    audio: &[f32],
    sr: f32,
    sk: &Skeleton,
    notes: &[RawNote],
    opts: &RoleOptions,
) -> Vec<BarRoles> {
    let beat = 60.0 / sk.bpm.unwrap_or(120.0);
    let pe = glaux_engine::tonal::pitch_energy(audio, sr, sk.tuning_cents);
    // 伸ばす音: C3〜C7 のある高さの音が、曲全体の最大から 30dB 以内・その小節の最大から 20dB 以内で 1.5 拍以上続く
    let global_top = pe
        .db
        .iter()
        .flat_map(|r| r[48..96].iter().copied())
        .fold(f32::MIN, f32::max);
    let need = (1.5 * beat / pe.hop).ceil() as usize;
    let mut on = vec![false; pe.db.len()];
    if !pe.db.is_empty() {
        // 小節より長く続く音も拾えるよう、曲全体で続きを数える(下限は小節の外の大きさに左右されないよう、曲全体の最大から決める)
        for p in 48..96 {
            let mut run = 0usize;
            for k in 0..pe.db.len() {
                if pe.db[k][p] >= global_top - 30.0 {
                    run += 1;
                    if run >= need {
                        for x in on.iter_mut().take(k + 1).skip(k + 1 - run) {
                            *x = true;
                        }
                    }
                } else {
                    run = 0;
                }
            }
        }
    }
    // その時の和音の音と、調の音階
    bar_bounds(sk)
        .into_iter()
        .map(|(a, b)| {
            let (i0, i1) = (pe.frame(a), pe.frame(b));
            let sustain_ratio = if i1 > i0 {
                on[i0..i1].iter().filter(|x| **x).count() as f64 / (i1 - i0) as f64
            } else {
                0.0
            };
            let inw: Vec<&RawNote> = notes
                .iter()
                .filter(|n| n.start >= a && n.start < b)
                .collect();
            let short: Vec<&&RawNote> = inw
                .iter()
                .filter(|n| n.end - n.start < 1.5 * beat)
                .collect();
            let in_chord = |n: &RawNote| {
                inw.iter()
                    .any(|m| !std::ptr::eq(*m, n) && (m.start - n.start).abs() <= 0.03)
            };
            let mut t: Vec<f64> = short
                .iter()
                .filter(|n| in_chord(n))
                .map(|n| n.start)
                .collect();
            t.sort_by(f64::total_cmp);
            t.dedup_by(|x, y| (*x - *y).abs() <= 0.03);
            let stabs = t.len();
            let line_notes = short.iter().filter(|n| !in_chord(n)).count();
            BarRoles {
                start: a,
                end: b,
                sustain_ratio,
                stabs,
                line_notes,
                sustain: sustain_ratio >= opts.sustain_ratio,
                stab: stabs >= opts.stabs,
                line: line_notes >= opts.line_notes,
            }
        })
        .collect()
}

/// 役割ごとの音符を組み立てる(鳴っている小節だけ)。`notes` は分けた「その他」か曲全体から起こした音符(刻みの位置・線の音に使う)
pub fn role_notes(role: Role, sk: &Skeleton, bars: &[BarRoles], notes: &[RawNote]) -> Vec<RawNote> {
    let active = |t: f64| {
        bars.iter()
            .any(|b| b.has(role) && t >= b.start && t < b.end)
    };
    let beat = 60.0 / sk.bpm.unwrap_or(120.0);
    match role {
        Role::Sustain => {
            // 推定した和音を積んで伸ばす(鳴っている小節だけ。小節の外へはみ出す所は切る)
            let mut out = Vec::new();
            for n in crate::recon::chord_part_notes(sk) {
                for b in bars.iter().filter(|b| b.sustain) {
                    let (s, e) = (n.start.max(b.start), n.end.min(b.end));
                    if e - s > 0.05 {
                        out.push(RawNote {
                            start: s,
                            end: e,
                            ..n
                        });
                    }
                }
            }
            merge_ties(out)
        }
        Role::Stab => {
            // 聞こえた短い和音の位置で、その時の和音を短く鳴らす
            let chords = crate::recon::chord_part_notes(sk);
            let mut t: Vec<f64> = notes
                .iter()
                .filter(|n| n.end - n.start < 1.5 * beat)
                .filter(|n| {
                    notes
                        .iter()
                        .any(|m| !std::ptr::eq(m, *n) && (m.start - n.start).abs() <= 0.03)
                })
                .map(|n| n.start)
                .filter(|t| active(*t))
                .collect();
            t.sort_by(f64::total_cmp);
            t.dedup_by(|x, y| (*x - *y).abs() <= 0.05);
            let len = (beat * 0.5).min(0.3);
            t.into_iter()
                .flat_map(|s| {
                    chords
                        .iter()
                        .filter(move |c| c.start <= s + 0.02 && s < c.end)
                        .map(move |c| RawNote {
                            start: s,
                            end: s + len,
                            pitch: c.pitch,
                            vel: 100,
                        })
                        .collect::<Vec<_>>()
                })
                .collect()
        }
        Role::Line => {
            // 単音の短い音を、その時の和音の音(無ければ調の音階)へ寄せる
            let chords: Vec<(f64, f64, Vec<u8>)> = sk
                .chords
                .iter()
                .filter_map(|c| {
                    let pcs = glaux_core::harmony::chord_pitch_classes(&c.symbol)?;
                    Some((c.start, c.end, pcs))
                })
                .collect();
            let scale: Option<Vec<u8>> = sk.key.as_ref().map(|k| {
                glaux_core::harmony::scale_pitch_classes(
                    k.tonic,
                    if k.minor { "minor" } else { "major" },
                )
            });
            let mut out: Vec<RawNote> = notes
                .iter()
                .filter(|n| n.end - n.start < 1.5 * beat && n.pitch >= 55 && active(n.start))
                .map(|n| {
                    let pcs = chords
                        .iter()
                        .find(|c| c.0 <= n.start && n.start < c.1)
                        .map(|c| c.2.clone())
                        .or_else(|| scale.clone());
                    let pitch = match pcs {
                        Some(pcs) if !pcs.is_empty() => {
                            glaux_core::harmony::snap_to_pitch_classes(n.pitch as i32, &pcs)
                                .clamp(0, 127) as u8
                        }
                        _ => n.pitch,
                    };
                    RawNote { pitch, ..*n }
                })
                .collect();
            // 線は単音: 同時に立ち上がる音はいちばん上だけ
            out.sort_by(|a, b| a.start.total_cmp(&b.start).then(b.pitch.cmp(&a.pitch)));
            out.dedup_by(|x, y| (x.start - y.start).abs() <= 0.03);
            out
        }
    }
}

/// 同じ高さで隙間なく続く音符をつなぐ(小節で切った伸ばす音を 1 つに戻す)
fn merge_ties(mut v: Vec<RawNote>) -> Vec<RawNote> {
    v.sort_by(|a, b| a.pitch.cmp(&b.pitch).then(a.start.total_cmp(&b.start)));
    let mut out: Vec<RawNote> = Vec::with_capacity(v.len());
    for n in v {
        if let Some(last) = out.last_mut() {
            if last.pitch == n.pitch && (n.start - last.end).abs() < 0.02 {
                last.end = last.end.max(n.end);
                continue;
            }
        }
        out.push(n);
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

/// 小節のうち、この数より少なくしか鳴っていない役割は「曲に無い」とみなし、トラックを作らない
pub const MIN_BARS: usize = 2;

/// 役割のトラックの既定の音色(内蔵のシンセ)。曲の音色に合わせるのは別に行う
pub fn role_device(role: Role) -> glaux_core::Device {
    use glaux_core::{Device, ParamValue as P};
    let mut d = Device::builtin("subtractive");
    let mut set = |k: &str, v: P| {
        d.params.insert(k.into(), v);
    };
    match role {
        Role::Sustain => {
            // 広がりのある鋸歯状波のパッド
            set("waveform", P::Enum("saw".into()));
            set("unison", P::Int(5));
            set("detune", P::Float(18.0));
            set("attack", P::Float(0.3));
            set("sustain", P::Float(0.9));
            set("release", P::Float(0.8));
            set("cutoff", P::Float(2400.0));
            set("filter_env", P::Float(0.1));
            set("gain_db", P::Float(-12.0));
        }
        Role::Stab => {
            // 短く切ったスタブ
            set("waveform", P::Enum("saw".into()));
            set("unison", P::Int(3));
            set("detune", P::Float(14.0));
            set("attack", P::Float(0.002));
            set("decay", P::Float(0.2));
            set("sustain", P::Float(0.0));
            set("release", P::Float(0.18));
            set("cutoff", P::Float(2600.0));
            set("filter_env", P::Float(0.5));
            set("gain_db", P::Float(-12.0));
        }
        Role::Line => {
            // 矩形波のプラック(アルペジオ・旋律)
            set("waveform", P::Enum("square".into()));
            set("unison", P::Int(2));
            set("attack", P::Float(0.002));
            set("decay", P::Float(0.16));
            set("sustain", P::Float(0.1));
            set("release", P::Float(0.15));
            set("cutoff", P::Float(3200.0));
            set("filter_env", P::Float(0.4));
            set("gain_db", P::Float(-12.0));
        }
    }
    d
}

/// 役割ごとに、曲の音(分けた「その他」か曲全体)から起こした音符から、役割の音符を組み立てる。
/// 返り値は、役割が鳴っている小節の数と音符(鳴っている小節が MIN_BARS 未満なら音符は空)
pub fn transcribe_role(
    role: Role,
    audio: &[f32],
    sr: f32,
    sk: &Skeleton,
    from_mix: bool,
) -> Result<(usize, Vec<RawNote>), String> {
    let mut est = crate::recon::transcribe_part(
        audio,
        sr,
        crate::recon::PartKind::Poly,
        crate::recon::NoteMethod::BasicPitch,
        sk.tuning_cents,
    )?;
    if from_mix {
        // 曲全体から起こしたときは、ベースの音域(C3 未満)を除く
        est.retain(|n| n.pitch >= 48);
    }
    let bars = detect(audio, sr, sk, &est, &RoleOptions::default());
    let n = bars.iter().filter(|b| b.has(role)).count();
    if n < MIN_BARS {
        return Ok((n, Vec::new()));
    }
    Ok((n, role_notes(role, sk, &bars, &est)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_engine::mir_eval::ChordSpan;

    fn skeleton() -> Skeleton {
        // 120 BPM・4/4、2 秒ごとの小節が 4 つ。C が 2 小節、Am が 2 小節
        let span = |a: f64, b: f64, s: &str| ChordSpan {
            start: a,
            end: b,
            symbol: s.into(),
        };
        Skeleton {
            duration_sec: 8.0,
            beats: (0..16).map(|i| i as f64 * 0.5).collect(),
            downbeats: vec![0.0, 2.0, 4.0, 6.0],
            bpm: Some(120.0),
            tempo_map: vec![],
            beats_per_bar: Some(4),
            tuning_cents: 0.0,
            key: None,
            chords: vec![span(0.0, 4.0, "C"), span(4.0, 8.0, "Am")],
            groove: None,
        }
    }

    fn bars(sustain: [bool; 4]) -> Vec<BarRoles> {
        (0..4)
            .map(|i| BarRoles {
                start: i as f64 * 2.0,
                end: i as f64 * 2.0 + 2.0,
                sustain_ratio: 0.0,
                stabs: 0,
                line_notes: 0,
                sustain: sustain[i],
                stab: false,
                line: false,
            })
            .collect()
    }

    #[test]
    fn sustain_part_only_sounds_where_the_role_was_heard() {
        let sk = skeleton();
        // 2 小節目と 3 小節目だけ伸ばす音がある
        let notes = role_notes(Role::Sustain, &sk, &bars([false, true, true, false]), &[]);
        assert!(!notes.is_empty());
        assert!(
            notes
                .iter()
                .all(|n| n.start >= 2.0 - 1e-9 && n.end <= 6.0 + 1e-9),
            "{notes:?}"
        );
        // 和音どおり: 2 小節目は C、3 小節目は Am の音
        let pcs = |t: f64| -> Vec<u8> {
            let mut v: Vec<u8> = notes
                .iter()
                .filter(|n| n.start <= t && t < n.end)
                .map(|n| n.pitch % 12)
                .collect();
            v.sort();
            v.dedup();
            v
        };
        assert_eq!(pcs(3.0), vec![0, 4, 7]);
        assert_eq!(pcs(5.0), vec![0, 4, 9]);
    }

    #[test]
    fn line_notes_are_pulled_to_the_chord() {
        let sk = skeleton();
        let mut b = bars([false; 4]);
        b[0].line = true;
        // C の小節で、和音に無い D(62)と F#(66)を弾いた
        let heard = vec![
            RawNote {
                start: 0.5,
                end: 0.7,
                pitch: 62,
                vel: 90,
            },
            RawNote {
                start: 1.0,
                end: 1.2,
                pitch: 66,
                vel: 90,
            },
            // 鳴っていない小節の音は使わない
            RawNote {
                start: 2.5,
                end: 2.7,
                pitch: 64,
                vel: 90,
            },
        ];
        let notes = role_notes(Role::Line, &sk, &b, &heard);
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(
            notes.iter().all(|n| [0, 4, 7].contains(&(n.pitch % 12))),
            "{notes:?}"
        );
    }

    #[test]
    fn each_role_has_its_own_builtin_sound() {
        for r in [Role::Sustain, Role::Stab, Role::Line] {
            let d = role_device(r);
            assert!(d.params.contains_key("attack"));
        }
        let attack = |r| match role_device(r).params.get("attack") {
            Some(glaux_core::ParamValue::Float(v)) => *v,
            _ => 0.0,
        };
        // 伸ばす音はゆっくり立ち上がり、刻む音はすぐ鳴る
        assert!(attack(Role::Sustain) > attack(Role::Stab));
    }
}
