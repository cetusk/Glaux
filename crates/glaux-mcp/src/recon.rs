//! 曲から起こす: 曲の骨組み(拍・小節頭・テンポ・拍子・チューニング・調・和音)の解析。
//!
//! 参考曲の音声から、作業場の最初の手順「格子を合わせる」と、AI が参考曲を読むための材料を作る。
//! 方式は段ごとに選べるようにしておき、評価(`examples/recon_eval.rs`)で比べて既定を決める。

use glaux_engine::mir_eval::ChordSpan;
use serde::Serialize;

/// 拍・小節頭の求め方
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BeatMethod {
    /// Beat This! の公式の「最小の後処理」(ロジットの極大)
    Minimal,
    /// 動的ベイジアンネットワーク(madmom の移植。拍子と小節頭を小節単位で整合させる)
    #[default]
    Dbn,
}

/// 和音の求め方
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ChordMethod {
    /// 小節ごとのクロマをテンプレートに当てる(構成の解析と同じ。三和音だけ)
    PerBar,
    /// 拍ごとの NNLS クロマ(チューニング補正つき)を和音の形に当て、隠れマルコフモデルでつなぐ
    #[default]
    Nnls,
}

/// 解析の方式
#[derive(Clone, Debug, Default)]
pub struct SkeletonOptions {
    pub beat: BeatMethod,
    pub chord: ChordMethod,
    pub chord_opts: glaux_engine::tonal::ChordOptions,
}

/// 調の推定
#[derive(Clone, Debug, Serialize)]
pub struct KeyGuess {
    /// 主音のピッチクラス(C = 0)
    pub tonic: u8,
    pub minor: bool,
    /// "C major" / "A minor"
    pub name: String,
    pub confidence: f64,
}

/// テンポの区間(この拍から次の区間の頭まで一定)
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TempoSegment {
    /// 何番目の拍から(0 始まり。`Skeleton::beats` の番号)
    pub beat: usize,
    /// その拍の時刻(秒)
    pub time: f64,
    pub bpm: f64,
}

/// 曲の骨組み
#[derive(Clone, Debug, Serialize)]
pub struct Skeleton {
    pub duration_sec: f64,
    /// ビートの時刻(秒)
    pub beats: Vec<f64>,
    /// 小節頭の時刻(秒)
    pub downbeats: Vec<f64>,
    /// 代表のテンポ(拍の間隔の中央値から)
    pub bpm: Option<f64>,
    /// テンポの変化(最初の拍から。区間の中は一定)。曲の格子にそのまま使える
    pub tempo_map: Vec<TempoSegment>,
    /// 1 小節のビート数
    pub beats_per_bar: Option<u32>,
    /// チューニングのずれ(セント。A = 440Hz からの。-50〜+50)
    pub tuning_cents: f64,
    pub key: Option<KeyGuess>,
    /// 和音の並び(秒。"N" は和音なし)
    pub chords: Vec<ChordSpan>,
    /// ノリ(ハネ具合・揺れ・細かさ・密度)
    pub groove: Option<glaux_engine::feel::Groove>,
}

