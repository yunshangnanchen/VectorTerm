# VectorTerm 阶段性代码审计与性能报告(2026-11-07)

> 本轮范围:① 代码审计与冗余清理;② 安全/健壮性缺口排查与加固;③ 与上一阶段、
> 与 **kitty / Ghostty / WezTerm / Windows Terminal** 的实测对比。
> 除 4.6 节明确标注的"非本机参考档",所有数字都是本机 release 实测,复现方法见第 6 节。

---

## 0. 摘要

| 维度 | 结果 | 评价 |
|---|---|---|
| 死代码 | **0 处**(rustc dead-code 告警为空) | 代码卫生良好 |
| 运行期 `unwrap/expect` | **0 处**(主程序路径) | panic 面很小 |
| 本轮修掉的健壮性缺口 | **4 类**(锁中毒 / 图片解压炸弹 / 静默 panic / Lua 内存) | — |
| 渲染 200×60 满屏彩色 | CPU **12.74 ms**(78 fps) / GPU **6.00 ms**(167 fps) | 稳态不卡 |
| 7.84 MiB 端到端 | 五方全在 **1.37–1.85 MiB/s**(WezTerm 1.85 / WT 1.73 / VectorTerm 1.65) | **平台级天花板,VectorTerm 在正常水平** |
| 空闲内存 | CPU **109 MB** / Ghostty 124 / WT 180 / WezTerm 209 / GPU **588 MB** | GPU 后端最贵 |
| 冷启动到首帧 | WT **0.51 s** / Ghostty 0.57 / VectorTerm CPU 0.88 / GPU **2.07 s** | GPU 后端最慢 |

**一句话结论**:稳态渲染和批量输出都已经跟主流终端打平 —— 五个不同实现(VectorTerm 双后端、
Windows Terminal、WezTerm、Ghostty 的 Windows 移植)全部落在 1.4–1.9 MiB/s,连"只读管道
不画"也只有 0.94 MiB/s,说明天花板是 ConPTY 而不是某个终端的代码。**VectorTerm 现在唯一明显
落后的是 GPU 后端的 588 MB 常驻内存和 2.07 s 冷启动**,那是自己的问题,也是最值得修的
下一件事。(kitty 没有 Windows 版本,无法实测,只能给参考档 —— 见 4.5。)

> **勘误(相对本文件上一版)**:上一版称"Windows Terminal 快 30 倍、吞吐 ≈60 MB/s,
> 根因是 VectorTerm 没开 ConPTY passthrough"。那个数字来自窗口回收缺陷导致的误判
> (详见 4.1)。改正后 30× 差距不存在,5.1 节已重写。原始结论不再引用。

---

## 1. 代码审计

### 1.1 死代码:没有

- `cargo build --release` 对本项目源码输出 **0 条 `dead_code` / `unused` 告警**。
- `#[allow(dead_code)]` 只有 3 处,逐一核对后都合理:
  - `cursor.rs::is_animating` / `speedup` —— 单元测试在用(非测试构建确实不调用);
  - `script.rs::lua` —— 为了让 Lua 状态活到引擎销毁而保留的访问器。
- 全部 `pub fn/struct/const` 做过引用计数扫描,没有"只出现一次(仅定义处)"的项。
- 没有注释掉的死代码块,没有 `TODO/FIXME/HACK` 遗留。

### 1.2 冗余清理(本轮改动)

| 位置 | 问题 | 处理 |
|---|---|---|
| `main.rs::tab_bar_layout` | `if A { 0 } else if B { 0 } else { n }` 两个分支同体 | 合并成一个 `hide` 条件 |
| `config.rs` ×4 | `for pair in t.pairs() { if let Ok(..) = pair }` | 改用 `.flatten()` |
| `config.rs::parse_background` | `v.max(0.0).min(200.0)` | `v.clamp(0.0, 200.0)` |
| `cursor.rs::clamp_tail` | 用 `for i in 0..4` 索引两个四元数组 | `zip` 迭代 |
| `cursor.rs::step` | 同上,且 `for` + `break` 找标志位 | `any()` |
| `selection.rs` | `for k in from..=to { &row[k] }` | 直接切片迭代 |
| `render.rs::convert_cell` | 对 `Copy` 类型的 `ColorAttribute` 调 `.clone()` | 去掉 |
| `main.rs` | `mod resize_settle_tests` 夹在两条 `struct` 定义中间 | 移到文件末尾 |

