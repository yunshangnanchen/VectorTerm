# 终端模拟器性能对比测试报告
## WezTerm vs Ghostty vs Kitty —— 基于公开网络基准数据的横向分析

| 项 | 内容 |
|---|---|
| 报告日期 | 2026-11-05 |
| 对比对象 | WezTerm、Ghostty、Kitty |
| 数据来源 | **全部取自公开网络资料**（官方基准 / 第三方独立基准 / 社区实测），逐条编号见第 8 节 |
| 重要声明 | **本报告未在本机安装或运行任何被测终端**，所有对比数字均为引述，不是本机构实测结果 |
| 主要指标 | 解析吞吐、端到端大文件吞吐、持续刷新帧率、键盘到屏幕延迟、冷启动首帧、空闲内存、CPU 占用 |
| 结论性质 | 方向性 / 分级结论，**不是可跨来源直接相减的排行榜** |

---

## 0. 摘要（结论先行）

公开数据里**不存在一份"三款同机、同日、同版本、同配置"的官方横评**。我们找到的最可靠做法是：把方法透明的独立基准当作横向底稿，把厂商自测当作架构侧证，把互相打架的数字单独标注冲突原因。

三条最稳的结论：

1. **WezTerm 在端到端大文件吞吐上稳定垫底**。两份互不相关的独立基准分别测出 `cat 11 MB = 1246 ms`[S6] 与 `cat 16 MiB = 916 ms`[S7]，约为 Ghostty / Kitty 的 **3～4.7 倍**；持续刷新帧率也最低（64 FPS）。但它的**冷启动首帧最快**（101 ms）[S7]。
2. **Kitty 在持续高频刷新场景最强**（DOOM-fire 273 FPS，Ghostty 135 / WezTerm 64）[S6]，厂商自测的解析吞吐也最高（134.55 MB/s）[S1] —— 但后者是 2023 年、抑制渲染、且**表里没有 Ghostty**，不能推广成 2026 年的结论。
3. **Ghostty 1.4.0 是 2026 年的变量**。官方 release notes 给出 ASCII 解析 `128 → 725 MB/s`（5.7×）[S3]，独立基准同机复测 `cat 16 MiB` 从 1.3.1 的 542 ms 降到 **195 ms**（2.8×）[S7]，两者互相印证。代价是 Linux 上 GTK4 外壳把冷启动首帧拖到 308 ms[S7]。

一句话选型：**要吞吐和刷新率看 Kitty / Ghostty 1.4，要启动快和内建多路复用看 WezTerm；在声明了合成器状态的可比测量里，三者输入延迟只差 3～6 ms，而合成器本身就能引入 30～45 ms。**

> ⚠️ **平台前提**：三者中**只有 WezTerm 原生支持 Windows**[S10]。Ghostty 官方已明确"放弃 Ghostty GUI 的 Windows 路线图"[S3]，Kitty 官方列出 Linux / macOS / Various BSDs[S2]。因此本报告**所有第三方性能数字都来自 Linux 或 macOS**，不能代表它们在 Windows 上的表现。

---

## 1. 可比性声明（读数字前必看）

### 1.1 四组数据的机器、系统、年代完全不同

| 来源 | 时间 | 机器 / OS | 图形栈 | 测了什么 |
|---|---|---|---|---|
| [S1] kitty 官方 | 2023（kitty 0.33 / wezterm 20230712） | AMD Ryzen 7 PRO 5850U，Linux/X11 | OpenGL | `kitten __benchmark__` 解析吞吐 |
| [S6] vizenc | 2024-12 提交，2025-04 更新 | ThinkPad T430 / i7-3632QM / Intel HD4000，Arch + Hyprland Wayland | 混合 | 延迟 / 11 MB cat / 帧率 / 内存 |
| [S7] ComputingForGeeks | 2026-10 | Proxmox VM，4 vCPU (i7-1165G7)，Ubuntu 26.04 + GNOME 50 Wayland | **VirGL 透传**（有翻译层） | 首帧 / 16 MiB cat / PSS 内存 |
| [S8] caluga.de | 2024-12 | Apple M1 Ultra / Intel i9 MacBookPro，macOS | Metal / OpenGL | bench.sh / termbench / vtebench |

