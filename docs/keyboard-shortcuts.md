# VectorTerm 键位总览

> 一份完整的快捷键 / 鼠标操作清单。内容以源码 `src/commands.rs` 的 `key_lines()`、
> `src/main.rs` 的快捷键处理为权威来源,与命令面板的 `key` 命令输出保持一致。
>
> 想改键位见文末 [§用 `keys` 重映射](#用-keys-重映射键位) 与
> [配置手册 §15](config-manual.md#15-键位映射)。

## 速查表

| 类别 | 快捷键 | 作用 |
|---|---|---|
| 命令 / 搜索 | `Ctrl+Shift+P` | 打开命令面板(不必先开搜索) |
| | `Ctrl+Shift+F` | 打开 / 关闭历史搜索栏 |
| | 搜索框首字符 `/` | 从搜索栏切到命令面板 |
| 分屏 | `Ctrl+Shift+Alt+"` / `Ctrl+Shift+"` / `Ctrl+Shift+Alt+V` | 左右分屏 |
| | `Ctrl+Shift+Alt+%` / `Ctrl+Shift+%` / `Ctrl+Shift+Alt+H` | 上下分屏 |
| | `Ctrl+Shift+方向键` | 焦点移到相邻窗格 |
| | `Ctrl+Shift+X` | 关闭当前窗格(只剩一个则关标签) |
| | `Ctrl+Shift+Alt+方向键` | 调整分割线位置(窗格大小) |
| | `Alt+1..9` | 聚焦第 N 个窗格 |
| 文件 | `Ctrl+Shift+E` | 文件浏览器侧边栏开关 |
| | `/files` · `/settings` · `/reload` | 文件管理器 · 配置管理器 · 重载配置 |
| 标签 | `Ctrl+Shift+T` / `W` | 新建 / 关闭标签 |
| | `Ctrl+Tab` / `Ctrl+Shift+Tab` | 下一个 / 上一个标签 |
| | `Ctrl+1..9` | 跳到第 N 个标签 |
| 剪贴板 | `Ctrl+Shift+C` / `V` | 复制 / 粘贴 |
| | `Ctrl+Insert` / `Shift+Insert` | 复制 / 粘贴 |
| 回看 | `Shift+PageUp` / `PageDown` | 回看历史(向上 / 向下) |
| 会话 | `Ctrl+Shift+A` | 切回守护进程里其他未显示会话(开启会话保持时) |

---

## 命令面板与历史搜索

唤起方式有两种:

- **命令面板**:`Ctrl+Shift+P` 直接打开(再按一次关闭)。
- **历史搜索栏**:`Ctrl+Shift+F` 打开(再按一次关闭),输入框里第一个字符输 `/`
  就切到命令面板。

命令面板里可输入 `files` / `settings` / `help` / `key` / `reload` / `search`,
支持前缀过滤与 `Tab` 补全。

| 场景 | 键 | 作用 |
|---|---|---|
| 命令面板 | `↑` / `↓` | 上 / 下选择命令 |
| | `Tab` | 用当前输入补全命令名 |
| | `Enter` | 执行选中的命令 |
| | `Esc` | 关闭面板(在 `help`/`key` 输出视图里先退回列表) |
| | `Ctrl+Shift+F` | 切到历史搜索栏 |
| | `Ctrl+Shift+P` | 关闭面板 |
| | 任意字符 | 输入过滤词 |
| 历史搜索栏 | 输入文字 | 在全部历史里搜索、高亮匹配 |
| | `Enter` | 跳到下一个匹配(同时回看定位) |
| | `Shift+Enter` | 跳到上一个匹配 |
| | `↑` / `↓` | 上一个 / 下一个匹配 |
| | `Tab` | 切换大小写敏感 |
| | `Ctrl+Shift+P` | 直接切到命令面板 |
| | `Esc` | 关闭搜索栏 |
| `help` / `key` 输出视图 | `↑` / `↓` | 逐行上 / 下 |
| | `PgUp` / `PgDn` | 翻页(上一屏 / 下一屏) |
| | `Home` / `End` | 跳到首行 / 末行 |
| | `Esc` | 返回命令列表 |

> 命令面板和搜索栏是**互斥**的浮层:同一时刻只可能有一个处于激活态。

两个浮层都是**模态文本输入**:打开之后输入法上屏的中文直接进那个输入框,
不会再漏到终端里;浮层在屏上时终端的光标也会收起来(输入焦点已经搬走了),
关掉再回来。拼音还没上屏的组合串画在输入框里、带一条下划线,输入法候选窗
也跟着输入框走,不会飘回终端那边去。

---

## 中文输入法开着的时候

### 为什么会失配

Windows 的输入法接管字母/数字键时,会把 `WM_KEYDOWN` 的 `wParam` 换成
`VK_PROCESSKEY`(0xE5)。`lParam` 里的扫描码**没有**被改,但 winit 拿
`wParam` 去查布局,得到的 `logical_key` 就是 `Unidentified` ——
凡是按字符匹配的快捷键(`Ctrl+Shift+E`、`Ctrl+Shift+T`、`Ctrl+C`…)全部失配。
典型症状:

- 输入法是微软拼音时 `Ctrl+Shift+E` 打不开文件面板,切成「微软美国输入法」才行;
- `Ctrl+Shift+F` 却能开搜索栏 —— 因为它还额外比对了 `physical_key`,
  而物理键是从扫描码算的,输入法改不到。

### 现在的做法

窗口过程里做了一层子类化(`src/winhook.rs`):看到 `VK_PROCESSKEY` 就用
`MapVirtualKeyW` 把 `lParam` 的扫描码反推成真实虚拟键码,换回去再转给 winit。
应用收到的输入流和「没开输入法」完全一致,不需要在每个快捷键里各写一套
物理键兜底。

边界只有一条:**只对按住 `Ctrl` / `Alt` 的组合生效**。裸按字母原样留给输入法
组字 —— 换掉 `wParam` 的话,拼音的按键会同时被当成 ASCII 灌进终端。

这条子类化链两个渲染后端都装(`render = "cpu"` 也一样),因为它不只服务
交换链预配置。

### 排查

```bash
VECTORTERM_DEBUG_KEYS=1
```

日志里会看到成对的两行:

```text
[dbg] hook msg=0x0100 wp=0xE5 scan=0x12 held=1 ime_open=1
[dbg] VK_PROCESSKEY 还原 vk=0x45 scan=0x12 down
[dbg] key Pressed logical=Character("E") physical=Code(KeyE) ...
```

第一行是**消息层**看到的原始值(`wp=0xE5` 就是被输入法接管了),
第二行是还原结果,第三行是 winit 算出来的键 —— 三行连着看就能判断
到底是输入法捣乱还是键位真绑错了。

---

## 分屏与窗格

一个标签里可以并排多个终端会话(pane),每个 pane 独立持有自己的 shell /
滚动缓冲 / 选区。以下任一组合都能分屏,按不顺手的那组用。

| 快捷键 | 作用 |
|---|---|
| `Ctrl+Shift+Alt+"` | 左右分屏(中间一条竖杠,新 pane 在右半) |
| `Ctrl+Shift+"` | 同上,不按 Alt |
| `Ctrl+Shift+Alt+V` | 同上,纯字母别名 |
| `Ctrl+Shift+Alt+%` | 上下分屏(中间一条横杠,新 pane 在下半) |
| `Ctrl+Shift+%` | 同上,不按 Alt |
| `Ctrl+Shift+Alt+H` | 同上,纯字母别名 |
| `Ctrl+Shift+方向键` | 把焦点移到相邻 pane(按几何最近原则) |
| `Ctrl+Shift+X` | 关掉当前 pane(只剩一个 pane 时等同关标签) |
| `Ctrl+Shift+Alt+方向键` | 调整 pane 大小(移动分割线) |
| `Alt+1..9` | 聚焦第 N 个 pane(需 `pane_focus.number_shortcuts = true`,默认开) |

> `Ctrl+Shift+X` 只关**当前**窗格,旁边的窗格继续跑。窗格里还有程序在跑时
> (不只 shell 本身)会先弹一句 `● Really kill this pane?` + `[Y]es / [N]o`;
> 只剩一个窗格时它等价于关标签,文案换成关标签 / 关窗口那两句。判定和
> `quit_confirmation` 同一套,详见 [手册 §4.7](config-manual.md#47-quit_confirmation关窗--关标签--关窗格前确认)。

> `"` 是 `Shift+'`、`%` 是 `Shift+5`,按物理键即可,不用真的按出双引号。
> 别名不是重复功能:`Ctrl+Shift+Alt` 常被中文输入法、显卡驱动或 PowerToys 之类的
> 全局热键拦掉,这时用不带 Alt 的组合;懒得找符号键就用 `V` / `H`。
>
> 分屏组合按**物理键**判定,不看字符。Windows 把 `Ctrl+Alt` 当 AltGr,winit 在这个
> 组合下给不出 `"` / `%`(只给控制字符或 `Unidentified`),所以读 `physical_key`
>(`Quote` / `Digit5`)并用 `GetAsyncKeyState` 补上被 AltGr 过滤掉的 Ctrl/Alt 状态。

**鼠标操作(分屏)**:

- 鼠标**拖动分割线**:自由调整两侧 pane 大小(热区宽度、最小占比、步长都可配,
  见 [手册 §15.8.7](config-manual.md#1587-拖动分割栏与快捷键调整窗格大小))。
- 鼠标**点击窗格**:聚焦该窗格。只有聚焦窗格显示输入光标,切换时新窗格的光标
  立刻出现在真实位置。

---

## 文件与配置

| 快捷键 / 命令 | 作用 |
|---|---|
| `Ctrl+Shift+E` | 打开 / 收起**文件浏览器侧边栏**(从左边分出一条窄栏,面板从左边滑进来 / 滑回去) |
| `/files`(命令面板) | 把当前窗格变成文件管理器(铺满宿主 pane,分屏时只盖这一边) |
| `/settings`(命令面板) | 打开可视化配置管理器 |
| `/reload`(命令面板) | 重新加载 `config.lua` |
| `/help`(命令面板) | 列出全部命令与用法 |
| `/key`(命令面板) | 查看全部键位与鼠标操作(即本表) |
| `/search`(命令面板) | 切回历史搜索栏 |

文件管理器详细键位见 [§文件浏览器专用键](#文件浏览器专用键)。

---

## 标签 / 剪贴板 / 回看

| 快捷键 | 作用 |
|---|---|
| `Ctrl+Shift+T` | 新建标签 |
| `Ctrl+Shift+W` | 关闭当前标签(里面有程序在跑时先弹确认) |
| `Ctrl+Tab` | 下一个标签 |
| `Ctrl+Shift+Tab` | 上一个标签 |
| `Ctrl+1` … `Ctrl+9` | 跳到第 N 个标签 |
| `Ctrl+Shift+A` | 切回守护进程里其他未显示会话(开启 `session_persistence` 时) |
| `Ctrl+Shift+C` | 复制选区 |
| `Ctrl+Shift+V` | 粘贴 |
| `Ctrl+Insert` | 复制选区 |
| `Shift+Insert` | 粘贴 |
| `Shift+PageUp` | 回看历史(向上翻一页) |
| `Shift+PageDown` | 回看历史(向下翻一页) |

- `selection.ctrl_shift_v = "copy"` 时 `C` / `V` 对调 —— 把 `V` 当复制键用。
- 回看是**亚行缓动动画**,不是整数行瞬移;按键输入会优先回到窗口底部。

**鼠标回看操作**:

- 鼠标**滚轮**:回看(亚行缓动)。
- 拖动**滚动条**:回看;点轨道:**翻一页**;拖选文字到窗口边缘会**自动滚屏并继续扩选**。

**鼠标选区**:双击选词、三击选行,松开即复制;应用抓了鼠标时按住 `Shift` 仍可本地选择。

---

## 文件浏览器专用键

文件管理器有两种形态:命令面板的 `/files` 是**铺满当前 pane** 的列表,
`Ctrl+Shift+E` 是从左侧分出的**侧边栏**。两者都遵循下面的键位,且**属于某个 pane**
—— 在哪个终端打开就只铺那个 pane,点到另一个终端会连列表带 cwd 一起移交。

当面板**获得焦点**时:

| 键 | 作用 |
|---|---|
| `↑` / `↓` | 上 / 下移动选择 |
| `PageUp` / `PageDown` | 上 / 下翻页 |
| `Home` / `End` | 跳到首 / 尾 |
| `Enter` 或 `→` | 进目录,或把文件交给编辑器打开 |
| `Backspace` 或 `←` | 回上级目录(并同步终端 cwd) |
| `.` | 切换显示隐藏文件 |
| `r` | 刷新列表 |
| `q` 或 `Esc` | 关闭面板 |
| 其它可打印字符 | 吞掉(不漏给 shell,避免边看列表边打字把命令打进终端) |

> 面板与终端**目录双向同步**:终端里 `cd`,面板跟着走;面板里进目录会给终端写一行
> `cd`(开关 `files_gui.follow_cwd` / `files_gui.push_cwd`,默认都开)。
> 回车打开文件默认**另开一个窗口**用 `nvim` / `vim`(可配 `files_gui.open_mode` 改成
> 右侧分屏或当前终端 shell),见 [手册 §15.10](config-manual.md#1510-文件浏览器)。

---

## 关闭确认框

窗口 / 标签里有程序在跑时,关闭会先在内容区正中问一句 `[Y]es / [N]o`:

| 键 | 作用 |
|---|---|
| `y` / `Y` | 确认关闭 |
| `n` / `N` / `Esc` | 取消 |

`Enter` / `Tab` / 方向键在确认框里没有任何作用(与 wezterm 一致)。

---

## 用 `keys` 重映射键位

内置快捷键都可以在 `config.lua` 里用 `keys` 表覆盖或新增。例如:

```lua
keys = {
  ["ctrl+shift+r"] = "reload",
  ["f12"]          = "send:dir\r",
  ["ctrl+alt+t"]   = function() VectorTerm.notify("hi") end,
}
```

要点:

- 键名大小写无关、修饰键顺序无关(`shift+ctrl+t` 等价 `ctrl+shift+t`)。
- 值为 `"send:..."` 时后面是**字面量**,想回车执行必须写 `\r`。
- `keys` 表**最先派发**:命中就拦截,不再走内置快捷键,所以能覆盖 `Ctrl+Shift+T` 等。
- 可用动作字符串与键名、wezterm 风格数组写法,见
  [配置手册 §15](config-manual.md#15-键位映射)。
- 分屏动作(`SplitPane`)没有独立 Lua 动作名,是内置快捷键直连;但 `keys` 可以
  `['ctrl+alt+shift+"'] = function() ... end` 这样的键名**接管**分屏键。
- **关窗格**有独立动作名,能绑到任意键:`["ctrl+alt+w"] = "close_pane"`,
  wezterm 风格写 `{ key = 'W', mods = 'CTRL|ALT', action = wezterm.action.CloseCurrentPane }`。
  内置的 `Ctrl+Shift+X` 仍然保留,加一条只是多一个键。
- `Ctrl+Shift+Alt+方向键` 调整窗格大小的组合是**内置硬编码**,不能在 `keys` 表里
  绑到别的键上,只能拦截。
- 键位也可以拆成独立文件,用配置里的 `keys_file = "vim.lua"` 引用(详见
  [配置手册 §15.11](config-manual.md#1511-键位方案文件keys_file))。外部文件里的
  `keys` 是**基础层**,配置内联 `keys` 是**覆盖层**,两层都没写出来的组合键一律
  **退回终端内置默认键位**——只覆盖想改的键,其余保持默认即可,不必抄全整张表。

---

## 说明

- 所有 `Super`(Win 键)组合归操作系统,程序不抢占。
- 派发顺序:`keys` 绑定 → 内置标签栏快捷键 → 内置分屏快捷键 → 内置剪贴板快捷键
  → `Shift+PageUp/PageDown` 回看 → 交给终端状态机编码写入 PTY。
- 诊断:设 `VECTORTERM_DEBUG_KEYS=1` 会把每个按键事件(`logical_key` / `physical_key` /
  修饰键)和命中的分屏快捷键写进日志,用于排查 Windows 上 IME / AltGr 改写组合键的场景。
