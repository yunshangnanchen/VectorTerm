# 自定义背景

对应配置项：`window_background_*` / `win32_system_backdrop`。名字尽量和 wezterm 对齐，wezterm 的配置可以整段搬过来。

## 1. 能配什么

| 配置项 | 取值 | 说明 |
|---|---|---|
| `window_background_color` | `"#101418"` | 纯色，覆盖 `colors.background` |
| `window_background_image` | 路径 | 壁纸。相对路径按配置文件所在目录解析 |
| `window_background_image_hsb` | `{ brightness, hue, saturation }` | 三个乘数，`1.0` = 原样 |
| `window_background_blur` | 0~200 | 模糊半径（图片像素），`8~24` 是磨砂感 |
| `window_background_fill` | `cover` / `contain` / `stretch` / `tile` / `center` | 填充方式，默认 `cover` |
| `window_background_opacity` | 0~1 | 背景层不透明度，默认 1，见第 5 节 |
| `window_background_gradient` | `{ colors = {a,b}, orientation }` | 两色渐变，画在图片下面 |
| `win32_system_backdrop` | `none` / `mica` / `acrylic` / `tabbed` | 系统背板材质，`acrylic` 即毛玻璃，见第 6 节 |

支持格式：png / jpg / webp / bmp / gif（动图取首帧）。超过 2560px 的长边会先等比降采样再模糊——背景最终是被缩放到窗口的，再大的像素也用不上。

## 2. 常用配方

**纯色**
```lua
window_background_color = "#0f1419"
```

**壁纸 + 压暗（最常用）**
```lua
window_background_image = "bg/wallpaper.jpg"
window_background_image_hsb = { brightness = 0.45 }
window_background_fill = "cover"
```

**磨砂壁纸**
```lua
window_background_image = "bg/wallpaper.jpg"
window_background_blur = 18
window_background_image_hsb = { brightness = 0.5, saturation = 0.8 }
```

**半透明（透出桌面）**
```lua
window_background_color = "#101820"
window_background_opacity = 0.82   -- 整窗一起变淡,CPU/GPU 都支持
```

**渐变**
```lua
window_background_gradient = {
  colors = { "#0b1020", "#1b2a4a" },
  orientation = "Vertical",   -- 或 "Horizontal"
}
```
注意 `colors` / `orientation` 这两个键名是 wezterm 的写法。Lua 里 `end` 是保留字，所以不能写成 `{ start = ..., end = ... }`。

## 3. 填充方式

```text
cover    等比放大到铺满,超出裁掉       contain  等比缩到整张可见,四周留底色
┌─────────────────┐                    ┌─────────────────┐
│▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓│                    │                 │
│▓▓▓▓ 图 片 ▓▓▓▓▓│                    │   ┌─────────┐   │
│▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓│                    │   │ 图 片   │   │
└─────────────────┘                    │   └─────────┘   │
                                       └─────────────────┘
stretch  拉伸铺满(不保比例)          tile  原尺寸平铺        center  原尺寸居中
```

`tile` 有个保护：块数超过 512 时自动退化成 `cover`，否则 1px 的图铺满 4K 会生成上万个四边形。

## 4. 两条后端怎么画

背景层**只在启动时预处理一次**（解码 → 降采样 → HSB → 模糊），产出一张位图，两条后端共用：

- **CPU**：`paint_cpu` 先铺底色，再按填充方式把位图 `blit_image` 进去，超出画布的部分由 `blit_image` 内部裁掉（缩放比例仍按完整矩形算，所以 cover 不会被压缩）。
- **GPU**：把位图上传成一张纹理，算出的矩形先收敛到画布内，再反算 uv 子区间——`cover` 超出画布的那部分如果不反算 uv，就会被压进可见区域，变成拉伸。最后拼成一个 quad 放在所有实例之前，且**不受内容区 scissor 裁剪**（否则背景会被标签栏的安全区切掉一条）。

渐变不单独实现，而是被**烘焙成 1×256 / 256×1 的小图**走同一条路径。这样两条后端的渐变必然一致，也省了两套插值代码。

有一条容易踩的坑：每个单元格本来都会铺一块「默认底色」矩形。只要挂了背景层，这块矩形必须让位，否则整张壁纸会被几千个不透明小方块盖死。判断条件是「单元格底色 == 调色板底色 且 没配背景层也没配纯色」。

## 5. 半透明是怎么做的

两条后端不一样：

| 后端 | 机制 | 透明度语义 |
|---|---|---|
| `render = "gpu"` | `CreateSwapChainForComposition` + DirectComposition visual + `WS_EX_NOREDIRECTIONBITMAP` | **逐像素 alpha**：只有背景层变淡，字形和光标保持实心 |
| `render = "cpu"` | `WS_EX_LAYERED` + `SetLayeredWindowAttributes(LWA_ALPHA)` | **整窗 alpha**：文字跟着一起变淡 |

### 5.1 GPU:逐像素 alpha

`CreateSwapChainForHwnd` 不接受 `DXGI_ALPHA_MODE_PREMULTIPLIED`（返回 `0x887A0001`），
所以 GPU 后端的窗口表面改成"合成交换链"：

1. 窗口创建时带 `WS_EX_NOREDIRECTIONBITMAP`（**必须创建时给**，事后 `SetWindowLongW` 无效，
   会给出一块纯灰画面）；
2. 用 `IDCompositionDevice` → `IDCompositionTarget` → `IDCompositionVisual`，
   `SetRoot` 到窗口，`CreateSwapChainForComposition` 出的交换链挂在这个 visual 上；
