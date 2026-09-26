//! 聴き方の切り替え(モニター)と、ステレオの見え方(相関・ゴニオメーター)。
//!
//! - モニターの切り替え(モノ・サイド・左右入れ替え・クロスフィード)は、出力デバイスへ送る直前だけに掛ける。
//!   書き出し・フリーズ・Godot はそれぞれ自分の [`crate::render::Shared`] を持つので、ここの設定は入らない
//! - 相関とゴニオメーターの点は、マスターの音量の後(クリップ防止の前)の音で測る
//! - ラウドネス(LUFS の M / S / I)・True Peak・スペクトルは、同じ音を輪のバッファに写しておき、
//!   UI の問い合わせのたびに別のスレッドで計算する([`Meter`])
//!
//! オーディオスレッドはアトミックに書くだけ(アロケーション・ロックなし)。

use std::sync::atomic::{AtomicU32, Ordering};

/// ゴニオメーターに残す点の数(2 サンプルに 1 点。48kHz で約 43ms 分)
pub const SCOPE_LEN: usize = 1024;
/// ゴニオメーターの間引き(何サンプルに 1 点残すか)
const SCOPE_DECIM: u32 = 2;
/// 相関の平均の時定数(秒)
const CORR_SECS: f32 = 0.3;
/// クロスフィード: 反対側へ回す低域の境目(Hz)と、低域での直接音と回り込みの差(dB)。bs2b の既定に近い値
const XFEED_HZ: f32 = 700.0;
const XFEED_DB: f32 = 4.5;

/// 聴き方
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MonitorMode {
    /// そのまま
    Stereo,
    /// 左右を足して両耳へ(モノラルで再生されたときの確認。位相の打ち消しが分かる)
    Mono,
    /// 左右の差(サイド)だけ(広がり・リバーブ・位相のずれの確認)
    Side,
    /// 左右を入れ替える(片耳の聞こえ方の癖を打ち消して、偏りを確かめる)
    Swap,
}

impl MonitorMode {
    pub fn name(self) -> &'static str {
        match self {
            MonitorMode::Stereo => "stereo",
            MonitorMode::Mono => "mono",
            MonitorMode::Side => "side",
            MonitorMode::Swap => "swap",
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        Some(match s {
            "stereo" => MonitorMode::Stereo,
            "mono" => MonitorMode::Mono,
            "side" => MonitorMode::Side,
            "swap" => MonitorMode::Swap,
            _ => return None,
        })
    }

    fn bits(self) -> u32 {
        match self {
            MonitorMode::Stereo => 0,
            MonitorMode::Mono => 1,
            MonitorMode::Side => 2,
            MonitorMode::Swap => 3,
        }
    }

    fn from_bits(b: u32) -> Self {
        match b & 3 {
            1 => MonitorMode::Mono,
            2 => MonitorMode::Side,
            3 => MonitorMode::Swap,
            _ => MonitorMode::Stereo,
        }
    }
}

const XFEED_BIT: u32 = 1 << 8;

/// 小さなスピーカーで鳴らしたときの聞こえ方(設計値のフィルタ。実測のデータは使わない)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speaker {
    /// そのまま
    Off,
    /// スマホの内蔵スピーカー: モノラル、300Hz より下と 8kHz より上を削り、2.5kHz あたりが少し出る
    Phone,
    /// ノート PC の内蔵スピーカー: 左右は狭く、150Hz より下と 12kHz より上を削る
    Laptop,
}

impl Speaker {
    pub fn name(self) -> &'static str {
        match self {
            Speaker::Off => "off",
            Speaker::Phone => "phone",
            Speaker::Laptop => "laptop",
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        Some(match s {
            "off" => Speaker::Off,
            "phone" => Speaker::Phone,
            "laptop" => Speaker::Laptop,
            _ => return None,
        })
    }

