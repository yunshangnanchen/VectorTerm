"""实机验证:关闭标签 / 关闭窗口的确认框。

起因(用户报的问题):
  点标签栏的 × 之后,那个 "Really kill this tab ..." 的确认界面没有出现,
  标签直接就没了。

原因:`close_tab` 以前是直接关的,只有「关最后一个标签 = 关窗口」那条路
会走 `ask_quit` 弹确认 —— 也就是关**标签**从来没有确认这一环。wezterm 是
两条独立的确认(`confirm_close_tab` / `confirm_close_window`),文案也不同。

本探针跑一遍完整流程:
  1. `new@900` 开标签 2;
  2. 程序自己 2.5s 时把 `ping -t 127.0.0.1` 写进活动标签(标签 2),
     于是标签 2 里"有程序在跑";
  3. `close@3200` → 应该弹确认框,**文案是 tab 版**;
  4. `no@4200` → 取消,标签还在;
  5. `close@5000` → 再弹一次;
  6. `yes@6000` → 标签真的关掉,回到标签 1。

为什么不用 `SendInput` 发 `Ctrl+Shift+W`:窗口没有键盘焦点时那些键会被
静默丢掉,自动化里时灵时不灵(这台机器上实测就完全没送到)。`VECTORTERM_AUTO_TAB`
走的是同一个内部入口(`close_tab_or_ask` / `confirm_yes`),只是绕开焦点问题。

用法: python tools/diagnostics/confirm_probe.py [debug|release]
"""
import ctypes
import ctypes.wintypes as wt
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
MODE = sys.argv[1] if len(sys.argv) > 1 else "debug"
EXE = ROOT / "target" / MODE / "VectorTerm.exe"
OUT = ROOT / "target" / "confirm_probe"
OUT.mkdir(parents=True, exist_ok=True)

# cmd.exe 里 `ping -t` 是不退出的前台程序,正好当"还有程序在跑"的靶子。
CFG = """return {
  initial_cols = 100,
  initial_rows = 26,
  shell = "cmd.exe",
  shell_args = { "/k", "title probe" },
  status_bar = { enabled = false },
  colors = { foreground = "#cccccc", background = "#0c0c0c" },
  font = { size = 15, line_height = 1.1, cell_width = 1.0 },
}
"""
CFG_PATH = OUT / "probe.lua"
CFG_PATH.write_text(CFG, encoding="utf-8")

LOG = Path(os.path.expanduser("~/.config/VectorTerm/VectorTerm.log"))

u32 = ctypes.WinDLL("user32", use_last_error=True)
try:
    u32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
except Exception:
    pass
u32.GetWindowThreadProcessId.argtypes = [wt.HWND, ctypes.POINTER(wt.DWORD)]
u32.SetForegroundWindow.argtypes = [wt.HWND]


def find_window(pid):
    """返回 (hwnd, rect)。矩形要在这里一并拿 —— 窗口关掉之后句柄就失效了,
    而我们要拍的最后一帧恰恰是"关完"。"""
    found = []

    @ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
    def cb(hwnd, _):
        wpid = wt.DWORD()
        u32.GetWindowThreadProcessId(hwnd, ctypes.byref(wpid))
        if wpid.value == pid and u32.IsWindowVisible(hwnd):
            r = wt.RECT()
            u32.GetWindowRect(hwnd, ctypes.byref(r))
            w, h = r.right - r.left, r.bottom - r.top
            if w > 400 and h > 300:
                found.append((w * h, hwnd, r))
        return True

    u32.EnumWindows(cb, 0)
    if not found:
        return None, None
    _, hwnd, rect = max(found, key=lambda x: x[0])
    return hwnd, rect


def grab_frame(at_ms):
    """取程序在 `at_ms` 那一刻导出的帧。

    不用 `ImageGrab` 截屏 —— 窗口一旦被别的窗口盖住,截到的就是别人;
    程序自己导出的帧是渲染结果,和遮挡、焦点、DPI 都无关。
    """
    from PIL import Image
    for _ in range(40):
        f = OUT / f"frame-{at_ms}.bmp"
        if f.exists() and f.stat().st_size > 0:
            time.sleep(0.05)          # 等写完
            im = Image.open(f).convert("RGB")
            im.save(OUT / f"confirm_{at_ms}.png")
            return im
        time.sleep(0.05)
    raise SystemExit(f"没有导出 frame-{at_ms}.bmp")


