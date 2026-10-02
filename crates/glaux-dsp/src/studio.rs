//! スタジオ系のエフェクト(段階 4 で足したもの): eq8(8 バンドのパラメトリック EQ)/ saturation /
//! deesser / gate。
//!
//! - eq8: バンドごとに種類(ベル・シェルフ・カット 12/24dB・ノッチ)・周波数・ゲイン・Q・有効、
//!   ダイナミック(しきい値と range。その帯域が大きいときだけゲインを動かす)、
//!   ステレオ / ミッド / サイドの処理先を持つ。古い eq(5 バンド固定)はそのまま残す
//! - saturation: テープ・真空管・トランジスタ・ソフトクリップの 4 つの曲線。2 倍オーバーサンプリング
//! - deesser: 高域だけを、刺さる音(歯擦音)が出ている間だけ下げる(下げない帯域は元の音のまま)
//! - gate: しきい値より小さい音を閉じる(ヒステリシス・ホールド付き)。別トラックの音で開閉もできる
//!
//! 状態は固定長で、オーディオスレッドでは確保しない。

use crate::effects::{smooth_coef, time_coef, Smoothed, SvfCoeffs, SvfState};
use crate::modfx::{choice, e, f, get};
use crate::oversample::Halfband;
use glaux_core::{ParamMap, ParamRange, ParamSpec, ParamValue};

/// eq8 のバンド数
pub const EQ8_BANDS: usize = 8;
/// ダイナミックなバンドの係数を求め直す間隔(サンプル)
const DYN_UPDATE: u32 = 16;

/// バンドの種類
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandType {
    Bell,
    LowShelf,
    HighShelf,
    LowCut,
    HighCut,
    LowCut24,
    HighCut24,
    Notch,
}

const BAND_TYPES: &[&str] = &[
    "bell",
    "low_shelf",
    "high_shelf",
    "low_cut",
    "high_cut",
    "low_cut_24",
    "high_cut_24",
    "notch",
];

impl BandType {
    fn parse(s: &str) -> BandType {
        match s {
            "low_shelf" => BandType::LowShelf,
            "high_shelf" => BandType::HighShelf,
            "low_cut" => BandType::LowCut,
            "high_cut" => BandType::HighCut,
            "low_cut_24" => BandType::LowCut24,
            "high_cut_24" => BandType::HighCut24,
            "notch" => BandType::Notch,
            _ => BandType::Bell,
        }
    }

    /// ゲインを持つ(ダイナミックにできる)種類
    fn has_gain(self) -> bool {
        matches!(
            self,
            BandType::Bell | BandType::LowShelf | BandType::HighShelf
        )
    }
}

/// バンドを掛ける先
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandStereo {
    Stereo,
    Mid,
    Side,
}

/// 1 バンドの生の値(ParamSpec と同じ単位)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandRaw {
    pub kind: BandType,
    pub freq: f32,
    pub gain_db: f32,
    pub q: f32,
    pub on: bool,
    pub threshold_db: f32,
    pub range_db: f32,
    pub stereo: BandStereo,
}

/// 焼き込み済みの 1 バンド
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Eq8Band {
    pub raw: BandRaw,
    /// 1 段目・2 段目(24dB/oct のときだけ 2 段目を使う)
    c: [SvfCoeffs; 2],
    two_stages: bool,
    /// カットが端(20Hz 以下・20kHz 以上)にあって効かない(状態は追い続けて、出力は素通し)
    bypass: bool,
    /// ダイナミック(range ≠ 0 でゲインのある種類)
    dynamic: bool,
    /// 検出の帯域(ベル = 帯域通過、低域シェルフ = ローパス、高域シェルフ = ハイパス)
    detect: SvfCoeffs,
    /// しきい値(リニア)
    thr_lin: f32,
}

/// 低域シェルフ(Q 付き。Q = 0.707 で eq の low_shelf と同じ)
fn low_shelf_q(sr: f32, freq: f32, q: f32, gain_db: f32) -> SvfCoeffs {
    let a = 10.0_f32.powf(gain_db / 40.0);
    let k = 1.0 / q.max(0.1);
    SvfCoeffs {
        g: SvfCoeffs::g_of(sr, freq) / a.sqrt(),
        k,
        m0: 1.0,
        m1: k * (a - 1.0),
        m2: a * a - 1.0,
    }
}

fn high_shelf_q(sr: f32, freq: f32, q: f32, gain_db: f32) -> SvfCoeffs {
    let a = 10.0_f32.powf(gain_db / 40.0);
    let k = 1.0 / q.max(0.1);
    SvfCoeffs {
        g: SvfCoeffs::g_of(sr, freq) * a.sqrt(),
        k,
        m0: a * a,
        m1: k * (1.0 - a) * a,
        m2: 1.0 - a * a,
    }
}

fn notch(sr: f32, freq: f32, q: f32) -> SvfCoeffs {
    let k = 1.0 / q.max(0.1);
    SvfCoeffs {
        g: SvfCoeffs::g_of(sr, freq),
        k,
        m0: 1.0,
        m1: -k,
        m2: 0.0,
    }
}

/// 4 次バターワースの 2 段の Q
const BW4_Q: [f32; 2] = [0.541_196, 1.306_563];

impl Eq8Band {
    fn new(raw: BandRaw, sr: f32) -> Eq8Band {
        let freq = raw.freq.clamp(20.0, 20_000.0).min(sr * 0.49);
        let q = raw.q.clamp(0.1, 18.0);
        let gain_db = raw.gain_db.clamp(-24.0, 24.0);
        let raw = BandRaw {
            freq,
            q,
            gain_db,
            threshold_db: raw.threshold_db.clamp(-60.0, 0.0),
            range_db: raw.range_db.clamp(-24.0, 24.0),
            ..raw
        };
        let id = SvfCoeffs::identity();
        // 24dB/oct は 2 段のバターワース(2 段目の Q を band の Q で持ち上げられる)
        let q2 = BW4_Q[1] * q / std::f32::consts::FRAC_1_SQRT_2;
        let (c, two_stages, bypass) = match raw.kind {
            BandType::Bell => ([SvfCoeffs::bell(sr, freq, q, gain_db), id], false, false),
            BandType::LowShelf => ([low_shelf_q(sr, freq, q, gain_db), id], false, false),
            BandType::HighShelf => ([high_shelf_q(sr, freq, q, gain_db), id], false, false),
            BandType::Notch => ([notch(sr, freq, q), id], false, false),
            BandType::LowCut => (
                [SvfCoeffs::high_pass_q(sr, freq, q), id],
                false,
                freq <= 20.0,
            ),
            BandType::HighCut => (
                [SvfCoeffs::low_pass_q(sr, freq, q), id],
                false,
                freq >= 20_000.0,
            ),
            BandType::LowCut24 => (
                [
                    SvfCoeffs::high_pass_q(sr, freq, BW4_Q[0]),
                    SvfCoeffs::high_pass_q(sr, freq, q2),
                ],
                true,
                freq <= 20.0,
            ),
            BandType::HighCut24 => (
                [
                    SvfCoeffs::low_pass_q(sr, freq, BW4_Q[0]),
                    SvfCoeffs::low_pass_q(sr, freq, q2),
                ],
                true,
                freq >= 20_000.0,
            ),
        };
        let detect = match raw.kind {
            BandType::LowShelf => SvfCoeffs::low_pass(sr, freq),
            BandType::HighShelf => SvfCoeffs::high_pass(sr, freq),
            _ => SvfCoeffs::band_pass_q(sr, freq, q.max(0.5)),
        };
        Eq8Band {
            raw,
            c,
            two_stages,
            bypass,
            dynamic: raw.kind.has_gain() && raw.range_db != 0.0,
            detect,
            thr_lin: 10.0_f32.powf(raw.threshold_db / 20.0),
        }
    }

