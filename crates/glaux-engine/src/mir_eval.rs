//! 曲の解析(拍・小節頭・テンポ・調・和音・チューニング)の当たり具合の指標。
//!
//! 曲から起こす機能の評価に使う。定義は研究で広く使われる mir_eval(Raffel ら、ISMIR 2014)と MIREX に合わせ、
//! 論文の数字と比べられるようにする:
//!
//! - 拍・小節頭: 正解の ±70ms 以内を当たりとした F 値。冒頭 5 秒は数えない(mir_eval の `trim_beats` と同じ)
//! - テンポ: 正解の ±4% 以内なら Acc1、2 倍・半分・3 倍・1/3 も許すなら Acc2
//! - 調: MIREX の重み(同じ 1.0、5 度上下 0.5、平行調 0.3、同主調 0.2、ほか 0)
//! - 和音: 時間で重みを付けた一致率(WCSR)。根音 / 長短(majmin)/ 長短 + 最低音(majmin_inv)/ 四和音まで(sevenths)。
//!   正解がその語彙で書けない区間(例: majmin で sus4)は分母から除く。正解が無音(N)の区間は「N と答えたら当たり」
//!
//! どれも純粋な計算で、音声には触れない。

use glaux_core::chord;

/// 拍の当たりとみなす幅(秒)
pub const BEAT_WINDOW: f64 = 0.07;
/// 拍の評価で数えない冒頭(秒)
pub const BEAT_TRIM: f64 = 5.0;

/// 当たりの数から出す F 値などの組
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize)]
pub struct Fmeasure {
    pub f: f64,
    pub precision: f64,
    pub recall: f64,
}

impl Fmeasure {
    fn from_counts(hits: usize, n_est: usize, n_ref: usize) -> Self {
        if n_est == 0 || n_ref == 0 {
            // 両方空なら当たり、片方だけ空なら外れ
            let v = if n_est == n_ref { 1.0 } else { 0.0 };
            return Fmeasure {
                f: v,
                precision: v,
                recall: v,
            };
        }
        let precision = hits as f64 / n_est as f64;
        let recall = hits as f64 / n_ref as f64;
        let f = if precision + recall > 0.0 {
            2.0 * precision * recall / (precision + recall)
        } else {
            0.0
        };
        Fmeasure {
            f,
            precision,
            recall,
        }
    }
}

/// 2 つの時刻の列(どちらも昇順)を ±`window` 秒で 1 対 1 に組にした数。
/// 1 次元で幅が左右対称なので、左から順に組む貪欲法で最大の組数になる
pub fn match_events(reference: &[f64], estimate: &[f64], window: f64) -> usize {
    let (mut i, mut j, mut hits) = (0, 0, 0);
    while i < reference.len() && j < estimate.len() {
        let d = estimate[j] - reference[i];
        if d.abs() <= window {
            hits += 1;
            i += 1;
            j += 1;
        } else if d < 0.0 {
            j += 1;
        } else {
            i += 1;
        }
    }
    hits
}

/// 拍(または小節頭)の F 値。冒頭 [`BEAT_TRIM`] 秒は数えない
pub fn beat_f(reference: &[f64], estimate: &[f64]) -> Fmeasure {
    let trim = |v: &[f64]| -> Vec<f64> {
        let mut v: Vec<f64> = v.iter().copied().filter(|&t| t >= BEAT_TRIM).collect();
        v.sort_by(f64::total_cmp);
        v
    };
    let (r, e) = (trim(reference), trim(estimate));
    Fmeasure::from_counts(match_events(&r, &e, BEAT_WINDOW), e.len(), r.len())
}

/// テンポの当たり。(Acc1: ±4% 以内, Acc2: 2 倍・半分・3 倍・1/3 も許す)
pub fn tempo_accuracy(reference_bpm: f64, estimate_bpm: f64) -> (bool, bool) {
    if reference_bpm <= 0.0 || estimate_bpm <= 0.0 {
        return (false, false);
    }
    let near = |f: f64| (estimate_bpm - reference_bpm * f).abs() <= reference_bpm * f * 0.04;
    let acc1 = near(1.0);
    (
        acc1,
        acc1 || [2.0, 0.5, 3.0, 1.0 / 3.0].into_iter().any(near),
    )
}

