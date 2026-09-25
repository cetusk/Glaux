//! 編集コマンド。UI も AI も MCP もすべてこの型を通してプロジェクトを変更する。
//!
//! 設計上の約束:
//! - **決定的**: 新規 ID はコマンドを作る側が渡す(内部生成しない)。同じコマンド列を
//!   リプレイすれば同じ結果になり、AI 側が ID を予測できる。
//! - **絶対値**: 「+480 tick 動かす」のような相対操作は持たない。相対操作は UI/MCP 層で
//!   絶対値に変換してから流す。逆コマンドが自明に作れる。
//! - `Batch` が Undo の単位。AI が数百コマンドを 1 回で戻せる。

use crate::id::{AssetId, ClipId, FxId, NoteId, TrackId};
use crate::model::{
    Articulation, Asset, AutomationPoint, Clip, Device, Effect, Note, ParamPath, ParamValue,
    PitchPoint, SectionMarker, Stretch, Track,
};
use crate::time::{TempoEvent, Tick, TimeSigEvent};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "prop", content = "value", rename_all = "snake_case")]
pub enum TrackProp {
    Name(String),
    Color(Option<String>),
    Mute(bool),
    Solo(bool),
    VolumeDb(f32),
    Pan(f32),
}

/// ノートの部分更新。`None` のフィールドは変更しない。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct NoteChange {
    pub id: NoteId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pos: Option<Tick>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dur: Option<Tick>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vel: Option<u8>,
    /// 奏法の変更。`"normal"` を渡すと通常に戻す
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub articulation: Option<Articulation>,
    /// ピッチカーブの差し替え。空配列でカーブ削除
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch_curve: Option<Vec<PitchPoint>>,
    /// ポルタメントで滑る時間(ms)。0 以下でノート個別の指定を消す(トラックの値に戻る)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glide_ms: Option<f32>,
}

