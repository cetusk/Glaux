//! 単一サンプル再生の音源 `sampler`(ワンショット)。
//!
//! 1 つの WAV をトラックの音源にし、`root`(サンプル自身の音程)からの
//! ピッチ差を再生レートに変換して鳴らす。実録の質感(本物のギター、
//! ボーカルチョップ、生ドラムのワンショット等)はシンセでは出ないのでこれを使う。
//!
//! - 波形データは `Arc<SampleData>` で共有(構築時に読み込み済み。
//!   オーディオスレッドは読むだけでアロケーションしない)
//! - ループ(開始・終わり・クロスフェード)、音量の ADSR、フィルタ(共通部品 [`crate::tone`] の SVF)、
//!   ステレオ再生(左右差成分 `side`)、キー追従のオン/オフ、スライス(音の頭で切って鍵盤に並べる)。
//!   どれも既定値では従来のワンショットと同じ音
//! - テンポ追従(元のテンポ → 曲のテンポ)の伸縮はエンジン側で済ませた波形を受け取る
//! - 複数サンプルのマッピング(音域ごとの切り替え)は SFZ / SoundFont([`crate::multi`])

use crate::expr::PitchExpr;
use glaux_core::Articulation;
use std::sync::Arc;

/// 読み込み済みのサンプル波形(モノラルに合算済み)。
#[derive(Debug)]
pub struct SampleData {
    /// モノラル成分(ステレオ素材なら左右の平均 M = (L + R) / 2)。解析・譜起こし・波形表示はこれを使う
    pub frames: Vec<f32>,
    pub sample_rate: f32,
    /// ステレオ素材の左右差成分 S = (L − R) / 2(L = M + S、R = M − S)。モノラル素材は None
    pub side: Option<Vec<f32>>,
    /// 音程を上げて鳴らすときの、帯域を制限した縮小版(レベル k は長さ 1/2^k・帯域 1/2^k)。
    /// サンプラー・SoundFont で使うときにだけ作る([`SampleData::prepare_mips`]。オーディオスレッドの外で)
    pub mips: std::sync::OnceLock<Mips>,
}

/// 縮小版(中央成分と、ステレオ素材なら左右差成分)
#[derive(Debug, Default)]
pub struct Mips {
    mid: Vec<Vec<f32>>,
    side: Vec<Vec<f32>>,
}

/// 縮小版の段数(2 オクターブ × 2 = 16 倍の速さまで)
const MIP_LEVELS: usize = 4;
/// 半分にするときのローパス(窓付き sinc)の片側のタップ数
const MIP_HALF: usize = 16;

/// 帯域を半分にしてから 1 つおきに間引く(窓付き sinc、Blackman 窓。遅れは中央合わせで 0)
pub(crate) fn halve(x: &[f32]) -> Vec<f32> {
    let taps: Vec<f32> = (0..=2 * MIP_HALF)
        .map(|k| {
            let n = k as f32 - MIP_HALF as f32;
            // 遮断は元のナイキストの 0.45 倍(= 間引いた後のナイキストの 0.9 倍)
            let fc = 0.225f32;
            let sinc = if n == 0.0 {
                2.0 * fc
            } else {
                (std::f32::consts::TAU * fc * n).sin() / (std::f32::consts::PI * n)
            };
            let t = k as f32 / (2 * MIP_HALF) as f32;
            let w = 0.42 - 0.5 * (std::f32::consts::TAU * t).cos()
                + 0.08 * (2.0 * std::f32::consts::TAU * t).cos();
            sinc * w
        })
        .collect();
    // 係数は先に正規化しておく(1 点ごとの割り算を省く)
    let sum: f32 = taps.iter().sum();
    let taps: Vec<f32> = taps.iter().map(|t| t / sum).collect();
    let at = |i: isize| -> f32 {
        if i < 0 || i as usize >= x.len() {
            0.0
        } else {
            x[i as usize]
        }
    };
    (0..x.len().div_ceil(2))
        .map(|n| {
            let c = 2 * n;
            if c >= MIP_HALF && c + MIP_HALF < x.len() {
                // 内側: 範囲チェックなしのスライスの内積(自動でベクトル化される)
                x[c - MIP_HALF..=c + MIP_HALF]
                    .iter()
                    .zip(&taps)
                    .map(|(v, t)| v * t)
                    .sum::<f32>()
            } else {
                taps.iter()
                    .enumerate()
                    .map(|(k, t)| t * at(c as isize + k as isize - MIP_HALF as isize))
                    .sum::<f32>()
            }
        })
        .collect()
}

impl SampleData {
    /// モノラルの波形。
    pub fn mono(frames: Vec<f32>, sample_rate: f32) -> Self {
        SampleData {
            frames,
            sample_rate,
            side: None,
            mips: Default::default(),
        }
    }

    /// 左右の波形から(同じ長さであること)。
    pub fn stereo(left: &[f32], right: &[f32], sample_rate: f32) -> Self {
        let frames = left.iter().zip(right).map(|(l, r)| (l + r) * 0.5).collect();
        let side = left.iter().zip(right).map(|(l, r)| (l - r) * 0.5).collect();
        SampleData {
            frames,
            sample_rate,
            side: Some(side),
            mips: Default::default(),
        }
    }

    /// 音程を上げて鳴らすための縮小版を作っておく(1 回だけ。オーディオスレッドの外で呼ぶ)
    pub fn prepare_mips(&self) {
        let build = |x: &[f32]| {
            let mut out: Vec<Vec<f32>> = Vec::with_capacity(MIP_LEVELS);
            let mut cur = halve(x);
            for _ in 0..MIP_LEVELS {
                let next = halve(&cur);
                out.push(std::mem::replace(&mut cur, next));
                if out.last().is_some_and(|v| v.len() < 4) {
                    break;
                }
            }
            out
        };
        self.mips.get_or_init(|| Mips {
            mid: build(&self.frames),
            side: self.side.as_deref().map(build).unwrap_or_default(),
        });
    }

