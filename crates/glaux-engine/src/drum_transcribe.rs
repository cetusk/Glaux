//! ドラムの譜起こし(信号処理だけの基準の実装)。学習したモデルは使わない(自前の学習は保留。利用者が取得したモデルは別に読む)。
//!
//! ドラムの音(分けた音、または分けていない曲の打楽器の成分)の短時間フーリエ変換(約 21ms、5ms 刻み)を
//! 4 つの帯域(低 30〜150Hz・胴 150〜400Hz・響き 1.5〜5kHz・高 7〜16kHz)にまとめ、それぞれの「音の増え方」
//! (スペクトル流束)の山を立ち上がりにする:
//!
//! 1. 高い帯域の山と余韻: 高い帯域の音が 20dB 下がるか、立ち上がりの前の大きさ + 3dB に戻るまでの長さ
//!    (下がり切る前に次の山が来たら、下がり方の速さから見積もる)。400ms を超えればシンバル
//! 2. キックとスネアの候補: 低い帯域の山と、胴 + 響きの帯域の山(響きが高い帯域より目立つか、胴の音がはっきりあるもの)。
//!    30ms 以内は 1 つにまとめ、直前の候補から 80ms 以内で全帯域が 6dB 以上小さいもの(余韻の中の山)は除く
//! 3. 候補の音の形(低 − 胴、低 − 響き の dB)を曲の中で 2 つの群に分け(k 平均)、低い帯域が目立つ方をキックにする。
//!    大きさの絶対の値は音色で大きく違う(GM の生ドラムのキックは打面の音で響きの帯域までスネアより大きい。
//!    内蔵のキックは響きの帯域がほとんど無い)ので、決まった値で切らずに曲ごとに分ける
//! 4. 同時に鳴った音: キックの群で、響きの帯域の大きさか余韻(40〜120ms 後)がキックだけのとき(群の下から 1/4)より
//!    はっきり大きければスネアも、スネアの群で低い帯域がキックの群の中央値から 6dB 以内ならキックも鳴っている。
//!    シンバルと同時のときは、シンバルの雑音をスネアと取り違えないよう、スネアを足さない
//! 5. 高い帯域の山を、余韻の長さで閉じた(短い)/ 開いた(120ms 超)ハイハット・シンバルにする
//!    (スネアと同時なら、シンバルか、高い帯域がスネアの分を超えて目立つときだけ)
//!
//! 強さは立ち上がりの直後の帯域の大きさから(曲の中の最大を 127 近くに)。
//!
//! 分けていない曲そのものから起こすとき(`DrumOptions::mixed`)は、ほかの楽器の立ち上がり・持続音で崩れないよう、
//! 山の高さを上げ、帯域の最大から 10dB 以内の立ち上がりだけを候補にし、胴の音だけのスネアの候補と余韻で見るスネアを使わない。
//!
//! オフライン処理(UI・MCP のスレッドで呼ぶ)。

use rustfft::{num_complex::Complex, FftPlanner};

/// 打楽器の種類(GM の番号に写す)
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DrumKind {
    Kick,
    Snare,
    ClosedHat,
    OpenHat,
    Cymbal,
}

impl DrumKind {
    /// GM のドラムの音の番号
    pub fn gm_note(self) -> u8 {
        match self {
            DrumKind::Kick => 36,
            DrumKind::Snare => 38,
            DrumKind::ClosedHat => 42,
            DrumKind::OpenHat => 46,
            DrumKind::Cymbal => 49,
        }
    }
}

/// 打った音 1 つ
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrumHit {
    pub time: f64,
    pub kind: DrumKind,
    pub vel: u8,
}

/// 調整できる値
#[derive(Clone, Copy, Debug)]
pub struct DrumOptions {
    /// 山とみなす高さ(流束の最大に対する割合。下げると拾う数が増える)
    pub threshold: f32,
    /// スネアとみなす「中の帯域 ÷ 高い帯域」の大きさの比(dB)
    pub snare_vs_hat_db: f32,
    /// 開いたハイハット・シンバルとみなす余韻の長さ(秒)
    pub open_secs: f64,
    pub cymbal_secs: f64,
    /// 分けていない曲(ほかの楽器の音が混ざる)から起こす。弱い立ち上がりを候補にせず、持続音で崩れる判定
    /// (胴の音だけのスネアの候補・響きの余韻で見るキックと同時のスネア)を使わない
    pub mixed: bool,
}