/// 調の当たり(MIREX の重み)。`chord::Key` で渡す
pub fn key_score(reference: chord::Key, estimate: chord::Key) -> f64 {
    let up = (estimate.tonic + 12 - reference.tonic) % 12;
    if reference.minor == estimate.minor {
        match up {
            0 => 1.0,
            // 5 度上・下(属調・下属調)
            7 | 5 => 0.5,
            _ => 0.0,
        }
    } else if up == 0 {
        // 同主調(C と Cm)
        0.2
    } else if (!reference.minor && up == 9) || (reference.minor && up == 3) {
        // 平行調(C と Am)
        0.3
    } else {
        0.0
    }
}

/// 時刻つきの和音の並び 1 つ分(開始・終了の秒と記号。"N" / "N.C." は無音・和音なし)
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ChordSpan {
    pub start: f64,
    pub end: f64,
    pub symbol: String,
}

/// 和音を比べる細かさ
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChordVocab {
    /// 根音だけ
    Root,
    /// 根音と長短(三和音)。正解が長・短三和音とその 7th などに縮められない区間は除く
    MajMin,
    /// 長短と最低音(転回形)
    MajMinInv,
    /// 長・短・属七・長七・短七の 5 種まで(三和音は三和音のまま比べる)
    Sevenths,
}

/// 比べるために縮めた和音。`None` = 無音・和音なし(N)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reduced {
    N,
    Chord {
        root: u8,
        /// 0 = 長, 1 = 短, 2 = 属七, 3 = 長七, 4 = 短七, 255 = 比べない(根音だけ使う)
        quality: u8,
        bass: u8,
    },
}

fn reduce(symbol: &str, vocab: ChordVocab) -> Option<Reduced> {
    let s = symbol.trim();
    if s.eq_ignore_ascii_case("n") || s.is_empty() {
        return Some(Reduced::N);
    }
    let c = match chord::parse(s) {
        Ok(Some(c)) => c,
        Ok(None) => return Some(Reduced::N),
        Err(_) => return None,
    };
    let iv: Vec<u8> = c.tones.iter().map(|t| t.interval).collect();
    let has = |i: u8| iv.contains(&i);
    let triad = if has(4) && has(7) {
        Some(0)
    } else if has(3) && has(7) {
        Some(1)
    } else {
        None
    };
    let quality = match vocab {
        ChordVocab::Root => 255,
        ChordVocab::MajMin | ChordVocab::MajMinInv => triad?,
        ChordVocab::Sevenths => match (triad?, has(10), has(11)) {
            (0, true, false) => 2,
            (0, false, true) => 3,
            (1, true, false) => 4,
            (1, false, true) => return None,
            (t, false, false) => t,
            _ => return None,
        },
    };
    let bass = if vocab == ChordVocab::MajMinInv {
        c.bass_pc()
    } else {
        c.root
    };
    Some(Reduced::Chord {
        root: c.root,
        quality,
        bass,
    })
}

/// 時間で重みを付けた和音の一致率(0〜1)。比べられる区間が無ければ `None`。
/// 正解の区間の外の推定は数えない
pub fn chord_wcsr(
    reference: &[ChordSpan],
    estimate: &[ChordSpan],
    vocab: ChordVocab,
) -> Option<f64> {
    let (mut total, mut hit) = (0.0, 0.0);
    for r in reference {
        let Some(rr) = reduce(&r.symbol, vocab) else {
            continue;
        };
        let len = r.end - r.start;
        if len <= 0.0 {
            continue;
        }
        total += len;
        // 推定の区間と重なる長さのうち、同じとみなせる分
        let mut covered = 0.0;
        for e in estimate {
            let ov = r.end.min(e.end) - r.start.max(e.start);
            if ov <= 0.0 {
                continue;
            }
            let same = match (rr, reduce(&e.symbol, vocab)) {
                (Reduced::N, Some(Reduced::N)) => true,
                (
                    Reduced::Chord {
                        root,
                        quality,
                        bass,
                    },
                    Some(Reduced::Chord {
                        root: r2,
                        quality: q2,
                        bass: b2,
                    }),
                ) => root == r2 && (quality == 255 || quality == q2) && bass == b2,
                _ => false,
            };
            if same {
                covered += ov;
            }
        }
        hit += covered.min(len);
    }
    (total > 0.0).then(|| hit / total)
}

/// 音符 1 つ(秒と MIDI の音高)
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NoteSpan {
    pub onset: f64,
    pub offset: f64,
    pub pitch: u8,
}

/// 音符の当たりとみなす立ち上がりのずれ(秒)
pub const NOTE_ONSET_TOL: f64 = 0.05;

