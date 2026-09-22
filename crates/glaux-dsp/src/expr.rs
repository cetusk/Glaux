//! ノート単位のピッチ表現(ビブラート / チョーキング)。
//!
//! `Articulation::Vibrato` / `Articulation::Bend` をボイス内の周波数比の
//! 時間変化として実装する。subtractive と pluck の両方が共用する。
//! 連続ピッチカーブ(自由な描画)の本格版はサンプラー検討時に別途設計する。

use glaux_core::Articulation;

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
}

const INERT: PitchExpr = PitchExpr {
    vib_cents: 0.0,
    vib_inc: 0.0,
    bend_start: 1.0,
    bend_samples: 1.0,
    phase: 0.0,
    age: 0.0,
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

    /// 効果を持つか(持たないボイスは呼び出しを省ける)。
    pub(crate) fn is_active(&self) -> bool {
        self.vib_cents != 0.0 || self.bend_start != 1.0
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
        ratio
    }
}
