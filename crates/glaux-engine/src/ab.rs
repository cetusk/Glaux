//! 音量をそろえた A/B の聴き比べ。
//!
//! 2 つの状態(例: 履歴のある地点と今)の同じ範囲を書き出し、統合ラウドネスを測って、大きい方を小さい方に
//! そろえる(上げはしないので割れない)。再生中はレンダラが今の位置の A か B を鳴らし、切り替えは短く
//! クロスフェードする(位置はそのまま)。人の耳は大きい方を良いと感じるので、その錯覚を避けて比べるため
//! (docs/DSP_RESEARCH.md の「A/B 比較は必ずラウドネスをそろえる」)。

use crate::data::SampleBank;
use glaux_core::Project;
use serde::Serialize;

/// 聴き比べる音(2 つ以上。ステレオ・インターリーブ、同じ長さ)と、そろえるための倍率。
/// 0 番が A(今・前)、1 番が B、2 番が C …
#[derive(Debug)]
pub struct AbClip {
    /// 範囲の頭(エンジンのサンプル位置)
    pub start: u64,
    pub takes: Vec<Vec<f32>>,
    pub gains: Vec<f32>,
}

/// いっしょに聴き比べられる音の数の上限(今 + 案 4 つ)
pub const MAX_TAKES: usize = 5;

impl AbClip {
    /// 位置 `pos` の、`i` 番の音のフレーム(そろえた後)。範囲の外・無い番号は None
    #[inline]
    pub fn take(&self, i: usize, pos: u64) -> Option<(f32, f32)> {
        let k = pos.checked_sub(self.start)? as usize;
        let v = self.takes.get(i)?;
        let g = *self.gains.get(i)?;
        Some((*v.get(k * 2)? * g, *v.get(k * 2 + 1)? * g))
    }
}

/// どれを鳴らすか(コード 0 = 聴き比べをしていない、1 = A、2 = B、3 = C …)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbSide {
    /// 聴き比べをしていない(ふつうの再生)
    Off,
    /// `n` 番の音(0 = A)
    Take(u8),
}

impl AbSide {
    pub const A: AbSide = AbSide::Take(0);
    pub const B: AbSide = AbSide::Take(1);

    pub fn code(self) -> u8 {
        match self {
            AbSide::Off => 0,
            AbSide::Take(n) => n.saturating_add(1),
        }
    }

    pub fn from_code(c: u8) -> AbSide {
        match c {
            0 => AbSide::Off,
            n => AbSide::Take(n - 1),
        }
    }

    /// 番号(Off なら None)
    pub fn index(self) -> Option<usize> {
        match self {
            AbSide::Off => None,
            AbSide::Take(n) => Some(n as usize),
        }
    }
}

/// 用意した聴き比べの情報(画面に出す)
#[derive(Clone, Debug, Serialize)]
pub struct AbInfo {
    /// それぞれの統合ラウドネス(LUFS。短すぎ・無音なら None)
    pub lufs_a: Option<f64>,
    pub lufs_b: Option<f64>,
    /// そろえるために掛けた量(dB。0 か負)
    pub gain_a_db: f64,
    pub gain_b_db: f64,
    /// 範囲(秒)
    pub from_secs: f64,
    pub to_secs: f64,
    /// 範囲の中で A と B の音が違い始める位置(範囲の頭からの秒)。同じなら None(切り替えても違いが聞こえない)
    pub first_diff_secs: Option<f64>,
}

/// 2 つの書き出し(ステレオ・インターリーブ)が違い始めるフレーム。同じなら None
fn first_diff(a: &[f32], b: &[f32]) -> Option<usize> {
    // -80 dBFS 程度より小さい違いは聞こえないので同じとみなす
    const EPS: f32 = 1e-4;
    a.iter()
        .zip(b)
        .position(|(x, y)| (x - y).abs() > EPS)
        .map(|i| i / 2)
}

/// そろえる量の上限(dB)。片方がほぼ無音のときに、もう片方を大きく下げすぎないように
const MAX_MATCH_DB: f64 = 24.0;

/// 2 つの統合ラウドネスから、そろえる倍率(dB。大きい方を下げる)
pub fn match_gains(lufs_a: Option<f64>, lufs_b: Option<f64>) -> (f64, f64) {
    match (lufs_a, lufs_b) {
        (Some(a), Some(b)) => {
            let target = a.min(b);
            (
                (target - a).max(-MAX_MATCH_DB),
                (target - b).max(-MAX_MATCH_DB),
            )
        }
        _ => (0.0, 0.0),
    }
}

