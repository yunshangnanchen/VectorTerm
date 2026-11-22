"""五方端到端基准:VectorTerm(GPU/CPU) / Windows Terminal / WezTerm / Ghostty。

沿用 bench_split.py 的两段式口径:
  * 空负载(只有洋红标记) -> 进程启动到首帧上屏
  * 8.1 MiB 负载          -> 启动 + 传输 + 解析 + 渲染
  两者相减 = 纯吞吐。判定"画到屏幕"靠负载末尾那片真彩色洋红(48;2;255;0;255),
  不受深浅主题干扰。

Ghostty 说明
------------
官方 Ghostty 没有 Windows GUI 构建(上游 1.3 release note 明确写"still not
planned"),这里测的是第三方原生移植 shiweis/ghostty-windows 的 win-v1.3.0
便携版(Zig ReleaseFast + OpenGL/WGL + ConPTY)。它自己的 README 也声明
"上游 macOS/Linux 的 benchmark 数字不适用于 Windows",所以本脚本只把它当作
"一个 Windows 上的 Ghostty 内核"来测,不代表官方性能。

kitty 说明
----------
kitty 没有 Windows 版(依赖 X11/Wayland),本机 wsl.exe 又被安全策略拦截,
无法实测,不在此脚本内。
"""

import ctypes
import os
import subprocess
import sys
import time
from ctypes import wintypes

from PIL import ImageGrab

u = ctypes.windll.user32
u.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
u.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int,
                           ctypes.c_int, ctypes.c_int, ctypes.c_int,
                           ctypes.c_uint]

SWP_NOSIZE = 0x0001
SWP_NOZORDER = 0x0004
SWP_NOACTIVATE = 0x0010
PARK = -30000  # 挪到屏幕外,不参与 DWM 合成

HERE = os.path.abspath(os.getcwd())
GHOSTTY = os.path.join(HERE, "target", "terms", "gw", "ghostty", "ghostty.exe")
# WezTerm 的安装位置因机器而异,默认取官方安装包的位置,可用环境变量覆盖
WEZTERM = os.environ.get("WEZTERM_EXE", r"C:\Program Files\WezTerm\wezterm.exe")
COLS = 120
MARK_ROWS = 40
BIG_ROWS = int(os.environ.get("BIG_ROWS", "104000"))  # 80 字符 x 104000 ≈ 8.1 MiB
RUNS = int(os.environ.get("RUNS", "3"))


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
    """新出现的、够大的、有标题的可见窗口 —— 排除工具提示和隐形辅助窗口。"""
    hit = []
    buf = ctypes.create_unicode_buffer(512)
    rect = wintypes.RECT()

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def cb(h, _):
        if h in before or not u.IsWindowVisible(h):
            return True
        if not u.GetWindowTextW(h, buf, 512):
            return True
        u.GetWindowRect(h, ctypes.byref(rect))
        if rect.right - rect.left < 200 or rect.bottom - rect.top < 100:
            return True
        hit.append(h)
        return False

    u.EnumWindows(cb, 0)
    return hit[0] if hit else 0


def bright_ratio(bbox):
    """窗口里"洋红像素"的占比 —— 只认负载末尾刷的那一片真彩色底。

    bbox 为 None 时抓整个虚拟屏幕,用来判断有没有上一轮的残留。
    """
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


RUN_BAT = os.path.join(HERE, "target", "bench_run.bat")


def set_payload(payload):
    """把负载写进固定路径的 .bat —— argv 里直接引用它,两边保持一致。"""
    with open(RUN_BAT, "w", newline="", encoding="ascii") as f:
        f.write("@echo off\r\n")
        f.write(f'type "{payload}"\r\n')
        f.write("ping -n 60 127.0.0.1 >nul\r\n")
    return RUN_BAT


def brightness_of_full_screen():
    return bright_ratio(None)


def park(hwnd):
    """把旧窗口挪到屏幕外。

    `taskkill /PID` 对 wt.exe / wezterm.exe 往往只杀掉启动器存根,真正的
    窗口进程还在、还停在原来的位置上,而且屏幕上还留着上一轮那片洋红。
    下一轮的新窗口开在同一坐标,截屏就把旧窗口的洋红读成"新窗口画完了",
    于是量出 7840 MiB/s 这种不可能的吞吐。挪出屏幕比强杀安全,也不影响
    用户自己开着的窗口。
    """
    if not hwnd:
        return
    u.SetWindowPos(hwnd, None, PARK, PARK, 0, 0,
                   SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE)


def wait_clear(tries=20):
    """确认整个虚拟屏幕上已经没有洋红残留,再开始下一轮。"""
    for _ in range(tries):
        if brightness_of_full_screen() < 0.02:
            return True
        time.sleep(0.3)
    return False


