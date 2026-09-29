//! 参考曲の構成の解析。
//!
//! 小節頭(拍の推定から)で区切り、小節ごとの特徴(クロマ = 和音の色、帯域ごとの大きさ = 音色と編成、音量)を出す。
//! 小節どうしの似かたの表(自己類似行列)に、市松模様の窓を対角線に沿って当てて「前後で変わる度合い」
//! (Foote のノベルティ)を求め、その山を区間の境目にする。区間どうしを同じ位置どうしで比べて似ていれば
//! 同じ記号(A・B・C…)を付け、音量と並びから役割(イントロ・A メロ・サビなど)を推し量る。

use glaux_core::harmony::{chord_from_histogram, key_from_histogram, KeyEstimate};
use rustfft::num_complex::Complex;
use serde::Serialize;

/// 帯域の数(40Hz〜16kHz を対数で等分)
const BANDS: usize = 16;

/// 1 小節の特徴
#[derive(Clone, Debug)]
struct BarFeat {
    /// クロマ(和が 1)
    chroma: [f64; 12],
    /// 低音域(55〜220Hz)のクロマ(ベース音の推定に使う)
    bass: [f64; 12],
    /// 帯域ごとの大きさ(dB)
    bands: [f64; BANDS],
    /// 音量(RMS、dBFS)
    rms_db: f64,
    /// 明るさ(スペクトルの重心、Hz)
    centroid: f64,
}

/// 区間(構成の 1 まとまり)
#[derive(Clone, Debug, Serialize)]
pub struct Section {
    /// 繰り返しの記号(同じ記号 = 同じ内容の繰り返し)
    pub label: String,
    /// 役割の推定(intro / verse / pre_chorus / chorus / bridge / outro。分からなければ空)
    pub role: &'static str,
    /// 役割の日本語(イントロ / A メロ / B メロ / サビ / C メロ・間奏 / アウトロ)
    pub role_ja: &'static str,
    /// 開始の小節(1 始まり。解析した最初の小節頭を 1 とする)
    pub start_bar: usize,
    pub bars: usize,
    pub start_sec: f64,
    pub end_sec: f64,
    /// 音量(いちばん大きい区間を 0 とした dB)
    pub energy_db: f64,
    /// 静か / 中くらい / 大きい
    pub level: &'static str,
    /// 明るさ(スペクトルの重心、Hz)
    pub brightness_hz: f64,
    /// 小節ごとのコードの推定(先頭から 8 小節まで。音声からなので目安)
    pub chords: Vec<String>,
    /// 同じ記号の最初の区間との似かた(0〜1。最初の区間は 1)
    pub similarity: f64,
}

/// 構成の解析結果
#[derive(Clone, Debug, Serialize)]
pub struct Structure {
    /// 解析した小節の数
    pub bars: usize,
    pub sections: Vec<Section>,
    /// 記号の並び(例 "A B A B C B")
    pub form: String,
    /// 役割の並び(例 "イントロ(4) → A メロ(8) → サビ(8)")
    pub form_ja: String,
    /// キーの推定(音声のクロマから)
    pub key: Option<KeyEstimate>,
    /// 小節ごとの音量(dBFS、小数 1 桁)
    pub bar_energy_db: Vec<f64>,
}

