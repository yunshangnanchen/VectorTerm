# 4. 依赖清单

| 依赖 | 版本 | 来源 | 作用 | 为什么选它 |
|---|---|---|---|---|
| `wezterm-term` | tag `20240203-110809-5046fc22` | git(浅克隆到 `.wezterm-src/`,以 path 依赖引入) | 终端状态机:ANSI 解析、grid、滚动回看、调色板、备用屏、括号粘贴、OSC | wezterm 的核心,成熟度最高;**未发布到 crates.io**,故用 tag 锁定的浅克隆 |
| `termwiz` | 同 tag(随 wezterm 源码) | path 依赖 | 单元格/线/光标形状类型(与 wezterm-term 同一实例) | 与核心共用同一份代码,避免版本漂移 |
| `portable-pty` | 0.9 | crates.io | PTY 抽象:ConPTY 开/读/写/缩放/杀进程,另有 shell 参数、环境变量透传、会话记录 | 同为 wezterm 系,API 干净 |
| `mlua` | 0.12(feature:`lua54`,`vendored`) | crates.io | 配置加载 + 键位映射 + 运行时动作脚本 | 纯 Rust 构建链内完成 Lua 编译(vendored),无运行时 DLL 依赖 |
| `winit` | 0.30 | crates.io | 窗口 + 输入事件循环 | Rust 生态事实标准 |
| `softbuffer` | 0.4 | crates.io | CPU 像素缓冲 → 窗口呈现(Windows 走 GDI `BitBlt`,**没有交换链**) | `render = "cpu"` 后端:保真、零 GPU 依赖,远程桌面/老核显上更稳;因为没有 DXGI 缩放环节,拖动缩放窗口时不抖 |
| `wgpu` | 24(feature 只开 `dx12`,`wgsl`) | crates.io | `render = "gpu"` 后端:纹理图集 + 实例化绘制 | 只用 DX12 后端,不拉 Vulkan/Metal/GL 编译,与"只兼容 Windows"的定位一致 |
| `bytemuck` | 1(derive) | crates.io | 顶点/实例结构体的零拷贝转换 | POD 结构体转 `&[u8]`,避免手写 unsafe 转换 |
| `pollster` | 0.4 | crates.io | 阻塞等待 wgpu 异步初始化 | 渲染器构造在启动路径上,写 `.block_on()` 比引入 async 运行时轻 |
| `ab_glyph` | 0.2 | crates.io | 字体文件加载、字形栅格化(支持 TTC)、字体度量 | 纯 Rust、无系统字体服务依赖;`font.rs` 只做家族名索引,渲染仍由 ab_glyph 完成 |
| `arboard` | 3 | crates.io | 剪贴板读取(粘贴) | 轻量、无系统库依赖 |
| `anyhow` / `parking_lot` | 1 / 0.12 | crates.io | 错误处理 / 跨线程锁 | 标准配套 |

> **关于 `wezterm-term` 的引入方式**:该 crate 从未发布到 crates.io。本项目用 `setup.ps1` / `setup.sh` 从 GitHub 按 tag 浅克隆(sparse-checkout 只取 `term` 及其仓库内路径依赖:`termwiz`、`bidi`、`wezterm-dynamic`、`color-types`、`wezterm-blob-leases`、`wezterm-input-types`、`filedescriptor`、`vtparse`),然后以相对路径依赖(`path = ".wezterm-src/term"`)接入,并删除上游工作区清单避免冲突。tag 锁定保证可复现构建。
