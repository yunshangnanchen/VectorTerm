-- VectorTerm 默认配置(程序自带)
--
-- 终端启动时按下面的顺序找配置,只加载**一份**:
--   1. --config <路径>                      显式指定
--   2. %USERPROFILE%\.config\VectorTerm\current\ 里唯一的 .lua   ← 你在配置管理器里
--      「设为当前配置」的那份(文件名随意;目录里有多份会报错并退回这里)
--   3. 本文件(程序目录 config\config.lua)
--
-- 也就是说:配置文件本身就是一份完整的配置,**不再通过 config_files 引用别的
-- lua**。想自己改,就在配置管理器里选一份 →「设为当前配置」,它会被复制到
-- 用户目录的 current\ 下,之后改什么都落在生效的那份上。
--
-- 没写到的键一律用程序内置默认值,所以这里只列常用项。
-- 完整可选项见 config.example.lua;逐项说明(类型 / 默认值 / 取值范围)见
-- docs/config-manual.md。
return {
  -- ── 启动窗口尺寸(物理像素,可选) ──────────────────────────────────
  -- 单位是客户区真实像素,不受系统缩放(125% / 150%)影响:写 1300 就是 1300 像素
  -- 宽。写了这一对之后,调 font.size 不会再改变窗口大小。详见手册 §7.2。
  -- window_width = 1300,
  -- window_height = 734,

  -- 默认 shell:PowerShell 7(pwsh)。它通常装在 WindowsApps 里,
  -- 靠 PATH 上的应用执行别名 pwsh.exe 解析,升级后不用改路径。
  -- 没装 PS7 的机器会自动降级 powershell.exe → cmd.exe,不会起不来。
  -- 想换回系统自带 Windows PowerShell 就写 "powershell.exe",
  -- 用 cmd 写 "cmd.exe"。
  shell = "pwsh.exe",
  -- -NoLogo 关掉 PS7 的启动 banner(终端里通常不需要)
  shell_args = { "-NoLogo" },

  -- 渲染后端:cpu(默认,软渲染 + GDI BitBlt,拖动缩放不抖)
  --           gpu(强制 GPU,更快但拖动窗口时会看到文字被拉伸)
  --           auto(先试 GPU,拿不到设备退回软渲染)
  -- 改这一项要重启;启动时 stderr 会打印实际用的是哪个。
  render = "cpu",

  -- 双显卡机器上优先核显(low,默认、省电)还是独显(high)
  gpu_power = "low",

  -- 终端图片协议(iTerm2 的 OSC 1337;File=... 与 Kitty 的 APC G):
  -- enable = false 时不认这两类报文,按普通控制序列丢弃。
  -- max_memory_mb 是解码后位图的缓存上限(CPU 与 GPU 各自按它控制)。
  images = { enable = true, max_memory_mb = 256 },

  -- ── 窗口装饰 ───────────────────────────────────────────────────────
  -- 直接沿用 wezterm 的 window_decorations 标志组合(可任意用 | 组合)。
  --   "TITLE|RESIZE"               系统标题栏 + 原生三枚按钮 + 系统缩放边框(VectorTerm 默认)
  --   "RESIZE"                     只去掉 Windows 顶部标题栏,四边缩放由程序自绘
  --   "INTEGRATED_BUTTONS|RESIZE"  去掉标题栏,把三枚按钮画进标签栏(无边框观感)
  --   "NONE"                       去掉标题栏,也不画按钮、不允许缩放
  -- 简写:window_titlebar = false 只关掉"有没有 Windows 顶部标题栏"这一维。
  -- 想用自绘边框拖拽缩放,别忘了同时打开 window_resize = true。
  window_decorations = "TITLE|RESIZE",
  -- Win11 无边框窗口的圆角:去掉系统标题栏后默认由 DWM 画圆角。
  -- 想要硬直角就改成 false(只对 Win11 有效,重启生效)。
  window_rounded_corners = true,

  -- 鼠标拖边框时终端行列数怎么跟随窗口(支持热重载)。
  --   "realtime" = 默认,wezterm 行为:每跨一格当场同步,文字实时重排
  --   "debounce" = 拖动中按下面的 resize_debounce_ms 合并
  --   "freeze"   = 拖动中冻结行列数,松手一次性同步(右下角会留白)
  live_resize_mode = "realtime",
  -- 仅「非拖动」resize(程序化改尺寸 / 最大化 / Snap / Win+方向键)的
  -- 稳定窗口(ms):停下这么久后才同步 PTY 行列数。0 = 实时,上限 1000。
  resize_debounce_ms = 150,
  -- wezterm 同名项:窗口按 cell_w x cell_h 整格步进缩放,拖动时 WM_SIZE 稀疏化。
  -- false = 允许逐像素缩放。支持热重载。
  use_resize_increments = true,

  -- ── 标签栏(默认开启) ──────────────────────────────────────────────
  -- Ctrl+Shift+T 新建、Ctrl+Shift+W 关闭、Ctrl+Tab / Ctrl+Shift+Tab 切换、
  -- Ctrl+1..9 跳转;左键切换、中键关闭,每个标签右侧的 "×" 只关那一个。
  -- 关掉它就是 enable = false(状态栏画在这条栏上,会一起隐藏)。
  tab_bar = {
    enable = true,
    at_bottom = false,
    max_width = 16,
    show_close_button = true,
    -- 标签栏到窗口上沿的距离(逻辑像素)。不写 = 自动:有系统标题栏时 4,
    -- 无边框时 10(躲开 Win11 圆角)。
    -- top_padding = 0,
  },

  -- 鼠标框选:松开即复制;Ctrl+Shift+C 复制、Ctrl+Shift+V 粘贴
  -- (把 ctrl_shift_v 改成 "copy" 可让 V 变成复制键)
  selection = { copy_on_select = true, background = "#3a6ea5", opacity = 0.45 },

  -- 键位方案文件(可选)。终端会先按本文件所在目录找,再到
  -- %USERPROFILE%\.config\VectorTerm\keys\ 里找,所以在配置管理器里挂的键位文件
  -- 复制配置后也不会失效。没列出的组合键照常退回内置默认键位。
  -- keys_file = "keys.lua",

  -- 自定义背景(全部可选,注释掉就是默认纯色)
  -- 纯色:window_background_color = "#101418"
  -- 壁纸:window_background_image = "bg/wallpaper.jpg"(相对本文件所在目录)
  --   配 window_background_image_hsb = { brightness = 0.45 } 压暗
  --   配 window_background_blur = 12 做磨砂
  -- 半透明:window_background_opacity = 0.85(整窗统一 alpha,CPU/GPU 都支持)

  -- 调试用:把 shell 的原始输出(含转义序列)落盘,排查显示问题很方便
  -- session_log = "target/session.log",
}
