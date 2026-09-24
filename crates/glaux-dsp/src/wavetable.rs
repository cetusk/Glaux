//! ウェーブテーブルシンセ `wavetable`。
//!
//! 1 周期ぶんの波形(フレーム)を 16 枚並べたテーブルを `position` で滑らかに行き来して、
//! 音色そのものを動かす(減算式のフィルタ開閉とは違う「形が変わる」変化。現代の EDM・
//! ベースミュージックのうねるベース、変化するパッドの定番)。
//!
//! - テーブルは 5 種(analog / pulse / vocal / sync / organ)。倍音の設計図から逆 FFT で作る
//! - 折り返し雑音を避けるため、1 オクターブごとに倍音を間引いた版(ミップマップ)を持ち、
//!   鳴らす高さで選ぶ
//! - テーブルは初回の焼き込み時(UI スレッド)に 1 度だけ作る。オーディオスレッドは読むだけ
//! - position は専用の減衰エンベロープ(pos_env)と LFO でも動かせる
//! - その後は subtractive と同じ SVF ローパス + ADSR
//!
//! RT セーフ: ボイスは値型のみ。テーブル参照は `OnceLock::get`(ロックなし)。

use std::sync::OnceLock;

use rustfft::{num_complex::Complex, FftPlanner};

/// 1 フレームのサンプル数
const N: usize = 2048;
/// 補間用に末尾へ先頭を 1 つ複製した長さ
const ROW: usize = N + 1;
/// 1 テーブルのフレーム数
pub const FRAMES: usize = 16;
/// ミップマップの段数(段 ℓ の最大倍音 = 1023 >> ℓ)
const LEVELS: usize = 11;
const MAX_HARMONIC: usize = N / 2 - 1;

/// テーブル名(ParamSpec の choices と同じ並び)。
pub const TABLE_NAMES: &[&str] = &["analog", "pulse", "vocal", "sync", "organ"];

/// 全テーブル([テーブル][フレーム][段][ROW])。
struct Bank {
    data: Vec<f32>,
}

impl Bank {
    fn row(&self, table: usize, frame: usize, level: usize) -> &[f32] {
        let off = ((table * FRAMES + frame) * LEVELS + level) * ROW;
        &self.data[off..off + ROW]
    }
}

static BANK: OnceLock<Bank> = OnceLock::new();

/// テーブルを用意する(焼き込み時に呼ぶ。2 回目以降は何もしない)。
pub fn ensure_tables() {
    BANK.get_or_init(build_bank);
}

