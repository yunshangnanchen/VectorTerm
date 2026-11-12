--[====================================================================[
  VectorTerm 内置主题:ChatGPT Dark(纯灰阶)

  结构和 deepseek.lua 完全一样 —— 只有颜色不同,键位、窗口尺寸、状态栏
  参数(0.9 字号 / 0 片段间距 / 0.2 空格宽)全部照搬,切换主题时不会
  出现"换了配色窗口大小也变了"的情况。

  ChatGPT 深色界面本身没有彩色:主聊天区是近黑,侧栏更深一档,分隔和
  悬停全靠几级灰,主文本近白。所以这套主题**界面装饰全部是灰阶** ——
  没有品牌绿,活动标签是白底黑字(整份主题里对比最强的一块),光标是
  实心白块,分割线中段一道白,选区是中性灰。

  色阶(从深到浅):
    #171717  侧栏 / 标签栏底    最深一档
    #1A1A1A  ANSI 黑            代码块底色
    #212121  画布 / 窗口底       主聊天区
    #2A2A2A  非活动标签
    #3A3A3A  悬停 / 选区 / 滚动条
    #3D3D3D  分割线 / 标签边缘
    #7A7A7A  状态栏次要信息(日期、分隔点)
    #9B9B9B  新标签按钮、非活动标签文字
    #C5C5C5  次级文本
    #ECECEC  主文本 / 活动标签底 / 分割线中段
    #FFFFFF  光标、低电量提醒、bright white

  ANSI 16 色保留功能辨识(git diff、ls、编译器输出要靠色相区分),但整体
  压到很低的饱和度,近灰的冷暖差,不会把界面染成彩色。想彻底变纯灰,
  把 ansi/brights 换成几级灰即可。

  ── 怎么用 ──────────────────────────────────────────────────────────
  1) 推荐:配置管理器左栏选中本文件 → 右键「设为当前配置」
     它会被复制到用户目录的 .config\VectorTerm\current\,终端下次启动就整份
     加载它(配色 + 状态栏 + 窗口设置都在这里)。
  2) 临时试:VectorTerm.exe --config "<程序目录>\config\chatgpt.lua"
  3) 当主题底表:在一份配置里写 theme = "chatgpt",再覆盖少量键。

  每一项的含义见 docs/config-manual.md。
--]====================================================================]

-- ── 状态栏(§12):标签栏右侧的问候语 / 时钟 / 日期 / 电量 / 签名 ─────────
local nf = wezterm.nerdfonts
local THEME = 'dark'          -- 'dark' | 'light',只影响状态栏文字的明暗
local MONO_TEXT = '#ECECEC'   -- 主文本(与 colors.foreground 同色)
local MONO_DIM = '#7A7A7A'    -- 次要信息 / 分隔点
local MONO_SIG = '#C5C5C5'    -- 签名,比正文淡一档

-- 每个时段一套:问候语 + 图标
local function slot(h)
  if h < 5 then
    return '夜深了', nf.md_weather_night
  elseif h < 9 then
    return '早安', nf.md_white_balance_sunny
  elseif h < 12 then
    return '上午好', nf.md_weather_sunny
  elseif h < 14 then
    return '午安', nf.md_food
  elseif h < 18 then
    return '下午好', nf.md_weather_partly_cloudy
  elseif h < 23 then
    return '晚上好', nf.md_weather_sunset
  end
  return '夜深了', nf.md_sleep
end

