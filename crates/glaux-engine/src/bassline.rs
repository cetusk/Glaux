//! ベースの音符を、音の高さの候補(pYIN)と音符の隠れマルコフモデル(Tony)で起こす(曲から起こす機能の試作。調査文書 13.24 節)。
//!
//! 分離ツールで分けたベースの音から、単音のベースの音符を起こす。ウォブル(LFO でフィルタが動く音)・808 のように
//! 音色が時間で大きく変わる音では、音量の立ち上がりが音符の頭と合わず、basic-pitch・SwiftF0 の音符の区切りが崩れる
//! (音の高さはフレームごとには 5〜7 割合っている)。そこで、区切りを「音量の立ち上がり」ではなく、
//! 音の高さの並びと拍の位置を含む 1 つの確率モデルで決める:
//!
//! 1. 低い帯域だけを残して間引き、フレーム(10ms)ごとに pYIN(Mauch & Dixon 2014)で基音の候補と確からしさを出す
//!    (しきい値を 1 つに決めず、しきい値の分布で重みを付けた候補の集まりにする)。SwiftF0 は 46.875Hz より下を扱えず、
//!    808・サブベース(30〜45Hz)を取れないため、時間領域の方法にする
//! 2. 音符の HMM(Mauch ら 2015 の Tony の音符の層): 音の高さ(半音)ごとに 立ち上がり → 続き の状態と、無音の状態。
//!    遷移は 音を終える・次の音へ移る(音程の差の分布)・無音から始める(音の高さの分布) の確率で、
//!    「音符が始まる確率」を拍の中の位置で変える(Araz ら 2021 の拍の格子を使う区切りの考え)。
//!    立ち上がりの状態だけ、全帯域のスペクトルの増え方(音の頭)の観測を持つ
//! 3. 遷移の確率・音程の差の分布・音の高さの分布・拍の中の位置の重み・音量の上がり方の尤度比を、
//!    いちばん良い経路(ビタビ)で数え直して曲ごとに学び直す(ビタビ学習)。固定のしきい値で区切らない
//!
//! オフライン処理(UI・MCP のスレッドで呼ぶ)。

use crate::notefit::Note;
use rustfft::{num_complex::Complex, FftPlanner};

/// ベースの譜起こしの設定
#[derive(Clone, Copy, Debug)]
pub struct BassOptions {
    /// 音域(MIDI)
    pub lo: u8,
    pub hi: u8,
    /// 曲ごとに学び直す回数
    pub iterations: usize,
    /// 拍の中の位置で音符の始まる確率を変える
    pub grid: bool,
    /// 音量の上がり方を立ち上がりの観測にする
    pub onset: bool,
}

impl Default for BassOptions {
    fn default() -> Self {
        BassOptions {
            lo: 24,
            hi: 64,
            iterations: 5,
            grid: true,
            onset: true,
        }
    }
}

/// フレームの間隔(秒)
const HOP: f64 = 0.01;
/// 間引いた後の目安の標本化周波数
const TARGET_SR: f32 = 4000.0;
/// 間引く前に掛ける低域通過の切れ目(Hz)
const LOWPASS_HZ: f32 = 1000.0;
/// 候補の音の高さを半音に寄せるときの広がり(半音)
const PITCH_SIGMA: f32 = 0.5;
/// 1 拍を何個に分けて、音符の始まる確率を学ぶか(16 分と 3 連の両方を含む)
const GRID_BINS: usize = 24;
/// 音量の上がり方を何段に分けて、尤度比を学ぶか
const RISE_BINS: usize = 8;
/// 音の高さの読み違いを何半音まで見るか
const ERR_RANGE: usize = 24;
/// 有声の確からしさを何段に分けるか
const VOICE_BINS: usize = 10;
/// 音量を何段に分けるか
const LEVEL_BINS: usize = 8;
/// 立ち上がりの後、必ず続ける長さ(フレーム。音符の最短 = これ + 2)
const CHAIN: usize = 2;

/// 2 次の低域通過(バターワースの 1 段)
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [f32; 2],
}

impl Biquad {
    fn lowpass(sr: f32, fc: f32, q: f32) -> Self {
        let w = 2.0 * std::f32::consts::PI * fc / sr;
        let (s, c) = w.sin_cos();
        let alpha = s / (2.0 * q);
        let a0 = 1.0 + alpha;
        let b1 = (1.0 - c) / a0;
        Biquad {
            b: [b1 / 2.0, b1, b1 / 2.0],
            a: [-2.0 * c / a0, (1.0 - alpha) / a0],
            z: [0.0; 2],
        }
    }

