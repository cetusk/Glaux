//! 多層サンプル(SFZ・SoundFont)のゾーンの波形。
//!
//! 大きな音源は波形が数百 MB になるので、16bit(波形のピークで正規化した倍率付き)で持つ(f32 の半分)。
//! 24bit の素材は、ピークから約 -96dB より下が丸められる(サンプルの再生には十分)。
//! SoundFont はもともと 16bit なので、そのまま写す。
//!
//! 音程を上げて鳴らすときの縮小版(段 k は長さ 1/2^k・帯域 1/2^k)は、ゾーンの鍵盤の範囲から要る段だけ作る
//! (多点サンプルの音源はほとんど 1 段で足りる)。段ごとに `OnceLock` で持つので、同じ波形を別の楽器が
//! もっと高く使うときは後から足せる。オーディオスレッドは読むだけ(確保もロックもしない)。

use std::sync::OnceLock;

/// 縮小版の段数の上限(2 オクターブ × 2 = 16 倍の速さまで)
pub const MIP_LEVELS: usize = 4;

pub struct Wave {
    pcm: Box<[i16]>,
    /// `pcm` × `scale` が値
    scale: f32,
    pub sample_rate: f32,
    /// 段 k + 1 の縮小版(16bit と倍率)。作っていない段は空
    mips: [OnceLock<(Box<[i16]>, f32)>; MIP_LEVELS],
}

impl std::fmt::Debug for Wave {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Wave({} frames @ {} Hz)",
            self.pcm.len(),
            self.sample_rate
        )
    }
}

/// 16bit にする(ピークで正規化)。無音なら倍率 0
fn quantize(x: &[f32]) -> (Box<[i16]>, f32) {
    let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if peak <= 0.0 || !peak.is_finite() {
        return (vec![0i16; x.len()].into_boxed_slice(), 0.0);
    }
    let scale = peak / i16::MAX as f32;
    let inv = 1.0 / scale;
    let pcm = x
        .iter()
        .map(|v| (v * inv).round().clamp(-(i16::MAX as f32), i16::MAX as f32) as i16)
        .collect();
    (pcm, scale)
}

/// [`crate::hermite`] の 16bit 版(値は `pcm` の単位)
#[inline]
fn hermite_i16(x: &[i16], i: usize, frac: f32) -> f32 {
    let x0 = x[i] as f32;
    let x1 = x[i + 1] as f32;
    let xm1 = if i > 0 { x[i - 1] as f32 } else { x0 };
    let x2 = x.get(i + 2).map_or(x1, |v| *v as f32);
    let c1 = 0.5 * (x1 - xm1);
    let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
    ((c3 * frac + c2) * frac + c1) * frac + x0
}

impl Wave {
    /// f32 の波形から(16bit に丸める)
    pub fn from_f32(x: &[f32], sample_rate: f32) -> Wave {
        let (pcm, scale) = quantize(x);
        Wave {
            pcm,
            scale,
            sample_rate,
            mips: Default::default(),
        }
    }

    /// 16bit の波形から(SoundFont。そのまま写す)
    pub fn from_i16(x: &[i16], sample_rate: f32) -> Wave {
        Wave {
            pcm: x.into(),
            scale: 1.0 / 32768.0,
            sample_rate,
            mips: Default::default(),
        }
    }

    pub fn len(&self) -> usize {
        self.pcm.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pcm.is_empty()
    }

    /// サンプル `i` の値
    #[inline]
    pub fn at(&self, i: usize) -> f32 {
        self.pcm[i] as f32 * self.scale
    }

    /// f32 の波形にする(解析・テスト用)
    pub fn to_f32(&self) -> Vec<f32> {
        self.pcm.iter().map(|v| *v as f32 * self.scale).collect()
    }

    /// 持っている大きさ(バイト。縮小版を含む)
    pub fn bytes(&self) -> usize {
        (self.pcm.len()
            + self
                .mips
                .iter()
                .filter_map(|m| m.get())
                .map(|(p, _)| p.len())
                .sum::<usize>())
            * 2
    }

    /// 作ってある縮小版の段数
    pub fn mip_levels(&self) -> usize {
        self.mips.iter().take_while(|m| m.get().is_some()).count()
    }

    /// 縮小版を段 `levels` まで用意する(オーディオスレッドの外で。作ってある段は作り直さない)
    pub fn prepare_mips(&self, levels: usize) {
        for k in 0..levels.min(MIP_LEVELS) {
            if self.mips[k].get().is_some() {
                continue;
            }
            let prev: Vec<f32> = if k == 0 {
                self.to_f32()
            } else {
                match self.mips[k - 1].get() {
                    Some((p, s)) => p.iter().map(|v| *v as f32 * s).collect(),
                    None => return,
                }
            };
            if prev.len() < 8 {
                return;
            }
            let _ = self.mips[k].set(quantize(&crate::sampler::halve(&prev)));
        }
    }

