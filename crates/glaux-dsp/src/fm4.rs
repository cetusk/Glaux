//! 4 オペレーターの FM シンセ `fm4`(DX21 / TX81Z 系の 8 アルゴリズム)。
//!
//! 4 つの正弦波(オペレーター)を、アルゴリズム(つなぎ方)に従って互いの位相を揺らし合う。
//! 2 オペレーターの `fm` より倍音の作り込みが細かく、DX7 らしいエレピ・ベル・ブラス・
//! 木琴・オルガン・FM ベースが作れる。
//!
//! - オペレーターごとに周波数比(ratio)・出力(level)・簡単なエンベロープ(attack / decay / sustain)。
//!   リリースは全体で 1 つ
//! - キャリア(音として出るオペレーター)の level は音量、モジュレーター(ほかを揺らす)の level は
//!   変調の深さ(1 で約 6 ラジアン)
//! - オペレーター 4 は自分自身を揺らせる(feedback。ノコギリ波寄りのざらつき・ノイズ)
//! - 深い変調の側帯波の折り返しを抑えるため、発振は 2 倍のレートで回してハーフバンドで戻す
//!
//! アルゴリズム(→ は「揺らす」、[ ] がキャリア):
//!
//! | 番号 | つなぎ方 | 向く音 |
//! |---|---|---|
//! | 1 | 4→3→2→[1] | 直列。ブラス・リード・歪んだベース(倍音がいちばん複雑) |
//! | 2 | (4+3)→2→[1] | 2 つの変調を混ぜて直列。弦・ブラスの厚み |
//! | 3 | (3→2 + 4)→[1] | 直列と単独の変調。エレピ・クラビ |
//! | 4 | (4→3 + 2)→[1] | 3 と同じ形の入れ替え。ベース・木管 |
//! | 5 | 2→[1] と 4→[3] | 2 組の 2 段。DX のエレピ(胴 + 金属的なアタック)・ベル |
//! | 6 | 4→[1]・[2]・[3] | 1 つの変調で 3 つのキャリア。オルガン・ブラス合奏 |
//! | 7 | 4→[3] と [1]・[2] | 1 組 + 正弦波 2 つ。オルガン・マリンバ |
//! | 8 | [1]・[2]・[3]・[4] | 4 つの正弦波を足す(加算)。オルガン・笛 |
//!
//! RT セーフ: 値型のみでアロケーションなし。

use glaux_core::Articulation;

/// アルゴリズムの数
pub const ALGORITHMS: usize = 8;
/// level 1 のモジュレーターの変調の深さ(ラジアン)
const MAX_INDEX: f32 = 6.0;

/// オペレーター 1 つの焼き込み済みパラメータ
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fm4Op {
    /// 周波数比(鳴らす音の高さに対する)
    pub ratio: f32,
    /// 出力 0..=1(キャリアは音量、モジュレーターは変調の深さ)
    pub level: f32,
    /// 立ち上がり(秒)
    pub attack: f32,
    /// sustain まで下がる時間(秒。この時間でほぼ落ち着く)
    pub decay: f32,
    /// 減衰後の高さ 0..=1
    pub sustain: f32,
}

/// 焼き込み済みパラメータ(1 トラック分)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fm4Params {
    /// アルゴリズム(0..8。表示は 1..=8)
    pub algorithm: u8,
    pub ops: [Fm4Op; 4],
    /// オペレーター 4 の自己フィードバック 0..=1
    pub feedback: f32,
    /// ノートオフ後に消える時間(秒。全オペレーター共通)
    pub release: f32,
    /// ベロシティで変調の深さ(明るさ)が変わる量 0..=1
    pub vel_bright: f32,
    /// リニアゲイン(dB から変換済み)
    pub gain: f32,
}

/// アルゴリズムごとのキャリアの印(ビット。1 << 添字、添字 0 = オペレーター 1)。
/// つなぎ方そのものは [`Fm4Voice::osc`] にアルゴリズムごとに直に書いてある(モジュールの説明の表と同じ)
const CARRIERS: [u8; ALGORITHMS] = [
    0b0001, 0b0001, 0b0001, 0b0001, 0b0101, 0b0111, 0b0111, 0b1111,
];

/// アルゴリズム `a` でオペレーター `i`(0..4)がキャリアか
pub fn is_carrier(a: u8, i: usize) -> bool {
    CARRIERS[(a as usize).min(ALGORITHMS - 1)] & (1 << i) != 0
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    Attack,
    Decay,
}