    /// ゲイン(dB)を変えた係数(ダイナミック用)
    fn with_gain(&self, sr: f32, gain_db: f32) -> SvfCoeffs {
        let r = &self.raw;
        match r.kind {
            BandType::LowShelf => low_shelf_q(sr, r.freq, r.q, gain_db),
            BandType::HighShelf => high_shelf_q(sr, r.freq, r.q, gain_db),
            _ => SvfCoeffs::bell(sr, r.freq, r.q, gain_db),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Eq8Params {
    pub bands: [Eq8Band; EQ8_BANDS],
    pub output_db: f32,
    output: f32,
    pub dyn_attack_ms: f32,
    pub dyn_release_ms: f32,
    dyn_att: f32,
    dyn_rel: f32,
    env_att: f32,
    env_rel: f32,
    smooth: f32,
    sr: f32,
}

impl Eq8Params {
    pub fn new(
        raw: [BandRaw; EQ8_BANDS],
        output_db: f32,
        attack_ms: f32,
        release_ms: f32,
        sr: f32,
    ) -> Self {
        let output_db = output_db.clamp(-24.0, 24.0);
        let attack_ms = attack_ms.clamp(0.5, 100.0);
        let release_ms = release_ms.clamp(10.0, 1000.0);
        Eq8Params {
            bands: raw.map(|r| Eq8Band::new(r, sr)),
            output_db,
            output: 10.0_f32.powf(output_db / 20.0),
            dyn_attack_ms: attack_ms,
            dyn_release_ms: release_ms,
            dyn_att: time_coef(attack_ms, sr),
            dyn_rel: time_coef(release_ms, sr),
            env_att: time_coef(1.0, sr),
            env_rel: time_coef(50.0, sr),
            smooth: smooth_coef(sr),
            sr,
        }
    }

    /// 周波数 `freq` での利得(dB。ダイナミックは止まっているときの値)。表示・テスト用
    pub fn magnitude_db(&self, freq: f32) -> f32 {
        self.bands
            .iter()
            .filter(|b| b.raw.on && !b.bypass && b.raw.stereo == BandStereo::Stereo)
            .map(|b| {
                let mut m = b.c[0].magnitude_db(self.sr, freq);
                if b.two_stages {
                    m += b.c[1].magnitude_db(self.sr, freq);
                }
                m
            })
            .sum::<f32>()
            + self.output_db
    }
}

/// 1 バンドの状態
#[derive(Clone, Copy, Debug, Default)]
struct Eq8BandState {
    /// [チャンネル(ステレオなら L/R、ミッド・サイドなら 0 だけ)][段]
    f: [[SvfState; 2]; 2],
    detect: SvfState,
    env: f32,
    /// ダイナミックで動かしている量(dB、正)
    amount: f32,
    coeffs: Option<SvfCoeffs>,
    active: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Eq8State {
    bands: [Eq8BandState; EQ8_BANDS],
    count: u32,
    out: Smoothed,
}

impl Eq8State {
    #[inline]
    fn run_band(st: &mut Eq8BandState, b: &Eq8Band, smooth: f32, ch: usize, x: f32) -> f32 {
        let (c0, sm) = match (&st.coeffs, b.dynamic) {
            (Some(c), true) => (c, 1.0 / DYN_UPDATE as f32),
            _ => (&b.c[0], smooth),
        };
        let mut y = st.f[ch][0].process(c0, sm, x);
        if b.two_stages {
            y = st.f[ch][1].process(&b.c[1], sm, y);
        }
        if b.bypass {
            x
        } else {
            y
        }
    }

    #[inline]
    fn detect(st: &mut Eq8BandState, b: &Eq8Band, p: &Eq8Params, x: f32, update: bool) {
        let lvl = st.detect.process(&b.detect, 1.0, x).abs();
        let c = if lvl > st.env { p.env_att } else { p.env_rel };
        st.env = c * st.env + (1.0 - c) * lvl;
        let want = if st.env <= b.thr_lin {
            0.0
        } else {
            let over = 20.0 * st.env.max(1e-9).log10() - b.raw.threshold_db;
            (over * (2.0 / 3.0)).min(b.raw.range_db.abs())
        };
        let c = if want > st.amount {
            p.dyn_att
        } else {
            p.dyn_rel
        };
        st.amount = c * st.amount + (1.0 - c) * want;
        if update || st.coeffs.is_none() {
            let g = b.raw.gain_db + st.amount * b.raw.range_db.signum();
            st.coeffs = Some(b.with_gain(p.sr, g));
        }
    }

    pub fn process(&mut self, p: &Eq8Params, mut l: f32, mut r: f32) -> (f32, f32) {
        let update = self.count == 0;
        self.count = (self.count + 1) % DYN_UPDATE;
        for (b, st) in p.bands.iter().zip(self.bands.iter_mut()) {
            if !b.raw.on {
                st.active = false;
                continue;
            }
            if !st.active {
                // 止まっていた間の古い状態を使わない
                *st = Eq8BandState {
                    active: true,
                    ..Default::default()
                };
            }
            match b.raw.stereo {
                BandStereo::Stereo => {
                    if b.dynamic {
                        Self::detect(st, b, p, 0.5 * (l + r), update);
                    }
                    l = Self::run_band(st, b, p.smooth, 0, l);
                    r = Self::run_band(st, b, p.smooth, 1, r);
                }
                BandStereo::Mid | BandStereo::Side => {
                    let (m, s) = (0.5 * (l + r), 0.5 * (l - r));
                    let mid = b.raw.stereo == BandStereo::Mid;
                    let x = if mid { m } else { s };
                    if b.dynamic {
                        Self::detect(st, b, p, x, update);
                    }
                    let y = Self::run_band(st, b, p.smooth, 0, x);
                    let (m, s) = if mid { (y, s) } else { (m, y) };
                    l = m + s;
                    r = m - s;
                }
            }
        }
        let g = self.out.next(p.output, p.smooth);
        (l * g, r * g)
    }
}

// ============================= Saturation ==============================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SatMode {
    Tape,
    Tube,
    Transistor,
    SoftClip,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SaturationParams {
    pub mode: SatMode,
    /// ドライブ(リニア)
    pub drive: f32,
    /// トーンの傾き(−1 暗い〜1 明るい)
    pub tone: f32,
    /// 傾きの分かれ目の 1 次ローパスの係数(2 倍のレート用)
    tilt_coef: f32,
    pub mix: f32,
    /// 出力(リニア)
    pub output: f32,
    smooth: f32,
    sr: f32,
}

/// 真空管の曲線の偏り(偶数次の倍音)と、その tanh(原点の値と傾きをそろえる用)
const TUBE_BIAS: f32 = 0.25;
const TUBE_T0: f32 = 0.244_918_66;

impl SatMode {
    /// 原点の傾きが 1 で、大きい音ほど丸く潰れる曲線
    #[inline]
    fn shape(self, u: f32) -> f32 {
        match self {
            // テープ: tanh よりなだらかに潰れる(atan)
            SatMode::Tape => std::f32::consts::FRAC_2_PI * (u * std::f32::consts::FRAC_PI_2).atan(),
            // 真空管: 片側に寄せた tanh(偶数次の倍音で太く温かい)。直流は後で取る
            SatMode::Tube => ((u + TUBE_BIAS).tanh() - TUBE_T0) / (1.0 - TUBE_T0 * TUBE_T0),
            // トランジスタ: 角の硬い対称の曲線(奇数次が多く、荒い)
            SatMode::Transistor => {
                let u2 = u * u;
                u / (1.0 + u2 * u2).sqrt().sqrt()
            }
            // ソフトクリップ: 3 次の曲線で ±1 に張り付く
            SatMode::SoftClip => {
                let v = (u / 1.5).clamp(-1.0, 1.0);
                1.5 * v - 0.5 * v * v * v
            }
        }
    }
}

impl SaturationParams {
    pub fn new(mode: SatMode, drive_db: f32, tone: f32, mix: f32, output_db: f32, sr: f32) -> Self {
        SaturationParams {
            mode,
            drive: 10.0_f32.powf(drive_db.clamp(0.0, 36.0) / 20.0),
            tone: tone.clamp(-1.0, 1.0),
            tilt_coef: 1.0 - (-std::f32::consts::TAU * 1200.0 / (2.0 * sr)).exp(),
            mix: mix.clamp(0.0, 1.0),
            output: 10.0_f32.powf(output_db.clamp(-24.0, 12.0) / 20.0),
            smooth: smooth_coef(sr),
            sr,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SaturationState {
    os: [Halfband; 2],
    tilt_lp: [f32; 2],
    dc: [(f32, f32); 2],
    drive: Smoothed,
    tone: Smoothed,
    mix: Smoothed,
    out: Smoothed,
}

impl SaturationState {
    pub fn process(&mut self, p: &SaturationParams, l: f32, r: f32) -> (f32, f32) {
        let drive = self.drive.next(p.drive, p.smooth);
        let tone = self.tone.next(p.tone, p.smooth);
        let mix = self.mix.next(p.mix, p.smooth);
        let out = self.out.next(p.output, p.smooth);
        // 小さい音はほぼ同じ大きさのまま、大きい音ほど潰れる(ドライブで音量が跳ね上がらない)
        let inv = 1.0 / drive;
        // トーン: 1.2kHz を境に ±6dB 傾ける
        let gh = 2.0_f32.powf(tone);
        let gl = 1.0 / gh;
        let mode = p.mode;
        let dc_r = 1.0 - std::f32::consts::TAU * 15.0 / p.sr;
        let mut ch = |c: usize, x: f32| {
            let lp = &mut self.tilt_lp[c];
            let tc = p.tilt_coef;
            let y = self.os[c].run(x, |u| {
                let w = mode.shape(u * drive) * inv;
                *lp += (w - *lp) * tc;
                let w = *lp * gl + (w - *lp) * gh;
                u + (w - u) * mix
            });
            let y = if mode == SatMode::Tube {
                // 偏りで出た直流を取る
                let (x1, y1) = &mut self.dc[c];
                let o = y - *x1 + dc_r * *y1;
                *x1 = y;
                *y1 = o;
                o
            } else {
                y
            };
            y * out
        };
        (ch(0, l), ch(1, r))
    }
}

// =============================== De-esser ==============================

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeesserParams {
    pub freq: f32,
    pub threshold_db: f32,
    pub range_db: f32,
    hp: SvfCoeffs,
    thr_lin: f32,
    env_att: f32,
    env_rel: f32,
    att: f32,
    rel: f32,
    sr: f32,
}

impl DeesserParams {
    pub fn new(freq: f32, threshold_db: f32, range_db: f32, sr: f32) -> Self {
        let freq = freq.clamp(2000.0, 16_000.0).min(sr * 0.45);
        let threshold_db = threshold_db.clamp(-60.0, 0.0);
        DeesserParams {
            freq,
            threshold_db,
            range_db: range_db.clamp(0.0, 24.0),
            hp: SvfCoeffs::high_pass(sr, freq),
            thr_lin: 10.0_f32.powf(threshold_db / 20.0),
            env_att: time_coef(0.3, sr),
            env_rel: time_coef(40.0, sr),
            att: time_coef(1.0, sr),
            rel: time_coef(60.0, sr),
            sr,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DeesserState {
    /// 検出のハイパス(左右)と、下げる高域シェルフ(左右)
    hp: [SvfState; 2],
    shelf: [SvfState; 2],
    coeffs: Option<SvfCoeffs>,
    count: u32,
    env: f32,
    /// いま下げている量(dB、正)
    reduction: f32,
}

impl DeesserState {
    pub fn process(&mut self, p: &DeesserParams, l: f32, r: f32) -> (f32, f32) {
        // freq より上の音量で検出(左右の大きい方)
        let hl = self.hp[0].process(&p.hp, 1.0, l);
        let hr = self.hp[1].process(&p.hp, 1.0, r);
        let lvl = hl.abs().max(hr.abs());
        let c = if lvl > self.env { p.env_att } else { p.env_rel };
        self.env = c * self.env + (1.0 - c) * lvl;
        let want = if self.env <= p.thr_lin {
            0.0
        } else {
            // 超えた分を 4:1 で
            ((20.0 * self.env.log10() - p.threshold_db) * 0.75).min(p.range_db)
        };
        let c = if want > self.reduction { p.att } else { p.rel };
        self.reduction = c * self.reduction + (1.0 - c) * want;
        // 下げるのは高域シェルフ(下げていないときは素通しの係数で、状態だけ追う)
        if self.count == 0 || self.coeffs.is_none() {
            self.coeffs = Some(SvfCoeffs::high_shelf(p.sr, p.freq, -self.reduction));
        }
        self.count = (self.count + 1) % DYN_UPDATE;
        let Some(co) = self.coeffs else {
            return (l, r);
        };
        let sm = 1.0 / DYN_UPDATE as f32;
        (
            self.shelf[0].process(&co, sm, l),
            self.shelf[1].process(&co, sm, r),
        )
    }

    /// いま下げている量(dB)。テスト用
    #[cfg(test)]
    fn reduction(&self) -> f32 {
        self.reduction
    }
}

// ================================ Gate =================================

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GateParams {
    pub threshold_db: f32,
    pub range_db: f32,
    pub attack_ms: f32,
    pub hold_ms: f32,
    pub release_ms: f32,
    pub hysteresis_db: f32,
    open_lin: f32,
    close_lin: f32,
    /// 開く・閉じるときに 1 サンプルで動かす量(dB。range を attack / release の時間で動ききる)
    att_step: f32,
    rel_step: f32,
    hold: f32,
    env_rel: f32,
    /// 開閉の検出に使うトラック(u32::MAX = 自分の音)
    pub source_track: u32,
    sr: f32,
}

impl GateParams {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        threshold_db: f32,
        range_db: f32,
        attack_ms: f32,
        hold_ms: f32,
        release_ms: f32,
        hysteresis_db: f32,
        source_track: u32,
        sr: f32,
    ) -> Self {
        let threshold_db = threshold_db.clamp(-80.0, 0.0);
        let range_db = range_db.clamp(0.0, 80.0);
        let attack_ms = attack_ms.clamp(0.05, 50.0);
        let hold_ms = hold_ms.clamp(0.0, 500.0);
        let release_ms = release_ms.clamp(5.0, 2000.0);
        let hysteresis_db = hysteresis_db.clamp(0.0, 12.0);
        GateParams {
            threshold_db,
            range_db,
            attack_ms,
            hold_ms,
            release_ms,
            hysteresis_db,
            open_lin: 10.0_f32.powf(threshold_db / 20.0),
            close_lin: 10.0_f32.powf((threshold_db - hysteresis_db) / 20.0),
            att_step: range_db / (attack_ms * 0.001 * sr),
            rel_step: range_db / (release_ms * 0.001 * sr),
            hold: hold_ms * 0.001 * sr,
            env_rel: time_coef(10.0, sr),
            source_track,
            sr,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GateState {
    env: f32,
    open: bool,
    /// 閉じる条件になってからのサンプル数
    below: f32,
    /// いまの音量(dB、0 で開ききり、−range で閉じきり)と、そのリニアの値
    gain_db: f32,
    gain: f32,
    primed: bool,
}

impl GateState {
    pub fn process(&mut self, p: &GateParams, l: f32, r: f32, key: f32) -> (f32, f32) {
        if !self.primed {
            // 閉じた状態から
            self.primed = true;
            self.gain_db = -p.range_db;
            self.gain = 10.0_f32.powf(self.gain_db / 20.0);
        }
        let lvl = if p.source_track == u32::MAX {
            l.abs().max(r.abs())
        } else {
            key.abs()
        };
        // ピーク(すぐ上がり、約 10ms で下がる。波形の谷で閉じないように)
        self.env = lvl.max(p.env_rel * self.env);
        if self.env > p.open_lin {
            self.open = true;
            self.below = 0.0;
        } else if self.open && self.env < p.close_lin {
            self.below += 1.0;
            if self.below > p.hold {
                self.open = false;
            }
        }
        // dB で直線に動かす(release の時間で閉じきる)
        let target = if self.open { 0.0 } else { -p.range_db };
        if self.gain_db != target {
            self.gain_db = if self.gain_db < target {
                (self.gain_db + p.att_step).min(target)
            } else {
                (self.gain_db - p.rel_step).max(target)
            };
            self.gain = if self.gain_db >= 0.0 {
                1.0
            } else if self.gain_db <= -80.0 {
                0.0
            } else {
                10.0_f32.powf(self.gain_db / 20.0)
            };
        }
        (l * self.gain, r * self.gain)
    }
}

// ============================= まとめ ==================================

/// 焼き込み済みの値(`EffectParams` と同じく `Copy` のまま持つ)
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StudioParams {
    Eq8(Eq8Params),
    Saturation(SaturationParams),
    Deesser(DeesserParams),
    Gate(GateParams),
}

impl StudioParams {
    /// 種類の番号(状態を作り直すかの判定用)
    pub(crate) fn kind(&self) -> u8 {
        match self {
            StudioParams::Eq8(_) => 0,
            StudioParams::Saturation(_) => 1,
            StudioParams::Deesser(_) => 2,
            StudioParams::Gate(_) => 3,
        }
    }

    pub fn key_source(&self) -> Option<u32> {
        match self {
            StudioParams::Gate(g) if g.source_track != u32::MAX => Some(g.source_track),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct StudioState {
    eq8: Eq8State,
    sat: SaturationState,
    deesser: DeesserState,
    gate: GateState,
}

impl StudioState {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    #[inline]
    pub fn process(&mut self, p: &StudioParams, l: f32, r: f32, key: f32) -> (f32, f32) {
        match p {
            StudioParams::Eq8(q) => self.eq8.process(q, l, r),
            StudioParams::Saturation(s) => self.sat.process(s, l, r),
            StudioParams::Deesser(d) => self.deesser.process(d, l, r),
            StudioParams::Gate(g) => self.gate.process(g, l, r, key),
        }
    }
}

// ============================= つまみ ==================================

macro_rules! eq8_specs {
    ($(($n:literal, $kind:literal, $freq:literal, $q:literal)),* $(,)?) => {
        &[
            $(
                ParamSpec {
                    name: concat!("b", $n, "_on"),
                    display_name: concat!("バンド", $n, " 有効"),
                    unit: None,
                    range: ParamRange::Bool { default: true },
                    description: "このバンドを使う。切るとそのバンドは素通し。",
                },
                ParamSpec {
                    name: concat!("b", $n, "_type"),
                    display_name: concat!("バンド", $n, " 種類"),
                    unit: None,
                    range: ParamRange::Enum { choices: BAND_TYPES, default: $kind },
                    description: "bell は周波数のまわりを山・谷にする(Q で幅)。low_shelf / high_shelf はそれより下 / 上を\
                        まとめて上げ下げ(太さ・明るさ)。low_cut / high_cut はそれより下 / 上を削る(12dB/oct、_24 は 24dB/oct で急)。\
                        notch は 1 点だけを深くえぐる(ハム・鳴きの除去)。",
                },
                ParamSpec {
                    name: concat!("b", $n, "_freq"),
                    display_name: concat!("バンド", $n, " 周波数"),
                    unit: Some("Hz"),
                    range: ParamRange::Float { min: 20.0, max: 20000.0, default: $freq, skew: Some(0.3) },
                    description: "効かせる周波数。低音の太さ 60〜120、こもり 200〜500、鼻づまり 800〜1500、\
                        抜け・刺さり 2k〜5k、空気感 10k 以上。カットは 20 以下 / 20000 以上で効かない。",
                },
                ParamSpec {
                    name: concat!("b", $n, "_gain_db"),
                    display_name: concat!("バンド", $n, " ゲイン"),
                    unit: Some("dB"),
                    range: ParamRange::Float { min: -24.0, max: 24.0, default: 0.0, skew: None },
                    description: "bell・シェルフで上げ下げする量。上げるより削る方が自然に聞こえる(±3〜6 が目安)。",
                },
                ParamSpec {
                    name: concat!("b", $n, "_q"),
                    display_name: concat!("バンド", $n, " Q"),
                    unit: None,
                    range: ParamRange::Float { min: 0.1, max: 18.0, default: $q, skew: Some(0.4) },
                    description: "幅の狭さ。bell は 0.5〜1 で広く音楽的、4 以上で狭く外科的(鳴きを削る)。\
                        シェルフ・カットは 0.707 が素直で、上げると境目が盛り上がる。",
                },
                ParamSpec {
                    name: concat!("b", $n, "_threshold_db"),
                    display_name: concat!("バンド", $n, " しきい値"),
                    unit: Some("dB"),
                    range: ParamRange::Float { min: -60.0, max: 0.0, default: -24.0, skew: None },
                    description: "ダイナミック(range が 0 でないとき): この帯域の音量がこれを超えた分だけゲインを動かす。",
                },
                ParamSpec {
                    name: concat!("b", $n, "_range_db"),
                    display_name: concat!("バンド", $n, " レンジ"),
                    unit: Some("dB"),
                    range: ParamRange::Float { min: -24.0, max: 24.0, default: 0.0, skew: None },
                    description: "ダイナミックで動かす最大量(0 = 動かさない普通の EQ)。マイナスでその帯域が大きいときだけ削る\
                        (刺さり・こもり・ブーミーさを鳴っているときだけ抑える)、プラスで大きいときだけ持ち上げる。\
                        超えた分の 2/3 だけ動く(3:1)。bell・シェルフだけに効く。",
                },
                ParamSpec {
                    name: concat!("b", $n, "_stereo"),
                    display_name: concat!("バンド", $n, " 処理先"),
                    unit: None,
                    range: ParamRange::Enum { choices: &["stereo", "mid", "side"], default: "stereo" },
                    description: "stereo は左右とも、mid は真ん中(ボーカル・キック・ベース)だけ、side は左右の広がりだけに掛ける。\
                        side の低域を low_cut すると低音が締まり、side の高域を上げると広がりが明るくなる。",
                },
            )*
            f("output_db", "出力", Some("dB"), -24.0, 24.0, 0.0, "EQ の後の音量。大きく上げた分を戻して、音量の違いに騙されずに比べる。"),
            f("dyn_attack_ms", "ダイナミックのアタック", Some("ms"), 0.5, 100.0, 5.0, "ダイナミックなバンドが動き始める速さ。"),
            f("dyn_release_ms", "ダイナミックのリリース", Some("ms"), 10.0, 1000.0, 120.0, "ダイナミックなバンドが戻る速さ。"),
        ]
    };
}

pub static EQ8_SPECS: &[ParamSpec] = eq8_specs![
    (1, "low_cut", 20.0, 0.707),
    (2, "low_shelf", 100.0, 0.707),
    (3, "bell", 250.0, 1.0),
    (4, "bell", 700.0, 1.0),
    (5, "bell", 1500.0, 1.0),
    (6, "bell", 3500.0, 1.0),
    (7, "high_shelf", 8000.0, 0.707),
    (8, "high_cut", 20000.0, 0.707),
];

pub static SATURATION_SPECS: &[ParamSpec] = &[
    e("type", "種類", &["tape", "tube", "transistor", "soft_clip"], "tape", "tape はなめらかに丸く潰れて密度が増す(バス・マスター)。tube は偶数次の倍音で太く温かい(ボーカル・ベース・エレピ)。transistor は角が立って荒い(ドラム・ギター)。soft_clip は一定の大きさに張り付く(音圧)。"),
    f("drive_db", "ドライブ", Some("dB"), 0.0, 36.0, 12.0, "どれだけ強く当てるか。小さい音の大きさは変えずに大きい音ほど潰れる。6〜12 で温かさ・密度、18 以上ではっきり歪む。"),
    f("tone", "トーン", None, -1.0, 1.0, 0.0, "歪ませた音の明るさの傾き(−1 暗い〜1 明るい。±6dB)。倍音がギラつくなら下げる。"),
    f("mix", "ミックス", None, 0.0, 1.0, 1.0, "原音との混ぜ具合(パラレル・サチュレーション。0.3〜0.5 でアタックを残したまま太く)。"),
    f("output_db", "出力", Some("dB"), -24.0, 12.0, 0.0, "後の音量。"),
];

pub static DEESSER_SPECS: &[ParamSpec] = &[
    f("freq", "周波数", Some("Hz"), 2000.0, 16000.0, 6000.0, "これより上の高域を検出して下げる。女性ボーカルは 6000〜8000、男性は 4000〜6000、ハット・シンバルの刺さりは 8000 以上。"),
    f("threshold_db", "スレッショルド", Some("dB"), -60.0, 0.0, -30.0, "高域の音量がこれを超えたら下げ始める。下げるほど多くの「サ行」に効く。"),
    f("range_db", "レンジ", Some("dB"), 0.0, 24.0, 8.0, "最大でどれだけ下げるか。4〜8 で自然、12 以上は舌足らず(lisp)に聞こえやすい。"),
];

pub static GATE_SPECS: &[ParamSpec] = &[
    f("threshold_db", "スレッショルド", Some("dB"), -80.0, 0.0, -40.0, "これより大きい音で開く。ドラムのかぶり・アンプのノイズ・残響の尻尾より上、鳴らしたい音より下に。"),
    f("range_db", "レンジ", Some("dB"), 0.0, 80.0, 80.0, "閉じたときに下げる量。80 で無音、10〜20 で控えめに(かぶりを残して自然に)。"),
    f("attack_ms", "アタック", Some("ms"), 0.05, 50.0, 0.5, "開く速さ。短いほどアタックが立つ(短すぎると低音でプチッと鳴る)。"),
    f("hold_ms", "ホールド", Some("ms"), 0.0, 500.0, 20.0, "下回ってから閉じ始めるまで待つ時間(音の途中でパタパタ閉じないように)。"),
    f("release_ms", "リリース", Some("ms"), 5.0, 2000.0, 100.0, "閉じる速さ。短いとタイトに切れ(ゲートらしい効果)、長いと自然に消える。"),
    f("hysteresis_db", "ヒステリシス", Some("dB"), 0.0, 12.0, 4.0, "閉じるしきい値を開くしきい値よりこれだけ下にする(境目でばたつかない)。"),
    ParamSpec {
        name: "source",
        display_name: "検出のトラック",
        unit: None,
        range: ParamRange::Enum { choices: &[], default: "" },
        description: "空なら自分の音で開閉。トラック ID を入れると、そのトラックが鳴っている間だけ開く\
            (キックに合わせてベースやパッドを刻む、など)。例: set_param path=fx/<fx_id>/source value=\"trk_drm001\"",
    },
];

pub fn specs(name: &str) -> Option<&'static [ParamSpec]> {
    Some(match name {
        "eq8" => EQ8_SPECS,
        "saturation" => SATURATION_SPECS,
        "deesser" => DEESSER_SPECS,
        "gate" => GATE_SPECS,
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
        info("eq8", "8 バンドのパラメトリック EQ。バンドごとに種類(bell・シェルフ・カット 12/24dB・ノッチ)・周波数・ゲイン・Q、\
            ダイナミック(range: その帯域が大きいときだけ動く)、処理先(stereo / mid / side)。細かい帯域の整理・M/S の処理に。\
            つまみは b1_〜b8_ の頭で選ぶ(例 b3_gain_db)。既定は何もしない。"),
        info("saturation", "サチュレーション(tape / tube / transistor / soft_clip)。倍音を足して太さ・温かさ・密度・前に出る感じを作る。\
            小さい音の大きさは変えずに大きい音ほど潰れる。distortion より音楽的で、バス・マスター・ボーカル・ベースに薄く。"),
        info("deesser", "ディエッサー。ボーカルの「サ行」の刺さり(歯擦音)だけを、出ている間だけ下げる。高域以外の音は変えない。"),
        info("gate", "ゲート。しきい値より小さい音を閉じる(ドラムのかぶり・ノイズ・残響の尻尾を切る)。source に別トラックを入れると、\
            そのトラックが鳴っている間だけ開く(リズムで刻む)。"),
    ]
}

fn band_raw(map: &ParamMap, s: &[ParamSpec], n: usize) -> BandRaw {
    let name = |w: &str| format!("b{n}_{w}");
    BandRaw {
        kind: BandType::parse(choice(map, s, &name("type"))),
        freq: get(map, s, &name("freq")),
        gain_db: get(map, s, &name("gain_db")),
        q: get(map, s, &name("q")),
        on: !matches!(map.get(&name("on")), Some(ParamValue::Bool(false))),
        threshold_db: get(map, s, &name("threshold_db")),
        range_db: get(map, s, &name("range_db")),
        stereo: match choice(map, s, &name("stereo")) {
            "mid" => BandStereo::Mid,
            "side" => BandStereo::Side,
            _ => BandStereo::Stereo,
        },
    }
}

/// つまみの値から焼き込む。`resolve_track` はゲートの source(トラック ID)を index に引く
pub fn bake(
    name: &str,
    map: &ParamMap,
    sr: f32,
    resolve_track: &dyn Fn(&str) -> Option<u32>,
) -> Option<StudioParams> {
    let s = specs(name)?;
    Some(match name {
        "eq8" => StudioParams::Eq8(Eq8Params::new(
            std::array::from_fn(|i| band_raw(map, s, i + 1)),
            get(map, s, "output_db"),
            get(map, s, "dyn_attack_ms"),
            get(map, s, "dyn_release_ms"),
            sr,
        )),
        "saturation" => StudioParams::Saturation(SaturationParams::new(
            match choice(map, s, "type") {
                "tube" => SatMode::Tube,
                "transistor" => SatMode::Transistor,
                "soft_clip" => SatMode::SoftClip,
                _ => SatMode::Tape,
            },
            get(map, s, "drive_db"),
            get(map, s, "tone"),
            get(map, s, "mix"),
            get(map, s, "output_db"),
            sr,
        )),
        "deesser" => StudioParams::Deesser(DeesserParams::new(
            get(map, s, "freq"),
            get(map, s, "threshold_db"),
            get(map, s, "range_db"),
            sr,
        )),
        "gate" => {
            let source_track = match map.get("source") {
                Some(ParamValue::Enum(id)) if !id.is_empty() => {
                    resolve_track(id).unwrap_or(u32::MAX)
                }
                _ => u32::MAX,
            };
            StudioParams::Gate(GateParams::new(
                get(map, s, "threshold_db"),
                get(map, s, "range_db"),
                get(map, s, "attack_ms"),
                get(map, s, "hold_ms"),
                get(map, s, "release_ms"),
                get(map, s, "hysteresis_db"),
                source_track,
                sr,
            ))
        }
        _ => return None,
    })
}

impl StudioParams {
    /// オートメーション: 連続のつまみを上書き(確保しない)
    pub fn set_continuous(&mut self, name: &str, v: f32, sr: f32) -> bool {
        match self {
            StudioParams::Eq8(p) => {
                let (mut out, mut att, mut rel) = (p.output_db, p.dyn_attack_ms, p.dyn_release_ms);
                match name {
                    "output_db" => out = v,
                    "dyn_attack_ms" => att = v,
                    "dyn_release_ms" => rel = v,
                    _ => {
                        // b<番号>_<名前>
                        let Some((b, what)) =
                            name.strip_prefix('b').and_then(|n| n.split_once('_'))
                        else {
                            return false;
                        };
                        let Some(i) = b
                            .parse::<usize>()
                            .ok()
                            .filter(|i| (1..=EQ8_BANDS).contains(i))
                        else {
                            return false;
                        };
                        let mut raw = p.bands[i - 1].raw;
                        match what {
                            "freq" => raw.freq = v,
                            "gain_db" => raw.gain_db = v,
                            "q" => raw.q = v,
                            "threshold_db" => raw.threshold_db = v,
                            "range_db" => raw.range_db = v,
                            _ => return false,
                        }
                        p.bands[i - 1] = Eq8Band::new(raw, p.sr);
                        return true;
                    }
                }
                let bands = p.bands.map(|b| b.raw);
                *p = Eq8Params::new(bands, out, att, rel, p.sr);
            }
            StudioParams::Saturation(p) => {
                let mut drive_db = 20.0 * p.drive.log10();
                let mut out_db = 20.0 * p.output.log10();
                let (mut tone, mut mix) = (p.tone, p.mix);
                match name {
                    "drive_db" => drive_db = v,
                    "tone" => tone = v,
                    "mix" => mix = v,
                    "output_db" => out_db = v,
                    _ => return false,
                }
                *p = SaturationParams::new(p.mode, drive_db, tone, mix, out_db, p.sr);
            }
            StudioParams::Deesser(p) => {
                let (mut fr, mut t, mut r) = (p.freq, p.threshold_db, p.range_db);
                match name {
                    "freq" => fr = v,
                    "threshold_db" => t = v,
                    "range_db" => r = v,
                    _ => return false,
                }
                *p = DeesserParams::new(fr, t, r, sr);
            }
            StudioParams::Gate(p) => {
                let mut a = [
                    p.threshold_db,
                    p.range_db,
                    p.attack_ms,
                    p.hold_ms,
                    p.release_ms,
                    p.hysteresis_db,
                ];
                let i = match name {
                    "threshold_db" => 0,
                    "range_db" => 1,
                    "attack_ms" => 2,
                    "hold_ms" => 3,
                    "release_ms" => 4,
                    "hysteresis_db" => 5,
                    _ => return false,
                };
                a[i] = v;
                *p = GateParams::new(a[0], a[1], a[2], a[3], a[4], a[5], p.source_track, p.sr);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn map(pairs: &[(&str, ParamValue)]) -> ParamMap {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect()
    }

    fn baked(name: &str, pairs: &[(&str, ParamValue)]) -> StudioParams {
        bake(name, &map(pairs), SR, &|_| None).unwrap()
    }

    fn sine(freq: f32, amp: f32) -> impl Fn(usize) -> f32 {
        move |i| (i as f32 * freq * std::f32::consts::TAU / SR).sin() * amp
    }

    /// 後半の RMS(左)
    fn rms(p: &StudioParams, n: usize, input: impl Fn(usize) -> (f32, f32)) -> f32 {
        let mut st = StudioState::default();
        let mut s = 0.0f64;
        for i in 0..n {
            let (l, r) = input(i);
            let (y, _) = st.process(p, l, r, 0.0);
            if i >= n / 2 {
                s += (y * y) as f64;
            }
        }
        (s / (n - n / 2) as f64).sqrt() as f32
    }

    fn db(a: f32, b: f32) -> f32 {
        20.0 * (a / b).log10()
    }

    #[test]
    fn eq8_default_is_transparent() {
        let p = baked("eq8", &[]);
        let mut st = StudioState::default();
        for i in 0..4800 {
            let x = sine(440.0, 0.5)(i);
            let (l, r) = st.process(&p, x, -x, 0.0);
            assert!((l - x).abs() < 1e-5 && (r + x).abs() < 1e-5, "{i}: {l} {x}");
        }
    }

    #[test]
    fn eq8_bands_shape_the_spectrum() {
        // ベル +6dB @1k
        let p = baked(
            "eq8",
            &[("b5_freq", 1000.0.into()), ("b5_gain_db", 6.0.into())],
        );
        let base = rms(&baked("eq8", &[]), 9600, |i| (sine(1000.0, 0.3)(i), 0.0));
        let up = rms(&p, 9600, |i| (sine(1000.0, 0.3)(i), 0.0));
        assert!((db(up, base) - 6.0).abs() < 0.3, "{}", db(up, base));
        // 離れた周波数はほぼ変わらない
        let far = rms(&p, 9600, |i| (sine(100.0, 0.3)(i), 0.0));
        let far0 = rms(&baked("eq8", &[]), 9600, |i| (sine(100.0, 0.3)(i), 0.0));
        assert!(db(far, far0).abs() < 0.5);
        // 24dB/oct のローカットは 12dB/oct より深く削る
        let cut = |t: &str| {
            let p = baked(
                "eq8",
                &[
                    ("b1_type", ParamValue::Enum(t.into())),
                    ("b1_freq", 400.0.into()),
                ],
            );
            db(rms(&p, 19200, |i| (sine(100.0, 0.3)(i), 0.0)), far0)
        };
        let (c12, c24) = (cut("low_cut"), cut("low_cut_24"));
        assert!(c12 < -18.0 && c24 < c12 - 10.0, "{c12} {c24}");
        // ノッチは中心をえぐる
        let p = baked(
            "eq8",
            &[
                ("b4_type", ParamValue::Enum("notch".into())),
                ("b4_freq", 1000.0.into()),
                ("b4_q", 4.0.into()),
            ],
        );
        assert!(db(rms(&p, 19200, |i| (sine(1000.0, 0.3)(i), 0.0)), base) < -30.0);
        // magnitude_db と実際の音がそろう
        let p = baked("eq8", &[("b7_gain_db", (-6.0).into())]);
        if let StudioParams::Eq8(q) = &p {
            let want = q.magnitude_db(12_000.0);
            let s12 = |p: &StudioParams| rms(p, 9600, |i| (sine(12_000.0, 0.3)(i), 0.0));
            let got = db(s12(&p), s12(&baked("eq8", &[])));
            assert!((want - got).abs() < 0.3, "{want} {got}");
        }
    }

    #[test]
    fn eq8_mid_side_touches_only_its_part() {
        // side だけ 12dB 下げる: 同相(真ん中)の音は変わらず、逆相(広がり)だけ下がる
        let p = baked(
            "eq8",
            &[
                ("b5_gain_db", (-12.0).into()),
                ("b5_q", 0.3.into()),
                ("b5_stereo", ParamValue::Enum("side".into())),
            ],
        );
        let mid = rms(&p, 9600, |i| (sine(1500.0, 0.3)(i), sine(1500.0, 0.3)(i)));
        let side = rms(&p, 9600, |i| (sine(1500.0, 0.3)(i), -sine(1500.0, 0.3)(i)));
        let r = 0.3 / 2f32.sqrt();
        assert!(db(mid, r).abs() < 0.1, "{}", db(mid, r));
        assert!(db(side, r) < -9.0, "{}", db(side, r));
    }

    #[test]
    fn eq8_dynamic_band_cuts_only_when_loud() {
        let p = baked(
            "eq8",
            &[
                ("b6_freq", 3000.0.into()),
                ("b6_range_db", (-12.0).into()),
                ("b6_threshold_db", (-30.0).into()),
            ],
        );
        let quiet = rms(&p, 19200, |i| (sine(3000.0, 0.005)(i), 0.0));
        let loud = rms(&p, 19200, |i| (sine(3000.0, 0.5)(i), 0.0));
        let q_db = db(quiet, 0.005 / 2f32.sqrt());
        let l_db = db(loud, 0.5 / 2f32.sqrt());
        assert!(q_db.abs() < 0.5, "小さい音 {q_db}");
        assert!(l_db < -6.0, "大きい音 {l_db}");
    }

    #[test]
    fn eq8_automation_names() {
        let mut p = baked("eq8", &[]);
        assert!(p.set_continuous("b3_gain_db", 4.0, SR));
        assert!(p.set_continuous("output_db", -2.0, SR));
        assert!(!p.set_continuous("b9_gain_db", 4.0, SR));
        assert!(!p.set_continuous("b3_type", 4.0, SR));
        let direct = baked(
            "eq8",
            &[("b3_gain_db", 4.0.into()), ("output_db", (-2.0).into())],
        );
        assert_eq!(p, direct);
    }

    #[test]
    fn saturation_adds_harmonics_and_keeps_small_signals() {
        for t in ["tape", "tube", "transistor", "soft_clip"] {
            let p = baked(
                "saturation",
                &[
                    ("type", ParamValue::Enum(t.into())),
                    ("drive_db", 18.0.into()),
                ],
            );
            // 小さい音はほぼそのまま
            let small = rms(&p, 9600, |i| (sine(200.0, 0.001)(i), 0.0));
            assert!(db(small, 0.001 / 2f32.sqrt()).abs() < 0.5, "{t}: {small}");
            // 大きい音は倍音(3 倍・2 倍)が出る
            let mut st = StudioState::default();
            let n = 9600;
            let (mut h2, mut h3) = ((0.0f32, 0.0f32), (0.0f32, 0.0f32));
            for i in 0..n {
                let (y, _) = st.process(&p, sine(200.0, 0.8)(i), 0.0, 0.0);
                let ph = i as f32 * std::f32::consts::TAU / SR;
                h2.0 += y * (ph * 400.0).sin();
                h2.1 += y * (ph * 400.0).cos();
                h3.0 += y * (ph * 600.0).sin();
                h3.1 += y * (ph * 600.0).cos();
            }
            let mag = |h: (f32, f32)| (h.0 * h.0 + h.1 * h.1).sqrt() * 2.0 / n as f32;
            assert!(
                mag(h3) > 0.01 || mag(h2) > 0.01,
                "{t}: {} {}",
                mag(h2),
                mag(h3)
            );
            if t == "tube" {
                assert!(mag(h2) > 0.005, "真空管は偶数次 {}", mag(h2));
            }
        }
    }

    #[test]
    fn deesser_cuts_highs_only_when_sibilant() {
        let p = baked("deesser", &[("threshold_db", (-30.0).into())]);
        // 低い音は変わらない
        let low = rms(&p, 9600, |i| (sine(300.0, 0.5)(i), 0.0));
        assert!(db(low, 0.5 / 2f32.sqrt()).abs() < 0.2);
        // 大きな高域は下がる
        let hi = rms(&p, 9600, |i| (sine(8000.0, 0.5)(i), 0.0));
        assert!(
            db(hi, 0.5 / 2f32.sqrt()) < -4.0,
            "{}",
            db(hi, 0.5 / 2f32.sqrt())
        );
        // 小さな高域は下げない
        let mut st = DeesserState::default();
        if let StudioParams::Deesser(d) = &p {
            for i in 0..4800 {
                st.process(d, sine(8000.0, 0.005)(i), 0.0);
            }
            assert!(st.reduction() < 0.1);
        }
    }

    #[test]
    fn gate_closes_quiet_parts_and_follows_a_key() {
        let p = baked("gate", &[("threshold_db", (-30.0).into())]);
        // 大きい音(-6dB)は通り、小さい音(-50dB)は閉じる
        let loud = rms(&p, 9600, |i| (sine(200.0, 0.5)(i), 0.0));
        assert!(db(loud, 0.5 / 2f32.sqrt()).abs() < 0.3);
        let quiet = rms(&p, 9600, |i| (sine(200.0, 0.003)(i), 0.0));
        assert!(quiet < 1e-5, "{quiet}");
        // 別トラックの音で開閉
        let p = bake(
            "gate",
            &map(&[("source", ParamValue::Enum("trk".into()))]),
            SR,
            &|_| Some(3),
        )
        .unwrap();
        assert_eq!(p.key_source(), Some(3));
        let mut st = StudioState::default();
        let mut open = 0.0;
        let mut closed = 0.0;
        for i in 0..48_000 {
            let key = if i < 24_000 { 0.5 } else { 0.0 };
            let (y, _) = st.process(&p, 0.5, 0.5, key);
            if (12_000..24_000).contains(&i) {
                open += y;
            } else if i > 36_000 {
                closed += y;
            }
        }
        assert!(
            open / 12_000.0 > 0.49 && closed.abs() < 1e-3,
            "{open} {closed}"
        );
    }

    #[test]
    fn automation_matches_baking() {
        let cases: &[(&str, &str, f32)] = &[
            ("saturation", "drive_db", 20.0),
            ("saturation", "mix", 0.4),
            ("deesser", "threshold_db", -20.0),
            ("gate", "release_ms", 300.0),
        ];
        for (fx, name, v) in cases {
            let mut p = baked(fx, &[]);
            assert!(p.set_continuous(name, *v, SR), "{fx}/{name}");
            let direct = baked(fx, &[(name, (*v as f64).into())]);
            match (&p, &direct) {
                (StudioParams::Saturation(a), StudioParams::Saturation(b)) => {
                    assert!((a.drive - b.drive).abs() < 1e-4 && (a.mix - b.mix).abs() < 1e-6);
                }
                _ => assert_eq!(p, direct, "{fx}/{name}"),
            }
        }
    }
}
