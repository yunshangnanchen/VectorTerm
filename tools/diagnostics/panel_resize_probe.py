"""实机回归:命令面板 / 分割栏拖动 / 快捷键调整窗格大小 / 滚动条外观。

用法: python target/panel_resize_probe.py [debug|release]

用 VECTORTERM_AUTO_SPLIT=v 自动左右分屏,分割线配成纯红方便按像素量位置。
依次验证:
  1. 鼠标拖分割线 → 红线明显左移,且另一侧没被压没
  2. Ctrl+Shift+Alt+Right → 红线按 step 右移
  3. Ctrl+Shift+Alt+Left  → 红线按 step 左移(双向都可调)
  4. Ctrl+Shift+P → 命令面板出现(橙色输入竖条 + 命令列表)
  5. 输入 key 回车 → 多行键位表输出视图;PgDn 能翻到后半段
  6. Ctrl+Shift+F 后输入 / → 从搜索框切到命令面板
  7. 输入 files 回车 → 铺满当前 pane 的文件管理器

方向键必须用扫描码 + KEYEVENTF_EXTENDEDKEY 注入:keybd_event(VK_RIGHT,
scan=0) 在 NumLock 打开时会被 Windows 翻译成小键盘 6,并伪造 Shift
up/down,keydown 直接丢失(日志里只剩 Released / physical=Code(Numpad6))。
另外用 VECTORTERM_DUMP_FRAME 单独 A/B 验证滚动条 show / side / show_track / colors。
"""
import ctypes
import ctypes.wintypes as wt
import os
import subprocess
import sys
import time
from pathlib import Path

from PIL import Image, ImageGrab

ROOT = Path(__file__).resolve().parent.parent.parent
MODE = sys.argv[1] if len(sys.argv) > 1 else "release"
EXE = ROOT / "target" / MODE / "VectorTerm.exe"
OUT = ROOT / "target"

CFG = """return {
  initial_cols = 100,
  initial_rows = 28,
  shell = "cmd.exe",
  colors = { foreground = "#cccccc", background = "#0c0c0c" },
  split_divider = { color = "#ff0000", thickness = 4, text_gap = 2 },
  scrollbar = {
    show = "always", side = "right", show_track = true,
    width = 12, opacity = 1.0,
    track_color = "#202020", thumb_color = "#ff00ff", hover_color = "#00ff00",
  },
}
"""

u32 = ctypes.WinDLL("user32", use_last_error=True)
try:
    ctypes.windll.shcore.SetProcessDpiAwareness(2)
except Exception:
    u32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
u32.GetWindowThreadProcessId.argtypes = [wt.HWND, ctypes.POINTER(wt.DWORD)]
u32.SetForegroundWindow.argtypes = [wt.HWND]
u32.SetWindowPos.argtypes = [wt.HWND, wt.HWND, ctypes.c_int, ctypes.c_int,
                             ctypes.c_int, ctypes.c_int, ctypes.c_uint]
u32.GetWindowRect.argtypes = [wt.HWND, ctypes.POINTER(wt.RECT)]
u32.SetCursorPos.argtypes = [ctypes.c_int, ctypes.c_int]
u32.mouse_event.argtypes = [wt.DWORD, wt.DWORD, wt.DWORD, wt.DWORD, ctypes.c_void_p]
u32.keybd_event.argtypes = [ctypes.c_ubyte, ctypes.c_ubyte, wt.DWORD, ctypes.c_void_p]

HWND_TOPMOST = -1
SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW = 0x0002, 0x0001, 0x0040
LEFTDOWN, LEFTUP = 0x0002, 0x0004
VK_CONTROL, VK_SHIFT, VK_MENU = 0x11, 0x10, 0x12
VK_P, VK_F, VK_ENTER, VK_ESC, VK_RIGHT, VK_LEFT = 0x50, 0x46, 0x0D, 0x1B, 0x27, 0x25
VK_PRIOR, VK_NEXT = 0x21, 0x22  # PageUp / PageDown
KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP = 0x0001, 0x0002
KEYEVENTF_UNICODE, KEYEVENTF_SCANCODE = 0x0004, 0x0008


