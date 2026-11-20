# 侧边栏开合后首次输入错位：ConPTY 行号记账 vs PSReadLine 绝对定位

> 基线提交 `2b1e389`。**本文只做定位与修复建议，不含任何源码改动。**
> 所有结论来自本机实机对照（`tools/diagnostics/sidebar_input_probe.py`），截图与 PTY
> 原始字节保存在 `target/_sidebar_bug/`。环境：Windows 11、`render = "cpu"`、
> 1200×800、`font.size = 16`、默认 `pwsh.exe` + PSReadLine。

---

## 0. 结论先行

三句话：

1. **不是渲染错位，也不是侧边栏画错了位置。** 开/关侧边栏会对同一个 PTY 连续做两次
   `ResizePseudoConsole`（本例 98 列 → 68 列 → 98 列），ConPTY 缓冲区里的长行被折行后
   **不能合并回去**；而 `wezterm-term` 的 `term.resize()` 会 reflow 合并。两边对"提示符
   在缓冲区第几行"从此不一致。
2. **PSReadLine 缓存了提示符的绝对坐标，首次按键按缓存重绘。** 实测正常时它发
   `ESC[28;20H`，开合后发 `ESC[25;20H` —— 列号 20（提示符长度）不变，行号错了。
   于是输入的字母被画到错误的行、左边空出提示符那一整段，就是报的"空出一大截"。
3. **跟 pwsh 美化有关，但不是美化的错。** 责任方是 PSReadLine 的行编辑器缓存；
   `oh-my-posh` 每次按键都重绘整条提示符，反而把过期坐标覆盖掉，所以**带美化看着基本正常，
   关掉美化（`-NoProfile`）错位最明显**。fastfetch 的 kitty 立绘是另一个已知的同类错位源
   （见 `docs/kitty-image-cursor-rca.md`），机理相同、触发路径不同。

**推荐修法：侧边栏不要动 PTY 列数**（只做渲染层的平移 + 裁剪），让 shell 完全感知不到
侧边栏。见 §5.1。治标试验与临时规避见 §5.2 / §5.3。

---

## 1. 复现与现象

按用户给的准确顺序（**先让屏幕上有 `ls` 输出当参照物**）：

```text
启动 pwsh 并执行 ls（屏幕上有 30 行左右的目录列表）
  → Ctrl+Shift+E 打开侧边栏 → 再按一次关闭
  → 输入任意英文字母（如 abc）
```

现象：`PS C:\Users\DYBAG>` 留在原处，`abc` 落到**下一行**、起点右移约 20 列（正好是提示符
宽度），左侧空出一大片。继续敲字母错位持续；回车后 shell 照常执行 `abc`（输入内容没错，
只是画错了地方）；按 `Ctrl+L` 重绘后立刻恢复。

关键截图：

| 文件 | 结果 |
|---|---|
| `target/_sidebar_bug/plain_base-3400.png` | 未开侧边栏：`PS ...> abc` 同行紧贴，正常 |
| `target/_sidebar_bug/plain_toggle-3400.png` | **开→关后：`abc` 掉到下一行、左侧空白，错位** |
| `target/_sidebar_bug/cmd_toggle-3400.png` | cmd.exe：`C:\...>abc` 同行紧贴，正常 |
| `target/_sidebar_bug/beauty_toggle_redr-3400.png` | 开合后按 `Ctrl+L` 重绘，恢复正常 |
| `target/_sidebar_bug/cursor_base-3400.png` / `cursor_toggle-3400.png` | ConPTY 自报坐标，见 §2 |

一键复现（约 1.5 分钟跑完 14 组）：

```bash
python tools/diagnostics/sidebar_input_probe.py
# 只跑核心三组：
python tools/diagnostics/sidebar_input_probe.py plain_toggle cursor_toggle pty_toggle
```

---

## 2. 证据链

### 2.1 六组对照（全部同一窗口尺寸、同一份 ls 内容）

