//! タイムストレッチ(音程を変えずに長さを変える)。WSOLA と、位相をそろえたフェーズボコーダ。
//! [`stretch_channels`] が素材に合わせて選ぶ(打楽器・雑音の多い素材は WSOLA、和音・持続音はフェーズボコーダ)。
//!
//! WSOLA:
//!
//! 出力の各フレーム(40ms、50% 重なり)について「本来読むべき入力位置」を
//! 写像 `src_pos` から求め、その周囲 ±10ms で直前フレームの自然な続きと最も似た
//! 区間を探して重ね合わせる。波形の周期がそろうので、単純な切り貼りより
//! ブツブツ感が少ない。声・楽器の単音・リズム素材向けの汎用手法で、
//! テンポ変化が大きい(±30% 超)と金属的な響きが出やすい。
//!
//! オフライン処理(再生データの構築時に UI スレッドで呼ぶ)。オーディオスレッドでは使わない。

/// フレーム長(秒)と探索幅(秒)
const FRAME_SECS: f32 = 0.040;
const TOLERANCE_SECS: f32 = 0.010;
/// 類似度の粗探索で間引く間隔(サンプル)
const COARSE: usize = 4;

/// `input` をタイムストレッチして `out_len` サンプルの波形を作る。
/// `src_pos(i)` は出力サンプル `i` に対応する入力上の位置(サンプル、小数可)で、
/// 単調増加であること。入力の終わりを越えたところからは無音になる。
pub fn wsola(
    input: &[f32],
    sample_rate: f32,
    out_len: usize,
    src_pos: impl Fn(usize) -> f64,
) -> Vec<f32> {
    wsola_channels(&[input], sample_rate, out_len, src_pos)
        .pop()
        .unwrap_or_default()
}

/// 複数チャンネルを同じ切り貼り位置で伸縮する(位置は最初のチャンネルで決める)。
/// ステレオは M(左右の平均)と S(左右差)を渡すと、左右の定位が崩れない。
pub fn wsola_channels(
    channels: &[&[f32]],
    sample_rate: f32,
    out_len: usize,
    src_pos: impl Fn(usize) -> f64,
) -> Vec<Vec<f32>> {
    let Some(input) = channels.first().copied() else {
        return Vec::new();
    };
    let n = ((FRAME_SECS * sample_rate) as usize).max(16) & !1;
    let hop = n / 2;
    let tol = ((TOLERANCE_SECS * sample_rate) as usize).max(1);
    let mut outs = vec![vec![0.0f32; out_len + n]; channels.len()];
    let mut wsum = vec![0.0f32; out_len + n];
    if input.len() < n || out_len == 0 {
        for o in outs.iter_mut() {
            o.truncate(out_len);
        }
        return outs;
    }
    // 周期ハン窓(50% 重なりで和が 1)
    let window: Vec<f32> = (0..n)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos())
        .collect();
    let last_start = input.len() - n;

    let mut prev: Option<usize> = None;
    let mut k = 0usize;
    loop {
        let out_start = k * hop;
        if out_start >= out_len {
            break;
        }
        let target = src_pos(out_start).round();
        if target < 0.0 || target as usize > last_start {
            if target as usize > last_start {
                break; // 入力を読み切った
            }
            k += 1;
            continue;
        }
        let target = target as usize;
        let start = match prev {
            None => target,
            Some(p) => {
                let natural = p + hop;
                if natural > last_start {
                    target
                } else {
                    best_match(input, natural, target, tol, hop, last_start)
                }
            }
        };
        for (out, ch) in outs.iter_mut().zip(channels) {
            if ch.len() < start + n {
                continue;
            }
            for i in 0..n {
                out[out_start + i] += ch[start + i] * window[i];
            }
        }
        for i in 0..n {
            wsum[out_start + i] += window[i];
        }
        prev = Some(start);
        k += 1;
    }
    for out in outs.iter_mut() {
        for (o, w) in out.iter_mut().zip(&wsum) {
            if *w > 1e-3 {
                *o /= *w;
            }
        }
        out.truncate(out_len);
    }
    outs
}

