# 渲染后端：CPU 与 GPU 各自怎么工作

> 对应配置项 `render = "cpu" | "gpu" | "auto"`。**默认 `cpu`**。

## 0. 一句话对照

| | CPU（默认） | GPU |
|---|---|---|
| 谁填像素 | CPU 逐像素软件混合 | 显卡（DX12 光栅化） |
| 怎么上屏 | GDI `BitBlt` 1:1 拷贝 | DXGI 交换链 present |
| 拖动缩放会抖 | **不会** | **会**（旧帧被 DXGI 拉伸） |
| 满屏彩色文本单帧 | 13.7ms（200×60） | 2.3ms |
| 冷启动首帧 | 112ms / 55MB | 580ms / 485MB |

---

## 1. 共同的上游：布局只算一遍

两条路径**不是两套渲染器**，而是同一份显示列表的两个消费者：

```text
ConPTY 字节流 → wezterm-term 解析 → 屏幕网格
              → render.rs::snapshot() → Frame { items, clip_top, clip_bottom, overlay_at, clear }
                                            │
                             ┌──────────────┴──────────────┐
                        paint_cpu()                     gpu.draw()
                    （逐像素混合到 u32）            （翻译成实例化四边形）
```

`Item` 只有三种：`Rect`（背景/光标块/装饰线）、`Glyph`（图集字形 + 目标矩形）、`Trail`（四角可自由变形的尾迹）。因为几何在 `snapshot` 阶段就定死了，切后端不会出现"下划线偏半像素""尾迹位置变了"这类分叉——`gpu_matches_cpu` 测试专门守这条线（同一份 `Frame` 走两条路径，逐点比像素）。

**注意：两种模式都要先在 CPU 上排版。** 这一点决定了下面对"纯 GPU"的回答。

---

## 2. CPU 路径（默认）

### 2.1 绘制

`render.rs:1297` 的 `paint_cpu(buf: &mut [u32], w, h, frame)`：

1. `Canvas::fill_all(frame.clear)` —— 整屏铺终端背景色；
2. 顺序遍历 `frame.items`，逐条目混合：
   - `Rect` → `fill_rect`，按 `alpha` 做 `dst = src*a + dst*(1-a)`；
   - `Glyph` → `blit_glyph`，从字形缓存取 8bit alpha 位图，源/目标尺寸不同时走**面积平均 box filter**（不是最近邻），再混合；
   - `Trail` → 按四角多边形做遮罩混合；
3. 内容区按 `clip_top/clip_bottom` 裁剪；`overlay_at`（标签栏）之后取消裁剪，否则标签栏会被内容区裁掉。

### 2.2 字形从哪来

`rasterize_in()`（`render.rs:1623`）用 `ab_glyph` 的 `outline_glyph()` 拿到轮廓，`og.draw()` 回调逐点写出 alpha 值，得到 `CachedGlyph { w, h, left, top, advance, data }`，按 `(char, style)` 存在 `HashMap` 里。ASCII 只在第一次出现时栅格化。

### 2.3 呈现

```rust
surface.resize(w, h)?;          // 重新分配一块 w*h*4 的内存
let mut buffer = surface.buffer_mut()?;
renderer.paint_cpu(&mut buffer, w, h, &frame);
buffer.present()?;              // → GDI::BitBlt(dc, 0,0,w,h, buf.dc, 0,0, SRCCOPY) + ValidateRect
```

关键性质（这也是它不抖的原因）：

- **没有交换链**。那块 `u32` 内存和窗口没有任何绑定关系，只是"往窗口左上角贴 w×h 个像素"。
- **`BitBlt` 没有缩放能力**。它是同尺寸像素块拷贝，要缩放得显式调 `StretchBlt`。所以"把旧帧拉长铺满新窗口"这个动作物理上不存在。
- **`present` 同步返回**。不进 GPU 队列、不等 vsync、没有"帧在队列里、窗口已经变了"的空窗。

代价：单核、耗时与像素数成正比（1080p 满屏约 14.5ms，p95 可到 19ms）。

---

## 3. GPU 路径：不是纯 GPU，是"CPU 排版 + GPU 合成"

### 3.1 分工

| 阶段 | 谁做 |
|---|---|
| 读 ConPTY、VT 解析、回滚、软折行 | CPU |
| 网格 → 显示列表（每格位置、裁剪、光标） | CPU |
| **字形栅格化**（`ab_glyph` 轮廓 → 8bit alpha 位图） | **CPU** |
| 脏矩形上传字形图集（R8，2048×2048 货架分配） | CPU 发起，GPU 执行 |
| 四边形填充、纹理采样、alpha 混合、裁剪 | **GPU** |
| 合成上屏（DXGI 交换链 + DWM） | **GPU** |

所以准确说法是：**GPU 模式把"填充像素与上屏"搬到了显卡，排版和字形栅格化仍在 CPU。** 没有任何终端是"纯 GPU"的——即便字形也用 GPU 栅格化（SDF/MSDF 那类方案），VT 解析和排版仍然在 CPU。

### 3.2 每帧做什么

`gpu.rs:1140` 的 `draw()`：

```text
sync_atlas()   脏矩形上传新出现的字形（通常 0~2 块）
            → 遍历 items，把每个条目写成实例（6 顶点 quad + 实例属性：rect / uv / color / alpha）
            → 按批切换管线（普通 / 尾迹），set_scissor_rect 裁掉图集 padding
            → pass.draw(0..6, seg.start..seg.end)   一次实例化画完一屏
```

