//! ドラムシンセ `drum`。
//!
//! MIDI ノート番号(General MIDI 準拠)で音色を弾き分ける:
//! 35/36=キック, 37=サイドスティック, 38/40=スネア, 39=クラップ, 42/44=クローズド・ペダルハット,
//! 46=オープンハット, 41..50 の奇数側と 48・50=タム, 49/57=クラッシュ, 51/59=ライド, 53=ライドのベル,
//! 52=チャイナ, 55=リバースクラッシュ(ビルドアップ用), 54=タンバリン, 56=カウベル, 60〜64=ボンゴ・コンガ,
//! 65/66=ティンバレス, 67/68=アゴゴ, 69=カバサ, 70/82=マラカス・シェイカー, 75=クラベス, 76/77=ウッドブロック,
//! 80/81=トライアングル。その他は短いパーカッション。
//!
//! 作り方はアナログのドラムマシン(TR-808・909)にならう。どのパーツも次の部品を組み合わせる:
//! - 音程のある成分(サイン波 3 つまで。鳴り始めに音程が上から落ちる)
//! - 金属音(矩形波 6 つの和 = 808 のハイハット・シンバル・カウベルの作り方)をバンドパスとハイパスに通したもの
//! - ノイズ(スネアの響き線・クラップ・シェイカー)をフィルタに通したもの
//! - アタックのクリック
//!
//! 音量の減りは「速い減衰 + 遅い減衰」の 2 つの指数の和で、パーツの音らしさ(スネアの初めの破裂と響き線の尾、
//! シンバルの初めの明るさと長い余韻)を作る。キットは modern / 808 / 909 の 3 つで、同じパーツの部品の値を変える。
//! 強く叩くほど明るく(フィルタが開き、クリックが増える)。クローズド・ペダルハットはオープンハットを止める。
//! すべて合成(サンプル不使用)。発音時に値を決め、処理中はアロケーション・ロックなし。

use std::f32::consts::TAU;

/// キットの種類
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DrumKit {
    /// 今どきの打ち込み(太いキック・張ったスネア・明るいハット)
    #[default]
    Modern,
    /// TR-808 風(長く伸びるキック、細いスネア、金属的なハット・カウベル)
    Tr808,
    /// TR-909 風(アタックの強いキック、ノイズの多いスネア、明るいハット)
    Tr909,
}

impl DrumKit {
    pub fn parse(s: &str) -> Self {
        match s {
            "808" => DrumKit::Tr808,
            "909" => DrumKit::Tr909,
            _ => DrumKit::Modern,
        }
    }
}

/// 焼き込み済みパラメータ(1 トラック分)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrumParams {
    /// リニアゲイン(dB から変換済み)
    pub gain: f32,
    /// 減衰時間の倍率 0.25..=4
    pub decay: f32,
    /// 明るさ 0..=1(ノイズ成分・クリック量)
    pub tone: f32,
    /// 半音単位のチューニング(キック・スネア・タム・パーカッションの音程)
    pub tune: f32,
    pub kit: DrumKit,
    /// キックの音程(半音)
    pub kick_tune: f32,
    /// キックの長さの倍率
    pub kick_decay: f32,
    /// キックのアタック(クリックと歪み)0..=1
    pub kick_punch: f32,
    /// スネアの音程(半音)
    pub snare_tune: f32,
    /// スネアの響き線(ノイズ)の量 0..=1
    pub snare_snappy: f32,
    /// ハイハットの長さの倍率
    pub hat_decay: f32,
}

impl Default for DrumParams {
    fn default() -> Self {
        DrumParams {
            gain: 0.5,
            decay: 1.0,
            tone: 0.5,
            tune: 0.0,
            kit: DrumKit::Modern,
            kick_tune: 0.0,
            kick_decay: 1.0,
            kick_punch: 0.5,
            snare_tune: 0.0,
            snare_snappy: 0.6,
            hat_decay: 1.0,
        }
    }
}

/// 指数の減衰(ホールドの後に `k` を掛け続ける)
#[derive(Clone, Copy, Debug, Default)]
struct Env {
    v: f32,
    k: f32,
    hold: u32,
}

impl Env {
    fn new(level: f32, tau: f32, hold: f32, sr: f32) -> Self {
        Env {
            v: level,
            k: (-1.0 / (tau.max(0.0005) * sr)).exp(),
            hold: (hold * sr) as u32,
        }
    }

    /// 鳴り終わるまでのサンプル数(-66dB まで)
    fn len(&self) -> u32 {
        if self.v <= 0.0 {
            return 0;
        }
        let per = -self.k.ln();
        self.hold + ((self.v / 0.0005).max(1.0).ln() / per.max(1e-9)) as u32
    }

    #[inline]
    fn next(&mut self) -> f32 {
        let out = self.v;
        if self.hold > 0 {
            self.hold -= 1;
        } else {
            self.v *= self.k;
        }
        out
    }
}

