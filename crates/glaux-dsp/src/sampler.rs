//! 単一サンプル再生の音源 `sampler`(ワンショット)。
//!
//! 1 つの WAV をトラックの音源にし、`root`(サンプル自身の音程)からの
//! ピッチ差を再生レートに変換して鳴らす。実録の質感(本物のギター、
//! ボーカルチョップ、生ドラムのワンショット等)はシンセでは出ないのでこれを使う。
//!
//! - 波形データは `Arc<SampleData>` で共有(構築時に読み込み済み。
//!   オーディオスレッドは読むだけでアロケーションしない)
//! - ループ再生・複数サンプルのマッピング(音域ごとの切り替え)は将来課題

use crate::expr::PitchExpr;
use glaux_core::Articulation;
use std::sync::Arc;

/// 読み込み済みのサンプル波形(モノラルに合算済み)。
#[derive(Debug)]
pub struct SampleData {
    /// モノラル成分(ステレオ素材なら左右の平均 M = (L + R) / 2)。解析・譜起こし・波形表示はこれを使う
    pub frames: Vec<f32>,
    pub sample_rate: f32,
    /// ステレオ素材の左右差成分 S = (L − R) / 2(L = M + S、R = M − S)。モノラル素材は None
    pub side: Option<Vec<f32>>,
}

impl SampleData {
    /// モノラルの波形。
    pub fn mono(frames: Vec<f32>, sample_rate: f32) -> Self {
        SampleData {
            frames,
            sample_rate,
            side: None,
        }
    }

    /// 左右の波形から(同じ長さであること)。
    pub fn stereo(left: &[f32], right: &[f32], sample_rate: f32) -> Self {
        let frames = left.iter().zip(right).map(|(l, r)| (l + r) * 0.5).collect();
        let side = left.iter().zip(right).map(|(l, r)| (l - r) * 0.5).collect();
        SampleData {
            frames,
            sample_rate,
            side: Some(side),
        }
    }

    /// 左右の波形(モノラルなら同じもの)。
    pub fn left_right(&self) -> (Vec<f32>, Vec<f32>) {
        match &self.side {
            Some(side) => (
                self.frames.iter().zip(side).map(|(m, s)| m + s).collect(),
                self.frames.iter().zip(side).map(|(m, s)| m - s).collect(),
            ),
            None => (self.frames.clone(), self.frames.clone()),
        }
    }
}

/// 焼き込み済みパラメータ(1 トラック分)。
#[derive(Clone, Debug)]
pub struct SamplerParams {
    pub data: Arc<SampleData>,
    /// サンプル自身の音程(MIDI ノート番号)。この音で等速再生になる
    pub root: u8,
    /// リニアゲイン(dB から変換済み)
    pub gain: f32,
    /// note_off 後のフェード係数(1 サンプルあたり)
    pub release_coef: f32,
}

impl PartialEq for SamplerParams {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.data, &other.data)
            && self.root == other.root
            && self.gain == other.gain
            && self.release_coef == other.release_coef
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SamplerVoice {
    /// 再生位置(サンプル、波形のネイティブレート基準)
    pos: f64,
    /// 1 出力サンプルあたりの進み(ピッチ + レート変換込み)
    rate: f64,
    amp: f32,
    /// 頭のデクリック(約 2ms)
    attack_env: f32,
    attack_inc: f32,
    release_env: f32,
    released: bool,
    /// 波形を最後まで読み切った
    done: bool,
    pub(crate) expr: PitchExpr,
    sample_rate: f32,
}

impl SamplerVoice {
    pub fn start(
        p: &SamplerParams,
        pitch: u8,
        vel: f32,
        articulation: Articulation,
        sample_rate: f32,
    ) -> Self {
        let amp_mul = if articulation == Articulation::Accent {
            1.3
        } else {
            1.0
        };
        let semis = pitch as f64 - p.root as f64;
        let rate = (p.data.sample_rate as f64 / sample_rate as f64) * (2.0_f64).powf(semis / 12.0);
        SamplerVoice {
            pos: 0.0,
            rate,
            amp: vel * amp_mul,
            attack_env: 0.0,
            attack_inc: 1.0 / (0.002 * sample_rate),
            release_env: 1.0,
            released: false,
            done: false,
            expr: PitchExpr::new(articulation, sample_rate),
            sample_rate,
        }
    }

    pub fn note_off(&mut self) {
        self.released = true;
    }

    pub fn finished(&self) -> bool {
        self.done
    }

    pub fn next(&mut self, p: &SamplerParams) -> f32 {
        if self.done {
            return 0.0;
        }
        let frames = &p.data.frames;
        let i = self.pos as usize;
        if i + 1 >= frames.len() {
            self.done = true;
            return 0.0;
        }
        let frac = (self.pos - i as f64) as f32;
        let s = hermite(frames, i, frac);

        // ピッチ表現(ビブラート / チョーキング)はレートに掛ける
        let ratio = if self.expr.is_active() {
            self.expr.next_ratio(self.sample_rate) as f64
        } else {
            1.0
        };
        self.pos += self.rate * ratio;

        if self.attack_env < 1.0 {
            self.attack_env = (self.attack_env + self.attack_inc).min(1.0);
        }
        if self.released {
            self.release_env *= 1.0 - p.release_coef;
            if self.release_env < 1e-4 {
                self.done = true;
            }
        }
        s * self.amp * self.attack_env * self.release_env * p.gain
    }
}

