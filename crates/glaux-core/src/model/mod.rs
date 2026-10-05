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
    check_layers, check_macros, check_modulators, routing_error, Device, Effect, EffectUi, Layer,
    LfoShape, Macro, MacroTarget, MasterBus, Modulator, PluginSource, Send, Track, TrackKind,
    MAX_LAYERS, MAX_MACROS, MAX_MODULATORS,
};

pub(crate) use clip::sort_notes;

use crate::id::{AssetId, ClipId, FxId, SectionId, TrackId};
use crate::time::{TempoMap, Tick, TimeSigEvent, PPQ};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const FORMAT_NAME: &str = "glaux";
/// 形式の版。2: 拍子の拍のまとまり(`TimeSigEvent.grouping`)。3: 区間の ID・盛り上がりの形・境目、音の固定、
/// AI が作ったときの指紋(`made`)。古い版の曲はそのまま読める
pub const FORMAT_VERSION: u32 = 3;

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
    /// クリップがどの計画の、どの版から作られたか(計画は曲とは別の文書 `plans.json` にあり、別の履歴を持つ)。
    /// 消したクリップの参照は残っていても無視する
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub plan_refs: BTreeMap<ClipId, crate::plan::PlanRef>,
    /// AI が作った・直したクリップの、その時の中身の指紋(小節ごと)。今の中身と違う小節は人が手で直した所
    /// ([`crate::made`])。消したクリップの記録は残っていても無視する
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub made: BTreeMap<ClipId, crate::made::Made>,
}

/// 曲構成のマーカー。「サビだけ盛り上げて」のような構造単位の指示に使う。
/// 区間の設計(盛り上がり・形・境目・鳴らすトラック・メモ)の持ち主は曲全体の計画(`plan::SongPlan::sections`)で、
/// 曲を読み出すときにここへ重ねる(点検・設計画面が計画と実際を比べる)。曲の履歴には位置と名前だけを残す
#[derive(Clone, PartialEq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SectionMarker {
    /// 区間の ID(計画が区間を指す。古い曲には無い。set_song_plan などが付ける)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<SectionId>,
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
    /// 区間の中の盛り上がりの形: [位置 0〜1, 値 0〜10] の点の列(位置の昇順)。空なら `energy` の平ら
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub curve: Vec<[f32; 2]>,
    /// 次の区間との境目(省略でつなぐ)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join: Option<SectionJoin>,
}

/// 区間の境目
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionJoin {
    /// つなぐ(この区間の終わりと次の頭を同じ値にそろえる)
    Smooth,
    /// 段差(急に変わる。ビルド → ドロップの落差など)
    Step,
}

/// 区間の盛り上がりの形の点の上限
pub const MAX_SECTION_CURVE_POINTS: usize = 32;

impl SectionMarker {
    /// 区間の中の位置 `t`(0〜1)の盛り上がり。形が無ければ `energy`
    pub fn energy_at(&self, t: f32) -> Option<f32> {
        let c = &self.curve;
        if c.is_empty() {
            return self.energy;
        }
        let t = t.clamp(0.0, 1.0);
        if t <= c[0][0] {
            return Some(c[0][1]);
        }
        for w in c.windows(2) {
            let ([a, va], [b, vb]) = (w[0], w[1]);
            if t <= b {
                let k = if b > a { (t - a) / (b - a) } else { 1.0 };
                return Some(va + (vb - va) * k);
            }
        }
        c.last().map(|p| p[1])
    }

    /// 区間の盛り上がりの平均(形があれば形の平均、無ければ `energy`)
    pub fn energy_mean(&self) -> Option<f32> {
        if self.curve.is_empty() {
            return self.energy;
        }
        const N: usize = 32;
        Some(
            (0..=N)
                .filter_map(|i| self.energy_at(i as f32 / N as f32))
                .sum::<f32>()
                / (N + 1) as f32,
        )
    }
}

/// 盛り上がりの形の検証(点の数・位置 0〜1 の昇順・値 0〜10)
pub fn check_section_curve(curve: &[[f32; 2]]) -> Result<(), String> {
    if curve.len() > MAX_SECTION_CURVE_POINTS {
        return Err(format!("curve の点は {MAX_SECTION_CURVE_POINTS} 個まで"));
    }
    for (i, [t, v]) in curve.iter().enumerate() {
        if !(0.0..=1.0).contains(t) || !t.is_finite() {
            return Err(format!("curve/{i}: 位置は 0〜1"));
        }
        if !(0.0..=10.0).contains(v) || !v.is_finite() {
            return Err(format!("curve/{i}: 値は 0〜10"));
        }
        if i > 0 && curve[i - 1][0] > *t {
            return Err(format!("curve/{i}: 位置は昇順に"));
        }
    }
    Ok(())
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
            plan_refs: BTreeMap::new(),
            made: BTreeMap::new(),
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

    /// 整形しない JSON(保存用。整形の約半分の大きさで速い。読み込みは同じ `from_json`)
    pub fn to_json_compact(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }

    pub fn from_json(s: &str) -> serde_json::Result<Self> {
        serde_json::from_str(s)
    }
}
