"""抓 PTY 原始字节:session_log 把终端收到的每个字节写文件(配置项,无需改代码)。"""
import os
import pathlib
import subprocess
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
EXE = ROOT / "target" / "release" / "VectorTerm.exe"
PROBE = ROOT / "target" / "bugprobe"

for name, ff in (("img", "fastfetch"), ("off", "fastfetch --logo none")):
    log = ROOT / "target" / f"pty_{name}.bin"
    if log.exists():
        log.unlink()
    cfg = PROBE / f"log_{name}.lua"
    cfg.write_text(
        'return {\n  shell_args = { "-NoLogo", "-NoProfile", "-NoExit", "-Command", "%s" },\n'
        '  render = "cpu",\n  font = { size = 16 },\n  session_log = "%s",\n}\n'
        % (ff, log.as_posix()),
        encoding="utf-8",
    )
    env = dict(os.environ)
    env["VECTORTERM_DUMP_FRAME"] = str(ROOT / "target" / f"pty_{name}.bmp")
    env["VECTORTERM_DUMP_AT"] = "3600"
    p = subprocess.Popen([str(EXE), "--config", str(cfg)], env=env,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(7)
    p.terminate()
    try:
        p.wait(timeout=3)
    except subprocess.TimeoutExpired:
        p.kill()
    print(name, log.stat().st_size if log.exists() else "NO LOG", flush=True)
