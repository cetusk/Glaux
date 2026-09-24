# Glaux を Godot で使う(glaux-godot)

Glaux で作った曲(`.glaux` フォルダ)を Godot 4 のゲームの中でそのまま鳴らし、敵の動きなどを
曲の拍・小節・マーカー・特定の音(キック等)に同期させるための拡張です。

- 音は Glaux の再生エンジンそのもので作ります(DAW で聴いた音がゲームで鳴る)
- 「いま聞こえている位置」(音声出力の遅れを補正済み)で、拍・マーカー・ノートをシグナルで知らせます
- 先読み(次の拍の時刻、これから鳴るキックの一覧など)も問い合わせられます

対応: Godot 4.3 以降、Windows(64bit)。Linux / macOS でもビルドすれば動きます。

## 1. ビルド(Windows)

必要なもの: Rust 1.94 以降(`rustup update` で最新にできます)。

リポジトリのフォルダで次を実行します。

```bat
godot\build_windows.bat
```

`cargo build -p glaux-godot --release` でビルドし、できた `glaux_godot.dll` を
`godot\demo\addons\glaux\bin\` に置きます(初回は数分かかります)。

## 2. デモを動かす

Godot 4.3 で `godot/demo/project.godot` を開き、実行(F5)します。デモの曲(4 小節)が鳴り、
出力パネルに小節・マーカーが表示されます。

## 3. 自分のゲームに入れる

1. `godot/demo/addons/glaux/` フォルダ(`glaux.gdextension` と `bin/glaux_godot.dll`)を、
   ゲームのプロジェクトの `addons/glaux/` にコピーします
2. Godot のエディタを開き直します(拡張はエディタ起動時に読み込まれます)
3. 曲を置きます: Glaux のプロジェクトフォルダ(例 `Stage1.glaux`)をゲームの `res://songs/` などへ
   コピーします。必要なのは `project.json` と `audio/`(音声クリップ・サンプラーの WAV)だけです
   (`history.jsonl` や `cache/` はあっても構いません)
4. シーンに `GlauxPlayer` ノードを追加し、スクリプトから使います

```gdscript
extends Node2D

@onready var music: GlauxPlayer = $GlauxPlayer

func _ready() -> void:
	if not music.load_song("res://songs/Stage1.glaux"):
		push_error(music.get_last_error())
		return
	music.watch_track("Kick")               # このトラックの音ごとに note シグナルを出す
	music.beat.connect(_on_beat)
	music.note.connect(_on_note)
	music.section.connect(_on_section)
	await get_tree().process_frame          # 起動直後の重いフレームを避けてから鳴らす
	music.play()

func _process(_delta: float) -> void:
	# 拍の中の位置(0〜1)で敵の動きを補間する
	var phase := fmod(music.get_beat_position(), 1.0)
	$Enemy.scale = Vector2.ONE * (1.0 + 0.2 * (1.0 - phase))

func _on_beat(bar: int, beat: int, time: float) -> void:
	if beat == 1:
		$Enemy.change_pattern(bar)

func _on_note(track: String, pitch: int, velocity: int, time: float, duration: float) -> void:
	$Enemy.attack()

func _on_section(name: String, time: float) -> void:
	if name == "サビ":
		$Enemy.enrage()
```

## 4. GlauxPlayer の一覧

### シグナル

| シグナル | 引数 | いつ |
|---|---|---|
| `beat` | `bar, beat, time` | 拍の頭を通り過ぎた(小節・拍は 1 始まり。`time` はその拍の本来の時刻・秒) |
| `section` | `name, time` | Glaux で打ったマーカー(「サビ」など)を通り過ぎた |
| `note` | `track, pitch, velocity, time, duration` | `watch_track` したトラックの音が鳴った |
| `song_finished` | − | 曲が余韻まで鳴り終わった |

