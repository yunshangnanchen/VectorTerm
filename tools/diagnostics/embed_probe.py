"""内置主题实机回归:exe 旁边和 cwd 都**没有** config 目录时,主题要能出来。

四套配色已经 `include_str!` 编进 exe(`config.rs::BUILTIN_THEMES`),所以把
VectorTerm.exe 单独拷到一个空目录、cwd 也指向另一个空目录,配置里写
`theme = "chatgpt"` 仍应套上 ChatGPT 的纯灰阶配色:

  画布 #212121 / 活动标签 #ECECEC 底 / 状态栏有字(主题里的 wezterm.on 执行了)

只要画布退回默认底色 #0c0c0c,或者活动标签不是白底,就说明内嵌回退没生效。
"""
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent.parent
MODE = sys.argv[1] if len(sys.argv) > 1 else "release"
EXE = ROOT / "target" / MODE / "VectorTerm.exe"
OUT = ROOT / "target"


def frame(exe, tag, cfg_text, workdir):
    bmp = OUT / f"embed_{tag}.bmp"
    if bmp.exists():
        bmp.unlink()
    cfg = Path(workdir) / f"embed_{tag}.lua"
    cfg.write_text(cfg_text, encoding="utf-8")
    env = dict(os.environ)
    env["VECTORTERM_DUMP_FRAME"] = str(bmp)
    # 状态栏靠定时器派发 update-right-status,太早拍是空的 —— 等它走过一轮
    env["VECTORTERM_DUMP_AT"] = os.environ.get("VECTORTERM_DUMP_AT", "2500")
    for k in ("VECTORTERM_AUTO_SPLIT", "VECTORTERM_AUTO_FILES"):
        env.pop(k, None)
    proc = subprocess.Popen(
        [str(exe), "--config", str(cfg)],
        cwd=str(workdir), env=env,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    try:
        for _ in range(100):
            time.sleep(0.2)
            if bmp.exists() and bmp.stat().st_size > 10000:
                time.sleep(0.5)
                break
        if not bmp.exists():
            raise RuntimeError(f"{tag}: 没拿到帧导出")
        return Image.open(bmp).convert("RGB"), bmp
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()


def count(im, rgb, tol=1, y_max=None):
    px = im.load()
    w, h = im.size
    n = 0
    for y in range(0, min(h, y_max or h), 2):
        for x in range(0, w, 2):
            r, g, b = px[x, y]
            if abs(r - rgb[0]) <= tol and abs(g - rgb[1]) <= tol and abs(b - rgb[2]) <= tol:
                n += 1
    return n


def main():
    if not EXE.is_file():
        sys.exit(f"缺少 {EXE}")

    # 关键:exe 单独放一个空目录,cwd 也换成一个空目录,两处都没有 config/
    root = Path(tempfile.mkdtemp(prefix="vectorterm-embed-"))
    exe_dir = root / "app"
    work = root / "work"
    exe_dir.mkdir()
    work.mkdir()
    exe = exe_dir / "VectorTerm.exe"
    shutil.copy2(EXE, exe)
    assert not (exe_dir / "config").exists(), "exe 目录必须没有 config/"

    im, bmp = frame(exe, "chatgpt", 'return { theme = "chatgpt" }', work)
    w, h = im.size

    canvas = count(im, (0x21, 0x21, 0x21), tol=1)
    white_tab = count(im, (0xEC, 0xEC, 0xEC), tol=1, y_max=60)
    default_bg = count(im, (0x0C, 0x0C, 0x0C), tol=1)

    print(f"帧 {w}x{h}  {bmp}")
    print(f"  画布 #212121 命中 {canvas}")
    print(f"  顶部活动标签 #ECECEC 命中 {white_tab}")
    print(f"  默认底色 #0C0C0C 残留 {default_bg}")

    ok = canvas > 1000 and white_tab > 200 and default_bg < 50
    shutil.rmtree(root, ignore_errors=True)
    print("PASS: 无 config 目录也能用内嵌主题" if ok else "FAIL: 内嵌主题没生效")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