    fn bits(self) -> u32 {
        match self {
            Speaker::Off => 0,
            Speaker::Phone => 1,
            Speaker::Laptop => 2,
        }
    }

    fn from_bits(b: u32) -> Self {
        match b {
            1 => Speaker::Phone,
            2 => Speaker::Laptop,
            _ => Speaker::Off,
        }
    }

    /// (ハイパス、ローパス、山の周波数、山の高さ dB、左右の幅)
    fn design(self) -> (f32, f32, f32, f32, f32) {
        match self {
            Speaker::Off => (20.0, 20_000.0, 1000.0, 0.0, 1.0),
            Speaker::Phone => (300.0, 8000.0, 2500.0, 4.0, 0.0),
            Speaker::Laptop => (150.0, 12_000.0, 3000.0, 2.0, 0.5),
        }
    }
}

/// 小さなスピーカーのフィルタ(左右)。係数はスピーカーとサンプルレートが変わったときだけ作り直す
#[derive(Default)]
struct SpeakerSim {
    key: Option<(Speaker, u32)>,
    hp: Option<glaux_dsp::SvfCoeffs>,
    lp: Option<glaux_dsp::SvfCoeffs>,
    bell: Option<glaux_dsp::SvfCoeffs>,
    width: f32,
    st: [[glaux_dsp::SvfState; 5]; 2],
}

impl SpeakerSim {
    fn process(&mut self, speaker: Speaker, l: f32, r: f32, sr: f32) -> (f32, f32) {
        let key = (speaker, sr as u32);
        if self.key != Some(key) {
            let (hp, lp, f, g, w) = speaker.design();
            self.hp = Some(glaux_dsp::SvfCoeffs::high_pass(sr, hp));
            self.lp = Some(glaux_dsp::SvfCoeffs::low_pass(sr, lp));
            self.bell = Some(glaux_dsp::SvfCoeffs::bell(sr, f, 1.0, g));
            self.width = w;
            self.st = Default::default();
            self.key = Some(key);
        }
        let (Some(hp), Some(lp), Some(bell)) = (&self.hp, &self.lp, &self.bell) else {
            return (l, r);
        };
        // 幅(0 でモノラル)
        let m = 0.5 * (l + r);
        let s = 0.5 * (l - r) * self.width;
        let mut out = [m + s, m - s];
        for (x, st) in out.iter_mut().zip(self.st.iter_mut()) {
            // 低域は 24dB/oct、高域は 24dB/oct で削る(小さなスピーカーは低音が急に出なくなる)
            let mut v = st[0].process(hp, 1.0, *x);
            v = st[1].process(hp, 1.0, v);
            v = st[2].process(lp, 1.0, v);
            v = st[3].process(lp, 1.0, v);
            *x = st[4].process(bell, 1.0, v);
        }
        (out[0], out[1])
    }
}

/// ラウドネス・スペクトル用に残す音の長さ(フレーム。48kHz で約 0.68 秒。UI の問い合わせの間隔より十分長く)
pub const AUDIO_RING: usize = 1 << 15;

/// UI とオーディオスレッドで共有する部分
pub struct MonitorShared {
    /// 下位 2 ビット = 聴き方、8 ビット目 = クロスフィード
    settings: AtomicU32,
    /// 小さなスピーカーのシミュレーション([`Speaker`])
    speaker: AtomicU32,
    /// 相関(f32 のビット列。NaN = ほぼ無音で測れない)
    correlation: AtomicU32,
    /// ゴニオメーターの点(左, 右 の f32 ビット列を交互に)。リングバッファ
    points: [AtomicU32; SCOPE_LEN * 2],
    /// 次に書く点の位置
    write: AtomicU32,
    /// マスターの音(左, 右 の f32 ビット列を交互に)。輪のバッファ
    audio: Box<[AtomicU32]>,
    /// 書いたフレームの総数(輪の中の位置は AUDIO_RING で割った余り)
    audio_written: std::sync::atomic::AtomicU64,
}