    /// 位置 `pos`(元のサンプルの番号、小数)の値を、1 サンプルあたり `step` 進む速さで鳴らすときの帯域で読む。
    /// `step` が 1 を超える(元より高く鳴らす)と、ナイキストを超えて折り返す倍音を縮小版で落とす。
    /// 段の間は速さの比で混ぜる(少し上げただけで急にこもらないように)。縮小版が無ければ元のまま読む。
    /// 呼び出し側が `pos as usize + 1 < frames.len()` を保証する
    #[inline]
    pub fn read(&self, pos: f64, step: f64) -> f32 {
        self.read_cached(pos, step, &mut (f64::NAN, 0.0))
    }

    /// [`Self::read`] の、段の計算(`step` の対数)を `level` に覚えておく版。`step` が前と同じなら
    /// 対数を取らない(ボイスごとに `level` を持ち、1 サンプルごとに呼ぶ)
    #[inline]
    pub fn read_cached(&self, pos: f64, step: f64, level: &mut (f64, f64)) -> f32 {
        let mips = self.mips.get().map_or(&[][..], |m| &m.mid[..]);
        if mips.is_empty() || step <= 1.0 {
            return read_band_limited(&self.frames, mips, pos, 0.0);
        }
        if level.0 != step {
            *level = (step, step.log2());
        }
        read_band_limited(&self.frames, mips, pos, level.1)
    }

    /// 段(速さの 2 を底とする対数 `lg`。0 以下は元のまま)を先に求めてある版。
    /// 速さが一定の読み出し(グラニュラーの粒)で、1 サンプルごとの対数を省く
    #[inline]
    pub fn read_at_level(&self, pos: f64, lg: f64) -> f32 {
        let mips = self.mips.get().map_or(&[][..], |m| &m.mid[..]);
        read_band_limited(&self.frames, mips, pos, lg)
    }

    /// [`Self::read_cached`] の左右差成分版(モノラル素材は 0)
    #[inline]
    pub fn read_side_cached(&self, pos: f64, step: f64, level: &mut (f64, f64)) -> f32 {
        let Some(side) = &self.side else {
            return 0.0;
        };
        let mips = self.mips.get().map_or(&[][..], |m| &m.side[..]);
        if mips.is_empty() || step <= 1.0 {
            return read_band_limited(side, mips, pos, 0.0);
        }
        if level.0 != step {
            *level = (step, step.log2());
        }
        read_band_limited(side, mips, pos, level.1)
    }

    /// 左右の波形(モノラルなら同じもの)。
    pub fn left_right(&self) -> (Vec<f32>, Vec<f32>) {
        match &self.side {
            Some(side) => (
                self.frames.iter().zip(side).map(|(m, s)| m + s).collect(),
                self.frames.iter().zip(side).map(|(m, s)| m - s).collect(),
            ),
            None => (self.frames.clone(), self.frames.clone()),
        }
    }
}

/// [`SampleData::read_cached`] の中身(1 チャンネル分)。`lg` は速さの 2 を底とする対数(0 以下は元のまま)
#[inline]
fn read_band_limited(frames: &[f32], mips: &[Vec<f32>], pos: f64, lg: f64) -> f32 {
    let base = |pos: f64| {
        let i = pos as usize;
        hermite(frames, i, (pos - i as f64) as f32)
    };
    if mips.is_empty() || lg <= 0.0 {
        return base(pos);
    }
    let level = lg.min(mips.len() as f64);
    let lo = level.floor() as usize;
    let t = (level - lo as f64) as f32;
    let at = |k: usize| -> f32 {
        if k == 0 {
            return base(pos);
        }
        let m = &mips[k - 1];
        let p = pos / (1u64 << k) as f64;
        let i = (p as usize).min(m.len().saturating_sub(2));
        hermite(m, i, ((p - i as f64) as f32).clamp(0.0, 1.0))
    };
    if t <= 0.0 || lo >= mips.len() {
        at(lo.min(mips.len()))
    } else {
        at(lo) * (1.0 - t) + at(lo + 1) * t
    }
}

/// 焼き込み済みパラメータ(1 トラック分)。既定値(下の各つまみの既定)では従来のワンショットと同じ音
#[derive(Clone, Debug)]
pub struct SamplerParams {
    pub data: Arc<SampleData>,
    /// サンプル自身の音程(MIDI ノート番号)。この音で等速再生になる
    pub root: u8,
    /// リニアゲイン(dB から変換済み)
    pub gain: f32,
    /// note_off 後のフェード係数(1 サンプルあたり)
    pub release_coef: f32,
    /// 立ち上がり(秒。既定 0.002 = 従来の頭のデクリック)
    pub attack: f32,
    /// 減衰(秒。sustain まで下がるおおよその時間)
    pub decay: f32,
    /// 減衰後の高さ 0..=1(1 = 減衰しない = 従来)
    pub sustain: f32,
    /// ループするか
    pub looping: bool,
    /// ループの開始・終わり(波形の長さに対する割合 0..=1)
    pub loop_start: f32,
    pub loop_end: f32,
    /// ループのつなぎ目のクロスフェード(秒)
    pub loop_xfade: f32,
    /// フィルタを掛けるか(false = 素通し = 従来)
    pub filter_on: bool,
    pub cutoff: f32,
    pub resonance: f32,
    /// 音量の包絡でカットオフを開く量 0..=1
    pub filter_env: f32,
    /// フィルタの種類・ベロシティ追従(共通部品のつまみ。ほかは既定のまま)
    pub tone: crate::tone::ToneParams,
    /// ステレオ素材を左右のまま鳴らすか(false = 中央に合算 = 従来)
    pub stereo: bool,
    /// 鍵盤の高さに音程が付いていくか(false = どの鍵盤でも元の高さ。ドラムのワンショット・効果音)
    pub key_track: bool,
    /// スライスの頭の位置(サンプル番号、昇順、先頭は使う所の始まり)。空ならスライスしない。
    /// root の鍵盤から順に 1 つずつ割り当て、元の高さで頭から次の頭(最後は使う所の終わり)まで鳴らす
    pub slices: Arc<[u32]>,
    /// 使う所(波形の長さに対する割合 0..=1)。この外は鳴らさない(既定 0〜1 = 全部)
    pub start: f32,
    pub end: f32,
}

