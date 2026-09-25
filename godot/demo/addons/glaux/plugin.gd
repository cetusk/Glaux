@tool
extends EditorPlugin
## Glaux のエディタ用プラグイン。ゲームの書き出しに曲(.glaux フォルダ)を含める書き出しプラグインを登録する。
## 「プロジェクト設定 → プラグイン」で Glaux を有効にしておくこと。

var _export: EditorExportPlugin


func _enter_tree() -> void:
	_export = preload("res://addons/glaux/export_plugin.gd").new()
	add_export_plugin(_export)


func _exit_tree() -> void:
	remove_export_plugin(_export)
	_export = null
