//! プロジェクトモデル。`project.json` にそのまま対応する。

mod asset;
mod automation;
mod clip;
mod param;
pub mod routing;
mod track;

pub use asset::Asset;
pub use automation::{AutomationLane, AutomationPoint, Curve};
pub use clip::{
    check_condition, check_expr_curve, check_pitch_curve, check_vibrato, Articulation, Clip,
    ClipContent, CurvePoint, CurveShape, Note, NoteCondition, PitchPoint, Stretch, Vibrato,
    BRIGHTNESS_RANGE, GLIDE_MS_RANGE, LEGATO_MS_RANGE, MAX_EXPR_POINTS, MAX_PITCH_CENTS,
    MAX_PITCH_POINTS, VOLUME_CURVE_DB,
};
pub use param::{ParamMap, ParamPath, ParamRange, ParamSpec, ParamValue};
pub use routing::{FxIoPos, FxLink, FxNode};
pub use track::{
    check_layers, check_macros, check_modulators, Device, Effect, EffectUi, Layer, LfoShape, Macro,
    MacroTarget, MasterBus, Modulator, PluginSource, Send, Track, TrackKind, MAX_LAYERS,
    MAX_MACROS, MAX_MODULATORS,
};

pub(crate) use clip::sort_notes;

use crate::id::{AssetId, ClipId, FxId, TrackId};
use crate::time::{TempoMap, Tick, TimeSigEvent, PPQ};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const FORMAT_NAME: &str = "glaux";
/// 形式の版。2: 拍子の拍のまとまり(`TimeSigEvent.grouping`)。1 の曲はそのまま読める
pub const FORMAT_VERSION: u32 = 2;

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Meta {
    pub title: String,
    pub created: DateTime<Utc>,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Project {
    pub format: String,
    pub version: u32,
    pub ppq: u64,
    pub meta: Meta,
    #[serde(default)]
    pub tempo_map: TempoMap,
    #[serde(default = "default_time_sig")]
    pub time_sig_map: Vec<TimeSigEvent>,
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub master: MasterBus,
    /// BTreeMap で JSON の順序を安定させる
    #[serde(default)]
    pub assets: BTreeMap<AssetId, Asset>,
    /// 曲の構成マーカー(intro / verse / chorus 等)。tick 昇順。
    /// 各セクションはそのマーカーの tick から次のマーカーの手前まで
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<SectionMarker>,
}

/// 曲構成のマーカー。「サビだけ盛り上げて」のような構造単位の指示に使う。
/// 曲の計画書(MCP の set_song_plan)の中身(盛り上がり・鳴らすトラック・役割)も持つ。点検が計画と実際を比べる
#[derive(Clone, PartialEq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SectionMarker {
    pub tick: Tick,
    pub name: String,
    /// 計画の盛り上がり 0〜10(critique_arrangement の区間の energy と同じ目盛り)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub energy: Option<f32>,
    /// 計画で、この区間に鳴らすトラックの名前(空 = 決めていない)。トラックを作る前に書けるよう名前で持つ
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tracks: Vec<String>,
    /// 計画の役割・意図のメモ(例「キックとベースを抜いてパッドだけ」)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn default_time_sig() -> Vec<TimeSigEvent> {
    vec![TimeSigEvent {
        tick: Tick::ZERO,
        num: 4,
        den: 4,
        grouping: None,
    }]
}

impl Default for Project {
    fn default() -> Self {
        Self::new("Untitled")
    }
}

impl Project {
    pub fn new(title: impl Into<String>) -> Self {
        Project {
            format: FORMAT_NAME.into(),
            version: FORMAT_VERSION,
            ppq: PPQ,
            meta: Meta {
                title: title.into(),
                created: Utc::now(),
            },
            tempo_map: TempoMap::default(),
            time_sig_map: default_time_sig(),
            tracks: vec![],
            master: MasterBus::default(),
            assets: BTreeMap::new(),
            sections: vec![],
        }
    }

    // ---- lookup -------------------------------------------------------

    pub fn track_index(&self, id: &TrackId) -> Option<usize> {
        self.tracks.iter().position(|t| &t.id == id)
    }

    pub fn track(&self, id: &TrackId) -> Option<&Track> {
        self.tracks.iter().find(|t| &t.id == id)
    }

    pub fn track_mut(&mut self, id: &TrackId) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| &t.id == id)
    }

    /// クリップの (トラック index, クリップ index)
    pub fn clip_location(&self, id: &ClipId) -> Option<(usize, usize)> {
        self.tracks
            .iter()
            .enumerate()
            .find_map(|(ti, t)| t.clips.iter().position(|c| &c.id == id).map(|ci| (ti, ci)))
    }

    pub fn clip(&self, id: &ClipId) -> Option<(&Track, &Clip)> {
        let (ti, ci) = self.clip_location(id)?;
        Some((&self.tracks[ti], &self.tracks[ti].clips[ci]))
    }

    /// エフェクトの (トラック index, エフェクト index)
    pub fn effect_location(&self, id: &FxId) -> Option<(usize, usize)> {
        self.tracks
            .iter()
            .enumerate()
            .find_map(|(ti, t)| t.effect_index(id).map(|ei| (ti, ei)))
    }

    pub fn all_clip_ids(&self) -> impl Iterator<Item = &ClipId> {
        self.tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(|c| &c.id))
    }

    pub fn all_effect_ids(&self) -> impl Iterator<Item = &FxId> {
        self.tracks
            .iter()
            .flat_map(|t| t.effects.iter().map(|e| &e.id))
            .chain(self.master.effects.iter().map(|e| &e.id))
    }

    /// マスターバスのエフェクトの位置。
    pub fn master_effect_index(&self, id: &FxId) -> Option<usize> {
        self.master.effects.iter().position(|e| &e.id == id)
    }

    /// プロジェクト全体の終端 Tick
    pub fn end(&self) -> Tick {
        self.tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(|c| c.end()))
            .max()
            .unwrap_or(Tick::ZERO)
    }

    // ---- io -----------------------------------------------------------

    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json(s: &str) -> serde_json::Result<Self> {
        serde_json::from_str(s)
    }
}
