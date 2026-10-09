use crate::id::{AssetId, ClipId, EntryId, FxId, NoteId, PlanId, TrackId};
use crate::model::ParamPath;
use crate::time::{Tick, TimeError};

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("track not found: {0}")]
    TrackNotFound(TrackId),
    #[error("clip not found: {0}")]
    ClipNotFound(ClipId),
    #[error("note {note} not found in clip {clip}")]
    NoteNotFound { clip: ClipId, note: NoteId },
    #[error("effect not found: {0}")]
    EffectNotFound(FxId),
    #[error("asset not found: {0}")]
    AssetNotFound(AssetId),
    #[error("plan not found: {0}")]
    PlanNotFound(PlanId),
    #[error("invalid plan: {0}")]
    InvalidPlan(String),
    #[error("history entry not found: {0}")]
    EntryNotFound(EntryId),
    #[error("checkpoint not found: {0}")]
    CheckpointNotFound(String),
    #[error("track {0} has no device")]
    NoDevice(TrackId),
    #[error("unknown parameter: {0}")]
    UnknownParam(ParamPath),
    #[error("parameter {0} is not set")]
    ParamNotSet(ParamPath),
    #[error("duplicate id: {0}")]
    DuplicateId(String),
    #[error("id mismatch: expected {expected}, got {got}")]
    IdMismatch { expected: String, got: String },
    #[error("clip {0} is not a MIDI clip")]
    NotMidiClip(ClipId),
    #[error("clip {0} is not an audio clip")]
    NotAudioClip(ClipId),
    #[error("split point {at:?} is outside clip {clip}")]
    InvalidSplit { clip: ClipId, at: Tick },
    #[error("index {index} out of range (len {len})")]
    IndexOutOfRange { index: usize, len: usize },
    #[error("clip kind does not match track kind")]
    ClipKindMismatch,
    #[error("value out of range: {0}")]
    OutOfRange(String),
    /// 履歴の外で変えられるのは聴き方(ミュート・ソロ)だけ
    #[error("only mute / solo can change outside the history: {0}")]
    NotListenOnly(String),
    /// AI の編集が固定の音にだけ当たって、何も変えなかった
    #[error(
        "固定の音への変更だけだったので、何も変えていません(固定は人が外すまで AI は変えない): {0}"
    )]
    Locked(String),
    #[error("invalid effect links: {0}")]
    InvalidLinks(String),
    #[error(transparent)]
    Time(#[from] TimeError),
    #[error("batch failed at command #{index}: {source}")]
    Batch {
        index: usize,
        #[source]
        source: Box<CoreError>,
    },
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, CoreError>;
