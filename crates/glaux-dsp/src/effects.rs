//! 内蔵エフェクト: `eq` / `compressor` / `reverb` / `distortion` / `amp` / `sidechain` /
//! `delay` / `chorus` / `tape` ほか。変調系は [`crate::modfx`]、スタジオ系(eq8・saturation・
//! deesser・gate)は [`crate::studio`]、音程を動かすものは [`crate::pitch`] にある。
//!
//! - パラメータはデータ構築時(UI スレッド)に**係数まで焼き込む**([`bake_effect`])。
//!   オーディオスレッドは焼き込み済みの [`EffectParams`] を読むだけ
//! - 状態([`EffectState`])はエンジンが起動時にプール確保する。リバーブの
//!   ディレイバッファ込みで、オーディオスレッドでのアロケーションはない
//! - 処理はステレオ 1 サンプルずつ(`process`)

use glaux_core::{ParamMap, ParamRange, ParamSpec, ParamValue};

// ============================== EQ =====================================
//
// Cytomic(Andrew Simper)の線形台形積分 SVF。係数を毎サンプル動かしても安定し、
// 1 つの構造で ベル・シェルフ・ハイパス・ローパスを出せる(RBJ の biquad は係数の急な変化に弱い)。
// 出力 = m0·入力 + m1·バンドパス + m2·ローパス。係数は g = tan(π·fc/sr)、k = 1/Q から作る。

/// SVF 1 バンド分の係数(g・k と出力の混ぜ方)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SvfCoeffs {
    pub g: f32,
    pub k: f32,
    pub m0: f32,
    pub m1: f32,
    pub m2: f32,
}

impl SvfCoeffs {
    /// 何もしない(素通し)
    pub fn identity() -> Self {
        SvfCoeffs {
            g: 0.1,
            k: 1.414,
            m0: 1.0,
            m1: 0.0,
            m2: 0.0,
        }
    }

    pub(crate) fn g_of(sr: f32, freq: f32) -> f32 {
        (std::f32::consts::PI * (freq / sr).clamp(0.0001, 0.49)).tan()
    }

    /// ベル(ピーク / ディップ)
    pub fn bell(sr: f32, freq: f32, q: f32, gain_db: f32) -> Self {
        let a = 10.0_f32.powf(gain_db / 40.0);
        let k = 1.0 / (q.max(0.1) * a);
        SvfCoeffs {
            g: Self::g_of(sr, freq),
            k,
            m0: 1.0,
            m1: k * (a * a - 1.0),
            m2: 0.0,
        }
    }

    /// 低域シェルフ(Q = 0.707 で RBJ の S = 1 と同じ傾き)
    pub fn low_shelf(sr: f32, freq: f32, gain_db: f32) -> Self {
        let a = 10.0_f32.powf(gain_db / 40.0);
        let k = std::f32::consts::SQRT_2;
        SvfCoeffs {
            g: Self::g_of(sr, freq) / a.sqrt(),
            k,
            m0: 1.0,
            m1: k * (a - 1.0),
            m2: a * a - 1.0,
        }
    }

    /// 高域シェルフ
    pub fn high_shelf(sr: f32, freq: f32, gain_db: f32) -> Self {
        let a = 10.0_f32.powf(gain_db / 40.0);
        let k = std::f32::consts::SQRT_2;
        SvfCoeffs {
            g: Self::g_of(sr, freq) * a.sqrt(),
            k,
            m0: a * a,
            m1: k * (1.0 - a) * a,
            m2: 1.0 - a * a,
        }
    }

    /// 12 dB/oct のハイパス(バターワース)
    pub fn high_pass(sr: f32, freq: f32) -> Self {
        let k = std::f32::consts::SQRT_2;
        SvfCoeffs {
            g: Self::g_of(sr, freq),
            k,
            m0: 1.0,
            m1: -k,
            m2: -1.0,
        }
    }

    /// 周波数 `freq` での利得(dB)。SVF(台形積分)の伝達関数
    /// H = m0 + (m1·s + m2) / (s² + k·s + 1)、s = j·tan(π f / sr) / g
    pub fn magnitude_db(&self, sr: f32, freq: f32) -> f32 {
        let w = (std::f64::consts::PI * (freq as f64 / sr as f64).clamp(0.0, 0.4999)).tan()
            / (self.g as f64).max(1e-9);
        // s = j·w
        let (k, m0, m1, m2) = (
            self.k as f64,
            self.m0 as f64,
            self.m1 as f64,
            self.m2 as f64,
        );
        // 分母 (1 − w²) + j·k·w、分子 m2 + j·m1·w
        let (dr, di) = (1.0 - w * w, k * w);
        let (nr, ni) = (m2, m1 * w);
        let d2 = dr * dr + di * di;
        let (qr, qi) = ((nr * dr + ni * di) / d2, (ni * dr - nr * di) / d2);
        let (hr, hi) = (m0 + qr, qi);
        (10.0 * (hr * hr + hi * hi).max(1e-20).log10()) as f32
    }

    /// 2 次のオールパス(Q = 0.707。LR4 の分かれ目の位相の回りと同じ)
    pub fn all_pass(sr: f32, freq: f32) -> Self {
        let k = std::f32::consts::SQRT_2;
        SvfCoeffs {
            g: Self::g_of(sr, freq),
            k,
            m0: 1.0,
            m1: -2.0 * k,
            m2: 0.0,
        }
    }

    /// Q を指定した 12 dB/oct のローパス(Q を上げるとカットオフ付近が持ち上がる)
    pub fn low_pass_q(sr: f32, freq: f32, q: f32) -> Self {
        SvfCoeffs {
            g: Self::g_of(sr, freq),
            k: 1.0 / q.max(0.1),
            m0: 0.0,
            m1: 0.0,
            m2: 1.0,
        }
    }

    /// Q を指定した 12 dB/oct のハイパス
    pub fn high_pass_q(sr: f32, freq: f32, q: f32) -> Self {
        let k = 1.0 / q.max(0.1);
        SvfCoeffs {
            g: Self::g_of(sr, freq),
            k,
            m0: 1.0,
            m1: -k,
            m2: -1.0,
        }
    }

    /// Q を指定したバンドパス(中心で 0dB)
    pub fn band_pass_q(sr: f32, freq: f32, q: f32) -> Self {
        let k = 1.0 / q.max(0.1);
        SvfCoeffs {
            g: Self::g_of(sr, freq),
            k,
            m0: 0.0,
            m1: k,
            m2: 0.0,
        }
    }

    /// 12 dB/oct のローパス(バターワース)
    pub fn low_pass(sr: f32, freq: f32) -> Self {
        SvfCoeffs {
            g: Self::g_of(sr, freq),
            k: std::f32::consts::SQRT_2,
            m0: 0.0,
            m1: 0.0,
            m2: 1.0,
        }
    }
}

/// SVF の状態。係数は `cur` をゆっくり目標へ寄せて使う(オートメーションで係数が段差にならないように)
#[derive(Clone, Copy, Debug)]
pub struct SvfState {
    ic1: f32,
    ic2: f32,
    cur: SvfCoeffs,
    /// 最初の 1 サンプルは目標にそろえる
    primed: bool,
    /// `cur` から求めた a1・a2・a3(係数が落ち着いている間は求め直さない)
    a: [f32; 3],
    /// 係数が目標に落ち着いている
    settled: bool,
}

impl Default for SvfState {
    fn default() -> Self {
        SvfState {
            ic1: 0.0,
            ic2: 0.0,
            cur: SvfCoeffs::identity(),
            primed: false,
            a: [0.0; 3],
            settled: false,
        }
    }
}

impl SvfState {
    /// 1 サンプル処理する。`smooth` は係数を目標に寄せる 1 サンプルあたりの割合(0..1)
    #[inline]
    pub fn process(&mut self, target: &SvfCoeffs, smooth: f32, v0: f32) -> f32 {
        // 係数が目標に落ち着いていて目標も変わらなければ、平滑化と a の計算(割り算)を省く
        if !(self.settled && self.cur == *target) {
            if self.primed {
                let c = &mut self.cur;
                c.g += (target.g - c.g) * smooth;
                c.k += (target.k - c.k) * smooth;
                c.m0 += (target.m0 - c.m0) * smooth;
                c.m1 += (target.m1 - c.m1) * smooth;
                c.m2 += (target.m2 - c.m2) * smooth;
                // 十分近づいたら目標にそろえる(以後は省ける)
                let close = |a: f32, b: f32| (a - b).abs() <= 1e-6 * (1.0 + b.abs());
                self.settled = close(c.g, target.g)
                    && close(c.k, target.k)
                    && close(c.m0, target.m0)
                    && close(c.m1, target.m1)
                    && close(c.m2, target.m2);
                if self.settled {
                    *c = *target;
                }
            } else {
                self.cur = *target;
                self.primed = true;
                self.settled = true;
            }
            let c = &self.cur;
            let a1 = 1.0 / (1.0 + c.g * (c.g + c.k));
            let a2 = c.g * a1;
            self.a = [a1, a2, c.g * a2];
        }
        let [a1, a2, a3] = self.a;
        let v3 = v0 - self.ic2;
        let v1 = a1 * self.ic1 + a2 * v3;
        let v2 = self.ic2 + a2 * self.ic1 + a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        let c = &self.cur;
        c.m0 * v0 + c.m1 * v1 + c.m2 * v2
    }
}

/// 係数の平滑化の時定数(秒)
const SMOOTH_SECS: f32 = 0.005;

pub(crate) fn smooth_coef(sample_rate: f32) -> f32 {
    1.0 - (-1.0 / (SMOOTH_SECS * sample_rate)).exp()
}

/// つまみの値(ミックス・ゲインなど)を数 ms かけて目標へ寄せる 1 次の追従。
/// オートメーションはブロック(128 フレーム)ごとに値を変えるので、そのままだと階段の段差が
/// ジッパー音になる。最初の 1 サンプルと、目標に着いた後は目標そのものを返す
/// (オートメーションが無ければ平滑化しない場合と同じ音)
#[derive(Clone, Copy, Debug)]
pub(crate) struct Smoothed(f32);

impl Default for Smoothed {
    fn default() -> Self {
        Smoothed(f32::NAN)
    }
}

impl Smoothed {
    #[inline]
    pub(crate) fn next(&mut self, target: f32, k: f32) -> f32 {
        if self.0 == target || self.0.is_nan() {
            self.0 = target;
        } else {
            self.0 += (target - self.0) * k;
            if (self.0 - target).abs() <= 1e-6 * (1.0 + target.abs()) {
                self.0 = target;
            }
        }
        self.0
    }
}

/// EQ のバンド数(ハイパス・低域・中域・高域・ローパス)
const EQ_BANDS: usize = 5;

/// EQ の焼き込み済み係数。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EqParams {
    /// ハイパス・低域シェルフ・中域ベル・高域シェルフ・ローパス の順
    pub bands: [SvfCoeffs; EQ_BANDS],
    /// 使うバンド(ハイパス・ローパスは切ってあれば飛ばす)
    pub active: [bool; EQ_BANDS],
    /// 係数の平滑化の割合(1 サンプルあたり)
    pub smooth: f32,
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
    /// ハイパスの周波数(20 以下で切る)
    pub hp_freq: f32,
    /// ローパスの周波数(20000 以上で切る)
    pub lp_freq: f32,
}

/// ハイパス・ローパスを「切る」とみなす周波数
const EQ_HP_OFF: f32 = 20.0;
const EQ_LP_OFF: f32 = 20_000.0;

impl EqParams {
    fn from_raw(sample_rate: f32, r: EqRaw) -> EqParams {
        let hp_on = r.hp_freq > EQ_HP_OFF;
        let lp_on = r.lp_freq < EQ_LP_OFF;
        EqParams {
            bands: [
                SvfCoeffs::high_pass(sample_rate, r.hp_freq.max(EQ_HP_OFF)),
                SvfCoeffs::low_shelf(sample_rate, r.low_freq, r.low_gain_db.clamp(-15.0, 15.0)),
                SvfCoeffs::bell(
                    sample_rate,
                    r.mid_freq,
                    r.mid_q,
                    r.mid_gain_db.clamp(-15.0, 15.0),
                ),
                SvfCoeffs::high_shelf(sample_rate, r.high_freq, r.high_gain_db.clamp(-15.0, 15.0)),
                SvfCoeffs::low_pass(sample_rate, r.lp_freq.min(EQ_LP_OFF)),
            ],
            active: [hp_on, true, true, true, lp_on],
            smooth: smooth_coef(sample_rate),
            raw: r,
        }
    }

    /// 周波数 `freq` での利得(dB)。オフラインで EQ を当てはめるときに、音を通さずに特性を求める用
    pub fn magnitude_db(&self, sample_rate: f32, freq: f32) -> f32 {
        self.bands
            .iter()
            .zip(self.active)
            .filter(|(_, on)| *on)
            .map(|(b, _)| b.magnitude_db(sample_rate, freq))
            .sum()
    }
}

// =========================== Compressor ================================
//
// Giannoulis・Massberg・Reiss(JAES 2012)の構成: フィードフォワード、ゲインの計算は dB 領域で
// ソフトニー、減衰量(dB)を「なめらかで分離したピーク検出」でアタック / リリースに追従させる。
// 検出はステレオリンク(左右の大きい方)で、ピークか RMS を選べる。検出側にハイパス(低音でポンプしないように)。

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompressorParams {
    pub threshold_db: f32,
    pub ratio: f32,
    /// ソフトニーの幅(dB。0 でハードニー)
    pub knee_db: f32,
    /// アタック / リリースの 1 サンプルあたりの係数(exp(-1/(t·sr)))
    pub attack_coef: f32,
    pub release_coef: f32,
    pub makeup: f32,
    /// RMS で検出する(false はピーク)
    pub rms: bool,
    /// RMS の平均化の係数(約 10ms)
    pub rms_coef: f32,
    /// 検出側のハイパス(None で掛けない)
    pub sc_hpf: Option<SvfCoeffs>,
    /// ハイパスの周波数(オートメーション用の生の値)
    pub sc_hpf_hz: f32,
    /// 種類と先読み
    pub ext: CompExtra,
}

/// コンプの種類(機材の癖)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CompCharacter {
    /// 素直(以前からのコンプ)
    #[default]
    Clean,
    /// VCA: 長く圧縮したあとはゆっくり戻る(自動リリース。バス・ミックスのまとまり)
    Vca,
    /// FET: とても速いアタックと、圧縮に応じて足される歪み(攻撃的。ドラム・ボーカルを前へ)
    Fet,
    /// 光学式: 遅めのアタックと 2 段の戻り(速く半分戻り、残りはゆっくり。なめらか)
    Opto,
}

