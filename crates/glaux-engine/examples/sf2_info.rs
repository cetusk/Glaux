//! SoundFont の中身を確認する開発用ツール。
//! 使い方: cargo run -p glaux-engine --example sf2_info -- <path/to/font.sf2>

fn main() {
    let path = std::env::args().nth(1).expect("usage: sf2_info <path.sf2>");
    let font = glaux_engine::sf2::load_font(std::path::Path::new(&path)).expect("parse failed");
    let presets = glaux_engine::sf2::list_presets(&font);
    println!("presets: {}", presets.len());
    for p in presets.iter().take(12) {
        println!("  bank {:3} preset {:3}  {}", p.bank, p.preset, p.name);
    }
    // 代表的なプリセットでゾーン構築も確認
    for (bank, preset) in [(0u16, 0u16), (0, 24), (0, 48), (128, 0)] {
        match glaux_engine::sf2::build_zones(&font, bank, preset) {
            Some(z) => println!("zones({bank},{preset}): {}", z.len()),
            None => println!("zones({bank},{preset}): なし"),
        }
    }
}
