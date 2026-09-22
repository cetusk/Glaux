//! 内蔵エフェクト: `eq` / `compressor` / `reverb`。
//!
//! - パラメータはデータ構築時(UI スレッド)に**係数まで焼き込む**([`bake_effect`])。
//!   オーディオスレッドは焼き込み済みの [`EffectParams`] を読むだけ
//! - 状態([`EffectState`])はエンジンが起動時にプール確保する。リバーブの
//!   ディレイバッファ込みで、オーディオスレッドでのアロケーションはない
//! - 処理はステレオ 1 サンプルずつ(`process`)

use glaux_core::{ParamMap, ParamRange, ParamSpec, ParamValue};

// ============================== EQ =====================================

/// biquad 係数(1 バンド分)。
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct BiquadCoeffs {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl BiquadCoeffs {
    pub fn identity() -> Self {
        BiquadCoeffs {
            b0: 1.0,
            ..Default::default()
        }
    }

    /// RBJ cookbook: low shelf
    pub fn low_shelf(sr: f32, freq: f32, gain_db: f32) -> Self {
        Self::shelf(sr, freq, gain_db, true)
    }

    /// RBJ cookbook: high shelf
    pub fn high_shelf(sr: f32, freq: f32, gain_db: f32) -> Self {
        Self::shelf(sr, freq, gain_db, false)
    }

    fn shelf(sr: f32, freq: f32, gain_db: f32, low: bool) -> Self {
        let a = 10.0_f32.powf(gain_db / 40.0);
        let w0 = std::f32::consts::TAU * (freq / sr).clamp(0.0001, 0.49);
        let (sin, cos) = w0.sin_cos();
        let s = 1.0f32; // shelf slope
        let alpha = sin / 2.0 * ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).sqrt();
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
        let sign = if low { 1.0 } else { -1.0 };

        let b0 = a * ((a + 1.0) - sign * (a - 1.0) * cos + two_sqrt_a_alpha);
        let b1 = sign * 2.0 * a * ((a - 1.0) - sign * (a + 1.0) * cos);
        let b2 = a * ((a + 1.0) - sign * (a - 1.0) * cos - two_sqrt_a_alpha);
        let a0 = (a + 1.0) + sign * (a - 1.0) * cos + two_sqrt_a_alpha;
        let a1 = sign * -2.0 * ((a - 1.0) + sign * (a + 1.0) * cos);
        let a2 = (a + 1.0) + sign * (a - 1.0) * cos - two_sqrt_a_alpha;
        BiquadCoeffs {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    /// RBJ cookbook: peaking EQ
    pub fn peaking(sr: f32, freq: f32, q: f32, gain_db: f32) -> Self {
        let a = 10.0_f32.powf(gain_db / 40.0);
        let w0 = std::f32::consts::TAU * (freq / sr).clamp(0.0001, 0.49);
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q.max(0.1));
        let a0 = 1.0 + alpha / a;
        BiquadCoeffs {
            b0: (1.0 + alpha * a) / a0,
            b1: (-2.0 * cos) / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: (-2.0 * cos) / a0,
            a2: (1.0 - alpha / a) / a0,
        }
    }
}

/// EQ の焼き込み済み係数(3 バンド)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EqParams {
    pub low: BiquadCoeffs,
    pub mid: BiquadCoeffs,
    pub high: BiquadCoeffs,
}

#[derive(Clone, Copy, Debug, Default)]
struct BiquadState {
    x: [f32; 2],
    y: [f32; 2],
}

impl BiquadState {
    fn next(&mut self, c: &BiquadCoeffs, x0: f32) -> f32 {
        let y0 =
            c.b0 * x0 + c.b1 * self.x[0] + c.b2 * self.x[1] - c.a1 * self.y[0] - c.a2 * self.y[1];
        self.x = [x0, self.x[0]];
        self.y = [y0, self.y[0]];
        y0
    }
}

// =========================== Compressor ================================

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompressorParams {
    pub threshold_db: f32,
    pub ratio: f32,
    /// 1 サンプルあたりのエンベロープ追従係数(attack)
    pub attack_coef: f32,
    pub release_coef: f32,
    pub makeup: f32,
}

