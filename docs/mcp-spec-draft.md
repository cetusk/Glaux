# glaux-mcp ツール仕様(草案)

`Session` を AI に公開する MCP サーバー。すべてのツールは `glaux_core::Command` に変換されて `Session::apply` を通る。
AI が書いた JSON をそのまま `Command` にデシリアライズできるよう、`Command` の serde 形をそのまま使う。

## 読み取り

### `get_project`
```
args: { track_ids?: string[], include_notes?: bool (default true), include_automation?: bool (default true) }
returns: Project(JSON)。フィルタ付きで部分取得可
```
ノートが数千個になるとレスポンスが巨大になるので、`include_notes: false` でまず構造だけ取り、
必要なクリップだけ `get_clip` で取る運用を AI に促す(ツール description に書く)。

### `get_clip`
```
args: { clip_id: string }
returns: Clip(JSON)
```

### `list_params`
```
args: { track_id: string }
returns: [{ path: "device/filter.cutoff", display_name, unit, range, description, current: value }]
```
`ParamSpec` + 現在値。`description` は聴感上の効果を書いた文。AI がつまみを理解する唯一の情報源。

### `get_history`
```
args: { author?: "human" | "ai" | "system", since?: entry_id, limit?: number }
returns: [{ id, author, timestamp, label, targets, reverts? }]
```
コマンド本体は含めない(大きい)。必要なら `get_history_entry`。

### `get_history_entry`
```
args: { entry_id }
returns: HistoryEntry(forward/inverse 込み)
```

## 編集

### `apply_commands`
```
args: { commands: Command[], label: string }
returns: { entry_id, changes: Change[] }
```
複数渡すと `Batch` にまとめる(= 1 回の undo で戻る)。`author` はサーバーが `Ai { model }` を付ける。
失敗時はどのコマンドで失敗したか(`Batch { index }`)を返す。

新規 ID は AI が生成してもよいが、形式ミスが多そうなら `new_ids { kind: "clip", count: 3 }` ツールを用意して
サーバー側で生成させる案もある。

### 便利ツール(相対操作 → 絶対値の `update_notes` に変換)
```
transpose_notes { clip_id, note_ids?: [] (省略で全部), semitones: int }
shift_notes     { clip_id, note_ids?, delta_ticks: int }
scale_velocity  { clip_id, note_ids?, factor: number }
quantize_notes  { clip_id, note_ids?, grid_ticks: int, strength?: 0..1 }
```
内部で現在値を読み、`UpdateNotes` を組み立てて `apply`。これらも履歴に載る。

## 履歴操作(git ライク)

```
undo   { n?: 1 }
redo   { n?: 1 }
checkpoint  { label }
revert_to   { label }
revert      { entry_id }   → { entry_id(新), conflicts: entry_id[] }
```

`revert` の `conflicts` が空でなければ、AI はそれらを確認してから進むこと(ツール description に明記)。

## 解析(エンジン実装後)

### `analyze_audio`
```
args: { clip_id } | { track_id, range: {start_tick, end_tick} } | { master: true, range }
returns: {
  loudness_lufs, peak_db, crest_factor,
  spectral_centroid_hz, band_energy: { low, mid, high },   // 比率
  onsets: [tick...], estimated_tempo?, estimated_key?, estimated_chords?: [{tick, chord}]
}
```
LLM は生音声を扱えないので、これが AI の「耳」になる。返す指標は「AI が判断に使えるか」で選ぶ。

### `render`
```
args: { range, tracks?: [] }
returns: { asset_id }   // cache/ に書いた WAV。analyze_audio に渡せる
```

## 実装メモ

- SDK: `rmcp`(Rust 公式)。まず stdio トランスポートで単体起動できるようにし、
  `claude mcp add glaux -- ./target/debug/glaux-mcp <MySong.glaux>` で Claude Code / Claude Desktop から叩けるようにする
- `Session` はアクター(専用スレッド + mpsc)で持ち、MCP と将来の Tauri UI が同じキューに入れる
- ツール description は AI が読む最重要ドキュメント。「いつ使うか」「注意点」を丁寧に書く
- 各ツールの結果には `project_version`(版数。編集・undo・redo のたびに増え、戻らない)を含め、AI が自分の理解が古いか判断できるようにする