/// 曲の骨組みを解析する(モノラル音声)
pub fn analyze(
    frames: &[f32],
    sample_rate: f32,
    opts: SkeletonOptions,
) -> Result<Skeleton, String> {
    let dur = frames.len() as f64 / sample_rate as f64;
    let t = match opts.beat {
        BeatMethod::Dbn => glaux_ml::beats::track_dbn(frames, sample_rate, &Default::default()),
        BeatMethod::Minimal => glaux_ml::beats::track(frames, sample_rate),
    }
    .map_err(|e| e.to_string())?;
    let bpb = t.beats_per_bar();
    let mut beats: Vec<f64> = t.beats.iter().map(|&b| b as f64).collect();
    let mut downbeats: Vec<f64> = t.downbeats.iter().map(|&b| b as f64).collect();
    let onsets = glaux_engine::feel::onsets(frames, sample_rate);
    let mut groove = glaux_engine::feel::analyze(&onsets, &beats);
    // 8 分の裏が拍の半分より前(逆向きのハネ)なら、拍がハネた裏に乗っている。実際の曲ではまず無いので、
    // 拍をその「裏」の位置(本当の拍の頭)へずらす
    if let Some(p) = groove
        .as_ref()
        .and_then(|g| g.swing_8th_pct)
        .filter(|p| *p < 45.0)
    {
        let shift = |v: &[f64]| -> Vec<f64> {
            v.iter()
                .map(|&x| {
                    let i = beats.partition_point(|b| *b <= x).saturating_sub(1);
                    let ibi = if i + 1 < beats.len() {
                        beats[i + 1] - beats[i]
                    } else if i > 0 {
                        beats[i] - beats[i - 1]
                    } else {
                        0.5
                    };
                    x + ibi * p / 100.0
                })
                .collect()
        };
        downbeats = shift(&downbeats);
        beats = shift(&beats);
        groove = glaux_engine::feel::analyze(&onsets, &beats);
    }
    // 構成の解析と同じく、小節頭が取れなければビート n 個ごと
    let bars: Vec<f64> = if downbeats.len() >= 4 {
        downbeats.clone()
    } else if beats.len() >= 16 {
        beats
            .iter()
            .step_by(bpb.unwrap_or(4).max(1) as usize)
            .copied()
            .collect()
    } else {
        (0..(dur / 2.0) as usize).map(|i| i as f64 * 2.0).collect()
    };
    let tempo_map = tempo_map(&beats);
    let bpm = median_bpm(&beats);
    let tuning = glaux_engine::tonal::estimate_tuning(frames, sample_rate);
    let (key, chords) = match opts.chord {
        ChordMethod::PerBar => match glaux_engine::structure::analyze(frames, sample_rate, &bars) {
            Ok(st) => {
                let key = st.key.as_ref().map(|k| KeyGuess {
                    tonic: k.tonic,
                    minor: k.mode == "minor",
                    name: k.name.clone(),
                    confidence: k.confidence,
                });
                let chords = st
                    .bar_bounds
                    .iter()
                    .zip(&st.bar_chords)
                    .map(|(&(start, end), c)| ChordSpan {
                        start,
                        end,
                        symbol: if c == "N.C." {
                            "N".to_owned()
                        } else {
                            c.clone()
                        },
                    })
                    .collect();
                (key, chords)
            }
            Err(_) => (None, Vec::new()),
        },
        ChordMethod::Nnls => {
            let (bounds, pos) = beat_segments(&beats, &downbeats, dur);
            let notes = glaux_engine::tonal::note_activations(frames, sample_rate, tuning, &bounds);
            let chords = glaux_engine::tonal::recognize_chords(
                &notes,
                &bounds,
                &pos,
                bpb.unwrap_or(4),
                &opts.chord_opts,
            );
            let key = glaux_engine::tonal::estimate_key(&notes, &bounds).map(
                |(tonic, minor, confidence)| KeyGuess {
                    tonic,
                    minor,
                    name: format!(
                        "{} {}",
                        glaux_core::chord::note_name(tonic + 60)
                            .trim_end_matches(|c: char| c.is_ascii_digit()),
                        if minor { "minor" } else { "major" }
                    ),
                    confidence,
                },
            );
            (key, chords)
        }
    };
    Ok(Skeleton {
        duration_sec: dur,
        beats,
        downbeats,
        bpm,
        tempo_map,
        beats_per_bar: bpb,
        tuning_cents: (tuning * 10.0).round() / 10.0,
        key,
        chords,
        groove,
    })
}

