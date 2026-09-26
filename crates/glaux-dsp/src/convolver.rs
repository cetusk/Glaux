//! 畳み込みリバーブ(実在の部屋などの響き = インパルス応答 IR を、音に畳み込む)。
//!
//! 一様分割の周波数領域の畳み込み(overlap-save、分割 512 サンプル)。入力はモノラル(左右の平均)、
//! 出力はステレオ(IR の左右)。左右の結果は実数なので、左 + j·右 を 1 回の逆 FFT でまとめて求める。
//! 遅れは 1 分割(512 サンプル、48kHz で約 10.7ms)で、原音も同じだけ遅らせて混ぜる(エンジンの遅延補正に申告)。
//!
//! バッファ・FFT の計画・IR のスペクトルはすべて [`ConvEngine::new`] で(オーディオスレッドの外で)用意する。
//! 状態はオーディオスレッドだけが書き換える前提で `UnsafeCell` に持ち、`busy` の旗で同時に 2 か所から
//! 触られないことを保証する(万一重なったら、その回は原音を返す)。

use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 分割の長さ(サンプル)
pub const PART: usize = 512;
/// IR の長さの上限(秒)
pub const MAX_IR_SECS: f32 = 8.0;

struct Inner {
    /// 入力をためる(この分割の分)
    in_buf: Vec<f32>,
    /// 1 つ前の分割の入力(overlap-save 用)
    prev: Vec<f32>,
    /// 過去の入力のスペクトル(輪。新しいものが `head`)
    fdl: Vec<Vec<Complex<f32>>>,
    head: usize,
    /// 出力(次の分割の間に 1 つずつ出す)と原音の遅れ
    out_l: Vec<f32>,
    out_r: Vec<f32>,
    dry_l: Vec<f32>,
    dry_r: Vec<f32>,
    pos: usize,
    /// 作業用
    work: Vec<Complex<f32>>,
    acc: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
}

pub struct ConvEngine {
    /// IR の左右のスペクトル(分割ごと、長さ 2·PART)
    ir_l: Vec<Vec<Complex<f32>>>,
    ir_r: Vec<Vec<Complex<f32>>>,
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    inner: UnsafeCell<Inner>,
    busy: AtomicBool,
}

// SAFETY: `inner` を書き換えるのは `process_block` だけで、`busy` の旗で同時に 1 か所しか入れない
unsafe impl Sync for ConvEngine {}

impl std::fmt::Debug for ConvEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ConvEngine({} 分割)", self.ir_l.len())
    }
}

/// 4 点補間でサンプルレートを変える(IR 用。オーディオスレッドの外で)
fn resample(x: &[f32], from: f32, to: f32) -> Vec<f32> {
    if (from - to).abs() < 0.5 || x.len() < 2 {
        return x.to_vec();
    }
    let step = from as f64 / to as f64;
    let n = ((x.len() - 1) as f64 / step) as usize;
    (0..n)
        .map(|k| {
            let p = k as f64 * step;
            let i = p as usize;
            crate::sampler::hermite(x, i, (p - i as f64) as f32)
        })
        .collect()
}

