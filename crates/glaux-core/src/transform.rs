//! 旋律の変形(MCP の `transform_notes` の中身)。動機を展開して曲に統一感を出す、作曲の基本の技法を正確に行う。
//!
//! - 反行(invert): 軸の音を中心に上下を反転する
//! - 逆行(retrograde): 選んだ範囲の中で時間を逆にする
//! - 音階に沿った移調(transpose_diatonic): 音階の度数で動かす(C メジャーの 2 度上なら E→F、F→G)
//! - 反復進行(sequence): 選んだ音を、少し後ろへ・度数をずらして何回か写す(新しいノートを作る)
//! - 拡大・縮小(stretch): 範囲の頭を中心に、位置と長さを何倍かにする
//!
//! 音階の外の音(臨時記号)は、すぐ下の音階の音からの半音のずれとして保つ。音階を与えなければ半音で数える。

use crate::id::NoteId;
use crate::model::Note;
use crate::time::Tick;

/// 変形の種類
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Invert {
        axis: Option<u8>,
    },
    Retrograde,
    TransposeDiatonic {
        steps: i32,
    },
    Sequence {
        steps: i32,
        times: u32,
        offset: Option<u64>,
    },
    Stretch {
        factor: f64,
    },
}

/// 変形の結果: 既存のノートの変更と、新しく足すノート
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Result {
    pub changes: Vec<(NoteId, u64, u64, u8)>,
    pub added: Vec<Note>,
}

/// 音階(ピッチクラスの昇順の並び)。空なら半音で数える
#[derive(Clone, Debug)]
pub struct Scale(Vec<u8>);

impl Scale {
    pub fn new(mut pcs: Vec<u8>) -> Scale {
        pcs.iter_mut().for_each(|p| *p %= 12);
        pcs.sort_unstable();
        pcs.dedup();
        Scale(pcs)
    }

    pub fn chromatic() -> Scale {
        Scale((0..12).collect())
    }

    /// 音 → (度数の通し番号, 音階の音からの半音のずれ)
    fn degree(&self, pitch: i32) -> (i32, i32) {
        let n = self.0.len() as i32;
        let oct = pitch.div_euclid(12);
        let pc = pitch.rem_euclid(12) as u8;
        // すぐ下(か同じ)の音階の音
        let (idx, below) = self
            .0
            .iter()
            .enumerate()
            .rev()
            .find(|(_, &p)| p <= pc)
            .map(|(i, &p)| (i as i32, p as i32))
            .unwrap_or((n - 1, self.0[n as usize - 1] as i32 - 12));
        (oct * n + idx, pc as i32 - below)
    }

    fn pitch(&self, degree: i32, accidental: i32) -> i32 {
        let n = self.0.len() as i32;
        let oct = degree.div_euclid(n);
        self.0[degree.rem_euclid(n) as usize] as i32 + 12 * oct + accidental
    }

    /// 度数で `steps` 動かした音
    pub fn shift(&self, pitch: u8, steps: i32) -> i32 {
        let (d, acc) = self.degree(pitch as i32);
        self.pitch(d + steps, acc)
    }

    /// `axis` を中心に反転した音
    pub fn invert(&self, pitch: u8, axis: u8) -> i32 {
        let (d, acc) = self.degree(pitch as i32);
        let (a, _) = self.degree(axis as i32);
        self.pitch(2 * a - d, -acc)
    }
}

fn clamp_pitch(p: i32) -> std::result::Result<u8, String> {
    if (0..=127).contains(&p) {
        Ok(p as u8)
    } else {
        Err(format!(
            "音程が 0〜127 の外になります({p})。移調の量を減らしてください"
        ))
    }
}

