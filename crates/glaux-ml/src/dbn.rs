//! 拍と小節頭の後処理: 動的ベイジアンネットワーク(madmom の `DBNDownBeatTrackingProcessor` の移植。コードは BSD)。
//!
//! 「小節の中のどこにいるか」を指す針(bar pointer)を状態にした隠れマルコフモデル(Krebs ら 2015・Böck ら 2016)。
//! 状態は「拍子(3 拍子・4 拍子などを別のモデルに)× 拍の何番目か × テンポ(1 拍のフレーム数)× 拍の中の位置」。
//! 針は 1 フレームに 1 つ進み、拍の終わりでだけ次の拍へ移ってテンポを変えられる(変わりにくさは `transition_lambda`)。
//! 観測はフレームごとの「拍(小節頭以外)」「小節頭」の確率で、拍の頭の近くの状態はそれぞれの確率、
//! ほかは「どちらでもない」確率で重みを付ける。拍子ごとにビタビで最もありそうな道をたどり、
//! いちばん確からしい拍子を採る。最後に、拍の頭の区間ごとに確率がいちばん高いフレームを拍にする
//! (madmom の `correct=True` と同じ)。
//!
//! Beat This! の公式の後処理(`postp_dbn`)と同じく、拍の確率から小節頭の確率を引いたものを「拍(小節頭以外)」にする。
//! 公式の「最小の後処理」(ロジットの極大)より、小節頭の数え間違い・拍子の取り違えに強い。

/// 後処理の設定(既定は Beat This! の公式の設定)
#[derive(Clone, Debug)]
pub struct DbnOptions {
    /// 候補の拍子(1 小節の拍数)
    pub beats_per_bar: Vec<u32>,
    pub min_bpm: f64,
    pub max_bpm: f64,
    /// テンポの段数(対数で等分。整数のフレーム数にそろえるので実際は少なくなることがある)
    pub num_tempi: usize,
    /// テンポの変わりにくさ(大きいほど変わりにくい)
    pub transition_lambda: f64,
    /// 拍の頭とみなす幅(拍の長さの 1/この値)
    pub observation_lambda: f64,
    /// これより確率の低い冒頭・末尾は解析しない
    pub threshold: f32,
    /// 1 秒のフレーム数
    pub fps: f64,
}

impl Default for DbnOptions {
    fn default() -> Self {
        DbnOptions {
            beats_per_bar: vec![3, 4],
            min_bpm: 55.0,
            max_bpm: 215.0,
            num_tempi: 60,
            transition_lambda: 100.0,
            observation_lambda: 16.0,
            threshold: 0.05,
            fps: 50.0,
        }
    }
}

/// 結果
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DbnResult {
    /// 拍の時刻(秒)と、小節の何拍目か(1 始まり。1 が小節頭)
    pub beats: Vec<(f64, u32)>,
    /// 選んだ拍子(1 小節の拍数。拍が無ければ 0)
    pub beats_per_bar: u32,
    /// 選んだ道の対数尤度
    pub log_prob: f64,
}

/// 1 拍のフレーム数の候補(madmom の `BeatStateSpace` と同じ作り方)
fn intervals(min: f64, max: f64, n: usize) -> Vec<usize> {
    let all: Vec<usize> = ((min.round() as usize)..=(max.round() as usize)).collect();
    if n >= all.len() {
        return all;
    }
    let mut k = n;
    loop {
        let mut v: Vec<usize> = (0..k)
            .map(|i| {
                let e = min.log2() + (max.log2() - min.log2()) * i as f64 / (k - 1).max(1) as f64;
                2f64.powf(e).round() as usize
            })
            .collect();
        v.dedup();
        if v.len() >= n {
            return v;
        }
        k += 1;
    }
}

/// 拍子 1 つ分のモデルをビタビでたどる。(道〈フレームごとの状態〉, 対数尤度)
struct BarModel {
    beats: usize,
    /// テンポごとの 1 拍のフレーム数
    lens: Vec<usize>,
    /// テンポごとの最初の状態(1 拍の中で)
    offs: Vec<usize>,
    /// 1 拍の状態の数
    per_beat: usize,
    /// テンポの移り変わりの対数確率 [前][後](移れないものは -inf)
    trans: Vec<Vec<f64>>,
    /// 拍の頭とみなす位置の数(テンポごと)
    head: Vec<usize>,
}

