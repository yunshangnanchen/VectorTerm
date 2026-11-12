"""让 ConPTY 自己报行号:在提示符后自动输入 [Console]::CursorTop 并回车。

对比"无图 / 有图"两组:fastfetch 输出的文字行数完全一样,若 conhost 报出来的
行号却差 1,说明图片在 ConPTY 的记账里多占了行(而终端没画出来),PSReadLine
据此定位就差了那一行。
"""
import os
import pathlib
import subprocess
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
EXE = ROOT / "target" / "release" / "VectorTerm.exe"
PROBE = ROOT / "target" / "bugprobe"

CASES = {
    "off": 'return {\n  shell_args = { "-NoLogo", "-NoProfile", "-NoExit", "-Command", "fastfetch --logo none" },\n  render = "cpu",\n  font = { size = 16 },\n}\n',
    "img": 'return {\n  shell_args = { "-NoLogo", "-NoProfile", "-NoExit", "-Command", "fastfetch" },\n  render = "cpu",\n  font = { size = 16 },\n}\n',
}

for name, lua in CASES.items():
    cfg = PROBE / f"ct_{name}.lua"
    cfg.write_text(lua, encoding="utf-8")
    bmp = ROOT / "target" / f"bug_ct_{name}.bmp"
    if bmp.exists():
        bmp.unlink()
    env = dict(os.environ)
    env["VECTORTERM_DUMP_FRAME"] = str(bmp)
    env["VECTORTERM_DUMP_AT"] = "4200"
    env["VECTORTERM_AUTOTYPE"] = "[Console]::CursorTop\\r"
    p = subprocess.Popen([str(EXE), "--config", str(cfg)], env=env,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(8)
    p.terminate()
    try:
        p.wait(timeout=3)
    except subprocess.TimeoutExpired:
        p.kill()
    print(name, "ok" if bmp.exists() else "NO FRAME", flush=True)
