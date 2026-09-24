# AI 向け手引き: Glaux アドオン(GlauxPlayer)で曲とゲームを同期させる

このファイルは、ゲームのプロジェクトで作業する AI(Claude など)が `addons/glaux` を正しく使うための手引きです。
人間向けの説明は同じフォルダの `README.md` にあります(内容は一致しています)。

## 0. 前提(まず理解すること)

- **曲は Glaux(別アプリの DAW)で作られ、ゲームは読むだけ。** 曲は `.glaux` フォルダ
  (`project.json` と `audio/`)として `res://songs/` などに置かれている。
  **`project.json` を手で書き換えて曲を変えないこと。** 曲を変えたいときは人間に Glaux で直してもらう
  (曲をゲームの `songs/` の中で直接作っていれば保存と同時に反映される。別の場所の曲をコピーして
  使っている場合はコピーし直しが要る)
- `GlauxPlayer`(Node)が曲を鳴らし、**「いま聞こえている位置」の時計**として働く。敵の動き・予兆・判定は
  この時計を基準に作る。`Time.get_ticks_msec()` やフレームの `delta` の積算で拍を数えない(ずれる)
- 時刻の単位はすべて**秒(float)**、曲頭が 0。小節・拍は **1 始まり**
- テンポ・拍子は曲の途中で変わりうる。**`60.0 / bpm` で拍の長さを計算しない。** 拍の時刻は
  `get_beats()` / `get_time_of()` / `get_next_beat_time()` で取る
- 対応: Godot 4.3 以降、Windows。CLAP プラグイン(Surge XT 等)の音源のトラックはゲームでは鳴らない
  (`get_warnings()` に出る)

## 1. 導入の確認

1. `res://addons/glaux/glaux.gdextension` と `res://addons/glaux/bin/glaux_godot.dll` がある
2. エディタで `GlauxPlayer` をノードとして追加できる(できなければ拡張が読み込まれていない。エディタの再起動、
   DLL の有無、Godot の版を確認)
3. 曲フォルダに `project.json` がある。`load_song()` が true を返す(false なら `get_last_error()`)
4. 使うトラック名は `get_track_names()` で確認する。**推測で書かない**

## 2. API(正確な仕様)

### シグナル

```gdscript
signal beat(bar: int, beat: int, time: float)        # 拍の頭を通り過ぎた。time はその拍の本来の時刻(秒)
signal section(name: String, time: float)            # マーカー(「サビ」など)を通り過ぎた
signal note(track: String, pitch: int, velocity: int, time: float, duration: float)
                                                     # watch_track したトラックの音が鳴った(pitch は MIDI 番号)
signal song_finished()                               # 曲が余韻まで鳴り終わった
```

- シグナルは `_process` の中で、前のフレームから今までに通り過ぎた出来事を**時刻順にまとめて**出す。
  **最大 1 フレーム遅れる**ので、正確さが要る処理は引数の `time` を使う(`get_song_time() - time` が遅れ)
- 再生開始・`seek` の位置ちょうどの出来事(曲頭の 1 拍目など)も出る。`seek` で飛ばした区間の出来事は出ない

### メソッド

| メソッド | 返り値 | 仕様 |
|---|---|---|
| `load_song(path: String)` | `bool` | `.glaux` フォルダのパス(例 `"res://songs/Stage1.glaux"`)。読むと停止状態。失敗理由は `get_last_error()` |
| `get_last_error()` | `String` | 直前の失敗理由 |
| `get_warnings()` | `PackedStringArray` | 読み込み時の注意(CLAP の音源は鳴らない等) |
| `play()` | − | 曲頭から再生 |
| `play_from(sec: float)` | − | `sec` 秒から再生 |
| `pause()` / `resume()` | − | 一時停止 / 続きから(`pause` → `play` は曲頭から) |
| `stop()` | − | 停止して曲頭へ |
| `seek(sec: float)` | − | 位置を移す(再生中ならそのまま続く) |
| `is_playing()` | `bool` | 再生中か(一時停止・停止・曲の終わりで false) |
| `get_song_time()` | `float` | いま聞こえている位置(秒)。出力の遅れを補正済み。再生中は単調増加。**再生直後は負になりうる** |
| `get_beat_position()` | `float` | 曲頭から何拍目か(小数。例 12.37)。`fmod(x, 1.0)` で拍の中の位置 0〜1 |
| `get_bar()` / `get_beat()` | `int` | いまの小節・小節内の拍(1 始まり) |
| `get_bpm()` | `float` | いまのテンポ |
| `get_next_beat_time()` | `float` | 次の拍の頭の時刻(秒)。無ければ -1 |
| `get_section()` | `String` | いまのマーカー名(無ければ空) |
| `get_time_of(bar: int, beat: int)` | `float` | その小節・拍の頭の時刻(秒)。範囲外なら -1 |
| `get_length()` | `float` | 曲の長さ(秒。最後のクリップ・マーカーまで。余韻は含まない) |
| `get_track_names()` | `PackedStringArray` | トラック名の一覧 |
| `watch_track(track: String)` / `unwatch_track(track)` | − | `note` シグナルを出すトラック(名前か `trk_...` の ID)。同名が複数なら先頭 |
| `get_notes(track: String, from: float, to: float)` | `Array` | `[from, to)` に始まる音。要素は `{time, duration, pitch, velocity}` の Dictionary |
| `get_beats(from: float, to: float)` | `Array` | `[from, to)` の拍。要素は `{time, bar, beat, beats_in_bar}` |
| `get_sections()` | `Array` | マーカー全部。要素は `{time, name}` |

