"""空闲内存对比:各终端跑一个空 shell,5 秒后读工作集。"""

import ctypes
import os
import subprocess
import time
from ctypes import wintypes

HERE = os.path.abspath(os.getcwd())
u = ctypes.windll.user32


def procs(name):
    """同名进程的 (pid, working_set_bytes)。"""
    out = subprocess.run(
        ["tasklist", "/FI", f"IMAGENAME eq {name}", "/FO", "CSV", "/NH"],
        capture_output=True, text=True, errors="replace").stdout
    rows = []
    for line in out.splitlines():
        parts = [p.strip('"') for p in line.split('","')]
        if len(parts) >= 5 and parts[0].lower() == name.lower():
            try:
                rows.append((int(parts[1]), parts[4]))
            except ValueError:
                pass
    return rows


def mem_of(names):
    total = 0
    detail = []
    for n in names:
        for pid, mem in procs(n):
            # tasklist 的内存列形如 "12,345 K"
            kb = int(mem.replace(",", "").replace(" K", "").replace("K", "").strip())
            total += kb
            detail.append(f"{n}({pid})={kb/1024:.1f}MB")
    return total / 1024, detail


def run(label, argv, names, wait=5.0):
    for n in names:
        subprocess.run(["taskkill", "/F", "/IM", n], capture_output=True)
    time.sleep(0.5)
    p = subprocess.Popen(argv, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(wait)
    mb, detail = mem_of(names)
    print(f"{label:18s} {mb:7.1f} MB   {' '.join(detail) or '(无进程)'}")
    subprocess.run(["taskkill", "/F", "/T", "/PID", str(p.pid)], capture_output=True)
    for n in names:
        subprocess.run(["taskkill", "/F", "/IM", n], capture_output=True)
    time.sleep(1.0)
    return mb


if __name__ == "__main__":
    cfg = os.path.join(HERE, "target", "bench_mem.lua")
    with open(cfg, "w", encoding="utf-8") as f:
        f.write('return {\n  render = "gpu",\n  shell = "cmd.exe",\n')
        f.write('  window_decorations = "TITLE|RESIZE",\n  initial_cols = 120,\n  initial_rows = 40,\n}\n')

    print("=== 空闲工作集(启动 5 秒后) ===")
    run("VectorTerm", [os.path.join(HERE, "target", "release", "VectorTerm.exe"),
                   "--config", cfg], ["VectorTerm.exe"])
    run("Windows Terminal", ["wt.exe", "-w", "-1", "cmd.exe"],
        ["WindowsTerminal.exe", "OpenConsole.exe"])
    run("WezTerm", ["wezterm.exe", "start", "--always-new-process", "--",
                    "cmd.exe"], ["wezterm-gui.exe", "wezterm-mux-server.exe"])