/// コンプの種類・先読みと、つまみの生の値
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompExtra {
    pub character: CompCharacter,
    /// 先読み(サンプル。音を遅らせて、検出を先に走らせる)
    pub lookahead: u32,
    pub lookahead_ms: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    /// vca / opto のゆっくり戻る追従: 溜まる係数・抜ける係数・効かせる割合(0 = 使わない)
    pub slow_charge: f32,
    pub slow_coef: f32,
    pub slow_weight: f32,
    /// メイクアップの平滑化
    pub smooth: f32,
}

impl Default for CompExtra {
    fn default() -> Self {
        CompExtra {
            character: CompCharacter::Clean,
            lookahead: 0,
            lookahead_ms: 0.0,
            attack_ms: 10.0,
            release_ms: 150.0,
            slow_charge: 0.0,
            slow_coef: 0.0,
            slow_weight: 0.0,
            smooth: 1.0,
        }
    }
}

impl CompExtra {
    /// 種類ごとのアタック / リリースの係数を求め、ゆっくり戻る追従の係数を入れる
    fn coefs(&mut self, sample_rate: f32) -> (f32, f32) {
        let (a, r) = (self.attack_ms, self.release_ms);
        let t = |ms: f32| time_coef(ms, sample_rate);
        let (att, rel, slow) = match self.character {
            CompCharacter::Clean => (t(a), t(r), None),
            // 長く潰した分だけ(4 倍の時間で溜まる)ゆっくり戻る
            CompCharacter::Vca => (t(a), t(r), Some((t(r * 4.0), t(r * 4.0), 1.0))),
            CompCharacter::Fet => (t(a * 0.2), t(r * 0.6), None),
            // 半分は速く戻り、残りの半分は溜まった分だけ 15 倍の時間でゆっくり抜ける
            CompCharacter::Opto => (
                t(a.max(10.0)),
                t(r * 0.5),
                Some((t(r * 3.0), t(r * 15.0), 0.5)),
            ),
        };
        (self.slow_charge, self.slow_coef, self.slow_weight) = slow.unwrap_or((0.0, 0.0, 0.0));
        (att, rel)
    }
}

impl CompressorParams {
    /// dB 領域のゲインの計算(ソフトニー)。入力のレベル(dB)に対して掛ける量(dB、0 以下)
    #[inline]
    pub(crate) fn gain_db(&self, x: f32) -> f32 {
        let (t, r, w) = (self.threshold_db, self.ratio, self.knee_db);
        let over = x - t;
        let y = if 2.0 * over < -w {
            x
        } else if w > 0.0 && 2.0 * over.abs() <= w {
            x + (1.0 / r - 1.0) * (over + w / 2.0).powi(2) / (2.0 * w)
        } else {
            t + over / r
        };
        y - x
    }
}

/// アタック / リリースの時間(ms)から 1 サンプルあたりの係数
pub(crate) fn time_coef(ms: f32, sample_rate: f32) -> f32 {
    (-1.0 / (ms.max(0.1) * 0.001 * sample_rate)).exp()
}

// ============================= Reverb ==================================
// 本体は crate::reverb(8 本の遅延線の FDN)

pub use crate::reverb::ReverbParams;

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
    /// ドライブ・ミックス・レベルの平滑化
    pub smooth: f32,
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
    /// 段間 LP の係数(クリップ段の間で高域を丸め、フィジーさを抑える。プリアンプは 2 倍のレートで回るので 2 倍レート用)
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
    /// プリアンプ(2 段のクリップ)を 2 倍のレートで回す(折り返し対策)
    os: crate::oversample::Halfband,
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
pub(crate) const DLY_LEN: usize = 1 << 16;
pub(crate) const DLY_MASK: usize = DLY_LEN - 1;
/// ディレイの長さの上限(サンプル。テンポに合わせて長くなってもバッファに収める)
const DELAY_MAX: f32 = (DLY_LEN - 4) as f32;

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
    /// ダッキング: 原音が鳴っている間にやまびこを沈める量(dB。0 = しない)
    pub duck_db: f32,
    /// 種類(digital = 以前からのディレイ)
    pub kind: DelayKind,
    /// テンポに合わせる長さ(tick。0 = time を使う)
    pub sync_ticks: f64,
    /// multitap の読み出し口の数(2〜4)
    pub taps: u8,
    /// ミックス・フィードバックの平滑化と、長さの変化を追う係数(digital / 揺れる種類)
    pub smooth: f32,
    pub glide: f32,
    pub glide_slow: f32,
    /// 揺れ(tape / bbd)の 1 サンプルあたりの位相の進み
    pub wow_inc: f32,
    pub flutter_inc: f32,
    pub sample_rate: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DelayKind {
    Digital,
    Tape,
    Bbd,
    Multitap,
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
    /// ミックスの平滑化
    pub smooth: f32,
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
    /// レコードのパチパチが 1 サンプルあたりに起きる確率(0 = 無し)
    pub crackle_rate: f32,
    /// パチパチの大きさ(最大)
    pub crackle_amp: f32,
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

/// 小数遅延の読み出し(4 点の 3 次 Hermite 補間)。`idx` は次に書く位置。
/// 線形補間は遅延を揺らす(コーラス・テープ)と高域が落ちるので、4 点で読む
pub(crate) fn read_frac(buf: &[f32], idx: usize, delay: f32) -> f32 {
    let delay = delay.clamp(2.0, (DLY_LEN - 3) as f32);
    let d0 = delay.floor();
    let t = delay - d0;
    // 新しい順に y0(遅延 d0-1)・y1(d0)・y2(d0+1)・y3(d0+2)。t は y1 → y2 の間
    let i1 = idx.wrapping_sub(d0 as usize) & DLY_MASK;
    let y0 = buf[(i1 + 1) & DLY_MASK];
    let y1 = buf[i1];
    let y2 = buf[i1.wrapping_sub(1) & DLY_MASK];
    let y3 = buf[i1.wrapping_sub(2) & DLY_MASK];
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + y1
}

// ======================= 統合(定義と状態) =============================

/// 焼き込み済みのエフェクト定義(オーディオスレッドは読むだけ)。
/// `Copy` のまま再生データの中に並べる(箱に入れると差し替えのたびに確保が要る)ので、
/// 大きな種類(eq8 の 8 バンド)との大きさの差は受け入れる
#[allow(clippy::large_enum_variant)]
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
    Multiband(crate::dynamics::MultibandParams),
    Transient(crate::dynamics::TransientParams),
    Limiter(crate::limiter::LimiterParams),
    Width(crate::width::WidthParams),
    DynamicEq(crate::dynamics::DynEqParams),
    /// 畳み込みリバーブ。本体([`crate::convolver::ConvEngine`])は再生データが持ち、エンジンが通す
    Convolution(ConvParams),
    Resonance(crate::resonance::ResonanceParams),
    VirtualBass(crate::dynamics::VirtualBassParams),
    /// 変調系・質感系(clipper / bitcrush / tremolo / phaser / flanger / trance_gate / auto_filter / volume_shaper)
    Mod(crate::modfx::ModFxParams),
    /// スタジオ系(eq8 / saturation / deesser / gate)
    Studio(crate::studio::StudioParams),
    /// 音程を動かすもの(pitch_shift / harmonizer / pitch_correct)
    Pitch(crate::pitch::PitchParams),
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
    Multiband,
    Transient,
    Limiter,
    Width,
    DynamicEq,
    Convolution,
    Resonance,
    VirtualBass,
    Mod(u8),
    Studio(u8),
    Pitch(u8),
}

/// エフェクト 1 スロット分の状態。全種類のバッファを持ち、起動時に確保して使い回す。
#[derive(Clone)]
pub struct EffectState {
    kind: EffectKind,
    // EQ: 5 バンド × 2ch
    eq: [[SvfState; EQ_BANDS]; 2],
    // Compressor / Sidechain の検出エンベロープ(Compressor は減衰量 dB)
    envelope: f32,
    // Compressor: 分離したピーク検出の 1 段目・RMS の平均・検出側のハイパス(2ch)
    comp_y1: f32,
    comp_ms: f32,
    comp_hp: [SvfState; 2],
    // Compressor: ニーの下端(dB)と、そのリニアの値(下端が変わったときだけ求め直す)
    comp_floor: (f32, f32),
    // Sidechain(ダッカー)のトリガー状態
    duck_pos: f32,
    duck_active: bool,
    key_was_above: bool,
    // Distortion のトーン用 1 次 LP(2ch)
    tone_lp: [f32; 2],
    // Distortion の折り返し対策(tanh の ADAA、2ch)
    adaa: [crate::oversample::AdaaTanh; 2],
    // Tape の飽和を 2 倍のレートで回す(2ch)
    tape_os: [crate::oversample::Halfband; 2],
    // Amp のフィルタ群(2ch)
    amp: [AmpChState; 2],
    // Reverb
    reverb: crate::reverb::FdnState,
    // Delay / Chorus / Tape の共有ディレイバッファ(2ch)
    dly: [Vec<f32>; 2],
    dly_idx: usize,
    dly_lp: [f32; 2],
    // Chorus / Tape の LFO 位相(0..1)
    lfo: [f32; 2],
    // Tape のヒスノイズ用乱数(xorshift32)
    rng: u32,
    // Tape のパチパチ(左右それぞれの減衰中の振幅。符号込み)
    crackle: [f32; 2],
    // マルチバンドコンプ・トランジェントシェイパー
    multiband: crate::dynamics::MultibandState,
    transient: crate::dynamics::TransientState,
    limiter: crate::limiter::LimiterState,
    width: crate::width::WidthState,
    dyn_eq: crate::dynamics::DynEqState,
    resonance: crate::resonance::ResonanceState,
    virtual_bass: crate::dynamics::VirtualBassState,
    /// 変調系・質感系の状態
    modfx: crate::modfx::ModFxState,
    /// スタジオ系・音程の状態
    studio: crate::studio::StudioState,
    pitch: Box<crate::pitch::PitchState>,
    /// つまみの平滑化(ミックス・ゲインなど。種類ごとに使い方が違う)
    sm: [Smoothed; 3],
    /// 共有ディレイバッファを今のエフェクトが使っている / 前に使って汚れている
    dly_user: bool,
    dly_dirty: bool,
    /// 曲の 1 サンプルあたりの tick(テンポに合わせるディレイ用。停止中は 0)
    clock_tps: f64,
    /// ディレイ: いまの長さ(なめらかに追う)・テンポに合わせた最後の長さ・tape の低域の削り・bbd の 2 段目のローパス
    dly_time: Smoothed,
    sync_time: f32,
    dly_hp: [f32; 2],
    dly_lp2: [f32; 2],
    /// bbd: 2 段目のローパスの係数と、それを求めたときの長さ
    bbd: (f32, f32),
    /// コンプ: ゆっくり戻る追従(vca / opto)
    comp_y2: f32,
}

const RNG_SEED: u32 = 0x9E37_79B9;

impl Default for EffectState {
    fn default() -> Self {
        EffectState {
            dly: [vec![0.0; DLY_LEN], vec![0.0; DLY_LEN]],
            modfx: crate::modfx::ModFxState::default(),
            ..Self::light()
        }
    }
}

impl EffectState {
    /// ディレイ系(delay / chorus / tape)のバッファを持たない軽い状態(オフラインで reverb 等を
    /// 何度も掛ける用。1 回あたり 512KB の確保を省く)。ディレイ系を渡すと素通しになる。
    pub fn without_delay_buffers() -> Self {
        EffectState {
            dly: [Vec::new(), Vec::new()],
            ..Self::light()
        }
    }

    fn light() -> Self {
        EffectState {
            kind: EffectKind::None,
            eq: Default::default(),
            envelope: 0.0,
            comp_y1: 0.0,
            comp_ms: 0.0,
            comp_hp: Default::default(),
            comp_floor: (f32::MAX, 0.0),
            duck_pos: 0.0,
            duck_active: false,
            key_was_above: false,
            tone_lp: [0.0; 2],
            adaa: Default::default(),
            tape_os: Default::default(),
            amp: Default::default(),
            reverb: crate::reverb::FdnState::new(),
            dly: [Vec::new(), Vec::new()],
            dly_idx: 0,
            dly_lp: [0.0; 2],
            lfo: [0.0; 2],
            rng: RNG_SEED,
            crackle: [0.0; 2],
            multiband: Default::default(),
            transient: Default::default(),
            limiter: Default::default(),
            width: Default::default(),
            dyn_eq: Default::default(),
            resonance: Default::default(),
            virtual_bass: Default::default(),
            modfx: crate::modfx::ModFxState::light(),
            studio: Default::default(),
            pitch: Default::default(),
            sm: Default::default(),
            dly_user: false,
            dly_dirty: false,
            clock_tps: 0.0,
            dly_time: Smoothed::default(),
            sync_time: 0.0,
            dly_hp: [0.0; 2],
            dly_lp2: [0.0; 2],
            bbd: (f32::NAN, 0.0),
            comp_y2: 0.0,
        }
    }

    /// このブロックの頭の曲の位置(tick)と 1 サンプルあたりの tick(テンポに合わせるエフェクト用。停止中は 0)
    pub fn set_clock(&mut self, tick: f64, ticks_per_sample: f64) {
        self.modfx.set_clock(tick, ticks_per_sample);
        self.clock_tps = ticks_per_sample;
    }

