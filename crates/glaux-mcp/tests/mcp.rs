//! MCP 層のエンドツーエンドテスト。
//!
//! 実際の rmcp クライアントとインメモリ(duplex)トランスポートで接続し、
//! ツール呼び出し → structured_content の中身までを検証する。

use glaux_mcp::{actor::SessionHandle, server::GlauxServer, store::Store};
use rmcp::{
    model::{CallToolRequestParams, CallToolResult},
    service::{RoleClient, RunningService},
    ServiceExt,
};
use serde_json::{json, Value};
use tempfile::TempDir;

struct Fixture {
    client: RunningService<RoleClient, ()>,
    handle: SessionHandle,
    /// プロジェクトフォルダの生存を保つ
    _tmp: TempDir,
    dir: std::path::PathBuf,
}

async fn setup() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("Song.glaux");
    let (store, session) = Store::open_or_create(dir.to_str().unwrap()).unwrap();
    let handle = SessionHandle::spawn(session, store);

    let (server_transport, client_transport) = tokio::io::duplex(1 << 16);
    let server_handle = handle.clone();
    tokio::spawn(async move {
        if let Ok(running) = GlauxServer::new(server_handle)
            .serve(server_transport)
            .await
        {
            let _ = running.waiting().await;
        }
    });
    let client = ().serve(client_transport).await.unwrap();
    Fixture {
        client,
        handle,
        _tmp: tmp,
        dir,
    }
}

async fn call(fx: &Fixture, tool: &'static str, args: Value) -> CallToolResult {
    let params = match args {
        Value::Null => CallToolRequestParams::new(tool),
        Value::Object(map) => CallToolRequestParams::new(tool).with_arguments(map),
        _ => panic!("args must be an object or null"),
    };
    fx.client.call_tool(params).await.unwrap()
}

/// 成功前提で結果の JSON(text の内容)を取り出す。
/// 同じ JSON を structuredContent にも入れると量が 2 倍になるので、text だけで返している
fn ok_json(result: &CallToolResult) -> Value {
    assert_ne!(
        result.is_error,
        Some(true),
        "tool failed: {:?}",
        result.content
    );
    assert!(result.structured_content.is_none(), "text だけで返す");
    let text = result.content[0].as_text().expect("text").text.clone();
    serde_json::from_str(&text).expect("JSON")
}

fn add_track_args(track_id: &str, name: &str) -> Value {
    json!({
        "commands": [
            { "op": "add_track", "track": { "id": track_id, "name": name, "kind": "midi" } }
        ],
        "label": format!("{name} を追加"),
    })
}

