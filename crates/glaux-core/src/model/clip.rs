//! クリップ。MIDI クリップと音声クリップを `kind` で区別する。
//!
//! 音声クリップも拍(Tick)上に置く。元音声は変更せず、切り出し位置・ゲイン・フェード・
//! ストレッチをデータとして重ねる非破壊編集。

use crate::id::{AssetId, ClipId, NoteId};
use crate::time::Tick;
use serde::{Deserialize, Serialize};

/// ノート単位の奏法(アーティキュレーション)。
/// 「同じ音源で奏法を切り替える」表現(メタルのブリッジミュート等)。
/// `Normal` は JSON に書かない(= フィールド省略)ので旧ファイルとそのまま互換。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Articulation {
    /// 通常(既定)
    #[default]
    Normal,
    /// ブリッジ(パーム)ミュート: 減衰が速く、こもった「ズンズン」した音
    PalmMute,
    /// スタッカート: 音価の半分で切る歯切れのよい発音
    Staccato,
    /// アクセント: その音だけ強く・明るく強調
    Accent,
    /// ビブラート: 音の後半にピッチの揺れが深くなっていく
    Vibrato,
    /// チョーキング(ベンド): 全音下から書かれた音程へ滑り上がる
    Bend,
    /// レガート(スラー): 同じトラックの直前の音から弾き直さずに滑らかにつなぐ
    /// (立ち上がりを消し、前の音はつなぎ目で消える。弦・管・歌のフレーズ、ギターのハンマリング)
    Legato,
    /// ポルタメント: レガートでつなぎ、直前の音の高さから書かれた音程へ滑らせる
    /// (ストリングスのポルタメント、シンセリードのグライド、ギターのスライド)
    Portamento,
}

impl Articulation {
    pub fn is_normal(&self) -> bool {
        *self == Articulation::Normal
    }
}

/// 連続ピッチカーブの 1 点。ノート先頭からの相対 tick と、書かれた音程からの
/// ずれ(セント。100 = 半音)。点の間は線形補間、最初の点より前 / 最後の点より後は
/// その値を保持する。
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct PitchPoint {
    pub tick: Tick,
    pub cents: f32,
}

/// ピッチカーブの点数上限(ボイス側が固定長で持つため)
pub const MAX_PITCH_POINTS: usize = 8;
/// ピッチカーブの振れ幅の上限(セント)
pub const MAX_PITCH_CENTS: f32 = 2400.0;

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Note {
    pub id: NoteId,
    /// クリップ先頭からの相対位置
    pub pos: Tick,
    pub dur: Tick,
    /// MIDI ノート番号 0..=127
    pub pitch: u8,
    /// ベロシティ 1..=127
    pub vel: u8,
    /// 奏法。省略時 Normal
    #[serde(default, skip_serializing_if = "Articulation::is_normal")]
    pub articulation: Articulation,
    /// 連続ピッチカーブ(自由描画のベンド・ポルタメント等)。空なら無し。
    /// 奏法(vibrato / bend)と併用できる(掛け合わせ)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pitch_curve: Vec<PitchPoint>,
    /// ポルタメントで滑る時間(ms)。省略時はトラックの `glide_ms`(それも無ければ 150ms)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glide_ms: Option<f32>,
}

/// ポルタメントで滑る時間の範囲(ms)
pub const GLIDE_MS_RANGE: std::ops::RangeInclusive<f32> = 10.0..=2000.0;
/// レガートのつなぎ目の長さの範囲(ms)
pub const LEGATO_MS_RANGE: std::ops::RangeInclusive<f32> = 5.0..=200.0;

/// ピッチカーブの検証(最大 `MAX_PITCH_POINTS` 点・tick は昇順・±`MAX_PITCH_CENTS` 以内)。
pub fn check_pitch_curve(curve: &[PitchPoint]) -> Result<(), String> {
    if curve.len() > MAX_PITCH_POINTS {
        return Err(format!("pitch_curve は最大 {MAX_PITCH_POINTS} 点"));
    }
    if curve.windows(2).any(|w| w[1].tick < w[0].tick) {
        return Err("pitch_curve の tick は昇順に".to_owned());
    }
    if curve
        .iter()
        .any(|p| !p.cents.is_finite() || p.cents.abs() > MAX_PITCH_CENTS)
    {
        return Err(format!("pitch_curve の cents は ±{MAX_PITCH_CENTS} 以内"));
    }
    Ok(())
}

impl Note {
    pub fn end(&self) -> Tick {
        self.pos + self.dur
    }
}

#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Stretch {
    /// ストレッチなし(元の速度で再生)
    #[default]
    None,
    /// テンポに追従する(音程は変えずに伸縮)。素材は `original_bpm` で演奏されたものとして
    /// 扱い、1 tick = 60 / (original_bpm × PPQ) 秒ぶんの素材が常に 1 tick に対応する。
    /// プロジェクトのテンポを変えても拍がずれない
    Follow { original_bpm: f64 },
}

