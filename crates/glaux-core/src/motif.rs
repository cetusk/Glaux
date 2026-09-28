//! 動機の展開(MCP の develop_motif の中身)。LLM が書いた短い動機を、形式の型に沿って決まった手順で展開する。
//!
//! LLM は動機を写すだけで展開しない・形式を保てない(Zhou ら 2024)。そこで意図(動機・形式・山の位置)は LLM が決め、
//! 展開は道具が行う。どの操作も音階の度数の空間で計算し(`transform::Scale`)、強拍の音は和音の音に合わせる。
//!
//! 形式は「スロット」(動機の長さの区切り)の並び。各スロットに操作の列を当てる:
//! - `a`: 動機そのまま / `adapt`: リズムと輪郭を保って和音に合わせる / `seq(n)`: 音階の度数で n ずらす
//! - `frag`: 動機の前半を 2 回(2 回目は 1 度下へ)/ `invert`: 反行 / `retro`: 逆行
//! - `tail`: 最後の音を開いて終える(2 度・5 度。問い)/ `cadence`: 最後の音を主音で閉じて伸ばす(答え)
//! - `fill`: 7 半音以上の跳躍の後を、逆向きの順次進行に

use crate::chord::{Chord, Key};
use crate::comp::Span;
use crate::transform::Scale;

/// 動機の 1 音(動機の頭からの tick)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MotifNote {
    pub offset: u64,
    pub dur: u64,
    pub pitch: u8,
}

/// スロット 1 つの操作
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Adapt,
    Seq(i32),
    Frag,
    Invert,
    Retro,
    Tail,
    Cadence,
    Fill,
    /// リズムの変奏(長い音を割る・短い音をまとめる・付点・弱拍を休みに。seed で決まる 1〜2 種類)
    Vary,
    /// 前半を倍の長さに(ゆっくり)
    Augment,
    /// 倍の速さで 2 回
    Diminish,
    /// 8 分 n 個ぶんずらして入る(正 = 遅れて、負 = 食って)
    Displace(i32),
}

/// 形式の型 → スロットの操作の並び
pub fn form(name: &str) -> Option<Vec<Vec<Op>>> {
    use Op::*;
    Some(match name.trim().to_lowercase().as_str() {
        // 提示 → 反復(和音に合わせる)→ 断片化 → 終止
        "sentence" => vec![
            vec![],
            vec![Adapt, Vary],
            vec![Frag, Adapt],
            vec![Adapt, Vary, Cadence],
        ],
        // 前楽節(開いて終わる)+ 後楽節(同じ頭で閉じる)
        "period" => vec![
            vec![],
            vec![Adapt, Tail],
            vec![Adapt, Vary],
            vec![Adapt, Cadence],
        ],
        // 同じ句 2 回 → 対比(反行して上へ)→ 戻る
        "aaba" => vec![
            vec![],
            vec![Adapt, Vary],
            vec![Invert, Seq(2), Displace(1), Adapt, Fill],
            vec![Adapt, Cadence],
        ],
        "aab" => vec![
            vec![],
            vec![Adapt, Vary],
            vec![Frag, Seq(1), Adapt, Cadence],
        ],
        // 問いと答え(答えは逆行)
        "call_response" => vec![
            vec![],
            vec![Retro, Adapt, Tail],
            vec![Adapt, Vary],
            vec![Retro, Adapt, Cadence],
        ],
        // EDM・トラップのループ: 同じ動機を和音に合わせて繰り返し、最後だけ変える
        "loop" => vec![vec![], vec![Adapt], vec![Adapt, Vary], vec![Adapt, Tail]],
        _ => return None,
    })
}

