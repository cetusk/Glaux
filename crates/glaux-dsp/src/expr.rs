//! ノート単位のピッチ表現(ビブラート / チョーキング)。
//!
//! `Articulation::Vibrato` / `Articulation::Bend` と、ノートに描かれた連続
//! ピッチカーブ([`PitchCurve`])をボイス内の周波数比の時間変化として実装する。
//! subtractive / pluck / sampler / sf2 が共用する。

use glaux_core::Articulation;

/// ボイスが持つ固定長のピッチカーブ(サンプル位置, セント)。
/// `glaux_core::Note::pitch_curve` をエンジンがサンプル位置に換算したもの。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PitchCurve {
    pub pts: [(f32, f32); glaux_core::MAX_PITCH_POINTS],
    pub len: u8,
}

impl PitchCurve {
    /// (ノート先頭からのサンプル数, セント) の列から作る。上限を超えた分は捨てる。
    pub fn from_points(points: &[(f32, f32)]) -> PitchCurve {
        let mut c = PitchCurve::default();
        for (i, p) in points.iter().take(glaux_core::MAX_PITCH_POINTS).enumerate() {
            c.pts[i] = *p;
            c.len = (i + 1) as u8;
        }
        c
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// `age`(サンプル)でのセント値。区分線形、両端は保持。
    pub fn cents_at(&self, age: f32) -> f32 {
        let n = self.len as usize;
        if n == 0 {
            return 0.0;
        }
        let pts = &self.pts[..n];
        if age <= pts[0].0 {
            return pts[0].1;
        }
        for w in pts.windows(2) {
            let (t0, c0) = w[0];
            let (t1, c1) = w[1];
            if age < t1 {
                let span = (t1 - t0).max(1.0);
                return c0 + (c1 - c0) * ((age - t0) / span);
            }
        }
        pts[n - 1].1
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
        len: 0,
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

    /// 効果を持つか(持たないボイスは呼び出しを省ける)。
    pub(crate) fn is_active(&self) -> bool {
        self.vib_cents != 0.0 || self.bend_start != 1.0 || !self.curve.is_empty()
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
