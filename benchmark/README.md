# VectorTerm 性能基准目录

这个目录是**性能相关的全部资料与工具的唯一入口**:报告、实测数据、配置、复现脚本。
源码里的可测点留在 `src/bench.rs`(Rust 测试模块,必须住在 `src/`)与 `src/main.rs`
的 `VECTORTERM_PROFILE_*` 环境变量采样,但运行入口都在这里。

```text
benchmark/
├── README.md                          本文件
├── terminal-performance-report.md     网络横评:wezterm / ghostty / kitty(15 个公开来源)
├── vectorterm-performance-benchmark.md    VectorTerm 本机 release 实测报告
├── configs/
│   ├── bench-gpu.lua                  真实窗口:GPU / DX12 + 满屏彩色文本
│   ├── bench-cpu.lua                  真实窗口:CPU 软渲染 + 同一份文本
│   └── bench-verify.lua               校验素材确实进了终端(写 session_log)
├── data/
│   ├── bench-frames.csv               渲染分阶段原始数据(3 画面 × 4 尺寸 × 2 后端)
│   ├── bench-parse.csv                VT 解析吞吐原始数据
│   ├── profile-window-full.txt        真实窗口帧率矩阵(2 尺寸 × 2 后端 × 3 次)
│   ├── startup-measurements.txt       冷启动首帧 / 后端名
│   └── .gitignore                     生成的素材与运行日志不入库
├── run.ps1                            Windows 一键复现(含冷启动与内存采样)
└── run.sh                             Git Bash 精简版
```

## 一键复现

```bat
cd /d "C:\path\to\VectorTerm"   :: 换成你 clone 下来的实际路径
powershell -ExecutionPolicy Bypass -File benchmark\run.ps1
```

脚本会依次:生成固定素材 → 经 `build.cmd` 编译 release(含测试二进制) → 进程内基准 →
真实窗口帧率矩阵 → 校验素材到位 → 冷启动/内存采样 → 写 `data/latest-summary.txt`。
赶时间可以加 `-SkipStartup` 或 `-SkipWindow`。
所有 cargo 调用都走项目根目录的 `build.cmd`:它会先初始化 MSVC 环境(vcvars64 +
`CRATE_CC_NO_DEFAULTS=1`),直接调 cargo 在冷缓存上会踩 cc-rs 的 `D8050`。
`run.ps1` **刻意只用 ASCII**:Windows PowerShell 5.1 不带 BOM 时按系统 ANSI 码页读
`.ps1`,中文字符串会损坏引号导致解析失败;需要中文说明请看本文件。

Git Bash 下可用 `bash benchmark/run.sh`(不含冷启动/内存采样)。

只跑单项(同样走构建入口,不要直接 `cargo ...`):

```bat
build.cmd test --release bench_frames -- --ignored --nocapture
build.cmd test --release bench_parse -- --ignored --nocapture
build.cmd test --release bench_gpu_pipeline -- --ignored --nocapture
build.cmd test --release bench_burst -- --ignored --nocapture
build.cmd test --release                  :: 全量回归(含 scrollback 回绕)
```

`build.cmd --help` 列出全部子命令与 `VECTORTERM_*` 开关。

真实窗口帧率也可以手动触发:

```bat
set VECTORTERM_PROFILE_FRAMES=300
set VECTORTERM_PROFILE_SIZE=1920x1080
target\release\VectorTerm.exe --config benchmark\configs\bench-gpu.lua
```

`VECTORTERM_PROFILE_*` 只在设置了环境变量时生效,正常启动完全关闭;
`VECTORTERM_BENCH_OUT` 可改 CSV/载荷的输出目录(默认 `benchmark/data`)。

## 读数据前必须知道的三件事

1. **必须 release**。debug 构建的逐像素光栅化会慢一个数量级,与报告里的数字不可比。
2. **不要逐帧 `sync()` 等 GPU**。那会在 DX12 提交队列里制造 5～13 ms 的气泡,
   空屏也测出 5.6 ms。`bench_gpu_pipeline` 专门做了同步/流水线对照。
3. **先确认窗口里真有内容**。用 `bench-verify.lua` 跑一次,看 `data/session-bench.log`
   的字节数是否约等于 `bench-screen.txt`;曾经因为 cmd 参数引号被转义测成了空屏,
   那批数字已作废。

## 数据来源边界

- `terminal-performance-report.md` 的数字**全部来自公开网络**,没有在本机安装或运行被测终端。
- `vectorterm-performance-benchmark.md` 的数字**全部来自本机实测**,不参与跨终端横评。
- 两份报告的机器/OS/年代/版本都不同,跨表毫秒数不能相减;详见各自的可比性声明章节。
