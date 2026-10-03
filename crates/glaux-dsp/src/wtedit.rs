//! ウェーブテーブルを作り込む部品: 作る(倍音の設計図・定番の変化)・直す(加工の手順)・確かめる(数値の要約)・
//! 書き出す(配布形式の WAV)。
//!
//! テーブルは 1 周期 [`CYCLE`] 点の波形を枚数ぶん並べた `Vec<f32>`(内蔵の wavetable が鳴らす形。
//! [`crate::UserTable::from_cycles`] で鳴らせる形にする)。作り方と加工は [`TableRecipe`] として JSON で残せる
//! (同じ手順からいつでも作り直せる)。ここはオーディオスレッドの外で使う(確保・FFT をする)。

use rustfft::{num_complex::Complex, FftPlanner};
use serde::{Deserialize, Serialize};

/// 1 周期のサンプル数
pub const CYCLE: usize = 2048;
/// 使う倍音の上限(ナイキストの 1 つ手前)
pub const MAX_HARMONIC: usize = CYCLE / 2 - 1;
/// 枚数の上限(鳴らせる上限と同じ)
pub const MAX_FRAMES: usize = crate::wavetable::MAX_USER_FRAMES;

/// 定番の変化の名前(`shape` に指定できるもの)と、その説明
pub const SHAPES: &[(&str, &str)] = &[
    (
        "sine_to_saw",
        "正弦 → ノコギリ(倍音が下から順に増える。丸い → 明るい)",
    ),
    (
        "sine_to_square",
        "正弦 → 矩形(奇数倍音が増える。丸い → 中空の太い音)",
    ),
    (
        "analog",
        "正弦 → 三角 → ノコギリ → 矩形(内蔵の analog と同じ)",
    ),
    ("pwm", "パルス幅 50% → 5%(細く鼻にかかる)"),
    ("sync", "ハードシンク 1 → 8 倍(ギラついた金属的な変化)"),
    (
        "fm",
        "FM の深さ 0 → 8(同じ高さの正弦で揺らす。ベル・エレピ → 濁ったうなり)",
    ),
    (
        "fm_octave",
        "FM の深さ 0 → 6(1 オクターブ上の正弦で。硬いデジタルの音)",
    ),
    ("vowels", "母音 あ → え → い → お → う(しゃべるような音)"),
    (
        "growl",
        "うなる母音(お → あ → え)を倍音の多い音で(ダブステップのグロウル)",
    ),
    (
        "fold",
        "ウェーブフォールド 1 → 8 倍(折り返すほど倍音が増えて荒れる。ニューロ系)",
    ),
    (
        "harmonic_sweep",
        "明るい倍音の帯が下から上へ動く(ピアノの弦をこするような変化)",
    ),
    (
        "digital",
        "ノコギリを 64 → 2 段に量子化(ビットを落としたような粗い音)",
    ),
    (
        "organ",
        "倍音を 1 本ずつ足すドローバー(内蔵の organ と同じ)",
    ),
];

/// 倍音の設計図の 1 点: テーブルの位置 `pos`(0〜1)での倍音の振幅(1 番目が基音)と位相(回転数 0〜1)
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HarmonicKey {
    pub pos: f32,
    pub amps: Vec<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phases: Option<Vec<f32>>,
}

/// 加工の 1 手順(順に当てる)
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum EditOp {
    /// いちばん大きい所を 0.9 に(`per_frame` で 1 枚ずつ。枚数の間の音量差を消す)
    Normalize {
        #[serde(default)]
        per_frame: bool,
    },
    /// 直流(上下のずれ)を除く
    RemoveDc,
    /// 高域の傾き(1 オクターブあたり dB。負で暗く、正で明るく)
    Tilt { db_per_octave: f32 },
    /// 奇数・偶数の倍音の量(倍率。偶数を 0 で矩形寄りの中空の音、奇数を下げると管楽器から遠ざかる)
    OddEven { odd: f32, even: f32 },
    /// 倍音の帯を上げ下げする(`from`〜`to` 番目を gain_db。-100 以下で消す)
    Band {
        from: usize,
        to: usize,
        gain_db: f32,
    },
    /// 倍音を `max` 番目までに絞る(なめらかに減らす)
    Lowpass { max: usize },
    /// 位相: zero = 全部そろえる(角の立った形)、align = 枚数の間でそろえる(position を動かしても濁らない)、
    /// random = ばらす(同じ倍音でも柔らかい形に。`seed` で決まる)
    Phase {
        mode: String,
        #[serde(default)]
        seed: u32,
    },
    /// 隣の枚数となじませる(0〜1。大きいほど position の変化がなめらか)
    Smooth { amount: f32 },
    /// 並びを逆に
    Reverse,
    /// 枚数を変える(間は隣り合う 2 枚から作る)
    Resize { frames: usize },
    /// 位置 `from`〜`to`(0〜1)だけを取り出す(枚数はそのまま)
    Select { from: f32, to: f32 },
    /// 飽和させる(1〜20。tanh で丸める。倍音が増えて太くなる)
    Saturate { drive: f32 },
    /// 波形を折り返す(1〜8。増えるほど荒れる)
    Fold { gain: f32 },
}