/// フレーム `t`(0..=1)の複素スペクトル(添字 = 倍音番号、0 は直流)。
fn spectrum(table: usize, t: f32, planner: &mut FftPlanner<f32>) -> Vec<Complex<f32>> {
    let mut s = vec![Complex::new(0.0f32, 0.0); MAX_HARMONIC + 1];
    let pi = std::f32::consts::PI;
    // 正弦成分(sin(2πhx))は複素係数 -i/2 に相当。ここでは実部・虚部をそのまま
    // 逆 FFT に入れて実数部を取るので、sin は (0, -a)、cos は (a, 0) で表す
    let sin = |a: f32| Complex::new(0.0, -a);
    match TABLE_NAMES[table] {
        // sine → triangle → saw → square を倍音の混ぜ合わせで連続につなぐ
        "analog" => {
            let shape = |k: usize, h: usize| -> f32 {
                let hf = h as f32;
                match k {
                    0 => (h == 1) as u8 as f32,
                    1 if h % 2 == 1 => {
                        let sign = if (h / 2) % 2 == 0 { 1.0 } else { -1.0 };
                        sign * 8.0 / (pi * pi * hf * hf)
                    }
                    2 => 2.0 / (pi * hf),
                    3 if h % 2 == 1 => 4.0 / (pi * hf),
                    _ => 0.0,
                }
            };
            let x = t.clamp(0.0, 1.0) * 3.0;
            let k = (x.floor() as usize).min(2);
            let f = x - k as f32;
            for (h, v) in s.iter_mut().enumerate().skip(1) {
                *v = sin(shape(k, h) * (1.0 - f) + shape(k + 1, h) * f);
            }
        }
        // パルス幅 50% → 5%(PWM。細くなるほど鼻にかかった細い音)
        "pulse" => {
            let d = 0.5 - 0.45 * t.clamp(0.0, 1.0);
            for (h, v) in s.iter_mut().enumerate().skip(1) {
                let hf = h as f32;
                let a = 2.0 / (pi * hf) * (pi * hf * d).sin();
                // 中心を揃える(幅が変わっても位相が跳ばない)
                let ph = -2.0 * pi * hf * d * 0.5;
                *v = Complex::new(a * ph.cos(), a * ph.sin());
            }
        }
        // 母音 あ → え → い → お → う(フォルマントを周波数で補間)。基音 110Hz 想定
        "vocal" => {
            const VOWELS: [[f32; 3]; 5] = [
                [800.0, 1200.0, 2800.0],
                [500.0, 1900.0, 2600.0],
                [300.0, 2300.0, 3000.0],
                [500.0, 850.0, 2700.0],
                [350.0, 1250.0, 2400.0],
            ];
            let x = t.clamp(0.0, 1.0) * 4.0;
            let k = (x.floor() as usize).min(3);
            let f = x - k as f32;
            let fm: Vec<f32> = (0..3)
                .map(|i| VOWELS[k][i] * (1.0 - f) + VOWELS[k + 1][i] * f)
                .collect();
            let gains = [1.0f32, 0.6, 0.3];
            let bw = [90.0f32, 110.0, 150.0];
            for (h, v) in s.iter_mut().enumerate().skip(1) {
                let hz = 110.0 * h as f32;
                let env: f32 = (0..3)
                    .map(|i| gains[i] / (1.0 + ((hz - fm[i]) / bw[i]).powi(2)))
                    .sum();
                // 声帯の音源(高域ほど弱い)× フォルマント
                *v = sin(env / (h as f32).sqrt() + 0.02 / h as f32);
            }
        }
        // ハードシンク: 従属側のノコギリ波の周期を 1 → 8 倍に(ギラついた変化)
        "sync" => {
            let ratio = 1.0 + 7.0 * t.clamp(0.0, 1.0);
            // 8 倍の解像度で作って FFT し、低い倍音だけ使う(素朴な波形の折り返しを避ける)
            let m = N * 8;
            let mut buf: Vec<Complex<f32>> = (0..m)
                .map(|i| {
                    let x = i as f32 / m as f32;
                    Complex::new(2.0 * (x * ratio).fract() - 1.0, 0.0)
                })
                .collect();
            planner.plan_fft_forward(m).process(&mut buf);
            for (h, v) in s.iter_mut().enumerate().skip(1) {
                // 実数信号: x = Σ (2/m) Re(X_h e^{i2πhx})
                *v = buf[h] * (2.0 / m as f32);
            }
        }
        // オルガン: ドローバーを 1 本ずつ引き出すように倍音を足していく
        _ => {
            const HARM: [usize; 9] = [1, 2, 3, 4, 5, 6, 8, 10, 12];
            let x = t.clamp(0.0, 1.0) * (HARM.len() - 1) as f32;
            for (k, &h) in HARM.iter().enumerate() {
                let w = (x - k as f32 + 1.0).clamp(0.0, 1.0);
                s[h] = sin(w * 0.8f32.powi(k as i32));
            }
        }
    }
    s
}

