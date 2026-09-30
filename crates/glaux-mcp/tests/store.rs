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

    // undo 後は履歴を切り詰め、整合が保たれる
    session.undo().unwrap().unwrap();
    store.save_after_change(&session).unwrap();
    let (_s3, reopened) = Store::open_or_create(dir_s).unwrap();
    assert_eq!(reopened.history().len(), 1);
    assert!(reopened.project().track(&id1).is_some());
    assert!(reopened.project().track(&id2).is_none());
}

#[test]
fn undo_truncates_history_and_redo_appends_again() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("Song.glaux");
    let dir_s = dir.to_str().unwrap();
    let lines = || {
        fs::read_to_string(dir.join("history.jsonl"))
            .unwrap()
            .lines()
            .count()
    };

    let (store, mut session) = Store::open_or_create(dir_s).unwrap();
    let mut ids = Vec::new();
    for name in ["A", "B", "C"] {
        let (id, cmd) = add_track_cmd(name);
        session.apply(cmd, Author::Human, name).unwrap();
        store.save_after_change(&session).unwrap();
        ids.push(id);
    }
    let full = fs::read_to_string(dir.join("history.jsonl")).unwrap();
    // undo 2 回: 行を切り詰めるだけ(残る行は元の先頭と同じ)
    session.undo().unwrap().unwrap();
    store.save_after_change(&session).unwrap();
    session.undo().unwrap().unwrap();
    store.save_after_change(&session).unwrap();
    assert_eq!(lines(), 1);
    let now = fs::read_to_string(dir.join("history.jsonl")).unwrap();
    assert!(full.starts_with(&now));
    // redo は追記、その後の undo も切り詰め
    session.redo().unwrap().unwrap();
    store.save_after_change(&session).unwrap();
    assert_eq!(lines(), 2);
    session.undo().unwrap().unwrap();
    store.save_after_change(&session).unwrap();
    assert_eq!(lines(), 1);
    drop(store);

    // 開き直しても整合し、開いた後の undo も切り詰めで済む
    let (store, mut reopened) = Store::open_or_create(dir_s).unwrap();
    assert_eq!(reopened.history().len(), 1);
    assert!(reopened.project().track(&ids[0]).is_some());
    assert!(reopened.project().track(&ids[1]).is_none());
    reopened.undo().unwrap().unwrap();
    store.save_after_change(&reopened).unwrap();
    assert_eq!(lines(), 0);
    drop(store);
    let (_s, again) = Store::open_or_create(dir_s).unwrap();
    assert_eq!(again.history().len(), 0);
    assert!(again.project().track(&ids[0]).is_none());
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

#[test]
fn background_writes_finish_on_flush_and_drop() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("Song.glaux");
    let dir_s = dir.to_str().unwrap();
    let (mut store, mut session) = Store::open_or_create(dir_s).unwrap();
    store.enable_background_writes();
    let mut ids = Vec::new();
    for i in 0..20 {
        let (id, cmd) = add_track_cmd(&format!("T{i}"));
        session.apply(cmd, Author::Human, format!("T{i}")).unwrap();
        store.save_after_change_snapshot(&session).unwrap();
        ids.push(id);
    }
    // 書き終えるのを待てば、project.json は最新
    store.flush().unwrap();
    let saved = fs::read_to_string(dir.join("project.json")).unwrap();
    assert!(saved.contains(ids[19].as_str()));
    // undo は裏の書き込みを待ってから、その場で書く(履歴が project.json より遅れない)
    session.undo().unwrap().unwrap();
    store.save_after_change_snapshot(&session).unwrap();
    let saved = fs::read_to_string(dir.join("project.json")).unwrap();
    assert!(!saved.contains(ids[19].as_str()));
    // 捨てる(= 終了)ときも書き終える
    let (id, cmd) = add_track_cmd("last");
    session.apply(cmd, Author::Human, "last").unwrap();
    store.save_after_change_snapshot(&session).unwrap();
    drop(store);
    let (_s, reopened) = Store::open_or_create(dir_s).unwrap();
    assert_eq!(reopened.history().len(), 20);
    assert!(reopened.project().track(&id).is_some());
}

#[test]
fn project_json_several_steps_behind_is_recovered_from_history() {
    // 裏の書き込みが数手遅れているうちに落ちた: 履歴は先にあり、その手は「やり直し」で戻せる
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("Song.glaux");
    let dir_s = dir.to_str().unwrap();
    let ids = project_with_tracks(dir_s, 2);
    let stale = fs::read_to_string(dir.join("project.json")).unwrap();
    {
        let (store, mut session) = Store::open_or_create(dir_s).unwrap();
        for i in 0..6 {
            let (_, cmd) = add_track_cmd(&format!("U{i}"));
            session.apply(cmd, Author::Human, format!("U{i}")).unwrap();
            store.save_after_change(&session).unwrap();
        }
    }
    fs::write(dir.join("project.json"), stale).unwrap();
    let (_s, mut reopened) = Store::open_or_create(dir_s).unwrap();
    assert_eq!(reopened.history().len(), 2);
    assert!(reopened.project().track(&ids[1]).is_some());
    for _ in 0..6 {
        reopened.redo().unwrap().unwrap();
    }
    assert_eq!(reopened.project().tracks.len(), 8);
}