/// 構成を解析する。`downbeats` は小節頭の時刻(秒、昇順)。小節が 4 つに満たなければ Err
pub fn analyze(frames: &[f32], sample_rate: f32, downbeats: &[f64]) -> Result<Structure, String> {
    let dur = frames.len() as f64 / sample_rate as f64;
    let bounds = bar_bounds(downbeats, dur);
    if bounds.len() < 4 {
        return Err("小節が少なすぎて構成を解析できません(4 小節以上の音声が要ります)".to_owned());
    }
    let feats = bar_features(frames, sample_rate, &bounds);
    let n = feats.len();
    let sim = similarity(&feats);
    let nov = novelty(&sim);
    let min_len = if n >= 24 { 4 } else { 2 };
    let cuts = pick_boundaries(&nov, n, min_len);

    // 区間
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut s = 0;
    for &c in cuts.iter().chain(std::iter::once(&n)) {
        spans.push((s, c));
        s = c;
    }
    let (labels, sims) = label_sections(&spans, &sim);

    // 音量(区間の平均パワー)
    let sec_db: Vec<f64> = spans
        .iter()
        .map(|&(a, b)| {
            let p: f64 = feats[a..b]
                .iter()
                .map(|f| 10f64.powf(f.rms_db / 10.0))
                .sum::<f64>()
                / (b - a) as f64;
            10.0 * p.max(1e-12).log10()
        })
        .collect();
    let loudest = sec_db.iter().cloned().fold(f64::MIN, f64::max);
    let roles = guess_roles(&labels, &spans, &sec_db);

    let chords: Vec<String> = feats
        .iter()
        .map(|f| {
            if f.rms_db < -50.0 {
                return "N.C.".to_owned();
            }
            let bass = argmax(&f.bass).map(|b| b as u8);
            // 倍音の影響を抑える: 第 3 倍音(12 度上 = 5 度上の音名)に漏れた分を差し引く
            // (C の和音の E の倍音で B が立ち、Cmaj7 と読み違えるのを防ぐ)
            let mut w = [0.0f64; 12];
            for (p, o) in w.iter_mut().enumerate() {
                *o = (f.chroma[p] - 0.35 * f.chroma[(p + 5) % 12]).max(0.0);
            }
            chord_from_histogram(&w, bass, false).0
        })
        .collect();

    let r1 = |x: f64| (x * 10.0).round() / 10.0;
    let r2 = |x: f64| (x * 100.0).round() / 100.0;
    let sections: Vec<Section> = spans
        .iter()
        .enumerate()
        .map(|(i, &(a, b))| {
            let rel = sec_db[i] - loudest;
            let (role, role_ja) = roles[i];
            let bright = feats[a..b].iter().map(|f| f.centroid).sum::<f64>() / (b - a) as f64;
            Section {
                label: labels[i].clone(),
                role,
                role_ja,
                start_bar: a + 1,
                bars: b - a,
                start_sec: r2(bounds[a].0),
                end_sec: r2(bounds[b - 1].1),
                energy_db: r1(rel),
                level: if rel >= -3.0 {
                    "大きい"
                } else if rel >= -8.0 {
                    "中くらい"
                } else {
                    "静か"
                },
                brightness_hz: bright.round(),
                chords: chords[a..b.min(a + 8)].to_vec(),
                similarity: r2(sims[i]),
            }
        })
        .collect();

    // キー: 音量で重み付けしたクロマの合計
    let mut hist = [0.0f64; 12];
    for f in &feats {
        let w = 10f64.powf(f.rms_db / 20.0);
        for (h, c) in hist.iter_mut().zip(&f.chroma) {
            *h += c * w;
        }
    }
    let key = (hist.iter().sum::<f64>() > 0.0).then(|| key_from_histogram(&hist));

    let form = sections
        .iter()
        .map(|s| s.label.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let form_ja = sections
        .iter()
        .map(|s| {
            let name = if s.role_ja.is_empty() {
                s.label.as_str()
            } else {
                s.role_ja
            };
            format!("{name}({})", s.bars)
        })
        .collect::<Vec<_>>()
        .join(" → ");
    Ok(Structure {
        bars: n,
        sections,
        form,
        form_ja,
        key,
        bar_energy_db: feats.iter().map(|f| r1(f.rms_db)).collect(),
    })
}

/// 小節の (開始, 終了) 秒。小節頭の間に大きな隙間(中央値の 1.5 倍超。ブレイクで拍が取れなかった所など)があれば
/// 中央値の長さの小節で埋め、小節頭が曲の一部しか覆っていなければ前後も中央値の長さで延ばす
/// (途中で拍を見失っても、小節の番号と数がずれないように)
fn bar_bounds(downbeats: &[f64], dur: f64) -> Vec<(f64, f64)> {
    let mut beats: Vec<f64> = downbeats
        .iter()
        .copied()
        .filter(|t| t.is_finite() && *t >= 0.0 && *t < dur)
        .collect();
    beats.sort_by(|a, b| a.total_cmp(b));
    beats.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    let raw: Vec<(f64, f64)> = beats.windows(2).map(|w| (w[0], w[1])).collect();
    let Some(len) = median_len(&raw) else {
        return raw;
    };
    let mut v: Vec<(f64, f64)> = Vec::new();
    // 前: 最初の小節頭より前を中央値の長さで(半分以上残る所まで)
    let first = beats[0];
    let mut t = first;
    let mut before = Vec::new();
    while t - len >= -len * 0.5 {
        let s = (t - len).max(0.0);
        before.push((s, t));
        t -= len;
    }
    before.reverse();
    v.extend(before);
    // 間: 大きな隙間は中央値の長さの小節に分ける
    for &(a, b) in &raw {
        let gap = b - a;
        if gap > len * 1.5 {
            let k = (gap / len).round().max(1.0) as usize;
            let step = gap / k as f64;
            v.extend((0..k).map(|i| (a + step * i as f64, a + step * (i + 1) as f64)));
        } else {
            v.push((a, b));
        }
    }
    // 後: 最後の小節頭から曲の終わりまで
    let mut t = *beats.last().unwrap_or(&0.0);
    while dur - t >= len * 0.5 {
        let e = (t + len).min(dur);
        v.push((t, e));
        t += len;
    }
    v
}

fn median_len(v: &[(f64, f64)]) -> Option<f64> {
    let mut l: Vec<f64> = v.iter().map(|(a, b)| b - a).collect();
    if l.is_empty() {
        return None;
    }
    l.sort_by(|a, b| a.total_cmp(b));
    Some(l[l.len() / 2])
}

fn argmax(v: &[f64]) -> Option<usize> {
    let (i, m) = v.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
    (*m > 0.0).then_some(i)
}

/// 小節ごとの特徴。STFT(窓 4096〜8192)のフレームを、中心の時刻が入る小節に集める
fn bar_features(frames: &[f32], sr: f32, bounds: &[(f64, f64)]) -> Vec<BarFeat> {
    let win = if sr > 32_000.0 { 8192 } else { 4096 };
    let hop = win / 2;
    let fft = rustfft::FftPlanner::<f32>::new().plan_fft_forward(win);
    let hann: Vec<f32> = (0..win)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / win as f32).cos())
        .collect();
    let bin_hz = sr as f64 / win as f64;
    // ビン → ピッチクラス(55〜4200Hz)、低音のピッチクラス(55〜220Hz)、帯域
    let pc_of = |k: usize| -> Option<usize> {
        let f = k as f64 * bin_hz;
        (55.0..4200.0).contains(&f).then(|| {
            let p = 69.0 + 12.0 * (f / 440.0).log2();
            (p.round() as i64).rem_euclid(12) as usize
        })
    };
    let band_of = |k: usize| -> Option<usize> {
        let f = k as f64 * bin_hz;
        (40.0..16_000.0)
            .contains(&f)
            .then(|| (((f / 40.0).ln() / (400f64).ln() * BANDS as f64) as usize).min(BANDS - 1))
    };
    let pcs: Vec<Option<usize>> = (0..win / 2).map(pc_of).collect();
    let bands: Vec<Option<usize>> = (0..win / 2).map(band_of).collect();

    let n = bounds.len();
    let mut chroma = vec![[0.0f64; 12]; n];
    let mut bass = vec![[0.0f64; 12]; n];
    let mut band = vec![[0.0f64; BANDS]; n];
    let mut cent = vec![(0.0f64, 0.0f64); n];
    let mut count = vec![0usize; n];
    let mut buf = vec![Complex::new(0.0f32, 0.0); win];
    let mut start = 0usize;
    let mut bar = 0usize;
    while start + win <= frames.len() {
        let t = (start + win / 2) as f64 / sr as f64;
        while bar < n && t >= bounds[bar].1 {
            bar += 1;
        }
        if bar >= n {
            break;
        }
        if t >= bounds[bar].0 {
            for (i, c) in buf.iter_mut().enumerate() {
                *c = Complex::new(frames[start + i] * hann[i], 0.0);
            }
            fft.process(&mut buf);
            let mut ch = [0.0f64; 12];
            let mut bs = [0.0f64; 12];
            let mut be = [0.0f64; BANDS];
            let (mut fw, mut w) = (0.0f64, 0.0f64);
            for k in 1..win / 2 {
                let m = buf[k].norm() as f64;
                if let Some(pc) = pcs[k] {
                    ch[pc] += m.sqrt();
                    if (k as f64 * bin_hz) < 220.0 {
                        bs[pc] += m;
                    }
                }
                if let Some(b) = bands[k] {
                    be[b] += m * m;
                }
                fw += k as f64 * bin_hz * m;
                w += m;
            }
            let sum: f64 = ch.iter().sum();
            if sum > 1e-9 {
                for (a, c) in chroma[bar].iter_mut().zip(ch) {
                    *a += c / sum;
                }
            }
            for (a, c) in bass[bar].iter_mut().zip(bs) {
                *a += c;
            }
            for (a, e) in band[bar].iter_mut().zip(be) {
                *a += e;
            }
            cent[bar].0 += fw;
            cent[bar].1 += w;
            count[bar] += 1;
        }
        start += hop;
    }

    (0..n)
        .map(|i| {
            let (a, b) = bounds[i];
            let (sa, sb) = (
                (a * sr as f64) as usize,
                ((b * sr as f64) as usize).min(frames.len()),
            );
            let seg = &frames[sa.min(sb)..sb];
            let rms = if seg.is_empty() {
                0.0
            } else {
                (seg.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / seg.len() as f64).sqrt()
            };
            let c = count[i].max(1) as f64;
            let mut ch = chroma[i];
            let s: f64 = ch.iter().sum();
            if s > 0.0 {
                for x in ch.iter_mut() {
                    *x /= s;
                }
            }
            let mut bd = [0.0f64; BANDS];
            for (o, e) in bd.iter_mut().zip(band[i]) {
                *o = 10.0 * (e / c + 1e-10).log10();
            }
            BarFeat {
                chroma: ch,
                bass: bass[i],
                bands: bd,
                rms_db: 20.0 * rms.max(1e-6).log10(),
                centroid: if cent[i].1 > 0.0 {
                    cent[i].0 / cent[i].1
                } else {
                    0.0
                },
            }
        })
        .collect()
}

