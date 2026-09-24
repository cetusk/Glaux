"""assets/ のブランドキットから、アプリと Godot アドオンのアイコンと README のロゴを作る。

使い方(Pillow が要る):  python scripts/make_icons.py

- 余白はできるだけ削る(ユーザーの指示。ブランドガイドの推奨余白より優先): 見える図形の端で切り、
  正方形にするのに要る分(図形の短い辺の側)だけ透明を足す
- 40px 以下は顔のみ版(assets/icons/favicon.png)。小さくても目元が分かるよう、
  2 段階で縮めてから小さい半径で鮮明化する(1 回で縮めるとぼやける)
- 48px 以上は白フチの全身版(assets/icons/glaux-app-icon.png)
- README のロゴ: 白背景の横組み(assets/logos/glaux-lockup-white.png)を、文字とフクロウの端で切って幅 960px に
  (README の表示幅 480px の 2 倍)
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
# これ以下の不透明度は「見えない」とみなす。元の PNG には背景を抜いたときのほぼ透明なもや(不透明度 1〜8)が
# 広く残っていて、それを図形に含めると余白が大きく残る
HAZE_ALPHA = 8


def squared(path: Path, pad_ratio: float = 0.0) -> Image.Image:
    """見える図形を基準に正方形へ切る(周囲に図形の大きさ × pad_ratio の余白。既定は 0 = 余白なし)。"""
    im = Image.open(path).convert("RGBA")
    # ほぼ透明なもやを消してから、見える図形の範囲を求める
    alpha = im.getchannel("A").point(lambda a: a if a > HAZE_ALPHA else 0)
    im.putalpha(alpha)
    x0, y0, x1, y1 = alpha.getbbox()
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
        return shrink(squared(FACE), size, 120)
    return shrink(squared(FULL), size, 60 if size <= 96 else 0)


def readme_logo() -> Image.Image:
    """白背景の横組みロゴを、文字とフクロウの端(白でない画素の範囲)で切り、幅 960px にする。"""
    im = Image.open(ROOT / "assets/logos/glaux-lockup-white.png").convert("RGB")
    # 白(アンチエイリアスのわずかな色を含む)以外の画素の範囲
    mask = im.convert("L").point(lambda v: 255 if v < 250 else 0)
    box = mask.getbbox()
    cropped = im.crop(box)
    w = 960
    h = round(cropped.height * w / cropped.width)
    return cropped.resize((w, h), Image.LANCZOS)


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
    shrink(squared(FULL), 84, 40).save(ROOT / "app/public/glaux-icon.png")
    # README のロゴ
    (ROOT / "docs/images").mkdir(exist_ok=True)
    readme_logo().save(ROOT / "docs/images/glaux-logo.png", optimize=True)
    # 開発時の favicon(ブラウザのタブ)
    icon(32).save(ROOT / "app/public/favicon.png")
    # Godot: エディタのノードのアイコン(16px)、デモのプロジェクトのアイコン
    icon(16).save(ROOT / "godot/demo/addons/glaux/glaux_player.png")
    icon(128).save(ROOT / "godot/demo/icon.png")
    print("ICO のサイズ:", ICO_SIZES)


if __name__ == "__main__":
    main()
