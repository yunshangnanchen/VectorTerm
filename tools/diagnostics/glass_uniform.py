"""在纯白背景上量"窗口各区域的实际不透明度",找渲染不一致的横带。

背景 = 另一个 VectorTerm 实例,铺一张纯白图(比 tkinter 稳,环境里没装 tk);
前台 = 内容空白的终端(cmd /c timeout,屏幕上没有文字)。

亮度换算:纯白(255)在 alpha=a、底色=clear 时 -> v = clear*a + 255*(1-a),
所以"越亮 = 越透"。同一列上若出现台阶,就是 alpha 不一致的横带。

用法: python tools/diagnostics/glass_uniform.py [配置文件]
"""
import ctypes
import ctypes.wintypes as wt
import os
import subprocess
import sys
import time
from pathlib import Path

from PIL import Image, ImageGrab, ImageStat

ROOT = Path(r"F:\Code\VectorTerm")
OUT = ROOT / "target" / "_glass_render"
OUT.mkdir(parents=True, exist_ok=True)
EXE = str(ROOT / "target" / "release" / "VectorTerm.exe")

ctypes.windll.shcore.SetProcessDpiAwareness(2)
u = ctypes.windll.user32
u.SetWindowPos.argtypes = [
    wt.HWND, wt.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_uint
]
u.MoveWindow.argtypes = [wt.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wt.BOOL]

W, H, X, Y = 1250, 750, 500, 350
M = 80  # 背景比前台大一圈

cfg = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "config" / "deepseek-glass.lua"


def find(title, timeout=15.0):
    end = time.time() + timeout
    while time.time() < end:
        h = u.FindWindowW(None, title)
        if h:
            return h
        time.sleep(0.2)
    return 0


for name in ("VectorTerm.exe",):
    subprocess.run(["taskkill", "/F", "/IM", name],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

env = dict(os.environ)
for k in list(env):
    if k.startswith("VECTORTERM_"):
        env.pop(k, None)

# 纯白背板
white = OUT / "white.png"
Image.new("RGB", (64, 64), (255, 255, 255)).save(white)
back_cfg = OUT / "uniform_back.lua"
back_cfg.write_text(f'''return {{
  shell = "cmd.exe",
  shell_args = {{ "/c", "timeout /t 90 > nul" }},
  render = "cpu",
  window_decorations = "NONE",
  padding = 0,
  tab_bar = {{ enable = false }},
  window_width = {W + 2 * M}, window_height = {H + 2 * M},
  window_background_image = "{white.as_posix()}",
  window_background_fill = "stretch",
  colors = {{ background = "#FFFFFF", foreground = "#FFFFFF" }},
}}
''', encoding="utf-8")

# 内容空白的终端:屏幕上只有光标
inner = OUT / "uniform_front.lua"
inner.write_text(f'''local cfg = dofile([[{cfg.as_posix()}]])
cfg.render = "gpu"
cfg.window_decorations = "NONE"
cfg.window_width = {W}
cfg.window_height = {H}
cfg.shell = "cmd.exe"
cfg.shell_args = {{ "/c", "timeout /t 60 > nul" }}
return cfg
''', encoding="utf-8")

back = subprocess.Popen([EXE, "--config", str(back_cfg)], env=env,
                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
bh = find("VectorTerm - cmd")
if bh:
    u.MoveWindow(bh, X - M, Y - M, W + 2 * M, H + 2 * M, True)
    u.SetWindowPos(bh, wt.HWND(-1), 0, 0, 0, 0, 1 | 2)
time.sleep(1.5)

log = open(OUT / "uniform.log", "w", encoding="utf-8")
front = subprocess.Popen([EXE, "--config", str(inner)], env=env,
                         stdout=log, stderr=subprocess.STDOUT)

hits = []


@ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
def cb(h, l):
    n = u.GetWindowTextLengthW(h)
    if n and u.IsWindowVisible(h):
        b = ctypes.create_unicode_buffer(n + 1)
        u.GetWindowTextW(h, b, n + 1)
        if b.value.startswith("VectorTerm - cmd"):
            hits.append(h)
    return True


# 毛玻璃窗口要等后端就绪才显示(2~3 秒),这里等到出现第二个窗口
fh = 0
for _ in range(100):
    hits.clear()
    u.EnumWindows(cb, 0)
    others = [h for h in hits if h != bh]
    if others:
        fh = others[-1]
        break
    time.sleep(0.2)
if not fh:
    print("前台窗口没起来")
    front.kill()
    back.kill()
    sys.exit(1)
u.SetWindowPos(fh, wt.HWND(-1), X, Y, W, H, 0x0040 | 0x0010)
u.SetForegroundWindow(fh)
time.sleep(2.2)

shot = ImageGrab.grab(bbox=(X, Y, X + W, Y + H), all_screens=True)
shot.save(OUT / "uniform.png")
front.kill()
back.kill()
log.close()

im = shot.convert("RGB")
print("  y    全宽  左padding  x600-900  x1100-1200   (越亮=越透/alpha 越小)")
prev = None
for y in range(0, H, 6):
    def row(a, b):
        return sum(ImageStat.Stat(im.crop((a, y, b, min(y + 6, H)))).mean) / 3
    full, lp, mid, right = row(0, W), row(0, 8), row(600, 900), row(1100, 1200)
    mark = ""
    if prev is not None and abs(mid - prev) > 3.0:
        mark = f"  <-- 台阶 {mid - prev:+.1f}"
    print(f"{y:4d} {full:7.1f} {lp:9.1f} {mid:9.1f} {right:10.1f}{mark}")
    prev = mid
