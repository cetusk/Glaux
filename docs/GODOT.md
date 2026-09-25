# Glaux を Godot で使う(glaux-godot)

Glaux で作った曲(`.glaux` フォルダ)を Godot 4 のゲームの中でそのまま鳴らし、敵の動きなどを
曲の拍・小節・マーカー・特定の音(キック等)に同期させるための拡張です。

この文書は **Glaux 側の作業**(ビルド・配布物の作り方・デモ)の説明です。
ゲーム側での使い方は、配布物に同梱の説明書を見てください:

| ファイル | 対象 | 内容 |
|---|---|---|
| [`godot/demo/addons/glaux/README.md`](../godot/demo/addons/glaux/README.md) | 人間 | 導入・GDScript の例・API の一覧・注意事項 |
| [`godot/demo/addons/glaux/AI_GUIDE.md`](../godot/demo/addons/glaux/AI_GUIDE.md) | AI | ゲーム側の AI 向けの手引き(正確な API 仕様・実装パターン・落とし穴) |
| [`godot/demo/addons/glaux/PROMPT.md`](../godot/demo/addons/glaux/PROMPT.md) | 人間 → AI | ゲーム側の AI に渡すプロンプトの例(導入・敵の同期・判定・調整・効果音) |
| [`godot/demo/addons/glaux/CHANGELOG.md`](../godot/demo/addons/glaux/CHANGELOG.md) | 両方 | 改訂ノート(版ごとの変更。更新したら必ず書き足す) |

対応: Godot 4.3 以降、Windows(64bit)。Linux でも動作確認済み(macOS は未確認)。

## 1. ビルドと配布物の作成(Windows)

必要なもの: Rust 1.94 以降(`rustup update` で最新にできます)。

Godot のエディタを閉じてから、リポジトリのフォルダで次を実行します。

```bat
godot\build_windows.bat
```

やること:

1. `cargo build -p glaux-godot --release` で拡張(`glaux_godot.dll`)をビルド(初回は数分)
2. デモのアドオン `godot\demo\addons\glaux\bin\` に DLL を置く
3. 配布物を作る:
   - `godot\dist\addons\glaux\`(`glaux.gdextension`・`bin\glaux_godot.dll`・書き出しプラグイン `plugin.cfg` / `plugin.gd` / `export_plugin.gd`・`README.md`・`AI_GUIDE.md`・`PROMPT.md`・`CHANGELOG.md`)
   - `godot\dist\glaux-godot-addon.zip`(上のフォルダを zip にしたもの。`addons\glaux\` の形で入っている)

## 2. 別のプロジェクト(ゲーム)へ入れる

1. `godot\dist\glaux-godot-addon.zip` をゲームのプロジェクトのフォルダに展開する
   (`addons\glaux\` ができる。既にあれば丸ごと置き換える)。フォルダをコピーしても同じ
2. 曲を置く。おすすめは Glaux で**ゲームの `songs\` の中に直接作る / 開く**こと(曲名のメニュー →
   「フォルダを選択して開く…」で `songs` を選ぶと、中の曲の一覧か「このフォルダに新しい曲を作る」が出る)。
   コピーの手間がなく、Glaux で直せばそのままゲームに入る。別の場所の曲を使うなら、その曲のフォルダ
   (例 `Stage1.glaux`)をコピーする(必要なのは `project.json` と `audio\`)
3. Godot のエディタを開き直し、「プロジェクト設定 → プラグイン」で「Glaux」を有効にする(書き出したゲームに曲を入れる)
4. ゲーム側で AI を使うなら、`addons\glaux\PROMPT.md` の「1. 最初の導入」を AI に渡す

拡張を更新したときは、1 章をやり直して、できた配布物でゲーム側の `addons\glaux\` を丸ごと置き換えます
(エディタを閉じてから)。曲をコピーして使っている場合は、曲を直すたびにコピーし直します
(ゲームの `songs\` の中で直接作っている曲は不要)。

## 3. デモ

Godot 4.3 で `godot/demo/project.godot` を開き、実行(F5)します。デモの曲(4 小節)が鳴り、
出力パネルに小節・マーカーが表示されます。

ヘッドレスでの自動確認(4.5 秒鳴らして、拍・キック・マーカーの数と音量の最大を出して終了):

```bat
Godot_v4.3-stable_win64.exe --headless --path godot\demo -- --check
```

`RESULT ... beats=10 kicks=10 sections=["intro", "サビ"] peak=0.298 ... errors=0` のようになれば正常です
(ヘッドレスでは出力の遅れが 0 として扱われます)。

## 4. 開発メモ

- 実装: `crates/glaux-godot`(`GlauxPlayer` = `player.rs`、音声スレッド = `stream.rs`、読み込み = `song.rs`)、
  時間軸は `crates/glaux-engine/src/timeline.rs`
- アドオンの説明書(`README.md` / `AI_GUIDE.md` / `PROMPT.md`)は `godot/demo/addons/glaux/` が正。
  API を変えたら 3 つとも更新し、`AI_GUIDE.md` のコード例は Godot で実際に動かして確かめる
