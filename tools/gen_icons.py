# -*- coding: utf-8 -*-
"""从根目录母版 `VectorTermLogo.png` 生成各位置所需分辨率的图标。

    python tools/gen_icons.py

产出(都在 assets/icons/):

    VectorTerm-16/20/24/32/40/48/64/96/128/256/512.png   各档单张
    VectorTerm.ico                                       exe 图标(16~256 多尺寸)

位置对应:

    窗口图标(任务栏 / Alt+Tab / 标题栏)   VectorTerm-256.png —— winit 只收一张 RGBA,
                                          256 足够覆盖 Windows 会用的最大尺寸
    exe 图标(资源管理器 / 快捷方式)       VectorTerm.ico  —— 多尺寸,系统按 DPI 取最合适的一档
    README 头图                           VectorTerm-256.png(按 128 宽显示,2x 屏不糊)
    文档 / 商店页 / 高 DPI               其余档位按需取

母版是唯一要手改的文件,改完跑一次本脚本;不要直接编辑 assets/icons/ 下的产物。
"""
from __future__ import annotations

import io
import struct
import sys
from pathlib import Path

from PIL import Image, ImageFilter

ROOT = Path(__file__).resolve().parent.parent
MASTER = ROOT / "VectorTermLogo.png"
OUT = ROOT / "assets" / "icons"

# 小尺寸需要单独锐化,否则白色线稿缩下去就糊成一团灰
ICO_SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]
PNG_SIZES = ICO_SIZES + [512]
SHARPEN_AT_OR_BELOW = 48


def square(img: Image.Image) -> Image.Image:
    """母版不是正方形时补透明边,图标 API 只吃方图。"""
    w, h = img.size
    if w == h:
        return img
    side = max(w, h)
    canvas = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    canvas.paste(img, ((side - w) // 2, (side - h) // 2), img)
    return canvas


def render(master: Image.Image, size: int) -> Image.Image:
    small = master.resize((size, size), Image.LANCZOS)
    if size <= SHARPEN_AT_OR_BELOW:
        # 深底白线稿:缩完边缘会发灰,轻微反锐化把线拉回来
        small = small.filter(ImageFilter.UnsharpMask(radius=0.6, percent=120, threshold=0))
    return small


def write_ico(path: Path, frames: list[tuple[int, Image.Image]]) -> None:
    """手写 ICO 容器;每帧一张 PNG(Vista 起全支持),这样每档能单独调。"""
    blobs: list[tuple[int, bytes]] = []
    for size, im in frames:
        buf = io.BytesIO()
        im.save(buf, format="PNG", optimize=True)
        blobs.append((size, buf.getvalue()))

    out = bytearray()
    out += struct.pack("<HHH", 0, 1, len(blobs))  # reserved / type=icon / count
    offset = 6 + 16 * len(blobs)
    for size, blob in blobs:
        out += struct.pack(
            "<BBBBHHII",
            size % 256, size % 256,  # 宽高,256 要写成 0
            0, 0,                    # 调色板数 / 保留
            1, 32,                   # planes / 位深
            len(blob), offset,
        )
        offset += len(blob)
    for _, blob in blobs:
        out += blob
    path.write_bytes(bytes(out))


def main() -> int:
    if not MASTER.is_file():
        print(f"找不到母版: {MASTER}")
        return 1
    master = square(Image.open(MASTER).convert("RGBA"))
    OUT.mkdir(parents=True, exist_ok=True)
    print(f"母版 {MASTER.name} {Image.open(MASTER).size} → 正方形 {master.size}")

    frames = []
    for size in PNG_SIZES:
        im = render(master, size)
        p = OUT / f"vectorterm-{size}.png"
        im.save(p, optimize=True)
        frames.append((size, im))
        print(f"  {p.relative_to(ROOT)!s:34s} {p.stat().st_size:>7,} bytes")

    ico = OUT / "VectorTerm.ico"
    write_ico(ico, [(s, im) for s, im in frames if s in ICO_SIZES])
    print(f"  {ico.relative_to(ROOT)!s:34s} {ico.stat().st_size:>7,} bytes "
          f"({len(ICO_SIZES)} 档)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