### プロパティ

| プロパティ | 型 | 仕様 |
|---|---|---|
| `bus` | `StringName` | 出力バス(既定 `Master`)。**`load_song` の前に**設定する |
| `volume_db` | `float` | 音量(いつ変えても反映) |
| `latency_offset_ms` | `float` | 手動の遅れ補正。正で位置・シグナルが遅れる。プレイヤーが調整できる設定項目にする |

## 3. 実装パターン

### 3-1. 基本の組み立て

```gdscript
extends Node2D

@onready var music: GlauxPlayer = $GlauxPlayer

func _ready() -> void:
	if not music.load_song("res://songs/Stage1.glaux"):
		push_error("Glaux: " + music.get_last_error())
		return
	for w in music.get_warnings():
		push_warning("Glaux: " + w)
	music.watch_track("Kick")          # 監視は play の前に
	music.beat.connect(_on_beat)
	music.note.connect(_on_note)
	music.section.connect(_on_section)
	music.song_finished.connect(_on_song_finished)
	await get_tree().process_frame     # 起動直後の重いフレームを避けてから鳴らす(最初の拍が遅れない)
	music.play()
```

### 3-2. 拍に合わせて動く(シグナル駆動)

```gdscript
func _on_beat(bar: int, beat: int, time: float) -> void:
	if beat == 1:
		enemy.start_pattern(bar)       # 小節の頭で行動を切り替える
	enemy.step()                       # 毎拍の動き

func _on_note(track: String, pitch: int, velocity: int, time: float, duration: float) -> void:
	if track == "Kick":
		enemy.shoot(velocity / 127.0)  # キックの強さで弾の強さを変える
```

### 3-3. 拍の中で滑らかに動く(毎フレーム)

```gdscript
func _process(_delta: float) -> void:
	var b := music.get_beat_position()
	var phase := fmod(b, 1.0)                        # 拍の中の位置 0〜1
	enemy.scale = Vector2.ONE * (1.0 + 0.15 * pow(1.0 - phase, 3.0))   # 拍の頭で膨らむ
```

`get_beat_position()` は「聞こえている位置」なので、フレームの遅れに関係なく音と合う。
アニメーションはシグナルより毎フレームの位置で作る方がずれない。

### 3-4. 先読みして予兆を出す

```gdscript
const LEAD := 0.5   # 何秒前から予兆を出すか
var telegraphed := {}   # 同じ音に 2 回予兆を出さないため(time をキーにする)

func _process(_delta: float) -> void:
	var now := music.get_song_time()
	for n in music.get_notes("Kick", now, now + LEAD):
		if not telegraphed.has(n.time):
			telegraphed[n.time] = true
			enemy.telegraph(n.time - now)        # あと何秒で来るか
```

### 3-5. 読み込み時に譜面(攻撃パターン)を作る

```gdscript
var attacks: Array = []

func _build_chart() -> void:
	attacks = music.get_notes("Kick", 0.0, music.get_length() + 1.0)   # 曲全体のキック
	var sections := music.get_sections()                               # [{time, name}, ...]
	# 例: 「サビ」の間だけ攻撃を 2 倍にする
	for s in sections:
		if s.name == "サビ":
			pass
```

### 3-6. 入力の判定(ずれを ms で測る)

