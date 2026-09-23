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
                note_id: None,
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

#[test]
fn params_are_listed_and_set_by_events() {
    let Some(path) = test_plugin() else {
        eprintln!("GLAUX_TEST_CLAP が未設定のためスキップ");
        return;
    };
    let info = describe(&path)
        .unwrap()
        .into_iter()
        .find(|p| p.is_instrument())
        .unwrap();
    let mut plugin = ClapPlugin::new(&path, &info.id).unwrap();
    let params = plugin.param_infos();
    let visible: Vec<&ParamInfo> = params
        .iter()
        .filter(|p| p.automatable && !p.hidden && !p.readonly)
        .collect();
    eprintln!(
        "パラメータ {} 個(操作できるもの {} 個)",
        params.len(),
        visible.len()
    );
    assert!(!visible.is_empty());
    for p in visible.iter().take(8) {
        eprintln!(
            "  {} / {} [{}..{}] 既定 {} stepped={}",
            p.module, p.name, p.min, p.max, p.default, p.stepped
        );
    }
    // 連続値のパラメータを 1 つ選び、今と違う端の値にする
    let target = visible
        .iter()
        .find(|p| !p.stepped && p.max > p.min)
        .expect("連続値のパラメータがある");
    eprintln!(
        "対象: {} / {} [{}..{}] 既定 {}",
        target.module, target.name, target.min, target.max, target.default
    );
    let before = plugin.param_values(&[target.id]);
    assert_eq!(before.len(), 1);
    eprintln!("変更前: {:?}", before[0]);

    let mut proc = plugin.activate(48_000.0).unwrap();
    let min = if (before[0].1 - target.min).abs() < 1e-6 {
        target.max
    } else {
        target.min
    };
    let id = target.id;
    let proc = std::thread::spawn(move || {
        proc.process(
            256,
            &[NoteMsg::Param {
                time: 0,
                id,
                value: min,
            }],
        );
        proc.process(256, &[]);
        proc
    })
    .join()
    .unwrap();
    let after = plugin.param_values(&[target.id]);
    eprintln!("変更後: {:?}", after[0]);
    assert!((after[0].1 - min).abs() < 1e-6, "イベントで値が変わる");
    plugin.deactivate(proc);
}

/// `GLAUX_TEST_CLAP_PRESET` にプリセットファイル(Surge XT なら .fxp)を指定したときだけ動く。
#[test]
fn preset_file_loads_and_changes_params() {
    let (Some(path), Some(preset)) = (
        test_plugin(),
        std::env::var_os("GLAUX_TEST_CLAP_PRESET").map(PathBuf::from),
    ) else {
        eprintln!("GLAUX_TEST_CLAP / GLAUX_TEST_CLAP_PRESET が未設定のためスキップ");
        return;
    };
    let info = describe(&path)
        .unwrap()
        .into_iter()
        .find(|p| p.is_instrument())
        .unwrap();
    let mut plugin = ClapPlugin::new(&path, &info.id).unwrap();
    assert!(plugin.can_load_presets());
    let ids: Vec<u32> = plugin
        .param_infos()
        .iter()
        .filter(|p| p.automatable && !p.hidden)
        .map(|p| p.id)
        .collect();
    let before = plugin.param_values(&ids);
    let state_before = plugin.save_state().unwrap();
    plugin
        .load_preset_file(&preset, None)
        .expect("プリセットを読み込める");
    let after = plugin.param_values(&ids);
    let changed = before
        .iter()
        .zip(&after)
        .filter(|(a, b)| (a.1 - b.1).abs() > 1e-9)
        .count();
    eprintln!("プリセットで {changed} 個のつまみが変わった");
    assert!(changed > 0);
    assert_ne!(state_before, plugin.save_state().unwrap());
}