#[tokio::test]
async fn full_editing_flow() {
    let fx = setup().await;

    // 1. 空プロジェクトの取得
    let r = call(&fx, "get_project", json!({})).await;
    let v = ok_json(&r);
    assert_eq!(v["project_version"], 0);
    assert_eq!(v["project"]["tracks"], json!([]));

    // 2. 編集: トラック追加 + 音量変更を 1 バッチで
    let r = call(
        &fx,
        "apply_commands",
        json!({
            "commands": [
                { "op": "add_track", "track": { "id": "trk_bass01", "name": "Bass", "kind": "midi" } },
                { "op": "set_track_prop", "id": "trk_bass01", "prop": "volume_db", "value": -6.0 }
            ],
            "label": "ベーストラックを用意",
        }),
    )
    .await;
    let v = ok_json(&r);
    assert_eq!(v["project_version"], 1);
    let entry_id = v["entry_id"].as_str().unwrap().to_owned();
    assert!(entry_id.starts_with("hst_"));
    assert!(v["save_error"].is_null());

    // 保存もされている
    let saved = std::fs::read_to_string(fx.dir.join("project.json")).unwrap();
    assert!(saved.contains("trk_bass01"));

    // 3. 取得(フィルタ付き)に反映されている
    let r = call(&fx, "get_project", json!({ "track_ids": ["trk_bass01"] })).await;
    let v = ok_json(&r);
    let track = &v["project"]["tracks"][0];
    assert_eq!(track["name"], "Bass");
    assert_eq!(track["volume_db"], -6.0);

    // 4. 履歴: AI(このテストクライアント)の作業として記録されている
    let r = call(&fx, "get_history", json!({ "author": "ai" })).await;
    let v = ok_json(&r);
    let entries = v["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["id"], entry_id.as_str());
    assert_eq!(entries[0]["label"], "ベーストラックを用意");
    assert_eq!(entries[0]["author"]["kind"], "ai");

    // 5. checkpoint → さらに編集 → revert_to で戻る
    // project_version は操作のたびに 1 ずつ増え、undo・revert_to でも戻らない
    let r = call(&fx, "checkpoint", json!({ "label": "base" })).await;
    assert_eq!(ok_json(&r)["project_version"], 2);

    let r = call(
        &fx,
        "apply_commands",
        json!({
            "commands": [ { "op": "set_master_volume", "volume_db": -3.0 } ],
            "label": "マスター音量",
        }),
    )
    .await;
    assert_eq!(ok_json(&r)["project_version"], 3);

    let r = call(&fx, "revert_to", json!({ "label": "base" })).await;
    assert_eq!(ok_json(&r)["project_version"], 4);

    // 6. since フィルタ: base 以降の履歴は無い(revert_to は undo なので履歴を増やさない)
    let r = call(&fx, "get_history", json!({ "since": entry_id })).await;
    assert_eq!(ok_json(&r)["entries"], json!([]));

    // 7. undo でトラック追加ごと取り消し(Batch が 1 単位)
    let r = call(&fx, "undo", json!({})).await;
    let v = ok_json(&r);
    assert_eq!(v["undone"], 1);
    assert_eq!(v["project_version"], 5);

    let r = call(&fx, "get_project", json!({})).await;
    assert_eq!(ok_json(&r)["project"]["tracks"], json!([]));

    // 8. redo で復活
    let r = call(&fx, "redo", json!({})).await;
    let v = ok_json(&r);
    assert_eq!(v["redone"], 1);
    assert_eq!(v["project_version"], 6);
}

#[tokio::test]
async fn revert_single_entry_keeps_later_edits() {
    let fx = setup().await;

    // トラック追加 → 音量変更 → マスター音量、の 3 エントリ
    let r = call(&fx, "apply_commands", add_track_args("trk_gtr001", "Gt")).await;
    ok_json(&r);
    let r = call(
        &fx,
        "apply_commands",
        json!({
            "commands": [ { "op": "set_track_prop", "id": "trk_gtr001", "prop": "volume_db", "value": -9.0 } ],
            "label": "音量を下げる",
        }),
    )
    .await;
    let vol_entry = ok_json(&r)["entry_id"].as_str().unwrap().to_owned();
    let r = call(
        &fx,
        "apply_commands",
        json!({
            "commands": [ { "op": "set_master_volume", "volume_db": -3.0 } ],
            "label": "マスター音量",
        }),
    )
    .await;
    ok_json(&r);

    // 真ん中の「音量を下げる」だけを取り消す
    let r = call(&fx, "revert", json!({ "entry_id": vol_entry })).await;
    let v = ok_json(&r);
    assert_eq!(v["conflicts"], json!([]), "対象が違うので衝突なし");
    assert_eq!(v["project_version"], 4, "revert 自体が履歴に積まれる");

    // 音量は元に戻り、後続のマスター音量変更は保持される
    let r = call(&fx, "get_project", json!({})).await;
    let v = ok_json(&r);
    assert_eq!(v["project"]["tracks"][0]["volume_db"], 0.0);
    assert_eq!(v["project"]["master"]["volume_db"], -3.0);

    // 履歴には reverts 付きのエントリが載る
    let r = call(&fx, "get_history", json!({})).await;
    let entries = ok_json(&r)["entries"].as_array().unwrap().clone();
    assert_eq!(entries.len(), 4);
    assert_eq!(entries[3]["reverts"], json!(vol_entry));

    // 同じ対象を触った後続編集があると conflicts で警告される
    let r = call(
        &fx,
        "apply_commands",
        json!({
            "commands": [ { "op": "set_track_prop", "id": "trk_gtr001", "prop": "pan", "value": 0.5 } ],
            "label": "パン変更",
        }),
    )
    .await;
    ok_json(&r);
    let first_entry = entries[0]["id"].as_str().unwrap().to_owned();
    let r = call(&fx, "revert", json!({ "entry_id": first_entry })).await;
    let v = ok_json(&r);
    let conflicts = v["conflicts"].as_array().unwrap();
    assert!(
        !conflicts.is_empty(),
        "同一トラックの後続編集が衝突扱いになるはず"
    );

    // 存在しないエントリはツールエラー
    let r = call(&fx, "revert", json!({ "entry_id": "hst_nonono" })).await;
    assert_eq!(r.is_error, Some(true));
}

#[tokio::test]
async fn include_notes_false_returns_note_counts() {
    let fx = setup().await;
    call(&fx, "apply_commands", add_track_args("trk_keys01", "Keys")).await;
    let r = call(
        &fx,
        "apply_commands",
        json!({
            "commands": [
                {
                    "op": "add_clip",
                    "track": "trk_keys01",
                    "clip": {
                        "id": "clp_intro1", "name": "Intro", "start": 0, "length": 3840,
                        "kind": "midi",
                        "notes": [
                            { "id": "nt_c40001", "pos": 0, "dur": 480, "pitch": 60, "vel": 100 },
                            { "id": "nt_e40001", "pos": 480, "dur": 480, "pitch": 64, "vel": 100 }
                        ]
                    }
                }
            ],
            "label": "イントロのクリップ",
        }),
    )
    .await;
    assert_ne!(r.is_error, Some(true), "{:?}", r.content);

    let r = call(&fx, "get_project", json!({ "include_notes": false })).await;
    let clip = &ok_json(&r)["project"]["tracks"][0]["clips"][0];
    assert_eq!(clip["notes"], json!([]));
    assert_eq!(clip["note_count"], 2);

    // 既定(include_notes: true)ではノートがそのまま返る
    let r = call(&fx, "get_project", json!({})).await;
    let clip = &ok_json(&r)["project"]["tracks"][0]["clips"][0];
    assert_eq!(clip["notes"].as_array().unwrap().len(), 2);
    assert!(clip.get("note_count").is_none());
}

#[tokio::test]
async fn get_project_filters_by_clip_and_range_and_compacts_notes() {
    let fx = setup().await;
    call(&fx, "apply_commands", add_track_args("trk_keys01", "Keys")).await;
    let r = call(
        &fx,
        "apply_commands",
        json!({
            "commands": [
                {
                    "op": "add_clip", "track": "trk_keys01",
                    "clip": {
                        "id": "clp_intro1", "name": "Intro", "start": 0, "length": 7680, "kind": "midi",
                        "notes": [
                            { "id": "nt_a00001", "pos": 0, "dur": 480, "pitch": 60, "vel": 100 },
                            { "id": "nt_b00001", "pos": 3840, "dur": 480, "pitch": 64, "vel": 90,
                              "articulation": "staccato" }
                        ]
                    }
                },
                {
                    "op": "add_clip", "track": "trk_keys01",
                    "clip": { "id": "clp_verse1", "name": "Verse", "start": 7680, "length": 3840, "kind": "midi",
                              "notes": [ { "id": "nt_c00001", "pos": 0, "dur": 480, "pitch": 67, "vel": 100 } ] }
                }
            ],
            "label": "2 つのクリップ",
        }),
    )
    .await;
    assert_ne!(r.is_error, Some(true), "{:?}", r.content);

    // クリップ ID で絞る
    let r = call(&fx, "get_project", json!({ "clip_ids": ["clp_verse1"] })).await;
    let clips = ok_json(&r)["project"]["tracks"][0]["clips"].clone();
    assert_eq!(clips.as_array().unwrap().len(), 1);
    assert_eq!(clips[0]["id"], "clp_verse1");

    // 範囲で絞る: 2 小節目だけ → Intro の 2 つ目のノートだけ(元は 2 個)。Verse は範囲外
    let r = call(
        &fx,
        "get_project",
        json!({ "start_tick": 3840, "end_tick": 7680 }),
    )
    .await;
    let clips = ok_json(&r)["project"]["tracks"][0]["clips"].clone();
    assert_eq!(clips.as_array().unwrap().len(), 1);
    assert_eq!(clips[0]["notes"].as_array().unwrap().len(), 1);
    assert_eq!(clips[0]["notes"][0]["id"], "nt_b00001");
    assert_eq!(clips[0]["notes_in_range"], true);
    assert_eq!(clips[0]["note_count"], 2);

    // 配列形式: [id, pos, dur, pitch, vel]、奏法があるものは 6 番目
    let r = call(
        &fx,
        "get_project",
        json!({ "clip_ids": ["clp_intro1"], "note_format": "compact" }),
    )
    .await;
    let v = ok_json(&r);
    assert_eq!(v["note_fields"][0], "id");
    let notes = v["project"]["tracks"][0]["clips"][0]["notes"].clone();
    assert_eq!(notes[0], json!(["nt_a00001", 0, 480, 60, 100]));
    assert_eq!(
        notes[1],
        json!(["nt_b00001", 3840, 480, 64, 90, { "articulation": "staccato" }])
    );

    // 不正な指定はエラー
    let r = call(&fx, "get_project", json!({ "note_format": "xml" })).await;
    assert_eq!(r.is_error, Some(true));
    let r = call(
        &fx,
        "get_project",
        json!({ "start_tick": 10, "end_tick": 5 }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
}

#[tokio::test]
async fn arrangement_tools_and_changes() {
    let fx = setup().await;
    // ID を省いたクリップとノート → サーバーが振って返す
    let r = call(
        &fx,
        "apply_commands",
        json!({
            "commands": [
                { "op": "add_track", "track": { "id": "trk_arr001", "name": "Keys", "kind": "midi" } },
                { "op": "add_clip", "track": "trk_arr001",
                  "clip": { "name": "A", "start": 0, "length": 3840, "kind": "midi",
                            "notes": [ { "pos": 0, "dur": 480, "pitch": 60, "vel": 100 },
                                       { "pos": 960, "dur": 480, "pitch": 64, "vel": 100 } ] } }
            ],
            "label": "ID なしで追加",
        }),
    )
    .await;
    let v = ok_json(&r);
    let first_entry = v["entry_id"].as_str().unwrap().to_owned();
    let assigned = v["assigned_ids"].as_array().unwrap();
    assert_eq!(
        assigned.len(),
        1,
        "クリップの ID だけ返る(ノートは数が多いので返さない): {assigned:?}"
    );
    let clip_id = assigned[0]["id"].as_str().unwrap().to_owned();
    assert!(clip_id.starts_with("clp_"));

    // 複製: 位置の指定なし → 直後(1 小節目の後ろ = 3840)
    let r = call(&fx, "duplicate_clips", json!({ "clip_ids": [clip_id] })).await;
    let copy = ok_json(&r)["clips"][0].as_str().unwrap().to_owned();
    // 1 小節目の頭に 2 小節挿入 → 両方のクリップが 2 小節ずれる
    let r = call(&fx, "insert_bars", json!({ "bar": 1, "count": 2 })).await;
    assert_eq!(ok_json(&r)["length_ticks"], 7680);
    let r = call(&fx, "get_project", json!({ "note_format": "compact" })).await;
    let clips = ok_json(&r)["project"]["tracks"][0]["clips"].clone();
    let starts: Vec<u64> = clips
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["start"].as_u64().unwrap())
        .collect();
    assert_eq!(starts, vec![7680, 11520]);
    assert_eq!(clips[1]["id"], copy.as_str());

    // 最初の追加より後の変更の要約
    let r = call(&fx, "get_changes", json!({ "since": first_entry })).await;
    let v = ok_json(&r);
    assert_eq!(v["entries"].as_array().unwrap().len(), 2);
    let copied = v["clips"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["clip_id"] == copy.as_str())
        .unwrap();
    assert!(copied["ops"]
        .as_array()
        .unwrap()
        .iter()
        .any(|o| o == "add_clip"));

    // 小節の挿入は 1 回の undo で戻る
    ok_json(&call(&fx, "undo", json!({})).await);
    let r = call(&fx, "get_project", json!({})).await;
    let starts: Vec<u64> = ok_json(&r)["project"]["tracks"][0]["clips"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["start"].as_u64().unwrap())
        .collect();
    assert_eq!(starts, vec![0, 3840]);
}

#[tokio::test]
async fn errors_are_reported_as_tool_errors() {
    let fx = setup().await;

    // 不正なコマンド JSON: どの要素が悪いかを示す
    let r = call(
        &fx,
        "apply_commands",
        json!({ "commands": [ { "op": "no_such_op" } ], "label": "x" }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));

    // 適用時エラー(存在しないトラック)は CoreError の文言が返る
    let r = call(
        &fx,
        "apply_commands",
        json!({
            "commands": [ { "op": "remove_track", "id": "trk_nothere" } ],
            "label": "x",
        }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));

    // 存在しないチェックポイント
    let r = call(&fx, "revert_to", json!({ "label": "no-such-checkpoint" })).await;
    assert_eq!(r.is_error, Some(true));

    // 失敗した編集は履歴に残らない
    let r = call(&fx, "get_history", json!({})).await;
    assert_eq!(ok_json(&r)["entries"], json!([]));
}

#[tokio::test]
async fn events_fire_for_ui_subscribers() {
    let fx = setup().await;
    let mut activity = fx.handle.subscribe_activity();
    let mut changes = fx.handle.subscribe();

    // ツール呼び出し中は busy=true、終了で busy=false(UI の「AI 作業中」表示用)
    call(&fx, "apply_commands", add_track_args("trk_evt001", "Evt")).await;
    let started = activity.recv().await.unwrap();
    assert_eq!(started.tool, "apply_commands");
    assert!(started.busy);
    let finished = activity.recv().await.unwrap();
    assert_eq!(finished.tool, "apply_commands");
    assert!(!finished.busy);

    // 変更通知(UI はこれを受けて再取得する)
    let changed = changes.recv().await.unwrap();
    assert_eq!(changed.project_version, 1);
    assert!(!changed.changes.is_empty());

    // 読み取り専用ツールもアクティビティは出す(が、変更通知は出さない)
    call(&fx, "get_history", json!({})).await;
    let started = activity.recv().await.unwrap();
    assert_eq!(started.tool, "get_history");
    assert!(started.busy);
    let finished = activity.recv().await.unwrap();
    assert!(!finished.busy);
    assert!(changes.try_recv().is_err());
}

#[tokio::test]
async fn list_params_catalog_and_track() {
    let fx = setup().await;

    // カタログモード
    let r = call(&fx, "list_params", json!({})).await;
    let v = ok_json(&r);
    let instruments = v["instruments"].as_array().unwrap();
    assert!(instruments.iter().any(|i| i["name"] == "subtractive"));
    assert!(instruments.iter().any(|i| i["name"] == "drum"));

    // device 未設定トラック → デフォルト音源(subtractive)の spec + デフォルト値
    call(&fx, "apply_commands", add_track_args("trk_lead01", "Lead")).await;
    let r = call(&fx, "list_params", json!({ "track_id": "trk_lead01" })).await;
    let v = ok_json(&r);
    assert_eq!(v["device"]["name"], "subtractive");
    assert_eq!(v["device"]["is_default_fallback"], true);
    let cutoff = v["params"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "cutoff")
        .expect("cutoff spec");
    assert_eq!(cutoff["current"], 8000.0);
    assert_eq!(cutoff["path"], "device/cutoff");

    // set_device + set_param 後は現在値が反映される
    let r = call(
        &fx,
        "apply_commands",
        json!({
            "commands": [
                { "op": "set_device", "track": "trk_lead01",
                  "device": { "type": "builtin", "name": "drum" } },
                { "op": "set_param", "track": "trk_lead01",
                  "path": "device/tone", "value": 0.9 }
            ],
            "label": "ドラム音源を設定",
        }),
    )
    .await;
    assert_ne!(r.is_error, Some(true), "{:?}", r.content);

    let r = call(&fx, "list_params", json!({ "track_id": "trk_lead01" })).await;
    let v = ok_json(&r);
    assert_eq!(v["device"]["name"], "drum");
    assert_eq!(v["device"]["is_default_fallback"], false);
    let tone = v["params"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "tone")
        .unwrap();
    assert_eq!(tone["current"], 0.9);
}

#[tokio::test]
async fn analyze_audio_returns_metrics() {
    let fx = setup().await;
    call(&fx, "apply_commands", add_track_args("trk_bass02", "Bass")).await;
    let r = call(
        &fx,
        "apply_commands",
        json!({
            "commands": [
                {
                    "op": "add_clip",
                    "track": "trk_bass02",
                    "clip": {
                        "id": "clp_bass01", "name": "B", "start": 0, "length": 3840,
                        "kind": "midi",
                        "notes": [
                            { "id": "nt_bass01", "pos": 0, "dur": 1920, "pitch": 33, "vel": 110 },
                            { "id": "nt_bass02", "pos": 1920, "dur": 1920, "pitch": 36, "vel": 110 }
                        ]
                    }
                }
            ],
            "label": "ベースを追加",
        }),
    )
    .await;
    assert_ne!(r.is_error, Some(true), "{:?}", r.content);

    // 全体解析
    let r = call(&fx, "analyze_audio", json!({})).await;
    let v = ok_json(&r);
    assert!(v["duration_seconds"].as_f64().unwrap() > 1.0);
    assert!(v["loudness_lufs"].as_f64().unwrap() < 0.0);
    assert!(v["band_energy"]["low"].as_f64().unwrap() > 0.4);
    assert_eq!(v["clipped"], false);
    assert_eq!(v["project_version"], 2);

    // トラック指定 + 範囲指定
    let r = call(
        &fx,
        "analyze_audio",
        json!({ "track_ids": ["trk_bass02"], "start_tick": 0, "end_tick": 1920 }),
    )
    .await;
    let v = ok_json(&r);
    assert!(v["duration_seconds"].as_f64().unwrap() < 1.5);

    // 空範囲はエラー
    let r = call(
        &fx,
        "analyze_audio",
        json!({ "start_tick": 100, "end_tick": 100 }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
}

#[tokio::test]
async fn list_params_includes_effects() {
    let fx = setup().await;

    // カタログにエフェクトが載る
    let r = call(&fx, "list_params", json!({})).await;
    let v = ok_json(&r);
    let effects = v["effects"].as_array().unwrap();
    for name in ["eq", "compressor", "reverb"] {
        assert!(effects.iter().any(|e| e["name"] == name), "{name}");
    }

    // エフェクトを追加すると spec + current + path が返る
    call(&fx, "apply_commands", add_track_args("trk_pad001", "Pad")).await;
    let r = call(
        &fx,
        "apply_commands",
        json!({
            "commands": [
                { "op": "add_effect", "track": "trk_pad001",
                  "effect": { "id": "fx_rev001", "type": "builtin", "name": "reverb" } },
                { "op": "set_param", "track": "trk_pad001",
                  "path": "fx/fx_rev001/mix", "value": 0.4 }
            ],
            "label": "リバーブを追加",
        }),
    )
    .await;
    assert_ne!(r.is_error, Some(true), "{:?}", r.content);

    let r = call(&fx, "list_params", json!({ "track_id": "trk_pad001" })).await;
    let v = ok_json(&r);
    let fx_list = v["effects"].as_array().unwrap();
    assert_eq!(fx_list.len(), 1);
    assert_eq!(fx_list[0]["name"], "reverb");
    assert_eq!(fx_list[0]["bypass"], false);
    let mix = fx_list[0]["params"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "mix")
        .unwrap();
    assert_eq!(mix["current"], 0.4);
    assert_eq!(mix["path"], "fx/fx_rev001/mix");
}

#[tokio::test]
async fn switch_project_swaps_session_for_all_handles() {
    let fx = setup().await;
    call(&fx, "apply_commands", add_track_args("trk_old001", "Old")).await;

    let mut changes = fx.handle.subscribe();
    let dir_b = fx._tmp.path().join("Another.glaux");
    let (title, version) = fx
        .handle
        .switch_project(dir_b.to_string_lossy().into_owned())
        .await
        .unwrap();
    assert_eq!(title, "Another");
    assert_eq!(version, 0);

    // 購読者(UI / エンジン)には全体更新が通知される
    let ev = changes.recv().await.unwrap();
    assert_eq!(ev.project_version, 0);

    // MCP クライアント(同じハンドル)も新プロジェクトを見る
    let r = call(&fx, "get_project", json!({})).await;
    let v = ok_json(&r);
    assert_eq!(v["project"]["meta"]["title"], "Another");
    assert_eq!(v["project"]["tracks"], json!([]));

    // 新プロジェクトへの編集は新しいフォルダに保存される
    call(&fx, "apply_commands", add_track_args("trk_new001", "New")).await;
    assert!(dir_b.join("project.json").exists());
    let saved = std::fs::read_to_string(dir_b.join("project.json")).unwrap();
    assert!(saved.contains("trk_new001"));

    // 存在しないフォルダ指定は open_or_create が新規作成する(切り替え自体は成功)
    // 一方、呼び出し側(アプリ)は create=false 時に project.json の存在を事前検証する
}

#[tokio::test]
async fn compare_mix_reports_loudness_and_tone_separately() {
    let fx = setup().await;
    call(&fx, "apply_commands", add_track_args("trk_pad001", "Pad")).await;
    let r = call(
        &fx,
        "apply_commands",
        json!({
            "commands": [{
                "op": "add_clip", "track": "trk_pad001",
                "clip": { "id": "clp_pad001", "name": "P", "start": 0, "length": 3840, "kind": "midi",
                  "notes": [
                    { "id": "nt_pad001", "pos": 0, "dur": 3840, "pitch": 57, "vel": 100 },
                    { "id": "nt_pad002", "pos": 0, "dur": 3840, "pitch": 64, "vel": 100 }
                  ] }
            }],
            "label": "パッドを追加",
        }),
    )
    .await;
    assert_ne!(r.is_error, Some(true), "{:?}", r.content);
    call(&fx, "checkpoint", json!({ "label": "mix_a" })).await;
    let r = call(
        &fx,
        "apply_commands",
        json!({ "commands": [{ "op": "set_track_prop", "id": "trk_pad001", "prop": "volume_db", "value": -6.0 }], "label": "下げる" }),
    )
    .await;
    assert_ne!(r.is_error, Some(true), "{:?}", r.content);

    let v = ok_json(&call(&fx, "compare_mix", json!({ "checkpoint": "mix_a" })).await);
    assert_eq!(v["edits_compared"], 1);
    let d = v["loudness_diff_db"].as_f64().unwrap();
    assert!((d + 6.0).abs() < 0.3, "音量差: {d}");
    assert!((v["match_gain_db"].as_f64().unwrap() - 6.0).abs() < 0.3);
    assert!(v["tonal_balance"].as_array().unwrap().len() >= 8);
    assert!(v["notes"][0].as_str().unwrap().contains("小さい"));
    // 省略時は直前の 1 編集の前と比べる(同じ結果)
    let v2 = ok_json(&call(&fx, "compare_mix", json!({})).await);
    assert_eq!(v2["loudness_diff_db"], v["loudness_diff_db"]);
    // 今のプロジェクトは変わらない
    let p = ok_json(&call(&fx, "get_project", json!({ "include_notes": false })).await);
    assert_eq!(p["project"]["tracks"][0]["volume_db"], -6.0);
    // 2 つ指定はエラー
    let r = call(
        &fx,
        "compare_mix",
        json!({ "checkpoint": "mix_a", "back": 1 }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
}

#[tokio::test]
async fn master_mix_adds_a_mastering_chain_and_can_be_undone() {
    let fx = setup().await;
    call(&fx, "apply_commands", add_track_args("trk_pad002", "Pad")).await;
    let r = call(
        &fx,
        "apply_commands",
        json!({
            "commands": [{
                "op": "add_clip", "track": "trk_pad002",
                "clip": { "id": "clp_pad002", "name": "P", "start": 0, "length": 7680, "kind": "midi",
                  "notes": [
                    { "id": "nt_pad101", "pos": 0, "dur": 7680, "pitch": 48, "vel": 90 },
                    { "id": "nt_pad102", "pos": 0, "dur": 7680, "pitch": 55, "vel": 90 },
                    { "id": "nt_pad103", "pos": 0, "dur": 7680, "pitch": 64, "vel": 90 }
                  ] }
            }],
            "label": "パッド",
        }),
    )
    .await;
    assert_ne!(r.is_error, Some(true), "{:?}", r.content);
    // 案だけ: プロジェクトは変わらない
    let v = ok_json(&call(&fx, "master_mix", json!({ "apply": false })).await);
    assert_eq!(v["applied"], false);
    let p = ok_json(&call(&fx, "get_project", json!({ "include_notes": false })).await);
    assert_eq!(p["project"]["master"]["effects"], json!([]));
    // 足す: EQ・コンプ・リミッタが入り、-14 LUFS 前後・True Peak -1 以下
    let v = ok_json(&call(&fx, "master_mix", json!({ "target": "spotify" })).await);
    eprintln!("{v:#}");
    assert_eq!(v["applied"], true);
    let lufs = v["after"]["loudness_lufs"].as_f64().unwrap();
    assert!((lufs + 14.0).abs() < 1.0, "{lufs}");
    assert!(v["after"]["true_peak_dbtp"].as_f64().unwrap() <= -0.8);
    let p = ok_json(&call(&fx, "get_project", json!({ "include_notes": false })).await);
    let names: Vec<&str> = p["project"]["master"]["effects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap_or(""))
        .collect();
    assert!(
        names.ends_with(&["compressor", "limiter"]) || names.ends_with(&["width", "limiter"]),
        "{names:?}"
    );
    assert!(names.contains(&"eq"));
    // 1 回の undo で戻る
    ok_json(&call(&fx, "undo", json!({})).await);
    let p = ok_json(&call(&fx, "get_project", json!({ "include_notes": false })).await);
    assert_eq!(p["project"]["master"]["effects"], json!([]));
    // 分からない target はエラー
    let r = call(&fx, "master_mix", json!({ "target": "cd" })).await;
    assert_eq!(r.is_error, Some(true));
    // 参照曲(44.1kHz のモノラルの雑音)に寄せる案: 音量が参照曲に合う
    let mut rng: u32 = 5;
    let noise: Vec<f32> = (0..44_100 * 5)
        .map(|_| {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            (rng as f32 / u32::MAX as f32 - 0.5) * 0.5
        })
        .collect();
    let path = fx.dir.join("reference.wav");
    write_mono_wav(&path, &noise, 44_100);
    let v = ok_json(
        &call(
            &fx,
            "master_mix",
            json!({ "reference_file": path.to_string_lossy(), "apply": false }),
        )
        .await,
    );
    let (got, want) = (
        v["after"]["loudness_lufs"].as_f64().unwrap(),
        v["reference"]["loudness_lufs"].as_f64().unwrap(),
    );
    assert!((got - want).abs() < 1.0, "{got} / {want}");
    let (e0, e1) = (
        v["tonal_error_db"][0].as_f64().unwrap(),
        v["tonal_error_db"][1].as_f64().unwrap(),
    );
    assert!(e1 < e0, "釣り合いが参照曲に近づく: {e0} → {e1}");
}

/// 言葉で追い込む(CLAP の音声側のモデルが要る。`GLAUX_CLAP_MODEL` が無ければスキップ)
#[tokio::test]
async fn refine_by_words_moves_eq_toward_the_words() {
    if std::env::var_os("GLAUX_CLAP_MODEL").is_none() {
        eprintln!("GLAUX_CLAP_MODEL が未設定のためスキップ");
        return;
    }
    let fx = setup().await;
    // 明るいのこぎり波のリードに、素通しの EQ
    let r = call(
        &fx,
        "apply_commands",
        json!({
            "commands": [
                { "op": "add_track", "track": { "id": "trk_lead09", "name": "Lead", "kind": "midi" } },
                { "op": "add_clip", "track": "trk_lead09",
                  "clip": { "id": "clp_ld0009", "name": "L", "start": 0, "length": 7680, "kind": "midi",
                    "notes": [
                      { "id": "nt_ld0901", "pos": 0, "dur": 1920, "pitch": 72, "vel": 110 },
                      { "id": "nt_ld0902", "pos": 1920, "dur": 1920, "pitch": 76, "vel": 110 },
                      { "id": "nt_ld0903", "pos": 3840, "dur": 3840, "pitch": 79, "vel": 110 }
                    ] } },
                { "op": "set_device", "track": "trk_lead09", "device": { "type": "builtin", "name": "subtractive" } },
                { "op": "set_param", "track": "trk_lead09", "path": "device/cutoff", "value": 12000.0 },
                { "op": "add_effect", "track": "trk_lead09", "effect": { "id": "fx_eq0009", "type": "builtin", "name": "eq" } }
            ],
            "label": "リード",
        }),
    )
    .await;
    assert_ne!(r.is_error, Some(true), "{:?}", r.content);
    let v = ok_json(
        &call(
            &fx,
            "refine_by_words",
            json!({
                "track_id": "trk_lead09",
                "toward": ["warm", "暗い"],
                "away": ["bright", "harsh"],
                "params": ["high_gain_db", "lp_freq"],
                "max_evals": 18
            }),
        )
        .await,
    );
    eprintln!("{v:#}");
    let (b, a) = (
        v["score_before"].as_f64().unwrap(),
        v["score_after"].as_f64().unwrap(),
    );
    assert!(a > b, "言葉に近づく: {b} → {a}");
    assert_eq!(v["applied"], true);
    assert!(!v["changes"].as_array().unwrap().is_empty());
    // 辞書に無い語はエラー(使える語の一覧を返す)
    let r = call(
        &fx,
        "refine_by_words",
        json!({ "track_id": "trk_lead09", "toward": ["ふわもこ"] }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
}

#[tokio::test]
async fn import_ir_adds_a_convolution_reverb() {
    let fx = setup().await;
    call(&fx, "apply_commands", add_track_args("trk_ir0001", "Keys")).await;
    // IR: 0.5 秒の減衰する雑音
    let mut rng: u32 = 11;
    let ir: Vec<f32> = (0..24_000)
        .map(|i| {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            (rng as f32 / u32::MAX as f32 - 0.5) * (-(i as f32) / 5000.0).exp()
        })
        .collect();
    let path = fx.dir.join("hall.wav");
    write_mono_wav(&path, &ir, 48_000);
    let v = ok_json(
        &call(
            &fx,
            "import_ir",
            json!({ "path": path.to_string_lossy(), "track_id": "trk_ir0001", "mix": 0.4 }),
        )
        .await,
    );
    assert!(v["asset"].as_str().unwrap().starts_with("sha256:"));
    let p = ok_json(&call(&fx, "get_project", json!({ "include_notes": false })).await);
    let fxs = &p["project"]["tracks"][0]["effects"];
    assert_eq!(fxs[0]["name"], "convolution", "{fxs}");
    assert_eq!(fxs[0]["params"]["ir"], v["asset"]);
    // マスターにも(track_id 省略)
    ok_json(&call(&fx, "import_ir", json!({ "path": path.to_string_lossy() })).await);
    let p = ok_json(&call(&fx, "get_project", json!({ "include_notes": false })).await);
    assert_eq!(p["project"]["master"]["effects"][0]["name"], "convolution");
    // 1 回の undo で戻る
    ok_json(&call(&fx, "undo", json!({})).await);
    let p = ok_json(&call(&fx, "get_project", json!({ "include_notes": false })).await);
    assert_eq!(p["project"]["master"]["effects"], json!([]));
}

#[tokio::test]
async fn analyze_audio_per_track_reveals_balance() {
    let fx = setup().await;
    // 静かなリードと大きいベース
    let r = call(
        &fx,
        "apply_commands",
        json!({
            "commands": [
                { "op": "add_track", "track": { "id": "trk_lead02", "name": "Lead", "kind": "midi", "volume_db": -18.0 } },
                { "op": "add_clip", "track": "trk_lead02",
                  "clip": { "id": "clp_ld0001", "name": "L", "start": 0, "length": 3840, "kind": "midi",
                    "notes": [ { "id": "nt_ld0001", "pos": 0, "dur": 3840, "pitch": 72, "vel": 100 } ] } },
                { "op": "add_track", "track": { "id": "trk_bass03", "name": "Bass", "kind": "midi" } },
                { "op": "add_clip", "track": "trk_bass03",
                  "clip": { "id": "clp_bs0001", "name": "B", "start": 0, "length": 3840, "kind": "midi",
                    "notes": [ { "id": "nt_bs0001", "pos": 0, "dur": 3840, "pitch": 33, "vel": 110 } ] } }
            ],
            "label": "バランステスト用",
        }),
    )
    .await;
    assert_ne!(r.is_error, Some(true), "{:?}", r.content);

    let r = call(&fx, "analyze_audio", json!({ "per_track": true })).await;
    let v = ok_json(&r);
    let tracks = v["tracks"].as_array().expect("tracks が返るはず");
    assert_eq!(tracks.len(), 2);
    // うるさい順ソート: Bass が先頭
    assert_eq!(tracks[0]["name"], "Bass");
    assert_eq!(tracks[1]["name"], "Lead");
    let diff =
        tracks[0]["loudness_lufs"].as_f64().unwrap() - tracks[1]["loudness_lufs"].as_f64().unwrap();
    assert!(diff > 6.0, "音量差が数値に出るはず: {diff}");

    // per_track なしなら tracks は付かない
    let r = call(&fx, "analyze_audio", json!({})).await;
    assert!(ok_json(&r).get("tracks").is_none());
}

#[tokio::test]
async fn note_utility_tools_edit_and_undo_as_one_step() {
    let fx = setup().await;
    call(&fx, "apply_commands", add_track_args("trk_keys01", "Keys")).await;
    let r = call(
        &fx,
        "apply_commands",
        json!({
            "commands": [
                { "op": "add_clip", "track": "trk_keys01",
                  "clip": { "id": "clp_riff01", "name": "Riff", "start": 0, "length": 3840, "kind": "midi",
                    "notes": [
                        { "id": "nt_aaa001", "pos": 10,  "dur": 480, "pitch": 60,  "vel": 100 },
                        { "id": "nt_bbb001", "pos": 490, "dur": 480, "pitch": 64,  "vel": 80 },
                        { "id": "nt_ccc001", "pos": 960, "dur": 480, "pitch": 126, "vel": 120 }
                    ] } }
            ],
            "label": "リフを追加",
        }),
    )
    .await;
    assert_ne!(r.is_error, Some(true), "{:?}", r.content);

    // 1. 移調 +12: 126 は 127 に丸められる(clamped: 1)
    let r = call(
        &fx,
        "transpose_notes",
        json!({ "clip_id": "clp_riff01", "semitones": 12 }),
    )
    .await;
    let v = ok_json(&r);
    assert_eq!(v["changed"], 3);
    assert_eq!(v["clamped"], 1);

    // 2. 選択ノートだけ後ろへ移動
    let r = call(
        &fx,
        "shift_notes",
        json!({ "clip_id": "clp_riff01", "delta_ticks": 480, "note_ids": ["nt_aaa001"] }),
    )
    .await;
    assert_eq!(ok_json(&r)["changed"], 1);

    // 3. クオンタイズ 1/8: 490 の 2 音が 480 に寄る(960 は変更なし)
    let r = call(
        &fx,
        "quantize_notes",
        json!({ "clip_id": "clp_riff01", "grid_ticks": 480 }),
    )
    .await;
    assert_eq!(ok_json(&r)["changed"], 2);

    // 4. ベロシティを半分に
    let r = call(
        &fx,
        "scale_velocity",
        json!({ "clip_id": "clp_riff01", "factor": 0.5 }),
    )
    .await;
    let v = ok_json(&r);
    assert_eq!(v["changed"], 3);
    assert!(v.get("clamped").is_none());

    // 5. 全部グリッド上なので no-op: 履歴を汚さない
    let before = ok_json(&call(&fx, "get_history", json!({})).await)["entries"]
        .as_array()
        .unwrap()
        .len();
    let r = call(
        &fx,
        "quantize_notes",
        json!({ "clip_id": "clp_riff01", "grid_ticks": 480 }),
    )
    .await;
    assert_eq!(ok_json(&r)["changed"], 0);
    let after = ok_json(&call(&fx, "get_history", json!({})).await)["entries"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(before, after, "no-op は履歴エントリを作らない");

    // 最終状態(ノートは pos, pitch, id 順にソートされる)
    let r = call(&fx, "get_project", json!({})).await;
    let notes = ok_json(&r)["project"]["tracks"][0]["clips"][0]["notes"].clone();
    assert_eq!(
        notes,
        json!([
            { "id": "nt_aaa001", "pos": 480, "dur": 480, "pitch": 72,  "vel": 50 },
            { "id": "nt_bbb001", "pos": 480, "dur": 480, "pitch": 76,  "vel": 40 },
            { "id": "nt_ccc001", "pos": 960, "dur": 480, "pitch": 127, "vel": 60 }
        ])
    );

    // undo 1 回 = ベロシティ調整だけが戻る(1 ツール呼び出し = 1 履歴エントリ)
    call(&fx, "undo", json!({})).await;
    let r = call(&fx, "get_project", json!({})).await;
    let notes = ok_json(&r)["project"]["tracks"][0]["clips"][0]["notes"].clone();
    assert_eq!(notes[0]["vel"], 100);
    assert_eq!(notes[0]["pos"], 480, "位置の編集は残る");
}

#[tokio::test]
async fn note_utility_tools_validate_inputs() {
    let fx = setup().await;
    call(&fx, "apply_commands", add_track_args("trk_keys01", "Keys")).await;
    call(
        &fx,
        "apply_commands",
        json!({
            "commands": [
                { "op": "add_clip", "track": "trk_keys01",
                  "clip": { "id": "clp_riff01", "name": "Riff", "start": 0, "length": 3840, "kind": "midi",
                    "notes": [ { "id": "nt_aaa001", "pos": 0, "dur": 480, "pitch": 60, "vel": 100 } ] } }
            ],
            "label": "リフを追加",
        }),
    )
    .await;

    // 変更量ゼロ・範囲外・対象なしはツールエラー
    for (tool, args) in [
        (
            "transpose_notes",
            json!({ "clip_id": "clp_riff01", "semitones": 0 }),
        ),
        (
            "shift_notes",
            json!({ "clip_id": "clp_riff01", "delta_ticks": 0 }),
        ),
        (
            "quantize_notes",
            json!({ "clip_id": "clp_riff01", "grid_ticks": 0 }),
        ),
        (
            "quantize_notes",
            json!({ "clip_id": "clp_riff01", "grid_ticks": 480, "strength": 1.5 }),
        ),
        ("scale_velocity", json!({ "clip_id": "clp_riff01" })),
        (
            "transpose_notes",
            json!({ "clip_id": "clp_nothere", "semitones": 1 }),
        ),
        (
            "transpose_notes",
            json!({ "clip_id": "clp_riff01", "semitones": 1, "note_ids": ["nt_zzz999"] }),
        ),
    ] {
        let r = call(&fx, tool, args.clone()).await;
        assert_eq!(r.is_error, Some(true), "{tool} {args} は失敗するはず");
    }

    // エラーは履歴に残らない
    let r = call(&fx, "get_history", json!({})).await;
    assert_eq!(ok_json(&r)["entries"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn preset_tools_save_and_apply_across_tracks() {
    // プリセット置き場をテスト用に隔離(default_dir は APPDATA/XDG を見る)
    let preset_tmp = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", preset_tmp.path());
    std::env::set_var("APPDATA", preset_tmp.path());

    let fx = setup().await;
    // 音作り済みのトラック(supersaw + distortion)
    let r = call(
        &fx,
        "apply_commands",
        json!({
            "commands": [
                { "op": "add_track", "track": { "id": "trk_lead01", "name": "Lead", "kind": "midi" } },
                { "op": "set_device", "track": "trk_lead01",
                  "device": { "type": "builtin", "name": "subtractive",
                    "params": { "unison": 7, "detune_cents": 25.0 } } },
                { "op": "add_effect", "track": "trk_lead01",
                  "effect": { "id": "fx_dist01", "type": "builtin", "name": "distortion",
                    "params": { "drive": 6.0 } } },
                { "op": "add_track", "track": { "id": "trk_lead02", "name": "Lead2", "kind": "midi" } },
                { "op": "add_effect", "track": "trk_lead02",
                  "effect": { "id": "fx_rev001", "type": "builtin", "name": "reverb" } }
            ],
            "label": "準備",
        }),
    )
    .await;
    assert_ne!(r.is_error, Some(true), "{:?}", r.content);

    // 保存 → 一覧
    let r = call(
        &fx,
        "save_preset",
        json!({ "track_id": "trk_lead01", "name": "スーパーソー", "description": "EDM リード" }),
    )
    .await;
    let v = ok_json(&r);
    assert_eq!(v["saved"], "スーパーソー");
    assert_eq!(v["effect_count"], 1);

    let r = call(&fx, "list_presets", json!({})).await;
    let presets = ok_json(&r)["presets"].as_array().unwrap().clone();
    assert!(presets.iter().any(|p| p["name"] == "スーパーソー"));

    // 別トラックへ適用: 音源が差し替わり、既存の reverb はプリセットの distortion に置き換わる
    let r = call(
        &fx,
        "load_preset",
        json!({ "track_id": "trk_lead02", "name": "スーパーソー" }),
    )
    .await;
    assert_eq!(ok_json(&r)["applied"], "スーパーソー");

    let r = call(&fx, "get_project", json!({ "track_ids": ["trk_lead02"] })).await;
    let track = &ok_json(&r)["project"]["tracks"][0];
    assert_eq!(track["device"]["name"], "subtractive");
    assert_eq!(track["device"]["params"]["unison"], 7);
    let effects = track["effects"].as_array().unwrap();
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0]["name"], "distortion");
    assert_eq!(effects[0]["params"]["drive"], 6.0);

    // 1 回の undo でまとめて元に戻る(device なし + reverb)
    call(&fx, "undo", json!({})).await;
    let r = call(&fx, "get_project", json!({ "track_ids": ["trk_lead02"] })).await;
    let track = &ok_json(&r)["project"]["tracks"][0];
    assert!(track["device"].is_null());
    assert_eq!(track["effects"][0]["name"], "reverb");

    // 上書き保護と削除
    let r = call(
        &fx,
        "save_preset",
        json!({ "track_id": "trk_lead01", "name": "スーパーソー" }),
    )
    .await;
    assert_eq!(
        r.is_error,
        Some(true),
        "上書きは overwrite なしで失敗するはず"
    );
    let r = call(&fx, "delete_preset", json!({ "name": "スーパーソー" })).await;
    assert_eq!(ok_json(&r)["deleted"], "スーパーソー");

    // エフェクトのプリセット: distortion 1 つを保存し、別トラックへ「外してある」状態で足す
    let r = call(
        &fx,
        "save_effect_preset",
        json!({ "target": "trk_lead01", "fx_id": "fx_dist01", "name": "太い歪み", "note": "リード用" }),
    )
    .await;
    assert_eq!(ok_json(&r)["saved"], "太い歪み");
    let r = call(&fx, "list_effect_presets", json!({})).await;
    let list = ok_json(&r)["effect_presets"].as_array().unwrap().clone();
    assert_eq!(list[0]["kind"], "distortion");
    assert_eq!(list[0]["origin"], "Lead");
    let r = call(&fx, "list_presets", json!({})).await;
    assert!(
        ok_json(&r)["presets"].as_array().unwrap().is_empty(),
        "音色のプリセットの一覧には混ざらない"
    );

    let r = call(
        &fx,
        "load_effect_preset",
        json!({ "target": "trk_lead02", "name": "太い歪み", "parked": true }),
    )
    .await;
    let fx_id = ok_json(&r)["fx_id"].as_str().unwrap().to_owned();
    let r = call(&fx, "get_project", json!({ "track_ids": ["trk_lead02"] })).await;
    let effects = ok_json(&r)["project"]["tracks"][0]["effects"]
        .as_array()
        .unwrap()
        .clone();
    let added = effects.iter().find(|e| e["id"] == fx_id.as_str()).unwrap();
    assert_eq!(added["label"], "太い歪み");
    assert_eq!(added["note"], "リード用");
    assert_eq!(added["parked"], true);
    assert_eq!(added["params"]["drive"], 6.0);

    let r = call(
        &fx,
        "load_effect_preset",
        json!({ "target": "master", "name": "太い歪み" }),
    )
    .await;
    assert_ne!(r.is_error, Some(true), "{:?}", r.content);
    let r = call(&fx, "delete_effect_preset", json!({ "name": "太い歪み" })).await;
    assert_eq!(ok_json(&r)["deleted"], "太い歪み");
}

#[tokio::test]
async fn import_sample_sets_sampler_device() {
    let fx = setup().await;
    call(
        &fx,
        "apply_commands",
        add_track_args("trk_gtr001", "Guitar"),
    )
    .await;

    // テスト用 WAV を書く
    let wav_dir = tempfile::tempdir().unwrap();
    let wav = wav_dir.path().join("riff.wav");
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&wav, spec).unwrap();
    for i in 0..4410 {
        let s = ((i as f32 * 0.03).sin() * 10_000.0) as i16;
        w.write_sample(s).unwrap(); // L
        w.write_sample(s).unwrap(); // R
    }
    w.finalize().unwrap();

    let r = call(
        &fx,
        "import_sample",
        json!({
            "track_id": "trk_gtr001",
            "path": wav.to_string_lossy(),
            "root": 57,
        }),
    )
    .await;
    let v = ok_json(&r);
    let asset_id = v["asset_id"].as_str().unwrap().to_owned();
    assert!(asset_id.starts_with("sha256:"));
    assert_eq!(v["sample_rate"], 44_100);
    assert_eq!(v["frames"], 4410);

    // プロジェクト側: アセット登録 + デバイスが sampler + ファイルコピー済み
    let r = call(&fx, "get_project", json!({})).await;
    let p = ok_json(&r)["project"].clone();
    let asset = &p["assets"][&asset_id];
    assert_eq!(asset["channels"], 2);
    let rel = asset["path"].as_str().unwrap();
    assert!(fx.dir.join(rel).exists(), "audio/ にコピーされるはず");
    let device = &p["tracks"][0]["device"];
    assert_eq!(device["type"], "sampler");
    assert_eq!(device["asset"], asset_id);
    assert_eq!(device["params"]["root"], 57);

    // list_params は sampler のスペックを返す
    let r = call(&fx, "list_params", json!({ "track_id": "trk_gtr001" })).await;
    let v = ok_json(&r);
    assert_eq!(v["device"]["name"], "sampler");
    assert!(v["params"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["name"] == "root" && s["current"] == 57));

    // 1 undo で音源設定ごと戻る
    call(&fx, "undo", json!({})).await;
    let r = call(&fx, "get_project", json!({})).await;
    assert!(ok_json(&r)["project"]["tracks"][0]["device"].is_null());

    // WAV でないファイルはエラー
    let bad = wav_dir.path().join("bad.wav");
    std::fs::write(&bad, b"not a wav").unwrap();
    let r = call(
        &fx,
        "import_sample",
        json!({ "track_id": "trk_gtr001", "path": bad.to_string_lossy() }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
}

#[tokio::test]
async fn analyze_harmony_detects_key_and_chords() {
    let fx = setup().await;
    let r = call(
        &fx,
        "apply_commands",
        json!({
            "commands": [
                { "op": "add_track", "track": { "id": "trk_keys01", "name": "Keys", "kind": "midi" } },
                { "op": "add_clip", "track": "trk_keys01",
                  "clip": { "id": "clp_prog01", "name": "prog", "start": 0, "length": 15360, "kind": "midi",
                    "notes": [
                        { "id": "nt_c00001", "pos": 0, "dur": 3840, "pitch": 60, "vel": 100 },
                        { "id": "nt_e00001", "pos": 0, "dur": 3840, "pitch": 64, "vel": 100 },
                        { "id": "nt_g00001", "pos": 0, "dur": 3840, "pitch": 67, "vel": 100 },
                        { "id": "nt_f00001", "pos": 3840, "dur": 3840, "pitch": 53, "vel": 100 },
                        { "id": "nt_a00001", "pos": 3840, "dur": 3840, "pitch": 57, "vel": 100 },
                        { "id": "nt_c00002", "pos": 3840, "dur": 3840, "pitch": 60, "vel": 100 },
                        { "id": "nt_g00002", "pos": 7680, "dur": 3840, "pitch": 55, "vel": 100 },
                        { "id": "nt_b00001", "pos": 7680, "dur": 3840, "pitch": 59, "vel": 100 },
                        { "id": "nt_d00001", "pos": 7680, "dur": 3840, "pitch": 62, "vel": 100 },
                        { "id": "nt_c00003", "pos": 11520, "dur": 3840, "pitch": 48, "vel": 100 },
                        { "id": "nt_e00002", "pos": 11520, "dur": 3840, "pitch": 64, "vel": 100 },
                        { "id": "nt_g00003", "pos": 11520, "dur": 3840, "pitch": 67, "vel": 100 }
                    ] } }
            ],
            "label": "C → F → G → C",
        }),
    )
    .await;
    assert_ne!(r.is_error, Some(true), "{:?}", r.content);

    let r = call(&fx, "analyze_harmony", json!({})).await;
    let v = ok_json(&r);
    assert_eq!(v["key"]["name"], "C major");
    assert_eq!(v["chords"][0]["chord"], "C");
    assert_eq!(v["chords"][1]["chord"], "F");
    assert_eq!(v["chords"][2]["chord"], "G");
    assert_eq!(v["chords"][3]["chord"], "C");
    assert_eq!(v["note_count"], 12);
}

#[tokio::test]
async fn clap_state_is_elided_for_ai_and_restored_on_apply() {
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_clap01", "Synth")).await);
    let state = "QUJD".repeat(200); // 800 文字の base64
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({
                "label": "CLAP 音源",
                "commands": [{
                    "op": "set_device",
                    "track": "trk_clap01",
                    "device": { "type": "clap", "plugin_id": "org.example.synth", "state": state }
                }]
            }),
        )
        .await,
    );
    // AI には省略表示で見える
    let r = call(&fx, "get_project", json!({})).await;
    let shown = ok_json(&r)["project"]["tracks"][0]["device"]["state"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(shown.contains("省略") && shown.contains("800"), "{shown}");

    // 省略表示のまま送り返しても(音量だけ変えたつもりの丸ごと置換など)状態は壊れない
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({
                "label": "そのまま送り返す",
                "commands": [{
                    "op": "set_device",
                    "track": "trk_clap01",
                    "device": { "type": "clap", "plugin_id": "org.example.synth", "state": shown }
                }]
            }),
        )
        .await,
    );
    let (project, _) = fx.handle.get_project().await.unwrap();
    match &project.tracks[0].device.as_ref().unwrap().source {
        glaux_core::PluginSource::Clap { state: s, .. } => {
            assert_eq!(s.as_deref(), Some(state.as_str()))
        }
        other => panic!("CLAP のまま: {other:?}"),
    }

    // 一覧ツールは呼べる(プラグインが無い環境でも空で返る)
    let r = call(&fx, "list_plugins", json!({})).await;
    assert!(ok_json(&r)["plugins"].is_array());
}

#[tokio::test]
async fn list_params_on_clap_track_returns_effects_without_error() {
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_clap02", "Synth")).await);
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({
                "label": "CLAP 音源",
                "commands": [
                    { "op": "set_device", "track": "trk_clap02",
                      "device": { "type": "clap", "plugin_id": "org.example.synth" } },
                    { "op": "add_effect", "track": "trk_clap02",
                      "effect": { "id": "fx_rev001", "type": "builtin", "name": "reverb" } }
                ]
            }),
        )
        .await,
    );
    let r = call(&fx, "list_params", json!({ "track_id": "trk_clap02" })).await;
    let v = ok_json(&r);
    assert_eq!(v["device"]["name"], "clap");
    assert_eq!(v["params"].as_array().unwrap().len(), 0);
    assert_eq!(v["effects"].as_array().unwrap().len(), 1, "{v}");
}

/// 実プラグインを使う(`GLAUX_TEST_CLAP` 未設定なら何もしない)。
#[tokio::test]
async fn list_params_filters_real_clap_params() {
    let Some(path) = std::env::var_os("GLAUX_TEST_CLAP").map(std::path::PathBuf::from) else {
        eprintln!("GLAUX_TEST_CLAP が未設定のためスキップ");
        return;
    };
    std::env::set_var("GLAUX_CLAP_PATH", path.parent().unwrap());
    let plugin = glaux_engine::plugins::rescan()
        .into_iter()
        .find(|p| p.is_instrument())
        .unwrap();
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_clap03", "Synth")).await);
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({
                "label": "CLAP 音源",
                "commands": [{ "op": "set_device", "track": "trk_clap03",
                               "device": { "type": "clap", "plugin_id": plugin.id } }]
            }),
        )
        .await,
    );
    let r = call(
        &fx,
        "list_params",
        json!({ "track_id": "trk_clap03", "filter": "cutoff", "limit": 5 }),
    )
    .await;
    let v = ok_json(&r);
    let params = v["params"].as_array().unwrap();
    eprintln!(
        "cutoff: {} 件中 {:?}",
        v["params_total"],
        params
            .iter()
            .map(|p| p["display_name"].clone())
            .collect::<Vec<_>>()
    );
    assert!(!params.is_empty() && params.len() <= 5);
    assert!(params[0]["path"]
        .as_str()
        .unwrap()
        .starts_with("device/clap:"));
    // そのまま set_param できる
    let path = params[0]["path"].as_str().unwrap().to_owned();
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "cutoff", "commands": [{ "op": "set_param", "track": "trk_clap03", "path": path, "value": 0.25 }] }),
        )
        .await,
    );
    let r = call(
        &fx,
        "list_params",
        json!({ "track_id": "trk_clap03", "filter": "cutoff", "limit": 5 }),
    )
    .await;
    assert_eq!(ok_json(&r)["params"][0]["current"], json!(0.25));

    // プラグインに届いた後は、変更した値にも画面表示の文字列が付く
    // (このテストはエンジンを動かさないので、共有表に「届いた」値を直接置いて確かめる)
    let id: u32 = path.trim_start_matches("device/clap:").parse().unwrap();
    let tid = glaux_core::TrackId::parse("trk_clap03").unwrap();
    glaux_engine::plugins::set_live_values_for_test(
        &glaux_engine::plugins::PluginOwner::Track(tid.clone()),
        id,
        0.25,
        "123 Hz",
    );
    let r = call(
        &fx,
        "list_params",
        json!({ "track_id": "trk_clap03", "filter": "cutoff", "limit": 5 }),
    )
    .await;
    assert_eq!(ok_json(&r)["params"][0]["current_text"], json!("123 Hz"));
}

/// 実プラグインを使う(`GLAUX_TEST_CLAP` 未設定、またはプリセットが無ければ何もしない)。
#[tokio::test]
async fn plugin_presets_list_load_and_undo() {
    let Some(path) = std::env::var_os("GLAUX_TEST_CLAP").map(std::path::PathBuf::from) else {
        eprintln!("GLAUX_TEST_CLAP が未設定のためスキップ");
        return;
    };
    std::env::set_var("GLAUX_CLAP_PATH", path.parent().unwrap());
    let plugin = glaux_engine::plugins::rescan()
        .into_iter()
        .find(|p| p.is_instrument())
        .unwrap();
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_clap04", "Synth")).await);
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({
                "label": "CLAP 音源",
                "commands": [{ "op": "set_device", "track": "trk_clap04",
                               "device": { "type": "clap", "plugin_id": plugin.id } }]
            }),
        )
        .await,
    );
    let r = call(
        &fx,
        "list_plugin_presets",
        json!({ "track_id": "trk_clap04", "filter": "pad" }),
    )
    .await;
    let v = ok_json(&r).clone();
    eprintln!(
        "{} 件中 {} 件: {}",
        v["total"], v["matched"], v["categories"]
    );
    let Some(first) = v["presets"].as_array().and_then(|a| a.first()).cloned() else {
        eprintln!("プリセットが無いため読み込みは確認しない");
        return;
    };
    let r = call(
        &fx,
        "load_plugin_preset",
        json!({ "track_id": "trk_clap04", "preset": first["id"] }),
    )
    .await;
    assert_eq!(ok_json(&r)["preset"], first["name"]);
    let (project, _) = fx.handle.get_project().await.unwrap();
    let dev = project.tracks[0].device.clone().unwrap();
    assert_eq!(
        dev.params.get("preset"),
        Some(&glaux_core::ParamValue::Enum(
            first["name"].as_str().unwrap().to_owned()
        ))
    );
    let glaux_core::PluginSource::Clap { state, .. } = &dev.source else {
        panic!("CLAP のまま");
    };
    assert!(state.is_some(), "読み込んだ音色の状態が保存される");
    // 一覧の current_preset にも出る
    let r = call(
        &fx,
        "list_plugin_presets",
        json!({ "track_id": "trk_clap04", "limit": 1 }),
    )
    .await;
    assert_eq!(ok_json(&r)["current_preset"], first["name"]);
    // 取り消すと元(状態なし)に戻る
    ok_json(&call(&fx, "undo", json!({})).await);
    let (project, _) = fx.handle.get_project().await.unwrap();
    let dev = project.tracks[0].device.clone().unwrap();
    assert!(!dev.params.contains_key("preset"));
}

