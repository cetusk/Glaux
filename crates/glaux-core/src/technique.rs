//! 奏法の細部(MCP の strum_chord・drum_rudiment の中身)。和音のストローク・ばらしと、ドラムのルーディメント。
//!
//! 数値は調査の目安(経験則を含む): ストロークの幅はギター 35ms・ピアノのばらし 120ms、拍の手前に幅の 2 割、
//! 後の弦ほど弱く(1 音 −8%)、上げのストロークは低い弦を省く。フラムの装飾音は 25ms 前・強さ半分、
//! ドラッグは 2 つ(40ms の中に)、ラフは 3 つ。1 秒 22 打を超える連打は人間には難しいので知らせる。
//! 長さは tick で受ける(ms からの換算は呼び出し側)。揺れは seed から作る。

use crate::model::Note;

/// ストロークの向き
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrumDir {
    /// 低い音から(ギターの下げ)
    Down,
    /// 高い音から(ギターの上げ)
    Up,
    /// 拍の頭は下げ、裏は上げ
    Alternate,
}

impl StrumDir {
    pub fn parse(s: &str) -> Option<StrumDir> {
        Some(match s.trim().to_lowercase().as_str() {
            "down" => StrumDir::Down,
            "up" => StrumDir::Up,
            "alternate" | "alternate_by_beat" => StrumDir::Alternate,
            _ => return None,
        })
    }
}

/// ストロークの条件
pub struct StrumOptions {
    pub dir: StrumDir,
    /// 最初の音から最後の音までの幅(tick)
    pub span: u64,
    /// 幅のうち拍の手前に出す割合 0〜1
    pub anchor: f64,
    /// 並びの曲がり −1〜1(正で後ろが詰まる = 最初はゆっくり)
    pub tension: f64,
    /// 後の音ほど強さを変える割合(1 音ごと。−0.08 で 8% ずつ弱く)
    pub vel_slope: f64,
    /// 上げのストロークで一番低い音を省く(4 音以上の和音)
    pub up_skip_low: bool,
    /// 揺れ(tick、±)
    pub jitter: u64,
    /// 拍の長さ(tick。alternate の表裏の判定)
    pub beat: u64,
    /// 同時とみなす幅(tick)
    pub tol: u64,
    pub seed: u64,
}

/// ストロークの結果: (音の番号, 新しい位置, 新しい長さ, 新しい強さ)と、省く音の番号
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StrumResult {
    pub moved: Vec<(usize, u64, u64, u8)>,
    pub removed: Vec<usize>,
    pub chords: usize,
}

fn unit(state: &mut u64) -> f64 {
    let mut x = (*state).max(1);
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *state = x;
    (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
}

/// 同じ位置(±`tol`)で鳴る音のまとまり(音の番号。低い順)。`abs` はクリップの頭の絶対 tick
pub fn chord_groups(notes: &[Note], tol: u64) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..notes.len()).collect();
    order.sort_by_key(|&i| (notes[i].pos, notes[i].pitch));
    let mut out: Vec<Vec<usize>> = Vec::new();
    for i in order {
        match out.last_mut() {
            Some(g) if notes[i].pos.0 <= notes[g[0]].pos.0 + tol => g.push(i),
            _ => out.push(vec![i]),
        }
    }
    for g in &mut out {
        g.sort_by_key(|&i| notes[i].pitch);
    }
    out
}

/// 和音をストロークにする。`clip_start` はクリップの頭の絶対 tick(表裏の判定用)
pub fn strum(notes: &[Note], clip_start: u64, o: &StrumOptions) -> StrumResult {
    let mut st = o.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x57;
    let mut res = StrumResult::default();
    for g in chord_groups(notes, o.tol) {
        if g.len() < 2 {
            continue;
        }
        res.chords += 1;
        let head = g.iter().map(|&i| notes[i].pos.0).min().unwrap_or(0);
        let on_beat = (clip_start + head) % o.beat.max(1) < o.tol.max(1);
        let down = match o.dir {
            StrumDir::Down => true,
            StrumDir::Up => false,
            StrumDir::Alternate => on_beat,
        };
        let mut order: Vec<usize> = g.clone();
        if !down {
            order.reverse();
            // 上げは低い弦に届かない
            if o.up_skip_low && order.len() >= 4 {
                res.removed.push(order.pop().expect("4 音以上"));
            }
        }
        let n = order.len();
        let e = 2f64.powf(o.tension.clamp(-1.0, 1.0));
        let early = (o.span as f64 * o.anchor.clamp(0.0, 1.0)).round() as i64;
        for (k, &i) in order.iter().enumerate() {
            let x = if n > 1 {
                k as f64 / (n - 1) as f64
            } else {
                0.0
            };
            let mut off = (o.span as f64 * x.powf(e)).round() as i64 - early;
            if o.jitter > 0 && k > 0 {
                off += ((unit(&mut st) * 2.0 - 1.0) * o.jitter as f64).round() as i64;
            }
            let nt = &notes[i];
            let end = nt.pos.0 + nt.dur.0;
            let pos = (head as i64 + off).max(0) as u64;
            let pos = pos.min(end.saturating_sub(1));
            let vel = (nt.vel as f64 * (1.0 + o.vel_slope * k as f64))
                .round()
                .clamp(1.0, 127.0) as u8;
            if pos != nt.pos.0 || vel != nt.vel {
                res.moved.push((i, pos, end - pos, vel));
            }
        }
    }
    res
}