    fn run(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// 低域だけを残して間引く。(音声, 標本化周波数)
fn decimate(audio: &[f32], sr: f32) -> (Vec<f32>, f32) {
    let factor = ((sr / TARGET_SR).floor() as usize).max(1);
    // 4 次のバターワース(2 段)
    let mut f1 = Biquad::lowpass(sr, LOWPASS_HZ, 0.5412);
    let mut f2 = Biquad::lowpass(sr, LOWPASS_HZ, 1.3066);
    let mut out = Vec::with_capacity(audio.len() / factor + 1);
    for (i, &x) in audio.iter().enumerate() {
        let y = f2.run(f1.run(x));
        if i % factor == 0 {
            out.push(y);
        }
    }
    (out, sr / factor as f32)
}

/// フレームごとの基音の候補(MIDI, 確からしさ)。確からしさの和が有声の確からしさ
fn pyin_candidates(
    x: &[f32],
    sr: f32,
    lo_hz: f32,
    hi_hz: f32,
    frames: usize,
) -> Vec<Vec<(f32, f32)>> {
    let tau_min = ((sr / hi_hz).floor() as usize).max(2);
    let tau_max = (sr / lo_hz).ceil() as usize;
    let win = tau_max;
    let hop = HOP * sr as f64;
    // しきい値の事前分布: Beta(2, 11.33)(平均 0.15。pYIN の 3 つの分布の中ほど)を 0.01 刻みで
    let thresholds: Vec<(f32, f32)> = {
        let (a, b) = (2.0f64, 11.333f64);
        let raw: Vec<(f32, f64)> = (1..100)
            .map(|k| {
                let s = k as f64 / 100.0;
                (s as f32, s.powf(a - 1.0) * (1.0 - s).powf(b - 1.0))
            })
            .collect();
        let z: f64 = raw.iter().map(|r| r.1).sum();
        raw.into_iter().map(|(s, w)| (s, (w / z) as f32)).collect()
    };
    // どのしきい値の下にも谷が無いときに、いちばん深い谷に残す確からしさ(pYIN の p_a)
    const ABSOLUTE_MIN_PROB: f32 = 0.01;
    let mut d = vec![0f32; tau_max + 2];
    let mut cm = vec![1f32; tau_max + 2];
    (0..frames)
        .map(|t| {
            // 窓はフレームの時刻から先を見る(t に始まった音は、t の窓から混じりけなく入る)
            let start = (t as f64 * hop) as isize;
            let at = |i: isize| -> f32 {
                if i < 0 || i as usize >= x.len() {
                    0.0
                } else {
                    x[i as usize]
                }
            };
            let energy: f32 = (0..win).map(|j| at(start + j as isize).powi(2)).sum();
            if energy <= 1e-10 {
                return Vec::new();
            }
            for (tau, dv) in d.iter_mut().enumerate().take(tau_max + 2).skip(1) {
                let mut s = 0.0;
                for j in 0..win {
                    let v = at(start + j as isize) - at(start + (j + tau) as isize);
                    s += v * v;
                }
                *dv = s;
            }
            let mut run = 0.0;
            for tau in 1..tau_max + 2 {
                run += d[tau];
                cm[tau] = if run > 0.0 {
                    d[tau] * tau as f32 / run
                } else {
                    1.0
                };
            }
            // 谷(極小)の位置
            let mins: Vec<usize> = (tau_min..=tau_max)
                .filter(|&tau| cm[tau] <= cm[tau - 1] && cm[tau] <= cm[tau + 1])
                .collect();
            if mins.is_empty() {
                return Vec::new();
            }
            let mut prob: Vec<(usize, f32)> = Vec::new();
            let add = |prob: &mut Vec<(usize, f32)>, tau: usize, p: f32| {
                if let Some(e) = prob.iter_mut().find(|e| e.0 == tau) {
                    e.1 += p;
                } else {
                    prob.push((tau, p));
                }
            };
            let deepest = mins
                .iter()
                .copied()
                .min_by(|a, b| cm[*a].total_cmp(&cm[*b]))
                .unwrap_or(tau_min);
            for &(s, w) in &thresholds {
                match mins.iter().find(|&&tau| cm[tau] < s) {
                    Some(&tau) => add(&mut prob, tau, w),
                    None => add(&mut prob, deepest, w * ABSOLUTE_MIN_PROB),
                }
            }
            prob.into_iter()
                .map(|(tau, p)| {
                    // 放物線で谷の位置を細かく
                    let (a, b, c) = (cm[tau - 1], cm[tau], cm[tau + 1]);
                    let den = a - 2.0 * b + c;
                    let off = if den.abs() > 1e-9 {
                        (0.5 * (a - c) / den).clamp(-1.0, 1.0)
                    } else {
                        0.0
                    };
                    let f0 = sr / (tau as f32 + off);
                    (69.0 + 12.0 * (f0 / 440.0).log2(), p)
                })
                .collect()
        })
        .collect()
}

/// フレームごとの低い帯域の音量(dB、10ms の窓)
fn frame_db(x: &[f32], sr: f32, frames: usize) -> Vec<f32> {
    let hop = HOP * sr as f64;
    let half = (0.5 * hop) as isize;
    (0..frames)
        .map(|t| {
            let c = (t as f64 * hop) as isize;
            let (a, b) = (
                (c - half).max(0) as usize,
                ((c + half) as usize).min(x.len()),
            );
            if b <= a {
                return -120.0;
            }
            let e = x[a..b].iter().map(|v| v * v).sum::<f32>() / (b - a) as f32;
            10.0 * e.max(1e-12).log10()
        })
        .collect()
}

/// 曲の中の分位で n 段に分ける
fn quantile_bins(v: &[f32], n: usize) -> Vec<usize> {
    let mut sorted = v.to_vec();
    sorted.sort_by(f32::total_cmp);
    let edges: Vec<f32> = (1..n).map(|q| sorted[(sorted.len() - 1) * q / n]).collect();
    v.iter()
        .map(|r| edges.iter().filter(|e| r > e).count())
        .collect()
}

/// フレームごとのスペクトルの増え方(対数の大きさの増えた分の和。全帯域。音の頭の観測)。
/// 低域だけの音量は、同じ高さで弾き直した 808・サブベースでほとんど変わらないので、倍音・フィルタの開きの立ち上がりも見る
fn spectral_flux(audio: &[f32], sr: f32, frames: usize) -> Vec<f32> {
    let n = ((sr * 0.023) as usize).next_power_of_two();
    let hop = HOP * sr as f64;
    let fft = FftPlanner::<f32>::new().plan_fft_forward(n);
    let hann: Vec<f32> = (0..n)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos())
        .collect();
    let mut buf = vec![Complex::new(0.0f32, 0.0); n];
    let mut prev = vec![0f32; n / 2];
    (0..frames)
        .map(|t| {
            let c = (t as f64 * hop) as isize - (n / 2) as isize;
            for (i, b) in buf.iter_mut().enumerate() {
                let k = c + i as isize;
                let x = if k >= 0 {
                    audio.get(k as usize).copied().unwrap_or(0.0)
                } else {
                    0.0
                };
                *b = Complex::new(x * hann[i], 0.0);
            }
            fft.process(&mut buf);
            let mut flux = 0.0;
            for (k, p) in prev.iter_mut().enumerate() {
                let l = (1.0 + 100.0 * buf[k].norm()).ln();
                flux += (l - *p).max(0.0);
                *p = l;
            }
            flux
        })
        .collect()
}

/// 拍の中の位置(0..GRID_BINS)。拍が無ければ None
fn grid_bins(beats: &[f64], frames: usize) -> Vec<Option<usize>> {
    if beats.len() < 2 {
        return vec![None; frames];
    }
    let mut out = Vec::with_capacity(frames);
    let mut i = 0;
    for t in 0..frames {
        let time = t as f64 * HOP;
        while i + 2 < beats.len() && beats[i + 1] <= time {
            i += 1;
        }
        let (a, b) = (beats[i], beats[i + 1]);
        let phase = ((time - a) / (b - a)).rem_euclid(1.0);
        out.push(Some(
            ((phase * GRID_BINS as f64) as usize).min(GRID_BINS - 1),
        ));
    }
    out
}

/// 学び直す確率
struct Params {
    /// 無音から音符を始める確率(フレームごと、拍の位置の重みを掛ける前)
    p_start: f64,
    /// 音符を終えて無音にする確率
    p_end: f64,
    /// 次の音符へ移る確率(拍の位置の重みを掛ける前)
    p_change: f64,
    /// 音程の差の分布(添字 = 差 + M − 1)
    jump: Vec<f64>,
    /// 無音から始めるときの音の高さの分布
    pitch: Vec<f64>,
    /// 拍の中の位置の重み(平均 1)
    grid: Vec<f64>,
    /// 音量の上がり方の段ごとの尤度比(立ち上がりのフレーム / それ以外)
    rise_on: Vec<f64>,
    rise_off: Vec<f64>,
    /// 有声の確からしさの段・音量の段の分布(音符の状態 / 無音の状態)
    /// 音の高さの読み違いの分布(添字 = ずれ + ERR_RANGE)
    err: Vec<f64>,
    voice_note: Vec<f64>,
    voice_sil: Vec<f64>,
    level_note: Vec<f64>,
    level_sil: Vec<f64>,
}

/// 和が 1 になるように割る
fn norm(v: &mut [f64]) {
    let z: f64 = v.iter().sum();
    if z > 0.0 {
        v.iter_mut().for_each(|x| *x /= z);
    }
}

/// フレームごとの観測(学び直しの間は変わらない)
struct Observations<'a> {
    m_count: usize,
    /// 半音の向きの音の高さの分布(フレーム × 半音)
    pdist: &'a [f32],
    voiced: &'a [f32],
    vbin: &'a [usize],
    ebin: &'a [usize],
    rise_bin: &'a [usize],
    gbin: &'a [Option<usize>],
    onset: bool,
}