#[derive(Clone, Copy, Debug)]
pub struct Fm4Voice {
    freq: f32,
    /// 位相(回転数。0..1)
    phase: [f32; 4],
    env: [f32; 4],
    stage: [Stage; 4],
    released: bool,
    /// ノートオフ後の共通の減衰(1 → 0)
    rel_env: f32,
    /// オペレーター 4 の直前 2 サンプルの出力(フィードバック。平均して発振を抑える)
    fb_hist: [f32; 2],
    vel: f32,
    /// ベロシティで変わる変調の深さの倍率
    mod_scale: f32,
    decay_mul: f32,
    pub(crate) expr: crate::expr::PitchExpr,
    hb: crate::oversample::Halfband,
    sample_rate: f32,
    /// 係数(CTRL サンプルごとに求め直す)
    att_inc: [f32; 4],
    dec_coef: [f32; 4],
    rel_coef: f32,
    out_scale: f32,
    ctrl: u32,
}

/// 係数を求め直す間隔(サンプル)
const CTRL: u32 = 32;

/// 正弦の表の大きさ(1 周期)
const SIN_N: usize = 4096;
/// 正弦の表(1 周期 + 補間用に頭を 1 つ複製)。焼き込み時(UI スレッド)に 1 度だけ作る
static SIN_TABLE: std::sync::OnceLock<Box<[f32; SIN_N + 1]>> = std::sync::OnceLock::new();

/// 正弦の表を用意する(焼き込み時に呼ぶ。2 回目以降は何もしない)
pub fn ensure_sine_table() {
    SIN_TABLE.get_or_init(|| {
        let mut t = Box::new([0.0f32; SIN_N + 1]);
        for (i, v) in t.iter_mut().enumerate() {
            *v = (i as f64 / SIN_N as f64 * std::f64::consts::TAU).sin() as f32;
        }
        t
    });
}

/// 表を引いて sin(2π x)(x は回転数、−8..8 の範囲)。線形補間(誤差 1e-5 未満)。
/// FM のように前の結果を次の入力にする直列の計算では、多項式より待ちが短い
#[inline(always)]
fn sin_lut(t: &[f32; SIN_N + 1], x: f32) -> f32 {
    // 負の値も切り捨て = 床になるように 8 周ぶん足してから整数にする
    let xi = x * SIN_N as f32 + (SIN_N * 8) as f32;
    let i = xi as i32;
    let f = xi - i as f32;
    let k = (i as usize) & (SIN_N - 1);
    let a = t[k];
    a + (t[k + 1] - a) * f
}

/// sin(2π x)(x は回転数。±2^31 の範囲ならどんな値でもよい)。9 次の多項式(誤差 4e-6 未満)。
/// 分岐なし・ライブラリ呼び出しなし(floor の代わりに整数への変換)なので、加算合成の部分音の
/// ループではまとめて計算(ベクトル化)される
#[inline(always)]
pub(crate) fn sin_turns(x: f32) -> f32 {
    let y = x + 0.5;
    let i = y as i32 as f32;
    let fl = if i > y { i - 1.0 } else { i };
    let t = x - fl; // −0.5..0.5
                    // sin(π − θ) = sin θ で −0.25..0.25 に畳む
    let folded = 0.5f32.copysign(t) - t;
    let t = if t.abs() > 0.25 { folded } else { t };
    let th = t * std::f32::consts::TAU;
    let t2 = th * th;
    th * (1.0
        + t2 * (-1.0 / 6.0 + t2 * (1.0 / 120.0 + t2 * (-1.0 / 5040.0 + t2 * (1.0 / 362_880.0)))))
}

impl Fm4Voice {
    pub fn start(
        _p: &Fm4Params,
        freq: f32,
        vel: f32,
        articulation: Articulation,
        sample_rate: f32,
    ) -> Self {
        let (vel, decay_mul) = match articulation {
            Articulation::Accent => ((vel * 1.3).min(1.0), 1.0),
            // ミュート: 減衰を速く
            Articulation::PalmMute => (vel, 0.25),
            _ => (vel, 1.0),
        };
        Fm4Voice {
            freq,
            phase: [0.0; 4],
            env: [0.0; 4],
            stage: [Stage::Attack; 4],
            released: false,
            rel_env: 1.0,
            fb_hist: [0.0; 2],
            vel,
            mod_scale: 1.0,
            decay_mul,
            expr: crate::expr::PitchExpr::new(articulation, sample_rate),
            hb: Default::default(),
            sample_rate,
            att_inc: [0.0; 4],
            dec_coef: [0.0; 4],
            rel_coef: 0.0,
            out_scale: 1.0,
            ctrl: 0,
        }
    }