清理后 `cargo clippy --all-targets` 对本项目源码剩 12 条告警,全是纯风格项:
函数参数超过 7 个(4 条,都是渲染/绘制函数,拆结构体反而更绕)、复杂类型别名
(3 条)、`same item pushed into Vec`(3 条,位平面提取的误报)、`chunks_exact`
建议换 `as_chunks`(3 条,需更高 MSRV)。逐一评估后不改:有的是误报,有的
按建议改会降低可读性或抬升工具链要求。

### 1.3 安全冗余:修了什么

**① 锁中毒导致的连环 panic(中危)**

输入缓冲 `WriterInner.buf` 原本是 `std::sync::Mutex`,三处 `lock().unwrap()`。
只要有一个线程在持锁时 panic(窗口线程索引越界、字体线程异常都算),之后**每敲一个键
都会再 panic 一次**,终端彻底废掉。改成 `parking_lot::Mutex`(不中毒),并去掉 `unwrap`。

**② 图片解码的"解压炸弹"(高危)**

`images.rs::decode_bytes` 对 Kitty / iTerm2 / sixel 传来的编码图直接 `image::load_from_memory`。
一张几十 KB 的 PNG 可以声明 50000×50000,解码时按 `w*h*4` 一次分配 10 GB ——
Rust 里分配失败是 **abort**,不是可捕获异常,窗口当场消失且不留痕迹。
现在先读文件头拿尺寸,超过 64 M 像素(256 MB RGBA)直接跳过并打日志,裸 RGBA 路径一并挡掉。

**③ panic 完全静默(中危)**

主线程 panic → 窗口消失,用户只看到"闪退";解析/等待/刷写三个子线程 panic 更糟——
窗口还在、终端不动,看起来像"卡死"。现在统一挂 `panic::set_hook`,把时间戳、线程名、
位置写进 `~/.config/VectorTerm/crash.log`(超过 256 KB 自动换新),stderr 照旧保留。

**④ Lua 配置无内存上限(低-中危)**

配置脚本是用户自己写的,但一个手滑的 `('x'):rep(1e9)` 就能吃光内存并 abort 进程 ——
而且发生在启动阶段,用户连窗口都看不到。现在 Lua 堆上限 64 MB,超限变成普通 Lua 错误,
走已有的"配置加载失败 → 用默认配置"分支。

### 1.4 仍然存在的限制(未修,已评估)

| 项 | 现状 | 风险 |
|---|---|---|
| 配置脚本死循环 | `while true do end` 会卡住启动 | 低(自己写的配置);加指令数钩子会拖慢正常配置 |
| scrollback 无上限 | `scrollback_lines` 用户可配到任意大 | 低(显式配置行为) |
| 窗口最小尺寸 | 未调 `set_min_inner_size`,但三处渲染入口都判了 `w==0 \|\| h==0` | 低 |
| GPU 设备丢失 | 处理了 `SurfaceError::Lost/Outdated`,但 `DeviceLost` 只读日志不重建 | 中(独显切换时可能黑屏) |
| 图片总内存 | 有 LRU 淘汰(默认上限),但单张上限刚加,累计仍按 LRU 走 | 低 |

---

## 2. 测试环境与口径

| 项 | 值 |
|---|---|
| OS | Windows 11 |
| CPU / GPU | 本机 15.52 GB 内存;GPU 为 Intel(R) Graphics(核显,DX12) |
| 构建 | `cargo build --release`,`lto = "thin"`,`codegen-units = 1` |
| 显示 | 系统缩放 125%,终端窗口 120×40 或 200×60 |

**三个容易踩的口径陷阱**(与上一份报告一致,这里重申):

1. **进程内微基准 ≠ 真实窗口**:微基准用 offscreen 目标,没有 vsync、没有 DWM 合成。
2. **"解析吞吐"不是"端到端吞吐"**:前者是纯状态机,后者要过 ConPTY、要画到屏幕。
3. **GPU 的 `wait` 时间包含 DX12 fence 固定延迟**,拿它和 CPU 的墙钟直接比会低估 GPU。

本轮新增的端到端口径见 4.1:负载末尾刷一片**真彩色洋红底**(`ESC[48;2;255;0;255m`),
外部按窗口区域截屏,洋红像素占比超过阈值即判定"这批数据已经画到屏幕上"。
这个颜色在任何主题下都不会自然出现,不受深浅色主题干扰。

