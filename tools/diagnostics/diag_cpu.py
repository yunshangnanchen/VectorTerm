"""VectorTerm 跑 8 MiB:同时记录墙钟与进程 CPU 时间。

墙钟远大于 CPU 时间 = 时间花在等 ConPTY 出数据(终端侧没活干),
而不是解析/渲染。这是判断"瓶颈在管道还是在终端"最直接的证据。
"""

import ctypes
import os
import subprocess
import time
from ctypes import wintypes

import bench_split as B

k32 = ctypes.windll.kernel32
HERE = os.path.abspath(os.getcwd())
big = os.path.join(HERE, "target", "bench_big.txt")


class FILETIME(ctypes.Structure):
    _fields_ = [("lo", wintypes.DWORD), ("hi", wintypes.DWORD)]


def cpu_seconds(pid):
    h = k32.OpenProcess(0x0400 | 0x0010, False, pid)  # QUERY_INFORMATION | VM_READ
    if not h:
        return None
    c, e, kt, ut = FILETIME(), FILETIME(), FILETIME(), FILETIME()
    ok = k32.GetProcessTimes(h, ctypes.byref(c), ctypes.byref(e),
                             ctypes.byref(kt), ctypes.byref(ut))
    k32.CloseHandle(h)
    if not ok:
        return None

    def secs(ft):
        return ((ft.hi << 32) | ft.lo) / 1e7

    return secs(kt) + secs(ut)


def run(payload, label):
    bat = os.path.join(HERE, "target", f"cpu_{label}.bat")
    with open(bat, "w", newline="", encoding="ascii") as f:
        f.write("@echo off\r\n")
        f.write(f'type "{payload}"\r\n')
        f.write("ping -n 60 127.0.0.1 >nul\r\n")

    cfg = os.path.join(HERE, "target", "bench_term.lua")
    with open(cfg, "w", encoding="utf-8") as f:
        f.write('return {\n  render = "gpu",\n')
        f.write('  default_prog = { "cmd.exe", "/c", "'
                + bat.replace("\\", "\\\\") + '" },\n')
        f.write('  window_decorations = "TITLE|RESIZE",\n  initial_cols = 120,\n  initial_rows = 40,\n}\n')

    t0 = time.perf_counter()
    before = B.snapshot_windows()
    p = subprocess.Popen([os.path.join(HERE, "target", "release", "VectorTerm.exe"),
                          "--config", cfg],
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    hwnd = 0
    while time.perf_counter() - t0 < 25:
        hwnd = B.new_window(before)
        if hwnd:
            break
        time.sleep(0.02)
    mark = None
    if hwnd:
        r = wintypes.RECT()
        u = ctypes.windll.user32
        u.GetWindowRect(hwnd, ctypes.byref(r))
        while time.perf_counter() - t0 < 120:
            if B.bright_ratio((r.left, r.top, r.right, r.bottom)) > 0.35:
                mark = time.perf_counter() - t0
                break
            time.sleep(0.02)

    cpu = cpu_seconds(p.pid)
    subprocess.run(["taskkill", "/F", "/T", "/PID", str(p.pid)], capture_output=True)
    time.sleep(0.5)
    print(f"{label}: 墙钟 {mark if mark is None else round(mark, 3)} s, "
          f"CPU {None if cpu is None else round(cpu, 3)} s")
    return mark, cpu


if __name__ == "__main__":
    empty = os.path.join(HERE, "target", "bench_empty.txt")
    for pl, lb in ((empty, "empty"), (big, "big")):
        run(pl, lb)
