//! リバーブ(8 本の遅延線のフィードバック・ディレイ・ネットワーク、FDN)。
//!
//! 入力 → プリディレイ → 4 段のオールパス(入力の拡散。Dattorro の構成)→ 8 本の遅延線。
//! 遅延線の出口は 1 次ローパス(ダンピング)を通り、Hadamard 行列(直交なので減衰を乱さない)で
//! 混ぜ合わせて入口へ戻す。遅延線ごとに「1 周で減る量」を残響時間(RT60)から決めるので、
//! 長さの違う線でも同じ速さで減衰する。遅延線の長さは揺らさない(揺らすと、似た音を作る
//! 自動合わせ(match_sound)でリバーブの量を探すときに、評価が乱れて合わせにくくなった)。
//! 左右の出力は、遅延線の出口を直交する符号の並びで足す(左右で無相関な広がり)。
//!
//! - 部屋(room): 30〜58ms の遅延線。プレート(plate): 18〜36ms の短く密な線、拡散を強め、高域を残す
//! - 長さはサンプルレートに合わせて伸縮する(96kHz まで同じ部屋。それより上は 96kHz の長さで頭打ち)
//! - バッファは作るときに 1 回だけ確保する。処理中はアロケーションしない

/// 遅延線の本数
const LINES: usize = 8;
/// 遅延線の長さ(48kHz のサンプル数。互いに素)
const ROOM_LENS: [f32; LINES] = [1433., 1601., 1867., 2053., 2251., 2399., 2617., 2797.];
const PLATE_LENS: [f32; LINES] = [887., 1009., 1123., 1277., 1361., 1499., 1613., 1741.];
/// 入力の拡散(オールパス)の長さ(48kHz)と係数
const DIFF_LENS: [f32; 4] = [210., 158., 561., 410.];
const ROOM_DIFF_G: [f32; 4] = [0.75, 0.75, 0.625, 0.625];
const PLATE_DIFF_G: [f32; 4] = [0.8, 0.8, 0.7, 0.7];
/// 伸縮の上限(96kHz まで)
const MAX_SCALE: f32 = 2.0;
/// プリディレイの上限(ms)
pub const PREDELAY_MAX_MS: f32 = 100.0;
/// 入力を遅延線へ配るときの符号と、左右の出力の符号(互いに直交)
const IN_SIGN: [f32; LINES] = [1., -1., 1., 1., -1., 1., -1., -1.];
const OUT_L: [f32; LINES] = [1., -1., 1., -1., 1., -1., 1., -1.];
const OUT_R: [f32; LINES] = [1., 1., -1., -1., 1., 1., -1., -1.];
/// 出力の大きさ(以前のリバーブと、ノイズを入れたときのウェットの音量がそろうように合わせた値。
/// プレートは線が短くエネルギーが多い分だけ下げる)
const ROOM_OUT_GAIN: f32 = 0.935;
const PLATE_OUT_GAIN: f32 = 0.80;

/// 設定の元の値(オートメーションで 1 つ変えたときに作り直す用)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReverbRaw {
    pub size: f32,
    pub damping: f32,
    pub predelay_ms: f32,
    pub plate: bool,
    pub sample_rate: f32,
}

/// 焼き込み済みのリバーブの設定
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReverbParams {
    /// ウェット比率 0..=1
    pub mix: f32,
    /// 遅延線の長さ(サンプル、サンプルレートに合わせて伸縮済み)
    len: [usize; LINES],
    /// 1 周ごとに掛ける減衰(残響時間から)
    gain: [f32; LINES],
    /// ダンピングの 1 次ローパスの係数(0 = 素通し)
    damp: f32,
    diff_len: [usize; 4],
    diff_g: [f32; 4],
    /// プリディレイ(サンプル)
    pre: usize,
    out_gain: f32,
    pub raw: ReverbRaw,
}

/// サイズ(0..1)→ 残響時間(秒)。以前のリバーブ(コムの帰還 0.7〜0.98)と同じ伸び方にしてある
pub fn rt60_of(size: f32) -> f32 {
    let fb = 0.7 + size.clamp(0.0, 1.0) * 0.28;
    -3.0 * 0.0324 / fb.log10()
}

