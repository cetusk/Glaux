# 引き継ぎ文書(HANDOFF)

作成日: 2026-09-21(最終更新: 2026-09-23)

関連文書: AI ができること(感覚・操作・奏法)と作れる曲のジャンルの整理は [`CAPABILITIES.md`](CAPABILITIES.md)。
音を分析する能力の強化に向けた研究・ツールの調査は [`AUDIO_ANALYSIS_RESEARCH.md`](AUDIO_ANALYSIS_RESEARCH.md)。
状態(2026-09-22 時点): 主要 4 クレート + アプリがすべて動作し、Windows 実機で確認済み。
- `glaux-core`: モデル(セクション・奏法込み)/ Command(約 25 種)/ 履歴 /
  和声分析(harmony)/ リズム分析(rhythm)
- `glaux-ml`: 学習済みモデルの推論(basic-pitch による和音の譜起こし、tract)
- `glaux-clap`: CLAP プラグインのホスト(探索・生成・process・状態・Windows の画面)
- `glaux-mcp`: **29 ツール** = 基本 10(get_project / apply_commands / undo / redo / checkpoint /
  revert_to / revert / get_history / list_params / analyze_audio)+ 分析 2(analyze_harmony /
  analyze_rhythm)+ ノート便利 4(transpose / shift / quantize / scale_velocity)+
  プリセット 4(list / save / load / delete)+ 素材 6(import_sample / import_audio_clip /
  transcribe_audio / separate_audio / list_soundfonts / set_soundfont_instrument)+ list_plugins /
  list_plugin_presets / load_plugin_preset。履歴は 3000 件超で自動 compactionstdio 単体 + アプリ内 HTTP の両対応。
  ほかに presets / assets モジュール(アプリと共用)
- `glaux-engine`: 再生(ループ・オートメーション・音声クリップ・自動停止・テンポ変更時の
  位置保持)・録音(record.rs)・WAV エクスポート・音声解析(AI の耳)・
  SoundFont 読み込み(sf2.rs、フィルタ/LFO 込み)・サンプルキャッシュ(SampleBank)
- `glaux-dsp`: 楽器 7 種(subtractive / drum / pluck / sampler / sf2 / fm / wavetable)+
  エフェクト 9 種(eq / compressor / reverb / distortion / amp / sidechain / delay / chorus / tape)+
  奏法 5 種(楽器別カタログ)+ ピッチ表現(expr: 奏法 + 連続ピッチカーブ)
- `app/`: タイムライン(セクション・拍子対応グリッド)・ピアノロール(奏法・3 連・
  フレット盤・ドラムキット)・音作りビュー・プリセット/SoundFont UI・ループ再生・
  プロジェクト管理(作成/切替/移動/SoundLab)・履歴・チャット・WAV 書き出し
実機確認済みのハイライト: AI がチャット指示で作曲 → analyze_audio/harmony/rhythm で
自己確認 → エフェクト・プリセット・SoundFont で音作り、のループが完走。
AI の能力一覧は §7.5「感覚マップ」、今後の課題は §8 を参照。テストは 158 件。

この文書は、企画段階の議論で決めたことを **理由付きで** 残したものです。
判断を覆すときは、ここに書いてある理由を上回る根拠を示してください。

---

## 0. 名称: Glaux

ギリシャ語 γλαύξ(グラウクス)= フクロウ。アテナの肩に乗る知恵のフクロウで、語源は「輝く」(γλαυκός)にも通じる。

- アイコンはフクロウ(黄色い大きな目を最小限の線で。「輝く」に合わせて目の光をモチーフに)
- 込めた意味: 知恵(AI)、ヒトと AI の協調、作りたい曲にすぐ手が届くユートピア
- 他候補との比較: Chouette(仏語フクロウ+「いいね!」)、Athene(属名+女神)、Nocturne、Owlet、Parliament(フクロウの群れ)など。短さ・被りにくさ・技術製品らしい響きで Glaux を採用
- 商標・既存製品: 2026-09 時点で音楽・オーディオ分野に同名製品なし。別分野に小規模なもの(Python ライブラリ、古代ギリシャ語コーパス GLAUx、EIP-7702 ウォレット、学術出版サービス)があるのみ。GitHub ユーザー名 `glaux` は取得済みなので org/リポジトリ名は `Glaux` または `glaux-daw`
- タグライン案: "Your wise co-writer" / "Music, within reach"
- 命名規則の一覧は `CLAUDE.md` を参照

## 1. プロダクトのコンセプト

6 つの柱(優先順位あり):

1. **AI との共同作業が可能&互いに編集しやすい** ← 最大の差別化点。既存 DAW はバイナリ/独自形式で AI を後付けしにくい
2. **AI とデータのやり取りをしやすいデータ構造**
3. **シンプルで誰もが使いやすい UI**
4. **高速かつ軽量**
5. **音作りが可能**
6. **波形の編集が可能**

### コンセプト間の緊張と解決方針

- **シンプル UI vs 音作り・波形編集**: 段階的開示(最初は最小限、必要なときに詳細を開く)。
  **AI を「複雑さを吸収する層」として位置づける**。「もっと温かい音に」と言えば AI がパラメータを動かす。
- **高速・軽量 vs AI 連携**: AI 処理(LLM 呼び出し)は遅く不安定なので、音声処理経路から完全に切り離す。
  AI は「プロジェクトデータを編集する外部の協力者」として扱う。

### スコープ

- **MIDI が主軸**。音声録音・インポートは後から追加するが、設計上は最初から対応可能にしておく
- **外部音源(CLAP プラグイン)**: 音源プラグインは 2026-09-23 に第 1 段階を実装(`glaux-clap`)。エフェクトプラグインと AI からのつまみ操作は第 2 段階
- **デスクトップアプリ**(ブラウザ版は将来、Rust コアを WASM 化して「共有・軽作業用」として出す可能性あり)

---

## 2. 技術選定と理由

| 選択 | 却下した案 | 理由 |
|---|---|---|
| **Rust**(コア) | Nim | リアルタイム安全性をコンパイラが保証する(`Send`/`Sync`)。オーディオ系クレートの厚み(cpal, nih-plug, clack, symphonia, fundsp)。Tauri と同一プロセスで完結。AI に相談したときの回答品質(学習データ量)。Nim は DSP の書きやすさとビルド速度で勝るが、エコシステムと Tauri 統合で劣る |
| **Tauri**(UI 基盤) | Electron, egui/iced | Electron より圧倒的に軽い。UI を Web 技術で作り込める。egui/iced は速いが「シンプルな UI」の作り込みで表現力が劣る |
| **Svelte 5 or SolidJS**(フロント) | React | DAW は状態が細かく頻繁に更新されるので、細粒度リアクティビティが有利。**未決定、着手時に決める** |
| **Canvas/WebGL で自前描画**(波形・ピアノロール) | DOM 要素 | DOM を大量に並べると必ず重い |
| **CLAP**(プラグイン規格、将来) | VST3 | C ABI がシンプルで Rust バインディングが整っている。VST3 は後から追加可能 |
| **MCP サーバー**(AI 連携) | 独自 API | Claude などの既存 AI クライアントから直接操作できる。「AI 共同作業」の間口が広がる |
| **AI はサイドカー(別プロセス)** | 同一プロセス | AI 側がクラッシュしても音が止まらない。モデル差し替えが自由 |

### プロセス・スレッド構成

```
┌─────────────────────────────────────────┐
│ Tauri アプリ                              │
│  UIスレッド(Web)  ──Command──▶ ┌────────┐ │
│                               │Session │ │
│  MCPサーバー  ──────Command───▶ │(glaux-core)│
│  (AI から)                     └───┬────┘ │
│                                   │ Change │
│                          ロックフリーキュー  │
│                                   ▼       │
│                          オーディオスレッド   │
│                          (glaux-engine, RT)   │
└─────────────────────────────────────────┘
```

- 人間も AI も **同じ `Command` API** を通って `Session` を変更する
- オーディオスレッドは `Project` を読まない。`Change` 通知と、エンジン用に変換した再生データの `Arc` を受け取るだけ

---

## 3. データ設計(決定事項)

### ファイル形式: フォルダ

```
MySong.glaux/
  project.json      ← 構造(トラック、クリップ、パラメータ)。人間が読め、Git で差分が取れる
  history.jsonl     ← コマンドログ(1行1エントリ、author 付き)
  audio/
    <hash>.wav      ← 内容ハッシュ名で参照
  cache/            ← 波形ピーク、解析結果(再生成可能、Git 管理外)
```

### 時間

- **Tick(整数、PPQ=960)が正**。`Tick(3840)` = 1小節@4/4
- 音声クリップも Tick 上に置く。秒への変換は `TempoMap::tick_to_seconds`
- `bar:beat:tick` 表記は UI 側で計算。ファイルには書かない
- 理由: 整数なので誤差ゼロ、テンポ変更でノート位置が動かない、AI も人間も読める

### ID

- `trk_xxxxxx` / `clp_xxxxxx` / `nt_xxxxxx` / `fx_xxxxxx` / `hst_xxxxxx`(種別プレフィックス + base36 6桁)
- アセットは `sha256:<hex>`(内容ハッシュ)
- Rust 上は newtype で型が分かれる(`TrackId` を `ClipId` の引数に渡せない)
- **ノートにも ID を付ける**。AI が「このノートを半音上げて」と指定しやすくなる
- 名前変更で ID は変わらない

### パラメータ

- **ファイルには値だけ**(`"filter.cutoff": 800.0`)。単位・範囲・説明は `ParamSpec` としてデバイス定義(Rust コード)が持ち、MCP の `list_params` で返す
- 理由: 全パラメータに意味情報を書くとファイルが肥大化し、AI に渡すときも冗長
- `ParamSpec::description` は AI 向けに **聴感上の効果** を書く(「下げると音がこもり、上げると明るくなる」)。AI の操作精度がここで決まる
- ファイルにはデフォルトと異なる値だけ書く方針(なので `SetParam` は未設定キーに書けて、逆は `UnsetParam`)
- `ParamValue` は untagged: JSON の `800` は `Int`、`800.0` は `Float`。読み側は `as_f64()` を使う

### クリップ

- `"kind": "midi"` / `"kind": "audio"` の明示タグ(`notes` の有無で判別する案は却下: serde 的に脆く、AI にも不明瞭)
- 音声は **非破壊編集**: 元音声は変えず `offset_samples` / `gain_db` / `fade_in_ms` / `fade_out_ms` / `stretch` を重ねる
- `stretch` は `none` / `follow {original_bpm}`(テンポ追従、2026-09-23 実装。MIDI 主軸の DAW で音声を入れたとき最初に欲しくなる機能)
- オートメーションはトラック単位(クリップ単位は必要になったら追加)

### 楽器・エフェクト

- `PluginSource` で `builtin` / `clap` / `sampler` / `sf2` を切り替える(clap は音源のみ実装済み)
- CLAP の内部状態は `state`(base64、不透明)。ただし CLAP はパラメータを ID 付きで公開するので、主要なものは `params` に写して AI から触れるようにする
- 将来: シンセ/エフェクトをノードグラフで表現する案あり(モジュラー的接続。AI がパッチを組める)。UI ではプリセット+主要つまみ数個から始め、上級者だけグラフを開く

### 不変条件(コードが保証する)

- `Track.clips` は `(start, id)` 昇順、MIDI ノートは `(pos, pitch, id)` 昇順に常に保たれる
  → 挿入位置の管理が不要になり、逆コマンドが単純になる。トラックとエフェクトはユーザーが順序を決めるので index 付き
- `apply` が失敗したときプロジェクトは一切変更されない(`Batch` は巻き戻す)
- 任意のコマンドで `apply(cmd)` → `apply(inverse)` は完全に元に戻る
- JSON 往復で `Project` は完全一致(`serde_json` の `float_roundtrip` が必須。デフォルトでは最下位ビットがずれてリプレイテストが落ちた)

---

## 4. コマンドと履歴(決定事項)

### なぜ逆コマンド方式か

Undo の方式は 2 案あった:
- A. スナップショット方式(`im` クレートで `Project` 丸ごと保存)
- B. **逆コマンド方式**(採用): `apply` が「元に戻すコマンド」を返し、それを積む

B の理由: `history.jsonl` にそのまま書ける / AI の変更を選択的に取り消せる / エンジンへの差分通知にも使える。

### コマンド設計の約束

- **決定的**: 新規 ID はコマンドを作る側が渡す(`SplitClip { new_id }`)。リプレイで同じ結果になり、AI が ID を予測できる
- **絶対値**: 相対操作は持たない。逆コマンドが自明に作れる。相対→絶対の変換は UI/MCP 層
- **`Batch` が Undo の単位**。AI が「4小節の伴奏」で数百コマンドを 1 回で戻せる
- `SplitClip` の逆は `Batch[RemoveClip{new}, ReplaceClip{原本}]`(分割時にノートを切り詰めるので、完全復元には原本が要る)
- `SplitClip`: 分割点をまたぐ MIDI ノートは左側で切り詰める(右にコピーしない。コピーすると新 ID が要り非決定的になる)

### Git ライクな履歴(`Session`)

AI との作業は Git の発想と親和性が高い、という合意のもとで設計した。

| 操作 | Git 対応 | 用途 |
|---|---|---|
| `undo` / `redo` | reset(redo 可) | 直前の取り消し |
| `checkpoint(label)` / `revert_to(label)` | tag + reset | **AI の試行錯誤の足場**。「ベースライン作る→解析→気に入らなければ戻す→別案」 |
| `revert(entry_id)` | `git revert` | 履歴の **途中** のエントリだけ取り消す。逆コマンドを新エントリとして末尾に積む。取り消した事実も履歴に残る |
| `by_author(...)` | `git log --author` | AI の作業一覧。「AI の変更をハイライトして個別に却下」の UI に使う |
| `replay` | clone | `history.jsonl` から再構築 |

`revert` の衝突検出は **対象 ID の重なり** を見るだけ(`HistoryEntry.targets`)。厳密な依存解析はしない。
後続エントリが同じ対象を触っていれば `conflicts` に返す。`apply` が `Err` になる衝突は自動検出できるが、
「成功はするが意図しない結果」は検出できないので、この警告で補う。

AI にとってのもう一つの利点: 履歴がプロジェクト側にあるので、**AI は自分の過去の操作を `get_history(author=ai)` で振り返れる**。
会話コンテキストではなくプロジェクトが状態を持つので、セッションをまたいでも継続できる。

### 現在の `Command` 一覧

| op | 逆 | 備考 |
|---|---|---|
| `add_track {track, index?}` | `remove_track` | |
| `remove_track {id}` | `add_track` (index 付き) | |
| `set_track_prop {id, prop, value}` | 同 (旧値) | prop: name/color/mute/solo/volume_db/pan |
| `move_track {id, to_index}` | 同 (旧 index) | |
| `add_clip {track, clip}` | `remove_clip` | kind とトラック kind が一致すること |
| `remove_clip {id}` | `add_clip` | |
| `replace_clip {id, clip}` | 同 (旧 clip) | |
| `move_clip {id, start, track?}` | 同 | トラック間移動は kind 一致が必要 |
| `resize_clip {id, length}` | 同 | length > 0 |
| `add_master_effect {effect, index?}` | `remove_effect` | マスターバスにエフェクト(2026-09-23)。`remove_effect` / `set_effect_bypass` はマスターのエフェクトも対象 |
| `set_master_param {path, value}` / `unset_master_param {path}` | 同 / 逆 | マスターのエフェクトのつまみ(path は `fx/<id>/<name>`) |
| `set_clip_loop {id, loop_len}` | 同(元の loop_len) | MIDI クリップのループ。loop_len(繰り返す長さ、クリップ先頭から)で ON、null で OFF。再生・分析は `Clip::playback_notes` で展開 |
| `split_clip {id, at, new_id}` | `batch[remove_clip, replace_clip]` | 音声は `offset_samples` をテンポマップから計算 |
| `add_notes {clip, notes}` | `remove_notes` | pitch/vel ≤ 127。`Note.articulation`(palm_mute / staccato / accent、省略で normal)対応 |
| `remove_notes {clip, ids}` | `add_notes` | |
| `update_notes {clip, changes}` | 同 (旧値、逆順) | `NoteChange` は Option フィールドの部分更新(articulation 含む) |
| `set_param {track, path, value}` | 同 or `unset_param` | path: `device/<n>`, `fx/<id>/<n>`, `track/volume_db|pan` |
| `unset_param {track, path}` | `set_param` | |
| `set_device {track, device?}` | 同 | |
| `add_effect {track, effect, index?}` | `remove_effect` | |
| `remove_effect {id}` | `add_effect` | |
| `set_effect_bypass {id, bypass}` | 同 | |
| `set_send {track, target, level_db?, pre_fader?}` | 同(前のセンド。無ければ外す) | センド。送り元はバス以外、送り先はバス。level_db -60〜12、省略で外す(2026-09-24) |
| `set_effect_state {id, state?}` | 同(前の状態) | CLAP エフェクトの状態(base64)。トラック・マスター両方。内蔵エフェクトにはエラー(2026-09-24) |
| `set_automation_points {track, target, points}` | 同 | 空配列でレーン削除。レーンは対象パスの文字列順に挿入(削除 → 逆で元の並びに戻るよう正規化) |
| `set_clip_stretch {id, stretch}` | 同(旧値) | 音声クリップのテンポ追従(2026-09-23)。`{mode: follow, original_bpm}`(20〜400)/ `{mode: none}`。音声クリップ以外はエラー |
| `set_master_automation_points {target, points}` | 同 | マスターのレーン(2026-09-23)。target は `track/volume_db` か `fx/<マスターのエフェクト ID>/<名前>`。`MasterBus.automation` に保存 |
| `set_tempo {events}` | 同 | tick 0 から始まる昇順 |
| `set_time_sig {events}` | 同 | |
| `set_master_volume {volume_db}` | 同 | |
| `set_title {title}` | 同(旧タイトル) | meta.title の変更。プロジェクトの移動/名前変更 UI からも system author で使う |
| `add_asset {id, asset}` / `remove_asset {id}` | 互いに | |
| `batch {commands, label}` | `batch` (逆順) | 途中失敗で巻き戻し |

