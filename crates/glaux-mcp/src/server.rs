//! MCP ツール層。
//!
//! ここは「AI が読む API ドキュメント」でもある。ツールの description には
//! いつ使うか・注意点を書く(`docs/mcp-spec-draft.md` の方針)。
//!
//! すべての編集は `glaux_core::Command` に変換して Session アクターに送る。
//! 各レスポンスには `project_version`(適用済み履歴エントリ数)を含め、
//! AI が自分の把握が古くなっていないか判断できるようにする。

use crate::actor::{Mutated, SessionHandle};
use glaux_core::{Author, Command, EntryId};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerConfig},
    service::RequestContext,
    tool, tool_handler, tool_router, Json, RoleServer, ServerHandler,
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
    /// WAV ファイルの絶対パス(ユーザーのマシン上のファイル)。WAV のみ対応。
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
    /// WAV ファイルの絶対パス(ユーザーのマシン上のファイル)。WAV のみ対応。
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
    /// これより短いノートは捨てる(ms)。既定 80。
    #[serde(default)]
    pub min_note_ms: Option<f32>,
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

type ToolResult = Result<Json<Value>, String>;

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
/// MCP の `list_params`(track_id 指定)とアプリの音作りビューが共用する。
pub fn track_params_json(track: &glaux_core::Track) -> Result<Value, String> {
    let (device_name, device_params, is_default) = match &track.device {
        Some(d) => match &d.source {
            glaux_core::PluginSource::Builtin { name } => (name.clone(), d.params.clone(), false),
            glaux_core::PluginSource::Sampler { .. } => {
                ("sampler".to_owned(), d.params.clone(), false)
            }
            glaux_core::PluginSource::Sf2 { .. } => ("sf2".to_owned(), d.params.clone(), false),
            other => {
                return Err(format!(
                    "このトラックのデバイスは対応外です({other:?})。builtin / sampler のみ対応"
                ))
            }
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
    let effects_list: Vec<Value> = track
        .effects
        .iter()
        .map(|e| {
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
        })
        .collect();

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
            return Ok(Json(json!({
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
        Ok(Json(v))
    }
}

#[tool_router]
impl GlauxServer {
    #[tool(
        description = "プロジェクト全体(トラック・クリップ・パラメータ・テンポ)を JSON で取得する。\
        ノートが多いと巨大になるので、まず include_notes: false で構造を把握し、必要な部分だけ改めて取得するとよい。\
        返り値の project_version は適用済み履歴エントリ数。編集後に増えていれば自分の把握は古い。"
    )]
    async fn get_project(&self, params: Parameters<GetProjectParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("get_project");
        let p = params.0;
        let (project, version) = self.handle.get_project().await?;
        let mut v = serde_json::to_value(&project).map_err(|e| e.to_string())?;

        if let Some(ids) = &p.track_ids {
            if let Some(tracks) = v.get_mut("tracks").and_then(Value::as_array_mut) {
                tracks.retain(|t| {
                    t.get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| ids.iter().any(|x| x == id))
                });
            }
        }
        let include_notes = p.include_notes.unwrap_or(true);
        let include_automation = p.include_automation.unwrap_or(true);
        if let Some(tracks) = v.get_mut("tracks").and_then(Value::as_array_mut) {
            for track in tracks {
                if !include_automation {
                    if let Some(a) = track.get_mut("automation") {
                        *a = json!([]);
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

        Ok(Json(json!({ "project_version": version, "project": v })))
    }

    #[tool(
        description = "コマンドを適用してプロジェクトを編集する。唯一の編集手段。\
        commands には glaux の Command JSON({\"op\": ..., ...})を並べる。複数渡すと 1 つの Batch になり、1 回の undo でまとめて戻せる。\
        新規 ID は呼び出し側が生成して渡す(トラック trk_、クリップ clp_、ノート nt_、エフェクト fx_ + 英数 6 桁。例 trk_a1b2c3)。\
        相対操作(「半音上げる」等)は不可。現在値を読んで絶対値を計算してから送ること。\
        ただしノートの移調・時間移動・クオンタイズ・ベロシティ一括調整は\
        専用ツール(transpose_notes / shift_notes / quantize_notes / scale_velocity)の方が速くて確実。\
        代表例: add_track {track,index?} / add_clip {track,clip} / add_notes {clip,notes} / update_notes {clip,changes} / \
        set_track_prop {id,prop,value} / set_param {track,path,value} / set_tempo {events} / move_clip {id,start,track?} / \
        set_title {title}(曲名の変更)/ \
        set_sections {sections: [{tick, name}]}(曲の構成マーカーを丸ごと置換。\
        intro / Aメロ / サビ 等。各セクションはそのマーカーから次のマーカーの手前まで。\
        構成を決めたら早めに打っておくと「サビだけ〜して」の指示を tick 範囲に解決できる)/ \
        ノートには articulation を付けられる: \"palm_mute\"(ブリッジミュート。減衰が速いこもった刻み)/ \
        \"staccato\"(音価半分で切る)/ \"accent\"(強く明るく)/ \
        \"vibrato\"(後半にかけて深くなるピッチの揺れ。ロングトーンの表情付け)/ \
        \"bend\"(チョーキング: 全音下から書かれた音程へ滑り上がる。ギターソロの決め音に)。\
        省略で通常。update_notes でも変更可。\
        連続ピッチカーブ: ノートの pitch_curve に [{tick, cents}](tick はノート先頭からの相対、\
        cents は書かれた音程からのずれ。100 = 半音、±2400 まで、最大 8 点、点の間は線形補間、\
        両端は保持)を書くと自由なベンド・ポルタメント・うねりが作れる\
        (例: ギターのチョーキングを 1 拍かけて上げる = [{tick:0,cents:-200},{tick:960,cents:0}]、\
        ダイブ = [{tick:0,cents:0},{tick:1920,cents:-1200}])。update_notes の pitch_curve で差し替え、[] で削除。\
        メタルの「ズクズク」した刻みは distortion + 低音 + palm_mute ノートの組み合わせで作る。\
        set_automation_points {track,target,points}(target は \"track/volume_db\" / \"track/pan\" / \
        \"device/<パラメータ名>\"(例 device/cutoff。list_params にある連続値パラメータ。\
        値はパラメータと同じ単位)、points は [{tick,value,curve?}] で curve は \
        linear/hold/exponential。フェードイン・ビルドアップの音量カーブ・左右の揺れ・\
        フィルタスイープなど時間変化する表現に使う。\
        レーンがあるとフェーダー/つまみの値より優先。空配列でレーン削除)。\
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
        for (i, value) in p.commands.into_iter().enumerate() {
            let cmd: Command = serde_json::from_value(value)
                .map_err(|e| format!("commands[{i}] を Command として解釈できません: {e}"))?;
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
        Ok(Json(v))
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
        Ok(Json(v))
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
        Ok(Json(v))
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
        Ok(Json(v))
    }

    #[tool(
        description = "チェックポイントまで編集を巻き戻す(undo の繰り返し)。巻き戻した分は redo でやり直せる。\
        チェックポイント名が存在しないとエラー。"
    )]
    async fn revert_to(&self, params: Parameters<RevertToParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("revert_to");
        let m = flatten(self.handle.revert_to(params.0.label).await)?;
        Ok(Json(mutated_json(&m)))
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
        Ok(Json(v))
    }

    #[tool(
        description = "楽器・エフェクトのパラメータ仕様と現在値を返す。つまみを理解する唯一の情報源。\
        track_id を省略するとカタログ: 内蔵楽器(subtractive / drum)と内蔵エフェクト(eq / compressor / reverb)の\
        全パラメータ仕様(範囲と聴感上の効果)を返す。\
        track_id を指定するとそのトラックの現在のデバイスとエフェクトチェーン(spec + current)を返す。\
        音源の設定は set_device(例: {\"op\":\"set_device\",\"track\":\"trk_x\",\"device\":{\"type\":\"builtin\",\"name\":\"drum\"}})、\
        エフェクト追加は add_effect(例: {\"op\":\"add_effect\",\"track\":\"trk_x\",\"effect\":{\"id\":\"fx_a1b2c3\",\"type\":\"builtin\",\"name\":\"reverb\"}})。\
        つまみは set_param で、path は楽器 \"device/<名前>\"、エフェクト \"fx/<fx_id>/<名前>\"。\
        ドラムトラックには必ず drum を設定すること(未設定は subtractive で鳴る)。"
    )]
    async fn list_params(&self, params: Parameters<ListParamsParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("list_params");
        let (project, version) = self.handle.get_project().await?;

        let Some(track_id) = params.0.track_id else {
            // カタログモード
            return Ok(Json(json!({
                "project_version": version,
                "instruments": glaux_dsp::instrument_catalog(),
                "effects": glaux_dsp::effect_catalog(),
                "default_instrument": glaux_dsp::DEFAULT_INSTRUMENT,
            })));
        };

        let track_id = glaux_core::TrackId::parse(&track_id).map_err(|e| e.to_string())?;
        let track = project
            .track(&track_id)
            .ok_or_else(|| format!("track not found: {track_id}"))?;

        let mut v = track_params_json(track)?;
        v["project_version"] = json!(version);
        v["track_id"] = json!(track_id);
        Ok(Json(v))
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
        【ミックスバランスの診断】per_track: true で各トラックの loudness/band_energy 一覧が返る。\
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
            let bank = glaux_engine::SampleBank::load(&project, std::path::Path::new(&project_dir));
            let a = glaux_engine::analyze_project(&project, track_ids.as_deref(), range, &bank);
            let t = if per_track {
                Some(glaux_engine::analyze_project_tracks(&project, range, &bank))
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
        if let Some(mut tracks) = track_summaries {
            // うるさい順に並べる(バランス診断で読みやすい)
            tracks.sort_by(|a, b| {
                b.loudness_lufs
                    .partial_cmp(&a.loudness_lufs)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            v["tracks"] = serde_json::to_value(&tracks).map_err(|e| e.to_string())?;
        }
        Ok(Json(v))
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
        Ok(Json(v))
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
        Ok(Json(v))
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
        let (entries, version) = flatten(
            self.handle
                .get_history(p.author, since, p.limit.map(|n| n as usize))
                .await,
        )?;
        Ok(Json(
            json!({ "project_version": version, "entries": entries }),
        ))
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
            None => Ok(Json(json!({
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
                Ok(Json(json!({
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
        Ok(Json(v))
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
            crate::assets::import_wav(std::path::Path::new(&dir), std::path::Path::new(&p.path))?;

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
        Ok(Json(v))
    }

    #[tool(
        description = "WAV ファイルを音声クリップとして音声トラック(kind: \"audio\")に置く。\
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
            crate::assets::import_wav(std::path::Path::new(&dir), std::path::Path::new(&p.path))?;
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
        Ok(Json(v))
    }

    #[tool(
        description = "音声クリップ(鼻歌・歌・単音のギター等の**単旋律**)を譜起こしして、\
        同じ位置・長さの MIDI クリップを作る(履歴 1 件)。和音・複数楽器・ドラムは対象外。\
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
        Ok(Json(v))
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
        Ok(Json(json!({
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
        Ok(Json(json!({
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
        Ok(Json(v))
    }

    #[tool(
        description = "プリセットをライブラリから削除する(元に戻せない。undo の対象外)。\
        ユーザーに頼まれたときだけ使うこと。"
    )]
    async fn delete_preset(&self, params: Parameters<DeletePresetParams>) -> ToolResult {
        let _activity = self.handle.begin_activity("delete_preset");
        let name = params.0.name;
        crate::presets::remove(&crate::presets::default_dir(), &name)?;
        Ok(Json(json!({ "deleted": name })))
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
            .with_instructions(
                "Glaux(AI と共同作業できる DAW)のプロジェクト編集サーバー。\
                 まず get_project(include_notes: false)で構造を把握 → apply_commands で編集、が基本の流れ。\
                 ノートの移調・時間移動・クオンタイズ・ベロシティ調整は専用ツール\
                 (transpose_notes / shift_notes / quantize_notes / scale_velocity)が使える。\
                 音源: トラックには set_device で内蔵楽器(subtractive / drum / pluck)を設定でき、\
                 本物っぽい楽器一式(ピアノ・ストリングス・ブラス等)は SoundFont: \
                 list_soundfonts で確認 → set_soundfont_instrument で設定(無ければユーザーに導入を提案)。\
                 ギター・ベース・ハープなど「弾く弦」の音は pluck(撥弦の物理モデル)を使う。\
                 エレキギターは pluck + amp(アンプシミュレータ。gain_db 30 前後から歪み、40 以上でメタル)。\
                 メタルの刻みはさらに palm_mute ノート。出荷時プリセット(クリーンエレキ / クランチギター / \
                 メタルギター)を load_preset するのが早い。\
                 list_params でパラメータの意味と現在値を確認して set_param で調整する。\
                 ドラムトラックには drum を設定すること。\
                 analyze_audio が「耳」: 編集結果をレンダしてラウドネス・帯域バランス等を返す。\
                 analyze_harmony が「音楽理論の目」: ノートからキーと小節ごとのコード進行を推定する。\
                 メロディ・ハモリ・ベースを足す前に呼ぶと調性に合った音を選べる。\
                 analyze_rhythm が「リズム感」: スウィング・グリッド・シンコペーションを測る。\
                 既存曲にフレーズを足す前に呼び、同じノリで書くこと。\
                 曲の構成は sections(set_sections)で管理し、「サビ」等の指示は tick 範囲に解決する。\
                 【セルフレビューの習慣】まとまった編集を終えたら、完了報告の前に必ず自己確認する: \
                 (1) analyze_harmony で調性が意図どおりか、(2) analyze_audio でクリップや\
                 バランス破綻がないか。問題があればその場で直してから報告し、\
                 報告には確認結果(キー・LUFS 等)を一言添える。\
                 ミックス調整は 編集 → analyze_audio → 微調整 のループで行う。\
                 エフェクト(eq / compressor / reverb / distortion / sidechain)は add_effect で追加し、\
                 set_param(fx/<id>/<名前>)で調整する。マスターにも掛けられる。\
                 EDM のポンピングは sidechain(source にキックのトラック ID)、\
                 supersaw は subtractive の unison + detune、歪みは distortion。\
                 メタルのブリッジミュートはノートの articulation: \"palm_mute\"(+ distortion)。\
                 音色プリセット: 良い音ができたら save_preset で保存し(全プロジェクト共通)、\
                 音作りの依頼ではまず list_presets で使える音がないか確認 → load_preset で適用 → 微調整。\
                 大きな試行錯誤の前に checkpoint を打ち、気に入らなければ revert_to で戻る。\
                 「さっきのあの編集だけ戻して」は revert {entry_id}(後続の編集は保持される)。\
                 音声素材(録音・WAV)は音声トラック(kind: \"audio\")のクリップとして再生される。\
                 WAV を置くときは import_audio_clip。人間の録音も同じ形で入ってくるので analyze_audio で聴ける。\
                 鼻歌・単旋律の録音は transcribe_audio で MIDI クリップにできる(その後キー確認と整えを忘れずに)。\
                 すべての編集は履歴に残り、get_history(author: \"ai\")で自分の過去の作業を確認できる。",
            )
    }
}