impl NoteChange {
    pub fn new(id: NoteId) -> Self {
        NoteChange {
            id,
            pos: None,
            dur: None,
            pitch: None,
            vel: None,
            articulation: None,
            pitch_curve: None,
            glide_ms: None,
        }
    }
    pub fn pos(mut self, v: Tick) -> Self {
        self.pos = Some(v);
        self
    }
    pub fn dur(mut self, v: Tick) -> Self {
        self.dur = Some(v);
        self
    }
    pub fn pitch(mut self, v: u8) -> Self {
        self.pitch = Some(v);
        self
    }
    pub fn vel(mut self, v: u8) -> Self {
        self.vel = Some(v);
        self
    }
    pub fn articulation(mut self, v: Articulation) -> Self {
        self.articulation = Some(v);
        self
    }
    pub fn pitch_curve(mut self, v: Vec<PitchPoint>) -> Self {
        self.pitch_curve = Some(v);
        self
    }
    pub fn glide_ms(mut self, v: f32) -> Self {
        self.glide_ms = Some(v);
        self
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Command {
    // ---- track ----
    AddTrack {
        track: Track,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
    },
    RemoveTrack {
        id: TrackId,
    },
    SetTrackProp {
        id: TrackId,
        #[serde(flatten)]
        prop: TrackProp,
    },
    MoveTrack {
        id: TrackId,
        to_index: usize,
    },

    // ---- clip ----
    AddClip {
        track: TrackId,
        clip: Clip,
    },
    RemoveClip {
        id: ClipId,
    },
    /// クリップ全体を差し替える(id は一致していること)
    ReplaceClip {
        id: ClipId,
        clip: Clip,
    },
    MoveClip {
        id: ClipId,
        start: Tick,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        track: Option<TrackId>,
    },
    ResizeClip {
        id: ClipId,
        length: Tick,
    },
    /// MIDI クリップのループ(繰り返し)を設定する。`loop_len` に繰り返す長さ(クリップ先頭から)を
    /// 指定するとループ ON、`null` で OFF。ループ中はクリップを伸ばすと中身が繰り返し鳴る。
    SetClipLoop {
        id: ClipId,
        #[serde(default)]
        loop_len: Option<Tick>,
    },
    /// 音声クリップのタイムストレッチ(テンポ追従)を設定する
    SetClipStretch {
        id: ClipId,
        stretch: Stretch,
    },
    /// `at`(絶対 Tick)で分割。右側が `new_id` になる。
    /// MIDI: 分割点をまたぐノートは左側で切り詰める。
    SplitClip {
        id: ClipId,
        at: Tick,
        new_id: ClipId,
    },

    // ---- note ----
    AddNotes {
        clip: ClipId,
        notes: Vec<Note>,
    },
    RemoveNotes {
        clip: ClipId,
        ids: Vec<NoteId>,
    },
    UpdateNotes {
        clip: ClipId,
        changes: Vec<NoteChange>,
    },

    // ---- param / device / effect ----
    SetParam {
        track: TrackId,
        path: ParamPath,
        value: ParamValue,
    },
    UnsetParam {
        track: TrackId,
        path: ParamPath,
    },
    SetDevice {
        track: TrackId,
        device: Option<Device>,
    },
    AddEffect {
        track: TrackId,
        effect: Effect,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
    },
    /// トラック・マスターのどちらのエフェクトでも削除できる
    RemoveEffect {
        id: FxId,
    },
    /// エフェクトの順番を変える(トラック・マスターのどちらでも。同じ列の中で `to_index` へ)
    MoveEffect {
        id: FxId,
        to_index: usize,
    },
    /// マスターバスにエフェクトを追加する(`index` 省略で末尾)
    AddMasterEffect {
        effect: Effect,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
    },
    /// マスターバスのエフェクトのパラメータ(path は `fx/<id>/<name>`)
    SetMasterParam {
        path: ParamPath,
        value: ParamValue,
    },
    UnsetMasterParam {
        path: ParamPath,
    },
    SetEffectBypass {
        id: FxId,
        bypass: bool,
    },
    /// センドを設定する(`level_db` 省略でそのセンドを外す)。送り元はバス以外、送り先はバス
    SetSend {
        track: TrackId,
        target: TrackId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        level_db: Option<f32>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        pre_fader: bool,
    },
    /// CLAP プラグインのエフェクトの状態(プラグイン固有の不透明データ、base64)。
    /// トラック・マスターのどちらのエフェクトでもよい。内蔵エフェクトには使えない
    SetEffectState {
        id: FxId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<String>,
    },

    // ---- automation ----
    /// レーン全体を差し替える。`points` が空ならレーンを削除。
    SetAutomationPoints {
        track: TrackId,
        target: ParamPath,
        points: Vec<AutomationPoint>,
    },
    /// マスターのオートメーションレーンを差し替える。`points` が空ならレーンを削除。
    /// `target` は `track/volume_db` か `fx/<マスターのエフェクト ID>/<パラメータ>`
    SetMasterAutomationPoints {
        target: ParamPath,
        points: Vec<AutomationPoint>,
    },

    // ---- global ----
    SetTempo {
        events: Vec<TempoEvent>,
    },
    SetTimeSig {
        events: Vec<TimeSigEvent>,
    },
    SetMasterVolume {
        volume_db: f32,
    },
    /// プロジェクトのタイトル(meta.title)を変更する
    SetTitle {
        title: String,
    },
    /// 曲の構成マーカー(intro / verse / chorus 等)を丸ごと置き換える
    SetSections {
        sections: Vec<SectionMarker>,
    },
    AddAsset {
        id: AssetId,
        asset: Asset,
    },
    RemoveAsset {
        id: AssetId,
    },

    // ---- composite ----
    Batch {
        commands: Vec<Command>,
        #[serde(default)]
        label: String,
    },
}

/// コマンドが触る対象。履歴の依存チェック(revert 時の衝突警告)に使う。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum Target {
    Track(TrackId),
    Clip(ClipId),
    Note(NoteId),
    Effect(FxId),
    Asset(AssetId),
    Tempo,
    TimeSig,
    Master,
    Meta,
    Sections,
}

impl Command {
    pub fn batch(label: impl Into<String>, commands: Vec<Command>) -> Self {
        Command::Batch {
            commands,
            label: label.into(),
        }
    }

