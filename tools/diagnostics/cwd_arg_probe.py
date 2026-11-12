r"""实机验证 --cwd:右键菜单把文件夹带进新窗口的那条链路。

资源管理器右键菜单注册的命令是 `VectorTerm.exe --cwd "<文件夹>"`,
这里直接按同样的命令行拉起窗口,检查 shell 启动后到底落在哪个目录。

场景:
1. 普通路径(带空格 + 中文)
2. 盘根 —— 命令行里 `--cwd "D:\\"` 会被解析成 `D:"`,看程序能不能补回来
3. 不存在的目录 —— 回落默认目录,窗口照常起来

参数按注册表里那一份原样传:`--cwd "<目录>\."`(收尾的 `.` 是绕开命令行转义用的,
见 installer\VectorTerm.iss 与 src/main.rs 的 tidy_cwd_arg)。

判据不用「自动输入」(VECTORTERM_AUTOTYPE 2.5s 后才敲,依赖窗口事件循环,
无人值守时可能压根不触发),而是让 cmd 用 `/k` 在启动时就把 cwd 写进文件:
`shell_args = { "/k", "cd > <文件>" }`,一次到位、与 UI 时序无关。

用法: python tools/diagnostics/cwd_arg_probe.py
"""
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
EXE = ROOT / "target" / "release" / "VectorTerm.exe"
WORK = ROOT / "target" / "cwd_arg_probe"
LOG = Path(os.path.expanduser("~/.config/VectorTerm/VectorTerm.log"))
# shell 启动时要写一个证明文件:`/k "cd> <文件>"` 这串是 cmd 自己重新解析的,
# 路径里带空格就得嵌套引号,极易解析错 —— 所以落在无空格的临时目录里。
PROOF_DIR = Path(os.environ.get("USERPROFILE", "C:/")) / "AppData" / "Local" / "Temp"

CONFIG = """return {{
  shell = "C:\\\\Windows\\\\System32\\\\cmd.exe",
  shell_args = {{ "/k", "cd> {out}" }},
  window_width = 900,
  window_height = 520,
  session_persistence = false,
}}
"""


def launch(tag, cwd_arg):
    """跑一次窗口,返回(日志增量, shell 报告的 cwd)。"""
    WORK.mkdir(parents=True, exist_ok=True)
    PROOF_DIR.mkdir(parents=True, exist_ok=True)
    out = PROOF_DIR / f"vectorterm_cwd_{tag}.txt"
    if out.exists():
        out.unlink()
    cfg = WORK / f"{tag}.lua"
    # 路径要进 Lua 字符串:反斜杠必须成对,否则 `\U` / `\D` 这种非法转义会让
    # 整份配置解析失败(窗口会静默回落到用户默认配置 —— 排查时极难看出来)
    cfg.write_text(CONFIG.format(out=str(out).replace("\\", "\\\\")), encoding="utf-8")

    offset = LOG.stat().st_size if LOG.exists() else 0
    env = dict(os.environ)
    env.pop("VECTORTERM_AUTOTYPE", None)
    argv = [str(EXE), "--config", str(cfg)]
    if cwd_arg is not None:
        argv += ["--cwd", cwd_arg]

    p = subprocess.Popen(argv, cwd=str(ROOT), env=env,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    deadline = time.time() + 15
    while time.time() < deadline and not out.exists():
        time.sleep(0.2)
    time.sleep(0.3)
    p.terminate()
    try:
        p.wait(timeout=5)
    except subprocess.TimeoutExpired:
        p.kill()
    time.sleep(0.3)

    log_new = ""
    if LOG.exists():
        with LOG.open("rb") as f:
            f.seek(offset)
            log_new = f.read().decode("utf-8", errors="replace")

    # cmd 写文件用的是控制台代码页(简中 = 936),别拿 UTF-8 硬解
    if out.exists():
        raw = out.read_bytes()
        for enc in ("gbk", "utf-8"):
            try:
                body = raw.decode(enc).strip()
                break
            except UnicodeDecodeError:
                continue
        else:
            body = "<解不开>"
    else:
        body = "<没写出来>"
    return log_new, body


def check(name, got, want):
    ok = got.lower().rstrip("\\") == want.lower().rstrip("\\")
    print(f"  {'PASS' if ok else 'FAIL'} {name}: shell 落在 {got!r}(期望 {want!r})")
    return ok


def main():
    if not EXE.exists():
        sys.exit(f"找不到 {EXE}(先 cargo build --release)")

    results = []

    # 1. 带空格和中文的普通目录 —— 右键菜单最常见的那一下
    target = WORK / "测试 目录"
    target.mkdir(parents=True, exist_ok=True)
    log, body = launch("plain", str(target) + "\\.")
    print("=== 1. 普通路径 --cwd ===")
    print("   日志:", [l for l in log.splitlines() if "工作目录" in l] or "无")
    results.append(check("带空格中文路径", body, str(target)))

    # 2. 盘根:命令行里 `--cwd "C:\"` 会被解析成 `C:"`
    print("=== 2. 盘根 ===")
    log, body = launch("root", "C:\\" + "\\.")
    print("   日志:", [l for l in log.splitlines() if "工作目录" in l] or "无")
    results.append(check("盘根 C:\\", body, "C:\\"))

    # 3. 不存在的目录:应该回落,窗口照常起来
    log, body = launch("bad", "Z:\\no\\such\\dir")
    print("=== 3. 不存在的目录 ===")
    print("   日志:", [l for l in log.splitlines() if "工作目录" in l or "不可用" in l] or "无")
    ok = body not in ("<没写出来>", "<解不开>") and not body.lower().startswith("z:")
    print(f"  {'PASS' if ok else 'FAIL'} 回落默认目录: {body!r}")
    results.append(ok)

    print(f"\n=== {sum(results)}/{len(results)} PASS ===")
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
