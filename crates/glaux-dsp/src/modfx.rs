//! 変調系・質感系のエフェクト(段階 3 で足したもの): clipper / bitcrush / tremolo(オートパン)/ phaser /
//! flanger / trance_gate / auto_filter / volume_shaper。
//!
//! テンポに合わせる LFO・型は、エンジンがブロックごとに渡す曲の位置(tick)から位相を出す
//! (`ModFxState::set_clock`)。停止中は位置が進まないので止まったまま。
//! 状態は固定長(flanger のディレイだけ起動時に確保)で、オーディオスレッドでは確保しない。

use crate::effects::{SvfCoeffs, SvfState};
use glaux_core::{ParamMap, ParamRange, ParamSpec, ParamValue};

const TAU: f32 = std::f32::consts::TAU;
/// flanger のディレイの長さ(1ch、2 のべき乗。96kHz で約 21ms)
const FL_LEN: usize = 2048;
const FL_MASK: usize = FL_LEN - 1;

/// テンポに合わせる周期の選択肢(tick。0 = 合わせない)
const SYNC_CHOICES: &[&str] = &[
    "off", "4/1", "2/1", "1/1", "1/2", "1/4", "1/8", "1/16", "1/32", "1/2d", "1/4d", "1/8d",
    "1/16d", "1/2t", "1/4t", "1/8t", "1/16t",
];

fn sync_ticks(s: &str) -> f64 {
    let q = glaux_core::PPQ as f64 * 4.0;
    let (base, mul) = if let Some(b) = s.strip_suffix('d') {
        (b, 1.5)
    } else if let Some(b) = s.strip_suffix('t') {
        (b, 2.0 / 3.0)
    } else {
        (s, 1.0)
    };
    let Some((n, d)) = base.split_once('/') else {
        return 0.0;
    };
    let (Ok(n), Ok(d)) = (n.parse::<f64>(), d.parse::<f64>()) else {
        return 0.0;
    };
    q * n / d * mul
}

/// LFO の形
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Sine,
    Triangle,
    Square,
    SawDown,
}

impl Shape {
    fn parse(s: &str) -> Shape {
        match s {
            "triangle" => Shape::Triangle,
            "square" => Shape::Square,
            "saw_down" => Shape::SawDown,
            _ => Shape::Sine,
        }
    }

    /// 位相 0〜1 → −1〜1
    fn at(self, ph: f32) -> f32 {
        match self {
            Shape::Sine => (TAU * ph).sin(),
            Shape::Triangle => 1.0 - 4.0 * (ph - 0.25 - (ph - 0.25).floor() - 0.5).abs(),
            Shape::Square => {
                if ph.fract() < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            Shape::SawDown => 1.0 - 2.0 * ph.fract(),
        }
    }
}

/// 速さ(テンポに合わせる周期 tick か、Hz の 1 サンプルあたりの位相増分)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rate {
    pub sync_ticks: f64,
    pub inc: f32,
}

