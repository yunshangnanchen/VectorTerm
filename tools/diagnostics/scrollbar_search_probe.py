"""实机验证:可视化滚动条 + 历史搜索 + 拖选边缘自动滚屏。

流程(全程用真实窗口 / 真实按键):
  1. cmd 里跑 for /L 吐 160 行带 NEEDLE 的日志;
  2. 贴底截一帧 -> 洋红滑块应在轨道最下方;
  3. Shift+PageUp 回看 -> 滑块上移;
  4. Ctrl+Shift+F 打开搜索栏并敲 "needle" -> 绿色匹配高亮出现;
  5. Enter 跳到下一处匹配 -> 视口位置明显变化。

颜色判据(配置里写死,避免和终端文字混淆):
  - scrollbar.thumb_color = #ff00ff(洋红),opacity = 1.0
  - search.highlight_color = #00ff00(绿),current_color = #ff8f3f(橙)

用法: python target/scrollbar_search_probe.py [debug|release]
"""
import ctypes
import ctypes.wintypes as wt
import os
import subprocess
import sys
import time
from pathlib import Path

from PIL import ImageGrab

ROOT = Path(__file__).resolve().parent.parent.parent
MODE = sys.argv[1] if len(sys.argv) > 1 else "debug"
EXE = ROOT / "target" / MODE / "VectorTerm.exe"
OUT = ROOT / "target"
WORK = OUT / "scrollbar_search_probe"
WORK.mkdir(parents=True, exist_ok=True)

CFG = """return {
  initial_cols = 100,
  initial_rows = 28,
  shell = "cmd.exe",
  colors = {
    foreground = "#cccccc", background = "#0c0c0c",
    cursor_bg = "#ffffff", cursor_fg = "#0c0c0c",
  },
  scrollback_lines = 3500,
  scrollbar = {
    enabled = true, width = 12, min_thumb = 28, auto_hide = false,
    opacity = 1.0, track_color = "#202020", thumb_color = "#ff00ff",
  },
  search = {
    enabled = true, case_sensitive = false,
    highlight_color = "#00ff00", current_color = "#ff8f3f",
    max_matches = 5000,
  },
  selection = { edge_scroll = true },
}
"""
cfg_path = WORK / "probe.lua"
cfg_path.write_text(CFG, encoding="utf-8")

u32 = ctypes.WinDLL("user32", use_last_error=True)
# 125% 缩放下不设 DPI 感知,GetWindowRect/ImageGrab 会被虚拟化,位置全错
try:
    ctypes.WinDLL("user32").SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
except Exception as exc:  # pragma: no cover
    print("DPI 感知设置失败(继续):", exc)

u32.GetWindowThreadProcessId.argtypes = [wt.HWND, ctypes.POINTER(wt.DWORD)]
u32.SetForegroundWindow.argtypes = [wt.HWND]
u32.SetWindowPos.argtypes = [wt.HWND, wt.HWND, ctypes.c_int, ctypes.c_int,
                             ctypes.c_int, ctypes.c_int, ctypes.c_uint]
u32.GetWindowRect.argtypes = [wt.HWND, ctypes.POINTER(wt.RECT)]
u32.keybd_event.argtypes = [ctypes.c_ubyte, ctypes.c_ubyte, wt.DWORD, ctypes.c_void_p]
u32.SetCursorPos.argtypes = [ctypes.c_int, ctypes.c_int]
u32.mouse_event.argtypes = [wt.DWORD, wt.DWORD, wt.DWORD, wt.DWORD, ctypes.c_void_p]

HWND_TOPMOST = -1
SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW = 0x0002, 0x0001, 0x0040
KEYUP = 0x0002
LEFTDOWN, LEFTUP = 0x0002, 0x0004
LOG = Path(os.path.expanduser("~/.config/VectorTerm/VectorTerm.log"))


class KEYBDINPUT(ctypes.Structure):
    _fields_ = [("wVk", wt.WORD), ("wScan", wt.WORD), ("dwFlags", wt.DWORD),
                ("time", wt.DWORD), ("dwExtraInfo", ctypes.POINTER(wt.ULONG))]


class INPUTUNION(ctypes.Union):
    _fields_ = [("ki", KEYBDINPUT), ("padding", ctypes.c_byte * 32)]


