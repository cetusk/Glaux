//! 音色の共通部品(subtractive・wavetable が使う)。「生きた音」にするための仕掛けをまとめる:
//!
//! - **広がり**(spread): ユニゾンの声部を左右に振り分ける。中央の成分(mid)は振り分けない時と同じなので、
//!   モノに畳んでも音は変わらない。左右の差(side)だけが増える
//! - **揺らぎ**(analog): 声部ごとの小さなデチューンのばらつき、ゆっくりした音程・明るさの漂い、
//!   音ごとの音量のばらつき、初期位相の乱数。同じ音を繰り返しても毎回少し違う(アナログシンセの「生きた」感じ)
//! - **フィルタの種類**: 12dB のローパス(従来)・24dB のローパス・ハイパス・バンドパス・ノッチと、
//!   フィルタの前の歪み(drive。ADAA の tanh)
//! - **フィルタのエンベロープ**: 従来は音量の ADSR を流用。filter_decay を 0 より大きくすると独立した AD(S)
//! - **ベロシティとキーの追従**: 強く弾くと明るい・高い音ほど明るい
//! - **LFO 2 本**: 形(sine / triangle / square / saw / random)と行き先(音程・カットオフ・音量・パン・position)
//!
//! 新しいつまみがすべて既定値なら、従来とまったく同じ音になる(既存の曲の音を変えない)。
//! 揺らぎの乱数は音ごとに決まる種から作るので、同じ曲は何度描き出しても同じ音になる。
//! RT セーフ: 値型のみでアロケーションなし。

use crate::oversample::AdaaTanh;

/// フィルタの種類
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FilterType {
    /// 12dB/oct のローパス(従来の音)
    #[default]
    Lp12,
    /// 24dB/oct のローパス(SVF を 2 段。より急に高域が落ちる、太いアナログの低音)
    Lp24,
    /// ハイパス(低域を削る。細く・軽く)
    Hp,
    /// バンドパス(中域だけ。電話・ラジオ・鼻にかかった音)
    Bp,
    /// ノッチ(カットオフ付近だけ削る。フェイザーのような抜け)
    Notch,
}

