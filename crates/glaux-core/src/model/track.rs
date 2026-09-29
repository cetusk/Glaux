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
    /// SFZ の楽器。`instrument` はライブラリフォルダ(`~/.config/glaux/sfz/`)からの
    /// 相対パス(`Piano/piano.sfz` など。区切りは `/`)。`cc` は音源の調整つまみ(CC 番号 → 0〜127)の
    /// 上書き(音源の `<control>` の set_cc の代わり。マイクの混ぜ方・スネアの snap など)
    Sfz {
        instrument: String,
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        cc: std::collections::BTreeMap<u8, u8>,
    },
}

impl PluginSource {
    /// ドラムの楽器(音程でなく鍵盤で打楽器を選ぶ)か。内蔵 drum・SoundFont の bank 128・
    /// 名前に drum / kit / perc を含む SFZ
    pub fn is_drum_kit(&self) -> bool {
        match self {
            PluginSource::Builtin { name } => name == "drum",
            PluginSource::Sf2 { bank, .. } => *bank == 128,
            PluginSource::Sfz { instrument, .. } => {
                let s = instrument.to_ascii_lowercase();
                ["drum", "kit", "perc"].iter().any(|w| s.contains(w))
            }
            _ => false,
        }
    }
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
    /// ノード表示での入力と出口の置き場所(無ければ自動)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fx_io_pos: Option<super::routing::FxIoPos>,
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
    /// 変調(LFO)。内蔵の音源・エフェクトのつまみを揺らす(再生データを作るときにオートメーションに焼き込む)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modulators: Vec<Modulator>,
    /// 重ねる音源(本体の device とは別に、最大 [`MAX_LAYERS`] 個)。1 つのノートで範囲に合う層がすべて鳴る
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<Layer>,
    /// マクロ(最大 [`MAX_MACROS`] 個)。1 つの値で複数のつまみを動かす(再生データを作るときに焼き込む)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub macros: Vec<Macro>,
}

/// 本体の音源に重ねる層の上限(本体と合わせて 4 層)
pub const MAX_LAYERS: usize = 3;

fn default_key_hi() -> u8 {
    127
}
fn default_vel_lo() -> u8 {
    1
}
fn default_vel_hi() -> u8 {
    127
}
fn is_zero_f32(v: &f32) -> bool {
    *v == 0.0
}
fn is_zero_i8(v: &i8) -> bool {
    *v == 0
}

/// 重ねる音源 1 つ(内蔵・SoundFont・SFZ・サンプラー。CLAP は不可)
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Layer {
    #[serde(default)]
    pub name: String,
    pub device: Device,
    /// 層の音量(dB。トラックのフェーダーの前)
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub volume_db: f32,
    /// 層のパン(-1..1。トラックの中での左右)
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub pan: f32,
    /// 移調(半音)。1 オクターブ下のサブを重ねるなら -12
    #[serde(default, skip_serializing_if = "is_zero_i8")]
    pub transpose: i8,
    /// 鳴らす鍵盤とベロシティの範囲(両端を含む。元のノートの音程・強さで判定)
    #[serde(default)]
    pub key_lo: u8,
    #[serde(default = "default_key_hi")]
    pub key_hi: u8,
    #[serde(default = "default_vel_lo")]
    pub vel_lo: u8,
    #[serde(default = "default_vel_hi")]
    pub vel_hi: u8,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mute: bool,
}

impl Layer {
    pub fn new(device: Device) -> Self {
        Layer {
            name: String::new(),
            device,
            volume_db: 0.0,
            pan: 0.0,
            transpose: 0,
            key_lo: 0,
            key_hi: 127,
            vel_lo: 1,
            vel_hi: 127,
            mute: false,
        }
    }

    /// このノートで鳴るか
    pub fn plays(&self, pitch: u8, vel: u8) -> bool {
        !self.mute
            && (self.key_lo..=self.key_hi).contains(&pitch)
            && (self.vel_lo..=self.vel_hi).contains(&vel)
    }
}

/// 層の検証
pub fn check_layers(layers: &[Layer]) -> Result<(), String> {
    if layers.len() > MAX_LAYERS {
        return Err(format!(
            "layers は {MAX_LAYERS} 個まで(本体と合わせて 4 層)"
        ));
    }
    for l in layers {
        if matches!(l.device.source, PluginSource::Clap { .. }) {
            return Err("CLAP プラグインは層にできません(本体の音源にしてください)".to_owned());
        }
        if !(-60.0..=12.0).contains(&l.volume_db) || !(-1.0..=1.0).contains(&l.pan) {
            return Err("層の volume_db は -60〜12、pan は -1〜1".to_owned());
        }
        if !(-48..=48).contains(&l.transpose) {
            return Err("層の transpose は -48〜48".to_owned());
        }
        if l.key_lo > l.key_hi || l.key_hi > 127 || l.vel_lo > l.vel_hi || l.vel_hi > 127 {
            return Err("層の鍵盤・ベロシティの範囲が正しくありません".to_owned());
        }
    }
    Ok(())
}