    /// 0..1 の一様乱数(xorshift32)
    fn next_rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng as f32 / u32::MAX as f32
    }

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
            EffectParams::Multiband(_) => EffectKind::Multiband,
            EffectParams::Transient(_) => EffectKind::Transient,
            EffectParams::Limiter(_) => EffectKind::Limiter,
            EffectParams::Width(_) => EffectKind::Width,
            EffectParams::DynamicEq(_) => EffectKind::DynamicEq,
            EffectParams::Convolution(_) => EffectKind::Convolution,
            EffectParams::Resonance(_) => EffectKind::Resonance,
            EffectParams::VirtualBass(_) => EffectKind::VirtualBass,
            EffectParams::Mod(m) => EffectKind::Mod(m.kind()),
            EffectParams::Studio(s) => EffectKind::Studio(s.kind()),
            EffectParams::Pitch(p) => EffectKind::Pitch(p.kind()),
            EffectParams::External => EffectKind::None,
        }
    }

    /// 共有のディレイバッファを使うか
    fn uses_dly(p: &EffectParams) -> bool {
        match p {
            EffectParams::Delay(_)
            | EffectParams::Chorus(_)
            | EffectParams::Tape(_)
            | EffectParams::Pitch(_) => true,
            EffectParams::Compressor(c) => c.ext.lookahead > 0,
            _ => false,
        }
    }

    /// データ差し替えでスロットの中身が変わったときに呼ぶ(アロケーションなし)。
    pub fn ensure_kind(&mut self, p: &EffectParams) {
        let kind = Self::kind_of(p);
        if self.kind != kind {
            self.kind = kind;
            self.eq = Default::default();
            self.envelope = 0.0;
            self.comp_y1 = 0.0;
            self.comp_ms = 0.0;
            self.comp_hp = Default::default();
            self.duck_pos = 0.0;
            self.duck_active = false;
            self.key_was_above = false;
            self.tone_lp = [0.0; 2];
            self.adaa = Default::default();
            self.tape_os = Default::default();
            self.amp = Default::default();
            // 大きなバッファは、その種類に切り替わるときだけ消す(使わない種類の領域に触れて
            // 実メモリにしない。戻ってきたときにはここで消える)
            if kind == EffectKind::Reverb {
                self.reverb.reset();
            }
            if matches!(
                kind,
                EffectKind::Delay | EffectKind::Chorus | EffectKind::Tape
            ) {
                self.dly[0].fill(0.0);
                self.dly[1].fill(0.0);
                self.dly_dirty = false;
            }
            self.dly_idx = 0;
            self.dly_user = false;
            self.comp_y2 = 0.0;
            self.sm = Default::default();
            self.dly_time = Smoothed::default();
            self.sync_time = 0.0;
            self.dly_hp = [0.0; 2];
            self.dly_lp2 = [0.0; 2];
            self.bbd = (f32::NAN, 0.0);
            if matches!(kind, EffectKind::Studio(_)) {
                self.studio.reset();
            }
            if matches!(kind, EffectKind::Pitch(_)) {
                self.pitch.reset();
            }
            self.dly_lp = [0.0; 2];
            self.lfo = [0.0; 2];
            self.rng = RNG_SEED;
            self.crackle = [0.0; 2];
            self.multiband = Default::default();
            self.transient = Default::default();
            self.limiter = Default::default();
            self.width = Default::default();
            self.dyn_eq = Default::default();
            if kind == EffectKind::Resonance {
                self.resonance.reset();
            }
            self.virtual_bass = Default::default();
            if matches!(kind, EffectKind::Mod(_)) {
                self.modfx.reset();
            }
        }
        // 共有のディレイバッファを使い始めるとき、前の種類が残した音を消す
        // (コンプの先読みは種類を変えずに入る・切れるので、ここで見る)
        let uses = Self::uses_dly(p);
        if uses && !self.dly_user {
            if self.dly_dirty {
                self.dly[0].fill(0.0);
                self.dly[1].fill(0.0);
            }
            self.dly_idx = 0;
        }
        self.dly_user = uses;
        if uses && !self.dly[0].is_empty() {
            self.dly_dirty = true;
        }
    }

    /// ステレオ 1 サンプル処理。`key` はサイドチェインの検出信号
    /// (通常はソーストラックのモノ合算。サイドチェイン以外は無視する)。
    pub fn process(&mut self, p: &EffectParams, l: f32, r: f32, key: f32) -> (f32, f32) {
        if self.dly[0].is_empty()
            && matches!(
                p,
                EffectParams::Delay(_)
                    | EffectParams::Chorus(_)
                    | EffectParams::Tape(_)
                    | EffectParams::Pitch(_)
            )
        {
            return (l, r);
        }
        match p {
            EffectParams::External => (l, r),
            EffectParams::Eq(eq) => {
                let ch = |mut s: f32, st: &mut [SvfState; EQ_BANDS]| {
                    for (b, state) in st.iter_mut().enumerate() {
                        if eq.active[b] {
                            s = state.process(&eq.bands[b], eq.smooth, s);
                        }
                    }
                    s
                };
                let [sl, sr] = &mut self.eq;
                (ch(l, sl), ch(r, sr))
            }
            EffectParams::Compressor(c) => {
                // 先読み: 音は遅らせて出し、検出は今の音で走らせる(減衰が音より先に効き始める)
                let (xl, xr) = if c.ext.lookahead > 0 && !self.dly[0].is_empty() {
                    let idx = self.dly_idx;
                    self.dly[0][idx] = l;
                    self.dly[1][idx] = r;
                    self.dly_idx = (idx + 1) & DLY_MASK;
                    let back = idx.wrapping_sub(c.ext.lookahead as usize) & DLY_MASK;
                    (self.dly[0][back], self.dly[1][back])
                } else {
                    (l, r)
                };
                // 検出: 検出側のハイパス → 左右の大きい方(ピーク)か、二乗平均(RMS)
                let (dl, dr) = match &c.sc_hpf {
                    Some(hp) => (
                        self.comp_hp[0].process(hp, 1.0, l),
                        self.comp_hp[1].process(hp, 1.0, r),
                    ),
                    None => (l, r),
                };
                let level = if c.rms {
                    let sq = (dl * dl).max(dr * dr);
                    self.comp_ms = c.rms_coef * self.comp_ms + (1.0 - c.rms_coef) * sq;
                    self.comp_ms.sqrt()
                } else {
                    dl.abs().max(dr.abs())
                };
                // ニーより十分小さければ減衰は 0(対数を取らずに判定する)
                let floor_db = c.threshold_db - 0.5 * c.knee_db;
                if self.comp_floor.0 != floor_db {
                    self.comp_floor = (floor_db, 10.0_f32.powf(floor_db / 20.0) * 0.999);
                }
                let want = if level < self.comp_floor.1 {
                    0.0
                } else {
                    -c.gain_db(20.0 * level.max(1e-6).log10())
                };
                // 減衰量(dB、正)をなめらかで分離したピーク検出で追う
                self.comp_y1 =
                    want.max(c.release_coef * self.comp_y1 + (1.0 - c.release_coef) * want);
                // vca / opto: 長く圧縮していた分だけゆっくり戻る(短いアタックの後は速く戻る)
                let y = if c.ext.slow_weight > 0.0 {
                    let k = if want > self.comp_y2 {
                        c.ext.slow_charge
                    } else {
                        c.ext.slow_coef
                    };
                    self.comp_y2 = k * self.comp_y2 + (1.0 - k) * want;
                    self.comp_y1.max(self.comp_y2 * c.ext.slow_weight)
                } else {
                    self.comp_y1
                };
                self.envelope = c.attack_coef * self.envelope + (1.0 - c.attack_coef) * y;
                let makeup = self.sm[0].next(c.makeup, c.ext.smooth);
                // 減衰がほぼ 0(-100dB 未満の差)なら累乗を省く
                let gain = if self.envelope < 1e-5 {
                    makeup
                } else {
                    10.0_f32.powf(-self.envelope / 20.0) * makeup
                };
                if c.ext.character == CompCharacter::Fet {
                    // FET: 潰すほど奇数次の歪みが乗る(小さい音はほぼそのまま)
                    let k = 1.0 + self.envelope * (1.0 / 12.0);
                    let sat = |x: f32| (x * k).tanh() / k;
                    return (sat(xl * gain), sat(xr * gain));
                }
                (xl * gain, xr * gain)
            }
            EffectParams::Reverb(rv) => {
                let (mut wl, mut wr) = self.reverb.process(rv, l, r);
                if rv.gate > 0.0 {
                    // ゲートリバーブ: 入ってくる音が -34dB を下回ってから gate サンプルで残響を閉じる(約 5ms で開閉)
                    if l.abs().max(r.abs()) > 0.02 {
                        self.duck_pos = 0.0;
                    } else {
                        self.duck_pos += 1.0;
                    }
                    let target = if self.duck_pos < rv.gate { 1.0 } else { 0.0 };
                    self.envelope += (target - self.envelope) * 0.004;
                    wl *= self.envelope;
                    wr *= self.envelope;
                }
                let mix = self.sm[0].next(rv.mix, rv.smooth);
                (l * (1.0 - mix) + wl * mix, r * (1.0 - mix) + wr * mix)
            }
            EffectParams::Distortion(d) => {
                let drive = self.sm[0].next(d.drive, d.smooth);
                let mix = self.sm[1].next(d.mix, d.smooth);
                let level = self.sm[2].next(d.level, d.smooth);
                let mut shape = |x: f32, ch: usize| {
                    // tanh を ADAA で(強く歪ませた高い音の折り返しを減らす。遅れは 0.5 サンプル)
                    let wet = self.adaa[ch].process(x * drive);
                    // 歪みで出た高域のギラつきをトーンで丸める(1 次 LP)
                    self.tone_lp[ch] += (wet - self.tone_lp[ch]) * (1.0 - d.tone_coef);
                    let toned = self.tone_lp[ch];
                    (x * (1.0 - mix) + toned * mix) * level
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
                    // 2 段目は非対称クリップ(偶数次倍音 = 真空管っぽい太さ)。
                    // 歪みで出た倍音が折り返さないよう、2 倍のレートで回す(段間 LP の係数も 2 倍レート用)
                    let AmpChState { os, stage_lp, .. } = st;
                    let s2 = os.run(hp, |u| {
                        let s1 = (u * a.gain * 0.5).tanh();
                        *stage_lp += (s1 - *stage_lp) * a.stage_coef;
                        (*stage_lp * 2.4 + 0.12).tanh() - 0.119_4
                    });
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
            EffectParams::Delay(d) => self.process_delay(d, l, r),
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
                let mix = self.sm[0].next(c.mix, c.smooth);
                (l * (1.0 - mix) + yl * mix, r * (1.0 - mix) + yr * mix)
            }
            EffectParams::Multiband(m) => self.multiband.process(m, l, r),
            EffectParams::Transient(t) => self.transient.process(t, l, r),
            EffectParams::Limiter(m) => self.limiter.process(m, l, r),
            EffectParams::Width(w) => self.width.process(w, l, r),
            EffectParams::DynamicEq(d) => self.dyn_eq.process(d, l, r, key),
            // 本体はエンジンが(再生データの畳み込みの本体で)通す
            EffectParams::Convolution(_) => (l, r),
            EffectParams::Resonance(rp) => self.resonance.process(rp, l, r),
            EffectParams::VirtualBass(vb) => self.virtual_bass.process(vb, l, r),
            EffectParams::Mod(m) => self.modfx.process(m, l, r),
            EffectParams::Studio(sp) => self.studio.process(sp, l, r, key),
            EffectParams::Pitch(pp) => {
                let idx = self.dly_idx;
                let out = self.pitch.process(pp, &mut self.dly, idx, l, r);
                self.dly_idx = (idx + 1) & DLY_MASK;
                out
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
                // レコードのパチパチ: まれに起きる、ごく短く減衰する鋭い音(左右どちらか・両方)
                if t.crackle_rate > 0.0 {
                    let u = self.next_rand();
                    if u < t.crackle_rate {
                        let amp = t.crackle_amp * (0.15 + 0.85 * self.next_rand().powi(3));
                        let sign = if self.next_rand() < 0.5 { -1.0 } else { 1.0 };
                        let side = self.next_rand();
                        // 両方 60%・左だけ 20%・右だけ 20%
                        if side < 0.8 {
                            self.crackle[0] = amp * sign;
                        }
                        if side < 0.6 || side >= 0.8 {
                            self.crackle[1] = amp * sign;
                        }
                    }
                }
                let mut out = [0.0f32; 2];
                for (ch, o) in out.iter_mut().enumerate() {
                    let x = read_frac(&self.dly[ch], idx, delay);
                    // 飽和(小さい音はほぼ素通し、大きい音ほど丸く潰れる)。折り返さないよう 2 倍のレートで
                    let drive = t.drive;
                    let mut y = self.tape_os[ch].run(x, |u| (u * drive).tanh() / drive);
                    self.tone_lp[ch] += (y - self.tone_lp[ch]) * (1.0 - t.tone_coef);
                    y = self.tone_lp[ch];
                    if t.hiss > 0.0 {
                        y += (self.next_rand() - 0.5) * 2.0 * t.hiss;
                    }
                    if self.crackle[ch] != 0.0 {
                        y += self.crackle[ch];
                        // 約 0.1ms で消える(次のサンプルでは符号も反転させて「プチッ」とした形に)
                        self.crackle[ch] *= -0.45;
                        if self.crackle[ch].abs() < 1e-5 {
                            self.crackle[ch] = 0.0;
                        }
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

    /// ディレイ(digital / tape / bbd / multitap)
    fn process_delay(&mut self, d: &DelayParams, l: f32, r: f32) -> (f32, f32) {
        let idx = self.dly_idx;
        // 長さの目標: テンポに合わせるなら曲の速さから(停止中は最後に合わせた長さ)
        let target = if d.sync_ticks > 0.0 && self.clock_tps > 0.0 {
            self.sync_time = ((d.sync_ticks / self.clock_tps) as f32).clamp(2.0, DELAY_MAX);
            self.sync_time
        } else if d.sync_ticks > 0.0 && self.sync_time > 0.0 {
            self.sync_time
        } else {
            d.time
        };
        // 長さが変わったら(テンポ・オートメーション)なめらかに追う。テープ・BBD はゆっくり(音程がうねる)
        let glide = match d.kind {
            DelayKind::Tape | DelayKind::Bbd => d.glide_slow,
            _ => d.glide,
        };
        let time = self.dly_time.next(target, glide);
        let mix = self.sm[0].next(d.mix, d.smooth);
        let feedback = self.sm[1].next(d.feedback, d.smooth);
        let tau = std::f32::consts::TAU;
        let (el, er) = match d.kind {
            DelayKind::Digital => {
                // やまびこは毎回トーンの LP を通る(回を重ねるほど暗くなる)
                for ch in 0..2 {
                    let y = read_frac(&self.dly[ch], idx, time);
                    self.dly_lp[ch] += (y - self.dly_lp[ch]) * (1.0 - d.tone_coef);
                }
                let [el, er] = self.dly_lp;
                let (wl, wr) = if d.ping_pong {
                    // 入力は左へ、左のやまびこは右へ、右は左へ
                    ((l + r) * 0.5 + er * feedback, el * feedback)
                } else {
                    (l + el * feedback, r + er * feedback)
                };
                self.dly[0][idx] = wl;
                self.dly[1][idx] = wr;
                (el, er)
            }
            DelayKind::Tape | DelayKind::Bbd => {
                let ms = 0.001 * d.sample_rate;
                let tape = d.kind == DelayKind::Tape;
                // テープは回転むら(ワウ・フラッター)、BBD はクロックのゆっくりした揺れ
                let wobble = if tape {
                    0.5 * ms * (tau * self.lfo[0]).sin() + 0.03 * ms * (tau * self.lfo[1]).sin()
                } else {
                    0.15 * ms * (tau * self.lfo[0]).sin()
                };
                self.lfo[0] = (self.lfo[0] + d.wow_inc).fract();
                self.lfo[1] = (self.lfo[1] + d.flutter_inc).fract();
                if !tape && ((self.bbd.0 - time).abs() > 0.5 || self.bbd.0.is_nan()) {
                    // BBD は長くするほどクロックが下がり、高域が出なくなる(4096 段の素子を想定)
                    let secs = time / d.sample_rate;
                    let fc = (0.45 * 4096.0 / (2.0 * secs.max(1e-3))).min(0.45 * d.sample_rate);
                    self.bbd = (time, (-tau * fc / d.sample_rate).exp());
                }
                let mut e = [0.0f32; 2];
                for (ch, ech) in e.iter_mut().enumerate() {
                    let y = read_frac(&self.dly[ch], idx, time + wobble);
                    self.dly_lp[ch] += (y - self.dly_lp[ch]) * (1.0 - d.tone_coef);
                    let mut v = self.dly_lp[ch];
                    if tape {
                        // 回を重ねても低音が膨らまないよう、約 120Hz 以下を少し削る
                        self.dly_hp[ch] += (v - self.dly_hp[ch]) * (tau * 120.0 / d.sample_rate);
                        v -= self.dly_hp[ch];
                    } else {
                        self.dly_lp2[ch] += (v - self.dly_lp2[ch]) * (1.0 - self.bbd.1);
                        v = self.dly_lp2[ch];
                    }
                    *ech = v;
                }
                let [el, er] = e;
                // 繰り返しは飽和して丸くなる(フィードバックを上げても発振しきらない)
                let fb = |x: f32| (x * feedback * 1.3).tanh() * (1.0 / 1.3);
                let (wl, wr) = if d.ping_pong {
                    ((l + r) * 0.5 + fb(er), fb(el))
                } else {
                    (l + fb(el), r + fb(er))
                };
                self.dly[0][idx] = wl;
                self.dly[1][idx] = wr;
                (el, er)
            }
            DelayKind::Multitap => {
                // time を taps 等分した位置に読み出し口。後ろほど小さく、ping_pong なら左右交互
                let n = d.taps.clamp(2, 4) as usize;
                let (mut wl, mut wr) = (0.0, 0.0);
                let mut g = 1.0;
                for k in 1..=n {
                    let at = time * k as f32 / n as f32;
                    let yl = read_frac(&self.dly[0], idx, at);
                    let yr = read_frac(&self.dly[1], idx, at);
                    if d.ping_pong {
                        let m = 0.5 * (yl + yr) * g;
                        if k % 2 == 1 {
                            wl += m;
                        } else {
                            wr += m;
                        }
                    } else {
                        wl += yl * g;
                        wr += yr * g;
                    }
                    g *= 0.7;
                }
                // トーン(全体)と、最後の読み出し口からのフィードバック(繰り返すほど暗く)
                self.dly_lp[0] += (wl - self.dly_lp[0]) * (1.0 - d.tone_coef);
                self.dly_lp[1] += (wr - self.dly_lp[1]) * (1.0 - d.tone_coef);
                for ch in 0..2 {
                    let last = read_frac(&self.dly[ch], idx, time);
                    self.dly_lp2[ch] += (last - self.dly_lp2[ch]) * (1.0 - d.tone_coef);
                }
                let (il, ir) = if d.ping_pong {
                    let m = (l + r) * 0.5;
                    (m, m)
                } else {
                    (l, r)
                };
                self.dly[0][idx] = il + self.dly_lp2[0] * feedback;
                self.dly[1][idx] = ir + self.dly_lp2[1] * feedback;
                (self.dly_lp[0], self.dly_lp[1])
            }
        };
        self.dly_idx = (idx + 1) & DLY_MASK;
        // ダッキング: 原音の大きさ(速く上がって約 40ms で下がる)でやまびこを沈める
        let duck = if d.duck_db > 0.0 {
            let lvl = l.abs().max(r.abs());
            self.envelope = lvl.max(self.envelope * 0.9995);
            10.0_f32.powf(-d.duck_db * (self.envelope / 0.25).min(1.0) / 20.0)
        } else {
            1.0
        };
        (
            l * (1.0 - mix) + el * mix * duck,
            r * (1.0 - mix) + er * mix * duck,
        )
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
    ParamSpec {
        name: "hp_freq",
        display_name: "ハイパス",
        unit: Some("Hz"),
        range: ParamRange::Float {
            min: 20.0,
            max: 1000.0,
            default: 20.0,
            skew: Some(0.4),
        },
        description: "これより低い音を 12dB/oct で削る(20 で切る)。ベース・キック以外のトラックの\
            不要な低音(80〜150Hz 以下)を削ると、低域がすっきりして被りが減る。",
    },
    ParamSpec {
        name: "lp_freq",
        display_name: "ローパス",
        unit: Some("Hz"),
        range: ParamRange::Float {
            min: 1000.0,
            max: 20000.0,
            default: 20000.0,
            skew: Some(0.4),
        },
        description: "これより高い音を 12dB/oct で削る(20000 で切る)。刺さる高域やノイズを抑え、\
            音を奥に引っ込める。",
    },
];

pub static MULTIBAND_SPECS: &[ParamSpec] = &[
    ParamSpec {
        name: "low_freq",
        display_name: "低域の分かれ目",
        unit: Some("Hz"),
        range: ParamRange::Float {
            min: 40.0,
            max: 1000.0,
            default: 200.0,
            skew: Some(0.4),
        },
        description: "低域と中域を分ける周波数。キック・ベースだけを低域に入れるなら 120〜250。",
    },
    ParamSpec {
        name: "high_freq",
        display_name: "高域の分かれ目",
        unit: Some("Hz"),
        range: ParamRange::Float {
            min: 1000.0,
            max: 12000.0,
            default: 3000.0,
            skew: Some(0.4),
        },
        description:
            "中域と高域を分ける周波数。ボーカルの刺さり・シンバルを高域に分けるなら 3000〜6000。",
    },
    ParamSpec {
        name: "low_threshold_db",
        display_name: "低域のスレッショルド",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: -60.0,
            max: 0.0,
            default: -20.0,
            skew: None,
        },
        description: "低域がこれを超えると圧縮する。",
    },
    ParamSpec {
        name: "low_ratio",
        display_name: "低域のレシオ",
        unit: None,
        range: ParamRange::Float {
            min: 1.0,
            max: 10.0,
            default: 2.0,
            skew: None,
        },
        description: "低域の圧縮の強さ。1 で圧縮しない。",
    },
    ParamSpec {
        name: "low_gain_db",
        display_name: "低域のゲイン",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: -24.0,
            max: 24.0,
            default: 0.0,
            skew: None,
        },
        description: "圧縮した後の低域の音量。帯域ごとの EQ としても使える。",
    },
    ParamSpec {
        name: "mid_threshold_db",
        display_name: "中域のスレッショルド",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: -60.0,
            max: 0.0,
            default: -20.0,
            skew: None,
        },
        description: "中域がこれを超えると圧縮する。",
    },
    ParamSpec {
        name: "mid_ratio",
        display_name: "中域のレシオ",
        unit: None,
        range: ParamRange::Float {
            min: 1.0,
            max: 10.0,
            default: 2.0,
            skew: None,
        },
        description: "中域の圧縮の強さ。1 で圧縮しない。",
    },
    ParamSpec {
        name: "mid_gain_db",
        display_name: "中域のゲイン",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: -24.0,
            max: 24.0,
            default: 0.0,
            skew: None,
        },
        description: "圧縮した後の中域の音量。帯域ごとの EQ としても使える。",
    },
    ParamSpec {
        name: "high_threshold_db",
        display_name: "高域のスレッショルド",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: -60.0,
            max: 0.0,
            default: -20.0,
            skew: None,
        },
        description: "高域がこれを超えると圧縮する。",
    },
    ParamSpec {
        name: "high_ratio",
        display_name: "高域のレシオ",
        unit: None,
        range: ParamRange::Float {
            min: 1.0,
            max: 10.0,
            default: 2.0,
            skew: None,
        },
        description: "高域の圧縮の強さ。1 で圧縮しない。",
    },
    ParamSpec {
        name: "high_gain_db",
        display_name: "高域のゲイン",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: -24.0,
            max: 24.0,
            default: 0.0,
            skew: None,
        },
        description: "圧縮した後の高域の音量。帯域ごとの EQ としても使える。",
    },
    ParamSpec {
        name: "attack_ms",
        display_name: "アタック",
        unit: Some("ms"),
        range: ParamRange::Float {
            min: 0.1,
            max: 100.0,
            default: 10.0,
            skew: Some(0.5),
        },
        description: "圧縮が効き始める速さ(全帯域共通)。",
    },
    ParamSpec {
        name: "release_ms",
        display_name: "リリース",
        unit: Some("ms"),
        range: ParamRange::Float {
            min: 10.0,
            max: 1000.0,
            default: 120.0,
            skew: Some(0.5),
        },
        description: "圧縮が戻る速さ(全帯域共通)。",
    },
];