impl Rate {
    fn from(map: &ParamMap, specs: &[ParamSpec], sr: f32) -> Rate {
        Rate {
            sync_ticks: sync_ticks(choice(map, specs, "sync")),
            inc: get(map, specs, "rate_hz") / sr,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipMode {
    Soft,
    Hard,
    Fold,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterMode {
    Low,
    High,
    Band,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShaperCurve {
    /// 素早く沈んで指数的に戻る(Kickstart 型)
    Pump,
    /// 直線で戻る
    Linear,
    /// 沈んだまま、周期の終わりで戻る
    Gate,
}

/// 焼き込み済みの値
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ModFxParams {
    Clipper {
        drive: f32,
        mode: ClipMode,
        ceiling: f32,
        mix: f32,
    },
    Bitcrush {
        levels: f32,
        hold: u32,
        mix: f32,
    },
    Tremolo {
        rate: Rate,
        depth: f32,
        shape: Shape,
        stereo: f32,
    },
    Phaser {
        rate: Rate,
        depth: f32,
        feedback: f32,
        stages: u8,
        center: f32,
        mix: f32,
        sr: f32,
    },
    Flanger {
        rate: Rate,
        depth: f32,
        base: f32,
        feedback: f32,
        mix: f32,
    },
    TranceGate {
        pattern: [bool; 16],
        step_ticks: f64,
        depth: f32,
        smooth: f32,
    },
    AutoFilter {
        mode: FilterMode,
        cutoff: f32,
        resonance: f32,
        rate: Rate,
        depth: f32,
        shape: Shape,
        env_amount: f32,
        sr: f32,
    },
    VolumeShaper {
        period_ticks: f64,
        depth: f32,
        curve: ShaperCurve,
        release: f32,
    },
}

/// 状態
#[derive(Clone, Debug)]
pub struct ModFxState {
    /// 曲の位置(tick)と 1 サンプルあたりの tick
    tick: f64,
    tps: f64,
    /// Hz の LFO の位相
    phase: f32,
    // bitcrush のサンプルホールド
    held: [f32; 2],
    hold_count: u32,
    // phaser のオールパスの状態(2ch × 8 段)と帰還
    ap_x: [[f32; 8]; 2],
    ap_y: [[f32; 8]; 2],
    ph_fb: [f32; 2],
    // flanger のディレイ
    fl: [Vec<f32>; 2],
    fl_idx: usize,
    // trance_gate の平滑化した音量
    gate: f32,
    // auto_filter の SVF と音量の追従
    svf: [SvfState; 2],
    coeffs: Option<SvfCoeffs>,
    coef_count: u32,
    env: f32,
}

impl Default for ModFxState {
    fn default() -> Self {
        ModFxState {
            fl: [vec![0.0; FL_LEN], vec![0.0; FL_LEN]],
            ..Self::light()
        }
    }
}

impl ModFxState {
    /// flanger のバッファを持たない軽い状態(flanger は素通し)
    pub fn light() -> Self {
        ModFxState {
            tick: 0.0,
            tps: 0.0,
            phase: 0.0,
            held: [0.0; 2],
            hold_count: 0,
            ap_x: [[0.0; 8]; 2],
            ap_y: [[0.0; 8]; 2],
            ph_fb: [0.0; 2],
            fl: [Vec::new(), Vec::new()],
            fl_idx: 0,
            gate: 1.0,
            svf: Default::default(),
            coeffs: None,
            coef_count: 0,
            env: 0.0,
        }
    }

    /// 状態を初めに戻す(種類が変わったとき。確保しない)
    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.held = [0.0; 2];
        self.hold_count = 0;
        self.ap_x = [[0.0; 8]; 2];
        self.ap_y = [[0.0; 8]; 2];
        self.ph_fb = [0.0; 2];
        for b in &mut self.fl {
            b.fill(0.0);
        }
        self.fl_idx = 0;
        self.gate = 1.0;
        self.svf = Default::default();
        self.coeffs = None;
        self.coef_count = 0;
        self.env = 0.0;
    }

    /// このブロックの頭の曲の位置(tick)と、1 サンプルあたりの tick(停止中は 0)
    pub fn set_clock(&mut self, tick: f64, ticks_per_sample: f64) {
        self.tick = tick;
        self.tps = ticks_per_sample;
    }

    /// LFO の位相(0〜1)。テンポに合わせるなら曲の位置から、でなければ Hz で進める
    fn lfo_phase(&mut self, r: &Rate) -> f32 {
        if r.sync_ticks > 0.0 {
            (self.tick / r.sync_ticks).fract() as f32
        } else {
            let ph = self.phase;
            self.phase = (ph + r.inc).fract();
            ph
        }
    }

    pub fn process(&mut self, p: &ModFxParams, l: f32, r: f32) -> (f32, f32) {
        let out = match *p {
            ModFxParams::Clipper {
                drive,
                mode,
                ceiling,
                mix,
            } => {
                let f = |x: f32| {
                    let y = x * drive / ceiling;
                    let c = match mode {
                        ClipMode::Soft => y.tanh(),
                        ClipMode::Hard => y.clamp(-1.0, 1.0),
                        // 1 を超えた分を折り返す(サイン折り返し)
                        ClipMode::Fold => (y * std::f32::consts::FRAC_PI_2).sin(),
                    };
                    c * ceiling
                };
                (l + (f(l) - l) * mix, r + (f(r) - r) * mix)
            }
            ModFxParams::Bitcrush { levels, hold, mix } => {
                if self.hold_count == 0 {
                    let q = |x: f32| (x * levels).round() / levels;
                    self.held = [q(l), q(r)];
                }
                self.hold_count = (self.hold_count + 1) % hold.max(1);
                (l + (self.held[0] - l) * mix, r + (self.held[1] - r) * mix)
            }
            ModFxParams::Tremolo {
                rate,
                depth,
                shape,
                stereo,
            } => {
                let ph = self.lfo_phase(&rate);
                let a = shape.at(ph);
                // stereo 1 で左右が逆向き(オートパン)
                let b = shape.at((ph + 0.5 * stereo).fract());
                let gl = 1.0 - depth * 0.5 * (1.0 - a);
                let gr = 1.0 - depth * 0.5 * (1.0 - b);
                (l * gl, r * gr)
            }
            ModFxParams::Phaser {
                rate,
                depth,
                feedback,
                stages,
                center,
                mix,
                sr,
            } => {
                let ph = self.lfo_phase(&rate);
                // 中心から ±depth × 2 オクターブ
                let fc = (center * (depth * 2.0 * (TAU * ph).sin()).exp2()).clamp(40.0, sr * 0.45);
                let t = (std::f32::consts::PI * fc / sr).tan();
                let a = (t - 1.0) / (t + 1.0);
                let mut run = |ch: usize, x: f32| {
                    let mut s = x + self.ph_fb[ch] * feedback;
                    for k in 0..stages as usize {
                        let y = a * s + self.ap_x[ch][k] - a * self.ap_y[ch][k];
                        self.ap_x[ch][k] = s;
                        self.ap_y[ch][k] = y;
                        s = y;
                    }
                    self.ph_fb[ch] = s;
                    x * (1.0 - mix * 0.5) + s * mix * 0.5
                };
                (run(0, l), run(1, r))
            }
            ModFxParams::Flanger {
                rate,
                depth,
                base,
                feedback,
                mix,
            } => {
                if self.fl[0].is_empty() {
                    return (l, r);
                }
                let ph = self.lfo_phase(&rate);
                let idx = self.fl_idx;
                let mut out = [0.0f32; 2];
                for (ch, x) in [l, r].into_iter().enumerate() {
                    // 左右で LFO を 90° ずらす
                    let m = (TAU * (ph + 0.25 * ch as f32)).sin() * 0.5 + 0.5;
                    let d = (base + depth * m).clamp(1.0, (FL_LEN - 2) as f32);
                    let pos = idx as f32 + FL_LEN as f32 - d;
                    let i0 = pos.floor() as usize & FL_MASK;
                    let fr = pos.fract();
                    let buf = &self.fl[ch];
                    let y = buf[i0] * (1.0 - fr) + buf[(i0 + 1) & FL_MASK] * fr;
                    self.fl[ch][idx] = x + y * feedback;
                    out[ch] = x * (1.0 - mix * 0.5) + y * mix * 0.5;
                }
                self.fl_idx = (idx + 1) & FL_MASK;
                (out[0], out[1])
            }
            ModFxParams::TranceGate {
                pattern,
                step_ticks,
                depth,
                smooth,
            } => {
                let on = if step_ticks > 0.0 {
                    let step = (self.tick / step_ticks).floor() as i64;
                    pattern[step.rem_euclid(16) as usize]
                } else {
                    true
                };
                let target = if on { 1.0 } else { 1.0 - depth };
                self.gate += (target - self.gate) * smooth;
                (l * self.gate, r * self.gate)
            }
            ModFxParams::AutoFilter {
                mode,
                cutoff,
                resonance,
                rate,
                depth,
                shape,
                env_amount,
                sr,
            } => {
                let ph = self.lfo_phase(&rate);
                // 音量に追従(速く上がってゆっくり下がる)
                let lvl = l.abs().max(r.abs());
                let k = if lvl > self.env { 0.01 } else { 0.0005 };
                self.env += (lvl - self.env) * k;
                if self.coef_count == 0 || self.coeffs.is_none() {
                    let oct = depth * shape.at(ph) + env_amount * self.env.min(1.0) * 2.0;
                    let fc = (cutoff * oct.exp2()).clamp(30.0, sr * 0.45);
                    let q = 0.5 + resonance * 9.5;
                    self.coeffs = Some(match mode {
                        FilterMode::Low => SvfCoeffs::low_pass_q(sr, fc, q),
                        FilterMode::High => SvfCoeffs::high_pass_q(sr, fc, q),
                        FilterMode::Band => SvfCoeffs::band_pass_q(sr, fc, q),
                    });
                }
                self.coef_count = (self.coef_count + 1) % 16;
                let c = self.coeffs.expect("上で作る");
                (
                    self.svf[0].process(&c, 1.0, l),
                    self.svf[1].process(&c, 1.0, r),
                )
            }
            ModFxParams::VolumeShaper {
                period_ticks,
                depth,
                curve,
                release,
            } => {
                let x = if period_ticks > 0.0 {
                    (self.tick / period_ticks).fract() as f32
                } else {
                    1.0
                };
                // 周期の頭で沈み、release の割合で戻る
                let y = (x / release.max(0.01)).min(1.0);
                let back = match curve {
                    ShaperCurve::Pump => 1.0 - (1.0 - y).powi(3),
                    ShaperCurve::Linear => y,
                    ShaperCurve::Gate => {
                        if y >= 1.0 {
                            1.0
                        } else {
                            0.0
                        }
                    }
                };
                let g = depth + (1.0 - depth) * back;
                (l * g, r * g)
            }
        };
        self.tick += self.tps;
        out
    }
}

fn get(map: &ParamMap, specs: &[ParamSpec], name: &str) -> f32 {
    if let Some(v) = map.get(name).and_then(ParamValue::as_f64) {
        return v as f32;
    }
    match specs.iter().find(|s| s.name == name).map(|s| &s.range) {
        Some(ParamRange::Float { default, .. }) => *default as f32,
        _ => 0.0,
    }
}

fn choice<'a>(map: &'a ParamMap, specs: &[ParamSpec], name: &str) -> &'a str {
    if let Some(ParamValue::Enum(v)) = map.get(name) {
        return v.as_str();
    }
    match specs.iter().find(|s| s.name == name).map(|s| &s.range) {
        Some(ParamRange::Enum { default, .. }) => default,
        _ => "",
    }
}

const fn f(
    name: &'static str,
    display_name: &'static str,
    unit: Option<&'static str>,
    min: f64,
    max: f64,
    default: f64,
    description: &'static str,
) -> ParamSpec {
    ParamSpec {
        name,
        display_name,
        unit,
        range: ParamRange::Float {
            min,
            max,
            default,
            skew: None,
        },
        description,
    }
}

const fn e(
    name: &'static str,
    display_name: &'static str,
    choices: &'static [&'static str],
    default: &'static str,
    description: &'static str,
) -> ParamSpec {
    ParamSpec {
        name,
        display_name,
        unit: None,
        range: ParamRange::Enum { choices, default },
        description,
    }
}

const SYNC_DESC: &str = "テンポに合わせた周期(1/4 = 4 分、1/8d = 付点 8 分、1/8t = 3 連の 8 分、1/1 = 1 小節)。off で rate_hz を使う。";

pub static CLIPPER_SPECS: &[ParamSpec] = &[
    f("drive_db", "ドライブ", Some("dB"), 0.0, 24.0, 6.0, "入れる前に上げる量。上げるほど頭が潰れて音が前に出る(ドラムバス・マスター前は 2〜6、ベースの歪みは 12 以上)。"),
    e("mode", "形", &["soft", "hard", "fold"], "soft", "soft は丸く潰す(tanh)、hard は天井で切る(硬くて速い)、fold は超えた分を折り返す(倍音が増えて金属的)。"),
    f("ceiling_db", "天井", Some("dB"), -12.0, 0.0, -0.3, "潰す上限の音量。"),
    f("mix", "ミックス", None, 0.0, 1.0, 1.0, "原音と混ぜる割合(パラレルに潰す)。"),
];

pub static BITCRUSH_SPECS: &[ParamSpec] = &[
    f(
        "bits",
        "ビット",
        None,
        1.0,
        16.0,
        8.0,
        "音の細かさ。8 でファミコン風、4 以下で強いザラつき。",
    ),
    f(
        "downsample",
        "ダウンサンプル",
        None,
        1.0,
        32.0,
        1.0,
        "何サンプルごとに値を保持するか。上げるほど高域が荒く、ラジオ・ゲーム機のような音。",
    ),
    f("mix", "ミックス", None, 0.0, 1.0, 1.0, "原音と混ぜる割合。"),
];

pub static TREMOLO_SPECS: &[ParamSpec] = &[
    e("sync", "同期", SYNC_CHOICES, "1/8", SYNC_DESC),
    f(
        "rate_hz",
        "レート",
        Some("Hz"),
        0.1,
        20.0,
        4.0,
        "sync が off のときの揺れの速さ。",
    ),
    f(
        "depth",
        "深さ",
        None,
        0.0,
        1.0,
        0.5,
        "音量の揺れの深さ。1 で消えるところまで。",
    ),
    e(
        "shape",
        "形",
        &["sine", "triangle", "square", "saw_down"],
        "sine",
        "揺れの形。square でスタッターのように刻む、saw_down で頭が強い刻み。",
    ),
    f(
        "stereo",
        "左右",
        None,
        0.0,
        1.0,
        0.0,
        "0 で左右同時(トレモロ)、1 で左右が逆向き(オートパン)。",
    ),
];

pub static PHASER_SPECS: &[ParamSpec] = &[
    e("sync", "同期", SYNC_CHOICES, "off", SYNC_DESC),
    f(
        "rate_hz",
        "レート",
        Some("Hz"),
        0.02,
        8.0,
        0.3,
        "うねりの速さ。0.1〜0.5 でゆったり、2 以上で揺れが目立つ。",
    ),
    f("depth", "深さ", None, 0.0, 1.0, 0.7, "うねりの幅。"),
    f(
        "feedback",
        "フィードバック",
        None,
        0.0,
        0.9,
        0.5,
        "上げるほどシュワシュワした癖が強い。",
    ),
    e(
        "stages",
        "段",
        &["4", "6", "8"],
        "6",
        "オールパスの段数。多いほど溝が増えて濃い。",
    ),
    f(
        "center_hz",
        "中心",
        Some("Hz"),
        200.0,
        4000.0,
        800.0,
        "うねる帯域の中心。",
    ),
    f(
        "mix",
        "ミックス",
        None,
        0.0,
        1.0,
        1.0,
        "原音と混ぜる割合(1 で半々の定番の位相の打ち消し)。",
    ),
];

pub static FLANGER_SPECS: &[ParamSpec] = &[
    e("sync", "同期", SYNC_CHOICES, "off", SYNC_DESC),
    f(
        "rate_hz",
        "レート",
        Some("Hz"),
        0.02,
        5.0,
        0.25,
        "ジェット機のような揺れの速さ。",
    ),
    f(
        "depth_ms",
        "深さ",
        Some("ms"),
        0.0,
        8.0,
        2.0,
        "遅れを揺らす幅。",
    ),
    f(
        "delay_ms",
        "遅れ",
        Some("ms"),
        0.5,
        10.0,
        1.5,
        "中心の遅れ。短いほど高い帯域で鳴る。",
    ),
    f(
        "feedback",
        "フィードバック",
        None,
        -0.9,
        0.9,
        0.5,
        "上げるほど金属的に鳴る(負で別の癖)。",
    ),
    f("mix", "ミックス", None, 0.0, 1.0, 1.0, "原音と混ぜる割合。"),
];

const GATE_PATTERNS: &[(&str, &str)] = &[
    ("8th", "x-x-x-x-x-x-x-x-"),
    ("16th", "xxxxxxxxxxxxxxxx"),
    ("offbeat", "--x---x---x---x-"),
    ("dotted", "x--x--x--x--x--x"),
    ("syncopated", "x-xx-x-xx-x-x-xx"),
    ("halftime", "x-------x-------"),
    ("trance", "x-xx-xx-x-xx-xx-"),
];

pub static TRANCE_GATE_SPECS: &[ParamSpec] = &[
    e("pattern", "型", &["8th", "16th", "offbeat", "dotted", "syncopated", "halftime", "trance"], "trance", "16 分 16 ステップのどこで音を通すか。trance はトランスの定番のゲート、offbeat は裏だけ。16th は 16 分ごとに刻み直す(smooth で区切りが聞こえる)。"),
    f("depth", "深さ", None, 0.0, 1.0, 1.0, "閉じたときに下げる量。1 で無音まで。"),
    f("smooth_ms", "なめらかさ", Some("ms"), 0.5, 30.0, 4.0, "開け閉めの速さ。短いと鋭く、長いとふわっと。"),
];

pub static AUTO_FILTER_SPECS: &[ParamSpec] = &[
    e("mode", "種類", &["lowpass", "highpass", "bandpass"], "lowpass", "lowpass は高域を削って暗く、highpass は低域を削って軽く(ビルドの定番)、bandpass はワウのように。"),
    f("cutoff", "カットオフ", Some("Hz"), 30.0, 16000.0, 1000.0, "中心の周波数。"),
    f("resonance", "レゾナンス", None, 0.0, 0.9, 0.3, "カットオフ付近の強調。上げるとピュンと鳴る。"),
    e("sync", "同期", SYNC_CHOICES, "1/4", SYNC_DESC),
    f("rate_hz", "レート", Some("Hz"), 0.02, 20.0, 1.0, "sync が off のときの速さ。"),
    f("depth", "深さ", Some("oct"), 0.0, 6.0, 2.0, "LFO で動かす幅(オクターブ)。0 で止めて cutoff のオートメーションだけで動かす。"),
    e("shape", "形", &["sine", "triangle", "square", "saw_down"], "sine", "動かし方の形。saw_down で音の頭が開いて閉じるワウ風の刻み。"),
    f("env_amount", "音量で開く", Some("oct"), -4.0, 4.0, 0.0, "入ってくる音が大きいほど開く量(エンベロープフォロワー。ファンクのオートワウは bandpass + 2〜3)。"),
];

pub static VOLUME_SHAPER_SPECS: &[ParamSpec] = &[
    e("sync", "周期", SYNC_CHOICES, "1/4", "沈める周期(1/4 = 4 分ごと = 4 つ打ちのキックに合わせたポンピング)。"),
    f("depth_db", "深さ", Some("dB"), 0.0, 36.0, 12.0, "周期の頭で沈める量。ベース・パッドで 6〜12、はっきりポンプさせるなら 18 以上。"),
    e("curve", "戻り方", &["pump", "linear", "gate"], "pump", "pump は素早く沈んでなめらかに戻る(サイドチェインのコンプ風)、linear はまっすぐ戻る、gate は戻る直前まで沈んだまま。"),
    f("release", "戻る長さ", None, 0.05, 1.0, 0.6, "周期のうち戻るまでの割合。"),
];

pub fn specs(name: &str) -> Option<&'static [ParamSpec]> {
    Some(match name {
        "clipper" => CLIPPER_SPECS,
        "bitcrush" => BITCRUSH_SPECS,
        "tremolo" => TREMOLO_SPECS,
        "phaser" => PHASER_SPECS,
        "flanger" => FLANGER_SPECS,
        "trance_gate" => TRANCE_GATE_SPECS,
        "auto_filter" => AUTO_FILTER_SPECS,
        "volume_shaper" => VOLUME_SHAPER_SPECS,
        _ => return None,
    })
}

