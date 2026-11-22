--[====================================================================[
  VectorTerm 内置主题:DeepSeek Glass(夜空蓝 · 真毛玻璃)

  尺寸、排版、状态栏脚本全部沿用 deepseek.lua —— 窗口还是 1250×750、
  14 号字、行高 1.0、padding 10、顶部标签栏,换过来尺寸一个字都不会变。
  变的只有"玻璃"这三层:

    1) 材质    win32_system_backdrop = "Acrylic":DWM 在窗口**背后**画
              Acrylic 材质,背后的一切先被糊掉再透上来;
    2) 逐像素透明 window_background_opacity = 0.55,只压**画布** ——
              清屏底色、壁纸/渐变、标签栏合成完整体 55% 不透明(叠几层也只
              透一次),文字/光标/图片仍是实心。单元格自己设的底色
              (powerline 提示符、反显、ls 的目录色)不受它影响,由
              `text_background_opacity` 管,默认 1.0 = 保持原色;
    3) 配色    前景提到 #F2F6FF、选区改成半透明品牌蓝。

  ── 为什么这次能真透 ────────────────────────────────────────────────
  以前不行:窗口交换链是 `CreateSwapChainForHwnd`,DXGI 不接受
  PREMULTIPLIED alpha(返回 0x887A0001),而 GDI 的 BitBlt 也没有 alpha
  通道,所以材质设了也透不出来。

  现在渲染层换成了 **DirectComposition 合成交换链**
  (`CreateSwapChainForComposition` + `IDCompositionVisual`),交换链本身带
  预乘 alpha:清屏和背景层按 opacity 输出,DWM 就能把材质垫在背后透出来。
  这需要 GPU 后端(`render = "gpu"` 或 `"auto"`);CPU 软渲染拿不到逐像素
  alpha,窗口会保持不透明(而不是把文字一起压淡),日志里会说明。

  ── 失焦之后 ────────────────────────────────────────────────────────
  wezterm 的"切到别处毛玻璃就没了"是 DWM 材质(DWMSBT_TRANSIENTWINDOW)
  在窗口失活时被系统撤掉。这里改用 accent 模糊(作用在窗口自身合成上,
  与激活状态无关),实测有焦点/失焦的窗口内梯度能量都是 4.5,不会掉。

  ── 想要更夸张的效果 ────────────────────────────────────────────────
  - 更透:opacity 调到 0.35~0.45;再低文字和背景的分界就糊了。
  - 不想要模糊、只想透:win32_system_backdrop = "none"。

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
  -- 整块画布 55% 不透明:背后的 Acrylic 材质透出 45%。
  -- 这是"只压画布"的语义 —— 逐像素 alpha 下文字仍然实心,所以
  -- 0.4~0.6 都能读(旧的整窗 alpha 才需要 0.75 以上)。
  -- 画布叠几层(壁纸 + 默认底色 + 标签栏底色)也只看这一个值。
  window_background_opacity = 0.55,
  -- 单元格自设底色(oh-my-posh 的提示符色块、反显、ls 目录色)保持原色。
  -- 调到 0.5 之类的值它们也会跟着透,但色块里的字会和背后的玻璃糊在一起 ——
  -- 提示符的圆角是前景字形、不会跟着透,两边一对比就像"渲染坏了"。
  text_background_opacity = 1.0,
  -- DWM 材质:Acrylic(实时模糊窗口后方)。Mica 只采壁纸、更省电;
  -- tabbed 是 Win11 标签页材质。
  win32_system_backdrop = "Acrylic",

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
  -- 毛玻璃要 GPU(合成交换链);没有 GPU 的机器会自动退回软渲染
  render = "auto",

  keys_file = "keys.lua",
  padding = 10,
  exit_behavior = "Close",
  status_bar = { item_spacing = 0,
  font_scale = 0.9,
  space_width = 0.2, },
}