**未実装で必要になりそうなもの**: `move_effect`、クリップ単位オートメーション、
`set_clip_prop`(name/loop/gain/fade を個別に変える。今は `replace_clip` で代替)、トラックのグループ/バス。

---

## 5. 今後のロードマップ

### 「最初の 2 週間」計画(当初案)

1. `cpal` でサイン波を鳴らし、UI のボタンで ON/OFF できる Tauri アプリ(スレッド間通信の型を確立)
2. `project.json` のスキーマ → コマンド → Undo/Redo ← **済(glaux-core)**
3. WAV を読み込んでタイムライン上で再生
4. MCP サーバーを立てて、Claude からトラック追加やボリューム変更 ← **次**

4 まで到達すればコンセプトの核が全部つながった状態で検証できる。
**MCP を先にやる**のがおすすめ(コンセプトの核を早く検証できる。エンジンなしでもプロジェクト編集は検証可能)。

### `crates/glaux-mcp`(第 1 段階 済)

`docs/mcp-spec-draft.md` が仕様。実装済みの範囲と判断:

- **rmcp 3.4**(Rust 公式 SDK)+ stdio トランスポート。`claude mcp add glaux -- <path>/glaux-mcp <MySong.glaux>` で接続できる
  - 注意: rmcp は MSRV 1.88 を要求するので `glaux-mcp` だけ `rust-version = "1.88"`(glaux-core は 1.75 のまま)
- **アクター方式を採用**: `actor::SessionHandle`。専用スレッドが `Session` + `Store` を所有し、
  tokio mpsc でリクエストを受ける。Tauri UI も将来同じハンドルを使う
- 実装済みツール: `get_project`(track_ids / include_notes / include_automation フィルタ付き。
  include_notes: false で notes を空にし note_count を付ける)/ `apply_commands`(複数コマンドは Batch 化、
  author はクライアント名から `Ai { model }`)/ `undo` / `redo` / `checkpoint` / `revert_to` / `get_history`
  (author / since / limit フィルタ。forward/inverse を含まない軽量ビュー)
- すべてのレスポンスに `project_version`(適用済み履歴エントリ数)を含める
- 永続化: 変更のたびに `project.json` + `history.jsonl` を temp+rename で全書き。
  起動時は `history.jsonl` からのリプレイを試み(過去セッションの undo が可能)、
  `project.json` と一致しなければ project.json を正として履歴を `history.jsonl.orphan` に退避
- Command の JSON は schema にせず `Vec<serde_json::Value>` で受けて serde でパース
  (internally-tagged + flatten は JSON Schema 化が困難。ツール description に形を書いた)
- MCP はリクエストを並行処理するので、クライアントが応答を待たず連投すると実行順は保証されない
  (アクターが状態は守る。通常の AI クライアントは逐次呼び出しなので問題にならない)

- **便利ツール実装済み(2026-09-22)**: `transpose_notes` / `shift_notes` / `quantize_notes` /
  `scale_velocity`。MCP 層で現在値を読んで絶対値の `UpdateNotes` **1 コマンド**に変換する
  (= 1 回の undo で戻せる)。note_ids 省略で全ノート、指定で部分編集。
  端に当たって丸めた場合は `clamped` を返して AI に知らせる。
  実質 no-op(全ノート変更なし)のときは履歴を汚さず `changed: 0` を返す

残り(未実装): `get_clip` / `get_history_entry` / `new_ids` / `render`(`revert(entry_id)` は実装済み 2026-09-22)

### `crates/glaux-engine`(MVP 済)

実装済み(2026-09-21):

- `cpal 0.18` で出力。`Stream` は `Send` でないため専用スレッド(`glaux-audio`)が保持し、
  UI には Send+Sync な `EngineHandle` を渡す
- 制御はアトミック + `arc-swap` のみ(playing / pos / seek + `ArcSwap<PlaybackData>`)。
  **レンダラ(`render.rs`)はアロケーション・ロックなし**。ボイスは固定 64。
  旧データの解放は `EngineHandle` 側の graveyard が引き受ける(オーディオスレッドで dealloc しない)
- `data.rs`: `Project` → ノートを**絶対サンプル**に展開・ソートした `PlaybackData`。
  UI スレッドで構築し、プロジェクト変更通知のたびに丸ごと作り直して差し替え
  (ParamChanged の軽量パスは未実装。全再構築で十分速いうちはこのまま)
- 音源は内蔵サイン波ポリシンセ 1 種(アタック 5ms / リリース 30ms)。device 未設定の
  トラックも鳴る。volume_db / pan(等パワー)/ mute / solo / master を反映
- 曲終端 + 1 秒で自動停止。アプリ側: 再生/一時停止(スペースキー)・停止・
  ルーラークリックでシーク・再生ヘッド表示(100ms ポーリング)
- オーディオデバイスが無い環境ではエンジンなしで起動を続ける(トランスポート無効表示)

MVP の割り切り(将来課題):

- **再生位置はサンプル保持**だが、`PlaybackData::tempo`(テンポ区間表)を使って
  データ差し替え時に tick を保ったまま換算し直すので、再生中のテンポ変更でずれない
  (2026-09-22)。tick⇔秒変換は `TempoMap` を使用
- 音声クリップは再生対応(2026-09-22。固定プール 16、線形補間、フェード、途中再開)。
  ループクリップは対応済み(2026-09-23)。テンポ追従(Stretch::Follow)も対応済み(2026-09-23)
- オートメーションは track/volume_db・track/pan(サンプル単位で補間)と
  device/<パラメータ>(音色。ブロックレート ≈ 数 ms で評価、
  `InstrumentParams::set_continuous` に raw 値を流し込む)、
  fx/<id>/<パラメータ>(2026-09-23。`EffectParams::set_continuous` がブロック頭で係数を計算し直し、
  レンダラのスロット別作業コピー `fx_scratch` に適用。EQ は生の値 `EqRaw` を保持して帯域の係数を
  再計算。バイパス中のエフェクトのレーンは鳴らさない)に対応。
  マスターも音量(`master_vol_auto`、サンプル単位)とエフェクト(`master_fx_auto`、同じ `fx_scratch`)に対応(2026-09-23)
- 発音中のデータ差し替えはボイスを切り直す(クリックノイズが出うる)

今後: `ParamChanged` の軽量差し替え(MIDI 入力は `midir` で実装済み、`midi.rs`)、
WAV 読み込みは `symphonia`、リサンプリングは `rubato`、書き出しは `hound`

### `crates/glaux-dsp`(楽器 済)

実装済み(2026-09-21)。`fundsp` は使わず自前(依存ゼロで RT 安全を確実にするため):

- **`subtractive`**: PolyBLEP オシレータ(saw/square/triangle/sine)→ SVF(TPT)ローパス → ADSR。
  フィルタエンベロープ付き。device 未設定トラックの既定音源(未知の builtin 名や、見つからない CLAP プラグインもこれで代用)
- **`drum`**: GM 配置のドラムシンセ(36=キック, 38=スネア, 39=クラップ, 42/46=ハット,
  41〜50=タム, 49/51=シンバル)。全合成・サンプル不使用。ノイズは xorshift32
- **ParamSpec レジストリ**(`params.rs`): 全パラメータに**聴感上の効果を書いた日本語説明**。
  これが MCP `list_params` の実体で、AI の音作り精度を決める。
  `bake_instrument(Option<&Device>)` が「spec デフォルト + ParamMap 上書き + clamp」で
  再生用構造体に焼き込む
- ボイス(`VoiceState`)はすべて Copy な値型。`next`/`note_off`/`finished` はアロケーションなし
- エンジン統合: `TrackMix::instrument` に焼き込み、レンダラは種別ディスパッチで発音。
  マスターに tanh ソフトクリップ。set_device / set_param は既存の変更通知でライブ反映
- MCP に `list_params` ツール追加(track_id 省略でカタログ、指定で spec + current)
- **エフェクト実装済み**(`glaux-dsp/src/effects.rs`): `eq`(3 バンド: 低/高シェルフ + 中域ピーク、
  RBJ biquad)/ `compressor`(エンベロープフォロワ + レシオ)/ `reverb`(Freeverb 系:
  コム 4 + オールパス 2、damping、ステレオスプレッド)。トラック・マスター両方に掛けられる。
  - パラメータは構築時に**係数まで焼き込み**(`bake_effect`)。オーディオスレッドは読むだけ
  - 状態はエンジンが起動時に固定プール(`MAX_EFFECT_SLOTS`=64。リバーブのバッファ込みで確保)。
    データ差し替えでスロットの種類が変わったときだけ `ensure_kind` でリセット(アロケーションなし)
  - 信号経路: 楽器合算(モノ)→ トラックのチェーン(ステレオ)→ 音量/パン → マスターの
    チェーン → マスター音量 → tanh。`MAX_TRACKS`=64 超のトラックはエフェクトなしで直行
  - `list_params` がエフェクトも返す(カタログ + トラックのチェーンの spec/current/path)。
    追加は `add_effect`、調整は `set_param`(`fx/<id>/<名前>`)、`set_effect_bypass` も既存
- **WAV エクスポート**(`glaux-engine/src/export.rs`): 再生と同じレンダラでオフラインレンダ
  (デバイス不要・実時間より速い)→ 16bit/48kHz ステレオ WAV。末尾の無音は自動で切り詰め。
  アプリの「⬇ WAV」ボタンで `<プロジェクト>/export/<title>_<日時>.wav` に書き出す
- **`analyze_audio`(AI の耳)実装済み**(`glaux-engine/src/analyze.rs` + MCP ツール):
  オフラインレンダ → 統合ラウドネス(ITU-R BS.1770 の K 特性 + ゲーティング。本物の LUFS)、
  peak/rms/クレストファクタ、スペクトル重心、帯域比(low<250Hz / mid / high>4kHz)、
  オンセット位置(tick)、クリップ検出。track_ids で単体トラック、start/end_tick で範囲を絞れる
  (UI の小節範囲マスクと併用できる)。FFT は rustfft、解析は常に 48kHz。
  ツール description に指標の読み方(-14 LUFS 目安、low>0.6 はこもり等)を記載。
  システムプロンプトにも「編集 → analyze_audio → 微調整のループを回せ」を明記。
  **per_track: true**(2026-09-22 追加)で各トラックをソロでレンダした要約
  (loudness / band_energy / 重心)をうるさい順の一覧で返す。「リードが埋もれる」等の
  **トラック間バランス診断**はこれを使う。ミキシングの定石(主役は伴奏より 2〜4dB 上、
  帯域の住み分け、音量より先に被り削り)を description とシステムプロンプトに記載。
  spec の estimated_tempo / key / chords は未実装(MIDI からの推定は AI 自身ができるため保留)
- 未実装: エフェクト(compressor / eq / reverb)、サンプラー、ノートオフ後のドラムのチョーク

### `app/`(Tauri)(第 1 段階 済)

- **フロント: Svelte 5 に決定**(runes。エコシステムと情報量で SolidJS より優位と判断)
- **アプリ内 HTTP MCP サーバーを採用**: Tauri アプリが `SessionHandle`(glaux-mcp のアクター)を所有し、
  同一プロセス内で rmcp の streamable HTTP サーバーを `http://127.0.0.1:41920/mcp` に立てる
  (ポートは `GLAUX_MCP_PORT` で変更可)。Claude Code からは
  `claude mcp add --transport http glaux http://127.0.0.1:41920/mcp`
- アクターの変更通知(`SessionHandle::subscribe`、tokio broadcast)を Tauri イベント
  `project-changed` に転送。UI はイベントを受けるたびに全体を取得し直す(粒度最適化は後回し)
- AI のツール呼び出し状況も通知する: `SessionHandle::begin_activity`(RAII ガード)を
  MCP の各ツールが呼び、`subscribe_activity` → Tauri イベント `ai-activity` → ヘッダーの
  「AI が編集しています…」インジケータ(紫のパルス)につながる。
  UI 側は呼び出しの合間も「AI が作業中です…」(控えめ表示)を継続し、最後の呼び出しから
  20 秒(`SESSION_LINGER_MS`)無通信で消灯する。「指示を出した瞬間」はアプリから観測できない
  ための近似で、正確な区間表示はアプリ内チャット(Agent SDK 統合)実装時に可能になる
- **注意(ハマりどころ)**: Tauri v2 はフロントの `listen()` 等に `src-tauri/capabilities/default.json`
  の権限(`core:default`)が必要。無いとイベント購読が黙って失敗し「リアルタイム反映されない」症状になる
- 実装済み UI: トラック一覧+タイムライン(小節ルーラー、クリップ配置、MIDI ノートの縮小プレビュー)、
  履歴パネル(author 別の色分け、AI の編集をハイライト)、undo/redo ボタン、
  MCP URL 表示(クリックで登録コマンドをコピー)
- プロジェクトパス: CLI 引数 → `GLAUX_PROJECT` 環境変数 → `<home>/Music/GlauxDemo.glaux`
- 起動: `scripts/glaux-app.bat [プロジェクトパス]`(省略時 `tests/live/TestSong.glaux`)。
  ホスト要件: Rust 1.88+、Node.js LTS、WebView2
- **注意: stdio 版(`glaux-mcp` バイナリ)とアプリを同じプロジェクトで同時に動かさない**。
  それぞれが独立に Session を持ち、後勝ちの上書きで編集が失われる。アプリ使用時は
  HTTP 登録に切り替え、stdio 登録は外すこと
- **アプリ内チャット(UI → AI 指示)実装済み**(`app/src-tauri/src/chat.rs` + `ChatPanel.svelte`):
  - 送信ごとにヘッドレス `claude -p <指示> --output-format stream-json` をホストで起動。
    認証はホストの Claude Code ログインを流用(API キー不要)
  - `--mcp-config` でこのアプリの HTTP MCP だけを渡し(`--strict-mcp-config`)、
    `--allowedTools mcp__glaux` で glaux ツールを自動許可。編集経路は従来どおり MCP →
    アクターなので、タイムライン反映・インジケータ・履歴ハイライトがそのまま機能する
  - 会話継続は `--resume <session_id>`(init イベントから取得)。「新しい会話」でリセット。
    session_id は `<プロジェクト>/cache/chat-session.txt` に保存され、**アプリ再起動をまたいで継続**する。
    保存済みセッションを claude 側で再開できなかった場合(履歴掃除等)は、自動で新しい会話に
    フォールバックして 1 回だけ再試行し、その旨をチャットに表示する
  - `--append-system-prompt` で「編集は必ず glaux MCP ツール経由、ファイル直接編集禁止」を指示
  - stdout の NDJSON は `chat::parse_line`(単体テストあり)で UI イベント化 →
    Tauri イベント `chat-event` → チャットパネル(発話・ツールチップ・エラー表示)
  - Windows では `claude.cmd` → `claude`(exe)の順で起動を試す。CREATE_NO_WINDOW 付き
- **UI からの編集**: Tauri コマンド `apply_edit`(Command JSON + label → author: human で適用)。
  現在つながっているのはマスター音量スライダー、トラックの音量スライダー・M/S ボタン、
  トラックヘッド右クリックメニュー(上へ/下へ移動・削除。2026-09-22 追加)、
  「+ トラックを追加」、ピアノロールの全編集。
  原則どおり全部 Command API 経由なので履歴に載り、undo でき、AI からも見える
- **人間編集の AI キャッチアップ**: チャット送信時に `build_chat_context`(main.rs)が
  「前回ターン以降の人間の編集一覧 + 現在の project_version」をプロンプトに前置する。
  最後に見せたエントリ ID は `cache/chat-last-seen.txt` に永続化(アプリ再起動をまたぐ)。
  そのエントリが undo で消えていた場合は「履歴が巻き戻されている」旨を注入して振り出しに戻す。
  システムプロンプトにも「人間が並行編集する。会話の記憶より project_version と履歴を信頼」を明記
- レイアウト: タイムラインが全幅、チャットと履歴は下部に横並び。下部の高さと
  チャット/履歴の幅はドラッグハンドルで調整可(localStorage に保存)
- トランスポート: ⏮/⏪/▶/⏹/⏩/⏭(先頭・前小節頭・再生・停止・次小節頭・終端)。
  キー: Space / ← / → / Home / End。再生中はタイムラインが再生ヘッドを自動追従(ページ送り)
- **小節範囲マスク**: ルーラーをドラッグすると小節範囲を選択(琥珀色ハイライト)。
  選択中にチャットで指示すると「この範囲に限定して編集せよ」が自動で前置される
  (`selection.svelte.ts` 共有ストア。クリックで解除)。範囲は送信後も維持され連続指示できる
- **プロジェクト管理 UI 実装済み**: ヘッダーの曲名がメニュー(新規作成 / フォルダを開く /
  最近使ったプロジェクト)。フォルダ選択は `tauri-plugin-dialog`(capability に `dialog:default`)。
  最近使った一覧は `%APPDATA%\glaux\recent.json`(他 OS は XDG / ~/.config)、上限 15 件。
  新規作成の既定の場所は `<ホーム>/Music/Glaux/<名前>.glaux`
  - **切り替えはインプロセス**: アクターに `SwitchProject` リクエストを追加し、アクター自身が
    Session + Store を差し替える。SessionHandle を持つ全員(UI・アプリ内 MCP・チャット・
    エンジン再構築タスク)がそのまま追従するので、プロセス再起動不要(`tauri dev` でも動く)。
    切り替え時は ProjectChanged が飛び、UI 再取得・エンジン再構築が自動で走る
  - チャットはプロジェクトごとに別会話(cache のセッション ID / last_seen を読み替え)。
    ウィンドウタイトルも「Glaux — <曲名>」に更新