/// `target` の ±`tol` から、`natural` で始まる区間(長さ `len`)と最も似た開始位置を探す。
fn best_match(
    input: &[f32],
    natural: usize,
    target: usize,
    tol: usize,
    len: usize,
    last: usize,
) -> usize {
    let lo = target.saturating_sub(tol);
    let hi = (target + tol).min(last);
    if lo >= hi {
        return target.min(last);
    }
    let reference = &input[natural..natural + len];
    // 正規化相互相関(音量差に引っ張られないように候補側のエネルギーで割る)
    let score = |cand: usize, stride: usize| -> f32 {
        let seg = &input[cand..cand + len];
        let mut dot = 0.0f32;
        let mut energy = 1e-9f32;
        let mut i = 0;
        while i < len {
            dot += reference[i] * seg[i];
            energy += seg[i] * seg[i];
            i += stride;
        }
        dot / energy.sqrt()
    };
    let mut best = target.clamp(lo, hi);
    let mut best_score = f32::MIN;
    let mut c = lo;
    while c <= hi {
        let s = score(c, COARSE);
        if s > best_score {
            best_score = s;
            best = c;
        }
        c += COARSE;
    }
    // 粗探索の最良点の周りを 1 サンプル刻みで詰める
    let (rlo, rhi) = (
        best.saturating_sub(COARSE - 1).max(lo),
        (best + COARSE - 1).min(hi),
    );
    best_score = f32::MIN;
    for c in rlo..=rhi {
        let s = score(c, 1);
        if s > best_score {
            best_score = s;
            best = c;
        }
    }
    best
}

// ===================== 位相をそろえたフェーズボコーダ =====================
//
// 和音・持続音向け。4096 点(48kHz で約 85ms)の短時間フーリエ変換で、出力のフレームごとに
// 入力の位置(写像 `src_pos`)の振幅と、そこから 1 ずらし幅先の位相との差で「本当の周波数」を求め、
// 出力の位相を積み上げる。周波数の山(ピーク)の位相だけを積み上げ、周りのビンは山との位相の差を
// そのまま保つ(identity phase locking。Laroche & Dolson)ので、和音が金属的・ぼやけた響きになりにくい。
// 打点(スペクトルの急な増え)では、出力の位相を入力の位相にそろえ直して、アタックがにじまないようにする。

const PV_N: usize = 4096;
const PV_HOP: usize = PV_N / 4;

