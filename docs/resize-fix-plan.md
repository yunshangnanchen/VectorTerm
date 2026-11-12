# 拖动缩放抖动的实施方案（效果优先，不动体验）

> 本文只给方案。截至撰写时 `src/` 一行未改。

## 0. 结论

把 DXGI 交换链的 `Scaling` 从 `STRETCH` 改成 `NONE`。

这是唯一一个**从定义上取消"缩放"**的改法：窗口尺寸与后缓冲尺寸不一致时，DXGI 不再把旧画面拉满，而是 1:1 贴左上角、超出裁掉、不足填背景色。抖动不再被"优化"，而是不再可能发生。

配套一条（L2）：拖动期间让后缓冲**只增不减**并留富余，把"填背景色"的那一帧也消掉。

改动集中在两处：`wgpu-hal` 的一行（本地 patch）+ `gpu.rs` 的尺寸策略。**标签栏、动画、insets、reflow 策略、PTY 同步、`winhook.rs` 全部不动。**

---

## 1. 为什么这条能终结问题，而前四轮不能

前四轮修的都是同一个量：

```text
T = t(新尺寸的像素上屏) − t(窗口矩形变大)
```

T 越小越好，但 T 有下限：`ResizeBuffers` 4~5ms + vsync 量化 16.7ms 网格。四轮把 T 从 8.2ms 压到 1.0ms，**但 T 永远不可能为 0**，所以"总有一次跨过合成拍"这件事在概率上无法消除。这就是"改了四轮还抖"的结构性原因。

`Scaling = NONE` 换了一个量。它不关心 T：

| | 前四轮 | 本方案 |
|---|---|---|
| 优化目标 | 让新帧更早到（T ↓） | 让旧帧**不被变形**（缩放语义删除） |
| T 迟到时 | 整屏内容被拉伸 1.5%，再弹回 | 内容纹丝不动，边缘露出一条背景色带 |
| 依赖 vsync / 驱动 / DWM 行为 | 是，全部依赖 | 否，语义由 DXGI 保证 |
| 失败模式 | 不可预测（取决于机器负载） | 可预测（只影响边缘一条带） |

一句话：**前四轮在跟时间赛跑，NONE 让这场比赛变得无关紧要。**

---

## 2. 证据链

方案不是推测，四条都可以复现：

1. **拉伸源在 DXGI，不在 DWM。**
   `wgpu-hal-24.0.4/src/dx12/mod.rs:1135`
   ```rust
   Scaling: Dxgi::DXGI_SCALING_STRETCH,
   SwapEffect: Dxgi::DXGI_SWAP_EFFECT_FLIP_DISCARD,
   ```
   `Scaling` 只在**创建**时生效，`ResizeBuffers`（第二次 configure 走的分支）不会改它——所以改这一处即全局生效。

2. **MSDN 明确 NONE 支持我们用的 swap effect。**
   当前文档原文：*"The DXGI_SCALING_NONE value is supported only for flip presentation model swap chains that you create with the DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL **or DXGI_SWAP_EFFECT_FLIP_DISCARD** value."*
   旧版文档只写了 `FLIP_SEQUENTIAL`，容易误判为不可用；现版已明确包含 `FLIP_DISCARD`。

3. **NONE 的语义正好是我们要的。**
   原文：不缩放、后缓冲与呈现目标**左上角对齐**、**后缓冲之外的所有区域用 `IDXGISwapChain1::SetBackgroundColor` 填**。

4. **GDI 系天然不抖，是因为它们没有交换链。**
   `softbuffer-0.4.8/src/backends/win32.rs:189` 用的是 `Gdi::BitBlt` 直接贴到窗口 DC，没有 DXGI 缩放环节。这解释了为什么 `render = "cpu"` 时序更稳、为什么 ConEmu/mintty/PuTTY 从不需要处理这个问题——**不是它们写得好，是它们没有这个语义**。

附带证据：wgpu 上游 issue **#10196**（2026-08-29）里，报告者的 Intel Vulkan 驱动升级后画面从"拉伸"变成"letterbox 黑边"。**letterbox 就是 NONE 的另一种说法**，说明这套语义在真实驱动上是成熟、可用的，不是理论值。

