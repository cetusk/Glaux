//! 起こした音符を、音符の並び方の手がかりで音源(トラック)へ振り分ける(曲から起こす機能の試作。調査文書 13.24 節)。
//!
//! 静的な音色の形だけでは、分離ツールの「その他」の中の楽器を見分けられなかった(13.23 節)。そこで、
//! 1 つの音源の音符が持つ「並び方」の性質を、曲ごとに学ぶ確率モデル(マルコフ確率場)で振り分ける:
//!
//! - 音符ごとの項(音源ごとの分布、曲ごとに学ぶ): 音の高さ(正規分布)・長さ(対数の正規分布)・小節の中の位置
//!   (16 分の位置の分布)・曲の中で鳴っている区間(区間ごとの分布。電子音楽のトラックは区間で出入りする)・
//!   どの分けた音から来たか(ベース・その他)
//! - 並びの項(同じ音源の中のつながり): 前の音からの音程の飛び(ラプラス分布。McLeod & Steedman 2016 の声部の分け方の
//!   考え)と、同時に鳴る音の有無(音源ごとの確率 ρ_k。和音を弾く音源・単音の音源を曲ごとに学ぶ。
//!   「同時に鳴る音は別の音源」という決め打ちの制約の代わり)
//! - ごみの音源: 候補の音符のうちの余分(倍音・こだま・漏れ)を受ける。分解(`notefit`)の「外したときの説明の
//!   悪くなり方」を観測にして、本物の音源とごみで別の分布を学ぶ(固定のしきい値で落とさない)
//!
//! 振り分けは ICM(音符を 1 つずつ、全体の尤度がいちばん上がる音源へ移す)と、分布の学び直しを交互に繰り返す
//! (分類 EM)。音源の数 K は 1〜上限で解き、BIC でいちばん良いものを選ぶ。
//!
//! オフライン処理(UI・MCP のスレッドで呼ぶ)。

use std::collections::BTreeSet;

/// 振り分ける音符
#[derive(Clone, Copy, Debug)]
pub struct StreamNote {
    pub start: f64,
    pub end: f64,
    pub pitch: u8,
    /// どの分けた音から来たか(0 = ベース、1 = その他 など)
    pub stem: usize,
    /// 外したときの説明の悪くなり方(`notefit` の gain)。無ければ None
    pub gain: Option<f32>,
}

/// 振り分けの設定
#[derive(Clone, Copy, Debug)]
pub struct StreamOptions {
    /// 音源の数の上限
    pub max_sources: usize,
    /// 振り分けと学び直しを繰り返す回数
    pub iterations: usize,
    /// 並びの項の重み(0 なら音符ごとの項だけ)
    pub sequence_weight: f64,
    /// 鳴っている区間の項の重み
    pub activity_weight: f64,
    /// ごみの音源を使う
    pub garbage: bool,
}

impl Default for StreamOptions {
    fn default() -> Self {
        StreamOptions {
            max_sources: 8,
            iterations: 12,
            sequence_weight: 1.0,
            activity_weight: 1.0,
            garbage: true,
        }
    }
}

/// 音源の数 1 つ分の結果
#[derive(Clone, Debug)]
pub struct StreamLevel {
    pub k: usize,
    /// 対数尤度(重みを掛けた全体)
    pub loglik: f64,
    /// 学んだ値の数(BIC 用)
    pub params: usize,
    /// 音符ごとの音源(ごみは None)
    pub source: Vec<Option<usize>>,
}

impl StreamLevel {
    /// BIC(小さいほど良い)
    pub fn bic(&self, n: usize) -> f64 {
        -2.0 * self.loglik + self.params as f64 * (n.max(2) as f64).ln()
    }
}

/// 同時とみなす頭のずれ(秒)
const SIMUL: f64 = 0.03;
/// 前の音を探す範囲(秒)。これより前に同じ音源の音が無ければ「最初の音」
const PRED_WINDOW: f64 = 2.0;
/// 1 小節の中の位置の数(16 分)
const POS_PER_BEAT: usize = 4;
/// 鳴っている区間の長さ(秒)
const BLOCK_SECS: f64 = 8.0;
/// 同時に鳴っているかを見るとき、さかのぼる音符の数の上限
const OVERLAP_LOOKBACK: usize = 64;