impl ReverbParams {
    pub fn new(mix: f32, raw: ReverbRaw) -> Self {
        let sr = raw.sample_rate.max(1.0);
        let scale = (sr / 48_000.0).clamp(0.1, MAX_SCALE);
        let base = if raw.plate { PLATE_LENS } else { ROOM_LENS };
        let rt60 = rt60_of(raw.size);
        let len: [usize; LINES] = std::array::from_fn(|i| ((base[i] * scale) as usize).max(1));
        let gain = std::array::from_fn(|i| 10.0_f32.powf(-3.0 * len[i] as f32 / (sr * rt60)));
        // 48kHz での係数をほかのレートでも同じ周波数になるように(プレートは明るめ)
        let d = raw.damping.clamp(0.0, 1.0) * if raw.plate { 0.6 } else { 1.0 };
        let damp = d.powf(48_000.0 / sr);
        ReverbParams {
            mix: mix.clamp(0.0, 1.0),
            len,
            gain,
            damp,
            diff_len: std::array::from_fn(|k| ((DIFF_LENS[k] * scale) as usize).max(1)),
            diff_g: if raw.plate { PLATE_DIFF_G } else { ROOM_DIFF_G },
            pre: (raw.predelay_ms.clamp(0.0, PREDELAY_MAX_MS) * 0.001 * sr) as usize,
            out_gain: if raw.plate {
                PLATE_OUT_GAIN
            } else {
                ROOM_OUT_GAIN
            },
            raw,
        }
    }
}

/// 1 本の輪のバッファの区切り(共有バッファの中の位置・長さ・次に書く位置)
#[derive(Clone, Copy, Debug, Default)]
struct Ring {
    off: usize,
    cap: usize,
    w: usize,
}

impl Ring {
    #[inline]
    fn write(&mut self, buf: &mut [f32], v: f32) {
        buf[self.off + self.w] = v;
        self.w = (self.w + 1) % self.cap;
    }

    /// `d` サンプル前(1 = いちばん新しい)
    #[inline]
    fn read(&self, buf: &[f32], d: usize) -> f32 {
        let d = d.clamp(1, self.cap);
        buf[self.off + (self.w + self.cap - d) % self.cap]
    }
}

/// リバーブの状態(1 スロット分、ステレオ)
#[derive(Clone)]
pub struct FdnState {
    buf: Vec<f32>,
    lines: [Ring; LINES],
    diff: [Ring; 4],
    pre: Ring,
    lp: [f32; LINES],
}

impl Default for FdnState {
    fn default() -> Self {
        Self::new()
    }
}

impl FdnState {
    /// バッファを確保する(96kHz の部屋の長さ + プリディレイ 100ms)
    pub fn new() -> Self {
        let mut off = 0;
        let mut ring = |cap: usize| {
            let r = Ring { off, cap, w: 0 };
            off += cap;
            r
        };
        let longest = ROOM_LENS.iter().zip(PLATE_LENS).map(|(a, b)| a.max(b));
        let lines: Vec<Ring> = longest
            .map(|l| ring((l * MAX_SCALE) as usize + 2))
            .collect();
        let diff: Vec<Ring> = DIFF_LENS
            .iter()
            .map(|l| ring((l * MAX_SCALE) as usize + 2))
            .collect();
        let pre = ring((PREDELAY_MAX_MS * 0.001 * 48_000.0 * MAX_SCALE) as usize + 2);
        FdnState {
            buf: vec![0.0; off],
            lines: std::array::from_fn(|i| lines[i]),
            diff: std::array::from_fn(|i| diff[i]),
            pre,
            lp: [0.0; LINES],
        }
    }

    pub fn reset(&mut self) {
        self.buf.fill(0.0);
        self.lp = [0.0; LINES];
    }

    /// ステレオ 1 サンプル。戻り値はウェットだけ(ミックスは呼ぶ側)
    #[inline]
    pub fn process(&mut self, p: &ReverbParams, l: f32, r: f32) -> (f32, f32) {
        let buf = &mut self.buf;
        let mut x = (l + r) * 0.5;
        // プリディレイ
        self.pre.write(buf, x);
        if p.pre > 0 {
            x = self.pre.read(buf, p.pre);
        }
        // 入力の拡散(オールパス: w = x + g·w[n-D]、y = -g·w + w[n-D])
        for k in 0..4 {
            let ring = &mut self.diff[k];
            let delayed = ring.read(buf, p.diff_len[k]);
            let w = x + p.diff_g[k] * delayed;
            ring.write(buf, w);
            x = delayed - p.diff_g[k] * w;
        }
        // 遅延線の出口
        let y: [f32; LINES] = std::array::from_fn(|i| self.lines[i].read(buf, p.len[i]));
        let (mut out_l, mut out_r) = (0.0, 0.0);
        for i in 0..LINES {
            out_l += y[i] * OUT_L[i];
            out_r += y[i] * OUT_R[i];
        }
        // ダンピング → Hadamard で混ぜる → 減衰を掛けて入口へ
        let mut v = [0.0f32; LINES];
        for i in 0..LINES {
            self.lp[i] = y[i] * (1.0 - p.damp) + self.lp[i] * p.damp;
            v[i] = self.lp[i];
        }
        hadamard8(&mut v);
        let inject = x * std::f32::consts::FRAC_1_SQRT_2 * 0.5;
        for i in 0..LINES {
            let w = inject * IN_SIGN[i] + p.gain[i] * v[i];
            self.lines[i].write(buf, w);
        }
        (out_l * p.out_gain, out_r * p.out_gain)
    }
}

