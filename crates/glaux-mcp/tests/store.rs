//! Store の永続化テスト: 新規作成、履歴リプレイでの再開、壊れた履歴の退避。

use glaux_core::{Author, Command, Track, TrackId, TrackKind};
use glaux_mcp::store::Store;
use std::fs;

fn add_track_cmd(name: &str) -> (TrackId, Command) {
    let id = TrackId::new();
    let track = Track::new(id.clone(), name, TrackKind::Midi);
    (id, Command::AddTrack { track, index: None })
}

#[test]
fn creates_new_project_with_dir_stem_title() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("MySong.glaux");

    let (_store, session) = Store::open_or_create(dir.to_str().unwrap()).unwrap();

    assert_eq!(session.project().meta.title, "MySong");
    assert!(dir.join("project.json").exists());
    assert!(dir.join("history.jsonl").exists());
}

#[test]
fn reopen_replays_history_and_keeps_undo() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("Song.glaux");
    let dir_s = dir.to_str().unwrap();

    let (store, mut session) = Store::open_or_create(dir_s).unwrap();
    let (track_id, cmd) = add_track_cmd("Bass");
    session.apply(cmd, Author::Human, "トラック追加").unwrap();
    store.save(&session).unwrap();
    drop(session);

    // 再オープン: 履歴から再構築され、過去セッションの操作を undo できる
    let (_store, mut reopened) = Store::open_or_create(dir_s).unwrap();
    assert_eq!(reopened.history().len(), 1);
    assert!(reopened.project().track(&track_id).is_some());
    assert!(reopened.can_undo());
    reopened.undo().unwrap().unwrap();
    assert!(reopened.project().track(&track_id).is_none());
}

#[test]
fn broken_history_is_moved_aside_and_project_wins() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("Song.glaux");
    let dir_s = dir.to_str().unwrap();

    let (store, mut session) = Store::open_or_create(dir_s).unwrap();
    let (track_id, cmd) = add_track_cmd("Lead");
    session.apply(cmd, Author::Human, "トラック追加").unwrap();
    store.save(&session).unwrap();
    drop(session);

    fs::write(dir.join("history.jsonl"), "not json\n").unwrap();

    let (_store, reopened) = Store::open_or_create(dir_s).unwrap();
    // project.json が正として採用され、履歴は空・壊れたファイルは退避
    assert!(reopened.project().track(&track_id).is_some());
    assert_eq!(reopened.history().len(), 0);
    assert!(dir.join("history.jsonl.orphan").exists());
}

#[test]
fn incremental_save_appends_and_falls_back_on_undo() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("Song.glaux");
    let dir_s = dir.to_str().unwrap();

    let (store, mut session) = Store::open_or_create(dir_s).unwrap();
    let (id1, cmd1) = add_track_cmd("A");
    session.apply(cmd1, Author::Human, "A").unwrap();
    store.save_after_change(&session).unwrap(); // 追記パス
    let (id2, cmd2) = add_track_cmd("B");
    session.apply(cmd2, Author::Human, "B").unwrap();
    store.save_after_change(&session).unwrap(); // 追記パス

    // 追記された 2 行が正しくリプレイできる
    let (_s2, reopened) = Store::open_or_create(dir_s).unwrap();
    assert_eq!(reopened.history().len(), 2);
    assert!(reopened.project().track(&id1).is_some());
    assert!(reopened.project().track(&id2).is_some());
    drop(reopened);

    // undo 後は全書き換えにフォールバックし、整合が保たれる
    session.undo().unwrap().unwrap();
    store.save_after_change(&session).unwrap();
    let (_s3, reopened) = Store::open_or_create(dir_s).unwrap();
    assert_eq!(reopened.history().len(), 1);
    assert!(reopened.project().track(&id1).is_some());
    assert!(reopened.project().track(&id2).is_none());
}