// ============================= Reverb ==================================

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReverbParams {
    /// ウェット比率 0..=1
    pub mix: f32,
    /// コムフィードバック 0.7..=0.98 に写像済み
    pub feedback: f32,
    /// 高域減衰 0..=1
    pub damping: f32,
}

/// コムフィルタのディレイ長(48kHz 基準、Freeverb 由来の素数付近)。
const COMB_LENS: [usize; 4] = [1557, 1617, 1491, 1422];
const ALLPASS_LENS: [usize; 2] = [225, 556];
/// 右チャンネルはディレイをずらしてステレオ感を出す
const STEREO_SPREAD: usize = 23;
const MAX_COMB: usize = 1617 + STEREO_SPREAD;
const MAX_ALLPASS: usize = 556 + STEREO_SPREAD;

#[derive(Clone)]
struct ReverbChannel {
    combs: [Vec<f32>; 4],
    comb_lp: [f32; 4],
    comb_idx: [usize; 4],
    allpasses: [Vec<f32>; 2],
    ap_idx: [usize; 2],
    spread: usize,
}

impl ReverbChannel {
    fn new(spread: usize) -> Self {
        ReverbChannel {
            combs: std::array::from_fn(|_| vec![0.0; MAX_COMB]),
            comb_lp: [0.0; 4],
            comb_idx: [0; 4],
            allpasses: std::array::from_fn(|_| vec![0.0; MAX_ALLPASS]),
            ap_idx: [0; 2],
            spread,
        }
    }

    fn reset(&mut self) {
        for c in &mut self.combs {
            c.fill(0.0);
        }
        for a in &mut self.allpasses {
            a.fill(0.0);
        }
        self.comb_lp = [0.0; 4];
    }

    fn next(&mut self, p: &ReverbParams, x: f32) -> f32 {
        let mut wet = 0.0;
        for (i, base_len) in COMB_LENS.iter().enumerate() {
            let len = base_len + self.spread;
            let idx = self.comb_idx[i];
            let out = self.combs[i][idx];
            // フィードバック経路の一次ローパス(damping)
            self.comb_lp[i] = out * (1.0 - p.damping) + self.comb_lp[i] * p.damping;
            self.combs[i][idx] = x + self.comb_lp[i] * p.feedback;
            self.comb_idx[i] = (idx + 1) % len;
            wet += out;
        }
        wet *= 0.25;
        for (i, base_len) in ALLPASS_LENS.iter().enumerate() {
            let len = base_len + self.spread;
            let idx = self.ap_idx[i];
            let buf = self.allpasses[i][idx];
            let out = -wet + buf;
            self.allpasses[i][idx] = wet + buf * 0.5;
            self.ap_idx[i] = (idx + 1) % len;
            wet = out;
        }
        wet
    }
}

// =========================== Distortion ================================

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DistortionParams {
    /// プリゲイン(リニア。drive_db から変換済み)
    pub drive: f32,
    /// 歪み後のトーン用ローパス係数(0..1、1 に近いほど暗い)
    pub tone_coef: f32,
    /// ウェット比率 0..=1
    pub mix: f32,
    /// 出力レベル(リニア)
    pub level: f32,
}

// ========================== Sidechain Comp =============================

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SidechainParams {
    /// 検出信号にするトラックの index(構築時に ID から解決済み)。
    /// u32::MAX なら未解決(ダッキングしない)
    pub source_track: u32,
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_coef: f32,
    pub release_coef: f32,
}

// ======================= 統合(定義と状態) =============================

/// 焼き込み済みのエフェクト定義(オーディオスレッドは読むだけ)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EffectParams {
    Eq(EqParams),
    Compressor(CompressorParams),
    Reverb(ReverbParams),
    Distortion(DistortionParams),
    Sidechain(SidechainParams),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EffectKind {
    None,
    Eq,
    Compressor,
    Reverb,
    Distortion,
    Sidechain,
}