/// 事後確率で数えた出来事の数(バウム・ウェルチの E ステップ)
struct Counts {
    /// 無音にいたフレームの数(拍の位置の重みを掛けて)
    sil_w: f64,
    starts: f64,
    /// 続きにいたフレームの数(そのまま・拍の位置の重みを掛けて)
    sus_n: f64,
    sus_w: f64,
    ends: f64,
    changes: f64,
    jump: Vec<f64>,
    pitch: Vec<f64>,
    grid_on: Vec<f64>,
    grid_all: Vec<f64>,
    r_on: Vec<f64>,
    r_all: Vec<f64>,
    voice_note: Vec<f64>,
    voice_sil: Vec<f64>,
    level_note: Vec<f64>,
    level_sil: Vec<f64>,
    err: Vec<f64>,
}

impl Params {
    /// 始めの値。`p_change` は次の音符へ移る確率、`reattack` は同じ高さの弾き直しの重み(音程の差 0 の重みに掛ける)
    fn initial(m_count: usize, p_change: f64, reattack: f64) -> Self {
        let n_jump = 2 * m_count - 1;
        let mut prm = Params {
            p_start: 0.02,
            p_end: 0.01,
            p_change,
            jump: (0..n_jump)
                .map(|i| {
                    let d = (i as f64 - (m_count as f64 - 1.0)).abs();
                    (-d / 4.0).exp() * if d == 0.0 { reattack } else { 1.0 }
                })
                .collect(),
            pitch: vec![1.0 / m_count as f64; m_count],
            grid: vec![1.0; GRID_BINS],
            rise_on: (0..RISE_BINS).map(|q| 2f64.powf(q as f64 - 4.0)).collect(),
            rise_off: vec![1.0; RISE_BINS],
            err: (0..=2 * ERR_RANGE)
                .map(|k| match (k as isize - ERR_RANGE as isize).abs() {
                    0 => 1.0,
                    1 | 12 => 0.1,
                    _ => 0.01,
                })
                .collect(),
            voice_note: (0..VOICE_BINS).map(|b| (b + 1) as f64).collect(),
            voice_sil: (0..VOICE_BINS).map(|b| (VOICE_BINS - b) as f64).collect(),
            level_note: (0..LEVEL_BINS).map(|b| (b + 1) as f64).collect(),
            level_sil: (0..LEVEL_BINS).map(|b| (LEVEL_BINS - b) as f64).collect(),
        };
        for v in [
            &mut prm.jump,
            &mut prm.err,
            &mut prm.voice_note,
            &mut prm.voice_sil,
            &mut prm.level_note,
            &mut prm.level_sil,
        ] {
            norm(v);
        }
        prm
    }