/// 作り方(元)
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TableSource {
    /// 定番の変化([`SHAPES`])
    Shape { name: String },
    /// 倍音の設計図(位置の間は振幅を補間)
    Harmonics { keys: Vec<HarmonicKey> },
}

/// テーブルの作り方の手順(元 → 枚数 → 加工)。JSON で残して、同じものを作り直せる
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TableRecipe {
    pub source: TableSource,
    #[serde(default = "default_frames")]
    pub frames: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edits: Vec<EditOp>,
}

fn default_frames() -> usize {
    64
}

/// 波形の並び(1 周期 [`CYCLE`] 点 × 枚数)
pub type Cycles = Vec<f32>;

/// 枚数
pub fn frame_count(c: &[f32]) -> usize {
    c.len() / CYCLE
}

struct Fft {
    fwd: std::sync::Arc<dyn rustfft::Fft<f32>>,
    inv: std::sync::Arc<dyn rustfft::Fft<f32>>,
}

impl Fft {
    fn new() -> Self {
        let mut p = FftPlanner::new();
        Fft {
            fwd: p.plan_fft_forward(CYCLE),
            inv: p.plan_fft_inverse(CYCLE),
        }
    }

    /// 1 周期 → 倍音(添字 = 倍音番号、0 は直流。振幅 = |c|、sin の位相の基準は Glaux の内蔵テーブルと同じ)
    fn spectrum(&self, cycle: &[f32]) -> Vec<Complex<f32>> {
        let mut b: Vec<Complex<f32>> = cycle.iter().map(|v| Complex::new(*v, 0.0)).collect();
        self.fwd.process(&mut b);
        let s = 2.0 / CYCLE as f32;
        let mut out = vec![Complex::new(0.0, 0.0); MAX_HARMONIC + 1];
        out[0] = b[0] / CYCLE as f32;
        for h in 1..=MAX_HARMONIC {
            out[h] = b[h] * s;
        }
        out
    }

    /// 倍音 → 1 周期(直流も含める)
    fn synth(&self, spec: &[Complex<f32>]) -> Vec<f32> {
        let mut b = vec![Complex::new(0.0f32, 0.0); CYCLE];
        for (h, c) in spec.iter().enumerate().take(MAX_HARMONIC + 1).skip(1) {
            b[h] = *c;
        }
        self.inv.process(&mut b);
        let dc = spec.first().map_or(0.0, |c| c.re);
        b.iter().map(|c| c.re + dc).collect()
    }
}

/// 時間の形 `f(x)`(x は 0〜1 の 1 周期)を 8 倍の細かさで作って倍音に分け、折り返しの無い 1 周期にする
fn bandlimit(f: impl Fn(f32) -> f32) -> Vec<f32> {
    let m = CYCLE * 8;
    let mut buf: Vec<Complex<f32>> = (0..m)
        .map(|i| Complex::new(f(i as f32 / m as f32), 0.0))
        .collect();
    let mut p = FftPlanner::new();
    p.plan_fft_forward(m).process(&mut buf);
    let mut spec = vec![Complex::new(0.0f32, 0.0); MAX_HARMONIC + 1];
    for (h, s) in spec.iter_mut().enumerate().skip(1) {
        *s = buf[h] * (2.0 / m as f32);
    }
    Fft::new().synth(&spec)
}

/// sin(2πhx) の倍音を `amp` の振幅で(Glaux の内蔵テーブルと同じ符号)
fn sin_c(amp: f32) -> Complex<f32> {
    Complex::new(0.0, -amp)
}