/// 小節(`bounds` の (開始, 終了) 秒)ごとの和音の名前(区間に 1/8 以上かかる和音を順に。"C G/B")。和音が無ければ "N.C."
pub fn bar_chord_names(chords: &[ChordSpan], bounds: &[(f64, f64)]) -> Vec<String> {
    bounds
        .iter()
        .map(|&(s, e)| {
            let min = (e - s) / 8.0;
            let mut names: Vec<&str> = Vec::new();
            for c in chords {
                let ov = e.min(c.end) - s.max(c.start);
                if ov >= min && c.symbol != "N" && names.last() != Some(&c.symbol.as_str()) {
                    names.push(&c.symbol);
                }
            }
            if names.is_empty() {
                "N.C.".to_owned()
            } else {
                names.join(" ")
            }
        })
        .collect()
}

/// 小節の (開始, 終了) 秒。小節頭から(最初の小節頭より前に拍があれば 0 小節目〈弱起〉も)
fn bars_of(sk: &Skeleton) -> Vec<(usize, f64, f64)> {
    let d = &sk.downbeats;
    let mut out = Vec::new();
    if d.is_empty() {
        return out;
    }
    if sk.beats.first().is_some_and(|b| *b < d[0] - 0.05) {
        out.push((0, sk.beats[0].min(d[0]), d[0]));
    }
    for (i, &t) in d.iter().enumerate() {
        let end = d.get(i + 1).copied().unwrap_or_else(|| {
            let len = if i > 0 { t - d[i - 1] } else { 2.0 };
            (t + len).min(sk.duration_sec)
        });
        if end > t {
            out.push((i + 1, t, end));
        }
    }
    out
}

/// 曲の骨組みを AI 向けの JSON にする(小節ごとの和音・テンポの変化を小節と拍の位置で)
pub fn skeleton_json(sk: &Skeleton, source: &str) -> serde_json::Value {
    use serde_json::json;
    let r2 = |v: f64| (v * 100.0).round() / 100.0;
    let r3 = |v: f64| (v * 1000.0).round() / 1000.0;
    let bars = bars_of(sk);
    let bounds: Vec<(f64, f64)> = bars.iter().map(|b| (b.1, b.2)).collect();
    let names = bar_chord_names(&sk.chords, &bounds);
    // 時刻 → (小節, 拍)
    let bar_beat = |t: f64| -> (usize, usize) {
        let k = bars.iter().rposition(|b| b.1 <= t + 1e-3).unwrap_or(0);
        let start = bars.get(k).map_or(0.0, |b| b.1);
        let beat = sk
            .beats
            .iter()
            .filter(|&&b| b >= start - 1e-3 && b < t - 1e-3)
            .count();
        (bars.get(k).map_or(0, |b| b.0), beat + 1)
    };
    let tempo: Vec<serde_json::Value> = sk
        .tempo_map
        .iter()
        .map(|seg| {
            let (bar, beat) = bar_beat(seg.time);
            json!({"bar": bar, "beat": beat, "time_sec": r3(seg.time), "bpm": seg.bpm})
        })
        .collect();
    const MAX_BARS: usize = 300;
    let bar_list: Vec<serde_json::Value> = bars
        .iter()
        .zip(&names)
        .take(MAX_BARS)
        .map(|(b, n)| json!({"bar": b.0, "start_sec": r2(b.1), "chords": n}))
        .collect();
    let changes = sk.tempo_map.len().saturating_sub(1);
    let summary = format!(
        "{}{}{}{}。{}{}",
        sk.bpm
            .map(|b| format!("約 {b} BPM"))
            .unwrap_or_else(|| "テンポ不明".to_owned()),
        sk.beats_per_bar
            .map(|n| format!("・1 小節 {n} 拍"))
            .unwrap_or_default(),
        sk.key
            .as_ref()
            .map(|k| format!("・{}(確からしさ {:.0}%)", k.name, k.confidence * 100.0))
            .unwrap_or_default(),
        if sk.tuning_cents.abs() >= 5.0 {
            format!("・チューニングが {:+.0} セントずれている", sk.tuning_cents)
        } else {
            String::new()
        },
        if changes == 0 {
            "テンポは一定".to_owned()
        } else if changes <= 3 {
            format!("テンポが {changes} 回変わる")
        } else {
            format!("テンポが細かく揺れる・だんだん変わる({} 区間)", changes + 1)
        },
        sk.groove
            .as_ref()
            .filter(|g| !g.summary.is_empty())
            .map(|g| format!("。ノリ: {}", g.summary))
            .unwrap_or_default(),
    );
    json!({
        "source": source,
        "duration_sec": r2(sk.duration_sec),
        "bpm": sk.bpm,
        "beats_per_bar": sk.beats_per_bar,
        "first_downbeat_sec": sk.downbeats.first().map(|d| r3(*d)),
        "tuning_cents": sk.tuning_cents,
        "key": sk.key,
        "tempo_map": tempo,
        "bars": bar_list,
        "bars_total": bars.len(),
        "groove": sk.groove,
        "summary": summary,
    })
}

