//! MCP ツール層。
//!
//! ここは「AI が読む API ドキュメント」でもある。ツールの description には
//! いつ使うか・注意点を書く。
//!
//! すべての編集は `glaux_core::Command` に変換して Session アクターに送る。
//! 各レスポンスには `project_version`(版数。編集・undo・redo のたびに増え、戻らない)を含め、
//! AI が自分の把握が古くなっていないか判断できるようにする。

use crate::actor::{Mutated, SessionHandle};
use glaux_core::{Author, Command, EntryId};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerConfig},
    service::RequestContext,
    tool, tool_handler, tool_router, RoleServer, ServerHandler,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Clone)]
pub struct GlauxServer {
    handle: SessionHandle,
    tool_router: ToolRouter<Self>,
}

// ---- パラメータ型 -------------------------------------------------------

#[derive(Deserialize, JsonSchema)]
pub struct GetProjectParams {
    /// 指定したトラック ID(`trk_xxxxxx`)だけを返す。省略で全トラック。
    #[serde(default)]
    pub track_ids: Option<Vec<String>>,
    /// false にすると MIDI クリップの `notes` を空にし、代わりに `note_count` を付ける。
    /// ノートが多いプロジェクトではまず false で構造を把握することを推奨。既定 true。
    #[serde(default)]
    pub include_notes: Option<bool>,
    /// false にするとトラックの `automation` を省く。既定 true。
    #[serde(default)]
    pub include_automation: Option<bool>,
    /// 指定したクリップ ID(`clp_xxxxxx`)だけを返す(それを含むトラックだけ残る)。
    #[serde(default)]
    pub clip_ids: Option<Vec<String>>,
    /// この tick 範囲 [start_tick, end_tick) に掛かるクリップだけを返し、MIDI クリップのノートも
    /// 範囲に掛かるものだけにする(そのクリップには `notes_in_range: true` と全体の `note_count` が付く)。
    #[serde(default)]
    pub start_tick: Option<u64>,
    #[serde(default)]
    pub end_tick: Option<u64>,
    /// "compact" にするとノートを配列 `[id, pos, dur, pitch, vel]` で返す(量が約半分)。
    /// 奏法・ピッチカーブ・glide_ms があるノートだけ 6 番目に `{articulation, pitch_curve, glide_ms}` が付く。
    /// 既定 "full"(オブジェクト)。
    #[serde(default)]
    pub note_format: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ApplyCommandsParams {
    /// 適用するコマンド(glaux の Command JSON)の配列。
    /// 例: `{"op":"set_track_prop","id":"trk_a1b2c3","prop":"volume_db","value":-6.0}`
    pub commands: Vec<Value>,
    /// この編集のまとまりに付けるラベル(履歴・Undo 単位の名前)。日本語可。
    pub label: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct UndoRedoParams {
    /// 取り消し(やり直し)する回数。省略時 1。
    #[serde(default)]
    pub n: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
pub struct CheckpointParams {
    /// チェックポイント名。後で `revert_to` に渡す。
    pub label: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct RevertToParams {
    /// 戻り先のチェックポイント名(`checkpoint` で付けたもの)。
    pub label: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct RevertEntryParams {
    /// 取り消す履歴エントリの ID(`hst_xxxxxx`。get_history で確認)。
    pub entry_id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListParamsParams {
    /// 対象トラック ID(`trk_xxxxxx`)。省略すると内蔵楽器のカタログ
    /// (利用できる楽器名と全パラメータ仕様)を返す。
    #[serde(default)]
    pub track_id: Option<String>,
    /// CLAP プラグインのトラックで、つまみを名前・所属の部分一致で絞り込む(例 "cutoff"、"filter"、"reverb")。
    #[serde(default)]
    pub filter: Option<String>,
    /// CLAP プラグインのつまみを最大何個返すか(既定 80)。
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct AnalyzeAudioParams {
    /// 解析対象のトラック ID の配列。省略で全トラックのミックス。
    /// 1 つだけ渡せばそのトラック単体を「聴く」ことになる。
    #[serde(default)]
    pub track_ids: Option<Vec<String>>,
    /// 解析範囲の開始 tick。省略で曲頭から。
    #[serde(default)]
    pub start_tick: Option<u64>,
    /// 解析範囲の終了 tick。省略で曲末まで。
    #[serde(default)]
    pub end_tick: Option<u64>,
    /// true にすると各トラックをソロでレンダした要約(loudness / band_energy 等)を
    /// tracks 配列として追加で返す。ミックスバランスの診断はこれを使う。
    #[serde(default)]
    pub per_track: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct CompareMixParams {
    /// 「前」にするチェックポイントの名前(checkpoint で付けたもの)。
    /// checkpoint / before_entry / back のどれか 1 つを指定する(どれも無ければ直前の 1 編集の前)
    #[serde(default)]
    pub checkpoint: Option<String>,
    /// 「前」にする履歴エントリ ID(そのエントリを適用する直前と比べる)
    #[serde(default)]
    pub before_entry: Option<String>,
    /// 最新から n 個の編集を戻した所を「前」にする
    #[serde(default)]
    pub back: Option<usize>,
    /// 比べるトラック ID の配列。省略で全トラックのミックス
    #[serde(default)]
    pub track_ids: Option<Vec<String>>,
    /// 比べる範囲の開始 tick。省略で曲頭から
    #[serde(default)]
    pub start_tick: Option<u64>,
    /// 比べる範囲の終了 tick。省略で曲末まで
    #[serde(default)]
    pub end_tick: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct AnalyzeHarmonyParams {
    /// 対象トラック ID の配列。省略で全トラック(ドラムは自動で除外される)。
    #[serde(default)]
    pub track_ids: Option<Vec<String>>,
    /// 分析範囲の開始 tick。省略で曲頭から。
    #[serde(default)]
    pub start_tick: Option<u64>,
    /// 分析範囲の終了 tick。省略で曲末まで。
    #[serde(default)]
    pub end_tick: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct AnalyzeRhythmParams {
    /// 対象トラック ID の配列。省略で全トラック(リズムはドラムも対象)。
    #[serde(default)]
    pub track_ids: Option<Vec<String>>,
    /// 分析範囲の開始 tick。省略で曲頭から。
    #[serde(default)]
    pub start_tick: Option<u64>,
    /// 分析範囲の終了 tick。省略で曲末まで。
    #[serde(default)]
    pub end_tick: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetHistoryParams {
    /// 作者で絞り込む: "human" | "ai" | "system"。省略で全部。
    #[serde(default)]
    pub author: Option<String>,
    /// この履歴エントリ ID(`hst_xxxxxx`)より後のエントリだけを返す(当該エントリは含まない)。
    #[serde(default)]
    pub since: Option<String>,
    /// 返す最大件数。最新側から数える(返却順は古い→新しい)。
    /// 省略時は、since があれば全件、なければ最新 50 件(total に全件数が入る)。
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListSoundfontsParams {
    /// 指定するとその .sf2 のプリセット一覧(bank / preset / 名前)を返す。
    /// 省略でライブラリフォルダ内のファイル一覧。
    #[serde(default)]
    pub file: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SetSoundfontParams {
    /// 音源を設定するトラック ID(`trk_xxxxxx`)。
    pub track_id: String,
    /// ライブラリフォルダ内の .sf2 ファイル名(list_soundfonts で確認)。
    pub soundfont: String,
    /// バンク番号(GM 音色は 0、GM ドラムキットは 128 が慣例)。
    pub bank: u16,
    /// プリセット(プログラム)番号。
    pub preset: u16,
}

#[derive(Deserialize, JsonSchema)]
pub struct ImportSampleParams {
    /// 音源を設定するトラック ID(`trk_xxxxxx`)。
    pub track_id: String,
    /// 音声ファイルの絶対パス(ユーザーのマシン上のファイル)。WAV / MP3 / FLAC / OGG / M4A に対応
    /// (WAV 以外は取り込み時に WAV へ変換される)。
    pub path: String,
    /// サンプル自身の音程(MIDI ノート番号。60 = C4)。この音で等速再生になる。
    /// 省略時 60。音程のない素材(ドラムワンショット等)は 60 のままでよい。
    #[serde(default)]
    pub root: Option<u8>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ImportAudioClipParams {
    /// 置き先の音声トラック ID(`trk_xxxxxx`、kind: "audio")。
    pub track_id: String,
    /// 音声ファイルの絶対パス(ユーザーのマシン上のファイル)。WAV / MP3 / FLAC / OGG / M4A に対応
    /// (WAV 以外は取り込み時に WAV へ変換される)。
    pub path: String,
    /// クリップの開始位置(tick)。省略で曲頭(0)。
    #[serde(default)]
    pub start_tick: Option<u64>,
    /// クリップ名。省略でファイル名。
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct TranscribeAudioParams {
    /// 譜起こしする音声クリップ ID(`clp_xxxxxx`、kind: "audio")。
    pub clip_id: String,
    /// ノートを置く MIDI トラック ID。省略すると音声トラックの直後に「<名前> MIDI」を新設。
    #[serde(default)]
    pub dest_track_id: Option<String>,
    /// 開始位置と長さを丸めるグリッド(tick)。既定 240(1/16)。0 で丸めない。
    #[serde(default)]
    pub quantize_ticks: Option<u64>,
    /// これより短いノートは捨てる(ms)。既定 80(poly は 128)。
    #[serde(default)]
    pub min_note_ms: Option<f32>,
    /// 方式: "melody"(既定。鼻歌・歌・単音。音程の揺れやしゃくれに強い)/
    /// "poly"(和音。ピアノ・ギターのコード、伴奏入りの素材。basic-pitch)。
    #[serde(default)]
    pub mode: Option<String>,
}

/// 解析・比較の対象の音(clip_id / file / track_id のどれか 1 つ)。
#[derive(Deserialize, JsonSchema, Clone)]
pub struct SoundSourceParams {
    /// 音声クリップ ID(`clp_xxxxxx`、kind: "audio")。クリップが参照している範囲を使う。
    #[serde(default)]
    pub clip_id: Option<String>,
    /// 音声ファイルのパス(WAV / MP3 / FLAC / OGG / M4A)。
    #[serde(default)]
    pub file: Option<String>,
    /// MIDI トラック ID。そのトラックの音源とエフェクトで 1 音だけ鳴らした音を使う。
    #[serde(default)]
    pub track_id: Option<String>,
    /// track_id のときの音の高さ(MIDI ノート番号。既定 60 = C4)。
    #[serde(default)]
    pub pitch: Option<u8>,
    /// track_id のときのベロシティ(既定 100)。
    #[serde(default)]
    pub velocity: Option<u8>,
    /// track_id のときの鍵盤を押している長さ(ms。既定 1000)。
    #[serde(default)]
    pub duration_ms: Option<u32>,
}

impl SoundSourceParams {
    pub fn to_source(&self) -> Result<crate::sound::SoundSource, String> {
        use crate::sound::SoundSource;
        let n = [
            self.clip_id.is_some(),
            self.file.is_some(),
            self.track_id.is_some(),
        ]
        .iter()
        .filter(|b| **b)
        .count();
        if n != 1 {
            return Err("clip_id / file / track_id のどれか 1 つを指定してください".to_owned());
        }
        if let Some(c) = &self.clip_id {
            return Ok(SoundSource::Clip(
                glaux_core::ClipId::parse(c).map_err(|e| e.to_string())?,
            ));
        }
        if let Some(f) = &self.file {
            return Ok(SoundSource::File(std::path::PathBuf::from(f)));
        }
        let t = self.track_id.as_deref().unwrap_or_default();
        Ok(SoundSource::TrackNote {
            track: glaux_core::TrackId::parse(t).map_err(|e| e.to_string())?,
            pitch: self.pitch.unwrap_or(60).min(127),
            velocity: self.velocity.unwrap_or(100).clamp(1, 127),
            seconds: self.duration_ms.unwrap_or(1000).clamp(50, 8000) as f64 / 1000.0,
        })
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct AnalyzeSoundParams {
    #[serde(flatten)]
    pub source: SoundSourceParams,
}

#[derive(Deserialize, JsonSchema)]
pub struct CompareSoundsParams {
    /// 比べる音 A(基準。clip_id / file / track_id のどれか 1 つ。track_id なら pitch 等も)。
    pub a: SoundSourceParams,
    /// 比べる音 B。
    pub b: SoundSourceParams,
}

#[derive(Deserialize, JsonSchema)]
pub struct MatchSoundParams {
    /// 目標の音: 音声クリップ ID(`clp_xxxxxx`)。
    #[serde(default)]
    pub clip_id: Option<String>,
    /// 目標の音: 音声ファイルのパス。
    #[serde(default)]
    pub file: Option<String>,
    /// 音色を合わせる MIDI トラック ID。音源は内蔵 subtractive か fm になる。
    pub track_id: String,
    /// 合わせる音源: "auto"(既定。subtractive / fm / wavetable を探して最も近いもの)/ "subtractive" / "fm" / "wavetable"。
    #[serde(default)]
    pub instrument: Option<String>,
    /// リバーブの量と広さも一緒に探し、効きがあればトラックの最後にリバーブを足す(既定 false)。
    #[serde(default)]
    pub reverb: Option<bool>,
    /// 探す時間の上限(秒。既定は instrument が auto なら 30、それ以外 20。最大 120)。長いほど近づく。
    #[serde(default)]
    pub max_seconds: Option<f32>,
    /// トラックの音源が内蔵の subtractive / fm / wavetable 以外(CLAP・SoundFont 等)でも置き換える(既定 false = エラーにする)。
    #[serde(default)]
    pub replace_device: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct FindSimilarPresetsParams {
    /// 目標の音: 音声クリップ ID(`clp_xxxxxx`)。
    #[serde(default)]
    pub clip_id: Option<String>,
    /// 目標の音: 音声ファイルのパス。
    #[serde(default)]
    pub file: Option<String>,
    /// CLAP プラグイン(例 Surge XT)を音源にしたトラック ID。そのプラグインのプリセットから探す。
    pub track_id: String,
    /// カテゴリ(フォルダ名)の前方一致で絞る(例 "Pads"、"Leads"、"Basses")。絞ると索引作りも速い。
    #[serde(default)]
    pub category: Option<String>,
    /// 何件返すか(既定 5)。
    #[serde(default)]
    pub limit: Option<usize>,
    /// 索引(プリセットを 1 音ずつ鳴らした記録)を作り足す時間の上限(秒。既定 60、0 で作らない)。
    #[serde(default)]
    pub index_seconds: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct RefinePluginParamsParams {
    /// 目標の音: 音声クリップ ID(`clp_xxxxxx`)。
    #[serde(default)]
    pub clip_id: Option<String>,
    /// 目標の音: 音声ファイルのパス。
    #[serde(default)]
    pub file: Option<String>,
    /// CLAP 音源(例 Surge XT)のトラック ID。今の音色(プリセット + 上書き値)から出発する。
    pub track_id: String,
    /// 動かすつまみの名前の部分一致の並び(例 ["cutoff", "resonance", "amp eg release"])。
    /// 省略でフィルターのカットオフ・レゾナンス・エンベロープ量、アンプの ADSR、フィルターのディケイ、デチューンを自動で選ぶ。
    #[serde(default)]
    pub params: Option<Vec<String>>,
    /// 探す時間の上限(秒。既定 20)。
    #[serde(default)]
    pub max_seconds: Option<f32>,
}

#[derive(Deserialize, JsonSchema)]
pub struct AnalyzeBeatsParams {
    /// 音声クリップ ID(`clp_xxxxxx`、kind: "audio")。クリップが参照している範囲を使う。
    #[serde(default)]
    pub clip_id: Option<String>,
    /// 音声ファイルのパス(WAV / MP3 / FLAC / OGG / M4A)。
    #[serde(default)]
    pub file: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListPluginPresetsParams {
    /// CLAP プラグインを音源にしたトラック ID(`trk_xxxxxx`)。エフェクトなら fx_id を使う。
    #[serde(default)]
    pub track_id: Option<String>,
    /// CLAP プラグインのエフェクト ID(`fx_xxxxxx`。トラック・マスターどちらでも)。
    #[serde(default)]
    pub fx_id: Option<String>,
    /// 名前・カテゴリ・作者・タグの部分一致(例 "pad"、"bass"、"pluck")。
    #[serde(default)]
    pub filter: Option<String>,
    /// カテゴリ(フォルダ名)の前方一致(例 "Pads"、"Leads")。一覧の categories から選ぶ。
    #[serde(default)]
    pub category: Option<String>,
    /// 最大何件返すか(既定 50)。
    #[serde(default)]
    pub limit: Option<usize>,
    /// true でプリセットを探し直す(プリセットを追加した後など)。
    #[serde(default)]
    pub rescan: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct LoadPluginPresetParams {
    /// CLAP プラグインを音源にしたトラック ID(`trk_xxxxxx`)。エフェクトなら fx_id を使う。
    #[serde(default)]
    pub track_id: Option<String>,
    /// CLAP プラグインのエフェクト ID(`fx_xxxxxx`)。
    #[serde(default)]
    pub fx_id: Option<String>,
    /// list_plugin_presets が返したプリセットの id。
    pub preset: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListPluginsParams {
    /// true でプラグインを探し直す(インストールした後など)。
    #[serde(default)]
    pub rescan: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SeparateAudioParams {
    /// 分離する音声クリップ ID(`clp_xxxxxx`、kind: "audio")。
    pub clip_id: String,
    /// 方式: "builtin"(既定。内蔵の信号処理で「打楽器」「音程楽器」の 2 パート。速い)/
    /// "demucs"(外部の Demucs がインストールされていれば「ボーカル」「ドラム」「ベース」「その他」の
    /// 4 パート。高品質だが数十秒〜数分かかる)。
    #[serde(default)]
    pub method: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SavePresetParams {
    /// 保存元のトラック ID(`trk_xxxxxx`)。そのトラックの音源 + エフェクトチェーンを保存する。
    pub track_id: String,
    /// プリセット名(ファイル名になる。日本語可。/ \ : * ? " < > | は不可)。
    pub name: String,
    /// 用途メモ(例: 「EDM リード用。unison 7 + distortion」)。
    #[serde(default)]
    pub description: Option<String>,
    /// 同名プリセットがあるとき上書きする。既定 false。
    #[serde(default)]
    pub overwrite: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct LoadPresetParams {
    /// 適用先のトラック ID(`trk_xxxxxx`)。
    pub track_id: String,
    /// 適用するプリセット名(list_presets で確認)。
    pub name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct DeletePresetParams {
    /// 削除するプリセット名。
    pub name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct SaveEffectPresetParams {
    /// 保存元のエフェクトがあるトラック ID(`trk_xxxxxx`)。マスターのエフェクトなら "master"。
    pub target: String,
    /// 保存するエフェクトの ID(`fx_xxxxxx`。get_project の effects[].id)。
    pub fx_id: String,
    /// プリセット名(ファイル名になる。日本語可。/ \ : * ? " < > | は不可)。
    pub name: String,
    /// メモ(どんな音か・何に使うか)。省略でエフェクトに付いているメモ。
    #[serde(default)]
    pub note: Option<String>,
    /// 同名プリセットがあるとき上書きする。既定 false。
    #[serde(default)]
    pub overwrite: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct LoadEffectPresetParams {
    /// 足す先のトラック ID(`trk_xxxxxx`)。マスターなら "master"。
    pub target: String,
    /// 足すプリセット名(list_effect_presets で確認)。
    pub name: String,
    /// チェーンの何番目に入れるか(0 始まり)。省略で末尾。
    #[serde(default)]
    pub index: Option<usize>,
    /// true なら鳴らさずに「外してある」状態で置く(後で使う候補として)。既定 false。
    #[serde(default)]
    pub parked: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetGuideParams {
    /// instruments / genres / expression / mix / audio / sound_match / clap。省略で一覧。
    #[serde(default)]
    pub topic: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetChangesParams {
    /// この履歴エントリ ID(`hst_xxxxxx`。apply_commands の entry_id など)より後の変更をまとめる。
    /// 省略で最新 20 件。
    #[serde(default)]
    pub since: Option<String>,
    /// 作者で絞り込む: "human" | "ai" | "system"(人間が何を変えたかを知るなら "human")。
    #[serde(default)]
    pub author: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct DuplicateClipsParams {
    /// 複製するクリップ ID(`clp_xxxxxx`)の配列。
    pub clip_ids: Vec<String>,
    /// 置く位置(tick)。複数なら最初のクリップがここに来て、互いの間隔は保つ。
    #[serde(default)]
    pub to_tick: Option<u64>,
    /// または、元の位置からのずらし量(tick。3840 = 4/4 の 1 小節)。to_tick と両方省略すると、
    /// 選んだクリップの範囲の直後に続けて置く(「サビをもう 1 回」)。
    #[serde(default)]
    pub offset_ticks: Option<i64>,
    /// 置くトラック ID。省略で元と同じトラック。
    #[serde(default)]
    pub track_id: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct BounceTrackParams {
    /// 音声にするトラック ID(`trk_xxxxxx`)。MIDI・音声トラック(バスは不可)。
    pub track_id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ExportMidiParams {
    /// 書き出す .mid ファイルの絶対パス。省略でプロジェクトの export/ に日時付きの名前
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct BarsParams {
    /// 小節番号(1 始まり)。insert_bars はこの小節の頭に挿入、delete_bars はこの小節から削除。
    pub bar: u32,
    /// 小節数。
    pub count: u32,
}

#[derive(Deserialize, JsonSchema)]
pub struct TransposeNotesParams {
    /// 対象クリップ ID(`clp_xxxxxx`)。
    pub clip_id: String,
    /// 移動量(半音単位)。正で上、負で下。12 で 1 オクターブ。
    pub semitones: i32,
    /// 対象ノート ID(`nt_xxxxxx`)の配列。省略でクリップ内の全ノート。
    #[serde(default)]
    pub note_ids: Option<Vec<String>>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ShiftNotesParams {
    /// 対象クリップ ID(`clp_xxxxxx`)。
    pub clip_id: String,
    /// 移動量(tick)。正で後ろ、負で前。960 = 4 分音符、3840 = 4/4 の 1 小節。
    pub delta_ticks: i64,
    /// 対象ノート ID の配列。省略でクリップ内の全ノート。
    #[serde(default)]
    pub note_ids: Option<Vec<String>>,
}

#[derive(Deserialize, JsonSchema)]
pub struct QuantizeNotesParams {
    /// 対象クリップ ID(`clp_xxxxxx`)。
    pub clip_id: String,
    /// グリッド間隔(tick)。240 = 1/16、480 = 1/8、960 = 1/4。
    pub grid_ticks: u64,
    /// 掛かり具合 0.0〜1.0。1.0 でグリッドに完全一致、0.5 で半分だけ寄せる
    /// (人間味を残すなら 0.5〜0.8)。省略時 1.0。
    #[serde(default)]
    pub strength: Option<f64>,
    /// 対象ノート ID の配列。省略でクリップ内の全ノート。
    #[serde(default)]
    pub note_ids: Option<Vec<String>>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SwingNotesParams {
    /// 対象クリップ ID(`clp_xxxxxx`)。
    pub clip_id: String,
    /// 裏拍の単位(tick)。480 = 8 分(既定)、240 = 16 分。
    #[serde(default)]
    pub grid_ticks: Option<u64>,
    /// スウィング率 0.5〜0.8。0.5 = ストレート、0.58 ≈ 軽め、0.667 ≈ 3 連(シャッフル)、0.75 = 付点(ハネ強め)。
    pub swing: f64,
    /// 掛かり具合 0.0〜1.0(省略時 1.0)。
    #[serde(default)]
    pub strength: Option<f64>,
    /// 対象ノート ID の配列。省略でクリップ内の全ノート。
    #[serde(default)]
    pub note_ids: Option<Vec<String>>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ScaleVelocityParams {
    /// 対象クリップ ID(`clp_xxxxxx`)。
    pub clip_id: String,
    /// 倍率。vel = round(vel * factor + offset)。省略時 1.0。
    #[serde(default)]
    pub factor: Option<f64>,
    /// 加算量。負で弱く。省略時 0。factor とどちらかは必ず指定する。
    #[serde(default)]
    pub offset: Option<f64>,
    /// 対象ノート ID の配列。省略でクリップ内の全ノート。
    #[serde(default)]
    pub note_ids: Option<Vec<String>>,
}

// ---- ヘルパー -----------------------------------------------------------

type ToolResult = Result<JsonText, String>;

/// apply_commands の Command JSON のうち、省略された新規 ID を振る。
/// 対象: ノート(add_notes / add_clip / add_track の中)・クリップ・トラック・エフェクト・split_clip の new_id。
/// 振った ID は `assigned` に `{command, kind, id}` で積む(同じ呼び出しの中で後から参照するものは、
/// 呼び出し側が自分で ID を付ける)
fn assign_missing_ids(cmd: &mut Value, index: usize, assigned: &mut Vec<Value>) {
    fn fill(obj: &mut Value, kind: &str, index: usize, assigned: &mut Vec<Value>) {
        let Some(map) = obj.as_object_mut() else {
            return;
        };
        if map.get("id").is_some_and(|v| !v.is_null()) {
            return;
        }
        let id = match kind {
            "note" => glaux_core::NoteId::new().to_string(),
            "clip" => glaux_core::ClipId::new().to_string(),
            "track" => glaux_core::TrackId::new().to_string(),
            _ => glaux_core::FxId::new().to_string(),
        };
        map.insert("id".into(), json!(id));
        // ノートは数が多いので返さない(get_project で見られる)
        if kind != "note" {
            assigned.push(json!({ "command": index, "kind": kind, "id": id }));
        }
    }
    fn clip(c: &mut Value, index: usize, assigned: &mut Vec<Value>) {
        fill(c, "clip", index, assigned);
        if let Some(notes) = c.get_mut("notes").and_then(Value::as_array_mut) {
            for n in notes {
                fill(n, "note", index, assigned);
            }
        }
    }
    match cmd.get("op").and_then(Value::as_str) {
        Some("add_notes") => {
            if let Some(notes) = cmd.get_mut("notes").and_then(Value::as_array_mut) {
                for n in notes {
                    fill(n, "note", index, assigned);
                }
            }
        }
        Some("add_clip") => {
            if let Some(c) = cmd.get_mut("clip") {
                clip(c, index, assigned);
            }
        }
        Some("add_track") => {
            if let Some(t) = cmd.get_mut("track") {
                fill(t, "track", index, assigned);
                if let Some(clips) = t.get_mut("clips").and_then(Value::as_array_mut) {
                    for c in clips {
                        clip(c, index, assigned);
                    }
                }
            }
        }
        Some("add_effect") | Some("add_master_effect") => {
            if let Some(e) = cmd.get_mut("effect") {
                fill(e, "effect", index, assigned);
            }
        }
        Some("split_clip") => {
            if cmd.get("new_id").is_none_or(Value::is_null) {
                let id = glaux_core::ClipId::new().to_string();
                cmd["new_id"] = json!(id);
                assigned.push(json!({ "command": index, "kind": "clip", "id": id }));
            }
        }
        Some("batch") => {
            if let Some(cmds) = cmd.get_mut("commands").and_then(Value::as_array_mut) {
                for c in cmds {
                    assign_missing_ids(c, index, assigned);
                }
            }
        }
        _ => {}
    }
}

/// ノートを配列 `[id, pos, dur, pitch, vel]` にする(奏法などがあれば 6 番目にまとめる)
fn compact_note(n: &Value) -> Value {
    let mut a = vec![
        n["id"].clone(),
        n["pos"].clone(),
        n["dur"].clone(),
        n["pitch"].clone(),
        n["vel"].clone(),
    ];
    let mut extra = serde_json::Map::new();
    for key in ["articulation", "pitch_curve", "glide_ms"] {
        match n.get(key) {
            None | Some(Value::Null) => {}
            Some(Value::String(s)) if key == "articulation" && s == "normal" => {}
            Some(Value::Array(x)) if x.is_empty() => {}
            Some(x) => {
                extra.insert(key.to_owned(), x.clone());
            }
        }
    }
    if !extra.is_empty() {
        a.push(Value::Object(extra));
    }
    Value::Array(a)
}

/// get_history で limit も since も無いときに返す件数
const DEFAULT_HISTORY_LIMIT: usize = 50;

/// ツールの結果の JSON。text の内容だけで返す。
/// rmcp の `Json` は同じ JSON を text と structuredContent の両方に入れるので、通信量と
/// クライアント(AI)が受け取る量が 2 倍になる(ノート 1 万の get_project で 1.36MB)
pub struct JsonText(pub Value);

impl rmcp::handler::server::tool::IntoCallToolResult for JsonText {
    fn into_call_tool_result(self) -> Result<rmcp::model::CallToolResponse, rmcp::ErrorData> {
        Ok(
            rmcp::model::CallToolResult::success(vec![rmcp::model::ContentBlock::text(
                self.0.to_string(),
            )])
            .into(),
        )
    }
}

/// CLAP プラグインの持ち主(音源のトラック ID か、エフェクト ID のどちらか)。
fn plugin_owner(
    track_id: Option<&str>,
    fx_id: Option<&str>,
) -> Result<glaux_engine::plugins::PluginOwner, String> {
    use glaux_engine::plugins::PluginOwner;
    match (fx_id, track_id) {
        (Some(f), _) => Ok(PluginOwner::Effect(
            glaux_core::FxId::parse(f).map_err(|e| e.to_string())?,
        )),
        (None, Some(t)) => Ok(PluginOwner::Track(
            glaux_core::TrackId::parse(t).map_err(|e| e.to_string())?,
        )),
        (None, None) => Err("track_id か fx_id を指定してください".to_owned()),
    }
}

fn mutated_json(m: &Mutated) -> Value {
    let mut v = json!({
        "changes": m.changes,
        "project_version": m.project_version,
    });
    if let Some(err) = &m.save_error {
        v["save_error"] = json!(err);
    }
    v
}

/// アクター往復の二重 Result(チャネル断 / CoreError)を平らにする。
fn flatten<T>(r: Result<Result<T, glaux_core::CoreError>, String>) -> Result<T, String> {
    r.and_then(|inner| inner.map_err(|e| e.to_string()))
}

/// `ParamRange` からデフォルト値を JSON で取り出す。
fn range_default(range: &glaux_core::ParamRange) -> Value {
    use glaux_core::ParamRange as R;
    match range {
        R::Float { default, .. } => json!(default),
        R::Int { default, .. } => json!(default),
        R::Bool { default } => json!(default),
        R::Enum { default, .. } => json!(default),
    }
}

/// トラックの音源・エフェクトの「spec + 現在値 + path」ビュー。
/// CLAP エフェクトのつまみを一覧に載せる数の上限(多いものは list_params の filter で探す)
const CLAP_EFFECT_PARAM_LIMIT: usize = 64;

/// MCP の `list_params`(track_id 指定)とアプリの音作りビューが共用する。
/// エフェクトチェーンの spec + 現在値 + path(トラック・マスター共用)。
/// エフェクトの一覧(つまみ込み)。`fx_links` はトラック(マスター)のつながりの表(無ければ並び順の直列)。
/// 各エフェクトに `sounding`(入力から出口まで線でたどれて鳴るか)を付ける
pub fn effects_json(
    effects: &[glaux_core::Effect],
    fx_links: Option<&[glaux_core::FxLink]>,
) -> Vec<Value> {
    let valid = fx_links.filter(|l| glaux_core::model::routing::validate_links(effects, l).is_ok());
    let on = glaux_core::model::routing::sounding(&glaux_core::model::routing::effective_links(
        effects, valid,
    ));
    effects
        .iter()
        .map(|e| {
            let mut v = effect_json(e);
            v["sounding"] = json!(on.contains(&e.id));
            // 表示名・外してあるか・メモ・ノード表示での位置
            let ui = &e.ui;
            if let Some(l) = &ui.label {
                v["label"] = json!(l);
            }
            // つながりの表があるときは parked を使わない(つながっているかは sounding と fx_links で分かる)
            if ui.parked && fx_links.is_none() {
                v["parked"] = json!(true);
            }
            if let Some(n) = &ui.note {
                v["note"] = json!(n);
            }
            if let Some(p) = ui.pos {
                v["pos"] = json!(p);
            }
            v
        })
        .collect()
}

fn effect_json(e: &glaux_core::Effect) -> Value {
    // CLAP エフェクト: プラグインが公開しているつまみ(fx/<id>/clap:<param id>)
    if let glaux_core::PluginSource::Clap { plugin_id, .. } = &e.source {
        let (params, total) = clap_params_json(
            &glaux_engine::plugins::PluginOwner::Effect(e.id.clone()),
            plugin_id,
            &e.params,
            None,
            CLAP_EFFECT_PARAM_LIMIT,
            &format!("fx/{}/", e.id),
        );
        let info = glaux_engine::plugins::find(plugin_id);
        return json!({
            "id": e.id,
            "name": "clap",
            "plugin_id": plugin_id,
            "plugin_name": info.as_ref().map(|i| i.name.clone()),
            "missing": info.is_none(),
            "bypass": e.bypass,
            "params": params,
            "param_total": total,
        });
    }
    let name = match &e.source {
        glaux_core::PluginSource::Builtin { name } => name.clone(),
        other => format!("{other:?}"),
    };
    let fx_params: Vec<Value> = glaux_dsp::effect_params_spec(&name)
        .map(|specs| {
            specs
                .iter()
                .map(|spec| {
                    let current = e
                        .params
                        .get(spec.name)
                        .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
                        .unwrap_or_else(|| range_default(&spec.range));
                    let mut v = serde_json::to_value(spec).expect("ParamSpec serializes");
                    v["path"] = json!(format!("fx/{}/{}", e.id, spec.name));
                    v["current"] = current;
                    v
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "id": e.id,
        "name": name,
        "bypass": e.bypass,
        "params": fx_params,
    })
}

pub fn track_params_json(track: &glaux_core::Track) -> Result<Value, String> {
    track_params_json_filtered(track, None, usize::MAX)
}

/// CLAP プラグインのパラメータ一覧。`filter` は名前・所属の部分一致(大文字小文字を区別しない)。
/// 値は「プロジェクトの上書き値 → プラグインの今の値 → 既定値」の順に採る
/// `path_prefix` はパスの頭(音源なら `device/`、エフェクトなら `fx/<id>/`)。
fn clap_params_json(
    owner: &glaux_engine::plugins::PluginOwner,
    plugin_id: &str,
    overrides: &glaux_core::ParamMap,
    filter: Option<&str>,
    limit: usize,
    path_prefix: &str,
) -> (Vec<Value>, usize) {
    let Some(infos) = glaux_engine::plugins::param_infos(plugin_id) else {
        return (vec![], 0);
    };
    let live = glaux_engine::plugins::live_values(owner).unwrap_or_default();
    let needle = filter.map(str::to_lowercase);
    let matched: Vec<&glaux_clap::ParamInfo> = infos
        .iter()
        .filter(|p| glaux_engine::plugins::is_public_param(p))
        .filter(|p| {
            needle.as_ref().is_none_or(|n| {
                p.name.to_lowercase().contains(n) || p.module.to_lowercase().contains(n)
            })
        })
        .collect();
    let total = matched.len();
    let list = matched
        .into_iter()
        .take(limit)
        .map(|p| {
            let key = glaux_engine::plugins::param_key(p.id);
            let over = match overrides.get(&key) {
                Some(glaux_core::ParamValue::Float(v)) => Some(*v),
                Some(glaux_core::ParamValue::Int(v)) => Some(*v as f64),
                _ => None,
            };
            let (current, text) = match (over, live.get(&p.id)) {
                // 上書き値がプラグインに届いていれば、プラグインの表示文字列も返す
                (Some(v), Some((lv, t))) if (v - lv).abs() <= 1e-6 * (1.0 + v.abs()) => {
                    (v, Some(t.clone()))
                }
                (Some(v), _) => (v, None),
                (None, Some((lv, t))) => (*lv, Some(t.clone())),
                (None, None) => (p.default, None),
            };
            let module = p.module.trim_matches('/');
            let display = if module.is_empty() {
                p.name.clone()
            } else {
                format!("{module} / {}", p.name)
            };
            json!({
                "name": key,
                "path": format!("{path_prefix}{key}"),
                "display_name": display,
                "unit": "",
                "range": {
                    "kind": if p.stepped { "int" } else { "float" },
                    "min": p.min,
                    "max": p.max,
                    "default": p.default,
                },
                "current": current,
                "current_text": text,
                "description": "CLAP プラグインのパラメータ(値はプラグイン固有の単位。current_text が画面上の表示)",
            })
        })
        .collect();
    (list, total)
}

/// [`track_params_json`] の、CLAP プラグインのつまみを絞り込める版。
pub fn track_params_json_filtered(
    track: &glaux_core::Track,
    filter: Option<&str>,
    limit: usize,
) -> Result<Value, String> {
    // CLAP プラグイン: つまみはプラグインが公開するパラメータ。エフェクトは Glaux 側
    if let Some(d) = &track.device {
        if let glaux_core::PluginSource::Clap { plugin_id, .. } = &d.source {
            let (params, total) = clap_params_json(
                &glaux_engine::plugins::PluginOwner::Track(track.id.clone()),
                plugin_id,
                &d.params,
                filter,
                limit,
                "device/",
            );
            return Ok(json!({
                "device": {
                    "name": "clap",
                    "plugin_id": plugin_id,
                    "is_default_fallback": false,
                    "note": "CLAP プラグインのつまみは set_param {track, path: \"device/clap:<id>\", value}(プラグインの単位)で動かせ、\
                             set_automation_points の target にも使える。数が多いので filter で絞り込むこと",
                },
                "params": params,
                "params_total": total,
                // ビブラート・ベンドは 1 音ごとの音程変化として送る(CLAP のノート表現に対応したプラグインのみ)
                "articulations": glaux_dsp::articulations_for("clap"),
                "effects": effects_json(&track.effects, track.fx_links.as_deref()),
                "fx_links": track.fx_links,
            }));
        }
    }
    let (device_name, device_params, is_default) = match &track.device {
        Some(d) => match &d.source {
            glaux_core::PluginSource::Builtin { name } => (name.clone(), d.params.clone(), false),
            glaux_core::PluginSource::Sampler { .. } => {
                ("sampler".to_owned(), d.params.clone(), false)
            }
            glaux_core::PluginSource::Sf2 { .. } => ("sf2".to_owned(), d.params.clone(), false),
            other => return Err(format!("このトラックのデバイスは対応外です({other:?})")),
        },
        None => (
            glaux_dsp::DEFAULT_INSTRUMENT.to_owned(),
            Default::default(),
            true,
        ),
    };
    let specs = glaux_dsp::instrument_params(&device_name)
        .ok_or_else(|| format!("未知の内蔵デバイス: {device_name}"))?;

    let param_list: Vec<Value> = specs
        .iter()
        .map(|spec| {
            let current = device_params
                .get(spec.name)
                .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
                .unwrap_or_else(|| range_default(&spec.range));
            let mut v = serde_json::to_value(spec).expect("ParamSpec serializes");
            v["path"] = json!(format!("device/{}", spec.name));
            v["current"] = current;
            v
        })
        .collect();

    // エフェクトチェーン(spec + current)
    let effects_list = effects_json(&track.effects, track.fx_links.as_deref());

    Ok(json!({
        "device": {
            "name": device_name,
            // true なら device 未設定でデフォルト音源が鳴っている状態
            "is_default_fallback": is_default,
        },
        "params": param_list,
        // この楽器で効く奏法(Note.articulation)。載っていないものは no-op
        "articulations": glaux_dsp::articulations_for(&device_name),
        "effects": effects_list,
        "fx_links": track.fx_links,
    }))
}

impl GlauxServer {
    pub fn new(handle: SessionHandle) -> Self {
        GlauxServer {
            handle,
            tool_router: Self::tool_router(),
        }
    }

    /// 接続中クライアントの名前から `Author::Ai` を作る。
    fn author(&self, ctx: &RequestContext<RoleServer>) -> Author {
        let model = ctx
            .peer
            .peer_info()
            .map(|info| info.client_info.name.clone())
            .unwrap_or_else(|| "unknown".to_owned());
        Author::Ai { model }
    }

    /// 便利ツール共通: MIDI クリップの現在のノート(と選択部分集合)を読む。
    async fn load_notes(
        &self,
        clip_id: &str,
        note_ids: &Option<Vec<String>>,
    ) -> Result<
        (
            glaux_core::ClipId,
            glaux_core::Tick,
            Vec<glaux_core::Note>,
            usize,
        ),
        String,
    > {
        let id = glaux_core::ClipId::parse(clip_id).map_err(|e| e.to_string())?;
        let (project, version) = self.handle.get_project().await?;
        let (_track, clip) = project
            .clip(&id)
            .ok_or_else(|| format!("clip not found: {id}"))?;
        let notes = clip
            .notes()
            .ok_or_else(|| format!("clip {id} は MIDI クリップではありません"))?;
        let selected: Vec<glaux_core::Note> = match note_ids {
            None => notes.to_vec(),
            Some(list) => {
                let mut out = Vec::with_capacity(list.len());
                let mut seen = std::collections::HashSet::new();
                for s in list {
                    let nid = glaux_core::NoteId::parse(s).map_err(|e| e.to_string())?;
                    if !seen.insert(nid.clone()) {
                        continue; // 重複指定は無視
                    }
                    let n = notes
                        .iter()
                        .find(|n| n.id == nid)
                        .ok_or_else(|| format!("note not found in clip {id}: {s}"))?;
                    out.push(n.clone());
                }
                out
            }
        };
        Ok((id, clip.length, selected, version))
    }

    /// 便利ツール共通: 変更を UpdateNotes 1 コマンドとして適用する。
    /// 変更が空(全部が実質 no-op)なら履歴を汚さず changed: 0 を返す。
    async fn apply_note_changes(
        &self,
        clip: glaux_core::ClipId,
        changes: Vec<glaux_core::NoteChange>,
        clamped: usize,
        label: String,
        version: usize,
        ctx: &RequestContext<RoleServer>,
    ) -> ToolResult {
        if changes.is_empty() {
            return Ok(JsonText(json!({
                "project_version": version,
                "changed": 0,
                "note": "対象ノートはすべて変更不要でした",
            })));
        }
        let changed = changes.len();
        let command = Command::UpdateNotes { clip, changes };
        let author = self.author(ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["changed"] = json!(changed);
        if clamped > 0 {
            // 端に当たって値を丸めたことを AI に知らせる(意図とずれている可能性)
            v["clamped"] = json!(clamped);
        }
        Ok(JsonText(v))
    }
}

/// get_project で CLAP プラグインの状態を省略したときの表示(apply_commands で元に戻す目印)
const ELIDED_CLAP_STATE: &str = "(省略: CLAP プラグインの状態 ";

#[tool_router]
impl GlauxServer {
    #[tool(
        description = "プロジェクト全体(トラック・クリップ・パラメータ・テンポ)を JSON で取得する。\
        ノートが多いと巨大になるので、まず include_notes: false で構造を把握し、必要なクリップ(clip_ids)や\
        範囲(start_tick / end_tick)だけを note_format: \"compact\"(ノートを [id, pos, dur, pitch, vel] の配列にする。量が約半分)で取得するとよい。\
        返り値の project_version は版数(編集・undo・redo のたびに増え、戻らない)。自分が最後に見た値より大きければ、その間に誰かが変更している。"
    )]
    async fn get_project(&self, params: Parameters<GetProjectParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("get_project");
        let p = params.0;
        let (mut project, version) = self.handle.get_project().await?;
        let compact = match p.note_format.as_deref() {
            None | Some("full") => false,
            Some("compact") => true,
            Some(other) => return Err(format!("note_format は full / compact(got: {other})")),
        };
        let range = match (p.start_tick, p.end_tick) {
            (None, None) => None,
            (s, e) => {
                let (s, e) = (s.unwrap_or(0), e.unwrap_or(u64::MAX));
                if e <= s {
                    return Err("end_tick は start_tick より大きくすること".to_owned());
                }
                Some((s, e))
            }
        };
        // JSON にする前に、型のまま絞り込む(全体を JSON にしてから削ると、大きな曲で毎回重い)
        if let Some(ids) = &p.track_ids {
            project
                .tracks
                .retain(|t| ids.iter().any(|x| x == t.id.as_str()));
        }
        // 範囲で削ったクリップ(ID → 元のノート数)
        let mut trimmed: std::collections::HashMap<String, usize> = Default::default();
        if p.clip_ids.is_some() || range.is_some() {
            for t in &mut project.tracks {
                t.clips.retain(|c| {
                    let by_id = p
                        .clip_ids
                        .as_ref()
                        .is_none_or(|ids| ids.iter().any(|x| x == c.id.as_str()));
                    let by_range =
                        range.is_none_or(|(s, e)| c.start.0 < e && c.start.0 + c.length.0 > s);
                    by_id && by_range
                });
                if let Some((s, e)) = range {
                    for c in &mut t.clips {
                        let start = c.start.0;
                        let looped = c.loop_len().is_some();
                        let id = c.id.to_string();
                        if let Some(notes) = c.notes_mut() {
                            // ループクリップは繰り返しの位置が分かりにくいので削らない
                            if looped {
                                continue;
                            }
                            let before = notes.len();
                            notes.retain(|n| {
                                let a = start + n.pos.0;
                                a < e && a + n.dur.0 > s
                            });
                            if notes.len() != before {
                                trimmed.insert(id, before);
                            }
                        }
                    }
                }
            }
            if p.clip_ids.is_some() {
                project.tracks.retain(|t| !t.clips.is_empty());
            }
        }
        let mut v = serde_json::to_value(&project).map_err(|e| e.to_string())?;
        let include_notes = p.include_notes.unwrap_or(true);
        let include_automation = p.include_automation.unwrap_or(true);
        // CLAP プラグインの状態は巨大な不透明データなので省略して見せる
        let elide = |d: &mut Value| {
            if d.get("type").and_then(Value::as_str) != Some("clap") {
                return;
            }
            if let Some(state) = d.get_mut("state") {
                let len = state.as_str().map_or(0, str::len);
                *state = json!(format!("{ELIDED_CLAP_STATE}{len} 文字)"));
            }
        };
        if let Some(fx) = v
            .get_mut("master")
            .and_then(|m| m.get_mut("effects"))
            .and_then(Value::as_array_mut)
        {
            fx.iter_mut().for_each(elide);
        }
        if let Some(tracks) = v.get_mut("tracks").and_then(Value::as_array_mut) {
            for track in tracks {
                if let Some(d) = track.get_mut("device") {
                    elide(d);
                }
                if let Some(fx) = track.get_mut("effects").and_then(Value::as_array_mut) {
                    fx.iter_mut().for_each(elide);
                }
                if !include_automation {
                    if let Some(a) = track.get_mut("automation") {
                        *a = json!([]);
                    }
                }
                if let Some(clips) = track.get_mut("clips").and_then(Value::as_array_mut) {
                    for clip in clips.iter_mut() {
                        let id = clip.get("id").and_then(Value::as_str).unwrap_or_default();
                        if let Some(total) = trimmed.get(id) {
                            clip["notes_in_range"] = json!(true);
                            clip["note_count"] = json!(total);
                        }
                        if compact && include_notes {
                            if let Some(notes) = clip.get_mut("notes").and_then(Value::as_array_mut)
                            {
                                for n in notes.iter_mut() {
                                    *n = compact_note(n);
                                }
                            }
                        }
                    }
                }
                if !include_notes {
                    if let Some(clips) = track.get_mut("clips").and_then(Value::as_array_mut) {
                        for clip in clips {
                            if clip.get("kind").and_then(Value::as_str) == Some("midi") {
                                let count = clip
                                    .get("notes")
                                    .and_then(Value::as_array)
                                    .map_or(0, Vec::len);
                                clip["note_count"] = json!(count);
                                clip["notes"] = json!([]);
                            }
                        }
                    }
                }
            }
        }

        let mut out = json!({ "project_version": version, "project": v });
        if compact {
            out["note_fields"] = json!(["id", "pos", "dur", "pitch", "vel", "extra?"]);
        }
        Ok(JsonText(out))
    }

    #[tool(
        description = "コマンドを適用してプロジェクトを編集する。唯一の編集手段。\
        commands には glaux の Command JSON({\"op\": ..., ...})を並べる。複数渡すと 1 つの Batch になり、1 回の undo でまとめて戻せる。\
        新規 ID(トラック trk_、クリップ clp_、ノート nt_、エフェクト fx_ + 英数 6 桁。例 trk_a1b2c3)は省略するとサーバーが振り、\
        ノート以外は返り値 assigned_ids で返す(同じ呼び出しの中で後から参照するトラック・クリップは自分で付ける)。\
        相対操作(「半音上げる」等)は不可。現在値を読んで絶対値を計算してから送ること。\
        ただしノートの移調・時間移動・クオンタイズ・ベロシティ一括調整は\
        専用ツール(transpose_notes / shift_notes / quantize_notes / scale_velocity)の方が速くて確実。\
        クリップの複製・小節の挿入と削除は duplicate_clips / insert_bars / delete_bars。\
        代表例: add_track {track,index?} / add_clip {track,clip} / add_notes {clip,notes} / update_notes {clip,changes} / \
        バス(リターン): add_track の kind: \"bus\" で作る(クリップは置けない。エフェクトを挿して共有リバーブ・ディレイにする。\
        リバーブは mix: 1.0 = ウェットのみが基本)。set_send {track, target, level_db, pre_fader?} でトラックからバスへ送る\
        (level_db -60〜12。省略でセンドを外す。pre_fader: true でフェーダー前 = トラック音量に追従しない)。\
        送り元はバス以外、送り先はバスのみ。バスはソロの影響を受けない。ボーカル・スネア・パッドを同じ空間に置くのに使う / \
        set_track_prop {id,prop,value} / set_param {track,path,value} / set_tempo {events} / move_clip {id,start,track?} / \
        set_title {title}(曲名の変更)/ \
        set_sections {sections: [{tick, name}]}(曲の構成マーカーを丸ごと置換。\
        intro / Aメロ / サビ 等。各セクションはそのマーカーから次のマーカーの手前まで。\
        構成を決めたら早めに打っておくと「サビだけ〜して」の指示を tick 範囲に解決できる)/ \
        ノートには articulation を付けられる: \"palm_mute\"(ブリッジミュート。減衰が速いこもった刻み)/ \
        \"staccato\"(音価半分で切る)/ \"accent\"(強く明るく)/ \
        \"vibrato\"(後半にかけて深くなるピッチの揺れ。ロングトーンの表情付け)/ \
        \"bend\"(チョーキング: 全音下から書かれた音程へ滑り上がる。ギターソロの決め音に)/ \
        \"legato\"(同じトラックの直前の音から弾き直さずにつなぐ。弦・管・歌・リードのフレーズ、ギターのハンマリング。\
        前の音との隙間 0.3 秒まで。つなげたい 2 音目以降に付ける)/ \
        \"portamento\"(legato でつなぎ、直前の音の高さから約 0.15 秒で滑らせる。直前の音が無い(フレーズの頭・\
        0.3 秒より離れた)ときは全音下から滑り込む。ストリングスのポルタメント・\
        シンセのグライド・ギターのスライド)。\
        省略で通常。update_notes でも変更可。\
        滑る時間は、トラック全体なら set_param {track, path: \"track/glide_ms\", value}(10〜2000ms、既定 150)、\
        1 音だけならノートの glide_ms(add_notes / update_notes。0 で解除してトラックの値へ)。\
        レガートのつなぎ目の長さは track/legato_ms(5〜200ms、既定 30。長いほどふんわり重なる)。\
        どちらも unset_param で既定に戻る。\
        連続ピッチカーブ: ノートの pitch_curve に [{tick, cents}](tick はノート先頭からの相対、\
        cents は書かれた音程からのずれ。100 = 半音、±2400 まで、最大 8 点、点の間は線形補間、\
        両端は保持)を書くと自由なベンド・ポルタメント・うねりが作れる\
        (例: ギターのチョーキングを 1 拍かけて上げる = [{tick:0,cents:-200},{tick:960,cents:0}]、\
        ダイブ = [{tick:0,cents:0},{tick:1920,cents:-1200}])。update_notes の pitch_curve で差し替え、[] で削除。\
        メタルの「ズクズク」した刻みは pluck + amp(gain_db 40 以上)+ 低音 + palm_mute ノートの組み合わせで作る。\
        マスターバスのエフェクトは add_master_effect {effect, index?} / set_master_param {path: \"fx/<id>/<名前>\", value} / \
        unset_master_param {path}、削除・並べ替え・バイパスはトラックと同じ remove_effect / move_effect {id, to_index} / set_effect_bypass \
        / set_effect_prop {id, prop: \"label\" | \"parked\" | \"note\" | \"pos\", value}(表示名・線から外す・メモ・ノード表示の位置。\
        parked: true のエフェクトは鳴らないが設定は残る。ユーザーが取っておいたものなので、頼まれない限り消さない)/ \
        set_fx_links {track?, links}(エフェクトのつながり = ノード表示の線を丸ごと置き換える。track 省略でマスター。\
        links は [{from, to, gain_db?}] で、端は \"in\"(音源・受けた音)/ \"out\"(音量・パンへ)/ エフェクト ID。\
        1 つの口から何本でも出せ(分岐 = 同じ音を配る)、1 つの口に何本でも入れられる(合流 = 足し合わせる)。\
        入力から出口まで線でたどれるエフェクトだけが鳴る(各エフェクトの sounding で分かる)。輪は不可。\
        例: 原音とリバーブを並列に混ぜる = [in→eq, eq→out, eq→rev(gain_db -8), rev→out]。null で並び順の直列に戻す。\
        つながりの表(get_project の tracks[].fx_links)があるトラックでは parked は使えず、add_effect は出口の直前に入り、\
        remove_effect は前後をつなぎ直す。表を書き換える前に今の表を読み、ユーザーのつなぎ方を勝手に崩さないこと)\
        (マスターのチェーンは get_project の master.effects で見える。仕上げのコンプ・EQ・リミッター的な使い方に)/ \
        set_clip_loop {id, loop_len}(MIDI クリップのループ。loop_len に繰り返す長さ(クリップ先頭から、\
        tick)を渡すと、クリップ長までその範囲が繰り返し鳴る。null で解除。ドラムパターンやリフは \
        1〜2 小節を作ってループにし、resize_clip で伸ばすのが速い。ループ範囲より後ろのノートは鳴らない)/ \
        set_clip_stretch {id, stretch}(音声クリップのテンポ追従。stretch は {mode: \"follow\", original_bpm} \
        で素材を original_bpm の演奏として扱い、曲のテンポを変えても拍がずれないよう音程を保ったまま伸縮する。\
        録音・取り込んだときの曲のテンポを original_bpm に入れるのが基本。{mode: \"none\"} で解除)/ \
                set_automation_points {track,target,points}(target は \"track/volume_db\" / \"track/pan\" / \
        \"device/<パラメータ名>\"(例 device/cutoff。list_params にある連続値パラメータ。\
        値はパラメータと同じ単位)/ \"fx/<エフェクト ID>/<パラメータ名>\"(そのトラックのエフェクト。\
        例 リバーブの mix をサビで上げる、EQ の high_gain_db を開いていく)、points は [{tick,value,curve?}] で curve は \
        linear/hold/exponential。フェードイン・ビルドアップの音量カーブ・左右の揺れ・\
        フィルタスイープなど時間変化する表現に使う。\
        レーンがあるとフェーダー/つまみの値より優先。空配列でレーン削除)/ \
        set_master_automation_points {target,points}(マスターのレーン。target は \"track/volume_db\"\
        (曲全体のフェードアウト等)か \"fx/<マスターのエフェクト ID>/<パラメータ名>\"。\
        レーンは get_project の master.automation で見える)。\
        失敗時はどのコマンドで失敗したかがエラーメッセージに入る(batch failed at command #N)。"
    )]
    async fn apply_commands(
        &self,
        params: Parameters<ApplyCommandsParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("apply_commands");
        let p = params.0;
        if p.commands.is_empty() {
            return Err("commands が空です".to_owned());
        }
        let mut commands = Vec::with_capacity(p.commands.len());
        let mut current: Option<glaux_core::Project> = None;
        let mut assigned: Vec<Value> = Vec::new();
        for (i, mut value) in p.commands.into_iter().enumerate() {
            // 省略された ID はここで振る(コマンドは決定的なので、apply ではなく作る側 = MCP 層で)
            assign_missing_ids(&mut value, i, &mut assigned);
            let mut cmd: Command = serde_json::from_value(value)
                .map_err(|e| format!("commands[{i}] を Command として解釈できません: {e}"))?;
            // get_project で省略表示した CLAP の状態をそのまま送ってきたら、今の状態に戻す
            if let Command::SetDevice {
                track,
                device:
                    Some(glaux_core::Device {
                        source: glaux_core::PluginSource::Clap { plugin_id, state },
                        ..
                    }),
            } = &mut cmd
            {
                if state
                    .as_deref()
                    .is_some_and(|s| s.starts_with(ELIDED_CLAP_STATE))
                {
                    if current.is_none() {
                        current = Some(self.handle.get_project().await?.0);
                    }
                    *state = current
                        .as_ref()
                        .and_then(|p| p.track(track))
                        .and_then(|t| t.device.as_ref())
                        .and_then(|d| match &d.source {
                            glaux_core::PluginSource::Clap {
                                plugin_id: cur_id,
                                state,
                            } if cur_id == plugin_id => state.clone(),
                            _ => None,
                        });
                }
            }
            // エフェクトの状態も同様(同じ ID のエフェクトの今の状態。無ければ既定の状態)
            let fx_state = match &mut cmd {
                Command::AddEffect { effect, .. } | Command::AddMasterEffect { effect, .. } => {
                    match &mut effect.source {
                        glaux_core::PluginSource::Clap { state, .. } => {
                            Some((effect.id.clone(), state))
                        }
                        _ => None,
                    }
                }
                Command::SetEffectState { id, state } => Some((id.clone(), state)),
                _ => None,
            };
            if let Some((id, state)) = fx_state {
                if state
                    .as_deref()
                    .is_some_and(|s| s.starts_with(ELIDED_CLAP_STATE))
                {
                    if current.is_none() {
                        current = Some(self.handle.get_project().await?.0);
                    }
                    *state = current.as_ref().and_then(|p| {
                        p.tracks
                            .iter()
                            .flat_map(|t| t.effects.iter())
                            .chain(p.master.effects.iter())
                            .find(|e| e.id == id)
                            .and_then(|e| match &e.source {
                                glaux_core::PluginSource::Clap { state, .. } => state.clone(),
                                _ => None,
                            })
                    });
                }
            }
            commands.push(cmd);
        }
        let command = if commands.len() == 1 {
            commands.pop().expect("len checked")
        } else {
            Command::batch(p.label.clone(), commands)
        };

        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, p.label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        if !assigned.is_empty() {
            v["assigned_ids"] = json!(assigned);
        }
        Ok(JsonText(v))
    }

    #[tool(
        description = "直前の編集を取り消す(n 回分。既定 1)。apply_commands の 1 呼び出し(Batch)が 1 回分。\
        返り値 undone が実際に取り消せた回数(履歴の先頭に達すると少なくなる)。"
    )]
    async fn undo(&self, params: Parameters<UndoRedoParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("undo");
        let n = params.0.n.unwrap_or(1) as usize;
        let (undone, m) = flatten(self.handle.undo(n).await)?;
        let mut v = mutated_json(&m);
        v["undone"] = json!(undone);
        Ok(JsonText(v))
    }

    #[tool(
        description = "undo で取り消した編集をやり直す(n 回分。既定 1)。返り値 redone が実際にやり直せた回数。"
    )]
    async fn redo(&self, params: Parameters<UndoRedoParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("redo");
        let n = params.0.n.unwrap_or(1) as usize;
        let (redone, m) = flatten(self.handle.redo(n).await)?;
        let mut v = mutated_json(&m);
        v["redone"] = json!(redone);
        Ok(JsonText(v))
    }

    #[tool(description = "現在の状態にチェックポイント名を付ける(git tag 相当)。\
        試行錯誤の前に打っておき、気に入らなければ revert_to で一括で戻る、という使い方を推奨。\
        同名で打ち直すと上書き。新しい編集をすると、それより先にあったチェックポイントは消える。")]
    async fn checkpoint(&self, params: Parameters<CheckpointParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("checkpoint");
        let label = params.0.label;
        let m = self.handle.checkpoint(label.clone()).await?;
        let mut v = mutated_json(&m);
        v["label"] = json!(label);
        Ok(JsonText(v))
    }

    #[tool(
        description = "チェックポイントまで編集を巻き戻す(undo の繰り返し)。巻き戻した分は redo でやり直せる。\
        チェックポイント名が存在しないとエラー。"
    )]
    async fn revert_to(&self, params: Parameters<RevertToParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("revert_to");
        let m = flatten(self.handle.revert_to(params.0.label).await)?;
        Ok(JsonText(mutated_json(&m)))
    }

    #[tool(
        description = "履歴の途中のエントリを 1 件だけ取り消す(git revert 相当)。\
        逆コマンドが新しいエントリとして積まれるので、取り消した事実も履歴に残り、それ自体も undo できる。\
        undo と違い、そのエントリより後の編集は保持される。\
        「さっきの AI のあの編集だけ戻して」に使う。entry_id は get_history で確認。\
        返り値の conflicts に ID が入っている場合、後続の編集が同じ対象を触っており\
        意図しない結果になっている可能性があるので、get_project で結果を確認すること。"
    )]
    async fn revert(
        &self,
        params: Parameters<RevertEntryParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("revert");
        let id = EntryId::parse(&params.0.entry_id).map_err(|e| e.to_string())?;
        let author = self.author(&ctx);
        let (entry, conflicts, m) = flatten(self.handle.revert_entry(id, author).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry);
        v["conflicts"] = json!(conflicts);
        Ok(JsonText(v))
    }

    #[tool(
        description = "楽器・エフェクトのパラメータ仕様と現在値を返す。つまみを理解する唯一の情報源。\
        track_id を省略するとカタログ: 内蔵楽器(subtractive / drum / pluck / fm / wavetable 等)と内蔵エフェクト(eq / compressor / multiband / transient / limiter / reverb / delay / chorus / tape 等)の\
        全パラメータ仕様(範囲と聴感上の効果)と、今のマスターのエフェクトチェーン(master_effects)を返す。\
        track_id を指定するとそのトラックの現在のデバイスとエフェクトチェーン(spec + current)を返す。\
        音源の設定は set_device(例: {\"op\":\"set_device\",\"track\":\"trk_x\",\"device\":{\"type\":\"builtin\",\"name\":\"drum\"}})、\
        エフェクト追加は add_effect(例: {\"op\":\"add_effect\",\"track\":\"trk_x\",\"effect\":{\"id\":\"fx_a1b2c3\",\"type\":\"builtin\",\"name\":\"reverb\"}})。\
        つまみは set_param で、path は楽器 \"device/<名前>\"、エフェクト \"fx/<fx_id>/<名前>\"。\
        CLAP プラグインのトラックではプラグインのつまみ(path \"device/clap:<id>\"、値はプラグインの単位)が返る。\
        CLAP エフェクトは effects の中に name: \"clap\"、plugin_name、つまみ(path \"fx/<fx_id>/clap:<id>\"、最大 64 個)で返る。\
        数百個あるので filter(例 \"cutoff\"、\"filter\"、\"attack\")で絞り込み、current_text で画面上の表示を確認すること。\
        ドラムトラックには必ず drum を設定すること(未設定は subtractive で鳴る)。"
    )]
    async fn list_params(&self, params: Parameters<ListParamsParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("list_params");
        let (project, version) = self.handle.get_project().await?;

        let Some(track_id) = params.0.track_id else {
            // カタログモード(+ 今のマスターのエフェクトチェーン)
            let master = project.master.clone();
            let master_effects = tokio::task::spawn_blocking(move || {
                effects_json(&master.effects, master.fx_links.as_deref())
            })
            .await
            .map_err(|e| e.to_string())?;
            return Ok(JsonText(json!({
                "project_version": version,
                "instruments": glaux_dsp::instrument_catalog(),
                "effects": glaux_dsp::effect_catalog(),
                "default_instrument": glaux_dsp::DEFAULT_INSTRUMENT,
                "master_effects": master_effects,
            })));
        };

        let track_id = glaux_core::TrackId::parse(&track_id).map_err(|e| e.to_string())?;
        let track = project
            .track(&track_id)
            .ok_or_else(|| format!("track not found: {track_id}"))?;

        let track = track.clone();
        let (filter, limit) = (params.0.filter, params.0.limit.unwrap_or(80));
        // CLAP のつまみ一覧は初回にプラグインを読み込むことがあるので別スレッドで
        let mut v = tokio::task::spawn_blocking(move || {
            track_params_json_filtered(&track, filter.as_deref(), limit)
        })
        .await
        .map_err(|e| e.to_string())??;
        v["project_version"] = json!(version);
        v["track_id"] = json!(track_id);
        Ok(JsonText(v))
    }

    #[tool(
        description = "あなたの「耳」。プロジェクトをオフラインレンダして音響指標を返す(音声そのものは返らない)。\
        編集 → analyze_audio → 微調整のループでミックスの質を上げるのに使う。指標の読み方: \
        loudness_lufs=統合ラウドネス(配信の目安 -14 前後。-30 以下はかなり小さい)、\
        peak_db が 0 に近く clipped=true なら歪んでいるのでゲインを下げる、\
        crest_factor_db=ダイナミクス(6 以下は潰れ気味、12 以上はスカスカかも)、\
        band_energy=low(<250Hz)/mid/high(>4kHz) の比率(low>0.6 はこもり気味、high>0.5 は刺さり気味。\
        バランスの取れた曲はおおむね low 0.3-0.5 / mid 0.3-0.5 / high 0.05-0.25)、\
        spectral_centroid_hz=明るさの重心、onsets_ticks=発音タイミング(リズムの確認用)。\
        track_ids に 1 トラックだけ渡せば単体を聴ける。start/end_tick で範囲を絞れる(範囲指定の指示と併用推奨)。\
        loudness_range_lu=曲中の音量の起伏(小さいと平板)、true_peak_dbtp=サンプル間のピーク(配信は -1 以下が目安)、\
        short_term_lufs=1 秒ごとの短期ラウドネスの推移(展開・盛り上がりの確認)、\
        plr_db=True Peak − 統合ラウドネス(小さいほど潰れている)、psr_min_db=いちばん詰まった所のピークと短期ラウドネスの差(実務の目安は 8 以上)、\
        streaming=Spotify / Apple Music / YouTube / AES77 で再生されたときの音量の調整の予測(gain_db がマイナスなら下げられる。\
        大きく下げられるなら音圧を上げすぎ。正規化される配信では、潰して大きくしても得をしない)、\
        stereo(correlation=左右の相関。1=モノラル、負=逆相で危険 / low_correlation=250Hz 以下の相関。低域は 1 近くが望ましい /\
        side_to_mid_db=広がり / balance_db=左右の偏り)。\
        【ミックスバランスの診断】per_track: true で各トラックの loudness/band_energy 一覧と、\
        masking(トラック間の周波数のかぶり: track が masked_by に band_hz の帯域で time_ratio の時間 6dB 以上負けている。\
        band_share はその帯域が track の音に占める割合)が返る。かぶりは EQ で片方を削る・パンで分ける・sidechain で解消する。\
        目立たせたいトラック(リード/ボーカル的存在)は伴奏より 2〜4dB 上、\
        同じ帯域に重心が密集していたら EQ で住み分け(片方の被り帯域を削る)か\
        sidechain で空間を空ける。「あるトラックが埋もれる」相談ではまず per_track で全体像を見ること。"
    )]
    async fn analyze_audio(&self, params: Parameters<AnalyzeAudioParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("analyze_audio");
        let p = params.0;
        let (project, version) = self.handle.get_project().await?;

        let track_ids: Option<Vec<glaux_core::TrackId>> = match &p.track_ids {
            None => None,
            Some(ids) => Some(
                ids.iter()
                    .map(|s| glaux_core::TrackId::parse(s).map_err(|e| e.to_string()))
                    .collect::<Result<_, _>>()?,
            ),
        };
        let range = match (p.start_tick, p.end_tick) {
            (None, None) => None,
            (s, e) => {
                let start = s.unwrap_or(0);
                let end = e.unwrap_or(u64::MAX);
                if end <= start {
                    return Err("end_tick は start_tick より大きくすること".to_owned());
                }
                Some((glaux_core::Tick(start), glaux_core::Tick(end)))
            }
        };

        // レンダ + FFT は CPU バウンドなのでブロッキングスレッドで
        let per_track = p.per_track.unwrap_or(false);
        let project_dir = self.handle.project_dir().await?;
        let (analysis, track_summaries) = tokio::task::spawn_blocking(move || {
            // サンプラー音源の WAV を読み込む(オフライン解析なのでキャッシュなしでよい)
            let bank =
                glaux_engine::SampleBank::for_offline(&project, std::path::Path::new(&project_dir));
            let a = glaux_engine::analyze_project(&project, track_ids.as_deref(), range, &bank);
            let t = if per_track {
                Some(glaux_engine::analyze_mix(&project, range, &bank))
            } else {
                None
            };
            (a, t)
        })
        .await
        .map_err(|e| e.to_string())?;
        let analysis = analysis.map_err(|e| format!("解析できません: {e}"))?;

        let mut v = serde_json::to_value(&analysis).map_err(|e| e.to_string())?;
        v["project_version"] = json!(version);
        if let Some(mix) = track_summaries {
            let mut tracks = mix.tracks;
            v["masking"] = serde_json::to_value(&mix.masking).map_err(|e| e.to_string())?;
            // うるさい順に並べる(バランス診断で読みやすい)
            tracks.sort_by(|a, b| {
                b.loudness_lufs
                    .partial_cmp(&a.loudness_lufs)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            v["tracks"] = serde_json::to_value(&tracks).map_err(|e| e.to_string())?;
        }
        Ok(JsonText(v))
    }

    #[tool(
        description = "編集の前と後を、同じ条件でレンダして比べる(聴き比べ)。ミックスの編集が本当に良くなったかの確認に使う。\
        「前」は checkpoint(名前)・before_entry(履歴エントリ ID の直前)・back(最新から n 個戻した所)のどれかで指定\
        (省略で直前の 1 編集の前)。今のプロジェクトは変えない。\
        【大事】人の耳は 0.5〜1dB 大きいだけで「良くなった」と感じる。loudness_diff_db が 0 でなければ、\
        その差で良し悪しを決めないこと。tonal_balance(オクターブ帯域ごとの、それぞれ全体に対する dB。音量差の影響を受けない)・\
        stereo・plr_db / psr_min_db(ダイナミクス)・loudness_range_lu で比べる。match_gain_db は後の方に掛けると前と同じ音量になる量。\
        notes に目立つ違いの要約が入る。編集で音量まで変えたくなければ、音量(フェーダー・makeup)を match_gain_db の分だけ戻して\
        もう一度比べるとよい。track_ids・start/end_tick は analyze_audio と同じ。"
    )]
    async fn compare_mix(&self, params: Parameters<CompareMixParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("compare_mix");
        let p = params.0;
        let point = match (p.checkpoint, p.before_entry, p.back) {
            (Some(c), None, None) => glaux_core::HistoryPoint::Checkpoint(c),
            (None, Some(e), None) => glaux_core::HistoryPoint::BeforeEntry(
                glaux_core::EntryId::parse(&e).map_err(|e| e.to_string())?,
            ),
            (None, None, Some(n)) => glaux_core::HistoryPoint::Back(n),
            (None, None, None) => glaux_core::HistoryPoint::Back(1),
            _ => {
                return Err(
                    "checkpoint / before_entry / back はどれか 1 つだけ指定すること".to_owned(),
                )
            }
        };
        let (before, after, version, back) = self
            .handle
            .project_at(point)
            .await?
            .map_err(|e| e.to_string())?;
        if back == 0 {
            return Err("「前」が今と同じ地点です(比べる編集がありません)".to_owned());
        }
        let track_ids: Option<Vec<glaux_core::TrackId>> = match &p.track_ids {
            None => None,
            Some(ids) => Some(
                ids.iter()
                    .map(|s| glaux_core::TrackId::parse(s).map_err(|e| e.to_string()))
                    .collect::<Result<_, _>>()?,
            ),
        };
        let range = match (p.start_tick, p.end_tick) {
            (None, None) => None,
            (s, e) => {
                let start = s.unwrap_or(0);
                let end = e.unwrap_or(u64::MAX);
                if end <= start {
                    return Err("end_tick は start_tick より大きくすること".to_owned());
                }
                Some((glaux_core::Tick(start), glaux_core::Tick(end)))
            }
        };
        let project_dir = self.handle.project_dir().await?;
        let cmp = tokio::task::spawn_blocking(move || {
            let dir = std::path::Path::new(&project_dir);
            let bank_before = glaux_engine::SampleBank::for_offline(&before, dir);
            let bank_after = glaux_engine::SampleBank::for_offline(&after, dir);
            glaux_engine::compare_projects(
                &before,
                &after,
                track_ids.as_deref(),
                range,
                &bank_before,
                &bank_after,
            )
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("比べられません: {e}"))?;
        let mut v = serde_json::to_value(&cmp).map_err(|e| e.to_string())?;
        v["edits_compared"] = json!(back);
        v["project_version"] = json!(version);
        Ok(JsonText(v))
    }

    #[tool(
        description = "あなたの「音楽理論の目」。ノートデータからキーと小節ごとのコード進行を推定する\
        (オーディオではなく記号的分析。ドラムトラックは自動で除外)。\
        作曲・アレンジの前にまずこれで現状の調性を把握するとよい: \
        メロディを足すときはキーのスケール音を基本に、ハモリ・ベースはその小節のコードトーンから選ぶ。\
        confidence が低い小節は経過音が多いか複合和音なので鵜呑みにしない。\
        out_of_key_ratio が高い(> 0.15)場合は転調や借用和音を含む可能性がある。\
        すべて推定値であり、意図的な不協和・転調を「修正」しないこと。"
    )]
    async fn analyze_harmony(&self, params: Parameters<AnalyzeHarmonyParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("analyze_harmony");
        let p = params.0;
        let (project, version) = self.handle.get_project().await?;
        let track_ids: Option<Vec<glaux_core::TrackId>> = match &p.track_ids {
            None => None,
            Some(ids) => Some(
                ids.iter()
                    .map(|s| glaux_core::TrackId::parse(s).map_err(|e| e.to_string()))
                    .collect::<Result<_, _>>()?,
            ),
        };
        let range = match (p.start_tick, p.end_tick) {
            (None, None) => None,
            (s, e) => Some((
                glaux_core::Tick(s.unwrap_or(0)),
                glaux_core::Tick(e.unwrap_or(u64::MAX)),
            )),
        };
        let analysis = glaux_core::harmony::analyze(&project, track_ids.as_deref(), range);
        let mut v = serde_json::to_value(&analysis).map_err(|e| e.to_string())?;
        v["project_version"] = json!(version);
        Ok(JsonText(v))
    }

    #[tool(
        description = "あなたの「リズム感」。ノートの発音位置からグルーヴを推定する: \
        swing_ratio(1.0=ストレート、1.33≈3 連シャッフル)/ grid(straight | triplet)/ \
        syncopation(裏に乗る発音の割合)/ avg_deviation_ticks(グリッドからのずれ。\
        0=機械的、15〜40≈ヒューマナイズ)/ density_per_bar。\
        既存の曲にフレーズを足すときは、まずこれで「ノリ」を測り、\
        同じスウィング・同じグリッドで書くこと(ストレートな曲に 3 連を混ぜない、逆も同様)。\
        ドラムだけの track_ids 指定でビートのノリ、メロディだけ指定でフレージングのノリを個別に見られる。"
    )]
    async fn analyze_rhythm(&self, params: Parameters<AnalyzeRhythmParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("analyze_rhythm");
        let p = params.0;
        let (project, version) = self.handle.get_project().await?;
        let track_ids: Option<Vec<glaux_core::TrackId>> = match &p.track_ids {
            None => None,
            Some(ids) => Some(
                ids.iter()
                    .map(|s| glaux_core::TrackId::parse(s).map_err(|e| e.to_string()))
                    .collect::<Result<_, _>>()?,
            ),
        };
        let range = match (p.start_tick, p.end_tick) {
            (None, None) => None,
            (s, e) => Some((
                glaux_core::Tick(s.unwrap_or(0)),
                glaux_core::Tick(e.unwrap_or(u64::MAX)),
            )),
        };
        let analysis = glaux_core::rhythm::analyze(&project, track_ids.as_deref(), range);
        let mut v = serde_json::to_value(&analysis).map_err(|e| e.to_string())?;
        v["project_version"] = json!(version);
        Ok(JsonText(v))
    }

    #[tool(
        description = "編集履歴の一覧を返す(古い→新しい)。各エントリはコマンド本体を含まない軽量ビュー。\
        author: \"ai\" で自分(AI)の過去の作業だけを振り返れる。前回確認済みの位置からは since に最後に見たエントリ ID を渡す。\
        セッションをまたいでも履歴はプロジェクトに保存されている。"
    )]
    async fn get_history(&self, params: Parameters<GetHistoryParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("get_history");
        let p = params.0;
        if let Some(a) = p.author.as_deref() {
            if !matches!(a, "human" | "ai" | "system") {
                return Err(format!(
                    "author は human / ai / system のいずれか(got: {a})"
                ));
            }
        }
        let since = match p.since.as_deref() {
            Some(s) => Some(EntryId::parse(s).map_err(|e| e.to_string())?),
            None => None,
        };
        // 何も指定しないと数千件になりうるので、最新 50 件に絞る(total で全件数が分かる)
        let limit = match (p.limit, &since) {
            (Some(n), _) => Some(n as usize),
            (None, Some(_)) => None,
            (None, None) => Some(DEFAULT_HISTORY_LIMIT),
        };
        let page = flatten(self.handle.get_history(p.author, since, limit).await)?;
        Ok(JsonText(
            serde_json::to_value(page).map_err(|e| e.to_string())?,
        ))
    }

    #[tool(
        description = "インストール済みの CLAP プラグイン(外部の音源・エフェクト)を一覧する。\
        音源(instrument: true)は set_device {track, device: {type: \"clap\", plugin_id}} でトラックの音源にできる\
        (Surge XT・Vital・TAL-NoiseMaker などの本格的なシンセ)。音色の大枠はプラグイン自身の画面で人間が作るが、\
        つまみは list_params {track_id, filter} で探して set_param {path: \"device/clap:<id>\"} で動かせ、\
        set_automation_points の target にもできる(フィルタスイープ等)。\
        エフェクト(effect: true。リバーブ・ディレイ・コーラス・マスタリング系など)は add_effect / add_master_effect の\
        effect に {id, type: \"clap\", plugin_id} を渡してトラック・マスターのチェーンに挿せる(内蔵エフェクトと混ぜてよい。\
        チェーンの順に通る)。つまみは list_params {track_id} の effects(path \"fx/<fx_id>/clap:<id>\")で見て set_param /\
        set_master_param / set_automation_points で動かす。プリセットは list_plugin_presets / load_plugin_preset に fx_id。\
        get_project では CLAP の状態(state)は省略表示になる。音源を差し替えるときは state を付けないこと。\
        rescan: true でインストールし直したプラグインを探し直す。"
    )]
    async fn list_plugins(&self, params: Parameters<ListPluginsParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("list_plugins");
        let rescan = params.0.rescan.unwrap_or(false);
        let list = tokio::task::spawn_blocking(move || {
            if rescan {
                glaux_engine::plugins::rescan()
            } else {
                glaux_engine::plugins::catalog()
            }
        })
        .await
        .map_err(|e| e.to_string())?;
        Ok(JsonText(json!({
            "plugins": list.iter().map(|p| json!({
                "id": p.id,
                "name": p.name,
                "vendor": p.vendor,
                "instrument": p.is_instrument(),
                "effect": p.is_effect(),
                "features": p.features,
            })).collect::<Vec<_>>(),
            "search_dirs": glaux_engine::plugins::search_paths()
                .iter()
                .map(|d| d.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
        })))
    }

    #[tool(
        description = "あなたの「音色を聴き分ける耳」。1 つの音(単音のサンプル・音声クリップ・トラックの音源で鳴らした 1 音)を\
        細かく数値化して返す。analyze_audio が曲全体の要約なのに対し、こちらは音色そのもの:\
        envelope(attack_ms=立ち上がり 10→90%、decay_ms、sustain_db=持続レベル、release_ms、decays_continuously=減衰し続けるか、\
        curve_db=音量の推移 20 点)/ pitch(f0・MIDI・ずれのセント・しゃくり glide_cents・ビブラートの速さと深さ・安定度。\
        音程の無い音では null)/ spectrum(centroid=明るさ、flatness=ノイズっぽさ、rolloff、flux=変化の激しさ、\
        centroid_start/mid/end と centroid_curve_hz=明るさの推移 → フィルタの開閉)/ harmonics(16 次までの倍音の振幅、\
        odd_even_db=奇数倍音の多さ、slope_db_per_octave=倍音の減り方、inharmonicity=金属っぽさ、hnr_db=倍音とノイズの比、\
        waveform_guess=sine/saw/square/triangle/noise/complex)/ labels(数値からの言葉の要約)/ \
        words(音と言葉を結びつける学習済みモデル CLAP で「聴いた」印象。instrument=何の音らしいか、tone=明るさ・太さ、\
        texture=質感、envelope=時間変化、movement=揺れ・動き、space=空間、mood=雰囲気。各 3 語、z は「その語としては\
        珍しく当てはまる度合い」で 2 以上ならかなり、1 未満なら弱い。モデル未取得なら null)。\
        対象は clip_id(音声クリップ)/ file(音声ファイルのパス)/ track_id(+ pitch / velocity / duration_ms。\
        そのトラックの音源とエフェクトで 1 音鳴らす)のどれか 1 つ。\
        使いどころ: 取り込んだサンプルがどんな音かを把握する、自分が作った音色と比べる(数値の差を見てつまみを直す)。"
    )]
    async fn analyze_sound(&self, params: Parameters<AnalyzeSoundParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("analyze_sound");
        let source = params.0.source.to_source()?;
        let (project, _) = self.handle.get_project().await?;
        let dir = self.handle.project_dir().await?;
        let v = tokio::task::spawn_blocking(move || -> Result<Value, String> {
            let sound = crate::sound::load(&project, std::path::Path::new(&dir), &source)?;
            let d = crate::sound::describe(&sound);
            let mut v = serde_json::to_value(&d).map_err(|e| e.to_string())?;
            v["source"] = json!(sound.label);
            if glaux_ml::clap::available() {
                let e = crate::sound::embedding(&sound)?;
                v["words"] = crate::sound::words_json(&e);
            } else {
                v["words"] = Value::Null;
                v["words_note"] = json!(crate::sound::clap_missing_note());
            }
            Ok(v)
        })
        .await
        .map_err(|e| e.to_string())??;
        Ok(JsonText(v))
    }

    #[tool(
        description = "2 つの音を比べる(A を基準に B がどう違うか)。distance(total / spectral=音色 / envelope=音量の時間変化。\
        0.15 未満 ほぼ同じ、0.35 未満 よく似ている、0.7 未満 似ている部分がある、それ以上 かなり違う)、verdict、\
        differences(明るさ・立ち上がり・減衰・長さ・ノイズっぽさ・倍音・音程・ビブラートの違いと、B を A に寄せる手がかり)、\
        clap_similarity(CLAP で聴いた印象の近さ -1〜1。モデル取得済みのときだけ)。\
        a / b はそれぞれ {clip_id} / {file} / {track_id, pitch, velocity, duration_ms} のどれか。\
        使いどころ: サンプルに似せて音作りするとき、a = 目標のサンプル、b = 自分のトラックの音 にして、\
        つまみを変えるたびに比べて distance が下がるか確かめる。"
    )]
    async fn compare_sounds(&self, params: Parameters<CompareSoundsParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("compare_sounds");
        let p = params.0;
        let (sa, sb) = (p.a.to_source()?, p.b.to_source()?);
        let (project, _) = self.handle.get_project().await?;
        let dir = self.handle.project_dir().await?;
        let v = tokio::task::spawn_blocking(move || -> Result<Value, String> {
            let dir = std::path::Path::new(&dir);
            let a = crate::sound::load(&project, dir, &sa)?;
            let b = crate::sound::load(&project, dir, &sb)?;
            crate::sound::compare(&a, &b)
        })
        .await
        .map_err(|e| e.to_string())??;
        Ok(JsonText(v))
    }

    #[tool(
        description = "目標の音(音声クリップ・音声ファイル)に似せて、MIDI トラックの内蔵シンセのつまみを自動で合わせる\
        (CMA-ES という進化的な探索で数百〜数千通り試す。既定 20 秒以内)。instrument: auto(既定)は subtractive(減算式:\
        波形・カットオフ・レゾナンス・ADSR・フィルターエンベロープ・ユニゾン等)と fm(FM: 周波数比・変調の深さとその減衰・\
        フィードバック・ADSR。エレピ・ベル・金属的な音)と wavetable(テーブル 5 種・position とその掃引・カットオフ・ADSR・\
        ユニゾン。母音のような音・シンクのギラつき・パルス)を探して最も近いものを採る。reverb: true でリバーブの量と広さも探し、\
        効きがあればトラックにリバーブを足す。結果は 1 回の履歴として残る(undo で戻せる)。\
        返り値: instrument(採った音源)、params(合わせたつまみ)、reverb(mix / size)、pitch(目標の音の高さ)、\
        distance(0.15 未満 ほぼ同じ … 0.7 以上 かなり違う)、initial_distance(探索前)、variants_tried(試した候補と距離)、\
        verified_distance(トラックのエフェクトも通して鳴らした音と目標の距離)。\
        内蔵シンセで作れない音(生楽器・サンプル特有の質感)は近づくが一致はしない。そのときは\
        compare_sounds の differences を見てエフェクトを足すか、CLAP プラグインで find_similar_presets → refine_plugin_params。\
        トラックの音源が内蔵の subtractive / fm / wavetable 以外なら replace_device: true が必要。"
    )]
    async fn match_sound(
        &self,
        params: Parameters<MatchSoundParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("match_sound");
        let p = params.0;
        let source = match (&p.clip_id, &p.file) {
            (Some(c), None) => crate::sound::SoundSource::Clip(
                glaux_core::ClipId::parse(c).map_err(|e| e.to_string())?,
            ),
            (None, Some(f)) => crate::sound::SoundSource::File(std::path::PathBuf::from(f)),
            _ => {
                return Err(
                    "目標の音は clip_id / file のどちらか 1 つを指定してください".to_owned(),
                )
            }
        };
        let track_id = glaux_core::TrackId::parse(&p.track_id).map_err(|e| e.to_string())?;
        let (project, _) = self.handle.get_project().await?;
        let track = project
            .track(&track_id)
            .ok_or_else(|| format!("トラックが見つかりません: {track_id}"))?;
        if track.kind != glaux_core::TrackKind::Midi {
            return Err(format!("「{}」は MIDI トラックではありません", track.name));
        }
        let is_synth = match &track.device {
            None => true,
            Some(d) => matches!(
                &d.source,
                glaux_core::PluginSource::Builtin { name }
                    if matches!(name.as_str(), "subtractive" | "fm" | "wavetable")
            ),
        };
        if !is_synth && !p.replace_device.unwrap_or(false) {
            return Err(format!(
                "「{}」の音源は内蔵の subtractive / fm / wavetable ではありません。置き換えてよければ replace_device: true を付けてください",
                track.name
            ));
        }
        let instrument = match p.instrument.as_deref().unwrap_or("auto") {
            "auto" => None,
            other => Some(
                glaux_engine::sound_match::FitInstrument::parse(other).ok_or_else(|| {
                    format!("instrument は auto / subtractive / fm / wavetable: {other}")
                })?,
            ),
        };
        let reverb = p.reverb.unwrap_or(false);
        let current = track.device.clone();
        let track_name = track.name.clone();
        let dir = self.handle.project_dir().await?;
        let max_seconds = p
            .max_seconds
            .unwrap_or(if instrument.is_none() { 30.0 } else { 20.0 });
        let (outcome, label) = tokio::task::spawn_blocking({
            let project = project.clone();
            let dir = dir.clone();
            let source = source.clone();
            move || -> Result<_, String> {
                let target = crate::sound::load(&project, std::path::Path::new(&dir), &source)?;
                let label = target.label.clone();
                Ok((
                    crate::sound::match_sound(&target, instrument, reverb, max_seconds),
                    label,
                ))
            }
        })
        .await
        .map_err(|e| e.to_string())??;
        let device = crate::sound::matched_device(&outcome, current.as_ref());
        let mut cmds = vec![glaux_core::Command::SetDevice {
            track: track_id.clone(),
            device: Some(device),
        }];
        if let Some(rv) = crate::sound::matched_reverb(&outcome) {
            cmds.push(glaux_core::Command::AddEffect {
                track: track_id.clone(),
                effect: rv,
                index: None,
            });
        }
        let edit_label = format!("{label}に似せて「{track_name}」の音色を自動調整");
        let command = if cmds.len() == 1 {
            cmds.remove(0)
        } else {
            glaux_core::Command::batch(edit_label.clone(), cmds)
        };
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, edit_label).await)?;
        // トラックのエフェクトも通して鳴らし、目標とどれだけ近いか確かめる
        let (project, _) = self.handle.get_project().await?;
        let (pitch, hold) = (outcome.pitch, outcome.hold);
        let verified = tokio::task::spawn_blocking(move || -> Result<f32, String> {
            let dirp = std::path::Path::new(&dir);
            let target = crate::sound::load(&project, dirp, &source)?;
            let mut mine =
                crate::sound::render_note(&project, dirp, &track_id, pitch, 100, hold as f64)?;
            // 目標と同じ長さで比べる(試し鳴らしは余韻の分だけ長い)
            let secs = target.frames.len() as f32 / target.sample_rate;
            mine.frames.truncate((secs * mine.sample_rate) as usize);
            Ok(glaux_engine::sound_match::compare(
                &target.frames,
                target.sample_rate,
                &mine.frames,
                mine.sample_rate,
            )
            .total)
        })
        .await
        .map_err(|e| e.to_string())?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["match"] = crate::sound::match_json(&outcome);
        v["target"] = json!(label);
        if let Ok(d) = verified {
            v["verified_distance"] = json!((d as f64 * 1000.0).round() / 1000.0);
        }
        Ok(JsonText(v))
    }

    #[tool(
        description = "目標の音(音声クリップ・音声ファイル)に近い CLAP プラグイン(例 Surge XT)のプリセットを探す。\
        プリセットを 1 音ずつ鳴らした索引(設定フォルダにキャッシュ。初回は数千個で数分かかるので index_seconds で区切って\
        作り足し、続きは次の呼び出しで)から、音色の要約と CLAP の印象の近さで候補を絞り、目標と同じ高さ・長さで鳴らし直して\
        距離で並べる。返り値: results(id・name・category・distance(0.15 未満 ほぼ同じ … 0.7 以上 かなり違う)・\
        clap_similarity)、index(total / indexed。indexed < total なら未索引のプリセットはまだ探していない)。\
        category で絞ると速い。気に入った候補は load_plugin_preset(id)で読み込み、list_params のつまみや\
        compare_sounds で詰める。内蔵シンセで作れる音なら match_sound の方が速い。"
    )]
    async fn find_similar_presets(
        &self,
        params: Parameters<FindSimilarPresetsParams>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("find_similar_presets");
        let p = params.0;
        let source = match (&p.clip_id, &p.file) {
            (Some(c), None) => crate::sound::SoundSource::Clip(
                glaux_core::ClipId::parse(c).map_err(|e| e.to_string())?,
            ),
            (None, Some(f)) => crate::sound::SoundSource::File(std::path::PathBuf::from(f)),
            _ => {
                return Err(
                    "目標の音は clip_id / file のどちらか 1 つを指定してください".to_owned(),
                )
            }
        };
        let track_id = glaux_core::TrackId::parse(&p.track_id).map_err(|e| e.to_string())?;
        let (project, _) = self.handle.get_project().await?;
        let dir = self.handle.project_dir().await?;
        let v = tokio::task::spawn_blocking(move || {
            crate::preset_index::similar_json(
                &project,
                std::path::Path::new(&dir),
                &source,
                &track_id,
                p.category.as_deref(),
                p.limit.unwrap_or(5),
                std::time::Duration::from_secs(p.index_seconds.unwrap_or(60).min(600)),
                &mut |_| {},
            )
        })
        .await
        .map_err(|e| e.to_string())??;
        Ok(JsonText(v))
    }

    #[tool(
        description = "CLAP 音源(例 Surge XT)のつまみを目標の音(音声クリップ・音声ファイル)に自動で合わせる(CMA-ES、既定 20 秒)。\
        今の音色(読み込んだプリセットと上書き値)から出発して、params(名前の部分一致)か自動で選んだ主要なつまみ\
        (フィルターのカットオフ・レゾナンス・エンベロープ量、アンプの ADSR 等)を動かし、変わったつまみを set_param の\
        まとめ 1 回として書く(undo で戻せる)。返り値: changed(つまみごとの before / after)、initial_distance → distance、\
        verdict。使い方: find_similar_presets で近いプリセットを探して load_plugin_preset → これで詰める。\
        LFO のテンポ同期・モジュレーションの割り当てなど、つまみとして公開されていない部分は変えられない。"
    )]
    async fn refine_plugin_params(
        &self,
        params: Parameters<RefinePluginParamsParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("refine_plugin_params");
        let p = params.0;
        let source = match (&p.clip_id, &p.file) {
            (Some(c), None) => crate::sound::SoundSource::Clip(
                glaux_core::ClipId::parse(c).map_err(|e| e.to_string())?,
            ),
            (None, Some(f)) => crate::sound::SoundSource::File(std::path::PathBuf::from(f)),
            _ => {
                return Err(
                    "目標の音は clip_id / file のどちらか 1 つを指定してください".to_owned(),
                )
            }
        };
        let track_id = glaux_core::TrackId::parse(&p.track_id).map_err(|e| e.to_string())?;
        let (project, _) = self.handle.get_project().await?;
        let dir = self.handle.project_dir().await?;
        let wanted = p.params.unwrap_or_default();
        let max_seconds = p.max_seconds.unwrap_or(20.0);
        let refined = tokio::task::spawn_blocking({
            let track_id = track_id.clone();
            move || -> Result<_, String> {
                let target = crate::sound::load(&project, std::path::Path::new(&dir), &source)?;
                crate::preset_index::refine_params(
                    &project,
                    &track_id,
                    &target,
                    &wanted,
                    max_seconds,
                )
            }
        })
        .await
        .map_err(|e| e.to_string())??;
        let mut v = refined.json;
        if refined.commands.is_empty() {
            v["note"] = json!("今の値のままが最も近かったため、つまみは変えませんでした");
            return Ok(JsonText(v));
        }
        let label = format!(
            "「{}」の CLAP のつまみを目標の音に合わせる({} 個)",
            v["track"].as_str().unwrap_or(""),
            refined.commands.len()
        );
        let author = self.author(&ctx);
        let command = glaux_core::Command::batch(label.clone(), refined.commands);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut out = mutated_json(&m);
        out["entry_id"] = json!(entry_id);
        out["refine"] = v;
        Ok(JsonText(out))
    }

    #[tool(
        description = "あなたの「拍を感じる耳」。音声(音声クリップ・音声ファイル)のビート・小節頭・テンポを\
        学習済みモデル(Beat This!)で推定する。返り値: bpm(平均テンポ、小数 2 桁)、bpm_alternatives(半分・倍。\
        ビートの取り方の解釈違い。曲調に合う方を選ぶ)、tempo_variation(0.015 未満 = 打ち込み・クリックに合わせた演奏)、\
        beats_per_bar(1 小節の拍数)、first_downbeat_sec(最初の小節頭。素材の先頭からの秒)、beats / downbeats(秒)、summary。\
        対象は clip_id か file のどちらか 1 つ。\
        使いどころ: 取り込んだ音声を曲のテンポに合わせる(set_clip_stretch の original_bpm に bpm を入れる。\
        曲のテンポを素材に合わせるなら set_tempo)、ループ素材の小節頭をクリップの先頭にそろえる\
        (first_downbeat_sec から offset を決める)、録音のテンポの揺れを確かめる。"
    )]
    async fn analyze_beats(&self, params: Parameters<AnalyzeBeatsParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("analyze_beats");
        let p = params.0;
        let source = match (&p.clip_id, &p.file) {
            (Some(c), None) => crate::sound::SoundSource::Clip(
                glaux_core::ClipId::parse(c).map_err(|e| e.to_string())?,
            ),
            (None, Some(f)) => crate::sound::SoundSource::File(std::path::PathBuf::from(f)),
            _ => return Err("clip_id / file のどちらか 1 つを指定してください".to_owned()),
        };
        let (project, _) = self.handle.get_project().await?;
        let dir = self.handle.project_dir().await?;
        let v = tokio::task::spawn_blocking(move || -> Result<Value, String> {
            let sound = crate::sound::load(&project, std::path::Path::new(&dir), &source)?;
            let r = crate::sound::beats(&sound)?;
            serde_json::to_value(&r).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())??;
        Ok(JsonText(v))
    }

    #[tool(
        description = "CLAP プラグイン(例 Surge XT。音源なら track_id、エフェクトなら fx_id)のプリセット(作り込まれた音色)を一覧する。\
        プリセットを公開していないプラグインもある(例 Surge XT Effects は 0 件)。そのときは人間にプラグインの画面の\
        プリセットメニューから選んでもらうか、list_params のつまみで作る。\
        filter(名前・カテゴリ・作者の部分一致、例 \"pad\" / \"bass\")や category(フォルダ名、例 \"Pads\")で絞り込む。\
        返り値の categories でどんな系統があるか分かる。current_preset は今読み込まれているプリセット名。\
        プリセットにはつまみとして公開されていない設定(LFO のテンポ同期、モジュレーションの割り当て、\
        内蔵エフェクト)も入っているので、ポンピングやゲートのような動きのある音は、まずそれらしい\
        プリセットを探して load_plugin_preset で読み込み、list_params のつまみで微調整するのが近道。"
    )]
    async fn list_plugin_presets(&self, params: Parameters<ListPluginPresetsParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("list_plugin_presets");
        let p = params.0;
        let track_id = plugin_owner(p.track_id.as_deref(), p.fx_id.as_deref())?;
        let (project, _) = self.handle.get_project().await?;
        let v = tokio::task::spawn_blocking(move || {
            crate::clap_presets::list(
                &project,
                &track_id,
                p.filter.as_deref(),
                p.category.as_deref(),
                p.limit.unwrap_or(50),
                p.rescan.unwrap_or(false),
            )
        })
        .await
        .map_err(|e| e.to_string())??;
        Ok(JsonText(v))
    }

    #[tool(
        description = "CLAP プラグイン(音源なら track_id、エフェクトなら fx_id)にプリセットを読み込む(音色が丸ごと入れ替わる。履歴 1 件、取り消し可)。\
        preset は list_plugin_presets の id。set_param で上書きしていたつまみの値は消える(プリセットの値になる)。\
        読み込み後は list_params で主なつまみ(cutoff・attack など)を確認し、必要なら微調整すること。"
    )]
    async fn load_plugin_preset(
        &self,
        params: Parameters<LoadPluginPresetParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("load_plugin_preset");
        let p = params.0;
        let track_id = plugin_owner(p.track_id.as_deref(), p.fx_id.as_deref())?;
        let (project, _) = self.handle.get_project().await?;
        let (command, label, name) = tokio::task::spawn_blocking(move || {
            crate::clap_presets::load_command(&project, &track_id, &p.preset)
        })
        .await
        .map_err(|e| e.to_string())??;
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["preset"] = json!(name);
        Ok(JsonText(v))
    }

    #[tool(
        description = "SoundFont ライブラリを一覧する。引数なしで .sf2 ファイル一覧、\
        file を指定するとそのフォントのプリセット一覧(bank / preset / 名前)。\
        ピアノ・ストリングス・ブラスなど本物っぽい楽器一式が欲しいときは、まずここを確認して\
        set_soundfont_instrument で設定する。ライブラリフォルダに .sf2 が無い場合は、\
        ユーザーに FluidR3_GM などのフリー SoundFont の導入を提案すること。"
    )]
    async fn list_soundfonts(&self, params: Parameters<ListSoundfontsParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("list_soundfonts");
        let dir = glaux_engine::sf2::default_dir();
        match params.0.file {
            None => Ok(JsonText(json!({
                "dir": dir.to_string_lossy(),
                "files": glaux_engine::sf2::list_files(&dir),
            }))),
            Some(file) => {
                let font = tokio::task::spawn_blocking({
                    let path = dir.join(&file);
                    move || glaux_engine::sf2::load_font(&path)
                })
                .await
                .map_err(|e| e.to_string())??;
                Ok(JsonText(json!({
                    "file": file,
                    "presets": glaux_engine::sf2::list_presets(&font),
                })))
            }
        }
    }

    #[tool(
        description = "トラックの音源を SoundFont のプリセットにする(sf2 マルチサンプラー)。\
        soundfont / bank / preset は list_soundfonts で確認したものを渡す。\
        GM 配列の目安: 0=ピアノ, 24=ギター(ナイロン), 25(スチール), 30(歪みギター), \
        32〜39=ベース, 40=バイオリン, 48=ストリングス, 56=トランペット, 73=フルート。\
        ドラムは bank 128。設定は undo で戻せる。"
    )]
    async fn set_soundfont_instrument(
        &self,
        params: Parameters<SetSoundfontParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("set_soundfont_instrument");
        let p = params.0;
        let track_id = glaux_core::TrackId::parse(&p.track_id).map_err(|e| e.to_string())?;
        let (project, _) = self.handle.get_project().await?;
        let track = project
            .track(&track_id)
            .ok_or_else(|| format!("track not found: {track_id}"))?;

        // 事前検証: フォントとプリセットの存在(音が出ない設定を防ぐ)
        let dir = glaux_engine::sf2::default_dir();
        let (bank, preset) = (p.bank, p.preset);
        let preset_name = tokio::task::spawn_blocking({
            let path = dir.join(&p.soundfont);
            move || -> Result<String, String> {
                let font = glaux_engine::sf2::load_font(&path)?;
                glaux_engine::sf2::list_presets(&font)
                    .into_iter()
                    .find(|m| m.bank == bank && m.preset == preset)
                    .map(|m| m.name)
                    .ok_or_else(|| format!("プリセットがありません: bank={bank} preset={preset}"))
            }
        })
        .await
        .map_err(|e| e.to_string())??;

        let label = format!("{} の音源を「{preset_name}」(SoundFont)に変更", track.name);
        let command = Command::SetDevice {
            track: track_id,
            device: Some(glaux_core::Device {
                source: glaux_core::PluginSource::Sf2 {
                    soundfont: p.soundfont,
                    bank: p.bank,
                    preset: p.preset,
                },
                params: glaux_core::ParamMap::new(),
            }),
        };
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["preset_name"] = json!(preset_name);
        Ok(JsonText(v))
    }

    #[tool(
        description = "WAV ファイルをプロジェクトに取り込み、トラックの音源を sampler にする。\
        サンプルは内容ハッシュ名で <プロジェクト>/audio/ にコピーされ、ノートは root からの\
        ピッチ変換で再生される(実録の質感が欲しいときに使う)。\
        音程のある素材は root にサンプルの実音を指定すること(例: A3 の単音ギターなら 57)。\
        取り込み + 音源設定は 1 Batch = 1 回の undo で戻せる。WAV 以外はエラー。"
    )]
    async fn import_sample(
        &self,
        params: Parameters<ImportSampleParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("import_sample");
        let p = params.0;
        let track_id = glaux_core::TrackId::parse(&p.track_id).map_err(|e| e.to_string())?;
        let (project, _) = self.handle.get_project().await?;
        let track = project
            .track(&track_id)
            .ok_or_else(|| format!("track not found: {track_id}"))?;

        let dir = self.handle.project_dir().await?;
        let imported =
            crate::assets::import_audio(std::path::Path::new(&dir), std::path::Path::new(&p.path))?;

        let mut params_map = glaux_core::ParamMap::new();
        if let Some(root) = p.root {
            params_map.insert(
                "root".to_owned(),
                glaux_core::ParamValue::Int(root.min(127) as i64),
            );
        }
        let mut cmds = Vec::new();
        if !project.assets.contains_key(&imported.id) {
            cmds.push(Command::AddAsset {
                id: imported.id.clone(),
                asset: imported.asset.clone(),
            });
        }
        cmds.push(Command::SetDevice {
            track: track_id.clone(),
            device: Some(glaux_core::Device {
                source: glaux_core::PluginSource::Sampler {
                    asset: imported.id.clone(),
                },
                params: params_map,
            }),
        });
        let file_name = std::path::Path::new(&p.path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "sample".to_owned());
        let label = format!("{} にサンプル「{file_name}」を設定", track.name);
        let command = Command::batch(label.clone(), cmds);
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["asset_id"] = json!(imported.id);
        v["sample_rate"] = json!(imported.asset.sample_rate);
        v["frames"] = json!(imported.asset.frames);
        Ok(JsonText(v))
    }

    #[tool(
        description = "音声ファイル(WAV / MP3 / FLAC / OGG / M4A)を音声クリップとして音声トラック(kind: \"audio\")に置く。\
        ボーカル・実録ギター・ループ素材など「そのまま鳴らす」音声はこれ(音程を付けて\
        鳴らしたいワンショットは import_sample でサンプラー音源にする)。\
        クリップ長は WAV の秒数をその位置のテンポで tick に換算。元の速度で再生される\
        (テンポ追従ストレッチは未対応)。ファイルはプロジェクトの audio/ にコピーされる。\
        音声トラックがなければ先に apply_commands の add_track(kind: \"audio\")で作る。"
    )]
    async fn import_audio_clip(
        &self,
        params: Parameters<ImportAudioClipParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("import_audio_clip");
        let p = params.0;
        let track_id = glaux_core::TrackId::parse(&p.track_id).map_err(|e| e.to_string())?;
        let (project, _) = self.handle.get_project().await?;
        let dir = self.handle.project_dir().await?;
        let imported =
            crate::assets::import_audio(std::path::Path::new(&dir), std::path::Path::new(&p.path))?;
        let file_name = std::path::Path::new(&p.path)
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "audio".to_owned());
        let name = p.name.unwrap_or(file_name);
        let clip_id = glaux_core::ClipId::new();
        let start = glaux_core::Tick(p.start_tick.unwrap_or(0));
        let cmds = crate::assets::audio_clip_commands(
            &project,
            &track_id,
            &imported,
            clip_id.clone(),
            start,
            &name,
        )?;
        let track_name = project
            .track(&track_id)
            .map(|t| t.name.clone())
            .unwrap_or_default();
        let label = format!("{track_name} に音声クリップ「{name}」を配置");
        let command = Command::batch(label.clone(), cmds);
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["clip_id"] = json!(clip_id);
        v["asset_id"] = json!(imported.id);
        v["seconds"] = json!(imported.asset.frames as f64 / imported.asset.sample_rate as f64);
        Ok(JsonText(v))
    }

    #[tool(
        description = "音声クリップを譜起こしして、同じ位置・長さの MIDI クリップを作る(履歴 1 件)。\
        mode: \"melody\"(既定)は鼻歌・歌・単音のギター等の**単旋律**向け、\
        mode: \"poly\" はピアノ・ギターの**和音**や伴奏入りの素材向け(学習済みモデル basic-pitch。\
        倍音を別の音と取り違えることがあるので、結果は analyze_harmony と照らして整える)。ドラムは対象外。\
        人間が ⏺ で鼻歌を録音したら、これで MIDI にしてから analyze_harmony でキーを確認し、\
        オクターブ誤検出(前後と 12 半音ずれた短い音)や外れた音を update_notes で整える、が定石。\
        結果には note_count と、新設した場合の track_id が入る。"
    )]
    async fn transcribe_audio(
        &self,
        params: Parameters<TranscribeAudioParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("transcribe_audio");
        let p = params.0;
        let clip_id = glaux_core::ClipId::parse(&p.clip_id).map_err(|e| e.to_string())?;
        let dest = match &p.dest_track_id {
            Some(id) => Some(glaux_core::TrackId::parse(id).map_err(|e| e.to_string())?),
            None => None,
        };
        let (project, _) = self.handle.get_project().await?;
        let dir = self.handle.project_dir().await?;
        let mut opts = glaux_engine::transcribe::TranscribeOptions::default();
        if let Some(ms) = p.min_note_ms {
            opts.min_note_ms = ms.clamp(20.0, 2000.0);
        }
        let t = crate::transcribe::transcribe_clip_commands(
            &project,
            std::path::Path::new(&dir),
            &clip_id,
            dest.as_ref(),
            p.quantize_ticks.unwrap_or(240),
            &opts,
            crate::transcribe::TranscribeMode::parse(p.mode.as_deref())?,
        )?;
        let label = format!("音声クリップを譜起こし({} ノート)", t.note_count);
        let command = Command::batch(label.clone(), t.commands);
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["clip_id"] = json!(t.clip_id);
        v["track_id"] = json!(t.track_id);
        v["created_track"] = json!(t.created_track);
        v["note_count"] = json!(t.note_count);
        Ok(JsonText(v))
    }

    #[tool(
        description = "音声クリップをパート(ステム)に分離し、パートごとの音声トラック(元トラックの直後)に \
        同じ位置・長さで置く。元のトラックはミュートする(履歴 1 件、取り消しで全部戻る)。\
        method: \"builtin\"(既定)= 打楽器 / 音程楽器 の 2 つ、\"demucs\" = ボーカル / ドラム / ベース / その他 \
        の 4 つ(外部ツール Demucs が必要。無ければエラーで案内が返る)。\
        使いどころ: 取り込んだ曲のベースだけ聴いて譜起こしする(分離 → transcribe_audio)、\
        ドラムだけ差し替える、ボーカルを抜いてオケにする、など。結果には作ったトラックの一覧が入る。"
    )]
    async fn separate_audio(
        &self,
        params: Parameters<SeparateAudioParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("separate_audio");
        let p = params.0;
        let clip_id = glaux_core::ClipId::parse(&p.clip_id).map_err(|e| e.to_string())?;
        let method = crate::stems::SeparateMethod::parse(p.method.as_deref())?;
        let (project, _) = self.handle.get_project().await?;
        let dir = self.handle.project_dir().await?;
        let s = tokio::task::spawn_blocking(move || {
            crate::stems::separate_clip_commands(
                &project,
                std::path::Path::new(&dir),
                &clip_id,
                method,
            )
        })
        .await
        .map_err(|e| e.to_string())??;
        let names: Vec<&str> = s.tracks.iter().map(|(n, _)| n.as_str()).collect();
        let label = format!("音声クリップをパートに分離({})", names.join(" / "));
        let command = Command::batch(label.clone(), s.commands);
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["tracks"] = json!(s
            .tracks
            .iter()
            .map(|(name, id)| json!({ "part": name, "track_id": id }))
            .collect::<Vec<_>>());
        Ok(JsonText(v))
    }

    // ---- 音色プリセット ----------------------------------------------------
    // 「音源 + エフェクトチェーン」をパッチとして設定ディレクトリに保存し、
    // 曲プロジェクトをまたいで再利用する。

    #[tool(
        description = "保存済みの音色プリセット一覧を返す(名前・説明・音源・エフェクト構成)。\
        プリセットは全プロジェクト共通のライブラリ。音作りを頼まれたら、まずここに\
        使える音がないか確認するとよい。"
    )]
    async fn list_presets(&self) -> ToolResult {
        let _activity = self.handle.begin_activity("list_presets");
        let (_, version) = self.handle.get_project().await?;
        Ok(JsonText(json!({
            "project_version": version,
            "presets": crate::presets::list(&crate::presets::default_dir()),
        })))
    }

    #[tool(
        description = "トラックの現在の音(音源のパラメータ + エフェクトチェーン)を\
        名前を付けてプリセット保存する。良い音ができたら保存しておくと、別の曲でも\
        load_preset で呼び出せる。description には用途と音の特徴を書くこと\
        (後で一覧から選ぶときの手掛かりになる)。"
    )]
    async fn save_preset(&self, params: Parameters<SavePresetParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("save_preset");
        let p = params.0;
        let track_id = glaux_core::TrackId::parse(&p.track_id).map_err(|e| e.to_string())?;
        let (project, version) = self.handle.get_project().await?;
        let track = project
            .track(&track_id)
            .ok_or_else(|| format!("track not found: {track_id}"))?;
        let preset = crate::presets::save(
            &crate::presets::default_dir(),
            track,
            &p.name,
            p.description,
            p.overwrite.unwrap_or(false),
        )?;
        Ok(JsonText(json!({
            "project_version": version,
            "saved": preset.name,
            "instrument": match &preset.device.source {
                glaux_core::PluginSource::Builtin { name } => name.clone(),
                other => format!("{other:?}"),
            },
            "effect_count": preset.effects.len(),
        })))
    }

    #[tool(
        description = "プリセットをトラックに適用する。音源を差し替え、既存のエフェクト\
        チェーンをプリセットの内容で置き換える(1 回の undo でまとめて戻せる)。\
        適用後に微調整するときは list_params で現在値を確認してから set_param。"
    )]
    async fn load_preset(
        &self,
        params: Parameters<LoadPresetParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("load_preset");
        let p = params.0;
        let track_id = glaux_core::TrackId::parse(&p.track_id).map_err(|e| e.to_string())?;
        let (project, _) = self.handle.get_project().await?;
        let track = project
            .track(&track_id)
            .ok_or_else(|| format!("track not found: {track_id}"))?;
        let preset = crate::presets::load(&crate::presets::default_dir(), &p.name)?;
        let label = format!("{} にプリセット「{}」を適用", track.name, preset.name);
        let cmds = crate::presets::apply_commands(track, &preset);
        let command = Command::batch(label.clone(), cmds);
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["applied"] = json!(preset.name);
        Ok(JsonText(v))
    }

    #[tool(
        description = "プリセットをライブラリから削除する(元に戻せない。undo の対象外)。\
        ユーザーに頼まれたときだけ使うこと。"
    )]
    async fn delete_preset(&self, params: Parameters<DeletePresetParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("delete_preset");
        let name = params.0.name;
        crate::presets::remove(&crate::presets::default_dir(), &name)?;
        Ok(JsonText(json!({ "deleted": name })))
    }

    // ---- エフェクトのプリセット ------------------------------------------
    // エフェクト 1 つ分(種類 + パラメータ + メモ)を曲をまたいで使い回す。

    #[tool(
        description = "保存済みのエフェクトのプリセット一覧を返す(名前・種類・メモ・保存元のトラック名)。\
        音色のプリセット(list_presets。音源 + エフェクト一式)とは別の、エフェクト 1 つ分のライブラリ。\
        エフェクトを足す前に、使える設定がないかここを確認するとよい。"
    )]
    async fn list_effect_presets(&self) -> ToolResult {
        let _activity = self.handle.begin_activity("list_effect_presets");
        Ok(JsonText(json!({
            "effect_presets": crate::fx_presets::list(&crate::fx_presets::default_dir()),
        })))
    }

    #[tool(
        description = "トラック(またはマスター)のエフェクト 1 つを、名前を付けてエフェクトのプリセットに保存する。\
        別のトラックや曲でも load_effect_preset で呼び出せる。note には音の特徴と用途を書くこと。"
    )]
    async fn save_effect_preset(&self, params: Parameters<SaveEffectPresetParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("save_effect_preset");
        let p = params.0;
        let target = crate::fx_presets::Target::parse(&p.target)?;
        let fx_id = glaux_core::FxId::parse(&p.fx_id).map_err(|e| e.to_string())?;
        let (project, version) = self.handle.get_project().await?;
        let (effect, owner) = crate::fx_presets::find_effect(&project, &target, &fx_id)?;
        let preset = crate::fx_presets::save(
            &crate::fx_presets::default_dir(),
            effect,
            &p.name,
            p.note,
            Some(owner),
            p.overwrite.unwrap_or(false),
        )?;
        Ok(JsonText(json!({
            "project_version": version,
            "saved": preset.name,
        })))
    }

    #[tool(
        description = "エフェクトのプリセットをトラック(またはマスター)に足す。表示名はプリセット名になる。\
        parked: true なら鳴らさずに「外してある」状態で置く。足した後に微調整するときは list_params で\
        現在値を確認してから set_param。"
    )]
    async fn load_effect_preset(
        &self,
        params: Parameters<LoadEffectPresetParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("load_effect_preset");
        let p = params.0;
        let target = crate::fx_presets::Target::parse(&p.target)?;
        let (project, _) = self.handle.get_project().await?;
        let preset = crate::fx_presets::load(&crate::fx_presets::default_dir(), &p.name)?;
        let (command, fx_id) = crate::fx_presets::add_command(
            &project,
            &target,
            &preset,
            p.index,
            p.parked.unwrap_or(false),
            None,
        )?;
        let label = format!("エフェクトのプリセット「{}」を追加", preset.name);
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["fx_id"] = json!(fx_id);
        Ok(JsonText(v))
    }

    #[tool(
        description = "エフェクトのプリセットをライブラリから削除する(元に戻せない。undo の対象外)。\
        ユーザーに頼まれたときだけ使うこと。"
    )]
    async fn delete_effect_preset(&self, params: Parameters<DeletePresetParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("delete_effect_preset");
        let name = params.0.name;
        crate::fx_presets::remove(&crate::fx_presets::default_dir(), &name)?;
        Ok(JsonText(json!({ "deleted": name })))
    }

    #[tool(
        description = "音作り・ジャンル・奏法・ミックス・音声素材・似た音作り・CLAP の定石を読む。\
        topic: instruments(音源の選び方・エレキギター・SoundFont)/ genres(EDM・メタル・Lo-fi・ループ・構成)/\
        expression(奏法・レガート・ポルタメント・ピッチカーブ)/ mix(エフェクト・バス・バランス・オートメーション)/\
        audio(音声素材・分離・譜起こし・テンポ追従)/ sound_match(似た音を作る)/ clap(プラグイン)。\
        省略で一覧。その分野の作業を始める前に読むと、道具の選び方と値の目安が分かる。"
    )]
    async fn get_guide(&self, params: Parameters<GetGuideParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("get_guide");
        match params.0.topic.as_deref() {
            None => Ok(JsonText(json!({
                "topics": crate::guide::TOPICS
                    .iter()
                    .map(|(name, title, _)| json!({ "topic": name, "title": title }))
                    .collect::<Vec<_>>(),
            }))),
            Some(t) => crate::guide::guide(t)
                .map(|text| JsonText(json!({ "topic": t, "guide": text })))
                .ok_or_else(|| {
                    let names: Vec<&str> =
                        crate::guide::TOPICS.iter().map(|(n, _, _)| *n).collect();
                    format!("topic は {} のいずれか(got: {t})", names.join(" / "))
                }),
        }
    }

    #[tool(
        description = "前回見た位置からの変更を要約する(get_project を読み直して自分で比べずに済む)。\
        since に最後に見た履歴エントリ ID(apply_commands の entry_id など)を渡す。author: \"human\" で人間の編集だけ。\
        返り値 clips にクリップごとのノートの追加・削除・変更の数と ID(多いときは最新 30 個)、tracks / effects に\
        操作の種類、global にテンポ・拍子・セクション・マスターの操作。詳しい中身は get_project(clip_ids)で読む。"
    )]
    async fn get_changes(&self, params: Parameters<GetChangesParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("get_changes");
        let p = params.0;
        if let Some(a) = p.author.as_deref() {
            if !matches!(a, "human" | "ai" | "system") {
                return Err(format!(
                    "author は human / ai / system のいずれか(got: {a})"
                ));
            }
        }
        let since = match p.since.as_deref() {
            Some(s) => Some(EntryId::parse(s).map_err(|e| e.to_string())?),
            None => None,
        };
        let limit = if since.is_some() { 1000 } else { 20 };
        let entries = flatten(self.handle.get_entries(since, limit).await)?;
        let entries: Vec<&glaux_core::HistoryEntry> = entries
            .iter()
            .filter(|e| match p.author.as_deref() {
                None => true,
                Some("human") => matches!(e.author, Author::Human),
                Some("ai") => matches!(e.author, Author::Ai { .. }),
                Some(_) => matches!(e.author, Author::System),
            })
            .collect();
        let (project, version) = self.handle.get_project().await?;
        let mut v = crate::changes::summarize(&entries, &project);
        v["project_version"] = json!(version);
        Ok(JsonText(v))
    }

    // ---- 構成の編集 ------------------------------------------------------
    // クリップ・ノートを 1 つずつ組み立てずに済むよう、サーバー側で絶対値のコマンド列を作る(Batch 1 件)。

    #[tool(
        description = "クリップを複製する(新しい ID。ノートの ID も振り直す)。1 回の undo で戻る。\
        to_tick(最初のクリップの置き場所)か offset_ticks(元からのずらし量)で位置を指定。両方省略すると、\
        選んだクリップの範囲の直後に続けて置く(「サビをもう 1 回」は、サビのクリップを全トラックぶん渡すだけ)。\
        返り値 clips に新しいクリップ ID。"
    )]
    async fn duplicate_clips(
        &self,
        params: Parameters<DuplicateClipsParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("duplicate_clips");
        let p = params.0;
        if p.clip_ids.is_empty() {
            return Err("clip_ids が空です".to_owned());
        }
        let ids = p
            .clip_ids
            .iter()
            .map(|s| glaux_core::ClipId::parse(s).map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let track = match &p.track_id {
            Some(t) => Some(glaux_core::TrackId::parse(t).map_err(|e| e.to_string())?),
            None => None,
        };
        let (project, _) = self.handle.get_project().await?;
        let clips: Vec<_> = project
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .filter(|c| ids.contains(&c.id))
            .collect();
        if clips.len() != ids.len() {
            return Err("見つからないクリップがあります(get_project で ID を確認)".to_owned());
        }
        let min_start = clips.iter().map(|c| c.start.0).min().unwrap_or(0);
        let max_end = clips.iter().map(|c| c.end().0).max().unwrap_or(0);
        let offset = match (p.to_tick, p.offset_ticks) {
            (Some(to), _) => to as i64 - min_start as i64,
            (None, Some(o)) => o,
            (None, None) => (max_end - min_start) as i64,
        };
        let made = glaux_core::arrange::duplicate_clips(&project, &ids, offset, track.as_ref())?;
        let new_ids: Vec<String> = made.iter().map(|(id, _)| id.to_string()).collect();
        let label = format!("クリップ {} 個を複製", made.len());
        let command = Command::batch(label.clone(), made.into_iter().map(|(_, c)| c).collect());
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["clips"] = json!(new_ids);
        Ok(JsonText(v))
    }

    #[tool(
        description = "小節を挿入する(bar 小節目の頭に count 小節の空白)。それより後ろのクリップ・テンポ・拍子・\
        セクション・オートメーションをまとめて後ろへずらし、またいでいるクリップは分割する。1 回の undo で戻る。\
        「間奏を 4 小節足して」はこれで空けてから中身を書く。小節は拍子の変化も考慮して数える。"
    )]
    async fn insert_bars(
        &self,
        params: Parameters<BarsParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("insert_bars");
        let p = params.0;
        let (project, _) = self.handle.get_project().await?;
        let (at, len) = glaux_core::arrange::bar_range(&project, p.bar, p.count)
            .ok_or("bar と count は 1 以上")?;
        let cmds = glaux_core::arrange::insert_time(&project, at, len);
        let label = format!("{} 小節目に {} 小節を挿入", p.bar, p.count);
        self.apply_arrangement(cmds, label, at, len, &ctx).await
    }

    #[tool(
        description = "小節を削除して後ろを詰める(bar 小節目から count 小節)。範囲内のクリップは消し、範囲にかかる\
        クリップは外側だけ残す。範囲内のテンポ・拍子・セクション・オートメーションの点も消し、後ろを前へずらす。\
        1 回の undo で戻る。"
    )]
    async fn delete_bars(
        &self,
        params: Parameters<BarsParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("delete_bars");
        let p = params.0;
        let (project, _) = self.handle.get_project().await?;
        let (from, len) = glaux_core::arrange::bar_range(&project, p.bar, p.count)
            .ok_or("bar と count は 1 以上")?;
        let cmds = glaux_core::arrange::delete_time(&project, from, len);
        let label = format!("{} 小節目から {} 小節を削除", p.bar, p.count);
        self.apply_arrangement(cmds, label, from, len, &ctx).await
    }

    #[tool(
        description = "トラックを音声にする(フリーズ)。自分のエフェクト・音量・パン・オートメーションと、センドしているバスの\
        響きまで込みで描き出し(マスターのエフェクトは通さない)、新しい音声トラックとして直後に置き、元のトラックはミュートする。\
        1 回の undo で戻る。CLAP プラグインの音源はゲーム(Godot)で鳴らないので、ゲームに使う曲はこれで音声にしておく。\
        重いトラックを軽くしたいときにも使う。時間がかかる(曲の長さの数分の 1)。"
    )]
    async fn bounce_track(
        &self,
        params: Parameters<BounceTrackParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("bounce_track");
        let track_id = glaux_core::TrackId::parse(&params.0.track_id).map_err(|e| e.to_string())?;
        let (project, _) = self.handle.get_project().await?;
        let dir = self.handle.project_dir().await?;
        let b = tokio::task::spawn_blocking(move || {
            let dir = std::path::Path::new(&dir);
            let bank = glaux_engine::SampleBank::for_offline(&project, dir);
            crate::bounce::bounce_track(&project, dir, &track_id, &bank)
        })
        .await
        .map_err(|e| e.to_string())??;
        let command = Command::batch(b.label.clone(), b.commands);
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, b.label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["new_track"] = json!(b.new_track);
        v["seconds"] = json!((b.seconds * 100.0).round() / 100.0);
        Ok(JsonText(v))
    }

    #[tool(
        description = "曲を WAV に書き出す。sample_rate(44100 / 48000)、bits(16 / 24 / 32 = 浮動小数)、範囲(start_tick / end_tick)、\
        loudness_lufs(音量の目標。配信なら -14。True Peak が -1 dBTP を超える所はリミッタで抑える)を選べる。stems: true でトラックごと(マスターを通さない)。\
        path 省略でプロジェクトの export/ に日時付きの名前。返り値に書いたファイルと、ラウドネス・サンプルのピーク・True Peak・PLR・\
        掛けたゲイン・limiter_db(リミッタで最も下げた量。1 dB 程度を超えるなら目標が大きすぎるか、ミックスのピークが強すぎる)・\
        streaming(配信サービスでの音量の調整の予測)。\
        書き出したらその値で音量を確かめて報告する。"
    )]
    async fn export_audio(&self, params: Parameters<crate::export::ExportRequest>) -> ToolResult {
        let _activity = self.handle.begin_activity("export_audio");
        let req = params.0;
        let (project, _) = self.handle.get_project().await?;
        let dir = self.handle.project_dir().await?;
        let v = tokio::task::spawn_blocking(move || {
            let dir = std::path::Path::new(&dir);
            let bank = glaux_engine::SampleBank::for_offline(&project, dir);
            crate::export::run(&project, dir, &req, &bank)
        })
        .await
        .map_err(|e| e.to_string())??;
        Ok(JsonText(v))
    }

    #[tool(
        description = "MIDI ファイル(.mid)を読み込み、パート(元のトラック × チャンネル)ごとに新しいトラックを末尾に足す(1 回の undo で戻る)。\
        ノート・最初の音色(GM)・音量・パン・マーカーを移す。10ch はドラム(内蔵 drum)、ほかは GM の分類で内蔵の楽器を選ぶ\
        (soundfont を渡すとその SoundFont の GM プリセット)。テンポと拍子は、set_tempo を省略するとプロジェクトにクリップが無いときだけ使う。\
        start_tick で置く位置をずらせる。読み込んだら get_project で中身と音源を確かめ、必要なら音色を整える。"
    )]
    async fn import_midi(
        &self,
        params: Parameters<crate::midi::ImportMidiRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("import_midi");
        let req = params.0;
        let (project, _) = self.handle.get_project().await?;
        let imp = tokio::task::spawn_blocking(move || crate::midi::import_file(&project, &req))
            .await
            .map_err(|e| e.to_string())??;
        let command = Command::batch(imp.label.clone(), imp.commands);
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, imp.label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["tempo_set"] = json!(imp.tempo_set);
        v["tracks"] = json!(imp
            .tracks
            .iter()
            .map(|(id, name, notes)| json!({ "track_id": id, "name": name, "notes": notes }))
            .collect::<Vec<_>>());
        Ok(JsonText(v))
    }

    #[tool(
        description = "曲を MIDI ファイル(SMF 1)に書き出す。MIDI トラックごとに 1 トラック(ドラムは 10ch)、テンポ・拍子・マーカー・音量・パンも入れる。\
        ループのクリップは展開する。ピッチカーブ・奏法・オートメーション・音色そのもの(GM の番号に近いものを選ぶだけ)は移らない。\
        path 省略でプロジェクトの export/ に日時付きの名前。"
    )]
    async fn export_midi(&self, params: Parameters<ExportMidiParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("export_midi");
        let (project, _) = self.handle.get_project().await?;
        let dir = self.handle.project_dir().await?;
        let v = crate::midi::export_file(
            &project,
            std::path::Path::new(&dir),
            params.0.path.as_deref(),
        )?;
        Ok(JsonText(v))
    }

    async fn apply_arrangement(
        &self,
        cmds: Vec<Command>,
        label: String,
        start: u64,
        len: u64,
        ctx: &RequestContext<RoleServer>,
    ) -> ToolResult {
        if cmds.is_empty() {
            return Err("変更がありません(その位置より後ろに何もない)".to_owned());
        }
        let command = Command::batch(label.clone(), cmds);
        let author = self.author(ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["start_tick"] = json!(start);
        v["length_ticks"] = json!(len);
        Ok(JsonText(v))
    }

    // ---- ノート便利ツール ------------------------------------------------
    // 「現在値を読んで絶対値に変換」をサーバー側で肩代わりする相対編集。
    // 中身はすべて UpdateNotes 1 コマンド = 1 回の undo で戻せる。

    #[tool(
        description = "クリップ内のノートを半音単位で移調する(相対編集の代行)。\
        semitones: +12 で 1 オクターブ上、-12 で下。note_ids 省略で全ノート。\
        音域(0..127)からはみ出す音は端に丸め、丸めた個数を clamped で返す\
        (clamped が付いたら意図どおりか確認を)。転調・オクターブ移動はこれを使い、\
        自分で update_notes を組み立てない。"
    )]
    async fn transpose_notes(
        &self,
        params: Parameters<TransposeNotesParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("transpose_notes");
        let p = params.0;
        let (clip, _len, notes, version) = self.load_notes(&p.clip_id, &p.note_ids).await?;
        if p.semitones == 0 {
            return Err("semitones が 0 です(変更なし)".to_owned());
        }
        let mut clamped = 0usize;
        let changes: Vec<_> = notes
            .iter()
            .map(|n| {
                let raw = n.pitch as i32 + p.semitones;
                let new = raw.clamp(0, 127) as u8;
                if raw != new as i32 {
                    clamped += 1;
                }
                (n, new)
            })
            .filter(|(n, new)| n.pitch != *new)
            .map(|(n, new)| glaux_core::NoteChange::new(n.id.clone()).pitch(new))
            .collect();
        let label = format!("{:+} 半音移調({} ノート)", p.semitones, changes.len());
        self.apply_note_changes(clip, changes, clamped, label, version, &ctx)
            .await
    }

    #[tool(
        description = "クリップ内のノートを時間方向に移動する(相対編集の代行)。\
        delta_ticks: 正で後ろ、負で前(960 = 4 分音符、3840 = 4/4 の 1 小節)。note_ids 省略で全ノート。\
        クリップ範囲(0..length-1)からはみ出す開始位置は端に丸め、丸めた個数を clamped で返す\
        (前に寄せすぎてタイミングが崩れていないか確認を)。\
        フレーズ全体を 1 拍ずらす・裏拍に移す、などはこれを使う。"
    )]
    async fn shift_notes(
        &self,
        params: Parameters<ShiftNotesParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("shift_notes");
        let p = params.0;
        let (clip, len, notes, version) = self.load_notes(&p.clip_id, &p.note_ids).await?;
        if p.delta_ticks == 0 {
            return Err("delta_ticks が 0 です(変更なし)".to_owned());
        }
        let max_pos = len.0.saturating_sub(1) as i64;
        let mut clamped = 0usize;
        let changes: Vec<_> = notes
            .iter()
            .map(|n| {
                let raw = n.pos.0 as i64 + p.delta_ticks;
                let new = raw.clamp(0, max_pos) as u64;
                if raw != new as i64 {
                    clamped += 1;
                }
                (n, new)
            })
            .filter(|(n, new)| n.pos.0 != *new)
            .map(|(n, new)| glaux_core::NoteChange::new(n.id.clone()).pos(glaux_core::Tick(new)))
            .collect();
        let label = format!("{:+} tick 移動({} ノート)", p.delta_ticks, changes.len());
        self.apply_note_changes(clip, changes, clamped, label, version, &ctx)
            .await
    }

    #[tool(
        description = "ノートにスウィング(ハネ・シャッフル)を掛ける。裏拍(grid_ticks 480 = 8 分裏、240 = 16 分裏)の音だけを\
        swing の位置へ寄せる(0.5 = ストレートに戻す、0.667 ≈ 3 連シャッフル、0.75 = 付点)。表の音と長さは変えない。\
        拍は曲頭から数える。同じ設定なら何度掛けても同じ結果。analyze_rhythm の swing_ratio(裏 8 分の位置 / 480)は\
        swing × 2 に相当する(swing_ratio 1.33 ≈ swing 0.667)。既存のノリに合わせるなら先に analyze_rhythm で測る。"
    )]
    async fn swing_notes(
        &self,
        params: Parameters<SwingNotesParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("swing_notes");
        let p = params.0;
        if !(0.5..=0.8).contains(&p.swing) {
            return Err(format!("swing は 0.5〜0.8(got: {})", p.swing));
        }
        let grid = p.grid_ticks.unwrap_or(480);
        if grid < 60 {
            return Err("grid_ticks は 60 以上にすること".to_owned());
        }
        let strength = p.strength.unwrap_or(1.0);
        if !(0.0..=1.0).contains(&strength) {
            return Err(format!("strength は 0.0〜1.0(got: {strength})"));
        }
        let (clip, len, notes, version) = self.load_notes(&p.clip_id, &p.note_ids).await?;
        let (project, _) = self.handle.get_project().await?;
        let start = project.clip(&clip).map(|(_, c)| c.start.0).unwrap_or(0);
        let changes: Vec<_> =
            glaux_core::rhythm::swing_positions(&notes, start, len.0, grid, p.swing, strength)
                .into_iter()
                .map(|(id, pos)| glaux_core::NoteChange::new(id).pos(glaux_core::Tick(pos)))
                .collect();
        let label = format!(
            "スウィング {:.0}%(1/{}、{} ノート)",
            p.swing * 100.0,
            3840 / grid,
            changes.len()
        );
        self.apply_note_changes(clip, changes, 0, label, version, &ctx)
            .await
    }

    #[tool(description = "ノートの開始位置をグリッドに寄せる(クオンタイズ)。\
        grid_ticks: 240 = 1/16、480 = 1/8。strength 1.0 で完全一致、0.5〜0.8 で人間味を残す。\
        note_ids 省略で全ノート。長さ(dur)は変えない。\
        人間の打ち込みのヨレを直すときは、先に get_project でリズムの意図(シャッフル等)がないか確認してから。")]
    async fn quantize_notes(
        &self,
        params: Parameters<QuantizeNotesParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("quantize_notes");
        let p = params.0;
        if p.grid_ticks == 0 {
            return Err("grid_ticks は 1 以上にすること".to_owned());
        }
        let strength = p.strength.unwrap_or(1.0);
        if !(0.0..=1.0).contains(&strength) {
            return Err(format!("strength は 0.0〜1.0(got: {strength})"));
        }
        let (clip, len, notes, version) = self.load_notes(&p.clip_id, &p.note_ids).await?;
        let max_pos = len.0.saturating_sub(1);
        let changes: Vec<_> = notes
            .iter()
            .map(|n| {
                let pos = n.pos.0;
                let target = ((pos as f64 / p.grid_ticks as f64).round() as u64) * p.grid_ticks;
                let new = (pos as f64 + (target as f64 - pos as f64) * strength).round() as u64;
                (n, new.min(max_pos))
            })
            .filter(|(n, new)| n.pos.0 != *new)
            .map(|(n, new)| glaux_core::NoteChange::new(n.id.clone()).pos(glaux_core::Tick(new)))
            .collect();
        let label = format!(
            "クオンタイズ 1/{}({} ノート)",
            3840 / p.grid_ticks.max(1),
            changes.len()
        );
        self.apply_note_changes(clip, changes, 0, label, version, &ctx)
            .await
    }

    #[tool(
        description = "ノートのベロシティをまとめて変える。vel = round(vel * factor + offset) を 1..127 に丸める。\
        例: factor 0.8 で全体を弱く、offset +15 で底上げ、factor 0.5 + offset 40 でダイナミクスを圧縮。\
        note_ids 省略で全ノート。端に丸めた個数を clamped で返す。\
        「このフレーズを弱く」「ゴーストノートを作る」などに使う。"
    )]
    async fn scale_velocity(
        &self,
        params: Parameters<ScaleVelocityParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("scale_velocity");
        let p = params.0;
        if p.factor.is_none() && p.offset.is_none() {
            return Err("factor と offset のどちらかは指定すること".to_owned());
        }
        let factor = p.factor.unwrap_or(1.0);
        let offset = p.offset.unwrap_or(0.0);
        if !(0.0..=16.0).contains(&factor) {
            return Err(format!("factor は 0.0〜16.0(got: {factor})"));
        }
        let (clip, _len, notes, version) = self.load_notes(&p.clip_id, &p.note_ids).await?;
        let mut clamped = 0usize;
        let changes: Vec<_> = notes
            .iter()
            .map(|n| {
                let raw = (n.vel as f64 * factor + offset).round();
                let new = raw.clamp(1.0, 127.0) as u8;
                if raw != new as f64 {
                    clamped += 1;
                }
                (n, new)
            })
            .filter(|(n, new)| n.vel != *new)
            .map(|(n, new)| glaux_core::NoteChange::new(n.id.clone()).vel(new))
            .collect();
        let label = format!(
            "ベロシティ調整 ×{factor}{}({} ノート)",
            if offset != 0.0 {
                format!(" {offset:+}")
            } else {
                String::new()
            },
            changes.len()
        );
        self.apply_note_changes(clip, changes, clamped, label, version, &ctx)
            .await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for GlauxServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(crate::guide::CORE)
    }
}
