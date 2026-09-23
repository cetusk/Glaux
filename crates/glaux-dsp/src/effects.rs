//! 内蔵エフェクト: `eq` / `compressor` / `reverb` / `distortion` / `amp` / `sidechain` /
//! `delay` / `chorus` / `tape`。
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
    /// 係数を計算し直すための生の値(オートメーション用)
    pub raw: EqRaw,
}

/// EQ の生の値(周波数 Hz・ゲイン dB・Q)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EqRaw {
    pub low_freq: f32,
    pub low_gain_db: f32,
    pub mid_freq: f32,
    pub mid_q: f32,
    pub mid_gain_db: f32,
    pub high_freq: f32,
    pub high_gain_db: f32,
}

impl EqParams {
    fn from_raw(sample_rate: f32, r: EqRaw) -> EqParams {
        EqParams {
            low: BiquadCoeffs::low_shelf(sample_rate, r.low_freq, r.low_gain_db.clamp(-15.0, 15.0)),
            mid: BiquadCoeffs::peaking(
                sample_rate,
                r.mid_freq,
                r.mid_q,
                r.mid_gain_db.clamp(-15.0, 15.0),
            ),
            high: BiquadCoeffs::high_shelf(
                sample_rate,
                r.high_freq,
                r.high_gain_db.clamp(-15.0, 15.0),
            ),
            raw: r,
        }
    }
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

// =============================== Amp ===================================

/// ギターアンプシミュレータ。
/// pluck(エレキ)は「アンプに繋いでいない生の弦」しか出さない。エレキの音の
/// 大半はアンプ側(大ゲインの多段クリップ + キャビネットの箱鳴り)が作るので、
/// それを 1 エフェクトとして再現する。distortion がペダル 1 個ぶんの軽い歪み
/// なのに対し、こちらは 54dB 級のプリゲインを持つ。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AmpParams {
    /// プリゲイン(リニア。gain_db から変換済み)
    pub gain: f32,
    /// 段間 LP の係数(クリップ段の間で高域を丸め、フィジーさを抑える)
    pub stage_coef: f32,
    /// 歪み後トーン LP の係数(exp(-2πfc/sr)。1 に近いほど暗い)
    pub tone_coef: f32,
    /// プレゼンス量 0..1(3kHz 以上のエッジ)
    pub presence: f32,
    /// プレゼンス分離用 LP の係数
    pub pres_coef: f32,
    /// キャビネットシミュ有効
    pub cab: bool,
    /// キャビ HP(約 80Hz)の帰還係数
    pub cab_hp_r: f32,
    /// キャビ LP(約 4.2kHz、3 段 = 18dB/oct)の係数
    pub cab_lp_coef: f32,
    /// DC ブロッカの帰還係数(非対称クリップで生じる直流を除く)
    pub dc_r: f32,
    /// 出力レベル(リニア)
    pub level: f32,
}

/// アンプ 1ch 分のフィルタ状態。
#[derive(Clone, Copy, Default)]
struct AmpChState {
    dc_in: f32,
    dc_out: f32,
    stage_lp: f32,
    tone_lp: f32,
    pres_lp: f32,
    cab_hp_in: f32,
    cab_hp_out: f32,
    cab_lp1: f32,
    cab_lp2: f32,
    cab_lp3: f32,
}

// ========================== Sidechain Comp =============================

/// ダッカー型サイドチェイン。
/// レベル追従型だとキックの胴鳴りの間ずっと沈み「揺れ」にならないため、
/// キックの立ち上がりをトリガーに固定形状のエンベロープで沈む方式にしている。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SidechainParams {
    /// 検出信号にするトラックの index(構築時に ID から解決済み)。
    /// u32::MAX なら未解決(ダッキングしない)
    pub source_track: u32,
    /// トリガーしきい値(リニア振幅)
    pub threshold: f32,
    /// 沈み込みの底のゲイン(リニア。duck_db から変換済み)
    pub duck_floor: f32,
    /// 底まで沈む時間(サンプル)
    pub attack_samples: f32,
    /// 浮かび上がる時間(サンプル)
    pub release_samples: f32,
}

// ============================ Delay ====================================

/// ディレイ系(delay / chorus / tape)が共有するバッファ長(1ch 分、2 のべき乗)。
/// 48kHz で約 1.36 秒。delay の最大 1000ms はここに収まる(高いサンプルレートでは頭打ち)。
const DLY_LEN: usize = 1 << 16;
const DLY_MASK: usize = DLY_LEN - 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DelayParams {
    /// 遅延(サンプル)
    pub time: f32,
    /// フィードバック 0..=0.95
    pub feedback: f32,
    /// ウェット比率 0..=1
    pub mix: f32,
    /// やまびこ用 1 次 LP の係数(exp(-2πfc/sr)。1 に近いほど暗い)
    pub tone_coef: f32,
    /// 左右交互に跳ねる
    pub ping_pong: bool,
}

// ============================ Chorus ===================================

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChorusParams {
    /// LFO の 1 サンプルあたりの位相増分(周期 = 1)
    pub rate_inc: f32,
    /// 揺らす幅(サンプル)
    pub depth: f32,
    /// 中心の遅延(サンプル)
    pub base: f32,
    pub mix: f32,
}

// ============================= Tape ====================================

/// テープ/ローファイ。ワウ(ゆっくりした回転むら)・フラッター(速い回転むら)で音程を
/// 揺らし、テープの飽和・高域の減衰・ヒスノイズ・ビット落としで古びた質感を作る。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TapeParams {
    /// 中心の遅延(サンプル。揺れの幅より大きく取る)
    pub base: f32,
    pub wow_depth: f32,
    pub wow_inc: f32,
    pub flutter_depth: f32,
    pub flutter_inc: f32,
    /// 飽和の入力ゲイン(1 = ほぼ素通し)
    pub drive: f32,
    pub tone_coef: f32,
    /// ヒスノイズの振幅
    pub hiss: f32,
    /// ビット落としの量子化段数(0 = 落とさない)
    pub quant: f32,
}

