//! 減算方式シンセ `subtractive`。
//!
//! PolyBLEP オシレータ(saw / square はエイリアシング低減済み)
//! → SVF(TPT 型)ローパス → ADSR。フィルタエンベロープでアタック時に
//! カットオフが開く、いわゆる「アナログシンセの基本形」。

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Waveform {
    Saw,
    Square,
    Triangle,
    Sine,
}

impl Waveform {
    pub fn parse(s: &str) -> Waveform {
        match s {
            "square" => Waveform::Square,
            "triangle" => Waveform::Triangle,
            "sine" => Waveform::Sine,
            _ => Waveform::Saw,
        }
    }
}

/// 焼き込み済みパラメータ(1 トラック分)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SubtractiveParams {
    pub waveform: Waveform,
    /// Hz
    pub cutoff: f32,
    /// 0..=0.95
    pub resonance: f32,
    /// 秒
    pub attack: f32,
    pub decay: f32,
    /// 0..=1
    pub sustain: f32,
    pub release: f32,
    /// エンベロープでカットオフを開く量 0..=1(1 で約 +3 オクターブ)
    pub filter_env: f32,
    /// リニアゲイン(dB から変換済み)
    pub gain: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EnvStage {
    Attack,
    Decay,
    Release,
}

#[derive(Clone, Copy, Debug)]
pub struct SubtractiveVoice {
    freq: f32,
    amp: f32,
    phase: f32,
    // ADSR
    stage: EnvStage,
    env: f32,
    // SVF 状態
    ic1: f32,
    ic2: f32,
    sample_rate: f32,
}

/// t ∈ [0,1) の位相不連続を滑らかにする PolyBLEP 補正。
fn poly_blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let t = t / dt;
        t + t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

impl SubtractiveVoice {
    pub fn start(p: &SubtractiveParams, freq: f32, vel: f32, sample_rate: f32) -> Self {
        let _ = p;
        SubtractiveVoice {
            freq,
            amp: vel,
            phase: 0.0,
            stage: EnvStage::Attack,
            env: 0.0,
            ic1: 0.0,
            ic2: 0.0,
            sample_rate,
        }
    }

    pub fn note_off(&mut self) {
        self.stage = EnvStage::Release;
    }

    pub fn finished(&self) -> bool {
        self.stage == EnvStage::Release && self.env < 1e-4
    }

    pub fn next(&mut self, p: &SubtractiveParams) -> f32 {
        let sr = self.sample_rate;

        // ---- ADSR(attack は線形、decay/release は指数) ----
        match self.stage {
            EnvStage::Attack => {
                self.env += 1.0 / (p.attack.max(0.0005) * sr);
                if self.env >= 1.0 {
                    self.env = 1.0;
                    self.stage = EnvStage::Decay;
                }
            }
            EnvStage::Decay => {
                let coef = 1.0 - 1.0 / (p.decay.max(0.005) * sr);
                self.env = p.sustain + (self.env - p.sustain) * coef;
            }
            EnvStage::Release => {
                let coef = 1.0 - 1.0 / (p.release.max(0.005) * sr);
                self.env *= coef;
            }
        }

        // ---- オシレータ ----
        let dt = self.freq / sr;
        let t = self.phase;
        let osc = match p.waveform {
            Waveform::Saw => 2.0 * t - 1.0 - poly_blep(t, dt),
            Waveform::Square => {
                let raw = if t < 0.5 { 1.0 } else { -1.0 };
                let t2 = if t + 0.5 >= 1.0 { t - 0.5 } else { t + 0.5 };
                raw + poly_blep(t, dt) - poly_blep(t2, dt)
            }
            Waveform::Triangle => 4.0 * (t - 0.5).abs() - 1.0,
            Waveform::Sine => (t * std::f32::consts::TAU).sin(),
        };
        self.phase += dt;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }

        // ---- SVF ローパス(TPT)。エンベロープでカットオフを開く ----
        let fc = (p.cutoff * (2.0_f32).powf(p.filter_env * self.env * 3.0)).min(sr * 0.45);
        let g = (std::f32::consts::PI * fc / sr).tan();
        let k = 2.0 * (1.0 - p.resonance.min(0.95));
        let a1 = 1.0 / (1.0 + g * (g + k));
        let v1 = a1 * (self.ic1 + g * (osc - self.ic2));
        let v2 = self.ic2 + g * v1;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        let lp = v2;

        lp * self.env * self.amp * p.gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_params() -> SubtractiveParams {
        SubtractiveParams {
            waveform: Waveform::Saw,
            cutoff: 8000.0,
            resonance: 0.15,
            attack: 0.005,
            decay: 0.15,
            sustain: 0.7,
            release: 0.05,
            filter_env: 0.35,
            gain: 0.35,
        }
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn produces_sound_then_decays_after_note_off() {
        let p = default_params();
        let mut v = SubtractiveVoice::start(&p, 220.0, 1.0, 48_000.0);
        let held: Vec<f32> = (0..4800).map(|_| v.next(&p)).collect();
        assert!(rms(&held) > 0.05, "発音中は音が出るはず");

        v.note_off();
        let mut tail = Vec::new();
        for _ in 0..48_000 {
            tail.push(v.next(&p));
            if v.finished() {
                break;
            }
        }
        assert!(v.finished(), "リリース後に減衰しきるはず");
        assert!(tail.last().unwrap().abs() < 1e-3);
    }

    #[test]
    fn cutoff_darkens_sound() {
        // カットオフを下げると高域が減る → 隣接サンプル差分のエネルギーが小さくなる
        let hf_energy = |cutoff: f32| {
            let p = SubtractiveParams {
                cutoff,
                filter_env: 0.0,
                ..default_params()
            };
            let mut v = SubtractiveVoice::start(&p, 220.0, 1.0, 48_000.0);
            let out: Vec<f32> = (0..9600).map(|_| v.next(&p)).collect();
            out.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum::<f32>()
        };
        assert!(
            hf_energy(400.0) < hf_energy(8000.0) * 0.3,
            "低カットオフはこもるはず"
        );
    }

    #[test]
    fn waveforms_differ() {
        let render = |w: Waveform| {
            let p = SubtractiveParams {
                waveform: w,
                ..default_params()
            };
            let mut v = SubtractiveVoice::start(&p, 220.0, 1.0, 48_000.0);
            (0..4800).map(|_| v.next(&p)).collect::<Vec<f32>>()
        };
        let saw = render(Waveform::Saw);
        let sine = render(Waveform::Sine);
        let diff: f32 = saw.iter().zip(&sine).map(|(a, b)| (a - b).abs()).sum();
        assert!(diff > 10.0, "波形で音が変わるはず");
    }
}