fn build_bank() -> Bank {
    let mut planner = FftPlanner::<f32>::new();
    let ifft = planner.plan_fft_inverse(N);
    let mut data = vec![0.0f32; TABLE_NAMES.len() * FRAMES * LEVELS * ROW];
    let mut buf = vec![Complex::new(0.0f32, 0.0); N];
    for table in 0..TABLE_NAMES.len() {
        for frame in 0..FRAMES {
            let spec = spectrum(table, frame as f32 / (FRAMES - 1) as f32, &mut planner);
            let mut scale = 1.0f32;
            for level in 0..LEVELS {
                let max_h = MAX_HARMONIC >> level;
                buf.fill(Complex::new(0.0, 0.0));
                for (h, c) in spec.iter().enumerate().take(max_h + 1).skip(1) {
                    buf[h] = *c;
                }
                ifft.process(&mut buf);
                // 段 0(全倍音)のピークで正規化し、他の段にも同じ倍率を使う
                if level == 0 {
                    let peak = buf.iter().map(|c| c.re.abs()).fold(0.0f32, f32::max);
                    scale = if peak > 1e-6 { 0.9 / peak } else { 0.0 };
                }
                let off = ((table * FRAMES + frame) * LEVELS + level) * ROW;
                let row = &mut data[off..off + ROW];
                for (o, c) in row.iter_mut().zip(&buf) {
                    *o = c.re * scale;
                }
                row[N] = row[0];
            }
        }
    }
    Bank { data }
}

/// 焼き込み済みパラメータ(1 トラック分)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WavetableParams {
    /// テーブル番号(TABLE_NAMES の添字)
    pub table: u8,
    /// 0..=1
    pub position: f32,
    /// 鳴り始めに position をずらす量 -1..=1(減衰して position に戻る)
    pub pos_env: f32,
    /// その戻る時間(秒)
    pub pos_decay: f32,
    /// position を揺らす LFO(Hz)
    pub lfo_rate: f32,
    /// LFO の深さ 0..=1(position の振れ幅)
    pub lfo_depth: f32,
    pub unison: u8,
    pub detune_cents: f32,
    pub cutoff: f32,
    pub resonance: f32,
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    pub gain: f32,
}

const MAX_UNISON: usize = 7;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    Attack,
    Decay,
    Release,
}

#[derive(Clone, Copy, Debug)]
pub struct WavetableVoice {
    freq: f32,
    amp: f32,
    phases: [f32; MAX_UNISON],
    lfo_phase: f32,
    /// position のエンベロープ(1 → 0)
    pos_env: f32,
    /// 奏法による position のずれ(アクセントで明るく)
    pos_bias: f32,
    decay_mul: f32,
    sustain_mul: f32,
    release_mul: f32,
    cutoff_mul: f32,
    stage: Stage,
    env: f32,
    ic1: f32,
    ic2: f32,
    pub(crate) expr: crate::expr::PitchExpr,
    sample_rate: f32,
}

/// 周波数 → ミップマップの段(倍音がナイキストの手前に収まる最も豊かな段)。
fn level_for(freq: f32, sample_rate: f32) -> usize {
    let allowed = (0.45 * sample_rate / freq.max(1.0)).max(1.0) as usize;
    let mut level = 0;
    while level + 1 < LEVELS && (MAX_HARMONIC >> level) > allowed {
        level += 1;
    }
    level
}

fn read(row: &[f32], phase: f32) -> f32 {
    let x = phase * N as f32;
    let i = (x as usize).min(N - 1);
    let f = x - i as f32;
    row[i] + (row[i + 1] - row[i]) * f
}

impl WavetableVoice {
    pub fn start(
        _p: &WavetableParams,
        freq: f32,
        vel: f32,
        articulation: glaux_core::Articulation,
        sample_rate: f32,
    ) -> Self {
        use glaux_core::Articulation as A;
        let (amp, pos_bias, decay_mul, sustain_mul, release_mul, cutoff_mul) = match articulation {
            A::Accent => ((vel * 1.3).min(1.0), 0.15, 1.0, 1.0, 1.0, 1.5),
            A::PalmMute => (vel, 0.0, 0.2, 0.0, 0.6, 0.3),
            A::Staccato => (vel, 0.0, 1.0, 1.0, 0.5, 1.0),
            _ => (vel, 0.0, 1.0, 1.0, 1.0, 1.0),
        };
        let mut phases = [0.0f32; MAX_UNISON];
        for (i, ph) in phases.iter_mut().enumerate() {
            *ph = (i as f32 * 0.371) % 1.0;
        }
        WavetableVoice {
            freq,
            amp,
            phases,
            lfo_phase: 0.0,
            pos_env: 1.0,
            pos_bias,
            decay_mul,
            sustain_mul,
            release_mul,
            cutoff_mul,
            stage: Stage::Attack,
            env: 0.0,
            ic1: 0.0,
            ic2: 0.0,
            expr: crate::expr::PitchExpr::new(articulation, sample_rate),
            sample_rate,
        }
    }

