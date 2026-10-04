//! # glaux-core
//!
//! DAW のプロジェクトモデル・コマンド・履歴を担う純粋なデータ層。
//! オーディオ処理にも UI にも依存しない。
//!
//! 設計の柱:
//! - すべての時間は [`Tick`](time::Tick)(PPQ = 960)で表す。音声は拍上に置き、
//!   再生時に [`TempoMap`](time::TempoMap) で秒に変換する。
//! - すべての編集は [`Command`] として表現し、人間の UI も AI も同じ経路を通る。
//! - [`Project::apply`] は逆コマンドを返す(逆コマンド方式の Undo)。
//! - [`Session`] が Git ライクな履歴(undo/redo/checkpoint/revert/replay)を提供する。
//! - ID は生成側が決める(コマンドは決定的)。

pub mod aftercare;
pub mod apply;
pub mod arp;
pub mod arrange;
pub mod bassline;
pub mod chord;
pub mod command;
pub mod comp;
pub mod critique;
pub mod designcheck;
pub mod drums;
pub mod error;
pub mod gesture;
pub mod groove;
pub mod harmony;
pub mod history;
pub mod id;
pub mod made;
pub mod melexpr;
pub mod melgen;
pub mod melody;
pub mod melplan;
pub mod melriff;
pub mod melstruct;
pub mod meter;
pub mod model;
pub mod motif;
pub mod ornament;
pub mod plan;
pub mod progressions;
pub mod rhythm;
pub mod shape;
pub mod technique;
pub mod time;
pub mod transform;
pub mod validate;
pub mod voicing;

pub use apply::{Applied, Change};
pub use command::{Command, EffectProp, NoteChange, Target, TrackProp};
pub use error::CoreError;
pub use history::{
    Author, Document, EntryNote, History, HistoryCommand, HistoryEntry, HistoryPoint, Measure,
    RevertResult, Session, Trigger,
};
pub use id::{AssetId, ClipId, EntryId, FxId, NoteId, PlanId, SectionId, TrackId};
pub use model::*;
pub use time::{TempoEvent, TempoMap, Tick, TimeSigEvent, MAX_TICK, PPQ};
