//! # glaux-dsp
//!
//! 内蔵楽器デバイスと、その [`ParamSpec`](glaux_core::ParamSpec) レジストリ。
//!
//! - すべてのボイスは **RT セーフ**(アロケーション・ロックなし)。
//!   glaux-engine のオーディオスレッドから 1 サンプルずつ呼ばれる
//! - パラメータの意味情報(範囲・単位・**聴感上の効果を書いた説明**)はここが持ち、
//!   MCP の `list_params` がそのまま AI に返す。説明文の質が AI の操作精度を決める
//! - ファイル(`project.json`)には値だけが載る。[`bake_instrument`] が
//!   「スペックのデフォルト + 上書き値」を再生用の構造体に焼き込む
//!
//! 現在の内蔵楽器:
//! - `subtractive`: 減算方式シンセ(PolyBLEP オシレータ + SVF ローパス + ADSR)
//! - `drum`: ドラムシンセ(MIDI ノート番号でキック/スネア/ハイハット等を弾き分け)
//! - `pluck`: 撥弦の物理モデル(Karplus-Strong。ギター/ベース/ハープ)
//! - `sampler`: 単一サンプル再生(ワンショット。実録の質感)

mod drum;
mod effects;
mod expr;
mod params;
mod pluck;
mod sampler;
mod subtractive;
mod voice;

pub use drum::DrumParams;
pub use effects::{bake_effect, effect_catalog, effect_params_spec, EffectParams, EffectState};
pub use params::{
    bake_instrument, bake_sampler, instrument_catalog, instrument_params, InstrumentInfo,
};
pub use pluck::PluckParams;
pub use sampler::{SampleData, SamplerParams, SamplerVoice};
pub use subtractive::{SubtractiveParams, Waveform};
pub use voice::{InstrumentKind, InstrumentParams, VoiceState};

/// device 未設定トラックに使う既定の楽器名。
pub const DEFAULT_INSTRUMENT: &str = "subtractive";
