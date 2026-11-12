--[====================================================================[
  VectorTerm 内置主题:DeepSeek Dark(夜空蓝)

  色板逐项取自早期 wezterm 配置的 DARK 表(THEME = 'dark'),不另造颜色:
    品牌蓝      #4D6BFE   light/dark 共用,光标 / 蓝色 ANSI / 活动标签
    蓝色浅化    #6D87FF   bright blue
    夜空蓝底    #0B1220   画布 / 标签栏底 / 新标签底
    面板        #16223A   非活动标签 / 分割线渐变两端
    选区/悬停    #1E2A4A
    主文本      #E8EDF9
    次文本      #A9B4CC / #8A93A8
    分割线      #3B4A72
    滚动条滑块  #28324B   (本主题默认关掉滚动条,仅在你重新打开时生效)
  ANSI 8 基础 + 8 亮色、tab_bar 六组配色也原样搬来,见下方各行的注释。

  ── 怎么用 ──────────────────────────────────────────────────────────
  1) 推荐:配置管理器左栏选中本文件 → 右键「设为当前配置」
     它会被复制到用户目录的 .config\VectorTerm\current\,终端下次启动就整份
     加载它(配色 + 状态栏 + 窗口设置都在这里)。
  2) 临时试:VectorTerm.exe --config "<程序目录>\config\deepseek.lua"
  3) 当主题底表:在一份配置里写 theme = "deepseek",再覆盖少量键(兼容旧写法)。

  本文件既能当"主题"也能当"完整配置",由 build.cmd / 安装包随程序分发到
  <程序目录>\config\。每一项的含义见 docs/config-manual.md。
--]====================================================================]

