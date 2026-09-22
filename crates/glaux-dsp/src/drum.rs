//! ドラムシンセ `drum`。
//!
//! MIDI ノート番号(General MIDI 準拠)で音色を弾き分ける:
//! 35/36=キック, 38/40=スネア, 39=クラップ, 42/44=クローズドハット, 46=オープンハット,
//! 41..50 の奇数側=タム, 49/51 等=シンバル。その他は短いパーカッション。
//! すべて合成(サンプル不使用)なので RT セーフでアロケーションなし。

/// 焼き込み済みパラメータ(1 トラック分)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrumParams {
    /// リニアゲイン(dB から変換済み)
    pub gain: f32,
    /// 減衰時間の倍率 0.25..=4
    pub decay: f32,
    /// 明るさ 0..=1(ノイズ成分・クリック量)
    pub tone: f32,
    /// 半音単位のチューニング(キック・タムの音程)
    pub tune: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DrumSound {
    Kick,
    Snare,
    Clap,
    ClosedHat,
    OpenHat,
    Tom,
    Cymbal,
    Perc,
}

fn sound_for_pitch(pitch: u8) -> DrumSound {
    match pitch {
        35 | 36 => DrumSound::Kick,
        38 | 40 => DrumSound::Snare,
        39 => DrumSound::Clap,
        42 | 44 => DrumSound::ClosedHat,
        46 => DrumSound::OpenHat,
        41 | 43 | 45 | 47 | 48 | 50 => DrumSound::Tom,
        49 | 51 | 52 | 53 | 55 | 57 | 59 => DrumSound::Cymbal,
        _ => DrumSound::Perc,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DrumVoice {
    sound: DrumSound,
    pitch: u8,
    amp: f32,
    /// 発音からの経過サンプル
    t: u64,
    phase: f32,
    /// xorshift32 の状態(ノイズ用)
    rng: u32,
    /// ハイパス用 1 次ローパスの状態
    lp: f32,
    sample_rate: f32,
}

impl DrumVoice {
    pub fn start(p: &DrumParams, pitch: u8, vel: f32, sample_rate: f32) -> Self {
        let _ = p;
        DrumVoice {
            sound: sound_for_pitch(pitch),
            pitch,
            amp: vel,
            t: 0,
            phase: 0.0,
            rng: 0x9e37_79b9 ^ (pitch as u32) << 8 | 1,
            lp: 0.0,
            sample_rate,
        }
    }

    /// ドラムはワンショットなのでノートオフは無視する。
    pub fn note_off(&mut self) {}

    fn noise(&mut self) -> f32 {
        // xorshift32: RT セーフな擬似乱数
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    /// 各音色の全長(秒)。これを過ぎたら finished。
    fn duration(&self, p: &DrumParams) -> f32 {
        let base = match self.sound {
            DrumSound::Kick => 0.45,
            DrumSound::Snare => 0.25,
            DrumSound::Clap => 0.20,
            DrumSound::ClosedHat => 0.08,
            DrumSound::OpenHat => 0.40,
            DrumSound::Tom => 0.35,
            DrumSound::Cymbal => 0.9,
            DrumSound::Perc => 0.15,
        };
        base * p.decay
    }

    pub fn finished(&self, p: &DrumParams) -> bool {
        self.t as f32 / self.sample_rate > self.duration(p)
    }

    pub fn next(&mut self, p: &DrumParams) -> f32 {
        let sr = self.sample_rate;
        let t = self.t as f32 / sr;
        self.t += 1;
        let dur = self.duration(p);
        if t >= dur {
            return 0.0;
        }
        let tune = (2.0_f32).powf(p.tune / 12.0);
        // 音色ごとの合成。env は指数減衰
        let out = match self.sound {
            DrumSound::Kick => {
                // ピッチが急降下するサイン + クリック
                let freq = 48.0 * tune * (1.0 + 5.0 * (-t * 35.0).exp());
                self.phase += freq / sr;
                let body = (self.phase * std::f32::consts::TAU).sin() * (-t * 9.0 / p.decay).exp();
                let click = self.noise() * (-t * 300.0).exp() * p.tone * 0.6;
                body * 1.2 + click
            }
            DrumSound::Snare => {
                let freq = 190.0 * tune;
                self.phase += freq / sr;
                let body = (self.phase * std::f32::consts::TAU).sin() * (-t * 30.0).exp() * 0.5;
                let n = self.noise();
                // tone でノイズの明るさ(ハイパス量)を変える
                self.lp += (n - self.lp) * 0.25;
                let noise = (n - self.lp * (1.0 - p.tone)) * (-t * 14.0 / p.decay).exp();
                body + noise * (0.4 + 0.6 * p.tone)
            }
            DrumSound::Clap => {
                // 数ミリ秒ずれた 3 連バースト風の変調を掛けたノイズ
                let burst = 1.0 + 0.8 * (t * 220.0 * std::f32::consts::TAU).sin();
                let n = self.noise();
                self.lp += (n - self.lp) * 0.3;
                (n - self.lp * 0.7) * burst * (-t * 16.0 / p.decay).exp() * 0.8
            }
            DrumSound::ClosedHat | DrumSound::OpenHat | DrumSound::Cymbal => {
                let rate = match self.sound {
                    DrumSound::ClosedHat => 90.0,
                    DrumSound::OpenHat => 18.0,
                    _ => 7.0,
                };
                let n = self.noise();
                self.lp += (n - self.lp) * (0.15 + 0.3 * (1.0 - p.tone));
                (n - self.lp) * (-t * rate / p.decay).exp() * (0.5 + 0.5 * p.tone)
            }
            DrumSound::Tom => {
                let base = 110.0 * tune * (2.0_f32).powf((self.pitch as f32 - 45.0) / 12.0);
                let freq = base * (1.0 + 0.8 * (-t * 20.0).exp());
                self.phase += freq / sr;
                (self.phase * std::f32::consts::TAU).sin() * (-t * 10.0 / p.decay).exp()
            }
            DrumSound::Perc => {
                let freq = 400.0 * tune;
                self.phase += freq / sr;
                let body = (self.phase * std::f32::consts::TAU).sin() * 0.4;
                let n = self.noise() * p.tone;
                (body + n) * (-t * 25.0 / p.decay).exp()
            }
        };
        out * self.amp * p.gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> DrumParams {
        DrumParams {
            gain: 0.5,
            decay: 1.0,
            tone: 0.5,
            tune: 0.0,
        }
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32).sqrt()
    }

    fn render(pitch: u8, samples: usize) -> Vec<f32> {
        let p = params();
        let mut v = DrumVoice::start(&p, pitch, 1.0, 48_000.0);
        (0..samples).map(|_| v.next(&p)).collect()
    }

    #[test]
    fn kick_and_snare_sound_and_finish() {
        let p = params();
        for pitch in [36u8, 38, 42, 46, 45, 39, 49] {
            let out = render(pitch, 4800);
            assert!(rms(&out) > 0.01, "pitch {pitch} が鳴るはず");
            let mut v = DrumVoice::start(&p, pitch, 1.0, 48_000.0);
            for _ in 0..48_000 * 4 {
                v.next(&p);
                if v.finished(&p) {
                    break;
                }
            }
            assert!(v.finished(&p), "pitch {pitch} は減衰しきるはず");
        }
    }

    #[test]
    fn kick_is_darker_than_hat() {
        // キックは低域中心、ハットは高域中心 → 隣接差分エネルギー比で確認
        let hf = |out: &[f32]| out.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum::<f32>();
        let total = |out: &[f32]| out.iter().map(|s| s * s).sum::<f32>().max(1e-9);
        let kick = render(36, 4800);
        let hat = render(42, 2400);
        let kick_ratio = hf(&kick) / total(&kick);
        let hat_ratio = hf(&hat) / total(&hat);
        assert!(
            kick_ratio * 5.0 < hat_ratio,
            "kick={kick_ratio}, hat={hat_ratio}"
        );
    }

    #[test]
    fn decay_param_shortens_sound() {
        let short = DrumParams {
            decay: 0.25,
            ..params()
        };
        let long = DrumParams {
            decay: 2.0,
            ..params()
        };
        let mut v1 = DrumVoice::start(&short, 46, 1.0, 48_000.0);
        let mut v2 = DrumVoice::start(&long, 46, 1.0, 48_000.0);
        let mut n1 = 0;
        let mut n2 = 0;
        for _ in 0..48_000 * 2 {
            v1.next(&short);
            v2.next(&long);
            if !v1.finished(&short) {
                n1 += 1;
            }
            if !v2.finished(&long) {
                n2 += 1;
            }
        }
        assert!(n1 * 3 < n2, "decay が短いほど早く終わるはず({n1} vs {n2})");
    }
}