---

## 3. 稳态渲染:不是瓶颈

`bench_frames`,每帧完整走"快照 → 布局 → 光栅/编码 → 呈现",3 种画面 × 4 种尺寸 × 2 后端。

### 3.1 关键尺寸 200×60(1816×976,约 1.8 MPx)

| 后端 | 画面 | mean | p50 | p95 | 理论 FPS | 分解 |
|---|---|---|---|---|---|---|
| CPU | 彩色满屏 | 12.74 ms | 12.65 | 14.21 | 78 | snap 0.94 + layout 0.73 + **raster 11.07** |
| GPU | 彩色满屏 | **6.00 ms** | 5.85 | 7.92 | 167 | snap 0.89 + layout 0.65 + encode 2.42 + wait 2.04 |
| CPU | 纯文本 | 9.11 ms | 16.71 | 17.29 | 110 | raster 7.62 |
| GPU | 纯文本 | 7.27 ms | 10.58 | 11.79 | 137 | encode 3.21 + wait 2.54 |
| CPU | 空白 | 0.78 ms | 1.10 | 1.23 | 1278 | 纯背景填充 |
| GPU | 空白 | 3.01 ms | 5.58 | 6.19 | 333 | 空白也要提交一次 |

读法:

- **CPU 后端在 1080p 满屏彩色时贴着 60 fps 预算**(12.7 ms vs 16.7 ms),余量很小;
  再大一号(300×80)就掉到 37 fps,软渲染的天花板就在这里。
- **GPU 后端 6 ms/帧,留了 2.7 倍余量**;但空白画面也要 3 ms —— 这是 wgpu 提交与
  fence 的固定成本,不是像素成本。
- `bench_paint`(同一尺寸、单帧):CPU 11.46 ms / GPU 1.51 ms。GPU 的 6 ms 里有一大半
  是 `wait`(等设备完成),真实窗口里这段会被 vsync 吸收。

### 3.2 与上一阶段对比(同机、同尺寸、同画面)

| 指标 | 上一阶段 | 本轮 | 变化 |
|---|---|---|---|
| CPU 彩色 mean | 13.74 ms | 12.74 ms | −7.3% |
| CPU 彩色 p95 | 14.87 ms | 14.21 ms | −4.4% |
| GPU 彩色 mean | 8.19 ms | 6.00 ms | −26.7% |
| **GPU 彩色 p95** | **23.61 ms** | **7.92 ms** | **−66.5%** |

GPU 的 p95 从"偶发 23 ms 卡顿"收敛到 8 ms 以内,帧时间分布明显变窄。
本轮改动没有触及渲染路径,这个改善更可能来自驱动/系统状态,保守记为"环境观测",
不作为版本能力提升宣传。

---

## 4. 端到端:五方横向对比(kitty / Ghostty / WezTerm / WT / VectorTerm)

### 4.1 测法,以及一个必须修掉的坑

- 负载:7.84 MiB ASCII(104000 行 × 80 字符)+ 末尾 40 行洋红底标记;
  `cmd.exe /c type payload` 输出,批处理末尾 `ping` 保住窗口 60 秒。
- 判定:轮询截屏窗口区域,洋红像素占比 > 35% 视为"已经画到屏幕"。
  洋红用真彩色 `48;2;255;0;255`,不受深浅主题干扰。
- 两段式:同一终端先跑"空负载"(只有 40 行洋红)得启动基线,再跑 7.84 MiB;
  两者相减是纯吞吐。每组预热 1 次 + 3 次取中位。
- 全部终端同一台机器、同一天、120 列 × 40 行、同一份负载。

**坑:上一版这个表是错的,必须留档。** 上一版报 Windows Terminal
"8.1 MiB 用 0.37 s、≈60 MB/s",而它的冷启动是 0.39 s —— 总耗时比启动还短,
物理上不成立。根因是回收不彻底:`taskkill /PID` 对 `wt.exe` / `wezterm.exe`
只杀掉了启动器存根,真正的窗口进程还开着、还停在原坐标、屏幕上还留着上一轮
那片全屏洋红;下一轮新窗口开在同一位置,截屏把旧窗口读成了"新窗口画完了"。
把 VectorTerm 那几行修好之后它自己也变快了同样的幅度,一并说明这是测量侧的问题。