pub static TRANSIENT_SPECS: &[ParamSpec] = &[
    ParamSpec {
        name: "attack_db",
        display_name: "アタック",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: -12.0,
            max: 12.0,
            default: 0.0,
            skew: None,
        },
        description: "打点(鳴り始めの一瞬)の増減。上げるとドラムやギターの輪郭が立ち、下げると角が取れて奥に下がる。",
    },
    ParamSpec {
        name: "sustain_db",
        display_name: "サステイン",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: -12.0,
            max: 12.0,
            default: 0.0,
            skew: None,
        },
        description: "余韻の増減。下げるとドラムの響き・部屋鳴りが締まり、上げると太く長く鳴る。",
    },
];

pub static LIMITER_SPECS: &[ParamSpec] = &[
    ParamSpec {
        name: "input_db",
        display_name: "入力",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: 0.0,
            max: 24.0,
            default: 0.0,
            skew: None,
        },
        description: "入力を持ち上げる量。上げるほど上限に張り付いて音圧が上がるが、潰れてダイナミクスが減る。配信は -14 LUFS に正規化されるので、上げすぎても得をしない(analyze_audio の streaming で確かめる)。",
    },
    ParamSpec {
        name: "ceiling_db",
        display_name: "上限",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: -12.0,
            max: 0.0,
            default: -1.0,
            skew: None,
        },
        description: "出力の上限(True Peak。サンプルの間の山も含む)。配信用は -1、音圧の高い曲は -2 が目安。",
    },
    ParamSpec {
        name: "release_ms",
        display_name: "リリース",
        unit: Some("ms"),
        range: ParamRange::Float {
            min: 5.0,
            max: 2000.0,
            default: 100.0,
            skew: Some(0.4),
        },
        description: "音量が戻る速さ。短いと音圧が上がるが歪みやポンピングが出やすく、長いと自然。",
    },
];

pub static WIDTH_SPECS: &[ParamSpec] = &[
    ParamSpec {
        name: "width",
        display_name: "幅",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 2.0,
            default: 1.0,
            skew: None,
        },
        description: "左右の広がり。0 でモノラル、1 でそのまま、2 で広く(左右の差を 2 倍)。広げすぎるとモノラルで痩せる。",
    },
    ParamSpec {
        name: "mono_below_hz",
        display_name: "低域のモノ化",
        unit: Some("Hz"),
        range: ParamRange::Float {
            min: 20.0,
            max: 500.0,
            default: 20.0,
            skew: Some(0.5),
        },
        description: "これより低い音を中央に集める(20 で切る)。ミックス全体やシンセのバスに 100〜150 で、低音の位置が定まりモノラルでも痩せない。",
    },
    ParamSpec {
        name: "decorrelate",
        display_name: "広げる",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 1.0,
            default: 0.0,
            skew: None,
        },
        description: "モノラルの音にも左右の違いを作って広げる(まばらな雑音で畳み込んだ音を左右の差に足す)。モノラルにすると消えて元の音に戻る。打点はにじまないよう弱める。パッド・コーラス・ボーカルの重ねに。",
    },
];

pub static DYNAMIC_EQ_SPECS: &[ParamSpec] = &[
    ParamSpec {
        name: "freq",
        display_name: "周波数",
        unit: Some("Hz"),
        range: ParamRange::Float {
            min: 40.0,
            max: 16000.0,
            default: 3000.0,
            skew: Some(0.3),
        },
        description: "動かす帯域の中心。ボーカルの刺さり(歯擦音)は 5000〜8000、こもりは 200〜400、ボーカルと伴奏の住み分けは 2000〜4000。",
    },
    ParamSpec {
        name: "q",
        display_name: "Q",
        unit: None,
        range: ParamRange::Float {
            min: 0.3,
            max: 10.0,
            default: 1.5,
            skew: Some(0.5),
        },
        description: "帯域の幅。大きいほど狭い。",
    },
    ParamSpec {
        name: "threshold_db",
        display_name: "スレッショルド",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: -60.0,
            max: 0.0,
            default: -30.0,
            skew: None,
        },
        description: "その帯域の音量がこれを超えた分だけ下げる。",
    },
    ParamSpec {
        name: "ratio",
        display_name: "レシオ",
        unit: None,
        range: ParamRange::Float {
            min: 1.0,
            max: 10.0,
            default: 3.0,
            skew: None,
        },
        description: "下げる強さ。",
    },
    ParamSpec {
        name: "range_db",
        display_name: "最大の下げ幅",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: 0.0,
            max: 24.0,
            default: 12.0,
            skew: None,
        },
        description: "下げる量の上限。",
    },
    ParamSpec {
        name: "attack_ms",
        display_name: "アタック",
        unit: Some("ms"),
        range: ParamRange::Float {
            min: 0.5,
            max: 100.0,
            default: 5.0,
            skew: Some(0.5),
        },
        description: "下がり始める速さ。",
    },
    ParamSpec {
        name: "release_ms",
        display_name: "リリース",
        unit: Some("ms"),
        range: ParamRange::Float {
            min: 10.0,
            max: 1000.0,
            default: 120.0,
            skew: Some(0.5),
        },
        description: "戻る速さ。",
    },
    ParamSpec {
        name: "source",
        display_name: "検出のトラック",
        unit: None,
        range: ParamRange::Enum {
            choices: &[],
            default: "",
        },
        description: "空なら自分の音で検出(その帯域が大きいときだけ下げる = 歯擦音やこもりの抑え)。\
            トラック ID を入れると、そのトラックがこの帯域で鳴っている間だけ自分を下げる\
            (伴奏にボーカルのトラックを指定して 2〜4kHz を空ける、など)。\
            例: set_param path=fx/<fx_id>/source value=\"trk_vox001\"",
    },
];