#[tokio::test]
async fn analyze_sound_describes_track_note_and_file() {
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_snd001", "Lead")).await);
    // トラックの音源(既定の subtractive)で A4 を鳴らす
    let r = call(
        &fx,
        "analyze_sound",
        json!({ "track_id": "trk_snd001", "pitch": 69 }),
    )
    .await;
    let v = ok_json(&r);
    eprintln!("{}", serde_json::to_string(&v["labels"]).unwrap());
    assert_eq!(v["pitch"]["midi"], json!(69));
    assert!(v["envelope"]["attack_ms"].is_number());
    assert!(v["harmonics"]["waveform_guess"].is_string());
    assert!(v["labels"].as_array().unwrap().len() >= 3);
    // CLAP のモデルがあれば音色語(カテゴリごと)、無ければ取得方法の案内
    if glaux_ml::clap::available() {
        eprintln!("{}", serde_json::to_string(&v["words"]).unwrap());
        assert_eq!(v["words"]["instrument"].as_array().unwrap().len(), 3);
        assert!(v["words"]["mood"].is_array());
    } else {
        assert!(v["words"].is_null());
        assert!(v["words_note"].as_str().unwrap().contains("CLAP"));
    }

    // WAV ファイル(矩形波寄り: 奇数倍音だけ)
    let path = fx.dir.join("square.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for i in 0..44_100 {
        let t = i as f32 / 44_100.0;
        let mut x = 0.0;
        for k in (1..30).step_by(2) {
            x += (std::f32::consts::TAU * 220.0 * k as f32 * t).sin() / k as f32;
        }
        w.write_sample((x * 8000.0) as i16).unwrap();
    }
    w.finalize().unwrap();
    let r = call(
        &fx,
        "analyze_sound",
        json!({ "file": path.to_string_lossy() }),
    )
    .await;
    let v = ok_json(&r);
    assert_eq!(v["harmonics"]["waveform_guess"], json!("square"));
    assert_eq!(v["pitch"]["midi"], json!(57));

    // ビブラート(5.5Hz・±40 セント)付きの音。音程は SwiftF0 で測られる
    let path = fx.dir.join("vibrato.wav");
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    let mut phase = 0.0f64;
    for i in 0..(44_100 * 2) {
        let t = i as f64 / 44_100.0;
        let f = 330.0 * 2f64.powf(40.0 * (std::f64::consts::TAU * 5.5 * t).sin() / 1200.0);
        phase += std::f64::consts::TAU * f / 44_100.0;
        let x: f64 = (1..=5).map(|k| (phase * k as f64).sin() / k as f64).sum();
        w.write_sample((x * 8000.0) as i16).unwrap();
    }
    w.finalize().unwrap();
    let r = call(
        &fx,
        "analyze_sound",
        json!({ "file": path.to_string_lossy() }),
    )
    .await;
    let v = ok_json(&r);
    let p = &v["pitch"];
    assert_eq!(p["midi"], json!(64), "{p}");
    let rate = p["vibrato_rate_hz"].as_f64().unwrap();
    let depth = p["vibrato_depth_cents"].as_f64().unwrap();
    assert!((4.5..6.5).contains(&rate), "{p}");
    assert!((25.0..60.0).contains(&depth), "{p}");

    // 対象の指定が無い・複数ならエラー
    let r = call(&fx, "analyze_sound", json!({})).await;
    assert_eq!(r.is_error, Some(true));
}

/// 簡単なドラムのループ(キック・スネア・ハイハット)を WAV に書く。
fn write_drum_wav(path: &std::path::Path, bpm: f64, bars: usize, sr: u32) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: sr,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let beat = 60.0 / bpm;
    let n = (bars as f64 * 4.0 * beat * sr as f64) as usize;
    let mut x = vec![0.0f64; n];
    let mut seed = 7u32;
    let mut noise = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) as f64 / (1u32 << 24) as f64 * 2.0 - 1.0
    };
    for k in 0..bars * 8 {
        let s = (k as f64 * beat / 2.0 * sr as f64) as usize;
        let pos = (k / 2) % 4;
        for i in 0..(0.25 * sr as f64) as usize {
            if s + i >= n {
                break;
            }
            let t = i as f64 / sr as f64;
            let mut v = 0.15 * noise() * (-t * 60.0).exp();
            if k % 2 == 0 && pos % 2 == 0 {
                v += 0.9
                    * (std::f64::consts::TAU * (50.0 + 100.0 * (-t * 30.0).exp()) * t).sin()
                    * (-t * 12.0).exp();
            }
            if k % 2 == 0 && pos % 2 == 1 {
                v += 0.5 * noise() * (-t * 20.0).exp();
            }
            x[s + i] += v;
        }
    }
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for v in x {
        w.write_sample((v.clamp(-1.0, 1.0) * 20000.0) as i16)
            .unwrap();
    }
    w.finalize().unwrap();
}

#[tokio::test]
async fn analyze_beats_detects_tempo_of_file() {
    let fx = setup().await;
    let path = fx.dir.join("drums.wav");
    write_drum_wav(&path, 96.0, 8, 44_100);
    let r = call(
        &fx,
        "analyze_beats",
        json!({ "file": path.to_string_lossy() }),
    )
    .await;
    let v = ok_json(&r);
    eprintln!("{v}");
    let bpm = v["bpm"].as_f64().unwrap();
    assert!((bpm - 96.0).abs() < 0.5, "{v}");
    assert!(v["summary"].as_str().unwrap().contains("BPM"));
    assert!(!v["bpm_alternatives"].as_array().unwrap().is_empty());

    let r = call(&fx, "analyze_beats", json!({})).await;
    assert_eq!(r.is_error, Some(true));
}

/// 実際の曲で公式実装(Python)の結果と比べる(`GLAUX_TEST_BEAT_AUDIO` と `GLAUX_TEST_BEAT_GOLDEN`
/// 未設定なら何もしない)。beat-this-rs の test_files と tests/fixtures/golden_small.json を使う想定。
#[test]
fn beats_match_reference_implementation() {
    let (Some(audio), Some(golden)) = (
        std::env::var_os("GLAUX_TEST_BEAT_AUDIO"),
        std::env::var_os("GLAUX_TEST_BEAT_GOLDEN"),
    ) else {
        eprintln!("GLAUX_TEST_BEAT_AUDIO / GLAUX_TEST_BEAT_GOLDEN が未設定のためスキップ");
        return;
    };
    let sound = glaux_mcp::sound::load_file(std::path::Path::new(&audio)).unwrap();
    let t0 = std::time::Instant::now();
    let r = glaux_ml::beats::track(&sound.frames, sound.sample_rate).unwrap();
    let g: Value = serde_json::from_slice(&std::fs::read(golden).unwrap()).unwrap();
    // MIR の F 値(±70ms)
    let f_measure = |est: &[f32], refs: &Value| {
        let refs: Vec<f64> = refs
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let mut used = vec![false; refs.len()];
        let mut hit = 0;
        for &e in est {
            if let Some(i) =
                (0..refs.len()).find(|&i| !used[i] && (refs[i] - e as f64).abs() <= 0.07)
            {
                used[i] = true;
                hit += 1;
            }
        }
        let p = hit as f64 / est.len().max(1) as f64;
        let rc = hit as f64 / refs.len().max(1) as f64;
        if p + rc == 0.0 {
            0.0
        } else {
            2.0 * p * rc / (p + rc)
        }
    };
    let fb = f_measure(&r.beats, &g["beats"]);
    let fd = f_measure(&r.downbeats, &g["downbeats"]);
    eprintln!(
        "{:.1} 秒の曲を {:?} で解析。ビート F={fb:.3}、小節頭 F={fd:.3}、{:?} BPM、{:?} 拍子",
        sound.frames.len() as f32 / sound.sample_rate,
        t0.elapsed(),
        r.bpm(),
        r.beats_per_bar()
    );
    assert!(fb > 0.95, "{fb}");
    assert!(fd > 0.9, "{fd}");
}

/// 実際に CLAP のモデルを取得する(約 280MB。`GLAUX_TEST_DOWNLOAD_CLAP` に保存先のファイルを指定したときだけ)。
#[test]
fn clap_model_downloads_and_verifies() {
    let Some(dest) = std::env::var_os("GLAUX_TEST_DOWNLOAD_CLAP") else {
        eprintln!("GLAUX_TEST_DOWNLOAD_CLAP が未設定のためスキップ");
        return;
    };
    std::env::set_var("GLAUX_CLAP_MODEL", &dest);
    let _ = std::fs::remove_file(&dest);
    let mut calls = 0;
    let p = glaux_mcp::models::download_clap(&mut |_, _| calls += 1).unwrap();
    assert!(p.is_file());
    assert!(calls > 100);
    assert!(glaux_mcp::models::clap_status().available);
}

fn write_mono_wav(path: &std::path::Path, frames: &[f32], sr: u32) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: sr,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for v in frames {
        w.write_sample((v.clamp(-1.0, 1.0) * 20000.0) as i16)
            .unwrap();
    }
    w.finalize().unwrap();
}

#[tokio::test]
async fn compare_sounds_reports_distance_and_differences() {
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_cmp001", "Lead")).await);
    let note = json!({ "track_id": "trk_cmp001", "pitch": 57, "duration_ms": 600 });
    let r = call(&fx, "compare_sounds", json!({ "a": note, "b": note })).await;
    let v = ok_json(&r);
    assert!(v["distance"]["total"].as_f64().unwrap() < 0.05, "{v}");
    assert_eq!(v["verdict"], json!("ほぼ同じ音"));

    // 暗くゆっくり立ち上がる矩形波と比べる
    use glaux_core::ParamValue;
    let mut p = glaux_core::ParamMap::new();
    p.insert("waveform".into(), ParamValue::Enum("square".into()));
    p.insert("cutoff".into(), ParamValue::Float(500.0));
    p.insert("attack".into(), ParamValue::Float(0.4));
    let y = glaux_engine::sound_match::render_subtractive(&p, 57, 0.6, 44_100, 44_100.0);
    let path = fx.dir.join("slow_dark.wav");
    write_mono_wav(&path, &y, 44_100);
    let r = call(
        &fx,
        "compare_sounds",
        json!({ "a": note, "b": { "file": path.to_string_lossy() } }),
    )
    .await;
    let v = ok_json(&r);
    eprintln!("{v}");
    assert!(v["distance"]["total"].as_f64().unwrap() > 0.35, "{v}");
    let diffs = v["differences"].as_array().unwrap();
    let text = diffs
        .iter()
        .map(|d| d.as_str().unwrap())
        .collect::<Vec<_>>()
        .join(" / ");
    assert!(text.contains("暗い"), "{text}");
    assert!(text.contains("立ち上がりが遅い"), "{text}");
}

#[tokio::test]
async fn match_sound_fits_subtractive_and_can_be_undone() {
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_mat001", "Copy")).await);
    // 目標: レゾナンスの効いた暗めのノコギリ波のプラック
    use glaux_core::ParamValue;
    let mut p = glaux_core::ParamMap::new();
    p.insert("waveform".into(), ParamValue::Enum("saw".into()));
    p.insert("cutoff".into(), ParamValue::Float(700.0));
    p.insert("resonance".into(), ParamValue::Float(0.5));
    p.insert("decay".into(), ParamValue::Float(0.25));
    p.insert("sustain".into(), ParamValue::Float(0.0));
    p.insert("filter_env".into(), ParamValue::Float(0.7));
    let y = glaux_engine::sound_match::render_subtractive(&p, 48, 0.8, 48_000, 48_000.0);
    let path = fx.dir.join("pluck.wav");
    write_mono_wav(&path, &y, 48_000);
    let r = call(
        &fx,
        "match_sound",
        json!({ "file": path.to_string_lossy(), "track_id": "trk_mat001", "max_seconds": 8 }),
    )
    .await;
    let v = ok_json(&r);
    eprintln!("{} verified {}", v["match"], v["verified_distance"]);
    let m = &v["match"];
    assert_eq!(m["pitch"], json!(48));
    assert!(m["distance"].as_f64().unwrap() < m["initial_distance"].as_f64().unwrap());
    assert!(m["distance"].as_f64().unwrap() < 0.35, "{m}");
    assert!(v["verified_distance"].as_f64().unwrap() < 0.3, "{v}");
    // トラックの音源に反映され、undo で戻る
    let r = call(&fx, "get_project", json!({})).await;
    let proj = ok_json(&r);
    let dev = &proj["project"]["tracks"][0]["device"];
    assert_eq!(dev["name"], json!("subtractive"), "{dev}");
    assert!(dev["params"]["cutoff"].is_number());
    ok_json(&call(&fx, "undo", json!({})).await);
    let r = call(&fx, "get_project", json!({})).await;
    let proj = ok_json(&r);
    assert!(proj["project"]["tracks"][0]["device"]["params"]["cutoff"].is_null());
}

#[tokio::test]
async fn match_clip_commands_create_a_resembling_track() {
    let fx = setup().await;
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "音声トラック", "commands": [
                { "op": "add_track", "track": { "id": "trk_aud001", "name": "Sample", "kind": "audio" } }
            ] }),
        )
        .await,
    );
    use glaux_core::ParamValue;
    let mut p = glaux_core::ParamMap::new();
    p.insert("waveform".into(), ParamValue::Enum("square".into()));
    p.insert("cutoff".into(), ParamValue::Float(1500.0));
    p.insert("attack".into(), ParamValue::Float(0.1));
    let y = glaux_engine::sound_match::render_subtractive(&p, 60, 0.7, 44_100, 44_100.0);
    let path = fx.dir.join("lead.wav");
    write_mono_wav(&path, &y, 44_100);
    let r = call(
        &fx,
        "import_audio_clip",
        json!({ "track_id": "trk_aud001", "path": path.to_string_lossy(), "start_tick": 1920 }),
    )
    .await;
    let clip_id = ok_json(&r)["clip_id"].as_str().unwrap().to_owned();
    let (project, _) = fx.handle.get_project().await.unwrap();
    let cid = glaux_core::ClipId::parse(&clip_id).unwrap();
    let m = glaux_mcp::sound::match_clip_commands(&project, &fx.dir, &cid, 4.0).unwrap();
    assert_eq!(m.commands.len(), 2);
    assert_eq!(m.outcome.pitch, 60);
    assert!(
        m.outcome.fit.distance.total < 0.35,
        "{:?}",
        m.outcome.fit.distance
    );
    // 適用すると元のトラックの直後に、同じ位置の 1 音つきで入る
    let mut view = project.clone();
    for c in &m.commands {
        view.apply(c).unwrap();
    }
    assert_eq!(view.tracks[1].id, m.track_id);
    assert_eq!(view.tracks[1].clips[0].start, glaux_core::Tick(1920));
}

/// 実プラグインで、あるプリセットの音を目標にして同じプリセットが見つかるか
/// (`GLAUX_TEST_CLAP` 未設定、またはプリセットが 2 つ未満なら何もしない)。
#[tokio::test]
async fn find_similar_presets_finds_the_source_preset() {
    let Some(path) = std::env::var_os("GLAUX_TEST_CLAP").map(std::path::PathBuf::from) else {
        eprintln!("GLAUX_TEST_CLAP が未設定のためスキップ");
        return;
    };
    std::env::set_var("GLAUX_CLAP_PATH", path.parent().unwrap());
    // 索引のキャッシュを一時フォルダに
    let cache = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cache.path());
    let plugin = glaux_engine::plugins::rescan()
        .into_iter()
        .find(|p| p.is_instrument())
        .unwrap();
    let list = glaux_engine::plugins::presets(&plugin.id, false).unwrap();
    if list.len() < 2 {
        eprintln!("プリセットが少ないため確認しない");
        return;
    }
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_clap05", "Synth")).await);
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({
                "label": "CLAP 音源",
                "commands": [{ "op": "set_device", "track": "trk_clap05",
                               "device": { "type": "clap", "plugin_id": plugin.id } }]
            }),
        )
        .await,
    );
    // 目標: 2 番目のプリセットを E4 で鳴らした音
    let source = list[1].clone();
    let mut target = None;
    glaux_engine::plugins::render_presets(
        &plugin.id,
        std::slice::from_ref(&source),
        glaux_engine::plugins::PresetRenderSpec {
            pitch: 64,
            velocity: 0.8,
            hold: 0.8,
            total: 1.5,
            sample_rate: 44_100.0,
        },
        |_, r| {
            target = r.ok();
            true
        },
    )
    .unwrap();
    let target = target.unwrap();
    let wav = fx.dir.join("target.wav");
    write_mono_wav(&wav, &target, 44_100);
    let r = call(
        &fx,
        "find_similar_presets",
        json!({ "file": wav.to_string_lossy(), "track_id": "trk_clap05", "limit": 3 }),
    )
    .await;
    let v = ok_json(&r);
    eprintln!("{v}");
    assert_eq!(v["index"]["total"], v["index"]["indexed"]);
    assert_eq!(v["results"][0]["id"], json!(source.id()), "{v}");
    // サンプルレート(44.1k と 48k)と押していた長さの推定の違いがあるので 0 にはならないが、2 位とは大差
    let d0 = v["results"][0]["distance"].as_f64().unwrap();
    assert!(d0 < 0.5, "{v}");
    if let Some(d1) = v["results"][1]["distance"].as_f64() {
        assert!(d1 > d0 * 2.0, "{v}");
    }
    // 2 回目は索引を作らずに済む
    let r = call(
        &fx,
        "find_similar_presets",
        json!({ "file": wav.to_string_lossy(), "track_id": "trk_clap05", "index_seconds": 0 }),
    )
    .await;
    assert_eq!(ok_json(&r)["index"]["added"], json!(0));
}