现在的做法:每轮结束用 `SetWindowPos` 把窗口挪到屏幕外(`-30000,-30000`,
不参与 DWM 合成),再 `taskkill`;开测前额外断言"整个虚拟屏幕上洋红占比 < 2%",
并且新窗口刚出现时若区域内已有洋红,该次数值直接作废。

### 4.2 结果(本机实测)

| 终端 | 实现 | 冷启动 | 7.84 MiB 总耗时 | 纯吞吐 |
|---|---|---|---|---|
| WezTerm | Rust + OpenGL | 1.42 s | 5.66 s | **1.85 MiB/s** |
| Windows Terminal | C++ / DX | 0.51 s | 5.05 s | **1.73 MiB/s** |
| VectorTerm(GPU) | Rust + wgpu/DX12 | 2.07 s | 6.83 s | **1.65 MiB/s** |
| VectorTerm(CPU) | Rust + softbuffer | 0.88 s | 5.89 s | **1.56 MiB/s** |
| Ghostty(Windows 移植 1.3) | Zig + OpenGL | 0.57 s | 6.29 s | **1.37 MiB/s** |
| kitty | — | — | — | **无法在 Windows 上测** |

**五个完全不同的实现,吞吐挤在 1.37–1.85 MiB/s,极差只有 1.35 倍。**
语言不同(Zig / Rust / C++)、渲染后端不同(OpenGL / DX12 / 纯软件)、
架构不同(单进程 / mux 分离 / 软件光栅),吞吐几乎一样 —— 这类分布只可能是
被同一个**外部**因素卡住的,不是各自代码质量的结果。

顺带两个和吞吐无关的观察:

- **VectorTerm 的 CPU 后端正反都赢 GPU 一次**:吞吐持平(1.56 vs 1.65),但启动
  快 2.4 倍(0.88 s vs 2.07 s)、内存省 5.4 倍(109 MB vs 588 MB)。
- **Ghostty 是这批里最慢的**,但它也是这批里最省内存的(124 MB),而且它自己
  的 README 明确说"上游 macOS/Linux 的 benchmark 数字不适用于 Windows",
  所以这个 1.37 只能当作"一个跑在 Windows 上的 Ghostty 内核",不代表官方性能。

### 4.3 先证明测的不是噪声

把负载按 1.96 / 3.92 / 7.84 MiB 三档喂给 Windows Terminal,看耗时是否线性:

| 负载 | 总耗时 | 扣掉启动后 | 吞吐 |
|---|---|---|---|
| 1.96 MiB | 1.64 s | 1.14 s | 1.72 MiB/s |
| 3.92 MiB | 2.57 s | 2.03 s | 1.93 MiB/s |
| 7.84 MiB | 4.85 s | 4.15 s | 1.89 MiB/s |

斜率稳定在 1.9 MiB/s、截距 ≈ 0.5 s(正好是冷启动)。这说明测到的是真实的
"每字节成本",不是某个固定等待或固定超时。`tools/diagnostics/bench_five.py` 里任何
"总耗时 < 启动耗时"的结果都会被上面那条断言直接判废,不会再流出去。

### 4.4 天花板在哪:三层对照

| 层 | 做法 | 吞吐 |
|---|---|---|
| 生产者 | `cmd /c type big.txt > NUL`,不经过任何控制台 | **40.3 MiB/s** |
| ConPTY(只读不画) | 用项目自带的 `portable-pty` 开同一个 ConPTY,全速读空管道,不解析不渲染 | **0.94 MiB/s** |
| 上屏端到端 | 五个终端实测 | **1.37–1.85 MiB/s** |

- 生产者能跑 40 MiB/s,排除了"cmd 太慢"。
- **把终端软件整个拿掉、只读管道,也只有 0.94 MiB/s** —— 甚至比上屏还略慢
  一点。这个基线没有做批处理(终端会把多帧合并成一次绘制,它只是来什么收什么),
  所以它是个下界而不是最优值,但它足以证明:**解析、排版、绘制都不在关键路径上**。
- 结论:8 MiB 输出慢在 **ConPTY/conhost 这一层**,而且它对所有 Windows 终端
  一视同仁。这不是 VectorTerm 的缺陷,换渲染后端、换解析器、换语言都不会有量级收益。

