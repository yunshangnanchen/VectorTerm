#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""逐帧拍浮层(历史搜索 / 命令面板)的进出场动画。

浮层的动画只有一两百毫秒,肉眼只能看出"有动画",看不出中途有没有跳变、
缩放和位移是不是同步收尾。这里的做法是让程序自己在指定毫秒各导一帧
(`VECTORTERM_DUMP_AT`),再把这些帧拼成一张横向连拍图。

触发靠两个诊断钩子:
  `VECTORTERM_AUTO_PANEL=search|commands[@毫秒]`  到点打开
  `VECTORTERM_AUTO_PANEL_CLOSE=毫秒`             到点关闭(测退场)

用法(项目根目录):
  python tools/diagnostics/overlay_probe.py search
  python tools/diagnostics/overlay_probe.py commands --close-at 2000
  python tools/diagnostics/overlay_probe.py search --frames 1210,1240,1280,1330
  python tools/diagnostics/overlay_probe.py search --zoom 2 --out target/_x.png
"""
import argparse
import os
import pathlib
import subprocess
import sys
import time

from PIL import Image, ImageDraw

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
THEME = ROOT / "config" / "deepseek.lua"
TMP = ROOT / "target" / "_overlay_probe"

# 进场 190ms / 退场 140ms(见 main.rs 的 OverlayAnim),这两串时刻就是照着它排的
IN_FRAMES = "auto+4,auto+12,auto+22,auto+34,auto+50,auto+75,auto+120,auto+220"
OUT_FRAMES = "close+4,close+20,close+40,close+70,close+110,close+180"


def parse_frames(spec: str, auto: int, close: int | None) -> list[int]:
    """`auto+28` / `close+74` / 纯数字 三种写法混用。"""
    out = []
    for tok in spec.replace(" ", "").split(","):
        if not tok:
            continue
        if tok.startswith("auto+"):
            out.append(auto + int(tok[5:]))
        elif tok.startswith("close+"):
            if close is None:
                sys.exit("用了 close+… 但没给 --close-at")
            out.append(close + int(tok[6:]))
        else:
            out.append(int(tok))
    return sorted(set(out))


def run(exe: pathlib.Path, cfg: pathlib.Path, panel: str, open_at: int,
        close_at: int | None, times: list[int], base: pathlib.Path) -> list[pathlib.Path]:
    env = dict(os.environ)
    for k in ("VECTORTERM_AUTOTYPE", "VECTORTERM_AUTO_SPLIT", "VECTORTERM_AUTO_FILES",
              "VECTORTERM_AUTO_FOCUS", "VECTORTERM_DUMP_TRAIL"):
        env.pop(k, None)
    env["VECTORTERM_AUTO_PANEL"] = f"{panel}@{open_at}"
    env["VECTORTERM_DUMP_FRAME"] = str(base)
    env["VECTORTERM_DUMP_AT"] = ",".join(str(t) for t in times)
    if close_at is not None:
        env["VECTORTERM_AUTO_PANEL_CLOSE"] = str(close_at)

    proc = subprocess.Popen(
        [str(exe), "--config", str(cfg)], cwd=str(ROOT), env=env,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    want = [base.with_name(f"{base.stem}-{t}{base.suffix}") for t in times]
    try:
        deadline = time.time() + max(times) / 1000.0 + 20.0
        while time.time() < deadline:
            time.sleep(0.2)
            if all(p.exists() and p.stat().st_size > 10000 for p in want):
                break
        missing = [p.name for p in want if not p.exists()]
        if missing:
            sys.exit(f"少了几帧: {missing}(检查 VectorTerm.log)")
        time.sleep(0.2)
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()
    return want


def strip(frames: list[tuple[int, pathlib.Path]], panel: str, zoom: int,
          out: pathlib.Path) -> pathlib.Path:
    """把若干帧裁到浮层所在的区域,横向拼成一张图,标上毫秒数。"""
    ims = [(t, Image.open(p).convert("RGB")) for t, p in frames]
    w, h = ims[0][1].size
    # 搜索栏画在底部、命令面板画在顶部:各裁各的那一段,别把整屏都堆上来
    if panel.startswith("command") or panel == "panel":
        box = (0, 0, w, min(h, 360))
    else:
        box = (0, max(0, h - 130), w, h)
    cw, ch = box[2] - box[0], box[3] - box[1]
    tag_h = 18
    canvas = Image.new("RGB", (cw * zoom, (ch * zoom + tag_h) * len(ims)), (8, 10, 16))
    draw = ImageDraw.Draw(canvas)
    for i, (t, im) in enumerate(ims):
        crop = im.crop(box).resize((cw * zoom, ch * zoom), Image.LANCZOS)
        y = i * (ch * zoom + tag_h)
        canvas.paste(crop, (0, y + tag_h))
        draw.text((4, y + 4), f"t={t}ms", fill=(150, 170, 210))
    out.parent.mkdir(parents=True, exist_ok=True)
    canvas.save(out)
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("panel", choices=["search", "commands", "panel"], help="拍哪个浮层")
    ap.add_argument("--mode", default="release", choices=["release", "debug"])
    ap.add_argument("--open-at", type=int, default=1200)
    ap.add_argument("--close-at", type=int, default=None)
    ap.add_argument("--frames", default=None,
                    help=f"默认进场 {IN_FRAMES};给了 --close-at 就再加退场 {OUT_FRAMES}")
    ap.add_argument("--zoom", type=int, default=1)
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    exe = ROOT / "target" / args.mode / "VectorTerm.exe"
    if not exe.is_file():
        sys.exit(f"缺少 {exe},先跑 build.cmd --release")
    TMP.mkdir(parents=True, exist_ok=True)
    # 每次重跑都清掉旧帧,免得把上一轮的图看成本轮的
    for old in TMP.glob("f-*.bmp"):
        old.unlink()

    spec = args.frames
    if spec is None:
        spec = IN_FRAMES + ("," + OUT_FRAMES if args.close_at is not None else "")
    times = parse_frames(spec, args.open_at, args.close_at)

    base = TMP / "f.bmp"
    paths = run(exe, THEME, args.panel, args.open_at, args.close_at, times, base)
    pairs = list(zip(times, paths))
    out = pathlib.Path(args.out or (TMP / f"{args.panel}_strip.png"))
    p = strip(pairs, args.panel, args.zoom, out)
    print(f"{len(pairs)} 帧 -> {p}  ({Image.open(p).size[0]}x{Image.open(p).size[1]})")


if __name__ == "__main__":
    main()
