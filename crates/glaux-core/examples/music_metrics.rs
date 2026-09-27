//! 曲の「機械っぽさ・平板さ」の数値を一覧にする(同じ依頼を作らせて、前後の版を比べるベンチマーク用)。
//! `cargo run -p glaux-core --example music_metrics -- <曲のフォルダ(.glaux)か project.json> ...`
//!
//! 数値は critique_arrangement と同じ計算。音(音量・帯域)の数値は MCP の analyze_audio で別に取る。

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("使い方: music_metrics <Song.glaux か project.json> ...");
        std::process::exit(2);
    }
    println!(
        "{:<28} {:>5} {:>7} {:>6} {:>6} {:>5} {:>6} {:>6} {:>5}",
        "曲", "小節", "格子%", "強弱SD", "表情%", "自動", "起伏", "warn", "info"
    );
    for p in paths {
        let file = if p.ends_with(".json") {
            p.clone()
        } else {
            format!("{p}/project.json")
        };
        let project = match std::fs::read_to_string(&file)
            .map_err(|e| e.to_string())
            .and_then(|s| glaux_core::Project::from_json(&s).map_err(|e| e.to_string()))
        {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{file}: 読めません({e})");
                continue;
            }
        };
        let c = glaux_core::critique::critique(&project);
        // 音の数で重み付けした平均(ドラム・パッドも含む全トラック)
        let total: usize = c.tracks.iter().map(|t| t.notes).sum::<usize>().max(1);
        let w = |f: &dyn Fn(&glaux_core::critique::TrackMetrics) -> f64| {
            c.tracks.iter().map(|t| f(t) * t.notes as f64).sum::<f64>() / total as f64
        };
        let on_grid = w(&|t| t.on_grid) * 100.0;
        let vel_sd = w(&|t| t.velocity_sd);
        let expressive = w(&|t| t.expressive) * 100.0;
        let energy_spread = if c.sections.len() >= 2 {
            let es: Vec<f64> = c.sections.iter().map(|s| s.energy).collect();
            es.iter().cloned().fold(f64::MIN, f64::max)
                - es.iter().cloned().fold(f64::MAX, f64::min)
        } else {
            f64::NAN
        };
        let warns = c.findings.iter().filter(|f| f.severity == "warn").count();
        let name = std::path::Path::new(&p)
            .file_name()
            .map_or(p.clone(), |n| n.to_string_lossy().into_owned());
        println!(
            "{:<28} {:>5} {:>7.1} {:>6.1} {:>6.1} {:>5} {:>6.1} {:>6} {:>5}",
            name,
            c.song_bars,
            on_grid,
            vel_sd,
            expressive,
            c.automation_lanes,
            energy_spread,
            warns,
            c.findings.len() - warns
        );
    }
}
