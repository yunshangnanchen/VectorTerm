--[====================================================================[
  VectorTerm 内置主题:Gemini Dark(Google 蓝紫粉)

  结构和 deepseek.lua 完全一样 —— 只有颜色不同,键位、窗口尺寸、状态栏
  参数(0.9 字号 / 0 片段间距 / 0.2 空格宽)全部照搬。

  色板取自 Gemini 深色界面与它的品牌渐变(蓝 → 紫 → 粉):
    品牌蓝      #1A73E8   Google Blue 600:活动标签底色、光标
    浅蓝        #8AB4F8   Google Blue 200:深底上的强调色,状态栏问候语
    品牌紫      #9B72CB   渐变中段
    品牌粉      #D96570   渐变末端,签名
    画布        #131314   Gemini 深色的底色
    侧栏/标签底  #0F0F10
    面板        #1E1F20   Google 深色 surface
    选区/悬停    #2D2E30
    选中(带蓝)  #283449
    主文本      #E8EAED   Google Grey 200
    次文本      #9AA0A6   Google Grey 500
    分割线      #3C4043
    滚动条滑块  #3C4043   (本主题默认关掉滚动条)
  ANSI 的标准红/绿/黄用 Google 深色主题那一套 200/300 号色,在深底上不刺眼。
  活动标签用 #1A73E8 压白字(比 #4285F4 深一档,白字对比度到 4.5),
  状态栏则用浅蓝 #8AB4F8,深底上更清楚。

  ── 怎么用 ──────────────────────────────────────────────────────────
  1) 推荐:配置管理器左栏选中本文件 → 右键「设为当前配置」
  2) 临时试:VectorTerm.exe --config "<程序目录>\config\gemini.lua"
  3) 当主题底表:在一份配置里写 theme = "gemini",再覆盖少量键。
--]====================================================================]

-- ── 状态栏(§12):标签栏右侧的问候语 / 时钟 / 日期 / 电量 / 签名 ─────────
local nf = wezterm.nerdfonts
local THEME = 'dark'        -- 'dark' | 'light',只影响状态栏文字的明暗
local GM_BLUE = '#1A73E8'   -- 品牌蓝(与 colors.cursor_bg 同色)
local GM_BLUE2 = '#8AB4F8'  -- 深底上用的浅蓝(状态栏文字)
local GM_PINK = '#D96570'   -- 品牌粉(签名)

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

  -- 状态栏文字颜色跟着主题走
  local is_light = THEME == 'light'
  local dim   = is_light and '#5F6368' or '#9AA0A6'
  local plain = is_light and '#202124' or '#E8EAED'
  local sig   = is_light and GM_BLUE or GM_PINK
  local accent = is_light and GM_BLUE or GM_BLUE2

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

  -- 电量:满电 / 充电 / 低电各有图标和颜色。台式机没有电池时循环不进,
  -- 这一段自动跳过。
  for _, b in ipairs(wezterm.battery_info()) do
    local pct = math.floor(b.state_of_charge * 100 + 0.5)
    local bicon, bcolor
    if b.state == 'Charging' then
      bicon, bcolor = nf.md_battery_charging, accent
    elseif pct >= 80 then
      bicon, bcolor = nf.md_battery, '#81C995'
    elseif pct >= 60 then
      bicon, bcolor = nf.md_battery_60, '#81C995'
    elseif pct >= 40 then
      bicon, bcolor = nf.md_battery_40, '#FDD663'
    elseif pct >= 20 then
      bicon, bcolor = nf.md_battery_20, '#F9AB00'
    else
      bicon, bcolor = nf.md_battery_alert, '#F28B82'
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
  -- 窗口底色与终端底色统一,标签栏/留白处不会露出另一种黑。
  window_background_color = "#131314",

  -- ── 基础配色(§6.1) ────────────────────────────────────────────────
  colors = {
    foreground = "#E8EAED",   -- 主文本
    background = "#131314",   -- 画布
    cursor_fg  = "#FFFFFF",   -- 蓝色块状光标上的文字
    cursor_bg  = "#1A73E8",   -- 品牌蓝光标(GM_BLUE)

    -- 16 色表(§6.2):8 基础 + 8 亮色,Google 深色主题那一套
    ansi = {
      "#131314",  -- 0 black   → 与画布同色
      "#F28B82",  -- 1 red     → Google Red 200
      "#81C995",  -- 2 green   → Google Green 300
      "#FDD663",  -- 3 yellow  → Google Yellow 300
      "#8AB4F8",  -- 4 blue    → Google Blue 200(GM_BLUE2)
      "#C58AF9",  -- 5 magenta → Google Purple 200
      "#7DD3E8",  -- 6 cyan
      "#DADCE0",  -- 7 white   → Google Grey 300
    },
    brights = {
      "#3C4043",  --  8 bright black → Google Grey 800
      "#F6AEA9",  --  9 bright red
      "#A8DAB5",  -- 10 bright green
      "#FDE293",  -- 11 bright yellow
      "#AECBFA",  -- 12 bright blue
      "#D7AEFB",  -- 13 bright magenta
      "#9BE7F3",  -- 14 bright cyan
      "#FFFFFF",  -- 15 bright white
    },

    -- 标签栏(§8.6):活动标签是实心品牌蓝 + 白字
    tab_bar = {
      background         = "#0F0F10",
      active_tab         = { bg_color = "#1A73E8", fg_color = "#FFFFFF" },
      inactive_tab       = { bg_color = "#1E1F20", fg_color = "#9AA0A6" },
      inactive_tab_hover = { bg_color = "#2D2E30", fg_color = "#E8EAED" },
      new_tab            = { bg_color = "#0F0F10", fg_color = "#9AA0A6" },
      new_tab_hover      = { bg_color = "#1A73E8", fg_color = "#FFFFFF" },
      inactive_tab_edge  = "#3C4043",
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

  -- ── 选区(§6.3):蓝调底色,和品牌蓝同族
  selection = {
    copy_on_select = true,
    edge_scroll    = true,
    background     = "#283449",
    opacity        = 1.0,
  },

  -- ── 输入光标与尾迹(§9):尾迹用品牌蓝 ─────────────────────────────
  cursor = {
    trail                  = 20,
    trail_start_threshold  = 0,
    trail_decay            = "0.1 0.4",
    trail_max              = 220,
    trail_color            = "#1A73E8",
    blink = "auto",
  },

  -- 切换分屏焦点时的输入光标飞行动画,与普通尾迹共用同一套参数(§15.9)
  pane_focus = {
    sync_trail = true,
    trail_ms   = 320,
  },

  -- ── 分屏分割线(§15.8):渐变走完 蓝 → 紫 → 粉 的品牌渐变 ───────────
  split_divider = {
    enabled   = true,
    color     = "#3C4043",
    gradient  = {
      colors      = { "#1A73E8", "#9B72CB", "#D96570" },
      orientation = "Auto",
    },
    draggable = true,
  },

  -- ── 历史搜索(§11.5) ─────────────────────────────────────────────
  search = {
    enabled         = true,
    case_sensitive  = false,
    highlight_color = "#283449",  -- 全部匹配:选区同色
    current_color   = "#1A73E8",  -- 当前匹配:品牌蓝
  },

  -- ── 右侧滚动条:彻底去掉(§11.4) ──────────────────────────────────
  scrollbar = {
    enabled     = false,
    show        = "never",
    thumb_color = "#3C4043",
  },

  -- ── 字体 ────────────────────────────────────────────────────────
  -- 字号只影响每个格子的大小;下面写了 window_width/height(物理像素)后,
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