/// 音符ごとの観測(前もって求める)
#[derive(Clone)]
struct Feat {
    pitch: f64,
    log_dur: f64,
    pos: usize,
    block: usize,
    stem: usize,
    /// 外したときの悪くなり方の変換(asinh)
    gain: Option<f64>,
}

/// 音源 1 つの分布
#[derive(Clone, Debug)]
struct Source {
    log_pi: f64,
    mu_p: f64,
    var_p: f64,
    mu_d: f64,
    var_d: f64,
    pos: Vec<f64>,
    block: Vec<f64>,
    stem: Vec<f64>,
    /// 同時にほかの音が鳴っている確率
    rho: f64,
    /// 音程の飛びのラプラス分布の幅
    jump_b: f64,
    /// 前の音が無い(最初の音)確率
    first: f64,
}

/// 正規分布の対数密度
fn ln_normal(x: f64, mu: f64, var: f64) -> f64 {
    -0.5 * ((x - mu).powi(2) / var + (std::f64::consts::TAU * var).ln())
}

/// 整数の上のラプラス分布の対数確率
fn ln_laplace(d: f64, b: f64) -> f64 {
    let q = (-1.0 / b).exp();
    ((1.0 - q) / (1.0 + q)).ln() - d.abs() / b
}

struct Model<'a> {
    notes: &'a [StreamNote],
    feat: Vec<Feat>,
    n_pos: usize,
    n_block: usize,
    n_stem: usize,
    opts: StreamOptions,
    /// 音源(最後の 1 つがごみ。ごみは小節の中の位置・鳴っている区間が一様)
    src: Vec<Source>,
    /// 外したときの悪くなり方の分布(本物・ごみ)
    gain_real: (f64, f64),
    gain_garbage: (f64, f64),
    /// 音源ごとの音符(音符の番号は頭の順。最後がごみ)
    members: Vec<BTreeSet<usize>>,
    /// 音符ごとの音源(ごみは None)
    z: Vec<Option<usize>>,
}

impl<'a> Model<'a> {
    /// 本物の音源の数
    fn k(&self) -> usize {
        self.src.len() - 1
    }

    /// 音源の番号(ごみは最後)
    fn idx(&self, k: Option<usize>) -> usize {
        k.unwrap_or(self.src.len() - 1)
    }

    /// 音符ごとの項(音の高さは並びの項で扱う)
    fn unary(&self, n: usize, k: Option<usize>) -> f64 {
        let f = &self.feat[n];
        let s = &self.src[self.idx(k)];
        let mut e = s.log_pi + ln_normal(f.log_dur, s.mu_d, s.var_d) + s.stem[f.stem].ln();
        match k {
            Some(_) => {
                e += s.pos[f.pos].ln() + self.opts.activity_weight * s.block[f.block].ln();
            }
            None => {
                e -= (self.n_pos as f64).ln()
                    + self.opts.activity_weight * (self.n_block as f64).ln();
            }
        }
        if let Some(g) = f.gain {
            let (mu, var) = if k.is_some() {
                self.gain_real
            } else {
                self.gain_garbage
            };
            e += ln_normal(g, mu, var);
        }
        e
    }

    /// 音源 i の中の音符 x の並びの項(x が i に入っているとして)。(同時に鳴っているか, 前の音からの飛び(無ければ None))
    fn context(&self, i: usize, x: usize) -> (bool, Option<f64>) {
        let nx = &self.notes[x];
        let set = &self.members[i];
        let mut overlap = false;
        // 同時に始まる、後ろの音
        for &y in set.range(x + 1..) {
            if self.notes[y].start > nx.start + SIMUL {
                break;
            }
            overlap = true;
        }
        let mut pred: Option<f64> = None;
        let mut pred_start = f64::NEG_INFINITY;
        for (seen, &y) in set.range(..x).rev().enumerate() {
            let ny = &self.notes[y];
            if ny.start <= nx.start + SIMUL && ny.end > nx.start + SIMUL {
                overlap = true;
            }
            let age = nx.start - ny.start;
            if age > SIMUL && age <= PRED_WINDOW {
                // いちばん新しい頭の組の中で、いちばん近い音程
                if pred.is_none() {
                    pred_start = ny.start;
                }
                if ny.start >= pred_start - SIMUL {
                    let d = nx.pitch as f64 - ny.pitch as f64;
                    if pred.is_none_or(|p| d.abs() < p.abs()) {
                        pred = Some(d);
                    }
                }
            }
            let pred_done = ny.start < pred_start - SIMUL || age > PRED_WINDOW;
            if pred_done && (overlap || seen >= OVERLAP_LOOKBACK) {
                break;
            }
        }
        (overlap, pred)
    }

