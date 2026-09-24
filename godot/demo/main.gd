# Glaux の曲を鳴らし、拍・マーカー・キックに合わせて画面を動かすデモ。
# - 円: 拍の頭で膨らむ(get_beat_position で毎フレーム滑らかに)
# - 4 つの四角: 小節の中の今の拍(beat シグナル)
# - 背景の光: キックが鳴った瞬間(note シグナル)
# 操作: スペース = 再生 / 一時停止、R = 最初から
# 動作確認(ヘッドレス)では、コマンドライン引数に --check を付けると数秒で結果を出して終わる。
extends Node2D

@onready var music: GlauxPlayer = $GlauxPlayer

const BG := Color(0.08, 0.09, 0.12)
const ACCENT := Color(0.3, 0.85, 0.75)
const DOWNBEAT := Color(1.0, 0.75, 0.3)
const KICK_FLASH := Color(0.35, 0.2, 0.45)
const DIM := Color(0.55, 0.58, 0.65)

var font: Font
var beats := 0
var kicks := 0
var sections: Array[String] = []
var peak := 0.0
var capture: AudioEffectCapture
var errors: Array[String] = []
var check_mode := false

# 画面表示用
var cur_bar := 0
var cur_beat := 0
var beats_in_bar := 4
var flash := 0.0           # キックの光(1 → 0 へ減衰)
var section_name := ""
var finished := false
var log_lines: Array[String] = []

func _ready() -> void:
	check_mode = "--check" in OS.get_cmdline_user_args()
	# 日本語が確実に出るよう、OS の日本語フォントを使う
	var sf := SystemFont.new()
	sf.font_names = PackedStringArray(["Yu Gothic UI", "Meiryo", "Noto Sans CJK JP", "Noto Sans JP", "sans-serif"])
	font = sf
	# マスターに録音用のエフェクトを挿して、実際に音が出ているか測る
	if not "--nocap" in OS.get_cmdline_user_args():
		capture = AudioEffectCapture.new()
		AudioServer.add_bus_effect(0, capture)
	if not music.load_song("res://songs/demo.glaux"):
		push_error("読み込み失敗: " + music.get_last_error())
		get_tree().quit(1)
		return
	print("トラック: ", music.get_track_names(), " 長さ: ", music.get_length(), " 秒")
	print("マーカー: ", music.get_sections())
	print("最初の 2 拍: ", music.get_beats(0.0, 1.0))
	music.watch_track("Kick")
	music.beat.connect(_on_beat)
	music.note.connect(_on_note)
	music.section.connect(_on_section)
	music.song_finished.connect(_on_finished)
	# 起動直後のフレームは重いので、1 フレーム待ってから鳴らす(最初の拍のシグナルが遅れないように)
	await get_tree().process_frame
	_restart()
	if check_mode:
		await get_tree().create_timer(4.5).timeout
		_report()

func _restart() -> void:
	beats = 0
	kicks = 0
	sections.clear()
	errors.clear()
	log_lines.clear()
	finished = false
	section_name = ""
	music.play()

func _unhandled_input(event: InputEvent) -> void:
	if event is InputEventKey and event.pressed and not event.echo:
		match event.keycode:
			KEY_SPACE:
				if finished or not music.is_playing() and music.get_song_time() <= 0.0:
					_restart()
				elif music.is_playing():
					music.pause()
				else:
					music.resume()
			KEY_R:
				_restart()

func _process(delta: float) -> void:
	if capture:
		var n := capture.get_frames_available()
		if n > 0:
			for f in capture.get_buffer(n):
				peak = max(peak, abs(f.x), abs(f.y))
	flash = max(0.0, flash - delta / 0.15)
	queue_redraw()

func _on_beat(bar: int, beat: int, time: float) -> void:
	beats += 1
	cur_bar = bar
	cur_beat = beat
	var b := music.get_beats(time - 0.001, time + 0.001)
	if b.size() > 0:
		beats_in_bar = b[0].beats_in_bar
	# シグナルは「聞こえている位置」が拍を過ぎた直後に届く(遅れは 1 フレーム程度)
	var late := music.get_song_time() - time
	if late < -0.001 or late > 0.1:
		errors.append("拍 %d:%d が %.3f 秒ずれて届いた" % [bar, beat, late])
	if beat == 1:
		_log("小節 %d(%.2f 秒)" % [bar, time])
		print("小節 ", bar, " (", snappedf(time, 0.001), " 秒) 拍位置 ", snappedf(music.get_beat_position(), 0.01))

func _on_note(track: String, pitch: int, _velocity: int, _time: float, _duration: float) -> void:
	if track == "Kick" and pitch == 36:
		kicks += 1
		flash = 1.0

