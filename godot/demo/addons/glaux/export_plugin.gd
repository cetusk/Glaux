@tool
extends EditorExportPlugin
## ゲームを書き出すとき、Glaux の曲をそのまま .pck に入れる。
##
## GlauxPlayer は曲フォルダの project.json と audio/ の WAV、SoundFont(曲フォルダの soundfonts/ か
## res://soundfonts/)を FileAccess で読む。ところが Godot の書き出しは
## - .json や .sf2 を書き出しに含めない
## - WAV を取り込み(インポート)後の形に置き換えて、元のファイルを含めない
## ので、そのままでは書き出したゲームで曲を読めない。ここで元のファイルを追加し、取り込み版は外す。


func _get_name() -> String:
	return "Glaux"


func _export_begin(_features: PackedStringArray, _is_debug: bool, _path: String, _flags: int) -> void:
	_add_songs("res://")
	_add_folder("res://soundfonts")


## .glaux フォルダの中身は下の _export_begin で元のまま入れるので、Godot の取り込み版は入れない
func _export_file(path: String, _type: String, _features: PackedStringArray) -> void:
	if ".glaux/" in path:
		skip()


## res:// の下を探して、見つけた .glaux フォルダの曲を入れる
func _add_songs(dir: String) -> void:
	var d := DirAccess.open(dir)
	if d == null:
		return
	d.include_hidden = false
	for sub in d.get_directories():
		if sub.begins_with("."):
			continue  # .godot などの内部のフォルダ
		var path := dir.path_join(sub)
		if sub.ends_with(".glaux"):
			_add_song(path)
		else:
			_add_songs(path)


## 曲フォルダから、ゲームで読むもの(project.json・audio/・soundfonts/)だけを入れる。
## history.jsonl(編集の履歴)や cache/ は入れない
func _add_song(song: String) -> void:
	_add_raw(song.path_join("project.json"))
	_add_folder(song.path_join("audio"))
	_add_folder(song.path_join("soundfonts"))


func _add_folder(dir: String) -> void:
	var d := DirAccess.open(dir)
	if d == null:
		return
	for f in d.get_files():
		if f.ends_with(".import") or f.ends_with(".uid") or f.begins_with("."):
			continue
		_add_raw(dir.path_join(f))


func _add_raw(path: String) -> void:
	if not FileAccess.file_exists(path):
		return
	add_file(path, FileAccess.get_file_as_bytes(path), false)
