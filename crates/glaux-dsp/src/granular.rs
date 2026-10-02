//! グラニュラー音源 `granular`。
//!
//! 取り込んだ音声素材から、短い粒(グレイン。数 ms〜数百 ms)を次々に切り出して重ねる。
//! 声・楽器・環境音から、元の素材とは別物の持続音・パッド・きらめき・ざわめきを作る
//! (時間を止めた音、引き伸ばした音、粒の雲)。
//!
//! - 粒の読み出し位置 position(素材の頭 0 〜 終わり 1)と、そのばらつき spray
//! - 粒の長さ grain_ms・1 秒あたりの数 density・形(窓)window
//! - 粒ごとの音程のばらつき pitch_rand・左右への散らばり spread
//! - 押さえている間に読み出し位置を進める scan(0 = 止まった時間)
//! - 全体の ADSR
//!
//! 粒の音程は、鍵盤の高さと root の差で決まる(root で元の高さ)。
//! 粒は 1 音につき最大 [`MAX_GRAINS`] 個を固定の配列で持ち、オーディオスレッドでアロケーションしない。
//! 乱数は音ごとの種から作る(同じ曲は何度描き出しても同じ音)。

use crate::sampler::SampleData;
use glaux_core::Articulation;
use std::sync::Arc;

/// 1 音で同時に鳴らす粒の上限
pub const MAX_GRAINS: usize = 32;

/// 粒の形(窓)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GrainWindow {
    /// なめらかな山(既定。柔らかい雲)
    #[default]
    Hann,
    /// 三角(少しはっきりした粒)
    Triangle,
    /// 台形(頭と終わりだけ短くフェード。粒の中身がそのまま聞こえる)
    Trapezoid,
    /// 鋭い立ち上がりと減衰(打楽器的な粒。パラパラ・チリチリ)
    Perc,
}

impl GrainWindow {
    pub const NAMES: [&'static str; 4] = ["hann", "triangle", "trapezoid", "perc"];

    pub fn parse(s: &str) -> GrainWindow {
        match s {
            "triangle" => GrainWindow::Triangle,
            "trapezoid" => GrainWindow::Trapezoid,
            "perc" => GrainWindow::Perc,
            _ => GrainWindow::Hann,
        }
    }

    /// 粒の中の位置 t(0..1)での高さ
    #[inline]
    fn at(self, t: f32) -> f32 {
        match self {
            GrainWindow::Hann => {
                let s = crate::fm4::sin_turns(t * 0.5);
                s * s
            }
            GrainWindow::Triangle => 1.0 - (2.0 * t - 1.0).abs(),
            GrainWindow::Trapezoid => (t * 10.0).min(1.0).min((1.0 - t) * 10.0),
            GrainWindow::Perc => {
                // 立ち上がり 3%、その後は指数で減衰(終わりで 0 に寄せる)
                if t < 0.03 {
                    t / 0.03
                } else {
                    let d = (t - 0.03) / 0.97;
                    (1.0 - d) * (1.0 - d) * (1.0 - d)
                }
            }
        }
    }
}

/// 焼き込み済みパラメータ(1 トラック分)
#[derive(Clone, Debug)]
pub struct GranularParams {
    /// 素材(無ければ無音)
    pub data: Option<Arc<SampleData>>,
    /// この鍵盤で素材の元の高さ
    pub root: u8,
    /// 読み出し位置 0..=1
    pub position: f32,
    /// 位置のばらつき(秒)
    pub spray: f32,
    /// 粒の長さ(秒)
    pub grain: f32,
    /// 1 秒あたりの粒の数
    pub density: f32,
    /// 粒ごとの音程のばらつき(半音)
    pub pitch_rand: f32,
    /// 左右への散らばり 0..=1
    pub spread: f32,
    pub window: GrainWindow,
    /// 押さえている間に位置を進める速さ(1 = 素材の元の速さ。負で逆へ)
    pub scan: f32,
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    /// リニアゲイン(dB から変換済み)
    pub gain: f32,
}

impl PartialEq for GranularParams {
    fn eq(&self, other: &Self) -> bool {
        let same_data = match (&self.data, &other.data) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        same_data
            && self.root == other.root
            && self.position == other.position
            && self.spray == other.spray
            && self.grain == other.grain
            && self.density == other.density
            && self.pitch_rand == other.pitch_rand
            && self.spread == other.spread
            && self.window == other.window
            && self.scan == other.scan
            && self.attack == other.attack
            && self.decay == other.decay
            && self.sustain == other.sustain
            && self.release == other.release
            && self.gain == other.gain
    }
}

/// 粒 1 つ(ボイスを小さく保つため 32 バイトに詰める)
#[derive(Clone, Copy, Debug, Default)]
struct Grain {
    /// 素材の読み出し位置(素材のサンプル番号)
    pos: f64,
    /// 1 出力サンプルあたりの進み
    step: f32,
    /// 縮小版の段(進みの 2 を底とする対数。粒の間は一定)
    lg: f32,
    /// 経過・長さ(出力サンプル。長さ 0 = 使っていない)
    age: u32,
    len: u32,
    /// 左右の位置 −1..1
    pan: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    Attack,
    Decay,
    Release,
}

#[derive(Clone, Copy, Debug)]
pub struct GranularVoice {
    grains: [Grain; MAX_GRAINS],
    /// 次の粒までの出力サンプル数
    until_next: f32,
    /// 押さえてからの時間(秒。scan の位置の計算)
    elapsed: f64,
    /// 鍵盤の高さによる速さ(素材のレートの変換込み)
    base_step: f64,
    env: f32,
    stage: Stage,
    amp: f32,
    rng: u32,
    pub(crate) expr: crate::expr::PitchExpr,
    sample_rate: f32,
    /// 1 サンプルの秒数
    dt: f64,
    /// 係数(attack の 1 歩・decay・release・重なりの補正)と、次に求め直すまでのサンプル数
    coefs: [f32; 4],
    ctrl: u32,
}

/// 係数を求め直す間隔(サンプル)
const CTRL: u32 = 32;

impl GranularVoice {
    pub fn start(
        p: &GranularParams,
        pitch: u8,
        vel: f32,
        articulation: Articulation,
        sample_rate: f32,
    ) -> Self {
        Self::start_seeded(p, pitch, vel, articulation, sample_rate, pitch as u32)
    }

