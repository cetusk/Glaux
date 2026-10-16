//! 曲の中で学ぶ音色の型で候補の音符を説明し、同時に音色のまとまり(トラック)へ振り分ける(曲から起こす機能。調査文書 13.23 節)。
//!
//! 譜起こし(basic-pitch)が緩く出した候補の音符を、「型」の組み合わせで観測(対数周波数のスペクトログラム)を説明する
//! 分解で決め直す。型は曲の中の音色の数だけ持ち、曲の中で学ぶ(事前に学んだ楽器の辞書は使わない)。
//!
//! 型 k は次の 3 つでできている:
//! - 倍音の並び `e_k[h]`(基音からの比で決まる。全音高で共有し、対数周波数の上でずらして使う。Vincent・Bertin・Badeau 2010、
//!   Ewert & Müller 2012)と、絶対の周波数で決まるフィルタ `F_k(周波数)`(音源・フィルタの形。Virtanen & Klapuri 2006)
//! - 立ち上がりからの時間の形 `E_k[τ]`(減衰・ディレイのこだま・揺れを含む。NMFD の時間の型、Smaragdis 2004)と、
//!   音を離した後の形 `R_k[r]`(リリース・こだま。Cheng ら 2016 の立ち上がり・減衰の型を、形を決めずに学ぶ形)
//!
//! 候補の音符 n は、どれか 1 つの型 z_n と、大きさ γ_n(1 つの数)だけを持つ。音符の中で大きさを自由に動かせないので、
//! ほかの音符の倍音・こだまに当たる位置の候補は説明する分が残らず、外しても説明が悪くならない(= 余分と分かる)。
//!
//! 決めること:
//! - どの型が各音符を説明するか(z_n): 型を固定して、音符ごとに説明がいちばん良くなる型に付け替える(分類 EM)
//! - 型の数 K: 1 から始め、型を 1 つずつ割って(音符の観測した倍音の並び・減衰の主成分で 2 つに分ける。Boley 1998)増やす。
//!   K ごとの説明の悪さを残し、型の数との釣り合い(`Fit::select` の重み)で曲ごとに選ぶ
//! - 残す音符: その音符を外したときに説明がどれだけ悪くなるか(β = 1、KL ダイバージェンス)
//!
//! 固定のしきい値で音符・型を選ばず、全体で決める重みは「型を 1 つ増やす重み」と「音符を 1 つ残す重み」の 2 つだけ
//! (評価では 1 曲を抜いた交差検証で決める)。
//!
//! オフライン処理(UI・MCP のスレッドで呼ぶ)。

use rustfft::{num_complex::Complex, FftPlanner};

/// 候補の音符・振り分けた音符
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Note {
    pub start: f64,
    pub end: f64,
    pub pitch: u8,
    pub vel: u8,
}

/// 分解の設定
#[derive(Clone, Copy, Debug)]
pub struct FitOptions {
    /// 型の数の上限
    pub max_sources: usize,
    /// 1 回の当てはめの更新の回数
    pub iterations: usize,
    /// 絶対の周波数で決まるフィルタも学ぶ(false なら倍音の並びだけ)
    pub filter: bool,
    /// 音程の無い音(ドラムの漏れ・雑音)を、なめらかな広い帯域の成分で受ける
    pub background: bool,
}

impl Default for FitOptions {
    fn default() -> Self {
        FitOptions {
            max_sources: 8,
            iterations: 20,
            filter: true,
            background: true,
        }
    }
}

/// 型の数 1 つ分の結果
#[derive(Clone, Debug)]
pub struct Level {
    /// 型の数
    pub k: usize,
    /// 説明の悪さ(KL ダイバージェンス、1 フレームあたり)
    pub divergence: f64,
    /// 候補の音符ごとの型の番号(音域の外で扱わなかった音符は `usize::MAX`)
    pub source: Vec<usize>,
    /// 候補の音符ごとの、外したときの説明の悪くなり方(KL ダイバージェンスの増え方)
    pub gain: Vec<f32>,
}

/// 分解の結果(型の数ごと)
#[derive(Clone, Debug)]
pub struct Fit {
    pub levels: Vec<Level>,
    /// 時間の刻みの数(フレーム)
    pub frames: usize,
}

impl Fit {
    /// 型の数を「説明の悪さ + 型を 1 つ増やす重み × 型の数」がいちばん小さいもので選び、
    /// 外したときの悪くなり方が `note_cost` 以上の音符を、型ごとのまとまりで返す(空のまとまりは返さない)
    pub fn select(&self, cands: &[Note], source_cost: f64, note_cost: f32) -> Vec<Vec<Note>> {
        let Some(level) = self.levels.iter().min_by(|a, b| {
            (a.divergence + source_cost * a.k as f64)
                .total_cmp(&(b.divergence + source_cost * b.k as f64))
        }) else {
            return Vec::new();
        };
        let mut groups = vec![Vec::new(); level.k];
        for (i, c) in cands.iter().enumerate() {
            let z = level.source[i];
            if z < level.k && level.gain[i] >= note_cost {
                groups[z].push(*c);
            }
        }
        groups.retain(|g| !g.is_empty());
        groups
    }
}

