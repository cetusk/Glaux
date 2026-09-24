"""assets/ のブランドキットから、アプリと Godot アドオンのアイコンを作る。

使い方(Pillow が要る):  python scripts/make_icons.py

- 40px 以下は顔のみ版(assets/icons/favicon.png)。小さくても目元が分かるよう、余白を図形の 1/16 に詰め、
  2 段階で縮めてから小さい半径で鮮明化する(1 回で縮めるとぼやける)
- 48px 以上は白フチの全身版(assets/icons/glaux-app-icon.png)。余白は図形の 1/8(ブランドガイドの推奨)
- ICO には Windows が表示倍率ごとに使うサイズをすべて入れる(足りないと拡大・縮小されてぼやける)。
  並びは大きい順(Tauri はウィンドウのアイコンに ICO の最初の 1 枚を使うので、小さいものが先だと拡大されてぼやける)
- タスクバー用: 表示倍率ごとのタスクバーの大きさ(24px × 倍率)の画像を RGBA の生データで書き出す。
  アプリが起動時に表示倍率に合うものをウィンドウに設定する(app/src-tauri/src/main.rs の taskbar_icon)
"""

from pathlib import Path

from PIL import Image, ImageFilter

ROOT = Path(__file__).resolve().parent.parent
FACE = ROOT / "assets/icons/favicon.png"
FULL = ROOT / "assets/icons/glaux-app-icon.png"

# 100% / 125% / 150% / 200% などで Windows が使うサイズ
ICO_SIZES = [16, 20, 24, 30, 32, 36, 40, 48, 60, 64, 72, 80, 96, 128, 256]
# タスクバーのアイコンの大きさ(24px × 表示倍率 100 / 125 / 150 / 200 / 250 / 300 / 400%)
TASKBAR_SIZES = [24, 30, 36, 48, 60, 72, 96]
SMALL_MAX = 40


def squared(path: Path, pad_ratio: float) -> Image.Image:
    """見える図形を基準に正方形へ切り、周囲に図形の大きさ × pad_ratio の余白をつける。"""
    im = Image.open(path).convert("RGBA")
    x0, y0, x1, y1 = im.getchannel("A").getbbox()
    side = max(x1 - x0, y1 - y0)
    pad = int(side * pad_ratio)
    c = Image.new("RGBA", (side + 2 * pad, side + 2 * pad), (0, 0, 0, 0))
    c.alpha_composite(
        im.crop((x0, y0, x1, y1)),
        (pad + (side - (x1 - x0)) // 2, pad + (side - (y1 - y0)) // 2),
    )
    return c


def shrink(src: Image.Image, size: int, amount: int) -> Image.Image:
    """2 段階(途中は 4 倍)で縮め、色と輪郭(アルファ)に小さい半径のアンシャープをかける。"""
    mid = src.resize((size * 4, size * 4), Image.LANCZOS)
    im = mid.resize((size, size), Image.LANCZOS)
    if amount <= 0:
        return im
    rgb = im.convert("RGB").filter(ImageFilter.UnsharpMask(radius=0.6, percent=amount, threshold=0))
    alpha = im.getchannel("A").filter(
        ImageFilter.UnsharpMask(radius=0.5, percent=amount * 2 // 3, threshold=0)
    )
    out = rgb.convert("RGBA")
    out.putalpha(alpha)
    return out


def icon(size: int) -> Image.Image:
    if size <= SMALL_MAX:
        return shrink(squared(FACE, 1 / 16), size, 120)
    return shrink(squared(FULL, 1 / 8), size, 60 if size <= 96 else 0)


def write_ico(path: Path, sizes: list[int]) -> None:
    """ICO を大きい順に書く(各画像は PNG で格納。Windows Vista 以降・Tauri の ico クレートとも読める)。"""
    import io
    import struct

    entries = []
    for s in sorted(sizes, reverse=True):
        buf = io.BytesIO()
        icon(s).save(buf, format="PNG")
        entries.append((s, buf.getvalue()))
    header = struct.pack("<HHH", 0, 1, len(entries))
    offset = 6 + 16 * len(entries)
    dirs = b""
    for s, data in entries:
        dim = 0 if s >= 256 else s
        dirs += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    path.write_bytes(header + dirs + b"".join(d for _, d in entries))


def main() -> None:
    write_ico(ROOT / "app/src-tauri/icons/icon.ico", ICO_SIZES)
    # タスクバー用(RGBA の生データ。幅 = 高さ = サイズ)
    taskbar = ROOT / "app/src-tauri/icons/taskbar"
    taskbar.mkdir(exist_ok=True)
    for s in TASKBAR_SIZES:
        (taskbar / f"taskbar-{s}.rgba").write_bytes(icon(s).tobytes())
    icon(512).save(ROOT / "app/src-tauri/icons/icon.png")
    # アプリのヘッダー(白フチの全身版を 28px で表示。3 倍の解像度)
    shrink(squared(FULL, 1 / 16), 84, 40).save(ROOT / "app/public/glaux-icon.png")
    # 開発時の favicon(ブラウザのタブ)
    icon(32).save(ROOT / "app/public/favicon.png")
    # Godot: エディタのノードのアイコン(16px)、デモのプロジェクトのアイコン
    icon(16).save(ROOT / "godot/demo/addons/glaux/glaux_player.png")
    icon(128).save(ROOT / "godot/demo/icon.png")
    print("ICO のサイズ:", ICO_SIZES)


if __name__ == "__main__":
    main()