/// 1 トラックのマクロの上限
pub const MAX_MACROS: usize = 8;

/// マクロ: 1 つの値(0〜1)で、割り当てたつまみを最小〜最大の間で動かす
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Macro {
    /// 「明るさ」「広がり」のような意味の名前
    pub name: String,
    /// 今の値(0〜1)。`macro/N` のオートメーションがあればそちらが優先
    pub value: f64,
    pub targets: Vec<MacroTarget>,
}

/// マクロの割り当て先 1 つ
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct MacroTarget {
    /// 内蔵の音源(`device/<名前>`)・エフェクト(`fx/<id>/<名前>`)のつまみか、`track/volume_db`・`track/pan`
    pub target: super::ParamPath,
    /// マクロが 0 のときと 1 のときの値(逆向きにしてもよい)
    pub min: f64,
    pub max: f64,
    /// 曲線(-1〜1)。0 = 直線、正 = はじめはゆっくり後で急に、負 = はじめに急に
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub curve: f64,
}

fn is_zero_f64(v: &f64) -> bool {
    *v == 0.0
}

impl MacroTarget {
    /// マクロの値(0〜1)をつまみの値にする
    pub fn map(&self, v: f64) -> f64 {
        let x = v.clamp(0.0, 1.0);
        let c = self.curve.clamp(-1.0, 1.0);
        let shaped = if c >= 0.0 {
            x.powf(1.0 + 3.0 * c)
        } else {
            1.0 - (1.0 - x).powf(1.0 - 3.0 * c)
        };
        self.min + (self.max - self.min) * shaped
    }
}

/// マクロの検証
pub fn check_macros(macros: &[Macro]) -> Result<(), String> {
    if macros.len() > MAX_MACROS {
        return Err(format!("macros は {MAX_MACROS} 個まで"));
    }
    for m in macros {
        if !(0.0..=1.0).contains(&m.value) {
            return Err(format!("マクロ「{}」の value は 0〜1", m.name));
        }
        if m.targets.is_empty() || m.targets.len() > 16 {
            return Err(format!("マクロ「{}」の割り当ては 1〜16 個", m.name));
        }
        for t in &m.targets {
            match &t.target {
                super::ParamPath::Device { name } if name.starts_with("clap:") => {
                    return Err("CLAP のつまみはマクロに割り当てられません(内蔵の音源・エフェクトだけ)".to_owned());
                }
                super::ParamPath::Device { .. } | super::ParamPath::Effect { .. } => {}
                super::ParamPath::Track { name } if name == "volume_db" || name == "pan" => {}
                _ => {
                    return Err(format!(
                        "マクロの割り当て先は device/<名前>・fx/<id>/<名前>・track/volume_db・track/pan({})",
                        t.target
                    ))
                }
            }
            if !t.min.is_finite() || !t.max.is_finite() || !t.curve.is_finite() {
                return Err("マクロの min・max・curve は数".to_owned());
            }
        }
    }
    Ok(())
}

