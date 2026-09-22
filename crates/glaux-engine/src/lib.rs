//! # glaux-engine
//!
//! リアルタイムオーディオエンジン(MVP)。
//!
//! 構成(`docs/HANDOFF.md` のプロセス構成に対応):
//! - [`data`]: `Project` → 再生データ(絶対サンプルに展開・ソート済みイベント列)。
//!   **UI スレッドで**構築する
//! - [`render`]: RT セーフなレンダラ。アロケーション・ロックなし。
//!   音源は内蔵ポリシンセ 1 種(サイン波)。`glaux-dsp` 実装後に置き換える
//! - [`output`]: cpal ストリームを専用スレッドで保持し、[`EngineHandle`] を UI に渡す
//!
//! MVP の割り切り(将来課題):
//! - 再生位置はサンプルで保持(再生中のテンポ変更で音楽的位置が僅かにずれる)
//! - 音声クリップ・エフェクト・オートメーションは未対応
//! - ループクリップは 1 回だけ再生

pub mod analyze;
pub mod data;
pub mod export;
pub mod output;
pub mod render;

pub use analyze::{analyze_project, analyze_project_tracks, Analysis, TrackAnalysis};
pub use data::{build_playback_data, PlaybackData, SampleBank};
pub use export::{export_wav, render_project, ExportError};
pub use output::{start_engine, EngineError, EngineHandle};