/// エフェクト 1 スロット分の状態。全種類のバッファを持ち、起動時に確保して使い回す。
#[derive(Clone)]
pub struct EffectState {
    kind: EffectKind,
    // EQ: 3 バンド × 2ch
    eq: [[BiquadState; 3]; 2],
    // Compressor / Sidechain
    envelope: f32,
    // Distortion のトーン用 1 次 LP(2ch)
    tone_lp: [f32; 2],
    // Reverb
    reverb: [ReverbChannel; 2],
}

impl Default for EffectState {
    fn default() -> Self {
        EffectState {
            kind: EffectKind::None,
            eq: Default::default(),
            envelope: 0.0,
            tone_lp: [0.0; 2],
            reverb: [ReverbChannel::new(0), ReverbChannel::new(STEREO_SPREAD)],
        }
    }
}

impl EffectState {
    fn kind_of(p: &EffectParams) -> EffectKind {
        match p {
            EffectParams::Eq(_) => EffectKind::Eq,
            EffectParams::Compressor(_) => EffectKind::Compressor,
            EffectParams::Reverb(_) => EffectKind::Reverb,
            EffectParams::Distortion(_) => EffectKind::Distortion,
            EffectParams::Sidechain(_) => EffectKind::Sidechain,
        }
    }

    /// データ差し替えでスロットの中身が変わったときに呼ぶ(アロケーションなし)。
    pub fn ensure_kind(&mut self, p: &EffectParams) {
        let kind = Self::kind_of(p);
        if self.kind != kind {
            self.kind = kind;
            self.eq = Default::default();
            self.envelope = 0.0;
            self.tone_lp = [0.0; 2];
            self.reverb[0].reset();
            self.reverb[1].reset();
        }
    }

    /// ステレオ 1 サンプル処理。`key` はサイドチェインの検出信号
    /// (通常はソーストラックのモノ合算。サイドチェイン以外は無視する)。
    pub fn process(&mut self, p: &EffectParams, l: f32, r: f32, key: f32) -> (f32, f32) {
        match p {
            EffectParams::Eq(eq) => {
                let ch = |s: f32, st: &mut [BiquadState; 3]| {
                    let s = st[0].next(&eq.low, s);
                    let s = st[1].next(&eq.mid, s);
                    st[2].next(&eq.high, s)
                };
                let [sl, sr] = &mut self.eq;
                (ch(l, sl), ch(r, sr))
            }
            EffectParams::Compressor(c) => {
                let level = l.abs().max(r.abs());
                let coef = if level > self.envelope {
                    c.attack_coef
                } else {
                    c.release_coef
                };
                self.envelope += (level - self.envelope) * coef;
                let level_db = 20.0 * self.envelope.max(1e-6).log10();
                let over = level_db - c.threshold_db;
                let gain_db = if over > 0.0 {
                    -over * (1.0 - 1.0 / c.ratio)
                } else {
                    0.0
                };
                let gain = 10.0_f32.powf(gain_db / 20.0) * c.makeup;
                (l * gain, r * gain)
            }
            EffectParams::Reverb(rv) => {
                let input = (l + r) * 0.35;
                let wl = self.reverb[0].next(rv, input);
                let wr = self.reverb[1].next(rv, input);
                (
                    l * (1.0 - rv.mix) + wl * rv.mix,
                    r * (1.0 - rv.mix) + wr * rv.mix,
                )
            }
            EffectParams::Distortion(d) => {
                let mut shape = |x: f32, ch: usize| {
                    let wet = (x * d.drive).tanh();
                    // 歪みで出た高域のギラつきをトーンで丸める(1 次 LP)
                    self.tone_lp[ch] += (wet - self.tone_lp[ch]) * (1.0 - d.tone_coef);
                    let toned = self.tone_lp[ch];
                    (x * (1.0 - d.mix) + toned * d.mix) * d.level
                };
                (shape(l, 0), shape(r, 1))
            }
            EffectParams::Sidechain(sc) => {
                let level = key.abs();
                let coef = if level > self.envelope {
                    sc.attack_coef
                } else {
                    sc.release_coef
                };
                self.envelope += (level - self.envelope) * coef;
                let level_db = 20.0 * self.envelope.max(1e-6).log10();
                let over = level_db - sc.threshold_db;
                let gain_db = if over > 0.0 {
                    -over * (1.0 - 1.0 / sc.ratio)
                } else {
                    0.0
                };
                let gain = 10.0_f32.powf(gain_db / 20.0);
                (l * gain, r * gain)
            }
        }
    }
}