复现:`cargo run --release --example conpty_ceiling -- target/bench_big_104000.txt`

### 4.5 kitty 和 Ghostty 为什么只能这么比

**kitty:完全没有 Windows 版。** 官方支持范围是 Linux / macOS / BSD,
Windows 只能跑在 WSL 里(`kitty` 最新 0.48.2,2026-07 发布,仍无原生构建)。
本机 `wsl.exe` 被安全策略拦截(沙箱明确拒绝,不可绕过),所以这台机器上
既没有原生 kitty 也没有 WSL kitty,**实测数据不存在,不编**。

**Ghostty:上游明确不计划做 Windows GUI。** 1.3 的 release note 原话是
"support for Microsoft Windows is still not planned";只有底层的
`libghostty` 支持 Windows。本表里的 Ghostty 是第三方原生移植
`shiweis/ghostty-windows` 的 `win-v1.3.0` 便携版
(Zig `ReleaseFast`、OpenGL/WGL、ConPTY、直连 Win32 运行时,不走 WSL/Cygwin)。
包 SHA256 `c921a05c1e589e17e030c92b0f59eb5e5cbeb99c8a1dd1c83b86182c2828661e`。
它是目前唯一能在 Windows 上实测的 Ghostty 家族实现,但**不是官方产物**。

### 4.6 Linux/macOS 参考档(非本机,口径不同,只作量级参照)

| 终端 | 平台 / 来源 | 测法 | 数字 |
|---|---|---|---|
| kitty 0.33 | Linux/X11,Ryzen 7 5850U,kitty 官方 | `kitten __benchmark__`(**默认抑制渲染**,纯解析) | 平均 **134.6 MB/s**(ASCII 121.8 / Unicode 105.0 / CSI 59.8) |
| wezterm 20230712 | 同上,kitty 官方 | 同上 | 平均 **48.5 MB/s** |
| alacritty 0.13 | 同上,kitty 官方 | 同上 | 平均 **54.1 MB/s** |
| ghostty 1.3.1 | Apple silicon,第三方 README | 100 MB `cat` 到 GUI,80×24 | **~64 MiB/s** |
| kitty 0.48.2 | 同上 | 同上 | **~75 MiB/s** |
| alacritty 0.17 | 同上 | 同上 | **~99 MiB/s** |

对照结论:

- 同一批终端在 Linux/macOS 上能跑 **48–135 MB/s**,在 Windows 上只有
  **1.4–1.9 MB/s**。差 30–70 倍的是**平台**,不是某个终端。
- kitty 官方那组数字**抑制了渲染**,测的是纯解析;拿它和本表的"上屏端到端"
  直接比是不对的,所以这里分开列,不给合并排名。
- 这组数据反过来还佐证了 4.4:同样这些代码,换到没有 ConPTY 的平台就快了
  一到两个数量级 —— 瓶颈确实在 Windows 的伪控制台层。

### 4.7 内存与冷启动

| 终端 | 空闲工作集 | 冷启动(到首帧) |
|---|---|---|
| VectorTerm(CPU) | **109 MB** | 0.88 s |
| Ghostty(Windows 移植) | **124 MB** | 0.57 s |
| Windows Terminal | 180 MB | **0.51 s** |
| WezTerm | 209 MB | 1.42 s |
| VectorTerm(GPU) | **588 MB** | **2.07 s** |

- **GPU 后端的 588 MB 仍是最大短板**。2048² 的 R8 字形图集只有 4 MB,大头来自
  DX12 驱动预留 + wgpu 上传堆 + 交换链。上一份报告已提出,本轮没有改善。
- 冷启动 2.07 s 同理:wgpu 设备创建 + 着色器编译。CPU 后端 0.88 s 说明这部分
  成本完全可以省掉 —— 如果 GPU 后端要做,应该做成"先出首帧、再异步初始化
  GPU",而不是阻塞启动。

---

## 5. 结论与优化优先级

### 5.1 头号问题:ConPTY / conhost 这一层(**不是** 30× 差距,此处勘误)

上一版这一节写的是"Windows Terminal 快 30 倍,因为 VectorTerm 没开
`PSEUDOCONSOLE_PASSTHROUGH_MODE`"。**那个 30 倍的数字是错的**,来自 4.1 里
描述的窗口回收缺陷。改正后的结论如下。

