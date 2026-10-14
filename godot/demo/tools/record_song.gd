# 曲を GlauxPlayer で鳴らし、マスターの音を WAV に録る(アプリの書き出しと同じ音になるかの確かめ用)。
# 画面なしで動かす:
#   godot --headless --path godot/demo -s res://tools/record_song.gd -- --song res://songs/demo.glaux --out /tmp/godot.wav --seconds 20
# 録った WAV は、アプリ側の描き出しと比べる(cargo run -p glaux-engine --example render_compare -- <曲のフォルダ> <WAV>)。
# 使い方の全体は godot/check_render.sh
extends SceneTree

var player: GlauxPlayer
var rec: AudioEffectRecord
var out_path := ""
var seconds := 20.0
var started := 0
var done := false


func _arg(name: String, fallback: String) -> String:
	var args := OS.get_cmdline_user_args()
	var i := args.find(name)
	return args[i + 1] if i >= 0 and i + 1 < args.size() else fallback


func _initialize() -> void:
	out_path = _arg("--out", "user://godot.wav")
	seconds = float(_arg("--seconds", "20"))
	rec = AudioEffectRecord.new()
	AudioServer.add_bus_effect(0, rec)
	player = GlauxPlayer.new()
	root.add_child(player)


# 場面に入ってから(最初のフレームで)読み込んで鳴らす。_initialize の中ではまだ鳴らせない
func _start() -> bool:
	var song := _arg("--song", "res://songs/demo.glaux")
	if not player.load_song(song):
		printerr("RECORD_ERROR 読み込めません: ", player.get_last_error())
		quit(1)
		return false
	for w in player.get_warnings():
		print("RECORD_WARN ", w)
	print("RECORD_RATE ", AudioServer.get_mix_rate())
	rec.set_recording_active(true)
	player.play()
	started = Time.get_ticks_msec()
	return true


func _process(_delta: float) -> bool:
	if done:
		return false
	if started == 0:
		_start()
		return false
	# 曲が終わる(自動で止まる)か、決めた秒数が過ぎたら録り終える
	var elapsed := (Time.get_ticks_msec() - started) / 1000.0
	if elapsed < seconds and (player.is_playing() or elapsed < 1.0):
		return false
	done = true
	rec.set_recording_active(false)
	var wav := rec.get_recording()
	if wav == null:
		printerr("RECORD_ERROR 録れませんでした(音声の出力が動いていない)")
		quit(1)
		return true
	var err := wav.save_to_wav(out_path)
	print("RECORD_DONE ", out_path, " ", "%.1f" % elapsed, "s mixes=", player.get_mix_count(), " err=", err)
	quit(0 if err == OK else 1)
	return true
