-- VectorTerm 配置示例
-- 用法:复制到 %USERPROFILE%\.config\VectorTerm\config.lua(推荐,优先级最高),
--       或 exe 同目录的 config\config.lua;按需修改。查找顺序见手册 §2.1。
-- 颜色支持 #RGB / #RRGGBB。保存后自动热重载(动画/颜色/脚本立即生效)。

return {
  -- 内置主题:随包分发在 config/deepseek.lua(DeepSeek 夜空蓝配色,色板取自
  -- 内置的 DARK 色板:品牌蓝 #4D6BFE 活动标签 + 画布 #0B1220,
  -- 并彻底关掉右侧滚动条)。
  -- 写了 theme 之后,下面所有键都是「盖在主题之上」的覆盖项(深合并),
  -- 只改一两个值也不用复制整份主题。不写 = 纯内置默认值。详见手册 §2.5。
  -- theme = "deepseek",

  -- 启动的 shell:写命令名(走 PATH)或完整路径。默认 "pwsh.exe"
  --   PowerShell 7 → "pwsh.exe"(应用执行别名,升级不用改路径)—— 默认值
  --   系统自带     → "powershell.exe"(没装 PS7 时自动降到这个)
  --   cmd          → "cmd.exe"(连 PowerShell 都没有时降到这个)
  -- 找不到时会自动降级:pwsh → powershell → cmd → %COMSPEC%,
  -- 所以配置从别的机器拷过来也不会起不来(启动日志会说明换成了哪个)。
  shell = "pwsh.exe",
  -- 传给 shell 的额外参数(可选)。显式写了 shell 时默认参数不继承,
  -- 需要关横幅就自己加 { "-NoLogo" };cmd 不认这个开关,别配错。
  -- shell_args = { "-NoLogo" },
  -- wezterm 写法:第 1 项是程序,其余是参数(显式写了 shell 时忽略这一项)
  -- default_prog = { "pwsh.exe", "-NoLogo" },
  -- 追加/覆盖环境变量(可选)
  -- env = { FOO = "bar", GIT_PAGER = "cat" },
  -- 会话输出记录(可选;原始字节,含 ANSI 转义序列)
  -- session_log = "C:/Users/you/vectorterm-session.log",
  -- 启动目录(不填则继承 vectorterm 的 cwd;路径不存在时自动忽略)
  -- cwd = "C:/Users/you",

  -- 字体:默认就是 wezterm 同款,三款都编进了 exe,不用先装字体。
  -- 主字体 JetBrains Mono(含 Regular/Bold/Italic/BoldItalic);
  -- 图标字体 Nerd Font Symbols;彩色 emoji 走内嵌的 Noto Color Emoji。
  -- 中文仍需系统字体(Microsoft YaHei),体积太大没打包。
  font = {
    -- 主字体:家族名或字体文件路径(TTF/OTF/TTC)
    family = "JetBrains Mono",
    -- 逻辑字号(px @96dpi,随窗口 DPI 缩放)
    size = 18,
    -- 文本回退链:主字体缺字时依次尝试(图标、中文等)
    fallback = { "Nerd Font Symbols", "Microsoft YaHei" },
    -- 行高/格宽倍数(可选)。不写 line_height 时用字体自身行高,
    -- 但限制在 1.0em~1.6em(Nerd Font 的行高常偏大,可写 0.85 收紧)
    -- line_height = 0.95,
    -- cell_width = 1.0,
    -- 四个样式槽(可选)。不写则用同家族的 Bold/Italic face;
    -- 没有对应 face 时自动合成(粗体加描、斜体剪切变形)
    -- bold = "JetBrains Mono Bold",
    -- italic = "JetBrains Mono Italic",
    -- bold_italic = "JetBrains Mono Bold Italic",
    -- Nerd Font 图标(PUA 私用区字形)的来源:
    --   "auto" / true = 自动在系统里找(Symbols/Mono 变体优先)
    --   "MesloLGL Nerd Font Mono" = 指定家族
    --   false = 关闭
    nerd_font = "auto",
    -- 额外的字体搜索目录(可选)
    -- dirs = { "D:/Fonts" },
  },

  -- 旧写法仍兼容:
  -- font = "C:/Windows/Fonts/consola.ttf",
  -- font_fallback = { "C:/Windows/Fonts/msyh.ttc" },
  -- font_size = 18.0,
  padding = 8.0,
  -- 初始窗口大小,两种写法(重启生效):
  --   1) 像素(VectorTerm 扩展,优先级最高):物理像素,不随系统缩放放大;
  --      写死后调 font.size 不会改变窗口大小
  --      window_width = 1300,
  --      window_height = 734,
  --   2) 单元格数(wezterm 同款):按"几列几行"反推像素,改字号会连带改窗口大小
  --      initial_cols = 120,
  --      initial_rows = 32,
  -- 都不写 = 内置 960 x 640 起点。
  -- window_width = 1300,
  -- window_height = 734,
  -- initial_cols = 120,
  -- initial_rows = 32,
  scrollback_lines = 3500,

  -- 用鼠标拖窗口边框时,终端行列数怎么跟着窗口变(支持热重载)。
  -- wezterm 的 WindowEvent::Resized { live_resizing } 一进来就
  -- apply_dimensions(),拖边框时文字位置是实时调整的;这里默认一致。
  --   "realtime" = 默认。每跨过一个字符格就当场同步 PTY,长行实时折行,
  --                画布 / 标签栏 / surface 与行列数始终同一组尺寸,无空档
  --   "debounce" = 拖动中按 resize_debounce_ms 防抖,停下才同步
  --   "freeze"   = 整个拖动过程冻结行列数,松手一次性同步(拖动中右下留白)
  -- 后两种是给 ConPTY + PSReadLine 重绘太跳的机器留的退路;实测默认项下
  -- 拖动中同步 10 次、内容原点位移恒为 (0,0),不会整屏抖。
  live_resize_mode = "realtime",

  -- 「非拖动」场景(程序化改尺寸、最大化 / Snap、Win+方向键)连续 resize
  -- 停下多少毫秒后才同步行列数。鼠标拖动不受这一项影响。
  --   0     = 逐次实时同步(wezterm 行为)
  --   150   = 默认
  --   上限 1000ms;支持热重载,是 VectorTerm 扩展项
  resize_debounce_ms = 150,

  -- wezterm 同名项:窗口缩放步长锁到一个字符格(cell_w x cell_h)。
  -- 对应 wezterm 的 ResizeIncrementCalculator + set_resize_increments,
  -- 拖边框时 WM_SIZE 只在跨过整格时来一次,rows/cols 每次恰好 +-1。
  --   true  = 默认,wezterm 的行为(实测宽度每 12px 一跳,而不是每 4px)
  --   false = 允许逐像素缩放
  use_resize_increments = true,

  -- 渲染后端(仅 Windows)
  --   "cpu"  = 软渲染(softbuffer + GDI BitBlt),**默认**
  --   "gpu"  = GPU 实例化渲染(wgpu / DX12);初始化失败仍会退回 CPU,不会起不来
  --   "auto" = 先试 GPU,拿不到设备再退回软渲染
  --
  -- 默认用 cpu 的原因:GPU 路径在拖动缩放窗口时,新尺寸的交换链还没重建完,
  -- DXGI 会按 DXGI_SCALING_STRETCH 把旧尺寸的后缓冲缩放铺满新客户区,看到
  -- 的效果是文字被横向拉长一下又弹回(拖右边/下边最明显)。
  -- CPU 路径用 BitBlt 做 1:1 像素拷贝,没有"缩放"这个环节,所以不抖。
  -- 详细机制与两条路径的差别见 docs/rendering-backends.md。
  --
  -- 两种后端共享同一份显示列表,字形位置/下划线/光标尾迹的几何完全一致,
  -- 切换后端不会改变任何版式。
  -- 改这一项需要重启(热重载只提示,不切换)。
  render = "cpu",

  -- ===== 自定义背景 =====
  -- 纯色背景(不配就用 colors.background)
  -- window_background_color = "#101418",

  -- 图片背景。相对路径按本文件所在目录解析;png/jpg/webp/bmp/gif(取首帧)
  -- window_background_image = "bg/wallpaper.jpg",

  -- 亮度 / 色相 / 饱和度(1.0 = 原样,只影响图片)
  -- 把 brightness 压到 0.4~0.5 能让文字在花哨壁纸上仍然清楚
  -- window_background_image_hsb = {
  --   brightness = 0.45,
  --   hue = 1.0,
  --   saturation = 0.9,
  -- },

  -- 模糊半径(像素,0 = 不模糊;8~24 是毛玻璃感,再大就糊成色块了)
  -- window_background_blur = 12,

  -- 填充方式:
  --   cover   = 等比铺满并居中裁切(默认,和 wezterm 一致)
  --   contain = 等比缩到整张可见,四周露出底色
  --   stretch = 拉伸铺满,不保持比例
  --   tile    = 按原始像素平铺(小图当纹理用)
  --   center  = 原始像素居中,不缩放
  -- window_background_fill = "cover",

  -- 不透明度 0..1:1 = 不透明,0.85 能隐约透出背后的窗口
  -- 实现方式是"层叠窗口 + 统一 alpha",整块窗口(含文字)一起变淡,
  -- CPU / GPU 两个后端都支持。详见 docs/custom-background.md
  -- window_background_opacity = 0.85,

  -- 渐变(图片存在时画在图片下面,图片半透明时才透得出来)
  -- window_background_gradient = {
  --   colors = { "#0b1020", "#1b2a4a" },
  --   orientation = "Vertical",   -- 或 "Horizontal"
  -- },

  -- Win11 系统背板(毛玻璃):none(默认)/ mica / acrylic / tabbed
  -- 目前只解析不生效:材质要靠"窗口客户区对 DWM 逐像素透明"才透得出来,
  -- 而窗口交换链(CreateSwapChainForHwnd)不接受预乘 alpha。配了会在
  -- 启动日志里提示。半透明请用 window_background_opacity。
  -- win32_system_backdrop = "acrylic",

  -- 双显卡机器上先挑哪块 GPU(改这一项需要重启)
  --   "low"  = 优先核显(默认。终端是常驻窗口,核显省电、发热低)
  --   "high" = 优先独显(4K/高刷屏或核显驱动有问题时用)
  -- 只装了单块显卡时这项没有影响。启动日志会打印实际选中了哪块。
  gpu_power = "low",

  -- 终端图片协议:iTerm2(OSC 1337;File=...)与 Kitty(APC G ...)。
  -- 常见用法:`wezterm imgcat foo.png`、`chafa -f kitty`、`timg -p kitty`、
  -- `lsix`、`catimg -i`,以及 neovim / yazi 里走 Kitty 协议的预览插件。
  images = {
    -- false = 完全不认这两类报文(报文当普通控制序列丢掉,不会错位)
    enable = true,
    -- 解码后位图的缓存上限(MB),按内容哈希 LRU 淘汰。
    -- CPU 侧常驻 RGBA8,GPU 侧另有同等量级的纹理,256 大约留住十几张 1080p 图。
    max_memory_mb = 256,
  },

  -- 窗口装饰:完全沿用 wezterm 的 window_decorations 标志组合。
  -- 默认值 = "TITLE|RESIZE"(用 Windows 自己的标题栏和缩放边框)。
  --   "TITLE|RESIZE"               系统标题栏 + 原生三枚按钮 + 系统缩放边框
  --   "RESIZE"                     去掉系统标题栏,标签栏里也不画按钮,四边缩放由程序自绘
  --   "INTEGRATED_BUTTONS|RESIZE"  去掉系统标题栏,把窗口按钮画进标签栏
  --   "NONE"                       去掉系统标题栏,也不画按钮、不允许缩放
  -- 注:winit 的无装饰窗口非客户区为 0,所以无 TITLE 时 RESIZE 靠自绘热区模拟,
  --    也不会有系统阴影(Win11 圆角由下面的 window_rounded_corners 单独恢复);
  --    只有 "TITLE|RESIZE" 才是原生边框。
  -- wezterm 的 macOS 专有标志(MACOS_*)在 Windows 上忽略;未知标志只提示不报错。
  -- 简写 window_titlebar = true/false 只覆盖"有无系统标题栏"这一维,
  -- 和 window_decorations 一起写时以它为准(其余维度仍取 flags 的结果)。
  -- window_titlebar = false,   -- 例:只去掉系统标题栏(重启生效)
  window_decorations = "TITLE|RESIZE",

  -- 关窗确认:某个标签里还有程序在跑时(不只是 shell 本身),先清屏并在正中
  -- 打一句 "● Really kill this window and all contained tabs and panes?"(句首
  -- 是一个红点) + [Y]es / [N]o。照 wezterm 的规则只认 Y / N / Esc 和鼠标点击
  -- (回车不生效),悬停到选项上反显。默认 true,写 false 直接关窗口。
  -- quit_confirmation = true,

  -- 哪些进程名算"没有程序在跑"(wezterm 的 skip_close_confirmation_for_processes_named)。
  -- 判定照 wezterm 的 LocalPane::can_close_without_prompting:从 shell 出发取整棵
  -- 进程树的 exe 名,全在这个名单里才直接关;只要多出一个(比如 python.exe、
  -- node.exe、vim.exe),关窗前先弹确认。
  -- 想让它安静点就往里加;想严格点就删掉几项。写空表 = 连 shell 自己都算在跑。
  -- skip_close_confirmation_for_processes_named = {
  --   "bash", "sh", "zsh", "fish", "tmux", "nu",
  --   "cmd.exe", "pwsh.exe", "powershell.exe",
  --   "conhost.exe", "openconsole.exe", -- ConPTY 宿主,不是用户跑的程序
  -- },

  -- 会话(shell)自己退出之后怎么办,对齐 wezterm 的 exit_behavior:
  --   "Close"(默认)= 收掉这个标签;关的是最后一个标签就跟着退出窗口
  --   "Hold"        = 留着最后一屏,末尾补一行灰色 [会话结束:...]
  -- 敲 exit、Ctrl+D、程序被 kill 都算退出。
  -- exit_behavior = "Close",

  -- 会话保持(wezterm 式多路复用,VectorTerm 扩展)。开启后所有 shell 由后台
  -- 守护进程(vectorterm --mux-server)托管:关掉窗口/标签只是「断开视图」,
  -- shell 和它正在跑的程序继续存活、输出不丢;下次启动自动 reattach,
  -- 滚动缓冲完整恢复现场。Ctrl+Shift+A 切换回其他未显示的会话。
  -- 默认 false(本地直连,不启动守护进程)。
  -- session_persistence = false,

  -- Win11 无边框窗口圆角(VectorTerm 扩展,wezterm 没有同名项)。
  -- 关掉系统标题栏后 winit 把非客户区压成 0,DWM 默认给方角;
  -- 这里通过 DWMWCP_ROUND 把系统圆角要回来,最大化/全屏时系统自动不圆。
  -- false = 强制硬直角;Windows 10 无圆角能力时本项无效果。重启生效。
  window_rounded_corners = true,

  -- 标签栏里的窗口按钮:顺序即从左到右的绘制顺序。
  -- 取值 'Hide'(=最小化,也接受 'Minimize')/'Maximize'/'Close'。
  -- 空表 {} 或 false = 三枚全隐藏 —— 注意这只关按钮,顶部标签栏仍在,
  -- 照样能拖动窗口、双击最大化,不会把顶部那条状态栏一起去掉。
  -- integrated_title_buttons = { 'Close' },
  -- integrated_title_buttons = { 'Close', 'Maximize', 'Hide' },
  -- integrated_title_buttons = {},
  -- 按钮靠左还是靠右(默认 "Right")
  -- integrated_title_button_alignment = "Right",   -- "Right" | "Left"
  -- 图标配色:"Auto"(默认,跟随标签栏前景色)或任意颜色字符串
  -- integrated_title_button_color = "Auto",
  -- 样式:"Windows"(默认,方形图标)/ "Gnome"(圆形按钮底)
  -- integrated_title_button_style = "Windows",

  -- 状态栏刷新间隔(毫秒,夹到 50~60000);update-status / update-right-status
  -- 按这个节奏派发。不写 = 1000。
  -- status_update_interval = 1000,

  -- 右侧状态栏的外观(详见手册 §8.7)。
  -- 内容由 update-right-status 脚本决定(wezterm.format 里每换一次颜色就是一段);
  -- 这里只调长相:字号 / 整条栏高度 / 左右留白 / 段与段之间的距离。
  -- status_bar = {
  --   enable = true,          -- false = 右侧状态栏整段不画
  --   keep_script = true,     -- 保留脚本 set_right_status 的文字
  --   font_scale = 0.9,       -- 状态栏字号 = 终端字号 × 0.9(只影响状态栏)
  --   height = 34,            -- 整条栏最小高度(物理像素);不写 = 跟随字号
  --   padding = 10,           -- 左右留白(物理像素)
  --   item_spacing = 0,       -- 相邻两段文字之间再空几像素
  --   space_width = 0.5,      -- 一个空格占几字宽:脚本里小点两侧的空隙就是空格,
  --                           -- 调小只是把切片收紧,字不会变窄(1.0 = 和 wezterm 一致)
  --   text = "VectorTerm",        -- 额外拼在前面的一段固定文字(可不写)
  -- },

  -- 标签栏。快捷键:Ctrl+Shift+T 新建、Ctrl+Shift+W 关闭、
  -- Ctrl+Tab / Ctrl+Shift+Tab 切换、Ctrl+1..9 跳到第 n 个;
  -- 鼠标左键切换、中键关闭、点标签右侧的 "×" 只关那一个、点 "+" 新建。
  tab_bar = {
    enable = true,
    -- 放到窗口底部
    at_bottom = false,
    -- true = 标签是带缝隙的独立块;false = 连成一片、用分隔线
    fancy = true,
    -- 只有一个标签时隐藏(隐藏后内容区会多一行)
    hide_if_one = false,
    -- 单个标签最大宽度(单位:单元格),与 wezterm 的 tab_max_width 一致
    max_width = 16,
    -- 显示 "1:" 这样的序号
    show_index = true,
    -- 显示末尾的 "+" 新建按钮
    show_new_tab_button = true,
    -- 每个标签右侧的 "×" 关闭按钮。标签被挤到放不下标题时会自动隐藏
    show_close_button = true,
    -- 标签圆角半径(像素)。不写 = 按标签栏高度自动(约 8px);
    -- 0 = 直角;太大时自动夹到标签栏高度的一半以内。仅 fancy = true 生效
    -- corner_radius = 8,
    -- 无边框 + Win11 圆角时,标签栏四周默认各留 10 逻辑像素(按 DPI 放大):
    -- 标签不会被窗口圆角裁掉,到上边框和到左右边框的距离也一致。
    -- 有系统标题栏时上沿是直的,默认只留 4 逻辑像素,更贴近标题栏。
    -- 想改距离就写数值(逻辑像素),写 0 = 贴边。
    -- 顶层别名:tab_bar_edge_padding / tab_bar_top_padding
    -- edge_padding = 10,   -- 左右安全区
    -- top_padding = 0,     -- 上/下安全区(底部栏时是到下边框的距离)
    colors = {
      background = "#0c0c0c",
      active_tab = { bg = "#2f2f2f", fg = "#ffffff" },
      inactive_tab = { bg = "#161616", fg = "#a0a0a0" },
      inactive_tab_hover = { bg = "#262626", fg = "#e6e6e6" },
      new_tab = { bg = "#0c0c0c", fg = "#a0a0a0" },
      new_tab_hover = { bg = "#2f2f2f", fg = "#ffffff" },
      -- 非活动标签之间的分隔线(仅 fancy = false 时画)
      inactive_tab_edge = "#2a2a2a",
    },
  },

  -- 分屏中间那根分割线(详见 docs/config-manual.md §15.8)
  -- 分割线永远在每次分割的几何中线上,两侧 pane 分到的宽度严格相等。
  -- 整段不写 = 自动间距 + 3px #323236 细线。
  -- split_divider = {
  --   enabled = true,
  --   -- 线宽(逻辑像素)。不写 = 自动 3px 细线;0 = 隐藏;可以大于缝宽
  --   thickness = 3,
  --   -- 文字 / 光标到分割线的距离(详见 §15.8.6):
  --   -- gap 是两侧文字之间的总缝宽,text_gap 是单侧文字到线边缘的距离。
  --   -- 都不写 = Auto:线宽 + 2px(文字到线只留 1px),不随字号变宽。
  --   -- 配置管理器「分屏与分割线」分组顶部有同名的距离滑杆,拖一下就行。
  --   -- gap = "Auto",
  --   -- text_gap = 6,     -- 0 = 贴着线,4~8 是常用档位
  --   -- 两端到内容区边界的距离,也就是线的长度:
  --   --   竖线:inset_start = 离上边、inset_end = 离下边
  --   --   横线:inset_start = 离左边、inset_end = 离右边
  --   inset_start = 24,
  --   inset_end = 24,
  --   -- inset = 24,         -- 两端相同的简写(会被上面两项覆盖)
  --   -- 颜色支持单色:
  --   color = "#7aa2f7",
  --   -- 也支持渐变(二色到多色,配了就覆盖 color):
  --   -- gradient = {
  --   --   colors = { "#ff5f6d", "#ffc371", "#45b7d1" },
  --   --   -- Auto(默认)= 沿线方向:竖线上下渐变、横线左右渐变
  --   --   -- Vertical / Horizontal 可强制方向
  --   --   orientation = "Auto",
  --   -- },
  --   opacity = 1.0,
  --   -- 分割栏拖动 / 快捷键调整窗格大小(详见 §15.8.7):
  --   draggable = true,       -- 鼠标拖分割线改两侧占比;关掉后快捷键仍可用
  --   hit_width = 8,          -- 鼠标热区半宽(px)
  --   min_ratio = 0.08,       -- 任一侧最小占比
  --   step = 0.05,            -- Ctrl+Shift+Alt+方向键每次移动多少占比
  -- },

  -- 分屏焦点(详见 docs/config-manual.md §15.9)
  -- 鼠标点进哪个 pane 就聚焦哪个;Alt+1..9 可直接跳到第 N 个 pane。
  -- 只有聚焦 pane 显示输入光标;切换时这个光标沿直线滑到新 pane 的光标处,
  -- 身后拖着和窗格内一样的 kitty 光标尾迹(同一套积分器,长度/缓急看 cursor.trail_*)。
  -- pane_focus = {
  --   click_to_focus = true,
  --   hide_inactive_caret = true,
  --   number_shortcuts = true,
  --   trail = true,
  --   sync_trail = true,          -- 速度 / 尾迹颜色 / 透明度沿用 cursor.trail_*
  --   trail_ms = 320,
  --   -- trail_color = "#ffcc00", -- 仅 sync_trail = false 时生效
  --   landing = "caret",          -- 已废弃:仍解析但不生效,旧配置留着不报错
  -- },

  -- 文件浏览器(详见 docs/config-manual.md §15.10)
  -- 命令面板 /files(Ctrl+Shift+P 后输 files,或搜索框输 / 再输 files)把当前
  -- pane 整个变成文件列表(路径 = 该 pane 的 cwd,分屏时只盖这一边);
  -- Ctrl+Shift+E 从左边分出一条侧边栏。旧的 shell 内 `files-gui` 命令已移除。
  -- 回车打开文件 = 用下面的编辑器另开一个窗口(默认),不动当前窗口的分屏和 shell。
  -- files_gui = {
  --   -- "window"  = 单独新开一个 VectorTerm 窗口(默认)
  --   -- "split"   = 在当前窗口向右分一个 pane
  --   -- "current" = 把命令写进当前 pane 的 shell
  --   open_mode = "window",
  --   editor = { "nvim" },   -- 或 { "code", "-g" };不写 = 自动探测 nvim → vim → 记事本
  --   side_cols = 30,        -- 侧边栏宽度(列),窄于 24 列就不再显示文件尺寸
  --   show_hidden = false,   -- 打开时是否列出隐藏文件(面板里按 . 随时切换)
  --   follow_cwd = true,     -- 终端 cd 之后面板跟着换目录(PowerShell 会注入 prompt 钩子)
  --   push_cwd = true,       -- 面板进目录 / 回上级时把 cd 写进终端
  -- },

  -- 鼠标文本选择与复制
  selection = {
    -- 松开鼠标就把选区写进系统剪贴板(PuTTY / Windows Terminal 风格)
    copy_on_select = true,
    -- 拖选到窗口上/下边缘时自动翻回看历史并继续扩选(见手册 §11.4)
    edge_scroll = true,
    -- 选区底色与不透明度(高亮画在文字下面,不会染色字形)
    background = "#3a6ea5",
    opacity = 0.45,
    -- Ctrl+Shift+V 的语义:
    --   "paste"(默认)= 粘贴;Ctrl+Shift+C 复制选区
    --   "copy"       = 复制选区;Ctrl+Shift+C 改为粘贴
    ctrl_shift_v = "paste",
  },

  colors = {
    foreground = "#cccccc",
    background = "#0c0c0c",
    cursor_fg  = "#0c0c0c",
    cursor_bg  = "#ffffff",   -- 白色光标
    -- 覆盖基础 16 色(可选):顺序 black red green yellow blue magenta cyan white,
    -- 之后是各自的亮色版本。子进程已带 TERM=xterm-256color / COLORTERM=truecolor。
    -- ansi = { "#0c0c0c", "#c50f1f", "#13a10e", "#c19c00",
    --          "#0037da", "#881798", "#3a96dd", "#cccccc" },
    -- 亮色版本(可选):接在 ansi 之后,覆盖 8~15 号色
    -- brights = { "#767676", "#e74856", "#16c60c", "#f9f1a5",
    --             "#3b78ff", "#b4009e", "#61d6d6", "#f2f2f2" },
    -- 覆盖 256 色表任意下标(可选):SGR 38;5;n / 48;5;n 用的就是这张表
    -- indexed = { [196] = "#ff0000" },
  },

  -- 光标尾迹(移植自 kitty/cursor_trail.c:半透明拉伸四边形,不是渐隐方块串)
  cursor = {
    -- 光标在某个位置静止多少毫秒后,尾迹才开始跟随;0 = 关闭。
    trail = 20,
    -- 每轴最小位移(格):0 = 每次移动都拖尾;2 = kitty 默认(单格移动不拖尾)
    trail_start_threshold = 0,
    -- (最快, 最慢)衰减时间(秒),越大尾巴越长。连续输入时光标尾迹会自动加速,
    -- 松开按键后再平滑回落到这个手感。
    trail_decay = "0.1 0.4",
    -- 单段尾迹动画的硬上限(毫秒;0 = 不限制)。到点直接吸附到最新位置,
    -- 所以快速连按回车不会攒出一串要逐个播完的旧动画。
    trail_max = 220,
    -- 尾迹颜色;none = 跟随光标颜色
    trail_color = "none",

    -- ── 光标本体:形状 / 尺寸 / 闪烁 / 动画(全部可热重载)────────────
    -- 下面这些全都不写 = 旧版行为:整格方块、不闪、不动。
    -- shape = "bar",              -- auto(跟终端 DECSCUSR) / block / bar / underline
    -- width = 1.0,                -- 相对默认宽度的倍数,1 = 整格
    -- height = 1.0,               -- 相对默认高度的倍数;bar/underline 是粗细倍率
    -- align_x = "center",         -- left / center / right,也可写 0..1 的数字
    -- align_y = "bottom",         -- top / center / bottom;不写:方块贴顶、下划线贴底
    -- thickness = 3,              -- bar 宽 / underline 高(px);0 = 按格子自动
    -- alpha = 0.9,                -- 光标本体不透明度
    -- blink = true,               -- auto / true / false
    -- blink_rate = 800,           -- 一个完整周期的毫秒数;SGR 5/6 文字闪烁共用
    -- blink_duty = 0.5,           -- 一个周期里「亮」占多少
    -- blink_style = "fade",       -- hidden(整块消失) / fade(淡出) / outline(空心描边)
    -- anim = "pulse",             -- none / pulse(尺寸脉冲) / breathe(透明度呼吸)
    -- anim_ms = 1200,             -- 常驻动画周期
    -- anim_amount = 0.25,         -- 幅度:脉冲的缩放比例 / 呼吸的最低折损
  },

  -- 回看滚动(scrollback)动画
  scroll = {
    -- 一个滚轮档位(3 行)的平滑动画时长(毫秒);0 = 关闭,恢复逐行瞬移。
    -- 从静止开始时用 ease-in-out(起步慢、中段快、收尾慢);
    -- 连续滚轮时用 ease-out 接着已有速度,不会每档重新加速。
    anim = 90,
    -- 单段动画的硬上限(毫秒):连发再密也不会攒出一段要播很久的动画
    max = 200,
    -- 按键/粘贴后回到底部的时长(毫秒)
    back = 90,
    -- 长按连续输入(回车连发)时回到底部的时长(毫秒):比 back 更短,
    -- 让视口赶在下一批输出之前就位,剩下的位移交给光标尾迹自己播完
    back_burst = 40,
  },

  -- 右侧可视化滚动条(详见 docs/config-manual.md §11.4)
  -- 每个 pane 一条,拖滑块直接跳到对应历史位置,点轨道翻一页;
  -- 不占终端列,不影响 PTY 尺寸。scrollbar = false 可整体关掉。
  scrollbar = {
    enabled = true,
    -- 显示策略:always 一直画 / auto 贴底自动隐藏 / never 完全不画
    -- (auto_hide 是旧字段,写了 show 时以 show 为准)
    show = "auto",
    side = "right",        -- 停靠侧:right / left;分屏时左 pane 可改 left 避开分割线
    show_track = true,     -- 关掉后只画滑块
    width = 9,             -- 轨道宽度(px)
    min_thumb = 28,        -- 滑块最小高度(px)
    auto_hide = true,
    opacity = 0.85,
    -- track_color = "#333840",  -- 不写 = 由前景/背景色自动混合
    -- thumb_color = "#7aa2f7",
    -- hover_color = "#ffffff",  -- 鼠标悬停 / 拖动滑块时的颜色
  },

  -- 历史搜索 / 命令面板(详见 docs/config-manual.md §11.5–11.6)
  -- Ctrl+Shift+F 打开,在全部 scrollback 里找;Enter 下一个、Tab 切大小写、Esc 关闭。
  -- 在搜索框里输入第一个字符 / 会切到命令面板;Ctrl+Shift+P 可直接打开。
  -- 命令:files(铺满当前窗格的文件管理器)、settings(配置管理器)、help、key、reload、search。
  search = {
    enabled = true,
    case_sensitive = false,
    highlight_color = "#f0c674",
    current_color = "#ff8f3f",
    max_matches = 5000,
  },

  -- 键位映射(mlua 脚本):值为内置动作字符串,或 Lua 函数。
  -- 组合名大小写/顺序无关,支持:ctrl alt shift super + 字母/数字/命名键
  -- (enter escape tab space backspace delete insert up down left right
  --  home end pageup pagedown f1..f12)。
  keys = {
    ["ctrl+shift+r"] = "reload",        -- 重载配置
    ["ctrl+shift+q"] = "quit",          -- 退出
    ["ctrl+shift+v"] = "paste",         -- 粘贴(与 Ctrl+V 相同)
    ["f12"]          = "send:dir\r",    -- 发送字面量(注意 \r 才会执行)

    -- Lua 回调:通过全局表 VectorTerm 调用动作
    ["ctrl+alt+t"] = function()
      VectorTerm.notify("来自 Lua 的问候")
      -- 可用的运行时动作:
      --   VectorTerm.notify(msg)            改窗口标题提示
      --   VectorTerm.send(text)             向 shell 发送文本
      --   VectorTerm.paste()                粘贴剪贴板
      --   VectorTerm.reload()               重载配置
      --   VectorTerm.quit()                 退出
      --   VectorTerm.cursor_color("#rrggbb") 改光标颜色
      --   VectorTerm.trail_dwell(ms)        尾迹静止阈值
      --   VectorTerm.trail_threshold(x [,y]) 每轴格数阈值
      --   VectorTerm.trail_decay(fast [,slow]) 衰减时间(秒)
      --   VectorTerm.trail_max(ms)          单段动画硬上限(0 = 不限制)
      --   VectorTerm.trail_color("#rrggbb" 或 nil)
      VectorTerm.trail_decay(0.15, 0.8)     -- 让尾巴更长
    end,
  },

  -- ===== wezterm 配置兼容 =====
  -- 可以直接把 wezterm 的配置喂给 VectorTerm:
  --   VectorTerm.exe --config C:/Users/you/.wezterm.lua
  -- `require 'wezterm'`、`wezterm.config_builder()`、`wezterm.font_with_fallback`
  -- 等构造配置用的 API 都已实现。
  --
  -- 生效的字段:default_prog / font / font_size / line_height / window_padding /
  --   initial_cols / initial_rows / colors(含 ansi、brights、selection_bg、
  --   tab_bar)/ enable_tab_bar / tab_bar_at_bottom / use_fancy_tab_bar /
  --   hide_tab_bar_if_only_one_tab / tab_max_width / show_tab_index_in_tab_bar /
  --   show_new_tab_button_in_tab_bar / set_environment_variables /
  --   keys(数组格式 + wezterm.action.*)。
  -- 暂不生效:window_frame、enable_scroll_bar、default_cursor_style /
  --   cursor_blink_rate / animation_fps(这几个是 wezterm 原生顶层键;自定义
  --   光标闪烁请写上面的 cursor.blink / cursor.blink_rate)、wezterm.on(...)
  --   注册的状态栏回调。
  -- 全部配置项逐条说明见 docs/config-manual.md。
}