const WOW_HZ: f32 = 0.55;
const FLUTTER_HZ: f32 = 6.5;
/// wow = 1 のときの揺れ幅(ms)。0.55Hz で約 ±0.8%(±14 セント)の音程の揺れ
const WOW_MAX_MS: f32 = 2.4;
/// flutter = 1 のときの揺れ幅(ms)。6.5Hz で約 ±0.5%
const FLUTTER_MAX_MS: f32 = 0.12;

impl TapeParams {
    fn base_for(sample_rate: f32) -> f32 {
        (WOW_MAX_MS + FLUTTER_MAX_MS + 1.0) * 0.001 * sample_rate
    }
}

/// 小数遅延の読み出し(線形補間)。`idx` は次に書く位置。
fn read_frac(buf: &[f32], idx: usize, delay: f32) -> f32 {
    let delay = delay.clamp(1.0, (DLY_LEN - 2) as f32);
    let d0 = delay.floor();
    let frac = delay - d0;
    let i0 = idx.wrapping_sub(d0 as usize) & DLY_MASK;
    let i1 = i0.wrapping_sub(1) & DLY_MASK;
    buf[i0] * (1.0 - frac) + buf[i1] * frac
}

// ======================= 統合(定義と状態) =============================

/// 焼き込み済みのエフェクト定義(オーディオスレッドは読むだけ)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EffectParams {
    Eq(EqParams),
    Compressor(CompressorParams),
    Reverb(ReverbParams),
    Distortion(DistortionParams),
    Amp(AmpParams),
    Sidechain(SidechainParams),
    Delay(DelayParams),
    Chorus(ChorusParams),
    Tape(TapeParams),
    /// glaux-dsp の外(CLAP プラグイン)で処理するエフェクト。ここでは素通し
    External,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EffectKind {
    None,
    Eq,
    Compressor,
    Reverb,
    Distortion,
    Amp,
    Sidechain,
    Delay,
    Chorus,
    Tape,
}

/// エフェクト 1 スロット分の状態。全種類のバッファを持ち、起動時に確保して使い回す。
#[derive(Clone)]
pub struct EffectState {
    kind: EffectKind,
    // EQ: 3 バンド × 2ch
    eq: [[BiquadState; 3]; 2],
    // Compressor / Sidechain の検出エンベロープ
    envelope: f32,
    // Sidechain(ダッカー)のトリガー状態
    duck_pos: f32,
    duck_active: bool,
    key_was_above: bool,
    // Distortion のトーン用 1 次 LP(2ch)
    tone_lp: [f32; 2],
    // Amp のフィルタ群(2ch)
    amp: [AmpChState; 2],
    // Reverb
    reverb: [ReverbChannel; 2],
    // Delay / Chorus / Tape の共有ディレイバッファ(2ch)
    dly: [Vec<f32>; 2],
    dly_idx: usize,
    dly_lp: [f32; 2],
    // Chorus / Tape の LFO 位相(0..1)
    lfo: [f32; 2],
    // Tape のヒスノイズ用乱数(xorshift32)
    rng: u32,
}

const RNG_SEED: u32 = 0x9E37_79B9;

impl Default for EffectState {
    fn default() -> Self {
        EffectState {
            kind: EffectKind::None,
            eq: Default::default(),
            envelope: 0.0,
            duck_pos: 0.0,
            duck_active: false,
            key_was_above: false,
            tone_lp: [0.0; 2],
            amp: Default::default(),
            reverb: [ReverbChannel::new(0), ReverbChannel::new(STEREO_SPREAD)],
            dly: [vec![0.0; DLY_LEN], vec![0.0; DLY_LEN]],
            dly_idx: 0,
            dly_lp: [0.0; 2],
            lfo: [0.0; 2],
            rng: RNG_SEED,
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
            EffectParams::Amp(_) => EffectKind::Amp,
            EffectParams::Sidechain(_) => EffectKind::Sidechain,
            EffectParams::Delay(_) => EffectKind::Delay,
            EffectParams::Chorus(_) => EffectKind::Chorus,
            EffectParams::Tape(_) => EffectKind::Tape,
            EffectParams::External => EffectKind::None,
        }
    }

    /// データ差し替えでスロットの中身が変わったときに呼ぶ(アロケーションなし)。
    pub fn ensure_kind(&mut self, p: &EffectParams) {
        let kind = Self::kind_of(p);
        if self.kind != kind {
            self.kind = kind;
            self.eq = Default::default();
            self.envelope = 0.0;
            self.duck_pos = 0.0;
            self.duck_active = false;
            self.key_was_above = false;
            self.tone_lp = [0.0; 2];
            self.amp = Default::default();
            self.reverb[0].reset();
            self.reverb[1].reset();
            if matches!(
                kind,
                EffectKind::Delay | EffectKind::Chorus | EffectKind::Tape
            ) {
                self.dly[0].fill(0.0);
                self.dly[1].fill(0.0);
            }
            self.dly_idx = 0;
            self.dly_lp = [0.0; 2];
            self.lfo = [0.0; 2];
            self.rng = RNG_SEED;
        }
    }