```gdscript
var next_index := 0

func _unhandled_input(event: InputEvent) -> void:
	if not event.is_action_pressed("hit"):
		return
	var t := music.get_song_time()                  # 入力した瞬間に聞こえていた位置
	# いちばん近い音を探す(attacks は 3-5 で作った一覧、時刻順)
	while next_index < attacks.size() - 1 and attacks[next_index + 1].time <= t:
		next_index += 1
	var best: Dictionary = attacks[next_index]
	if next_index + 1 < attacks.size() and abs(attacks[next_index + 1].time - t) < abs(best.time - t):
		best = attacks[next_index + 1]
	var err_ms: float = (t - best.time) * 1000.0          # 正 = 遅い、負 = 早い
	judge(err_ms)
```

判定は `get_song_time()`(聞こえている位置)と音の `time` の差で行う。シグナルが届いた時刻で判定しない。

### 3-7. 展開(マーカー)で段階を切り替える

```gdscript
func _on_section(name: String, time: float) -> void:
	match name:
		"サビ":
			enemy.enrage()
		"outro":
			enemy.retreat()
```

マーカー名は曲側(Glaux)で決まる。`get_sections()` で実際の名前を確認してから書く。

## 4. 守ること・落とし穴

- `play()` を `_ready()` で直接呼ばず、`await get_tree().process_frame` の後に呼ぶ
- シグナルの到着時刻を判定や位置合わせに使わない(最大 1 フレーム遅れる)。引数の `time` か `get_song_time()` を使う
- 再生直後の `get_song_time()` は負になりうる。`max(0.0, ...)` で潰さず、負の間は「まだ始まっていない」と扱う
- 拍の長さを BPM から計算しない(テンポ・拍子の変化で壊れる)。`get_beats()` などで取る
- トラック名・マーカー名は `get_track_names()` / `get_sections()` で確認してから使う(曲側の名前に依存する)
- `watch_track` は `play()` の前に
- `bus` は `load_song()` の前に設定
- シーンを切り替えるときは `stop()` してから(`GlauxPlayer` をツリーから外すと音も止まる)
- 曲を切り替えるときは同じ `GlauxPlayer` で `load_song()` し直してよい(前の曲は解放される)
- 1 つの `GlauxPlayer` で鳴る曲は 1 つ。同時に別の曲・ジングルを鳴らすなら `GlauxPlayer` をもう 1 つ置く
- 効果音(ヒット音など)を拍に合わせて鳴らすのはこのアドオンの対象外(今は曲の再生と同期のみ)。
  効果音は Godot の `AudioStreamPlayer` で鳴らし、鳴らす時刻を `get_next_beat_time()` などで決める
- 終了時の「ObjectDB instances leaked at exit」、初回の「Native class "GlauxPlayer" not found」は無害

## 5. 動作確認のしかた

- 手早く確かめるには、拍ごとに画面を光らせる・ログを出す(`print(bar, ":", beat, " ", time)`)
- ヘッドレスで自動確認する場合(Godot の実行ファイルのパスは環境に合わせる):
  `Godot_v4.3-stable_win64.exe --headless --path . -- --check` のように引数を渡し、
  スクリプト側で `OS.get_cmdline_user_args()` を見て、数秒鳴らしてから拍・音の数を `print` して `get_tree().quit()` する。
  ヘッドレスでは出力の遅れが 0 として扱われる(実機より音が早い)点に注意
- 音と画面のずれを感じたら、まず `latency_offset_ms` を調整する(正で画面側が遅れる)

## 6. 曲そのものを変えたいとき

- ゲーム側では曲を編集しない。人間に Glaux で直してもらう(ゲームの `songs/` の中の曲を Glaux で開いて直せば、
  コピーなしで反映される)
- 必要なら、欲しい変更を具体的に伝える: 「2 小節目からキックを 8 分で」「サビの前に 1 小節のブレイク」
  「敵の予兆に使うので、ハイハットを別トラック "HiHat" に分けて」など。
  トラックを役割ごとに分けてもらうと `watch_track` で使いやすい
- (任意)Glaux のアプリが同じ PC で起動していれば、Glaux の MCP サーバー(`http://127.0.0.1:41920/mcp`)に
  つないで曲を読んだり直したりできる(`claude mcp add --transport http glaux http://127.0.0.1:41920/mcp`)。
  Glaux で開いている曲がゲームの `songs/` の中のものなら、直した内容はそのままゲームに反映される
