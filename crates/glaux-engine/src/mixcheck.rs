//! 音と混ぜ具合の点検のための測定(MCP の critique_mix の材料)。
//!
//! トラックごとのソロの描き出し([`crate::analyze::render_tracks_solo`])から:
//! - **低域の左右の広がり**: 150Hz より下の左右の差 ÷ 中央。低域が広がっているとモノで減り、クラブで揺れる
//! - **音の頭と終わりのクリック**: 無音から一気に立ち上がる段差・鳴っている途中でいきなり無音になる所
//!   (アタック・リリースが短すぎる、サンプルにフェードが無い)
//! - **キックとベースの低域の重なり**: 40〜120Hz で両方が同時に強い時間の割合
//!
//! ここは測った値だけを返す。指摘の文言と直し方は呼び出し側(MCP)が組み立てる。

use crate::analyze::{Analysis, MaskingIssue};
use glaux_core::{Project, Tick, TrackId};

const SR: f64 = 48_000.0;

/// RBJ の 2 次フィルタ(Direct Form 1)
#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    x: [f64; 2],
    y: [f64; 2],
}

impl Biquad {
    fn new(kind: char, f: f64) -> Biquad {
        let w = std::f64::consts::TAU * f / SR;
        let (c, sn) = (w.cos(), w.sin());
        let alpha = sn / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let (b0, b1, b2) = if kind == 'l' {
            ((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0)
        } else {
            ((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0)
        };
        let a0 = 1.0 + alpha;
        Biquad {
            b: [b0 / a0, b1 / a0, b2 / a0],
            a: [-2.0 * c / a0, (1.0 - alpha) / a0],
            x: [0.0; 2],
            y: [0.0; 2],
        }
    }

    #[inline]
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[0] * self.y[0]
            - self.a[1] * self.y[1];
        self.x = [x, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }
}

/// 1 トラック分の測定
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct TrackCheck {
    pub track_id: String,
    pub name: String,
    pub peak_db: f64,
    /// スペクトルの重心(Hz。高いほど明るい・刺さりやすい)
    pub centroid_hz: f64,
    /// 150Hz より下の (左右の差 ÷ 中央)(dB)。低域が小さいトラックは None
    pub low_side_to_mid_db: Option<f64>,
    /// 150Hz より下のエネルギーがトラック全体に占める割合
    pub low_share: f64,
    /// ピークと RMS の差(dB)。歪み・コンプで潰すと小さくなる
    pub crest_db: f64,
    /// 無音から段差で立ち上がった所(秒。曲の頭から)
    pub abrupt_starts: Vec<f64>,
    /// 鳴っている途中でいきなり無音になった所(秒)
    pub abrupt_ends: Vec<f64>,
}

/// 点検の材料一式
#[derive(serde::Serialize)]
pub struct MixCheck {
    /// 調べた範囲(tick)。None なら曲全体
    pub range: Option<(u64, u64)>,
    pub mix: Analysis,
    pub tracks: Vec<TrackCheck>,
    pub masking: Vec<MaskingIssue>,
    /// キックとベースの低域(40〜120Hz)が同時に強い割合(キックが強い時間のうち)。どちらかが無ければ None
    pub kick_bass_overlap: Option<f64>,
}

/// 点検の範囲の既定: 曲が 16 小節より長ければ、ノートが最も多い 16 小節(重い描き出しを避ける)
pub fn default_range(project: &Project) -> Option<(Tick, Tick)> {
    let end = project.end().0;
    let grid = glaux_core::arrange::bar_grid(project, end.max(1));
    if grid.len() <= 16 {
        return None;
    }
    let mut per_bar = vec![0usize; grid.len()];
    for t in &project.tracks {
        for c in &t.clips {
            let start = c.start.0;
            match c.notes() {
                Some(ns) => {
                    for n in ns {
                        let at = start + n.pos.0;
                        let k = grid.partition_point(|(s, _)| *s <= at).saturating_sub(1);
                        per_bar[k] += 1;
                    }
                }
                None => {
                    // 音声クリップは小節ごとに 4 音ぶんと数える
                    for (k, (s, len)) in grid.iter().enumerate() {
                        if *s < start + c.length.0 && s + len > start {
                            per_bar[k] += 4;
                        }
                    }
                }
            }
        }
    }
    let (mut best, mut best_sum) = (0, 0);
    let mut sum: usize = per_bar[..16].iter().sum();
    for k in 0..=grid.len() - 16 {
        if k > 0 {
            sum = sum + per_bar[k + 15] - per_bar[k - 1];
        }
        if sum > best_sum {
            best_sum = sum;
            best = k;
        }
    }
    let s = grid[best].0;
    let e = grid[best + 15].0 + grid[best + 15].1;
    Some((Tick(s), Tick(e)))
}

/// 1 トラックのステレオ(インターリーブ)を測る。`clicks` が false ならクリックは調べない(ドラムなど)
pub fn measure_track(stereo: &[f32], offset_secs: f64, clicks: bool) -> TrackCheck {
    let frames = stereo.len() / 2;
    let mut lp_l = [Biquad::new('l', 150.0), Biquad::new('l', 150.0)];
    let mut lp_r = [Biquad::new('l', 150.0), Biquad::new('l', 150.0)];
    let (mut em, mut es, mut total) = (0.0f64, 0.0f64, 0.0f64);
    let mut peak = 0.0f32;
    let mut mono = Vec::with_capacity(frames);
    for i in 0..frames {
        let (l, r) = (stereo[2 * i] as f64, stereo[2 * i + 1] as f64);
        peak = peak.max(stereo[2 * i].abs()).max(stereo[2 * i + 1].abs());
        total += l * l + r * r;
        let ll = lp_l[0].run(l);
        let ll = lp_l[1].run(ll);
        let rr = lp_r[0].run(r);
        let rr = lp_r[1].run(rr);
        let (m, s) = (0.5 * (ll + rr), 0.5 * (ll - rr));
        em += m * m;
        es += s * s;
        mono.push(0.5 * (l + r) as f32);
    }
    let low_share = (2.0 * (em + es) / total.max(1e-12)).min(1.0);
    let low_side_to_mid_db =
        (low_share > 0.05 && em > 1e-6).then(|| 10.0 * (es.max(1e-14) / em).log10());
    let (abrupt_starts, abrupt_ends) = if clicks {
        find_clicks(&mono, offset_secs)
    } else {
        (vec![], vec![])
    };
    TrackCheck {
        peak_db: 20.0 * (peak as f64).max(1e-6).log10(),
        low_side_to_mid_db,
        low_share,
        abrupt_starts,
        abrupt_ends,
        ..Default::default()
    }
}

/// 段差の立ち上がりと、いきなりの無音を探す(50ms 以内の重なりは 1 つに数える)
fn find_clicks(x: &[f32], offset_secs: f64) -> (Vec<f64>, Vec<f64>) {
    let n = x.len();
    let pre = 96; // 2ms
    let post = 240; // 5ms
    let quiet = 3e-4f32;
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    let gap = (0.05 * SR) as usize;
    // 前の 2ms のエネルギーを窓で追う
    let mut acc = 0.0f32;
    for i in 1..n.saturating_sub(post) {
        acc += x[i - 1] * x[i - 1];
        if i > pre {
            acc -= x[i - 1 - pre] * x[i - 1 - pre];
        }
        let before = (acc.max(0.0) / pre as f32).sqrt();
        let jump = (x[i] - x[i - 1]).abs();
        if before < quiet && jump > 0.02 {
            // 段差: 直後 5ms の大きさの半分以上が 1 サンプルで立ち上がっている
            let after = x[i..i + post].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            if jump >= 0.5 * after && starts.last().is_none_or(|&s: &usize| i - s > gap) {
                starts.push(i);
            }
        }
        // いきなり無音: 直前 1ms は鳴っていて、その後 5ms がほぼ無音、境目で 1 サンプルに落ちる
        if i > 48 && jump > 0.02 {
            let tail = x[i..i + post].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            let head = x[i - 48..i].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            if tail < quiet * 3.0 && head > 0.02 && ends.last().is_none_or(|&s: &usize| i - s > gap)
            {
                ends.push(i);
            }
        }
    }
    let secs = |v: Vec<usize>| -> Vec<f64> {
        v.into_iter()
            .map(|i| ((offset_secs + i as f64 / SR) * 1000.0).round() / 1000.0)
            .collect()
    };
    (secs(starts), secs(ends))
}

/// 40〜120Hz の 20ms ごとのエネルギー(dB)
fn low_band_frames(stereo: &[f32]) -> Vec<f64> {
    let mut hp = [Biquad::new('h', 40.0), Biquad::new('h', 40.0)];
    let mut lp = [Biquad::new('l', 120.0), Biquad::new('l', 120.0)];
    let hop = (0.02 * SR) as usize;
    let mut out = Vec::new();
    let mut acc = 0.0f64;
    for (i, c) in stereo.as_chunks::<2>().0.iter().enumerate() {
        let x = 0.5 * (c[0] + c[1]) as f64;
        let y = hp[0].run(x);
        let y = hp[1].run(y);
        let y = lp[0].run(y);
        let y = lp[1].run(y);
        acc += y * y;
        if (i + 1) % hop == 0 {
            out.push(10.0 * (acc / hop as f64).max(1e-14).log10());
            acc = 0.0;
        }
    }
    out
}

/// キックとベースの低域が同時に強い割合(キックが強い時間のうち)
pub fn kick_bass_overlap(kick: &[f32], bass: &[f32]) -> Option<f64> {
    let k = low_band_frames(kick);
    let b = low_band_frames(bass);
    let n = k.len().min(b.len());
    if n == 0 {
        return None;
    }
    let kmax = k[..n].iter().copied().fold(f64::MIN, f64::max);
    let bmax = b[..n].iter().copied().fold(f64::MIN, f64::max);
    if kmax < -70.0 || bmax < -70.0 {
        return None;
    }
    let kick_on: Vec<usize> = (0..n).filter(|&i| k[i] > kmax - 12.0).collect();
    if kick_on.is_empty() {
        return None;
    }
    // ベースはユニゾンのうなりや減衰で周期的に沈むので、鳴っている判定は広めに(最大から 20dB)
    let both = kick_on.iter().filter(|&&i| b[i] > bmax - 20.0).count();
    Some((both as f64 / kick_on.len() as f64 * 100.0).round() / 100.0)
}

/// 点検の材料を集める。`range` が None なら [`default_range`]。`no_clicks` のトラック(ドラムなど)は
/// クリックを調べない。`kick`・`bass` があれば低域の重なりを測る
pub fn check_mix(
    project: &Project,
    range: Option<(Tick, Tick)>,
    bank: &crate::data::SampleBank,
    no_clicks: &[TrackId],
    kick: Option<&TrackId>,
    bass: Option<&TrackId>,
) -> Result<MixCheck, crate::export::ExportError> {
    let range = range.or_else(|| default_range(project));
    let mix = crate::analyze::analyze_project(project, None, range, bank)?;
    let rendered = crate::analyze::render_tracks_solo(project, range, bank);
    let offset = range.map_or(0.0, |(s, _)| project.tempo_map.tick_to_seconds(s));
    let mut tracks = Vec::new();
    for (t, r) in project.tracks.iter().zip(&rendered) {
        let Some(st) = r else { continue };
        if st.len() < 8192 {
            continue;
        }
        let mut c = measure_track(st, offset, !no_clicks.contains(&t.id));
        c.track_id = t.id.to_string();
        c.name = t.name.clone();
        tracks.push(c);
    }
    let find = |id: Option<&TrackId>| {
        id.and_then(|id| {
            project
                .tracks
                .iter()
                .position(|t| &t.id == id)
                .and_then(|i| rendered[i].as_deref())
        })
    };
    let kick_bass_overlap = match (find(kick), find(bass)) {
        (Some(k), Some(b)) => kick_bass_overlap(k, b),
        _ => None,
    };
    let ma = crate::analyze::analyze_mix_rendered(project, &rendered);
    for c in &mut tracks {
        if let Some(a) = ma.tracks.iter().find(|a| a.track_id == c.track_id) {
            c.centroid_hz = a.spectral_centroid_hz;
            c.crest_db = a.crest_factor_db;
        }
    }
    let masking = ma.masking;
    Ok(MixCheck {
        range: range.map(|(a, b)| (a.0, b.0)),
        mix,
        tracks,
        masking,
        kick_bass_overlap,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo(l: impl Fn(usize) -> f32, r: impl Fn(usize) -> f32, n: usize) -> Vec<f32> {
        (0..n).flat_map(|i| [l(i), r(i)]).collect()
    }

    #[test]
    fn wide_low_end_is_measured() {
        let sine =
            |f: f64| move |i: usize| (std::f64::consts::TAU * f * i as f64 / SR).sin() as f32 * 0.5;
        // 60Hz を左右で逆相(低域が広がりきった最悪の形)と、同相
        let wide = stereo(sine(60.0), |i| -sine(60.0)(i), 48_000);
        let mono = stereo(sine(60.0), sine(60.0), 48_000);
        let w = measure_track(&wide, 0.0, false);
        let m = measure_track(&mono, 0.0, false);
        assert!(w.low_side_to_mid_db.unwrap_or(99.0) > 10.0 || w.low_side_to_mid_db.is_none());
        assert!(
            m.low_side_to_mid_db.unwrap() < -40.0,
            "{:?}",
            m.low_side_to_mid_db
        );
        assert!(m.low_share > 0.9);
    }

    #[test]
    fn clicks_are_found_at_hard_starts_and_stops() {
        // 0.5 秒の無音 → いきなり 0.4 の矩形の続き(段差)→ 0.5 秒鳴って、いきなり無音
        let n = 72_000;
        let x = |i: usize| {
            if (24_000..48_000).contains(&i) {
                if (i / 40).is_multiple_of(2) {
                    0.4
                } else {
                    -0.4
                }
            } else {
                0.0
            }
        };
        let st = stereo(x, x, n);
        let c = measure_track(&st, 1.0, true);
        assert_eq!(c.abrupt_starts.len(), 1, "{:?}", c.abrupt_starts);
        assert!((c.abrupt_starts[0] - 1.5).abs() < 0.01);
        assert_eq!(c.abrupt_ends.len(), 1, "{:?}", c.abrupt_ends);
        // ゆっくり立ち上がる音は数えない
        let soft = |i: usize| {
            if i >= 24_000 {
                let t = ((i - 24_000) as f32 / 480.0).min(1.0);
                t * 0.4 * (i as f32 * 0.05).sin()
            } else {
                0.0
            }
        };
        let st = stereo(soft, soft, n);
        assert!(measure_track(&st, 0.0, true).abrupt_starts.is_empty());
    }

    #[test]
    fn kick_and_bass_overlap_ratio() {
        // キックは 0.5 秒ごとに 0.1 秒の 50Hz、ベースは鳴りっぱなしの 55Hz → ほぼ重なる
        let kick = |i: usize| {
            if i % 24_000 < 4_800 {
                (std::f64::consts::TAU * 50.0 * i as f64 / SR).sin() as f32 * 0.8
            } else {
                0.0
            }
        };
        let bass = |i: usize| (std::f64::consts::TAU * 55.0 * i as f64 / SR).sin() as f32 * 0.5;
        let k = stereo(kick, kick, 96_000);
        let b = stereo(bass, bass, 96_000);
        assert!(kick_bass_overlap(&k, &b).unwrap() > 0.8);
        // ベースがキックの間は鳴らない → 重ならない
        let bass_gap = |i: usize| if i % 24_000 < 4_800 { 0.0 } else { bass(i) };
        let b2 = stereo(bass_gap, bass_gap, 96_000);
        assert!(kick_bass_overlap(&k, &b2).unwrap() < 0.3);
    }
}