/// フェーズボコーダで伸縮する(チャンネルは M と S を想定。位相の進みは最初のチャンネルで決め、
/// ほかのチャンネルにも同じ回転を掛けて左右の関係を保つ)
pub fn phase_vocoder_channels(
    channels: &[&[f32]],
    out_len: usize,
    src_pos: impl Fn(usize) -> f64,
) -> Vec<Vec<f32>> {
    use rustfft::{num_complex::Complex, FftPlanner};
    let nch = channels.len();
    let Some(input) = channels.first().copied() else {
        return Vec::new();
    };
    let mut outs = vec![vec![0.0f32; out_len + PV_N]; nch];
    let mut wsum = vec![0.0f32; out_len + PV_N];
    if input.len() < PV_N || out_len == 0 {
        for o in outs.iter_mut() {
            o.truncate(out_len);
        }
        return outs;
    }
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(PV_N);
    let ifft = planner.plan_fft_inverse(PV_N);
    let window: Vec<f32> = (0..PV_N)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / PV_N as f32).cos())
        .collect();
    let bins = PV_N / 2 + 1;
    let frame = |ch: &[f32], start: isize| -> Vec<Complex<f32>> {
        let mut buf: Vec<Complex<f32>> = (0..PV_N)
            .map(|i| {
                let j = start + i as isize;
                let v = if j < 0 || j as usize >= ch.len() {
                    0.0
                } else {
                    ch[j as usize]
                };
                Complex::new(v * window[i], 0.0)
            })
            .collect();
        fft.process(&mut buf);
        buf
    };
    let princ = |x: f32| {
        let tau = std::f32::consts::TAU;
        x - tau * (x / tau).round()
    };
    let mut synth_phase = vec![0.0f32; bins];
    let mut prev_mag = vec![0.0f32; bins];
    let mut flux_avg = 0.0f32;
    let mut first = true;
    let last_start = input.len().saturating_sub(PV_N) as f64;
    let mut m = 0usize;
    loop {
        let out_start = m * PV_HOP;
        if out_start >= out_len {
            break;
        }
        let a = src_pos(out_start + PV_N / 2) - (PV_N / 2) as f64;
        if a > last_start {
            break;
        }
        let a = a.round() as isize;
        let x0 = frame(input, a);
        let x1 = frame(input, a + PV_HOP as isize);
        let mag: Vec<f32> = x0[..bins].iter().map(|c| c.norm()).collect();
        let phase0: Vec<f32> = x0[..bins].iter().map(|c| c.arg()).collect();
        // 打点: 増えた分のスペクトルの和が、これまでの平均の 3 倍を超えたら位相をそろえ直す
        let flux: f32 = mag
            .iter()
            .zip(&prev_mag)
            .map(|(a, b)| (a - b).max(0.0))
            .sum();
        let onset = first || flux > 3.0 * flux_avg.max(1e-6);
        flux_avg = 0.9 * flux_avg + 0.1 * flux;
        prev_mag.copy_from_slice(&mag);
        if onset {
            synth_phase.copy_from_slice(&phase0);
        } else {
            // 周波数の山を探し、山の位相だけを「本当の周波数 × ずらし幅」だけ進める
            let mut peak_of = vec![0usize; bins];
            let mut peaks = Vec::new();
            for k in 0..bins {
                let lo = k.saturating_sub(2);
                let hi = (k + 2).min(bins - 1);
                if mag[k] > 0.0 && (lo..=hi).all(|j| j == k || mag[j] <= mag[k]) {
                    peaks.push(k);
                }
            }
            if peaks.is_empty() {
                peaks.push(0);
            }
            // ビンごとに近い方の山(山の間は真ん中で分ける)
            let mut pi = 0;
            for (k, p) in peak_of.iter_mut().enumerate() {
                while pi + 1 < peaks.len()
                    && (peaks[pi + 1] as isize - k as isize).abs()
                        < (k as isize - peaks[pi] as isize).abs()
                {
                    pi += 1;
                }
                *p = peaks[pi];
            }
            let mut new_phase = vec![0.0f32; bins];
            for &p in &peaks {
                let omega = std::f32::consts::TAU * p as f32 / PV_N as f32;
                let dphi = x1[p].arg() - phase0[p];
                let adv = omega * PV_HOP as f32 + princ(dphi - omega * PV_HOP as f32);
                new_phase[p] = synth_phase[p] + adv;
            }
            for k in 0..bins {
                let p = peak_of[k];
                if k != p {
                    new_phase[k] = new_phase[p] + (phase0[k] - phase0[p]);
                }
            }
            synth_phase = new_phase;
        }
        first = false;
        // 位相の回転(出力の位相 − 入力の位相)を全チャンネルに掛けて戻す
        for (c, ch) in channels.iter().enumerate() {
            let spec = if c == 0 { x0.clone() } else { frame(ch, a) };
            let mut buf = vec![Complex::new(0.0f32, 0.0); PV_N];
            for k in 0..bins {
                let rot = Complex::from_polar(1.0, synth_phase[k] - phase0[k]);
                buf[k] = spec[k] * rot;
                if k > 0 && k < PV_N - k {
                    buf[PV_N - k] = buf[k].conj();
                }
            }
            ifft.process(&mut buf);
            let out = &mut outs[c];
            for i in 0..PV_N {
                out[out_start + i] += buf[i].re / PV_N as f32 * window[i];
            }
        }
        for i in 0..PV_N {
            wsum[out_start + i] += window[i] * window[i];
        }
        m += 1;
    }
    for out in outs.iter_mut() {
        for (o, w) in out.iter_mut().zip(&wsum) {
            if *w > 1e-3 {
                *o /= *w;
            }
        }
        out.truncate(out_len);
    }
    outs
}

