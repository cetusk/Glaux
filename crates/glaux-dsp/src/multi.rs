//! マルチサンプラー音源 `sf2`(SoundFont のゾーンを再生する)。
//!
//! 「音域 × ベロシティごとに別サンプル + ループ点 + 音量エンベロープ」という
//! SoundFont の中身を [`Zone`] の列に落として再生する。ゾーンの解決
//! (.sf2 のパースとプリセット/インストゥルメントの合成)はエンジン側
//! (`glaux-engine/src/sf2.rs`)が行い、dsp は出来上がったゾーン列を鳴らすだけ。
//!
//! 第 2 段でローパスフィルタ(initialFilterFc/Q)、ビブラート LFO、
//! モジュレーション LFO(ピッチ / フィルタ)、モジュレーションエンベロープ
//! (ピッチ / フィルタ)に対応した([`ZoneMod`])。これらは 32 サンプルごとの
//! 制御レートで評価する(pow / tan をサンプルごとに呼ばない)。
//! CC 経由のモジュレータ(モジュレーションホイール等)は未対応。

use crate::expr::PitchExpr;
use crate::sampler::SampleData;
use glaux_core::Articulation;
use std::sync::Arc;

/// 同時に鳴らすレイヤー数の上限(ステレオペア + 重ね録りを想定)。
pub const MAX_LAYERS: usize = 4;

/// 音量エンベロープ(SF2 の DAHDSR から delay を除いた形)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoneEnv {
    /// 秒
    pub attack: f32,
    pub hold: f32,
    pub decay: f32,
    /// 0..=1(振幅)
    pub sustain: f32,
    pub release: f32,
}

/// ゾーンの変調設定(SF2 ジェネレータ由来)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoneMod {
    /// 初期カットオフ(Hz)。既定 13500 = 実質バイパス
    pub cutoff_hz: f32,
    /// フィルタ Q(dB)
    pub q_db: f32,
    /// ビブラート LFO: ピッチへの深さ(セント)/ 周波数(Hz)/ 開始までの遅延(秒)
    pub vib_to_pitch: f32,
    pub vib_freq: f32,
    pub vib_delay: f32,
    /// モジュレーション LFO: ピッチ(セント)/ フィルタ(セント)/ 周波数(Hz)/ 遅延(秒)
    pub mod_to_pitch: f32,
    pub mod_to_filter: f32,
    pub mod_freq: f32,
    pub mod_delay: f32,
    /// モジュレーションエンベロープ: ピッチ(セント)/ フィルタ(セント)と時間(秒)
    pub env_to_pitch: f32,
    pub env_to_filter: f32,
    pub env_delay: f32,
    pub env_attack: f32,
    pub env_hold: f32,
    pub env_decay: f32,
    /// 0..=1(レベル)
    pub env_sustain: f32,
    pub env_release: f32,
}

impl Default for ZoneMod {
    fn default() -> Self {
        ZoneMod {
            cutoff_hz: 13_500.0,
            q_db: 0.0,
            vib_to_pitch: 0.0,
            vib_freq: 8.2,
            vib_delay: 0.0,
            mod_to_pitch: 0.0,
            mod_to_filter: 0.0,
            mod_freq: 8.2,
            mod_delay: 0.0,
            env_to_pitch: 0.0,
            env_to_filter: 0.0,
            env_delay: 0.0,
            env_attack: 0.001,
            env_hold: 0.0,
            env_decay: 0.001,
            env_sustain: 1.0,
            env_release: 0.001,
        }
    }
}

impl ZoneMod {
    /// 変調もフィルタも効かない(= 処理を省ける)か
    fn is_inert(&self) -> bool {
        self.cutoff_hz >= 13_000.0
            && self.vib_to_pitch == 0.0
            && self.mod_to_pitch == 0.0
            && self.mod_to_filter == 0.0
            && self.env_to_pitch == 0.0
            && self.env_to_filter == 0.0
    }
}

/// 制御レート(サンプル)。LFO / エンベロープ / フィルタ係数をこの間隔で更新する
const CTRL_RATE: u32 = 32;

