#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""用 config/deepseek.lua 主题导出 README 头图 —— 拍「打开后的第一屏」。

这里不跑任何演示脚本、不模拟按键:配置只写 `theme = "deepseek"`,让 pwsh
按用户自己的 profile 启动(oh-my-posh 提示符 + fastfetch 立绘 + 系统信息),
导出的就是每天打开 VectorTerm 看到的第一个画面。

流程:
  1. 把仓库里的主题同步到 <exe>/config/,确保 theme 命中最新配色
  2. 写一份临时 Lua:theme = "deepseek" + 真实 pwsh(加载 profile)
  3. VECTORTERM_DUMP_FRAME 导出 framebuffer BMP,VECTORTERM_DUMP_AT 留到
     fastfetch 跑完 + 状态栏定时器刷过一轮之后
  4. 转成 PNG 写进 assets/screenshot.png

用法(项目根目录):
  python tools/diagnostics/readme_shot.py                 # 默认 release
  python tools/diagnostics/readme_shot.py debug           # 用 debug 版 exe
  README_SHOT_SIZE=1600x820 python tools/diagnostics/readme_shot.py
  README_SHOT_OUT=target/_readme_shot/probe.png python tools/diagnostics/readme_shot.py

临时配置只写 `theme = "deepseek"`,**不覆盖任何布局项** —— 窗口尺寸
(window_width/height = 1250×750)、shell 与参数、字体、标签栏、状态栏
全部继承主题,截图尺寸和内容就是每天打开 VectorTerm 看到的那一屏。
README_SHOT_SIZE 仅供临时排查(例如 fastfetch 折行残尾),默认不要设。
"""
import os
import pathlib
import shutil
import subprocess
import sys
import time

from PIL import Image

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
THEME = ROOT / "config" / "deepseek.lua"
TMP = ROOT / "target" / "_readme_shot"
OUT = pathlib.Path(os.environ.get("README_SHOT_OUT", ROOT / "assets" / "screenshot.png"))
SIZE = os.environ.get("README_SHOT_SIZE")   # 默认 None:完全继承主题里的窗口尺寸

MODE = sys.argv[1] if len(sys.argv) > 1 else "release"
EXE = ROOT / "target" / MODE / "VectorTerm.exe"
if not EXE.is_file():
    sys.exit(f"缺少 {EXE},先跑 build.cmd --release")

TMP.mkdir(parents=True, exist_ok=True)
OUT.parent.mkdir(parents=True, exist_ok=True)
shipped = ROOT / "target" / MODE / "config"
shipped.mkdir(parents=True, exist_ok=True)
shutil.copy2(THEME, shipped / THEME.name)

cfg = TMP / "readme.lua"
lua = ["return {", '  theme = "deepseek",']
if SIZE:
    width, height = (int(x) for x in SIZE.lower().split("x"))
    lua.append(f"  window_width = {width},")
    lua.append(f"  window_height = {height},")
lua.append("}")
cfg.write_text("\n".join(lua) + "\n", encoding="utf-8")

bmp = TMP / f"frame_{SIZE or 'theme'}.bmp"
if bmp.exists():
    bmp.unlink()
env = dict(os.environ)
env["VECTORTERM_DUMP_FRAME"] = str(bmp)
env["VECTORTERM_DUMP_AT"] = "7000"
# 不要任何自动输入/分屏/自动关闭:头图必须是干净的第一屏
for k in ("VECTORTERM_AUTOTYPE", "VECTORTERM_AUTO_SPLIT", "VECTORTERM_AUTO_FILES", "VECTORTERM_AUTO_CLOSE"):
    env.pop(k, None)

proc = subprocess.Popen(
    [str(EXE), "--config", str(cfg)], cwd=str(ROOT), env=env,
    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
)
try:
    for _ in range(100):
        time.sleep(0.25)
        if bmp.exists() and bmp.stat().st_size > 10000:
            break
    if not bmp.exists():
        sys.exit("没拿到帧导出;检查 VECTORTERM_DUMP_FRAME 和 VectorTerm.log")
    time.sleep(0.4)
finally:
    proc.terminate()
    try:
        proc.wait(timeout=3)
    except subprocess.TimeoutExpired:
        proc.kill()

im = Image.open(bmp).convert("RGB")
im.save(OUT, optimize=True)
print(f"{OUT}  {im.size[0]}x{im.size[1]}  ({OUT.stat().st_size / 1024:.0f} KiB)")