    /// ステレオ 1 サンプル処理。`key` はサイドチェインの検出信号
    /// (通常はソーストラックのモノ合算。サイドチェイン以外は無視する)。
    pub fn process(&mut self, p: &EffectParams, l: f32, r: f32, key: f32) -> (f32, f32) {
        match p {
            EffectParams::External => (l, r),
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
            EffectParams::Amp(a) => {
                let ch = |x: f32, st: &mut AmpChState| -> f32 {
                    // DC ブロック
                    let hp = x - st.dc_in + a.dc_r * st.dc_out;
                    st.dc_in = x;
                    st.dc_out = hp;
                    // プリアンプ 2 段。段間 LP でフィジーさを抑え、
                    // 2 段目は非対称クリップ(偶数次倍音 = 真空管っぽい太さ)
                    let s1 = (hp * a.gain * 0.5).tanh();
                    st.stage_lp += (s1 - st.stage_lp) * a.stage_coef;
                    let s2 = (st.stage_lp * 2.4 + 0.12).tanh() - 0.119_4;
                    // トーン
                    st.tone_lp += (s2 - st.tone_lp) * (1.0 - a.tone_coef);
                    let mut y = st.tone_lp;
                    // プレゼンス(3kHz 以上を足してピッキングの輪郭を立てる)
                    st.pres_lp += (y - st.pres_lp) * a.pres_coef;
                    y += (y - st.pres_lp) * a.presence * 1.4;
                    // キャビネット(HP 80Hz + LP 4.2kHz × 3 の箱鳴り帯域。
                    // 歪みのフィジーな超高域はスピーカーからはほぼ出ない)
                    if a.cab {
                        let c = y - st.cab_hp_in + a.cab_hp_r * st.cab_hp_out;
                        st.cab_hp_in = y;
                        st.cab_hp_out = c;
                        st.cab_lp1 += (c - st.cab_lp1) * a.cab_lp_coef;
                        st.cab_lp2 += (st.cab_lp1 - st.cab_lp2) * a.cab_lp_coef;
                        st.cab_lp3 += (st.cab_lp2 - st.cab_lp3) * a.cab_lp_coef;
                        y = st.cab_lp3;
                    }
                    y * a.level
                };
                let [sl, sr] = &mut self.amp;
                (ch(l, sl), ch(r, sr))
            }
            EffectParams::Sidechain(sc) => {
                // 検出信号を高速フォロワで整える(生波形の振動でチャタらないように)
                let level = key.abs();
                if level > self.envelope {
                    self.envelope += (level - self.envelope) * 0.3;
                } else {
                    self.envelope += (level - self.envelope) * 0.0008; // 約 25ms
                }
                // 立ち上がり(下→上のしきい値クロス)でトリガー。キックごとにリトリガー
                let above = self.envelope > sc.threshold;
                if above && !self.key_was_above {
                    self.duck_pos = 0.0;
                    self.duck_active = true;
                }
                self.key_was_above = above;

                let gain = if self.duck_active {
                    let g = if self.duck_pos < sc.attack_samples {
                        // 底へ沈む
                        let t = self.duck_pos / sc.attack_samples.max(1.0);
                        1.0 + (sc.duck_floor - 1.0) * t
                    } else {
                        // キックの余韻に関係なく release_samples かけて浮上
                        let t = (self.duck_pos - sc.attack_samples) / sc.release_samples.max(1.0);
                        if t >= 1.0 {
                            self.duck_active = false;
                            1.0
                        } else {
                            sc.duck_floor + (1.0 - sc.duck_floor) * t
                        }
                    };
                    self.duck_pos += 1.0;
                    g
                } else {
                    1.0
                };
                (l * gain, r * gain)
            }
            EffectParams::Delay(d) => {
                let idx = self.dly_idx;
                // やまびこは毎回トーンの LP を通る(回を重ねるほど暗くなる)
                for ch in 0..2 {
                    let y = read_frac(&self.dly[ch], idx, d.time);
                    self.dly_lp[ch] += (y - self.dly_lp[ch]) * (1.0 - d.tone_coef);
                }
                let [el, er] = self.dly_lp;
                let (wl, wr) = if d.ping_pong {
                    // 入力は左へ、左のやまびこは右へ、右は左へ
                    ((l + r) * 0.5 + er * d.feedback, el * d.feedback)
                } else {
                    (l + el * d.feedback, r + er * d.feedback)
                };
                self.dly[0][idx] = wl;
                self.dly[1][idx] = wr;
                self.dly_idx = (idx + 1) & DLY_MASK;
                (
                    l * (1.0 - d.mix) + el * d.mix,
                    r * (1.0 - d.mix) + er * d.mix,
                )
            }
            EffectParams::Chorus(c) => {
                let idx = self.dly_idx;
                self.dly[0][idx] = l;
                self.dly[1][idx] = r;
                let tau = std::f32::consts::TAU;
                let ph = self.lfo[0];
                // 左右で LFO を 90° ずらして広がりを出す
                let ml = (tau * ph).sin();
                let mr = (tau * (ph + 0.25)).sin();
                let yl = read_frac(&self.dly[0], idx, c.base + c.depth * ml);
                let yr = read_frac(&self.dly[1], idx, c.base + c.depth * mr);
                self.lfo[0] = (ph + c.rate_inc).fract();
                self.dly_idx = (idx + 1) & DLY_MASK;
                (
                    l * (1.0 - c.mix) + yl * c.mix,
                    r * (1.0 - c.mix) + yr * c.mix,
                )
            }
            EffectParams::Tape(t) => {
                let idx = self.dly_idx;
                self.dly[0][idx] = l;
                self.dly[1][idx] = r;
                let tau = std::f32::consts::TAU;
                // 回転むらは左右共通(テープ全体が揺れる)
                let delay = t.base
                    + t.wow_depth * (tau * self.lfo[0]).sin()
                    + t.flutter_depth * (tau * self.lfo[1]).sin();
                self.lfo[0] = (self.lfo[0] + t.wow_inc).fract();
                self.lfo[1] = (self.lfo[1] + t.flutter_inc).fract();
                self.dly_idx = (idx + 1) & DLY_MASK;
                let mut out = [0.0f32; 2];
                for (ch, o) in out.iter_mut().enumerate() {
                    let x = read_frac(&self.dly[ch], idx, delay);
                    // 飽和(小さい音はほぼ素通し、大きい音ほど丸く潰れる)
                    let mut y = (x * t.drive).tanh() / t.drive;
                    self.tone_lp[ch] += (y - self.tone_lp[ch]) * (1.0 - t.tone_coef);
                    y = self.tone_lp[ch];
                    if t.hiss > 0.0 {
                        self.rng ^= self.rng << 13;
                        self.rng ^= self.rng >> 17;
                        self.rng ^= self.rng << 5;
                        y += (self.rng as f32 / u32::MAX as f32 - 0.5) * 2.0 * t.hiss;
                    }
                    if t.quant > 0.0 {
                        y = (y * t.quant).round() / t.quant;
                    }
                    *o = y;
                }
                (out[0], out[1])
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

pub static AMP_SPECS: &[ParamSpec] = &[
    ParamSpec {
        name: "gain_db",
        display_name: "ゲイン",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: 0.0,
            max: 54.0,
            default: 30.0,
            skew: None,
        },
        description: "歪みの深さ(プリアンプのゲイン)。〜10 でクリーン、15〜25 で\
            クランチ、30 前後でオーバードライブ、40 以上でメタル級ハイゲイン。\
            上げるほどサスティンも伸びる。",
    },
    ParamSpec {
        name: "tone",
        display_name: "トーン",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 1.0,
            default: 0.5,
            skew: None,
        },
        description: "歪み後の明るさ。下げると太く丸く、上げるとザクザクとエッジが立つ。",
    },
    ParamSpec {
        name: "presence",
        display_name: "プレゼンス",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 1.0,
            default: 0.35,
            skew: None,
        },
        description: "高域のエッジ(3kHz 以上)。上げるとピッキングの輪郭が立つ。\
            上げすぎると刺さる。",
    },
    ParamSpec {
        name: "cab",
        display_name: "キャビネット",
        unit: None,
        range: ParamRange::Bool { default: true },
        description: "スピーカーキャビネットの箱鳴り(80Hz〜4.5kHz に整形)。\
            OFF はライン直結の広帯域で、通常は ON のままにする。",
    },
    ParamSpec {
        name: "level_db",
        display_name: "レベル",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: -30.0,
            max: 6.0,
            default: -12.0,
            skew: None,
        },
        description: "出力音量。ゲインを上げると音圧が大きく上がるのでここで戻す。",
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
        name: "duck_db",
        display_name: "沈み込みの深さ",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: 0.0,
            max: 24.0,
            default: 8.0,
            skew: None,
        },
        description: "キックのたびに沈む深さ。6〜10 で心地よいポンピング、\
            12 以上でガッツリ潜る EDM 的な揺れ。",
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
        description: "底まで沈む速さ。短いほどキックの頭がクッキリ抜ける。",
    },
    ParamSpec {
        name: "release_ms",
        display_name: "リリース",
        unit: Some("ms"),
        range: ParamRange::Float {
            min: 20.0,
            max: 1000.0,
            default: 200.0,
            skew: Some(0.3),
        },
        description: "浮き上がる時間。ポンピングの「揺れ」はここで決まる。\
            ビートに合わせるのがコツ: 8 分音符の長さ(60000/BPM/2 ms)前後、\
            例えば 128BPM なら 200〜230ms にすると気持ちよく揺れる。",
    },
];