impl SamplerParams {
    /// 波形・root・音量・リリース以外は既定値(従来のワンショット)
    pub fn one_shot(data: Arc<SampleData>, root: u8, gain: f32, release_coef: f32) -> Self {
        SamplerParams {
            data,
            root,
            gain,
            release_coef,
            attack: 0.002,
            decay: 1.0,
            sustain: 1.0,
            looping: false,
            loop_start: 0.0,
            loop_end: 1.0,
            loop_xfade: 0.01,
            filter_on: false,
            cutoff: 20_000.0,
            resonance: 0.0,
            filter_env: 0.0,
            tone: crate::tone::ToneParams::default(),
            stereo: false,
            key_track: true,
            slices: Arc::from(Vec::new()),
            start: 0.0,
            end: 1.0,
        }
    }
}

impl PartialEq for SamplerParams {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.data, &other.data)
            && self.root == other.root
            && self.gain == other.gain
            && self.release_coef == other.release_coef
            && self.attack == other.attack
            && self.decay == other.decay
            && self.sustain == other.sustain
            && self.looping == other.looping
            && self.loop_start == other.loop_start
            && self.loop_end == other.loop_end
            && self.loop_xfade == other.loop_xfade
            && self.filter_on == other.filter_on
            && self.cutoff == other.cutoff
            && self.resonance == other.resonance
            && self.filter_env == other.filter_env
            && self.tone == other.tone
            && self.stereo == other.stereo
            && self.key_track == other.key_track
            && self.slices == other.slices
            && self.start == other.start
            && self.end == other.end
    }
}

/// スライスの終わりのフェード(秒。次の頭の手前で切ってもプツッといわない)
const SLICE_FADE: f32 = 0.003;
/// フィルタの係数を更新する間隔(サンプル)
const CTRL_RATE: u32 = 32;
/// ループの最短の長さ(サンプル)
const MIN_LOOP: f64 = 16.0;

