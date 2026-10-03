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
//! - **声ごとのモジュレーター 2 本**(mod1 / mod2): テンポに合わせた速さ(拍あたりの回数。Hz も可)、音の頭で揺れ直すか
//!   曲の拍に固定するか、形(定番 + 自分で描くカーブ = MSEG)、1 本から複数の行き先へ深さを変えて送る(変調行列)。
//!   ワブルベースの「速さを拍ごとに切り替えてしゃべらせる」ための仕組み。速さのつまみはオートメーションできる
//!
//! 新しいつまみがすべて既定値なら、従来とまったく同じ音になる(既存の曲の音を変えない)。
//! 揺らぎの乱数は音ごとに決まる種から作るので、同じ曲は何度描き出しても同じ音になる。
//! RT セーフ: 値型のみでアロケーションなし。

use crate::oversample::AdaaTanh;
use std::cell::Cell;

thread_local! {
    /// 今のテンポ(1 拍 = 4 分音符の秒数)と、これから鳴らし始める音の曲の位置(拍)。
    /// レンダラが描き出すスレッドでブロックごと・音の頭ごとに書く(声は同じスレッドで読む。ロックなし)
    static BEAT_SECS: Cell<f32> = const { Cell::new(0.5) };
    static SONG_BEAT: Cell<f64> = const { Cell::new(0.0) };
}

/// 今のテンポ(1 拍の秒数)を知らせる(レンダラがブロックの頭で呼ぶ)
pub fn set_beat_secs(secs: f32) {
    if secs.is_finite() && secs > 0.0 {
        BEAT_SECS.with(|c| c.set(secs));
    }
}

/// これから鳴らし始める音の曲の位置(拍)を知らせる(レンダラが音の頭で呼ぶ)
pub fn set_song_beat(beat: f64) {
    if beat.is_finite() {
        SONG_BEAT.with(|c| c.set(beat));
    }
}

fn beat_secs() -> f32 {
    BEAT_SECS.with(|c| c.get())
}

fn song_beat() -> f64 {
    SONG_BEAT.with(|c| c.get())
}

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
    /// ウェーブテーブルの変形の量(wavetable だけ。深さ 1 で ±0.5)
    Warp,
}

