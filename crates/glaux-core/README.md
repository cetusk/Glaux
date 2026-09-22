# glaux-core

DAW のプロジェクトモデル・コマンド・履歴を担う純粋なデータ層。
オーディオ処理にも UI にも依存しない(依存: serde / serde_json / thiserror / rand / chrono)。

## 設計の柱

| 柱 | 実装 |
|---|---|
| 時間は拍(Tick, PPQ=960)が正。音声も拍上に置き、秒への変換は `TempoMap` | `time.rs` |
| 型付き ID(`trk_…` `clp_…` `nt_…` `fx_…`)。生成側が ID を決める(コマンドは決定的) | `id.rs` |
| ファイルには値だけ。単位・範囲・説明は `ParamSpec` としてコード側が持つ | `model/param.rs` |
| すべての編集は `Command`。UI も AI も MCP も同じ経路 | `command.rs` |
| `Project::apply` は逆コマンドと変更通知を返す(逆コマンド方式の Undo) | `apply.rs` |
| Git ライクな履歴: undo / redo / checkpoint / revert / replay、`author` 付き | `history.rs` |

## 使い方

```rust
use glaux_core::*;

let mut s = Session::new(Project::new("My Song"));
let ai = Author::Ai { model: "claude".into() };

// トラックとクリップを追加(ID は呼び出し側が生成する)
let tid = TrackId::new();
let mut t = Track::new(tid.clone(), "Bass", TrackKind::Midi);
t.device = Some(Device::builtin("subtractive"));
s.apply(Command::AddTrack { track: t, index: None }, Author::Human, "add bass").unwrap();

// AI が試行錯誤する足場
s.checkpoint("before_bassline");
let cid = ClipId::new();
s.apply(Command::batch("bassline v1", vec![
    Command::AddClip { track: tid.clone(), clip: Clip::new_midi(cid.clone(), "A", Tick(0), Tick(3840)) },
    Command::AddNotes { clip: cid.clone(), notes: vec![
        Note { id: NoteId::new(), pos: Tick(0), dur: Tick(480), pitch: 36, vel: 100 },
    ]},
    Command::SetParam { track: tid.clone(), path: ParamPath::device("filter.cutoff"), value: 600.0.into() },
]), ai.clone(), "generate bassline").unwrap();

// 気に入らなければ戻す
s.revert_to("before_bassline").unwrap();

// 途中のエントリだけ取り消す(git revert)。衝突候補が返る。
// let r = s.revert(&entry_id, Author::Human).unwrap();  r.conflicts

// 保存
let project_json = s.project().to_json().unwrap();      // project.json
let history_jsonl = s.history().to_jsonl().unwrap();    // history.jsonl

// 復元(リプレイ)
let entries = History::entries_from_jsonl(&history_jsonl).unwrap();
let rebuilt = Session::replay(Project::new("My Song"), entries).unwrap();
```

## コマンドの JSON 形

```json
{ "op": "set_param",  "track": "trk_a1b2c3", "path": "device/filter.cutoff", "value": 600.0 }
{ "op": "add_notes",  "clip": "clp_c3d4e5", "notes": [{ "id": "nt_000009", "pos": 1920, "dur": 480, "pitch": 43, "vel": 100 }] }
{ "op": "update_notes", "clip": "clp_c3d4e5", "changes": [{ "id": "nt_000001", "pitch": 38 }] }
{ "op": "split_clip", "id": "clp_c3d4e5", "at": 1920, "new_id": "clp_ffff01" }
{ "op": "set_track_prop", "id": "trk_a1b2c3", "prop": "mute", "value": true }
{ "op": "batch", "label": "bassline v1", "commands": [ ... ] }
```

相対操作(「+480 tick 動かす」)は持たない。UI / MCP 層で絶対値に変換してから流す。

## 不変条件

- `Track.clips` は `(start, id)` 昇順、MIDI ノートは `(pos, pitch, id)` 昇順に常に保たれる
- `apply` が失敗したときプロジェクトは変更されない(`Batch` は巻き戻す)
- 任意のコマンドについて `apply(cmd)` → `apply(inverse)` で完全に元に戻る(ランダムテストで検証)
- `history.jsonl` を空プロジェクトに順に適用すると現在の `Project` と一致する
- JSON 往復で `Project` は完全に一致する(`serde_json` の `float_roundtrip` 有効)

## 決めごと(要注意)

- `SplitClip`: 分割点をまたぐ MIDI ノートは左側で切り詰める(右側にコピーしない)
- `SetAutomationPoints` に空配列を渡すとレーン削除
- `SetParam` は未設定のキーにも書ける。その逆は `UnsetParam`
- `ParamValue` は untagged なので、JSON の `800` は `Int`、`800.0` は `Float` として読まれる。
  読み取り側は `as_f64()` を使うこと

## 次のステップ

- `glaux-engine`: `Change` を受け取り、`Project` からエンジン用の再生データ(tick ソート済みイベント列)を組み立てる
- `glaux-dsp`: 内蔵デバイスのレジストリと `ParamSpec` の実体
- `glaux-mcp`: `get_project` / `apply_commands` / `list_params` / `undo` / `checkpoint` / `revert_to` / `get_history` / `revert` の公開

## テスト

```
cargo test
```