/// 対数周波数の刻み(1 半音あたり)
const BINS_PER_SEMI: usize = 3;
/// いちばん低い音(MIDI 番号)と、高い音
const LOW_MIDI: u8 = 24;
const HIGH_MIDI: u8 = 108;
/// 倍音の数
const HARMONICS: usize = 20;
/// 倍音の山の広がり(対数周波数の刻みの数、標準偏差)
const PARTIAL_SIGMA: f32 = 1.0;
/// 時間の刻み(秒)
const HOP_SECS: f64 = 0.01;
/// 立ち上がりからの時間の形の長さ(フレーム)。これより後は最後の値のまま
const ENV_LEN: usize = 200;
/// 音を離した後の形の長さ(フレーム)
const REL_LEN: usize = 60;
/// 立ち上がりの前に見る分(フレーム)。譜起こしの立ち上がりの時刻の揺れを吸収する
const PRE_ROLL: usize = 3;
/// フィルタの節の間隔(対数周波数の刻みの数)
const KNOT: usize = 6;
/// 音程の無い成分の中心の間隔と広がり(刻みの数)
const BG_STEP: usize = 12;
const BG_SIGMA: f32 = 12.0;
/// 模型の床(0 での割り算を避ける)
const EPS: f32 = 1e-3;

/// 対数周波数の振幅スペクトログラム(フレーム × 刻み、平らに並べる)。`tuning_cents` だけずらした音の高さに刻みを合わせる
fn log_spectrogram(frames: &[f32], sr: f32, tuning_cents: f64) -> (Vec<f32>, usize, usize) {
    let n = ((sr as f64 * 0.186) as usize).next_power_of_two();
    let hop = ((sr as f64 * HOP_SECS) as usize).max(1);
    let bins = (HIGH_MIDI - LOW_MIDI) as usize * BINS_PER_SEMI + 1;
    if frames.len() < n {
        return (Vec::new(), 0, bins);
    }
    let fft = FftPlanner::<f32>::new().plan_fft_forward(n);
    let hann: Vec<f32> = (0..n)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos())
        .collect();
    let bin_hz = sr as f64 / n as f64;
    // 刻みごとの中心周波数と、そこに入る FFT の番号の範囲(範囲が空なら近くの 2 つの間を補う)
    let center = |b: f64| -> f64 {
        let m = LOW_MIDI as f64 + b / BINS_PER_SEMI as f64 + tuning_cents / 100.0;
        440.0 * 2f64.powf((m - 69.0) / 12.0)
    };
    let ranges: Vec<(f64, usize, usize)> = (0..bins)
        .map(|b| {
            let f = center(b as f64);
            let lo = (center(b as f64 - 0.5) / bin_hz).ceil() as usize;
            let hi = (center(b as f64 + 0.5) / bin_hz).floor() as usize;
            (f / bin_hz, lo, hi)
        })
        .collect();
    let mut buf = vec![Complex::new(0.0f32, 0.0); n];
    let mut out = Vec::new();
    let mut t_len = 0;
    let mut center_sample = 0usize;
    while center_sample < frames.len() {
        for (i, c) in buf.iter_mut().enumerate() {
            let x = (center_sample + i)
                .checked_sub(n / 2)
                .and_then(|k| frames.get(k))
                .copied()
                .unwrap_or(0.0);
            *c = Complex::new(x * hann[i], 0.0);
        }
        fft.process(&mut buf);
        let mag = |k: usize| buf.get(k).map_or(0.0, |c| c.norm());
        out.extend(ranges.iter().map(|&(fk, lo, hi)| {
            if hi >= lo {
                (lo..=hi).map(mag).fold(0.0f32, f32::max)
            } else {
                let k = fk.floor() as usize;
                let t = (fk - k as f64) as f32;
                mag(k) * (1.0 - t) + mag(k + 1) * t
            }
        }));
        t_len += 1;
        center_sample += hop;
    }
    (out, t_len, bins)
}

/// KL ダイバージェンスの 1 つ分
fn kl(v: f32, x: f32) -> f32 {
    let x = x.max(EPS);
    if v <= 0.0 {
        x
    } else {
        v * (v / x).ln() - v + x
    }
}

/// 音の高さ 1 つ分の型の置き場所: 使う刻みの並びと、(刻みの並びの番号, 倍音の番号, 山の重み)
struct PitchLayout {
    bins: Vec<usize>,
    entries: Vec<(usize, usize, f32)>,
    /// 倍音の並びの、高い音の形の重み(低い音の形は 1 − w)
    w: f32,
}

