# 引き継ぎ文書(HANDOFF)

作成日: 2026-09-21(最終更新: 2026-09-21)
状態(2026-09-21 時点): 主要 4 クレート + アプリがすべて動作し、Windows 実機で確認済み。
- `glaux-core`: モデル / Command / 履歴
- `glaux-mcp`: 9 ツール(get_project / apply_commands / undo / redo / checkpoint / revert_to /
  get_history / list_params / analyze_audio)。stdio 単体 + アプリ内 HTTP の両対応
- `glaux-engine`: 再生・WAV エクスポート・音声解析(AI の耳)
- `glaux-dsp`: 楽器(subtractive / drum)+ エフェクト(eq / compressor / reverb)
- `app/`: タイムライン・履歴・チャット(ヘッドレス claude)・トランスポート・範囲マスク・
  音量/M/S 操作・WAV 書き出し
実機確認済みのハイライト: AI がチャット指示で作曲 → analyze_audio で自分の耳を使い →
エフェクトでミックス調整、のループが完走(「音圧配分が良くなって迫力が上がった」との評価)。
プロジェクト管理 UI も実装済み。今後の課題の全体像と優先順位は §8 を参照。

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
- **外部音源(CLAP プラグイン)のインポート**はゆくゆく対応。`PluginSource::Clap` の枠は用意済み
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
- `stretch` は今は `none` のみ。将来 `follow`(テンポ追従)を足す枠がある。MIDI 主軸の DAW で音声を入れたとき最初に欲しくなる機能
- オートメーションはトラック単位(クリップ単位は必要になったら追加)

### 楽器・エフェクト

- `PluginSource` で `builtin` / `clap` / `sampler` を切り替える。今は `builtin` のみ実装
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
| `split_clip {id, at, new_id}` | `batch[remove_clip, replace_clip]` | 音声は `offset_samples` をテンポマップから計算 |
| `add_notes {clip, notes}` | `remove_notes` | pitch/vel ≤ 127 |
| `remove_notes {clip, ids}` | `add_notes` | |
| `update_notes {clip, changes}` | 同 (旧値、逆順) | `NoteChange` は Option フィールドの部分更新 |
| `set_param {track, path, value}` | 同 or `unset_param` | path: `device/<n>`, `fx/<id>/<n>`, `track/volume_db|pan` |
| `unset_param {track, path}` | `set_param` | |
| `set_device {track, device?}` | 同 | |
| `add_effect {track, effect, index?}` | `remove_effect` | |
| `remove_effect {id}` | `add_effect` | |
| `set_effect_bypass {id, bypass}` | 同 | |
| `set_automation_points {track, target, points}` | 同 | 空配列でレーン削除 |
| `set_tempo {events}` | 同 | tick 0 から始まる昇順 |
| `set_time_sig {events}` | 同 | |
| `set_master_volume {volume_db}` | 同 | |
| `add_asset {id, asset}` / `remove_asset {id}` | 互いに | |
| `batch {commands, label}` | `batch` (逆順) | 途中失敗で巻き戻し |

**未実装で必要になりそうなもの**: `move_effect`、マスターバスのエフェクト操作、クリップ単位オートメーション、
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

残り(未実装): `get_clip` / `list_params`(glaux-dsp の ParamSpec 待ち)/ `get_history_entry` /
`revert(entry_id)` / 便利ツール(transpose_notes / shift_notes / scale_velocity / quantize_notes)/
`new_ids` / `analyze_audio`・`render`(エンジン実装後)

### 次の作業の論点

- `analyze_audio` の返す情報(エンジン実装後): ラウドネス(LUFS)、スペクトル重心、低/中/高域のエネルギー比、オンセット位置、推定ピッチ/コード。LLM は生音声を扱えないので **テキストで聴かせる**
- 相対操作の便利ツール(transpose_notes 等)は MCP 層で現在値を読んで絶対値の `update_notes` に変換する方針(決定済み、未実装)

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

- **再生位置はサンプル保持**。再生中のテンポ変更で音楽的位置が僅かにずれる
  (当初方針の「Tick で持つ」への移行は将来。tick⇔秒変換は `TempoMap` を使用)
- 音声クリップ・エフェクト・オートメーション・ループクリップは未対応
- 発音中のデータ差し替えはボイスを切り直す(クリックノイズが出うる)

今後: `ParamChanged` の軽量差し替え、MIDI 入力は `midir`、
WAV 読み込みは `symphonia`、リサンプリングは `rubato`、書き出しは `hound`

