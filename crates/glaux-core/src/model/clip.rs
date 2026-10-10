//! クリップ。MIDI クリップと音声クリップを `kind` で区別する。
//!
//! 音声クリップも拍(Tick)上に置く。元音声は変更せず、切り出し位置・ゲイン・フェード・
//! ストレッチをデータとして重ねる非破壊編集。

use crate::id::{AssetId, ClipId, NoteId};
use crate::time::Tick;
use serde::{Deserialize, Serialize};

/// ノート単位の奏法(アーティキュレーション)。
/// 「同じ音源で奏法を切り替える」表現(メタルのブリッジミュート等)。
/// `Normal` は JSON に書かない(= フィールド省略)ので旧ファイルとそのまま互換。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Articulation {
    /// 通常(既定)
    #[default]
    Normal,
    /// ブリッジ(パーム)ミュート: 減衰が速く、こもった「ズンズン」した音
    PalmMute,
    /// スタッカート: 音価の半分で切る歯切れのよい発音
    Staccato,
    /// アクセント: その音だけ強く・明るく強調
    Accent,
    /// ビブラート: 音の後半にピッチの揺れが深くなっていく
    Vibrato,
    /// チョーキング(ベンド): 全音下から書かれた音程へ滑り上がる
    Bend,
    /// レガート(スラー): 同じトラックの直前の音から弾き直さずに滑らかにつなぐ
    /// (立ち上がりを消し、前の音はつなぎ目で消える。弦・管・歌のフレーズ、ギターのハンマリング)
    Legato,
    /// ポルタメント: レガートでつなぎ、直前の音の高さから書かれた音程へ滑らせる
    /// (ストリングスのポルタメント、シンセリードのグライド、ギターのスライド)
    Portamento,
}

impl Articulation {
    pub fn is_normal(&self) -> bool {
        *self == Articulation::Normal
    }

    /// 番号(時刻指定のノートに載せるとき)
    pub fn code(self) -> u8 {
        self as u8
    }

    /// 番号から(知らない番号は Normal)
    pub fn from_code(c: u8) -> Articulation {
        use Articulation::*;
        [
            Normal, PalmMute, Staccato, Accent, Vibrato, Bend, Legato, Portamento,
        ]
        .get(c as usize)
        .copied()
        .unwrap_or(Normal)
    }
}

/// ピッチカーブの区間の曲がり方(その点から次の点まで)
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurveShape {
    /// 直線(既定)
    #[default]
    Linear,
    /// ゆっくり動き出して速く着く
    EaseIn,
    /// 速く動き出してゆっくり着く(しゃくり・スクープの着地)
    EaseOut,
    /// ゆっくり動き出してゆっくり着く
    EaseInOut,
    /// 次の点まで今の値のまま、次の点で跳ぶ
    Hold,
}

impl CurveShape {
    pub fn is_linear(&self) -> bool {
        *self == CurveShape::Linear
    }

    /// 区間の中の割合 `x`(0〜1)を曲げる
    pub fn apply(self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        match self {
            CurveShape::Linear => x,
            CurveShape::EaseIn => x * x,
            CurveShape::EaseOut => x * (2.0 - x),
            CurveShape::EaseInOut => x * x * (3.0 - 2.0 * x),
            CurveShape::Hold => 0.0,
        }
    }

    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(c: u8) -> CurveShape {
        match c {
            1 => CurveShape::EaseIn,
            2 => CurveShape::EaseOut,
            3 => CurveShape::EaseInOut,
            4 => CurveShape::Hold,
            _ => CurveShape::Linear,
        }
    }
}

/// 連続ピッチカーブの 1 点。ノート先頭からの相対 tick と、書かれた音程からの
/// ずれ(セント。100 = 半音)。点の間は `shape` の曲がり方で補間(既定は直線)、
/// 最初の点より前 / 最後の点より後はその値を保持する。
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct PitchPoint {
    pub tick: Tick,
    pub cents: f32,
    /// この点から次の点までの曲がり方。省略で直線
    #[serde(default, skip_serializing_if = "CurveShape::is_linear")]
    pub shape: CurveShape,
}

