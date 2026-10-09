//! 加算合成の音源 `additive`。
//!
//! 正弦波(部分音)を 1 本ずつ足して音を作る。倍音の並びを直接いじれるので、減算(フィルタで削る)や
//! FM(揺らして増やす)とは違う、澄んだ・ガラスのような・オルガン・声のような音が作れる。
//!
//! - 部分音の数 partials(8〜64。多いほど明るい音まで作れて重い)
//! - 明るさの傾き tilt(高い部分音ほど弱く。-6dB/oct でノコギリ波の並び)
//! - 奇数・偶数の釣り合い odd_even(奇数だけ = 矩形波・クラリネット、偶数を強く = 中空でない明るさ)
//! - フォルマント(鳴らす高さに関係なく決まった周波数のあたりを持ち上げる山。声・管楽器らしさ)
//! - 高い部分音ほど速く消える damping(撥弦・打鍵の、明るい頭から丸い尾への変化)
//! - 部分音の伸び inharmonic(ピアノ・ベルのように上の部分音が少しずつ高い)
//! - 揺らぎ wobble(部分音ごとのゆっくりした音程・音量の漂い。アナログの生きた感じ)
//! - 全体の ADSR
//!
//! 部分音の周波数・音量は 32 サンプルごとに求め、間は直線でつなぐ(1 サンプルごとの計算は位相を進めて
//! 正弦を足すだけ)。ナイキストに近い部分音は弱めて、超えたものは鳴らさない(折り返さない)。
//! 揺らぎの乱数は音ごとの種から作る(同じ曲は何度描き出しても同じ音)。
//! RT セーフ: 値型のみでアロケーションなし。

use glaux_core::Articulation;

/// 部分音の数の上限
pub const MAX_PARTIALS: usize = 64;
/// 部分音の周波数・音量を求め直す間隔(サンプル)
const CTRL: u32 = 32;
/// 手で描いた山の点・山の数の上限
pub const MAX_EDIT_POINTS: usize = 64;
pub const MAX_EDIT_GROUPS: usize = 32;

/// 手で描いた山(部分音の強さへの上乗せ。dB)。続けて描いた点のまとまりが 1 つの山。
/// 周波数に付ける山(どの高さで弾いても同じ周波数が強い。声の母音・胴の響き)と、
/// 何番目に付ける山(どの高さでも同じ番目の部分音が強い)のどちらか 1 つ
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartialEdits {
    /// true = 周波数に付ける山(点の位置は log2(Hz))、false = 何番目に付ける山(点の位置は 0 からの番目)
    pub hz: bool,
    /// 点(位置, dB)。山ごとに続けて並べ、山の中は位置の順
    pub pts: [(f32, f32); MAX_EDIT_POINTS],
    pub n: u8,
    /// 山(pts の中の始まり, 数)
    pub groups: [(u8, u8); MAX_EDIT_GROUPS],
    pub ng: u8,
    /// 中身の印(部分音の形を求め直すかどうかに使う)
    pub sig: u32,
}

impl Default for PartialEdits {
    fn default() -> Self {
        PartialEdits {
            hz: true,
            pts: [(0.0, 0.0); MAX_EDIT_POINTS],
            n: 0,
            groups: [(0, 0); MAX_EDIT_GROUPS],
            ng: 0,
            sig: 0,
        }
    }
}