    /// フレーム t の (始める確率, 移る確率, 続ける確率)
    fn trans(&self, g: Option<usize>) -> (f64, f64, f64) {
        let g = g.map_or(1.0, |b| self.grid[b]);
        let ps = (self.p_start * g).min(0.5);
        let pc = (self.p_change * g).min(0.5);
        (ps, pc, (1.0 - self.p_end - pc).max(1e-6))
    }

    /// 数えた出来事から確率を決め直す(M ステップ。少ない数は前の値ではなく弱い事前分布に寄せる)
    fn update(&mut self, c: &Counts, with_grid: bool) {
        self.p_start = (c.starts + 1.0) / (c.sil_w + 2.0);
        self.p_end = (c.ends + 1.0) / (c.sus_n + 2.0);
        self.p_change = (c.changes + 1.0) / (c.sus_w + 2.0);
        let smooth = |v: &[f64], a: f64| -> Vec<f64> {
            let mut out: Vec<f64> = v.iter().map(|x| x + a).collect();
            norm(&mut out);
            out
        };
        self.jump = smooth(&c.jump, 0.5);
        self.pitch = smooth(&c.pitch, 0.5);
        self.err = smooth(&c.err, 0.1);
        self.voice_note = smooth(&c.voice_note, 1.0);
        self.voice_sil = smooth(&c.voice_sil, 1.0);
        self.level_note = smooth(&c.level_note, 1.0);
        self.level_sil = smooth(&c.level_sil, 1.0);
        if with_grid {
            // 拍の位置ごとの始まりやすさ(位置ごとの始まりの割合 / 全体の割合)。数が少ない位置は 1 に寄せる
            let rate = c.grid_on.iter().sum::<f64>() / c.grid_all.iter().sum::<f64>().max(1.0);
            self.grid = c
                .grid_on
                .iter()
                .zip(&c.grid_all)
                .map(|(o, a)| (o + 1.0) / (a * rate + 1.0))
                .collect();
        }
        // 段ごとの尤度比: P(段 | 立ち上がり) / P(段) と P(段 | それ以外) / P(段)
        let (n_on, n_all) = (c.r_on.iter().sum::<f64>(), c.r_all.iter().sum::<f64>());
        for q in 0..RISE_BINS {
            let p_q = (c.r_all[q] + 1.0) / (n_all + RISE_BINS as f64);
            let p_on = (c.r_on[q] + 1.0) / (n_on + RISE_BINS as f64);
            let p_off = (c.r_all[q] - c.r_on[q] + 1.0) / (n_all - n_on + RISE_BINS as f64);
            self.rise_on[q] = p_on / p_q;
            self.rise_off[q] = p_off / p_q;
        }
    }
}

