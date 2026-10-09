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
//! - `sampler`: 単一サンプル再生(実録の質感。ループ・ADSR・フィルタ・ステレオ・スライス・テンポ追従)
//! - `sf2`: マルチサンプラー(SoundFont のゾーンを再生。GM 音源一式が鳴る)
//! - `fm`: FM シンセ(2 オペレーター + フィードバック。エレピ・ベル)
//! - `wavetable`: ウェーブテーブルシンセ(波形の並びを行き来して音色を動かす)
//! - `fm4`: 4 オペレーターの FM シンセ(8 アルゴリズム。DX のエレピ・ベル・ブラス・オルガン)
//! - `granular`: グラニュラー(音声素材から粒を切り出して重ねる。パッド・きらめき・時間を止めた声)
//! - `additive`: 加算合成(正弦波の部分音を最大 64 本。傾き・奇数偶数・フォルマント・部分音ごとの減衰)

mod additive;
pub mod convolver;
mod drum;
mod dynamics;
mod effects;
mod expr;
mod fm;
mod fm4;
mod granular;
pub mod limiter;
mod modfx;
mod multi;
mod oversample;
mod params;
mod pitch;
mod pluck;
pub mod resonance;
mod reverb;
mod sampler;
pub mod stretch;
pub mod string_pool;
mod studio;
mod subtractive;
pub mod tone;
mod voice;
mod wave;
mod wavetable;
mod width;
pub mod wtedit;
mod wtexpr;

pub use additive::{AdditiveParams, AdditiveVoice};
pub use drum::{DrumKit, DrumParams};
pub use effects::{
    bake_effect, convolution_length, effect_catalog, effect_params_spec, ConvParams, EffectParams,
    EffectState, SvfCoeffs, SvfState,
};
pub use expr::{articulation_cents, articulation_moves_pitch, NoteShape, PitchCurve, VibratoSpec};
pub use fm::{FmParams, FmVoice};
pub use fm4::{Fm4Op, Fm4Params, Fm4Voice};
pub use granular::{GrainWindow, GranularParams, GranularVoice};
pub use multi::{MultiSamplerParams, MultiVoice, Zone, ZoneEnv, ZoneMod, ZonePlay, MAX_LAYERS};
pub use params::{
    articulations_for, bake_granular, bake_instrument, bake_sampler, bake_sf2, instrument_catalog,
    instrument_params, parse_key_adjust, sampler_orig_bpm, ArticulationInfo, InstrumentInfo,
    KeyAdjust,
};
pub use pluck::PluckParams;
pub use sampler::{detect_slices, hermite, Mips, SampleData, SamplerParams, SamplerVoice};
pub use subtractive::{NoiseColor, SubtractiveParams, Waveform};
pub use voice::{InstrumentKind, InstrumentParams, VoiceState};
pub use wave::Wave;
pub use wavetable::{
    builtin_cycle, cycles_from_audio, UserTable, UserTableRef, WavetableParams, WavetableVoice,
    MAX_USER_FRAMES, TABLE_NAMES, WARP_NAMES,
};

/// device 未設定トラックに使う既定の楽器名。
pub const DEFAULT_INSTRUMENT: &str = "subtractive";