impl ConvEngine {
    /// IR(左右、`ir_sr` のサンプルレート)から作る。`length`(0.05〜1)で IR の後ろを短くする
    /// (終わりはなめらかに消す)。IR の後ろの無音(-80dB)は切る
    pub fn new(ir_l: &[f32], ir_r: &[f32], ir_sr: f32, sr: f32, length: f32) -> Self {
        let mut l = resample(ir_l, ir_sr, sr);
        let mut r = resample(ir_r, ir_sr, sr);
        let n = l.len().min(r.len());
        l.truncate(n);
        r.truncate(n);
        // 後ろの無音を切る
        let peak = l
            .iter()
            .chain(&r)
            .fold(0.0f32, |m, v| m.max(v.abs()))
            .max(1e-9);
        let last = (0..n)
            .rev()
            .find(|&i| l[i].abs().max(r[i].abs()) > peak * 1e-4)
            .map_or(1, |i| i + 1);
        let max = (MAX_IR_SECS * sr) as usize;
        let keep = ((last as f32 * length.clamp(0.05, 1.0)) as usize).clamp(1, max.max(1));
        l.truncate(keep);
        r.truncate(keep);
        // 短くしたときは最後の 20% をなめらかに消す
        if length < 1.0 {
            let fade = (keep / 5).max(1);
            for i in 0..fade {
                let g = 1.0 - i as f32 / fade as f32;
                l[keep - fade + i] *= g;
                r[keep - fade + i] *= g;
            }
        }
        // 大きさをそろえる: 左右のエネルギーの平均が 1 になるよう(IR ごとの音量のばらつきを消す)
        let energy = (l.iter().chain(&r).map(|v| v * v).sum::<f32>() / 2.0)
            .sqrt()
            .max(1e-9);
        l.iter_mut().chain(r.iter_mut()).for_each(|v| *v /= energy);

        let size = 2 * PART;
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(size);
        let ifft = planner.plan_fft_inverse(size);
        let parts = keep.div_ceil(PART).max(1);
        let spectra = |x: &[f32]| -> Vec<Vec<Complex<f32>>> {
            (0..parts)
                .map(|p| {
                    let mut buf = vec![Complex::new(0.0, 0.0); size];
                    for (i, v) in x.iter().skip(p * PART).take(PART).enumerate() {
                        buf[i] = Complex::new(*v, 0.0);
                    }
                    fft.process(&mut buf);
                    buf
                })
                .collect()
        };
        let (ir_l, ir_r) = (spectra(&l), spectra(&r));
        let scratch_len = fft
            .get_inplace_scratch_len()
            .max(ifft.get_inplace_scratch_len());
        ConvEngine {
            ir_l,
            ir_r,
            fft,
            ifft,
            inner: UnsafeCell::new(Inner {
                in_buf: vec![0.0; PART],
                prev: vec![0.0; PART],
                fdl: vec![vec![Complex::new(0.0, 0.0); size]; parts],
                head: 0,
                out_l: vec![0.0; PART],
                out_r: vec![0.0; PART],
                dry_l: vec![0.0; PART],
                dry_r: vec![0.0; PART],
                pos: 0,
                work: vec![Complex::new(0.0, 0.0); size],
                acc: vec![Complex::new(0.0, 0.0); size],
                scratch: vec![Complex::new(0.0, 0.0); scratch_len],
            }),
            busy: AtomicBool::new(false),
        }
    }

    /// 遅れ(サンプル)
    pub fn latency(&self) -> u32 {
        PART as u32
    }