证据链(全部是本轮实测,可复现):

1. **生产者不是瓶颈**:`cmd /c type big.txt > NUL` 40.3 MiB/s。
2. **解析不是瓶颈**:`wezterm-term` 纯解析 40.3 MB/s(ASCII),7.84 MiB 只需 0.2 s。
3. **渲染不是瓶颈**:200×60 满屏 GPU 6 ms/帧、CPU 12.7 ms/帧;而且**换后端吞吐不变**
   (1.65 vs 1.56 MiB/s)。
4. **把终端软件整个拿掉也不是更快**:只读空管道 0.94 MiB/s(见 4.4)。
5. **五个互不相干的实现在 1.37–1.85 MiB/s 聚集,极差 1.35 倍**;
   同样这些代码在 Linux/macOS 上是 48–135 MB/s(见 4.6)。

1+2+3+4 指向同一个位置:**ConPTY/conhost 是共同天花板,它对所有 Windows 终端
一视同仁**。所以:

- 这是**平台成本**,不是 VectorTerm 的缺陷。VectorTerm 已经处在 Windows 平台的正常水平,
  和 WezTerm / WT / Ghostty(Windows 移植)互有胜负、无本质差距。
- 上一版"预期吞吐提升一个数量级"的承诺**不成立**,不再作为主线目标。

**仍然成立、且值得做的一件事**:`portable-pty 0.9` 的 `psuedocon.rs` 里定义了
`PSEUDOCONSOLE_PASSTHROUGH_MODE = 0x8`,创建时只传了
`INHERIT_CURSOR | RESIZE_QUIRK | WIN32_INPUT_MODE`。这个事实没变,但**它值多少
现在没有证据** —— 需要先做一次受控实验:同一个 ConPTY、同样的负载,分别打开和
不打开该 flag,用 `conpty_ceiling` 那套只读方式量,才能知道收益是 5% 还是 5 倍。
在拿到这个数之前,不建议动 `[patch.crates-io]`。

如果要在这条线上继续投入,建议的顺序是:

| 步骤 | 内容 | 判据 |
|---|---|---|
| 1 | 受控实验:ConPTY 开/关 passthrough,只读吞吐对比 | 差距 < 1.5× 就放弃这条线 |
| 2 | 若收益大,再 vendor `portable-pty` 打补丁并留配置开关 | 兼容性回归(依赖屏幕缓冲的老程序) |

### 5.2 已知落后项(相对同平台对手,不看吞吐)

| 优先级 | 项 | 现状 | 建议 |
|---|---|---|---|
| **P0** | GPU 后端 588 MB 常驻 + 2.07 s 冷启动 | 全面落后:Ghostty 124 MB / WT 180 MB;启动比 CPU 后端慢 2.4× | 先出首帧再异步初始化 GPU;图集缩到 1024²、字形按需上传 |
| P1 | CSI 密集解析 5.6 MB/s | TUI 全屏重绘时是本地瓶颈(端到端被 ConPTY 掩盖) | 喂状态机前合并连续光标定位/擦除 |
| P1 | Ghostty/WT 都能在 0.5–0.6 s 出首帧 | VectorTerm CPU 0.88 s,GPU 2.07 s | 配置解析 + 字体扫描放到首帧之后 |
| P2 | `DeviceLost` 不重建 | 只记录日志 | 独显切换场景可能黑屏,建议重建 swapchain |
| P3 | 配置脚本无指令上限 | 死循环会卡启动(内存已由 64 MB 上限兜住) | 用 `set_hook` 每 N 条指令检查截止时间 |

### 5.3 一句话总结

> 稳态渲染已经和主流终端打平(GPU 6 ms/帧),大批量输出也已经是 Windows 平台的
> 正常水平(五个终端全在 1.4–1.9 MiB/s)。**现在唯一明显落后的是 GPU 后端的内存和
> 冷启动** —— 那是 VectorTerm 自己的问题,也是最值得修的。

---

## 6. 复现