/// 4 点 3 次 Hermite 補間(Catmull-Rom)。`frames[i]` と `frames[i + 1]` の間の `frac`(0..1)の位置の値。
/// 呼び出し側が `i + 1 < frames.len()` を保証する。前後の点が範囲外なら端の値で代わりにする。
/// 以前は線形補間で、素材とエンジンのサンプルレートが違う(44.1k の素材など)と、高い音域で
/// 折り返し雑音が出て高域もこもった
#[inline]
pub fn hermite(frames: &[f32], i: usize, frac: f32) -> f32 {
    let x0 = frames[i];
    let x1 = frames[i + 1];
    let xm1 = if i > 0 { frames[i - 1] } else { x0 };
    let x2 = frames.get(i + 2).copied().unwrap_or(x1);
    let c1 = 0.5 * (x1 - xm1);
    let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
    ((c3 * frac + c2) * frac + c1) * frac + x0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hermite_hits_the_points_and_is_smoother_than_linear() {
        let f = [0.0f32, 1.0, 0.0, -1.0, 0.0];
        assert_eq!(hermite(&f, 1, 0.0), 1.0);
        assert!((hermite(&f, 1, 1.0) - 0.0).abs() < 1e-6);
        // サイン波の途中の値は線形補間より本来の値に近い
        let sr = 48_000.0f32;
        let w: Vec<f32> = (0..64)
            .map(|k| (k as f32 * 5_000.0 * std::f32::consts::TAU / sr).sin())
            .collect();
        let (mut e_lin, mut e_her) = (0.0f32, 0.0f32);
        for k in 2..60 {
            let t = k as f32 + 0.5;
            let truth = (t * 5_000.0 * std::f32::consts::TAU / sr).sin();
            e_lin += (w[k] + (w[k + 1] - w[k]) * 0.5 - truth).abs();
            e_her += (hermite(&w, k, 0.5) - truth).abs();
        }
        assert!(e_her < e_lin * 0.5, "Hermite {e_her} / 線形 {e_lin}");
    }

    /// 440Hz サイン波 0.5 秒ぶんのサンプル
    fn test_sample(sr: f32) -> Arc<SampleData> {
        let frames = (0..(sr * 0.5) as usize)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / sr).sin() * 0.5)
            .collect();
        Arc::new(SampleData {
            frames,
            sample_rate: sr,
            side: None,
        })
    }

    fn params(root: u8) -> SamplerParams {
        SamplerParams {
            data: test_sample(48_000.0),
            root,
            gain: 1.0,
            release_coef: 1.0 / (0.03 * 48_000.0),
        }
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32).sqrt()
    }

    fn crossings(v: &[f32]) -> usize {
        v.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count()
    }

    #[test]
    fn plays_at_root_and_transposes_octave() {
        let p = params(60);
        let render = |pitch: u8| {
            let mut v = SamplerVoice::start(&p, pitch, 1.0, Articulation::Normal, 48_000.0);
            (0..12_000).map(|_| v.next(&p)).collect::<Vec<f32>>()
        };
        let base = render(60); // 等速 = 440Hz
        let octave = render(72); // 2 倍速 = 880Hz
        assert!(rms(&base) > 0.1, "root では等速で鳴るはず");
        let ratio = crossings(&octave) as f32 / crossings(&base) as f32;
        assert!(
            (ratio - 2.0).abs() < 0.1,
            "1 オクターブ上は 2 倍の周波数のはず: {ratio}"
        );
    }

    #[test]
    fn stops_at_sample_end_and_fades_on_note_off() {
        let p = params(60);
        // サンプル終端(0.5 秒)で自然に終わる
        let mut v = SamplerVoice::start(&p, 60, 1.0, Articulation::Normal, 48_000.0);
        for _ in 0..30_000 {
            v.next(&p);
        }
        assert!(v.finished(), "波形を読み切ったら finished のはず");

        // note_off で 30ms(時定数)フェード → 十分な猶予の後には消えている
        let mut v = SamplerVoice::start(&p, 60, 1.0, Articulation::Normal, 48_000.0);
        for _ in 0..4_800 {
            v.next(&p);
        }
        v.note_off();
        let mut out_after = 0.0f32;
        for i in 0..20_000 {
            let s = v.next(&p).abs();
            if i > 9_600 {
                out_after = out_after.max(s);
            }
        }
        assert!(v.finished(), "note_off 後は消えるはず");
        assert!(out_after < 1e-2, "フェード後はほぼ無音のはず: {out_after}");
    }
}
