//! 装飾音(MCP の add_ornament の中身)と、旋律を先に聞かせるずらし(melody_lead)。
//!
//! 数値は研究の目安: 短前打音は 8 分の約 20%(45〜125ms に収める。Windsor ら)、トリルは 1 秒に約 10 音
//! (8〜12)。短前打音は前の音の区間に置き(前の音を短くする)、長前打音は本音の半分を取る。
//! 装飾の音は音階の隣の音(`Scale`)か、半音の数で決める。新しいノートの ID は呼び出し側が渡す関数で作る。

use crate::model::Note;
use crate::time::Tick;
use crate::transform::Scale;

/// 装飾の種類
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ornament {
    /// 短前打音(前に短く 1 音。前の音の区間から取る)
    Acciaccatura,
    /// 長前打音(本音の頭の半分を上の隣の音で。倚音)
    Appoggiatura,
    /// モルデント(本音 → 下の隣 → 本音を素早く)
    Mordent,
    /// 上のモルデント(本音 → 上の隣 → 本音)
    InvertedMordent,
    /// ターン(上 → 本音 → 下 → 本音)
    Turn,
    /// トリル(本音と上の隣を音の長さいっぱいに)
    Trill,
    /// シュライファー(下から 2 音で滑り込む)
    Schleifer,
}

impl Ornament {
    pub fn parse(s: &str) -> Option<Ornament> {
        Some(match s.trim().to_lowercase().as_str() {
            "acciaccatura" | "grace" => Ornament::Acciaccatura,
            "appoggiatura" => Ornament::Appoggiatura,
            "mordent" => Ornament::Mordent,
            "inverted_mordent" | "upper_mordent" => Ornament::InvertedMordent,
            "turn" => Ornament::Turn,
            "trill" => Ornament::Trill,
            "schleifer" | "slide" => Ornament::Schleifer,
            _ => return None,
        })
    }
}

/// 装飾の条件
pub struct OrnamentOptions {
    /// 装飾の音 1 つの長さ(tick)
    pub grace: u64,
    /// トリルの音 1 つの長さ(tick)
    pub trill_step: u64,
    /// トリルを上の隣から始めるか(バロック)。false なら本音から
    pub trill_from_upper: bool,
    /// トリルの終わりをターン(下の隣 → 本音)で閉じるか
    pub trill_turn_end: bool,
    /// 装飾の音の強さの割合
    pub vel_ratio: f64,
    /// 隣の音の半音の数(None なら音階の隣)
    pub interval: Option<i32>,
}

/// 1 音の装飾の結果: 本音の新しい位置・長さ、足す音 (位置, 長さ, 音, 強さ)、前の音の新しい長さ
#[derive(Clone, Debug, PartialEq)]
pub struct Decorated {
    pub main_pos: u64,
    pub main_dur: u64,
    pub added: Vec<(u64, u64, u8, u8)>,
    /// 短前打音で前の音を短くするとき (前の音の番号, 新しい長さ)
    pub shorten_prev: Option<(usize, u64)>,
}

fn neighbor(p: u8, dir: i32, scale: &Scale, interval: Option<i32>) -> u8 {
    match interval {
        Some(n) => (p as i32 + dir * n.abs().max(1)).clamp(0, 127) as u8,
        None => scale.shift(p, dir).clamp(0, 127) as u8,
    }
}

