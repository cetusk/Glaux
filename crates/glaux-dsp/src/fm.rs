//! FM シンセ `fm`(2 オペレーター + フィードバック。DX 系の基本形)。
//!
//! モジュレーター(周波数 = キャリア × ratio)でキャリアの位相を揺らす。変調の深さ(index)に
//! 専用の減衰を持たせると、鳴り始めだけ倍音が多く・すぐ丸くなる音(エレピ・ベル・マレット)が作れる。
//! 整数比は楽器らしい倍音、非整数比(例 3.5、1.41)は金属的・鐘のような非調和な響きになる。
//! 減算式(subtractive)では作れない音色の担当。
//!
//! RT セーフ: 値型のみでアロケーションなし。

use glaux_core::Articulation;

/// 焼き込み済みパラメータ(1 トラック分)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FmParams {
    /// モジュレーターの周波数比(キャリアに対する)
    pub ratio: f32,
    /// 変調の深さの最大値(ラジアン相当。0 = 正弦波)
    pub index: f32,
    /// 変調の深さが残る分まで減る時間(秒。この時間でほぼ落ち着く)
    pub index_decay: f32,
    /// 減った後に残る変調の深さ(index に対する割合)
    pub index_sustain: f32,
    /// モジュレーターの自己フィードバック 0..=1(上げるとノコギリ波寄りのざらつき)
    pub feedback: f32,
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    /// リニアゲイン(dB から変換済み)
    pub gain: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct FmVoice {
    freq: f32,
    car_phase: f32,
    mod_phase: f32,
    /// 直前 2 サンプルのモジュレーター出力の平均(フィードバック用。発振を抑える)
    fb_hist: [f32; 2],
    /// 変調の深さの包絡(1 → index_sustain)
    mod_env: f32,
    /// 振幅の包絡
    amp_env: f32,
    stage: Stage,
    vel: f32,
    /// ベロシティで変わる変調の深さの倍率(強く弾くと明るい)
    index_scale: f32,
    decay_mul: f32,
    pub(crate) expr: crate::expr::PitchExpr,
    sample_rate: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    Attack,
    Decay,
    Release,
}

impl FmVoice {
    pub fn start(
        _p: &FmParams,
        freq: f32,
        vel: f32,
        articulation: Articulation,
        sample_rate: f32,
    ) -> Self {
        let (vel, decay_mul) = match articulation {
            Articulation::Accent => ((vel * 1.3).min(1.0), 1.0),
            // ミュート: 減衰を速く
            Articulation::PalmMute => (vel, 0.25),
            _ => (vel, 1.0),
        };
        FmVoice {
            freq,
            car_phase: 0.0,
            mod_phase: 0.0,
            fb_hist: [0.0; 2],
            mod_env: 1.0,
            amp_env: 0.0,
            stage: Stage::Attack,
            vel,
            index_scale: 0.4 + 0.6 * vel,
            decay_mul,
            expr: crate::expr::PitchExpr::new(articulation, sample_rate),
            sample_rate,
        }
    }

    pub fn note_off(&mut self) {
        self.stage = Stage::Release;
    }

    pub fn finished(&self) -> bool {
        self.stage == Stage::Release && self.amp_env < 1e-4
    }

    pub fn next(&mut self, p: &FmParams) -> f32 {
        let sr = self.sample_rate;
        // 振幅の包絡(attack は線形、decay / release は指数)
        match self.stage {
            Stage::Attack => {
                self.amp_env += 1.0 / (p.attack.max(0.0005) * sr);
                if self.amp_env >= 1.0 {
                    self.amp_env = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            // decay / release は「その時間でほぼ消える(-60dB)」
            Stage::Decay => {
                let coef = (6.9 / ((p.decay * self.decay_mul).max(0.005) * sr)).min(1.0);
                self.amp_env += (p.sustain - self.amp_env) * coef;
            }
            Stage::Release => {
                let coef = (6.9 / ((p.release * self.decay_mul).max(0.005) * sr)).min(1.0);
                self.amp_env -= self.amp_env * coef;
            }
        }
        // 変調の深さの包絡(1 → sustain へ指数で。index_decay でほぼ落ち着く)
        let mcoef = (3.0 / ((p.index_decay * self.decay_mul).max(0.002) * sr)).min(1.0);
        self.mod_env += (p.index_sustain - self.mod_env) * mcoef;

        let ratio_expr = if self.expr.is_active() {
            self.expr.next_ratio(sr)
        } else {
            1.0
        };
        let f = self.freq * ratio_expr;
        let tau = std::f32::consts::TAU;
        let fb = p.feedback * 0.5 * (self.fb_hist[0] + self.fb_hist[1]);
        let m = (tau * self.mod_phase + fb * std::f32::consts::PI).sin();
        self.fb_hist = [self.fb_hist[1], m];
        let index = p.index * self.mod_env * self.index_scale;
        let out = (tau * self.car_phase + index * m).sin();

        self.car_phase += f / sr;
        self.car_phase -= self.car_phase.floor();
        self.mod_phase += f * p.ratio / sr;
        self.mod_phase -= self.mod_phase.floor();
        out * self.amp_env * self.vel * p.gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> FmParams {
        FmParams {
            ratio: 1.0,
            index: 3.0,
            index_decay: 0.3,
            index_sustain: 0.2,
            feedback: 0.0,
            attack: 0.002,
            decay: 1.0,
            sustain: 0.5,
            release: 0.2,
            gain: 0.5,
        }
    }

    fn render(p: &FmParams, n: usize) -> Vec<f32> {
        let mut v = FmVoice::start(p, 220.0, 0.8, Articulation::Normal, 48_000.0);
        (0..n).map(|_| v.next(p)).collect()
    }

    /// 零交差の数から、ざっくり高い倍音の多さを測る
    fn crossings(x: &[f32]) -> usize {
        x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count()
    }

    #[test]
    fn index_zero_is_a_sine_and_modulation_adds_brightness() {
        let mut sine = params();
        sine.index = 0.0;
        let s = render(&sine, 4800);
        // 0.1 秒で 220Hz ≈ 22 周期
        assert!((crossings(&s) as i32 - 22).abs() <= 1, "{}", crossings(&s));
        let mut bright = params();
        bright.index = 8.0;
        bright.ratio = 1.0;
        bright.index_sustain = 1.0;
        let b = render(&bright, 4800);
        assert!(crossings(&b) > crossings(&s) + 5, "変調で倍音が増える");
    }

    #[test]
    fn modulation_decays_like_an_electric_piano() {
        // 鳴り始めは明るく、後半は丸くなる(変調の深さが減る)
        let p = params();
        let x = render(&p, 48_000);
        let head = crossings(&x[..2400]);
        let tail = crossings(&x[43_200..45_600]);
        assert!(head > tail, "{head} vs {tail}");
    }

    #[test]
    fn releases_and_finishes() {
        let p = params();
        let mut v = FmVoice::start(&p, 440.0, 1.0, Articulation::Normal, 48_000.0);
        for _ in 0..4800 {
            v.next(&p);
        }
        v.note_off();
        for _ in 0..48_000 {
            v.next(&p);
        }
        assert!(v.finished());
    }
}