pub static CONVOLUTION_SPECS: &[ParamSpec] = &[
    ParamSpec {
        name: "ir",
        display_name: "響き(IR)",
        unit: None,
        range: ParamRange::Enum {
            choices: &[],
            default: "",
        },
        description: "畳み込む響き(インパルス応答)の音声素材の ID(sha256:…)。部屋・ホール・教会・スプリング・\
            プレートなどを録った WAV を import_ir で取り込むと入る。空なら素通し。",
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
        description: "響きの割合。センド用のバスに挿すなら 1。",
    },
    ParamSpec {
        name: "wet_db",
        display_name: "響きの音量",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: -24.0,
            max: 12.0,
            default: 0.0,
            skew: None,
        },
        description: "響きの音量。IR ごとの大きさはそろえてある。",
    },
    ParamSpec {
        name: "length",
        display_name: "長さ",
        unit: None,
        range: ParamRange::Float {
            min: 0.05,
            max: 1.0,
            default: 1.0,
            skew: None,
        },
        description: "IR の後ろを短くして、響きを短く切る(終わりはなめらかに消す)。変えると響きが一度切れる。",
    },
];

pub static RESONANCE_SPECS: &[ParamSpec] = &[
    ParamSpec {
        name: "depth_db",
        display_name: "深さ",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: 0.0,
            max: 18.0,
            default: 6.0,
            skew: None,
        },
        description: "共鳴を下げる量の上限。",
    },
    ParamSpec {
        name: "threshold_db",
        display_name: "しきい値",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: 0.0,
            max: 12.0,
            default: 3.0,
            skew: None,
        },
        description: "周りより何 dB 出ている所を共鳴とみなすか。小さいほど細かく効く。",
    },
    ParamSpec {
        name: "width",
        display_name: "周りの幅",
        unit: Some("oct"),
        range: ParamRange::Float {
            min: 0.1,
            max: 2.0,
            default: 0.5,
            skew: Some(0.6),
        },
        description: "共鳴かどうかを比べる周りの幅(オクターブ)。狭いと細い鳴きだけ、広いと広めの出っ張りも下げる。",
    },
    ParamSpec {
        name: "low_hz",
        display_name: "下の周波数",
        unit: Some("Hz"),
        range: ParamRange::Float {
            min: 20.0,
            max: 20000.0,
            default: 200.0,
            skew: Some(0.3),
        },
        description: "これより低い所は触らない。",
    },
    ParamSpec {
        name: "high_hz",
        display_name: "上の周波数",
        unit: Some("Hz"),
        range: ParamRange::Float {
            min: 20.0,
            max: 20000.0,
            default: 12000.0,
            skew: Some(0.3),
        },
        description: "これより高い所は触らない。",
    },
    ParamSpec {
        name: "release_ms",
        display_name: "戻り",
        unit: Some("ms"),
        range: ParamRange::Float {
            min: 10.0,
            max: 1000.0,
            default: 80.0,
            skew: Some(0.5),
        },
        description: "共鳴が消えてから元に戻る速さ。",
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
        description: "効かせた音の割合。",
    },
];

pub static VIRTUAL_BASS_SPECS: &[ParamSpec] = &[
    ParamSpec {
        name: "freq",
        display_name: "低音の上限",
        unit: Some("Hz"),
        range: ParamRange::Float {
            min: 30.0,
            max: 250.0,
            default: 90.0,
            skew: Some(0.6),
        },
        description: "これより低い音から倍音を作る。",
    },
    ParamSpec {
        name: "amount",
        display_name: "量",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 1.0,
            default: 0.5,
            skew: None,
        },
        description: "足す倍音の量。上げすぎると濁る。",
    },
    ParamSpec {
        name: "remove_lows",
        display_name: "元の低音を減らす",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 1.0,
            default: 0.0,
            skew: None,
        },
        description: "元の低音をどれだけ減らすか(減らした分は倍音が補う)。スピーカーで出ない低音に余裕を取られないように。",
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
        name: "knee_db",
        display_name: "ニー",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: 0.0,
            max: 24.0,
            default: 6.0,
            skew: None,
        },
        description:
            "スレッショルドの前後で圧縮がなだらかに効き始める幅。0 でカチッと(ハードニー)、\
            大きいほど自然。ボーカルやバスには 6〜12、ドラムの潰しには 0〜3。",
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
    ParamSpec {
        name: "detector",
        display_name: "検出",
        unit: None,
        range: ParamRange::Enum {
            choices: &["peak", "rms"],
            default: "peak",
        },
        description: "音量の測り方。peak は瞬間の山に反応して素早く抑える(ドラム・ピーク管理)、\
            rms は平均的な音量に反応してなめらか(ボーカル・バス・音量をそろえる)。",
    },
    ParamSpec {
        name: "sc_hpf_hz",
        display_name: "検出のハイパス",
        unit: Some("Hz"),
        range: ParamRange::Float {
            min: 20.0,
            max: 500.0,
            default: 20.0,
            skew: Some(0.5),
        },
        description: "音量を測るときだけ低音を削る(20 で切る)。キックやベースの低音でコンプが\
            ポンピングするのを防ぐ。ミックス全体・バスには 80〜150。",
    },
    ParamSpec {
        name: "character",
        display_name: "種類",
        unit: None,
        range: ParamRange::Enum {
            choices: &["clean", "vca", "fet", "opto"],
            default: "clean",
        },
        description: "コンプの癖。clean は色付けのない素直な圧縮。vca は長く潰した後ほどゆっくり戻る(自動リリース)\
            まとまりのある圧縮で、バス・ミックス全体に。fet はとても速く掴み、潰すほど歪みが乗る攻撃的な音で、\
            ドラム・ボーカル・ベースを前へ。opto は遅めに掴んで速く半分戻り残りはゆっくり戻る、なめらかで\
            自然な圧縮で、ボーカル・ベース・パッドに。",
    },
    ParamSpec {
        name: "lookahead_ms",
        display_name: "先読み",
        unit: Some("ms"),
        range: ParamRange::Float {
            min: 0.0,
            max: 10.0,
            default: 0.0,
            skew: None,
        },
        description: "音をこれだけ遅らせて、圧縮を先に効かせ始める(速いアタックでも頭の山を取りこぼさない)。\
            1〜5 でピークを確実に抑える。遅れはエンジンが補正する(オートメーションでは動かせない)。",
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
    ParamSpec {
        name: "predelay_ms",
        display_name: "プリディレイ",
        unit: Some("ms"),
        range: ParamRange::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
            skew: Some(0.5),
        },
        description:
            "原音から残響が始まるまでの間。10〜30ms 空けると、ボーカルや楽器の輪郭が残響に埋もれず\
            前に出る。大きいほど広い空間の印象。",
    },
    ParamSpec {
        name: "gate_ms",
        display_name: "ゲート",
        unit: Some("ms"),
        range: ParamRange::Float {
            min: 0.0,
            max: 600.0,
            default: 0.0,
            skew: None,
        },
        description:
            "ゲートリバーブ: 入ってくる音が途切れてからこの時間で残響を切る(0 で切らない)。\
            80 年代のスネアは size 0.8 以上 + gate 150〜300 で「バシャッ」と大きく短い響き。",
    },
    ParamSpec {
        name: "character",
        display_name: "種類",
        unit: None,
        range: ParamRange::Enum {
            choices: &["room", "plate", "hall", "chamber", "shimmer"],
            default: "room",
        },
        description: "room は部屋の自然な響き。plate は鉄板リバーブ風の密で明るい響きで、\
            ボーカルやスネアに艶を足す定番。hall は大きなホールの長く広い響き(同じ size でも 1.6 倍長い。\
            ストリングス・パッド・バラード)。chamber は石の部屋の密で明るい短めの響き(ボーカル・ドラム)。\
            shimmer はホールの響きが 1 オクターブ上へ昇っていくきらめき(アンビエント・パッド・ギター。shimmer で量)。",
    },
    ParamSpec {
        name: "shimmer",
        display_name: "シマー",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 1.0,
            default: 0.5,
            skew: None,
        },
        description: "character が shimmer のとき、響きをオクターブ上げて戻す量。上げるほどきらめきが積み重なって\
            天井へ昇る。0.3〜0.6 が使いやすい。",
    },
    ParamSpec {
        name: "modulation",
        display_name: "揺らぎ",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 1.0,
            default: 0.0,
            skew: None,
        },
        description: "残響の中をわずかに揺らし、長い残響の金属的な鳴き(特定の音が残るリンギング)を和らげて滑らかにする。\
            0.2〜0.5 でパッド・ボーカル・ピアノの長い残響が自然に。上げすぎると残響の音程がわずかに揺れる(コーラス風)。",
    },
    ParamSpec {
        name: "low_mult",
        display_name: "低域の長さ",
        unit: Some("×"),
        range: ParamRange::Float {
            min: 0.25,
            max: 2.5,
            default: 1.0,
            skew: None,
        },
        description: "250Hz 以下の残響時間の倍率。下げると低域の濁りが減って締まる(ミックスでは 0.5〜0.8 が定番。\
            ベース・キックと響きがぶつからない)。上げると大聖堂のような重く長い低域。",
    },
    ParamSpec {
        name: "high_mult",
        display_name: "高域の長さ",
        unit: Some("×"),
        range: ParamRange::Float {
            min: 0.25,
            max: 2.0,
            default: 1.0,
            skew: None,
        },
        description: "4kHz 以上の残響時間の倍率(damping と一緒に効く)。下げると本物の部屋のように高域が先に消えて\
            落ち着く。上げるとプレートのように明るい響きが長く伸びる(歯擦音が目立つときは下げる)。",
    },
    ParamSpec {
        name: "early",
        display_name: "初期反射",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 1.0,
            default: 0.0,
            skew: None,
        },
        description: "壁からの最初の跳ね返り(数〜数十 ms)の量。上げると部屋の大きさと、音源までの距離感がはっきりする\
            (遠くに置くなら上げて mix も上げる)。0.3〜0.6 で生楽器・ボーカルを部屋に置いた感じ。\
            hall で長く、plate では短く弱い(鉄板には壁が無い)。",
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
    ParamSpec {
        name: "duck_db",
        display_name: "ダッキング",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: 0.0,
            max: 24.0,
            default: 0.0,
            skew: None,
        },
        description: "原音が鳴っている間にやまびこを沈める量(ダッキングディレイ)。歌・リードの言葉を濁さず、\
            伸ばしや句の切れ目でだけやまびこが聞こえる。3〜6 が定番。",
    },
    ParamSpec {
        name: "sync",
        display_name: "テンポ同期",
        unit: None,
        range: ParamRange::Enum {
            choices: crate::modfx::SYNC_CHOICES,
            default: "off",
        },
        description: "やまびこの間隔を曲のテンポに合わせる(1/4 = 4 分、1/8d = 付点 8 分、1/8t = 3 連の 8 分)。\
            off で time_ms を使う。テンポが変わっても付いていく(長さはバッファの上限、48kHz で約 1.3 秒まで)。",
    },
    ParamSpec {
        name: "type",
        display_name: "種類",
        unit: None,
        range: ParamRange::Enum {
            choices: &["digital", "tape", "bbd", "multitap"],
            default: "digital",
        },
        description: "digital はくっきりした繰り返し。tape はテープエコー: 回転むらで少し揺れ、繰り返すほど丸く\
            飽和して低音が膨らまない(ダブ・ロック・ボーカル)。bbd はアナログの BBD: 長くするほど暗くなる\
            温かい繰り返し(シンセ・ギター)。multitap は time を taps 等分した位置に読み出し口を並べた\
            リズミカルな繰り返し(ping_pong で左右交互)。",
    },
    ParamSpec {
        name: "taps",
        display_name: "タップ数",
        unit: None,
        range: ParamRange::Float {
            min: 2.0,
            max: 4.0,
            default: 3.0,
            skew: None,
        },
        description: "multitap の読み出し口の数(2〜4)。3 で 3 連符、4 で 16 分の刻み(time を 4 分にしたとき)。",
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
        name: "crackle",
        display_name: "クラックル",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 1.0,
            default: 0.0,
            skew: None,
        },
        description: "レコードのプチプチ・パチパチというノイズ。0.2〜0.4 で古いレコードらしさ(Lo-fi Hip Hop の定番)、\
            上げるほど頻繁に大きく鳴る。回転むら(wow)を切ってヒスとこれだけ使えばレコード風になる。",
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
        "multiband" => Some(MULTIBAND_SPECS),
        "transient" => Some(TRANSIENT_SPECS),
        "limiter" => Some(LIMITER_SPECS),
        "width" => Some(WIDTH_SPECS),
        "dynamic_eq" => Some(DYNAMIC_EQ_SPECS),
        "convolution" => Some(CONVOLUTION_SPECS),
        "resonance" => Some(RESONANCE_SPECS),
        "virtual_bass" => Some(VIRTUAL_BASS_SPECS),
        other => crate::modfx::specs(other)
            .or_else(|| crate::studio::specs(other))
            .or_else(|| crate::pitch::specs(other)),
    }
}