**所以：不同表格之间的毫秒数不能直接换算或相减。** 例如 [S6] 的 11 MB cat 401 ms 和 [S7] 的 16 MiB cat 212 ms 并不矛盾——文本类型、窗口尺寸、机器、渲染栈全都不同。只有**同一张表内部**的名次与倍数是有意义的。

### 1.2 版本差带来的"假矛盾"

| 基准 | Ghostty 版本 | WezTerm 版本 | Kitty 版本 |
|---|---|---|---|
| [S1] | 未收录（当时未发布 / 非 X11） | 20230712 | 0.33 |
| [S6] | 1.0 前后（仓库 2024-12） | 当期最新版 | 当期最新版 |
| [S7] | **1.4.0**（2026-10-27） | nightly 20260929 | 0.45.0 |

[S6] 里 Ghostty 的吞吐只是中上（407 ms，与 Kitty 401 ms 持平），[S7] 里却成了第一（195 ms）——**这不是数据打架，而是 1.4 的解析器重写**：官方基准 5.7×[S3]、独立复测 2.8×[S7]，方向一致。凡引用 Ghostty 的性能数字，**必须带上版本号**。

### 1.3 "GPU 加速"不预测性能

[S7] 的观察最直接：CPU 渲染（pixman）的 foot 以 39 ms 首帧 / 168 ms dump 双料第一，而同样挂着 GPU 标签的 WezTerm 是全场最慢的 916 ms。决定吞吐的是**解析路径、批处理、缓冲与线程模型**，不是有没有 GPU。

### 1.4 延迟指标受合成器支配

[S12] 的一组 typometer 实测显示，同一台机器上开启 picom 合成器后，三款终端的平均输入延迟从 18～25 ms 抬到 56～63 ms，而三者之间的差距只有几毫秒。**任何不声明"合成器开/关、刷新率、是否 VSync"的延迟对比都没有横比价值。**

---

## 2. 来源可信度分级

| 级别 | 来源 | 理由 |
|---|---|---|
| ★★★ 一手且方法透明 | [S1][S3][S9][S10] 官方文档/自测；[S6][S7] 独立基准 | 给出机器、版本、命令、重复次数与统计口径；[S7] 甚至给出 strace 证据与修复命令 |
| ★★☆ 一手但方法不全 | [S8] 个人多基准评测 | 重复多次、统一窗口尺寸，但**未给 OS 版本与终端版本**，原文自身存在数值不一致（Ghostty termbench 47.9777 vs 汇总 48.9777；wezterm unicode 把标准差 56.64 当成均值） |
| ★★☆ 二手汇总 / 单用户实测 | [S11][S12][S13][S14] | 数据可追溯到一手源或给出明确环境，但为转述；[S12] 原页无法直连核验，仅由搜索索引取得表格 |
| ★☆☆ 已排除 | 见第 5 节 | 无方法、无版本、数字与一手来源冲突，疑似批量生成的 SEO 内容 |

---

## 3. 逐项对比

### 3.1 解析吞吐（`kitten __benchmark__`，厂商自测）[S1]

单位 MB/s，越高越好。同字体、同字号、同窗口尺寸、默认设置，Linux/X11 / Ryzen 7 PRO 5850U。

| 终端 | ASCII | Unicode | CSI | Images | **Average** |
|---|---:|---:|---:|---:|---:|
| kitty 0.33 | 121.8 | 105.0 | 59.8 | 251.6 | **134.55** |
| gnome-terminal 3.50.1 | 33.4 | 55.0 | 16.1 | 142.8 | 61.83 |
| alacritty 0.13.1 | 43.1 | 46.5 | 32.5 | 94.1 | 54.05 |
| **wezterm 20230712** | **16.4** | **26.0** | **11.1** | 140.5 | **48.5** |
| xterm 389 | 47.7 | 18.3 | 0.6 | 56.3 | 30.72 |
| konsole 23.08.04 | 25.2 | 37.7 | 23.6 | 23.4 | 27.48 |
| alacritty + tmux | 30.3 | 7.8 | 14.7 | 46.1 | 24.73 |

**必须一起读的三条官方注记**：

