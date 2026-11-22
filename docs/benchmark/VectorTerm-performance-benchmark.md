# VectorTerm 性能实测报告

> 测试日期:2026-11-05
> 基线:`eb78721` (wgpu/DX12 后端) + 本轮新增的基准套件与上游补丁
> 构建:`build.cmd --release` / `build.cmd test --release`(`lto = "thin"`, rustc 1.98.1)
> 全部数字均为**本机实测**,不是网络数据;与前一份《终端性能对比报告》分开归档。

## 0. 摘要

| 维度 | CPU 软渲染 | GPU (DX12) | 判读 |
|---|---|---|---|
| 200×60 满屏彩色文本,单帧 CPU 侧 | **13.74 ms** | **2.28 ms** | GPU 把逐像素混合交给显卡 |
| 1920×1080 满屏,真实窗口帧间隔 | 14.24~14.56 ms (≈69 FPS) | **3.33 ms** (≈300 提交/秒) | GPU 不受窗口尺寸影响 |
| 同上 p95 | **18.4~19.4 ms**(超 16.7 ms 预算) | 3.87~4.19 ms | 大窗口软渲染会偶发掉帧 |
| 冷启动首帧 | **112 ms** | **580 ms** | DX12 设备/着色器初始化慢 5× |
| 峰值内存(WorkingSet) | **55.5 MB** | **485 MB** | GPU 后端内存高约 9× |
| 16 MiB 洪泛(60Hz 节流) | 24 帧 / 0 超预算 | 23 帧 / 0 超预算 | 解析线程不拖慢渲染 |

**一句话结论**:GPU 后端的稳态渲染已经不是瓶颈,真正的代价转移到了**冷启动(580 ms)和常驻内存(485 MB)**;软渲染在 1080p 满屏时会突破 60fps 预算;解析侧**CSI 密集负载只有 5.4 MB/s**,是下一个值得优化的热点。

---

## 1. 测试环境与方法

### 1.1 环境

| 项 | 值 |
|---|---|
| OS | Microsoft Windows 11 家庭版, build 26300 |
| CPU | Intel Core Ultra 7 251HX, 18 核 / 18 逻辑处理器 |
| 内存 | 15.52 GB |
| GPU | Intel(R) Graphics, 驱动 32.0.101.8626, DX12;另有 IddDesk 虚拟显示器 |
| DPI 缩放 | 125%(逻辑 1920×1080 → 物理 2400×1350) |
| 字体 | 默认 Consolas 系,size 16.0,padding 6 |
| 终端核心 | wezterm-term(tag `20240203-110809-5046fc22`) |

### 1.2 基准清单

新增 `src/bench.rs`(仅 `cargo test` 编译),全部为 `#[ignore]` 基准,不影响日常测试:

| 名称 | 测什么 |
|---|---|
| `bench_frames` | 4 种窗口 × 4 种画面 × CPU/GPU,每帧分阶段计时(快照/布局/光栅化/GPU 编码/GPU 等待) |
| `bench_gpu_pipeline` | 逐帧强制同步 vs 流水线化提交,校准 GPU 测量口径 |
| `bench_parse` | 8 MiB × 3 次,4 类 VT 负载的纯解析吞吐 |
| `bench_burst` | 16 MiB 持续灌入 + 主线程 60Hz 节流渲染,拆出等锁/快照/绘制 |
| `bench_pty_pipeline` | ConPTY 端到端(**本环境未取得有效数据**,见第 7 节) |
| `bench_env` | 打印编译/运行环境 |

另外在 `main.rs` 加了**仅环境变量触发**的满载帧率采样(`VECTORTERM_PROFILE_FRAMES`),默认完全关闭,不改变正常使用路径。

复现(所有 cargo 调用都经由 `build.cmd`,它负责 vcvars64 + `CRATE_CC_NO_DEFAULTS=1`;
直接调 cargo 在冷缓存上会踩 cc-rs 的 `D8050`):

```bat
build.cmd test --release bench_ -- --ignored --nocapture
```

真实窗口帧率(配置文件在 `benchmark/configs/`,素材由 `benchmark/run.ps1` 生成,内容见第 3 节):

```bat
set VECTORTERM_PROFILE_FRAMES=300
set VECTORTERM_PROFILE_SIZE=1920x1080
target\release\VectorTerm.exe --config benchmark\configs\bench-gpu.lua
```

### 1.3 三个必须说明的口径陷阱

