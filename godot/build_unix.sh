#!/usr/bin/env bash
# Glaux の Godot 拡張を Linux / macOS 向けにビルドする(release)。
# - デモのアドオン(godot/demo/addons/glaux/bin)へ .so / .dylib を置く
# - 配布用のアドオン(godot/dist/addons/glaux と godot/dist/glaux-godot-addon.zip)に加える
#   (dist の bin にある他の OS の DLL などは残すので、Windows で作った dist に足して 1 つの zip にできる)
# macOS で --universal を付けると、Apple Silicon と Intel の両方を 1 つの .dylib にまとめる
# (rustup target add aarch64-apple-darwin x86_64-apple-darwin が要る)。
# Rust 1.94 以降が要る。
set -euo pipefail
cd "$(dirname "$0")/.."

T="${CARGO_TARGET_DIR:-target}"
case "$(uname -s)" in
  Linux)
    cargo build -p glaux-godot --release
    LIB="$T/release/libglaux_godot.so"
    ;;
  Darwin)
    if [ "${1:-}" = "--universal" ]; then
      cargo build -p glaux-godot --release --target aarch64-apple-darwin
      cargo build -p glaux-godot --release --target x86_64-apple-darwin
      mkdir -p "$T/universal"
      lipo -create \
        "$T/aarch64-apple-darwin/release/libglaux_godot.dylib" \
        "$T/x86_64-apple-darwin/release/libglaux_godot.dylib" \
        -output "$T/universal/libglaux_godot.dylib"
      LIB="$T/universal/libglaux_godot.dylib"
    else
      cargo build -p glaux-godot --release
      LIB="$T/release/libglaux_godot.dylib"
    fi
    ;;
  *)
    echo "Linux か macOS で実行してください(Windows は godot/build_windows.bat)" >&2
    exit 1
    ;;
esac

SRC=godot/demo/addons/glaux
mkdir -p "$SRC/bin"
cp -f "$LIB" "$SRC/bin/"
echo "Copied to $SRC/bin/$(basename "$LIB")"

# --- 配布用のアドオン ---
DIST=godot/dist/addons/glaux
mkdir -p "$DIST/bin"
for f in glaux.gdextension glaux_player.png plugin.cfg plugin.gd export_plugin.gd README.md AI_GUIDE.md PROMPT.md CHANGELOG.md; do
  cp -f "$SRC/$f" "$DIST/"
done
cp -f "$LIB" "$DIST/bin/"
rm -f godot/dist/glaux-godot-addon.zip
if command -v zip >/dev/null 2>&1; then
  (cd godot/dist && zip -qr glaux-godot-addon.zip addons)
else
  (cd godot/dist && python3 -m zipfile -c glaux-godot-addon.zip addons)
fi
echo "Made $DIST and godot/dist/glaux-godot-addon.zip"