/// 実楽器(SoundFont)の 1 音に内蔵シンセを合わせたときの近さと CLAP の語を表示する評価用
/// (`GLAUX_TEST_SF2` に FluidR3_GM.sf2 などを指定したときだけ。約 2 分)。
#[tokio::test]
async fn real_instruments_fit_report() {
    let Some(sf) = std::env::var_os("GLAUX_TEST_SF2") else {
        eprintln!("GLAUX_TEST_SF2 が未設定のためスキップ");
        return;
    };
    let fx = setup().await;
    for (i, (preset, name, pitch)) in [
        (0u16, "piano", 60u8),
        (33, "fingered bass", 40),
        (73, "flute", 72),
        (81, "saw lead", 64),
        (89, "warm pad", 60),
    ]
    .iter()
    .enumerate()
    {
        let tid = format!("trk_sf{i:04}");
        ok_json(&call(&fx, "apply_commands", add_track_args(&tid, name)).await);
        ok_json(&call(&fx, "set_soundfont_instrument", json!({"track_id": tid, "soundfont": sf.to_string_lossy(), "bank": 0, "preset": preset})).await);
        let (project, _) = fx.handle.get_project().await.unwrap();
        let t = glaux_core::TrackId::parse(&tid).unwrap();
        let target =
            glaux_mcp::sound::render_note(&project, &fx.dir, &t, *pitch, 100, 1.0).unwrap();
        let o = glaux_mcp::sound::match_sound(&target, None, true, 20.0);
        let words = if glaux_ml::clap::available() {
            let e = glaux_mcp::sound::embedding(&target).unwrap();
            let w = glaux_ml::clap::describe(&e, 2);
            w.iter()
                .filter(|w| w.category == "instrument" || w.category == "tone")
                .map(|w| w.ja.clone())
                .collect::<Vec<_>>()
                .join(",")
        } else {
            String::new()
        };
        let j = glaux_mcp::sound::match_json(&o);
        eprintln!(
            "{name}: {:.3} → {:.3} ({}) {} + リバーブ {} | {words}",
            o.fit.initial_distance.total,
            o.fit.distance.total,
            j["verdict"],
            j["instrument"],
            j["reverb"]
        );
    }
}

#[tokio::test]
async fn clap_effect_can_be_added_listed_and_state_elided() {
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_cfx001", "Lead")).await);
    // 見つからないプラグインでも挿せる(鳴らすときは素通し)
    let r = call(
        &fx,
        "apply_commands",
        json!({ "label": "CLAP エフェクト", "commands": [
            { "op": "add_effect", "track": "trk_cfx001",
              "effect": { "id": "fx_cfx001", "type": "clap", "plugin_id": "com.example.missing" } },
            { "op": "set_effect_state", "id": "fx_cfx001", "state": "c3RhdGUtZGF0YQ==" },
            { "op": "set_param", "track": "trk_cfx001", "path": "fx/fx_cfx001/clap:7", "value": 0.5 }
        ] }),
    )
    .await;
    ok_json(&r);
    let r = call(&fx, "list_params", json!({ "track_id": "trk_cfx001" })).await;
    let v = ok_json(&r);
    let e = &v["effects"][0];
    assert_eq!(e["name"], json!("clap"), "{v}");
    assert_eq!(e["missing"], json!(true));
    // 状態は get_project で省略表示され、そのまま送り返しても今の状態が保たれる
    let r = call(&fx, "get_project", json!({})).await;
    let state = ok_json(&r)["project"]["tracks"][0]["effects"][0]["state"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(state.starts_with("(省略"), "{state}");
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "そのまま", "commands": [
                { "op": "set_effect_state", "id": "fx_cfx001", "state": state }
            ] }),
        )
        .await,
    );
    let (project, _) = fx.handle.get_project().await.unwrap();
    match &project.tracks[0].effects[0].source {
        glaux_core::PluginSource::Clap { state, .. } => {
            assert_eq!(state.as_deref(), Some("c3RhdGUtZGF0YQ=="))
        }
        other => panic!("{other:?}"),
    }
    // 鳴らしても落ちない(素通し)
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "音", "commands": [
                { "op": "add_clip", "track": "trk_cfx001", "clip": {
                    "id": "clp_cfx001", "name": "c", "start": 0, "length": 1920, "kind": "midi",
                    "notes": [{ "id": "nt_cfx001", "pos": 0, "dur": 480, "pitch": 60, "vel": 100 }] } }
            ] }),
        )
        .await,
    );
    let r = call(&fx, "analyze_audio", json!({})).await;
    assert!(ok_json(&r)["loudness_lufs"].as_f64().unwrap() < 0.0);
}

/// 実プラグイン(`GLAUX_TEST_CLAP_FX`)のエフェクトを挿し、つまみが一覧に出て動かせる。
#[tokio::test]
async fn real_clap_effect_params_are_listed_and_set() {
    let Some(path) = std::env::var_os("GLAUX_TEST_CLAP_FX").map(std::path::PathBuf::from) else {
        eprintln!("GLAUX_TEST_CLAP_FX が未設定のためスキップ");
        return;
    };
    std::env::set_var("GLAUX_CLAP_PATH", path.parent().unwrap());
    let plugin = glaux_engine::plugins::rescan()
        .into_iter()
        .find(|p| p.is_effect() && !p.is_instrument())
        .unwrap();
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_cfx002", "Lead")).await);
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "CLAP エフェクト", "commands": [
                { "op": "add_master_effect",
                  "effect": { "id": "fx_cfx002", "type": "clap", "plugin_id": plugin.id } }
            ] }),
        )
        .await,
    );
    // マスターのエフェクトのつまみは list_params(track_id なし)の master_effects に出る
    let r = call(&fx, "list_params", json!({})).await;
    let e = ok_json(&r)["master_effects"][0].clone();
    eprintln!("{} のつまみ {} 個", e["plugin_name"], e["param_total"]);
    assert_eq!(e["missing"], json!(false));
    let params = e["params"].as_array().unwrap();
    assert!(!params.is_empty());
    let path = params[0]["path"].as_str().unwrap().to_owned();
    assert!(path.starts_with("fx/fx_cfx002/clap:"), "{path}");
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "つまみ", "commands": [
                { "op": "set_master_param", "path": path, "value": 0.7 }
            ] }),
        )
        .await,
    );
    let (project, _) = fx.handle.get_project().await.unwrap();
    let list = glaux_mcp::server::effects_json(&project.master.effects, None);
    assert_eq!(list[0]["params"][0]["current"], json!(0.7));
}

#[tokio::test]
async fn bus_and_send_via_apply_commands() {
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_snd101", "Lead")).await);
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "リバーブのバス", "commands": [
                { "op": "add_track", "track": { "id": "trk_bus101", "name": "Reverb", "kind": "bus" } },
                { "op": "add_effect", "track": "trk_bus101",
                  "effect": { "id": "fx_bus101", "type": "builtin", "name": "reverb", "params": { "mix": 1.0 } } },
                { "op": "add_clip", "track": "trk_snd101", "clip": {
                    "id": "clp_snd101", "name": "c", "start": 0, "length": 1920, "kind": "midi",
                    "notes": [{ "id": "nt_snd101", "pos": 0, "dur": 240, "pitch": 72, "vel": 110 }] } }
            ] }),
        )
        .await,
    );
    let r = call(&fx, "analyze_audio", json!({})).await;
    let dry = ok_json(&r)["duration_seconds"].as_f64().unwrap();
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "センド", "commands": [
                { "op": "set_send", "track": "trk_snd101", "target": "trk_bus101", "level_db": -3.0 }
            ] }),
        )
        .await,
    );
    let r = call(&fx, "get_project", json!({})).await;
    let v = ok_json(&r);
    assert_eq!(
        v["project"]["tracks"][0]["sends"][0]["target"],
        json!("trk_bus101")
    );
    assert_eq!(v["project"]["tracks"][1]["kind"], json!("bus"));
    // リバーブの残響ぶん長く鳴る
    let r = call(&fx, "analyze_audio", json!({})).await;
    let wet = ok_json(&r)["duration_seconds"].as_f64().unwrap();
    assert!(wet > dry, "{wet} vs {dry}");
    // バスへのセンドは取り消せる
    ok_json(&call(&fx, "undo", json!({})).await);
    let (project, _) = fx.handle.get_project().await.unwrap();
    assert!(project.tracks[0].sends.is_empty());
    // バスから送る・バスにクリップを置くのはエラー
    let r = call(
        &fx,
        "apply_commands",
        json!({ "label": "x", "commands": [
            { "op": "set_send", "track": "trk_bus101", "target": "trk_bus101", "level_db": 0.0 }
        ] }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
    let r = call(
        &fx,
        "apply_commands",
        json!({ "label": "x", "commands": [
            { "op": "add_clip", "track": "trk_bus101", "clip": {
                "id": "clp_bus101", "name": "c", "start": 0, "length": 480, "kind": "midi", "notes": [] } }
        ] }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
}

/// エフェクト枠の CLAP プラグインにプリセットを読み込む(`GLAUX_TEST_CLAP` のプリセットを使う。
/// 読み込みの仕組みはプラグインの種類によらないので、プリセットのある音源プラグインで確かめる)。
#[tokio::test]
async fn preset_loads_into_clap_effect_and_undoes() {
    let Some(path) = std::env::var_os("GLAUX_TEST_CLAP").map(std::path::PathBuf::from) else {
        eprintln!("GLAUX_TEST_CLAP が未設定のためスキップ");
        return;
    };
    std::env::set_var("GLAUX_CLAP_PATH", path.parent().unwrap());
    let plugin = glaux_engine::plugins::rescan()
        .into_iter()
        .find(|p| p.is_instrument())
        .unwrap();
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_fxp001", "Lead")).await);
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "CLAP", "commands": [
                { "op": "add_effect", "track": "trk_fxp001",
                  "effect": { "id": "fx_fxp001", "type": "clap", "plugin_id": plugin.id,
                              "params": { "clap:1": 0.5 } } }
            ] }),
        )
        .await,
    );
    let r = call(&fx, "list_plugin_presets", json!({ "fx_id": "fx_fxp001" })).await;
    let v = ok_json(&r).clone();
    let Some(first) = v["presets"].as_array().and_then(|a| a.first()).cloned() else {
        eprintln!("プリセットが無いため確認しない");
        return;
    };
    let r = call(
        &fx,
        "load_plugin_preset",
        json!({ "fx_id": "fx_fxp001", "preset": first["id"] }),
    )
    .await;
    assert_eq!(ok_json(&r)["preset"], first["name"]);
    let (project, _) = fx.handle.get_project().await.unwrap();
    let e = &project.tracks[0].effects[0];
    match &e.source {
        glaux_core::PluginSource::Clap { state, .. } => assert!(state.is_some()),
        other => panic!("{other:?}"),
    }
    assert!(!e.params.contains_key("clap:1"), "上書き値は消える");
    assert_eq!(
        e.params.get("preset"),
        Some(&glaux_core::ParamValue::Enum(
            first["name"].as_str().unwrap().to_owned()
        ))
    );
    // 1 回の undo で元に戻る
    ok_json(&call(&fx, "undo", json!({})).await);
    let (project, _) = fx.handle.get_project().await.unwrap();
    let e = &project.tracks[0].effects[0];
    assert!(e.params.contains_key("clap:1"));
    assert!(matches!(
        &e.source,
        glaux_core::PluginSource::Clap { state: None, .. }
    ));
}

#[tokio::test]
async fn swing_notes_moves_offbeats_and_matches_analysis() {
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_swg001", "Hat")).await);
    let notes: Vec<Value> = (0..16)
        .map(|i| json!({ "id": format!("nt_swg{i:03}"), "pos": i * 480, "dur": 120, "pitch": 42, "vel": 90 }))
        .collect();
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "ハット", "commands": [
                { "op": "add_clip", "track": "trk_swg001", "clip": {
                    "id": "clp_swg001", "name": "c", "start": 0, "length": 7680, "kind": "midi", "notes": notes } }
            ] }),
        )
        .await,
    );
    let r = call(
        &fx,
        "swing_notes",
        json!({ "clip_id": "clp_swg001", "swing": 0.6667 }),
    )
    .await;
    ok_json(&r);
    // 解析すると 3 連スウィング(swing_ratio ≈ 1.33)
    let r = call(&fx, "analyze_rhythm", json!({})).await;
    let v = ok_json(&r);
    let ratio = v["swing_ratio"]
        .as_f64()
        .or_else(|| v["rhythm"]["swing_ratio"].as_f64());
    eprintln!("{v}");
    assert!((ratio.unwrap() - 1.333).abs() < 0.02, "{v}");
    // 表は動かない、undo で戻る
    let (project, _) = fx.handle.get_project().await.unwrap();
    let ns = project.tracks[0].clips[0].notes().unwrap();
    assert_eq!(ns[0].pos.0, 0);
    assert_eq!(ns[1].pos.0, 640);
    ok_json(&call(&fx, "undo", json!({})).await);
    let (project, _) = fx.handle.get_project().await.unwrap();
    assert_eq!(project.tracks[0].clips[0].notes().unwrap()[1].pos.0, 480);
    // 範囲外はエラー
    let r = call(
        &fx,
        "swing_notes",
        json!({ "clip_id": "clp_swg001", "swing": 0.9 }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
    // 複数のクリップにまとめて掛け(1 回の undo で戻る)、続けて apply_groove を促す
    let notes2: Vec<Value> = (0..8)
        .map(|i| json!({ "pos": i * 480, "dur": 120, "pitch": 40, "vel": 90 }))
        .collect();
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "2 つ目", "commands": [
                { "op": "add_clip", "track": "trk_swg001", "clip": {
                    "id": "clp_swg002", "name": "d", "start": 7680, "length": 3840, "kind": "midi", "notes": notes2 } }
            ] }),
        )
        .await,
    );
    let v = ok_json(
        &call(
            &fx,
            "swing_notes",
            json!({ "clip_ids": ["clp_swg001", "clp_swg002"], "swing": 0.6 }),
        )
        .await,
    );
    assert_eq!(v["changed"], 12, "{v}");
    assert!(v["next"].as_str().unwrap().contains("apply_groove"));
    ok_json(&call(&fx, "undo", json!({})).await);
    let (project, _) = fx.handle.get_project().await.unwrap();
    assert!(project.tracks[0]
        .clips
        .iter()
        .flat_map(|c| c.notes().unwrap())
        .all(|n| n.pos.0 % 480 == 0));
}

#[tokio::test]
async fn match_sound_picks_fm_for_a_bell_and_adds_reverb() {
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_bel001", "Bell")).await);
    use glaux_core::ParamValue;
    let mut p = glaux_core::ParamMap::new();
    for (k, v) in [
        ("ratio", 3.5),
        ("index", 5.0),
        ("index_decay", 0.6),
        ("index_sustain", 0.1),
        ("decay", 1.5),
        ("sustain", 0.0),
        ("release", 1.0),
    ] {
        p.insert(k.into(), ParamValue::Float(v));
    }
    let mut y = glaux_engine::sound_match::render_instrument("fm", &p, 72, 1.0, 72_000, 48_000.0);
    glaux_engine::sound_match::apply_reverb(&mut y, 0.4, 0.7, 48_000.0);
    let path = fx.dir.join("bell.wav");
    write_mono_wav(&path, &y, 48_000);
    let r = call(
        &fx,
        "match_sound",
        json!({ "file": path.to_string_lossy(), "track_id": "trk_bel001", "max_seconds": 16, "reverb": true }),
    )
    .await;
    let v = ok_json(&r);
    eprintln!("{}", v["match"]);
    assert_eq!(v["match"]["instrument"], json!("fm"), "{}", v["match"]);
    // 探索は評価回数で打ち切るので、CPU の混み具合に関係なく同じ結果になる(0.299)
    let d = v["match"]["distance"].as_f64().unwrap();
    assert!(d < 0.35, "{d}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    let t = &project.tracks[0];
    assert!(
        matches!(&t.device.as_ref().unwrap().source, glaux_core::PluginSource::Builtin { name } if name == "fm")
    );
    assert!(t.effects.iter().any(
        |e| matches!(&e.source, glaux_core::PluginSource::Builtin { name } if name == "reverb")
    ));
    // 1 回の undo で音源もリバーブも戻る
    ok_json(&call(&fx, "undo", json!({})).await);
    let (project, _) = fx.handle.get_project().await.unwrap();
    assert!(project.tracks[0].effects.is_empty());
}

/// CLAP 音源のつまみを目標の音に合わせる(`GLAUX_TEST_CLAP`)。
#[tokio::test]
async fn refine_plugin_params_fits_and_undoes() {
    let Some(path) = std::env::var_os("GLAUX_TEST_CLAP").map(std::path::PathBuf::from) else {
        eprintln!("GLAUX_TEST_CLAP が未設定のためスキップ");
        return;
    };
    std::env::set_var("GLAUX_CLAP_PATH", path.parent().unwrap());
    let plugin = glaux_engine::plugins::rescan()
        .into_iter()
        .find(|p| p.is_instrument())
        .unwrap();
    // 目標: 初期の音色のアンプのサスティンを 0 にしたプラック
    let target = {
        let mut r = glaux_engine::plugins::PluginRenderer::new(&plugin.id, 44_100.0).unwrap();
        let params = r.params();
        let sus = params
            .iter()
            .find(|(p, _)| p.name.to_lowercase().contains("amp eg sustain"))
            .unwrap()
            .0
            .clone();
        r.render(
            &[(sus.id, sus.min)],
            glaux_engine::plugins::PresetRenderSpec {
                pitch: 60,
                velocity: 0.8,
                hold: 0.8,
                total: 1.2,
                sample_rate: 44_100.0,
            },
        )
        .unwrap()
    };
    let fx = setup().await;
    let wav = fx.dir.join("pluck.wav");
    write_mono_wav(&wav, &target, 44_100);
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_ref001", "Synth")).await);
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "CLAP 音源", "commands": [{ "op": "set_device", "track": "trk_ref001",
                    "device": { "type": "clap", "plugin_id": plugin.id } }] }),
        )
        .await,
    );
    let r = call(
        &fx,
        "refine_plugin_params",
        json!({ "file": wav.to_string_lossy(), "track_id": "trk_ref001",
                "params": ["amp eg sustain", "amp eg decay", "amp eg release"], "max_seconds": 8 }),
    )
    .await;
    let v = ok_json(&r);
    eprintln!("{}", v["refine"]);
    let rf = &v["refine"];
    assert!(rf["distance"].as_f64().unwrap() < rf["initial_distance"].as_f64().unwrap() * 0.5);
    let changed = rf["changed"].as_array().unwrap();
    assert!(changed.iter().any(|c| c["name"]
        .as_str()
        .unwrap()
        .to_lowercase()
        .contains("sustain")));
    let (project, _) = fx.handle.get_project().await.unwrap();
    let dev = project.tracks[0].device.as_ref().unwrap();
    assert!(dev.params.keys().any(|k| k.starts_with("clap:")));
    ok_json(&call(&fx, "undo", json!({})).await);
    let (project, _) = fx.handle.get_project().await.unwrap();
    assert!(!project.tracks[0]
        .device
        .as_ref()
        .unwrap()
        .params
        .keys()
        .any(|k| k.starts_with("clap:")));
}

