"""验证渲染层毛玻璃(逐像素 alpha + DWM Acrylic)在真终端里的效果。

背景放一块高频棋盘 + 渐变(模糊后应该糊成一片),前面盖一个
`theme = "deepseek-glass"` 的 VectorTerm,配置成:
  win32_system_backdrop = "Acrylic"
  window_background_opacity = 0.30   <- 只压背景层,文字应该还是实的
  render = "gpu"                     <- 走 DirectComposition 合成交换链

判据:
  1. 窗口内能看到背后的颜色(逐像素 alpha 生效)
  2. 棋盘格在窗口内糊掉(材质生效)
  3. 文字边缘锐利、不发灰(前景 alpha = 1,不是整窗变淡)
"""

import ctypes
import ctypes.wintypes as wt
import os
import subprocess
import time
from pathlib import Path

from PIL import Image, ImageGrab

ROOT = Path(r"F:\Code\VectorTerm")
OUT = ROOT / "target" / "_glass_render"
OUT.mkdir(parents=True, exist_ok=True)

ctypes.windll.shcore.SetProcessDpiAwareness(2)
user32 = ctypes.windll.user32
user32.SetWindowPos.argtypes = [
    wt.HWND, wt.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_uint
]
user32.MoveWindow.argtypes = [wt.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wt.BOOL]

W, H = 1250, 750
X, Y = 240, 180
M = 110  # 背景窗口比前台窗口大一圈,用来露出"清晰棋盘格"的对照区


def checker(path: Path, w: int, h: int, cell: int = 10) -> None:
    im = Image.new("RGB", (w, h))
    px = im.load()
    for y in range(h):
        for x in range(w):
            u = x / max(1, w - 1)
            v = y / max(1, h - 1)
            base = (
                int(60 + 170 * u),
                int(70 + 70 * (1 - u) * (1 - v)),
                int(190 - 110 * u + 40 * v),
            )
            if ((x // cell) + (y // cell)) % 2 == 0:
                base = tuple(min(255, c + 60) for c in base)
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
    checker(img, W + 2 * M, H + 2 * M)

    back_cfg = OUT / "back.lua"
    back_cfg.write_text(
        f'''return {{
  shell = "cmd.exe",
  shell_args = {{ "/c", "timeout /t 90 > nul" }},
  render = "cpu",
  padding = 0,
  window_decorations = "NONE",
  tab_bar = {{ enable = false }},
  window_width = {W + 2 * M},
  window_height = {H + 2 * M},
  window_background_image = "{img.as_posix()}",
  window_background_fill = "cover",
  colors = {{ background = "#000000", foreground = "#000000" }},
}}
''',
        encoding="utf-8",
    )

    front_cfg = OUT / "front.lua"
    front_cfg.write_text(
        f'''return {{
  theme = "deepseek-glass",
  render = "gpu",
  win32_system_backdrop = "Acrylic",
  window_background_opacity = 0.30,
  shell = "pwsh.exe",
  shell_args = {{ "-NoLogo", "-NoProfile", "-NoExit", "-Command",
                 "Get-ChildItem | Select-Object -First 12 | Format-Table -AutoSize" }},
  window_width = {W},
  window_height = {H},
  session_log = "{(OUT / "front.bin").as_posix()}",
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
        user32.MoveWindow(bh, X - M, Y - M, W + 2 * M, H + 2 * M, True)
        user32.SetWindowPos(bh, wt.HWND(-1), 0, 0, 0, 0, 0x0001 | 0x0002)
    time.sleep(1.5)

    log = open(OUT / "front.log", "w", encoding="utf-8")
    front = subprocess.Popen(
        [str(ROOT / "target" / "release" / "VectorTerm.exe"), "--config", str(front_cfg)],
        env=env, stdout=log, stderr=subprocess.STDOUT)
    fh = find_window("VectorTerm - pwsh", timeout=10.0) or find_window("VectorTerm", timeout=2.0)
    if not fh:
        print("没找到前台窗口")
        front.kill(); back.kill()
        return
    user32.MoveWindow(fh, X, Y, W, H, True)
    user32.SetWindowPos(fh, wt.HWND(-1), 0, 0, 0, 0, 0x0001 | 0x0002)
    user32.SetForegroundWindow(fh)
    time.sleep(2.2)

    # 取景比前台窗口大一圈:窗口外是清晰的棋盘格,窗口内应该糊成一片,
    # 这样才能排除"颜色本来就长这样"的假阳性。
    ImageGrab.grab(
        bbox=(X - M, Y - M, X + W + M, Y + H + M), all_screens=True
    ).save(OUT / "shot.png")
    print("已截图 ->", OUT / "shot.png")

    front.kill(); back.kill()
    log.close()
    print((OUT / "front.log").read_text(encoding="utf-8", errors="replace")[:2000])


if __name__ == "__main__":
    main()