#[test]
fn large_history_is_compacted_by_size() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("Song.glaux");
    let dir_s = dir.to_str().unwrap();
    let (store, mut session) = Store::open_or_create(dir_s).unwrap();
    let mut ids = Vec::new();
    for i in 0..40 {
        let (id, cmd) = add_track_cmd(&format!("T{i}"));
        session.apply(cmd, Author::Human, format!("T{i}")).unwrap();
        store.save_after_change(&session).unwrap();
        ids.push(id);
    }
    let size = fs::metadata(dir.join("history.jsonl")).unwrap().len();
    // 件数は少ないが、大きさのしきい値(今の半分)を超えている → 後ろから 1/4 の大きさ分だけ残す
    assert!(store
        .maybe_compact_by_bytes(&mut session, size / 2)
        .unwrap());
    let kept = session.history().len();
    assert!((8..=12).contains(&kept), "{kept}");
    let after = fs::metadata(dir.join("history.jsonl")).unwrap().len();
    assert!(after <= size / 4 + size / 40, "{after} / {size}");
    drop(store);
    let (_s, reopened) = Store::open_or_create(dir_s).unwrap();
    assert_eq!(reopened.history().len(), kept);
    assert!(reopened.project().track(&ids[0]).is_some());
    assert!(reopened.project().track(&ids[39]).is_some());
}

/// 中身のある曲(デモ曲・曲の写し)
fn song_with_tracks(n: usize) -> (glaux_core::Project, Vec<TrackId>) {
    let mut p = glaux_core::Project::new("写し");
    let mut ids = Vec::new();
    for i in 0..n {
        let id = TrackId::new();
        p.tracks
            .push(Track::new(id.clone(), format!("元{i}"), TrackKind::Midi));
        ids.push(id);
    }
    (p, ids)
}

#[test]
fn a_song_created_with_content_keeps_its_history_on_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("Demo.glaux");
    let dir_s = dir.to_str().unwrap();
    let (p, ids) = song_with_tracks(2);
    glaux_mcp::store::create_project_from(&dir, p).unwrap();
    assert!(dir.join("history.base.json").exists(), "起点を書く");
    let (store, mut session) = Store::open_or_create(dir_s).unwrap();
    // 元からあるトラックを消す編集(空の曲からは再生できない)
    session
        .apply(
            Command::RemoveTrack { id: ids[0].clone() },
            Author::Human,
            "元0 を消す",
        )
        .unwrap();
    store.save_after_change(&session).unwrap();
    drop(store);
    for _ in 0..2 {
        let (_s, reopened) = Store::open_or_create(dir_s).unwrap();
        assert_eq!(reopened.history().len(), 1);
        assert!(reopened.project().track(&ids[0]).is_none());
        assert!(!dir.join("history.jsonl.orphan").exists());
    }
}

#[test]
fn history_without_a_start_point_is_recovered_by_working_backwards() {
    // 起点(history.base.json)を書かずに中身のある曲の上へ履歴を積んだ状態(以前の不具合で作られた曲)
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("Rev.glaux");
    let dir_s = dir.to_str().unwrap();
    let (p, ids) = song_with_tracks(3);
    glaux_mcp::store::create_project_from(&dir, p).unwrap();
    let (store, mut session) = Store::open_or_create(dir_s).unwrap();
    session
        .apply(
            Command::RemoveTrack { id: ids[1].clone() },
            Author::Human,
            "元1 を消す",
        )
        .unwrap();
    store.save_after_change(&session).unwrap();
    let (new_id, cmd) = add_track_cmd("新");
    session.apply(cmd, Author::Human, "新を足す").unwrap();
    store.save_after_change(&session).unwrap();
    drop(store);
    fs::remove_file(dir.join("history.base.json")).unwrap();
    // 開く: 空の曲からは再生できないが、project.json から起点を逆算して履歴を生かす
    let (store, mut reopened) = Store::open_or_create(dir_s).unwrap();
    assert_eq!(reopened.history().len(), 2);
    assert!(dir.join("history.base.json").exists());
    assert!(!dir.join("history.jsonl.orphan").exists());
    // 元に戻せる
    reopened.undo().unwrap().unwrap();
    reopened.undo().unwrap().unwrap();
    assert!(reopened.project().track(&ids[1]).is_some());
    assert!(reopened.project().track(&new_id).is_none());
    store.save_after_change(&reopened).unwrap();
    drop(store);
    let (_s, again) = Store::open_or_create(dir_s).unwrap();
    assert_eq!(again.history().len(), 0);
    assert!(again.project().track(&ids[1]).is_some());
}

#[test]
fn after_an_unusable_history_is_set_aside_the_new_history_survives() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("Song.glaux");
    let dir_s = dir.to_str().unwrap();
    let ids = project_with_tracks(dir_s, 2);
    // 読めない履歴
    fs::write(dir.join("history.jsonl"), "not json\n").unwrap();
    fs::write(dir.join("history.jsonl.orphan"), "前の退避\n").unwrap();
    let (store, mut session) = Store::open_or_create(dir_s).unwrap();
    assert_eq!(session.history().len(), 0);
    // 前の退避は上書きしない
    assert_eq!(
        fs::read_to_string(dir.join("history.jsonl.orphan")).unwrap(),
        "前の退避\n"
    );
    assert!(dir.join("history.jsonl.orphan.2").exists());
    // 新しい履歴(元からあるトラックを消す)は、次に開いても残る
    session
        .apply(
            Command::RemoveTrack { id: ids[0].clone() },
            Author::Human,
            "消す",
        )
        .unwrap();
    store.save_after_change(&session).unwrap();
    drop(store);
    let (_s, reopened) = Store::open_or_create(dir_s).unwrap();
    assert_eq!(reopened.history().len(), 1);
    assert!(reopened.project().track(&ids[0]).is_none());
    assert!(!dir.join("history.jsonl.orphan.3").exists());
}