#[tokio::test]
async fn midi_file_round_trips_through_the_tools() {
    let fx = setup().await;
    // 別の曲(ドラム 1 本・ピアノ 1 本、テンポ 90)を .mid にしておく
    let mut src = glaux_core::Project::new("src");
    src.apply(&glaux_core::Command::SetTempo {
        events: vec![glaux_core::TempoEvent {
            tick: glaux_core::Tick(0),
            bpm: 90.0,
        }],
    })
    .unwrap();
    for (name, dev, pitch) in [("Drums", "drum", 36u8), ("Keys", "fm", 60u8)] {
        let mut t = glaux_core::Track::new(
            glaux_core::TrackId::new(),
            name,
            glaux_core::TrackKind::Midi,
        );
        t.device = Some(glaux_core::Device::builtin(dev));
        let mut c = glaux_core::Clip::new_midi(
            glaux_core::ClipId::new(),
            "c",
            glaux_core::Tick(0),
            glaux_core::Tick(3840),
        );
        c.notes_mut().unwrap().push(glaux_core::Note {
            id: glaux_core::NoteId::new(),
            pos: glaux_core::Tick(0),
            dur: glaux_core::Tick(480),
            pitch,
            vel: 100,
            articulation: Default::default(),
            pitch_curve: vec![],
            glide_ms: None,
            vibrato: None,
            volume_curve: vec![],
            brightness_curve: vec![],
            condition: None,
        });
        t.clips.push(c);
        src.tracks.push(t);
    }
    let mid = fx.dir.join("src.mid");
    std::fs::write(&mid, glaux_mcp::midi::export_smf(&src).unwrap()).unwrap();

    let r = ok_json(&call(&fx, "import_midi", json!({ "path": mid.to_string_lossy() })).await);
    assert_eq!(r["tempo_set"], json!(true), "{r}");
    assert_eq!(r["tracks"].as_array().unwrap().len(), 2);
    let (project, _) = fx.handle.get_project().await.unwrap();
    assert_eq!(project.tempo_map.bpm_at(glaux_core::Tick(0)), 90.0);
    assert_eq!(project.tracks.len(), 2);
    // 1 回の undo で全部戻る
    ok_json(&call(&fx, "undo", json!({})).await);
    let (project, _) = fx.handle.get_project().await.unwrap();
    assert!(project.tracks.is_empty());
    ok_json(&call(&fx, "redo", json!({})).await);

    let r = ok_json(&call(&fx, "export_midi", json!({})).await);
    let path = r["path"].as_str().unwrap();
    assert!(path.ends_with(".mid") && path.contains("export"), "{path}");
    assert_eq!(r["tracks"], json!(2));
    let back = glaux_mcp::midi::parse(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(back.parts.len(), 2);
    assert!(back.parts[0].is_drum());
}

#[tokio::test]
async fn fx_links_branch_and_report_which_effects_sound() {
    let fx = setup().await;
    let r = call(
        &fx,
        "apply_commands",
        json!({
            "commands": [
                { "op": "add_track", "track": { "id": "trk_lead01", "name": "Lead", "kind": "midi" } },
                { "op": "add_effect", "track": "trk_lead01",
                  "effect": { "id": "fx_eq0001", "type": "builtin", "name": "eq" } },
                { "op": "add_effect", "track": "trk_lead01",
                  "effect": { "id": "fx_rev001", "type": "builtin", "name": "reverb" } },
                { "op": "add_effect", "track": "trk_lead01",
                  "effect": { "id": "fx_tape01", "type": "builtin", "name": "tape" } },
                // 原音(eq)とリバーブを並列に。tape はどこにもつながない
                { "op": "set_fx_links", "track": "trk_lead01", "links": [
                    { "from": "in", "to": "fx_eq0001" },
                    { "from": "fx_eq0001", "to": "out" },
                    { "from": "fx_eq0001", "to": "fx_rev001", "gain_db": -8.0 },
                    { "from": "fx_rev001", "to": "out" }
                ] }
            ],
            "label": "並列のリバーブ",
        }),
    )
    .await;
    assert_ne!(r.is_error, Some(true), "{:?}", r.content);

    let r = call(&fx, "list_params", json!({ "track_id": "trk_lead01" })).await;
    let v = ok_json(&r);
    let sounding: Vec<(String, bool)> = v["effects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["id"].as_str().unwrap().to_owned(),
                e["sounding"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        sounding,
        vec![
            ("fx_eq0001".to_owned(), true),
            ("fx_rev001".to_owned(), true),
            ("fx_tape01".to_owned(), false)
        ]
    );
    assert_eq!(v["fx_links"].as_array().unwrap().len(), 4);

    // 表があるトラックに足すと出口の直前に入って鳴る。消すと前後がつながり直す
    let r = call(
        &fx,
        "apply_commands",
        json!({ "commands": [
            { "op": "add_effect", "track": "trk_lead01",
              "effect": { "id": "fx_comp01", "type": "builtin", "name": "compressor" } },
            { "op": "remove_effect", "id": "fx_rev001" }
        ], "label": "足して消す" }),
    )
    .await;
    assert_ne!(r.is_error, Some(true), "{:?}", r.content);
    let r = call(&fx, "get_project", json!({ "track_ids": ["trk_lead01"] })).await;
    let links = ok_json(&r)["project"]["tracks"][0]["fx_links"].clone();
    let has = |from: &str, to: &str| {
        links
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["from"] == from && l["to"] == to)
    };
    assert!(has("fx_comp01", "out"));
    assert!(has("fx_eq0001", "fx_comp01"));
    assert!(!has("fx_rev001", "out"));

    // 輪になるつなぎ方と、表があるトラックでの parked は断る
    let r = call(
        &fx,
        "apply_commands",
        json!({ "commands": [ { "op": "set_fx_links", "track": "trk_lead01", "links": [
            { "from": "fx_eq0001", "to": "fx_comp01" }, { "from": "fx_comp01", "to": "fx_eq0001" }
        ] } ], "label": "輪" }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
    let r = call(
        &fx,
        "apply_commands",
        json!({ "commands": [ { "op": "set_effect_prop", "id": "fx_eq0001", "prop": "parked", "value": true } ],
                "label": "外す" }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));

    // 1 回の undo で表ごと戻る
    call(&fx, "undo", json!({})).await;
    let r = call(&fx, "get_project", json!({ "track_ids": ["trk_lead01"] })).await;
    let t = &ok_json(&r)["project"]["tracks"][0];
    assert_eq!(t["fx_links"].as_array().unwrap().len(), 4);
    assert_eq!(t["effects"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn shape_automation_writes_builds_pumps_and_keeps_other_points() {
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_shp001", "Lead")).await);
    // 2 小節目から 8 小節、カットオフを指数で開く(範囲外の値は範囲に収める)
    let v = ok_json(
        &call(
            &fx,
            "shape_automation",
            json!({ "track_id": "trk_shp001", "target": "device/cutoff", "start": "2", "bars": 8,
                    "shape": "exp", "from": 300, "to": 99999 }),
        )
        .await,
    );
    assert!(v["note"].as_str().unwrap_or("").contains("範囲"), "{v}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    let lane = &project.tracks[0].automation[0];
    assert_eq!(lane.target.to_string(), "device/cutoff");
    assert_eq!(lane.points.first().unwrap().tick.0, 3840);
    assert_eq!(lane.points.last().unwrap().tick.0, 3840 * 9);
    assert!(lane.points.windows(2).all(|w| w[1].value >= w[0].value));
    assert!(lane.points.len() > 10);
    // 音量を 4 分ごとにポンピング(2 小節)。カットオフのレーンは残る
    ok_json(
        &call(
            &fx,
            "shape_automation",
            json!({ "track_id": "trk_shp001", "target": "track/volume_db", "start": "3:1", "end": "5",
                    "shape": "pump", "from": 0, "to": -8 }),
        )
        .await,
    );
    let (project, _) = fx.handle.get_project().await.unwrap();
    assert_eq!(project.tracks[0].automation.len(), 2);
    let vol = project.tracks[0]
        .automation
        .iter()
        .find(|l| l.target.to_string() == "track/volume_db")
        .unwrap();
    assert_eq!(vol.points.len(), 16, "8 拍 × 2 点");
    assert_eq!((vol.points[0].tick.0, vol.points[0].value), (7680, -8.0));
    // 区間の外の点は残して、区間だけ差し替える
    ok_json(
        &call(
            &fx,
            "shape_automation",
            json!({ "track_id": "trk_shp001", "target": "track/volume_db", "start": "4", "bars": 1,
                    "shape": "linear", "from": -3, "to": -3 }),
        )
        .await,
    );
    let (project, _) = fx.handle.get_project().await.unwrap();
    let vol = project.tracks[0]
        .automation
        .iter()
        .find(|l| l.target.to_string() == "track/volume_db")
        .unwrap();
    assert!(vol
        .points
        .iter()
        .any(|p| p.tick.0 == 7680 && p.value == -8.0));
    assert!(vol
        .points
        .iter()
        .any(|p| p.tick.0 == 11520 && p.value == -3.0));
    // 1 回の undo で戻る。マスター(track_id 省略)の音量も書ける
    ok_json(&call(&fx, "undo", json!({})).await);
    ok_json(
        &call(
            &fx,
            "shape_automation",
            json!({ "target": "track/volume_db", "start": "10", "bars": 4, "shape": "log", "from": 0, "to": -60 }),
        )
        .await,
    );
    let (project, _) = fx.handle.get_project().await.unwrap();
    assert_eq!(
        project.master.automation[0].points.last().unwrap().value,
        -60.0
    );
    // 形の名前が違えばエラー
    let r = call(
        &fx,
        "shape_automation",
        json!({ "track_id": "trk_shp001", "target": "track/pan", "start": "1", "bars": 1, "shape": "zigzag", "from": 0, "to": 1 }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
}

#[tokio::test]
async fn apply_groove_and_ghost_notes_make_a_grid_beat_breathe() {
    let fx = setup().await;
    // ドラム(内蔵 drum)の格子どおりのビート 2 小節: キック 1・3、スネア 2・4、ハット 16 分(全部 100)
    let mut notes = Vec::new();
    for bar in 0..2u64 {
        let b = bar * 3840;
        notes.push(json!({ "pos": b, "dur": 120, "pitch": 36, "vel": 100 }));
        notes.push(json!({ "pos": b + 1920, "dur": 120, "pitch": 36, "vel": 100 }));
        notes.push(json!({ "pos": b + 960, "dur": 120, "pitch": 38, "vel": 100 }));
        notes.push(json!({ "pos": b + 2880, "dur": 120, "pitch": 38, "vel": 100 }));
        for k in 0..16u64 {
            notes.push(json!({ "pos": b + k * 240, "dur": 60, "pitch": 42, "vel": 100 }));
        }
    }
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "ドラム", "commands": [
                { "op": "add_track", "track": { "id": "trk_grv001", "name": "Drums", "kind": "midi",
                  "device": { "type": "builtin", "name": "drum" } } },
                { "op": "add_clip", "track": "trk_grv001", "clip": {
                    "id": "clp_grv001", "name": "beat", "start": 0, "length": 7680, "kind": "midi", "notes": notes } }
            ] }),
        )
        .await,
    );
    let v = ok_json(
        &call(
            &fx,
            "apply_groove",
            json!({ "clip_id": "clp_grv001", "style": "funk", "humanize_ms": 4, "pocket_ms": { "snare": 6 } }),
        )
        .await,
    );
    assert!(v["changed"].as_u64().unwrap() > 20, "{v}");
    assert_eq!(v["style"]["from_dataset"], true);
    let (project, _) = fx.handle.get_project().await.unwrap();
    let ns = project.tracks[0].clips[0].notes().unwrap();
    // 格子ちょうどの割合が下がり、ハットの強弱に幅が出て、スネアは後ろへ
    let on_grid = ns.iter().filter(|n| n.pos.0 % 240 == 0).count() as f64 / ns.len() as f64;
    assert!(on_grid < 0.5, "{on_grid}");
    let hats: Vec<u8> = ns.iter().filter(|n| n.pitch == 42).map(|n| n.vel).collect();
    assert!(hats.iter().max().unwrap() - hats.iter().min().unwrap() > 30);
    let snare = ns
        .iter()
        .filter(|n| n.pitch == 38)
        .map(|n| n.pos.0)
        .min()
        .unwrap();
    assert!(snare > 960 && snare < 1000, "{snare}");
    // 同じ設定をもう一度掛けると同じ結果(undo してやり直す)
    ok_json(&call(&fx, "undo", json!({})).await);
    ok_json(
        &call(
            &fx,
            "apply_groove",
            json!({ "clip_id": "clp_grv001", "style": "funk", "humanize_ms": 4, "pocket_ms": { "snare": 6 } }),
        )
        .await,
    );
    let (again, _) = fx.handle.get_project().await.unwrap();
    assert_eq!(again.tracks[0].clips[0].notes().unwrap(), ns);
    // ゴーストノート: バックビートを避けて弱く足す
    let v = ok_json(
        &call(
            &fx,
            "add_ghost_notes",
            json!({ "clip_id": "clp_grv001", "style": "funk", "density": 1.0, "seed": 3 }),
        )
        .await,
    );
    let added = v["added"].as_u64().unwrap();
    assert!(added >= 2, "{v}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    let ghosts: Vec<_> = project.tracks[0].clips[0]
        .notes()
        .unwrap()
        .iter()
        .filter(|n| n.pitch == 38 && n.vel <= 60)
        .cloned()
        .collect();
    assert_eq!(ghosts.len() as u64, added);
    assert!(ghosts
        .iter()
        .all(|n| n.pos.0 % 3840 != 960 && n.pos.0 % 3840 != 2880));
    // 不明な型はエラー(使える型を知らせる)
    let r = call(
        &fx,
        "apply_groove",
        json!({ "clip_id": "clp_grv001", "style": "polka" }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
}

#[tokio::test]
async fn apply_groove_takes_many_clips_keeps_the_kick_and_unrolls_loops() {
    let fx = setup().await;
    // ハウス: 1 小節のループ(4 つ打ち + 16 分のハット)を 4 小節ぶん、コードの刻みのクリップ
    let mut beat = Vec::new();
    for k in 0..4u64 {
        beat.push(json!({ "pos": k * 960, "dur": 120, "pitch": 36, "vel": 110 }));
    }
    for k in 0..16u64 {
        beat.push(json!({ "pos": k * 240, "dur": 60, "pitch": 42, "vel": 100 }));
    }
    let stab: Vec<_> = (0..8u64)
        .map(|k| json!({ "pos": k * 960 + 480, "dur": 240, "pitch": 60, "vel": 100 }))
        .collect();
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "ハウス", "commands": [
                { "op": "add_track", "track": { "id": "trk_hse001", "name": "Drums", "kind": "midi",
                  "device": { "type": "builtin", "name": "drum" } } },
                { "op": "add_clip", "track": "trk_hse001", "clip": {
                    "id": "clp_hse001", "name": "loop", "start": 0, "length": 15360, "kind": "midi",
                    "loop": true, "loop_len": 3840, "notes": beat } },
                { "op": "add_track", "track": { "id": "trk_hse002", "name": "Stab", "kind": "midi" } },
                { "op": "add_clip", "track": "trk_hse002", "clip": {
                    "id": "clp_hse002", "name": "stab", "start": 0, "length": 7680, "kind": "midi", "notes": stab } }
            ] }),
        )
        .await,
    );
    let args =
        json!({ "clip_ids": ["clp_hse001", "clp_hse002"], "style": "house", "humanize_ms": 5 });
    let v = ok_json(&call(&fx, "apply_groove", args.clone()).await);
    assert_eq!(v["clips"].as_array().unwrap().len(), 2, "{v}");
    assert_eq!(v["style"]["locked"], json!(["kick"]));
    // ループの中身に当てたこと(揺れが毎回同じ)を知らせる
    assert!(v["note"].as_str().unwrap().contains("unroll_loop"), "{v}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    let drums = &project.tracks[0].clips[0];
    assert!(drums.loop_len().is_some());
    // 4 つ打ちのキックは動かない
    let kicks: Vec<(u64, u8)> = drums
        .notes()
        .unwrap()
        .iter()
        .filter(|n| n.pitch == 36)
        .map(|n| (n.pos.0, n.vel))
        .collect();
    assert_eq!(kicks, vec![(0, 110), (960, 110), (1920, 110), (2880, 110)]);
    // 刻みも動いた
    assert!(project.tracks[1].clips[0]
        .notes()
        .unwrap()
        .iter()
        .any(|n| n.pos.0 % 480 != 0));
    // 1 回の undo で両方戻る
    ok_json(&call(&fx, "undo", json!({})).await);
    let (back, _) = fx.handle.get_project().await.unwrap();
    assert!(back.tracks[1].clips[0]
        .notes()
        .unwrap()
        .iter()
        .all(|n| n.pos.0 % 480 == 0));
    // ループをほどいてから: 繰り返しごとに違う揺れ、キックは 16 発とも格子どおり
    let mut args2 = args.clone();
    args2["unroll_loop"] = json!(true);
    let v = ok_json(&call(&fx, "apply_groove", args2).await);
    assert!(v.get("note").is_none(), "{v}");
    assert_eq!(v["clips"][0]["unrolled"], true);
    let (project, _) = fx.handle.get_project().await.unwrap();
    let drums = &project.tracks[0].clips[0];
    assert!(drums.loop_len().is_none());
    let ns = drums.notes().unwrap();
    let kicks: Vec<u64> = ns
        .iter()
        .filter(|n| n.pitch == 36)
        .map(|n| n.pos.0)
        .collect();
    assert_eq!(kicks, (0..16u64).map(|k| k * 960).collect::<Vec<_>>());
    let hat_offsets = |bar: u64| -> Vec<i64> {
        ns.iter()
            .filter(|n| n.pitch == 42 && n.pos.0 / 3840 == bar)
            .map(|n| n.pos.0 as i64 - ((n.pos.0 + 120) / 240 * 240) as i64)
            .collect()
    };
    assert_eq!(hat_offsets(0).len(), 16);
    assert_ne!(hat_offsets(0), hat_offsets(1));
    // 1 回の undo でループに戻る
    ok_json(&call(&fx, "undo", json!({})).await);
    let (back, _) = fx.handle.get_project().await.unwrap();
    assert_eq!(back.tracks[0].clips[0].notes().unwrap().len(), 20);
    assert!(back.tracks[0].clips[0].loop_len().is_some());
    // note_ids は 1 つのクリップのときだけ
    let r = call(
        &fx,
        "apply_groove",
        json!({ "clip_ids": ["clp_hse001", "clp_hse002"], "style": "house", "note_ids": ["nt_aaaaaa"] }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
    // キックだけのループは変わる音が無いので、ほどかない(ほどくだけの編集を残さない)
    let kicks: Vec<_> = (0..4u64)
        .map(|k| json!({ "pos": k * 960, "dur": 120, "pitch": 36, "vel": 110 }))
        .collect();
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "キック", "commands": [
                { "op": "add_clip", "track": "trk_hse001", "clip": {
                    "id": "clp_hse003", "name": "kick", "start": 15360, "length": 15360, "kind": "midi",
                    "loop": true, "loop_len": 3840, "notes": kicks } }
            ] }),
        )
        .await,
    );
    let v = ok_json(
        &call(
            &fx,
            "apply_groove",
            json!({ "clip_id": "clp_hse003", "style": "house", "humanize_ms": 5, "unroll_loop": true }),
        )
        .await,
    );
    assert_eq!(v["changed"], 0, "{v}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    let (_, k) = project
        .clip(&glaux_core::ClipId::parse("clp_hse003").unwrap())
        .unwrap();
    assert!(k.loop_len().is_some());
}

#[tokio::test]
async fn write_chords_voices_a_progression_with_a_rhythm() {
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_chd001", "Keys")).await);
    // 王道進行(C)をドロップ 2・裏拍の刻みで、2 回
    let v = ok_json(
        &call(
            &fx,
            "write_chords",
            json!({ "track_id": "trk_chd001", "chords": "IVmaj7 | V7 | iii7 | vi", "key": "C major",
                    "style": "drop2", "rhythm": "offbeat", "repeat": 2, "bar": 3 }),
        )
        .await,
    );
    assert_eq!(v["bars"], 8, "{v}");
    let names: Vec<&str> = v["chords"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["chord"].as_str().unwrap())
        .take(4)
        .collect();
    assert_eq!(names, vec!["Fmaj7", "G7", "Em7", "Am"]);
    assert_eq!(v["chords"][0]["bar"], 3);
    let (project, _) = fx.handle.get_project().await.unwrap();
    let clip = &project.tracks[0].clips[0];
    // 3 小節目から 8 小節
    assert_eq!((clip.start.0, clip.length.0), (3840 * 2, 3840 * 8));
    let notes = clip.notes().unwrap();
    // 裏拍に 4 回 × 4 声 × 8 小節
    assert_eq!(notes.len(), 4 * 4 * 8, "{}", notes.len());
    // 各小節の音は和音の構成音だけ(Fmaj7 = F A C E)
    let pcs = |bar: u64| -> Vec<u8> {
        let mut v: Vec<u8> = notes
            .iter()
            .filter(|n| n.pos.0 / 3840 == bar)
            .map(|n| n.pitch % 12)
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    assert_eq!(pcs(0), vec![0, 4, 5, 9]);
    assert_eq!(pcs(1), vec![2, 5, 7, 11]);
    assert!(notes.iter().all(|n| n.pos.0 % 960 == 480));
    assert!(v["mean_motion"].as_f64().unwrap() < 8.0, "{v}");
    // 伸ばし(既定): 同じ和音が続く小節は 1 つにまとめる。低音を足す
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_chd002", "Pad")).await);
    let v = ok_json(
        &call(
            &fx,
            "write_chords",
            json!({ "track_id": "trk_chd002", "chords": "Am7 | % | Dm9 G13 | Cmaj9", "style": "spread", "bass": true }),
        )
        .await,
    );
    assert_eq!(v["chords"].as_array().unwrap().len(), 4, "{v}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    let pad = project.tracks[1].clips[0].notes().unwrap();
    let first: Vec<_> = pad.iter().filter(|n| n.pos.0 == 0).collect();
    assert_eq!(first.len(), 5);
    assert!(first.iter().all(|n| n.dur.0 == 3840 * 2));
    // 低音は A(根音)
    assert_eq!(first.iter().map(|n| n.pitch).min().unwrap() % 12, 9);
    // 1 回の undo で消える
    ok_json(&call(&fx, "undo", json!({})).await);
    let (project, _) = fx.handle.get_project().await.unwrap();
    assert!(project.tracks[1].clips.is_empty());
    // 読めないコードはまとめて知らせる
    let r = call(
        &fx,
        "write_chords",
        json!({ "track_id": "trk_chd002", "chords": "Am7 | Hx | Cq" }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
    let text = format!("{r:?}");
    assert!(text.contains("Hx") && text.contains("Cq"), "{text}");
}

#[tokio::test]
async fn write_bassline_follows_the_chords_the_kick_and_slides() {
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_bas001", "Bass")).await);
    // 根音の 8 分(既定): 伴奏と同じ進行の根音
    let v = ok_json(
        &call(
            &fx,
            "write_bassline",
            json!({ "track_id": "trk_bas001", "chords": "Am7 | Fmaj7 | C G/B | %", "approach": "chromatic" }),
        )
        .await,
    );
    assert_eq!(v["bars"], 4, "{v}");
    let roots: Vec<&str> = v["chords"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["root"].as_str().unwrap())
        .collect();
    assert!(
        roots[0].starts_with('A') && roots[1].starts_with('F'),
        "{roots:?}"
    );
    // 分数コード G/B は B
    assert!(roots[3].starts_with('B'), "{roots:?}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    let ns = project.tracks[0].clips[0].notes().unwrap();
    assert!(ns.iter().all(|n| (28..=52).contains(&n.pitch)));
    // キックに合わせる
    let kicks: Vec<Value> = [0u64, 960, 1680, 2880]
        .iter()
        .map(|p| json!({ "pos": p, "dur": 120, "pitch": 36, "vel": 110 }))
        .collect();
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "キック", "commands": [
                { "op": "add_track", "track": { "id": "trk_bas002", "name": "Kick", "kind": "midi",
                  "device": { "type": "builtin", "name": "drum" } } },
                { "op": "add_clip", "track": "trk_bas002", "clip": {
                    "id": "clp_bas002", "name": "k", "start": 0, "length": 7680, "kind": "midi",
                    "loop": true, "loop_len": 3840, "notes": kicks } },
                { "op": "add_track", "track": { "id": "trk_bas003", "name": "Bass2", "kind": "midi" } }
            ] }),
        )
        .await,
    );
    ok_json(
        &call(
            &fx,
            "write_bassline",
            json!({ "track_id": "trk_bas003", "chords": "Am | F", "follow_kick": "clp_bas002" }),
        )
        .await,
    );
    let (project, _) = fx.handle.get_project().await.unwrap();
    let b2 = project.tracks[2].clips[0].notes().unwrap();
    let pos: Vec<u64> = b2.iter().map(|n| n.pos.0).collect();
    assert_eq!(pos, vec![0, 960, 1680, 2880, 3840, 4800, 5520, 6720]);
    // 808: 音が変わる所は滑らせる(ポルタメント + glide_ms)
    ok_json(&call(&fx, "undo", json!({})).await);
    let v = ok_json(
        &call(
            &fx,
            "write_bassline",
            json!({ "track_id": "trk_bas003", "chords": "Am | F | C | G", "pattern": "808", "range": "C1-C3", "glide_ms": 80 }),
        )
        .await,
    );
    let (project, _) = fx.handle.get_project().await.unwrap();
    let b3 = project.tracks[2].clips[0].notes().unwrap();
    assert!(
        b3.iter()
            .any(|n| n.articulation == glaux_core::Articulation::Portamento
                && n.glide_ms == Some(80.0)),
        "{v}"
    );
    // 読めない型はエラー
    let r = call(
        &fx,
        "write_bassline",
        json!({ "track_id": "trk_bas003", "chords": "Am", "pattern": "xyz" }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
}

#[tokio::test]
async fn odd_meter_tools_follow_the_beat_groups() {
    let fx = setup().await;
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "7/8", "commands": [
                { "op": "set_time_sig", "events": [ { "tick": 0, "num": 7, "den": 8, "grouping": [2, 2, 3] } ] },
                { "op": "add_track", "track": { "id": "trk_odd001", "name": "Drums", "kind": "midi",
                  "device": { "type": "builtin", "name": "drum" } } },
                { "op": "add_track", "track": { "id": "trk_odd002", "name": "Keys", "kind": "midi" } },
                { "op": "add_track", "track": { "id": "trk_odd003", "name": "Bass", "kind": "midi" } },
                { "op": "add_track", "track": { "id": "trk_odd004", "name": "Lead", "kind": "midi" } }
            ] }),
        )
        .await,
    );
    // まとまりの和が合わない拍子はエラー
    let r = call(
        &fx,
        "apply_commands",
        json!({ "label": "x", "commands": [
            { "op": "set_time_sig", "events": [ { "tick": 0, "num": 7, "den": 8, "grouping": [3, 3] } ] }
        ] }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
    // get_project は拍子の説明を添える
    let v = ok_json(&call(&fx, "get_project", json!({ "include_notes": false })).await);
    assert_eq!(
        v["project"]["time_sig_map"][0]["meter"], "7/8 (2+2+3)",
        "{v}"
    );
    assert_eq!(v["project"]["time_sig_map"][0]["bar_ticks"], 3360);
    assert_eq!(v["project"]["time_sig_map"][0]["steps_16th"], 14);
    const BAR: u64 = 3360;
    ok_json(
        &call(
            &fx,
            "write_drums",
            json!({ "track_id": "trk_odd001", "style": "rock", "bars": 4, "fill": "none",
                    "crash": false, "variation": 0.0 }),
        )
        .await,
    );
    ok_json(
        &call(
            &fx,
            "write_chords",
            json!({ "track_id": "trk_odd002", "chords": "Am G | F | Dm | E", "rhythm": "eighth" }),
        )
        .await,
    );
    ok_json(
        &call(
            &fx,
            "write_bassline",
            json!({ "track_id": "trk_odd003", "chords": "Am G | F | Dm | E", "pattern": "root8" }),
        )
        .await,
    );
    let mel = ok_json(
        &call(
            &fx,
            "write_melody",
            json!({ "track_id": "trk_odd004", "chords": "Am G | F | Dm | E", "key": "A minor",
                    "bars": 4, "role": "verse" }),
        )
        .await,
    );
    let (project, _) = fx.handle.get_project().await.unwrap();
    let notes_of = |i: usize| -> Vec<(u64, u8)> {
        let c = &project.tracks[i].clips[0];
        c.notes()
            .unwrap()
            .iter()
            .map(|n| (c.start.0 + n.pos.0, n.pitch))
            .collect()
    };
    // すべて 16 分の格子の上(1 小節を 16 等分した 210 tick の端数が無い)
    for i in 0..4 {
        for (t, _) in notes_of(i) {
            assert_eq!(t % 240, 0, "track {i}: {t}");
        }
    }
    // キックは 1・3 番目のまとまりの頭、スネアは 2 番目の頭
    let drums = notes_of(0);
    let kicks: Vec<u64> = drums
        .iter()
        .filter(|n| n.1 == 36 && n.0 < BAR)
        .map(|n| n.0)
        .collect();
    let snares: Vec<u64> = drums
        .iter()
        .filter(|n| n.1 == 38 && n.0 < BAR)
        .map(|n| n.0)
        .collect();
    assert_eq!(kicks, vec![0, 1920]);
    assert_eq!(snares, vec![960]);
    // 1 小節目の 2 つ目の和音(G)はまとまりの頭(1920)から
    let keys = notes_of(1);
    assert!(
        keys.iter().any(|n| n.0 == 1920 && n.1 % 12 == 7),
        "{keys:?}"
    );
    assert!(!keys.iter().any(|n| n.0 == 1680));
    assert!(mel["score"].as_u64().is_some(), "{mel}");
}

#[tokio::test]
async fn meter_tools_change_bars_layer_cycles_and_bend_long_beats() {
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_mtr001", "Keys")).await);
    // 4 小節の 4 分の刻み
    let notes: Vec<Value> = (0..16u64)
        .map(|k| json!({ "pos": k * 960, "dur": 480, "pitch": 60, "vel": 90 }))
        .collect();
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "音", "commands": [
                { "op": "add_clip", "track": "trk_mtr001", "clip": {
                    "id": "clp_mtr001", "name": "k", "start": 0, "length": 15360, "kind": "midi", "notes": notes } }
            ] }),
        )
        .await,
    );
    // 2 小節目を 2/4 に(2 拍抜く)
    let v = ok_json(&call(&fx, "change_meter", json!({ "bar": 2, "beats": -2 })).await);
    assert_eq!(v["meter"], "2/4", "{v}");
    assert_eq!(v["removed_notes"], 2);
    let (project, _) = fx.handle.get_project().await.unwrap();
    let sigs: Vec<(u64, u8)> = project
        .time_sig_map
        .iter()
        .map(|e| (e.tick.0, e.num))
        .collect();
    assert_eq!(sigs, vec![(0, 4), (3840, 2), (5760, 4)]);
    ok_json(&call(&fx, "undo", json!({})).await);
    // 1 小節目を 7/8 3+2+2 に
    let v = ok_json(&call(&fx, "change_meter", json!({ "bar": 1, "to": "7/8 3+2+2" })).await);
    assert_eq!(v["meter"], "7/8 (3+2+2)", "{v}");
    assert_eq!(v["bar_ticks"], 3360);
    let r = call(&fx, "change_meter", json!({ "bar": 1 })).await;
    assert_eq!(r.is_error, Some(true));
    let r = call(&fx, "change_meter", json!({ "bar": 1, "to": "7/8 3+3" })).await;
    assert_eq!(r.is_error, Some(true));
    ok_json(&call(&fx, "undo", json!({})).await);

    // ポリリズム 3:2 を 1 小節: 2 拍に 3 つ × 2 組 = 6 音、640 ごと。b の側も置く
    let v = ok_json(
        &call(
            &fx,
            "write_polyrhythm",
            json!({ "track_id": "trk_mtr001", "ratio": "3:2", "bar": 6, "pitch": "C5", "pitch2": "C4" }),
        )
        .await,
    );
    assert_eq!(v["notes"], 6 + 4, "{v}");
    assert_eq!(v["rounding_error_ticks"], 0.0);
    let (project, _) = fx.handle.get_project().await.unwrap();
    let clip = project.tracks[0]
        .clips
        .iter()
        .find(|c| c.id.as_str() == v["clip_id"])
        .unwrap();
    let a: Vec<u64> = clip
        .notes()
        .unwrap()
        .iter()
        .filter(|n| n.pitch == 72)
        .map(|n| n.pos.0)
        .collect();
    assert_eq!(a, vec![0, 640, 1280, 1920, 2560, 3200]);
    let r = call(
        &fx,
        "write_polyrhythm",
        json!({ "track_id": "trk_mtr001", "ratio": "3-2" }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));

    // ポリメーター: 3 ステップの型を 16 分で 4/4 に = 3 小節で元に戻る
    let v = ok_json(
        &call(
            &fx,
            "write_polymeter",
            json!({ "track_id": "trk_mtr001", "pattern": "X..", "bar": 8, "bars": 3 }),
        )
        .await,
    );
    assert_eq!(v["cycle_steps"], 3, "{v}");
    assert_eq!(v["realign_bars"], 3);
    assert_eq!(v["notes"], 16);
    let v = ok_json(
        &call(
            &fx,
            "write_polymeter",
            json!({ "track_id": "trk_mtr001", "pattern": "E(3,8)", "unit": "8th", "bar": 12, "bars": 2,
                    "reset_every_bars": 1 }),
        )
        .await,
    );
    assert_eq!(v["cycle"], "X..x..x.", "{v}");
    assert_eq!(v["notes"], 6);

    // アクサクの揺れ: 7/8 (2+2+3) の小節で長い拍を詰める
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "7/8", "commands": [
                { "op": "set_time_sig", "events": [ { "tick": 0, "num": 4, "den": 4 },
                    { "tick": 61440, "num": 7, "den": 8 } ] },
                { "op": "add_clip", "track": "trk_mtr001", "clip": {
                    "id": "clp_mtr009", "name": "a", "start": 61440, "length": 3360, "kind": "midi", "notes": [
                        { "pos": 0, "dur": 480, "pitch": 60, "vel": 90 },
                        { "pos": 960, "dur": 480, "pitch": 60, "vel": 90 },
                        { "pos": 1920, "dur": 480, "pitch": 60, "vel": 90 },
                        { "pos": 2880, "dur": 480, "pitch": 60, "vel": 90 } ] } }
            ] }),
        )
        .await,
    );
    let v = ok_json(
        &call(
            &fx,
            "set_meter_feel",
            json!({ "clip_id": "clp_mtr009", "long_ratio": 1.4 }),
        )
        .await,
    );
    assert!(v["changed"].as_u64().unwrap() >= 3, "{v}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    let (_, c) = project.clip(&"clp_mtr009".parse().unwrap()).unwrap();
    let pos: Vec<u64> = c.notes().unwrap().iter().map(|n| n.pos.0).collect();
    assert_eq!(pos[0], 0);
    // 短い拍が長くなり、長い拍の中の音は前へ詰まる(小節の長さは同じ)
    assert!(pos[1] > 960 && pos[2] > 1920, "{pos:?}");
    assert!(pos[3] < 3360);
    let r = call(
        &fx,
        "set_meter_feel",
        json!({ "clip_id": "clp_mtr009", "long_ratio": 3.0 }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
}

#[tokio::test]
async fn pitch_gestures_and_vibrato_shape_the_melody() {
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_ges001", "Vocal")).await);
    // 句 1: C4 D4 G4(跳躍)、休み、句 2: E4(2 拍)
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "歌", "commands": [
                { "op": "add_clip", "track": "trk_ges001", "clip": {
                    "id": "clp_ges001", "name": "v", "start": 0, "length": 7680, "kind": "midi", "notes": [
                        { "id": "nt_ges001", "pos": 0, "dur": 480, "pitch": 60, "vel": 90 },
                        { "id": "nt_ges002", "pos": 480, "dur": 480, "pitch": 62, "vel": 90 },
                        { "id": "nt_ges003", "pos": 960, "dur": 960, "pitch": 67, "vel": 90 },
                        { "id": "nt_ges004", "pos": 2880, "dur": 1920, "pitch": 64, "vel": 90 } ] } }
            ] }),
        )
        .await,
    );
    // しゃくり: 跳躍と句の頭(全部に付ける)
    let v = ok_json(
        &call(
            &fx,
            "pitch_gesture",
            json!({ "clip_id": "clp_ges001", "kind": "shakuri", "probability": 1.0 }),
        )
        .await,
    );
    assert_eq!(v["changed"], 3, "{v}");
    // フォール: 句の終わり(G4 と E4)。しゃくりの点は残る
    let v = ok_json(
        &call(
            &fx,
            "pitch_gesture",
            json!({ "clip_id": "clp_ges001", "kind": "fall", "probability": 1.0 }),
        )
        .await,
    );
    assert_eq!(v["changed"], 2, "{v}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    let ns = project.tracks[0].clips[0].notes().unwrap().to_vec();
    let e4 = ns.iter().find(|n| n.pitch == 64).unwrap();
    assert_eq!(e4.pitch_curve[0].cents, -150.0);
    assert_eq!(e4.pitch_curve[0].shape, glaux_core::CurveShape::EaseOut);
    assert_eq!(e4.pitch_curve.last().unwrap().cents, -700.0);
    assert_eq!(e4.pitch_curve.last().unwrap().tick.0, 1920);
    // 120 BPM で 250ms = 480 tick のフォール
    assert!(e4
        .pitch_curve
        .iter()
        .any(|p| p.tick.0 == 1440 && p.cents == 0.0));
    let d4 = ns.iter().find(|n| n.pitch == 62).unwrap();
    assert!(d4.pitch_curve.is_empty(), "跳躍でも句の端でもない");
    // フォールは音量も下げる
    assert_eq!(e4.volume_curve.last().unwrap().value, -18.0);
    assert_eq!(e4.volume_curve[0].tick.0, 1440);
    // ビブラート: 1 拍以上の音(G4・E4)
    let v = ok_json(
        &call(
            &fx,
            "set_vibrato",
            json!({ "clip_id": "clp_ges001", "style": "vocal", "depth_cents": 50, "humanize": 0 }),
        )
        .await,
    );
    assert_eq!(v["changed"], 2, "{v}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    let e4 = project.tracks[0].clips[0]
        .notes()
        .unwrap()
        .iter()
        .find(|n| n.pitch == 64)
        .unwrap()
        .clone();
    let vib = e4.vibrato.unwrap();
    assert_eq!(
        (vib.rate_hz, vib.depth_cents, vib.delay_ms),
        (5.5, 50.0, 250.0)
    );
    // compact 表記に vibrato が載る
    let g = ok_json(
        &call(
            &fx,
            "get_project",
            json!({ "clip_ids": ["clp_ges001"], "note_format": "compact" }),
        )
        .await,
    );
    assert!(g.to_string().contains("depth_cents"), "{g}");
    // 外す → undo で戻る
    let v = ok_json(
        &call(
            &fx,
            "set_vibrato",
            json!({ "clip_id": "clp_ges001", "remove": true }),
        )
        .await,
    );
    assert_eq!(v["changed"], 2);
    ok_json(&call(&fx, "undo", json!({})).await);
    let (project, _) = fx.handle.get_project().await.unwrap();
    assert!(
        project.tracks[0].clips[0]
            .notes()
            .unwrap()
            .iter()
            .filter(|n| n.vibrato.is_some())
            .count()
            == 2
    );
    // 音の中の動き: 長い音(G4・E4)を暗くから開く、音量をふくらませる(E4 のフォールの音量は置き換わる)
    let v = ok_json(
        &call(
            &fx,
            "note_dynamics",
            json!({ "clip_id": "clp_ges001", "kind": "open" }),
        )
        .await,
    );
    assert_eq!(v["changed"], 2, "{v}");
    let v = ok_json(
        &call(
            &fx,
            "note_dynamics",
            json!({ "clip_id": "clp_ges001", "kind": "swell", "amount": 10 }),
        )
        .await,
    );
    assert_eq!(v["changed"], 2, "{v}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    let e4 = project.tracks[0].clips[0]
        .notes()
        .unwrap()
        .iter()
        .find(|n| n.pitch == 64)
        .unwrap()
        .clone();
    assert_eq!(e4.volume_curve[0].value, -10.0);
    assert_eq!(e4.brightness_curve[0].value, -0.7);
    let v = ok_json(
        &call(
            &fx,
            "note_dynamics",
            json!({ "clip_id": "clp_ges001", "kind": "open", "remove": true }),
        )
        .await,
    );
    assert_eq!(v["changed"], 2, "{v}");
    // エラー
    for bad in [
        (
            "note_dynamics",
            json!({ "clip_id": "clp_ges001", "kind": "wah" }),
        ),
        (
            "pitch_gesture",
            json!({ "clip_id": "clp_ges001", "kind": "wobble" }),
        ),
        (
            "pitch_gesture",
            json!({ "clip_id": "clp_ges001", "kind": "fall", "target": "middle" }),
        ),
        (
            "set_vibrato",
            json!({ "clip_id": "clp_ges001", "style": "theremin" }),
        ),
        (
            "set_vibrato",
            json!({ "clip_id": "clp_ges001", "rate_hz": 30 }),
        ),
        ("set_vibrato", json!({})),
    ] {
        let r = call(&fx, bad.0, bad.1.clone()).await;
        assert_eq!(r.is_error, Some(true), "{bad:?}");
    }
}

#[tokio::test]
async fn ornaments_and_melody_lead() {
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_orn001", "Flute")).await);
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "笛", "commands": [
                { "op": "add_clip", "track": "trk_orn001", "clip": {
                    "id": "clp_orn001", "name": "f", "start": 0, "length": 7680, "kind": "midi", "notes": [
                        { "id": "nt_orn001", "pos": 0, "dur": 960, "pitch": 62, "vel": 90 },
                        { "id": "nt_orn002", "pos": 960, "dur": 960, "pitch": 69, "vel": 90 },
                        { "id": "nt_orn003", "pos": 1920, "dur": 1920, "pitch": 65, "vel": 90 } ] } }
            ] }),
        )
        .await,
    );
    // 短前打音を A4 に: 前の D4 が短くなり、装飾の音(D minor の上の隣 = Bb4)が前に入る
    let v = ok_json(
        &call(
            &fx,
            "add_ornament",
            json!({ "clip_id": "clp_orn001", "kind": "acciaccatura", "note_ids": ["nt_orn002"], "key": "D minor" }),
        )
        .await,
    );
    assert_eq!(v["decorated"], 1, "{v}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    let ns = project.tracks[0].clips[0].notes().unwrap().to_vec();
    assert_eq!(ns.len(), 4);
    let grace = ns.iter().find(|n| n.pitch == 70).expect("Bb4");
    assert!(
        grace.pos.0 < 960 && grace.pos.0 + grace.dur.0 <= 960,
        "{ns:?}"
    );
    let d4 = ns.iter().find(|n| n.pitch == 62).unwrap();
    assert_eq!(d4.pos.0 + d4.dur.0, grace.pos.0);
    ok_json(&call(&fx, "undo", json!({})).await);
    // トリル: 長い F4 を音の長さいっぱいに(合計の長さは同じ)
    let v = ok_json(
        &call(
            &fx,
            "add_ornament",
            json!({ "clip_id": "clp_orn001", "kind": "trill", "note_ids": ["nt_orn003"], "key": "D minor", "trill_end": "turn" }),
        )
        .await,
    );
    assert_eq!(v["decorated"], 1, "{v}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    let ns = project.tracks[0].clips[0].notes().unwrap().to_vec();
    let trill: Vec<_> = ns.iter().filter(|n| n.pos.0 >= 1920).collect();
    assert!(trill.len() >= 10, "{}", trill.len());
    let end = trill.iter().map(|n| n.pos.0 + n.dur.0).max().unwrap();
    assert_eq!(end, 3840);
    assert_eq!(
        trill.last().unwrap().pitch,
        65,
        "ターンで本音に戻って終わる"
    );
    ok_json(&call(&fx, "undo", json!({})).await);
    // 規則で選ぶ(1 拍以上の音にモルデント)
    let v = ok_json(
        &call(
            &fx,
            "add_ornament",
            json!({ "clip_id": "clp_orn001", "kind": "mordent", "probability": 1.0 }),
        )
        .await,
    );
    assert_eq!(v["decorated"], 3, "{v}");
    ok_json(&call(&fx, "undo", json!({})).await);
    // メロディーのリード: 単音なので頭以外が 20ms(120BPM で 38 tick)前へ、終わりは同じ
    let v = ok_json(&call(&fx, "melody_lead", json!({ "clip_id": "clp_orn001" })).await);
    assert_eq!(v["changed"], 2, "{v}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    let a4 = project.tracks[0].clips[0]
        .notes()
        .unwrap()
        .iter()
        .find(|n| n.pitch == 69)
        .unwrap()
        .clone();
    assert_eq!((a4.pos.0, a4.pos.0 + a4.dur.0), (960 - 38, 1920));
    for bad in [
        (
            "add_ornament",
            json!({ "clip_id": "clp_orn001", "kind": "glissando" }),
        ),
        (
            "melody_lead",
            json!({ "clip_id": "clp_orn001", "lead_ms": 200 }),
        ),
        (
            "melody_lead",
            json!({ "clip_id": "clp_orn001", "mode": "x" }),
        ),
    ] {
        let r = call(&fx, bad.0, bad.1.clone()).await;
        assert_eq!(r.is_error, Some(true), "{bad:?}");
    }
}

#[tokio::test]
async fn strums_and_drum_rudiments() {
    let fx = setup().await;
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "トラック", "commands": [
                { "op": "add_track", "track": { "id": "trk_tec001", "name": "Guitar", "kind": "midi" } },
                { "op": "add_track", "track": { "id": "trk_tec002", "name": "Drums", "kind": "midi",
                  "device": { "type": "builtin", "name": "drum" } } }
            ] }),
        )
        .await,
    );
    // 拍の頭と裏に 6 弦の和音
    let mut notes = Vec::new();
    for pos in [0u64, 480] {
        for p in [40u8, 45, 50, 55, 59, 64] {
            notes.push(json!({ "pos": pos, "dur": 400, "pitch": p, "vel": 100 }));
        }
    }
    let snares: Vec<Value> = [960u64, 2880]
        .iter()
        .map(|&pos| json!({ "pos": pos, "dur": 120, "pitch": 38, "vel": 110 }))
        .collect();
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "音", "commands": [
                { "op": "add_clip", "track": "trk_tec001", "clip": {
                    "id": "clp_tec001", "name": "g", "start": 3840, "length": 3840, "kind": "midi", "notes": notes } },
                { "op": "add_clip", "track": "trk_tec002", "clip": {
                    "id": "clp_tec002", "name": "d", "start": 0, "length": 7680, "kind": "midi", "notes": snares } }
            ] }),
        )
        .await,
    );
    // ギターのストローク: 頭は下げ(低い弦から)、裏は上げ(高い弦から・一番低い弦を省く)
    let v = ok_json(
        &call(
            &fx,
            "strum_chord",
            json!({ "clip_id": "clp_tec001", "jitter_ms": 0 }),
        )
        .await,
    );
    assert_eq!(v["chords"], 2, "{v}");
    // 上げは上の 4 本だけ(低い 2 本を省く)
    assert_eq!(v["removed"], 2);
    let (project, _) = fx.handle.get_project().await.unwrap();
    let ns = project.tracks[0].clips[0].notes().unwrap().to_vec();
    assert_eq!(ns.len(), 10);
    let at = |pitch: u8, near: u64| {
        ns.iter()
            .find(|n| n.pitch == pitch && n.pos.0.abs_diff(near) < 200)
            .unwrap()
            .pos
            .0
    };
    assert!(at(40, 0) < at(64, 0), "下げは低い弦が先");
    assert!(at(64, 480) < at(50, 480), "上げは高い弦が先");
    assert!(
        !ns.iter().any(|n| n.pitch == 45 && n.pos.0 > 300),
        "上げは A 弦を鳴らさない"
    );
    // 上げは下げより弱い(最初に当たる弦どうし)
    let vel = |pitch: u8, near: u64| {
        ns.iter()
            .find(|n| n.pitch == pitch && n.pos.0.abs_diff(near) < 200)
            .unwrap()
            .vel
    };
    assert!(vel(64, 480) < vel(40, 0), "{} {}", vel(64, 480), vel(40, 0));
    assert!(
        at(40, 0) < 3840,
        "拍の手前に出る(前の小節に食い込まないよう 0 で止まる)"
    );
    // 終わりは同じ
    assert!(ns
        .iter()
        .filter(|n| n.pos.0 < 400)
        .all(|n| n.pos.0 + n.dur.0 == 400));
    ok_json(&call(&fx, "undo", json!({})).await);
    // 全部の弦で上げる
    let v = ok_json(
        &call(
            &fx,
            "strum_chord",
            json!({ "clip_id": "clp_tec001", "up_strings": 0 }),
        )
        .await,
    );
    assert_eq!(v["removed"], 0, "{v}");
    ok_json(&call(&fx, "undo", json!({})).await);
    // ピアノのばらし
    let v = ok_json(
        &call(
            &fx,
            "strum_chord",
            json!({ "clip_id": "clp_tec001", "style": "piano" }),
        )
        .await,
    );
    assert_eq!(v["removed"], 0, "{v}");
    // フラム: スネア 2 つに装飾音(120 BPM で 25ms = 48 tick 前、強さ半分)
    let v = ok_json(
        &call(
            &fx,
            "drum_rudiment",
            json!({ "clip_id": "clp_tec002", "kind": "flam" }),
        )
        .await,
    );
    assert_eq!(v["added"], 2, "{v}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    let d = project.tracks[1].clips[0].notes().unwrap().to_vec();
    assert!(d.iter().any(|n| n.pos.0 == 912 && n.vel == 55), "{d:?}");
    // ロール: 2 小節目の 3 拍目から 2 拍を 32 分で、だんだん強く(区間の同じ音程の音は置き換え)
    let v = ok_json(
        &call(
            &fx,
            "drum_rudiment",
            json!({ "clip_id": "clp_tec002", "kind": "roll", "bar": 2, "beat": 3, "length_beats": 2 }),
        )
        .await,
    );
    assert_eq!(v["added"], 16, "{v}");
    assert_eq!(v["replaced"], 0);
    // ラチェット: 3 回に割る
    let v = ok_json(
        &call(&fx, "drum_rudiment", json!({ "clip_id": "clp_tec002", "kind": "ratchet", "note_ids": [d.iter().find(|n| n.pos.0 == 960).unwrap().id.to_string()] })).await,
    );
    assert_eq!(v["added"], 2, "{v}");
    // 速すぎる連打は warning
    let v = ok_json(
        &call(
            &fx,
            "drum_rudiment",
            json!({ "clip_id": "clp_tec002", "kind": "hat_roll", "bar": 1, "beat": 1, "length_beats": 1, "rate": "1/64" }),
        )
        .await,
    );
    assert!(v["warning"].is_string(), "{v}");
    for bad in [
        (
            "drum_rudiment",
            json!({ "clip_id": "clp_tec002", "kind": "paradiddle" }),
        ),
        (
            "drum_rudiment",
            json!({ "clip_id": "clp_tec002", "kind": "roll" }),
        ),
        (
            "drum_rudiment",
            json!({ "clip_id": "clp_tec002", "kind": "roll", "bar": 9 }),
        ),
        (
            "strum_chord",
            json!({ "clip_id": "clp_tec001", "style": "banjo" }),
        ),
    ] {
        let r = call(&fx, bad.0, bad.1.clone()).await;
        assert_eq!(r.is_error, Some(true), "{bad:?}");
    }
}

