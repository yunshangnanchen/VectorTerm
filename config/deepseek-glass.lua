--[====================================================================[
  VectorTerm 内置主题:DeepSeek Glass(夜空蓝 · 半透明磨砂)

  尺寸、排版、状态栏脚本全部沿用 deepseek.lua —— 窗口还是 1250×750、
  14 号字、行高 1.0、padding 10、顶部标签栏,换过来尺寸一个字都不会变。
  变的只有"玻璃感"这三层:

    1) 底色层  window_background_gradient 深空蓝渐变,取代单一实色,
              上半部分略亮,像玻璃上沿的一道反光;
    2) 整窗透明 window_background_opacity = 0.80,桌面从背后透出来 20%;
    3) 配色   前景提到 #F2F6FF、选区改成半透明品牌蓝 —— 因为统一 alpha
              会把文字一起变淡,不往上提一档会发灰。

  ── 关于"毛玻璃"要说清楚 ───────────────────────────────────────────
  Win11 的 Mica / Acrylic 走的是 DWM 在窗口**背后**画材质,要求客户区对
  DWM 逐像素透明。本项目的交换链是 CreateSwapChainForHwnd,不接受
  PREMULTIPLIED alpha,softbuffer 走的 GDI BitBlt 也没有 alpha 通道,
  所以 win32_system_backdrop 设了也透不出来(启动日志会明说)。
  本文件因此**不设** win32_system_backdrop —— 设了只是多一条警告。

  能拿到的"毛玻璃"是:窗内渐变 + 整窗半透明(透出桌面但不模糊)。
  真想要模糊,只有换 DirectComposition 合成表面这条路,属于渲染层改动,
  配置层做不到。想要"磨砂纹理"的话,用现成的壁纸通道自己糊一张:

      window_background_image = "C:/path/to/wallpaper.jpg"
      window_background_blur  = 18          -- 8~24 是磨砂感
      window_background_image_hsb = { brightness = 0.5, saturation = 0.8 }

  注意相对路径按**配置文件所在目录**解析:这份预设被「设为当前配置」复制到
  %USERPROFILE%\.config\VectorTerm\current\ 之后,相对路径会失效,
  要么写绝对路径,要么把图片一起拷过去。

  ── 怎么用 ──────────────────────────────────────────────────────────
  1) 配置管理器左栏选中本文件 → 右键「设为当前配置」;
  2) 临时试:VectorTerm.exe --config "<程序目录>\config\deepseek-glass.lua"
  3) 当底表:theme = "deepseek-glass",再覆盖少量键。

  每一项的含义见 docs/config-manual.md,透明度的完整说明见
  docs/custom-background.md。
--]====================================================================]

-- ── 状态栏(§12):与 deepseek.lua 同一套问候语/时钟/电量,只是配色更亮 ─
local nf = wezterm.nerdfonts
local DS_BLUE = '#5B7CFF'   -- 比 deepseek 的品牌蓝亮一档(透明度会吃掉一点亮度)
local DS_BLUE2 = '#8FA8FF'

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

  -- 半透明窗口下的文字要更亮才不发灰,分隔点用中间调
  local dim   = '#8FA0BF'
  local plain = '#F2F6FF'
  local sig   = DS_BLUE2
  local accent = DS_BLUE

  local items = {}
  local function put(color, text)
    if color then
      table.insert(items, { Foreground = { Color = color } })
    end
    table.insert(items, { Text = text })
  end

  put(accent, '  ' .. icon .. '  ' .. greeting)
  put(dim, '   ·   ')
  put(plain, clock)
  put(dim, '  ' .. day .. ' 周' .. week .. '  ')

  for _, b in ipairs(wezterm.battery_info()) do
    local pct = math.floor(b.state_of_charge * 100 + 0.5)
    local bicon, bcolor
    if b.state == 'Charging' then
      bicon, bcolor = nf.md_battery_charging, accent
    elseif pct >= 80 then
      bicon, bcolor = nf.md_battery, '#2ED47A'
    elseif pct >= 60 then
      bicon, bcolor = nf.md_battery_60, '#2ED47A'
    elseif pct >= 40 then
      bicon, bcolor = nf.md_battery_40, '#D9A441'
    elseif pct >= 20 then
      bicon, bcolor = nf.md_battery_20, '#E8883A'
    else
      bicon, bcolor = nf.md_battery_alert, '#F2645A'
    end
    put(dim, ' · ')
    put(bcolor, '  ' .. bicon .. '  ' .. pct .. '%')
  end

  put(dim, '  ·  ')
  put(sig, 'Elaina-SakiOVO')
  put(nil, '  ')

  window:set_right_status(wezterm.format(items))
end)