/// 1 ゾーン = 1 サンプル + 適用範囲 + 再生条件。
#[derive(Clone, Debug)]
pub struct Zone {
    pub key_lo: u8,
    pub key_hi: u8,
    pub vel_lo: u8,
    pub vel_hi: u8,
    /// スライス済みサンプル(ゾーンの start..end)
    pub data: Arc<SampleData>,
    /// `data` 内のループ区間(開始, 終了)。None ならワンショット
    pub loop_range: Option<(f64, f64)>,
    /// true なら note_off 後はループを抜けて末尾まで再生(SF2 mode 3)
    pub loop_until_release: bool,
    /// 実効ルート(セミトーン。coarse/fine チューニング込み)
    pub root: f32,
    /// ゾーン固有のゲイン(initialAttenuation 由来、リニア)
    pub gain: f32,
    pub env: ZoneEnv,
    /// フィルタ・LFO・モジュレーションエンベロープ
    pub modu: ZoneMod,
}

impl Zone {
    pub fn contains(&self, key: u8, vel: u8) -> bool {
        self.key_lo <= key && key <= self.key_hi && self.vel_lo <= vel && vel <= self.vel_hi
    }
}

/// 焼き込み済みパラメータ(1 トラック分)。
#[derive(Clone, Debug)]
pub struct MultiSamplerParams {
    pub zones: Arc<Vec<Zone>>,
    /// 全体ゲイン(gain_db から変換済み)
    pub gain: f32,
}

impl PartialEq for MultiSamplerParams {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.zones, &other.zones) && self.gain == other.gain
    }
}

const STAGE_ATTACK: u8 = 0;
const STAGE_HOLD: u8 = 1;
const STAGE_DECAY: u8 = 2;
const STAGE_RELEASE: u8 = 3;

#[derive(Clone, Copy, Debug, Default)]
struct ZonePlayer {
    active: bool,
    /// `MultiSamplerParams::zones` への添字
    zone: u16,
    pos: f64,
    rate: f64,
    env: f32,
    stage: u8,
    hold_left: f32,
    /// ---- 変調(制御レートで更新)----
    /// LFO / エンベロープ由来のピッチ倍率
    pitch_mul: f64,
    /// フィルタを通すか(カットオフが開き切っていれば省く)
    filter_on: bool,
    /// TPT SVF の係数と状態
    a1: f32,
    a2: f32,
    a3: f32,
    ic1: f32,
    ic2: f32,
    /// モジュレーションエンベロープ
    menv: f32,
    menv_stage: u8,
    menv_hold_left: f32,
}

impl ZonePlayer {
    /// モジュレーションエンベロープを `dt` 秒ぶん進めて現在値(0..1)を返す。
    fn advance_menv(&mut self, m: &ZoneMod, released: bool, dt: f32) -> f32 {
        if released && self.menv_stage != STAGE_RELEASE {
            self.menv_stage = STAGE_RELEASE;
        }
        match self.menv_stage {
            STAGE_ATTACK => {
                self.menv += dt / m.env_attack.max(0.001);
                if self.menv >= 1.0 {
                    self.menv = 1.0;
                    self.menv_stage = STAGE_HOLD;
                    self.menv_hold_left = m.env_hold;
                }
            }
            STAGE_HOLD => {
                self.menv_hold_left -= dt;
                if self.menv_hold_left <= 0.0 {
                    self.menv_stage = STAGE_DECAY;
                }
            }
            STAGE_DECAY => {
                let coef = (-dt / m.env_decay.max(0.001)).exp();
                self.menv = m.env_sustain + (self.menv - m.env_sustain) * coef;
            }
            _ => {
                let coef = (-dt / m.env_release.max(0.001)).exp();
                self.menv *= coef;
            }
        }
        self.menv
    }