/// 定番の変化の、位置 `t`(0〜1)の 1 周期。知らない名前は None
pub fn shape_cycle(name: &str, t: f32) -> Option<Vec<f32>> {
    let t = t.clamp(0.0, 1.0);
    let pi = std::f32::consts::PI;
    let tau = std::f32::consts::TAU;
    let fft = Fft::new();
    let from_amps = |amp: &dyn Fn(usize) -> f32| -> Vec<f32> {
        let mut s = vec![Complex::new(0.0f32, 0.0); MAX_HARMONIC + 1];
        for (h, v) in s.iter_mut().enumerate().skip(1) {
            *v = sin_c(amp(h));
        }
        fft.synth(&s)
    };
    // 内蔵のテーブルと同じもの(段 0 = 全倍音)
    let builtin = |table: &str| {
        let i = crate::wavetable::TABLE_NAMES
            .iter()
            .position(|n| *n == table)?;
        Some(crate::wavetable::builtin_cycle(i, t))
    };
    let c = match name {
        "sine_to_saw" => {
            // 倍音 h は t に合わせて順に入ってくる(上限は 2^(10t) 番目まで、境目はなめらかに)
            let top = 2f32.powf(t * 10.0);
            from_amps(&|h| {
                let w = (top - h as f32 + 1.0).clamp(0.0, 1.0);
                w * 2.0 / (pi * h as f32)
            })
        }
        "sine_to_square" => {
            let top = 2f32.powf(t * 10.0);
            from_amps(&|h| {
                if h % 2 == 0 {
                    return 0.0;
                }
                let w = (top - h as f32 + 1.0).clamp(0.0, 1.0);
                w * 4.0 / (pi * h as f32)
            })
        }
        "analog" => builtin("analog")?,
        "pwm" => builtin("pulse")?,
        "sync" => builtin("sync")?,
        "vowels" => builtin("vocal")?,
        "organ" => builtin("organ")?,
        "fm" => {
            let index = 8.0 * t;
            bandlimit(|x| (tau * x + index * (tau * x).sin()).sin())
        }
        "fm_octave" => {
            let index = 6.0 * t;
            bandlimit(|x| (tau * x + index * (2.0 * tau * x).sin()).sin())
        }
        "growl" => {
            // お → あ → え のフォルマントを、ノコギリの倍音に掛ける(基音 55Hz 想定の低い声)
            const V: [[f32; 3]; 3] = [
                [450.0, 800.0, 2830.0],
                [730.0, 1090.0, 2440.0],
                [530.0, 1840.0, 2480.0],
            ];
            let x = t * 2.0;
            let k = (x.floor() as usize).min(1);
            let f = x - k as f32;
            let fm: Vec<f32> = (0..3)
                .map(|i| V[k][i] * (1.0 - f) + V[k + 1][i] * f)
                .collect();
            let gains = [1.0f32, 0.8, 0.35];
            let bw = [80.0f32, 100.0, 140.0];
            let raw = from_amps(&|h| {
                let hz = 55.0 * h as f32;
                let env: f32 = (0..3)
                    .map(|i| gains[i] / (1.0 + ((hz - fm[i]) / bw[i]).powi(2)))
                    .sum();
                (0.15 + env) * 2.0 / (pi * h as f32)
            });
            // 軽く歪ませてうなりを足す
            let peak = raw.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
            let shaped: Vec<f32> = raw.iter().map(|v| (2.5 * v / peak).tanh()).collect();
            bandlimit(|x| shaped[((x * CYCLE as f32) as usize).min(CYCLE - 1)])
        }
        "fold" => {
            let g = 1.0 + 7.0 * t;
            bandlimit(|x| fold(g * (tau * x).sin()))
        }
        "harmonic_sweep" => {
            // 中心の倍音が 1 → 64 番目へ(対数で)動く、幅 1 オクターブほどの帯 + 弱い基音
            let center = 2f32.powf(t * 6.0);
            from_amps(&|h| {
                let d = (h as f32 / center).log2();
                let band = (-d * d * 2.0).exp();
                if h == 1 {
                    0.3 + band
                } else {
                    band / (h as f32).sqrt()
                }
            })
        }
        "digital" => {
            let levels = 64f32 * (1.0 / 32.0f32).powf(t);
            bandlimit(|x| {
                let v = 2.0 * x - 1.0;
                (v * levels * 0.5).round() / (levels * 0.5)
            })
        }
        _ => return None,
    };
    Some(c)
}

/// 波形の折り返し(±1 を超えた分を内側へ折る)
fn fold(v: f32) -> f32 {
    let x = (v + 1.0).rem_euclid(4.0);
    if x < 2.0 {
        x - 1.0
    } else {
        3.0 - x
    }
}

/// 元からテーブルを作る(加工の前)
pub fn generate(source: &TableSource, frames: usize) -> Result<Cycles, String> {
    let frames = frames.clamp(2, MAX_FRAMES);
    let pos = |k: usize| k as f32 / (frames - 1) as f32;
    match source {
        TableSource::Shape { name } => {
            if shape_cycle(name, 0.0).is_none() {
                let names: Vec<&str> = SHAPES.iter().map(|(n, _)| *n).collect();
                return Err(format!(
                    "shape は {} のどれか(got: {name})",
                    names.join(" / ")
                ));
            }
            let mut out = Vec::with_capacity(frames * CYCLE);
            for k in 0..frames {
                out.extend(shape_cycle(name, pos(k)).unwrap_or_else(|| vec![0.0; CYCLE]));
            }
            Ok(out)
        }
        TableSource::Harmonics { keys } => {
            if keys.is_empty() {
                return Err("keys が空です(位置ごとの倍音の振幅を 1 つ以上)".to_owned());
            }
            if keys.iter().any(|k| k.amps.len() > MAX_HARMONIC) {
                return Err(format!("倍音は {MAX_HARMONIC} 番目まで"));
            }
            let mut keys = keys.clone();
            keys.sort_by(|a, b| a.pos.total_cmp(&b.pos));
            let fft = Fft::new();
            let mut out = Vec::with_capacity(frames * CYCLE);
            for k in 0..frames {
                let t = pos(k);
                // t を挟む 2 点(端は外へ伸ばす)
                let j = keys.iter().rposition(|x| x.pos <= t).unwrap_or(0);
                let (a, b) = (&keys[j], &keys[(j + 1).min(keys.len() - 1)]);
                let f = if b.pos > a.pos {
                    ((t - a.pos) / (b.pos - a.pos)).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let n = a.amps.len().max(b.amps.len());
                let mut s = vec![Complex::new(0.0f32, 0.0); MAX_HARMONIC + 1];
                for (h, slot) in s.iter_mut().enumerate().take(n + 1).skip(1) {
                    let get = |key: &HarmonicKey| key.amps.get(h - 1).copied().unwrap_or(0.0);
                    let ph = |key: &HarmonicKey| {
                        key.phases
                            .as_ref()
                            .and_then(|p| p.get(h - 1).copied())
                            .unwrap_or(0.0)
                    };
                    let amp = get(a) * (1.0 - f) + get(b) * f;
                    let phase = (ph(a) * (1.0 - f) + ph(b) * f) * std::f32::consts::TAU;
                    *slot = sin_c(amp) * Complex::from_polar(1.0, phase);
                }
                out.extend(fft.synth(&s));
            }
            Ok(out)
        }
    }
}

/// 加工を順に当てる
pub fn apply_edits(cycles: &mut Cycles, edits: &[EditOp]) -> Result<(), String> {
    for e in edits {
        apply_edit(cycles, e)?;
    }
    Ok(())
}

/// 枚数ごとに倍音を書き換える
fn map_spectra(cycles: &mut Cycles, f: &mut dyn FnMut(usize, &mut [Complex<f32>])) {
    let fft = Fft::new();
    let n = frame_count(cycles);
    for k in 0..n {
        let frame = &mut cycles[k * CYCLE..(k + 1) * CYCLE];
        let mut s = fft.spectrum(frame);
        f(k, &mut s);
        frame.copy_from_slice(&fft.synth(&s));
    }
}

/// 簡単な乱数(xorshift)
fn rand01(state: &mut u32) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state as f32 / u32::MAX as f32
}