#[test]
fn compaction_keeps_reopen_and_undo_working() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("Song.glaux");
    let dir_s = dir.to_str().unwrap();

    let (store, mut session) = Store::open_or_create(dir_s).unwrap();
    let mut ids = Vec::new();
    for i in 0..40 {
        let (id, cmd) = add_track_cmd(&format!("T{i}"));
        ids.push(id);
        session.apply(cmd, Author::Human, format!("T{i}")).unwrap();
        store.save_after_change(&session).unwrap();
    }
    assert_eq!(session.history().len(), 40);

    // しきい値 30 超 → 直近 10 件だけ残す
    assert!(store.maybe_compact_with(&mut session, 30, 10).unwrap());
    assert_eq!(session.history().len(), 10);
    assert!(dir.join("history.base.json").exists());
    let lines = fs::read_to_string(dir.join("history.jsonl")).unwrap();
    assert_eq!(lines.lines().count(), 10);
    let archive = fs::read_to_string(dir.join("history.archive.jsonl")).unwrap();
    assert_eq!(archive.lines().count(), 30);
    // 2 回目は何もしない
    assert!(!store.maybe_compact_with(&mut session, 30, 10).unwrap());

    // 追記パスは compaction 後も動く
    let (id41, cmd) = add_track_cmd("T40");
    session.apply(cmd, Author::Human, "T40").unwrap();
    store.save_after_change(&session).unwrap();
    let expected = session.project().clone();
    drop(session);

    // 再オープン: base + 履歴 11 件で復元され、全トラックがあり、undo は残った分だけ
    let (_s2, mut reopened) = Store::open_or_create(dir_s).unwrap();
    assert_eq!(reopened.project(), &expected);
    assert_eq!(reopened.history().len(), 11);
    assert!(reopened.project().track(&ids[0]).is_some());
    for _ in 0..11 {
        reopened.undo().unwrap().unwrap();
    }
    assert!(!reopened.can_undo());
    assert!(reopened.project().track(&id41).is_none());
    assert!(
        reopened.project().track(&ids[29]).is_some(),
        "起点に含まれる分は残る"
    );
    assert!(reopened.project().track(&ids[30]).is_none());
}

// ---- 保存の途中で落ちたときの復元 ----------------------------------------------

/// トラックを n 本足して保存したプロジェクトを作る
fn project_with_tracks(dir_s: &str, n: usize) -> Vec<TrackId> {
    let (store, mut session) = Store::open_or_create(dir_s).unwrap();
    let mut ids = Vec::new();
    for i in 0..n {
        let (id, cmd) = add_track_cmd(&format!("T{i}"));
        ids.push(id);
        session.apply(cmd, Author::Human, format!("T{i}")).unwrap();
        store.save_after_change(&session).unwrap();
    }
    ids
}

#[test]
fn truncated_last_history_line_is_dropped() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("Song.glaux");
    let dir_s = dir.to_str().unwrap();
    let ids = project_with_tracks(dir_s, 2);

    // 3 本目の行を書いている途中で落ちた(project.json は 2 本のまま)
    let path = dir.join("history.jsonl");
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str("{\"id\":\"hst_trunc");
    fs::write(&path, text).unwrap();

    let (_s, mut reopened) = Store::open_or_create(dir_s).unwrap();
    assert!(!dir.join("history.jsonl.orphan").exists());
    assert_eq!(reopened.history().len(), 2);
    reopened.undo().unwrap().unwrap();
    assert!(reopened.project().track(&ids[1]).is_none());
    // 壊れた行は書き直されて消えている
    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(text.lines().count(), 2);
}

#[test]
fn crash_after_history_append_keeps_undo_and_allows_redo() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("Song.glaux");
    let dir_s = dir.to_str().unwrap();
    project_with_tracks(dir_s, 2);
    let old_project = fs::read_to_string(dir.join("project.json")).unwrap();

    // 3 本目: 履歴への追記は済んだが、project.json を書く前に落ちた
    let (store, mut session) = Store::open_or_create(dir_s).unwrap();
    let (id3, cmd) = add_track_cmd("T2");
    session.apply(cmd, Author::Human, "T2").unwrap();
    store.save_after_change(&session).unwrap();
    drop(session);
    fs::write(dir.join("project.json"), &old_project).unwrap();

    let (store, mut reopened) = Store::open_or_create(dir_s).unwrap();
    assert!(!dir.join("history.jsonl.orphan").exists());
    assert_eq!(reopened.history().len(), 2, "undo の履歴は残る");
    assert!(reopened.project().track(&id3).is_none());
    // 落ちる直前の編集は「やり直し」で戻せる
    assert!(reopened.can_redo());
    reopened.redo().unwrap().unwrap();
    assert!(reopened.project().track(&id3).is_some());
    // その後の編集も普通に保存・再現できる
    store.save_after_change(&reopened).unwrap();
    let expected = reopened.project().clone();
    drop(reopened);
    let (_s, again) = Store::open_or_create(dir_s).unwrap();
    assert_eq!(again.project(), &expected);
    assert_eq!(again.history().len(), 3);
}