1. **debug 数字不可用**。先前记录的 "CPU 单帧 11.55 ms" 是 debug 构建;本次全部在 release/lto 下重测,200×60 满屏彩色文本为 13.74 ms(场景更重,不可与旧值直接相减)。
2. **每帧强制等 GPU 会得到虚高数字**。`device.poll(Maintain::Wait)` 空调用耗时 0.000 ms,但"提交一帧→立刻等"会引入 DX12 队列气泡,空屏也要 5.6 ms。真实窗口从不逐帧等待。因此本报告以**流水线化提交**和**真实窗口帧间隔**为准。
3. **真实窗口的 3.33 ms 是提交节流上限,不是渲染耗时**。GPU 路径在 80×24 空屏和 198×58 满屏下都精确落在 3.33 ms(p50 3.32),说明上限来自 DXGI 帧延迟队列/DWM;显示器 60Hz 时用户实际看到的是 60 FPS。CPU 路径没有这个固定节流,帧间隔随像素数线性增长。

---

## 2. 渲染管线微基准

串行整帧计时(snapshot → 布局 → 呈现),warmup 8 帧、采样 40 帧,数据写入 `benchmark/data/bench-frames.csv`。下表为**最坏画面**(每格都有字形、前景/背景逐格切换)与**空屏**的对照,单位 ms。

| 格数 / 画布 | 画面 | CPU 快照 | CPU 布局 | CPU 光栅 | CPU 合计 | GPU 编码 | GPU 等待* | GPU 合计* |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| 80×24 / 736×400 | blank | 0.01 | 0.01 | 0.08 | **0.09** | 0.23 | 5.34 | 5.59 |
| 80×24 / 736×400 | color | 0.11 | 0.09 | 1.53 | **1.73** | 0.31 | 4.41 | 4.92 |
| 120×40 / 1096×656 | color | 0.30 | 0.28 | 4.65 | **5.23** | 0.50 | 5.26 | 6.29 |
| 200×60 / 1816×976 | blank | 0.04 | 0.04 | 0.58 | **0.66** | 0.40 | 5.69 | 6.22 |
| 200×60 / 1816×976 | plain | 0.65 | 0.64 | 8.08 | **9.37** | 1.14 | 6.35 | 8.81 |
| 200×60 / 1816×976 | color | 0.76 | 0.75 | 12.23 | **13.74** | 0.95 | 5.91 | 8.19 |
| 200×60 / 1816×976 | cjk | 0.39 | 0.38 | 5.07 | **5.84** | 0.69 | 6.31 | 7.77 |
| 300×80 / 2716×1296 | plain | 1.35 | 1.46 | 20.32 | **23.13** | 1.91 | 8.46 | 12.93 |
| 300×80 / 2716×1296 | color | 1.45 | 1.53 | 25.38 | **28.36** | 1.93 | 8.46 | 13.20 |

\* "GPU 等待/合计"是**逐帧同步口径**,含气泡,只用于与流水线口径对照,不代表真实帧时间。

**CPU 侧可归因**:光栅化占满屏帧的 85~90%,且随像素数线性增长(1.53 → 4.65 → 12.23 → 25.38 ms);快照+布局只占 5~10%(0.2 → 2.98 ms)。GPU 路径的 CPU 成本 = 快照 + 布局 + 编码,200×60 满屏约 **2.28 ms**,300×80 约 4.74 ms。

**GPU 提交口径对照**(`bench_gpu_pipeline`,连续 60 帧,color 满屏):

| 格数 | 空 sync | 逐帧 sync / 帧 | 流水线 / 帧 | 流水线理论 FPS |
|---|---:|---:|---:|---:|
| 80×24 | 0.000 | 5.689 | **1.279** | 782 |
| 200×60 | 0.000 | 7.453 | **3.638** | 275 |
| 300×80 | 0.000 | 13.639 | **7.072** | 141 |

逐帧同步的 5~14 ms 几乎全是"提交后立刻等"的气泡;流水线化后真实 GPU 吞吐是 1.3~7.1 ms/帧,300×80 满屏也仍在 16.7 ms 预算内。

---

## 3. 真实窗口满载帧率

**画面已校验**:`cmd.exe /k type target\bench-screen.txt` 输出 200 列 × 60 行逐格着色的文本,用 `session_log` 落盘确认收到 **240,281 字节**(源文件 240,118 字节),即满屏内容确实到达解析线程。第一次测量因命令参数引号被转义只渲染了空屏,已作废并重测。

