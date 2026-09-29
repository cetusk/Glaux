//! 旋律の表情(MCP の pitch_gesture・set_vibrato の中身)。しゃくり・スクープ・フォール・ドイト・こぶし・ベンドを
//! ノートのピッチカーブの点に展開し、ビブラートは楽器の型から引数を決める。
//!
//! 数値は調査の目安(しゃくり −150 セント → 0 を 120ms、こぶし 0 → +80 → 0 を 200ms、フォールは終わりの 250ms で
//! −700 セント、ベンド +200 を 200ms。歌の音高の遷移は約 0.22 秒 = 山本ら)。長さは ms で受け、そのときのテンポで
//! tick に直す(呼び出し側が `ms_to_tick` を渡す)。

use crate::model::{CurveShape, Note, PitchPoint, Vibrato, MAX_PITCH_POINTS};
use crate::time::Tick;

/// 表情の種類
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    /// 下からすくい上げる(ジャズの管・歌)
    Scoop,
    /// しゃくり(J-POP の歌。下から素早く上がり、わずかに行き過ぎて落ち着く)
    Shakuri,
    /// 上から落ちて入る
    Plop,
    /// 下から滑って入る(ギターのスライド・歌の大きいずり上げ)
    SlideIn,
    /// チョーキング(全音下から上げる)
    Bend,
    /// 上げた状態で弾いて戻す(ギター)
    PrebendRelease,
    /// 音の終わりで上へ抜ける(ジャズの管)
    Doit,
    /// 音の終わりで下へ落ちる(ジャズの管・歌の語尾)
    Fall,
    /// こぶし(音の途中で上へ小さく回す)
    Kobushi,
    /// シェイク(音の途中から速く大きく揺らす。ジャズのトランペット)
    Shake,
}

impl Gesture {
    pub fn parse(s: &str) -> Option<Gesture> {
        Some(match s.trim().to_lowercase().as_str() {
            "scoop" => Gesture::Scoop,
            "shakuri" | "しゃくり" => Gesture::Shakuri,
            "plop" => Gesture::Plop,
            "slide_in" | "slide" => Gesture::SlideIn,
            "bend" => Gesture::Bend,
            "prebend_release" | "release" => Gesture::PrebendRelease,
            "doit" => Gesture::Doit,
            "fall" | "フォール" => Gesture::Fall,
            "kobushi" | "こぶし" => Gesture::Kobushi,
            "shake" => Gesture::Shake,
            _ => return None,
        })
    }

    /// 既定の (深さ セント, 長さ ms)
    pub fn defaults(self) -> (f32, f32) {
        match self {
            Gesture::Scoop => (150.0, 120.0),
            Gesture::Shakuri => (150.0, 120.0),
            Gesture::Plop => (300.0, 90.0),
            Gesture::SlideIn => (500.0, 160.0),
            Gesture::Bend => (200.0, 200.0),
            Gesture::PrebendRelease => (200.0, 250.0),
            Gesture::Doit => (500.0, 250.0),
            Gesture::Fall => (700.0, 250.0),
            Gesture::Kobushi => (80.0, 200.0),
            Gesture::Shake => (150.0, 400.0),
        }
    }

    /// 既定で付ける音の選び方
    pub fn default_target(self) -> Target {
        match self {
            Gesture::Scoop | Gesture::Shakuri | Gesture::SlideIn | Gesture::Bend => Target::LeapUp,
            Gesture::Plop => Target::PhraseStart,
            Gesture::Doit | Gesture::Fall => Target::PhraseEnd,
            Gesture::Kobushi | Gesture::Shake | Gesture::PrebendRelease => Target::Long,
        }
    }
}

/// 表情を付ける音の選び方
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    All,
    /// 句の頭(最初の音と、8 分以上の休みの後の音)
    PhraseStart,
    /// 句の終わり(最後の音と、8 分以上の休みの前の音)
    PhraseEnd,
    /// 前の音から 3 半音以上上がる音と句の頭
    LeapUp,
    /// 1 拍以上の音
    Long,
}

impl Target {
    pub fn parse(s: &str) -> Option<Target> {
        Some(match s.trim().to_lowercase().as_str() {
            "all" => Target::All,
            "phrase_start" | "start" => Target::PhraseStart,
            "phrase_end" | "end" => Target::PhraseEnd,
            "leap_up" | "leap" => Target::LeapUp,
            "long" => Target::Long,
            _ => return None,
        })
    }
}