| 组 | shell | 侧边栏 | 结果 |
|---|---|---|---|
| `plain_base` | pwsh `-NoProfile` | 不开 | 正常（`abc` 紧贴提示符） |
| **`plain_toggle`** | pwsh `-NoProfile` | **开 → 关** | **错位，最明显** |
| `plain_keep` | pwsh `-NoProfile` | 只开不关 | 正常 |
| `beauty_toggle` | pwsh 带 profile / oh-my-posh | 开 → 关 | 基本正常（有轻微 1 字符覆盖） |
| `beauty_base` | pwsh 带 profile | 不开 | 首字符被光标块盖住（另一个已知错位） |
| `cmd_toggle` | cmd.exe | 开 → 关 | 正常 |
| `split_only` | pwsh `-NoProfile` | 无侧边栏，只分屏 | 两个 pane 都正常 |

三个推论：

- **cmd 正常、分屏正常、只开不关正常** → 渲染几何、`pad_x` 平移、`Frame::clip_x`、
  鼠标命中都无罪；触发条件是"**变窄之后再变宽**"这一次往返。
- **带美化反而正常** → oh-my-posh 每次按键重绘提示符，掩盖了 PSReadLine 的过期坐标。
- `ls` 输出在 `plain_base` 与 `plain_toggle` 两张图里**像素级一致**，只有输入行不同 →
  不是整屏偏移，是 shell 重绘时发出来的字节不同。

### 2.2 ConPTY 自报坐标（硬数据）

让 shell 自己打印 `[Console]::CursorLeft` / `CursorTop` / `$Host.UI.RawUI.WindowSize.Width`：

| | LEFT | TOP | W |
|---|---|---|---|
| `cursor_base`（无侧边栏） | 0 | **27** | 98 |
| `cursor_toggle`（开→关后） | 0 | **26** | 98 |

**宽度已正确恢复成 98 列，行号却差 1。** 说明 PTY 尺寸同步本身是成功的，坏的是缓冲区
行号的记账 —— 这正是 `docs/config-manual.md` §12.4 特意规避的那件事：*"缓冲区本身没有
改动，所以 ConPTY 的行号与画面始终对齐（改了缓冲区会让之后输出的文字落到历史行上）"*。
侧边栏的列数往返恰恰改了缓冲区。

### 2.3 PTY 原始字节（`session_log`）

`target/_sidebar_bug/pty_base.bin` / `pty_toggle.bin` 各 5190 字节，**长度完全相同**，
差异只在 PSReadLine 重绘输入区时的绝对光标定位：

```text
base:    PS C:\Users\DYBAG> \e[?25l\e[28;20H...\e[93ma\e[0m...\e[28;21H...
toggle:  PS C:\Users\DYBAG> \e[?25l\e[25;20H...\e[93ma\e[0m...\e[25;21H...
```

`20` 是提示符末尾列号，两组一致；行号 `28 → 25`。这与 §2.2 的 `CursorTop` 偏差幅度
不同，说明被弄脏的是 **PSReadLine 自己维护的那份"提示符在缓冲区哪一行"的缓存**，
它既不等于 ConPTY 当前 `CursorTop`，也不等于屏幕上的真实行号。偏差幅度会随 resize 时
缓冲区里被折行的长行数量而变，不是固定的 1 或 3。

---

## 3. 代码链路

```text
Ctrl+Shift+E → browser_open()            src/main.rs:6194
                 ├ tab.browser = Some(Side)
                 └ commit_grid(grid)     src/main.rs:6209
再按一次     → browser_close()           src/main.rs:6218
                 ├ tab.browser = None（逻辑上立刻算"关了"）
                 └ commit_grid(grid)     src/main.rs:6225
                        ↓
commit_grid → resize_tab_layout          src/main.rs:4396 / 3237
            → tab_pane_geoms → shrink_pane_for_browser
                 g.x += inset; g.grid.0 = 可用列数    src/main.rs:3176
            → resize_pane                            src/main.rs:3440
                 ① p.resize_backend(rows, cols, px)  → pty.rs:334 → ConPTY ResizePseudoConsole
                 ② term.resize(TerminalSize{...})     → wezterm-term 整屏 reflow
                 ③ p.grid = grid
```

`browser_inset()`（`src/main.rs:3163`）在 `Mode::Side` 时返回
`side_cols * cell_w + 1`，默认 `DEFAULT_SIDE_COLS = 30`（`src/files.rs:44`），所以本例是
**98 → 68 → 98 列**。全屏模式（`Mode::Full`）返回 0，因此 `/files` 不触发本 bug。