impl Default for MonitorShared {
    fn default() -> Self {
        MonitorShared {
            settings: AtomicU32::new(0),
            speaker: AtomicU32::new(0),
            correlation: AtomicU32::new(f32::NAN.to_bits()),
            points: std::array::from_fn(|_| AtomicU32::new(0)),
            write: AtomicU32::new(0),
            audio: (0..AUDIO_RING * 2).map(|_| AtomicU32::new(0)).collect(),
            audio_written: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

impl MonitorShared {
    pub fn set(&self, mode: MonitorMode, crossfeed: bool) {
        let bits = mode.bits() | if crossfeed { XFEED_BIT } else { 0 };
        self.settings.store(bits, Ordering::Release);
    }

    pub fn set_speaker(&self, speaker: Speaker) {
        self.speaker.store(speaker.bits(), Ordering::Release);
    }

    pub fn speaker(&self) -> Speaker {
        Speaker::from_bits(self.speaker.load(Ordering::Acquire))
    }

    pub fn get(&self) -> (MonitorMode, bool) {
        let b = self.settings.load(Ordering::Acquire);
        (MonitorMode::from_bits(b), b & XFEED_BIT != 0)
    }

    /// 相関(-1 = 逆相、0 = 無関係、+1 = モノラル)。ほぼ無音なら None
    pub fn correlation(&self) -> Option<f32> {
        let v = f32::from_bits(self.correlation.load(Ordering::Relaxed));
        v.is_finite().then_some(v)
    }

    /// ゴニオメーターの点(古い順の (左, 右))。書き込み中の点が混ざることがあるが、表示には問題ない
    pub fn scope_points(&self) -> Vec<(f32, f32)> {
        let w = self.write.load(Ordering::Acquire) as usize;
        (0..SCOPE_LEN)
            .map(|k| {
                let i = (w + k) % SCOPE_LEN;
                (
                    f32::from_bits(self.points[i * 2].load(Ordering::Relaxed)),
                    f32::from_bits(self.points[i * 2 + 1].load(Ordering::Relaxed)),
                )
            })
            .collect()
    }
}

/// オーディオスレッド側の状態
#[derive(Default)]
pub struct MonitorState {
    /// 輪のバッファに書いたフレームの総数(ブロックの終わりに公開する)
    audio_written: u64,
    lr: f32,
    ll: f32,
    rr: f32,
    decim: u32,
    write: usize,
    /// クロスフィードのローパス(左, 右)
    lp: (f32, f32),
    speaker: SpeakerSim,
}

impl MonitorState {
    /// マスターの音を 1 サンプル測る(相関とゴニオメーター)
    pub fn measure(&mut self, shared: &MonitorShared, l: f32, r: f32, sr: f32) {
        let a = 1.0 - (-1.0 / (CORR_SECS * sr.max(1.0))).exp();
        self.lr += (l * r - self.lr) * a;
        self.ll += (l * l - self.ll) * a;
        self.rr += (r * r - self.rr) * a;
        let i = (self.audio_written % AUDIO_RING as u64) as usize;
        shared.audio[i * 2].store(l.to_bits(), Ordering::Relaxed);
        shared.audio[i * 2 + 1].store(r.to_bits(), Ordering::Relaxed);
        self.audio_written += 1;
        self.decim += 1;
        if self.decim >= SCOPE_DECIM {
            self.decim = 0;
            shared.points[self.write * 2].store(l.to_bits(), Ordering::Relaxed);
            shared.points[self.write * 2 + 1].store(r.to_bits(), Ordering::Relaxed);
            self.write = (self.write + 1) % SCOPE_LEN;
        }
    }

    /// ブロックの終わりに、相関とゴニオメーターの書き込み位置を公開する
    pub fn publish(&self, shared: &MonitorShared) {
        let energy = (self.ll * self.rr).sqrt();
        // -70 dBFS 程度より小さければ測れない扱い
        let corr = if energy > 1e-7 {
            (self.lr / energy).clamp(-1.0, 1.0)
        } else {
            f32::NAN
        };
        shared.correlation.store(corr.to_bits(), Ordering::Relaxed);
        shared.write.store(self.write as u32, Ordering::Release);
        shared
            .audio_written
            .store(self.audio_written, Ordering::Release);
    }

    /// 出力デバイスへ送る直前の聴き方の切り替え
    /// 小さなスピーカーのシミュレーションを掛けていれば、クロスフィードは掛けない
    /// (スピーカーで聞く想定なので、ヘッドホン向けの処理は要らない)
    pub fn apply(
        &mut self,
        mode: MonitorMode,
        crossfeed: bool,
        speaker: Speaker,
        l: f32,
        r: f32,
        sr: f32,
    ) -> (f32, f32) {
        let (l, r) = match mode {
            MonitorMode::Stereo => (l, r),
            MonitorMode::Mono => {
                let m = 0.5 * (l + r);
                (m, m)
            }
            MonitorMode::Side => {
                let s = 0.5 * (l - r);
                (s, s)
            }
            MonitorMode::Swap => (r, l),
        };
        if speaker != Speaker::Off {
            return self.speaker.process(speaker, l, r, sr);
        }
        if !crossfeed {
            return (l, r);
        }
        // 反対側の低域を回り込ませ(ヘッドホンでの極端な左右の分離をやわらげる)、
        // 直接音の高域を少し持ち上げて、モノラルの音の高さの釣り合い(周波数特性)は平らに保つ。
        // 低域: 直接 a・回り込み c(c / a = -4.5dB、a + c = 1)。高域: 直接 a + c = 1
        let c = 1.0 / (1.0 + 10.0_f32.powf(XFEED_DB / 20.0));
        let a = 1.0 - c;
        let k = 1.0 - (-std::f32::consts::TAU * XFEED_HZ / sr.max(1.0)).exp();
        self.lp.0 += (l - self.lp.0) * k;
        self.lp.1 += (r - self.lp.1) * k;
        let (hl, hr) = (l - self.lp.0, r - self.lp.1);
        (
            a * l + c * hl + c * self.lp.1,
            a * r + c * hr + c * self.lp.0,
        )
    }
}

// ============================ メーター(RT の外) ============================

/// 1/3 オクターブの帯域の中心(Hz。25Hz〜20kHz の 30 本)
pub const SPECTRUM_BANDS: [f32; 30] = [
    25., 31.5, 40., 50., 63., 80., 100., 125., 160., 200., 250., 315., 400., 500., 630., 800.,
    1000., 1250., 1600., 2000., 2500., 3150., 4000., 5000., 6300., 8000., 10000., 12500., 16000.,
    20000.,
];
/// スペクトルの FFT の長さ
const FFT_LEN: usize = 8192;

/// ラウドネスの読み(LUFS。測れないときは None)
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct LoudnessReading {
    /// 瞬時(400ms)
    pub momentary: Option<f64>,
    /// 短期(3 秒)
    pub short_term: Option<f64>,
    /// 統合(測り始めてから。再生を始めるたびにやり直す)
    pub integrated: Option<f64>,
    /// 測り始めてからの True Peak の最大(dBTP)
    pub true_peak_max: Option<f64>,
    /// 直近の問い合わせの間の True Peak(dBTP)
    pub true_peak: Option<f64>,
}

/// 輪のバッファの音を読んで、ラウドネスとスペクトルを求める(UI の問い合わせのスレッドで動く)
pub struct Meter {
    ebu: Option<ebur128::EbuR128>,
    sample_rate: u32,
    read: u64,
    was_playing: bool,
    reading: LoudnessReading,
    scratch: Vec<f32>,
    fft: std::sync::Arc<dyn rustfft::Fft<f32>>,
    window: Vec<f32>,
}

impl Default for Meter {
    fn default() -> Self {
        let mut planner = rustfft::FftPlanner::<f32>::new();
        Meter {
            ebu: None,
            sample_rate: 0,
            read: 0,
            was_playing: false,
            reading: LoudnessReading::default(),
            scratch: Vec::new(),
            fft: planner.plan_fft_forward(FFT_LEN),
            window: (0..FFT_LEN)
                .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / FFT_LEN as f32).cos())
                .collect(),
        }
    }
}

fn finite(v: Result<f64, ebur128::Error>) -> Option<f64> {
    v.ok()
        .filter(|x| x.is_finite() && *x > -70.0)
        .map(|x| (x * 10.0).round() / 10.0)
}

impl Meter {
    /// 測り直す(統合ラウドネスと True Peak の最大を捨てる)
    pub fn reset(&mut self) {
        self.ebu = None;
        self.reading = LoudnessReading::default();
    }