    /// 並びの項: 音の高さ(前の音があれば飛びの分布、無ければ音源の高さの分布)と、同時に鳴っているか。
    /// 重み w で、音の高さの分布だけの模型(w = 0)と並びの模型(w = 1)の間をとる
    fn seq(&self, i: usize, x: usize) -> f64 {
        let s = &self.src[i];
        let w = self.opts.sequence_weight;
        let pitch = self.feat[x].pitch;
        let static_p = ln_normal(pitch, s.mu_p, s.var_p);
        if w == 0.0 {
            return static_p;
        }
        let (overlap, pred) = self.context(i, x);
        let mut e = if overlap {
            s.rho.ln()
        } else {
            (1.0 - s.rho).ln()
        };
        e += match pred {
            Some(d) => (1.0 - s.first).ln() + ln_laplace(d, s.jump_b),
            None => s.first.ln() + static_p,
        };
        (1.0 - w) * static_p + w * e
    }

    /// 音符 n を音源 i に出し入れしたときに、並びの項が変わる音符(n を除く)
    fn affected(&self, i: usize, n: usize) -> Vec<usize> {
        if self.opts.sequence_weight == 0.0 {
            return Vec::new();
        }
        let nn = &self.notes[n];
        let set = &self.members[i];
        let mut out = Vec::new();
        // 同時に始まった前の音(同時に鳴っているかが変わる)
        for &y in set.range(..n).rev() {
            if self.notes[y].start < nn.start - SIMUL {
                break;
            }
            out.push(y);
        }
        // n が鳴っている間に始まる音と、n の次の頭の組(前の音が変わる)
        let mut next_group: Option<f64> = None;
        for &y in set.range(n + 1..) {
            let ny = &self.notes[y];
            let in_sound = ny.start < nn.end - SIMUL;
            let after = ny.start > nn.start + SIMUL;
            let in_next = if after {
                let g = *next_group.get_or_insert(ny.start);
                ny.start <= g + SIMUL
            } else {
                false
            };
            if !(in_sound || in_next || !after) {
                break;
            }
            out.push(y);
        }
        out
    }

    fn local_seq(&self, i: usize, xs: &[usize]) -> f64 {
        xs.iter().map(|&x| self.seq(i, x)).sum()
    }

    /// 音符 n を音源 to へ移したときの尤度の変化(移さずに求める)
    fn move_gain(&mut self, n: usize, to: Option<usize>) -> f64 {
        let from = self.z[n];
        if from == to {
            return 0.0;
        }
        let (fi, ti) = (self.idx(from), self.idx(to));
        let aff_from = self.affected(fi, n);
        let aff_to = self.affected(ti, n);
        let before = self.unary(n, from)
            + self.seq(fi, n)
            + self.local_seq(fi, &aff_from)
            + self.local_seq(ti, &aff_to);
        self.set(n, to);
        let after = self.unary(n, to)
            + self.seq(ti, n)
            + self.local_seq(fi, &aff_from)
            + self.local_seq(ti, &aff_to);
        self.set(n, from);
        after - before
    }

    fn set(&mut self, n: usize, k: Option<usize>) {
        let old = self.idx(self.z[n]);
        self.members[old].remove(&n);
        let new = self.idx(k);
        self.members[new].insert(n);
        self.z[n] = k;
    }

