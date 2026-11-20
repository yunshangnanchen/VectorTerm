"""拖入文件 / 文件夹 → 终端里出现绝对路径:一键验证探针。

真拖拽由 OLE 拖放循环驱动,脚本没法凭空造一次 DnD,所以这里分两步验证:
  1. 走**内部入口**(`VECTORTERM_AUTO_DROP`,和真实拖放同一条代码:引号规则、
     落点选窗格、写 PTY),导帧 + `session_log` 确认路径真的到了提示符后面;
  2. 用 ctypes 查窗口上的 `OleDropTargetInterface` 属性,确认 winit 已经把
     OLE 拖放目标注册到 HWND 上 —— 注册在,资源管理器拖进来就一定会走
     `WindowEvent::DroppedFile` 那条分支。

用法:`python tools/diagnostics/drop_path_probe.py`
产物:`target/_drop_probe/*.png`(导出的帧)与终端里的对照表。
"""
import ctypes
import os
import pathlib
import subprocess
import sys
import time

from PIL import Image

ROOT = pathlib.Path(__file__).resolve().parents[2]
EXE = ROOT / "target" / "release" / "VectorTerm.exe"
OUT = ROOT / "target" / "_drop_probe"

# cmd.exe:没有 PSReadLine,也没有启动美化,回显最干净,适合对着截图肉眼核对
CFG_CMD = """return {{
  shell = "cmd.exe",
  render = "cpu",
  font = {{ size = 16 }},
  session_log = "{log}",
}}
"""
# 有 PSReadLine 的对照:路径同样是被"敲"进去的,应该出现在当前提示符后面
CFG_PWSH = """return {{
  shell = "pwsh.exe",
  shell_args = {{ "-NoLogo", "-NoProfile", "-NoExit" }},
  render = "cpu",
  font = {{ size = 16 }},
  session_log = "{log}",
}}
"""

user32 = ctypes.WinDLL("user32", use_last_error=True)


def window_of_pid(pid):
    """按进程号找第一个可见的顶层窗口。"""
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


def ole_drop_registered(hwnd):
    """窗口上有没有 OLE 拖放目标(winit 注册时会设这个窗口属性)。"""
    user32.GetPropW.restype = ctypes.c_void_p
    user32.GetPropW.argtypes = [ctypes.c_void_p, ctypes.c_wchar_p]
    return user32.GetPropW(hwnd, "OleDropTargetInterface") is not None


def run(name, cfg_tpl, drop_spec, dump_at=4000):
    OUT.mkdir(parents=True, exist_ok=True)
    log = OUT / f"{name}.bin"
    bmp = OUT / f"{name}.bmp"
    for p in (log, bmp):
        if p.exists():
            p.unlink()

    cfg = OUT / f"{name}.lua"
    cfg.write_text(cfg_tpl.format(log=log.as_posix()), encoding="utf-8")

    env = dict(os.environ)
    for k in ("VECTORTERM_AUTO_DROP", "VECTORTERM_AUTOTYPE", "VECTORTERM_DUMP_AT",
              "VECTORTERM_DUMP_FRAME"):
        env.pop(k, None)
    env["VECTORTERM_AUTO_DROP"] = drop_spec
    env["VECTORTERM_DUMP_AT"] = str(dump_at)
    env["VECTORTERM_DUMP_FRAME"] = str(bmp)

    proc = subprocess.Popen([str(EXE), "--config", str(cfg)], env=env)
    time.sleep(0.9)
    hwnd = window_of_pid(proc.pid)
    ole = ole_drop_registered(hwnd) if hwnd else None
    time.sleep(dump_at / 1000.0 + 1.6)
    proc.kill()
    proc.wait(timeout=10)

    png = None
    if bmp.exists():
        png = bmp.with_suffix(".png")
        Image.open(bmp).save(png)
    raw = log.read_bytes() if log.exists() else b""
    print(f"[{name}] OLE 拖放目标={'已注册' if ole else '未注册/没找到窗口'}  "
          f"日志={len(raw)}B  帧={png}")
    return png, raw


def main():
    if not EXE.exists():
        sys.exit(f"找不到 {EXE},先 cargo build --release")
    # 名字里带空格、带中文、带括号:三种最容易被 shell 拆开的字符
    spaced = OUT / "带 空格 的文件 (1).txt"
    OUT.mkdir(parents=True, exist_ok=True)
    spaced.write_text("probe", encoding="utf-8")
    folder = OUT / "有 空格的 文件夹"
    folder.mkdir(exist_ok=True)

    cases = [
        ("cmd_file", CFG_CMD, str(spaced), "带 空格 的文件"),
        ("cmd_folder", CFG_CMD, str(folder), "有 空格的 文件夹"),
        ("cmd_two", CFG_CMD, f"{spaced}|{folder}", "带 空格 的文件"),
        ("pwsh_file", CFG_PWSH, str(spaced), "带 空格 的文件"),
    ]
    for name, tpl, spec, expect in cases:
        _, raw = run(name, tpl, spec)
        tail = raw.decode("utf-8", "replace")
        # 日志里能搜到路径 = 这段字节真的写到了 shell 的输入上
        print("   日志里出现路径:", "是" if expect in tail else "否")

    print("\n验收:截图里提示符后面应出现带双引号的绝对路径,末尾一个空格。")


if __name__ == "__main__":
    main()