/// 8 点の Hadamard 変換(直交になるよう 1/√8 を掛ける)
#[inline]
fn hadamard8(v: &mut [f32; LINES]) {
    let mut h = 1;
    while h < LINES {
        let mut i = 0;
        while i < LINES {
            for j in i..i + h {
                let (a, b) = (v[j], v[j + h]);
                v[j] = a + b;
                v[j + h] = a - b;
            }
            i += 2 * h;
        }
        h *= 2;
    }
    let n = 1.0 / (LINES as f32).sqrt();
    for x in v.iter_mut() {
        *x *= n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(size: f32, plate: bool, sr: f32) -> ReverbRaw {
        ReverbRaw {
            size,
            damping: 0.0,
            predelay_ms: 0.0,
            plate,
            sample_rate: sr,
        }
    }

    /// インパルス応答のエネルギーが -60dB になるまでの秒数
    fn t60(p: &ReverbParams, sr: f32) -> f32 {
        let mut st = FdnState::new();
        let n = (sr * 30.0) as usize;
        let block = (sr * 0.05) as usize;
        let mut first = 0.0f32;
        let mut e = 0.0f32;
        for i in 0..n {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let (l, r) = st.process(p, x, x);
            e += l * l + r * r;
            if (i + 1) % block == 0 {
                let k = (i + 1) / block;
                // 最初の 0.1〜0.2 秒の区間を基準に
                if k == 3 {
                    first = e;
                } else if k > 3 && first > 0.0 && 10.0 * (e / first).log10() < -60.0 {
                    return (k - 3) as f32 * 0.05;
                }
                e = 0.0;
            }
        }
        f32::INFINITY
    }

    #[test]
    fn decay_follows_the_size() {
        for size in [0.0, 0.5, 0.9] {
            let want = rt60_of(size);
            let got = t60(
                &ReverbParams::new(1.0, raw(size, false, 48_000.0)),
                48_000.0,
            );
            assert!(
                (got - want).abs() < want * 0.2 + 0.1,
                "size {size}: {got:.2} 秒(期待 {want:.2})"
            );
        }
    }

    #[test]
    fn same_room_at_other_sample_rates() {
        let a = t60(&ReverbParams::new(1.0, raw(0.5, false, 48_000.0)), 48_000.0);
        let b = t60(&ReverbParams::new(1.0, raw(0.5, false, 96_000.0)), 96_000.0);
        let c = t60(&ReverbParams::new(1.0, raw(0.5, false, 44_100.0)), 44_100.0);
        assert!(
            (a - b).abs() < 0.15 && (a - c).abs() < 0.15,
            "{a} / {b} / {c}"
        );
    }

    #[test]
    fn output_is_wide_and_stable() {
        // 左右の相関が低い(広がる)こと、長く回しても発散しないこと
        let p = ReverbParams::new(1.0, raw(1.0, false, 48_000.0));
        let mut st = FdnState::new();
        let mut rng: u32 = 7;
        let (mut lr, mut ll, mut rr) = (0.0f64, 0.0f64, 0.0f64);
        let mut peak = 0.0f32;
        for i in 0..480_000 {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            let x = if i < 48_000 {
                rng as f32 / u32::MAX as f32 - 0.5
            } else {
                0.0
            };
            let (l, r) = st.process(&p, x, x);
            if (24_000..48_000).contains(&i) {
                lr += (l * r) as f64;
                ll += (l * l) as f64;
                rr += (r * r) as f64;
            }
            peak = peak.max(l.abs()).max(r.abs());
        }
        let corr = lr / (ll * rr).sqrt();
        assert!(corr.abs() < 0.3, "相関 {corr:.2}");
        assert!(peak < 4.0 && peak.is_finite(), "{peak}");
    }

    #[test]
    fn predelay_delays_the_onset() {
        let mut r = raw(0.5, false, 48_000.0);
        r.predelay_ms = 50.0;
        let p = ReverbParams::new(1.0, r);
        let mut st = FdnState::new();
        let first = (0..48_000)
            .position(|i| {
                let x = if i == 0 { 1.0 } else { 0.0 };
                let (l, r) = st.process(&p, x, x);
                l.abs() + r.abs() > 1e-4
            })
            .unwrap();
        // プリディレイ 50ms(2400)+ 最短の線(1433)より後
        assert!(first >= 2400 + 1433 - 8, "{first}");
    }

    #[test]
    fn plate_is_denser_early() {
        // プレートは最初の反射が早い(短い線)
        let onset = |plate: bool| {
            let p = ReverbParams::new(1.0, raw(0.5, plate, 48_000.0));
            let mut st = FdnState::new();
            (0..48_000)
                .position(|i| {
                    let x = if i == 0 { 1.0 } else { 0.0 };
                    let (l, r) = st.process(&p, x, x);
                    l.abs() + r.abs() > 1e-4
                })
                .unwrap()
        };
        assert!(onset(true) < onset(false));
    }
}