---

## 3. 实施

### L1：一行 patch（约半天，必做）

骨架已经搭好了：`vendor/wgpu-hal`（24.0.4）已入库，`[patch.crates-io]` 已指向它，
建交换链的 `Scaling` / `SwapEffect` 可以**用环境变量覆盖**，默认值与上游完全一致
（改动细节见 `vendor/wgpu-hal/PATCH.md`）。所以 L1 现在可以零改动先验证：

```bash
VECTORTERM_DX_SCALING=none ./target/release/VectorTerm.exe
# 对照组:VECTORTERM_DX_SCALING=none VECTORTERM_DX_EFFECT=sequential
```

确认有效之后，再把默认值改成 `Dxgi::DXGI_SCALING_NONE`：

```rust
- Scaling: Dxgi::DXGI_SCALING_STRETCH,
+ Scaling: Dxgi::DXGI_SCALING_NONE,
```

**加一个失败回退**（约 10 行，在同一函数里）：`CreateSwapChainForHwnd` 若对 `NONE` 返回 `E_INVALIDARG`（老驱动 / 老系统），把 `Scaling` 换回 `STRETCH` 再建一次并打日志。这样这个改动**不可能让程序起不来**。（这一步还没做：当前默认仍是 `STRETCH`。）

### L1.5：背景色对齐（约 20 行，建议）

`NONE` 下未覆盖区域默认填**黑色**。深色主题无感，浅色主题会突兀。在创建成功后调用一次：

```rust
swap_chain1.SetBackgroundColor(&Dxgi::Common::DXGI_RGBA { r, g, b, a: 1.0 })?;
```

颜色来源：`gpu.rs` 从 `WGPU_SWAPCHAIN_BG="r,g,b"` 环境变量读取（`main.rs` 用终端背景色注入），未设置就用黑色。**不改任何渲染逻辑**，只在创建时传一个值。

### L2：拖动期间后缓冲"只增不减 + 富余"（约 1 天，效果最好的必要条件）

`NONE` 解决了"变形"，但还剩"边缘一条背景色带"。带宽 = 窗口比后缓冲大的那部分像素。整格吸附（`use_resize_increments = true`）下每步约一格（8~10px），一帧 8px 的色带在快速拖动时可能被看到。

消除办法：**让后缓冲永远不小于窗口。**

1. `gpu.rs` 把两个尺寸分开：
   - `surface_extent` = 实际 configure 给 DXGI 的尺寸（带富余）
   - `render_extent` = 窗口客户区尺寸（用于 viewport / scissor）
2. 尺寸策略改为单调增长：
   ```text
   需要 surface_extent >= window_size
   若 need > surface_extent：surface_extent = ceil(need * 1.25) 向上对齐 64
   若 need < surface_extent * 0.5：保持不动（拖动中绝不收缩）
   ```
3. `draw()` 里 `viewport` / `scissor` 用 `render_extent`，render pass 的 `load_op` 仍为 `Clear`（清成终端背景色）——富余区域天然是背景色，被裁掉时观众看不到。
4. 松手（`WM_EXITSIZEMOVE`，`winhook.rs` 已有钩子，只需补两条消息）时收紧到精确尺寸。这一步发生在拖动结束后，没有观感影响。

效果：一次拖动里 `ResizeBuffers` 从"每步一次"降到 **0~1 次**，色带宽度恒为 0。此时 `T` 变成多少都无所谓了。

> 这也是为什么 L2 必须配 L1：`STRETCH` 下后缓冲比窗口大 = 画面被缩小，反而更糟。两者是配套的。

---

## 4. 明确不动的部分

| 部分 | 状态 |
|---|---|
| 标签栏布局 / `top_padding` / `edge_padding` / inset 动画 | 不动 |
| `live_resize_mode = realtime`、行列数实时跟随 | 不动（体验不变） |
| `compute_grid` / `commit_grid` / 整格吸附 | 不动 |
| PTY resize 同步、PSReadLine 重绘 | 不动 |
| `winhook.rs` 的 `WM_SIZING` 预配置 | 保留，但**降级为次要优化**（只影响色带宽度，不再是正确性依赖） |
| `winit` / 渲染后端选择 / CPU 回退 | 不动 |

