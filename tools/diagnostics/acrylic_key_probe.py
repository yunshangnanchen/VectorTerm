"""亚克力实验 · 第二方案:颜色键(LWA_COLORKEY)代替统一 alpha。

上一轮已证明:`SetWindowCompositionAttribute` 的亚克力 / BlurBehind 对
**LWA_ALPHA 层叠窗口**完全无效(前后两张图逐字节相同)。剩下的可能是
颜色键:把窗口底色写成魔法色 `#010203` 并声明为透明色,让那块像素**全透明**
(DWM 才会把背后的东西露出来),文字仍不透明。这样的窗口再挂上亚克力材质,
有可能真的看到模糊。

背后用**另一个终端窗口**铺满密集文字当高频背景(比噪点更容易看出"糊没糊")。

  a_magic    纯魔法色,不做处理(对照:背后什么都看不见)
  b_key      只有颜色键        → 背后文字应该清晰透出
  c_key_acr  颜色键 + 亚克力   → 背后文字若变糊 = 材质真的生效

用法:python tools/diagnostics/acrylic_key_probe.py
产物:target/_acrylic_key/*.png
"""

import ctypes
import ctypes.wintypes
import os
import pathlib
import subprocess
import sys
import time

from PIL import Image

ROOT = pathlib.Path(__file__).resolve().parents[2]
EXE = ROOT / "target" / "release" / "VectorTerm.exe"
OUT = ROOT / "target" / "_acrylic_key"

try:
    ctypes.WinDLL("shcore").SetProcessDpiAwareness(2)
except Exception:
    try:
        ctypes.WinDLL("user32").SetProcessDPIAware()
    except Exception:
        pass

user32 = ctypes.WinDLL("user32", use_last_error=True)
gdi32 = ctypes.WinDLL("gdi32", use_last_error=True)

SRCCOPY = 0x00CC0020
CAPTUREBLT = 0x40000000
GWL_EXSTYLE = -20
WS_EX_LAYERED = 0x00080000
LWA_COLORKEY = 0x1
MAGIC = "#010203"
COLORREF_MAGIC = 0x00030201  # 0x00BBGGRR


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
    bi.bmiHeader.biHeight = -h
    bi.bmiHeader.biPlanes = 1
    bi.bmiHeader.biBitCount = 32
    bi.bmiHeader.biCompression = 0
    buf = ctypes.create_string_buffer(w * h * 4)
    gdi32.GetDIBits(mem, bmp, 0, h, buf, ctypes.byref(bi), 0)
    gdi32.DeleteObject(bmp)
    gdi32.DeleteDC(mem)
    user32.ReleaseDC(0, hdc)
    return Image.frombuffer("RGBA", (w, h), buf.raw, "raw", "BGRA", 0, 1).convert("RGB")


def window_of_pid(pid):
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
    user32.SetWindowPos(hwnd, 0, x, y, w, h, 0x0004 | 0x0010)


ACCENT_ENABLE_ACRYLICBLURBEHIND = 4
WCA_ACCENT_POLICY = 19


class ACCENTPOLICY(ctypes.Structure):
    _fields_ = [("AccentState", ctypes.c_int), ("AccentFlags", ctypes.c_int),
                ("GradientColor", ctypes.c_uint), ("AnimationId", ctypes.c_int)]


class WINCOMPATTRDATA(ctypes.Structure):
    _fields_ = [("Attribute", ctypes.c_int), ("Data", ctypes.c_void_p),
                ("SizeOfData", ctypes.c_size_t)]


def set_accent(hwnd, state, gradient=0x99_0B1220):
    policy = ACCENTPOLICY(state, 0, gradient, 0)
    data = WINCOMPATTRDATA(WCA_ACCENT_POLICY,
                           ctypes.cast(ctypes.byref(policy), ctypes.c_void_p),
                           ctypes.sizeof(policy))
    return bool(user32.SetWindowCompositionAttribute(hwnd, ctypes.byref(data)))