/// "a | adapt | frag seq(-1) | cadence" の形の並びを読む
pub fn parse_plan(s: &str) -> Result<Vec<Vec<Op>>, String> {
    if let Some(f) = form(s) {
        return Ok(f);
    }
    let mut out = Vec::new();
    for slot in s.split('|') {
        let mut ops = Vec::new();
        for tok in slot.split_whitespace() {
            let t = tok.to_lowercase();
            let op = match t.as_str() {
                "a" => continue,
                "adapt" => Op::Adapt,
                "frag" => Op::Frag,
                "invert" => Op::Invert,
                "retro" => Op::Retro,
                "tail" => Op::Tail,
                "cadence" => Op::Cadence,
                "fill" => Op::Fill,
                "vary" => Op::Vary,
                "augment" => Op::Augment,
                "diminish" => Op::Diminish,
                _ => {
                    let arg = |name: &str| {
                        t.strip_prefix(name)
                            .and_then(|r| r.strip_suffix(')'))
                            .and_then(|n| n.parse::<i32>().ok())
                    };
                    if let Some(n) = arg("seq(") {
                        Op::Seq(n.clamp(-7, 7))
                    } else if let Some(n) = arg("displace(") {
                        Op::Displace(n.clamp(-4, 4))
                    } else {
                        return Err(format!(
                            "読めない操作「{tok}」(a / adapt / seq(n) / frag / invert / retro / tail / cadence / fill / \
                             vary / augment / diminish / displace(n))"
                        ));
                    }
                }
            };
            ops.push(op);
        }
        out.push(ops);
    }
    if out.is_empty() || out.len() > 32 {
        return Err("plan のスロットは 1〜32 個".to_owned());
    }
    Ok(out)
}

/// 展開の条件
pub struct Options {
    pub key: Key,
    /// 音域
    pub low: u8,
    pub high: u8,
    /// 最高音を置くスロット(None なら全体の 60〜75% の位置)
    pub peak_slot: Option<usize>,
    /// 最高音の高さ(None なら展開したままの高さ)
    pub peak_pitch: Option<u8>,
    /// 強拍の音を 8 分前へ食わせる割合 0〜1
    pub anticipate: f64,
    pub seed: u64,
}

/// 展開した旋律の 1 音(クリップの頭から)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Out {
    pub pos: u64,
    pub dur: u64,
    pub pitch: u8,
}

fn chord_tones(c: &Chord) -> Vec<u8> {
    c.pitch_classes()
}

/// `p` に最も近い、`pcs` のどれかの音(同じ近さなら上)
fn nearest(p: i32, pcs: &[u8]) -> i32 {
    crate::harmony::snap_to_pitch_classes(p, pcs)
}

