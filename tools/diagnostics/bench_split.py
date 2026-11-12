"""VectorTerm vs Windows Terminal:把"启动"和"吞吐"分开测。

两段负载
--------
* 空负载:只有 40 行反显标记(不产出文本) —— 测的是**进程启动到首帧上屏**
* 重负载:8 MiB 文本 + 同样的反显标记 —— 测的是启动 + 传输 + 解析 + 渲染

两者相减就是纯吞吐时间。之所以这样拆:标题/像素标记只能测到"最后一批
像素出现",而一次窗口创建(尤其 WT 的 TerminalApp 冷启动)能占掉几百毫秒,
不扣掉的话会把吞吐比得面目全非。

wezterm 不在此脚本内:它在启动时会探测 WSL 域,本机 wsl.exe 被安全策略
拦截,启动时间会被这条分支污染,数据放到报告里单独说明。
"""

import ctypes
import os
import subprocess
import time
from ctypes import wintypes

from PIL import ImageGrab

u = ctypes.windll.user32
u.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]

HERE = os.path.abspath(os.getcwd())
COLS = 120
MARK_ROWS = 40
BIG_ROWS = 104_000  # 80 字符 x 104000 ≈ 8.1 MiB


def build(path, text_rows):
    if os.path.exists(path):
        return
    with open(path, "w", encoding="ascii", newline="") as f:
        for i in range(text_rows):
            f.write(f"{i:06d} " + "abcdefghij" * 7 + "\r\n")
        # 真彩色洋红底:任何主题下都不会自然出现,判定不会误伤
        f.write("\x1b[48;2;255;0;255m")
        for _ in range(MARK_ROWS):
            f.write(" " * COLS + "\r\n")
        f.write("\x1b[0m")


def snapshot_windows():
    seen = set()

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def cb(h, _):
        if u.IsWindowVisible(h):
            seen.add(h)
        return True

    u.EnumWindows(cb, 0)
    return seen


def new_window(before):
    hit = []
    buf = ctypes.create_unicode_buffer(512)

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def cb(h, _):
        if h not in before and u.IsWindowVisible(h) and u.GetWindowTextW(h, buf, 512):
            hit.append(h)
            return False
        return True

    u.EnumWindows(cb, 0)
    return hit[0] if hit else 0


def bright_ratio(bbox):
    """窗口里"洋红像素"的占比 —— 只认负载末尾刷的那一片真彩色底。"""
    try:
        sc = ImageGrab.grab(bbox=bbox, all_screens=True)
    except Exception:
        return 0.0
    small = sc.resize((max(1, sc.size[0] // 5), max(1, sc.size[1] // 5)))
    px = small.load()
    w, h = small.size
    total = 0
    hit = 0
    for y in range(0, h, 2):
        for x in range(0, w, 2):
            r, g, b = px[x, y][:3]
            total += 1
            if r > 150 and b > 150 and g < 110:
                hit += 1
    return hit / max(1, total)


def run_once(argv_fn, payload, timeout=120):
    tag = f"B{int(time.time()*1000)}"
    bat = os.path.join(HERE, "target", f"bench_{tag}.bat")
    with open(bat, "w", newline="", encoding="ascii") as f:
        f.write("@echo off\r\n")
        f.write(f'type "{payload}"\r\n')
        f.write("ping -n 60 127.0.0.1 >nul\r\n")

    t0 = time.perf_counter()
    before = snapshot_windows()
    p = subprocess.Popen(argv_fn(bat), stdout=subprocess.DEVNULL,
                         stderr=subprocess.DEVNULL)

    hwnd = 0
    while time.perf_counter() - t0 < 25:
        hwnd = new_window(before)
        if hwnd:
            break
        time.sleep(0.02)

    done = None
    if hwnd:
        r = wintypes.RECT()
        u.GetWindowRect(hwnd, ctypes.byref(r))
        while time.perf_counter() - t0 < timeout:
            if bright_ratio((r.left, r.top, r.right, r.bottom)) > 0.35:
                done = time.perf_counter() - t0
                break
            time.sleep(0.02)

    subprocess.run(["taskkill", "/F", "/T", "/PID", str(p.pid)],
                   capture_output=True)
    time.sleep(0.8)
    return done


def measure(name, argv_fn, payload, runs=5):
    xs = []
    for i in range(runs):
        d = run_once(argv_fn, payload)
        if d is None:
            print(f"  {name} #{i+1}: 未检出")
            continue
        xs.append(d)
        print(f"  {name} #{i+1}: {d:.3f}s")
    if not xs:
        return None
    xs.sort()
    return xs[len(xs) // 2]


def main():
    empty = os.path.join(HERE, "target", "bench_empty.txt")
    big = os.path.join(HERE, "target", "bench_big.txt")
    build(empty, 0)
    build(big, BIG_ROWS)
    size = os.path.getsize(big) - 0  # 反显标记也在里面,占比很小

    def vectorterm(bat):
        cfg = os.path.join(HERE, "target", "bench_term.lua")
        with open(cfg, "w", encoding="utf-8") as f:
            f.write('return {\n  render = "gpu",\n')
            f.write('  default_prog = { "cmd.exe", "/c", "'
                    + bat.replace("\\", "\\\\") + '" },\n')
            f.write('  window_decorations = "TITLE|RESIZE",\n')
            f.write(f'  initial_cols = {COLS},\n  initial_rows = 40,\n}}\n')
        return [os.path.join(HERE, "target", "release", "VectorTerm.exe"),
                "--config", cfg]

    def wt(bat):
        return ["wt.exe", "-w", "-1", "cmd.exe", "/c", bat]

    print(f"重负载 {size/1048576:.2f} MiB")
    for name, fn in (("VectorTerm", vectorterm), ("Windows Terminal", wt)):
        print(f"\n== {name} 空负载 ==")
        a = measure(name + " empty", fn, empty)
        print(f"\n== {name} 8 MiB ==")
        b = measure(name + " big", fn, big)
        if a and b:
            tp = size / 1048576 / max(0.001, b - a)
            print(f"\n{name}: 启动 {a*1000:.0f} ms / 总 {b*1000:.0f} ms "
                  f"/ 纯吞吐 {tp:.2f} MiB/s")
        else:
            print(f"\n{name}: 数据不全")


if __name__ == "__main__":
    main()
