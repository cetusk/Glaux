//! 撥弦の物理モデル `pluck`(Karplus-Strong 拡張)。
//!
//! ディレイライン(= 弦)にノイズバースト(= ピッキング)を注入し、
//! ループごとにローパスで減衰させる。減算シンセでは出ない
//! 「弾いた瞬間だけ倍音が豊かで、そこから不均一に減衰する」弦の挙動が物理として出る。
//! ギター(+distortion でエレキ)・ベース・ハープ系が守備範囲。
//!
//! RT セーフ: 固定長バッファ(`Copy`)のみでアロケーションなし。

use glaux_core::Articulation;

/// ディレイラインの最大長。48kHz で約 23Hz(MIDI 16 相当)まで対応
const DELAY_MAX: usize = 2048;

/// 焼き込み済みパラメータ(1 トラック分)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PluckParams {
    /// 弦の鳴りの長さ(秒)
    pub decay: f32,
    /// 弦の明るさ 0..=0.95(高域がどれだけ長く残るか)
    pub brightness: f32,
    /// ピッキングの硬さ 0..=1
    pub pick: f32,
    /// リニアゲイン(dB から変換済み)
    pub gain: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct PluckVoice {
    /// 弦(リングバッファ)
    buf: [f32; DELAY_MAX],
    write: usize,
    /// 周期(サンプル、小数)
    period: f32,
    /// 1 周回あたりの減衰(奏法込みで焼き込み済み)
    loop_gain: f32,
    /// 実効明るさ(奏法込み)
    bright: f32,
    amp: f32,
    /// ピッチ表現(ビブラート / チョーキング)
    expr: crate::expr::PitchExpr,
    /// note_off 後のフェード(押さえて止める)
    release_env: f32,
    released: bool,
    /// 出力エネルギーの追跡(finished 判定)
    env: f32,
    sample_rate: f32,
}

impl PluckVoice {
    pub fn start(
        p: &PluckParams,
        freq: f32,
        vel: f32,
        articulation: Articulation,
        sample_rate: f32,
    ) -> Self {
        let sr = sample_rate;
        let period = (sr / freq.max(1.0)).clamp(2.0, (DELAY_MAX - 3) as f32);

        // 奏法: パームミュートは「掌で弦を押さえる」= 減衰を強く・暗く・ピックも柔らかめ
        let (decay_mul, bright_mul, pick_mul, amp_mul) = match articulation {
            Articulation::PalmMute => (0.12, 0.4, 0.6, 1.0),
            Articulation::Accent => (1.0, 1.15, 1.2, 1.4),
            _ => (1.0, 1.0, 1.0, 1.0),
        };
        let decay = (p.decay * decay_mul).max(0.02);
        // 「decay 秒後に -60dB」となる周回あたりの利得
        let loop_gain = (10.0_f32).powf(-3.0 * period / (sr * decay));
        let bright = (p.brightness * bright_mul).clamp(0.0, 0.95);
        let pick = (p.pick * pick_mul).clamp(0.0, 1.0);

        // 励起: ピッキングの硬さでフィルタしたノイズバーストを 1 周期分注入
        let mut buf = [0.0f32; DELAY_MAX];
        let n = (period.ceil() as usize).min(DELAY_MAX);
        let mut rng: u32 = (freq.to_bits() | 1).wrapping_mul(0x9e37_79b9);
        let coef = 0.08 + 0.92 * pick * pick; // 硬い = 生ノイズに近い(明るい)
        let mut lp = 0.0f32;
        let mut sum = 0.0f32;
        for slot in buf.iter_mut().take(n) {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            let noise = (rng as f32 / u32::MAX as f32) * 2.0 - 1.0;
            lp += coef * (noise - lp);
            *slot = lp;
            sum += lp;
        }
        // DC 成分を除く(残すと「ボッ」という低域の塊が出る)
        let mean = sum / n.max(1) as f32;
        for slot in buf.iter_mut().take(n) {
            *slot -= mean;
        }

        PluckVoice {
            buf,
            write: n % DELAY_MAX,
            period,
            loop_gain,
            bright,
            amp: vel * amp_mul,
            expr: crate::expr::PitchExpr::new(articulation, sample_rate),
            release_env: 1.0,
            released: false,
            env: 0.5,
            sample_rate: sr,
        }
    }

    pub fn note_off(&mut self) {
        self.released = true;
    }

    pub fn finished(&self) -> bool {
        self.released && self.env < 1e-4
    }

    fn read_at(&self, delay: f32) -> f32 {
        let pos = self.write as f32 - delay + DELAY_MAX as f32;
        let i0 = pos as usize;
        let frac = pos - i0 as f32;
        let a = self.buf[i0 % DELAY_MAX];
        let b = self.buf[(i0 + 1) % DELAY_MAX];
        a + (b - a) * frac
    }

