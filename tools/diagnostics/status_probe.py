#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""量测标签栏右侧状态栏的排版:字形墨迹包围盒 / 基线 / 间距。

状态栏是终端里唯一"不按网格排"的文字(按 shaper 的自然字距推进),
所以字形大小、垂直对齐、空格收缩这些东西肉眼看不出差几像素,只能量。

做法:把 config/deepseek.lua 复制一份,在 `update-right-status` 回调开头
塞一段固定测试文字,然后让程序用 VECTORTERM_DUMP_FRAME 导出一帧,再在标签栏
那一行上按列扫描墨迹 —— 打印每段的包围盒,并另存一张放大图给人看。

用法(项目根目录):
  python tools/diagnostics/status_probe.py "☾ 晚上好|晚上好|97%"
  python tools/diagnostics/status_probe.py "a|b" --out target/_probe/bar.png
  python tools/diagnostics/status_probe.py "..." --mode debug

测试文字用 `|` 分段:每段之间互相独立地量,免得宽度算错时串行。
段首写 `@单元格底色` 之类的高级玩法这里不需要 —— 只看排版。
"""
import argparse
import pathlib
import re
import subprocess
import sys
import time

from PIL import Image

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
THEME = ROOT / "config" / "deepseek.lua"
TMP = ROOT / "target" / "_status_probe"

# 标签栏那一行的扫描区间(物理像素)。默认取 1250x750 主题尺寸的前 34 行。
BAR_H = 34


def build_lua(segments: str) -> str:
    """把 `|` 分段拼成 Lua 的 FormatItem 列表,段与段之间插一个竖线标记。"""
    lit = segments.split("|")
    parts = []
    for i, seg in enumerate(lit):
        if i:
            parts.append("{ Foreground = { Color = '#8A93A8' } }, { Text = '|' },")
        parts.append("{ Foreground = { Color = '#6D87FF' } }, { Text = '%s' }," % seg.replace("\\", "\\\\").replace("'", "\\'"))
    return "\n".join(parts)


def make_config(segments: str) -> pathlib.Path:
    src = THEME.read_text(encoding="utf-8")
    anchor = "wezterm.on('update-right-status', function(window, pane)"
    if anchor not in src:
        sys.exit("deepseek.lua 里找不到 update-right-status 回调,状态栏脚本结构变了")
    inject = (
        anchor
        + "\n  -- [probe] 固定测试文字,后面那句 return 让原脚本不执行\n"
        + "  window:set_right_status(wezterm.format({\n"
        + build_lua(segments)
        + "\n  }))\n  do return end\n"
    )
    probe = TMP / "probe.lua"
    probe.write_text(src.replace(anchor, inject, 1), encoding="utf-8")
    return probe


def dump_frame(exe: pathlib.Path, cfg: pathlib.Path, wait_ms: int) -> pathlib.Path:
    bmp = TMP / "frame.bmp"
    if bmp.exists():
        bmp.unlink()
    env = dict(__import__("os").environ)
    env["VECTORTERM_DUMP_FRAME"] = str(bmp)
    env["VECTORTERM_DUMP_AT"] = str(wait_ms)
    for k in ("VECTORTERM_AUTOTYPE", "VECTORTERM_AUTO_SPLIT", "VECTORTERM_AUTO_FILES", "VECTORTERM_AUTO_CLOSE"):
        env.pop(k, None)
    proc = subprocess.Popen(
        [str(exe), "--config", str(cfg)], cwd=str(ROOT), env=env,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    try:
        for _ in range(120):
            time.sleep(0.25)
            if bmp.exists() and bmp.stat().st_size > 10000:
                break
        if not bmp.exists():
            sys.exit("没拿到帧导出;检查 VECTORTERM_DUMP_FRAME 与 VectorTerm.log")
        time.sleep(0.3)
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()
    return bmp


def ink_runs(im: Image.Image, y0: int = 3, y1: int = BAR_H, thr: int = 70):
    """按列扫墨迹,返回 [(x0, x1), ...] 连续有墨迹的列区间。"""
    px = im.load()
    runs, start = [], None
    for x in range(im.size[0]):
        ink = any(max(px[x, y]) > thr for y in range(y0, min(y1, im.size[1])))
        if ink and start is None:
            start = x
        elif not ink and start is not None:
            runs.append((start, x - 1))
            start = None
    if start is not None:
        runs.append((start, im.size[0] - 1))
    return runs


def bbox(im: Image.Image, x0: int, x1: int, y0: int = 3, y1: int = BAR_H, thr: int = 70):
    px = im.load()
    pts = [(x, y) for y in range(y0, min(y1, im.size[1])) for x in range(x0, x1 + 1)
           if max(px[x, y]) > thr]
    if not pts:
        return None
    return (min(p[0] for p in pts), max(p[0] for p in pts),
            min(p[1] for p in pts), max(p[1] for p in pts))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("segments", help="测试文字,用 | 分段")
    ap.add_argument("--mode", default="release", choices=["release", "debug"])
    ap.add_argument("--wait", type=int, default=1500)
    ap.add_argument("--out", default=str(TMP / "bar.png"))
    ap.add_argument("--zoom", type=int, default=3)
    args = ap.parse_args()

    exe = ROOT / "target" / args.mode / "VectorTerm.exe"
    if not exe.is_file():
        sys.exit(f"缺少 {exe},先跑 build.cmd --release")
    TMP.mkdir(parents=True, exist_ok=True)
    cfg = make_config(args.segments)
    bmp = dump_frame(exe, cfg, args.wait)
    im = Image.open(bmp).convert("RGB")

    runs = ink_runs(im)
    print(f"帧 {im.size[0]}x{im.size[1]},标签栏墨迹列区间 {len(runs)} 段")
    for i, (a, b) in enumerate(runs):
        print(f"  #{i:02d} x {a:4d}..{b:4d} (宽 {b - a + 1:3d}) 包围盒 {bbox(im, a, b)}")

    # 左右各留一点白,纵向取标签栏,放大后存给人看
    if runs:
        left = max(0, runs[0][0] - 12)
        right = min(im.size[0], runs[-1][1] + 12)
    else:
        left, right = 0, im.size[0]
    crop = im.crop((left, 0, right, BAR_H))
    crop = crop.resize((crop.width * args.zoom, crop.height * args.zoom), Image.NEAREST)
    out = pathlib.Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    crop.save(out)
    print(f"放大图 {out}  ({crop.size[0]}x{crop.size[1]})")


if __name__ == "__main__":
    main()