impl LfoTarget {
    pub const NAMES: [&'static str; 6] = ["pitch", "cutoff", "amp", "pan", "position", "warp"];

    pub fn parse(s: &str) -> LfoTarget {
        match s {
            "pitch" => LfoTarget::Pitch,
            "amp" => LfoTarget::Amp,
            "pan" => LfoTarget::Pan,
            "position" => LfoTarget::Position,
            "warp" => LfoTarget::Warp,
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

/// 声ごとのモジュレーターの形
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModShape {
    #[default]
    Sine,
    Triangle,
    Square,
    SawUp,
    SawDown,
    Random,
    /// 自分で描くカーブ(points)と、定番の形(下の名前。中身は点の並び)
    Curve,
}

/// 名前で選べる形(curve の定番)。点は (x 0..1, y −1..1, 曲がり −1..1)。同じ x が続くと段差
pub const MOD_SHAPE_NAMES: [&str; 11] = [
    "sine",
    "triangle",
    "square",
    "saw_up",
    "saw_down",
    "random",
    "wub",
    "saw_down_curve",
    "yoi",
    "stairs",
    "custom",
];

/// 定番の形の点
fn preset_points(name: &str) -> Option<&'static [(f32, f32, f32)]> {
    Some(match name {
        // 「ワウ」: すばやく開いて、ゆっくり閉じる
        "wub" => &[
            (0.0, -1.0, 0.0),
            (0.12, 1.0, -0.6),
            (0.55, 0.1, 0.4),
            (1.0, -1.0, 0.5),
        ],
        // 開いてから指数的に閉じる(ベースの「ブォ」)
        "saw_down_curve" => &[(0.0, 1.0, 0.0), (1.0, -1.0, 0.75)],
        // 「ヨイ」: 1 周期に 2 回、形の違う山
        "yoi" => &[
            (0.0, -1.0, 0.0),
            (0.18, 1.0, -0.5),
            (0.42, -0.5, 0.4),
            (0.62, 0.7, -0.5),
            (1.0, -1.0, 0.5),
        ],
        // 4 段の階段(刻むような変化)
        "stairs" => &[
            (0.0, -1.0, 0.0),
            (0.25, -1.0, 0.0),
            (0.25, -0.33, 0.0),
            (0.5, -0.33, 0.0),
            (0.5, 0.33, 0.0),
            (0.75, 0.33, 0.0),
            (0.75, 1.0, 0.0),
            (1.0, 1.0, 0.0),
        ],
        _ => return None,
    })
}

/// カーブの点の上限
pub const MAX_CURVE_POINTS: usize = 16;

/// 自分で描くカーブ(固定長。オーディオスレッドで確保しない)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModCurve {
    pub pts: [(f32, f32, f32); MAX_CURVE_POINTS],
    pub len: u8,
}

impl Default for ModCurve {
    fn default() -> Self {
        ModCurve {
            pts: [(0.0, 0.0, 0.0); MAX_CURVE_POINTS],
            len: 0,
        }
    }
}

impl ModCurve {
    pub fn from_points(points: &[(f32, f32, f32)]) -> ModCurve {
        let mut c = ModCurve::default();
        let mut last_x = 0.0f32;
        for (i, &(x, y, k)) in points.iter().take(MAX_CURVE_POINTS).enumerate() {
            let x = x.clamp(last_x, 1.0);
            last_x = x;
            c.pts[i] = (x, y.clamp(-1.0, 1.0), k.clamp(-1.0, 1.0));
            c.len = (i + 1) as u8;
        }
        c
    }

    /// 文字列 "x,y[,曲がり]; x,y; …"(x 0〜1 昇順、y −1〜1、曲がり −1〜1。正で後ろ寄り = 始めゆっくり)。読めなければ None
    pub fn parse(s: &str) -> Option<ModCurve> {
        let mut pts = Vec::new();
        for item in s.split(';').map(str::trim).filter(|t| !t.is_empty()) {
            let v: Vec<f32> = item
                .split(',')
                .map(|t| t.trim().parse::<f32>())
                .collect::<Result<_, _>>()
                .ok()?;
            if v.len() < 2 || v.iter().any(|x| !x.is_finite()) {
                return None;
            }
            pts.push((v[0], v[1], v.get(2).copied().unwrap_or(0.0)));
        }
        if pts.is_empty() || pts.windows(2).any(|w| w[1].0 < w[0].0) {
            return None;
        }
        Some(ModCurve::from_points(&pts))
    }

    /// 位相 `ph`(0..1)での値(−1..1)
    pub fn at(&self, ph: f32) -> f32 {
        let n = self.len as usize;
        if n == 0 {
            return 0.0;
        }
        let pts = &self.pts[..n];
        if ph <= pts[0].0 {
            return pts[0].1;
        }
        // ph を挟む区間(同じ x の段差は後ろの点を使う)
        let mut i = 0;
        while i + 1 < n && pts[i + 1].0 <= ph {
            i += 1;
        }
        if i + 1 >= n {
            return pts[n - 1].1;
        }
        let (x0, y0, _) = pts[i];
        let (x1, y1, k) = pts[i + 1];
        let w = x1 - x0;
        if w <= 1e-6 {
            return y1;
        }
        let t = ((ph - x0) / w).clamp(0.0, 1.0);
        // 曲がり: 正で始めゆっくり(t の累乗)、負で始め速く
        let t = if k >= 0.0 {
            t.powf(1.0 + k * 4.0)
        } else {
            1.0 - (1.0 - t).powf(1.0 - k * 4.0)
        };
        y0 + (y1 - y0) * t
    }
}

/// 音の頭で揺れ直すか
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModRetrig {
    /// 音の頭で位相を戻す(音ごとに同じ揺れ。ワブルの基本)
    #[default]
    Note,
    /// 曲の拍に固定する(全声部がそろい、長い音でも拍の頭で揺れがそろう。速さを拍ごとに替えても拍に合う)
    Song,
}

/// モジュレーターの行き先の数
pub const MOD_DESTS: usize = 8;
/// 行き先の名前(つまみの名前は `mod1_<行き先>`)
pub const MOD_DEST_NAMES: [&str; MOD_DESTS] = [
    "pitch", "cutoff", "res", "amp", "pan", "position", "warp", "drive",
];

/// 声ごとのモジュレーター 1 本(焼き込み済み)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModParams {
    /// 速さ: 拍(4 分音符)あたりの回数(1/4 = 1、1/8 = 2、1/8t = 3、1/16 = 4)。`hz` が 0 より大きければそちら
    pub rate: f32,
    pub hz: f32,
    pub shape: ModShape,
    pub curve: ModCurve,
    pub retrig: ModRetrig,
    /// 始まりの位相 0..1
    pub phase: f32,
    /// 行き先ごとの深さ −1..1([`MOD_DEST_NAMES`] の順)
    pub depth: [f32; MOD_DESTS],
}

impl Default for ModParams {
    fn default() -> Self {
        ModParams {
            rate: 2.0,
            hz: 0.0,
            shape: ModShape::Sine,
            curve: ModCurve::default(),
            retrig: ModRetrig::Note,
            phase: 0.0,
            depth: [0.0; MOD_DESTS],
        }
    }
}

impl ModParams {
    fn active(&self) -> bool {
        self.depth.iter().any(|d| *d != 0.0)
    }