-- ── 状态栏(§12):标签栏右侧的问候语 / 时钟 / 日期 / 电量 / 签名 ─────────
--
-- 只在标签栏显示时才有位置可画;把 tab_bar.enable 设成 false 时状态栏
-- 一起隐藏(窗口四条边没有别的容器可挂)。
--
-- 想要"状态栏常驻、但不要标签"的话:保留 tab_bar.enable = true,
-- 再把 hide_if_one 打开 —— 单标签时整条收起,多标签时才出现。
local nf = wezterm.nerdfonts
local THEME = 'dark'        -- 'dark' | 'light',只影响状态栏文字的明暗
local DS_BLUE = '#4D6BFE'   -- 品牌蓝(与 colors.cursor_bg 同色)
local DS_BLUE2 = '#6D87FF'  -- 亮一档的蓝(与 brights[12] 同色)

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
  local dim   = is_light and '#8A93A8' or '#6B7791'
  local plain = is_light and '#16203A' or '#E8EDF9'
  local sig   = is_light and DS_BLUE or DS_BLUE2
  local accent = DS_BLUE

  local items = {}
  local function put(color, text)
    if color then
      table.insert(items, { Foreground = { Color = color } })
    end
    table.insert(items, { Text = text })
  end

  -- 图标两侧留白:空格被 `status_bar.space_width = 0.2` 收到 1/5 宽,
  -- 一个空格只有 2px,图标会贴住旁边的字,所以这里给两个。
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
      bicon, bcolor = nf.md_battery, '#0F9D58'
    elseif pct >= 60 then
      bicon, bcolor = nf.md_battery_60, '#0F9D58'
    elseif pct >= 40 then
      bicon, bcolor = nf.md_battery_40, '#B26A00'
    elseif pct >= 20 then
      bicon, bcolor = nf.md_battery_20, '#D97706'
    else
      bicon, bcolor = nf.md_battery_alert, '#D93025'
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
  -- 窗口底色与终端底色统一成夜空蓝,标签栏/留白处不会露出另一种黑。
  window_background_color = "#0B1220",

  -- ── 基础配色(§6.1):DARK 色板 ────────────────────────────
  colors = {
    foreground = "#E8EDF9",   -- 主文本
    background = "#0B1220",   -- 画布
    cursor_fg  = "#FFFFFF",   -- 蓝色块状光标上的文字
    cursor_bg  = "#4D6BFE",   -- 品牌蓝光标(DS_BLUE)

    -- 16 色表(§6.2):8 基础 + 8 亮色,与 DARK 色板的 ansi/brights 一致
    ansi = {
      "#0B1220",  -- 0 black   → 与画布同色
      "#F0564A",  -- 1 red
      "#3DDC97",  -- 2 green
      "#F5C451",  -- 3 yellow
      "#4D6BFE",  -- 4 blue    → DeepSeek 品牌蓝
      "#A78BFA",  -- 5 magenta
      "#4DD0E1",  -- 6 cyan
      "#C9D3E8",  -- 7 white
    },
    brights = {
      "#2A3550",  --  8 bright black
      "#FF7B6B",  --  9 bright red
      "#6BE8B4",  -- 10 bright green
      "#FFD97A",  -- 11 bright yellow
      "#6D87FF",  -- 12 bright blue → DS_BLUE2
      "#C4B5FD",  -- 13 bright magenta
      "#7BE3F2",  -- 14 bright cyan
      "#FFFFFF",  -- 15 bright white
    },

    -- 标签栏(§8.6):活动标签是实心品牌蓝 + 白字,DARK 色板的招牌样式
    tab_bar = {
      background         = "#0B1220",
      active_tab         = { bg_color = "#4D6BFE", fg_color = "#FFFFFF" },
      inactive_tab       = { bg_color = "#16223A", fg_color = "#A9B4CC" },
      inactive_tab_hover = { bg_color = "#1E2A4A", fg_color = "#E8EDF9" },
      new_tab            = { bg_color = "#0B1220", fg_color = "#8A93A8" },
      new_tab_hover      = { bg_color = "#4D6BFE", fg_color = "#FFFFFF" },
      inactive_tab_edge  = "#3B4A72",  -- 取自 DARK 色板的 split 色
    },
  },

  -- ── 标签栏行为(§8) ────────────────────────────────────────────────
  -- ⚠ 状态栏是画在标签栏上的,所以想看到上面的问候语/时钟/电量,这条
  --   `enable` 就必须是 true。不想要标签栏就设成 false,状态栏也随之消失。
  --   `hide_if_one` 打开后,只有一个标签时整条收起(状态栏也没了)。
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

  -- ── 选区(§6.3):DARK 色板的 selection_bg / selection_fg 是不透明实色,
  --    VectorTerm 只画底色,所以 opacity 写满 1.0,文字仍用主文本色。
  selection = {
    copy_on_select = true,
    edge_scroll    = true,
    background     = "#1E2A4A",
    opacity        = 1.0,
  },

  -- ── 输入光标与尾迹(§9):尾迹也用品牌蓝 ─────────────────────────────
  cursor = {
    trail                  = 20,
    trail_start_threshold  = 0,
    trail_decay            = "0.1 0.4",
    trail_max              = 220,
    trail_color            = "#4D6BFE",
    blink = "auto",
  },

  -- 切换分屏焦点时的输入光标飞行动画,与普通尾迹共用同一套参数(§15.9)
  pane_focus = {
    sync_trail = true,
    trail_ms   = 320,
  },

  -- ── 分屏分割线(§15.8):DARK 色板的 split #3B4A72 作兜底,
  --    渐变只用同一张色板里的面板色与品牌蓝。
  split_divider = {
    enabled   = true,
    color     = "#3B4A72",
    gradient  = {
      colors      = { "#16223A", "#4D6BFE", "#16223A" },
      orientation = "Auto",
    },
    draggable = true,
  },

  -- ── 历史搜索(§11.5) ───────────────────────────────────────────────
  search = {
    enabled         = true,
    case_sensitive  = false,
    highlight_color = "#1E2A4A",  -- 全部匹配:选区同色
    current_color   = "#4D6BFE",  -- 当前匹配:品牌蓝
  },

  -- ── 右侧滚动条:彻底去掉(§11.4) ────────────────────────────────────
  -- enabled = false 与 show = "never" 任一都能关掉;两个都写是为了
  -- 无论旧版(只看 enabled)还是新版(只看 show)都不会画出轨道/滑块。
  -- 回看历史仍可用滚轮 / Shift+PageUp / Ctrl+Shift+F,只是没有可视滑条。
  -- 若你自己重新打开,滑块沿用 DARK 色板的 scrollbar_thumb。
  scrollbar = {
    enabled     = false,
    show        = "never",
    thumb_color = "#28324B",
  },
  -- ── 字体 ──────────────────────────────────────────────────────────
  -- 字号只影响每个格子的大小;下面写了 window_width/height(物理像素)后,
  -- 改字号不会再连带改变启动窗口大小。详见手册 §7.2。
  -- line_height 是行高倍数(1.1 = 比字体原生行距松 10%),cell_width 是格宽倍数(1.0 = 字形自然宽度)。
  font = { size = 14, line_height = 1, cell_width = 1 },

  -- ── 窗口装饰:去掉 Windows 顶部标题栏,三枚按钮画进标签栏 ───────────
  -- 想要原生标题栏就改成 "TITLE|RESIZE";只去掉标题栏、不要按钮用 "RESIZE"。
  window_decorations = "RESIZE",
  window_rounded_corners = true,
  window_width = 1250,
  window_height = 750,

  -- ── 常用运行项(和主题无关,顺手给一份能直接用的) ─────────────────
  shell = "",
  shell_args = { "-NoLogo" },
  render = "cpu",

  -- 键位方案:放在 ~/.config/VectorTerm/keys/ 里,裸名字由终端自动去找
  keys_file = "keys.lua",
  padding = 10,
  exit_behavior = "Close",
  status_bar = { item_spacing = 0,
  font_scale = 0.9,
  space_width = 0.2, },
}