    /// 振り分けから分布を学び直す
    fn m_step(&mut self) {
        let n = self.notes.len() as f64;
        let all = self.src.len();
        // 分散は小さい数で 0 にならないよう、floor の 4 倍の事前の分を足す
        let fit_gauss = |xs: &[f64], floor: f64| -> (f64, f64) {
            if xs.is_empty() {
                return (0.0, 1e6);
            }
            let mu = xs.iter().sum::<f64>() / xs.len() as f64;
            let ss = xs.iter().map(|x| (x - mu).powi(2)).sum::<f64>();
            (
                mu,
                ((ss + 4.0 * floor) / (xs.len() as f64 + 1.0)).max(floor),
            )
        };
        let cat = |idx: &mut dyn Iterator<Item = usize>, size: usize, alpha: f64| -> Vec<f64> {
            let mut c = vec![alpha; size];
            for i in idx {
                c[i] += 1.0;
            }
            let z: f64 = c.iter().sum();
            c.iter().map(|x| x / z).collect()
        };
        for i in 0..all {
            let ids: Vec<usize> = self.members[i].iter().copied().collect();
            let m = ids.len() as f64;
            let s = &mut self.src[i];
            s.log_pi = ((m + 1.0) / (n + all as f64)).ln();
            let p: Vec<f64> = ids.iter().map(|&i| self.feat[i].pitch).collect();
            (s.mu_p, s.var_p) = fit_gauss(&p, 1.0);
            let d: Vec<f64> = ids.iter().map(|&i| self.feat[i].log_dur).collect();
            (s.mu_d, s.var_d) = fit_gauss(&d, 0.05);
            s.pos = cat(&mut ids.iter().map(|&i| self.feat[i].pos), self.n_pos, 0.5);
            s.block = cat(
                &mut ids.iter().map(|&i| self.feat[i].block),
                self.n_block,
                0.5,
            );
            s.stem = cat(
                &mut ids.iter().map(|&i| self.feat[i].stem),
                self.n_stem,
                0.5,
            );
        }
        // 並びの項(振り分けだけから数える)
        if self.opts.sequence_weight > 0.0 {
            for i in 0..all {
                let ids: Vec<usize> = self.members[i].iter().copied().collect();
                let (mut over, mut firsts, mut jumps) = (0.0, 0.0, Vec::new());
                for &x in &ids {
                    let (o, p) = self.context(i, x);
                    over += o as u8 as f64;
                    match p {
                        Some(d) => jumps.push(d.abs()),
                        None => firsts += 1.0,
                    }
                }
                let m = ids.len() as f64;
                let s = &mut self.src[i];
                s.rho = (over + 0.5) / (m + 1.0);
                s.first = (firsts + 0.5) / (m + 1.0);
                s.jump_b =
                    ((jumps.iter().sum::<f64>() + 1.0) / (jumps.len() as f64 + 1.0)).max(0.3);
            }
        }
        let gains = |real: bool| -> Vec<f64> {
            (0..self.notes.len())
                .filter(|&i| self.z[i].is_some() == real)
                .filter_map(|i| self.feat[i].gain)
                .collect()
        };
        self.gain_real = fit_gauss(&gains(true), 0.05);
        self.gain_garbage = fit_gauss(&gains(false), 0.05);
    }

    /// 全体の対数尤度
    fn loglik(&self) -> f64 {
        (0..self.notes.len())
            .map(|n| self.unary(n, self.z[n]) + self.seq(self.idx(self.z[n]), n))
            .sum()
    }

    /// ICM を 1 回(音符を頭の順に、いちばん尤度が上がる音源へ)。動いた数
    fn sweep(&mut self) -> usize {
        let mut moved = 0;
        let k_count = self.k();
        for n in 0..self.notes.len() {
            let mut best = (0.0, self.z[n]);
            let options = (0..k_count)
                .map(Some)
                .chain(self.opts.garbage.then_some(None));
            for to in options {
                if to == self.z[n] {
                    continue;
                }
                let g = self.move_gain(n, to);
                if g > best.0 + 1e-9 {
                    best = (g, to);
                }
            }
            if best.1 != self.z[n] {
                self.set(n, best.1);
                moved += 1;
            }
        }
        moved
    }
}

