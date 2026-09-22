//! 楽器の種別ディスパッチ。glaux-engine のボイスプールが保持する型。
//!
//! すべて `Copy` 可能な小さい値型で、`next` / `note_off` / `finished` は
//! アロケーションしない(RT セーフ)。

use crate::drum::{DrumParams, DrumVoice};
use crate::multi::{MultiSamplerParams, MultiVoice};
use crate::pluck::{PluckParams, PluckVoice};
use crate::sampler::{SamplerParams, SamplerVoice};
use crate::subtractive::{SubtractiveParams, SubtractiveVoice};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InstrumentKind {
    Subtractive,
    Drum,
    Pluck,
    Sampler,
    Sf2,
}

/// トラックごとに焼き込まれたパラメータ。
/// Sampler は波形の `Arc` を持つため Copy ではない(clone は参照カウントのみで
/// アロケーションしないので、オーディオスレッドでも安全)。
#[derive(Clone, Debug, PartialEq)]
pub enum InstrumentParams {
    Subtractive(SubtractiveParams),
    Drum(DrumParams),
    Pluck(PluckParams),
    Sampler(SamplerParams),
    Sf2(MultiSamplerParams),
}

impl Default for InstrumentParams {
    fn default() -> Self {
        crate::params::bake_instrument(None).1
    }
}

/// 発音中の 1 ボイス。
/// PluckVoice はディレイライン(固定長バッファ)を内包するため他より大きいが、
/// ボイス起動はオーディオスレッド上なので Box(アロケーション)にはできない。
/// プールは起動時に固定容量で確保されるためメモリ増は既知・有限。
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug)]
pub enum VoiceState {
    Subtractive(SubtractiveVoice),
    Drum(DrumVoice),
    Pluck(PluckVoice),
    Sampler(SamplerVoice),
    Sf2(MultiVoice),
}

impl VoiceState {
    pub fn start(
        params: &InstrumentParams,
        freq: f32,
        pitch: u8,
        vel: f32,
        articulation: glaux_core::Articulation,
        sample_rate: f32,
    ) -> VoiceState {
        match params {
            InstrumentParams::Subtractive(p) => VoiceState::Subtractive(SubtractiveVoice::start(
                p,
                freq,
                vel,
                articulation,
                sample_rate,
            )),
            InstrumentParams::Drum(p) => {
                // ドラムは音程楽器ほど奏法の情報がない。アクセントの強調だけ反映する
                let vel = if articulation == glaux_core::Articulation::Accent {
                    (vel * 1.3).min(1.0)
                } else {
                    vel
                };
                VoiceState::Drum(DrumVoice::start(p, pitch, vel, sample_rate))
            }
            InstrumentParams::Pluck(p) => {
                VoiceState::Pluck(PluckVoice::start(p, freq, vel, articulation, sample_rate))
            }
            InstrumentParams::Sampler(p) => VoiceState::Sampler(SamplerVoice::start(
                p,
                pitch,
                vel,
                articulation,
                sample_rate,
            )),
            InstrumentParams::Sf2(p) => {
                VoiceState::Sf2(MultiVoice::start(p, pitch, vel, articulation, sample_rate))
            }
        }
    }

    /// ノートのピッチカーブを付ける(ドラムは無視)。発音直後に呼ぶ。
    pub fn set_curve(&mut self, curve: &crate::expr::PitchCurve) {
        match self {
            VoiceState::Subtractive(v) => v.expr.set_curve(curve),
            VoiceState::Pluck(v) => v.expr.set_curve(curve),
            VoiceState::Sampler(v) => v.expr.set_curve(curve),
            VoiceState::Sf2(v) => v.expr.set_curve(curve),
            VoiceState::Drum(_) => {}
        }
    }

    /// 1 サンプル生成。`params` はボイス生成時と同じ楽器種であること
    /// (種別が変わるデータ差し替え時はエンジンがボイスを作り直す)。
    pub fn next(&mut self, params: &InstrumentParams) -> f32 {
        match (self, params) {
            (VoiceState::Subtractive(v), InstrumentParams::Subtractive(p)) => v.next(p),
            (VoiceState::Drum(v), InstrumentParams::Drum(p)) => v.next(p),
            (VoiceState::Pluck(v), InstrumentParams::Pluck(p)) => v.next(p),
            (VoiceState::Sampler(v), InstrumentParams::Sampler(p)) => v.next(p),
            (VoiceState::Sf2(v), InstrumentParams::Sf2(p)) => v.next(p),
            _ => 0.0,
        }
    }

    pub fn note_off(&mut self) {
        match self {
            VoiceState::Subtractive(v) => v.note_off(),
            VoiceState::Drum(v) => v.note_off(),
            VoiceState::Pluck(v) => v.note_off(),
            VoiceState::Sampler(v) => v.note_off(),
            VoiceState::Sf2(v) => v.note_off(),
        }
    }

    pub fn finished(&self, params: &InstrumentParams) -> bool {
        match (self, params) {
            (VoiceState::Subtractive(v), _) => v.finished(),
            (VoiceState::Drum(v), InstrumentParams::Drum(p)) => v.finished(p),
            (VoiceState::Drum(_), _) => true,
            (VoiceState::Pluck(v), _) => v.finished(),
            (VoiceState::Sampler(v), _) => v.finished(),
            (VoiceState::Sf2(v), _) => v.finished(),
        }
    }
}
