-- 校验配置:确认满屏文本确实到达了解析线程(会话日志应约 240 KB)。
-- 由 benchmark/run.ps1 或 run.sh 调用,用于排除"测了个空屏"的假数据。
return {
  shell = "cmd.exe",
  shell_args = { "/k", [[type benchmark\data\bench-screen.txt]] },
  render = "gpu",
  padding = 6,
  session_log = "benchmark/data/session-bench.log",
}