#[derive(Clone, Copy, Debug)]
pub struct SamplerVoice {
    /// 再生位置(サンプル、波形のネイティブレート基準)
    pos: f64,
    /// 1 出力サンプルあたりの進み(ピッチ + レート変換込み)
    rate: f64,
    amp: f32,
    /// 立ち上がり〜減衰の包絡(従来は頭のデクリック 2ms だけ)
    attack_env: f32,
    attack_inc: f32,
    /// 立ち上がりを終えた
    decaying: bool,
    release_env: f32,
    released: bool,
    /// 波形を最後まで読み切った
    done: bool,
    /// 鳴らす範囲の終わり(スライスの終わり。スライスしないなら波形の終わり)
    end: f64,
    /// スライスで鳴らしている
    sliced: bool,
    pub(crate) expr: PitchExpr,
    sample_rate: f32,
    /// 縮小版の段の計算の覚え([`SampleData::read_cached`])。中央と左右差
    level: (f64, f64),
    level_side: (f64, f64),
    /// フィルタ(共通部品)の状態と係数
    tone: crate::tone::ToneVoice,
    coefs: crate::tone::SvfCoefs,
    ctrl: u32,
    vel: f32,
    /// 鍵盤の高さ(Hz。フィルタのキー追従用)
    freq: f32,
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
        let len = p.data.frames.len() as f64;
        // 使う所(始まり・終わり)。区分のときも、最後の区分は使う所の終わりまで
        let use_end = (p.end.clamp(0.0, 1.0) as f64 * len).clamp(1.0, len);
        let mut pos = (p.start.clamp(0.0, 1.0) as f64 * len)
            .min(use_end - 1.0)
            .max(0.0);
        let mut end = use_end;
        let mut sliced = false;
        let mut done = false;
        let semis = if !p.slices.is_empty() {
            // スライス: root から順に 1 つずつ。範囲外の鍵盤は鳴らさない
            sliced = true;
            match (pitch as usize)
                .checked_sub(p.root as usize)
                .filter(|k| *k < p.slices.len())
            {
                Some(k) => {
                    pos = p.slices[k] as f64;
                    end = p.slices.get(k + 1).map_or(use_end, |e| *e as f64);
                }
                None => done = true,
            }
            0.0
        } else if p.key_track {
            pitch as f64 - p.root as f64
        } else {
            0.0
        };
        let rate = (p.data.sample_rate as f64 / sample_rate as f64) * (2.0_f64).powf(semis / 12.0);
        SamplerVoice {
            pos,
            rate,
            amp: vel * amp_mul,
            attack_env: 0.0,
            attack_inc: 1.0 / (p.attack.max(0.0005) * sample_rate),
            decaying: false,
            release_env: 1.0,
            released: false,
            done,
            end,
            sliced,
            expr: PitchExpr::new(articulation, sample_rate),
            sample_rate,
            level: (f64::NAN, 0.0),
            level_side: (f64::NAN, 0.0),
            tone: crate::tone::ToneVoice::new(pitch as u32),
            coefs: crate::tone::SvfCoefs::default(),
            ctrl: 0,
            vel,
            freq: 440.0 * 2.0f32.powf((pitch as f32 - 69.0) / 12.0),
        }
    }

    pub fn note_off(&mut self) {
        self.released = true;
    }

    pub fn finished(&self) -> bool {
        self.done
    }

    /// 中央の成分だけ(ステレオで鳴らしていなければ全体)
    pub fn next(&mut self, p: &SamplerParams) -> f32 {
        self.next_stereo(p).0
    }

    /// ループの範囲(サンプル番号)と、つなぎ目のクロスフェードの長さ。ループしないなら None
    fn loop_range(&self, p: &SamplerParams) -> Option<(f64, f64, f64)> {
        if !p.looping || self.sliced {
            return None;
        }
        let n = p.data.frames.len() as f64;
        let ls = (p.loop_start.clamp(0.0, 1.0) as f64 * n).floor();
        let le = (p.loop_end.clamp(0.0, 1.0) as f64 * n).floor().min(n - 2.0);
        if le - ls < MIN_LOOP {
            return None;
        }
        // つなぎ目の手前で、ループの頭の手前の素材へ移していく(頭の手前に素材が無い分だけ短くする)
        let xf = (p.loop_xfade.max(0.0) as f64 * p.data.sample_rate as f64)
            .min((le - ls) * 0.5)
            .min(ls);
        Some((ls, le, xf))
    }

    /// (中央, 左右の差)。L = 中央 + 差、R = 中央 − 差。ステレオで鳴らさないなら差は 0
    pub fn next_stereo(&mut self, p: &SamplerParams) -> (f32, f32) {
        if self.done {
            return (0.0, 0.0);
        }
        let frames = &p.data.frames;
        let lp = self.loop_range(p);
        if let Some((ls, le, _)) = lp {
            if self.pos >= le {
                self.pos = ls + (self.pos - le) % (le - ls);
            }
        }
        let i = self.pos as usize;
        // 区分の終わり・使う所の終わり(ループしている間はループが決める)で止める
        if i + 1 >= frames.len() || ((self.sliced || lp.is_none()) && self.pos >= self.end) {
            self.done = true;
            return (0.0, 0.0);
        }
        // ピッチ表現(ビブラート / チョーキング)はレートに掛ける
        let ratio = if self.expr.is_active() {
            self.expr.next_ratio(self.sample_rate) as f64
        } else {
            1.0
        };
        let step = self.rate * ratio;
        let stereo = p.stereo && p.data.side.is_some();
        // 高く鳴らすときは帯域を制限した縮小版から読む(折り返し雑音を出さない)
        let mut s = p.data.read_cached(self.pos, step, &mut self.level);
        let mut side = if stereo {
            p.data
                .read_side_cached(self.pos, step, &mut self.level_side)
        } else {
            0.0
        };
        // ループのつなぎ目: 終わりの手前 xf の間に、ループの頭の手前の同じ所へ等パワーで移していく
        if let Some((ls, le, xf)) = lp {
            let from = le - xf;
            if xf > 0.0 && self.pos >= from {
                let t = ((self.pos - from) / xf) as f32;
                let (a, b) = (
                    (t * std::f32::consts::FRAC_PI_2).cos(),
                    (t * std::f32::consts::FRAC_PI_2).sin(),
                );
                let other = self.pos - (le - ls);
                let mut lv = (f64::NAN, 0.0);
                s = s * a + p.data.read_cached(other, step, &mut lv) * b;
                if stereo {
                    side = side * a + p.data.read_side_cached(other, step, &mut lv) * b;
                }
            }
        }
        self.pos += step;

        // ---- 音量の包絡(立ち上がり → 減衰 → 持続、ノートオフで従来のフェード) ----
        if !self.decaying {
            self.attack_env = (self.attack_env + self.attack_inc).min(1.0);
            if self.attack_env >= 1.0 {
                self.decaying = true;
            }
        } else if p.sustain < 1.0 {
            let coef = (6.9 / (p.decay.max(0.005) * self.sample_rate)).min(1.0);
            self.attack_env += (p.sustain - self.attack_env) * coef;
        }
        if self.released {
            self.release_env *= 1.0 - p.release_coef;
            if self.release_env < 1e-4 {
                self.done = true;
            }
        }
        // スライス・切った使う所の終わりの手前で短くフェード(素材の終わりまで鳴らすときは従来どおり)
        let trimmed = lp.is_none() && self.end < frames.len() as f64 - 1.0;
        let tail = if self.sliced || trimmed {
            let left = ((self.end - self.pos) / self.rate) as f32;
            (left / (SLICE_FADE * self.sample_rate)).clamp(0.0, 1.0)
        } else {
            1.0
        };

        // ---- フィルタ(掛けるときだけ。係数は制御レートで更新) ----
        if p.filter_on {
            let sr = self.sample_rate;
            if self.ctrl == 0 {
                self.tone.control(&p.tone, CTRL_RATE, sr, self.released);
                self.tone.stereo = stereo;
                let env = self.attack_env * self.release_env;
                let fc = self
                    .tone
                    .cutoff(&p.tone, p.cutoff, p.filter_env, env, self.vel, self.freq)
                    .clamp(20.0, sr * 0.45);
                self.coefs = crate::tone::SvfCoefs::new(fc, p.resonance, sr);
                self.ctrl = CTRL_RATE;
            }
            self.ctrl -= 1;
            let c = self.coefs;
            (s, side) = self.tone.filter(&p.tone, &c, s, side);
        }
        // 掛ける順は従来どおり(既定のつまみで従来とビット単位で同じ音にする)
        let out = s * self.amp * self.attack_env * self.release_env * p.gain;
        let out_side = side * self.amp * self.attack_env * self.release_env * p.gain;
        if tail < 1.0 {
            (out * tail, out_side * tail)
        } else {
            (out, out_side)
        }
    }
}