    pub fn next(&mut self, p: &PluckParams) -> f32 {
        // ピッチ表現(ビブラート / チョーキング): 実効周期を比で割る
        let period = if self.expr.is_active() {
            (self.period / self.expr.next_ratio(self.sample_rate))
                .clamp(2.0, (DELAY_MAX - 3) as f32)
        } else {
            self.period
        };
        // 弦の 1 サンプル: 周期前の 2 点の平均(ローパス)と生値を明るさでブレンド
        let s0 = self.read_at(period);
        let s1 = self.read_at(period + 1.0);
        let avg = 0.5 * (s0 + s1);
        let filtered = avg + self.bright * (s0 - avg);
        let new = filtered * self.loop_gain;
        self.buf[self.write] = new;
        self.write = (self.write + 1) % DELAY_MAX;

        // note_off 後は指で止めるように短く(約 60ms)フェード
        if self.released {
            self.release_env *= 1.0 - 1.0 / (0.06 * self.sample_rate);
        }

        let out = new * self.amp * self.release_env * p.gain;
        self.env = out.abs().max(self.env * 0.999);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_params() -> PluckParams {
        PluckParams {
            decay: 2.5,
            brightness: 0.5,
            pick: 0.6,
            gain: 1.0,
        }
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32).sqrt()
    }

    fn render(p: &PluckParams, art: Articulation, samples: usize) -> Vec<f32> {
        let mut v = PluckVoice::start(p, 220.0, 1.0, art, 48_000.0);
        (0..samples).map(|_| v.next(p)).collect()
    }

    #[test]
    fn plucks_then_decays_and_stops_after_note_off() {
        let p = default_params();
        let mut v = PluckVoice::start(&p, 220.0, 1.0, Articulation::Normal, 48_000.0);
        let head: Vec<f32> = (0..4800).map(|_| v.next(&p)).collect();
        assert!(rms(&head) > 0.02, "弾いた直後は鳴るはず: {}", rms(&head));

        v.note_off();
        let mut stopped = false;
        for _ in 0..48_000 {
            v.next(&p);
            if v.finished() {
                stopped = true;
                break;
            }
        }
        assert!(stopped, "note_off 後は止まるはず");
    }

    #[test]
    fn pitch_is_roughly_correct() {
        // 倍音が豊かなのでゼロクロスでは測れない。自己相関のピーク位置 = 周期で確認
        let p = default_params();
        let out = render(&p, Articulation::Normal, 48_000);
        let tail = &out[24_000..36_000];
        let expected = 48_000.0 / 220.0; // ≈ 218 サンプル
        let mut best_lag = 0usize;
        let mut best = f32::MIN;
        for lag in 120..400 {
            let c: f32 = (0..8_000).map(|i| tail[i] * tail[i + lag]).sum();
            if c > best {
                best = c;
                best_lag = lag;
            }
        }
        assert!(
            (best_lag as f32 - expected).abs() < expected * 0.08,
            "音程がずれすぎ: 周期 {best_lag} サンプル(期待 {expected})"
        );
    }

    #[test]
    fn palm_mute_is_short_and_dark() {
        let p = default_params();
        let normal = render(&p, Articulation::Normal, 24_000);
        let muted = render(&p, Articulation::PalmMute, 24_000);

        // 0.25 秒以降の残エネルギー: ミュートはほぼ消えている
        let late = |v: &Vec<f32>| rms(&v[12_000..]);
        assert!(
            late(&muted) < late(&normal) * 0.25,
            "パームミュートは早く消えるはず: muted={} normal={}",
            late(&muted),
            late(&normal)
        );
        // 頭のアタックは鳴る
        assert!(rms(&muted[..2_400]) > 0.02);
    }

    #[test]
    fn bend_slides_up_to_written_pitch() {
        // チョーキング: 序盤は全音下(周期が長い)、0.22 秒以降は書かれた音程
        let p = default_params();
        let out = render(&p, Articulation::Bend, 48_000);
        let peak_lag = |window: &[f32]| {
            let mut best_lag = 0usize;
            let mut best = f32::MIN;
            for lag in 120..400 {
                let c: f32 = (0..3_000).map(|i| window[i] * window[i + lag]).sum();
                if c > best {
                    best = c;
                    best_lag = lag;
                }
            }
            best_lag
        };
        let early = peak_lag(&out[0..4_000]); // 0〜83ms(まだ下)
        let late = peak_lag(&out[24_000..30_000]); // 0.5 秒以降(到達済み)
        let expected = 48_000.0 / 220.0;
        assert!(
            (late as f32 - expected).abs() < expected * 0.08,
            "到達後は書かれた音程のはず: {late}"
        );
        assert!(
            early as f32 > late as f32 * 1.05,
            "序盤は低い(周期が長い)はず: early={early} late={late}"
        );
    }

    #[test]
    fn brightness_keeps_highs_longer() {
        let hf = |b: f32| {
            let p = PluckParams {
                brightness: b,
                ..default_params()
            };
            let out = render(&p, Articulation::Normal, 24_000);
            out[12_000..]
                .windows(2)
                .map(|w| (w[1] - w[0]).powi(2))
                .sum::<f32>()
        };
        assert!(hf(0.9) > hf(0.1) * 2.0, "明るいほど高域が残るはず");
    }
}