/// ドラムのルーディメント
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rudiment {
    /// 装飾音 1 つ(25ms 前、強さ半分)
    Flam,
    /// 装飾音 2 つ
    Drag,
    /// 装飾音 3 つ
    Ruff,
    /// 音の長さの中を n 回に割る(Elektron のラチェット)
    Ratchet,
    /// 区間を同じ間隔で埋める(強さを傾けられる)
    Roll,
    /// バズロール(ごく細かく弱く、少し揺らす)
    Buzz,
}

impl Rudiment {
    pub fn parse(s: &str) -> Option<Rudiment> {
        Some(match s.trim().to_lowercase().as_str() {
            "flam" => Rudiment::Flam,
            "drag" => Rudiment::Drag,
            "ruff" => Rudiment::Ruff,
            "ratchet" => Rudiment::Ratchet,
            "roll" | "hat_roll" => Rudiment::Roll,
            "buzz" => Rudiment::Buzz,
            _ => return None,
        })
    }

    /// 音に付けるもの(区間を埋めるのでない)
    pub fn on_notes(self) -> bool {
        matches!(
            self,
            Rudiment::Flam | Rudiment::Drag | Rudiment::Ruff | Rudiment::Ratchet
        )
    }
}

/// 装飾音(前に置く音)の (本音からの前へのずれ tick, 強さ)。`grace` は装飾の間隔(tick)
pub fn grace_notes(kind: Rudiment, vel: u8, grace: u64, grace_vel: f64) -> Vec<(u64, u8)> {
    let count = match kind {
        Rudiment::Flam => 1,
        Rudiment::Drag => 2,
        Rudiment::Ruff => 3,
        _ => 0,
    };
    let v = ((vel as f64 * grace_vel).round() as u8).clamp(1, 127);
    // 本音に近いほど少し強く
    (0..count)
        .map(|k| {
            let back = grace * (count - k) as u64;
            let k_ratio = if count > 1 {
                0.85 + 0.15 * k as f64 / (count - 1) as f64
            } else {
                1.0
            };
            let vk = (v as f64 * k_ratio).round() as u8;
            (back, vk.max(1))
        })
        .collect()
}

/// 連打の位置と強さ: `len` を `step` ごとに、強さは `vel` から `vel_curve`(−128〜127: 負で弱く、正で強く
/// なっていく。Elektron と同じ向き)に沿って。`jitter` は強さの揺れ(0〜1)
pub fn fill(
    len: u64,
    step: u64,
    vel: u8,
    vel_curve: i32,
    jitter: f64,
    seed: u64,
) -> Vec<(u64, u8)> {
    let step = step.max(1);
    let n = len.div_ceil(step);
    let mut st = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x7A;
    let target = (vel as f64 * (1.0 + vel_curve.clamp(-128, 127) as f64 / 128.0)).clamp(1.0, 127.0);
    (0..n)
        .map(|k| {
            let x = if n > 1 {
                k as f64 / (n - 1) as f64
            } else {
                0.0
            };
            let mut v = vel as f64 + (target - vel as f64) * x;
            if jitter > 0.0 {
                v *= 1.0 + jitter * (unit(&mut st) * 2.0 - 1.0);
            }
            (k * step, v.round().clamp(1.0, 127.0) as u8)
        })
        .collect()
}

/// 音の切り方
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArticStyle {
    /// 次の音に少し重ねる
    Legato,
    /// 次の音まで目いっぱい
    Tenuto,
    /// 次の音までの 75%(KTH の規則の既定の量と同じ)
    Portato,
    /// 次の音までの 50%
    Staccato,
}

impl ArticStyle {
    pub fn parse(s: &str) -> Option<ArticStyle> {
        Some(match s.trim().to_lowercase().as_str() {
            "legato" => ArticStyle::Legato,
            "tenuto" => ArticStyle::Tenuto,
            "portato" => ArticStyle::Portato,
            "staccato" => ArticStyle::Staccato,
            _ => return None,
        })
    }

