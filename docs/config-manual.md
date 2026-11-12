# VectorTerm Lua 配置完全手册

> 从安装、找到配置文件、写第一行配置,到每一个配置项的类型 / 默认值 / 取值范围 /
> 生效时机,全部在这一篇里。文档以 v0.1.0 的源码 `src/config.rs`、`src/script.rs`、
> `src/wezterm_compat.rs` 为准 —— 写在这里的每个键都是解析器真正会读的键,
> 没有"预留""计划支持"的占位项。
>
> 配套材料:[`config.example.lua`](../config.example.lua)(注释版示例)、
> [`configuration.md`](configuration.md)(设计取舍与排障长文)、
> [`custom-background.md`](custom-background.md)(背景与透明度的实现细节)。

## 目录

- [1 安装与首次运行](#1-安装与首次运行)
- [2 配置文件放在哪里](#2-配置文件放在哪里)
- [3 配置文件怎么写](#3-配置文件怎么写)
- [4 会话与启动项](#4-会话与启动项)
- [5 字体](#5-字体)
- [6 颜色](#6-颜色)
- [7 窗口与外观](#7-窗口与外观)
- [8 标签栏](#8-标签栏)
- [9 鼠标选区与复制](#9-鼠标选区与复制)
- [10 光标尾迹](#10-光标尾迹)
- [11 回看滚动动画](#11-回看滚动动画)
- [12 渲染后端与缩放](#12-渲染后端与缩放)
- [13 自定义背景](#13-自定义背景)
- [14 终端图片协议](#14-终端图片协议)
- [15 键位映射](#15-键位映射)
- [16 VectorTerm 运行时 API](#16-vectorterm-运行时-api)
- [17 wezterm 兼容层](#17-wezterm-兼容层)
- [18 热重载对照表](#18-热重载对照表)
- [19 全量配置模板](#19-全量配置模板)
- [20 常见配方](#20-常见配方)
- [21 排错](#21-排错)
- [22 默认值总表](#22-默认值总表)

---

## 1 安装与首次运行

### 1.1 运行环境

| 项目 | 要求 |
|---|---|
| 系统 | Windows 10 1809 及以上(需要 ConPTY)、Windows 11 |
| 架构 | x64 |
| 运行时依赖 | 无。VC 运行库走系统自带 / 静态链接,不需要额外安装 |

### 1.2 拿到可执行文件

**方式 A:下载发行包**

从 Releases 下载 zip,解压到任意有写权限的目录。装在 `C:\Program Files` 下也能用,
但那里改文件要过 UAC,自定义配置请一律放到用户目录(见 2.2):

```text
VectorTerm/
├── VectorTerm.exe
└── config/
    └── config.lua        ← 随包附带的默认配置
```

**方式 B:从源码构建**

```bat
:: 首次构建会浅克隆 wezterm-term 源码(需要网络)
git clone https://github.com/yunshangnanchen/VectorTerm.git && cd VectorTerm
build.cmd --release
```

产物是 `target\release\VectorTerm.exe`(GUI 子系统),默认配置会被同步到
`target\release\config\config.lua`。想让别人的机器上双击就能跑,把这两样一起拷过去即可。

构建细节(为什么必须用 `build.cmd` 而不是裸 `cargo build`)见 [build.md](build.md)。

### 1.3 第一次启动

```bat
:: 双击,或
VectorTerm.exe

:: 指定一份配置启动(优先级最高,见第 2 章)
VectorTerm.exe --config C:\Users\you\.config\VectorTerm\config.lua
```

- 启动的是一个 **PowerShell 7** 会话。没装 PS7 会自动降级到 Windows 自带的
  `powershell.exe`,再降到 `cmd.exe`,不用改配置也不会起不来。
- 发行版是 **GUI 子系统**:双击不会附带黑色控制台窗口。
- 默认窗口 960×640,带 Windows 系统标题栏和一条标签栏。
- 配置有错时,主窗口照常用默认配置打开,旁边弹出一个独立的
  `VectorTerm Debug Console` 列出错误 —— 当前终端里不会出现任何错误文字(见第 21 章)。

### 1.4 想让软件换个地方跑(便携安装)

VectorTerm 不写注册表,所有状态都在两处:

| 位置 | 内容 | 能否删 |
|---|---|---|
| `%USERPROFILE%\.config\VectorTerm\` | 你的 `config.lua`、`VectorTerm.log`、`config-error.log` | 可以(会退回默认配置) |
| 程序目录 | `VectorTerm.exe` + `config\config.lua`(默认配置) | 整个目录可搬走 |

所以"便携版"就是把这两样塞进 U 盘:程序目录带 `config\config.lua`,
你的自定义配置另外用 `--config` 指过去。

---

## 2 配置文件放在哪里

### 2.1 查找顺序

按下列顺序查找,**先命中的那个生效**(后面的完全不读):

| 优先级 | 路径 | 用途 |
|---|---|---|
| 1 | `--config <路径>` | 显式指定。**存在与否都直接用它**,读不到会报错 |
| 2 | `%USERPROFILE%\.config\VectorTerm\current\` 里唯一的 `.lua` | **当前配置** —— 配置管理器「设为当前配置」复制过来的那份,文件名随意 |
| 3 | `<exe 目录>\config\config.lua` | 随软件分发的默认配置 |
| 4 | `<exe 目录>\config.lua` | 旧布局,兼容早期把配置放在 exe 旁边的版本 |
| 5 | `<当前工作目录>\config\config.lua` | 开发时从仓库根启动也能吃到默认配置 |

`%USERPROFILE%` 不存在时退回 `%HOME%`。

`current\` 目录有一条硬规则:**里面只能有一份 `.lua`**。

| 目录里的 lua 数量 | 行为 |
|---|---|
| 0 份(或目录不存在) | 静默加载第 3 档:程序自带默认配置 |
| 恰好 1 份 | 加载它,不管它叫什么名字(`mine.lua`、`deepseek.lua` 都行) |
| 2 份及以上 | **报错**,并退回程序自带默认配置;原因写进 `config-error.log` 与调试终端 |

这条规则是为了让"到底哪份配置生效"永远只有一个答案。旧版本那种
"`config.lua` 引用 `settings.lua`、再被内联项盖住"的多层叠加已经**彻底移除**:
配置文件不再支持 `config_files`,`config.lua` 也没有任何特殊身份 —— 它只是
一份普通配置,放在哪、叫什么都不会被特殊对待。

### 2.2 正确做法:用配置管理器「设为当前配置」

`<exe 目录>\config\config.lua` 是**随包分发**的默认配置,重新构建 / 重新解压会被覆盖。
自定义请走这条路:

1. 打开「VectorTerm 配置管理器」;
2. 左栏选中任意 `.lua`(程序目录的默认配置、`deepseek.lua`,或你自己新建的);
3. 右键 →「设为当前配置」(或者打开它后点顶部的「设为当前配置」)。

管理器会把这份文件**复制**到:

```text
%USERPROFILE%\.config\VectorTerm\current\<原文件名>
```

并把 `current\` 里其它 lua 清掉。终端启动时只加载这一份,之后你在管理器里改的
也就是这一份 —— **不存在"改了没反应"的层级问题**:改的就是终端读的。

> 目录为空时,管理器中栏会显示"以程序默认配置创建当前配置"按钮,点一下就有了。
> 想回到程序默认:左栏「当前配置」旁边的「清除」按钮清空 `current\` 即可。

> **旧版的 `%USERPROFILE%\.config\VectorTerm\config.lua` 不再参与加载。** 它可以继续
> 留在原地当备份,也可以手动删掉;想沿用它里面的设置,在管理器里打开它,把需要的
> 项挪到当前配置里(或者直接把旧文件设为当前配置,前提是它本身是一份完整配置)。

### 2.3 相对路径相对谁

| 配置项 | 相对路径的基准 |
|---|---|
| `window_background_image` | **配置文件所在目录** |
| `session_log` | 进程的当前工作目录 |
| `cwd` | 不涉及相对路径,但路径不存在时整项忽略(只记一条日志) |
| `font.dirs` | 原样交给字体加载器 |
| `keys_file` | 配置所在目录 → `%USERPROFILE%\.config\VectorTerm\keys\` → `<exe 目录>\config\` → 工作目录 `config\`(取第一个存在的) |

### 2.4 附带的默认配置长什么样

只有**一份**默认配置:`config/config.lua`。它本身就是完整配置,只列了常用项:

```lua
return {
  shell = "pwsh.exe",
  shell_args = { "-NoLogo" },
  render = "cpu",
  gpu_power = "low",
  images = { enable = true, max_memory_mb = 256 },
  window_decorations = "TITLE|RESIZE",
  window_rounded_corners = true,
  live_resize_mode = "realtime",
  resize_debounce_ms = 150,
  use_resize_increments = true,
  tab_bar = { enable = true, at_bottom = false, max_width = 16, show_close_button = true },
  selection = { copy_on_select = true, background = "#3a6ea5", opacity = 0.45 },
}
```

没写的项一律取第 22 章的默认值。另外随包带一份 `config/deepseek.lua`:
它是 DeepSeek Dark 配色 + 自带状态栏的**完整配置**,在管理器里把它设为当前配置
就能整份用上(见 [§2.5](#25-内置主题))。

### 2.5 内置主题

`theme = "name"` 仍然支持,作为"配色底表":它只影响**同一份配置里没写到的键**。
日常使用不必用它 —— 直接把 `deepseek.lua`(或 `chatgpt.lua` / `claude.lua` /
`gemini.lua`)设为当前配置更直观。

- **当前配置**:`current\` 里唯一那份,见 [§2.1](#21-查找顺序)。
- **主题底表(可选)**:配置里写 `theme = "deepseek"`,终端会先找
  `config/deepseek.lua` 当底层,再把你这份配置的字段深合并盖上去。

四套配色是**编译进 exe 的**(`config.rs` 里 `BUILTIN_THEMES` 用 `include_str!`
把 `config/*.lua` 嵌进二进制),所以只拷一个 `VectorTerm.exe` 出去,`theme =
"chatgpt"` 照样能用,不依赖磁盘上的 `config\` 目录。磁盘上存在同名文件时仍以
磁盘为准 —— 你想改某套配色,复制一份放到配置同级目录改就行。

`build.cmd` 构建和安装包仍会把 `config/*.lua` 一起放进 `<exe 目录>\config\`
(方便打开、复制、`--config` 直接加载),其中 `config.lua` 受
`onlyifdoesntexist` 保护,升级不会覆盖你改过的默认配置。

**用配置管理器(推荐)**:左栏「当前配置」显示终端正在加载哪一份;选中
`deepseek.lua` 右键「设为当前配置」即整份生效。管理器不会再往配置里塞
`theme` / `config_files` 之类的"引用指令"。

**手写也行**,在配置里写一行就能套用配色(不用把整份方案复制过来):

```lua
-- %USERPROFILE%\.config\VectorTerm\current\mine.lua
return {
  theme = "deepseek",
  -- 只想微调某一项就直接写,它会盖在主题之上
  -- colors = { foreground = "#ffffff" },
  shell = "pwsh.exe",
}
```

`theme` 是**加载指令**,不是普通配置项:

| 行为 | 说明 |
|---|---|
| 合并方式 | 先解析主题表,再把你写的键**深合并**盖上去。同名的表递归合并,所以只改 `colors.foreground` 时,主题里的 `colors.ansi` / `tab_bar` / `scrollbar` 等全部保留 |
| 名字限制 | 只允许字母 / 数字 / `-` / `_`,最长 64 字符;`..`、斜杠、绝对路径一律拒绝(防止读到程序目录外的文件) |
| 查找顺序 | ① 配置文件同目录 `themes/<name>.lua` → ② 同目录 `<name>.lua` → ③ `<exe 目录>\config\themes\<name>.lua` → ④ `<exe 目录>\config\<name>.lua` → ⑤ 工作目录下同样两处(开发时用) → ⑥ **exe 内置副本**(`deepseek` / `chatgpt` / `claude` / `gemini` 四套,磁盘上都没有时才用) |
| 找不到 / 语法错 | 按普通配置错误处理:窗口照常用内置默认配置打开,原因写进 `config-error.log`;报错里会列出可用的内置主题名 |
| 不写 `theme` 也行 | 把 `deepseek.lua` 设为当前配置,或 `VectorTerm.exe --config "<exe 目录>\config\deepseek.lua"` |

热重载监视的是**当前配置**的修改时间:直接改主题底表不会自动重载,把当前配置
存一次盘即可。窗口尺寸 / 装饰这类启动项仍要重启(见 [§18.3](#183-必须重启))。

#### DeepSeek Dark 色板(夜空蓝)

`deepseek.lua` 的色值取自早期本地 wezterm 配置的 `DARK` 色板
(`THEME = 'dark'`),不是上一版那套「官方近黑设计系统」:

| 用途 | 色值 |
|---|---|
| 画布 / 终端底色 / 标签栏底 / 新标签底 | `#0B1220` |
| 凹陷面板 / 非活动标签 | `#16223A` |
| 选区 / 悬停 / 全部匹配 | `#1E2A4A` |
| 主文本 | `#E8EDF9` |
| 次文本 / 三级文本 | `#A9B4CC` / `#8A93A8` |
| **品牌蓝**(光标 / 蓝色 ANSI / 活动标签 / 当前匹配) | `#4D6BFE` |
| 浅化蓝(bright blue) | `#6D87FF` |
| 分割线 / 非活动标签分隔线 | `#3B4A72` |
| 滚动条滑块(主题默认关滚动条,重新打开时才生效) | `#28324B` |
| ANSI 红 / 绿 / 黄 / 洋红 / 青 | `#F0564A` / `#3DDC97` / `#F5C451` / `#A78BFA` / `#4DD0E1` |

最显眼的差别是**活动标签改成实心品牌蓝 + 白字**(`#4D6BFE` / `#FFFFFF`),
非活动标签是凹陷的 `#16223A` + `#A9B4CC`;旧版用的是 `#1E2240` 蓝柔光标签。
16 色表(`ansi` 8 项 + `brights` 8 项)与 `tab_bar` 六组配色都按同一份 `DARK` 色板
原样搬来,所以 `ls` / 语法高亮的观感和那份 wezterm 配置一致。
`selection` 沿用 wezterm 的 `selection_bg` 实色,`opacity = 1.0`;分屏分割线用
`#3B4A72` 兜底,渐变只取同一张色板里的 `#16223A → #4D6BFE → #16223A`。

这份配置还做了三件默认配置没有的事:

- **彻底关掉右侧滚动条**:`scrollbar.enabled = false` + `show = "never"`(见
  [§11.4](#114-可视化滚动条与边缘滚屏))。回看不受影响,滚轮 / `Shift+PageUp` /
  `Ctrl+Shift+F` 照常可用,只是不再画轨道和滑块。
- **自带状态栏**:标签栏右侧是「时段问候语 + 图标 + 时钟 + 日期 + 电量 + 签名」,
  颜色跟着配色走,实现在 `wezterm.on('update-right-status')` 里(见
  [§8.7](#87-状态栏左边一条右边一条))。状态栏画在标签栏上,所以这份配置把
  `tab_bar.enable` 设为 `true` —— 不想要标签栏就设回 `false`,状态栏会一起消失。
- **字号 16 + 无标题栏**:`font = { size = 16 }`、`window_decorations = "RESIZE"`
  (去掉 Windows 顶部标题栏,缩放边框保留);窗口尺寸用 `window_width = 1250` /
  `window_height = 700` 写死物理像素,改字号不会连带改窗口大小(见
  [§7.2](#72-初始窗口尺寸))。
- **状态栏收窄**:`status_bar = { font_scale = 0.9, item_spacing = 0, space_width = 0.2 }`
  —— 字号小一档,小点两侧的空隙压到 0.2 个字宽(见 [§8.7](#87-状态栏左边一条右边一条))。

#### 另外三套配色:ChatGPT / Claude / Gemini

`config/` 里还有三份和 `deepseek.lua` **结构完全相同**的整套配置,只有颜色不同
(窗口尺寸、字号、状态栏参数都一样),所以换来换去不会出现"布局跟着变":

| 文件 | 主题 | 画布 | 强调色 | 活动标签 |
|---|---|---|---|---|
| `chatgpt.lua` | ChatGPT Dark(纯灰阶) | `#212121` | `#FFFFFF` | 白底黑字 |
| `claude.lua` | Claude Dark(黏土橙) | `#1F1E1D` | `#D97757` | 橙底深墨字 |
| `gemini.lua` | Gemini Dark(Google 蓝紫粉) | `#131314` | `#1A73E8` / 浅蓝 `#8AB4F8` | 蓝底白字 |

三份都能三种用法随便挑:管理器里「设为当前配置」、`--config <路径>`,或
在一份配置里写 `theme = "chatgpt"` 当底表。分屏分割线的渐变也各自贴合主题:
Claude 是暖橙、Gemini 直接走 蓝 → 紫 → 粉 的品牌渐变,ChatGPT 是灰底一道
白(因为它的界面本身没有彩色,见下)。

`chatgpt.lua` 是唯一一套**纯灰阶**主题:ChatGPT 深色界面没有品牌色,所以
标签栏、光标、选区、分割线、状态栏、电量全走 `#171717 → #ECECEC` 的灰阶,
活动标签是白底黑字。只有 ANSI 16 色保留了低饱和的功能色 —— `git diff`、
`ls`、编译器输出要靠色相区分,压到近灰但不至于全变一色。想彻底纯灰,把
`colors.ansi` / `colors.brights` 换成几级灰即可。

### 2.6 配置目录结构(current/ 与 keys/)

用户目录 `%USERPROFILE%\.config\VectorTerm\` 长这样:

| 路径 | 作用 |
|---|---|
| `current\` | **当前配置**:里面只能有一份 `.lua`,终端只加载它 |
| `keys\` | **键位文件**:`keys.lua` / `keys1.lua` …;通过配置里的 `keys_file` 引用(也可以放在配置同目录) |
| `config-error.log` | 配置解析 / 多份 lua 冲突等错误记录 |
| `VectorTerm.log` | 启动日志(含「初始窗口尺寸来源」「窗口装饰」等诊断信息) |

早期版本留在根目录的 `config.lua` / `settings.lua` 不会被加载,可以当备份留着,
也可以删掉。**注意别把多份 lua 直接拷进 `current\`** —— 那会触发"多份配置"报错,
终端退回程序默认配置。

## 3 配置文件怎么写

### 3.1 硬性规则:必须 `return` 一张表

配置文件是一个 **Lua 5.4 脚本**,执行结果的返回值就是配置表。所以最外层必须
`return { ... }`,漏了 `return` 会报
`配置加载失败,使用默认配置:... attempt to index a nil value`。

```lua
return {
  window_padding = 10,          -- 注意是 window_padding;顶层 padding 不是有效键
  font = { family = "Consolas", size = 16 },
}
```

### 3.2 三种合法写法

**写法一:直接返回表**(推荐,最短)

```lua
return {
  font = { family = "Cascadia Code", size = 15 },
  padding = 6,
}
```

**写法二:先算后返回**(需要条件逻辑时)

```lua
local is_laptop = os.getenv("COMPUTERNAME") == "LAPTOP-01"

return {
  font = { family = "Consolas", size = is_laptop and 14 or 18 },
  render = is_laptop and "cpu" or "gpu",
}
```

**写法三:wezterm 风格**(兼容层,见第 17 章)

```lua
local wezterm = require 'wezterm'
local config = wezterm.config_builder()

config.font = wezterm.font_with_fallback({ 'MesloLGM Nerd Font', 'Microsoft YaHei' })
config.font_size = 14.0
config.colors = { background = '#0B1220' }
config.keys = {
  { key = 'T', mods = 'CTRL|SHIFT', action = wezterm.action.SpawnTab 'DefaultDomain' },
}

return config
```

### 3.3 数值、字符串、表的写法约定

| 类型 | 写法 | 备注 |
|---|---|---|
| 字符串 | `"Consolas"` / `'Consolas'` | 单双引号等价 |
| 数字 | `18` / `18.0` / `0.95` | Lua 里都是 number,整数与浮点都能被接受 |
| 布尔 | `true` / `false` | 别写成 `"true"` 字符串 |
| 数组 | `{ "A", "B" }` | 下标从 1 开始 |
| 字典 | `{ family = "X" }` | 键名区分大小写,**全部小写下划线**,与 wezterm 一致 |

`nil` 等价于"不写"。写 `padding = nil` 和删除该行效果相同。

### 3.4 颜色值的写法

所有颜色字段都交给 wezterm 的颜色解析器,支持:

| 写法 | 例子 |
|---|---|
| `#RGB` | `"#fff"` |
| `#RRGGBB` | `"#0c0c0c"` |
| `#RRGGBBAA` | `"#0c0c0cff"` |
| `rgb:r/g/b` | `"rgb:ff/80/00"` |
| CSS 颜色名 / `rgb()` / `hsl()` | `"tomato"`、`"rgb(255 128 0)"` |

日常用 `#RRGGBB` 就够了。**只接受 ASCII**:写中文或全角符号会解析失败,
该字段静默保持原值(不报错、不中断启动)。

### 3.5 未识别的键会怎样

- **拼错的键名**:被忽略,不报错。`padidng = 10` 就是没写。
- **已识别但值不合法的字符串**(如 `render = "vulkan"`):打印一条提示,
  保持该项的当前值 —— 不会把你悄悄切到另一条路径上。
- **类型完全不对**(如 `padding = "8"` 字符串):该字段取默认值。

所以"配了没反应"第一件事是对着第 22 章核对键名拼写,而不是怀疑软件。

### 3.6 热重载

保存配置文件后约 **0.4 秒**内自动重新加载,不需要重启。哪些项能即时生效、
哪些要新开标签、哪些必须重启,见第 18 章。

热重载时如果语法写坏了(编辑器里改一半就保存):

- **保留当前正在用的配置**,终端不会突然变样;
- 错误送到独立调试终端,同一个错误只报一次(按内容去重);
- 改好再保存即恢复。

### 3.7 安全边界

- Lua 堆上限 **64 MB**。`('x'):rep(1e9)` 这类操作会变成普通的 Lua 错误,
  走"配置加载失败 → 默认配置"分支,而不是让进程 abort。
- 但**不拦死循环**:配置文件里写 `while true do end` 会让启动卡住,只能关掉进程。

---

## 4 会话与启动项

控制"启动什么程序、在哪儿启动、带什么环境、退出后怎么办"。

### 4.1 配置项

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `shell` | 字符串 | `"pwsh.exe"` | 要启动的程序。命令名(走 PATH)或绝对路径 |
| `shell_args` | 数组 | `{ "-NoLogo" }` | 传给 shell 的参数 |
| `default_prog` | 数组 | 无 | wezterm 写法:第 1 项是程序,其余是参数。**`shell` 显式写了就忽略它** |
| `env` | 表 | 无 | 追加 / 覆盖环境变量 |
| `set_environment_variables` | 表 | 无 | wezterm 写法,同上;同名键后写的覆盖先写的 |
| `cwd` | 字符串 | 无 | 启动目录。路径不存在时**整项忽略**并记日志 |
| `session_log` | 字符串 | 无 | 把 shell 的原始输出(含 ANSI 转义)落盘。空串 = 不记录 |
| `exit_behavior` | 字符串 | `"Close"` | 会话退出后:`"Close"` 关标签 / `"Hold"` 留屏 |
| `quit_confirmation` | 布尔 | `true` | 有程序在跑时,关窗 / 关标签 / 关窗格要不要先确认 |
| `skip_close_confirmation_for_processes_named` | 字符串表 | 见 [§4.7](#47-quit_confirmation关窗--关标签--关窗格前确认) | 这张名单里的进程不算"有程序在跑" |
| `session_persistence` | 布尔 | `false` | 会话保持:交给后台 `--mux-server` 托管,关窗不杀会话(见 [§4.8](#48-session_persistence会话保持wezterm-式多路复用)) |

### 4.2 `shell`:自动降级链

写 `shell` 只需要写命令名,找不到时会沿着下列顺序找替代品,并打印一条提示:

```text
你写的 shell → pwsh.exe → powershell.exe → cmd.exe → %COMSPEC% → 原样启动(会失败)
```

找 `pwsh.exe` 时除了 PATH,还会额外探测两个常见位置:

- `%LOCALAPPDATA%\Microsoft\WindowsApps\pwsh.exe`(应用执行别名,商店版)
- `%ProgramFiles%\PowerShell\7\pwsh.exe` 与 `\7-preview\pwsh.exe`(MSI / winget 版)

这是为了刚装完 PS7、资源管理器还带着旧 PATH 的情况。

```lua
-- 四选一
shell = "pwsh.exe"                    -- 默认,PowerShell 7
-- shell = "powershell.exe"           -- Windows 自带的 5.1
-- shell = "cmd.exe"                  -- 命令提示符
-- shell = "C:/msys64/usr/bin/bash.exe"  -- 绝对路径
```

### 4.3 `shell_args`:显式 shell 会清空默认参数

这是个容易踩的坑:**一旦显式写了 `shell`,默认的 `-NoLogo` 就不再继承**,
避免把 PowerShell 的开关带到 `cmd.exe` 上(那会以"无效开关"直接退出)。

```lua
-- 想要 PS7 带 -NoLogo,得自己写全
shell = "pwsh.exe"
shell_args = { "-NoLogo", "-NoProfile" }

-- cmd 不需要参数
shell = "cmd.exe"
shell_args = {}            -- 也可以整行不写
```

降级时参数按**目标 shell** 补:`pwsh.exe` / `powershell.exe` 带 `-NoLogo`,`cmd.exe` 不带。

### 4.4 `env` / `set_environment_variables`

值必须是字符串(Lua 里写 `1` 会被跳过)。父进程环境会被完整继承,这里只做追加 / 覆盖。

```lua
env = {
  FOO = "bar",
  GIT_PAGER = "cat",
  LESS = "-R",
}

set_environment_variables = {
  DSH_TUI_IMAGE_PROTOCOL = "kitty",   -- 让支持探测的 TUI 走 Kitty 图形协议
}
```

两份表都写、且键名重复时,`set_environment_variables` 胜出。

软件本身会给子进程带上 `TERM=xterm-256color` 与 `COLORTERM=truecolor`,不用自己配。

### 4.5 `session_log`:排查显示问题的利器

记录的是**原始字节流**,包含所有转义序列,可以直接 `cat` 出来看重放:

```lua
session_log = "C:/Users/you/vectorterm-session.log"
```

日志随会话增长且不会自动截断,排完问题记得注释掉。

### 4.6 `exit_behavior`:敲 `exit` 之后

| 值 | 行为 |
|---|---|
| `"Close"`(默认) | 收掉这个标签;如果它是最后一个标签,窗口一起退出 |
| `"Hold"` | 留着最后一屏,末尾补一行灰色的 `[会话结束:...]` |

大小写不敏感(`"hold"` 也行)。写别的值会提示并按 `Close` 处理。

### 4.7 `quit_confirmation`:关窗 / 关标签 / 关窗格前确认

默认 `true`:某个标签里还有**程序在跑**(不只是 shell 本身)时,关窗会先清屏,
在正中问一句 `● Really kill this window and all contained tabs and panes?` +
`[Y]es / [N]o`。只认 `Y` / `N` / `Esc` 和鼠标点击(**回车不生效**,防止误按),
悬停到选项上会反显。

同一个开关也管**关标签**(`Ctrl+Shift+W`、标签上的 `×`、中键点标签、Lua 的
`CloseTab`),文案换成 `● Really kill this tab and all contained panes?`;
以及**关窗格**(`Ctrl+Shift+X`、Lua 的 `close_pane` / wezterm 的
`CloseCurrentPane`),文案换成 `● Really kill this pane?`:

| 场景 | 问不问 |
|---|---|
| 关某个标签,它里面还有程序在跑 | 问,只查**这个**标签 |
| 关某个标签,它里面只有 shell 本身 | 不问,直接关 |
| 关最后一个标签 / 点标题栏 ✕ / `Alt+F4` | 问,文案是"关窗口"那句 |
| 相邻标签里跑着程序,但被关的那个是空的 | 不问 —— 别人的程序不该拦住这个标签 |
| 关某个窗格,它里面还有程序在跑 | 问,只查**这个**窗格(同标签的隔壁窗格不算) |
| 关某个窗格,它里面只有 shell 本身 | 不问,直接关 |
| 只剩一个窗格时按 `Ctrl+Shift+X` | 等同关标签,走标签 / 窗口那两句文案 |

```lua
quit_confirmation = false   -- 直接关,不弹确认
```

**怎么算"有程序在跑"**:照 wezterm 的 `LocalPane::can_close_without_prompting`
——从 shell 的 pid 出发,把**整棵进程树**(含 shell 自己)的 exe 名收成一个集合,
再看这个集合是不是 `skip_close_confirmation_for_processes_named`(下面这张表)
的子集。是 → 直接关;多出任何一个名字 → 弹确认。

所以判定是**白名单**而不是"看有没有子进程":`pwsh` 里再开一个 `cmd` 不算"有程序
在跑"(`cmd.exe` 本来就在名单里),而 `python.exe` / `node.exe` / `vim.exe` 会拦。
进程表查不到时**照样弹**,和 wezterm 一样——判不准的时候宁可多问一句。

这张表就是上面说的白名单。默认值和 wezterm 一字不差(末尾两项是我们补的
ConPTY 宿主 —— 它们由系统在起 shell 时创建,不算用户跑的程序):

```lua
skip_close_confirmation_for_processes_named = {
  "bash", "sh", "zsh", "fish", "tmux", "nu",
  "cmd.exe", "pwsh.exe", "powershell.exe",
  "conhost.exe", "openconsole.exe",
}
```

| 想干嘛 | 怎么写 |
|---|---|
| 跑着 `python` 时也别问 | 往里加 `"python.exe"` |
| 连 shell 都算"有程序在跑"(每次都问) | 写 `{}` |
| `shell = "wsl.exe"` 却不想每次关窗都问 | 加 `"wsl.exe"` |

名字比较不分大小写,`PING.EXE` 和 `ping.exe` 等价。

### 4.8 `session_persistence`:会话保持(wezterm 式多路复用)

默认 `false`:每个窗口直连自己的 PTY,关窗就结束会话。写成 `true` 后,所有 shell
交给后台守护进程 `vectorterm --mux-server` 托管,GUI 窗口只当客户端:

| 场景 | 行为 |
|---|---|
| 关标签 / 关窗口 | 只是**断开视图**,shell 和它正在跑的程序继续存活、输出不丢 |
| 下次启动 | 自动 reattach 最近的会话,滚动缓冲完整回放 |
| `Ctrl+Shift+A` | 切到守护进程里其他未显示的会话 |

命令行另有 `vectorterm --mux-list`(列出存活会话)与 `vectorterm --mux-attach <id>`
(恢复指定会话),守护进程也能用 `vectorterm --mux-server` 手动起。VectorTerm 只做本机
单用户(命名管道,无 TLS / SSH),server 存原始输出字节、前端重放重建屏幕。

```lua
session_persistence = true
```

> 这是**启动时**的决定:改完必须重启(见 [§18.3](#183-必须重启)),已经开着的
> 窗口不会中途切到守护进程。文件浏览器 `open_mode = "window"` 拉起的编辑器窗口
> 会强制把它关掉,不让编辑器跨进程保活。

---

## 5 字体

### 5.0 出厂就是 wezterm 同款,字体已编进 exe

不写 `font` 时,默认口径与 wezterm 一致,而且**三款字体都随程序发布**,
系统里没装也能显示:

| 角色 | 字体 | 怎么用 |
|---|---|---|
| 主字体 | **JetBrains Mono**(Regular / Bold / Italic / BoldItalic 四款) | 普通文字,直接按家族名解析到内嵌字节 |
| 图标回退 | **Nerd Font Symbols**(Symbols Nerd Font Mono) | 文本回退链第 1 项,PUA 图标 |
| 彩色 emoji | **Noto Color Emoji** | 单独走彩色位图路径,不进文本回退链 |
| 中文 | 系统字体 Microsoft YaHei | 体积太大(几十 MB),不打包 |

前两款来自 wezterm 仓库 `assets/fonts/`,彩色 emoji 用的是同一字体家族的官方
**CBDT 构建**(wezterm 那份是 COLRv1 矢量渐变版)。原因是 CBDT 里每个 emoji
就是一张 PNG,运行时直接解码即可;COLRv1 要在终端里实现渐变/变换/合成等
十来种 Paint 结构才画得出来。所以彩色 emoji 由渲染器按 CBDT/CBLC 单独取图,
**不能**把它写进 `fallback` —— 那份字体没有 glyf 轮廓,ab_glyph 读不到字形。

显式写 `family = "JetBrains Mono"` 命中的仍是程序自带的那一份(内嵌优先于
系统字体);想用自己机器上的字体,写具体的 `.ttf` 路径或别的家族名。

```lua
-- 什么都不写 = 上表这套;只调字号的话:
font = { size = 15, line_height = 1.1, cell_width = 1.0 }
```

### 5.1 三种写法(三选一)

```lua
-- ① 推荐:表格式,直接写家族名
font = { family = "JetBrains Mono", size = 16 }

-- ② 数组:第 1 项主字体,其余是回退链
font = { "JetBrainsMono Nerd Font", "Microsoft YaHei" }

-- ③ 旧写法:字符串直接当主字体(家族名或字体文件路径都行)
font = "C:/Windows/Fonts/consola.ttf"
```

`font` 写表时,数组形式(`[1]` 是字符串)优先于字典形式;两者不要混着写。

### 5.2 `font` 表内的键

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `family` | 字符串 | `"JetBrains Mono"` | 家族名或字体文件路径(TTF / OTF / TTC);默认值已编进 exe |
| `size` | 数字 | `18` | 逻辑字号 px @96dpi,运行时乘窗口 DPI;**夹到 6–96** |
| `font_size` | 数字 | — | `size` 的别名,两个都写时以 `size` 为准 |
| `line_height` | 数字 | 自动 | 行高倍数;**夹到 0.6–3.0** |
| `cell_width` | 数字 | `1.0` | 格宽倍数;**夹到 0.5–3.0** |
| `fallback` | 数组 | `{ "Nerd Font Symbols", "Microsoft YaHei" }` | 文本回退链,按顺序尝试;彩色 emoji 不在这里 |
| `bold` | 字符串 | 同家族 Bold face | 粗体专用字体 |
| `italic` | 字符串 | 同家族 Italic face | 斜体专用字体 |
| `bold_italic` | 字符串 | 同家族 BoldItalic face | 粗斜体专用字体 |
| `nerd_font` | 布尔 / 字符串 | `"auto"` | PUA 图标字形来源,见 5.4 |
| `dirs` | 数组 | 无 | 额外的字体搜索目录 |

`line_height` 不写时用字体自身的 asc-desc,但夹在 **1.0em–1.6em**:个别字体的 hhea
会给到 2em,不限制的话行距会离谱。想要更紧凑就显式写小(如 `0.85`)。

### 5.3 顶层别名与优先级

| 键 | 生效条件 |
|---|---|
| `font_fallback` | `font` 表里没有给回退链时才生效 |
| `font_size` | `font` 表里没写 `size` / `font_size` 时才生效 |
| `line_height` | **总是覆盖** `font.line_height`(后读) |
| `cell_width` | **总是覆盖** `font.cell_width`(后读) |

```lua
font = { family = "JetBrains Mono", size = 16 }
font_size = 20        -- 被忽略:font.size 已经给了
line_height = 0.9     -- 生效:顶层覆盖表内
```

### 5.4 `nerd_font`:私用区图标来源

Nerd Font 的图标码点在 Unicode 私用区(`U+E000–U+F8FF`、`U+F0000–U+FFFFD` 等),
普通字体没有这些字形。

| 值 | 行为 |
|---|---|
| `"auto"` / 不写 / `true` | 首次遇到缺字时扫描系统字体,挑一个 Nerd Font(Symbols / Mono 变体优先),**排在文本回退链之前** |
| `"MesloLGL Nerd Font Mono"` | 固定用这个家族 |
| `"off"` / `"none"` / `false` | 不加载图标字体,私用区字符按缺字处理 |

排在文本回退链之前很关键:某些 CJK 字体在私用区放了占位字形,顺序错了图标会变成方块。

### 5.5 四个样式槽

同一家族会自动拆成 Regular / Bold / Italic / BoldItalic 四个槽,渲染时按 SGR 属性选 face;
缺 face 才合成(粗体加描、斜体剪切变形)。想用别的家族的粗体,显式指定:

```lua
font = {
  family = "Source Han Sans SC",
  size = 16,
  bold = "Source Han Sans SC Bold",
  italic = "Source Han Sans SC Italic",
  bold_italic = "Source Han Sans SC Bold Italic",
}
```

### 5.6 中英混排的两条规则

1. **按 em 归一**。`ab_glyph` 的 `PxScale` 表示 ascender→descender 的高度而非字号,
   微软雅黑的 asc-desc 是 1.32em,不归一的话汉字会缩到 0.76em、在两格里留出空隙。
2. **保持字体自然比例**。只有墨迹超过所占单元格(附 0.25 格容差)时才等比缩小,
   不为了填满格子横向拉伸 —— 这是 wezterm glyph cache 的规则。

### 5.7 示例:开发者常用配置

```lua
font = {
  family   = "MesloLGL Nerd Font Mono",   -- 有图标
  size     = 15,
  line_height = 0.95,                     -- Nerd Font 的 hhea 偏大,收紧一点
  fallback = { "Microsoft YaHei UI" },    -- 中文
  bold     = "MesloLGL Nerd Font Mono Bold",
  nerd_font = "auto",
  dirs = { "D:/Fonts" },                  -- 自己放到非系统目录的字体
}
```

---

## 6 颜色

### 6.1 基础配色

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `colors.foreground` | 颜色 | `"#cccccc"` | 默认前景色 |
| `colors.background` | 颜色 | `"#0c0c0c"` | 默认背景色 |
| `colors.cursor_fg` | 颜色 | `"#0c0c0c"` | 光标下方文字色 |
| `colors.cursor_bg` | 颜色 | `"#ffffff"` | 光标本体色(块状光标) |

```lua
colors = {
  foreground = "#e6e6e6",
  background = "#0c0c0c",
  cursor_fg  = "#0c0c0c",
  cursor_bg  = "#ffffff",
}
```

配置里的值与 OSC 4/10/11/12 的动态改色并存:应用运行时改色照常生效,
下次热重载会把这些覆盖回配置值。

### 6.2 覆盖调色板

| 键 | 类型 | 说明 |
|---|---|---|
| `colors.ansi` | 数组 | 8 个基础色,顺序 black / red / green / yellow / blue / magenta / cyan / white |
| `colors.brights` | 数组 | 8 个亮色,同样的顺序。会**接在 `ansi` 后面**拼成 16 色表 |
| `colors.indexed` | 表 | `{ [下标] = 颜色 }`,下标 0–255,覆盖任意索引色 |

内部是一张 16 色表 + 一张 256 色表:只写 `ansi` 就只覆盖前 8 项,写了 `brights`
则覆盖 9–16 项。`indexed` 里的下标越界(小于 0 或大于 255)直接丢弃。

```lua
colors = {
  ansi = {
    "#0c0c0c", "#c50f1f", "#13a10e", "#c19c00",
    "#0037da", "#881798", "#3a96dd", "#cccccc",
  },
  brights = {
    "#767676", "#e74856", "#16c60c", "#f9f1a5",
    "#3b78ff", "#b4009e", "#61d6d6", "#f2f2f2",
  },
  indexed = {
    [196] = "#ff0000",   -- 216 色立方里的纯红
    [235] = "#262626",   -- 灰阶
  },
}
```

调色板默认就是完整的 256 色(xterm 16 基础色 + 216 色立方 + 24 灰阶),
真彩色 `SGR 38;2;r;g;b` / `48;2` 原样透传不降级。

### 6.3 选区配色

选区的颜色有两个入口,优先级如下:

| 写法 | 结果 |
|---|---|
| `selection.background`(显式写) | 用它,配合 `selection.opacity` 做半透明高亮 |
| `colors.selection_bg`(wezterm 写法) | 用它,且**不透明度强制为 1.0**(wezterm 的选区色是不透明的) |
| 都不写 | `#3a6ea5` + 不透明度 `0.45` |

```lua
colors = { selection_bg = "#1E2A4A" }   -- 不透明高亮
```

### 6.4 标签栏配色

`colors.tab_bar` 与 wezterm 同名同结构,详见 [8.6](#86-标签栏配色)。

---

## 7 窗口与外观

### 7.1 内边距

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `padding` | 数字 | `8.0` | 内容区四周留白(逻辑像素) |
| `window_padding` | 数字 / 表 | `8.0` | wezterm 写法。表里取 `top`,没有就取 `left` |

两个都写时 **`padding` 胜出**(后解析)。表格式的 `left` / `right` / `bottom`
在 VectorTerm 里不生效 —— 只支持统一内边距,这是刻意的简化。

```lua
padding = 10

-- 或者沿用 wezterm 的写法(只取 top)
window_padding = { left = 6, right = 6, top = 10, bottom = 6 }
```

### 7.2 初始窗口尺寸

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `window_width` | 浮点(px) | 不指定 | **启动窗口宽度**,物理像素(客户区真实像素)。VectorTerm 扩展,wezterm 没有 |
| `window_height` | 浮点(px) | 不指定 | **启动窗口高度**,物理像素 |
| `initial_cols` | 整数 | 自动 | 初始列数,小于 2 视为没写 |
| `initial_rows` | 整数 | 自动 | 初始行数,小于 2 视为没写 |

两种写法,**像素优先**:

1. 写了 `window_width` / `window_height` → 窗口就是这么大,和字号**完全解耦**。
   之后把 `font.size` 从 15 调到 18,窗口尺寸不变,只是每行少显示几列。
2. 没写像素、只写 `initial_cols` / `initial_rows` → 按**单元格数**反推像素
   (等价 wezterm 的同名项)。这时**改字号会连带改变窗口大小**,因为
   "多少格"不变而"每格多大"变了 —— 很多人被这一点坑到。
3. 都不写 → 内置 960×640 起点(逻辑像素,会随系统缩放放大)。

`window_width` / `window_height` 的单位是**物理像素**,不乘 DPI 缩放:
在 125% 缩放的屏幕上写 `1300`,客户区就是 1300 像素宽(而不是 1300 逻辑像素
→ 1625 物理像素)。想按"视觉大小"给尺寸的话,自己乘以缩放比即可。

```lua
-- 想要「字号随便调,窗口尺寸纹丝不动」
window_width  = 1300,
window_height = 734,
font = { size = 15 },
```

> 窗口尺寸只在**创建窗口**时生效,改了要重启(配置管理器里这一项标的是
> 「重启生效」)。启动日志会打印一行「初始窗口尺寸来源」,一眼能看出当前是
> 像素尺寸、列行数还是内置默认在起作用。

> 截图 / 录屏时特别注意:先把窗口开到目标尺寸再输出内容。窗口从"小"变"大"只会
> 在底部补空行,已经滚出视口的行不会回来。

### 7.3 窗口装饰

`window_decorations` 完全沿用 wezterm 的标志组合,"有没有系统标题栏"、
"能不能缩放"、"按钮画在哪"是**三个独立维度**:

| 写法 | 系统标题栏 | 能否缩放 | 标签栏里的窗口按钮 |
|---|---|---|---|
| `"TITLE\|RESIZE"`(**默认**) | ✅ 原生三枚按钮 | ✅ 系统边框 | ❌ |
| `"TITLE"` | ✅ | ❌ | ❌ |
| `"RESIZE"` | ❌ | ✅ 自绘四边热区 | ❌ |
| `"INTEGRATED_BUTTONS\|RESIZE"` | ❌ | ✅ 自绘四边热区 | ✅ 三枚 |
| `"INTEGRATED_BUTTONS"` | ❌ | ❌ | ✅ 三枚 |
| `"NONE"` | ❌ | ❌ | ❌ |

- 标志可以任意 `|` 组合,大小写不敏感。
- `INTEGRATED_BUTTONS` 与 `TITLE` 互斥:写了前者,系统标题栏一定去掉。
- macOS 专有标志(`MACOS_*`)在 Windows 上忽略;拼错的标志只提示不报错,不会挡住启动。
- **没有系统标题栏时,标签栏兼任标题栏**:拖空白处移动窗口、双击最大化。
  即使 `tab_bar.enable = false`,也会留一条纯拖动条,否则窗口没法拖。
- `"RESIZE"` 的缩放热区由程序自绘(winit 的无装饰窗口把非客户区压成 0,
  补 `WS_THICKFRAME` 不会出现系统边框),四边四角贴边时光标变双箭头。

### 7.4 简写 `window_titlebar`

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `window_titlebar` | 布尔 | `true` | 只覆盖"有没有系统标题栏"这一维 |

`true` 会同时把 `INTEGRATED_BUTTONS` 关掉(按钮由系统画,标签栏不重复);
其余维度保持 `window_decorations` 的结果。和 `window_decorations` 一起写时以它为准。

```lua
window_titlebar = false                       -- 无边框,但标签栏里也没按钮
window_decorations = "RESIZE"                 -- 同上,推荐这种写法
```

**想去掉 Windows 顶部那条标题栏,就是这两项之一**:

| 想要的效果 | 写法 |
|---|---|
| 没有系统标题栏,窗口按钮画进标签栏(推荐) | `window_decorations = "INTEGRATED_BUTTONS\|RESIZE"` |
| 没有系统标题栏,也不要按钮,只靠快捷键(Alt+F4) | `window_decorations = "RESIZE"` |
| 只关掉标题栏,其它保持 | `window_titlebar = false` |

配置管理器里这两项在「窗口与外观」组:`窗口装饰(含 Windows 顶部标题栏)`
和 `显示 Windows 顶部标题栏`,标的是**重启生效**。

### 7.5 无边框窗口的圆角

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `window_rounded_corners` | 布尔 | `true` | 无边框时向 DWM 要 Win11 圆角;`false` = 强制方角 |

去掉系统标题栏后 winit 把非客户区压成 0,DWM 默认给方角;这项通过
`DWMWA_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND` 把圆角要回来,最大化 / 全屏时
系统自动不圆。**Windows 10 没有圆角能力,该项无效果**。有系统标题栏时保持 `Default`,
跟随系统 / 用户的全局圆角设置。

### 7.6 标签栏里的窗口按钮

只有 `INTEGRATED_BUTTONS` 生效时才画。**关掉按钮只是不画按钮,顶部那条栏还在,
照样能拖动窗口**,不会连带把标签栏一起去掉。

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `integrated_title_buttons` | 数组 / `false` | `{ 'Hide', 'Maximize', 'Close' }` | 顺序即从左到右的绘制顺序 |
| `integrated_title_button_alignment` | 字符串 | `"Right"` | `"Right"` / `"Left"` |
| `integrated_title_button_color` | 字符串 | `"Auto"` | `"Auto"`(跟随标签栏前景色)或任意颜色 |
| `integrated_title_button_style` | 字符串 | `"Windows"` | `"Windows"`(方形图标)/ `"Gnome"`(圆形按钮底) |

`integrated_title_buttons` 的取值:`'Hide'`(也接受 `'Minimize'` / `'Min'`,Windows 上就是最小化)、
`'Maximize'`(也接受 `'Max'`)、`'Close'`。重复项自动去重,未知项提示后忽略。

样式与对齐的取值为:`"Windows"`(也接受 macOS 的 `"MacOsNative"`,Windows 上统一方形图标)、
`"Gnome"`(也接受 `"Adwaita"`,圆形按钮底);对齐是 `"Right"` / `"Left"`。都大小写不敏感。

```lua
window_decorations = "INTEGRATED_BUTTONS|RESIZE"

integrated_title_buttons = { 'Close' }              -- 只留关闭
-- integrated_title_buttons = {}                    -- 三枚全隐藏
-- integrated_title_buttons = false                 -- 同上
integrated_title_button_alignment = "Right"
integrated_title_button_style = "Gnome"
```

按钮尺寸是标准的 46 逻辑像素(随 DPI 缩放),悬停高亮、关闭悬停变红,
最大化后图标自动变成"还原"双框。

### 7.7 生效时机

这一组都是**窗口创建时**决定的,**改完要重启**:
`window_decorations` / `window_titlebar` / `window_rounded_corners` /
`integrated_title_*` / `initial_cols` / `initial_rows` / `padding`。

---

## 8 标签栏

### 8.1 两种写法,一套参数

顶层别名(wezterm 风格)与 `tab_bar` 表(原生)写的是同一批东西,
**`tab_bar` 表后解析,所以它胜出**:

| `tab_bar` 表内的键 | 顶层别名 | 类型 | 默认 |
|---|---|---|---|
| `enable` | `enable_tab_bar` | 布尔 | `true` |
| `at_bottom` | `tab_bar_at_bottom` | 布尔 | `false` |
| `fancy` | `use_fancy_tab_bar` | 布尔 | `true` |
| `hide_if_one` | `hide_tab_bar_if_only_one_tab` | 布尔 | `false` |
| `max_width` | `tab_max_width` | 整数 | `16`(夹到 3–128) |
| `show_index` | `show_tab_index_in_tab_bar` | 布尔 | `true` |
| `show_new_tab_button` | `show_new_tab_button_in_tab_bar` | 布尔 | `true` |
| `show_close_button` | `show_close_tab_button_in_tabs` | 布尔 | `true` |
| `corner_radius` | — | 数字 | 自动 |
| `edge_padding` | `tab_bar_edge_padding` | 数字 | 自动 |
| `top_padding` | `tab_bar_top_padding` | 数字 | 自动 |
| `colors` | — | 表 | 见 8.6 |

```lua
tab_bar = {
  enable = true,
  at_bottom = false,
  fancy = true,
  hide_if_one = false,
  max_width = 16,
  show_index = true,
  show_new_tab_button = true,
  show_close_button = true,
  corner_radius = 8,
  edge_padding = 10,
  top_padding = 10,
  colors = { ... },
}
```

### 8.2 `enable` / `at_bottom` / `hide_if_one`

| 项 | 效果 |
|---|---|
| `enable = false` | 整条标签栏消失。**左右状态栏也一起消失**(它们画在这条栏上)。无边框模式下退化成一条纯拖动条(否则窗口没法拖) |
| `at_bottom = true` | 标签栏放到窗口底部,内容区在上 |
| `hide_if_one = true` | 只有一个标签时不占高度,内容区自动多一行;新建标签时出现 |

> 有系统标题栏(默认 `window_decorations = "TITLE\|RESIZE"`)时,`enable = false`
> 就是**整条消失**,内容区顶到窗口上沿;只有去掉系统标题栏的无边框模式才会保留
> 那条纯拖动条。

### 8.3 `fancy`:块状标签还是连成一片

| 值 | 外观 |
|---|---|
| `true`(默认) | 标签是带缝隙的独立块,圆角由 `corner_radius` 控制 |
| `false` | 标签连成一片,用 `inactive_tab_edge` 画分隔线,纯扁平色块 |

`fancy = true` 时,所有标签统一成 wezterm fancy tab 的形状:**只有背向内容区的
两角是圆的,朝内容区那条边补成直角**,和终端连成一体。

### 8.4 `max_width`:单个标签的最大宽度

单位是**单元格**。布局规则与 wezterm 一致:先按 `max_width` 封顶,剩余宽度均分;
标签多到放不下就继续挤压(**不做横向滚动**)。取值范围夹到 3–128。

标题按单元格宽度排版:中文占两格,超出用省略号。

### 8.5 `corner_radius` / `edge_padding` / `top_padding`

| 键 | 单位 | 默认 | 说明 |
|---|---|---|---|
| `corner_radius` | 像素 | 自动(约 8) | `0` = 直角;过大时自动夹到标签栏高度的一半以内。仅 `fancy = true` 生效 |
| `edge_padding` | 逻辑像素 | 自动 | 标签栏内容左右各让出的安全区 |
| `top_padding` | 逻辑像素 | 自动 | 到窗口上边框(底部栏时是到下边框)的距离;`0` = 贴边 |

两项写 `0` 就是贴边,写正数按 DPI 放大。负数按 0 处理。

自动值的规则(避免标签被窗口圆角裁掉,同时不留多余的缝):

| 窗口状态 | 自动留白 |
|---|---|
| 无边框 + 圆角 | 10 × DPI 缩放 |
| 有系统标题栏 | 4 × DPI 缩放(客户区上沿是直的,只留一条呼吸缝) |
| 无边框 + `window_rounded_corners = false` | 0 |

窗口被拉得很矮时留白会自动收缩,不会把标签挤没。

### 8.6 标签栏配色

字段名与 wezterm 的 `colors.tab_bar` 一致。子表里 `bg_color` / `fg_color`
与简写 `bg` / `fg` 等价。

| 键 | 默认 | 说明 |
|---|---|---|
| `background` | `"#0c0c0c"` | 标签栏底色 |
| `active_tab` | `#2f2f2f` / `#ffffff` | 活动标签 |
| `inactive_tab` | `#161616` / `#a0a0a0` | 非活动标签 |
| `inactive_tab_hover` | `#262626` / `#e6e6e6` | 非活动标签悬停 |
| `new_tab` | `#0c0c0c` / `#a0a0a0` | 末尾的"+"按钮 |
| `new_tab_hover` | `#2f2f2f` / `#ffffff` | "+"按钮悬停 |
| `inactive_tab_edge` | `"#2a2a2a"` | 非活动标签之间的分隔线,仅在 `fancy = false` 时画;空串 = 不画 |

`colors.tab_bar`(写在 `colors` 里)与 `tab_bar.colors`(写在 `tab_bar` 里)都能用,
后解析的那份覆盖先解析的。

```lua
colors = {
  tab_bar = {
    background = "#0B1220",
    active_tab = { bg_color = "#4D6BFE", fg_color = "#FFFFFF" },
    inactive_tab = { bg_color = "#16223A", fg_color = "#A9B4CC" },
    new_tab = { bg = "#0B1220", fg = "#8A93A8" },
    inactive_tab_edge = "#22304A",
  },
}
```

### 8.7 状态栏:左边一条,右边一条

和 wezterm 一样,标签栏这一行可以额外挂两段文字:左状态栏贴在标签左边,
右状态栏浮到最右(无边框模式下会自动让开窗口按钮)。它们由**事件**驱动:

> ⚠ 状态栏是画在**标签栏**上的 —— `tab_bar.enable = false` 时没有容器,
> 状态栏会一起消失。想要状态栏就得留着标签栏;嫌单标签时占地方可以打开
> `hide_if_one`,让它只在多标签时出现。

```lua
wezterm.on('update-status',       function(window, pane) ... end)  -- 每窗口都发
wezterm.on('update-right-status', function(window, pane) ... end)  -- 只发给当前窗口

window:set_left_status(text)   -- 左状态栏
window:set_right_status(text)  -- 右状态栏
```

`text` 不是纯文本,而是**带转义序列的字符串** —— 用 `wezterm.format()` 生成。
颜色就是这样带进去的:VectorTerm 会把 SGR 解析回来,按片段上色。

#### 刷新节奏

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `status_update_interval` | 整数(毫秒) | `1000` | 两个 `update-*` 事件的派发间隔,夹到 50–60000 |

它同时是空闲窗口的"唤醒时刻":设 `300` 就是每秒刷新三次,时钟走秒也不会跳。
没注册任何状态栏回调时,这一项不产生任何开销。

#### `wezterm.format()` 接受哪些项

| 写法 | 效果 |
|---|---|
| `{ Text = 'abc' }` | 文字 |
| `{ Foreground = { Color = '#7aa2f7' } }` | 之后的前景色(`#rgb` / `#rrggbb` / `rgba:…` 都行) |
| `{ Foreground = { AnsiColor = 'Maroon' } }` | 用调色板里的 16 色之一 |
| `{ Background = … }` | 同上,底色 |
| `{ Attribute = { Intensity = 'Bold' } }` | 粗体(还有 `'Half'`) |
| `{ Attribute = { Italic = true } }` | 斜体 |
| `{ Attribute = { Underline = 'Single' } }` | 下划线 |
| `{ ResetAttributes }` | 清掉当前样式 |

颜色项**只影响它后面**的文字,和 ANSI 的语义一致 —— 所以"先给颜色、再给文字"
是固定套路。

#### 配套的 wezterm API

| 函数 | 返回 |
|---|---|
| `wezterm.strftime(fmt)` | 按 `os.date` 格式化当前时间,如 `'%H:%M'` |
| `wezterm.battery_info()` | 电池列表,每项 `{ state_of_charge = 0..1, state = 'Charging' \| 'Discharging' \| 'Full' \| 'Unknown' }`;台式机为空表 |
| `wezterm.nerdfonts.<名字>` | Nerd Fonts 字形,如 `nf.md_weather_sunset`。共 10396 个名字,键名与 wezterm 一致;查不到的名字返回空串 |

`wezterm.nerdfonts` 是一张**真实**的字形表,由 `tools/gen_nerdfonts.py` 从本机
Nerd Font 的 `post` + `cmap` 表生成(`src/nerdfonts.rs`),所以码位和屏幕上真正
渲染的那版字体完全对应。换字体后想更新:

```bash
python tools/gen_nerdfonts.py                    # 自动找本机 Nerd Font
python tools/gen_nerdfonts.py --font X.ttf       # 指定字体
```

字形不在主字体里时,要保证 `font` 的回退链包含那个字体(中文字形尤其如此),
否则会显示成空白。

#### 状态栏:「长什么样」可以调,「写什么」交给脚本

右侧状态栏的文字由配置里的脚本决定 —— 通常是在 `wezterm.on('update-right-status', …)`
里 `window:set_right_status(wezterm.format{ … })`。`status_bar` 这一组键只管外观:
字号、整条栏高度、左右留白、**相邻两段文字之间的间距**,以及**空格占多宽**。
这一组键同时也是配置管理器里「状态栏」那一页的控件。

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `status_bar.enable` | 布尔 | `true` | `false` = 右侧状态栏整段不画,脚本写的 `set_right_status` 也一起隐藏 |
| `status_bar.keep_script` | 布尔 | `true` | 保留脚本结果的文字;`false` = 只显示 `status_bar.text` |
| `status_bar.font_scale` | 浮点 | `1.0` | 状态栏文字大小 = 终端字号的多少倍(夹到 0.5–2.0),**只影响状态栏** |
| `status_bar.height` | 整数(px) | 不指定 | 整条栏的最小高度;不写 = 跟着字号自动。改它要重启 |
| `status_bar.padding` | 整数(px) | `8` | 状态栏离窗口左右边缘的留白 |
| `status_bar.item_spacing` | 整数(px) | `0` | 相邻两段文字之间的额外间距,见下面的说明 |
| `status_bar.space_width` | 浮点 | `1.0` | 一个空格占几个字宽(0–2);脚本里"小点两侧"的空隙就是空格,调小只是收紧,字不变窄 |
| `status_bar.text` | 字符串 | 空 | 额外拼在前面的一段固定文字(不用写 Lua) |

两个"间距"管的是不同的东西,别弄混:

- `item_spacing`(像素)加在 `wezterm.format` 的**片段之间** —— 每换一次颜色就是一段,
  段与段之间多出这么多像素。它只在片段交界处生效。
- `space_width`(字宽倍数)缩放**空格字符本身的推进量**。脚本里那些"小点分隔带"
  写的就是空格,例如 `'   ·   '` 是点两侧各 3 个字宽,`' · '` 是各 1 个字宽。
  一个空格原本正好是一个字宽(和 wezterm 一样),所以 3 个字宽看着松;
  把 `space_width` 调到 `0.5`,同样的脚本就变成各 1.5 个字宽,调到 `0` 空格完全不占位置。
  字形不参与缩放,字不会被压扁,也不会发糊。

想让「·」两侧收紧又不想改脚本,就调 `space_width`;想让某两段整体再分开一点,
才用 `item_spacing`。

```lua
return {
  status_bar = {
    font_scale   = 0.95,  -- 比终端字小一档
    padding      = 10,    -- 离右边缘 10px
    space_width  = 0.5,   -- 空格只占半个字宽:小点两侧的空隙减半
    item_spacing = 0,     -- 片段交界处不再额外加空
  },
}
```

几句要点:

- 间距按像素算、按当前状态栏字号绘制,改完下一次状态栏刷新(默认每秒)就能看到,
  不用重开窗口;不再像旧版那样"折算成整数个空格"。
- 换字号时终端会**按新尺寸重新栅格化字形**(和 wezterm 画标签栏的做法一样),
  不是把原字号的位图拉伸 —— 所以放大缩小都还是清晰的,颜色也不会发灰。
- 状态栏按字体的**自然字距**排版(不像正文那样对齐到整格),`item_spacing = 0` /
  `space_width = 1.0` 时排出来的松紧和 wezterm 完全一致;`font_scale = 1.0` 时
  直接用正文那份位图。
- `space_width` 只改空格,不改字;收窄之后同一段宽度能塞下更多内容,右对齐的位置
  也会跟着变 —— 占位和绘制用的是同一份算法,不会出现"文字画出去但标签栏没让位"。
- **图标会自己找齐**:Nerd Font 里图标的墨迹占多大全看它是从哪套图形转来的
  (`nf.md_weather_night` 只有半个字高,`nf.md_battery` 却画满整个 em 框)。
  状态栏拿数字 `0` 的墨迹当基准,比字明显高的图标会被**按比例重新栅格化**小
  一档,再把墨迹中线对齐到文字中线 —— 所以电池不会比旁边的 `97%` 高出一截、
  底边也不会坠到基线下面;本来就小的图标保持原样,只做中线对齐。
- `space_width` 收窄之后,图标两侧只留一个空格会显得贴脸(0.2 字宽 ≈ 2px),
  脚本里给图标两边各写**两个空格**就自然了 —— 内置四套主题都是这么写的。
- `height` 撑高的是**整条栏**(标签也跟着变高),状态栏和标签共用这一条。
- 想彻底改内容还是改脚本;`status_bar.text` 只是"再补一段固定文字"的捷径,
  它排在脚本结果的右边。

#### 完整示例:问候语 + 时钟 + 电量 + 签名

```lua
local wezterm = require 'wezterm'
local nf = wezterm.nerdfonts

-- 每个时段一套:问候语 + 图标
local function slot(h)
  if h < 5 then return '夜深了', nf.md_weather_night
  elseif h < 9 then return '早安', nf.md_white_balance_sunny
  elseif h < 12 then return '上午好', nf.md_weather_sunny
  elseif h < 14 then return '午安', nf.md_food
  elseif h < 18 then return '下午好', nf.md_weather_partly_cloudy
  elseif h < 23 then return '晚上好', nf.md_weather_sunset
  end
  return '夜深了', nf.md_sleep
end

wezterm.on('update-right-status', function(window, pane)
  local greeting, icon = slot(tonumber(wezterm.strftime('%H')) or 0)
  local clock = wezterm.strftime('%H:%M')
  local day = wezterm.strftime('%m/%d')
  local week = ({ '日', '一', '二', '三', '四', '五', '六' })[tonumber(wezterm.strftime('%w')) + 1]

  local items = {}
  local function put(color, text)
    if color then table.insert(items, { Foreground = { Color = color } }) end
    table.insert(items, { Text = text })
  end

  put('#7aa2f7', '  ' .. icon .. '  ' .. greeting)
  put('#6B7791', '   ·   ')
  put('#E8EDF9', clock)
  put('#6B7791', '  ' .. day .. ' 周' .. week .. '  ')

  for _, b in ipairs(wezterm.battery_info()) do
    local pct = math.floor(b.state_of_charge * 100 + 0.5)
    local bicon, bcolor = nf.md_battery, '#0F9D58'
    if b.state == 'Charging' then bicon, bcolor = nf.md_battery_charging, '#7aa2f7'
    elseif pct < 20 then bicon, bcolor = nf.md_battery_alert, '#D93025' end
    put('#6B7791', ' · ')
    put(bcolor, '  ' .. bicon .. '  ' .. pct .. '%')
  end

  put('#6B7791', '  ·  ')
  put('#bb9af7', 'Elaina-SakiOVO')
  put(nil, '  ')

  window:set_right_status(wezterm.format(items))
end)
```

#### 排版规则

- 两段状态栏先按文字宽度占位,**合计最多吃掉标签栏的 60%**;窗口很窄时等比压缩,
  标签不会被挤成零宽。
- 压缩后放不下的部分:左状态栏丢**尾巴**,右状态栏丢**开头**(时间比日期重要)。
- 没有系统标题栏时,右状态栏自动排到窗口按钮左边。
- 字号、字体、行高跟终端正文一致,所以状态栏和标签文字在一条基线上。
- 状态栏区域不响应点击(它只是显示)。

> 内置主题 `config/deepseek.lua` 已经内置了上面这套状态栏(问候语 + 时钟 +
> 日期 + 电量 + 签名,颜色跟着主题走),用 `theme = "deepseek"` 就能直接看到。
> 想改文字 / 加内容,复制 `config/deepseek.lua` 到自己目录再改那份。

### 8.8 自定义每个标签的标题与配色

`format-tab-title` 事件对**每个标签**走一遍,拿到的返回值同时决定文字和颜色:

```lua
wezterm.on('format-tab-title', function(tab, tabs, panes, config, hover, max_width)
  return {
    { Background = { Color = tab.is_active and '#7aa2f7' or '#1f2335' } },
    { Foreground = { Color = tab.is_active and '#1a1b26' or '#9aa5ce' } },
    { Text = ' ' .. (tab.tab_index + 1) .. ' ' .. tab.active_pane.title .. ' ' },
  }
end)
```

| 参数 | 说明 |
|---|---|
| `tab.tab_index` | 0 基下标 |
| `tab.tab_id` | 稳定 id(关标签不会串号) |
| `tab.is_active` | 是不是当前标签 |
| `tab.active_pane` | 这个标签的 pane 对象 |
| `tabs` / `panes` | 全部标签 / 全部 pane 的数组(下标从 1 开始) |
| `config` | `{ tab_max_width, use_fancy_tab_bar, enable_tab_bar }` |
| `hover` | 鼠标是不是停在这个标签上 |
| `max_width` | 这个标签最多能有多宽(单位:单元格) |

返回**格式项列表**(和 `wezterm.format` 同一套写法),另外两项由它直接决定:

| 项 | 效果 |
|---|---|
| `{ Background = ... }` | 这个标签的底色(覆盖 `colors.tab_bar.*_tab`) |
| `{ Foreground = ... }` | 没单独上色的文字用的前景色 |

也可以直接返回一个字符串。返回空/出错时退回默认的"序号: 标题"。

pane 对象上常用的方法:

| 方法 | 返回 |
|---|---|
| `pane:get_title()` / `pane.title` | 标签标题(应用通过 OSC 0/2 设的) |
| `pane:get_current_working_dir()` | Url 表(`.file_path`),来自 shell 报的 OSC 7;没报过返回 `nil` |
| `pane:get_foreground_process_name()` | 标签里最靠前的程序名,如 `pwsh.exe`;没有就 `nil` |

`format-tab-title` 只在**活动标签 / 悬停标签 / 标题**变化时重算,每帧都调会白烧 CPU。
它和 `show_index` 互斥:返回了自己的文字,就不再自动加 `序号:` 前缀。

```lua
-- 不想要彩色、只想改字样?返回字符串更省事
wezterm.on('format-tab-title', function(tab)
  return (tab.is_active and '● ' or '○ ') .. tab.active_pane.title
end)
```

### 8.9 标签栏的默认快捷键与鼠标操作

| 操作 | 动作 |
|---|---|
| `Ctrl+Shift+T` | 新建标签 |
| `Ctrl+Shift+W` | 关闭当前标签(里面有程序在跑时先弹确认) |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | 下一个 / 上一个标签 |
| `Ctrl+1` … `Ctrl+9` | 跳到第 n 个标签 |
| 左键点标签 | 切换 |
| 中键点标签 | 关闭 |
| 点标签右侧 `×` | 只关那一个。标签被挤到放不下标题时(`< 4` 格余量)`×` 自动隐藏 |
| 点末尾 `+` | 新建标签 |

关掉最后一个标签等同退出程序。每个标签是一个**独立的 ConPTY 会话**(独立终端状态、
回看缓冲、选区、尾迹动画)。`keys` 里的绑定优先于这些内置快捷键,可以覆盖。

关标签也有二次确认:标签里还有程序在跑(on `×`、中键、`Ctrl+Shift+W` 时都会拦一道),
问的是"关这个标签和它里面的 pane"。只有 shell 本身时不问;关最后一个标签问的是
"关窗口"。开关同样是 `quit_confirmation`(见 [§4.7](#47-quit_confirmation关窗--关标签--关窗格前确认))。

标签标题优先用应用通过 OSC 0/2 设置的窗口标题,没有时显示 shell 名。

---

## 9 鼠标选区与复制

### 9.1 配置项

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `selection.copy_on_select` | 布尔 | `true` | 松开鼠标即把选区写进系统剪贴板 |
| `selection.edge_scroll` | 布尔 | `true` | 拖选到窗口上/下边缘时自动翻回看历史并继续扩选(见 [§11.4](#114-可视化滚动条与边缘滚屏)) |
| `selection.background` | 颜色 | `"#3a6ea5"` | 选区底色 |
| `selection.opacity` | 数字 | `0.45` | 高亮不透明度,**夹到 0.05–1.0** |
| `selection.ctrl_shift_v` | 字符串 | `"paste"` | `Ctrl+Shift+V` 的语义:`"paste"` 或 `"copy"` |

```lua
selection = {
  copy_on_select = true,
  background = "#3a6ea5",
  opacity = 0.45,
  ctrl_shift_v = "paste",
}
```

### 9.2 操作对照

| 操作 | 结果 |
|---|---|
| 左键拖拽 | 框选,松开自动复制(受 `copy_on_select` 控制) |
| 双击 / 三击 | 选词 / 选整行,继续拖动按词 / 行扩展 |
| `Ctrl+Shift+C`、`Ctrl+Insert`、右键(有选区时) | 复制选区 |
| `Ctrl+Shift+V`、`Ctrl+V`、`Shift+Insert`、右键(无选区时) | 粘贴(应用开了 `?2004h` 时自动加括号粘贴包裹) |
| `Shift` + 左键拖拽 | 应用抓了鼠标(vim / tmux)时强制本地框选 |

把 `ctrl_shift_v` 改成 `"copy"` 会让 **V 变成复制**、`Ctrl+Shift+C` 改为粘贴,
照顾"V 记成复制"的操作习惯。

### 9.3 选区的几条硬规则

- 选区用**绝对物理行号**锚定,回看滚动时跟着原来的文字走;
- 复制时宽字符不重复、行尾空格裁掉、软折行的两行之间不插换行;
- 应用抓了鼠标时,普通左键仍按 1000 / 1002 / 1003 模式原样上报,不会抢它的点击;
- 选区颜色是画在文字**下面**的高亮,不会给字形染色;
- 拖到内容区上/下边缘会自动滚屏并**继续扩选**(越贴边越快),不用先松手滚轮翻页再重新拖。

`selection.*` 全部支持热重载。

---

## 10 光标尾迹

移植自 kitty 的 `cursor_trail.c`:光标立即定位,尾迹是一个四角各自指数追赶的
拉伸四边形(而不是一串渐隐的方块)。输入越密追得越快,松手即收尾。

### 10.1 配置项

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `cursor.trail` | 数字 / 布尔 | `20` | 光标静止多少毫秒后尾迹才开始跟随;`0` = 关闭。`true` → 40,`false` → 0 |
| `cursor.trail_start_threshold` | 数字 / 字符串 / 数组 | `0` | 每轴最小位移(格)。两轴都小于阈值时直接吸附,不拖尾 |
| `cursor.trail_decay` | 数字 / 字符串 / 数组 | `"0.1 0.4"` | (最快, 最慢)衰减时间,单位**秒**。越大尾巴越长 |
| `cursor.trail_max` | 数字 | `220` | 单段尾迹动画的硬上限(毫秒);`0` = 不限制 |
| `cursor.trail_color` | 字符串 | `"none"` | 尾迹颜色;`"none"` = 跟随光标颜色 |

```lua
cursor = {
  trail = 20,
  trail_start_threshold = 0,
  trail_decay = "0.1 0.4",
  trail_max = 220,
  trail_color = "none",
}
```

### 10.2 三种写法的取值

`trail_start_threshold` 与 `trail_decay` 都接受三种形式(下面每种是**三选一**):

```lua
-- 阈值:数字 / "x y" / { x, y }
-- cursor = { trail_start_threshold = 2 }
-- cursor = { trail_start_threshold = "2 0" }
-- cursor = { trail_start_threshold = { 2, 0 } }

-- 衰减:数字 / "fast slow" / { fast, slow }
-- cursor = { trail_decay = 0.4 }
-- cursor = { trail_decay = "0.1 0.4" }
-- cursor = { trail_decay = { 0.1, 0.4 } }
```

只写一个数时 `x` 与 `y`(或快与慢)取同一个值;写两项时第二项省略则等于第一项。

- 阈值最终会被夹到 **≥ 0**;`2` 是 kitty 的默认值(单格移动不拖尾)。
- 衰减时间 `fast` 夹到 **≥ 0.01**,`slow` 不小于 `fast`(写反了会自动修正)。

### 10.3 为什么有 `trail_max`

连续输入时**不排队**:目标始终只有一个(最新位置),动画到点直接吸附。
所以再密的输入也不会攒出一串要逐个"播完"的旧动画。`220ms` 是单段的硬上限,
调大尾巴更"黏",调小更跟手。

### 10.4 关联的滚动行为

尾迹与回看滚动是两件事,但手感需要配合:按键 / 粘贴后视口会回到底部,
长按连续输入(回车连发)时回底更快(见 11.3 的 `back_burst`),
让视口赶在下一批输出之前就位,剩下的位移交给尾迹自己播完。

### 10.5 光标本体自定义

尾迹只管「移动时那条尾巴」,这一节管「静止时那块光标」。所有键都写在
`cursor = { ... }` 里,**不写就完全保持旧版行为**(整格方块、不闪、不动),
全部支持热重载,配置管理器的「光标 / 尾迹」分组里也能直接拖。

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `cursor.shape` | 字符串 | `"auto"` | `auto` 跟随终端 DECSCUSR;也可强制 `block` / `bar` / `underline` |
| `cursor.width` | 数字 | `1.0` | 宽度的倍数(0.1–4)。方块是整格宽,竖条是默认粗细;`2` 就是两倍宽 |
| `cursor.height` | 数字 | `1.0` | 高度的倍数(0.1–4)。下划线是默认线粗 |
| `cursor.align_x` | 字符串 / 数字 | 跟随形状 | `left` / `center` / `right`,或 0–1 的数字 |
| `cursor.align_y` | 字符串 / 数字 | 跟随形状 | `top` / `center` / `bottom`,或 0–1 的数字 |
| `cursor.thickness` | 数字 | `0` | `bar` 宽 / `underline` 高的固定像素;0 = 自动(格宽 1/6、格高 1/8) |
| `cursor.alpha` | 数字 | `1.0` | 光标本体不透明度(0.05–1) |
| `cursor.blink` | 字符串 / 布尔 | `"auto"` | `auto` 只在应用声明 `Blinking*` 形状时闪;`true` 始终闪;`false` 不闪 |
| `cursor.blink_rate` | 数字 | `1060` | 一个完整亮灭周期(ms,60–10000);SGR 5/6 文字闪烁共用这条时间轴 |
| `cursor.blink_duty` | 数字 | `0.5` | 一个周期里「亮」占多少(0.05–0.95) |
| `cursor.blink_style` | 字符串 | `"hidden"` | `hidden` 整块消失 / `fade` 淡出再淡回 / `outline` 只留一圈空心描边 |
| `cursor.anim` | 字符串 | `"none"` | `none` / `pulse`(尺寸脉冲) / `breathe`(透明度呼吸) |
| `cursor.anim_ms` | 数字 | `1200` | 常驻动画周期(ms,60–10000) |
| `cursor.anim_amount` | 数字 | `0.25` | 幅度(0–1):脉冲的缩放比例 / 呼吸的最大折损 |

**尺寸与锚点。**`width` / `height` 是相对「该形状的默认尺寸」的倍数,所以
`1.0` 恒等于旧版;`align_x` / `align_y` 决定缩放后在格子里的落点,不写时
方块与竖条贴左上、下划线贴底(与旧版逐像素一致)。写大于 1 的值允许光标
越过格子边界,尾迹的挖空区域仍按整格计算。方块被缩到 40% 以下、或正在
淡出时不再反色重画底下的字,免得字跑到光标外面。

**闪烁。**`blink = "auto"` 时只有 `DECSCUSR` 声明 `Blinking*` 才闪;显式写
`true` 则任何形状都闪。`blink_rate` 同时驱动 SGR 5/6 的闪烁文字,两者不会
「光标闪得比字快」。`fade` 在灭相从 100% 淡到 8% 再淡回,两端连续无跳变;
`outline` 画四条边,适合想要「空心块」的观感。

**动画。**`pulse` / `breathe` 只改显示列表里的矩形尺寸与 alpha,**不改变
光标所在的格子**,所以 IME 候选窗定位、鼠标命中、尾迹挖空都不受影响,也能
和 `trail` 叠加。

```lua
cursor = {
  shape = "bar", thickness = 2, width = 1.0, height = 1.0,
  blink = true, blink_rate = 800, blink_duty = 0.5, blink_style = "fade",
  anim = "breathe", anim_ms = 1600, anim_amount = 0.35,
  trail = 20,                    -- 尾迹照常开
}
```

> 空闲开销:`pulse` / `breathe` / `fade` 需要连续帧(8ms 唤醒),`hidden` /
> `outline` 只在亮灭切换时重绘一次。默认全不开,空闲时仍走 400ms 低频轮询。
> 这些是 wezterm 原生顶层键 `default_cursor_style` / `cursor_blink_rate`
> 的替代写法 —— 那两个名字在本项目里不生效。

---

## 11 回看滚动动画

滚轮滚动是**亚行缓动动画**,不是整数行瞬移。

### 11.1 配置项

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `scroll` | `false` | — | 整体关闭平滑滚动(等价 `anim = 0`) |
| `scroll.anim` | 数字 | `90` | 一个滚轮档位(3 行)的平滑动画时长(毫秒);`0` = 逐行瞬移 |
| `scroll.max` | 数字 | `200` | 单段动画硬上限(毫秒),**最小 1** |
| `scroll.back` | 数字 | `90` | 按键 / 粘贴后回到底部的时长(毫秒) |
| `scroll.back_burst` | 数字 | `40` | 长按连续输入时回底的时长(毫秒) |

```lua
scroll = {
  anim = 90,
  max = 200,
  back = 90,
  back_burst = 40,
}

-- 或者整体关掉
scroll = false
```

### 11.2 缓动策略

- 从静止开始滚动:用 **ease-in-out**(起步慢、中段快、收尾慢);
- 连续滚轮:用 **ease-out** 接着已有速度,不会每档重新加速;
- 按键时有输入优先:立即回底,不让用户在半空中打字。

### 11.3 `back` 与 `back_burst` 的区别

`back` 是普通情况(敲一个键、粘贴一段)回底的时长;`back_burst` 是长按连发
(回车按住不放)时的时长,**明显更短** —— 视口要先就位,下一批输出才不会把
正在看的行顶走。

`scrollback_lines` 决定回看缓冲有多少行,默认 **3500**。

### 11.4 可视化滚动条与边缘滚屏

每个 pane 的右边缘内侧有一条**滚动条**:轨道代表整个回看缓冲,滑块的长度
是「一屏占多少历史」,位置是「现在看到哪一段」。`scroll = 0`(贴底)时滑块在
最下面,往回翻它就往上走。它**不占终端列**,不影响 PTY 尺寸与软折行。

- **拖滑块**:直接落到对应位置,不走缓动动画(拖动必须跟手);
- **点轨道**:在滑块上方/下方点一下就往上/下翻一页;
- 点到的 pane 同时成为焦点 pane(与鼠标点击聚焦一致);
- 分屏时每个 pane 一条,各自显示自己那份缓冲;
- 备用屏(vim / less)没有回看历史,滑块会占满整条轨道。
- 默认**贴底自动隐藏**:静止在底部时不画轨道。分屏时左 pane 的轨道正好贴着
  分割线,常显会形成一条 ~9px 的灰带,看起来像「文字离分割线很远」;自动隐藏
  后文字只剩 1px 空气,一回看或鼠标移到轨道上滚动条照常出现。想始终显示设
  `scrollbar.show = "always"`。若想保留常显滑块但不要那条长轨道,设
  `scrollbar.show_track = false`;想整体换边设 `scrollbar.side = "left"`。
  颜色三件套 `track_color` / `thumb_color` / `hover_color` 都支持 `#RRGGBB`,
  不写则由前景/背景色自动混合;配置管理器「滚动与回看」分组里都有对应控件。

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `scrollbar` | `false` | — | 整体关掉(`scrollbar.enabled = false` 同义) |
| `scrollbar.enabled` | 布尔 | `true` | 是否画滚动条 |
| `scrollbar.width` | 数字 | `9` | 轨道宽度(px),夹到 2–64 |
| `scrollbar.min_thumb` | 数字 | `28` | 滑块最小高度(px),夹到 4–400 |
| `scrollbar.show` | 字符串 | `"auto"` | `always` 一直画 / `auto` 贴底自动隐藏 / `never` 完全不画。写了它就不看 `auto_hide` |
| `scrollbar.side` | 字符串 | `"right"` | 停靠侧:`right` / `left`。分屏时把左 pane 改成 `left`,滚动条就不贴着中间分割线 |
| `scrollbar.show_track` | 布尔 | `true` | 关掉后只画滑块,中间分割线旁边不会再有一条长灰带 |
| `scrollbar.auto_hide` | 布尔 | `true` | 旧字段,等价于 `show`(`true`→`auto`,`false`→`always`) |
| `scrollbar.opacity` | 数字 | `0.85` | 整体不透明度,轨道再乘 0.45 |
| `scrollbar.track_color` | 颜色 | 无 | 不写 / `"Auto"` / `"none"` = 由前景/背景色自动混合 |
| `scrollbar.thumb_color` | 颜色 | 无 | 同上 |
| `scrollbar.hover_color` | 颜色 | 无 | 鼠标悬停 / 拖动滑块时的颜色;不写 = 沿用滑块色 |

`selection.edge_scroll`(默认 `true`)控制**拖选到边缘自动滚屏**:按住左键把
鼠标拖到内容区上/下 3 格高的带状区域里,视口就按 45ms 一拍持续翻历史,越
贴边越快(1–8 行/拍),选区终点始终锚在鼠标所在的绝对物理行上;离开带状
区域或松手立即停。关掉它就退回「选区只限当前视口」。

```lua
scrollbar = {
  enabled = true,
  width = 9,
  min_thumb = 28,
  show = "auto",
  side = "right",
  show_track = true,
  auto_hide = true,        -- 兼容旧配置;写了 show 时以 show 为准
  opacity = 0.85,
  -- track_color = "#333840",
  -- thumb_color = "#7aa2f7",
  -- hover_color = "#ffffff",
},
selection = { edge_scroll = true },
```

### 11.5 历史搜索

`Ctrl+Shift+F` 打开搜索栏(再按一次 / `Esc` 关闭),在**整个回看缓冲**里找,
不只是当前屏幕。输入时所有匹配实时高亮,当前那一处用更亮的颜色并在左边
画一条竖线。搜索栏是模态输入:按键不会发给终端。

| 按键 | 作用 |
|---|---|
| 直接打字 | 追加到查询词,实时重算匹配 |
| `Backspace` | 删一个字符 |
| `Enter` / `↓` | 下一个匹配,并把视口滚到那一行(尽量居中) |
| `Shift+Enter` / `↑` | 上一个匹配 |
| `Tab` | 切换是否区分大小写(左边出现 `[Aa]`) |
| `Esc` / `Ctrl+Shift+F` | 关闭搜索栏 |

匹配用的是**绝对物理行号**,和选区同一套口径;宽字符按它实际占的列数换算,
所以 CJK / emoji 不会让高亮框偏位。终端还在吐字时结果按 180ms 节流重算,
回车跳转前必定拿最新结果。

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `search` | `false` | — | 整体关掉 |
| `search.enabled` | 布尔 | `true` | 是否启用搜索栏 |
| `search.case_sensitive` | 布尔 | `false` | 默认忽略大小写;栏里按 `Tab` 可临时切 |
| `search.highlight_color` | 颜色 | `"#f0c674"` | 所有匹配的半透明底色 |
| `search.current_color` | 颜色 | `"#ff8f3f"` | 当前匹配 / 输入位置的小竖条 |
| `search.max_matches` | 整数 | `5000` | 一次最多收集多少处,上限 200000 |

```lua
search = {
  enabled = true,
  case_sensitive = false,
  highlight_color = "#f0c674",
  current_color = "#ff8f3f",
  max_matches = 5000,
},
```

**进出场动画**。搜索栏和命令面板不是"啪"地出现(§11.6 的浮层同一套):

- 打开时从下方 22px(命令面板 16px)**非线性地**升到位,同时从 94% 放大到
  原尺寸并淡入,190ms 走完 —— 位移、缩放、亮度共用一条 ease-out 曲线,
  所以三者是同时收尾的,不会"框到位了字还在飘";
- 关闭时同一套动作反过来,160ms 收干净。**逻辑上关闭的那一刻按键就交回终端**
  (面板已经不吃键了),只是画面还在把镜头收完;
- 淡入用的是开方曲线而不是线性:190ms 里线性会让前半程几乎全黑,观感像突然
  出现;开方在前三分之一就把轮廓交代清楚。

打开的一瞬间,**输入光标会从终端里的位置一路飞到浮层的输入框**,拖影和
跨窗格切换(`pane_focus.trail`)是**同一个** `CursorTrail` 状态机 —— 同样的
停留门限、衰减时间、硬上限和速度曲线。所以想让它更快 / 更短 / 换个颜色,
调的是同一组键,不用另学一套:

| 想改什么 | 改哪里 |
|---|---|
| 完全关掉飞行 | `pane_focus.trail = false`,或 `cursor.trail = 0`(尾迹系统整体关) |
| 飞得快慢 | `pane_focus.trail_ms`(默认 320,0 = 关闭) |
| 拖影长度 / 浓淡 | `cursor.trail_decay`、`cursor.trail_max` |
| 颜色 | `cursor.trail_color`(`pane_focus.sync_trail = true` 时两边共用) |

**输入焦点真的搬过去了**。浮层是模态文本输入,所以:

- 浮层在屏上(含退场过程)时,终端里那个输入光标会**收起来** —— 否则"光标
  飞过去"之后原地还留着一个在闪,看着像两个光标;关掉浮层它再回来;
- 输入法上屏的文字进浮层,不会漏给终端。开着搜索框用拼音打「，」,这个符号
  落在搜索框里,不会变成 shell 的一条命令;
- 拼音还没上屏的组合串直接画在输入框里(`/ ni` 后面加一条下划线),输入法
  候选窗也跟着输入框走,不会飘回终端光标那边去。

顺带一个更大的坑:中文输入法会接管字母键的虚拟键码,让所有按字符匹配的
快捷键失配。这条已经在窗口过程里统一处理了,详见
[键位总览 §中文输入法开着的时候](keyboard-shortcuts.md#中文输入法开着的时候)。

### 11.6 命令面板(替代 shell 内命令)

以前唤起终端自身的 UI,得在 shell 里敲 `files-gui` 这种专用命令(它往 stdout
打一条私有 OSC,终端再从 PTY 输出流里扫出来)。**那条链路已经整个移除** ——
现在所有「终端自身的动作」都从输入框发起,按键不会漏给 shell,也不需要往
`PATH` 里装任何小工具。

- `Ctrl+Shift+P` 直接打开命令面板。它的进出场动画、以及"打开时光标飞过去"
  都和搜索栏是同一套(见 [§11.5](#115-历史搜索) 末尾),没有单独的开关;
- `Ctrl+Shift+F` 打开历史搜索后,输入第一个字符 `/` 也会切到命令面板
  (和历史搜索共用同一个浮层,`/` 本身不会进搜索词);
- `↑` / `↓` 选命令,`Tab` 补全名字,`Enter` 执行,`Esc` 关闭;直接敲名字、
  别名或唯一前缀(如 `set` → `settings`)都行。

| 命令 | 别名 | 作用 |
|---|---|---|
| `files` | `f` / `explorer` / `file` | 打开**铺满当前 pane** 的可视化文件管理器(起始目录 = 当前 pane 的 cwd) |
| `settings` | `s` / `config` | 打开可视化配置管理器(独立窗口) |
| `help` | `h` / `?` | 列出全部命令与用法 |
| `key` | `keys` / `k` | 查看全部键位与鼠标操作 |
| `reload` | `r` | 重新加载 `config.lua` |
| `search` | `find` | 切回历史搜索栏 |

`help` / `key` 不关闭面板,而是把内容显示成多行输出视图。一屏放不下时
(一屏行数随窗口高度取 8~32 行)用 `↑` / `↓` 逐行、`PgUp` / `PgDn` 翻页、
`Home` / `End` 跳到首尾,底部会显示 `[起始行-结束行 / 总行数]`;再按 `Esc`
回到命令列表、`Enter` 直接关闭面板。`key` 的表和运行时快捷键同源,改了
内置键位也跟着更新。

> 已移除:`files-gui` 可执行文件及其 OSC 1338 标记、`vectorterm settings` 子命令
> (后者本来就只有文档写过)。Lua `keys` 里的 `"files_gui"` 是**键位动作名**,
> 用于绑定快捷键,不是 shell 命令,仍然保留。

---

## 12 渲染后端与缩放

### 12.1 渲染后端

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `render` | 字符串 / 表 | `"cpu"` | `"cpu"` / `"gpu"` / `"auto"` |

| 值 | 别名 | 行为 |
|---|---|---|
| `"cpu"` | `"software"` | softbuffer + GDI `BitBlt`,**默认** |
| `"gpu"` | `"wgpu"` / `"dx12"` | wgpu(DX12)实例化渲染;初始化失败仍退回 CPU,不会起不来 |
| `"auto"` | — | 先试 GPU,拿不到设备再退回软渲染 |

也接受 wezterm 的表格式:

```lua
render = { backend = "gpu" }
```

**默认为什么是 `cpu`**:GPU 路径在拖动缩放窗口时,新尺寸的交换链还没重建完,
DXGI 会按 `DXGI_SCALING_STRETCH` 把旧尺寸的后缓冲缩放铺满新客户区,看到的效果是
文字被横向拉长一下再弹回。CPU 路径用 `BitBlt` 做 1:1 像素拷贝,没有"缩放"这个环节。
机制对比见 [rendering-backends.md](rendering-backends.md)。

两条路径共享同一份显示列表,字形位置 / 下划线 / 尾迹的几何完全一致,切后端不改版式。

写错值(如 `"vulkan"`)会提示并**保持当前默认**,不会把你悄悄切到会抖的 GPU 路径上。

### 12.2 `gpu_power`:双显卡选哪块

| 值 | 别名 | 说明 |
|---|---|---|
| `"low"` | `"power"` / `"integrated"` | 优先核显,**默认**。终端是常驻窗口,核显省电、发热低 |
| `"high"` | `"performance"` / `"discrete"` | 优先独显。4K / 高刷屏,或核显驱动有问题时用 |

只装了一块显卡时这项没有影响;启动日志会打印实际选中了哪块。

### 12.3 拖动缩放时的行列数

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `live_resize_mode` | 字符串 | `"realtime"` | 拖动中行列数怎么跟随窗口 |
| `resize_debounce_ms` | 整数 | `150` | **非拖动**场景的稳定窗口,夹到 ≤ 1000 |
| `use_resize_increments` | 布尔 | `true` | 窗口按 `cell_w × cell_h` 整格步进缩放 |

| `live_resize_mode` | 别名 | 行为 |
|---|---|---|
| `"realtime"` | `"live"` / `"immediate"` | 每跨过一个字符格就当场同步 PTY,长行实时折行(wezterm 行为,**默认**) |
| `"debounce"` | `"settle"` | 拖动中按 `resize_debounce_ms` 合并,停下才同步 |
| `"freeze"` | `"defer"` | 整个拖动过程冻结行列数,松手一次性同步(拖动中右下留白) |

后两种是 VectorTerm 扩展,给 ConPTY + PSReadLine 重绘太跳的机器留的退路。

`resize_debounce_ms` **只管非拖动**的尺寸变化:程序化改尺寸、最大化、Snap、
`Win`+方向键。`0` = 立即同步。鼠标拖动期间的语义由 `live_resize_mode` 决定,
不受它影响。上限 1000ms 是防手滑写成一个让窗口"卡住"的值。

`use_resize_increments = true` 时,拖边框只在跨过整格时产生 `WM_SIZE`
(实测宽度每 12px 一跳,而不是每 4px),`rows` / `cols` 每次恰好 ±1。
`false` = 允许逐像素缩放。

三项都支持热重载;`render` 与 `gpu_power` 需要重启(热重载只提示不切换)。

### 12.4 变高之后提示符为什么还在底部

ConPTY 把滚动区当成不可变的:窗口一变高,它就在屏幕尾部补一排空行,于是
提示符会停在窗口中间、下面留一大片空白。VectorTerm 在**显示层**把这排空行让给
回看历史 —— 窗口变高时从历史里多露几行,光标所在行(最新的一行)始终贴着
窗口底部;继续输出时视口跟着往下走,和普通终端一致。

缓冲区本身没有改动,所以 ConPTY 的行号与画面始终对齐(这一点很关键:改了
缓冲区会让之后输出的文字落到历史行上)。回看状态下调整窗口,视口顶行保持
不动,正在读的内容不会被拽回底部。全屏应用(备用屏)没有回看历史,不参与
这个补偿。

这项行为没有开关。

---

## 13 自定义背景

一组**与 wezterm 同名**的键,直接搬 wezterm 配置也能用。图片的解码、降采样、
HSB 调整、模糊都在**加载时做一次**,产出的位图同时交给 CPU 与 GPU 两条路径,
所以切换后端背景不会变样。

### 13.1 配置项

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `window_background_color` | 颜色 | 无 | 纯色背景,覆盖 `colors.background` |
| `window_background_image` | 字符串 | 无 | 背景图路径,相对**配置文件所在目录** |
| `window_background_image_hsb.brightness` | 数字 | `1.0` | 亮度乘数,`0` = 全黑,`0.45` 压暗一半多 |
| `window_background_image_hsb.hue` | 数字 | `1.0` | 色相乘数,`1.0` = 不变 |
| `window_background_image_hsb.saturation` | 数字 | `1.0` | 饱和度乘数,`0` = 灰度 |
| `window_background_blur` | 数字 | `0` | 模糊半径(相对图片原始像素),夹到 0–200 |
| `window_background_image_blur` | 数字 | `0` | 上面那项的别名,两个都写时后者生效 |
| `window_background_fill` | 字符串 | `"cover"` | 缩放 / 填充方式,见 13.3 |
| `window_background_opacity` | 数字 | 无 | 整窗不透明度 0–1,夹到区间 |
| `win32_system_backdrop` | 字符串 | `"none"` | Win11 系统背板,见 13.5 |
| `window_background_gradient` | 表 | 无 | 渐变,见 13.4 |

```lua
window_background_color = "#101418"

window_background_image = "bg/wallpaper.jpg"
window_background_image_hsb = { brightness = 0.45, hue = 1.0, saturation = 0.9 }
window_background_blur = 12
window_background_fill = "cover"
window_background_opacity = 0.92
```

### 13.2 图片格式与分辨率

- 解码走 `image` crate:PNG / JPEG / WebP / BMP / GIF(取首帧)/ TGA / TIFF 等常见格式都能读。
- 图片最长边超过 **2560** 会先等比降采样再处理(背景最终只铺到窗口大小,
  再大只是多占内存和模糊时间);模糊半径按同一次缩放折算,视觉半径不变。
- 读取失败只打印原因并退回纯色,不会挡住启动。
- 空串、`"nil"` 都视为没配。

### 13.3 `window_background_fill`:五种填充

| 值 | 别名 | 行为 |
|---|---|---|
| `"cover"` | `"fill"` | 等比放大到完全覆盖窗口,超出部分居中裁掉(**默认**,等同 wezterm) |
| `"contain"` | `"fit"` | 等比缩到整张图都可见,四周露出底色 |
| `"stretch"` | — | 拉伸铺满,不保持宽高比 |
| `"tile"` | `"repeat"` | 按原始像素尺寸平铺(小图当纹理) |
| `"center"` | `"none"` | 原始像素尺寸居中,不缩放也不平铺 |

wezterm 只有 `cover` 一种行为;其余几个是 VectorTerm 的扩展,因为背景图的分辨率和
窗口比例经常对不上。写错值会提示并保持 `cover`。

### 13.4 `window_background_gradient`

```lua
-- 简写:竖向 / 横向
window_background_gradient = {
  colors = { "#0b1020", "#1b2a4a" },
  orientation = "Vertical",     -- 或 "Horizontal"
}

-- wezterm 的完整写法:角度 90° 视为竖向
window_background_gradient = {
  colors = { "#0b1020", "#1b2a4a" },
  orientation = { Linear = { angle = 90.0 } },
}
```

- `colors` 至少要有 2 项,只取前两项(起点 / 终点);
- `orientation` 不写默认竖向;`"Horizontal"`(大小写不敏感)才是横向;
- 只支持线性两色渐变,不支持 wezterm 的多色标;
- **图片存在时渐变画在图片下面**,只有图片半透明才透得出来。

### 13.5 `win32_system_backdrop`:Win11 毛玻璃

| 值 | 别名 | 对应 DWM 材质 |
|---|---|---|
| `"none"` | `"disable"` / `"off"` / `""` | 不启用 |
| `"mica"` | `"mainwindow"` | `DWMSBT_MAINWINDOW` |
| `"acrylic"` | `"transientwindow"` / `"blur"` | `DWMSBT_TRANSIENTWINDOW` |
| `"tabbed"` | — | `DWMSBT_TABBEDWINDOW` |
| `"auto"` | — | 等同 `mica` |

> **当前实现只设 DWM 属性,材质透不出来**。原因:材质要靠"窗口客户区对 DWM
> 逐像素透明"才看得见,而窗口交换链(`CreateSwapChainForHwnd`)不接受预乘 alpha。
> 配了会在启动日志里明确提示。**想要半透明请用 `window_background_opacity`。**

### 13.6 `window_background_opacity`:整窗半透明

实现方式是"层叠窗口 + 统一 alpha":DWM 把整块窗口按这个 alpha 合成,
GDI 位图(CPU 后端)和 DXGI 画面(GPU 后端)都会跟着变淡,所以两个后端行为一致,
不需要逐像素 alpha。

**注意它作用于整个窗口,包含文字** —— `0.85` 是"隐约透出背后的窗口",
`0.3` 会让文字也跟着难读。**不写这项就是完全不透明(`1.0`)**,即使同时配了
`win32_system_backdrop` 也不会自动变透明 —— 想让毛玻璃透出来必须自己把 `opacity`
调低,而由于 13.5 说的交换链限制,目前即使调低也只会得到"整窗变淡"而不是真毛玻璃。

这些项都在**启动时**应用,改完要重启。

---

## 14 终端图片协议

iTerm2(`OSC 1337;File=...`)、Kitty(`APC G`)与 sixel 三条协议都由 `wezterm-term`
解析成"挂在单元格上的图",渲染侧一视同仁(每格一块落格,GPU 每图一条纹理)。

### 14.1 配置项

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `images.enable` | 布尔 | `true` | `false` = 完全不认两类报文,按普通控制序列丢弃 |
| `images.max_memory_mb` | 整数 | `256` | 解码后位图的内存预算(MB),夹到 8–4096,超出按最久未用淘汰 |

```lua
images = {
  enable = true,
  max_memory_mb = 256,
}
```

### 14.2 `enable = false` 的效果

`enable = false` 时 Kitty 协议**不回响应**,应用会自己走降级路径(通常退回文字描述),
iTerm2 / sixel 的图也不再绘制。报文被当普通控制序列丢掉,**不会**出现错位或乱码。

### 14.3 `max_memory_mb` 的量级

CPU 侧常驻 RGBA8,GPU 侧另有同等量级的纹理。`256` 大约留住十几张 1080p 图;
`8` 是下限(约一张大图),`4096` 是上限。

常见用法:`wezterm imgcat foo.png`、`chafa -f kitty`、`timg -p kitty`、`lsix`、
`catimg -i`,以及 neovim / yazi 里走 Kitty 协议的预览插件。

`enable` 支持热重载;`max_memory_mb` 在启动时建缓存,改完要重启。

---

## 15 键位映射

`keys` 把"键位组合"映射到内置动作字符串或 Lua 函数。

> 想一眼看全所有快捷键与鼠标操作,见 **[键位速查文档](keyboard-shortcuts.md)** —— 命令面板、
> 分屏、标签、剪贴板、回看、文件浏览器逐类列出,与命令面板的 `key` 命令输出同源。

### 15.1 字典写法(VectorTerm 原生)

```lua
keys = {
  ["ctrl+shift+r"] = "reload",
  ["ctrl+shift+q"] = "quit",
  ["f12"]          = "send:dir\r",
  ["ctrl+alt+t"]   = function() VectorTerm.notify("hi") end,
}
```

- 键名**大小写无关、修饰键顺序无关**:`shift+ctrl+t` 等价 `ctrl+shift+t`。
- 值为 `"send:..."` 时,后面是**字面量**。想回车执行必须写 `\r`(Lua 转义成 CR)。
- 同一个组合写两次,后写的覆盖先写的。

### 15.2 数组写法(wezterm 风格)

```lua
keys = {
  { key = 'T', mods = 'CTRL|SHIFT', action = wezterm.action.SpawnTab 'DefaultDomain' },
  { key = 'W', mods = 'CTRL|SHIFT', action = wezterm.action.CloseCurrentTab },
  { key = 'Tab', mods = 'CTRL',     action = wezterm.action.ActivateTabRelative(1) },
  { key = '1',   mods = 'CTRL',     action = wezterm.action.ActivateTab(0) },
}
```

- `mods` 支持 `CTRL` / `CONTROL`、`SHIFT`、`ALT` / `META`、`SUPER` / `CMD` / `WIN`,用 `|` 连接;
- `action` 可以是内置动作字符串,或 `wezterm.action.*` 构造的表;
- 这种写法里**不能**直接塞 Lua 函数(动作会被解析成内置动作,函数项忽略)。

### 15.3 内置动作

| 字符串 | 作用 |
|---|---|
| `"paste"` | 粘贴剪贴板 |
| `"copy"` | 复制选区 |
| `"reload"` | 重载配置 |
| `"quit"` | 退出程序 |
| `"new_tab"` / `"spawn_tab"` | 新建标签 |
| `"close_tab"` | 关闭当前标签 |
| `"close_pane"` | 关闭当前窗格(只剩一个窗格时等同关标签) |
| `"next_tab"` / `"prev_tab"` | 下一个 / 上一个标签 |
| `"send:<文本>"` | 向 shell 发送字面量文本 |

未知动作会打印 `[VectorTerm] 未知的键位动作:...` 并且**不绑定**(按下时走原行为)。

### 15.4 wezterm 动作映射

| `wezterm.action.*` | 映射到 |
|---|---|
| `ReloadConfiguration` | `reload` |
| `SpawnTab` | 新建标签(参数忽略) |
| `CloseCurrentTab` | 关闭当前标签 |
| `CloseCurrentPane` | 关闭当前窗格(参数 `confirm` 忽略:确认由 `quit_confirmation` 控制) |
| `ActivateTabRelative(n)` | `n < 0` → 上一个;否则下一个 |
| `ActivateTab(n)` | 第 n 个标签(0 起) |
| `ActivateLastTab` | 回到上次的标签 |
| `CopyTo` / `PasteFrom` | 复制 / 粘贴 |
| `ToggleFullScreen` | 全屏开关 |
| `SendString(s)` | 发送字符串 |
| `Nop` | 什么都不做 |

其余动作(缩放字体、改窗口标题等)会打印"暂不支持的 wezterm 动作"后忽略。

> 分屏动作(`SplitPane`)没有独立的 Lua 动作名,是内置快捷键直连,见
> [§15.7 分屏](#157-分屏pane)。不过内置键位本身仍能被 `keys` **拦截覆盖**
> (按 [§15.6 派发顺序](#156-派发顺序),`keys` 最先派发)。键名里带引号,
> 用 Lua 单引号字符串写 `['ctrl+alt+shift+"'] = function() ... end`
> 就能接管左右分屏键;只是没法把分屏动作本身绑到另一个键上。
>
> **关闭窗格是例外**:它有独立动作名,可以绑到任意键 ——
> `keys = { ["ctrl+alt+w"] = "close_pane" }`,wezterm 风格则是
> `{ key = 'W', mods = 'CTRL|ALT', action = wezterm.action.CloseCurrentPane }`。
> 内置的 `Ctrl+Shift+X` 仍然保留,加一条就是多一个键,不会冲突。

### 15.5 可用的键名

| 类别 | 可用值 |
|---|---|
| 字符键 | 单个字母 / 数字 / 符号,如 `a`、`1`、`/` |
| 编辑键 | `enter` `escape` `tab` `space` `backspace` `delete` `insert` |
| 方向键 | `up` `down` `left` `right` `home` `end` `pageup` `pagedown` |
| 功能键 | `f1` … `f24` |
| 修饰键 | `ctrl` `alt` `shift` `super` |

wezterm 的 `LeftArrow` / `RightArrow` / `UpArrow` / `DownArrow` 会自动归一成
`left` / `right` / `up` / `down`。

带 `shift` 的字母键:`Shift+A` 产生的组合是 `shift+a`(键名取小写),
所以写 `["ctrl+shift+t"]` 就能命中 `Ctrl+Shift+T`。

### 15.6 派发顺序

按键按下时依次尝试:

1. 配置 `keys_file`(见 [§15.11](#1511-键位方案文件keys_file))指向的外部键位
   文件里的 `keys` 绑定(**基础层**);
2. 配置内联 `keys` 里的绑定(**覆盖层**,命中就 `return`,不再往下走);
3. 内置标签栏快捷键(`Ctrl+Shift+T/W`、`Ctrl+Tab`、`Ctrl+1..9`);
4. 内置分屏快捷键(`Ctrl+Shift+Alt+"` / `%` 及它们的别名、`Ctrl+Shift+方向键`,
   完整列表见 [§15.7](#157-分屏pane));
5. 内置剪贴板快捷键(`Ctrl+Shift+C/V`、`Ctrl+Insert`、`Shift+Insert`);
6. `Shift+PageUp` / `Shift+PageDown` 本地回看翻页;
7. 交给终端状态机编码,写进 PTY。

所以 `keys`(无论是外部文件还是配置内联)可以覆盖内置快捷键 —— 比如把
`ctrl+shift+t` 改成别的动作,新建标签就只能用鼠标点 `+` 了。

**未列出的组合键一律退回终端内置默认键位**:外部文件 / 内联 `keys` 只对写出来的
组合键生效,其余照常走上面的第 3~7 步。所以"只改某几个键、其余保持默认"天然
成立,不用把完整键位表抄一遍。

### 15.7 分屏(pane)

VectorTerm 支持 wezterm 同款分屏:一个标签里并排多个终端会话,每个 pane 独立
持有自己的 shell / 滚动缓冲 / 选区。以下任一组合都能分屏,按不顺手的那组用:

| 快捷键 | 动作 |
|---|---|
| `Ctrl+Shift+Alt+"` | 左右分屏(中间一条竖杠,新 pane 在右半) |
| `Ctrl+Shift+"` | 同上,不按 Alt |
| `Ctrl+Shift+Alt+V` | 同上,纯字母别名 |
| `Ctrl+Shift+Alt+%` | 上下分屏(中间一条横杠,新 pane 在下半) |
| `Ctrl+Shift+%` | 同上,不按 Alt |
| `Ctrl+Shift+Alt+H` | 同上,纯字母别名 |
| `Ctrl+Shift+方向键` | 把焦点移到相邻 pane(按几何最近原则) |
| `Ctrl+Shift+X` | 关掉当前 pane(只剩一个时等同关标签;有程序在跑时先弹确认) |
| `Ctrl+Shift+E` | 打开 / 收起文件浏览器侧边栏(见 [§15.10](#1510-文件浏览器)) |
| `Ctrl+Shift+Alt+方向键` | 调整 pane 大小(移动分割线,详见 [§15.8.7](#1587-拖动分割栏与快捷键调整窗格大小)) |

> `"` 是 `Shift+'`、`%` 是 `Shift+5`,按物理键即可,不用真的去按出双引号。
> 别名不是重复功能:`Ctrl+Shift+Alt` 常被中文输入法、显卡驱动或
> PowerToys 之类的全局热键拦掉,这时用不带 Alt 的组合;懒得找符号键
> 就用 `V` / `H`。

**实现上按物理键判定,不看字符。** Windows 把 `Ctrl+Alt` 当 AltGr,winit
在这个组合下给不出 `"` / `%`(只给控制字符或 `Unidentified`),只看
`logical_key` 的旧实现因此完全失效;现在读 `physical_key`(`Quote` /
`Digit5`),并用 `GetAsyncKeyState` 补上被 AltGr 过滤掉的 Ctrl/Alt 状态。
`keys` 表里的键名生成也做了同样的兜底,所以现在能正常覆盖这些键。

分屏动画:新 pane 像抽屉一样滑入 —— 左右分屏从**右边**拉开(分割线向左走),
上下分屏从**下边**顶上来(分割线向上走),180ms ease-out,最后停在窗口中央。
原 pane 的内容在动画**开始**时就已按最终尺寸重排好,滑入过程只是把分割线和
各自的矩形挪过去,所以停在中间那一帧不会再跳一下(旧实现等动画结束才 resize,
收尾时 26 行突然变 12 行,内容整块重排)。动画没跑完就再分屏 / 关 pane 时,
旧的那条线会先落到中线,不会卡在半路。

其余行为:

- 关掉一个 pane(`Ctrl+Shift+X`、Lua 的 `close_pane`,或 shell 自己退出)时,
  剩余 pane 自动铺满;最后一个 pane 关闭则连标签一起关。主动关闭时如果这个
  pane 里还有程序在跑,会先弹 `● Really kill this pane?`(见
  [§4.7](#47-quit_confirmation关窗--关标签--关窗格前确认));
  会话自己退出则不弹 —— 进程都没了,再问只会挡路。
- 焦点 pane 的标签栏标题、状态栏等跟随 `active_pane` 变化。
- 分屏快捷键在内置标签栏快捷键之后派发,`keys` 里可以覆盖它们。

### 15.8 分割线外观

两个 pane 的文字区之间留一道**缝**,分割线画在缝的正中央;缝宽由
`split_divider.gap` / `text_gap` 决定。默认只留 **1px 空气**(`缝宽 = 线宽 + 2`),
文字几乎贴着线 —— 和 kitty / wezterm 的分屏观感一致,也不会随字号、DPI 变宽。
分割线永远落在**每次分割的几何中线**上,两侧 pane 分到的像素宽度严格相等 ——
不论窗口是奇数还是偶数格、嵌套分了几层,线都在正中间。`split_divider`
决定的是**这根线画成什么样**:多粗、两端离边界多远、单色还是渐变。

不写这一节时的默认外观:自动间距 + 3px `#323236` 细线。想回到旧版
「整格缝填满粗灰条」,显式写 `thickness = 10`(或你的字符宽度)即可。

所有值都是**逻辑像素**,改完保存配置即生效(见 [§18.1](#181-即时生效不用重启、不用新开标签)),
不用重启、不用新开标签。配置管理器的「分屏与分割线」分组里有实时预览。

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `split_divider.enabled` | bool | `true` | `false` = 不画线,缝仍在(露出窗口背景) |
| `split_divider.thickness` | number / 不写 | 不写 | 线宽。不写 = **自动 3px 细线**;`0` = 隐藏;允许大于缝宽(会盖住两侧文字) |
| `split_divider.gap` | number / `"Auto"` | `Auto` | 两侧文字区之间的缝宽(px)。`Auto` = **线宽 + 2px**(即文字到线 1px),不随字号缩放 |
| `split_divider.text_gap` | number | 无 | 文字 / 光标到线**边缘**的距离(px),`0` = 贴着线。设置后覆盖 `gap`:缝宽 = `2 × text_gap + 线宽`;范围夹到 0..256 |
| `split_divider.inset_start` | number | `0` | 起点端留白:竖线离**上**边界、横线离**左**边界的距离 |
| `split_divider.inset_end` | number | `0` | 终点端留白:竖线离**下**边界、横线离**右**边界的距离 |
| `split_divider.inset` | number | 无 | 两端相同的简写,随后可被 `inset_start` / `inset_end` 分别覆盖 |
| `split_divider.color` | string / 数组 | `"#323236"` | 单色(HEX / wezterm 颜色名);给数组则是沿线渐变的简写 |
| `split_divider.gradient.colors` | 数组 | 无 | 渐变色停靠点,**2 个以上**才生效,顺序 = 沿线从起点到终点 |
| `split_divider.gradient.orientation` | string | `"Auto"` | `Auto` / `Vertical` / `Horizontal`,也认 wezterm 的 `{ Linear = { angle = 90 } }` |
| `split_divider.opacity` | number | `1.0` | 0..1,夹到范围内 |

> 留白量的是**内容区边界**(已扣掉 `padding` 和标签栏),不是窗口外框;
> 两端留白加起来超过线的长度时就不画线,不会产生负尺寸矩形。
> `gap` / `text_gap` 会改变两侧 pane 的可用列数 / 行数(PTY 会跟着 resize);
> `thickness` 只改变线的视觉粗细。缝宽至少等于线宽(线最多贴到文字边、
> 不会压字),极窄的 pane 会自动把缝收窄到该区域的 1/6,所以 `text_gap`
> 设得特别大时,窄 pane 里的实际距离会小于设定值。

#### 15.8.1 粗细

```lua
-- 一条 2px 的细线,居中浮在缝里
split_divider = { thickness = 2 }
```

#### 15.8.2 两端留白(长度)

```lua
-- 竖线上下各留 40px,做成「悬空」的短分隔;横线则是左右各留 40px
split_divider = { thickness = 3, inset = 40 }

-- 两端不对称:顶部 16px,底部 80px
split_divider = { thickness = 3, inset_start = 16, inset_end = 80 }
```

#### 15.8.3 颜色:单色与渐变

单色直接给字符串;渐变给 2 个以上停靠点,两端之间等距插值(8bit 精度,
最多切 256 段,CPU / GPU 两个后端效果一致)。

```lua
-- 单色
split_divider = { thickness = 2, color = "#7aa2f7" }

-- 两色渐变(简写:color 直接给数组)
split_divider = { thickness = 3, inset = 24, color = { "#ff6b6b", "#4ecdc4" } }

-- 三色渐变 + 指定方向(完整写法)
split_divider = {
  thickness = 4,
  inset_start = 12,
  inset_end = 12,
  gradient = {
    colors = { "#ff5f6d", "#ffc371", "#45b7d1" },
    orientation = "Vertical",   -- Auto(默认) / Vertical / Horizontal
  },
  opacity = 0.9,
}
```

`Auto` = **沿分割线本身**:左右分屏的竖线从上往下渐变,上下分屏的横线从左往右渐变。
显式写 `Vertical` / `Horizontal` 可以反过来(例如让 4px 宽的竖线在**横向**上变色)。
渐变里的颜色解析不出来时跳过该停靠点,一个都不剩就退回 `color` 单色。

#### 15.8.4 隐藏与半透明

```lua
split_divider = { enabled = false }        -- 完全不画线
split_divider = { thickness = 0 }          -- 等价写法
split_divider = { opacity = 0.45 }         -- 半透明
```

#### 15.8.5 在配置管理器里调

命令面板里输入 `settings`(或 `Ctrl+Shift+P` 后选它)打开配置管理器,中间栏的「分屏与分割线」分组顶部有
两张缩略图(左右分屏 / 上下分屏),调粗细、留白、渐变色停靠点时**即时重画**;
颜色项点色块开拾色器,渐变停靠点用「+ 添加」逐项加、各自也能点色块调色。
分组顶部还有一条 **「文字到分割线的距离」** 滑杆(`split_divider.text_gap`):
拖 0~24px 实时改两侧留白,旁边 `0 / 2 / 4 / 8` 是常用档位,勾「自动」等于
不写这个键。滑杆下方会直接算出换算后的缝宽(缝宽 = 2×距离 + 线宽)。
预览按 400px 内容区等比缩放留白,实际长度以终端窗口为准。
预览里只有左半 pane 画了一个输入光标、右半不画 —— 这就是 `pane_focus.hide_inactive_caret`
的效果;关掉它,右半也会出现一个变暗的 caret。

#### 15.8.6 文字 / 光标到分割线的距离

旧版固定留一整格(约 10px)的粗缝,文字被推得太远;后来的版本虽然改成
`0.7 × 字符宽`,但字号一大(或高 DPI)缝又跟着变宽。现在的默认是**固定
1px 空气**,文字几乎贴着线,和字号 / DPI 无关:

```lua
-- 默认(与不写等价):缝宽 = 线宽 + 2px,即文字到线边缘 1px
split_divider = { gap = "Auto" }

-- 直接指定两侧文字之间的总缝宽
split_divider = { gap = 12 }

-- 更符合直觉:指定文字到线边缘的距离,线宽自动算进去
split_divider = { text_gap = 6 }         -- 缝宽 = 2×6 + 3 = 15px

-- 三者一起:细线 + 大留白 + 让文字几乎贴着线
split_divider = { thickness = 2, text_gap = 0, color = "#7aa2f7" }
```

优先级:`text_gap` > `gap` > `Auto`。`Auto` 现在只看**线宽**,不再看字符宽高,
所以换字号、换显示器都不用重新调;`text_gap = 0` 让文字直接贴到线边缘。
分割线以缝中心为轴,所以两侧文字到线的距离**永远相等**。

配置管理器里不用写 Lua:分组顶部的「文字到分割线的距离」滑杆直接拖 0~24px,
`0 / 2 / 4 / 8` 是常用档位,勾「自动」就回到 1px 空气。改这个值会改变两侧
pane 的可用列数 / 行数,PTY 会跟着 resize(正在跑的程序会收到窗口尺寸变化,
`vim`、`htop` 这类全屏程序会重排)。

文字只能按整格排,`cols × 字符宽` 之外剩下的不足一格像素,现在统一放到
**远离分割线**的一侧(左 pane 留在左边、右 pane 留在右边)。所以有长行时,
两侧文字都会**紧贴分割线**——默认只剩 1px(半个缝宽减半个线宽),不会出现
"左边空出近一格、右边却贴着线"的不对称。

多个窗格嵌套时,**横纵两个方向各自对齐**:竖直分割只决定左右留白、水平分割
只决定上下留白,互不干扰。所以三窗格(左一整条 + 右侧上下)里,右上的文字
下缘、右下的文字上缘都贴着同一条横线,不会因为"要贴右边"而多空出近一格。

#### 15.8.7 拖动分割栏与快捷键调整窗格大小

分割栏不只是装饰,它是**窗格大小调整**的手柄:

- **鼠标**:把指针移到分割线上(默认向两侧各膨胀 8px 好抓),光标变成左右 /
  上下调整形状,按住左键拖动即可实时改变两侧占比,松手即定。拖动时两侧 PTY
  跟着 resize,`vim` / `htop` 这类程序会重排。
- **快捷键**:`Ctrl+Shift+Alt+←/→/↑/↓` 每按一次移动一步(默认 5%)。方向是
  **分割线的移动方向**:`→` 让左(上)侧变宽,`←` 变窄;上下同理,和鼠标拖动
  完全同一套语义。嵌套分屏时自动挑「包含当前 pane、且方向与按键同轴」的
  最深那一条分割线。
  > 不要改成 `Ctrl+Alt+方向键`:那是部分显卡驱动「旋转屏幕」的全局热键,
  > keydown 会被系统吃掉,窗口根本收不到。

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `split_divider.draggable` | 布尔 | `true` | 允许鼠标拖动;关掉后快捷键仍可用 |
| `split_divider.hit_width` | 数字 | `8` | 热区半宽(px),只影响命中,不影响布局 / 文字距离 |
| `split_divider.min_ratio` | 数字 | `0.08` | 任一侧最小占比,拖动和快捷键都夹住 |
| `split_divider.step` | 数字 | `0.05` | 快捷键每次移动的占比 |

分割线初始精确落在区域中线(`ratio = 0.5`);拖动时直接改布局树里对应的那个
Split 节点,渲染、鼠标命中、PTY 列数全部走同一份 `pane_geoms()`,不会跑偏。

### 15.9 分屏焦点

| 操作 | 效果 |
|---|---|
| 鼠标点进某个 pane | 聚焦该 pane;这一次点击仍照常开始选区 / 转发给应用 |
| `Ctrl+Shift+方向键` | 焦点移到该方向上几何最近的 pane |
| `Alt+1..9` / `Ctrl+Shift+Alt+1..9` | 直接聚焦第 N 个 pane(含小键盘) |
| `Ctrl+1..9` | **仍是切标签**,不受影响 |

**只有聚焦的 pane 显示输入光标**:其余 pane 的 caret 一律隐藏,连 kitty
光标尾迹一起收掉。焦点靠这唯一的光标表达,不再额外画窗格描边。

切换焦点的时候,**新 pane 的输入光标立刻出现在它的真实位置** —— 和窗格内敲回车
完全一样,光标本体不做滑行。要「从那边飞过来」的观感,全部交给**一条跨窗格
尾迹**:它被「种」在**旧 pane 的光标格**上,然后按和窗格内尾迹**同一套状态机**
收拢到新光标上 —— 不是另外写的一套飞行动画:

* 同一个四角指数追赶积分器 —— 运动前方的角先归位、后方的角留在原地,四边形被
  拉长成一条尾巴,前后缘始终连成一条**直线**四边形;
* 同一个 `CursorTrail`,所以停留门限 `trail_dwell_ms`、起止阈值
  `trail_start_x/y`、硬上限 `cursor.trail_max_ms`、淡入曲线全都在;
* 同一个颜色来源(默认 `cursor.trail_color`,没配就取终端光标色);
* 和窗格内尾迹一样,**当前光标格会被挖空**,光标本体保持清晰。

跨屏的距离是窗格内的几十倍,所以两处额外锁死了参数,不然「一闪就没」:

* **倍率锁在 `1.0`**,不套距离自适应 —— 和「窗格里挪一两行」同一个节奏,只是
  拉得更长。距离自适应会在几十格的位移上把倍率顶到 4 倍以上,四边形一两帧就收完;
* **单帧推进量收到 `1/60` 秒** —— 切换那一帧如果刚好掉帧,一次几十毫秒的 `dt`
  会让四边形一步到位,屏幕上一个像素都看不到。

这条尾迹画在**所有 pane 之上**,不受单个 pane 的裁剪区限制,所以从右 pane 划到
左 pane 时是一整条穿过去的。

`trail_ms` 是它的**时长倍率**(相对 `cursor.trail_*` 的原生节奏):`320` = 和窗格内
尾迹一模一样,`640` = 慢一倍,`160` = 快一倍,`0` = 关掉。上限 5000。

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `pane_focus.click_to_focus` | bool | `true` | 关掉后点别的 pane 不抢焦点(仍可框选内容) |
| `pane_focus.number_shortcuts` | bool | `true` | 关掉 `Alt+数字` 聚焦 |
| `pane_focus.hide_inactive_caret` | bool | `true` | 非聚焦 pane 隐藏输入光标;关掉则每个 pane 都画 caret |
| `pane_focus.trail` | bool | `true` | 切换焦点时播跨窗格尾迹 |
| `pane_focus.sync_trail` | bool | `true` | 速度 / 尾迹颜色 / 不透明度全部沿用 `cursor.trail_*`;关掉才会用 `trail_color` |
| `pane_focus.trail_ms` | number | `320` | 尾迹时长倍率(基准 `320` = 与窗格内尾迹同节奏),`0` = 关闭;上限 5000 |
| `pane_focus.trail_color` | string | 无 | 仅 `sync_trail = false` 时生效;不写 / `"Auto"` / `"none"` = 跟随终端光标色 |
| `pane_focus.landing` | string | `"caret"` | **已废弃**:光标本体永远落在真实输入光标处。仍然解析,但不产生效果(旧配置不报错) |

```lua
pane_focus = {
  click_to_focus = true,
  hide_inactive_caret = true,  -- 只有聚焦 pane 显示输入光标
  number_shortcuts = true,
  trail = true,
  sync_trail = true,           -- 速度 / 颜色 / 透明度跟随 cursor.trail_*
  trail_ms = 320,              -- 320 = 与窗格内尾迹同节奏;640 = 慢一倍
  -- trail_color = "#ffcc00",  -- 仅 sync_trail = false 时需要
}
```

> 分屏只有一个 pane 时不播跨窗格尾迹(本来就没有"从哪个 pane 过来"这回事)。
> `keys` 表优先级最高,想改快捷键可以在 `keys` 里覆盖 `alt+1` 等组合。

### 15.10 文件浏览器

三种进入方式,同一个面板:

| 入口 | 形态 |
|---|---|
| 命令面板 `/files` | **铺满宿主 pane**:那个窗格整个变成文件列表(分屏时只盖当前 pane,另一边照常显示终端) |
| `Ctrl+Shift+E` | **侧边栏**:从左边分出一条窄栏,终端被推到右边并重新排布 |
| `keys` 里绑 `"files_gui"` | 和快捷键等价(开 / 换焦点 / 收起) |

命令面板入口是 `Ctrl+Shift+P`(或 `Ctrl+Shift+F` 后输入 `/`),选中 `files`
回车即可;起始路径取自**当前 pane 的 cwd**(OSC 7 / 进程工作目录),和
`Ctrl+Shift+E` 同源。旧的 `files-gui` shell 命令已移除,详见 [§11.6](#116-命令面板替代-shell-内命令)。

面板里的键位:

| 按键 | 效果 |
|---|---|
| `↑` / `↓` / `PgUp` / `PgDn` / `Home` / `End` | 移动选择 |
| `Enter` / `→` | 进入目录;选中文件则用编辑器打开 |
| `←` / `Backspace` | 回到上级目录(并把刚才那一层选中) |
| `.` | 显示 / 隐藏隐藏文件 |
| `r` | 重新读目录 |
| `Esc` / `q` | 关闭面板(侧边栏关闭后终端恢复全宽) |
| 鼠标左键 | 点一下选中;在同一行再点一次 = 打开 |
| 鼠标滚轮 | 滚列表(不抢键盘焦点) |

点击终端区域会把键盘交还给 shell;再按 `Ctrl+Shift+E` 可以把焦点拉回列表。

**进出场是滑的,不是"啪"地出现**:面板从自己宽度的左侧滑进来(190ms、缓出),
收起时 160ms 滑回左边。两种入口(侧边栏 / 全屏)都有;全屏模式是滑过终端,
侧边栏模式滑进来盖住的正是终端让出来的那条空档。几处实现上的取舍:

* 和搜索栏 / 命令面板共用同一份 `OverlayAnim`(纯时间函数,`t: 0..1`),只是把
  "竖直位移 + 缩放"换成横向位移 —— 所以开关中途反悔(开了立刻按 `Esc`)也不跳变;
* 面板宽度不变、整块平移,文字不重排;右边那条竖线跟着面板一起滑;
* 滑动期间绘制被裁在自己的矩形里(`Frame::clip_x`,CPU 夹 x 区间、GPU 收
  scissor),否则分屏时半块面板会糊到左邻窗格的终端上;
* 退场期间面板**逻辑上已经关了** —— 按键、鼠标、pane 宽度全部按"关闭"处理,
  只有像素还在往左滑,放完才把列表数据丢掉;
* 终端在打开的一瞬间就按新宽度重排(PTY 只 resize 一次,不会在动画里反复
  `SIGWINCH`),所以看到的是"终端先让开、面板滑进来盖住空档"。

**面板属于某个 pane,而不是属于整个窗口**。在哪里打开,它就只铺在哪个 pane 的
矩形里 —— 分屏之后在右半边执行 `/files`,左边那个终端的画面分毫不动:

* 全屏模式铺满**宿主 pane**,侧边栏模式只从**宿主 pane 的左边**分出一条窄栏,
  被让开宽度的也只有宿主 pane;
* 已经开着面板时点到另一个终端,**面板跟过去**:焦点移交,列表换成那个 pane 的
  cwd,当前选中行保持不变。不用关掉再重开;
* 宿主 pane 被关掉时面板一起收掉(不会留一个没有主人的浮层);
* `/files` 命令作用在**当前聚焦的 pane**上。

**打开文件**:回车那一刻按 `files_gui.open_mode` 决定编辑器跑在哪儿,默认是
**新开一个窗口**:

| `open_mode` | 行为 |
|---|---|
| `"window"`(默认) | **另起一个 VectorTerm 窗口**跑编辑器。新窗口是独立进程、独立 PTY,不动当前窗口的分屏和 shell;关掉它不影响原来的终端,当前窗口只是把面板收起来 |
| `"split"` | 在宿主 pane 里**向右分出一个 pane** 跑编辑器,编辑器退出后这个 pane 自动收起 |
| `"current"` | 把命令**写进宿主 pane 的 shell**,和你自己敲 `nvim 文件` 一样;编辑器退出后回到原来的 shell,`:e`、`:Ex`、`:cd` 都在这个终端的 cwd 里 |

新窗口 / 分屏模式下,新终端的工作目录就是面板当前的目录(`nvim` 里 `:Ex`
看到的就是你刚才浏览的那一层),命令行里带空格 / 中文的路径也不会被拆开。
`window` 模式里编辑器退出时**新窗口自己关掉**(`exit_behavior = "Close"` 默认;
写成 `"Hold"` 则留下最后一屏),当前窗口完全不受影响。
`split` 模式会**先收起面板再分屏** —— 面板和终端共用同一块文字区,留着它新
pane 只能拿到"侧边栏之外那一半的一半",编辑器会被挤成一条窄缝。
`current` 模式同样先收起面板(它和终端共用同一块文字区),所以铺满 pane 的
列表按回车后,编辑器直接占满这个终端。

`current` 模式按目标 shell 的引用规则拼命令:cmd 用双引号,PowerShell 用
`& '...'`。两种情况不盲写,只给面板底部英文提示:宿主 pane 已经结束(这时才
退回「新分屏」),或前台正跑着全屏程序(vim / less,盲写会把命令当成按键)。
`window` 模式没有这两个限制 —— 它根本不碰当前 pane。

编辑器**先确认能跑起来再动手**:配置里写了就看它是否在 `PATH`(或常见安装
位置)里,没写就按 `nvim` → `vim` 探测。探测不到时不会把一条不存在的命令写进
终端(那只会得到 `'nvim' 不是内部或外部命令`),而是在面板**底部**留一行黄色
英文提示,列表和焦点都原地不动:

```text
"nvim" not installed or not in PATH      # 配置里指定的编辑器找不到
nvim not installed or not in PATH (using notepad)   # 一个 vim 系都没有,退回记事本
```

照提示装好编辑器、或把它加进 `PATH`、或在配置里写绝对路径即可;下次回车会重
新探测(只缓存成功的结果)。

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `files_gui.open_mode` | string | `"window"` | 编辑器跑在哪儿:`"window"` 新开一个窗口 / `"split"` 右侧分屏 / `"current"` 当前终端的 shell。写错时保留默认(新窗口) |
| `files_gui.editor` | 数组 / 字符串 | 无(自动) | 编辑器命令:第一个是程序,其余是固定参数。不写则探测 `nvim` → `vim`,都没有再退回记事本。程序找不到时面板底部给英文提示,不会把命令写进 shell |
| `files_gui.side_cols` | number | `30` | 侧边栏宽度(列),夹到 8..120;窄于 24 列时不显示文件尺寸 |
| `files_gui.show_hidden` | bool | `false` | 打开时是否列出隐藏文件(面板里按 `.` 可随时切换) |
| `files_gui.follow_cwd` | bool | `true` | 面板跟随终端:shell 里 `cd` 之后面板自动切到那个目录 |
| `files_gui.push_cwd` | bool | `true` | 反向同步:面板里进目录 / 回上级时,把 `cd` 写进所属终端的 shell |

```lua
files_gui = {
  open_mode = "window",    -- "window"(默认)/ "split" / "current"
  editor = { "nvim" },     -- 或 { "code", "-g" } / { "vim", "-p" }
  side_cols = 30,
  show_hidden = false,
  follow_cwd = true,       -- 终端换目录,面板跟着走
  push_cwd = true,         -- 面板换目录,终端跟着 cd
}
```

**目录同步(双向,默认开)**。面板和它所属的终端看的是**同一个目录**:

* **终端 → 面板**(`follow_cwd`):在 shell 里 `cd` 之后,面板会在 ~350ms 内切到
  那个目录,选中项按名字尽量保留。目录来源有两条 —— shell 自己上报的 OSC 7
  (最可靠),以及读 shell **进程的工作目录**(`cmd.exe` 的 `cd` 会调
  `SetCurrentDirectoryW`,读得到)。
* **面板 → 终端**(`push_cwd`):在面板里回车进目录 / 回上级时,VectorTerm 把一行
  `cd` 写进所属 pane 的 shell,和用户自己敲一样。cmd 用 `cd /d "…"`,
  PowerShell 用 `Set-Location -LiteralPath '…'`,其它 shell 用 POSIX 风格的
  `cd "…"`(反斜杠换成 `/`,MSYS / WSL 的 bash 也认)。
* 只在**某一边真的变了**时才同步:你在面板里翻目录时 shell 没动,面板不会被
  拽回去;两个方向都无条件同步会互相打架。
* 三种情况**不写** `cd`:功能关掉了、shell 已经退出、前台正跑着全屏程序
  (vim / less —— 这时 `cd` 会被当成按键喂给它)。

**为什么 PowerShell 需要一点配合**:PowerShell 的 `Set-Location` 只改它自己的
位置,**不动进程的工作目录**,从外面读到的永远是启动时那个目录。所以开着
`follow_cwd` 时,VectorTerm 启动 PowerShell 会追加两个参数:

```text
pwsh -NoLogo -NoExit -File %TEMP%\vectorterm\osc7-prompt.ps1
```

那个脚本把用户原来的 `prompt` 存起来、换成一层包装:每次画提示符前先用
OSC 7 把当前目录报给终端,再调用原来的 prompt —— 所以 oh-my-posh / starship
之类自定义提示符照常工作,只是多了一串不可见的控制序列。脚本每次启动都会重新
生成(内容固定),删掉也没关系。

不想被注入就设 `follow_cwd = false`;如果 `shell_args` 里已经写了 `-Command` /
`-File` 这类**非交互**用法,VectorTerm 也不会硬塞 `-NoExit`(那会改写用户的意图)。
顺带的好处:注入上报的 OSC 7 也让 Lua 的 `pane:get_current_working_dir()`
在 PowerShell 下返回真实目录,而不只是启动目录。

> 面板没打开时不做轮询。首次打开的目录取自当前 pane 的 cwd,一定是当前目录。

> **明明装了 nvim 却提示 not installed**:从资源管理器启动的进程继承的是**启动
> 那一刻**的 `PATH`。装完编辑器不重启资源管理器(或改完环境变量没重开 VectorTerm),
> 子进程就看不到它。除了 `PATH`,探测还会额外查 `%ProgramFiles%\Neovim\bin`、
> `%LOCALAPPDATA%\nvim\bin`、scoop shims、chocolatey、WinGet Links —— 都不中就在
> 配置里写绝对路径:`editor = { "E:/Program Files/Neovim/bin/nvim.exe" }`。

### 15.11 键位方案文件(keys_file)

除了把 `keys` 直接写在配置里,你还可以把键位单独抽成一个 Lua 文件,再用配置
里的 `keys_file`(或复数 `keys_files`)去**引用**它。这份文件通常放在
`%USERPROFILE%\.config\VectorTerm\keys\`(配置管理器新建键位文件的默认位置),
也可以和配置放在一起。

**为什么这么设计**:键位只认"显式写出来的组合键",没写的组合键**自动退回终端
内置默认键位**(分屏、标签、剪贴板、回看等硬快捷键都不在这个表里,但可以被同名
组合键覆盖)。所以无论是外部文件还是配置内联 `keys`,都只是"补丁层",不必抄
全整张键位表。

**两层叠加,后者覆盖前者**:

1. **基础层** —— `keys_file` / `keys_files` 指向的外部 Lua 文件里的 `keys` 表;
2. **覆盖层** —— 配置自己内联的 `keys` 表(优先级最高)。

同一组合键若两层都写了,配置内联的胜出;两层都没写,就走 [§15.6](#156-派发顺序)
的内置默认。

#### 15.11.1 引用写法

```lua
-- 当前配置(例如 ~/.config/VectorTerm/current/mine.lua)
keys_file  = "keys.lua"          -- 单个文件(裸名字,自动补 .lua)
-- keys_files = { "keys.lua", "extra.lua" }   -- 或多个文件,按数组顺序叠加
```

引用字符串按下面的规则解析成真实文件:

| 写法 | 解析结果 |
|---|---|
| `"keys"` / `"keys.lua"`(裸名字) | 先找配置同目录的 `keys.lua`,找不到再到 `%USERPROFILE%\.config\VectorTerm\keys\`、`<exe 目录>\config\`、工作目录 `config\` 里找 |
| `"keymaps/vim.lua"` / `"./x.lua"`(含 `/` `\` `..`) | 相对配置所在目录的路径(统一用正斜杠) |
| `"C:/Users/me/keys/vim.lua"`(绝对路径) | 原样使用 |

裸名字会回退到 `keys\` 目录,是为了让"配置被复制进 `current\`"之后键位引用
依然有效 —— 键位文件不该跟着配置一起挤进 `current\`(那里只允许一份 lua)。

外部文件自身就是一张普通的 `return { ... }` 表,里面只用写 `keys`(其余字段会被
忽略);它的 `keys` 和配置内联 `keys` 走**同一套解析**:字典写法
(`["ctrl+shift+k"] = "reload"`)、wezterm 数组写法、以及指向 Lua 函数,三者皆可。

```lua
-- vim.lua(一份只覆盖几个键的键位方案)
return {
  keys = {
    ["ctrl+shift+k"] = "reload",                       -- 内置动作
    ["ctrl+alt+t"]   = function() VectorTerm.notify("hi") end,  -- 或 Lua 回调
    ["ctrl+shift+y"] = "send:ls\r",                     -- 把文本发进终端
  },
}
```

> 没列在 `keys` 里的组合键(例如分屏 `Ctrl+Shift+Alt+"`、新建标签 `Ctrl+Shift+T`)
> 仍然照常生效 —— 它们只是没被这份方案覆盖而已。

#### 15.11.2 在配置管理器里管键位方案

**左栏下方的「键位文件」栏**专放键位 Lua(默认位于
`%USERPROFILE%\.config\VectorTerm\keys\`,也识别程序目录里 `keys.lua` / `keys1.lua` …),
点击即把它挂到当前配置的 `keys_file` 并打开编辑;顶部工具栏的「键位方案」
下拉是等价的快捷入口。

- 「新建键位文件」按钮按 `keys.lua` → `keys1.lua` → `keys2.lua` 的顺序自动命名
  (弹窗里预填、可改),生成带动作清单注释的模板,并立刻写进当前配置的 `keys_file`。
- 下拉 / 列表只列**键位文件**,不会把普通配置混进来。
- **键位引用写在当前配置里**:所以要先「设为当前配置」;没有当前配置时点键位
  会提示你先设一份,绝不会把 `keys_file` 写到程序默认配置上。
- 「键位映射」面板列出当前键位文件里的键位,支持逐条删除、新增组合键;没列出的
  组合键退回终端内置默认键位。面板顶部会标出**写入目标**是哪个键位文件。
- 除了命名规则,被 `keys_file` 显式引用的文件也一律算键位文件 —— 老版本建的
  `vim.lua` 之类不会被当成普通配置、也不会显示成配置项表单。
- 删除文件会自动**解除引用**:删掉被引用的键位文件会同时清掉 `keys_file`,
  不会留下指向空文件的悬空路径。

> 配置管理器里的「当前配置」就是终端启动时加载的那一份
> (`%USERPROFILE%\.config\VectorTerm\current\` 里唯一的 lua)—— 详见
> [§2.6](#26-配置目录结构current-与-keys)。

---

## 16 VectorTerm 运行时 API

在 `keys` 的 Lua 函数里可以访问全局表 `VectorTerm`(旧名 `wintty` 仍作为别名保留)。**所有动作都先入队**,
等本次 Lua 调用返回后由主线程统一执行,避免重入。

| 方法 | 作用 |
|---|---|
| `VectorTerm.notify(msg)` | 改窗口标题作为提示 |
| `VectorTerm.send(text)` | 向 shell 发送文本(不自动加回车) |
| `VectorTerm.paste()` | 粘贴剪贴板 |
| `VectorTerm.reload()` | 重载配置 |
| `VectorTerm.quit()` | 退出程序 |
| `VectorTerm.cursor_color("#rrggbb")` | 改光标颜色(立即生效,不改配置文件) |
| `VectorTerm.trail_dwell(ms)` | 尾迹静止阈值 |
| `VectorTerm.trail_threshold(x [, y])` | 每轴格数阈值,`y` 省略时等于 `x` |
| `VectorTerm.trail_decay(fast [, slow])` | 衰减时间(秒),`slow` 省略时等于 `fast` |
| `VectorTerm.trail_max(ms)` | 单段尾迹动画硬上限,`0` = 不限制 |
| `VectorTerm.trail_color("#rrggbb" 或 nil)` | 尾迹颜色,`nil` = 跟随光标 |

```lua
keys = {
  ["ctrl+alt+d"] = function()
    VectorTerm.trail_decay(0.15, 0.8)     -- 尾巴更长
    VectorTerm.trail_color("#7aa2f7")
    VectorTerm.notify("demo mode")
  end,
  ["f9"] = function()
    VectorTerm.send("git status\r")        -- \r 才会执行
  end,
}
```

这些运行时改动只作用于当前进程,不写回配置文件;下一次热重载会按配置重新初始化。

---

## 17 wezterm 兼容层

目标:让 `VectorTerm.exe --config .wezterm.lua` 直接吃 wezterm 的原配置。

### 17.1 已实现的 API

| API | 行为 |
|---|---|
| `require 'wezterm'` | 返回同一个 `wezterm` 表 |
| `wezterm.config_builder()` | 返回空表(解析层按缺省值补齐) |
| `wezterm.font(spec)` | 返回 `{ family = spec }` |
| `wezterm.font_with_fallback({ ... })` | 原样返回数组,正好对应 `font = { 主字体, 回退… }` |
| `wezterm.action.*` | 常量与带参调用两种写法都支持 |
| `wezterm.nerdfonts.*` | 按名字查本机 Nerd Font 字形表,返回真实字符;未收录的返回 `""` |
| `wezterm.on(event, cb)` | 登记并按事件派发(见 8.7 / 8.8: `update-status` / `update-right-status` / `format-tab-title`) |
| `wezterm.strftime(fmt)` | 转给 Lua 的 `os.date` |
| `wezterm.format({...})` | 翻译成真正的 SGR 序列,颜色 / 粗斜体 / 下划线都会画出来 |
| `wezterm.battery_info()` | 读系统电池,返回 `{ state_of_charge, state }` 的表(台式机返回空表) |
| `wezterm.home_dir` / `.target_triple` / `.version` | 环境信息 |

### 17.2 直接生效的 wezterm 字段

| wezterm 字段 | 对应 |
|---|---|
| `default_prog` | `shell` + `shell_args`(`shell` 显式写了就忽略) |
| `font` / `font_size` / `line_height` / `cell_width` | 同名字段 |
| `window_padding` | `padding`(只取 `top`) |
| `initial_cols` / `initial_rows` | 同名 |
| `colors` 里的 `background` / `foreground` / `cursor_fg` / `cursor_bg` | 同名 |
| `colors.ansi` + `colors.brights` | 拼成 16 色表 |
| `colors.selection_bg` | `selection.background`(`opacity` 强制 1.0) |
| `colors.tab_bar` | `colors.tab_bar`(同名同结构) |
| `enable_tab_bar` / `tab_bar_at_bottom` / `use_fancy_tab_bar` | 标签栏开关 |
| `hide_tab_bar_if_only_one_tab` / `tab_max_width` | 标签栏行为 |
| `show_tab_index_in_tab_bar` / `show_new_tab_button_in_tab_bar` | 标签栏元素 |
| `set_environment_variables` | `env` |
| `window_decorations` + `integrated_title_*` 四件套 | 窗口装饰全套 |
| `use_resize_increments` | 同名 |
| `window_background_*` 一族 | 背景全套(`color` / `image` / `image_hsb` / `opacity` / `gradient`) |
| `win32_system_backdrop` | 同名(只设 DWM 属性,见 13.5) |
| `keys`(数组格式 + `wezterm.action.*`) | `keys` |
| `exit_behavior` | 同名(`"Close"` / `"Hold"`) |

VectorTerm 自己扩展的项(wezterm 没有同名):`window_rounded_corners`、
`live_resize_mode`、`resize_debounce_ms`、`tab_bar_edge_padding`、`tab_bar_top_padding`、
`tab_bar.show_close_button`、`images.*`、`cursor.*`、`scroll.*`、`render`、`gpu_power`、
`session_log`、`quit_confirmation`、`window_background_blur`。

### 17.3 明确不生效的 wezterm 配置

`window_frame`、`enable_scroll_bar`、`default_cursor_style` / `cursor_blink_rate`
(光标形状与闪烁跟随应用)、`animation_fps`(动画固定 120fps 上限)、
`update-left-status` / `window-config-reloaded` 等其余 `wezterm.on` 事件、
domain / 多路复用(SSH/mux)相关的配置。

> 分屏与 pane 焦点切换已支持(内置快捷键,见 [§15.7 分屏](#157-分屏pane));
> pane 尺寸调整已支持,但组合是内置硬编码(见 [§15.8.7](#1587-拖动分割栏与快捷键调整窗格大小)):不能在 `keys` 表里把 `AdjustPaneSize` 绑到别的键。

这些键不会报错,只是被忽略 —— 拿一份 wezterm 配置直接喂进来通常是能启动的。

---

## 18 热重载对照表

保存文件后约 0.4 秒内重新加载。三类生效方式:

### 18.1 即时生效(不用重启、不用新开标签)

| 分组 | 配置项 |
|---|---|
| 颜色 | `colors.*`(含 `ansi` / `brights` / `indexed` / `selection_bg` / `tab_bar`) |
| 选区 | `selection.*` |
| 尾迹 | `cursor.*` |
| 滚动 | `scroll` / `scroll.*`、`scrollback_lines`、`scrollbar.*`、`search.*`、`selection.edge_scroll` |
| 标签栏 | `tab_bar.*`、所有顶层别名 |
| 状态栏 | `status_bar.*`(开关 / 字号倍数 / 高度 / 留白 / 片段间距 / 空格宽度 / 固定文字;改完下一次状态栏刷新即可见,见 [§8.7](#87-状态栏左边一条右边一条)) |
| 分屏 | `split_divider.*`(粗细 / 间距 / 留白 / 单色 / 渐变 / 拖动与快捷键步长,下一帧生效,见 [§15.8](#158-分割线外观))、`pane_focus.*`(见 [§15.9](#159-分屏焦点)) |
| 文件浏览器 | `files_gui.*`(打开方式 / 编辑器 / 侧边栏宽度 / 隐藏项 / 目录同步开关;`follow_cwd` 的 PowerShell 注入要重开窗口才生效,其余下次打开面板生效) |
| 缩放 | `live_resize_mode`、`resize_debounce_ms`、`use_resize_increments` |
| 键位 | `keys`、`VectorTerm.*` 脚本 |
| 主题 | `theme`(换主题或改覆盖项,保存**当前配置**就重新合并;主题里若带启动项仍要重启,见 [§2.5](#25-内置主题)) |
| 行为 | `exit_behavior`、`quit_confirmation` |
| 图片 | `images.enable` |

### 18.2 新开标签才生效

`shell`、`shell_args`、`default_prog`、`env`、`set_environment_variables`、`cwd`、
`session_log` —— 已存在的会话不会被替换(否则正在跑的命令会被杀掉)。

### 18.3 必须重启

| 分组 | 配置项 | 原因 |
|---|---|---|
| 字体 | `font.*`、`font_fallback`、`font_size`、`line_height`、`cell_width` | 渲染器在启动时建字形图集 |
| 内边距 | `padding`、`window_padding` | 画布布局在启动时算好 |
| 窗口 | `window_decorations`、`window_titlebar`、`window_rounded_corners`、`integrated_title_*` | 窗口创建时定 |
| 几何 | `initial_cols`、`initial_rows`、`window_width`、`window_height` | 只影响建窗口那一次 |
| 渲染 | `render`、`gpu_power` | 后端 / 显卡在启动时选定 |
| 背景 | `window_background_*`、`win32_system_backdrop` | 图片与透明度在启动时应用 |
| 图片 | `images.max_memory_mb` | 缓存容量在启动时建 |
| 会话保持 | `session_persistence` | 启动时决定 shell 由本地 PTY 还是守护进程托管(见 [§4.8](#48-session_persistence会话保持wezterm-式多路复用)) |

其中 `render` 与 `gpu_power` 在热重载时会明确写一句
`[VectorTerm] render 从 cpu 改为 gpu,重启后生效`,不会让你对着没变化的窗口猜;
其余几项没有这条提示,改完记得手动重启。

---

## 19 全量配置模板

下面这份是**所有可配置项 + 官方默认值**的完整清单,整段复制就能跑。
不需要的整行删掉或注释掉即可。

```lua
return {
  -- 内置主题:随包分发的 config/deepseek.lua(DeepSeek 深色配色 + 关掉滚动条)。
  -- 写了它,下面所有键都变成「盖在主题之上」的覆盖项;不写就是纯内置默认值。
  -- theme = "deepseek",                   -- 详见 §2.5

  -- ===================== 会话与启动 =====================
  shell      = "pwsh.exe",                 -- 命令名(走 PATH)或绝对路径
  shell_args = { "-NoLogo" },              -- 显式写 shell 时默认参数不继承,要自己给
  -- default_prog = { "pwsh.exe", "-NoLogo" },  -- wezterm 写法,shell 显式写了就忽略它
  -- env = { FOO = "bar", GIT_PAGER = "cat" },
  -- set_environment_variables = { DSH_TUI_IMAGE_PROTOCOL = "kitty" },
  -- cwd = "C:/Users/you",
  -- session_log = "C:/Users/you/vectorterm-session.log",
  exit_behavior     = "Close",             -- "Close" | "Hold"
  quit_confirmation = true,                -- 有程序在跑时,关窗 / 关标签 / 关窗格先问一句
  -- 这张名单里的进程不算"有程序在跑"(wezterm 同名选项 + 两个 ConPTY 宿主)
  skip_close_confirmation_for_processes_named = {
    "bash", "sh", "zsh", "fish", "tmux", "nu",
    "cmd.exe", "pwsh.exe", "powershell.exe", "conhost.exe", "openconsole.exe",
  },
  session_persistence = false,             -- true = 交给 --mux-server 托管(重启生效,见 §4.8)

  -- ===================== 字体 =====================
  font = {
    family      = "JetBrains Mono",                  -- 内嵌;也可写家族名/字体文件路径
    size        = 18,                               -- 逻辑 px @96dpi,夹到 6–96
    line_height = nil,                              -- 不写 = 字体自身(夹到 1.0–1.6em)
    cell_width  = 1.0,                              -- 夹到 0.5–3.0
    fallback    = { "Nerd Font Symbols", "Microsoft YaHei" }, -- 文本回退链
    -- bold = "JetBrains Mono Bold",
    -- italic = "JetBrains Mono Italic",
    -- bold_italic = "JetBrains Mono Bold Italic",
    nerd_font = "auto",                             -- "auto" | 家族名 | false
    dirs      = {},                                 -- 额外字体目录
  },
  -- 顶层别名(注意优先级):
  -- font_fallback = { "Microsoft YaHei" },  -- 仅当 font.fallback 没写时生效
  -- font_size     = 16,                     -- 仅当 font.size/font_size 没写时生效
  -- line_height   = 0.95,                   -- 总是覆盖 font.line_height
  -- cell_width    = 1.0,                    -- 总是覆盖 font.cell_width

  -- ===================== 颜色 =====================
  colors = {
    foreground = "#cccccc",
    background = "#0c0c0c",
    cursor_fg  = "#0c0c0c",
    cursor_bg  = "#ffffff",
    -- ansi = { 8 个基础色 },
    -- brights = { 8 个亮色 },
    -- indexed = { [196] = "#ff0000" },
    -- selection_bg = "#1E2A4A",             -- 不透明选区(覆盖 selection.background)
    tab_bar = {
      background        = "#0c0c0c",
      active_tab        = { bg_color = "#2f2f2f", fg_color = "#ffffff" },
      inactive_tab      = { bg_color = "#161616", fg_color = "#a0a0a0" },
      inactive_tab_hover= { bg_color = "#262626", fg_color = "#e6e6e6" },
      new_tab           = { bg_color = "#0c0c0c", fg_color = "#a0a0a0" },
      new_tab_hover     = { bg_color = "#2f2f2f", fg_color = "#ffffff" },
      inactive_tab_edge = "#2a2a2a",
    },
  },

  -- ===================== 窗口与外观 =====================
  padding = 8.0,                            -- 内容区四周留白(逻辑像素)
  -- window_padding = { top = 8 },          -- wezterm 写法(只取 top),padding 优先
  -- window_width = 1300,                   -- 启动窗口像素宽(优先于 initial_cols)
  -- window_height = 734,                   -- 启动窗口像素高
  -- initial_cols = 120,                    -- 或按单元格数(改字号会连带改窗口大小)
  -- initial_rows = 32,
  window_decorations     = "TITLE|RESIZE",  -- TITLE | RESIZE | INTEGRATED_BUTTONS | NONE
  -- window_titlebar     = true,            -- 简写:只改"有无系统标题栏"
  window_rounded_corners = true,            -- 无边框时向 DWM 要 Win11 圆角
  integrated_title_buttons          = { "Hide", "Maximize", "Close" },
  integrated_title_button_alignment = "Right",    -- "Right" | "Left"
  integrated_title_button_color     = "Auto",     -- "Auto" 或颜色
  integrated_title_button_style     = "Windows",  -- "Windows" | "Gnome"

  -- ===================== 标签栏 =====================
  status_update_interval = 1000,           -- 状态栏回调派发间隔(ms),夹到 50–60000
  tab_bar = {
    enable              = true,
    at_bottom           = false,
    fancy               = true,
    hide_if_one         = false,
    max_width           = 16,               -- 单元格,夹到 3–128
    show_index          = true,
    show_new_tab_button = true,
    show_close_button   = true,
    -- corner_radius = 8,                   -- 像素,0 = 直角,不写按高度自动
    -- edge_padding  = 10,                  -- 逻辑像素,0 = 贴边
    -- top_padding   = 10,                  -- 逻辑像素,0 = 贴边
  },

  -- ===================== 状态栏 =====================
  -- 右侧状态栏的外观(内容仍由 update-right-status 脚本决定),详见 §8.7。
  -- 不写这一节也完全能用:内容由 update-right-status 脚本决定。
  status_bar = {
    enable       = true,                   -- false = 右侧状态栏整段不画
    keep_script  = true,                   -- 保留脚本 set_right_status 的文字
    font_scale   = 1.0,                    -- 状态栏字号 = 终端字号 × 它(夹到 0.5–2.0)
    -- height    = 34,                     -- 整条栏最小高度(物理像素),不写 = 跟随字号
    padding      = 8,                      -- 左右留白(物理像素)
    item_spacing = 0,                      -- 相邻两段文字(每次换色的那一段)之间的额外间距
    space_width  = 1.0,                    -- 一个空格占几个字宽(1.0 = 和 wezterm 一致;调小收紧切片)
    text         = "",                     -- 额外拼在前面的一段固定文字(不写 Lua)
  },

  -- ===================== 分屏分割线 =====================
  -- 不写这一节 = 自动间距 + 3px 细线。详见 §15.8  split_divider = {
    enabled     = true,
    -- thickness   = 2,                -- 不写 = 自动 3px 细线;0 = 隐藏;单位 px
    -- gap         = "Auto",          -- 缝宽;不写 = 线宽 + 2px(文字到线 1px)
    -- text_gap    = 6,               -- 文字到线边缘的距离,设置后覆盖 gap
    inset_start = 0,                  -- 竖线离上边 / 横线离左边
    inset_end   = 0,                  -- 竖线离下边 / 横线离右边
    -- inset     = 40,               -- 两端相同的简写
    color       = "#323236",          -- 单色
    -- gradient = {                  -- 配了渐变就覆盖 color
    --   colors = { "#ff5f6d", "#ffc371", "#45b7d1" },
    --   orientation = "Auto",       -- Auto | Vertical | Horizontal
    -- },
    opacity     = 1.0,
    -- 分割栏拖动 / 快捷键调整窗格大小(详见 §15.8.7)
    draggable   = true,
    hit_width   = 8,
    min_ratio   = 0.08,
    step        = 0.05,
  },

  -- ===================== 分屏焦点 =====================
  -- 详见 §15.9:只有聚焦 pane 显示输入光标;鼠标点击 / Ctrl+Shift+方向键 /
  -- Alt+数字 切焦点时,这个输入光标从旧 pane 飞到新 pane 的光标处。
  pane_focus = {
    click_to_focus = true,
    hide_inactive_caret = true,
    number_shortcuts = true,
    trail = true,
    sync_trail = true,                -- 速度 / 颜色 / 透明度跟随 cursor.trail_*
    trail_ms = 320,
    -- trail_color = "#ffcc00",      -- 仅 sync_trail = false 时生效
    -- landing = "caret",           -- 已废弃:解析但不生效,仅为旧配置兼容
  },

  -- ===================== 文件浏览器 =====================
  -- 详见 §15.10:命令面板 /files 铺满当前 pane,Ctrl+Shift+E 是侧边栏
  files_gui = {
    open_mode   = "window",               -- "window" 新窗口(默认) / "split" 分屏 / "current" 当前 shell
    -- editor = { "nvim" },              -- 不写 = 自动探测 nvim → vim → 记事本
    side_cols   = 30,                     -- 侧边栏宽度(列),窄于 24 不显示尺寸
    show_hidden = false,
    follow_cwd  = true,                   -- 终端 cd → 面板跟着走
    push_cwd    = true,                   -- 面板进目录 → 给终端写 cd
  },

  -- ===================== 鼠标选区 =====================
  selection = {
    copy_on_select = true,
    edge_scroll    = true,                -- 拖到上/下边缘自动翻回看历史(详见 §11.4)
    background     = "#3a6ea5",
    opacity        = 0.45,                  -- 夹到 0.05–1.0
    ctrl_shift_v   = "paste",               -- "paste" | "copy"
  },

  -- ===================== 光标尾迹 =====================
  cursor = {
    trail                 = 20,             -- 静止毫秒后开始跟随;0 = 关闭
    trail_start_threshold = 0,              -- 每轴最小位移(格);2 = kitty 默认
    trail_decay           = "0.1 0.4",      -- (最快, 最慢)秒
    trail_max             = 220,            -- 单段硬上限(ms);0 = 不限制
    trail_color           = "none",         -- "none" = 跟随光标
    -- 光标本体(§10.5):下面全是可选,不写 = 整格方块 / 不闪 / 不动
    -- shape             = "auto",
    -- width             = 1.0,
    -- height            = 1.0,
    -- align_x           = "left",
    -- align_y           = "top",
    -- thickness         = 0,
    -- alpha             = 1.0,
    -- blink             = "auto",
    -- blink_rate        = 1060,
    -- blink_duty        = 0.5,
    -- blink_style       = "hidden",
    -- anim              = "none",
    -- anim_ms           = 1200,
    -- anim_amount       = 0.25,
  },

  -- ===================== 回看滚动 =====================
  scrollback_lines = 3500,
  scroll = {
    anim       = 90,                        -- 一个滚轮档位(3 行)的时长;0 = 瞬移
    max        = 200,                       -- 单段硬上限,最小 1
    back       = 90,                        -- 按键/粘贴后回底的时长
    back_burst = 40,                        -- 连发输入时回底的时长
  },
  -- 右侧可拖动滚动条(详见 §11.4);scrollbar = false 整体关掉
  scrollbar = {
    enabled     = true,
    show        = "auto",        -- "always" 一直画 | "auto" 贴底隐藏 | "never" 不画
    side        = "right",       -- "right" | "left"(分屏左 pane 可改 left 避开分割线)
    show_track  = true,          -- false = 只画滑块
    width       = 9,             -- 轨道宽度(px)
    min_thumb   = 28,            -- 滑块最小高度(px)
    auto_hide   = true,          -- show 的兼容别名;写了 show 时以 show 为准
    opacity     = 0.85,
    -- track_color = "#333840",   -- 不写 = 由前景/背景色自动混合
    -- thumb_color = "#7aa2f7",   -- 不写 = 自动配色
    -- hover_color = "#ffffff",   -- 悬停 / 拖动时的颜色
  },
  -- Ctrl+Shift+F 在全部历史里搜索(详见 §11.5)
  search = {
    enabled         = true,
    case_sensitive  = false,
    highlight_color = "#f0c674",
    current_color   = "#ff8f3f",
    max_matches     = 5000,
  },

  -- ===================== 渲染与缩放 =====================
  render                = "cpu",            -- "cpu" | "gpu" | "auto"(重启生效)
  gpu_power             = "low",            -- "low" | "high"(重启生效)
  live_resize_mode      = "realtime",       -- "realtime" | "debounce" | "freeze"
  resize_debounce_ms    = 150,              -- 仅非拖动,夹到 ≤ 1000
  use_resize_increments = true,             -- 按整格步进缩放

  -- ===================== 自定义背景 =====================
  -- window_background_color = "#101418",
  -- window_background_image = "bg/wallpaper.jpg",     -- 相对配置文件目录
  -- window_background_image_hsb = {
  --   brightness = 0.45,         -- 压暗壁纸,文字更清楚
  --   hue        = 1.0,
  --   saturation = 0.9,
  -- },
  -- window_background_blur = 12,                       -- 0–200
  -- window_background_fill = "cover",                  -- cover|contain|stretch|tile|center
  -- window_background_opacity = 0.92,                  -- 整窗(含文字)统一 alpha
  -- win32_system_backdrop = "none",                    -- none|mica|acrylic|tabbed
  -- window_background_gradient = {
  --   colors = { "#0b1020", "#1b2a4a" },
  --   orientation = "Vertical",                         -- 或 "Horizontal"
  -- },

  -- ===================== 图片协议 =====================
  images = {
    enable        = true,                   -- false = 不认 iTerm2 / Kitty 图形报文
    max_memory_mb = 256,                    -- 夹到 8–4096(重启生效)
  },

  -- ===================== 键位 =====================
  keys = {
    ["ctrl+shift+r"] = "reload",
    ["ctrl+shift+q"] = "quit",
    ["f12"]          = "send:dir\r",
    ["ctrl+alt+t"]   = function()
      VectorTerm.notify("hi")
    end,
  },
}
```

---

## 20 常见配方

### 20.1 像 wezterm 那样的无边框窗口

```lua
window_decorations     = "INTEGRATED_BUTTONS|RESIZE",
window_rounded_corners = true,
integrated_title_buttons = { "Close", "Maximize", "Hide" },   -- 顺序自定
integrated_title_button_style = "Windows",
tab_bar = { enable = true, max_width = 22 },
```

标签栏兼任标题栏:拖空白处移动窗口,双击最大化;三枚按钮画在标签栏里。

### 20.2 只想去掉标题栏,不要自绘按钮

```lua
window_decorations = "RESIZE",   -- 顶部那条标签栏仍在,只是不画按钮
```

### 20.3 最省心的 Nerd Font + 中英混排

```lua
font = {
  family      = "MesloLGL Nerd Font Mono",
  size        = 15,
  line_height = 0.95,
  fallback    = { "Microsoft YaHei UI" },
  nerd_font   = "auto",
}
```

### 20.4 半透明 + 压暗壁纸

```lua
window_background_image = "bg/wallpaper.jpg",
window_background_image_hsb = { brightness = 0.45, saturation = 0.9 },
window_background_blur = 10,
window_background_opacity = 0.92,     -- 整窗含文字一起变淡,别低于 0.85
colors = { background = "#101418" },
```

### 20.5 大回看 + 关动画(性能优先)

```lua
scrollback_lines = 20000,
scroll  = false,          -- 滚轮不再有平滑动画
cursor  = { trail = 0 },  -- 关掉尾迹
render  = "gpu",          -- 4K / 高刷屏
```

### 20.6 V 键当复制的操作习惯

```lua
selection = { ctrl_shift_v = "copy" },
```

`Ctrl+Shift+V` 变成复制、`Ctrl+Shift+C` 变成粘贴。

### 20.7 让 TUI 应用走 Kitty 图形协议

```lua
images = { enable = true, max_memory_mb = 512 },
set_environment_variables = { DSH_TUI_IMAGE_PROTOCOL = "kitty" },
```

### 20.8 直接复用 wezterm 配置

```bat
VectorTerm.exe --config C:\Users\you\.wezterm.lua
```

不生效的字段见 17.3,不会报错。

---

## 21 排错

### 21.1 配置错了会怎样

和 wezterm 一样,**软件自身的错误不进当前终端**:

1. 主窗口照常打开,用默认配置启动 shell,**当前终端画面里没有任何注入的错误文字**;
2. 自动弹出独立的控制台窗口 `VectorTerm Debug Console - N 个错误`,按时间顺序列出全部
   软件日志,ERROR 行标红,带文件、行列号与原始报错;
3. 日志同时写 `%USERPROFILE%\.config\VectorTerm\VectorTerm.log`,配置错误另存
   `config-error.log`(超过 64KB 自动重来);
4. 改好保存后自动热重载,旧会话一直可用。

调试终端**只在出错时才弹**。想平时也盯着日志设 `VECTORTERM_DEBUG_CONSOLE=1`;
想完全不弹(后台 / 自动化)设 `VECTORTERM_DEBUG_CONSOLE=0`,日志仍写文件。
它的关闭按钮被禁用、Ctrl 事件被忽略 —— Windows 控制台的关闭事件默认会终止宿主进程,
不能让随手一个 X 把终端连带 shell 一起关掉。

### 21.2 查配置问题的正确姿势

```bat
:: 1. 看最近一次配置错误
type "%USERPROFILE%\.config\VectorTerm\config-error.log"

:: 2. 看完整日志(含降级、后端选择、装饰、缩放策略)
type "%USERPROFILE%\.config\VectorTerm\VectorTerm.log"

:: 3. 强制开着调试终端启动
set VECTORTERM_DEBUG_CONSOLE=1
VectorTerm.exe --config "%USERPROFILE%\.config\VectorTerm\current\deepseek.lua"
```

启动日志会把关键决策直接打出来,例如:`找不到 pwsh.exe,已自动改用 powershell.exe`、
`窗口装饰:… / 自绘缩放边框=… / 标签栏按钮 N 枚`、`拖动时行列数=… / 非拖动稳定窗口=…`、
`渲染后端:CPU 软渲染`;热重载时另有 `已重新加载配置:<路径>` 与
`render 从 cpu 改为 gpu,重启后生效`。配没配对一眼就能看出来。

### 21.3 常见坑

| 现象 | 原因 | 处理 |
|---|---|---|
| 整个配置没生效 | 文件没 `return` 一张表;或改的不是"当前配置"(终端只加载 `current/` 里那一份) | 第一行写 `return {`;在配置管理器里点「设为当前配置」,确认左栏「当前配置」是你改的那份 |
| 启动日志报「当前配置目录下有 N 个 .lua」 | `~/.config/VectorTerm/current/` 里被放了多份 lua,终端无法判断用哪份,已退回程序默认配置 | 删掉多余的,或在配置管理器里重新「设为当前配置」(会自动清理) |
| 窗口尺寸改了没变化 | 改的是"主题底表"而不是当前配置(主题里写的尺寸会被配置覆盖);或不是写在 `window_width` / `window_height` 里 | 改当前配置里的这两项;它们是**物理像素**,125% 缩放下填 1300 就是 1300 像素,改完重启终端 |
| 换了 `shell` 后一启动就退出 | 显式写 `shell` 会清空默认参数,`cmd.exe` 拿到 `-NoLogo` 会报"无效开关" | 清掉 `shell_args`,或改成目标 shell 认识的参数 |
| 字体没变化 | 字体与内边距要重启 | 重启程序 |
| 窗口装饰没变化 | 同上,窗口创建时定 | 重启程序 |
| `window_background_image` 找不到图 | 相对路径是相对**配置文件**目录,不是 exe 目录 | 用绝对路径,或把图放到配置文件旁边 |
| 只配了 `opacity` 但看不出透明 | 透明度作用于整窗,壁纸区域可能本来就和背后窗口同色 | 配合 `window_background_image` 或降低 `colors.background` 亮度 |
| 配了 `win32_system_backdrop` 没效果 | 交换链给不出逐像素透明,材质透不出来(已知限制) | 改用 `window_background_opacity` |
| Nerd Font 图标显示成方块 | 私用区字形被 CJK 字体的占位字形抢了;或 `nerd_font = false` | 设 `nerd_font = "auto"` 或直接指定 Nerd Font 家族 |
| 汉字之间有缝隙 / 汉字偏小 | 字体 asc-desc 远大于 1em,且没配回退 | 配 `fallback`,或换一个 em 接近 1.0 的中文字体 |
| 标签栏第一个标签被圆角裁掉 | 无边框 + 圆角时留白被改成了 0 | 别把 `edge_padding` / `top_padding` 设 0,或删掉让它自动 |
| `Ctrl+Shift+T` 不能新建标签 | `keys` 里绑了同一个组合 | 删掉或换键位;`keys` 优先于内置快捷键 |
| 键位写了没反应 | 动作名拼错(会打印"未知的键位动作"),或组合里用了不支持的键名 | 对照 15.3 / 15.5 |
| 配置里写 `while true do end` 卡住启动 | Lua 堆有 64MB 上限,但不拦死循环 | 关掉进程,改配置 |
| 用户配置里的中文路径乱码 | 颜色值只接受 ASCII;文件路径请用 UTF-8 保存的 lua | 颜色写 `#RRGGBB`,路径用正斜杠 `/` |

---

## 22 默认值总表

不给任何配置时的完整默认值。查"我该写什么"看这里,查"某项怎么用"看对应章节。

### 22.1 会话

| 键 | 默认值 |
|---|---|
| `shell` | `"pwsh.exe"` |
| `theme` | 无(不加载内置主题,各分组都取本表默认值;见 [§2.5](#25-内置主题)) |
| `shell_args` | `{ "-NoLogo" }` |
| `default_prog` | 无 |
| `env` / `set_environment_variables` | 无 |
| `cwd` | 无(继承进程当前目录) |
| `session_log` | 无(不记录) |
| `exit_behavior` | `"Close"` |
| `quit_confirmation` | `true` |
| `skip_close_confirmation_for_processes_named` | `bash` / `sh` / `zsh` / `fish` / `tmux` / `nu` / `cmd.exe` / `pwsh.exe` / `powershell.exe` / `conhost.exe` / `openconsole.exe` |
| `session_persistence` | `false`(本地直连,不开守护进程) |

### 22.2 字体

| 键 | 默认值 |
|---|---|
| `font.family` | `"JetBrains Mono"`(内嵌) |
| `font.size` | `18` |
| `font.line_height` | 自动(夹到 1.0–1.6em) |
| `font.cell_width` | `1.0` |
| `font.fallback` | `{ "Nerd Font Symbols", "Microsoft YaHei" }` |
| `font.bold` / `italic` / `bold_italic` | 同家族对应 face |
| `font.nerd_font` | `"auto"` |
| `font.dirs` | 空 |

### 22.3 颜色

| 键 | 默认值 |
|---|---|
| `colors.foreground` | `"#cccccc"` |
| `colors.background` | `"#0c0c0c"` |
| `colors.cursor_fg` | `"#0c0c0c"` |
| `colors.cursor_bg` | `"#ffffff"` |
| `colors.ansi` / `brights` / `indexed` | 空(用内置 256 色表) |
| `selection.background` | `"#3a6ea5"` |
| `selection.opacity` | `0.45` |

### 22.4 窗口与标签栏

| 键 | 默认值 |
|---|---|
| `padding` | `8.0` |
| `window_width` / `window_height` | 无(物理像素,写了就按像素开窗,不随 DPI 放大) |
| `initial_cols` / `initial_rows` | 无(内置 960×640 起点) |
| `window_decorations` | `"TITLE\|RESIZE"` |
| `window_titlebar` | 无(简写,只覆盖“有无系统标题栏”这一维) |
| `window_rounded_corners` | `true` |
| `integrated_title_buttons` | `{ Hide, Maximize, Close }` |
| `integrated_title_button_alignment` | `"Right"` |
| `integrated_title_button_color` | `"Auto"` |
| `integrated_title_button_style` | `"Windows"` |
| `tab_bar.enable` | `true` |
| `tab_bar.at_bottom` | `false` |
| `tab_bar.fancy` | `true` |
| `tab_bar.hide_if_one` | `false` |
| `tab_bar.max_width` | `16` |
| `tab_bar.show_index` | `true` |
| `tab_bar.show_new_tab_button` | `true` |
| `tab_bar.show_close_button` | `true` |
| `tab_bar.corner_radius` | 自动(约 8px) |
| `tab_bar.edge_padding` / `top_padding` | 自动(有标题栏 4 / 无边框+圆角 10,按 DPI 放大) |
| `status_update_interval` | `1000`(毫秒;夹到 50–60000) |
| `status_bar.enable` | `true` |
| `status_bar.keep_script` | `true` |
| `status_bar.font_scale` | `1.0`(夹到 0.5–2.0) |
| `status_bar.height` | 无(跟随字号) |
| `status_bar.padding` | `8`(物理像素) |
| `status_bar.item_spacing` | `0`(物理像素) |
| `status_bar.space_width` | `1.0`(一个空格一个字宽) |
| `status_bar.text` | 空 |
| `tab_bar.colors.background` | `"#0c0c0c"` |
| `tab_bar.colors.active_tab` | `#2f2f2f` / `#ffffff` |
| `tab_bar.colors.inactive_tab` | `#161616` / `#a0a0a0` |
| `tab_bar.colors.inactive_tab_hover` | `#262626` / `#e6e6e6` |
| `tab_bar.colors.new_tab` | `#0c0c0c` / `#a0a0a0` |
| `tab_bar.colors.new_tab_hover` | `#2f2f2f` / `#ffffff` |
| `tab_bar.colors.inactive_tab_edge` | `"#2a2a2a"` |

### 22.5 动画与渲染

| 键 | 默认值 |
|---|---|
| `selection.copy_on_select` | `true` |
| `selection.ctrl_shift_v` | `"paste"` |
| `cursor.trail` | `20` |
| `cursor.trail_start_threshold` | `0` |
| `cursor.trail_decay` | `0.1` / `0.4` |
| `cursor.trail_max` | `220` |
| `cursor.trail_color` | `none`(跟随光标) |
| `cursor.shape` | `"auto"` |
| `cursor.width` | `1.0` |
| `cursor.height` | `1.0` |
| `cursor.align_x` | 跟随形状(贴左) |
| `cursor.align_y` | 跟随形状(方块贴顶 / 下划线贴底) |
| `cursor.thickness` | `0`(自动) |
| `cursor.alpha` | `1.0` |
| `cursor.blink` | `"auto"` |
| `cursor.blink_rate` | `1060` |
| `cursor.blink_duty` | `0.5` |
| `cursor.blink_style` | `"hidden"` |
| `cursor.anim` | `"none"` |
| `cursor.anim_ms` | `1200` |
| `cursor.anim_amount` | `0.25` |
| `scrollback_lines` | `3500` |
| `scroll.anim` | `90` |
| `scroll.max` | `200` |
| `scroll.back` | `90` |
| `scroll.back_burst` | `40` |
| `scrollbar.enabled` | `true` |
| `scrollbar.width` | `9` |
| `scrollbar.min_thumb` | `28` |
| `scrollbar.show` | `"auto"` |
| `scrollbar.side` | `"right"` |
| `scrollbar.show_track` | `true` |
| `scrollbar.auto_hide` | `true` |
| `scrollbar.opacity` | `0.85` |
| `scrollbar.track_color` / `thumb_color` / `hover_color` | 无(自动配色) |
| `split_divider.draggable` | `true` |
| `split_divider.hit_width` | `8` |
| `split_divider.min_ratio` | `0.08` |
| `split_divider.step` | `0.05` |
| `search.enabled` | `true` |
| `search.case_sensitive` | `false` |
| `search.highlight_color` | `"#f0c674"` |
| `search.current_color` | `"#ff8f3f"` |
| `search.max_matches` | `5000` |
| `selection.edge_scroll` | `true` |
| `render` | `"cpu"` |
| `gpu_power` | `"low"` |
| `live_resize_mode` | `"realtime"` |
| `resize_debounce_ms` | `150` |
| `use_resize_increments` | `true` |

### 22.6 背景与图片

| 键 | 默认值 |
|---|---|
| `window_background_color` | 无(用 `colors.background`) |
| `window_background_image` | 无 |
| `window_background_image_hsb` | 无(不调整) |
| `window_background_image_hsb.brightness` | `1.0`(只影响图片,0.4~0.5 可压暗) |
| `window_background_image_hsb.hue` | `1.0` |
| `window_background_image_hsb.saturation` | `1.0` |
| `window_background_blur` | `0` |
| `window_background_fill` | `"cover"` |
| `window_background_opacity` | 无(不透明) |
| `win32_system_backdrop` | `"none"` |
| `window_background_gradient` | 无 |
| `images.enable` | `true` |
| `images.max_memory_mb` | `256` |
| `keys` | 空(只用内置快捷键) |

### 22.7 分屏分割线

| 键 | 默认值 |
|---|---|
| `split_divider.enabled` | `true` |
| `split_divider.thickness` | 无(自动 3px 细线) |
| `split_divider.gap` | 无(`Auto` = 线宽 + 2px) |
| `split_divider.text_gap` | 无(自动 = 1px 空气) |
| `split_divider.inset_start` / `inset_end` | `0` / `0` |
| `split_divider.color` | `"#323236"` |
| `split_divider.gradient` | 无(单色) |
| `split_divider.gradient.colors` | 无(配了渐变时至少 2 个颜色) |
| `split_divider.gradient.orientation` | `"Auto"`(沿线方向:竖线上下 / 横线左右) |
| `split_divider.opacity` | `1.0` |

各项含义与配方见 [§15.8 分割线外观](#158-分割线外观)。

### 22.8 分屏焦点

| 键 | 默认值 |
|---|---|
| `pane_focus.click_to_focus` | `true` |
| `pane_focus.number_shortcuts` | `true` |
| `pane_focus.hide_inactive_caret` | `true` |
| `pane_focus.trail` | `true` |
| `pane_focus.sync_trail` | `true` |
| `pane_focus.trail_ms` | `320`(时长倍率,`320` = 与窗格内尾迹同节奏) |
| `pane_focus.trail_color` | 无(跟随光标色,仅 `sync_trail = false` 时生效) |
| `pane_focus.landing` | `"caret"`(已废弃,仅为旧配置兼容而解析) |

各项含义见 [§15.9 分屏焦点](#159-分屏焦点)。

### 22.9 文件浏览器

| 键 | 默认值 |
|---|---|
| `files_gui.open_mode` | `"window"`(另开窗口) |
| `files_gui.editor` | 无(自动探测 `nvim` → `vim` → 记事本) |
| `files_gui.side_cols` | `30`(列,夹到 8–120) |
| `files_gui.show_hidden` | `false` |
| `files_gui.follow_cwd` | `true` |
| `files_gui.push_cwd` | `true` |

各项含义、三种打开方式与目录双向同步见 [§15.10 文件浏览器](#1510-文件浏览器)。

