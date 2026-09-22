//! クリップ。MIDI クリップと音声クリップを `kind` で区別する。
//!
//! 音声クリップも拍(Tick)上に置く。元音声は変更せず、切り出し位置・ゲイン・フェード・
//! ストレッチをデータとして重ねる非破壊編集。

use crate::id::{AssetId, ClipId, NoteId};
use crate::time::Tick;
use serde::{Deserialize, Serialize};

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
    /// テンポに追従(将来実装)
    Follow { original_bpm: f64 },
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
}

pub(crate) fn sort_notes(notes: &mut [Note]) {
    notes.sort_by(|a, b| (a.pos, a.pitch, &a.id).cmp(&(b.pos, b.pitch, &b.id)));
}