/// 音の頭(オンセット)を見つけて、最大 `max` 個のスライスの頭(サンプル番号、昇順、先頭は 0)を返す。
/// 短い区間(約 5ms)ごとの音量の対数の立ち上がりが大きい所を、強い順に選ぶ(近すぎる頭は 50ms 空ける)。
/// 頭が見つからなければ先頭 1 つだけ
pub fn detect_slices(frames: &[f32], sample_rate: f32, max: usize) -> Vec<u32> {
    let max = max.max(1);
    let hop = ((sample_rate * 0.005) as usize).max(16);
    let n = frames.len() / hop;
    if n < 3 {
        return vec![0];
    }
    // 区間ごとの音量(dB)
    let db: Vec<f32> = (0..n)
        .map(|k| {
            let w = &frames[k * hop..(k + 1) * hop];
            let e = w.iter().map(|v| v * v).sum::<f32>() / hop as f32;
            10.0 * (e + 1e-10).log10()
        })
        .collect();
    let peak = db.iter().copied().fold(f32::MIN, f32::max);
    // 立ち上がり = 直前 3 区間の最小からの上がり幅(ゆっくり膨らむ音は拾わない)
    let mut cand: Vec<(usize, f32)> = (1..n)
        .filter_map(|k| {
            let before = db[k.saturating_sub(3)..k]
                .iter()
                .copied()
                .fold(f32::MAX, f32::min);
            let rise = db[k] - before;
            // 静かすぎる所(いちばん大きい所から 40dB 下)と、小さな上がりは頭とみなさない
            (rise > 6.0 && db[k] > peak - 40.0).then_some((k, rise))
        })
        .collect();
    // 隣り合う区間の重複は、上がり幅の大きい方だけ残す
    cand.sort_by(|a, b| b.1.total_cmp(&a.1));
    let gap = ((sample_rate * 0.05) as usize / hop).max(1);
    let mut picked: Vec<usize> = Vec::new();
    for (k, _) in cand {
        if picked.len() + 1 >= max {
            break;
        }
        if k < gap || picked.iter().any(|p| p.abs_diff(k) < gap) {
            continue;
        }
        picked.push(k);
    }
    picked.sort_unstable();
    // 頭の位置は、その区間の中で音が急に大きくなる所まで詰める(区間の頭だと少し早い)
    let mut out = vec![0u32];
    for k in picked {
        let from = (k - 1) * hop;
        let to = ((k + 1) * hop).min(frames.len());
        let thr = frames[from..to].iter().fold(0.0f32, |m, v| m.max(v.abs())) * 0.1;
        let at = (from..to)
            .find(|&i| frames[i].abs() >= thr)
            .unwrap_or(k * hop);
        // ゼロ交差の少し手前から始める(頭のクリックを避ける)
        let start = at.saturating_sub(hop / 4);
        out.push(start as u32);
    }
    out.dedup();
    out
}

