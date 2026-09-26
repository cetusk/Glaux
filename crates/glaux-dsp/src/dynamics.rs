//! マルチバンドコンプとトランジェントシェイパー。
//!
//! - マルチバンドコンプ: Linkwitz-Riley 4 次(バターワース 2 次の 2 段)で低・中・高の 3 帯域に分け、
//!   帯域ごとにコンプ(ソフトニー 6dB、ステレオリンクのピーク検出)を掛けて足し戻す。
//!   低域には高い方の分かれ目のオールパスを掛けて位相をそろえるので、圧縮しなければ
//!   足し戻した音は元の音と同じ大きさ(周波数特性が平ら)になる
//! - トランジェントシェイパー: 速い包絡と遅い包絡の差で「打点(アタック)」と「余韻(サステイン)」を見分け、
//!   それぞれを持ち上げたり下げたりする。音量のしきい値が無いので、どんな音量の素材にも同じように効く
//!
//! どちらもアロケーションなし(`Copy` の状態だけ)。

use crate::effects::{time_coef, CompressorParams, SvfCoeffs, SvfState};

/// 帯域の数
pub const BANDS: usize = 3;
/// 帯域のコンプの固定のニー(dB)
const BAND_KNEE_DB: f32 = 6.0;

/// 焼き込み前の値(オートメーションで 1 つ変えたときに作り直す用)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MultibandRaw {
    pub low_freq: f32,
    pub high_freq: f32,
    pub threshold_db: [f32; BANDS],
    pub ratio: [f32; BANDS],
    pub gain_db: [f32; BANDS],
    pub attack_ms: f32,
    pub release_ms: f32,
    pub sample_rate: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MultibandParams {
    lp1: SvfCoeffs,
    hp1: SvfCoeffs,
    lp2: SvfCoeffs,
    hp2: SvfCoeffs,
    ap2: SvfCoeffs,
    bands: [CompressorParams; BANDS],
    pub raw: MultibandRaw,
}

impl MultibandParams {
    pub fn new(raw: MultibandRaw) -> Self {
        let sr = raw.sample_rate.max(1.0);
        let lo = raw.low_freq.clamp(40.0, 1000.0);
        // 高い方の分かれ目は低い方の 1 オクターブ上より上に
        let hi = raw.high_freq.clamp(lo * 2.0, 12_000.0);
        let bands = std::array::from_fn(|b| CompressorParams {
            threshold_db: raw.threshold_db[b].clamp(-60.0, 0.0),
            ratio: raw.ratio[b].clamp(1.0, 20.0),
            knee_db: BAND_KNEE_DB,
            attack_coef: time_coef(raw.attack_ms, sr),
            release_coef: time_coef(raw.release_ms, sr),
            makeup: 10.0_f32.powf(raw.gain_db[b].clamp(-24.0, 24.0) / 20.0),
            rms: false,
            rms_coef: 0.0,
            sc_hpf: None,
            sc_hpf_hz: 20.0,
        });
        MultibandParams {
            lp1: SvfCoeffs::low_pass(sr, lo),
            hp1: SvfCoeffs::high_pass(sr, lo),
            lp2: SvfCoeffs::low_pass(sr, hi),
            hp2: SvfCoeffs::high_pass(sr, hi),
            ap2: SvfCoeffs::all_pass(sr, hi),
            bands,
            raw,
        }
    }
}

/// 1 チャンネル分の分割フィルタ(LR4 = 2 次を 2 段)
#[derive(Clone, Copy, Debug, Default)]
struct Split {
    lp1: [SvfState; 2],
    hp1: [SvfState; 2],
    lp2: [SvfState; 2],
    hp2: [SvfState; 2],
    ap2: SvfState,
}

