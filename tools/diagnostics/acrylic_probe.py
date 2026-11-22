"""亚克力(毛玻璃)可行性实验。

背景:docs/custom-background.md 说 DWM 的 Mica/Acrylic 透不出来,因为交换链
拿不到逐像素透明。但还有另一条路 —— `SetWindowCompositionAttribute` 的
ACCENT_ENABLE_ACRYLICBLURBEHIND(win10 1803+ 的老牌做法),它对**层叠窗口**
(LWA_ALPHA 半透明)是否生效,本项目没验证过。

做法:在终端窗口背后铺一块高频彩色噪点(tkinter 画),窗口设 opacity,然后
屏幕截图对比四个状态:

  a_opaque      opacity 忽略(纯色背景)—— 对照组,背后什么都看不见
  b_alpha       只有 LWA_ALPHA       —— 背后噪点应该"清晰"地透出来
  c_acrylic     alpha + 亚克力 accent —— 如果生效,噪点会变成糊成一片的色块
  d_blurbehind  alpha + 经典 BlurBehind —— 老式 Win10 方案

用法:python tools/diagnostics/acrylic_probe.py
产物:target/_acrylic/*.png
"""
import ctypes
import ctypes.wintypes
import os
import pathlib
import subprocess
import sys
import threading
import time

from PIL import Image

ROOT = pathlib.Path(__file__).resolve().parents[2]
EXE = ROOT / "target" / "release" / "VectorTerm.exe"
OUT = ROOT / "target" / "_acrylic"

try:
    ctypes.WinDLL("shcore").SetProcessDpiAwareness(2)
except Exception:
    try:
        ctypes.WinDLL("user32").SetProcessDPIAware()
    except Exception:
        pass

user32 = ctypes.WinDLL("user32", use_last_error=True)
gdi32 = ctypes.WinDLL("gdi32", use_last_error=True)
k32 = ctypes.WinDLL("kernel32", use_last_error=True)

SRCCOPY = 0x00CC0020
CAPTUREBLT = 0x40000000


# ---------------- 屏幕截图 ----------------

class BITMAPINFOHEADER(ctypes.Structure):
    _fields_ = [("biSize", ctypes.wintypes.DWORD), ("biWidth", ctypes.wintypes.LONG),
                ("biHeight", ctypes.wintypes.LONG), ("biPlanes", ctypes.wintypes.WORD),
                ("biBitCount", ctypes.wintypes.WORD), ("biCompression", ctypes.wintypes.DWORD),
                ("biSizeImage", ctypes.wintypes.DWORD),
                ("biXPelsPerMeter", ctypes.wintypes.LONG),
                ("biYPelsPerMeter", ctypes.wintypes.LONG),
                ("biClrUsed", ctypes.wintypes.DWORD), ("biClrImportant", ctypes.wintypes.DWORD)]


class BITMAPINFO(ctypes.Structure):
    _fields_ = [("bmiHeader", BITMAPINFOHEADER), ("bmiColors", ctypes.wintypes.DWORD * 3)]


def grab(x, y, w, h):
    hdc = user32.GetDC(0)
    mem = gdi32.CreateCompatibleDC(hdc)
    bmp = gdi32.CreateCompatibleBitmap(hdc, w, h)
    gdi32.SelectObject(mem, bmp)
    gdi32.BitBlt(mem, 0, 0, w, h, hdc, x, y, SRCCOPY | CAPTUREBLT)
    bi = BITMAPINFO()
    bi.bmiHeader.biSize = ctypes.sizeof(BITMAPINFOHEADER)
    bi.bmiHeader.biWidth = w
    bi.bmiHeader.biHeight = -h          # 负数 = 自上而下
    bi.bmiHeader.biPlanes = 1
    bi.bmiHeader.biBitCount = 32
    bi.bmiHeader.biCompression = 0
    buf = ctypes.create_string_buffer(w * h * 4)
    gdi32.GetDIBits(mem, bmp, 0, h, buf, ctypes.byref(bi), 0)
    gdi32.DeleteObject(bmp)
    gdi32.DeleteDC(mem)
    user32.ReleaseDC(0, hdc)
    return Image.frombuffer("RGBA", (w, h), buf.raw, "raw", "BGRA", 0, 1).convert("RGB")


# ---------------- 窗口工具 ----------------

def window_of_pid(pid, title_hint=None):
    found = []

    @ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)
    def enum_proc(hwnd, _l):
        owner = ctypes.c_ulong()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid and user32.IsWindowVisible(hwnd):
            found.append(hwnd)
        return False

    user32.EnumWindows(enum_proc, 0)
    return found[0] if found else None


def move_window(hwnd, x, y, w, h):
    user32.SetWindowPos(hwnd, 0, x, y, w, h, 0x0004 | 0x0010)  # NOZORDER|NOACTIVATE


ACCENT_ENABLE_BLURBEHIND = 3
ACCENT_ENABLE_ACRYLICBLURBEHIND = 4
WCA_ACCENT_POLICY = 19


class ACCENTPOLICY(ctypes.Structure):
    _fields_ = [("AccentState", ctypes.c_int), ("AccentFlags", ctypes.c_int),
                ("GradientColor", ctypes.c_uint), ("AnimationId", ctypes.c_int)]


class WINCOMPATTRDATA(ctypes.Structure):
    _fields_ = [("Attribute", ctypes.c_int), ("Data", ctypes.c_void_p),
                ("SizeOfData", ctypes.c_size_t)]