class KEYBDINPUT(ctypes.Structure):
    _fields_ = [("wVk", wt.WORD), ("wScan", wt.WORD), ("dwFlags", wt.DWORD),
                ("time", wt.DWORD), ("dwExtraInfo", ctypes.POINTER(wt.ULONG))]


class INPUTUNION(ctypes.Union):
    # 必须撑到 32 字节(MOUSEINPUT 的大小),否则 sizeof(INPUT) 不是 40,
    # SendInput 会静默失败 —— 字符根本到不了窗口。
    _fields_ = [("ki", KEYBDINPUT), ("padding", ctypes.c_byte * 32)]


class INPUT(ctypes.Structure):
    _fields_ = [("type", wt.DWORD), ("u", INPUTUNION)]


u32.SendInput.argtypes = [wt.UINT, ctypes.POINTER(INPUT), ctypes.c_int]
u32.SendInput.restype = wt.UINT
u32.MapVirtualKeyW.argtypes = [wt.UINT, wt.UINT]
u32.MapVirtualKeyW.restype = wt.UINT

ACTIVE = None


def foreground():
    if ACTIVE:
        u32.SetWindowPos(ACTIVE, HWND_TOPMOST, 0, 0, 0, 0,
                         SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW)
        u32.SetForegroundWindow(ACTIVE)


def type_char(ch):
    """字符必须走 SendInput(KEYEVENTF_UNICODE),否则 text=None 命令框收不到。"""
    foreground()
    for flags in (KEYEVENTF_UNICODE, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP):
        inp = INPUT()
        inp.type = 1
        inp.u.ki.wScan = ord(ch)
        inp.u.ki.dwFlags = flags
        u32.SendInput(1, ctypes.byref(inp), ctypes.sizeof(INPUT))
        time.sleep(0.02)


def type_text(s):
    for c in s:
        type_char(c)
    time.sleep(0.12)


def combo(mods, vk):
    foreground()
    for m in mods:
        u32.keybd_event(m, 0, 0, None)
        time.sleep(0.03)
    time.sleep(0.10)
    u32.keybd_event(vk, 0, 0, None)
    time.sleep(0.05)
    u32.keybd_event(vk, 0, 2, None)
    for m in reversed(mods):
        u32.keybd_event(m, 0, 2, None)
        time.sleep(0.03)
    time.sleep(0.22)


def send_scan(vk, extended=False, up=False):
    """按扫描码注入一个键。方向键必须带 EXTENDEDKEY,否则会变成小键盘数字。"""
    inp = INPUT()
    inp.type = 1
    inp.u.ki.wVk = 0
    inp.u.ki.wScan = u32.MapVirtualKeyW(vk, 0) & 0xFF
    inp.u.ki.dwFlags = KEYEVENTF_SCANCODE
    if extended:
        inp.u.ki.dwFlags |= KEYEVENTF_EXTENDEDKEY
    if up:
        inp.u.ki.dwFlags |= KEYEVENTF_KEYUP
    u32.SendInput(1, ctypes.byref(inp), ctypes.sizeof(INPUT))


def combo_scan(mods, vk, extended=False):
    """整串组合键都走扫描码,避免 keybd_event 的 VK → 小键盘翻译。"""
    foreground()
    for m in mods:
        send_scan(m)
        time.sleep(0.03)
    time.sleep(0.12)
    send_scan(vk, extended)
    time.sleep(0.06)
    send_scan(vk, extended, up=True)
    for m in reversed(mods):
        send_scan(m, up=True)
        time.sleep(0.03)
    time.sleep(0.28)


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


def shot(name, rect):
    foreground()
    im = ImageGrab.grab(bbox=(rect.left, rect.top, rect.right, rect.bottom)).convert("RGB")
    im.save(OUT / f"panel_{name}.png")
    return im