/// 複数の音をそろえた聴き比べの情報(画面に出す)
#[derive(Clone, Debug, Serialize)]
pub struct AbManyInfo {
    /// それぞれの統合ラウドネス(LUFS。短すぎ・無音なら None)と、そろえるために掛けた量(dB。0 か負)
    pub lufs: Vec<Option<f64>>,
    pub gains_db: Vec<f64>,
    pub from_secs: f64,
    pub to_secs: f64,
    /// それぞれが 0 番(A)と違い始める位置(範囲の頭からの秒。0 番自身と、同じものは None)
    pub first_diff_secs: Vec<Option<f64>>,
}

/// 複数の曲の `[from_secs, to_secs)` を並べて書き出し、いちばん小さい音量にそろえた聴き比べを用意する(最大 [`MAX_TAKES`])
pub fn prepare_many(
    songs: &[(&Project, &SampleBank)],
    sample_rate: f64,
    from_secs: f64,
    to_secs: f64,
) -> Result<(AbClip, AbManyInfo), crate::export::ExportError> {
    if songs.len() < 2 || songs.len() > MAX_TAKES {
        return Err(crate::export::ExportError::Render(format!(
            "聴き比べる音は 2〜{MAX_TAKES} 個"
        )));
    }
    let renders: Vec<Result<Vec<f32>, crate::export::ExportError>> = std::thread::scope(|s| {
        let hs: Vec<_> = songs
            .iter()
            .map(|(p, bank)| {
                s.spawn(move || {
                    crate::export::render_project_range(p, sample_rate, bank, from_secs, to_secs)
                })
            })
            .collect();
        hs.into_iter()
            .map(|h| {
                h.join().unwrap_or_else(|_| {
                    Err(crate::export::ExportError::Render(
                        "書き出しが途中で止まりました".into(),
                    ))
                })
            })
            .collect()
    });
    let mut takes = Vec::with_capacity(renders.len());
    for r in renders {
        takes.push(r?);
    }
    // 長さをそろえる(末尾は無音で埋める)
    let len = takes.iter().map(|t| t.len()).max().unwrap_or(0);
    for t in &mut takes {
        t.resize(len, 0.0);
    }
    let lufs: Vec<Option<f64>> = takes
        .iter()
        .map(|v| {
            let l = crate::export::lufs_at(v, sample_rate as u32);
            (l.is_finite() && l > -70.0).then_some(l)
        })
        .collect();
    // いちばん小さい音量にそろえる(上げはしないので割れない。無音の音は動かさない)
    let target = lufs.iter().flatten().copied().fold(f64::INFINITY, f64::min);
    let gains_db: Vec<f64> = lufs
        .iter()
        .map(|l| match l {
            Some(l) if target.is_finite() => (target - l).max(-MAX_MATCH_DB),
            _ => 0.0,
        })
        .collect();
    let first_diff_secs: Vec<Option<f64>> = takes
        .iter()
        .enumerate()
        .map(|(i, t)| {
            if i == 0 {
                None
            } else {
                first_diff(&takes[0], t).map(|f| f as f64 / sample_rate)
            }
        })
        .collect();
    let amp = |db: f64| 10f64.powf(db / 20.0) as f32;
    let clip = AbClip {
        start: (from_secs.max(0.0) * sample_rate) as u64,
        gains: gains_db.iter().map(|g| amp(*g)).collect(),
        takes,
    };
    Ok((
        clip,
        AbManyInfo {
            lufs,
            gains_db,
            from_secs,
            to_secs,
            first_diff_secs,
        },
    ))
}