impl FilterType {
    pub const NAMES: [&'static str; 5] = ["lp12", "lp24", "hp", "bp", "notch"];

    pub fn parse(s: &str) -> FilterType {
        match s {
            "lp24" => FilterType::Lp24,
            "hp" => FilterType::Hp,
            "bp" => FilterType::Bp,
            "notch" => FilterType::Notch,
            _ => FilterType::Lp12,
        }
    }
}

/// LFO の形
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LfoShape {
    #[default]
    Sine,
    Triangle,
    Square,
    Saw,
    /// 1 周期ごとに乱数(サンプル&ホールド)
    Random,
}

impl LfoShape {
    pub const NAMES: [&'static str; 5] = ["sine", "triangle", "square", "saw", "random"];

    pub fn parse(s: &str) -> LfoShape {
        match s {
            "triangle" => LfoShape::Triangle,
            "square" => LfoShape::Square,
            "saw" => LfoShape::Saw,
            "random" => LfoShape::Random,
            _ => LfoShape::Sine,
        }
    }
}

/// LFO の行き先
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LfoTarget {
    /// 音程(深さ 1 で ±2 半音。0.05〜0.15 でビブラート)
    Pitch,
    /// カットオフ(深さ 1 で ±3 オクターブ。ワウ・ウォブル)
    #[default]
    Cutoff,
    /// 音量(深さ 1 で無音まで。トレモロ)
    Amp,
    /// 左右(深さ 1 で端まで。オートパン)
    Pan,
    /// ウェーブテーブルの位置(wavetable だけ。深さ 1 で ±0.5)
    Position,
}

impl LfoTarget {
    pub const NAMES: [&'static str; 5] = ["pitch", "cutoff", "amp", "pan", "position"];

    pub fn parse(s: &str) -> LfoTarget {
        match s {
            "pitch" => LfoTarget::Pitch,
            "amp" => LfoTarget::Amp,
            "pan" => LfoTarget::Pan,
            "position" => LfoTarget::Position,
            _ => LfoTarget::Cutoff,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LfoParams {
    /// Hz
    pub rate: f32,
    /// 0..=1(0 = 掛けない)
    pub depth: f32,
    pub shape: LfoShape,
    pub target: LfoTarget,
}

impl Default for LfoParams {
    fn default() -> Self {
        LfoParams {
            rate: 2.0,
            depth: 0.0,
            shape: LfoShape::Sine,
            target: LfoTarget::Cutoff,
        }
    }
}

/// 焼き込み済みの共通のつまみ(1 トラック分)。既定値は「従来と同じ音」
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToneParams {
    /// ユニゾンの左右への広がり 0..=1
    pub spread: f32,
    /// 揺らぎ 0..=1
    pub analog: f32,
    pub filter_type: FilterType,
    /// フィルタの前の歪み 0..=1
    pub drive: f32,
    /// ベロシティでカットオフが変わる量 0..=1(1 で最弱の音が 3 オクターブ暗い)
    pub vel_cutoff: f32,
    /// キー(音の高さ)にカットオフが付いていく量 0..=1(1 で 1 オクターブ上がるとカットオフも 1 オクターブ上がる)
    pub key_track: f32,
    /// フィルタのエンベロープの立ち上がり(秒)
    pub filter_attack: f32,
    /// フィルタのエンベロープの減衰(秒。0 = 音量の ADSR を使う = 従来)
    pub filter_decay: f32,
    /// フィルタのエンベロープの減衰後の高さ 0..=1
    pub filter_sustain: f32,
    pub lfo: [LfoParams; 2],
}

impl Default for ToneParams {
    fn default() -> Self {
        ToneParams {
            spread: 0.0,
            analog: 0.0,
            filter_type: FilterType::Lp12,
            drive: 0.0,
            vel_cutoff: 0.0,
            key_track: 0.0,
            filter_attack: 0.003,
            filter_decay: 0.0,
            filter_sustain: 0.0,
            lfo: [
                LfoParams {
                    rate: 5.0,
                    ..LfoParams::default()
                },
                LfoParams::default(),
            ],
        }
    }
}

impl ToneParams {
    /// 連続値のつまみをオートメーションで変える(変えられたら true)
    pub fn set_continuous(&mut self, name: &str, v: f32) -> bool {
        match name {
            "spread" => self.spread = v.clamp(0.0, 1.0),
            "analog" => self.analog = v.clamp(0.0, 1.0),
            "drive" => self.drive = v.clamp(0.0, 1.0),
            "vel_cutoff" => self.vel_cutoff = v.clamp(0.0, 1.0),
            "key_track" => self.key_track = v.clamp(0.0, 1.0),
            "filter_attack" => self.filter_attack = v.clamp(0.001, 5.0),
            "filter_decay" => self.filter_decay = v.clamp(0.0, 5.0),
            "filter_sustain" => self.filter_sustain = v.clamp(0.0, 1.0),
            "lfo1_rate" => self.lfo[0].rate = v.clamp(0.05, 20.0),
            "lfo1_depth" => self.lfo[0].depth = v.clamp(0.0, 1.0),
            "lfo2_rate" => self.lfo[1].rate = v.clamp(0.05, 20.0),
            "lfo2_depth" => self.lfo[1].depth = v.clamp(0.0, 1.0),
            _ => return false,
        }
        true
    }

    /// 既定(従来と同じ音)のままか。そうなら声の側の追加の計算を省ける
    fn is_plain(&self) -> bool {
        self.spread == 0.0
            && self.analog == 0.0
            && self.filter_type == FilterType::Lp12
            && self.drive == 0.0
            && self.lfo[0].depth == 0.0
            && self.lfo[1].depth == 0.0
    }
}

/// 最大のユニゾン数(subtractive・wavetable と同じ)
pub const MAX_UNISON: usize = 7;

/// 声の側の状態(1 音分)
#[derive(Clone, Copy, Debug)]
pub struct ToneVoice {
    rng: u32,
    /// 声部ごとの左右の位置 −1..1(spread 1 のとき)
    pub pan: [f32; MAX_UNISON],
    /// 声部ごとの音程のばらつき(セント。analog 1 のとき)
    pub detune: [f32; MAX_UNISON],
    /// 音ごとの音量のばらつき(倍率の対数の素。analog 1 のとき ±1)
    level_rand: f32,
    /// ゆっくり漂う音程・明るさ(−1..1 の乱歩と、その目標)
    drift_pitch: f32,
    drift_pitch_to: f32,
    drift_bright: f32,
    drift_bright_to: f32,
    /// LFO の位相と、乱数の形の今の値
    lfo_phase: [f32; 2],
    lfo_hold: [f32; 2],
    /// 制御レートで求めた変調の値
    pub pitch_ratio: f32,
    pub cutoff_mul: f32,
    pub amp_mul: f32,
    pub pan_mod: f32,
    pub position_add: f32,
    /// フィルタのエンベロープ
    fenv: f32,
    fenv_stage: u8,
    /// フィルタの状態(mid / side。24dB は 2 段)
    svf: [[Svf; 2]; 2],
    drive: [AdaaTanh; 2],
    /// side の経路を使っているか(広がり・パンの LFO があるとき)
    pub stereo: bool,
    /// 従来と同じ計算でよいか(新しいつまみが全部既定値)
    pub plain: bool,
}

/// TPT の状態変数フィルタ(1 チャンネル分の状態)
#[derive(Clone, Copy, Debug, Default)]
struct Svf {
    ic1: f32,
    ic2: f32,
}

impl Svf {
    #[inline]
    fn tick(&mut self, x: f32, c: &SvfCoefs) -> (f32, f32, f32) {
        let v1 = c.a1 * (self.ic1 + c.g * (x - self.ic2));
        let v2 = self.ic2 + c.g * v1;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        // (ローパス, バンドパス, ハイパス)
        (v2, v1, x - c.k * v1 - v2)
    }
}

/// フィルタの係数(制御レートで更新)
#[derive(Clone, Copy, Debug, Default)]
pub struct SvfCoefs {
    g: f32,
    k: f32,
    a1: f32,
    /// 24dB の 2 段目(共振を抑える)
    k2: f32,
    a1b: f32,
}

impl SvfCoefs {
    pub fn new(fc: f32, resonance: f32, sr: f32) -> SvfCoefs {
        let g = (std::f32::consts::PI * fc / sr).tan();
        let k = 2.0 * (1.0 - resonance.min(0.95));
        let k2 = 2.0 * (1.0 - (resonance * 0.5).min(0.95));
        SvfCoefs {
            g,
            k,
            a1: 1.0 / (1.0 + g * (g + k)),
            k2,
            a1b: 1.0 / (1.0 + g * (g + k2)),
        }
    }
}

impl ToneVoice {
    pub fn new(seed: u32) -> ToneVoice {
        let mut v = ToneVoice {
            rng: 1,
            pan: [0.0; MAX_UNISON],
            detune: [0.0; MAX_UNISON],
            level_rand: 0.0,
            drift_pitch: 0.0,
            drift_pitch_to: 0.0,
            drift_bright: 0.0,
            drift_bright_to: 0.0,
            lfo_phase: [0.0; 2],
            lfo_hold: [0.0; 2],
            pitch_ratio: 1.0,
            cutoff_mul: 1.0,
            amp_mul: 1.0,
            pan_mod: 0.0,
            position_add: 0.0,
            fenv: 0.0,
            fenv_stage: 0,
            svf: [[Svf::default(); 2]; 2],
            drive: [AdaaTanh::default(); 2],
            stereo: false,
            plain: true,
        };
        v.reseed(seed);
        v
    }

    /// 種を決め直す(音ごとに違う種を渡すと、同じ音を繰り返しても少しずつ違う音になる)
    pub fn reseed(&mut self, seed: u32) {
        self.rng = (seed ^ 0x5bd1_e995).wrapping_mul(0x9e37_79b9) | 1;
        for i in 0..MAX_UNISON {
            self.detune[i] = self.rand();
        }
        self.level_rand = self.rand();
        self.drift_pitch = self.rand() * 0.5;
        self.drift_pitch_to = self.rand();
        self.drift_bright = self.rand() * 0.5;
        self.drift_bright_to = self.rand();
        for k in 0..2 {
            self.lfo_hold[k] = self.rand();
        }
    }

    /// −1..1 の乱数(xorshift32)
    pub fn rand(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    /// 声部の左右の位置を決める(声部数が変わったときに呼ぶ)。外側ほど端に、交互に左右へ
    pub fn layout(&mut self, n: usize) {
        for i in 0..MAX_UNISON {
            self.pan[i] = if n <= 1 || i >= n {
                0.0
            } else {
                // デチューンの並び(−1..1)と同じ順に左右を交互にする(低い声部と高い声部が同じ側に偏らない)
                let x = (i as f32 / (n - 1) as f32) * 2.0 - 1.0;
                if i % 2 == 0 {
                    x.abs()
                } else {
                    -x.abs()
                }
            };
        }
    }

    /// 音の頭に呼ぶ。ユニゾンの初期位相を乱数にする(analog があるとき。無ければ従来の固定の並び)
    pub fn start_phases(&mut self, p: &ToneParams, phases: &mut [f32; MAX_UNISON]) {
        if p.analog > 0.0 {
            for ph in phases.iter_mut() {
                let r = self.rand() * 0.5 + 0.5;
                *ph = (*ph + r * p.analog).fract();
            }
        }
    }

    /// 制御レート(32 サンプルごと)で変調を進める。`n` はこの間のサンプル数、`amp_env` は音量の包絡、
    /// `released` はノートオフ後か、`vel` はベロシティ
    pub fn control(&mut self, p: &ToneParams, n: u32, sr: f32, released: bool) {
        self.plain = p.is_plain();
        let dt = n as f32 / sr;
        // フィルタのエンベロープ(独立させたときだけ)
        if p.filter_decay > 0.0 {
            match (self.fenv_stage, released) {
                (_, true) => {
                    self.fenv -= self.fenv * (dt * 6.0 / 0.3).min(1.0);
                    self.fenv_stage = 2;
                }
                (0, _) => {
                    self.fenv += dt / p.filter_attack.max(0.001);
                    if self.fenv >= 1.0 {
                        self.fenv = 1.0;
                        self.fenv_stage = 1;
                    }
                }
                _ => {
                    let c = (dt * 6.9 / p.filter_decay.max(0.005)).min(1.0);
                    self.fenv += (p.filter_sustain - self.fenv) * c;
                }
            }
        }
        // 漂い(0.3〜1Hz ほどで目標へ寄り、着いたら次の目標)
        if p.analog > 0.0 {
            let c = (dt * 1.5).min(1.0);
            self.drift_pitch += (self.drift_pitch_to - self.drift_pitch) * c;
            if (self.drift_pitch - self.drift_pitch_to).abs() < 0.05 {
                self.drift_pitch_to = self.rand();
            }
            self.drift_bright += (self.drift_bright_to - self.drift_bright) * c * 0.7;
            if (self.drift_bright - self.drift_bright_to).abs() < 0.05 {
                self.drift_bright_to = self.rand();
            }
        }
        // LFO
        let mut pitch_semi = 0.0f32;
        let mut cutoff_oct = 0.0f32;
        let mut amp = 1.0f32;
        let mut pan = 0.0f32;
        let mut pos = 0.0f32;
        for k in 0..2 {
            let l = p.lfo[k];
            if l.depth <= 0.0 {
                continue;
            }
            let ph = self.lfo_phase[k];
            let v = match l.shape {
                LfoShape::Sine => (ph * std::f32::consts::TAU).sin(),
                LfoShape::Triangle => 1.0 - 4.0 * (ph - 0.5).abs(),
                LfoShape::Square => {
                    if ph < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                }
                LfoShape::Saw => 1.0 - 2.0 * ph,
                LfoShape::Random => self.lfo_hold[k],
            };
            let next = ph + l.rate * dt;
            if next >= 1.0 && l.shape == LfoShape::Random {
                self.lfo_hold[k] = self.rand();
            }
            self.lfo_phase[k] = next.fract();
            match l.target {
                LfoTarget::Pitch => pitch_semi += v * l.depth * 2.0,
                LfoTarget::Cutoff => cutoff_oct += v * l.depth * 3.0,
                LfoTarget::Amp => amp *= 1.0 - l.depth * (0.5 - 0.5 * v),
                LfoTarget::Pan => pan += v * l.depth,
                LfoTarget::Position => pos += v * l.depth * 0.5,
            }
        }
        // 揺らぎ: 音程 ±4 セント、明るさ ±0.15 オクターブ
        let drift_cents = p.analog * 4.0 * self.drift_pitch;
        let drift_oct = p.analog * 0.15 * self.drift_bright;
        self.pitch_ratio = if pitch_semi == 0.0 && drift_cents == 0.0 {
            1.0
        } else {
            (2.0f32).powf(pitch_semi / 12.0 + drift_cents / 1200.0)
        };
        self.cutoff_mul = if cutoff_oct == 0.0 && drift_oct == 0.0 {
            1.0
        } else {
            (2.0f32).powf(cutoff_oct + drift_oct)
        };
        // 音ごとの音量のばらつき ±1dB
        self.amp_mul = if p.analog > 0.0 {
            amp * (1.0 + 0.11 * p.analog * self.level_rand)
        } else {
            amp
        };
        self.pan_mod = pan.clamp(-1.0, 1.0);
        self.position_add = pos;
        self.stereo = p.spread > 0.0 || pan != 0.0;
    }

    /// カットオフ(Hz)を、フィルタのエンベロープ・ベロシティ・キー・変調で動かす。
    /// `env` は従来どおり音量の包絡(filter_decay 0 のとき)、`filter_env` はエンベロープの深さ
    pub fn cutoff(
        &self,
        p: &ToneParams,
        base: f32,
        filter_env: f32,
        env: f32,
        vel: f32,
        freq: f32,
    ) -> f32 {
        let e = if p.filter_decay > 0.0 { self.fenv } else { env };
        let mut fc = base * (2.0_f32).powf(filter_env * e * 3.0);
        if p.vel_cutoff > 0.0 {
            fc *= (2.0_f32).powf((vel.clamp(0.0, 1.0) - 1.0) * p.vel_cutoff * 3.0);
        }
        if p.key_track > 0.0 {
            fc *= (freq / 261.63).max(1e-3).powf(p.key_track);
        }
        fc * self.cutoff_mul
    }

    /// 声部 i の音程の倍率(揺らぎのばらつき、analog 1 で ±6 セント)
    #[inline]
    pub fn unison_detune(&self, p: &ToneParams, i: usize) -> f32 {
        if p.analog > 0.0 {
            1.0 + self.detune[i] * p.analog * 6.0 * (std::f32::consts::LN_2 / 1200.0)
        } else {
            1.0
        }
    }

    /// 声部 i の左右の位置(−1..1)
    #[inline]
    pub fn unison_pan(&self, p: &ToneParams, i: usize) -> f32 {
        self.pan[i] * p.spread * 0.9
    }

    /// (mid, side) をフィルタに通す。戻り値も (mid, side)
    #[inline]
    pub fn filter(&mut self, p: &ToneParams, c: &SvfCoefs, mid: f32, side: f32) -> (f32, f32) {
        let (mut m, mut s) = (mid, side);
        if p.drive > 0.0 {
            // 左右それぞれで歪ませる(ADAA の tanh。1 + drive × 7 倍まで押し込み、音量は戻す)
            let pre = 1.0 + p.drive * 7.0;
            let post = 1.0 / (1.0 + p.drive * 2.0);
            let (l, r) = (m + s, m - s);
            let l = self.drive[0].process(l * pre) * post;
            let r = if self.stereo {
                self.drive[1].process(r * pre) * post
            } else {
                l
            };
            m = 0.5 * (l + r);
            s = 0.5 * (l - r);
        }
        let mid_out = Self::run(&mut self.svf[0], p.filter_type, c, m);
        let side_out = if self.stereo {
            Self::run(&mut self.svf[1], p.filter_type, c, s)
        } else {
            0.0
        };
        (mid_out, side_out)
    }

    #[inline]
    fn run(st: &mut [Svf; 2], t: FilterType, c: &SvfCoefs, x: f32) -> f32 {
        let (lp, bp, hp) = st[0].tick(x, c);
        match t {
            FilterType::Lp12 => lp,
            FilterType::Hp => hp,
            FilterType::Bp => bp * c.k,
            FilterType::Notch => lp + hp,
            FilterType::Lp24 => {
                let c2 = SvfCoefs {
                    k: c.k2,
                    a1: c.a1b,
                    ..*c
                };
                st[1].tick(lp, &c2).0
            }
        }
    }

    /// パンの LFO を (mid, side) に掛ける
    #[inline]
    pub fn apply_pan(&self, mid: f32, side: f32) -> (f32, f32) {
        if self.pan_mod == 0.0 {
            (mid, side)
        } else {
            (mid, side + mid * self.pan_mod * 0.9)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for n in FilterType::NAMES {
            assert_eq!(FilterType::NAMES[FilterType::parse(n) as usize], n);
        }
        for n in LfoShape::NAMES {
            assert_eq!(LfoShape::NAMES[LfoShape::parse(n) as usize], n);
        }
        for n in LfoTarget::NAMES {
            assert_eq!(LfoTarget::NAMES[LfoTarget::parse(n) as usize], n);
        }
    }

    #[test]
    fn layout_alternates_sides_and_keeps_one_voice_centered() {
        let mut v = ToneVoice::new(1);
        v.layout(1);
        assert_eq!(v.pan[0], 0.0);
        v.layout(4);
        let p = &v.pan[..4];
        assert!(p.iter().filter(|x| **x > 0.0).count() >= 1);
        assert!(p.iter().filter(|x| **x < 0.0).count() >= 1);
        assert!(p.iter().all(|x| x.abs() <= 1.0));
    }

    #[test]
    fn seeds_change_the_variation_but_are_repeatable() {
        let a = ToneVoice::new(7);
        let b = ToneVoice::new(7);
        let c = ToneVoice::new(8);
        assert_eq!(a.detune, b.detune);
        assert_ne!(a.detune, c.detune);
    }

    #[test]
    fn filter_types_shape_the_spectrum() {
        // 1kHz のカットオフで、100Hz と 8kHz の正弦波の通り方を比べる
        let sr = 48_000.0;
        let gain = |t: FilterType, f: f32| {
            let p = ToneParams {
                filter_type: t,
                ..ToneParams::default()
            };
            let c = SvfCoefs::new(1000.0, 0.0, sr);
            let mut v = ToneVoice::new(1);
            let mut acc = 0.0f32;
            for i in 0..9600 {
                let x = (i as f32 * f / sr * std::f32::consts::TAU).sin();
                let (y, _) = v.filter(&p, &c, x, 0.0);
                if i > 4800 {
                    acc += y * y;
                }
            }
            (acc / 4800.0).sqrt()
        };
        assert!(gain(FilterType::Lp12, 100.0) > gain(FilterType::Lp12, 8000.0) * 10.0);
        assert!(gain(FilterType::Lp24, 8000.0) < gain(FilterType::Lp12, 8000.0) * 0.3);
        assert!(gain(FilterType::Hp, 8000.0) > gain(FilterType::Hp, 100.0) * 10.0);
        assert!(gain(FilterType::Bp, 1000.0) > gain(FilterType::Bp, 100.0) * 3.0);
        assert!(gain(FilterType::Notch, 1000.0) < gain(FilterType::Notch, 100.0) * 0.3);
    }
}
