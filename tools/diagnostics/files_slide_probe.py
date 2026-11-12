#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""逐帧拍文件面板(侧边栏 / 全屏文件管理器)的横向进出场动画。

面板的进出场是从左到右滑出来的,只有两百毫秒。肉眼能看出"有动画",但看不出
中途有没有跳变、是不是每帧都在动、有没有糊到左邻窗格的终端上。做法和
`overlay_probe.py` 一样:让程序在指定毫秒各导一帧(`VECTORTERM_DUMP_AT`),
再拼成横向连拍图,同时量一下面板右边缘每一帧走到哪 —— 位移得**单调**。

触发靠两个诊断钩子:
  `VECTORTERM_AUTO_FILES=side|full[@毫秒]`  到点打开(默认 1200ms)
  `VECTORTERM_AUTO_FILES_CLOSE=毫秒`        到点收起(测退场)

量测口径:面板底色和终端底色是同一个色(都取 palette.bg),所以不能靠颜色
找边界,只能找**墨迹**(文字、标题下细线、右边那条竖线)。打开前那块区域
已经被布局让空,除了面板什么都没有,于是"墨迹最右列"就是面板的右边缘。
最终宽度从最后一帧自测出来,不用在脚本里写死列宽。

用法(项目根目录):
  python tools/diagnostics/files_slide_probe.py side
  python tools/diagnostics/files_slide_probe.py side --close-at 2000
  python tools/diagnostics/files_slide_probe.py full --zoom 2
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
TMP = ROOT / "target" / "_files_probe"

# 进场 190ms / 退场 160ms(见 main.rs 的 OverlayAnim),两串时刻照它排。
# 间隔别小于 20ms:一次重绘约 25ms,挨太近的几个时刻会落在同一帧上,
# 导出来的帧完全一样,看着像"动画卡住了"。
IN_FRAMES = "auto+4,auto+12,auto+22,auto+34,auto+50,auto+75,auto+120,auto+220"
OUT_FRAMES = "close+6,close+30,close+56,close+84,close+120,close+170"
# 单独再拍一张"落定"的:量最终宽度、当连拍图的参照
SETTLED = "auto+400"


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


def run(exe: pathlib.Path, cfg: pathlib.Path, mode: str, open_at: int,
        close_at: int | None, times: list[int], base: pathlib.Path) -> list[pathlib.Path]:
    env = dict(os.environ)
    for k in ("VECTORTERM_AUTOTYPE", "VECTORTERM_AUTO_SPLIT", "VECTORTERM_AUTO_PANEL",
              "VECTORTERM_AUTO_FOCUS", "VECTORTERM_DUMP_TRAIL"):
        env.pop(k, None)
    env["VECTORTERM_AUTO_FILES"] = f"{mode}@{open_at}"
    env["VECTORTERM_DUMP_FRAME"] = str(base)
    env["VECTORTERM_DUMP_AT"] = ",".join(str(t) for t in times)
    if close_at is not None:
        env["VECTORTERM_AUTO_FILES_CLOSE"] = str(close_at)

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


# 侧边栏右边那条竖线的颜色(files::Theme::border = SPLIT_DIVIDER_COLOR)
DIVIDER = (50, 50, 54)


def edge_x(img: Image.Image, limit_x: int) -> int:
    """侧边栏右边缘的 x(面板右边那条 1px 竖线的位置),没有则返回 0。

    为什么不量"墨迹最右列":面板底色和终端底色是同一个色,而终端背后还有壁纸,
    退场时壁纸一露出来,最亮/最暗的列全变了,量出来是个跳来跳去的数。竖线的颜色
    反而是个常量 (50,50,54),整列七百行行行都是它 —— 全屏按这个找,进也准退也准。
    """
    w, h = img.size
    px = img.load()
    y0, y1 = int(h * 0.12), int(h * 0.92)  # 跳过顶部标签栏、底部状态栏
    rows = (y1 - y0) // 2 or 1
    best_x, best_n = 0, 0
    for x in range(0, min(limit_x, w)):
        n = 0
        for y in range(y0, y1, 2):
            r, g, b = px[x, y]
            if abs(r - 50) <= 6 and abs(g - 50) <= 6 and abs(b - 54) <= 6:
                n += 1
        if n > best_n:
            best_n, best_x = n, x
    # 一半以上的行都是这条线的颜色才算数;否则就是没面板
    return best_x if best_n * 2 > rows else 0


