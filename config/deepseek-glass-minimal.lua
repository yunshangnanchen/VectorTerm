--[====================================================================[
  VectorTerm 内置主题:DeepSeek Glass · 极简(只剩终端)

  deepseek-glass.lua 的"减法版":同一套夜空蓝 + 真毛玻璃,但把所有非终端的
  东西全部拿掉 —— 没有标签栏、没有窗口按钮、没有标题栏,窗口里从上到下
  只有滚动内容。透明背景下这一整块玻璃就是终端的形状。

  ── 去掉了什么 ────────────────────────────────────────────────────
  - `window_decorations = "RESIZE"`:不要 TITLE(系统标题栏),也不要
    INTEGRATED_BUTTONS(标签栏里的小三角/方块/叉)。四边四角仍可拖拽缩放。
  - `tab_bar = { enable = false, keep_as_title_bar = false }`:
    无边框模式下这条栏默认"兼任标题栏",关掉标签栏也会留一条空白条
    (否则窗口没地方可拖)。`keep_as_title_bar = false` 才是真的不留 ——
    高度归零,终端直接顶到玻璃上沿。

  ── 没有标题栏以后怎么操作 ────────────────────────────────────────
  - 移动窗口:按住 **Super(Win)+ 左键拖动**,在哪都行,和浏览器里
    "按住 Win 拖"一个手感;双击同一处仍是最大化 / 还原。
  - 缩放窗口:拖窗口最外圈几像素(无边框模式自绘的缩放热区)。
  - 最大化 / 还原:双击 + 拖动,或 Win+↑ / Win+↓。
  - 关窗口:Alt+F4(和任何窗口一样)。
  - 标签:快捷键照旧(`Ctrl+Shift+T` 新建等,见 keys.lua),只是不占可见空间。

  ── 失焦白边(focus_border)──────────────────────────────────────
  没有标题栏之后,系统那点"激活窗口的边框高光"也没了 —— 光看内容分不出
  键盘在谁那儿,打了一半的字可能全进了别处。这里自绘一圈极细的白边:

    - 只在**失去焦点**时出现,拿回焦点就消失;
    - 淡入 220ms(ease-out),淡出 140ms —— 直接"啪"地出现太抢眼,
      淡进来才像玻璃的一部分;收回去要快,回来的时候得立刻有回应;
    - 线宽 1 逻辑像素,圆角 8 与窗口圆角对齐(最大化时自动变直角);
    - `width = 0` 或 `enabled = false` 就是不要它。

  ── 玻璃那一层(与 deepseek-glass 完全相同)──────────────────────
  Acrylic 材质在窗口背后模糊;`window_background_opacity = 0.55` 只压画布
  (清屏色 + 渐变 + 默认底色,叠几层也只透一次),文字/光标/图片仍是实心;
  单元格自设底色(oh-my-posh 提示符、反选、ls 目录色)由
  `text_background_opacity` 管,默认 1.0 保持原色。

  这需要 GPU(DirectComposition 合成交换链);没有 GPU 的机器会退回软渲染,
  窗口保持不透明(而不是把文字一起压淡),日志里会说明。

  每一项的含义见 docs/config-manual.md,透明度见 docs/custom-background.md。
--]====================================================================]