    /// ブロックを処理する(オーディオスレッド。アロケーションなし)。`mix` はウェットの割合、`wet` はウェットの音量
    pub fn process_block(&self, l: &mut [f32], r: &mut [f32], mix: f32, wet: f32) {
        if self.busy.swap(true, Ordering::Acquire) {
            return;
        }
        // SAFETY: busy の旗で、ここに入っているのは 1 か所だけ
        let s = unsafe { &mut *self.inner.get() };
        let n = l.len().min(r.len());
        let size = 2 * PART;
        let norm = 1.0 / size as f32;
        for i in 0..n {
            let (x_l, x_r) = (l[i], r[i]);
            // 出力(1 分割遅れ)
            let (dl, dr) = (s.dry_l[s.pos], s.dry_r[s.pos]);
            let (wl, wr) = (s.out_l[s.pos], s.out_r[s.pos]);
            l[i] = dl * (1.0 - mix) + wl * wet * mix;
            r[i] = dr * (1.0 - mix) + wr * wet * mix;
            s.dry_l[s.pos] = x_l;
            s.dry_r[s.pos] = x_r;
            s.in_buf[s.pos] = 0.5 * (x_l + x_r);
            s.pos += 1;
            if s.pos < PART {
                continue;
            }
            s.pos = 0;
            // 1 分割たまった: [前の分割, この分割] の FFT を輪に入れる
            let parts = self.ir_l.len();
            s.head = (s.head + parts - 1) % parts;
            let slot = &mut s.fdl[s.head];
            for k in 0..PART {
                slot[k] = Complex::new(s.prev[k], 0.0);
                slot[PART + k] = Complex::new(s.in_buf[k], 0.0);
            }
            self.fft.process_with_scratch(slot, &mut s.scratch);
            s.prev.copy_from_slice(&s.in_buf);
            // Σ 過去の入力 × IR の分割(左は実部、右は虚部にまとめて 1 回の逆 FFT)
            s.acc.iter_mut().for_each(|c| *c = Complex::new(0.0, 0.0));
            let j = Complex::new(0.0, 1.0);
            for p in 0..parts {
                let x = &s.fdl[(s.head + p) % parts];
                let (hl, hr) = (&self.ir_l[p], &self.ir_r[p]);
                for k in 0..size {
                    s.acc[k] += x[k] * (hl[k] + j * hr[k]);
                }
            }
            s.work.copy_from_slice(&s.acc);
            self.ifft.process_with_scratch(&mut s.work, &mut s.scratch);
            // 後ろ半分が有効(overlap-save)
            for k in 0..PART {
                let v = s.work[PART + k] * norm;
                s.out_l[k] = v.re;
                s.out_r[k] = v.im;
            }
        }
        self.busy.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 畳み込みの答え(時間領域でそのまま)
    fn direct(x: &[f32], h: &[f32]) -> Vec<f32> {
        (0..x.len())
            .map(|n| {
                (0..h.len().min(n + 1))
                    .map(|k| h[k] * x[n - k])
                    .sum::<f32>()
            })
            .collect()
    }

    #[test]
    fn matches_direct_convolution_with_one_block_delay() {
        // 左右で違う短い IR。ウェットだけ(mix 1)
        let ir_l: Vec<f32> = (0..1500)
            .map(|i| (-(i as f32) / 300.0).exp() * if i % 7 == 0 { 1.0 } else { -0.3 })
            .collect();
        let ir_r: Vec<f32> = (0..1500)
            .map(|i| (-(i as f32) / 200.0).exp() * if i % 5 == 0 { 0.8 } else { 0.2 })
            .collect();
        let eng = ConvEngine::new(&ir_l, &ir_r, 48_000.0, 48_000.0, 1.0);
        // 大きさをそろえた後の IR で答えを作る
        let energy = ((ir_l.iter().chain(&ir_r).map(|v| v * v).sum::<f32>()) / 2.0).sqrt();
        let (hl, hr): (Vec<f32>, Vec<f32>) = (
            ir_l.iter().map(|v| v / energy).collect(),
            ir_r.iter().map(|v| v / energy).collect(),
        );
        let x: Vec<f32> = (0..6000)
            .map(|i| ((i * 7919) % 1000) as f32 / 1000.0 - 0.5)
            .collect();
        let (mut l, mut r) = (x.clone(), x.clone());
        // エンジンのブロックの長さと分割の長さはそろっていなくてよい
        for (a, b) in l.chunks_mut(300).zip(r.chunks_mut(300)) {
            eng.process_block(a, b, 1.0, 1.0);
        }
        let (want_l, want_r) = (direct(&x, &hl), direct(&x, &hr));
        for n in PART..6000 {
            assert!(
                (l[n] - want_l[n - PART]).abs() < 1e-3,
                "左 {n}: {} / {}",
                l[n],
                want_l[n - PART]
            );
            assert!((r[n] - want_r[n - PART]).abs() < 1e-3, "右 {n}");
        }
    }

    #[test]
    fn dry_is_delayed_and_length_shortens_the_tail() {
        let ir: Vec<f32> = (0..48_000)
            .map(|i| (-(i as f32) / 8000.0).exp() * ((i * 31) % 17) as f32 / 17.0)
            .collect();
        let eng = ConvEngine::new(&ir, &ir, 48_000.0, 48_000.0, 1.0);
        let mut l: Vec<f32> = (0..2048).map(|i| if i == 10 { 1.0 } else { 0.0 }).collect();
        let mut r = l.clone();
        eng.process_block(&mut l, &mut r, 0.0, 1.0);
        assert_eq!(l[10 + PART], 1.0, "原音は 1 分割遅れる");
        let short = ConvEngine::new(&ir, &ir, 48_000.0, 48_000.0, 0.25);
        assert!(short.ir_l.len() < eng.ir_l.len() / 3);
    }
}