/// 自分の曲(解析と同じ条件で描き出したステレオ)のノリ。拍は曲のテンポの格子から
pub fn song_groove(
    project: &glaux_core::Project,
    stereo: &[f32],
    start_sec: f64,
) -> Option<glaux_engine::feel::Groove> {
    let mono: Vec<f32> = stereo.chunks(2).map(|c| (c[0] + c[1]) * 0.5).collect();
    let sr = 48_000.0f32;
    let dur = mono.len() as f64 / sr as f64;
    let tm = &project.tempo_map;
    let ppq = glaux_core::time::PPQ;
    let first = (tm.seconds_to_tick_f64(start_sec) / ppq as f64).floor() as u64;
    let beats: Vec<f64> = (first..)
        .map(|b| tm.tick_to_seconds(glaux_core::Tick(b * ppq)) - start_sec)
        .skip_while(|t| *t < 0.0)
        .take_while(|t| *t < dur)
        .collect();
    let onsets = glaux_engine::feel::onsets(&mono, sr);
    glaux_engine::feel::analyze(&onsets, &beats)
}

/// 参考曲と曲の統合ラウドネス(LUFS)を、同じ範囲で測る。範囲は曲の秒(None なら参考曲のクリップの範囲)。
/// 参考曲は、参考曲のトラックの最初の音声クリップ。(曲, 参考曲)。測れなければ None
pub fn reference_loudness(
    project: &glaux_core::Project,
    dir: &std::path::Path,
    range: Option<(f64, f64)>,
) -> Result<(Option<f64>, Option<f64>), String> {
    use glaux_core::ClipContent;
    let clip = project
        .tracks
        .iter()
        .filter(|t| t.reference)
        .flat_map(|t| t.clips.iter())
        .find(|c| matches!(c.content, ClipContent::Audio { .. }))
        .ok_or_else(|| "参考曲のトラックがありません".to_owned())?;
    let tm = &project.tempo_map;
    let (c0, c1) = (
        tm.tick_to_seconds(clip.start),
        tm.tick_to_seconds(clip.start + clip.length),
    );
    let (from, to) = match range {
        Some((a, b)) => (a.max(c0), b.min(c1)),
        None => (c0, c1),
    };
    if to - from < 1.0 {
        return Err("参考曲と重なる範囲が短すぎます(1 秒以上)".to_owned());
    }
    let rate = crate::sound::RENDER_RATE;
    let all = crate::sound::load_stereo_source(
        project,
        dir,
        &crate::sound::SoundSource::Clip(clip.id.clone()),
    )?;
    let a = (((from - c0) * rate) as usize * 2).min(all.len());
    let b = (((to - c0) * rate) as usize * 2).min(all.len());
    let reference = &all[a..b];
    let bank = glaux_engine::SampleBank::for_offline(project, dir);
    let span = (tm.seconds_to_tick(from), tm.seconds_to_tick(to));
    // 曲がまだ空(参考曲しか無い)なら曲側は測れない
    let song =
        glaux_engine::analyze::render_song_for_analysis(project, None, Some(span), &bank).ok();
    let lufs = |x: &[f32]| {
        let v = glaux_engine::export::lufs_at(x, rate as u32);
        v.is_finite().then_some((v * 10.0).round() / 10.0)
    };
    Ok((song.as_deref().and_then(lufs), lufs(reference)))
}