/// 選んだノート(`selected`)を変形する。`clip_len` を越える位置はエラー(クリップを伸ばしてから)。
/// `new_id` は新しいノートの ID を作る関数(コマンドを作る側が振る)
pub fn transform(
    selected: &[Note],
    clip_len: u64,
    op: &Op,
    scale: &Scale,
    new_id: &mut dyn FnMut() -> NoteId,
) -> std::result::Result<Result, String> {
    if selected.is_empty() {
        return Err("変形するノートがありません".to_owned());
    }
    let start = selected.iter().map(|n| n.pos.0).min().unwrap_or(0);
    let end = selected
        .iter()
        .map(|n| n.pos.0 + n.dur.0)
        .max()
        .unwrap_or(0);
    let check_end = |pos: u64, dur: u64| {
        if pos + dur > clip_len {
            Err(format!(
                "クリップの長さ({clip_len} tick)を越えます。resize_clip で伸ばしてから掛けてください"
            ))
        } else {
            Ok(())
        }
    };
    let mut out = Result::default();
    match op {
        Op::Invert { axis } => {
            // 軸の既定は最初の音(時間順)
            let axis = axis.unwrap_or_else(|| {
                selected
                    .iter()
                    .min_by_key(|n| (n.pos, n.pitch))
                    .map_or(60, |n| n.pitch)
            });
            for n in selected {
                let p = clamp_pitch(scale.invert(n.pitch, axis))?;
                out.changes.push((n.id.clone(), n.pos.0, n.dur.0, p));
            }
        }
        Op::Retrograde => {
            for n in selected {
                let pos = start + end - (n.pos.0 + n.dur.0);
                out.changes.push((n.id.clone(), pos, n.dur.0, n.pitch));
            }
        }
        Op::TransposeDiatonic { steps } => {
            for n in selected {
                let p = clamp_pitch(scale.shift(n.pitch, *steps))?;
                out.changes.push((n.id.clone(), n.pos.0, n.dur.0, p));
            }
        }
        Op::Stretch { factor } => {
            if !(0.125..=8.0).contains(factor) {
                return Err("factor は 0.125〜8".to_owned());
            }
            for n in selected {
                let pos = start + ((n.pos.0 - start) as f64 * factor).round() as u64;
                let dur = ((n.dur.0 as f64 * factor).round() as u64).max(1);
                check_end(pos, dur)?;
                out.changes.push((n.id.clone(), pos, dur, n.pitch));
            }
        }
        Op::Sequence {
            steps,
            times,
            offset,
        } => {
            if *times == 0 || *times > 16 {
                return Err("times は 1〜16".to_owned());
            }
            let shift = offset.unwrap_or(end - start).max(1);
            for k in 1..=*times as u64 {
                for n in selected {
                    let pos = n.pos.0 + shift * k;
                    check_end(pos, n.dur.0)?;
                    let p = clamp_pitch(scale.shift(n.pitch, steps * k as i32))?;
                    let mut m = n.clone();
                    m.id = new_id();
                    m.pos = Tick(pos);
                    m.pitch = p;
                    out.added.push(m);
                }
            }
        }
    }
    // 変わらないものは除く
    out.changes.retain(|(id, pos, dur, pitch)| {
        selected
            .iter()
            .find(|n| &n.id == id)
            .is_some_and(|n| n.pos.0 != *pos || n.dur.0 != *dur || n.pitch != *pitch)
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Articulation;

    fn n(i: usize, pos: u64, pitch: u8) -> Note {
        Note {
            id: NoteId::parse(&format!("nt_t{i:05}")).unwrap(),
            pos: Tick(pos),
            dur: Tick(480),
            pitch,
            vel: 100,
            articulation: Articulation::Normal,
            pitch_curve: vec![],
            glide_ms: None,
            vibrato: None,
            volume_curve: vec![],
            brightness_curve: vec![],
            condition: None,
        }
    }

    fn c_major() -> Scale {
        Scale::new(crate::harmony::scale_pitch_classes(0, "major"))
    }

    #[test]
    fn diatonic_shift_keeps_the_scale_and_accidentals() {
        let s = c_major();
        assert_eq!(s.shift(64, 1), 65); // E → F
        assert_eq!(s.shift(65, 1), 67); // F → G
        assert_eq!(s.shift(71, 1), 72); // B → C(オクターブをまたぐ)
        assert_eq!(s.shift(60, -1), 59); // C → B
        assert_eq!(s.shift(60, 7), 72); // 7 度 = 1 オクターブ
        assert_eq!(s.shift(61, 2), 65); // C# → E + 1 = F(臨時記号を保つ)
                                        // 半音で数える音階
        assert_eq!(Scale::chromatic().shift(60, 3), 63);
    }

    #[test]
    fn inversion_mirrors_around_the_axis_in_the_scale() {
        let s = c_major();
        // C を軸に: E(3 度上)→ A(3 度下)、G → F
        assert_eq!(s.invert(64, 60), 57);
        assert_eq!(s.invert(67, 60), 53);
        assert_eq!(s.invert(60, 60), 60);
    }

    #[test]
    fn ops_on_a_motif() {
        // 動機: C D E(4 分)
        let m = vec![n(0, 0, 60), n(1, 480, 62), n(2, 960, 64)];
        let mut k = 0;
        let mut id = || {
            k += 1;
            NoteId::parse(&format!("nt_new{k:03}")).unwrap()
        };
        // 逆行: E D C の順
        let r = transform(&m, 7680, &Op::Retrograde, &c_major(), &mut id).unwrap();
        let pos_of =
            |r: &Result, i: &str| r.changes.iter().find(|c| c.0.as_str() == i).map(|c| c.1);
        assert_eq!(pos_of(&r, "nt_t00000"), Some(960));
        assert_eq!(pos_of(&r, "nt_t00002"), Some(0));
        // 反行(軸 = 最初の音 C): C B A
        let r = transform(&m, 7680, &Op::Invert { axis: None }, &c_major(), &mut id).unwrap();
        let pitches: Vec<u8> = r.changes.iter().map(|c| c.3).collect();
        assert_eq!(pitches, vec![59, 57]);
        // 反復進行: 2 度上に 2 回(動機の長さずつ後ろへ)→ D E F、E F G
        let r = transform(
            &m,
            7680,
            &Op::Sequence {
                steps: 1,
                times: 2,
                offset: None,
            },
            &c_major(),
            &mut id,
        )
        .unwrap();
        let added: Vec<(u64, u8)> = r.added.iter().map(|n| (n.pos.0, n.pitch)).collect();
        assert_eq!(
            added,
            vec![
                (1440, 62),
                (1920, 64),
                (2400, 65),
                (2880, 64),
                (3360, 65),
                (3840, 67)
            ]
        );
        assert!(r.changes.is_empty());
        // 拡大(2 倍)
        let r = transform(&m, 7680, &Op::Stretch { factor: 2.0 }, &c_major(), &mut id).unwrap();
        assert!(r
            .changes
            .iter()
            .any(|c| c.0.as_str() == "nt_t00002" && c.1 == 1920 && c.2 == 960));
        // クリップを越えるならエラー
        assert!(transform(&m, 1200, &Op::Stretch { factor: 2.0 }, &c_major(), &mut id).is_err());
        // 音域の外もエラー
        assert!(transform(
            &[n(0, 0, 125)],
            7680,
            &Op::TransposeDiatonic { steps: 5 },
            &c_major(),
            &mut id
        )
        .is_err());
    }
}