/// 和声の一致の F 値: 50ms ごとに、鳴っている音名(ピッチクラス)の集まりを比べる。適合率 = 推定の音名のうち正解にもある割合、
/// 再現率 = 正解の音名のうち推定にもある割合(どちらもフレームを通して数える)。リズム・音の高さ(オクターブ)・積み方が違っても、
/// 同じ和声を鳴らしていれば高くなる(和音から組み立てた伴奏と、音符を起こした結果を比べるため)
pub fn chroma_f(reference: &[NoteSpan], estimate: &[NoteSpan]) -> Fmeasure {
    const HOP: f64 = 0.05;
    let end = reference
        .iter()
        .chain(estimate)
        .map(|n| n.offset)
        .fold(0.0f64, f64::max);
    let n = (end / HOP).ceil() as usize + 1;
    let frames = |notes: &[NoteSpan]| -> Vec<u16> {
        let mut v = vec![0u16; n];
        for x in notes {
            let (a, b) = (
                (x.onset / HOP).ceil() as usize,
                (x.offset / HOP).ceil() as usize,
            );
            for f in v.iter_mut().take(b.min(n)).skip(a) {
                *f |= 1 << (x.pitch % 12);
            }
        }
        v
    };
    let (r, e) = (frames(reference), frames(estimate));
    let (mut hit, mut nr, mut ne) = (0u32, 0u32, 0u32);
    for (a, b) in r.iter().zip(&e) {
        hit += (a & b).count_ones();
        nr += a.count_ones();
        ne += b.count_ones();
    }
    let precision = if ne == 0 { 0.0 } else { hit as f64 / ne as f64 };
    let recall = if nr == 0 { 0.0 } else { hit as f64 / nr as f64 };
    let f = if precision + recall == 0.0 {
        0.0
    } else {
        2.0 * precision * recall / (precision + recall)
    };
    Fmeasure {
        precision,
        recall,
        f,
    }
}

/// 音符の F 値(mir_eval の transcription と同じ考え方)。立ち上がりが ±50ms、音高が同じなら当たり。
/// `with_offset` なら、終わりも「正解の長さの 20% か 50ms の大きい方」以内であること。
/// 音高はチューニングを直した後の MIDI 番号で比べる(曲の中の音として合っているか)
pub fn note_f(reference: &[NoteSpan], estimate: &[NoteSpan], with_offset: bool) -> Fmeasure {
    let mut r: Vec<&NoteSpan> = reference.iter().collect();
    r.sort_by(|a, b| a.onset.total_cmp(&b.onset));
    let mut e: Vec<&NoteSpan> = estimate.iter().collect();
    e.sort_by(|a, b| a.onset.total_cmp(&b.onset));
    let mut used = vec![false; e.len()];
    let mut hits = 0;
    let mut lo = 0;
    for rn in &r {
        while lo < e.len() && e[lo].onset < rn.onset - NOTE_ONSET_TOL {
            lo += 1;
        }
        let mut best: Option<(usize, f64)> = None;
        for (j, en) in e.iter().enumerate().skip(lo) {
            if en.onset > rn.onset + NOTE_ONSET_TOL {
                break;
            }
            if used[j] || en.pitch != rn.pitch {
                continue;
            }
            if with_offset {
                let tol = (0.2 * (rn.offset - rn.onset)).max(0.05);
                if (en.offset - rn.offset).abs() > tol {
                    continue;
                }
            }
            let d = (en.onset - rn.onset).abs();
            if best.is_none_or(|(_, bd)| d < bd) {
                best = Some((j, d));
            }
        }
        if let Some((j, _)) = best {
            used[j] = true;
            hits += 1;
        }
    }
    Fmeasure::from_counts(hits, e.len(), r.len())
}