/// 固定係数の状態変数フィルタ(TPT)
#[derive(Clone, Copy, Debug, Default)]
struct Svf {
    a1: f32,
    a2: f32,
    a3: f32,
    m0: f32,
    m1: f32,
    m2: f32,
    ic1: f32,
    ic2: f32,
    on: bool,
}

impl Svf {
    fn make(sr: f32, freq: f32, q: f32, m: (f32, f32, f32)) -> Self {
        let g = (std::f32::consts::PI * freq.clamp(20.0, sr * 0.45) / sr).tan();
        let k = 1.0 / q.max(0.1);
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        Svf {
            a1,
            a2,
            a3: g * a2,
            m0: m.0,
            m1: m.1 * k,
            m2: m.2,
            on: true,
            ..Default::default()
        }
    }
    fn lp(sr: f32, f: f32, q: f32) -> Self {
        Self::make(sr, f, q, (0.0, 0.0, 1.0))
    }
    fn hp(sr: f32, f: f32, q: f32) -> Self {
        Self::make(sr, f, q, (1.0, -1.0, -1.0))
    }
    fn bp(sr: f32, f: f32, q: f32) -> Self {
        Self::make(sr, f, q, (0.0, 1.0, 0.0))
    }

    #[inline]
    fn run(&mut self, v0: f32) -> f32 {
        if !self.on {
            return v0;
        }
        let v3 = v0 - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        self.m0 * v0 + self.m1 * v1 + self.m2 * v2
    }
}

/// 音程のある成分(サイン波)。鳴り始めに `1 + sweep` 倍から落ちる
#[derive(Clone, Copy, Debug, Default)]
struct Tone {
    freq: f32,
    sweep: f32,
    /// 音程の落ち方(1 サンプルあたりの倍率)
    pk: f32,
    pe: f32,
    phase: f32,
    env: Env,
    env2: Env,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Shape {
    #[default]
    Plain,
    /// クラップ: 間隔 `burst_gap` の連打のあとに尾
    Clap,
    /// リバースクラッシュ: 全長まで 3 乗で迫り上がり、終端で切れる
    Swell,
}

const MAX_TONES: usize = 3;
const METALS: usize = 6;
/// 808 のハイハット・シンバルの 6 つの矩形波(Hz)
const METAL_808: [f32; METALS] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];

/// スネアの部品の値(キットごと)
struct SnareRecipe {
    f0: f32,
    sweep: f32,
    /// 胴の成分(基音に対する倍率, 大きさ, 減衰の秒)
    modes: &'static [(f32, f32, f32)],
    hp: f32,
    lp: f32,
    crack: f32,
    tail: f32,
    tail_tau: f32,
    click: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct DrumVoice {
    tones: [Tone; MAX_TONES],
    n_tones: usize,
    metal_freq: [f32; METALS],
    metal_phase: [f32; METALS],
    n_metal: usize,
    metal_env: Env,
    metal_env2: Env,
    metal_f1: Svf,
    metal_f2: Svf,
    noise_env: Env,
    noise_env2: Env,
    noise_f1: Svf,
    noise_f2: Svf,
    click_env: Env,
    click_f: Svf,
    /// xorshift32 の状態(ノイズ用)
    rng: u32,
    shape: Shape,
    burst_gap: u32,
    /// 歪みの混ぜ具合(0 = なし)。鳴り始めだけに掛け、`drive_env` で消えていく(余韻を持ち上げない)
    drive: f32,
    drive_env: Env,
    /// 音量(ベロシティ × パーツの大きさ)
    amp: f32,
    t: u32,
    /// 全長(サンプル)
    len: u32,
    /// チョークの減衰(1 = 通常)
    choke_gain: f32,
    choking: bool,
    /// チョーク: このグループに属する(0 = なし)/ このグループが鳴ったら止まる
    group: u8,
    off_by: u8,
    sample_rate: f32,
}

fn st(semis: f32) -> f32 {
    2.0_f32.powf(semis / 12.0)
}

impl DrumVoice {
    pub fn start(p: &DrumParams, pitch: u8, vel: f32, sample_rate: f32) -> Self {
        let sr = sample_rate;
        let mut v = DrumVoice {
            tones: [Tone::default(); MAX_TONES],
            n_tones: 0,
            metal_freq: [0.0; METALS],
            metal_phase: [0.0; METALS],
            n_metal: 0,
            metal_env: Env::default(),
            metal_env2: Env::default(),
            metal_f1: Svf::default(),
            metal_f2: Svf::default(),
            noise_env: Env::default(),
            noise_env2: Env::default(),
            noise_f1: Svf::default(),
            noise_f2: Svf::default(),
            click_env: Env::default(),
            click_f: Svf::default(),
            rng: 0x9e37_79b9 ^ ((pitch as u32) << 8) | 1,
            shape: Shape::Plain,
            burst_gap: 0,
            drive: 0.0,
            drive_env: Env::new(1.0, 0.08, 0.0, sr),
            amp: vel.max(0.0),
            t: 0,
            len: 0,
            choke_gain: 1.0,
            choking: false,
            group: 0,
            off_by: 0,
            sample_rate: sr,
        };
        v.build(p, pitch, vel.clamp(0.0, 1.5));
        // 全長: どの成分も十分小さくなるまで
        let mut len = v.metal_env.len().max(v.metal_env2.len());
        len = len.max(v.noise_env.len()).max(v.noise_env2.len());
        len = len.max(v.click_env.len());
        for t in &v.tones[..v.n_tones] {
            len = len.max(t.env.len()).max(t.env2.len());
        }
        if v.shape == Shape::Clap {
            len += v.burst_gap * 3;
        }
        if v.shape != Shape::Swell {
            v.len = len.min((sr * 8.0) as u32);
        }
        v
    }

