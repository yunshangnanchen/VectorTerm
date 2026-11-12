"""高速抓帧版的拉伸检测。

PIL 的 ImageGrab 一次要 20~50ms,而 DWM 的「旧表面被拉伸」只存在到下一次
present(几毫秒到一帧),用低帧率采样根本统计不出比例。这里改用 GDI
BitBlt + CreateDIBSection,只抓标尺文字所在的一条横带,可以跑到几百帧/秒。

用法: python target/verify_stretch_fast.py <输出前缀> <配置lua> [x|y]
环境: VECTORTERM_EXE 指定二进制; VECTORTERM_STEPS / VECTORTERM_STEP_PX / VECTORTERM_STEP_S 调拖动
"""
import ctypes
import ctypes.wintypes as wt
import os
import subprocess
import sys
import threading
import time
from pathlib import Path

import numpy as np
from PIL import Image, ImageGrab

ROOT = Path(__file__).resolve().parent.parent.parent
EXE = Path(os.environ.get("VECTORTERM_EXE", ROOT / "target" / "release" / "VectorTerm.exe"))

try:
    ctypes.windll.shcore.SetProcessDpiAwareness(2)
except Exception:
    try:
        ctypes.windll.user32.SetProcessDPIAware()
    except Exception:
        pass

user32 = ctypes.WinDLL("user32", use_last_error=True)
gdi32 = ctypes.WinDLL("gdi32", use_last_error=True)

user32.EnumWindows.argtypes = [ctypes.WINFUNCTYPE(ctypes.c_bool, wt.HWND, wt.LPARAM), wt.LPARAM]
user32.GetWindowThreadProcessId.argtypes = [wt.HWND, ctypes.POINTER(wt.DWORD)]
user32.GetWindowTextW.argtypes = [wt.HWND, wt.LPWSTR, ctypes.c_int]
user32.GetWindowRect.argtypes = [wt.HWND, ctypes.POINTER(wt.RECT)]
user32.GetClientRect.argtypes = [wt.HWND, ctypes.POINTER(wt.RECT)]
user32.ClientToScreen.argtypes = [wt.HWND, ctypes.POINTER(wt.POINT)]
user32.SetCursorPos.argtypes = [ctypes.c_int, ctypes.c_int]
user32.mouse_event.argtypes = [wt.DWORD, wt.DWORD, wt.DWORD, wt.DWORD, ctypes.c_void_p]
user32.SetForegroundWindow.argtypes = [wt.HWND]
user32.GetDC.restype = ctypes.c_void_p
user32.GetDC.argtypes = [wt.HWND]
user32.ReleaseDC.argtypes = [wt.HWND, ctypes.c_void_p]

gdi32.CreateCompatibleDC.restype = ctypes.c_void_p
gdi32.CreateCompatibleDC.argtypes = [ctypes.c_void_p]
gdi32.SelectObject.restype = ctypes.c_void_p
gdi32.SelectObject.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
gdi32.BitBlt.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_int,
                         ctypes.c_int, ctypes.c_void_p, ctypes.c_int, ctypes.c_int, wt.DWORD]


class BITMAPINFOHEADER(ctypes.Structure):
    _fields_ = [
        ("biSize", wt.DWORD), ("biWidth", wt.LONG), ("biHeight", wt.LONG),
        ("biPlanes", wt.WORD), ("biBitCount", wt.WORD), ("biCompression", wt.DWORD),
        ("biSizeImage", wt.DWORD), ("biXPelsPerMeter", wt.LONG),
        ("biYPelsPerMeter", wt.LONG), ("biClrUsed", wt.DWORD), ("biClrImportant", wt.DWORD),
    ]


class BITMAPINFO(ctypes.Structure):
    _fields_ = [("bmiHeader", BITMAPINFOHEADER), ("bmiColors", wt.DWORD * 3)]


gdi32.CreateDIBSection.restype = ctypes.c_void_p
gdi32.CreateDIBSection.argtypes = [ctypes.c_void_p, ctypes.POINTER(BITMAPINFO), wt.UINT,
                                   ctypes.POINTER(ctypes.c_void_p), ctypes.c_void_p, wt.DWORD]