pub fn catalog() -> Vec<crate::params::InstrumentInfo> {
    let info = |name: &'static str, description: &'static str| crate::params::InstrumentInfo {
        name,
        description,
        params: specs(name).expect("一覧にある"),
        articulations: &[],
    };
    vec![
        info("clipper", "クリッパー。音の頭を潰して音圧と前に出る感じを作る(ドラムバス・ベース・マスターの前)。soft / hard / fold。"),
        info("bitcrush", "ビットクラッシャー。ビットとサンプルレートを落としてザラついたローファイ・ゲーム機の音に。"),
        info("tremolo", "トレモロ / オートパン。音量をテンポに合わせて揺らす(stereo 1 で左右に振る)。エレピ・ギター・パッドの揺れ。"),
        info("phaser", "フェイザー。うねるシュワシュワした揺れ。エレピ・ギター・パッド・ハットに。"),
        info("flanger", "フランジャー。ジェット機のような金属的なうねり。ギター・ドラムのフィル・ビルドに。"),
        info("trance_gate", "トランスゲート。16 分の型で音を刻む(パッド・コードをリズムにする)。テンポに合わせる。"),
        info("auto_filter", "動くフィルター。LFO(テンポ同期)か入ってくる音量でカットオフを動かす。ビルドのハイパス、オートワウ、うねるベース。"),
        info("volume_shaper", "音量シェイパー。周期の頭で沈めて戻す(キックに合わせたポンピングをサイドチェイン無しで)。ベース・パッド・コードに。"),
    ]
}

