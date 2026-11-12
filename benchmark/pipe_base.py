"""定位 1.9 MiB/s 这个天花板在哪一层 —— 生产者一侧的两条基线。

  * bash 读文件 -> /dev/null : 磁盘 + 进程启动的底
  * cmd /c type -> NUL       : 生产者(cmd 的写路径)本身能跑多快

本机结果(7.84 MiB):
    bash 读文件 -> /dev/null     43 ms   181.3 MiB/s
    cmd /c type -> NUL          195 ms    40.3 MiB/s

所以"cmd 太慢"被排除:生产者有 40 MiB/s,而所有终端端到端只有 1.4-1.9 MiB/s。
剩下的差距在 ConPTY/conhost 这一层,由 examples/conpty_ceiling.rs 单独量。

注:另有一条"cmd | cat"想测"绕开 ConPTY 的匿名管道",但 Git Bash 下
`cmd //c` 的路径改写会把命令弄坏(42 ms、实际没输出),不要看那条。
"""

import os
import subprocess
import time

F = os.path.abspath("target/bench_big_104000.txt")
size = os.path.getsize(F) / 1048576


def best(name, argv, repeat=3):
    t = 1e9
    for _ in range(repeat):
        s = time.perf_counter()
        subprocess.run(argv, stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL)
        t = min(t, time.perf_counter() - s)
    print(f"{name:30s} {t*1000:7.0f} ms   {size/t:7.2f} MiB/s", flush=True)
    return t


if __name__ == "__main__":
    print(f"负载 {size:.2f} MiB\n")
    best("bash 读文件 -> /dev/null", ["bash", "-c", f'cat "{F}" > /dev/null'])
    best("cmd /c type -> NUL", ["cmd", "/c", "type", F])
