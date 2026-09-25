<p align="center"><img src="docs/images/glaux-logo.png" alt="Glaux" width="480"></p>

# Glaux: A Lightweight DAW for Making Music with AI

日本語 | [English](README.en.md)

**AI と一緒に曲を作る、シンプルで軽量なデスクトップ DAW** です(Rust + Tauri + Svelte 5)。
チャットで「4 小節のベースを作って」「サビだけ盛り上げて」と頼むと、AI がプロジェクトを直接編集し、タイムラインにすぐ反映されます。

> [!NOTE]
> 個人で開発している実験的なプロジェクトです。主に Windows 11 で確認しています(Linux はコアとエンジンのビルド・テストのみ、macOS は未確認)。
> 仕様やファイル形式は予告なく変わることがあります。

## 特徴

- **AI と共同作曲** — 相手は Claude(Claude Code)か GPT(Codex CLI)。人間と AI の編集は同じ履歴に残り、AI の 1 ターン分をまとめて取り消せます
- **AI の「耳」** — 音量・帯域・キーとコード・リズム・音色を AI 自身が測り、確かめてから報告します
- **音源とエフェクト** — 内蔵シンセ 7 種(シンセ・FM・ウェーブテーブル・ドラム・撥弦・サンプラー・SoundFont)、エフェクト 9 種(EQ・コンプ・リバーブ・アンプなど)、CLAP プラグイン(Surge XT など)
- **打ち込みと録音** — ピアノロール、ドラムキット、フレット盤、MIDI キーボード、録音、鼻歌の譜起こし、パート分離、テンポ追従
- **AI にやさしい設計** — プロジェクトは読める JSON。すべての編集が同じコマンドを通り、完全に undo できます。MCP サーバーを内蔵
- **ゲームで鳴らす** — Godot 4 の拡張で、曲をゲームの中で鳴らし、敵の動きや判定を拍に同期できます

AI ができること・作れるジャンルの詳細は [`docs/CAPABILITIES.md`](docs/CAPABILITIES.md) にあります。

## はじめかた(Windows)

### アプリを使う

```bat
scripts\build-release.bat
```

`release\` に `Glaux.exe`(そのまま動く単体の exe)とインストーラーができます。必要なのは WebView2 だけです(Windows 10 / 11 に同梱)。

### ソースから動かす

必要なもの: [Rust](https://rustup.rs) 1.89 以上 / [Node.js](https://nodejs.org) LTS

```bat
scripts\glaux-app.bat [C:\path\to\MySong.glaux]
```

### AI チャットの準備

チャットは、PC にインストールしてログイン済みの CLI を使います。チャットパネルの左上で相手を切り替えます。

| 相手 | CLI | 準備 |
|---|---|---|
| Claude | [Claude Code](https://claude.com/claude-code) | インストールしてログイン |
| GPT | [Codex CLI](https://github.com/openai/codex) | `npm i -g @openai/codex` → `codex login` |

AI が使えるのは Glaux のツールだけです(PC のファイルの読み書きやコマンドの実行はさせません)。

## MCP クライアントからつなぐ

アプリの起動中は、HTTP の MCP サーバーが `http://127.0.0.1:41920/mcp` で動いています。

```bat
claude mcp add --transport http glaux http://127.0.0.1:41920/mcp
```

Codex なら `~/.codex/config.toml` に `[mcp_servers.glaux]` と `url = "http://127.0.0.1:41920/mcp"` を書きます。
アプリなしで使う stdio 版は `scripts\glaux-mcp.bat <曲のフォルダ>` です(同じ曲をアプリと同時には開けません)。

<details>
<summary>ツールの一覧(43 個)</summary>

| 分類 | ツール |
|---|---|
| 基本 | `get_project` `apply_commands` `undo` `redo` `checkpoint` `revert_to` `revert` `get_history` `get_changes` `list_params` `get_guide` |
| 構成 | `duplicate_clips` `insert_bars` `delete_bars` `bounce_track` `export_audio` |
| 分析 | `analyze_audio` `analyze_harmony` `analyze_rhythm` `analyze_beats` `analyze_sound` `compare_sounds` `match_sound` |
| ノート | `transpose_notes` `shift_notes` `swing_notes` `quantize_notes` `scale_velocity` |
| 音色 | `list_presets` `save_preset` `load_preset` `delete_preset` `find_similar_presets` `list_soundfonts` `set_soundfont_instrument` |
| CLAP | `list_plugins` `list_plugin_presets` `load_plugin_preset` `refine_plugin_params` |
| 素材 | `import_sample` `import_audio_clip` `transcribe_audio` `separate_audio` |

</details>

## Godot 4 で使う

`GlauxPlayer` ノードで `.glaux` の曲を鳴らし、「いま聞こえている位置」で拍・マーカー・ノートをシグナルとして受け取れます。
効果音を BGM のコードに合わせて鳴らすこともできます。ビルドと配布物の作り方は [`docs/GODOT.md`](docs/GODOT.md)、
ゲーム側での使い方はアドオンの [`README`](godot/demo/addons/glaux/README.md) を見てください。

## 構成

```
crates/
  glaux-core    プロジェクトモデル・コマンド・履歴・和声とリズムの分析
  glaux-mcp     MCP サーバー・セッション・プリセット
  glaux-engine  リアルタイム再生・録音・MIDI 入力・書き出し・音の解析
  glaux-dsp     内蔵の楽器とエフェクト
  glaux-ml      学習済みモデルの推論(譜起こし・音程・拍・音色)
  glaux-clap    CLAP プラグインのホスト
  glaux-godot   Godot 4.3+ の拡張
app/            デスクトップアプリ(Tauri + Svelte 5)
godot/          Godot のデモとアドオンのビルド
```

開発: `cargo test --workspace` / `cargo clippy --workspace --all-targets`。ドキュメント・コメントは日本語で、AI(Claude Code)と共同で開発しています。
改善の予定は [`docs/IMPROVEMENTS.md`](docs/IMPROVEMENTS.md) にあります。

## 同梱物とライセンス

ソースコードとドキュメントは **MIT または Apache-2.0** のデュアルライセンスです([LICENSE-MIT](LICENSE-MIT) / [LICENSE-APACHE](LICENSE-APACHE))。

| もの | 扱い | ライセンス |
|---|---|---|
| 学習済みモデル(basic-pitch / SwiftF0 / Beat This!) | 同梱([出典](crates/glaux-ml/models/README.md)) | Apache-2.0 / MIT / MIT |
| 音色語の辞書(LAION-CLAP の言葉側から作成) | 同梱 | Apache-2.0 |
| LAION-CLAP の音声側モデル | 初めて使うときに取得 | Apache-2.0 |
| SoundFont・CLAP プラグイン・Demucs | 同梱しない(各自で入手) | それぞれによる |
| **ロゴ・アイコン** | `assets/` ほか | **上のライセンスの対象外** |

ロゴ・アイコン(Glaux のロゴ、フクロウの図柄、それらから作った画像)は、Glaux を紹介する目的以外で使ったり、改変したり、
自分の製品やフォークのロゴにしたりしないでください。フォークを配布するときは別の名前とロゴにしてください。
使い分けは [`assets/BRAND_GUIDE.md`](assets/BRAND_GUIDE.md) にあります。