/// 雑音っぽさ(スペクトルの平坦さの平均、0〜1)。打楽器・雑音の多い素材ほど大きい
fn noisiness(x: &[f32]) -> f32 {
    use rustfft::{num_complex::Complex, FftPlanner};
    let n = 2048;
    if x.len() < n {
        return 1.0;
    }
    let fft = FftPlanner::<f32>::new().plan_fft_forward(n);
    let step = (x.len() / 40).max(n);
    let (mut sum, mut count) = (0.0f32, 0usize);
    let mut pos = 0;
    while pos + n <= x.len() {
        let mut buf: Vec<Complex<f32>> = x[pos..pos + n]
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let w = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos();
                Complex::new(v * w, 0.0)
            })
            .collect();
        fft.process(&mut buf);
        // 約 100Hz〜10kHz(48kHz のとき)の平坦さ
        let p: Vec<f32> = buf[4..430].iter().map(|c| c.norm_sqr() + 1e-12).collect();
        let energy: f32 = p.iter().sum();
        if energy > 1e-6 {
            let geo = (p.iter().map(|v| v.ln()).sum::<f32>() / p.len() as f32).exp();
            sum += geo / (energy / p.len() as f32);
            count += 1;
        }
        pos += step;
    }
    if count == 0 {
        1.0
    } else {
        sum / count as f32
    }
}

/// 雑音っぽい(打楽器が中心の)素材とみなす平坦さ
const NOISY: f32 = 0.2;