/// 旋律(時間順)の中で `target` に当たる音の番号。和音は一番上の音だけを旋律とみなす
pub fn select(notes: &[Note], target: Target, beat: u64) -> Vec<usize> {
    let mut order: Vec<usize> = (0..notes.len()).collect();
    order.sort_by_key(|&i| (notes[i].pos, std::cmp::Reverse(notes[i].pitch)));
    // 同じ位置の音は一番上だけ
    order.dedup_by_key(|i| notes[*i].pos);
    let gap = beat / 2;
    let mut out = Vec::new();
    for (k, &i) in order.iter().enumerate() {
        let n = &notes[i];
        let prev = k.checked_sub(1).map(|j| &notes[order[j]]);
        let next = order.get(k + 1).map(|&j| &notes[j]);
        let start = prev.map_or(true, |p| n.pos.0 >= p.pos.0 + p.dur.0 + gap);
        let end = next.map_or(true, |x| x.pos.0 >= n.pos.0 + n.dur.0 + gap);
        let hit = match target {
            Target::All => true,
            Target::PhraseStart => start,
            Target::PhraseEnd => end,
            Target::LeapUp => start || prev.is_some_and(|p| n.pitch as i32 - p.pitch as i32 >= 3),
            Target::Long => n.dur.0 >= beat,
        };
        if hit {
            out.push(i);
        }
    }
    out
}

/// 表情のピッチカーブ(ノート先頭からの tick, セント, 曲がり方)。`len` は音の長さ(tick)、
/// `time` は表情の長さ(tick。音の長さの半分までに縮める)、`amount` は深さ(セント)
pub fn curve(g: Gesture, len: u64, time: u64, amount: f32) -> Vec<(u64, f32, CurveShape)> {
    use CurveShape::*;
    let t = time.min(len / 2).max(1);
    match g {
        Gesture::Scoop => vec![(0, -amount, EaseOut), (t, 0.0, Linear)],
        Gesture::Shakuri => vec![
            (0, -amount, EaseOut),
            (t * 4 / 5, amount * 0.06, EaseInOut),
            (t, 0.0, Linear),
        ],
        Gesture::Plop => vec![(0, amount, EaseIn), (t, 0.0, Linear)],
        Gesture::SlideIn => vec![(0, -amount, Linear), (t, 0.0, Linear)],
        Gesture::Bend => vec![(0, -amount, EaseOut), (t, 0.0, Linear)],
        Gesture::PrebendRelease => vec![
            (0, amount, Hold),
            (t * 2 / 5, amount, EaseInOut),
            (t, 0.0, Linear),
        ],
        Gesture::Doit => vec![(len - t, 0.0, EaseIn), (len, amount, Linear)],
        Gesture::Fall => vec![(len - t, 0.0, EaseIn), (len, -amount, Linear)],
        Gesture::Kobushi => {
            let a = (len * 3 / 10).min(len.saturating_sub(t));
            vec![
                (a, 0.0, EaseInOut),
                (a + t / 2, amount, EaseInOut),
                (a + t, 0.0, Linear),
            ]
        }
        Gesture::Shake => {
            // 音の 3 割から終わりまで、約 7 回/秒 の往復(点の上限まで)
            let a = len * 3 / 10;
            let half = (t / 6).max(1);
            let mut v = vec![(a, 0.0, EaseInOut)];
            let mut k = 1u64;
            while a + k * half < len && v.len() < MAX_PITCH_POINTS - 1 {
                let c = if k % 2 == 1 { amount } else { 0.0 };
                v.push((a + k * half, c, EaseInOut));
                k += 1;
            }
            v.push((len, 0.0, Linear));
            v
        }
    }
}

/// 今のピッチカーブに表情を重ねる。表情の範囲にある元の点は消し、範囲の外は残す(上限まで)
pub fn merge(existing: &[PitchPoint], pts: &[(u64, f32, CurveShape)]) -> Vec<PitchPoint> {
    let lo = pts.first().map_or(0, |p| p.0);
    let hi = pts.last().map_or(0, |p| p.0);
    let mut out: Vec<PitchPoint> = existing
        .iter()
        .filter(|p| p.tick.0 < lo || p.tick.0 > hi)
        .copied()
        .collect();
    out.extend(
        pts.iter()
            .map(|&(t, c, s)| PitchPoint::shaped(Tick(t), c, s)),
    );
    out.sort_by_key(|p| p.tick);
    out.dedup_by_key(|p| p.tick);
    out.truncate(MAX_PITCH_POINTS);
    out
}