/// `notes[i]` を装飾する。`prev` は旋律で直前の音の番号。装飾できない(音が短すぎる)なら None
pub fn decorate(
    notes: &[Note],
    i: usize,
    prev: Option<usize>,
    kind: Ornament,
    scale: &Scale,
    o: &OrnamentOptions,
) -> Option<Decorated> {
    let n = &notes[i];
    let (pos, dur, p) = (n.pos.0, n.dur.0, n.pitch);
    let g = o.grace.max(1);
    let gv = ((n.vel as f64 * o.vel_ratio).round() as u8).clamp(1, 127);
    let up = neighbor(p, 1, scale, o.interval);
    let down = neighbor(p, -1, scale, o.interval);
    let keep = |main_pos: u64, main_dur: u64, added: Vec<(u64, u64, u8, u8)>| {
        Some(Decorated {
            main_pos,
            main_dur,
            added,
            shorten_prev: None,
        })
    };
    match kind {
        Ornament::Acciaccatura | Ornament::Schleifer => {
            let count = if kind == Ornament::Schleifer { 2 } else { 1 };
            let need = g * count;
            let pitches: Vec<u8> = if kind == Ornament::Schleifer {
                vec![neighbor(down, -1, scale, o.interval), down]
            } else {
                vec![up]
            };
            // 前に置く場所が無ければ(クリップの頭)、本音の頭から取る
            if pos < need {
                if dur < need * 2 {
                    return None;
                }
                let added = pitches
                    .iter()
                    .enumerate()
                    .map(|(k, &q)| (pos + k as u64 * g, g, q, gv))
                    .collect();
                return keep(pos + need, dur - need, added);
            }
            let start = pos - need;
            let added = pitches
                .iter()
                .enumerate()
                .map(|(k, &q)| (start + k as u64 * g, g, q, gv))
                .collect();
            // 前の音が装飾にかかるなら短くする(短くしすぎるなら装飾しない)
            let shorten_prev = match prev.map(|j| (j, &notes[j])) {
                Some((j, pn)) if pn.pos.0 + pn.dur.0 > start => {
                    if start <= pn.pos.0 + g / 2 {
                        return None;
                    }
                    Some((j, start - pn.pos.0))
                }
                _ => None,
            };
            Some(Decorated {
                main_pos: pos,
                main_dur: dur,
                added,
                shorten_prev,
            })
        }
        Ornament::Appoggiatura => {
            if dur < g * 2 {
                return None;
            }
            let half = dur / 2;
            keep(pos + half, dur - half, vec![(pos, half, up, n.vel)])
        }
        Ornament::Mordent | Ornament::InvertedMordent => {
            if dur < g * 4 {
                return None;
            }
            let nb = if kind == Ornament::Mordent { down } else { up };
            keep(
                pos + 2 * g,
                dur - 2 * g,
                vec![(pos, g, p, n.vel), (pos + g, g, nb, gv)],
            )
        }
        Ornament::Turn => {
            if dur < g * 5 {
                return None;
            }
            keep(
                pos + 3 * g,
                dur - 3 * g,
                vec![
                    (pos, g, up, n.vel),
                    (pos + g, g, p, gv),
                    (pos + 2 * g, g, down, gv),
                ],
            )
        }
        Ornament::Trill => {
            let step = o.trill_step.max(1);
            let count = (dur / step) as usize;
            if count < 3 {
                return None;
            }
            // 丸めの誤差は各音に振り分けて合計の長さを保つ
            let at = |k: usize| pos + (k as u64 * dur) / count as u64;
            let mut seq: Vec<u8> = (0..count)
                .map(|k| {
                    let upper_first = o.trill_from_upper;
                    if (k % 2 == 0) == upper_first {
                        up
                    } else {
                        p
                    }
                })
                .collect();
            if o.trill_turn_end && count >= 4 {
                seq[count - 2] = down;
                seq[count - 1] = p;
            }
            // 最初の音は本音のノートを使う(ID を保つ)。本音は 1 つ目の長さに縮め、残りを足す
            let main_first = seq[0] == p;
            let mut added = Vec::new();
            for (k, &q) in seq.iter().enumerate() {
                if k == 0 && main_first {
                    continue;
                }
                let v = if q == p { n.vel } else { gv };
                added.push((at(k), at(k + 1) - at(k), q, v));
            }
            if main_first {
                keep(pos, at(1) - pos, added)
            } else {
                // 上から始めるときは、最後の本音を本音のノートにする
                let last = added.pop().expect("3 音以上");
                keep(last.0, last.1, added)
            }
        }
    }
}

/// 旋律を先に聞かせるずらし(tick)。`notes` の中で同じ位置(±`tol`)に 2 音以上ある所は一番上の音だけ、
/// 1 音だけの所(単音の旋律)はその音を `lead` だけ前へ。`by_velocity` なら上の音と他の音の強さの差に比例
/// (差 20 で `lead`、最大 2 倍。Goebl のハンマーの遅れの再現)。戻り値は (音の番号, 新しい位置)
pub fn melody_lead(notes: &[Note], lead: u64, by_velocity: bool, tol: u64) -> Vec<(usize, u64)> {
    let mut order: Vec<usize> = (0..notes.len()).collect();
    order.sort_by_key(|&i| (notes[i].pos, notes[i].pitch));
    let mut out = Vec::new();
    let mut k = 0;
    while k < order.len() {
        let head = notes[order[k]].pos.0;
        let mut j = k;
        while j < order.len() && notes[order[j]].pos.0 <= head + tol {
            j += 1;
        }
        let group = &order[k..j];
        let top = *group
            .iter()
            .max_by_key(|&&i| (notes[i].pitch, notes[i].vel))
            .expect("1 つ以上");
        let amount = if by_velocity && group.len() > 1 {
            let others: Vec<f64> = group
                .iter()
                .filter(|&&i| i != top)
                .map(|&i| notes[i].vel as f64)
                .collect();
            let mean = others.iter().sum::<f64>() / others.len() as f64;
            let diff = notes[top].vel as f64 - mean;
            (lead as f64 * (diff / 20.0).clamp(0.0, 2.0)).round() as u64
        } else {
            lead
        };
        let pos = notes[top].pos.0;
        if amount > 0 && pos > 0 {
            out.push((top, pos.saturating_sub(amount)));
        }
        k = j;
    }
    out
}

