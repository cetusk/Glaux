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

/// 成功前提で structured_content を取り出す
fn ok_json(result: &CallToolResult) -> &Value {
    assert_ne!(
        result.is_error,
        Some(true),
        "tool failed: {:?}",
        result.content
    );
    result
        .structured_content
        .as_ref()
        .expect("structured_content")
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
    let r = call(&fx, "checkpoint", json!({ "label": "base" })).await;
    assert_eq!(ok_json(&r)["project_version"], 1);

    let r = call(
        &fx,
        "apply_commands",
        json!({
            "commands": [ { "op": "set_master_volume", "volume_db": -3.0 } ],
            "label": "マスター音量",
        }),
    )
    .await;
    assert_eq!(ok_json(&r)["project_version"], 2);

    let r = call(&fx, "revert_to", json!({ "label": "base" })).await;
    assert_eq!(ok_json(&r)["project_version"], 1);

    // 6. since フィルタ: base 以降の履歴は無い(revert_to は undo なので履歴を増やさない)
    let r = call(&fx, "get_history", json!({ "since": entry_id })).await;
    assert_eq!(ok_json(&r)["entries"], json!([]));

    // 7. undo でトラック追加ごと取り消し(Batch が 1 単位)
    let r = call(&fx, "undo", json!({})).await;
    let v = ok_json(&r);
    assert_eq!(v["undone"], 1);
    assert_eq!(v["project_version"], 0);

    let r = call(&fx, "get_project", json!({})).await;
    assert_eq!(ok_json(&r)["project"]["tracks"], json!([]));

    // 8. redo で復活
    let r = call(&fx, "redo", json!({})).await;
    let v = ok_json(&r);
    assert_eq!(v["redone"], 1);
    assert_eq!(v["project_version"], 1);
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
