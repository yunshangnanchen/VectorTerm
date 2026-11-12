# 9. 目录结构

工作区原本 19 GB(2026-11-10 清理后 1.5 GB,见 9.5),但**项目本身只有 5.0 MiB**
(204 个文件,`git ls-files` 全量跟踪)。剩下的全是 `target/` 里的 Rust 构建缓存 ——
删掉随时能重建,只是下次编译要重来一遍。下面按"是什么 / 多大 / 能不能删"过一遍。

## 9.1 一屏地图

| 路径 | 内容 | 体积 | 入库 |
|---|---|---|---|
| `src/` | 终端本体,31 个模块(逐个见 9.3) | 1.6 MiB | 是 |
| `settings-gui/` | 配置管理器,独立 Rust + egui crate,与终端本体零耦合 | 314 KiB | 是 |
| `vendor/` | 打过补丁的 `wgpu-hal`,经 `[patch.crates-io]` 接入 | 1.7 MiB | 是 |
| `assets/` | 图标分辨率梯子 + `VectorTerm.ico` + 文档截图 | 439 KiB | 是 |
| `docs/` | 14 篇设计文档与原理剖析(含本文件) | 319 KiB | 是 |
| `tools/` | 图标生成、Nerd Font 表生成、以及 `diagnostics/` 下 21 个诊断脚本 | 118 KiB | 是 |
| `benchmark/` | 性能报告 + 原始 CSV + 一键复现脚本 | 98 KiB | 是 |
| `config/` | `config.lua` 默认配置 + 四套内置主题(`deepseek`/`chatgpt`/`claude`/`gemini`) | 47 KiB | 是 |
| `installer/` | Inno Setup 脚本(可选 PATH、App Paths、卸载清理) | 33 KiB | 是 |
| 根目录文件 | `README.md`、`Cargo.toml`/`Cargo.lock`、`build.cmd`、`config.example.lua`、`VectorTermLogo.png`、两份许可证 | 450 KiB | 是 |
| `examples/`、`patches/` | 一个示例 + 上游 `wezterm-term` 回绕越界补丁 | 5 KiB | 是 |
| `.git/` | 版本库本体(204 个文件的历史) | 5 MiB | — |
| `.wezterm-src/` | `wezterm-term` / `termwiz` 的浅克隆,**编译必需**,`setup.ps1` 会拉 | 23 MiB | 否 |
| `.workbuddy/` | 开发会话的工作记忆(日期日志 + 项目笔记) | 204 KiB | 否 |
| `config/.venv/` | 旧的 tkinter 版配置管理器的 Python 虚拟环境,**已废弃并已删除**(见 9.5) | 曾占 14 MiB | 否 |
| `target/` | 构建产物与一次性探针,详见 9.2;2026-11-10 清理后只剩 1.5 GB(见 9.5) | 最大 19 GB | 否 |

## 9.2 target 里那 19 GB 从哪来

| 子路径 | 体积 | 是什么 |
|---|---|---|
| `target/debug/deps/` | 9.6 GB | 606 个 `.rlib`(合计 3.8 GB)+ `.rmeta` + 链接出来的可执行文件 |
| `target/debug/incremental/` | 6.2 GB | rustc 增量编译缓存:184 个 crate 各自的 `.o` 与 dep-graph |
| `target/debug/examples/` | 424 MB | 64 个探针 example,其中 401 MB 是 `.pdb` 调试符号 |
| `target/debug/build/` | 177 MB | 各依赖 `build.rs` 的产物与输出日志 |
| `target/release/` | 1.5 GB | 发布版:`deps/` 1.4 GB(含 LTO 中间产物)+ `build/` 121 MB;`VectorTerm.exe` 在 `release/` 根 |
| `target/_archive/` | 415 MB | 2026-11-10 归档的 1126 个历史探针文件,附 `manifest.txt` 可查 |
| `target/terms/`、`ft*/`、`fv*/`、`resize/`、`bugprobe/`、`_readme_shot/`、`_cursor_shot/` … | ~250 MB | 一次性实验产物:fastfetch 对照帧、行投影图、ghostty 源码包、缩放与截图探针 |

