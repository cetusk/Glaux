//! ノート単位のピッチ表現(ビブラート / チョーキング)。
//!
//! `Articulation::Vibrato` / `Articulation::Bend` と、ノートに描かれた連続
//! ピッチカーブ([`PitchCurve`])をボイス内の周波数比の時間変化として実装する。
//! subtractive / pluck / sampler / sf2 が共用する。

use glaux_core::Articulation;

/// ボイスが持つ固定長のピッチカーブ(サンプル位置, セント)と区間の曲がり方。
/// `glaux_core::Note::pitch_curve` をエンジンがサンプル位置に換算したもの。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PitchCurve {
    pub pts: [(f32, f32); glaux_core::MAX_PITCH_POINTS],
    /// 点 i から次の点までの曲がり方(`glaux_core::CurveShape::code`)
    pub shapes: [u8; glaux_core::MAX_PITCH_POINTS],
    pub len: u8,
}

impl PitchCurve {
    /// (ノート先頭からのサンプル数, セント) の列から作る(区間は直線)。上限を超えた分は捨てる。
    pub fn from_points(points: &[(f32, f32)]) -> PitchCurve {
        let mut c = PitchCurve::default();
        for (i, p) in points.iter().take(glaux_core::MAX_PITCH_POINTS).enumerate() {
            c.pts[i] = *p;
            c.len = (i + 1) as u8;
        }
        c
    }

    /// (サンプル数, セント, 曲がり方) の列から作る。
    pub fn from_shaped(points: &[(f32, f32, glaux_core::CurveShape)]) -> PitchCurve {
        let mut c = PitchCurve::default();
        for (i, p) in points.iter().take(glaux_core::MAX_PITCH_POINTS).enumerate() {
            c.pts[i] = (p.0, p.1);
            c.shapes[i] = p.2.code();
            c.len = (i + 1) as u8;
        }
        c
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// `age`(サンプル)でのセント値。区間は点の曲がり方で補間、両端は保持。
    pub fn cents_at(&self, age: f32) -> f32 {
        let n = self.len as usize;
        if n == 0 {
            return 0.0;
        }
        let pts = &self.pts[..n];
        if age <= pts[0].0 {
            return pts[0].1;
        }
        for i in 0..n - 1 {
            let (t0, c0) = pts[i];
            let (t1, c1) = pts[i + 1];
            if age < t1 {
                let span = (t1 - t0).max(1.0);
                let x = glaux_core::CurveShape::from_code(self.shapes[i]).apply((age - t0) / span);
                return c0 + (c1 - c0) * x;
            }
        }
        pts[n - 1].1
    }
}

/// ノートのビブラート(サンプル単位に換算済み)。深さ 0 なら無効。
/// 位相はノートの頭で 0。速さは頭の `rate` から終わりの `rate_end` へ直線で変わる
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VibratoSpec {
    /// 深さ(セント、片側)
    pub cents: f32,
    /// 速さ(Hz)の頭と終わり
    pub rate: f32,
    pub rate_end: f32,
    /// 揺れ始めるまで・全深度になるまで・終わりで消す長さ(サンプル)
    pub delay: f32,
    pub fade_in: f32,
    pub fade_out: f32,
    /// ノートの長さ(サンプル)
    pub len: f32,
}

impl VibratoSpec {
    pub fn is_active(&self) -> bool {
        self.cents != 0.0
    }

    /// `age`(サンプル)でのセント値
    pub fn cents_at(&self, age: f32, sample_rate: f32) -> f32 {
        if self.cents == 0.0 || age < self.delay {
            return 0.0;
        }
        let t = age / sample_rate;
        let len_s = (self.len / sample_rate).max(1e-3);
        // 速さを直線で変えたときの位相(速さの積分)
        let phase = self.rate * t + (self.rate_end - self.rate) * t * t / (2.0 * len_s);
        let mut env = if self.fade_in > 0.0 {
            ((age - self.delay) / self.fade_in).clamp(0.0, 1.0)
        } else {
            1.0
        };
        if self.fade_out > 0.0 && self.len > 0.0 {
            env *= ((self.len - age) / self.fade_out).clamp(0.0, 1.0);
        }
        self.cents * env * (std::f32::consts::TAU * phase).sin()
    }
}

/// ノートの音量・明るさの曲線(サンプル位置に換算済み)を、ボイスの出口にかける。
/// 音量は dB の曲線、明るさは −1〜1(負は 1 次の低域通過で暗く、正は 1.5kHz より上を足して明るく)。
/// どの音源にも同じように効く。状態は低域通過の 1 つだけ(固定長。オーディオスレッドで確保しない)
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NoteShape {
    /// 音量(dB)
    pub volume: PitchCurve,
    /// 明るさ(−1〜1)
    pub bright: PitchCurve,
    lp: f32,
}

impl NoteShape {
    pub fn new(volume: PitchCurve, bright: PitchCurve) -> NoteShape {
        NoteShape {
            volume,
            bright,
            lp: 0.0,
        }
    }