LEFTDOWN, LEFTUP = 0x0002, 0x0004
SRCCOPY = 0x00CC0020
CAPTUREBLT = 0x40000000
STEP_PX = int(os.environ.get("VECTORTERM_STEP_PX", "8"))
STEP_S = float(os.environ.get("VECTORTERM_STEP_S", "0.05"))
STEPS = int(os.environ.get("VECTORTERM_STEPS", "20"))
VK_LBUTTON = 1


class FastGrab:
    """固定区域的高速抓帧器(32bpp top-down DIB)。"""

    def __init__(self, x, y, w, h):
        self.x, self.y, self.w, self.h = x, y, w, h
        self.screen = user32.GetDC(None)
        self.mem = gdi32.CreateCompatibleDC(self.screen)
        bmi = BITMAPINFO()
        bmi.bmiHeader.biSize = ctypes.sizeof(BITMAPINFOHEADER)
        bmi.bmiHeader.biWidth = w
        bmi.bmiHeader.biHeight = -h  # 负数 = top-down
        bmi.bmiHeader.biPlanes = 1
        bmi.bmiHeader.biBitCount = 32
        bmi.bmiHeader.biCompression = 0
        self.bits = ctypes.c_void_p()
        self.bmp = gdi32.CreateDIBSection(self.screen, ctypes.byref(bmi), 0,
                                          ctypes.byref(self.bits), None, 0)
        gdi32.SelectObject(self.mem, self.bmp)

    def grab(self):
        gdi32.BitBlt(self.mem, 0, 0, self.w, self.h, self.screen,
                     self.x, self.y, SRCCOPY | CAPTUREBLT)
        buf = ctypes.string_at(self.bits, self.w * self.h * 4)
        a = np.frombuffer(buf, dtype=np.uint8).reshape(self.h, self.w, 4)
        # BGR -> 亮度。必须用 int32:255*150 已经超出 int16,溢出会把亮部
        # 变成负数,阈值判断全部失效。
        return ((a[:, :, 2].astype(np.int32) * 77 +
                 a[:, :, 1].astype(np.int32) * 150 +
                 a[:, :, 0].astype(np.int32) * 29) >> 8).clip(0, 255).astype(np.int16)


def find_hwnd(pid):
    found = []

    @ctypes.WINFUNCTYPE(ctypes.c_bool, wt.HWND, wt.LPARAM)
    def cb(hwnd, _):
        p = wt.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(p))
        if p.value == pid and user32.IsWindowVisible(hwnd):
            t = ctypes.create_unicode_buffer(256)
            user32.GetWindowTextW(hwnd, t, 256)
            if t.value == "VectorTerm":
                found.append(hwnd)
        return True

    user32.EnumWindows(cb, 0)
    return found[0] if found else None


def rect_of(hwnd):
    r = wt.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(r))
    return r


def client_origin(hwnd):
    p = wt.POINT(0, 0)
    user32.ClientToScreen(hwnd, ctypes.byref(p))
    return p.x, p.y


def span_of(img, y0, y1):
    """在给定行范围内找最宽文字行的 (x_min, x_max, span)。"""
    best = None
    for y in range(y0, min(y1, img.shape[0])):
        xs = np.nonzero(img[y] > 120)[0]
        if len(xs) < 16:
            continue
        s = int(xs.max() - xs.min())
        if best is None or s > best[2]:
            best = (int(xs.min()), int(xs.max()), s)
    return best


