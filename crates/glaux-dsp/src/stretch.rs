//! タイムストレッチ(音程を変えずに長さを変える)。WSOLA 方式。
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
