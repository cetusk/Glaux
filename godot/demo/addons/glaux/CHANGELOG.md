# 改訂ノート(Glaux for Godot)

ゲーム側で `addons/glaux` を使っている人・AI 向けの更新記録です。新しいものが上です。
使い方の詳細は同じフォルダの `README.md`(人間向け)と `AI_GUIDE.md`(AI 向け)にあります。

---

## 0.3.1(2026-09-26)外してあるエフェクトを鳴らさない

### 要点

- Glaux 0.1.1 のミキサーで「線から外して取っておいた」エフェクト(`project.json` の effects で `"parked": true`)を、
  アプリと同じく鳴らさないようにしました。0.3.0 までの拡張では、外してあるエフェクトも通して鳴っていました
- API・シグナル・設定の変更はありません。`.dll` / `.so` を差し替えるだけで使えます

---

## 0.3.0(2026-09-25)書き出したゲームで曲を読めるように / CLAP のトラックは「音声にする」で

### 要点

- **ゲームを書き出した(エクスポート)後も、曲を読めるようになりました。** 以前は Godot の書き出しが
  `project.json` と元の WAV を .pck に入れないため、書き出したゲームでは曲を読めませんでした
- **CLAP プラグイン(Surge XT 等)の音源のトラックは、Glaux の「音声にする(フリーズ)」でゲームでも鳴らせます**

### 更新のしかた(0.2 からの変更点あり)

1. いつもどおり `godot\build_windows.bat` で配布物を作り、ゲーム側の `addons\glaux\` を丸ごと置き換える
   (新しいファイル `plugin.cfg` / `plugin.gd` / `export_plugin.gd` が入ります)
2. Godot のエディタを開き直し、**「プロジェクト → プロジェクト設定 → プラグイン」で「Glaux」を有効にする**
   (これが書き出しに曲を入れます。一度有効にすれば project.godot に保存されます)
3. 書き出しのプリセットの設定は変えなくて構いません

### CLAP のトラックをゲームで鳴らす

Glaux でそのトラックの見出しを右クリック →「🧊 音声にする(フリーズ)」。エフェクト・音量・パン・センドの響きまで込みで
音声トラックになり、元のトラックはミュートされます(Ctrl+Z で戻せます)。保存すればゲームにそのまま反映されます。
`get_warnings()` の「CLAP プラグインのため、ゲームでは鳴りません」はこの手順を案内するようになりました。

### そのほか(Glaux 本体の改善で、ゲームの音にも効くもの)

- SoundFont の音の余韻が仕様どおりの長さに(以前は約 11 倍長かった)。内蔵シンセ(subtractive)の release も
  「その秒数で消える」に揃いました。**以前より余韻が短く聞こえる曲があります**(Glaux で release を長くして調整)
- マスターに常に掛かっていた軽い歪み(tanh)をやめ、大きすぎる音だけを抑えるようにしました
- 同時に鳴らせる音が 64 → 256 に増え、超えたときも新しい音が消えずに古い音から止まります
- サンプル・SoundFont の読み出しの補間が良くなりました(高い音のざらつきが減る)

---

## 0.2.1(2026-09-25)エディタのノードに Glaux のアイコン

- Godot のエディタで `GlauxPlayer` ノードに Glaux のフクロウのアイコンが付くようになりました
  (シーンツリーやノードの追加画面で見分けやすくなります)。アイコンの画像は `addons/glaux/glaux_player.png`
- 機能・メソッドの変更はありません。更新のしかたは 0.2.0 と同じです(配布物で `addons/glaux/` を丸ごと置き換える)

---

## 0.2.0(2026-09-25)効果音を WAV なしで鳴らし、音程を BGM のコードに合わせる

### 要点

- **効果音を WAV に書き出さずに、Glaux の曲のトラックから 1 音ずつ鳴らせるようになりました。**
  好きな音程で鳴らせるので、`pitch_scale` で WAV を上げ下げする必要がなく、音色・長さが崩れません
- **BGM のキーと、その時々のコードを問い合わせられるようになりました。** 効果音の音程を BGM に合わせるのに使います
- **BGM の拍に合わせて予約した効果音は、BGM とサンプル単位でそろいます**(測定で 0.1ms 以内)
- 既存の機能・メソッドの変更はありません。今のゲームのコードはそのまま動きます

### 追加したもの

**効果音を鳴らす**(効果音用の `GlauxPlayer` で使う)

| メソッド・プロパティ | 内容 |
|---|---|
| `play_note(track, pitch, velocity = 100, duration = 0.2)` | トラック(の音源とエフェクト)で 1 音をすぐ鳴らす。曲を再生していなくても鳴る。プレイヤーの入力に反応する音向け |
| `play_note_at(track, pitch, time, velocity = 100, duration = 0.2)` | `time` 秒に**聞こえるように**鳴らす(出力の遅れを見込む)。敵の攻撃など、拍に合わせる音向け |
| `sync_to` | `play_note_at` の `time` の基準にする BGM の `GlauxPlayer`。効果音用のプレイヤーに設定する |
| `release_notes()` | 鳴らした音をすべて離す |

**BGM の和声を問い合わせる**(BGM の `GlauxPlayer` で使う)

| メソッド | 返り値 |
|---|---|
| `get_key()` | キー `{name: "A minor", tonic: 9, mode: "minor", confidence}` |
| `get_chords()` / `get_chord_at(time)` | 小節ごとのコードの一覧 / その時刻のコード名("Am" / "G7" など) |
| `get_chord_tones(time)` / `get_scale_pitch_classes()` | コードの構成音 / キーのスケールの音(C=0〜B=11 の番号) |
| `get_chord_note(time, index, base = 60)` | その時刻のコードの構成音を、`base`(MIDI 番号)以上で下から数えた `index` 番目の音 |
| `get_scale_note(index, base = 60)` | キーのスケールの音を、`base` 以上で下から数えた `index` 番目の音 |
| `snap_to_chord(pitch, time)` / `snap_to_scale(pitch)` | 決めた音を、コード / スケールのいちばん近い音に寄せる |

### 使い方の例

```gdscript
@onready var music: GlauxPlayer = $Music   # BGM
@onready var sfx: GlauxPlayer = $Sfx       # 効果音(play は呼ばない)