impl BarModel {
    fn new(beats: usize, opts: &DbnOptions) -> Self {
        let min_iv = 60.0 * opts.fps / opts.max_bpm;
        let max_iv = 60.0 * opts.fps / opts.min_bpm;
        let lens = intervals(min_iv, max_iv, opts.num_tempi);
        let mut offs = Vec::with_capacity(lens.len());
        let mut acc = 0;
        for &l in &lens {
            offs.push(acc);
            acc += l;
        }
        let trans = lens
            .iter()
            .map(|&from| {
                let row: Vec<f64> = lens
                    .iter()
                    .map(|&to| {
                        let p =
                            (-opts.transition_lambda * (to as f64 / from as f64 - 1.0).abs()).exp();
                        if p <= f64::EPSILON {
                            0.0
                        } else {
                            p
                        }
                    })
                    .collect();
                let sum: f64 = row.iter().sum();
                row.iter()
                    .map(|p| {
                        if *p > 0.0 {
                            (p / sum).ln()
                        } else {
                            f64::NEG_INFINITY
                        }
                    })
                    .collect()
            })
            .collect();
        // 位置 j / L < 1 / λ の状態が拍の頭
        let head = lens
            .iter()
            .map(|&l| {
                (0..l)
                    .filter(|&j| (j as f64) / (l as f64) < 1.0 / opts.observation_lambda)
                    .count()
            })
            .collect();
        BarModel {
            beats,
            lens,
            offs,
            per_beat: acc,
            trans,
            head,
        }
    }

    fn n_states(&self) -> usize {
        self.beats * self.per_beat
    }

    /// 状態 → (拍, テンポ, 位置)
    fn decompose(&self, s: usize) -> (usize, usize, usize) {
        let b = s / self.per_beat;
        let r = s % self.per_beat;
        let k = self.offs.partition_point(|&o| o <= r) - 1;
        (b, k, r - self.offs[k])
    }

    /// 観測の種類(0 = どちらでもない, 1 = 拍, 2 = 小節頭)
    fn pointer(&self, b: usize, k: usize, j: usize) -> usize {
        if j < self.head[k] {
            if b == 0 {
                2
            } else {
                1
            }
        } else {
            0
        }
    }

    /// ビタビ。`obs[t]` は 3 種類の観測の対数密度
    fn viterbi(&self, obs: &[[f64; 3]]) -> (Vec<usize>, f64) {
        let n = self.n_states();
        let nk = self.lens.len();
        let t_len = obs.len();
        let mut cur = vec![-(n as f64).ln(); n];
        // 冒頭のフレームの観測
        for b in 0..self.beats {
            for k in 0..nk {
                for j in 0..self.lens[k] {
                    cur[b * self.per_beat + self.offs[k] + j] += obs[0][self.pointer(b, k, j)];
                }
            }
        }
        // 拍の頭の状態にだけ、前の拍のどのテンポから来たかを残す
        let mut back = vec![0u8; t_len * self.beats * nk];
        let mut next = vec![f64::NEG_INFINITY; n];
        for t in 1..t_len {
            let o = &obs[t];
            for b in 0..self.beats {
                let prev_b = (b + self.beats - 1) % self.beats;
                let base = b * self.per_beat;
                let pbase = prev_b * self.per_beat;
                for k in 0..nk {
                    let l = self.lens[k];
                    let s0 = base + self.offs[k];
                    // 拍の頭: 前の拍の終わりから
                    let (mut best, mut arg) = (f64::NEG_INFINITY, 0usize);
                    for (kp, row) in self.trans.iter().enumerate() {
                        let tr = row[k];
                        if tr == f64::NEG_INFINITY {
                            continue;
                        }
                        let v = cur[pbase + self.offs[kp] + self.lens[kp] - 1] + tr;
                        if v > best {
                            best = v;
                            arg = kp;
                        }
                    }
                    next[s0] = best + o[self.pointer(b, k, 0)];
                    back[(t * self.beats + b) * nk + k] = arg as u8;
                    // 拍の中: 1 つ前の位置から
                    for j in 1..l {
                        next[s0 + j] = cur[s0 + j - 1] + o[self.pointer(b, k, j)];
                    }
                }
            }
            std::mem::swap(&mut cur, &mut next);
        }
        let (mut s, logp) = cur
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, v)| (i, *v))
            .unwrap_or((0, f64::NEG_INFINITY));
        let mut path = vec![0usize; t_len];
        for t in (0..t_len).rev() {
            path[t] = s;
            if t == 0 {
                break;
            }
            let (b, k, j) = self.decompose(s);
            s = if j > 0 {
                s - 1
            } else {
                let kp = back[(t * self.beats + b) * nk + k] as usize;
                let pb = (b + self.beats - 1) % self.beats;
                pb * self.per_beat + self.offs[kp] + self.lens[kp] - 1
            };
        }
        (path, logp)
    }
}