pub static DELAY_SPECS: &[ParamSpec] = &[
    ParamSpec {
        name: "time_ms",
        display_name: "タイム",
        unit: Some("ms"),
        range: ParamRange::Float {
            min: 10.0,
            max: 1000.0,
            default: 375.0,
            skew: Some(0.5),
        },
        description: "やまびこの間隔。テンポに合わせるのが基本: 4 分音符 = 60000/BPM ms、\
            付点 8 分 = 45000/BPM ms(例 120BPM なら 500 / 375)。30〜80ms はダブリング・スラップバック。",
    },
    ParamSpec {
        name: "feedback",
        display_name: "フィードバック",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 0.95,
            default: 0.35,
            skew: None,
        },
        description: "やまびこの繰り返しの多さ。0.2 で 2〜3 回、0.5 で長く続き、0.8 以上はほぼ鳴りやまない。",
    },
    ParamSpec {
        name: "mix",
        display_name: "ミックス",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 1.0,
            default: 0.3,
            skew: None,
        },
        description: "やまびこの混ぜ具合。0.15〜0.3 でさりげなく、0.5 前後で主張する。",
    },
    ParamSpec {
        name: "tone",
        display_name: "トーン",
        unit: Some("Hz"),
        range: ParamRange::Float {
            min: 1000.0,
            max: 16000.0,
            default: 6000.0,
            skew: Some(0.4),
        },
        description: "やまびこの明るさ(ローパス)。下げると回を重ねるほど暗くなるアナログ/テープ風、\
            上げるとくっきりしたデジタルディレイ。",
    },
    ParamSpec {
        name: "ping_pong",
        display_name: "ピンポン",
        unit: None,
        range: ParamRange::Bool { default: false },
        description: "やまびこを左右交互に跳ねさせる。広がりが大きく出る。",
    },
];

pub static CHORUS_SPECS: &[ParamSpec] = &[
    ParamSpec {
        name: "rate_hz",
        display_name: "レート",
        unit: Some("Hz"),
        range: ParamRange::Float {
            min: 0.05,
            max: 5.0,
            default: 0.8,
            skew: Some(0.5),
        },
        description: "揺れの速さ。0.3〜1 でゆったり広がり、3 以上はビブラート風に揺れる。",
    },
    ParamSpec {
        name: "depth_ms",
        display_name: "デプス",
        unit: Some("ms"),
        range: ParamRange::Float {
            min: 0.0,
            max: 8.0,
            default: 2.5,
            skew: None,
        },
        description: "揺れの深さ。1〜3 で自然な厚み、5 以上で揺れがはっきり分かる(80 年代風)。",
    },
    ParamSpec {
        name: "delay_ms",
        display_name: "ディレイ",
        unit: Some("ms"),
        range: ParamRange::Float {
            min: 3.0,
            max: 30.0,
            default: 12.0,
            skew: None,
        },
        description: "原音からのずれ。短い(3〜6)とフランジャー寄りの金属感、長い(15〜25)と\
            2 人で弾いているようなダブリング感。",
    },
    ParamSpec {
        name: "mix",
        display_name: "ミックス",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 1.0,
            default: 0.5,
            skew: None,
        },
        description: "揺らした音の混ぜ具合。0.5 で最も濃いコーラス。",
    },
];