/// フレームごとの対数の観測の尤度: [無音, 立ち上がり(半音ごと), 音符の続き(半音ごと)]
fn log_emissions(o: &Observations, prm: &Params) -> Vec<f32> {
    let m = o.m_count;
    let n_e = 1 + 2 * m;
    let frames = o.voiced.len();
    let ln = |p: f64| p.max(1e-300).ln() as f32;
    let mut out = vec![0f32; frames * n_e];
    for t in 0..frames {
        let (r_on, r_off) = if o.onset {
            (
                ln(prm.rise_on[o.rise_bin[t]]),
                ln(prm.rise_off[o.rise_bin[t]]),
            )
        } else {
            (0.0, 0.0)
        };
        let (vb, eb) = (o.vbin[t], o.ebin[t]);
        let e_note = ln(prm.voice_note[vb]) + ln(prm.level_note[eb]);
        let row = &mut out[t * n_e..(t + 1) * n_e];
        // 無音は音の高さについて何も言わない(半音ごとに 1/M)
        row[0] = ln(prm.voice_sil[vb]) + ln(prm.level_sil[eb]) - (m as f32).ln() + r_off;
        for j in 0..m {
            // 状態の半音 j から d ずれた所に候補が出る確率(読み違いの分布)を掛けて足す
            let mut e = 0.0;
            for (k, &p) in prm.err.iter().enumerate() {
                let q = j as isize + k as isize - ERR_RANGE as isize;
                if (0..m as isize).contains(&q) {
                    e += p * o.pdist[t * m + q as usize] as f64;
                }
            }
            let e = ln(e.max(1e-3 / m as f64)) + e_note;
            row[1 + j] = e + r_on;
            row[1 + m + j] = e + r_off;
        }
    }
    out
}

/// 前向き・後ろ向きで事後確率を求め、出来事を数える。(数, 対数尤度)
fn forward_backward(o: &Observations, prm: &Params) -> (Counts, f64) {
    let m = o.m_count;
    let stride = CHAIN + 2;
    let s_count = 1 + stride * m;
    let n_e = 1 + 2 * m;
    let frames = o.voiced.len();
    let le = log_emissions(o, prm);
    // 観測の尤度(フレームごとに最大で割る。前向きの正規化で打ち消され、尤度には足し戻す)
    let tops: Vec<f32> = (0..frames)
        .map(|t| {
            le[t * n_e..(t + 1) * n_e]
                .iter()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max)
        })
        .collect();
    let b: Vec<f64> = (0..frames)
        .flat_map(|t| {
            let top = tops[t];
            le[t * n_e..(t + 1) * n_e]
                .iter()
                .map(move |x| ((x - top) as f64).exp())
        })
        .collect();
    let state_b = |t: usize, s: usize| -> f64 {
        if s == 0 {
            b[t * n_e]
        } else {
            let j = (s - 1) / stride;
            if (s - 1).is_multiple_of(stride) {
                b[t * n_e + 1 + j]
            } else {
                b[t * n_e + 1 + m + j]
            }
        }
    };
    let sus = |j: usize| 1 + j * stride + stride - 1;
    let att = |j: usize| 1 + j * stride;
    // 前向き
    let mut alpha = vec![0f64; frames * s_count];
    let mut scale = vec![1f64; frames];
    alpha[0] = state_b(0, 0);
    scale[0] = alpha[0].max(1e-300);
    alpha[0] /= scale[0];
    let mut inflow = vec![0f64; m];
    for t in 1..frames {
        let (ps, pc, stay) = prm.trans(o.gbin[t]);
        let (prev, cur) = alpha.split_at_mut(t * s_count);
        let prev = &prev[(t - 1) * s_count..];
        let cur = &mut cur[..s_count];
        let sus_sum: f64 = (0..m).map(|i| prev[sus(i)]).sum();
        cur[0] = (prev[0] * (1.0 - ps) + sus_sum * prm.p_end) * state_b(t, 0);
        for (j, f) in inflow.iter_mut().enumerate() {
            let mut s = 0.0;
            for i in 0..m {
                s += prev[sus(i)] * prm.jump[j + m - 1 - i];
            }
            *f = prev[0] * ps * prm.pitch[j] + pc * s;
        }
        for j in 0..m {
            let base = att(j);
            let bn = b[t * n_e + 1 + m + j];
            cur[base] = inflow[j] * b[t * n_e + 1 + j];
            for c in 1..stride - 1 {
                cur[base + c] = prev[base + c - 1] * bn;
            }
            let s = sus(j);
            cur[s] = (prev[s - 1] + prev[s] * stay) * bn;
        }
        let z: f64 = cur.iter().sum::<f64>().max(1e-300);
        scale[t] = z;
        cur.iter_mut().for_each(|x| *x /= z);
    }
    // 後ろ向き(数えながら)
    let mut c = Counts {
        sil_w: 0.0,
        starts: 0.0,
        sus_n: 0.0,
        sus_w: 0.0,
        ends: 0.0,
        changes: 0.0,
        jump: vec![0.0; 2 * m - 1],
        pitch: vec![0.0; m],
        grid_on: vec![0.0; GRID_BINS],
        grid_all: vec![0.0; GRID_BINS],
        r_on: vec![0.0; RISE_BINS],
        r_all: vec![0.0; RISE_BINS],
        voice_note: vec![0.0; VOICE_BINS],
        voice_sil: vec![0.0; VOICE_BINS],
        level_note: vec![0.0; LEVEL_BINS],
        level_sil: vec![0.0; LEVEL_BINS],
        err: vec![0.0; 2 * ERR_RANGE + 1],
    };
    // フレーム t の事後確率から、観測の分布を数える
    let count_frame = |c: &mut Counts, t: usize, gamma: &[f64]| {
        let a: f64 = (0..m).map(|j| gamma[att(j)]).sum();
        if let Some(g) = o.gbin[t] {
            c.grid_on[g] += a;
            c.grid_all[g] += 1.0;
        }
        c.r_on[o.rise_bin[t]] += a;
        c.r_all[o.rise_bin[t]] += 1.0;
        let s = gamma[0];
        c.voice_sil[o.vbin[t]] += s;
        c.voice_note[o.vbin[t]] += 1.0 - s;
        c.level_sil[o.ebin[t]] += s;
        c.level_note[o.ebin[t]] += 1.0 - s;
        if o.voiced[t] > 1e-6 {
            for j in 0..m {
                let base = att(j);
                let w: f64 = gamma[base..base + stride].iter().sum();
                if w < 1e-6 {
                    continue;
                }
                for (k, e) in c.err.iter_mut().enumerate() {
                    let q = j as isize + k as isize - ERR_RANGE as isize;
                    if (0..m as isize).contains(&q) {
                        *e += w * o.pdist[t * m + q as usize] as f64;
                    }
                }
            }
        }
    };
    let mut beta = vec![1f64; s_count];
    let mut prev_beta = vec![0f64; s_count];
    let mut w = vec![0f64; s_count];
    let mut gamma = vec![0f64; s_count];
    {
        let t = frames - 1;
        gamma.copy_from_slice(&alpha[t * s_count..(t + 1) * s_count]);
        count_frame(&mut c, t, &gamma);
    }
    for t in (1..frames).rev() {
        let (ps, pc, stay) = prm.trans(o.gbin[t]);
        let g = o.gbin[t].map_or(1.0, |x| prm.grid[x]);
        for (s, ws) in w.iter_mut().enumerate() {
            *ws = state_b(t, s) * beta[s] / scale[t];
        }
        let prev = &alpha[(t - 1) * s_count..t * s_count];
        // 無音から
        let mut from_sil = (1.0 - ps) * w[0];
        for j in 0..m {
            let x = ps * prm.pitch[j] * w[att(j)];
            from_sil += x;
            let xi = prev[0] * x;
            c.starts += xi;
            c.pitch[j] += xi;
        }
        prev_beta[0] = from_sil;
        // 続きから: 無音へ・次の音符へ・そのまま
        for i in 0..m {
            let s = sus(i);
            let mut to_att = 0.0;
            for j in 0..m {
                let x = pc * prm.jump[j + m - 1 - i] * w[att(j)];
                to_att += x;
                let xi = prev[s] * x;
                c.jump[j + m - 1 - i] += xi;
                c.changes += xi;
            }
            c.ends += prev[s] * prm.p_end * w[0];
            prev_beta[s] = prm.p_end * w[0] + to_att + stay * w[s];
            for k in 0..stride - 1 {
                prev_beta[att(i) + k] = w[att(i) + k + 1];
            }
        }
        std::mem::swap(&mut beta, &mut prev_beta);
        // フレーム t − 1 の事後確率
        for s in 0..s_count {
            gamma[s] = prev[s] * beta[s];
        }
        let z: f64 = gamma.iter().sum::<f64>().max(1e-300);
        gamma.iter_mut().for_each(|x| *x /= z);
        c.sil_w += gamma[0] * g;
        let sus_p: f64 = (0..m).map(|i| gamma[sus(i)]).sum();
        c.sus_n += sus_p;
        c.sus_w += sus_p * g;
        count_frame(&mut c, t - 1, &gamma);
    }
    let ll =
        scale.iter().map(|x| x.ln()).sum::<f64>() + tops.iter().map(|&x| x as f64).sum::<f64>();
    (c, ll)
}

