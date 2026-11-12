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
| `window_background_opacity` | 0~1 | 整窗不透明度，默认 1，见第 5 节 |
| `window_background_gradient` | `{ colors = {a,b}, orientation }` | 两色渐变，画在图片下面 |
| `win32_system_backdrop` | `none` / `mica` / `acrylic` / `tabbed` | **只解析不生效**，见第 6 节 |

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

用的是**层叠窗口 + 统一 alpha**（`WS_EX_LAYERED` + `SetLayeredWindowAttributes(LWA_ALPHA)`），DWM 在合成阶段把整块窗口按这个 alpha 叠到桌面上：

| 能力 | CPU 后端 | GPU 后端 |
|---|---|---|
| 纯色 / 壁纸 / 渐变 / 模糊 / 填充 | ✅ | ✅ |
| 半透明（透出背后的窗口） | ✅ | ✅ |

两个后端行为一致，因为变淡发生在合成阶段，和下面画的是 GDI 位图还是 DXGI 画面无关。

**代价：文字也会一起变淡。** 这是统一 alpha 的语义，`opacity = 0.8` 表示「整块窗口 80% 不透明」，不是「只有背景 80% 不透明」。想要「背景半透明、文字全不透明」需要逐像素 alpha，见第 6 节。

为什么不用逐像素 alpha（试过的路）：

1. **GPU 交换链**：`CreateSwapChainForHwnd` 不接受 `DXGI_ALPHA_MODE_PREMULTIPLIED`，DXGI 直接返回 `0x887A0001`（`DXGI_ERROR_INVALID_CALL`）。四种组合都试过——`Scaling` 的 `STRETCH`/`NONE` × `SwapEffect` 的 `FLIP_DISCARD`/`FLIP_SEQUENTIAL`——全部失败；把窗口的透明属性去掉也一样失败。所以**不是驱动限制，也不是参数搭配问题**，是 hwnd 交换链本身不支持。
2. **CPU 位图**：softbuffer 走 GDI `BitBlt` 贴到窗口 DC，这条路径根本没有 alpha 通道。

真要逐像素，唯一的路是把交换链换成 `CreateSwapChainForComposition`，挂到 DirectComposition 的 visual 上，同时窗口要加 `WS_EX_NOREDIRECTIONBITMAP`。这条路会连带影响窗口阴影、圆角、以及现在的 `WM_SIZING` 预配置逻辑，所以没有动。

## 6. 毛玻璃（mica / acrylic）当前不生效

`win32_system_backdrop` 会被解析，也会真的去调 `DwmExtendFrameIntoClientArea` + `DwmSetWindowAttribute(DWMWA_SYSTEMBACKDROP_TYPE)`，但**看不到材质**：材质是 DWM 画在窗口背后、需要客户区对 DWM 逐像素透明才能露出来，而上面第 5 节说了当前给不出逐像素透明。启动日志会明确提示：

```text
[VectorTerm] win32_system_backdrop = "acrylic" 只设了 DWM 属性,材质透不出来:窗口交换链给不出逐像素透明
```

之所以保留解析：wezterm 的配置能整段搬过来不至于报错。想要半透明效果请用 `window_background_opacity`。

## 7. 代价

- **内存**：一张 2560×1440 的 RGBA 背景约 14MB（CPU 与 GPU 各持一份）。
- **启动**：解码 + 模糊一次性完成，4K 原图缩放后约 20~30ms；之后每帧不参与。
- **每帧**：CPU 端多一次全屏位图混合（约 +1.2ms @1080p）；GPU 端只是一个 quad，可忽略。
- **平铺**：每块一个四边形，上限 512 块。

## 8. 已知限制

- 单元格里显式设了背景色的内容（选中块、反显、`ls` 的目录底色）保持不透明，不受 `opacity` 以外的背景层影响，否则会糊成一片。
- 层叠窗口下不要再去调 `SetLayeredWindowAttributes` 的第二个参数（色键），否则会和统一 alpha 打架。
- 毛玻璃材质需要 DirectComposition 合成表面，见第 5 / 6 节。