    /// コマンドが静的に(プロジェクトを見ずに)分かる範囲で触る対象。
    pub fn targets(&self) -> BTreeSet<Target> {
        let mut out = BTreeSet::new();
        self.collect_targets(&mut out);
        out
    }

    fn collect_targets(&self, out: &mut BTreeSet<Target>) {
        use Command::*;
        use Target as T;
        match self {
            AddTrack { track, .. } => {
                out.insert(T::Track(track.id.clone()));
            }
            RemoveTrack { id } | SetTrackProp { id, .. } | MoveTrack { id, .. } => {
                out.insert(T::Track(id.clone()));
            }
            AddClip { track, clip } => {
                out.insert(T::Track(track.clone()));
                out.insert(T::Clip(clip.id.clone()));
            }
            RemoveClip { id }
            | ResizeClip { id, .. }
            | SetClipLoop { id, .. }
            | SetClipStretch { id, .. } => {
                out.insert(T::Clip(id.clone()));
            }
            ReplaceClip { id, clip } => {
                out.insert(T::Clip(id.clone()));
                out.insert(T::Clip(clip.id.clone()));
            }
            MoveClip { id, track, .. } => {
                out.insert(T::Clip(id.clone()));
                if let Some(t) = track {
                    out.insert(T::Track(t.clone()));
                }
            }
            SplitClip { id, new_id, .. } => {
                out.insert(T::Clip(id.clone()));
                out.insert(T::Clip(new_id.clone()));
            }
            AddNotes { clip, notes } => {
                out.insert(T::Clip(clip.clone()));
                out.extend(notes.iter().map(|n| T::Note(n.id.clone())));
            }
            RemoveNotes { clip, ids } => {
                out.insert(T::Clip(clip.clone()));
                out.extend(ids.iter().map(|n| T::Note(n.clone())));
            }
            UpdateNotes { clip, changes } => {
                out.insert(T::Clip(clip.clone()));
                out.extend(changes.iter().map(|c| T::Note(c.id.clone())));
            }
            SetParam { track, path, .. } | UnsetParam { track, path } => {
                out.insert(T::Track(track.clone()));
                if let ParamPath::Effect { id, .. } = path {
                    out.insert(T::Effect(id.clone()));
                }
            }
            SetDevice { track, .. } => {
                out.insert(T::Track(track.clone()));
            }
            AddEffect { track, effect, .. } => {
                out.insert(T::Track(track.clone()));
                out.insert(T::Effect(effect.id.clone()));
            }
            SetSend { track, target, .. } => {
                out.insert(T::Track(track.clone()));
                out.insert(T::Track(target.clone()));
            }
            RemoveEffect { id }
            | MoveEffect { id, .. }
            | SetEffectBypass { id, .. }
            | SetEffectState { id, .. } => {
                out.insert(T::Effect(id.clone()));
            }
            AddMasterEffect { effect, .. } => {
                out.insert(T::Master);
                out.insert(T::Effect(effect.id.clone()));
            }
            SetMasterParam { path, .. } | UnsetMasterParam { path } => {
                out.insert(T::Master);
                if let ParamPath::Effect { id, .. } = path {
                    out.insert(T::Effect(id.clone()));
                }
            }
            SetMasterAutomationPoints { target, .. } => {
                out.insert(T::Master);
                if let ParamPath::Effect { id, .. } = target {
                    out.insert(T::Effect(id.clone()));
                }
            }
            SetAutomationPoints { track, target, .. } => {
                out.insert(T::Track(track.clone()));
                if let ParamPath::Effect { id, .. } = target {
                    out.insert(T::Effect(id.clone()));
                }
            }
            SetTempo { .. } => {
                out.insert(T::Tempo);
            }
            SetTimeSig { .. } => {
                out.insert(T::TimeSig);
            }
            SetMasterVolume { .. } => {
                out.insert(T::Master);
            }
            SetTitle { .. } => {
                out.insert(T::Meta);
            }
            SetSections { .. } => {
                out.insert(T::Sections);
            }
            AddAsset { id, .. } | RemoveAsset { id } => {
                out.insert(T::Asset(id.clone()));
            }
            Batch { commands, .. } => {
                for c in commands {
                    c.collect_targets(out);
                }
            }
        }
    }
}
