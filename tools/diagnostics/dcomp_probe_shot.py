"""验证 DirectComposition 合成交换链 + DWM Acrylic 材质。

背后起一个带彩色渐变背景的 VectorTerm 当参照物,前面盖上 _dcomp_probe 的
45% 不透明窗口,截屏看三件事:
  1. probe 的内容(深蓝)有没有真的上屏 —— 没有就是 NOREDIRECTIONBITMAP 没设
  2. 背后的粉色/蓝色能不能透进 probe 窗口(逐像素 alpha 生效)
  3. 透出来的是清晰的图还是被糊过(材质生效)

踩过的坑:
  - Python 不是 DPI aware 时 MoveWindow 的坐标会被系统缩放,窗口跑到别处
  - SetWindowPos 的 hWndInsertAfter 是 HWND(指针),64 位下传 `-1` 会被截断成
    0xFFFFFFFF,调用直接返回 0(失败);必须 `wintypes.HWND(-1)`
  - 全屏独占的游戏会把 GDI 抓屏整个顶掉,验证前先关掉
"""

import ctypes
import ctypes.wintypes as wt
import os
import subprocess
import time
from pathlib import Path

from PIL import Image, ImageGrab

ROOT = Path(r"F:\Code\VectorTerm")
OUT = ROOT / "target" / "_dcomp"
OUT.mkdir(parents=True, exist_ok=True)

ctypes.windll.shcore.SetProcessDpiAwareness(2)
user32 = ctypes.windll.user32
user32.SetWindowPos.argtypes = [
    wt.HWND, wt.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_uint
]
user32.MoveWindow.argtypes = [wt.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wt.BOOL]

BACK_W, BACK_H = 1250, 750
BACK_X, BACK_Y = 200, 130


def gradient(path: Path, w: int, h: int) -> None:
    im = Image.new("RGB", (w, h))
    px = im.load()
    for y in range(h):
        for x in range(w):
            u = x / max(1, w - 1)
            v = y / max(1, h - 1)
            base = (
                int(40 + 200 * u),
                int(60 + 80 * (1 - u) * (1 - v)),
                int(200 - 120 * u + 40 * v),
            )
            # 高频棋盘:8px 一格,模糊后应该变成一片平均色
            if ((x // 8) + (y // 8)) % 2 == 0:
                base = tuple(min(255, c + 70) for c in base)
            px[x, y] = base
    im.save(path)


def find_window(title: str, timeout: float = 12.0):
    end = time.time() + timeout
    while time.time() < end:
        hwnd = user32.FindWindowW(None, title)
        if hwnd:
            return hwnd
        time.sleep(0.2)
    return None


def main():
    for name in ("VectorTerm.exe", "_dcomp_probe.exe"):
        subprocess.run(["taskkill", "/F", "/IM", name],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    img = OUT / "bg.png"
    gradient(img, BACK_W, BACK_H)
    back_cfg = OUT / "back.lua"
    back_cfg.write_text(
        f'''return {{
  shell = "cmd.exe",
  shell_args = {{ "/c", "timeout /t 60 > nul" }},
  render = "cpu",
  padding = 0,
  window_decorations = "NONE",
  tab_bar = {{ enable = false }},
  window_width = {BACK_W},
  window_height = {BACK_H},
  window_background_image = "{img.as_posix()}",
  window_background_fill = "cover",
  colors = {{ background = "#000000", foreground = "#000000" }},
}}
''',
        encoding="utf-8",
    )

    env = dict(os.environ)
    for k in list(env):
        if k.startswith("VECTORTERM_"):
            env.pop(k, None)

    back = subprocess.Popen(
        [str(ROOT / "target" / "release" / "VectorTerm.exe"), "--config", str(back_cfg)],
        env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    bh = find_window("VectorTerm - cmd")
    if bh:
        user32.MoveWindow(bh, BACK_X, BACK_Y, BACK_W, BACK_H, True)
        user32.SetWindowPos(bh, wt.HWND(-1), 0, 0, 0, 0, 0x0001 | 0x0002)
    time.sleep(1.5)

    front = subprocess.Popen(
        [str(ROOT / "target" / "release" / "_dcomp_probe.exe")],
        env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    fh = find_window("dcomp probe", timeout=8.0)
    if not fh:
        print("没找到 probe 窗口")
        front.kill(); back.kill()
        return
    user32.MoveWindow(fh, BACK_X + 130, BACK_Y + 110, 1125, 700, True)
    user32.SetWindowPos(fh, wt.HWND(-1), 0, 0, 0, 0, 0x0001 | 0x0002)
    user32.SetForegroundWindow(fh)
    time.sleep(1.5)

    box = (BACK_X, BACK_Y, BACK_X + BACK_W, BACK_Y + BACK_H)
    shot = ImageGrab.grab(bbox=box, all_screens=True)
    shot.save(OUT / "shot.png")
    print("已截图 ->", OUT / "shot.png", shot.size)

    try:
        out = front.communicate(timeout=12)[0]
        print(out.strip()[:1200])
    except subprocess.TimeoutExpired:
        front.kill()
    back.kill()


if __name__ == "__main__":
    main()