impl DrumOptions {
    /// 分けていない曲から起こすときの設定(山の高さを上げ、弱い立ち上がりを候補にしない)
    pub fn mixed() -> Self {
        DrumOptions {
            threshold: 0.25,
            mixed: true,
            ..Default::default()
        }
    }
}

impl Default for DrumOptions {
    fn default() -> Self {
        DrumOptions {
            threshold: 0.16,
            snare_vs_hat_db: 3.0,
            open_secs: 0.12,
            cymbal_secs: 0.4,
            mixed: false,
        }
    }
}

/// 帯域(Hz): 低(キック)・胴(スネアの胴)・響き(スネアの響き線・手拍子の雑音)・高(ハイハット・シンバル)
const BANDS: [(f64, f64); 4] = [
    (30.0, 150.0),
    (150.0, 400.0),
    (1500.0, 5000.0),
    (7000.0, 16000.0),
];
const N_BANDS: usize = BANDS.len();
/// 分けていない曲で、キック・スネアの候補とみなす帯域の大きさ(曲の中の最大からの dB)
const MIX_MIN_DB: f32 = -10.0;
/// 帯域の大きさの底(曲の中の最大からの dB)
const FLOOR_DB: f32 = 50.0;

/// フレームごとの帯域の大きさ(パワー)と刻み(秒)・窓の長さ
fn band_energies(frames: &[f32], sr: f32) -> (Vec<[f32; N_BANDS]>, f64, usize) {
    let n = ((sr as f64 * 0.021) as usize).next_power_of_two();
    let hop = n / 4;
    if frames.len() < n {
        return (Vec::new(), hop as f64 / sr as f64, n);
    }
    let fft = FftPlanner::<f32>::new().plan_fft_forward(n);
    let hann: Vec<f32> = (0..n)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos())
        .collect();
    let bin_hz = sr as f64 / n as f64;
    let which = |k: usize| -> Option<usize> {
        let f = k as f64 * bin_hz;
        BANDS.iter().position(|&(a, b)| f >= a && f < b)
    };
    let map: Vec<Option<usize>> = (0..n / 2).map(which).collect();
    let mut buf = vec![Complex::new(0.0f32, 0.0); n];
    let mut out = Vec::new();
    let mut start = 0;
    while start + n <= frames.len() {
        for (i, c) in buf.iter_mut().enumerate() {
            *c = Complex::new(frames[start + i] * hann[i], 0.0);
        }
        fft.process(&mut buf);
        let mut e = [0.0f32; N_BANDS];
        for (k, b) in map.iter().enumerate() {
            if let Some(b) = b {
                e[*b] += buf[k].norm_sqr();
            }
        }
        out.push(e);
        start += hop;
    }
    (out, hop as f64 / sr as f64, n)
}

/// 帯域の大きさ(dB)の増え方(負は 0)
fn flux(db: &[f32]) -> Vec<f32> {
    let mut v = vec![0.0f32; db.len()];
    for i in 1..db.len() {
        v[i] = (db[i] - db[i - 1]).max(0.0);
    }
    v
}

/// 山を拾う: 前後 ±30ms で最大、前後 ±150ms の中央値 + 最大 × threshold を超え、前の山から 40ms 以上空く
fn peaks(odf: &[f32], hop: f64, threshold: f32) -> Vec<usize> {
    let max = odf.iter().cloned().fold(0.0f32, f32::max);
    if max <= 1e-6 {
        return Vec::new();
    }
    let w = (0.03 / hop).round().max(1.0) as usize;
    let wa = (0.15 / hop).round().max(1.0) as usize;
    let gap = (0.04 / hop).round() as usize;
    let mut out: Vec<usize> = Vec::new();
    for i in 0..odf.len() {
        let v = odf[i];
        let (a, z) = (i.saturating_sub(w), (i + w + 1).min(odf.len()));
        if odf[a..z].iter().any(|x| *x > v) {
            continue;
        }
        let (a2, z2) = (i.saturating_sub(wa), (i + wa + 1).min(odf.len()));
        let mut win: Vec<f32> = odf[a2..z2].to_vec();
        win.sort_by(f32::total_cmp);
        let med = win[win.len() / 2];
        if v < med + threshold * max {
            continue;
        }
        if out.last().is_some_and(|&p| i - p < gap) {
            continue;
        }
        out.push(i);
    }
    out
}