def strip(frames: list[tuple[int, pathlib.Path]], width: int, zoom: int,
          out: pathlib.Path, marks: dict[int, int]) -> pathlib.Path:
    """裁到左侧面板所在的那一块,横向拼成一张图,标上毫秒数与实测右边缘。"""
    ims = [(t, Image.open(p).convert("RGB")) for t, p in frames]
    h = ims[0][1].size[1]
    box = (0, 0, min(width, ims[0][1].size[0]), min(h, 420))
    cw, ch = box[2] - box[0], box[3] - box[1]
    tag_h = 18
    canvas = Image.new("RGB", (cw * zoom, (ch * zoom + tag_h) * len(ims)), (8, 10, 16))
    draw = ImageDraw.Draw(canvas)
    for i, (t, im) in enumerate(ims):
        crop = im.crop(box).resize((cw * zoom, ch * zoom), Image.LANCZOS)
        y = i * (ch * zoom + tag_h)
        canvas.paste(crop, (0, y + tag_h))
        edge = marks.get(t)
        tag = f"t={t}ms" + (f"  edge={edge}px" if edge is not None else "")
        draw.text((4, y + 4), tag, fill=(150, 170, 210))
    out.parent.mkdir(parents=True, exist_ok=True)
    canvas.save(out)
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("mode", choices=["side", "full"], help="拍哪种面板")
    ap.add_argument("--mode-bin", default=None, choices=["release", "debug"],
                    help="(兼容旧写法)可执行文件用哪个构建")
    ap.add_argument("--build", default="release", choices=["release", "debug"])
    ap.add_argument("--open-at", type=int, default=1200)
    ap.add_argument("--close-at", type=int, default=None)
    ap.add_argument("--frames", default=None,
                    help=f"默认进场 {IN_FRAMES};给了 --close-at 就再加退场 {OUT_FRAMES}")
    ap.add_argument("--zoom", type=int, default=1)
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    exe = ROOT / "target" / args.build / "VectorTerm.exe"
    if not exe.is_file():
        sys.exit(f"缺少 {exe},先跑 build.cmd --release")
    TMP.mkdir(parents=True, exist_ok=True)
    for old in TMP.glob("f-*.bmp"):
        old.unlink()

    spec = args.frames
    if spec is None:
        spec = IN_FRAMES + ("," + OUT_FRAMES if args.close_at is not None else "")
    times = parse_frames(spec, args.open_at, args.close_at)
    settled_at = parse_frames(SETTLED, args.open_at, args.close_at)[0]
    all_times = sorted(set(times + [settled_at]))

    base = TMP / "f.bmp"
    paths = run(exe, THEME, args.mode, args.open_at, args.close_at, all_times, base)
    path_of = dict(zip(all_times, paths))

    # 用"落定"那一帧自测面板宽度:窗口左边缘起、墨迹最右列就是它
    settled = Image.open(path_of[settled_at]).convert("RGB")
    scan_limit = int(settled.size[0] * 0.6)
    width = edge_x(settled, scan_limit)
    marks = {}
    for t in times:
        im = Image.open(path_of[t]).convert("RGB")
        # 打开前 / 关完之后那块区域是空的,量出来会是 0(或噪声),这正是判据
        marks[t] = edge_x(im, scan_limit)
    if args.mode == "side" and width <= 0:
        sys.exit("落定帧里找不到侧边栏右边那条竖线,检查 VECTORTERM_AUTO_FILES / 主题")

    out = pathlib.Path(args.out or (TMP / f"{args.mode}_strip.png"))
    # 侧边栏裁到它自己那一条(full 模式没有"那一条",整屏裁下来才看得出滑到哪)
    crop_w = width + 70 if width > 0 else min(settled.size[0], 900)
    p = strip([(t, path_of[t]) for t in times], crop_w, args.zoom, out,
              marks if args.mode == "side" else {})
    print(f"侧边栏最终右边缘 x={width}px;{len(times)} 帧 -> {p}")
    if args.mode == "full":
        print("(full 模式铺满内容区、没有右边那条竖线,下面的数字仅供参考,看连拍图)")

    ins = [t for t in times if t <= settled_at]
    print("右边缘:", " ".join(f"{t}:{marks[t]}" for t in sorted(marks)))
    vals = [marks[t] for t in sorted(ins)]
    dup = sum(1 for a, b in zip(vals, vals[1:]) if a == b and a != 0)
    if dup:
        print(f"提示:{dup} 对相邻帧数值相同 —— 多半是采点比一次重绘还密,被并成同一帧了")
    growing = all(b >= a - 2 for a, b in zip(vals, vals[1:]))
    print("进场单调右移:", "是" if growing else "否 !!")
    if args.close_at is not None:
        outs = [t for t in times if t >= args.close_at]
        ov = [marks[t] for t in sorted(outs)]
        shrinking = all(b <= a + 2 for a, b in zip(ov, ov[1:]))
        print("退场单调左移:", "是" if shrinking else "否 !!")
        if not shrinking:
            return 1
    if not growing:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
