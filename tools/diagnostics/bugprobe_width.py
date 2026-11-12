"""宽度对照:同样带图,只改窗口宽度,看输入错位是否消失。

假设:fastfetch 按"终端宽 - 图片占列"排版,但 ConPTY 按整行重算宽度 -> 若干
行顶到右边界被自动折行,多占几行;图片是绝对定位的 overlay 不跟着走,于是
图片/文字/光标三者的行号分叉。
"""
import os
import pathlib
import subprocess
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
EXE = ROOT / "target" / "release" / "VectorTerm.exe"
PROBE = ROOT / "target" / "bugprobe"

for width in (1250, 1500, 1700):
    cfg = PROBE / f"w{width}.lua"
    cfg.write_text(
        'return {\n  shell_args = { "-NoLogo", "-NoProfile", "-NoExit", "-Command", "fastfetch" },\n'
        '  render = "cpu",\n  font = { size = 16 },\n'
        f'  window_width = {width},\n  window_height = 700,\n}}\n',
        encoding="utf-8",
    )
    bmp = ROOT / "target" / f"bug_w{width}.bmp"
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
    print(f"width={width}", "ok" if bmp.exists() else "NO FRAME", flush=True)