func _on_section(name: String, time: float) -> void:
	sections.append(name)
	section_name = name
	_log("マーカー「%s」(%.2f 秒)" % [name, time])
	print("マーカー「", name, "」 ", snappedf(time, 0.001), " 秒")

func _on_finished() -> void:
	finished = true
	_log("曲が終わりました(スペース / R でもう一度)")
	print("曲が終わりました")

func _log(s: String) -> void:
	log_lines.append(s)
	if log_lines.size() > 4:
		log_lines.pop_front()

func _draw() -> void:
	var size := get_viewport_rect().size
	# 背景(キックで光る)
	draw_rect(Rect2(Vector2.ZERO, size), BG.lerp(KICK_FLASH, flash))

	var t := music.get_song_time()
	var len := music.get_length()

	# 拍で膨らむ円(拍の中の位置 0〜1 で大きさを決める)
	var center := Vector2(size.x * 0.3, size.y * 0.45)
	var phase := fmod(max(music.get_beat_position(), 0.0), 1.0)
	var radius := 80.0 * (1.0 + 0.35 * pow(1.0 - phase, 3.0))
	var col := DOWNBEAT if cur_beat == 1 else ACCENT
	if not music.is_playing():
		col = DIM
	draw_circle(center, radius, col)

	# 小節の中の拍(今の拍を明るく)
	var box := 28.0
	var gap := 12.0
	var total := beats_in_bar * box + (beats_in_bar - 1) * gap
	for i in beats_in_bar:
		var pos := Vector2(center.x - total / 2.0 + i * (box + gap), center.y + 140.0)
		var on := music.is_playing() and i + 1 == cur_beat
		var c := (DOWNBEAT if i == 0 else ACCENT) if on else Color(1, 1, 1, 0.15)
		draw_rect(Rect2(pos, Vector2(box, box)), c)

	# 文字の情報
	var x := size.x * 0.55
	var y := size.y * 0.2
	draw_string(font, Vector2(x, y), "Glaux デモ", HORIZONTAL_ALIGNMENT_LEFT, -1, 32, Color.WHITE)
	y += 56
	var state := "終わり" if finished else "一時停止"
	if music.is_playing():
		# 曲の長さを過ぎても、最後の音の余韻が消えるまで(約 2 秒)は鳴り続ける
		state = "再生中(余韻)" if t > len else "再生中"
	var lines := [
		"小節 %d : 拍 %d" % [cur_bar, cur_beat],
		"時刻 %.2f / %.2f 秒" % [max(t, 0.0), len],
		"BPM %.1f" % music.get_bpm(),
		"マーカー: %s" % (section_name if section_name != "" else "-"),
		"キック: %d 回" % kicks,
		"状態: %s" % state,
	]
	for l in lines:
		draw_string(font, Vector2(x, y), l, HORIZONTAL_ALIGNMENT_LEFT, -1, 22, Color.WHITE)
		y += 34

	# 再生位置のバー
	var bar_rect := Rect2(Vector2(40, size.y - 90), Vector2(size.x - 80, 10))
	draw_rect(bar_rect, Color(1, 1, 1, 0.12))
	if len > 0.0:
		var w := bar_rect.size.x * clampf(t / len, 0.0, 1.0)
		draw_rect(Rect2(bar_rect.position, Vector2(w, bar_rect.size.y)), ACCENT)
	# マーカーの位置
	for s in music.get_sections():
		if len > 0.0:
			var mx: float = bar_rect.position.x + bar_rect.size.x * clampf(s.time / len, 0.0, 1.0)
			draw_line(Vector2(mx, bar_rect.position.y - 6), Vector2(mx, bar_rect.end.y + 6), DOWNBEAT, 2.0)
			draw_string(font, Vector2(mx + 4, bar_rect.position.y - 10), s.name, HORIZONTAL_ALIGNMENT_LEFT, -1, 14, DIM)

	# 出来事の記録(文字の情報のすぐ下。新しいものが下)
	y += 10
	for l in log_lines:
		draw_string(font, Vector2(x, y), l, HORIZONTAL_ALIGNMENT_LEFT, -1, 16, DIM)
		y += 22

	draw_string(font, Vector2(40, size.y - 40), "スペース: 再生 / 一時停止    R: 最初から", HORIZONTAL_ALIGNMENT_LEFT, -1, 16, DIM)

func _report() -> void:
	var t := music.get_song_time()
	print("RESULT time=%.3f beats=%d kicks=%d sections=%s peak=%.3f mixes=%d latency=%.3f errors=%d" % [
		t, beats, kicks, sections, peak, music.get_mix_count(), AudioServer.get_output_latency(), errors.size()])
	for e in errors:
		print("  ", e)
	get_tree().quit.call_deferred()