impl PitchPoint {
    pub fn new(tick: Tick, cents: f32) -> Self {
        PitchPoint {
            tick,
            cents,
            shape: CurveShape::Linear,
        }
    }

    pub fn shaped(tick: Tick, cents: f32, shape: CurveShape) -> Self {
        PitchPoint { tick, cents, shape }
    }
}

/// ノートの表情の曲線(音量・明るさ)の 1 点。ノート先頭からの相対 tick と値、次の点までの曲がり方
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct CurvePoint {
    pub tick: Tick,
    pub value: f32,
    #[serde(default, skip_serializing_if = "CurveShape::is_linear")]
    pub shape: CurveShape,
}

impl CurvePoint {
    pub fn new(tick: Tick, value: f32, shape: CurveShape) -> Self {
        CurvePoint { tick, value, shape }
    }
}

/// 音量・明るさの曲線の点数上限(ボイス側が固定長で持つため)
pub const MAX_EXPR_POINTS: usize = 8;
/// 音量の曲線の範囲(dB。ノートの強さに対する増減)
pub const VOLUME_CURVE_DB: std::ops::RangeInclusive<f32> = -60.0..=12.0;
/// 明るさの曲線の範囲(−1 = 暗く、0 = そのまま、1 = 明るく)
pub const BRIGHTNESS_RANGE: std::ops::RangeInclusive<f32> = -1.0..=1.0;

/// 表情の曲線の検証(点数・tick の昇順・値の範囲)
pub fn check_expr_curve(
    what: &str,
    curve: &[CurvePoint],
    range: std::ops::RangeInclusive<f32>,
) -> Result<(), String> {
    if curve.len() > MAX_EXPR_POINTS {
        return Err(format!("{what} は最大 {MAX_EXPR_POINTS} 点"));
    }
    if curve.windows(2).any(|w| w[1].tick < w[0].tick) {
        return Err(format!("{what} の tick は昇順に"));
    }
    if curve
        .iter()
        .any(|p| !p.value.is_finite() || !range.contains(&p.value))
    {
        return Err(format!(
            "{what} の value は {}〜{}",
            range.start(),
            range.end()
        ));
    }
    Ok(())
}

/// ノートのビブラート(音源の側で正弦波として作る)。`Articulation::Vibrato` はこの既定値
/// (5.5Hz・±30 セント・0.12 秒後から 0.25 秒で全深度)と同じ意味。両方あればこちらが優先
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct Vibrato {
    /// 速さ(Hz)
    pub rate_hz: f32,
    /// 深さ(セント、片側)。歌は 30〜60、強いものは 90
    pub depth_cents: f32,
    /// 揺れ始めるまで(ms。ノートの頭から)
    #[serde(default)]
    pub delay_ms: f32,
    /// 全深度になるまで(ms)
    #[serde(default)]
    pub fade_in_ms: f32,
    /// 音の終わりの何 ms で揺れを消すか(0 = 消さない)
    #[serde(default)]
    pub fade_out_ms: f32,
    /// 音の終わりでの速さ(Hz。省略で rate_hz のまま。終わりで速める歌い方)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_end_hz: Option<f32>,
}

/// ビブラートの検証
pub fn check_vibrato(v: &Vibrato) -> Result<(), String> {
    let ok = |x: f32, lo: f32, hi: f32| x.is_finite() && (lo..=hi).contains(&x);
    if !ok(v.rate_hz, 0.5, 12.0) || !v.rate_end_hz.map_or(true, |r| ok(r, 0.5, 12.0)) {
        return Err("vibrato の rate_hz は 0.5〜12".to_owned());
    }
    if !ok(v.depth_cents, 0.0, 300.0) {
        return Err("vibrato の depth_cents は 0〜300".to_owned());
    }
    if !ok(v.delay_ms, 0.0, 5000.0)
        || !ok(v.fade_in_ms, 0.0, 5000.0)
        || !ok(v.fade_out_ms, 0.0, 5000.0)
    {
        return Err("vibrato の delay_ms・fade_in_ms・fade_out_ms は 0〜5000".to_owned());
    }
    Ok(())
}