/// 動機を形式に沿って展開する。`bar_len` はスロットの長さを決める小節の長さ(tick)。
/// `chord_at(クリップの頭からの tick)` はその時の和音
pub fn develop(
    motif: &[MotifNote],
    plan: &[Vec<Op>],
    bar_len: u64,
    chord_at: &dyn Fn(u64) -> Option<Chord>,
    opts: &Options,
) -> Result<Vec<Out>, String> {
    if motif.is_empty() {
        return Err("動機の音がありません".to_owned());
    }
    let scale_pcs = crate::harmony::scale_pitch_classes(
        opts.key.tonic,
        if opts.key.minor { "minor" } else { "major" },
    );
    let scale = Scale::new(scale_pcs.clone());
    let motif_end = motif.iter().map(|n| n.offset + n.dur).max().unwrap_or(0);
    let slot_len = motif_end.div_ceil(bar_len).clamp(1, 4) * bar_len;
    let beat = crate::time::PPQ;
    let is_strong = |rel_in_bar: u64| rel_in_bar % (bar_len / 2).max(1) < 60;
    let mut out: Vec<Out> = Vec::new();
    let mut keep_pc: Vec<bool> = Vec::new();
    for (si, ops) in plan.iter().enumerate() {
        let base = si as u64 * slot_len;
        // 動機を度数の空間で変形する(位置は動機の頭から)
        let mut notes: Vec<MotifNote> = motif.to_vec();
        for op in ops {
            match op {
                Op::Seq(n) => {
                    for x in &mut notes {
                        x.pitch = scale.shift(x.pitch, *n).clamp(0, 127) as u8;
                    }
                }
                Op::Invert => {
                    let axis = notes[0].pitch;
                    for x in &mut notes {
                        x.pitch = scale.invert(x.pitch, axis).clamp(0, 127) as u8;
                    }
                }
                Op::Retro => {
                    let end = notes.iter().map(|n| n.offset + n.dur).max().unwrap_or(0);
                    let start = notes.iter().map(|n| n.offset).min().unwrap_or(0);
                    for x in &mut notes {
                        x.offset = start + end - (x.offset + x.dur);
                    }
                    notes.sort_by_key(|n| n.offset);
                }
                Op::Frag => {
                    let half = (slot_len / 2).max(beat);
                    let first: Vec<MotifNote> = notes
                        .iter()
                        .filter(|n| n.offset < half)
                        .map(|n| MotifNote {
                            dur: n.dur.min(half - n.offset),
                            ..*n
                        })
                        .collect();
                    let mut v = first.clone();
                    v.extend(first.iter().map(|n| MotifNote {
                        offset: n.offset + half,
                        pitch: scale.shift(n.pitch, -1).clamp(0, 127) as u8,
                        dur: n.dur,
                    }));
                    notes = v;
                }
                Op::Augment => {
                    // 前半を倍の長さに
                    let half = slot_len / 2;
                    notes = notes
                        .iter()
                        .filter(|n| n.offset < half)
                        .map(|n| MotifNote {
                            offset: n.offset * 2,
                            dur: (n.dur * 2).min(slot_len - n.offset * 2),
                            pitch: n.pitch,
                        })
                        .collect();
                }
                Op::Diminish => {
                    // 倍の速さで 2 回(2 回目は 1 度上)
                    let half = slot_len / 2;
                    let fast: Vec<MotifNote> = notes
                        .iter()
                        .map(|n| MotifNote {
                            offset: n.offset / 2,
                            dur: (n.dur / 2).max(60),
                            pitch: n.pitch,
                        })
                        .filter(|n| n.offset < half)
                        .collect();
                    let mut v = fast.clone();
                    v.extend(fast.iter().map(|n| MotifNote {
                        offset: n.offset + half,
                        pitch: scale.shift(n.pitch, 1).clamp(0, 127) as u8,
                        dur: n.dur,
                    }));
                    notes = v;
                }
                Op::Displace(n) => {
                    // 8 分 n 個ぶんずらす(はみ出す音は落とす)
                    let d = *n as i64 * (beat as i64 / 2);
                    notes = notes
                        .iter()
                        .filter_map(|x| {
                            let o = x.offset as i64 + d;
                            (o >= 0 && (o as u64) < slot_len).then(|| MotifNote {
                                offset: o as u64,
                                dur: x.dur.min(slot_len - o as u64),
                                pitch: x.pitch,
                            })
                        })
                        .collect();
                }
                Op::Vary => vary(
                    &mut notes,
                    &scale,
                    beat,
                    opts.seed.wrapping_add(si as u64 * 31),
                ),
                _ => {}
            }
        }
        // 和音に合わせる: 全体を度数で -3〜+3 ずらして強拍の音が和音の音に最も近くなる所を選び、
        // 残りの強拍の和音の外の音を最も近い和音の音へ、弱拍の音は音階の音へ
        if ops.contains(&Op::Adapt) {
            let strong: Vec<(usize, Vec<u8>)> = notes
                .iter()
                .enumerate()
                .filter(|(_, n)| is_strong((base + n.offset) % bar_len))
                .filter_map(|(i, n)| chord_at(base + n.offset).map(|c| (i, chord_tones(&c))))
                .collect();
            let cost = |shift: i32| -> i32 {
                strong
                    .iter()
                    .map(|(i, pcs)| {
                        let p = scale.shift(notes[*i].pitch, shift);
                        (nearest(p, pcs) - p).abs() + shift.abs() / 2
                    })
                    .sum()
            };
            let best = (-3..=3)
                .min_by_key(|s| (cost(*s), s.abs(), *s))
                .unwrap_or(0);
            for x in &mut notes {
                x.pitch = scale.shift(x.pitch, best).clamp(0, 127) as u8;
            }
            for (i, pcs) in &strong {
                let p = notes[*i].pitch as i32;
                notes[*i].pitch = nearest(p, pcs).clamp(0, 127) as u8;
            }
            for x in &mut notes {
                if !is_strong((base + x.offset) % bar_len) {
                    x.pitch = nearest(x.pitch as i32, &scale_pcs).clamp(0, 127) as u8;
                }
            }
        }
        // 跳躍の後の戻し
        if ops.contains(&Op::Fill) {
            for i in 2..notes.len() {
                let a = notes[i - 1].pitch as i32 - notes[i - 2].pitch as i32;
                let b = notes[i].pitch as i32 - notes[i - 1].pitch as i32;
                if a.abs() >= 7 && (b == 0 || b.signum() == a.signum()) {
                    notes[i].pitch =
                        scale.shift(notes[i - 1].pitch, -a.signum()).clamp(0, 127) as u8;
                }
            }
        }
        // 句の終わり: 問い(開く)は 2 度か 5 度、答え(閉じる)は主音。答えは伸ばす
        let open = ops.contains(&Op::Tail);
        let close = ops.contains(&Op::Cadence);
        if (open || close) && !notes.is_empty() {
            let last = notes.len() - 1;
            let targets: Vec<u8> = if close {
                vec![scale_pcs[0]]
            } else {
                vec![scale_pcs[1], scale_pcs[4]]
            };
            let p = notes[last].pitch as i32;
            // 和音の音と重なる方を優先
            let chord = chord_at(base + notes[last].offset).map(|c| chord_tones(&c));
            let preferred: Vec<u8> = match &chord {
                Some(pcs) => {
                    let both: Vec<u8> = targets
                        .iter()
                        .copied()
                        .filter(|t| pcs.contains(t))
                        .collect();
                    if both.is_empty() {
                        targets.clone()
                    } else {
                        both
                    }
                }
                None => targets.clone(),
            };
            // 句の終わりは下がる(研究の傾向)。1 オクターブ以内の下の候補
            let below = (p - 11..=p)
                .rev()
                .find(|q| preferred.contains(&(q.rem_euclid(12) as u8)));
            notes[last].pitch = below
                .unwrap_or_else(|| nearest(p, &preferred))
                .clamp(0, 127) as u8;
            if close {
                // 句の終わりはスロットの終わりの 8 分前まで伸ばす
                let end = slot_len.saturating_sub(beat / 2);
                if end > notes[last].offset {
                    notes[last].dur = notes[last].dur.max(end - notes[last].offset);
                }
            }
        }
        let count = notes.len();
        for (i, n) in notes.into_iter().enumerate() {
            if n.offset >= slot_len {
                continue;
            }
            out.push(Out {
                pos: base + n.offset,
                dur: n.dur.min(slot_len - n.offset).max(1),
                pitch: n.pitch,
            });
            // 句の終わりの音(問い・答え)は、下げるときも音名を保つ
            keep_pc.push((open || close) && i + 1 == count);
        }
    }
    // 音域: 全体をオクターブで動かして中心を合わせ、はみ出す音はオクターブで戻す
    if !out.is_empty() {
        let mean = out.iter().map(|o| o.pitch as f64).sum::<f64>() / out.len() as f64;
        let center = (opts.low as f64 + opts.high as f64) / 2.0;
        let shift = ((center - mean) / 12.0).round() as i32 * 12;
        for o in &mut out {
            let mut p = o.pitch as i32 + shift;
            while p > opts.high as i32 && p - 12 >= 0 {
                p -= 12;
            }
            while p < opts.low as i32 && p + 12 <= 127 {
                p += 12;
            }
            o.pitch = p as u8;
        }
    }
    let nslots = plan.len();
    // 最高音: 狙ったスロットに 1 回だけ。ほかのスロットの音はそれより下げる
    let peak_slot = opts
        .peak_slot
        .unwrap_or(((nslots as f64 - 1.0) * 0.67).round() as usize)
        .min(nslots - 1);
    let slot_of = |o: &Out| (o.pos / slot_len) as usize;
    if let Some(pp) = opts.peak_pitch {
        if let Some(top) = out
            .iter_mut()
            .filter(|o| (o.pos / slot_len) as usize == peak_slot)
            .max_by_key(|o| o.pitch)
        {
            top.pitch = pp;
        }
    }
    // 下げる: 句の終わりの音はオクターブで、ほかは音階の 1 度ずつ
    let lower = |p: u8, keep: bool| {
        if keep {
            p.saturating_sub(12)
        } else {
            scale.shift(p, -1).clamp(0, 127) as u8
        }
    };
    if nslots > 1 && opts.peak_pitch.is_none() {
        // 山のスロットがほかより低いときは、ほかを下げず(動機の形を崩さない)、山のスロットを上へ移す
        let outside = out
            .iter()
            .filter(|o| slot_of(o) != peak_slot)
            .map(|o| o.pitch)
            .max();
        let inside = out
            .iter()
            .filter(|o| slot_of(o) == peak_slot)
            .map(|o| o.pitch)
            .max();
        if let (Some(outside), Some(inside)) = (outside, inside) {
            if inside <= outside {
                // 山のスロット全体を音階の度数で上へ移す(反復進行で上げる。輪郭を保つ)。
                // ほかのどの音より高くなる最小の段数(4 段まで)。句の終わりの音(問い・答え)は動かさない
                let steps = (1..=4)
                    .find(|&k| {
                        out.iter()
                            .zip(&keep_pc)
                            .filter(|(o, keep)| slot_of(o) == peak_slot && !**keep)
                            .any(|(o, _)| scale.shift(o.pitch, k) > outside as i32)
                    })
                    .unwrap_or(4);
                for (o, keep) in out.iter_mut().zip(&keep_pc) {
                    if (o.pos / slot_len) as usize != peak_slot || *keep {
                        continue;
                    }
                    let mut p = scale.shift(o.pitch, steps);
                    // 強拍は和音の音へ(移した後も和音に合う)
                    if is_strong(o.pos % bar_len) {
                        if let Some(c) = chord_at(o.pos) {
                            p = nearest(p, &chord_tones(&c));
                        }
                    }
                    o.pitch = p.clamp(0, opts.high as i32) as u8;
                }
            }
        }
    }
    if nslots > 1 {
        if let Some(peak) = out
            .iter()
            .filter(|o| slot_of(o) == peak_slot)
            .map(|o| o.pitch)
            .max()
        {
            let mut seen = false;
            for (o, keep) in out.iter_mut().zip(&keep_pc) {
                if (o.pos / slot_len) as usize != peak_slot {
                    while o.pitch >= peak {
                        o.pitch = lower(o.pitch, *keep);
                    }
                } else if o.pitch == peak {
                    // 山のスロットの中でも最高音は 1 回だけ(2 回目以降は下げる)
                    if seen {
                        o.pitch = lower(o.pitch, *keep);
                    }
                    seen = true;
                }
            }
        }
    }
    out.sort_by_key(|o| (o.pos, o.pitch));
    // 食い: 強拍(小節の頭・半ば)の音を 8 分前へ(前の音を短くして)。最初の音と句の頭は動かさない
    if opts.anticipate > 0.0 {
        let mut rng = opts.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut unit = || {
            rng ^= rng >> 12;
            rng ^= rng << 25;
            rng ^= rng >> 27;
            (rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
        };
        for i in 1..out.len() {
            let pos = out[i].pos;
            if !is_strong(pos % bar_len) || pos % slot_len == 0 {
                continue;
            }
            let prev = out[i - 1];
            let new_pos = pos - beat / 2;
            // 前の音が 8 分前より前に始まり、短くしても 16 分以上残るときだけ
            if prev.pos + beat / 4 > new_pos || unit() >= opts.anticipate {
                continue;
            }
            out[i - 1].dur = out[i - 1].dur.min(new_pos - prev.pos);
            out[i].dur += pos - new_pos;
            out[i].pos = new_pos;
        }
    }
    out.sort_by_key(|o| (o.pos, o.pitch));
    Ok(out)
}

fn next_unit(state: &mut u64) -> f64 {
    let mut x = (*state).max(1);
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *state = x;
    (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
}

/// リズムの変奏: 長い音を割る / 同じ長さの 2 音を付点に / 短い 2 音をまとめる / 弱拍の短い音を休みに、から
/// seed で 1〜2 種類を選んで当てる(当てられないものは飛ばす)。音程の輪郭は保つ
fn vary(notes: &mut Vec<MotifNote>, scale: &Scale, beat: u64, seed: u64) {
    if notes.len() < 2 {
        return;
    }
    let mut st = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x5EED;
    let mut kinds = [0usize, 1, 2, 3];
    // 選ぶ順を seed で並べ替える
    for i in (1..kinds.len()).rev() {
        let j = (next_unit(&mut st) * (i + 1) as f64) as usize;
        kinds.swap(i, j.min(i));
    }
    let want = 1 + (next_unit(&mut st) < 0.5) as usize;
    let mut done = 0;
    let mut used: Vec<usize> = Vec::new();
    for k in kinds {
        if done >= want {
            break;
        }
        // 割る(0)と、まとめる(2)・休みにする(3)は打ち消し合うので、片方だけ
        let clash = |a: usize, b: usize| (a == 0 && b >= 2) || (b == 0 && a >= 2);
        if used.iter().any(|&u| clash(u, k)) {
            continue;
        }
        let last = notes.len() - 1;
        let applied = match k {
            // 長い音(4 分以上、最後以外)を 2 つに割り、後ろは次の音へ向かう隣の音。
            // 後ろが休みの音(句の終わりの伸ばし。息継ぎの前)は割らない
            0 => {
                let pick = (0..last)
                    .filter(|&i| {
                        notes[i].dur >= beat
                            && notes[i].offset + notes[i].dur >= notes[i + 1].offset
                    })
                    .max_by_key(|&i| (notes[i].dur, std::cmp::Reverse(i)));
                pick.map(|i| {
                    let n = notes[i];
                    let half = n.dur / 2;
                    let dir = if notes[i + 1].pitch >= n.pitch { 1 } else { -1 };
                    notes[i].dur = half;
                    notes.insert(
                        i + 1,
                        MotifNote {
                            offset: n.offset + half,
                            dur: n.dur - half,
                            pitch: scale.shift(n.pitch, dir).clamp(0, 127) as u8,
                        },
                    );
                })
                .is_some()
            }
            // 拍の頭で始まる同じ長さの 2 音を、付点 + 短い音に
            1 => {
                let pick = (0..last).find(|&i| {
                    let (a, b) = (notes[i], notes[i + 1]);
                    // 8 分か 4 分の音だけ(付点 8 分を付点にすると 32 分の細かさになる)
                    a.dur == b.dur
                        && (a.dur == beat / 2 || a.dur == beat)
                        && a.offset % beat == 0
                        && a.offset + a.dur == b.offset
                });
                pick.map(|i| {
                    let d = notes[i].dur;
                    notes[i].dur = d + d / 2;
                    notes[i + 1].offset += d / 2;
                    notes[i + 1].dur = d / 2;
                })
                .is_some()
            }
            // 続く短い 2 音(8 分以下)を 1 つにまとめる
            2 => {
                let pick = (0..last).find(|&i| {
                    let (a, b) = (notes[i], notes[i + 1]);
                    a.dur <= beat / 2 && b.dur <= beat / 2 && a.offset + a.dur == b.offset
                });
                pick.map(|i| {
                    notes[i].dur += notes[i + 1].dur;
                    notes.remove(i + 1);
                })
                .is_some()
            }
            // 弱拍で始まる短い音(最初と最後以外)を休みに
            _ => {
                let pick =
                    (1..last).find(|&i| notes[i].offset % beat != 0 && notes[i].dur <= beat / 2);
                pick.map(|i| {
                    notes.remove(i);
                })
                .is_some()
            }
        };
        if applied {
            done += 1;
            used.push(k);
        }
    }
}

/// 区間(comp::Span)と和音の列から、tick → 和音 を引く関数を作る
pub fn chord_lookup<'a>(
    spans: &'a [Span],
    chords: &'a [Chord],
) -> impl Fn(u64) -> Option<Chord> + 'a {
    move |t: u64| {
        spans
            .iter()
            .find(|s| s.start <= t && t < s.start + s.len)
            .and_then(|s| s.chord)
            .map(|i| chords[i].clone())
    }
}