fn pitch_layout(p: u8, bins: usize) -> PitchLayout {
    let base = (p as i32 - LOW_MIDI as i32) as f32 * BINS_PER_SEMI as f32;
    let mut raw: Vec<(usize, usize, f32)> = Vec::new();
    for h in 0..HARMONICS {
        let c = base + (BINS_PER_SEMI * 12) as f32 * ((h + 1) as f32).log2();
        let (lo, hi) = (
            (c - 3.0 * PARTIAL_SIGMA).floor() as i32,
            (c + 3.0 * PARTIAL_SIGMA).ceil() as i32,
        );
        for b in lo.max(0)..=hi.min(bins as i32 - 1) {
            let d = (b as f32 - c) / PARTIAL_SIGMA;
            raw.push((b as usize, h, (-0.5 * d * d).exp()));
        }
    }
    let mut list: Vec<usize> = raw.iter().map(|r| r.0).collect();
    list.sort_unstable();
    list.dedup();
    let entries = raw
        .into_iter()
        .map(|(b, h, g)| (list.binary_search(&b).unwrap_or(0), h, g))
        .collect();
    PitchLayout {
        bins: list,
        entries,
        w: (p - LOW_MIDI) as f32 / (HIGH_MIDI - LOW_MIDI) as f32,
    }
}

/// 候補の音符 1 つ分(フレームの単位)
#[derive(Clone, Copy)]
struct Event {
    /// 型の時間の形の始まり(立ち上がりの PRE_ROLL 前)
    onset: usize,
    /// 始まりから音を離すまで
    dur: usize,
    /// 始まりから模型が音を出す長さ(音を離した後の形まで。曲の終わりで切る)
    len: usize,
    /// 音の高さの番号(`Model::layouts`)
    pitch: usize,
    /// 音の高さ(MIDI 番号)
    midi: u8,
}

/// 型 1 つ分
#[derive(Clone)]
struct Source {
    /// 倍音の並び: 前半が低い音(LOW_MIDI)の形、後半が高い音(HIGH_MIDI)の形。間の音は 2 つを音の高さで混ぜる
    /// (1 つの楽器でも音域で倍音の並びが変わるのを、楽器の違いと取り違えないため)
    harm: [f32; 2 * HARMONICS],
    /// フィルタの節の値
    filt: Vec<f32>,
    env: Vec<f32>,
    rel: Vec<f32>,
}

impl Source {
    fn new(knots: usize) -> Source {
        let mut harm = [0f32; 2 * HARMONICS];
        for (h, x) in harm.iter_mut().enumerate() {
            *x = 1.0 / (h % HARMONICS + 1) as f32;
        }
        Source {
            harm,
            filt: vec![1.0; knots],
            env: (0..ENV_LEN).map(|t| (-(t as f32) / 100.0).exp()).collect(),
            rel: (0..REL_LEN).map(|t| (-(t as f32) / 10.0).exp()).collect(),
        }
    }

    /// 始まりから τ フレーム目の大きさ(音を離すまで dur フレーム)
    fn shape(&self, dur: usize, tau: usize) -> f32 {
        let e = self.env[tau.min(ENV_LEN - 1)];
        if tau < dur {
            e
        } else {
            e * self.rel[(tau - dur).min(REL_LEN - 1)]
        }
    }
}

/// フィルタの節の補間(刻み b の値 = 節 q の重み w0 + 節 q+1 の重み w1)
fn knot_of(b: usize) -> (usize, f32, f32) {
    let q = b / KNOT;
    let w1 = (b % KNOT) as f32 / KNOT as f32;
    (q, 1.0 - w1, w1)
}

#[derive(Clone)]
struct Model<'a> {
    v: &'a [f32],
    t_len: usize,
    bins: usize,
    layouts: &'a [PitchLayout],
    events: &'a [Event],
    opts: FitOptions,
    gamma: Vec<f32>,
    z: Vec<usize>,
    sources: Vec<Source>,
    bg_shape: &'a [Vec<f32>],
    bg_act: Vec<f32>,
}

impl<'a> Model<'a> {
    /// 型ごと・音の高さごとのスペクトル(`PitchLayout::bins` の並び)
    fn spectra(&self) -> Vec<Vec<Vec<f32>>> {
        self.sources
            .iter()
            .map(|s| {
                self.layouts
                    .iter()
                    .map(|l| {
                        let mut out = vec![0f32; l.bins.len()];
                        for &(j, h, g) in &l.entries {
                            out[j] += ((1.0 - l.w) * s.harm[h] + l.w * s.harm[HARMONICS + h]) * g;
                        }
                        if self.opts.filter {
                            for (j, &b) in l.bins.iter().enumerate() {
                                let (q, w0, w1) = knot_of(b);
                                out[j] *= s.filt[q] * w0 + s.filt[q + 1] * w1;
                            }
                        }
                        out
                    })
                    .collect()
            })
            .collect()
    }

    /// 模型(フレーム × 刻み)
    fn model(&self, spec: &[Vec<Vec<f32>>]) -> Vec<f32> {
        let bins = self.bins;
        let mut lam = vec![EPS; self.t_len * bins];
        if self.opts.background {
            let j_len = self.bg_shape.len();
            for t in 0..self.t_len {
                let row = &mut lam[t * bins..(t + 1) * bins];
                for j in 0..j_len {
                    let u = self.bg_act[t * j_len + j];
                    if u > 0.0 {
                        for (x, s) in row.iter_mut().zip(&self.bg_shape[j]) {
                            *x += u * s;
                        }
                    }
                }
            }
        }
        for (n, ev) in self.events.iter().enumerate() {
            let s = &spec[self.z[n]][ev.pitch];
            let src = &self.sources[self.z[n]];
            let l = &self.layouts[ev.pitch];
            for tau in 0..ev.len {
                let a = self.gamma[n] * src.shape(ev.dur, tau);
                if a <= 0.0 {
                    continue;
                }
                let row = (ev.onset + tau) * bins;
                for (j, &b) in l.bins.iter().enumerate() {
                    lam[row + b] += a * s[j];
                }
            }
        }
        lam
    }

