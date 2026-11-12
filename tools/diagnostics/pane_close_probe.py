"""实机验证:`Ctrl+Shift+X` 关窗格(以及"有程序在跑时先问一句")。

三个场景:

  A. 空 shell 分屏 → 关窗格:cmd.exe 在白名单里,应该**不问**,直接关。
     判据是中间那根竖分隔线消失(默认色 #323236 = (50,50,54),全高一条)。
  B. 分屏后在新窗格里跑 `ping -t` → 关窗格:应弹确认,窗格与线都还在。
     判定照 wezterm `CloseCurrentPane { confirm = true }` —— 只查**这个**
     窗格自己的进程树,同标签里别的窗格在跑什么不影响。
  C. 接 B 按 `y`:窗格关掉,竖线消失,窗口还在(标签里还剩一个窗格)。

为什么不注入按键:窗口没有键盘焦点时 `SendInput` 会被静默丢弃(见
`window_close_probe.py` 的说明)。这里走同一个内部入口 —— `VECTORTERM_AUTO_TAB`
的 `closepane` 调的就是 `close_pane_or_ask`,和 `Ctrl+Shift+X` 完全同一条路。
帧也用程序自导出(`VECTORTERM_DUMP_FRAME`),避免被别的窗口遮挡。

用法: python tools/diagnostics/pane_close_probe.py [debug|release]
"""
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
MODE = sys.argv[1] if len(sys.argv) > 1 else "debug"
EXE = ROOT / "target" / MODE / "VectorTerm.exe"
OUT = ROOT / "target" / "pane_probe"
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

DIVIDER = (50, 50, 54)


def divider_px(paths, tol=8):
    """中间那根竖分隔线占了多长。

    分屏后是一条近全高的纯色竖线(默认 #323236),关掉窗格后这条线消失。
    只扫中间 35%~65% 的列,避免把两侧终端内容算进来。
    """
    from PIL import Image

    path = next((q for q in paths if q.exists()), None)
    if path is None:
        return 0
    im = Image.open(path).convert("RGB")
    w, h = im.size
    px = im.load()
    best = 0
    for x in range(int(w * 0.35), int(w * 0.65)):
        n = 0
        for y in range(int(h * 0.15), int(h * 0.85), 2):
            r, g, b = px[x, y]
            if abs(r - DIVIDER[0]) + abs(g - DIVIDER[1]) + abs(b - DIVIDER[2]) <= tol * 3:
                n += 1
        best = max(best, n)
    return best


def mid_ink(paths):
    """内容区中部的非底色像素:确认框居中,终端输出贴顶。"""
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


def spawn(extra_env, dump_at, tag):
    off = LOG.stat().st_size if LOG.exists() else 0
    env = dict(os.environ)
    for k in list(env):
        if k.startswith("VECTORTERM_"):
            env.pop(k)
    env.update(extra_env)
    frame = OUT / f"{tag}.bmp"
    env["VECTORTERM_DUMP_FRAME"] = str(frame)
    env["VECTORTERM_DUMP_AT"] = dump_at
    p = subprocess.Popen([str(EXE), "--config", str(CFG_PATH)], cwd=str(ROOT), env=env,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    # `VECTORTERM_DUMP_AT` 只给一个时刻时不加 -<毫秒> 后缀,给一串才加
    times = [t.strip() for t in dump_at.split(",")]
    frames = [frame] if len(times) == 1 else [OUT / f"{tag}-{t}.bmp" for t in times]
    return p, off, frames


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

# ---- A:空 shell,关窗格不该问 ----
p, off, frames = spawn({"VECTORTERM_AUTO_SPLIT": "v", "VECTORTERM_AUTO_TAB": "closepane@3000"},
                       "2400,3600", "a")
time.sleep(4.2)
alive_a = p.poll() is None
before_a = divider_px([frames[0]])
after_a = divider_px([frames[1]])
asked_a = "已弹出关闭确认" in log_since(off)
finish(p)
print(f"[A] 空 shell 关窗格: 分屏后线长={before_a} 关后线长={after_a} "
      f"弹框={asked_a} 窗口存活={alive_a}")
if not (before_a > 100 and after_a < 30 and not asked_a and alive_a):
    print("    !! 空 shell 关窗格应该直接关(白名单里的 cmd.exe 不该触发确认)")
    ok = False

# ---- B:窗格里跑着 ping,关窗格要先问 ----
# 注意确认框是"清屏 + 居中提示"(和 wezterm 一样),弹框期间分隔线也被盖掉,
# 所以"线还在不在"要看**按 n 之后**那一帧,不能用弹框那一帧。
p, off, frames = spawn({
    "VECTORTERM_AUTO_SPLIT": "v",
    "VECTORTERM_AUTOTYPE": "ping -t 127.0.0.1\\r",
    "VECTORTERM_AUTO_TAB": "closepane@4200,no@5000",
}, "4800,6200", "b")
time.sleep(6.8)
alive_b = p.poll() is None
ink_b = mid_ink([frames[0]])
line_b = divider_px([frames[1]])
asked_b = "已弹出关闭确认" in log_since(off)
finish(p)
print(f"[B] ping 中关窗格: 有框={ink_b} 按 n 后线长={line_b} 弹框={asked_b} 窗口存活={alive_b}")
if not (alive_b and asked_b and ink_b > 200 and line_b > 100):
    print("    !! 有程序在跑时关窗格没有弹确认,或者按 n 之后窗格没了")
    ok = False

# ---- C:确认后按 y,窗格真的关掉 ----
p, off, frames = spawn({
    "VECTORTERM_AUTO_SPLIT": "v",
    "VECTORTERM_AUTOTYPE": "ping -t 127.0.0.1\\r",
    "VECTORTERM_AUTO_TAB": "closepane@4200,yes@5000",
}, "4800,6200", "c")
time.sleep(6.8)
alive_c = p.poll() is None
ink_c = mid_ink([frames[0]])
line_c = divider_px([frames[1]])
asked_c = "已弹出关闭确认" in log_since(off)
finish(p)
print(f"[C] 按 y 之后: 有框={ink_c} 线长={line_c} 弹框={asked_c} 窗口存活={alive_c}")
if not (line_c < 30 and alive_c and asked_c and ink_c > 200):
    print("    !! 按 y 之后窗格没关掉,或者没问就关了,或者连带把窗口也关了")
    ok = False

# ---- D:只剩一个窗格时,快捷键等同关标签(有程序则走关窗口那句文案) ----
p, off, frames = spawn({
    "VECTORTERM_AUTOTYPE": "ping -t 127.0.0.1\\r",
    "VECTORTERM_AUTO_TAB": "closepane@4000",
}, "4800", "d")
time.sleep(5.4)
alive_d = p.poll() is None
ink_d = mid_ink([frames[0]])
# 单标签时 close_tab_or_ask → close_window_or_ask,日志是"退出确认"
asked_d = "已弹出退出确认" in log_since(off)
finish(p)
print(f"[D] 单窗格 + ping: 有框={ink_d} 走关窗口文案={asked_d} 窗口存活={alive_d}")
if not (alive_d and asked_d and ink_d > 200):
    print("    !! 只剩一个窗格时不该绕过确认直接退出")
    ok = False

print("\n涉及的日志:")
for line in (log_since(0)).splitlines():
    if "窗格" in line and "确认" in line:
        print("  ", line.strip())

print("\n结果:", "PASS" if ok else "FAIL")
sys.exit(0 if ok else 1)