fn apply_edit(cycles: &mut Cycles, e: &EditOp) -> Result<(), String> {
    let n = frame_count(cycles);
    if n == 0 {
        return Err("テーブルが空です".to_owned());
    }
    match e {
        EditOp::Normalize { per_frame } => {
            if *per_frame {
                for f in cycles.chunks_mut(CYCLE) {
                    let p = f.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                    if p > 1e-6 {
                        f.iter_mut().for_each(|v| *v *= 0.9 / p);
                    }
                }
            } else {
                let p = cycles.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                if p > 1e-6 {
                    cycles.iter_mut().for_each(|v| *v *= 0.9 / p);
                }
            }
        }
        EditOp::RemoveDc => {
            for f in cycles.chunks_mut(CYCLE) {
                let dc = f.iter().sum::<f32>() / CYCLE as f32;
                f.iter_mut().for_each(|v| *v -= dc);
            }
        }
        EditOp::Tilt { db_per_octave } => {
            let d = db_per_octave.clamp(-24.0, 24.0);
            map_spectra(cycles, &mut |_, s| {
                for (h, c) in s.iter_mut().enumerate().skip(1) {
                    *c *= 10f32.powf(d * (h as f32).log2() / 20.0);
                }
            });
        }
        EditOp::OddEven { odd, even } => {
            let (o, ev) = (odd.clamp(0.0, 4.0), even.clamp(0.0, 4.0));
            map_spectra(cycles, &mut |_, s| {
                for (h, c) in s.iter_mut().enumerate().skip(2) {
                    *c *= if h % 2 == 1 { o } else { ev };
                }
            });
        }
        EditOp::Band { from, to, gain_db } => {
            let (lo, hi) = ((*from).max(1), (*to).min(MAX_HARMONIC));
            if lo > hi {
                return Err("band の from は to 以下に".to_owned());
            }
            let g = if *gain_db <= -100.0 {
                0.0
            } else {
                10f32.powf(gain_db.clamp(-100.0, 36.0) / 20.0)
            };
            map_spectra(cycles, &mut |_, s| {
                for c in &mut s[lo..=hi] {
                    *c *= g;
                }
            });
        }
        EditOp::Lowpass { max } => {
            let m = (*max).clamp(1, MAX_HARMONIC) as f32;
            map_spectra(cycles, &mut |_, s| {
                for (h, c) in s.iter_mut().enumerate().skip(1) {
                    // 上限の手前 1/2 オクターブでなめらかに下げる
                    let x = (h as f32 / m).log2() * 2.0 + 1.0;
                    *c *= (1.0 - x).clamp(0.0, 1.0);
                }
            });
        }
        EditOp::Phase { mode, seed } => match mode.as_str() {
            "zero" => map_spectra(cycles, &mut |_, s| {
                for c in s.iter_mut().skip(1) {
                    *c = sin_c(c.norm());
                }
            }),
            "random" => {
                let mut st = seed.wrapping_mul(2_654_435_761).max(1);
                let ph: Vec<f32> = (0..=MAX_HARMONIC).map(|_| rand01(&mut st)).collect();
                map_spectra(cycles, &mut |_, s| {
                    for (h, c) in s.iter_mut().enumerate().skip(2) {
                        *c = sin_c(c.norm())
                            * Complex::from_polar(1.0, ph[h] * std::f32::consts::TAU);
                    }
                });
            }
            "align" => align_phases(cycles),
            other => {
                return Err(format!(
                    "phase の mode は zero / align / random(got: {other})"
                ))
            }
        },
        EditOp::Smooth { amount } => {
            let a = amount.clamp(0.0, 1.0);
            if a > 0.0 && n > 2 {
                let src = cycles.clone();
                let w = a * 0.5;
                for k in 0..n {
                    let prev = &src[k.saturating_sub(1) * CYCLE..][..CYCLE];
                    let next = &src[(k + 1).min(n - 1) * CYCLE..][..CYCLE];
                    let cur = &mut cycles[k * CYCLE..(k + 1) * CYCLE];
                    for i in 0..CYCLE {
                        cur[i] = cur[i] * (1.0 - w) + (prev[i] + next[i]) * 0.5 * w;
                    }
                }
            }
        }
        EditOp::Reverse => {
            let src = cycles.clone();
            for k in 0..n {
                cycles[k * CYCLE..(k + 1) * CYCLE]
                    .copy_from_slice(&src[(n - 1 - k) * CYCLE..(n - k) * CYCLE]);
            }
        }
        EditOp::Resize { frames } => {
            *cycles = resample_frames(cycles, 0.0, 1.0, (*frames).clamp(2, MAX_FRAMES));
        }
        EditOp::Select { from, to } => {
            let (a, b) = (from.clamp(0.0, 1.0), to.clamp(0.0, 1.0));
            if b <= a {
                return Err("select の to は from より大きく".to_owned());
            }
            *cycles = resample_frames(cycles, a, b, n.max(2));
        }
        EditOp::Saturate { drive } => {
            let d = drive.clamp(1.0, 20.0);
            shape_frames(cycles, |v| (d * v).tanh());
        }
        EditOp::Fold { gain } => {
            let g = gain.clamp(1.0, 8.0);
            shape_frames(cycles, |v| fold(g * v));
        }
    }
    Ok(())
}