    /// 乱数の種を渡して鳴らす(音ごとに違う種で、同じ音を繰り返しても粒の並びが変わる)
    pub fn start_seeded(
        p: &GranularParams,
        pitch: u8,
        vel: f32,
        articulation: Articulation,
        sample_rate: f32,
        seed: u32,
    ) -> Self {
        let amp = if articulation == Articulation::Accent {
            (vel * 1.3).min(1.0)
        } else {
            vel
        };
        let src_rate = p.data.as_ref().map_or(sample_rate, |d| d.sample_rate) as f64;
        let semis = pitch as f64 - p.root as f64;
        GranularVoice {
            grains: [Grain::default(); MAX_GRAINS],
            until_next: 0.0,
            elapsed: 0.0,
            base_step: src_rate / sample_rate as f64 * 2f64.powf(semis / 12.0),
            env: 0.0,
            stage: Stage::Attack,
            amp,
            rng: (seed ^ 0x2545_f491).wrapping_mul(0x9e37_79b9) | 1,
            expr: crate::expr::PitchExpr::new(articulation, sample_rate),
            sample_rate,
            dt: 1.0 / sample_rate as f64,
            coefs: [0.0; 4],
            ctrl: 0,
        }
    }

    pub fn note_off(&mut self) {
        self.stage = Stage::Release;
    }

    pub fn finished(&self) -> bool {
        self.stage == Stage::Release && self.env < 1e-4
    }

    /// 0..1 の乱数(xorshift32)
    #[inline]
    fn rand(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        x as f32 / u32::MAX as f32
    }

    /// 中央の成分だけ
    pub fn next(&mut self, p: &GranularParams) -> f32 {
        self.next_stereo(p).0
    }