    pub fn is_active(&self) -> bool {
        !self.volume.is_empty() || !self.bright.is_empty()
    }

    /// `age`(サンプル)での音量の倍率
    pub fn gain_at(&self, age: f32) -> f32 {
        if self.volume.is_empty() {
            1.0
        } else {
            (self.volume.cents_at(age) * (std::f32::consts::LN_10 / 20.0)).exp()
        }
    }

    /// `age` での明るさ(−1〜1)
    pub fn brightness_at(&self, age: f32) -> f32 {
        if self.bright.is_empty() {
            0.0
        } else {
            self.bright.cents_at(age).clamp(-1.0, 1.0)
        }
    }

    /// 1 サンプルを通す
    pub fn process(&mut self, x: f32, age: f32, sample_rate: f32) -> f32 {
        let mut y = x;
        if !self.bright.is_empty() {
            let b = self.brightness_at(age);
            let tau = std::f32::consts::TAU;
            if b < 0.0 {
                // 18kHz(b = 0)から 280Hz(b = −1)まで
                let fc = 18_000.0 * (b * 6.0).exp2();
                let a = 1.0 - (-tau * fc / sample_rate).exp();
                self.lp += a * (y - self.lp);
                y = self.lp;
            } else {
                let a = 1.0 - (-tau * 1500.0 / sample_rate).exp();
                self.lp += a * (y - self.lp);
                y += b * 1.5 * (y - self.lp);
            }
        }
        y * self.gain_at(age)
    }
}

/// 奏法による音程の変化(セント)を、ノート先頭からの経過 `age`(サンプル)で求める。
/// 内蔵音源の [`PitchExpr`] と同じ形(ビブラート: 5.5Hz・±30 セント、0.12 秒後から 0.25 秒で全深度 /
/// ベンド: 全音下から約 0.22 秒で到達)。CLAP 音源へ 1 音ごとの音程変化として送るのに使う。
/// ビブラートの位相はノート先頭で 0。効果の無い奏法は 0。
pub fn articulation_cents(a: Articulation, age: f32, sample_rate: f32) -> f32 {
    match a {
        Articulation::Vibrato => {
            let secs = age / sample_rate;
            let onset = ((secs - 0.12) / 0.25).clamp(0.0, 1.0);
            30.0 * onset * (std::f32::consts::TAU * 5.5 * secs).sin()
        }
        Articulation::Bend => {
            let t = (age / (0.22 * sample_rate)).min(1.0);
            let ease = t * (2.0 - t);
            // 周波数比で線形に補間しているのと同じ形(内蔵音源と揃える)
            let start = (2.0_f32).powf(-2.0 / 12.0);
            let ratio = start + (1.0 - start) * ease;
            1200.0 * ratio.log2()
        }
        _ => 0.0,
    }
}

/// 奏法が音程を動かすか(CLAP 音源へ音程の変化を送る必要があるか)。
pub fn articulation_moves_pitch(a: Articulation) -> bool {
    matches!(a, Articulation::Vibrato | Articulation::Bend)
}

/// ピッチの時間変化。1 サンプルごとに `next_ratio` で周波数比を得る。
#[derive(Clone, Copy, Debug)]
pub(crate) struct PitchExpr {
    /// ビブラートの深さ(セント)。0 なら無効
    vib_cents: f32,
    /// ビブラート LFO の 1 サンプルあたりの位相増分
    vib_inc: f32,
    /// ベンドの開始周波数比。1.0 なら無効
    bend_start: f32,
    /// ベンドが目標(1.0)に到達するまでのサンプル数
    bend_samples: f32,
    phase: f32,
    age: f32,
    /// ノートに描かれた連続ピッチカーブ(空なら無効)
    curve: PitchCurve,
    /// ノートのビブラートの引数(深さ 0 なら無効。あれば奏法のビブラートの代わり)
    vib: VibratoSpec,
}

const INERT: PitchExpr = PitchExpr {
    vib_cents: 0.0,
    vib_inc: 0.0,
    bend_start: 1.0,
    bend_samples: 1.0,
    phase: 0.0,
    age: 0.0,
    curve: PitchCurve {
        pts: [(0.0, 0.0); glaux_core::MAX_PITCH_POINTS],
        shapes: [0; glaux_core::MAX_PITCH_POINTS],
        len: 0,
    },
    vib: VibratoSpec {
        cents: 0.0,
        rate: 0.0,
        rate_end: 0.0,
        delay: 0.0,
        fade_in: 0.0,
        fade_out: 0.0,
        len: 0.0,
    },
};

impl PitchExpr {
    pub(crate) fn new(a: Articulation, sample_rate: f32) -> PitchExpr {
        match a {
            // 5.5Hz・±30 セント。出だしは揺らさず後半に深くなる(歌・ギターの慣例)
            Articulation::Vibrato => PitchExpr {
                vib_cents: 30.0,
                vib_inc: 5.5 / sample_rate,
                ..INERT
            },
            // チョーキング: 全音(200 セント)下から約 0.22 秒で書かれた音程へ滑り上がる
            Articulation::Bend => PitchExpr {
                bend_start: (2.0_f32).powf(-2.0 / 12.0),
                bend_samples: 0.22 * sample_rate,
                ..INERT
            },
            _ => INERT,
        }
    }