// ====================== ParamSpec とベイク =============================

pub static EQ_SPECS: &[ParamSpec] = &[
    ParamSpec {
        name: "low_gain_db",
        display_name: "低域ゲイン",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: -15.0,
            max: 15.0,
            default: 0.0,
            skew: None,
        },
        description: "低域シェルフ(low_freq 以下)。上げると太く重く、下げるとすっきり軽くなる。\
            こもりを取るときはまずここを下げる。",
    },
    ParamSpec {
        name: "low_freq",
        display_name: "低域周波数",
        unit: Some("Hz"),
        range: ParamRange::Float {
            min: 40.0,
            max: 500.0,
            default: 200.0,
            skew: Some(0.5),
        },
        description: "低域シェルフの境界。",
    },
    ParamSpec {
        name: "mid_gain_db",
        display_name: "中域ゲイン",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: -15.0,
            max: 15.0,
            default: 0.0,
            skew: None,
        },
        description: "mid_freq を中心としたピーク/ディップ。ボーカルやリードの存在感は\
            1〜3kHz、モコモコ感は 300〜500Hz を下げると解消しやすい。",
    },
    ParamSpec {
        name: "mid_freq",
        display_name: "中域周波数",
        unit: Some("Hz"),
        range: ParamRange::Float {
            min: 200.0,
            max: 6000.0,
            default: 1000.0,
            skew: Some(0.3),
        },
        description: "中域ピークの中心周波数。",
    },
    ParamSpec {
        name: "mid_q",
        display_name: "中域 Q",
        unit: None,
        range: ParamRange::Float {
            min: 0.3,
            max: 4.0,
            default: 1.0,
            skew: None,
        },
        description: "中域ピークの幅。大きいほど狭くピンポイント。",
    },
    ParamSpec {
        name: "high_gain_db",
        display_name: "高域ゲイン",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: -15.0,
            max: 15.0,
            default: 0.0,
            skew: None,
        },
        description: "高域シェルフ(high_freq 以上)。上げると明るく空気感が出て、\
            下げると刺さりやシャリつきが収まる。",
    },
    ParamSpec {
        name: "high_freq",
        display_name: "高域周波数",
        unit: Some("Hz"),
        range: ParamRange::Float {
            min: 2000.0,
            max: 12000.0,
            default: 5000.0,
            skew: Some(0.5),
        },
        description: "高域シェルフの境界。",
    },
];

pub static COMPRESSOR_SPECS: &[ParamSpec] = &[
    ParamSpec {
        name: "threshold_db",
        display_name: "スレッショルド",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: -40.0,
            max: 0.0,
            default: -18.0,
            skew: None,
        },
        description: "これを超えた音量を圧縮し始める。下げるほど強くかかる。",
    },
    ParamSpec {
        name: "ratio",
        display_name: "レシオ",
        unit: None,
        range: ParamRange::Float {
            min: 1.0,
            max: 20.0,
            default: 4.0,
            skew: Some(0.5),
        },
        description: "圧縮の強さ。2〜4 で自然に音量を揃え、8 以上でパツパツに潰れた質感になる。",
    },
    ParamSpec {
        name: "attack_ms",
        display_name: "アタック",
        unit: Some("ms"),
        range: ParamRange::Float {
            min: 0.1,
            max: 100.0,
            default: 10.0,
            skew: Some(0.3),
        },
        description: "圧縮が効き始めるまでの時間。短いとアタックまで潰れて丸くなり、\
            長いと頭のアタック感を残したまま胴だけ潰せる(ドラムのパンチ)。",
    },
    ParamSpec {
        name: "release_ms",
        display_name: "リリース",
        unit: Some("ms"),
        range: ParamRange::Float {
            min: 10.0,
            max: 1000.0,
            default: 150.0,
            skew: Some(0.3),
        },
        description: "圧縮が戻るまでの時間。短いとポンピング(うねり)、長いと滑らか。",
    },
    ParamSpec {
        name: "makeup_db",
        display_name: "メイクアップゲイン",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: 0.0,
            max: 24.0,
            default: 0.0,
            skew: None,
        },
        description: "圧縮で下がった分の音量を持ち上げる。",
    },
];