/// 拍ごとの区間(秒)と、区間の頭が小節の何拍目か(0 始まり)。最初の拍の前・最後の拍の後も区間にする
fn beat_segments(
    beats: &[f64],
    downbeats: &[f64],
    dur: f64,
) -> (Vec<(f64, f64)>, Vec<Option<u32>>) {
    let mut bounds = Vec::new();
    let mut pos = Vec::new();
    if beats.is_empty() {
        // 拍が無い: 1 秒ずつ
        let n = dur.ceil().max(1.0) as usize;
        for i in 0..n {
            bounds.push((i as f64, ((i + 1) as f64).min(dur)));
            pos.push(None);
        }
        return (bounds, pos);
    }
    if beats[0] > 0.05 {
        bounds.push((0.0, beats[0]));
        pos.push(None);
    }
    let mut count: Option<u32> = None;
    for (i, &b) in beats.iter().enumerate() {
        let end = beats.get(i + 1).copied().unwrap_or_else(|| {
            // 最後の拍は直前の拍の長さで閉じる
            let ibi = if i > 0 { b - beats[i - 1] } else { 0.5 };
            (b + ibi).min(dur)
        });
        if downbeats.iter().any(|d| (d - b).abs() < 1e-3) {
            count = Some(0);
        } else if let Some(c) = count.as_mut() {
            *c += 1;
        }
        if end > b {
            bounds.push((b, end));
            pos.push(count);
        }
    }
    if let Some(&(_, last)) = bounds.last() {
        if dur - last > 0.05 {
            bounds.push((last, dur));
            pos.push(None);
        }
    }
    (bounds, pos)
}

/// 拍の間隔の中央値からのテンポ
fn median_bpm(beats: &[f64]) -> Option<f64> {
    let mut ibi: Vec<f64> = beats
        .windows(2)
        .map(|w| w[1] - w[0])
        .filter(|d| *d > 0.0)
        .collect();
    if ibi.is_empty() {
        return None;
    }
    ibi.sort_by(f64::total_cmp);
    Some((60.0 / ibi[ibi.len() / 2] * 100.0).round() / 100.0)
}

/// テンポの区間に入れてよい、拍のずれの上限(秒)。拍の当たりの幅(70ms)より十分狭く
const TEMPO_TOL: f64 = 0.045;