- 默认**抑制实际渲染**（靠 synchronized update 转义码），测的是**解析器速度**，不是端到端显示速度；`--render` 可打开渲染，但官方说那更像在比"渲染下一帧前批量缓冲多少输入"，横比意义有限。
- konsole / gnome-terminal / xterm 不支持同步更新转义码，**若支持，数字可能提升 20–50%**。
- foot / iTerm2 / Terminal.app 因不跑 X11 被排除；alacritty+tmux 只是用来展示"中间加一层复用器会把吞吐砍半"。

> 对三款对比对象的影响：**这张表里没有 Ghostty**，且 WezTerm 是 2023 年的旧构建。它能说明"kitty 的解析路径短、WezTerm 的 mux 层有额外开销"，**不能**用来断言 2026 年三者的排序。

### 3.2 端到端大文件吞吐（独立基准，可比性最好）

| 基准 | WezTerm | Ghostty | Kitty | 口径 |
|---|---:|---:|---:|---|
| [S6] `time cat` 11 MB 莎士比亚，5 次均值 | **1246 ms** | 407 ms | 401 ms | T430 / Hyprland，174×39 附近 |
| [S7] `cat` 16 MiB base64 行，6 次取后 5 次中位数 | **916 ms** | **195 ms**（1.4.0） | 212 ms | Ubuntu 26.04 VM / VirGL，100×30 |
| [S8] `bench.sh` total（M1 Ultra） | 5.131 s | **2.247 s** | 5.203 s | macOS，未给版本 |
| [S8] termbench TermMarkV2 Normal | 102.69 s (0.0278 gb/s) | **47.98 s (0.0595 gb/s)** | 80.93 s (0.0353 gb/s) | macOS M1 Ultra |

对照组（说明量级）：[S6] foot 251 ms、alacritty 404 ms；[S7] foot 168 ms、alacritty 256 ms、Rio 215 ms。

**读数**：WezTerm 在两份独立基准里都慢约一个数量级的一半以上（相对领先者 3.1× 与 4.7×），是最一致的结论。Ghostty 与 Kitty 在 2026 年的 [S7] 里只差 17 ms（195 vs 212），已经属于同一档；而 [S6] 的 Ghostty 是 1.0 时代版本，407 ms 与 Kitty 401 ms 持平，正好佐证 1.4 的跃升。

[S7] 还给出一个可操作的诊断：**Kitty 0.45.0 的 Ubuntu 包首帧 558 ms 是 Python 字节码缓存权限问题**（`strace` 抓到 135 次 `opt-2.pyc ... EACCES`），执行 `sudo python3 -OO -m compileall -q /usr/lib/kitty` 后降到 199 ms，吞吐不变。这说明"启动慢"未必是架构问题。

### 3.3 持续高频刷新帧率 [S6]

DOOM-fire-zig，窗口约 174×39，5 次均值，越高越好：

| Alacritty | Foot | **Ghostty** | **Kitty** | **WezTerm** |
|---:|---:|---:|---:|---:|
| 164 FPS | 199 FPS | 135 FPS | **273 FPS** | **64 FPS** |

Kitty 领先第二名 37%，是最能体现"字形图集 + 批量顶点 + 增量重绘"在**小更新高频场景**下成本恒定的指标。WezTerm 的 64 FPS 与它在吞吐上的落后方向一致。

> 注意：这是 240Hz 以上才会真正体现差距的极端场景。60Hz 屏幕上三者都会被 VSync 钳到 60。

### 3.4 键盘到屏幕延迟

**组 A：[S6] 用 is-it-snappy（键灯亮 → 字符首次出现），5 次均值，T430 / Hyprland**

| Alacritty | Foot | **Ghostty** | **Kitty** | **WezTerm** |
|---:|---:|---:|---:|---:|
| 16.7 ms | 15.0 ms | 38.3 ms ⚠️ | 18.3 ms | 30.8 ms |

⚠️ Ghostty 的 38.3 ms 是明显离群值。[S11] 判断为"Hyprland 下 GTK/XWayland 的测量伪影"，[S8] 与 [S13] 也把 Ghostty 放在第一梯队。**这一格不建议作为结论依据。**

**组 B：[S12] typometer，Arch / i3 on X11 / Intel 集显，Avg(SD) ms，单用户实测**