### `crates/glaux-dsp`(楽器 済)

実装済み(2026-09-21)。`fundsp` は使わず自前(依存ゼロで RT 安全を確実にするため):

- **`subtractive`**: PolyBLEP オシレータ(saw/square/triangle/sine)→ SVF(TPT)ローパス → ADSR。
  フィルタエンベロープ付き。device 未設定トラックの既定音源(未知の builtin 名や CLAP も当面これで代用)
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
  現在つながっているのはマスター音量スライダー、トラックの音量スライダー・M/S ボタン。
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
- 未実装(次段階): ピアノロール(Canvas)、クリップ移動等の編集 UI、`revert(entry_id)` の UI、
  段階的開示のデバイスパネル、チャットのターン境界と
  AI インジケータの連動はチャット経由のみ正確(外部 MCP クライアントは近似のまま)

### 将来

- CLAP プラグインホスティング(`clack` クレート)
- タイムストレッチ(`Stretch::Follow`)
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
- `history.jsonl` の肥大化対策(スナップショット + 以降の差分、`git gc` 相当)

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
   - 未実装: ベロシティ編集 UI(表示は濃淡のみ。AI 指示で代用可)
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

4. ~~音作り拡張フェーズ 1(EDM / メタル)~~ → **実装済み(2026-09-22)**:
   - **`distortion`** エフェクト: tanh 波形整形(drive_db)+ トーン LP + mix(パラレル可)+ level
   - **`sidechain`** エフェクト: `source` パラメータ(トラック ID 文字列、構築時に index 解決)。
     検出信号は**ソーストラックの生モノ合算(エフェクト前)**。レンダラはフレームごとに
     全トラックの生合算を先に確定させるので、トラックの処理順に依存しない。
     ミュートしたソースは検出 0(= 踏まれない)という素直な意味論
   - **subtractive 拡張**: `unison`(1..7)+ `detune`(supersaw)、`sub`(1 オクターブ下)、
     `noise`。デフォルトは従来音を完全維持(既存プロジェクトの音は変わらない)。
     ユニゾンは等パワー正規化(√n)
   - **drum にノート 55 = リバースクラッシュ**(全長 1.6s×decay、3 乗カーブで迫り上がる)。
     ドラム UI のキット図には未追加(AI 経由で使用可。カタログ説明に記載)
   - 未実装のまま: 第 2 オシレータ、EDM キックの専用チューニング(tune/decay で代用)、
     エンジンのバス構造(サイドチェインは上記方式で不要になった)

5. **アーティキュレーション設計(ブリッジミュート等)**
   - 「同じ音源で奏法を切り替える」表現。メタルのブリッジミュートが代表例
   - 案 A: `Note` に articulation フィールドを追加(スキーマ変更 + コマンド拡張。本命)
   - 案 B: キースイッチ(特定ノート番号で切替。変更不要だが暗黙的で AI に不親切)
   - 案 C(今すぐ可能な回避策): 別トラックで代用。palm mute ≒ 短 decay + 低 cutoff
   - ギター音源そのもの(Karplus-Strong 物理モデル or サンプラー)も併せて検討

6. **音作りプロジェクト(サウンドデザインモード)+ プリセット**
   - 「音を作るためのプロジェクト」: 単音/短フレーズをループ試聴しながらパッチを練る
   - 作ったパッチの**プリセット保存/読み込み**(置き場: `~/.config/glaux/presets/` 案。
     曲プロジェクト間で共有できることが重要)
   - MCP: プリセット一覧ツール、`set_device` でプリセット指定
   - プロジェクトメニューからモード付き新規作成、など

7. **MCP 便利ツール**(transpose_notes / shift_notes / quantize_notes / scale_velocity)
   - AI の編集の効率と確実性を上げる(現在値読み→絶対値変換をサーバー側で肩代わり)

### バックログ(順不同)

- `revert(entry_id)` の UI(履歴パネルから AI の編集を個別却下)
- クリップの移動・リサイズ等のタイムライン直接編集
- 段階的開示のデバイスパネル(UI からつまみを操作。今は AI 経由のみ)
- 音声クリップ再生・サンプラー・録音、ループクリップの再生対応
- 再生位置の Tick 管理(再生中のテンポ変更でのずれ解消)
- 外部 MCP クライアント使用時の AI インジケータ精度(今は 20 秒近似)
- `history.jsonl` 肥大化対策(スナップショット + 差分)
- CLAP プラグイン対応、ブラウザ版(WASM)