/// ドラムの音から打った音を起こす(モノラル)
pub fn transcribe(frames: &[f32], sr: f32, opts: &DrumOptions) -> Vec<DrumHit> {
    let (e, hop, n) = band_energies(frames, sr);
    if e.len() < 4 {
        return Vec::new();
    }
    // dB は帯域の曲の中の最大から FLOOR_DB 下で底を打たせる(無音からの立ち上がりで増え方が桁違いに大きくなり、
    // 山の高さの基準〈増え方の最大〉を独り占めして、ほかの音を拾えなくなるのを防ぐ)
    let to_db = |v: &[f32]| -> Vec<f32> {
        let d: Vec<f32> = v.iter().map(|x| 10.0 * (x + 1e-10).log10()).collect();
        let top = d.iter().cloned().fold(f32::MIN, f32::max);
        d.into_iter().map(|x| x.max(top - FLOOR_DB)).collect()
    };
    let db: [Vec<f32>; N_BANDS] =
        std::array::from_fn(|b| to_db(&e.iter().map(|x| x[b]).collect::<Vec<_>>()));
    // 窓の中心から少し前(窓の 1/4)を立ち上がりとする
    let t_of = |i: usize| ((i as f64) * hop + n as f64 / 4.0 / sr as f64).max(0.0);
    let k30 = (0.03 / hop).round() as usize + 1;
    // 立ち上がりの直後 30ms の最大(dB)
    let after = |b: usize, i: usize| -> f32 {
        db[b][i..(i + k30).min(db[b].len())]
            .iter()
            .cloned()
            .fold(f32::MIN, f32::max)
    };
    // 帯域ごとの曲の中の最大に対する大きさ(dB。楽器・録音の明るさに左右されにくく)
    let peak_db: [f32; N_BANDS] =
        std::array::from_fn(|b| db[b].iter().cloned().fold(f32::MIN, f32::max));
    let rel = |b: usize, i: usize| after(b, i) - peak_db[b];
    let vel = |b: usize, i: usize| -> u8 { (127.0 + rel(b, i) * 2.5).clamp(20.0, 127.0) as u8 };
    let mut hits = Vec::new();
    // キックとスネアの候補: 低い帯域の立ち上がり(低い帯域が十分大きい)と、胴と響きの帯域の立ち上がり
    // (響きの雑音が十分あり、高い帯域より目立つ)。30ms 以内は 1 つにまとめる
    let mid = to_db(&e.iter().map(|x| x[1] + x[2]).collect::<Vec<_>>());
    let mut cands: Vec<usize> = Vec::new();
    for i in peaks(&flux(&db[0]), hop, opts.threshold) {
        if rel(0, i) > if opts.mixed { MIX_MIN_DB } else { -15.0 } {
            cands.push(i);
        }
    }
    for i in peaks(&flux(&mid), hop, opts.threshold) {
        let noisy = rel(2, i) > if opts.mixed { MIX_MIN_DB } else { -20.0 }
            && rel(2, i) - rel(3, i) >= opts.snare_vs_hat_db - 6.0;
        // 胴の音がはっきりある(ハイハットと同時に鳴ったスネア。響きの帯域の最大がキックの打面の音のとき、
        // スネアの響きは高い帯域〈ハイハット〉より小さく見える)
        let body = rel(1, i) > SNARE_BODY_DB;
        if noisy || (body && !opts.mixed) {
            cands.push(i);
        }
    }
    cands.sort_unstable();
    let merge = (0.03 / hop).round() as usize;
    cands.dedup_by(|b, a| b.abs_diff(*a) <= merge);
    // 余韻の中の山(GM のキックは立ち上がりの約 60ms 後にもう一度増える)を除く: 直前の候補から 80ms 以内で、
    // どの帯域も直前の候補より 6dB 以上小さいものは、新しく打った音ではない
    let tail = (0.08 / hop).round() as usize;
    let mut kept: Vec<usize> = Vec::with_capacity(cands.len());
    for &i in &cands {
        if let Some(&p) = kept.last() {
            if i - p <= tail && (0..N_BANDS).all(|b| after(b, i) < after(b, p) - TAIL_DB) {
                continue;
            }
        }
        kept.push(i);
    }
    let cands = kept;
    // 音の形: 低 − 胴、低 − 響き(dB)。キックは低い帯域が目立ち、スネアは胴・響きが目立つ。
    // 大きさの絶対の値は音色ごとに大きく違う(GM の生ドラムのキックは打面の音で響きの帯域もスネアより大きい)ので、
    // 曲の中の候補を 2 つの群に分け(k 平均)、低い帯域が目立つ方をキックにする。群が分かれないときは決まった値で分ける
    let shape = |i: usize| [after(0, i) - after(1, i), after(0, i) - after(2, i)];
    let feats: Vec<[f32; 2]> = cands.iter().map(|&i| shape(i)).collect();
    let (is_kick, clustered) = classify_kick(&feats);
    // キックの群の「響きの帯域」の大きさの下から 1/4(キックだけで鳴ったときの値の目安)。
    // これより 6dB 以上大きいキックは、同時にスネア(手拍子)も鳴っている
    let kick_noise = {
        let mut v: Vec<f32> = cands
            .iter()
            .zip(&is_kick)
            .filter(|(_, k)| **k)
            .map(|(&i, _)| rel(2, i))
            .collect();
        v.sort_by(f32::total_cmp);
        (clustered && v.len() >= 4).then(|| v[v.len() / 4])
    };
    // 響きの帯域の余韻(立ち上がりの 40〜120ms 後の平均 − 直後の最大、dB)。キックの打面の音はすぐ消えるが、
    // 手拍子・スネアの雑音は残る。キックの群の下から 1/4(キックだけのとき)より大きく残れば、同時にスネアも鳴っている
    // (GM の生ドラムのキックは打面の音が大きく、重なった手拍子で響きの帯域の大きさはほとんど増えない)
    let decay2 = |i: usize| -> f32 {
        let (a, z) = (
            i + (0.04 / hop).round() as usize,
            (i + (0.12 / hop).round() as usize).min(e.len()),
        );
        if a >= z {
            return -FLOOR_DB;
        }
        let mean = db[2][a..z]
            .iter()
            .map(|x| 10f32.powf(x / 10.0))
            .sum::<f32>()
            / (z - a) as f32;
        10.0 * mean.log10() - after(2, i)
    };
    let kick_decay = {
        let mut v: Vec<f32> = cands
            .iter()
            .zip(&is_kick)
            .filter(|(_, k)| **k)
            .map(|(&i, _)| decay2(i))
            .collect();
        v.sort_by(f32::total_cmp);
        (v.len() >= 4).then(|| v[v.len() / 4])
    };
    // キックの群の低い帯域の大きさの中央値。スネアの群でも、低い帯域がこれに近い(6dB 以内)ものは同時にキックも鳴っている
    // (4 つ打ちのキックに毎回手拍子を重ねた曲では、重なった音がスネアの群になる)
    let kick_low = {
        let mut v: Vec<f32> = cands
            .iter()
            .zip(&is_kick)
            .filter(|(_, k)| **k)
            .map(|(&i, _)| rel(0, i))
            .collect();
        v.sort_by(f32::total_cmp);
        (clustered && v.len() >= 4).then(|| v[v.len() / 2])
    };
    // 高い帯域の山と余韻(先に求めて、シンバルと重なったキック・スネアの判定に使う)。
    // 余韻: 高い帯域が立ち上がりの直後の最大から 20dB 下がるか、立ち上がりの前(直前 20ms の最小)の大きさ + 3dB に
    // 戻るまで(シンバルの響きの中で叩いたハイハットは、響きの大きさまで下がれば終わり。響きを自分の余韻と取り違えない)。
    // 下がり切る前に次の山が来たら、そこまでの下がり方の速さから下がり切る時間を見積もる
    // (8 分のハイハットの中のシンバル、細かいスネアの連打の中の山)
    let high_peaks: Vec<usize> = peaks(&flux(&db[3]), hop, opts.threshold);
    let high: Vec<(usize, f64)> = high_peaks
        .iter()
        .enumerate()
        .filter(|(_, &i)| rel(3, i) >= -30.0)
        .map(|(n, &i)| {
            let top = after(3, i);
            let pre_n = (0.02 / hop).round().max(1.0) as usize;
            let pre = db[3][i.saturating_sub(pre_n)..=i]
                .iter()
                .cloned()
                .fold(f32::MAX, f32::min);
            let floor = (top - 20.0).max(pre + 3.0);
            let next = high_peaks
                .get(n + 1)
                .map_or(db[3].len(), |&j| j.saturating_sub(1))
                .min(db[3].len());
            let limit = i + (1.5 / hop) as usize;
            // 立ち上がりの直後の最大の所から測る
            let k0 = (i..(i + k30).min(db[3].len()))
                .max_by(|&a, &b| db[3][a].total_cmp(&db[3][b]))
                .unwrap_or(i);
            let mut k = k0;
            while k + 1 < next && k < limit && db[3][k] > floor {
                k += 1;
            }
            let elapsed = (k - i) as f64 * hop;
            if db[3][k] > floor && k + 1 >= next && k > k0 {
                // 次の山までに下がり切らなかった: 下がり方の速さから見積もる
                let drop = (top - db[3][k]).max(0.5) as f64;
                let span = (k - k0) as f64 * hop;
                (i, (span * (top - floor) as f64 / drop).min(1.5))
            } else {
                (i, elapsed)
            }
        })
        .collect();
    let gap = (0.025 / hop).round() as usize;
    // シンバルと同時か(シンバルの雑音は響きの帯域にも長く残り、スネアの雑音と見分けにくい)
    let with_cymbal = |i: usize| {
        high.iter()
            .any(|&(h, d)| d >= opts.cymbal_secs && h.abs_diff(i) <= gap)
    };
    let mut snare_frames = Vec::new();
    for (&i, kick) in cands.iter().zip(is_kick) {
        if kick {
            hits.push(DrumHit {
                time: t_of(i),
                kind: DrumKind::Kick,
                vel: vel(0, i),
            });
            if !with_cymbal(i)
                && (kick_noise.is_some_and(|q| rel(2, i) > q + SNARE_ON_KICK_DB)
                    || (!opts.mixed && kick_decay.is_some_and(|q| decay2(i) > q + SNARE_DECAY_DB)))
            {
                hits.push(DrumHit {
                    time: t_of(i),
                    kind: DrumKind::Snare,
                    vel: vel(2, i),
                });
                snare_frames.push(i);
            }
        } else {
            let also_kick = kick_low.is_some_and(|m| rel(0, i) > m - KICK_ON_SNARE_DB);
            if also_kick {
                hits.push(DrumHit {
                    time: t_of(i),
                    kind: DrumKind::Kick,
                    vel: vel(0, i),
                });
                // シンバルと重なったキック: シンバルの雑音でスネアの群に入っただけ
                if with_cymbal(i) {
                    continue;
                }
            }
            hits.push(DrumHit {
                time: t_of(i),
                kind: DrumKind::Snare,
                vel: vel(2, i),
            });
            snare_frames.push(i);
        }
    }
    // 高い帯域の山: ハイハット・シンバル(スネアと同時なら、シンバルか、高い帯域がスネアの分を超えて目立つときだけ)
    for &(i, decay) in &high {
        let with_snare = snare_frames.iter().any(|&s| s.abs_diff(i) <= gap);
        if with_snare && decay < opts.cymbal_secs && rel(3, i) < rel(2, i) + opts.snare_vs_hat_db {
            continue;
        }
        let kind = if decay >= opts.cymbal_secs {
            DrumKind::Cymbal
        } else if decay >= opts.open_secs {
            DrumKind::OpenHat
        } else {
            DrumKind::ClosedHat
        };
        hits.push(DrumHit {
            time: t_of(i),
            kind,
            vel: vel(3, i),
        });
    }
    hits.sort_by(|a, b| a.time.total_cmp(&b.time));
    hits
}