/// ピッチカーブの点数上限(ボイス側が固定長で持つため)
pub const MAX_PITCH_POINTS: usize = 16;
/// ピッチカーブの振れ幅の上限(セント)
pub const MAX_PITCH_CENTS: f32 = 2400.0;

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Note {
    pub id: NoteId,
    /// クリップ先頭からの相対位置
    pub pos: Tick,
    pub dur: Tick,
    /// MIDI ノート番号 0..=127
    pub pitch: u8,
    /// ベロシティ 1..=127
    pub vel: u8,
    /// 奏法。省略時 Normal
    #[serde(default, skip_serializing_if = "Articulation::is_normal")]
    pub articulation: Articulation,
    /// 連続ピッチカーブ(自由描画のベンド・ポルタメント等)。空なら無し。
    /// 奏法(vibrato / bend)と併用できる(掛け合わせ)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pitch_curve: Vec<PitchPoint>,
    /// ポルタメントで滑る時間(ms)。省略時はトラックの `glide_ms`(それも無ければ 150ms)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glide_ms: Option<f32>,
    /// ビブラート(速さ・深さ・始まり・フェード)。省略で無し(奏法の vibrato は既定値のビブラート)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vibrato: Option<Vibrato>,
    /// 音量の曲線(dB。音の中でのクレッシェンド・スフォルツァンド・フォールで消える等)。空なら無し
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volume_curve: Vec<CurvePoint>,
    /// 明るさの曲線(−1〜1。音の中で暗く・明るく)。空なら無し
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub brightness_curve: Vec<CurvePoint>,
    /// 条件付きの発音(確率・ループの何回目か)。省略でいつも鳴る
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<NoteCondition>,
    /// 固定。AI が作り直す・直すときに変えない音(「手直しも含めて上書き」でも)。人はいつでも変えられる
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
}

/// 条件付きの発音(Elektron の条件と同じ考え方)。ループの何回目か(`rep`、0 始まり)とノートの ID から決まるので、
/// 何度鳴らしても・書き出しても同じ結果
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct NoteCondition {
    /// 鳴る確率 0〜1(既定 1)
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub probability: f32,
    /// [a, b]: b 回の繰り返しのうち a 回目だけ鳴る(1 始まり。[4, 4] = 4 回に 1 回、最後だけ)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<[u8; 2]>,
}

fn one() -> f32 {
    1.0
}

fn is_one(x: &f32) -> bool {
    *x >= 1.0
}

impl NoteCondition {
    /// 何もしない条件(いつも鳴る)か
    pub fn is_always(&self) -> bool {
        self.probability >= 1.0 && self.every.is_none()
    }
}

/// 条件の検証
pub fn check_condition(c: &NoteCondition) -> Result<(), String> {
    if !(0.0..=1.0).contains(&c.probability) {
        return Err("condition の probability は 0〜1".to_owned());
    }
    if let Some([a, b]) = c.every {
        if b == 0 || a == 0 || a > b || b > 16 {
            return Err("condition の every は [a, b](1 ≤ a ≤ b ≤ 16)".to_owned());
        }
    }
    Ok(())
}

/// ポルタメントで滑る時間の範囲(ms)
pub const GLIDE_MS_RANGE: std::ops::RangeInclusive<f32> = 10.0..=2000.0;
/// レガートのつなぎ目の長さの範囲(ms)
pub const LEGATO_MS_RANGE: std::ops::RangeInclusive<f32> = 5.0..=200.0;

/// ピッチカーブの検証(最大 `MAX_PITCH_POINTS` 点・tick は昇順・±`MAX_PITCH_CENTS` 以内)。
pub fn check_pitch_curve(curve: &[PitchPoint]) -> Result<(), String> {
    if curve.len() > MAX_PITCH_POINTS {
        return Err(format!("pitch_curve は最大 {MAX_PITCH_POINTS} 点"));
    }
    if curve.windows(2).any(|w| w[1].tick < w[0].tick) {
        return Err("pitch_curve の tick は昇順に".to_owned());
    }
    if curve
        .iter()
        .any(|p| !p.cents.is_finite() || p.cents.abs() > MAX_PITCH_CENTS)
    {
        return Err(format!("pitch_curve の cents は ±{MAX_PITCH_CENTS} 以内"));
    }
    Ok(())
}

impl Note {
    pub fn end(&self) -> Tick {
        self.pos + self.dur
    }

