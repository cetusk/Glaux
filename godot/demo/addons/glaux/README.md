# Glaux for Godot(addons/glaux)

[Glaux](https://github.com/cetusk/Glaux)(AI と一緒に曲を作れる DAW)で作った曲を Godot 4 のゲームの中で
そのまま鳴らし、敵の動きなどを曲の拍・小節・マーカー・特定の音(キック等)に同期させるアドオンです。

- 音は Glaux の再生エンジンそのもので作ります(DAW で聴いた音がゲームで鳴る)
- 「いま聞こえている位置」(音声出力の遅れを補正済み)で、拍・マーカー・ノートをシグナルで知らせます
- 先読み(次の拍の時刻、これから鳴るキックの一覧など)も問い合わせられます

対応: Godot 4.3 以降、Windows(64bit)・Linux(x86_64)。

このフォルダの中身:

| ファイル | 内容 |
|---|---|
| `glaux.gdextension` | Godot に拡張を読み込ませる設定 |
| `bin/glaux_godot.dll` | 拡張本体(Windows 用) |
| `bin/libglaux_godot.so` | 拡張本体(Linux 用。リリースの zip に入っています) |
| `README.md` | この説明書(人間向け) |
| `AI_GUIDE.md` | ゲーム側で AI(Claude など)にこのアドオンを使わせるときの手引き |
| `PROMPT.md` | AI に最初に渡すプロンプトの例 |
| `CHANGELOG.md` | 改訂ノート(版ごとに何が変わったか) |
| `glaux_player.png` | エディタで `GlauxPlayer` ノードに付くアイコン |

## 1. 導入

1. この `addons/glaux/` フォルダ(`glaux.gdextension` と `bin/` の拡張本体、この README など)を、
   ゲームのプロジェクトの `addons/glaux/` にそのままコピーします。
   [Glaux のリリース](https://github.com/cetusk/Glaux/releases)の `glaux-godot-addon-<版>.zip` を展開すると、
   このフォルダが `addons/glaux/` として出てきます
2. Godot のエディタを開き直します(拡張はエディタ起動時に読み込まれます)
3. **「プロジェクト → プロジェクト設定 → プラグイン」で「Glaux」を有効にします。** ゲームを書き出す(エクスポート)
   ときに、曲のファイル(`project.json`・`audio/` の WAV・SoundFont)をそのまま .pck に入れるためのものです。
   有効にしないと、書き出したゲームで曲を読めません(エディタでの実行は、無効でも動きます)
4. 曲を置きます。おすすめは、**Glaux で曲をゲームの `songs/` フォルダの中に直接作る(または開く)**ことです。
   Glaux で直した内容がそのままゲームに反映され、コピーの手間がありません:
   - 新しく作る: Glaux の曲名のメニュー →「フォルダを選択して開く…」でゲームの `songs` を選び、
     「このフォルダに新しい曲を作る」→ 曲名を入れて作成(`songs/曲名.glaux` ができる)
   - 既にある曲を開く: 同じく `songs` を選ぶと、中の曲の一覧から選べる
   - 別の場所で作った曲を使うときは、その曲のフォルダ(例 `Stage1.glaux`)を `res://songs/` などへコピーする。
     必要なのは `project.json` と `audio/`(音声クリップ・サンプラーの WAV)だけ
     (`history.jsonl` や `cache/` はあっても構いません)
5. シーンに `GlauxPlayer` ノードを追加し、スクリプトから使います

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

## 2. GlauxPlayer の一覧

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

### 和声(効果音の音程を曲に合わせる)

曲のノートから推定したキーと、小節ごとのコードを問い合わせられます(推定なので、コードは 1 小節に 1 つ)。

| メソッド | 返り値 |
|---|---|
| `get_key()` | キー `{name: "A minor", tonic: 9, mode: "minor", confidence}`(`tonic` は C=0〜B=11)。ノートが無い曲は空 |
| `get_chords()` | 小節ごとのコードの一覧 `{time, bar, chord}`(`chord` は "Am" / "G7" / "N.C." など) |
| `get_chord_at(time)` | `time` 秒のコード名 |
| `get_chord_tones(time)` | `time` 秒のコードの構成音(C=0〜B=11 の番号。ルートが先頭) |
| `get_scale_pitch_classes()` | キーのスケールの 7 音(同じく番号) |
| `snap_to_scale(pitch)` | 音(MIDI 番号)をキーのスケールでいちばん近い音に寄せる |
| `snap_to_chord(pitch, time)` | 音を `time` 秒のコードの構成音でいちばん近い音に寄せる |
| `get_chord_note(time, index, base = 60)` | `time` 秒のコードの構成音を、`base`(MIDI 番号、60 = C4)以上で下から数えた `index` 番目の音(使い切ったら 1 オクターブ上へ) |
| `get_scale_note(index, base = 60)` | キーのスケールの音を、`base` 以上で下から数えた `index` 番目の音 |

### 効果音を 1 音ずつ鳴らす(WAV 不要)

曲の中のトラック(の音源とエフェクト)で、1 音を好きな音程で鳴らせます。曲を再生していなくても鳴ります。

| メソッド・プロパティ | 説明 |
|---|---|
| `play_note(track, pitch, velocity = 100, duration = 0.2)` | すぐ鳴らす(`duration` は押している秒数。その後は音源のリリースで消える) |
| `play_note_at(track, pitch, time, velocity = 100, duration = 0.2)` | `time` 秒に**聞こえるように**鳴らす(出力の遅れを見込む。過ぎた時刻ならすぐ) |
| `sync_to` | `play_note_at` の `time` の基準にする BGM の `GlauxPlayer`。未設定なら自分の曲の時刻 |
| `release_notes()` | 鳴らした音をすべて離す |

```gdscript
@onready var music: GlauxPlayer = $Music   # BGM
@onready var sfx: GlauxPlayer = $Sfx       # 効果音(曲は再生せず、1 音ずつ鳴らすだけ)

func _ready() -> void:
	music.load_song("res://songs/Stage1.glaux")
	sfx.load_song("res://songs/SEKit.glaux")   # 効果音用の曲(トラックごとに音色を作っておく)
	sfx.sync_to = music                        # play_note_at の時刻 = BGM の時刻

# 敵の攻撃: BGM の次の拍に、その時のコードの構成音で鳴らす(拍にぴったり合う)
func enemy_attack() -> void:
	var t := music.get_next_beat_time()
	sfx.play_note_at("Hit", music.get_chord_note(t, 0, 48), t)

# プレイヤーの入力: すぐ鳴らす。コンボが上がるほどコードの構成音を上っていく
func on_player_hit(combo: int) -> void:
	var now := music.get_song_time()
	sfx.play_note("Blip", music.get_chord_note(now, combo, 72), 110, 0.15)
```

- 音程を変えても、その音程で音源が鳴るので音色・長さが崩れません(`pitch_scale` で WAV を上げ下げするのと違う)
- Glaux で効果音の曲の音色を直せば、書き出し直さずにそのまま反映されます
- `play_note_at` で BGM の拍に合わせた音は、BGM とサンプル単位でそろいます(同じミックスの時計で位置を決めるため)

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

## 3. 使えるもの・使えないもの

- 使える: 内蔵音源(subtractive / fm / wavetable / drum / pluck / sampler)、内蔵エフェクト、音声クリップ、
  テンポ・拍子の変化、オートメーション、センド / バス、レガート / ポルタメント
- SoundFont(sf2): `.sf2` ファイルを曲フォルダの `soundfonts/` か `res://soundfonts/` に置けば鳴ります
- 使えない: CLAP プラグイン(Surge XT 等)。その音源のトラックは無音になり、`get_warnings()` に理由が出ます。
  ゲームで使うには、Glaux でそのトラックの見出しを右クリック →「🧊 音声にする(フリーズ)」を選びます。
  エフェクト・音量・パン・センドの響きまで込みで音声トラックになり、元のトラックはミュートされます
  (Ctrl+Z で戻せます。AI に「◯◯のトラックを音声にして」と頼んでも同じ)

## 4. 注意事項

### 拡張(DLL)の更新

- **DLL を差し替えるときは、Godot のエディタを閉じてから**にしてください。エディタが DLL を開いたままだと
  上書きに失敗することがあります(「別のプロセスが使用中」など)
- 拡張を更新するときは、Glaux 側で作り直した配布用フォルダ(`godot/dist/addons/glaux/`)の中身で、
  ゲーム側の `addons/glaux/` を丸ごと置き換えてください(DLL と説明書が同じ版にそろいます)

### 曲のファイル

- 曲をゲームの `songs/` の中で直接作っている(開いている)なら、Glaux で直した内容は保存と同時にゲーム側の
  ファイルにも入っています。ゲームを実行し直せば(`load_song` し直せば)反映されます
- 別の場所で作った曲をコピーして使っている場合は、**Glaux で直すたびにコピーし直してください**
  (`project.json` と `audio/` を上書きすれば十分です)
- 曲フォルダには Glaux の編集履歴(`history.jsonl` など)も入ります。ゲームには不要ですが、あっても害はありません
- 曲フォルダ内の WAV は、Godot のエディタが自動で取り込み、`.import` ファイルを作ります。
  拡張は WAV を直接読むので、取り込みの設定は動作に影響しません
- トラックは名前か ID で指定します。**同じ名前のトラックが複数あると、先頭のものが使われます。**
  名前を重ねないか、ID(`project.json` の `"id": "trk_..."`)で指定してください
- `bus`(出力先のバス)は `load_song` のときに反映されます。変えたいときは `load_song` の前に設定してください
  (`volume_db` はいつ変えても反映されます)
- 曲は Godot の出力のサンプルレート(プロジェクト設定 `audio/driver/mix_rate`、既定 44100Hz)で作って鳴らします。
  Glaux で作ったときと違うレートでも、音程・テンポは変わりません(音声クリップも自動で変換されます)

### 同期とタイミング

- シグナルは毎フレーム、前のフレームから通り過ぎた出来事をまとめて出すので、**最大 1 フレーム遅れて届きます**
  (60fps で最大約 17ms)。判定などで正確さが要るときは、引数の `time`(本来の時刻)と `get_song_time()` の
  差を使ってください
- `play()` を `_ready()` の中で直接呼ぶと、起動直後の重いフレームのあいだに曲が進み、最初の拍の知らせが
  遅れることがあります。`await get_tree().process_frame` で 1 フレーム待ってから鳴らすと安定します
- 再生直後の `get_song_time()` は、音が耳に届くまでの間(出力の遅れ、数十 ms)**負の値**になります。
  最初の拍のシグナルは音が届いてから出ます
- `seek()` で飛ばした区間の出来事はシグナルになりません(移った位置ちょうどの出来事は出ます)
- `pause()` → `resume()` は続きから、`pause()` → `play()` は曲頭からです
- **遅れの手動補正(`latency_offset_ms`)**: 出力の遅れは Godot が報告する値で自動補正しますが、
  Bluetooth のヘッドホンや一部のオーディオ機器では報告より実際の遅れが大きく、画面が音より早く動いて見えます。
  合わせ方の例:
  1. 拍ごとに画面を光らせる(`beat` シグナルで一瞬色を変える)
  2. 実際に使う機器で聴きながら、光るのが音より早ければ `latency_offset_ms` を増やす(20〜40 ずつ)
  3. ぴったり合ったら、その値をゲームの設定として保存する(プレイヤーが自分で合わせられる設定項目にするのが一般的)
  - 正の値でシグナル・位置が遅れます。負の値にすると早まります

### 効果音(`play_note`)

- 鳴らせるのは内蔵音源・SoundFont・サンプラーのトラックです。CLAP プラグイン(Surge XT 等)のトラックは鳴りません
- 効果音用の曲でトラックを**ミュート・ソロ**にしたまま保存すると、`play_note` の音もその状態になります
  (ミュートしたトラックは鳴らない)。Glaux で試聴した後はミュート・ソロを外して保存してください
- 1 つの `GlauxPlayer` で同時に鳴らせる音は 32 まで(超えると古い音から止まる)、予約は一度に 512 まで
- 押してから音が出るまでの遅れは、Godot の普通の効果音と同じくらい(出力の遅れのぶん)
- コードは小節ごとの推定です。小節の途中でコードが変わる曲では、その小節でいちばん合うコードになります
- 重い音や、いつも同じでよい音は、今までどおり WAV にしてもかまいません(併用できます)

### 処理の重さ

- シンセ(subtractive / fm / wavetable など)の音は、ゲーム中にその場で計算します。トラック数・同時に鳴る音・
  ユニゾンが多い曲や、リバーブなどのエフェクトを多く挿した曲は CPU を多く使います
- 重いと感じたら、重いトラックを音声クリップに置き換えると軽くなります(手順は 3 章の CLAP の場合と同じ)
- `GlauxPlayer` を複数置けば、同時に複数の曲(例: 曲とジングル)を鳴らせます。そのぶん処理も増えます

### 出ても問題のない警告

- 終了時の「ObjectDB instances leaked at exit」: 曲を鳴らしたままゲームを終了すると出ます。Godot 標準の
  音声ストリームでも同じように出る Godot の終了処理の都合で、害はありません
- 初回の読み込みなどで出る「Native class "GlauxPlayer" not found」: 拡張が読み込まれる前にスクリプトを
  調べたためで、実行には影響しません
- `load_song` のときの「CLAP プラグインのため、ゲームでは鳴りません」: 3 章のとおりです。
  `get_warnings()` でも確認できます

### ゲームの書き出し(エクスポート)

- 「プラグイン」で Glaux を有効にしてあれば、書き出しの設定は要りません。`res://` の下の `.glaux` フォルダを探して、
  `project.json`・`audio/`・`soundfonts/` と `res://soundfonts/` を元のまま .pck に入れます
  (編集の履歴 `history.jsonl` や `cache/` は入れません)
- 書き出したゲームでは、拡張の DLL がゲームの実行ファイルと同じフォルダに置かれます(Godot が自動で行います)
- 確認済み: Linux で .pck に書き出したゲームから、デモ曲と WAV を持つ曲が読み込まれて鳴ること、効果音が拍に 0.1ms で
  合うこと。Windows で書き出したゲームでの確認は、ゲームの側でお願いします

### 動作確認

- 確認済み: Windows(エディタでのデモ)、Linux(ヘッドレス・書き出した .pck)。macOS は未確認
