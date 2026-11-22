"""毛玻璃的数值判据(不靠肉眼)。

背景窗口 = 高频棋盘 + 渐变,前台窗口 = 终端,两者同位置、前台小一圈,
四周露出 M 像素的"原始棋盘"作为参照。

判据:
  1. alpha 是否透出来:窗口内空白块的平均色 应当明显偏向前台自身底色之外,
     即与窗口外背景的色相有相关性;最严格的做法是 opacity=0.0 时窗口内
     应当几乎等于背景模糊图。
  2. 是否被模糊:窗口内一小块区域的梯度能量(相邻像素差的平方和)应当
     远小于窗口外同一尺度的区域。棋盘格模糊后会接近常数,梯度骤降。

用法: python tools/diagnostics/glass_metric.py <tag> <backdrop:none|acrylic|mica> <opacity>
"""

import ctypes
import ctypes.wintypes as wt
import os
import subprocess
import sys
import time
from pathlib import Path

from PIL import Image, ImageGrab, ImageChops, ImageStat

ROOT = Path(r"F:\Code\VectorTerm")
OUT = ROOT / "target" / "_glass_render"
OUT.mkdir(parents=True, exist_ok=True)

ctypes.windll.shcore.SetProcessDpiAwareness(2)
u = ctypes.windll.user32
u.SetWindowPos.argtypes = [
    wt.HWND, wt.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_uint
]
u.MoveWindow.argtypes = [wt.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wt.BOOL]

W, H = 1250, 750
POS = [(260, 200), (40, 40), (1000, 560)]
X, Y = POS[0]
M = 110  # 四周露出的背景宽度
EXE = str(ROOT / "target" / "release" / "VectorTerm.exe")


