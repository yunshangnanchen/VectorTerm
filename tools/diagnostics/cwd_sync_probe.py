"""实机验证:文件面板与终端目录的双向同步。

场景:
1. cmd —— 在终端里 `cd /d C:\\Windows`,面板应该跟着换过去(靠读进程 PEB)
2. cmd —— 在面板里进 C:\\Windows,终端应该跟着 cd(靠往 shell 写 cd)
3. PowerShell —— 靠自动注入的 prompt 钩子上报 OSC 7,面板同样跟着走

用法: python target/cwd_sync_probe.py [release|debug]
"""
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
PROFILE = sys.argv[1] if len(sys.argv) > 1 else "release"
EXE = ROOT / "target" / PROFILE / "VectorTerm.exe"
WORK = ROOT / "target" / "cwd_sync_probe"
LOG = Path(os.path.expanduser("~/.config/VectorTerm/VectorTerm.log"))

CONFIG = """return {{
  shell = "{shell}",
  {args}
  files_gui = {{ follow_cwd = true, push_cwd = true }},
  window_width = 1100,
  window_height = 620,
}}
"""


def write_config(name, shell, args="", extra=""):
    WORK.mkdir(parents=True, exist_ok=True)
    p = WORK / f"{name}.lua"
    p.write_text(CONFIG.format(shell=shell, args=args) + extra, encoding="utf-8")
    return p


def run(tag, cfg, env_extra, wait):
    """跑一次窗口,返回这期间新增的日志行。"""
    offset = LOG.stat().st_size if LOG.exists() else 0
    env = dict(os.environ)
    env.update(env_extra)
    env.pop("VECTORTERM_AUTO_SPLIT", None)
    p = subprocess.Popen([str(EXE), "--config", str(cfg)], cwd=str(ROOT), env=env,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(wait)
    p.terminate()
    try:
        p.wait(timeout=5)
    except subprocess.TimeoutExpired:
        p.kill()
    time.sleep(0.4)
    if not LOG.exists():
        return []
    with LOG.open("rb") as f:
        f.seek(offset)
        text = f.read().decode("utf-8", errors="replace")
    return [ln for ln in text.splitlines() if "cwdsync" in ln or "注入 OSC 7" in ln or "改用" in ln]


def norm(text):
    """路径大小写 / 正反斜杠都不算差别。"""
    return text.replace("/", "\\").lower()


def check(name, lines, must, forbid=()):
    joined = norm("\n".join(lines))
    ok = all(norm(m) in joined for m in must)
    bad = [f for f in forbid if norm(f) in joined]
    print(f"[{name}] {'OK' if ok and not bad else 'FAIL'}")
    for ln in lines:
        print("   ", ln)
    if not ok:
        print(f"    缺少:{[m for m in must if norm(m) not in joined]}")
    if bad:
        print(f"    不该出现:{bad}")
    return ok and not bad


def main():
    if not EXE.exists():
        print(f"找不到 {EXE}")
        return 1
    results = []

    # 1) 终端 cd → 面板跟随(cmd / PEB)
    cfg = write_config("cmd_follow", "cmd.exe")
    lines = run("cmd_follow", cfg, {
        "VECTORTERM_DEBUG_CWD": "1",
        "VECTORTERM_AUTO_FILES": "side",
        "VECTORTERM_AUTO_FILES_DIR": "C:\\",
        "VECTORTERM_AUTOTYPE": r"cd /d C:\Windows\r",
    }, wait=7.0)
    results.append(check("cmd:终端 cd → 面板跟随", lines, ["终端 → 面板:C:\\Windows"]))

    # 2) 面板进目录 → 终端跟随(写 cd 进 shell)
    frame = WORK / "cmd_push.bmp"
    lines = run("cmd_push", cfg, {
        "VECTORTERM_DEBUG_CWD": "1",
        "VECTORTERM_AUTO_FILES": "side",
        "VECTORTERM_AUTO_FILES_DIR": "C:\\",
        "VECTORTERM_AUTO_FILES_OPEN": "Windows",
        "VECTORTERM_DUMP_FRAME": str(frame),
        "VECTORTERM_DUMP_AT": "5000",
    }, wait=7.0)
    results.append(check("cmd:面板进目录 → 终端 cd", lines,
                         ["面板 → 终端:C:\\Windows", "cd /d"]))
    dumped = WORK / "cmd_push.bmp"
    print(f"    帧:{dumped} ({'有' if dumped.exists() else '没有'})")

    # 3) PowerShell:注入 prompt 钩子 → OSC 7 → 面板跟随
    cfg = write_config("ps_follow", "pwsh.exe", 'shell_args = { "-NoLogo" },')
    lines = run("ps_follow", cfg, {
        "VECTORTERM_DEBUG_CWD": "1",
        "VECTORTERM_AUTO_FILES": "full",
        "VECTORTERM_AUTO_FILES_DIR": "C:\\",
        "VECTORTERM_AUTOTYPE": r"cd C:\Windows\r",
    }, wait=9.0)
    results.append(check("pwsh:终端 cd → 面板跟随(OSC 7)", lines,
                         ["注入 OSC 7", "终端 → 面板:C:\\Windows"]))

    print()
    print("总共:", f"{sum(1 for r in results if r)}/{len(results)} 通过")
    return 0 if all(results) else 1


if __name__ == "__main__":
    sys.exit(main())
