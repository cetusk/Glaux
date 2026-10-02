//! ウェーブテーブルシンセ `wavetable`。
//!
//! 1 周期ぶんの波形(フレーム)を 16 枚並べたテーブルを `position` で滑らかに行き来して、
//! 音色そのものを動かす(減算式のフィルタ開閉とは違う「形が変わる」変化。現代の EDM・
//! ベースミュージックのうねるベース、変化するパッドの定番)。
//!
//! - テーブルは 5 種(analog / pulse / vocal / sync / organ)。倍音の設計図から逆 FFT で作る
//! - 音声ファイルから作ったテーブル([`UserTable`]、2048 点 × N フレーム)も使える。`table` に素材の ID
//!   (sha256:…)を入れると、エンジンが素材から作って渡す([`WavetableParams::user`])
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

/// 音声から作ったテーブルのフレーム数の上限
pub const MAX_USER_FRAMES: usize = 256;

/// 音声から作ったテーブル([フレーム][段][ROW])。段ごとに倍音を間引いてある(内蔵のテーブルと同じ形)
pub struct UserTable {
    frames: usize,
    data: Vec<f32>,
}

impl std::fmt::Debug for UserTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "UserTable({} フレーム)", self.frames)
    }
}

impl UserTable {
    /// フレーム数(2 以上。1 周期だけの素材は同じものを 2 枚並べる)
    pub fn frames(&self) -> usize {
        self.frames
    }

    fn row(&self, frame: usize, level: usize) -> &[f32] {
        let off = (frame * LEVELS + level) * ROW;
        &self.data[off..off + ROW]
    }

    /// 1 周期 2048 点の波形を並べたもの(長さが 2048 の倍数)からテーブルを作る。
    /// 段ごとに倍音を間引き、全フレームを通したいちばん大きい所で正規化する(フレームの間の音量差は残す)。
    /// 長さが合わない・空なら None。オーディオスレッドの外で呼ぶ
    pub fn from_cycles(cycles: &[f32]) -> Option<UserTable> {
        if cycles.is_empty() || cycles.len() % N != 0 {
            return None;
        }
        let n_in = (cycles.len() / N).min(MAX_USER_FRAMES);
        let frames = n_in.max(2);
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(N);
        let ifft = planner.plan_fft_inverse(N);
        let mut data = vec![0.0f32; frames * LEVELS * ROW];
        let mut spec = vec![Complex::new(0.0f32, 0.0); N];
        let mut buf = vec![Complex::new(0.0f32, 0.0); N];
        for frame in 0..frames {
            let src = &cycles[frame.min(n_in - 1) * N..][..N];
            for (c, v) in spec.iter_mut().zip(src) {
                *c = Complex::new(*v, 0.0);
            }
            fft.process(&mut spec);
            for level in 0..LEVELS {
                let max_h = MAX_HARMONIC >> level;
                buf.fill(Complex::new(0.0, 0.0));
                // 実数の波形 = Σ (2/N) Re(X_h e^{i2πhx})(直流とナイキストは除く)
                for h in 1..=max_h {
                    buf[h] = spec[h] * (2.0 / N as f32);
                }
                ifft.process(&mut buf);
                let off = (frame * LEVELS + level) * ROW;
                let row = &mut data[off..off + ROW];
                for (o, c) in row.iter_mut().zip(&buf) {
                    *o = c.re;
                }
                row[N] = row[0];
            }
        }
        let peak = (0..frames)
            .flat_map(|f| {
                let off = f * LEVELS * ROW;
                data[off..off + ROW].iter()
            })
            .fold(0.0f32, |m, v| m.max(v.abs()));
        if peak <= 1e-6 {
            return None;
        }
        let scale = 0.9 / peak;
        for v in &mut data {
            *v *= scale;
        }
        Some(UserTable { frames, data })
    }
}

/// 焼き込み済みのパラメータに載せる、音声から作ったテーブルへの参照(比較はポインタで)
#[derive(Clone, Debug)]
pub struct UserTableRef(pub std::sync::Arc<UserTable>);