两次 resize 之间窗口像素尺寸**始终没变**，变的只有 pane 的列数。ConPTY 的控制台缓冲区是
固定宽度的二维网格：变窄时把长行硬折成多行，变宽时**不会把软折行合并回去**；
`wezterm-term` 侧却会 reflow 合并。两边行号一旦错位，PSReadLine 的绝对定位就再也没法
对齐屏幕上的提示符。

注意 `live_resize_mode = "freeze" / "debounce"` **对此无效**：那两个模式只作用于
`apply_resize()`（窗口 `WM_SIZE`），而 `browser_open/close` 直接调 `commit_grid()`，
绕过 `resize_settler`（`src/main.rs:4351`）。

---

## 4. 明确排除的方向

| 方向 | 排除依据 |
|---|---|
| `renderer.pad_x` / pane 原点没还原 | `redraw` 每帧用 `orig_px` 复位（`src/main.rs:8596/8763`）；ls 输出位置两张图一致 |
| `Frame::clip_x` 退场后没清 | 每帧 `Frame::begin()` 置 `None`，且只在 `off > 0.5` 时开（`src/main.rs:8915`） |
| DXGI 拉伸 / 呈现层 | 窗口尺寸全程没变，且 cmd 组正常 |
| 输入法 / `winhook::fix_processkey` | `VECTORTERM_AUTOTYPE` 直接写 PTY、完全绕过键盘消息，照样复现 |
| oh-my-posh / fastfetch 画错 | 关掉美化后错位**更明显**；带美化反而基本正常 |
| 往 PTY 注入 `\n` 补行号 | 会污染 ConPTY 缓冲区，直接违反 §12.4 的设计原则，且对 vim/less 等全屏程序有害 |

---

## 5. 怎么修

### 5.1 推荐（治本）：侧边栏不再改 PTY 列数

思路：把侧边栏降级成**纯渲染层叠加**。PTY 永远保持整窗列数，shell 一次
`ResizePseudoConsole` 都收不到，PSReadLine 的坐标缓存就永远不会过期。

1. `src/main.rs:3176 shrink_pane_for_browser()`：**删掉 `g.grid.0 = cols`**，只保留
   `g.x += inset`（渲染 / 命中平移）与 `g.w`（可见宽度）。
2. `src/main.rs:6193/6218 browser_open()/browser_close()`：删掉两处 `commit_grid(grid)`，
   保留 `browser_anim` 与 `request_redraw()`。
3. 渲染：`redraw` 已经按 `g.x` 平移 `renderer.pad_x`（`src/main.rs:8716`），无需改；
   需要新增的是**把 pane 超出可见宽度的部分裁掉**。现有 `Frame::clip_from_here()`
   （`src/frame.rs:167`）是"从当前下标一直生效到 `overlay_at`"，得扩成带终点的区间
   （例如 `clip_x: Vec<(from, to, x0, x1)>` 或加 `clip_until_here()`），在画完 pane 之后、
   画面板之前放开，再为面板单独开一条裁剪带（现有逻辑已做）。CPU 端 `render.rs:2530`、
   GPU 端 `gpu.rs:1285/1548` 两处都要跟着改，并补 `clip_x_holds_on_both_backends` 单测。
4. 命中：`cell_at()`（`src/main.rs:6114`）、`pane_at()` 已经按 `g.x` 换算，确认列号仍按
   全宽网格算即可。
5. `open_in_current_pane()`（`src/main.rs:6686`）那条"先收面板让编辑器拿到全宽网格"的注释
   与 `browser_close()` 调用可以简化 —— 网格本来就没变过。

代价：侧边栏打开期间终端**右侧约 30 列被裁掉不可见**，长行不再实时重排。建议加一个
`files_gui.resize_pty = false`（默认 false，旧行为保留为可选），灰度切换。

**先做 30 秒 A/B 验证再动大改**：把 `browser_inset()` 临时改成直接 `return 0.0`（等价于
PTY 不变、侧边栏盖住终端左侧），重跑
`python tools/diagnostics/sidebar_input_probe.py plain_toggle cursor_toggle pty_toggle`。
若 `abc` 回到同行紧贴、`TOP` 回到 27、`\e[28;20H` 恢复，就证明"只要不 resize PTY 就好"，
再按上面 1~5 实施裁剪以保住"让开一条"的观感。

