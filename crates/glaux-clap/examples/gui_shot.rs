//! プラグインの画面を開いて、数秒後の中身を PPM に保存する(Linux の画面の動作確認用)。
//!   cargo run -p glaux-clap --example gui_shot -- <.clap のパス> <出力.ppm> [秒]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = std::path::PathBuf::from(&args[1]);
    let out = &args[2];
    let secs: f64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(3.0);
    glaux_clap::mark_main_thread();
    let id = glaux_clap::describe(&path).expect("記述子を読める")[0]
        .id
        .clone();
    let mut plugin = glaux_clap::ClapPlugin::new(&path, &id).expect("生成できる");
    println!("{id}: 画面あり = {}", plugin.has_gui());
    plugin.open_gui(&id).expect("画面を開ける");
    let until = std::time::Instant::now() + std::time::Duration::from_secs_f64(secs);
    while std::time::Instant::now() < until {
        plugin.poll();
        let _ = plugin.gui_tick();
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
    #[cfg(target_os = "linux")]
    {
        let (w, h, rgb) = plugin.capture_gui_for_test().expect("中身を読める");
        let mut colors = std::collections::HashSet::new();
        for p in rgb.chunks(3) {
            colors.insert((p[0], p[1], p[2]));
        }
        println!("{w}x{h}、色 {} 種", colors.len());
        let mut f = format!("P6\n{w} {h}\n255\n").into_bytes();
        f.extend_from_slice(&rgb);
        std::fs::write(out, f).unwrap();
    }
    plugin.close_gui();
    let _ = out;
}