def run_once(argv, cwd, timeout=120):
    t0 = time.perf_counter()
    before = snapshot_windows()
    p = subprocess.Popen(argv, cwd=cwd, stdout=subprocess.DEVNULL,
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
        box = (r.left, r.top, r.right, r.bottom)
        # 新窗口刚出现时不该已经有洋红;有就是上一轮漏在屏幕上了
        if bright_ratio(box) > 0.20:
            print("    ! 起测时区域已有洋红,本次数值作废", flush=True)
        else:
            while time.perf_counter() - t0 < timeout:
                if bright_ratio(box) > 0.35:
                    done = time.perf_counter() - t0
                    break
                time.sleep(0.02)

    subprocess.run(["taskkill", "/F", "/T", "/PID", str(p.pid)],
                   capture_output=True)
    park(hwnd)
    time.sleep(0.4)
    wait_clear()
    return done


def measure(name, argv, cwd, runs=RUNS):
    xs = []
    run_once(argv, cwd)  # 预热,不计入(尤其 wezterm 要拉 mux server)
    for i in range(runs):
        d = run_once(argv, cwd)
        if d is None:
            print(f"  {name} #{i+1}: 未检出", flush=True)
            continue
        xs.append(d)
        print(f"  {name} #{i+1}: {d:.3f}s", flush=True)
    if not xs:
        return None
    xs.sort()
    return xs[len(xs) // 2]


def vectorterm_args(render, bat):
    cfg = os.path.join(HERE, "target", f"bench_{render}.lua")
    with open(cfg, "w", encoding="utf-8") as f:
        f.write('return {\n  render = "' + render + '",\n')
        f.write('  default_prog = { "cmd.exe", "/c", "'
                + bat.replace("\\", "\\\\") + '" },\n')
        f.write('  window_decorations = "TITLE|RESIZE",\n')
        f.write(f'  initial_cols = {COLS},\n  initial_rows = 40,\n}}\n')
    return ([os.path.join(HERE, "target", "release", "VectorTerm.exe"),
             "--config", cfg], HERE)


def targets(bat):
    gpu = vectorterm_args("gpu", bat)
    cpu = vectorterm_args("cpu", bat)
    return [
        ("VectorTerm GPU", gpu[0], gpu[1]),
        ("VectorTerm CPU", cpu[0], cpu[1]),
        ("Windows Terminal", ["wt.exe", "-w", "-1", "cmd.exe", "/c", bat], HERE),
        ("WezTerm", [WEZTERM, "start", "--", "cmd.exe", "/c", bat], HERE),
        ("Ghostty (win 1.3 fork)",
         [GHOSTTY, "-e", "cmd.exe", "/c", bat], os.path.dirname(GHOSTTY)),
    ]


def main():
    empty = os.path.join(HERE, "target", "bench_empty.txt")
    big = os.path.join(HERE, "target", f"bench_big_{BIG_ROWS}.txt")
    build(empty, 0)
    build(big, BIG_ROWS)
    size = os.path.getsize(big)
    only = sys.argv[1].lower() if len(sys.argv) > 1 else ""

    print(f"重负载 {size/1048576:.2f} MiB   窗口 {COLS} 列 x 40 行   "
          f"每项 {RUNS} 次取中位数", flush=True)

    rows = []
    for name, argv, cwd in targets(RUN_BAT):
        if only and only not in name.lower():
            continue
        if not os.path.exists(argv[0]) and os.path.sep in argv[0]:
            print(f"\n== {name}: 可执行文件不存在,跳过 ==", flush=True)
            continue
        print(f"\n== {name} 空负载 ==", flush=True)
        set_payload(empty)
        a = measure(name + " empty", argv, cwd)
        print(f"== {name} 8 MiB ==", flush=True)
        set_payload(big)
        b = measure(name + " big", argv, cwd)
        if a and b:
            secs = max(0.001, b - a)
            tp = size / 1048576 / secs
            rows.append((name, a, b, secs, tp))
            print(f">> {name}: 启动 {a*1000:.0f} ms / 总 {b*1000:.0f} ms / "
                  f"纯吞吐 {tp:.2f} MiB/s", flush=True)
        else:
            print(f">> {name}: 数据不全", flush=True)

    print("\n| 终端 | 冷启动 | 8.1 MiB 总耗时 | 纯吞吐 |", flush=True)
    print("|---|---|---|---|", flush=True)
    for name, a, b, secs, tp in rows:
        print(f"| {name} | {a*1000:.0f} ms | {b:.2f} s | {tp:.2f} MiB/s |",
              flush=True)


if __name__ == "__main__":
    main()