/// 小節どうしの似かた(-1〜1)。特徴を小節全体で標準化してから余弦で比べる(クロマ 6 : 音色 4)
fn similarity(feats: &[BarFeat]) -> Vec<Vec<f64>> {
    let n = feats.len();
    let standardize = |rows: Vec<Vec<f64>>, floor: f64| -> Vec<Vec<f64>> {
        let d = rows[0].len();
        let mut out = rows.clone();
        for j in 0..d {
            let mean = rows.iter().map(|r| r[j]).sum::<f64>() / n as f64;
            let var = rows.iter().map(|r| (r[j] - mean).powi(2)).sum::<f64>() / n as f64;
            let sd = var.sqrt().max(floor);
            for (o, r) in out.iter_mut().zip(&rows) {
                o[j] = (r[j] - mean) / sd;
            }
        }
        out
    };
    let chroma = standardize(feats.iter().map(|f| f.chroma.to_vec()).collect(), 0.01);
    let timbre = standardize(
        feats
            .iter()
            .map(|f| {
                let mut v = f.bands.to_vec();
                v.push(f.rms_db);
                v
            })
            .collect(),
        0.5,
    );
    let cos = |a: &[f64], b: &[f64]| {
        let (mut d, mut na, mut nb) = (0.0, 0.0, 0.0);
        for (x, y) in a.iter().zip(b) {
            d += x * y;
            na += x * x;
            nb += y * y;
        }
        if na <= 0.0 || nb <= 0.0 {
            0.0
        } else {
            d / (na.sqrt() * nb.sqrt())
        }
    };
    (0..n)
        .map(|i| {
            (0..n)
                .map(|j| 0.6 * cos(&chroma[i], &chroma[j]) + 0.4 * cos(&timbre[i], &timbre[j]))
                .collect()
        })
        .collect()
}

