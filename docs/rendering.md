# 6. 渲染后端与性能

## 一次布局,两条路径

无论选哪个后端,每帧的**布局只算一遍**:`render.rs` 把这一帧要画的东西写进一份显示列表(`src/frame.rs`),条目只有三种:

```rust
enum Item {
    Rect  { x, y, w, h, color, alpha },                        // 背景、光标块、装饰线
    Glyph { id, src, dst, color, italic, bold },               // 图集里的字形 + 目标矩形
    Trail { corners: [Point2; 4], color, alpha, mask },        // 尾迹(四角可自由变形)
}
```

CPU 后端(`paint_cpu`)读这份列表做逐像素 alpha 混合,GPU 后端(`gpu.rs`)把同样的列表翻译成实例化四边形。这样**两条路径的几何必然一致** —— 不会出现"切到 GPU 之后下划线偏了半像素/尾迹位置变了"这类分叉。有个 `gpu_matches_cpu` 测试专门守这条线:同一份 `Frame` 分别走两条路径,读回像素逐点比对(当前 536×196 下平均通道误差 1.63、差异像素 1.77%,全落在抗锯齿边缘)。

## GPU 后端怎么画一屏

```text
字形首次出现 → ab_glyph 栅格化成 8bit 灰度 → 塞进 2048×2048 的 R8 图集(货架式分配,1px padding)
每帧         → 脏矩形上传图集(通常 0~2 块) → 实例缓冲写 N 个四边形
             → set_scissor_rect(裁掉 padding 区) → draw(0..6, 0..N) 一次画完
尾迹         → 单独一条管线:4 个角作为 uniform,片元里对光标矩形 discard 挖空
```

字体图集是**跨帧缓存**的:ASCII 字符只在第一帧上传一次,之后再出帧只需重写实例缓冲。

## 字符显示:按字形角色排版

移植 Oh My Posh / Starship 这类主题时最容易翻车的是 powerline 字形(`U+E0B0`–`E0B7`)和 Nerd Font 图标。实测 `MesloLGL Nerd Font Mono` 的箭头墨迹是 14×32px,而 18px 字号下一个格子只有 11×20px —— 旧实现只做横向缩放,纵向放任溢出,结果就是箭头**顶穿上下一行**、图标互相压住、色块之间露出深色竖缝。

现在按四类角色分别排:

| 角色 | 范围 | 排版规则 |
|---|---|---|
| `Separator` | powerline `U+E0B0`–`E0B7` | **铺满整格**(横向铺满 + 上下顶到格边),接缝由相邻格子自己盖住,没有竖缝 |
| `BoxGlyph` | 制表符 `U+2500`–`257F`、方块 `U+2580`–`259F` | 按 em 盒映射再裁边,保住线宽比例,`│`/`─` 才能连成一条线 |
| `Icon` | 私有区(PUA)图标 | 等比缩进格子 + 垂直夹取,不越界不压邻格 |
| `Text` | 其余字符 | 保持字体自然大小,只在超格时等比缩小(**只缩不拉**) |

缩放一律走面积平均的 box filter,不是最近邻 —— 否则笔画会被压成色块。

## 性能(release 构建 / 200×60 格 / 画布 1816×976 / 满屏彩色文本)

| 阶段 | CPU 软渲染 | GPU (DX12) |
|---|---:|---:|
| 快照 + 布局 | 1.51 ms | 1.33 ms |
| 光栅化 / GPU 编码 | 12.23 ms | 0.95 ms |
| **CPU 侧单帧合计** | **13.74 ms** | **2.28 ms** |

真实窗口 1920×1080 满屏:GPU 帧间隔稳定 3.33 ms(受 DXGI 提交节流;显示器 60Hz 时用户看到的是 60 FPS),CPU 14.5 ms 且 p95 达 19 ms,会偶发掉帧。代价在别处:**冷启动首帧 GPU 580 ms / CPU 112 ms,峰值内存 GPU 485 MB / CPU 55 MB**。

完整实测数据、口径说明(逐帧同步伪影、debug 数字不可比、ConPTY 未完成项)与优化优先级见 [`benchmark/vectorterm-performance-benchmark.md`](../benchmark/vectorterm-performance-benchmark.md)。全部报告、原始 CSV、基准配置与一键复现脚本统一放在 [`benchmark/`](../benchmark/README.md);本机复现:`powershell -ExecutionPolicy Bypass -File benchmark\run.ps1`。

## 配置

```lua
render = "cpu"     -- 默认:软渲染 + GDI BitBlt,拖动缩放窗口不抖
-- render = "gpu"  -- wgpu/DX12;更快,但拖动缩放时旧帧会被 DXGI 拉伸
-- render = "auto" -- 先试 GPU,拿不到设备再退回软渲染
gpu_power = "low"  -- 双显卡机器:low = 优先核显(默认,省电);high = 优先独显
```

启动时 stderr 会打印实际走的是哪条,并带上设备类型和图形后端:

```text
[VectorTerm] 渲染后端:CPU 软渲染
[VectorTerm] 渲染后端:GPU(AMD Radeon(TM) 610M / 核显 / Dx12) —— 拖动缩放窗口时旧帧会被 DXGI 拉伸, 想避免请改用 render = "cpu"
[VectorTerm] 渲染后端:GPU(NVIDIA GeForce RTX 5060 Laptop GPU / 独显 / Dx12) —— ...
[VectorTerm] 未找到可用的 GPU(...),使用 CPU 软渲染
```

改 `render` / `gpu_power` 需要重启(热重载只提示不切换)。

## 6.5 为什么默认用 CPU

