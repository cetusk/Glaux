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