def main():
    out = Path(sys.argv[1])
    cfg = Path(sys.argv[2])
    axis = sys.argv[3] if len(sys.argv) > 3 else "x"
    err_path = out.with_name(out.stem + ".err")
    with err_path.open("wb") as err:
        proc = subprocess.Popen([str(EXE), "--config", str(cfg)],
                                stdout=subprocess.DEVNULL, stderr=err, cwd=str(ROOT))
    try:
        hwnd = None
        for _ in range(60):
            time.sleep(0.2)
            hwnd = find_hwnd(proc.pid)
            if hwnd:
                break
        if not hwnd:
            print("WINDOW NOT FOUND")
            return
        user32.SetForegroundWindow(hwnd)
        time.sleep(3.2)

        x0, y0 = client_origin(hwnd)
        cr = wt.RECT()
        user32.GetClientRect(hwnd, ctypes.byref(cr))
        cw, ch = cr.right - cr.left, cr.bottom - cr.top
        r = rect_of(hwnd)
        w0, h0 = r.right - r.left, r.bottom - r.top

        # 先用一次常规截图定位标尺文字行的 y
        probe = np.asarray(ImageGrab.grab(bbox=(x0, y0, x0 + min(1000, cw), y0 + 120)).convert("L"),
                           dtype=np.int16)
        best_y, best_span = 34, 0
        for y in range(34, probe.shape[0]):
            xs = np.nonzero(probe[y] > 120)[0]
            if len(xs) >= 16 and (xs.max() - xs.min()) > best_span:
                best_span = int(xs.max() - xs.min())
                best_y = y
        band_y0 = max(0, best_y - 3)
        band_h = 14
        print("INIT frame=%dx%d client=%dx%d drag_from=(%d,%d) axis=%s exe=%s band_y=%d base_span=%d" %
              (w0, h0, cw, ch, 0, 0, axis, EXE.name, best_y, best_span))

        user32.SetCursorPos(x0 + cw // 2, y0 + ch // 2)
        time.sleep(0.2)
        user32.mouse_event(LEFTDOWN, 0, 0, 0, None)
        user32.mouse_event(LEFTUP, 0, 0, 0, None)
        time.sleep(0.5)

        grab = FastGrab(x0, y0 + band_y0, min(1000, cw), band_h)
        samples = []
        stop = threading.Event()

        def loop():
            while not stop.is_set():
                t = time.perf_counter()
                g = grab.grab()
                samples.append((t, span_of(g, 0, band_h)))

        if axis == "y":
            sx, sy = x0 + cw // 2, y0 + ch - 4
        else:
            sx, sy = x0 + cw - 4, y0 + ch // 2
        print("DRAG_FROM (%d,%d)" % (sx, sy))

        th = threading.Thread(target=loop, daemon=True)
        th.start()
        time.sleep(0.4)          # 静止基线
        base_n = len(samples)

        user32.SetCursorPos(sx, sy)
        time.sleep(0.2)
        user32.mouse_event(LEFTDOWN, 0, 0, 0, None)
        time.sleep(0.12)
        print("LBUTTON_DOWN", bool(user32.GetAsyncKeyState(VK_LBUTTON) & 0x8000))

        t0 = time.perf_counter()
        sizes = []
        for i in range(STEPS):
            if axis == "y":
                user32.SetCursorPos(sx, sy + (i + 1) * STEP_PX)
            else:
                user32.SetCursorPos(sx + (i + 1) * STEP_PX, sy)
            time.sleep(STEP_S)
            rr = rect_of(hwnd)
            sizes.append((rr.right - rr.left, rr.bottom - rr.top))
        t1 = time.perf_counter()
        user32.mouse_event(LEFTUP, 0, 0, 0, None)
        time.sleep(0.5)
        stop.set()
        th.join(timeout=3)

        base = [s[2] for _, s in samples[:base_n] if s]
        if not base:
            print("NO BASELINE")
            return
        base_span = max(set(base), key=base.count)
        dur = t1 - t0
        drag = [(t, s) for t, s in samples if t0 <= t <= t1 and s]
        fps = len(drag) / dur if dur > 0 else 0
        spans = [s[2] for _, s in drag]

        print("FRAME %dx%d -> %dx%d" % (sizes[0][0], sizes[0][1], sizes[-1][0], sizes[-1][1]))
        print("SAMPLES total=%d baseline=%d during_drag=%d  grab_fps=%.0f" %
              (len(samples), base_n, len(drag), fps))
        print("BASE_SPAN %d  DRAG_SPAN min=%d max=%d" % (base_span, min(spans), max(spans)))
        over = [s for s in spans if s > base_span + 2]
        under = [s for s in spans if s < base_span - 2]
        print("STRETCH_FRAMES %d/%d (%.1f%%)  max_over=%+d px (%.2f%%)  shrunk=%d" %
              (len(over), len(spans), 100.0 * len(over) / len(spans),
               max(spans) - base_span, (max(spans) - base_span) / base_span * 100.0, len(under)))
        hist = {}
        for s in spans:
            hist[s] = hist.get(s, 0) + 1
        print("DRAG_HIST", " ".join("%d:%d" % kv for kv in sorted(hist.items())))
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()


if __name__ == "__main__":
    main()
