# 配置原理与 wezterm 兼容层

> 本文讲**设计取舍与实现原理**:配置为什么这样解析、wezterm 兼容层做了什么、
> 窗口装饰 / 拖动缩放 / 字体回退 / 选区复制背后的机制。
>
> **查某个配置项该写什么**请用 [config-manual.md](config-manual.md) —— 逐项列出类型、
> 默认值、取值范围与生效时机,另附 [§19 全量配置模板](config-manual.md#19-全量配置模板)
> 与 [§22 默认值总表](config-manual.md#22-默认值总表);可直接复制的示例见
> [`config.example.lua`](../config.example.lua)。
>
> 后续新增的配置分组也都只在手册里展开:[§11.4 滚动条](config-manual.md#114-可视化滚动条与边缘滚屏)、
> [§11.5 历史搜索](config-manual.md#115-历史搜索)、
> [§11.6 命令面板](config-manual.md#116-命令面板替代-shell-内命令)、
> [§15.8.7 窗格大小调整](config-manual.md#1587-拖动分割栏与快捷键调整窗格大小)、
> [§15.10 文件浏览器](config-manual.md#1510-文件浏览器)、
> [§4.8 会话保持](config-manual.md#48-session_persistence会话保持wezterm-式多路复用)。
>
> 内置主题(`theme = "deepseek"`,随发布版本分发的 `config/*.lua`)在**配置求值之前**
> 先加载一张底表,再把用户表深合并盖上去;名字白名单、查找顺序与热重载边界见
> [手册 §2.5](config-manual.md#25-内置主题)。

配置文件按下列顺序查找,**先命中的那个生效**:

| 优先级 | 位置 | 说明 |
|---|---|---|
| 1 | `--config <路径>` | 显式指定,存在与否都直接用它 |
| 2 | `%USERPROFILE%\.config\VectorTerm\current\` 里唯一的 `.lua` | 当前配置(管理器「设为当前配置」复制过来的);目录里多份会报错并退回第 3 项 |
| 3 | `<exe 目录>\config\config.lua` | 随软件分发的默认配置(当前默认 shell = PowerShell 7) |
| 4 | `<exe 目录>\config.lua` | 旧布局,兼容早期放在 exe 旁边的配置 |
| 5 | `<工作目录>\config\config.lua` | 开发时从仓库根启动也能吃到默认配置 |

想改配置不要动第 3 项(软件目录里的默认文件,每次同步源配置时会被覆盖):用配置管理器把它「设为当前配置」,它会被复制到 `C:\Users\<你>\.config\VectorTerm\current\`,之后改的就是终端加载的那份。`build.cmd` 编译成功后会把仓库的 `config\config.lua` 同步到 `target\debug\config\` 或 `target\release\config\`(仅在内容不同时才写入,可用 `VECTORTERM_NO_CONFIG_COPY=1` 关闭),所以双击 exe、从任意工作目录启动都能用上默认配置。配置加载失败会打印到 stderr 并回退内置默认值,不影响启动。

`shell` 写命令名即可(经 PATH 查找)。**默认值就是 PowerShell 7(`"pwsh.exe"` + `-NoLogo`)**,所以正常情况下不用写这一项;想用系统自带的写 `"powershell.exe"`,cmd 写 `"cmd.exe"`。PowerShell 7 通常是从 Microsoft Store 装的,`C:\Users\<你>\AppData\Local\Microsoft\WindowsApps\pwsh.exe` 只是个应用执行别名,系统会解析到 `C:\Program Files\WindowsApps\Microsoft.PowerShell_<版本>_x64__...` 下的真实 exe,所以版本升级后配置不用改;MSI / winget 装的则在 `C:\Program Files\PowerShell\7\pwsh.exe`,这两处不在 PATH 上时也会被找到(刚装完 PS7、资源管理器还带着旧 PATH 的情况)。

**注意**:显式写了 `shell` 就不会继承默认的 `-NoLogo`,需要关横幅请自己加 `shell_args`;`cmd.exe` 不认这个开关,配错会以"无效开关"直接退出。

## 配置写错了会怎样

和 wezterm 一样:**软件自身的错误不写进当前终端**,而是送到一个独立的调试终端窗口。
本机实测 wezterm 坏配置时会同时出现 `wezterm Configuration Error` 和正常的
`cmd.exe` 两个窗口 —— 错误窗口专门放错误,shell 窗口一个字节都不动。VectorTerm 同理:

1. 主窗口照常打开,用默认配置启动 shell,**当前终端画面里没有任何注入的错误文字,
   标题栏也不改**;
2. 自动弹出独立的控制台窗口 `VectorTerm Debug Console - N 个错误`,按时间顺序列出
   **全部**软件日志,ERROR 行标红,带文件、行列号和原始报错:
   `ERROR [VectorTerm] 配置加载失败,使用默认配置:解析 ... 失败:syntax error: ...:3: unexpected symbol near ','`;
3. 所有日志同时写 `~/.config/VectorTerm/VectorTerm.log`,配置错误另存 `config-error.log`;
4. 改好保存后自动热重载,旧会话一直可用,不会突然变样。

> 与 wezterm 的一点差别:它配置一坏就干脆不起 shell;VectorTerm 保留一个能用的会话
> (默认配置也能用),错误放在旁边的调试终端里,不至于让人没法干活。

调试终端**只在出错时**才弹,平时一个窗口都不多。想平时也盯着日志设
`VECTORTERM_DEBUG_CONSOLE=1`;想完全不弹(后台 / 自动化)设 `VECTORTERM_DEBUG_CONSOLE=0`,
日志仍写文件。窗口的关闭按钮已禁用、Ctrl+C 被忽略 —— Windows 控制台的关闭事件
默认会终止宿主进程,不能让用户随手一个 X 把终端连带 shell 一起关掉。

热重载失败(编辑器里改一半就保存)按错误内容去重,**同一个错误只弹一次**;
这时**保留旧配置**而不是回退默认值,你的终端不会突然变样。

完整示例见 [`config.example.lua`](../config.example.lua):

```lua
return {
  -- 默认 shell 就是 PowerShell 7(下面两行不用写)。第一顺位是 "pwsh.exe"
  -- (靠 PATH 上的应用执行别名,版本升级后不用改路径),没装则自动降到系统
  -- 自带的 "powershell.exe",再降到 "cmd.exe"
  shell    = "pwsh.exe",
  shell_args = { "-NoLogo" },           -- 关掉 PS7 的启动 banner(默认已配)
  padding       = 8.0,
  scrollback_lines = 3500,
  live_resize_mode = "realtime",         -- 拖边框时行列数实时跟随(wezterm 行为,见后文「拖动缩放」)
  resize_debounce_ms = 150,               -- 仅「非拖动」resize 的稳定窗口(0 = 实时,见后文「拖动缩放」)
  use_resize_increments = true,          -- wezterm 同名项:窗口按 cell_w×cell_h 整格步进缩放
  render        = "cpu",                  -- cpu(默认,拖动不抖) | gpu | auto(重启生效,见 [rendering.md](rendering.md))
  images        = { enable = true, max_memory_mb = 256 },  -- iTerm2 / Kitty 图形(见 [rendering.md](rendering.md))
  window_decorations = "TITLE|RESIZE",  -- 默认系统标题栏 + 原生缩放边框(见后文「窗口装饰」)
  -- integrated_title_buttons = { 'Hide', 'Maximize', 'Close' },  -- 顺序/集合,空表 = 全隐藏
  -- integrated_title_button_alignment = "Right",  -- Right | Left
  -- integrated_title_button_color = "Auto",      -- Auto | 颜色字符串
  -- integrated_title_button_style = "Windows",   -- Windows | Gnome
  window_rounded_corners = true,       -- Win11 无边框窗口圆角(默认开,见第 4 节)
  tab_bar = {
    enable = true,                       -- 关闭后标签栏整条消失(无边框时退化成标题条)
    at_bottom = false,                   -- true = 放到窗口底部
    fancy = true,                        -- 块状标签;false = 连成一片 + 分隔线
    hide_if_one = false,                 -- 只有一个标签时隐藏
    max_width = 16,                      -- 单个标签最大宽度(单元格)
    show_index = true,                   -- 显示 "1:" 序号
    show_new_tab_button = true,          -- 末尾的 "+"
    show_close_button = true,            -- 每个标签右侧的 "×"
    -- corner_radius = 8,                -- 圆角半径(px);不写按标签栏高度自动,0 = 直角
    -- 无系统标题栏 + Win11 圆角时,默认给标签栏四周各留 10 逻辑像素
    -- (按 DPI 放大),避免标签块被窗口圆角裁掉、也让上下左右留白对称。
    -- 想改就写数值(逻辑像素),写 0 = 贴边。顶层别名:
    -- tab_bar_edge_padding / tab_bar_top_padding
    -- edge_padding = 10,                 -- 左右各让出多少
    -- top_padding = 10,                  -- 到上边框的距离(底部栏时到下边框)
    colors = {                           -- 与 wezterm 的 colors.tab_bar 同结构
      background = "#0c0c0c",
      active_tab = { bg = "#2f2f2f", fg = "#ffffff" },
      inactive_tab = { bg = "#161616", fg = "#a0a0a0" },
      inactive_tab_hover = { bg = "#262626", fg = "#e6e6e6" },
      new_tab = { bg = "#0c0c0c", fg = "#a0a0a0" },
      new_tab_hover = { bg = "#2f2f2f", fg = "#ffffff" },
    },
  },
  selection = {
    copy_on_select = true,              -- 松开鼠标即复制选区
    edge_scroll = true,                 -- 拖到上/下边缘自动翻回看历史并继续扩选
    background = "#3a6ea5",             -- 选区底色
    opacity    = 0.45,                  -- 高亮不透明度
    ctrl_shift_v = "paste",             -- "paste"(默认)| "copy"(与 C 对调)
  },

  -- 字体:直接写家族名,或字体文件路径
  font = {
    family    = "MesloLGL Nerd Font Mono",  -- 想用图标就选 Nerd Font(或用回退)
    size      = 18,                         -- px @96dpi,随 DPI 缩放
    fallback  = { "Microsoft YaHei" },      -- 文本回退链(中文等)
    line_height = 0.95,                     -- 可选;不写则用字体自身行高(1.0~1.6em)
    -- cell_width = 1.0,
    -- bold = "MesloLGL Nerd Font Mono Bold",
    -- italic = "MesloLGL Nerd Font Mono Italic",
    nerd_font = "auto",                     -- 自动发现 Nerd Font 画 PUA 图标
    -- dirs = { "D:/Fonts" },               -- 额外字体目录
  },

  -- 可选:环境变量 / 会话记录
  -- env = { FOO = "bar" },
  -- session_log = "C:/Users/you/vectorterm-session.log",
  colors = {
    foreground = "#cccccc",
    background = "#0c0c0c",
    cursor_fg  = "#0c0c0c",
    cursor_bg  = "#ffffff",
    -- 可选:覆盖基础 16 色(顺序 black red green yellow blue magenta cyan white + 亮色)
    -- ansi = { "#0c0c0c", "#c50f1f", "#13a10e", ... },
    -- 可选:覆盖 256 色表的任意下标(SGR 38;5;n / 48;5;n 用这张表)
    -- indexed = { [196] = "#ff0000", [231] = "#ffffff" },
  },
  cursor = {
    trail                 = 20,          -- 静止毫秒数(0 = 关闭)
    trail_start_threshold = 0,           -- 每轴最小位移(格);0 = 每一步都拖尾,2 = kitty 默认
    trail_decay           = "0.1 0.4",   -- (最快, 最慢)衰减时间(秒)
    trail_max             = 220,         -- 单段动画硬上限(ms);0 = 不限制
    trail_color           = "none",      -- none = 跟随光标色
  },
  scroll = {
    anim       = 90,    -- 一个滚轮档位的平滑动画时长(ms);0 = 关闭(逐行瞬移)
    max        = 200,   -- 单段动画硬上限(ms)
    back       = 90,    -- 按键/粘贴后回到底部的时长(ms)
    back_burst = 40,    -- 长按连续输入(回车连发)时回底的时长(ms)
  },

  -- 键位映射(可选):值为内置动作或 Lua 函数
  keys = {
    ["ctrl+shift+r"] = "reload",
    ["f12"]          = "send:dir\r",
    ["ctrl+alt+t"]   = function() VectorTerm.notify("hi") end,
  },
}
```

**标签栏**:每个标签是一个独立的 ConPTY 会话(独立终端状态、回看缓冲、选区、尾迹动画)。键盘 `Ctrl+Shift+T` 新建、`Ctrl+Shift+W` 关闭、`Ctrl+Tab` / `Ctrl+Shift+Tab` 前后切换、`Ctrl+1..9` 跳到第 n 个;鼠标左键切换、中键关闭、点标签右侧的 `×` 只关那一个、点末尾 `+` 新建,悬停会高亮。关掉最后一个标签等同退出程序。标签标题优先用应用通过 OSC 0/2 设置的窗口标题,没有时显示 shell 名。

标签块默认带圆角(`fancy = true` 时),半径由标签栏高度推出,可用 `corner_radius` 固定;活动标签朝内容区的那条边是直角,和终端连成一体。标签被挤窄到放不下标题时(`< 4 格`余量)会自动隐藏 `×` 按钮,避免标题被吃掉——此时仍可用中键、`Ctrl+Shift+W` 或 `Ctrl+1..9` 定位后关闭。

**四周留白(到窗口边框的距离)**:无系统标题栏 + Win11 圆角时,标签栏内容会在**上下左右各让出 10 逻辑像素**(按 `scale_factor` 放大),这样第一个标签不会钻进圆角三角区,fancy tab 背向内容区的两个圆角也能完整露出来,而且标签到上边框的距离和到左右边框的距离一致。实现上把标签栏拆成「整条占位区」和「内容带」:靠窗口外框那侧是纯背景留白(顶部栏在上、底部栏在下),内容带另一侧始终贴着终端内容区,不留缝;背景条仍铺满整宽由 DWM 裁圆,CSD 模式下留白区照样能拖动窗口。留白可用 `tab_bar.edge_padding`(左右)/ `tab_bar.top_padding`(上下,也支持顶层别名 `tab_bar_edge_padding` / `tab_bar_top_padding`)按逻辑像素自定义,写 `0` 就是贴边;不写则自动跟随窗口模式(**有系统标题栏时 4** —— 客户区上沿是直的,不用躲圆角,只留一条呼吸缝;无边框且关了圆角时为 0)。窗口被拖到很矮时留白会自动收缩,不会把标签挤没。

标签宽度与 wezterm 一致:先按 `tab_max_width` 封顶、剩余宽度均分,标签多到放不下就继续挤压(不做横向滚动)。`hide_if_one = true` 时单标签不占高度,内容区自动多一行。标题按**单元格宽度**排版:中文占两格,超出用省略号。

**窗口装饰**(`window_decorations`):这一项照搬 wezterm 的标志组合语义 —— "有没有系统标题栏"、"有没有可缩放边框"、"窗口按钮画在哪"是**三个独立维度**,不再像旧版那样一关标题栏就自动塞三枚自绘按钮。

| 写法 | 系统标题栏 | 窗口能否缩放 | 标签栏里的窗口按钮 |
|---|---|---|---|
| `"TITLE\|RESIZE"`(VectorTerm 默认,也是 wezterm 默认) | ✅ 原生三枚按钮 | ✅ 系统边框 | ❌ |
| `"TITLE"` | ✅ | ❌ | ❌ |
| `"RESIZE"` | ❌ | ✅ 自绘四边热区 | ❌ |
| `"INTEGRATED_BUTTONS\|RESIZE"` | ❌ | ✅ 自绘四边热区 | ✅ 三枚 |
| `"INTEGRATED_BUTTONS"` | ❌ | ❌ | ✅ 三枚 |
| `"NONE"` | ❌ | ❌ | ❌ |

- 不写 = `TITLE|RESIZE`,即**默认显示 Windows 系统标题栏** —— 系统阴影、贴边吸附(Aero Snap)、双击标题栏最大化、任务栏右键菜单都是原生的,不用自己模拟。想要无边框 + 标签栏集成按钮写 `"INTEGRATED_BUTTONS|RESIZE"` 或简写 `window_titlebar = false`。
- `"RESIZE"` 是 wezterm 官方推荐的"只去掉标题栏"写法:**顶部那条标签栏仍在,但标签栏里不画窗口按钮**,只留拖动区。这里必须说明一个实现差异:wezterm 自己创建 Win32 窗口,能保留 `WS_THICKFRAME` 让系统继续画缩放边框;VectorTerm 用的 winit 0.30 是靠 `WM_NCCALCSIZE` 把非客户区压成 0 实现无装饰的,补 `WS_THICKFRAME` 也不会真的出现系统边框。所以 `RESIZE` 由程序自绘四边/四角热区来**模拟**系统缩放(光标贴边变双箭头),`NONE` 则完全不缩放。
- 只有 `INTEGRATED_BUTTONS` 才把 **最小化 / 最大化 / 关闭** 搬进标签栏(46 逻辑像素那种标准尺寸,随 DPI 缩放),悬停高亮、关闭悬停变红,最大化后图标变成"还原"双框。按钮集合/顺序/左右/颜色/样式全部可配:`integrated_title_buttons = { 'Hide', 'Maximize', 'Close' }`(`{}` 或 `false` = 三枚全隐藏、也能只留 `'Close'`)、`integrated_title_button_alignment = "Right" | "Left"`、`integrated_title_button_color = "Auto" | "#RRGGBB"`、`integrated_title_button_style = "Windows" | "Gnome"`。**关按钮只是不画按钮,顶部那条栏照样在、照样能拖窗口**,不会连带把状态栏去掉 —— 这正是和旧实现最大的区别。
- 没有系统标题栏时标签栏一定保留(它兼任标题栏):拖动空白处移动窗口、**双击**切换最大化;即使 `tab_bar.enable = false` 或 `hide_if_one = true`,也会留一条纯拖动条。窗口处于最大化时,拖动空白处会**先还原成普通尺寸再跟着鼠标走**(抓哪儿的相对位置就停在哪儿),和系统标题栏一致;单纯单击(不移动)不会把窗口还原。另外最大化时不再把贴顶/贴左右的那几像素当成缩放热区 —— 否则点在标题栏最上沿会被判成"上边缘缩放",表现就是窗口既缩不了也拖不动。
- `"TITLE"` 由系统负责缩放;无系统标题栏且带 `RESIZE` 时四边四角的缩放热区由程序自绘(热区约 0.35 行高、夹在 4–10px)。winit 的无装饰窗口没有系统阴影,Win11 拖到屏幕边缘的 Aero Snap 也不可用;窗口移动仍走系统原生拖动。
- **圆角**:无系统标题栏时 winit 把非客户区压成 0,DWM 默认按 Win11 的"方角"处理,所以早期版本关掉标题栏后窗口四角是硬的直角。现在会通过 `DwmSetWindowAttribute(DWMWA_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND)` 把圆角要回来(`"Round"`,最大化/全屏时由系统自动不圆),`window_rounded_corners = false` 可强制方角。这一项是 VectorTerm 扩展,wezterm 没有同名配置;Windows 10 无圆角能力时该项无效果。系统标题栏模式保持 `Default`,跟随系统/用户的全局圆角设置。
- `window_titlebar = true/false` 是简写,只覆盖"有无系统标题栏"这一维;和 `window_decorations` 一起写时以它为准,其余维度仍取 flags 的结果。未知标志(macOS 专有、拼错)只提示不报错。启动日志会打印 `[VectorTerm] 窗口装饰: … / 自绘缩放边框=… / 标签栏按钮 N 枚 …` 和 `[VectorTerm] 标签栏安全区: 左右=… / 上下=… (内容带 … / 整条占位 …)`,配置有没有对上一眼就能看出来。这一组需要重启生效。

**拖动缩放时文字不抖 / 与 wezterm 的 resize 逻辑对照**:Windows 拖窗口边框时每移动一两个像素就发一次 `WM_SIZE`。这里走过两个弯路:先加 150ms 防抖(拖动中一次 >150ms 的空隙就被误判松手、当场同步 PTY),再改成拖动期间完全冻结网格。后者尤其不对——设 `VECTORTERM_TRACE_RESIZE=1` 抓时序日志可以看到,winit 在模态缩放循环里 `WindowEvent::Resized` → `RedrawRequested` 是**实时成对**派发的(946→1008→1016→1024→1032 每次都重绘),呈现层根本没掉帧;冻结反而让画布实时变大、行列数却纹丝不动,标签栏实时布局而终端文字不动,右侧留白,松手瞬间整屏重排。用户看到的「标签栏文字和所有内容一起抖」就是这种脱节,而不是 PTY 重绘。

wezterm 从一开始就是实时 `apply_dimensions()`:文字位置跟着窗口实时调整。现在默认与它一致。

**实时同步行列数只解决了一半**。剩下那个「拖右边/下边时内容被横向拉伸一下又弹回」跟行列数无关,是**呈现时机**问题:窗口客户区先变大、新尺寸的帧还没 present 上去,DXGI / DWM 就会把上一帧(旧尺寸)缩放填满新客户区 —— 拉一下缩放一次、再恢复,每步都来一遍,看上去就是文字在横向抖。拖左边/上边时窗口位置也在变,DWM 走的是另一条合成路径,基本看不出来,所以这个现象是**单边**的。

只把 `WindowEvent::Resized` 改成同步重绘还不够:`redraw()` 里第一步是 `Surface::configure`,也就是 DXGI `ResizeBuffers` —— 实测这台机器上要 **4~5ms**(与 `PresentMode` 无关,Immediate 也一样)。这 5ms 正好落在「窗口已经变大、新帧还没上屏」的空档里,所以单靠同步重绘,12% 的拖动帧仍然是拉伸帧。

两个实现的窗口架构差别正在这里:

| | wezterm(自己就是窗口过程) | VectorTerm(winit) |
|---|---|---|
| 尺寸消息 | `WM_SIZE` / `WM_WINDOWPOSCHANGED` → `wm_size()` → **同步** `dispatch(WindowEvent::Resized)`,紧接着 `wm_paint()` → `dispatch(NeedRepaint)`;应用在**同一条窗口消息里**完成 `apply_dimensions()` + 绘制 + present | `WM_SIZE` 只被转成事件塞进队列,回到主循环才派发;`WM_PAINT` 里 `BeginPaint` / `EndPaint` **什么都不画**(源码注释就是 `// Do nothing right now`),然后 `DefWindowProc` 把区域标记成已更新 |
| 拖动中推进 | `in_size_move` 为真时 `wm_size()` 还会 `SPAWN_QUEUE.run()`,把排队的异步 paint 任务当场跑掉 | 没有等价机制,重绘只能靠 `request_redraw()` 排到下一轮 |
| 交换链何时变大 | `wm_size()` 里当场 `webgpu.resize(dimensions)` —— 仍在窗口变大**之后** | 同左,而且是重绘的第一步,白占 5ms |

**真正把空档压掉的是把 `ResizeBuffers` 提到窗口变大之前**(`src/winhook.rs`)。`WM_SIZING` 是系统在**真正改窗口矩形之前**发来的,带一个可修改的目标矩形;winit 自己在这个消息里做整格吸附。于是用 `SetWindowLongPtrW(GWLP_WNDPROC)` 串一个窗口过程:先原样转发给 winit(让它把矩形吸附好),再读那个「最终会被采用」的矩形,当场把交换链配成新的客户区尺寸。等系统真的改完窗口矩形、发来的 `WM_SIZE`,交换链已经是新尺寸,`redraw()` 里只剩绘制 + present:

| | 改动前(7ms 节流 + `WM_SIZE` 里重建交换链) | 改动后 |
|---|---|---|
| `EV_RESIZED` → present 完成 | 5.0~5.2ms | **0.8~1.4ms** |
| 其中 `WM_SIZE` 里那次 `GPU_CONFIGURE` | p90 3.5~4.9ms,max 7.2ms | **0(不再在 `WM_SIZE` 里重建)** |
| 高速抓帧统计的拉伸帧 | 20/167 = **12%**(文字跨度 +3px) | **0/167**,多轮复测均 0 |

抓帧用的是 `tools/diagnostics/verify_stretch_fast.py`:只抓标尺文字所在的一条横带(GDI BitBlt,~166fps),统计拖动全程文字跨度的变化;`VECTORTERM_NO_SIZING_HOOK=1` 可关掉预配置做 A/B。单看抓帧,关掉钩子的对照也常常是 0% —— 去掉那 7ms 节流后每一步本来就都能同步呈现;但那 3.5~4.3ms 的重建仍然压在「窗口已经变大」之后,究竟会不会被 DWM 合成到,取决于合成点落在哪一帧。所以判断依据要看时序(`EV_RESIZED` → `PRESENT_SYNC` 的毫秒差),不是只看抓帧比例。

配套的两处调整:`WindowEvent::Resized` 里**无条件**同步 `redraw()` + present(不再有 7ms 节流,因为一次只要 ~1ms);`WM_PAINT` 带来的重复重绘只在「尺寸和上次呈现的一模一样」时才跳过 —— 原先按时间跳过会在快速拖动时把**新尺寸**的帧一起丢掉。预配置万一失准(系统最终给了别的尺寸,比如撞到 min/max、贴屏幕边),`WM_SIZE` 里发现尺寸对不上会再配一次,退化成改动前的行为,不会画错。

对照 wezterm 的真实实现(`wezterm-gui/src/termwindow/resize.rs` 的 `resize()` / `apply_dimensions()`,以及 `resize_increment_calculator.rs`):

| | wezterm | VectorTerm |
|---|---|---|
| 拖动中同步 PTY | **立即**:`resize()` → `apply_dimensions()` → `for tab in window.iter_tabs() { tab.resize(size) }`,没有防抖 / 定时器 | **默认立即**(`live_resize_mode = "realtime"`),并在 `Resized` 事件里同步绘制 + present,不等下一轮事件循环;可选 `debounce` / `freeze` 作退路 |
| 抑制抖动的主要手段 | `window.set_resize_increments(ResizeIncrementCalculator{ x: cell_w, y: cell_h, base_width: padding…, base_height: … + tab_bar_height }.into())` —— 窗口只按整格变化,WM_SIZE 稀疏化 | 同样设整格步进 + 实时同步呈现 + 精确格子像素;画布、标签栏、surface 共用一个尺寸源 |
| 画布尺寸 vs 行列数 | `self.dimensions` / `webgpu.resize(dimensions)` 与 `self.terminal_size` / `tab.resize(size)` 分开 | `grid_px` / `update_geometry()` 与 `commit_grid()` 分开,但默认同一帧一起推进 |
| 传给 terminal 的像素尺寸 | `pixel_width = cols * cell_size.width`(精确格子像素,不用可用空间,见 wezterm issue #535) | 同左,`commit_grid()` 按 `grid × cell_w/cell_h` 算 |
| 去重 | `if self.dimensions == dimensions && self.window_state == window_state { NOP }` | `if (w,h) == grid_px { return }` + 网格没变就不提交 |
| `live_resizing` | `WM_ENTERSIZEMOVE` / `WM_EXITSIZEMOVE` 维护 `in_size_move`,随 `WindowEvent::Resized { live_resizing }` 传出;只用来跳过 DPI / 字体缩放重算,仍然立即 `apply_dimensions()` | winit 0.30 没暴露这个标志,用全局左键状态 `GetAsyncKeyState(VK_LBUTTON)` 等价判定模态缩放循环(不依赖主循环在模态期间是否还跑);只用来区分「拖动走 `live_resize_mode`」与「非拖动走 `resize_debounce_ms`」 |

**实测**(原样 DARK 色板配置,自绘缩放边框,真实鼠标拖右边框 86px):`realtime` 拖动中同步 **10 次**、`debounce` / `freeze` 各 **1 次**(松手后);三种模式下标签栏带与终端带的**内容位移恒为 (0,0)**,说明实时同步不会造成整帧拉伸或错位。换一份 229 字符长行的静态画面再拖:相邻帧差异呈 `12.3 / 12.5 / 0.0 / 12.3…` 的交替,正好对应整格步进——每跨一格长行折行点前移一次,没跨格的那帧完全不变;对比拖动前后截图,折行位置明显变了而标签栏、提示符位置丝毫未动,即「文字实时调整」而非「内容抖」。上下拖 32px(整格步进 2 次)同样全程 0 位移。

**为什么早先会觉得「实时同步 = 抖」**:那是在程序化 `SetWindowPos` 下用「距最后一次 resize 的时间」做防抖,拖动中 GPU 重建交换链、系统调度、抓帧都可能让两帧之间出现一次 >150ms 的空隙,于是被误判成松手、当场同步 PTY,画面跟着拖动方向跳一下(左右拖改列数→软折行重排,上下拖改行数→提示符整屏上移)。实测在连续 resize 中插入一次 350ms 卡顿:纯防抖会在**拖动期间**同步 1 次、全程 2 次;完全冻结则拖动期间 **0 次**、松手后 1 次。现在 `live_resize_mode = "realtime"` 干脆每次当场同步,不再有「误判松手」这个状态机;`resize_debounce_ms` 只管非拖动的尺寸变化(程序化 resize、最大化 / Snap、Win+方向键)。实在遇到 PSReadLine 重绘太跳的机器,可退到 `debounce` 或 `freeze`。

- `use_resize_increments`(wezterm 同名配置,默认 `true`,支持热重载):用 `cell_w × cell_h` 作窗口缩放步长。真实鼠标拖动实测:开启时窗口宽度每 **12px** 一跳、24 次鼠标移动只产生 **9 个**不同尺寸;关掉时每 4px 一跳、**24 个**不同尺寸。wezterm 的计算器还带 `base_width` / `base_height`(padding + border + 标签栏高度),winit 只暴露增量本身;无边框窗口外框=客户区,所以用纯格子增量即可。`false` = 允许逐像素缩放。
- 画布、标签栏、surface / swapchain 始终以同一组尺寸(`grid_px`)为准并立即跟随窗口,避免 `WM_PAINT` 先于 `WM_SIZE` 时同一屏里内容按旧宽、标签栏按新宽摆放。格宽由字体度量固定,不随窗口宽度变化,所以文字不会被水平拉伸。
- 拖动期间(左键按住 = Windows 模态缩放循环)按 `live_resize_mode` 决定:默认 `realtime`,每次格子数变化当场 `commit_grid()`,文字实时重排;`debounce` 用 `resize_debounce_ms` 合并;`freeze` 一次都不提交、松手由 `redraw` / `about_to_wait` 检测边沿提交一次。非拖动尺寸变化走 `resize_debounce_ms`(默认 150ms,上限 1000ms),`about_to_wait` 用 `WaitUntil` 兜底唤醒。后两项是 VectorTerm 扩展,wezterm 没有同名配置。
- `live_resize_mode` / `resize_debounce_ms` / `use_resize_increments` 都支持热重载;启动日志会打印 `拖动时行列数=… / 非拖动稳定窗口=… / 缩放步进=…`。诊断用的环境变量:`VECTORTERM_TRACE_RESIZE=1` 打出每次 `EV_RESIZED` / `DRAW_ENTER` / `GPU_CONFIGURE` / `GPU_DRAW` 的毫秒级时序;`VECTORTERM_NO_SIZING_HOOK=1` 关掉 `WM_SIZING` 预配置做 A/B(退回「`WM_SIZE` 里才重建交换链」的老行为);`VECTORTERM_PRESENT_MODE=immediate|mailbox|novsync` 与 `VECTORTERM_FRAME_LATENCY=N` 用来试不同的呈现模式,默认 `AutoVsync` + 1。
- `freeze` 模式的代价:拖动中窗口右侧/下侧会暂时留白(格子数还没跟上),松手后一次填满;默认 `realtime` 没有这个空档。实测模拟 60Hz 连续拖动(每 ~14ms 加宽 8px),`freeze` 把网格同步次数从 **17 次降到 1 次**、文字区域相邻帧平均变化率从 **11.39% 降到 0.00%**,代价是松手时一次性重排。
- 拖动判据用全局左键状态,副作用仅是「按住左键选文本时若窗口被程序改尺寸,会走拖动分支的策略」;不影响正常拖动。静止画面下左右 / 上下各 16 步 resize,相邻帧差异 **0.00%**、内容位移恒为 (0,0),说明静止画面本身没问题 —— 抖动只在「窗口变大但新帧未上屏」的那一刻出现(见上一节的 `WM_SIZING` 预配置)。

**窗口圆角与标签栏**:无边框 + DWM 圆角时,贴边的标签块会被左上/右上角裁掉一块(早期版本第一个标签从 `x = 0` 开始,正好压在圆角三角区里)。现在标签栏内容(标签、新建按钮、窗口按钮)四周各让出 `10 × scale_factor` 物理像素的安全区(左右 `edge_padding`、上下 `top_padding`,可自定义或写 0 贴边),背景条仍铺满整宽、由 DWM 裁圆;同时所有标签统一成 wezterm fancy tab 的形状 —— **只有背向内容区的两角是圆的,朝内容区那条边补成直角**、和终端连成一片(原先只有活动标签这样,非活动标签是四角全圆的胶囊,贴着圆角时更突兀)。有系统标题栏时客户区上沿是直的,只留 `4 × scale_factor` 像素的呼吸缝(标签紧紧跟在标题栏下面);`window_rounded_corners = false` 且无边框时完全不加内边距。

**wezterm 配置兼容**:`VectorTerm.exe --config C:/Users/you/.wezterm.lua` 可直接加载 wezterm 的配置。兼容层实现了 `require 'wezterm'`、`config_builder()`、`font()` / `font_with_fallback()`、`action.*`、`nerdfonts`、`on()`、`strftime()`、`format()`、`battery_info()`。生效字段:`default_prog`、`font` / `font_size` / `line_height`、`window_padding`、`initial_cols` / `initial_rows`、`colors`(含 `ansi` + `brights`、`selection_bg`、`tab_bar`)、`enable_tab_bar` / `tab_bar_at_bottom` / `use_fancy_tab_bar` / `hide_tab_bar_if_only_one_tab` / `tab_max_width` / `show_tab_index_in_tab_bar` / `show_new_tab_button_in_tab_bar`、`set_environment_variables`、`keys`(数组格式 + `wezterm.action.*`)、`window_decorations` 全套标志 + `integrated_title_buttons` / `integrated_title_button_alignment` / `integrated_title_button_color` / `integrated_title_button_style`、`use_resize_increments`、简写 `window_titlebar`、VectorTerm 扩展 `window_rounded_corners` / `live_resize_mode` / `resize_debounce_ms`(完整映射见后文「窗口装饰」)。暂不生效:`window_frame`、`enable_scroll_bar`(VectorTerm 自己的可视化滚动条是 `scrollbar.*`,与这个 wezterm 开关无关,见 [手册 §11.4](config-manual.md#114-可视化滚动条与边缘滚屏))、`default_cursor_style` / `cursor_blink_rate` / `animation_fps`(光标形状与闪烁跟随应用、动画固定为 120fps 上限)、`wezterm.on(...)` 注册的状态栏回调。逐项配置手册见 [config-manual.md](config-manual.md)。

**鼠标文本选择**:左键拖拽即可框选,双击选词、三击选整行(继续拖动仍按词/行扩展);松开时按 `selection.copy_on_select` 自动写入系统剪贴板,选区保持高亮直到下一次按键或点击。选区用**绝对物理行号**锚定,回看滚动时跟着原来的文字走;复制时宽字符不重复、行尾空格裁掉、软折行的两行之间不插换行。

| 操作 | 结果 |
|---|---|
| 左键拖拽 | 框选 + 松开自动复制 |
| 双击 / 三击 | 选词 / 选整行 |
| `Ctrl+Shift+C`、`Ctrl+Insert`、右键(有选区时) | 复制选区 |
| `Ctrl+Shift+V`、`Ctrl+V`、`Shift+Insert`、右键(无选区时) | 粘贴(按 `?2004h` 自动加括号粘贴包裹) |
| `Shift` + 左键拖拽 | 应用开启鼠标报点(vim/tmux)时强制本地框选 |

把 `selection.ctrl_shift_v` 设为 `"copy"` 即可让 **`Ctrl+Shift+V` 变成复制选区**、同时 `Ctrl+Shift+C` 改为粘贴;这是为了兼容把 V 记成"复制"的操作习惯。应用抓了鼠标时,普通左键仍按 1000/1002/1003 模式原样上报给应用,不会抢它的点击。所有 `selection.*` 配置都支持热重载。

颜色值支持 `#RGB` / `#RRGGBB`(由 wezterm 的颜色解析器处理)。默认光标为白色(`cursor_bg = #ffffff`,字色取背景色)。

**颜色能力**:调色板默认就是完整的 256 色表(xterm 16 基础色 + 216 色立方 + 24 灰阶),真彩色(`SGR 38;2;r;g;b` / `48;2`)原样透传不降级;子进程默认带 `TERM=xterm-256color`、`COLORTERM=truecolor` 环境变量,应用会主动使用 256 色/真彩色。`colors.ansi` / `colors.indexed` 可覆盖任意调色板下标;OSC 4/10/11/12 动态改色同样生效。

**字体与回退**:`font.family` 可以是家族名(`"Consolas"`、`"MesloLGL Nerd Font Mono"`),也可以是字体文件路径。字体索引只读 sfnt 头与 `name`/`OS/2`/`head`/`post` 小表,因此不必把 `C:\Windows\Fonts`(约 600MB)读进内存;同一家族会自动拆成 Regular / Bold / Italic / BoldItalic 四个槽,渲染时按 SGR 属性选 face,缺 face 才合成。

**中文渲染**:字体按 em 归一化 —— ab_glyph 的 `PxScale` 表示的是 "ascender→descender 的高度" 而不是字号,微软雅黑的 asc-desc 是 1.32em,不归一的话汉字会缩小到 0.76em、在两格单元里留出明显空隙。字形**保持字体自然比例**,只有墨迹超过所占单元格(附 0.25 格容差)时才等比缩小,不再为了填满格子而横向拉伸(这是 wezterm glyph cache 的规则);缩小用水平 box filter,避免笔画被最近邻压成色块。

**Nerd Font**:图标的码点都在 Unicode 私用区(`U+E000–U+F8FF`、`U+F0000–U+FFFFD` 等)。`nerd_font = "auto"`(默认)会在第一次遇到缺字时扫描系统字体、挑出 Nerd Font(Symbols / Mono 变体优先),并把它放在**文本回退链之前**——否则某些 CJK 字体私用区里的占位字形会把图标变成方块。图标按 `min(格子宽, 自然步进)` 等比缩放并在格内居中,powerline 箭头也能首尾相接。也可显式写 `nerd_font = "MesloLGL Nerd Font Mono"`,或 `false` 关闭。

**行高**:默认取字体自身的 asc-desc,但夹在 1.0em~1.6em。Meslo 等 Nerd Font 的 hhea 是 1.55em,行距会偏大;想紧凑就写 `line_height = 0.85`。

**热重载**:保存**当前配置**(`~/.config/VectorTerm/current/` 里那份)后约 0.4 秒内自动生效,覆盖动画参数、颜色、键位/脚本、**选区配置**、**标签栏开关 / 配色 / 四周留白**、**滚动条 / 历史搜索 / 分割线 / 文件浏览器**、**`live_resize_mode`** / **`resize_debounce_ms`** / **`use_resize_increments`**;字体、窗口几何、`render` 后端、`session_persistence`、shell 与启动参数需要重启(逐项对照表见 [手册 §18](config-manual.md#18-热重载对照表),边界见 [limitations.md](limitations.md))。

**键位映射**:`keys` 的键名大小写/顺序无关(`shift+ctrl+t` 等价 `ctrl+shift+t`),支持 `ctrl`/`alt`/`shift`/`super` 与字母、数字、`enter escape tab space backspace delete insert up down left right home end pageup pagedown f1..f12`。值可以是:

- 内置动作字符串:`"paste"`、`"reload"`、`"quit"`、`"send:<文本>"`(文本里的 `\r` 才会回车执行)
- Lua 函数(在按下时调用,可用全局表 `VectorTerm`)

`VectorTerm` API:

| 方法 | 作用 |
|---|---|
| `VectorTerm.notify(msg)` | 改窗口标题作为提示 |
| `VectorTerm.send(text)` | 向 shell 发送文本 |
| `VectorTerm.paste()` | 粘贴剪贴板 |
| `VectorTerm.reload()` | 重载配置 |
| `VectorTerm.quit()` | 退出 |
| `VectorTerm.cursor_color("#rrggbb")` | 改光标颜色(立即生效) |
| `VectorTerm.trail_dwell(ms)` | 尾迹静止阈值 |
| `VectorTerm.trail_threshold(x [, y])` | 每轴格数阈值 |
| `VectorTerm.trail_decay(fast [, slow])` | 衰减时间(秒) |
| `VectorTerm.trail_max(ms)` | 单段动画硬上限;`0` = 不限制 |
| `VectorTerm.trail_color("#rrggbb" 或 nil)` | 尾迹颜色 |

> **从 WinTTY 更名而来的兼容**:更名前写的 `wintty.set(...)` / `wintty.notify(...)`
> 仍然可用 —— `wintty` 与 `VectorTerm` 指向同一张表,新老写法可以混用。
> 配置目录也会在首次启动时自动从 `%USERPROFILE%\.config\wintty\` 迁移到
> `%USERPROFILE%\.config\VectorTerm\`(搬不动就继续用旧目录,不会丢配置)。