def red_line_x(im):
    """贯穿大半个高度的纯红列的中点;找不到返回 None。"""
    px = im.load()
    w, h = im.size
    cols = [x for x in range(w)
            if sum(1 for y in range(int(h * 0.22), int(h * 0.85))
                   if px[x, y][0] > 180 and px[x, y][1] < 80 and px[x, y][2] < 80)
            > h * 0.35]
    return (min(cols) + max(cols) + 1) / 2 if cols else None


def orange_pixels(im):
    """命令面板的橙色输入竖条 / 选中项高亮。"""
    px = im.load()
    w, h = im.size
    return sum(1 for y in range(0, h, 2) for x in range(0, w, 2)
               if px[x, y][0] > 190 and 90 < px[x, y][1] < 190 and px[x, y][2] < 110)


def diff_pixels(a, b):
    """两张截图的明显变化量(每 3 像素采样),用来确认文件管理器真的铺开了。"""
    pa, pb = a.load(), b.load()
    w, h = a.size
    return sum(1 for y in range(0, h, 3) for x in range(0, w, 3)
               if max(abs(pa[x, y][i] - pb[x, y][i]) for i in range(3)) > 40)


def drag(rect, x0, y, x1, steps=12):
    foreground()
    u32.SetCursorPos(int(rect.left + x0), int(rect.top + y))
    time.sleep(0.1)
    u32.mouse_event(LEFTDOWN, 0, 0, 0, None)
    for i in range(1, steps + 1):
        xx = x0 + (x1 - x0) * i / steps
        u32.SetCursorPos(int(rect.left + xx), int(rect.top + y))
        time.sleep(0.025)
    time.sleep(0.1)
    u32.mouse_event(LEFTUP, 0, 0, 0, None)
    time.sleep(0.25)