/// 境目 b(小節 b−1 と b の間)ごとの「前後で変わる度合い」。ガウスで重みを付けた市松模様の窓を対角線に沿って当てる
fn novelty(sim: &[Vec<f64>]) -> Vec<f64> {
    let n = sim.len();
    let k: i64 = if n >= 24 { 4 } else { 2 };
    let g = |d: i64| {
        let x = (d as f64 + 0.5) / (k as f64 * 0.6);
        (-0.5 * x * x).exp()
    };
    (0..=n)
        .map(|b| {
            let (mut acc, mut wsum) = (0.0, 0.0);
            for a in -k..k {
                for c in -k..k {
                    let (i, j) = (b as i64 + a, b as i64 + c);
                    if i < 0 || j < 0 || i >= n as i64 || j >= n as i64 {
                        continue;
                    }
                    let (da, dc) = (
                        if a < 0 { -a - 1 } else { a },
                        if c < 0 { -c - 1 } else { c },
                    );
                    let w = g(da) * g(dc);
                    let sign = if (a < 0) == (c < 0) { 1.0 } else { -1.0 };
                    acc += sign * w * sim[i as usize][j as usize];
                    wsum += w;
                }
            }
            if wsum > 0.0 {
                acc / wsum
            } else {
                0.0
            }
        })
        .collect()
}