/// エフェクトカタログ(MCP `list_params` 用)。
pub fn effect_catalog() -> Vec<crate::params::InstrumentInfo> {
    vec![
        crate::params::InstrumentInfo {
            name: "eq",
            description: "EQ(ハイパス + 低域シェルフ + 中域ピーク + 高域シェルフ + ローパス)。\
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
        crate::params::InstrumentInfo {
            name: "multiband",
            description: "マルチバンドコンプ。低・中・高の 3 帯域に分けて、帯域ごとに圧縮と音量を変える。\
                低音だけ暴れるベース、刺さる高域だけ抑えたいボーカル、マスターの帯域ごとの整えに。\
                圧縮しなければ(ratio 1)元の音と同じ。",
            params: MULTIBAND_SPECS,
            articulations: &[],
        },
        crate::params::InstrumentInfo {
            name: "transient",
            description: "トランジェントシェイパー。音量に関係なく、打点(アタック)と余韻(サステイン)を\
                別々に増減する。ドラムの輪郭を立てる・響きを締める、ギターのピッキングを目立たせるなどに。",
            params: TRANSIENT_SPECS,
            articulations: &[],
        },
        crate::params::InstrumentInfo {
            name: "limiter",
            description: "True Peak リミッタ(先読み)。サンプルの間の山も含めて、出力を上限(ceiling_db)より\
                上に出さない。マスターの最後に挿して音圧と安全を整える定番。約 1ms 遅れる(エンジンが遅延補正する)。",
            params: LIMITER_SPECS,
            articulations: &[],
        },
        crate::params::InstrumentInfo {
            name: "width",
            description: "ステレオの幅(M/S)。左右の広がりを狭める・広げる、低域だけ中央に集める、\
                モノラルの音を広げる。広げてもモノラルにすると元の音に戻る作り(モノラルで音が消えない)。",
            params: WIDTH_SPECS,
            articulations: &[],
        },
        crate::params::InstrumentInfo {
            name: "dynamic_eq",
            description: "ダイナミック EQ(1 バンド)。その帯域が大きいときだけベルで下げる(歯擦音・こもり・\
                ギターの耳障りな帯域を、鳴っているときだけ抑える)。source に別トラックを入れると、\
                そのトラックがその帯域で鳴っている間だけ下げる(帯域を絞ったダッキング)。",
            params: DYNAMIC_EQ_SPECS,
            articulations: &[],
        },
        crate::params::InstrumentInfo {
            name: "convolution",
            description: "畳み込みリバーブ。実在の部屋・ホール・機材の響き(インパルス応答 IR の WAV)をそのまま音に重ねる。\
                本物の空間の質感が欲しいときに(reverb より重い)。IR は import_ir で取り込む。約 11ms 遅れる(遅延補正される)。",
            params: CONVOLUTION_SPECS,
            articulations: &[],
        },
        crate::params::InstrumentInfo {
            name: "resonance",
            description: "共鳴抑制。周りより細く出っ張った帯域(耳につく鳴き・部屋の響きのこもり・ボーカルの刺さり)だけを、\
                鳴っている間だけ自動で下げる。EQ で一点を削るより自然。約 21ms 遅れる(遅延補正される)。",
            params: RESONANCE_SPECS,
            articulations: &[],
        },
        crate::params::InstrumentInfo {
            name: "virtual_bass",
            description: "仮想低音。低音から倍音を作って足し、スマホやノート PC のように低音が出ない機器でも\
                ベースやキックがあるように聞かせる(耳が倍音の並びから基音を補う)。ベース・キックのトラックかマスターに。",
            params: VIRTUAL_BASS_SPECS,
            articulations: &[],
        },
    ]
    .into_iter()
    .chain(crate::modfx::catalog())
    .chain(crate::studio::catalog())
    .chain(crate::pitch::catalog())
    .collect()
}

/// 畳み込みリバーブのつまみ(本体の番号は再生データの中の位置)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConvParams {
    pub index: u32,
    pub mix: f32,
    /// 響きの音量(リニア)
    pub wet: f32,
    pub latency: u32,
}

impl ConvParams {
    /// つまみ(mix / wet_db)を読む。本体の番号と遅れは呼ぶ側が入れる
    pub fn from_map(map: &ParamMap, index: u32, latency: u32) -> Self {
        let s = CONVOLUTION_SPECS;
        ConvParams {
            index,
            mix: get(map, s, "mix").clamp(0.0, 1.0),
            wet: 10.0_f32.powf(get(map, s, "wet_db").clamp(-24.0, 12.0) / 20.0),
            latency,
        }
    }
}

/// 畳み込みリバーブの IR の長さの設定(0.05〜1)
pub fn convolution_length(map: &ParamMap) -> f32 {
    get(map, CONVOLUTION_SPECS, "length").clamp(0.05, 1.0)
}

/// 選択肢のパラメータ(無ければ既定)
fn get_choice<'a>(map: &'a ParamMap, specs: &[ParamSpec], name: &str) -> &'a str {
    if let Some(ParamValue::Enum(v)) = map.get(name) {
        return v.as_str();
    }
    match specs.iter().find(|s| s.name == name).map(|s| &s.range) {
        Some(ParamRange::Enum { default, .. }) => default,
        _ => "",
    }
}