return {
  -- ── 玻璃:底色 + 渐变 + 整窗不透明度 ────────────────────────────────
  -- 底色略亮于 deepseek 的 #0B1220:半透明时如果底色太黑,桌面透上来会发脏。
  window_background_color = "#0D1826",
  -- 上亮下暗的竖向渐变 = 玻璃上沿的反光。两色都取自夜空蓝的同一家族。
  window_background_gradient = {
    colors = { "#12203A", "#0A1120" },
    orientation = "Vertical",
  },
  -- 整窗 80% 不透明:桌面透出 20%。0.75~0.85 是甜区 —— 再往下调,
  -- 桌面颜色会盖过文字;低于 0.7 基本没法读(统一 alpha 是"整块窗口变淡",
  -- 不是只有背景)。
  window_background_opacity = 0.80,

  -- ── 基础配色(§6.1):DARK 色板,前景按透明度提亮 ──────────────────
  colors = {
    foreground = "#F2F6FF",   -- 主文本(deepseek 的 #E8EDF9 在 84% 下会发灰)
    background = "#0B1220",   -- 画布(被上面的渐变盖住,只在渐变不到的角落兜底)
    cursor_fg  = "#0B1220",   -- 蓝色块状光标上的文字:深色更清楚
    cursor_bg  = "#5B7CFF",   -- 品牌蓝提亮一档

    -- 16 色表(§6.2):与 DARK 色板一致;只有 black 提亮,避免在半透明底上糊掉
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
      "#8FA8FF",  -- 12 bright blue → DS_BLUE2
      "#C4B5FD",  -- 13 bright magenta
      "#7BE3F2",  -- 14 bright cyan
      "#FFFFFF",  -- 15 bright white
    },

    -- 标签栏(§8.6):活动标签仍是实心品牌蓝 —— 玻璃窗口里需要一块"不透明"
    -- 的锚点,否则整条栏都飘。非活动标签压暗,和渐变底色拉开层次。
    tab_bar = {
      background         = "#0D1826",
      active_tab         = { bg_color = "#5B7CFF", fg_color = "#FFFFFF" },
      inactive_tab       = { bg_color = "#111C31", fg_color = "#A9B4CC" },
      inactive_tab_hover = { bg_color = "#1E2A4A", fg_color = "#F2F6FF" },
      new_tab            = { bg_color = "#0D1826", fg_color = "#8A93A8" },
      new_tab_hover      = { bg_color = "#5B7CFF", fg_color = "#FFFFFF" },
      inactive_tab_edge  = "#3B4A72",
    },
  },

  -- ── 标签栏行为(§8):与 deepseek 完全一致 ───────────────────────────
  tab_bar = {
    enable            = true,
    hide_if_one       = false,
    at_bottom         = false,
    max_width         = 20,
    show_close_button = true,
    corner_radius     = 6,
    fancy = true,
    show_new_tab_button = true,
    top_padding = 5,
    edge_padding = 5,
    colors = { active_tab = { bg_color = "", fg_color = "" } },
  },

  -- ── 选区(§6.3):半透明品牌蓝。这是 glass 主题里最"玻璃"的一处 ——
  --   选中代码时底下的渐变还能透出来,而不是糊一块实色。
  selection = {
    copy_on_select = true,
    edge_scroll    = true,
    background     = "#5B7CFF",
    opacity        = 0.30,
  },

  -- ── 输入光标与尾迹(§9):尾迹延长、颜色提亮,在透明底上拖影更明显 ────
  cursor = {
    trail                  = 24,
    trail_start_threshold  = 0,
    trail_decay            = "0.1 0.4",
    trail_max              = 220,
    trail_color            = "#6D87FF",
    blink = "auto",
  },

  -- 切换分屏焦点时的输入光标飞行动画,与普通尾迹共用同一套参数(§15.9)
  pane_focus = {
    sync_trail = true,
    trail_ms   = 320,
  },

  -- ── 分屏分割线(§15.8):两端压暗、中间品牌蓝,在渐变底上像一道高光 ──
  split_divider = {
    enabled   = true,
    color     = "#3B4A72",
    gradient  = {
      colors      = { "#16223A", "#5B7CFF", "#16223A" },
      orientation = "Auto",
    },
    draggable = true,
  },

  -- ── 历史搜索(§11.5) ───────────────────────────────────────────────
  search = {
    enabled         = true,
    case_sensitive  = false,
    highlight_color = "#1E2A4A",
    current_color   = "#5B7CFF",
  },

  -- ── 右侧滚动条:与 deepseek 一样彻底去掉(§11.4) ────────────────────
  scrollbar = {
    enabled     = false,
    show        = "never",
    thumb_color = "#28324B",
  },

  -- ── 字体:与 deepseek 同号,换主题不会改变窗口尺寸 ──────────────────
  font = { size = 14, line_height = 1, cell_width = 1 },

  -- ── 窗口:1250×750、无标题栏、圆角 —— 与 deepseek 逐项相同 ──────────
  window_decorations = "RESIZE",
  window_rounded_corners = true,
  window_width = 1250,
  window_height = 750,

  -- ── 常用运行项 ────────────────────────────────────────────────────
  shell = "",
  shell_args = { "-NoLogo" },
  render = "cpu",

  keys_file = "keys.lua",
  padding = 10,
  exit_behavior = "Close",
  status_bar = { item_spacing = 0,
  font_scale = 0.9,
  space_width = 0.2, },
}