/// 1 枚ずつ波形の値を写し、折り返さないように倍音を絞る(8 倍の細かさで写す)
fn shape_frames(cycles: &mut Cycles, f: impl Fn(f32) -> f32) {
    let peak = cycles.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
    for frame in cycles.chunks_mut(CYCLE) {
        let src: Vec<f32> = frame.iter().map(|v| v / peak).collect();
        let read = |x: f32| {
            let p = x * CYCLE as f32;
            let i = (p as usize).min(CYCLE - 1);
            let fr = p - i as f32;
            src[i] + (src[(i + 1) % CYCLE] - src[i]) * fr
        };
        frame.copy_from_slice(&bandlimit(|x| f(read(x))));
    }
}

/// 位置 `a`〜`b` の範囲を `frames` 枚に並べ直す(間は隣り合う 2 枚を混ぜる)
fn resample_frames(cycles: &[f32], a: f32, b: f32, frames: usize) -> Cycles {
    let n = frame_count(cycles);
    let mut out = Vec::with_capacity(frames * CYCLE);
    for k in 0..frames {
        let t = a + (b - a) * k as f32 / (frames - 1).max(1) as f32;
        let x = t * (n - 1) as f32;
        let i = (x as usize).min(n.saturating_sub(2));
        let f = if n > 1 { x - i as f32 } else { 0.0 };
        let j = (i + 1).min(n - 1);
        for s in 0..CYCLE {
            let p = cycles[i * CYCLE + s];
            let q = cycles[j * CYCLE + s];
            out.push(p + (q - p) * f);
        }
    }
    out
}

/// 枚数の間で位相をそろえる: 各枚を回して、基音の位相を前の枚に合わせる
/// (隣の波形が同じ向きに並ぶので、position を動かしても打ち消し合って細ったり濁ったりしない)
pub fn align_phases(cycles: &mut Cycles) {
    let fft = Fft::new();
    let n = frame_count(cycles);
    let mut prev: Option<f32> = None;
    for k in 0..n {
        let frame = &mut cycles[k * CYCLE..(k + 1) * CYCLE];
        let s = fft.spectrum(frame);
        // 基音が弱いときは、いちばん強い倍音の位相を基音相当に割って使う
        let (h, c) = s
            .iter()
            .enumerate()
            .skip(1)
            .take(16)
            .max_by(|a, b| a.1.norm().total_cmp(&b.1.norm()))
            .map(|(h, c)| (h, *c))
            .unwrap_or((1, Complex::new(0.0, 0.0)));
        if c.norm() < 1e-6 {
            continue;
        }
        let phase = c.arg() / h as f32;
        let target = prev.unwrap_or(phase);
        // 回す量(サンプル)
        let turns = (phase - target) / std::f32::consts::TAU;
        let shift = (turns * CYCLE as f32).round() as isize;
        if shift != 0 {
            let src = frame.to_vec();
            for (i, v) in frame.iter_mut().enumerate() {
                *v = src[((i as isize - shift).rem_euclid(CYCLE as isize)) as usize];
            }
        }
        prev = Some(target);
    }
}

/// 2 つのテーブルを混ぜる(`amount` 0 = a、1 = b。枚数は a にそろえる)
pub fn mix(a: &[f32], b: &[f32], amount: f32) -> Cycles {
    let n = frame_count(a);
    let b = resample_frames(b, 0.0, 1.0, n.max(2));
    let w = amount.clamp(0.0, 1.0);
    a.iter()
        .zip(&b)
        .map(|(x, y)| x * (1.0 - w) + y * w)
        .collect()
}

