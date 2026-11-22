"""半透明磨砂主题的实拍验证。

做的事:生成一块高彩度背景图 → 用它铺一个"背后窗口" → 把 deepseek-glass
主题的窗口盖在上面 → **截屏**(不是导自己的帧)看桌面/背后的颜色有没有透上来。

  a_back      只有背后那块彩图窗口(基准色)
  b_glass     盖上半透明主题窗口
  c_front     前景窗口自己的帧(看主题本身的配色)

用法:python tools/diagnostics/glass_theme_probe.py
产物:target/_glass/*.png
"""
import os
import pathlib
import random
import subprocess
import sys
import time

from PIL import Image, ImageDraw, ImageFilter

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from acrylic_key_probe import grab, move_window, window_of_pid, user32  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parents[2]
EXE = ROOT / "target" / "release" / "VectorTerm.exe"
OUT = ROOT / "target" / "_glass"

BACK_W, BACK_H = 1120, 700
BACK_AT = (90, 90)
FRONT_W, FRONT_H = 900, 560
FRONT_AT = (300, 190)


def make_backdrop(path):
    """高彩度背景:蓝→紫→粉的斜向渐变 + 几个模糊光斑。"""
    w, h = BACK_W, BACK_H
    im = Image.new("RGB", (w, h))
    d = ImageDraw.Draw(im)
    for y in range(h):
        t = y / h
        d.line([(0, y), (w, y)], fill=(int(30 + 60 * t), int(90 + 40 * t), int(200 - 40 * t)))
    for i in range(w):
        t = i / w
        d.line([(i, 0), (i, h)], fill=None)  # 占位,下面的光斑覆盖
    lay = Image.new("RGB", (w, h), (18, 40, 120))
    ld = ImageDraw.Draw(lay)
    for y in range(h):
        t = y / h
        ld.line([(0, y), (w, y)], fill=(int(40 + 20 * t), int(60 + 90 * t), int(190 + 40 * t)))
    lay = lay.convert("RGBA")
    blobs = [((160, 120, 320), (255, 90, 140, 190)),
             ((760, 180, 300), (120, 230, 255, 170)),
             ((520, 560, 380), (170, 120, 255, 180)),
             ((980, 600, 260), (255, 210, 90, 150))]
    for (cx, cy, r), color in blobs:
        layer = Image.new("RGBA", (w, h), (0, 0, 0, 0))
        ImageDraw.Draw(layer).ellipse([cx - r, cy - r, cx + r, cy + r], fill=color)
        lay = Image.alpha_composite(lay, layer.filter(ImageFilter.GaussianBlur(60)))
    lay.convert("RGB").save(path)
    return path


BACK_CFG = """return {
  shell = "pwsh.exe",
  shell_args = { "-NoLogo", "-NoProfile", "-NoExit" },
  render = "cpu",
  font = { size = 14 },
  padding = 0,
  window_decorations = "RESIZE",
  window_width = __W__,
  window_height = __H__,
  window_background_image = "__IMG__",
  window_background_fill = "cover",
  tab_bar = { enable = false },
  colors = { background = "#101a2e", foreground = "#ffffff" },
  session_log = "__LOG__",
}
"""

FRONT_CFG = """return {
  theme = "deepseek-glass",
  window_width = __W__,
  window_height = __H__,
  shell = "pwsh.exe",
  shell_args = { "-NoLogo", "-NoProfile", "-NoExit", "-Command",
                 "Get-ChildItem | Format-Table -AutoSize" },
  session_log = "__LOG__",
}
"""


def launch(cfg_text, name, at, size, dump=None):
    cfg = OUT / f"{name}.lua"
    cfg.write_text(cfg_text, encoding="utf-8")
    env = dict(os.environ)
    for k in list(env):
        if k.startswith("VECTORTERM_"):
            env.pop(k, None)
    if dump:
        env["VECTORTERM_DUMP_AT"] = "3600"
        env["VECTORTERM_DUMP_FRAME"] = str(dump)
    p = subprocess.Popen([str(EXE), "--config", str(cfg)], env=env,
                         stdout=subprocess.DEVNULL,
                         stderr=open(OUT / f"{name}.trace.log", "wb"))
    time.sleep(2.3)
    h = window_of_pid(p.pid)
    if h:
        move_window(h, at[0], at[1], size[0], size[1])
    return p, h


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    img = make_backdrop(OUT / "backdrop.png")

    back, bh = launch(BACK_CFG.replace("__W__", str(BACK_W)).replace("__H__", str(BACK_H))
                          .replace("__IMG__", img.as_posix())
                          .replace("__LOG__", (OUT / "back.bin").as_posix()),
                      "back", BACK_AT, (BACK_W, BACK_H))
    time.sleep(0.9)
    # 先截"只有背景"的基准图,再让玻璃窗口盖上来
    rect = (BACK_AT[0], BACK_AT[1],
            BACK_AT[0] + BACK_W, BACK_AT[1] + BACK_H)
    grab(*rect).save(OUT / "a_back.png")

    front, fh = launch(FRONT_CFG.replace("__W__", str(FRONT_W)).replace("__H__", str(FRONT_H))
                           .replace("__LOG__", (OUT / "front.bin").as_posix()),
                       "front", FRONT_AT, (FRONT_W, FRONT_H),
                       dump=OUT / "c_front.bmp")
    time.sleep(1.4)
    user32.SetForegroundWindow(fh)
    if fh:
        move_window(fh, FRONT_AT[0], FRONT_AT[1], FRONT_W, FRONT_H)
    time.sleep(0.9)
    grab(*rect).save(OUT / "b_glass.png")

    front.terminate()
    back.terminate()
    for p in (front, back):
        try:
            p.wait(timeout=8)
        except subprocess.TimeoutExpired:
            p.kill()

    if (OUT / "c_front.bmp").exists():
        Image.open(OUT / "c_front.bmp").save(OUT / "c_front.png")

    # 量化:同一坐标(窗口中部)在"纯背景"和"盖上玻璃"两张图里的颜色差
    a = Image.open(OUT / "a_back.png").convert("RGB")
    b = Image.open(OUT / "b_glass.png").convert("RGB")
    pa, pb = a.load(), b.load()
    # 窗口在 crop 坐标里是 (210,100)-(1110,660)
    pts = [(300, 300), (700, 430), (900, 200)]
    for x, y in pts:
        print(f"  ({x},{y}) 背景 {pa[x, y]} → 玻璃窗口上 {pb[x, y]}", flush=True)


if __name__ == "__main__":
    main()