每个组合跑 3 次,每次 299 个有效帧(丢弃前 5 帧预热):

| 逻辑窗口 / 物理像素 / 格数 | 后端 | 均值 ms | p50 ms | p95 ms | 最大 ms | 提交 FPS |
|---|---|---:|---:|---:|---:|---:|
| 960×640 / 1200×800 / 98×34 | GPU | 3.33 | 3.33 | 3.84~4.12 | 6.7~7.8 | 300 |
| 960×640 / 1200×800 / 98×34 | CPU | 5.19~5.35 | 4.18~4.34 | 11.2~13.3 | 22~26 | 187~193 |
| 1920×1080 / 2400×1350 / 198×58 | GPU | 3.34~3.39 | 3.32 | 3.87~4.19 | 6.6~7.4 | 295~299 |
| 1920×1080 / 2400×1350 / 198×58 | CPU | 14.24~14.55 | 13.64~14.07 | **18.4~19.4** | 24~29 | 68.7~70.2 |

- GPU:窗口面积扩大 2.8 倍、格数扩大 3.5 倍,帧间隔纹丝不动(3.33 ms),进一步印证瓶颈在提交节流而非渲染。
- CPU:均值 5.3 → 14.5 ms,p95 已越过 16.7 ms 的 60fps 预算,且 p50/均值差距小、最大值 24~29 ms,属于**稳定偏慢 + 偶发抖动**,不是偶发 GC。
- 微基准(200×60 color,13.74 ms)与真实窗口 CPU(198×58,14.5 ms)互相印证,两条独立测量链一致。

复现用配置(`benchmark/configs/bench-gpu.lua`,`render` 改成 `cpu` 即 CPU 版;
从项目根目录运行,素材路径是相对路径):

```lua
return {
  shell = "cmd.exe",
  shell_args = { "/k", [[type benchmark\data\bench-screen.txt]] },
  render = "gpu",
  padding = 6,
}
```

生成画面(项目根目录执行,或让 `benchmark/run.ps1` 自动生成):

```powershell
$p="benchmark\data\bench-screen.txt"; $e=[char]27
$line = -join (1..100 | ForEach-Object { "$e[38;5;196mW$e[0m$e[48;5;24;38;5;231mM$e[0m" })
[IO.File]::WriteAllText($p, ((1..60 | ForEach-Object { $line }) -join "`r`n"))
```

---

## 4. VT 解析吞吐

`bench_parse`:8 MiB 负载喂给 `wezterm-term::Terminal::advance_bytes`,3 次取中位,无渲染、无 PTY。口径接近 kitty `kitten __benchmark__`(抑制渲染的纯解析)。

| 负载 | 中位 MB/s | 最快 MB/s | 8 MiB 耗时 | 说明 |
|---|---:|---:|---:|---|
| ascii | 45.2 | 45.2 | 185.7 ms | 纯文本,无转义序列 |
| sgr | **72.7** | 73.0 | 115.4 ms | 每字符约 2 组 SGR(颜色解析比想象中便宜) |
| csi(cursor-move) | **5.4** | 5.5 | 1563.4 ms | `\x1b[12;34H` + `\x1b[K` 擦行,模拟 TUI 全屏重绘 |
| unicode(cjk) | 43.9 | 51.2 | 191.2 ms | 宽字符 + emoji + 组合字符 |

**CSI 密集负载是纯文本负载的 1/8**,因为每次 `\x1b[K` 都要重写整行 200 格。这解释了为什么 vim/tmux 类全屏重绘比 `cat` 大文件更容易卡,也是后续最值得投入的优化点(批量合并连续 CSI、跳过离屏区域)。

与网络报告的量级参照(机器/OS/年代/方法全不同,**不能相减**):kitty 官方 2023 表 ASCII 平均 134.55 MB/s;Ghostty 1.4.0 官方称 ASCII 解析 128 → 725 MB/s。我们走的是上游 wezterm-term,解析性能上限由上游决定。

---

## 5. 大输出洪泛(应用内端到端)

`bench_burst`:解析线程按 16 KiB 块连续灌入 16 MiB ASCII,主线程按 60Hz(16.667 ms)节流取快照并渲染,锁用与真实程序一致的 `parking_lot::Mutex`。