    /// 形と点を名前から決める(curve の定番は点を入れる。custom は `points` の文字列)
    pub fn set_shape(&mut self, name: &str, points: &str) {
        self.shape = match name {
            "triangle" => ModShape::Triangle,
            "square" => ModShape::Square,
            "saw_up" => ModShape::SawUp,
            "saw_down" => ModShape::SawDown,
            "random" => ModShape::Random,
            "sine" => ModShape::Sine,
            _ => ModShape::Curve,
        };
        if self.shape == ModShape::Curve {
            self.curve = match preset_points(name) {
                Some(p) => ModCurve::from_points(p),
                None => ModCurve::parse(points).unwrap_or_else(|| {
                    // 読めない・空のカーブは正弦の代わり
                    self.shape = ModShape::Sine;
                    ModCurve::default()
                }),
            };
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
    /// 声ごとのモジュレーター(深さが全部 0 なら掛からない)
    pub mods: [ModParams; 2],
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
            mods: [ModParams::default(); 2],
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
            _ => {
                // mod1_rate / mod2_cutoff …
                let Some(rest) = name.strip_prefix("mod") else {
                    return false;
                };
                let (k, key) = match rest.split_once('_') {
                    Some(("1", key)) => (0, key),
                    Some(("2", key)) => (1, key),
                    _ => return false,
                };
                let m = &mut self.mods[k];
                match key {
                    "rate" => m.rate = v.clamp(1.0 / 32.0, 32.0),
                    "hz" => m.hz = v.clamp(0.0, 40.0),
                    "phase" => m.phase = v.clamp(0.0, 1.0),
                    _ => match MOD_DEST_NAMES.iter().position(|d| *d == key) {
                        Some(i) => m.depth[i] = v.clamp(-1.0, 1.0),
                        None => return false,
                    },
                }
            }
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
            && !self.mods[0].active()
            && !self.mods[1].active()
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
    /// 変形の量に足す分(wavetable)
    pub warp_add: f32,
    /// レゾナンス・ドライブに足す分(声ごとのモジュレーター)
    pub res_add: f32,
    pub drive_add: f32,
    /// 声ごとのモジュレーターの位相・乱数の今の値と、この音の曲の位置(拍)
    mod_phase: [f32; 2],
    mod_hold: [f32; 2],
    song_beat: f64,
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
            warp_add: 0.0,
            res_add: 0.0,
            drive_add: 0.0,
            mod_phase: [0.0; 2],
            mod_hold: [0.0; 2],
            song_beat: song_beat(),
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
        // 声ごとのモジュレーターの乱数は、ほかの乱数の並びを変えないよう別の種から
        let mut r = (seed ^ 0x2545_f491).wrapping_mul(0x9e37_79b9) | 1;
        for h in self.mod_hold.iter_mut() {
            r ^= r << 13;
            r ^= r >> 17;
            r ^= r << 5;
            *h = (r as f32 / u32::MAX as f32) * 2.0 - 1.0;
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
        // 声ごとのモジュレーターは音の頭で始まりの位相へ(曲の拍に固定するものは control で求める)
        for (ph, m) in self.mod_phase.iter_mut().zip(&p.mods) {
            *ph = m.phase;
        }
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
        let mut warp = 0.0f32;
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
                LfoTarget::Warp => warp += v * l.depth * 0.5,
            }
        }
        // 声ごとのモジュレーター(深さが 0 のものは計算しない)
        let mut res = 0.0f32;
        let mut drive = 0.0f32;
        // 曲の位置は常に進める(途中のオートメーションで掛け始めても拍に合うように)
        let beats = dt / beat_secs();
        if p.mods[0].active() || p.mods[1].active() {
            for k in 0..2 {
                let m = &p.mods[k];
                if !m.active() {
                    continue;
                }
                // 位相: 音の頭からの経過か、曲の拍に固定か
                let ph = match (m.retrig, m.hz > 0.0) {
                    (ModRetrig::Song, false) => {
                        ((self.song_beat * m.rate as f64 + m.phase as f64).rem_euclid(1.0)) as f32
                    }
                    _ => self.mod_phase[k],
                };
                let v = match m.shape {
                    ModShape::Sine => (ph * std::f32::consts::TAU).sin(),
                    ModShape::Triangle => 1.0 - 4.0 * (ph - 0.5).abs(),
                    ModShape::Square => {
                        if ph < 0.5 {
                            1.0
                        } else {
                            -1.0
                        }
                    }
                    ModShape::SawUp => 2.0 * ph - 1.0,
                    ModShape::SawDown => 1.0 - 2.0 * ph,
                    ModShape::Random => self.mod_hold[k],
                    ModShape::Curve => m.curve.at(ph),
                };
                let step = if m.hz > 0.0 {
                    m.hz * dt
                } else {
                    m.rate * beats
                };
                let next = ph + step;
                if next >= 1.0 && m.shape == ModShape::Random {
                    // 周期ごとに次の値(声の乱数とは別に回す)
                    let mut r = self.mod_hold[k].to_bits() | 1;
                    r ^= r << 13;
                    r ^= r >> 17;
                    r ^= r << 5;
                    self.mod_hold[k] = (r as f32 / u32::MAX as f32) * 2.0 - 1.0;
                }
                self.mod_phase[k] = next.fract();
                let d = &m.depth;
                pitch_semi += v * d[0] * 12.0;
                cutoff_oct += v * d[1] * 4.0;
                res += v * d[2] * 0.9;
                if d[3] != 0.0 {
                    let a = d[3].abs();
                    let vv = if d[3] > 0.0 { v } else { -v };
                    amp *= 1.0 - a * (0.5 - 0.5 * vv);
                }
                pan += v * d[4];
                pos += v * d[5];
                warp += v * d[6];
                drive += v * d[7];
            }
        }
        self.song_beat += beats as f64;
        self.res_add = res;
        self.drive_add = drive;
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
        self.warp_add = warp;
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
        let drive = if self.drive_add != 0.0 {
            (p.drive + self.drive_add).clamp(0.0, 1.0)
        } else {
            p.drive
        };
        if drive > 0.0 {
            // 左右それぞれで歪ませる(ADAA の tanh。1 + drive × 7 倍まで押し込み、音量は戻す)
            let pre = 1.0 + drive * 7.0;
            let post = 1.0 / (1.0 + drive * 2.0);
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

    /// レゾナンス(声ごとのモジュレーターの分を足す)
    #[inline]
    pub fn resonance(&self, base: f32) -> f32 {
        if self.res_add != 0.0 {
            (base + self.res_add).clamp(0.0, 0.95)
        } else {
            base
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

    /// 32 サンプルごとに control を回し、各時点の値を集める
    fn run_mod(
        p: &ToneParams,
        v: &mut ToneVoice,
        steps: usize,
        f: impl Fn(&ToneVoice) -> f32,
    ) -> Vec<f32> {
        let mut ph = [0.0f32; MAX_UNISON];
        v.start_phases(p, &mut ph);
        (0..steps)
            .map(|_| {
                v.control(p, 32, 48_000.0, false);
                f(v)
            })
            .collect()
    }

    fn cutoff_mod(rate: f32, shape: &str) -> ToneParams {
        let mut p = ToneParams::default();
        p.mods[0].rate = rate;
        p.mods[0].set_shape(shape, "");
        p.mods[0].depth[1] = 0.5;
        p
    }

    #[test]
    fn mod_rate_follows_the_tempo() {
        // 120 BPM(1 拍 0.5 秒)で 1/8(拍あたり 2 回)= 0.25 秒 = 375 ステップで 1 周期
        set_beat_secs(0.5);
        set_song_beat(0.0);
        let p = cutoff_mod(2.0, "saw_up");
        let mut v = ToneVoice::new(1);
        let c = run_mod(&p, &mut v, 1200, |v| v.cutoff_mul.log2());
        // 鋸歯の折り返し(急に下がる所)の間隔
        let wraps: Vec<usize> = (1..c.len()).filter(|&i| c[i] < c[i - 1] - 0.5).collect();
        assert!(wraps.len() >= 2, "{wraps:?}");
        assert!(
            wraps.windows(2).all(|w| (w[1] - w[0]).abs_diff(375) <= 1),
            "{wraps:?}"
        );
        // テンポが倍になると周期は半分
        set_beat_secs(0.25);
        let mut v = ToneVoice::new(1);
        let c = run_mod(&p, &mut v, 600, |v| v.cutoff_mul.log2());
        let wraps: Vec<usize> = (1..c.len()).filter(|&i| c[i] < c[i - 1] - 0.5).collect();
        assert!(
            wraps.windows(2).all(|w| (w[1] - w[0]).abs_diff(188) <= 1),
            "{wraps:?}"
        );
        set_beat_secs(0.5);
    }

    #[test]
    fn note_retrig_restarts_and_song_mode_locks_to_the_beat() {
        set_beat_secs(0.5);
        let mut p = cutoff_mod(1.0, "saw_up");
        // 音の頭で揺れ直す: 曲のどこで鳴らしても、頭の値は同じ
        set_song_beat(0.3);
        let a = run_mod(&p, &mut ToneVoice::new(1), 3, |v| v.cutoff_mul);
        set_song_beat(2.7);
        let b = run_mod(&p, &mut ToneVoice::new(2), 3, |v| v.cutoff_mul);
        assert_eq!(a, b);
        // 曲の拍に固定: 拍の 0.25 から鳴らすと、位相も 0.25 から(頭で鳴らした音の 0.25 拍後と同じ)
        p.mods[0].retrig = ModRetrig::Song;
        set_song_beat(0.25);
        let late = run_mod(&p, &mut ToneVoice::new(3), 1, |v| v.cutoff_mul)[0];
        set_song_beat(0.0);
        let early = run_mod(&p, &mut ToneVoice::new(4), 376, |v| v.cutoff_mul);
        // 0.25 拍 = 0.125 秒 = 187.5 ステップ後の値と同じ
        let d = |i: usize| (late.log2() - early[i].log2()).abs();
        assert!(
            d(187).min(d(188)) < 0.02,
            "{late} vs {} / {}",
            early[187],
            early[188]
        );
    }

    #[test]
    fn curves_follow_their_points() {
        let c = ModCurve::parse("0,-1; 0.5,1; 1,-1").unwrap();
        assert!((c.at(0.0) + 1.0).abs() < 1e-6);
        assert!((c.at(0.25)).abs() < 1e-6);
        assert!((c.at(0.5) - 1.0).abs() < 1e-6);
        // 曲がり: 正で始めゆっくり
        let slow = ModCurve::parse("0,0; 1,1,0.5").unwrap();
        assert!(slow.at(0.5) < 0.5);
        let fast = ModCurve::parse("0,0; 1,1,-0.5").unwrap();
        assert!(fast.at(0.5) > 0.5);
        // 段差(同じ x)
        let step = ModCurve::parse("0,0; 0.5,0; 0.5,1; 1,1").unwrap();
        assert_eq!((step.at(0.49), step.at(0.51)), (0.0, 1.0));
        // 読めないもの
        assert!(ModCurve::parse("0,0; 0.5").is_none());
        assert!(ModCurve::parse("0.5,0; 0.2,1").is_none());
        // 定番の形はどれも −1..1 の中で動く
        for name in ["wub", "saw_down_curve", "yoi", "stairs"] {
            let mut m = ModParams::default();
            m.set_shape(name, "");
            assert_eq!(m.shape, ModShape::Curve);
            let vals: Vec<f32> = (0..=100).map(|i| m.curve.at(i as f32 / 100.0)).collect();
            assert!(vals.iter().all(|v| (-1.0..=1.0).contains(v)), "{name}");
            let span = vals.iter().fold(f32::MIN, |a, b| a.max(*b))
                - vals.iter().fold(f32::MAX, |a, b| a.min(*b));
            assert!(span > 1.0, "{name}");
        }
        // custom の読めない点は正弦に
        let mut m = ModParams::default();
        m.set_shape("custom", "nope");
        assert_eq!(m.shape, ModShape::Sine);
    }

    #[test]
    fn one_mod_drives_several_destinations() {
        set_beat_secs(0.5);
        let mut p = cutoff_mod(1.0, "sine");
        p.mods[0].depth[5] = 0.5; // position
        p.mods[0].depth[2] = -0.5; // レゾナンスは逆向き
        p.mods[0].depth[7] = 0.5; // ドライブ
        let mut v = ToneVoice::new(1);
        let mut ph = [0.0f32; MAX_UNISON];
        v.start_phases(&p, &mut ph);
        for _ in 0..100 {
            v.control(&p, 32, 48_000.0, false);
        }
        // サインの山の手前(位相 > 0): カットオフは開き、position は進み、レゾナンスは下がる
        assert!(v.cutoff_mul > 1.0);
        assert!(v.position_add > 0.0);
        assert!(v.resonance(0.5) < 0.5);
        assert!(v.drive_add > 0.0);
        assert!(!v.plain);
    }

    #[test]
    fn mod_knobs_are_automatable_and_default_is_plain() {
        let mut p = ToneParams::default();
        assert!(p.is_plain());
        assert!(p.set_continuous("mod2_rate", 6.0));
        assert!(p.set_continuous("mod1_cutoff", 0.7));
        assert!(p.set_continuous("mod2_position", -0.4));
        assert!(!p.set_continuous("mod3_rate", 1.0));
        assert!(!p.set_continuous("mod1_nope", 1.0));
        assert_eq!(p.mods[1].rate, 6.0);
        assert_eq!(p.mods[0].depth[1], 0.7);
        assert_eq!(p.mods[1].depth[5], -0.4);
        assert!(!p.is_plain());
        for n in MOD_DEST_NAMES {
            let mut q = ToneParams::default();
            assert!(q.set_continuous(&format!("mod1_{n}"), 0.1), "{n}");
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