/// 2 つのテーブルをつなぐ(a の後に b。上限の枚数を超えたら全体を詰める)
pub fn concat(a: &[f32], b: &[f32]) -> Cycles {
    let mut out = a.to_vec();
    out.extend_from_slice(b);
    let n = frame_count(&out);
    if n > MAX_FRAMES {
        out = resample_frames(&out, 0.0, 1.0, MAX_FRAMES);
    }
    out
}

/// 作り方の手順からテーブルを作る
pub fn build(recipe: &TableRecipe) -> Result<Cycles, String> {
    let mut c = generate(&recipe.source, recipe.frames)?;
    apply_edits(&mut c, &recipe.edits)?;
    Ok(c)
}

/// テーブルの中身の要約(1 枚分)
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FrameSummary {
    /// 位置(0〜1)
    pub pos: f32,
    /// 音量(RMS、dB。いちばん大きい枚を 0)
    pub level_db: f32,
    /// 明るさ: 倍音の重心(何番目の倍音あたりにエネルギーがあるか)
    pub centroid: f32,
    /// -40dB より大きい倍音の数(多いほど豊か・荒い)
    pub harmonics: usize,
    /// 奇数倍音の割合(0〜1。1 に近いほど中空の矩形寄り、0.5 前後でノコギリ寄り)
    pub odd_ratio: f32,
    /// 前の枚からの変わり方(倍音の分布の差。0〜1)
    pub change: f32,
}

/// テーブル全体の要約
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TableSummary {
    pub frames: usize,
    /// 枚数が多いときは等間隔に抜き出した要約
    pub rows: Vec<FrameSummary>,
    /// 明るさの幅(いちばん暗い枚と明るい枚の重心)
    pub centroid_range: (f32, f32),
    /// 枚ごとの変わり方のいちばん大きい所(位置と量。段差のように聞こえる所)
    pub biggest_step: (f32, f32),
    /// 枚ごとの変わり方の平均(大きいほど position でよく動く)
    pub mean_change: f32,
    /// 直流のいちばん大きい所(0.05 を超えるとクリックの原因)
    pub max_dc: f32,
    /// 言葉の手がかり
    pub notes: Vec<String>,
}

/// テーブルを数値で要約する(AI が仕上がりを確かめる手がかり)。`rows` は抜き出す数
pub fn describe(cycles: &[f32], rows: usize) -> TableSummary {
    let fft = Fft::new();
    let n = frame_count(cycles);
    let mut all = Vec::with_capacity(n);
    let mut prev_dist: Option<Vec<f32>> = None;
    let mut max_dc = 0.0f32;
    for k in 0..n {
        let frame = &cycles[k * CYCLE..(k + 1) * CYCLE];
        let s = fft.spectrum(frame);
        max_dc = max_dc.max(s[0].re.abs());
        let mags: Vec<f32> = s.iter().skip(1).map(|c| c.norm()).collect();
        let energy: f32 = mags.iter().map(|m| m * m).sum::<f32>().max(1e-12);
        let centroid = mags
            .iter()
            .enumerate()
            .map(|(i, m)| (i + 1) as f32 * m * m)
            .sum::<f32>()
            / energy;
        let peak = mags.iter().fold(0.0f32, |m, v| m.max(*v)).max(1e-9);
        let harmonics = mags.iter().filter(|m| **m > peak * 0.01).count();
        let odd: f32 = mags
            .iter()
            .enumerate()
            .filter(|(i, _)| i % 2 == 0)
            .map(|(_, m)| m * m)
            .sum();
        let upper: f32 = mags.iter().skip(1).map(|m| m * m).sum::<f32>();
        let odd_upper = mags
            .iter()
            .enumerate()
            .skip(1)
            .filter(|(i, _)| i % 2 == 0)
            .map(|(_, m)| m * m)
            .sum::<f32>();
        let odd_ratio = if upper > 1e-9 {
            odd_upper / upper
        } else {
            (odd / energy).min(1.0)
        };
        let dist: Vec<f32> = mags.iter().map(|m| m * m / energy).collect();
        let change = prev_dist.as_ref().map_or(0.0, |p| {
            p.iter().zip(&dist).map(|(a, b)| (a - b).abs()).sum::<f32>() * 0.5
        });
        prev_dist = Some(dist);
        let rms = (frame.iter().map(|v| v * v).sum::<f32>() / CYCLE as f32).sqrt();
        all.push((
            rms,
            FrameSummary {
                pos: if n > 1 {
                    k as f32 / (n - 1) as f32
                } else {
                    0.0
                },
                level_db: 0.0,
                centroid,
                harmonics,
                odd_ratio,
                change,
            },
        ));
    }
    let top = all.iter().fold(0.0f32, |m, (r, _)| m.max(*r)).max(1e-9);
    for (r, s) in all.iter_mut() {
        s.level_db = 20.0 * (*r / top).max(1e-6).log10();
    }
    let frames: Vec<FrameSummary> = all.into_iter().map(|(_, s)| s).collect();
    let cmin = frames.iter().map(|f| f.centroid).fold(f32::MAX, f32::min);
    let cmax = frames.iter().map(|f| f.centroid).fold(0.0f32, f32::max);
    let biggest = frames
        .iter()
        .skip(1)
        .max_by(|a, b| a.change.total_cmp(&b.change))
        .map_or((0.0, 0.0), |f| (f.pos, f.change));
    let mean_change = if n > 1 {
        frames.iter().skip(1).map(|f| f.change).sum::<f32>() / (n - 1) as f32
    } else {
        0.0
    };
    let mut notes = Vec::new();
    if cmax > cmin * 3.0 {
        notes.push(format!(
            "position で明るさが大きく変わる(倍音の重心 {cmin:.1} → {cmax:.1} 番目)"
        ));
    } else if cmax < cmin * 1.2 {
        notes.push("position を動かしても明るさはあまり変わらない".to_owned());
    }
    if biggest.1 > (mean_change * 4.0).max(0.15) {
        notes.push(format!(
            "位置 {:.2} あたりで急に変わる(position を動かすと段差に聞こえやすい。smooth や resize でなめらかに)",
            biggest.0
        ));
    }
    if max_dc > 0.05 {
        notes.push("直流が残っている(remove_dc で除くとクリックを防げる)".to_owned());
    }
    let lmin = frames.iter().map(|f| f.level_db).fold(0.0f32, f32::min);
    if lmin < -12.0 {
        notes.push(format!(
            "枚数の間の音量差が大きい(いちばん小さい枚は {lmin:.0} dB。normalize per_frame でそろえられる)"
        ));
    }
    let take = rows.clamp(1, n.max(1));
    let rows: Vec<FrameSummary> = (0..take)
        .map(|i| {
            let k = if take == 1 {
                0
            } else {
                i * (n - 1) / (take - 1)
            };
            frames[k].clone()
        })
        .collect();
    TableSummary {
        frames: n,
        rows,
        centroid_range: (cmin, cmax),
        biggest_step: biggest,
        mean_change,
        max_dc,
        notes,
    }
}