/// ノベルティの山から境目を選ぶ。区間どうしは `min_len` 小節以上離す(曲の頭と終わりの区間は 2 小節から)。
/// 4 小節ごとの位置を少し優先してから山を探す(拍の推定の小節頭が 1 つずれても句の頭に寄せる)
fn pick_boundaries(nov: &[f64], n: usize, min_len: usize) -> Vec<usize> {
    let edge = 2.min(min_len);
    if n < edge * 2 + 1 {
        return Vec::new();
    }
    let bonus = |b: usize| {
        if b.is_multiple_of(4) {
            1.15
        } else if b.is_multiple_of(2) {
            1.0
        } else {
            0.85
        }
    };
    let adj: Vec<f64> = (0..=n).map(|b| nov[b] * bonus(b)).collect();
    let inner: Vec<f64> = (edge..=n - edge).map(|b| nov[b]).collect();
    let mean = inner.iter().sum::<f64>() / inner.len() as f64;
    let sd = (inner.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / inner.len() as f64).sqrt();
    let mut cands: Vec<(usize, f64)> = (edge..=n - edge)
        .filter(|&b| adj[b] >= adj[b - 1] && adj[b] >= adj[b + 1])
        .filter(|&b| nov[b].max(nov[b - 1]).max(nov[b + 1]) > mean + 0.25 * sd)
        .map(|b| (b, adj[b]))
        .filter(|(_, s)| *s > 0.0)
        .collect();
    cands.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut cuts: Vec<usize> = Vec::new();
    for (b, _) in cands {
        if cuts.len() >= n / min_len {
            break;
        }
        if cuts.iter().all(|&c| c.abs_diff(b) >= min_len) {
            cuts.push(b);
        }
    }
    cuts.sort();
    cuts
}

/// 区間 p と q を先頭からそろえて比べた似かた(同じ位置の小節どうしの似かたの平均)
fn section_similarity(p: (usize, usize), q: (usize, usize), sim: &[Vec<f64>]) -> f64 {
    let len = (p.1 - p.0).min(q.1 - q.0);
    if len == 0 {
        return 0.0;
    }
    (0..len).map(|t| sim[p.0 + t][q.0 + t]).sum::<f64>() / len as f64
}

