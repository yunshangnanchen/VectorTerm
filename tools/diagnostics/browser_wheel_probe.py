"""文件面板滚轮方向:真实滚轮事件下的前后对照。

症状:滚轮往**下**滚,文件列表却往上跑(选中项往回走)。

判定方式不靠猜:面板开着、鼠标停在面板上,用 `mouse_event` 发真的滚轮事件,
前后各导一帧,比较**列表第一个可见条目**和**滚动条滑块**的位置 ——
滚轮向下 = 列表朝"后面的条目"走 = 滑块往下移 = 首条目变成更靠后的名字。

  base   只把鼠标移进面板,不发滚轮
  down   移到面板上,发 3 格滚轮向下
  up     移到面板上,发 3 格滚轮向上(对照,应该回到更前面的条目)

用法:`python tools/diagnostics/browser_wheel_probe.py`
产物:`target/_browser_wheel/*.png`
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
OUT = ROOT / "target" / "_browser_wheel"

CFG = """return {
  shell = "pwsh.exe",
  shell_args = { "-NoLogo", "-NoProfile", "-NoExit" },
  render = "cpu",
  font = { size = 14, line_height = 1, cell_width = 1 },
  padding = 10,
  window_decorations = "RESIZE",
  window_width = 1100,
  window_height = 720,
  tab_bar = { enable = true, hide_if_one = false, at_bottom = false },
  files_gui = { side_cols = 34 },
  session_log = "__LOG__",
}
"""

try:
    ctypes.WinDLL("shcore").SetProcessDpiAwareness(2)
except Exception:
    try:
        ctypes.WinDLL("user32").SetProcessDPIAware()
    except Exception:
        pass

user32 = ctypes.WinDLL("user32", use_last_error=True)
k32 = ctypes.WinDLL("kernel32", use_last_error=True)


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


def client_size(hwnd):
    r = ctypes.wintypes.RECT()
    user32.GetClientRect(hwnd, ctypes.byref(r))
    return (r.right - r.left, r.bottom - r.top)


def to_screen(hwnd, x, y):
    pt = ctypes.wintypes.POINT(int(x), int(y))
    user32.ClientToScreen(hwnd, ctypes.byref(pt))
    return (pt.x, pt.y)


def hover(hwnd, x, y):
    """把系统光标放到客户区 (x, y) 上,并让窗口拿到前台。"""
    user32.SetForegroundWindow(hwnd)
    sx, sy = to_screen(hwnd, x, y)
    user32.SetCursorPos(sx, sy)
    time.sleep(0.15)


def wheel(notches):
    """正数 = 滚轮向上,负数 = 向下(和 WM_MOUSEWHEEL 一致,一格 120)。"""
    for _ in range(abs(notches)):
        delta = 120 if notches > 0 else -120
        user32.mouse_event(0x0800, 0, 0, ctypes.c_ulong(delta & 0xFFFFFFFF), 0)
        time.sleep(0.08)


def run(name, notches, hover_at=(120, 300), dump_at=3000):
    OUT.mkdir(parents=True, exist_ok=True)
    bmp, png, cfg, log = (OUT / f"{name}.bmp", OUT / f"{name}.png",
                          OUT / f"{name}.lua", OUT / f"{name}.bin")
    for p in (bmp, png, cfg, log):
        if p.exists():
            p.unlink()
    cfg.write_text(CFG.replace("__LOG__", log.as_posix()), encoding="utf-8")

    env = dict(os.environ)
    for k in list(env):
        if k.startswith("VECTORTERM_"):
            env.pop(k, None)
    env["VECTORTERM_AUTO_FILES"] = "side@1200"
    env["VECTORTERM_DUMP_AT"] = str(dump_at)
    env["VECTORTERM_DUMP_FRAME"] = str(bmp)

    t0 = time.time()
    proc = subprocess.Popen([str(EXE), "--config", str(cfg), "--cwd", str(ROOT / "src")],
                            env=env, stdout=subprocess.DEVNULL,
                            stderr=open(OUT / f"{name}.trace.log", "wb"))

    def wait(t):
        d = t0 + t - time.time()
        if d > 0:
            time.sleep(d)

    wait(1.6)
    hwnd = window_of_pid(proc.pid)
    if not hwnd:
        proc.kill()
        sys.exit("没找到窗口")
    hover(hwnd, *hover_at)
    fg = user32.GetForegroundWindow()
    print(f"    [{name}] 前台窗口 {'是' if fg == hwnd else '否'}  "
          f"客户区 {client_size(hwnd)}", flush=True)
    # 滚轮事件要比导帧早一点发,留出重绘时间
    if notches:
        wheel(notches)
    wait(dump_at / 1000.0 + 1.0)
    proc.terminate()
    try:
        proc.wait(timeout=8)
    except subprocess.TimeoutExpired:
        proc.kill()

    if bmp.exists():
        Image.open(bmp).save(png)
        return png
    return None


def thumb_y(png, panel_w=300):
    """在面板右侧找滚动条滑块(3px 宽、半透明的圆角条),返回它的纵向中心。"""
    im = Image.open(png).convert("RGB")
    px = im.load()
    best = None
    for x in range(panel_w // 2, panel_w):
        col = [px[x, y] for y in range(im.size[1])]
        # 滑块比面板底色稍亮,且是一段连续的窄条
        runs, start = [], None
        for y, c in enumerate(col):
            bright = sum(c) > 60 and sum(c) < 400
            if bright and start is None:
                start = y
            elif not bright and start is not None:
                if y - start > 10:
                    runs.append((start, y - 1))
                start = None
        if len(runs) == 1:
            a, b = runs[0]
            best = (x, (a + b) / 2, b - a + 1)
            break
    return best


def main():
    if not EXE.exists():
        sys.exit(f"找不到 {EXE},先 cargo build --release")
    print("列表内容 = src/ 目录(条目数 > 一屏),面板宽度 34 列", flush=True)
    base = run("base", 0)
    down = run("down", -3)
    up = run("up", +3)
    for tag, p in (("基准", base), ("滚轮向下 ×3", down), ("滚轮向上 ×3", up)):
        if not p:
            print(f"  {tag}: 没导出帧")
            continue
        t = thumb_y(p)
        print(f"  {tag:<12} 滚动条滑块 (x={t[0]}, 中心y={t[1]:.0f}, 高{t[2]})" if t
              else f"  {tag:<12} 没找到滑块", flush=True)


if __name__ == "__main__":
    main()