/// キックとスネアの候補の音の形(低 − 胴、低 − 響き)を 2 つの群に分け、キックの側なら true。
/// 群の中心が 5dB 以上離れないとき(どちらか一方しか無い曲)は、低 − 響き が 0dB を超えるものをキックにする
/// 2 つの群に分かれたかどうかも返す
fn classify_kick(feats: &[[f32; 2]]) -> (Vec<bool>, bool) {
    let score = |f: &[f32; 2]| f[0] + f[1];
    let fallback = || (feats.iter().map(|f| f[1] > 0.0).collect::<Vec<_>>(), false);
    if feats.len() < 4 {
        return fallback();
    }
    // 初めの中心: 音の形の得点の上から 15%・下から 15% のもの(外れた 1 つに引きずられないよう、端は使わない)
    let mut sorted: Vec<[f32; 2]> = feats.to_vec();
    sorted.sort_by(|a, b| score(a).total_cmp(&score(b)));
    let mut cs = sorted[sorted.len() * 15 / 100];
    let mut ck = sorted[sorted.len() * 85 / 100];
    let d2 = |a: &[f32; 2], b: &[f32; 2]| (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2);
    let mut assign = vec![false; feats.len()];
    for _ in 0..20 {
        for (a, f) in assign.iter_mut().zip(feats) {
            *a = d2(f, &ck) < d2(f, &cs);
        }
        let mean = |want: bool| -> Option<[f32; 2]> {
            let v: Vec<&[f32; 2]> = feats
                .iter()
                .zip(&assign)
                .filter(|(_, a)| **a == want)
                .map(|(f, _)| f)
                .collect();
            (!v.is_empty()).then(|| {
                let n = v.len() as f32;
                [
                    v.iter().map(|f| f[0]).sum::<f32>() / n,
                    v.iter().map(|f| f[1]).sum::<f32>() / n,
                ]
            })
        };
        match (mean(true), mean(false)) {
            (Some(k), Some(s)) => {
                ck = k;
                cs = s;
            }
            _ => return fallback(),
        }
    }
    if d2(&ck, &cs).sqrt() < 5.0 {
        return fallback();
    }
    (assign, true)
}

