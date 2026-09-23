//! 実プラグインを使うテスト。環境変数 `GLAUX_TEST_CLAP` に音源プラグインの `.clap`
//! (例: Surge XT)を指定したときだけ動く(無ければ何もせず成功扱い)。

use super::*;

fn test_plugin() -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var_os("GLAUX_TEST_CLAP")?);
    p.exists().then_some(p)
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

#[test]
fn scan_finds_nothing_in_empty_dir() {
    let tmp = std::env::temp_dir().join("glaux-clap-empty");
    let _ = std::fs::create_dir_all(&tmp);
    assert!(scan(&[tmp]).is_empty());
}

#[test]
fn instrument_plays_notes_and_restores_state() {
    let Some(path) = test_plugin() else {
        eprintln!("GLAUX_TEST_CLAP が未設定のためスキップ");
        return;
    };
    let list = describe(&path).expect("記述子を読める");
    let info = list
        .iter()
        .find(|p| p.is_instrument())
        .expect("音源プラグインが入っている");
    eprintln!(
        "テスト対象: {} ({}) {:?}",
        info.name, info.id, info.features
    );

    let mut plugin = ClapPlugin::new(&path, &info.id).expect("生成できる");
    let state = plugin.save_state().expect("状態を保存できる");
    assert!(!state.is_empty());

    let mut proc = plugin.activate(48_000.0).expect("起動できる");
    assert!(proc.accepts_notes());
    // 別スレッド(オーディオスレッド役)で鳴らす
    let (proc, loud, quiet) = std::thread::spawn(move || {
        proc.process(1024, &[]);
        let quiet = rms(proc.output().unwrap().0);
        proc.process(
            1024,
            &[NoteMsg::On {
                time: 0,
                key: 60,
                velocity: 0.9,
            }],
        );
        let mut loud = 0.0f32;
        for _ in 0..10 {
            proc.process(1024, &[]);
            loud = loud.max(rms(&proc.output().unwrap().0[..1024]));
        }
        proc.process(1024, &[NoteMsg::AllOff { time: 0 }]);
        (proc, loud, quiet)
    })
    .join()
    .unwrap();
    assert!(!proc.has_failed());
    assert!(quiet < 1e-4, "ノート前は無音: {quiet}");
    assert!(loud > 1e-3, "ノートで音が出る: {loud}");
    plugin.deactivate(proc);

    // 状態を戻せる(同じ状態を読み込んで保存し直しても壊れない)
    plugin.load_state(&state).expect("状態を戻せる");
    let again = plugin.save_state().unwrap();
    assert!(!again.is_empty());
}