/// つまみの値から焼き込む
pub fn bake(name: &str, map: &ParamMap, sr: f32) -> Option<ModFxParams> {
    let s = specs(name)?;
    let db = |x: f32| 10.0_f32.powf(x / 20.0);
    Some(match name {
        "clipper" => ModFxParams::Clipper {
            drive: db(get(map, s, "drive_db").clamp(0.0, 24.0)),
            mode: match choice(map, s, "mode") {
                "hard" => ClipMode::Hard,
                "fold" => ClipMode::Fold,
                _ => ClipMode::Soft,
            },
            ceiling: db(get(map, s, "ceiling_db").clamp(-12.0, 0.0)),
            mix: get(map, s, "mix").clamp(0.0, 1.0),
        },
        "bitcrush" => ModFxParams::Bitcrush {
            levels: 2f32.powf(get(map, s, "bits").clamp(1.0, 16.0) - 1.0),
            hold: get(map, s, "downsample").clamp(1.0, 32.0) as u32,
            mix: get(map, s, "mix").clamp(0.0, 1.0),
        },
        "tremolo" => ModFxParams::Tremolo {
            rate: Rate::from(map, s, sr),
            depth: get(map, s, "depth").clamp(0.0, 1.0),
            shape: Shape::parse(choice(map, s, "shape")),
            stereo: get(map, s, "stereo").clamp(0.0, 1.0),
        },
        "phaser" => ModFxParams::Phaser {
            rate: Rate::from(map, s, sr),
            depth: get(map, s, "depth").clamp(0.0, 1.0),
            feedback: get(map, s, "feedback").clamp(0.0, 0.9),
            stages: choice(map, s, "stages").parse().unwrap_or(6),
            center: get(map, s, "center_hz").clamp(200.0, 4000.0),
            mix: get(map, s, "mix").clamp(0.0, 1.0),
            sr,
        },
        "flanger" => ModFxParams::Flanger {
            rate: Rate::from(map, s, sr),
            depth: get(map, s, "depth_ms").clamp(0.0, 8.0) * 0.001 * sr,
            base: get(map, s, "delay_ms").clamp(0.5, 10.0) * 0.001 * sr,
            feedback: get(map, s, "feedback").clamp(-0.9, 0.9),
            mix: get(map, s, "mix").clamp(0.0, 1.0),
        },
        "trance_gate" => {
            let name = choice(map, s, "pattern");
            let pat = GATE_PATTERNS
                .iter()
                .find(|g| g.0 == name)
                .map_or(GATE_PATTERNS[0].1, |g| g.1);
            let mut pattern = [false; 16];
            for (i, c) in pat.chars().take(16).enumerate() {
                pattern[i] = c == 'x';
            }
            let ms = get(map, s, "smooth_ms").clamp(0.5, 30.0);
            ModFxParams::TranceGate {
                pattern,
                step_ticks: glaux_core::PPQ as f64 / 4.0,
                depth: get(map, s, "depth").clamp(0.0, 1.0),
                smooth: 1.0 - (-1.0 / (ms * 0.001 * sr)).exp(),
            }
        }
        "auto_filter" => ModFxParams::AutoFilter {
            mode: match choice(map, s, "mode") {
                "highpass" => FilterMode::High,
                "bandpass" => FilterMode::Band,
                _ => FilterMode::Low,
            },
            cutoff: get(map, s, "cutoff").clamp(30.0, 16000.0),
            resonance: get(map, s, "resonance").clamp(0.0, 0.9),
            rate: Rate::from(map, s, sr),
            depth: get(map, s, "depth").clamp(0.0, 6.0),
            shape: Shape::parse(choice(map, s, "shape")),
            env_amount: get(map, s, "env_amount").clamp(-4.0, 4.0),
            sr,
        },
        "volume_shaper" => ModFxParams::VolumeShaper {
            period_ticks: sync_ticks(choice(map, s, "sync")),
            depth: db(-get(map, s, "depth_db").clamp(0.0, 36.0)),
            curve: match choice(map, s, "curve") {
                "linear" => ShaperCurve::Linear,
                "gate" => ShaperCurve::Gate,
                _ => ShaperCurve::Pump,
            },
            release: get(map, s, "release").clamp(0.05, 1.0),
        },
        _ => return None,
    })
}

