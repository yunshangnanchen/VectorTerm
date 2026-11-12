"""实机验证:关**窗口**前的确认(用户报的第二个 bug)。

起因:
  在终端里跑着一个 exe,点标题栏的 ✕ / 按 Alt+F4 / 关掉唯一的那个标签,
  窗口直接没了,一句提示都没有。

两个原因,都是"漏了入口"而不是"判据算错":
  1. `tabbar::WindowButton::Close`(自绘标题栏的 ✕)直接调 `request_quit()`,
     压根没过 `ask_quit`;
  2. 只剩一个标签时,`ask_close_tab` 一上来就 `background.len() <= 1` 返回
     false,于是 `close_tab` → `request_quit` 一路直通,也没确认。

判定本身也照 wezterm 重写了(`LocalPane::can_close_without_prompting`):
从 shell 取整棵进程树的 exe 名,减去 `skip_close_confirmation_for_processes_named`,
剩东西才弹。这里顺带验证白名单的口径 —— 跑着 `ping` 要问,空 shell 不问。

三个场景:
  A. 跑着 `ping -t`,发 `WM_CLOSE`(等价 Alt+F4)→ 应弹确认,窗口不关;
  B. 跑着 `ping -t`,走 `VECTORTERM_AUTO_TAB=close@3000`(等价点唯一的标签的 ×)
     → 应弹确认,文案是"关窗口"那句;
  C. 空 shell(`cmd /k`,没程序)→ `WM_CLOSE` → 直接关掉,不弹。

为什么不用 `SendInput` 点 ✕:窗口没有键盘焦点时键会被静默丢掉,而且这台机器
上自绘标题栏默认没开按钮(`window_decorations = "RESIZE"` 不画按钮)。所以走
`WM_CLOSE` + 内部入口这两条等价的路。截图用程序自导出帧(`VECTORTERM_DUMP_FRAME`),
不用 PrintWindow —— 窗口被别的窗口压着时拍到的是遮挡物。

用法: python tools/diagnostics/window_close_probe.py [debug|release]
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
OUT = ROOT / "target" / "close_probe"
OUT.mkdir(parents=True, exist_ok=True)

CFG = """return {
  initial_cols = 100,
  initial_rows = 26,
  shell = "cmd.exe",
  shell_args = { "/k", "title probe" },
  status_bar = { enabled = false },
}
"""
CFG_PATH = OUT / "probe.lua"
CFG_PATH.write_text(CFG, encoding="utf-8")
LOG = Path(os.path.expanduser("~/.config/VectorTerm/VectorTerm.log"))

u32 = ctypes.WinDLL("user32", use_last_error=True)
u32.GetWindowThreadProcessId.argtypes = [wt.HWND, ctypes.POINTER(wt.DWORD)]


def find_window(pid):
    found = []

    @ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
    def cb(hwnd, _):
        wpid = wt.DWORD()
        u32.GetWindowThreadProcessId(hwnd, ctypes.byref(wpid))
        if wpid.value == pid and u32.IsWindowVisible(hwnd):
            r = wt.RECT()
            u32.GetWindowRect(hwnd, ctypes.byref(r))
            if r.right - r.left > 400 and r.bottom - r.top > 300:
                found.append(hwnd)
        return True

    u32.EnumWindows(cb, 0)
    return found[0] if found else None


def mid_ink(paths):
    """内容区中部的非底色像素。确认框居中、终端输出贴顶,这段最能分开两者。"""
    from PIL import Image
    path = next((q for q in paths if q.exists()), None)
    if path is None:
        return 0
    im = Image.open(path).convert("RGB")
    w, h = im.size
    px = im.load()
    bg = px[w - 4, h - 4]
    n = 0
    for y in range(int(h * .35), int(h * .66), 2):
        for x in range(int(w * .04), int(w * .96), 2):
            r, g, b = px[x, y]
            if abs(r - bg[0]) + abs(g - bg[1]) + abs(b - bg[2]) > 40:
                n += 1
    return n


def spawn(prog, extra_env, dump_at, tag):
    off = LOG.stat().st_size if LOG.exists() else 0
    env = dict(os.environ)
    for k in list(env):
        if k.startswith("VECTORTERM_"):
            env.pop(k)
    if prog:
        env["VECTORTERM_AUTOTYPE"] = prog + "\\r"
    env.update(extra_env)
    frame = OUT / f"{tag}.bmp"
    env["VECTORTERM_DUMP_FRAME"] = str(frame)
    env["VECTORTERM_DUMP_AT"] = str(dump_at)
    p = subprocess.Popen([str(EXE), "--config", str(CFG_PATH)], cwd=str(ROOT), env=env,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    # `VECTORTERM_DUMP_AT` 只给一个时刻时不加后缀,给一串才加 `-<毫秒>`
    return p, off, [frame, OUT / f"{tag}-{dump_at}.bmp"]


def finish(p):
    p.terminate()
    try:
        p.wait(timeout=3)
    except subprocess.TimeoutExpired:
        p.kill()


def log_since(off):
    if not LOG.exists():
        return ""
    with LOG.open("r", encoding="utf-8", errors="replace") as f:
        f.seek(off)
        return f.read()


ok = True

# ---- A:WM_CLOSE(Alt+F4 / 系统菜单关窗) ----
p, off, frame = spawn("ping -t 127.0.0.1", {}, 5200, "a")
time.sleep(1.2)
hwnd = find_window(p.pid)
if not hwnd:
    finish(p)
    sys.exit("A: 找不到窗口")
time.sleep(3.4)
u32.PostMessageW(hwnd, 0x0010, 0, 0)          # WM_CLOSE
time.sleep(1.4)
alive_a = p.poll() is None
ink_a = mid_ink(frame)
log_a = log_since(off)
asked_a = "已弹出退出确认" in log_a
finish(p)
print(f"[A] WM_CLOSE + ping: 窗口存活={alive_a} 墨迹={ink_a} 弹框={asked_a}")
if not (alive_a and asked_a and ink_a > 200):
    print("    !! 关窗口没有弹确认,或者框没画出来")
    ok = False

# ---- B:只剩一个标签时关标签(等价点标签栏的 ×) ----
p, off, frame = spawn("ping -t 127.0.0.1", {"VECTORTERM_AUTO_TAB": "close@3000"}, 4000, "b")
time.sleep(4.4)
alive_b = p.poll() is None
ink_b = mid_ink(frame)
log_b = log_since(off)
asked_b = "已弹出退出确认" in log_b
finish(p)
print(f"[B] 单选标签 × + ping: 窗口存活={alive_b} 墨迹={ink_b} 弹框={asked_b}")
if not (alive_b and asked_b and ink_b > 200):
    print("    !! 关最后一个标签时静默退出了")
    ok = False

# ---- C:空 shell → 不该问 ----
p, off, frame = spawn(None, {}, 3400, "c")
time.sleep(1.2)
hwnd = find_window(p.pid)
if not hwnd:
    finish(p)
    sys.exit("C: 找不到窗口")
time.sleep(2.2)
u32.PostMessageW(hwnd, 0x0010, 0, 0)
time.sleep(1.4)
gone_c = p.poll() is not None
asked_c = "已弹出退出确认" in log_since(off)
finish(p)
print(f"[C] WM_CLOSE + 空 shell: 已退出={gone_c} 弹框={asked_c}")
if not (gone_c and not asked_c):
    print("    !! 空 shell 不该弹确认(白名单没生效?)")
    ok = False

print("\n涉及的日志:")
for line in (log_a + log_b).splitlines():
    if "确认" in line:
        print("  ", line.strip())

print("\n结果:", "PASS" if ok else "FAIL")
sys.exit(0 if ok else 1)