#[tokio::test]
async fn articulation_tremolo_and_glissando_tools() {
    let fx = setup().await;
    ok_json(
        &call(
            &fx,
            "apply_commands",
            add_track_args("trk_art001", "Strings"),
        )
        .await,
    );
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "弦", "commands": [
                { "op": "add_clip", "track": "trk_art001", "clip": {
                    "id": "clp_art001", "name": "s", "start": 0, "length": 7680, "kind": "midi", "notes": [
                        { "id": "nt_art001", "pos": 0, "dur": 600, "pitch": 60, "vel": 90 },
                        { "id": "nt_art002", "pos": 960, "dur": 600, "pitch": 62, "vel": 90 },
                        { "id": "nt_art003", "pos": 1920, "dur": 1920, "pitch": 64, "vel": 90 },
                        { "id": "nt_art004", "pos": 3840, "dur": 960, "pitch": 72, "vel": 90 } ] } }
            ] }),
        )
        .await,
    );
    // レガート + スラー: 次の音へ 15ms(29 tick)重ね、2 音目以降に奏法 legato
    let v = ok_json(
        &call(
            &fx,
            "articulate_notes",
            json!({ "clip_id": "clp_art001", "style": "legato", "slur": true }),
        )
        .await,
    );
    assert!(v["changed"].as_u64().unwrap() >= 3, "{v}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    let ns = project.tracks[0].clips[0].notes().unwrap().to_vec();
    assert_eq!(ns[0].dur.0, 989);
    assert_eq!(ns[1].articulation, glaux_core::Articulation::Legato);
    assert_eq!(ns[0].articulation, glaux_core::Articulation::Normal);
    ok_json(&call(&fx, "undo", json!({})).await);
    // スタッカート
    ok_json(
        &call(
            &fx,
            "articulate_notes",
            json!({ "clip_id": "clp_art001", "style": "staccato" }),
        )
        .await,
    );
    let (project, _) = fx.handle.get_project().await.unwrap();
    assert_eq!(project.tracks[0].clips[0].notes().unwrap()[0].dur.0, 480);
    ok_json(&call(&fx, "undo", json!({})).await);
    // トレモロ: 2 拍の E4 を 32 分の交互に(16 打、裏は 3 度上の G4)
    let v = ok_json(
        &call(&fx, "tremolo", json!({ "clip_id": "clp_art001", "kind": "alternating", "note_ids": ["nt_art003"], "key": "C major" })).await,
    );
    assert_eq!(v["added"], 15, "{v}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    let ns = project.tracks[0].clips[0].notes().unwrap().to_vec();
    assert!(
        ns.iter().any(|n| n.pos.0 == 2040 && n.pitch == 67),
        "{ns:?}"
    );
    ok_json(&call(&fx, "undo", json!({})).await);
    // グリッサンド: E4 → 次の C5 を白鍵で
    let v = ok_json(
        &call(
            &fx,
            "glissando",
            json!({ "clip_id": "clp_art001", "note_ids": ["nt_art003"], "scale": "white" }),
        )
        .await,
    );
    // E4 から C5: F G A B の 4 音
    assert_eq!(v["added"], 4, "{v}");
    ok_json(&call(&fx, "undo", json!({})).await);
    // 滑らか: 音程の曲線で +800 セントへ
    ok_json(
        &call(
            &fx,
            "glissando",
            json!({ "clip_id": "clp_art001", "note_ids": ["nt_art003"], "mode": "continuous" }),
        )
        .await,
    );
    let (project, _) = fx.handle.get_project().await.unwrap();
    let e4 = project.tracks[0].clips[0].notes().unwrap()[2].clone();
    assert_eq!(e4.pitch_curve.last().unwrap().cents, 800.0);
    assert_eq!(e4.pitch_curve[0].tick.0, 960);
    for bad in [
        (
            "articulate_notes",
            json!({ "clip_id": "clp_art001", "style": "marcatissimo" }),
        ),
        (
            "tremolo",
            json!({ "clip_id": "clp_art001", "kind": "chord", "note_ids": ["nt_art001"], "division": "1/7" }),
        ),
        (
            "glissando",
            json!({ "clip_id": "clp_art001", "note_ids": ["nt_art004"] }),
        ),
        (
            "glissando",
            json!({ "clip_id": "clp_art001", "note_ids": ["nt_art003"], "mode": "wobble" }),
        ),
    ] {
        let r = call(&fx, bad.0, bad.1.clone()).await;
        assert_eq!(r.is_error, Some(true), "{bad:?}");
    }
}

#[tokio::test]
async fn shape_phrase_and_jazz_tempo_swing() {
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_phr001", "Piano")).await);
    // 8 小節の 8 分の刻み
    let notes: Vec<Value> = (0..64u64)
        .map(|k| json!({ "pos": k * 480, "dur": 400, "pitch": 60 + (k % 5) as u8, "vel": 80 }))
        .collect();
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "音", "commands": [
                { "op": "add_clip", "track": "trk_phr001", "clip": {
                    "id": "clp_phr001", "name": "p", "start": 0, "length": 30720, "kind": "midi", "notes": notes } }
            ] }),
        )
        .await,
    );
    // 5〜8 小節を句にして、最後の 1 小節で 0.6 倍へ緩める
    let v = ok_json(
        &call(
            &fx,
            "shape_phrase",
            json!({ "bar": 5, "bars": 4, "final_tempo": 0.6 }),
        )
        .await,
    );
    let before = v["seconds"]["before"].as_f64().unwrap();
    let after = v["seconds"]["after"].as_f64().unwrap();
    assert!((before - 8.0).abs() < 0.01 && after > before, "{v}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    let ev = project.tempo_map.events();
    assert!(
        ev.iter().any(|e| e.tick.0 == 15360 + 7680 && e.bpm > 120.0),
        "半ばで少し速い"
    );
    assert!(
        ev.iter().any(|e| e.tick.0 > 26880 && e.bpm < 80.0),
        "終わりで緩む"
    );
    assert_eq!(
        project.tempo_map.bpm_at(glaux_core::Tick(30720)),
        120.0,
        "区間の後は元に戻る"
    );
    assert_eq!(
        project.tempo_map.bpm_at(glaux_core::Tick(3840)),
        120.0,
        "区間の前は変えない"
    );
    // 緩む所の音は弱く
    let ns = project.tracks[0].clips[0].notes().unwrap();
    assert!(ns.last().unwrap().vel < 80);
    assert_eq!(ns[0].vel, 80);
    ok_json(&call(&fx, "undo", json!({})).await);
    let (project, _) = fx.handle.get_project().await.unwrap();
    assert_eq!(project.tempo_map.events().len(), 1);
    // ジャズのハネをテンポから: 120 BPM は 3 連(裏の 480 → 640)
    ok_json(
        &call(
            &fx,
            "swing_notes",
            json!({ "clip_id": "clp_phr001", "mode": "jazz_tempo" }),
        )
        .await,
    );
    let (project, _) = fx.handle.get_project().await.unwrap();
    assert_eq!(project.tracks[0].clips[0].notes().unwrap()[1].pos.0, 640);
    let r = call(&fx, "swing_notes", json!({ "clip_id": "clp_phr001" })).await;
    assert_eq!(r.is_error, Some(true), "swing も mode も無い");
}