/// 配布形式のウェーブテーブルの WAV(32bit 浮動小数・モノラル・1 周期 2048 点の並び)の中身。
/// 1 周期の長さを示す `clm ` の塊(Serum などが読む)を付ける
pub fn to_wav_bytes(cycles: &[f32]) -> Vec<u8> {
    let mut data = Vec::with_capacity(cycles.len() * 4);
    for v in cycles {
        data.extend_from_slice(&v.to_le_bytes());
    }
    let clm = format!("<!>{CYCLE} 10000000 wavetable (glaux)");
    let mut clm_bytes = clm.into_bytes();
    if clm_bytes.len() % 2 == 1 {
        clm_bytes.push(0);
    }
    let fmt_len = 16u32;
    let riff_len = 4 + (8 + fmt_len) + (8 + clm_bytes.len() as u32) + (8 + data.len() as u32);
    let mut out = Vec::with_capacity(riff_len as usize + 8);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_len.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&fmt_len.to_le_bytes());
    out.extend_from_slice(&3u16.to_le_bytes()); // 浮動小数
    out.extend_from_slice(&1u16.to_le_bytes()); // モノラル
    out.extend_from_slice(&48_000u32.to_le_bytes());
    out.extend_from_slice(&(48_000u32 * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(b"clm ");
    out.extend_from_slice(&(clm_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&clm_bytes);
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn centroid(c: &[f32], k: usize) -> f32 {
        let s = describe(&c[k * CYCLE..(k + 1) * CYCLE], 1);
        s.rows[0].centroid
    }

    #[test]
    fn every_shape_makes_a_band_limited_table_that_changes() {
        for (name, _) in SHAPES {
            let c = generate(
                &TableSource::Shape {
                    name: (*name).into(),
                },
                16,
            )
            .unwrap();
            assert_eq!(frame_count(&c), 16, "{name}");
            assert!(c.iter().all(|v| v.is_finite()), "{name}");
            let peak = c.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak > 0.05, "{name}: {peak}");
            let s = describe(&c, 4);
            assert!(s.mean_change > 1e-4, "{name} は position で変わる: {s:?}");
        }
        assert!(generate(
            &TableSource::Shape {
                name: "nope".into()
            },
            4
        )
        .is_err());
    }

    #[test]
    fn sine_to_saw_gets_brighter() {
        let c = generate(
            &TableSource::Shape {
                name: "sine_to_saw".into(),
            },
            8,
        )
        .unwrap();
        assert!(centroid(&c, 0) < 1.05);
        assert!(centroid(&c, 7) > 3.0, "{}", centroid(&c, 7));
    }

    #[test]
    fn harmonics_keys_interpolate_between_positions() {
        let src = TableSource::Harmonics {
            keys: vec![
                HarmonicKey {
                    pos: 0.0,
                    amps: vec![1.0],
                    phases: None,
                },
                HarmonicKey {
                    pos: 1.0,
                    amps: vec![0.0, 0.0, 1.0],
                    phases: None,
                },
            ],
        };
        let c = generate(&src, 3).unwrap();
        let fft = Fft::new();
        let mid = fft.spectrum(&c[CYCLE..2 * CYCLE]);
        assert!((mid[1].norm() - 0.5).abs() < 1e-3);
        assert!((mid[3].norm() - 0.5).abs() < 1e-3);
        assert!(mid[2].norm() < 1e-4);
    }

    #[test]
    fn edits_change_the_spectrum_as_named() {
        let mut c = generate(
            &TableSource::Shape {
                name: "analog".into(),
            },
            4,
        )
        .unwrap();
        let before = centroid(&c, 3);
        apply_edits(
            &mut c,
            &[EditOp::Tilt {
                db_per_octave: -6.0,
            }],
        )
        .unwrap();
        assert!(centroid(&c, 3) < before);
        // 偶数倍音を消すと奇数だけ
        apply_edits(
            &mut c,
            &[EditOp::OddEven {
                odd: 1.0,
                even: 0.0,
            }],
        )
        .unwrap();
        let s = describe(&c, 4);
        assert!(s.rows[3].odd_ratio > 0.99, "{:?}", s.rows[3]);
        // 帯を消す
        apply_edits(
            &mut c,
            &[EditOp::Band {
                from: 2,
                to: 1023,
                gain_db: -120.0,
            }],
        )
        .unwrap();
        assert!(centroid(&c, 3) < 1.01);
        // 枚数を変える・逆に・取り出す
        apply_edits(&mut c, &[EditOp::Resize { frames: 9 }, EditOp::Reverse]).unwrap();
        assert_eq!(frame_count(&c), 9);
        apply_edits(&mut c, &[EditOp::Select { from: 0.0, to: 0.5 }]).unwrap();
        assert_eq!(frame_count(&c), 9);
        assert!(apply_edits(&mut c, &[EditOp::Select { from: 0.5, to: 0.1 }]).is_err());
    }

    #[test]
    fn saturate_and_fold_add_harmonics_without_aliasing_garbage() {
        let mut c = generate(
            &TableSource::Harmonics {
                keys: vec![HarmonicKey {
                    pos: 0.0,
                    amps: vec![1.0],
                    phases: None,
                }],
            },
            2,
        )
        .unwrap();
        apply_edits(&mut c, &[EditOp::Fold { gain: 4.0 }]).unwrap();
        let s = describe(&c, 2);
        assert!(s.rows[0].harmonics > 5, "{:?}", s.rows[0]);
        assert!(c.iter().all(|v| v.is_finite() && v.abs() < 2.0));
    }

    #[test]
    fn align_phases_makes_neighbours_add_up() {
        // 同じ正弦を位相だけずらした 2 枚: そろえると真ん中(混ぜた所)が細らない
        let mut c: Vec<f32> = (0..CYCLE)
            .map(|i| (std::f32::consts::TAU * i as f32 / CYCLE as f32).sin())
            .chain(
                (0..CYCLE).map(|i| (std::f32::consts::TAU * i as f32 / CYCLE as f32 + 2.5).sin()),
            )
            .collect();
        let mid_rms = |c: &[f32]| {
            (c[..CYCLE]
                .iter()
                .zip(&c[CYCLE..])
                .map(|(a, b)| ((a + b) * 0.5).powi(2))
                .sum::<f32>()
                / CYCLE as f32)
                .sqrt()
        };
        let before = mid_rms(&c);
        align_phases(&mut c);
        assert!(mid_rms(&c) > before + 0.2, "{} → {}", before, mid_rms(&c));
        assert!(mid_rms(&c) > 0.69);
    }

    #[test]
    fn recipe_roundtrips_through_json_and_rebuilds_the_same_table() {
        let r = TableRecipe {
            source: TableSource::Shape {
                name: "growl".into(),
            },
            frames: 8,
            edits: vec![
                EditOp::Smooth { amount: 0.5 },
                EditOp::Phase {
                    mode: "random".into(),
                    seed: 3,
                },
                EditOp::Normalize { per_frame: true },
            ],
        };
        let j = serde_json::to_string(&r).unwrap();
        let back: TableRecipe = serde_json::from_str(&j).unwrap();
        assert_eq!(back, r);
        assert_eq!(build(&back).unwrap(), build(&r).unwrap());
        assert!(crate::UserTable::from_cycles(&build(&r).unwrap()).is_some());
    }

    #[test]
    fn describe_points_out_steps_and_dc() {
        let mut c = generate(
            &TableSource::Harmonics {
                keys: vec![HarmonicKey {
                    pos: 0.0,
                    amps: vec![1.0],
                    phases: None,
                }],
            },
            4,
        )
        .unwrap();
        // 最後の枚だけ別物にして、直流も足す
        for v in &mut c[3 * CYCLE..] {
            *v = if *v > 0.0 { 0.9 } else { -0.5 };
        }
        let s = describe(&c, 4);
        assert!(s.max_dc > 0.05);
        assert!(s.notes.iter().any(|n| n.contains("直流")));
        assert!((s.biggest_step.0 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn wav_bytes_have_the_clm_chunk_and_read_back() {
        let c = generate(&TableSource::Shape { name: "pwm".into() }, 4).unwrap();
        let bytes = to_wav_bytes(&c);
        assert!(bytes.windows(4).any(|w| w == b"clm "));
        let r = hound::WavReader::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(r.spec().channels, 1);
        let back: Vec<f32> = r.into_samples::<f32>().map(|v| v.unwrap()).collect();
        assert_eq!(back, c);
    }
}
