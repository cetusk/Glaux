//! 音程を動かすエフェクト(段階 4 で足したもの): pitch_shift / harmonizer / pitch_correct。
//!
//! 音程を変える本体は、2 本の読み出し口を持つ遅延線(ディレイ・ライン方式のピッチシフター)。
//! 読み出し口を書き込みより速く(遅く)動かすと音程が上がる(下がる)。読み出し口は遅延線の中を
//! ずれていくので、約 21ms ごとにもう 1 本を置き直して sin² の重みで入れ替える。置き直す位置は、
//! 今鳴っている方と波形がいちばんそろう所を相互相関で探す(±10ms。継ぎ目で位相が飛ばず、
//! 単音でも音程が正確で、うなりが出にくい)。遅延線はディレイ系と共有のバッファ(`EffectState` の `dly`)を使う。
//!
//! - 原音は読み出し口の中心の遅れ(動かす幅で 16〜75ms)だけ遅らせて混ぜ、その量を遅れとして申告する
//!   (エンジンが遅延補正する)
//! - pitch_correct は入力の基本周波数を YIN(差分関数の累積平均で正規化した谷)で約 10ms ごとに測り、
//!   キーとスケールのいちばん近い音へ寄せる量を `speed_ms` でなめらかに追う。測るのは 1/4 ほどに間引いた
//!   信号(約 12kHz、窓 640 点 ≒ 53ms、65〜1100Hz)で、1 回あたり約 8 万回の掛け算
//! - 状態は固定長。オーディオスレッドでは確保しない

use crate::effects::{read_frac, smooth_coef, Smoothed, SvfCoeffs, SvfState, DLY_MASK};
use crate::modfx::{choice, e, f, get};
use glaux_core::{ParamMap, ParamSpec};

/// 入れ替えの長さ・置き直す位置を探す幅・波形を比べる長さ(48kHz のサンプル数。サンプルレートに合わせて伸縮)
const GRAIN: f32 = 1024.0;
const SEARCH: f32 = 512.0;
const CORR_WIN: f32 = 512.0;
/// 探すときの間引き(48kHz で 4 サンプルおき)と、比べる点の数の上限
const CORR_STEP: f32 = 4.0;
const CORR_MAX_POINTS: usize = 192;
/// 読み出し口の最短の遅れ(サンプル。3 次補間の余裕)
const BASE: usize = 4;
/// 音程の検出の窓(間引いた後のサンプル数)
const YIN_N: usize = 640;
/// 検出する周期の上限(間引いた後のサンプル数)
const TAU_CAP: usize = 280;
/// 検出の間隔(間引いた後のサンプル数)
const HOP: u32 = 128;
/// YIN のしきい値(これより谷が浅いときは「音程が無い」)
const YIN_THRESHOLD: f32 = 0.15;
/// 検出する範囲(Hz)
const DETECT_MIN_HZ: f32 = 65.0;
const DETECT_MAX_HZ: f32 = 1100.0;