def checker(path: Path, w: int, h: int, cell: int = 10) -> None:
    im = Image.new("RGB", (w, h))
    px = im.load()
    for y in range(h):
        v = y / max(1, h - 1)
        for x in range(w):
            uu = x / max(1, w - 1)
            c = (int(60 + 170 * uu), int(70 + 70 * (1 - uu) * (1 - v)), int(190 - 110 * uu + 40 * v))
            if ((x // cell) + (y // cell)) % 2 == 0:
                c = tuple(min(255, k + 60) for k in c)
            px[x, y] = c
    im.save(path)


def find(title, timeout=12.0):
    end = time.time() + timeout
    while time.time() < end:
        h = u.FindWindowW(None, title)
        if h:
            return h
        time.sleep(0.2)
    return 0


def grad_energy(im: Image.Image) -> float:
    """相邻像素差平方的均值,棋盘格很大、糊掉接近 0。"""
    g = im.convert("L")
    d = ImageChops.difference(g.crop((1, 0, g.width, g.height)), g.crop((0, 0, g.width - 1, g.height)))
    d2 = ImageChops.difference(g.crop((0, 1, g.width, g.height)), g.crop((0, 0, g.width, g.height - 1)))
    e1 = ImageStat.Stat(d).rms[0]
    e2 = ImageStat.Stat(d2).rms[0]
    return e1 * e1 + e2 * e2


def mean(im: Image.Image):
    return tuple(round(v, 1) for v in ImageStat.Stat(im.convert("RGB")).mean)


def main(tag: str, backdrop: str, opacity: float, pos: int = 0, focus: str = "win"):
    global X, Y
    X, Y = POS[pos]
    tag = f"{tag}_p{pos}"
    for name in ("VectorTerm.exe",):
        subprocess.run(["taskkill", "/F", "/IM", name],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    bgp = OUT / "bg.png"
    checker(bgp, W + 2 * M, H + 2 * M)

    back_cfg = OUT / "back.lua"
    back_cfg.write_text(f'''return {{
  shell = "cmd.exe",
  shell_args = {{ "/c", "timeout /t 90 > nul" }},
  render = "cpu",
  padding = 0,
  window_decorations = "NONE",
  tab_bar = {{ enable = false }},
  window_width = {W + 2 * M}, window_height = {H + 2 * M},
  window_background_image = "{bgp.as_posix()}",
  window_background_fill = "cover",
  colors = {{ background = "#000000", foreground = "#000000" }},
}}
''', encoding="utf-8")

    bd = f'win32_system_backdrop = "{backdrop}",' if backdrop != "none" else ""
    front_cfg = OUT / f"{tag}.lua"
    front_cfg.write_text(f'''return {{
  render = "gpu",
  window_decorations = "NONE",
  padding = 8,
  {bd}
  window_background_opacity = {opacity},
  colors = {{ background = "#0B1220", foreground = "#E6EDF7" }},
  shell = "pwsh.exe",
  shell_args = {{ "-NoLogo", "-NoProfile", "-NoExit", "-Command",
                 "1..8 | ForEach-Object {{ 'line $_ glass test' }}" }},
  window_width = {W}, window_height = {H},
  tab_bar = {{ enable = false }},
}}
''', encoding="utf-8")

    env = dict(os.environ)
    for k in list(env):
        if k.startswith("VECTORTERM_"):
            env.pop(k, None)

    back = subprocess.Popen([EXE, "--config", str(back_cfg)], env=env,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    bh = find("VectorTerm - cmd")
    if bh:
        u.MoveWindow(bh, X - M, Y - M, W + 2 * M, H + 2 * M, True)
        u.SetWindowPos(bh, wt.HWND(-1), 0, 0, 0, 0, 1 | 2)
    time.sleep(1.4)

    log = open(OUT / f"{tag}.log", "w", encoding="utf-8")
    front = subprocess.Popen([EXE, "--config", str(front_cfg)], env=env,
                             stdout=log, stderr=subprocess.STDOUT)
    fh = find("VectorTerm - pwsh") or find("VectorTerm")
    if not fh:
        print("没找到前台窗口")
        front.kill()
        back.kill()
        return
    u.MoveWindow(fh, X, Y, W, H, True)
    u.SetWindowPos(fh, wt.HWND(-1), 0, 0, 0, 0, 1 | 2)
    time.sleep(2.0)

    # 焦点控制:SetForegroundWindow 常被系统拒绝,直接用真实点击。
    if focus == "win":
        click(X + W // 2, Y + H - 40)
    elif focus == "back":
        # 点背景窗口露出前窗之外的那一条
        click(X - M // 2, Y + H // 2)
    time.sleep(1.8)

    shot = ImageGrab.grab(bbox=(X - M, Y - M, X + W + M, Y + H + M), all_screens=True)
    shot.save(OUT / f"{tag}.png")
    fg = u.GetForegroundWindow()
    front.kill()
    back.kill()
    log.close()

    # 取景坐标换算:窗口左上角在截图里的位置 = (M, M)
    # 外部参照:窗口左侧那条背景带
    outside = shot.crop((4, M + 60, M - 6, H + M))
    # 内部参照:避开文字与标签栏,取右下方空白区
    inside = shot.crop((M + 700, M + 620, M + 1100, M + 700))
    print(f"== {tag} backdrop={backdrop} opacity={opacity} focus={focus}"
          f"{' (前台=前窗)' if fg == fh else ' (前台=别的窗口)'}")
    print(f"   outside grad_energy = {grad_energy(outside):8.1f}  mean = {mean(outside)}")
    print(f"   inside  grad_energy = {grad_energy(inside):8.1f}  mean = {mean(inside)}")
    print(f"   -> 截图 {OUT / (tag + '.png')}")


def click(px: int, py: int):
    u.SetCursorPos(px, py)
    time.sleep(0.15)
    u.mouse_event(0x0002, 0, 0, 0, 0)  # LEFTDOWN
    time.sleep(0.05)
    u.mouse_event(0x0004, 0, 0, 0, 0)  # LEFTUP


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2], float(sys.argv[3]),
         pos=int(sys.argv[4]) if len(sys.argv) > 4 else 0,
         focus=sys.argv[5] if len(sys.argv) > 5 else "win")