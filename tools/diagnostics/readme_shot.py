#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""导出 README 用的截图 —— 拍「打开后的第一屏」。

不跑任何演示脚本、不模拟按键:配置只写 `theme = "<名字>"`,让 pwsh 按用户
自己的 profile 启动(oh-my-posh 提示符 + fastfetch 立绘 + 系统信息),导出的
就是每天打开 VectorTerm 看到的第一个画面。

两张图:

  assets/screenshot.png           头图,`deepseek-glass-minimal`(毛玻璃 + 只剩终端)
  assets/screenshot-deepseek.png  内置主题配图,`deepseek`(标签栏 + 状态栏都在)

两种取帧方式(`--capture auto` 会按主题自己选):

* `dump`   —— `VECTORTERM_DUMP_FRAME` 让程序把帧缓冲写成 BMP。拿到的是**纯
  渲染结果**,与桌面无关,可复现性最好。缺点:24 位 BMP 丢掉 alpha,毛玻璃
  主题导出来是"预乘过的深色底",不是肉眼看到的样子 —— 所以玻璃走下面这条。
* `screen` —— `ImageGrab` 抓窗口矩形,抓的是 DWM 合成后的画面:壁纸透过
  Acrylic、文字保持实心,和屏幕上一模一样。代价是壁纸会入镜,换台机器重拍
  背景就不一样(题注里已经写明这是本机实拍)。

流程:
  1. 把仓库里的主题同步到 <exe>/config/,确保 theme 命中最新配色
  2. 写一份临时 Lua:theme = "<名字>" + 真实 pwsh(加载 profile)
  3. 取帧(见上),写进 assets/
  4. 打印尺寸与体积

用法(项目根目录):
  python tools/diagnostics/readme_shot.py                          # 头图(minimal)
  python tools/diagnostics/readme_shot.py --theme deepseek \\
         --out assets/screenshot-deepseek.png                      # 主题配图
  python tools/diagnostics/readme_shot.py debug                    # 用 debug 版 exe
  README_SHOT_WAIT=9000 python tools/diagnostics/readme_shot.py    # 等 fastfetch 慢一点