class INPUT(ctypes.Structure):
    _fields_ = [("type", wt.DWORD), ("u", INPUTUNION)]


u32.SendInput.argtypes = [wt.UINT, ctypes.POINTER(INPUT), ctypes.c_int]
u32.SendInput.restype = wt.UINT
KEYEVENTF_KEYUP, KEYEVENTF_UNICODE = 0x0002, 0x0004


def type_char(ch):
    """keybd_event 用 VK 码在部分键盘布局下会变成 Named(Process)(没有 text),
    这里直接注入 Unicode 字符,等同于用户真的敲了那个字。"""
    if ACTIVE:
        u32.SetForegroundWindow(ACTIVE)
    for flags in (KEYEVENTF_UNICODE, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP):
        inp = INPUT(type=1, u=INPUTUNION(ki=KEYBDINPUT(0, ord(ch), flags, 0, None)))
        u32.SendInput(1, ctypes.byref(inp), ctypes.sizeof(INPUT))
        time.sleep(0.03)
VK_CONTROL, VK_SHIFT, VK_F, VK_PRIOR, VK_RETURN = 0x11, 0x10, 0x46, 0x21, 0x0D


def find_window(pid):
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
                found.append((w * h, hwnd))
        return True

    u32.EnumWindows(cb, 0)
    return max(found)[1] if found else None


ACTIVE = None


def press(*vks):
    # 拖过鼠标之后前台窗口有可能被系统换掉,每次按键前重新拉回来
    if ACTIVE:
        u32.SetForegroundWindow(ACTIVE)
        time.sleep(0.05)
    for vk in vks:
        u32.keybd_event(vk, 0, 0, 0)
        time.sleep(0.03)
    time.sleep(0.08)
    for vk in reversed(vks):
        u32.keybd_event(vk, 0, KEYUP, 0)
        time.sleep(0.03)


def shot(name, box):
    im = ImageGrab.grab(bbox=(box.left, box.top, box.right, box.bottom)).convert("RGB")
    path = OUT / f"sbsearch_{name}.png"
    im.save(path)
    return im


def stats(im):
    """(洋红像素数, 滑块 y 范围, 绿色高亮数, 橙色数, 洋红 x 范围)"""
    px = im.load()
    w, h = im.size
    magenta, green, orange = 0, 0, 0
    ys, xs = [], []
    for y in range(0, h, 2):
        for x in range(0, w, 2):
            r, g, b = px[x, y]
            if r > 130 and b > 130 and g < 90:
                magenta += 1
                ys.append(y)
                xs.append(x)
            elif g > 55 and g > r + 30 and g > b + 30:
                green += 1
            elif r > 110 and g > 50 and b < 80 and r > g + 40:
                orange += 1
    yspan = (min(ys), max(ys)) if ys else None
    xspan = (min(xs), max(xs)) if xs else None
    return magenta, yspan, green, orange, xspan


def drag(box, x, y0, y1, hold=0.0):
    """屏幕物理坐标拖动左键(分 10 步,保证 winit 收到 CursorMoved)。"""
    u32.SetCursorPos(box.left + int(x), box.top + int(y0))
    u32.mouse_event(LEFTDOWN, 0, 0, 0, None)
    for i in range(1, 11):
        u32.SetCursorPos(box.left + int(x), box.top + int(y0 + (y1 - y0) * i / 10))
        time.sleep(0.03)
    time.sleep(hold)
    u32.mouse_event(LEFTUP, 0, 0, 0, None)


