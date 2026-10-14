//! 音程を変えずに再生の速さを変える(聴き取りの練習用の遅い再生)。流しながら使う WSOLA。
//!
//! 再生エンジンの出力(ステレオ)を少しずつ受け取り、速さ `speed`(0.5 = 半分の速さ)で伸ばして返す。
//! 出力の各フレーム(40ms、半分ずつ重ねる)について、入力の本来の位置(前のフレームから `speed` × 20ms 進んだ所)の
//! 前後 ±10ms から、直前のフレームの自然な続きといちばん似た区間を探して重ね合わせる(オフラインの
//! [`crate::stretch::wsola_channels`] と同じ考え方)。左右は同じ位置で切り貼りするので定位が崩れない。
//!
//! オーディオスレッドで使う: 作るとき([`TimeStretcher::new`])にだけメモリを確保し、処理中は確保・ロックをしない。

/// フレーム長(秒)と探索幅(秒)
const FRAME_SECS: f32 = 0.040;
const TOLERANCE_SECS: f32 = 0.010;
/// 似かたの粗探索で間引く間隔(サンプル)
const COARSE: usize = 4;
/// 受け付ける遅さの下限(これより遅くはしない)
pub const MIN_SPEED: f32 = 0.25;

/// 流しながら伸ばす WSOLA
pub struct TimeStretcher {
    n: usize,
    hop: usize,
    tol: usize,
    window: Vec<f32>,
    /// 入力(先頭が `inp` の 0 番)
    inp_l: Vec<f32>,
    inp_r: Vec<f32>,
    in_len: usize,
    /// 次のフレームの入力の本来の位置(入力の先頭から)
    ana: f64,
    /// 直前に使った区間の頭
    prev: Option<usize>,
    /// 重ね合わせの途中(出力のこれからの `n` サンプル)
    acc_l: Vec<f32>,
    acc_r: Vec<f32>,
    /// できあがった出力(輪)
    out_l: Vec<f32>,
    out_r: Vec<f32>,
    out_rd: usize,
    out_len: usize,
    speed: f32,
}