为什么能到这么大,三条:

- **依赖本身就重**。`wgpu` 拖进 `naga`(着色器编译器)、`winit`、`image`、`mlua`(连 Lua 5.4 源码一起编)。
  debug 版不做优化,却保留全部调试信息和泛型单态化结果 —— 一个 `libnaga` 的 `.rlib` 就 **104 MB**。
- **同一个 crate 会留好几份**。feature 组合不同、profile 不同、打补丁前后,都会各生成一份指纹不同的 `.rlib`。
  实测 `windows_sys` 有 **25 份**、`syn` 12 份、`image` 7 份。我们有 2 个 crate(`vectorterm` + `vectorterm-settings`)、
  64 个 example、还有 `#[cfg(test)]` 的测试二进制,每个都得独立链接一次。
- **增量缓存只增不减**。改一次代码就往 `incremental/` 加一批 `.o`,cargo 不会自己回收,
  只有 `cargo clean` 或手动删目录才会掉下去。

## 9.3 src 模块职责

| 模块 | 职责 |
|---|---|
| `main.rs` | 入口:winit 事件循环、`GuiState`、多标签槽位、PTY 线程编排、`SharedWriter` |
| `config.rs` | `AppConfig` + mlua 配置加载与热重载,含 wezterm 字段映射 |
| `wezterm_compat.rs` | wezterm 配置 API 兼容层(`config_builder`/`font`/`action`/`nerdfonts` …) |
| `script.rs` | Lua 脚本层:键位映射 + `VectorTerm.*` 运行时动作 + 状态栏脚本 |
| `term.rs` | `TerminalConfiguration` 实现、调色板构建、标题告警、OSC 52 剪贴板桥接 |
| `pty.rs` | portable-pty(ConPTY)会话:参数 / 环境变量 / 会话记录 |
| `render.rs` | 布局 + CPU 光栅化 + 终端快照(含亚行滚动位移)+ 字形角色排版 |
| `frame.rs` | 显示列表:CPU / GPU 共用的帧描述(`Rect` / `Glyph` / `Trail`)+ 内容区竖直裁剪、面板横向裁剪(`clip_x`) |
| `gpu.rs` | wgpu / DX12 后端:字形图集 + 实例化四边形 + 尾迹管线 |
| `cursor.rs` | 光标尾迹动画状态机(kitty `cursor_trail.c` 的 Rust 版) |
| `scroll.rs` / `scrollbar.rs` | 回看滚动的缓入缓出动画、输入优先回底;滚动条几何与命中测试 |
| `selection.rs` | 鼠标选区:绝对行号锚定、多击选词选行、文本抽取 |
| `search.rs` | 历史搜索:scrollback 拍平成可检索文本 + 高亮 |
| `tabbar.rs` | 标签栏纯几何布局:宽度分配 / 命中测试 / 标题按格宽裁剪 / 窗口按钮 |
| `status.rs` | 状态栏文本:把 `wezterm.format` 产出的 SGR 串解析成可绘制片段 |
| `font.rs` | 系统字体索引(sfnt / name / OS/2 / head / post)+ 家族样式匹配 + Nerd Font 发现 |
| `nerdfonts.rs` | Nerd Fonts 字形表(`tools/gen_nerdfonts.py` 生成,勿手改) |
| `input.rs` | 键盘事件 → `KeyCode` / 文本(编码交给终端状态机) |
| `images.rs` | 图片解码缓存(kitty / iTerm2 / sixel 共用) |
| `background.rs` | 自定义背景:图片解码 → 降采样 → HSB 调整 → 模糊 / 渐变 / 半透明 |
| `files.rs` | 文件浏览器:列表模型 + 面板绘制 |
| `commands.rs` | 命令面板(`/` 或 `Ctrl+Shift+P`) |
| `procs.rs` | 进程树查询:判断某个标签里除了 shell 还有没有别的进程 |
| `mux.rs` / `mux_server.rs` | mux 协议与客户端 / 守护进程,wezterm 式会话保持 |
| `debug_term.rs` | 软件自身的诊断输出,统一送进独立的调试控制台窗口 |
| `win_console.rs` | 发行版把 stdout / stderr 接到"能用"的地方(无控制台时不吞输出) |
| `winhook.rs` | 窗口过程钩子:在 `WM_SIZING` 阶段提前把交换链调好 |
| `win_il.rs` | 进程完整性级别与父进程探测(仅启动时用一次) |
| `bench.rs` | `#[cfg(test)]` 性能基准套件(6 个 ignored)+ 回绕越界回归测试 |