def main():
    global ACTIVE
    env = dict(os.environ)
    env["VECTORTERM_AUTOTYPE"] = r"for /L %i in (1,1,160) do @echo NEEDLE_%i_abcdefghijklmnop\r"
    env.pop("VECTORTERM_AUTO_SPLIT", None)
    env.pop("VECTORTERM_DUMP_FRAME", None)
    env["VECTORTERM_DEBUG_KEYS"] = "1"
    log_off = LOG.stat().st_size if LOG.exists() else 0
    p = subprocess.Popen(
        [str(EXE), "--config", str(cfg_path)], cwd=str(ROOT), env=env,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    hwnd = None
    for _ in range(40):
        time.sleep(0.25)
        hwnd = find_window(p.pid)
        if hwnd:
            break
    if not hwnd:
        print("FAIL: 找不到窗口")
        p.terminate()
        return 1
    ACTIVE = hwnd
    u32.SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0,
                     SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW)
    u32.SetForegroundWindow(hwnd)
    box = wt.RECT()
    u32.GetWindowRect(hwnd, ctypes.byref(box))
    # SetForegroundWindow 可能被系统拒绝;点一下标题栏确保键盘焦点真在窗口上
    u32.SetCursorPos(box.left + 140, box.top + 12)
    u32.mouse_event(LEFTDOWN, 0, 0, 0, None)
    u32.mouse_event(LEFTUP, 0, 0, 0, None)
    time.sleep(0.3)
    u32.SetForegroundWindow(hwnd)
    time.sleep(5.0)  # 等 AUTOTYPE 执行完 + 输出稳定

    bottom = stats(shot("bottom", box))
    print("贴底:", bottom)

    # 拖动滑块从贴底位置往上:既是回看,也验证鼠标拖动这条交互路径
    x_mid = sum(bottom[4]) / 2 if bottom[4] else 900
    y_mid = sum(bottom[1]) / 2 if bottom[1] else 670
    drag(box, x_mid, y_mid, y_mid - 260)
    time.sleep(0.5)
    scrolled = stats(shot("scrolled", box))
    print("拖动滑块后:", scrolled)

    # 拖选到内容区上边缘并停住(别落到标签栏上,那里是窗口拖动区):
    # edge_scroll 应该继续把视口往上推
    drag(box, (box.right - box.left) * 0.35, 320, 70, hold=0.9)
    time.sleep(0.4)
    edge = stats(shot("edge_scroll", box))
    print("边缘自动滚屏后:", edge)

    press(VK_CONTROL, VK_SHIFT, VK_F)
    time.sleep(0.3)
    for ch in "needle":
        type_char(ch)
        time.sleep(0.05)
    time.sleep(0.8)
    search = stats(shot("search", box))
    print("搜索中:", search)

    # 边缘滚屏已经把视口推到历史顶部了;先把滑块拖回贴底,这样"回车跳到
    # 匹配行"必然带来可观测的位移(顺便验证搜索栏开着时滚动条仍能拖)。
    drag(box, x_mid, sum(search[1]) / 2, 690)
    time.sleep(0.4)
    before_enter = stats(shot("before_enter", box))
    print("回车前(拖回贴底):", before_enter)
    press(VK_RETURN)
    time.sleep(0.6)
    jump = stats(shot("jump", box))
    print("回车跳转后:", jump)

    checks = [
        ("贴底有洋红滑块", bottom[0] > 80),
        ("轨道上滑块贴底", bottom[1] is not None and bottom[1][1] > box.bottom - box.top - 90),
        ("拖动后滑块上移", scrolled[1] is not None and bottom[1] is not None
         and scrolled[1][0] < bottom[1][0] - 40),
        ("边缘拖选继续上滚", edge[1] is not None and scrolled[1] is not None
         and edge[1][0] < scrolled[1][0] - 6),
        ("搜索后仍有滑块", search[0] > 80),
        ("匹配高亮出现", search[2] > 60),
        ("当前匹配高亮出现", search[3] > 20),
        ("拖回贴底生效", before_enter[1] is not None and before_enter[1][0] > 500),
        ("回车跳到匹配行", jump[1] is not None and before_enter[1] is not None
         and jump[1][0] < before_enter[1][0] - 60),
    ]
    for name, ok in checks:
        print(f"  [{'OK' if ok else 'FAIL'}] {name}")
    if LOG.exists():
        with LOG.open("rb") as f:
            f.seek(log_off)
            tail = f.read().decode("utf-8", "replace").splitlines()
        keys = [ln.strip() for ln in tail if "[dbg] key Pressed" in ln]
        print("--- 收到的按键(最后 12 条) ---")
        for ln in keys[-12:]:
            print("   ", ln[-170:])
        if not keys:
            print("    (一条都没有:窗口没拿到键盘焦点)")
    p.terminate()
    try:
        p.wait(timeout=5)
    except subprocess.TimeoutExpired:
        p.kill()
    return 0 if all(ok for _, ok in checks) else 1


if __name__ == "__main__":
    sys.exit(main())