    pub fn default_ratio(self) -> f64 {
        match self {
            ArticStyle::Legato | ArticStyle::Tenuto => 1.0,
            ArticStyle::Portato => 0.75,
            ArticStyle::Staccato => 0.5,
        }
    }
}

/// 音の長さを切り方に合わせる。次の音の頭(和音は同じ位置のまとまり)までの間隔で決め、
/// 同じ音程が続くときは `repeat_gap` 空ける。戻り値は (音の番号, 新しい長さ)
pub fn articulate(
    notes: &[Note],
    targets: &[usize],
    style: ArticStyle,
    ratio: f64,
    overlap: u64,
    repeat_gap: u64,
) -> Vec<(usize, u64)> {
    let mut onsets: Vec<u64> = notes.iter().map(|n| n.pos.0).collect();
    onsets.sort_unstable();
    onsets.dedup();
    let mut out = Vec::new();
    for &i in targets {
        let n = &notes[i];
        let next = onsets.iter().find(|&&t| t > n.pos.0 + 10).copied();
        let ioi = next.map_or(n.dur.0, |t| t - n.pos.0);
        let same_next =
            next.is_some_and(|t| notes.iter().any(|m| m.pos.0 == t && m.pitch == n.pitch));
        let gap = if same_next { repeat_gap } else { 0 };
        let dur = match style {
            ArticStyle::Legato if !same_next && next.is_some() => ioi + overlap,
            ArticStyle::Legato | ArticStyle::Tenuto => ioi.saturating_sub(gap),
            ArticStyle::Portato | ArticStyle::Staccato => {
                ((ioi as f64 * ratio).round() as u64).min(ioi.saturating_sub(gap))
            }
        };
        let dur = dur.max(10);
        if dur != n.dur.0 {
            out.push((i, dur));
        }
    }
    out
}

/// トレモロの種類
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tremolo {
    /// 同じ音の連打(弦の刻み・マンドリン)
    Single,
    /// 2 音を交互(ピアノ・弦のフィンガートレモロ)
    Alternating,
    /// 和音の連打
    Chord,
}

impl Tremolo {
    pub fn parse(s: &str) -> Option<Tremolo> {
        Some(match s.trim().to_lowercase().as_str() {
            "single" => Tremolo::Single,
            "alternating" | "fingered" => Tremolo::Alternating,
            "chord" => Tremolo::Chord,
            _ => return None,
        })
    }
}

/// 1 音(か和音)を `step` ごとの連打にする: (頭からのずれ, 長さ, 強さの割合, 交互の上の音か)。
/// 1 打目の強さはそのまま、あとは 0.8〜0.9 を交互に(拍の頭を少し強く)
pub fn tremolo_hits(len: u64, step: u64, alternating: bool) -> Vec<(u64, u64, f64, bool)> {
    let step = step.max(10);
    let n = (len / step).max(1);
    (0..n)
        .map(|k| {
            let v = if k == 0 {
                1.0
            } else if k % 2 == 0 {
                0.9
            } else {
                0.8
            };
            let d = if k + 1 == n { len - k * step } else { step };
            (k * step, (d * 9 / 10).max(1), v, alternating && k % 2 == 1)
        })
        .collect()
}