---

## 5. 验收

判据只有一个，而且它**不依赖时序**：

```text
拖动全程，标尺文字跨度（SPAN）必须恒等于 BASE_SPAN，偏差 0
```

```bash
# L1 生效后
VECTORTERM_STEP_PX=8 python tools/diagnostics/verify_stretch_fast.py target/none_x config/config.lua x
VECTORTERM_STEP_PX=8 python tools/diagnostics/verify_stretch_fast.py target/none_y config/config.lua y
```

**关键的反向验证**（这是判断"是不是架构性修复"的分水岭）：

```bash
VECTORTERM_NO_SIZING_HOOK=1 VECTORTERM_STEP_PX=8 python tools/diagnostics/verify_stretch_fast.py target/none_nohook config/config.lua x
```

前四轮的成果全部依赖 `WM_SIZING` 预配置——关掉它就会退化。**如果关掉钩子后仍然 0 拉伸帧，说明变形已经从机制上不可能发生，这次是真修好了。** 这条对照是过去四轮一直缺的"通过/不通过判定"。

再看 `EV_RESIZED → PRESENT` 的毫秒数就没意义了：L1 之后它可以是 5ms、也可以是 10ms，跨度都不变。

---

## 6. 风险与代价

| 风险 | 概率 | 处理 |
|---|---|---|
| 驱动不接受 `FLIP_DISCARD + NONE` | 低（MSDN 明确支持） | 已内置回退到 `STRETCH`，功能不受影响 |
| 浅色主题下边缘黑带 | 中 | L1.5 传背景色；L2 彻底消除 |
| 快速拖动时一帧色带 | 中 | L2 消除 |
| 维护一个 fork | 确定 | 只 1 行 + `PATCH.md`；升级 wgpu 时重新应用（可写进 `setup.ps1`） |
| 上游未来改了默认值 | 低 | `PATCH.md` 记录基线版本 |
| 多显示器 DPI 切换时 `surface_extent` 失配 | 低 | `render_extent` 与 `surface_extent` 分离后天然容忍 |

**长期正解**：给 wgpu 提 PR，在 `SurfaceConfiguration` 加 `scaling: SurfaceScaling { Stretch, None }`，D3D12 映射到 `DXGI_SCALING_*`、Vulkan 映射到 `VK_COMPOSITION_...` / `present_mode`。issue #10196 正在等这个，提上去大概率被接受。届时删掉 fork 即可。

---

## 7. 为什么不是其它方案

| 方案 | 不选的原因 |
|---|---|
| **A. 拖动中不重排**（kitty `static`） | 行列数不再实时跟随 → **改了体验**，用户明确要求保留 |
| **子窗口分层** | 若内容子窗口跟着 resize，DXGI 照旧缩放它（问题原样搬家）；若冻结尺寸就等于 A。且要重做焦点 / IME / 双交换链 |
| **换掉 winit** | 只换到消息时序（追平 wezterm），**不改变"帧内容 = f(尺寸)"**，抖动照旧 |
| **继续压缩 T** | 已到 1ms，物理下限是 vsync 网格；且概率上无法消除 |
| **提高刷新率 / 换 `Immediate`** | 实测无收益（6.2 vs 6.5ms），且不改变缩放语义 |

---

## 8. 建议执行顺序

1. **今天，30 秒**：`render = "cpu"` 拖一次右边框。GDI 没有交换链（`BitBlt`），若此时完全不抖 → 说明拉伸 100% 来自 DXGI 呈现层，本方案的前提成立。这是一次零成本的假设检验。
2. **半天**：L1 + 失败回退，跑第 5 节三条命令验收。
3. **一天（可选）**：L1.5 + L2，把色带归零。
4. **之后**：提交上游 PR，等待上游合并后移除 fork。