impl Stretch {
    /// 追従時に受け付ける元テンポの範囲
    pub const BPM_RANGE: std::ops::RangeInclusive<f64> = 20.0..=400.0;

    /// クリップ先頭から `ticks` 進んだ位置が、素材の何秒目(`offset` から)に当たるか。
    /// 追従しないときは `None`(テンポマップで秒に直す)
    pub fn follow_seconds(&self, ticks: f64) -> Option<f64> {
        match self {
            Stretch::None => None,
            Stretch::Follow { original_bpm } => {
                Some(ticks * 60.0 / (original_bpm * crate::time::PPQ as f64))
            }
        }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClipContent {
    Midi {
        /// (pos, pitch, id) 昇順を保つ
        #[serde(default)]
        notes: Vec<Note>,
        #[serde(default, rename = "loop")]
        looped: bool,
        /// ループ時に繰り返す長さ(クリップ先頭から)。`looped` が true のときだけ意味を持つ。
        /// クリップ長がこれより長いと、その範囲のノートが繰り返し鳴る
        #[serde(default, skip_serializing_if = "Option::is_none")]
        loop_len: Option<Tick>,
    },
    Audio {
        asset: AssetId,
        /// アセット内の再生開始位置(サンプル)
        #[serde(default)]
        offset_samples: u64,
        #[serde(default)]
        gain_db: f32,
        #[serde(default)]
        fade_in_ms: f32,
        #[serde(default)]
        fade_out_ms: f32,
        #[serde(default)]
        stretch: Stretch,
    },
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Clip {
    pub id: ClipId,
    pub name: String,
    pub start: Tick,
    pub length: Tick,
    #[serde(flatten)]
    pub content: ClipContent,
}

impl Clip {
    pub fn new_midi(id: ClipId, name: impl Into<String>, start: Tick, length: Tick) -> Self {
        Clip {
            id,
            name: name.into(),
            start,
            length,
            content: ClipContent::Midi {
                notes: vec![],
                looped: false,
                loop_len: None,
            },
        }
    }

    pub fn new_audio(
        id: ClipId,
        name: impl Into<String>,
        start: Tick,
        length: Tick,
        asset: AssetId,
    ) -> Self {
        Clip {
            id,
            name: name.into(),
            start,
            length,
            content: ClipContent::Audio {
                asset,
                offset_samples: 0,
                gain_db: 0.0,
                fade_in_ms: 0.0,
                fade_out_ms: 0.0,
                stretch: Stretch::None,
            },
        }
    }

    pub fn end(&self) -> Tick {
        self.start + self.length
    }

    pub fn is_midi(&self) -> bool {
        matches!(self.content, ClipContent::Midi { .. })
    }

    pub fn notes(&self) -> Option<&[Note]> {
        match &self.content {
            ClipContent::Midi { notes, .. } => Some(notes),
            _ => None,
        }
    }

    pub fn notes_mut(&mut self) -> Option<&mut Vec<Note>> {
        match &mut self.content {
            ClipContent::Midi { notes, .. } => Some(notes),
            _ => None,
        }
    }

    /// ループの繰り返し長(ループでなければ None)。
    pub fn loop_len(&self) -> Option<Tick> {
        match &self.content {
            ClipContent::Midi {
                looped: true,
                loop_len: Some(l),
                ..
            } if l.0 > 0 => Some(*l),
            _ => None,
        }
    }

    /// 再生・分析で実際に鳴るノート列(クリップ先頭からの位置)。
    /// クリップ長の外は切り捨て・切り詰め、ループクリップは繰り返しを展開する
    /// (ループ境界をまたぐノートは境界で切る。ループ長より後ろのノートは鳴らない)。
    /// 展開したノートは元と同じ ID を持つので、編集には使わないこと。
    pub fn playback_notes(&self) -> Vec<Note> {
        let Some(notes) = self.notes() else {
            return vec![];
        };
        let len = self.length.0;
        let clip_to = |n: &Note, offset: u64, limit: u64| -> Option<Note> {
            let pos = n.pos.0 + offset;
            let end = (n.pos.0 + n.dur.0).min(limit) + offset;
            let end = end.min(len);
            (pos < len && end > pos).then(|| Note {
                pos: Tick(pos),
                dur: Tick(end - pos),
                ..n.clone()
            })
        };
        match self.loop_len() {
            None => notes
                .iter()
                .filter_map(|n| clip_to(n, 0, u64::MAX))
                .collect(),
            Some(l) => {
                let l = l.0;
                let mut out = Vec::new();
                let mut offset = 0;
                while offset < len {
                    out.extend(
                        notes
                            .iter()
                            .filter(|n| n.pos.0 < l)
                            .filter_map(|n| clip_to(n, offset, l)),
                    );
                    offset += l;
                }
                out
            }
        }
    }
}

pub(crate) fn sort_notes(notes: &mut [Note]) {
    notes.sort_by(|a, b| (a.pos, a.pitch, &a.id).cmp(&(b.pos, b.pitch, &b.id)));
}