/// 音の中の強弱・明るさの動き
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dynamic {
    /// 音の中でふくらむ(弦・管・パッドのクレッシェンド)
    Swell,
    /// 音の終わりへ消える
    Fade,
    /// スフォルツァンド(頭を強く打って落とし、そのまま)
    Sfz,
    /// フォルテピアノ(強く入ってすぐ弱く)
    Fp,
    /// 音量の揺れ(トレモロ。点の上限まで)
    Pulse,
    /// 暗くから開く(明るさ)
    Open,
    /// 明るくから閉じる(明るさ)
    Close,
}

impl Dynamic {
    pub fn parse(s: &str) -> Option<Dynamic> {
        Some(match s.trim().to_lowercase().as_str() {
            "swell" | "crescendo" => Dynamic::Swell,
            "fade" | "decrescendo" => Dynamic::Fade,
            "sfz" | "sforzando" => Dynamic::Sfz,
            "fp" => Dynamic::Fp,
            "pulse" | "tremolo" => Dynamic::Pulse,
            "open" => Dynamic::Open,
            "close" => Dynamic::Close,
            _ => return None,
        })
    }

    /// 明るさの曲線か(でなければ音量)
    pub fn is_brightness(self) -> bool {
        matches!(self, Dynamic::Open | Dynamic::Close)
    }

    /// 既定の (深さ, 長さ ms)。深さは音量なら dB、明るさなら 0〜1
    pub fn defaults(self) -> (f32, f32) {
        match self {
            Dynamic::Swell => (12.0, 0.0),
            Dynamic::Fade => (24.0, 0.0),
            Dynamic::Sfz => (9.0, 150.0),
            Dynamic::Fp => (12.0, 120.0),
            Dynamic::Pulse => (9.0, 100.0),
            Dynamic::Open => (0.7, 0.0),
            Dynamic::Close => (0.7, 0.0),
        }
    }

    /// 曲線 (tick, 値, 曲がり方)。`len` は音の長さ、`time` は sfz・fp の落ちる長さ・pulse の半周期(tick)
    pub fn curve(self, len: u64, time: u64, amount: f32) -> Vec<(u64, f32, CurveShape)> {
        use CurveShape::*;
        let t = time.min(len / 2).max(1);
        match self {
            Dynamic::Swell => vec![(0, -amount, EaseIn), (len * 7 / 10, 0.0, Linear)],
            Dynamic::Fade => vec![(len * 2 / 5, 0.0, EaseIn), (len, -amount, Linear)],
            Dynamic::Sfz => vec![(0, 3.0, EaseOut), (t, -amount, Linear)],
            Dynamic::Fp => vec![(0, 0.0, EaseOut), (t, -amount, Linear)],
            Dynamic::Pulse => {
                let mut v = Vec::new();
                let mut k = 0u64;
                while k * t < len && v.len() < crate::model::MAX_EXPR_POINTS {
                    v.push((k * t, if k % 2 == 0 { 0.0 } else { -amount }, EaseInOut));
                    k += 1;
                }
                v
            }
            Dynamic::Open => vec![(0, -amount, EaseOut), (len * 3 / 5, 0.2, Linear)],
            Dynamic::Close => vec![(0, 0.2, EaseIn), (len, -amount, Linear)],
        }
    }
}

/// ビブラートの型 (名前, 速さ Hz, 深さ セント, 始まり ms, フェードイン ms, 説明)
pub const VIBRATO_STYLES: &[(&str, f32, f32, f32, f32, &str)] = &[
    (
        "vocal",
        5.5,
        40.0,
        250.0,
        200.0,
        "歌(ポップス): 伸ばしの後半から自然に",
    ),
    (
        "vocal_strong",
        5.8,
        80.0,
        200.0,
        150.0,
        "強い歌(演歌・J-POP のサビの伸ばし)",
    ),
    ("strings", 5.5, 20.0, 150.0, 300.0, "弦: 浅く、ゆっくり深く"),
    (
        "guitar",
        6.0,
        35.0,
        150.0,
        120.0,
        "ギターのソロ: 指で揺らす",
    ),
    (
        "wind",
        5.0,
        25.0,
        200.0,
        250.0,
        "管(サックス・フルート): 息で揺らす",
    ),
    (
        "synth",
        5.5,
        25.0,
        0.0,
        400.0,
        "シンセのリード: 最初からゆっくり深く",
    ),
];