| 后端 | 墙钟 | 解析 | 等效 MB/s | 帧数 | 等锁 ms | 快照 ms | 绘制 ms | 超 16.7ms 的帧 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| CPU | 405.0 ms | 399.4 ms | 41.4 | 24 | 0.21 | 0.52 | 4.39 | **0 / 24** |
| GPU | 385.5 ms | 381.1 ms | 43.5 | 23 | 0.24 | 0.34 | 1.10 | **0 / 23** |

0.385 s 内应该出 23 帧,实测 23~24 帧,全部守在预算内 —— **解析线程与渲染主线程的锁协作在真实锁实现下是健康的**。

一个有价值的对照:最初误用 `std::sync::Mutex` 时,解析线程把主线程饿死,等锁 **67~70 ms**、只剩 5 帧且全部超预算。换成 `parking_lot::Mutex` 后消失。这不是 VectorTerm 的缺陷(主程序本来就用 parking_lot),但说明这类架构对锁实现非常敏感。

**口径限制**:不含 ConPTY 的 VT 转换、不含子进程启动,是应用内部链路的下界;不能与 Linux `cat 16MiB` 的数字直接比较。

---

## 6. 冷启动与内存

1920×1080、180 帧 profile、每种后端 3 次。`hwnd` = 进程启动到窗口句柄出现;`first_frame` = 进程启动到首帧呈现完成(由程序内计时);内存为进程存活期间的 WorkingSet / Private Bytes 峰值。

| 后端 | 窗口出现 | 首帧(3 次) | 首帧中位 | 峰值 WorkingSet | 峰值 Private |
|---|---|---:|---:|---:|---:|
| GPU | 224~237 ms | 689 / 576 / 580 ms | **580 ms** | 484~485 MB | 499~501 MB |
| CPU | 212~245 ms | 112 / 148 / 111 ms | **112 ms** | 55.5~55.6 MB | 40.5~41.4 MB |

- 两者窗口创建时间几乎一致(~230 ms),**GPU 多出的约 470 ms 全在 DX12 设备/着色器/交换链初始化**。
- **485 MB 常驻内存是本次测试最刺眼的数字**。2048² R8 字形图集本身只有 4 MB,大头更可能来自 DX12 驱动预留、wgpu 上传堆与交换链缓冲;建议用 DXGI/D3D12 内存分析定位,再决定是否缩小图集、按需分配或延迟初始化。
- 作为参照,前一份网络报告里 15 款 Linux 终端的空闲内存多在 20~100 MiB 区间(不同平台、不同 PSS 口径,仅量级参照)。

---

## 7. 未取得有效数据的项目

**ConPTY 端到端吞吐(与 `cat 16MiB` 同口径)未完成**。在 `cargo test` 进程内启动 `cmd.exe` 后,PTY 只读到 **55~61 字节**初始化序列(`\x1b[1t` 等 DSR/DA 查询)便阻塞,即使把终端应答通过 `ReplyWriter` 回送 PTY 输入也无改善;探针 `echo` 同样 3 s 超时。基准已改为**探针失败即快速跳过**,不会挂住测试。

已排除的原因:命令参数引号转义(改用无空格相对路径 + 显式 `cwd` 后,真实 exe 的 `session_log` 确认 240 KB 完整到达);因此这是测试进程内的 PTY 查询/应答链路差异,**不影响已报告的渲染、解析、帧率、启动数据**。

**输入延迟未做硬件级测量**。没有高速摄像/示波器/光电探头,无法给出"按键→屏幕发光"的毫秒数。本报告只给渲染链路的软件下界(快照 + 布局 + 编码 ≈ 2.3 ms @200×60),真实端到端还叠加事件循环唤醒、DWM 合成与显示器刷新(通常 16.7 ms 量级)。若需要,可接入 typometer 类工具或光电测量。

---

## 8. 本次测试顺带定位并修复的缺陷

洪泛基准在 scrollback 填满后稳定触发:

```text
thread 'bench::bench_burst' panicked at .wezterm-src/term/src/screen.rs:932
range start index 3500 out of range for slice of length 451
```

根因:**上游 wezterm-term `Screen::with_phys_lines` 的 bug**。`VecDeque::as_slices()` 返回 `(first, second)`,代码把 `second` 的范围算成绝对物理行号(`first.len()..first.len()+second.len()`),却直接拿这个绝对索引去切 `second` 这个局部切片;只要回看缓冲发生回绕(长期运行必然发生),取快照就越界 panic。这**不是基准特有**,真实终端长时间滚动同样会崩。

处理:

- 修补 `.wezterm-src/term/src/screen.rs`,把 `second_range` 减去 `first.len()` 转成局部索引(零额外开销);
- 因 `.wezterm-src` 在 `.gitignore` 中,补丁固化为 `patches/wezterm-term-screen.patch`;
- `setup.ps1` / `setup.sh` 增加**幂等**补丁应用(`git apply --check` 通过才打),重新引导源码也不会丢;
- 新增回归测试 `snapshot_survives_scrollback_wrap`,灌 5000 行(超过 3500 行回看上限)后分别取贴底与亚行回看快照。

全量测试:**50 passed / 13 ignored / 0 failed**(原 49 passed,新增 1 个回归 + 6 个 ignored 基准)。

---

## 9. 结论与优化优先级

按"收益 / 改动量"排序:

1. **GPU 冷启动与内存**(580 ms / 485 MB)。稳态帧率已经不是问题,但用户每次开窗口都要付这笔成本。先定位 485 MB 的来源(DX12 驱动预留 / wgpu 上传堆 / 交换链),再谈缩小图集或共享设备。这是 GPU 后端目前最大的短板。
2. **软渲染大窗口**(1080p 满屏 14.5 ms,p95 19 ms)。可做的:相邻同色单元格矩形合并、只重绘脏行、alpha 混合上 SIMD。`render = "cpu"` 是远程桌面/兼容性兜底,不必追平 GPU,但应保证 p95 不过 16.7 ms。
3. **CSI 密集解析 5.4 MB/s**。TUI 全屏重绘场景的瓶颈,考虑在喂给状态机前合并连续光标定位/擦除,或让上游按离屏区域跳过擦行。
4. **独立 IO 线程 / 双缓冲快照**。本次洪泛证明 `parking_lot` 下 16 MiB 连续灌入不掉帧,紧迫性不高;但若将来解析吞吐提升 10×,锁竞争会重新浮现(`std::sync::Mutex` 的对照已经演示了后果)。
5. **补齐测量**:输入延迟硬件测量、ConPTY 端到端(建议在真实 exe 内用 `session_log` + 时间戳,而不是在测试进程里起 PTY)。

**与前一份网络报告的关系**:那份报告的数据全部来自公开网络、机器/OS/版本各异;本报告是同一台 Windows 机器上的单项目实测。两者只能做**量级与方法论**参照,任何跨表毫秒数相减都无效。若要做真正可比的横评,Windows 上建议改成 `WezTerm vs VectorTerm vs Windows Terminal vs Alacritty`(Ghostty / kitty 无原生 Windows 版)。

---

## 附录 A:原始数据文件

| 文件 | 内容 |
|---|---|
| `benchmark/data/bench-frames.csv` | 32 行完整微基准(均值/p50/p95/min/max/分项/理论 FPS/实例数) |
| `benchmark/data/profile-window-full.txt` | 满屏内容下 12 次真实窗口运行的逐次统计 |
| `benchmark/data/bench-parse.csv` | 4 类解析负载的吞吐 |
| `benchmark/data/startup-measurements.txt` | 冷启动首帧与后端名(6 次运行) |
| `benchmark/data/run-*.txt` | 每次一键复现的完整输出(由 `run.ps1` 生成,不入库) |

产物统一落在 `benchmark/data/`(可用 `VECTORTERM_BENCH_OUT` 覆盖),不再随 `cargo clean` 丢失;
关键数字已全文收录于本报告。

## 附录 B:复现清单

全套一键脚本见 `benchmark/README.md`,Windows 下:

```bat
cd /d "C:\path\to\VectorTerm"   :: 换成你 clone 下来的实际路径
powershell -ExecutionPolicy Bypass -File benchmark\run.ps1
```

只跑单项(全部经由 `build.cmd`,不需要手动设 `PATH` 或 `CRATE_CC_NO_DEFAULTS`):

```bat
build.cmd test --release bench_ -- --ignored --nocapture

build.cmd test --release bench_frames -- --ignored --nocapture
build.cmd test --release bench_parse -- --ignored --nocapture
build.cmd test --release bench_gpu_pipeline -- --ignored --nocapture
build.cmd test --release bench_burst -- --ignored --nocapture
build.cmd test --release              :: 全量回归(含 scrollback 回绕)

:: 真实窗口(脚本会自动生成 bench-screen.txt 并校验内容到位)
set VECTORTERM_PROFILE_FRAMES=300
set VECTORTERM_PROFILE_SIZE=1920x1080
target\release\VectorTerm.exe --config benchmark\configs\bench-gpu.lua
```