impl PartialEdits {
    /// 「hz|1800:6,2000:9|4000:-6」(周波数の山 2 つ)・「idx|7:12,8:6」(何番目の山 1 つ)を読む。
    /// 先頭が形の種類、「|」で山を区切り、山の中は「位置:dB」を「,」で並べる。読めない点は飛ばす
    pub fn parse(s: &str) -> PartialEdits {
        let mut e = PartialEdits::default();
        let mut parts = s.split('|');
        match parts.next().map(str::trim) {
            Some("idx") => e.hz = false,
            Some("hz") => e.hz = true,
            _ => return e,
        }
        for g in parts {
            if e.ng as usize >= MAX_EDIT_GROUPS {
                break;
            }
            let start = e.n;
            let mut pts: Vec<(f32, f32)> = g
                .split(',')
                .filter_map(|p| {
                    let (a, b) = p.split_once(':')?;
                    let (at, db) = (a.trim().parse::<f32>().ok()?, b.trim().parse::<f32>().ok()?);
                    if !at.is_finite() || !db.is_finite() {
                        return None;
                    }
                    let at = if e.hz {
                        if !(20.0..=20_000.0).contains(&at) {
                            return None;
                        }
                        at.log2()
                    } else {
                        if !(0.0..MAX_PARTIALS as f32).contains(&at) {
                            return None;
                        }
                        at.round()
                    };
                    Some((at, db.clamp(-60.0, 40.0)))
                })
                .collect();
            pts.sort_by(|a, b| a.0.total_cmp(&b.0));
            for p in pts {
                if e.n as usize >= MAX_EDIT_POINTS {
                    break;
                }
                e.pts[e.n as usize] = p;
                e.n += 1;
            }
            if e.n > start {
                e.groups[e.ng as usize] = (start, e.n - start);
                e.ng += 1;
            }
        }
        // 印: 中身の FNV-1a
        let mut h: u32 = 0x811c_9dc5;
        for b in s.bytes() {
            h ^= b as u32;
            h = h.wrapping_mul(0x0100_0193);
        }
        e.sig = h;
        e
    }

    /// k 番目(0 から)・周波数 `f` の部分音への上乗せ(dB)。山ごとに足す(山と山の間は持ち上げない)
    pub fn offset_db(&self, k: usize, f: f32) -> f32 {
        if self.n == 0 {
            return 0.0;
        }
        if !self.hz {
            return self.pts[..self.n as usize]
                .iter()
                .filter(|p| p.0 as usize == k)
                .map(|p| p.1)
                .sum();
        }
        // 周波数の山: 山の中は周波数(対数)で直線につなぎ、外側は 2 半音かけて 0 へ
        let lf = f.max(1.0).log2();
        let taper = 2.0 / 12.0;
        let mut sum = 0.0;
        for &(st, len) in &self.groups[..self.ng as usize] {
            let pts = &self.pts[st as usize..(st + len) as usize];
            let (first, last) = (pts[0], pts[pts.len() - 1]);
            sum += if lf <= first.0 {
                let t = (first.0 - lf) / taper;
                if t >= 1.0 {
                    0.0
                } else {
                    first.1 * 0.5 * (1.0 + (std::f32::consts::PI * t).cos())
                }
            } else if lf >= last.0 {
                let t = (lf - last.0) / taper;
                if t >= 1.0 {
                    0.0
                } else {
                    last.1 * 0.5 * (1.0 + (std::f32::consts::PI * t).cos())
                }
            } else {
                let i = pts.windows(2).position(|w| lf <= w[1].0).unwrap_or(0);
                let (a, b) = (pts[i], pts[i + 1]);
                a.1 + (b.1 - a.1) * (lf - a.0) / (b.0 - a.0).max(1e-6)
            };
        }
        sum
    }
}

/// 焼き込み済みパラメータ(1 トラック分)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdditiveParams {
    pub partials: u8,
    /// 明るさの傾き(dB/oct。0 = どの部分音も同じ強さ、-6 = 1/k)
    pub tilt: f32,
    /// −1..1(負で偶数を、正で奇数〈基音以外〉を弱める)
    pub odd_even: f32,
    /// フォルマントの中心(Hz)・持ち上げる量(dB)・幅(オクターブ)
    pub formant_hz: f32,
    pub formant_db: f32,
    pub formant_width: f32,
    /// 高い部分音ほど速く消える量 0..=1
    pub damping: f32,
    /// 部分音の伸び 0..=1(ピアノ・ベルの非調和)
    pub inharmonic: f32,
    /// 揺らぎ 0..=1
    pub wobble: f32,
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    /// リニアゲイン(dB から変換済み)
    pub gain: f32,
    /// 手で描いた山
    pub edits: PartialEdits,
}

