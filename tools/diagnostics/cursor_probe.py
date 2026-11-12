#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""实机验证 cursor.* 配置:用一组临时 Lua 启动真窗口,量光标色块的尺寸。

把光标色改成 #FF00FF(品红),屏幕上就只有光标是品红,方便按像素统计。
输出帧/对照图写到 target/_cursor_shot/,不入库。
用法(项目根目录): python tools/diagnostics/cursor_probe.py
"""
import os
import pathlib
import shutil
import subprocess
import time

import numpy as np
from PIL import Image

ROOT = pathlib.Path(__file__).resolve().parents[2]
EXE = ROOT / "target" / "release" / "VectorTerm.exe"
TMP = ROOT / "target" / "_cursor_shot"
TMP.mkdir(parents=True, exist_ok=True)
shipped = ROOT / "target" / "release" / "config"
shipped.mkdir(parents=True, exist_ok=True)
shutil.copy2(ROOT / "config" / "deepseek.lua", shipped / "deepseek.lua")

CASES = {
    # 不写 cursor:默认整格方块、不闪、不动
    "default": "",
    # 细竖条
    "bar": 'shape = "bar", thickness = 2, blink = false,',
    # 半格方块居中
    "small": 'width = 0.5, height = 0.5, align_x = "center", align_y = "center", blink = false,',
    # 闪烁:duty 压到 5% → 95% 的时间都在灭相,不依赖精确截帧时刻
    "hidden": 'blink = true, blink_rate = 1000, blink_duty = 0.05, blink_style = "hidden",',
    "outline": 'blink = true, blink_rate = 1000, blink_duty = 0.05, blink_style = "outline",',
    "fade": 'blink = true, blink_rate = 1000, blink_duty = 0.05, blink_style = "fade",',
    # 尺寸脉冲:4s 周期,2.5s 处应该在放大相
    "pulse": 'anim = "pulse", anim_ms = 4000, anim_amount = 0.5, blink = false,',
}


def measure(path):
    a = np.array(Image.open(path).convert("RGB")).astype(int)
    r, g, b = a[:, :, 0], a[:, :, 1], a[:, :, 2]
    mask = (b - g > 25) & (r - g > 8) & (r > 25)
    ys, xs = np.where(mask)
    if len(ys) == 0:
        return 0, 0, 0, None
    mid = len(ys) // 2
    return len(ys), xs.max() - xs.min() + 1, ys.max() - ys.min() + 1, a[ys[mid], xs[mid]].tolist()


for name, extra in CASES.items():
    cfg = TMP / f"{name}.lua"
    cfg.write_text(
        "return {\n"
        '  theme = "deepseek",\n'
        '  colors = { cursor_bg = "#FF00FF" },\n'
        '  shell = "pwsh.exe",\n'
        '  shell_args = { "-NoLogo", "-NoProfile", "-NoExit", "-Command", '
        '"Write-Host \'CURSOR PROBE\'" },\n'
        f"  cursor = {{ {extra} }},\n"
        "}\n",
        encoding="utf-8",
    )
    bmp = TMP / f"{name}.bmp"
    if bmp.exists():
        bmp.unlink()
    env = dict(os.environ)
    env["VECTORTERM_DUMP_FRAME"] = str(bmp)
    env["VECTORTERM_DUMP_AT"] = "2500"
    for k in ("VECTORTERM_AUTOTYPE", "VECTORTERM_AUTO_SPLIT", "VECTORTERM_AUTO_FILES", "VECTORTERM_AUTO_CLOSE"):
        env.pop(k, None)
    p = subprocess.Popen(
        [str(EXE), "--config", str(cfg)], cwd=str(ROOT), env=env,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    try:
        for _ in range(60):
            time.sleep(0.25)
            if bmp.exists() and bmp.stat().st_size > 10000:
                break
        time.sleep(0.3)
    finally:
        p.terminate()
        try:
            p.wait(timeout=3)
        except subprocess.TimeoutExpired:
            p.kill()
    if not bmp.exists():
        print(f"{name:<9} NO FRAME", flush=True)
        continue
    Image.open(bmp).convert("RGB").save(TMP / f"{name}.png")
    n, w, h, mid = measure(bmp)
    print(f"{name:<9} magenta_px={n:<6} bbox={w}x{h} mid_rgb={mid}", flush=True)