/// 音符を音源へ振り分ける。`beats`・`downbeats` は曲の拍・小節頭の時刻(秒)。音源の数 1〜上限の結果を返す
pub fn stream(
    notes: &[StreamNote],
    beats: &[f64],
    downbeats: &[f64],
    opts: &StreamOptions,
) -> Vec<StreamLevel> {
    if notes.is_empty() {
        return Vec::new();
    }
    // 頭の順に並べ替えて解き、最後に元の順へ戻す
    let mut order: Vec<usize> = (0..notes.len()).collect();
    order.sort_by(|&a, &b| notes[a].start.total_cmp(&notes[b].start));
    let sorted: Vec<StreamNote> = order.iter().map(|&i| notes[i]).collect();
    // 小節の中の位置: 小節頭からの拍の数(拍の時刻で内挿)× 4 を丸める
    let beats_per_bar = {
        let mut c: Vec<usize> = downbeats
            .windows(2)
            .map(|w| {
                beats
                    .iter()
                    .filter(|&&b| b >= w[0] - 0.02 && b < w[1] - 0.02)
                    .count()
            })
            .filter(|&c| c > 0)
            .collect();
        c.sort_unstable();
        c.get(c.len() / 2).copied().unwrap_or(4).clamp(1, 12)
    };
    let n_pos = beats_per_bar * POS_PER_BEAT;
    let beat_index = |t: f64| -> f64 {
        if beats.len() < 2 {
            return t * 2.0;
        }
        let i = beats.partition_point(|&b| b <= t).clamp(1, beats.len() - 1);
        let (a, b) = (beats[i - 1], beats[i]);
        (i - 1) as f64 + (t - a) / (b - a)
    };
    let first_down = downbeats.first().map_or(0.0, |&d| beat_index(d));
    let end = sorted.iter().map(|n| n.end).fold(0.0, f64::max);
    let n_block = ((end / BLOCK_SECS).ceil() as usize).max(1);
    let n_stem = sorted.iter().map(|n| n.stem + 1).max().unwrap_or(1);
    let feat: Vec<Feat> = sorted
        .iter()
        .map(|n| {
            let b = beat_index(n.start) - first_down;
            let pos = ((b * POS_PER_BEAT as f64).round() as i64).rem_euclid(n_pos as i64) as usize;
            Feat {
                pitch: n.pitch as f64,
                log_dur: (n.end - n.start).max(0.01).ln(),
                pos,
                block: ((n.start / BLOCK_SECS) as usize).min(n_block - 1),
                stem: n.stem,
                gain: n.gain.map(|g| (g as f64 / 0.1).asinh()),
            }
        })
        .collect();
    let blank = Source {
        log_pi: 0.0,
        mu_p: 60.0,
        var_p: 100.0,
        mu_d: -1.0,
        var_d: 1.0,
        pos: vec![1.0 / n_pos as f64; n_pos],
        block: vec![1.0 / n_block as f64; n_block],
        stem: vec![1.0 / n_stem as f64; n_stem],
        rho: 0.5,
        jump_b: 3.0,
        first: 0.1,
    };
    let mut levels = Vec::new();
    for k in 1..=opts.max_sources.max(1) {
        let mut m = Model {
            notes: &sorted,
            feat: Vec::new(),
            n_pos,
            n_block,
            n_stem,
            opts: *opts,
            src: vec![blank.clone(); k + 1],
            gain_real: (0.0, 1.0),
            gain_garbage: (0.0, 1.0),
            members: vec![BTreeSet::new(); k + 1],
            z: vec![None; sorted.len()],
        };
        m.feat = feat.clone();
        // 始めの振り分け: 外したときの悪くなり方が小さい音はごみ、残りを (音の高さ, 長さ, 分けた音) の k-means で
        let init = kmeans_init(&m.feat, k);
        for (n, &k0) in init.iter().enumerate() {
            let garbage =
                opts.garbage && m.feat[n].gain.is_some_and(|g| g < (0.3f64 / 0.1).asinh());
            m.set(n, if garbage { None } else { Some(k0) });
        }
        m.m_step();
        for _ in 0..opts.iterations {
            let moved = m.sweep();
            m.m_step();
            if moved == 0 {
                break;
            }
        }
        let per_source = 4 + (n_pos - 1) + (n_block - 1) + (n_stem - 1) + 3 + 1;
        let params = k * per_source + 8 + (n_stem - 1) + 4;
        let mut source = vec![None; notes.len()];
        for (si, &oi) in order.iter().enumerate() {
            source[oi] = m.z[si];
        }
        levels.push(StreamLevel {
            k,
            loglik: m.loglik(),
            params,
            source,
        });
    }
    levels
}