    /// レガートで次の音へ移る: 発音し直さず(エンベロープ・波の位相・フィルタはそのまま)高さだけを変える。
    /// 音程の表現(奏法のビブラートなど)は次の音のものにする
    pub fn glide_to(
        &mut self,
        freq: f32,
        articulation: glaux_core::Articulation,
        sample_rate: f32,
    ) {
        self.freq = freq;
        self.expr = crate::expr::PitchExpr::new(articulation, sample_rate);
    }

    pub fn note_off(&mut self) {
        self.released = true;
    }

    /// レガート: 立ち上がりを飛ばして、鳴り続けている状態(各オペレーターの sustain。
    /// 減衰しきるキャリアは 0.35)から始める
    pub fn skip_attack(&mut self, p: &Fm4Params) {
        for i in 0..4 {
            let s = p.ops[i].sustain;
            self.env[i] = if is_carrier(p.algorithm, i) {
                s.clamp(0.35, 1.0)
            } else {
                s
            };
            self.stage[i] = Stage::Decay;
        }
    }

    pub fn finished(&self) -> bool {
        self.released && self.rel_env < 1e-4
    }

    /// 係数を求め直す(CTRL サンプルごと。割り算を 1 サンプルごとにしない)
    fn control(&mut self, p: &Fm4Params) {
        let sr = self.sample_rate;
        for i in 0..4 {
            let op = &p.ops[i];
            self.att_inc[i] = 1.0 / (op.attack.max(0.0005) * sr);
            self.dec_coef[i] = (6.9 / ((op.decay * self.decay_mul).max(0.005) * sr)).min(1.0);
        }
        self.rel_coef = (6.9 / ((p.release * self.decay_mul).max(0.005) * sr)).min(1.0);
        self.mod_scale = 1.0 - p.vel_bright * (1.0 - self.vel);
        let carriers = CARRIERS[(p.algorithm as usize).min(ALGORITHMS - 1)];
        // キャリアの数で割って、アルゴリズムを変えても音量がそろうように
        self.out_scale = 1.0 / (carriers.count_ones().max(1) as f32).sqrt();
        // キャリアがすべて減衰しきったら止める(ノートオフを待たない)
        let decayed = |i: usize| {
            carriers & (1 << i) == 0
                || (self.stage[i] == Stage::Decay && p.ops[i].sustain <= 0.0 && self.env[i] < 1e-4)
        };
        if !self.released && (0..4).all(decayed) {
            self.released = true;
            self.rel_env = 0.0;
        }
    }

    pub fn next(&mut self, p: &Fm4Params) -> f32 {
        let sr = self.sample_rate;
        if self.ctrl == 0 {
            self.control(p);
            self.ctrl = CTRL;
        }
        self.ctrl -= 1;
        // ---- エンベロープ(オペレーターごと。attack は線形、decay は指数) ----
        for i in 0..4 {
            match self.stage[i] {
                Stage::Attack => {
                    self.env[i] += self.att_inc[i];
                    if self.env[i] >= 1.0 {
                        self.env[i] = 1.0;
                        self.stage[i] = Stage::Decay;
                    }
                }
                Stage::Decay => {
                    self.env[i] += (p.ops[i].sustain - self.env[i]) * self.dec_coef[i];
                }
            }
        }
        if self.released {
            self.rel_env -= self.rel_env * self.rel_coef;
        }
        let ratio_expr = if self.expr.is_active() {
            self.expr.next_ratio(sr)
        } else {
            1.0
        };
        let f = self.freq * ratio_expr;
        // 発振は 2 倍のレートで 2 回回し、ハーフバンドで戻す(深い変調の折り返しを抑える)
        let inc = f / (sr * 2.0);
        let Some(t) = SIN_TABLE.get() else {
            return 0.0;
        };
        let x0 = self.osc(p, t, inc);
        let x1 = self.osc(p, t, inc);
        let out = self.hb.down(x0, x1);
        out * self.out_scale * self.rel_env * self.vel * p.gain
    }