/// コンプの検出側のハイパス(20Hz 以下なら掛けない)
fn comp_sc_hpf(hz: f32, sample_rate: f32) -> Option<SvfCoeffs> {
    (hz > 20.0).then(|| SvfCoeffs::high_pass(sample_rate, hz.min(1000.0)))
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
    /// 検出に別トラックの音を使うエフェクト(サイドチェイン・ダイナミック EQ)の、そのトラックの index
    pub fn key_source(&self) -> Option<u32> {
        match self {
            EffectParams::Sidechain(sc) => Some(sc.source_track),
            EffectParams::DynamicEq(d) if d.source_track != u32::MAX => Some(d.source_track),
            EffectParams::Studio(s) => s.key_source(),
            _ => None,
        }
    }

    /// 処理の遅れ(サンプル)。エンジンの遅延補正に使う(先読みするリミッタだけ 0 でない)
    pub fn latency(&self) -> u32 {
        match self {
            EffectParams::Limiter(p) => p.latency(),
            EffectParams::Convolution(p) => p.latency,
            EffectParams::Resonance(p) => p.latency(),
            EffectParams::Pitch(p) => p.latency(),
            EffectParams::Compressor(c) => c.ext.lookahead,
            _ => 0,
        }
    }

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
                    "hp_freq" => r.hp_freq = v,
                    "lp_freq" => r.lp_freq = v,
                    _ => return false,
                }
                *p = EqParams::from_raw(sample_rate, r);
            }
            EffectParams::Compressor(p) => match name {
                "threshold_db" => p.threshold_db = v.clamp(-40.0, 0.0),
                "ratio" => p.ratio = v.clamp(1.0, 20.0),
                "knee_db" => p.knee_db = v.clamp(0.0, 24.0),
                "attack_ms" | "release_ms" => {
                    if name == "attack_ms" {
                        p.ext.attack_ms = v;
                    } else {
                        p.ext.release_ms = v;
                    }
                    (p.attack_coef, p.release_coef) = p.ext.coefs(sample_rate);
                }
                "makeup_db" => p.makeup = db(v.clamp(0.0, 24.0)),
                "sc_hpf_hz" => {
                    p.sc_hpf_hz = v;
                    p.sc_hpf = comp_sc_hpf(v, sample_rate);
                }
                _ => return false,
            },
            EffectParams::Reverb(p) => {
                let mut raw = p.raw;
                match name {
                    "mix" => {
                        p.mix = v.clamp(0.0, 1.0);
                        return true;
                    }
                    "size" => raw.size = v.clamp(0.0, 1.0),
                    "damping" => raw.damping = v.clamp(0.0, 1.0),
                    "predelay_ms" => raw.predelay_ms = v.clamp(0.0, crate::reverb::PREDELAY_MAX_MS),
                    "gate_ms" => {
                        p.gate = v.clamp(0.0, 600.0) * 0.001 * sample_rate;
                        return true;
                    }
                    "shimmer" => raw.shimmer = v.clamp(0.0, 1.0),
                    "modulation" => raw.modulation = v.clamp(0.0, 1.0),
                    "low_mult" => raw.low_mult = v.clamp(0.25, 2.5),
                    "high_mult" => raw.high_mult = v.clamp(0.25, 2.0),
                    "early" => raw.early = v.clamp(0.0, 1.0),
                    _ => return false,
                }
                let gate = p.gate;
                *p = ReverbParams::new(p.mix, raw);
                p.gate = gate;
            }
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
                "duck_db" => p.duck_db = v.clamp(0.0, 24.0),
                "tone" => p.tone_coef = (-tau * v.clamp(1000.0, 16000.0) / sample_rate).exp(),
                "taps" => p.taps = v.clamp(2.0, 4.0).round() as u8,
                _ => return false,
            },
            EffectParams::Chorus(p) => match name {
                "rate_hz" => p.rate_inc = v.clamp(0.05, 5.0) / sample_rate,
                "depth_ms" => p.depth = v.clamp(0.0, 8.0) * 0.001 * sample_rate,
                "delay_ms" => p.base = v.clamp(3.0, 30.0) * 0.001 * sample_rate,
                "mix" => p.mix = v.clamp(0.0, 1.0),
                _ => return false,
            },
            EffectParams::Multiband(p) => {
                let mut r = p.raw;
                let band = |n: &str| match n {
                    "low" => Some(0),
                    "mid" => Some(1),
                    "high" => Some(2),
                    _ => None,
                };
                match name {
                    "low_freq" => r.low_freq = v,
                    "high_freq" => r.high_freq = v,
                    "attack_ms" => r.attack_ms = v,
                    "release_ms" => r.release_ms = v,
                    _ => {
                        let Some((b, what)) = name.split_once('_') else {
                            return false;
                        };
                        let Some(b) = band(b) else {
                            return false;
                        };
                        match what {
                            "threshold_db" => r.threshold_db[b] = v,
                            "ratio" => r.ratio[b] = v,
                            "gain_db" => r.gain_db[b] = v,
                            _ => return false,
                        }
                    }
                }
                *p = crate::dynamics::MultibandParams::new(r);
            }
            EffectParams::Mod(m) => return m.set_continuous(name, v, sample_rate),
            EffectParams::Studio(m) => return m.set_continuous(name, v, sample_rate),
            EffectParams::Pitch(m) => return m.set_continuous(name, v, sample_rate),
            EffectParams::VirtualBass(p) => {
                let (mut f, mut a, mut rl) = (p.freq, p.amount, p.remove_lows);
                match name {
                    "freq" => f = v,
                    "amount" => a = v,
                    "remove_lows" => rl = v,
                    _ => return false,
                }
                *p = crate::dynamics::VirtualBassParams::new(f, a, rl, sample_rate);
            }
            EffectParams::Resonance(p) => {
                let (mut d, mut t, mut w, mut lo, mut hi, mut rel, mut m) = (
                    p.depth_db,
                    p.threshold_db,
                    p.width,
                    p.low_hz,
                    p.high_hz,
                    p.release_ms,
                    p.mix,
                );
                match name {
                    "depth_db" => d = v,
                    "threshold_db" => t = v,
                    "width" => w = v,
                    "low_hz" => lo = v,
                    "high_hz" => hi = v,
                    "release_ms" => rel = v,
                    "mix" => m = v,
                    _ => return false,
                }
                *p = crate::resonance::ResonanceParams::new(d, t, lo, hi, rel, m, w, sample_rate);
            }
            EffectParams::Convolution(p) => match name {
                "mix" => p.mix = v.clamp(0.0, 1.0),
                "wet_db" => p.wet = db(v.clamp(-24.0, 12.0)),
                _ => return false,
            },
            EffectParams::DynamicEq(p) => {
                let mut r = p.raw;
                match name {
                    "freq" => r.freq = v,
                    "q" => r.q = v,
                    "threshold_db" => r.threshold_db = v,
                    "ratio" => r.ratio = v,
                    "range_db" => r.range_db = v,
                    "attack_ms" => r.attack_ms = v,
                    "release_ms" => r.release_ms = v,
                    _ => return false,
                }
                *p = crate::dynamics::DynEqParams::new(r, p.source_track);
            }
            EffectParams::Width(p) => {
                let (mut w, mut m, mut d) = (p.width, p.mono_below_hz, p.decorrelate);
                match name {
                    "width" => w = v,
                    "mono_below_hz" => m = v,
                    "decorrelate" => d = v,
                    _ => return false,
                }
                *p = crate::width::WidthParams::new(w, m, d, p.sample_rate);
            }
            EffectParams::Limiter(p) => {
                let (mut i, mut c, mut r) = (p.input_db, p.ceiling_db, p.release_ms);
                match name {
                    "input_db" => i = v,
                    "ceiling_db" => c = v,
                    "release_ms" => r = v,
                    _ => return false,
                }
                *p = crate::limiter::LimiterParams::new(i, c, r, p.sample_rate);
            }
            EffectParams::Transient(p) => match name {
                "attack_db" => {
                    *p = crate::dynamics::TransientParams::new(v, p.sustain_db, p.sample_rate)
                }
                "sustain_db" => {
                    *p = crate::dynamics::TransientParams::new(p.attack_db, v, p.sample_rate)
                }
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
                "crackle" => {
                    (p.crackle_rate, p.crackle_amp) = tape_crackle(v, sample_rate);
                }
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

/// クラックル量 → (1 サンプルあたりの発生確率, 最大振幅)。1.0 で毎秒約 30 回・最大 0.25
fn tape_crackle(c: f32, sample_rate: f32) -> (f32, f32) {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.0 {
        return (0.0, 0.0);
    }
    (
        c * c * 30.0 / sample_rate + c * 2.0 / sample_rate,
        0.05 + 0.2 * c,
    )
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
                    hp_freq: get(map, s, "hp_freq"),
                    lp_freq: get(map, s, "lp_freq"),
                },
            )))
        }
        "compressor" => {
            let s = COMPRESSOR_SPECS;
            let hpf = get(map, s, "sc_hpf_hz");
            let mut c = CompressorParams {
                threshold_db: get(map, s, "threshold_db").clamp(-40.0, 0.0),
                ratio: get(map, s, "ratio").clamp(1.0, 20.0),
                knee_db: get(map, s, "knee_db").clamp(0.0, 24.0),
                attack_coef: time_coef(get(map, s, "attack_ms"), sample_rate),
                release_coef: time_coef(get(map, s, "release_ms"), sample_rate),
                makeup: 10.0_f32.powf(get(map, s, "makeup_db").clamp(0.0, 24.0) / 20.0),
                rms: get_choice(map, s, "detector") == "rms",
                rms_coef: time_coef(10.0, sample_rate),
                sc_hpf: comp_sc_hpf(hpf, sample_rate),
                sc_hpf_hz: hpf,
                ext: CompExtra::default(),
            };
            let la = get(map, s, "lookahead_ms").clamp(0.0, 10.0);
            c.ext = CompExtra {
                character: match get_choice(map, s, "character") {
                    "vca" => CompCharacter::Vca,
                    "fet" => CompCharacter::Fet,
                    "opto" => CompCharacter::Opto,
                    _ => CompCharacter::Clean,
                },
                lookahead: (la * 0.001 * sample_rate).round() as u32,
                lookahead_ms: la,
                attack_ms: get(map, s, "attack_ms"),
                release_ms: get(map, s, "release_ms"),
                slow_charge: 0.0,
                slow_coef: 0.0,
                slow_weight: 0.0,
                smooth: smooth_coef(sample_rate),
            };
            (c.attack_coef, c.release_coef) = c.ext.coefs(sample_rate);
            Some(EffectParams::Compressor(c))
        }
        "reverb" => {
            let s = REVERB_SPECS;
            let mut rp = ReverbParams::new(
                get(map, s, "mix"),
                crate::reverb::ReverbRaw {
                    size: get(map, s, "size").clamp(0.0, 1.0),
                    damping: get(map, s, "damping").clamp(0.0, 1.0),
                    predelay_ms: get(map, s, "predelay_ms")
                        .clamp(0.0, crate::reverb::PREDELAY_MAX_MS),
                    character: crate::reverb::ReverbCharacter::parse(get_choice(
                        map,
                        s,
                        "character",
                    )),
                    shimmer: get(map, s, "shimmer"),
                    modulation: get(map, s, "modulation").clamp(0.0, 1.0),
                    low_mult: get(map, s, "low_mult").clamp(0.25, 2.5),
                    high_mult: get(map, s, "high_mult").clamp(0.25, 2.0),
                    early: get(map, s, "early").clamp(0.0, 1.0),
                    sample_rate,
                },
            );
            rp.gate = get(map, s, "gate_ms").clamp(0.0, 600.0) * 0.001 * sample_rate;
            Some(EffectParams::Reverb(rp))
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
                smooth: smooth_coef(sample_rate),
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
                // 段間 LP はプリアンプと一緒に 2 倍のレートで回る
                stage_coef: 1.0 - (-tau * 6000.0 / (2.0 * sample_rate)).exp(),
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
                duck_db: get(map, s, "duck_db").clamp(0.0, 24.0),
                kind: match get_choice(map, s, "type") {
                    "tape" => DelayKind::Tape,
                    "bbd" => DelayKind::Bbd,
                    "multitap" => DelayKind::Multitap,
                    _ => DelayKind::Digital,
                },
                sync_ticks: glaux_core::meter::sync_ticks(get_choice(map, s, "sync")),
                taps: get(map, s, "taps").clamp(2.0, 4.0).round() as u8,
                smooth: smooth_coef(sample_rate),
                // 長さの変化を追う時間: 30ms(digital・multitap)/ 250ms(tape・bbd)
                glide: 1.0 - (-1.0 / (0.03 * sample_rate)).exp(),
                glide_slow: 1.0 - (-1.0 / (0.25 * sample_rate)).exp(),
                wow_inc: 0.6 / sample_rate,
                flutter_inc: 5.5 / sample_rate,
                sample_rate,
            }))
        }
        "chorus" => {
            let s = CHORUS_SPECS;
            Some(EffectParams::Chorus(ChorusParams {
                rate_inc: get(map, s, "rate_hz").clamp(0.05, 5.0) / sample_rate,
                depth: get(map, s, "depth_ms").clamp(0.0, 8.0) * 0.001 * sample_rate,
                base: get(map, s, "delay_ms").clamp(3.0, 30.0) * 0.001 * sample_rate,
                mix: get(map, s, "mix").clamp(0.0, 1.0),
                smooth: smooth_coef(sample_rate),
            }))
        }
        "multiband" => {
            let s = MULTIBAND_SPECS;
            let per = |what: &str| -> [f32; 3] {
                ["low", "mid", "high"].map(|b| get(map, s, &format!("{b}_{what}")))
            };
            Some(EffectParams::Multiband(
                crate::dynamics::MultibandParams::new(crate::dynamics::MultibandRaw {
                    low_freq: get(map, s, "low_freq"),
                    high_freq: get(map, s, "high_freq"),
                    threshold_db: per("threshold_db"),
                    ratio: per("ratio"),
                    gain_db: per("gain_db"),
                    attack_ms: get(map, s, "attack_ms"),
                    release_ms: get(map, s, "release_ms"),
                    sample_rate,
                }),
            ))
        }
        "dynamic_eq" => {
            let s = DYNAMIC_EQ_SPECS;
            let source_track = match map.get("source") {
                Some(ParamValue::Enum(id)) if !id.is_empty() => {
                    resolve_track(id).unwrap_or(u32::MAX)
                }
                _ => u32::MAX,
            };
            Some(EffectParams::DynamicEq(crate::dynamics::DynEqParams::new(
                crate::dynamics::DynEqRaw {
                    freq: get(map, s, "freq"),
                    q: get(map, s, "q"),
                    threshold_db: get(map, s, "threshold_db").clamp(-60.0, 0.0),
                    ratio: get(map, s, "ratio").clamp(1.0, 10.0),
                    range_db: get(map, s, "range_db").clamp(0.0, 24.0),
                    attack_ms: get(map, s, "attack_ms"),
                    release_ms: get(map, s, "release_ms"),
                    sample_rate,
                },
                source_track,
            )))
        }
        other if crate::modfx::specs(other).is_some() => {
            crate::modfx::bake(other, map, sample_rate).map(EffectParams::Mod)
        }
        other if crate::studio::specs(other).is_some() => {
            crate::studio::bake(other, map, sample_rate, resolve_track).map(EffectParams::Studio)
        }
        other if crate::pitch::specs(other).is_some() => {
            crate::pitch::bake(other, map, sample_rate).map(EffectParams::Pitch)
        }
        "virtual_bass" => {
            let s = VIRTUAL_BASS_SPECS;
            Some(EffectParams::VirtualBass(
                crate::dynamics::VirtualBassParams::new(
                    get(map, s, "freq"),
                    get(map, s, "amount"),
                    get(map, s, "remove_lows"),
                    sample_rate,
                ),
            ))
        }
        "resonance" => {
            let s = RESONANCE_SPECS;
            Some(EffectParams::Resonance(
                crate::resonance::ResonanceParams::new(
                    get(map, s, "depth_db"),
                    get(map, s, "threshold_db"),
                    get(map, s, "low_hz"),
                    get(map, s, "high_hz"),
                    get(map, s, "release_ms"),
                    get(map, s, "mix"),
                    get(map, s, "width"),
                    sample_rate,
                ),
            ))
        }
        "width" => {
            let s = WIDTH_SPECS;
            Some(EffectParams::Width(crate::width::WidthParams::new(
                get(map, s, "width"),
                get(map, s, "mono_below_hz"),
                get(map, s, "decorrelate"),
                sample_rate,
            )))
        }
        "limiter" => {
            let s = LIMITER_SPECS;
            Some(EffectParams::Limiter(crate::limiter::LimiterParams::new(
                get(map, s, "input_db"),
                get(map, s, "ceiling_db"),
                get(map, s, "release_ms"),
                sample_rate,
            )))
        }
        "transient" => {
            let s = TRANSIENT_SPECS;
            Some(EffectParams::Transient(
                crate::dynamics::TransientParams::new(
                    get(map, s, "attack_db"),
                    get(map, s, "sustain_db"),
                    sample_rate,
                ),
            ))
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
                crackle_rate: tape_crackle(get(map, s, "crackle"), sample_rate).0,
                crackle_amp: tape_crackle(get(map, s, "crackle"), sample_rate).1,
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
            ("compressor", "knee_db", 0.0),
            ("compressor", "attack_ms", 3.0),
            ("compressor", "sc_hpf_hz", 120.0),
            ("eq", "hp_freq", 100.0),
            ("eq", "lp_freq", 8000.0),
            ("reverb", "mix", 0.8),
            ("reverb", "size", 0.9),
            ("reverb", "predelay_ms", 25.0),
            ("reverb", "modulation", 0.4),
            ("reverb", "low_mult", 0.5),
            ("reverb", "high_mult", 1.6),
            ("reverb", "early", 0.5),
            ("distortion", "drive_db", 30.0),
            ("amp", "tone", 0.2),
            ("sidechain", "duck_db", 12.0),
            ("delay", "time_ms", 250.0),
            ("delay", "tone", 3000.0),
            ("chorus", "depth_ms", 5.0),
            ("tape", "wow", 0.8),
            ("tape", "bits", 8.0),
            ("tape", "crackle", 0.4),
            ("multiband", "low_ratio", 4.0),
            ("multiband", "high_freq", 5000.0),
            ("multiband", "mid_gain_db", -3.0),
            ("transient", "attack_db", 6.0),
            ("transient", "sustain_db", -4.0),
            ("limiter", "input_db", 6.0),
            ("limiter", "ceiling_db", -2.0),
            ("width", "width", 1.5),
            ("width", "mono_below_hz", 120.0),
            ("width", "decorrelate", 0.4),
            ("dynamic_eq", "freq", 6000.0),
            ("dynamic_eq", "threshold_db", -20.0),
            ("resonance", "depth_db", 9.0),
            ("resonance", "low_hz", 400.0),
            ("virtual_bass", "amount", 0.8),
            ("virtual_bass", "freq", 120.0),
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

    /// リバーブの最初の反響は、サンプルレートが違っても同じ秒数で返ってくる
    #[test]
    fn reverb_delay_follows_sample_rate() {
        let none = |_: &str| None;
        let first_echo_secs = |sr: f32| {
            let p = bake_effect(&effect("reverb", &[("mix", 1.0)]), sr, &none).unwrap();
            let mut st = EffectState::without_delay_buffers();
            st.ensure_kind(&p);
            let mut t = 0usize;
            loop {
                let x = if t == 0 { 1.0 } else { 0.0 };
                let (l, _) = st.process(&p, x, x, 0.0);
                if l.abs() > 1e-6 && t > 0 {
                    return t as f32 / sr;
                }
                t += 1;
                assert!(t < 10_000, "反響が返ってこない");
            }
        };
        let a = first_echo_secs(48_000.0);
        let b = first_echo_secs(44_100.0);
        assert!((a - b).abs() < 0.0005, "48k {a:.4} 秒 / 44.1k {b:.4} 秒");
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
    fn eq_bell_boost_matches_gain_and_flat_is_transparent() {
        // 1kHz を +6dB 持ち上げると、1kHz はほぼ 2 倍(+6dB)、既定値(全部 0)は素通し
        let p = bake(&effect("eq", &[("mid_gain_db", 6.0), ("mid_freq", 1000.0)])).unwrap();
        let boosted = rms_through(&p, 1000.0, 19200);
        let flat = bake(&effect("eq", &[])).unwrap();
        let through = rms_through(&flat, 1000.0, 19200);
        let db = 20.0 * (boosted / through).log10();
        assert!((db - 6.0).abs() < 0.3, "ベルの中心は +6dB: {db:.2}");
        assert!(
            (through - 0.5 / 2.0_f32.sqrt()).abs() < 1e-3,
            "素通し: {through}"
        );
    }

    #[test]
    fn eq_magnitude_matches_the_processed_level() {
        // 計算した特性と、実際に通したときの音量が一致する
        let e = effect(
            "eq",
            &[
                ("low_gain_db", 4.0),
                ("mid_gain_db", -6.0),
                ("mid_freq", 1500.0),
                ("high_gain_db", 3.0),
                ("hp_freq", 60.0),
            ],
        );
        let p = bake(&e).unwrap();
        let EffectParams::Eq(eq) = &p else { panic!() };
        let flat = rms_through(&bake(&effect("eq", &[])).unwrap(), 1000.0, 19200);
        for f in [40.0f32, 120.0, 800.0, 1500.0, 5000.0, 12000.0] {
            let got = 20.0 * (rms_through(&p, f, 19200) / flat).log10();
            let want = eq.magnitude_db(48_000.0, f);
            assert!(
                (got - want).abs() < 0.3,
                "{f}Hz: 通した {got:.2} / 計算 {want:.2}"
            );
        }
    }

    #[test]
    fn eq_high_and_low_pass() {
        let p = bake(&effect("eq", &[("hp_freq", 200.0), ("lp_freq", 4000.0)])).unwrap();
        let ref_rms = 0.5 / 2.0_f32.sqrt();
        let db = |f: f32| 20.0 * (rms_through(&p, f, 19200) / ref_rms).log10();
        // 12dB/oct: 1 オクターブ下・上でおよそ -12dB
        assert!(db(50.0) < -20.0, "50Hz はハイパスで削れる: {:.1}", db(50.0));
        assert!(
            db(1000.0).abs() < 0.5,
            "通過域はそのまま: {:.1}",
            db(1000.0)
        );
        assert!(
            db(16000.0) < -20.0,
            "16kHz はローパスで削れる: {:.1}",
            db(16000.0)
        );
    }

    #[test]
    fn compressor_static_curve_and_soft_knee() {
        // 一定の振幅の直流で、落ち着いた後の出力レベルが静特性どおりになる
        let settle = |params: &[(&str, f64)], level_db: f32| {
            let p = bake(&effect("compressor", params)).unwrap();
            let mut st = EffectState::default();
            st.ensure_kind(&p);
            let x = 10.0_f32.powf(level_db / 20.0);
            let mut y = 0.0;
            for _ in 0..48_000 {
                y = st.process(&p, x, x, 0.0).0;
            }
            20.0 * y.log10()
        };
        let hard = [("threshold_db", -20.0), ("ratio", 4.0), ("knee_db", 0.0)];
        // -8dB 入力 = 12dB 超え → 4:1 で 3dB 超え = -17dB
        assert!((settle(&hard, -8.0) + 17.0).abs() < 0.2);
        // スレッショルド以下は素通し
        assert!((settle(&hard, -30.0) + 30.0).abs() < 0.05);
        // ソフトニー: スレッショルドちょうどでも少し効く(ハードニーは効かない)
        let soft = [("threshold_db", -20.0), ("ratio", 4.0), ("knee_db", 12.0)];
        assert!(settle(&soft, -20.0) < -20.5);
        assert!((settle(&hard, -20.0) + 20.0).abs() < 0.05);
    }

    #[test]
    fn compressor_sidechain_hpf_ignores_low_end() {
        // 検出のハイパスを掛けると、40Hz の大きな低音ではほとんど圧縮しない
        let params = |hpf: f64| {
            bake(&effect(
                "compressor",
                &[("threshold_db", -20.0), ("ratio", 8.0), ("sc_hpf_hz", hpf)],
            ))
            .unwrap()
        };
        let plain = rms_through(&params(20.0), 40.0, 48_000);
        let hpf = rms_through(&params(300.0), 40.0, 48_000);
        assert!(hpf > plain * 1.5, "低音で潰れない: {hpf} vs {plain}");
    }

    #[test]
    fn hermite_read_is_accurate_between_samples() {
        // ゆっくりしたサイン波を小数遅延で読むと、線形補間より正確に元の値が出る
        let mut buf = vec![0.0f32; DLY_LEN];
        let w = 0.3f32;
        let idx = 1000usize;
        for k in 0..64 {
            // idx - 1 - k に「k + 1 サンプル前」の値
            buf[(idx - 1 - k) & DLY_MASK] = (w * -((k + 1) as f32)).sin();
        }
        for d in [2.25f32, 3.5, 7.75, 20.5] {
            let got = read_frac(&buf, idx, d);
            let want = (w * -d).sin();
            let (d0, t) = (d.floor(), d - d.floor());
            let linear = (w * -d0).sin() * (1.0 - t) + (w * -(d0 + 1.0)).sin() * t;
            let err = (got - want).abs();
            assert!(err < 1e-3, "遅延 {d}: {got} vs {want}");
            assert!(
                err * 5.0 < (linear - want).abs(),
                "線形補間より正確: 遅延 {d}"
            );
        }
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
    fn gated_reverb_and_ducking_delay() {
        // ゲート: 短い音の後、ゲートの時間(100ms)を過ぎると残響が消える
        let p = bake(&effect(
            "reverb",
            &[("size", 0.9), ("mix", 1.0), ("gate_ms", 100.0)],
        ))
        .unwrap();
        let mut st = EffectState::default();
        let out: Vec<f32> = (0..48_000)
            .map(|i| st.process(&p, if i < 480 { 0.5 } else { 0.0 }, 0.0, 0.0).0)
            .collect();
        let tail = |a: usize, b: usize| out[a..b].iter().map(|x| x * x).sum::<f32>();
        let open = bake(&effect("reverb", &[("size", 0.9), ("mix", 1.0)])).unwrap();
        let mut st2 = EffectState::default();
        let free: Vec<f32> = (0..48_000)
            .map(|i| {
                st2.process(&open, if i < 480 { 0.5 } else { 0.0 }, 0.0, 0.0)
                    .0
            })
            .collect();
        let free_tail: f32 = free[12_000..24_000].iter().map(|x| x * x).sum();
        assert!(
            tail(12_000, 24_000) < free_tail * 0.01,
            "ゲートの後は残響が消える"
        );
        assert!(tail(1_000, 4_000) > 0.0, "ゲートの間は鳴る");
        // ダッキング: 原音が鳴っている間はやまびこが小さい
        let run = |duck: f64| {
            let p = bake(&effect(
                "delay",
                &[
                    ("time_ms", 100.0),
                    ("mix", 1.0),
                    ("feedback", 0.0),
                    ("duck_db", duck),
                ],
            ))
            .unwrap();
            let mut st = EffectState::default();
            // 原音は鳴り続け、やまびこだけを見る(100ms 後から)
            (0..24_000)
                .map(|_| st.process(&p, 0.5, 0.5, 0.0).0)
                .skip(10_000)
                .map(|x| x.abs())
                .sum::<f32>()
        };
        assert!(run(12.0) < run(0.0) * 0.5);
    }

    #[test]
    fn mod_effects_are_listed_and_baked() {
        let names: Vec<&str> = effect_catalog().iter().map(|i| i.name).collect();
        for n in [
            "clipper",
            "bitcrush",
            "tremolo",
            "phaser",
            "flanger",
            "trance_gate",
            "auto_filter",
            "volume_shaper",
        ] {
            assert!(names.contains(&n), "{n}");
            assert!(effect_params_spec(n).is_some());
            let p = bake(&effect(n, &[])).unwrap();
            assert!(matches!(p, EffectParams::Mod(_)));
        }
        // オートメーション
        let mut p = bake(&effect("auto_filter", &[])).unwrap();
        assert!(p.set_continuous("cutoff", 400.0, 48_000.0));
        assert!(!p.set_continuous("nope", 1.0, 48_000.0));
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

    #[test]
    fn light_state_passes_delay_through_and_still_reverbs() {
        let mut st = EffectState::without_delay_buffers();
        let d = bake(&effect("delay", &[("mix", 1.0)])).unwrap();
        st.ensure_kind(&d);
        assert_eq!(st.process(&d, 0.3, -0.2, 0.0), (0.3, -0.2));
        let rv = bake(&effect("reverb", &[("mix", 0.5)])).unwrap();
        st.ensure_kind(&rv);
        st.process(&rv, 1.0, 1.0, 0.0);
        let tail = (0..48_000)
            .map(|_| st.process(&rv, 0.0, 0.0, 0.0).0.abs())
            .fold(0.0f32, f32::max);
        assert!(tail > 0.01);
    }

    #[test]
    fn tape_crackle_adds_sparse_clicks() {
        let p = bake(&effect("tape", &[("hiss", 0.0), ("crackle", 0.5)])).unwrap();
        let out = run(&p, |_| (0.0, 0.0), 48_000 * 4);
        // クリックの立ち上がり(0 から大きく跳ねた点)を数える
        let clicks = |ch: usize| {
            out.windows(2)
                .filter(|w| {
                    let (a, b) = if ch == 0 {
                        (w[0].0, w[1].0)
                    } else {
                        (w[0].1, w[1].1)
                    };
                    a.abs() < 1e-4 && b.abs() > 0.01
                })
                .count()
        };
        let (l, r) = (clicks(0), clicks(1));
        // 0.5 で毎秒約 8.5 回 → 4 秒で 20〜60 回程度(片側だけのものもある)
        assert!((15..80).contains(&l) && (15..80).contains(&r), "{l} {r}");
        let peak = out.iter().map(|o| o.0.abs()).fold(0.0f32, f32::max);
        assert!(peak > 0.03 && peak < 0.3, "{peak}");
        // 0 なら無音のまま
        let off = bake(&effect("tape", &[("hiss", 0.0)])).unwrap();
        assert!(run(&off, |_| (0.0, 0.0), 48_000).iter().all(|o| o.0 == 0.0));
    }

    #[test]
    fn studio_and_pitch_effects_are_listed_and_baked() {
        let names: Vec<&str> = effect_catalog().iter().map(|i| i.name).collect();
        for n in [
            "eq8",
            "saturation",
            "deesser",
            "gate",
            "pitch_shift",
            "harmonizer",
            "pitch_correct",
        ] {
            assert!(names.contains(&n), "{n}");
            assert!(effect_params_spec(n).is_some());
            let p = bake(&effect(n, &[])).unwrap();
            assert!(matches!(
                p,
                EffectParams::Studio(_) | EffectParams::Pitch(_)
            ));
            // 既定のつまみで通しても壊れない
            let out = run(&p, |i| ((i as f32 * 0.05).sin() * 0.5, 0.0), 9600);
            assert!(
                out.iter().all(|s| s.0.is_finite() && s.1.is_finite()),
                "{n}"
            );
        }
        // 音程を動かすものは遅れを申告する(遅延補正のため)
        assert!(bake(&effect("pitch_shift", &[])).unwrap().latency() > 0);
        // ゲートは source のトラックを検出に使う
        let mut e = effect("gate", &[]);
        e.params
            .insert("source".into(), ParamValue::Enum("trk_kick".into()));
        let g = bake_effect(&e, 48_000.0, &|_| Some(2)).unwrap();
        assert_eq!(g.key_source(), Some(2));
    }

    #[test]
    fn pitch_effects_without_delay_buffers_pass_through() {
        let p = bake(&effect("pitch_shift", &[("semitones", 7.0)])).unwrap();
        let mut st = EffectState::without_delay_buffers();
        st.ensure_kind(&p);
        assert_eq!(st.process(&p, 0.3, -0.2, 0.0), (0.3, -0.2));
    }

    /// 大きな音が急に来たときの、頭の数 ms の出力の山
    fn onset_peak(p: &EffectParams, lat: usize) -> f32 {
        let out = run(
            p,
            |i| {
                let x = if i >= 4800 { 0.9 } else { 0.01 };
                (x, x)
            },
            9600,
        );
        out[4800 + lat..4800 + lat + 96]
            .iter()
            .map(|s| s.0.abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn compressor_lookahead_catches_the_onset_and_reports_latency() {
        let plain = bake(&effect(
            "compressor",
            &[("threshold_db", -20.0), ("ratio", 20.0), ("attack_ms", 5.0)],
        ))
        .unwrap();
        let la = bake(&effect(
            "compressor",
            &[
                ("threshold_db", -20.0),
                ("ratio", 20.0),
                ("attack_ms", 5.0),
                ("lookahead_ms", 5.0),
            ],
        ))
        .unwrap();
        assert_eq!(plain.latency(), 0);
        assert_eq!(la.latency(), 240);
        let (a, b) = (onset_peak(&plain, 0), onset_peak(&la, 240));
        assert!(b < a * 0.7, "先読みなし {a} / あり {b}");
    }

    #[test]
    fn compressor_characters_differ_as_described() {
        let comp = |c: &str| {
            let mut e = effect(
                "compressor",
                &[
                    ("threshold_db", -30.0),
                    ("ratio", 8.0),
                    ("release_ms", 100.0),
                ],
            );
            e.params
                .insert("character".into(), ParamValue::Enum(c.into()));
            bake(&e).unwrap()
        };
        // 1 秒の大きな音のあと静かにして、`ms` 後にどれだけ戻ったか(出力 ÷ 入力)
        let recovered = |p: &EffectParams, ms: usize| {
            let n = 48_000 + ms * 48;
            let out = run(
                p,
                |i| {
                    let x = if i < 48_000 { 0.8 } else { 0.01 };
                    (x, x)
                },
                n,
            );
            out[n - 1].0 / 0.01
        };
        let clean = recovered(&comp("clean"), 100);
        let vca = recovered(&comp("vca"), 100);
        assert!(
            vca < clean * 0.8,
            "vca は長く潰した後ゆっくり戻る {vca} {clean}"
        );
        // opto は 100ms で vca より戻るが、1 秒たっても戻りきらない(clean は戻りきる)
        let opto = recovered(&comp("opto"), 100);
        assert!(opto > vca, "{opto} {vca}");
        let (opto1, clean1) = (
            recovered(&comp("opto"), 1000),
            recovered(&comp("clean"), 1000),
        );
        assert!(clean1 > 0.98 && opto1 < 0.9, "{opto1} {clean1}");
        // fet は潰した音に歪みが乗る(3 倍音)
        let h3 = |p: &EffectParams| {
            let out = run(
                p,
                |i| {
                    let x = (i as f32 * 200.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.8;
                    (x, x)
                },
                24_000,
            );
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, s) in out.iter().enumerate().skip(12_000) {
                let w = i as f32 * 600.0 * std::f32::consts::TAU / 48_000.0;
                re += s.0 * w.cos();
                im += s.0 * w.sin();
            }
            (re * re + im * im).sqrt()
        };
        assert!(h3(&comp("fet")) > h3(&comp("clean")) * 3.0);
    }

    #[test]
    fn delay_follows_the_tempo_and_each_type_echoes() {
        // 120BPM で 1/4 = 0.5 秒 = 24000 サンプル
        let mut e = effect("delay", &[("mix", 1.0), ("feedback", 0.0)]);
        e.params
            .insert("sync".into(), ParamValue::Enum("1/4".into()));
        let p = bake(&e).unwrap();
        let mut st = EffectState::default();
        st.ensure_kind(&p);
        st.set_clock(0.0, glaux_core::PPQ as f64 * 2.0 / 48_000.0);
        let out: Vec<f32> = (0..30_000)
            .map(|i| st.process(&p, impulse(i).0, impulse(i).1, 0.0).0)
            .collect();
        let at = out
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .unwrap()
            .0;
        assert!((at as i64 - 24_000).abs() <= 3, "{at}");
        for t in ["tape", "bbd", "multitap"] {
            let mut e = effect(
                "delay",
                &[
                    ("mix", 1.0),
                    ("feedback", 0.5),
                    ("time_ms", 100.0),
                    ("taps", 4.0),
                ],
            );
            e.params.insert("type".into(), ParamValue::Enum(t.into()));
            let p = bake(&e).unwrap();
            let out = run(&p, impulse, 48_000);
            let energy: f32 = out.iter().map(|s| s.0 * s.0).sum();
            assert!(energy > 1e-4 && energy.is_finite(), "{t}: {energy}");
            // 最初のやまびこは 100ms 付近(multitap は 25ms ごと)
            let first = out.iter().position(|s| s.0.abs() > 0.01).unwrap();
            let want = if t == "multitap" { 1200 } else { 4800 };
            assert!((first as i64 - want).abs() < 60, "{t}: {first}");
        }
        // bbd は長くするほど暗い
        let bright = |ms: f64| {
            let mut e = effect("delay", &[("mix", 1.0), ("feedback", 0.0), ("time_ms", ms)]);
            e.params
                .insert("type".into(), ParamValue::Enum("bbd".into()));
            let p = bake(&e).unwrap();
            let out = run(
                &p,
                |i| {
                    let x = (i as f32 * 6000.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.3;
                    (x, x)
                },
                48_000,
            );
            out[40_000..].iter().map(|s| s.0 * s.0).sum::<f32>()
        };
        assert!(bright(800.0) < bright(100.0) * 0.5);
    }

    #[test]
    fn automation_steps_are_smoothed() {
        // 出力レベルを 1 ブロックで 0 → -24dB にしても、次のサンプルで段差にならない
        let mut p = bake(&effect("distortion", &[("mix", 0.0), ("level_db", 0.0)])).unwrap();
        let mut st = EffectState::default();
        st.ensure_kind(&p);
        for _ in 0..1000 {
            st.process(&p, 0.5, 0.5, 0.0);
        }
        assert!(p.set_continuous("level_db", -24.0, 48_000.0));
        let first = st.process(&p, 0.5, 0.5, 0.0).0;
        assert!(first > 0.45, "{first}");
        let mut last = first;
        for _ in 0..4800 {
            last = st.process(&p, 0.5, 0.5, 0.0).0;
        }
        assert!(
            (last - 0.5 * 10f32.powf(-24.0 / 20.0)).abs() < 1e-3,
            "{last}"
        );
        // ミックスも同じ
        let mut p = bake(&effect("reverb", &[("mix", 0.0)])).unwrap();
        let mut st = EffectState::default();
        st.ensure_kind(&p);
        let sine = |i: usize| (i as f32 * 0.03).sin() * 0.5;
        let mut prev = 0.0;
        for i in 0..2000 {
            prev = st.process(&p, sine(i), sine(i), 0.0).0;
        }
        assert!(p.set_continuous("mix", 1.0, 48_000.0));
        let next = st.process(&p, sine(2000), sine(2000), 0.0).0;
        assert!((next - prev).abs() < 0.05, "{prev} → {next}");
    }
}