/// 4 点 3 次 Hermite 補間(Catmull-Rom)。`frames[i]` と `frames[i + 1]` の間の `frac`(0..1)の位置の値。
/// 呼び出し側が `i + 1 < frames.len()` を保証する。前後の点が範囲外なら端の値で代わりにする。
/// 以前は線形補間で、素材とエンジンのサンプルレートが違う(44.1k の素材など)と、高い音域で
/// 折り返し雑音が出て高域もこもった
#[inline]
pub fn hermite(frames: &[f32], i: usize, frac: f32) -> f32 {
    let x0 = frames[i];
    let x1 = frames[i + 1];
    let xm1 = if i > 0 { frames[i - 1] } else { x0 };
    let x2 = frames.get(i + 2).copied().unwrap_or(x1);
    let c1 = 0.5 * (x1 - xm1);
    let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
    ((c3 * frac + c2) * frac + c1) * frac + x0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hermite_hits_the_points_and_is_smoother_than_linear() {
        let f = [0.0f32, 1.0, 0.0, -1.0, 0.0];
        assert_eq!(hermite(&f, 1, 0.0), 1.0);
        assert!((hermite(&f, 1, 1.0) - 0.0).abs() < 1e-6);
        // サイン波の途中の値は線形補間より本来の値に近い
        let sr = 48_000.0f32;
        let w: Vec<f32> = (0..64)
            .map(|k| (k as f32 * 5_000.0 * std::f32::consts::TAU / sr).sin())
            .collect();
        let (mut e_lin, mut e_her) = (0.0f32, 0.0f32);
        for k in 2..60 {
            let t = k as f32 + 0.5;
            let truth = (t * 5_000.0 * std::f32::consts::TAU / sr).sin();
            e_lin += (w[k] + (w[k + 1] - w[k]) * 0.5 - truth).abs();
            e_her += (hermite(&w, k, 0.5) - truth).abs();
        }
        assert!(e_her < e_lin * 0.5, "Hermite {e_her} / 線形 {e_lin}");
    }

    /// 440Hz サイン波 0.5 秒ぶんのサンプル
    fn test_sample(sr: f32) -> Arc<SampleData> {
        let frames = (0..(sr * 0.5) as usize)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / sr).sin() * 0.5)
            .collect();
        Arc::new(SampleData::mono(frames, sr))
    }

    fn params(root: u8) -> SamplerParams {
        SamplerParams::one_shot(test_sample(48_000.0), root, 1.0, 1.0 / (0.03 * 48_000.0))
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32).sqrt()
    }

    /// 倍音(f0 の整数倍)以外の成分の、全体に対する割合(dB)
    fn alias_db(v: &[f32], f0: f32, sr: f32) -> f32 {
        let (mut total, mut alias) = (0.0f64, 0.0f64);
        let bins = 300;
        let len = v.len() as f64;
        for b in 1..bins {
            let f = b as f64 * sr as f64 / 2.0 / bins as f64;
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (k, s) in v.iter().enumerate() {
                let w = 0.5 - 0.5 * (std::f64::consts::TAU * k as f64 / len).cos();
                let ph = k as f64 * f * std::f64::consts::TAU / sr as f64;
                re += *s as f64 * w * ph.cos();
                im += *s as f64 * w * ph.sin();
            }
            let pw = re * re + im * im;
            total += pw;
            let h = f / f0 as f64;
            if (h - h.round()).abs() * f0 as f64 > 150.0 {
                alias += pw;
            }
        }
        (10.0 * (alias / total).log10()) as f32
    }

    #[test]
    fn pitching_up_uses_band_limited_mips() {
        // 倍音をナイキストの手前(20kHz)まで並べたのこぎり波(1234Hz。48kHz の約数にならない高さ)を
        // 2 オクターブ上(4 倍の速さ)で鳴らすと、5kHz より上の倍音が折り返す
        let sr = 48_000.0f32;
        let f = 1234.0f32;
        let saw: Vec<f32> = (0..24_000)
            .map(|i| {
                let t = i as f32 / sr;
                (1..=(20_000.0 / f) as usize)
                    .map(|h| (std::f32::consts::TAU * f * h as f32 * t).sin() / h as f32)
                    .sum::<f32>()
                    * 0.2
            })
            .collect();
        let play = |mips: bool| {
            let data = Arc::new(SampleData::mono(saw.clone(), sr));
            if mips {
                data.prepare_mips();
            }
            let p = SamplerParams::one_shot(data, 60, 1.0, 0.001);
            let mut v = SamplerVoice::start(&p, 84, 1.0, Articulation::Normal, sr);
            (0..4800).map(|_| v.next(&p)).collect::<Vec<f32>>()
        };
        let plain = alias_db(&play(false)[400..], f * 4.0, sr);
        let mipped = alias_db(&play(true)[400..], f * 4.0, sr);
        assert!(
            mipped < plain - 10.0,
            "縮小版 {mipped:.1} dB / そのまま {plain:.1} dB"
        );
        // 元の高さ以下では縮小版を使わない(同じ音)
        let data = Arc::new(SampleData::mono(saw.clone(), sr));
        data.prepare_mips();
        for k in 0..100 {
            let pos = k as f64 * 3.3;
            assert_eq!(
                data.read(pos, 1.0),
                hermite(&saw, pos as usize, (pos - pos.floor()) as f32)
            );
        }
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

    /// 従来(ループ・ADSR・フィルタを足す前)のワンショットの計算をそのまま写したもの
    fn legacy(p: &SamplerParams, pitch: u8, vel: f32, off_at: usize, n: usize) -> Vec<f32> {
        let sr = 48_000.0f32;
        let semis = pitch as f64 - p.root as f64;
        let rate = (p.data.sample_rate as f64 / sr as f64) * (2.0_f64).powf(semis / 12.0);
        let (mut pos, mut att, mut rel, mut done) = (0.0f64, 0.0f32, 1.0f32, false);
        let inc = 1.0 / (0.002 * sr);
        let mut level = (f64::NAN, 0.0);
        (0..n)
            .map(|k| {
                if done {
                    return 0.0;
                }
                if pos as usize + 1 >= p.data.frames.len() {
                    done = true;
                    return 0.0;
                }
                let s = p.data.read_cached(pos, rate, &mut level);
                pos += rate;
                if att < 1.0 {
                    att = (att + inc).min(1.0);
                }
                if k >= off_at {
                    rel *= 1.0 - p.release_coef;
                    if rel < 1e-4 {
                        done = true;
                    }
                }
                s * vel * att * rel * p.gain
            })
            .collect()
    }

    #[test]
    fn default_knobs_sound_exactly_like_the_old_one_shot() {
        // 既定のつまみ(焼き込み経由)で、従来の計算とビット単位で同じ
        let data = test_sample(48_000.0);
        let p = crate::params::bake_sampler(&glaux_core::ParamMap::new(), data, 48_000.0);
        for pitch in [55u8, 60, 67, 79] {
            let mut v = SamplerVoice::start(&p, pitch, 0.8, Articulation::Normal, 48_000.0);
            let new: Vec<(f32, f32)> = (0..30_000)
                .map(|k| {
                    if k == 9_000 {
                        v.note_off();
                    }
                    v.next_stereo(&p)
                })
                .collect();
            let old = legacy(&p, pitch, 0.8, 9_000, 30_000);
            for (k, ((m, s), o)) in new.iter().zip(&old).enumerate() {
                assert_eq!(m.to_bits(), o.to_bits(), "pitch {pitch} の {k} サンプル目");
                assert_eq!(*s, 0.0);
            }
        }
    }

    #[test]
    fn loop_sustains_while_held_and_releases() {
        let mut p = params(60);
        p.looping = true;
        p.loop_start = 0.3;
        p.loop_end = 0.8;
        let mut v = SamplerVoice::start(&p, 60, 1.0, Articulation::Normal, 48_000.0);
        // 0.5 秒のサンプルでも 2 秒押さえれば鳴り続ける
        let held: Vec<f32> = (0..96_000).map(|_| v.next(&p)).collect();
        assert!(!v.finished());
        assert!(rms(&held[72_000..96_000]) > 0.2, "{}", rms(&held[72_000..]));
        v.note_off();
        for _ in 0..48_000 {
            v.next(&p);
        }
        assert!(v.finished(), "離せばリリースで消える");
    }

    #[test]
    fn loop_crossfade_smooths_the_seam() {
        // 周期の合わないループ(441Hz を 0.3〜0.7 で)。つなぎ目の段差はクロスフェードで小さくなる
        let sr = 48_000.0f32;
        let frames: Vec<f32> = (0..24_000)
            .map(|i| (i as f32 * 441.7 * std::f32::consts::TAU / sr).sin() * 0.5)
            .collect();
        let jump = |xf: f32| {
            let mut p = SamplerParams::one_shot(
                Arc::new(SampleData::mono(frames.clone(), sr)),
                60,
                1.0,
                0.001,
            );
            p.looping = true;
            p.loop_start = 0.3;
            p.loop_end = 0.714_29; // 長さ ≈ 91.5 周期(つなぎ目で位相が逆になる)
            p.loop_xfade = xf;
            let mut v = SamplerVoice::start(&p, 60, 1.0, Articulation::Normal, sr);
            let out: Vec<f32> = (0..60_000).map(|_| v.next(&p)).collect();
            // 隣り合うサンプルの差の最大(正弦波そのものの最大の傾きは 2π·441.7/48000·0.5 ≈ 0.029)
            out[1000..]
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0f32, f32::max)
        };
        let hard = jump(0.0);
        let smooth = jump(0.02);
        assert!(hard > 0.05, "つなぎ目で段差が出るはず: {hard}");
        assert!(smooth < 0.035, "クロスフェードでなめらか: {smooth}");
    }

    #[test]
    fn decay_and_sustain_shape_the_volume() {
        let mut p = params(60);
        p.sustain = 0.0;
        p.decay = 0.05;
        let mut v = SamplerVoice::start(&p, 60, 1.0, Articulation::Normal, 48_000.0);
        let x: Vec<f32> = (0..24_000).map(|_| v.next(&p)).collect();
        assert!(rms(&x[0..960]) > 0.1);
        assert!(rms(&x[14_400..19_200]) < 1e-3, "ディケイの後は消える");
        // 長いアタックはゆっくり立ち上がる
        let mut p = params(60);
        p.attack = 0.2;
        let mut v = SamplerVoice::start(&p, 60, 1.0, Articulation::Normal, 48_000.0);
        let x: Vec<f32> = (0..19_200).map(|_| v.next(&p)).collect();
        assert!(rms(&x[0..960]) < rms(&x[9_600..19_200]) * 0.3);
    }

    /// 白色雑音(決まった種)
    fn noise(n: usize) -> Vec<f32> {
        let mut r = 12345u32;
        (0..n)
            .map(|_| {
                r ^= r << 13;
                r ^= r >> 17;
                r ^= r << 5;
                (r as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect()
    }

    #[test]
    fn filter_darkens_and_follows_velocity() {
        let data = Arc::new(SampleData::mono(noise(24_000), 48_000.0));
        let run = |filter: Option<crate::tone::FilterType>, cutoff: f32, vel: f32, vc: f32| {
            let mut p = SamplerParams::one_shot(data.clone(), 60, 1.0, 0.001);
            if let Some(t) = filter {
                p.filter_on = true;
                p.tone.filter_type = t;
                p.cutoff = cutoff;
                p.tone.vel_cutoff = vc;
            }
            let mut v = SamplerVoice::start(&p, 60, vel, Articulation::Normal, 48_000.0);
            let x: Vec<f32> = (0..20_000).map(|_| v.next(&p)).collect();
            // 音量はベロシティで割り戻して比べる(明るさだけを見る)
            rms(&x[2_000..]) / vel
        };
        use crate::tone::FilterType as F;
        let dry = run(None, 0.0, 1.0, 0.0);
        let lp = run(Some(F::Lp12), 500.0, 1.0, 0.0);
        let hp = run(Some(F::Hp), 8_000.0, 1.0, 0.0);
        assert!(lp < dry * 0.3, "ローパスで暗く: {lp} / {dry}");
        assert!(hp < dry * 0.8 && hp > lp, "ハイパス: {hp}");
        let loud = run(Some(F::Lp12), 4_000.0, 1.0, 1.0);
        let soft = run(Some(F::Lp12), 4_000.0, 0.2, 1.0);
        assert!(soft < loud * 0.7, "弱い音ほど暗い: {soft} / {loud}");
    }

    #[test]
    fn stereo_plays_left_and_right() {
        // 左だけに音がある素材
        let sr = 48_000.0;
        let l: Vec<f32> = (0..12_000)
            .map(|i| (i as f32 * 300.0 * std::f32::consts::TAU / sr).sin() * 0.5)
            .collect();
        let r = vec![0.0f32; 12_000];
        let data = Arc::new(SampleData::stereo(&l, &r, sr));
        let mut p = SamplerParams::one_shot(data, 60, 1.0, 0.001);
        let render = |p: &SamplerParams| {
            let mut v = SamplerVoice::start(p, 60, 1.0, Articulation::Normal, sr);
            (0..10_000).map(|_| v.next_stereo(p)).collect::<Vec<_>>()
        };
        let mono = render(&p);
        assert!(mono.iter().all(|(_, s)| *s == 0.0), "既定は中央に合算");
        p.stereo = true;
        let st = render(&p);
        let right: Vec<f32> = st.iter().map(|(m, s)| m - s).collect();
        let left: Vec<f32> = st.iter().map(|(m, s)| m + s).collect();
        assert!(rms(&left[1000..]) > 0.3);
        assert!(rms(&right[1000..]) < 1e-4, "右は無音のまま");
    }

    #[test]
    fn key_track_off_keeps_the_original_pitch() {
        let mut p = params(60);
        p.key_track = false;
        let render = |pitch: u8| {
            let mut v = SamplerVoice::start(&p, pitch, 1.0, Articulation::Normal, 48_000.0);
            (0..12_000).map(|_| v.next(&p)).collect::<Vec<f32>>()
        };
        assert_eq!(crossings(&render(60)), crossings(&render(72)));
        assert_eq!(crossings(&render(60)), crossings(&render(41)));
    }

    /// 0.25 秒おきに、減衰する打音を 4 つ並べた素材(最初の打音は 0.05 秒目から)
    fn hits(sr: f32) -> (Vec<f32>, Vec<usize>) {
        let n = (sr * 1.1) as usize;
        let starts: Vec<usize> = (0..4)
            .map(|k| (sr * (0.05 + 0.25 * k as f32)) as usize)
            .collect();
        let mut x = vec![0.0f32; n];
        for (k, &st) in starts.iter().enumerate() {
            let f = 200.0 * (k + 1) as f32;
            for i in 0..(sr * 0.2) as usize {
                let t = i as f32 / sr;
                x[st + i] += (t * f * std::f32::consts::TAU).sin() * (-t * 30.0).exp() * 0.8;
            }
        }
        (x, starts)
    }

    #[test]
    fn detects_slices_at_the_hits() {
        let sr = 48_000.0;
        let (x, starts) = hits(sr);
        let sl = detect_slices(&x, sr, 16);
        assert_eq!(sl[0], 0);
        assert_eq!(sl.len(), 5, "{sl:?}");
        for (got, want) in sl[1..].iter().zip(&starts) {
            assert!(
                (*got as i64 - *want as i64).abs() < 300,
                "{got} ≈ {want}: {sl:?}"
            );
        }
        // 数を絞ると強い順に選び、頭の並びは昇順
        let few = detect_slices(&x, sr, 3);
        assert_eq!(few.len(), 3);
        assert!(few.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn slice_mode_maps_keys_to_hits() {
        let sr = 48_000.0;
        let (x, starts) = hits(sr);
        let data = Arc::new(SampleData::mono(x, sr));
        let mut p = SamplerParams::one_shot(data.clone(), 36, 1.0, 0.001);
        p.slices = detect_slices(&data.frames, sr, 16).into();
        // 36 = 先頭(無音の頭)、37 = 1 つ目の打音、39 = 3 つ目の打音(600Hz)
        let mut v = SamplerVoice::start(&p, 39, 1.0, Articulation::Normal, sr);
        let mut len = 0;
        let out: Vec<f32> = (0..20_000)
            .map(|k| {
                let s = v.next(&p);
                if !v.finished() {
                    len = k + 1;
                }
                s
            })
            .collect();
        assert!(v.finished(), "次の頭までで止まる");
        let want = starts[3] - starts[2];
        assert!((len as i64 - want as i64).abs() < 400, "{len} ≈ {want}");
        // 600Hz の打音: 最初の 0.1 秒の上向きゼロ交差 ≈ 60
        let c = crossings(&out[..4_800]);
        assert!((55..=65).contains(&c), "{c}");
        // 範囲外の鍵盤は鳴らない
        let mut v = SamplerVoice::start(&p, 30, 1.0, Articulation::Normal, sr);
        assert_eq!(v.next(&p), 0.0);
        assert!(v.finished());
    }

    /// 使う所: 始まり〜終わりだけを鳴らす(1 つの音)。区分の線を手で決めると、その線で区分に分ける(自動で探さない)。
    /// 使う所の外の線は使わない
    #[test]
    fn start_end_and_hand_slice_points() {
        let sr = 48_000.0;
        let (x, starts) = hits(sr);
        let n = x.len() as f32;
        let data = Arc::new(SampleData::mono(x, sr));
        let mut map = glaux_core::ParamMap::new();
        map.insert("root".into(), glaux_core::ParamValue::Float(60.0));
        // 2 つ目の打音の頭〜3 つ目の打音の頭だけを使う
        let a = starts[1] as f32 / n;
        let b = starts[2] as f32 / n;
        map.insert("start".into(), glaux_core::ParamValue::Float(a as f64));
        map.insert("end".into(), glaux_core::ParamValue::Float(b as f64));
        let p = crate::params::bake_sampler(&map, data.clone(), sr);
        let mut v = SamplerVoice::start(&p, 60, 1.0, Articulation::Normal, sr);
        let mut len = 0;
        let out: Vec<f32> = (0..60_000)
            .map(|k| {
                let s = v.next(&p);
                if !v.finished() {
                    len = k + 1;
                }
                s
            })
            .collect();
        let want = starts[2] - starts[1];
        assert!(
            (len as i64 - want as i64).abs() < 400,
            "使う所の長さ {len} ≈ {want}"
        );
        // 400Hz の打音(2 つ目): 最初の 0.1 秒の上向きゼロ交差 ≈ 40
        let c = crossings(&out[..4_800]);
        assert!((35..=45).contains(&c), "{c}");
        // 手で決めた線: 使う所(全部)の中の線 0.5 で 2 つの区分。外の線(1.5)は使わない
        let mut map = glaux_core::ParamMap::new();
        map.insert("root".into(), glaux_core::ParamValue::Float(60.0));
        map.insert("slices".into(), glaux_core::ParamValue::Float(8.0));
        map.insert(
            "slice_points".into(),
            glaux_core::ParamValue::Enum("0.5,1.5".into()),
        );
        let p = crate::params::bake_sampler(&map, data.clone(), sr);
        assert_eq!(p.slices.len(), 2, "{:?}", p.slices);
        assert_eq!(p.slices[0], 0);
        assert!((p.slices[1] as f32 - n * 0.5).abs() < 2.0);
        // 線が無ければ、使う所の中で音の頭を自動で探す
        let mut map = glaux_core::ParamMap::new();
        map.insert("slices".into(), glaux_core::ParamValue::Float(16.0));
        map.insert("start".into(), glaux_core::ParamValue::Float(a as f64));
        let p = crate::params::bake_sampler(&map, data, sr);
        assert_eq!(
            p.slices[0], starts[1] as u32,
            "区分 1 の頭 = 使う所の始まり"
        );
        assert!(p.slices.len() >= 3, "{:?}", p.slices);
    }
}