/// 素材に合わせて伸縮の方法を選ぶ: 和音・持続音はフェーズボコーダ、打楽器・雑音の多い素材は WSOLA
pub fn stretch_channels(
    channels: &[&[f32]],
    sample_rate: f32,
    out_len: usize,
    src_pos: impl Fn(usize) -> f64,
) -> Vec<Vec<f32>> {
    let percussive = channels.first().map_or(true, |c| noisiness(c) > NOISY);
    if percussive {
        wsola_channels(channels, sample_rate, out_len, src_pos)
    } else {
        phase_vocoder_channels(channels, out_len, src_pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn sine(freq: f32, secs: f32) -> Vec<f32> {
        (0..(secs * SR) as usize)
            .map(|i| (std::f32::consts::TAU * freq * i as f32 / SR).sin() * 0.5)
            .collect()
    }

    /// 上向きゼロ交差の間隔から周波数を推定する
    fn freq_of(x: &[f32]) -> f32 {
        let crossings: Vec<usize> = (1..x.len())
            .filter(|&i| x[i - 1] < 0.0 && x[i] >= 0.0)
            .collect();
        let span = (crossings[crossings.len() - 1] - crossings[0]) as f32;
        (crossings.len() - 1) as f32 * SR / span
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    /// 和音(3 音)以外の成分の、全体に対する割合(dB)
    fn impurity_db(x: &[f32], notes: &[f32]) -> f32 {
        let (mut total, mut other) = (0.0f64, 0.0f64);
        let n = x.len() as f64;
        for b in 1..400 {
            let f = b as f64 * 10.0 + 5.0;
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (k, v) in x.iter().enumerate() {
                let w = 0.5 - 0.5 * (std::f64::consts::TAU * k as f64 / n).cos();
                let ph = k as f64 * f * std::f64::consts::TAU / SR as f64;
                re += *v as f64 * w * ph.cos();
                im += *v as f64 * w * ph.sin();
            }
            let pw = re * re + im * im;
            total += pw;
            if notes.iter().all(|nf| (f - *nf as f64).abs() > 25.0) {
                other += pw;
            }
        }
        (10.0 * (other / total).log10()) as f32
    }

    #[test]
    fn phase_vocoder_keeps_chords_cleaner_than_wsola() {
        // 和音を 1.5 倍に伸ばす: フェーズボコーダの方が和音以外の成分(にじみ・うなり)が少ない
        let notes = [220.0f32, 277.2, 329.6];
        let x: Vec<f32> = (0..(SR * 2.0) as usize)
            .map(|i| {
                notes
                    .iter()
                    .map(|f| (std::f32::consts::TAU * f * i as f32 / SR).sin())
                    .sum::<f32>()
                    * 0.2
            })
            .collect();
        let out_len = (x.len() as f32 * 1.5) as usize;
        let map = |i: usize| i as f64 / 1.5;
        let pv = phase_vocoder_channels(&[&x], out_len, map).remove(0);
        let ws = wsola(&x, SR, out_len, map);
        let (a, b) = (
            impurity_db(&pv[12_000..60_000], &notes),
            impurity_db(&ws[12_000..60_000], &notes),
        );
        assert!(a < b - 3.0, "フェーズボコーダ {a:.1} dB / WSOLA {b:.1} dB");
        // 音程と音量はそのまま
        let one =
            phase_vocoder_channels(&[&sine(440.0, 1.0)], 72_000, |i| i as f64 / 1.5).remove(0);
        assert!((freq_of(&one[10_000..60_000]) - 440.0).abs() < 2.0);
        assert!((rms(&one[10_000..60_000]) - 0.5 / 2f32.sqrt()).abs() < 0.05);
    }

    #[test]
    fn phase_vocoder_keeps_attacks_sharp() {
        // 0.25 秒ごとの減衰する打撃音(和音)を 1.25 倍に: 打点の鋭さ(最初の 5ms のピーク)が残る
        let hit = |i: usize| {
            let t = (i % 12_000) as f32 / SR;
            (-t / 0.05).exp()
                * ((std::f32::consts::TAU * 330.0 * i as f32 / SR).sin()
                    + (std::f32::consts::TAU * 440.0 * i as f32 / SR).sin())
                * 0.3
        };
        let x: Vec<f32> = (0..48_000).map(hit).collect();
        let y = phase_vocoder_channels(&[&x], 60_000, |i| i as f64 / 1.25).remove(0);
        let x_peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let y_peak = y[15_000..45_000].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(y_peak > x_peak * 0.7, "打点が残る: {y_peak} / {x_peak}");
    }

    #[test]
    fn chooses_the_method_by_content() {
        let chord: Vec<f32> = (0..48_000)
            .map(|i| {
                ((std::f32::consts::TAU * 220.0 * i as f32 / SR).sin()
                    + (std::f32::consts::TAU * 330.0 * i as f32 / SR).sin())
                    * 0.2
            })
            .collect();
        let mut r: u32 = 1;
        let drums: Vec<f32> = (0..48_000)
            .map(|i| {
                r ^= r << 13;
                r ^= r >> 17;
                r ^= r << 5;
                let t = (i % 12_000) as f32 / SR;
                (r as f32 / u32::MAX as f32 - 0.5) * (-t / 0.03).exp()
            })
            .collect();
        assert!(noisiness(&chord) < NOISY, "和音 {}", noisiness(&chord));
        assert!(noisiness(&drums) > NOISY, "打楽器 {}", noisiness(&drums));
    }

    #[test]
    fn identity_map_reproduces_input() {
        let x = sine(330.0, 1.0);
        let y = wsola(&x, SR, x.len(), |i| i as f64);
        let err: Vec<f32> = x[4800..40_000]
            .iter()
            .zip(&y[4800..40_000])
            .map(|(a, b)| a - b)
            .collect();
        assert!(rms(&err) < 1e-3, "等倍ならほぼ同じ波形: {}", rms(&err));
    }

    #[test]
    fn slower_and_faster_keep_pitch() {
        let x = sine(440.0, 1.0);
        for factor in [1.5f64, 0.7] {
            // factor 倍の長さにする(入力位置 = 出力位置 / factor)
            let out_len = (x.len() as f64 * factor) as usize;
            let y = wsola(&x, SR, out_len, |i| i as f64 / factor);
            assert_eq!(y.len(), out_len);
            let body = &y[2400..out_len - 4800];
            let f = freq_of(body);
            assert!((f - 440.0).abs() < 3.0, "音程は変わらない(×{factor}): {f}");
            assert!(rms(body) > 0.3, "音量も保つ(×{factor}): {}", rms(body));
        }
    }

    #[test]
    fn stops_when_input_runs_out() {
        let x = sine(220.0, 0.5);
        // 2 倍速で 1 秒ぶん要求 → 0.25 秒で入力を読み切り、その後は無音
        let y = wsola(&x, SR, 48_000, |i| i as f64 * 2.0);
        assert!(rms(&y[2400..9_600]) > 0.3);
        assert!(rms(&y[14_000..]) < 1e-6);
    }
}