/// 記号を付ける: 先の区間と十分に似ていて長さが近ければ同じ記号
fn label_sections(spans: &[(usize, usize)], sim: &[Vec<f64>]) -> (Vec<String>, Vec<f64>) {
    const SAME: f64 = 0.4;
    let mut labels: Vec<String> = Vec::new();
    let mut sims = Vec::new();
    let mut reps: Vec<(String, (usize, usize))> = Vec::new();
    for &sp in spans {
        let len = sp.1 - sp.0;
        let best = reps
            .iter()
            .filter(|(_, r)| {
                let rl = r.1 - r.0;
                len.min(rl) * 2 >= len.max(rl)
            })
            .map(|(l, r)| (l.clone(), section_similarity(sp, *r, sim)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((l, s)) if s >= SAME => {
                labels.push(l);
                sims.push(s.clamp(0.0, 1.0));
            }
            _ => {
                let l = letter(reps.len());
                reps.push((l.clone(), sp));
                labels.push(l);
                sims.push(1.0);
            }
        }
    }
    (labels, sims)
}

fn letter(i: usize) -> String {
    let c = (b'A' + (i % 26) as u8) as char;
    if i < 26 {
        c.to_string()
    } else {
        format!("{c}{}", i / 26)
    }
}

/// 役割の推定。繰り返す記号のうちいちばん大きいのがサビ、その前に来る繰り返しが A メロ(直前の別の記号は B メロ)、
/// 先頭の 1 回きりで静かな区間がイントロ、末尾の 1 回きりがアウトロ、後半の 1 回きりが C メロ・間奏
fn guess_roles(
    labels: &[String],
    spans: &[(usize, usize)],
    db: &[f64],
) -> Vec<(&'static str, &'static str)> {
    let n = labels.len();
    let mut out = vec![("", ""); n];
    let count = |l: &str| labels.iter().filter(|x| *x == l).count();
    let mean_db = |l: &str| {
        let v: Vec<f64> = (0..n).filter(|&i| labels[i] == l).map(|i| db[i]).collect();
        v.iter().sum::<f64>() / v.len().max(1) as f64
    };
    let mut uniq: Vec<&String> = Vec::new();
    for l in labels {
        if !uniq.contains(&l) {
            uniq.push(l);
        }
    }
    let median_db = {
        let mut v = db.to_vec();
        v.sort_by(|a, b| a.total_cmp(b));
        v[v.len() / 2]
    };
    // 先頭(と末尾)にだけ出る、短いか静かな記号はイントロ・アウトロ(繰り返しとして数えない)
    let edge = (n > 2
        && (0..n).all(|i| labels[i] != labels[0] || i == 0 || i == n - 1)
        && (spans[0].1 - spans[0].0 <= 8 || db[0] < median_db))
        .then(|| labels[0].clone());
    let chorus = uniq
        .iter()
        .filter(|l| count(l) >= 2 && Some((**l).clone()) != edge)
        .max_by(|a, b| mean_db(a).total_cmp(&mean_db(b)))
        .map(|l| l.to_string());
    let first_chorus = chorus
        .as_ref()
        .and_then(|c| labels.iter().position(|l| l == c));
    // A メロ: サビより前に始まる繰り返し(いちばん早いもの)
    let verse = uniq
        .iter()
        .filter(|l| count(l) >= 2 && Some(l.to_string()) != chorus && Some((**l).clone()) != edge)
        .filter(|l| {
            let p = labels.iter().position(|x| x == **l).unwrap_or(n);
            first_chorus.is_none_or(|fc| p < fc)
        })
        .map(|l| l.to_string())
        .next();
    // B メロ: サビの直前に 2 回以上来る、A メロでもサビでもない記号
    let pre = chorus.as_ref().and_then(|c| {
        uniq.iter()
            .filter(|l| Some(l.to_string()) != verse && **l != c && Some((**l).clone()) != edge)
            .find(|l| {
                (1..n)
                    .filter(|&i| &labels[i] == c && labels[i - 1] == ***l)
                    .count()
                    >= 2
            })
            .map(|l| l.to_string())
    });
    for i in 0..n {
        let l = &labels[i];
        out[i] = if Some(l) == edge.as_ref() && i == 0 {
            ("intro", "イントロ")
        } else if Some(l) == edge.as_ref() {
            ("outro", "アウトロ")
        } else if Some(l) == chorus.as_ref() {
            ("chorus", "サビ")
        } else if Some(l) == verse.as_ref() {
            ("verse", "A メロ")
        } else if Some(l) == pre.as_ref() {
            ("pre_chorus", "B メロ")
        } else if i == 0 && n > 1 && (db[i] < median_db || spans[i].1 - spans[i].0 <= 8) {
            ("intro", "イントロ")
        } else if i == n - 1 && n > 1 && (count(l) == 1 || labels[0] == *l) {
            ("outro", "アウトロ")
        } else if count(l) == 1 && i * 2 >= n {
            ("bridge", "C メロ・間奏")
        } else {
            ("", "")
        };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 試しの曲: 1 小節 2 秒(120BPM の 4/4)。和音(倍音付きのサイン波)と、区間によってはドラム(雑音)とメロディ
    fn song(plan: &[(char, usize)], sr: f32) -> (Vec<f32>, Vec<f64>) {
        let chords = |c: char| -> [[u8; 3]; 4] {
            match c {
                'A' => [[60, 64, 67], [55, 59, 62], [57, 60, 64], [53, 57, 60]], // C G Am F
                'B' => [[53, 57, 60], [55, 59, 62], [52, 55, 59], [57, 60, 64]], // F G Em Am
                'I' => [[57, 60, 64]; 4],                                        // Am を伸ばすだけ
                _ => [[62, 65, 69], [58, 62, 65], [60, 64, 67], [55, 59, 62]],   // Dm Bb C G
            }
        };
        let bar = 2.0f64;
        let total: usize = plan.iter().map(|p| p.1).sum();
        let len = (total as f64 * bar * sr as f64) as usize;
        let mut out = vec![0.0f32; len];
        let mut seed = 12345u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        };
        let mut b0 = 0usize;
        let mut downbeats = Vec::new();
        for &(sec, bars) in plan {
            let (amp, drums, melody) = match sec {
                'A' => (0.08, false, false),
                'B' => (0.12, true, true),
                'I' => (0.04, false, false),
                _ => (0.06, false, true),
            };
            for k in 0..bars {
                let t0 = (b0 + k) as f64 * bar;
                downbeats.push(t0);
                let ch = chords(sec)[k % 4];
                let s0 = (t0 * sr as f64) as usize;
                let s1 = ((t0 + bar) * sr as f64) as usize;
                for (i, o) in out[s0..s1.min(len)].iter_mut().enumerate() {
                    let t = i as f32 / sr;
                    let mut v = 0.0;
                    for &p in &ch {
                        let f = 440.0 * 2f32.powf((p as f32 - 69.0) / 12.0);
                        for h in 1..=3 {
                            v += (std::f32::consts::TAU * f * h as f32 * t).sin() / h as f32;
                        }
                    }
                    v *= amp;
                    if melody {
                        let f = 440.0 * 2f32.powf((ch[2] as f32 + 12.0 - 69.0) / 12.0);
                        v += 0.05 * (std::f32::consts::TAU * f * t).sin();
                    }
                    if drums {
                        // 拍ごとの雑音の打撃
                        let beat_t = t % 0.5;
                        v += 0.3 * noise() * (-beat_t * 30.0).exp();
                    }
                    *o = v;
                }
            }
            b0 += bars;
        }
        (out, downbeats)
    }

    #[test]
    fn finds_sections_and_repeats() {
        let sr = 16_000.0;
        let plan = [('A', 8), ('B', 8), ('A', 8), ('B', 8), ('C', 8), ('B', 8)];
        let (audio, downbeats) = song(&plan, sr);
        let r = analyze(&audio, sr, &downbeats).unwrap();
        assert_eq!(r.bars, 48);
        let starts: Vec<usize> = r.sections.iter().map(|s| s.start_bar).collect();
        assert_eq!(starts, [1, 9, 17, 25, 33, 41], "{}", r.form_ja);
        assert_eq!(r.form, "A B A B C B", "{}", r.form_ja);
        let roles: Vec<&str> = r.sections.iter().map(|s| s.role).collect();
        assert_eq!(
            roles,
            ["verse", "chorus", "verse", "chorus", "bridge", "chorus"],
            "{}",
            r.form_ja
        );
        // サビがいちばん大きい
        assert_eq!(r.sections[1].energy_db, 0.0);
        assert!(r.sections[0].energy_db < -1.0);
        // コードは音声からの目安(A 区間の 1 小節目は C)
        assert_eq!(r.sections[0].chords[0], "C");
        assert_eq!(r.key.as_ref().unwrap().name, "C major");
    }

    #[test]
    fn intro_and_outro_are_named() {
        let sr = 16_000.0;
        let plan = [('I', 4), ('A', 8), ('B', 8), ('A', 8), ('B', 8), ('I', 4)];
        let (audio, downbeats) = song(&plan, sr);
        let r = analyze(&audio, sr, &downbeats).unwrap();
        let roles: Vec<&str> = r.sections.iter().map(|s| s.role).collect();
        assert_eq!(
            roles,
            ["intro", "verse", "chorus", "verse", "chorus", "outro"],
            "{}",
            r.form_ja
        );
    }

    #[test]
    fn too_short_is_an_error() {
        assert!(analyze(&vec![0.0; 16_000], 16_000.0, &[0.0, 0.5]).is_err());
    }

    #[test]
    fn bars_fill_gaps_and_cover_the_song() {
        // 2 秒の小節で、10〜16 秒しか小節頭が取れなかった 30 秒の曲
        let b = bar_bounds(&[10.0, 12.0, 14.0, 16.0], 30.0);
        assert_eq!(b.len(), 15, "{b:?}");
        assert_eq!(b[0], (0.0, 2.0));
        assert_eq!(b.last().unwrap().1, 30.0);
        // 途中の 8 秒の隙間(ブレイク)は 4 小節
        let b = bar_bounds(&[0.0, 2.0, 4.0, 12.0, 14.0, 16.0], 18.0);
        assert_eq!(b.len(), 9, "{b:?}");
        assert!(b.iter().all(|(s, e)| ((e - s) - 2.0).abs() < 1e-9));
    }
}