pub fn vibrato_style(name: &str) -> Option<Vibrato> {
    VIBRATO_STYLES
        .iter()
        .find(|s| s.0.eq_ignore_ascii_case(name.trim()))
        .map(|&(_, rate, depth, delay, fade, _)| Vibrato {
            rate_hz: rate,
            depth_cents: depth,
            delay_ms: delay,
            fade_in_ms: fade,
            fade_out_ms: 0.0,
            rate_end_hz: None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::NoteId;

    fn note(pos: u64, dur: u64, pitch: u8) -> Note {
        Note {
            id: NoteId::new(),
            pos: Tick(pos),
            dur: Tick(dur),
            pitch,
            vel: 90,
            articulation: Default::default(),
            pitch_curve: vec![],
            glide_ms: None,
            vibrato: None,
            volume_curve: vec![],
            brightness_curve: vec![],
            condition: None,
        }
    }

    #[test]
    fn targets_pick_phrase_edges_leaps_and_long_notes() {
        // 句 1: C D G(上へ跳躍)、休み、句 2: E(長い)
        let ns = vec![
            note(0, 480, 60),
            note(480, 480, 62),
            note(960, 480, 67),
            note(2400, 1920, 64),
        ];
        assert_eq!(select(&ns, Target::PhraseStart, 960), vec![0, 3]);
        assert_eq!(select(&ns, Target::PhraseEnd, 960), vec![2, 3]);
        assert_eq!(select(&ns, Target::LeapUp, 960), vec![0, 2, 3]);
        assert_eq!(select(&ns, Target::Long, 960), vec![3]);
        // 和音は一番上だけ
        let chord = vec![note(0, 960, 60), note(0, 960, 67)];
        assert_eq!(select(&chord, Target::All, 960), vec![1]);
    }

    #[test]
    fn gesture_curves_have_the_expected_shape() {
        let c = curve(Gesture::Shakuri, 1920, 230, 150.0);
        assert_eq!(c[0], (0, -150.0, CurveShape::EaseOut));
        assert!(c[1].1 > 0.0, "わずかに行き過ぎる");
        assert_eq!(c.last().unwrap().1, 0.0);
        let f = curve(Gesture::Fall, 1920, 480, 700.0);
        assert_eq!(f[0].0, 1440);
        assert_eq!(f.last().unwrap(), &(1920, -700.0, CurveShape::Linear));
        // 短い音では音の半分までに縮める
        let s = curve(Gesture::Scoop, 200, 480, 150.0);
        assert_eq!(s.last().unwrap().0, 100);
        let k = curve(Gesture::Kobushi, 1920, 384, 80.0);
        assert!(k.iter().any(|p| p.1 == 80.0));
        let sh = curve(Gesture::Shake, 3840, 768, 150.0);
        assert!(sh.len() <= MAX_PITCH_POINTS && sh.len() > 6);
        assert_eq!(Gesture::parse("しゃくり"), Some(Gesture::Shakuri));
        assert!(Gesture::parse("wobble").is_none());
    }

    #[test]
    fn merging_keeps_points_outside_the_gesture() {
        // 頭のしゃくりの上に、終わりのフォールを重ねる
        let head: Vec<PitchPoint> = curve(Gesture::Shakuri, 1920, 230, 150.0)
            .iter()
            .map(|&(t, c, s)| PitchPoint::shaped(Tick(t), c, s))
            .collect();
        let both = merge(&head, &curve(Gesture::Fall, 1920, 480, 700.0));
        assert_eq!(both.len(), head.len() + 2);
        assert!(both.windows(2).all(|w| w[0].tick < w[1].tick));
        assert!(crate::model::check_pitch_curve(&both).is_ok());
    }

    #[test]
    fn dynamics_stay_in_range() {
        for d in [
            Dynamic::Swell,
            Dynamic::Fade,
            Dynamic::Sfz,
            Dynamic::Fp,
            Dynamic::Pulse,
            Dynamic::Open,
            Dynamic::Close,
        ] {
            let (amount, ms) = d.defaults();
            let c: Vec<crate::model::CurvePoint> = d
                .curve(1920, (ms * 1.92) as u64, amount)
                .into_iter()
                .map(|(t, v, s)| crate::model::CurvePoint::new(Tick(t), v, s))
                .collect();
            let range = if d.is_brightness() {
                crate::model::BRIGHTNESS_RANGE
            } else {
                crate::model::VOLUME_CURVE_DB
            };
            assert!(
                crate::model::check_expr_curve("x", &c, range).is_ok(),
                "{d:?} {c:?}"
            );
        }
        assert_eq!(Dynamic::parse("sforzando"), Some(Dynamic::Sfz));
    }

    #[test]
    fn vibrato_styles_are_valid() {
        for s in VIBRATO_STYLES {
            let v = vibrato_style(s.0).unwrap();
            assert!(crate::model::check_vibrato(&v).is_ok(), "{}", s.0);
        }
        assert!(vibrato_style("theremin").is_none());
    }
}