    /// ピッチカーブを付ける(奏法の効果と掛け合わせ)。
    pub(crate) fn set_curve(&mut self, curve: &PitchCurve) {
        self.curve = *curve;
    }

    /// ノートのビブラートを付ける(奏法のビブラートの代わりになる)。
    pub(crate) fn set_vibrato(&mut self, v: &VibratoSpec) {
        if v.is_active() {
            self.vib = *v;
            self.vib_cents = 0.0;
        }
    }

    /// 効果を持つか(持たないボイスは呼び出しを省ける)。
    pub(crate) fn is_active(&self) -> bool {
        self.vib_cents != 0.0
            || self.bend_start != 1.0
            || !self.curve.is_empty()
            || self.vib.is_active()
    }

    /// 現在の周波数比を返し、時間を 1 サンプル進める。
    pub(crate) fn next_ratio(&mut self, sample_rate: f32) -> f32 {
        self.age += 1.0;
        let mut ratio = 1.0;
        if self.bend_start != 1.0 {
            let t = (self.age / self.bend_samples).min(1.0);
            let ease = t * (2.0 - t); // 減速しながら到達(指の押し込みの感じ)
            ratio *= self.bend_start + (1.0 - self.bend_start) * ease;
        }
        if self.vib_cents != 0.0 {
            self.phase += self.vib_inc;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
            // 0.12 秒までは揺らさず、その後 0.25 秒かけて全深度へ
            let onset = ((self.age / sample_rate - 0.12) / 0.25).clamp(0.0, 1.0);
            let cents = self.vib_cents * onset * (self.phase * std::f32::consts::TAU).sin();
            // 小さい cents に対する 2^(c/1200) の一次近似(±30 セントで誤差は無視できる)
            ratio *= 1.0 + cents * (std::f32::consts::LN_2 / 1200.0);
        }
        if self.vib.is_active() {
            let cents = self.vib.cents_at(self.age, sample_rate);
            ratio *= (cents * (std::f32::consts::LN_2 / 1200.0)).exp();
        }
        if !self.curve.is_empty() {
            let cents = self.curve.cents_at(self.age);
            if cents != 0.0 {
                ratio *= (cents * (std::f32::consts::LN_2 / 1200.0)).exp();
            }
        }
        ratio
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_interpolates_and_holds_ends() {
        let c = PitchCurve::from_points(&[(0.0, -200.0), (100.0, 0.0), (200.0, 100.0)]);
        assert_eq!(c.cents_at(0.0), -200.0);
        assert!((c.cents_at(50.0) - -100.0).abs() < 1e-3);
        assert!((c.cents_at(150.0) - 50.0).abs() < 1e-3);
        assert_eq!(c.cents_at(999.0), 100.0, "最後の点より後は保持");
        let late = PitchCurve::from_points(&[(100.0, 50.0)]);
        assert_eq!(late.cents_at(0.0), 50.0, "最初の点より前は保持");
    }

    #[test]
    fn shaped_curves_and_vibrato_params() {
        use glaux_core::CurveShape;
        let c = PitchCurve::from_shaped(&[
            (0.0, -200.0, CurveShape::EaseOut),
            (100.0, 0.0, CurveShape::Hold),
            (200.0, 100.0, CurveShape::Linear),
        ]);
        // ease_out は前半で多く進む、hold は次の点まで保持
        assert!(c.cents_at(50.0) > -100.0 + 20.0);
        assert_eq!(c.cents_at(150.0), 0.0);
        assert_eq!(c.cents_at(200.0), 100.0);
        let sr = 48_000.0;
        let v = VibratoSpec {
            cents: 60.0,
            rate: 5.0,
            rate_end: 5.0,
            delay: 0.25 * sr,
            fade_in: 0.2 * sr,
            fade_out: 0.1 * sr,
            len: 2.0 * sr,
        };
        assert_eq!(v.cents_at(0.1 * sr, sr), 0.0, "始まるまでは揺らさない");
        let peak = (0..(2.0 * sr) as u32)
            .map(|i| v.cents_at(i as f32, sr).abs())
            .fold(0.0f32, f32::max);
        assert!(peak > 55.0 && peak <= 60.0, "{peak}");
        assert!(v.cents_at(1.999 * sr, sr).abs() < 2.0, "終わりで消える");
        // ボイスでは奏法のビブラートの代わりになる
        let mut e = PitchExpr::new(Articulation::Vibrato, sr);
        e.set_vibrato(&v);
        for i in 1..(sr as u32) {
            let r = e.next_ratio(sr);
            if i % 1009 == 0 {
                let c = 1200.0 * r.log2();
                assert!((c - v.cents_at(i as f32, sr)).abs() < 0.5, "{i}");
            }
        }
    }

    #[test]
    fn note_shape_scales_and_darkens() {
        let sr = 48_000.0;
        // 0 dB → −20 dB に下がる音量
        let vol = PitchCurve::from_points(&[(0.0, 0.0), (1000.0, -20.0)]);
        let mut s = NoteShape::new(vol, PitchCurve::default());
        assert!((s.process(1.0, 0.0, sr) - 1.0).abs() < 1e-4);
        assert!((s.process(1.0, 1000.0, sr) - 0.1).abs() < 1e-3);
        // 暗くすると高い周波数(交互の ±1)が弱まる、明るくすると強まる
        let energy = |b: f32| {
            let mut n = NoteShape::new(PitchCurve::default(), PitchCurve::from_points(&[(0.0, b)]));
            let mut e = 0.0;
            for i in 0..4800 {
                let x = if i % 2 == 0 { 1.0 } else { -1.0 };
                let y = n.process(x, i as f32, sr);
                if i > 100 {
                    e += y * y;
                }
            }
            e
        };
        assert!(energy(-0.8) < energy(0.0) * 0.1);
        assert!(energy(0.8) > energy(0.0));
        assert!(!NoteShape::default().is_active());
    }

    #[test]
    fn articulation_cents_matches_the_voice_expression() {
        let sr = 48_000.0;
        for a in [Articulation::Vibrato, Articulation::Bend] {
            let mut e = PitchExpr::new(a, sr);
            for i in 1..40_000u32 {
                let r = e.next_ratio(sr);
                if i % 997 == 0 {
                    let c = articulation_cents(a, i as f32, sr);
                    let from_ratio = 1200.0 * r.log2();
                    assert!(
                        (c - from_ratio).abs() < 1.0,
                        "{a:?} {i}: {c} vs {from_ratio}"
                    );
                }
            }
        }
        assert_eq!(articulation_cents(Articulation::Staccato, 1000.0, sr), 0.0);
        assert!((articulation_cents(Articulation::Bend, 0.0, sr) + 200.0).abs() < 0.1);
    }

    #[test]
    fn curve_changes_frequency_ratio() {
        let mut e = PitchExpr::new(Articulation::Normal, 48_000.0);
        assert!(!e.is_active());
        e.set_curve(&PitchCurve::from_points(&[(0.0, -1200.0), (480.0, 0.0)]));
        assert!(e.is_active());
        let first = e.next_ratio(48_000.0);
        assert!((first - 0.5).abs() < 0.01, "1 オクターブ下から: {first}");
        for _ in 0..600 {
            e.next_ratio(48_000.0);
        }
        let last = e.next_ratio(48_000.0);
        assert!((last - 1.0).abs() < 1e-4, "書かれた音程へ到達: {last}");
    }
}