GPU 路径在**拖动窗口右边或下边**时,内容(包括标签栏文字)会被横向/纵向拉长一下再弹回,每拖一点来一次。原因不在"画得慢",而在 DXGI 的呈现语义:

1. 窗口客户区先变大,新尺寸的后缓冲要等 `ResizeBuffers`(实测 4~5ms);
2. 这段空档里 DXGI 发现后缓冲尺寸 ≠ 呈现目标尺寸,按 **`DXGI_SCALING_STRETCH`** 把旧画面缩放铺满新客户区;
3. 新帧上屏后画面弹回正常比例。

`wgpu-hal-24.0.4/src/dx12/mod.rs:1135` 把 `Scaling` 硬编码成了 `STRETCH`,而 `wgpu::SurfaceConfiguration` 没暴露这个字段——所以应用层无论怎么优化呈现时机,都只能把窗口期从 8ms 压到 1ms,压不掉 vsync 的 16.7ms 网格和概率性跨拍。

CPU 路径用 `Gdi::BitBlt` 做 1:1 像素拷贝,**没有"适配呈现目标尺寸"这个环节**,所以这条链根本不存在。拖得再快,最坏也只是新区域晚几毫秒画上(露背景色),不是文字变形。

代价是 CPU 单帧更贵(1080p 满屏 14.5ms vs GPU 2.3ms,见上表)。所以:**日常默认 CPU;4K/高刷/满屏滚动/大量图片时,可以在配置里显式写 `render = "gpu"`,接受拖动时的拉伸。** 想要两边都要,就得改 DXGI 的 `Scaling`(方案见 [`resize-fix-plan.md`](resize-fix-plan.md))。

两条路径的完整机制(谁负责排版、谁负责填像素、已做的三次缓解各卡在哪)见 [`rendering-backends.md`](rendering-backends.md)。

## 适配器选择

借鉴 ghostty 的做法(每个平台固定一个图形 API、初始化失败时明确告知而不是黑屏),但 Windows 上可以更宽容:一次能摸到 DX12 / Vulkan / GL 三套后端,所以这里做的是**枚举 → 排序 → 逐个尝试**。

1. 用 `DX12 | VULKAN | GL` 建 instance,`enumerate_adapters` 拿到全部适配器
2. 过滤掉软件适配器(WARP / Microsoft Basic Render Driver / lavapipe)——本项目的 CPU 光栅化比它们快,真到这一步不如直接走软渲染
3. 排序:**先比后端**(DX12 > Vulkan > GL),**再比设备类型**(`gpu_power = "low"` 时核显优先,`"high"` 时独显优先)
4. 逐个 `request_device`,第一个成功的就用;某个驱动拒绝就试下一个,而不是直接掉到 CPU

**交换链在设备创建成功之后才建**。反过来的话,GPU 用不了的机器上 HWND 会先被 DXGI 挂上交换链,再交给 softbuffer 行为不可控 —— 现在的顺序保证"GPU 完全不可用"时窗口从未被 GPU 碰过。

排除软件适配器的默认行为可以用 `VECTORTERM_GPU_ALLOW_SOFTWARE=1` 放开(一般只用于排查)。

## 图片协议(iTerm2 / Kitty)

协议解析不自己写:wezterm-term 里 `iterm` / `kitty` / `sixel` 三条路径最后都落到同一个 `assign_image_to_cells`,把一张图切成"一格一块"的 `ImageCell` 挂到 `CellAttributes` 上。所以只要打开 `TerminalConfiguration::enable_kitty_graphics()`(Kitty 协议默认是关的),两种协议就都通了。

```lua
images = { enable = true, max_memory_mb = 256 }
```

渲染侧的做法:

1. **快照带着 uv 走**。每个 `ImageCell` 记录了本格在图里的纹理坐标 `tl`/`br` 和四个方向的 `padding`(格子里没被图覆盖的像素)。
2. **一图一四边形**。逐格画会在格线处留下接缝,所以反过来算:某格的 uv 跨度是 `du`,该格实际被图覆盖的宽度是 `格宽 - 左右 padding`,于是整图宽 `= 覆盖宽 / du`,左上角 `= 本格位置 - tl × 整图宽`。从任意可见格出发都成立,图片上沿滚出视口也不会飘。同一张图(按内容哈希 + z + 锚点)只发一次。
3. **padding 要换算坐标系**。`padding` 是 wezterm 用"终端自己假定的格子像素"算的,和渲染器按字体度量出来的格子未必相同(测试里是 10×20 vs 10×18)。不换算的话,"比一格还小"的图(比如 2×2 px 的 Kitty 图)会被算成"本格没被覆盖"而整张丢掉。
4. **z 序**:`z_index < 0` 画在文字下(常见于背景图),其余画在文字上;同层按 z 升序。
5. **GPU 侧不新开管线**。图片实例只是把 bind group 从字形图集换成自己的纹理,同一条管线同一份实例缓冲;`Batches` 按"相邻同 bind 合并成段"切分,每段一次 `draw`。实例里用 `extra.x = -1` 作为"这是图片"的哨兵(新增 varyings 会让 GPU/CPU 像素对比从 1.85 恶化到 70,所以没这么干)。

> 一个踩过的坑:片元着色器里写"图片实例提前 `return s`"会在 Intel 的 DXIL 编译器上把输出寄存器的 **r 分量**留着不覆盖,整屏红通道恒为 255。改成单出口 + `select` 后正常 —— 现在 CPU/GPU 两条路径的圆角、图片、文本对比分别是 **0.00 / 0.83 / 1.85**(平均通道误差)。
