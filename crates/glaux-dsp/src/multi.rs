//! マルチサンプラー音源 `sf2`(SoundFont のゾーンを再生する)。
//!
//! 「音域 × ベロシティごとに別サンプル + ループ点 + 音量エンベロープ」という
//! SoundFont の中身を [`Zone`] の列に落として再生する。ゾーンの解決
//! (.sf2 のパースとプリセット/インストゥルメントの合成)はエンジン側
//! (`glaux-engine/src/sf2.rs`)が行い、dsp は出来上がったゾーン列を鳴らすだけ。
//!
//! SF2 のモジュレータ・フィルタ・LFO は第 1 段では省略(音量エンベロープと
//! ループがあれば GM 音源の実用度は十分高い)。

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
}

#[derive(Clone, Copy, Debug)]
pub struct MultiVoice {
    players: [ZonePlayer; MAX_LAYERS],
    /// レイヤー数に応じた等パワー正規化
    layer_norm: f32,
    amp: f32,
    released: bool,
    expr: PitchExpr,
    sample_rate: f32,
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
        for (i, z) in p.zones.iter().enumerate() {
            if n >= MAX_LAYERS {
                break;
            }
            if !z.contains(pitch, vel_midi) {
                continue;
            }
            let semis = pitch as f64 - z.root as f64;
            players[n] = ZonePlayer {
                active: true,
                zone: i as u16,
                pos: 0.0,
                rate: (z.data.sample_rate as f64 / sample_rate as f64)
                    * (2.0_f64).powf(semis / 12.0),
                env: 0.0,
                stage: STAGE_ATTACK,
                hold_left: z.env.hold * sample_rate,
            };
            n += 1;
        }
        MultiVoice {
            players,
            layer_norm: 1.0 / (n.max(1) as f32).sqrt(),
            amp: vel * amp_mul,
            released: false,
            expr: PitchExpr::new(articulation, sample_rate),
            sample_rate,
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
            let s = frames[i] + (frames[i + 1] - frames[i]) * frac;
            pl.pos += pl.rate * ratio;

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