/// チューニングのずれ(セント)の誤差。±50 セントで折り返す(+45 と -45 は 10 セント違い)
pub fn tuning_error_cents(reference: f64, estimate: f64) -> f64 {
    let d = (estimate - reference).rem_euclid(100.0);
    d.min(100.0 - d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: f64, end: f64, s: &str) -> ChordSpan {
        ChordSpan {
            start,
            end,
            symbol: s.to_owned(),
        }
    }

    #[test]
    fn beats_match_within_window_and_skip_intro() {
        let r: Vec<f64> = (0..20).map(|i| i as f64 * 0.5).collect();
        // 半分を 50ms ずらし、残りを 100ms ずらす → 当たりは半分
        let e: Vec<f64> = r
            .iter()
            .enumerate()
            .map(|(i, t)| t + if i % 2 == 0 { 0.05 } else { 0.1 })
            .collect();
        let f = beat_f(&r, &e);
        // 5 秒以降の正解は 10 個(5.0〜9.5)、推定も 10 個。50ms ずれの 5 個だけが当たり
        assert!((f.recall - 0.5).abs() < 0.11, "{f:?}");
        assert_eq!(beat_f(&r, &r).f, 1.0);
        assert_eq!(beat_f(&[], &[]).f, 1.0);
        assert_eq!(beat_f(&r, &[]).f, 0.0);
    }

    #[test]
    fn matching_is_one_to_one() {
        // 推定が 1 つの正解の近くに 2 つあっても、当たりは 1 つ
        assert_eq!(match_events(&[1.0, 2.0], &[0.99, 1.01, 2.0], 0.07), 2);
        assert_eq!(match_events(&[1.0], &[0.99, 1.01], 0.07), 1);
    }

    #[test]
    fn tempo_octave_errors_count_only_in_acc2() {
        assert_eq!(tempo_accuracy(120.0, 122.0), (true, true));
        assert_eq!(tempo_accuracy(120.0, 60.5), (false, true));
        assert_eq!(tempo_accuracy(120.0, 90.0), (false, false));
    }

    #[test]
    fn key_weights_follow_mirex() {
        let k = |s: &str| chord::Key::parse(s).unwrap();
        assert_eq!(key_score(k("C"), k("C")), 1.0);
        assert_eq!(key_score(k("C"), k("G")), 0.5);
        assert_eq!(key_score(k("C"), k("F")), 0.5);
        assert_eq!(key_score(k("C"), k("Am")), 0.3);
        assert_eq!(key_score(k("Am"), k("C")), 0.3);
        assert_eq!(key_score(k("C"), k("Cm")), 0.2);
        assert_eq!(key_score(k("C"), k("D")), 0.0);
    }

    #[test]
    fn chord_vocabularies() {
        let r = vec![
            span(0.0, 2.0, "C"),
            span(2.0, 4.0, "G7/B"),
            span(4.0, 5.0, "N"),
        ];
        let e = vec![
            span(0.0, 2.0, "C"),
            span(2.0, 4.0, "G"),
            span(4.0, 5.0, "N"),
        ];
        assert_eq!(chord_wcsr(&r, &e, ChordVocab::Root), Some(1.0));
        assert_eq!(chord_wcsr(&r, &e, ChordVocab::MajMin), Some(1.0));
        // 最低音(B)を外したので 2 秒分外れ
        assert_eq!(chord_wcsr(&r, &e, ChordVocab::MajMinInv), Some(0.6));
        // 7th を三和音で答えた
        assert_eq!(chord_wcsr(&r, &e, ChordVocab::Sevenths), Some(0.6));
        // sus4 は長短で書けないので除く
        let r2 = vec![span(0.0, 1.0, "Csus4"), span(1.0, 2.0, "Am")];
        let e2 = vec![span(0.0, 2.0, "Am")];
        assert_eq!(chord_wcsr(&r2, &e2, ChordVocab::MajMin), Some(1.0));
        // 半分ずれた区間
        let e3 = vec![span(0.0, 1.0, "C"), span(1.0, 5.0, "F")];
        assert_eq!(chord_wcsr(&r, &e3, ChordVocab::Root), Some(0.2));
    }

    #[test]
    fn notes_match_by_onset_and_pitch() {
        let n = |on: f64, off: f64, p: u8| NoteSpan {
            onset: on,
            offset: off,
            pitch: p,
        };
        let r = vec![n(0.0, 0.5, 60), n(0.5, 1.0, 62), n(1.0, 2.0, 64)];
        // 1 つ目は 30ms ずれ(当たり)、2 つ目は音高違い、3 つ目は終わりが短すぎる
        let e = vec![n(0.03, 0.5, 60), n(0.5, 1.0, 61), n(1.0, 1.3, 64)];
        let f = note_f(&r, &e, false);
        assert!((f.f - 2.0 / 3.0).abs() < 1e-9, "{f:?}");
        let f = note_f(&r, &e, true);
        assert!((f.f - 1.0 / 3.0).abs() < 1e-9, "{f:?}");
        assert_eq!(note_f(&r, &r, true).f, 1.0);
    }

    #[test]
    fn tuning_wraps_around() {
        assert!((tuning_error_cents(45.0, -45.0) - 10.0).abs() < 1e-9);
        assert!((tuning_error_cents(10.0, 15.0) - 5.0).abs() < 1e-9);
    }
}