| 合成器状态 | Ghostty | Kitty | WezTerm | 说明 |
|---|---:|---:|---:|---|
| 无 picom | **18.8** (3.8) | 24.4 (5.8) | 22.7 (3.7) | 三者差 5.6 ms |
| picom --realtime | 33.2 (11.6) | **30.8** (10.3) | 33.6 (11.1) | 合成器 +12 ms |
| picom（默认） | 63.4 (45.5) | **57.2** (18.5) | **56.2** (15.4) | 合成器 +35 ms，Ghostty 出现 611.8 ms 尖峰 |

**最有价值的不是名次，是量级**：合成器带来的 12～35 ms 远大于终端之间的差异 3～6 ms。[S1] 官方也承认 kitty 有人工延迟默认值（`input_delay 3 ms`、`repaint_delay`），调成 `input_delay 0 / repaint_delay 2 / sync_to_monitor no` 可以再压低，代价是耗电与可能的画面撕裂。

### 3.5 冷启动首帧 [S7]

GNOME Shell extension 记录合成器收到首帧的时刻，3 次预热 + 20 次中位数；窗口 100×30，发行版默认配置。

| 终端 | 首帧 | 备注 |
|---|---:|---|
| foot（CPU 对照） | 39 ms | 全场最快 |
| Rio / Alacritty | 98 / 100 ms | |
| **WezTerm** nightly 20260929 | **101 ms** | 三者中最快 |
| **Kitty** 0.45.0（预编译字节码后） | **199 ms** | 修复后 |
| **Ghostty** 1.4.0 | **308 ms** | GTK4 外壳 |
| **Kitty** 0.45.0（Ubuntu 原包） | **558 ms** | 缓存权限问题 |

[S7] 的关键观察：GNOME Console / GNOME Terminal / Ptyxis 与 Ghostty（同用 GTK/VTE 系窗口）首帧都落在 **308–348 ms**，说明**工具包初始化设了地板，不是渲染器**。Kitty 的瓶颈是 Python 解释器与模块编译（一次 `compileall` 就能砍掉 359 ms）。WezTerm 的单二进制 + 延迟加载在这里占优。

> [S13] 提醒：各来源对 WezTerm 启动的估值从 56 ms 到 165 ms 不等，**三者官方都没有发布相互对标的启动基准**，把启动数字当精确值用要谨慎。

### 3.6 空闲内存

| 来源 | WezTerm | Ghostty | Kitty | 口径 |
|---|---:|---:|---:|---|
| [S6] | 130 MB | 174 MB | **91 MB** | `btop` 重启后，5 次均值，T430 |
| [S7] | **66.8 MiB** | 78.9 MiB | 88.0 MiB | `smaps_rollup` PSS 总和，启动 10 s，3 次中位数，不含 shell |

两份数据**绝对值差 2 倍**（口径不同：RSS vs PSS、不同发行版/版本/是否计入多进程、VM vs 裸机），但**相对关系一致：Kitty 与 WezTerm 较省，Ghostty 单窗口稍高**。

[S11] 转述的多窗口伸缩测量解释了 Ghostty 的设计取舍 —— 单进程共享字体/图集/服务，窗口越多越划算：

| 窗口数 | Ghostty | Foot (client/server) | Foot (standalone) |
|---:|---:|---:|---:|
| 1 | 229 MB | **43 MB** | 35 MB |
| 5 | **260 MB** | **125 MB** | 175 MB |
| 10 | **300 MB** | 225 MB | 350 MB |
| 25 | **423 MB** | 538 MB | 875 MB |

Ghostty 1.4 官方又补了一刀[S3]：scrollback 压缩让不可见历史的**物理内存降 70–90%**，默认上限从 10 MB 提到 50 MB；新空终端页 128 KiB → 16 KiB；20 个隐藏标签页的 GPU 内存 385 MiB → 18 MiB。所以 [S6] 的 174 MB 是 1.0 时代的旧数，不能直接套到 1.4。

### 3.7 CPU 占用 / 能耗 [S1]

kitty 官方测 `less` 连续滚动大文件（htop，终端进程 + X 合计；i7-4820K + Radeon HD 7770）：

| 终端 | CPU | 备注 |
|---|---:|---|
| **kitty** | **6–8%** | 官方称滚动明显更顺 |
| xterm | 5–7% | 但"滚动极其卡顿" |
| termite | 10–13% | |
| urxvt | 12–14% | |
| gnome-terminal | 15–17% | |
| konsole | 29–31% | |