impl TimeStretcher {
    /// `max_block` は 1 回に取り出す最大の長さ(オーディオの 1 回の呼び出しの長さ)
    pub fn new(sample_rate: f32, max_block: usize) -> Self {
        let n = ((FRAME_SECS * sample_rate) as usize).max(16) & !1;
        let hop = n / 2;
        let tol = ((TOLERANCE_SECS * sample_rate) as usize).max(1);
        // 入力: 1 フレーム分を読むのに要る長さの数倍(ためて詰め直す)
        let in_cap = (n + 2 * tol + hop) * 8;
        let out_cap = (max_block + n) * 2;
        TimeStretcher {
            n,
            hop,
            tol,
            window: (0..n)
                .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos())
                .collect(),
            inp_l: vec![0.0; in_cap],
            inp_r: vec![0.0; in_cap],
            in_len: 0,
            ana: 0.0,
            prev: None,
            acc_l: vec![0.0; n],
            acc_r: vec![0.0; n],
            out_l: vec![0.0; out_cap],
            out_r: vec![0.0; out_cap],
            out_rd: 0,
            out_len: 0,
            speed: 1.0,
        }
    }

    /// 途中の音を捨てて最初から(シーク・再生の開始)
    pub fn reset(&mut self) {
        self.in_len = 0;
        self.ana = 0.0;
        self.prev = None;
        self.acc_l.fill(0.0);
        self.acc_r.fill(0.0);
        self.out_rd = 0;
        self.out_len = 0;
    }

    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed.clamp(MIN_SPEED, 1.0);
    }

    /// 取り出せる出力の長さ
    pub fn available(&self) -> usize {
        self.out_len
    }

    /// 次のフレームを作るのに足りない入力の長さ(足りていれば 0)
    fn deficit(&self) -> usize {
        let target = self.ana.round() as usize;
        let mut need = target + self.tol + self.n;
        if let Some(p) = self.prev {
            need = need.max(p + self.hop + self.n);
        }
        need.saturating_sub(self.in_len)
    }

    /// 次のフレームを作るのに足りない入力の長さ(足りていれば 0)。入れられる長さを超えない
    pub fn input_wanted(&self) -> usize {
        self.deficit().min(self.inp_l.len() - self.in_len)
    }

    /// 入力を足す(インターリーブのステレオ)。入りきらない分は捨てる
    pub fn push_interleaved(&mut self, frames: &[f32]) {
        for c in frames.chunks_exact(2) {
            if self.in_len >= self.inp_l.len() {
                break;
            }
            self.inp_l[self.in_len] = c[0];
            self.inp_r[self.in_len] = c[1];
            self.in_len += 1;
        }
        self.run();
    }

    /// 作れるだけフレームを作る
    fn run(&mut self) {
        while self.deficit() == 0 && self.out_len + self.hop <= self.out_l.len() {
            self.step();
            self.compact();
        }
    }

    fn step(&mut self) {
        let n = self.n;
        let target = self.ana.round() as usize;
        let start = match self.prev {
            None => target,
            Some(p) => self.best_match(p + self.hop, target),
        };
        for i in 0..n {
            let w = self.window[i];
            self.acc_l[i] += self.inp_l[start + i] * w;
            self.acc_r[i] += self.inp_r[start + i] * w;
        }
        // 前半はもう重ならないので出力へ
        let cap = self.out_l.len();
        for i in 0..self.hop {
            let k = (self.out_rd + self.out_len + i) % cap;
            self.out_l[k] = self.acc_l[i];
            self.out_r[k] = self.acc_r[i];
        }
        self.out_len += self.hop;
        self.acc_l.copy_within(self.hop.., 0);
        self.acc_r.copy_within(self.hop.., 0);
        self.acc_l[n - self.hop..].fill(0.0);
        self.acc_r[n - self.hop..].fill(0.0);
        self.prev = Some(start);
        self.ana += self.hop as f64 * self.speed as f64;
    }

    /// もう読まない頭の部分を捨てて詰める(半分を超えたら)
    fn compact(&mut self) {
        let keep_from = (self.ana.floor() as usize)
            .saturating_sub(self.tol)
            .min(self.prev.map_or(usize::MAX, |p| p + self.hop));
        if keep_from == 0 || keep_from < self.inp_l.len() / 2 || keep_from > self.in_len {
            return;
        }
        self.inp_l.copy_within(keep_from..self.in_len, 0);
        self.inp_r.copy_within(keep_from..self.in_len, 0);
        self.in_len -= keep_from;
        self.ana -= keep_from as f64;
        self.prev = self.prev.map(|p| p - keep_from);
    }

    /// `target` の ±tol から、`natural` で始まる区間(重なりの長さ)といちばん似た開始位置
    fn best_match(&self, natural: usize, target: usize) -> usize {
        let lo = target.saturating_sub(self.tol);
        let hi = target + self.tol;
        let len = self.hop;
        let score = |cand: usize, stride: usize| -> f32 {
            let (mut dot, mut energy) = (0.0f32, 1e-9f32);
            let mut i = 0;
            while i < len {
                let r = self.inp_l[natural + i] + self.inp_r[natural + i];
                let s = self.inp_l[cand + i] + self.inp_r[cand + i];
                dot += r * s;
                energy += s * s;
                i += stride;
            }
            dot / energy.sqrt()
        };
        let mut best = target;
        let mut best_score = f32::MIN;
        let mut c = lo;
        while c <= hi {
            let s = score(c, 2);
            if s > best_score {
                best_score = s;
                best = c;
            }
            c += COARSE;
        }
        let (rlo, rhi) = (
            best.saturating_sub(COARSE - 1).max(lo),
            (best + COARSE - 1).min(hi),
        );
        best_score = f32::MIN;
        for c in rlo..=rhi {
            let s = score(c, 1);
            if s > best_score {
                best_score = s;
                best = c;
            }
        }
        best
    }

    /// 出力を取り出してインターリーブで書く(`channels` が 2 より多ければ残りは 0)。取り出した長さを返す
    pub fn pull(&mut self, out: &mut [f32], channels: usize) -> usize {
        let ch = channels.max(1);
        let frames = (out.len() / ch).min(self.out_len);
        let cap = self.out_l.len();
        for (f, frame) in out.chunks_mut(ch).take(frames).enumerate() {
            let k = (self.out_rd + f) % cap;
            frame[0] = self.out_l[k];
            if ch >= 2 {
                frame[1] = self.out_r[k];
                frame[2..].fill(0.0);
            }
        }
        self.out_rd = (self.out_rd + frames) % cap;
        self.out_len -= frames;
        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 正弦波を入れて `speed` で伸ばす: 長さが 1/speed 倍、音程(ゼロ交差の間隔)はそのまま
    #[test]
    fn stretches_length_and_keeps_pitch() {
        let sr = 48_000.0;
        for speed in [0.5f32, 0.75] {
            let mut st = TimeStretcher::new(sr, 1024);
            st.set_speed(speed);
            let f = 440.0;
            let total_in = 48_000usize;
            let mut fed = 0usize;
            let mut out = Vec::new();
            let mut buf = vec![0.0f32; 2048];
            let mut block = vec![0.0f32; 1024 * 2];
            while fed < total_in {
                let want = st.input_wanted().clamp(1, 1024).min(total_in - fed);
                for i in 0..want {
                    let v = (std::f32::consts::TAU * f * (fed + i) as f32 / sr).sin() * 0.5;
                    buf[i * 2] = v;
                    buf[i * 2 + 1] = v;
                }
                st.push_interleaved(&buf[..want * 2]);
                fed += want;
                while st.available() >= 1024 {
                    let n = st.pull(&mut block, 2);
                    out.extend(block[..n * 2].chunks(2).map(|c| c[0]));
                }
            }
            let expected = total_in as f32 / speed;
            let got = out.len() as f32;
            assert!(
                (got - expected).abs() / expected < 0.08,
                "{speed}: {got} vs {expected}"
            );
            // 真ん中あたりのゼロ交差の数から周波数
            let mid = &out[out.len() / 4..out.len() * 3 / 4];
            let crossings = mid.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
            let freq = crossings as f32 / (mid.len() as f32 / sr);
            assert!((freq - f).abs() < 6.0, "{speed}: {freq} Hz");
            // 途切れ・大きな山が無い
            let peak = mid.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak < 0.6 && peak > 0.4, "{peak}");
        }
    }

    #[test]
    fn reset_drops_buffered_audio() {
        let mut st = TimeStretcher::new(48_000.0, 512);
        st.set_speed(0.5);
        st.push_interleaved(&vec![0.3; 20_000]);
        assert!(st.available() > 0);
        st.reset();
        assert_eq!(st.available(), 0);
        assert!(st.input_wanted() > 0);
    }
}