## 9.4 想瘦身怎么删

按"回收得多 / 代价小"排序,都可以随时重建:

| 操作 | 回收 | 代价 |
|---|---|---|
| `rm -rf target/debug/incremental` | 6.2 GB | 下次改一行代码要重编更多,但依赖仍复用 |
| `rm -rf target/debug` | 17 GB | `cargo test` / `cargo run` 要重新编译;release 不受影响 |
| `rm -rf target/debug/examples` | 424 MB | 探针 example 重编才有 |
| `rm -rf target/ft* target/fv* target/terms target/resize target/bugprobe target/_readme_shot target/_cursor_shot target/anim_frames target/_reschk target/icon target/shotparts` | ~250 MB | 一次性实验产物,`tools/diagnostics/` 里的脚本能重跑 |
| `rm -rf target/_archive` | 415 MB | 2026-11-10 的归档;删前可先看里面的 `manifest.txt` |
| `rm -rf config/.venv` | 14 MB | 无,旧 tkinter 配置管理器已废弃 |
| `cargo clean` | 19 GB | 全部重编(含 release) |

**别删**:`target/release/VectorTerm.exe`(跑起来的就是它)、`target/release/config/`(发行时同步过去的主题)、
`.wezterm-src/`(编译必需)、`vendor/`(`wgpu-hal` 补丁)、`config/*.lua`。

日常开发只跑 `build.cmd`(release)的话,`target/debug` 那 17 GB 纯属 `cargo test` 留下的,
一次 `rm -rf target/debug` 就能把工作区从 19 GB 压回 2 GB。

## 9.5 2026-11-10 实际清理记录

上表按清单执行过一次,工作区 **19 GB → 1.5 GB**:

| 删掉的 | 回收 |
|---|---|
| `target/debug/`(deps / incremental / examples / build) | 17 GB |
| `target/terms`、`ft`~`ft6`、`fv`~`fv3`、`resize`、`bugprobe`、`_readme_shot`、`_cursor_shot`、`anim_frames`、`_reschk`、`icon`、`shotparts`、`flycheck0`、`cwd_sync_probe`、`files-*`、`gui_probe*`、`scrollbar_search_probe` | 约 250 MB |
| `target/_archive/2026-11-10/`(1126 个历史文件,manifest 已注明可删) | 415 MB |
| `config/.venv/`(废弃的 tkinter 版虚拟环境) | 14 MB |
| `target/` 根下散落的 `status_compare.png`、`test-font.lua`、`test-out.txt`、`theme-preview.png` | 忽略不计 |

剩下 `target/release/`(1.5 GB,含 `VectorTerm.exe`)与 `target/installer/`(7 MB 安装包),
清理后重新导出过一帧确认 release 版仍能正常启动。想再回收 1.4 GB 就得动
`target/release/deps/`(LTO 中间产物),但 `cargo clean` 会连 `VectorTerm.exe` 一起删,
没有备份前别动。

注意:这只是清掉存量,不是防增长。跑一次 `cargo test` 或 `cargo run`,`debug/`
又会从零长回来(依赖全量重编),体积照样回到 17 GB 量级 —— 想长期控制靠定期清,
或者用 `CARGO_TARGET_DIR` 把不同项目的缓存分开放。
