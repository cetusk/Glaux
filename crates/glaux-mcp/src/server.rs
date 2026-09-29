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
    /// 奏法・ピッチカーブ・glide_ms・ビブラート・音量と明るさの曲線があるノートだけ 6 番目にそれらをまとめたオブジェクトが付く。
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
pub struct ImportIrParams {
    /// インパルス応答(IR)の音声ファイルの絶対パス(WAV / MP3 / FLAC など。部屋・ホール・機材の響きを録ったもの)
    pub path: String,
    /// 畳み込みリバーブを挿すトラックの ID。省略でマスター
    #[serde(default)]
    pub track_id: Option<String>,
    /// 響きの割合(0〜1、既定 0.3。センド用のバスなら 1)
    #[serde(default)]
    pub mix: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct RefineByWordsParams {
    /// 追い込むトラックの ID
    pub track_id: String,
    /// 近づけたい音色語(英語か日本語。例 ["warm", "soft"] / ["暖かい"])。辞書の語だけ使える
    #[serde(default)]
    pub toward: Vec<String>,
    /// 遠ざけたい音色語(例 ["harsh", "thin"])
    #[serde(default)]
    pub away: Vec<String>,
    /// 動かすつまみを絞る(`<fx_id>/<name>` か `<name>`。例 ["high_gain_db", "lp_freq"])。省略でトラックの内蔵エフェクトのつまみ全部(最大 10)
    #[serde(default)]
    pub params: Vec<String>,
    /// 評価の回数(1 回 1 秒ほど。既定 30、最大 120)
    #[serde(default)]
    pub max_evals: Option<usize>,
    /// false なら案を返すだけ。既定は true(1 回の undo で戻せる)
    #[serde(default)]
    pub apply: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct MasterMixParams {
    /// 参照曲の音声ファイルの絶対パス(WAV / MP3 / FLAC / OGG / M4A)。あれば、その音色の釣り合い・広がり・音量に寄せる
    #[serde(default)]
    pub reference_file: Option<String>,
    /// 目標の音量: "spotify"(-14 LUFS)/ "youtube"(-14)/ "apple"(-16)/ "loud"(-9、クラブ・EDM 向け)/
    /// "reference"(参照曲と同じ。参照曲があるときの既定)。参照曲が無いときの既定は "spotify"
    #[serde(default)]
    pub target: Option<String>,
    /// 目標の統合ラウドネス(LUFS)を数値で(target より優先)
    #[serde(default)]
    pub target_lufs: Option<f64>,
    /// false なら案を返すだけ(プロジェクトは変えない)。既定は true(マスターに足す。1 回の undo で戻せる)
    #[serde(default)]
    pub apply: Option<bool>,
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
pub struct ShapeAutomationParams {
    /// トラック ID。省略でマスター(target は track/volume_db か fx/<マスターのエフェクト ID>/<名前>)。
    #[serde(default)]
    pub track_id: Option<String>,
    /// 動かすもの: "track/volume_db"・"track/pan"・"device/<つまみ>"(例 device/cutoff)・"fx/<エフェクト ID>/<つまみ>"。
    pub target: String,
    /// 始まり。"小節" か "小節:拍"(1 始まり。例 "9"・"16:3"・"12:2.5")。
    pub start: String,
    /// 終わり("小節:拍"。その位置の頭まで)。bars と どちらか。
    #[serde(default)]
    pub end: Option<String>,
    /// 長さ(小節)。end と どちらか。
    #[serde(default)]
    pub bars: Option<f64>,
    /// 形: linear / exp(初めゆっくり・終わりで急に。ビルドアップ)/ log(初め急に。フェードアウト)/ s_curve /
    /// swell(to へ膨らんで from へ戻る)/ dip(to へ沈んで戻る)/ step(区間の頭で to に切り替えて保つ)/
    /// 周期で揺らす: sine / triangle / saw_up / saw_down(= pump。周期の頭で to に沈み from へ戻る)/ square。
    pub shape: String,
    /// 始まりの値(つまみの単位。track/volume_db なら dB)。周期で揺らす形では揺れの片側。
    pub from: f64,
    /// 終わりの値。swell / dip では山(谷)の値、周期で揺らす形では揺れのもう片側。
    pub to: f64,
    /// 周期で揺らす形の周期(拍)。1 = 4 分ごと(ポンピング)、0.5 = 8 分、4 = 1 小節。既定 1。
    #[serde(default)]
    pub period_beats: Option<f64>,
    /// false で区間の外の点も消してレーン全体をこの区間の点だけにする。既定 true(区間の外は残す)。
    #[serde(default)]
    pub keep_outside: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ApplyGrooveParams {
    /// 対象クリップ ID(`clp_xxxxxx`)。1 つなら clip_id、まとめて当てるなら clip_ids(1 回の undo で戻る)。
    #[serde(default)]
    pub clip_id: Option<String>,
    /// 対象クリップ ID の配列(ドラム・ベース・コードなど、トラックをまたいでよい)。
    #[serde(default)]
    pub clip_ids: Option<Vec<String>>,
    /// true でループのクリップの繰り返しを書き出してから当てる(繰り返しごとに違う揺れになる)。
    /// 既定 false(ループの中身に当てるので、揺れも毎回同じ)。生演奏らしさが要るジャンルで humanize_ms と一緒に。
    #[serde(default)]
    pub unroll_loop: Option<bool>,
    /// 型: funk / hiphop / soul / rock / pop / jazz / latin / neworleans / afrobeat(人間のドラマーの演奏から集計)、
    /// house / techno / trap(電子音楽の手作り。タイミングはほぼ格子どおりで強弱の型)。
    pub style: String,
    /// 先に 16 分の格子へ寄せる量 0〜1(既定 0 = 今の位置にずれを足す。スウィング済みのノートにも使える)。
    #[serde(default)]
    pub quantize: Option<f64>,
    /// 型のずれの効き 0〜1.5(既定 1.0。誇張は評価が下がるので 1 を超えるのは控えめに)。
    #[serde(default)]
    pub timing: Option<f64>,
    /// 型の強弱の効き 0〜1(既定 0.7)。拍の位置ごとのアクセント(表が強く、16 分の裏が弱い、など)。
    #[serde(default)]
    pub velocity: Option<f64>,
    /// 小さな揺れ(1/f の相関がある揺れ)の標準偏差(ms。既定 0、目安 3〜8、上限 20)。小節の頭は揺らさない。
    #[serde(default)]
    pub humanize_ms: Option<f64>,
    /// 楽器ごとの前ノリ(負)・後ノリ(正)(ms、-30〜30)。例 {"snare": 8, "hat": -4}(レイドバック)。
    #[serde(default)]
    pub pocket_ms: Option<std::collections::HashMap<String, f64>>,
    /// ドラム以外のトラックで使う型の楽器: kick(ベースをキックに合わせる)/ snare / hat(コードの刻み・アルペジオ)/ ride。
    /// 省略時、ドラムのトラックは音程で楽器を分け、それ以外は hat。
    #[serde(default)]
    pub as_part: Option<String>,
    /// 揺れの乱数の種(既定 1。同じ値なら同じ結果)。
    #[serde(default)]
    pub seed: Option<u64>,
    /// 対象ノート ID の配列(clip_id を 1 つ指定したときだけ)。省略でクリップ内の全ノート。
    #[serde(default)]
    pub note_ids: Option<Vec<String>>,
}

#[derive(Deserialize, JsonSchema)]
pub struct AddGhostNotesParams {
    /// 対象クリップ ID(ドラムのクリップ)。
    pub clip_id: String,
    /// どの型のゴーストの置き方に倣うか(既定 funk。ゴーストが多い順に neworleans / latin / funk / jazz / afrobeat / soul / rock / hiphop / pop)。
    #[serde(default)]
    pub style: Option<String>,
    /// 型の出現率に掛ける量 0〜2(既定 0.5。1 で型どおり)。
    #[serde(default)]
    pub density: Option<f64>,
    /// 音程(既定 38 = スネア)。
    #[serde(default)]
    pub pitch: Option<u8>,
    /// 強さ(1〜60。省略で型の平均。±4 ほど揺らす)。
    #[serde(default)]
    pub velocity: Option<u8>,
    /// 乱数の種(既定 1)。
    #[serde(default)]
    pub seed: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct WriteChordsParams {
    /// 置く MIDI トラックの ID(`trk_xxxxxx`)。新しいクリップを作って置く。
    pub track_id: String,
    /// コード進行。`|` で小節を区切り、小節の中は空白で区切って均等に分ける。`%` は前の小節をもう一度、N.C. は休み。
    /// 記号(Am7 / Fmaj7 / G7(b9) / Bbmaj9 / C/E / F#m7b5)か、key を指定すればローマ数字(vi7 / IVmaj7 / V7/V / bVII)。
    /// 例: "Am7 | Fmaj7 | C G/B | %"
    pub chords: String,
    /// 始まりの小節(既定 1)。
    #[serde(default)]
    pub bar: Option<u32>,
    /// 進行を何回繰り返すか(既定 1)。
    #[serde(default)]
    pub repeat: Option<u32>,
    /// ローマ数字を読むキー("C major" / "A minor" / "F#m")。
    #[serde(default)]
    pub key: Option<String>,
    /// 積み方: close(密集)/ open(開離)/ drop2(ピアノ・ギターの定番)/ drop3 / spread(広い。パッド・ストリングス)/
    /// shell(3 度と 7 度だけ。低音は根音)/ rootless(根音を省く。ベースが別にあるジャズ)。既定 close。
    #[serde(default)]
    pub style: Option<String>,
    /// 上の声部の数 2〜6(既定 4。shell は 2)。
    #[serde(default)]
    pub voices: Option<u8>,
    /// 上の声部の音域: "low"(A2〜A4)/ "mid"(E3〜E5、既定)/ "high"(C4〜C6)か "E3-C5" の形。
    #[serde(default)]
    pub range: Option<String>,
    /// 一番上の声部を寄せる高さ("E5" など)。旋律の線をそろえたいとき。
    #[serde(default)]
    pub top: Option<String>,
    /// true で低音(根音か分数コードの最低音)を C2〜E3 に足す(ピアノの左手)。ベースのトラックが別にあるなら false(既定)。
    #[serde(default)]
    pub bass: Option<bool>,
    /// リズム: sustain(和音ごとに伸ばす。既定)/ whole / half / quarter / eighth / offbeat(ハウスのスタブ)/
    /// charleston / backbeat / syncopated か、1 小節を等分した文字列(x = 打つ、- = 伸ばす、. = 休み。例 "x..x..x...x..x..")。
    #[serde(default)]
    pub rhythm: Option<String>,
    /// 伸ばさない音の長さの割合 0.1〜1(既定 0.9。スタブを短く切るなら 0.3〜0.5)。
    #[serde(default)]
    pub gate: Option<f64>,
    /// 強さ 1〜127(既定 88。拍の頭は少し強くなる)。
    #[serde(default)]
    pub velocity: Option<u8>,
    /// 下の音から順に遅らせる(ms。ギターのストローク 10〜25、既定 0)。
    #[serde(default)]
    pub strum_ms: Option<f64>,
    /// クリップの名前(既定 "Chords")。
    #[serde(default)]
    pub name: Option<String>,
    /// 和音の音すべてに付ける奏法: palm_mute(ブリッジミュートの刻み)/ staccato(歯切れよく。カッティング)/ accent /
    /// legato / portamento / vibrato / bend。省略で通常。SoundFont のギター・ベースの奏法の音色もこれで切り替わる。
    #[serde(default)]
    pub articulation: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct WriteBasslineParams {
    /// 置く MIDI トラックの ID(`trk_xxxxxx`)。新しいクリップを作って置く。
    pub track_id: String,
    /// コード進行(write_chords と同じ書き方。伴奏と同じ文字列を渡すと合う)。例 "Am7 | Fmaj7 | C G/B | %"
    pub chords: String,
    /// 始まりの小節(既定 1)。
    #[serde(default)]
    pub bar: Option<u32>,
    /// 進行を何回繰り返すか(既定 1)。
    #[serde(default)]
    pub repeat: Option<u32>,
    /// ローマ数字を読むキー。音階に沿う経過音(approach: "scale")にも使う。
    #[serde(default)]
    pub key: Option<String>,
    /// 型: root(伸ばし)/ root8(根音の 8 分。既定)/ offbeat(裏拍)/ octave(ディスコ)/ tresillo(3-3-2)/
    /// 808(トラップ。伸ばして滑らせる)/ funk / walking(ジャズ・ローファイ)か、1 小節を等分した文字列
    /// (x = 根音、o = 1 オクターブ上、5 / 3 / 7 = その度数、- = 伸ばす、. = 休み。例 "x..o..x.5.x..7..")。
    #[serde(default)]
    pub pattern: Option<String>,
    /// 音域(既定 "E1-E3"。808 は "C1-C3" くらいまで下げてよい)。
    #[serde(default)]
    pub range: Option<String>,
    /// 次の和音へ近づく経過音: none(既定)/ chromatic(半音)/ scale(音階。key が要る)。walking は既定で chromatic。
    #[serde(default)]
    pub approach: Option<String>,
    /// キックのクリップ ID。渡すとキックと同じ位置で根音を鳴らす(ベースとキックをかみ合わせる。pattern より優先)。
    #[serde(default)]
    pub follow_kick: Option<String>,
    /// 808 の滑る時間(ms、既定 60)。
    #[serde(default)]
    pub glide_ms: Option<f32>,
    /// 伸ばさない音の長さの割合 0.1〜1(既定 0.85)。
    #[serde(default)]
    pub gate: Option<f64>,
    /// 強さ(既定 96)。
    #[serde(default)]
    pub velocity: Option<u8>,
    /// クリップの名前(既定 "Bass")。
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct WriteDrumsParams {
    /// 置くドラムのトラックの ID(内蔵 drum か SoundFont のドラム)。新しいクリップを作って置く。
    pub track_id: String,
    /// 小節数(1〜512)。
    pub bars: u32,
    /// 始まりの小節(既定 1)。
    #[serde(default)]
    pub bar: Option<u32>,
    /// 型: house / techno / trap / hiphop / lofi / funk / rock / pop / dnb / disco / reggaeton / halftime / 2step。
    pub style: String,
    /// 音の多さ 0〜1(既定 0.6)。ハットの細かさ(4 分 → 8 分 → 16 分)とオープンハット。静かな区間は 0.3、山は 0.8〜1。
    #[serde(default)]
    pub intensity: Option<f64>,
    /// 小節ごとの変化 0〜1(既定 0.3)。キックの型を入れ替える割合と、裏のオープンハット。
    #[serde(default)]
    pub variation: Option<f64>,
    /// フィル: snare(既定)/ toms / none。fill_every 小節ごとの最後の小節の 3・4 拍目に入れる。
    #[serde(default)]
    pub fill: Option<String>,
    /// 何小節ごとにフィルを入れるか(既定 8。0 で入れない)。
    #[serde(default)]
    pub fill_every: Option<u32>,
    /// 最初の小節の頭と、フィルの次の小節の頭にクラッシュ(既定 true)。
    #[serde(default)]
    pub crash: Option<bool>,
    /// 最後の何小節をスネアのロールのビルド(4 分 → 8 分 → 16 分 → 32 分でだんだん強く)にするか(既定 0)。
    #[serde(default)]
    pub build_bars: Option<u32>,
    /// 最後の何拍を無音にするか(ドロップ・サビの直前の無音。既定 0)。
    #[serde(default)]
    pub gap_beats: Option<u32>,
    /// 乱数の種(既定 1。同じ値なら同じ結果)。
    #[serde(default)]
    pub seed: Option<u64>,
    /// 置く楽器(省略で全部): kick / snare(スネア・クラップ・リム)/ hat(クローズ・ペダル・オープン)/ tom /
    /// cymbal(クラッシュ・ライド)/ perc(それ以外)。ドラムを楽器ごとの別トラックにした曲では、トラックごとに
    /// parts を変えて同じ style・seed で呼ぶ(型がそろう)。
    #[serde(default)]
    pub parts: Option<Vec<String>>,
    /// 4/4 以外の小節の組み立て方: auto(既定。7/8・6/8 など分母 8 以上は group、3/4・5/4 など分母 4 以下は cut)/
    /// group(拍のまとまりの頭にキックとスネアを交互、ハットはまとまりごとに刻み直す)/
    /// cut(4/4 の型を 16 分の格子で切るか延ばす。7/8 = 4/4 から最後の 8 分を抜く)/ stretch(1 小節を 16 等分。以前の動き)。
    #[serde(default)]
    pub odd_meter: Option<String>,
    /// クリップの名前(既定 "Drums")。
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct WriteTransitionParams {
    /// つなぐ先: 区間の名前("ドロップ1" など。set_song_plan の名前)か小節番号("25")。その小節の頭が区切り。
    pub to: String,
    /// 区切りの直前の何拍を全トラック無音にするか(ドロップ・サビの前の「溜め」。0.5〜4。既定 0)。
    /// その範囲で始まる音は消し、かかる音は手前で切る(ループのクリップは書き出してから)。
    #[serde(default)]
    pub gap_beats: Option<f64>,
    /// 無音にしないトラックの名前(ライザー・FX など)。
    #[serde(default)]
    pub keep_tracks: Option<Vec<String>>,
    /// ドラムのトラック ID。渡すと、区切りに向けたリバースクラッシュ・スネアのロール・区切りのクラッシュを置く。
    #[serde(default)]
    pub drum_track_id: Option<String>,
    /// リバースクラッシュ(55)を、区切り(無音があればその手前)でちょうど鳴り終わるように置く(既定 true)。
    #[serde(default)]
    pub reverse_crash: Option<bool>,
    /// 区切りの頭にクラッシュとキック(既定 true)。
    #[serde(default)]
    pub crash: Option<bool>,
    /// 区切りの前の何小節をスネアのロールにするか(既定 0。1〜8)。
    #[serde(default)]
    pub roll_bars: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SuggestProgressionParams {
    /// ジャンル(jpop / pop / rock / edm / trance / house / futurebass / trap / hiphop / lofi / jazz / citypop / neosoul /
    /// funk / disco / blues / ballad / cinematic / latin / metal など)。
    #[serde(default)]
    pub genre: Option<String>,
    /// 雰囲気(bright / emotional / sad / dark / chill / epic / dramatic / groovy / dreamy / tense / nostalgic など)。
    #[serde(default)]
    pub mood: Option<String>,
    /// キー("C major" / "A minor" / "F#m")。進行の長短と違えば平行調に読み替える。既定 C major / A minor。
    #[serde(default)]
    pub key: Option<String>,
    /// 候補の数(既定 5)。
    #[serde(default)]
    pub count: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct CritiqueMelodyParams {
    /// 旋律のトラック ID(トラックのすべてのクリップ)。clip_id でクリップ 1 つだけでもよい。
    #[serde(default)]
    pub track_id: Option<String>,
    #[serde(default)]
    pub clip_id: Option<String>,
    /// しきい値の組: pop(既定。jpop・rock・ballad も)/ edm(house・trance)/ trap(hiphop)/ lofi / jazz / funk(disco)。
    #[serde(default)]
    pub genre: Option<String>,
    /// キー("C major" など)。省略で旋律から推定。
    #[serde(default)]
    pub key: Option<String>,
    /// 下の和音の進行(write_chords と同じ書き方)。省略でほかのトラックから小節ごとに推定する。
    #[serde(default)]
    pub chords: Option<String>,
    /// chords の始まりの小節(既定 1)。
    #[serde(default)]
    pub bar: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
pub struct DevelopMotifParams {
    /// 置く MIDI トラックの ID。新しいクリップを作る。
    pub track_id: String,
    /// 動機(1〜2 小節)。"音名:長さ" を空白で並べる。長さ: w 全 / h 2 分 / q 4 分 / e 8 分 / s 16 分 / t 32 分、
    /// 後ろの . で付点、3 で 3 連。r は休み。例 "E5:q D5:e C5:e E5:q G5:q | G5:w"(| は読み飛ばす)。
    /// または motif_clip_id で既存のクリップの音を動機にする。
    #[serde(default)]
    pub motif: Option<String>,
    #[serde(default)]
    pub motif_clip_id: Option<String>,
    /// 形式: sentence(既定。提示 → 反復 → 断片化 → 終止。サビ・フック)/ period(問い → 答え。A メロ)/ aaba / aab /
    /// call_response / loop(EDM・トラップ。繰り返して最後だけ変える)。または動機の長さごとの操作の並び
    /// "a | adapt vary | frag seq(-1) | adapt cadence"(音: a / adapt / seq(n) / frag / invert / retro / tail / cadence / fill、
    /// リズム: vary(割る・付点・まとめる・休む)/ augment(前半をゆっくり)/ diminish(速く 2 回)/ displace(n)(8 分 n 個ずらす))。
    /// 型にはリズムの変化も入っている(繰り返しの 2 回目以降は vary)。
    #[serde(default)]
    pub form: Option<String>,
    /// 下の和音の進行(write_chords と同じ書き方。足りなければ繰り返す)。省略でほかのトラックから推定。
    #[serde(default)]
    pub chords: Option<String>,
    /// キー。省略で進行か動機から推定。
    #[serde(default)]
    pub key: Option<String>,
    /// 始まりの小節(既定 1)。
    #[serde(default)]
    pub bar: Option<u32>,
    /// 最高音を置く小節(曲の小節番号)。省略で全体の 60〜75% の位置。
    #[serde(default)]
    pub peak_bar: Option<u32>,
    /// 最高音の高さ("A5" など)。省略で展開したままの高さ。
    #[serde(default)]
    pub peak: Option<String>,
    /// 強拍の音を 8 分前へ食わせる割合 0〜1(既定 0.2。ロックの歌の実測 約 0.23 に合わせた値。EDM は 0.3〜0.5)。
    #[serde(default)]
    pub anticipate: Option<f64>,
    /// 音域(既定は動機の中心から ±10 半音くらい。例 "C4-G5")。
    #[serde(default)]
    pub range: Option<String>,
    /// 点検のしきい値の組(critique_melody の genre)。
    #[serde(default)]
    pub genre: Option<String>,
    /// 強さ(既定 92。強拍は少し強く)。
    #[serde(default)]
    pub velocity: Option<u8>,
    /// 乱数の種(食わせる音の選び方。既定 1)。
    #[serde(default)]
    pub seed: Option<u64>,
    /// クリップの名前(既定 "Melody")。
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct WriteMelodyParams {
    /// 置く MIDI トラックの ID。新しいクリップを作る。
    pub track_id: String,
    /// 役割: verse(A メロ。低く・少なく・問い → 答え)/ pre(B メロ。上がって溜める)/ chorus(既定。サビ。高く・伸ばし・フック)/
    /// hook(短い動機の繰り返し)/ lead(シンセのリード。高め)。形式・輪郭・音域・音数の既定が変わる。
    #[serde(default)]
    pub role: Option<String>,
    /// ジャンル(リズムの型と点検のしきい値): pop(既定)/ edm / trap / lofi / jazz / funk(critique_melody と同じ)。
    #[serde(default)]
    pub genre: Option<String>,
    /// 下の和音の進行(write_chords と同じ書き方。足りなければ繰り返す)。省略でほかのトラックから推定。
    #[serde(default)]
    pub chords: Option<String>,
    /// キー。省略で進行から推定。
    #[serde(default)]
    pub key: Option<String>,
    /// 始まりの小節(既定 1)と小節数(既定 8)。
    #[serde(default)]
    pub bar: Option<u32>,
    #[serde(default)]
    pub bars: Option<u32>,
    /// 形式(develop_motif の form)。省略で役割から(案ごとに変える)。
    #[serde(default)]
    pub form: Option<String>,
    /// 動機の輪郭: arch(弧)/ rise / fall / valley / flat_hook(同じ音を叩く)。省略で役割から(案ごとに変える)。
    #[serde(default)]
    pub contour: Option<String>,
    /// 動機のリズムを固定する(16 分 1 つが 1 文字: x 音の頭 / - 伸ばす / . 休み。| は読み飛ばす。1〜2 小節。
    /// 7/8 なら 1 小節 14 文字)。
    /// 例 "x-x-x-x-x---x---|x-------x-------"。省略でジャンルのリズムの型から選ぶ(1 小節目は動く型、2 小節目は伸ばす型)。
    #[serde(default)]
    pub rhythm: Option<String>,
    /// 動機の小節数 1〜2(既定 2。4 小節以下の区間と、EDM・トラップの loop は 1)。
    #[serde(default)]
    pub motif_bars: Option<u32>,
    /// 音域(例 "E4-G5")。省略で役割から(verse C4-D5、pre D4-F5、chorus・hook E4-G5、lead C5-D6)。
    #[serde(default)]
    pub range: Option<String>,
    /// 最高音を置く小節(曲の小節番号)と高さ。省略で全体の 60〜75% の位置、展開したままの高さ。
    #[serde(default)]
    pub peak_bar: Option<u32>,
    #[serde(default)]
    pub peak: Option<String>,
    /// 強拍の音を 8 分前へ食わせる割合 0〜1(省略でジャンルから。pop 0.2、edm 0.35)。
    #[serde(default)]
    pub anticipate: Option<f64>,
    /// 作る案の数(既定 4、最大 12)。点検の点数の順に並べる。
    #[serde(default)]
    pub candidates: Option<usize>,
    /// 置く案の数 1〜2(既定 1)。2 なら 2 番目の案を、同じ音色のトラックを複製して(ミュートで)置き、聴き比べられるようにする。
    #[serde(default)]
    pub place: Option<usize>,
    /// 乱数の種(既定は始まりの小節番号。区間ごとに違う案になる)。案 k は seed + k で作るので、返った seed を
    /// candidates: 1 で渡すとその案だけを作り直せる。
    #[serde(default)]
    pub seed: Option<u64>,
    /// 強さ(既定 92)。
    #[serde(default)]
    pub velocity: Option<u8>,
    /// クリップの名前(既定 "Melody")。
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ChangeMeterParams {
    /// 拍子を変える小節(1 始まり)。
    pub bar: u32,
    /// 何小節続けて変えるか(既定 1)。その後は元の拍子に戻る。
    #[serde(default)]
    pub count: Option<u32>,
    /// 新しい拍子("2/4"・"7/8"・"7/8 3+2+2")。beats とどちらか。
    #[serde(default)]
    pub to: Option<String>,
    /// 今の拍子から分母の音符をいくつ足すか(+1 で 1 拍足す、-2 で 2 拍抜く。4/4 の -2 = 2/4)。to とどちらか。
    #[serde(default)]
    pub beats: Option<i32>,
}

#[derive(Deserialize, JsonSchema)]
pub struct WritePolyrhythmParams {
    /// 置く MIDI トラックの ID。新しいクリップを作る。
    pub track_id: String,
    /// "a:b" = 4 分 b 個の長さに a 個を等間隔に(3:2 = 2 拍に 3 つ = 3 連の 4 分、4:3 = 3 拍に 4 つ、5:4 = 1 小節に 5 つ)。
    pub ratio: String,
    /// 始まりの小節(既定 1)と小節数(既定 1)。この範囲に a:b の組を並べる(小節線にはそろえない)。
    #[serde(default)]
    pub bar: Option<u32>,
    #[serde(default)]
    pub bars: Option<u32>,
    /// a の側の音(音名 "C5" か MIDI 番号。既定: ドラムのトラックは 37 リム、ほかは C5)。
    #[serde(default)]
    pub pitch: Option<String>,
    /// b の側(4 分の刻み)も置くならその音。省略で置かない。
    #[serde(default)]
    pub pitch2: Option<String>,
    /// 強さ(既定 90)。組の頭は少し強く。
    #[serde(default)]
    pub velocity: Option<u8>,
    /// 音の長さ(間隔に対する割合 0.1〜1、既定 0.5)。
    #[serde(default)]
    pub gate: Option<f64>,
    /// クリップの名前(既定 "Poly a:b")。
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct WritePolymeterParams {
    /// 置く MIDI トラックの ID。新しいクリップを作る。
    pub track_id: String,
    /// 周期の型: X = 強く、x = 普通、. = 休み(1〜64 文字)。またはユークリッドリズム "E(5,16)" / "E(3,8,2)"(2 = 回す数)。
    /// 小節線を無視して周期のまま並べるので、小節と長さが違うとずれていき、最小公倍数で元に戻る。
    pub pattern: String,
    /// 1 文字の長さ: 16th(既定)/ 8th / quarter / 8t(3 連の 8 分)/ 16t、または tick の数。
    #[serde(default)]
    pub unit: Option<String>,
    /// 始まりの小節(既定 1)と小節数(既定 4)。
    #[serde(default)]
    pub bar: Option<u32>,
    #[serde(default)]
    pub bars: Option<u32>,
    /// 何小節ごとに型の頭へ戻すか(0 = 戻さない。既定 0)。
    #[serde(default)]
    pub reset_every_bars: Option<u32>,
    /// 音(音名か MIDI 番号。既定: ドラムのトラックは 37 リム、ほかは C5)。
    #[serde(default)]
    pub pitch: Option<String>,
    /// 強さ(既定 90。X は +20)。
    #[serde(default)]
    pub velocity: Option<u8>,
    /// 音の長さ(1 文字に対する割合 0.1〜1、既定 0.5)。
    #[serde(default)]
    pub gate: Option<f64>,
    /// クリップの名前(既定 "Polymeter")。
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SetMeterFeelParams {
    /// 対象クリップ ID。1 つなら clip_id、まとめて掛けるなら clip_ids(1 回の undo で戻る)。
    #[serde(default)]
    pub clip_id: Option<String>,
    #[serde(default)]
    pub clip_ids: Option<Vec<String>>,
    /// 長い拍(3 のまとまり)の長さ ÷ 短い拍(2 のまとまり)の長さ 1.2〜1.8(既定 1.5 = 変えない)。
    /// 実測は 1.43〜1.54 くらい(遅いトランシルヴァニアの舞曲で平均 1.52 = S:L 0.656)。1.4 で長い拍が詰まり、前へ転がる感じ。
    #[serde(default)]
    pub long_ratio: Option<f64>,
    /// 小節の 2 つ目以降の短い拍 ÷ 最初の短い拍 0.85〜1.0(既定 1.0。バルカンの打楽器の実測は約 0.92)。
    #[serde(default)]
    pub short_skew: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct PitchGestureParams {
    /// 対象クリップ ID。1 つなら clip_id、まとめて掛けるなら clip_ids(1 回の undo で戻る)。
    #[serde(default)]
    pub clip_id: Option<String>,
    #[serde(default)]
    pub clip_ids: Option<Vec<String>>,
    /// 表情: shakuri(しゃくり。J-POP の歌の上がり)/ scoop(下からすくう。ジャズの管・歌)/ plop(上から落ちて入る)/
    /// slide_in(下から大きく滑って入る)/ bend(チョーキング)/ prebend_release(上げて弾いて戻す)/
    /// fall(音の終わりで落ちる)/ doit(音の終わりで上へ抜ける)/ kobushi(こぶし。途中で上へ小さく回す)/ shake(速く大きく揺らす)。
    pub kind: String,
    /// 付ける音のノート ID。省略で target の規則で選ぶ。
    #[serde(default)]
    pub note_ids: Option<Vec<String>>,
    /// 規則: phrase_start(句の頭)/ phrase_end(句の終わり)/ leap_up(3 半音以上上がる音と句の頭)/ long(1 拍以上)/ all。
    /// 省略で表情ごとの既定(しゃくり・スクープ・ベンド = leap_up、フォール・ドイト = phrase_end、こぶし・シェイク = long)。
    #[serde(default)]
    pub target: Option<String>,
    /// 規則で選んだ音のうち付ける割合 0〜1(既定 0.7。全部に付けるとくどい)。note_ids を渡したときは全部。
    #[serde(default)]
    pub probability: Option<f64>,
    /// 深さ(セント。100 = 半音)。省略で表情ごとの既定(しゃくり 150、フォール 700、こぶし 80 など)。
    #[serde(default)]
    pub amount_cents: Option<f32>,
    /// 長さ(ms)。省略で表情ごとの既定(しゃくり 120、フォール 250 など)。音の長さの半分までに縮める。
    #[serde(default)]
    pub time_ms: Option<f32>,
    /// fall・doit で音量も下げるか(既定 true。音の終わりで −18 dB。音量の曲線がある音はそのまま)。
    #[serde(default)]
    pub fade_volume: Option<bool>,
    /// 乱数の種(既定 1)。
    #[serde(default)]
    pub seed: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SetVibratoParams {
    /// 対象クリップ ID。1 つなら clip_id、まとめて掛けるなら clip_ids(1 回の undo で戻る)。
    #[serde(default)]
    pub clip_id: Option<String>,
    #[serde(default)]
    pub clip_ids: Option<Vec<String>>,
    /// 付ける音のノート ID。省略で min_beats 以上の長さの音。
    #[serde(default)]
    pub note_ids: Option<Vec<String>>,
    /// 型: vocal(既定。5.5Hz・±40 セント・250ms 後から)/ vocal_strong(±80)/ strings(±20)/ guitar / wind / synth。
    #[serde(default)]
    pub style: Option<String>,
    /// 型の値を上書き: 速さ(Hz)・深さ(セント)・始まるまで(ms)・全深度になるまで(ms)・終わりで消す(ms)・終わりの速さ(Hz)。
    #[serde(default)]
    pub rate_hz: Option<f32>,
    #[serde(default)]
    pub depth_cents: Option<f32>,
    #[serde(default)]
    pub delay_ms: Option<f32>,
    #[serde(default)]
    pub fade_in_ms: Option<f32>,
    #[serde(default)]
    pub fade_out_ms: Option<f32>,
    #[serde(default)]
    pub rate_end_hz: Option<f32>,
    /// note_ids を省略したとき、この拍数以上の音だけに付ける(既定 1 = 4 分以上)。
    #[serde(default)]
    pub min_beats: Option<f64>,
    /// 音ごとの揺らぎ 0〜0.3(深さと速さを少しずつ変える。既定 0.1)。
    #[serde(default)]
    pub humanize: Option<f64>,
    /// true でビブラートを外す(奏法の vibrato は残る)。
    #[serde(default)]
    pub remove: Option<bool>,
    /// 乱数の種(既定 1)。
    #[serde(default)]
    pub seed: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct AddOrnamentParams {
    /// 対象クリップ ID。1 つなら clip_id、まとめて掛けるなら clip_ids(1 回の undo で戻る)。
    #[serde(default)]
    pub clip_id: Option<String>,
    #[serde(default)]
    pub clip_ids: Option<Vec<String>>,
    /// 装飾: acciaccatura(短前打音。前に短く 1 音)/ appoggiatura(長前打音。本音の頭の半分を上の隣で)/
    /// mordent(本音 → 下の隣 → 本音)/ inverted_mordent(本音 → 上の隣 → 本音)/ turn(上 → 本音 → 下 → 本音)/
    /// trill(本音と上の隣を音の長さいっぱいに)/ schleifer(下から 2 音で滑り込む)。
    pub kind: String,
    /// 付ける音のノート ID。省略で target の規則で選ぶ。
    #[serde(default)]
    pub note_ids: Option<Vec<String>>,
    /// 規則: phrase_start / phrase_end / leap_up / long / all(pitch_gesture と同じ)。
    /// 省略で装飾ごとの既定(前打音・シュライファー = leap_up、長前打音 = phrase_end、モルデント・ターン・トリル = long)。
    #[serde(default)]
    pub target: Option<String>,
    /// 規則で選んだ音のうち付ける割合 0〜1(既定 0.5)。note_ids を渡したときは全部。
    #[serde(default)]
    pub probability: Option<f64>,
    /// キー("D minor" など。隣の音を音階で選ぶ)。省略でクリップの音から推定。
    #[serde(default)]
    pub key: Option<String>,
    /// 隣の音を半音の数で決める(1 = 半音、2 = 全音)。省略で音階の隣。
    #[serde(default)]
    pub interval: Option<i32>,
    /// 装飾の音 1 つの長さ(ms)。省略で 8 分の 20%(45〜125ms に収める)。
    #[serde(default)]
    pub grace_ms: Option<f64>,
    /// トリルの速さ(音/秒、4〜16。既定 10)。
    #[serde(default)]
    pub trill_rate: Option<f64>,
    /// トリルの始まり: main(既定。本音から)/ upper(上の隣から。バロック)。
    #[serde(default)]
    pub trill_start: Option<String>,
    /// トリルの終わり: none(既定)/ turn(下の隣 → 本音で閉じる)。
    #[serde(default)]
    pub trill_end: Option<String>,
    /// 装飾の音の強さの割合(既定 0.7)。
    #[serde(default)]
    pub vel_ratio: Option<f64>,
    /// 乱数の種(既定 1)。
    #[serde(default)]
    pub seed: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct MelodyLeadParams {
    /// 対象クリップ ID。1 つなら clip_id、まとめて掛けるなら clip_ids(1 回の undo で戻る)。
    #[serde(default)]
    pub clip_id: Option<String>,
    #[serde(default)]
    pub clip_ids: Option<Vec<String>>,
    /// どれだけ先に鳴らすか(ms、0〜60。既定 20)。
    #[serde(default)]
    pub lead_ms: Option<f64>,
    /// fixed(既定。一定)/ by_velocity(上の音と他の音の強さの差に比例。差 20 で lead_ms、最大 2 倍)。
    #[serde(default)]
    pub mode: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct NoteDynamicsParams {
    /// 対象クリップ ID。1 つなら clip_id、まとめて掛けるなら clip_ids(1 回の undo で戻る)。
    #[serde(default)]
    pub clip_id: Option<String>,
    #[serde(default)]
    pub clip_ids: Option<Vec<String>>,
    /// 動き: swell(音の中でふくらむ)/ fade(終わりへ消える)/ sfz(頭を強く打って落とす)/ fp(強く入ってすぐ弱く)/
    /// pulse(音量の揺れ)/ open(暗くから開く。明るさ)/ close(明るくから閉じる。明るさ)。
    pub kind: String,
    /// 付ける音のノート ID。省略で target の規則で選ぶ。
    #[serde(default)]
    pub note_ids: Option<Vec<String>>,
    /// 規則(pitch_gesture と同じ)。省略で long(1 拍以上の音)。
    #[serde(default)]
    pub target: Option<String>,
    /// 規則で選んだ音のうち付ける割合 0〜1(既定 1)。
    #[serde(default)]
    pub probability: Option<f64>,
    /// 深さ: 音量なら dB(swell 12・fade 24・sfz 9・fp 12・pulse 9)、明るさなら 0〜1(既定 0.7)。
    #[serde(default)]
    pub amount: Option<f32>,
    /// sfz・fp の落ちる長さ・pulse の半周期(ms。既定 150・120・100)。
    #[serde(default)]
    pub time_ms: Option<f32>,
    /// true で音量・明るさの曲線を外す(kind の側だけ)。
    #[serde(default)]
    pub remove: Option<bool>,
    /// 乱数の種(既定 1)。
    #[serde(default)]
    pub seed: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct StrumChordParams {
    /// 対象クリップ ID。1 つなら clip_id、まとめて掛けるなら clip_ids(1 回の undo で戻る)。
    #[serde(default)]
    pub clip_id: Option<String>,
    #[serde(default)]
    pub clip_ids: Option<Vec<String>>,
    /// 型: guitar(既定。35ms、拍の手前に 2 割、後の弦ほど弱く、上げは低い弦を省く)/ piano(ばらし 120ms、下から)/
    /// harp(200ms、下から)。
    #[serde(default)]
    pub style: Option<String>,
    /// 向き: down(低い音から)/ up(高い音から)/ alternate(拍の頭は下げ、裏は上げ。guitar の既定)。
    #[serde(default)]
    pub direction: Option<String>,
    /// 型の値を上書き: 幅(ms。下げで一番低い弦から一番高い弦まで)・拍の手前に出す割合 0〜1・並びの曲がり −1〜1・
    /// 後の音の強さの変化(1 音ごと)・揺れ(ms)。up_skip_low: false は up_strings: 0 と同じ(以前の引数)。
    #[serde(default)]
    pub span_ms: Option<f64>,
    #[serde(default)]
    pub anchor: Option<f64>,
    #[serde(default)]
    pub tension: Option<f64>,
    #[serde(default)]
    pub vel_slope: Option<f64>,
    #[serde(default)]
    pub up_skip_low: Option<bool>,
    /// 上げのストローク: 鳴らす弦の数(上から。0 で全部。guitar の既定 4)・強さの倍率(既定 0.8)・
    /// 弦ごとの間隔の倍率(既定 0.75。振りが速いので短い)。piano・harp は全部の音・1・1。
    #[serde(default)]
    pub up_strings: Option<usize>,
    #[serde(default)]
    pub up_velocity: Option<f64>,
    #[serde(default)]
    pub up_span: Option<f64>,
    #[serde(default)]
    pub jitter_ms: Option<f64>,
    /// 乱数の種(既定 1)。
    #[serde(default)]
    pub seed: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct DrumRudimentParams {
    /// 対象のクリップ ID(ドラムのクリップ)。
    pub clip_id: String,
    /// 種類: flam(装飾音 1 つ)/ drag(2 つ)/ ruff(3 つ)/ ratchet(音の長さを count 回に割る)/
    /// roll(区間を rate で埋める。スネアのロール)/ hat_roll(同じくハット。トラップ)/ buzz(バズロール。ごく細かく弱く)。
    pub kind: String,
    /// flam・drag・ruff・ratchet を付ける音のノート ID。省略で pitch の音すべて(probability で間引く)。
    #[serde(default)]
    pub note_ids: Option<Vec<String>>,
    /// 対象の音程(flam 等: この音程の音に付ける。roll 等: 置く音程。既定 38 スネア、hat_roll は 42)。
    #[serde(default)]
    pub pitch: Option<u8>,
    /// 付ける割合 0〜1(既定 1。note_ids を渡したときは全部)。
    #[serde(default)]
    pub probability: Option<f64>,
    /// 装飾音の間隔(ms。既定 flam 25・drag 20・ruff 18)と強さの割合(既定 0.5)。
    #[serde(default)]
    pub grace_ms: Option<f64>,
    #[serde(default)]
    pub grace_vel: Option<f64>,
    /// ratchet の回数 2〜8(既定 3)。
    #[serde(default)]
    pub count: Option<u32>,
    /// roll・hat_roll・buzz の区間: 小節(曲の小節番号)・拍(1 始まり、小数可。既定 1)・拍数(既定 1)。
    #[serde(default)]
    pub bar: Option<u32>,
    #[serde(default)]
    pub beat: Option<f64>,
    #[serde(default)]
    pub length_beats: Option<f64>,
    /// 刻み: 1/16 / 1/32(roll・hat_roll の既定)/ 1/64(buzz の既定)/ 1/16t / 1/32t、または Hz("24hz")。
    #[serde(default)]
    pub rate: Option<String>,
    /// 強さ(既定: roll 70、buzz 45)と、区間の中での変化 −128〜127(正でだんだん強く。既定 roll 64、ほか 0)。
    #[serde(default)]
    pub velocity: Option<u8>,
    #[serde(default)]
    pub vel_curve: Option<i32>,
    /// 乱数の種(既定 1)。
    #[serde(default)]
    pub seed: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ArticulateNotesParams {
    /// 対象クリップ ID。1 つなら clip_id、まとめて掛けるなら clip_ids(1 回の undo で戻る)。
    #[serde(default)]
    pub clip_id: Option<String>,
    #[serde(default)]
    pub clip_ids: Option<Vec<String>>,
    /// 切り方: legato(次の音に少し重ねる)/ tenuto(次の音まで目いっぱい)/ portato(次の音までの 75%)/ staccato(50%)。
    pub style: String,
    /// 対象のノート ID。省略でクリップ内の全ノート。
    #[serde(default)]
    pub note_ids: Option<Vec<String>>,
    /// 次の音までの間隔に対する長さの割合(portato・staccato の上書き。0.1〜1)。
    #[serde(default)]
    pub ratio: Option<f64>,
    /// legato で重ねる長さ(ms、既定 15)と、同じ音が続くときに空ける長さ(ms、既定 30)。
    #[serde(default)]
    pub overlap_ms: Option<f64>,
    #[serde(default)]
    pub repeat_gap_ms: Option<f64>,
    /// legato のとき、句の 2 音目以降に奏法 legato も付ける(弾き直さずにつなぐ。弦・管・シンセのリード)。既定 false。
    #[serde(default)]
    pub slur: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct TremoloParams {
    /// 対象クリップ ID。
    pub clip_id: String,
    /// 種類: single(同じ音の連打。弦の刻み・マンドリン)/ alternating(上の音と交互。フィンガートレモロ)/ chord(和音の連打)。
    pub kind: String,
    /// 対象のノート ID。省略で min_beats 拍以上の音。
    #[serde(default)]
    pub note_ids: Option<Vec<String>>,
    /// 刻み: 1/16 / 1/32(既定)/ 1/64 / 1/16t / 1/32t。rate_hz を渡すとそれに近い刻み(数えないトレモロ)。
    #[serde(default)]
    pub division: Option<String>,
    #[serde(default)]
    pub rate_hz: Option<f64>,
    /// alternating の上の音: 半音の数(省略でキーの音階の 3 度上)。
    #[serde(default)]
    pub interval: Option<i32>,
    /// キー(省略でクリップから推定)。
    #[serde(default)]
    pub key: Option<String>,
    /// note_ids 省略時の対象の長さ(拍、既定 1)。
    #[serde(default)]
    pub min_beats: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GlissandoParams {
    /// 対象クリップ ID。
    pub clip_id: String,
    /// グリッサンドを始める音のノート ID(その音から to へ)。
    pub note_ids: Vec<String>,
    /// 行き先の音("C6" か MIDI 番号)。省略で旋律の次の音。
    #[serde(default)]
    pub to: Option<String>,
    /// steps(既定。音を並べる。ピアノ・ハープ)/ continuous(音程の曲線で滑らかに。弦・トロンボーン・シンセ)。
    #[serde(default)]
    pub mode: Option<String>,
    /// 音の選び方(steps): key(既定。キーの音階)/ white(白鍵)/ black(黒鍵)/ chromatic(半音)。
    #[serde(default)]
    pub scale: Option<String>,
    /// キー(省略でクリップから推定)。
    #[serde(default)]
    pub key: Option<String>,
    /// 始める音の長さのうち、グリッサンドに使う後ろの割合 0.1〜1(既定 steps 1、continuous 0.5)。
    #[serde(default)]
    pub portion: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ShapePhraseParams {
    /// 句の始まりの小節(曲の小節番号)と小節数(既定 4)。
    pub bar: u32,
    #[serde(default)]
    pub bars: Option<u32>,
    /// 弧の大きさ 0〜1(句の半ばへ最大 6% 速く、また戻す。既定 0.5。0 で無し)。
    #[serde(default)]
    pub arc: Option<f64>,
    /// 終わりのリタルダンドの拍数(既定 4。0 で無し)と、最後のテンポの倍率 w(0.2〜1、既定 0.7。曲の終わりなら 0.5〜0.6)。
    #[serde(default)]
    pub ritard_beats: Option<f64>,
    #[serde(default)]
    pub final_tempo: Option<f64>,
    /// 緩み方の形 q(0.5〜5、既定 2。Friberg らの式 v(x) = [1 + (w^q − 1)x]^(1/q)。大きいほど最後に急に緩む)。
    #[serde(default)]
    pub curve: Option<f64>,
    /// 強さもテンポに連動させる(速い所は強く、緩む所は弱く。既定 true。ドラムのトラックも含む)。
    #[serde(default)]
    pub couple_dynamics: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct MetricModulationParams {
    /// 切り替える小節(曲の小節番号)。その小節の頭から新しいテンポ。
    pub bar: u32,
    /// 前のテンポの音価(from)= 新しいテンポの音価(to)。quarter / 8th / 16th / half / whole、dotted_ と triplet_ を前に付けられる
    /// (例 from triplet_8th・to 8th で 1.5 倍、from dotted_quarter・to quarter で 2/3 倍)。ratio とどちらか。
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    /// 新しいテンポ ÷ 前のテンポ("3:2" = 1.5 倍)。
    #[serde(default)]
    pub ratio: Option<String>,
    /// 同じ小節から拍子も変える("6/8"・"7/8 2+2+3")。省略で変えない。
    #[serde(default)]
    pub time_sig: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct HemiolaParams {
    /// 3 拍子の 2 小節の最初の小節(曲の小節番号)。
    pub bar: u32,
    /// アクセントを付け直すクリップ(省略で、その 2 小節に音のある全クリップ)。
    #[serde(default)]
    pub clip_ids: Option<Vec<String>>,
    /// アクセントの強さ 0〜0.5(既定 0.25。2 拍ごとの頭を強く、元の 2 小節目の頭を弱く)。
    #[serde(default)]
    pub accent: Option<f64>,
    /// true で拍子も 2 小節ぶんを 1 小節の 6/4(2+2+2)に書き換える(譜面・ルーラーの見た目もヘミオラに。音の位置は同じ)。
    #[serde(default)]
    pub rebar: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct WriteTihaiParams {
    /// 置く MIDI トラックの ID。新しいクリップを作る。
    pub track_id: String,
    /// 句(16 分 1 つが 1 文字: x = 音、- = 伸ばす、. = 休み)。例 "x.x.xx"。3 回繰り返す。
    pub phrase: String,
    /// 句と句の間の休み(ステップ、既定 2)。
    #[serde(default)]
    pub gap_steps: Option<u64>,
    /// 着地する小節(曲の小節番号)。3 回目の最後の音がその小節の頭に来る。
    pub land_bar: u32,
    /// 1 文字の長さ: 16th(既定)/ 8th / 16t。
    #[serde(default)]
    pub unit: Option<String>,
    /// 音(音名か MIDI 番号。既定: ドラムのトラックは 38 スネア、ほかは C5)。
    #[serde(default)]
    pub pitch: Option<String>,
    /// 強さ(既定 90。各回の頭と着地の音は強く)。
    #[serde(default)]
    pub velocity: Option<u8>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ModulateParams {
    /// トラックの ID。
    pub track_id: String,
    /// 揺らすつまみ: 音源のつまみの名前("cutoff"・"position"・"index" など)、エフェクトの "種類.つまみ"
    /// ("auto_filter.cutoff"・"delay.mix"。そのトラックの最初のその種類)か "fx/<id>/<名前>"。remove のときは省略で全部外す。
    #[serde(default)]
    pub target: Option<String>,
    /// 形: sine(既定)/ triangle / square / saw_up / saw_down / random(周期ごとに跳ぶ S&H)。
    #[serde(default)]
    pub shape: Option<String>,
    /// テンポに合わせた周期("1/4"・"1/8d"・"1/8t"・"1/1"・"4/1" = 4 小節)。省略で rate_hz。
    #[serde(default)]
    pub sync: Option<String>,
    /// 周期(Hz。sync が無いとき。既定 1)。
    #[serde(default)]
    pub rate_hz: Option<f32>,
    /// 揺らす幅(つまみの単位で ±。cutoff なら Hz)。省略でつまみの範囲の 1/4。
    #[serde(default)]
    pub depth: Option<f64>,
    /// 始まりの位相 0〜1(0.25 で頂点から)。
    #[serde(default)]
    pub phase: Option<f32>,
    /// true で外す。
    #[serde(default)]
    pub remove: Option<bool>,
}

/// コード進行を小節と区間に並べたもの(write_chords・write_bassline で共通)
struct Layout {
    /// 鳴らす和音(休みを除く)の並び
    chords: Vec<glaux_core::chord::Chord>,
    /// 区間(クリップの頭から)。chord は `chords` の番号
    spans: Vec<glaux_core::comp::Span>,
    /// 区間ごとの小節番号
    span_bar: Vec<u32>,
    /// クリップの頭からの (小節の頭, 小節の長さ)
    bar_list: Vec<(u64, u64)>,
    clip_start: u64,
    clip_len: u64,
    n_bars: u32,
}

/// コード進行の文字列を読み、小節の中を均等に分けて区間にする。`merge_same` なら同じ和音が続く区間を 1 つにまとめる
fn progression_layout(
    project: &glaux_core::Project,
    chords: &str,
    key: Option<&str>,
    repeat: Option<u32>,
    bar: Option<u32>,
    merge_same: bool,
) -> Result<Layout, String> {
    use glaux_core::{chord, comp};
    let key =
        match key {
            Some(k) => Some(chord::Key::parse(k).ok_or_else(|| {
                format!("key は \"C major\" / \"A minor\" / \"F#m\" の形(got: {k})")
            })?),
            None => None,
        };
    let mut bars_sym = chord::split_progression(chords)?;
    let repeat = repeat.unwrap_or(1).clamp(1, 64) as usize;
    bars_sym = bars_sym
        .iter()
        .cloned()
        .cycle()
        .take(bars_sym.len() * repeat)
        .collect();
    if bars_sym.len() > 512 {
        return Err("小節が多すぎます(512 まで)".to_owned());
    }
    // 記号を読む(読めないものはまとめて知らせる)
    let mut errors = Vec::new();
    let parsed: Vec<Vec<Option<chord::Chord>>> = bars_sym
        .iter()
        .map(|bar| {
            bar.iter()
                .map(|sym| match chord::parse_in_key(sym, key) {
                    Ok(c) => c,
                    Err(e) => {
                        errors.push(e);
                        None
                    }
                })
                .collect()
        })
        .collect();
    if !errors.is_empty() {
        errors.dedup();
        return Err(format!("コードが読めません: {}", errors.join(" / ")));
    }
    // 小節の位置
    let first_bar = bar.unwrap_or(1).max(1);
    let n_bars = bars_sym.len() as u32;
    let (clip_start, clip_len) =
        glaux_core::arrange::bar_range(project, first_bar, n_bars).ok_or("小節を数えられません")?;
    let bar_list: Vec<(u64, u64)> = (0..n_bars)
        .map(|i| {
            glaux_core::arrange::bar_range(project, first_bar + i, 1)
                .map(|(s, l)| (s - clip_start, l))
                .ok_or("小節を数えられません")
        })
        .collect::<Result<_, _>>()?;
    let mut chords_list: Vec<chord::Chord> = Vec::new();
    let mut spans: Vec<comp::Span> = Vec::new();
    let mut span_bar: Vec<u32> = Vec::new();
    let meters = bar_meters_in(project, clip_start, clip_len);
    for (bi, (bar_chords, &(bstart, blen))) in parsed.iter().zip(&bar_list).enumerate() {
        let k = bar_chords.len() as u64;
        // 小節の中の和音の変わり目: 4/4 は等分、変拍子はまとまりの頭(足りなければ 16 分の格子)
        let cuts = chord_cuts(meters.get(bi), blen, k);
        for (ci, c) in bar_chords.iter().enumerate() {
            let start = bstart + cuts[ci];
            let end = bstart + cuts[ci + 1];
            let same_as_prev = merge_same
                && spans.last().is_some_and(|last: &comp::Span| {
                    last.start + last.len == start
                        && match (last.chord, c) {
                            (Some(i), Some(c)) => chords_list[i].name == c.name,
                            (None, None) => true,
                            _ => false,
                        }
                });
            if same_as_prev {
                let last = spans.last_mut().expect("直前がある");
                last.len = end - last.start;
                continue;
            }
            let idx = c.as_ref().map(|c| {
                chords_list.push(c.clone());
                chords_list.len() - 1
            });
            spans.push(comp::Span {
                start,
                len: end - start,
                chord: idx,
            });
            span_bar.push(first_bar + bi as u32);
        }
    }
    if chords_list.is_empty() {
        return Err("鳴らすコードがありません(すべて N.C.)".to_owned());
    }
    Ok(Layout {
        chords: chords_list,
        spans,
        span_bar,
        bar_list,
        clip_start,
        clip_len,
        n_bars,
    })
}

/// 同じ音色・エフェクト・音量のトラックの器(クリップ・オートメーションは空。エフェクトは新しい ID で、つながりも付け替える)
fn copy_track_shell(track: &glaux_core::Track, name: String) -> glaux_core::Track {
    use glaux_core::model::routing::FxNode;
    let mut t = track.clone();
    t.id = glaux_core::TrackId::new();
    t.name = name;
    t.mute = true;
    t.solo = false;
    t.clips.clear();
    t.automation.clear();
    let mut map = std::collections::HashMap::new();
    for e in &mut t.effects {
        let new = glaux_core::FxId::new();
        map.insert(e.id.clone(), new.clone());
        e.id = new;
    }
    if let Some(links) = &mut t.fx_links {
        let remap = |n: &mut FxNode| {
            if let FxNode::Fx(id) = n {
                if let Some(new) = map.get(id) {
                    *id = new.clone();
                }
            }
        };
        for l in links.iter_mut() {
            remap(&mut l.from);
            remap(&mut l.to);
        }
    }
    // 変調のエフェクトの先も新しい ID に
    for m in &mut t.modulators {
        if let glaux_core::ParamPath::Effect { id, .. } = &mut m.target {
            if let Some(new) = map.get(id) {
                *id = new.clone();
            }
        }
    }
    t
}

/// 旋律の道具の下の和音(クリップの頭からの区間と、和音の列)。`chords` が無ければほかのトラックから推定する
#[allow(clippy::too_many_arguments)]
fn melody_chords(
    project: &glaux_core::Project,
    tid: &glaux_core::TrackId,
    chords: Option<&str>,
    key: Option<&str>,
    first_bar: u32,
    total_bars: u32,
    clip_start: u64,
    clip_len: u64,
) -> Result<(Vec<glaux_core::comp::Span>, Vec<glaux_core::chord::Chord>), String> {
    use glaux_core::chord;
    if let Some(ch) = chords {
        let prog_bars = chord::split_progression(ch)?.len() as u32;
        let repeat = total_bars.div_ceil(prog_bars.max(1));
        let l = progression_layout(project, ch, key, Some(repeat), Some(first_bar), false)?;
        return Ok((l.spans, l.chords));
    }
    let others: Vec<glaux_core::TrackId> = project
        .tracks
        .iter()
        .filter(|t| &t.id != tid && !is_drum_track(t))
        .map(|t| t.id.clone())
        .collect();
    let h = glaux_core::harmony::analyze(project, Some(&others), None);
    let mut spans = Vec::new();
    let mut list = Vec::new();
    for (i, c) in h.chords.iter().enumerate() {
        let end = h.chords.get(i + 1).map_or(u64::MAX, |n| n.tick);
        if end <= clip_start || c.tick >= clip_start + clip_len {
            continue;
        }
        if let Ok(Some(ch)) = chord::parse(&c.chord) {
            let s0 = c.tick.max(clip_start) - clip_start;
            let s1 = end.min(clip_start + clip_len) - clip_start;
            list.push(ch);
            spans.push(glaux_core::comp::Span {
                start: s0,
                len: s1 - s0,
                chord: Some(list.len() - 1),
            });
        }
    }
    Ok((spans, list))
}

/// 展開した旋律をクリップにする(小節の頭・半ばの音は少し強く)
fn melody_clip(
    out: &[glaux_core::motif::Out],
    clip_start: u64,
    clip_len: u64,
    bar_len: u64,
    vel: u8,
    name: String,
) -> glaux_core::Clip {
    let vel = vel.clamp(1, 127);
    let mut clip = glaux_core::Clip::new_midi(
        glaux_core::ClipId::new(),
        name,
        glaux_core::Tick(clip_start),
        glaux_core::Tick(clip_len),
    );
    if let Some(ns) = clip.notes_mut() {
        *ns = out
            .iter()
            .filter(|o| o.pos < clip_len)
            .map(|o| glaux_core::Note {
                id: glaux_core::NoteId::new(),
                pos: glaux_core::Tick(o.pos),
                dur: glaux_core::Tick(o.dur.min(clip_len - o.pos)),
                pitch: o.pitch,
                vel: if o.pos % (bar_len / 2).max(1) == 0 {
                    (vel as u16 + 6).min(127) as u8
                } else {
                    vel
                },
                articulation: Default::default(),
                pitch_curve: vec![],
                glide_ms: None,
                vibrato: None,
                volume_curve: vec![],
                brightness_curve: vec![],
            })
            .collect();
        ns.sort_by(|a, b| (a.pos, a.pitch, &a.id).cmp(&(b.pos, b.pitch, &b.id)));
    }
    clip
}

/// 展開した旋律を点検する(区間のマーカーと下の和音を見る)
fn melody_critique(
    project: &glaux_core::Project,
    target: &str,
    out: &[glaux_core::motif::Out],
    clip_start: u64,
    look: &dyn Fn(u64) -> Option<glaux_core::chord::Chord>,
    key: glaux_core::chord::Key,
    genre: &'static glaux_core::melody::Genre,
) -> glaux_core::melody::MelodyCritique {
    use glaux_core::melody;
    let mel: Vec<melody::MelNote> = out
        .iter()
        .map(|o| melody::MelNote {
            pos: clip_start + o.pos,
            dur: o.dur,
            pitch: o.pitch,
        })
        .collect();
    let abs_look = |t: u64| {
        t.checked_sub(clip_start)
            .and_then(look)
            .map(|c| c.pitch_classes())
    };
    let mut marks = project.sections.clone();
    marks.sort_by_key(|m| m.tick);
    let clip_end = out
        .iter()
        .map(|o| clip_start + o.pos + o.dur)
        .max()
        .unwrap_or(0);
    let end = project.end().0.max(clip_end);
    let sections = marks
        .iter()
        .enumerate()
        .map(|(i, m)| melody::SectionSpan {
            name: m.name.clone(),
            start: m.tick.0,
            end: marks.get(i + 1).map_or(end, |n| n.tick.0),
            energy: m.energy,
        })
        .collect();
    melody::critique(
        &mel,
        &melody::Context {
            project,
            chord_at: &abs_look,
            key: Some(key),
            genre,
            sections,
            target: target.to_owned(),
        },
    )
}

/// `start` から `len` の範囲の小節の拍子(小節の頭は曲の頭からの tick のまま)
fn bar_meters_in(
    project: &glaux_core::Project,
    start: u64,
    len: u64,
) -> Vec<glaux_core::meter::BarMeter> {
    glaux_core::meter::bar_meters(project, start + len)
        .into_iter()
        .filter(|m| m.start >= start && m.start < start + len)
        .collect()
}

/// 小節の中の和音の変わり目(小節の頭からの tick。先頭 0・末尾 `blen` を含む `k + 1` 個)。
/// 4/4 は等分、変拍子はまとまりの頭のうち等分に近いもの(足りなければ 16 分の格子に丸める)
fn chord_cuts(meter: Option<&glaux_core::meter::BarMeter>, blen: u64, k: u64) -> Vec<u64> {
    let k = k.max(1);
    let even: Vec<u64> = (0..=k).map(|i| blen * i / k).collect();
    let Some(m) = meter.filter(|m| !m.is_common()) else {
        return even;
    };
    let heads: Vec<u64> = m.groups().iter().map(|g| g.0).filter(|&h| h > 0).collect();
    let mut cuts = vec![0u64];
    for i in 1..k {
        let target = even[i as usize];
        let prev = *cuts.last().unwrap_or(&0);
        let pick = heads
            .iter()
            .copied()
            .filter(|&h| h > prev)
            .min_by_key(|&h| h.abs_diff(target));
        let step = glaux_core::meter::STEP;
        let snapped = (target + step / 2) / step * step;
        cuts.push(match pick {
            // 残りの和音の数だけ頭が残っているときだけ使う
            Some(h) if heads.iter().filter(|&&x| x > h).count() as u64 >= k - 1 - i => h,
            _ => snapped.max(prev + step),
        });
    }
    cuts.push(blen);
    cuts
}

/// GM のドラムの音程を楽器の分け方に(write_drums の parts)
fn drum_part_of(pitch: u8) -> &'static str {
    match pitch {
        35 | 36 => "kick",
        37..=40 => "snare",
        42 | 44 | 46 => "hat",
        41 | 43 | 45 | 47 | 48 | 50 => "tom",
        49 | 51 | 52 | 53 | 55 | 57 | 59 => "cymbal",
        _ => "perc",
    }
}

/// clip_id と clip_ids をまとめる(重複は除く。どちらも無ければエラー)
fn clip_id_list(one: &Option<String>, many: &Option<Vec<String>>) -> Result<Vec<String>, String> {
    let mut ids: Vec<String> = many.clone().unwrap_or_default();
    if let Some(c) = one {
        if !ids.contains(c) {
            ids.push(c.clone());
        }
    }
    if ids.is_empty() {
        return Err("clip_id か clip_ids を指定してください".to_owned());
    }
    Ok(ids)
}

/// 音名("C5")か MIDI 番号("37")。省略なら `default`
fn parse_pitch(s: Option<&str>, default: u8) -> Result<u8, String> {
    match s.map(str::trim) {
        None | Some("") => Ok(default),
        Some(x) => x
            .parse::<u8>()
            .ok()
            .filter(|n| *n <= 127)
            .or_else(|| glaux_core::chord::parse_note(x))
            .ok_or_else(|| format!("音が読めません: {x}(\"C5\" か 0〜127)")),
    }
}

/// (位置, 長さ, 音, 強さ) の列から MIDI クリップを作る
fn simple_clip(
    name: String,
    start: u64,
    len: u64,
    notes: &[(u64, u64, u8, u8)],
) -> glaux_core::Clip {
    let mut clip = glaux_core::Clip::new_midi(
        glaux_core::ClipId::new(),
        name,
        glaux_core::Tick(start),
        glaux_core::Tick(len),
    );
    if let Some(ns) = clip.notes_mut() {
        *ns = notes
            .iter()
            .map(|&(pos, dur, pitch, vel)| glaux_core::Note {
                id: glaux_core::NoteId::new(),
                pos: glaux_core::Tick(pos),
                dur: glaux_core::Tick(dur.min(len - pos).max(1)),
                pitch,
                vel,
                articulation: Default::default(),
                pitch_curve: vec![],
                glide_ms: None,
                vibrato: None,
                volume_curve: vec![],
                brightness_curve: vec![],
            })
            .collect();
        ns.sort_by(|a, b| (a.pos, a.pitch, &a.id).cmp(&(b.pos, b.pitch, &b.id)));
    }
    clip
}

/// "E3-C5" / "low" / "mid" / "high" を音域に
fn parse_range(s: Option<&str>) -> Result<(u8, u8), String> {
    let s = s.unwrap_or("mid").trim();
    match s.to_ascii_lowercase().as_str() {
        "low" => return Ok((45, 69)),
        "mid" => return Ok((52, 76)),
        "high" => return Ok((60, 84)),
        _ => {}
    }
    let (a, b) = s
        .split_once(['-', '~', '〜'])
        .ok_or_else(|| format!("range は low / mid / high か \"E3-C5\" の形(got: {s})"))?;
    let lo = glaux_core::chord::parse_note(a).ok_or_else(|| format!("音名が読めません: {a}"))?;
    let hi = glaux_core::chord::parse_note(b).ok_or_else(|| format!("音名が読めません: {b}"))?;
    Ok((lo.min(hi), lo.max(hi)))
}

#[derive(Deserialize, JsonSchema)]
pub struct SongPlanSection {
    /// 区間の名前(intro / Aメロ / ビルド / ドロップ など)。
    pub name: String,
    /// 小節数(4 / 8 / 16 が基本)。
    pub bars: u32,
    /// 盛り上がり 0〜10(critique_arrangement の区間の energy と同じ目盛り。山は 8〜10、静かな区間は 2〜5)。
    #[serde(default)]
    pub energy: Option<f32>,
    /// この区間で鳴らすトラックの名前(トラックを作る前でもよい。critique が実際と突き合わせる)。
    #[serde(default)]
    pub tracks: Option<Vec<String>>,
    /// 役割・意図(例「キックとベースを抜いてパッドと旋律だけ」「フィルタを開いて次のドロップを予告」)。
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SetSongPlanParams {
    /// 区間の並び(曲の頭から順に。区間のマーカーはこれで置き直す)。
    pub sections: Vec<SongPlanSection>,
    /// 最初の区間の小節(既定 1)。
    #[serde(default)]
    pub start_bar: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
pub struct TransformNotesParams {
    /// 対象クリップ ID(`clp_xxxxxx`)。
    pub clip_id: String,
    /// 変形: invert(反行。axis を中心に上下を反転)/ retrograde(逆行。範囲の中で時間を逆に)/
    /// transpose(音階の度数で移調。steps)/ sequence(反復進行。steps 度ずつずらして times 回、後ろへ写す)/
    /// stretch(範囲の頭を中心に位置と長さを factor 倍。2 = 拡大、0.5 = 縮小)。
    pub op: String,
    /// transpose / sequence の度数(音階の度数。1 = 2 度上、-2 = 3 度下、7 = 1 オクターブ)。
    #[serde(default)]
    pub steps: Option<i32>,
    /// sequence の回数(1〜16、既定 1)。
    #[serde(default)]
    pub times: Option<u32>,
    /// sequence で 1 回ごとに後ろへずらす量(拍)。省略で選んだ音の長さ(動機の長さ)。
    #[serde(default)]
    pub offset_beats: Option<f64>,
    /// invert の軸(MIDI 番号)。省略で最初の音。
    #[serde(default)]
    pub axis: Option<u8>,
    /// stretch の倍率(0.125〜8)。
    #[serde(default)]
    pub factor: Option<f64>,
    /// 音階: "C major" / "A minor" / "F# minor" など、または "chromatic"(半音で数える)。省略で曲から推定したキー。
    #[serde(default)]
    pub key: Option<String>,
    /// 対象ノート ID の配列。省略でクリップ内の全ノート。
    #[serde(default)]
    pub note_ids: Option<Vec<String>>,
}

/// "C major" / "A minor" / "Bb minor" / "chromatic" → 音階
fn parse_key(s: &str) -> Result<glaux_core::transform::Scale, String> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("chromatic") {
        return Ok(glaux_core::transform::Scale::chromatic());
    }
    let (note, mode) = s
        .split_once(' ')
        .ok_or_else(|| format!("key は \"C major\" / \"A minor\" の形で(got: {s})"))?;
    let mut chars = note.chars();
    let base = match chars.next().map(|c| c.to_ascii_uppercase()) {
        Some('C') => 0,
        Some('D') => 2,
        Some('E') => 4,
        Some('F') => 5,
        Some('G') => 7,
        Some('A') => 9,
        Some('B') => 11,
        _ => return Err(format!("key の音名が不正です({note})")),
    };
    let acc: i32 = chars
        .map(|c| match c {
            '#' | '♯' => 1,
            'b' | '♭' => -1,
            _ => 0,
        })
        .sum();
    let tonic = (base + acc).rem_euclid(12) as u8;
    let mode = match mode.trim().to_ascii_lowercase().as_str() {
        "major" | "maj" => "major",
        "minor" | "min" => "minor",
        other => return Err(format!("key の種類は major / minor(got: {other})")),
    };
    Ok(glaux_core::transform::Scale::new(
        glaux_core::harmony::scale_pitch_classes(tonic, mode),
    ))
}

/// クリップの範囲の小節の頭(拍子に沿う)を引く関数を作る
fn bar_start_fn(project: &glaux_core::Project, end: u64) -> impl Fn(u64) -> u64 {
    let grid: Vec<u64> = glaux_core::arrange::bar_grid(project, end + 3840)
        .into_iter()
        .map(|(t, _)| t)
        .collect();
    move |t: u64| {
        let i = grid.partition_point(|&b| b <= t);
        grid.get(i.saturating_sub(1)).copied().unwrap_or(0)
    }
}

/// トラックがドラム(音程で楽器を分ける)か
fn is_drum_track(track: &glaux_core::Track) -> bool {
    match track.device.as_ref().map(|d| &d.source) {
        Some(glaux_core::PluginSource::Builtin { name }) => name == "drum",
        Some(glaux_core::PluginSource::Sf2 { bank, .. }) => *bank == 128,
        _ => false,
    }
}

/// つまみの一覧(JSON)から、path が一致するつまみの (最小, 最大) を探す
fn find_param_range(v: &Value, path: &str) -> Option<(f64, f64)> {
    match v {
        Value::Object(m) => {
            if m.get("path").and_then(Value::as_str) == Some(path) {
                let r = &m["range"];
                if let (Some(lo), Some(hi)) = (r["min"].as_f64(), r["max"].as_f64()) {
                    return Some((lo, hi));
                }
            }
            m.values().find_map(|x| find_param_range(x, path))
        }
        Value::Array(a) => a.iter().find_map(|x| find_param_range(x, path)),
        _ => None,
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct GetGuideParams {
    /// workflow / melody / groove / instruments / genres / expression / mix / audio / sound_match / clap。省略で一覧。
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
    /// true で小節の中で回す(拍の置き換え。小節の後ろにはみ出した音は同じ小節の頭へ)。リフを 8 分ずらして
    /// 裏から聞かせる、など。
    #[serde(default)]
    pub wrap_in_bar: Option<bool>,
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
    /// 対象クリップ ID(`clp_xxxxxx`)。1 つなら clip_id、まとめて掛けるなら clip_ids(1 回の undo で戻る)。
    #[serde(default)]
    pub clip_id: Option<String>,
    /// 対象クリップ ID の配列(トラックをまたいでよい)。
    #[serde(default)]
    pub clip_ids: Option<Vec<String>>,
    /// 裏拍の単位(tick)。480 = 8 分(既定)、240 = 16 分。
    #[serde(default)]
    pub grid_ticks: Option<u64>,
    /// スウィング率 0.5〜0.8。0.5 = ストレート、0.58 ≈ 軽め、0.667 ≈ 3 連(シャッフル)、0.75 = 付点(ハネ強め)。
    /// mode: jazz_tempo のときは省略できる。
    #[serde(default)]
    pub swing: Option<f64>,
    /// jazz_tempo: テンポから量を決める(短い方の音 = max(100ms, 3 連の 8 分)。遅い曲は 3 連、速い曲ほどまっすぐ。
    /// Friberg & Sundström の実測)。
    #[serde(default)]
    pub mode: Option<String>,
    /// 掛かり具合 0.0〜1.0(省略時 1.0)。
    #[serde(default)]
    pub strength: Option<f64>,
    /// 対象ノート ID の配列(clip_id を 1 つ指定したときだけ)。省略でクリップ内の全ノート。
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
    for key in [
        "articulation",
        "pitch_curve",
        "glide_ms",
        "vibrato",
        "volume_curve",
        "brightness_curve",
    ] {
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
        // 拍子: まとまり(省略時は既定値)・1 小節の tick・16 分の数を添える(AI が拍を数え間違えないように)
        if let Some(arr) = v.get_mut("time_sig_map").and_then(Value::as_array_mut) {
            for (e, sig) in arr.iter_mut().zip(&project.time_sig_map) {
                let len = glaux_core::PPQ * 4 * sig.num as u64 / sig.den.max(1) as u64;
                let m = glaux_core::meter::BarMeter::from_sig(sig, sig.tick.0, len);
                e["meter"] = json!(m.label());
                e["bar_ticks"] = json!(m.len);
                e["steps_16th"] = json!(m.steps());
            }
        }
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
        連続ピッチカーブ: ノートの pitch_curve に [{tick, cents, shape?}](tick はノート先頭からの相対、\
        cents は書かれた音程からのずれ。100 = 半音、±2400 まで、最大 16 点、点から次の点までの曲がり方 shape は\
        linear(既定)/ ease_in / ease_out / ease_in_out / hold、両端は保持)を書くと自由なベンド・ポルタメント・うねりが作れる\
        (しゃくり・フォールなどの定番は pitch_gesture、ビブラートの細かい指定はノートの vibrato か set_vibrato)\
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
        track_id を省略するとカタログ: 内蔵楽器(subtractive / drum / pluck / fm / wavetable 等)と内蔵エフェクト(eq / dynamic_eq / resonance / compressor / multiband / transient / limiter / width / virtual_bass / reverb / delay / chorus / tape 等)の\
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
        side_to_mid_db=広がり / balance_db=左右の偏り / bands=低・中・高の帯域ごとの相関と広がり(低いほど中央、高いほど広いのがふつう) /\
        negative_correlation_ratio=逆相だった時間の割合 / mono_loudness_change_db=モノラルにしたときのラウドネスの変化。\
        無相関で約 -3、それより大きく下がるならモノラルで音が消えている)、\
        tonal_balance(third_octave=1/3 オクターブごとの量、slope_db_per_oct=傾き。0 = ピンクノイズと同じ・マイナスほど暗い、\
        deviations=傾きの直線から ±3dB 以上ずれた帯域。プラスはこもり・刺さりの候補。EQ で直す場所の目星に)。\
        【ミックスバランスの診断】per_track: true で各トラックの loudness/band_energy 一覧と、\
        masking(トラック間の周波数のかぶり。心理音響モデル(広がり・純音か雑音か・聞こえる最小の音・直後の残り)で、\
        track が masked_by に band_hz の帯域で time_ratio の時間覆われて聞こえない。band_share はその帯域が track の音に占める割合、\
        suggest_cut_db は masked_by をその帯域でどれだけ下げれば聞こえてくるかの目安)が返る。\
        かぶりは EQ・dynamic_eq(source に track を入れると、track が鳴る間だけ masked_by を下げる)で削る・パンで分ける・sidechain で解消する。\
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
        description = "畳み込みリバーブを足す。インパルス応答(IR: 実在の部屋・ホール・教会・スプリング・プレートなどの響きを録った音声)を\
        プロジェクトに取り込み、トラック(省略でマスター)に convolution エフェクトを挿す(1 回の undo で戻せる)。\
        本物の空間の質感が欲しいときに。複数のトラックで同じ響きを使うなら、バスに mix 1 で挿して set_send で送るのが軽い。\
        つまみ(mix / wet_db / length)は set_param で変えられる。約 11ms 遅れる(エンジンが遅延補正する)。"
    )]
    async fn import_ir(
        &self,
        params: Parameters<ImportIrParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("import_ir");
        let p = params.0;
        let (project, _) = self.handle.get_project().await?;
        let track = match &p.track_id {
            Some(t) => {
                let id = glaux_core::TrackId::parse(t).map_err(|e| e.to_string())?;
                if project.track(&id).is_none() {
                    return Err(format!("トラックが見つかりません: {id}"));
                }
                Some(id)
            }
            None => None,
        };
        let dir = self.handle.project_dir().await?;
        let imported =
            crate::assets::import_audio(std::path::Path::new(&dir), std::path::Path::new(&p.path))?;
        let mut cmds = Vec::new();
        if !project.assets.contains_key(&imported.id) {
            cmds.push(Command::AddAsset {
                id: imported.id.clone(),
                asset: imported.asset.clone(),
            });
        }
        let mut effect = glaux_core::Effect::builtin(glaux_core::FxId::new(), "convolution");
        effect.params.insert(
            "ir".into(),
            glaux_core::ParamValue::Enum(imported.id.to_string()),
        );
        if let Some(m) = p.mix {
            effect.params.insert(
                "mix".into(),
                glaux_core::ParamValue::Float(m.clamp(0.0, 1.0)),
            );
        }
        let name = std::path::Path::new(&p.path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        effect.ui.label = Some(format!("響き: {name}"));
        let fx_id = effect.id.clone();
        cmds.push(match &track {
            Some(t) => Command::AddEffect {
                track: t.clone(),
                effect,
                index: None,
            },
            None => Command::AddMasterEffect {
                effect,
                index: None,
            },
        });
        let label = format!("畳み込みリバーブ「{name}」を足す");
        let command = glaux_core::Command::batch(label.clone(), cmds);
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        Ok(JsonText(json!({
            "fx_id": fx_id.to_string(),
            "asset": imported.id.to_string(),
            "seconds": imported.asset.frames as f64 / imported.asset.sample_rate.max(1) as f64,
            "entry_id": entry_id.to_string(),
            "project_version": m.project_version,
        })))
    }

    #[tool(
        description = "言葉で音を追い込む。トラックの内蔵エフェクトのつまみを、音色語(CLAP の辞書の語。英語か日本語)に\
        近づく方へ自動で動かす(CMA-ES。音を実際に鳴らして CLAP で聴き比べる。音量はそろえて比べるので、大きくするだけでは近づかない)。\
        使い方: 人間の「もっと暖かく・刺さらないように」を toward: [\"warm\"], away: [\"harsh\"] のような語に置き換え、\
        先に必要なエフェクト(eq・compressor・reverb・width など)を add_effect で足してから呼ぶ。params で動かすつまみを絞ると速く確実。\
        元のつまみから離れすぎないようにしてある。良くならなければ変えない。\
        返り値: changes(変えたつまみの前後)、score_before / score_after(近づけたい語 − 遠ざけたい語の近さ。z 値)、\
        toward / away(語ごとの前後)。分岐・合流のあるトラックと、音色語のモデル(CLAP)が無い環境では使えない。"
    )]
    async fn refine_by_words(
        &self,
        params: Parameters<RefineByWordsParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("refine_by_words");
        let p = params.0;
        let track_id = glaux_core::TrackId::parse(&p.track_id).map_err(|e| e.to_string())?;
        let (project, _) = self.handle.get_project().await?;
        let dir = self.handle.project_dir().await?;
        let outcome = tokio::task::spawn_blocking({
            let project = project.clone();
            let track_id = track_id.clone();
            move || {
                crate::words::refine_by_words(
                    &project,
                    std::path::Path::new(&dir),
                    &track_id,
                    &p.toward,
                    &p.away,
                    &p.params,
                    p.max_evals.unwrap_or(30),
                )
            }
        })
        .await
        .map_err(|e| e.to_string())??;
        let mut v = serde_json::to_value(&outcome).map_err(|e| e.to_string())?;
        if p.apply.unwrap_or(true) && !outcome.changes.is_empty() {
            let cmds: Vec<glaux_core::Command> = outcome
                .changes
                .iter()
                .filter_map(|c| {
                    Some(glaux_core::Command::SetParam {
                        track: track_id.clone(),
                        path: glaux_core::ParamPath::effect(
                            glaux_core::FxId::parse(&c.fx_id).ok()?,
                            c.param.clone(),
                        ),
                        value: glaux_core::ParamValue::Float(c.after),
                    })
                })
                .collect();
            let words: Vec<&str> = outcome.toward.iter().map(|w| w.ja.as_str()).collect();
            let name = project
                .track(&track_id)
                .map(|t| t.name.clone())
                .unwrap_or_default();
            let label = if words.is_empty() {
                format!("「{name}」のエフェクトを言葉で追い込む")
            } else {
                format!("「{name}」を「{}」に寄せる", words.join("・"))
            };
            let command = glaux_core::Command::batch(label.clone(), cmds);
            let author = self.author(&ctx);
            let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
            v["applied"] = json!(true);
            v["entry_id"] = json!(entry_id.to_string());
            v["project_version"] = json!(m.project_version);
        } else {
            v["applied"] = json!(false);
        }
        Ok(JsonText(v))
    }

    #[tool(
        description = "マスタリングの助手・参照曲に寄せる。曲を描き出して、マスターの最後に足す EQ → コンプ → (幅)→ リミッタ\
        のつまみを決め、既定ではそのまま足す(1 回の undo で戻せる。apply: false なら案だけ)。\
        reference_file があれば、その曲の音色の釣り合い(1/3 オクターブ)・左右の広がり・音量に寄せる。\
        無ければ曲自身の出っ張り・へこみだけを均し、target(spotify -14 / youtube -14 / apple -16 / loud -9)の音量にする。\
        True Peak は -1 dBTP 以下に保つ。曲が目標より大きければマスター音量で下げる。\
        返り値: effects(足したエフェクトとつまみ)、master_volume_db、before / after / reference(ラウドネス・True Peak・PLR・\
        傾き・広がり)、tonal_error_db(釣り合いの目標とのずれ。EQ の前と後)、notes(潰しすぎなどの注意)。\
        仕上げの確認は compare_mix(checkpoint)で。音を大きくしすぎない(配信は正規化される)。"
    )]
    async fn master_mix(
        &self,
        params: Parameters<MasterMixParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("master_mix");
        let p = params.0;
        let reference = p.reference_file.map(std::path::PathBuf::from);
        let target = match (p.target_lufs, p.target.as_deref()) {
            (Some(v), _) => glaux_engine::mastering::LoudnessTarget::Lufs(v.clamp(-30.0, -5.0)),
            (None, Some("spotify" | "youtube")) => {
                glaux_engine::mastering::LoudnessTarget::Lufs(-14.0)
            }
            (None, Some("apple")) => glaux_engine::mastering::LoudnessTarget::Lufs(-16.0),
            (None, Some("loud")) => glaux_engine::mastering::LoudnessTarget::Lufs(-9.0),
            (None, Some("reference")) if reference.is_some() => {
                glaux_engine::mastering::LoudnessTarget::Reference
            }
            (None, None) if reference.is_some() => {
                glaux_engine::mastering::LoudnessTarget::Reference
            }
            (None, None) => glaux_engine::mastering::LoudnessTarget::Lufs(-14.0),
            (None, Some(t)) => {
                return Err(format!(
                    "target「{t}」は分かりません(spotify / youtube / apple / loud / reference)"
                ))
            }
        };
        let (project, _) = self.handle.get_project().await?;
        let dir = self.handle.project_dir().await?;
        let before_volume = project.master.volume_db as f64;
        let plan = tokio::task::spawn_blocking({
            let project = project.clone();
            move || -> Result<glaux_engine::mastering::MasterPlan, String> {
                // 足すエフェクトは今のマスターのエフェクトの後、マスター音量の前に入るので、音量 0dB で描き出す
                let mut p = project;
                p.master.volume_db = 0.0;
                let bank = glaux_engine::SampleBank::for_offline(&p, std::path::Path::new(&dir));
                let mix = glaux_engine::render_project(&p, 48_000.0, &bank)
                    .map_err(|e| format!("曲を描き出せません: {e}"))?;
                let reference = match &reference {
                    Some(path) => {
                        let r = crate::sound::load_stereo_48k(path)?;
                        if r.len() < 48_000 * 2 * 3 {
                            return Err("参照曲が短すぎます(3 秒以上)".to_owned());
                        }
                        Some(r)
                    }
                    None => None,
                };
                if mix.len() < 48_000 * 2 {
                    return Err("曲が短すぎます(1 秒以上)".to_owned());
                }
                Ok(glaux_engine::mastering::plan_master(
                    &mix,
                    reference.as_deref(),
                    target,
                ))
            }
        })
        .await
        .map_err(|e| e.to_string())??;
        let mut v = serde_json::to_value(&plan).map_err(|e| e.to_string())?;
        if p.apply.unwrap_or(true) {
            let mut cmds: Vec<glaux_core::Command> = plan
                .effects
                .iter()
                .map(|e| glaux_core::Command::AddMasterEffect {
                    effect: e.to_effect(),
                    index: None,
                })
                .collect();
            let volume_changed = (plan.master_volume_db - before_volume).abs() > 0.05;
            if volume_changed {
                cmds.push(glaux_core::Command::SetMasterVolume {
                    volume_db: plan.master_volume_db as f32,
                });
            }
            let label = if v["reference"].is_null() {
                "マスタリング(音量と釣り合いを整える)".to_owned()
            } else {
                "マスタリング(参照曲に寄せる)".to_owned()
            };
            let command = glaux_core::Command::batch(label.clone(), cmds);
            let author = self.author(&ctx);
            let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
            v["applied"] = json!(true);
            v["entry_id"] = json!(entry_id.to_string());
            v["project_version"] = json!(m.project_version);
            if volume_changed {
                if let Some(notes) = v["notes"].as_array_mut() {
                    notes.push(json!(format!(
                        "マスター音量を {before_volume:.1} dB から {:.1} dB に変えた",
                        plan.master_volume_db
                    )));
                }
            }
        } else {
            v["applied"] = json!(false);
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
        ユーザーに、アプリの設定 → 表示 →「はじめの確認」の「GM 音源を取得」(GeneralUser GS)で入れられることを案内する。"
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
        description = "曲の作り方の工程・旋律・グルーブ・音作り・ジャンル・奏法・ミックス・音声素材・似た音作り・CLAP の定石を読む。\
        topic: workflow(曲を作る工程と点検表。曲を作るときは最初に読む)/ melody(旋律の工程・良い旋律の性質・ジャンルの語法)/ groove(グルーブの型・前ノリ後ノリ・ゴースト・オートメーションの形)/\
        instruments(音源の選び方・エレキギター・SoundFont)/ genres(テンポ・ドラムの型・進行・ベース・構成・EDM の音作り)/\
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
        description = "曲を WAV / FLAC に書き出す。format(wav / flac。flac は可逆圧縮で 16 / 24bit のみ)、sample_rate(44100 / 48000)、bits(16 / 24 / 32 = 浮動小数)、範囲(start_tick / end_tick)、\
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
        description = "曲を MusicXML(譜面ソフト MuseScore・Dorico などで開ける)に書き出す。MIDI トラックごとに 1 パート。\
        拍子の拍のまとまりは <beats>2+2+3</beats> で書け(SMF では書けない)、テンポ・調号(曲の音から推定)も入る。\
        位置と長さは 32 分の格子にそろえ、1 パートを 1 声部にする(同じ位置の音は和音、小節をまたぐ音はタイ)。\
        path 省略でプロジェクトの export/ に日時付きの名前。"
    )]
    async fn export_musicxml(&self, params: Parameters<ExportMidiParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("export_musicxml");
        let (project, _) = self.handle.get_project().await?;
        let dir = self.handle.project_dir().await?;
        let path = params.0.path.map(|p| {
            if p.ends_with(".mid") {
                p.trim_end_matches(".mid").to_owned() + ".musicxml"
            } else {
                p
            }
        });
        let v =
            crate::musicxml::export_file(&project, std::path::Path::new(&dir), path.as_deref())?;
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
        // 小節の中で回す(拍の置き換え)
        let wrap = if p.wrap_in_bar.unwrap_or(false) {
            let cid = glaux_core::ClipId::parse(&p.clip_id).map_err(|e| e.to_string())?;
            let (project, _) = self.handle.get_project().await?;
            let start = project.clip(&cid).map_or(0, |(_, c)| c.start.0);
            Some((
                start,
                glaux_core::meter::bar_meters(&project, start + len.0 + 1),
            ))
        } else {
            None
        };
        let changes: Vec<_> = notes
            .iter()
            .map(|n| {
                if let Some((start, meters)) = &wrap {
                    let abs = start + n.pos.0;
                    if let Some(m) = meters.iter().rev().find(|m| m.start <= abs) {
                        let rel = (abs - m.start) as i64;
                        let r = (rel + p.delta_ticks).rem_euclid(m.len.max(1) as i64);
                        let new = (m.start as i64 + r - *start as i64).clamp(0, max_pos) as u64;
                        return (n, new);
                    }
                }
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
        description = "ノートにグルーブの型を当てる(打ち込みの機械っぽさを取る)。人間のドラマーの演奏から集計した型\
        (funk / hiphop / soul / rock / pop / jazz / latin / neworleans / afrobeat)と電子音楽の手作りの型(house / techno / trap)で、\
        楽器ごと・16 分の位置ごとのずれ(バックビートが少し後ろ、など)と強弱(表が強く 16 分の裏が弱い、など)を付ける。\
        pocket_ms で楽器ごとの前ノリ・後ノリ、humanize_ms で小さな 1/f の揺れ(3〜8ms)。ドラムは音程で楽器を分け、\
        ほかのトラックは as_part(ベースは kick、コードの刻みは hat)。誇張は逆効果なので既定値から始める。\
        電子音楽の型では 4 つ打ちのキックは動かさない(格子へ寄せる quantize だけ効く)。\
        clip_ids で曲じゅうのクリップにまとめて当てる(クリップごとに揺れの列は変わる)。ループのクリップは中身に当たるので\
        揺れも毎回同じ。生演奏らしさが要るなら unroll_loop: true で繰り返しを書き出してから当てる。\
        スウィングは swing_notes を先に掛け、ここは quantize 0 で重ねる。同じ seed なら同じ結果。1 回の undo で戻る。\
        型の出典: Groove MIDI Dataset(Google Magenta、CC BY 4.0)を集計して改変。"
    )]
    async fn apply_groove(
        &self,
        params: Parameters<ApplyGrooveParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("apply_groove");
        let p = params.0;
        let style = glaux_core::groove::style(&p.style).ok_or_else(|| {
            let names: Vec<_> = glaux_core::groove::styles().iter().map(|s| s.0).collect();
            format!(
                "style が不明です({})。使えるもの: {}",
                p.style,
                names.join(" / ")
            )
        })?;
        let mut ids: Vec<String> = p.clip_id.clone().into_iter().collect();
        for c in p.clip_ids.iter().flatten() {
            if !ids.contains(c) {
                ids.push(c.clone());
            }
        }
        if ids.is_empty() {
            return Err("clip_id か clip_ids を指定してください".to_owned());
        }
        if p.note_ids.is_some() && ids.len() > 1 {
            return Err("note_ids は clip_id を 1 つだけ指定したときに使えます".to_owned());
        }
        let unroll = p.unroll_loop.unwrap_or(false);
        let humanize = p.humanize_ms.unwrap_or(0.0).clamp(0.0, 20.0);
        let (project, version) = self.handle.get_project().await?;
        let mut commands = Vec::new();
        let mut clips_out = Vec::new();
        let mut total = 0usize;
        let mut shift_sum = 0.0;
        let mut shift_n = 0usize;
        let mut looped_kept = Vec::new();
        for (i, id) in ids.iter().enumerate() {
            let (clip_id, _, selected, _) = self.load_notes(id, &p.note_ids).await?;
            let (track, clip) = project.clip(&clip_id).ok_or("clip not found")?;
            let start = clip.start.0;
            let len = clip.length.0;
            // ループはほどいてから(繰り返しごとに違う揺れ)か、中身にそのまま当てる
            let mut unrolled_clip = None;
            let notes = match glaux_core::arrange::unroll_loop(clip) {
                Some(flat) if unroll => {
                    let wanted: Option<std::collections::HashSet<_>> = p
                        .note_ids
                        .as_ref()
                        .map(|_| selected.iter().map(|n| n.id.clone()).collect());
                    let notes: Vec<_> = flat
                        .notes()
                        .unwrap_or_default()
                        .iter()
                        .filter(|n| wanted.as_ref().is_none_or(|w| w.contains(&n.id)))
                        .cloned()
                        .collect();
                    unrolled_clip = Some(flat);
                    notes
                }
                Some(_) => {
                    looped_kept.push(clip_id.to_string());
                    selected
                }
                None => selected,
            };
            // ms → tick(クリップの頭のテンポで)
            let ticks_per_ms =
                glaux_core::PPQ as f64 * project.tempo_map.bpm_at(clip.start) / 60_000.0;
            let pocket = p
                .pocket_ms
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(|(k, v)| (k, v.clamp(-30.0, 30.0) * ticks_per_ms))
                .collect();
            let as_part = match &p.as_part {
                Some(x) => Some(x.clone()),
                None if is_drum_track(track) => None,
                None => Some("hat".to_owned()),
            };
            let opts = glaux_core::groove::GrooveOptions {
                quantize: p.quantize.unwrap_or(0.0),
                timing: p.timing.unwrap_or(1.0),
                velocity: p.velocity.unwrap_or(0.7),
                humanize_ticks: humanize * ticks_per_ms,
                pocket_ticks: pocket,
                as_part,
                // クリップごとに揺れの列を変える(同じ型のクリップが並んでも同じ揺れにならない)
                seed: p.seed.unwrap_or(1).wrapping_add(i as u64 * 7919),
            };
            let meters = glaux_core::meter::bar_meters(&project, start + len + 3840);
            let meter_of = |t: u64| {
                let i = meters.partition_point(|m| m.start <= t).saturating_sub(1);
                meters
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| glaux_core::meter::BarMeter::common(0))
            };
            let edits = glaux_core::groove::apply(&notes, start, len, style, &opts, &meter_of);
            for e in &edits {
                if let Some(n) = notes.iter().find(|n| n.id == e.id) {
                    shift_sum += (e.pos as f64 - n.pos.0 as f64).abs() / ticks_per_ms;
                    shift_n += 1;
                }
            }
            let changed = edits.len();
            total += changed;
            // 変わる音が無ければループはほどかない(ほどくだけの編集を残さない)
            let unrolled = changed > 0 && unrolled_clip.is_some();
            if let Some(flat) = unrolled_clip.filter(|_| unrolled) {
                commands.push(Command::ReplaceClip {
                    id: clip_id.clone(),
                    clip: flat,
                });
            }
            clips_out.push(json!({ "clip_id": clip_id, "changed": changed, "unrolled": unrolled }));
            if changed > 0 {
                commands.push(Command::UpdateNotes {
                    clip: clip_id,
                    changes: edits
                        .into_iter()
                        .map(|e| {
                            glaux_core::NoteChange::new(e.id)
                                .pos(glaux_core::Tick(e.pos))
                                .vel(e.vel)
                        })
                        .collect(),
                });
            }
        }
        let style_json = json!({ "name": p.style, "from_dataset": !style.handmade, "bpm": style.bpm,
                "locked": style.locked });
        if commands.is_empty() {
            return Ok(JsonText(json!({
                "project_version": version,
                "changed": 0,
                "note": "対象ノートはすべて変更不要でした",
                "style": style_json,
            })));
        }
        let label = if ids.len() == 1 {
            format!("{} のグルーブ({total} ノート)", p.style)
        } else {
            format!(
                "{} のグルーブ({} クリップ・{total} ノート)",
                p.style,
                ids.len()
            )
        };
        let command = if commands.len() == 1 {
            commands.pop().expect("1 つある")
        } else {
            Command::batch(label.clone(), commands)
        };
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["changed"] = json!(total);
        v["clips"] = json!(clips_out);
        v["mean_shift_ms"] = json!(if shift_n == 0 {
            0.0
        } else {
            (shift_sum / shift_n as f64 * 10.0).round() / 10.0
        });
        v["style"] = style_json;
        if !looped_kept.is_empty() && humanize > 0.0 {
            v["note"] = json!(format!(
                "ループのクリップ({})はループの中身に当てたので、揺れも毎回同じです。繰り返しごとに変えるなら unroll_loop: true",
                looped_kept.join(", ")
            ));
        }
        Ok(JsonText(v))
    }

    #[tool(
        description = "ドラムのクリップにゴーストノート(16 分の裏に入るごく弱いスネア)を足す。ファンク・ソウル・R&B・ヒップホップの\
        推進力になる。型(style)のゴーストの出やすい位置に倣い、バックビート(2・4 拍)と、同じ音程の音の近くは避ける。\
        density 0.3〜0.7 から。足した後に apply_groove を掛けると強弱とずれもそろう。同じ seed なら同じ結果。1 回の undo で戻る。"
    )]
    async fn add_ghost_notes(
        &self,
        params: Parameters<AddGhostNotesParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("add_ghost_notes");
        let p = params.0;
        let name = p.style.unwrap_or_else(|| "funk".to_owned());
        let style =
            glaux_core::groove::style(&name).ok_or_else(|| format!("style が不明です({name})"))?;
        let (clip_id, len, notes, version) = self.load_notes(&p.clip_id, &None).await?;
        let (project, _) = self.handle.get_project().await?;
        let (_, clip) = project.clip(&clip_id).ok_or("clip not found")?;
        let start = clip.start.0;
        let pitch = p.pitch.unwrap_or(38);
        let bar_of = bar_start_fn(&project, start + len.0);
        let pos = glaux_core::groove::ghost_positions(
            &notes,
            pitch,
            start,
            len.0,
            style,
            p.density.unwrap_or(0.5).clamp(0.0, 2.0),
            p.velocity.map(|v| v.clamp(1, 60)),
            p.seed.unwrap_or(1),
            &bar_of,
        );
        if pos.is_empty() {
            return Ok(JsonText(json!({ "project_version": version, "added": 0,
                "note": "足せる位置がありませんでした(density を上げるか、別の style を試す)" })));
        }
        let added = pos.len();
        let new_notes: Vec<glaux_core::Note> = pos
            .into_iter()
            .map(|(t, v)| glaux_core::Note {
                id: glaux_core::NoteId::new(),
                pos: glaux_core::Tick(t),
                dur: glaux_core::Tick(120),
                pitch,
                vel: v,
                articulation: Default::default(),
                pitch_curve: vec![],
                glide_ms: None,
                vibrato: None,
                volume_curve: vec![],
                brightness_curve: vec![],
            })
            .collect();
        let command = Command::AddNotes {
            clip: clip_id,
            notes: new_notes,
        };
        let label = format!("ゴーストノートを {added} 個({name} の置き方)");
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["added"] = json!(added);
        Ok(JsonText(v))
    }

    #[tool(
        description = "旋律を変形する(動機を展開して曲に統一感を出す作曲の技法を、音程を計算して正確に)。\
        op: invert(反行)/ retrograde(逆行)/ transpose(音階の度数で移調。steps: 1 = 2 度上)/ \
        sequence(反復進行。steps 度ずつずらして times 回、後ろへ写す。新しいノートを作る)/ stretch(factor 倍に拡大・縮小)。\
        音階は key(\"A minor\" など。省略で曲から推定、\"chromatic\" で半音)に沿い、臨時記号の音はそのずれを保つ。\
        note_ids で動機だけを選ぶ。クリップの長さを越えるときは先に resize_clip。1 回の undo で戻る。"
    )]
    async fn transform_notes(
        &self,
        params: Parameters<TransformNotesParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        use glaux_core::transform::{transform, Op};
        let _activity = self.handle.begin_activity("transform_notes");
        let p = params.0;
        let op = match p.op.as_str() {
            "invert" => Op::Invert { axis: p.axis },
            "retrograde" => Op::Retrograde,
            "transpose" => Op::TransposeDiatonic {
                steps: p.steps.ok_or("transpose には steps が要ります")?,
            },
            "sequence" => Op::Sequence {
                steps: p.steps.unwrap_or(0),
                times: p.times.unwrap_or(1),
                offset: p
                    .offset_beats
                    .map(|b| (b.max(0.0) * glaux_core::PPQ as f64).round() as u64),
            },
            "stretch" => Op::Stretch {
                factor: p.factor.ok_or("stretch には factor が要ります")?,
            },
            other => {
                return Err(format!(
                    "op が不正です({other})。invert / retrograde / transpose / sequence / stretch"
                ))
            }
        };
        let (clip_id, len, notes, version) = self.load_notes(&p.clip_id, &p.note_ids).await?;
        let (project, _) = self.handle.get_project().await?;
        let (scale, key_name) = match &p.key {
            Some(k) => (parse_key(k)?, k.clone()),
            None => match glaux_core::harmony::analyze(&project, None, None).key {
                Some(k) => (
                    glaux_core::transform::Scale::new(glaux_core::harmony::scale_pitch_classes(
                        k.tonic, k.mode,
                    )),
                    format!("{}(推定)", k.name),
                ),
                None => (
                    glaux_core::transform::Scale::chromatic(),
                    "chromatic".to_owned(),
                ),
            },
        };
        let r = transform(&notes, len.0, &op, &scale, &mut glaux_core::NoteId::new)?;
        let n_changed = r.changes.len();
        let n_added = r.added.len();
        if n_changed == 0 && n_added == 0 {
            return Ok(JsonText(
                json!({ "project_version": version, "changed": 0, "note": "変わる音はありませんでした" }),
            ));
        }
        let mut cmds = Vec::new();
        if n_changed > 0 {
            cmds.push(Command::UpdateNotes {
                clip: clip_id.clone(),
                changes: r
                    .changes
                    .into_iter()
                    .map(|(id, pos, dur, pitch)| {
                        glaux_core::NoteChange::new(id)
                            .pos(glaux_core::Tick(pos))
                            .dur(glaux_core::Tick(dur))
                            .pitch(pitch)
                    })
                    .collect(),
            });
        }
        if n_added > 0 {
            cmds.push(Command::AddNotes {
                clip: clip_id,
                notes: r.added,
            });
        }
        let label = format!("旋律の変形 {}({key_name})", p.op);
        let command = if cmds.len() == 1 {
            cmds.pop().expect("1 件")
        } else {
            Command::batch(label.clone(), cmds)
        };
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["changed"] = json!(n_changed);
        v["added"] = json!(n_added);
        v["key"] = json!(key_name);
        Ok(JsonText(v))
    }

    #[tool(
        description = "コード進行から伴奏を書く(和音の積み方と声部のつながりを計算して、指定のリズムで置く)。\
        進行は記号(Am7 / G7(b9) / C/E)か、key を付けてローマ数字(IVmaj7 V7 iii7 vi)。`|` で小節、空白で小節内を均等に分ける。\
        積み方(style: close / open / drop2 / drop3 / spread / shell / rootless)と声の数・音域に沿って、各声部の動きが最も小さく、\
        平行 5 度・8 度を避け、低音域で濁らない並びを選ぶ。top で一番上の声部の高さをそろえる。リズムは名前\
        (sustain / whole / half / quarter / eighth / offbeat / charleston / backbeat / syncopated)か \"x..x-...\" の文字列。\
        トラックに新しいクリップを作る(返り値 clip_id)。返り値の chords に各和音の実際の音。ずれ・強弱は後から apply_groove。\
        1 回の undo で戻る。"
    )]
    async fn write_chords(
        &self,
        params: Parameters<WriteChordsParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        use glaux_core::{chord, comp, voicing};
        let _activity = self.handle.begin_activity("write_chords");
        let p = params.0;
        let tid = glaux_core::TrackId::parse(&p.track_id).map_err(|e| e.to_string())?;
        let (project, _) = self.handle.get_project().await?;
        let track = project.track(&tid).ok_or("トラックが見つかりません")?;
        if track.kind != glaux_core::TrackKind::Midi {
            return Err("MIDI トラックを指定してください".to_owned());
        }
        let rhythm = comp::parse_rhythm(p.rhythm.as_deref().unwrap_or("sustain"))?;
        let style = match &p.style {
            Some(s) => voicing::Style::parse(s).ok_or_else(|| {
                format!(
                    "style は close / open / drop2 / drop3 / spread / shell / rootless(got: {s})"
                )
            })?,
            None => voicing::Style::Close,
        };
        // シェルは左手の音域(上の声部 D3〜G4、低音 E2〜G3)を既定に
        let (low, high) = match (&p.range, style) {
            (None, voicing::Style::Shell) => (50, 67),
            _ => parse_range(p.range.as_deref())?,
        };
        let top = match &p.top {
            Some(t) => {
                Some(chord::parse_note(t).ok_or_else(|| format!("top の音名が読めません: {t}"))?)
            }
            None => None,
        };
        let Layout {
            chords: chords_list,
            spans,
            span_bar,
            bar_list,
            clip_start,
            clip_len,
            n_bars,
        } = progression_layout(
            &project,
            &p.chords,
            p.key.as_deref(),
            p.repeat,
            p.bar,
            rhythm.is_none(),
        )?;
        let voices = p.voices.map_or(4, |v| v as usize);
        let opts = voicing::Options {
            style,
            voices,
            low,
            high,
            top,
            bass: (p.bass.unwrap_or(false) || style == voicing::Style::Shell).then_some(
                if style == voicing::Style::Shell {
                    (40, 55)
                } else {
                    (36, 52)
                },
            ),
        };
        let voiced = voicing::voice_progression(&chords_list, &opts)?;
        let notes_per_chord: Vec<Vec<u8>> = voiced
            .iter()
            .map(|v| v.bass.into_iter().chain(v.upper.iter().copied()).collect())
            .collect();
        let ticks_per_ms = glaux_core::PPQ as f64
            * project.tempo_map.bpm_at(glaux_core::Tick(clip_start))
            / 60_000.0;
        let comp_opts = comp::CompOptions {
            gate: p.gate.unwrap_or(0.9).clamp(0.1, 1.0),
            velocity: p.velocity.unwrap_or(88).clamp(1, 127),
            strum_ticks: (p.strum_ms.unwrap_or(0.0).clamp(0.0, 80.0) * ticks_per_ms).round() as u64,
        };
        let articulation: glaux_core::Articulation = match &p.articulation {
            Some(a) => serde_json::from_value(json!(a)).map_err(|_| {
                format!(
                    "articulation は palm_mute / staccato / accent / legato / portamento / vibrato / bend / normal(got: {a})"
                )
            })?,
            None => Default::default(),
        };
        let meters = bar_meters_in(&project, clip_start, clip_len);
        let rendered = comp::render(
            &spans,
            &notes_per_chord,
            rhythm.as_deref(),
            &bar_list,
            &meters,
            &comp_opts,
        );
        let mut clip = glaux_core::Clip::new_midi(
            glaux_core::ClipId::new(),
            p.name.clone().unwrap_or_else(|| "Chords".to_owned()),
            glaux_core::Tick(clip_start),
            glaux_core::Tick(clip_len),
        );
        let clip_id = clip.id.clone();
        if let Some(ns) = clip.notes_mut() {
            *ns = rendered
                .iter()
                .map(|n| glaux_core::Note {
                    id: glaux_core::NoteId::new(),
                    pos: glaux_core::Tick(n.pos),
                    dur: glaux_core::Tick(n.dur),
                    pitch: n.pitch,
                    vel: n.vel,
                    articulation,
                    pitch_curve: vec![],
                    glide_ms: None,
                    vibrato: None,
                    volume_curve: vec![],
                    brightness_curve: vec![],
                })
                .collect();
            ns.sort_by(|a, b| (a.pos, a.pitch, &a.id).cmp(&(b.pos, b.pitch, &b.id)));
        }
        // 返り値: 各和音の実際の音
        let mut summary = Vec::new();
        let mut motion = 0i32;
        let mut changes = 0i32;
        for (si, s) in spans.iter().enumerate() {
            let Some(ci) = s.chord else { continue };
            let v = &voiced[ci];
            if ci > 0 {
                let prev = &voiced[ci - 1];
                motion += prev
                    .upper
                    .iter()
                    .zip(&v.upper)
                    .map(|(a, b)| (*a as i32 - *b as i32).abs())
                    .sum::<i32>();
                changes += 1;
            }
            summary.push(json!({
                "bar": span_bar[si],
                "chord": chords_list[ci].name,
                "notes": v.upper.iter().map(|p| chord::note_name(*p)).collect::<Vec<_>>(),
                "bass": v.bass.map(chord::note_name),
            }));
        }
        let label = format!("コード進行({} 小節・{} 和音)", n_bars, chords_list.len());
        let command = Command::AddClip { track: tid, clip };
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut out = mutated_json(&m);
        out["entry_id"] = json!(entry_id);
        out["clip_id"] = json!(clip_id);
        out["bars"] = json!(n_bars);
        out["notes"] = json!(rendered.len());
        out["chords"] = json!(summary);
        out["mean_motion"] = json!(if changes == 0 {
            0.0
        } else {
            (motion as f64 / changes as f64 * 10.0).round() / 10.0
        });
        Ok(JsonText(out))
    }

    #[tool(
        description = "コード進行からベースラインを書く。根音(分数コードは最低音)のオクターブは進行全体で動きが小さく選ぶ。\
        型: root / root8 / offbeat / octave / tresillo / 808(伸ばして音が変わる所で滑らせる)/ funk / walking\
        (根音 → 和音の音 → 次の根音へ半音で近づく)か、x o 5 3 7 - . の文字列で度数とリズムを書く。\
        approach で区間の最後に次の根音へ近づく経過音、follow_kick でキックと同じ位置に置く(かみ合わせ)。\
        進行は write_chords と同じ書き方。トラックに新しいクリップを作る(返り値 clip_id)。ずれ・強弱は後から\
        apply_groove(as_part: kick)。1 回の undo で戻る。"
    )]
    async fn write_bassline(
        &self,
        params: Parameters<WriteBasslineParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        use glaux_core::{bassline, chord};
        let _activity = self.handle.begin_activity("write_bassline");
        let p = params.0;
        let tid = glaux_core::TrackId::parse(&p.track_id).map_err(|e| e.to_string())?;
        let (project, _) = self.handle.get_project().await?;
        let track = project.track(&tid).ok_or("トラックが見つかりません")?;
        if track.kind != glaux_core::TrackKind::Midi {
            return Err("MIDI トラックを指定してください".to_owned());
        }
        let (pattern, slide) = bassline::parse_pattern(p.pattern.as_deref().unwrap_or("root8"))?;
        let Layout {
            chords,
            spans,
            span_bar,
            bar_list,
            clip_start,
            clip_len,
            n_bars,
        } = progression_layout(
            &project,
            &p.chords,
            p.key.as_deref(),
            p.repeat,
            p.bar,
            pattern == bassline::Pattern::Sustain,
        )?;
        let (low, high) = parse_range(Some(p.range.as_deref().unwrap_or("E1-E3")))?;
        let approach = match p.approach.as_deref().unwrap_or("none") {
            "none" => bassline::Approach::None,
            "chromatic" => bassline::Approach::Chromatic,
            "scale" => bassline::Approach::Scale,
            other => {
                return Err(format!(
                    "approach は none / chromatic / scale(got: {other})"
                ))
            }
        };
        let scale = p.key.as_deref().and_then(chord::Key::parse).map(|k| {
            glaux_core::harmony::scale_pitch_classes(
                k.tonic,
                if k.minor { "minor" } else { "major" },
            )
        });
        if approach == bassline::Approach::Scale && scale.is_none() {
            return Err("approach: \"scale\" には key が要ります".to_owned());
        }
        // キックの位置(クリップの頭から)
        let kick: Option<Vec<u64>> = match &p.follow_kick {
            Some(k) => {
                let kid = glaux_core::ClipId::parse(k).map_err(|e| e.to_string())?;
                let (_, kc) = project
                    .clip(&kid)
                    .ok_or("キックのクリップが見つかりません")?;
                let end = clip_start + clip_len;
                let v: Vec<u64> = kc
                    .playback_notes()
                    .into_iter()
                    .filter(|n| matches!(n.pitch, 35 | 36))
                    .map(|n| kc.start.0 + n.pos.0)
                    .filter(|t| (clip_start..end).contains(t))
                    .map(|t| t - clip_start)
                    .collect();
                if v.is_empty() {
                    return Err(
                        "キックのクリップに、この範囲のキック(36 / 35)がありません".to_owned()
                    );
                }
                Some(v)
            }
            None => None,
        };
        let opts = bassline::Options {
            low,
            high,
            gate: p.gate.unwrap_or(0.85).clamp(0.1, 1.0),
            velocity: p.velocity.unwrap_or(96).clamp(1, 127),
            approach,
            scale,
            slide,
            beat: glaux_core::PPQ,
        };
        let meters = bar_meters_in(&project, clip_start, clip_len);
        let notes = bassline::render(
            &spans,
            &chords,
            &bar_list,
            &meters,
            &pattern,
            &opts,
            kick.as_deref(),
        )?;
        let glide = p.glide_ms.unwrap_or(60.0).clamp(5.0, 500.0);
        let mut clip = glaux_core::Clip::new_midi(
            glaux_core::ClipId::new(),
            p.name.clone().unwrap_or_else(|| "Bass".to_owned()),
            glaux_core::Tick(clip_start),
            glaux_core::Tick(clip_len),
        );
        let clip_id = clip.id.clone();
        if let Some(ns) = clip.notes_mut() {
            *ns = notes
                .iter()
                .map(|n| glaux_core::Note {
                    id: glaux_core::NoteId::new(),
                    pos: glaux_core::Tick(n.pos),
                    dur: glaux_core::Tick(n.dur),
                    pitch: n.pitch,
                    vel: n.vel,
                    articulation: if n.slide {
                        glaux_core::Articulation::Portamento
                    } else {
                        Default::default()
                    },
                    pitch_curve: vec![],
                    glide_ms: n.slide.then_some(glide),
                    vibrato: None,
                    volume_curve: vec![],
                    brightness_curve: vec![],
                })
                .collect();
            ns.sort_by(|a, b| (a.pos, a.pitch, &a.id).cmp(&(b.pos, b.pitch, &b.id)));
        }
        let summary: Vec<Value> = spans
            .iter()
            .zip(&span_bar)
            .filter_map(|(s, bar)| {
                let ci = s.chord?;
                let first = notes.iter().find(|n| n.pos >= s.start && n.pos < s.start + s.len)?;
                Some(json!({ "bar": bar, "chord": chords[ci].name, "root": chord::note_name(first.pitch) }))
            })
            .collect();
        let label = format!("ベースライン({n_bars} 小節)");
        let command = Command::AddClip { track: tid, clip };
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut out = mutated_json(&m);
        out["entry_id"] = json!(entry_id);
        out["clip_id"] = json!(clip_id);
        out["bars"] = json!(n_bars);
        out["notes"] = json!(notes.len());
        out["chords"] = json!(summary);
        Ok(JsonText(out))
    }

    #[tool(
        description = "ドラムの型を置く。ジャンルの型(house / techno / trap / hiphop / lofi / funk / rock / pop / dnb / disco / \
        reggaeton / halftime / 2step)を、音の多さ(intensity: ハットの細かさ)・小節ごとの変化(variation)・フィル\
        (fill_every 小節ごとの最後の 3・4 拍にスネアかタム)・クラッシュ(区切りの頭)と一緒に置く。ビルドは build_bars\
        (スネアのロールがだんだん細かく強く)と gap_beats(直前の無音)。区間ごとに呼び分けると区間の差になる\
        (イントロは intensity 0.3、山は 0.9 など)。音程は GM(36 キック・38 スネア・39 クラップ・42 / 46 ハット・49 クラッシュ)。\
        変拍子(7/8 など)は拍子の拍のまとまり(time_sig_map の grouping)の頭にキックとスネアを置く(odd_meter)。\
        ドラムのトラックに新しいクリップを作る(返り値 clip_id)。ずれ・強弱は後から apply_groove。1 回の undo で戻る。"
    )]
    async fn write_drums(
        &self,
        params: Parameters<WriteDrumsParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        use glaux_core::drums;
        let _activity = self.handle.begin_activity("write_drums");
        let p = params.0;
        let tid = glaux_core::TrackId::parse(&p.track_id).map_err(|e| e.to_string())?;
        let (project, _) = self.handle.get_project().await?;
        let track = project.track(&tid).ok_or("トラックが見つかりません")?;
        if !is_drum_track(track) {
            return Err("ドラムのトラック(音源が内蔵の drum か、SoundFont のドラム(bank 128))を指定してください".to_owned());
        }
        let style = drums::style(&p.style).ok_or_else(|| {
            format!(
                "style が不明です({})。使えるもの: {}",
                p.style,
                drums::STYLES
                    .iter()
                    .map(|s| s.name)
                    .collect::<Vec<_>>()
                    .join(" / ")
            )
        })?;
        if p.bars == 0 || p.bars > 512 {
            return Err("bars は 1〜512".to_owned());
        }
        let fill = match p.fill.as_deref().unwrap_or("snare") {
            "snare" => drums::Fill::Snare,
            "toms" => drums::Fill::Toms,
            "none" => drums::Fill::None,
            other => return Err(format!("fill は snare / toms / none(got: {other})")),
        };
        let first_bar = p.bar.unwrap_or(1).max(1);
        let (clip_start, clip_len) = glaux_core::arrange::bar_range(&project, first_bar, p.bars)
            .ok_or("小節を数えられません")?;
        let bar_list: Vec<(u64, u64)> = (0..p.bars)
            .map(|i| {
                glaux_core::arrange::bar_range(&project, first_bar + i, 1)
                    .map(|(s, l)| (s - clip_start, l))
                    .ok_or("小節を数えられません")
            })
            .collect::<Result<_, _>>()?;
        let opts = drums::Options {
            intensity: p.intensity.unwrap_or(0.6).clamp(0.0, 1.0),
            variation: p.variation.unwrap_or(0.3).clamp(0.0, 1.0),
            fill,
            fill_every: p.fill_every.unwrap_or(8),
            crash: p.crash.unwrap_or(true),
            build_bars: p.build_bars.unwrap_or(0).min(p.bars),
            gap_beats: p.gap_beats.unwrap_or(0).min(8),
            seed: p.seed.unwrap_or(1),
            odd_meter: match p.odd_meter.as_deref() {
                None => drums::OddMeter::Auto,
                Some(s) => drums::OddMeter::parse(s).ok_or_else(|| {
                    format!("odd_meter は auto / group / cut / stretch(got: {s})")
                })?,
            },
        };
        let meters = bar_meters_in(&project, clip_start, clip_len);
        let mut hits = drums::render(style, &bar_list, &meters, &opts);
        if let Some(parts) = &p.parts {
            for part in parts {
                if !["kick", "snare", "hat", "tom", "cymbal", "perc"].contains(&part.as_str()) {
                    return Err(format!(
                        "parts は kick / snare / hat / tom / cymbal / perc(got: {part})"
                    ));
                }
            }
            hits.retain(|h| parts.iter().any(|x| x == drum_part_of(h.pitch)));
            if hits.is_empty() {
                return Err("その parts の音がこの型にはありません".to_owned());
            }
        }
        let mut clip = glaux_core::Clip::new_midi(
            glaux_core::ClipId::new(),
            p.name.clone().unwrap_or_else(|| "Drums".to_owned()),
            glaux_core::Tick(clip_start),
            glaux_core::Tick(clip_len),
        );
        let clip_id = clip.id.clone();
        if let Some(ns) = clip.notes_mut() {
            *ns = hits
                .iter()
                .map(|h| glaux_core::Note {
                    id: glaux_core::NoteId::new(),
                    pos: glaux_core::Tick(h.pos),
                    dur: glaux_core::Tick(h.dur),
                    pitch: h.pitch,
                    vel: h.vel,
                    articulation: Default::default(),
                    pitch_curve: vec![],
                    glide_ms: None,
                    vibrato: None,
                    volume_curve: vec![],
                    brightness_curve: vec![],
                })
                .collect();
            ns.sort_by(|a, b| (a.pos, a.pitch, &a.id).cmp(&(b.pos, b.pitch, &b.id)));
        }
        let fills: Vec<u32> = if fill == drums::Fill::None || opts.fill_every == 0 {
            vec![]
        } else {
            (1..=p.bars)
                .filter(|b| b % opts.fill_every == 0 && *b <= p.bars - opts.build_bars)
                .map(|b| first_bar + b - 1)
                .collect()
        };
        let label = format!("ドラム({} の型・{} 小節)", style.name, p.bars);
        let command = Command::AddClip { track: tid, clip };
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut out = mutated_json(&m);
        out["entry_id"] = json!(entry_id);
        out["clip_id"] = json!(clip_id);
        out["bars"] = json!(p.bars);
        out["notes"] = json!(hits.len());
        out["style"] = json!({ "name": style.name, "bpm": style.bpm, "note": style.note });
        out["fill_bars"] = json!(fills);
        Ok(JsonText(out))
    }

    #[tool(
        description = "区間のつなぎを置く(ドロップ・サビ・区間の頭に向けて)。to に区間の名前か小節番号。\
        gap_beats で区切りの直前を全トラック無音にする(その範囲で始まる音を消し、かかる音は手前で切る。keep_tracks は除く)。\
        drum_track_id を渡すと、区切りでちょうど鳴り終わるリバースクラッシュ・roll_bars 小節のスネアのロール\
        (だんだん細かく強く)・区切りのクラッシュとキックを新しいクリップで置く。フィルタを開くライザーは shape_automation。\
        1 回の undo で戻る。"
    )]
    async fn write_transition(
        &self,
        params: Parameters<WriteTransitionParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("write_transition");
        let p = params.0;
        let (project, _) = self.handle.get_project().await?;
        // 区切りの小節
        let to = p.to.trim();
        let bar: u32 = match to.parse::<u32>() {
            Ok(b) if b >= 1 => b,
            _ => {
                let lower = to.to_lowercase();
                let sec = project
                    .sections
                    .iter()
                    .find(|s| s.name.to_lowercase() == lower)
                    .or_else(|| {
                        project
                            .sections
                            .iter()
                            .find(|s| s.name.to_lowercase().contains(&lower))
                    })
                    .ok_or_else(|| {
                        format!(
                            "区間「{to}」が見つかりません(ある区間: {})",
                            project
                                .sections
                                .iter()
                                .map(|s| s.name.as_str())
                                .collect::<Vec<_>>()
                                .join(" / ")
                        )
                    })?;
                let grid = glaux_core::arrange::bar_grid(&project, sec.tick.0 + 1);
                grid.partition_point(|(s, _)| *s <= sec.tick.0) as u32
            }
        };
        let (boundary, _) =
            glaux_core::arrange::bar_range(&project, bar, 1).ok_or("小節を数えられません")?;
        let beat_ticks = |beats: f64| (beats * glaux_core::PPQ as f64).round() as u64;
        let gap = beat_ticks(p.gap_beats.unwrap_or(0.0).clamp(0.0, 16.0));
        let gap_start = boundary.saturating_sub(gap);
        let mut commands = Vec::new();
        let mut silenced = 0usize;
        let mut skipped_audio = Vec::new();
        if gap > 0 {
            let keep_names: Vec<String> = p
                .keep_tracks
                .iter()
                .flatten()
                .map(|n| n.trim().to_lowercase())
                .collect();
            let keep: Vec<glaux_core::TrackId> = project
                .tracks
                .iter()
                .filter(|t| keep_names.contains(&t.name.to_lowercase()))
                .map(|t| t.id.clone())
                .collect();
            let (cmds, n, audio) =
                glaux_core::arrange::silence_range(&project, gap_start, boundary, &keep);
            commands.extend(cmds);
            silenced = n;
            skipped_audio = audio;
        }
        // ドラムのつなぎ
        let mut drum_clip_id = None;
        if let Some(did) = &p.drum_track_id {
            let tid = glaux_core::TrackId::parse(did).map_err(|e| e.to_string())?;
            let track = project
                .track(&tid)
                .ok_or("ドラムのトラックが見つかりません")?;
            if !is_drum_track(track) {
                return Err("drum_track_id にはドラムのトラックを指定してください".to_owned());
            }
            let roll_bars = p.roll_bars.unwrap_or(0).min(8);
            let first_bar = bar.saturating_sub(roll_bars.max(1)).max(1);
            let (clip_start, _) = glaux_core::arrange::bar_range(&project, first_bar, 1)
                .ok_or("小節を数えられません")?;
            let (_, after_len) =
                glaux_core::arrange::bar_range(&project, bar, 1).ok_or("小節を数えられません")?;
            let mut hits: Vec<glaux_core::drums::Hit> = Vec::new();
            if roll_bars > 0 {
                let rel: Vec<(u64, u64)> = (0..roll_bars)
                    .filter_map(|i| {
                        glaux_core::arrange::bar_range(&project, bar - roll_bars + i, 1)
                            .map(|(s, l)| (s - clip_start, l))
                    })
                    .collect();
                hits.extend(
                    glaux_core::drums::snare_roll(&rel)
                        .into_iter()
                        .filter(|h| clip_start + h.pos < gap_start),
                );
            }
            if p.reverse_crash.unwrap_or(true) {
                // 1.6 秒 × ドラムの減衰の倍率で鳴り終わる
                let decay = match track.device.as_ref().and_then(|d| d.params.get("decay")) {
                    Some(glaux_core::ParamValue::Float(v)) => *v,
                    _ => 1.0,
                };
                let secs = 1.6 * decay.clamp(0.25, 4.0);
                let bpm = project.tempo_map.bpm_at(glaux_core::Tick(gap_start));
                let len = (secs * bpm / 60.0 * glaux_core::PPQ as f64).round() as u64;
                let pos = gap_start.saturating_sub(len).max(clip_start);
                hits.push(glaux_core::drums::Hit {
                    pos: pos - clip_start,
                    dur: gap_start - pos,
                    pitch: 55,
                    vel: 100,
                });
            }
            if p.crash.unwrap_or(true) {
                for (pitch, vel) in [(49u8, 110u8), (36, 115)] {
                    hits.push(glaux_core::drums::Hit {
                        pos: boundary - clip_start,
                        dur: glaux_core::PPQ,
                        pitch,
                        vel,
                    });
                }
            }
            if !hits.is_empty() {
                let mut clip = glaux_core::Clip::new_midi(
                    glaux_core::ClipId::new(),
                    "つなぎ",
                    glaux_core::Tick(clip_start),
                    glaux_core::Tick(boundary + after_len - clip_start),
                );
                if let Some(ns) = clip.notes_mut() {
                    *ns = hits
                        .iter()
                        .map(|h| glaux_core::Note {
                            id: glaux_core::NoteId::new(),
                            pos: glaux_core::Tick(h.pos),
                            dur: glaux_core::Tick(h.dur.max(1)),
                            pitch: h.pitch,
                            vel: h.vel,
                            articulation: Default::default(),
                            pitch_curve: vec![],
                            glide_ms: None,
                            vibrato: None,
                            volume_curve: vec![],
                            brightness_curve: vec![],
                        })
                        .collect();
                    ns.sort_by(|a, b| (a.pos, a.pitch, &a.id).cmp(&(b.pos, b.pitch, &b.id)));
                }
                drum_clip_id = Some(clip.id.clone());
                commands.push(Command::AddClip { track: tid, clip });
            }
        }
        if commands.is_empty() {
            return Err("置くものがありません(gap_beats か drum_track_id を指定)".to_owned());
        }
        let label = format!("{bar} 小節目へのつなぎ");
        let command = if commands.len() == 1 {
            commands.pop().expect("1 つある")
        } else {
            Command::batch(label.clone(), commands)
        };
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut out = mutated_json(&m);
        out["entry_id"] = json!(entry_id);
        out["bar"] = json!(bar);
        out["silenced_notes"] = json!(silenced);
        out["drum_clip_id"] = json!(drum_clip_id);
        if !skipped_audio.is_empty() {
            out["note"] = json!(format!(
                "音声のクリップ({})は無音にできませんでした。split_clip で分けて消すか、音量のオートメーションで",
                skipped_audio.join(", ")
            ));
        }
        Ok(JsonText(out))
    }

    #[tool(
        description = "定番のコード進行の候補を返す(読むだけ)。ジャンルと雰囲気で選び、キーに当てたコード名を返す\
        (王道進行・丸サ進行・カノン・小室進行・I–V–vi–IV・ii–V–I・ブルース・短調の EDM 進行・アンダルシア終止など)。\
        返り値の chords は write_chords / write_bassline にそのまま渡せる。区間ごとに違う進行を選ぶと展開が付く\
        (サビは王道進行、A メロは I–vi–IV–V など)。"
    )]
    async fn suggest_progression(
        &self,
        params: Parameters<SuggestProgressionParams>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("suggest_progression");
        let p = params.0;
        let key = match &p.key {
            Some(k) => Some(glaux_core::chord::Key::parse(k).ok_or_else(|| {
                format!("key は \"C major\" / \"A minor\" / \"F#m\" の形(got: {k})")
            })?),
            None => None,
        };
        let list = glaux_core::progressions::suggest(
            p.genre.as_deref(),
            p.mood.as_deref(),
            key,
            p.count.unwrap_or(5),
        )?;
        let mut out = json!({ "progressions": list });
        if list.is_empty() {
            let (g, m) = glaux_core::progressions::vocabulary();
            out["note"] =
                json!("当てはまる進行がありません。genre / mood を次から選ぶか、省略してください");
            out["genres"] = json!(g);
            out["moods"] = json!(m);
        }
        Ok(JsonText(out))
    }

    #[tool(
        description = "旋律を点検する(旋律を書いたら必ず)。旋律の「センス」のうち数えられる性質を測り、直し方と一緒に返す: \
        跳躍の多さ(5 半音以上の割合)・跳躍の後に戻るか(研究では約 72% が戻る)・区間の中の音域・最高音が計画書の山の区間で\
        初めて出るか・強拍が和音の音か・リズムの単調さと小節の型の使い回し・動機の反復率(覚えやすさ。多すぎも指摘)・\
        食い(8 分の裏で始まる音)・句と息継ぎ・句の終わりが伸びるか・音ごとの驚き(Temperley の式)。genre でしきい値を切り替える。\
        score(0〜100)は複数の案を比べるのに使う。和音は chords(進行)か、ほかのトラックから推定。"
    )]
    async fn critique_melody(&self, params: Parameters<CritiqueMelodyParams>) -> ToolResult {
        use glaux_core::melody;
        let _activity = self.handle.begin_activity("critique_melody");
        let p = params.0;
        let (project, _) = self.handle.get_project().await?;
        // 旋律の音
        let (track, clips): (&glaux_core::Track, Vec<&glaux_core::Clip>) =
            match (&p.clip_id, &p.track_id) {
                (Some(c), _) => {
                    let cid = glaux_core::ClipId::parse(c).map_err(|e| e.to_string())?;
                    let (t, clip) = project.clip(&cid).ok_or("クリップが見つかりません")?;
                    (t, vec![clip])
                }
                (None, Some(t)) => {
                    let tid = glaux_core::TrackId::parse(t).map_err(|e| e.to_string())?;
                    let t = project.track(&tid).ok_or("トラックが見つかりません")?;
                    (t, t.clips.iter().collect())
                }
                (None, None) => return Err("track_id か clip_id を指定してください".to_owned()),
            };
        let notes: Vec<melody::MelNote> = clips
            .iter()
            .flat_map(|c| {
                c.playback_notes()
                    .into_iter()
                    .map(move |n| melody::MelNote {
                        pos: c.start.0 + n.pos.0,
                        dur: n.dur.0,
                        pitch: n.pitch,
                    })
            })
            .collect();
        let genre_name = p.genre.as_deref().unwrap_or("pop");
        let genre = melody::genre(genre_name).ok_or_else(|| {
            format!(
                "genre が不明です({genre_name})。使えるもの: {}",
                melody::GENRES
                    .iter()
                    .map(|g| g.name)
                    .collect::<Vec<_>>()
                    .join(" / ")
            )
        })?;
        let key = match &p.key {
            Some(k) => Some(
                glaux_core::chord::Key::parse(k)
                    .ok_or_else(|| format!("key は \"C major\" / \"A minor\" の形(got: {k})"))?,
            ),
            None => None,
        };
        // 和音: 進行が渡されればそれ、無ければほかのトラック(ドラム以外)から小節ごとに推定
        let chord_spans: Vec<(u64, u64, Vec<u8>)> = match &p.chords {
            Some(ch) => {
                let l = progression_layout(&project, ch, p.key.as_deref(), None, p.bar, false)?;
                l.spans
                    .iter()
                    .filter_map(|s| {
                        s.chord.map(|c| {
                            (
                                l.clip_start + s.start,
                                l.clip_start + s.start + s.len,
                                l.chords[c].pitch_classes(),
                            )
                        })
                    })
                    .collect()
            }
            None => {
                let others: Vec<glaux_core::TrackId> = project
                    .tracks
                    .iter()
                    .filter(|t| t.id != track.id && !is_drum_track(t))
                    .map(|t| t.id.clone())
                    .collect();
                let h = glaux_core::harmony::analyze(&project, Some(&others), None);
                let mut v: Vec<(u64, u64, Vec<u8>)> = Vec::new();
                for (i, c) in h.chords.iter().enumerate() {
                    let end = h.chords.get(i + 1).map_or(u64::MAX, |n| n.tick);
                    if let Some(pcs) = glaux_core::harmony::chord_pitch_classes(&c.chord) {
                        v.push((c.tick, end, pcs));
                    }
                }
                v
            }
        };
        let chord_at = move |t: u64| {
            chord_spans
                .iter()
                .find(|(a, b, _)| *a <= t && t < *b)
                .map(|(_, _, p)| p.clone())
        };
        let mut marks = project.sections.clone();
        marks.sort_by_key(|m| m.tick);
        let end = project.end().0;
        let sections = marks
            .iter()
            .enumerate()
            .map(|(i, m)| melody::SectionSpan {
                name: m.name.clone(),
                start: m.tick.0,
                end: marks.get(i + 1).map_or(end, |n| n.tick.0),
                energy: m.energy,
            })
            .collect();
        let ctx = melody::Context {
            project: &project,
            chord_at: &chord_at,
            key,
            genre,
            sections,
            target: track.name.clone(),
        };
        let c = melody::critique(&notes, &ctx);
        Ok(JsonText(
            serde_json::to_value(&c).map_err(|e| e.to_string())?,
        ))
    }

    #[tool(
        description = "動機を形式に沿って展開して旋律にする(旋律は LLM が全部の音を書くより、動機だけ書いてこれで展開する)。\
        form: sentence(提示 → 反復 → 断片化 → 終止)/ period(問い → 答え)/ aaba / aab / call_response / loop か操作の並び。\
        動機の輪郭を保ったまま、繰り返しの 2 回目以降はリズムを変え(vary / diminish / augment / displace)、強拍の音を和音の音に合わせ(adapt)、音階の度数で移し、問いは 2 度・5 度で開き、\
        答えは主音で閉じて伸ばす。最高音は全体の 60〜75% の位置(peak_bar)に 1 回だけ置き、anticipate の割合で強拍の音を\
        8 分前へ食わせる。新しいクリップを作り、critique_melody の結果(score と指摘)も返す。seed と form を変えて\
        2〜3 案を作り、点検と聴き比べで選ぶとよい。1 回の undo で戻る。"
    )]
    async fn develop_motif(
        &self,
        params: Parameters<DevelopMotifParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        use glaux_core::{chord, melody, motif};
        let _activity = self.handle.begin_activity("develop_motif");
        let p = params.0;
        let tid = glaux_core::TrackId::parse(&p.track_id).map_err(|e| e.to_string())?;
        let (project, _) = self.handle.get_project().await?;
        let track = project.track(&tid).ok_or("トラックが見つかりません")?;
        if track.kind != glaux_core::TrackKind::Midi {
            return Err("MIDI トラックを指定してください".to_owned());
        }
        let first_bar = p.bar.unwrap_or(1).max(1);
        let (clip_start, bar_len) =
            glaux_core::arrange::bar_range(&project, first_bar, 1).ok_or("小節を数えられません")?;
        // 動機
        let notes: Vec<motif::MotifNote> = match (&p.motif, &p.motif_clip_id) {
            (Some(m), _) => motif::parse_motif(&m.replace('|', " "))?,
            (None, Some(c)) => {
                let cid = glaux_core::ClipId::parse(c).map_err(|e| e.to_string())?;
                let (_, clip) = project.clip(&cid).ok_or("動機のクリップが見つかりません")?;
                let ns = clip.playback_notes();
                let first = ns
                    .iter()
                    .map(|n| clip.start.0 + n.pos.0)
                    .min()
                    .ok_or("動機のクリップに音がありません")?;
                let grid = glaux_core::arrange::bar_grid(&project, first + 1);
                let bar0 = grid
                    .iter()
                    .rev()
                    .find(|(s, _)| *s <= first)
                    .map_or(0, |g| g.0);
                ns.iter()
                    .map(|n| motif::MotifNote {
                        offset: clip.start.0 + n.pos.0 - bar0,
                        dur: n.dur.0,
                        pitch: n.pitch,
                    })
                    .collect()
            }
            (None, None) => return Err("motif か motif_clip_id を指定してください".to_owned()),
        };
        let plan = motif::parse_plan(p.form.as_deref().unwrap_or("sentence"))?;
        let motif_end = notes.iter().map(|n| n.offset + n.dur).max().unwrap_or(0);
        let slot_bars = motif_end.div_ceil(bar_len).clamp(1, 4);
        let total_bars = slot_bars as u32 * plan.len() as u32;
        let clip_len = glaux_core::arrange::bar_range(&project, first_bar, total_bars)
            .ok_or("小節を数えられません")?
            .1;
        // 和音(クリップの頭からの tick → 和音)
        let (spans, chords_list) = melody_chords(
            &project,
            &tid,
            p.chords.as_deref(),
            p.key.as_deref(),
            first_bar,
            total_bars,
            clip_start,
            clip_len,
        )?;
        let look = motif::chord_lookup(&spans, &chords_list);
        // キー: 指定 → 進行の最初の和音と動機から推定
        let key = match &p.key {
            Some(k) => {
                chord::Key::parse(k).ok_or_else(|| format!("key は \"C major\" の形(got: {k})"))?
            }
            None => melody::guess_key(
                &notes
                    .iter()
                    .map(|n| melody::MelNote {
                        pos: n.offset,
                        dur: n.dur,
                        pitch: n.pitch,
                    })
                    .chain(chords_list.iter().flat_map(|c| {
                        c.pitch_classes().into_iter().map(|pc| melody::MelNote {
                            pos: 0,
                            dur: 240,
                            pitch: 60 + pc,
                        })
                    }))
                    .collect::<Vec<_>>(),
            ),
        };
        let center = notes.iter().map(|n| n.pitch as i32).sum::<i32>() / notes.len() as i32;
        let (low, high) = match &p.range {
            Some(r) => parse_range(Some(r))?,
            None => (
                (center - 10).clamp(0, 117) as u8,
                (center + 10).clamp(10, 127) as u8,
            ),
        };
        let peak_slot = p
            .peak_bar
            .map(|b| (b.saturating_sub(first_bar) / slot_bars as u32) as usize);
        let peak_pitch = match &p.peak {
            Some(pk) => Some(
                chord::parse_note(pk).ok_or_else(|| format!("peak の音名が読めません: {pk}"))?,
            ),
            None => None,
        };
        let opts = motif::Options {
            key,
            low,
            high,
            peak_slot,
            peak_pitch,
            anticipate: p.anticipate.unwrap_or(0.2).clamp(0.0, 1.0),
            seed: p.seed.unwrap_or(1),
            strong: glaux_core::meter::meter_at(&project, clip_start).strong_ticks(),
        };
        let out = motif::develop(&notes, &plan, bar_len, &look, &opts)?;
        let clip = melody_clip(
            &out,
            clip_start,
            clip_len,
            bar_len,
            p.velocity.unwrap_or(92),
            p.name.clone().unwrap_or_else(|| "Melody".to_owned()),
        );
        let clip_id = clip.id.clone();
        let genre =
            melody::genre(p.genre.as_deref().unwrap_or("pop")).unwrap_or(&melody::GENRES[0]);
        let crit = melody_critique(&project, &track.name, &out, clip_start, &look, key, genre);
        let label = format!("旋律の展開({} 小節)", total_bars);
        let command = Command::AddClip { track: tid, clip };
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["clip_id"] = json!(clip_id);
        v["bars"] = json!(total_bars);
        v["notes"] = json!(out.len());
        v["key"] = json!(crit.key);
        v["score"] = json!(crit.score);
        v["findings"] = json!(crit.findings);
        v["metrics"] = json!(crit.metrics);
        Ok(JsonText(v))
    }

    #[tool(
        description = "旋律を作る(動機から道具に任せる入口)。役割(verse / pre / chorus / hook / lead)とジャンルのリズムの型から\
        動機のリズムを選び(2 小節の動機は 1 小節目が動き、2 小節目が伸ばす)、輪郭(arch / rise / fall / valley / flat_hook)に沿って\
        強拍に和音の音・弱拍に音階の音を当て、develop_motif と同じ手順で展開し、critique_melody で点数をつける。これを\
        candidates 案(既定 4)作って最も点数の高い案を置き、ほかの案の seed・形式・輪郭・点数と指摘を返す。place: 2 で 2 番目の案も\
        複製したトラック(ミュート)に置いて聴き比べられる。返る motif は develop_motif にそのまま渡せる(動機だけ手で直して展開し直せる)。\
        rhythm で動機のリズムを固定できる。1 回の undo で戻る。"
    )]
    async fn write_melody(
        &self,
        params: Parameters<WriteMelodyParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        use glaux_core::{chord, melgen, melody, motif};
        let _activity = self.handle.begin_activity("write_melody");
        let p = params.0;
        let tid = glaux_core::TrackId::parse(&p.track_id).map_err(|e| e.to_string())?;
        let (project, _) = self.handle.get_project().await?;
        let track = project.track(&tid).ok_or("トラックが見つかりません")?;
        if track.kind != glaux_core::TrackKind::Midi {
            return Err("MIDI トラックを指定してください".to_owned());
        }
        let role_name = p.role.as_deref().unwrap_or("chorus");
        let role = melgen::role(role_name).ok_or_else(|| {
            format!("role は verse / pre / chorus / hook / lead(got: {role_name})")
        })?;
        let genre_name = p.genre.as_deref().unwrap_or("pop");
        let genre = melody::genre(genre_name).ok_or_else(|| {
            format!("genre は pop / edm / trap / lofi / jazz / funk(got: {genre_name})")
        })?;
        let vocab = melgen::vocab(genre_name);
        let first_bar = p.bar.unwrap_or(1).max(1);
        let (bar0, bar_len) =
            glaux_core::arrange::bar_range(&project, first_bar, 1).ok_or("小節を数えられません")?;
        // 拍子(区間の最初の小節。変拍子はまとまりの頭が強拍)
        let meter = glaux_core::meter::meter_at(&project, bar0);
        let step = glaux_core::meter::STEP;
        // 動機のリズム(固定するとき)
        let fixed = match &p.rhythm {
            Some(r) => {
                let (notes, len) = melgen::parse_grid(r, step)?;
                if len > bar_len * 2 {
                    return Err("rhythm は 2 小節まで".to_owned());
                }
                Some((notes, len.div_ceil(bar_len).max(1), r.clone()))
            }
            None => None,
        };
        let forms: Vec<String> = match &p.form {
            Some(f) => vec![f.clone()],
            None => role.forms.iter().map(|f| f.to_string()).collect(),
        };
        for f in &forms {
            motif::parse_plan(f)?;
        }
        let contours: Vec<melgen::Contour> = match &p.contour {
            Some(c) => vec![melgen::Contour::parse(c).ok_or_else(|| {
                format!("contour は arch / rise / fall / valley / flat_hook(got: {c})")
            })?],
            None => role.contours.to_vec(),
        };
        let bars = p.bars.unwrap_or(8).clamp(1, 64);
        // 動機の長さ(案ごとに選ぶ候補)。4 小節以下の偶数小節の区間は 1 小節と 2 小節を案ごとに混ぜる
        // (1 小節の動機は全部同じリズムに、2 小節の動機は繰り返しが無くなりやすい。点検の点数で選ぶ)
        let motif_bar_choices: Vec<u64> = match (&fixed, p.motif_bars) {
            (Some((_, b, _)), _) => vec![*b],
            (None, Some(b)) => vec![b.clamp(1, 2) as u64],
            (None, None) => {
                if bars <= 4 {
                    if bars.is_multiple_of(2) {
                        vec![1, 2]
                    } else {
                        vec![1]
                    }
                } else if !genre.breath && forms.iter().all(|f| f == "loop") {
                    vec![1]
                } else {
                    vec![2]
                }
            }
        };
        let unit = *motif_bar_choices.iter().max().unwrap_or(&1);
        let total_bars = (bars as u64).div_ceil(unit) as u32 * unit as u32;
        let (clip_start, clip_len) =
            glaux_core::arrange::bar_range(&project, first_bar, total_bars)
                .ok_or("小節を数えられません")?;
        let (spans, chords_list) = melody_chords(
            &project,
            &tid,
            p.chords.as_deref(),
            p.key.as_deref(),
            first_bar,
            total_bars,
            clip_start,
            clip_len,
        )?;
        let look = motif::chord_lookup(&spans, &chords_list);
        let key = match &p.key {
            Some(k) => {
                chord::Key::parse(k).ok_or_else(|| format!("key は \"C major\" の形(got: {k})"))?
            }
            None => {
                if chords_list.is_empty() {
                    return Err(
                        "下の和音が見つかりません。chords か key を指定してください".to_owned()
                    );
                }
                melody::guess_key(
                    &chords_list
                        .iter()
                        .flat_map(|c| {
                            c.pitch_classes().into_iter().map(|pc| melody::MelNote {
                                pos: 0,
                                dur: 240,
                                pitch: 60 + pc,
                            })
                        })
                        .collect::<Vec<_>>(),
                )
            }
        };
        let (low, high) = match &p.range {
            Some(r) => parse_range(Some(r))?,
            None => role.range,
        };
        let peak_pitch = match &p.peak {
            Some(pk) => Some(
                chord::parse_note(pk).ok_or_else(|| format!("peak の音名が読めません: {pk}"))?,
            ),
            None => None,
        };
        let anticipate = p.anticipate.unwrap_or(vocab.anticipate).clamp(0.0, 1.0);
        let n = p.candidates.unwrap_or(4).clamp(1, 12);
        // 既定は始まりの小節番号(区間ごとに違う案になる)
        let base_seed = p.seed.unwrap_or(first_bar as u64);
        let start = (low as f64 + (high as f64 - low as f64) * role.start).round() as u8;
        struct Cand {
            seed: u64,
            form: String,
            contour: melgen::Contour,
            rhythm: String,
            motif: Vec<motif::MotifNote>,
            out: Vec<motif::Out>,
            crit: melody::MelodyCritique,
        }
        let mut cands: Vec<Cand> = Vec::new();
        for k in 0..n as u64 {
            let seed = base_seed.wrapping_add(k);
            let form = forms[(seed % forms.len() as u64) as usize].clone();
            let contour = contours[((seed / forms.len() as u64) % contours.len() as u64) as usize];
            let motif_bars = motif_bar_choices[((seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 33)
                % motif_bar_choices.len() as u64)
                as usize];
            let slots = (total_bars as u64 / motif_bars) as usize;
            let peak_slot = p
                .peak_bar
                .map(|b| (b.saturating_sub(first_bar) as u64 / motif_bars) as usize);
            let (rhythm, rhythm_name) = match &fixed {
                Some((r, _, name)) => (r.clone(), name.clone()),
                None => melgen::pick_rhythm(vocab, role, motif_bars, &meter, genre.breath, seed),
            };
            let spec = melgen::MotifSpec {
                key,
                contour,
                low,
                high,
                start,
                span: role.span,
                bar_len,
                strong: meter.strong_ticks(),
                chord_at: &look,
                seed,
            };
            let m = melgen::make_motif(&rhythm, &spec);
            let plan = melgen::fit_form(&motif::parse_plan(&form)?, slots);
            let opts = motif::Options {
                key,
                low,
                high,
                peak_slot,
                peak_pitch,
                anticipate,
                seed,
                strong: meter.strong_ticks(),
            };
            let out = motif::develop(&m, &plan, bar_len, &look, &opts)?;
            let crit = melody_critique(&project, &track.name, &out, clip_start, &look, key, genre);
            cands.push(Cand {
                seed,
                form,
                contour,
                rhythm: rhythm_name,
                motif: m,
                out,
                crit,
            });
        }
        cands.sort_by(|a, b| b.crit.score.cmp(&a.crit.score).then(a.seed.cmp(&b.seed)));
        let place = p.place.unwrap_or(1).clamp(1, 2).min(cands.len());
        let name = p.name.clone().unwrap_or_else(|| "Melody".to_owned());
        let vel = p.velocity.unwrap_or(92);
        let mut commands = Vec::new();
        let mut placed = Vec::new();
        for (i, c) in cands.iter().take(place).enumerate() {
            let clip = melody_clip(&c.out, clip_start, clip_len, bar_len, vel, name.clone());
            let clip_id = clip.id.clone();
            let target = if i == 0 {
                tid.clone()
            } else {
                // 同じ音色のトラックを複製して(ミュートで)置く
                let copy = copy_track_shell(track, format!("{} 案{}", track.name, i + 1));
                let id = copy.id.clone();
                let index = project
                    .tracks
                    .iter()
                    .position(|t| t.id == tid)
                    .map(|x| x + 1);
                commands.push(Command::AddTrack { track: copy, index });
                id
            };
            placed.push(json!({
                "clip_id": clip_id,
                "track_id": target,
                "seed": c.seed,
            }));
            commands.push(Command::AddClip {
                track: target,
                clip,
            });
        }
        let best = &cands[0];
        let summary = |c: &Cand| {
            json!({
                "seed": c.seed,
                "form": c.form,
                "contour": c.contour.name(),
                "rhythm": c.rhythm,
                "score": c.crit.score,
                "findings": c.crit.findings.iter().filter(|f| f.severity == "warn").map(|f| f.what.clone()).collect::<Vec<_>>(),
            })
        };
        let label = format!("旋律を作る({} 小節・{} 案から)", total_bars, cands.len());
        let command = Command::Batch {
            commands,
            label: label.clone(),
        };
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["placed"] = json!(placed);
        v["clip_id"] = placed[0]["clip_id"].clone();
        v["bars"] = json!(total_bars);
        v["key"] = json!(best.crit.key);
        v["seed"] = json!(best.seed);
        v["form"] = json!(best.form);
        v["contour"] = json!(best.contour.name());
        v["rhythm"] = json!(best.rhythm);
        v["motif"] = json!(melgen::format_motif(&best.motif));
        v["score"] = json!(best.crit.score);
        v["findings"] = json!(best.crit.findings);
        v["metrics"] = json!(best.crit.metrics);
        v["candidates"] = json!(cands.iter().map(summary).collect::<Vec<_>>());
        Ok(JsonText(v))
    }

    #[tool(
        description = "小節 1 つ(count で数小節)だけ拍子を変える: 1 拍足す・抜く(J-POP のサビ前の 2/4、プログレの 1 拍足し、\
        7/8 を 1 小節だけ挟む)。to(\"2/4\"・\"7/8 3+2+2\")か beats(+1 / -2 など)で指定。延ばすときは小節の終わりに空白を入れ、\
        縮めるときは小節の後ろを削る(その範囲の音は消える。消えた数を removed_notes で返す)。後ろのクリップ・テンポ・拍子・\
        マーカー・オートメーションはずらし、次の小節で元の拍子に戻す。1 回の undo で戻る。"
    )]
    async fn change_meter(
        &self,
        params: Parameters<ChangeMeterParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("change_meter");
        let p = params.0;
        let (project, _) = self.handle.get_project().await?;
        let (start, _) = glaux_core::arrange::bar_range(&project, p.bar.max(1), 1)
            .ok_or("小節を数えられません")?;
        let cur = glaux_core::meter::meter_at(&project, start);
        let (num, den, grouping) = match (&p.to, p.beats) {
            (Some(t), _) => {
                let t = t.trim();
                let (sig, g) = match t.split_once([' ', '(']) {
                    Some((a, b)) => (a, Some(b.trim_end_matches(')'))),
                    None => (t, None),
                };
                let (n, d) = sig
                    .split_once('/')
                    .ok_or_else(|| format!("to は \"7/8\" の形(got: {t})"))?;
                let n: u8 = n
                    .trim()
                    .parse()
                    .map_err(|_| format!("拍子が読めません: {t}"))?;
                let d: u8 = d
                    .trim()
                    .parse()
                    .map_err(|_| format!("拍子が読めません: {t}"))?;
                let g = match g.map(str::trim).filter(|g| !g.is_empty()) {
                    Some(g) => Some(glaux_core::meter::parse_grouping(g)?),
                    None => None,
                };
                (n, d, g)
            }
            (None, Some(b)) => {
                let n = cur.num as i32 + b;
                if !(1..=64).contains(&n) {
                    return Err(format!("{}/{} に {b} 拍は足せません", cur.num, cur.den));
                }
                (n as u8, cur.den, None)
            }
            (None, None) => return Err("to か beats を指定してください".to_owned()),
        };
        let count = p.count.unwrap_or(1);
        let (cmds, removed) = glaux_core::arrange::change_bar_meter(
            &project,
            p.bar.max(1),
            count,
            num,
            den,
            grouping.clone(),
        )?;
        if cmds.is_empty() {
            return Err("変更がありません".to_owned());
        }
        let label = format!(
            "{} 小節目{}を {}/{}{} に",
            p.bar,
            if count > 1 {
                format!("から {count} 小節")
            } else {
                String::new()
            },
            num,
            den,
            grouping
                .as_ref()
                .map(|g| format!("({})", glaux_core::meter::grouping_text(g)))
                .unwrap_or_default()
        );
        let command = Command::batch(label.clone(), cmds);
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let (after, _) = self.handle.get_project().await?;
        let meter = glaux_core::meter::meter_at(&after, start);
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["meter"] = json!(meter.label());
        v["was"] = json!(cur.label());
        v["bar_ticks"] = json!(meter.len);
        v["removed_notes"] = json!(removed);
        if removed > 0 {
            v["warning"] = json!(format!(
                "縮めた範囲の音 {removed} 個が消えました(戻すなら undo)"
            ));
        }
        Ok(JsonText(v))
    }

    #[tool(
        description = "ポリリズムを置く: ratio \"a:b\" = 4 分 b 個の長さに a 個を等間隔に(3:2 = 2 拍に 3 つ、4:3、5:4 など)。\
        bar から bars 小節に a:b の組を並べる。pitch2 を渡すと b の側(4 分の刻み)も置く。tick で割り切れないときは丸め、\
        誤差(rounding_error_ticks)を返す。新しいクリップを作る。1 回の undo で戻る。"
    )]
    async fn write_polyrhythm(
        &self,
        params: Parameters<WritePolyrhythmParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("write_polyrhythm");
        let p = params.0;
        let tid = glaux_core::TrackId::parse(&p.track_id).map_err(|e| e.to_string())?;
        let (project, _) = self.handle.get_project().await?;
        let track = project.track(&tid).ok_or("トラックが見つかりません")?;
        if track.kind != glaux_core::TrackKind::Midi {
            return Err("MIDI トラックを指定してください".to_owned());
        }
        let (a, b) = p
            .ratio
            .split_once(':')
            .and_then(|(a, b)| Some((a.trim().parse::<u32>().ok()?, b.trim().parse::<u32>().ok()?)))
            .filter(|(a, b)| (1..=32).contains(a) && (1..=32).contains(b))
            .ok_or_else(|| format!("ratio は \"3:2\" の形(1〜32。got: {})", p.ratio))?;
        let first_bar = p.bar.unwrap_or(1).max(1);
        let bars = p.bars.unwrap_or(1).clamp(1, 256);
        let (clip_start, clip_len) = glaux_core::arrange::bar_range(&project, first_bar, bars)
            .ok_or("小節を数えられません")?;
        let span = glaux_core::PPQ * b as u64;
        let (pos, err) = glaux_core::meter::polyrhythm(span, a);
        let drum = is_drum_track(track);
        let pitch = parse_pitch(p.pitch.as_deref(), if drum { 37 } else { 72 })?;
        let pitch2 = match &p.pitch2 {
            Some(x) => Some(parse_pitch(Some(x), 42)?),
            None => None,
        };
        let vel = p.velocity.unwrap_or(90).clamp(1, 127);
        let gate = p.gate.unwrap_or(0.5).clamp(0.1, 1.0);
        let spacing = span as f64 / a as f64;
        let mut notes: Vec<(u64, u64, u8, u8)> = Vec::new();
        let mut t = 0u64;
        while t < clip_len {
            for (k, &o) in pos.iter().enumerate() {
                let at = t + o;
                if at >= clip_len {
                    break;
                }
                let v = if k == 0 {
                    (vel as u16 + 12).min(127) as u8
                } else {
                    vel
                };
                notes.push((at, ((spacing * gate).round() as u64).max(1), pitch, v));
            }
            if let Some(p2) = pitch2 {
                for k in 0..b as u64 {
                    let at = t + k * glaux_core::PPQ;
                    if at < clip_len {
                        let v = if k == 0 {
                            vel
                        } else {
                            vel.saturating_sub(12).max(1)
                        };
                        notes.push((at, (glaux_core::PPQ as f64 * gate).round() as u64, p2, v));
                    }
                }
            }
            t += span;
        }
        let name = p.name.clone().unwrap_or_else(|| format!("Poly {a}:{b}"));
        let clip = simple_clip(name, clip_start, clip_len, &notes);
        let clip_id = clip.id.clone();
        let label = format!("ポリリズム {a}:{b}({bars} 小節)");
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(
            self.handle
                .apply(Command::AddClip { track: tid, clip }, author, label)
                .await,
        )?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["clip_id"] = json!(clip_id);
        v["notes"] = json!(notes.len());
        v["spacing_ticks"] = json!((spacing * 100.0).round() / 100.0);
        v["rounding_error_ticks"] = json!((err * 100.0).round() / 100.0);
        Ok(JsonText(v))
    }

    #[tool(
        description = "ポリメーターを置く: 周期の型(\"X..x..x.\" や ユークリッドリズム \"E(5,16)\")を小節線を無視して\
        周期のまま並べる(3 ステップの型を 16 分で並べると 4/4 の上でずれていき、3 小節で元に戻る)。unit で 1 文字の長さ\
        (16th / 8th / quarter / 8t / 16t)、reset_every_bars で何小節ごとに頭へ戻すか。返り値に元に戻るまでの小節数\
        (realign_bars)。ハットやパーカッションを周期の違う型で重ねると、繰り返しでも同じに聞こえにくい。1 回の undo で戻る。"
    )]
    async fn write_polymeter(
        &self,
        params: Parameters<WritePolymeterParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("write_polymeter");
        let p = params.0;
        let tid = glaux_core::TrackId::parse(&p.track_id).map_err(|e| e.to_string())?;
        let (project, _) = self.handle.get_project().await?;
        let track = project.track(&tid).ok_or("トラックが見つかりません")?;
        if track.kind != glaux_core::TrackKind::Midi {
            return Err("MIDI トラックを指定してください".to_owned());
        }
        let cycle = glaux_core::meter::parse_cycle(&p.pattern)?;
        let unit: u64 = match p.unit.as_deref().unwrap_or("16th").trim() {
            "16th" | "16" => 240,
            "8th" | "8" => 480,
            "quarter" | "4" => 960,
            "8t" => 320,
            "16t" => 160,
            other => other
                .parse::<u64>()
                .ok()
                .filter(|u| (30..=3840).contains(u))
                .ok_or_else(|| {
                    format!("unit は 16th / 8th / quarter / 8t / 16t か tick の数(got: {other})")
                })?,
        };
        let first_bar = p.bar.unwrap_or(1).max(1);
        let bars = p.bars.unwrap_or(4).clamp(1, 256);
        let (clip_start, clip_len) = glaux_core::arrange::bar_range(&project, first_bar, bars)
            .ok_or("小節を数えられません")?;
        let reset = p.reset_every_bars.unwrap_or(0);
        // 頭へ戻す位置(クリップの頭から)
        let mut resets: Vec<u64> = vec![0];
        if reset > 0 {
            let mut b = reset;
            while b < bars {
                if let Some((s, _)) = glaux_core::arrange::bar_range(&project, first_bar + b, 1) {
                    resets.push(s - clip_start);
                }
                b += reset;
            }
        }
        let drum = is_drum_track(track);
        let pitch = parse_pitch(p.pitch.as_deref(), if drum { 37 } else { 72 })?;
        let vel = p.velocity.unwrap_or(90).clamp(1, 107);
        let gate = p.gate.unwrap_or(0.5).clamp(0.1, 1.0);
        let dur = ((unit as f64 * gate).round() as u64).max(1);
        let mut notes: Vec<(u64, u64, u8, u8)> = Vec::new();
        for (ri, &r0) in resets.iter().enumerate() {
            let r1 = resets.get(ri + 1).copied().unwrap_or(clip_len);
            let mut k = 0u64;
            while r0 + k * unit < r1 {
                let c = cycle[(k % cycle.len() as u64) as usize];
                if c != '.' {
                    let v = if c == 'X' { vel + 20 } else { vel };
                    notes.push((r0 + k * unit, dur.min(r1 - (r0 + k * unit)), pitch, v));
                }
                k += 1;
            }
        }
        let cycle_ticks = cycle.len() as u64 * unit;
        let bar_len = glaux_core::meter::meter_at(&project, clip_start).len;
        let realign = glaux_core::meter::lcm(cycle_ticks, bar_len) / bar_len.max(1);
        let name = p.name.clone().unwrap_or_else(|| "Polymeter".to_owned());
        let clip = simple_clip(name, clip_start, clip_len, &notes);
        let clip_id = clip.id.clone();
        let label = format!("ポリメーター({} ステップの周期、{bars} 小節)", cycle.len());
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(
            self.handle
                .apply(Command::AddClip { track: tid, clip }, author, label)
                .await,
        )?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["clip_id"] = json!(clip_id);
        v["notes"] = json!(notes.len());
        v["cycle_steps"] = json!(cycle.len());
        v["cycle"] = json!(cycle.iter().collect::<String>());
        v["realign_bars"] = json!(realign);
        Ok(JsonText(v))
    }

    #[tool(
        description = "変拍子の「長い拍・短い拍」の比を揺らす(アクサクの揺れ): 7/8 の 2+2+3 などで、3 のまとまりを 2 のまとまりの\
        long_ratio 倍に(1.5 = 変えない。1.4 で長い拍が詰まり前へ転がる、1.55 で溜める)、short_skew で 2 つ目以降の短い拍を\
        少し短く。小節の長さは変えず、まとまりの中の音は比例で動かす(8 分のスウィングとは別のつまみ)。拍のまとまりが 2 と 3 の\
        小節だけが対象(4/4 は変わらない)。今の位置から計算するので 2 回かけると重なる。1 回の undo で戻る。"
    )]
    async fn set_meter_feel(
        &self,
        params: Parameters<SetMeterFeelParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("set_meter_feel");
        let p = params.0;
        let long_ratio = p.long_ratio.unwrap_or(1.5);
        let short_skew = p.short_skew.unwrap_or(1.0);
        if !(1.2..=1.8).contains(&long_ratio) {
            return Err(format!("long_ratio は 1.2〜1.8(got: {long_ratio})"));
        }
        if !(0.85..=1.0).contains(&short_skew) {
            return Err(format!("short_skew は 0.85〜1.0(got: {short_skew})"));
        }
        let mut ids: Vec<String> = p.clip_ids.clone().unwrap_or_default();
        if let Some(c) = &p.clip_id {
            if !ids.contains(c) {
                ids.push(c.clone());
            }
        }
        if ids.is_empty() {
            return Err("clip_id か clip_ids を指定してください".to_owned());
        }
        let (project, version) = self.handle.get_project().await?;
        let mut commands = Vec::new();
        let mut total = 0usize;
        let mut touched_bars = std::collections::BTreeSet::new();
        for id in &ids {
            let cid = glaux_core::ClipId::parse(id).map_err(|e| e.to_string())?;
            let (_, clip) = project
                .clip(&cid)
                .ok_or_else(|| format!("クリップが見つかりません: {id}"))?;
            let Some(notes) = clip.notes() else {
                return Err(format!("MIDI クリップではありません: {id}"));
            };
            let start = clip.start.0;
            let meters = glaux_core::meter::bar_meters(&project, start + clip.length.0 + 1);
            let find = |t: u64| {
                let i = meters.partition_point(|m| m.start <= t).checked_sub(1)?;
                meters.get(i)
            };
            let mut changes = Vec::new();
            for n in notes {
                let abs = start + n.pos.0;
                let Some(m) = find(abs) else { continue };
                let Some(segs) = glaux_core::meter::feel_segments(m, long_ratio, short_skew) else {
                    continue;
                };
                let new_abs = m.start + glaux_core::meter::feel_map(&segs, abs - m.start);
                let end = abs + n.dur.0;
                let new_end = if end <= m.start + m.len {
                    m.start + glaux_core::meter::feel_map(&segs, end - m.start)
                } else {
                    end
                };
                let new_pos = new_abs.saturating_sub(start);
                let new_dur = new_end.saturating_sub(new_abs).max(1);
                if new_pos != n.pos.0 || new_dur != n.dur.0 {
                    touched_bars.insert(m.start);
                    changes.push(
                        glaux_core::NoteChange::new(n.id.clone())
                            .pos(glaux_core::Tick(new_pos))
                            .dur(glaux_core::Tick(new_dur)),
                    );
                }
            }
            total += changes.len();
            if !changes.is_empty() {
                commands.push(Command::UpdateNotes { clip: cid, changes });
            }
        }
        if commands.is_empty() {
            return Ok(JsonText(json!({
                "project_version": version,
                "changed": 0,
                "note": "変わる音がありません(拍のまとまりが 2 と 3 の小節が無いか、比が 1.5・1.0)",
            })));
        }
        let label = format!("拍の揺れ(長い拍 ×{long_ratio:.2}、{total} ノート)");
        let command = Command::batch(label.clone(), commands);
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["changed"] = json!(total);
        v["bars"] = json!(touched_bars.len());
        Ok(JsonText(v))
    }

    #[tool(
        description = "旋律に音程の表情を付ける(ピッチカーブの点に展開): shakuri(しゃくり)/ scoop / plop / slide_in / bend / \
        prebend_release / fall / doit / kobushi(こぶし)/ shake。note_ids で音を選ぶか、target の規則(句の頭・句の終わり・\
        上への跳躍・長い音)と probability で選ぶ。深さ・長さは表情ごとの既定(しゃくりは −150 セントから 120ms、\
        フォールは終わりの 250ms で −700 セント)で、そのときのテンポで tick に直す。頭の表情と終わりの表情は重ねられる\
        (元のカーブの範囲の外の点は残す)。歌メロは shakuri を跳躍に、fall を句の終わりに少し。1 回の undo で戻る。"
    )]
    async fn pitch_gesture(
        &self,
        params: Parameters<PitchGestureParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        use glaux_core::gesture;
        let _activity = self.handle.begin_activity("pitch_gesture");
        let p = params.0;
        let g = gesture::Gesture::parse(&p.kind).ok_or_else(|| {
            format!(
                "kind は shakuri / scoop / plop / slide_in / bend / prebend_release / fall / doit / kobushi / shake(got: {})",
                p.kind
            )
        })?;
        let target = match &p.target {
            Some(t) => gesture::Target::parse(t).ok_or_else(|| {
                format!("target は phrase_start / phrase_end / leap_up / long / all(got: {t})")
            })?,
            None => g.default_target(),
        };
        let (def_amount, def_ms) = g.defaults();
        let amount = p.amount_cents.unwrap_or(def_amount).clamp(0.0, 2400.0);
        let ms = p.time_ms.unwrap_or(def_ms).clamp(10.0, 3000.0);
        let prob = if p.note_ids.is_some() {
            1.0
        } else {
            p.probability.unwrap_or(0.7).clamp(0.0, 1.0)
        };
        let ids = clip_id_list(&p.clip_id, &p.clip_ids)?;
        let (project, version) = self.handle.get_project().await?;
        let mut rng = p.seed.unwrap_or(1).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut unit = || {
            rng ^= rng >> 12;
            rng ^= rng << 25;
            rng ^= rng >> 27;
            (rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut commands = Vec::new();
        let mut total = 0usize;
        for id in &ids {
            let cid = glaux_core::ClipId::parse(id).map_err(|e| e.to_string())?;
            let (_, clip) = project
                .clip(&cid)
                .ok_or_else(|| format!("クリップが見つかりません: {id}"))?;
            let notes = clip
                .notes()
                .ok_or_else(|| format!("MIDI クリップではありません: {id}"))?;
            let chosen: Vec<usize> = match &p.note_ids {
                Some(want) => notes
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| want.iter().any(|w| w == n.id.as_str()))
                    .map(|(i, _)| i)
                    .collect(),
                None => gesture::select(notes, target, glaux_core::PPQ)
                    .into_iter()
                    .filter(|_| unit() < prob)
                    .collect(),
            };
            let mut changes = Vec::new();
            for i in chosen {
                let n = &notes[i];
                let abs = clip.start.0 + n.pos.0;
                let bpm = project.tempo_map.bpm_at(glaux_core::Tick(abs));
                let time = (ms as f64 * glaux_core::PPQ as f64 * bpm / 60_000.0).round() as u64;
                let pts = gesture::curve(g, n.dur.0, time, amount);
                let curve = gesture::merge(&n.pitch_curve, &pts);
                if curve != n.pitch_curve {
                    let mut ch = glaux_core::NoteChange::new(n.id.clone()).pitch_curve(curve);
                    // フォール・ドイトは抜けながら消える(音量の曲線が無い音だけ)
                    if matches!(g, gesture::Gesture::Fall | gesture::Gesture::Doit)
                        && p.fade_volume.unwrap_or(true)
                        && n.volume_curve.is_empty()
                    {
                        let start = pts.first().map_or(0, |x| x.0);
                        ch = ch.volume_curve(vec![
                            glaux_core::CurvePoint::new(
                                glaux_core::Tick(start),
                                0.0,
                                glaux_core::CurveShape::EaseIn,
                            ),
                            glaux_core::CurvePoint::new(
                                n.dur,
                                -18.0,
                                glaux_core::CurveShape::Linear,
                            ),
                        ]);
                    }
                    changes.push(ch);
                }
            }
            total += changes.len();
            if !changes.is_empty() {
                commands.push(Command::UpdateNotes { clip: cid, changes });
            }
        }
        if commands.is_empty() {
            return Ok(JsonText(json!({
                "project_version": version,
                "changed": 0,
                "note": "付ける音がありませんでした(target・probability・note_ids を確かめる)",
            })));
        }
        let label = format!("音程の表情 {}({total} ノート)", p.kind);
        let command = Command::batch(label.clone(), commands);
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["changed"] = json!(total);
        v["amount_cents"] = json!(amount);
        v["time_ms"] = json!(ms);
        Ok(JsonText(v))
    }

    #[tool(
        description = "ノートにビブラートを付ける(速さ・深さ・始まるまで・フェード・終わりの速さ)。style で楽器の型\
        (vocal 5.5Hz ±40 セント 250ms 後から / vocal_strong ±80 / strings ±20 / guitar / wind / synth)を選び、個々の値で上書きできる。\
        note_ids 省略で min_beats 拍以上の音(伸ばし)だけ。humanize で音ごとに深さ・速さを少し変える。奏法の vibrato\
        (5.5Hz ±30 固定)より細かく決められ、両方あればこちらが優先。remove: true で外す。1 回の undo で戻る。"
    )]
    async fn set_vibrato(
        &self,
        params: Parameters<SetVibratoParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("set_vibrato");
        let p = params.0;
        let style = p.style.as_deref().unwrap_or("vocal");
        let mut base = glaux_core::gesture::vibrato_style(style).ok_or_else(|| {
            format!("style は vocal / vocal_strong / strings / guitar / wind / synth(got: {style})")
        })?;
        if let Some(x) = p.rate_hz {
            base.rate_hz = x;
        }
        if let Some(x) = p.depth_cents {
            base.depth_cents = x;
        }
        if let Some(x) = p.delay_ms {
            base.delay_ms = x;
        }
        if let Some(x) = p.fade_in_ms {
            base.fade_in_ms = x;
        }
        if let Some(x) = p.fade_out_ms {
            base.fade_out_ms = x;
        }
        if p.rate_end_hz.is_some() {
            base.rate_end_hz = p.rate_end_hz;
        }
        let remove = p.remove.unwrap_or(false);
        if !remove {
            glaux_core::check_vibrato(&base)?;
        }
        let humanize = p.humanize.unwrap_or(0.1).clamp(0.0, 0.3) as f32;
        let min_len = (p.min_beats.unwrap_or(1.0).max(0.0) * glaux_core::PPQ as f64) as u64;
        let ids = clip_id_list(&p.clip_id, &p.clip_ids)?;
        let (project, version) = self.handle.get_project().await?;
        let mut rng = p.seed.unwrap_or(1).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut unit = || {
            rng ^= rng >> 12;
            rng ^= rng << 25;
            rng ^= rng >> 27;
            (rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f32 / (1u64 << 53) as f32
        };
        let mut commands = Vec::new();
        let mut total = 0usize;
        for id in &ids {
            let cid = glaux_core::ClipId::parse(id).map_err(|e| e.to_string())?;
            let (_, clip) = project
                .clip(&cid)
                .ok_or_else(|| format!("クリップが見つかりません: {id}"))?;
            let notes = clip
                .notes()
                .ok_or_else(|| format!("MIDI クリップではありません: {id}"))?;
            let mut changes = Vec::new();
            for n in notes {
                let chosen = match &p.note_ids {
                    Some(want) => want.iter().any(|w| w == n.id.as_str()),
                    None => n.dur.0 >= min_len,
                };
                if !chosen {
                    continue;
                }
                if remove {
                    if n.vibrato.is_some() {
                        changes.push(glaux_core::NoteChange::new(n.id.clone()).vibrato(
                            glaux_core::Vibrato {
                                depth_cents: 0.0,
                                ..base
                            },
                        ));
                    }
                    continue;
                }
                let mut v = base;
                if humanize > 0.0 {
                    v.depth_cents =
                        (v.depth_cents * (1.0 + humanize * (unit() * 2.0 - 1.0))).clamp(0.0, 300.0);
                    v.rate_hz = (v.rate_hz * (1.0 + humanize * 0.5 * (unit() * 2.0 - 1.0)))
                        .clamp(0.5, 12.0);
                    v.depth_cents = (v.depth_cents * 10.0).round() / 10.0;
                    v.rate_hz = (v.rate_hz * 100.0).round() / 100.0;
                }
                changes.push(glaux_core::NoteChange::new(n.id.clone()).vibrato(v));
            }
            total += changes.len();
            if !changes.is_empty() {
                commands.push(Command::UpdateNotes { clip: cid, changes });
            }
        }
        if commands.is_empty() {
            return Ok(JsonText(json!({
                "project_version": version,
                "changed": 0,
                "note": "付ける音がありませんでした(min_beats・note_ids を確かめる)",
            })));
        }
        let label = if remove {
            format!("ビブラートを外す({total} ノート)")
        } else {
            format!("ビブラート {style}({total} ノート)")
        };
        let command = Command::batch(label.clone(), commands);
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["changed"] = json!(total);
        v["vibrato"] = json!(base);
        Ok(JsonText(v))
    }

    #[tool(
        description = "装飾音を付ける: acciaccatura(短前打音)/ appoggiatura(長前打音)/ mordent / inverted_mordent / turn / \
        trill / schleifer(下から滑り込む 2 音)。note_ids で音を選ぶか、target の規則と probability(既定 0.5)で選ぶ。\
        隣の音はキーの音階(省略でクリップから推定)か interval の半音の数。装飾の音は 8 分の 20%(45〜125ms)、\
        トリルは 1 秒 10 音(研究の目安)。短前打音は前の音の区間に置き、前の音を短くする。短すぎる音は飛ばす。\
        ジャズ・クラシック・和風・ケルトの旋律の飾りに。1 回の undo で戻る。"
    )]
    async fn add_ornament(
        &self,
        params: Parameters<AddOrnamentParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        use glaux_core::{gesture, melody, ornament};
        let _activity = self.handle.begin_activity("add_ornament");
        let p = params.0;
        let kind = ornament::Ornament::parse(&p.kind).ok_or_else(|| {
            format!(
                "kind は acciaccatura / appoggiatura / mordent / inverted_mordent / turn / trill / schleifer(got: {})",
                p.kind
            )
        })?;
        let target = match &p.target {
            Some(t) => gesture::Target::parse(t).ok_or_else(|| {
                format!("target は phrase_start / phrase_end / leap_up / long / all(got: {t})")
            })?,
            None => match kind {
                ornament::Ornament::Acciaccatura | ornament::Ornament::Schleifer => {
                    gesture::Target::LeapUp
                }
                ornament::Ornament::Appoggiatura => gesture::Target::PhraseEnd,
                _ => gesture::Target::Long,
            },
        };
        let prob = if p.note_ids.is_some() {
            1.0
        } else {
            p.probability.unwrap_or(0.5).clamp(0.0, 1.0)
        };
        let ids = clip_id_list(&p.clip_id, &p.clip_ids)?;
        let (project, version) = self.handle.get_project().await?;
        let mut rng = p.seed.unwrap_or(1).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut unit = || {
            rng ^= rng >> 12;
            rng ^= rng << 25;
            rng ^= rng >> 27;
            (rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut commands = Vec::new();
        let mut total = 0usize;
        let mut skipped = 0usize;
        for id in &ids {
            let cid = glaux_core::ClipId::parse(id).map_err(|e| e.to_string())?;
            let (_, clip) = project
                .clip(&cid)
                .ok_or_else(|| format!("クリップが見つかりません: {id}"))?;
            let notes = clip
                .notes()
                .ok_or_else(|| format!("MIDI クリップではありません: {id}"))?;
            if notes.is_empty() {
                continue;
            }
            let key = match &p.key {
                Some(k) => glaux_core::chord::Key::parse(k)
                    .ok_or_else(|| format!("key は \"C major\" の形(got: {k})"))?,
                None => melody::guess_key(
                    &notes
                        .iter()
                        .map(|n| melody::MelNote {
                            pos: n.pos.0,
                            dur: n.dur.0,
                            pitch: n.pitch,
                        })
                        .collect::<Vec<_>>(),
                ),
            };
            let scale =
                glaux_core::transform::Scale::new(glaux_core::harmony::scale_pitch_classes(
                    key.tonic,
                    if key.minor { "minor" } else { "major" },
                ));
            let bpm = project.tempo_map.bpm_at(clip.start);
            let ms_tick = |ms: f64| (ms * glaux_core::PPQ as f64 * bpm / 60_000.0).round() as u64;
            // 8 分の 20%(45〜125ms)
            let eighth_ms = 30_000.0 / bpm;
            let grace_ms = p
                .grace_ms
                .unwrap_or((eighth_ms * 0.2).clamp(45.0, 125.0))
                .clamp(20.0, 250.0);
            let rate = p.trill_rate.unwrap_or(10.0).clamp(4.0, 16.0);
            let opts = ornament::OrnamentOptions {
                grace: ms_tick(grace_ms).max(15),
                trill_step: ms_tick(1000.0 / rate).max(15),
                trill_from_upper: p.trill_start.as_deref() == Some("upper"),
                trill_turn_end: p.trill_end.as_deref() == Some("turn"),
                vel_ratio: p.vel_ratio.unwrap_or(0.7).clamp(0.1, 1.0),
                interval: p.interval,
            };
            let chosen: Vec<usize> = match &p.note_ids {
                Some(want) => notes
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| want.iter().any(|w| w == n.id.as_str()))
                    .map(|(i, _)| i)
                    .collect(),
                None => gesture::select(notes, target, glaux_core::PPQ)
                    .into_iter()
                    .filter(|_| unit() < prob)
                    .collect(),
            };
            let prev = ornament::previous_in_line(notes);
            // 音ごとの新しい (位置, 長さ)(本音の変更と前の音の短縮が重なっても両方効くように)
            let mut moved: std::collections::BTreeMap<usize, (u64, u64)> = Default::default();
            let mut added: Vec<glaux_core::Note> = Vec::new();
            for i in chosen {
                let Some(d) = ornament::decorate(notes, i, prev[i], kind, &scale, &opts) else {
                    skipped += 1;
                    continue;
                };
                total += 1;
                if (d.main_pos, d.main_dur) != (notes[i].pos.0, notes[i].dur.0) {
                    moved.insert(i, (d.main_pos, d.main_dur));
                }
                if let Some((j, len)) = d.shorten_prev {
                    let cur = moved
                        .get(&j)
                        .copied()
                        .unwrap_or((notes[j].pos.0, notes[j].dur.0));
                    let end = notes[j].pos.0 + len;
                    moved.insert(j, (cur.0, end.saturating_sub(cur.0).min(cur.1).max(1)));
                }
                for (pos, dur, pitch, vel) in d.added {
                    added.push(glaux_core::Note {
                        id: glaux_core::NoteId::new(),
                        pos: glaux_core::Tick(pos),
                        dur: glaux_core::Tick(dur.max(1)),
                        pitch,
                        vel,
                        articulation: Default::default(),
                        pitch_curve: vec![],
                        glide_ms: None,
                        vibrato: None,
                        volume_curve: vec![],
                        brightness_curve: vec![],
                    });
                }
            }
            if !moved.is_empty() {
                commands.push(Command::UpdateNotes {
                    clip: cid.clone(),
                    changes: moved
                        .iter()
                        .map(|(&i, &(pos, dur))| {
                            glaux_core::NoteChange::new(notes[i].id.clone())
                                .pos(glaux_core::Tick(pos))
                                .dur(glaux_core::Tick(dur))
                        })
                        .collect(),
                });
            }
            if !added.is_empty() {
                commands.push(Command::AddNotes {
                    clip: cid,
                    notes: added,
                });
            }
        }
        if commands.is_empty() {
            return Ok(JsonText(json!({
                "project_version": version,
                "changed": 0,
                "skipped": skipped,
                "note": "付けられる音がありませんでした(音が短すぎるか、target・probability・note_ids を確かめる)",
            })));
        }
        let label = format!("装飾音 {}({total} ノート)", p.kind);
        let command = Command::batch(label.clone(), commands);
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["decorated"] = json!(total);
        v["skipped"] = json!(skipped);
        Ok(JsonText(v))
    }

    #[tool(
        description = "旋律を伴奏より少し先に鳴らす(メロディーのリード。ピアノでは旋律の音が 20〜40ms 早く聞こえる研究がある)。\
        和音のあるクリップはいちばん上の音だけ、単音の旋律のクリップはすべての音を lead_ms(既定 20)前へ。終わりの位置は\
        変えない(音を長くする)。mode: by_velocity で上の音と他の音の強さの差に比例させる。1 回の undo で戻る。"
    )]
    async fn melody_lead(
        &self,
        params: Parameters<MelodyLeadParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("melody_lead");
        let p = params.0;
        let lead_ms = p.lead_ms.unwrap_or(20.0);
        if !(0.0..=60.0).contains(&lead_ms) {
            return Err(format!("lead_ms は 0〜60(got: {lead_ms})"));
        }
        let by_velocity = match p.mode.as_deref().unwrap_or("fixed") {
            "fixed" => false,
            "by_velocity" => true,
            other => return Err(format!("mode は fixed / by_velocity(got: {other})")),
        };
        let ids = clip_id_list(&p.clip_id, &p.clip_ids)?;
        let (project, version) = self.handle.get_project().await?;
        let mut commands = Vec::new();
        let mut total = 0usize;
        for id in &ids {
            let cid = glaux_core::ClipId::parse(id).map_err(|e| e.to_string())?;
            let (_, clip) = project
                .clip(&cid)
                .ok_or_else(|| format!("クリップが見つかりません: {id}"))?;
            let notes = clip
                .notes()
                .ok_or_else(|| format!("MIDI クリップではありません: {id}"))?;
            let bpm = project.tempo_map.bpm_at(clip.start);
            let lead = (lead_ms * glaux_core::PPQ as f64 * bpm / 60_000.0).round() as u64;
            let changes: Vec<glaux_core::NoteChange> =
                glaux_core::ornament::melody_lead(notes, lead, by_velocity, 10)
                    .into_iter()
                    .map(|(i, pos)| {
                        let n = &notes[i];
                        glaux_core::NoteChange::new(n.id.clone())
                            .pos(glaux_core::Tick(pos))
                            .dur(glaux_core::Tick(n.dur.0 + (n.pos.0 - pos)))
                    })
                    .collect();
            total += changes.len();
            if !changes.is_empty() {
                commands.push(Command::UpdateNotes { clip: cid, changes });
            }
        }
        if commands.is_empty() {
            return Ok(JsonText(
                json!({ "project_version": version, "changed": 0 }),
            ));
        }
        let label = format!("旋律を {lead_ms:.0}ms 先に({total} ノート)");
        let command = Command::batch(label.clone(), commands);
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["changed"] = json!(total);
        Ok(JsonText(v))
    }

    #[tool(
        description = "音の中の強弱・明るさの動きを付ける(ノートごとの音量・明るさの曲線): swell(ふくらむ。弦・管・パッド)/ \
        fade(消える)/ sfz(頭を強く打って落とす)/ fp(強く入ってすぐ弱く)/ pulse(音量の揺れ)/ open(暗くから開く)/ \
        close(明るくから閉じる)。note_ids か target の規則(既定 long = 1 拍以上)で選ぶ。内蔵音源は出口で音量と\
        明るさ(低域を通す・高域を足す)を変え、CLAP 音源には 1 音ごとの VOLUME・BRIGHTNESS として送る。\
        トラック全体の動きはオートメーション(shape_automation)、1 音の中の動きはこれ。1 回の undo で戻る。"
    )]
    async fn note_dynamics(
        &self,
        params: Parameters<NoteDynamicsParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        use glaux_core::gesture;
        let _activity = self.handle.begin_activity("note_dynamics");
        let p = params.0;
        let d = gesture::Dynamic::parse(&p.kind).ok_or_else(|| {
            format!(
                "kind は swell / fade / sfz / fp / pulse / open / close(got: {})",
                p.kind
            )
        })?;
        let target = match &p.target {
            Some(t) => gesture::Target::parse(t).ok_or_else(|| {
                format!("target は phrase_start / phrase_end / leap_up / long / all(got: {t})")
            })?,
            None => gesture::Target::Long,
        };
        let (def_amount, def_ms) = d.defaults();
        let amount = if d.is_brightness() {
            p.amount.unwrap_or(def_amount).clamp(0.0, 1.0)
        } else {
            p.amount.unwrap_or(def_amount).clamp(0.0, 60.0)
        };
        let ms = p.time_ms.unwrap_or(def_ms.max(1.0)).clamp(10.0, 3000.0);
        let prob = if p.note_ids.is_some() {
            1.0
        } else {
            p.probability.unwrap_or(1.0).clamp(0.0, 1.0)
        };
        let remove = p.remove.unwrap_or(false);
        let ids = clip_id_list(&p.clip_id, &p.clip_ids)?;
        let (project, version) = self.handle.get_project().await?;
        let mut rng = p.seed.unwrap_or(1).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut unit = || {
            rng ^= rng >> 12;
            rng ^= rng << 25;
            rng ^= rng >> 27;
            (rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut commands = Vec::new();
        let mut total = 0usize;
        for id in &ids {
            let cid = glaux_core::ClipId::parse(id).map_err(|e| e.to_string())?;
            let (_, clip) = project
                .clip(&cid)
                .ok_or_else(|| format!("クリップが見つかりません: {id}"))?;
            let notes = clip
                .notes()
                .ok_or_else(|| format!("MIDI クリップではありません: {id}"))?;
            let chosen: Vec<usize> = match &p.note_ids {
                Some(want) => notes
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| want.iter().any(|w| w == n.id.as_str()))
                    .map(|(i, _)| i)
                    .collect(),
                None if remove => (0..notes.len()).collect(),
                None => gesture::select(notes, target, glaux_core::PPQ)
                    .into_iter()
                    .filter(|_| unit() < prob)
                    .collect(),
            };
            let bpm = project.tempo_map.bpm_at(clip.start);
            let time = (ms as f64 * glaux_core::PPQ as f64 * bpm / 60_000.0).round() as u64;
            let mut changes = Vec::new();
            for i in chosen {
                let n = &notes[i];
                let curve: Vec<glaux_core::CurvePoint> = if remove {
                    vec![]
                } else {
                    d.curve(n.dur.0, time, amount)
                        .into_iter()
                        .map(|(t, v, sh)| glaux_core::CurvePoint::new(glaux_core::Tick(t), v, sh))
                        .collect()
                };
                let current = if d.is_brightness() {
                    &n.brightness_curve
                } else {
                    &n.volume_curve
                };
                if &curve == current {
                    continue;
                }
                let ch = glaux_core::NoteChange::new(n.id.clone());
                changes.push(if d.is_brightness() {
                    ch.brightness_curve(curve)
                } else {
                    ch.volume_curve(curve)
                });
            }
            total += changes.len();
            if !changes.is_empty() {
                commands.push(Command::UpdateNotes { clip: cid, changes });
            }
        }
        if commands.is_empty() {
            return Ok(JsonText(json!({
                "project_version": version,
                "changed": 0,
                "note": "変わる音がありませんでした(target・note_ids を確かめる)",
            })));
        }
        let label = if remove {
            format!("音の中の動きを外す {}({total} ノート)", p.kind)
        } else {
            format!("音の中の動き {}({total} ノート)", p.kind)
        };
        let command = Command::batch(label.clone(), commands);
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["changed"] = json!(total);
        Ok(JsonText(v))
    }

    #[tool(
        description = "和音をストロークにする(同時に鳴る音を少しずつずらす): style guitar(35ms。拍の頭は下げ・裏は上げ、\
        拍の手前に幅の 2 割、後の弦ほど弱く。上げは上の 4 本だけを 0.8 倍の強さで、弦ごとの間隔 0.75 倍の速い振りで鳴らし、\
        ダウンとアップの差を出す)/ piano(ばらし 120ms)/ harp(200ms)。個々の値で上書きできる。\
        終わりの位置は変えない。write_chords の strum_ms より細かく、既存のクリップ(打ち込み・読み込んだ MIDI)にも掛けられる。\
        1 回の undo で戻る。"
    )]
    async fn strum_chord(
        &self,
        params: Parameters<StrumChordParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        use glaux_core::technique::{strum, StrumDir, StrumOptions};
        let _activity = self.handle.begin_activity("strum_chord");
        let p = params.0;
        let style = p.style.as_deref().unwrap_or("guitar");
        // (向き, 幅 ms, 手前の割合, 強さの変化, 上げで低い弦を省く)
        let (dir0, span0, anchor0, slope0, skip0) = match style {
            "guitar" => (StrumDir::Alternate, 35.0, 0.2, -0.08, true),
            "piano" => (StrumDir::Down, 120.0, 0.0, 0.0, false),
            "harp" => (StrumDir::Down, 200.0, 0.0, 0.0, false),
            other => return Err(format!("style は guitar / piano / harp(got: {other})")),
        };
        let dir = match &p.direction {
            Some(d) => StrumDir::parse(d)
                .ok_or_else(|| format!("direction は down / up / alternate(got: {d})"))?,
            None => dir0,
        };
        let span_ms = p.span_ms.unwrap_or(span0).clamp(1.0, 1000.0);
        let ids = clip_id_list(&p.clip_id, &p.clip_ids)?;
        let (project, version) = self.handle.get_project().await?;
        let mut commands = Vec::new();
        let (mut chords, mut moved, mut removed) = (0usize, 0usize, 0usize);
        for id in &ids {
            let cid = glaux_core::ClipId::parse(id).map_err(|e| e.to_string())?;
            let (_, clip) = project
                .clip(&cid)
                .ok_or_else(|| format!("クリップが見つかりません: {id}"))?;
            let notes = clip
                .notes()
                .ok_or_else(|| format!("MIDI クリップではありません: {id}"))?;
            let bpm = project.tempo_map.bpm_at(clip.start);
            let tick = |ms: f64| (ms * glaux_core::PPQ as f64 * bpm / 60_000.0).round() as u64;
            let o = StrumOptions {
                dir,
                span: tick(span_ms),
                anchor: p.anchor.unwrap_or(anchor0).clamp(0.0, 1.0),
                tension: p.tension.unwrap_or(0.0).clamp(-1.0, 1.0),
                vel_slope: p.vel_slope.unwrap_or(slope0).clamp(-0.3, 0.3),
                // up_skip_low: false(以前の引数)は全部の弦
                up_strings: match (p.up_strings, p.up_skip_low) {
                    (Some(n), _) => n.min(12),
                    (None, Some(false)) => 0,
                    _ => {
                        if skip0 {
                            4
                        } else {
                            0
                        }
                    }
                },
                up_vel: p.up_velocity.unwrap_or(if skip0 { 0.8 } else { 1.0 }),
                up_span: p.up_span.unwrap_or(if skip0 { 0.75 } else { 1.0 }),
                jitter: tick(p.jitter_ms.unwrap_or(2.0).clamp(0.0, 30.0)),
                beat: glaux_core::PPQ,
                tol: 10,
                seed: p.seed.unwrap_or(1),
            };
            let r = strum(notes, clip.start.0, &o);
            chords += r.chords;
            moved += r.moved.len();
            removed += r.removed.len();
            if !r.moved.is_empty() {
                commands.push(Command::UpdateNotes {
                    clip: cid.clone(),
                    changes: r
                        .moved
                        .iter()
                        .map(|&(i, pos, dur, vel)| {
                            glaux_core::NoteChange::new(notes[i].id.clone())
                                .pos(glaux_core::Tick(pos))
                                .dur(glaux_core::Tick(dur))
                                .vel(vel)
                        })
                        .collect(),
                });
            }
            if !r.removed.is_empty() {
                commands.push(Command::RemoveNotes {
                    clip: cid,
                    ids: r.removed.iter().map(|&i| notes[i].id.clone()).collect(),
                });
            }
        }
        if commands.is_empty() {
            return Ok(JsonText(json!({
                "project_version": version,
                "chords": chords,
                "changed": 0,
                "note": "同時に鳴る和音がありませんでした",
            })));
        }
        let label = format!("ストローク {style}({chords} 和音)");
        let command = Command::batch(label.clone(), commands);
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["chords"] = json!(chords);
        v["changed"] = json!(moved);
        v["removed"] = json!(removed);
        Ok(JsonText(v))
    }

    #[tool(
        description = "ドラムのルーディメントを付ける: flam(装飾音 1 つ。25ms 前、強さ半分)/ drag(2 つ)/ ruff(3 つ)/\
        ratchet(音の長さを count 回に割る)は音に付け(note_ids か pitch の音すべて)、roll(スネアのロール)/ hat_roll\
        (トラップのハットの連打)/ buzz(バズロール)は bar・beat・length_beats の区間を rate で埋める(区間の同じ音程の\
        音は置き換える)。vel_curve で強さを傾ける(正でだんだん強く)。1 秒 22 打を超えると warning。1 回の undo で戻る。"
    )]
    async fn drum_rudiment(
        &self,
        params: Parameters<DrumRudimentParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        use glaux_core::technique::{fill, grace_notes, Rudiment};
        let _activity = self.handle.begin_activity("drum_rudiment");
        let p = params.0;
        let kind_name = p.kind.trim().to_lowercase();
        let kind = Rudiment::parse(&kind_name).ok_or_else(|| {
            format!(
                "kind は flam / drag / ruff / ratchet / roll / hat_roll / buzz(got: {})",
                p.kind
            )
        })?;
        let cid = glaux_core::ClipId::parse(&p.clip_id).map_err(|e| e.to_string())?;
        let (project, _) = self.handle.get_project().await?;
        let (_, clip) = project.clip(&cid).ok_or("クリップが見つかりません")?;
        let notes = clip.notes().ok_or("MIDI クリップではありません")?;
        let bpm = project.tempo_map.bpm_at(clip.start);
        let tick = |ms: f64| (ms * glaux_core::PPQ as f64 * bpm / 60_000.0).round() as u64;
        let new_note = |pos: u64, dur: u64, pitch: u8, vel: u8| glaux_core::Note {
            id: glaux_core::NoteId::new(),
            pos: glaux_core::Tick(pos),
            dur: glaux_core::Tick(dur.max(1)),
            pitch,
            vel,
            articulation: Default::default(),
            pitch_curve: vec![],
            glide_ms: None,
            vibrato: None,
            volume_curve: vec![],
            brightness_curve: vec![],
        };
        let mut added: Vec<glaux_core::Note> = Vec::new();
        let mut changes: Vec<glaux_core::NoteChange> = Vec::new();
        let mut remove_ids: Vec<glaux_core::NoteId> = Vec::new();
        let mut warning = None;
        let mut rng = p.seed.unwrap_or(1).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut unit = || {
            rng ^= rng >> 12;
            rng ^= rng << 25;
            rng ^= rng >> 27;
            (rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut count_done = 0usize;
        if kind.on_notes() {
            let pitch = p.pitch.unwrap_or(38);
            let prob = if p.note_ids.is_some() {
                1.0
            } else {
                p.probability.unwrap_or(1.0).clamp(0.0, 1.0)
            };
            let targets: Vec<&glaux_core::Note> = match &p.note_ids {
                Some(want) => notes
                    .iter()
                    .filter(|n| want.iter().any(|w| w == n.id.as_str()))
                    .collect(),
                None => notes.iter().filter(|n| n.pitch == pitch).collect(),
            };
            let targets: Vec<&glaux_core::Note> =
                targets.into_iter().filter(|_| unit() < prob).collect();
            for n in targets {
                if kind == Rudiment::Ratchet {
                    let count = p.count.unwrap_or(3).clamp(2, 8) as u64;
                    let slot = n.dur.0 / count;
                    if slot < 15 {
                        continue;
                    }
                    let hits = fill(n.dur.0, slot, n.vel, p.vel_curve.unwrap_or(0), 0.0, 1);
                    changes.push(
                        glaux_core::NoteChange::new(n.id.clone())
                            .dur(glaux_core::Tick((slot * 4 / 5).max(1))),
                    );
                    for &(off, v) in hits.iter().skip(1).take(count as usize - 1) {
                        added.push(new_note(n.pos.0 + off, slot * 4 / 5, n.pitch, v));
                    }
                } else {
                    let default_ms = match kind {
                        Rudiment::Flam => 25.0,
                        Rudiment::Drag => 20.0,
                        _ => 18.0,
                    };
                    let g = tick(p.grace_ms.unwrap_or(default_ms).clamp(5.0, 80.0)).max(1);
                    for (back, v) in
                        grace_notes(kind, n.vel, g, p.grace_vel.unwrap_or(0.5).clamp(0.1, 1.0))
                    {
                        if back > n.pos.0 {
                            continue; // クリップの頭より前には置けない
                        }
                        added.push(new_note(n.pos.0 - back, g.min(120), n.pitch, v));
                    }
                }
                count_done += 1;
            }
        } else {
            let bar = p.bar.ok_or(
                "roll・hat_roll・buzz は bar(と beat・length_beats)で区間を指定してください",
            )?;
            let (bar_start, _) = glaux_core::arrange::bar_range(&project, bar.max(1), 1)
                .ok_or("小節を数えられません")?;
            let start_abs = bar_start
                + ((p.beat.unwrap_or(1.0).max(1.0) - 1.0) * glaux_core::PPQ as f64).round() as u64;
            let len = ((p.length_beats.unwrap_or(1.0).clamp(0.125, 64.0)) * glaux_core::PPQ as f64)
                .round() as u64;
            let (cs, ce) = (clip.start.0, clip.start.0 + clip.length.0);
            if start_abs < cs || start_abs >= ce {
                return Err("区間がクリップの外です(クリップの範囲の小節・拍を指定する)".to_owned());
            }
            let len = len.min(ce - start_abs);
            let pitch = p
                .pitch
                .unwrap_or(if kind_name == "hat_roll" { 42 } else { 38 });
            let default_rate = if kind == Rudiment::Buzz {
                "1/64"
            } else {
                "1/32"
            };
            let rate = p
                .rate
                .as_deref()
                .unwrap_or(default_rate)
                .trim()
                .to_lowercase();
            let step = match rate.as_str() {
                "1/16" => 240,
                "1/32" => 120,
                "1/64" => 60,
                "1/16t" => 160,
                "1/32t" => 80,
                "1/8" => 480,
                r if r.ends_with("hz") => {
                    let hz: f64 = r
                        .trim_end_matches("hz")
                        .trim()
                        .parse()
                        .map_err(|_| format!("rate が読めません: {r}"))?;
                    tick(1000.0 / hz.clamp(1.0, 60.0)).max(10)
                }
                r => {
                    return Err(format!(
                        "rate は 1/16 / 1/32 / 1/64 / 1/16t / 1/32t / 1/8 か \"24hz\"(got: {r})"
                    ))
                }
            };
            let per_sec = 1000.0 / (step as f64 * 60_000.0 / (glaux_core::PPQ as f64 * bpm));
            if per_sec > 22.0 && kind != Rudiment::Buzz {
                warning = Some(format!(
                    "1 秒に {per_sec:.0} 打(人間の叩ける目安 22 打を超える)。機械的に聞かせたいならそのままでよい"
                ));
            }
            let (vel, curve, jitter) = if kind == Rudiment::Buzz {
                (p.velocity.unwrap_or(45), p.vel_curve.unwrap_or(0), 0.15)
            } else {
                (p.velocity.unwrap_or(70), p.vel_curve.unwrap_or(64), 0.05)
            };
            let rel = start_abs - cs;
            // 区間の同じ音程の音は置き換える
            for n in notes {
                if n.pitch == pitch && n.pos.0 >= rel && n.pos.0 < rel + len {
                    remove_ids.push(n.id.clone());
                }
            }
            let dur = if kind == Rudiment::Buzz {
                step
            } else {
                (step / 2).max(10)
            };
            for (off, v) in fill(
                len,
                step,
                vel.clamp(1, 127),
                curve,
                jitter,
                p.seed.unwrap_or(1),
            ) {
                added.push(new_note(rel + off, dur, pitch, v));
            }
            count_done = added.len();
        }
        let mut commands = Vec::new();
        if !remove_ids.is_empty() {
            commands.push(Command::RemoveNotes {
                clip: cid.clone(),
                ids: remove_ids.clone(),
            });
        }
        if !changes.is_empty() {
            commands.push(Command::UpdateNotes {
                clip: cid.clone(),
                changes,
            });
        }
        if !added.is_empty() {
            commands.push(Command::AddNotes {
                clip: cid.clone(),
                notes: added.clone(),
            });
        }
        if commands.is_empty() {
            return Err("付ける音がありません(pitch・note_ids・区間を確かめる)".to_owned());
        }
        let label = format!("ルーディメント {kind_name}");
        let command = Command::batch(label.clone(), commands);
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["added"] = json!(added.len());
        v["replaced"] = json!(remove_ids.len());
        v["targets"] = json!(count_done);
        if let Some(w) = warning {
            v["warning"] = json!(w);
        }
        Ok(JsonText(v))
    }

    #[tool(
        description = "音の切り方をそろえる(次の音までの間隔で長さを決める): legato(15ms 重ねる)/ tenuto(目いっぱい)/ \
        portato(75%)/ staccato(50%)。同じ音が続くときは 30ms 空けて弾き直しを聞かせる。legato + slur で奏法 legato も付けて\
        弾き直さずにつなぐ。打ち込みの長さがばらばら・全部同じで機械っぽいときに。1 回の undo で戻る。"
    )]
    async fn articulate_notes(
        &self,
        params: Parameters<ArticulateNotesParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        use glaux_core::technique::{articulate, ArticStyle};
        let _activity = self.handle.begin_activity("articulate_notes");
        let p = params.0;
        let style = ArticStyle::parse(&p.style).ok_or_else(|| {
            format!(
                "style は legato / tenuto / portato / staccato(got: {})",
                p.style
            )
        })?;
        let ratio = p.ratio.unwrap_or(style.default_ratio()).clamp(0.1, 1.0);
        let slur = p.slur.unwrap_or(false) && style == ArticStyle::Legato;
        let ids = clip_id_list(&p.clip_id, &p.clip_ids)?;
        let (project, version) = self.handle.get_project().await?;
        let mut commands = Vec::new();
        let mut total = 0usize;
        for id in &ids {
            let cid = glaux_core::ClipId::parse(id).map_err(|e| e.to_string())?;
            let (_, clip) = project
                .clip(&cid)
                .ok_or_else(|| format!("クリップが見つかりません: {id}"))?;
            let notes = clip
                .notes()
                .ok_or_else(|| format!("MIDI クリップではありません: {id}"))?;
            let bpm = project.tempo_map.bpm_at(clip.start);
            let tick = |ms: f64| (ms * glaux_core::PPQ as f64 * bpm / 60_000.0).round() as u64;
            let targets: Vec<usize> = match &p.note_ids {
                Some(want) => (0..notes.len())
                    .filter(|&i| want.iter().any(|w| w == notes[i].id.as_str()))
                    .collect(),
                None => (0..notes.len()).collect(),
            };
            let lens = articulate(
                notes,
                &targets,
                style,
                ratio,
                tick(p.overlap_ms.unwrap_or(15.0).clamp(0.0, 200.0)),
                tick(p.repeat_gap_ms.unwrap_or(30.0).clamp(0.0, 200.0)),
            );
            let mut changes: std::collections::BTreeMap<usize, glaux_core::NoteChange> = lens
                .into_iter()
                .map(|(i, d)| {
                    (
                        i,
                        glaux_core::NoteChange::new(notes[i].id.clone()).dur(glaux_core::Tick(d)),
                    )
                })
                .collect();
            if slur {
                let starts = glaux_core::gesture::select(
                    notes,
                    glaux_core::gesture::Target::PhraseStart,
                    glaux_core::PPQ,
                );
                for &i in &targets {
                    if starts.contains(&i)
                        || notes[i].articulation == glaux_core::Articulation::Legato
                    {
                        continue;
                    }
                    let ch = changes
                        .remove(&i)
                        .unwrap_or_else(|| glaux_core::NoteChange::new(notes[i].id.clone()));
                    let mut ch = ch;
                    ch.articulation = Some(glaux_core::Articulation::Legato);
                    changes.insert(i, ch);
                }
            }
            total += changes.len();
            if !changes.is_empty() {
                commands.push(Command::UpdateNotes {
                    clip: cid,
                    changes: changes.into_values().collect(),
                });
            }
        }
        if commands.is_empty() {
            return Ok(JsonText(
                json!({ "project_version": version, "changed": 0 }),
            ));
        }
        let label = format!("音の切り方 {}({total} ノート)", p.style);
        let command = Command::batch(label.clone(), commands);
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["changed"] = json!(total);
        Ok(JsonText(v))
    }

    #[tool(
        description = "トレモロにする(音を細かく連打する): single(同じ音。弦の刻み・マンドリン)/ alternating(上の音と交互。\
        キーの 3 度上か interval)/ chord(和音の連打)。division(既定 1/32)か rate_hz(数えないトレモロ。12Hz 前後)。\
        note_ids 省略で 1 拍以上の音。1 打目はそのままの強さ、あとは少し弱く交互に。1 回の undo で戻る。"
    )]
    async fn tremolo(
        &self,
        params: Parameters<TremoloParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        use glaux_core::technique::{chord_groups, tremolo_hits, Tremolo};
        let _activity = self.handle.begin_activity("tremolo");
        let p = params.0;
        let kind = Tremolo::parse(&p.kind)
            .ok_or_else(|| format!("kind は single / alternating / chord(got: {})", p.kind))?;
        let cid = glaux_core::ClipId::parse(&p.clip_id).map_err(|e| e.to_string())?;
        let (project, _) = self.handle.get_project().await?;
        let (_, clip) = project.clip(&cid).ok_or("クリップが見つかりません")?;
        let notes = clip.notes().ok_or("MIDI クリップではありません")?;
        let bpm = project.tempo_map.bpm_at(clip.start);
        let step = match (&p.division, p.rate_hz) {
            (_, Some(hz)) => {
                let hz = hz.clamp(4.0, 40.0);
                (glaux_core::PPQ as f64 * bpm / 60.0 / hz).round() as u64
            }
            (d, None) => match d.as_deref().unwrap_or("1/32") {
                "1/16" => 240,
                "1/32" => 120,
                "1/64" => 60,
                "1/16t" => 160,
                "1/32t" => 80,
                other => {
                    return Err(format!(
                        "division は 1/16 / 1/32 / 1/64 / 1/16t / 1/32t(got: {other})"
                    ))
                }
            },
        }
        .max(20);
        let key = match &p.key {
            Some(k) => glaux_core::chord::Key::parse(k)
                .ok_or_else(|| format!("key は \"C major\" の形(got: {k})"))?,
            None => glaux_core::melody::guess_key(
                &notes
                    .iter()
                    .map(|n| glaux_core::melody::MelNote {
                        pos: n.pos.0,
                        dur: n.dur.0,
                        pitch: n.pitch,
                    })
                    .collect::<Vec<_>>(),
            ),
        };
        let scale = glaux_core::transform::Scale::new(glaux_core::harmony::scale_pitch_classes(
            key.tonic,
            if key.minor { "minor" } else { "major" },
        ));
        let min_len = (p.min_beats.unwrap_or(1.0).max(0.0) * glaux_core::PPQ as f64) as u64;
        let chosen = |n: &glaux_core::Note| match &p.note_ids {
            Some(want) => want.iter().any(|w| w == n.id.as_str()),
            None => n.dur.0 >= min_len,
        };
        let targets: Vec<usize> = match kind {
            Tremolo::Chord => chord_groups(notes, 10)
                .into_iter()
                .filter(|g| g.iter().any(|&i| chosen(&notes[i])))
                .flatten()
                .collect(),
            _ => (0..notes.len()).filter(|&i| chosen(&notes[i])).collect(),
        };
        let mut changes = Vec::new();
        let mut added = Vec::new();
        for i in targets {
            let n = &notes[i];
            let hits = tremolo_hits(n.dur.0, step, kind == Tremolo::Alternating);
            if hits.len() < 2 {
                continue;
            }
            let upper = match p.interval {
                Some(iv) => (n.pitch as i32 + iv).clamp(0, 127) as u8,
                None => scale.shift(n.pitch, 2).clamp(0, 127) as u8,
            };
            changes
                .push(glaux_core::NoteChange::new(n.id.clone()).dur(glaux_core::Tick(hits[0].1)));
            for &(off, dur, vr, up) in hits.iter().skip(1) {
                added.push(glaux_core::Note {
                    id: glaux_core::NoteId::new(),
                    pos: glaux_core::Tick(n.pos.0 + off),
                    dur: glaux_core::Tick(dur),
                    pitch: if up { upper } else { n.pitch },
                    vel: ((n.vel as f64 * vr).round() as u8).max(1),
                    articulation: Default::default(),
                    pitch_curve: vec![],
                    glide_ms: None,
                    vibrato: None,
                    volume_curve: vec![],
                    brightness_curve: vec![],
                });
            }
        }
        if changes.is_empty() {
            return Err(
                "トレモロにできる音がありません(音が短いか、note_ids・min_beats を確かめる)"
                    .to_owned(),
            );
        }
        let count = changes.len();
        let added_n = added.len();
        let label = format!("トレモロ {}({count} ノート)", p.kind);
        let command = Command::batch(
            label.clone(),
            vec![
                Command::UpdateNotes {
                    clip: cid.clone(),
                    changes,
                },
                Command::AddNotes {
                    clip: cid,
                    notes: added,
                },
            ],
        );
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["notes"] = json!(count);
        v["added"] = json!(added_n);
        v["step_ticks"] = json!(step);
        Ok(JsonText(v))
    }

    #[tool(
        description = "グリッサンドを付ける(note_ids の音から to へ。省略で旋律の次の音): mode steps(音を並べる。ピアノ・ハープ。\
        scale: key / white / black / chromatic)/ continuous(音程の曲線で滑らかに。弦・トロンボーン・シンセ)。\
        portion で始める音の後ろのどれだけを使うか。1 回の undo で戻る。"
    )]
    async fn glissando(
        &self,
        params: Parameters<GlissandoParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        use glaux_core::technique::glissando_pitches;
        let _activity = self.handle.begin_activity("glissando");
        let p = params.0;
        let continuous = match p.mode.as_deref().unwrap_or("steps") {
            "steps" => false,
            "continuous" => true,
            other => return Err(format!("mode は steps / continuous(got: {other})")),
        };
        let cid = glaux_core::ClipId::parse(&p.clip_id).map_err(|e| e.to_string())?;
        let (project, _) = self.handle.get_project().await?;
        let (_, clip) = project.clip(&cid).ok_or("クリップが見つかりません")?;
        let notes = clip.notes().ok_or("MIDI クリップではありません")?;
        let key = match &p.key {
            Some(k) => glaux_core::chord::Key::parse(k)
                .ok_or_else(|| format!("key は \"C major\" の形(got: {k})"))?,
            None => glaux_core::melody::guess_key(
                &notes
                    .iter()
                    .map(|n| glaux_core::melody::MelNote {
                        pos: n.pos.0,
                        dur: n.dur.0,
                        pitch: n.pitch,
                    })
                    .collect::<Vec<_>>(),
            ),
        };
        let pcs: Vec<u8> = match p.scale.as_deref().unwrap_or("key") {
            "key" => glaux_core::harmony::scale_pitch_classes(
                key.tonic,
                if key.minor { "minor" } else { "major" },
            ),
            "white" => vec![0, 2, 4, 5, 7, 9, 11],
            "black" => vec![1, 3, 6, 8, 10],
            "chromatic" => vec![],
            other => {
                return Err(format!(
                    "scale は key / white / black / chromatic(got: {other})"
                ))
            }
        };
        let portion = p
            .portion
            .unwrap_or(if continuous { 0.5 } else { 1.0 })
            .clamp(0.1, 1.0);
        let to_fixed = match &p.to {
            Some(t) => Some(parse_pitch(Some(t), 60)?),
            None => None,
        };
        let prev_next = {
            // 旋律の次の音(同じ位置は一番上)
            let mut order: Vec<usize> = (0..notes.len()).collect();
            order.sort_by_key(|&i| (notes[i].pos, std::cmp::Reverse(notes[i].pitch)));
            order.dedup_by_key(|i| notes[*i].pos);
            order
        };
        let mut changes = Vec::new();
        let mut added = Vec::new();
        for id in &p.note_ids {
            let Some(i) = notes.iter().position(|n| n.id.as_str() == id) else {
                return Err(format!("ノートが見つかりません: {id}"));
            };
            let n = &notes[i];
            let to = match to_fixed {
                Some(t) => t,
                None => {
                    let k = prev_next.iter().position(|&j| notes[j].pos == n.pos);
                    let nx = k
                        .and_then(|k| prev_next.get(k + 1))
                        .map(|&j| notes[j].pitch);
                    nx.ok_or_else(|| format!("{id} の次の音がありません(to を指定する)"))?
                }
            };
            if to == n.pitch {
                continue;
            }
            let run_len = ((n.dur.0 as f64 * portion).round() as u64).max(1);
            let run_start = n.pos.0 + n.dur.0 - run_len;
            if continuous {
                let cents = ((to as f32 - n.pitch as f32) * 100.0).clamp(-2400.0, 2400.0);
                let mut curve: Vec<glaux_core::PitchPoint> = n
                    .pitch_curve
                    .iter()
                    .filter(|q| q.tick.0 < run_start - n.pos.0)
                    .copied()
                    .collect();
                curve.push(glaux_core::PitchPoint::shaped(
                    glaux_core::Tick(run_start - n.pos.0),
                    0.0,
                    glaux_core::CurveShape::EaseIn,
                ));
                curve.push(glaux_core::PitchPoint::new(n.dur, cents));
                curve.truncate(glaux_core::MAX_PITCH_POINTS);
                changes.push(glaux_core::NoteChange::new(n.id.clone()).pitch_curve(curve));
            } else {
                let ps: Vec<u8> = glissando_pitches(n.pitch, to, &pcs)
                    .into_iter()
                    .filter(|&q| q != to)
                    .collect();
                if ps.is_empty() {
                    continue;
                }
                // 始める音は run_start まで(portion 1 なら 1 音ぶん)、残りを並べる
                let slots = ps.len() as u64 + if portion >= 1.0 { 1 } else { 0 };
                let step = (run_len / slots).max(10);
                let first_len = if portion >= 1.0 {
                    step
                } else {
                    run_start - n.pos.0
                };
                changes.push(
                    glaux_core::NoteChange::new(n.id.clone())
                        .dur(glaux_core::Tick(first_len.max(1))),
                );
                let base = if portion >= 1.0 {
                    n.pos.0 + step
                } else {
                    run_start
                };
                for (k, &q) in ps.iter().enumerate() {
                    let pos = base + k as u64 * step;
                    if pos >= n.pos.0 + n.dur.0 {
                        break;
                    }
                    added.push(glaux_core::Note {
                        id: glaux_core::NoteId::new(),
                        pos: glaux_core::Tick(pos),
                        dur: glaux_core::Tick(step.min(n.pos.0 + n.dur.0 - pos)),
                        pitch: q,
                        vel: ((n.vel as f64 * (0.85 + 0.15 * k as f64 / ps.len() as f64)).round()
                            as u8)
                            .max(1),
                        articulation: Default::default(),
                        pitch_curve: vec![],
                        glide_ms: None,
                        vibrato: None,
                        volume_curve: vec![],
                        brightness_curve: vec![],
                    });
                }
            }
        }
        if changes.is_empty() {
            return Err(
                "グリッサンドにできる音がありません(行き先が同じ音か、音が見つからない)".to_owned(),
            );
        }
        let mut commands = vec![Command::UpdateNotes {
            clip: cid.clone(),
            changes,
        }];
        let added_n = added.len();
        if !added.is_empty() {
            commands.push(Command::AddNotes {
                clip: cid,
                notes: added,
            });
        }
        let label = format!(
            "グリッサンド({})",
            if continuous {
                "滑らか"
            } else {
                "音を並べる"
            }
        );
        let command = Command::batch(label.clone(), commands);
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["added"] = json!(added_n);
        Ok(JsonText(v))
    }

    #[tool(
        description = "句に呼吸を付ける(テンポの変化として書く): 句の半ばへ少し速く(arc)、終わりをリタルダンドで緩める\
        (Friberg らの式。final_tempo 倍まで、ritard_beats 拍かけて)。強さもテンポに連動させる(速い所は強く、緩む所は弱く。\
        Todd のモデル)。曲の終わり・バラードの句の切れ目・サビ前の溜めに。区間の中のテンポの変更は置き換え、区間の後は元のテンポに\
        戻す。ノートの位置(tick)は変わらず、秒の長さだけ変わる。1 回の undo で戻る。"
    )]
    async fn shape_phrase(
        &self,
        params: Parameters<ShapePhraseParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("shape_phrase");
        let p = params.0;
        let bars = p.bars.unwrap_or(4).clamp(1, 64);
        let (project, _) = self.handle.get_project().await?;
        let (start, len) = glaux_core::arrange::bar_range(&project, p.bar.max(1), bars)
            .ok_or("小節を数えられません")?;
        let arc = p.arc.unwrap_or(0.5).clamp(0.0, 1.0);
        let ritard = ((p.ritard_beats.unwrap_or(4.0).clamp(0.0, 64.0)) * glaux_core::PPQ as f64)
            .round() as u64;
        let w = p.final_tempo.unwrap_or(0.7).clamp(0.2, 1.0);
        let q = p.curve.unwrap_or(2.0).clamp(0.5, 5.0);
        let shape =
            glaux_core::technique::phrase_tempo(start, len, arc, ritard, w, q, glaux_core::PPQ / 2);
        let end = start + len;
        // テンポ: 区間の中の変更は置き換え、区間の後は元のテンポ
        let events = project.tempo_map.events();
        let base_at = |t: u64| project.tempo_map.bpm_at(glaux_core::Tick(t));
        let mut out: Vec<glaux_core::TempoEvent> = events
            .iter()
            .filter(|e| e.tick.0 < start || e.tick.0 >= end)
            .copied()
            .collect();
        for &(t, m) in &shape {
            out.push(glaux_core::TempoEvent {
                tick: glaux_core::Tick(t),
                bpm: ((base_at(t) * m) * 100.0).round() / 100.0,
            });
        }
        if !events.iter().any(|e| e.tick.0 == end) {
            out.push(glaux_core::TempoEvent {
                tick: glaux_core::Tick(end),
                bpm: base_at(end),
            });
        }
        out.sort_by_key(|e| e.tick);
        out.dedup_by(|b, a| a.bpm == b.bpm);
        let mut commands = vec![Command::SetTempo { events: out }];
        // 強さ: 速い所は強く、緩む所は弱く
        let mut touched = 0usize;
        if p.couple_dynamics.unwrap_or(true) {
            let mult_at = |t: u64| shape.iter().rev().find(|x| x.0 <= t).map_or(1.0, |x| x.1);
            for track in &project.tracks {
                for clip in &track.clips {
                    let Some(notes) = clip.notes() else { continue };
                    if clip.loop_len().is_some() {
                        continue;
                    }
                    let changes: Vec<glaux_core::NoteChange> = notes
                        .iter()
                        .filter_map(|n| {
                            let abs = clip.start.0 + n.pos.0;
                            if abs < start || abs >= end {
                                return None;
                            }
                            let k = 1.0 + 1.5 * (mult_at(abs) - 1.0);
                            let v = ((n.vel as f64 * k).round()).clamp(1.0, 127.0) as u8;
                            (v != n.vel).then(|| glaux_core::NoteChange::new(n.id.clone()).vel(v))
                        })
                        .collect();
                    touched += changes.len();
                    if !changes.is_empty() {
                        commands.push(Command::UpdateNotes {
                            clip: clip.id.clone(),
                            changes,
                        });
                    }
                }
            }
        }
        let label = format!("句の呼吸({} 小節目から {bars} 小節)", p.bar);
        let command = Command::batch(label.clone(), commands);
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let (after, _) = self.handle.get_project().await?;
        let secs = |t: u64| after.tempo_map.tick_to_seconds(glaux_core::Tick(t));
        let before_secs = |t: u64| project.tempo_map.tick_to_seconds(glaux_core::Tick(t));
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["tempo_events"] = json!(shape.len());
        v["notes_reweighted"] = json!(touched);
        v["seconds"] = json!({
            "before": ((before_secs(end) - before_secs(start)) * 100.0).round() / 100.0,
            "after": ((secs(end) - secs(start)) * 100.0).round() / 100.0,
        });
        Ok(JsonText(v))
    }

    #[tool(
        description = "メトリック・モジュレーション: 前のテンポのある音価を、新しいテンポの別の音価と同じ長さにしてテンポを変える\
        (from triplet_8th・to 8th で 1.5 倍に速く、from dotted_quarter・to quarter で 2/3 倍に遅く。ratio \"3:2\" でも)。\
        bar の頭にテンポの変更を置き、time_sig で拍子も変えられる(12/8 → 4/4 など)。1 回の undo で戻る。"
    )]
    async fn metric_modulation(
        &self,
        params: Parameters<MetricModulationParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        use glaux_core::meter;
        let _activity = self.handle.begin_activity("metric_modulation");
        let p = params.0;
        let (project, _) = self.handle.get_project().await?;
        let (at, _) = glaux_core::arrange::bar_range(&project, p.bar.max(1), 1)
            .ok_or("小節を数えられません")?;
        let old = project
            .tempo_map
            .bpm_at(glaux_core::Tick(at.saturating_sub(1)));
        let factor = match (&p.ratio, &p.from, &p.to) {
            (Some(r), _, _) => {
                let (a, b) = r
                    .split_once(':')
                    .and_then(|(a, b)| {
                        Some((a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?))
                    })
                    .filter(|(a, b)| *a > 0.0 && *b > 0.0)
                    .ok_or_else(|| format!("ratio は \"3:2\" の形(got: {r})"))?;
                a / b
            }
            (None, Some(f), Some(t)) => {
                let fq =
                    meter::note_value(f).ok_or_else(|| format!("from の音価が読めません: {f}"))?;
                let tq =
                    meter::note_value(t).ok_or_else(|| format!("to の音価が読めません: {t}"))?;
                meter::metric_modulation(1.0, fq, tq)
            }
            _ => return Err("from と to か、ratio を指定してください".to_owned()),
        };
        let new = ((old * factor) * 100.0).round() / 100.0;
        if !(20.0..=400.0).contains(&new) {
            return Err(format!("新しいテンポ {new} BPM は範囲の外です(20〜400)"));
        }
        let mut events: Vec<glaux_core::TempoEvent> = project
            .tempo_map
            .events()
            .iter()
            .filter(|e| e.tick.0 != at)
            .copied()
            .collect();
        events.push(glaux_core::TempoEvent {
            tick: glaux_core::Tick(at),
            bpm: new,
        });
        events.sort_by_key(|e| e.tick);
        let mut commands = vec![Command::SetTempo { events }];
        if let Some(ts) = &p.time_sig {
            let ts = ts.trim();
            let (sig, g) = match ts.split_once([' ', '(']) {
                Some((a, b)) => (a, Some(b.trim_end_matches(')'))),
                None => (ts, None),
            };
            let (n, d) = sig
                .split_once('/')
                .and_then(|(n, d)| {
                    Some((n.trim().parse::<u8>().ok()?, d.trim().parse::<u8>().ok()?))
                })
                .ok_or_else(|| format!("time_sig は \"6/8\" の形(got: {ts})"))?;
            let grouping = match g.map(str::trim).filter(|g| !g.is_empty()) {
                Some(g) => Some(meter::parse_grouping(g)?),
                None => None,
            };
            let mut sigs: Vec<glaux_core::TimeSigEvent> = project
                .time_sig_map
                .iter()
                .filter(|e| e.tick.0 != at)
                .cloned()
                .collect();
            sigs.push(glaux_core::TimeSigEvent {
                tick: glaux_core::Tick(at),
                num: n,
                den: d,
                grouping,
            });
            sigs.sort_by_key(|e| e.tick);
            commands.push(Command::SetTimeSig { events: sigs });
        }
        let label = format!("{} 小節目からテンポ {old} → {new}", p.bar);
        let command = Command::batch(label.clone(), commands);
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["bpm_before"] = json!(old);
        v["bpm_after"] = json!(new);
        Ok(JsonText(v))
    }

    #[tool(
        description = "ヘミオラ: 3 拍子の 2 小節を、2 拍ずつのまとまり 3 つに聞かせる(終止の前の定番)。bar からの 2 小節で、\
        2 拍ごとの頭の音を強く、元の 2 小節目の頭を弱くする(accent)。rebar: true で拍子も 6/4(2+2+2)の 1 小節に\
        書き換える(音の位置は同じ)。1 回の undo で戻る。"
    )]
    async fn hemiola(
        &self,
        params: Parameters<HemiolaParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("hemiola");
        let p = params.0;
        let (project, _) = self.handle.get_project().await?;
        let (start, len) = glaux_core::arrange::bar_range(&project, p.bar.max(1), 2)
            .ok_or("小節を数えられません")?;
        let m0 = glaux_core::meter::meter_at(&project, start);
        if m0.num != 3 && !(m0.num == 6 && m0.den == 8) {
            return Err(format!(
                "ヘミオラは 3 拍子(3/4・3/8・6/8)の 2 小節に使う(この小節は {})",
                m0.label()
            ));
        }
        let accent = p.accent.unwrap_or(0.25).clamp(0.0, 0.5);
        let two_beats = len / 3; // 2 小節を 3 つに
        let second_bar = start + m0.len;
        let mut commands = Vec::new();
        let mut touched = 0usize;
        for track in &project.tracks {
            for clip in &track.clips {
                if let Some(ids) = &p.clip_ids {
                    if !ids.iter().any(|x| x == clip.id.as_str()) {
                        continue;
                    }
                }
                let Some(notes) = clip.notes() else { continue };
                if clip.loop_len().is_some() {
                    continue;
                }
                let changes: Vec<glaux_core::NoteChange> = notes
                    .iter()
                    .filter_map(|n| {
                        let abs = clip.start.0 + n.pos.0;
                        if abs < start || abs >= start + len {
                            return None;
                        }
                        let rel = abs - start;
                        let k = if rel % two_beats < 30 {
                            1.0 + accent
                        } else if abs.abs_diff(second_bar) < 30 {
                            1.0 - accent
                        } else {
                            return None;
                        };
                        let v = ((n.vel as f64 * k).round()).clamp(1.0, 127.0) as u8;
                        (v != n.vel).then(|| glaux_core::NoteChange::new(n.id.clone()).vel(v))
                    })
                    .collect();
                touched += changes.len();
                if !changes.is_empty() {
                    commands.push(Command::UpdateNotes {
                        clip: clip.id.clone(),
                        changes,
                    });
                }
            }
        }
        if p.rebar.unwrap_or(false) {
            let orig = project
                .time_sig_map
                .iter()
                .rev()
                .find(|e| e.tick.0 <= start)
                .cloned()
                .unwrap_or_else(|| glaux_core::TimeSigEvent::new(glaux_core::Tick(0), 4, 4));
            let mut sigs: Vec<glaux_core::TimeSigEvent> = project
                .time_sig_map
                .iter()
                .filter(|e| e.tick.0 != start && e.tick.0 != second_bar)
                .cloned()
                .collect();
            sigs.push(glaux_core::TimeSigEvent {
                tick: glaux_core::Tick(start),
                num: m0.num * 2,
                den: m0.den,
                grouping: Some(if m0.den == 8 {
                    vec![4, 4, 4]
                } else {
                    vec![2, 2, 2]
                }),
            });
            if !project.time_sig_map.iter().any(|e| e.tick.0 == start + len) {
                sigs.push(glaux_core::TimeSigEvent {
                    tick: glaux_core::Tick(start + len),
                    ..orig
                });
            }
            sigs.sort_by_key(|e| e.tick);
            commands.push(Command::SetTimeSig { events: sigs });
        }
        if commands.is_empty() {
            return Err(
                "アクセントを付け直す音がありません(その 2 小節に音のあるクリップを確かめる)"
                    .to_owned(),
            );
        }
        let label = format!("ヘミオラ({} 小節目から 2 小節)", p.bar);
        let command = Command::batch(label.clone(), commands);
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["accented"] = json!(touched);
        Ok(JsonText(v))
    }

    #[tool(
        description = "ティハイ(インド音楽の終止): 同じ句を 3 回、間に gap_steps の休みを挟んで並べ、3 回目の最後の音が\
        land_bar の頭にちょうど着地するように置く(始まりは着地から逆算)。区間の終わり・ドロップの直前のキメに。\
        新しいクリップを作る。1 回の undo で戻る。"
    )]
    async fn write_tihai(
        &self,
        params: Parameters<WriteTihaiParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("write_tihai");
        let p = params.0;
        let tid = glaux_core::TrackId::parse(&p.track_id).map_err(|e| e.to_string())?;
        let (project, _) = self.handle.get_project().await?;
        let track = project.track(&tid).ok_or("トラックが見つかりません")?;
        if track.kind != glaux_core::TrackKind::Midi {
            return Err("MIDI トラックを指定してください".to_owned());
        }
        let phrase: Vec<char> = p
            .phrase
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '|')
            .collect();
        if phrase.is_empty()
            || phrase.len() > 64
            || phrase.iter().any(|c| !matches!(c, 'x' | 'X' | '-' | '.'))
        {
            return Err("phrase は x - . の 1〜64 文字".to_owned());
        }
        let gap = p.gap_steps.unwrap_or(2).min(32);
        let unit: u64 = match p.unit.as_deref().unwrap_or("16th") {
            "16th" => 240,
            "8th" => 480,
            "16t" => 160,
            other => return Err(format!("unit は 16th / 8th / 16t(got: {other})")),
        };
        let hits = glaux_core::meter::tihai(&phrase, gap).ok_or("phrase に音(x)がありません")?;
        let (land, _) = glaux_core::arrange::bar_range(&project, p.land_bar.max(1), 1)
            .ok_or("小節を数えられません")?;
        let first = hits.first().map_or(0, |h| h.0);
        let first_abs = land as i64 + first * unit as i64;
        if first_abs < 0 {
            return Err(
                "着地が曲の頭に近すぎて、1 回目の句が置けません(land_bar を後ろに)".to_owned(),
            );
        }
        let clip_start = first_abs as u64;
        let clip_len = land + unit - clip_start;
        let drum = is_drum_track(track);
        let pitch = parse_pitch(p.pitch.as_deref(), if drum { 38 } else { 72 })?;
        let vel = p.velocity.unwrap_or(90).clamp(1, 110);
        let last = hits.len() - 1;
        let notes: Vec<(u64, u64, u8, u8)> = hits
            .iter()
            .enumerate()
            .map(|(i, &(off, len, _, head))| {
                let pos = (land as i64 + off * unit as i64) as u64 - clip_start;
                let v = if i == last {
                    (vel + 17).min(127)
                } else if head {
                    (vel + 10).min(127)
                } else {
                    vel
                };
                (pos, (len * unit * 9 / 10).max(1), pitch, v)
            })
            .collect();
        let clip = simple_clip("Tihai".to_owned(), clip_start, clip_len, &notes);
        let clip_id = clip.id.clone();
        let label = format!("ティハイ({} 小節目に着地)", p.land_bar);
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(
            self.handle
                .apply(Command::AddClip { track: tid, clip }, author, label)
                .await,
        )?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["clip_id"] = json!(clip_id);
        v["notes"] = json!(notes.len());
        v["start_tick"] = json!(clip_start);
        v["length_steps"] = json!((land - clip_start) / unit + 1);
        Ok(JsonText(v))
    }

    #[tool(
        description = "トラックに変調(LFO)を付けて、内蔵の音源・エフェクトのつまみを揺らす(動きのある音色): target につまみ\
        (\"cutoff\"・\"auto_filter.cutoff\"・\"fx/<id>/mix\")、sync でテンポに合わせた周期(\"1/4\"・\"1/8d\"・\"1/1\")か rate_hz、\
        shape(sine / triangle / square / saw_up / saw_down / random)、depth(つまみの単位で ±)。同じつまみのオートメーションが\
        あればその値を中心に揺らす。ワブルベース = wavetable の position か subtractive の cutoff を 1/8 の sine、うねるパッド = cutoff を\
        2/1 の triangle、ランダムに動くアルペジオ = random。1 トラック 8 個まで。同じつまみは置き換える。remove で外す。1 回の undo で戻る。"
    )]
    async fn modulate(
        &self,
        params: Parameters<ModulateParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("modulate");
        let p = params.0;
        let tid = glaux_core::TrackId::parse(&p.track_id).map_err(|e| e.to_string())?;
        let (project, _) = self.handle.get_project().await?;
        let track = project.track(&tid).ok_or("トラックが見つかりません")?;
        // つまみの場所
        let resolve = |t: &str| -> Result<glaux_core::ParamPath, String> {
            if t.starts_with("fx/") || t.starts_with("device/") {
                return glaux_core::ParamPath::parse(t).map_err(|e| e.to_string());
            }
            if let Some((kind, name)) = t.split_once('.') {
                let fx = track
                    .effects
                    .iter()
                    .find(|e| matches!(&e.source, glaux_core::PluginSource::Builtin { name: n } if n == kind))
                    .ok_or_else(|| format!("このトラックに {kind} のエフェクトがありません"))?;
                return Ok(glaux_core::ParamPath::effect(fx.id.clone(), name));
            }
            Ok(glaux_core::ParamPath::device(t))
        };
        let mut mods = track.modulators.clone();
        let label;
        let mut target_json = Value::Null;
        if p.remove.unwrap_or(false) {
            match &p.target {
                Some(t) => {
                    let path = resolve(t)?;
                    let before = mods.len();
                    mods.retain(|m| m.target != path);
                    if mods.len() == before {
                        return Err(format!("{t} の変調はありません"));
                    }
                    label = format!("変調を外す({t})");
                }
                None => {
                    if mods.is_empty() {
                        return Err("変調はありません".to_owned());
                    }
                    mods.clear();
                    label = "変調をすべて外す".to_owned();
                }
            }
        } else {
            let t = p.target.as_deref().ok_or("target を指定してください")?;
            let path = resolve(t)?;
            // つまみの範囲(depth の既定と確認)
            let spec = match &path {
                glaux_core::ParamPath::Device { name } => {
                    let dev = track
                        .device
                        .as_ref()
                        .and_then(|d| match &d.source {
                            glaux_core::PluginSource::Builtin { name } => Some(name.clone()),
                            _ => None,
                        })
                        .unwrap_or_else(|| glaux_dsp::DEFAULT_INSTRUMENT.to_owned());
                    glaux_dsp::instrument_params(&dev)
                        .and_then(|ss| ss.iter().find(|s| s.name == name.as_str()))
                }
                glaux_core::ParamPath::Effect { id, name } => track
                    .effects
                    .iter()
                    .find(|e| &e.id == id)
                    .and_then(|e| match &e.source {
                        glaux_core::PluginSource::Builtin { name } => Some(name.clone()),
                        _ => None,
                    })
                    .and_then(|n| glaux_dsp::effect_params_spec(&n))
                    .and_then(|ss| ss.iter().find(|s| s.name == name.as_str())),
                _ => None,
            };
            let Some(spec) = spec else {
                return Err(format!(
                    "{t} は揺らせるつまみではありません(list_params で名前を確かめる)"
                ));
            };
            let (lo, hi) = match spec.range {
                glaux_core::ParamRange::Float { min, max, .. } => (min, max),
                glaux_core::ParamRange::Int { min, max, .. } => (min as f64, max as f64),
                _ => {
                    return Err(format!(
                        "{t} は数のつまみではありません(選択肢は揺らせない)"
                    ))
                }
            };
            let shape: glaux_core::LfoShape = match &p.shape {
                Some(sh) => serde_json::from_value(json!(sh)).map_err(|_| {
                    format!(
                        "shape は sine / triangle / square / saw_up / saw_down / random(got: {sh})"
                    )
                })?,
                None => Default::default(),
            };
            let m = glaux_core::Modulator {
                target: path.clone(),
                shape,
                sync: p.sync.clone(),
                rate_hz: p.rate_hz.unwrap_or(1.0),
                depth: p.depth.unwrap_or((hi - lo) / 4.0),
                phase: p.phase.unwrap_or(0.0),
            };
            mods.retain(|x| x.target != path);
            mods.push(m);
            glaux_core::check_modulators(&mods)?;
            label = format!("変調 {t}");
            target_json = json!({ "path": path.to_string(), "min": lo, "max": hi });
        }
        let command = Command::SetTrackProp {
            id: tid,
            prop: glaux_core::TrackProp::Modulators(mods.clone()),
        };
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["modulators"] = json!(mods);
        v["target"] = target_json;
        Ok(JsonText(v))
    }

    #[tool(
        description = "曲の計画書を書く(曲を作るときの最初の一手)。区間ごとに名前・小節数・盛り上がり(energy 0〜10)・\
        鳴らすトラックの名前・役割を渡すと、小節の頭に区間のマーカーを置き直し、各区間の始まりの小節と曲の長さ(秒)を返す\
        (「3 分の曲」の長さ合わせに使う)。拍子の変化も考慮して小節を数える。critique_arrangement は、計画の盛り上がりの\
        上がり下がりと鳴らすトラックが実際と合っているかも点検する。計画を変えたら書き直す。1 回の undo で戻る。"
    )]
    async fn set_song_plan(
        &self,
        params: Parameters<SetSongPlanParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("set_song_plan");
        let p = params.0;
        let (project, _) = self.handle.get_project().await?;
        let plan: Vec<glaux_core::arrange::PlanSection> = p
            .sections
            .into_iter()
            .map(|s| glaux_core::arrange::PlanSection {
                name: s.name,
                bars: s.bars,
                energy: s.energy,
                tracks: s.tracks.unwrap_or_default(),
                note: s.note,
            })
            .collect();
        let bars_of: Vec<u32> = plan.iter().map(|s| s.bars).collect();
        let start_bar = p.start_bar.unwrap_or(1);
        let made = glaux_core::arrange::plan_markers(&project, start_bar, &plan)?;
        let end_tick = made.last().map_or(0, |m| m.2);
        let total_bars: u32 = bars_of.iter().sum();
        let seconds = project
            .tempo_map
            .tick_to_seconds(glaux_core::Tick(end_tick));
        let sections_json: Vec<Value> = made
            .iter()
            .zip(&bars_of)
            .map(|((m, bar, _), bars)| {
                json!({ "name": m.name, "start_bar": bar, "bars": bars, "tick": m.tick,
                    "energy": m.energy, "tracks": m.tracks, "note": m.note })
            })
            .collect();
        // 今の曲の中身の終わり(計画より長い・短いを知らせる)
        let content_end = project.end().0;
        let content_bars = glaux_core::arrange::bar_grid(&project, content_end.max(1))
            .iter()
            .filter(|(s, _)| *s < content_end)
            .count();
        let label = format!("曲の計画書({} 区間・{total_bars} 小節)", made.len());
        let command = Command::SetSections {
            sections: made.into_iter().map(|(m, _, _)| m).collect(),
        };
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["sections"] = json!(sections_json);
        v["total_bars"] = json!(total_bars);
        v["end_bar"] = json!(start_bar + total_bars - 1);
        let whole = seconds.round() as u64;
        v["duration"] = json!(format!("{}:{:02}", whole / 60, whole % 60));
        v["duration_sec"] = json!((seconds * 10.0).round() / 10.0);
        v["content_bars"] = json!(content_bars);
        Ok(JsonText(v))
    }

    #[tool(
        description = "編曲を点検する(完了を報告する前に必ず使う)。楽譜だけで分かる「機械的すぎる(16 分の格子どおり・強弱が平ら)」\
        「動きが無い(オートメーションが無い)」「変化が無い(同じ小節の繰り返し)」「区間ごとの起伏が小さい」「低い音域の濁り」\
        「音域の端」「計画書(set_song_plan)と違う(盛り上がりの上がり下がり・鳴らすトラック)」を見つけ、直し方(使う道具)と一緒に返す。warn は直してから報告する。トラックごと・区間ごとの数値\
        (格子どおりの割合・ベロシティの幅・違う小節の数・区間の energy 0〜10)も返す。音の点検は analyze_audio。"
    )]
    async fn critique_arrangement(&self) -> ToolResult {
        let _activity = self.handle.begin_activity("critique_arrangement");
        let (project, version) = self.handle.get_project().await?;
        let c = glaux_core::critique::critique(&project);
        let warns = c.findings.iter().filter(|f| f.severity == "warn").count();
        let mut v = serde_json::to_value(&c).map_err(|e| e.to_string())?;
        v["project_version"] = json!(version);
        v["summary"] = json!(if c.findings.is_empty() {
            "指摘はありません".to_owned()
        } else {
            format!(
                "直した方がよい所 {warns} 件、検討する所 {} 件",
                c.findings.len() - warns
            )
        });
        Ok(JsonText(v))
    }

    #[tool(
        description = "オートメーションを「区間と形」で書く(点は道具が並べる)。ビルドアップのフィルタ、ライザー、フェード、\
        スウェル、一瞬抜く、4 分ごとのポンピング、LFO のような揺れに使う。例: {track_id, target: \"device/cutoff\", start: \"9\", bars: 8, \
        shape: \"exp\", from: 300, to: 12000} / {track_id, target: \"track/volume_db\", start: \"17\", bars: 8, shape: \"pump\", from: 0, to: -8}。\
        位置は「小節:拍」(1 始まり)。値はつまみの単位で、範囲の外は範囲に収める。区間の外の既存の点は残る。1 回の undo で戻る。\
        target に使えるつまみと範囲は list_params で確かめる。"
    )]
    async fn shape_automation(
        &self,
        params: Parameters<ShapeAutomationParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        use glaux_core::shape::{merge_points, position_to_tick, shape_points, Shape};
        let _activity = self.handle.begin_activity("shape_automation");
        let p = params.0;
        let (project, _) = self.handle.get_project().await?;
        let shape = Shape::parse(&p.shape).ok_or_else(|| {
            format!(
                "shape が不正です({})。linear / exp / log / s_curve / swell / dip / step / sine / triangle / saw_up / saw_down / pump / square",
                p.shape
            )
        })?;
        let target = glaux_core::ParamPath::parse(&p.target).map_err(|e| e.to_string())?;
        let start = position_to_tick(&project, &p.start)?;
        let end = match (&p.end, p.bars) {
            (Some(e), _) => position_to_tick(&project, e)?,
            (None, Some(b)) if b > 0.0 => {
                // 小節数は拍子に沿って数える(始まりが小節の途中なら、終わりも同じだけ途中)
                let start_bar: u32 = p
                    .start
                    .split(':')
                    .next()
                    .and_then(|x| x.trim().parse().ok())
                    .unwrap_or(1);
                let bar0 = glaux_core::arrange::bar_range(&project, start_bar, 1)
                    .ok_or("小節が求められません")?
                    .0;
                let whole = b.floor() as u32;
                let after_whole = glaux_core::arrange::bar_range(&project, start_bar + whole, 1)
                    .ok_or("小節が求められません")?;
                let frac = ((b - whole as f64) * after_whole.1 as f64).round() as u64;
                after_whole.0 + (start - bar0) + frac
            }
            _ => return Err("end か bars(0 より大きい)を指定すること".to_owned()),
        };
        if end <= start {
            return Err("終わりは始まりより後にすること".to_owned());
        }
        // 値の範囲(つまみの一覧から。音量・パンは決まった範囲)
        let (track, lanes) = match &p.track_id {
            Some(id) => {
                let tid = glaux_core::TrackId::parse(id).map_err(|e| e.to_string())?;
                let t = project
                    .track(&tid)
                    .ok_or_else(|| format!("トラックが見つかりません: {id}"))?;
                (Some(t), &t.automation)
            }
            None => (None, &project.master.automation),
        };
        let range = match p.target.as_str() {
            "track/volume_db" => Some((-60.0, 12.0)),
            "track/pan" => Some((-1.0, 1.0)),
            path => {
                let list = match track {
                    Some(t) => track_params_json(t)?,
                    None => {
                        json!({ "effects": effects_json(&project.master.effects, project.master.fx_links.as_deref()) })
                    }
                };
                find_param_range(&list, path)
            }
        };
        let mut clamped = false;
        let mut clamp = |v: f64| match range {
            Some((lo, hi)) if v < lo || v > hi => {
                clamped = true;
                v.clamp(lo, hi)
            }
            _ => v,
        };
        let (from, to) = (clamp(p.from), clamp(p.to));
        let period = ((p.period_beats.unwrap_or(1.0).max(1.0 / 32.0)) * glaux_core::PPQ as f64)
            .round() as u64;
        let pts = shape_points(start, end - start, from, to, shape, period);
        let n = pts.len();
        let existing = lanes
            .iter()
            .find(|l| l.target == target)
            .map(|l| l.points.clone())
            .unwrap_or_default();
        let points = if p.keep_outside.unwrap_or(true) {
            merge_points(&existing, pts, start, end)
        } else {
            pts
        };
        let command = match track {
            Some(t) => Command::SetAutomationPoints {
                track: t.id.clone(),
                target,
                points,
            },
            None => Command::SetMasterAutomationPoints { target, points },
        };
        let who = track.map_or("マスター".to_owned(), |t| t.name.clone());
        let label = format!(
            "{who} の {} を {} 小節目から{}の形で動かす",
            p.target, p.start, p.shape
        );
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["points"] = json!(n);
        v["range_ticks"] = json!([start, end]);
        if clamped {
            v["note"] = json!(format!(
                "値をつまみの範囲 {:?} に収めました(from {from}, to {to})",
                range.unwrap_or_default()
            ));
        }
        Ok(JsonText(v))
    }

    #[tool(
        description = "ノートにスウィング(ハネ・シャッフル)を掛ける。裏拍(grid_ticks 480 = 8 分裏、240 = 16 分裏)の音だけを\
        swing の位置へ寄せる(0.5 = ストレートに戻す、0.667 ≈ 3 連シャッフル、0.75 = 付点)。表の音と長さは変えない。\
        拍は曲頭から数える。同じ設定なら何度掛けても同じ結果。analyze_rhythm の swing_ratio(裏 8 分の位置 / 480)は\
        swing × 2 に相当する(swing_ratio 1.33 ≈ swing 0.667)。既存のノリに合わせるなら先に analyze_rhythm で測る。\
        clip_ids で曲じゅうのクリップにまとめて掛けられる(1 回の undo で戻る)。\
        ハネだけでは全部の音が同じ位置にそろって機械的なので、続けて apply_groove(quantize 0)で楽器ごとのずれと揺れを付ける。"
    )]
    async fn swing_notes(
        &self,
        params: Parameters<SwingNotesParams>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let _activity = self.handle.begin_activity("swing_notes");
        let p = params.0;
        let jazz = match p.mode.as_deref() {
            None => false,
            Some("jazz_tempo") => true,
            Some(other) => return Err(format!("mode は jazz_tempo(got: {other})")),
        };
        let fixed = match (jazz, p.swing) {
            (true, _) => None,
            (false, Some(sw)) if (0.5..=0.8).contains(&sw) => Some(sw),
            (false, Some(sw)) => return Err(format!("swing は 0.5〜0.8(got: {sw})")),
            (false, None) => {
                return Err("swing(0.5〜0.8)か mode: jazz_tempo を指定してください".to_owned())
            }
        };
        let mut used_swing = fixed.unwrap_or(0.5);
        let grid = p.grid_ticks.unwrap_or(480);
        if grid < 60 {
            return Err("grid_ticks は 60 以上にすること".to_owned());
        }
        let strength = p.strength.unwrap_or(1.0);
        if !(0.0..=1.0).contains(&strength) {
            return Err(format!("strength は 0.0〜1.0(got: {strength})"));
        }
        let mut ids: Vec<String> = p.clip_id.clone().into_iter().collect();
        for c in p.clip_ids.iter().flatten() {
            if !ids.contains(c) {
                ids.push(c.clone());
            }
        }
        if ids.is_empty() {
            return Err("clip_id か clip_ids を指定してください".to_owned());
        }
        if p.note_ids.is_some() && ids.len() > 1 {
            return Err("note_ids は clip_id を 1 つだけ指定したときに使えます".to_owned());
        }
        let (project, version) = self.handle.get_project().await?;
        let mut commands = Vec::new();
        let mut total = 0usize;
        for id in &ids {
            let (clip, len, notes, _) = self.load_notes(id, &p.note_ids).await?;
            let start = project.clip(&clip).map(|(_, c)| c.start.0).unwrap_or(0);
            let changes: Vec<_> = glaux_core::rhythm::swing_positions(
                &notes,
                start,
                len.0,
                grid,
                {
                    let sw = fixed.unwrap_or_else(|| {
                        glaux_core::technique::jazz_swing_ratio(
                            project.tempo_map.bpm_at(glaux_core::Tick(start)),
                        )
                    });
                    used_swing = sw;
                    sw
                },
                strength,
                &glaux_core::meter::bar_meters(&project, start + len.0),
            )
            .into_iter()
            .map(|(id, pos)| glaux_core::NoteChange::new(id).pos(glaux_core::Tick(pos)))
            .collect();
            total += changes.len();
            if !changes.is_empty() {
                commands.push(Command::UpdateNotes { clip, changes });
            }
        }
        if commands.is_empty() {
            return Ok(JsonText(json!({
                "project_version": version,
                "changed": 0,
                "note": "対象ノートはすべて変更不要でした",
            })));
        }
        let label = if ids.len() == 1 {
            format!(
                "スウィング {:.0}%(1/{}、{total} ノート)",
                used_swing * 100.0,
                3840 / grid
            )
        } else {
            format!(
                "スウィング {:.0}%(1/{}、{} クリップ・{total} ノート)",
                used_swing * 100.0,
                3840 / grid,
                ids.len()
            )
        };
        let command = if commands.len() == 1 {
            commands.pop().expect("1 つある")
        } else {
            Command::batch(label.clone(), commands)
        };
        let author = self.author(&ctx);
        let (entry_id, m) = flatten(self.handle.apply(command, author, label).await)?;
        let mut v = mutated_json(&m);
        v["entry_id"] = json!(entry_id);
        v["changed"] = json!(total);
        v["next"] = json!(
            "ハネだけでは音が同じ位置にそろって機械的。続けて apply_groove(style はジャンル、quantize 0、同じ clip_ids)で楽器ごとのずれと揺れを付ける"
        );
        Ok(JsonText(v))
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