シグナルは毎フレーム(`_process`)、前のフレームから今までに通り過ぎた出来事をまとめて出します。
そのため最大 1 フレームぶん遅れて届きます。ぴったり合わせたいときは引数の `time` と
`get_song_time()` の差で補正してください。

### 操作

| メソッド | 説明 |
|---|---|
| `load_song(path) -> bool` | 曲を読み込む(停止した状態になる)。失敗理由は `get_last_error()` |
| `play()` / `play_from(sec)` | 曲頭から / `sec` 秒から再生 |
| `pause()` / `resume()` | 一時停止 / 続きから |
| `stop()` | 停止(曲頭へ戻る) |
| `seek(sec)` | 位置を移す(飛ばした区間のシグナルは出ない) |
| `is_playing() -> bool` | 再生中か |
| `watch_track(name)` / `unwatch_track(name)` | `note` シグナルを出すトラック(名前か ID) |

### 位置・問い合わせ

| メソッド | 返り値 |
|---|---|
| `get_song_time()` | いま聞こえている位置(秒)。再生直後は出力の遅れぶん負になることがある |
| `get_beat_position()` | 曲頭から何拍目か(小数。`fmod(x, 1.0)` で拍の中の位置) |
| `get_bar()` / `get_beat()` | いまの小節・拍(1 始まり) |
| `get_bpm()` | いまのテンポ |
| `get_next_beat_time()` | 次の拍の時刻(秒) |
| `get_section()` | いまのマーカー名 |
| `get_time_of(bar, beat)` | その小節・拍の時刻(秒) |
| `get_notes(track, from, to)` | `[from, to)` 秒に始まる音の一覧 `{time, duration, pitch, velocity}`(先読み用) |
| `get_beats(from, to)` | `[from, to)` 秒の拍の一覧 `{time, bar, beat, beats_in_bar}` |
| `get_sections()` | マーカーの一覧 `{time, name}` |
| `get_track_names()` / `get_length()` | トラック名の一覧 / 曲の長さ(秒) |
| `get_warnings()` | 読み込みで気づいた注意(CLAP の音源など) |

### プロパティ

| プロパティ | 説明 |
|---|---|
| `bus` | 出力するオーディオバス(既定 Master) |
| `volume_db` | 音量 |
| `latency_offset_ms` | 手動の遅れ補正。音より画面が早いと感じたら増やす(Bluetooth のヘッドホン等) |

### 先読みの例(1 拍前に予兆を出す)

```gdscript
func _process(_delta: float) -> void:
	var now := music.get_song_time()
	for n in music.get_notes("Kick", now, now + 0.5):
		$Enemy.telegraph(n.time - now)   # あと何秒で来るか
```

## 5. 使えるもの・使えないもの

- 使える: 内蔵音源(subtractive / fm / wavetable / drum / pluck / sampler)、内蔵エフェクト、音声クリップ、
  テンポ・拍子の変化、オートメーション、センド / バス、レガート / ポルタメント
- SoundFont(sf2): `.sf2` ファイルを曲フォルダの `soundfonts/` か `res://soundfonts/` に置けば鳴ります
- 使えない: CLAP プラグイン(Surge XT 等)。その音源のトラックは無音になり、`get_warnings()` に理由が出ます。
  ゲームで使う曲は、CLAP のトラックを Glaux で音声に書き出して音声クリップにしてください

## 6. 補足

- 終了時の「ObjectDB instances leaked at exit」の警告: 曲を鳴らしたままゲームを終了すると出ますが、
  Godot 標準の音声ストリームでも同じように出る Godot の終了処理の都合で、害はありません
- 初回の `--import` などで「Native class "GlauxPlayer" not found」と出ることがありますが、
  拡張が読み込まれる前にスクリプトを調べたためで、実行には影響しません
- ゲームの書き出し(エクスポート)後に曲を読むには、`project.json` と WAV が .pck に入っている必要があります。
  書き出しの設定は次の段階で確認します(未確認)