#[tokio::test]
async fn drum_parts_chord_articulation_and_noise_presets() {
    let fx = setup().await;
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "トラック", "commands": [
                { "op": "add_track", "track": { "id": "trk_prt001", "name": "Kick", "kind": "midi",
                  "device": { "type": "builtin", "name": "drum" } } },
                { "op": "add_track", "track": { "id": "trk_prt002", "name": "Top", "kind": "midi",
                  "device": { "type": "builtin", "name": "drum" } } },
                { "op": "add_track", "track": { "id": "trk_prt003", "name": "Gtr", "kind": "midi" } },
                { "op": "add_track", "track": { "id": "trk_prt004", "name": "Vinyl", "kind": "midi" } }
            ] }),
        )
        .await,
    );
    // キックだけ・スネアとハットだけを別トラックに(同じ型)
    ok_json(
        &call(
            &fx,
            "write_drums",
            json!({ "track_id": "trk_prt001", "style": "rock", "bars": 2, "parts": ["kick"] }),
        )
        .await,
    );
    ok_json(&call(&fx, "write_drums", json!({ "track_id": "trk_prt002", "style": "rock", "bars": 2, "parts": ["snare", "hat"] })).await);
    let (project, _) = fx.handle.get_project().await.unwrap();
    let kick = project.tracks[0].clips[0].notes().unwrap();
    assert!(!kick.is_empty() && kick.iter().all(|n| n.pitch == 36));
    let top = project.tracks[1].clips[0].notes().unwrap();
    assert!(top.iter().any(|n| n.pitch == 38) && top.iter().any(|n| n.pitch == 42));
    assert!(top.iter().all(|n| n.pitch != 36 && n.pitch != 49));
    let r = call(
        &fx,
        "write_drums",
        json!({ "track_id": "trk_prt001", "style": "rock", "bars": 2, "parts": ["cowbell"] }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
    // ギターのブリッジミュートの刻み
    ok_json(
        &call(
            &fx,
            "write_chords",
            json!({ "track_id": "trk_prt003", "chords": "E5 | G5", "rhythm": "eighth", "articulation": "palm_mute" }),
        )
        .await,
    );
    let (project, _) = fx.handle.get_project().await.unwrap();
    assert!(project.tracks[2].clips[0]
        .notes()
        .unwrap()
        .iter()
        .all(|n| n.articulation == glaux_core::Articulation::PalmMute));
    let r = call(
        &fx,
        "write_chords",
        json!({ "track_id": "trk_prt003", "chords": "E5", "articulation": "tapping" }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
    // 雑音の音源のつまみ
    let params = ok_json(&call(&fx, "list_params", json!({ "instrument": "subtractive" })).await);
    assert!(params.to_string().contains("crackle"), "{params}");
}

#[tokio::test]
async fn meter_extras_and_musicxml_export() {
    let fx = setup().await;
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "準備", "commands": [
                { "op": "set_time_sig", "events": [ { "tick": 0, "num": 3, "den": 4 } ] },
                { "op": "add_track", "track": { "id": "trk_mx0001", "name": "Piano & Voice", "kind": "midi" } },
                { "op": "add_track", "track": { "id": "trk_mx0002", "name": "Drums", "kind": "midi",
                  "device": { "type": "builtin", "name": "drum" } } }
            ] }),
        )
        .await,
    );
    // 3/4 の 4 小節に 4 分の音(1 小節目は和音)
    let notes: Vec<Value> = (0..12u64)
        .map(|k| json!({ "pos": k * 960, "dur": 900, "pitch": 60 + (k % 7) as u8, "vel": 80 }))
        .chain(std::iter::once(
            json!({ "pos": 0, "dur": 900, "pitch": 64, "vel": 80 }),
        ))
        .collect();
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "音", "commands": [
                { "op": "add_clip", "track": "trk_mx0001", "clip": {
                    "id": "clp_mx0001", "name": "p", "start": 0, "length": 11520, "kind": "midi", "notes": notes } }
            ] }),
        )
        .await,
    );
    // ヘミオラ: 1〜2 小節目の 2 拍ごとの頭(0, 1920, 3840)を強く、2 小節目の頭(2880)を弱く
    let v = ok_json(&call(&fx, "hemiola", json!({ "bar": 1, "rebar": true })).await);
    assert!(v["accented"].as_u64().unwrap() >= 4, "{v}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    let ns = project.tracks[0].clips[0].notes().unwrap();
    let vel_at = |t: u64| {
        ns.iter()
            .find(|n| n.pos.0 == t && n.pitch != 64)
            .unwrap()
            .vel
    };
    assert!(
        vel_at(1920) > 80 && vel_at(2880) < 80,
        "{} {}",
        vel_at(1920),
        vel_at(2880)
    );
    assert_eq!(project.time_sig_map[0].num, 6);
    assert_eq!(project.time_sig_map[0].grouping, Some(vec![2, 2, 2]));
    ok_json(&call(&fx, "undo", json!({})).await);
    // 拍の置き換え: 小節の中で 4 分ずつ回す(3/4 の 3 拍目 → 1 拍目)
    ok_json(
        &call(
            &fx,
            "shift_notes",
            json!({ "clip_id": "clp_mx0001", "delta_ticks": 960, "wrap_in_bar": true }),
        )
        .await,
    );
    let (project, _) = fx.handle.get_project().await.unwrap();
    let ns = project.tracks[0].clips[0].notes().unwrap();
    assert!(ns.iter().all(|n| n.pos.0 < 11520));
    assert!(
        ns.iter().any(|n| n.pos.0 == 0 && n.pitch == 62),
        "3 拍目の D が頭へ"
    );
    ok_json(&call(&fx, "undo", json!({})).await);
    // メトリック・モジュレーション: 3 小節目から 3 連の 8 分 = 8 分(120 → 180)、拍子も 6/8 に
    let v = ok_json(
        &call(
            &fx,
            "metric_modulation",
            json!({ "bar": 3, "from": "triplet_8th", "to": "8th", "time_sig": "6/8" }),
        )
        .await,
    );
    assert_eq!(v["bpm_after"], 180.0, "{v}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    assert_eq!(project.tempo_map.bpm_at(glaux_core::Tick(5760)), 180.0);
    assert!(project
        .time_sig_map
        .iter()
        .any(|e| e.tick.0 == 5760 && e.num == 6 && e.den == 8));
    ok_json(&call(&fx, "undo", json!({})).await);
    // ティハイ: 3 回目の最後の音が 4 小節目の頭(8640)に着地
    let v = ok_json(
        &call(
            &fx,
            "write_tihai",
            json!({ "track_id": "trk_mx0002", "phrase": "x.x.xx", "gap_steps": 2, "land_bar": 4 }),
        )
        .await,
    );
    assert_eq!(v["notes"], 12, "{v}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    let t = &project.tracks[1].clips[0];
    let last = t
        .notes()
        .unwrap()
        .iter()
        .map(|n| t.start.0 + n.pos.0)
        .max()
        .unwrap();
    assert_eq!(last, 8640);
    assert!(t.notes().unwrap().iter().all(|n| n.pitch == 38));
    // MusicXML: 7/8(2+2+3)の小節を足して書き出す
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "7/8", "commands": [
                { "op": "set_time_sig", "events": [ { "tick": 0, "num": 3, "den": 4 },
                    { "tick": 11520, "num": 7, "den": 8, "grouping": [2, 2, 3] } ] },
                { "op": "add_clip", "track": "trk_mx0001", "clip": {
                    "id": "clp_mx0002", "name": "q", "start": 11520, "length": 3360, "kind": "midi", "notes": [
                        { "pos": 0, "dur": 1500, "pitch": 61, "vel": 80 } ] } }
            ] }),
        )
        .await,
    );
    let path = std::env::temp_dir().join(format!("glaux-mx-{}.musicxml", std::process::id()));
    let v = ok_json(
        &call(
            &fx,
            "export_musicxml",
            json!({ "path": path.to_string_lossy() }),
        )
        .await,
    );
    assert_eq!(v["parts"], 2, "{v}");
    let xml = std::fs::read_to_string(&path).unwrap();
    assert!(xml.contains("<beats>2+2+3</beats><beat-type>8</beat-type>"));
    assert!(xml.contains("<beats>3</beats><beat-type>4</beat-type>"));
    assert!(xml.contains("<part-name>Piano &amp; Voice</part-name>"));
    assert!(xml.contains("<chord/>"));
    assert!(
        xml.contains("<tie type=\"start\"/>"),
        "1500 tick の音は付点 4 分 + 32 分… にタイで分かれる"
    );
    assert!(xml.contains("<per-minute>120</per-minute>"));
    let _ = std::fs::remove_file(&path);
    for bad in [
        (
            "metric_modulation",
            json!({ "bar": 2, "from": "triplet_8th" }),
        ),
        ("metric_modulation", json!({ "bar": 2, "ratio": "100:1" })),
        (
            "write_tihai",
            json!({ "track_id": "trk_mx0002", "phrase": "....", "land_bar": 4 }),
        ),
        (
            "write_tihai",
            json!({ "track_id": "trk_mx0002", "phrase": "xxxxxxxx", "land_bar": 1 }),
        ),
    ] {
        let r = call(&fx, bad.0, bad.1.clone()).await;
        assert_eq!(r.is_error, Some(true), "{bad:?}");
    }
}

#[tokio::test]
async fn modulate_track_params_with_lfos() {
    let fx = setup().await;
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "準備", "commands": [
                { "op": "add_track", "track": { "id": "trk_mod001", "name": "Bass", "kind": "midi",
                  "device": { "type": "builtin", "name": "subtractive" },
                  "effects": [ { "id": "fx_mod001", "type": "builtin", "name": "auto_filter" } ] } }
            ] }),
        )
        .await,
    );
    // 音源のカットオフを 8 分の sine で(depth 省略 = 範囲の 1/4)
    let v = ok_json(
        &call(
            &fx,
            "modulate",
            json!({ "track_id": "trk_mod001", "target": "cutoff", "sync": "1/8" }),
        )
        .await,
    );
    assert_eq!(v["modulators"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["target"]["path"], "device/cutoff");
    // エフェクトのつまみ("種類.つまみ")を 1 小節の saw_down で
    let v = ok_json(
        &call(
            &fx,
            "modulate",
            json!({ "track_id": "trk_mod001", "target": "auto_filter.cutoff", "sync": "1/1", "shape": "saw_down", "depth": 2000 }),
        )
        .await,
    );
    assert_eq!(v["modulators"].as_array().unwrap().len(), 2, "{v}");
    assert_eq!(v["target"]["path"], "fx/fx_mod001/cutoff");
    // 同じつまみは置き換える
    let v = ok_json(
        &call(&fx, "modulate", json!({ "track_id": "trk_mod001", "target": "cutoff", "rate_hz": 0.5, "shape": "random" })).await,
    );
    assert_eq!(v["modulators"].as_array().unwrap().len(), 2);
    let (project, _) = fx.handle.get_project().await.unwrap();
    let m = &project.tracks[0].modulators;
    assert!(m
        .iter()
        .any(|x| x.shape == glaux_core::LfoShape::Random && x.sync.is_none()));
    // 外す → undo で戻る
    ok_json(
        &call(
            &fx,
            "modulate",
            json!({ "track_id": "trk_mod001", "remove": true }),
        )
        .await,
    );
    let (project, _) = fx.handle.get_project().await.unwrap();
    assert!(project.tracks[0].modulators.is_empty());
    ok_json(&call(&fx, "undo", json!({})).await);
    let (project, _) = fx.handle.get_project().await.unwrap();
    assert_eq!(project.tracks[0].modulators.len(), 2);
    for bad in [
        json!({ "track_id": "trk_mod001", "target": "nope" }),
        json!({ "track_id": "trk_mod001", "target": "waveform" }),
        json!({ "track_id": "trk_mod001", "target": "delay.mix" }),
        json!({ "track_id": "trk_mod001", "target": "cutoff", "sync": "1/0" }),
        json!({ "track_id": "trk_mod001", "target": "cutoff", "shape": "wobble" }),
    ] {
        let r = call(&fx, "modulate", bad.clone()).await;
        assert_eq!(r.is_error, Some(true), "{bad}");
    }
}

#[tokio::test]
async fn sustain_pedal_lengthens_arpeggios_to_chord_changes() {
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_ped001", "Piano")).await);
    // C の分散和音 1 小節 → F の分散和音 1 小節(8 分ずつ、短く切った音)
    let notes: Vec<Value> = [
        60u8, 64, 67, 72, 67, 64, 60, 64, 65, 69, 72, 77, 72, 69, 65, 69,
    ]
    .iter()
    .enumerate()
    .map(|(k, &p)| json!({ "pos": k as u64 * 480, "dur": 200, "pitch": p, "vel": 80 }))
    .collect();
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "音", "commands": [
                { "op": "add_clip", "track": "trk_ped001", "clip": {
                    "id": "clp_ped001", "name": "p", "start": 0, "length": 7680, "kind": "midi", "notes": notes } }
            ] }),
        )
        .await,
    );
    let v = ok_json(&call(&fx, "sustain_pedal", json!({ "clip_id": "clp_ped001" })).await);
    assert_eq!(v["pedal_changes"], 2, "{v}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    let ns = project.tracks[0].clips[0].notes().unwrap();
    // 1 小節目の音は 2 小節目の頭の手前(120BPM で 60ms = 115 tick 前)まで、2 小節目の音はクリップの終わりの手前まで
    assert_eq!(ns[0].pos.0 + ns[0].dur.0, 3840 - 115);
    assert_eq!(ns[8].pos.0 + ns[8].dur.0, 7680 - 115);
    let r = call(
        &fx,
        "sustain_pedal",
        json!({ "clip_id": "clp_ped001", "mode": "sometimes" }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
}

#[tokio::test]
async fn note_conditions_vary_loop_repetitions() {
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_cnd001", "Drums")).await);
    // 1 小節のループを 8 小節: スネア(38)とハット(42)の 8 分
    let notes: Vec<Value> = (0..8u64)
        .map(|k| json!({ "pos": k * 480, "dur": 120, "pitch": 42, "vel": 80 }))
        .chain(std::iter::once(
            json!({ "pos": 3360, "dur": 120, "pitch": 38, "vel": 100 }),
        ))
        .collect();
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "音", "commands": [
                { "op": "add_clip", "track": "trk_cnd001", "clip": {
                    "id": "clp_cnd001", "name": "d", "start": 0, "length": 30720, "kind": "midi",
                    "loop": true, "loop_len": 3840, "notes": notes } }
            ] }),
        )
        .await,
    );
    // 最後の 8 分のスネアは 4 小節目・8 小節目だけ、ハットは 7 割
    let v = ok_json(
        &call(
            &fx,
            "set_note_condition",
            json!({ "clip_id": "clp_cnd001", "pitch": 38, "every": "4:4" }),
        )
        .await,
    );
    assert_eq!(v["changed"], 1, "{v}");
    ok_json(
        &call(
            &fx,
            "set_note_condition",
            json!({ "clip_id": "clp_cnd001", "pitch": 42, "probability": 0.7 }),
        )
        .await,
    );
    let (project, _) = fx.handle.get_project().await.unwrap();
    let played = project.tracks[0].clips[0].playback_notes();
    let snares: Vec<u64> = played
        .iter()
        .filter(|n| n.pitch == 38)
        .map(|n| n.pos.0)
        .collect();
    assert_eq!(snares, vec![3840 * 3 + 3360, 3840 * 7 + 3360]);
    let hats = played.iter().filter(|n| n.pitch == 42).count();
    assert!(hats > 30 && hats < 60, "{hats}");
    // 外す
    ok_json(
        &call(
            &fx,
            "set_note_condition",
            json!({ "clip_id": "clp_cnd001", "clear": true }),
        )
        .await,
    );
    let (project, _) = fx.handle.get_project().await.unwrap();
    assert_eq!(project.tracks[0].clips[0].playback_notes().len(), 72);
    for bad in [
        json!({ "clip_id": "clp_cnd001", "every": "5:4" }),
        json!({ "clip_id": "clp_cnd001", "probability": 1.5 }),
        json!({ "clip_id": "clp_cnd001" }),
    ] {
        let r = call(&fx, "set_note_condition", bad.clone()).await;
        assert_eq!(r.is_error, Some(true), "{bad}");
    }
}

#[tokio::test]
async fn write_drums_places_a_genre_pattern_with_fills_and_a_build() {
    let fx = setup().await;
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "ドラム", "commands": [
                { "op": "add_track", "track": { "id": "trk_drm001", "name": "Drums", "kind": "midi",
                  "device": { "type": "builtin", "name": "drum" } } },
                { "op": "add_track", "track": { "id": "trk_drm002", "name": "Keys", "kind": "midi" } }
            ] }),
        )
        .await,
    );
    // ハウス 8 小節(3 小節目から)、8 小節目にフィル
    let v = ok_json(
        &call(
            &fx,
            "write_drums",
            json!({ "track_id": "trk_drm001", "style": "house", "bars": 8, "bar": 3 }),
        )
        .await,
    );
    assert_eq!(v["fill_bars"], json!([10]), "{v}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    let clip = &project.tracks[0].clips[0];
    assert_eq!((clip.start.0, clip.length.0), (3840 * 2, 3840 * 8));
    let ns = clip.notes().unwrap();
    // 4 つ打ち(フィルの小節は 3 拍目まで)
    assert_eq!(ns.iter().filter(|n| n.pitch == 36).count(), 4 * 7 + 3);
    assert!(ns.iter().any(|n| n.pitch == 49 && n.pos.0 == 0));
    // ビルド 4 小節 + 最後の 1 拍は無音
    let v = ok_json(
        &call(
            &fx,
            "write_drums",
            json!({ "track_id": "trk_drm001", "style": "techno", "bars": 4, "bar": 11,
                    "build_bars": 4, "gap_beats": 1, "fill": "none" }),
        )
        .await,
    );
    assert_eq!(v["fill_bars"], json!([]), "{v}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    let b = project.tracks[0].clips[1].notes().unwrap();
    assert!(b.iter().filter(|n| n.pitch == 38).count() >= 4 + 8 + 16);
    assert!(b.iter().all(|n| n.pos.0 < 3840 * 4 - 960));
    // ドラムでないトラック・不明な型はエラー
    let r = call(
        &fx,
        "write_drums",
        json!({ "track_id": "trk_drm002", "style": "house", "bars": 4 }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
    let r = call(
        &fx,
        "write_drums",
        json!({ "track_id": "trk_drm001", "style": "polka", "bars": 4 }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
}

#[tokio::test]
async fn write_transition_leaves_a_gap_and_rolls_into_the_drop() {
    let fx = setup().await;
    // 120 BPM。build 4 小節 → drop。鍵盤が 2 分音符でずっと鳴っている
    ok_json(
        &call(
            &fx,
            "set_song_plan",
            json!({ "sections": [ { "name": "build", "bars": 4 }, { "name": "drop", "bars": 4 } ] }),
        )
        .await,
    );
    let keys: Vec<Value> = (0..16u64)
        .map(|i| json!({ "pos": i * 1920, "dur": 1900, "pitch": 60, "vel": 90 }))
        .collect();
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "音", "commands": [
                { "op": "add_track", "track": { "id": "trk_trn001", "name": "Keys", "kind": "midi" } },
                { "op": "add_clip", "track": "trk_trn001", "clip": {
                    "id": "clp_trn001", "name": "k", "start": 0, "length": 30720, "kind": "midi", "notes": keys } },
                { "op": "add_track", "track": { "id": "trk_trn002", "name": "Drums", "kind": "midi",
                  "device": { "type": "builtin", "name": "drum" } } }
            ] }),
        )
        .await,
    );
    let v = ok_json(
        &call(
            &fx,
            "write_transition",
            json!({ "to": "drop", "gap_beats": 1, "drum_track_id": "trk_trn002", "roll_bars": 2 }),
        )
        .await,
    );
    assert_eq!(v["bar"], 5, "{v}");
    let boundary = 3840 * 4u64;
    let gap_start = boundary - 960;
    let (project, _) = fx.handle.get_project().await.unwrap();
    // 鍵盤: 無音の範囲で始まる音は無く、かかっていた音は手前で切れている
    let ks = project.tracks[0].clips[0].notes().unwrap();
    assert!(ks.iter().all(|n| !(gap_start..boundary).contains(&n.pos.0)));
    assert!(ks
        .iter()
        .filter(|n| n.pos.0 < gap_start)
        .all(|n| n.pos.0 + n.dur.0 <= gap_start));
    assert!(v["silenced_notes"].as_u64().unwrap() >= 1);
    // ドラム: ロール(無音の手前まで)・リバースクラッシュ(無音の手前で鳴り終わる)・区切りのクラッシュ
    let d = &project.tracks[1].clips[0];
    let abs = |n: &glaux_core::Note| d.start.0 + n.pos.0;
    let ns = d.notes().unwrap();
    let rev = ns
        .iter()
        .find(|n| n.pitch == 55)
        .expect("リバースクラッシュ");
    assert_eq!(abs(rev) + rev.dur.0, gap_start);
    // 1.6 秒 = 120 BPM で 3.2 拍
    assert_eq!(rev.dur.0, 3072);
    assert!(ns.iter().any(|n| n.pitch == 49 && abs(n) == boundary));
    let rolls: Vec<u64> = ns.iter().filter(|n| n.pitch == 38).map(abs).collect();
    assert!(
        rolls.len() >= 16
            && rolls
                .iter()
                .all(|p| *p < gap_start && *p >= boundary - 7680)
    );
    // 1 回の undo で両方戻る
    ok_json(&call(&fx, "undo", json!({})).await);
    let (project, _) = fx.handle.get_project().await.unwrap();
    assert_eq!(project.tracks[0].clips[0].notes().unwrap().len(), 16);
    assert!(project.tracks[1].clips.is_empty());
    // 無い区間はエラー
    let r = call(
        &fx,
        "write_transition",
        json!({ "to": "chorus", "gap_beats": 1 }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
}

#[tokio::test]
async fn suggest_progression_returns_chords_ready_for_write_chords() {
    let fx = setup().await;
    let v = ok_json(
        &call(
            &fx,
            "suggest_progression",
            json!({ "genre": "jpop", "mood": "emotional", "key": "D major", "count": 3 }),
        )
        .await,
    );
    let first = &v["progressions"][0];
    assert_eq!(first["name"], "王道進行", "{v}");
    assert_eq!(first["chords"], "Gmaj7 | A7 | F#m7 | Bm");
    // そのまま write_chords に渡せる
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_sug001", "Keys")).await);
    ok_json(
        &call(
            &fx,
            "write_chords",
            json!({ "track_id": "trk_sug001", "chords": first["chords"] }),
        )
        .await,
    );
    // 当てはまらなければ語彙を返す
    let v = ok_json(&call(&fx, "suggest_progression", json!({ "genre": "polka" })).await);
    assert!(v["progressions"].as_array().unwrap().is_empty());
    assert!(v["genres"].as_array().unwrap().iter().any(|g| g == "lofi"));
}

#[tokio::test]
async fn critique_melody_scores_a_shaped_line_above_a_random_one() {
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_mel001", "Lead")).await);
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_mel002", "Keys")).await);
    // 形のある 8 小節(C | F | G | C の上): 動機の繰り返し、強拍は和音の音、句の終わりを伸ばす
    let q = 960u64;
    let e = 480u64;
    let bars: [&[(u64, u64, u8)]; 8] = [
        &[
            (0, q, 64),
            (q, e, 62),
            (q + e, e, 60),
            (2 * q, q, 64),
            (3 * q, q, 67),
        ],
        &[(0, 3 * q, 69)],
        &[
            (0, q, 62),
            (q, e, 60),
            (q + e, e, 59),
            (2 * q, q, 62),
            (3 * q, q, 67),
        ],
        &[(0, 3 * q, 64)],
        &[
            (0, q, 64),
            (q, e, 62),
            (q + e, e, 60),
            (2 * q, q, 64),
            (3 * q, q, 67),
        ],
        &[(0, q, 72), (q, q, 69), (2 * q, 2 * q, 65)],
        &[
            (0, q, 71),
            (q, e, 69),
            (q + e, e, 67),
            (2 * q, q, 62),
            (3 * q, q, 59),
        ],
        &[(0, 4 * q, 60)],
    ];
    let good: Vec<Value> = bars
        .iter()
        .enumerate()
        .flat_map(|(b, ns)| {
            ns.iter().map(move |&(p, d, pitch)| {
                json!({ "pos": b as u64 * 3840 + p, "dur": d, "pitch": pitch, "vel": 90 })
            })
        })
        .collect();
    // でたらめな 8 分の列(跳び回り、戻らず、休み無し)
    let pitches = [
        60u8, 71, 62, 74, 64, 76, 57, 69, 59, 72, 61, 75, 58, 70, 63, 77,
    ];
    let bad: Vec<Value> = (0..64u64)
        .map(|i| json!({ "pos": i * 480, "dur": 480, "pitch": pitches[(i as usize * 7) % 16], "vel": 90 }))
        .collect();
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "旋律", "commands": [
                { "op": "add_clip", "track": "trk_mel001", "clip": {
                    "id": "clp_mel001", "name": "good", "start": 0, "length": 30720, "kind": "midi", "notes": good } },
                { "op": "add_clip", "track": "trk_mel002", "clip": {
                    "id": "clp_mel002", "name": "bad", "start": 0, "length": 30720, "kind": "midi", "notes": bad } }
            ] }),
        )
        .await,
    );
    let g = ok_json(
        &call(
            &fx,
            "critique_melody",
            json!({ "clip_id": "clp_mel001", "chords": "C | F | G | C | C | F | G | C", "key": "C major" }),
        )
        .await,
    );
    let b = ok_json(
        &call(
            &fx,
            "critique_melody",
            json!({ "track_id": "trk_mel002", "chords": "C | F | G | C | C | F | G | C", "genre": "jpop" }),
        )
        .await,
    );
    assert_eq!(g["metrics"]["strong_chord_tone"], 1.0, "{g}");
    assert!(g["score"].as_u64().unwrap() >= 80, "{g}");
    assert!(b["score"].as_u64().unwrap() < 60, "{b}");
    assert_eq!(b["genre"], "pop");
    assert!(b["findings"].to_string().contains("跳躍が多すぎる"));
    // 和音を渡さなければ、ほかのトラックから推定する(エラーにならない)
    ok_json(&call(&fx, "critique_melody", json!({ "track_id": "trk_mel001" })).await);
    let r = call(
        &fx,
        "critique_melody",
        json!({ "track_id": "trk_mel001", "genre": "polka" }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
}

#[tokio::test]
async fn develop_motif_turns_a_motif_into_a_shaped_phrase() {
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_mot001", "Lead")).await);
    // 2 小節の動機を sentence で 8 小節に
    let v = ok_json(
        &call(
            &fx,
            "develop_motif",
            json!({ "track_id": "trk_mot001", "motif": "E5:q D5:e C5:e E5:q G5:q | G5:w",
                    "chords": "C | F | G | C | Am | F | G | C", "key": "C major", "bar": 5 }),
        )
        .await,
    );
    assert_eq!(v["bars"], 8, "{v}");
    assert!(v["score"].as_u64().unwrap() >= 70, "{v}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    let clip = &project.tracks[0].clips[0];
    assert_eq!((clip.start.0, clip.length.0), (3840 * 4, 3840 * 8));
    let ns = clip.notes().unwrap();
    // 最後は主音で伸ばす。最高音は 1 回
    assert_eq!(ns.last().unwrap().pitch % 12, 0);
    let hi = ns.iter().map(|n| n.pitch).max().unwrap();
    assert_eq!(ns.iter().filter(|n| n.pitch == hi).count(), 1);
    // 既存のクリップ(1 小節の動機)を period で(進行は足りなければ繰り返す)
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "動機", "commands": [
                { "op": "add_clip", "track": "trk_mot001", "clip": {
                    "id": "clp_mot009", "name": "m", "start": 3840 * 30, "length": 3840, "kind": "midi", "notes": [
                        { "pos": 0, "dur": 960, "pitch": 69, "vel": 90 },
                        { "pos": 960, "dur": 480, "pitch": 72, "vel": 90 },
                        { "pos": 1440, "dur": 480, "pitch": 71, "vel": 90 },
                        { "pos": 1920, "dur": 1920, "pitch": 69, "vel": 90 } ] } }
            ] }),
        )
        .await,
    );
    let v = ok_json(
        &call(
            &fx,
            "develop_motif",
            json!({ "track_id": "trk_mot001", "motif_clip_id": "clp_mot009", "form": "period",
                    "chords": "Am | F", "bar": 20, "anticipate": 0.0 }),
        )
        .await,
    );
    assert_eq!(v["bars"], 4, "{v}");
    assert_eq!(v["key"], "A minor", "{v}");
    // 読めない形式・動機はエラー
    let r = call(
        &fx,
        "develop_motif",
        json!({ "track_id": "trk_mot001", "motif": "E5:q", "form": "rondo xyz" }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
    let r = call(
        &fx,
        "develop_motif",
        json!({ "track_id": "trk_mot001", "motif": "Q9:q" }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
}

#[tokio::test]
async fn write_melody_picks_the_best_of_several_candidates() {
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_wml001", "Vocal")).await);
    let v = ok_json(
        &call(
            &fx,
            "write_melody",
            json!({ "track_id": "trk_wml001", "role": "chorus", "chords": "IV | V | iii | vi",
                    "key": "C major", "bar": 3, "bars": 8, "candidates": 5, "place": 2, "seed": 7 }),
        )
        .await,
    );
    assert_eq!(v["bars"], 8, "{v}");
    let cands = v["candidates"].as_array().unwrap();
    assert_eq!(cands.len(), 5);
    // 点数の高い順。置いたのは 1 番目
    let scores: Vec<u64> = cands.iter().map(|c| c["score"].as_u64().unwrap()).collect();
    assert!(scores.windows(2).all(|w| w[0] >= w[1]), "{scores:?}");
    assert_eq!(v["score"], cands[0]["score"]);
    assert_eq!(v["seed"], cands[0]["seed"]);
    assert!(v["score"].as_u64().unwrap() >= 70, "{v}");
    // 返る動機は develop_motif にそのまま渡せる
    let motif = v["motif"].as_str().unwrap().to_owned();
    assert!(glaux_core::motif::parse_motif(&motif).is_ok(), "{motif}");
    let (project, _) = fx.handle.get_project().await.unwrap();
    // 2 番目の案は同じ音色の複製トラック(ミュート)に
    assert_eq!(project.tracks.len(), 2);
    let copy = &project.tracks[1];
    assert!(copy.mute && copy.name.contains("案2"), "{}", copy.name);
    assert_eq!(copy.device, project.tracks[0].device);
    let clip = &project.tracks[0].clips[0];
    assert_eq!((clip.start.0, clip.length.0), (3840 * 2, 3840 * 8));
    let ns = clip.notes().unwrap();
    assert!(ns.len() >= 16);
    // 最後は主音
    assert_eq!(ns.last().unwrap().pitch % 12, 0);
    // リズムが小節ごとに全部同じではない
    let bar_rhythm = |b: u64| -> Vec<u64> {
        ns.iter()
            .filter(|n| n.pos.0 / 3840 == b)
            .map(|n| n.pos.0 % 3840)
            .collect()
    };
    let rhythms: Vec<Vec<u64>> = (0..8).map(bar_rhythm).collect();
    assert!(rhythms.iter().any(|r| r != &rhythms[0]), "{rhythms:?}");
    // 同じ seed で 1 案だけ作ると、同じ旋律になる
    let seed = v["seed"].as_u64().unwrap();
    ok_json(&call(&fx, "undo", json!({})).await);
    let (project, _) = fx.handle.get_project().await.unwrap();
    assert_eq!(project.tracks.len(), 1);
    assert!(project.tracks[0].clips.is_empty());
    let again = ok_json(
        &call(
            &fx,
            "write_melody",
            json!({ "track_id": "trk_wml001", "role": "chorus", "chords": "IV | V | iii | vi",
                    "key": "C major", "bar": 3, "bars": 8, "candidates": 1, "seed": seed }),
        )
        .await,
    );
    assert_eq!(again["motif"], v["motif"]);
    assert_eq!(again["score"], v["score"]);
    // リズムを固定できる。エラー
    let v = ok_json(
        &call(
            &fx,
            "write_melody",
            json!({ "track_id": "trk_wml001", "role": "verse", "genre": "lofi", "chords": "ii7 | V7 | Imaj7 | vi7",
                    "key": "F major", "bar": 20, "bars": 4, "rhythm": "x-x-x---x-------" }),
        )
        .await,
    );
    assert_eq!(v["rhythm"], "x-x-x---x-------", "{v}");
    for bad in [
        json!({ "track_id": "trk_wml001", "role": "bridge", "key": "C major" }),
        json!({ "track_id": "trk_wml001", "genre": "polka", "key": "C major" }),
        json!({ "track_id": "trk_wml001", "rhythm": "x-q", "key": "C major" }),
        json!({ "track_id": "trk_wml001", "contour": "zigzag", "key": "C major" }),
    ] {
        let r = call(&fx, "write_melody", bad.clone()).await;
        assert_eq!(r.is_error, Some(true), "{bad}");
    }
}

#[tokio::test]
async fn set_song_plan_places_sections_and_critique_checks_the_plan() {
    let fx = setup().await;
    // 120 BPM: intro 4 + verse 8 + chorus 8 = 20 小節 = 40 秒
    let v = ok_json(
        &call(
            &fx,
            "set_song_plan",
            json!({ "sections": [
                { "name": "intro", "bars": 4, "energy": 3, "tracks": ["Keys"], "note": "鍵盤だけ" },
                { "name": "verse", "bars": 8, "energy": 5, "tracks": ["Keys", "Bass"] },
                { "name": "chorus", "bars": 8, "energy": 8, "tracks": ["Keys", "Bass"] }
            ] }),
        )
        .await,
    );
    assert_eq!(v["total_bars"], 20, "{v}");
    assert_eq!(v["end_bar"], 20);
    assert_eq!(v["duration"], "0:40");
    assert_eq!(v["sections"][1]["start_bar"], 5);
    assert_eq!(v["sections"][2]["tick"], 3840 * 12);
    let (project, _) = fx.handle.get_project().await.unwrap();
    assert_eq!(project.sections.len(), 3);
    assert_eq!(project.sections[0].note.as_deref(), Some("鍵盤だけ"));
    assert_eq!(project.sections[2].energy, Some(8.0));
    // 計画と違う曲: Bass が intro から鳴っている
    let bass: Vec<Value> = (0..80u64)
        .map(|i| json!({ "pos": i * 960, "dur": 480, "pitch": 40 + (i % 5) as u8, "vel": 90 + (i % 20) as u8 }))
        .collect();
    let keys: Vec<Value> = (0..80u64)
        .map(|i| json!({ "pos": i * 960 + 7, "dur": 900, "pitch": 64 + (i % 7) as u8, "vel": 70 + (i % 25) as u8 }))
        .collect();
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "音", "commands": [
                { "op": "add_track", "track": { "id": "trk_pln001", "name": "Bass", "kind": "midi" } },
                { "op": "add_clip", "track": "trk_pln001", "clip": {
                    "id": "clp_pln001", "name": "b", "start": 0, "length": 76800, "kind": "midi", "notes": bass } },
                { "op": "add_track", "track": { "id": "trk_pln002", "name": "Keys", "kind": "midi" } },
                { "op": "add_clip", "track": "trk_pln002", "clip": {
                    "id": "clp_pln002", "name": "k", "start": 0, "length": 76800, "kind": "midi", "notes": keys } }
            ] }),
        )
        .await,
    );
    let v = ok_json(&call(&fx, "critique_arrangement", json!({})).await);
    let text = v.to_string();
    assert!(text.contains("鳴らさないはずの Bass"), "{text}");
    assert_eq!(v["sections"][0]["planned_energy"], 3.0);
    // 1 回の undo で計画(マーカー)も戻る
    ok_json(&call(&fx, "undo", json!({})).await);
    ok_json(&call(&fx, "undo", json!({})).await);
    let (project, _) = fx.handle.get_project().await.unwrap();
    assert!(project.sections.is_empty());
    // 小節数 0 はエラー
    let r = call(
        &fx,
        "set_song_plan",
        json!({ "sections": [ { "name": "x", "bars": 0 } ] }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
}

#[tokio::test]
async fn transform_notes_develops_a_motif_in_the_key() {
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_trf001", "Lead")).await);
    // 動機 C D E を 4 小節のクリップに
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "動機", "commands": [
                { "op": "add_clip", "track": "trk_trf001", "clip": {
                    "id": "clp_trf001", "name": "m", "start": 0, "length": 15360, "kind": "midi", "notes": [
                        { "id": "nt_trf001", "pos": 0, "dur": 480, "pitch": 60, "vel": 100 },
                        { "id": "nt_trf002", "pos": 480, "dur": 480, "pitch": 62, "vel": 100 },
                        { "id": "nt_trf003", "pos": 960, "dur": 480, "pitch": 64, "vel": 100 } ] } }
            ] }),
        )
        .await,
    );
    // 反復進行: C メジャーで 2 度上へ 2 回(1 小節ずつ後ろ)
    let v = ok_json(
        &call(
            &fx,
            "transform_notes",
            json!({ "clip_id": "clp_trf001", "op": "sequence", "steps": 1, "times": 2, "offset_beats": 4, "key": "C major" }),
        )
        .await,
    );
    assert_eq!(v["added"], 6);
    let (project, _) = fx.handle.get_project().await.unwrap();
    let mut got: Vec<(u64, u8)> = project.tracks[0].clips[0]
        .notes()
        .unwrap()
        .iter()
        .map(|n| (n.pos.0, n.pitch))
        .collect();
    got.sort();
    assert_eq!(
        got,
        vec![
            (0, 60),
            (480, 62),
            (960, 64),
            (3840, 62),
            (4320, 64),
            (4800, 65),
            (7680, 64),
            (8160, 65),
            (8640, 67)
        ]
    );
    // 1 回の undo で戻る。A マイナーで反行(軸 = 最初の音 C)
    ok_json(&call(&fx, "undo", json!({})).await);
    ok_json(
        &call(
            &fx,
            "transform_notes",
            json!({ "clip_id": "clp_trf001", "op": "invert", "key": "A minor" }),
        )
        .await,
    );
    let (project, _) = fx.handle.get_project().await.unwrap();
    let pitches: Vec<u8> = project.tracks[0].clips[0]
        .notes()
        .unwrap()
        .iter()
        .map(|n| n.pitch)
        .collect();
    assert_eq!(pitches, vec![60, 59, 57]);
    // 不正な op・キーはエラー
    assert_eq!(
        call(
            &fx,
            "transform_notes",
            json!({ "clip_id": "clp_trf001", "op": "fold" })
        )
        .await
        .is_error,
        Some(true)
    );
    assert_eq!(
        call(
            &fx,
            "transform_notes",
            json!({ "clip_id": "clp_trf001", "op": "transpose", "steps": 1, "key": "H dorian" })
        )
        .await
        .is_error,
        Some(true)
    );
}