    fn divergence(&self, lam: &[f32]) -> f64 {
        self.v
            .iter()
            .zip(lam)
            .map(|(&v, &x)| kl(v, x) as f64)
            .sum::<f64>()
            / self.t_len.max(1) as f64
    }

    /// 大きさ(音符の γ と、音程の無い成分の活性)の更新
    fn update_gains(&mut self) {
        let spec = self.spectra();
        let lam = self.model(&spec);
        let bins = self.bins;
        let q: Vec<f32> = self.v.iter().zip(&lam).map(|(&v, &x)| v / x).collect();
        for (n, ev) in self.events.iter().enumerate() {
            let s = &spec[self.z[n]][ev.pitch];
            let src = &self.sources[self.z[n]];
            let l = &self.layouts[ev.pitch];
            let s_sum: f32 = s.iter().sum();
            let (mut num, mut den) = (0f32, 0f32);
            for tau in 0..ev.len {
                let a = src.shape(ev.dur, tau);
                let row = (ev.onset + tau) * bins;
                let p: f32 = l.bins.iter().zip(s).map(|(&b, &w)| w * q[row + b]).sum();
                num += a * p;
                den += a * s_sum;
            }
            if den > 0.0 {
                self.gamma[n] *= num / den;
            }
        }
        if self.opts.background {
            let j_len = self.bg_shape.len();
            for t in 0..self.t_len {
                let qr = &q[t * bins..(t + 1) * bins];
                for j in 0..j_len {
                    let sh = &self.bg_shape[j];
                    let num: f32 = sh.iter().zip(qr).map(|(a, b)| a * b).sum();
                    let den: f32 = sh.iter().sum();
                    self.bg_act[t * j_len + j] *= num / den.max(1e-12);
                }
            }
        }
    }

    /// 型(倍音の並び・フィルタ・時間の形)の更新
    fn update_sources(&mut self) {
        let spec = self.spectra();
        let lam = self.model(&spec);
        let bins = self.bins;
        let q: Vec<f32> = self.v.iter().zip(&lam).map(|(&v, &x)| v / x).collect();
        let k_len = self.sources.len();
        let knots = self.sources[0].filt.len();
        let mut harm_n = vec![[0f32; 2 * HARMONICS]; k_len];
        let mut harm_d = vec![[0f32; 2 * HARMONICS]; k_len];
        let mut filt_n = vec![vec![0f32; knots]; k_len];
        let mut filt_d = vec![vec![0f32; knots]; k_len];
        let mut env_n = vec![vec![0f32; ENV_LEN]; k_len];
        let mut env_d = vec![vec![0f32; ENV_LEN]; k_len];
        let mut rel_n = vec![vec![0f32; REL_LEN]; k_len];
        let mut rel_d = vec![vec![0f32; REL_LEN]; k_len];
        let mut y = Vec::new();
        for (n, ev) in self.events.iter().enumerate() {
            let k = self.z[n];
            let g = self.gamma[n];
            if g <= 0.0 {
                continue;
            }
            let s = &spec[k][ev.pitch];
            let src = &self.sources[k];
            let l = &self.layouts[ev.pitch];
            let s_sum: f32 = s.iter().sum();
            // y[j] = Σ_τ 形(τ) q(τ, j)、a = Σ_τ 形(τ)
            y.clear();
            y.resize(l.bins.len(), 0f32);
            let mut a_sum = 0f32;
            for tau in 0..ev.len {
                let a = src.shape(ev.dur, tau);
                let row = (ev.onset + tau) * bins;
                let mut p = 0f32;
                for (j, &b) in l.bins.iter().enumerate() {
                    let qv = q[row + b];
                    p += s[j] * qv;
                    y[j] += a * qv;
                }
                a_sum += a;
                // 時間の形: 音を離すまでは env、後は env × rel
                let e = src.env[tau.min(ENV_LEN - 1)];
                if tau < ev.dur {
                    env_n[k][tau.min(ENV_LEN - 1)] += g * p;
                    env_d[k][tau.min(ENV_LEN - 1)] += g * s_sum;
                } else {
                    let r = (tau - ev.dur).min(REL_LEN - 1);
                    let rv = src.rel[r];
                    env_n[k][tau.min(ENV_LEN - 1)] += g * rv * p;
                    env_d[k][tau.min(ENV_LEN - 1)] += g * rv * s_sum;
                    rel_n[k][r] += g * e * p;
                    rel_d[k][r] += g * e * s_sum;
                }
            }
            // 倍音の並び・フィルタ
            for &(j, h, w) in &l.entries {
                let b = l.bins[j];
                let f = if self.opts.filter {
                    let (qk, w0, w1) = knot_of(b);
                    src.filt[qk] * w0 + src.filt[qk + 1] * w1
                } else {
                    1.0
                };
                harm_n[k][h] += (1.0 - l.w) * g * w * f * y[j];
                harm_d[k][h] += (1.0 - l.w) * g * w * f * a_sum;
                harm_n[k][HARMONICS + h] += l.w * g * w * f * y[j];
                harm_d[k][HARMONICS + h] += l.w * g * w * f * a_sum;
            }
            if self.opts.filter {
                // フィルタの節: 刻み b での倍音の並びの値 × 節の重み
                let mut hv = vec![0f32; l.bins.len()];
                for &(j, h, w) in &l.entries {
                    hv[j] += ((1.0 - l.w) * src.harm[h] + l.w * src.harm[HARMONICS + h]) * w;
                }
                for (j, &b) in l.bins.iter().enumerate() {
                    let (qk, w0, w1) = knot_of(b);
                    filt_n[k][qk] += g * w0 * hv[j] * y[j];
                    filt_d[k][qk] += g * w0 * hv[j] * a_sum;
                    filt_n[k][qk + 1] += g * w1 * hv[j] * y[j];
                    filt_d[k][qk + 1] += g * w1 * hv[j] * a_sum;
                }
            }
        }
        let mul = |x: &mut [f32], n: &[f32], d: &[f32]| {
            for ((x, n), d) in x.iter_mut().zip(n).zip(d) {
                if *d > 0.0 {
                    *x *= n / d;
                }
            }
        };
        for k in 0..k_len {
            let s = &mut self.sources[k];
            mul(&mut s.harm, &harm_n[k], &harm_d[k]);
            mul(&mut s.env, &env_n[k], &env_d[k]);
            mul(&mut s.rel, &rel_n[k], &rel_d[k]);
            if self.opts.filter {
                mul(&mut s.filt, &filt_n[k], &filt_d[k]);
            }
            // 大きさのそろえ: 倍音の並びの合計 1(低い音・高い音の形の平均で)・フィルタの最大 1・時間の形の最大 1。その分を音符の γ に移す
            let hs: f32 = (s.harm.iter().sum::<f32>() / 2.0).max(1e-12);
            s.harm.iter_mut().for_each(|x| *x /= hs);
            let fm = s.filt.iter().copied().fold(0f32, f32::max).max(1e-12);
            s.filt.iter_mut().for_each(|x| *x /= fm);
            let em = s.env.iter().copied().fold(0f32, f32::max).max(1e-12);
            s.env.iter_mut().for_each(|x| *x /= em);
            let rm = s.rel.iter().copied().fold(0f32, f32::max).max(1e-12);
            s.rel.iter_mut().for_each(|x| *x = (*x / rm).min(1.0));
            let scale = hs * fm * em;
            for n in 0..self.events.len() {
                if self.z[n] == k {
                    self.gamma[n] *= scale;
                }
            }
        }
    }