/// "E5:q D5:e r:e C5:h." の形の動機を読む(長さ: w 全 / h 2 分 / q 4 分 / e 8 分 / s 16 分 / t 32 分、
/// 後ろの . で付点、3 で 3 連。数字だけなら tick。r は休み)
pub fn parse_motif(s: &str) -> Result<Vec<MotifNote>, String> {
    let mut out = Vec::new();
    let mut t = 0u64;
    for tok in s.split_whitespace() {
        let (name, len) = tok
            .split_once(':')
            .ok_or_else(|| format!("動機の書き方は 音名:長さ(例 E5:q)。読めない: {tok}"))?;
        let dur = if let Ok(n) = len.parse::<u64>() {
            n
        } else {
            let mut chars = len.chars();
            let base = match chars.next() {
                Some('w') => 3840,
                Some('h') => 1920,
                Some('q') => 960,
                Some('e') => 480,
                Some('s') => 240,
                Some('t') => 120,
                _ => return Err(format!("長さが読めません: {tok}")),
            };
            let mut d = base as f64;
            for c in chars {
                match c {
                    '.' => d *= 1.5,
                    '3' => d = d * 2.0 / 3.0,
                    _ => return Err(format!("長さが読めません: {tok}")),
                }
            }
            d.round() as u64
        };
        if dur == 0 || dur > 3840 * 4 {
            return Err(format!("長さが範囲の外です: {tok}"));
        }
        if !name.eq_ignore_ascii_case("r") {
            let pitch =
                crate::chord::parse_note(name).ok_or_else(|| format!("音名が読めません: {tok}"))?;
            out.push(MotifNote {
                offset: t,
                dur,
                pitch,
            });
        }
        t += dur;
    }
    if out.is_empty() {
        return Err("動機に音がありません".to_owned());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chord::parse;

    const BAR: u64 = 3840;

    fn chords_c_f_g_c() -> (Vec<Span>, Vec<Chord>) {
        let names = ["C", "F", "G", "C", "Am", "F", "G", "C"];
        let chords: Vec<Chord> = names.iter().map(|n| parse(n).unwrap().unwrap()).collect();
        let spans = (0..names.len())
            .map(|i| Span {
                start: i as u64 * BAR,
                len: BAR,
                chord: Some(i),
            })
            .collect();
        (spans, chords)
    }

    fn opts() -> Options {
        Options {
            key: Key::parse("C major").unwrap(),
            low: 55,
            high: 79,
            peak_slot: None,
            peak_pitch: None,
            anticipate: 0.0,
            seed: 1,
        }
    }

    #[test]
    fn motif_strings_and_plans_parse() {
        let m = parse_motif("E5:q D5:e C5:e r:q G4:h.").unwrap();
        assert_eq!(m.len(), 4);
        assert_eq!((m[3].offset, m[3].dur, m[3].pitch), (2880, 2880, 67));
        assert_eq!(parse_motif("C4:e3").unwrap()[0].dur, 320);
        assert!(parse_motif("C4").is_err());
        assert!(parse_motif("X4:q").is_err());
        assert_eq!(parse_plan("sentence").unwrap().len(), 4);
        let p = parse_plan("a | adapt | frag seq(-1) | cadence").unwrap();
        assert_eq!(p[2], vec![Op::Frag, Op::Seq(-1)]);
        assert!(parse_plan("a | jump").is_err());
    }

    #[test]
    fn sentence_adapts_to_chords_places_one_peak_and_closes_on_the_tonic() {
        let (spans, chords) = chords_c_f_g_c();
        let look = chord_lookup(&spans, &chords);
        // 2 小節の動機: E D C E | G(伸ばし)
        let m = parse_motif("E5:q D5:e C5:e E5:q G5:q G5:w").unwrap();
        let plan = form("sentence").unwrap();
        let out = develop(&m, &plan, BAR, &look, &opts()).unwrap();
        // 4 スロット × 2 小節
        assert!(out.iter().all(|o| o.pos < 8 * BAR));
        // 強拍(小節の頭と半ば)の音は和音の音
        for o in out.iter().filter(|o| o.pos >= 2 * BAR) {
            if o.pos % (BAR / 2) == 0 {
                let c = look(o.pos).unwrap();
                assert!(
                    c.pitch_classes().contains(&(o.pitch % 12)),
                    "{o:?} {}",
                    c.name
                );
            }
        }
        // 最高音は 1 回だけで、全体の 60〜75%(3 番目のスロット = 5〜6 小節目)
        let hi = out.iter().map(|o| o.pitch).max().unwrap();
        let peaks: Vec<&Out> = out.iter().filter(|o| o.pitch == hi).collect();
        assert_eq!(peaks.len(), 1, "{out:?}");
        assert_eq!(peaks[0].pos / (2 * BAR), 2);
        // 最後は主音 C で、長く伸ばす
        let last = out.last().unwrap();
        assert_eq!(last.pitch % 12, 0);
        assert!(last.dur >= 960 * 3, "{last:?}");
        // 音域の中
        assert!(out.iter().all(|o| (55..=79).contains(&o.pitch)));
        // 同じ入力なら同じ結果
        assert_eq!(out, develop(&m, &plan, BAR, &look, &opts()).unwrap());
    }

    #[test]
    fn period_opens_then_closes_and_anticipation_moves_strong_beats() {
        let (spans, chords) = chords_c_f_g_c();
        let look = chord_lookup(&spans, &chords);
        let m = parse_motif("C5:q D5:q E5:q D5:q").unwrap();
        let plan = form("period").unwrap();
        let out = develop(&m, &plan, BAR, &look, &opts()).unwrap();
        // 2 番目のスロット(2 小節目)の最後の音は 2 度か 5 度(D か G)で開く
        let q_end = out.iter().rfind(|o| o.pos / BAR == 1).unwrap();
        assert!([2, 7].contains(&(q_end.pitch % 12)), "{q_end:?}");
        // 答えは主音
        assert_eq!(out.last().unwrap().pitch % 12, 0);
        // 食い: 強拍の音のいくつかが 8 分前へ
        let mut o = opts();
        o.anticipate = 1.0;
        let a = develop(&m, &plan, BAR, &look, &o).unwrap();
        assert!(a.iter().any(|x| x.pos % 960 == 480), "{a:?}");
        // 重ならない
        assert!(a.windows(2).all(|w| w[0].pos + w[0].dur <= w[1].pos));
    }

    #[test]
    fn rhythm_ops_change_the_rhythm_but_keep_the_contour() {
        let (spans, chords) = chords_c_f_g_c();
        let look = chord_lookup(&spans, &chords);
        let m = parse_motif("E5:q D5:q C5:q D5:q").unwrap();
        let onsets = |plan: &str| -> Vec<u64> {
            develop(&m, &parse_plan(plan).unwrap(), BAR, &look, &opts())
                .unwrap()
                .iter()
                .map(|o| o.pos)
                .collect()
        };
        // 変奏: 同じ seed なら同じ、元とはリズムが違う。いくつかの seed で必ず変わる
        let base = onsets("a");
        let mut changed = 0;
        for seed in 1..6 {
            let mut o = opts();
            o.seed = seed;
            let v: Vec<u64> = develop(&m, &parse_plan("vary").unwrap(), BAR, &look, &o)
                .unwrap()
                .iter()
                .map(|x| x.pos)
                .collect();
            changed += (v != base) as usize;
        }
        assert_eq!(changed, 5);
        // 拡大: 前半 2 音を倍に(0 と 2 拍目)
        assert_eq!(onsets("augment"), vec![0, 1920]);
        // 縮小: 8 分で 4 音を 2 回
        assert_eq!(
            onsets("diminish"),
            (0..8).map(|k| k * 480).collect::<Vec<_>>()
        );
        // ずらし: 8 分遅れて入る(最後の音は小節からはみ出して落ちる)
        assert_eq!(onsets("displace(1)"), vec![480, 1440, 2400, 3360]);
        let d = develop(&m, &parse_plan("displace(1)").unwrap(), BAR, &look, &opts()).unwrap();
        assert!(d.iter().all(|x| x.pos + x.dur <= BAR));
        // 形式の型にも入っている(sentence の 2 回目はリズムが変わる)
        let s = develop(&m, &form("sentence").unwrap(), BAR, &look, &opts()).unwrap();
        let bar = |b: u64| -> Vec<u64> {
            s.iter()
                .filter(|x| x.pos / BAR == b)
                .map(|x| x.pos % BAR)
                .collect()
        };
        assert_ne!(bar(0), bar(1));
        assert!(parse_plan("displace(x)").is_err());
    }

    #[test]
    fn fill_reverses_after_a_leap_and_range_is_kept() {
        let (spans, chords) = chords_c_f_g_c();
        let look = chord_lookup(&spans, &chords);
        let m = parse_motif("C4:q A4:q B4:q C5:q").unwrap();
        let plan = parse_plan("fill").unwrap();
        let out = develop(&m, &plan, BAR, &look, &opts()).unwrap();
        // C4 → A4(9 半音上)の次は下へ戻る
        assert!(out[2].pitch < out[1].pitch, "{out:?}");
        // 高い音域を指定すると 1 オクターブ上へ
        let mut o = opts();
        o.low = 72;
        o.high = 96;
        let hi = develop(&m, &plan, BAR, &look, &o).unwrap();
        assert!(hi.iter().all(|x| (72..=96).contains(&x.pitch)), "{hi:?}");
    }
}