#[test]
fn crash_during_undo_rewrite_keeps_history() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("Song.glaux");
    let dir_s = dir.to_str().unwrap();
    let ids = project_with_tracks(dir_s, 3);
    let old_history = fs::read_to_string(dir.join("history.jsonl")).unwrap();

    // undo: project.json は書けたが、history.jsonl の全書き換えの前に落ちた
    let (store, mut session) = Store::open_or_create(dir_s).unwrap();
    session.undo().unwrap().unwrap();
    store.save_after_change(&session).unwrap();
    drop(session);
    fs::write(dir.join("history.jsonl"), &old_history).unwrap();

    let (_s, reopened) = Store::open_or_create(dir_s).unwrap();
    assert!(!dir.join("history.jsonl.orphan").exists());
    assert_eq!(reopened.history().len(), 2);
    assert!(reopened.project().track(&ids[2]).is_none());
    assert!(reopened.can_redo());
}

#[test]
fn crash_during_compaction_does_not_apply_folded_entries_twice() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("Song.glaux");
    let dir_s = dir.to_str().unwrap();
    project_with_tracks(dir_s, 12);
    let old_history = fs::read_to_string(dir.join("history.jsonl")).unwrap();

    // compaction: 起点は書けたが、history.jsonl を書き直す前に落ちた
    let (store, mut session) = Store::open_or_create(dir_s).unwrap();
    assert!(store.maybe_compact_with(&mut session, 10, 4).unwrap());
    let expected = session.project().clone();
    drop(session);
    fs::write(dir.join("history.jsonl"), &old_history).unwrap();

    let (_s, reopened) = Store::open_or_create(dir_s).unwrap();
    assert!(!dir.join("history.jsonl.orphan").exists());
    assert_eq!(reopened.project(), &expected);
    assert_eq!(reopened.history().len(), 4);
}

#[test]
fn old_base_format_is_still_read() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("Song.glaux");
    let dir_s = dir.to_str().unwrap();
    project_with_tracks(dir_s, 12);
    let (store, mut session) = Store::open_or_create(dir_s).unwrap();
    assert!(store.maybe_compact_with(&mut session, 10, 4).unwrap());
    let expected = session.project().clone();
    drop(session);

    // 以前の形式(Project をそのまま書いた起点)に書き換える
    let base: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.join("history.base.json")).unwrap()).unwrap();
    fs::write(dir.join("history.base.json"), base["project"].to_string()).unwrap();

    let (_s, reopened) = Store::open_or_create(dir_s).unwrap();
    assert!(!dir.join("history.jsonl.orphan").exists());
    assert_eq!(reopened.project(), &expected);
    assert_eq!(reopened.history().len(), 4);
}

#[test]
fn another_process_cannot_open_the_same_project() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("Song.glaux");
    let dir_s = dir.to_str().unwrap();
    let (_store, _session) = Store::open_or_create(dir_s).unwrap();

    // 同じプロセスで開き直すのはよい(切り替えて戻る・読み直し)
    let (_again, _) = Store::open_or_create(dir_s).unwrap();

    // 別のプロセス(stdio の MCP サーバー)は断られる
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_glaux-mcp"))
        .arg(dir_s)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("別の Glaux"), "{stderr}");

    // ロックを外せば開ける
    glaux_mcp::store::release_lock(&dir);
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_glaux-mcp"))
        .arg(dir_s)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("別の Glaux"), "{stderr}");
}