    /// 発振を 1 歩進める(`inc` は基準の 1 歩の位相の進み)。アルゴリズムごとに直に書く
    /// (どのアルゴリズムも番号の大きいオペレーターが揺らす側なので、4 → 1 の順に求める)
    #[inline]
    fn osc(&mut self, p: &Fm4Params, t: &[f32; SIN_N + 1], inc: f32) -> f32 {
        let depth = MAX_INDEX / std::f32::consts::TAU * self.mod_scale;
        let ph = self.phase;
        let lv = |i: usize| p.ops[i].level * self.env[i];
        let op = |i: usize, m: f32| sin_lut(t, ph[i] + m * depth) * lv(i);
        // オペレーター 4(フィードバック付き。直前 2 サンプルの平均で発振を抑える)
        let fb = p.feedback * 0.25 * (self.fb_hist[0] + self.fb_hist[1]);
        let s4 = sin_lut(t, ph[3] + fb) * lv(3);
        self.fb_hist = [self.fb_hist[1], s4];
        let out = match p.algorithm {
            0 => {
                let s3 = op(2, s4);
                let s2 = op(1, s3);
                op(0, s2)
            }
            1 => {
                let s3 = op(2, 0.0);
                let s2 = op(1, s3 + s4);
                op(0, s2)
            }
            2 => {
                let s3 = op(2, 0.0);
                let s2 = op(1, s3);
                op(0, s2 + s4)
            }
            3 => {
                let s3 = op(2, s4);
                let s2 = op(1, 0.0);
                op(0, s2 + s3)
            }
            4 => {
                let s2 = op(1, 0.0);
                let s3 = op(2, s4);
                op(0, s2) + s3
            }
            5 => op(0, s4) + op(1, s4) + op(2, s4),
            6 => op(0, 0.0) + op(1, 0.0) + op(2, s4),
            _ => op(0, 0.0) + op(1, 0.0) + op(2, 0.0) + s4,
        };
        // 位相は正なので、整数への変換(切り捨て)で整数部を落とす(floor のライブラリ呼び出しを避ける)
        for i in 0..4 {
            let next = self.phase[i] + inc * p.ops[i].ratio;
            self.phase[i] = next - (next as i32) as f32;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(ratio: f32, level: f32) -> Fm4Op {
        Fm4Op {
            ratio,
            level,
            attack: 0.002,
            decay: 1.0,
            sustain: 1.0,
        }
    }

    fn params(algorithm: u8, levels: [f32; 4]) -> Fm4Params {
        Fm4Params {
            algorithm,
            ops: [
                op(1.0, levels[0]),
                op(1.0, levels[1]),
                op(1.0, levels[2]),
                op(1.0, levels[3]),
            ],
            feedback: 0.0,
            release: 0.2,
            vel_bright: 0.0,
            gain: 1.0,
        }
    }

    fn render(p: &Fm4Params, freq: f32, n: usize) -> Vec<f32> {
        ensure_sine_table();
        let mut v = Fm4Voice::start(p, freq, 1.0, Articulation::Normal, 48_000.0);
        (0..n).map(|_| v.next(p)).collect()
    }

    /// 周波数 `f` の成分の振幅
    fn bin(x: &[f32], f: f32) -> f32 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, v) in x.iter().enumerate() {
            let w = std::f64::consts::TAU * f as f64 * i as f64 / 48_000.0;
            re += *v as f64 * w.cos();
            im += *v as f64 * w.sin();
        }
        ((re * re + im * im).sqrt() * 2.0 / x.len() as f64) as f32
    }

    #[test]
    fn fast_sines_are_accurate() {
        ensure_sine_table();
        let t = SIN_TABLE.get().unwrap();
        for k in -2000..2000 {
            let x = k as f32 * 0.00377;
            let want = (x * std::f32::consts::TAU).sin();
            let e = (sin_turns(x) - want).abs();
            assert!(e < 1e-5, "{x}: {e}");
            let e = (sin_lut(t, x) - want).abs();
            assert!(e < 2e-5, "表 {x}: {e}");
        }
    }