func _ready() -> void:
	music.load_song("res://songs/Stage1.glaux")
	sfx.load_song("res://songs/SEKit.glaux")   # 効果音用の曲(トラックごとに音色を作っておく)
	sfx.sync_to = music                        # play_note_at の時刻 = BGM の時刻

# 敵の攻撃: BGM の次の拍に、その拍のコードの構成音で
func enemy_attack() -> void:
	var t := music.get_next_beat_time()
	sfx.play_note_at("Hit", music.get_chord_note(t, 0, 48), t)

# プレイヤーの入力: すぐ鳴らす。いまのコードの構成音を、コンボに応じて上っていく
func on_player_hit(combo: int) -> void:
	var now := music.get_song_time()
	sfx.play_note("Blip", music.get_chord_note(now, combo, 72), 110, 0.15)
```

ゲーム側の AI に任せる場合は、`PROMPT.md` の「6. 効果音を曲のキー・コードに合わせて鳴らす」をそのまま渡してください。

### 制限・注意

- 鳴らせるのは内蔵音源・SoundFont・サンプラーのトラックです。**CLAP プラグイン(Surge XT 等)のトラックは鳴りません**
  (`play_note` が false を返し、`get_warnings()` に理由が出ます)
- 効果音用の曲で**ミュート・ソロにしたまま保存したトラックは鳴りません。** Glaux で試聴した後は外して保存してください
- コードは**ノートからの推定で、1 小節に 1 つ**です。小節の途中でコードが変わる曲では、その小節でいちばん合うコードになります
- 1 つの `GlauxPlayer` で同時に鳴る音は 32 まで(超えると古い音から止まる)、予約は一度に 512 まで
- 音程が要らない音・重い音は、今までどおり WAV のままでかまいません(併用できます)

### Glaux アプリ側の関連する更新

- **曲をまとめたフォルダを「開く」で選べるようになりました。** 曲名のメニュー →「フォルダを選択して開く…」でゲームの
  `songs` フォルダを選ぶと、中の曲の一覧から開けます。曲が無ければ「このフォルダに新しい曲を作る」が出ます
  (以前は「Glaux プロジェクトではありません」で止まっていました)
- **おすすめ: 曲はゲームの `songs/` の中に直接作る / 開く。** Glaux で直した内容がそのままゲームに入り、コピーが要りません
- どこでも起動できるリリース版(`Glaux.exe` とインストーラー)を作れるようになりました(`scripts\build-release.bat`)

### 更新のしかた

1. Glaux 側で `godot\build_windows.bat` を実行して配布物を作る(Godot のエディタは閉じておく)
2. できた `godot\dist\glaux-godot-addon.zip` の中身で、ゲーム側の `addons\glaux\` を**丸ごと置き換える**
   (DLL と説明書が同じ版にそろいます)
3. Godot のエディタを開き直す
4. 更新できたかの確認: スクリプトで `music.has_method("play_note")` が true なら 0.2.0 以降です

---

## 0.1.0(2026-09-24)初版: 曲の再生と、拍・マーカー・ノートへの同期

- `GlauxPlayer` ノードで Glaux の曲(`.glaux` フォルダ)をゲームの中で鳴らす。音は Glaux の再生エンジンそのもの
- 「いま聞こえている位置」(出力の遅れを補正済み)で出来事を知らせるシグナル: `beat` / `section` / `note`(`watch_track`
  したトラックの音)/ `song_finished`
- 位置と先読み: `get_song_time` / `get_beat_position` / `get_bar` / `get_beat` / `get_bpm` / `get_next_beat_time` /
  `get_section` / `get_time_of` / `get_notes` / `get_beats` / `get_sections`
- 操作: `load_song` / `play` / `play_from` / `pause` / `resume` / `stop` / `seek`、プロパティ `bus` / `volume_db` /
  `latency_offset_ms`
- 説明書一式(`README.md` / `AI_GUIDE.md` / `PROMPT.md`)と、配布物を作る `godot\build_windows.bat`
- デモ(`godot/demo`)。画面に拍・小節・マーカー・キックを表示
- 制限: CLAP プラグインの音源のトラックは無音、ゲームを書き出した後の読み込みは未確認