pub static TAPE_SPECS: &[ParamSpec] = &[
    ParamSpec {
        name: "wow",
        display_name: "ワウ",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 1.0,
            default: 0.3,
            skew: None,
        },
        description: "ゆっくりした回転むら(約 0.5Hz)による音程のうねり。0.2〜0.4 で懐かしい揺れ、\
            0.8 以上で伸びたカセットのように酔う。",
    },
    ParamSpec {
        name: "flutter",
        display_name: "フラッター",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 1.0,
            default: 0.2,
            skew: None,
        },
        description: "速い回転むら(約 6.5Hz)による細かい震え。上げるとヨレた質感になる。",
    },
    ParamSpec {
        name: "saturation",
        display_name: "サチュレーション",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 1.0,
            default: 0.3,
            skew: None,
        },
        description:
            "テープの飽和。大きい音ほど丸く潰れて温かく太くなる。上げると音量のピークも下がる。",
    },
    ParamSpec {
        name: "tone",
        display_name: "トーン",
        unit: Some("Hz"),
        range: ParamRange::Float {
            min: 1500.0,
            max: 18000.0,
            default: 9000.0,
            skew: Some(0.4),
        },
        description:
            "高域の減衰(ローパス)。下げるほどこもった古い録音、3000 以下でラジオ・電話風。",
    },
    ParamSpec {
        name: "hiss",
        display_name: "ヒス",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 1.0,
            default: 0.15,
            skew: None,
        },
        description: "テープのサーッというノイズ。0.1〜0.3 で空気感、上げるとローファイ感が増す。",
    },
    ParamSpec {
        name: "bits",
        display_name: "ビット深度",
        unit: Some("bit"),
        range: ParamRange::Float {
            min: 4.0,
            max: 16.0,
            default: 16.0,
            skew: None,
        },
        description:
            "量子化の粗さ。16 で無効、8〜12 でざらついたサンプラー風、4〜6 で激しく荒れる。",
    },
];