/// フレームごとの拍(小節頭を含む)と小節頭の確率から、拍と小節頭を決める
pub fn decode(beat_prob: &[f32], downbeat_prob: &[f32], opts: &DbnOptions) -> DbnResult {
    let n = beat_prob.len().min(downbeat_prob.len());
    // 0 と 1 はモデルを壊すので少し内側へ(公式と同じ)
    const EPS: f32 = 1e-5;
    let act: Vec<[f32; 2]> = (0..n)
        .map(|t| {
            let b = beat_prob[t] * (1.0 - EPS) + EPS / 2.0;
            let d = downbeat_prob[t] * (1.0 - EPS) + EPS / 2.0;
            [(b - d).max(EPS / 2.0), d]
        })
        .collect();
    // 確率の低い冒頭・末尾を除く
    let first = act
        .iter()
        .position(|a| a[0] >= opts.threshold || a[1] >= opts.threshold);
    let Some(first) = first else {
        return DbnResult::default();
    };
    let last = act
        .iter()
        .rposition(|a| a[0] >= opts.threshold || a[1] >= opts.threshold)
        .map_or(first, |l| l + 1);
    let act = &act[first..last];
    if act.len() < 2 {
        return DbnResult::default();
    }
    let lam = opts.observation_lambda;
    let obs: Vec<[f64; 3]> = act
        .iter()
        .map(|a| {
            let none = ((1.0 - a[0] as f64 - a[1] as f64) / (lam - 1.0)).max(1e-300);
            [none.ln(), (a[0] as f64).ln(), (a[1] as f64).ln()]
        })
        .collect();

    // 拍子ごとにたどり、いちばん確からしいものを採る
    let mut best: Option<(BarModel, Vec<usize>, f64)> = None;
    for &bpb in &opts.beats_per_bar {
        if bpb == 0 {
            continue;
        }
        let m = BarModel::new(bpb as usize, opts);
        let (path, logp) = m.viterbi(&obs);
        if best.as_ref().is_none_or(|b| logp > b.2) {
            best = Some((m, path, logp));
        }
    }
    let Some((m, path, logp)) = best else {
        return DbnResult::default();
    };

    // 拍の頭の区間ごとに、確率のいちばん高いフレームを拍にする
    let in_head: Vec<bool> = path
        .iter()
        .map(|&s| {
            let (b, k, j) = m.decompose(s);
            m.pointer(b, k, j) >= 1
        })
        .collect();
    let mut beats = Vec::new();
    let mut t = 0;
    while t < in_head.len() {
        if !in_head[t] {
            t += 1;
            continue;
        }
        let left = t;
        while t < in_head.len() && in_head[t] {
            t += 1;
        }
        let peak = (left..t)
            .max_by(|&a, &b| {
                let va = act[a][0].max(act[a][1]);
                let vb = act[b][0].max(act[b][1]);
                va.total_cmp(&vb)
            })
            .unwrap_or(left);
        let (b, _, _) = m.decompose(path[peak]);
        beats.push(((peak + first) as f64 / opts.fps, b as u32 + 1));
    }
    DbnResult {
        beats,
        beats_per_bar: m.beats as u32,
        log_prob: logp,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `bpm`・`bpb` 拍子の確率の列(拍の位置に山、小節頭は小節頭の確率にも山)
    fn activations(bpm: f64, bpb: usize, secs: f64, fps: f64) -> (Vec<f32>, Vec<f32>) {
        let n = (secs * fps) as usize;
        let mut beat = vec![0.01f32; n];
        let mut down = vec![0.005f32; n];
        let ibi = 60.0 / bpm * fps;
        let mut k = 0;
        loop {
            let f = (1.0 + k as f64 * ibi).round() as usize;
            if f >= n {
                break;
            }
            beat[f] = 0.9;
            if k % bpb == 0 {
                down[f] = 0.8;
            } else {
                // 小節頭でない拍にも少し小節頭らしさを混ぜる(実際のモデルの出力に近づける)
                down[f] = 0.1;
            }
            k += 1;
        }
        (beat, down)
    }

    #[test]
    fn finds_beats_meter_and_downbeats() {
        for (bpm, bpb) in [(120.0, 4), (96.0, 3), (150.0, 4)] {
            let (b, d) = activations(bpm, bpb, 30.0, 50.0);
            let r = decode(&b, &d, &DbnOptions::default());
            assert_eq!(r.beats_per_bar, bpb as u32, "{bpm} BPM");
            let ibi = 60.0 / bpm;
            let n_expected = (30.0 / ibi) as usize;
            assert!(
                (r.beats.len() as i64 - n_expected as i64).abs() <= 1,
                "{} vs {n_expected}",
                r.beats.len()
            );
            // 小節頭は 0 番目の拍(フレーム 1)から bpb 拍ごと
            for (i, (t, num)) in r.beats.iter().enumerate() {
                let expect = (1.0 + (i as f64 * ibi * 50.0)).round() / 50.0;
                assert!((t - expect).abs() <= 0.021, "{t} vs {expect}");
                assert_eq!(*num as usize, i % bpb + 1);
            }
        }
    }

    #[test]
    fn silent_input_has_no_beats() {
        let r = decode(&[0.0; 500], &[0.0; 500], &DbnOptions::default());
        assert!(r.beats.is_empty());
    }

    #[test]
    fn interval_grid_matches_madmom() {
        // 50fps・55〜215BPM: 14〜55 フレームの全部(42 段)
        let v = intervals(60.0 * 50.0 / 215.0, 60.0 * 50.0 / 55.0, 60);
        assert_eq!(v.first(), Some(&14));
        assert_eq!(v.last(), Some(&55));
        assert_eq!(v.len(), 42);
        // 段数を絞ると対数で間引く
        let v = intervals(14.0, 55.0, 10);
        assert!(v.len() >= 10 && v.len() < 42);
    }
}