    /// 位置 `pos`(元のサンプルの番号、小数)の値を、1 サンプルあたり `step` 進む速さで鳴らすときの帯域で読む
    /// ([`crate::SampleData::read_cached`] と同じ)。作っていない段は、作ってあるいちばん細かい段で代える。
    /// 呼び出し側が `pos as usize + 1 < len()` を保証する
    #[inline]
    pub fn read_cached(&self, pos: f64, step: f64, level: &mut (f64, f64)) -> f32 {
        let base = |pos: f64| {
            let i = pos as usize;
            hermite_i16(&self.pcm, i, (pos - i as f64) as f32) * self.scale
        };
        if step <= 1.0 || self.mips[0].get().is_none() {
            return base(pos);
        }
        if level.0 != step {
            *level = (step, step.log2());
        }
        let lv = level.1.min(MIP_LEVELS as f64);
        let lo = lv.floor() as usize;
        let t = (lv - lo as f64) as f32;
        let at = |k: usize| -> f32 {
            // 作っていない段は、作ってある段まで下げる
            let mut k = k.min(MIP_LEVELS);
            while k > 0 && self.mips[k - 1].get().is_none() {
                k -= 1;
            }
            let Some((m, scale)) = k.checked_sub(1).and_then(|i| self.mips[i].get()) else {
                return base(pos);
            };
            let p = pos / (1u64 << k) as f64;
            let i = (p as usize).min(m.len().saturating_sub(2));
            hermite_i16(m, i, ((p - i as f64) as f32).clamp(0.0, 1.0)) * scale
        };
        if t <= 0.0 || lo >= MIP_LEVELS {
            at(lo)
        } else {
            at(lo) * (1.0 - t) + at(lo + 1) * t
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixteen_bits_keep_the_waveform() {
        let x: Vec<f32> = (0..4800)
            .map(|i| 0.3 * (std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0).sin())
            .collect();
        let w = Wave::from_f32(&x, 48_000.0);
        let err = x
            .iter()
            .zip(w.to_f32())
            .fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
        // ピークで正規化した 16bit なので、誤差はピークの 1/65534 以内
        assert!(err <= 0.3 / 32767.0 * 0.51, "{err}");
        assert_eq!(w.bytes(), 4800 * 2);
    }

    #[test]
    fn reads_like_the_f32_sample_data() {
        // 同じ波形を SampleData(f32、縮小版 4 段)と Wave(16bit、縮小版 4 段)で、いろいろな速さで読み比べる
        let x: Vec<f32> = (0..20_000)
            .map(|i| {
                let t = i as f32 / 48_000.0;
                0.4 * (std::f32::consts::TAU * 220.0 * t).sin()
                    + 0.2 * (std::f32::consts::TAU * 3_100.0 * t).sin()
            })
            .collect();
        let f = crate::SampleData::mono(x.clone(), 48_000.0);
        f.prepare_mips();
        let w = Wave::from_f32(&x, 48_000.0);
        w.prepare_mips(MIP_LEVELS);
        assert_eq!(w.mip_levels(), MIP_LEVELS);
        for step in [0.5, 1.0, 1.3, 2.0, 2.7, 5.5, 12.0] {
            let (mut a, mut b) = ((f64::NAN, 0.0), (f64::NAN, 0.0));
            let mut pos = 100.0;
            while pos < 1000.0 {
                let want = f.read_cached(pos, step, &mut a);
                let got = w.read_cached(pos, step, &mut b);
                assert!(
                    (want - got).abs() < 2e-4,
                    "step {step} pos {pos}: {want} / {got}"
                );
                pos += step;
            }
        }
    }

    #[test]
    fn levels_are_made_as_needed_and_can_be_added_later() {
        let x: Vec<f32> = (0..10_000)
            .map(|i| ((i * 37) % 101) as f32 / 101.0 - 0.5)
            .collect();
        let w = Wave::from_f32(&x, 48_000.0);
        assert_eq!(w.mip_levels(), 0);
        w.prepare_mips(1);
        assert_eq!(w.mip_levels(), 1);
        // 作っていない段(4 倍の速さ)は、作ってある段で読む(落ちない)
        let v = w.read_cached(500.0, 4.0, &mut (f64::NAN, 0.0));
        assert!(v.is_finite());
        w.prepare_mips(3);
        assert_eq!(w.mip_levels(), 3);
        assert!(w.bytes() > 10_000 * 2);
    }

    #[test]
    fn silence_stays_silent() {
        let w = Wave::from_f32(&[0.0; 100], 48_000.0);
        assert_eq!(w.at(10), 0.0);
        assert_eq!(w.read_cached(10.5, 1.0, &mut (f64::NAN, 0.0)), 0.0);
    }
}
