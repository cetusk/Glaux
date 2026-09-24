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
    /// ユニゾン本数 1..=7(supersaw)
    pub unison: u8,
    /// ユニゾンのデチューン幅(セント)
    pub detune_cents: f32,
    /// 1 オクターブ下のサブオシレータ量 0..=1
    pub sub: f32,
    /// ノイズ量 0..=1
    pub noise: f32,
    /// リニアゲイン(dB から変換済み)
    pub gain: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EnvStage {
    Attack,
    Decay,
    Release,
}

const MAX_UNISON: usize = 7;

/// ノート単位の奏法(アーティキュレーション)によるパラメータ倍率。
/// トラック共有の `SubtractiveParams` を書き換えずに、ボイス側で音を変える。
#[derive(Clone, Copy, Debug)]
struct ArtMod {
    cutoff_mul: f32,
    decay_mul: f32,
    sustain_mul: f32,
    release_mul: f32,
    amp_mul: f32,
}

impl ArtMod {
    fn from(a: glaux_core::Articulation) -> ArtMod {
        use glaux_core::Articulation as A;
        match a {
            // レガート・ポルタメントはエンジン側(つなぎ目のフェードと音程の滑り)で表現する
            A::Normal | A::Staccato | A::Vibrato | A::Bend | A::Legato | A::Portamento => ArtMod {
                cutoff_mul: 1.0,
                decay_mul: 1.0,
                sustain_mul: 1.0,
                // スタッカートは音価の短縮(エンジン側)が主。切れ際だけ締める
                release_mul: if a == A::Staccato { 0.5 } else { 1.0 },
                amp_mul: 1.0,
            },
            // こもった「ズンズン」: カットオフを絞り、速く減衰しきる
            A::PalmMute => ArtMod {
                cutoff_mul: 0.3,
                decay_mul: 0.18,
                sustain_mul: 0.0,
                release_mul: 0.6,
                amp_mul: 1.0,
            },
            // その音だけ強く・明るく
            A::Accent => ArtMod {
                cutoff_mul: 1.5,
                decay_mul: 1.0,
                sustain_mul: 1.0,
                release_mul: 1.0,
                amp_mul: 1.4,
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SubtractiveVoice {
    freq: f32,
    amp: f32,
    /// 奏法によるボイス固有の倍率
    art: ArtMod,
    /// ピッチ表現(ビブラート / チョーキング)
    pub(crate) expr: crate::expr::PitchExpr,
    /// ユニゾン各声部の位相
    phases: [f32; MAX_UNISON],
    /// サブオシレータの位相
    sub_phase: f32,
    /// ノイズ用 xorshift 状態
    rng: u32,
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
    pub fn start(
        p: &SubtractiveParams,
        freq: f32,
        vel: f32,
        articulation: glaux_core::Articulation,
        sample_rate: f32,
    ) -> Self {
        let _ = p;
        let art = ArtMod::from(articulation);
        // 各声部の初期位相をずらす(揃っていると立ち上がりが位相打ち消しでうねる)
        let mut phases = [0.0f32; MAX_UNISON];
        for (i, ph) in phases.iter_mut().enumerate() {
            *ph = (i as f32 * 0.371) % 1.0;
        }
        SubtractiveVoice {
            freq,
            amp: vel * art.amp_mul,
            art,
            expr: crate::expr::PitchExpr::new(articulation, sample_rate),
            phases,
            sub_phase: 0.0,
            rng: (freq.to_bits() | 1).wrapping_mul(0x9e37_79b9),
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

    /// レガート: 立ち上がりを飛ばして、鳴り続けている状態(サスティン。減衰しきる音は 0.35)から始める
    pub fn skip_attack(&mut self, p: &SubtractiveParams) {
        self.env = (p.sustain * self.art.sustain_mul).clamp(0.35, 1.0);
        self.stage = EnvStage::Decay;
    }

    pub fn finished(&self) -> bool {
        self.stage == EnvStage::Release && self.env < 1e-4
    }

    pub fn next(&mut self, p: &SubtractiveParams) -> f32 {
        let sr = self.sample_rate;

        // ---- ADSR(attack は線形、decay/release は指数)。奏法の倍率を反映 ----
        let sustain = p.sustain * self.art.sustain_mul;
        match self.stage {
            EnvStage::Attack => {
                self.env += 1.0 / (p.attack.max(0.0005) * sr);
                if self.env >= 1.0 {
                    self.env = 1.0;
                    self.stage = EnvStage::Decay;
                }
            }
            EnvStage::Decay => {
                let coef = 1.0 - 1.0 / ((p.decay * self.art.decay_mul).max(0.005) * sr);
                self.env = sustain + (self.env - sustain) * coef;
                // サスティン 0(プラック・パームミュート等)で減衰しきったら
                // リリース扱いにして finished でボイスを解放できるようにする
                if self.env < 1e-4 {
                    self.stage = EnvStage::Release;
                }
            }
            EnvStage::Release => {
                let coef = 1.0 - 1.0 / ((p.release * self.art.release_mul).max(0.005) * sr);
                self.env *= coef;
            }
        }

        // ---- ピッチ表現(ビブラート / チョーキング) ----
        let base_freq = if self.expr.is_active() {
            self.freq * self.expr.next_ratio(sr)
        } else {
            self.freq
        };

        // ---- オシレータ(ユニゾン対応) ----
        let n = (p.unison as usize).clamp(1, MAX_UNISON);
        let mut osc = 0.0f32;
        for i in 0..n {
            // 声部を中心対称にデチューン(-1..+1)
            let spread = if n == 1 {
                0.0
            } else {
                (i as f32 / (n - 1) as f32) * 2.0 - 1.0
            };
            let ratio = (2.0f32).powf(spread * p.detune_cents / 1200.0);
            let dt = base_freq * ratio / sr;
            let t = self.phases[i];
            osc += match p.waveform {
                Waveform::Saw => 2.0 * t - 1.0 - poly_blep(t, dt),
                Waveform::Square => {
                    let raw = if t < 0.5 { 1.0 } else { -1.0 };
                    let t2 = if t + 0.5 >= 1.0 { t - 0.5 } else { t + 0.5 };
                    raw + poly_blep(t, dt) - poly_blep(t2, dt)
                }
                Waveform::Triangle => 4.0 * (t - 0.5).abs() - 1.0,
                Waveform::Sine => (t * std::f32::consts::TAU).sin(),
            };
            self.phases[i] += dt;
            if self.phases[i] >= 1.0 {
                self.phases[i] -= 1.0;
            }
        }
        // 本数で音量が膨らみすぎないよう等パワー正規化
        osc /= (n as f32).sqrt();

        // サブオシレータ(1 オクターブ下のサイン。ベースの土台)
        if p.sub > 0.0 {
            self.sub_phase += base_freq * 0.5 / sr;
            if self.sub_phase >= 1.0 {
                self.sub_phase -= 1.0;
            }
            osc += (self.sub_phase * std::f32::consts::TAU).sin() * p.sub;
        }

        // ノイズ(息・ざらつき)
        if p.noise > 0.0 {
            let mut x = self.rng;
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            self.rng = x;
            osc += ((x as f32 / u32::MAX as f32) * 2.0 - 1.0) * p.noise;
        }

        // ---- SVF ローパス(TPT)。エンベロープでカットオフを開く ----
        let fc = (p.cutoff * self.art.cutoff_mul * (2.0_f32).powf(p.filter_env * self.env * 3.0))
            .clamp(40.0, sr * 0.45);
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
            unison: 1,
            detune_cents: 12.0,
            sub: 0.0,
            noise: 0.0,
            gain: 0.35,
        }
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn produces_sound_then_decays_after_note_off() {
        let p = default_params();
        let mut v =
            SubtractiveVoice::start(&p, 220.0, 1.0, glaux_core::Articulation::Normal, 48_000.0);
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
            let mut v =
                SubtractiveVoice::start(&p, 220.0, 1.0, glaux_core::Articulation::Normal, 48_000.0);
            let out: Vec<f32> = (0..9600).map(|_| v.next(&p)).collect();
            out.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum::<f32>()
        };
        assert!(
            hf_energy(400.0) < hf_energy(8000.0) * 0.3,
            "低カットオフはこもるはず"
        );
    }

    #[test]
    fn unison_thickens_and_sub_noise_add_energy() {
        let render = |p: SubtractiveParams| {
            let mut v =
                SubtractiveVoice::start(&p, 220.0, 1.0, glaux_core::Articulation::Normal, 48_000.0);
            (0..9600).map(|_| v.next(&p)).collect::<Vec<f32>>()
        };
        let single = render(default_params());
        let super_saw = render(SubtractiveParams {
            unison: 7,
            detune_cents: 25.0,
            ..default_params()
        });
        // デチューンで波形が変わる(単純な一致はしない)
        let diff: f32 = single
            .iter()
            .zip(&super_saw)
            .map(|(a, b)| (a - b).abs())
            .sum();
        assert!(diff > 10.0, "ユニゾンで音が変わるはず");
        assert!(rms(&super_saw) > 0.05);

        let with_sub = render(SubtractiveParams {
            sub: 0.8,
            cutoff: 400.0,
            filter_env: 0.0,
            ..default_params()
        });
        let without = render(SubtractiveParams {
            sub: 0.0,
            cutoff: 400.0,
            filter_env: 0.0,
            ..default_params()
        });
        assert!(
            rms(&with_sub) > rms(&without) * 1.1,
            "サブオシレータで低域が増えるはず"
        );
    }

    #[test]
    fn palm_mute_decays_fast_and_darkens() {
        use glaux_core::Articulation;
        let p = default_params();
        let render = |a: Articulation| {
            let mut v = SubtractiveVoice::start(&p, 110.0, 1.0, a, 48_000.0);
            (0..24_000).map(|_| v.next(&p)).collect::<Vec<f32>>()
        };
        let normal = render(Articulation::Normal);
        let muted = render(Articulation::PalmMute);

        // 0.5 秒経過時点(sustain 継続 vs 減衰しきり)の残エネルギー差
        let late_rms = |v: &[f32]| rms(&v[19_200..]);
        assert!(
            late_rms(&muted) < late_rms(&normal) * 0.2,
            "パームミュートは早く減衰しきるはず: muted={} normal={}",
            late_rms(&muted),
            late_rms(&normal)
        );

        // 立ち上がり(最初の 0.1 秒)は音が出ている(無音になっては困る)
        assert!(rms(&muted[..4_800]) > 0.03, "頭のアタックは鳴るはず");

        // 高域エネルギー(隣接差分)が小さい = こもっている
        let hf = |v: &[f32]| {
            v[..4_800]
                .windows(2)
                .map(|w| (w[1] - w[0]).powi(2))
                .sum::<f32>()
        };
        assert!(hf(&muted) < hf(&normal) * 0.5, "こもった音になるはず");
    }

    #[test]
    fn accent_is_louder() {
        use glaux_core::Articulation;
        let p = default_params();
        let render = |a: Articulation| {
            let mut v = SubtractiveVoice::start(&p, 220.0, 0.6, a, 48_000.0);
            (0..9_600).map(|_| v.next(&p)).collect::<Vec<f32>>()
        };
        assert!(
            rms(&render(Articulation::Accent)) > rms(&render(Articulation::Normal)) * 1.15,
            "アクセントは目立って大きいはず"
        );
    }

    #[test]
    fn vibrato_wobbles_after_onset() {
        use glaux_core::Articulation;
        let p = SubtractiveParams {
            waveform: Waveform::Sine,
            filter_env: 0.0,
            ..default_params()
        };
        let render = |a: Articulation| {
            let mut v = SubtractiveVoice::start(&p, 220.0, 1.0, a, 48_000.0);
            (0..48_000).map(|_| v.next(&p)).collect::<Vec<f32>>()
        };
        let normal = render(Articulation::Normal);
        let vib = render(Articulation::Vibrato);
        // 出だし(揺れる前)はほぼ同じ
        let head_diff: f32 = normal[..2_400]
            .iter()
            .zip(&vib[..2_400])
            .map(|(a, b)| (a - b).abs())
            .sum();
        // 後半は位相がずれて大きく異なる
        let tail_diff: f32 = normal[24_000..]
            .iter()
            .zip(&vib[24_000..])
            .map(|(a, b)| (a - b).abs())
            .sum();
        assert!(
            tail_diff > head_diff * 20.0,
            "後半にかけて揺れが深くなるはず: head={head_diff} tail={tail_diff}"
        );
    }

    #[test]
    fn waveforms_differ() {
        let render = |w: Waveform| {
            let p = SubtractiveParams {
                waveform: w,
                ..default_params()
            };
            let mut v =
                SubtractiveVoice::start(&p, 220.0, 1.0, glaux_core::Articulation::Normal, 48_000.0);
            (0..4800).map(|_| v.next(&p)).collect::<Vec<f32>>()
        };
        let saw = render(Waveform::Saw);
        let sine = render(Waveform::Sine);
        let diff: f32 = saw.iter().zip(&sine).map(|(a, b)| (a - b).abs()).sum();
        assert!(diff > 10.0, "波形で音が変わるはず");
    }
}