临时配置只写 `theme = "<名字>"`,**不覆盖任何布局项** —— 窗口尺寸、shell 与
参数、字体、标签栏、状态栏全部继承主题,截图就是每天打开看到的那一屏。
"""
import argparse
import ctypes
import ctypes.wintypes as wt
import os
import pathlib
import shutil
import subprocess
import sys
import time

from PIL import Image, ImageGrab

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent


def parse_args(argv):
    p = argparse.ArgumentParser(description="导出 README 截图")
    p.add_argument("mode", nargs="?", default="release",
                   help="release(默认)/ debug,选哪一版 exe")
    p.add_argument("--theme", default=os.environ.get("README_SHOT_THEME", "deepseek-glass-minimal"),
                   help="config/ 里的主题名,默认 deepseek-glass-minimal")
    p.add_argument("--out", default=os.environ.get("README_SHOT_OUT"),
                   help="输出 PNG 路径,默认 assets/screenshot.png")
    p.add_argument("--capture", default=os.environ.get("README_SHOT_CAPTURE", "auto"),
                   choices=["auto", "dump", "screen"], help="取帧方式,默认 auto")
    p.add_argument("--wait", type=int, default=int(os.environ.get("README_SHOT_WAIT", "7000")),
                   help="首帧之后等多少毫秒再拍(默认 7000,留给 fastfetch / 状态栏)")
    return p.parse_args(argv)


def theme_is_glass(path: pathlib.Path) -> bool:
    """主题自己声明了毛玻璃 / 整窗半透明?这种必须用屏幕抓图。"""
    text = path.read_text(encoding="utf-8", errors="replace")
    for line in text.splitlines():
        code = line.split("--", 1)[0]
        if "win32_system_backdrop" in code and "None" not in code and "none" not in code:
            return True
        if "window_background_opacity" in code:
            try:
                val = float(code.split("=", 1)[1].strip().strip(","))
                if val < 0.999:
                    return True
            except (IndexError, ValueError):
                pass
    return False


def grab_window(pid, timeout=20.0):
    """等进程的窗口出现,返回 hwnd(拿不到就是 0)。"""
    user32 = ctypes.windll.user32
    end = time.time() + timeout
    while time.time() < end:
        hwnd = user32.FindWindowW(None, "VectorTerm") or 0
        if hwnd:
            return hwnd
        if pid and ctypes.windll.kernel32.OpenProcess(0x1000, False, pid):
            pass
        time.sleep(0.02)
    return 0


def capture_screen(hwnd, out: pathlib.Path, wait_ms: int):
    """抓窗口矩形(含 DWM 合成后的毛玻璃)。

    用 `GetClientRect` + `ClientToScreen` 而不是 `GetWindowRect`:`GetWindowRect`
    把窗口阴影 / 不可见缩放边框也算进去,上下会各多出几十像素的桌面,裁出来的
    图里终端内容就不在正中间了。
    """
    try:
        ctypes.windll.shcore.SetProcessDpiAwareness(2)   # 每显示器 DPI 感知
    except Exception:
        ctypes.windll.user32.SetProcessDPIAware()
    user32 = ctypes.windll.user32
    user32.SetForegroundWindow(hwnd)
    time.sleep(wait_ms / 1000.0)

    client = wt.RECT()
    user32.GetClientRect(hwnd, ctypes.byref(client))
    origin = wt.POINT(0, 0)
    user32.ClientToScreen(hwnd, ctypes.byref(origin))
    box = (
        origin.x,
        origin.y,
        origin.x + client.right,
        origin.y + client.bottom,
    )
    img = ImageGrab.grab(bbox=box, all_screens=True).convert("RGB")
    out.parent.mkdir(parents=True, exist_ok=True)
    img.save(out, optimize=True)
    return img


def main():
    args = parse_args(sys.argv[1:])
    theme = args.theme
    theme_path = ROOT / "config" / f"{theme}.lua"
    if not theme_path.is_file():
        sys.exit(f"没有 {theme_path}")

    exe = ROOT / "target" / args.mode / "VectorTerm.exe"
    if not exe.is_file():
        sys.exit(f"缺少 {exe},先跑 build.cmd --release")

    capture = args.capture
    if capture == "auto":
        capture = "screen" if theme_is_glass(theme_path) else "dump"

    out = pathlib.Path(args.out) if args.out else ROOT / "assets" / "screenshot.png"
    tmp = ROOT / "target" / "_readme_shot"
    tmp.mkdir(parents=True, exist_ok=True)

    # 主题按"磁盘优先、内嵌兜底"查找,所以要把它放到 exe 旁边
    shipped = ROOT / "target" / args.mode / "config"
    shipped.mkdir(parents=True, exist_ok=True)
    shutil.copy2(theme_path, shipped / theme_path.name)

    cfg = tmp / f"readme-{theme}.lua"
    cfg.write_text(f'return {{\n  theme = "{theme}",\n}}\n', encoding="utf-8")

    env = dict(os.environ)
    # 不要任何自动输入 / 分屏 / 自动关闭:截图必须是干净的第一屏
    for k in ("VECTORTERM_AUTOTYPE", "VECTORTERM_AUTO_SPLIT", "VECTORTERM_AUTO_FILES",
              "VECTORTERM_AUTO_CLOSE", "VECTORTERM_START_TABS"):
        env.pop(k, None)

    bmp = tmp / f"frame-{theme}.bmp"
    if capture == "dump":
        if bmp.exists():
            bmp.unlink()
        env["VECTORTERM_DUMP_FRAME"] = str(bmp)
        env["VECTORTERM_DUMP_AT"] = str(args.wait)

    proc = subprocess.Popen(
        [str(exe), "--config", str(cfg)], cwd=str(ROOT), env=env,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    try:
        if capture == "dump":
            for _ in range(120):
                time.sleep(0.25)
                if bmp.exists() and bmp.stat().st_size > 10000:
                    break
            if not bmp.exists():
                sys.exit("没拿到帧导出;检查 VECTORTERM_DUMP_FRAME 和 VectorTerm.log")
            time.sleep(0.4)
            img = Image.open(bmp).convert("RGB")
            out.parent.mkdir(parents=True, exist_ok=True)
            img.save(out, optimize=True)
        else:
            hwnd = grab_window(proc.pid)
            if not hwnd:
                sys.exit("没等到窗口")
            img = capture_screen(hwnd, out, args.wait)
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()

    print(f"{out}  {img.size[0]}x{img.size[1]}  ({out.stat().st_size / 1024:.0f} KiB)  "
          f"theme={theme} capture={capture}")


if __name__ == "__main__":
    main()
