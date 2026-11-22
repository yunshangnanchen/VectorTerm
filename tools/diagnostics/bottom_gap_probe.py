"""终端内容底部留白过大:一键量化探针。

症状:输入行离窗口下边框的空隙明显大于 `padding`,而且往终端里灌满输出、
让画面滚起来之后这个空隙**不会缩小**;用鼠标拖动下边框改一次尺寸,空隙
立刻变正常。

这里不猜,直接把几种状态各导一帧 BMP,量出"最后一行文字下沿 → 客户区
底边"的像素距离:

  a_fresh        空屏(只有提示符)
  b_filled       连续 ls 十次,画面已滚满
  c_filled_move  同 b,但在导帧前把下边框往下拖 30px(超过一行)
  d_empty_move   空屏 + 拖动一次(对照组:没有历史时抬不起来)
  e/f            真实 `config/deepseek.lua` 下的空屏 / 输满

用法:`python tools/diagnostics/bottom_gap_probe.py`
产物:`target/_bottom_gap/*.png`,以及每个用例的 `.trace.log`(程序 stderr)。
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
OUT = ROOT / "target" / "_bottom_gap"

# 照抄 deepseek.lua 里所有影响"可用高度"的键:字体度量、留白、标签栏、窗口尺寸。
# shell 换成 -NoProfile,把 oh-my-posh / fastfetch 的干扰去掉,输出更好量。
CFG = """return {
  shell = "pwsh.exe",
  shell_args = { "-NoLogo", "-NoProfile", "-NoExit" },
  render = "cpu",
  font = { size = 14, line_height = 1, cell_width = 1 },
  padding = 10,
  window_decorations = "RESIZE",
  window_rounded_corners = true,
  window_width = 1250,
  window_height = 750,
  tab_bar = {
    enable = true, hide_if_one = false, at_bottom = false,
    max_width = 20, show_close_button = true, corner_radius = 6,
    fancy = true, show_new_tab_button = true,
    top_padding = 5, edge_padding = 5,
  },
  status_bar = { item_spacing = 0, font_scale = 0.9, space_width = 0.2 },
  session_log = "__LOG__",
}
"""

# 告诉 Windows 这个脚本按物理像素算,别把坐标按 96 DPI 虚拟化
# (默认 125% 缩放下 GetClientRect 会返回 1250/1.25 = 1000,量出来的边距会偏)。
try:
    ctypes.WinDLL("shcore").SetProcessDpiAwareness(2)
except Exception:
    try:
        ctypes.WinDLL("user32").SetProcessDPIAware()
    except Exception:
        pass

user32 = ctypes.WinDLL("user32", use_last_error=True)
SWP_NOZORDER = 0x0004
SWP_NOACTIVATE = 0x0010
SWP_ASYNCWINDOWPOS = 0x4000


def window_of_pid(pid):
    found = []

    @ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)
    def enum_proc(hwnd, _lparam):
        owner = ctypes.c_ulong()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid and user32.IsWindowVisible(hwnd):
            found.append(hwnd)
            return False
        return True

    user32.EnumWindows(enum_proc, 0)
    return found[0] if found else None


def client_size(hwnd):
    r = ctypes.wintypes.RECT()
    user32.GetClientRect(hwnd, ctypes.byref(r))
    return (r.right - r.left, r.bottom - r.top)


def grow_bottom(hwnd, delta):
    """把窗口外框高度加大 delta(等价于用鼠标把下边框往下拖)。

    用 `SWP_ASYNCWINDOWPOS`:窗口线程忙的时候也不会卡住调用方 ——
    同步版本在对方消息泵不转时会一直等下去。
    """
    r = ctypes.wintypes.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(r))
    user32.SetWindowPos(hwnd, 0, r.left, r.top, r.right - r.left,
                        r.bottom - r.top + delta,
                        SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS)


def wait_until(t0, secs):
    remain = secs - (time.time() - t0)
    if remain > 0:
        time.sleep(remain)


def measure(png):
    """量出最后一行文字的下沿,以及它到图片底边的距离。"""
    im = Image.open(png).convert("RGB")
    w, h = im.size
    px = im.load()

    # 背景色取左下角一小块里出现最多的颜色(避开文字与滚动条)
    tally = {}
    for y in range(h - 12, h - 2):
        for x in range(6, 26):
            c = px[x, y]
            tally[c] = tally.get(c, 0) + 1
    bg = max(tally, key=tally.get)

    def differs(c):
        return sum(abs(c[i] - bg[i]) for i in range(3)) > 60

    # 右侧留 40px 给滚动条,左侧留 30px 给圆角/边框
    x0, x1 = 30, max(31, w - 40)

    def row_has_ink(y):
        return sum(1 for x in range(x0, x1) if differs(px[x, y])) > 2

    last = None
    for y in range(h - 1, 0, -1):
        if row_has_ink(y):
            last = y
            break

    first = None
    for y in range(0, h):
        if row_has_ink(y):
            first = y
            break

    return {
        "size": (w, h),
        "first_ink": first,
        "last_ink": last,
        "gap": (h - 1 - last) if last is not None else None,
    }


def run(name, cfg_text, autotype="", move_at=None, dump_at=3000):
    OUT.mkdir(parents=True, exist_ok=True)
    log = OUT / f"{name}.bin"
    bmp = OUT / f"{name}.bmp"
    png = OUT / f"{name}.png"
    cfg = OUT / f"{name}.lua"
    trace = OUT / f"{name}.trace.log"
    for p in (log, bmp, png, trace):
        if p.exists():
            p.unlink()

    cfg.write_text(cfg_text.replace("__LOG__", log.as_posix(), 1), encoding="utf-8")

    env = dict(os.environ)
    for k in list(env):
        if k.startswith("VECTORTERM_AUTO") or k in (
            "VECTORTERM_DUMP_AT", "VECTORTERM_DUMP_FRAME", "VECTORTERM_TRACE_RESIZE",
        ):
            env.pop(k, None)
    if autotype:
        env["VECTORTERM_AUTOTYPE"] = autotype
    env["VECTORTERM_TRACE_RESIZE"] = "1"
    env["VECTORTERM_DUMP_AT"] = str(dump_at)
    env["VECTORTERM_DUMP_FRAME"] = str(bmp)

    # stderr 一定要落文件:TRACE_RESIZE 每帧都写,接管道会灌满 64KB 缓冲,
    # 子进程卡在 write() 上,整个事件循环停转(导帧和自动输入都跟着失效)。
    t0 = time.time()
    with open(trace, "wb") as errf:
        proc = subprocess.Popen([str(EXE), "--config", str(cfg)], env=env,
                                stdout=subprocess.DEVNULL, stderr=errf)
        wait_until(t0, 1.0)
        hwnd = window_of_pid(proc.pid)
        before = client_size(hwnd) if hwnd else None
        after = before
        if move_at and hwnd:
            wait_until(t0, move_at / 1000.0)
            grow_bottom(hwnd, 30)
            wait_until(t0, move_at / 1000.0 + 0.35)
            after = client_size(hwnd)
        wait_until(t0, dump_at / 1000.0 + 0.8)
        proc.terminate()
        try:
            proc.wait(timeout=8)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=5)

    m = None
    if bmp.exists():
        Image.open(bmp).save(png)
        m = measure(png)

    grids = [ln.strip() for ln in trace.read_text("utf-8", "replace").splitlines()
             if "GRID" in ln or "网格同步" in ln or "profile" in ln]
    print(f"[{name}] 客户区 {before}->{after}  帧={m['size'] if m else '无'}  "
          f"首行y={m['first_ink'] if m else '-'}  末行y={m['last_ink'] if m else '-'}  "
          f"下边距={m['gap'] if m else '-'}px", flush=True)
    for ln in grids[-4:]:
        print("        " + ln, flush=True)
    return m


def main():
    if not EXE.exists():
        sys.exit(f"找不到 {EXE},先 cargo build --release")

    ls10 = "ls\r" * 10
    deepseek = (ROOT / "config" / "deepseek.lua").read_text(encoding="utf-8")

    print("=" * 78)
    print("几何预设(照抄 deepseek.lua):1250x750, font 14 / line_height 1, padding 10")
    print("=" * 78, flush=True)
    a = run("a_fresh", CFG, "", None, 2600)
    b = run("b_filled", CFG, ls10, None, 5200)
    c = run("c_filled_move", CFG, ls10, 3600, 6000)
    d = run("d_empty_move", CFG, "", 2400, 3600)

    print("=" * 78)
    print("真实 config/deepseek.lua")
    print("=" * 78, flush=True)
    e = run("e_ds_fresh", deepseek, "", None, 3600)
    f = run("f_ds_filled", deepseek, ls10, None, 6000)

    print("=" * 78)
    print("下边距对照(单位 px)")
    for tag, m in (("空屏", a), ("输满", b), ("输满+拖动", c), ("空屏+拖动", d),
                   ("deepseek 空屏", e), ("deepseek 输满", f)):
        if m:
            print(f"  {tag:<16}{m['gap']:>5}     (客户区高 {m['size'][1]}, 末行 y={m['last_ink']})")


if __name__ == "__main__":
    main()
