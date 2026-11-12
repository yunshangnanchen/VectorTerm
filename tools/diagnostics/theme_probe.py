"""内置主题实机回归:导出真实帧,核对 DeepSeek(夜空蓝)配色 + 去掉右侧滚动条。

用 VECTORTERM_DUMP_FRAME 直接拿 CPU 重绘的 framebuffer,避开屏幕截图在
高 DPI 下的坐标缩放。两份配置都放在临时目录里,子进程 cwd 也设到那里,
所以 theme = "deepseek" 只能靠 **<exe 目录>/config/deepseek.lua** 命中 ——
顺带证明主题确实被随发布版本分发到了正确位置。

A: theme = "deepseek"                     → 夜空蓝主题,右侧无滚动条
B: 同上但显式把滚动条开成洋红常显         → 右侧出现洋红带(证明是主题关掉的)
"""
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent.parent
MODE = sys.argv[1] if len(sys.argv) > 1 else "release"
EXE = ROOT / "target" / MODE / "VectorTerm.exe"
OUT = ROOT / "target"
SHIPPED = ROOT / "target" / MODE / "config" / "deepseek.lua"
SRC = ROOT / "config" / "deepseek.lua"

KEEP = "ping -n 25 127.0.0.1 >nul"


def frame(tag, cfg_text, workdir):
    """启动一次,拿一帧 BMP,返回 PIL Image。"""
    bmp = OUT / f"theme_{tag}.bmp"
    if bmp.exists():
        bmp.unlink()
    cfg = Path(workdir) / f"theme_{tag}.lua"
    cfg.write_text(cfg_text, encoding="utf-8")
    env = dict(os.environ)
    env["VECTORTERM_DUMP_FRAME"] = str(bmp)
    for k in ("VECTORTERM_AUTO_SPLIT", "VECTORTERM_DUMP_AT", "VECTORTERM_AUTO_FILES"):
        env.pop(k, None)
    proc = subprocess.Popen(
        [str(EXE), "--config", str(cfg)],
        cwd=str(workdir), env=env,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    try:
        for _ in range(100):
            time.sleep(0.2)
            if bmp.exists() and bmp.stat().st_size > 10000:
                time.sleep(0.5)
                break
        if not bmp.exists():
            raise RuntimeError(f"{tag}: 没拿到帧导出")
        im = Image.open(bmp).convert("RGB")
        return im, bmp
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()


def count(im, rgb, tol=1):
    """精确取色。旧画布 #0E0E10 和默认底色 #0c0c0c 只差 2~4,容差放宽会互相串。"""
    px = im.load()
    w, h = im.size
    n = 0
    for y in range(0, h, 2):
        for x in range(0, w, 2):
            r, g, b = px[x, y]
            if abs(r - rgb[0]) <= tol and abs(g - rgb[1]) <= tol and abs(b - rgb[2]) <= tol:
                n += 1
    return n


def count_top(im, rgb, y_max, tol=1):
    """只数窗口顶部一带:证明活动标签本身是实心品牌蓝,而不只是光标蓝。"""
    px = im.load()
    w, h = im.size
    return sum(
        1
        for y in range(0, min(y_max, h), 2)
        for x in range(0, w, 2)
        if all(abs(px[x, y][i] - rgb[i]) <= tol for i in range(3))
    )


def magenta(im):
    """滚动条有 opacity(滑块 0.85 / 轨道再乘 0.45),纯 #ff00ff 会被混成
    (205,3,205) / (106,9,107)。用「红蓝高、绿低」判色调而不是精确取色。"""
    px = im.load()
    w, h = im.size
    return sum(
        1
        for y in range(0, h, 2)
        for x in range(0, w, 2)
        if px[x, y][0] > 70 and px[x, y][2] > 70 and px[x, y][1] < 55
    )


def main():
    if not EXE.is_file():
        print(f"FAIL: 找不到 {EXE}")
        return 1
    # 模拟 build.cmd 第 5b 段的产物:主题文件应出现在 <profile>/config/
    SHIPPED.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(SRC, SHIPPED)
    print(f"shipped theme: {SHIPPED}  ({SHIPPED.stat().st_size} bytes)")

    work = tempfile.mkdtemp(prefix="vectorterm-theme-probe-")
    common = f'default_prog = {{ "cmd.exe", "/c", "echo VectorTerm & {KEEP}" }},'
    a = "return { " + common + ' theme = "deepseek", }\n'
    b = ("return { " + common + ' theme = "deepseek", '
         'scrollbar = { enabled = true, show = "always", show_track = true, '
         'track_color = "#ff00ff", thumb_color = "#ff00ff" }, }\n')
    try:
        im_a, bmp_a = frame("deepseek", a, work)
        im_b, bmp_b = frame("control", b, work)
        print(f"frames: {im_a.size} / {im_b.size}  -> {bmp_a.name}, {bmp_b.name}")

        canvas = count(im_a, (11, 18, 32))       # #0B1220 夜空蓝画布(tmpceshi DARK)
        old_bg = count(im_a, (12, 12, 12))       # #0c0c0c 默认背景
        old_ds = count(im_a, (14, 14, 16))       # #0E0E10 上一版官方近黑画布
        blue = count(im_a, (77, 107, 254))       # #4D6BFE 实心活动标签 + 品牌蓝光标
        blue_tab = count_top(im_a, (77, 107, 254), 60)
        mag_a = magenta(im_a)
        mag_b = magenta(im_b)
        print(f"A: 画布={canvas} 默认底色={old_bg} 旧官方底色={old_ds} "
              f"品牌蓝={blue} 顶栏品牌蓝={blue_tab} 洋红={mag_a}")
        print(f"B: 洋红(常显滚动条)={mag_b}")

        checks = {
            "主题底色铺满画布(#0B1220)": canvas > 20000,
            "没有退回默认底色(#0c0c0c)": old_bg < 200,
            "没有退回上一版近黑底色(#0E0E10)": old_ds < 200,
            "活动标签是实心品牌蓝(#4D6BFE)": blue_tab > 100,
            "输入光标/强调色为品牌蓝(#4D6BFE)": blue > 200,
            "主题未画右侧滚动条": mag_a < 50,
            "对照组能画出常显滚动条": mag_b > 300,
        }
        ok = True
        for k, v in checks.items():
            print(("OK   " if v else "FAIL ") + k)
            ok &= v
        return 0 if ok else 1
    finally:
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