def main():
    global ACTIVE
    cfg = OUT / "panel_resize_probe.lua"
    cfg.write_text(CFG, encoding="utf-8")
    env = dict(os.environ)
    env["VECTORTERM_AUTO_SPLIT"] = "v"
    env["VECTORTERM_DEBUG_KEYS"] = "1"
    LOG = Path(os.path.expanduser("~/.config/VectorTerm/VectorTerm.log"))
    log_off = LOG.stat().st_size if LOG.exists() else 0
    proc = subprocess.Popen([str(EXE), "--config", str(cfg)], cwd=str(ROOT), env=env,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        hwnd = None
        for _ in range(60):
            time.sleep(0.2)
            hwnd = find_window(proc.pid)
            if hwnd:
                break
        if not hwnd:
            print("FAIL: 找不到窗口")
            return 1
        ACTIVE = hwnd
        time.sleep(2.4)  # 自动分屏 + 动画
        rect = wt.RECT()
        u32.GetWindowRect(hwnd, ctypes.byref(rect))
        w, h = rect.right - rect.left, rect.bottom - rect.top
        print(f"window {w}x{h}")

        im0 = shot("00_split", rect)
        x0 = red_line_x(im0)
        print(f"初始红线 x={x0}")
        if x0 is None:
            print("FAIL: 没找到分割线")
            return 1

        # 1) 鼠标拖分割线
        y = h * 0.6
        drag(rect, x0, y, x0 - 160)
        im1 = shot("01_drag", rect)
        x1 = red_line_x(im1)
        print(f"拖动后红线 x={x1} (位移 {None if x1 is None else x1 - x0:.0f})")

        # 2) 快捷键右移一步(默认 step=0.05,窗口约 1220px → ~60px)
        combo_scan([VK_CONTROL, VK_SHIFT, VK_MENU], VK_RIGHT, extended=True)
        im2 = shot("02_keyresize", rect)
        x2 = red_line_x(im2)
        print(f"快捷键 → 后红线 x={x2} (位移 {None if x2 is None else x2 - x1:.0f})")

        # 3) 再按一次反向,确认双向都能调
        #    不要改测 Ctrl+Alt+方向键:它是显卡驱动的旋转屏幕全局热键,
        #    keydown 会被系统吃掉,窗口只能收到 Released。
        combo_scan([VK_CONTROL, VK_SHIFT, VK_MENU], VK_LEFT, extended=True)
        im2b = shot("02b_keyresize_left", rect)
        x2b = red_line_x(im2b)
        print(f"快捷键 ← 后红线 x={x2b} (位移 {None if x2b is None else x2b - x2:.0f})")

        # 4) 命令面板
        combo([VK_CONTROL, VK_SHIFT], VK_P)
        im3 = shot("03_panel", rect)
        o3 = orange_pixels(im3)
        print(f"命令面板橙色像素={o3}")

        # 5) key 输出视图
        type_text("key")
        u32.keybd_event(VK_ENTER, 0, 0, None)
        u32.keybd_event(VK_ENTER, 0, 2, None)
        time.sleep(0.3)
        im4 = shot("04_keys", rect)
        o4 = orange_pixels(im4)
        print(f"key 输出视图橙色像素={o4}")

        # 5b) 键位表 28 行 > 一屏:PgDn 必须能翻到后半段,不能只截断
        foreground()
        u32.keybd_event(VK_NEXT, 0, 0, None)
        u32.keybd_event(VK_NEXT, 0, 2, None)
        time.sleep(0.3)
        im4b = shot("04b_keys_paged", rect)
        d4 = diff_pixels(im4, im4b)
        print(f"键位表 PgDn 翻页后画面变化像素={d4}")

        # 6) 从搜索框输 / 切到命令面板。
        #    Esc 一次只清输出视图,第二次才真正关面板 —— 必须按两次,否则
        #    Ctrl+Shift+F 会落在仍是模态的命令面板上。
        for _ in range(2):
            u32.keybd_event(VK_ESC, 0, 0, None)
            u32.keybd_event(VK_ESC, 0, 2, None)
            time.sleep(0.15)
        combo([VK_CONTROL, VK_SHIFT], VK_F)
        type_char("/")
        time.sleep(0.25)
        im5 = shot("05_slash", rect)
        o5 = orange_pixels(im5)
        print(f"搜索框输 / 后橙色像素={o5}")

        # 7) files → 全屏文件管理器
        type_text("files")
        u32.keybd_event(VK_ENTER, 0, 0, None)
        u32.keybd_event(VK_ENTER, 0, 2, None)
        time.sleep(0.7)
        im6 = shot("06_files", rect)
        d6 = diff_pixels(im5, im6)
        print(f"files 后画面变化像素={d6}")

        if LOG.exists():
            with LOG.open("rb") as f:
                f.seek(log_off)
                keys = [l.decode("utf-8", "replace").rstrip() for l in f.readlines()
                        if b"[dbg] key" in l or b"pane shortcut" in l]
            print("--- 关键按键日志(最后 14 条) ---")
            for l in keys[-14:]:
                print("   ", l[-150:])

        checks = {
            "拖动分割线明显左移": x1 is not None and x1 < x0 - 100,
            "拖动未把另一侧压没": x1 is not None and x1 > 80,
            "快捷键让分割线右移": x2 is not None and x1 is not None and x2 > x1 + 15,
            "反向快捷键让分割线左移": x2b is not None and x2 is not None and x2b < x2 - 15,
            "命令面板出现": o3 > 20,
            "key 输出视图出现": o4 > 20,
            "key 输出视图可翻页": d4 > 300,
            "搜索框 / 切到命令面板": o5 > 20,
            "/files 打开铺满窗格的文件管理器": d6 > 800,
        }
        for k, v in checks.items():
            print(f"{'OK  ' if v else 'FAIL'} {k}")
        return 0 if all(checks.values()) else 1
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()


if __name__ == "__main__":
    raise SystemExit(main())