    fn tone(&mut self, freq: f32, level: f32, tau: f32, sweep: f32, sweep_tau: f32, hold: f32) {
        if self.n_tones >= MAX_TONES {
            return;
        }
        let sr = self.sample_rate;
        self.tones[self.n_tones] = Tone {
            freq,
            sweep,
            pk: (-1.0 / (sweep_tau.max(0.0005) * sr)).exp(),
            pe: 1.0,
            phase: 0.0,
            env: Env::new(level, tau, hold, sr),
            env2: Env::default(),
        };
        self.n_tones += 1;
    }

    fn metal(&mut self, freqs: &[f32], scale: f32) {
        self.n_metal = freqs.len().min(METALS);
        for (i, f) in freqs.iter().take(METALS).enumerate() {
            self.metal_freq[i] = f * scale;
            // 位相をずらして鳴り始めの山を丸める
            self.metal_phase[i] = i as f32 * 0.37 % 1.0;
        }
    }

    /// パーツとキットから部品の値を決める
    fn build(&mut self, p: &DrumParams, pitch: u8, vel: f32) {
        let sr = self.sample_rate;
        let d = p.decay.clamp(0.25, 4.0);
        let tone = p.tone.clamp(0.0, 1.0);
        // 強く叩くほど明るく(フィルタが開く)
        let bright = (0.75 + 0.5 * tone) * (0.8 + 0.25 * vel.min(1.2));
        let kit = p.kit;
        let tune = p.tune;
        match pitch {
            // ---- キック ----
            35 | 36 => {
                let low = if pitch == 35 { -2.0 } else { 0.0 };
                let f = st(tune + p.kick_tune + low);
                let kd = d * p.kick_decay.clamp(0.25, 4.0);
                let punch = p.kick_punch.clamp(0.0, 1.0);
                let (f0, sweep, sweep_tau, hold, tau, drive, click) = match kit {
                    DrumKit::Modern => (52.0, 3.5, 0.030, 0.012, 0.30, 0.3 + 0.9 * punch, 0.5),
                    DrumKit::Tr808 => (48.0, 0.7, 0.020, 0.004, 0.55, 0.1 * punch, 0.15),
                    DrumKit::Tr909 => (55.0, 3.0, 0.022, 0.008, 0.24, 0.3 + 0.6 * punch, 0.7),
                };
                self.tone(f0 * f, 1.0, tau * kd, sweep, sweep_tau, hold);
                self.drive = drive;
                // クリック: 短いノイズをバンドパスで
                self.click_env =
                    Env::new(click * (0.3 + tone) * (0.4 + punch) * vel, 0.003, 0.0, sr);
                self.click_f = Svf::bp(sr, 3500.0 * bright, 0.8);
                self.amp *= 0.9;
            }
            // ---- スネア ----
            38 | 40 => {
                let f = st(tune + p.snare_tune + if pitch == 40 { 1.5 } else { 0.0 });
                let snappy = p.snare_snappy.clamp(0.0, 1.0) / 0.6;
                // 胴: 皮の振動の成分(基音の 1・1.59・2.14 倍。倍音関係にないので音程感が薄い)を短く。
                // 音程を上から落とすとタムの「ドゥン」になるので、落とすのは 909 のごく短い分だけ
                // (808 は実機どおり 2 音)
                let r = match kit {
                    DrumKit::Modern => SnareRecipe {
                        f0: 200.0,
                        sweep: 0.04,
                        modes: &[(1.0, 0.45, 0.045), (1.59, 0.3, 0.035), (2.14, 0.2, 0.025)],
                        hp: 1200.0,
                        lp: 12_000.0,
                        crack: 0.6,
                        tail: 0.85,
                        tail_tau: 0.17,
                        click: 0.7,
                    },
                    DrumKit::Tr808 => SnareRecipe {
                        f0: 238.0,
                        sweep: 0.0,
                        modes: &[(1.0, 0.5, 0.05), (2.0, 0.25, 0.035)],
                        hp: 1800.0,
                        lp: 9_000.0,
                        crack: 0.4,
                        tail: 0.75,
                        tail_tau: 0.11,
                        click: 0.5,
                    },
                    DrumKit::Tr909 => SnareRecipe {
                        f0: 190.0,
                        sweep: 0.12,
                        modes: &[(1.0, 0.45, 0.05), (1.59, 0.3, 0.035), (2.14, 0.2, 0.025)],
                        hp: 700.0,
                        lp: 12_000.0,
                        crack: 0.7,
                        tail: 1.0,
                        tail_tau: 0.19,
                        click: 0.8,
                    },
                };
                let SnareRecipe {
                    f0,
                    sweep,
                    modes,
                    hp,
                    lp,
                    crack,
                    tail,
                    tail_tau,
                    click,
                } = r;
                for &(ratio, level, tau) in modes {
                    self.tone(f0 * ratio * f, level, tau * d, sweep, 0.006, 0.0);
                }
                // 響き線: 初めの破裂と尾
                self.noise_env = Env::new(tail * snappy, tail_tau * d, 0.0, sr);
                self.noise_env2 = Env::new(crack * snappy, 0.010, 0.0, sr);
                self.noise_f1 = Svf::hp(sr, hp * bright, 0.7);
                self.noise_f2 = Svf::lp(sr, lp * bright, 0.7);
                // スティックが当たる「パン」: 帯域の広いクリック
                self.click_env = Env::new(click * vel, 0.005, 0.0, sr);
                self.click_f = Svf::bp(sr, 1500.0 * bright, 0.5);
                self.drive = 0.25;
                self.amp *= match kit {
                    DrumKit::Tr909 => 0.5,
                    DrumKit::Modern => 0.6,
                    DrumKit::Tr808 => 0.7,
                };
            }
            // ---- サイドスティック ----
            37 => {
                let f = st(tune);
                self.tone(520.0 * f, 0.5, 0.012, 0.1, 0.004, 0.0);
                self.tone(1650.0 * f, 0.35, 0.007, 0.0, 0.004, 0.0);
                self.click_env = Env::new(0.6 * vel, 0.002, 0.0, sr);
                self.click_f = Svf::bp(sr, 3000.0 * bright, 1.0);
                self.amp *= 0.9;
            }
            // ---- クラップ ----
            39 => {
                let (bp, q, tail_tau) = match kit {
                    DrumKit::Tr808 => (1100.0, 1.2, 0.12),
                    _ => (1300.0, 0.9, 0.09),
                };
                self.shape = Shape::Clap;
                self.burst_gap = (0.009 * sr) as u32;
                self.noise_env = Env::new(0.7, tail_tau * d, 0.0, sr);
                self.noise_f1 = Svf::bp(sr, bp * bright, q);
                self.noise_f2 = Svf::hp(sr, 600.0, 0.7);
                self.amp *= 2.3;
            }
            // ---- ハイハット ----
            42 | 44 | 46 => {
                let hd = d * p.hat_decay.clamp(0.25, 4.0);
                let (scale, center, noise, tau_c, tau_o) = match kit {
                    DrumKit::Modern => (1.0, 10_000.0, 0.35, 0.035, 0.30),
                    DrumKit::Tr808 => (1.0, 9_000.0, 0.1, 0.030, 0.35),
                    DrumKit::Tr909 => (1.2, 11_000.0, 0.55, 0.040, 0.40),
                };
                let (tau, level) = match pitch {
                    42 => (tau_c, 1.0),
                    44 => (tau_c * 0.7, 0.7),
                    _ => (tau_o, 0.9),
                };
                self.metal(&METAL_808, scale);
                self.metal_env = Env::new(level * (1.0 - noise), tau * hd, 0.0, sr);
                self.metal_env2 = Env::new(level * 0.3, 0.006, 0.0, sr);
                self.metal_f1 = Svf::bp(sr, center * bright, 0.9);
                self.metal_f2 = Svf::hp(sr, 7000.0 * bright.min(1.2), 0.7);
                self.noise_env = Env::new(level * noise, tau * hd, 0.0, sr);
                self.noise_f1 = Svf::hp(sr, 8000.0 * bright.min(1.2), 0.7);
                if pitch == 46 {
                    self.off_by = 1;
                } else {
                    self.group = 1;
                }
                self.amp *= 1.2;
            }
            // ---- シンバル ----
            49 | 57 | 52 => {
                let scale = match pitch {
                    49 => 1.9,
                    57 => 2.15,
                    _ => 2.4,
                };
                let long = if pitch == 52 { 0.9 } else { 1.3 };
                self.metal(&METAL_808, scale);
                self.metal_env = Env::new(0.45, long * d, 0.0, sr);
                self.metal_env2 = Env::new(0.55, 0.08, 0.0, sr);
                self.metal_f1 = Svf::bp(sr, 7000.0 * bright, 0.5);
                self.metal_f2 = Svf::hp(sr, 3500.0, 0.7);
                self.noise_env = Env::new(0.4, long * 0.8 * d, 0.0, sr);
                self.noise_env2 = Env::new(0.5, 0.06, 0.0, sr);
                self.noise_f1 = Svf::hp(sr, 5000.0 * bright.min(1.2), 0.7);
                if pitch == 52 {
                    self.drive = 0.8;
                }
                self.amp *= if pitch == 52 { 0.45 } else { 0.4 };
            }
            // ---- ライド・ライドのベル ----
            51 | 59 | 53 => {
                let scale = if pitch == 59 { 1.7 } else { 1.55 };
                self.metal(&METAL_808, scale);
                let bell = if pitch == 53 { 0.5 } else { 0.2 };
                self.metal_env = Env::new(0.3, 0.9 * d, 0.0, sr);
                self.metal_env2 = Env::new(0.4, 0.03, 0.0, sr);
                self.metal_f1 = Svf::bp(sr, 6000.0 * bright, 0.8);
                self.metal_f2 = Svf::hp(sr, 3000.0, 0.7);
                let f = st(tune);
                self.tone(760.0 * f, bell, 0.7 * d, 0.0, 0.01, 0.0);
                self.tone(1190.0 * f, bell * 0.6, 0.5 * d, 0.0, 0.01, 0.0);
                self.tone(2050.0 * f, bell * 0.35, 0.35 * d, 0.0, 0.01, 0.0);
                self.click_env = Env::new(0.3 * vel, 0.002, 0.0, sr);
                self.click_f = Svf::hp(sr, 4000.0, 0.7);
                self.amp *= 0.55;
            }
            // ---- リバースクラッシュ ----
            55 => {
                self.shape = Shape::Swell;
                self.len = (1.6 * d * sr) as u32;
                self.metal(&METAL_808, 1.9);
                self.metal_env = Env::new(0.5, 1e6, 0.0, sr);
                self.metal_f1 = Svf::bp(sr, 7000.0 * bright, 0.5);
                self.metal_f2 = Svf::hp(sr, 3500.0, 0.7);
                self.noise_env = Env::new(0.5, 1e6, 0.0, sr);
                self.noise_f1 = Svf::hp(sr, 4000.0 * bright.min(1.2), 0.7);
                self.amp *= 0.8;
            }
            // ---- タム ----
            41 | 43 | 45 | 47 | 48 | 50 => {
                let f = 110.0 * st(tune + (pitch as f32 - 45.0));
                let (sweep, tau, noise) = match kit {
                    DrumKit::Modern => (0.6, 0.30, 0.15),
                    DrumKit::Tr808 => (0.15, 0.45, 0.0),
                    DrumKit::Tr909 => (0.9, 0.25, 0.25),
                };
                self.tone(f, 0.85, tau * d, sweep, 0.05, 0.004);
                self.tone(f * 1.5, 0.2, tau * 0.4 * d, sweep, 0.05, 0.0);
                self.noise_env = Env::new(noise, 0.03, 0.0, sr);
                self.noise_f1 = Svf::bp(sr, 2000.0 * bright, 0.8);
                self.click_env = Env::new(0.2 * vel, 0.002, 0.0, sr);
                self.click_f = Svf::bp(sr, 3000.0, 0.8);
                self.drive = 0.2;
                self.amp *= 0.75;
            }
            // ---- タンバリン ----
            54 => {
                self.metal(&METAL_808, 4.0);
                self.metal_env = Env::new(0.4, 0.16 * d, 0.0, sr);
                self.metal_env2 = Env::new(0.5, 0.03, 0.0, sr);
                self.metal_f1 = Svf::bp(sr, 9000.0 * bright, 1.0);
                self.metal_f2 = Svf::hp(sr, 5000.0, 0.7);
                self.noise_env = Env::new(0.3, 0.08 * d, 0.0, sr);
                self.noise_f1 = Svf::hp(sr, 7000.0, 0.7);
                self.amp *= 0.6;
            }
            // ---- カウベル ----
            56 => {
                self.metal(&[540.0 * st(tune), 800.0 * st(tune)], 1.0);
                self.metal_env = Env::new(0.45, 0.13 * d, 0.0, sr);
                self.metal_env2 = Env::new(0.55, 0.012, 0.0, sr);
                self.metal_f1 = Svf::bp(sr, 1800.0, 1.5);
                self.amp *= 0.4;
            }
            // ---- ボンゴ・コンガ・ティンバレス ----
            60..=66 => {
                let (f0, tau, sweep) = match pitch {
                    60 => (410.0, 0.09, 0.3),
                    61 => (300.0, 0.11, 0.3),
                    62 => (330.0, 0.045, 0.2),
                    63 => (330.0, 0.18, 0.15),
                    64 => (220.0, 0.2, 0.15),
                    65 => (380.0, 0.16, 0.1),
                    _ => (280.0, 0.18, 0.1),
                };
                self.tone(f0 * st(tune), 0.8, tau * d, sweep, 0.012, 0.0);
                self.tone(f0 * 1.6 * st(tune), 0.15, tau * 0.5 * d, sweep, 0.012, 0.0);
                self.click_env = Env::new(0.35 * vel, 0.002, 0.0, sr);
                self.click_f = Svf::bp(sr, 2500.0 * bright, 0.8);
                if pitch >= 65 {
                    // ティンバレスは金属の胴の響きを少し
                    self.metal(&METAL_808, 2.5);
                    self.metal_env = Env::new(0.15, 0.1 * d, 0.0, sr);
                    self.metal_f1 = Svf::bp(sr, 5000.0, 0.8);
                }
                self.amp *= 0.8;
            }
            // ---- アゴゴ ----
            67 | 68 => {
                let f0 = if pitch == 67 { 900.0 } else { 700.0 } * st(tune);
                self.tone(f0, 0.6, 0.2 * d, 0.0, 0.01, 0.0);
                self.tone(f0 * 1.52, 0.3, 0.12 * d, 0.0, 0.01, 0.0);
                self.click_env = Env::new(0.2 * vel, 0.0015, 0.0, sr);
                self.click_f = Svf::hp(sr, 3000.0, 0.7);
                self.amp *= 0.6;
            }
            // ---- シェイカー類 ----
            69 | 70 | 82 => {
                let (f, tau) = match pitch {
                    69 => (7000.0, 0.05),
                    70 => (5000.0, 0.03),
                    _ => (6000.0, 0.06),
                };
                self.noise_env = Env::new(0.6, tau * d, 0.004, sr);
                self.noise_f1 = Svf::bp(sr, f * bright, 0.9);
                self.noise_f2 = Svf::hp(sr, 3000.0, 0.7);
                self.amp *= 1.2;
            }
            // ---- クラベス・ウッドブロック ----
            75..=77 => {
                let f0 = match pitch {
                    75 => 2500.0,
                    76 => 1100.0,
                    _ => 830.0,
                } * st(tune);
                self.tone(f0, 0.7, 0.03 * d, 0.02, 0.003, 0.0);
                self.tone(f0 * 2.03, 0.2, 0.015 * d, 0.0, 0.003, 0.0);
                self.amp *= 0.6;
            }
            // ---- トライアングル ----
            80 | 81 => {
                let tau = if pitch == 80 { 0.06 } else { 1.2 };
                self.tone(4200.0, 0.5, tau * d, 0.0, 0.01, 0.0);
                self.tone(6700.0, 0.3, tau * 0.8 * d, 0.0, 0.01, 0.0);
                self.tone(9900.0, 0.2, tau * 0.6 * d, 0.0, 0.01, 0.0);
                self.click_env = Env::new(0.2 * vel, 0.001, 0.0, sr);
                self.click_f = Svf::hp(sr, 5000.0, 0.7);
                self.amp *= 0.4;
            }
            // ---- その他: 短いパーカッション ----
            _ => {
                let f0 = 400.0 * st(tune + (pitch as f32 - 60.0) * 0.5);
                self.tone(f0, 0.6, 0.1 * d, 0.2, 0.01, 0.0);
                self.noise_env = Env::new(0.3 * tone, 0.03 * d, 0.0, sr);
                self.noise_f1 = Svf::bp(sr, 3000.0 * bright, 0.8);
                self.amp *= 0.6;
            }
        }
    }