wezterm.on('update-right-status', function(window, pane)
  local hh = tonumber(wezterm.strftime('%H')) or 0
  local greeting, icon = slot(hh)
  local clock = wezterm.strftime('%H:%M')
  local day = wezterm.strftime('%m/%d')
  local week = ({ '日', '一', '二', '三', '四', '五', '六' })
      [tonumber(wezterm.strftime('%w')) + 1]

  -- 状态栏文字颜色跟着主题走,亮色模式下整体反过来
  local is_light = THEME == 'light'
  local dim    = is_light and '#8E8E8E' or MONO_DIM
  local plain  = is_light and '#212121' or MONO_TEXT
  local accent = is_light and '#212121' or MONO_TEXT   -- 问候语 = 主文本色
  local sig    = is_light and '#555555' or MONO_SIG

  local items = {}
  local function put(color, text)
    if color then
      table.insert(items, { Foreground = { Color = color } })
    end
    table.insert(items, { Text = text })
  end

  -- 两个空格 = 4px:空格被 `space_width` 收到 1/5 字宽,一个会贴住图标
  put(accent, '  ' .. icon .. '  ' .. greeting)
  put(dim, '   ·   ')
  put(plain, clock)
  put(dim, '  ' .. day .. ' 周' .. week .. '  ')

  -- 电量:只靠灰阶层次提示。满电 = 主文本色,越少越淡,低电反而最亮,
  -- 用"最白"当警告。台式机没有电池时循环不进,这一段自动跳过。
  for _, b in ipairs(wezterm.battery_info()) do
    local pct = math.floor(b.state_of_charge * 100 + 0.5)
    local bicon, bcolor
    if b.state == 'Charging' then
      bicon, bcolor = nf.md_battery_charging, plain
    elseif pct >= 80 then
      bicon, bcolor = nf.md_battery, plain
    elseif pct >= 60 then
      bicon, bcolor = nf.md_battery_60, '#C5C5C5'
    elseif pct >= 40 then
      bicon, bcolor = nf.md_battery_40, '#9B9B9B'
    elseif pct >= 20 then
      bicon, bcolor = nf.md_battery_20, '#7A7A7A'
    else
      bicon, bcolor = nf.md_battery_alert, '#FFFFFF'
    end
    put(dim, ' · ')
    put(bcolor, '  ' .. bicon .. '  ' .. pct .. '%')
  end

  -- 签名保留
  put(dim, '  ·  ')
  put(sig, 'Elaina-SakiOVO')
  put(nil, '  ')

  window:set_right_status(wezterm.format(items))
end)