/// `a` と `b` の `[from_secs, to_secs)` を書き出して、音量をそろえた聴き比べを用意する。
/// 2 つの書き出しは並べて行う(CLAP は書き出し専用のインスタンスをそれぞれ作る)
pub fn prepare(
    a: &Project,
    bank_a: &SampleBank,
    b: &Project,
    bank_b: &SampleBank,
    sample_rate: f64,
    from_secs: f64,
    to_secs: f64,
) -> Result<(AbClip, AbInfo), crate::export::ExportError> {
    let (clip, m) = prepare_many(&[(a, bank_a), (b, bank_b)], sample_rate, from_secs, to_secs)?;
    let info = AbInfo {
        lufs_a: m.lufs[0],
        lufs_b: m.lufs[1],
        gain_a_db: m.gains_db[0],
        gain_b_db: m.gains_db[1],
        from_secs,
        to_secs,
        first_diff_secs: m.first_diff_secs[1],
    };
    Ok((clip, info))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn louder_side_is_turned_down_to_the_quieter() {
        assert_eq!(match_gains(Some(-10.0), Some(-14.0)), (-4.0, 0.0));
        assert_eq!(match_gains(Some(-20.0), Some(-12.5)), (0.0, -7.5));
        // 片方が無音なら何もしない、差が大きすぎるときは上限まで
        assert_eq!(match_gains(None, Some(-12.0)), (0.0, 0.0));
        assert_eq!(match_gains(Some(-8.0), Some(-60.0)), (-MAX_MATCH_DB, 0.0));
    }

    /// 4 小節伸ばしたままの音 1 本の曲
    fn pad(volume_db: f32) -> Project {
        use glaux_core::{
            Clip, ClipContent, ClipId, Note, NoteId, Tick, Track, TrackId, TrackKind,
        };
        let mut p = Project::new("t");
        let mut t = Track::new(TrackId::new(), "A", TrackKind::Midi);
        t.volume_db = volume_db;
        let mut c = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(3840 * 4));
        if let ClipContent::Midi { notes, .. } = &mut c.content {
            notes.push(Note {
                locked: false,
                id: NoteId::new(),
                pos: Tick(0),
                dur: Tick(3840 * 4),
                pitch: 57,
                vel: 100,
                articulation: Default::default(),
                pitch_curve: vec![],
                glide_ms: None,
                vibrato: None,
                volume_curve: vec![],
                brightness_curve: vec![],
                condition: None,
            });
        }
        t.clips.push(c);
        p.tracks.push(t);
        p
    }

    #[test]
    fn prepare_measures_both_and_turns_the_louder_down() {
        let (a, b) = (pad(-12.0), pad(-6.0));
        let bank = SampleBank::default();
        let (clip, info) = prepare(&a, &bank, &b, &bank, 48_000.0, 1.0, 3.0).unwrap();
        assert_eq!(clip.start, 48_000);
        assert_eq!(clip.takes[0].len(), clip.takes[1].len());
        let (la, lb) = (info.lufs_a.unwrap(), info.lufs_b.unwrap());
        assert!((lb - la - 6.0).abs() < 0.5, "{la} {lb}");
        assert_eq!(info.gain_a_db, 0.0);
        assert!((info.gain_b_db + (lb - la)).abs() < 1e-9);
        // そろえた後の大きさは同じ
        let rms = |v: &[f32], g: f32| {
            (v.iter().map(|x| (x * g) * (x * g)).sum::<f32>() / v.len() as f32).sqrt()
        };
        let (ra, rb) = (
            rms(&clip.takes[0], clip.gains[0]),
            rms(&clip.takes[1], clip.gains[1]),
        );
        assert!((20.0 * (rb / ra).log10()).abs() < 0.5, "{ra} {rb}");
    }

    #[test]
    fn frames_follow_the_range_and_gains() {
        let c = AbClip {
            start: 100,
            takes: vec![vec![1.0, 1.0, 0.5, 0.5], vec![2.0, 2.0, 1.0, 1.0]],
            gains: vec![1.0, 0.5],
        };
        assert_eq!(c.take(0, 99), None);
        assert_eq!(c.take(0, 101), Some((0.5, 0.5)));
        assert_eq!(c.take(1, 101), Some((0.5, 0.5)));
        assert_eq!(c.take(0, 102), None);
        assert_eq!(c.take(2, 100), None);
    }

    #[test]
    fn many_takes_are_matched_to_the_quietest_and_report_where_each_differs() {
        let (a, b, c) = (pad(-12.0), pad(-6.0), pad(-12.0));
        let bank = SampleBank::default();
        let (clip, info) =
            prepare_many(&[(&a, &bank), (&b, &bank), (&c, &bank)], 48_000.0, 1.0, 3.0).unwrap();
        assert_eq!(clip.takes.len(), 3);
        assert_eq!(info.gains_db[0], 0.0);
        assert!(info.gains_db[1] < -5.0, "{:?}", info.gains_db);
        assert_eq!(info.gains_db[2], 0.0);
        // B は頭から違い、C は A と同じ
        assert_eq!(info.first_diff_secs[0], None);
        assert!(info.first_diff_secs[1].is_some());
        assert_eq!(info.first_diff_secs[2], None);
        // 1 つだけ・多すぎるのは受け付けない
        assert!(prepare_many(&[(&a, &bank)], 48_000.0, 1.0, 3.0).is_err());
    }

    #[test]
    fn side_codes_round_trip() {
        for s in [AbSide::Off, AbSide::A, AbSide::B, AbSide::Take(4)] {
            assert_eq!(AbSide::from_code(s.code()), s);
        }
        assert_eq!(AbSide::Take(2).index(), Some(2));
    }
}
