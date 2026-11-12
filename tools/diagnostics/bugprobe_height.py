"""图片高度 vs 输入错位:三档 logo 高度,统一 -NoProfile,量 abc 与提示符是否同一行。

配置里路径一律用正斜杠并放到无空格目录 —— 上一版把 Windows 路径直接塞进
Lua 双引号字符串,内层引号提前闭合导致配置解析失败,三组实际都落回默认
配置,实验作废。
"""
import os
import pathlib
import re
import shutil
import subprocess
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
EXE = ROOT / "target" / "release" / "VectorTerm.exe"
PROBE = ROOT / "target" / "bugprobe"
SRC = pathlib.Path(os.path.expanduser("~")) / ".config" / "fastfetch" / "config.jsonc"
base = SRC.read_text(encoding="utf-8")

# fastfetch --config 的路径不能带空格(Lua 字符串里没法舒服地套引号)
TMP = pathlib.Path(tempfile.gettempdir()) / "ffprobe"
TMP.mkdir(exist_ok=True)

pat = re.compile(
    r'("source":\s*"[^"]*logo-kitty\.png",\s*"type":\s*"kitty-direct",\s*"width":\s*)\d+(,\s*"height":\s*)\d+'
)

CASES = [("off", None), ("h10", 10), ("h25", 25), ("h40", 40)]

for name, rows in CASES:
    if rows is None:
        ff = 'fastfetch --logo none'
    else:
        cols = round(29 / 25 * rows)
        ffcfg = TMP / f"ff{rows}.jsonc"
        ffcfg.write_text(pat.sub(rf"\g<1>{cols}\g<2>{rows}", base), encoding="utf-8")
        ff = f'fastfetch --config {ffcfg.as_posix()}'

    cfg = PROBE / f"n_{name}.lua"
    cfg.write_text(
        'return {\n  shell_args = { "-NoLogo", "-NoProfile", "-NoExit", "-Command", "%s" },\n'
        '  render = "cpu",\n  font = { size = 16 },\n}\n' % ff,
        encoding="utf-8",
    )
    bmp = ROOT / "target" / f"bug_n_{name}.bmp"
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
    print(f"{name:<5} {ff}", "ok" if bmp.exists() else "NO FRAME", flush=True)