### 5.2 保留实时重排时的低成本试验（按优先级，均**未实测**）

| 试验 | 改动 | 为什么可能有效 / 风险 |
|---|---|---|
| B1 | `resize_pane()` 把 `term.resize()` 挪到 `p.resize_backend()` **之前** | 让 resize 引发的 shell 重绘按新网格解析，而不是先按旧网格吃进去再 reflow 一次。1 行调换，先试这个 |
| B2 | `browser_close()` 的 `commit_grid()` 延到退场动画结束（`redraw` 里清 `browser_exit` 那一刻，`src/main.rs:8538`） | 消除"逻辑已关、像素还在滑"期间的中间态；对根因未必有用，但顺手让时序干净 |
| B3 | 恢复全宽后补一次**幂等** `pty.resize(rows, cols, px)` | 想借 `WINDOW_BUFFER_SIZE_EVENT` 让 PSReadLine 重建缓存。ConPTY 对同尺寸是否发事件需实测；若必须 `cols±1` 抖一下，会闪一帧，不建议默认开 |

B1~B3 都要用 §6 的判据验收；**不要**把"发 `Ctrl+L` 重绘"做成默认行为：会清掉历史输出，
且 vim / less / 正在编辑的命令行都会被破坏。

### 5.3 用户侧临时规避（现在就能用）

- 关掉侧边栏后按一次 `Ctrl+L`：实测 `abc` 立刻回到同行（`beauty_toggle_redr-3400.png`）；
  代价是清屏。回车也能让 PSReadLine 整块重排，但会执行刚敲的命令。
- **改用 `/files` 全屏文件管理器**代替 `Ctrl+Shift+E` 侧边栏：`Mode::Full` 的 inset 为 0，
  不改 PTY 列数，不触发本 bug。
- `Remove-Module PSReadLine` 或临时换 `cmd.exe`：能规避，但牺牲行编辑体验。
- 调 `live_resize_mode` 无效（见 §3 末）。

---

## 6. 验收判据与回归

修完后跑：

```bash
python tools/diagnostics/sidebar_input_probe.py plain_base plain_toggle cursor_base cursor_toggle pty_base pty_toggle
```

四条硬判据（**不靠肉眼看图**）：

1. `plain_toggle-3400.png` 里 `abc` 与 `PS C:\Users\DYBAG>` 同一行、紧贴，左侧无空白；
2. `cursor_toggle` 的 `TOP` == `cursor_base` 的 `TOP`（本例 27），`W` 均为 98；
3. `pty_toggle.bin` 里的 `\e[<row>;20H` 与 `pty_base.bin` 一致（本例 `28;20H`）；
4. `cmd_toggle` / `split_only` / `beauty_toggle` 不回退。

必须回归的场景：分屏后开侧边栏（只裁宿主 pane）、开合动画没跑完就再开再关、
`/files` 全屏、窗口拖动缩放、带 oh-my-posh + fastfetch 的 profile、GPU 后端
（`render = "gpu"`）+ `clip_x_holds_on_both_backends` 单测。

---

## 7. 风险与本次未覆盖

- 本轮全部对照在 `render = "cpu"` 下取得；GPU 后端那一组导出的帧是空白（本机 dump 帧
  在 GPU 路径不可用），**没有纳入结论**。落地前请在 GPU 后端重跑 `cursor_toggle`。
- 错位幅度（行号差、缩进列数）依赖 resize 时缓冲区里长行的数量与提示符长度，不同
  `side_cols`、不同窗口宽度、不同 shell 配置下数值会变；判据应看重"两组是否相等"，
  不要写死 27 / 28 / 20。
- §5.1 会改变侧边栏期间的可见内容（右侧被裁），属于产品行为变化，建议配配置项并让用户
  可选；§5.2 的三条试验尚未编译验证，属于假设，不要直接当结论写进提交信息。
- 同类问题还有 `docs/kitty-image-cursor-rca.md` 记录的 fastfetch 图片占位导致首行输入
  错位。两者机理相同（ConPTY 行号 ≠ 屏幕行号），但那是图片放置的落点问题，修侧边栏
  不会顺带解决它。
