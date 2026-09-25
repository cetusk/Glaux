#!/usr/bin/env python3
"""GitHub のリリースに載せるファイルを release/ にそろえる。

前もって Windows で:
  scripts\\build-release.bat   → release\\Glaux.exe と release\\Glaux_<版>_x64-setup.exe
  godot\\build_windows.bat     → godot\\demo\\addons\\glaux\\bin\\glaux_godot.dll
Linux で(サンドボックスなど):
  cargo build -p glaux-godot --release → libglaux_godot.so(このスクリプトが strip して入れる)

使い方: python3 scripts/make_release.py <libglaux_godot.so のパス>
できるもの(release/):
  Glaux-<版>-windows-x64.exe          単体の exe(Glaux.exe の名前を変えたもの)
  Glaux-<版>-windows-x64-setup.exe    インストーラー
  glaux-godot-addon-<アドオンの版>.zip   addons/glaux(Windows の .dll と Linux の .so 入り)
"""
import json
import re
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REL = ROOT / "release"
ADDON = ROOT / "godot/demo/addons/glaux"
# アドオンに入れるファイル(bin 以外。build_windows.bat と同じ)
ADDON_FILES = [
    "glaux.gdextension",
    "glaux_player.png",
    "plugin.cfg",
    "plugin.gd",
    "export_plugin.gd",
    "README.md",
    "AI_GUIDE.md",
    "PROMPT.md",
    "CHANGELOG.md",
]


def main() -> None:
    so = Path(sys.argv[1]) if len(sys.argv) > 1 else None
    app_ver = json.loads((ROOT / "app/src-tauri/tauri.conf.json").read_text(encoding="utf-8"))["version"]
    addon_ver = re.search(r'version="([^"]+)"', (ADDON / "plugin.cfg").read_text(encoding="utf-8")).group(1)

    # アプリ
    exe = REL / "Glaux.exe"
    setup = REL / f"Glaux_{app_ver}_x64-setup.exe"
    for f in (exe, setup):
        if not f.exists():
            sys.exit(f"{f} がありません(Windows で scripts\\build-release.bat を実行してください)")
    shutil.copy(exe, REL / f"Glaux-{app_ver}-windows-x64.exe")
    shutil.copy(setup, REL / f"Glaux-{app_ver}-windows-x64-setup.exe")

    # Godot のアドオン
    dll = ADDON / "bin/glaux_godot.dll"
    if not dll.exists():
        sys.exit(f"{dll} がありません(Windows で godot\\build_windows.bat を実行してください)")
    zpath = REL / f"glaux-godot-addon-{addon_ver}.zip"
    with zipfile.ZipFile(zpath, "w", zipfile.ZIP_DEFLATED) as z:
        for name in ADDON_FILES:
            z.write(ADDON / name, f"addons/glaux/{name}")
        z.write(dll, "addons/glaux/bin/glaux_godot.dll")
        if so:
            stripped = REL / "libglaux_godot.so"
            shutil.copy(so, stripped)
            subprocess.run(["strip", str(stripped)], check=True)
            z.write(stripped, "addons/glaux/bin/libglaux_godot.so")
            stripped.unlink()
        else:
            print("注意: libglaux_godot.so を渡していないので、Linux 用は入りません")

    for f in sorted(REL.glob(f"*{app_ver}-windows*")) + [zpath]:
        print(f"{f.name}  {f.stat().st_size / 1e6:.1f} MB")


if __name__ == "__main__":
    main()
