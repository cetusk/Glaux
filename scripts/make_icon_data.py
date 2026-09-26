#!/usr/bin/env python3
"""画面のアイコン(app/src/lib/icons.ts)を Lucide の SVG から作る。

使い方: npm pack lucide-static && tar xzf lucide-static-*.tgz
        python3 scripts/make_icon_data.py package
アイコンを足すときは NAMES に名前(https://lucide.dev/icons/ の名前)を足して作り直す。
ライセンス(ISC、一部 MIT)は app/LICENSE-lucide.txt にそのまま置く。
"""
import json
import re
import shutil
import sys
from pathlib import Path

NAMES = """
skip-back rewind play pause square fast-forward skip-forward repeat infinity metronome circle
undo-2 redo-2 rotate-ccw download file-music settings volume-2 sliders-horizontal keyboard-music
spline pencil-line ellipsis x refresh-cw folder-open audio-lines merge plus minus snowflake scissors
copy trash-2 sparkles history plug search wand-sparkles chevron-down chevron-right chevron-up
grip-vertical power app-window save mic speaker cpu audio-waveform drum guitar bell waves library
file-audio pencil arrow-up arrow-down palette layers music list-music move-horizontal circle-help
chart-no-axes-column rows-2 arrow-up-down magnet link check piano headphones triangle-alert info
loader-circle eye send square-stop message-square-plus folder file-plus-2 hand-metal ruler target clapperboard
unplug pin archive split scissors layout-grid zoom-in zoom-out
""".split()

ROOT = Path(__file__).resolve().parent.parent


def main() -> None:
    pkg = Path(sys.argv[1] if len(sys.argv) > 1 else "package")
    version = json.loads((pkg / "package.json").read_text())["version"]
    out = {}
    for n in NAMES:
        svg = (pkg / "icons" / f"{n}.svg").read_text()
        inner = svg[svg.index(">", svg.index("<svg")) + 1 : svg.rindex("</svg>")]
        out[n] = re.sub(r"\s+", " ", inner).strip()
    lines = [
        "// @generated scripts/make_icon_data.py から作る(手で書き換えない)。",
        f"// アイコン: Lucide(lucide-static v{version})https://lucide.dev — ISC ライセンス(一部 MIT)。",
        "// ライセンスの全文は app/LICENSE-lucide.txt。",
        "",
        "export const ICONS = {",
    ]
    lines += [f"  {json.dumps(k)}: {json.dumps(v)}," for k, v in out.items()]
    lines += ["} as const;", "", "export type IconName = keyof typeof ICONS;", ""]
    (ROOT / "app/src/lib/icons.ts").write_text("\n".join(lines), encoding="utf-8")
    shutil.copy(pkg / "LICENSE", ROOT / "app/LICENSE-lucide.txt")
    print(f"{len(out)} 個を書きました")


if __name__ == "__main__":
    main()
