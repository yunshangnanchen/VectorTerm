--[====================================================================[
  VectorTerm 内置主题:Claude Dark(黏土橙)

  结构和 deepseek.lua 完全一样 —— 只有颜色不同,键位、窗口尺寸、状态栏
  参数(0.9 字号 / 0 片段间距 / 0.2 空格宽)全部照搬。

  色板取自 Anthropic / Claude 的品牌色与深色界面:
    黏土橙      #D97757   Claude 的招牌强调色:活动标签 / 光标 / 问候语
    浅黏土      #E89478   亮一档,签名与 bright red
    画布        #1F1E1D   暖调深灰,不用纯黑(和 Claude 的界面一致)
    侧栏/标签底  #1A1918   再深一点
    面板        #2B2A28   非活动标签
    选区/悬停    #3A322C   暖褐,和黏土橙同族
    主文本      #F0EEE6   Claude 的暖白(不是纯白)
    次文本      #A39C93 / #8C857C
    分割线      #4A423C
    滚动条滑块  #3A3734   (本主题默认关掉滚动条)
  ANSI 8 基础 + 8 亮色偏低饱和,和这套暖色板配套,不会突然跳出一个刺眼的纯色。
  活动标签是橙底压深墨字:白字在 #D97757 上对比度只有 3.1,深墨字有 5.4,
  16px 标签看着更清楚。

  ── 怎么用 ──────────────────────────────────────────────────────────
  1) 推荐:配置管理器左栏选中本文件 → 右键「设为当前配置」
  2) 临时试:VectorTerm.exe --config "<程序目录>\config\claude.lua"
  3) 当主题底表:在一份配置里写 theme = "claude",再覆盖少量键。
--]====================================================================]

-- ── 状态栏(§12):标签栏右侧的问候语 / 时钟 / 日期 / 电量 / 签名 ─────────
local nf = wezterm.nerdfonts
local THEME = 'dark'        -- 'dark' | 'light',只影响状态栏文字的明暗
local CL_CLAY = '#D97757'   -- 黏土橙(与 colors.cursor_bg 同色)
local CL_CLAY2 = '#E89478'  -- 亮一档的黏土(brights[9])

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
  local dim   = is_light and '#8C857C' or '#9C958C'
  local plain = is_light and '#26241F' or '#F0EEE6'
  local sig   = is_light and CL_CLAY or CL_CLAY2
  local accent = CL_CLAY

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
      bicon, bcolor = nf.md_battery, '#86B36B'
    elseif pct >= 60 then
      bicon, bcolor = nf.md_battery_60, '#86B36B'
    elseif pct >= 40 then
      bicon, bcolor = nf.md_battery_40, '#C79A45'
    elseif pct >= 20 then
      bicon, bcolor = nf.md_battery_20, '#D97706'
    else
      bicon, bcolor = nf.md_battery_alert, '#DE6B57'
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
  window_background_color = "#1F1E1D",

  -- ── 基础配色(§6.1) ────────────────────────────────────────────────
  colors = {
    foreground = "#F0EEE6",   -- 主文本(暖白)
    background = "#1F1E1D",   -- 画布(暖调深灰)
    cursor_fg  = "#1F1E1D",   -- 黏土橙光标上的文字用深墨,对比更足
    cursor_bg  = "#D97757",   -- 黏土橙光标(CL_CLAY)

    -- 16 色表(§6.2):8 基础 + 8 亮色,整体低饱和
    ansi = {
      "#1A1918",  -- 0 black   → 比画布更深一档
      "#DE6B57",  -- 1 red     → 黏土红
      "#86B36B",  -- 2 green   → 橄榄绿
      "#E0B36A",  -- 3 yellow  → Kraft 牛皮黄
      "#7C9FD4",  -- 4 blue    → 灰蓝
      "#C08BB5",  -- 5 magenta → 灰紫
      "#74B3A6",  -- 6 cyan    → 灰青
      "#D8D3C8",  -- 7 white   → 暖灰白
    },
    brights = {
      "#4A4640",  --  8 bright black
      "#E89478",  --  9 bright red → CL_CLAY2
      "#A6D48C",  -- 10 bright green
      "#F2CD8C",  -- 11 bright yellow
      "#9BBCEA",  -- 12 bright blue
      "#DDA9CF",  -- 13 bright magenta
      "#94D2C4",  -- 14 bright cyan
      "#FFFDF7",  -- 15 bright white
    },

    -- 标签栏(§8.6):活动标签橙底压深墨字
    tab_bar = {
      background         = "#1A1918",
      active_tab         = { bg_color = "#D97757", fg_color = "#1F1E1D" },
      inactive_tab       = { bg_color = "#2B2A28", fg_color = "#A39C93" },
      inactive_tab_hover = { bg_color = "#3A322C", fg_color = "#F0EEE6" },
      new_tab            = { bg_color = "#1A1918", fg_color = "#8C857C" },
      new_tab_hover      = { bg_color = "#D97757", fg_color = "#1F1E1D" },
      inactive_tab_edge  = "#4A423C",
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

  -- ── 选区(§6.3):暖褐底色,和黏土橙同族
  selection = {
    copy_on_select = true,
    edge_scroll    = true,
    background     = "#3A322C",
    opacity        = 1.0,
  },

  -- ── 输入光标与尾迹(§9):尾迹用黏土橙 ─────────────────────────────
  cursor = {
    trail                  = 20,
    trail_start_threshold  = 0,
    trail_decay            = "0.1 0.4",
    trail_max              = 220,
    trail_color            = "#D97757",
    blink = "auto",
  },

  -- 切换分屏焦点时的输入光标飞行动画,与普通尾迹共用同一套参数(§15.9)
  pane_focus = {
    sync_trail = true,
    trail_ms   = 320,
  },

  -- ── 分屏分割线(§15.8) ──────────────────────────────────────────
  split_divider = {
    enabled   = true,
    color     = "#4A423C",
    gradient  = {
      colors      = { "#2B2A28", "#D97757", "#2B2A28" },
      orientation = "Auto",
    },
    draggable = true,
  },

  -- ── 历史搜索(§11.5) ─────────────────────────────────────────────
  search = {
    enabled         = true,
    case_sensitive  = false,
    highlight_color = "#3A322C",  -- 全部匹配:选区同色
    current_color   = "#D97757",  -- 当前匹配:黏土橙
  },

  -- ── 右侧滚动条:彻底去掉(§11.4) ──────────────────────────────────
  scrollbar = {
    enabled     = false,
    show        = "never",
    thumb_color = "#3A3734",
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