pub static REVERB_SPECS: &[ParamSpec] = &[
    ParamSpec {
        name: "mix",
        display_name: "ミックス",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 1.0,
            default: 0.25,
            skew: None,
        },
        description: "残響の混ぜ具合。0.1〜0.2 でさりげない空間、0.4 以上で夢見心地。\
            ドラムやベースに深くかけるとミックスが濁るので控えめに。",
    },
    ParamSpec {
        name: "size",
        display_name: "サイズ",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 1.0,
            default: 0.5,
            skew: None,
        },
        description: "空間の広さ(残響の長さ)。小さいと部屋、大きいとホール。",
    },
    ParamSpec {
        name: "damping",
        display_name: "ダンピング",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 1.0,
            default: 0.5,
            skew: None,
        },
        description: "残響の高域の減衰。上げると暗く柔らかい残響、下げるとキラキラ響く。",
    },
];

pub static DISTORTION_SPECS: &[ParamSpec] = &[
    ParamSpec {
        name: "drive_db",
        display_name: "ドライブ",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: 0.0,
            max: 40.0,
            default: 12.0,
            skew: None,
        },
        description: "歪みの深さ。6〜12 で温かいサチュレーション、18〜30 でロックの歪み、\
            30 以上でメタル・激歪み。ギター系は square 波 + 高ドライブが定番。",
    },
    ParamSpec {
        name: "tone",
        display_name: "トーン",
        unit: Some("Hz"),
        range: ParamRange::Float {
            min: 500.0,
            max: 12000.0,
            default: 4500.0,
            skew: Some(0.4),
        },
        description: "歪み後のざらつきを丸めるローパス。下げると太く暗い歪み、\
            上げるとジャリっと明るい歪み。",
    },
    ParamSpec {
        name: "mix",
        display_name: "ミックス",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 1.0,
            default: 1.0,
            skew: None,
        },
        description: "原音と歪みの混合。1.0 で完全に歪ませ、0.3〜0.6 で原音の芯を残す\
            パラレルディストーション。",
    },
    ParamSpec {
        name: "level_db",
        display_name: "レベル",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: -24.0,
            max: 6.0,
            default: -6.0,
            skew: None,
        },
        description: "出力音量。歪ませると音圧が上がるのでここで戻す。",
    },
];

pub static SIDECHAIN_SPECS: &[ParamSpec] = &[
    ParamSpec {
        name: "source",
        display_name: "ソーストラック",
        unit: None,
        range: ParamRange::Enum {
            choices: &[],
            default: "",
        },
        description: "検出信号にするトラックの ID(trk_xxxxxx)。通常はキックの\
            ドラムトラックを指定する。空のままだと何もしない。\
            例: set_param path=fx/<fx_id>/source value=\"trk_drm001\"",
    },
    ParamSpec {
        name: "threshold_db",
        display_name: "スレッショルド",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: -50.0,
            max: 0.0,
            default: -30.0,
            skew: None,
        },
        description: "ソースがこれを超えたときに沈み込む。低いほど敏感。",
    },
    ParamSpec {
        name: "ratio",
        display_name: "レシオ",
        unit: None,
        range: ParamRange::Float {
            min: 1.0,
            max: 20.0,
            default: 6.0,
            skew: Some(0.5),
        },
        description: "沈み込みの深さ。4〜8 で心地よいポンピング、それ以上でガッツリ潜る。",
    },
    ParamSpec {
        name: "attack_ms",
        display_name: "アタック",
        unit: Some("ms"),
        range: ParamRange::Float {
            min: 0.1,
            max: 50.0,
            default: 5.0,
            skew: Some(0.3),
        },
        description: "沈み始める速さ。短いほどキックの頭がクッキリ抜ける。",
    },
    ParamSpec {
        name: "release_ms",
        display_name: "リリース",
        unit: Some("ms"),
        range: ParamRange::Float {
            min: 20.0,
            max: 500.0,
            default: 120.0,
            skew: Some(0.3),
        },
        description: "浮き上がってくる速さ。EDM のポンピング感はここで決まる。\
            テンポに合わせて 80〜200ms あたりを探ると気持ちよい。",
    },
];