pub fn effect_params_spec(name: &str) -> Option<&'static [ParamSpec]> {
    match name {
        "eq" => Some(EQ_SPECS),
        "compressor" => Some(COMPRESSOR_SPECS),
        "reverb" => Some(REVERB_SPECS),
        "distortion" => Some(DISTORTION_SPECS),
        "amp" => Some(AMP_SPECS),
        "sidechain" => Some(SIDECHAIN_SPECS),
        "delay" => Some(DELAY_SPECS),
        "chorus" => Some(CHORUS_SPECS),
        "tape" => Some(TAPE_SPECS),
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
            articulations: &[],
        },
        crate::params::InstrumentInfo {
            name: "compressor",
            description: "コンプレッサー。音量のばらつきを揃え、音圧や密度を上げる。\
                かけすぎ(crest_factor が 6dB 以下)に注意。",
            params: COMPRESSOR_SPECS,
            articulations: &[],
        },
        crate::params::InstrumentInfo {
            name: "reverb",
            description: "リバーブ(残響)。奥行きと空間を作る。パッドやリードに薄く\
                かけると馴染む。低音楽器には控えめに。",
            params: REVERB_SPECS,
            articulations: &[],
        },
        crate::params::InstrumentInfo {
            name: "distortion",
            description: "ディストーション/サチュレーションペダル。EDM の荒い質感、\
                ドラムの太さ足し、アンプ(amp)前段のブースターに。エレキギターの\
                本格的な歪みは pluck + amp を使う。",
            params: DISTORTION_SPECS,
            articulations: &[],
        },
        crate::params::InstrumentInfo {
            name: "amp",
            description: "ギターアンプシミュレータ(多段クリップ + トーン + プレゼンス + \
                キャビネット)。pluck のエレキ化はこれが本体: pluck → amp で初めて\
                「アンプを通したエレキ」になる。gain_db 30 前後から歪み、40 以上でメタル。\
                distortion をペダルとして前段に挿すとさらに凶暴になる。",
            params: AMP_SPECS,
            articulations: &[],
        },
        crate::params::InstrumentInfo {
            name: "sidechain",
            description: "サイドチェインコンプ。別トラック(通常キック)が鳴った瞬間に\
                このトラックを沈み込ませる。EDM のポンピング/ビートダウンの要。\
                ベースやパッドに挿し、source にキックのトラック ID を設定して使う。",
            params: SIDECHAIN_SPECS,
            articulations: &[],
        },
        crate::params::InstrumentInfo {
            name: "delay",
            description: "ディレイ(やまびこ)。テンポに合わせた繰り返しでリードやボーカルに\
                奥行きと余韻を足す。ping_pong で左右に広がる。tone を下げるとアナログ風。\
                センド用バスに挿して複数トラックで共有するのも定番。",
            params: DELAY_SPECS,
            articulations: &[],
        },
        crate::params::InstrumentInfo {
            name: "chorus",
            description: "コーラス。少し遅らせて揺らした音を重ね、厚みと左右の広がりを出す。\
                クリーンギター・エレピ・パッド・シンセストリングスの定番。低音には控えめに。",
            params: CHORUS_SPECS,
            articulations: &[],
        },
        crate::params::InstrumentInfo {
            name: "tape",
            description: "テープ/ローファイ。回転むら(wow・flutter)の音程の揺れ、テープの飽和、\
                高域の減衰、ヒスノイズ、ビット落としで古びた質感を作る。Lo-fi Hip Hop、\
                シティポップ、ヴィンテージ感を出したいエレピやドラムバス・マスターに。",
            params: TAPE_SPECS,
            articulations: &[],
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

impl EffectParams {
    /// オートメーション用: 連続パラメータを生の値(ParamSpec と同じ単位)で上書きする。
    /// `bake_effect` と同じクランプ・変換を通す。対象外のパラメータは無視して false。
    /// オーディオスレッドからブロック単位で呼ばれる前提(アロケーションしない)。
    pub fn set_continuous(&mut self, name: &str, v: f32, sample_rate: f32) -> bool {
        let tau = std::f32::consts::TAU;
        let db = |x: f32| 10.0_f32.powf(x / 20.0);
        match self {
            EffectParams::Eq(p) => {
                let mut r = p.raw;
                match name {
                    "low_freq" => r.low_freq = v,
                    "low_gain_db" => r.low_gain_db = v,
                    "mid_freq" => r.mid_freq = v,
                    "mid_q" => r.mid_q = v,
                    "mid_gain_db" => r.mid_gain_db = v,
                    "high_freq" => r.high_freq = v,
                    "high_gain_db" => r.high_gain_db = v,
                    _ => return false,
                }
                *p = EqParams::from_raw(sample_rate, r);
            }
            EffectParams::Compressor(p) => {
                let coef = |ms: f32| 1.0 - (-1.0 / (ms.max(0.1) * 0.001 * sample_rate)).exp();
                match name {
                    "threshold_db" => p.threshold_db = v.clamp(-40.0, 0.0),
                    "ratio" => p.ratio = v.clamp(1.0, 20.0),
                    "attack_ms" => p.attack_coef = coef(v),
                    "release_ms" => p.release_coef = coef(v),
                    "makeup_db" => p.makeup = db(v.clamp(0.0, 24.0)),
                    _ => return false,
                }
            }
            EffectParams::Reverb(p) => match name {
                "mix" => p.mix = v.clamp(0.0, 1.0),
                "size" => p.feedback = 0.7 + v.clamp(0.0, 1.0) * 0.28,
                "damping" => p.damping = v.clamp(0.0, 1.0),
                _ => return false,
            },
            EffectParams::Distortion(p) => match name {
                "drive_db" => p.drive = db(v.clamp(0.0, 40.0)),
                "tone" => p.tone_coef = (-tau * v.clamp(500.0, 12000.0) / sample_rate).exp(),
                "mix" => p.mix = v.clamp(0.0, 1.0),
                "level_db" => p.level = db(v.clamp(-24.0, 6.0)),
                _ => return false,
            },
            EffectParams::Amp(p) => match name {
                "gain_db" => p.gain = db(v.clamp(0.0, 54.0)),
                "tone" => {
                    p.tone_coef = (-tau * (900.0 + v.clamp(0.0, 1.0) * 5500.0) / sample_rate).exp()
                }
                "presence" => p.presence = v.clamp(0.0, 1.0),
                "level_db" => p.level = db(v.clamp(-30.0, 6.0)),
                _ => return false,
            },
            EffectParams::Sidechain(p) => match name {
                "threshold_db" => p.threshold = db(v.clamp(-50.0, 0.0)),
                "duck_db" => p.duck_floor = db(-v.clamp(0.0, 24.0)),
                "attack_ms" => p.attack_samples = v.clamp(0.1, 50.0) * 0.001 * sample_rate,
                "release_ms" => p.release_samples = v.clamp(20.0, 1000.0) * 0.001 * sample_rate,
                _ => return false,
            },
            EffectParams::Delay(p) => match name {
                "time_ms" => p.time = delay_samples(v, sample_rate),
                "feedback" => p.feedback = v.clamp(0.0, 0.95),
                "mix" => p.mix = v.clamp(0.0, 1.0),
                "tone" => p.tone_coef = (-tau * v.clamp(1000.0, 16000.0) / sample_rate).exp(),
                _ => return false,
            },
            EffectParams::Chorus(p) => match name {
                "rate_hz" => p.rate_inc = v.clamp(0.05, 5.0) / sample_rate,
                "depth_ms" => p.depth = v.clamp(0.0, 8.0) * 0.001 * sample_rate,
                "delay_ms" => p.base = v.clamp(3.0, 30.0) * 0.001 * sample_rate,
                "mix" => p.mix = v.clamp(0.0, 1.0),
                _ => return false,
            },
            EffectParams::Tape(p) => match name {
                "wow" => p.wow_depth = v.clamp(0.0, 1.0) * WOW_MAX_MS * 0.001 * sample_rate,
                "flutter" => {
                    p.flutter_depth = v.clamp(0.0, 1.0) * FLUTTER_MAX_MS * 0.001 * sample_rate
                }
                "saturation" => p.drive = tape_drive(v),
                "tone" => p.tone_coef = (-tau * v.clamp(1500.0, 18000.0) / sample_rate).exp(),
                "hiss" => p.hiss = tape_hiss(v),
                "bits" => p.quant = tape_quant(v),
                _ => return false,
            },
            EffectParams::External => return false,
        }
        true
    }
}

fn delay_samples(ms: f32, sample_rate: f32) -> f32 {
    (ms.clamp(10.0, 1000.0) * 0.001 * sample_rate).min((DLY_LEN - 2) as f32)
}

fn tape_drive(sat: f32) -> f32 {
    1.0 + sat.clamp(0.0, 1.0) * 5.0
}

fn tape_hiss(h: f32) -> f32 {
    // 1.0 で約 -34dBFS
    h.clamp(0.0, 1.0) * 0.02
}

fn tape_quant(bits: f32) -> f32 {
    let b = bits.clamp(4.0, 16.0).round();
    if b >= 16.0 {
        0.0
    } else {
        2.0_f32.powf(b - 1.0)
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
            Some(EffectParams::Eq(EqParams::from_raw(
                sample_rate,
                EqRaw {
                    low_freq: get(map, s, "low_freq"),
                    low_gain_db: get(map, s, "low_gain_db"),
                    mid_freq: get(map, s, "mid_freq"),
                    mid_q: get(map, s, "mid_q"),
                    mid_gain_db: get(map, s, "mid_gain_db"),
                    high_freq: get(map, s, "high_freq"),
                    high_gain_db: get(map, s, "high_gain_db"),
                },
            )))
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
        "amp" => {
            let s = AMP_SPECS;
            let tau = std::f32::consts::TAU;
            let lp_coef = |fc: f32| 1.0 - (-tau * fc / sample_rate).exp();
            let tone = get(map, s, "tone").clamp(0.0, 1.0);
            let cab = match map.get("cab") {
                Some(ParamValue::Bool(b)) => *b,
                _ => true,
            };
            Some(EffectParams::Amp(AmpParams {
                gain: 10.0_f32.powf(get(map, s, "gain_db").clamp(0.0, 54.0) / 20.0),
                stage_coef: lp_coef(6000.0),
                tone_coef: (-tau * (900.0 + tone * 5500.0) / sample_rate).exp(),
                presence: get(map, s, "presence").clamp(0.0, 1.0),
                pres_coef: lp_coef(3000.0),
                cab,
                cab_hp_r: (-tau * 80.0 / sample_rate).exp(),
                cab_lp_coef: lp_coef(4200.0),
                dc_r: (-tau * 20.0 / sample_rate).exp(),
                level: 10.0_f32.powf(get(map, s, "level_db").clamp(-30.0, 6.0) / 20.0),
            }))
        }
        "sidechain" => {
            let s = SIDECHAIN_SPECS;
            let source_track = match map.get("source") {
                Some(ParamValue::Enum(id)) if !id.is_empty() => {
                    resolve_track(id).unwrap_or(u32::MAX)
                }
                _ => u32::MAX,
            };
            Some(EffectParams::Sidechain(SidechainParams {
                source_track,
                threshold: 10.0_f32.powf(get(map, s, "threshold_db").clamp(-50.0, 0.0) / 20.0),
                duck_floor: 10.0_f32.powf(-get(map, s, "duck_db").clamp(0.0, 24.0) / 20.0),
                attack_samples: get(map, s, "attack_ms").clamp(0.1, 50.0) * 0.001 * sample_rate,
                release_samples: get(map, s, "release_ms").clamp(20.0, 1000.0)
                    * 0.001
                    * sample_rate,
            }))
        }
        "delay" => {
            let s = DELAY_SPECS;
            let ping_pong = matches!(map.get("ping_pong"), Some(ParamValue::Bool(true)));
            Some(EffectParams::Delay(DelayParams {
                time: delay_samples(get(map, s, "time_ms"), sample_rate),
                feedback: get(map, s, "feedback").clamp(0.0, 0.95),
                mix: get(map, s, "mix").clamp(0.0, 1.0),
                tone_coef: (-std::f32::consts::TAU * get(map, s, "tone").clamp(1000.0, 16000.0)
                    / sample_rate)
                    .exp(),
                ping_pong,
            }))
        }
        "chorus" => {
            let s = CHORUS_SPECS;
            Some(EffectParams::Chorus(ChorusParams {
                rate_inc: get(map, s, "rate_hz").clamp(0.05, 5.0) / sample_rate,
                depth: get(map, s, "depth_ms").clamp(0.0, 8.0) * 0.001 * sample_rate,
                base: get(map, s, "delay_ms").clamp(3.0, 30.0) * 0.001 * sample_rate,
                mix: get(map, s, "mix").clamp(0.0, 1.0),
            }))
        }
        "tape" => {
            let s = TAPE_SPECS;
            let ms = 0.001 * sample_rate;
            Some(EffectParams::Tape(TapeParams {
                base: TapeParams::base_for(sample_rate),
                wow_depth: get(map, s, "wow").clamp(0.0, 1.0) * WOW_MAX_MS * ms,
                wow_inc: WOW_HZ / sample_rate,
                flutter_depth: get(map, s, "flutter").clamp(0.0, 1.0) * FLUTTER_MAX_MS * ms,
                flutter_inc: FLUTTER_HZ / sample_rate,
                drive: tape_drive(get(map, s, "saturation")),
                tone_coef: (-std::f32::consts::TAU * get(map, s, "tone").clamp(1500.0, 18000.0)
                    / sample_rate)
                    .exp(),
                hiss: tape_hiss(get(map, s, "hiss")),
                quant: tape_quant(get(map, s, "bits")),
            }))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::{Effect, FxId};

    #[test]
    fn set_continuous_matches_baking() {
        // オートメーションで上書きした結果が、最初からその値で焼いたものと一致する
        let cases: &[(&str, &str, f64)] = &[
            ("eq", "high_gain_db", 9.0),
            ("eq", "mid_freq", 1500.0),
            ("compressor", "ratio", 8.0),
            ("compressor", "makeup_db", 6.0),
            ("reverb", "mix", 0.8),
            ("distortion", "drive_db", 30.0),
            ("amp", "tone", 0.2),
            ("sidechain", "duck_db", 12.0),
            ("delay", "time_ms", 250.0),
            ("delay", "tone", 3000.0),
            ("chorus", "depth_ms", 5.0),
            ("tape", "wow", 0.8),
            ("tape", "bits", 8.0),
        ];
        let none = |_: &str| None;
        for (fx, name, v) in cases {
            let mut from_default = bake_effect(&effect(fx, &[]), 48_000.0, &none).unwrap();
            assert!(
                from_default.set_continuous(name, *v as f32, 48_000.0),
                "{fx}/{name}"
            );
            let direct = bake_effect(&effect(fx, &[(name, *v)]), 48_000.0, &none).unwrap();
            assert_eq!(from_default, direct, "{fx}/{name}");
        }
        let mut eq = bake_effect(&effect("eq", &[]), 48_000.0, &none).unwrap();
        assert!(!eq.set_continuous("no_such", 1.0, 48_000.0));
    }

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
    fn amp_high_gain_clips_small_input_into_sustain() {
        // 小さな入力(減衰した弦を想定)でも大ゲインで持ち上げて頭打ちにする
        // = 実機アンプの「サスティンが伸びる」挙動の源。
        // 入力を 4 倍(+12dB)にしても出力がほとんど増えなければ頭打ちしている
        let p = bake(&effect("amp", &[("gain_db", 45.0), ("level_db", 0.0)])).unwrap();
        let rms_at = |amp_in: f32| {
            let mut state = EffectState::default();
            state.ensure_kind(&p);
            let mut sum = 0.0f64;
            let mut n = 0usize;
            for i in 0..9600 {
                let x = (i as f32 * 110.0 * std::f32::consts::TAU / 48_000.0).sin() * amp_in;
                let (l, _) = state.process(&p, x, x, 0.0);
                if i > 4800 {
                    sum += (l as f64) * (l as f64);
                    n += 1;
                }
            }
            (sum / n as f64).sqrt() as f32
        };
        let quiet = rms_at(0.03);
        let loud = rms_at(0.12);
        assert!(
            quiet > 0.1,
            "0.03 の入力が大きく持ち上がるはず: rms={quiet}"
        );
        assert!(
            loud / quiet < 1.6,
            "入力 4 倍でも出力はほぼ増えない(頭打ち)はず: 比 {}",
            loud / quiet
        );
    }

    #[test]
    fn amp_cab_shapes_band() {
        // キャビネット ON は OFF より高域が削れる(箱鳴りの帯域整形)
        let make = |cab: bool| {
            // tone を最大にして高域を残し、キャビの整形だけを比較する
            let mut e = effect(
                "amp",
                &[("gain_db", 30.0), ("tone", 1.0), ("level_db", 0.0)],
            );
            e.params
                .insert("cab".to_owned(), glaux_core::ParamValue::Bool(cab));
            bake(&e).unwrap()
        };
        let energy_hf = |p: &EffectParams| {
            let mut state = EffectState::default();
            state.ensure_kind(p);
            let mut prev = 0.0f32;
            let mut acc = 0.0f64;
            for i in 0..9600 {
                let x = (i as f32 * 220.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.2;
                let (l, _) = state.process(p, x, x, 0.0);
                if i > 4800 {
                    acc += ((l - prev) as f64).powi(2);
                }
                prev = l;
            }
            acc
        };
        let on = energy_hf(&make(true));
        let off = energy_hf(&make(false));
        assert!(
            on < off * 0.75,
            "キャビ ON は高域が整形されるはず: on={on} off={off}"
        );
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

    fn run(p: &EffectParams, input: impl Fn(usize) -> (f32, f32), n: usize) -> Vec<(f32, f32)> {
        let mut st = EffectState::default();
        st.ensure_kind(p);
        (0..n)
            .map(|i| {
                let (l, r) = input(i);
                st.process(p, l, r, 0.0)
            })
            .collect()
    }

    fn impulse(i: usize) -> (f32, f32) {
        if i == 0 {
            (1.0, 1.0)
        } else {
            (0.0, 0.0)
        }
    }

    #[test]
    fn delay_echoes_at_the_given_time_and_decays() {
        let p = bake(&effect(
            "delay",
            &[
                ("time_ms", 100.0),
                ("feedback", 0.5),
                ("mix", 1.0),
                ("tone", 16000.0),
            ],
        ))
        .unwrap();
        let out = run(&p, impulse, 48_000);
        let peak_near = |center: usize| {
            out[center - 50..center + 50]
                .iter()
                .map(|o| o.0.abs())
                .fold(0.0f32, f32::max)
        };
        let e1 = peak_near(4800);
        let e2 = peak_near(9600);
        assert!(e1 > 0.3, "1 回目のやまびこ: {e1}");
        assert!(
            e2 < e1 * 0.7 && e2 > e1 * 0.2,
            "2 回目はフィードバック分小さい: {e1} {e2}"
        );
        // やまびこの間は静か
        assert!(peak_near(7200) < 0.01);
    }

    #[test]
    fn ping_pong_alternates_sides() {
        let p = bake(&effect(
            "delay",
            &[("time_ms", 100.0), ("feedback", 0.6), ("mix", 1.0)],
        ))
        .unwrap();
        let mut e = effect(
            "delay",
            &[("time_ms", 100.0), ("feedback", 0.6), ("mix", 1.0)],
        );
        e.params
            .insert("ping_pong".to_owned(), glaux_core::ParamValue::Bool(true));
        let pp = bake(&e).unwrap();
        assert_ne!(p, pp);
        let out = run(&pp, impulse, 20_000);
        let side = |center: usize| {
            let w = &out[center - 100..center + 100];
            (
                w.iter().map(|o| o.0.abs()).fold(0.0f32, f32::max),
                w.iter().map(|o| o.1.abs()).fold(0.0f32, f32::max),
            )
        };
        let (l1, r1) = side(4800);
        let (l2, r2) = side(9600);
        assert!(l1 > 0.1 && r1 < 0.01, "1 回目は左: {l1} {r1}");
        assert!(r2 > 0.05 && l2 < 0.01, "2 回目は右: {l2} {r2}");
    }

    #[test]
    fn chorus_makes_mono_input_wide_and_keeps_level() {
        let p = bake(&effect("chorus", &[("depth_ms", 4.0), ("rate_hz", 1.0)])).unwrap();
        let sine = |i: usize| {
            let x = (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5;
            (x, x)
        };
        let out = run(&p, sine, 48_000);
        let tail = &out[4800..];
        let diff: f32 = tail.iter().map(|o| (o.0 - o.1).powi(2)).sum::<f32>() / tail.len() as f32;
        let pow: f32 = tail.iter().map(|o| o.0 * o.0).sum::<f32>() / tail.len() as f32;
        assert!(diff.sqrt() > 0.05, "左右が違う(広がる): {}", diff.sqrt());
        assert!(
            pow.sqrt() > 0.15 && pow.sqrt() < 0.5,
            "音量はおおむね保つ: {}",
            pow.sqrt()
        );
    }

    /// 零交差の間隔から、区間ごとの周波数を測る
    fn zc_freq(x: &[f32]) -> f32 {
        let c: Vec<usize> = (1..x.len())
            .filter(|&i| x[i - 1] < 0.0 && x[i] >= 0.0)
            .collect();
        if c.len() < 2 {
            return 0.0;
        }
        (c.len() - 1) as f32 * 48_000.0 / (c[c.len() - 1] - c[0]) as f32
    }

    #[test]
    fn tape_wow_bends_pitch_and_off_keeps_it() {
        let sine = |i: usize| {
            let x = (i as f32 * 1000.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.3;
            (x, x)
        };
        let spread = |wow: f64| {
            let p = bake(&effect(
                "tape",
                &[
                    ("wow", wow),
                    ("flutter", 0.0),
                    ("hiss", 0.0),
                    ("tone", 18000.0),
                ],
            ))
            .unwrap();
            let out: Vec<f32> = run(&p, sine, 96_000).iter().map(|o| o.0).collect();
            let fs: Vec<f32> = out[4800..].chunks(4800).map(zc_freq).collect();
            let max = fs.iter().cloned().fold(f32::MIN, f32::max);
            let min = fs.iter().cloned().fold(f32::MAX, f32::min);
            max - min
        };
        let still = spread(0.0);
        let wobbly = spread(1.0);
        assert!(still < 1.0, "揺れなし: {still}Hz");
        assert!(wobbly > 8.0, "ワウで音程が揺れる: {wobbly}Hz");
    }

    #[test]
    fn tape_hiss_and_bits() {
        let p = bake(&effect("tape", &[("hiss", 1.0)])).unwrap();
        let out = run(&p, |_| (0.0, 0.0), 4800);
        let noise = out.iter().map(|o| o.0.abs()).fold(0.0f32, f32::max);
        assert!(noise > 0.005 && noise < 0.05, "無音にヒスが乗る: {noise}");
        // 4bit: 出力は 1/8 刻み
        let p = bake(&effect("tape", &[("hiss", 0.0), ("bits", 4.0)])).unwrap();
        let ramp = |i: usize| {
            let x = (i as f32 * 0.001).sin() * 0.8;
            (x, x)
        };
        for (l, _) in run(&p, ramp, 4800) {
            assert!(((l * 8.0).round() - l * 8.0).abs() < 1e-4, "{l}");
        }
    }
}