    /// (中央, 左右の差)
    pub fn next_stereo(&mut self, p: &GranularParams) -> (f32, f32) {
        let sr = self.sample_rate;
        // 係数は CTRL サンプルごとに求め直す(割り算・平方根を 1 サンプルごとにしない)
        if self.ctrl == 0 {
            self.coefs = [
                1.0 / (p.attack.max(0.0005) * sr),
                (6.9 / (p.decay.max(0.005) * sr)).min(1.0),
                (6.9 / (p.release.max(0.005) * sr)).min(1.0),
                // 重なりの数で音量をそろえる(重なるほど大きくならないように。無相関の和なので √)
                1.0 / (p.density * p.grain).max(1.0).sqrt(),
            ];
            self.ctrl = CTRL;
        }
        self.ctrl -= 1;
        // ---- 全体の ADSR ----
        match self.stage {
            Stage::Attack => {
                self.env += self.coefs[0];
                if self.env >= 1.0 {
                    self.env = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.env += (p.sustain - self.env) * self.coefs[1];
            }
            Stage::Release => {
                self.env -= self.env * self.coefs[2];
            }
        }
        let Some(data) = p.data.as_deref() else {
            return (0.0, 0.0);
        };
        let n = data.frames.len();
        if n < 8 {
            return (0.0, 0.0);
        }
        let ratio = if self.expr.is_active() {
            self.expr.next_ratio(sr) as f64
        } else {
            1.0
        };
        self.elapsed += self.dt;

        // ---- 新しい粒 ----
        self.until_next -= 1.0;
        if self.until_next <= 0.0 {
            let interval = sr / p.density.max(0.1);
            // 間隔に ±30% のゆらぎ(機械的な周期の唸りを出さない)
            self.until_next += interval * (0.7 + 0.6 * self.rand());
            if self.until_next < 1.0 {
                self.until_next = 1.0;
            }
            if let Some(slot) = self.grains.iter().position(|g| g.len == 0) {
                let len = (p.grain * sr).max(16.0) as u32;
                let src = data.sample_rate as f64;
                let centre = p.position as f64 * n as f64 + p.scan as f64 * self.elapsed * src;
                let jitter = (self.rand() as f64 * 2.0 - 1.0) * p.spray as f64 * src;
                let semis = (self.rand() * 2.0 - 1.0) * p.pitch_rand;
                let step = self.base_step * 2f64.powf(semis as f64 / 12.0);
                // 粒が素材の外へはみ出さないように、頭を詰める(scan で端を越えたら折り返す)
                let span = len as f64 * step;
                let room = (n as f64 - 2.0 - span).max(0.0);
                let mut start = (centre + jitter).rem_euclid(n as f64);
                if start > room {
                    start = room;
                }
                let pan = (self.rand() * 2.0 - 1.0) * p.spread;
                self.grains[slot] = Grain {
                    pos: start,
                    step: step as f32,
                    lg: step.log2() as f32,
                    age: 0,
                    len,
                    pan,
                };
            }
        }

        // ---- 粒を鳴らす ----
        let (mut mid, mut side) = (0.0f32, 0.0f32);
        for g in self.grains.iter_mut().filter(|g| g.len != 0) {
            let t = g.age as f32 / g.len as f32;
            let w = p.window.at(t);
            let i = g.pos as usize;
            let s = if i + 1 < n {
                data.read_at_level(g.pos, g.lg as f64)
            } else {
                0.0
            };
            let v = s * w;
            mid += v;
            side += v * g.pan;
            g.pos += g.step as f64 * ratio;
            g.age += 1;
            if g.age >= g.len {
                g.len = 0;
            }
        }
        let g = self.env * self.amp * p.gain * self.coefs[3];
        (mid * g, side * g)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f32, secs: f32) -> Arc<SampleData> {
        let sr = 48_000.0;
        let frames = (0..(sr * secs) as usize)
            .map(|i| (i as f32 * freq * std::f32::consts::TAU / sr).sin() * 0.5)
            .collect();
        Arc::new(SampleData::mono(frames, sr))
    }

    fn params(data: Arc<SampleData>) -> GranularParams {
        data.prepare_mips();
        GranularParams {
            data: Some(data),
            root: 60,
            position: 0.5,
            spray: 0.02,
            grain: 0.08,
            density: 30.0,
            pitch_rand: 0.0,
            spread: 0.0,
            window: GrainWindow::Hann,
            scan: 0.0,
            attack: 0.01,
            decay: 0.5,
            sustain: 1.0,
            release: 0.2,
            gain: 1.0,
        }
    }

    fn render(p: &GranularParams, pitch: u8, n: usize) -> Vec<(f32, f32)> {
        let mut v = GranularVoice::start(p, pitch, 1.0, Articulation::Normal, 48_000.0);
        (0..n).map(|_| v.next_stereo(p)).collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

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
    fn grains_sustain_a_short_sample_at_the_right_pitch() {
        // 0.5 秒の 440Hz から、2 秒の持続音(root で元の高さ、1 オクターブ上は 880Hz)
        let p = params(tone(440.0, 0.5));
        let x: Vec<f32> = render(&p, 60, 96_000).iter().map(|s| s.0).collect();
        assert!(
            rms(&x[72_000..]) > 0.05,
            "伸び続ける: {}",
            rms(&x[72_000..])
        );
        assert!(bin(&x[24_000..72_000], 440.0) > bin(&x[24_000..72_000], 880.0) * 5.0);
        let y: Vec<f32> = render(&p, 72, 48_000).iter().map(|s| s.0).collect();
        assert!(bin(&y[12_000..], 880.0) > bin(&y[12_000..], 440.0) * 5.0);
    }

    #[test]
    fn spread_makes_it_stereo_and_density_keeps_the_level() {
        let mut p = params(tone(300.0, 1.0));
        let mono = render(&p, 60, 48_000);
        assert!(mono.iter().all(|s| s.1 == 0.0));
        p.spread = 1.0;
        let st = render(&p, 60, 48_000);
        let side: Vec<f32> = st.iter().map(|s| s.1).collect();
        assert!(rms(&side[4800..]) > 0.02, "左右に散らばる");
        // 粒の数を増やしても音量は大きく変わらない(±6dB 以内)
        let lv = |density: f32| {
            let mut p = params(tone(300.0, 1.0));
            p.density = density;
            p.spray = 0.2;
            p.pitch_rand = 0.3;
            let x: Vec<f32> = render(&p, 60, 48_000).iter().map(|s| s.0).collect();
            rms(&x[9600..])
        };
        let (a, b) = (lv(10.0), lv(120.0));
        assert!(a / b < 2.0 && b / a < 2.0, "{a} / {b}");
    }

    #[test]
    fn windows_and_scan_change_the_sound() {
        // 前半 220Hz・後半 660Hz の素材。position 0.25 なら 220Hz、scan で後半へ進む
        let sr = 48_000.0;
        let frames: Vec<f32> = (0..96_000)
            .map(|i| {
                let f = if i < 48_000 { 220.0 } else { 660.0 };
                (i as f32 * f * std::f32::consts::TAU / sr).sin() * 0.5
            })
            .collect();
        let mut p = params(Arc::new(SampleData::mono(frames, sr)));
        p.position = 0.25;
        let x: Vec<f32> = render(&p, 60, 96_000).iter().map(|s| s.0).collect();
        let late = &x[72_000..];
        assert!(bin(late, 220.0) > bin(late, 660.0) * 3.0, "止まった時間");
        // 0.5 倍の速さで進めると、1.5〜2 秒には 0.25 + (0.75〜1.0) 秒 = 後半を読んでいる
        p.scan = 0.5;
        let x: Vec<f32> = render(&p, 60, 96_000).iter().map(|s| s.0).collect();
        let late = &x[72_000..];
        assert!(bin(late, 660.0) > bin(late, 220.0) * 3.0, "scan で後半へ");
        // 窓の形はどれも 0 から始まって 0 で終わる(粒の継ぎ目でプツッといわない)
        for w in GrainWindow::NAMES {
            let w = GrainWindow::parse(w);
            assert!(w.at(0.0).abs() < 1e-3, "{w:?}");
            assert!(w.at(0.999).abs() < 0.05, "{w:?}");
            assert!(w.at(0.5) > 0.05, "{w:?}");
        }
    }

    #[test]
    fn same_seed_same_grains_and_release_finishes() {
        let p = params(tone(440.0, 0.5));
        let a = render(&p, 60, 9600);
        let b = render(&p, 60, 9600);
        assert_eq!(a, b, "同じ種なら同じ粒の並び");
        let mut v = GranularVoice::start(&p, 60, 1.0, Articulation::Normal, 48_000.0);
        for _ in 0..4800 {
            v.next(&p);
        }
        v.note_off();
        for _ in 0..48_000 {
            v.next(&p);
        }
        assert!(v.finished());
        // 素材が無ければ無音(落ちない)
        let mut q = p.clone();
        q.data = None;
        assert!(render(&q, 60, 4800).iter().all(|s| *s == (0.0, 0.0)));
    }
}