    pub fn note_off(&mut self) {
        self.stage = Stage::Release;
    }

    /// レガート: 立ち上がり(と position の掃引)を飛ばして、鳴り続けている状態から始める
    pub fn skip_attack(&mut self, p: &WavetableParams) {
        self.env = (p.sustain * self.sustain_mul).clamp(0.35, 1.0);
        self.stage = Stage::Decay;
        self.pos_env = 0.0;
    }

    pub fn finished(&self) -> bool {
        self.stage == Stage::Release && self.env < 1e-4
    }

    pub fn next(&mut self, p: &WavetableParams) -> f32 {
        let Some(bank) = BANK.get() else {
            return 0.0;
        };
        let sr = self.sample_rate;

        // ---- 振幅 ADSR(decay / release はその時間でほぼ消える -60dB) ----
        let sustain = p.sustain * self.sustain_mul;
        match self.stage {
            Stage::Attack => {
                self.env += 1.0 / (p.attack.max(0.0005) * sr);
                if self.env >= 1.0 {
                    self.env = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                let coef = (6.9 / ((p.decay * self.decay_mul).max(0.005) * sr)).min(1.0);
                self.env += (sustain - self.env) * coef;
                if self.env < 1e-4 {
                    self.stage = Stage::Release;
                }
            }
            Stage::Release => {
                let coef = (6.9 / ((p.release * self.release_mul).max(0.005) * sr)).min(1.0);
                self.env -= self.env * coef;
            }
        }

        // ---- position(基準 + エンベロープ + LFO + 奏法) ----
        self.pos_env -= self.pos_env * (4.6 / (p.pos_decay.max(0.005) * sr)).min(1.0);
        let lfo = (std::f32::consts::TAU * self.lfo_phase).sin();
        self.lfo_phase = (self.lfo_phase + p.lfo_rate / sr).fract();
        let pos = (p.position + p.pos_env * self.pos_env + p.lfo_depth * 0.5 * lfo + self.pos_bias)
            .clamp(0.0, 1.0);
        let fpos = pos * (FRAMES - 1) as f32;
        let f0 = (fpos as usize).min(FRAMES - 2);
        let ff = fpos - f0 as f32;
        let table = (p.table as usize).min(TABLE_NAMES.len() - 1);

        // ---- ピッチ ----
        let base = if self.expr.is_active() {
            self.freq * self.expr.next_ratio(sr)
        } else {
            self.freq
        };
        let n = (p.unison as usize).clamp(1, MAX_UNISON);
        // いちばん高い声部で段を選ぶ(折り返しを出さない側に寄せる)
        let top = base * 2.0f32.powf(p.detune_cents / 1200.0);
        let level = level_for(top, sr);
        let row0 = bank.row(table, f0, level);
        let row1 = bank.row(table, f0 + 1, level);

        let mut osc = 0.0f32;
        for i in 0..n {
            let spread = if n == 1 {
                0.0
            } else {
                (i as f32 / (n - 1) as f32) * 2.0 - 1.0
            };
            let dt = base * 2.0f32.powf(spread * p.detune_cents / 1200.0) / sr;
            let ph = self.phases[i];
            let a = read(row0, ph);
            let b = read(row1, ph);
            osc += a + (b - a) * ff;
            self.phases[i] = (ph + dt).fract();
        }
        osc /= (n as f32).sqrt();

        // ---- SVF ローパス(TPT) ----
        let fc = (p.cutoff * self.cutoff_mul).clamp(40.0, sr * 0.45);
        let g = (std::f32::consts::PI * fc / sr).tan();
        let k = 2.0 * (1.0 - p.resonance.min(0.95));
        let a1 = 1.0 / (1.0 + g * (g + k));
        let v1 = a1 * (self.ic1 + g * (osc - self.ic2));
        let v2 = self.ic2 + g * v1;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;

        v2 * self.env * self.amp * p.gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::Articulation;

    fn params() -> WavetableParams {
        WavetableParams {
            table: 0,
            position: 0.0,
            pos_env: 0.0,
            pos_decay: 0.3,
            lfo_rate: 0.0,
            lfo_depth: 0.0,
            unison: 1,
            detune_cents: 0.0,
            cutoff: 20000.0,
            resonance: 0.0,
            attack: 0.001,
            decay: 1.0,
            sustain: 1.0,
            release: 0.1,
            gain: 1.0,
        }
    }

    fn render(p: &WavetableParams, freq: f32, n: usize) -> Vec<f32> {
        ensure_tables();
        let mut v = WavetableVoice::start(p, freq, 1.0, Articulation::Normal, 48_000.0);
        (0..n).map(|_| v.next(p)).collect()
    }

    /// 周波数 `f` の成分の振幅(単純な DFT)
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
    fn position_morphs_from_sine_to_rich_waveform() {
        // analog の 0 は正弦波(2 倍音なし)、終端は矩形波(奇数倍音)
        let mut p = params();
        let sine = render(&p, 200.0, 48_000);
        let s = &sine[4800..];
        assert!(bin(s, 200.0) > 0.5);
        assert!(bin(s, 600.0) < 0.01, "{}", bin(s, 600.0));
        p.position = 1.0;
        let sq = render(&p, 200.0, 48_000);
        let q = &sq[4800..];
        let r3 = bin(q, 600.0) / bin(q, 200.0);
        assert!((r3 - 1.0 / 3.0).abs() < 0.05, "3 倍音 ≈ 1/3: {r3}");
        assert!(bin(q, 400.0) < 0.01, "偶数倍音なし");
    }

    #[test]
    fn high_notes_do_not_alias() {
        // 4kHz の saw(analog の 2/3 地点)でも、ナイキストを超える倍音が折り返さない
        let mut p = params();
        p.position = 2.0 / 3.0;
        let x = render(&p, 4000.0, 24_000);
        let x = &x[2400..];
        // 4k の整数倍以外(折り返しで出る 48k-20k=28k→ 8k? など)を測る:
        // 4000 の倍数から外れた周波数 1000Hz・3000Hz にはほぼ何もない
        assert!(bin(x, 4000.0) > 0.3);
        for f in [1000.0, 3000.0, 6000.0, 10_000.0] {
            assert!(bin(x, f) < 0.005, "{f}Hz: {}", bin(x, f));
        }
    }

    #[test]
    fn every_table_sounds_and_position_changes_timbre() {
        for (t, name) in TABLE_NAMES.iter().enumerate() {
            let mut p = params();
            p.table = t as u8;
            p.position = 0.1;
            let a = render(&p, 110.0, 9600);
            p.position = 0.9;
            let b = render(&p, 110.0, 9600);
            let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
            assert!(rms(&a[4800..]) > 0.05, "{name}");
            let diff: Vec<f32> = a.iter().zip(&b).map(|(x, y)| x - y).collect();
            assert!(
                rms(&diff[4800..]) > 0.05,
                "{} で音色が変わる",
                TABLE_NAMES[t]
            );
        }
    }

    #[test]
    fn position_envelope_sweeps_then_settles() {
        // pos_env で鳴り始めだけ明るく(矩形寄り)、すぐ正弦波に戻る
        let mut p = params();
        p.pos_env = 1.0;
        p.pos_decay = 0.1;
        let x = render(&p, 200.0, 48_000);
        let head = bin(&x[0..2400], 600.0);
        let tail = bin(&x[38_400..], 600.0);
        assert!(head > 0.01 && tail < 0.001, "{head} → {tail}");
    }

    #[test]
    fn releases_and_finishes() {
        ensure_tables();
        let p = params();
        let mut v = WavetableVoice::start(&p, 220.0, 1.0, Articulation::Normal, 48_000.0);
        for _ in 0..4800 {
            v.next(&p);
        }
        v.note_off();
        for _ in 0..48_000 {
            v.next(&p);
        }
        assert!(v.finished());
    }
}