    fn fit(&mut self, iterations: usize) {
        for _ in 0..iterations {
            self.update_gains();
            self.update_sources();
        }
    }

    /// 音符 n を型 k で説明するときの、いちばん良い大きさと、その範囲の説明の悪さ。`lam` は音符 n を除いた模型
    fn best_gain(
        &self,
        n: usize,
        k: usize,
        spec: &[Vec<Vec<f32>>],
        lam: &[f32],
        g0: f32,
    ) -> (f32, f32) {
        let ev = self.events[n];
        let s = &spec[k][ev.pitch];
        let src = &self.sources[k];
        let l = &self.layouts[ev.pitch];
        let shape: Vec<f32> = (0..ev.len).map(|t| src.shape(ev.dur, t)).collect();
        let s_sum: f32 = s.iter().sum();
        let a_sum: f32 = shape.iter().sum();
        let mut g = g0.max(1e-3);
        for _ in 0..8 {
            let mut num = 0f32;
            for (tau, &a) in shape.iter().enumerate() {
                let row = (ev.onset + tau) * self.bins;
                for (j, &b) in l.bins.iter().enumerate() {
                    let phi = a * s[j];
                    num += phi * self.v[row + b] / (lam[row + b] + g * phi);
                }
            }
            g *= num / (a_sum * s_sum).max(1e-12);
        }
        let mut d = 0f32;
        for (tau, &a) in shape.iter().enumerate() {
            let row = (ev.onset + tau) * self.bins;
            for (j, &b) in l.bins.iter().enumerate() {
                d += kl(self.v[row + b], lam[row + b] + g * a * s[j]);
            }
        }
        (g, d)
    }

    /// 音符 n の今の分を模型に足す(sign = 1)・引く(sign = -1)
    fn add_note(&self, lam: &mut [f32], spec: &[Vec<Vec<f32>>], n: usize, sign: f32) {
        let ev = self.events[n];
        let s = &spec[self.z[n]][ev.pitch];
        let src = &self.sources[self.z[n]];
        let l = &self.layouts[ev.pitch];
        for tau in 0..ev.len {
            let a = sign * self.gamma[n] * src.shape(ev.dur, tau);
            let row = (ev.onset + tau) * self.bins;
            for (j, &b) in l.bins.iter().enumerate() {
                lam[row + b] = (lam[row + b] + a * s[j]).max(EPS);
            }
        }
    }