/// 旋律の並び(同じ位置は一番上の音)で、各音の直前の音の番号
pub fn previous_in_line(notes: &[Note]) -> Vec<Option<usize>> {
    let mut order: Vec<usize> = (0..notes.len()).collect();
    order.sort_by_key(|&i| (notes[i].pos, std::cmp::Reverse(notes[i].pitch)));
    let mut prev = vec![None; notes.len()];
    let mut last: Option<usize> = None;
    let mut last_pos: Option<Tick> = None;
    for &i in &order {
        if last_pos == Some(notes[i].pos) {
            // 和音の下の音は、旋律の直前の音を持たない
            continue;
        }
        prev[i] = last;
        last = Some(i);
        last_pos = Some(notes[i].pos);
    }
    prev
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::NoteId;

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
        }
    }

    fn c_major() -> Scale {
        Scale::new(crate::harmony::scale_pitch_classes(0, "major"))
    }

    fn opts() -> OrnamentOptions {
        OrnamentOptions {
            grace: 96,
            trill_step: 96,
            trill_from_upper: false,
            trill_turn_end: false,
            vel_ratio: 0.7,
            interval: None,
        }
    }

    #[test]
    fn grace_notes_take_time_from_the_previous_note() {
        // C4(1 拍)→ E4 に短前打音: F4 が E4 の 96 前、C4 は短くなる
        let ns = vec![note(0, 960, 60, 90), note(960, 960, 64, 90)];
        let d = decorate(&ns, 1, Some(0), Ornament::Acciaccatura, &c_major(), &opts()).unwrap();
        assert_eq!(d.added, vec![(864, 96, 65, 63)]);
        assert_eq!(d.shorten_prev, Some((0, 864)));
        assert_eq!((d.main_pos, d.main_dur), (960, 960));
        // クリップの頭では本音から取る
        let d = decorate(&ns, 0, None, Ornament::Acciaccatura, &c_major(), &opts()).unwrap();
        assert_eq!(d.main_pos, 96);
        // シュライファーは下から 2 音(C 長調の E4 → C4 D4)
        let d = decorate(&ns, 1, Some(0), Ornament::Schleifer, &c_major(), &opts()).unwrap();
        let ps: Vec<u8> = d.added.iter().map(|a| a.2).collect();
        assert_eq!(ps, vec![60, 62]);
    }

    #[test]
    fn mordents_turns_and_trills() {
        let ns = vec![note(0, 960, 64, 90)];
        let s = c_major();
        let m = decorate(&ns, 0, None, Ornament::Mordent, &s, &opts()).unwrap();
        assert_eq!(m.added, vec![(0, 96, 64, 90), (96, 96, 62, 63)]);
        assert_eq!((m.main_pos, m.main_dur), (192, 768));
        let t = decorate(&ns, 0, None, Ornament::Turn, &s, &opts()).unwrap();
        let ps: Vec<u8> = t.added.iter().map(|a| a.2).collect();
        assert_eq!(ps, vec![65, 64, 62]);
        // トリル: 960 を 96 ずつ = 10 音、本音 → 上 → 本音 …。合計の長さは元と同じ
        let tr = decorate(&ns, 0, None, Ornament::Trill, &s, &opts()).unwrap();
        assert_eq!(tr.main_pos, 0);
        assert_eq!(tr.added.len(), 9);
        let end = tr.added.last().map(|a| a.0 + a.1).unwrap();
        assert_eq!(end, 960);
        assert_eq!(tr.added[0].2, 65);
        // 半音で決める
        let mut o = opts();
        o.interval = Some(1);
        let m = decorate(&ns, 0, None, Ornament::InvertedMordent, &s, &o).unwrap();
        assert_eq!(m.added[1].2, 65);
        // 短すぎる音は装飾しない
        let short = vec![note(0, 200, 64, 90)];
        assert!(decorate(&short, 0, None, Ornament::Turn, &s, &opts()).is_none());
        assert_eq!(Ornament::parse("trill"), Some(Ornament::Trill));
    }

    #[test]
    fn melody_lead_moves_the_top_voice() {
        // 和音(上の音が強い)と、単音
        let ns = vec![
            note(960, 960, 60, 60),
            note(960, 960, 64, 60),
            note(960, 960, 72, 100),
            note(1920, 480, 74, 90),
        ];
        let m = melody_lead(&ns, 20, false, 10);
        assert_eq!(m, vec![(2, 940), (3, 1900)]);
        let v = melody_lead(&ns, 20, true, 10);
        // 強さの差 40 → 2 倍
        assert_eq!(v[0], (2, 920));
        assert_eq!(previous_in_line(&ns)[3], Some(2));
    }
}
