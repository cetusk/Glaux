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