    /// 音符ごとに、説明がいちばん良くなる型に付け替える(順に。付け替えた分はすぐ模型に反映する)。付け替えた数を返す
    fn reassign(&mut self) -> usize {
        let spec = self.spectra();
        let mut lam = self.model(&spec);
        let mut moved = 0;
        for n in 0..self.events.len() {
            self.add_note(&mut lam, &spec, n, -1.0);
            let mut best = (f32::MAX, self.z[n], self.gamma[n]);
            for k in 0..self.sources.len() {
                let (g, d) = self.best_gain(n, k, &spec, &lam, self.gamma[n]);
                if d < best.0 {
                    best = (d, k, g);
                }
            }
            if best.1 != self.z[n] {
                moved += 1;
            }
            self.z[n] = best.1;
            self.gamma[n] = best.2;
            self.add_note(&mut lam, &spec, n, 1.0);
        }
        moved
    }

    /// 音符ごとの、外したときの説明の悪くなり方
    fn gains(&self) -> Vec<f32> {
        let spec = self.spectra();
        let lam = self.model(&spec);
        (0..self.events.len())
            .map(|n| {
                let ev = self.events[n];
                let s = &spec[self.z[n]][ev.pitch];
                let src = &self.sources[self.z[n]];
                let l = &self.layouts[ev.pitch];
                let mut d = 0f32;
                for tau in 0..ev.len {
                    let a = self.gamma[n] * src.shape(ev.dur, tau);
                    let row = (ev.onset + tau) * self.bins;
                    for (j, &b) in l.bins.iter().enumerate() {
                        let x = lam[row + b];
                        let v = self.v[row + b];
                        d += kl(v, x - a * s[j]) - kl(v, x);
                    }
                }
                d
            })
            .collect()
    }

    /// 型 k の音符を、観測した倍音の並び・減衰の主成分で 2 つに分け、片方を新しい型にする。分けられなければ false
    fn split(&mut self, k: usize) -> bool {
        let members: Vec<usize> = (0..self.events.len()).filter(|&n| self.z[n] == k).collect();
        if members.len() < 8 {
            return false;
        }
        let feats: Vec<Vec<f32>> = members
            .iter()
            .map(|&n| note_features(self.v, self.bins, &self.events[n]))
            .collect();
        let dim = feats[0].len();
        // 次元ごとに、音の高さで説明できる分(1 次式の当てはめ)を引き、ばらつきをそろえる(平均 0・分散 1)。
        // 音域による違いで割らず、音色の違いで割るため
        let nf = feats.len() as f32;
        let pitch: Vec<f32> = members
            .iter()
            .map(|&n| self.events[n].midi as f32)
            .collect();
        let pm = pitch.iter().sum::<f32>() / nf;
        let pv = pitch
            .iter()
            .map(|p| (p - pm).powi(2))
            .sum::<f32>()
            .max(1e-6);
        let mut x: Vec<Vec<f32>> = vec![vec![0f32; dim]; feats.len()];
        for i in 0..dim {
            let m = feats.iter().map(|f| f[i]).sum::<f32>() / nf;
            let slope = feats
                .iter()
                .zip(&pitch)
                .map(|(f, p)| (f[i] - m) * (p - pm))
                .sum::<f32>()
                / pv;
            for (r, (f, p)) in x.iter_mut().zip(feats.iter().zip(&pitch)) {
                r[i] = f[i] - m - slope * (p - pm);
            }
            let sd = (x.iter().map(|r| r[i] * r[i]).sum::<f32>() / nf)
                .sqrt()
                .max(1e-6);
            x.iter_mut().for_each(|r| r[i] /= sd);
        }
        // 第 1 主成分(べき乗法)
        let mut w = vec![1f32; dim];
        for _ in 0..50 {
            let mut nw = vec![0f32; dim];
            for r in &x {
                let p: f32 = r.iter().zip(&w).map(|(a, b)| a * b).sum();
                for i in 0..dim {
                    nw[i] += p * r[i];
                }
            }
            let norm = nw.iter().map(|a| a * a).sum::<f32>().sqrt().max(1e-12);
            w = nw.into_iter().map(|a| a / norm).collect();
        }
        let side: Vec<bool> = x
            .iter()
            .map(|r| r.iter().zip(&w).map(|(a, b)| a * b).sum::<f32>() > 0.0)
            .collect();
        let ones = side.iter().filter(|s| **s).count();
        if ones == 0 || ones == side.len() {
            return false;
        }
        let new_k = self.sources.len();
        self.sources.push(self.sources[k].clone());
        for (i, &n) in members.iter().enumerate() {
            if side[i] {
                self.z[n] = new_k;
            }
        }
        true
    }
}

