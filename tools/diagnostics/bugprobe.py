"""定位"首行输入错位"的责任方:四组对照,每组在 2.5s 自动敲 abc,3.2s 导帧。

A 只有 fastfetch(带 kitty 立绘) + 朴素 prompt
B fastfetch --logo none(无图) + 朴素 prompt
C 只有 oh-my-posh prompt
D 真实组合(oh-my-posh + fastfetch)
"""
import os
import pathlib
import subprocess
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
EXE = ROOT / "target" / "release" / "VectorTerm.exe"
PROBE = ROOT / "target" / "bugprobe"
PROBE.mkdir(exist_ok=True)
OMP = r"C:\Users\DYBAG\oh-my-posh-themes\catppuccin.omp.json"

cases = {
    "A_ff_img": ["-NoLogo", "-NoProfile", "-NoExit", "-Command", "fastfetch"],
    "B_ff_noimg": ["-NoLogo", "-NoProfile", "-NoExit", "-Command",
                   "fastfetch --logo none"],
    "C_omp": ["-NoLogo", "-NoProfile", "-NoExit", "-Command",
              "oh-my-posh init pwsh --config '%s' | Invoke-Expression" % OMP],
    "D_both": ["-NoLogo", "-NoProfile", "-NoExit", "-Command",
               "oh-my-posh init pwsh --config '%s' | Invoke-Expression; fastfetch" % OMP],
}


def esc(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def run(name, args):
    cfg = PROBE / f"{name}.lua"
    lua = 'return {\n  shell_args = { %s },\n  render = "cpu",\n  font = { size = 16 },\n}\n' % (
        ", ".join(esc(a) for a in args)
    )
    cfg.write_text(lua, encoding="utf-8")
    bmp = ROOT / "target" / f"bug_{name}.bmp"
    if bmp.exists():
        bmp.unlink()
    env = dict(os.environ)
    env["VECTORTERM_DUMP_FRAME"] = str(bmp)
    env["VECTORTERM_DUMP_AT"] = "3200"
    env["VECTORTERM_AUTOTYPE"] = "abc"
    p = subprocess.Popen([str(EXE), "--config", str(cfg)], env=env,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(7)
    p.terminate()
    try:
        p.wait(timeout=3)
    except subprocess.TimeoutExpired:
        p.kill()
    print(name, "frame" if bmp.exists() else "NO FRAME", flush=True)


for n, a in cases.items():
    run(n, a)