/// 拍の時刻から、区間ごとに一定のテンポの並びを作る。区間の中の拍を最小二乗の直線(前の区間の終わりに
/// つなげる。最初の区間だけ切片も合わせる)で表し、ずれが [`TEMPO_TOL`] を超えない限り区間を伸ばす。
/// 演奏の細かな揺れや拍の推定の誤差は 1 つの区間にまとめ、テンポの変化(段差・だんだん遅く)は区間を分けて追う。
/// 区間の境目で時刻が飛ばない(曲のテンポの変化としてそのまま書ける)
pub fn tempo_map(beats: &[f64]) -> Vec<TempoSegment> {
    let n = beats.len();
    if n < 2 {
        return Vec::new();
    }
    // 区間 s..=e を直線で表す。anchor があれば s でその時刻を通る。(s での時刻, 1 拍の秒) とずれの最大
    let fit = |s: usize, e: usize, anchor: Option<f64>| -> Option<(f64, f64, f64)> {
        let (a, m) = match anchor {
            Some(a) => {
                let (mut num, mut den) = (0.0, 0.0);
                for (x, b) in beats[s + 1..=e].iter().enumerate() {
                    let x = (x + 1) as f64;
                    num += x * (b - a);
                    den += x * x;
                }
                (a, num / den)
            }
            None => {
                let cnt = (e - s + 1) as f64;
                let mx = (e - s) as f64 / 2.0;
                let my = beats[s..=e].iter().sum::<f64>() / cnt;
                let (mut sxy, mut sxx) = (0.0, 0.0);
                for (x, b) in beats[s..=e].iter().enumerate() {
                    let x = x as f64 - mx;
                    sxy += x * (b - my);
                    sxx += x * x;
                }
                let m = sxy / sxx;
                (my - m * mx, m)
            }
        };
        if m <= 0.0 {
            return None;
        }
        let worst = (s..=e)
            .map(|k| (beats[k] - (a + (k - s) as f64 * m)).abs())
            .fold(0.0, f64::max);
        Some((a, m, worst))
    };
    let mut out = Vec::new();
    let mut s = 0;
    let mut anchor: Option<f64> = None;
    while s < n - 1 {
        let mut best = None;
        for e in s + 1..n {
            match fit(s, e, anchor) {
                Some((a, m, worst)) if worst <= TEMPO_TOL || e == s + 1 => best = Some((e, a, m)),
                _ => break,
            }
        }
        let Some((e, a, m)) = best else {
            break;
        };
        out.push(TempoSegment {
            beat: s,
            time: (a * 1000.0).round() / 1000.0,
            bpm: (60.0 / m * 100.0).round() / 100.0,
        });
        anchor = Some(a + (e - s) as f64 * m);
        s = e;
    }
    out
}

/// テンポの並びから、`count` 個の拍の時刻を作る(最初の区間の頭から)
pub fn grid_beats(map: &[TempoSegment], count: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(count);
    let Some(first) = map.first() else {
        return out;
    };
    let mut seg = 0;
    let mut t = first.time;
    for i in first.beat..first.beat + count {
        while seg + 1 < map.len() && map[seg + 1].beat <= i {
            seg += 1;
            // 区間の頭は拍の時刻そのもの(丸めの誤差をためない)
            t = map[seg].time;
        }
        out.push(t);
        t += 60.0 / map[seg].bpm;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steady_beats_make_one_segment_and_changes_split() {
        // 120 BPM で人の揺れ(±15ms)
        let jit = [0.01, -0.015, 0.0, 0.012, -0.008, 0.015, -0.01, 0.0];
        let beats: Vec<f64> = (0..64).map(|i| 1.0 + i as f64 * 0.5 + jit[i % 8]).collect();
        let m = tempo_map(&beats);
        assert_eq!(m.len(), 1, "{m:?}");
        assert!((m[0].bpm - 120.0).abs() < 0.5);
        // 32 拍目から 100 BPM
        let mut b: Vec<f64> = (0..32).map(|i| i as f64 * 0.5).collect();
        let t0 = 32.0 * 0.5;
        b.extend((0..32).map(|i| t0 + i as f64 * 0.6));
        let m = tempo_map(&b);
        assert_eq!(m.len(), 2, "{m:?}");
        // 32 拍目(16.0 秒)までは 0.5 秒刻み、そこから 0.6 秒刻み
        assert_eq!(m[1].beat, 32);
        assert!((m[0].bpm - 120.0).abs() < 0.5 && (m[1].bpm - 100.0).abs() < 0.5);
        let g = grid_beats(&m, b.len());
        for (x, y) in g.iter().zip(&b) {
            assert!((x - y).abs() < 0.04, "{x} {y}");
        }
    }

    #[test]
    fn segments_cover_beats() {
        let (bounds, pos) = beat_segments(&[1.0, 1.5, 2.0, 2.5, 3.0], &[1.0, 3.0], 4.0);
        assert_eq!(bounds.first(), Some(&(0.0, 1.0)));
        assert_eq!(bounds.last(), Some(&(3.5, 4.0)));
        assert_eq!(&pos[1..5], &[Some(0), Some(1), Some(2), Some(3)]);
        assert_eq!(pos[5], Some(0));
    }
}
