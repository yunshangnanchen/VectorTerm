"""实机验证:内嵌字体(wezterm 同款)与彩色 emoji 是否真的画出来。

默认配置现在是 JetBrains Mono + Nerd Font Symbols + (彩色路径)Noto Color Emoji,
全部编进 exe。这个探针启动一个**不带 font 表**的配置,让默认值完全生效,
在终端里打印 ASCII / 中文 / emoji,再用 `VECTORTERM_DUMP_FRAME` 让程序自导出
一帧(不用 PrintWindow,窗口被遮挡时拍到的是别的窗口)。

判据:
  - 内容区出现成片的"非灰"像素 → 彩色 emoji 位图确实进了显示列表;
  - 内容区有足够墨迹 → 主字体与中文回退都加载成功。

用法: python tools/diagnostics/font_probe.py [debug|release]
"""
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
MODE = sys.argv[1] if len(sys.argv) > 1 else "debug"
EXE = ROOT / "target" / MODE / "VectorTerm.exe"
OUT = ROOT / "target" / "font_probe"
OUT.mkdir(parents=True, exist_ok=True)

CFG = """return {
  initial_cols = 100,
  initial_rows = 26,
  shell = "cmd.exe",
  shell_args = { "/k", "title font-probe" },
  status_bar = { enabled = false },
}
"""
cfg_path = OUT / "probe.lua"
cfg_path.write_text(CFG, encoding="utf-8")

env = dict(os.environ)
for k in list(env):
    if k.startswith("VECTORTERM_"):
        env.pop(k)
env["VECTORTERM_AUTOTYPE"] = (
    "chcp 65001 > nul\\r"
    "echo JetBrains Mono 12345 ABC abc  \\r"
    "echo 中文测试 终端字体  \\r"
    "echo EMOJI: 😀 🎉 🔥 ✅ 🛑  \\r"
)
frame = OUT / "frame.bmp"
env["VECTORTERM_DUMP_FRAME"] = str(frame)
env["VECTORTERM_DUMP_AT"] = "4600"

p = subprocess.Popen([str(EXE), "--config", str(cfg_path)], cwd=str(ROOT), env=env,
                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
try:
    time.sleep(5.4)
finally:
    p.terminate()
    try:
        p.wait(timeout=3)
    except subprocess.TimeoutExpired:
        p.kill()

from PIL import Image

candidates = [frame, OUT / "frame-4600.bmp"]
path = next((q for q in candidates if q.exists()), None)
if path is None:
    sys.exit("没有导出帧,检查 VECTORTERM_DUMP_AT / VECTORTERM_DUMP_FRAME")

im = Image.open(path).convert("RGB")
w, h = im.size
px = im.load()
bg = px[w - 4, h - 4]
ink = 0
colored = 0
for y in range(int(h * 0.05), int(h * 0.45)):
    for x in range(int(w * 0.02), int(w * 0.98)):
        r, g, b = px[x, y]
        if abs(r - bg[0]) + abs(g - bg[1]) + abs(b - bg[2]) > 40:
            ink += 1
            if max(r, g, b) - min(r, g, b) > 45 and max(r, g, b) > 90:
                colored += 1

im.crop((0, 0, min(w, 900), min(h, 260))).save(OUT / "view.png")
print(f"帧: {path.name} {w}x{h}")
print(f"内容区墨迹={ink}  彩色像素={colored}")
ok = ink > 500 and colored > 60
print("结果:", "PASS" if ok else "FAIL")
sys.exit(0 if ok else 1)