return {
  -- 窗口底色与终端底色统一成画布色,标签栏/留白处不会露出另一种黑。
  window_background_color = "#212121",

  -- ── 基础配色(§6.1) ────────────────────────────────────────────────
  colors = {
    foreground = "#ECECEC",   -- 主文本
    background = "#212121",   -- 画布
    cursor_fg  = "#171717",   -- 白色块状光标上的字
    cursor_bg  = "#FFFFFF",   -- 实心白光标:整套主题唯一的纯白实心块

    -- 16 色表(§6.2):8 基础 + 8 亮色。饱和度压到一成多,只留一丝冷暖差,
    -- 远看是一排灰阶 —— 但 git diff / ls 靠它还能分出红绿蓝。
    ansi = {
      "#1A1A1A",  -- 0 black   → 代码块/反色块底
      "#B99F9F",  -- 1 red     → 灰红
      "#9CAAA0",  -- 2 green   → 灰绿
      "#B5AD9B",  -- 3 yellow  → 灰黄
      "#A0A9B7",  -- 4 blue    → 灰蓝
      "#B0A5B5",  -- 5 magenta → 灰紫
      "#99A8A8",  -- 6 cyan    → 灰青
      "#C9C9C9",  -- 7 white
    },
    brights = {
      "#4A4A4A",  --  8 bright black
      "#D0B9B9",  --  9 bright red
      "#B7C2BA",  -- 10 bright green
      "#CAC4B5",  -- 11 bright yellow
      "#BCC3CE",  -- 12 bright blue
      "#C9C1CD",  -- 13 bright magenta
      "#B2BFBF",  -- 14 bright cyan
      "#FFFFFF",  -- 15 bright white
    },

    -- 标签栏(§8.6):活动标签白底黑字,是整份主题对比最强的一块
    tab_bar = {
      background         = "#171717",
      active_tab         = { bg_color = "#ECECEC", fg_color = "#171717" },
      inactive_tab       = { bg_color = "#2A2A2A", fg_color = "#9B9B9B" },
      inactive_tab_hover = { bg_color = "#3A3A3A", fg_color = "#ECECEC" },
      new_tab            = { bg_color = "#171717", fg_color = "#7A7A7A" },
      new_tab_hover      = { bg_color = "#ECECEC", fg_color = "#171717" },
      inactive_tab_edge  = "#3D3D3D",
    },
  },

  -- ── 标签栏行为(§8) ────────────────────────────────────────────────
  -- ⚠ 状态栏画在标签栏上,想看到问候语/时钟,这条 `enable` 必须是 true。
  tab_bar = {
    enable            = true,
    hide_if_one       = false,
    at_bottom         = false,
    max_width         = 20,
    show_close_button = true,
    corner_radius     = 6,
    fancy             = true,
    show_new_tab_button = true,
    top_padding = 5,
    edge_padding = 5,
    colors = { active_tab = { bg_color = "", fg_color = "" } },
  },

  -- ── 选区(§6.3):中性灰,不带任何色相;只画底色,所以 opacity 写满
  selection = {
    copy_on_select = true,
    edge_scroll    = true,
    background     = "#3A3A3A",
    opacity        = 1.0,
  },

  -- ── 输入光标与尾迹(§9):尾迹是白块拖出的灰影 ─────────────────────
  cursor = {
    trail                  = 20,
    trail_start_threshold  = 0,
    trail_decay            = "0.1 0.4",
    trail_max              = 220,
    trail_color            = "#8A8A8A",
    blink = "auto",
  },

  -- 切换分屏焦点时的输入光标飞行动画,与普通尾迹共用同一套参数(§15.9)
  pane_focus = {
    sync_trail = true,
    trail_ms   = 320,
  },

  -- ── 分屏分割线(§15.8):灰底 + 中段一道白 ─────────────────────────
  split_divider = {
    enabled   = true,
    color     = "#3D3D3D",
    gradient  = {
      colors      = { "#2A2A2A", "#ECECEC", "#2A2A2A" },
      orientation = "Auto",
    },
    draggable = true,
  },

  -- ── 历史搜索(§11.5) ─────────────────────────────────────────────
  search = {
    enabled         = true,
    case_sensitive  = false,
    highlight_color = "#3A3A3A",  -- 全部匹配:选区同色
    current_color   = "#ECECEC",  -- 当前匹配:主文本色
  },

  -- ── 右侧滚动条:彻底去掉(§11.4) ──────────────────────────────────
  scrollbar = {
    enabled     = false,
    show        = "never",
    thumb_color = "#3A3A3A",
  },

  -- ── 字体 ────────────────────────────────────────────────────────
  -- 字号只影响每个格子的大小;写了 window_width/height(物理像素)后,
  -- 改字号不会再连带改变启动窗口大小。详见手册 §7.2。
  -- line_height 是行高倍数(1.1 = 比字体原生行距松 10%),cell_width 是格宽倍数(1.0 = 字形自然宽度)。
  font = { size = 14, line_height = 1, cell_width = 1 },

  -- ── 窗口装饰:去掉 Windows 顶部标题栏,三枚按钮画进标签栏 ─────────
  window_decorations = "RESIZE",
  window_rounded_corners = true,
  window_width = 1250,
  window_height = 750,

  -- ── 常用运行项 ───────────────────────────────────────────────────
  shell = "",
  shell_args = { "-NoLogo" },
  render = "cpu",

  -- 键位方案:放在 ~/.config/VectorTerm/keys/ 里,裸名字由终端自动去找
  keys_file = "keys.lua",
  padding = 10,
  exit_behavior = "Close",
  status_bar = {
    item_spacing = 0,
    font_scale   = 0.9,
    space_width  = 0.2,
  },
}