    /// ドラムはワンショットなのでノートオフは無視する。
    pub fn note_off(&mut self) {}

    /// チョークのグループ(クローズド・ペダルハット = 1。0 = 無し)
    pub fn group(&self) -> u32 {
        self.group as u32
    }

    /// グループ `g` が鳴ったら止まるか(オープンハット)
    pub fn stopped_by(&self, g: u32) -> bool {
        g != 0 && self.off_by as u32 == g
    }

    /// すばやく(約 10ms で)止める
    pub fn choke(&mut self) {
        self.choking = true;
    }

    fn noise(&mut self) -> f32 {
        // xorshift32: RT セーフな擬似乱数
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    pub fn finished(&self, _p: &DrumParams) -> bool {
        self.t >= self.len || (self.choking && self.choke_gain < 1e-4)
    }

    pub fn next(&mut self, p: &DrumParams) -> f32 {
        if self.t >= self.len {
            return 0.0;
        }
        let sr = self.sample_rate;
        let t = self.t;
        self.t += 1;
        let mut out = 0.0f32;

        // 音程のある成分
        for tn in &mut self.tones[..self.n_tones] {
            let f = tn.freq * (1.0 + tn.sweep * tn.pe);
            tn.pe *= tn.pk;
            tn.phase += f / sr;
            if tn.phase >= 1.0 {
                tn.phase -= 1.0;
            }
            out += (tn.phase * TAU).sin() * (tn.env.next() + tn.env2.next());
        }
        // 金属音(矩形波の和)
        if self.n_metal > 0 {
            let mut m = 0.0;
            for i in 0..self.n_metal {
                self.metal_phase[i] += self.metal_freq[i] / sr;
                if self.metal_phase[i] >= 1.0 {
                    self.metal_phase[i] -= 1.0;
                }
                m += if self.metal_phase[i] < 0.5 { 1.0 } else { -1.0 };
            }
            m /= self.n_metal as f32;
            let m = self.metal_f2.run(self.metal_f1.run(m));
            out += m * (self.metal_env.next() + self.metal_env2.next()) * 2.5;
        }
        // ノイズ
        if self.noise_env.v > 0.0 || self.noise_env2.v > 0.0 {
            let n = self.noise();
            let n = self.noise_f2.run(self.noise_f1.run(n));
            let level = match self.shape {
                Shape::Clap => {
                    // 3 連打のあとに尾
                    let gap = self.burst_gap.max(1);
                    if t < gap * 3 {
                        let within = (t % gap) as f32 / sr;
                        (-within / 0.0025).exp()
                    } else {
                        self.noise_env.next()
                    }
                }
                _ => self.noise_env.next() + self.noise_env2.next(),
            };
            out += n * level;
        }
        // クリック
        if self.click_env.v > 1e-5 {
            let n = self.noise();
            out += self.click_f.run(n) * self.click_env.next();
        }
        // 歪み: 鳴り始めだけ飽和させて太く(山の高さはそのまま、余韻は元の音)
        if self.drive > 0.0 {
            let mix = (self.drive * self.drive_env.next()).min(1.0);
            let sat = (out * 3.0).tanh() / 3.0_f32.tanh();
            out += (sat - out) * mix;
        }
        if self.shape == Shape::Swell {
            let progress = (t as f32 / self.len.max(1) as f32).clamp(0.0, 1.0);
            out *= progress * progress * progress;
        }
        if self.choking {
            self.choke_gain *= (-1.0 / (0.004 * sr)).exp();
            out *= self.choke_gain;
        }
        out * self.amp * p.gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> DrumParams {
        DrumParams::default()
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32).sqrt()
    }

    fn render_with(p: &DrumParams, pitch: u8, vel: f32, samples: usize) -> Vec<f32> {
        let mut v = DrumVoice::start(p, pitch, vel, 48_000.0);
        (0..samples).map(|_| v.next(p)).collect()
    }

    fn render(pitch: u8, samples: usize) -> Vec<f32> {
        render_with(&params(), pitch, 1.0, samples)
    }

    /// 高域(隣どうしの差)のエネルギーの割合
    fn brightness(out: &[f32]) -> f32 {
        let hf = out.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum::<f32>();
        hf / out.iter().map(|s| s * s).sum::<f32>().max(1e-12)
    }

    #[test]
    fn every_part_sounds_and_finishes() {
        for kit in [DrumKit::Modern, DrumKit::Tr808, DrumKit::Tr909] {
            let p = DrumParams { kit, ..params() };
            // 55(リバースクラッシュ)は立ち上がりが無音なので専用テストで検証
            for pitch in [
                35u8, 36, 37, 38, 39, 40, 41, 42, 44, 45, 46, 48, 49, 51, 52, 53, 54, 56, 60, 63,
                64, 67, 69, 70, 75, 76, 80, 81, 90,
            ] {
                let out = render_with(&p, pitch, 1.0, 4800);
                assert!(rms(&out) > 0.005, "{kit:?} pitch {pitch} が鳴るはず");
                assert!(
                    out.iter().all(|x| x.is_finite() && x.abs() < 2.0),
                    "{kit:?} pitch {pitch} が暴れない"
                );
                let mut v = DrumVoice::start(&p, pitch, 1.0, 48_000.0);
                for _ in 0..48_000 * 8 {
                    v.next(&p);
                    if v.finished(&p) {
                        break;
                    }
                }
                assert!(v.finished(&p), "{kit:?} pitch {pitch} は減衰しきるはず");
            }
        }
    }

    #[test]
    fn kick_is_darker_than_hat_and_snare_between() {
        let kick = brightness(&render(36, 4800));
        let snare = brightness(&render(38, 4800));
        let hat = brightness(&render(42, 2400));
        assert!(kick * 5.0 < snare, "kick={kick}, snare={snare}");
        assert!(snare < hat, "snare={snare}, hat={hat}");
    }

    #[test]
    fn kick_pitch_falls_to_its_tune() {
        // 鳴り始めは高く、100ms 後は 52Hz 付近(ゼロ交差の間隔で測る)
        let out = render(36, 48_000);
        let freq = |seg: &[f32]| {
            let z = seg.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
            z as f32 / (seg.len() as f32 / 48_000.0)
        };
        let late = freq(&out[4800..14_400]);
        assert!((late - 52.0).abs() < 6.0, "late={late}");
        let early = freq(&out[0..960]);
        assert!(early > late * 1.5, "early={early} late={late}");
        let tuned = render_with(
            &DrumParams {
                kick_tune: 12.0,
                ..params()
            },
            36,
            1.0,
            14_400,
        );
        let up = freq(&tuned[4800..14_400]);
        assert!(
            (up / late - 2.0).abs() < 0.2,
            "1 オクターブ上: {up} / {late}"
        );
    }

    /// 20〜120ms の区間の自己相関の最大(2〜12ms のずれ)。音程がはっきりするほど 1 に近い
    fn periodicity(out: &[f32]) -> f32 {
        let seg = &out[960..5760];
        let e0: f32 = seg.iter().map(|x| x * x).sum();
        (96..576)
            .map(|lag| {
                let c: f32 = seg[..seg.len() - lag]
                    .iter()
                    .zip(&seg[lag..])
                    .map(|(a, b)| a * b)
                    .sum();
                c / e0.max(1e-12)
            })
            .fold(f32::MIN, f32::max)
    }

    #[test]
    fn snare_is_not_a_tom() {
        // タムは音程がはっきり、スネアは響き線が主で音程感が薄い
        for kit in [DrumKit::Modern, DrumKit::Tr909] {
            let p = DrumParams { kit, ..params() };
            let snare = periodicity(&render_with(&p, 38, 1.0, 9600));
            let tom = periodicity(&render_with(&p, 45, 1.0, 9600));
            assert!(snare < 0.4, "{kit:?} snare={snare}");
            assert!(tom > 0.6, "{kit:?} tom={tom}");
        }
    }

    #[test]
    fn harder_hits_are_brighter() {
        let soft = brightness(&render_with(&params(), 38, 0.3, 4800));
        let hard = brightness(&render_with(&params(), 38, 1.0, 4800));
        assert!(hard > soft * 1.05, "soft={soft} hard={hard}");
    }

    #[test]
    fn snappy_adds_noise() {
        let dry = DrumParams {
            snare_snappy: 0.1,
            ..params()
        };
        let wet = DrumParams {
            snare_snappy: 1.0,
            ..params()
        };
        let a = brightness(&render_with(&dry, 38, 1.0, 9600));
        let b = brightness(&render_with(&wet, 38, 1.0, 9600));
        assert!(b > a * 1.5, "dry={a} wet={b}");
    }

    #[test]
    fn clap_has_bursts() {
        // 最初の 30ms に 3 つの山(9ms おき)
        let out = render(39, 1440);
        let env: Vec<f32> = out
            .chunks(48)
            .map(|c| c.iter().map(|x| x.abs()).fold(0.0, f32::max))
            .collect();
        let peaks = (1..env.len() - 1)
            .filter(|&i| env[i] > env[i - 1] && env[i] >= env[i + 1] && env[i] > 0.05)
            .count();
        assert!(peaks >= 3, "peaks={peaks} {env:?}");
    }

    #[test]
    fn closed_hat_chokes_open_hat() {
        let p = params();
        let open = DrumVoice::start(&p, 46, 1.0, 48_000.0);
        let closed = DrumVoice::start(&p, 42, 1.0, 48_000.0);
        assert_eq!(closed.group(), 1);
        assert!(open.stopped_by(1));
        assert!(!closed.stopped_by(1));
        let mut open = open;
        for _ in 0..480 {
            open.next(&p);
        }
        open.choke();
        for _ in 0..2400 {
            open.next(&p);
        }
        assert!(open.finished(&p), "チョークで 50ms 以内に止まる");
    }

    #[test]
    fn kits_differ() {
        let a = render_with(&params(), 36, 1.0, 24_000);
        let b = render_with(
            &DrumParams {
                kit: DrumKit::Tr808,
                ..params()
            },
            36,
            1.0,
            24_000,
        );
        // 808 のキックは長く伸びる(後半が大きい)
        assert!(rms(&b[12_000..]) > rms(&a[12_000..]) * 1.5);
    }

    #[test]
    fn reverse_crash_swells_toward_end() {
        // 前半より後半のほうが大きい(迫り上がる)
        let out = render(55, 48_000); // 1 秒(全長 1.6 秒の途中まで)
        let first = rms(&out[..24_000]);
        let second = rms(&out[24_000..]);
        assert!(second > first * 2.0, "first={first} second={second}");
        assert!(second > 0.01, "終盤はしっかり鳴るはず");

        // 全長を過ぎたら finished
        let p = params();
        let mut v = DrumVoice::start(&p, 55, 1.0, 48_000.0);
        for _ in 0..48_000 * 2 {
            v.next(&p);
            if v.finished(&p) {
                break;
            }
        }
        assert!(v.finished(&p));
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