    #[test]
    fn carriers_without_modulation_are_sines() {
        // アルゴリズム 8(全部キャリア)で 1 つだけ鳴らすと正弦波
        let p = params(7, [1.0, 0.0, 0.0, 0.0]);
        let x = render(&p, 220.0, 24_000);
        let x = &x[2400..];
        // キャリア 4 つの足し算なので 1/√4 にそろえてある
        assert!((bin(x, 220.0) - 0.5).abs() < 0.02, "{}", bin(x, 220.0));
        assert!(bin(x, 440.0) < 0.01);
        assert!(bin(x, 660.0) < 0.01);
    }

    #[test]
    fn modulators_add_harmonics_and_algorithms_differ() {
        // 直列(1)は変調が深く、足し合わせ(8)は正弦波の和。同じ level でも倍音の量が違う
        let lv = [1.0, 0.5, 0.5, 0.5];
        let serial = render(&params(0, lv), 220.0, 24_000);
        let additive = render(&params(7, lv), 220.0, 24_000);
        let h = |x: &[f32]| {
            (2..10)
                .map(|k| bin(&x[2400..], 220.0 * k as f32))
                .sum::<f32>()
        };
        assert!(h(&serial) > 0.1, "直列は倍音が多い: {}", h(&serial));
        assert!(h(&additive) < 0.01, "加算は基音だけ: {}", h(&additive));
        // 8 つのアルゴリズムはどれも鳴り、互いに違う音(オペレーターごとに比と出力を変えて比べる)
        let all: Vec<Vec<f32>> = (0..ALGORITHMS as u8)
            .map(|a| {
                let mut p = params(a, [1.0, 0.7, 0.5, 0.3]);
                p.ops[2].ratio = 2.0;
                p.ops[3].ratio = 3.0;
                render(&p, 220.0, 9600)
            })
            .collect();
        let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
        for (a, x) in all.iter().enumerate() {
            assert!(rms(&x[2400..]) > 0.1, "アルゴリズム {}", a + 1);
            for (b, y) in all.iter().enumerate().skip(a + 1) {
                let d: Vec<f32> = x.iter().zip(y).map(|(p, q)| p - q).collect();
                assert!(rms(&d[2400..]) > 0.01, "{} と {}", a + 1, b + 1);
            }
        }
    }

    #[test]
    fn deep_modulation_on_high_notes_does_not_alias_much() {
        // 2kHz を直列で深く: 2kHz の整数倍以外(折り返し)は小さい
        let mut p = params(0, [1.0, 1.0, 1.0, 1.0]);
        p.feedback = 0.5;
        let x = render(&p, 2000.0, 24_000);
        let x = &x[2400..];
        let harm: f32 = (1..12).map(|k| bin(x, 2000.0 * k as f32)).sum();
        let alias: f32 = [700.0, 1300.0, 3100.0, 4900.0, 7300.0]
            .iter()
            .map(|f| bin(x, *f))
            .sum();
        assert!(alias < harm * 0.15, "折り返し {alias} / 倍音 {harm}");
    }

    #[test]
    fn modulator_envelope_makes_the_attack_bright() {
        // エレピ: モジュレーターだけ速く減衰 → 頭は明るく、後は丸い
        let mut p = params(4, [1.0, 0.8, 0.0, 0.0]);
        p.ops[1].decay = 0.2;
        p.ops[1].sustain = 0.0;
        let x = render(&p, 220.0, 48_000);
        let h = |x: &[f32]| (2..8).map(|k| bin(x, 220.0 * k as f32)).sum::<f32>();
        let head = h(&x[480..5280]);
        let tail = h(&x[38_400..43_200]);
        assert!(head > tail * 5.0, "{head} → {tail}");
    }

    #[test]
    fn releases_and_finishes_and_decayed_carriers_stop() {
        let p = params(4, [1.0, 0.5, 1.0, 0.5]);
        let mut v = Fm4Voice::start(&p, 440.0, 1.0, Articulation::Normal, 48_000.0);
        for _ in 0..4800 {
            v.next(&p);
        }
        v.note_off();
        for _ in 0..48_000 {
            v.next(&p);
        }
        assert!(v.finished());
        // キャリアの sustain が 0 なら、押さえたままでも減衰しきったら止まる
        let mut p = params(7, [1.0, 1.0, 1.0, 1.0]);
        for o in &mut p.ops {
            o.decay = 0.1;
            o.sustain = 0.0;
        }
        let mut v = Fm4Voice::start(&p, 440.0, 1.0, Articulation::Normal, 48_000.0);
        for _ in 0..48_000 {
            v.next(&p);
        }
        assert!(v.finished());
    }
}
