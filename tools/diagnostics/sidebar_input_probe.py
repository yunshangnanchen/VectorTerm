#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""复现「Ctrl+Shift+E 侧边栏开合后,首次输入错位」的全套对照探针。

设计要点(按报告 docs/sidebar-input-misalign-rca.md):
  * 启动先跑一次 ls,让屏幕上有内容当参照物 —— 错位只在输入行出现,
    光看启动画面看不出来;
  * shell 美化是变量:带 profile(oh-my-posh + fastfetch) / -NoProfile /
    cmd.exe(无 PSReadLine)三组对照;
  * 侧边栏「只开不关」「开→关」「普通分屏」分开跑,定位触发条件;
  * cursor_* 组让 ConPTY 自报 CursorLeft/CursorTop/WindowSize.Width;
  * pty_* 组用 session_log 落盘 PTY 原始字节,看 PSReadLine 的绝对定位序列。

全部走程序自带诊断钩子,不注入按键、不改源码:
  VECTORTERM_AUTO_FILES=side@1300 / VECTORTERM_AUTO_FILES_CLOSE=2100
  VECTORTERM_AUTOTYPE (2.5s 写 PTY,和键盘同一条输入路径)
  VECTORTERM_DUMP_AT=2750,3400 + VECTORTERM_DUMP_FRAME=<bmp>
  session_log = "<out>/pty_<tag>.bin"

用法(项目根目录):
  python tools/diagnostics/sidebar_input_probe.py            # 全部 14 组,约 1.5 分钟
  python tools/diagnostics/sidebar_input_probe.py plain_toggle cursor_toggle
