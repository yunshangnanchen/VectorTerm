-- 基准配置:GPU / DX12 满屏文本。由 benchmark/run.ps1 或 run.sh 调用。
-- 从项目根目录运行;benchmark\data\bench-screen.txt 由脚本生成。
return {
  shell = "cmd.exe",
  shell_args = { "/k", [[type benchmark\data\bench-screen.txt]] },
  render = "gpu",
  padding = 6,
}