```bat
REM 1) 进程内基准(解析 / 渲染 / GPU 流水线 / 环境)
cd /d "C:\path\to\VectorTerm"   :: 换成你 clone 下来的实际路径
cargo test --release bench_ -- --ignored --nocapture --test-threads=1

REM 2) 五方端到端对比(VectorTerm GPU/CPU、WT、WezTerm、Ghostty)
REM    Ghostty 需先解压 target\terms\ghostty-win.zip 到 target\terms\gw
python benchmark\bench_five.py

REM 3) 只读不画的 ConPTY 天花板(不解析不渲染)
cargo run --release --example conpty_ceiling -- target/bench_big_104000.txt

REM 4) 生产者基线(cmd 自身的写入速度)
python benchmark\pipe_base.py

REM 5) 空闲内存对比
python tools\diagnostics\bench_mem.py
```

单独只测某一个:`python benchmark\bench_five.py ghostty`(名字小写子串匹配)。
`RUNS=1` 加快轮次,`BIG_ROWS=26000` 换更小的负载做线性度校验。
脚本都在仓库根目录下执行(内部用 `os.getcwd()` 定位 `target/`)。

原始数据:

- `benchmark/data/bench-frames.csv` —— 渲染分阶段(3 画面 × 4 尺寸 × 2 后端)
- `benchmark/data/bench-parse.csv` —— VT 解析吞吐
- `benchmark/data/latest-summary.txt` —— `run.ps1` 汇总(如已跑过)

脚本说明:

- `benchmark/bench_five.py` —— 五方端到端:**本轮主口径,已入库**。两段式(空负载/重负载)+
  洋红像素判定;每轮结束把窗口挪出屏幕再 `taskkill`,开测前断言屏幕无残留,
  遇到"总耗时 < 启动耗时"直接判废。
- `benchmark/pipe_base.py` —— 生产者基线:`bash cat` / `cmd type > NUL` 各跑 3 次取最好。
- `examples/conpty_ceiling.rs` —— 只读不画的 ConPTY 基线(**已入库**,可 `cargo run --example`)。
- `tools/diagnostics/bench_mem.py` —— 同名进程工作集求和(VectorTerm 单进程、WT 双进程、WezTerm 单进程)。
- `tools/diagnostics/diag_cpu.py` —— `GetProcessTimes` 读内核+用户时间,和墙钟对照。
- `tools/diagnostics/bench_split.py` —— 上两方时代的旧脚本,已被 `bench_five.py` 取代(同样有窗口
  回收缺陷,不要再用它的数字)。
- python 取证脚本刻意放在 `target/`(不入库),只作本轮取证用;要长期保留请移到 `benchmark/`。

外部数据来源(4.5 / 4.6 节):

- kitty 官方 `docs/performance.rst`(`kitten __benchmark__`,Linux/X11,Ryzen 7 5850U)
- `shiweis/ghostty-windows` README 与 release `win-v1.3.0`(Windows 移植状态与免责声明)
- Ghostty 1.3 release notes("support for Microsoft Windows is still not planned")
- Ghostty 1.3.1 / kitty 0.48.2 / alacritty 0.17 的 macOS 数字取自 `pg83/shitty` README 的
  均衡化测试(Apple silicon,100 MB cat 到 GUI,80×24),属第三方自测,仅作量级参照

---

## 附录:本轮审计改动清单

| 文件 | 改动 | 性质 |
|---|---|---|
| `src/main.rs` | `WriterInner.buf` 换 `parking_lot::Mutex` | 加固 |
| `src/main.rs` | 新增 `install_panic_hook()` | 加固 |
| `src/main.rs` | 标签栏 `count` 合并重复分支 | 清理 |
| `src/images.rs` | 新增 `MAX_PIXELS`,解码前预检尺寸 | 加固 |
| `src/config.rs` | 新增 `new_lua()`,Lua 堆上限 64 MB | 加固 |
| `src/config.rs` | 4 处 `.flatten()`、1 处 `clamp` | 清理 |
| `src/cursor.rs` | 两处循环索引改 `zip`/`any` | 清理 |
| `src/selection.rs` | 索引循环改切片迭代 | 清理 |
| `src/render.rs` | 去掉 `Copy` 类型的 `.clone()` | 清理 |
| `src/render.rs` | 块元素按格子几何绘制(**上一提交**) | 修复 |
| `examples/conpty_ceiling.rs` | 新增:只读不画的 ConPTY 吞吐基线 | 工具 |
| `benchmark/stage-audit-2026-11-07.md` | 第 4/5 节重写:五方对比 + 勘误 | 文档 |

回归:`cargo test --release` 139 passed / 0 failed。