- **設定パネル**(2026-09-22): ヘッダーの ⚙ から。テーマカラー 6 種
  (既定はダーク + ターコイズ)、AI 完了チャイム(WebAudio、プラグイン不要)。
  localStorage 保存(`settings.svelte.ts`)。設定候補メモ: 既定プロジェクト保存先、
  MCP ポート、スナップ既定値、ラウドネス目標、自動バックアップ
- **BPM はヘッダーの表示をクリックして編集可**(set_tempo の先頭イベントを書き換え。
  途中のテンポイベントは保持)
- **音源はトラックヘッドの 🎹 ラベルをクリックして選択可**(2026-09-22。subtractive / drum。
  set_device 経由なので履歴に載り undo 可。切り替えでパラメータは初期化)
- **作業フォルダはプロジェクトメニューの「作業フォルダ:」クリックで変更・永続化**
  (2026-09-22。`%APPDATA%\glaux\settings.json` の `projects_dir`。新規作成の既定先になる)
- **プロジェクトの移動 / 名前変更**(2026-09-22): プロジェクトメニューに専用セクション。
  フォルダ移動は**アクター内で実行**(`Request::MoveProject`)して進行中の保存と直列化し、
  rename 不可(別ドライブ)ならコピー + 削除にフォールバック、開き直し失敗時は元の場所へ戻す。
  名前変更時は `set_title` でタイトルも追従。履歴・AI 会話(cache/)はフォルダごと移動するので
  継続する。AI チャット実行中は拒否。旧パスは recent から除去
- **ピアノロールの空白バグ修正**(2026-09-22): 長いクリップ × ズームで Canvas 実サイズが
  ブラウザ上限(約 16k px)を超えると黙って全消えしていた。上限内に収まる解像度スケールへ
  自動で落とす(極端な場合のみ僅かにぼやける)。根治は仮想化描画(バックログ)
- **ピアノロール「真っ暗・鍵盤が固定されない」の真因**(2026-09-22 解決):
  パネルのルート `<div class="overlay">` 用の CSS(z-index:5 + 不透明 background)が、
  同じ `overlay` クラスを持つオーバーレイ canvas にも当たっていた(Svelte のスコープは
  コンポーネント内のクラス衝突を防がない)。canvas 側を `note-layer` に改名して解決。
  headless Chrome + Tauri API モックの UI 検証で再現→修正確認済み。
  **教訓: 同一コンポーネント内で用途の違う要素に同じクラス名を使わない**
- 未実装(次段階): 
  段階的開示のデバイスパネル、チャットのターン境界と
  AI インジケータの連動はチャット経由のみ正確(外部 MCP クライアントは近似のまま)

### `crates/glaux-godot`(Godot 拡張、第 1 段階 2026-09-24)

目的: 別プロジェクトの Godot 4.3 の音ゲーで、曲と敵の動きを同期させる(曲 → 敵)。オフラインで書き出した
音声では敵のアルゴリズムと拍の連動が作りにくいので、Glaux の再生エンジンをゲーム内で動かす。使い方は `docs/GODOT.md`。

- gdext(`godot` 0.5、`api-4-3` + `experimental-threads`)。`crate-type = ["cdylib"]`。`RawPtr` が `godot` から
  再公開されていないため `godot-core` も直接依存(版は揃う)
- `GlauxStream`(AudioStream)/ `GlauxPlayback`(AudioStreamPlayback): Godot の音声スレッドの `_mix` で
  `Renderer::process` を回す(1024 フレームずつ、バッファは事前確保)。ミックスの頭の `Shared::pos` を
  `MixClock::mix_start` に置く
- `GlauxPlayer`(Node): 子に AudioStreamPlayer を 1 つ持ち、常に流したまま `Shared::playing` / `seek` で制御。
  聞こえている位置 = mix_start + `AudioServer.get_time_since_last_mix()` − `get_output_latency()` − 手動補正。
  再生中は単調(逆戻りしない)、再生直後は負になりうる(音が届いてから最初の出来事を出す)。
  `_process` で `(前回, 今]` の拍・マーカー・監視トラックのノートを時刻順にシグナル化(再生開始・シークの位置
  ちょうどの出来事も出す)。曲の終わり(レンダラの自動停止)で `song_finished`
- 時間軸は `glaux_engine::timeline::Timeline`(純粋な計算・単体テストあり): テンポ・拍子から拍の一覧(曲の終わりの
  次の小節頭まで)、マーカー、トラックごとのノート(ループ展開済み)、秒 ↔ tick ↔ 拍位置
- 読み込み(`song.rs`): Godot の `FileAccess` で `project.json` と WAV を読む(.pck 内でも読めるように)。
  `SampleBank::sync_with`(読み方を差し替えられる版)と `load_wav_bytes` / `sf2::load_font_bytes` を追加。
  SoundFont は曲フォルダの `soundfonts/` か `res://soundfonts/`。CLAP の音源のトラックは無音(既定のシンセで
  鳴ってしまうため)にし、CLAP エフェクトは外して警告
- 動作確認: `godot/demo`(4 小節のデモ曲、`--check` で 4.5 秒鳴らして結果を出して終了)を Godot 4.3 の Linux 版で
  ヘッドレス実行。拍 10・キック 10・マーカー 2 つが「聞こえている位置」から 0〜0.1 秒で届き、マスターに音が出る。
  終了時の ObjectDB リーク警告は Godot 標準の AudioStreamGenerator でも出る(再生中に終了した場合の Godot の都合)
- 配布物(2026-09-24): アドオンのフォルダ `godot/demo/addons/glaux/` が正で、説明書 3 点を同梱する
  (`README.md` = 人間向け、`AI_GUIDE.md` = ゲーム側の AI 向け・コード例は Godot で実行確認済み、
  `PROMPT.md` = ゲーム側の AI に渡すプロンプト例)。`godot/build_windows.bat` が DLL をビルドし、
  `godot/dist/addons/glaux/` と `godot/dist/glaux-godot-addon.zip` を作る(dist はコミットしない)。
  `docs/GODOT.md` は Glaux 側の手順(ビルド・配布・デモ)だけにした
- Windows 実機でデモの動作を確認済み(2026-09-24)
- 未確認・次段階: ゲーム書き出し後の読み込み(.pck に project.json と WAV を入れる設定)、
  ゲーム中のトラック音量・ミュート(展開の切り替え)、CLAP トラックの音声への焼き込み

### リリースビルド(2026-09-24)