"""
import os
import pathlib
import subprocess
import sys
import time
from PIL import Image

ROOT = pathlib.Path(__file__).resolve().parents[2]
EXE = ROOT / "target" / "release" / "VectorTerm.exe"
OUT = ROOT / "target" / "_sidebar_bug"
OUT.mkdir(parents=True, exist_ok=True)

# 带美化:加载 profile(oh-my-posh + fastfetch),先 ls 再停在提示符
CFG_BEAUTY = ('return {\n  shell = "pwsh.exe",\n'
              '  shell_args = { "-NoLogo", "-NoExit", "-Command", "ls" },\n'
              '  render = "cpu",\n  font = { size = 16 },\n}\n')
# 干净 pwsh:同样先 ls,-NoProfile 关掉美化(PSReadLine 仍在场)
CFG_PLAIN = ('return {\n  shell = "pwsh.exe",\n'
             '  shell_args = { "-NoLogo", "-NoProfile", "-NoExit", "-Command", "ls" },\n'
             '  render = "cpu",\n  font = { size = 16 },\n}\n')
# 无 PSReadLine
CFG_CMD = ('return {\n  shell = "cmd.exe",\n'
           '  render = "cpu",\n  font = { size = 16 },\n}\n')

POS = ("Write-Host ('LEFT=' + [Console]::CursorLeft + "
       "' TOP=' + [Console]::CursorTop + "
       "' W=' + $Host.UI.RawUI.WindowSize.Width)\r")


def cfg_with_log(tag):
    path = (OUT / f"pty_{tag}.bin").as_posix()
    return ('return {\n  shell = "pwsh.exe",\n'
            '  shell_args = { "-NoLogo", "-NoProfile", "-NoExit", "-Command", "ls" },\n'
            '  render = "cpu",\n  font = { size = 16 },\n'
            f'  session_log = "{path}",\n}}\n')


# name, cfg, open_side, do_close, autotype
CASES = [
    ("beauty_base",        CFG_BEAUTY,          False, False, "abc"),
    ("beauty_toggle",      CFG_BEAUTY,          True,  True,  "abc"),
    ("beauty_keep",        CFG_BEAUTY,          True,  False, "abc"),
    ("beauty_toggle_redr", CFG_BEAUTY,          True,  True,  "abc\x0c"),
    ("plain_base",         CFG_PLAIN,           False, False, "abc"),
    ("plain_toggle",       CFG_PLAIN,           True,  True,  "abc"),
    ("plain_keep",         CFG_PLAIN,           True,  False, "abc"),
    ("plain_toggle_ent",   CFG_PLAIN,           True,  True,  "abc\r"),
    ("split_only",         CFG_PLAIN,           False, False, "abc"),
    ("cmd_toggle",         CFG_CMD,             True,  True,  "abc"),
    ("cursor_base",        CFG_PLAIN,           False, False, POS),
    ("cursor_toggle",      CFG_PLAIN,           True,  True,  POS),
    ("pty_base",           cfg_with_log("base"),   False, False, "abc"),
    ("pty_toggle",         cfg_with_log("toggle"), True,  True,  "abc"),
]


def segments(im, y, bg, tol=70, gap=14):
    """一行里连续的墨迹段 [(x0,x1)]，间隔 > gap 视为断开。"""
    px = im.load()
    hits = [x for x in range(im.size[0])
            if sum(abs(a - b) for a, b in zip(px[x, y], bg)) > tol]
    if not hits:
        return []
    out, s, p = [], hits[0], hits[0]
    for x in hits[1:]:
        if x - p > gap:
            out.append((s, p))
            s = x
        p = x
    out.append((s, p))
    return out


def run(name, lua, open_side, do_close, keys):
    cfg = OUT / f"{name}.lua"
    cfg.write_text(lua, encoding="utf-8")
    base = OUT / f"{name}.bmp"
    for p in OUT.glob(f"{name}*.bmp"):
        p.unlink()
    for p in OUT.glob(f"{name}*.png"):
        p.unlink()

    env = dict(os.environ)
    for k in list(env):
        if k.startswith("VECTORTERM_AUTO_") or k in ("VECTORTERM_DUMP_FRAME", "VECTORTERM_DUMP_AT"):
            env.pop(k, None)
    env["VECTORTERM_DUMP_FRAME"] = str(base)
    env["VECTORTERM_DUMP_AT"] = "2750,3400"
    env["VECTORTERM_AUTOTYPE"] = keys
    if open_side:
        env["VECTORTERM_AUTO_FILES"] = "side@1300"
        if do_close:
            env["VECTORTERM_AUTO_FILES_CLOSE"] = "2100"
    if name.startswith("split"):
        env["VECTORTERM_AUTO_SPLIT"] = "v"

    p = subprocess.Popen([str(EXE), "--config", str(cfg)], cwd=str(ROOT), env=env,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(6.5)
    p.terminate()
    try:
        p.wait(timeout=3)
    except subprocess.TimeoutExpired:
        p.kill()

    frames = sorted(OUT.glob(f"{name}*.bmp"))
    for bmp in frames:
        im = Image.open(bmp).convert("RGB")
        im.save(bmp.with_suffix(".png"))
    # 只看最后一帧的墨迹分布(前 14 个采样行)
    if frames:
        im = Image.open(frames[-1]).convert("RGB")
        bg = im.getpixel((2, 2))
        report = []
        for y in range(40, im.size[1] - 10, 6):
            segs = segments(im, y, bg)
            if segs:
                report.append((y, segs[:6]))
            if len(report) >= 14:
                break
        print(name, "frames", [f.stem for f in frames], "bands", report, flush=True)
    else:
        print(name, "NO FRAME", flush=True)


def dump_pty_logs():
    """对比 PSReadLine 重绘时的绝对光标定位序列。"""
    import re
    for tag in ("base", "toggle"):
        p = OUT / f"pty_{tag}.bin"
        if not p.exists():
            print(f"pty_{tag}: NO LOG")
            continue
        b = p.read_bytes()
        s = "".join(chr(c) if 32 <= c < 127 else
                    {10: "\\n", 13: "\\r", 27: "\\e", 9: "\\t"}.get(c, f"\\x{c:02x}")
                    for c in b[-1200:])
        coords = sorted(set(re.findall(r"\\e\[(\d+);(\d+)H", s)))
        print(f"pty_{tag}: len={len(b)} absolute_cursor={coords} tail={s[-260:]}", flush=True)


if __name__ == "__main__":
    wanted = set(sys.argv[1:])
    cases = [c for c in CASES if not wanted or c[0] in wanted]
    for p in OUT.glob("pty_*.bin"):
        p.unlink()
    for c in cases:
        run(*c)
    if any(c[0].startswith("pty_") for c in cases):
        dump_pty_logs()