impl Split {
    /// (低, 中, 高)
    #[inline]
    fn bands(&mut self, p: &MultibandParams, x: f32) -> [f32; BANDS] {
        let two = |st: &mut [SvfState; 2], c: &SvfCoeffs, v: f32| {
            let v = st[0].process(c, 1.0, v);
            st[1].process(c, 1.0, v)
        };
        let low = two(&mut self.lp1, &p.lp1, x);
        let rest = two(&mut self.hp1, &p.hp1, x);
        // 中・高を分けたときの位相の回りを、低域にも同じだけ与える
        let low = self.ap2.process(&p.ap2, 1.0, low);
        let mid = two(&mut self.lp2, &p.lp2, rest);
        let high = two(&mut self.hp2, &p.hp2, rest);
        [low, mid, high]
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MultibandState {
    split: [Split; 2],
    /// 帯域ごとの減衰量の追従(分離したピーク検出の 1 段目・2 段目、dB)
    y1: [f32; BANDS],
    env: [f32; BANDS],
}

impl MultibandState {
    #[inline]
    pub fn process(&mut self, p: &MultibandParams, l: f32, r: f32) -> (f32, f32) {
        let bl = self.split[0].bands(p, l);
        let br = self.split[1].bands(p, r);
        let (mut ol, mut or) = (0.0, 0.0);
        for b in 0..BANDS {
            let c = &p.bands[b];
            let level = bl[b].abs().max(br[b].abs());
            let want = -c.gain_db(20.0 * level.max(1e-6).log10());
            self.y1[b] = want.max(c.release_coef * self.y1[b] + (1.0 - c.release_coef) * want);
            self.env[b] = c.attack_coef * self.env[b] + (1.0 - c.attack_coef) * self.y1[b];
            let g = 10.0_f32.powf(-self.env[b] / 20.0) * c.makeup;
            ol += bl[b] * g;
            or += br[b] * g;
        }
        (ol, or)
    }
}

// ========================= トランジェントシェイパー =========================

/// 打点・余韻をどれだけ変えるかの基準(包絡の差がこれだけあれば設定値いっぱいに効く、dB)
const TRANSIENT_REF_DB: f32 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransientParams {
    /// 打点の増減(dB、-12..12)
    pub attack_db: f32,
    /// 余韻の増減(dB、-12..12)
    pub sustain_db: f32,
    /// 包絡の係数: 速い追従(立ち上がり 1ms / 戻り 30ms)、遅い立ち上がり(30ms)、遅い戻り(300ms)
    fast_att: f32,
    fast_rel: f32,
    slow_att: f32,
    slow_rel: f32,
    pub sample_rate: f32,
}

impl TransientParams {
    pub fn new(attack_db: f32, sustain_db: f32, sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        TransientParams {
            attack_db: attack_db.clamp(-12.0, 12.0),
            sustain_db: sustain_db.clamp(-12.0, 12.0),
            fast_att: time_coef(1.0, sr),
            fast_rel: time_coef(30.0, sr),
            slow_att: time_coef(30.0, sr),
            slow_rel: time_coef(300.0, sr),
            sample_rate: sr,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TransientState {
    /// 速い包絡(立ち上がりも戻りも速い)
    fast: f32,
    /// 速い包絡を立ち上がりだけ遅く追う包絡(下がるときは速い包絡にそろえる)。速い包絡との差 = 打点
    slow_up: f32,
    /// 速い包絡を戻りだけ遅く追う包絡(上がるときは速い包絡にそろえる)。速い包絡との差 = 余韻
    slow_down: f32,
}

impl TransientState {
    #[inline]
    pub fn process(&mut self, p: &TransientParams, l: f32, r: f32) -> (f32, f32) {
        let x = l.abs().max(r.abs());
        // 速い包絡は波形の山を追う。遅い 2 本は速い包絡から作る(波形の山と谷の揺れを打点と取り違えないように)
        let c = if x > self.fast {
            p.fast_att
        } else {
            p.fast_rel
        };
        self.fast = c * self.fast + (1.0 - c) * x;
        // 立ち上がりだけ遅い: 打点の間は速い包絡より下にいる
        self.slow_up = if self.fast > self.slow_up {
            p.slow_att * self.slow_up + (1.0 - p.slow_att) * self.fast
        } else {
            self.fast
        };
        // 戻りだけ遅い: 減衰していく余韻の間は速い包絡より上にいる
        self.slow_down = if self.fast > self.slow_down {
            self.fast
        } else {
            p.slow_rel * self.slow_down + (1.0 - p.slow_rel) * self.fast
        };
        let db = |v: f32| 20.0 * v.max(1e-6).log10();
        let attack = (db(self.fast) - db(self.slow_up)).clamp(0.0, TRANSIENT_REF_DB);
        let sustain = (db(self.slow_down) - db(self.fast)).clamp(0.0, TRANSIENT_REF_DB);
        let g_db = (p.attack_db * attack + p.sustain_db * sustain) / TRANSIENT_REF_DB;
        let g = 10.0_f32.powf(g_db / 20.0);
        (l * g, r * g)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw() -> MultibandRaw {
        MultibandRaw {
            low_freq: 200.0,
            high_freq: 3000.0,
            threshold_db: [0.0; BANDS],
            ratio: [1.0; BANDS],
            gain_db: [0.0; BANDS],
            attack_ms: 10.0,
            release_ms: 100.0,
            sample_rate: 48_000.0,
        }
    }

    fn sine_rms_through(p: &MultibandParams, freq: f32) -> f32 {
        let mut st = MultibandState::default();
        let mut sum = 0.0f64;
        let n = 24_000;
        for i in 0..n {
            let x = (i as f32 * freq * std::f32::consts::TAU / 48_000.0).sin() * 0.25;
            let (l, _) = st.process(p, x, x);
            if i >= n / 2 {
                sum += (l * l) as f64;
            }
        }
        ((sum / (n / 2) as f64) as f32).sqrt() / (0.25 / std::f32::consts::SQRT_2)
    }

    #[test]
    fn bands_sum_back_flat_when_not_compressing() {
        let p = MultibandParams::new(raw());
        for f in [50.0, 200.0, 800.0, 3000.0, 9000.0] {
            let g = sine_rms_through(&p, f);
            assert!(
                (20.0 * g.log10()).abs() < 0.1,
                "{f}Hz: {:.2} dB",
                20.0 * g.log10()
            );
        }
    }

    #[test]
    fn compresses_only_the_chosen_band() {
        // 低域だけ強く圧縮: 80Hz は下がり、1kHz・8kHz は変わらない
        let mut r = raw();
        r.threshold_db[0] = -30.0;
        r.ratio[0] = 10.0;
        let p = MultibandParams::new(r);
        let db = |f: f32| 20.0 * sine_rms_through(&p, f).log10();
        assert!(db(80.0) < -6.0, "{:.1}", db(80.0));
        assert!(db(1000.0).abs() < 0.3, "{:.1}", db(1000.0));
        assert!(db(8000.0).abs() < 0.3, "{:.1}", db(8000.0));
        // 帯域の gain_db はその帯域だけ持ち上げる
        let mut r = raw();
        r.gain_db[2] = 6.0;
        let p = MultibandParams::new(r);
        assert!((20.0 * sine_rms_through(&p, 9000.0).log10() - 6.0).abs() < 0.3);
        assert!((20.0 * sine_rms_through(&p, 300.0).log10()).abs() < 0.5);
    }

    /// 減衰する打撃音(立ち上がり 0.5ms、減衰 150ms)を 4 回
    fn hits() -> Vec<f32> {
        (0..48_000)
            .map(|i| {
                let t = (i % 12_000) as f32 / 48_000.0;
                let env = (t / 0.0005).min(1.0) * (-t / 0.15).exp();
                env * (i as f32 * 200.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5
            })
            .collect()
    }

    fn energies(p: &TransientParams) -> (f32, f32) {
        let mut st = TransientState::default();
        let (mut head, mut tail) = (0.0f32, 0.0f32);
        for (i, x) in hits().into_iter().enumerate() {
            let (y, _) = st.process(p, x, x);
            let t = i % 12_000;
            if i >= 12_000 {
                if t < 480 {
                    head += y * y;
                } else if t > 4_800 {
                    tail += y * y;
                }
            }
        }
        (head, tail)
    }

    #[test]
    fn transient_shaper_moves_attack_and_sustain_separately() {
        let (h0, t0) = energies(&TransientParams::new(0.0, 0.0, 48_000.0));
        let (h1, t1) = energies(&TransientParams::new(9.0, 0.0, 48_000.0));
        let (h2, t2) = energies(&TransientParams::new(0.0, -9.0, 48_000.0));
        let db = |a: f32, b: f32| 10.0 * (a / b).log10();
        assert!(db(h1, h0) > 2.0, "打点が上がる: {:.1}", db(h1, h0));
        assert!(
            db(t1, t0).abs() < 1.0,
            "余韻はほぼそのまま: {:.1}",
            db(t1, t0)
        );
        assert!(db(t2, t0) < -3.0, "余韻が下がる: {:.1}", db(t2, t0));
        assert!(
            db(h2, h0).abs() < 1.5,
            "打点はほぼそのまま: {:.1}",
            db(h2, h0)
        );
    }
}
