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
    pub frames: Vec<f32>,
    pub sample_rate: f32,
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
        let s = frames[i] + (frames[i + 1] - frames[i]) * frac;

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

#[cfg(test)]
mod tests {
    use super::*;

    /// 440Hz サイン波 0.5 秒ぶんのサンプル
    fn test_sample(sr: f32) -> Arc<SampleData> {
        let frames = (0..(sr * 0.5) as usize)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / sr).sin() * 0.5)
            .collect();
        Arc::new(SampleData {
            frames,
            sample_rate: sr,
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