**这张表不含 WezTerm 与 Ghostty**，只能说明 kitty 的设计取舍（字形缓存进显存、SIMD 解析、一帧只向 GPU 发几字节增量）在 CPU 侧有效，不能用于三者横比。官方明确说没有能耗测量基础设施，用 CPU 占用代替。

### 3.8 渲染架构与后端（解释上面的数字）

| | WezTerm | Ghostty | Kitty |
|---|---|---|---|
| 语言 | Rust + 内嵌 Lua | Zig（macOS 侧 Swift/AppKit） | C 热路径 + Python 配置/插件 |
| 渲染 | 自绘；`front_end = OpenGL`（默认）/ `WebGpu`(wgpu → Metal/Vulkan/**DX12**)/ `Software`(CPU)[S9] | macOS Metal；Linux GTK4 + OpenGL/EGL，1.4 起用独立渲染线程 + DMABUF + 三缓冲[S3] | 自绘 OpenGL，字形图集常驻显存，批量顶点数组，增量重绘[S1] |
| 线程 | GUI / mux / term 三层，Mux 单例 + PDU 多路复用[S14] | 每个 Surface 三条线程：主线程(输入/UI) / 渲染线程 / IO 线程(PTY)[S15] | PTY IO 与渲染分离线程，字节流 SIMD 解析[S1] |
| 配置 | Lua（热重载） | 扁平 `key = value` | `kitty.conf` |
| 官方平台 | **Windows** / macOS / Linux / FreeBSD / NetBSD[S10] | macOS / Linux；**GUI 已放弃 Windows 路线图**[S3] | Linux / macOS / BSD[S2] |

**一个对本项目特别相关的细节**：WezTerm 在检测到 Windows 远程桌面环境时会**自动切到 `Software` 前端**[S9]。这说明"GPU 优先、拿不到设备就回退 CPU"是成熟终端的共同兜底策略。

---

## 4. 分项结论

| 维度 | 领先 | 落后 | 把握度 |
|---|---|---|---|
| 端到端大文件吞吐 | Ghostty 1.4 / Kitty（195 / 212 ms，2026 同机） | **WezTerm 916 ms** | ★★★ 两份独立基准同向 |
| 持续刷新帧率 | **Kitty 273 FPS** | WezTerm 64 FPS | ★★☆ 单一独立基准 |
| 解析吞吐 | Kitty 134.55 MB/s（2023 官方，无 Ghostty 对照） | WezTerm 48.5 MB/s | ★★☆ 厂商自测，旧版本 |
| 冷启动首帧 | **WezTerm 101 ms** | Ghostty 308 ms（GTK4）/ Kitty 558 ms（可修复到 199） | ★★★ 方法最严谨的一份 |
| 空闲内存 | Kitty ≈ WezTerm | Ghostty（1.0 时代；1.4 已大幅改善） | ★★☆ 口径不一致 |
| 输入延迟 | 三者差距 3–6 ms，无稳定赢家 | —— | ★☆☆ 受合成器支配，[S6] 的 Ghostty 是离群值 |
| 多窗口/大 scrollback 内存 | **Ghostty 1.4**（压缩 70–90%，共享资源） | —— | ★★★ 官方数据 + 第三方伸缩测量 |
| 平台覆盖（Windows） | **WezTerm 唯一** | Ghostty / Kitty 均不支持 | ★★★ 官方一手 |

**逐个画像**：

- **WezTerm**：功能与平台覆盖最强（内建 mux、免 tmux 会话持久化、五平台、Lua 脚本），但**数据路径最长**——本地 PTY 也被抽象成 domain/pane，配上 Lua 运行时，代价体现在吞吐与帧率两项 consistently 垫底。启动与内存反而不差。**适合：跨平台统一配置、SSH 会话常驻、需要免 tmux 持久化。**
- **Ghostty**：2026 年的性能跃升最猛（1.4 的 SIMD 解析器、独立渲染线程、scrollback 压缩），端到端吞吐已经反超；但 Linux 的 GTK4 外壳把启动钉在 300 ms 量级，且**没有 Windows GUI**。**适合：macOS 原生体验、大量窗口/长历史、最新稳定版用户。**
- **Kitty**：架构上最"教科书"——字形图集 + SIMD 解析 + 增量 GPU 提交，持续刷新帧率与 CPU 占用都最好；短板是 Python 运行时带来的启动开销（多数可一次修复）和单窗口内存。**适合：高频滚动输出、编译日志、图形协议 / 键盘协议生态。**

---

## 5. 已排除的不可信来源（重要）

检索中命中大量疑似批量生成的"性能评测"，本报告**未采用其任何数字**，理由是：无测量方法/无版本/无硬件，且数字与一手来源直接冲突。

| 来源 | 典型说法 | 排除理由 |
|---|---|---|
| CSDN `gitblog_00865` / `adg.csdn.net` 同源文 | "1GB 文本流 Ghostty 187 MB/s"、"冷启动 68 ms"、"纹理绑定减少 67%" | 与官方 725 MB/s ASCII 解析、[S7] 首帧 308 ms 冲突；宣称的源码文件名无法在仓库核实 |
| `binodbhatt.com.np` "2026 Shootout" | "示波器 + 1000 FPS 高速相机，Ghostty 3.1 ms / WezTerm 6.8 ms" | 单源、方法不可复现；"1% Low 4.2 ms" 与 avg 3.1 ms 表述不自洽 |
| `devtoolreviews.com` | "Ghostty cat 100K 行 0.7 s、输入延迟 ~2 ms" | 无版本、无命令、无硬件细节；与 [S12] 的 18–63 ms 量级冲突 |
| CSDN `gitblog_00905` "vtebench 对比" | "滚动 42.7 / 38.2 / 35.8 MB/s" | 数值体系与 [S1][S8] 的 vtebench 输出（毫秒制）不符，疑为拼凑 |
| 各类 "X 比 Y 快 N 倍" 图表 | —— | [S13] 明确指出这些图基本都早于 Ghostty 1.0（2024-12-26）或对比旧构建 |

---

## 6. 与本项目 VectorTerm 的对照（**本地实测，非网络数据**）

> 本节数字来自本项目 `cargo test bench_paint -- --ignored --nocapture` 的本地 debug 构建（Windows / wgpu DX12 / Intel Graphics），**与第 3 节的网络数据不可横比**：测量的是单帧合成耗时，不是端到端 cat 或键盘延迟。

| 阶段 | 耗时 | 谁承担 |
|---|---:|---|
| 快照（终端锁内拷贝） | 0.80 ms | 两条路径都要 |
| 布局（生成显示列表） | 0.71 ms | 两条路径都要 |
| 光栅化（全屏逐像素混合） | 10.04 ms | **仅 CPU 路径**（纯背景填充 2.46 ms） |
| **CPU 单帧合计** | **11.55 ms** | 200×60 格 / 1816×976 / 1.8 MPx |
| **GPU 单帧合计** | **1.51 ms** | 像素交给显卡 |

60 fps 预算是 16.7 ms，CPU 路径一个大窗口就吃掉 2/3 —— 这正好对应第 3.3 / 3.7 节的结论：**全屏重绘是 CPU 光栅化的死穴，kitty 的解法（字形图集 + 批量顶点 + 增量提交）与本项目新增的显示列表 + R8 图集 + 实例化四边形是同一条路**。

三点可迁移的经验：

1. **显示列表 + 跨帧字形图集**是三家共同的架构共识，本项目方向正确。
2. **`auto / cpu / gpu` 三态 + GPU 初始化失败回退 CPU** 与 WezTerm 的 `OpenGL / WebGpu / Software` + 远程桌面自动 Software 是同一个兜底逻辑[S9]。
3. **还没补的两项**：kitty/WezTerm/Ghostty 都有独立 PTY IO 线程与解析/渲染解耦，本项目目前仍是"快照 → 布局"同帧串行；以及**首帧时间与输入延迟本项目尚无测量**，而这两项恰是第三方基准里差距最容易感知的维度。建议后续按第 7 节的口径补测。

---

## 7. 若要得到真正可比的结论，建议这样复现

公开资料最大的缺口是没人做过"三款同机同日"的对照。建议的最小充分测试方案：

**统一条件**：同一台裸机（不用 VM / VirGL）、同一天、各项目最新稳定版；窗口固定 120×40；同一字体与字号、同一配色、相同 scrollback 行数；关闭透明/模糊/连字；固定合成器状态（建议关闭并单独记录一组开启数据）；固定刷新率与 VSync 状态；每个指标 ≥10 次取**中位数并报 p95**，同时记录 CPU 占用。

| 维度 | 工具 / 命令 | 口径 |
|---|---|---|
| 解析吞吐 | `kitten __benchmark__`（与 `--render` 各一组）[S1] | MB/s，注明是否抑制渲染 |
| 分项渲染 | `vtebench`（dense_cells / scrolling / unicode 等 9 项）[S8] | 每项 avg + 90 分位 + 样本数 |
| 端到端吞吐 | `cat` 固定 16 MiB 文件，终端内 `date +%s%N`[S7] | 从命令起到最后一行绘制完成 |
| 输入延迟 | typometer 或 is-it-snappy（高速摄像）[S6][S12] | 键触发 → 字符首次出现，声明合成器 |
| 启动 | 合成器首帧钩子 / 高速摄像，20 次中位数[S7] | fork → 合成器收到首帧 |
| 帧率 | DOOM-fire-zig 全屏重绘[S6] | FPS |
| 内存 | `/proc/<pid>/smaps_rollup` PSS 总和[S7] | 启动 10 s 后，不含 shell；另测 1/5/10/25 窗口伸缩 |

**Windows 特别说明**：Ghostty / Kitty 无法原生参与，若要做 Windows 横评，应改为 WezTerm（Windows）对 VectorTerm / Windows Terminal / Alacritty，并单独记录 DX12 后端与是否远程桌面。

---

## 8. 数据来源清单

全部于 **2026-11-05** 取用。

| 编号 | 来源 | URL | 本报告引用的内容 |
|---|---|---|---|
| [S1] | kitty 官方 *Performance* | <https://sw.kovidgoyal.net/kitty/performance/> | 解析吞吐表、CPU 占用表、渲染抑制/同步更新/排除终端注记、延迟调优配置 |
| [S2] | kitty 官网首页 | <https://sw.kovidgoyal.net/kitty/> | 官方平台列表（Linux/macOS/BSD） |
| [S3] | Ghostty 1.4.0 Release Notes（官方） | <https://ghostty.org/docs/install/release-notes/1-4-0> | 128→725 MB/s、cat 150 MB 1.5 s→566 ms、scrollback 压缩、渲染线程/三缓冲、放弃 Windows GUI 路线图 |
| [S4] | Ghostty 1.3.0 Release Notes（官方） | <https://ghostty.org/docs/install/release-notes/1-3-0> | "Windows support is still not planned" |
| [S5] | Ghostty 官方 Build from Source | <https://ghostty.org/docs/install/build> | 仅有 Linux / macOS 构建章节、Zig 版本矩阵 |
| [S6] | vizenc/terminal-benchmark（GitHub，独立第三方） | <https://github.com/vizenc/terminal-benchmark> | 延迟 / 11 MB cat / DOOM-fire FPS / 内存四张表及完整 5 次运行、测试机配置 |
| [S7] | ComputingForGeeks, *15 Emulators Tested and Benchmarked*（2026-10） | <https://computingforgeeks.com/best-terminal-emulators-for-linux/> | 首帧 / PSS / 16 MiB cat 全表、VirGL 环境、三种测量口径、Kitty 字节码修复、Ghostty 1.3.1→1.4 对比 |
| [S8] | caluga.de, *New Terminal Ghostty*（2024-12） | <https://www.caluga.de/v/2024/12/27/ghostty?lang=en> | bench.sh / termbench / vtebench 结果，M1 Ultra 与 i9 双机 |
| [S9] | WezTerm 官方 `front_end` 文档 | <https://wezfurlong.org/wezterm/config/lua/config/front_end.html> | OpenGL / WebGpu / Software、DX12 映射、远程桌面自动 Software |
| [S10] | WezTerm 官方 Download | <https://wezfurlong.org/wezterm/installation.html> | Windows / macOS / Linux / FreeBSD / NetBSD |
| [S11] | grigio.org 汇总（含 Umar Anis 多窗口内存数据） | <https://grigio.org/which-is-the-best-linux-terminal-alacritty-kitty-ghostty-foot-its-not-easy/> | Ghostty 延迟离群值判断、多窗口内存伸缩表 |
| [S12] | Hacker News typometer 讨论（镜像索引） | <https://innerself-hn.com/item?id=48103075> | 三组合成器状态下的 typometer 延迟表；**原页无法直连核验，数据取自搜索索引** |
| [S13] | SumGuy, *Ghostty vs Kitty vs WezTerm in 2026* | <https://www.sumguy.com/ghostty-vs-kitty-vs-wezterm/> | 版本/平台现状、"三者官方均无相互对标基准"、对旧倍率图的提醒 |
| [S14] | DeepWiki, WezTerm Architecture Overview | <https://deepwiki.com/wezterm/wezterm/1.2-architecture-overview> | term / mux / gui 三层、Mux 单例与 PDU 协议 |
| [S15] | Ghostty Architecture 文档镜像 | <https://docsmith.aigne.io/docs/ghostty/en/architecture-ed730d> | Surface 三线程模型、平台原生 UI 分层 |

---

## 附录 A：[S6] 完整原始数据（5 次运行 + 均值）

| 指标 / run | 1 | 2 | 3 | 4 | 5 | avg |
|---|---:|---:|---:|---:|---:|---:|
| 输入延迟 Ghostty (ms) | 41.7 | 37.5 | 37.5 | 37.5 | 37.5 | **38.3** |
| 输入延迟 Kitty (ms) | 20.8 | 12.5 | 20.8 | 16.7 | 20.8 | **18.3** |
| 输入延迟 WezTerm (ms) | 29.2 | 33.3 | 25.0 | 37.5 | 29.2 | **30.8** |
| 11 MB cat Ghostty (ms) | 393 | 420 | 423 | 409 | 390 | **407** |
| 11 MB cat Kitty (ms) | 396 | 418 | 386 | 398 | 407 | **401** |
| 11 MB cat WezTerm (ms) | 1248 | 1298 | 1157 | 1269 | 1258 | **1246** |
| FPS Ghostty | 135 | 135 | 135 | 134 | 138 | **135** |
| FPS Kitty | 274 | 271 | 268 | 272 | 278 | **273** |
| FPS WezTerm | 67 | 64 | 67 | 61 | 63 | **64** |
| 内存 Ghostty (MB) | 170 | 176 | 173 | 178 | 173 | **174** |
| 内存 Kitty (MB) | 91 | 91 | 89 | 91 | 93 | **91** |
| 内存 WezTerm (MB) | 127 | 127 | 134 | 128 | 134 | **130** |

## 附录 B：[S7] 完整 15 款基准表（按首帧排序，保留原文行序）

| 终端 | 渲染 | 首帧 ms | PSS MiB | cat 16 MiB ms | 版本 |
|---|---|---:|---:|---:|---|
| foot | CPU (pixman) | 39 | 6.9 | 168 | 1.25.0 |
| st | CPU (Xlib) | 45 | 3.4 | 278 | 0.9.3 |
| rxvt-unicode | CPU (Xlib) | 62 | 10.1 | 215 | 9.31 |
| Rio | GPU (wgpu) | 98 | 60.8 | 215 | 0.5.28 |
| Alacritty | GPU (OpenGL) | 100 | 39.0 | 256 | 0.16.1 |
| **WezTerm** | GPU (OpenGL) | **101** | **66.8** | **916** | nightly 20260929 |
| xterm | CPU (Xlib) | 125 | 7.0 | 485 | 407 |
| Konsole | Qt | 136 | 40.7 | 477 | 25.12.3 |
| Tilix | VTE (GTK3) | 185 | 36.7 | 376 | 1.9.6 |
| Terminator | VTE (GTK3) | 263 | 36.8 | 296 | 2.1.5 |
| **Ghostty** | GPU (EGL/GTK4) | **308** | **78.9** | **195** | 1.4.0 |
| GNOME Console | VTE (GTK4) | 317 | 67.2 | 461 | 49.2 |
| GNOME Terminal | VTE (GTK3) | 322 | 25.3 | 325 | 3.58.0 |
| Ptyxis | VTE (GTK4) | 348 | 68.2 | 346 | 50.1 |
| **Kitty (stock)** | GPU (OpenGL) | **558** | **88.0** | **212** | 0.45.0 |
| **Kitty (bytecode compiled)** | GPU (OpenGL) | **199** | **88.0** | **212** | 0.45.0 |

---

*报告口径：对比数据全部来自公开网络来源并标注出处；第 6 节的 VectorTerm 数字为本项目本地实测，已单独标注、不参与横评。*
