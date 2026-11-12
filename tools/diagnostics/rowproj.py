"""行投影:量出每行文字的像素区间,用来判定"输入的字"和"提示符"是否同一行。

只统计亮度 > 110 的像素(文字),按 y 输出连续区间(一个区间 ≈ 一行文字)。
"""
import sys
from PIL import Image

for path in sys.argv[1:]:
    im = Image.open(path).convert("RGB")
    w, h = im.size
    px = im.load()
    rows = []
    for y in range(h):
        n = 0
        for x in range(0, w, 2):
            r, g, b = px[x, y]
            if max(r, g, b) > 110:
                n += 1
        rows.append(n)

    print(f"== {path}  {w}x{h}")
    bands = []
    start = None
    for y, n in enumerate(rows):
        if n >= 3 and start is None:
            start = y
        elif n < 3 and start is not None:
            bands.append((start, y - 1))
            start = None
    if start is not None:
        bands.append((start, h - 1))
    for i, (a, b) in enumerate(bands[-8:]):
        total = sum(rows[a:b + 1])
        print(f"  band#{len(bands)-8+i if len(bands)>=8 else i}: y={a}..{b} px={total}")
