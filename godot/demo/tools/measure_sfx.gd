# 効果音を鳴らす速さ・同時に鳴らせる数・重さを測る(説明書のおすすめの置き方の根拠)。
#   godot --headless --path godot/demo -s res://tools/measure_sfx.gd
# 1 行ずつ「MEASURE 項目 値」を出す。重さは、そのパソコンで 1 つの音を作るのにかかる時間の割合(%)で、
# 速いパソコンほど小さい。比べるときは同じパソコンで
extends SceneTree

const SONG := "res://songs/demo.glaux"
const SFX := "res://songs/sfx.glaux"

var cap: AudioEffectCapture
var players: Array[GlauxPlayer] = []
var sfx: GlauxPlayer
var phase := 0
var t0 := 0
var trial := 0
var max_voices := 0
var queued_fail := 0


func _out(name: String, value: String) -> void:
	print("MEASURE ", name, " ", value)


func _peak() -> float:
	var p := 0.0
	for f in cap.get_buffer(cap.get_frames_available()):
		p = max(p, max(abs(f.x), abs(f.y)))
	return p


func _initialize() -> void:
	cap = AudioEffectCapture.new()
	cap.buffer_length = 0.05
	AudioServer.add_bus_effect(0, cap)
	sfx = GlauxPlayer.new()
	root.add_child(sfx)


func _add_bgm(n: int) -> void:
	while players.size() < n:
		var p := GlauxPlayer.new()
		root.add_child(p)
		p.load_song(SONG)
		p.set_loop(0.0, p.get_length())
		p.play()
		players.append(p)


func _process(_delta: float) -> bool:
	var now := Time.get_ticks_msec()
	match phase:
		0:
			sfx.load_song(SFX)
			_out("mix_rate", str(AudioServer.get_mix_rate()))
			_out("output_latency_ms", "%.1f" % (AudioServer.get_output_latency() * 1000.0))
			t0 = now
			phase = 1
		1:
			# 押してから鳴るまで = Glaux が次に音を作る区切りまで(最大 1 区切り)+ Godot の音の出口の遅れ。
			# 画面なしの仮の出口はまとめて作るので、ここでは時計で測らず、区切りの長さを数える
			if trial == 0:
				sfx.play_note("Blip", 72, 1, 0.01)
				trial = sfx.get_mix_count()
				t0 = now
			elif now - t0 > 2000:
				var mixes := sfx.get_mix_count() - trial
				var frames: float = 2.0 * AudioServer.get_mix_rate() / maxi(mixes, 1)
				_out("frames_per_mix", "%.0f" % frames)
				_out("sfx_latency_ms", "Glaux の分は最大 %.1f(1 区切り)+ Godot の出口の遅れ(設定 audio/driver/output_latency、既定 15)" % (frames / AudioServer.get_mix_rate() * 1000.0))
				phase = 2
				t0 = now
		2:
			# 同時に 64 音を頼む → 鳴るのは 32 まで
			for i in 64:
				sfx.play_note("Bell", 48 + i % 40, 90, 2.0)
			t0 = now
			phase = 3
		3:
			max_voices = max(max_voices, sfx.get_voice_count())
			if now - t0 > 300:
				_out("voices_after_64_requests", str(max_voices))
				sfx.get_load()
				t0 = now
				phase = 4
		4:
			if now - t0 > 1500:
				var l: Dictionary = sfx.get_load()
				_out("load_sfx_32_voices_pct", "平均 %.1f 最大 %.1f" % [l["avg"], l["max"]])
				sfx.release_notes()
				# 連打はゲームと同じ 60 fps で
				Engine.max_fps = 60
				phase = 5
				t0 = now
		5:
			# 毎フレーム鳴らす(60 fps で 3 秒の連打)
			if now - t0 < 3000:
				if not sfx.play_note("Blip", 60 + (now / 16) % 24, 100, 0.15):
					queued_fail += 1
				max_voices = max(max_voices, sfx.get_voice_count())
			else:
				var l: Dictionary = sfx.get_load()
				_out("rapid_fire_60fps", "声の数の最大 %d・断られた %d 回・重さ 平均 %.1f 最大 %.1f" % [sfx.get_voice_count(), queued_fail, l["avg"], l["max"]])
				sfx.release_notes()
				phase = 6
				trial = 1
		6:
			# 曲を鳴らすプレイヤーを 1・2・4・8 個にして重さを測る
			_add_bgm(trial)
			for p in players:
				p.get_load()
			t0 = now
			phase = 7
		7:
			if now - t0 > 2000:
				var avg := 0.0
				var mx := 0.0
				for p in players:
					var l: Dictionary = p.get_load()
					avg += float(l["avg"])
					mx = max(mx, float(l["max"]))
				_out("load_bgm_players_%d_pct" % trial, "合計の平均 %.1f・1 個の最大 %.1f" % [avg, mx])
				if trial >= 8:
					quit(0)
					phase = 99
				else:
					trial *= 2
					phase = 6
	return false