def set_accent(hwnd, state, gradient=0x99_0B1220):
    """state: 3 = BlurBehind,4 = Acrylic。gradient 是 AABBGGRR。"""
    policy = ACCENTPOLICY(state, 0, gradient, 0)
    data = WINCOMPATTRDATA(WCA_ACCENT_POLICY, ctypes.cast(ctypes.byref(policy), ctypes.c_void_p),
                           ctypes.sizeof(policy))
    fn = user32.SetWindowCompositionAttribute
    return bool(fn(hwnd, ctypes.byref(data)))


# ---------------- 背后那块彩噪 ----------------

def noise_window(x, y, w, h, stop):
    import tkinter as tk
    root = tk.Tk()
    root.overrideredirect(True)
    root.geometry(f"{w}x{h}+{x}+{y}")
    c = tk.Canvas(root, width=w, height=h, highlightthickness=0)
    c.pack()
    colors = ["#ff2d55", "#ffcc00", "#34c759", "#0a84ff", "#bf5af2",
              "#ff9f0a", "#64d2ff", "#ff375f"]
    s = 12
    for j in range(0, h, s):
        for i in range(0, w, s):
            c.create_rectangle(i, j, i + s - 1, j + s - 1, width=0,
                               fill=colors[((i // s) * 7 + (j // s) * 3) % len(colors)])
    c.create_text(w // 2, h // 2, text="BEHIND", font=("Consolas", 48, "bold"), fill="white")
    root.after(50, lambda: stop.is_set() and root.destroy())
    root.mainloop()


CFG_ALPHA = """return {
  shell = "pwsh.exe",
  shell_args = { "-NoLogo", "-NoProfile", "-NoExit" },
  render = "gpu",
  font = { size = 14 },
  padding = 10,
  window_decorations = "RESIZE",
  window_width = 620,
  window_height = 380,
  window_background_color = "#0B1220",
  window_background_opacity = 0.62,
  tab_bar = { enable = false },
  session_log = "__LOG__",
}
"""

CFG_OPAQUE = CFG_ALPHA.replace("  window_background_opacity = 0.62,\n", "")


def launch(name, cfg_text):
    cfg = OUT / f"{name}.lua"
    cfg.write_text(cfg_text.replace("__LOG__", (OUT / f"{name}.bin").as_posix()),
                   encoding="utf-8")
    env = dict(os.environ)
    for k in list(env):
        if k.startswith("VECTORTERM_"):
            env.pop(k, None)
    proc = subprocess.Popen([str(EXE), "--config", str(cfg)], env=env,
                            stdout=subprocess.DEVNULL,
                            stderr=open(OUT / f"{name}.trace.log", "wb"))
    time.sleep(2.2)
    hwnd = window_of_pid(proc.pid)
    return proc, hwnd


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    nx, ny, nw, nh = 120, 120, 900, 620
    stop = threading.Event()
    t = threading.Thread(target=noise_window, args=(nx, ny, nw, nh, stop), daemon=True)
    t.start()
    time.sleep(1.2)

    wx, wy, ww, wh = 260, 220, 620, 380

    results = {}
    # a: 不透明对照组
    proc, hwnd = launch("a_opaque", CFG_OPAQUE)
    move_window(hwnd, wx, wy, ww, wh)
    time.sleep(0.9)
    grab(wx, wy, ww, wh).save(OUT / "a_opaque.png")
    proc.terminate(); proc.wait(timeout=8)

    # b: 只有 alpha
    proc, hwnd = launch("b_alpha", CFG_ALPHA)
    move_window(hwnd, wx, wy, ww, wh)
    time.sleep(0.9)
    grab(wx, wy, ww, wh).save(OUT / "b_alpha.png")

    # c: alpha + acrylic
    ok = set_accent(hwnd, ACCENT_ENABLE_ACRYLICBLURBEHIND)
    time.sleep(0.7)
    grab(wx, wy, ww, wh).save(OUT / "c_acrylic.png")
    results["acrylic 调用"] = ok

    # d: alpha + 经典 blurbehind
    ok2 = set_accent(hwnd, ACCENT_ENABLE_BLURBEHIND)
    time.sleep(0.7)
    grab(wx, wy, ww, wh).save(OUT / "d_blurbehind.png")
    results["blurbehind 调用"] = ok2
    proc.terminate(); proc.wait(timeout=8)

    stop.set()
    time.sleep(0.4)

    # 量化:背后的彩噪是 12px 一个色块。清晰透过 → 相邻像素差大;糊掉 → 差小。
    for n in ("a_opaque", "b_alpha", "c_acrylic", "d_blurbehind"):
        im = Image.open(OUT / f"{n}.png").convert("RGB")
        px = im.load()
        w, h = im.size
        # 取窗口中部一块,避开文字多的左上角
        x0, y0, x1, y1 = w // 3, h // 2, w // 3 + 200, h // 2 + 120
        tot = cnt = 0
        for y in range(y0, y1, 2):
            for x in range(x0, x1, 2):
                a, b = px[x, y], px[x + 2, y]
                tot += sum(abs(a[i] - b[i]) for i in range(3))
                cnt += 1
        distinct = len({px[x, y] for y in range(y0, y1, 4) for x in range(x0, x1, 4)})
        print(f"  {n:<12} 相邻差 {tot / cnt:6.1f}   颜色种数 {distinct:4d}", flush=True)

    for k, v in results.items():
        print(f"  {k}: {v}", flush=True)


if __name__ == "__main__":
    main()
