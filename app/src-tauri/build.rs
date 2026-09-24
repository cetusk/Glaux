fn main() {
    // tauri-build は exe に埋め込むアイコン(icons/icon.ico)の変更を見張らないので、tauri.conf.json が
    // 変わらない限り古いアイコンが exe に残る。icons/ の変更でもビルドスクリプトをやり直す
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}
