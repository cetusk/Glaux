# ノートの表情(奏法・しゃくりなど)と、ゲームの中で曲を変える関数が Godot の上で動くかを確かめる。
#   godot --headless --path godot/demo -s res://tools/check_expression.gd
# 結果は 1 行ずつ「CHECK ok/ng 内容」。ng が 1 つでもあれば終了コード 1
extends SceneTree

var music: GlauxPlayer
var sfx: GlauxPlayer
var cap: AudioEffectCapture
var step := 0
var t0 := 0
var note_ex_count := 0
var note_ex_keys_ok := true
var failed := false
# この区切りの間に聞こえた音の最大(録音のバッファはいっぱいになると新しい音を捨てるので、毎フレーム読む)
var win_peak := 0.0


func _check(ok: bool, what: String, detail := "") -> void:
	print("CHECK ", "ok " if ok else "ng ", what, ("  " + detail) if detail != "" else "")
	if not ok:
		failed = true


func _initialize() -> void:
	cap = AudioEffectCapture.new()
	AudioServer.add_bus_effect(0, cap)
	music = GlauxPlayer.new()
	sfx = GlauxPlayer.new()
	root.add_child(music)
	root.add_child(sfx)


func _peak() -> float:
	var n := cap.get_frames_available()
	var p := 0.0
	for f in cap.get_buffer(n):
		p = max(p, max(abs(f.x), abs(f.y)))
	return p


func _on_note_ex(_track: String, info: Dictionary) -> void:
	note_ex_count += 1
	for k in ["time", "duration", "pitch", "velocity", "articulation", "pitch_curve", "glide_ms", "vibrato"]:
		if not info.has(k):
			note_ex_keys_ok = false


func _process(_delta: float) -> bool:
	var now := Time.get_ticks_msec()
	win_peak = max(win_peak, _peak())
	match step:
		0:
			_check(music.load_song("res://songs/demo.glaux"), "曲を読み込む", music.get_last_error())
			_check(sfx.load_song("res://songs/sfx.glaux"), "効果音用の曲を読み込む", sfx.get_last_error())
			music.watch_track("Lead")
			music.watch_track("Kick")
			music.note_ex.connect(_on_note_ex)
			# 表情を足す(曲のファイルは変えない)
			_check(music.apply_gesture("Lead", "shakuri", 0.0, 60.0, "all"), "apply_gesture でしゃくりを足す", music.get_last_error())
			var curved := 0
			for n in music.get_notes("Lead", 0.0, 60.0):
				if (n["pitch_curve"] as Array).size() > 0:
					curved += 1
			_check(curved > 0, "get_notes に音程の曲線が出る", "%d 音" % curved)
			music.play()
			t0 = now
			step = 1
		1:
			if now - t0 < 3000:
				return false
			_check(note_ex_count > 0 and note_ex_keys_ok, "note_ex が表情つきで届く", "%d 回" % note_ex_count)
			music.stop()
			_check(music.set_tempo(140.0) and abs(music.get_bpm() - 140.0) < 0.01, "set_tempo", str(music.get_bpm()))
			_check(music.set_track_mute("Pad", true), "set_track_mute", music.get_last_error())
			_check(not sfx.play_note_ex("Bell", 72, 100, 0.3, "", "nonsense"), "読めない表情は false")
			t0 = now
			step = 2
		2:
			# 曲を止めて静かになってから、効果音を表情つきで鳴らす
			# 余韻が消えるのを待ち、最後の 0.5 秒の大きさを「静か」とする
			if now - t0 < 1500:
				win_peak = 0.0
				return false
			if now - t0 < 2000:
				return false
			var quiet := win_peak
			win_peak = 0.0
			_check(sfx.play_note_ex("Bell", 72, 110, 0.4, "", "shakuri", 100.0, 120.0), "play_note_ex でしゃくりの効果音")
			_check(sfx.play_note_ex_at("Blip", 76, 0.0, 100, 0.2, "staccato"), "play_note_ex_at で奏法つき")
			t0 = now
			step = 3
			set_meta("quiet", quiet)
		3:
			if now - t0 < 600:
				return false
			var loud := win_peak
			_check(loud > 0.05 and loud > float(get_meta("quiet")) * 2.0, "効果音が鳴る", "静か %.4f → %.3f" % [float(get_meta("quiet")), loud])
			_check(music.apply_command('{"op":"set_title","title":"check"}'), "apply_command(Command の JSON)", music.get_last_error())
			quit(1 if failed else 0)
			step = 4
	return false