/// (音の高さ / 6, 長さの対数, 分けた音 × 2) の k-means の振り分け(遠い点から順に中心を選ぶ、決定的な初期化)
fn kmeans_init(feat: &[Feat], k: usize) -> Vec<usize> {
    let v: Vec<[f64; 3]> = feat
        .iter()
        .map(|f| [f.pitch / 6.0, f.log_dur, f.stem as f64 * 2.0])
        .collect();
    let dist =
        |a: &[f64; 3], b: &[f64; 3]| -> f64 { a.iter().zip(b).map(|(x, y)| (x - y).powi(2)).sum() };
    let mean: [f64; 3] = {
        let mut m = [0.0; 3];
        for x in &v {
            for i in 0..3 {
                m[i] += x[i] / v.len() as f64;
            }
        }
        m
    };
    let mut centers: Vec<[f64; 3]> = Vec::new();
    // 最初は平均にいちばん近い点、次からは今の中心からいちばん遠い点
    if let Some(c) = v
        .iter()
        .min_by(|a, b| dist(a, &mean).total_cmp(&dist(b, &mean)))
    {
        centers.push(*c);
    }
    while centers.len() < k {
        let far = v.iter().max_by(|a, b| {
            let da = centers.iter().map(|c| dist(a, c)).fold(f64::MAX, f64::min);
            let db = centers.iter().map(|c| dist(b, c)).fold(f64::MAX, f64::min);
            da.total_cmp(&db)
        });
        match far {
            Some(c) => centers.push(*c),
            None => break,
        }
    }
    let mut z = vec![0usize; v.len()];
    for _ in 0..20 {
        for (i, x) in v.iter().enumerate() {
            z[i] = (0..centers.len())
                .min_by(|&a, &b| dist(x, &centers[a]).total_cmp(&dist(x, &centers[b])))
                .unwrap_or(0);
        }
        for (c, center) in centers.iter_mut().enumerate() {
            let members: Vec<&[f64; 3]> = v
                .iter()
                .zip(&z)
                .filter(|(_, zz)| **zz == c)
                .map(|(x, _)| x)
                .collect();
            if members.is_empty() {
                continue;
            }
            for i in 0..3 {
                center[i] = members.iter().map(|x| x[i]).sum::<f64>() / members.len() as f64;
            }
        }
    }
    z
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 同じ音域で交互に鳴る 2 つの音源(白玉の和音と、16 分の単音のアルペジオ)を、並び方で分けられる
    #[test]
    fn chords_and_arpeggio_in_the_same_register_split() {
        let beats: Vec<f64> = (0..65).map(|i| i as f64 * 0.5).collect();
        let downbeats: Vec<f64> = (0..17).map(|i| i as f64 * 2.0).collect();
        let mut notes = Vec::new();
        let mut truth = Vec::new();
        for bar in 0..16 {
            let t0 = bar as f64 * 2.0;
            // 白玉の和音(小節頭から 2 秒)
            for p in [60u8, 64, 67] {
                notes.push(StreamNote {
                    start: t0,
                    end: t0 + 1.9,
                    pitch: p,
                    stem: 1,
                    gain: Some(2.0),
                });
                truth.push(0);
            }
            // 16 分のアルペジオ(1 音ずつ)
            for i in 0..16 {
                notes.push(StreamNote {
                    start: t0 + i as f64 * 0.125,
                    end: t0 + i as f64 * 0.125 + 0.08,
                    pitch: [60u8, 64, 67, 72][i % 4],
                    stem: 1,
                    gain: Some(1.0),
                });
                truth.push(1);
            }
        }
        let levels = stream(
            &notes,
            &beats,
            &downbeats,
            &StreamOptions {
                max_sources: 2,
                garbage: false,
                ..Default::default()
            },
        );
        let two = &levels[1];
        // 2 つの音源と正解の対応(多い方)で数えて、9 割以上が合う
        let mut agree = [[0usize; 2]; 2];
        for (s, t) in two.source.iter().zip(&truth) {
            if let Some(s) = s {
                agree[*s][*t] += 1;
            }
        }
        let best = (agree[0][0] + agree[1][1]).max(agree[0][1] + agree[1][0]);
        assert!(best as f64 >= 0.9 * notes.len() as f64, "{agree:?}");
        assert!(two.bic(notes.len()) < levels[0].bic(notes.len()));
    }
}