/// キックと同時にスネアも鳴っているとみなす、響きの帯域の増え方(dB)
const SNARE_ON_KICK_DB: f32 = 6.0;
/// スネアと同時にキックも鳴っているとみなす、キックの群の低い帯域からの下がり(dB)
const KICK_ON_SNARE_DB: f32 = 6.0;
/// キックと同時にスネアも鳴っているとみなす、響きの帯域の余韻の長さの差(dB)
const SNARE_DECAY_DB: f32 = 10.0;
/// スネアの候補とみなす胴の帯域の大きさ(曲の中の最大からの dB)
const SNARE_BODY_DB: f32 = -15.0;
/// 余韻の中の山とみなす、直前の候補からの下がり(dB。すべての帯域で)
const TAIL_DB: f32 = 6.0;

#[cfg(test)]
mod tests {
    use super::*;

    /// キック(低い正弦の減衰)・スネア(胴 + 雑音)・閉じたハイハット(高い雑音・短い)を並べた音。
    /// click: キックの打面の音(雑音)の大きさ(生ドラムのキックは響きの帯域までスネアより大きいことがある)
    fn pattern(sr: f32, click: f32) -> (Vec<f32>, Vec<(f64, DrumKind)>) {
        let secs = 4.6;
        let n = (secs * sr as f64) as usize;
        let mut x = vec![0.0f32; n];
        let mut seed = 7u64;
        let mut noise = move || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 33) as f32 / (1u64 << 31) as f32) * 2.0 - 1.0
        };
        let mut truth = Vec::new();
        let beat = 0.5;
        for k in 0..8 {
            let t = 0.25 + k as f64 * beat;
            let s = (t * sr as f64) as usize;
            if k % 2 == 0 {
                truth.push((t, DrumKind::Kick));
                for i in 0..(0.25 * sr) as usize {
                    let tt = i as f32 / sr;
                    let f = 50.0 + 80.0 * (-tt * 30.0).exp();
                    x[s + i] += 0.8 * (std::f32::consts::TAU * f * tt).sin() * (-tt * 12.0).exp()
                        + click * noise() * (-tt * 150.0).exp();
                }
            } else {
                truth.push((t, DrumKind::Snare));
                for i in 0..(0.2 * sr) as usize {
                    let tt = i as f32 / sr;
                    let body =
                        0.4 * (std::f32::consts::TAU * 200.0 * tt).sin() * (-tt * 25.0).exp();
                    // 響き線は 1.5〜5kHz に寄せた雑音(1 つ前との差で高域を少し抑える)
                    let nz = 0.35 * noise() * (-tt * 18.0).exp();
                    x[s + i] += body + nz;
                }
            }
            // 8 分ごとのハイハット(拍の頭と裏)
            for off in [0.0, beat / 2.0] {
                let th = t + off;
                if k % 2 == 1 && off == 0.0 {
                    continue; // スネアと同時のハットは鳴らさない
                }
                truth.push((th, DrumKind::ClosedHat));
                let sh = (th * sr as f64) as usize;
                let mut prev = 0.0;
                for i in 0..(0.05 * sr) as usize {
                    let tt = i as f32 / sr;
                    let w = noise();
                    // 高い帯域へ(差分で低域を落とす)
                    let hp = w - prev;
                    prev = w;
                    x[sh + i] += 0.25 * hp * (-tt * 80.0).exp();
                }
            }
        }
        truth.sort_by(|a, b| a.0.total_cmp(&b.0));
        (x, truth)
    }

    #[test]
    fn kick_snare_and_hats_are_told_apart() {
        let sr = 44_100.0;
        for click in [0.0, 1.2] {
            let (x, truth) = pattern(sr, click);
            check(&x, sr, &truth, &DrumOptions::default());
            // 曲そのもの用の設定でも、ドラムだけの音なら同じように拾う
            check(&x, sr, &truth, &DrumOptions::mixed());
        }
    }

    fn check(x: &[f32], sr: f32, truth: &[(f64, DrumKind)], opts: &DrumOptions) {
        let hits = transcribe(x, sr, opts);
        for kind in [DrumKind::Kick, DrumKind::Snare, DrumKind::ClosedHat] {
            let t: Vec<f64> = truth.iter().filter(|h| h.1 == kind).map(|h| h.0).collect();
            let e: Vec<f64> = hits
                .iter()
                .filter(|h| h.kind == kind)
                .map(|h| h.time)
                .collect();
            let hit = crate::mir_eval::match_events(&t, &e, 0.05);
            assert!(
                hit as f64 >= t.len() as f64 * 0.8 && e.len() as f64 <= t.len() as f64 * 1.3,
                "{kind:?}: 正解 {} 推定 {} 当たり {hit}\n{hits:?}",
                t.len(),
                e.len()
            );
        }
    }

    #[test]
    fn kick_cluster_follows_the_kit() {
        // 内蔵のキット: キックは 低 − 響き が大きく、スネアは負
        let builtin: Vec<[f32; 2]> = (0..20)
            .map(|i| {
                if i % 2 == 0 {
                    [-1.0, 27.0]
                } else {
                    [-7.5, -7.0]
                }
            })
            .collect();
        let (k, ok) = classify_kick(&builtin);
        assert!(ok);
        assert!(k.iter().enumerate().all(|(i, k)| *k == (i % 2 == 0)));
        // GM の生ドラム: キックもスネアも 低 − 響き が正(決まった値では分けられない)。外れた 1 つが混じっても
        let mut gm: Vec<[f32; 2]> = (0..20)
            .map(|i| if i % 2 == 0 { [9.5, 10.0] } else { [2.0, 7.0] })
            .collect();
        gm.push([22.0, 25.0]);
        let (k, ok) = classify_kick(&gm);
        assert!(ok);
        assert!(
            k[..20].iter().enumerate().all(|(i, k)| *k == (i % 2 == 0)),
            "{k:?}"
        );
        // キックしか無い: 群に分けない
        let only: Vec<[f32; 2]> = (0..20).map(|i| [9.0 + (i % 3) as f32, 10.0]).collect();
        assert!(!classify_kick(&only).1);
    }
}