`scripts/build-release.bat` → `npm run tauri build`(Tauri CLI v2 が `tauri/custom-protocol` を自動で有効にし、
フロントは exe に埋め込まれる)。`CARGO_TARGET_DIR=target\windows`(開発版と共有)。成果物を `release\` に集める:
`Glaux.exe`(単体で動く。要 WebView2)と NSIS インストーラー(`installMode: currentUser`・日本語)。
MSI(WiX)は VBScript に依存し新しい Windows で失敗しやすいので作らない(`bundle.targets = ["nsis"]`)。
アプリは元からリポジトリの場所に依存しない(既定のプロジェクトは `<ホーム>/Music/GlauxDemo.glaux`、設定類は
`%APPDATA%\glaux\`、学習済みモデルは exe に埋め込み、外部コマンドは CREATE_NO_WINDOW で起動)。
tauri.conf.json は CLI 同梱のスキーマで検証済み。Windows 実機のビルドは未確認

### Godot 拡張: 効果音と和声(2026-09-25)

ゲーム側(RHYTHRASH)の要望: 効果音を WAV にせず、音程を BGM のキー・コードに合わせて鳴らしたい(入力の瞬間と、拍に合わせる
音の両方)。

- glaux-core `harmony`: `chord_pitch_classes`(コード名 → 構成音のピッチクラス。分析が出す品質のみ)、
  `scale_pitch_classes`、`snap_to_pitch_classes`(同じ近さなら上)、`nth_pitch_from`(base 以上で下から数える)
- glaux-engine: 時刻指定のノート `midi::TimedNote` と `NoteQueue`(2 つの 64 bit 枠に詰めた SPSC リング。取り出しは
  ロックフリー)。`Shared::notes`。レンダラはブロック頭で受け取り(`timed`、容量 256)、フレームごとに時計
  (`Renderer::clock()`、再生・停止に関係なく進む)が来たものをライブのボイスとして鳴らし(`LiveVoice::off_at` で
  長さの後に離す)、トラックのエフェクトを通す。止まっている間の早道は `timed` が空のときだけ。CLAP のトラックは鳴らさない
- glaux-godot: 読み込み時に `harmony::analyze`(キーと小節ごとのコード)。`get_key` / `get_chords` / `get_chord_at` /
  `get_chord_tones` / `get_scale_pitch_classes` / `snap_to_scale` / `snap_to_chord` / `get_chord_note` / `get_scale_note`。
  `play_note`(すぐ = at 0)/ `play_note_at`(time に聞こえるように)/ `release_notes`、`sync_to`(時刻の基準の
  GlauxPlayer)。`MixClock::mix_clock` に直前のミックスの頭のレンダラの時計を置き、
  at = mix_clock + (経過 + d − 出力の遅れ)·sr(d = time − 基準の曲の聞こえている位置。手動補正は除く)。
  基準の曲の位置も同じ「経過 − 出力の遅れ」から出すので打ち消し合い、同じミックスの時計どうしでサンプル単位にそろう
- 確認: エンジンの単体テスト(止まっていても指定サンプルちょうど・長さの後に消える・過ぎた時刻はすぐ)、
  core の単体テスト、Godot のデモ(`--check` で BGM と効果音を別のバスに録り、4 拍への予約がすべて 0.1ms 以内)、
  説明書のコード例を Godot で実行。デモ曲に Pad(Am F C G)を足し、効果音用の `songs/sfx.glaux`(Bell / Blip)を追加。
  デモでは Z キーで次の拍にコードの構成音のベル

### 将来

- ~~CLAP プラグインホスティング~~ → 音源は実装済み(2026-09-23、`glaux-clap`)。エフェクト・パラメータ公開は第 2 段階
- ~~タイムストレッチ(`Stretch::Follow`)~~ → 実装済み(2026-09-23、WSOLA)。より高品質な伸縮(位相ボコーダ + 過渡保持)は将来
- ノードグラフによる音作り
- ブラウザ版(Rust コアを WASM 化、共有・軽作業用)
- ローカル AI モデル(音声解析、GPU 利用)

---

## 6. 未決定事項(要判断)

- ~~フロントエンドフレームワーク~~ → **Svelte 5 に決定**(2026-09-21)
- ~~`Session` の共有方式~~ → **アクター方式で実装済み**(`glaux_mcp::actor::SessionHandle`)
- ~~MCP のトランスポート~~ → **両対応**: 単体検証用に stdio(`glaux-mcp` バイナリ)、
  アプリ連携はアプリ内 HTTP(streamable)。同一プロジェクトでの同時使用は禁止
- `get_project` の分割・フィルタ API の形
- CLAP の `state` を `params` にどこまで写すか
- 音声ファイルの取り込み時に `audio/` へコピーするか参照するか ← コピー推奨(プロジェクトフォルダで完結)
- ~~`history.jsonl` の肥大化対策~~ → compaction 実装済み(§8 バックログ参照)

---

## 7. glaux-core のテスト構成(壊さないこと)

| テスト | 検証内容 |
|---|---|
| `tests/schema.rs::fixture_parses_and_roundtrips` | 手書き `project.json` が読め、往復で一致 |
| `tests/schema.rs::fixture_json_is_stable` | 書き出し→読み→書き出しが文字列一致(順序安定) |
| `tests/schema.rs::command_json_shape` | コマンドの JSON 形(AI が書く形)が期待通り |
| `tests/history.rs::every_command_is_invertible` | ランダム 1500 コマンドで `apply`→`apply(inverse)` が完全復元、失敗時無変更 |
| `tests/history.rs::batch_rolls_back_on_failure` | Batch の途中失敗で巻き戻し |
| `tests/history.rs::replay_reconstructs_project` | 300 操作を JSONL 経由でリプレイして一致 |
| `tests/history.rs::undo_redo_and_checkpoints` | undo/redo/checkpoint/revert_to |
| `tests/history.rs::revert_middle_entry_detects_conflicts` | 途中 revert と衝突検出、`by_author` |
| `tests/history.rs::split_midi_clip_moves_and_truncates_notes` | 分割時のノート移動・切り詰めと復元 |

新しい `Command` を足したら `random_command` に生成パターンを足すこと。可逆性テストが自動で守ってくれる。

---

## 7.5 AI の感覚マップ(2026-09-22 整理)

Glaux の AI が「何を知覚し、何を操作できるか」の一覧。新しい能力を足すときはここと
[`CAPABILITIES.md`](CAPABILITIES.md)(利用者向けの整理。ジャンル別の向き・不向き込み)を更新する。

| 感覚 | 実体 | 内容 |
|---|---|---|
| **視覚(楽譜)** | get_project | 全トラック・ノート・パラメータ・テンポ/拍子を JSON で読む。include_notes: false で構造だけ俯瞰 |
| **音楽理論の目** | analyze_harmony | ノートからキー(Krumhansl-Schmuckler)と小節ごとのコード(テンプレート照合 + ベース音補助)を推定。ドラム自動除外、confidence / out_of_key_ratio 付き |
| **聴覚** | analyze_audio | オフラインレンダ → LUFS / ピーク / クレスト / 帯域バランス / スペクトル重心 / オンセット。per_track でミックス内の各トラックの埋もれ診断 |
| **時間感覚** | tick(PPQ 960)+ tempo/time_sig_map | 絶対 tick で位置を把握。拍子変更・テンポ変更も読める |
| **記憶(短期)** | 会話セッション(--resume) | アプリ再起動をまたいで会話継続 |
| **記憶(長期)** | get_history + project_version | 履歴はプロジェクト側に永続。author=human で「人間が何をしたか」をキャッチアップ(チャットは差分を自動注入) |
| **手(作曲)** | apply_commands + 便利ツール | ノート/クリップ/トラック編集。transpose/shift/quantize/scale_velocity は相対編集の代行 |
| **手(音作り)** | list_params + set_param + add_effect | 全つまみに聴感説明付き。音源 7 種(subtractive/drum/pluck/sampler/sf2/fm/wavetable)+ エフェクト 9 種(eq/comp/reverb/dist/amp/sidechain/delay/chorus/tape) |
| **表現(奏法)** | Note.articulation | 楽器ごとに対応が異なる(下表)。カタログ(list_params)に楽器別の説明付きで載る |
| **リズム感** | analyze_rhythm | スウィング比・グリッド(straight / triplet)・シンコペーション・ずれ・密度 |
| **表現(時間変化)** | set_automation_points / set_master_automation_points | 音量・パン・音色・エフェクト・CLAP のつまみ・マスターのカーブ |
| **表現(ピッチ)** | Note.pitch_curve | 1 音ごとの自由な音程カーブ(内蔵音源と CLAP のノート表現の両方に届く) |
| **外部音源** | list_plugins / list_plugin_presets / load_plugin_preset / list_params | CLAP 音源の選択・プリセット・公開つまみ |
| **道具箱** | presets / soundfonts | save_preset / load_preset(全プロジェクト共通)、list_soundfonts / set_soundfont_instrument(GM 楽器一式)、import_sample(実録 WAV) |
| **安全網** | checkpoint / revert_to / undo | 試行錯誤の足場。Batch = 1 undo |
| **場の把握** | UI からの文脈注入 | 範囲選択・開いているクリップ・音作り中のトラックが指示に自動で付く |

### 奏法 × 楽器の対応表(glaux-dsp `articulations_for` が正)

| 奏法 | subtractive | drum | pluck | sampler / sf2 | fm / wavetable | clap | 効果 |
|---|---|---|---|---|---|---|---|
| palm_mute (M) | ○(こもった刻み) | − | ◎(ブリッジミュート。本命) | − | ○(減衰 4 倍速) | △(長さ半分・ベロシティ 0.85 倍) | 減衰を速く・暗く |
| staccato (S) | ○ | − | ○ | ○ | ○ | ○ | 音価半分 + 短リリース |
| accent (A) | ○ | ○ | ○ | ○ | ○ | ○(ベロシティ 1.25 倍) | 強く(楽器により明るく) |
| vibrato (V) | ○ | − | ○ | ○ | ○ | ○(Tuning 表現) | 後半に深くなる揺れ |
| bend (B) | ○ | − | ◎(チョーキング) | ○ | ○ | ○(Tuning 表現) | 全音下から滑り上がる |
| legato (T) | ◎(立ち上がりを飛ばす) | − | ○(ハンマリング) | ○(フェード) | ◎ | △(重ねて送る。プラグインのモノ / レガートモードで効く) | 直前の音から弾き直さずにつなぐ |
| portamento (P) | ◎ | − | ○(スライド) | ○ | ◎ | ○(Tuning 表現) | legato + 直前の音程(無ければ全音下)から約 0.15 秒で滑る |

CLAP のビブラート・ベンドは `glaux_dsp::articulation_cents`(内蔵の `PitchExpr` と同じ形。テストで 1 セント以内に一致)を
ピッチカーブと足して、ノート ID 付きの Tuning 表現として 64 サンプルごとに送る(2026-09-24)。MIDI だけのプラグインには届かない。

対応外の奏法を付けてもエラーにはならないが音は変わらない(no-op)。
UI のショートカットは楽器に応じて絞り込まれ、ヒント文にもその楽器のぶんだけ表示される。
新しい奏法・楽器を足すときは `articulations_for` と PianoRoll の `ARTS_BY_INSTRUMENT` の両方を更新すること。

**まだ持っていない感覚**: 生波形の知覚(analyze_audio は要約統計のみ)、
人間の演奏は録音・MIDI 入力で取り込めるが、AI がリアルタイムに聴くことはできない、
プラグインが公開していない設定(Surge XT の LFO のテンポ同期・モジュレーションの割り当て等)。
(曲の構成は sections、リズム・グルーヴは analyze_rhythm で把握済み)

## 8. 課題整理・ロードマップ(2026-09-21 整理)

実機での作曲テストを経て出た要望と既存の残課題を、優先度付きで整理する。
一気にはやらない。上から順に、1 項目 = 1 マイルストーンが目安。

### 優先キュー

1. ~~ピアノロール表示~~ → **実装済み(2026-09-21)**。クリップをダブルクリックで
   タイムライン上にオーバーレイ表示(Canvas 描画: 鍵盤・グリッド・ノート・再生ヘッド)。
   AI が編集すると開いたままライブ更新。ルーラークリックでシーク、Esc で閉じる

2. ~~ピアノロール編集 + AI アレンジワークフロー~~ → **実装済み(2026-09-21)**
   - 編集(すべて `apply_edit` → Command、author: human): ダブルクリックで追加(スナップ長)、
     ドラッグで移動(複数選択対応)、右端ドラッグで長さ、右クリック / Del で削除、
     矩形選択、Shift+クリックで選択トグル、スナップ切替(1 小節〜1/16)
   - **AI アレンジ連携**: ピアノロールを開いている間、チャット指示に「対象クリップ」が
     自動で前置される(`pianoRollStore.focus`)。範囲マスクとも併用可。
     人間編集のキャッチアップ機構がそのまま効くので「私が直したメロディにハモリを付けて」が通る
   - 追加実装(同日): ノート試聴(追加/クリック/移動中のピッチ変化で鳴る。エンジンに
     停止中でも鳴るプレビューボイス枠を追加 = `EngineHandle::preview_note`)、
     Ctrl+ホイール横ズーム / Shift+ホイール縦ズーム(カーソル位置維持)、
     Ctrl+C/X/V コピペ(クリップまたぎ可、貼り付けはホバー位置にスナップ)、
     ダブルクリック位置の小節を中央に表示、タイムラインに「+ トラックを追加」ボタン、
     Ctrl+Z / Ctrl+Y(Ctrl+Shift+Z)で undo / redo
   - ~~ベロシティ編集 UI~~ → **実装済み(2026-09-23)**: ピアノロール下端の「Vel」帯
     (縦スクロールしても下端に固定、横はノートと連動、ヘッダーの「ベロシティ」で開閉)。
     縦棒を上下にドラッグで変更、選択中のノートの棒を掴むと選択中すべてを同じ量だけ増減。
     確定時に update_notes 1 回(1 undo)
   - **ドラムキット UI**(2026-09-22): drum トラックのピアノロール上部に「真上から見た
     ドラムセット図」(SVG)を表示。パーツをクリックすると試聴 + 挿入カーソル位置に打ち込み +
     該当行へスクロール&ハイライト。鍵盤列にも Kick/Snare 等のラベル。🥁 ボタンで開閉。
     打ち込み先は**挿入カーソル**(青。グリッドの空白クリックで固定、←/→ でスナップ単位移動。
     ホバー追従の琥珀線とは別物 = キットまでマウスを動かしても位置が動かない)。
     Ctrl+V の貼り付けだけはホバー位置(キーボード操作なのでマウスは動かないため)。
     マッピング(pitch↔名前↔配置)は `app/src/lib/drumMap.ts` に一元化しており、
     dsp 側 drum.rs の GM 配置と対応。**将来キットを追加・差し替えるときは、この定数を
     dsp のレジストリから供給(list_params 経由等)する形に移行する**(拡張性の要検討事項)

3. ~~パフォーマンス最適化パス~~ → **第 1 弾実施済み(2026-09-22)**:
   - **タイムラインのノートプレビューを SVG → Canvas 化**(`ClipPreview.svelte`)。
     ノート数ぶんの DOM 要素生成が消えた(数千ノートで最大の改善)
   - **ピアノロールを 2 層 Canvas 化**: 静的層(グリッド+ノート。内容変更時のみ)と
     動的層(再生ヘッド・カーソル・ドラッグゴースト。高頻度更新はこちらだけ)
   - **UI 再取得のデバウンス**: project-changed の先頭は即時、連続分は 80ms 窓で合流
   - **エンジン再構築の合流**: 40ms の静穏待ち + イベントドレインで連続編集を 1 回の再構築に
   - **history.jsonl の追記モード**(`Store::save_after_change`): apply 時は末尾 1 行 append。
     undo/redo/revert 時のみ全書き換え。累積 O(n²) を解消
   - **transport ポーリングを停止中 400ms に間引き**(再生中は 100ms のまま)
   - **dev プロファイルでも glaux-dsp / glaux-engine / rustfft / hound を opt-level 3 に**
     (tauri dev でのオーディオ余裕と analyze/export 速度)
   - 残り(重さが再発したら): get_project の差分/部分取得、ParamChanged の軽量差し替え、
     履歴パネルの仮想スクロール、プロファイラでの実測
   - **第 2 弾(2026-09-22)**: 音源リッチ化で再生の瞬断が発生 →
     オーディオバッファを 1024 フレーム要求(拒否時は既定にフォールバック)+
     長く無音のトラック(残響テール 4 秒経過後)のエフェクト処理をスキップ

4. ~~音作り拡張フェーズ 1(EDM / メタル)~~ → **実装済み(2026-09-22)**:
   - **`distortion`** エフェクト: tanh 波形整形(drive_db)+ トーン LP + mix(パラレル可)+ level
   - **`sidechain`** エフェクト: `source` パラメータ(トラック ID 文字列、構築時に index 解決)。
     検出信号は**ソーストラックの生モノ合算(エフェクト前)**。レンダラはフレームごとに
     全トラックの生合算を先に確定させるので、トラックの処理順に依存しない。
     ミュートしたソースは検出 0(= 踏まれない)という素直な意味論。
     **同日ダッカー型に再設計**: レベル追従型はキックの胴鳴り(約 0.45s)の間ずっと沈み
     「揺れ」にならなかった。キックの立ち上がりでトリガー → duck_db まで attack_ms で沈み、
     余韻に関係なく release_ms で浮上(キックごとにリトリガー)。ratio → duck_db に置換。
     release をビート(8 分 = 60000/BPM/2 ms)に合わせるのがポンピングのコツ(spec に記載)
   - **subtractive 拡張**: `unison`(1..7)+ `detune`(supersaw)、`sub`(1 オクターブ下)、
     `noise`。デフォルトは従来音を完全維持(既存プロジェクトの音は変わらない)。
     ユニゾンは等パワー正規化(√n)
   - **drum にノート 55 = リバースクラッシュ**(全長 1.6s×decay、3 乗カーブで迫り上がる)。
     ドラム UI のキット図には未追加(AI 経由で使用可。カタログ説明に記載)
   - 未実装のまま: 第 2 オシレータ、EDM キックの専用チューニング(tune/decay で代用)、
     エンジンのバス構造(サイドチェインは上記方式で不要になった)

5. ~~アーティキュレーション設計~~ → **実装済み(2026-09-22。案 A)**
   - `Note.articulation: Articulation`(normal / **palm_mute** / staccato / accent)。
     normal は JSON に書かない(`skip_serializing_if`)ので旧ファイルと互換、
     FORMAT_VERSION 据え置き。`NoteChange.articulation` で部分更新可(可逆)
   - 音の実装: トラック共有の `SubtractiveParams` は変えず、**ボイス側の倍率(`ArtMod`)**で表現
     - palm_mute: cutoff×0.3 + decay×0.18 + sustain 0(ズンズンした刻み。distortion と併用推奨)
     - staccato: 音価を半分に短縮(エンジンの `build_playback_data`)+ release 短め
     - accent: amp×1.4 + cutoff×1.5(強く・明るく)
     - drum はアクセントの音量強調のみ反映
   - UI: ピアノロールでノート上にマーカー(M / S / >)表示、選択して **M / S / A キー**でトグル
     (全選択が同じ奏法なら解除)。AI へは apply_commands の説明・instructions・
     システムプロンプトに記載(メタルの刻み = distortion + palm_mute)
   - **ギター音源 `pluck` 実装済み(2026-09-22)**: Karplus-Strong 拡張の撥弦物理モデル
     (`glaux-dsp/src/pluck.rs`)。ディレイライン(固定 2048 サンプル、Copy)+ ピック硬さで
     フィルタしたノイズ励起 + 明るさブレンドのループ減衰。パラメータは decay / brightness /
     pick / gain_db。palm_mute は「掌で押さえる」を物理で表現(減衰 ×0.12 + 暗く)。
     出荷時プリセット(アコースティック / クリーンエレキ / メタルギター)を初回起動時に導入
     (`presets::ensure_factory`、マーカーファイルで再導入なし)。
     AI への案内: ギター・ベース・ハープは pluck、エレキ = pluck + **amp**。
     残る将来項: サンプラー音源、legato/slide(ボイス跨ぎが必要で大工事)
   - **アンプシミュレータ `amp` 実装済み(2026-09-22)**: pluck 単体は「アンプに
     繋いでいない生弦」でクリーンにしかならない、というユーザー指摘(段構成の欠落)を
     受けて追加した 6 つ目のエフェクト。DC ブロック → 2 段クリップ(段間 LP +
     非対称 tanh = 偶数次倍音)→ トーン → プレゼンス → キャビネット(HP 80Hz +
     LP 4.2kHz×3 = 18dB/oct)→ レベル。プリゲイン最大 54dB(distortion は
     ペダル 1 個ぶんの 40dB・整形なし。amp の前段ブースターとして共存)。
     出荷時プリセットは v2 に更新(クリーンエレキ / クランチギター / メタルギター、
     すべて pluck + amp 構成。`FACTORY_VERSION` マーカーで旧版から自動更新、
     ユーザー独自プリセットには触れない)

6. **音作りプロジェクト(サウンドデザインモード)+ プリセット**
   - ~~プリセット保存/読み込み~~ → **実装済み(2026-09-22)**:
     `glaux_mcp::presets` モジュール。「音源 + エフェクトチェーン」を 1 パッチとして
     `%APPDATA%\glaux\presets\<名前>.json` に保存(**全プロジェクト共通**)。
     - `FxId` は保存せず適用時に採番。適用 = set_device + 既存 FX 全削除 + 追加の
       **1 Batch(1 undo)**
     - MCP ツール: `list_presets` / `save_preset` / `load_preset` / `delete_preset`。
       instructions とシステムプロンプトに「音作りはまず list_presets → load_preset → 微調整」を記載
     - UI: トラックヘッドの 🎹 音源メニューにプリセット一覧(クリックで適用)と
       「今の音を保存」入力欄
   - **ループ再生(トランスポート)実装済み(2026-09-22)**: 🔁 ボタン(ショートカット L)。
     ルーラーで範囲選択していればその区間、なければ曲全体をループ。ループ中に選択を変えると追従。
     エンジンは `Shared::loop_start/loop_end`(サンプル、end<=start で無効)を毎フレーム判定し、
     終端で区間頭へ巻き戻し(発音中ボイスは note_off で余韻を残す、イベント/オートメーション
     カーソル再同期)。ループ中は自動停止を無効化。`EngineHandle` は tick で保持し
     テンポ変更時にサンプル位置を焼き直す。プロジェクト切り替え時は自動解除
   - **音作りビュー(Phase 1)実装済み(2026-09-22)**(`SoundDesignPanel.svelte`):
     「トラック 1 本にフォーカスするビュー」として設計(A: 既存トラックの音を変える /
     B: 新トラックで音作り / C・D: SoundLab プロジェクト、の 4 パターンを 1 部品で賄う。
     別プロジェクト同時オープンはアーキテクチャ大工事なので Phase 3 送り)。
     - トラックヘッドの 🎛 で右ドロワー開閉。ParamSpec 駆動のつまみ(float/int スライダー、
       bool チェック、enum セレクト。description がツールチップ)、エフェクトチェーン
       (ON/バイパス・削除・追加)、プリセット(適用・保存)、ソロ切替、試聴フレーズ挿入
       (ロングトーン → 刻み → 分散和音 → オクターブ上の 4 小節)
     - すべて Command API 経由(set_param / add_effect / set_effect_bypass / …)なので
       履歴に載り undo 可。spec+現在値は `glaux_mcp::server::track_params_json` を
       MCP list_params と共用(tauri コマンド get_track_params)
     - ビューを開いている間はチャットに「音作り中のトラック」を注入
       (「もっと太く」が対象トラックへ向かう)
   - **SoundLab テンプレート(Phase 2)実装済み(2026-09-22)**: 新規プロジェクトに
     「🎨 音作り用テンプレートで作成」チェック。ON にすると `<作業フォルダ>/SoundLab/` に
     作成し、subtractive トラック + 試聴フレーズ(`lib/phrase.ts`、音作りビューの挿入と共用)+
     ループ ON + 音作りビュー展開の状態で開く。§8-6 はこれで完了
     (Phase 3 = 別ウィンドウ / 同時 2 プロジェクトは必要になったら)

7. **ボリューム/パンのオートメーション対応**
   - ~~第 1 段: エンジン対応~~ → **実装済み(2026-09-22)**。track/volume_db(dB で補間)と
     track/pan のレーンをサンプル位置に焼き込み、レンダラで区分補間
     (linear/hold/exponential、core の `value_at` と同義)。評価カーソルは単調前進で
     resync 時リセット。**レーンがあるトラックはフェーダー値より優先**(DAW の慣例)。
     AI は set_automation_points でフェード・ビルドアップ・パンの揺れを描ける
     (apply_commands の説明とシステムプロンプトに記載済み)
   - ~~第 2 段: 人間用 UI~~ → **実装済み(2026-09-22)**(`AutomationLaneRow.svelte`):
     トラックヘッドの「〜」ボタンでトラック下にレーンが開く。音量/パンをタブ切替、
     ダブルクリックで点追加(1/16 スナップ)・ドラッグで移動・右クリックで削除
     (最後の点を消すとレーン削除 = フェーダー復帰)。カーブは linear/hold/exponential を
     描画(編集で付くのは linear。他は AI が書いたものを表示)。
     レーン未使用時はフェーダー値の破線を表示。**音量レーン使用中はフェーダーを無効化**して
     「オートメーション優先」を明示
   - 第 3 段: **音色パラメータの再生対応 実装済み(2026-09-22)**。target を
     `device/<パラメータ名>`(例 `device/cutoff`)にしたレーンが鳴る。エンジンは
     ブロック頭でレーンを評価し、トラック別スクラッチ(起動時確保)の
     `InstrumentParams` に `set_continuous`(bake と同じクランプ・dB 変換)で上書き。
     発音中のボイスにも効くのでフィルタスイープ・EDM ビルドアップが作れる。
     連続値パラメータのみ(waveform 等の enum/int は対象外)。UI レーンは音量/パンのみで
     device レーンは AI(set_automation_points)から使う想定

8. ~~途中の拍子変更のサポート~~ → **実装済み(2026-09-22)**(プログレ対応)
   - 共有の小節マップ `app/src/lib/barMap.ts` を新設: `time_sig_map` の全イベントから
     可変長の小節列(`buildBars`)を構築し、`barAtTick` / `prevBarHead` / `nextBarHead` を提供。
     **拍子イベントの tick は新しい小節の頭**として扱う(直前の小節は途中でも区切る =
     一般的な DAW と同じ)
   - Timeline: グリッド・ルーラー・範囲選択・全幅を小節マップ準拠に(7/8 の小節は狭く描く)。
     拍子が変わる小節のルーラーに「7/8」チップを表示。密度は「4/4 の 1 小節 = 96px」基準
   - PianoRoll: 小節線・拍線(拍 = 分母の音価)・ルーラー番号を曲の絶対小節に追従。
     小節番号もタイムラインと同じ絶対番号になった(以前はクリップ内 1 始まり)
   - App: 小節ナビゲーション(⏪⏩ / ←→ / End)をマップ準拠に。ヘッダーの拍子表示は
     クリックで編集可能(BPM と同様、先頭イベント書き換え。途中のイベントは保持し、
     ある場合は「4/4*」と表示)。**途中からの拍子変更の挿入は AI 経由**(`set_time_sig` は
     全置換なので UI 化するなら既存イベントとのマージ UI が要る → バックログ)

9. ~~MCP 便利ツール~~ → **実装済み(2026-09-22)**(transpose_notes / shift_notes /
   quantize_notes / scale_velocity)
   - 現在値読み→絶対値変換をサーバー側で肩代わり。詳細は「glaux-mcp」節の便利ツール項を参照。
     apply_commands の description・サーバー instructions・アプリ内チャットの
     システムプロンプトから誘導している

### バックログ(順不同)

- ~~`revert(entry_id)` の UI~~ → **実装済み(2026-09-22)**: コアの `Session::revert` を
  アクター(`RevertEntry`)・MCP ツール `revert {entry_id}`・Tauri `revert_entry` に配線。
  履歴パネルの各エントリに ↩ ボタン(取り消し済みは非表示)。conflicts(後続の編集が
  同じ対象を触っている)は履歴パネル上部に警告表示。revert 自体も履歴に載り undo 可
- ~~途中の拍子変更を人間が UI から挿入・削除~~ → 実装済み(2026-09-23。ルーラー右クリック / 拍子チップ)
- ~~分割ピアノロール~~ → **実装済み(2026-09-22)**: 上ペインの「⫶ 分割…」で別クリップを
  下に開く(`pianoRollStore.second`)。クリックしたペインがアクティブ(`active`)になり
  キー操作を受ける。クリップボードは `noteClipboard` ストアで共有(奏法も一緒にコピー)。
  下ペインは ⇅ で上下入れ替え、Esc/✕ でそのペインだけ閉じる
- ~~ギター用フレット盤 UI~~ → 実装済み(2026-09-22。`Fretboard.svelte`。標準チューニング
  6 弦 × 0〜15F、クリックで挿入カーソル位置に打ち込み。pluck トラックで 🎸 ボタン開閉)
- ~~ビブラート/チョーキング(簡易版)~~ → 実装済み(2026-09-22)。articulation に
  vibrato(5.5Hz・±30 セント、0.12 秒後から深くなる)と bend(全音下から 0.22 秒で
  滑り上がるチョーキング)を追加。DSP は `expr.rs` の `PitchExpr`(周波数比の時間変化)を
  subtractive(発振器の周波数)と pluck(ディレイ周期)が共用。UI は V / B キー、
  マーカーは ~ / ↑。**連続ピッチカーブ(自由描画)の本格版はサンプラー検討と併せて設計**
- ~~クリップの移動・リサイズ等のタイムライン直接編集~~ → **実装済み(2026-09-22)**:
  クリップ本体のドラッグで移動(1 拍スナップ、Alt で解除、縦方向は同種トラックへ。
  ドロップ先レーンは破線ハイライト)、右端 8px のドラッグでリサイズ(最小 240 tick)。
  確定時に `move_clip` / `resize_clip` を発行(= Undo 可・履歴に載る)。
  ドラッグ直後 400ms はダブルクリックを抑制してピアノロール誤開を防ぐ。
  headless Chrome ハーネスで移動/リサイズ/トラック間移動/ダブルクリック維持を検証済み
- 段階的開示のデバイスパネル(UI からつまみを操作。今は AI 経由のみ)
- ~~サンプラー音源~~ → **実装済み(2026-09-22。第 1 段 = ワンショット)**:
  - `glaux-dsp/src/sampler.rs`: `PluginSource::Sampler { asset }` を音源として再生。
    root(サンプルの実音)からのピッチ差を再生レートに変換(線形補間)。
    2ms デクリック + release_ms フェード。vibrato / bend(PitchExpr)もレートに掛かる
  - RT セーフ設計: 波形は `Arc<SampleData>`(engine の `SampleBank` が WAV をモノラル
    デコードしてキャッシュ。編集ごとの再デコードなし)。`InstrumentParams` は
    Copy → Clone に変更(Arc の clone は参照カウントのみでアロケーションなし)
  - 取り込み: `glaux_mcp::assets::import_wav`(sha256 内容ハッシュで audio/ にコピー、
    同内容は重複しない)。AI は `import_sample` ツール、人間は音作りビューの
    「🎼 WAV」ボタン。取り込み + set_device は 1 Batch = 1 undo
  - 署名変更: `build_playback_data` / `render_project` / `export_wav` /
    `analyze_project(_tracks)` に `&SampleBank`、`EngineHandle::set_project` に
    `project_dir` が追加。`SessionHandle::project_dir()` 新設
  - ~~第 2 段: ループ点 + 音域マッピング~~ → **SoundFont 対応として実装済み(2026-09-22)**
- **SoundFont(.sf2)対応 = RSE 相当の楽器ライブラリ(2026-09-22)**:
  「同梱サンプルが無い」問題への回答。FluidR3_GM 等のフリー .sf2 を 1 つ置けば
  GM 全 128 楽器 + ドラムキットが本物っぽく鳴る
  - パースは rustysynth(MIT・依存なし)。`glaux-engine/src/sf2.rs` がプリセット層 ×
    インストゥルメント層の合成(音域/ベロシティ交差、チューニング/減衰は加算、
    エンベロープ時間は乗算)を行い `glaux_dsp::Zone` 列に落とす
  - 再生は `glaux-dsp/src/multi.rs` のマルチサンプラー: ゾーン選択(key×vel、
    最大 4 レイヤー同時 = ステレオペア対応)+ ループ(mode1/3)+
    AHDSR 音量エンベロープ。vibrato / bend も効く。モジュレータ・フィルタ・LFO は省略
  - .sf2 はプロジェクトにコピーせず **`<設定>/glaux/soundfonts/` の共有ライブラリ**を
    ファイル名参照(`PluginSource::Sf2 { soundfont, bank, preset }`。100MB 級のため)。
    SampleBank がフォント/ゾーンをキャッシュ
  - MCP: `list_soundfonts`(ファイル一覧 / file 指定でプリセット一覧)、
    `set_soundfont_instrument`(存在検証つき)。UI: 音作りビューに SoundFont セクション
    (.sf2 追加 → プリセット選択 → 適用)
  - テスト: 最小 SF2 バイナリをテスト内で生成してパース〜ゾーン構築を検証
  - ~~SF2 のフィルタ/LFO/モジュレータ~~ → **実装済み(2026-09-22)**: `ZoneMod`
    (initialFilterFc/Q、ビブラート LFO、モジュレーション LFO のピッチ/フィルタ、
    モジュレーションエンベロープのピッチ/フィルタ)。32 サンプルの制御レートで評価し
    TPT SVF ローパスに反映。CC 経由のモジュレータ(モジュレーションホイール等)は未対応
  - ~~連続ピッチカーブ~~ → **実装済み(2026-09-22)**: `Note.pitch_curve`(最大 8 点、
    ±2400 cents、線形補間・両端保持)。`PitchCurve` → `PitchExpr` で奏法と掛け合わせ。
    UI は表示のみ(編集は AI 経由。`update_notes` の pitch_curve)
  - ~~ドラム用 SF2 キット UI~~ → 実装済み(2026-09-22。bank 128 でキット図表示)
  - ~~非 WAV 素材~~ → **実装済み(2026-09-22)**: `assets::import_audio` が拡張子で分岐し、
    WAV はそのままコピー、mp3 / flac / ogg(vorbis)/ m4a・aac は symphonia 0.5 でデコードして
    32bit float WAV(`audio/<sha256>.wav`)に変換して置く(エンジンは WAV だけ読む)。
    UI のファイル選択・MCP import_sample / import_audio_clip が対応。
    手動確認用 `cargo run -p glaux-mcp --example import_check -- <file>`
  - ~~ピッチカーブの手描き UI~~ → **実装済み(2026-09-23)**: ピアノロールの「〜 カーブ」モードで
    ノートの上をなぞる(1 行 = 半音 = 100 cents、±2400)。対象は押した時刻にかかるノートのうち
    選択中を優先、なければ高さが近いもの。軌跡は時間方向に等間隔の最大 8 点に間引いて
    update_notes の pitch_curve に。右クリックでカーブ削除、Esc でモード終了
- **オーディオデバイス・入力レベル・遅延較正・自動音量(2026-09-23)**:
  - デバイス: `glaux_engine::list_devices()`(出力・入力の一覧と OS 既定)。出力の切り替えは
    オーディオスレッドが制御チャネル(`Ctl::SwitchOutput`)で受け、旧ストリームを閉じて
    同じ `Shared` で開き直す(現在位置へのシークで再生位置を保つ。失敗時は既定デバイスへ戻す)。
    サンプルレートは `Arc<AtomicU64>` で持ち、切り替え後に呼び出し側が `set_project` で
    再生データを作り直す。入力は `EngineHandle::set_input_device`(録音・入力テストで使う)。
    UI: 設定パネル「オーディオデバイス」(選択は localStorage に保存し起動時に復元)、
    フッターに使用中の出力(サンプルレート)と入力を常時表示(クリックで設定)
  - 入力レベル: `record::Ring` がコールバック内でピークを `fetch_max`(f32 ビット列)。
    `record::start_input(None, ..)` は録音せずレベルだけ測る「入力テスト」。
    `transport_state.input_peak_db`(読むとリセット)。設定パネルのメーター(-12〜-6dB の
    目安帯つき)と、録音中のトランスポート横のミニメーター
  - 遅延較正: `calibrate_start`(曲頭から `Shared::click_only` でメトロノームだけ鳴らし、
    1 小節カウントイン + 8 拍を補正 0 で録音)→ `calibrate_stop`(`calibrate::estimate_latency`:
    2ms 包絡で「30ms 静か → しきい値超え」の立ち上がりを各拍の -80ms〜0.9 拍で探し、
    遅れの中央値とばらつき(MAD)を返す)。結果はレイテンシ補正に自動設定。
    スピーカー再生ならクリックの回り込みで純粋なシステム遅延が測れる
  - 自動音量: `record_stop(auto_gain)` が使う範囲のピークを -6dBFS に合わせる gain_db を
    クリップに設定(0〜+30dB、下げない。元の WAV は変えない)。波形表示もクリップの音量を反映
  - ヘッダーは常に 1 行(`white-space: nowrap` + 横スクロール、MCP の URL 表示から縮む)
- **マスターバスのエフェクト(2026-09-23)**: コマンド `add_master_effect` / `set_master_param` /
  `unset_master_param` を追加し、`remove_effect` / `set_effect_bypass` はマスターも探す。
  エフェクト ID の重複検査(apply・validate)にマスターを含めた。UI はヘッダーのマスター音量横の
  🎛(エフェクト数を表示)で、音作りビューを「マスター」モード(エフェクトの節だけ)で開く。
  エフェクト一覧の JSON は `server::effects_json` をトラック・マスターで共用。チャットには
  「音作り中: マスターバス」と添える
- **音を分析する能力の強化 A: 細かな数値化(2026-09-23)**: 調査は `docs/AUDIO_ANALYSIS_RESEARCH.md`。
  - `glaux-engine/src/timbre.rs`: 単音の音色記述子(Timbre Toolbox 準拠)。包絡(10→90% の立ち上がり、
    減衰、持続レベル、余韻、20 点の曲線、`decays_continuously` = 傾きが ±6dB/秒 以内のフレームが 2 割未満)、
    音程(YIN を 16kHz に間引いて 5ms 刻み。外から `PitchFrame` を渡せる。しゃくり = 最初の 3 有声フレーム、
    ビブラート = 0.25 秒の移動平均を引いた揺れのゼロ交差と RMS)、スペクトル(重心・広がり・平坦さ・
    ロールオフ・フラックス・重心の推移 8 点)、倍音(8192 点 FFT の持続部平均で 16 次まで、奇数/偶数比、
    tristimulus、非調和性、HNR、減り方の回帰、波形の推定)、言葉のラベル
  - `render_track_note`: トラックの音源とエフェクトで 1 音だけ鳴らす(120BPM 固定、他のクリップ・
    オートメーション・音量・パン・マスターは使わない。CLAP も鳴る)
  - `glaux_mcp::sound`: 対象(音声クリップ / ファイル / トラックの 1 音)の読み込み。MCP `analyze_sound`
  - `analyze.rs`: `ebur128`(MIT)で LRA・True Peak・短期ラウドネスの 1 秒ごとの推移、ステレオ
    (相関・250Hz 以下の相関・サイド/ミッド比・左右差)、`analyze_mix` でトラック間のかぶり(臨界帯域ごとに、
    自分が鳴っている時間のうち相手が 6dB 以上大きい割合。自分のエネルギーの 8% 以上を占める帯域だけ)。
    MCP の analyze_audio(per_track で masking)
- **音を分析する能力の強化 B1: SwiftF0 による音程推定(2026-09-23)**: `glaux-ml/src/pitch.rs`。
  - モデルは lars76/swift-f0 の `model.onnx`(MIT)を onnxruntime の基本最適化で定数畳み込みしたもの
    (元のままだと tract がフィルタバンク行列 `ScatterElements → Reshape → MatMul` の形を解析できない。
    畳み込むと 135KB → 1.1MB。onnxruntime 独自の演算は入らない基本レベルなので標準の演算だけ)。
    `pitch` 出力は float64 なので f32 に変換する
  - 16kHz・256 サンプル(16ms)ごと。tract で一度だけ最適化するため固定長 [1, 11×256 + 32000 + 10×256]
    で推論し、前 11 フレームを捨て 125 フレームずつつなぐ(有声フレームは一括推論と 1e-5Hz 以内で一致)。
    fmin/fmax は 46.875 / 2093.75、区間のピークが 1e-3 未満なら確からしさ 0(公式と同じ)
  - 確からしさ 0.5 以上を有声。純粋なサイン波は 0.5 前後(公式でも同じ)で、倍音のある音は 0.95 以上
  - `timbre::resample_pitch` で 5ms 刻み(`ENV_HOP`)にそろえてから `describe` に渡す(ビブラートの計算が
    刻み固定のため)。`glaux_mcp::sound::describe` が SwiftF0 → 失敗時は YIN
  - 鼻歌の単旋律譜起こし(`transcribe_mono`)は実録音で調整済みの YIN のまま(置き換えは未評価)
- **音を分析する能力の強化 B2: Beat This! によるビート・小節頭・テンポ(2026-09-23)**: `glaux-ml/src/beats.rs`。
  - モデルは beat-this-rs が ONNX 化した small1(10.5MB、同梱)。tract はシンボル長の時間軸だと注意機構の
    Reshape を解析できないので、窓の長さごとに具体的な長さで最適化する(1500 フレームの計画は使い回し、
    短い窓はその都度 0.2〜0.4 秒で作る)。窓は公式と同じ(1500 フレーム、両端 6 フレーム、最後の窓は末尾合わせ、
    後ろの窓から書いて前の窓で上書き)。窓ごとの推論はスレッドで並列(155 秒の曲で約 9 秒、dev ビルド)
  - 前処理は torchaudio の MelSpectrogram を移植: n_fft 1024・hop 441・中心合わせの反射パディング・
    周期ハン窓・`normalized="frame_length"` は √n_fft で割る(torch.stft の normalized=True)・
    Slaney のメル尺度 30〜11000Hz 128 帯域(面積正規化なし)・log1p(1000x)。公式の ONNX 版と 1.5e-4 以内で一致
  - 後処理は公式の minimal(ロジット > 0 かつ ±3 フレームの最大、隣り合う点は平均、小節頭は最も近いビートへ)。
    実曲(beat-this-rs の test_files)で公式 Python の結果と ビート F=0.998・小節頭 F=1.000
    (`GLAUX_TEST_BEAT_AUDIO` / `GLAUX_TEST_BEAT_GOLDEN` で有効になるテスト)
  - テンポはビートの時刻を拍番号(直前のビートからの間隔で数え、抜けは 2 拍ぶん)への直線の傾きから求める
    (フレームが 20ms 刻みなので間隔の中央値だと 128BPM が 125/130 になる)。揺れは 16 拍区間の直線からのずれの中央値
  - MCP `analyze_beats`(clip_id / file)。UI はクリップメニュー「テンポに追従させる(素材の元のテンポを自動で検出)」
    → Tauri `detect_clip_tempo`(先頭 60 秒)→ `set_clip_stretch`
- **音を分析する能力の強化 C: LAION-CLAP で音を言葉で捉える(2026-09-23)**: `glaux-ml/src/clap.rs`。
  - **`laion/larger_clap_music` は使わない**: Hugging Face 版は言葉側が壊れている(どの文もコサイン 0.999 の
    ほぼ同じ埋め込み、logit_scale ≈ 0.03。transformers 4.46 / 5.17 のどちらでも同じ = 重み自体の問題)。
    `laion/larger_clap_music_and_speech`(Apache-2.0)は正常で、合成音のキック・ノイズ・サイン波を正しく言い当てる
  - 音声側だけを tract で動かす(0.2 秒 / 10 秒の音)。モデル(約 280MB)は同梱せず、`clap::model_path()`
    (`GLAUX_CLAP_MODEL` → 設定ディレクトリの `models/clap_audio.onnx`)から読む。取得元は Xenova の ONNX 変換
    (リビジョン固定、SHA-256 を確認。自前で書き出したものと 1.4e-6 以内で一致)。取得は `glaux_mcp::models::download_clap`
    (ureq、プロキシは環境変数)、UI は設定の「追加モデル」(進捗は `model-download` イベント)。開発用は `/models/`(ignore 済み)
  - 前処理は ClapFeatureExtractor(rand_trunc / repeatpad)を移植: 48kHz、10 秒に満たなければ繰り返し、
    STFT(1024 / 480、周期ハン窓、パワー)、Slaney メル 50〜14000Hz 64 帯域(面積正規化)、10·log10。
    公式と 7.6e-6 以内で一致。10 秒を超える音は先頭・中央・末尾の平均(公式はランダムな切り出し)
  - 言葉側は `scripts/clap_vocab.py` で事前計算した音色語 108 語(楽器・明るさ・質感・時間変化・動き・空間・雰囲気)を
    `data/clap_vocab.json` に同梱(言い回し 3 通りの平均、int8 + base64、92KB)。「どんな音にも近い語」が
    上位に来ないよう、合成音 36 個との類似度の平均・標準偏差も入れ、z 値で並べる
  - MCP `analyze_sound` の `words`(カテゴリごとに 3 語と z)。モデルが無ければ `words: null` と取得方法の案内
- **音を分析する能力の強化 D: 似た音を作る(2026-09-23)**: `glaux-engine/src/sound_match.rs` ほか。
  - 距離 `sound_match::compare`: 鳴り始めからそろえ(最大 3 秒)、いちばん大きい 5ms で音量をそろえ
    (全体の RMS だと余韻の長さで変わる)、約 11 / 21 / 43ms の窓(秒で決めるのでサンプルレートが違っても比べられる。
    振幅は窓の和で割る)の 48 帯域メルで「対数 L1 × 0.5 + スペクトル収束度」の平均 + 5ms ごとの dB 包絡の L1 / 20。
    音色の項は重なっている時間だけ、長さの違いは包絡の項に出す。目安: 0.15 未満ほぼ同じ / 0.35 未満よく似ている /
    0.7 未満似ている部分がある / それ以上かなり違う
  - 自動合わせ `fit_subtractive`: `cmaes` クレート(MIT/Apache、描画なし、rayon で並列評価)。連続つまみ 11 個
    (cutoff・attack・decay・release は対数、unison は 0〜1 を 1/3/5/7 に)を 0〜1 に写して探し、範囲外は罰則。
    初期値は記述子から(立ち上がり・減衰・持続・余韻・明るさ×3 をカットオフ・明るさの推移でフィルターエンベロープ・
    平坦さでノイズ)。波形は初期値での距離で上位 2 つだけ探す。候補の音は `render_subtractive`(ボイスを直接鳴らす。
    エフェクト・トラック音量は通さない)。既定 20 秒・150 世代・16 個体。既知のパッチはほぼ復元(距離 0.05)。
    FluidR3 の実楽器(ピアノ・ベース・フルート・リード・パッド)では 0.48〜0.75(subtractive で作れる範囲の限界)
  - 押していた時間の推定は「鳴っている長さ − 余韻」(`sound::estimate_hold`)。「減衰し続ける」音でも同じ式
    (鳴っている長さ全部にすると、ゆっくり立ち上がるパッドで大きくずれた)
  - プリセット検索 `preset_index`: `plugins::render_presets` が 1 つのプラグインでプリセットを順に 1 音ずつ鳴らす
    (起動中のインスタンスに直接 `load_preset` すると Surge XT は以後鳴らなくなるため、止まっている読み込み用の
    インスタンスで読み込み → `save_state` → 起動中の方に `load_state`。1 プリセット約 33ms)。索引は C4・1 秒押し・
    2 秒の要約(4 区間の対数メル 48 帯域 + 50ms ごとの包絡 40 点)と CLAP の埋め込み(int8)を
    `<設定>/glaux/cache/presets/<プラグイン>.json` に保存。時間の上限まで作り足し、続きは次回。
    検索は要約の距離と CLAP の近さの順位を融合(RRF)して 12 個に絞り、目標と同じ高さ・長さで鳴らし直して `compare` で並べる
  - MCP: `compare_sounds`(a / b。距離・観点ごとの違いと寄せ方・CLAP の近さ)、`match_sound`(subtractive を合わせて
    set_device 1 回。トラックのエフェクトも通した `verified_distance` も返す)、`find_similar_presets`(計 34 ツール)。
    UI は音声クリップのメニュー「この音に似せた内蔵シンセのトラックを作る」(Tauri `match_clip_sound`:
    直後に MIDI トラックを足し、同じ位置に目標の高さ・長さの 1 音。履歴 1 件)
- **レガート / ポルタメント(2026-09-24)**: `Articulation::Legato` / `Portamento`(追加のみなので FORMAT_VERSION は据え置き)。
  つなぎはデータ構築時の `data::link_legato`: 同じトラックで、そのノートより前に始まり終わりが開始の 0.3 秒手前以内の音
  (最も後に始まったもの、同時なら音程の近いもの)を「直前の音」とし、前の音の end をつなぎ目に揃えて `fade_out`、次の音に
  `fade_in`(どちらも 30ms)を付ける。ポルタメントはノートに自前のピッチカーブが無ければ、前の音程から 0.15 秒(音が短ければ
  長さ。2026-09-24 に「半分」から変更 — 8 分音符では 80ms と 400ms の差が出なかった)で 0 セントへ滑るカーブ(3 点、前半で 7 割)を付ける。ドラムのトラックはつながない。
  エンジン: `Voice` に fade_in / fade_out / age。fade_out のある音は end で離さず直線で消して解放、fade_in の音は直線で立ち上げ、
  さらに `VoiceState::skip_attack` で subtractive / fm / wavetable はエンベロープをサスティン(下限 0.35。fm は変調の深さも
  落ち着いた値、wavetable は position の掃引なし)から始める。サンプル系・pluck はフェードで立ち上がりを消す。
  CLAP へは前の音を end + fade_out で離して重ねて送り、ポルタメントのカーブは既存の Tuning 表現で届く。
  テスト: つなぎの判定(隙間 0.25 秒はつなぐ)、立ち上がりの山(通常 2.62 → レガート 1.05)、ポルタメントの途中 296Hz → 329.6Hz。
  UI はピアノロールの奏法キー T / P(マーカー ⌒ と /)
- **つなぎ方の調整(2026-09-24)**: `Track.glide_ms`(ポルタメントで滑る時間、10〜2000、省略 150)と `Track.legato_ms`
  (つなぎ目の長さ、5〜200、省略 30)。どちらも `set_param track/glide_ms` 等で設定、`unset_param` で既定へ(Track パスで
  unset できるのはこの 2 つだけ)。`Note.glide_ms`(ノート個別。トラックより優先)は add_notes / update_notes で、
  update_notes では 0 以下で個別指定を消す。すべて追加のみ・省略時は書かないので FORMAT_VERSION は据え置き。
  エンジンは `data::LegatoSettings`(トラックごと)と `NoteEvent.glide`(秒)を `link_legato` に渡す。
  UI: 音作りビュー(MIDI トラック)の「⌒ つなぎ」に 2 本のスライダーと「既定に戻す」、ピアノロールの見出しに
  ポルタメントのノートを選んでいるときだけ「滑る時間」(トラックの設定 / 40〜800ms)
- **レガート / ポルタメントで前の音の余韻を切る(2026-09-24)**: 前の音を離しても、リリースの長い音(パッド・弦・
  Surge のプリセット等)は余韻が次の音に重なって「鳴り続けて」いた。`NoteEvent.choke`(つなぎ目の長さ)を、つながった・
  滑り込んだレガート / ポルタメントのノートに付け、
  - 内蔵音源: 発音時に同じトラックの「離し済み(released)」のボイスを、その長さで直線に消す(押さえたままの音は触らない)
  - CLAP: 曲のノートで離した鍵盤を `plugin_released`(スロット, 鍵盤)に覚え、`plugin_choke_at[slot]`(開始 + つなぎ目)で
    押さえ直していない鍵盤に `NoteMsg::Choke` → CLAP の `NoteChokeEvent`(MIDI だけのプラグインには送れない)。
    ループ折り返し・停止で予定と記録を消す。Surge XT で余韻 0.20 → 0.0015 を確認
  あわせて「直前の音」の判定を修正: 次の音の前半より長く鳴り続ける音(押さえたままの伴奏)はつながない
  (以前は伴奏の低音を直前の音とみなし、伴奏を切った上に 19 半音下から滑らせることがあった)
- **直前の音が無いポルタメント(2026-09-24)**: フレーズの頭(または直前の音から 0.3 秒より離れた)ポルタメントは、
  以前は何も起きず普通の音として鳴っていた(気づきにくい)。今は `PORTAMENTO_SCOOP_SEMITONES`(2 = 全音)下から、
  同じ滑る時間で滑り込む。立ち上がりは普通(fade_in なし)。直前の音が無いレガートはこれまでどおり普通に鳴る
- **不具合修正: update_notes の pitch_curve が反映されていなかった(2026-09-24)**: 601ae63 の説明にある差し替えと
  検証が apply.rs に入っておらず、ピアノロールで描いたカーブ・AI の update_notes のカーブが捨てられていた
  (可逆性テストは「何も変わらない」ので通っていた)。update_notes で差し替え(逆コマンドは旧カーブ)、
  add_notes / update_notes で検証(最大 8 点・tick 昇順・±2400 セント、`model::check_pitch_curve`)。
  値が実際に変わることを確かめるテストを追加
- **似た音の道具の UI(2026-09-24)**: 音声クリップのメニュー「この音に近い CLAP 音源のプリセットを探す」→
  `SimilarPresetDialog.svelte`。CLAP 音源のトラックとカテゴリ(`clap_presets` の categories)を選んで探す → 候補(名前・
  カテゴリ・近さの言葉。距離はツールチップ)を「読み込む」(`clap_load_preset`)→「つまみを自動で合わせる」。
  Tauri `find_similar_clap_presets`(索引は 1 回 90 秒まで作り足し、途中経過は `preset-index` イベント)と
  `refine_clap_params`(自動選択のつまみ・20 秒、Author::Human の履歴 1 件)。探す処理は MCP と共通の
  `preset_index::similar_json` に切り出した(結果に verdict を追加)。人間の操作なので AI インジケータは点けない
- **内蔵ウェーブテーブル `wavetable`(2026-09-24)**: `glaux-dsp/src/wavetable.rs`。テーブル 5 種(analog = 正弦→三角→
  ノコギリ→矩形 / pulse = 幅 50%→5% / vocal = 母音あえいおう(フォルマント周波数を補間、基音 110Hz 想定)/
  sync = ハードシンク比 1→8 / organ = ドローバー式に倍音を足す)× 16 フレーム × 11 段のミップマップ(段 ℓ は倍音 1023>>ℓ まで)
  × 2048 サンプル(約 7MB)。倍音の設計図から逆 FFT(rustfft を glaux-dsp の依存に追加)で作り、段 0 のピークで
  フレームごとに正規化。`OnceLock` に初回の `bake_instrument`(UI スレッド)で 1 度だけ作り、ボイスは `get()` で読むだけ
  (未作成なら無音)。段はいちばん高い声部の周波数で「倍音 ≤ 0.45·sr/f」になる最も豊かなものを選ぶ。
  つまみ: table・position・pos_env / pos_decay(鳴り始めのずれと戻り)・lfo_rate / lfo_depth(position を揺らす)・
  unison / detune・cutoff / resonance(SVF)・ADSR(-60dB 基準)・gain_db。奏法 5 種(accent は position を +0.15)。
  出荷時プリセット(v3)に「ウォブルベース」「母音パッド」「シンクリード」、新エフェクトを使う「Lo-fi エレピ」を追加
- **質感系エフェクト delay / chorus / tape(2026-09-24)**: `glaux-dsp/src/effects.rs`。3 種はスロットごとの共有
  ディレイバッファ(2ch × 65536、`EffectState::default` で確保。64 スロットで約 32MB)を使う。
  delay: やまびこは毎回トーンの 1 次 LP を通る(回を重ねるほど暗い)、ping_pong は入力を左へ・左の返りを右へ。
  chorus: 線形補間の小数遅延、左右で LFO を 90° ずらす。tape: wow 0.55Hz(最大 ±2.4ms)・flutter 6.5Hz(±0.12ms)で
  左右共通に遅延を揺らし、tanh(x·drive)/drive の飽和 → 1 次 LP → ヒス(xorshift、種はリセットで固定)→ ビット落とし。
  tape は約 3.5ms の遅れを生む(PDC の対象外。気になる量ではない)。
  crackle(2026-09-24 追加): 1 サンプルごとに確率 (c²·30 + c·2)/sr でクリックを起こし(両方 60%・左右片方 20% ずつ)、
  振幅 (0.05 + 0.2c)·(0.15 + 0.85·u³) から 1 サンプルごとに ×(−0.45) で減衰させる(約 0.1ms の「プチッ」)。出荷時プリセット v4
- **プラグインの遅延補正 PDC(2026-09-24)**: `ClapProcessor::latency`(起動時に latency 拡張で取得)。
  レンダラの `compute_pdc` がブロックごとに、トラックの遅延(CLAP 音源 + CLAP エフェクトの合計)を求め、通常トラックは
  その最大に、バスはバス同士の最大に揃うよう `pdc_delay` を決める。遅延はチェーンの後・センドと音量パンの前に
  トラックごとの遅延線(`delay_stereo`、上限 `MAX_PDC` = 8192 サンプル、起動時に確保)で入れる。
  通常トラックの合算はバスの最大の遅延ぶん遅らせてからバスを足す(バス経由の音と揃う)。遅延がすべて 0 なら何もしない。
  全体の出力の遅れ(再生ヘッド表示・録音との関係)は補正していない。プラグインの `latency changed` 通知も未対応
  (起動時の値のまま)。テスト: 申告を 480 にしたトラックに対し、ほかのトラックがちょうど 480 サンプル遅れる
- **CLAP 音源のつまみの自動合わせ(2026-09-24)**: `plugins::PluginRenderer`(プラグイン 1 つを持ち回り、状態の読み込み →
  つまみを Param イベントで送る → 無音で反映・余韻消去 → 1 音鳴らす。プリセット検索の `render_presets` もこれに乗せ替え)。
  `sound_match::choose_plugin_params`(`module/name` の小文字で判定。既定はフィルター 1 のカットオフ・レゾナンス・FEG 量、
  アンプ EG の ADSR、フィルター EG のディケイ、ユニゾンのデチューン。shape / lfo / mute / solo / route / link は除く)と
  `fit_plugin`(CMA-ES を逐次評価。プラグインは Send でないので並列にしない。個体 10、既定 20 秒)。
  MCP `refine_plugin_params`(今の状態 + 上書き値から出発し、変わったつまみを set_param のまとめ 1 回で書く。計 36 ツール)。
  Surge XT は同じつまみで鳴らしても毎回少し違う(距離 0.008〜0.04 程度)ので、微妙な差は揺らぎに埋もれる。
  テスト: 初期音色を「サスティン 0 のプラック」にした目標から、サスティンを 1.0 → 0.0 と当て、距離 0.88 → 0.10
- **自動合わせの強化(2026-09-24)**: `sound_match::fit_instrument(…, FitInstrument, FitOptions{reverb})`。
  音源ごとの定義(`FitInstrument`: 次元・0〜1 → つまみの写像・記述子からの初期値・候補)を持つ。subtractive は波形 4 種、
  fm は周波数比の出発点(1 / 2 / 3.5 / 1.41。非調和・音程なしの音は非整数比から)を候補にし、初期値での距離で上位 2 つを探す。
  `reverb: true` で次元の後ろにリバーブの mix / size を足し、候補の音に内蔵リバーブ(`apply_reverb`)を通して比べる。
  MCP `match_sound` の instrument(auto 既定 = subtractive と fm を時間を半分ずつで探して近い方)と reverb、
  UI の「この音に似せた内蔵シンセのトラックを作る」は auto + リバーブ(約 30 秒)。
  音程が取れない非調和な音(ベル)は `timbre::dominant_pitch`(最も低い強いピーク)を音の高さにする(以前は 60 固定で大きく外れた)。
  既知の音: FM ベル(比 3.5)は比 3.49998 まで復元、リバーブ mix 0.5 / size 0.8 も復元。
  FluidR3 の実楽器(20 秒): ピアノ 0.50(fm)、ベース 0.63、フルート 0.54(fm)、リード 0.89、パッド 0.52(fm)。
  以前(subtractive のみ 20 秒)より fm 向きの音は良く、subtractive 向きの音は時間が半分になった分やや悪い
- **自動合わせに wavetable・評価回数での打ち切り(2026-09-24)**: `FitInstrument::Wavetable`(テーブル 5 種を候補に、
  position・pos_env・pos_decay・cutoff・resonance・ADSR・detune・unison の 11 次元)。auto は 3 音源で時間を等分。
  探索は時間ではなく評価回数で打ち切る: 上限 = (max_seconds/2) × 1200 ÷ 目標の秒数(`AUDIO_SECONDS_PER_SECOND`)と
  世代数 × 候補数の小さい方。時間は安全のための上限(max_seconds の 4 倍)だけ。以前は時間で打ち切っていたため、
  CPU が混むと評価回数が半分ほどになって結果が変わった(並列テストで 3 回に 1 回失敗)。今は同じ入力なら同じ結果。
  テスト: vocal テーブル position 0.6 の音から table・position を復元(距離 0.001)、2 回やって同じ結果
- **内蔵 FM シンセ `fm`(2026-09-24)**: `glaux-dsp/src/fm.rs`。2 オペレーター(モジュレーター → キャリア)+ モジュレーターの
  自己フィードバック(直前 2 サンプルの平均で発振を抑える)。つまみ: ratio(0.5〜16、非整数で非調和 = 金属的)、index(0〜12)、
  index_decay / index_sustain(変調の深さの包絡。エレピ・ベルの「鳴り始めだけ硬い」)、feedback、ADSR(decay / release は
  その時間で -60dB)、gain_db。ベロシティで変調の深さも変わる(0.4 + 0.6·vel)。奏法は subtractive と同じ 5 種
  (palm_mute は減衰 4 倍速)。UI の音源メニュー・音作りビュー、MCP の説明・チャットに追加
- **スウィングの一括適用(2026-09-24)**: `glaux_core::rhythm::swing_positions`(純粋関数)。拍の組(2 × grid)の
  裏(組の頭から grid/2〜3/2·grid)にある音を、組の頭から 2·grid·swing の位置へ strength だけ寄せる(絶対位置なので
  同じ設定なら何度掛けても同じ、0.5 でストレートに戻る)。拍は曲頭から数える(clip.start を足して判定)。
  analyze_rhythm の swing_ratio(裏 8 分の位置 / 480)は swing × 2。MCP `swing_notes`、Tauri `swing_clip`、
  UI はピアノロールの見出しの「スウィング」(8 分 / 16 分と率。選択中のノート、無ければクリップ全体)
- **ステレオ音声(2026-09-24)**: 取り込み・再生・テンポ追従・パート分離・録音でステレオを保つ。
  - `SampleData` に `side: Option<Vec<f32>>` を追加。`frames` はこれまでどおりモノラル成分 M = (L+R)/2、ステレオ素材だけ
    左右差成分 S = (L−R)/2 を持つ(L = M+S、R = M−S)。解析・譜起こし・波形表示・サンプラーは M だけを使うので変更不要、
    メモリの増分は 1 チャンネルぶん。`load_wav` はステレオ(2 ch)なら S も持つ(左右が同一なら持たない)、
    `load_wav_mono` は S を捨てる(解析用)。`SampleBank` は `load_wav`。3 ch 以上はモノラルに合算
  - 再生: 音声クリップの S をトラックごとの `blk_side` に溜め、チェーンの入力で L = M+S、R = M−S に戻す。
    ステレオ素材を含むトラック(`TrackMix.stereo`)はパンを左右バランス(√2)として掛ける。サイドチェインの検出は M
  - テンポ追従: `stretch::wsola_channels` で M の切り貼り位置を S にも使う(定位が崩れない)
  - 分離: `separate::hpss_channels` で M からマスクを作り S にも掛ける。Demucs にはステレオで渡し、元がステレオなら
    ステレオのパートを受け取る。パートはステレオの WAV で取り込む(`write_wav_ms`)
  - 録音: 設定「ステレオで録音する」(既定オフ)で、入力が 2 ch 以上なら最初の 2 ch を録る。リングは左右を組で積む
    (`push_pair`、溢れるときは組ごと捨てる)。自動音量調整は左右それぞれのピークで決める
  - 取り込み(MP3 等 → WAV)はもともと元のチャンネル数のまま保存していた
- **センド / リターン(2026-09-24)**: 複数トラックでリバーブ・ディレイを共有する。
  - モデル: `TrackKind::Bus`(クリップを持たない。AddClip は ClipKindMismatch)、`Track.sends: [{target, level_db, pre_fader}]`
    (送り先 ID 順、空なら書き出さない)。足すだけの変更なので `FORMAT_VERSION` は 1 のまま(古いファイルはそのまま読める。
    バスを含むファイルは古いアプリでは読めない)。バスを消してもセンドは残る(送り先が無いセンドは鳴らさないだけ。取り消しで元通り)。
    サイドチェインの source と同じ扱い
  - コマンド `set_send`: バス → バスは循環を避けるため禁止
  - エンジン: `TrackMix { is_bus, sends: [SendMix{target(添字), amp, pre_fader}] }`。`process_track_chains` を 2 回に分け、
    通常トラック(チェーン → 音量/パン → マスター前の合算 + センド)→ バス(センドの合算を入力に チェーン → 音量/パン)。
    フェーダー前のセンドはチェーンの後・音量パンの前、後のセンドはトラックの出力(音量・パン込み)から。
    バスはステレオ入力なのでパンは左右バランス(√2)。バスはソロの影響を受けない(ソロにしたトラックの残響が消えない)。
    バスのミュートはチェーン処理を省いて止める(バスには発音が無いため)。バスの入力バッファは起動時に確保(トラック数 × MAX_FRAMES)
  - UI: タイムライン下の「+ 🔀 バス」(リバーブ mix 1.0 を挿して作る)、トラック見出しに BUS とセンド元の数。
    音作りビュー: 通常トラックに「🔀 センド」(バスごとに -60〜+6 dB、フェーダー前、送らない)、バスには受けているトラックの一覧
  - 解析でトラックを単体で聴く(analyze_audio の track_ids)ときは、そのトラックのバス経由の残響は含まれない
- **CLAP エフェクト(2026-09-24)**: トラック・マスターのエフェクトチェーンに CLAP プラグインを挿せる
  (`Effect.source = Clap { plugin_id, state }`。スキーマは変更なし、内蔵エフェクトと混在可)。
  - レンダラをブロック単位のチェーン処理に組み替えた: (1) フレームごとに発音・音声クリップ・ライブ演奏を回し、
    トラックごとのエフェクト前の合算を `blk_mono`、エフェクトを通さない分(MAX_TRACKS 超・試聴)を `blk_direct`、
    クリックと各フレームの再生位置を溜める → (2) `process_track_chains`: トラックごとに 入力(楽器 + プラグイン出力)
    → `run_chain` → 音量/パン。内蔵エフェクトはバッファ上をサンプルごと、CLAP エフェクトは入力口(`input_mut`)に書いて
    ブロックごと `process` → (3) `process_master`: マスターのチェーン → 音量 → ソフトクリップ。
    ループの折り返しでオートメーションのカーソルは `blk_pos` の逆戻りを見て戻す。バッファは起動時に MAX_FRAMES で確保
    (アロケーションなし)。負荷はベンチで変更前と同等(all: 平均 5.9% → 5.7%)
  - CLAP エフェクトのスロットは音源と同じプール。`PluginOwner`(`Track(TrackId)` / `Effect(FxId)`)で持ち主を表し、
    `PluginManager::sync` / `OfflinePlugins::create` / `live_values` / 画面 / 状態保存はすべて持ち主単位。
    `project_plugins` がプロジェクトの全 CLAP(バイパス中のエフェクトも。切り替えで作り直さない)を列挙する。
    焼き込みでは `BakedEffect { params: EffectParams::External, plugin: Some((slot, gen)) }`。用意できないプラグインは焼かない(素通し)。
    レンダラはブロック頭に `slot_is_fx` を作り、音源の一括処理からエフェクトのスロットを外す
  - 入力: `ClapProcessor` はメイン入力ポートを覚え(`main_in`)、入力は `is_constant: false` で渡す。モノラル入力には左右の平均。
    プラグインの遅延補正(PDC)は下記(2026-09-24)
  - **CLAP エフェクトは毎ブロック必ず `process` を呼ぶ**(鳴っていないトラック・バイパス中も、無音を入れて。`keep_effects_alive`)。
    呼ばないと Surge XT Effects が Windows で画面を開くときに音声処理側の応答を待って固まり、「プラグインが応答しません」になった
    (「処理中」にしたまま process を呼ばないホストを想定していない)。音源・バイパス中のエフェクトには入力を無音にしてから渡す
  - つまみ: `fx/<id>/clap:<param id>`(上書き値は音源と同じく SetParams で送る)。オートメーションはブロック頭に
    `NoteMsg::Param` として積む(`push_plugin_param`)。MCP `list_params` の effects に name: "clap"・plugin_name・つまみ(最大 64 個)・
    missing(プラグインが見つからない)。track_id なしの list_params は master_effects も返す
  - 状態: 新コマンド `set_effect_state`。アプリは画面での変更を `set_effect_state` + 上書き値の `set_param` / `set_master_param`
    のまとめ 1 件で保存。get_project はエフェクトの状態も省略表示し、add_effect / add_master_effect / set_effect_state で
    省略表示のまま送られたら今の状態に戻す。プリセットは list_plugin_presets / load_plugin_preset に fx_id
  - テスト: `GLAUX_TEST_CLAP_FX`(例 Surge XT Effects。既定の効果は Delay)で、入力が通る・トラック / マスターで効く・
    バイパスで完全に素通し・つまみが一覧に出て動かせる
  - UI(音作りビュー): 「エフェクトを追加…」に「CLAP プラグイン」のグループ(`clap_plugins` の effect: true)。
    CLAP エフェクトの行は「CLAP」の目印 + プラグイン名、「画面」ボタン(`clap_open_gui {fxId}`)、つまみは current_text
    (プラグイン自身の表示)を優先、64 個を超える分は案内だけ。見つからないプラグインは注意を出す。
    CLAP エフェクトの行の「プリセット」でプラグインのプリセットをカテゴリ別の一覧から選べる
    (Surge XT Effects は preset-discovery も preset-load も持たない = Glaux からは 0 件。FX プリセットは
    プラグインの画面の中だけで扱われる。画面で選べば状態として保存される。0 件のときはそう案内する)
    (Tauri `clap_presets` / `clap_load_preset` に fxId。読み込みは set_effect_state + 上書き値の削除 + preset 名のまとめ 1 件)
- **CLAP プラグイン(外部の音源)第 1 段階(2026-09-23)**: 新クレート `glaux-clap`
  (`clack-host` / `clack-extensions` 0.2、MIT OR Apache-2.0)+ `glaux-engine/src/plugins.rs`。
  - 探索: `GLAUX_CLAP_PATH` → `CLAP_PATH` → OS 標準(Windows は `%COMMONPROGRAMFILES%\CLAP` と
    `%LOCALAPPDATA%\Programs\Common\CLAP`、Linux は `~/.clap` と `/usr/lib/clap`)を再帰的に探し、
    記述子(ID・名前・ベンダー・features)を一覧にする(初回だけ。`rescan` で探し直し)
  - スレッド: プラグインのメインスレッド `glaux-plugins` が生成・起動(`activate`)・状態・画面・破棄を
    すべて担う(`ClapPlugin` は Send でない)。処理窓口 `ClapProcessor` は `PluginSlot`
    (`AtomicPtr` の受け取り口・返却口 + 世代番号、最大 16 スロット)でオーディオスレッドへ渡し、
    使い終えたら返却口に戻してメインスレッドが止めて解放する(オーディオスレッドで解放しない)。
    thread-check 拡張には「作ったスレッド = main」「process を呼ぶスレッド = audio」と答える
  - `PluginManager::sync`(`set_project` のたび): CLAP 音源のトラックに (スロット, 世代) を割り当てて
    生成を指示し、消えた・差し替わった・サンプルレートが変わったものは破棄。プロジェクト側の状態が
    外から変わった(取り消し等)ときだけ読み込み直す(状態のハッシュで判定)。見つからない
    プラグインは内蔵 subtractive で代用
  - レンダラ: プラグインのトラックは内蔵ボイスを作らず、`collect_plugin_notes` がブロック内の
    ノートを本処理と同じ規則(ループ折り返し含む)でなぞって CLAP の note on/off を時刻付きで積み、
    ブロック単位で `process`(最大 4096 フレーム、長いブロックは分割)。note off 待ちは
    `plugin_pending`(固定 1024)。停止・シーク・データ差し替えで曲のノートを離す。
    出力はステレオのままトラックのエフェクト → 音量/パン(ステレオはバランスとして √2 倍)。
    試聴・MIDI キーボードもプラグインへ送る。プラグインがあれば停止中も処理し続ける(余韻)。
    出力デバイスの切り替えでレンダラが作り直されるときは `Drop` で窓口を受け取り口へ戻して引き継ぐ
  - ノートの方式: ノートポートの優先方式が CLAP なら CLAP のノートイベント、そうでなければ MIDI。
    宣言された全音声ポートにバッファを用意し、メイン出力(IS_MAIN)を使う
  - 書き出し・解析: `OfflinePlugins` が呼んだスレッドで専用インスタンスを作る(状態を読み込んで起動)
  - 状態: `PluginSource::Clap.state` に base64。プラグインが mark_dirty したとき・画面を閉じたときに、
    アプリのバックグラウンドタスクが 1.5 秒ごとにまとめて `set_device` で保存(履歴 1 件、取り消し可)。
    手動保存は `clap_save_state`
  - 画面(Windows のみ): プラグインのスレッドで素のトップレベルウィンドウ(`glaux-clap/src/window.rs`、
    windows-sys)を作って `set_parent` で埋め込む(JUCE 製などは埋め込みのみ対応のため)。埋め込み非対応で
    浮動に対応するものは浮動で開く。メッセージはプラグインのスレッドで `PeekMessage` を回す
    (画面を開いている間は 8ms 周期)。閉じるボタンは隠すだけにして、画面の破棄 → ウィンドウの破棄の順。
    大きさの要求・利用者のリサイズを相互に伝える。Linux / macOS は未対応(X11 は posix-fd・timer 拡張が要る)
  - Tauri: `clap_plugins` / `clap_open_gui` / `clap_close_gui` / `clap_save_state`。UI は音源メニューの
    「🔌 CLAP プラグイン」欄(音源のみ)、トラック見出しの 🖥、音作りビューの CLAP 欄(つまみ欄は隠す)
  - MCP: `list_plugins`(計 27 ツール)。`get_project` は CLAP の状態を「(省略: … N 文字)」と
    省略表示し、その表示のまま `set_device` で送り返されたら今の状態に戻す
  - テスト: `GLAUX_TEST_CLAP` に音源の `.clap` を指定したときだけ実プラグインで動く(glaux-clap の
    発音と状態、エンジンの書き出しとリアルタイム経路の受け渡し・返却)。サンドボックスで Surge XT 1.3.4
    (Linux 版)を使って確認済み。**画面(Windows)は実機未検証**(Windows 向けのコンパイルは確認済み)
  - 第 2 段階の予定: エフェクトプラグイン(トラック・マスター)、Linux の画面
- **CLAP 第 2 段階: つまみ・ペダル・ピッチベンド(2026-09-23)**:
  - パラメータ: `ClapPlugin::param_infos`(params 拡張: id・名前・所属・範囲・既定・stepped /
    automatable / hidden / readonly)と `param_values`(今の値と `value_to_text` の表示文字列)。
    AI・UI に見せるのは automatable かつ hidden / readonly でないもの(Surge XT で 774 個)
  - プロジェクト上の表現: `device.params` に `clap:<id>` キーの「上書き値」(プラグインの単位)。
    パスは `device/clap:<id>`(`set_param` / `unset_param` / オートメーションの target)。
    `PluginManager::sync` が上書き値の差分を `SetParams` で、状態が変わった・上書きが消えた
    (取り消し)ときは `Reload`(状態を読み込み直してから残りの上書きを送る)でプラグインのスレッドへ。
    プラグインのスレッドはスロットの `ParamQueue`(SPSC、512)に積み、レンダラがブロック頭で
    CLAP の param value イベントにして送る。生成時の上書きも窓口を置く前に積む(書き出しも同様)
  - 状態の保存(画面を閉じた・mark_dirty)では、上書きしているパラメータを今の値に揃えて同じ
    `set_device` に含める(画面で動かした値を古い上書きで戻さないため)
  - 今の値の共有: プラグインのスレッドが読み込み後・変更後(120ms 後)・dirty 時に全公開パラメータを
    読み、`plugins::live_values(track)` に置く。`param_infos(plugin_id)` はキャッシュ(未読み込みなら
    呼んだスレッドで一時インスタンスを作って調べる)
  - オートメーション: `TrackMix::plugin_auto`(`device/clap:<id>` のレーン、最大 32)。レンダラが
    64 サンプルごとに評価し、変わったときだけ param value イベントを送る
  - ピッチカーブ: カーブのあるノートは note_id を付けて鳴らし、CLAP のノート表現(TUNING、半音)を
    発音時と 64 サンプルごとに送る(`PitchCurve::cents_at` を公開)。奏法(vibrato / bend 等)は未転送
  - MIDI キーボード: サステイン(CC64)とピッチベンド(`LiveEvent::PitchBend`、パックを 3 ビットの
    種類に変更、`LIVE_NO_TRACK` は 0x1FFF)を送り先がプラグインのトラックなら MIDI イベントで送る
    (MIDI を受けないプラグインには送らない)。ピッチベンドの MIDI 録音は未対応
  - イベントの並び: ブロック内で (時刻, 種類) 順に `sort_unstable`(離す → パラメータ/MIDI → 鳴らす → 表現)
  - MCP `list_params` に `filter` / `limit`(CLAP は既定 80 件、`params_total` 付き)。各つまみに
    `current_text`。UI のオートメーションレーンはつまみが 40 個を超えると絞り込み欄を出す
  - 限界(プラグイン側の仕様): CLAP のパラメータで触れるのはプラグインが公開したつまみだけ。
    Surge XT は LFO のテンポ同期(Rate の右クリック設定)やモジュレーションの割り当て(ドラッグ)を
    パラメータとして公開していないので、AI からはできない(画面で人間が行う)
  - プリセット(2026-09-23 調査): Surge XT 1.3.4 は CLAP の preset-discovery ファクトリ(/2)と
    preset-load 拡張(/2)に対応。プロバイダ 1 個("Surge XT Presets")、ファイル種別 `.fxp`、
    場所はユーザー(`~/.Surge XT/Patches` 等)+ データフォルダに `patches_factory` / `patches_3rdparty`
    があれば工場出荷・サードパーティも宣言する(ソース `SurgeCLAPPresetDiscovery.cpp` で確認。
    Linux のプラグイン単体配布には無いので未宣言)。メタデータは名前・作者のみ(load_key は空、
    カテゴリはフォルダ名で代用が必要)。`ClapPlugin::load_preset_file` で読み込みを確認
    (Bell Pad で 120 個のつまみ・エフェクト・テンポ同期値まで変わる)
  - プリセットの一覧と読み込み(2026-09-23 実装): `glaux_clap::list_presets`(preset-discovery の
    プロバイダを作り、宣言された置き場所を拡張子でたどってファイルごとにメタデータを受け取る。
    カテゴリは置き場所からのフォルダ名。プラグイン ID で絞る)、`PresetEntry::id`(ファイルならパス、
    本体内なら `plugin:<load_key>`)。エンジンの `plugins::presets` がプラグイン ID ごとにキャッシュ。
    読み込みは `plugins::state_with_preset`: 呼んだスレッドで一時インスタンスを作り、プロジェクトの状態 →
    プリセットを読み込んで状態を作る → `glaux_mcp::clap_presets::load_command` が `set_device` 1 件
    (CLAP の上書き値 `clap:*` は消し、`params.preset` にプリセット名)。再生中のプラグインは同期の
    Reload で読み込み直す。取り消しで元の音色へ
  - MCP: `list_plugin_presets {track_id, filter, category, limit, rescan}`(categories・current_preset 付き)/
    `load_plugin_preset {track_id, preset}`(計 29 ツール)。Tauri: `clap_presets` / `clap_load_preset`。
    UI は音作りビューの CLAP 欄にプリセット一覧(カテゴリ選択・名前で絞り込み・今のプリセットを強調)
  - `list_params` の `current_text` は、上書き値がプラグインに届いた後(共有表の値と一致したとき)にも返す
    (2026-09-23 修正。以前は上書きしたつまみの表示文字列が返らなかった)
  - 実プラグイン(Surge XT)で確認: 上書き値(Global Volume 0 dB → -48 dB)、オートメーションでの音量変化、
    ピッチカーブで 440 → 880 Hz、ピッチベンド最大で 440 → 493.8 Hz、ペダル中の保持と解放、
    `list_params` の絞り込み("cutoff" → 4 件)と set_param
- **和音の譜起こし(basic-pitch、2026-09-23)**: 新クレート `glaux-ml`。
  - モデル: spotify/basic-pitch の `nmp.onnx`(230KB、Apache-2.0)を `crates/glaux-ml/models/` に
    同梱し `include_bytes!`。推論は pure Rust の `tract-onnx` 0.23(ネイティブ DLL 不要。
    Windows では tract-linalg のアセンブリを MSVC の `ml64.exe` で組む)。モデルは初回に
    最適化して `OnceLock` に保持。dev ビルドでも tract 系は opt-level 3
  - 前処理・後処理は公式(`inference.py` / `note_creation.py`)の移植: 22.05kHz に窓付き sinc で
    リサンプル → 2 秒窓(重なり 30 フレーム、先頭に半分の無音)→ 窓の両端 15 フレームを捨てて連結 →
    onset(note 活性の立ち上がりで補強)の時間方向の極大 ≥ 0.5 から note ≥ 0.3 が続く限り伸ばす
    (途切れ許容 11 フレーム、最短 11 フレーム、隣接半音も消費)→ 残りのエネルギーから
    melodia trick。フレーム → 秒は窓ごとの端数補正込み
  - 検証: 同じ入力で onnxruntime との活性の差が 1e-6 以下、ノート列が公式の
    `output_to_notes_polyphonic` と完全一致(スクラッチで確認。リポジトリのテストは合成和音)
  - 配線: `glaux_mcp::transcribe::TranscribeMode { Melody, Poly }`、MCP `transcribe_audio` の
    `mode: "poly"`、Tauri `transcribe_clip` の `mode`、UI は音声クリップの ♫ ボタン。
    和音用の tick 変換 `to_clip_notes_poly`(同じ音高の重なりだけ詰める)。ベロシティ = 活性 × 127
- **ステム分離(2026-09-23)**: `glaux_mcp::stems`(UI と MCP で共用)。
  - 内蔵 `builtin`: `glaux-engine/src/separate.rs` の HPSS(STFT 2048/512、時間・周波数方向の
    17 点メディアン、ウィーナー型ソフトマスク)。打楽器 / 音程楽器 の 2 本、足すと元に戻る
  - 外部 `demucs`: Demucs(htdemucs)を `demucs` → `python -m demucs` → `py -m demucs` の順に探して
    起動(Windows はコンソールを出さない)。クリップの参照範囲を `cache/stems/` に書き出して渡し、
    ボーカル / ドラム / ベース / その他 を読み戻す(44.1kHz なら元のレートにリサンプル)。
    未インストールならインストール方法を案内するエラー。サンドボックスで CPU 版 torch + demucs を
    入れて通し確認済み(6 秒の合成音で 38 秒、モデルの初回ダウンロード込み)
  - 分けた音は元トラックの直後に「<トラック名> <パート>」の音声トラックを作り、元クリップと
    同じ位置・長さ・音量・フェード・テンポ追従で置く。元トラックはミュート。全体で 1 件の履歴
  - MCP `separate_audio {clip_id, method}`、Tauri `separate_clip`、UI は音声クリップの右クリック
    「🎚 パートに分ける」(内蔵 / Demucs)。処理中はクリップに「パートに分離中…」
- **音声クリップのテンポ追従(2026-09-23)**: `Stretch::Follow { original_bpm }` を実装。
  素材は元テンポ一定で演奏されたものとして扱い、1 tick = 60/(original_bpm×PPQ) 秒ぶんの素材が
  常に 1 tick に対応する(`Stretch::follow_seconds`)。
  - 伸縮は `glaux-dsp/src/stretch.rs` の WSOLA(40ms フレーム・50% 重なり・±10ms 探索、
    正規化相互相関を 4 サンプル間引きで粗探索 → 1 サンプルで詰める)。音程は変わらない。
    3 分の素材で約 0.3 秒(glaux-dsp は dev ビルドでも opt-level 3)
  - `SampleBank::sync` がクリップごとに条件(素材・offset・位置・長さ・元テンポ・テンポマップ)の
    ハッシュ付きで伸縮済み波形をキャッシュし、変わったものだけ作り直す。クリップ全体で
    テンポが元テンポと同じなら伸縮せず元素材を使う。再生・書き出し・解析のどれも同じ波形
  - 分割(`split_clip`)の右側の offset、波形表示(`clip_peaks`)、譜起こしの範囲・tick 換算は
    追従中なら元テンポ基準で計算する
  - UI: 音声クリップの右クリック「⇔ テンポに追従させる」(元テンポ = クリップ先頭の今のテンポ)/
    解除。追従中はクリップ名に「⇔120」のように表示
- **マスターのオートメーション(2026-09-23)**: `MasterBus.automation`(空なら JSON に出ない)と
  コマンド `set_master_automation_points`。UI はタイムライン最下段の「マスター」行の 〜 で
  レーンを開く(`AutomationLaneRow` に擬似トラック `MASTER_FOCUS_ID` を渡す。音量 + マスターの
  エフェクトのつまみ)。ついでに `tests/history.rs` の `random_command` が深さ 0 で Batch を
  生成していなかった(範囲の取り違え)のを修正
- **MIDI キーボード入力(2026-09-23)**: `glaux-engine/src/midi.rs`(依存 `midir` 0.11。
  cpal 0.18 と alsa 0.11 を共有できる版)。
  - ライブ演奏: midir の受信コールバック → `LiveQueue`(固定 256 の AtomicU32 リング。
    取り出しはロックフリー、積む側だけ短いロックで直列化)→ レンダラがブロック頭で取り出し、
    `LiveVoice`(固定 32、満杯なら離した音から捨てる)を送り先トラックの楽器で発音して
    `track_mono` に足す = トラックのエフェクト・音量・パンを通る。停止中も鳴り、
    最後の音の後 4 秒は残響のためにレンダリングを続ける。チャンネルはオムニ。
    CC64 サステイン、CC120/123 で全消音。note off は送り先を問わず同じ音高を離す
  - 送り先: `Shared::live_track`(index)。ハンドルは TrackId で持ち、`set_project` のたびに
    index を解決し直す。UI はトラック見出しの 🎹(MIDI トラックのみ、1 つだけアーム)→
    ピアノロールで開いているトラック → 最初の MIDI トラック の順で決めて `set_live_target`
  - MIDI 録音: 🎹 アーム中は ⏺ が MIDI 録音(`midi_record_start` / `midi_record_stop`)。
    受信時刻は `Shared::audible_pos()`(レンダラが書いた `pos` から 1 ブロック戻し、
    書き込みからの経過時間で補間。デバイス固有の出力遅延は含まない概算)で tick 化。
    `pair_notes`(ペダル中の note off は踏み終わりまで延長、打ち直しは前を切る、未解放は停止位置)
    → `take_to_notes`(カウントイン中の食い気味は 16 分以内なら頭へ、位置合わせは設定で
    しない / 16 分 / 8 分 / 3 連 8 分)→ クリップ長は小節単位に切り上げて `add_clip` 1 件
  - Tauri: `midi_inputs` / `set_midi_input` / `set_live_target` / `midi_record_start` /
    `midi_record_stop`、`transport_state` に `midi_recording` / `midi_idle_ms`。
    設定パネルに「MIDI キーボード」(入力選択・受信ランプ・位置合わせ)、フッターに 🎹 機器名。
    選んだ入力は localStorage(`settings.midiInput`)に保存し起動時に再接続
  - **サンドボックスに MIDI 機器が無いため実機未検証**(キュー・解釈・対組み・レンダラの
    ライブ発音 / ペダル / 上限は単体テスト済み、UI はハーネスで確認)
- **ループクリップ / 拍子の UI(2026-09-23)**:
  - ループ: `ClipContent::Midi` に `loop_len: Option<Tick>`(既存の `loop` フラグと組で使う)、
    コマンド `set_clip_loop`。展開は `Clip::playback_notes()` に一元化し、再生(engine data.rs)・
    和声分析・リズム分析がこれを使う(ループ境界をまたぐ音は境界で切る、ループ範囲より後ろの
    ノートは鳴らない)。UI: クリップ右クリック「ループにする(今の長さを繰り返す)/ 解除 /
    繰り返しをノートに展開」、名前に 🔁、縮小表示は 2 回目以降を薄く + 境目の線。
    ピアノロールは繰り返す 1 回分だけを編集範囲にし、再生ヘッドは 1 回分の中に畳んで表示。
    ループ中のクリップの分割は、同じ Batch 内で先に replace_clip で展開してから split_clip
  - 拍子: ルーラー右クリックで「N 小節目から拍子を変更」(分子・分母、4/4・3/4・6/8・7/8・5/4・12/8
    のプリセット)、拍子チップのクリックで同じメニュー + 「この拍子変更を削除」。
    set_time_sig の events を丸ごと作り直し、直前と同じ拍子の変更は取り除く
- **タイムラインのクリップ操作(2026-09-23)**: クリックで選択(Ctrl / Shift で追加・除外、
  Ctrl+A で全選択、空白クリック / Esc で解除、選択は枠線表示)、複数選択のまとめドラッグ移動
  (トラック間移動は 1 つのときのみ)、Delete で削除、S で再生ヘッド位置で分割、
  Ctrl+C / X / V(再生ヘッドを 1 拍に丸めた位置・元のトラックへ貼り付け)、Ctrl+D で直後に複製、
  右クリックメニュー(ここで分割 / 再生ヘッドで分割 / 複製 / コピー / 切り取り / 削除)。
  すべて既存コマンド(move_clip / split_clip / remove_clip / add_clip)の Batch で 1 undo。
  複製・貼り付けはクリップとノートに新 ID を振る。キーはピアノロール表示中・入力中は無効
  (ピアノロール側のノート操作に譲る)
- **オーディオ負荷の計測(2026-09-23)**: 「再生中に音がプツプツ途切れる」報告への切り分け用。
  `Renderer::process` がブロックごとに処理時間を測り、`Shared::stats`(アトミック)に
  平均・最大負荷と累計回数(処理落ち = 計算が予算超過 / 呼び出し遅延 = 前回から
  ブロック長の 1.8 倍以上空いた = 他プロセスに CPU を奪われた / 再生中のデータ差し替え)を
  記録。`transport_state` の `dsp` で UI に渡し、トランスポート横に「DSP n%」
  (処理落ち・遅延があれば ⚠ と回数)を表示。あわせてオーディオスレッドで FTZ/DAZ
  (デノーマル丸め)を有効化。ベンチ(`render_bench`、実プロジェクト .glaux も可)では
  CyberNeon 7 トラックで平均 5%。サンドボックス VM はホストと CPU を共有するため、
  サンドボックスでビルド中はホストの再生が途切れうる点に注意
- **譜起こし 第 3 弾(2026-09-23、実録音「テッテレー」で検証)**: 消えていたのは 80〜100ms の
  短い音。子音直後に音程が 3 半音ほど下がるため音程変化で 20〜40ms に割れ、各断片が
  最短長未満として捨てられていた。対策: `merge_short_runs`(隙間なく続く短い断片を
  隣とまとめる)、独立した短い音は 50ms で残す、`split_at_dips`(2ms 包絡で 25dB・
  30ms 以上の谷があれば別の発音 = 同音でも結合しない。解析窓 40ms が短い休符を
  覆い隠すため)、半音変化の判定を「現ノートの中央値」基準に(半音の中間で歌っても
  割れない)、音程は冒頭 30% と末尾 20% を除いた中央値。実録音で声のある 16 区間
  すべてがノート化されることを確認。診断ツール
  `cargo run -p glaux-engine --example transcribe_check -- <wav> [bpm] [frames]`
  - タイミング: 同じプロジェクトの録音 8 本で、クリップ頭から歌い出しまでが
    テンポに関係なく +0.12〜+0.33 秒(中央値 約 0.25 秒)。実際の往復遅延が
    レイテンシ補正の既定 60ms よりずっと大きい(Bluetooth 出力等)と考えられる。
    録音の生データの無音部がデジタルゼロ = OS 側のノイズ抑制が掛かった入力。
    対策候補: 設定値を 250ms 前後に上げる / タップで測るキャリブレーション機能
- **チャットの AI モデル選択(2026-09-22)**: チャットパネルのヘッダーで 既定 / Opus / Sonnet /
  Haiku / 任意のモデル名 を選ぶと、次の指示から `claude -p ... --model <名前>` で起動する
  (`ChatManager::set_model`、`send_chat` の `model` 引数)。`--resume` と併用できるので
  会話の文脈は引き継がれる。モデル名は英数字と `. _ - [ ]` のみ許可(`valid_model_name`、
  別オプションの注入防止)。選択は localStorage(`settings.chatModel`)に保存
- ~~音声クリップ再生・録音~~ → **実装済み(2026-09-22)**:
  - 再生: `AudioEvent` を固定プール(16)で線形補間再生。offset / gain_db / fade を反映。
    シーク・ループ折返し・一時停止からの再開・編集によるデータ差し替えのいずれでも
    途中から途切れず鳴る(`Renderer::resync_audio`)
  - 録音: `record.rs`。既定の入力デバイス → ロックフリー SPSC リング(AtomicU32)→
    書き込みスレッドがモノラル 16bit WAV。`EngineHandle::start/stop_recording`。
    Tauri `record_start`(再生も開始)/ `record_stop`(音声トラックに配置、無ければ
    「録音」トラックを新設)。UI は ⏺ ボタン(録音中は点滅)。**サンドボックスには
    入力デバイスが無いため実機未検証**(リング・WAV 書き出しは単体テスト済み)
  - 取り込み: `assets::audio_clip_commands`、MCP `import_audio_clip`、
    UI は「+ 🎵 音声トラック」+ 空きレーンのダブルクリックで WAV 配置
  - 波形表示: `AudioClipPreview.svelte`(Tauri `clip_peaks` がクリップ参照範囲の
    min/max ピークを返す。クリップ ID + 範囲 + 幅でキャッシュ)
  - タイムストレッチ(Stretch::Follow)・ループクリップは 2026-09-23 に対応
- **譜起こし(単旋律 → MIDI)実装済み(2026-09-22)**: `glaux-engine/src/transcribe.rs`。
  「鼻歌を録音して即 MIDI 化」が主用途。10ms フレームで YIN(CMND しきい値 0.15 +
  放物線補間)→ 音量(-40dB 以内)と明瞭度で有声判定 → 中央値フィルタ(窓 5)→
  「半音変化が 3 フレーム安定 / 8dB の立ち上がり / 無声 2 フレーム」で区切り →
  80ms 未満は捨てる → テンポマップで tick 化(既定 1/16 クオンタイズ)。
  配線: `glaux-mcp/src/transcribe.rs`(共通)、MCP `transcribe_audio`、
  Tauri `transcribe_clip`、UI は音声クリップの ♪ ボタンと録音直後の「♪ MIDI 化」。
  合成した鼻歌信号(倍音 + ビブラート + ノイズ)でメロディ・再アタック・低い声・
  tick 変換をテスト。**和音は basic-pitch(`glaux-ml`)、ステム分離は HPSS / Demucs で対応済み(2026-09-23)**
  - 実機フィードバック(細切れ・抜け・タイミングずれ)への対策(2026-09-22):
    明瞭度しきい値 0.5→0.35、音量フロア -40→-45dB、無声許容 20→60ms、
    半音変化の判定にヒステリシス(±0.7 半音)+ 40ms 安定、後処理で
    「同音に挟まれた短い別音程の吸収」「同音の結合(再アタックは除く)」、
    細かい包絡(2ms)で立ち上がり点を再探索(ノート冒頭ピーク -15dB を最初に超える点)
  - 第 2 弾(2026-09-22): 音程は「冒頭 30% を除いた区間の中央値」で決める(しゃくれ対策)。
    短く(<150ms)音程の定まらない断片(中央部の揺れ 0.8 半音超)が隙間なく隣へ
    滑り込んでいれば隣の音に吸収(`absorb_glides`)。安定した短い音(16 分)は残す。
    端の 1 半音ちらつき(<100ms)は長い隣の音程へ。A-B-A 吸収は 1 半音差に限定
    (全音の刺繍音は残す)。再アタック判定に「直前がノート内最大 -6dB の谷」を追加。
    検出上限 1000→2600Hz(口笛対応)
- **メトロノーム / 録音タイミング補正(2026-09-22)**:
  - `PlaybackData::sigs` + `next_beat()` で拍位置を求め、レンダラがクリック
    (小節頭 1500Hz / 拍 1000Hz、マスターエフェクトを通さない)。`Shared::metronome`。
    UI は ⏱ ボタン、Tauri `transport_set_metronome`
  - 録音は「再生を聴いて歌う」ので出力レイテンシ + 準備時間ぶん遅れて記録される。
    `EngineHandle::start_recording(path, count_in, latency_secs, metronome_on)` →
    `mark_play_started()` で準備時間を計測し、停止時に
    offset_samples = 準備時間 + カウントイン + レイテンシ補正 を波形の頭から捨てる。
    設定(localStorage): カウントイン小節数(既定 1)、レイテンシ補正 ms(既定 60)、
    録音中メトロノーム自動 ON。録音中は曲末の自動停止を抑止(`Shared::recording`)
- ~~再生位置の Tick 管理~~ → **実装済み(2026-09-22)**: `PlaybackData::tempo`(テンポ区間表)
  + `Renderer::last_tick`。データ差し替え時に tick を保ってサンプル位置を換算し直す
- 外部 MCP クライアント使用時の AI インジケータ精度(今は 20 秒近似)
- ~~`history.jsonl` 肥大化対策~~ → **実装済み(2026-09-22)**: `Session::compact(keep)` +
  `Store::maybe_compact`(3000 件超で直近 1500 件に。起点を `history.base.json`、
  捨てた分を `history.archive.jsonl` に保存。再オープンは base + history で復元)
- ブラウザ版(WASM)— 別プロジェクト級のため保留(CLAP は 2026-09-23 に着手、下記)
