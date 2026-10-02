//! 減算方式シンセ `subtractive`。
//!
//! PolyBLEP オシレータ(saw / square はエイリアシング低減済み、triangle は帯域制限した矩形の積分)
//! → SVF(TPT 型)フィルタ → ADSR。フィルタエンベロープでアタック時に
//! カットオフが開く、いわゆる「アナログシンセの基本形」。
//! 左右の広がり・揺らぎ・フィルタの種類・LFO などの共通部品は [`crate::tone`]。

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

/// 雑音の色
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NoiseColor {
    /// 全帯域が同じ強さ(シャーッ)
    White,
    /// 高い方ほど弱い(-3dB/オクターブ。雨・テープのヒス)
    Pink,
    /// さらに低い方に寄る(-6dB/オクターブ。風・ゴー)
    Brown,
}

impl NoiseColor {
    pub fn parse(s: &str) -> NoiseColor {
        match s {
            "pink" => NoiseColor::Pink,
            "brown" => NoiseColor::Brown,
            _ => NoiseColor::White,
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
    /// ノイズの色
    pub noise_color: NoiseColor,
    /// 元の波形の量 0..=1(0 で雑音とサブだけ。レコードノイズ・風・ライザー)
    pub osc_level: f32,
    /// レコードのパチパチ(まれな短いクリック)の多さ 0..=1
    pub crackle: f32,
    /// リニアゲイン(dB から変換済み)
    pub gain: f32,
    /// 広がり・揺らぎ・フィルタの種類・LFO など(既定値は従来と同じ音)
    pub tone: crate::tone::ToneParams,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EnvStage {
    Attack,
    Decay,
    Release,
}

const MAX_UNISON: usize = crate::tone::MAX_UNISON;

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
    /// ピンク(3 段の 1 次フィルタ)・ブラウン(積分)の状態
    pink: [f32; 3],
    brown: f32,
    /// パチパチの残り(短く減衰するクリック)
    click: f32,
    /// triangle の積分の値(声部ごと)
    tri: [f32; MAX_UNISON],
    // ADSR
    stage: EnvStage,
    env: f32,
    /// ベロシティ(カットオフの追従用。amp は奏法の倍率込み)
    vel: f32,
    /// 広がり・揺らぎ・フィルタ・LFO の状態
    tone: crate::tone::ToneVoice,
    /// フィルタの係数(制御レートで更新)と、次の更新までのサンプル数
    coefs: crate::tone::SvfCoefs,
    ctrl: u32,
    /// ユニゾンの声部ごとの周波数の倍率と、それを作ったときの (声部数, デチューン)
    ratios: [f32; MAX_UNISON],
    ratio_key: (usize, u32),
    sample_rate: f32,
}

/// フィルタ係数を更新する間隔(サンプル)。エンベロープによるカットオフの動きはこの粒度で十分なめらか
const CTRL_RATE: u32 = 32;

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
    /// −1〜1 の白色雑音(xorshift32)
    fn white(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    pub fn start(
        p: &SubtractiveParams,
        freq: f32,
        vel: f32,
        articulation: glaux_core::Articulation,
        sample_rate: f32,
    ) -> Self {
        Self::start_seeded(p, freq, vel, articulation, sample_rate, freq.to_bits())
    }

    /// 揺らぎの種を渡して鳴らす(音ごとに違う種で、同じ音を繰り返しても少しずつ違う音になる)
    pub fn start_seeded(
        p: &SubtractiveParams,
        freq: f32,
        vel: f32,
        articulation: glaux_core::Articulation,
        sample_rate: f32,
        seed: u32,
    ) -> Self {
        let art = ArtMod::from(articulation);
        // 各声部の初期位相をずらす(揃っていると立ち上がりが位相打ち消しでうねる)
        let mut phases = [0.0f32; MAX_UNISON];
        for (i, ph) in phases.iter_mut().enumerate() {
            *ph = (i as f32 * 0.371) % 1.0;
        }
        let mut tone = crate::tone::ToneVoice::new(seed);
        tone.start_phases(&p.tone, &mut phases);
        // triangle の積分の初期値は、その位相の三角波の値
        let mut tri = [0.0f32; MAX_UNISON];
        for (t, ph) in tri.iter_mut().zip(&phases) {
            *t = 4.0 * (ph - 0.5).abs() - 1.0;
        }
        SubtractiveVoice {
            freq,
            amp: vel * art.amp_mul,
            art,
            expr: crate::expr::PitchExpr::new(articulation, sample_rate),
            phases,
            sub_phase: 0.0,
            rng: (freq.to_bits() | 1).wrapping_mul(0x9e37_79b9),
            pink: [0.0; 3],
            brown: 0.0,
            click: 0.0,
            tri,
            stage: EnvStage::Attack,
            env: 0.0,
            vel,
            tone,
            coefs: crate::tone::SvfCoefs::default(),
            ctrl: 0,
            ratios: [1.0; MAX_UNISON],
            ratio_key: (0, u32::MAX),
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

    /// 中央の成分だけ(モノで使う所。左右に広げていなければ全体)
    pub fn next(&mut self, p: &SubtractiveParams) -> f32 {
        self.next_stereo(p).0
    }

    /// (中央, 左右の差)。L = 中央 + 差、R = 中央 − 差
    pub fn next_stereo(&mut self, p: &SubtractiveParams) -> (f32, f32) {
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
                // release は fm / wavetable と同じ「その時間でほぼ消える(-60dB)」。
                // 以前は時定数として扱っていて、既定 0.2 秒でも消えるまで約 1.8 秒かかっていた
                let coef = (6.9 / ((p.release * self.art.release_mul).max(0.005) * sr)).min(1.0);
                self.env -= self.env * coef;
            }
        }

        // ---- 変調(制御レート): LFO・揺らぎ・フィルタのエンベロープ → フィルタの係数 ----
        if self.ctrl == 0 {
            let tp = &p.tone;
            self.tone
                .control(tp, CTRL_RATE, sr, self.stage == EnvStage::Release);
            if tp.spread > 0.0 {
                self.tone.layout((p.unison as usize).clamp(1, MAX_UNISON));
            }
            let fc = self
                .tone
                .cutoff(
                    tp,
                    p.cutoff * self.art.cutoff_mul,
                    p.filter_env,
                    self.env,
                    self.vel,
                    self.freq,
                )
                .clamp(40.0, sr * 0.45);
            self.coefs = crate::tone::SvfCoefs::new(fc, p.resonance, sr);
            self.ctrl = CTRL_RATE;
        }
        self.ctrl -= 1;

        // ---- ピッチ表現(ビブラート / チョーキング)と LFO・揺らぎの音程 ----
        let base_freq = if self.expr.is_active() {
            self.freq * self.expr.next_ratio(sr)
        } else {
            self.freq
        } * self.tone.pitch_ratio;

        // ---- オシレータ(ユニゾン対応) ----
        let n = (p.unison as usize).clamp(1, MAX_UNISON);
        // 声部ごとの倍率は、声部数かデチューンが変わったときだけ求め直す(毎サンプルの powf を避ける)
        let key = (n, p.detune_cents.to_bits());
        if self.ratio_key != key {
            self.ratio_key = key;
            for (i, r) in self.ratios.iter_mut().enumerate().take(n) {
                // 声部を中心対称にデチューン(-1..+1)
                let spread = if n == 1 {
                    0.0
                } else {
                    (i as f32 / (n - 1) as f32) * 2.0 - 1.0
                };
                *r = (2.0f32).powf(spread * p.detune_cents / 1200.0);
            }
        }
        let stereo = self.tone.stereo;
        let mut osc = 0.0f32;
        let mut side = 0.0f32;
        for i in 0..n {
            let dt = base_freq * self.ratios[i] * self.tone.unison_detune(&p.tone, i) / sr;
            let t = self.phases[i];
            let s = match p.waveform {
                Waveform::Saw => 2.0 * t - 1.0 - poly_blep(t, dt),
                Waveform::Square => {
                    let raw = if t < 0.5 { 1.0 } else { -1.0 };
                    let t2 = if t + 0.5 >= 1.0 { t - 0.5 } else { t + 0.5 };
                    raw + poly_blep(t, dt) - poly_blep(t2, dt)
                }
                Waveform::Triangle => {
                    // 帯域制限した矩形を積分する(角の折り返しを抑える)。わずかに漏らして直流のずれを戻す
                    let raw = if t < 0.5 { 1.0 } else { -1.0 };
                    let t2 = if t + 0.5 >= 1.0 { t - 0.5 } else { t + 0.5 };
                    let sq = raw + poly_blep(t, dt) - poly_blep(t2, dt);
                    let naive = 4.0 * (t - 0.5).abs() - 1.0;
                    let tri = self.tri[i] * 0.999 + naive * 0.001 - 4.0 * dt * sq;
                    self.tri[i] = tri;
                    tri
                }
                Waveform::Sine => (t * std::f32::consts::TAU).sin(),
            };
            osc += s;
            if stereo {
                side += s * self.tone.unison_pan(&p.tone, i);
            }
            self.phases[i] += dt;
            if self.phases[i] >= 1.0 {
                self.phases[i] -= 1.0;
            }
        }
        // 本数で音量が膨らみすぎないよう等パワー正規化
        let norm = (n as f32).sqrt();
        osc /= norm;
        side /= norm;
        if p.osc_level != 1.0 {
            osc *= p.osc_level;
            side *= p.osc_level;
        }

        // サブオシレータ(1 オクターブ下のサイン。ベースの土台)
        if p.sub > 0.0 {
            self.sub_phase += base_freq * 0.5 / sr;
            if self.sub_phase >= 1.0 {
                self.sub_phase -= 1.0;
            }
            osc += (self.sub_phase * std::f32::consts::TAU).sin() * p.sub;
        }

        // ノイズ(息・ざらつき)。ピンクは Paul Kellet の 3 段の近似、ブラウンは漏れのある積分
        if p.noise > 0.0 {
            let w = self.white();
            let v = match p.noise_color {
                NoiseColor::White => w,
                NoiseColor::Pink => {
                    let b = &mut self.pink;
                    b[0] = 0.99765 * b[0] + w * 0.099_046;
                    b[1] = 0.963 * b[1] + w * 0.296_516_4;
                    b[2] = 0.57 * b[2] + w * 1.052_691_3;
                    (b[0] + b[1] + b[2] + w * 0.1848) * 0.3
                }
                NoiseColor::Brown => {
                    self.brown = (self.brown + 0.02 * w) / 1.02;
                    self.brown * 3.5
                }
            };
            osc += v * p.noise;
        }
        // レコードのパチパチ: 1 秒に最大約 40 回のまれなクリック(強さはばらばら、数サンプルで消える)
        if p.crackle > 0.0 {
            let rate = p.crackle * p.crackle * 40.0 / sr;
            if (self.white() * 0.5 + 0.5) < rate {
                let a = self.white();
                self.click = a.signum() * (0.3 + 0.7 * a.abs()) * (0.5 + p.crackle * 0.5);
            }
            osc += self.click;
            self.click *= 0.55;
        }

        // ---- フィルタ(SVF。係数は上の制御レートで更新済み) ----
        let coefs = self.coefs;
        let (mid, side) = self.tone.filter(&p.tone, &coefs, osc, side);
        let (mid, side) = self.tone.apply_pan(mid, side);
        let g = self.env * self.amp * p.gain;
        let a = self.tone.amp_mul;
        (mid * g * a, side * g * a)
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
            noise_color: NoiseColor::White,
            osc_level: 1.0,
            crackle: 0.0,
            gain: 0.35,
            tone: Default::default(),
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

    #[test]
    fn noise_colors_and_crackle_without_the_oscillator() {
        let render = |color: NoiseColor, crackle: f32, noise: f32| {
            let mut p = default_params();
            p.osc_level = 0.0;
            p.noise = noise;
            p.noise_color = color;
            p.crackle = crackle;
            p.cutoff = 12000.0;
            p.filter_env = 0.0;
            p.sustain = 1.0;
            let mut v = SubtractiveVoice::start(&p, 440.0, 1.0, Default::default(), 48_000.0);
            (0..48_000).map(|_| v.next(&p)).collect::<Vec<f32>>()
        };
        // 高い成分の割合(隣どうしの差の大きさ ÷ 大きさ): ホワイト > ピンク > ブラウン
        let hf = |x: &[f32]| {
            let d: f32 = x.windows(2).map(|w| (w[1] - w[0]).abs()).sum();
            let a: f32 = x.iter().map(|v| v.abs()).sum();
            d / a.max(1e-9)
        };
        let w = render(NoiseColor::White, 0.0, 1.0);
        let pk = render(NoiseColor::Pink, 0.0, 1.0);
        let br = render(NoiseColor::Brown, 0.0, 1.0);
        assert!(
            hf(&w) > hf(&pk) && hf(&pk) > hf(&br),
            "{} {} {}",
            hf(&w),
            hf(&pk),
            hf(&br)
        );
        assert!(rms(&br) > 0.01 && rms(&pk) > 0.01);
        // パチパチだけ: まれなクリック(ほとんどの時間は無音に近い)
        let c = render(NoiseColor::White, 0.5, 0.0);
        let loud = c.iter().filter(|v| v.abs() > 0.05).count();
        assert!(loud > 5 && loud < 2000, "{loud}");
        // 波形の量 0 で雑音も無ければ無音
        assert!(rms(&render(NoiseColor::White, 0.0, 0.0)) < 1e-6);
    }

    fn render_st(p: &SubtractiveParams, seed: u32, vel: f32, n: usize) -> Vec<(f32, f32)> {
        let mut v = SubtractiveVoice::start_seeded(
            p,
            220.0,
            vel,
            glaux_core::Articulation::Normal,
            48_000.0,
            seed,
        );
        (0..n).map(|_| v.next_stereo(p)).collect()
    }

    #[test]
    fn spread_widens_without_changing_the_mono_sum() {
        let mut p = default_params();
        p.unison = 5;
        p.detune_cents = 20.0;
        let narrow = render_st(&p, 1, 1.0, 9600);
        p.tone.spread = 0.8;
        let wide = render_st(&p, 1, 1.0, 9600);
        // 中央の成分(モノに畳んだ音)は広げる前と同じ
        let d: f32 = narrow
            .iter()
            .zip(&wide)
            .map(|(a, b)| (a.0 - b.0).abs())
            .sum();
        assert!(d < 1e-3, "{d}");
        // 左右の差は 0 から増える
        assert!(narrow.iter().all(|x| x.1 == 0.0));
        let side: Vec<f32> = wide.iter().map(|x| x.1).collect();
        let mid: Vec<f32> = wide.iter().map(|x| x.0).collect();
        assert!(rms(&side) > rms(&mid) * 0.2, "{} {}", rms(&side), rms(&mid));
    }

    #[test]
    fn analog_varies_by_seed_but_is_repeatable() {
        let mut p = default_params();
        p.unison = 3;
        p.tone.analog = 0.5;
        let a = render_st(&p, 11, 1.0, 9600);
        let b = render_st(&p, 11, 1.0, 9600);
        let c = render_st(&p, 12, 1.0, 9600);
        assert_eq!(a, b, "同じ種なら同じ音");
        let d: f32 = a.iter().zip(&c).map(|(x, y)| (x.0 - y.0).abs()).sum();
        assert!(d > 1.0, "種が違えば少し違う音: {d}");
        // 揺らぎ 0 なら種で変わらない
        p.tone.analog = 0.0;
        assert_eq!(render_st(&p, 11, 1.0, 4800), render_st(&p, 12, 1.0, 4800));
    }

    #[test]
    fn separate_filter_envelope_brightens_only_the_head() {
        let hf = |x: &[(f32, f32)]| x.windows(2).map(|w| (w[1].0 - w[0].0).powi(2)).sum::<f32>();
        let mut p = default_params();
        p.cutoff = 300.0;
        p.sustain = 1.0;
        p.filter_env = 1.0;
        p.tone.filter_decay = 0.08;
        let sep = render_st(&p, 1, 1.0, 48_000);
        // 従来(音量の ADSR に従う。サスティン 1 なので開いたまま)と比べ、後半(0.5 秒〜)は閉じて暗い
        p.tone.filter_decay = 0.0;
        let legacy = render_st(&p, 1, 1.0, 48_000);
        let tail_sep = hf(&sep[24_000..26_400]);
        let tail_legacy = hf(&legacy[24_000..26_400]);
        assert!(tail_sep < tail_legacy * 0.3, "{tail_sep} {tail_legacy}");
        // 頭は同じくらい開いている
        let head_sep = hf(&sep[200..1200]);
        let head_legacy = hf(&legacy[200..1200]);
        assert!(head_sep > head_legacy * 0.3, "{head_sep} {head_legacy}");
    }

    #[test]
    fn velocity_darkens_soft_notes_when_asked() {
        let hf = |x: &[(f32, f32)]| {
            let d: f32 = x.windows(2).map(|w| (w[1].0 - w[0].0).powi(2)).sum();
            let a: f32 = x.iter().map(|v| v.0 * v.0).sum();
            d / a.max(1e-12)
        };
        let mut p = default_params();
        p.filter_env = 0.0;
        p.resonance = 0.0;
        p.cutoff = 4000.0;
        p.tone.vel_cutoff = 0.8;
        let soft = render_st(&p, 1, 0.3, 9600);
        let hard = render_st(&p, 1, 1.0, 9600);
        assert!(hf(&soft) < hf(&hard) * 0.8, "{} {}", hf(&soft), hf(&hard));
    }

    #[test]
    fn lfo_to_pan_and_amp_moves_the_sound() {
        let mut p = default_params();
        p.sustain = 1.0;
        p.tone.lfo[0] = crate::tone::LfoParams {
            rate: 4.0,
            depth: 0.8,
            shape: crate::tone::LfoShape::Sine,
            target: crate::tone::LfoTarget::Pan,
        };
        let out = render_st(&p, 1, 1.0, 24_000);
        // パン: 左右の差が出て、符号が入れ替わる(左右へ揺れる)
        let pos = out.iter().filter(|x| x.1 > 0.01).count();
        let neg = out.iter().filter(|x| x.1 < -0.01).count();
        assert!(pos > 1000 && neg > 1000, "{pos} {neg}");
        // 音量: 包絡が 4Hz で上下する
        p.tone.lfo[0].target = crate::tone::LfoTarget::Amp;
        let out = render_st(&p, 1, 1.0, 24_000);
        let win: Vec<f32> = out
            .chunks(1200)
            .map(|c| rms(&c.iter().map(|x| x.0).collect::<Vec<_>>()))
            .collect();
        let (mn, mx) = win[2..]
            .iter()
            .fold((f32::MAX, 0.0f32), |(a, b), v| (a.min(*v), b.max(*v)));
        assert!(mx > mn * 2.0, "{mn} {mx}");
    }

    #[test]
    fn triangle_aliases_less_than_the_naive_one() {
        // 高い音(3.5kHz)の三角波: 基本波の 11 倍以上(ナイキストの向こう)の折り返しが、素朴な計算より小さい
        let mut p = default_params();
        p.waveform = Waveform::Triangle;
        p.cutoff = 12000.0;
        p.filter_env = 0.0;
        p.sustain = 1.0;
        let sr = 48_000.0;
        // DFT の区切りにちょうど乗る周波数(漏れで比べにくくならないように)
        let f0 = 300.0 * sr / 4096.0;
        let mut v = SubtractiveVoice::start(&p, f0, 1.0, Default::default(), sr);
        let ours: Vec<f32> = (0..8192).map(|_| v.next(&p)).collect();
        let naive: Vec<f32> = (0..8192)
            .map(|i| {
                let t = (i as f32 * f0 / sr).fract();
                4.0 * (t - 0.5).abs() - 1.0
            })
            .collect();
        // 折り返しは基本波の倍音でない周波数に出る: 4.2kHz 付近(13 倍音 45.5kHz の折り返し = 2.5kHz…)を避けて、
        // 倍音以外の成分の割合を DFT で比べる
        let inharmonic = |x: &[f32]| {
            let n = x.len();
            let mut harm = 0.0f64;
            let mut other = 0.0f64;
            for k in 1..n / 2 {
                let f = k as f64 * sr as f64 / n as f64;
                let (mut re, mut im) = (0.0f64, 0.0f64);
                for (i, s) in x.iter().enumerate() {
                    let a = std::f64::consts::TAU * (k * i) as f64 / n as f64;
                    re += *s as f64 * a.cos();
                    im -= *s as f64 * a.sin();
                }
                let pw = re * re + im * im;
                let h = (f / f0 as f64).round();
                if (f - h * f0 as f64).abs() < 30.0 {
                    harm += pw;
                } else {
                    other += pw;
                }
            }
            other / harm
        };
        let a = inharmonic(&ours[4096..]);
        let b = inharmonic(&naive[4096..]);
        assert!(a < b * 0.5, "ours {a} naive {b}");
    }
}
