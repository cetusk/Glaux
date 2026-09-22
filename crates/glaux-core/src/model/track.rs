//! トラック、楽器デバイス、エフェクト。
//!
//! 楽器もエフェクトも [`PluginSource`] で「内蔵 / CLAP / サンプラー」を切り替える。
//! 今は内蔵だけ実装し、外部音源のための枠を先に用意しておく。

use super::{AutomationLane, Clip, ParamMap};
use crate::id::{AssetId, ClipId, FxId, TrackId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    Midi,
    Audio,
}

/// 楽器・エフェクトの実体がどこにあるか。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginSource {
    /// 内蔵(name でレジストリを引く)
    Builtin { name: String },
    /// CLAP プラグイン。`state` はプラグイン固有の不透明データ(base64)。
    Clap {
        plugin_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<String>,
    },
    /// 単一サンプルを鳴らすサンプラー
    Sampler { asset: AssetId },
    /// SoundFont(.sf2)のプリセット。`soundfont` はライブラリフォルダ
    /// (`~/.config/glaux/soundfonts/`)内のファイル名
    Sf2 {
        soundfont: String,
        bank: u16,
        preset: u16,
    },
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Device {
    #[serde(flatten)]
    pub source: PluginSource,
    #[serde(default)]
    pub params: ParamMap,
}

impl Device {
    pub fn builtin(name: impl Into<String>) -> Self {
        Device {
            source: PluginSource::Builtin { name: name.into() },
            params: ParamMap::new(),
        }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Effect {
    pub id: FxId,
    #[serde(flatten)]
    pub source: PluginSource,
    #[serde(default)]
    pub bypass: bool,
    #[serde(default)]
    pub params: ParamMap,
}

impl Effect {
    pub fn builtin(id: FxId, name: impl Into<String>) -> Self {
        Effect {
            id,
            source: PluginSource::Builtin { name: name.into() },
            bypass: false,
            params: ParamMap::new(),
        }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub name: String,
    pub kind: TrackKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub solo: bool,
    #[serde(default)]
    pub volume_db: f32,
    /// -1.0 (L) ..= 1.0 (R)
    #[serde(default)]
    pub pan: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<Device>,
    #[serde(default)]
    pub effects: Vec<Effect>,
    /// (start, id) 昇順を保つ
    #[serde(default)]
    pub clips: Vec<Clip>,
    #[serde(default)]
    pub automation: Vec<AutomationLane>,
}

impl Track {
    pub fn new(id: TrackId, name: impl Into<String>, kind: TrackKind) -> Self {
        Track {
            id,
            name: name.into(),
            kind,
            color: None,
            mute: false,
            solo: false,
            volume_db: 0.0,
            pan: 0.0,
            device: None,
            effects: vec![],
            clips: vec![],
            automation: vec![],
        }
    }

    pub fn clip(&self, id: &ClipId) -> Option<&Clip> {
        self.clips.iter().find(|c| &c.id == id)
    }

    pub fn clip_mut(&mut self, id: &ClipId) -> Option<&mut Clip> {
        self.clips.iter_mut().find(|c| &c.id == id)
    }

    pub fn effect_index(&self, id: &FxId) -> Option<usize> {
        self.effects.iter().position(|e| &e.id == id)
    }

    pub(crate) fn sort_clips(&mut self) {
        self.clips
            .sort_by(|a, b| (a.start, &a.id).cmp(&(b.start, &b.id)));
    }
}

#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct MasterBus {
    #[serde(default)]
    pub volume_db: f32,
    #[serde(default)]
    pub effects: Vec<Effect>,
}