/// 型を割るときの音符の特徴: 観測した倍音の山の大きさ(対数、倍音の間の平均を引く)・立ち上がりの後の減り方・長さ
fn note_features(v: &[f32], bins: usize, ev: &Event) -> Vec<f32> {
    const H: usize = 10;
    let p_bin = |h: usize, pitch_bin0: f32| -> Option<usize> {
        let c = pitch_bin0 + (BINS_PER_SEMI * 12) as f32 * ((h + 1) as f32).log2();
        let b = c.round() as usize;
        (b < bins).then_some(b)
    };
    let pitch_bin0 = (ev.midi - LOW_MIDI) as f32 * BINS_PER_SEMI as f32;
    let early = |tau: usize| tau < ev.dur.clamp(PRE_ROLL + 1, PRE_ROLL + 15);
    let mut prof = [0f32; H];
    let (mut e_early, mut e_late) = (0f32, 0f32);
    for tau in 0..ev.len {
        let row = (ev.onset + tau) * bins;
        for (h, pr) in prof.iter_mut().enumerate() {
            let Some(b) = p_bin(h, pitch_bin0) else {
                continue;
            };
            let x = v[row + b];
            if early(tau) {
                *pr += x;
                if h < 4 {
                    e_early += x;
                }
            } else if tau < PRE_ROLL + 45 && h < 4 {
                e_late += x;
            }
        }
    }
    let lp: Vec<f32> = prof.iter().map(|x| (x + 1e-2).ln()).collect();
    let m = lp.iter().sum::<f32>() / H as f32;
    let mut f: Vec<f32> = lp.iter().map(|x| x - m).collect();
    f.push(((e_late + 1e-2) / (e_early + 1e-2)).ln());
    f.push((ev.dur as f32).ln());
    f
}