    /// 前回から増えた音を読んでラウドネスを進める。`playing` が止まっている → 再生の切り替わりで測り直す
    pub fn update(
        &mut self,
        shared: &MonitorShared,
        sample_rate: f64,
        playing: bool,
    ) -> LoudnessReading {
        use ebur128::{EbuR128, Mode};
        let sr = sample_rate.round().max(1.0) as u32;
        if playing && !self.was_playing || sr != self.sample_rate {
            self.reset();
            self.sample_rate = sr;
        }
        self.was_playing = playing;
        let written = shared.audio_written.load(Ordering::Acquire);
        // 読み残しが輪より多ければ(問い合わせが途切れた)、新しい方だけ読む
        let from = self.read.max(written.saturating_sub(AUDIO_RING as u64 - 1));
        self.read = written;
        if !playing || written <= from {
            self.reading.true_peak = None;
            return self.reading;
        }
        if self.ebu.is_none() {
            self.ebu = EbuR128::new(2, sr, Mode::M | Mode::S | Mode::I | Mode::TRUE_PEAK).ok();
        }
        let Some(ebu) = self.ebu.as_mut() else {
            return self.reading;
        };
        self.scratch.clear();
        for n in from..written {
            let i = (n % AUDIO_RING as u64) as usize;
            self.scratch
                .push(f32::from_bits(shared.audio[i * 2].load(Ordering::Relaxed)));
            self.scratch.push(f32::from_bits(
                shared.audio[i * 2 + 1].load(Ordering::Relaxed),
            ));
        }
        if ebu.add_frames_f32(&self.scratch).is_err() {
            return self.reading;
        }
        let db = |v: f64| (v > 0.0).then(|| ((20.0 * v.log10()) * 10.0).round() / 10.0);
        let tp_now = (0..2)
            .filter_map(|c| ebu.prev_true_peak(c).ok())
            .fold(0.0f64, f64::max);
        let tp_max = (0..2)
            .filter_map(|c| ebu.true_peak(c).ok())
            .fold(0.0f64, f64::max);
        self.reading = LoudnessReading {
            momentary: finite(ebu.loudness_momentary()),
            short_term: finite(ebu.loudness_shortterm()),
            integrated: finite(ebu.loudness_global()),
            true_peak_max: db(tp_max),
            true_peak: db(tp_now),
        };
        self.reading
    }

