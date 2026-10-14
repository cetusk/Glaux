#!/usr/bin/env bash
# 同じ曲がゲーム(Glaux for Godot)でもアプリと同じ音で鳴るかを確かめる(Linux)。
#   GODOT=/path/to/Godot_v4.3-stable_linux.x86_64 godot/check_render.sh [曲のフォルダ ...]
# 曲を省くとデモの曲(godot/demo/songs/*.glaux)。各曲を Godot で画面なしで鳴らして録り(tools/record_song.gd)、
# アプリの書き出しと同じ描き出しと比べる(glaux-engine の example render_compare)。差が -40 dB 以下なら ok。
# CLAP の音源のトラックはゲームでは鳴らないので、比べるときも両方で無音にする。
# 録る長さは REC_SECONDS(既定 30 秒。曲がそれより短ければ曲の終わりまで)。
# 最後に、ノートの表情とゲームの中で曲を変える関数が動くかも確かめる(tools/check_expression.gd)
set -euo pipefail
cd "$(dirname "$0")/.."
GODOT="${GODOT:?GODOT に Godot 4.3 以降の実行ファイルを指定してください}"
T="${CARGO_TARGET_DIR:-target}"
LEN="${REC_SECONDS:-30}"

cargo build -q -p glaux-godot --release
mkdir -p godot/demo/addons/glaux/bin
cp "$T/release/libglaux_godot.so" godot/demo/addons/glaux/bin/
cargo build -q -p glaux-engine --release --example render_compare
# 初回は拡張を登録するためにインポートする
if [ ! -f godot/demo/.godot/extension_list.cfg ]; then
  "$GODOT" --headless --path godot/demo --editor --quit >/dev/null 2>&1 || true
fi

if [ $# -eq 0 ]; then
  set -- godot/demo/songs/*.glaux
fi
OUT="$(mktemp -d)"
trap 'rm -rf "$OUT" godot/demo/.check' EXIT
fail=0
for song in "$@"; do
  song="${song%/}"
  name="$(basename "$song")"
  # Godot から読めるよう、デモの外の曲は godot/demo/.check に写す
  case "$(realpath "$song")" in
    "$(realpath godot/demo)"/*) res="res://${song#godot/demo/}" ;;
    *)
      mkdir -p godot/demo/.check
      rm -rf "godot/demo/.check/$name"
      cp -r "$song" "godot/demo/.check/$name"
      res="res://.check/$name"
      ;;
  esac
  echo "== $name"
  if ! "$GODOT" --headless --path godot/demo -s res://tools/record_song.gd -- \
      --song "$res" --out "$OUT/$name.wav" --seconds "$LEN" 2>&1 | grep -E "^RECORD_"; then
    echo "RESULT ng Godot で録れませんでした"
    fail=1
    continue
  fi
  "$T/release/examples/render_compare" "$song" "$OUT/$name.wav" || fail=1
done
echo "== 表情と、ゲームの中で曲を変える関数"
"$GODOT" --headless --path godot/demo -s res://tools/check_expression.gd 2>&1 | grep -E "^CHECK" || true
"$GODOT" --headless --path godot/demo -s res://tools/check_expression.gd >/dev/null 2>&1 || fail=1
exit $fail