#[tokio::test]
async fn critique_arrangement_points_out_what_to_fix_and_clears_after_fixing() {
    let fx = setup().await;
    // 格子どおり・強さ一定の 8 分のハット(ドラム)を 4 小節
    let notes: Vec<Value> = (0..32)
        .map(|i| json!({ "pos": i * 480, "dur": 120, "pitch": 42, "vel": 100 }))
        .collect();
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "ハット", "commands": [
                { "op": "add_track", "track": { "id": "trk_crt001", "name": "Hat", "kind": "midi",
                  "device": { "type": "builtin", "name": "drum" } } },
                { "op": "add_clip", "track": "trk_crt001", "clip": {
                    "id": "clp_crt001", "name": "h", "start": 0, "length": 15360, "kind": "midi", "notes": notes } }
            ] }),
        )
        .await,
    );
    let v = ok_json(&call(&fx, "critique_arrangement", json!({})).await);
    let whats: Vec<String> = v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["what"].as_str().unwrap().to_owned())
        .collect();
    assert!(whats.iter().any(|w| w.contains("格子ちょうど")), "{v}");
    assert!(whats.iter().any(|w| w.contains("平ら")), "{v}");
    assert_eq!(v["tracks"][0]["on_grid"], 1.0);
    // グルーブを掛けると、その 2 つは消える
    ok_json(
        &call(
            &fx,
            "apply_groove",
            json!({ "clip_id": "clp_crt001", "style": "hiphop", "humanize_ms": 5 }),
        )
        .await,
    );
    let v = ok_json(&call(&fx, "critique_arrangement", json!({})).await);
    let whats: Vec<String> = v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["what"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        !whats
            .iter()
            .any(|w| w.contains("格子ちょうど") || w.contains("平ら")),
        "{v}"
    );
}

#[tokio::test]
async fn set_soundfont_instrument_accepts_sfz() {
    let fx = setup().await;
    // 別の場所に置いた SFZ(絶対パス)。ハイハットのオープンとクローズ(チョーク)
    let lib = fx.dir.parent().unwrap().join("sfzlib");
    std::fs::create_dir_all(&lib).unwrap();
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(lib.join("hat.wav"), spec).unwrap();
    for i in 0..24_000u32 {
        let v = ((i * 7919) % 200) as i16 * 100 - 10_000;
        w.write_sample(v).unwrap();
    }
    w.finalize().unwrap();
    std::fs::write(
        lib.join("hats kit.sfz"),
        "<control> label_cc30=Hat vol set_cc30=100\n\
         <group> pitch_keytrack=0 loop_mode=one_shot\n\
         <region> sample=hat.wav key=42 group=1\n\
         <region> sample=hat.wav key=44 locc30=110\n\
         <region> sample=hat.wav key=46 off_by=1\n",
    )
    .unwrap();
    let sfz = lib.join("hats kit.sfz").to_string_lossy().into_owned();

    ok_json(&call(&fx, "apply_commands", add_track_args("trk_sfz001", "Hats")).await);
    let v = ok_json(
        &call(
            &fx,
            "set_soundfont_instrument",
            json!({"track_id": "trk_sfz001", "sfz": sfz}),
        )
        .await,
    );
    assert_eq!(v["zones"], 2, "locc30=110 は既定の 100 では鳴らない");
    assert_eq!(v["controls"][0]["cc"], 30);
    assert_eq!(v["controls"][0]["label"], "Hat vol");
    assert_eq!(v["controls"][0]["default"], 100);
    // つまみだけ上書き(sfz を省く)。CC30 を 120 にすると key 44 も鳴る
    let v = ok_json(
        &call(
            &fx,
            "set_soundfont_instrument",
            json!({"track_id": "trk_sfz001", "sfz_cc": {"30": 120}}),
        )
        .await,
    );
    assert_eq!(v["zones"], 3);
    let (p2, _) = fx.handle.get_project().await.unwrap();
    let t2 = glaux_core::TrackId::parse("trk_sfz001").unwrap();
    match &p2.track(&t2).unwrap().device.as_ref().unwrap().source {
        glaux_core::PluginSource::Sfz { cc, .. } => assert_eq!(cc.get(&30), Some(&120)),
        other => panic!("{other:?}"),
    }
    let s44 = glaux_mcp::sound::render_note(&p2, &fx.dir, &t2, 44, 100, 0.2).unwrap();
    assert!(
        s44.frames.iter().any(|x| x.abs() > 0.05),
        "上書きした CC で鳴る"
    );
    // 戻す
    let v = ok_json(
        &call(
            &fx,
            "set_soundfont_instrument",
            json!({"track_id": "trk_sfz001", "sfz_cc": {}}),
        )
        .await,
    );
    assert_eq!(v["zones"], 2);
    let (project, _) = fx.handle.get_project().await.unwrap();
    let t = glaux_core::TrackId::parse("trk_sfz001").unwrap();
    let dev = &project.track(&t).unwrap().device.as_ref().unwrap().source;
    assert!(matches!(dev, glaux_core::PluginSource::Sfz { .. }));
    assert!(dev.is_drum_kit(), "名前に kit を含むのでドラム扱い");
    let s = glaux_mcp::sound::render_note(&project, &fx.dir, &t, 42, 100, 0.2).unwrap();
    assert!(s.frames.iter().any(|x| x.abs() > 0.05), "SFZ の波形が鳴る");

    // 外へ出る名前・引数の不足はエラー
    for args in [
        json!({"track_id": "trk_sfz001", "sfz": "../x.sfz"}),
        json!({"track_id": "trk_sfz001", "sfz": "none/missing.sfz"}),
        json!({"track_id": "trk_sfz001", "soundfont": "a.sf2"}),
    ] {
        let r = call(&fx, "set_soundfont_instrument", args).await;
        assert_eq!(r.is_error, Some(true));
    }
}

#[tokio::test]
async fn analyze_reference_reads_structure_of_file() {
    let fx = setup().await;
    let path = fx.dir.join("ref.wav");
    write_drum_wav(&path, 120.0, 16, 44_100);
    let v = ok_json(
        &call(
            &fx,
            "analyze_reference",
            json!({ "file": path.to_string_lossy() }),
        )
        .await,
    );
    let bpm = v["bpm"].as_f64().unwrap();
    assert!((bpm - 120.0).abs() < 1.0, "{v}");
    assert!(v["bars"].as_u64().unwrap() >= 12, "{v}");
    assert!(!v["sections"].as_array().unwrap().is_empty());
    assert!(v["summary"].as_str().unwrap().contains("構成"));
    assert_eq!(
        v["bar_energy_db"].as_array().unwrap().len() as u64,
        v["bars"].as_u64().unwrap()
    );
    let r = call(&fx, "analyze_reference", json!({})).await;
    assert_eq!(r.is_error, Some(true));
}

#[tokio::test]
async fn write_arpeggio_plays_chords_one_note_at_a_time() {
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_arp001", "Arp")).await);
    // 2 小節(C、Am)を 16 分の up、2 オクターブ
    let v = ok_json(
        &call(
            &fx,
            "write_arpeggio",
            json!({ "track_id": "trk_arp001", "chords": "C | Am", "style": "up", "octaves": 2, "voices": 3 }),
        )
        .await,
    );
    assert_eq!(v["bars"], 2, "{v}");
    assert_eq!(v["notes"], 32, "{v}");
    assert_eq!(v["step_ticks"], 240);
    let (project, _) = fx.handle.get_project().await.unwrap();
    let clip = &project.tracks[0].clips[0];
    let notes = clip.notes().unwrap();
    // 1 小節目は C の構成音だけ、上がっていく(6 音で 1 回り)
    let bar1: Vec<u8> = notes
        .iter()
        .filter(|n| n.pos.0 < 3840)
        .map(|n| n.pitch)
        .collect();
    assert!(
        bar1.iter().all(|p| [0, 4, 7].contains(&(p % 12))),
        "{bar1:?}"
    );
    assert!(bar1[..6].windows(2).all(|w| w[0] < w[1]), "{bar1:?}");
    assert_eq!(bar1[0], bar1[6], "6 音で 1 回り");
    assert!(notes
        .iter()
        .filter(|n| n.pos.0 >= 3840)
        .all(|n| [9, 0, 4].contains(&(n.pitch % 12))));

    // 8 分の 3 連、強弱の列とリズムの列(ポリメーター)
    let v = ok_json(
        &call(
            &fx,
            "write_arpeggio",
            json!({ "track_id": "trk_arp001", "chords": "Am7", "style": "pinky_up", "rate": "1/8t",
                    "accents": "110 70 90", "rhythm": "x x .", "bar": 3 }),
        )
        .await,
    );
    assert_eq!(v["step_ticks"], 320, "{v}");
    assert_eq!(v["notes"], 8, "12 ステップのうち 8 つ鳴る: {v}");

    for bad in [
        json!({ "track_id": "trk_arp001", "chords": "C", "style": "sideways" }),
        json!({ "track_id": "trk_arp001", "chords": "C", "rate": "fast" }),
        json!({ "track_id": "trk_arp001", "chords": "C", "rhythm": "...." }),
        json!({ "track_id": "trk_arp001", "chords": "C", "accents": "200" }),
    ] {
        let r = call(&fx, "write_arpeggio", bad).await;
        assert_eq!(r.is_error, Some(true));
    }
}

#[tokio::test]
async fn set_layer_stacks_sounds_on_a_track() {
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_lyr001", "Lead")).await);
    let t = glaux_core::TrackId::parse("trk_lyr001").unwrap();
    let c3_level = |snd: &glaux_mcp::sound::LoadedSound| {
        // 130.8Hz の大きさ(ゲルツェル)
        let n = (snd.sample_rate as usize / 2).min(snd.frames.len());
        let w = 2.0 * std::f32::consts::PI * 130.81 / snd.sample_rate;
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for &v in &snd.frames[..n] {
            let s0 = v + 2.0 * w.cos() * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        (s1 * s1 + s2 * s2 - 2.0 * w.cos() * s1 * s2).sqrt() / n as f32
    };
    let (p0, _) = fx.handle.get_project().await.unwrap();
    let before = glaux_mcp::sound::render_note(&p0, &fx.dir, &t, 60, 100, 0.5).unwrap();
    // 1 オクターブ下のサインを重ねる
    let v = ok_json(
        &call(
            &fx,
            "set_layer",
            json!({ "track_id": "trk_lyr001", "instrument": "subtractive", "params": { "waveform": "sine" },
                    "transpose": -12, "name": "サブ", "volume_db": -3.0 }),
        )
        .await,
    );
    assert_eq!(v["layers"][0]["index"], 1, "{v}");
    assert_eq!(v["layers"][0]["transpose"], -12);
    let (p1, _) = fx.handle.get_project().await.unwrap();
    let after = glaux_mcp::sound::render_note(&p1, &fx.dir, &t, 60, 100, 0.5).unwrap();
    assert!(
        c3_level(&after) > c3_level(&before) * 5.0,
        "サブの C3 が鳴る"
    );
    // 置き換え: 高い鍵盤だけにすると C4 では鳴らない
    let v = ok_json(
        &call(
            &fx,
            "set_layer",
            json!({ "track_id": "trk_lyr001", "index": 1, "key_range": "C5-C8", "vel_range": "90-127" }),
        )
        .await,
    );
    assert_eq!(v["layers"][0]["key_range"], "C5-C8", "{v}");
    assert_eq!(v["layers"][0]["vel_range"], "90-127");
    let (p2, _) = fx.handle.get_project().await.unwrap();
    let ranged = glaux_mcp::sound::render_note(&p2, &fx.dir, &t, 60, 100, 0.5).unwrap();
    assert!(c3_level(&ranged) < c3_level(&after) * 0.2);
    // 4 層目は作れない
    for _ in 0..2 {
        ok_json(
            &call(
                &fx,
                "set_layer",
                json!({ "track_id": "trk_lyr001", "instrument": "fm" }),
            )
            .await,
        );
    }
    let r = call(
        &fx,
        "set_layer",
        json!({ "track_id": "trk_lyr001", "instrument": "fm" }),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
    // 音源の指定がおかしい・音源無しの新しい層・範囲外の index はエラー
    for bad in [
        json!({ "track_id": "trk_lyr001", "index": 1, "instrument": "clap" }),
        json!({ "track_id": "trk_lyr001", "index": 1, "instrument": "fm", "sfz": "x.sfz" }),
        json!({ "track_id": "trk_lyr001", "index": 7, "volume_db": -6.0 }),
    ] {
        let r = call(&fx, "set_layer", bad).await;
        assert_eq!(r.is_error, Some(true));
    }
    // 外す
    let v = ok_json(
        &call(
            &fx,
            "set_layer",
            json!({ "track_id": "trk_lyr001", "index": 2, "remove": true }),
        )
        .await,
    );
    assert_eq!(v["layers"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn set_macro_moves_several_knobs() {
    let fx = setup().await;
    ok_json(&call(&fx, "apply_commands", add_track_args("trk_mac001", "Pad")).await);
    let v = ok_json(
        &call(
            &fx,
            "set_macro",
            json!({ "track_id": "trk_mac001", "name": "明るさ", "value": 0.25,
                    "targets": [ { "path": "cutoff", "min": 300.0, "max": 6300.0 },
                                 { "path": "track/volume_db", "min": -12.0, "max": 0.0, "curve": -0.5 } ] }),
        )
        .await,
    );
    assert_eq!(v["macros"][0]["path"], "macro/1", "{v}");
    assert_eq!(v["macros"][0]["targets"][0]["now"], 1800.0);
    // 値は apply_commands の set_param でも(macro/1)
    ok_json(
        &call(
            &fx,
            "apply_commands",
            json!({ "label": "マクロ", "commands": [ { "op": "set_param", "track": "trk_mac001", "path": "macro/1", "value": 1.0 } ] }),
        )
        .await,
    );
    let (project, _) = fx.handle.get_project().await.unwrap();
    let t = project
        .track(&glaux_core::TrackId::parse("trk_mac001").unwrap())
        .unwrap();
    assert_eq!(t.macros[0].value, 1.0);
    // 値だけ変える
    let v = ok_json(
        &call(
            &fx,
            "set_macro",
            json!({ "track_id": "trk_mac001", "index": 1, "value": 0.0 }),
        )
        .await,
    );
    assert_eq!(v["macros"][0]["targets"][0]["now"], 300.0);
    // 範囲外の値・知らないつまみ・targets 無しの新しいマクロ・途中のマクロの削除はエラー
    ok_json(
        &call(
            &fx,
            "set_macro",
            json!({ "track_id": "trk_mac001", "name": "揺れ", "targets": [ { "path": "resonance", "min": 0.0, "max": 0.5 } ] }),
        )
        .await,
    );
    for bad in [
        json!({ "track_id": "trk_mac001", "targets": [ { "path": "cutoff", "min": 0.0, "max": 99999.0 } ] }),
        json!({ "track_id": "trk_mac001", "targets": [ { "path": "nothing", "min": 0.0, "max": 1.0 } ] }),
        json!({ "track_id": "trk_mac001", "name": "x" }),
        json!({ "track_id": "trk_mac001", "index": 1, "remove": true }),
        json!({ "track_id": "trk_mac001", "index": 1, "value": 2.0 }),
    ] {
        let r = call(&fx, "set_macro", bad).await;
        assert_eq!(r.is_error, Some(true), "{:?}", r.content);
    }
    let v = ok_json(
        &call(
            &fx,
            "set_macro",
            json!({ "track_id": "trk_mac001", "index": 2, "remove": true }),
        )
        .await,
    );
    assert_eq!(v["macros"].as_array().unwrap().len(), 1);
}
