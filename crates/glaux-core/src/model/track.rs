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
    /// バス(リターン)。クリップを持たず、他のトラックのセンドを受けてエフェクト → 音量/パン → マスターへ
    Bus,
}

/// センド: トラックの音を分けてバスへ送る(共有のリバーブ・ディレイ等)。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Send {
    /// 送り先のバストラック
    pub target: TrackId,
    /// 送る量(dB)
    pub level_db: f32,
    /// true = フェーダー・パンの前から送る(既定は後 = トラックの音量に追従)
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pre_fader: bool,
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
    /// 表示名・外してあるか・メモ・ノード表示での位置(音には関係しない。外してあるものは鳴らさない)
    #[serde(flatten, default)]
    pub ui: EffectUi,
}

/// エフェクトの表示と置き場所。ノード表示(カード + 線)で使う。
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct EffectUi {
    /// ユーザーが付けた名前(省略で種類の名前。例「サビの歪み」)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// 線から外して、わきに置いてある。設定は残り、音は通らない(バイパスと違い、並びに居座らない)
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub parked: bool,
    /// メモ(なぜ取っておいたかなど)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// ノード表示での位置 [x, y](わきに置いたカード。見た目だけ)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pos: Option<[f32; 2]>,
}

impl Effect {
    pub fn builtin(id: FxId, name: impl Into<String>) -> Self {
        Effect {
            id,
            source: PluginSource::Builtin { name: name.into() },
            bypass: false,
            params: ParamMap::new(),
            ui: EffectUi::default(),
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
    /// エフェクトのつながり(ノード表示の線)。無ければ並び順の直列([`super::routing`])
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fx_links: Option<Vec<super::routing::FxLink>>,
    /// (start, id) 昇順を保つ
    #[serde(default)]
    pub clips: Vec<Clip>,
    #[serde(default)]
    pub automation: Vec<AutomationLane>,
    /// センド(送り先の ID 順)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sends: Vec<Send>,
    /// ポルタメントで滑る時間(ms)。省略時 150ms。`track/glide_ms` で設定
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glide_ms: Option<f32>,
    /// レガートのつなぎ目の長さ(ms)。省略時 30ms。`track/legato_ms` で設定
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legato_ms: Option<f32>,
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
            fx_links: None,
            clips: vec![],
            automation: vec![],
            sends: vec![],
            glide_ms: None,
            legato_ms: None,
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

    /// 実際に使うエフェクトのつながり(表が無い・壊れているときは並び順の直列)
    pub fn links(&self) -> Vec<super::routing::FxLink> {
        effective(&self.effects, self.fx_links.as_deref())
    }

    pub(crate) fn sort_clips(&mut self) {
        self.clips
            .sort_by(|a, b| (a.start, &a.id).cmp(&(b.start, &b.id)));
    }
}

/// 表があって正しければ表を、無い・壊れているなら並び順の直列を返す
fn effective(
    effects: &[Effect],
    links: Option<&[super::routing::FxLink]>,
) -> Vec<super::routing::FxLink> {
    let links = links.filter(|l| super::routing::validate_links(effects, l).is_ok());
    super::routing::effective_links(effects, links)
}

impl MasterBus {
    /// 実際に使うマスターのエフェクトのつながり
    pub fn links(&self) -> Vec<super::routing::FxLink> {
        effective(&self.effects, self.fx_links.as_deref())
    }
}

#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct MasterBus {
    #[serde(default)]
    pub volume_db: f32,
    #[serde(default)]
    pub effects: Vec<Effect>,
    /// マスターのエフェクトのつながり。無ければ並び順の直列
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fx_links: Option<Vec<super::routing::FxLink>>,
    /// マスターのオートメーション。対象は `track/volume_db`(マスター音量)と
    /// `fx/<マスターのエフェクト ID>/<パラメータ>`
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub automation: Vec<AutomationLane>,
}