/// いちばん良い状態の並び(ビタビ)
fn viterbi(o: &Observations, prm: &Params) -> Vec<usize> {
    let m = o.m_count;
    let stride = CHAIN + 2;
    let s_count = 1 + stride * m;
    let n_e = 1 + 2 * m;
    let frames = o.voiced.len();
    let le = log_emissions(o, prm);
    let ln = |p: f64| p.max(1e-300).ln() as f32;
    let ljump: Vec<f32> = prm.jump.iter().map(|&p| ln(p)).collect();
    let lpitch: Vec<f32> = prm.pitch.iter().map(|&p| ln(p)).collect();
    let lend = ln(prm.p_end);
    let mut score = vec![f32::NEG_INFINITY; s_count];
    score[0] = 0.0;
    let mut back = vec![0u16; frames * s_count];
    let mut next = vec![f32::NEG_INFINITY; s_count];
    for t in 0..frames {
        let (ps, pc, stay) = prm.trans(o.gbin[t]);
        let row = &le[t * n_e..(t + 1) * n_e];
        let bk = &mut back[t * s_count..(t + 1) * s_count];
        // 無音
        let mut best = (score[0] + ln(1.0 - ps), 0usize);
        for j in 0..m {
            let s = 1 + j * stride + stride - 1;
            let v = score[s] + lend;
            if v > best.0 {
                best = (v, s);
            }
        }
        next[0] = best.0 + row[0];
        bk[0] = best.1 as u16;
        let (lps, lpc, lstay) = (ln(ps), ln(pc), ln(stay));
        for j in 0..m {
            let base = 1 + j * stride;
            // 立ち上がり: 無音から、または前の音符の続きから
            let mut best = (score[0] + lps + lpitch[j], 0usize);
            for i in 0..m {
                let s = 1 + i * stride + stride - 1;
                let v = score[s] + lpc + ljump[j + m - 1 - i];
                if v > best.0 {
                    best = (v, s);
                }
            }
            next[base] = best.0 + row[1 + j];
            bk[base] = best.1 as u16;
            let e = row[1 + m + j];
            // 立ち上がりの後の続き(必ず通る)
            for c in 1..stride - 1 {
                next[base + c] = score[base + c - 1] + e;
                bk[base + c] = (base + c - 1) as u16;
            }
            // 続き
            let s = base + stride - 1;
            let (a, b) = (score[s - 1], score[s] + lstay);
            if a >= b {
                next[s] = a + e;
                bk[s] = (s - 1) as u16;
            } else {
                next[s] = b + e;
                bk[s] = s as u16;
            }
        }
        // 桁あふれを防ぐ
        let top = next.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        for (a, b) in score.iter_mut().zip(next.iter()) {
            *a = b - top;
        }
    }
    let mut path = vec![0usize; frames];
    let mut s = (0..s_count)
        .max_by(|a, b| score[*a].total_cmp(&score[*b]))
        .unwrap_or(0);
    for t in (0..frames).rev() {
        path[t] = s;
        s = back[t * s_count + s] as usize;
    }
    path
}