impl ModFxParams {
    /// オートメーション: 連続のつまみを上書き(確保しない)
    pub fn set_continuous(&mut self, name: &str, v: f32, sr: f32) -> bool {
        let db = |x: f32| 10.0_f32.powf(x / 20.0);
        match self {
            ModFxParams::Clipper {
                drive,
                ceiling,
                mix,
                ..
            } => match name {
                "drive_db" => *drive = db(v.clamp(0.0, 24.0)),
                "ceiling_db" => *ceiling = db(v.clamp(-12.0, 0.0)),
                "mix" => *mix = v.clamp(0.0, 1.0),
                _ => return false,
            },
            ModFxParams::Bitcrush { levels, hold, mix } => match name {
                "bits" => *levels = 2f32.powf(v.clamp(1.0, 16.0) - 1.0),
                "downsample" => *hold = v.clamp(1.0, 32.0) as u32,
                "mix" => *mix = v.clamp(0.0, 1.0),
                _ => return false,
            },
            ModFxParams::Tremolo {
                rate,
                depth,
                stereo,
                ..
            } => match name {
                "rate_hz" => rate.inc = v.clamp(0.1, 20.0) / sr,
                "depth" => *depth = v.clamp(0.0, 1.0),
                "stereo" => *stereo = v.clamp(0.0, 1.0),
                _ => return false,
            },
            ModFxParams::Phaser {
                rate,
                depth,
                feedback,
                center,
                mix,
                ..
            } => match name {
                "rate_hz" => rate.inc = v.clamp(0.02, 8.0) / sr,
                "depth" => *depth = v.clamp(0.0, 1.0),
                "feedback" => *feedback = v.clamp(0.0, 0.9),
                "center_hz" => *center = v.clamp(200.0, 4000.0),
                "mix" => *mix = v.clamp(0.0, 1.0),
                _ => return false,
            },
            ModFxParams::Flanger {
                rate,
                depth,
                base,
                feedback,
                mix,
            } => match name {
                "rate_hz" => rate.inc = v.clamp(0.02, 5.0) / sr,
                "depth_ms" => *depth = v.clamp(0.0, 8.0) * 0.001 * sr,
                "delay_ms" => *base = v.clamp(0.5, 10.0) * 0.001 * sr,
                "feedback" => *feedback = v.clamp(-0.9, 0.9),
                "mix" => *mix = v.clamp(0.0, 1.0),
                _ => return false,
            },
            ModFxParams::TranceGate { depth, smooth, .. } => match name {
                "depth" => *depth = v.clamp(0.0, 1.0),
                "smooth_ms" => *smooth = 1.0 - (-1.0 / (v.clamp(0.5, 30.0) * 0.001 * sr)).exp(),
                _ => return false,
            },
            ModFxParams::AutoFilter {
                cutoff,
                resonance,
                rate,
                depth,
                env_amount,
                ..
            } => match name {
                "cutoff" => *cutoff = v.clamp(30.0, 16000.0),
                "resonance" => *resonance = v.clamp(0.0, 0.9),
                "rate_hz" => rate.inc = v.clamp(0.02, 20.0) / sr,
                "depth" => *depth = v.clamp(0.0, 6.0),
                "env_amount" => *env_amount = v.clamp(-4.0, 4.0),
                _ => return false,
            },
            ModFxParams::VolumeShaper { depth, release, .. } => match name {
                "depth_db" => *depth = db(-v.clamp(0.0, 36.0)),
                "release" => *release = v.clamp(0.05, 1.0),
                _ => return false,
            },
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, ParamValue)]) -> ParamMap {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect()
    }

    const SR: f32 = 48_000.0;

    fn run(p: &ModFxParams, n: usize, bpm: f64, input: impl Fn(usize) -> f32) -> Vec<(f32, f32)> {
        let mut st = ModFxState::default();
        // 1 サンプルあたりの tick
        st.set_clock(0.0, glaux_core::PPQ as f64 * bpm / 60.0 / SR as f64);
        (0..n).map(|i| st.process(p, input(i), input(i))).collect()
    }

    #[test]
    fn sync_values() {
        assert_eq!(sync_ticks("1/4"), 960.0);
        assert_eq!(sync_ticks("1/8d"), 720.0);
        assert_eq!(sync_ticks("1/8t"), 320.0);
        assert_eq!(sync_ticks("2/1"), 7680.0);
        assert_eq!(sync_ticks("off"), 0.0);
    }

    #[test]
    fn clipper_and_bitcrush() {
        let p = bake(
            "clipper",
            &map(&[("drive_db", ParamValue::Float(24.0))]),
            SR,
        )
        .unwrap();
        let out = run(&p, 10, 120.0, |_| 0.9);
        assert!(out[0].0 <= 1.0 && out[0].0 > 0.9);
        let p = bake(
            "clipper",
            &map(&[
                ("mode", ParamValue::Enum("hard".into())),
                ("drive_db", ParamValue::Float(12.0)),
            ]),
            SR,
        )
        .unwrap();
        assert!(run(&p, 1, 120.0, |_| 0.5)[0].0 <= 0.97);
        let p = bake(
            "bitcrush",
            &map(&[
                ("bits", ParamValue::Float(2.0)),
                ("downsample", ParamValue::Float(4.0)),
            ]),
            SR,
        )
        .unwrap();
        let out = run(&p, 8, 120.0, |i| i as f32 * 0.1);
        // 4 サンプルずつ同じ値、2 ビット(3 段)
        assert_eq!(out[0].0, out[3].0);
        assert!(out[4].0 != out[3].0);
    }

    #[test]
    fn tremolo_and_autopan_follow_the_tempo() {
        // 120 BPM の 1/4 = 0.5 秒周期
        let p = bake(
            "tremolo",
            &map(&[
                ("sync", ParamValue::Enum("1/4".into())),
                ("depth", ParamValue::Float(1.0)),
            ]),
            SR,
        )
        .unwrap();
        let out = run(&p, 24_000, 120.0, |_| 1.0);
        // 0.125 秒(位相 1/4)で最大、0.375 秒(3/4)で最小
        assert!((out[6_000].0 - 1.0).abs() < 0.01);
        assert!(out[18_000].0 < 0.01);
        // オートパン: 左右が逆
        let p = bake(
            "tremolo",
            &map(&[
                ("sync", ParamValue::Enum("1/4".into())),
                ("depth", ParamValue::Float(1.0)),
                ("stereo", ParamValue::Float(1.0)),
            ]),
            SR,
        )
        .unwrap();
        let out = run(&p, 24_000, 120.0, |_| 1.0);
        assert!(out[6_000].0 > 0.99 && out[6_000].1 < 0.01);
    }

    #[test]
    fn trance_gate_and_volume_shaper() {
        // halftime: 1 拍目と 3 拍目の 16 分だけ開く
        let p = bake(
            "trance_gate",
            &map(&[("pattern", ParamValue::Enum("halftime".into()))]),
            SR,
        )
        .unwrap();
        let out = run(&p, 48_000, 120.0, |_| 1.0);
        // 120 BPM の 16 分 = 0.125 秒 = 6000 サンプル
        assert!(out[5_000].0 > 0.99, "{}", out[5_000].0);
        assert!(out[9_000].0 < 0.01);
        // ポンピング: 4 分の頭で沈み、戻る
        let p = bake(
            "volume_shaper",
            &map(&[("depth_db", ParamValue::Float(20.0))]),
            SR,
        )
        .unwrap();
        let out = run(&p, 24_000, 120.0, |_| 1.0);
        assert!(out[10].0 < 0.2);
        assert!(out[20_000].0 > 0.99);
    }

    #[test]
    fn phaser_flanger_and_auto_filter_change_the_sound() {
        let noise = |i: usize| {
            (((i as u32).wrapping_mul(2_654_435_761) >> 8) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0
        };
        let energy = |v: &[(f32, f32)]| v.iter().map(|x| x.0 * x.0).sum::<f32>();
        let dry = energy(
            &(0..48_000)
                .map(|i| (noise(i), noise(i)))
                .collect::<Vec<_>>(),
        );
        for name in ["phaser", "flanger", "auto_filter"] {
            let p = bake(name, &ParamMap::new(), SR).unwrap();
            let out = run(&p, 48_000, 120.0, noise);
            let e = energy(&out);
            assert!(e.is_finite() && e > 0.0, "{name}");
            assert!((e - dry).abs() / dry > 0.05, "{name}: {e} {dry}");
        }
        // flanger はバッファが無ければ素通し
        let p = bake("flanger", &ParamMap::new(), SR).unwrap();
        let mut st = ModFxState::light();
        assert_eq!(st.process(&p, 0.5, 0.25), (0.5, 0.25));
    }
}