    /// 制御レートの更新: LFO・エンベロープからピッチ倍率とフィルタ係数を決める。
    fn update_control(&mut self, z: &Zone, t: f32, vib: f32, lfo: f32, released: bool, sr: f32) {
        let m = &z.modu;
        let menv = if m.env_to_pitch != 0.0 || m.env_to_filter != 0.0 {
            self.advance_menv(m, released, CTRL_RATE as f32 / sr)
        } else {
            0.0
        };
        let vib = if t >= m.vib_delay { vib } else { 0.0 };
        let lfo = if t >= m.mod_delay { lfo } else { 0.0 };

        let cents = vib * m.vib_to_pitch + lfo * m.mod_to_pitch + menv * m.env_to_pitch;
        self.pitch_mul = if cents == 0.0 {
            1.0
        } else {
            (2.0_f64).powf(cents as f64 / 1200.0)
        };

        let fc_cents = lfo * m.mod_to_filter + menv * m.env_to_filter;
        let fc = m.cutoff_hz * (2.0_f32).powf(fc_cents / 1200.0);
        if fc >= 13_000.0 {
            self.filter_on = false;
            return;
        }
        self.filter_on = true;
        let fc = fc.clamp(20.0, sr * 0.45);
        let g = (std::f32::consts::PI * fc / sr).tan();
        // Q(dB)→ 減衰係数 k = 1/Q。0dB で Butterworth 相当
        let q = (10.0_f32).powf(m.q_db.clamp(0.0, 96.0) / 20.0);
        let k = (1.0 / q).clamp(0.05, 2.0);
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    /// TPT 状態変数フィルタ(ローパス出力)。
    #[inline]
    fn filter(&mut self, x: f32) -> f32 {
        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        v2
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MultiVoice {
    players: [ZonePlayer; MAX_LAYERS],
    /// レイヤー数に応じた等パワー正規化
    layer_norm: f32,
    amp: f32,
    released: bool,
    pub(crate) expr: PitchExpr,
    sample_rate: f32,
    /// 経過サンプル数(制御レートのタイミングと LFO 遅延に使う)
    age: u32,
    /// LFO 位相(0..1)。ビブラート LFO とモジュレーション LFO
    vib_phase: f32,
    mod_phase: f32,
    /// いずれかのゾーンに変調があるか(無ければ制御レート処理を丸ごと省く)
    modulated: bool,
}

impl MultiVoice {
    pub fn start(
        p: &MultiSamplerParams,
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
        let vel_midi = (vel * 127.0).clamp(1.0, 127.0) as u8;
        let mut players = [ZonePlayer::default(); MAX_LAYERS];
        let mut n = 0usize;
        let mut modulated = false;
        for (i, z) in p.zones.iter().enumerate() {
            if n >= MAX_LAYERS {
                break;
            }
            if !z.contains(pitch, vel_midi) {
                continue;
            }
            let semis = pitch as f64 - z.root as f64;
            let mut pl = ZonePlayer {
                active: true,
                zone: i as u16,
                pos: 0.0,
                rate: (z.data.sample_rate as f64 / sample_rate as f64)
                    * (2.0_f64).powf(semis / 12.0),
                env: 0.0,
                stage: STAGE_ATTACK,
                hold_left: z.env.hold * sample_rate,
                pitch_mul: 1.0,
                ..ZonePlayer::default()
            };
            if !z.modu.is_inert() {
                modulated = true;
                pl.update_control(z, 0.0, 0.0, 0.0, false, sample_rate);
            }
            players[n] = pl;
            n += 1;
        }
        MultiVoice {
            players,
            layer_norm: 1.0 / (n.max(1) as f32).sqrt(),
            amp: vel * amp_mul,
            released: false,
            expr: PitchExpr::new(articulation, sample_rate),
            sample_rate,
            age: 0,
            vib_phase: 0.0,
            mod_phase: 0.0,
            modulated,
        }
    }

    pub fn note_off(&mut self) {
        self.released = true;
        for pl in &mut self.players {
            if pl.active {
                pl.stage = STAGE_RELEASE;
            }
        }
    }

    pub fn finished(&self) -> bool {
        self.players.iter().all(|pl| !pl.active)
    }

    pub fn next(&mut self, p: &MultiSamplerParams) -> f32 {
        let sr = self.sample_rate;
        let ratio = if self.expr.is_active() {
            self.expr.next_ratio(sr) as f64
        } else {
            1.0
        };

        // 制御レート: LFO を進め、各ゾーンのピッチ倍率とフィルタ係数を更新
        if self.modulated && self.age % CTRL_RATE == 0 {
            let t = self.age as f32 / sr;
            let vib = (self.vib_phase * std::f32::consts::TAU).sin();
            let lfo = (self.mod_phase * std::f32::consts::TAU).sin();
            let (mut vib_freq, mut mod_freq) = (0.0f32, 0.0f32);
            for pl in &mut self.players {
                if !pl.active {
                    continue;
                }
                let Some(z) = p.zones.get(pl.zone as usize) else {
                    continue;
                };
                if z.modu.is_inert() {
                    continue;
                }
                pl.update_control(z, t, vib, lfo, self.released, sr);
                vib_freq = vib_freq.max(z.modu.vib_freq);
                mod_freq = mod_freq.max(z.modu.mod_freq);
            }
            self.vib_phase = (self.vib_phase + vib_freq * CTRL_RATE as f32 / sr).fract();
            self.mod_phase = (self.mod_phase + mod_freq * CTRL_RATE as f32 / sr).fract();
        }
        self.age = self.age.wrapping_add(1);

        let mut out = 0.0f32;
        for pl in &mut self.players {
            if !pl.active {
                continue;
            }
            let Some(z) = p.zones.get(pl.zone as usize) else {
                pl.active = false;
                continue;
            };
            let frames = &z.data.frames;

            // ループ処理(mode 3 はリリース後にループを抜ける)
            if let Some((ls, le)) = z.loop_range {
                let looping = !(z.loop_until_release && self.released);
                if looping && pl.pos >= le && le > ls {
                    pl.pos = ls + (pl.pos - le) % (le - ls);
                }
            }
            let i = pl.pos as usize;
            if i + 1 >= frames.len() {
                pl.active = false;
                continue;
            }
            let frac = (pl.pos - i as f64) as f32;
            let mut s = frames[i] + (frames[i + 1] - frames[i]) * frac;
            pl.pos += pl.rate * ratio * pl.pitch_mul;
            if pl.filter_on {
                s = pl.filter(s);
            }

            // 音量エンベロープ
            match pl.stage {
                STAGE_ATTACK => {
                    pl.env += 1.0 / (z.env.attack.max(0.001) * sr);
                    if pl.env >= 1.0 {
                        pl.env = 1.0;
                        pl.stage = STAGE_HOLD;
                    }
                }
                STAGE_HOLD => {
                    pl.hold_left -= 1.0;
                    if pl.hold_left <= 0.0 {
                        pl.stage = STAGE_DECAY;
                    }
                }
                STAGE_DECAY => {
                    let coef = 1.0 - 1.0 / (z.env.decay.max(0.005) * sr);
                    pl.env = z.env.sustain + (pl.env - z.env.sustain) * coef;
                    // サスティンほぼ 0 のゾーン(ピアノ等)は減衰しきったら解放
                    if pl.env < 1e-4 {
                        pl.active = false;
                        continue;
                    }
                }
                _ => {
                    let coef = 1.0 - 1.0 / (z.env.release.max(0.005) * sr);
                    pl.env *= coef;
                    if pl.env < 1e-4 {
                        pl.active = false;
                        continue;
                    }
                }
            }

            out += s * pl.env * z.gain;
        }
        out * self.layer_norm * self.amp * p.gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_data(freq: f32, secs: f32, sr: f32) -> Arc<SampleData> {
        let frames = (0..(sr * secs) as usize)
            .map(|i| (i as f32 * freq * std::f32::consts::TAU / sr).sin() * 0.5)
            .collect();
        Arc::new(SampleData {
            frames,
            sample_rate: sr,
            side: None,
        })
    }

    fn env() -> ZoneEnv {
        ZoneEnv {
            attack: 0.002,
            hold: 0.0,
            decay: 100.0,
            sustain: 1.0,
            release: 0.05,
        }
    }

    fn zone(key_lo: u8, key_hi: u8, root: f32, data: Arc<SampleData>) -> Zone {
        Zone {
            key_lo,
            key_hi,
            vel_lo: 0,
            vel_hi: 127,
            data,
            loop_range: None,
            loop_until_release: false,
            root,
            gain: 1.0,
            env: env(),
            modu: ZoneMod::default(),
        }
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn selects_zone_by_key_range() {
        // 低音域と高音域で別サンプル(周波数で見分ける)
        let low = sine_data(110.0, 0.5, 48_000.0);
        let high = sine_data(880.0, 0.5, 48_000.0);
        let p = MultiSamplerParams {
            zones: Arc::new(vec![zone(0, 59, 48.0, low), zone(60, 127, 72.0, high)]),
            gain: 1.0,
        };
        let render = |pitch: u8| {
            let mut v = MultiVoice::start(&p, pitch, 1.0, Articulation::Normal, 48_000.0);
            (0..4_800).map(|_| v.next(&p)).collect::<Vec<f32>>()
        };
        let crossings = |v: &[f32]| v.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        // root で弾けば等速: 低ゾーン 110Hz、高ゾーン 880Hz
        let lo = crossings(&render(48));
        let hi = crossings(&render(72));
        assert!(
            hi as f32 / lo as f32 > 6.0,
            "音域でゾーンが切り替わるはず: lo={lo} hi={hi}"
        );
    }

    #[test]
    fn loop_sustains_long_notes() {
        // 0.1 秒しかないサンプルでも、ループがあれば 1 秒後も鳴っている
        let data = sine_data(220.0, 0.1, 48_000.0);
        let mut looped = zone(0, 127, 57.0, data.clone());
        looped.loop_range = Some((480.0, 4_320.0));
        let p_loop = MultiSamplerParams {
            zones: Arc::new(vec![looped]),
            gain: 1.0,
        };
        let p_once = MultiSamplerParams {
            zones: Arc::new(vec![zone(0, 127, 57.0, data)]),
            gain: 1.0,
        };
        let late = |p: &MultiSamplerParams| {
            let mut v = MultiVoice::start(p, 57, 1.0, Articulation::Normal, 48_000.0);
            let out: Vec<f32> = (0..48_000).map(|_| v.next(p)).collect();
            rms(&out[43_200..])
        };
        assert!(late(&p_loop) > 0.1, "ループで持続するはず");
        assert!(late(&p_once) < 1e-3, "ループなしは読み切って無音のはず");
    }

    #[test]
    fn release_fades_after_note_off() {
        let data = sine_data(220.0, 0.1, 48_000.0);
        let mut z = zone(0, 127, 57.0, data);
        z.loop_range = Some((480.0, 4_320.0));
        let p = MultiSamplerParams {
            zones: Arc::new(vec![z]),
            gain: 1.0,
        };
        let mut v = MultiVoice::start(&p, 57, 1.0, Articulation::Normal, 48_000.0);
        for _ in 0..9_600 {
            v.next(&p);
        }
        v.note_off();
        let mut stopped = false;
        for _ in 0..48_000 {
            v.next(&p);
            if v.finished() {
                stopped = true;
                break;
            }
        }
        assert!(stopped, "リリースで消えるはず");
    }

    /// 明るさの指標: 隣接差分 RMS / RMS(音量に依らない高域の割合)
    fn brightness(v: &[f32]) -> f32 {
        let dd: f32 = v.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
        let ss: f32 = v.iter().map(|s| s * s).sum();
        (dd / ss.max(1e-12)).sqrt()
    }

    /// 倍音を含む波形(矩形波)のサンプル
    fn square_data(freq: f32, secs: f32, sr: f32) -> Arc<SampleData> {
        let frames = (0..(sr * secs) as usize)
            .map(|i| {
                if ((i as f32 * freq / sr).fract()) < 0.5 {
                    0.5
                } else {
                    -0.5
                }
            })
            .collect();
        Arc::new(SampleData {
            frames,
            sample_rate: sr,
            side: None,
        })
    }

    #[test]
    fn lowpass_filter_darkens_tone() {
        let data = square_data(220.0, 0.5, 48_000.0);
        let mut dark = zone(0, 127, 57.0, data.clone());
        dark.modu.cutoff_hz = 300.0;
        let render = |z: Zone| {
            let p = MultiSamplerParams {
                zones: Arc::new(vec![z]),
                gain: 1.0,
            };
            let mut v = MultiVoice::start(&p, 57, 1.0, Articulation::Normal, 48_000.0);
            (0..9_600).map(|_| v.next(&p)).collect::<Vec<f32>>()
        };
        let open = brightness(&render(zone(0, 127, 57.0, data)));
        let closed = brightness(&render(dark));
        assert!(
            closed < open * 0.4,
            "カットオフ 300Hz で高域が落ちるはず: open={open} closed={closed}"
        );
    }

    #[test]
    fn vibrato_lfo_modulates_pitch() {
        let data = sine_data(220.0, 1.0, 48_000.0);
        let mut z = zone(0, 127, 57.0, data);
        z.modu.vib_to_pitch = 100.0; // ±半音
        z.modu.vib_freq = 6.0;
        z.modu.vib_delay = 0.0;
        let p = MultiSamplerParams {
            zones: Arc::new(vec![z]),
            gain: 1.0,
        };
        let mut v = MultiVoice::start(&p, 57, 1.0, Articulation::Normal, 48_000.0);
        let out: Vec<f32> = (0..48_000).map(|_| v.next(&p)).collect();
        // ゼロクロス間隔(周期)の最小と最大が半音ぶん(約 6%)以上開く
        let crossings: Vec<usize> = out
            .windows(2)
            .enumerate()
            .filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0)
            .map(|(i, _)| i)
            .collect();
        let periods: Vec<usize> = crossings.windows(2).map(|w| w[1] - w[0]).collect();
        let (mn, mx) = periods[10..]
            .iter()
            .fold((usize::MAX, 0), |(a, b), &x| (a.min(x), b.max(x)));
        assert!(
            mx as f32 / mn as f32 > 1.06,
            "ビブラートで周期が揺れるはず: min={mn} max={mx}"
        );
    }

    #[test]
    fn mod_envelope_opens_filter_then_closes() {
        let data = square_data(220.0, 1.0, 48_000.0);
        let mut z = zone(0, 127, 57.0, data);
        z.modu.cutoff_hz = 300.0;
        z.modu.env_to_filter = 4800.0; // +4 オクターブ開く
        z.modu.env_attack = 0.005;
        z.modu.env_decay = 0.15;
        z.modu.env_sustain = 0.0;
        let p = MultiSamplerParams {
            zones: Arc::new(vec![z]),
            gain: 1.0,
        };
        let mut v = MultiVoice::start(&p, 57, 1.0, Articulation::Normal, 48_000.0);
        let out: Vec<f32> = (0..48_000).map(|_| v.next(&p)).collect();
        let early = brightness(&out[480..4_800]); // 10〜100ms: 開いている
        let late = brightness(&out[38_400..48_000]); // 800ms〜: 閉じた
        assert!(
            early > late * 2.0,
            "エンベロープで開いてから閉じるはず: early={early} late={late}"
        );
    }

    #[test]
    fn layered_zones_play_together() {
        // 同じ音域に 2 ゾーン(ステレオペア相当)→ 両方鳴る
        let a = sine_data(220.0, 0.3, 48_000.0);
        let b = sine_data(220.0, 0.3, 48_000.0);
        let p2 = MultiSamplerParams {
            zones: Arc::new(vec![zone(0, 127, 57.0, a.clone()), zone(0, 127, 57.0, b)]),
            gain: 1.0,
        };
        let p1 = MultiSamplerParams {
            zones: Arc::new(vec![zone(0, 127, 57.0, a)]),
            gain: 1.0,
        };
        let level = |p: &MultiSamplerParams| {
            let mut v = MultiVoice::start(p, 57, 1.0, Articulation::Normal, 48_000.0);
            let out: Vec<f32> = (0..4_800).map(|_| v.next(p)).collect();
            rms(&out)
        };
        // 等パワー正規化(1/√n)なので、同位相 2 枚は √2 倍まで
        let r = level(&p2) / level(&p1);
        assert!(
            (1.2..1.6).contains(&r),
            "2 レイヤーは √2 倍程度になるはず: {r}"
        );
    }
}