    /// ループの `rep` 回目(0 始まり。ループでなければ 0)に鳴るか
    pub fn plays(&self, rep: u64) -> bool {
        let Some(c) = &self.condition else {
            return true;
        };
        if let Some([a, b]) = c.every {
            if (rep % b.max(1) as u64) + 1 != a as u64 {
                return false;
            }
        }
        if c.probability < 1.0 {
            // ノートの ID と繰り返しの回から決まる乱数
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for b in self.id.as_str().bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
            h ^= rep.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            h ^= h >> 33;
            h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
            h ^= h >> 33;
            let u = (h >> 11) as f64 / (1u64 << 53) as f64;
            return u < c.probability as f64;
        }
        true
    }
}

#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Stretch {
    /// ストレッチなし(元の速度で再生)
    #[default]
    None,
    /// テンポに追従する(音程は変えずに伸縮)。素材は `original_bpm` で演奏されたものとして
    /// 扱い、1 tick = 60 / (original_bpm × PPQ) 秒ぶんの素材が常に 1 tick に対応する。
    /// プロジェクトのテンポを変えても拍がずれない
    Follow { original_bpm: f64 },
}

impl Stretch {
    /// 追従時に受け付ける元テンポの範囲
    pub const BPM_RANGE: std::ops::RangeInclusive<f64> = 20.0..=400.0;