def enable_colorkey(hwnd):
    style = user32.GetWindowLongW(hwnd, GWL_EXSTYLE)
    user32.SetWindowLongW(hwnd, GWL_EXSTYLE, style | WS_EX_LAYERED)
    return bool(user32.SetLayeredWindowAttributes(
        hwnd, ctypes.c_uint(COLORREF_MAGIC), ctypes.c_ubyte(0), LWA_COLORKEY))


BACK_CFG = """return {
  shell = "pwsh.exe",
  shell_args = { "-NoLogo", "-NoProfile", "-NoExit", "-Command",
                 "1..400 | ForEach-Object { 'iiiiiiiiiiiiiiiiiiiiiiiiiiiiiiii  WWWWWWWWWWWWWWWW  ####  ' + $_ }" },
  render = "cpu",
  font = { size = 14 },
  padding = 8,
  window_decorations = "RESIZE",
  window_background_color = "#FFFFFF",
  colors = { background = "#FFFFFF", foreground = "#000000" },
  tab_bar = { enable = false },
  session_log = "__LOG__",
}
"""

FRONT_CFG = """return {
  shell = "pwsh.exe",
  shell_args = { "-NoLogo", "-NoProfile", "-NoExit" },
  render = "gpu",
  font = { size = 14 },
  padding = 10,
  window_decorations = "RESIZE",
  window_background_color = "MAGIC",
  colors = { background = "MAGIC" },
  tab_bar = { enable = false },
  session_log = "__LOG__",
}
"""


def launch(name, cfg_text):
    cfg = OUT / f"{name}.lua"
    cfg.write_text(cfg_text.replace("__LOG__", (OUT / f"{name}.bin").as_posix())
                   .replace("MAGIC", MAGIC), encoding="utf-8")
    env = dict(os.environ)
    for k in list(env):
        if k.startswith("VECTORTERM_"):
            env.pop(k, None)
    p = subprocess.Popen([str(EXE), "--config", str(cfg)], env=env,
                         stdout=subprocess.DEVNULL,
                         stderr=open(OUT / f"{name}.trace.log", "wb"))
    time.sleep(2.2)
    return p, window_of_pid(p.pid)


def edge_energy(img, box):
    """区域内相邻像素平均差:文字越清晰越大,糊掉会明显变小。"""
    px = img.load()
    x0, y0, x1, y1 = box
    tot = cnt = 0
    for y in range(y0, y1, 2):
        for x in range(x0, x1, 2):
            a, b = px[x, y], px[x + 1, y]
            tot += sum(abs(a[i] - b[i]) for i in range(3))
            cnt += 1
    return tot / max(1, cnt)


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    back, bh = launch("back", BACK_CFG)
    move_window(bh, 120, 120, 900, 620)
    time.sleep(0.8)

    front, fh = launch("front", FRONT_CFG)
    move_window(fh, 260, 220, 620, 380)
    time.sleep(0.9)
    user32.SetForegroundWindow(fh)

    box = (260, 220, 880, 600)
    grab(*box).save(OUT / "a_magic.png")

    ok_key = enable_colorkey(fh)
    time.sleep(0.6)
    grab(*box).save(OUT / "b_key.png")

    ok_acr = set_accent(fh, ACCENT_ENABLE_ACRYLICBLURBEHIND)
    time.sleep(0.7)
    grab(*box).save(OUT / "c_key_acr.png")

    front.terminate()
    back.terminate()
    for p in (front, back):
        try:
            p.wait(timeout=8)
        except subprocess.TimeoutExpired:
            p.kill()

    print(f"  颜色键调用 {ok_key} / 亚克力调用 {ok_acr}", flush=True)
    for n in ("a_magic", "b_key", "c_key_acr"):
        im = Image.open(OUT / f"{n}.png").convert("RGB")
        # 中部区域:避开提示符文字,主要看"背后文字透出来的清晰度"
        e = edge_energy(im, (300, 330, 620, 560))
        print(f"  {n:<12} 边缘能量 {e:6.2f}", flush=True)


if __name__ == "__main__":
    main()
