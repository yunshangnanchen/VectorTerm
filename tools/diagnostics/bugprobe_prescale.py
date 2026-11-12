"""验证「源图像素高度 <= 显示格数 × 格高」时错位是否消失。

思路:fastfetch 用 kitty-direct(t=f) 直接把文件路径发过去,终端拿**文件自身
像素尺寸**做 y_padding_shift 的比较;所以把 PNG 预缩到显示尺寸(29列×25行
≈ 319×525)以下,`draw_height + pad > 25*21` 就变成 false,不该再多下移一行。

对照组:
  orig  = 现在用的 logo-kitty.png(730×1250,必然触发 +1)
  small = 预缩到 300×500 的副本(两个阈值都不超)
"""
import os
import pathlib
import re
import subprocess
import tempfile
import time

from PIL import Image

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
EXE = ROOT / "target" / "release" / "VectorTerm.exe"
PROBE = ROOT / "target" / "bugprobe"
PROBE.mkdir(exist_ok=True)
HOME = pathlib.Path(os.path.expanduser("~"))
SRC = HOME / ".config" / "fastfetch" / "config.jsonc"
PNG = HOME / ".config" / "fastfetch" / "logo-kitty.png"
base = SRC.read_text(encoding="utf-8")

TMP = pathlib.Path(tempfile.gettempdir()) / "ffprobe"
TMP.mkdir(exist_ok=True)

small = TMP / "logo-small.png"
im = Image.open(PNG)
print("source png", im.size, flush=True)
im.resize((300, 500), Image.LANCZOS).save(small)
print("prescaled  ", Image.open(small).size, flush=True)

pat = re.compile(
    r'("source":\s*")[^"]*(",\s*"type":\s*"kitty-direct",\s*"width":\s*)\d+(,\s*"height":\s*)\d+'
)

CASES = {"orig": PNG.as_posix(), "small": small.as_posix()}

for name, src in CASES.items():
    ffcfg = TMP / f"ps_{name}.jsonc"
    ffcfg.write_text(pat.sub(rf"\g<1>{src}\g<2>29\g<3>25", base), encoding="utf-8")
    ff = f"fastfetch --config {ffcfg.as_posix()}"

    cfg = PROBE / f"ps_{name}.lua"
    cfg.write_text(
        'return {\n  shell_args = { "-NoLogo", "-NoProfile", "-NoExit", "-Command", "%s" },\n'
        '  render = "cpu",\n  font = { size = 16 },\n}\n' % ff,
        encoding="utf-8",
    )
    bmp = ROOT / "target" / f"bug_ps_{name}.bmp"
    if bmp.exists():
        bmp.unlink()
    env = dict(os.environ)
    env["VECTORTERM_DUMP_FRAME"] = str(bmp)
    env["VECTORTERM_DUMP_AT"] = "3600"
    env["VECTORTERM_AUTOTYPE"] = "abc"
    p = subprocess.Popen([str(EXE), "--config", str(cfg)], env=env,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(7)
    p.terminate()
    try:
        p.wait(timeout=3)
    except subprocess.TimeoutExpired:
        p.kill()
    print(f"{name:<6}", "ok" if bmp.exists() else "NO FRAME", flush=True)