pub fn effect_params_spec(name: &str) -> Option<&'static [ParamSpec]> {
    match name {
        "eq" => Some(EQ_SPECS),
        "compressor" => Some(COMPRESSOR_SPECS),
        "reverb" => Some(REVERB_SPECS),
        "distortion" => Some(DISTORTION_SPECS),
        "sidechain" => Some(SIDECHAIN_SPECS),
        _ => None,
    }
}

/// エフェクトカタログ(MCP `list_params` 用)。
pub fn effect_catalog() -> Vec<crate::params::InstrumentInfo> {
    vec![
        crate::params::InstrumentInfo {
            name: "eq",
            description: "3 バンド EQ(低域シェルフ + 中域ピーク + 高域シェルフ)。\
                帯域バランスの調整に使う。analyze_audio の band_energy と組み合わせると効果的。",
            params: EQ_SPECS,
        },
        crate::params::InstrumentInfo {
            name: "compressor",
            description: "コンプレッサー。音量のばらつきを揃え、音圧や密度を上げる。\
                かけすぎ(crest_factor が 6dB 以下)に注意。",
            params: COMPRESSOR_SPECS,
        },
        crate::params::InstrumentInfo {
            name: "reverb",
            description: "リバーブ(残響)。奥行きと空間を作る。パッドやリードに薄く\
                かけると馴染む。低音楽器には控えめに。",
            params: REVERB_SPECS,
        },
        crate::params::InstrumentInfo {
            name: "distortion",
            description: "ディストーション/サチュレーション。ギターの歪み、EDM の\
                荒い質感、ドラムの太さ足しに。subtractive(square 波)+ 高 drive で\
                エレキギター風になる。",
            params: DISTORTION_SPECS,
        },
        crate::params::InstrumentInfo {
            name: "sidechain",
            description: "サイドチェインコンプ。別トラック(通常キック)が鳴った瞬間に\
                このトラックを沈み込ませる。EDM のポンピング/ビートダウンの要。\
                ベースやパッドに挿し、source にキックのトラック ID を設定して使う。",
            params: SIDECHAIN_SPECS,
        },
    ]
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

/// `Effect`(builtin)を焼き込み済み定義に変換する。未知の名前は None。
/// `resolve_track` はサイドチェインの source(トラック ID 文字列)を index に引く
/// (エンジンがプロジェクトを知っているので、そちらから渡してもらう)。
pub fn bake_effect(
    effect: &glaux_core::Effect,
    sample_rate: f32,
    resolve_track: &dyn Fn(&str) -> Option<u32>,
) -> Option<EffectParams> {
    let glaux_core::PluginSource::Builtin { name } = &effect.source else {
        return None;
    };
    let map = &effect.params;
    match name.as_str() {
        "eq" => {
            let s = EQ_SPECS;
            Some(EffectParams::Eq(EqParams {
                low: BiquadCoeffs::low_shelf(
                    sample_rate,
                    get(map, s, "low_freq"),
                    get(map, s, "low_gain_db").clamp(-15.0, 15.0),
                ),
                mid: BiquadCoeffs::peaking(
                    sample_rate,
                    get(map, s, "mid_freq"),
                    get(map, s, "mid_q"),
                    get(map, s, "mid_gain_db").clamp(-15.0, 15.0),
                ),
                high: BiquadCoeffs::high_shelf(
                    sample_rate,
                    get(map, s, "high_freq"),
                    get(map, s, "high_gain_db").clamp(-15.0, 15.0),
                ),
            }))
        }
        "compressor" => {
            let s = COMPRESSOR_SPECS;
            let coef = |ms: f32| 1.0 - (-1.0 / (ms.max(0.1) * 0.001 * sample_rate)).exp();
            Some(EffectParams::Compressor(CompressorParams {
                threshold_db: get(map, s, "threshold_db").clamp(-40.0, 0.0),
                ratio: get(map, s, "ratio").clamp(1.0, 20.0),
                attack_coef: coef(get(map, s, "attack_ms")),
                release_coef: coef(get(map, s, "release_ms")),
                makeup: 10.0_f32.powf(get(map, s, "makeup_db").clamp(0.0, 24.0) / 20.0),
            }))
        }
        "reverb" => {
            let s = REVERB_SPECS;
            Some(EffectParams::Reverb(ReverbParams {
                mix: get(map, s, "mix").clamp(0.0, 1.0),
                feedback: 0.7 + get(map, s, "size").clamp(0.0, 1.0) * 0.28,
                damping: get(map, s, "damping").clamp(0.0, 1.0),
            }))
        }
        "distortion" => {
            let s = DISTORTION_SPECS;
            let tone_hz = get(map, s, "tone").clamp(500.0, 12000.0);
            Some(EffectParams::Distortion(DistortionParams {
                drive: 10.0_f32.powf(get(map, s, "drive_db").clamp(0.0, 40.0) / 20.0),
                // 1 次 LP: coef = exp(-2π fc / sr)
                tone_coef: (-std::f32::consts::TAU * tone_hz / sample_rate).exp(),
                mix: get(map, s, "mix").clamp(0.0, 1.0),
                level: 10.0_f32.powf(get(map, s, "level_db").clamp(-24.0, 6.0) / 20.0),
            }))
        }
        "sidechain" => {
            let s = SIDECHAIN_SPECS;
            let coef = |ms: f32| 1.0 - (-1.0 / (ms.max(0.1) * 0.001 * sample_rate)).exp();
            let source_track = match map.get("source") {
                Some(ParamValue::Enum(id)) if !id.is_empty() => {
                    resolve_track(id).unwrap_or(u32::MAX)
                }
                _ => u32::MAX,
            };
            Some(EffectParams::Sidechain(SidechainParams {
                source_track,
                threshold_db: get(map, s, "threshold_db").clamp(-50.0, 0.0),
                ratio: get(map, s, "ratio").clamp(1.0, 20.0),
                attack_coef: coef(get(map, s, "attack_ms")),
                release_coef: coef(get(map, s, "release_ms")),
            }))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::{Effect, FxId};

    fn effect(name: &str, params: &[(&str, f64)]) -> Effect {
        let mut e = Effect::builtin(FxId::new(), name);
        for (k, v) in params {
            e.params.insert((*k).to_owned(), (*v).into());
        }
        e
    }

    fn bake(e: &Effect) -> Option<EffectParams> {
        bake_effect(e, 48_000.0, &|_| None)
    }

    /// 440Hz サイン波を通した RMS を測る
    fn rms_through(p: &EffectParams, freq: f32, samples: usize) -> f32 {
        let mut state = EffectState::default();
        state.ensure_kind(p);
        let mut sum = 0.0f64;
        let mut measured = 0usize;
        for i in 0..samples {
            let x = (i as f32 * freq * std::f32::consts::TAU / 48_000.0).sin() * 0.5;
            let (l, _) = state.process(p, x, x, 0.0);
            // フィルタが落ち着いてから測る
            if i > samples / 2 {
                sum += (l as f64) * (l as f64);
                measured += 1;
            }
        }
        ((sum / measured as f64) as f32).sqrt()
    }

    #[test]
    fn eq_low_cut_reduces_low_and_keeps_high() {
        let p = bake(&effect("eq", &[("low_gain_db", -12.0)])).unwrap();
        let low = rms_through(&p, 100.0, 9600);
        let high = rms_through(&p, 4000.0, 9600);
        let flat = bake(&effect("eq", &[])).unwrap();
        let low_flat = rms_through(&flat, 100.0, 9600);
        assert!(
            low < low_flat * 0.5,
            "低域が下がるはず: {low} vs {low_flat}"
        );
        assert!(high > 0.2, "高域はほぼ素通しのはず: {high}");
    }

    #[test]
    fn compressor_reduces_loud_signal() {
        let p = bake(&effect(
            "compressor",
            &[("threshold_db", -20.0), ("ratio", 8.0)],
        ))
        .unwrap();
        let loud = rms_through(&p, 440.0, 9600); // 0.5 amp ≒ -9dB(スレッショルド超え)
        let bypass_rms = 0.5 / 2.0_f32.sqrt();
        assert!(loud < bypass_rms * 0.7, "圧縮で音量が下がるはず: {loud}");
    }

    #[test]
    fn reverb_produces_tail() {
        let p = bake(&effect("reverb", &[("mix", 0.5)])).unwrap();
        let mut state = EffectState::default();
        state.ensure_kind(&p);
        // インパルスを入れて、その後の無音区間に残響が出るか
        let (_, _) = state.process(&p, 1.0, 1.0, 0.0);
        let mut tail = 0.0f32;
        for _ in 0..48_000 {
            let (l, r) = state.process(&p, 0.0, 0.0, 0.0);
            tail = tail.max(l.abs()).max(r.abs());
        }
        assert!(tail > 0.01, "残響が出るはず: {tail}");
    }

    #[test]
    fn unknown_effect_is_none_and_catalog_resolves() {
        assert!(bake(&effect("no_such_fx", &[])).is_none());
        for info in effect_catalog() {
            assert!(effect_params_spec(info.name).is_some());
        }
    }

    #[test]
    fn distortion_adds_harmonics_and_level_controls_output() {
        let clean = bake(&effect(
            "distortion",
            &[("drive_db", 0.0), ("level_db", 0.0)],
        ))
        .unwrap();
        let driven = bake(&effect(
            "distortion",
            &[("drive_db", 30.0), ("level_db", 0.0)],
        ))
        .unwrap();
        // 深く歪ませると波形が矩形波化 → RMS が上がる(サインの 0.707 倍 → 1.0 に近づく)
        let clean_rms = rms_through(&clean, 220.0, 9600);
        let driven_rms = rms_through(&driven, 220.0, 9600);
        assert!(
            driven_rms > clean_rms * 1.15,
            "clean={clean_rms} driven={driven_rms}"
        );
        // level で出力を絞れる
        let quiet = bake(&effect(
            "distortion",
            &[("drive_db", 30.0), ("level_db", -18.0)],
        ))
        .unwrap();
        assert!(rms_through(&quiet, 220.0, 9600) < driven_rms * 0.3);
    }

    #[test]
    fn sidechain_ducks_only_when_key_is_loud() {
        let p = bake(&effect(
            "sidechain",
            &[
                ("threshold_db", -30.0),
                ("ratio", 10.0),
                ("release_ms", 50.0),
            ],
        ))
        .unwrap();
        let mut state = EffectState::default();
        state.ensure_kind(&p);
        // key が無音 → 素通し
        let mut through = 0.0f32;
        for _ in 0..4800 {
            let (l, _) = state.process(&p, 0.5, 0.5, 0.0);
            through = through.max(l.abs());
        }
        assert!((through - 0.5).abs() < 1e-3, "key 無音では変化しないはず");
        // key が大きい → 沈む
        let mut ducked = f32::MAX;
        for _ in 0..4800 {
            let (l, _) = state.process(&p, 0.5, 0.5, 0.9);
            ducked = ducked.min(l.abs());
        }
        assert!(ducked < 0.25, "key が鳴ると沈むはず: {ducked}");
    }

    #[test]
    fn ensure_kind_resets_state_between_kinds() {
        let rv = bake(&effect("reverb", &[("mix", 1.0)])).unwrap();
        let mut state = EffectState::default();
        state.ensure_kind(&rv);
        state.process(&rv, 1.0, 1.0, 0.0);
        // 別種に切り替え → リバーブへ戻してもバッファはリセット済み
        let eq = bake(&effect("eq", &[])).unwrap();
        state.ensure_kind(&eq);
        state.ensure_kind(&rv);
        let mut tail = 0.0f32;
        for _ in 0..4800 {
            let (l, _) = state.process(&rv, 0.0, 0.0, 0.0);
            tail = tail.max(l.abs());
        }
        assert!(tail < 1e-6, "リセット後は残響が残らないはず: {tail}");
    }
}