/// グリッサンドの音の並び(`from` の次から `to` まで。`to` は含む)
pub fn glissando_pitches(from: u8, to: u8, pcs: &[u8]) -> Vec<u8> {
    let (lo, hi) = (from.min(to), from.max(to));
    let mut v: Vec<u8> = (lo..=hi)
        .filter(|p| *p != from && (pcs.is_empty() || pcs.contains(&(p % 12))))
        .collect();
    if from > to {
        v.reverse();
    }
    if v.last() != Some(&to) {
        v.retain(|&p| p != to);
        v.push(to);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::NoteId;
    use crate::time::Tick;

    fn note(pos: u64, dur: u64, pitch: u8, vel: u8) -> Note {
        Note {
            id: NoteId::new(),
            pos: Tick(pos),
            dur: Tick(dur),
            pitch,
            vel,
            articulation: Default::default(),
            pitch_curve: vec![],
            glide_ms: None,
            vibrato: None,
            volume_curve: vec![],
            brightness_curve: vec![],
        }
    }

    fn opts(dir: StrumDir) -> StrumOptions {
        StrumOptions {
            dir,
            span: 60,
            anchor: 0.0,
            tension: 0.0,
            vel_slope: -0.08,
            up_skip_low: true,
            jitter: 0,
            beat: 960,
            tol: 10,
            seed: 1,
        }
    }

    fn chord(pos: u64) -> Vec<Note> {
        [52, 57, 62, 67, 71, 76]
            .iter()
            .map(|&p| note(pos, 480, p, 100))
            .collect()
    }

    #[test]
    fn down_and_up_strokes() {
        let ns = chord(960);
        let r = strum(&ns, 0, &opts(StrumDir::Down));
        assert_eq!(r.chords, 1);
        // 下げ: 低い音から 0, 12, 24 … 60。終わりは同じ、後の音ほど弱い
        let low = r.moved.iter().find(|m| m.0 == 1).unwrap();
        assert_eq!((low.1, low.2), (972, 468));
        let top = r.moved.iter().find(|m| m.0 == 5).unwrap();
        assert_eq!(top.1, 1020);
        assert!(top.3 < low.3);
        assert!(r.removed.is_empty());
        // 上げ: 高い音から、一番低い音は省く
        let r = strum(&ns, 0, &opts(StrumDir::Up));
        assert_eq!(r.removed, vec![0]);
        let top = r.moved.iter().find(|m| m.0 == 5);
        assert!(top.is_none() || top.unwrap().1 == 960, "上げは高い音が先");
        // 交互: 拍の頭は下げ、裏は上げ
        let mut two = chord(0);
        two.extend(chord(480));
        let r = strum(&two, 0, &opts(StrumDir::Alternate));
        assert_eq!(r.chords, 2);
        assert_eq!(r.removed.len(), 1);
        // 拍の手前に出す
        let mut o = opts(StrumDir::Down);
        o.anchor = 0.2;
        let r = strum(&ns, 0, &o);
        assert_eq!(r.moved.iter().find(|m| m.0 == 0).unwrap().1, 948);
        // 単音は触らない
        assert_eq!(strum(&[note(0, 480, 60, 90)], 0, &o).chords, 0);
    }

    #[test]
    fn articulation_tremolo_and_glissando() {
        // 4 分の C D D E(D の連打)
        let ns = vec![
            note(0, 900, 60, 90),
            note(960, 900, 62, 90),
            note(1920, 900, 62, 90),
            note(2880, 900, 64, 90),
        ];
        let all = [0, 1, 2, 3];
        let leg = articulate(&ns, &all, ArticStyle::Legato, 1.0, 29, 58);
        assert!(leg.contains(&(0, 989)), "{leg:?}");
        assert!(leg.contains(&(1, 902)), "同じ音の連打は空ける: {leg:?}");
        let st = articulate(&ns, &all, ArticStyle::Staccato, 0.5, 29, 58);
        assert!(st.contains(&(0, 480)));
        let pt = articulate(&ns, &all, ArticStyle::Portato, 0.75, 29, 58);
        assert!(pt.contains(&(0, 720)));
        // トレモロ: 1 拍を 32 分で = 8 打、交互は裏が上の音
        let t = tremolo_hits(960, 120, true);
        assert_eq!(t.len(), 8);
        assert!(t[1].3 && !t[2].3);
        assert_eq!(t[0].2, 1.0);
        // グリッサンド: C4 → C5 の白鍵、下がるときは逆順
        let white = [0, 2, 4, 5, 7, 9, 11];
        assert_eq!(
            glissando_pitches(60, 72, &white),
            vec![62, 64, 65, 67, 69, 71, 72]
        );
        assert_eq!(glissando_pitches(67, 60, &white), vec![65, 64, 62, 60]);
        assert_eq!(glissando_pitches(60, 63, &[]), vec![61, 62, 63]);
        assert_eq!(ArticStyle::parse("portato"), Some(ArticStyle::Portato));
        assert_eq!(Tremolo::parse("chord"), Some(Tremolo::Chord));
    }

    #[test]
    fn rudiments_graces_and_fills() {
        let g = grace_notes(Rudiment::Drag, 100, 40, 0.5);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].0, 80);
        assert_eq!(g[1].0, 40);
        assert!(g.iter().all(|x| x.1 <= 50));
        assert!(grace_notes(Rudiment::Roll, 100, 40, 0.5).is_empty());
        // 1 拍を 32 分で = 8 打、だんだん強く
        let f = fill(960, 120, 60, 127, 0.0, 1);
        assert_eq!(f.len(), 8);
        assert!(f.last().unwrap().1 > f[0].1);
        assert_eq!(f[7].0, 840);
        assert_eq!(Rudiment::parse("hat_roll"), Some(Rudiment::Roll));
        assert!(Rudiment::Flam.on_notes() && !Rudiment::Buzz.on_notes());
    }
}