片元着色器只有一句关键逻辑：`textureSample(atlas, samp, in.uv) * color`，再按 alpha 混合。尾迹是例外：四个角作为 uniform 传进去，片元里对光标矩形 `discard` 挖空。

字体图集跨帧缓存，ASCII 只在首帧上传一次；之后每帧只重写实例缓冲。

### 3.3 呈现

`wgpu::Surface` 在 Windows 上就是一个 **DXGI flip-model 交换链**（`CreateSwapChainForHwnd`，`FLIP_DISCARD`），`present()` 把后缓冲交给 DWM 合成。

---

## 4. GPU 模式为什么会抖（已知问题）

### 4.1 现象

拖动窗口的**右边或下边**时，窗口里所有内容（不只是终端文字，**标签栏文字也一起**）被横向/纵向拉长一下，然后弹回；每拖一点就来一次。拖左边或上边基本看不到。

### 4.2 因果链

```text
① 系统把窗口矩形改大（客户区变大）
       ↓
② 后缓冲还是旧尺寸（新尺寸要等 ResizeBuffers，实测 4~5ms）
       ↓
③ 这段空档里如果 present 一帧，DXGI 发现「后缓冲尺寸 ≠ 呈现目标尺寸」
   → 按 DXGI_SCALING_STRETCH 把旧画面缩放铺满新客户区
       ↓
④ 新尺寸的帧终于上屏，画面"弹回"正常比例
```

三个条件同时成立才会抖：

1. **一帧的内容依赖新尺寸**。`compute_grid → commit_grid → term.resize()` 要先重排，才能用新列数算字形位置。没有新 grid 就画不出新帧。
2. **后缓冲尺寸必须等于客户区**（flip model 的硬要求），不等就得 `ResizeBuffers`——而这个调用同步阻塞 4~5ms（与 `PresentMode` 无关，`Immediate` 实测 4.8ms）。
3. **present 被 vsync 量化**（16.7ms 网格）。省下来的毫秒如果没赶上同一拍，观感完全一样。

### 4.3 为什么只在右边/下边明显

拖右边/下边时**窗口原点不动**，DWM 就地把旧尺寸的重定向表面拉伸铺满新客户区。拖左边/上边时窗口位置和尺寸同时变化，DWM 走的是另一条合成路径，形变落在窗口边缘而不是内容上，肉眼几乎看不到。

### 4.4 为什么标签栏也抖

因为标签栏和终端内容在**同一份显示列表、同一张交换链**里（`overlay_at` 只是裁剪分界，不是独立图层）。旧帧被整体缩放时，标签栏自然一起被拉。Windows Terminal 之所以没这个症状，是因为它的标签栏是 XAML 独立合成层。

### 4.5 已经做过的缓解，以及为什么不够

| 做了什么 | 效果 | 局限 |
|---|---|---|
| `WindowEvent::Resized` 里同步 `redraw + present` | 不再排队等下一轮消息泵 | 压不掉 `ResizeBuffers` 本身 |
| `WM_SIZING` 钩子提前把交换链配成新尺寸 | `EV_RESIZED → present` 从 5.0~5.2ms 降到 0.8~1.4ms | 只能把窗口期压到 1ms，消不掉 vsync 网格 |
| `desired_maximum_frame_latency` 2 → 1 | 重建后更快拿到新 buffer | 同上 |

**根本原因不在毫秒数**：只要"帧内容 = f(新窗口尺寸)"这条依赖还在，就总有一次跨过合成拍。详见 [`resize-fix-plan.md`](resize-fix-plan.md)（根治方案：把 DXGI 的 `Scaling` 从 `STRETCH` 换成 `NONE`，让旧帧 1:1 贴左上角而不是被缩放）。

### 4.6 为什么 CPU 路径绕过了整件事

`BitBlt` 是 1:1 拷贝，**没有"适配呈现目标尺寸"这个概念**，所以 ①→③ 那条链根本不存在。拖得再快，最坏情况也只是新区域晚几毫秒才画上（露出背景色），而不是文字被拉长。这也是 GDI 时代的终端（ConEmu / mintty / PuTTY）从不需要处理这个问题的原因——不是它们写得好，是它们的呈现路径里没有缩放。

---

## 5. 怎么选

| 场景 | 建议 |
|---|---|
| 日常使用，经常拖窗口 | **`cpu`（默认）** |
| 4K / 高刷屏、满屏滚动、大量图片 | `gpu`（接受拖动时的拉伸） |
| 远程桌面、虚拟机、老核显驱动 | `cpu` |
| 只想确认 GPU 是否可用 | `auto`（先试 GPU，失败退回 CPU） |

切到 `gpu` 时启动日志会主动提示这条已知问题：

```text
[VectorTerm] 渲染后端:GPU(Intel(R) Graphics / 核显 / Dx12) —— 拖动缩放窗口时旧帧会被 DXGI 拉伸, 想避免请改用 render = "cpu"
```

改 `render` 需要重启（热重载只打印"重启后生效"）。

---

## 6. 附：相关实测

- 拖动缩放 `EV_RESIZED → present`：GPU 8.2ms（四轮优化前）→ 6.5ms → 1.0ms；CPU **3.1ms**。
- 正常出帧（200×60 格、1816×976 画布、满屏彩色文本）：GPU 2.28ms，CPU 13.74ms。
- 冷启动首帧 / 峰值内存：GPU 580ms / 485MB，CPU 112ms / 55MB。

完整口径见 [`benchmark/vectorterm-performance-benchmark.md`](../benchmark/vectorterm-performance-benchmark.md)。