    /// 直近 8192 フレームのスペクトル(1/3 オクターブ、dB。フルスケールのサイン波が 0dB)
    pub fn spectrum(&mut self, shared: &MonitorShared, sample_rate: f64) -> Vec<f32> {
        let written = shared.audio_written.load(Ordering::Acquire);
        let mut buf: Vec<rustfft::num_complex::Complex<f32>> = (0..FFT_LEN)
            .map(|k| {
                let n = written.wrapping_sub(FFT_LEN as u64) + k as u64;
                let i = (n % AUDIO_RING as u64) as usize;
                let l = f32::from_bits(shared.audio[i * 2].load(Ordering::Relaxed));
                let r = f32::from_bits(shared.audio[i * 2 + 1].load(Ordering::Relaxed));
                rustfft::num_complex::Complex::new((l + r) * 0.5 * self.window[k], 0.0)
            })
            .collect();
        if written < FFT_LEN as u64 {
            return vec![-120.0; SPECTRUM_BANDS.len()];
        }
        self.fft.process(&mut buf);
        let bin_hz = sample_rate as f32 / FFT_LEN as f32;
        // Hann 窓の山の高さ(N/4)の 2 乗 × 等価雑音帯域(1.5 ビン)で割ると、サイン波が帯域の中で 0dB になる
        let norm = (FFT_LEN as f32 / 4.0).powi(2) * 1.5;
        let edge = 2f32.powf(1.0 / 6.0);
        SPECTRUM_BANDS
            .iter()
            .map(|&c| {
                let lo = ((c / edge) / bin_hz).floor().max(1.0) as usize;
                let hi = (((c * edge) / bin_hz).ceil() as usize).min(FFT_LEN / 2 - 1);
                if lo > hi || c > sample_rate as f32 / 2.0 {
                    return -120.0;
                }
                let p: f32 = buf[lo..=hi].iter().map(|z| z.norm_sqr()).sum();
                (10.0 * (p / norm).max(1e-12).log10()).max(-120.0)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corr_of(f: impl Fn(usize) -> (f32, f32)) -> Option<f32> {
        let shared = MonitorShared::default();
        let mut st = MonitorState::default();
        for i in 0..48_000 {
            let (l, r) = f(i);
            st.measure(&shared, l, r, 48_000.0);
        }
        st.publish(&shared);
        shared.correlation()
    }

    #[test]
    fn correlation_reads_mono_wide_and_inverted() {
        let s = |i: usize| (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5;
        let c = |i: usize| (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).cos() * 0.5;
        assert!(corr_of(|i| (s(i), s(i))).unwrap() > 0.99, "モノラルは +1");
        assert!(corr_of(|i| (s(i), -s(i))).unwrap() < -0.99, "逆相は -1");
        assert!(
            corr_of(|i| (s(i), c(i))).unwrap().abs() < 0.05,
            "90° ずれは 0"
        );
        assert!(corr_of(|_| (0.0, 0.0)).is_none(), "無音は測れない");
    }

    #[test]
    fn meter_reads_loudness_and_spectrum() {
        // ピーク -20dBFS の 1kHz(左右同じ)を 3 秒流す
        let shared = MonitorShared::default();
        let mut st = MonitorState::default();
        let mut meter = Meter::default();
        let amp = 0.1f32; // -20 dBFS のピーク
        let mut last = LoudnessReading::default();
        for block in 0..30 {
            for i in 0..4800 {
                let n = block * 4800 + i;
                let x = (n as f32 * 1000.0 * std::f32::consts::TAU / 48_000.0).sin() * amp;
                st.measure(&shared, x, x, 48_000.0);
            }
            st.publish(&shared);
            last = meter.update(&shared, 48_000.0, true);
        }
        // サイン波ピーク -20dBFS の RMS は -23dB、左右 2ch の和で +3dB → 約 -20 LUFS
        let i = last.integrated.unwrap();
        assert!((i + 20.0).abs() < 1.0, "統合 {i}");
        assert!((last.momentary.unwrap() - i).abs() < 0.5);
        let tp = last.true_peak_max.unwrap();
        assert!((tp + 20.0).abs() < 0.3, "True Peak {tp}");
        let sp = meter.spectrum(&shared, 48_000.0);
        let k = SPECTRUM_BANDS.iter().position(|b| *b == 1000.0).unwrap();
        assert!((sp[k] + 20.0).abs() < 1.5, "1kHz の帯 {}", sp[k]);
        assert!(sp[3] < sp[k] - 40.0, "離れた帯は小さい {}", sp[3]);
        // 止めて再生し直すと測り直す
        meter.update(&shared, 48_000.0, false);
        let r = meter.update(&shared, 48_000.0, true);
        assert!(r.integrated.is_none());
    }

    #[test]
    fn phone_speaker_cuts_lows_and_is_mono() {
        let run = |f: f32, speaker: Speaker| {
            let mut st = MonitorState::default();
            let mut peak = (0.0f32, 0.0f32);
            for i in 0..24_000 {
                let x = (i as f32 * f * std::f32::consts::TAU / 48_000.0).sin();
                let (l, r) = st.apply(MonitorMode::Stereo, false, speaker, x, -x * 0.2, 48_000.0);
                if i > 12_000 {
                    peak = (peak.0.max(l.abs()), peak.1.max(r.abs()));
                }
            }
            peak
        };
        let low = run(80.0, Speaker::Phone);
        let mid = run(1000.0, Speaker::Phone);
        assert!(low.0 < mid.0 * 0.1, "80Hz は削れる: {low:?} / {mid:?}");
        assert!((mid.0 - mid.1).abs() < 1e-4, "モノラル: {mid:?}");
        // ノート PC は左右が残る(狭く)
        let lap = run(1000.0, Speaker::Laptop);
        assert!((lap.0 - lap.1).abs() > 0.05, "{lap:?}");
    }

    #[test]
    fn scope_keeps_recent_points_in_order() {
        let shared = MonitorShared::default();
        let mut st = MonitorState::default();
        for i in 0..(SCOPE_LEN * 3) {
            st.measure(&shared, i as f32, -(i as f32), 48_000.0);
        }
        st.publish(&shared);
        let pts = shared.scope_points();
        assert_eq!(pts.len(), SCOPE_LEN);
        assert!(pts.windows(2).all(|w| w[1].0 > w[0].0), "古い順");
        assert_eq!(pts.last().unwrap().0, (SCOPE_LEN * 3 - 1) as f32);
    }

    #[test]
    fn monitor_modes_and_flat_crossfeed() {
        let mut st = MonitorState::default();
        assert_eq!(
            st.apply(MonitorMode::Mono, false, Speaker::Off, 1.0, 0.0, 48_000.0),
            (0.5, 0.5)
        );
        assert_eq!(
            st.apply(MonitorMode::Side, false, Speaker::Off, 1.0, 1.0, 48_000.0),
            (0.0, 0.0)
        );
        assert_eq!(
            st.apply(MonitorMode::Swap, false, Speaker::Off, 1.0, 0.0, 48_000.0),
            (0.0, 1.0)
        );
        // クロスフィード: モノラルの音はそのまま、片側だけの低音は反対側へ -4.5dB 回り込む
        let mut st = MonitorState::default();
        for i in 0..4800 {
            let x = (i as f32 * 100.0 * std::f32::consts::TAU / 48_000.0).sin();
            let (l, r) = st.apply(MonitorMode::Stereo, true, Speaker::Off, x, x, 48_000.0);
            assert!((l - x).abs() < 1e-4 && (r - x).abs() < 1e-4);
        }
        let mut st = MonitorState::default();
        let (mut pl, mut pr) = (0.0f32, 0.0f32);
        for i in 0..9600 {
            let x = (i as f32 * 60.0 * std::f32::consts::TAU / 48_000.0).sin();
            let (l, r) = st.apply(MonitorMode::Stereo, true, Speaker::Off, x, 0.0, 48_000.0);
            if i > 4800 {
                pl = pl.max(l.abs());
                pr = pr.max(r.abs());
            }
        }
        let diff = 20.0 * (pl / pr).log10();
        assert!((diff - XFEED_DB).abs() < 0.5, "低音の左右差: {diff:.2} dB");
    }
}
