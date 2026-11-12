"""实机验证:中文输入法开着时,快捷键和输入法上屏还能不能正常工作。

起因(用户报的三件事):
  1. 打开搜索框之后,终端里原来那个光标没有消失,和搜索框的输入光标同时闪;
  2. 输入法是中文时,搜索框/命令面板里打出的中文符号直接进了终端;
  3. 输入法是中文时 `Ctrl+Shift+E`(文件面板)打不开,切成「微软美国输入法」
     才正常。

第 3 条已经用 `examples/ime_probe.rs` 定位过:消息本身到了,但 `wParam` 被
输入法换成了 `VK_PROCESSKEY`(0xE5),`lParam` 里的扫描码还是原样 ——
winit 据此算出的 `logical_key` 是 `Unidentified`,按字符匹配的快捷键全部失配。
修复在 `src/winhook.rs::fix_processkey`(扫描码反推真实键码再转发给 winit)。

本探针全程用真实窗口 + 扫描码注入(不能用 `wVk` 注入:那样 lParam 的扫描码
是 0,复现不出现场):
  1. `VECTORTERM_IME_OPEN=1` 让程序自己切到 0804 并打开输入法;
  2. 送 `Ctrl+Shift+E` → 日志里应出现「VK_PROCESSKEY 还原 vk=0x45」和
     `logical=Character("E")`,截图应能看到右侧文件面板;
  3. 收起来再送 `Ctrl+Shift+F` 开搜索框 → 截帧核对终端光标已经让位;
  4. 在搜索框里敲一个逗号 → 日志里应是 `ime commit "，" -> search`,
     而不是 `-> pty`。

用法: python tools/diagnostics/ime_shortcut_probe.py [debug|release]
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
OUT = ROOT / "target" / "ime_probe"
OUT.mkdir(parents=True, exist_ok=True)

CFG = """return {
  initial_cols = 100,
  initial_rows = 26,
  shell = "cmd.exe",
  status_bar = { enabled = false },
  colors = { foreground = "#cccccc", background = "#0c0c0c" },
  search = { enabled = true },
}
"""
cfg_path = OUT / "probe.lua"
cfg_path.write_text(CFG, encoding="utf-8")

u32 = ctypes.WinDLL("user32", use_last_error=True)
try:
    ctypes.WinDLL("user32").SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
except Exception as exc:  # pragma: no cover
    print("DPI 感知设置失败(继续):", exc)

u32.GetWindowThreadProcessId.argtypes = [wt.HWND, ctypes.POINTER(wt.DWORD)]
u32.SetForegroundWindow.argtypes = [wt.HWND]
u32.GetWindowRect.argtypes = [wt.HWND, ctypes.POINTER(wt.RECT)]
u32.SetWindowPos.argtypes = [wt.HWND, wt.HWND, ctypes.c_int, ctypes.c_int,
                             ctypes.c_int, ctypes.c_int, ctypes.c_uint]
u32.SetCursorPos.argtypes = [ctypes.c_int, ctypes.c_int]
u32.mouse_event.argtypes = [wt.DWORD, wt.DWORD, wt.DWORD, wt.DWORD, ctypes.c_void_p]

HWND_TOPMOST = -1
SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW = 0x0002, 0x0001, 0x0040
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
KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE = 0x0002, 0x0008

# 扫描码(Set 1)。必须走 KEYEVENTF_SCANCODE:用 wVk 注入的 lParam 里扫描码是 0,
# 而「输入法抹掉 wParam」这个 bug 的现场恰恰只有扫描码还在。
SC_CTRL, SC_SHIFT, SC_E, SC_F, SC_COMMA = 0x1D, 0x2A, 0x12, 0x21, 0x33
SC_N, SC_I, SC_SPACE = 0x31, 0x17, 0x39


def sc(scan, up=False):
    flags = KEYEVENTF_SCANCODE | (KEYEVENTF_KEYUP if up else 0)
    inp = INPUT(type=1, u=INPUTUNION(ki=KEYBDINPUT(0, scan, flags, 0, None)))
    u32.SendInput(1, ctypes.byref(inp), ctypes.sizeof(INPUT))
    time.sleep(0.03)


def combo(*scans, target=None):
    if target:
        u32.SetForegroundWindow(target)
        time.sleep(0.06)
    for s in scans[:-1]:
        sc(s)
    time.sleep(0.05)
    sc(scans[-1])
    sc(scans[-1], up=True)
    time.sleep(0.08)
    for s in reversed(scans[:-1]):
        sc(s, up=True)
    time.sleep(0.35)


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


def dump_frame(name, box):
    """抓整窗截图(ImageGrab,和 readme_shot 一致的物理像素口径)"""
    from PIL import ImageGrab
    im = ImageGrab.grab(bbox=(box.left, box.top, box.right, box.bottom)).convert("RGB")
    path = OUT / f"ime_{name}.png"
    im.save(path)
    return im, path


def ink(im, x0, y0, x1, y1, bg=(18, 18, 20), tol=14):
    """区域内的非背景像素数 —— 用来判断「有没有画出东西」"""
    px = im.load()
    n = 0
    for y in range(max(0, y0), min(im.size[1], y1), 2):
        for x in range(max(0, x0), min(im.size[0], x1), 2):
            r, g, b = px[x, y]
            if abs(r - bg[0]) > tol or abs(g - bg[1]) > tol or abs(b - bg[2]) > tol:
                n += 1
    return n


def bright(im, x0, y0, x1, y1, th=150):
    """亮像素数(光标是整格纯白/浅色反显,字形笔画细但数量同量级)"""
    px = im.load()
    n = 0
    for y in range(max(0, y0), min(im.size[1], y1)):
        for x in range(max(0, x0), min(im.size[0], x1)):
            r, g, b = px[x, y]
            if r > th and g > th and b > th:
                n += 1
    return n


def caret_peak(box, band, shots=4, gap=0.19):
    """终端光标「在不在」的判据:连拍几帧取亮像素数的**峰值**。

    单帧靠不住:光标会闪,可能正好拍到不亮的那半边;所以取峰值就等于
    「只要它亮过一次就算在」。比较两次测量的峰值,差的就是那一格光标
    的面积(整格反显,比字形笔画厚实得多)。
    """
    from PIL import ImageGrab
    vals = []
    for _ in range(shots):
        im = ImageGrab.grab(bbox=(box.left, box.top, box.right, box.bottom)).convert("RGB")
        vals.append(bright(im, *band))
        time.sleep(gap)
    return max(vals), vals


def tail(off):
    if not LOG.exists():
        return ""
    with LOG.open("r", encoding="utf-8", errors="replace") as f:
        f.seek(off)
        return f.read()


def main():
    env = dict(os.environ)
    env["VECTORTERM_IME_OPEN"] = "1"
    env["VECTORTERM_DEBUG_KEYS"] = "1"
    for k in ("VECTORTERM_AUTO_SPLIT", "VECTORTERM_AUTO_FILES", "VECTORTERM_AUTO_PANEL",
              "VECTORTERM_AUTOTYPE", "VECTORTERM_DUMP_FRAME"):
        env.pop(k, None)
    off = LOG.stat().st_size if LOG.exists() else 0
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

    u32.SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0,
                     SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW)
    u32.SetForegroundWindow(hwnd)
    box = wt.RECT()
    u32.GetWindowRect(hwnd, ctypes.byref(box))
    # 点一下内容区,确保键盘焦点真的在窗口上
    u32.SetCursorPos(box.left + 200, box.top + 200)
    u32.mouse_event(LEFTDOWN, 0, 0, 0, None)
    u32.mouse_event(LEFTUP, 0, 0, 0, None)
    time.sleep(1.2)

    ok = True
    log = tail(off)
    ime_on = "已强制打开输入法(0804)" in log
    print("输入法:", "已打开" if ime_on else "!! 没打开(后面的结论都不算数)")
    ok &= ime_on

    # ---- 1. Ctrl+Shift+E:文件面板 ----
    combo(SC_CTRL, SC_SHIFT, SC_E, target=hwnd)
    time.sleep(0.6)
    im, path1 = dump_frame("files", box)
    # 侧边栏在右侧:取右边 1/4 区域看有没有面板墨迹
    w, h = im.size
    right_ink = ink(im, int(w * 0.78), int(h * 0.15), w - 4, int(h * 0.9))
    print(f"文件面板区域墨迹: {right_ink}  -> {path1.name}")

    log = tail(off)
    recovered = "VK_PROCESSKEY 还原 vk=0x45" in log
    logical_e = 'logical=Character("E")' in log or "logical=Character('E')" in log
    print("  消息层还原真身:", "是" if recovered else "否")
    print("  winit 给出 Character(E):", "是" if logical_e else "否")
    ok &= recovered and logical_e and right_ink > 200

    # ---- 2. 收起文件面板,开搜索框 ----
    # browser_toggle 的语义是「没开→开 / 开着→关」,所以再按一次就收起来了
    combo(SC_CTRL, SC_SHIFT, SC_E, target=hwnd)
    combo(SC_CTRL, SC_SHIFT, SC_F, target=hwnd)
    time.sleep(0.8)
    im_before, path2 = dump_frame("search", box)
    print("搜索框已开 ->", path2.name)

    # 终端光标应当已经让位。判据:提示符所在那一带的亮像素峰值,搜索框
    # 开着 vs 关掉,差出来的正好是一整格光标。
    band = (0, int(h * 0.12), int(w * 0.5), int(h * 0.26))
    peak_open, vals_open = caret_peak(box, band)
    print(f"搜索框开着: 提示符一带亮像素 {vals_open}(峰值 {peak_open})")

    # ---- 3. 在搜索框里打拼音:先看组合串,再上屏 ----
    for s in (SC_N, SC_I):
        combo(s, target=hwnd)
    time.sleep(0.5)
    im_pre, path_pre = dump_frame("search_preedit", box)
    print("组合串(拼音未上屏) ->", path_pre.name)
    combo(SC_SPACE, target=hwnd)
    time.sleep(0.3)
    combo(SC_COMMA, target=hwnd)
    time.sleep(0.6)
    im_after, path3 = dump_frame("search_comma", box)
    print("  ->", path3.name)

    log = tail(off)
    committed = [ln for ln in log.splitlines() if "ime commit" in ln]
    routed_search = any("-> search" in ln for ln in committed)
    routed_pty = any("-> pty" in ln for ln in committed)
    print("输入法上屏路由:", "search" if routed_search else ("pty(错)" if routed_pty else "没收到提交"))
    for ln in committed:
        print("   ", ln.split("] ")[-1])
    ok &= routed_search and not routed_pty

    # ---- 3b. 命令面板同样要接住中文 ----
    combo(SC_CTRL, SC_SHIFT, 0x19, target=hwnd)  # Ctrl+Shift+P
    time.sleep(0.7)
    for s in (SC_N, SC_I):
        combo(s, target=hwnd)
    time.sleep(0.3)
    combo(SC_SPACE, target=hwnd)
    time.sleep(0.5)
    im_panel, path_panel = dump_frame("panel", box)
    print("命令面板 ->", path_panel.name)
    log = tail(off)
    routed_cmd = any("-> commands" in ln for ln in log.splitlines())
    print("面板里的上屏:", "commands" if routed_cmd else "没收到")
    ok &= routed_cmd
    combo(0x01, target=hwnd)  # Esc 关面板
    time.sleep(0.3)

    # ---- 4. 关掉搜索框:终端光标应该回来 ----
    combo(0x01, target=hwnd)  # Esc
    time.sleep(0.8)
    im_end, path4 = dump_frame("closed", box)
    peak_closed, vals_closed = caret_peak(box, band)
    print(f"搜索框已关: 提示符一带亮像素 {vals_closed}(峰值 {peak_closed})")
    print("  ->", path4.name)
    delta = peak_closed - peak_open
    print(f"  光标面积差: {delta}  (应当是一整格,几百像素)")
    if delta < 100:
        print("!! 终端光标没有随浮层开合让位/复位")
        ok = False

    p.terminate()
    time.sleep(0.4)
    print("\n结论:", "PASS" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