impl Track {
    /// マクロを焼き込んだトラック(マクロが無ければ借りたまま)。
    /// - `macro/N` のオートメーションがあれば、割り当て先ごとに値を写したオートメーションにする
    ///   (1/32 音符ごとに取り直すので曲線も写る)
    /// - 無ければマクロの今の値を写した値を、割り当て先のつまみに置く
    /// - どちらでも、割り当て先に元からあったオートメーションはマクロが上書きする(同じ先を 2 つのマクロが持つなら後のもの)
    /// - `macro/N` のオートメーションは取り除く(エンジンは知らない)
    pub fn with_macros_applied(&self) -> std::borrow::Cow<'_, Track> {
        use super::{AutomationPoint, Curve, ParamPath, ParamValue};
        if self.macros.is_empty()
            && !self
                .automation
                .iter()
                .any(|l| matches!(l.target, ParamPath::Macro { .. }))
        {
            return std::borrow::Cow::Borrowed(self);
        }
        let mut t = self.clone();
        for (i, m) in self.macros.iter().enumerate() {
            let lane = self
                .automation
                .iter()
                .find(|l| l.target == ParamPath::Macro { index: i as u8 + 1 })
                .filter(|l| !l.points.is_empty());
            for tg in &m.targets {
                t.automation.retain(|l| l.target != tg.target);
                match lane {
                    Some(lane) => {
                        let first = lane.points[0].tick.0;
                        let last = lane.points[lane.points.len() - 1].tick.0;
                        let mut ticks: Vec<u64> = lane.points.iter().map(|p| p.tick.0).collect();
                        let step = crate::PPQ / 8;
                        let mut k = first;
                        while k < last {
                            ticks.push(k);
                            k += step;
                        }
                        ticks.sort_unstable();
                        ticks.dedup();
                        let points = ticks
                            .into_iter()
                            .map(|k| AutomationPoint {
                                tick: crate::Tick(k),
                                value: tg.map(lane.value_at(crate::Tick(k)).unwrap_or(m.value)),
                                curve: Curve::Linear,
                            })
                            .collect();
                        t.automation.push(AutomationLane {
                            target: tg.target.clone(),
                            points,
                        });
                    }
                    None => {
                        let v = tg.map(m.value);
                        match &tg.target {
                            ParamPath::Device { name } => {
                                if let Some(d) = t.device.as_mut() {
                                    d.params.insert(name.clone(), ParamValue::Float(v));
                                }
                            }
                            ParamPath::Effect { id, name } => {
                                if let Some(e) = t.effects.iter_mut().find(|e| &e.id == id) {
                                    e.params.insert(name.clone(), ParamValue::Float(v));
                                }
                            }
                            ParamPath::Track { name } if name == "volume_db" => {
                                t.volume_db = v.clamp(-60.0, 12.0) as f32;
                            }
                            ParamPath::Track { name } if name == "pan" => {
                                t.pan = v.clamp(-1.0, 1.0) as f32;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        t.automation
            .retain(|l| !matches!(l.target, ParamPath::Macro { .. }));
        std::borrow::Cow::Owned(t)
    }
}

/// LFO の形
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LfoShape {
    #[default]
    Sine,
    Triangle,
    Square,
    SawUp,
    SawDown,
    /// 周期ごとに乱数の値へ跳ぶ(サンプル & ホールド)
    Random,
}

impl LfoShape {
    /// 位相 0〜1 → −1〜1。`cycle` は何周目か(random の種)
    pub fn at(self, ph: f64, cycle: i64) -> f64 {
        let ph = ph.rem_euclid(1.0);
        match self {
            LfoShape::Sine => (std::f64::consts::TAU * ph).sin(),
            LfoShape::Triangle => 1.0 - 4.0 * ((ph + 0.25).rem_euclid(1.0) - 0.5).abs(),
            LfoShape::Square => {
                if ph < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            LfoShape::SawUp => 2.0 * ph - 1.0,
            LfoShape::SawDown => 1.0 - 2.0 * ph,
            LfoShape::Random => {
                let mut x = (cycle as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x5DEE_CE66;
                x ^= x >> 31;
                x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
                x ^= x >> 29;
                (x >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
            }
        }
    }
}

/// 変調(LFO)1 つ。`target` のつまみを `元の値 + depth × LFO` にする(範囲に収める)
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Modulator {
    /// 揺らすつまみ(`device/<名前>` か `fx/<id>/<名前>`)
    pub target: super::ParamPath,
    #[serde(default)]
    pub shape: LfoShape,
    /// テンポに合わせた周期("1/4"・"1/8d"・"1/8t"・"2/1" など)。無ければ rate_hz
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<String>,
    /// 周期が Hz のとき(sync が無いとき)
    #[serde(default)]
    pub rate_hz: f32,
    /// 揺らす幅(つまみの単位。±)
    pub depth: f64,
    /// 始まりの位相 0〜1
    #[serde(default)]
    pub phase: f32,
}

/// 1 トラックの変調の上限
pub const MAX_MODULATORS: usize = 8;

/// 変調の検証
pub fn check_modulators(mods: &[Modulator]) -> Result<(), String> {
    if mods.len() > MAX_MODULATORS {
        return Err(format!("modulators は {MAX_MODULATORS} 個まで"));
    }
    for m in mods {
        match &m.target {
            super::ParamPath::Device { name } if name.starts_with("clap:") => {
                return Err("CLAP のつまみは変調できません(内蔵の音源・エフェクトだけ)".to_owned());
            }
            super::ParamPath::Device { .. } | super::ParamPath::Effect { .. } => {}
            super::ParamPath::Track { .. } | super::ParamPath::Macro { .. } => {
                return Err("変調の先は device/<名前> か fx/<id>/<名前>".to_owned());
            }
        }
        match &m.sync {
            Some(s) => {
                if crate::meter::sync_ticks(s) <= 0.0 {
                    return Err(format!(
                        "sync が読めません: {s}(\"1/4\"・\"1/8d\"・\"1/8t\" など)"
                    ));
                }
            }
            None => {
                if !(0.01..=40.0).contains(&m.rate_hz) {
                    return Err("rate_hz は 0.01〜40(sync が無いとき)".to_owned());
                }
            }
        }
        if !m.depth.is_finite() || !(0.0..=1.0).contains(&m.phase) {
            return Err("depth は数、phase は 0〜1".to_owned());
        }
    }
    Ok(())
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
            fx_io_pos: None,
            clips: vec![],
            automation: vec![],
            sends: vec![],
            glide_ms: None,
            legato_ms: None,
            modulators: vec![],
            layers: vec![],
            macros: vec![],
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
    /// ノード表示での入力と出口の置き場所(無ければ自動)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fx_io_pos: Option<super::routing::FxIoPos>,
    /// マスターのオートメーション。対象は `track/volume_db`(マスター音量)と
    /// `fx/<マスターのエフェクト ID>/<パラメータ>`
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub automation: Vec<AutomationLane>,
}

#[cfg(test)]
mod layer_macro_tests {
    use super::*;
    use crate::model::{AutomationPoint, Curve, ParamPath, ParamValue};

    #[test]
    fn macro_paths_parse() {
        assert_eq!(
            ParamPath::parse("macro/3").unwrap(),
            ParamPath::Macro { index: 3 }
        );
        assert_eq!(ParamPath::Macro { index: 8 }.to_string(), "macro/8");
        assert!(ParamPath::parse("macro/9").is_err());
        assert!(ParamPath::parse("macro/0").is_err());
    }

    #[test]
    fn macro_curves_map_between_min_and_max() {
        let t = |curve| MacroTarget {
            target: ParamPath::device("cutoff"),
            min: 100.0,
            max: 1100.0,
            curve,
        };
        assert_eq!(t(0.0).map(0.5), 600.0);
        assert_eq!(t(0.0).map(2.0), 1100.0);
        assert!(t(1.0).map(0.5) < 200.0, "正はゆっくり");
        assert!(t(-1.0).map(0.5) > 1000.0, "負は急に");
        let inv = MacroTarget {
            min: 1100.0,
            max: 100.0,
            ..t(0.0)
        };
        assert_eq!(inv.map(0.25), 850.0, "逆向き");
    }

    #[test]
    fn macros_are_baked_and_checked() {
        let mut track = Track::new(TrackId::new(), "T", TrackKind::Midi);
        assert!(matches!(
            track.with_macros_applied(),
            std::borrow::Cow::Borrowed(_)
        ));
        track.device = Some(Device::builtin("subtractive"));
        track.automation.push(AutomationLane {
            target: ParamPath::device("cutoff"),
            points: vec![AutomationPoint {
                tick: crate::Tick(0),
                value: 5000.0,
                curve: Curve::Linear,
            }],
        });
        track.macros = vec![Macro {
            name: "明るさ".into(),
            value: 1.0,
            targets: vec![MacroTarget {
                target: ParamPath::device("cutoff"),
                min: 300.0,
                max: 3000.0,
                curve: 0.0,
            }],
        }];
        let t = track.with_macros_applied();
        assert_eq!(
            t.device.as_ref().unwrap().params.get("cutoff"),
            Some(&ParamValue::Float(3000.0))
        );
        assert!(
            t.automation.is_empty(),
            "元のオートメーションはマクロが上書き"
        );
        // 検証
        assert!(check_macros(&track.macros).is_ok());
        let mut bad = track.macros.clone();
        bad[0].targets[0].target = ParamPath::device("clap:12");
        assert!(check_macros(&bad).is_err());
        bad[0].targets[0].target = ParamPath::track("glide_ms");
        assert!(check_macros(&bad).is_err());
        let mut bad = track.macros.clone();
        bad[0].value = 1.5;
        assert!(check_macros(&bad).is_err());
        let layer = Layer::new(Device::builtin("fm"));
        assert!(check_layers(&[layer.clone(), layer.clone(), layer.clone()]).is_ok());
        assert!(
            check_layers(&[layer.clone(), layer.clone(), layer.clone(), layer.clone()]).is_err()
        );
        let mut clap = layer.clone();
        clap.device.source = PluginSource::Clap {
            plugin_id: "x".into(),
            state: None,
        };
        assert!(check_layers(&[clap]).is_err());
        let mut odd = layer;
        odd.key_lo = 80;
        odd.key_hi = 70;
        assert!(check_layers(&[odd]).is_err());
    }
}
