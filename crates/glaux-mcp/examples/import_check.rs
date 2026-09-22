//! 音声ファイル取り込みの手動確認: `cargo run -p glaux-mcp --example import_check -- <file>`
fn main() {
    let src = std::env::args().nth(1).expect("path");
    let dir = std::env::temp_dir().join("glaux_import_check");
    std::fs::create_dir_all(&dir).unwrap();
    let a = glaux_mcp::assets::import_audio(&dir, std::path::Path::new(&src)).unwrap();
    let data = glaux_engine::load_wav_mono(&dir.join(&a.asset.path)).unwrap();
    let rms = (data.frames.iter().map(|s| s * s).sum::<f32>() / data.frames.len() as f32).sqrt();
    let secs = data.frames.len() as f32 / data.sample_rate;
    // 周波数は中央 1 秒で測る(mp3 の先頭・末尾の無音パディングを避ける)
    let sr = data.sample_rate as usize;
    let mid = &data.frames[data.frames.len() / 2 - sr / 2..data.frames.len() / 2 + sr / 2];
    let crossings = mid.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
    println!(
        "{} Hz, {} ch, {:.2} s, rms {:.3}, 推定周波数 {:.0} Hz",
        a.asset.sample_rate, a.asset.channels, secs, rms, crossings as f32
    );
}
