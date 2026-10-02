//! 音量をそろえた A/B の聴き比べ。
//!
//! 2 つの状態(例: 履歴のある地点と今)の同じ範囲を書き出し、統合ラウドネスを測って、大きい方を小さい方に
//! そろえる(上げはしないので割れない)。再生中はレンダラが今の位置の A か B を鳴らし、切り替えは短く
//! クロスフェードする(位置はそのまま)。人の耳は大きい方を良いと感じるので、その錯覚を避けて比べるため
//! (docs/DSP_RESEARCH.md の「A/B 比較は必ずラウドネスをそろえる」)。

use crate::data::SampleBank;
use glaux_core::Project;
use serde::Serialize;

/// 聴き比べる 2 つの音(ステレオ・インターリーブ、同じ長さ)と、そろえるための倍率
#[derive(Debug)]
pub struct AbClip {
    /// 範囲の頭(エンジンのサンプル位置)
    pub start: u64,
    pub a: Vec<f32>,
    pub b: Vec<f32>,
    pub gain_a: f32,
    pub gain_b: f32,
}

impl AbClip {
    /// 位置 `pos` の (A, B) のフレーム(そろえた後)。範囲の外は None
    #[inline]
    pub fn frame(&self, pos: u64) -> Option<((f32, f32), (f32, f32))> {
        let i = pos.checked_sub(self.start)? as usize;
        let at = |v: &[f32], g: f32| -> Option<(f32, f32)> {
            Some((*v.get(i * 2)? * g, *v.get(i * 2 + 1)? * g))
        };
        Some((at(&self.a, self.gain_a)?, at(&self.b, self.gain_b)?))
    }
}

/// どちらを鳴らすか
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AbSide {
    /// 聴き比べをしていない(ふつうの再生)
    Off,
    A,
    B,
}

impl AbSide {
    pub fn code(self) -> u8 {
        match self {
            AbSide::Off => 0,
            AbSide::A => 1,
            AbSide::B => 2,
        }
    }

    pub fn from_code(c: u8) -> AbSide {
        match c {
            1 => AbSide::A,
            2 => AbSide::B,
            _ => AbSide::Off,
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
    let render = |p: &Project, bank: &SampleBank| {
        crate::export::render_project_range(p, sample_rate, bank, from_secs, to_secs)
    };
    let (ra, rb) = std::thread::scope(|s| {
        let ha = s.spawn(|| render(a, bank_a));
        let rb = render(b, bank_b);
        (ha.join(), rb)
    });
    let mut ra =
        ra.map_err(|_| crate::export::ExportError::Render("書き出しが途中で止まりました".into()))??;
    let mut rb = rb?;
    // 長さをそろえる(末尾は無音で埋める)
    let len = ra.len().max(rb.len());
    ra.resize(len, 0.0);
    rb.resize(len, 0.0);
    let lufs = |v: &[f32]| {
        let l = crate::export::lufs_at(v, sample_rate as u32);
        (l.is_finite() && l > -70.0).then_some(l)
    };
    let (lufs_a, lufs_b) = (lufs(&ra), lufs(&rb));
    let (ga, gb) = match_gains(lufs_a, lufs_b);
    let amp = |db: f64| 10f64.powf(db / 20.0) as f32;
    let clip = AbClip {
        start: (from_secs.max(0.0) * sample_rate) as u64,
        a: ra,
        b: rb,
        gain_a: amp(ga),
        gain_b: amp(gb),
    };
    let info = AbInfo {
        lufs_a,
        lufs_b,
        gain_a_db: ga,
        gain_b_db: gb,
        from_secs,
        to_secs,
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
        assert_eq!(clip.a.len(), clip.b.len());
        let (la, lb) = (info.lufs_a.unwrap(), info.lufs_b.unwrap());
        assert!((lb - la - 6.0).abs() < 0.5, "{la} {lb}");
        assert_eq!(info.gain_a_db, 0.0);
        assert!((info.gain_b_db + (lb - la)).abs() < 1e-9);
        // そろえた後の大きさは同じ
        let rms = |v: &[f32], g: f32| {
            (v.iter().map(|x| (x * g) * (x * g)).sum::<f32>() / v.len() as f32).sqrt()
        };
        let (ra, rb) = (rms(&clip.a, clip.gain_a), rms(&clip.b, clip.gain_b));
        assert!((20.0 * (rb / ra).log10()).abs() < 0.5, "{ra} {rb}");
    }

    #[test]
    fn frames_follow_the_range_and_gains() {
        let c = AbClip {
            start: 100,
            a: vec![1.0, 1.0, 0.5, 0.5],
            b: vec![2.0, 2.0, 1.0, 1.0],
            gain_a: 1.0,
            gain_b: 0.5,
        };
        assert_eq!(c.frame(99), None);
        assert_eq!(c.frame(101), Some(((0.5, 0.5), (0.5, 0.5))));
        assert_eq!(c.frame(102), None);
    }
}