/// 候補の音符を、曲の中で学ぶ音色の型で説明し、型の数 1〜`max_sources` のそれぞれで振り分ける。
/// `audio` は分離ツールで分けた音の 1 つ(か曲全体)
pub fn fit(audio: &[f32], sr: f32, tuning_cents: f64, cands: &[Note], opts: &FitOptions) -> Fit {
    let (mut v, t_len, bins) = log_spectrogram(audio, sr, tuning_cents);
    if t_len == 0 || cands.is_empty() {
        return Fit {
            levels: Vec::new(),
            frames: t_len,
        };
    }
    // 大きさをそろえる(1 フレームあたりの合計の平均を 1 に)。重みを曲の音量に左右されないようにする
    let mean = v.iter().sum::<f32>() / t_len as f32;
    v.iter_mut().for_each(|x| *x /= mean.max(1e-12));
    let frame = |t: f64| ((t / HOP_SECS).round().max(0.0) as usize).min(t_len - 1);
    // 音の高さの並びと、扱う音符
    let mut pitches: Vec<u8> = cands
        .iter()
        .map(|c| c.pitch)
        .filter(|p| (LOW_MIDI..=HIGH_MIDI).contains(p))
        .collect();
    pitches.sort_unstable();
    pitches.dedup();
    let layouts: Vec<PitchLayout> = pitches.iter().map(|&p| pitch_layout(p, bins)).collect();
    let mut index = Vec::new();
    let events: Vec<Event> = cands
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let pi = pitches.binary_search(&c.pitch).ok()?;
            let onset = frame(c.start).saturating_sub(PRE_ROLL);
            let dur = (frame(c.end).max(frame(c.start) + 1)) - onset;
            let len = (dur + REL_LEN).min(t_len - onset);
            index.push(i);
            Some(Event {
                onset,
                dur,
                len,
                pitch: pi,
                midi: c.pitch,
            })
        })
        .collect();
    let bg_shape: Vec<Vec<f32>> = (0..bins.div_ceil(BG_STEP) + 1)
        .map(|j| {
            let c = (j * BG_STEP) as f32;
            (0..bins)
                .map(|b| (-0.5 * ((b as f32 - c) / BG_SIGMA).powi(2)).exp())
                .collect()
        })
        .collect();
    let knots = bins / KNOT + 2;
    let mut m = Model {
        v: &v,
        t_len,
        bins,
        layouts: &layouts,
        events: &events,
        opts: *opts,
        gamma: vec![1.0; events.len()],
        z: vec![0; events.len()],
        sources: vec![Source::new(knots)],
        bg_shape: &bg_shape,
        bg_act: vec![0.1; t_len * bg_shape.len()],
    };
    let mut levels = Vec::new();
    let record = |m: &Model, levels: &mut Vec<Level>| {
        let spec = m.spectra();
        let lam = m.model(&spec);
        let gains = m.gains();
        let mut source = vec![usize::MAX; cands.len()];
        let mut gain = vec![0f32; cands.len()];
        for (e, &i) in index.iter().enumerate() {
            source[i] = m.z[e];
            gain[i] = gains[e];
        }
        levels.push(Level {
            k: m.sources.len(),
            divergence: m.divergence(&lam),
            source,
            gain,
        });
    };
    m.fit(opts.iterations * 2);
    record(&m, &mut levels);
    while m.sources.len() < opts.max_sources {
        // 型ごとに割ってみて、説明がいちばん良くなる割り方を選ぶ
        let mut best: Option<(f64, Model)> = None;
        for k in 0..m.sources.len() {
            let mut trial = m.clone();
            if !trial.split(k) {
                continue;
            }
            trial.fit(opts.iterations / 4 + 1);
            trial.reassign();
            trial.fit(opts.iterations / 4 + 1);
            let d = trial.divergence(&trial.model(&trial.spectra()));
            if best.as_ref().is_none_or(|b| d < b.0) {
                best = Some((d, trial));
            }
        }
        let Some((_, next)) = best else {
            break;
        };
        m = next;
        for _ in 0..3 {
            m.fit(opts.iterations / 2);
            if m.reassign() == 0 {
                break;
            }
        }
        m.fit(opts.iterations / 2);
        // 空になった型は除く
        let used: Vec<bool> = (0..m.sources.len()).map(|k| m.z.contains(&k)).collect();
        if used.iter().any(|u| !u) {
            let map: Vec<usize> = used
                .iter()
                .scan(0, |c, &u| {
                    let r = *c;
                    if u {
                        *c += 1;
                    }
                    Some(r)
                })
                .collect();
            m.sources = m
                .sources
                .iter()
                .zip(&used)
                .filter(|(_, u)| **u)
                .map(|(s, _)| s.clone())
                .collect();
            m.z.iter_mut().for_each(|z| *z = map[*z]);
        }
        record(&m, &mut levels);
    }
    Fit {
        levels,
        frames: t_len,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 倍音が `amp(k)` で並ぶ音を、指定の区間に足す
    fn add_tone(
        out: &mut [f32],
        sr: f32,
        midi: f32,
        start: f32,
        end: f32,
        decay: f32,
        amp: impl Fn(usize) -> f32,
    ) {
        let f0 = 440.0 * 2f32.powf((midi - 69.0) / 12.0);
        let (a, b) = ((start * sr) as usize, (end * sr) as usize);
        for (i, x) in out.iter_mut().enumerate().take(b).skip(a) {
            let t = (i - a) as f32 / sr;
            let env = (-t / decay).exp() * 0.2;
            *x += env
                * (1..=30)
                    .filter(|k| (*k as f32) * f0 < sr / 2.0)
                    .map(|k| (std::f32::consts::TAU * k as f32 * f0 * t).sin() * amp(k))
                    .sum::<f32>();
        }
    }

    #[test]
    fn overtone_candidates_lose_their_gain() {
        // A3(57)の鋸歯状波を何度か鳴らす。候補には、基音と第 2 倍音(+12)の位置の余分な音符
        let sr = 16_000.0;
        let mut x = vec![0f32; (sr * 4.0) as usize];
        let mut cands = Vec::new();
        for i in 0..6 {
            let s = 0.1 + i as f32 * 0.6;
            let p = [57.0, 60.0, 64.0][i % 3];
            add_tone(&mut x, sr, p, s, s + 0.4, 1.0, |k| 1.0 / k as f32);
            for d in [0u8, 12] {
                cands.push(Note {
                    start: s as f64,
                    end: (s + 0.4) as f64,
                    pitch: p as u8 + d,
                    vel: 100,
                });
            }
        }
        let opts = FitOptions {
            max_sources: 1,
            ..Default::default()
        };
        let f = fit(&x, sr, 0.0, &cands, &opts);
        let g = &f.levels[0].gain;
        let (fund, over): (Vec<f32>, Vec<f32>) = (
            g.iter().step_by(2).copied().collect(),
            g.iter().skip(1).step_by(2).copied().collect(),
        );
        let min_f = fund.iter().copied().fold(f32::MAX, f32::min);
        let max_o = over.iter().copied().fold(0f32, f32::max);
        assert!(min_f > 10.0 * max_o, "基音 {fund:?} 倍音 {over:?}");
    }

    #[test]
    fn two_timbres_are_told_apart() {
        // 倍音の並びの違う 2 つの音色(鋸歯状波と、奇数倍音だけの矩形波に近い音)を交互に鳴らす
        let sr = 16_000.0;
        let mut x = vec![0f32; (sr * 6.0) as usize];
        let mut cands = Vec::new();
        let mut truth = Vec::new();
        for i in 0..16 {
            let s = 0.1 + i as f32 * 0.35;
            let p = [57.0, 62.0, 65.0, 69.0][i % 4];
            let square = i % 2 == 1;
            add_tone(
                &mut x,
                sr,
                p,
                s,
                s + 0.3,
                if square { 0.08 } else { 1.0 },
                |k| {
                    if square {
                        if k % 2 == 1 {
                            1.0 / k as f32
                        } else {
                            0.0
                        }
                    } else {
                        1.0 / k as f32
                    }
                },
            );
            cands.push(Note {
                start: s as f64,
                end: (s + 0.3) as f64,
                pitch: p as u8,
                vel: 100,
            });
            truth.push(square);
        }
        let opts = FitOptions {
            max_sources: 2,
            ..Default::default()
        };
        let f = fit(&x, sr, 0.0, &cands, &opts);
        let lv = f.levels.iter().find(|l| l.k == 2).expect("2 つの型");
        let a = lv.source[0];
        let agree = truth
            .iter()
            .zip(&lv.source)
            .filter(|(sq, z)| (**z == a) != **sq)
            .count();
        assert_eq!(agree, truth.len(), "{:?}", lv.source);
    }
}