/// 調べる用: フレームごとの (有声の確からしさ, いちばん確からしい候補の MIDI)
pub fn debug_frames(audio: &[f32], sr: f32, opts: &BassOptions) -> Vec<(f32, Option<f32>)> {
    let (x, dsr) = decimate(audio, sr);
    let frames = (x.len() as f64 / (HOP * dsr as f64)) as usize;
    let hz = |m: f32| 440.0 * 2f32.powf((m - 69.0) / 12.0);
    pyin_candidates(
        &x,
        dsr,
        hz(opts.lo as f32 - 1.0),
        hz(opts.hi as f32 + 1.0),
        frames,
    )
    .into_iter()
    .map(|c| {
        (
            c.iter().map(|x| x.1).sum(),
            c.iter().max_by(|a, b| a.1.total_cmp(&b.1)).map(|x| x.0),
        )
    })
    .collect()
}

/// 分けたベースの音から音符を起こす。`beats` は曲の拍の時刻(秒。無ければ空)
pub fn transcribe(
    audio: &[f32],
    sr: f32,
    tuning_cents: f64,
    beats: &[f64],
    opts: &BassOptions,
) -> Vec<Note> {
    let (x, dsr) = decimate(audio, sr);
    let frames = (x.len() as f64 / (HOP * dsr as f64)) as usize;
    if frames < 4 || opts.hi <= opts.lo {
        return Vec::new();
    }
    let hz = |m: f32| 440.0 * 2f32.powf((m - 69.0) / 12.0);
    let cands = pyin_candidates(
        &x,
        dsr,
        hz(opts.lo as f32 - 1.0),
        hz(opts.hi as f32 + 1.0),
        frames,
    );
    let m_count = (opts.hi - opts.lo + 1) as usize;
    // 観測: 音の高さ(半音)ごとの確からしさ。候補を半音へ広げ、半音の向きで 1 に正規化してから確からしさを掛ける
    // (音域の外の候補は無声に数える)。和は有声の確からしさ
    let cents = tuning_cents as f32 / 100.0;
    // 半音の格子の上での広がりの和(音域の中の候補なら 1 になるように割る)
    let z_full: f32 = (-8..=8)
        .map(|d| (-(d as f32).powi(2) / (2.0 * PITCH_SIGMA * PITCH_SIGMA)).exp())
        .sum();
    let mut obs = vec![0f32; frames * m_count];
    let mut voiced = vec![0f32; frames];
    for t in 0..frames {
        for &(midi, p) in &cands[t] {
            let midi = midi - cents;
            for j in 0..m_count {
                let dlt = midi - (opts.lo as f32 + j as f32);
                let k = (-dlt * dlt / (2.0 * PITCH_SIGMA * PITCH_SIGMA)).exp() / z_full;
                obs[t * m_count + j] += p * k;
                voiced[t] += p * k;
            }
        }
        voiced[t] = voiced[t].min(1.0);
    }
    // 音の頭の観測(全帯域のスペクトルの増え方)を、曲の中の分位で RISE_BINS 段に
    let db = frame_db(&x, dsr, frames);
    let rise = spectral_flux(audio, sr, frames);
    // 立ち上がりは少ない(フレームの数 % ほど)ので、上の方ほど細かく分ける(分位 1/2・3/4・7/8 …)
    let rise_bin = {
        let mut sorted = rise.clone();
        sorted.sort_by(f32::total_cmp);
        let edges: Vec<f32> = (1..RISE_BINS)
            .map(|k| sorted[((sorted.len() - 1) as f64 * (1.0 - 0.5f64.powi(k as i32))) as usize])
            .collect();
        rise.iter()
            .map(|r| edges.iter().filter(|e| r > *e).count())
            .collect::<Vec<usize>>()
    };
    // 半音の向きの音の高さの分布(有声の確からしさで割る。無声なら一様)
    let pdist: Vec<f32> = (0..frames * m_count)
        .map(|i| {
            let v = voiced[i / m_count];
            if v > 1e-6 {
                obs[i] / v
            } else {
                1.0 / m_count as f32
            }
        })
        .collect();
    // 有声の確からしさの段(0.1 刻み)と、音量の段(曲の中の分位)。どちらも音符か無音かの観測にし、分布を曲ごとに学ぶ
    let vbin: Vec<usize> = voiced
        .iter()
        .map(|v| ((v * VOICE_BINS as f32) as usize).min(VOICE_BINS - 1))
        .collect();
    let ebin = quantile_bins(&db, LEVEL_BINS);
    let gbin = if opts.grid {
        grid_bins(beats, frames)
    } else {
        vec![None; frames]
    };
    // 状態: 0 = 無音、1 + (CHAIN + 2)j = 立ち上がり、続く CHAIN 個 = 立ち上がりの後の続き(必ず通る)、最後 = 続き
    let stride = CHAIN + 2;
    let obsv = Observations {
        m_count,
        pdist: &pdist,
        voiced: &voiced,
        vbin: &vbin,
        ebin: &ebin,
        rise_bin: &rise_bin,
        gbin: &gbin,
        onset: opts.onset,
    };
    // 曲ごとに学び直す(バウム・ウェルチ。いちばん良い経路だけで数えると、少ない出来事(同じ高さの弾き直し)の
    // 確率が回を追って 0 へ寄っていくので、事後確率で数える)
    // EM は始めの値によって別の所に落ち着く(とくに、同じ高さの弾き直しが無い所に落ち着くと抜け出せない)。
    // 始めの値を 3 つ試し、尤度がいちばん高いものを選ぶ: 標準・音を移りやすい(弾き直しも)・移りにくい
    let with_grid = gbin.iter().any(|g| g.is_some());
    let mut best: Option<(f64, Params)> = None;
    for (p_change, reattack) in [(0.02, 1.0), (0.05, 8.0), (0.005, 1.0)] {
        let mut prm = Params::initial(m_count, p_change, reattack);
        for _ in 0..opts.iterations {
            let (counts, _) = forward_backward(&obsv, &prm);
            prm.update(&counts, with_grid);
        }
        let (_, ll) = forward_backward(&obsv, &prm);
        if best.as_ref().is_none_or(|b| ll > b.0) {
            best = Some((ll, prm));
        }
    }
    let Some((_, prm)) = best else {
        return Vec::new();
    };
    let path = viterbi(&obsv, &prm);
    // 経路を音符にする(窓が先を見る分、終わりは窓の半分だけ後ろ)
    let win_secs = 2.0 / hz(opts.lo as f32 - 1.0) as f64;
    let top_db = db.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut notes = Vec::new();
    let mut t = 0;
    while t < frames {
        let s = path[t];
        if s != 0 && (s - 1).is_multiple_of(stride) {
            let j = (s - 1) / stride;
            let mut e = t + 1;
            while e < frames
                && path[e] != 0
                && (path[e] - 1) / stride == j
                && !(path[e] - 1).is_multiple_of(stride)
            {
                e += 1;
            }
            let peak = db[t..e.min(t + 5)]
                .iter()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max);
            notes.push(Note {
                start: t as f64 * HOP,
                end: e as f64 * HOP + 0.5 * win_secs,
                pitch: opts.lo + j as u8,
                vel: (127.0 + 2.0 * (peak - top_db)).clamp(20.0, 127.0) as u8,
            });
            t = e;
        } else {
            t += 1;
        }
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 低いのこぎり波(フィルタが周期的に開閉する)の 3 音を、高さと区切りの両方で起こせる
    #[test]
    fn wobbling_low_notes_keep_their_pitch_and_boundaries() {
        let sr = 22050.0;
        // (開始, 終了, MIDI)
        let plan = [(0.2, 1.0, 33u8), (1.0, 1.8, 36), (2.2, 3.0, 29)];
        let mut audio = vec![0f32; (3.4 * sr) as usize];
        for &(a, b, m) in &plan {
            let f = 440.0 * 2f32.powf((m as f32 - 69.0) / 12.0);
            let mut lp = 0.0f32;
            let first = (a * sr) as usize;
            for (i, out) in audio[first..(b * sr) as usize].iter_mut().enumerate() {
                let t = (first + i) as f32 / sr;
                let saw = 2.0 * (t * f).fract() - 1.0;
                // 1/8 拍ごとに開閉するフィルタ(1 極の低域通過の係数を動かす)
                let lfo = 0.5 + 0.5 * (2.0 * std::f32::consts::PI * 4.0 * t).sin();
                let k = 0.02 + 0.3 * lfo;
                lp += k * (saw - lp);
                *out = 0.5 * lp;
            }
        }
        let beats: Vec<f64> = (0..8).map(|i| 0.2 + i as f64 * 0.4).collect();
        let notes = transcribe(&audio, sr, 0.0, &beats, &BassOptions::default());
        assert_eq!(notes.len(), 3, "{notes:?}");
        for (n, &(a, b, m)) in notes.iter().zip(&plan) {
            assert_eq!(n.pitch, m);
            assert!((n.start - a as f64).abs() < 0.05, "{n:?}");
            assert!((n.end - b as f64).abs() < 0.1, "{n:?}");
        }
    }
}