    /// クリップ先頭から `ticks` 進んだ位置が、素材の何秒目(`offset` から)に当たるか。
    /// 追従しないときは `None`(テンポマップで秒に直す)
    pub fn follow_seconds(&self, ticks: f64) -> Option<f64> {
        match self {
            Stretch::None => None,
            Stretch::Follow { original_bpm } => {
                Some(ticks * 60.0 / (original_bpm * crate::time::PPQ as f64))
            }
        }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClipContent {
    Midi {
        /// (pos, pitch, id) 昇順を保つ
        #[serde(default)]
        notes: Vec<Note>,
        #[serde(default, rename = "loop")]
        looped: bool,
        /// ループ時に繰り返す長さ(クリップ先頭から)。`looped` が true のときだけ意味を持つ。
        /// クリップ長がこれより長いと、その範囲のノートが繰り返し鳴る
        #[serde(default, skip_serializing_if = "Option::is_none")]
        loop_len: Option<Tick>,
    },
    Audio {
        asset: AssetId,
        /// アセット内の再生開始位置(サンプル)
        #[serde(default)]
        offset_samples: u64,
        #[serde(default)]
        gain_db: f32,
        #[serde(default)]
        fade_in_ms: f32,
        #[serde(default)]
        fade_out_ms: f32,
        #[serde(default)]
        stretch: Stretch,
    },
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Clip {
    pub id: ClipId,
    pub name: String,
    pub start: Tick,
    pub length: Tick,
    #[serde(flatten)]
    pub content: ClipContent,
}

impl Clip {
    pub fn new_midi(id: ClipId, name: impl Into<String>, start: Tick, length: Tick) -> Self {
        Clip {
            id,
            name: name.into(),
            start,
            length,
            content: ClipContent::Midi {
                notes: vec![],
                looped: false,
                loop_len: None,
            },
        }
    }

    pub fn new_audio(
        id: ClipId,
        name: impl Into<String>,
        start: Tick,
        length: Tick,
        asset: AssetId,
    ) -> Self {
        Clip {
            id,
            name: name.into(),
            start,
            length,
            content: ClipContent::Audio {
                asset,
                offset_samples: 0,
                gain_db: 0.0,
                fade_in_ms: 0.0,
                fade_out_ms: 0.0,
                stretch: Stretch::None,
            },
        }
    }

    pub fn end(&self) -> Tick {
        self.start + self.length
    }

    pub fn is_midi(&self) -> bool {
        matches!(self.content, ClipContent::Midi { .. })
    }

    pub fn notes(&self) -> Option<&[Note]> {
        match &self.content {
            ClipContent::Midi { notes, .. } => Some(notes),
            _ => None,
        }
    }

    pub fn notes_mut(&mut self) -> Option<&mut Vec<Note>> {
        match &mut self.content {
            ClipContent::Midi { notes, .. } => Some(notes),
            _ => None,
        }
    }

    /// ループの繰り返し長(ループでなければ None)。
    pub fn loop_len(&self) -> Option<Tick> {
        match &self.content {
            ClipContent::Midi {
                looped: true,
                loop_len: Some(l),
                ..
            } if l.0 > 0 => Some(*l),
            _ => None,
        }
    }

    /// 再生・分析で実際に鳴るノート列(クリップ先頭からの位置)。
    /// クリップ長の外は切り捨て・切り詰め、ループクリップは繰り返しを展開する
    /// (ループ境界をまたぐノートは境界で切る。ループ長より後ろのノートは鳴らない)。
    /// 展開したノートは元と同じ ID を持つので、編集には使わないこと。
    pub fn playback_notes(&self) -> Vec<Note> {
        let Some(notes) = self.notes() else {
            return vec![];
        };
        let len = self.length.0;
        let clip_to = |n: &Note, offset: u64, limit: u64| -> Option<Note> {
            let pos = n.pos.0 + offset;
            let end = (n.pos.0 + n.dur.0).min(limit) + offset;
            let end = end.min(len);
            (pos < len && end > pos).then(|| Note {
                pos: Tick(pos),
                dur: Tick(end - pos),
                ..n.clone()
            })
        };
        match self.loop_len() {
            None => notes
                .iter()
                .filter(|n| n.plays(0))
                .filter_map(|n| clip_to(n, 0, u64::MAX))
                .collect(),
            Some(l) => {
                let l = l.0;
                let mut out = Vec::new();
                let mut offset = 0;
                let mut rep = 0u64;
                while offset < len {
                    out.extend(
                        notes
                            .iter()
                            .filter(|n| n.pos.0 < l && n.plays(rep))
                            .filter_map(|n| clip_to(n, offset, l)),
                    );
                    offset += l;
                    rep += 1;
                }
                out
            }
        }
    }
}

pub(crate) fn sort_notes(notes: &mut [Note]) {
    notes.sort_by(|a, b| (a.pos, a.pitch, &a.id).cmp(&(b.pos, b.pitch, &b.id)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn looped_clip(cond: Option<NoteCondition>) -> Clip {
        let mut c = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(3840 * 8));
        if let ClipContent::Midi {
            notes,
            looped,
            loop_len,
        } = &mut c.content
        {
            *looped = true;
            *loop_len = Some(Tick(3840));
            notes.push(Note {
                locked: false,
                id: NoteId::new(),
                pos: Tick(0),
                dur: Tick(240),
                pitch: 38,
                vel: 100,
                articulation: Default::default(),
                pitch_curve: vec![],
                glide_ms: None,
                vibrato: None,
                volume_curve: vec![],
                brightness_curve: vec![],
                condition: cond,
            });
        }
        c
    }

    #[test]
    fn conditions_pick_loop_repetitions() {
        // 8 回の繰り返し: 条件なしは 8 回、4 回に 1 回(4 回目)は 2 回
        assert_eq!(looped_clip(None).playback_notes().len(), 8);
        let every = looped_clip(Some(NoteCondition {
            probability: 1.0,
            every: Some([4, 4]),
        }));
        let pos: Vec<u64> = every.playback_notes().iter().map(|n| n.pos.0).collect();
        assert_eq!(pos, vec![3840 * 3, 3840 * 7]);
        // 確率は決まった乱数(何度でも同じ)
        let half = looped_clip(Some(NoteCondition {
            probability: 0.5,
            every: None,
        }));
        let a = half.playback_notes();
        assert_eq!(a, half.playback_notes());
        // 種はノートの ID(作るたびに違う)なので、1 つのクリップでは 1/256 の確率で 8 回とも鳴る。
        // 4 つ作って、どれかで鳴らない回があることを見る
        let some_skip = std::iter::once(a.len())
            .chain((0..3).map(|_| {
                looped_clip(Some(NoteCondition {
                    probability: 0.5,
                    every: None,
                }))
                .playback_notes()
                .len()
            }))
            .any(|n| n < 8);
        assert!(some_skip);
        assert!(check_condition(&NoteCondition {
            probability: 1.0,
            every: Some([3, 2]),
        })
        .is_err());
    }
}
