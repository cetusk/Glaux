//! # glaux-engine
//!
//! リアルタイムオーディオエンジン(MVP)。
//!
//! 構成(`docs/HANDOFF.md` のプロセス構成に対応):
//! - [`data`]: `Project` → 再生データ(絶対サンプルに展開・ソート済みイベント列)。
//!   **UI スレッドで**構築する
//! - [`render`]: RT セーフなレンダラ。アロケーション・ロックなし。
//!   音源は `glaux-dsp` の内蔵楽器 5 種 + 音声クリップの直接再生
//! - [`output`]: cpal ストリームを専用スレッドで保持し、[`EngineHandle`] を UI に渡す
//! - [`plugins`]: CLAP プラグイン(外部の音源)の管理(プラグインのスレッド・窓口の受け渡し)
//! - [`record`]: 入力デバイスからの録音(リングバッファ → WAV)
//! - [`midi`]: MIDI キーボード入力(ライブ発音キュー・MIDI 録音)
//! - [`separate`]: 内蔵の音源分離(HPSS。打楽器 / 音程楽器)
//! - [`sf2`]: SoundFont の読み込みとゾーン構築
//!
//! 割り切り(将来課題):
//! - 音声クリップのテンポ追従(Stretch::Follow)は WSOLA でオフライン伸縮(`SampleBank` にキャッシュ)。
//!   伸縮の品質は声・単音・リズム素材向け(和音の多い素材で大きく伸ばすと揺れが出る)

pub mod analyze;
pub mod calibrate;
pub mod data;
pub mod export;
pub mod loudness;
pub mod mastering;
pub mod midi;
pub mod monitor;
pub mod output;
pub mod plugins;
pub mod record;
pub mod render;
pub mod separate;
pub mod sf2;
pub mod sound_match;
pub mod timbre;
pub mod timeline;
pub mod transcribe;

pub use analyze::{
    analyze_mix, analyze_project, analyze_project_tracks, compare_projects, Analysis, Comparison,
    MaskingIssue, MixAnalysis, TrackAnalysis,
};
pub use data::{
    build_playback_data, load_wav, load_wav_mono, wave_peaks, PlaybackData, SampleBank,
};
pub use export::{
    export_audio, export_wav, limit_peaks, render, render_project, render_project_range,
    render_stem, render_track_note, write_wav, ExportError, ExportOptions, ExportReport,
};
pub use midi::list_midi_inputs;
pub use output::{
    list_devices, start_engine, DeviceList, EngineError, EngineHandle, MidiRecordOutcome,
    RecordOutcome,
};
pub use record::RecordResult;