3. wgpu 侧走 `SurfaceTargetUnsafe::CompositionVisual(ptr)`，
   此时 `surface_capabilities` 才会如实报出 `PreMultiplied`；
4. 每帧 `Present` 之后补一次 `IDCompositionDevice::Commit`。

绘制时对 alpha 的处理：

- 清屏色和背景层（壁纸 / 纯色 / 渐变）的 alpha 乘 `opacity`；
- 单元格底色矩形和标签栏底色也乘同一个系数（漏掉任何一个，整块窗口就又不透了——
  这是实现时踩到的第一个坑）；
- 字形、光标、选中框 alpha 恒为 1，所以"背景半透明、文字实心"。

`WS_EX_NOREDIRECTIONBITMAP` 这个位有个副作用：**没有重定向表面，GDI 就画不上去**。
所以它只在"配置要玻璃 **且** 这次真的会走 GPU"时才给（窗口创建前先探一次硬件适配器）。
否则 `render = "cpu"` 会得到一个空窗口——只剩背后的模糊材质，什么字也没有。

`render = "auto"` 会优先走 GPU。拿不到逐像素 alpha 时**不再用整窗 alpha 硬凑**：
玻璃要的是"背景透、文字实"，整窗 alpha 只能一起变淡，材质还透不出来，是两头不讨好。
现在保持不透明，并在日志里说明：

```text
[VectorTerm] 玻璃需要逐像素 alpha,当前后端给不了:已保持不透明(...)。
             把 render 设为 "auto"/"gpu" 后**重启**才有毛玻璃
```

也就是说 **毛玻璃只有 GPU 后端（`auto` / `gpu`）才有**，而且 `render` 是启动时定的，
改完必须重启。想要"整窗一起变淡"那种老式半透明（不配 `win32_system_backdrop`），
CPU 后端仍然照做。

## 6. 毛玻璃

`win32_system_backdrop` 现在**真的能看到材质**了，前提是逐像素 alpha 已经通（见 5.1）：
材质由系统画在窗口背后，客户区必须对 DWM 透明才能露出来。

| 取值 | 实现 | 失焦后 |
|---|---|---|
| `"none"` | 无材质 | —— |
| `"acrylic"` | `SetWindowCompositionAttribute` + `ACCENT_ENABLE_BLURBEHIND`（高斯模糊，不带着色，着色交给自己的逐像素 alpha） | **磨砂保持** |
| `"mica"` / `"tabbed"` | `DWMWA_SYSTEMBACKDROP_TYPE`（壁纸着色，本来就不做磨砂） | 系统按材质规则变化 |

### 6.1 为什么 acrylic 不直接用 DWM 的

因为 wezterm 那个"一点别处磨砂就没了"的毛病，根源就在 DWM 材质本身：
`DWMSBT_TRANSIENTWINDOW`（Acrylic）和 `DWMSBT_MAINWINDOW`（Mica）在窗口**失去激活状态**
时会被 DWM 退化成一块纯色。

实测（`tools/diagnostics/glass_metric.py`，背景放高频棋盘格，测窗口内一块空白区的
"梯度能量"——棋盘清晰时约 660，完全糊掉时约 0）：

| 材质 | 有焦点 | 失焦 |
|---|---|---|
| DWM Acrylic（`DWMSBT_TRANSIENTWINDOW`） | 0.1，磨砂 | 0.0，但画面是**纯灰 RGB 84**，磨砂变实心 |
| DWM Mica（`DWMSBT_MAINWINDOW`） | 0.0，纯灰 32 | 0.0，纯灰 32 |
| **accent 模糊（现在用的）** | **4.5** | **4.5** |

accent 模糊作用在窗口自身的合成上，与激活状态无关，所以磨砂一直在。
万一系统不认这个接口，会退回 DWM Acrylic 并打日志说明"失焦会失去磨砂"。

复现脚本：`tools/diagnostics/glass_metric.py <tag> <none|acrylic|mica> <opacity> [位置] [win|desk]`。

> 测的时候**别去点背景窗口来取消焦点**：它通常也是 TOPMOST 且比前台窗口大，
> 一激活就抬到最上层把前台窗口整个盖住，量到的会变成背景自己的棋盘格（会误判成
> "磨砂掉了"）。取消焦点请点桌面空白处（`desk`）。

## 7. 代价

- **内存**：一张 2560×1440 的 RGBA 背景约 14MB（CPU 与 GPU 各持一份）。
- **启动**：解码 + 模糊一次性完成，4K 原图缩放后约 20~30ms；之后每帧不参与。
- **每帧**：CPU 端多一次全屏位图混合（约 +1.2ms @1080p）；GPU 端只是一个 quad，可忽略。
- **平铺**：每块一个四边形，上限 512 块。

## 8. 已知限制

- 单元格里显式设了背景色的内容（选中块、反显、`ls` 的目录底色）也乘 `opacity`，
  叠在玻璃上会略深，这是有意的。
- **毛玻璃只有 GPU 后端才有**。`render = "cpu"` 拿不到逐像素 alpha，配了
  `win32_system_backdrop` 也会保持不透明（不会退化成"文字发灰"的整窗 alpha）。
  改 `render` 需要重启。
- CPU 后端下不要再去调 `SetLayeredWindowAttributes` 的第二个参数（色键），会和统一 alpha 打架。
- 合成交换链（`WS_EX_NOREDIRECTIONBITMAP`）的窗口没有系统画的非客户区，圆角/阴影完全依赖
  DWM 自身策略；窗口拖动缩放时旧帧会被 DXGI 拉伸一段（GPU 后端固有行为）。