/// −1..1 を 16 ビットに
#[inline]
fn to_i16(x: f32) -> i16 {
    (x.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    Attack,
    Decay,
    Release,
}

#[derive(Clone, Copy, Debug)]
pub struct AdditiveVoice {
    freq: f32,
    /// 部分音ごとの位相(回転数)・1 サンプルの進み・音量・音量の 1 サンプルの変化
    phase: [f32; MAX_PARTIALS],
    inc: [f32; MAX_PARTIALS],
    amp: [f32; MAX_PARTIALS],
    amp_step: [f32; MAX_PARTIALS],
    /// 揺らぎ(部分音ごとの −1..1 の漂い。音程用・音量用を 1 つの値から作る)
    drift: [f32; MAX_PARTIALS],
    /// 漂いの目標(−1..1 を 16 ビットに詰める。ボイスを小さく保つため)
    drift_to: [i16; MAX_PARTIALS],
    /// 部分音の形(傾き・奇数偶数・フォルマント)と、それを求めたときのつまみ・高さ、音量の補正
    shape: [f32; MAX_PARTIALS],
    shape_key: [u32; 9],
    shape_gain: f32,
    env: f32,
    stage: Stage,
    vel: f32,
    /// 押さえてからの秒数(部分音ごとの減衰)
    age: f32,
    ctrl: u32,
    rng: u32,
    decay_mul: f32,
    pub(crate) expr: crate::expr::PitchExpr,
    sample_rate: f32,
}

impl AdditiveVoice {
    pub fn start(
        p: &AdditiveParams,
        freq: f32,
        vel: f32,
        articulation: Articulation,
        sample_rate: f32,
    ) -> Self {
        Self::start_seeded(p, freq, vel, articulation, sample_rate, freq.to_bits())
    }

    /// 揺らぎの種を渡して鳴らす
    pub fn start_seeded(
        p: &AdditiveParams,
        freq: f32,
        vel: f32,
        articulation: Articulation,
        sample_rate: f32,
        seed: u32,
    ) -> Self {
        let (vel, decay_mul) = match articulation {
            Articulation::Accent => ((vel * 1.3).min(1.0), 1.0),
            Articulation::PalmMute => (vel, 0.25),
            _ => (vel, 1.0),
        };
        let mut v = AdditiveVoice {
            freq,
            phase: [0.0; MAX_PARTIALS],
            inc: [0.0; MAX_PARTIALS],
            amp: [0.0; MAX_PARTIALS],
            amp_step: [0.0; MAX_PARTIALS],
            drift: [0.0; MAX_PARTIALS],
            drift_to: [0; MAX_PARTIALS],
            shape: [0.0; MAX_PARTIALS],
            shape_key: [u32::MAX; 9],
            shape_gain: 1.0,
            env: 0.0,
            stage: Stage::Attack,
            vel,
            age: 0.0,
            ctrl: 0,
            rng: (seed ^ 0x68e3_1da4).wrapping_mul(0x9e37_79b9) | 1,
            decay_mul,
            expr: crate::expr::PitchExpr::new(articulation, sample_rate),
            sample_rate,
        };
        // 初期位相: 揺らぎがあれば乱数(毎回少し違う頭)、無ければ 0(決まった波形)
        for k in 0..MAX_PARTIALS {
            let r = v.rand();
            let d = v.rand();
            if p.wobble > 0.0 {
                v.phase[k] = r * 0.5 + 0.5;
                v.drift[k] = d;
                v.drift_to[k] = to_i16(v.rand());
            }
        }
        // 頭の音量を先に求めておく(最初の 32 サンプルが無音にならないように)
        v.control(p, 1.0);
        v.ctrl = CTRL;
        v
    }

    /// レガートで次の音へ移る: 発音し直さず(エンベロープ・波の位相・フィルタはそのまま)高さだけを変える。
    /// 音程の表現(奏法のビブラートなど)は次の音のものにする
    pub fn glide_to(
        &mut self,
        freq: f32,
        articulation: glaux_core::Articulation,
        sample_rate: f32,
    ) {
        self.freq = freq;
        self.expr = crate::expr::PitchExpr::new(articulation, sample_rate);
    }

    pub fn note_off(&mut self) {
        self.stage = Stage::Release;
    }

    /// レガート: 立ち上がりを飛ばす
    pub fn skip_attack(&mut self, p: &AdditiveParams) {
        self.env = p.sustain.clamp(0.35, 1.0);
        self.stage = Stage::Decay;
    }

    pub fn finished(&self) -> bool {
        self.stage == Stage::Release && self.env < 1e-4
    }

    /// −1..1 の乱数(xorshift32)
    #[inline]
    fn rand(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    /// 部分音の周波数・音量を求め直す(`ratio` は音程表現の倍率)。音量は次の CTRL サンプルで今の値から
    /// 目標へ直線で動く
    fn control(&mut self, p: &AdditiveParams, ratio: f32) {
        let sr = self.sample_rate;
        let dt = CTRL as f32 / sr;
        let f0 = self.freq * ratio;
        let n = (p.partials as usize).clamp(1, MAX_PARTIALS);
        let nyq = sr * 0.45;
        // 傾き: k 番目は k^(tilt/6.02)(−6dB/oct で 1/k)
        let slope = p.tilt / 6.0206;
        // 伸び(ピアノの弦の式 f_k = k f0 √(1 + B k²))
        let b = p.inharmonic * p.inharmonic * 0.002;
        let wob = p.wobble;
        let c = (dt * 2.0).min(1.0);
        // 部分音の形(傾き・奇数偶数・フォルマント)は、つまみか高さが変わったときだけ求め直す
        // (累乗・対数・指数を 32 サンプルごとに 64 本ぶん求めない)
        let key = [
            p.tilt.to_bits(),
            p.odd_even.to_bits(),
            p.formant_hz.to_bits(),
            p.formant_db.to_bits(),
            p.formant_width.to_bits(),
            p.inharmonic.to_bits(),
            f0.to_bits(),
            n as u32,
            p.edits.sig,
        ];
        if key != self.shape_key {
            self.shape_key = key;
            let mut norm = 0.0f32;
            for k in 0..n {
                let kf = (k + 1) as f32;
                let fk = f0 * kf * (1.0 + b * kf * kf).sqrt();
                let mut a = kf.powf(slope);
                // 奇数・偶数(基音は残す)
                if k > 0 {
                    let even = (k + 1) % 2 == 0;
                    if even && p.odd_even < 0.0 {
                        a *= 1.0 + p.odd_even;
                    } else if !even && p.odd_even > 0.0 {
                        a *= 1.0 - p.odd_even;
                    }
                }
                // フォルマント(対数周波数の上のガウスの山)
                if p.formant_db > 0.0 {
                    let oct = (fk / p.formant_hz.max(20.0)).log2() / p.formant_width.max(0.05);
                    a *= 10f32.powf(p.formant_db / 20.0 * (-0.5 * oct * oct).exp());
                }
                // 部分音の数・傾きを変えても音量がそろうように(2 乗和で割る)。手で描いた山は、
                // 描いた分だけ大きく・小さく聞こえるように、そろえた後に掛ける
                norm += a * a;
                if p.edits.n > 0 {
                    a *= 10f32.powf(p.edits.offset_db(k, fk) / 20.0);
                }
                self.shape[k] = a;
            }
            self.shape_gain = 0.5 / norm.max(1e-9).sqrt();
        }
        let g = self.shape_gain;
        for k in 0..n {
            let kf = (k + 1) as f32;
            // 揺らぎ: 目標へ寄り、着いたら次の目標
            if wob > 0.0 {
                let to = self.drift_to[k] as f32 / i16::MAX as f32;
                self.drift[k] += (to - self.drift[k]) * c;
                if (self.drift[k] - to).abs() < 0.05 {
                    self.drift_to[k] = to_i16(self.rand());
                }
            }
            let cents = wob * 8.0 * self.drift[k];
            let fk = f0 * kf * (1.0 + b * kf * kf).sqrt() * (1.0 + cents * 0.000_577_8);
            // ナイキストの手前 10% で弱め、超えたら鳴らさない
            let edge = ((nyq - fk) / (nyq * 0.1)).clamp(0.0, 1.0);
            if edge <= 0.0 {
                // 鳴らさない部分音は位相も止める(進みが 1 周を超えて位相が膨らまないように)
                self.inc[k] = 0.0;
                self.amp_step[k] = -self.amp[k] / CTRL as f32;
                continue;
            }
            self.inc[k] = fk / sr;
            let mut a = self.shape[k];
            // 高い部分音ほど速く消える(基音は ADSR だけ。k 番目は k 倍速く)
            if p.damping > 0.0 && k > 0 {
                a *= (-self.age * p.damping * kf / (1.5 * self.decay_mul)).exp();
            }
            // 揺らぎ: 音量 ±20%
            if wob > 0.0 {
                a *= 1.0 + 0.2 * wob * self.drift[(k * 7 + 3) % n];
            }
            // 音量は次の CTRL サンプルで今の値から目標へ直線で動く
            self.amp_step[k] = (a * edge * g - self.amp[k]) / CTRL as f32;
        }
        // 使わない部分音(数を減らしたとき)は 0 へ
        for (st, a) in self.amp_step[n..].iter_mut().zip(&self.amp[n..]) {
            *st = -a / CTRL as f32;
        }
        self.age += dt;
    }

    pub fn next(&mut self, p: &AdditiveParams) -> f32 {
        let sr = self.sample_rate;
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
                self.env += (p.sustain - self.env) * coef;
            }
            Stage::Release => {
                let coef = (6.9 / (p.release.max(0.005) * sr)).min(1.0);
                self.env -= self.env * coef;
            }
        }
        let ratio = if self.expr.is_active() {
            self.expr.next_ratio(sr)
        } else {
            1.0
        };
        if self.ctrl == 0 {
            self.control(p, ratio);
            self.ctrl = CTRL;
        }
        self.ctrl -= 1;
        let n = (p.partials as usize).clamp(1, MAX_PARTIALS);
        let mut s = 0.0f32;
        // 部分音のループは分岐なしで 4 本ずつ(別々の和に足すので、まとめて計算〈ベクトル化〉される)。
        // 数を 4 の倍数に切り上げた分の部分音は音量 0(使わない部分音は音量が 0 へ向かう)。
        // 進みはナイキスト未満 = 0.5 周未満なので、1 を超えたら 1 を引くだけでよい
        let m = n.div_ceil(4) * 4;
        let mut acc = [0.0f32; 4];
        for c in 0..m / 4 {
            for (l, sum) in acc.iter_mut().enumerate() {
                let k = c * 4 + l;
                let a = self.amp[k] + self.amp_step[k];
                self.amp[k] = a;
                *sum += a * crate::fm4::sin_turns(self.phase[k]);
                let next = self.phase[k] + self.inc[k];
                self.phase[k] = if next >= 1.0 { next - 1.0 } else { next };
            }
        }
        s += acc[0] + acc[1] + acc[2] + acc[3];
        s * self.env * self.vel * p.gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> AdditiveParams {
        AdditiveParams {
            partials: 32,
            tilt: -6.0,
            odd_even: 0.0,
            formant_hz: 1000.0,
            formant_db: 0.0,
            formant_width: 0.5,
            damping: 0.0,
            inharmonic: 0.0,
            wobble: 0.0,
            attack: 0.005,
            decay: 1.0,
            sustain: 1.0,
            release: 0.2,
            gain: 1.0,
            edits: PartialEdits::default(),
        }
    }

    /// 手で描いた山: 何番目の山はどの高さでも同じ番目、周波数の山は高さで当たる番目が変わる
    #[test]
    fn hand_drawn_hills_follow_index_or_frequency() {
        let ratio = |p: &AdditiveParams, f0: f32, k: f32| {
            let x = render(p, f0, 24_000);
            bin(&x[4800..], f0 * k) / bin(&x[4800..], f0)
        };
        let plain = params();
        // 何番目: 5 番目(0 からの 4)を +12 dB
        let mut idx = params();
        idx.edits = PartialEdits::parse("idx|4:12");
        for f0 in [200.0, 400.0] {
            let r = ratio(&idx, f0, 5.0) / ratio(&plain, f0, 5.0);
            assert!((3.4..=4.6).contains(&r), "{f0}: 5 番目が約 4 倍のはず {r}");
        }
        // 周波数: 2 kHz に +12 dB の山。200 Hz なら 10 番目、400 Hz なら 5 番目が上がる
        let mut hz = params();
        hz.edits = PartialEdits::parse("hz|2000:12");
        let r10 = ratio(&hz, 200.0, 10.0) / ratio(&plain, 200.0, 10.0);
        let r5 = ratio(&hz, 400.0, 5.0) / ratio(&plain, 400.0, 5.0);
        assert!(r10 > 3.4 && r5 > 3.4, "{r10} {r5}");
        // 山の外(2 半音より遠い所)は変わらない
        let far = ratio(&hz, 200.0, 5.0) / ratio(&plain, 200.0, 5.0);
        assert!((far - 1.0).abs() < 0.05, "{far}");
        // 読めない物は飛ばす
        assert_eq!(PartialEdits::parse("").n, 0);
        assert_eq!(PartialEdits::parse("x|1:2").n, 0);
        let e = PartialEdits::parse("hz|2000:3,x,10:4|1000:2,1200:1");
        assert_eq!((e.n, e.ng), (3, 2));
    }

    fn render(p: &AdditiveParams, f: f32, n: usize) -> Vec<f32> {
        let mut v = AdditiveVoice::start(p, f, 1.0, Articulation::Normal, 48_000.0);
        (0..n).map(|_| v.next(p)).collect()
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
    fn tilt_sets_the_harmonic_slope() {
        // −6dB/oct: k 番目は 1/k
        let x = render(&params(), 200.0, 24_000);
        let x = &x[4800..];
        let h1 = bin(x, 200.0);
        for k in [2.0f32, 3.0, 5.0] {
            let r = bin(x, 200.0 * k) / h1;
            assert!((r - 1.0 / k).abs() < 0.03, "{k}: {r}");
        }
        // 傾きを急にすると暗い
        let mut dark = params();
        dark.tilt = -15.0;
        let y = render(&dark, 200.0, 24_000);
        assert!(bin(&y[4800..], 1000.0) / bin(&y[4800..], 200.0) < 0.05);
    }

    #[test]
    fn odd_only_and_formant_and_nyquist() {
        let mut p = params();
        p.odd_even = -1.0;
        let x = render(&p, 200.0, 24_000);
        let x = &x[4800..];
        assert!(bin(x, 400.0) < 0.002 && bin(x, 600.0) > 0.03, "奇数だけ");
        // フォルマント 2kHz +18dB: 10 番目(2kHz)が持ち上がる
        let mut f = params();
        f.formant_hz = 2000.0;
        f.formant_db = 18.0;
        let y = render(&f, 200.0, 24_000);
        let plain = render(&params(), 200.0, 24_000);
        let ratio = |v: &[f32]| bin(&v[4800..], 2000.0) / bin(&v[4800..], 200.0);
        assert!(ratio(&y) > ratio(&plain) * 4.0);
        // 高い音: ナイキストを超える部分音は鳴らさない(折り返しの周波数に何も無い)
        let z = render(&params(), 5000.0, 24_000);
        let z = &z[4800..];
        assert!(bin(z, 5000.0) > 0.05);
        for g in [3000.0, 7000.0, 13_000.0] {
            assert!(bin(z, g) < 0.002, "{g}: {}", bin(z, g));
        }
    }

    #[test]
    fn damping_darkens_over_time_and_inharmonic_stretches() {
        let mut p = params();
        p.damping = 1.0;
        let x = render(&p, 200.0, 96_000);
        let r = |v: &[f32]| bin(v, 1200.0) / bin(v, 200.0);
        assert!(r(&x[2400..7200]) > r(&x[72_000..76_800]) * 5.0);
        // 伸び: 10 番目が 2000Hz より高い
        let mut q = params();
        q.inharmonic = 1.0;
        let y = render(&q, 200.0, 48_000);
        let y = &y[4800..];
        let f10 = 200.0 * 10.0 * (1.0f32 + 0.002 * 100.0).sqrt();
        assert!(bin(y, f10) > bin(y, 2000.0) * 3.0, "{f10}");
    }

    #[test]
    fn wobble_varies_by_seed_and_is_repeatable() {
        let mut p = params();
        p.wobble = 0.6;
        let go = |seed: u32| {
            let mut v =
                AdditiveVoice::start_seeded(&p, 220.0, 1.0, Articulation::Normal, 48_000.0, seed);
            (0..9600).map(|_| v.next(&p)).collect::<Vec<f32>>()
        };
        assert_eq!(go(1), go(1));
        assert_ne!(go(1), go(2));
    }

    #[test]
    fn releases_and_finishes() {
        let p = params();
        let mut v = AdditiveVoice::start(&p, 220.0, 1.0, Articulation::Normal, 48_000.0);
        for _ in 0..4800 {
            v.next(&p);
        }
        v.note_off();
        for _ in 0..48_000 {
            v.next(&p);
        }
        assert!(v.finished());
    }
}
