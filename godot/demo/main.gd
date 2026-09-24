# Glaux の曲を鳴らし、拍・マーカー・キックに合わせて出来事を表示するデモ。
# 動作確認(ヘッドレス)では、コマンドライン引数に --check を付けると数秒で結果を出して終わる。
extends Node

@onready var music: GlauxPlayer = $GlauxPlayer

var beats := 0
var kicks := 0
var sections: Array[String] = []
var peak := 0.0
var capture: AudioEffectCapture
var errors: Array[String] = []

func _ready() -> void:
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
	music.play()
	if "--check" in OS.get_cmdline_user_args():
		await get_tree().create_timer(4.5).timeout
		_report()

func _process(_delta: float) -> void:
	if capture:
		var n := capture.get_frames_available()
		if n > 0:
			for f in capture.get_buffer(n):
				peak = max(peak, abs(f.x), abs(f.y))

func _on_beat(bar: int, beat: int, time: float) -> void:
	beats += 1
	# シグナルは「聞こえている位置」が拍を過ぎた直後に届く(遅れは 1 フレーム程度)
	var late := music.get_song_time() - time
	if late < -0.001 or late > 0.1:
		errors.append("拍 %d:%d が %.3f 秒ずれて届いた" % [bar, beat, late])
	if beat == 1:
		print("小節 ", bar, " (", snappedf(time, 0.001), " 秒) 拍位置 ", snappedf(music.get_beat_position(), 0.01))

func _on_note(track: String, pitch: int, _velocity: int, _time: float, _duration: float) -> void:
	if track == "Kick" and pitch == 36:
		kicks += 1

func _on_section(name: String, time: float) -> void:
	sections.append(name)
	print("マーカー「", name, "」 ", snappedf(time, 0.001), " 秒")

func _on_finished() -> void:
	print("曲が終わりました")

func _report() -> void:
	var t := music.get_song_time()
	print("RESULT time=%.3f beats=%d kicks=%d sections=%s peak=%.3f mixes=%d latency=%.3f errors=%d" % [
		t, beats, kicks, sections, peak, music.get_mix_count(), AudioServer.get_output_latency(), errors.size()])
	for e in errors:
		print("  ", e)
	get_tree().quit.call_deferred()