def mid_ink(im):
    """内容区**中部**的墨迹量 —— 确认框和最明显的区别就在这里。

    不能量"纯色占比":终端本来就有九成是底色(cmd 里 ping 只吐几行),
    确认框清屏之后也是九成底色,两者分不开。但位置完全不同 ——
    终端输出贴着顶部走,确认框是**垂直居中**的两行字。所以只数中间那一段
    (高度的 35%~65%)有多少非底色像素:确认框几百到上千,终端接近 0。
    """
    w, h = im.size
    px = im.load()
    bg = px[w - 4, h - 4]
    y0, y1 = int(h * 0.35), int(h * 0.66)
    x0, x1 = int(w * 0.04), int(w * 0.96)
    n = 0
    for y in range(y0, y1, 2):
        for x in range(x0, x1, 2):
            r, g, b = px[x, y]
            if abs(r - bg[0]) + abs(g - bg[1]) + abs(b - bg[2]) > 40:
                n += 1
    return n


def tail(off):
    if not LOG.exists():
        return ""
    with LOG.open("r", encoding="utf-8", errors="replace") as f:
        f.seek(off)
        return f.read()


ok = True
off = LOG.stat().st_size if LOG.exists() else 0

env = dict(os.environ)
for k in ("VECTORTERM_AUTO_SPLIT", "VECTORTERM_AUTO_FILES", "VECTORTERM_AUTO_PANEL",
          "VECTORTERM_DUMP_FRAME", "VECTORTERM_DUMP_AT", "VECTORTERM_IME_OPEN"):
    env.pop(k, None)
env["VECTORTERM_AUTOTYPE"] = "ping -t 127.0.0.1\\r"
env["VECTORTERM_AUTO_TAB"] = "new@900,close@3200,no@4200,close@5000,yes@6000"

DUMP_AT = (3500, 4600, 5400, 6500)
env["VECTORTERM_DUMP_FRAME"] = str(OUT / "frame.bmp")
env["VECTORTERM_DUMP_AT"] = ",".join(str(t) for t in DUMP_AT)

p = subprocess.Popen([str(EXE), "--config", str(CFG_PATH)], cwd=str(ROOT), env=env,
                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
time.sleep(1.2)
HWND, RECT = find_window(p.pid)
if not HWND:
    p.terminate()
    sys.exit("找不到窗口")

time.sleep(5.8)                       # 等 6500ms 那一帧落盘
log = tail(off)
asked = log.count("已弹出关闭确认")
print(f"日志里弹了 {asked} 次确认(应为 2)")

ims = [grab_frame(t) for t in DUMP_AT]
ratios = [mid_ink(im) for im in ims]
names = ["tab_ask", "tab_cancel", "tab_again", "tab_closed"]
for n, t, r in zip(names, DUMP_AT, ratios):
    print(f"  {t}ms [{n}] 中间一行字={r}")

# 1) 3200ms 关标签 → 3500ms 应看到清屏的确认框
if not (asked >= 1 and ratios[0] > 200):
    print("  !! 第一次关标签没有弹出确认,或屏幕没被清空")
    ok = False
# 2) no 之后回到终端
if ratios[1] > 100:
    print("  !! 按 no 之后确认框没关掉")
    ok = False
# 3) 再关一次,又弹
if not (asked >= 2 and ratios[2] > 200):
    print("  !! 第二次关标签没有弹出确认")
    ok = False
# 4) yes 之后确认框消失
if ratios[3] > 100:
    print("  !! 按 yes 之后确认框还在")
    ok = False

print(f"[5] 窗口存活={p.poll() is None}")

p.terminate()
try:
    p.wait(timeout=3)
except subprocess.TimeoutExpired:
    p.kill()

print("\n日志里这一段:")
for line in log.splitlines():
    if "确认" in line:
        print("  ", line.strip())

print("\n结果:", "PASS" if ok else "FAIL")
sys.exit(0 if ok else 1)