const KEYS: &[&str] = &[
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];
const SCALES: &[(&str, &[u8])] = &[
    ("chromatic", &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]),
    ("major", &[0, 2, 4, 5, 7, 9, 11]),
    ("minor", &[0, 2, 3, 5, 7, 8, 10]),
    ("harmonic_minor", &[0, 2, 3, 5, 7, 8, 11]),
    ("major_pentatonic", &[0, 2, 4, 7, 9]),
    ("minor_pentatonic", &[0, 3, 5, 7, 10]),
    ("blues", &[0, 3, 5, 6, 7, 10]),
];
const SCALE_NAMES: &[&str] = &[
    "chromatic",
    "major",
    "minor",
    "harmonic_minor",
    "major_pentatonic",
    "minor_pentatonic",
    "blues",
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PitchMode {
    Shift {
        semitones: f32,
        cents: f32,
        ratio: f32,
    },
    Harmonizer {
        intervals: [f32; 2],
        ratios: [f32; 2],
        voices: u8,
        spread: f32,
    },
    Correct {
        key: u8,
        /// 使える音(キーからの半音の数のビット)
        mask: u16,
        speed_ms: f32,
        /// 寄せる速さの 1 サンプルあたりの係数(1 = すぐ)
        speed: f32,
        /// 間引きの割合と、間引いた後のサンプルレート
        decim: u32,
        sr_d: f32,
        /// 間引く前のローパス
        aa: SvfCoeffs,
        tau_min: usize,
        tau_max: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PitchParams {
    pub mode: PitchMode,
    pub mix: f32,
    /// 入れ替えの長さ・探す幅・比べる長さ・探すときの間引き(サンプル)
    grain: u32,
    search: u32,
    corr_win: u32,
    step: u32,
    /// 遅れ(原音を遅らせる量 = 読み出し口の中心)
    lat: u32,
    smooth: f32,
    sr: f32,
}

fn ratio_of(semitones: f32) -> f32 {
    (semitones / 12.0).exp2()
}

/// 読み出し口が中心から離れる割合(|比 − 1|)の段: ±4 半音まで / ±12 半音まで / それ以上。
/// 遅れはこの段で決まる(同じ段の中ならオートメーションで動かしても遅れは変わらない)
fn dev_bucket(dev: f32) -> f32 {
    if dev <= 0.26 {
        0.26
    } else if dev <= 1.0 {
        1.0
    } else {
        3.0
    }
}

impl PitchParams {
    fn new(mode: PitchMode, mix: f32, sr: f32, dev: f32) -> Self {
        let scale = (sr / 48_000.0).clamp(0.25, 4.0);
        let grain = (GRAIN * scale) as u32;
        let search = (SEARCH * scale) as u32;
        PitchParams {
            mode,
            mix: mix.clamp(0.0, 1.0),
            grain,
            search,
            corr_win: (CORR_WIN * scale) as u32,
            step: ((CORR_STEP * scale).round() as u32).max(1),
            lat: BASE as u32 + (dev_bucket(dev) * grain as f32).ceil() as u32 + search,
            smooth: smooth_coef(sr),
            sr,
        }
    }

    /// つまみを変えても遅れ(と探す長さ)は焼き込んだときのまま
    fn keep_timing(mut self, from: &PitchParams) -> Self {
        self.lat = from.lat;
        self
    }

    pub fn shift(semitones: f32, cents: f32, mix: f32, sr: f32) -> Self {
        let semitones = semitones.clamp(-24.0, 24.0);
        let cents = cents.clamp(-100.0, 100.0);
        let mode = PitchMode::Shift {
            semitones,
            cents,
            ratio: ratio_of(semitones + cents / 100.0),
        };
        let dev = (ratio_of(semitones + cents / 100.0) - 1.0).abs();
        Self::new(mode, mix, sr, dev)
    }

    pub fn harmonizer(intervals: [f32; 2], voices: u8, spread: f32, mix: f32, sr: f32) -> Self {
        let intervals = intervals.map(|i| i.clamp(-24.0, 24.0));
        let mode = PitchMode::Harmonizer {
            intervals,
            ratios: intervals.map(ratio_of),
            voices: voices.clamp(1, 2),
            spread: spread.clamp(0.0, 1.0),
        };
        let dev = intervals
            .iter()
            .map(|i| (ratio_of(*i) - 1.0).abs())
            .fold(0.0, f32::max);
        Self::new(mode, mix, sr, dev)
    }

    pub fn correct(key: u8, scale: &str, speed_ms: f32, mix: f32, sr: f32) -> Self {
        let steps = SCALES
            .iter()
            .find(|s| s.0 == scale)
            .map_or(SCALES[0].1, |s| s.1);
        let mask = steps.iter().fold(0u16, |m, s| m | (1 << s));
        let speed_ms = speed_ms.clamp(0.0, 500.0);
        let decim = ((sr / 12_000.0).round() as u32).max(1);
        let sr_d = sr / decim as f32;
        let mode = PitchMode::Correct {
            key: key % 12,
            mask,
            speed_ms,
            speed: if speed_ms <= 0.0 {
                1.0
            } else {
                1.0 - (-1.0 / (speed_ms * 0.001 * sr)).exp()
            },
            decim,
            sr_d,
            aa: SvfCoeffs::low_pass(sr, (0.4 * sr_d).min(2000.0)),
            tau_min: ((sr_d / DETECT_MAX_HZ) as usize).max(2),
            tau_max: ((sr_d / DETECT_MIN_HZ).ceil() as usize).min(TAU_CAP),
        };
        // 寄せるのはふつう 1 半音以内(スケールの隙間でも 2 半音)
        Self::new(mode, mix, sr, 0.0)
    }

    /// 遅れ(サンプル)
    pub fn latency(&self) -> u32 {
        self.lat
    }

    pub(crate) fn kind(&self) -> u8 {
        match self.mode {
            PitchMode::Shift { .. } => 0,
            PitchMode::Harmonizer { .. } => 1,
            PitchMode::Correct { .. } => 2,
        }
    }
}

/// 一番近い使える音(MIDI の番号)。`m` は小数の音の高さ
fn nearest_note(m: f32, key: u8, mask: u16) -> f32 {
    let c = m.round() as i32;
    let mut best = c as f32;
    let mut dist = f32::MAX;
    for n in c - 6..=c + 6 {
        let rel = (n - key as i32).rem_euclid(12);
        if mask & (1 << rel) != 0 {
            let d = (n as f32 - m).abs();
            if d < dist {
                dist = d;
                best = n as f32;
            }
        }
    }
    best
}

/// sin²(π·p)(p は 0〜1。Bhaskara の近似で三角関数を使わない。誤差 0.2% 未満)
#[inline]
fn sin2_pi(p: f32) -> f32 {
    let q = p * (1.0 - p);
    let s = 16.0 * q / (5.0 - 4.0 * q);
    s * s
}

/// 1 本の音程を変えた声: 2 本の読み出し口
#[derive(Clone, Copy, Debug, Default)]
struct Voice {
    /// 読み出し口の遅れ(サンプル)
    d: [f32; 2],
    /// 今鳴っている方(もう一方が入ってくる)
    cur: usize,
    /// 入れ替えの中の位置と長さ
    k: u32,
    n: u32,
    started: bool,
}

/// 共有の遅延線の左右の和(`d` サンプル前。0 = 今書いた音)
#[inline]
fn mono(dly: &[Vec<f32>; 2], idx: usize, d: usize) -> f32 {
    let i = idx.wrapping_sub(d) & DLY_MASK;
    dly[0][i] + dly[1][i]
}

impl Voice {
    /// 入ってくる方を置き直す: 中心から `dev·n` ずれた所の前後 `search` で、今鳴っている方と
    /// 波形がいちばんそろう位置(相互相関 ÷ 候補の大きさ)を探す
    fn splice(&mut self, p: &PitchParams, ratio: f32, dly: &[Vec<f32>; 2], idx: usize) {
        let dev = ratio - 1.0;
        let lat = p.lat as f32;
        let room = lat - BASE as f32 - p.search as f32;
        self.n = if dev.abs() < 1e-6 {
            p.grain
        } else {
            ((room / dev.abs()) as u32).clamp(32, p.grain)
        };
        self.k = 0;
        let other = 1 - self.cur;
        let center = lat + dev * self.n as f32;
        if dev == 0.0 {
            // 音程を変えないなら、今と同じ位置(ずれない = ただの遅れ)
            self.d[other] = self.d[self.cur];
            return;
        }
        let step = p.step as usize;
        let pts = ((p.corr_win / p.step) as usize).min(CORR_MAX_POINTS);
        let cur = self.d[self.cur].round().max(0.0) as usize;
        let mut a = [0.0f32; CORR_MAX_POINTS];
        for (m, v) in a.iter_mut().take(pts).enumerate() {
            *v = mono(dly, idx, cur + m * step);
        }
        let base = (center.round() as i64 - p.search as i64).max(BASE as i64) as usize;
        let span = 2 * p.search as usize;
        let score = |start: usize, stride: usize, n: usize| -> f32 {
            let (mut ab, mut bb) = (0.0f32, 1e-9f32);
            for m in 0..n {
                let b = mono(dly, idx, start + m * stride);
                let av = match a.get(m) {
                    Some(v) if stride == step && m < pts => *v,
                    _ => mono(dly, idx, cur + m * stride),
                };
                ab += av * b;
                bb += b * b;
            }
            ab / bb.sqrt()
        };
        // 粗く(間引いて)探し、見つけた所の前後を 1 サンプルずつ詰める。同点なら中心に近い方
        let mid = center.round().max(BASE as f32) as usize;
        let mut best = (mid, f32::MIN);
        let mut off = 0usize;
        while off <= span {
            let at = base + off;
            let sc = score(at, step, pts);
            let closer = at.abs_diff(mid) < best.0.abs_diff(mid);
            if sc > best.1 || (sc == best.1 && closer) {
                best = (at, sc);
            }
            off += step;
        }
        let fine_pts = (p.corr_win as usize / 2).min(4 * CORR_MAX_POINTS);
        let (lo, hi) = (best.0.saturating_sub(step - 1).max(BASE), best.0 + step - 1);
        let mut fine = (best.0, f32::MIN);
        for at in lo..=hi {
            let sc = score(at, 1, fine_pts);
            if sc > fine.1 {
                fine = (at, sc);
            }
        }
        self.d[other] = fine.0 as f32;
    }

    /// 1 サンプル分の 2 本の読み出し口の(重み, 遅れ)を返して進める
    #[inline]
    fn step(
        &mut self,
        p: &PitchParams,
        ratio: f32,
        dly: &[Vec<f32>; 2],
        idx: usize,
    ) -> [(f32, f32); 2] {
        if !self.started {
            self.started = true;
            self.d = [p.lat as f32; 2];
            self.cur = 0;
            self.splice(p, ratio, dly, idx);
        } else if self.k >= self.n {
            // 入れ替え終わり: 入ってきた方が今の音になり、もう一方を置き直す
            self.cur = 1 - self.cur;
            self.splice(p, ratio, dly, idx);
        }
        let g = sin2_pi(self.k as f32 / (2 * self.n) as f32);
        let other = 1 - self.cur;
        let out = [(1.0 - g, self.d[self.cur]), (g, self.d[other])];
        let dev = ratio - 1.0;
        self.d[0] -= dev;
        self.d[1] -= dev;
        self.k += 1;
        out
    }
}

#[inline]
fn read(buf: &[f32], idx: usize, taps: &[(f32, f32); 2]) -> f32 {
    let mut y = 0.0;
    for &(g, d) in taps {
        if g > 1e-6 {
            y += g * read_frac(buf, idx, d);
        }
    }
    y
}

#[derive(Clone, Debug)]
pub struct PitchState {
    voices: [Voice; 2],
    mix: Smoothed,
    // 検出
    aa: [SvfState; 2],
    decim_count: u32,
    ring: [f32; YIN_N],
    ring_idx: usize,
    hop_count: u32,
    lin: [f32; YIN_N],
    diff: [f32; TAU_CAP + 2],
    /// 寄せる量(半音)。目標と、いま
    target: f32,
    corr: f32,
    /// 最後に測った基本周波数(Hz。0 = 音程が無い)
    detected: f32,
}

impl Default for PitchState {
    fn default() -> Self {
        PitchState {
            voices: Default::default(),
            mix: Smoothed::default(),
            aa: Default::default(),
            decim_count: 0,
            ring: [0.0; YIN_N],
            ring_idx: 0,
            hop_count: 0,
            lin: [0.0; YIN_N],
            diff: [0.0; TAU_CAP + 2],
            target: 0.0,
            corr: 0.0,
            detected: 0.0,
        }
    }
}

impl PitchState {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// 最後に測った基本周波数(Hz。0 = 音程が無い)。テスト用
    #[cfg(test)]
    fn detected_hz(&self) -> f32 {
        self.detected
    }

    /// YIN で基本周波数を測る(間引いた信号の窓全体)
    fn yin(&mut self, sr_d: f32, tau_min: usize, tau_max: usize) -> Option<f32> {
        let n = YIN_N;
        let mut energy = 0.0f32;
        for i in 0..n {
            let v = self.ring[(self.ring_idx + i) % n];
            self.lin[i] = v;
            energy += v * v;
        }
        // -60dBFS より小さければ測らない
        if energy / (n as f32) < 1e-6 {
            return None;
        }
        let w = n - tau_max;
        let x = &self.lin;
        let d = &mut self.diff;
        d[0] = 1.0;
        let mut running = 0.0f32;
        for tau in 1..=tau_max {
            let mut s = 0.0f32;
            for j in 0..w {
                let df = x[j] - x[j + tau];
                s += df * df;
            }
            running += s;
            d[tau] = if running > 0.0 {
                s * tau as f32 / running
            } else {
                1.0
            };
        }
        let mut tau = tau_min;
        while tau < tau_max && d[tau] >= YIN_THRESHOLD {
            tau += 1;
        }
        if tau >= tau_max {
            return None;
        }
        while tau + 1 < tau_max && d[tau + 1] < d[tau] {
            tau += 1;
        }
        let (a, b, c) = (d[tau - 1], d[tau], d[tau + 1]);
        let den = a - 2.0 * b + c;
        let shift = if den.abs() > 1e-9 {
            (0.5 * (a - c) / den).clamp(-1.0, 1.0)
        } else {
            0.0
        };
        Some(sr_d / (tau as f32 + shift))
    }

    /// ステレオ 1 サンプル。`dly` は共有の遅延線、`idx` は今回書く位置(呼ぶ側が進める)
    pub fn process(
        &mut self,
        p: &PitchParams,
        dly: &mut [Vec<f32>; 2],
        idx: usize,
        l: f32,
        r: f32,
    ) -> (f32, f32) {
        dly[0][idx] = l;
        dly[1][idx] = r;
        let back = idx.wrapping_sub(p.lat as usize) & DLY_MASK;
        let (dl, dr) = (dly[0][back], dly[1][back]);
        let mix = self.mix.next(p.mix, p.smooth);
        let (wl, wr) = match p.mode {
            PitchMode::Shift { ratio, .. } => {
                let t = self.voices[0].step(p, ratio, dly, idx);
                (read(&dly[0], idx, &t), read(&dly[1], idx, &t))
            }
            PitchMode::Harmonizer {
                ratios,
                voices,
                spread,
                ..
            } => {
                let (mut wl, mut wr) = (0.0, 0.0);
                for (v, (voice, ratio)) in self
                    .voices
                    .iter_mut()
                    .zip(ratios)
                    .take(voices as usize)
                    .enumerate()
                {
                    let t = voice.step(p, ratio, dly, idx);
                    // 2 声なら 1 声目を左、2 声目を右へ広げる
                    let pan = if voices == 2 {
                        if v == 0 {
                            -spread
                        } else {
                            spread
                        }
                    } else {
                        0.0
                    };
                    wl += read(&dly[0], idx, &t) * (1.0 - pan).min(1.0);
                    wr += read(&dly[1], idx, &t) * (1.0 + pan).min(1.0);
                }
                (wl, wr)
            }
            PitchMode::Correct {
                key,
                mask,
                speed,
                decim,
                sr_d,
                aa,
                tau_min,
                tau_max,
                ..
            } => {
                // 間引いて溜め、HOP ごとに測る
                let m = 0.5 * (l + r);
                let y = self.aa[0].process(&aa, 1.0, m);
                let y = self.aa[1].process(&aa, 1.0, y);
                self.decim_count += 1;
                if self.decim_count >= decim {
                    self.decim_count = 0;
                    self.ring[self.ring_idx] = y;
                    self.ring_idx = (self.ring_idx + 1) % YIN_N;
                    self.hop_count += 1;
                    if self.hop_count >= HOP {
                        self.hop_count = 0;
                        match self.yin(sr_d, tau_min, tau_max) {
                            Some(hz) => {
                                self.detected = hz;
                                let note = 69.0 + 12.0 * (hz / 440.0).log2();
                                self.target = nearest_note(note, key, mask) - note;
                            }
                            None => {
                                self.detected = 0.0;
                                self.target = 0.0;
                            }
                        }
                    }
                }
                self.corr += (self.target - self.corr) * speed;
                let t = self.voices[0].step(p, ratio_of(self.corr), dly, idx);
                (read(&dly[0], idx, &t), read(&dly[1], idx, &t))
            }
        };
        (dl + (wl - dl) * mix, dr + (wr - dr) * mix)
    }
}

// ============================= つまみ ==================================

pub static PITCH_SHIFT_SPECS: &[ParamSpec] = &[
    f("semitones", "半音", Some("半音"), -24.0, 24.0, 0.0, "音程を何半音動かすか(12 = 1 オクターブ上、-12 = 下)。ボーカルを ±12 で別人の声・オクターブの重ね、5・7 で 4 度・5 度の重ね。"),
    f("cents", "セント", Some("cent"), -100.0, 100.0, 0.0, "細かい音程(1 半音 = 100)。±5〜15 を原音に薄く混ぜると、ダブリングのような厚み。"),
    f("mix", "ミックス", None, 0.0, 1.0, 1.0, "原音との混ぜ具合(1 で動かした音だけ)。"),
];

pub static HARMONIZER_SPECS: &[ParamSpec] = &[
    f("interval1", "1 声目の音程", Some("半音"), -24.0, 24.0, 4.0, "1 声目を原音から何半音ずらすか(3 = 短 3 度、4 = 長 3 度、7 = 5 度、12 = オクターブ、-12 = 下のオクターブ)。スケールには合わせず、いつも同じ幅で動く。"),
    f("interval2", "2 声目の音程", Some("半音"), -24.0, 24.0, 7.0, "2 声目の音程(voices が 2 のとき)。"),
    e("voices", "声の数", &["1", "2"], "2", "重ねる声の数。"),
    f("spread", "広がり", None, 0.0, 1.0, 0.5, "2 声のとき、1 声目を左・2 声目を右へ広げる量。"),
    f("mix", "ミックス", None, 0.0, 1.0, 0.4, "重ねた声の量(0.5 で原音と同じくらい)。"),
];

pub static PITCH_CORRECT_SPECS: &[ParamSpec] = &[
    e("key", "キー", KEYS, "C", "曲のキー(主音)。"),
    e("scale", "スケール", SCALE_NAMES, "chromatic", "寄せ先の音の並び。chromatic は 12 音どれにでも(キーを知らなくても使える)。major / minor などは曲のキーに合わせると外れた音へ寄らない。"),
    f("speed_ms", "速さ", Some("ms"), 0.0, 500.0, 20.0, "寄せる速さ。0〜5 でケロケロしたロボット声(ハードチューン)、20〜50 で自然に補正、100 以上で軽く寄せるだけ(ビブラートが残る)。"),
    f("mix", "ミックス", None, 0.0, 1.0, 1.0, "原音との混ぜ具合。"),
];

pub fn specs(name: &str) -> Option<&'static [ParamSpec]> {
    Some(match name {
        "pitch_shift" => PITCH_SHIFT_SPECS,
        "harmonizer" => HARMONIZER_SPECS,
        "pitch_correct" => PITCH_CORRECT_SPECS,
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
        info("pitch_shift", "ピッチシフター。長さを変えずに音程だけ動かす(±24 半音 + セント)。オクターブの重ね・声の高さ変え・\
            ダブリング風の厚み。約 20ms 遅れる(遅延補正される)。"),
        info("harmonizer", "ハーモナイザー。原音に、決まった音程(3 度・5 度など)ずらした声を 1〜2 つ重ねる。\
            ボーカル・リードのハモり、厚み。約 20ms 遅れる(遅延補正される)。"),
        info("pitch_correct", "ピッチ補正。歌の音程を測って、キーとスケールの一番近い音へ寄せる。speed_ms が小さいほど\
            ケロケロしたハードチューン、大きいほど自然。単音(歌・管・リード)向けで、和音には効かない。約 20ms 遅れる(遅延補正される)。"),
    ]
}

pub fn bake(name: &str, map: &ParamMap, sr: f32) -> Option<PitchParams> {
    let s = specs(name)?;
    Some(match name {
        "pitch_shift" => PitchParams::shift(
            get(map, s, "semitones"),
            get(map, s, "cents"),
            get(map, s, "mix"),
            sr,
        ),
        "harmonizer" => PitchParams::harmonizer(
            [get(map, s, "interval1"), get(map, s, "interval2")],
            if choice(map, s, "voices") == "1" {
                1
            } else {
                2
            },
            get(map, s, "spread"),
            get(map, s, "mix"),
            sr,
        ),
        "pitch_correct" => PitchParams::correct(
            KEYS.iter()
                .position(|k| *k == choice(map, s, "key"))
                .unwrap_or(0) as u8,
            choice(map, s, "scale"),
            get(map, s, "speed_ms"),
            get(map, s, "mix"),
            sr,
        ),
        _ => return None,
    })
}

impl PitchParams {
    /// オートメーション: 連続のつまみを上書き(確保しない)
    pub fn set_continuous(&mut self, name: &str, v: f32, _sr: f32) -> bool {
        if name == "mix" {
            self.mix = v.clamp(0.0, 1.0);
            return true;
        }
        let sr = self.sr;
        let mix = self.mix;
        match self.mode {
            PitchMode::Shift {
                semitones, cents, ..
            } => match name {
                "semitones" => *self = Self::shift(v, cents, mix, sr).keep_timing(self),
                "cents" => *self = Self::shift(semitones, v, mix, sr).keep_timing(self),
                _ => return false,
            },
            PitchMode::Harmonizer {
                intervals,
                voices,
                spread,
                ..
            } => match name {
                "interval1" => {
                    *self = Self::harmonizer([v, intervals[1]], voices, spread, mix, sr)
                        .keep_timing(self)
                }
                "interval2" => {
                    *self = Self::harmonizer([intervals[0], v], voices, spread, mix, sr)
                        .keep_timing(self)
                }
                "spread" => {
                    *self = Self::harmonizer(intervals, voices, v, mix, sr).keep_timing(self)
                }
                _ => return false,
            },
            PitchMode::Correct {
                ref mut speed_ms,
                ref mut speed,
                ..
            } => match name {
                "speed_ms" => {
                    *speed_ms = v.clamp(0.0, 500.0);
                    *speed = if *speed_ms <= 0.0 {
                        1.0
                    } else {
                        1.0 - (-1.0 / (*speed_ms * 0.001 * sr)).exp()
                    };
                }
                _ => return false,
            },
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::DLY_LEN;
    use glaux_core::ParamValue;

    const SR: f32 = 48_000.0;

    fn map(pairs: &[(&str, ParamValue)]) -> ParamMap {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect()
    }

    fn run(p: &PitchParams, n: usize, input: impl Fn(usize) -> (f32, f32)) -> Vec<(f32, f32)> {
        let mut st = PitchState::default();
        let mut dly = [vec![0.0; DLY_LEN], vec![0.0; DLY_LEN]];
        (0..n)
            .map(|i| {
                let (l, r) = input(i);
                st.process(p, &mut dly, i & DLY_MASK, l, r)
            })
            .collect()
    }

    fn sine(freq: f32) -> impl Fn(usize) -> (f32, f32) {
        move |i| {
            let x = (i as f32 * freq * std::f32::consts::TAU / SR).sin() * 0.5;
            (x, x)
        }
    }

    /// `lo`〜`hi` Hz で一番強い周波数(0.5Hz 刻み)
    fn peak_hz(x: &[f32], lo: f32, hi: f32) -> f32 {
        let mut best = (0.0, 0.0f64);
        let mut fq = lo;
        while fq <= hi {
            let w = (fq * std::f32::consts::TAU / SR) as f64;
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, v) in x.iter().enumerate() {
                re += *v as f64 * (w * i as f64).cos();
                im += *v as f64 * (w * i as f64).sin();
            }
            let m = re * re + im * im;
            if m > best.1 {
                best = (fq, m);
            }
            fq += 0.5;
        }
        best.0
    }

    fn left(v: &[(f32, f32)]) -> Vec<f32> {
        v.iter().map(|s| s.0).collect()
    }

    #[test]
    fn zero_shift_is_a_pure_delay_of_the_reported_latency() {
        let p = bake("pitch_shift", &map(&[]), SR).unwrap();
        let lat = p.latency() as usize;
        assert!((700..900).contains(&lat), "{lat}");
        let input = |i: usize| ((i as f32 * 0.37).sin() * 0.5, (i as f32 * 0.11).cos() * 0.3);
        let out = run(&p, 6000, input);
        for (i, o) in out.iter().enumerate().skip(lat) {
            let (a, b) = input(i - lat);
            assert!((o.0 - a).abs() < 1e-5 && (o.1 - b).abs() < 1e-5, "{i}");
        }
    }

    #[test]
    fn shift_moves_the_pitch() {
        for (semi, cents, from, want) in [
            (12.0, 0.0, 220.0, 440.0),
            (-12.0, 0.0, 440.0, 220.0),
            (7.0, 0.0, 220.0, 329.63),
            (0.0, 50.0, 440.0, 452.89),
        ] {
            let p = PitchParams::shift(semi, cents, 1.0, SR);
            let out = left(&run(&p, 72_000, sine(from)));
            let got = peak_hz(&out[24_000..], want * 0.9, want * 1.1);
            assert!(
                (got - want).abs() < want * 0.004,
                "{semi} 半音 {cents} セント: {got} Hz(期待 {want})"
            );
        }
    }

    #[test]
    fn harmonizer_adds_the_intervals() {
        let p = bake(
            "harmonizer",
            &map(&[
                ("interval1", 4.0.into()),
                ("interval2", 7.0.into()),
                ("spread", 1.0.into()),
            ]),
            SR,
        )
        .unwrap();
        let out = run(&p, 72_000, sine(220.0));
        // 左に長 3 度(277.18Hz)、右に 5 度(329.63Hz)、両方に原音
        let l = left(&out[24_000..]);
        let r: Vec<f32> = out[24_000..].iter().map(|s| s.1).collect();
        assert!((peak_hz(&l, 260.0, 300.0) - 277.18).abs() < 1.5);
        assert!((peak_hz(&r, 310.0, 350.0) - 329.63).abs() < 1.5);
        assert!((peak_hz(&l, 200.0, 240.0) - 220.0).abs() < 1.0);
    }

    #[test]
    fn nearest_note_follows_the_scale() {
        let major = SCALES[1].1.iter().fold(0u16, |m, s| m | (1 << s));
        // A#(70)は C メジャーに無い: 70.2 は B(71)へ、69.8 は A(69)へ
        assert_eq!(nearest_note(70.2, 0, major), 71.0);
        assert_eq!(nearest_note(69.8, 0, major), 69.0);
        // クロマチックならそのまま一番近い音
        assert_eq!(nearest_note(70.2, 0, 0xFFF), 70.0);
        // キーを D にすると C#(61)が入る
        assert_eq!(nearest_note(61.1, 2, major), 61.0);
    }

    #[test]
    fn pitch_correct_pulls_to_the_nearest_note() {
        // 450Hz(A4 から +39 セント)→ クロマチックなら A4 440Hz
        let p = PitchParams::correct(0, "chromatic", 0.0, 1.0, SR);
        let out = left(&run(&p, 96_000, sine(450.0)));
        let got = peak_hz(&out[48_000..], 420.0, 470.0);
        assert!((got - 440.0).abs() < 1.5, "{got}");
        // 474Hz(A#4 から +29 セント)→ C メジャーなら B4 493.88Hz
        let p = PitchParams::correct(0, "major", 0.0, 1.0, SR);
        let out = left(&run(&p, 96_000, sine(474.0)));
        let got = peak_hz(&out[48_000..], 450.0, 510.0);
        assert!((got - 493.88).abs() < 2.0, "{got}");
        // 測った音程
        let mut st = PitchState::default();
        let mut dly = [vec![0.0; DLY_LEN], vec![0.0; DLY_LEN]];
        for i in 0..24_000 {
            let (l, r) = sine(196.0)(i);
            st.process(&p, &mut dly, i & DLY_MASK, l, r);
        }
        assert!(
            (st.detected_hz() - 196.0).abs() < 1.0,
            "{}",
            st.detected_hz()
        );
        // 無音は測らない(寄せない)
        let out = run(&p, 24_000, |_| (0.0, 0.0));
        assert!(out.iter().all(|s| s.0 == 0.0));
    }

    #[test]
    fn automation_matches_baking() {
        let mut p = bake("pitch_shift", &map(&[]), SR).unwrap();
        assert!(p.set_continuous("semitones", 3.0, SR));
        assert_eq!(
            p,
            bake("pitch_shift", &map(&[("semitones", 3.0.into())]), SR).unwrap()
        );
        // 段を越えて動かしても遅れは焼き込んだときのまま
        assert!(p.set_continuous("semitones", 12.0, SR));
        assert_eq!(
            p.latency(),
            bake("pitch_shift", &map(&[]), SR).unwrap().latency()
        );
        let mut p = bake("pitch_correct", &map(&[]), SR).unwrap();
        assert!(p.set_continuous("speed_ms", 80.0, SR));
        assert_eq!(
            p,
            bake("pitch_correct", &map(&[("speed_ms", 80.0.into())]), SR).unwrap()
        );
        assert!(!p.set_continuous("key", 1.0, SR));
    }
}