impl PartialEq for UserTableRef {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

/// 音声を、1 周期 2048 点 × `frames` 枚の並びにする(ウェーブテーブルの取り込み)。
///
/// - 長さが 2048 の倍数(256 枚まで)なら、すでにウェーブテーブルの形の素材とみなしてそのまま返す
///   (よくある配布形式: 1 周期 2048 点を並べた WAV)
/// - そうでなければ、音の高さ(1 周期の長さ)を見つけ、鳴っている所を頭から終わりまで `frames` 箇所で
///   1 周期ずつ切り出して 2048 点に引き伸ばす(声・楽器の 1 音の時間変化がそのまま position の変化になる)
/// - 高さが見つからない(雑音・打楽器)ときは 2048 点ずつそのまま切り出す
///
/// 切り出した 1 周期は、終わりと頭がつながるように直線でずれを均し、直流を除く
pub fn cycles_from_audio(x: &[f32], sample_rate: f32, frames: usize) -> Result<Vec<f32>, String> {
    if x.len() >= N && x.len() % N == 0 && x.len() / N <= MAX_USER_FRAMES {
        return Ok(x.to_vec());
    }
    let frames = frames.clamp(1, MAX_USER_FRAMES);
    let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if peak <= 1e-5 {
        return Err("無音の素材です".to_owned());
    }
    // 鳴っている所(いちばん大きい所の 5% を超える範囲)
    let first = x.iter().position(|v| v.abs() > peak * 0.05).unwrap_or(0);
    let last = x
        .iter()
        .rposition(|v| v.abs() > peak * 0.05)
        .unwrap_or(x.len() - 1);
    let period = detect_period(&x[first..=last], sample_rate);
    let span = match period {
        Some(p) => p,
        None => N as f64,
    };
    if ((last - first) as f64) < span + 4.0 {
        return Err("短すぎます(1 周期ぶんの長さがありません)".to_owned());
    }
    let mut out = Vec::with_capacity(frames * N);
    for k in 0..frames {
        let t = if frames == 1 {
            0.5
        } else {
            k as f64 / (frames - 1) as f64
        };
        let mut at = first as f64 + (last as f64 - first as f64 - span - 3.0) * t;
        // 高さがあるときは上向きのゼロ交差に合わせる(フレームの間で位相がそろい、position を動かしても濁らない)
        if period.is_some() {
            let from = at as usize;
            let to = ((at + span) as usize + 2).min(x.len() - 2);
            if let Some(i) = (from..to).find(|&i| x[i] <= 0.0 && x[i + 1] > 0.0) {
                let frac = -x[i] / (x[i + 1] - x[i]);
                at = i as f64 + frac as f64;
            }
        }
        let read = |pos: f64| {
            let i = (pos as usize).min(x.len() - 2);
            crate::sampler::hermite(x, i, (pos - i as f64) as f32)
        };
        let mut cycle: Vec<f32> = (0..N)
            .map(|j| read(at + span * j as f64 / N as f64))
            .collect();
        // 終わりと頭のずれ(音の高さ・形がゆっくり変わる分)を直線で均す(1 周期の外の値 = 次の周期の頭)。
        // ずれが大きいのは頭が段差の上にある(ノコギリ波など)ときで、均すと形が崩れるので触らない
        let gap = read(at + span) - cycle[0];
        let range = cycle.iter().fold(f32::MIN, |m, v| m.max(*v))
            - cycle.iter().fold(f32::MAX, |m, v| m.min(*v));
        if gap.abs() < range * 0.2 {
            for (j, v) in cycle.iter_mut().enumerate() {
                *v -= gap * j as f32 / N as f32;
            }
        }
        let dc = cycle.iter().sum::<f32>() / N as f32;
        out.extend(cycle.iter().map(|v| v - dc));
    }
    Ok(out)
}

/// 1 周期の長さ(サンプル、小数)。YIN の正規化した差分関数で 30Hz〜2kHz を探す。見つからなければ None
fn detect_period(x: &[f32], sample_rate: f32) -> Option<f64> {
    let tau_min = ((sample_rate / 2000.0) as usize).max(2);
    let tau_max = (sample_rate / 30.0) as usize;
    let win = 4096.min(x.len().saturating_sub(tau_max));
    if win < 512 {
        return None;
    }
    // いちばん大きい所(窓 win)で測る
    let step = (win / 2).max(1);
    let mut best = (0usize, 0.0f32);
    let mut i = 0;
    while i + win + tau_max <= x.len() {
        let e: f32 = x[i..i + win].iter().map(|v| v * v).sum();
        if e > best.1 {
            best = (i, e);
        }
        i += step;
    }
    let seg = &x[best.0..];
    let mut d = vec![0.0f32; tau_max + 1];
    for (tau, dv) in d.iter_mut().enumerate().skip(1) {
        *dv = seg[..win]
            .iter()
            .zip(&seg[tau..tau + win])
            .map(|(a, b)| (a - b) * (a - b))
            .sum();
    }
    // 累積平均で正規化
    let mut cum = 0.0f32;
    let mut dn = vec![1.0f32; tau_max + 1];
    for tau in 1..=tau_max {
        cum += d[tau];
        dn[tau] = if cum > 0.0 {
            d[tau] * tau as f32 / cum
        } else {
            1.0
        };
    }
    let mut pick = None;
    let mut tau = tau_min;
    while tau < tau_max {
        if dn[tau] < 0.15 {
            while tau + 1 < tau_max && dn[tau + 1] < dn[tau] {
                tau += 1;
            }
            pick = Some(tau);
            break;
        }
        tau += 1;
    }
    let tau = match pick {
        Some(t) => t,
        None => {
            let (t, v) = (tau_min..tau_max)
                .map(|t| (t, dn[t]))
                .min_by(|a, b| a.1.total_cmp(&b.1))?;
            if v > 0.35 {
                return None;
            }
            t
        }
    };
    // 放物線で小数の位置を補う
    let (a, b, c) = (dn[tau - 1], dn[tau], dn[(tau + 1).min(tau_max)]);
    let den = a - 2.0 * b + c;
    let shift = if den.abs() > 1e-9 {
        (0.5 * (a - c) / den).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    Some(tau as f64 + shift as f64)
}

/// 焼き込み済みパラメータ(1 トラック分)。
#[derive(Clone, Debug, PartialEq)]
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
    /// エンベロープでカットオフを開く量 0..=1(1 で約 +3 オクターブ。0 = 開かない = 従来)
    pub filter_env: f32,
    /// 広がり・揺らぎ・フィルタの種類・LFO など(既定値は従来と同じ音)
    pub tone: crate::tone::ToneParams,
    /// 音声から作ったテーブル(あればこちらを鳴らす。`table` は無視)
    pub user: Option<UserTableRef>,
}

const MAX_UNISON: usize = crate::tone::MAX_UNISON;
/// フィルタの係数を更新する間隔(サンプル)
const CTRL_RATE: u32 = 32;

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
    pub(crate) expr: crate::expr::PitchExpr,
    /// ユニゾンの声部ごとの倍率・いちばん高い声部の倍率と、それを作ったときの (声部数, デチューン)
    ratios: [f32; MAX_UNISON],
    top_ratio: f32,
    ratio_key: (usize, u32),
    /// ベロシティ(カットオフの追従用)
    vel: f32,
    /// 広がり・揺らぎ・フィルタ・LFO の状態
    tone: crate::tone::ToneVoice,
    /// フィルタの係数(制御レートで更新)と、次の更新までのサンプル数
    coefs: crate::tone::SvfCoefs,
    ctrl: u32,
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
        p: &WavetableParams,
        freq: f32,
        vel: f32,
        articulation: glaux_core::Articulation,
        sample_rate: f32,
    ) -> Self {
        Self::start_seeded(p, freq, vel, articulation, sample_rate, freq.to_bits())
    }

    /// 揺らぎの種を渡して鳴らす(音ごとに違う種で、同じ音を繰り返しても少しずつ違う音になる)
    pub fn start_seeded(
        p: &WavetableParams,
        freq: f32,
        vel: f32,
        articulation: glaux_core::Articulation,
        sample_rate: f32,
        seed: u32,
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
        let mut tone = crate::tone::ToneVoice::new(seed);
        tone.start_phases(&p.tone, &mut phases);
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
            expr: crate::expr::PitchExpr::new(articulation, sample_rate),
            ratios: [1.0; MAX_UNISON],
            top_ratio: 1.0,
            ratio_key: (0, u32::MAX),
            vel,
            tone,
            coefs: crate::tone::SvfCoefs::default(),
            ctrl: 0,
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

    /// 中央の成分だけ(モノで使う所。左右に広げていなければ全体)
    pub fn next(&mut self, p: &WavetableParams) -> f32 {
        self.next_stereo(p).0
    }

    /// (中央, 左右の差)。L = 中央 + 差、R = 中央 − 差
    pub fn next_stereo(&mut self, p: &WavetableParams) -> (f32, f32) {
        let Some(bank) = BANK.get() else {
            return (0.0, 0.0);
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

        // ---- 変調(制御レート): LFO・揺らぎ・フィルタのエンベロープ → フィルタの係数 ----
        if self.ctrl == 0 {
            let tp = &p.tone;
            self.tone
                .control(tp, CTRL_RATE, sr, self.stage == Stage::Release);
            if tp.spread > 0.0 {
                self.tone.layout((p.unison as usize).clamp(1, MAX_UNISON));
            }
            let fc = self
                .tone
                .cutoff(
                    tp,
                    p.cutoff * self.cutoff_mul,
                    p.filter_env,
                    self.env,
                    self.vel,
                    self.freq,
                )
                .clamp(40.0, sr * 0.45);
            self.coefs = crate::tone::SvfCoefs::new(fc, p.resonance, sr);
            self.ctrl = CTRL_RATE;
        }
        self.ctrl -= 1;

        // ---- position(基準 + エンベロープ + LFO + 奏法) ----
        self.pos_env -= self.pos_env * (4.6 / (p.pos_decay.max(0.005) * sr)).min(1.0);
        // LFO の深さが 0 のときは sin を求めない(位相だけ進める)
        let lfo = if p.lfo_depth != 0.0 {
            (std::f32::consts::TAU * self.lfo_phase).sin()
        } else {
            0.0
        };
        self.lfo_phase = (self.lfo_phase + p.lfo_rate / sr).fract();
        let pos = (p.position + p.pos_env * self.pos_env + p.lfo_depth * 0.5 * lfo + self.pos_bias)
            .clamp(0.0, 1.0);
        let pos = if self.tone.position_add != 0.0 {
            (pos + self.tone.position_add).clamp(0.0, 1.0)
        } else {
            pos
        };
        let frames = p.user.as_ref().map_or(FRAMES, |u| u.0.frames());
        let fpos = pos * (frames - 1) as f32;
        let f0 = (fpos as usize).min(frames - 2);
        let ff = fpos - f0 as f32;
        let table = (p.table as usize).min(TABLE_NAMES.len() - 1);

        // ---- ピッチ ----
        let base = if self.expr.is_active() {
            self.freq * self.expr.next_ratio(sr)
        } else {
            self.freq
        } * self.tone.pitch_ratio;
        let n = (p.unison as usize).clamp(1, MAX_UNISON);
        // 声部ごとの倍率は、声部数かデチューンが変わったときだけ求め直す(毎サンプルの powf を避ける)
        let key = (n, p.detune_cents.to_bits());
        if self.ratio_key != key {
            self.ratio_key = key;
            for (i, r) in self.ratios.iter_mut().enumerate().take(n) {
                let spread = if n == 1 {
                    0.0
                } else {
                    (i as f32 / (n - 1) as f32) * 2.0 - 1.0
                };
                *r = 2.0f32.powf(spread * p.detune_cents / 1200.0);
            }
            self.top_ratio = 2.0f32.powf(p.detune_cents / 1200.0);
        }
        // いちばん高い声部で段を選ぶ(折り返しを出さない側に寄せる)
        let top = base * self.top_ratio;
        let level = level_for(top, sr);
        let (row0, row1) = match &p.user {
            Some(u) => (u.0.row(f0, level), u.0.row(f0 + 1, level)),
            None => (bank.row(table, f0, level), bank.row(table, f0 + 1, level)),
        };

        let mut osc = 0.0f32;
        let mut side = 0.0f32;
        let stereo = self.tone.stereo;
        // 1 サンプルの位相の進み。fract は 1 を超えたとき(1 周期に 1 回)だけ求める
        let base_dt = base / sr;
        for i in 0..n {
            let dt = base_dt * self.ratios[i] * self.tone.unison_detune(&p.tone, i);
            let ph = self.phases[i];
            let a = read(row0, ph);
            let b = read(row1, ph);
            let v = a + (b - a) * ff;
            osc += v;
            if stereo {
                side += v * self.tone.unison_pan(&p.tone, i);
            }
            let next = ph + dt;
            self.phases[i] = if next >= 1.0 { next.fract() } else { next };
        }
        let norm = (n as f32).sqrt();
        osc /= norm;
        side /= norm;

        // ---- フィルタ(SVF。係数は上の制御レートで更新済み) ----
        let coefs = self.coefs;
        let (mid, side) = self.tone.filter(&p.tone, &coefs, osc, side);
        let (mid, side) = self.tone.apply_pan(mid, side);
        let g = self.env * self.amp * p.gain;
        let a = self.tone.amp_mul;
        (mid * g * a, side * g * a)
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
            filter_env: 0.0,
            tone: Default::default(),
            user: None,
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

    /// ノコギリ波 → 矩形波 へ変わっていく 2 秒の「録音」(220Hz、48kHz)
    fn morphing_take() -> Vec<f32> {
        let sr = 48_000.0f32;
        let n = (sr * 2.0) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / sr;
                let m = i as f32 / n as f32;
                let ph = (t * 220.0).fract();
                let saw = 1.0 - 2.0 * ph;
                let sq = if ph < 0.5 { 1.0 } else { -1.0 };
                (saw * (1.0 - m) + sq * m) * 0.5
            })
            .collect()
    }

    #[test]
    fn audio_becomes_cycles_that_follow_the_take() {
        let x = morphing_take();
        let cycles = cycles_from_audio(&x, 48_000.0, 8).unwrap();
        assert_eq!(cycles.len(), 8 * N);
        // 1 枚目はノコギリ波(2 倍音が強い)、最後は矩形波(2 倍音がほぼ無い)
        let h2 = |c: &[f32]| {
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (j, v) in c.iter().enumerate() {
                let w = std::f32::consts::TAU * 2.0 * j as f32 / N as f32;
                re += v * w.cos();
                im += v * w.sin();
            }
            (re * re + im * im).sqrt() / N as f32
        };
        let first = h2(&cycles[..N]);
        let last = h2(&cycles[7 * N..]);
        assert!(first > 0.05 && last < first * 0.3, "{first} → {last}");
        // すでにテーブルの形(2048 の倍数)ならそのまま
        let raw = vec![0.1f32; 3 * N];
        assert_eq!(cycles_from_audio(&raw, 48_000.0, 16).unwrap().len(), 3 * N);
        assert!(cycles_from_audio(&[0.0; 5000], 48_000.0, 4).is_err());
    }

    #[test]
    fn detects_the_period_of_a_tone() {
        let sr = 48_000.0;
        for f in [55.0f32, 220.0, 1234.0] {
            let x: Vec<f32> = (0..24_000)
                .map(|i| (i as f32 * f * std::f32::consts::TAU / sr).sin())
                .collect();
            let p = detect_period(&x, sr).unwrap();
            assert!((p - (sr / f) as f64).abs() < 0.2, "{f}Hz: {p}");
        }
    }

    #[test]
    fn user_table_plays_and_morphs_without_aliasing() {
        let cycles = cycles_from_audio(&morphing_take(), 48_000.0, 16).unwrap();
        let table = std::sync::Arc::new(UserTable::from_cycles(&cycles).unwrap());
        assert_eq!(table.frames(), 16);
        let mut p = params();
        p.user = Some(UserTableRef(table.clone()));
        // 頭(ノコギリ波)は 2 倍音あり、終わり(矩形波)は 2 倍音なし。鳴らす高さは元と違ってよい
        let a = render(&p, 200.0, 24_000);
        p.position = 1.0;
        let b = render(&p, 200.0, 24_000);
        let (a, b) = (&a[4800..], &b[4800..]);
        assert!(bin(a, 200.0) > 0.3, "{}", bin(a, 200.0));
        assert!(bin(a, 400.0) > 0.1, "ノコギリ: {}", bin(a, 400.0));
        assert!(bin(b, 400.0) < 0.02, "矩形: {}", bin(b, 400.0));
        // 高い音でも折り返さない
        let x = render(&p, 4000.0, 24_000);
        for f in [1000.0, 3000.0, 6000.0] {
            assert!(bin(&x[2400..], f) < 0.005, "{f}Hz: {}", bin(&x[2400..], f));
        }
        // 1 周期だけの素材も鳴る(同じものを 2 枚並べる)
        let one = UserTable::from_cycles(&cycles[..N]).unwrap();
        assert_eq!(one.frames(), 2);
        assert!(UserTable::from_cycles(&cycles[..N - 1]).is_none());
    }
}
