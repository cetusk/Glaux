# CLAUDE.md

このリポジトリは **Glaux** ― 「AI と共同作業できる、シンプルで軽量なデスクトップ DAW」です。
作業を始める前に必ず `docs/HANDOFF.md` を読んでください。設計判断とその理由がすべて書いてあります。

## 最重要の原則

1. **すべての編集は `glaux_core::Command` を通す。** UI からでも MCP からでも、`Project` を直接いじらない。
2. **コマンドは決定的。** 新規 ID(`TrackId::new()` など)はコマンドを作る側が生成し、`apply` の内部で生成しない。
3. **コマンドは絶対値。** 「+480 tick」のような相対操作は UI/MCP 層で絶対値に変換してから `Command` にする。
4. **オーディオスレッドではアロケーション・ロック・ブロッキング I/O をしない。** `glaux-engine` を書くときの絶対条件。
5. **`glaux-core` はオーディオにも UI にも依存させない。** テスト可能性と MCP からの直接利用のため。

## クレート構成(予定含む)

```
crates/
  glaux-core/    [済] プロジェクトモデル、Command、apply(逆コマンド)、Session(git ライク履歴)
  glaux-mcp/     [済(第1段階)] MCP サーバー(stdio)。get_project/apply_commands/undo/redo/checkpoint/revert_to/get_history
  glaux-engine/  [済(MVP)] cpal 出力、RT セーフレンダラ(内蔵サイン波ポリシンセ)、
                 Project→再生データ展開、再生/停止/シーク、volume/pan/mute/solo/master 反映
  glaux-dsp/     [済] 内蔵楽器 subtractive(PolyBLEP+SVF+ADSR)/ drum(GM 配置ドラムシンセ)、
                 内蔵エフェクト eq / compressor / reverb(トラック・マスター両対応)、
                 ParamSpec レジストリ(聴感説明付き、MCP list_params の実体)
app/           [済(第2段階)] Tauri + Svelte 5。タイムライン/履歴の表示、undo/redo、
               アプリ内 HTTP MCP サーバー(127.0.0.1:41920/mcp、UI と同じ Session を共有)、
               AI 作業インジケータ、チャットパネル(ヘッドレス claude を起動して指示)
```

## 変更時のルール

- `Command` を追加・変更したら:
  - `apply.rs` に適用と **逆コマンド** を実装
  - `command.rs` の `targets()` に対象 ID を追加
  - `tests/history.rs` の `random_command` に生成パターンを追加(可逆性テストが自動で検証する)
  - `docs/HANDOFF.md` のコマンド一覧を更新
- `Project` のスキーマを変えたら `tests/schema.rs` の FIXTURE を更新し、`FORMAT_VERSION` を上げるか判断する
- `cargo test` が通らない状態でコミットしない
- 浮動小数を JSON に書く箇所では `serde_json` の `float_roundtrip` が有効であることに依存している(外さない)

## 命名規則

| 対象 | 名前 |
|---|---|
| プロダクト名 | **Glaux**(表記は常に頭文字大文字。ギリシャ語 γλαύξ = フクロウ) |
| リポジトリ | `Glaux` |
| クレート | `glaux-core` / `glaux-mcp` / `glaux-engine` / `glaux-dsp`(Rust 識別子は `glaux_core` など) |
| Tauri アプリ | `app/`、バンドル識別子 `dev.glaux.app`(ドメイン取得状況で変更可) |
| プロジェクトファイル | フォルダ形式 `MySong.glaux/`(中に `project.json`, `history.jsonl`, `audio/`, `cache/`) |
| `project.json` の `format` | `"glaux"` |
| MCP サーバー名 | `glaux`(`claude mcp add glaux -- ...`) |
| CLI バイナリ | `glaux`(将来。今は `glaux-mcp` のみ) |
| 環境変数 | `GLAUX_*` |
| ログ・設定ディレクトリ | `~/.config/glaux/`(OS 標準の設定ディレクトリ配下) |

`daw` という語は「DAW というジャンル」を指すときだけ使い、識別子には使わない。

## コーディング規約

- Rust 2021、MSRV 1.75(`Cargo.toml` の `rust-version`)。例外: `glaux-mcp` は rmcp が 1.88 を要求するため個別に `rust-version = "1.88"`
- エラーは `thiserror`、`unwrap()` はテストとロールバック(失敗しない前提の箇所)以外で使わない
- ドキュメントコメントは日本語でよい。AI 向け説明文(`ParamSpec::description`)も日本語
- 新しい依存を足すときは `[workspace.dependencies]` に置く

## 確認コマンド

```
cargo test --workspace
cargo clippy --workspace --all-targets   # 導入されていれば
```
