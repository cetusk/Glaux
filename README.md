<p align="center"><img src="docs/images/glaux-logo.png" alt="Glaux" width="480"></p>

# Glaux: A Lightweight DAW for Making Music with AI

**AI と共同作業できる、シンプルで軽量なデスクトップ DAW**(Rust + Tauri + Svelte 5)

名前はギリシャ語のフクロウ **γλαύξ**(グラウクス)から。アテナの肩に乗る知恵のフクロウであり、語源は「輝く」にも通じます。

> Your wise co-writer — Music, within reach.

> [!NOTE]
> 個人で開発している実験的なプロジェクトです。主に **Windows 11** で開発・確認しています(Linux はコア・エンジンのビルドとテストのみ、macOS は未確認)。
> 仕様やプロジェクトファイルの形式は予告なく変わることがあります。

## なにができるか

- **AI との共同作曲**: アプリ内チャットで「4 小節のベースラインを作って」「サビだけ盛り上げて」と頼むと、AI がプロジェクトを直接編集します。編集はタイムラインにリアルタイムで反映されます。相手は **Claude**(Claude Code)と **GPT**(Codex CLI)から選べます
- **AI の感覚**: AI は編集結果を `analyze_audio` で「聴き」(LUFS・帯域バランス・クリップ検出)、`analyze_harmony` でキーとコード進行を、`analyze_rhythm` でスウィングやグルーヴを、`analyze_sound` で音色を言葉(「明るい」「金属的」など)で把握します。まとまった編集の後は自分でセルフレビューしてから報告します
- **音源 7 種**: subtractive(シンセ)/ fm(FM: エレピ・ベル)/ wavetable(ウェーブテーブル)/ drum(ドラムシンセ)/ pluck(撥弦の物理モデル。+amp でエレキ)/ sampler(WAV ワンショット)/ **sf2(SoundFont)** — FluidR3_GM などのフリー SoundFont を 1 ファイル置けば GM 全 128 楽器が鳴ります
- **エフェクト 9 種**: eq / compressor / reverb / distortion / **amp(ギターアンプシミュ)** / sidechain(EDM のポンピング)/ delay / chorus / tape(Lo-fi)。音量・パン・音色パラメータ(フィルタスイープ等)のオートメーション、途中のテンポ・拍子変更にも対応
- **人間の編集**: ピアノロール(複数選択・コピペ・3 連符・奏法 M/S/A/V/B・**2 ペイン分割**)、ドラムキット図・**ギター/ベースのフレット盤**からの打ち込み、クリップのドラッグ移動/リサイズ、音作りビュー(つまみ・エフェクトチェーン・プリセット)、ループ再生、小節範囲を指定した AI への指示(マスク)
- **音声素材**: ⏺ で演奏を録音して音声トラックに配置(波形表示付き)、WAV をクリップとして配置。**鼻歌を録音 → ♪ で即 MIDI 化**(単旋律の譜起こし。ピアノ・ギターの和音は ♫)して、AI に「キーを確認して整えて」と続けられます
- **パート分離**: 音声クリップの右クリックで「打楽器 / 音程楽器」(内蔵)や「ボーカル / ドラム / ベース / その他」([Demucs](https://github.com/facebookresearch/demucs) を `pip install demucs` しておくと使えます)に分けて、パートごとのトラックに置けます
- **テンポ追従**: 音声クリップを「テンポに追従」にすると、曲のテンポを変えても音程を保ったまま伸縮して拍がずれません
- **CLAP プラグイン**: Surge XT・Vital・TAL-NoiseMaker などの CLAP 版シンセをトラックの音源にできます(音源メニューの「🔌 CLAP プラグイン」)。🖥 でプラグインの画面を開いて音作りでき、設定はプロジェクトに保存されます(画面は今は Windows のみ)
- **MIDI キーボード**: 設定で MIDI 入力を選ぶと、トラックの音色・エフェクトでそのまま弾けます(サステインペダル対応)。トラック見出しの 🎹 をオンにすると ⏺ が MIDI 録音になり、弾いた内容がクリップとして置かれます
- **奏法(アーティキュレーション)**: ブリッジミュート・スタッカート・アクセント・ビブラート・チョーキングをノート単位で。楽器ごとに効き方が変わります。さらに自由な連続ピッチカーブ(ゆっくりしたチョーキング・ダイブ・ポルタメント)も AI に頼めます
- **音色の資産化**: 良い音はプリセットとして全プロジェクト共通のライブラリに保存(SoundLab テンプレートで音作り専用セッションも 1 クリック)
- **双方向のキャッチアップ**: 人間の編集も AI の編集も同じ履歴に author 付きで記録され、AI は次のターンで人間の変更を自動で把握します。Undo/Redo・チェックポイント・履歴パネルからの個別取り消し(`git revert` 相当)にも対応。履歴は長くなると自動で圧縮されます
- **再生と書き出し**: RT セーフな内蔵エンジンで再生し、WAV に書き出せます
- **ゲームで鳴らす(Godot 4)**: Glaux の曲をゲームの中でそのまま鳴らし、敵の動きや入力判定を拍・マーカー・特定の音に同期できます。効果音を BGM のコードに合わせた音程で鳴らすこともできます([`docs/GODOT.md`](docs/GODOT.md))

## なぜ AI と相性が良いのか

既存の DAW はバイナリ/独自形式で、AI を後付けしにくい。Glaux は最初から AI を前提に設計しています:

- プロジェクトは**人間が読める JSON**(`project.json`)+ **author 付きコマンドログ**(`history.jsonl`)
- すべての編集(UI も AI も)が同じ **Command API** を通る。逆コマンド方式で完全な Undo が保証される
- **MCP サーバー**(stdio / HTTP)を内蔵し、Claude などの MCP クライアントから直接操作できる

## アーキテクチャ

```
crates/
  glaux-core    プロジェクトモデル・Command・Git ライクな履歴・和声/リズム分析(依存最小の純データ層)
  glaux-mcp     MCP サーバー(36 ツール)+ Session アクター + プリセット/アセット管理・履歴 compaction
  glaux-ml      学習済みモデルの推論(譜起こし・音程・拍・音色語。tract による pure Rust 推論)
  glaux-engine  リアルタイムオーディオ(cpal)・音声クリップ・録音・MIDI 入力(midir)・WAV 書き出し・音声解析・SoundFont 読み込み
  glaux-dsp     内蔵楽器 7 種 + エフェクト 9 種 + 奏法(すべて RT セーフ・聴感説明付き)
  glaux-clap    CLAP プラグインのホスト(clack-host)
  glaux-godot   Godot 4.3+ の拡張(gdext)。GlauxPlayer ノード
app/            Tauri + Svelte 5 のデスクトップアプリ(アプリ内 MCP・チャット同梱)
godot/          Godot のデモプロジェクトと、ゲームへ配るアドオン一式を作るスクリプト
```

- 時間は整数 Tick(PPQ=960)。オーディオスレッドはアロケーション・ロックなし
- AI ができること(感覚・操作・奏法)と、今の機能で作れる曲のジャンルは [`docs/CAPABILITIES.md`](docs/CAPABILITIES.md) にまとめています

## 動かす(Windows)

必要なもの: [Rust](https://rustup.rs) 1.88+ / [Node.js](https://nodejs.org) LTS / WebView2(Windows 11 は同梱)

```bat
scripts\glaux-app.bat [C:\path\to\MySong.glaux]
```

初回はビルドに数分かかります。プロジェクトフォルダは無ければ自動作成されます。

チャット機能は、ホストにインストールしてログイン済みの CLI を使います。チャットパネルの左上で相手を切り替えます。

| 相手 | 使う CLI | 準備 |
|---|---|---|
| Claude | [Claude Code](https://claude.com/claude-code)(`claude`) | インストールしてログイン |
| GPT | [Codex CLI](https://github.com/openai/codex)(`codex`) | `npm i -g @openai/codex` の後、`codex login` |

- どちらも、アプリ内の MCP サーバーの Glaux のツールだけを使います(ファイルの読み書きやコマンドの実行はさせません)
- 相手を切り替えると新しい会話になります。モデルは相手ごとに選べます(GPT は既定・GPT-6 Astra・任意のモデル名)
- GPT のときは `~/.codex/config.toml` を読みません(ほかの MCP サーバーや承認の設定が混ざらないように)。ログインの情報は使います

### リリースビルド(どこでも起動できる exe)

```bat
scripts\build-release.bat
```

リポジトリの `release\` に次ができます(初回は数分):

| ファイル | 使い方 |
|---|---|
| `Glaux.exe` | そのまま動く単体の exe。好きな場所(デスクトップ・USB メモリ等)にコピーしてダブルクリック。リポジトリも Node.js も不要 |
| `Glaux_<版>_x64-setup.exe` | インストーラー。ユーザー単位でインストール(管理者権限不要)し、スタートメニューから起動できるようにする。アンインストールは Windows の「アプリ」から |

- 動かすのに必要なのは WebView2 ランタイムだけです(Windows 10 / 11 には入っています)
- 引数なしで起動すると `<ホーム>\Music\GlauxDemo.glaux` を開きます(無ければ作成)。別の曲は画面のプロジェクトメニューから開くか、
  `Glaux.exe C:\path\to\MySong.glaux` のように引数で渡します
- 設定・プリセット・SoundFont・追加モデルは `%APPDATA%\glaux\` にあり、開発版(`glaux-app.bat`)と共有されます
- チャット機能を使うには、開発版と同じく `claude`(GPT なら `codex`)にパスが通っている必要があります
- 開発版とリリース版を同時に起動すると、2 つ目はアプリ内 MCP サーバーのポート(41920)を使えません。
  片方ずつ使うか、`GLAUX_MCP_PORT` で別のポートにしてください

### MCP クライアントから直接つなぐ

アプリ起動中は HTTP MCP サーバーが立っています:

```bat
claude mcp add --transport http glaux http://127.0.0.1:41920/mcp
```

Codex CLI(GPT)なら `~/.codex/config.toml` に次を書きます:

```toml
[mcp_servers.glaux]
url = "http://127.0.0.1:41920/mcp"
```

アプリなしで単体検証する場合は stdio 版もあります:

```bat
claude mcp add glaux -- cmd /c "<repo>\scripts\glaux-mcp.bat" "C:\path\to\MySong.glaux"
```

公開ツール(36):

| 分類 | ツール |
|---|---|
| 基本 | `get_project` / `apply_commands` / `undo` / `redo` / `checkpoint` / `revert_to` / `revert` / `get_history` / `list_params` |
| 分析 | `analyze_audio` / `analyze_harmony` / `analyze_rhythm` / `analyze_beats` / `analyze_sound` / `compare_sounds` / `match_sound` |
| ノート | `transpose_notes` / `shift_notes` / `swing_notes` / `quantize_notes` / `scale_velocity` |
| 音色 | `list_presets` / `save_preset` / `load_preset` / `delete_preset` / `find_similar_presets` / `list_soundfonts` / `set_soundfont_instrument` |
| CLAP | `list_plugins` / `list_plugin_presets` / `load_plugin_preset` / `refine_plugin_params` |
| 素材 | `import_sample` / `import_audio_clip` / `transcribe_audio` / `separate_audio` |

## ゲームエンジン(Godot 4)で使う

`crates/glaux-godot` は Godot 4.3 以降の拡張です。`GlauxPlayer` ノードで `.glaux` の曲を鳴らし、「いま聞こえている位置」で
拍・マーカー・ノートをシグナルとして受け取れます。ビルドと配布物の作り方は [`docs/GODOT.md`](docs/GODOT.md)、
ゲーム側での使い方はアドオンに同梱の [`README`](godot/demo/addons/glaux/README.md) /
[`AI_GUIDE`](godot/demo/addons/glaux/AI_GUIDE.md)(ゲーム側の AI 向け)を見てください。

## ブランド

ロゴ・アイコン・配色は `assets/` にあります。使い分け(背景ごとの版、余白、してはいけない加工)は
[`assets/BRAND_GUIDE.md`](assets/BRAND_GUIDE.md) を見てください。表記は **Glaux**(G のみ大文字)。

## 開発

```
cargo test --workspace
cargo clippy --workspace --all-targets
```

Linux でもコア・エンジンのビルドとテストは可能です(ALSA ヘッダが必要。アプリのビルドには webkit2gtk 等)。

ドキュメント・コメント・コミットメッセージは日本語です。AI(Claude Code)と共同で開発しています。

## ステータス

活発に開発中の実験プロジェクトです。

## 同梱しているもの・別途入手するもの

| もの | 扱い | ライセンス |
|---|---|---|
| 学習済みモデル(basic-pitch / SwiftF0 / Beat This!) | `crates/glaux-ml/models/` に同梱し、実行ファイルに埋め込む。出典は同フォルダの [`README.md`](crates/glaux-ml/models/README.md) | Apache-2.0 / MIT / MIT |
| 音色語の辞書 `crates/glaux-ml/data/clap_vocab.json` | [LAION-CLAP](https://huggingface.co/laion/larger_clap_music_and_speech) の言葉側の埋め込みから作ったもの(`scripts/clap_vocab.py`) | Apache-2.0(元のモデル) |
| LAION-CLAP の音声側モデル | 同梱しない。音色の分析を初めて使うときに Hugging Face から取得 | Apache-2.0 |
| SoundFont(FluidR3_GM など) | 同梱しない。各自で入手して置く | 各 SoundFont による |
| CLAP プラグイン(Surge XT など) | 同梱しない。各自でインストールしたものを公開の CLAP API で読み込む | 各プラグインによる |
| Demucs(パート分離) | 同梱しない。使う場合は各自で `pip install demucs` | MIT |

## ライセンス

ソースコードとドキュメントは MIT または Apache-2.0 のデュアルライセンスです([LICENSE-MIT](LICENSE-MIT) / [LICENSE-APACHE](LICENSE-APACHE))。

**ロゴ・アイコン(Glaux の名前のロゴ、フクロウの図柄、およびそれらから作った画像)はこのライセンスの対象外です。**
対象は `assets/` の全ファイル、`docs/images/glaux-logo.png`、`app/public/` の画像、`app/src-tauri/icons/`、
`godot/demo/icon.png`、`godot/demo/addons/glaux/glaux_player.png` です。Glaux そのものを指し示す目的(紹介記事・リンクなど)
以外で使ったり、改変したり、自分の製品やフォークのロゴとして使ったりしないでください。
フォークを配布するときは、別の名前とロゴに置き換えてください。