return {
  -- ── 玻璃:底色 + 渐变 + 整窗不透明度 ────────────────────────────────
  window_background_color = "#0D1826",
  -- 上亮下暗的竖向渐变 = 玻璃上沿的反光
  window_background_gradient = {
    colors = { "#12203A", "#0A1120" },
    orientation = "Vertical",
  },
  -- 画布 55% 不透明:背后的 Acrylic 材质透出 45%。文字不受影响。
  window_background_opacity = 0.55,
  -- 单元格自设底色保持原色,否则提示符色块会被玻璃冲淡(两端是前景字形,
  -- 不跟着透,一对比就像渲染坏了)。
  text_background_opacity = 1.0,
  -- DWM 材质:Acrylic = 实时模糊窗口后方
  win32_system_backdrop = "Acrylic",

  -- ── 基础配色(§6.1):DARK 色板,前景按透明度提亮 ──────────────────
  colors = {
    foreground = "#F2F6FF",
    background = "#0B1220",
    cursor_fg  = "#0B1220",
    cursor_bg  = "#5B7CFF",

    ansi = {
      "#16233A",  -- 0 black(比画布亮一档,glass 下才分得出来)
      "#F0564A",  -- 1 red
      "#3DDC97",  -- 2 green
      "#F5C451",  -- 3 yellow
      "#5B7CFF",  -- 4 blue    → 品牌蓝
      "#A78BFA",  -- 5 magenta
      "#4DD0E1",  -- 6 cyan
      "#C9D3E8",  -- 7 white
    },
    brights = {
      "#33415F",  --  8 bright black
      "#FF7B6B",  --  9 bright red
      "#6BE8B4",  -- 10 bright green
      "#FFD97A",  -- 11 bright yellow
      "#8FA8FF",  -- 12 bright blue
      "#C4B5FD",  -- 13 bright magenta
      "#7BE3F2",  -- 14 bright cyan
      "#FFFFFF",  -- 15 bright white
    },
  },

  -- ── 极简:标签栏整条都不留 ────────────────────────────────────────
  -- enable = false 只是"不画标签";无边框模式下它还要兼任标题栏,所以
  -- 必须再加 keep_as_title_bar = false,高度才真的归零。
  tab_bar = {
    enable              = false,
    keep_as_title_bar   = false,
    show_new_tab_button = false,
    show_close_button   = false,
  },

  -- ── 失焦白边:细、淡、只在没焦点的时候在 ──────────────────────────
  focus_border = {
    enabled     = true,
    width       = 1.0,      -- 逻辑像素;想更明显就 1.5,超过 2 就不像"细边"了
    radius      = 8.0,      -- 跟 DWM 窗口圆角对齐,最大化时自动变直角
    color       = "#FFFFFF",
    opacity     = 0.9,      -- 淡入完成后的不透明度:压在玻璃上是一道冷白细线
    fade_in_ms  = 220,      -- 缓入。0 = 瞬间出现
    fade_out_ms = 140,      -- 拿回焦点时收回去,比淡入快
  },

  -- ── 选区(§6.3):半透明品牌蓝,选中时底下的渐变还透得出来 ──────────
  selection = {
    copy_on_select = true,
    edge_scroll    = true,
    background     = "#5B7CFF",
    opacity        = 0.30,
  },

  -- ── 输入光标与尾迹(§9):尾迹延长,在透明底上拖影更明显 ────────────
  cursor = {
    trail                 = 24,
    trail_start_threshold = 0,
    trail_decay           = "0.1 0.4",
    trail_max             = 220,
    trail_color           = "#6D87FF",
    blink = "auto",
  },

  -- 切换分屏焦点时的输入光标飞行动画(§15.9)
  pane_focus = {
    sync_trail = true,
    trail_ms   = 320,
  },

  -- ── 分屏分割线(§15.8):两端压暗、中间品牌蓝 ──────────────────────
  split_divider = {
    enabled   = true,
    color     = "#3B4A72",
    gradient  = {
      colors      = { "#16223A", "#5B7CFF", "#16223A" },
      orientation = "Auto",
    },
    draggable = true,
  },

  -- ── 历史搜索(§11.5)─────────────────────────────────────────────
  search = {
    enabled         = true,
    case_sensitive  = false,
    highlight_color = "#1E2A4A",
    current_color   = "#5B7CFF",
  },

  -- ── 滚动条:极简,不要 ───────────────────────────────────────────
  scrollbar = {
    enabled     = false,
    show        = "never",
    thumb_color = "#28324B",
  },

  -- ── 字体与留白 ───────────────────────────────────────────────────
  font = { size = 14, line_height = 1, cell_width = 1 },
  -- 没有标签栏以后整块窗口都是内容,留白就是玻璃四周那一圈"边框"。
  -- 想更满就调到 6,想更松就到 14。
  padding = 10,

  -- ── 窗口:无标题栏、无按钮、有圆角 ───────────────────────────────
  -- 只有 RESIZE:没有 TITLE(系统标题栏),没有 INTEGRATED_BUTTONS(自绘按钮)。
  -- 四边四角的缩放热区仍由程序接管,拖动靠 Super + 左键。
  window_decorations    = "RESIZE",
  window_rounded_corners = true,
  window_width  = 1250,
  window_height = 750,

  -- ── 常用运行项 ───────────────────────────────────────────────────
  shell = "",
  shell_args = { "-NoLogo" },
  -- 毛玻璃要 GPU(合成交换链);没有 GPU 时自动退回不透明的软渲染
  render = "auto",

  keys_file = "keys.lua",
  exit_behavior = "Close",
}
