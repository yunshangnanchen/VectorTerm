"""第 N 行到底有没有跟着输出滚动?用一个"数行数"的实验判定。

思路:让 shell 先把 60 行编号打出来,再把它自己看到的 PTY 视口行数打出来;
截图上从最上面那个编号就知道"画面上实际装了几行"。

  画面行数 == PTY 行数  → 网格和 PTY 一致,光标确实在最后一行(不是没更新)
  画面行数 <  PTY 行数  → 底下真的空出了若干整行 = 行数没同步

同时量"最后一行文字下沿 → 客户区底边"的距离,和 padding + 行距余量对一下。

用法: python tools/diagnostics/bottom_rows_probe.py
"""
import sys
import pathlib

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import bottom_gap_probe as g  # noqa: E402

# 与 deepseek.lua 一致的几何:1250x750 / font 14 / padding 10 / 顶部标签栏
CFG = """return {
  shell = "pwsh.exe",
  shell_args = { "-NoLogo", "-NoProfile", "-NoExit", "-Command",
                 "1..60 | ForEach-Object { '{0:D3}' -f $_ }; $w=$Host.UI.RawUI.WindowSize; 'ROWS=' + $w.Height + ' COLS=' + $w.Width" },
  render = "cpu",
  font = { size = 14, line_height = 1, cell_width = 1 },
  padding = 10,
  window_decorations = "RESIZE",
  window_rounded_corners = true,
  window_width = 1250,
  window_height = 750,
  tab_bar = {
    enable = true, hide_if_one = false, at_bottom = false,
    max_width = 20, show_close_button = true, corner_radius = 6,
    fancy = true, show_new_tab_button = true,
    top_padding = 5, edge_padding = 5,
  },
  status_bar = { item_spacing = 0, font_scale = 0.9, space_width = 0.2 },
  session_log = "__LOG__",
}
"""


def row_bands(png, bg_probe=True):
    """把有墨的连续行段切出来(每段 = 一行文字)。"""
    from PIL import Image
    im = Image.open(png).convert("RGB")
    w, h = im.size
    px = im.load()
    tally = {}
    for y in range(h - 12, h - 2):
        for x in range(6, 26):
            c = px[x, y]
            tally[c] = tally.get(c, 0) + 1
    bg = max(tally, key=tally.get)

    def ink(y):
        return sum(1 for x in range(30, max(31, w - 40))
                   if sum(abs(px[x, y][i] - bg[i]) for i in range(3)) > 60)

    bands, start = [], None
    for y in range(h):
        if ink(y) > 2 and start is None:
            start = y
        elif ink(y) <= 2 and start is not None:
            bands.append((start, y - 1))
            start = None
    if start is not None:
        bands.append((start, h - 1))
    return im.size, bands


def main():
    m = g.run("rows_filled", CFG, "", None, 4200)
    if not m:
        sys.exit("没导出帧")

    size, bands = row_bands(g.OUT / "rows_filled.png")
    tops = [b[0] for b in bands]
    steps = [b - a for a, b in zip(tops, tops[1:])]
    step = max(set(steps), key=steps.count) if steps else None

    print(f"    客户区 {size[0]}x{size[1]}  文字行段 {len(bands)}  行距 {step}px")
    print(f"    首段 y={bands[0][0]}  末段 y={bands[-1][0]}..{bands[-1][1]}  "
          f"下边距 {size[1] - 1 - bands[-1][1]}px")
    print(f"    行距余量: {size[1]} - {bands[0][0]} - {len(bands)}*{step} = "
          f"{size[1] - bands[0][0] - len(bands) * step}px")


if __name__ == "__main__":
    main()
