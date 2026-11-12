# 8. 构建与运行

> 构建入口、环境变量、发行版子系统与调试终端、换机器跑不起来的排查。

## 编译命令速查

所有命令都通过唯一入口 `build.cmd` 执行(`build.ps1` 只是 PowerShell 转发器,
行为完全一致)。它会在需要的时候自动跑 `setup.ps1` 拉取 wezterm-term 源码、
用 `vcvars64.bat` 初始化 MSVC 环境、设 `CRATE_CC_NO_DEFAULTS=1`,所以**不要**
直接裸跑 `cargo build`(原因见 [§为什么不能直接 cargo build](#为什么不能直接-cargo-build))。

| 命令 | 作用 |
|---|---|
| `build.cmd` | 调试构建(`cargo build`,控制台子系统,`cargo run` 可见日志) |
| `build.cmd --release` | **发布构建**(`cargo build --release`,GUI 子系统,双击无控制台)。要分发的就是它 |
| `build.cmd test [--release] [-- --nocapture]` | 跑工作区单元测试;任意 cargo 参数透传 |
| `build.cmd clippy -- -D warnings` | 静态检查 |
| `build.cmd fmt` | 用 rustfmt 格式化代码 |
| `build.cmd run -- --config config\config.lua` | 运行并把 `--` 之后的内容作为程序参数传给 `VectorTerm.exe` |
| `build.cmd --config config\config.lua` | 等价写法:自动按 `run --release -- ...` 路由 |
| `build.cmd doc` | 生成 Rust API 文档 |
| `build.cmd clean` | 清掉 `target` 增量产物 |
| `build.cmd --release -p vectorterm-settings` | 只构建配置管理器(`vectorterm-settings.exe`) |
| `build.cmd installer` | 发布构建 + Inno Setup 打包成安装程序 |
| `build.cmd package` | `installer` 的别名 |
| `build.cmd installer --no-build` | 复用现有 `target\release`,只重新打包 |
| `build.cmd installer --iscc "D:\...\ISCC.exe"` | 指定 Inno 编译器位置 |
| `build.cmd --help` | 打印完整用法、环境变量与全部开关 |

> 首参数只要是 cargo 子命令(`build` / `test` / `run` / `check` / `clippy` / `fmt` /
> `bench` / `clean` / `doc` / `tree` / `fix` / `metadata` / `rustc` …),就按该子命令执行;
> 否则整串参数交给 `cargo build`。参数保留原样(含空格的路径不会被拆开)。

### 几种构建方式

1. **推荐:`build.cmd`**(Windows,最省心)。自动处理 MSVC 环境、上游源码、配置同步与打包,
   一条命令覆盖 debug / release / test / 安装包。
2. **PowerShell:`build.ps1`**。功能和 `build.cmd` 完全一致,适合已经习惯 PS 的环境:
   `powershell -ExecutionPolicy Bypass -File .\build.ps1 --release`。
3. **裸 `cargo build`**(不推荐,仅 CI 或已配好环境时)。本机是 VS 2026 + 新版 `cc-rs`,
   必须先在「Developer Command Prompt」里(或手动 `call vcvars64.bat`)把 `cl.exe` 加进 PATH,
   再设 `CRATE_CC_NO_DEFAULTS=1`,否则 `cl.exe` 加载 `c1.dll` 失败(报 `D8050`)。
   `build.cmd` 正是替你做了这两步。
4. **CI / 容器**:用 `VECTORTERM_SKIP_VCVARS=1` 假设工具链已就绪,其余交给脚本。

要求:

- Windows 10 1809+(ConPTY)
- Rust stable(MSVC 工具链,实测 1.98.1)
- Visual Studio 2019/2022/2026 Build Tools(含 C++ 工具集与 Windows SDK;mlua 的 vendored Lua 也需要 `cl.exe`)
- git(拉取 wezterm 源码)

```bat
:: 构建(唯一入口:build.cmd;build.ps1 只是它的 PowerShell 转发器)
build.cmd                            :: cargo build
build.cmd --release                  :: cargo build --release
build.cmd test --release -- --nocapture   :: 任意 cargo 子命令透传
build.cmd clippy -- -D warnings
build.cmd run -- --config config\config.lua
build.cmd installer                  :: release 构建 + Inno Setup 安装包
build.cmd clean
build.cmd --help                     :: 完整用法与开关

:: 运行
target\release\VectorTerm.exe
target\release\VectorTerm.exe --config C:/path/to/config.lua
```

**发行版不会弹出控制台窗口。** `cargo build --release` 产出的是 **GUI 子系统**二进制
(`src/main.rs` 顶部的 `windows_subsystem = "windows"`,调试构建保持控制台子系统,
`cargo run` 照样能看日志),双击 `VectorTerm.exe` 只有终端窗口,没有那个黑色控制台。
发行版"丢掉"的两个标准流在启动第一件事就兜住(指向 `NUL`),**不会主动挂父控制台** ——
进程一旦挂过控制台,之后再 `AllocConsole` 开独立调试窗口会直接
`ERROR_ACCESS_DENIED`,而且日志混进启动它的 cmd 输出里也违背"调试输出不进当前终端":

| 软件自身的日志 | 去哪 |
|---|---|
| 任何启动方式 | `~/.config/VectorTerm/VectorTerm.log`(全量)+ 出错时弹出的独立 `VectorTerm Debug Console` |
| 已经重定向(`VectorTerm.exe > out.txt`) | stderr 原样写文件,同时仍写 `VectorTerm.log` |
| `VECTORTERM_DEBUG_CONSOLE=0` | 只写文件,不弹窗 |

源码里所有 `eprintln!` 都走统一的 `wlog!` / `werr!` 通道,`werr!` 额外负责把调试终端
调出来;**当前 shell 终端永远不会被软件日志污染**。

`build.cmd --release` 结束后会读一次 PE 头并打印 `[build] PE subsystem: ...`;
万一哪天退化回 `Windows CUI`(会弹控制台窗口),脚本会直接 WARN 出来。

### 图标与版本资源

图标的唯一母版是仓库根目录的 **`VectorTermLogo.png`**(1080×1080,深色圆角方块 + 白色线稿,
圆角外透明)。改完母版跑一次:

```bash
python tools/gen_icons.py       # 需要 Pillow
```

它把整套分辨率梯子生成到 `assets/icons/`,各位置按尺寸取用:

| 位置 | 文件 | 说明 |
|---|---|---|
| 窗口图标(任务栏 / Alt+Tab / 标题栏左上) | `VectorTerm-256.png` | winit 的 `Icon::from_rgba` 只收一张 RGBA,256 覆盖 Windows 会用的最大尺寸 |
| exe 图标(资源管理器 / 快捷方式 / 开始菜单) | `VectorTerm.ico` | 16/20/24/32/40/48/64/96/128/256 十档,系统按 DPI 取最合适的一档 |
| README 头图 | `VectorTerm-256.png` | 按 128 宽显示,2 倍屏不糊 |
| 文档 / 商店页 / 高 DPI | `VectorTerm-16…512.png` | 按需取 |

`src/main.rs` 把 `VectorTerm-256.png` 用 `include_bytes!` 编进二进制,并且**两档都设**:
`with_window_icon` 对应 `WM_SETICON(ICON_SMALL)`(标题栏左上角),`with_taskbar_icon`
对应 `WM_SETICON(ICON_BIG)`(任务栏按钮 / Alt+Tab)。只设 SMALL 的话,任务栏拿不到 BIG
就退回去读 exe 资源段,而那段有没有图标取决于构建环境 —— 这是个很容易踩的坑。

`build.rs` 负责 exe 的文件图标与版本信息,按机器上有没有 Windows SDK 分两条路:

1. **有 `rc.exe`**:写一份 `.rc` 让 SDK 编出 `.res`,图标和 `VERSIONINFO` 都齐;
2. **没有 `rc.exe`**:`build.rs` 自己按 RES 格式写出图标资源。图标照样进 exe
   (资源管理器 / 快捷方式 / 开始菜单都正常),只有版本信息会缺,构建时会打印一条 warning。

> 自己写 `.res` 时有两个坑,踩过:
> - 文件开头是**一条"空资源"**(`DataSize=0, HeaderSize=32`),不是 32 个零字节,
>   否则 `cvtres` 直接报 `CVT1107 文件损坏`;
> - Type / Name 是"数值还是字符串"的可变槽位,数值要写成**低 WORD `0xFFFF` + 高 WORD id**,
>   不是常见的“高位标记”。

小尺寸(≤48)生成时会做一次轻微反锐化:深底白线稿缩下去边缘发灰,不锐化 16px 就糊成一团。
**不要直接编辑 `assets/icons/` 下的产物**,它是生成的。

- 查找 `rc.exe` 的顺序:`RC` 环境变量 → `PATH`(用 `build.cmd` 构建时 `vcvars64.bat`
  已经把它加进去了)→ `WindowsSdkDir` → `%ProgramFiles(x86)%\Windows Kits\10\bin\*`
  (取最高版本)。手动指定:`set RC=E:\Windows Kits\10\bin\10.0.26100.0\x64\rc.exe`。
- 构建目录路径里**不能有空格**:rustc 按空白拆分 link-arg 且不认引号,所以脚本传的是
  相对包根目录的 `.res` 路径;路径含空格时会跳过并告警。
- 换完图标如果资源管理器 / 任务栏还显示旧的,那是 Windows 的图标缓存:
  重启 explorer,或跑一次 `ie4uinit.exe -show`。已经开着的窗口不会自己换图标,要重开。

## 配置管理器(vectorterm-settings)

仓库是个 **cargo workspace**:`vectorterm`(终端本体)和 `settings-gui`(配置管理器)
是两个**独立成员 crate**,各自编译、各自产物,代码零耦合。配置管理器用的是 Rust +
egui,不依赖终端本体的任何模块,可以单独分发。

```bat
build.cmd --release -p vectorterm-settings   :: 只构建配置管理器
cargo run -p vectorterm-settings             :: 开发时直接跑
target\release\vectorterm-settings.exe       :: 产物(装进 PATH 后终端里敲 vectorterm-settings 打开)
```

workspace 只产出上面两个可执行文件(`VectorTerm.exe` 与 `vectorterm-settings.exe`)。
早期版本还有第三个 `files-gui.exe`:它把当前目录编码成一条私有 OSC 标记打到
stdout,`vectorterm` 从 PTY 输出流里扫出来就打开文件浏览器,所以能在 shell 里敲
`files-gui` 开面板(路径天然等于敲命令时的 cwd)。**这条链路已彻底移除** ——
现在唤起终端自身 UI 一律走命令面板(`Ctrl+Shift+P`,或 `Ctrl+Shift+F` 后输
`/`,再输 `files`),不需要往 `PATH` 里装任何小工具,按键也不会漏给 shell;
详见 [手册 §11.6](config-manual.md#116-命令面板替代-shell-内命令)。

文件浏览器默认**另开一个窗口**打开编辑器,靠的是再启动一次本程序:

```text
VectorTerm.exe [--config <配置>] --open-file <文件路径> -- <编辑器程序> [固定参数...]
```

新进程把这几个参数翻译成"shell = 编辑器、参数末尾追加文件、cwd = 文件所在目录",
然后照常建窗 —— 所以新窗口是一个**独立进程**(独立 PTY、独立会话),和当前窗口
的分屏 / 标签 / shell 完全无关。同样的命令也能直接在命令行里用来验证。

三栏界面:左栏多配置文件管理(程序目录 / 用户目录 / 示例),中栏全部配置项(按手册
分组、可搜索),右栏 Lua 实时预览(可切"编辑"模式手动改)。颜色选项走专门的拾色器:
预设色板 + RGB 滑块 + HEX 输入,点色块即开;未填的颜色项会直接显示默认色的色块与
HEX 占位提示。「分屏与分割线」分组带两张实时缩略图,调线宽、文字间距、两端留白、
渐变色停靠点时即时重画;旁边的「分屏焦点」分组可调点击聚焦、是否隐藏非聚焦窗格的
输入光标、飞行动画时长(「落点」字段已废弃,仅为兼容旧配置保留),顶部还有 **「与普通光标尾迹同步」** 按钮 —— 点一下就把
`pane_focus.sync_trail` / `trail_color` 删回默认,跨窗飞行直接继承 `cursor.trail_*`
的速度、颜色与透明度(按钮旁边实时显示继承到的 `decay` 与颜色)。
预览里左半 pane 的输入光标也跟着变,不用切回终端窗口看效果。

键盘:`Ctrl+S` 保存、`Ctrl+F` 聚焦搜索框、`Ctrl+O` 打开一个已有的 `.lua`。
状态栏常驻显示这三个快捷键。实现上用 `InputState::consume_key` 而不是
`modifiers.command && key_pressed` —— 后者在焦点落进右侧代码编辑器或搜索框时
可能被 `TextEdit` 先消费掉,表现为"快捷键没反应"。

核心保证:**改配置只替换那几个字符**,注释、空行、对齐原样保留;删键只删那一行,
不会连带删掉整组说明注释。见 `settings-gui/src/lua.rs`(解析)与 `store.rs`(改写),
往返有单元测试兜底。

## 打包安装包(Inno Setup)

`build.cmd installer` 把 release 产物打成 `VectorTerm-<版本>-setup.exe`:

```bat
build.cmd installer                 :: 先 cargo build --release,再编译安装脚本
build.cmd installer --no-build      :: 复用现有的 target\release\VectorTerm.exe,只打包
build.cmd installer --iscc "D:\Tools\Inno Setup 7\ISCC.exe"
build.cmd package                   :: installer 的别名
```

产物固定落在 `<target>\installer\VectorTerm-<版本>-setup.exe`(实测约 18 MB,
`lzma2/max` 压缩;字体是 `include_bytes!` 内嵌在 exe 里的,压不下去)。
版本号从 `Cargo.toml` 的 `[workspace.package] version` 读,和 exe 的资源版本、
"应用和功能"里显示的名字永远一致 —— 只有一处可改(GitHub Actions 打 tag 时
按 tag 回写它,见本文末的[自动发布](#自动发布github-actions))。**workspace 会把 VectorTerm 和 vectorterm-settings 两个 exe
都打进去**(装进同一目录,PATH 里一次加两条命令)。

需要 **Inno Setup 6.5+**(脚本用了 `x64compatible` 架构名,并引用 6.5 版的中文
语言文件;7.x 同样可用):

```powershell
winget install --id JRSoftware.InnoSetup -e
```

脚本不依赖安装路径,按下面的顺序找 `ISCC.exe`,找到第一个就用:

1. `VECTORTERM_ISCC` 环境变量(可以给到 `ISCC.exe`,也可以给到它所在目录)
2. `PATH` 里的 `iscc.exe`
3. `%ProgramFiles%` / `%ProgramFiles(x86)%` / `%LOCALAPPDATA%\Programs` 下的
   `Inno Setup 7`、`Inno Setup 6`
4. **其它盘符**的 `\Program Files\Inno Setup {6,7}`(装在 `E:\` 上也能找到)
5. 卸载注册表项 `Inno Setup {6,7}_is1` 的 `InstallLocation`

全都找不到时会直接给出 `winget` 命令,不会静默失败。

### 安装包做了什么

| 项 | 行为 |
|---|---|
| 安装位置 | 默认 **当前用户**:`%LOCALAPPDATA%\Programs\VectorTerm`,**不需要管理员**;向导里选"为所有用户安装"才会进 `Program Files` 并请求提权(`PrivilegesRequired=lowest` + 允许在向导/命令行覆盖) |
| 主程序 | `VectorTerm.exe` |
| 配置管理器 | `vectorterm-settings.exe` —— 独立的图形化配置工具(见下),和终端本体一起打进同一目录 |
| 默认配置 | `config\config.lua` —— 只在缺失时放一份(`onlyifdoesntexist`),升级不会覆盖你改过的内容 |
| 内置主题 | `config\*.lua` 里除 `config.lua` 之外的文件(当前 `deepseek.lua` / `chatgpt.lua` / `claude.lua` / `gemini.lua` 四套配色)—— 同时**已 `include_str!` 编进 exe**,这里的副本只是方便打开/复制/`--config` 试用;随版本升级同步覆盖,它们是只读预设,不是用户配置 |
| 附带文档 | `config.example.lua`、`README.md`、`LICENSE`(英文原文)、`LICENSE.zh-CN.txt`(中文译本)、`THIRD-PARTY-NOTICES.md`、`docs\config-manual.md` |
| 字体许可 | `licenses\` —— 内嵌的三款字体(JetBrains Mono / Symbols Nerd Font / Noto Color Emoji)的 **SIL OFL 全文**与来源说明。OFL 要求随二进制分发许可文本,单独分发 `VectorTerm.exe` 时也别落下 |
| 开始菜单 / 桌面 | 开始菜单快捷方式默认建(含配置管理器);桌面图标是可选项(默认不勾) |
| App Paths | 写 `…\App Paths\VectorTerm.exe` 与 `…\App Paths\vectorterm-settings.exe`,所以 **Win+R** 里敲 `vectorterm` 或 `vectorterm-settings` 都能启动 |
| 右键菜单 | 可选(默认勾):在**文件夹空白处**、**文件夹图标上**、**盘符上**右键都有「在此处打开 VectorTerm」,启动参数是把该路径塞进 `--cwd`(见下) |
| 卸载 | 删文件、删快捷方式、删 App Paths 键、删右键菜单的 `…\Classes\…\shell\VectorTerm` 三个键、把 VectorTerm 从 PATH 里摘掉 |

`config\*.lua` 的主题现在有**三条**分发路径:

1. **编进 exe** —— `src/config.rs` 的 `BUILTIN_THEMES` 用 `include_str!` 把
   `config/deepseek.lua`、`chatgpt.lua`、`claude.lua`、`gemini.lua` 嵌进二进制。
   所以单个 `VectorTerm.exe` 拷到任何机器上,配置里写 `theme = "chatgpt"` 都能用,
   一个 `config\` 目录都没有也不影响。改配色 → 改 `config/<name>.lua` → 重新
   构建,内嵌副本自动跟着变(测试 `builtin_themes_are_embedded_and_parse` 会
   核对"目录里的每套都在二进制里")。
2. **随包同步** —— `build.cmd` 第 5 段每次构建都把 `config\*.lua`(排除
   `config.lua`)同步到 `target\<profile>\config\`,方便直接
   `--config config\chatgpt.lua` 试色,或复制一份改成自己的。
3. **安装包** —— 用 `Excludes: "config.lua"` 的通配规则装进 `{app}\config\`。

查找顺序是**磁盘优先、内嵌兜底**:用户目录 / `<exe>/config/` / 工作目录里有同名
文件就用文件,都没有才落到 exe 内置的那份。详见
[手册 §2.5](config-manual.md#25-内置主题)。

### PATH 那一项

附加任务里有一条 **"将 VectorTerm 加入 PATH 环境变量"**,**默认勾选**,取消勾选就完全不碰
你的 PATH。写的是哪个 hive 跟着安装模式走:

| 安装模式 | 改的 PATH |
|---|---|
| 当前用户(默认) | `HKCU\Environment` 的 `Path` |
| 为所有用户 | `HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment` 的 `Path` |

实现上有几个必须踩准的点:

- **整段比较,不是子串**:`C:\Tools` 和 `C:\ToolsExtra` 不能算同一个,否则会误判"已存在"
  而跳过真正该加的路径;
- **重复安装不追加第二次**:覆盖安装 / 修复安装会把路径判重后跳过;
- **写完要广播** `WM_SETTINGCHANGE`(`SendMessageTimeoutW(HWND_BROADCAST, …)`),
  否则已经开着的资源管理器 / 终端还是旧 PATH,新开的窗口要重启 explorer 才认;
- **卸载只摘自己那一段**,PATH 里别的条目(包括空段那种脏数据)原样保留;
- **卸载后能逐字节还原**:安装前会把"PATH 末尾有几个分号"记到
  `HKCU\Software\VectorTerm`,卸载时按原数补回 —— 因为实现是"按 `;` 切段再拼回去",
  末尾空段在切的时候天然被丢掉,不记的话 `…;Tools;` 会变成 `…;Tools`
  (少一个字符,语义等价但用对比工具会看成没卸干净)。这条记录卸载完即删。
- **摘条目不看安装时的勾选状态**,而是查 `Path` 里有没有这一段:装的时候没勾、
  或者你后来手工把这段删了,卸载都只是"没找到,跳过",不会报错也不会误删。

> 广播只影响**之后启动**的进程。安装完已经在跑的终端窗口里 `vectorterm` 仍然"不是命令",
> 新开一个窗口就好 —— 这不是没生效。

### 右键菜单那一项

附加任务里的 **"在资源管理器右键菜单加入「在此处打开 VectorTerm」"**(默认勾选)会写三处:

| 注册表位置 | 右键在哪 | 传进去的路径 |
|---|---|---|
| `…\Classes\Directory\Background\shell\VectorTerm` | 文件夹内**空白处** | `%V`(当前目录) |
| `…\Classes\Directory\shell\VectorTerm` | 文件夹**图标**上 | `%1`(被点的文件夹) |
| `…\Classes\Drive\shell\VectorTerm` | **盘符**上 | `%1`(如 `D:\`) |

三处共用 `"<exe>" --cwd "<路径>\."`,程序侧由 `src/main.rs` 的 `parse_cwd_arg`
接管(`-d` 是 Windows Terminal 风格的别名)。几个必须踩准的点:

- **盘根的引号陷阱(为什么参数尾巴多一个 `\.`)**:在 `D:\` 下右键时 `%V` 本身就是
  `D:\`,直接拼成 `--cwd "D:\"` 的话,Windows 命令行解析会把 `\"` 当成**转义的引号** ——
  不只是这条参数变成 `D:"`,连后面的 `--cwd` 之后的参数都会被并进同一个实参里,
  整条命令行拆错。补一个 `.` 收尾(`"D:\."`)就绕开了,`tidy_cwd_arg` 再把补的那段剪掉、
  给光秃秃的盘符补回反斜杠。这个坑实测过:不加 `\.` 时盘符右键开出来的窗口用的是
  用户默认配置和默认目录,而且完全不报错,极难看出来。
- **`MultiSelectModel=Single`**:选中多个文件夹时不显示这一项(verb 只认一个路径),
  免得点了只开其中某一个。
- **开了 `session_persistence` 也不影响**:带 `--cwd` 启动会**跳过"恢复最近会话"**
  (否则右键点出来的是上次的老会话,目录还是旧的,看起来就像右键没生效),
  但仍走 mux 托管,这个新会话关掉之后照样能被恢复。日志里会写一句
  「命令行指定了目录:跳过会话恢复,按该目录新开」。
- **写 `HKCU\Software\Classes`**("为所有用户安装"时是 `HKLM`),不需要管理员,
  卸载时 `uninsdeletekey` 连 `command` 子键一起清掉。
- **Windows 11 只把它放进「显示更多选项」**:新菜单默认只显示打包过的
  `IExplorerCommand`(需要 MSIX sparse package),老式 verb 一律落在二级菜单里。
  `Shift + 右键` 或右键 →「显示更多选项」就能看到,这是系统行为,不是没装好。
- **改完不用重启 explorer**:verb 是每次右键现查注册表的。

不想用安装包、直接跑 `target\release\VectorTerm.exe` 时,可以手动注册同一套条目:

```powershell
powershell -ExecutionPolicy Bypass -File .\context-menu.ps1              # 注册(自动找 target\release\VectorTerm.exe)
powershell -ExecutionPolicy Bypass -File .\context-menu.ps1 -Remove      # 撤销
powershell -ExecutionPolicy Bypass -File .\context-menu.ps1 -ExePath D:\apps\VectorTerm.exe
```

脚本写的是 `HKCU`,同样的三处 verb、同样的 `--cwd`,和安装包结果一致。

### 界面语言与许可页

向导**只有简体中文**,不弹语言选择框:

- `[Languages]` 指向仓库里的 `installer\ChineseSimplified.isl`(取自 Inno Setup 7
  自带的官方中文翻译,维护者 Zhenghan Yang),**不用** `compiler:Languages\...` ——
  只装了英文版 Inno 的机器上那个路径不存在,编译会直接失败。
- 这份翻译是 6.5.0+ 格式,所以最低要求是 **Inno Setup 6.5**。
- 产品描述、附加任务、快捷方式名、完成页那一句都已改成纯中文,不再带 `/ English`;
  页面上剩下的英文只有 `VectorTerm`、`PATH`、`LICENSE` 这类专有名词。
- 许可页显示 `LICENSE.zh-CN.txt`(中文参考译本),页脚注明"以 LICENSE 英文原文为准";
  两份文件都会装进 `{app}`。
- 许可页**默认选中"我同意"**(`InitializeWizard` 里置 `LicenseAcceptedRadio`),
  少一次点击;不想接受的照样能改选。

许可协议是 **PolyForm Noncommercial 1.0.0** —— 非商业用途随意使用、修改、再分发,
商业用途不获授权。换协议要同时改三处:`LICENSE`(英文正文、具法律效力)、
`LICENSE.zh-CN.txt`(向导里显示的中文)、`Cargo.toml` 的 `license` 字段。

### 改安装脚本时注意

- 文件是 **UTF-8 with BOM**。Inno 只在有 BOM 时按 UTF-8 解码,否则按 ANSI,
  中文注释和任务描述会变乱码(和 `build.cmd` 必须纯 ASCII 是同一个坑的两面)。
- 常量别用 `HWND_BROADCAST` 这类名字:**Inno 7 的脚本引擎已经预置了它**,
  重复定义会直接 `Duplicate identifier` 编译失败;脚本里统一加了 `WT_` 前缀。
- `/O="path"`(带等号)ISCC 不认,只能写 `/O"path"`;`/D"名字=含空格的值"` 可以。
- 版本号必须是纯数字点分(`1.2.3`),因为要塞进 `VersionInfoVersion`;
  带 `-beta` 之类后缀会让 ISCC 报错。
- `LICENSE.zh-CN.txt` 必须是 **UTF-8 with BOM + CRLF** —— 许可页是由 RTF 控件加载
  纯文本,没有 BOM 就按 ANSI 读,中文全变问号。换行用 LF 则整段挤成一行。

PowerShell 下等价写法(需要放开执行策略,因为 `.ps1` 未签名):
```powershell
powershell -ExecutionPolicy Bypass -File .\build.ps1 --release
powershell -ExecutionPolicy Bypass -File .\build.ps1 test --release -- --nocapture
```

`build.cmd` 会依次:定位 cargo → 若 `.wezterm-src` 缺失**自动执行一次 `setup.ps1`** →
用 `vswhere` 找 VC++ 工具集并 `call vcvars64.bat` → 设 `CRATE_CC_NO_DEFAULTS=1` →
按子命令调用 cargo → 打印耗时与 exe 路径 → 需要时同步默认配置与内置主题。
源码已就绪、或已在"Developer Command Prompt"里(cl.exe 在 PATH)时会跳过对应步骤,
所以重复构建没有额外开销。`setup.ps1` / `setup.sh` 仍可手动单独执行,幂等。

命令行的第一个参数如果是 cargo 子命令(`build` / `test` / `run` / `check` / `clippy` /
`fmt` / `bench` / `clean` / `doc` / `tree` / `fix` / `metadata` / `rustc` 等),就按该子命令执行;
否则整串参数交给 `cargo build`。参数用数组/PowerShell 转发,**含空格的路径不会被拆开**。

可用环境变量开关:

| 变量 | 作用 |
|---|---|
| `VECTORTERM_NO_SETUP=1` | 上游源码缺失时不自动跑 `setup.ps1`,改为报错 |
| `VECTORTERM_SKIP_VCVARS=1` | 假定 MSVC 环境已就绪,跳过 vswhere/vcvars(CI 用) |
| `VECTORTERM_NO_CC_FIX=1` | 不设置 `CRATE_CC_NO_DEFAULTS=1` |
| `VECTORTERM_NO_CONFIG_COPY=1` | 不把默认配置 / 内置主题复制到产物目录 |
| `VECTORTERM_TARGET_DIR=...` | 配置复制的目标目录(也支持标准 `CARGO_TARGET_DIR`) |
| `VECTORTERM_ISCC=...` | `build.cmd installer` 用的 `ISCC.exe` 路径(文件或所在目录) |

运行时诊断开关(不影响正常使用):

| 变量 | 作用 |
|---|---|
| `VECTORTERM_START_TABS=N` | 启动时直接开 N 个标签(1..9),省去手点 |
| `VECTORTERM_DUMP_FRAME=路径` | 首帧渲染后用 CPU 路径重绘并导出 24 位 BMP,排查显示问题 / 无截图环境核对画面 |
| `VECTORTERM_AUTO_SPLIT=v` / `h` / `vh` | 启动 1.5s 后自动分屏(逐字符执行,每次分当前 pane,可造出三窗格嵌套),配 `VECTORTERM_DUMP_FRAME` 做离屏核对 |
| `VECTORTERM_AUTO_FILES=full` / `side` | 启动 1.2s 后打开文件浏览器(`VECTORTERM_AUTO_FILES_DIR=目录` 指定起始目录),配 `VECTORTERM_DUMP_FRAME` 离屏核对面板 |
| `VECTORTERM_AUTO_FILES_OPEN=文件名` | 接着把列表里这一项"回车"(走真实的打开路径,含写进当前终端) |
| `VECTORTERM_AUTO_TAB=new@800,close@3000,no@3500,yes@4500` | 到点依次执行标签动作(`new` 新建 / `close` 走"该问就问"的关闭 / `yes` `no` 回答确认框),同样提前唤醒。**键盘注入在这儿不管用** —— 窗口没有键盘焦点时 `SendInput` 的键会被静默丢掉,所以走的是和 `×` / `Ctrl+Shift+W` 同一个内部入口 |
| `VECTORTERM_DEBUG_KEYS=1` | 把每个按键事件(`logical_key` / `physical_key` / `text` / 修饰键)和命中的分屏快捷键写进日志(`[dbg] key` / `[dbg] pane shortcut`),用来排查 Windows 上 IME / AltGr 把组合键改写的场景。`python tools/diagnostics/panel_resize_probe.py release` 会读它 |
| `VECTORTERM_DEBUG_CWD=1` | 把文件面板与终端的目录同步写进日志(`[cwdsync]` 行):基线、shell 目录变化、两个方向各写了什么。`python tools/diagnostics/cwd_sync_probe.py release` 靠它核对双向同步 |
| `VECTORTERM_FILES_EDITOR=程序名` | 临时覆盖 `files_gui.editor`,用来离线核对"编辑器找不到"底部提示(填个不存在的名字即可) |
| `VECTORTERM_AUTO_FOCUS=N` / `N@毫秒` | 到点(默认 2.2s)把焦点切到第 N 个 pane,配下面的导帧钩子核对跨窗格光标尾迹。`N@5000` 可以等分屏动画彻底收尾再切 |
| `VECTORTERM_DUMP_AT=毫秒[,毫秒...]` | 到点各导一帧;给多个时刻时路径自动加 `-<毫秒>` 后缀(`a.bmp` → `a-2250.bmp`),一条命令拍一段动画 |
| `VECTORTERM_DUMP_TRAIL=N` | **只在跨窗格尾迹真的被画出来的那一帧**导图,共 N 张(`路径-1.bmp` ...)。按时刻抓帧总会差一帧,这个钩子直接对画了尾迹的帧下手 |
| `VECTORTERM_SETTINGS_FILTER=关键词` | 配置管理器启动时预填搜索词(会话里抢不到前台、键盘注入无效时用) |
| `VECTORTERM_GPU_LIST=1` | 打印枚举到的全部适配器(名称 / 核显独显 / 后端 / 驱动版本) |
| `VECTORTERM_GPU_ADAPTER=<名字子串或序号>` | 只挑指定适配器,如 `NVIDIA`、`Intel`、`WARP`、`0` |
| `VECTORTERM_GPU_POWER=high` | 临时按"优先独显"排序,等价于配置里的 `gpu_power = "high"` |
| `VECTORTERM_GPU_ALLOW_SOFTWARE=1` | 允许 WARP / lavapipe 这类软件适配器(默认排除) |

## 自动发布(GitHub Actions)

仓库自带 [`.github/workflows/release.yml`](../.github/workflows/release.yml):
推一个 `v` 开头的 tag,就在 GitHub 的 Windows runner 上编译、打包、发 Release。

```bash
git tag v0.1.0
git push origin v0.1.0
```

Release 里的资产是 `VectorTerm-0.1.0-setup.exe` 与 `SHA256SUMS.txt`。
单次约 20–40 分钟,时间基本花在 wgpu / eframe 的 release + lto 上。

### 版本号:tag 说了算

- tag 去掉前导 `v` 就是版本号。workflow 会**回写 `Cargo.toml` 的
  `[workspace.package] version`**,让 exe 的资源版本、安装包的 `AppVersion`、
  "应用和功能"里显示的名字三处一致;`Cargo.lock` 由 cargo 自己跟着改。
- 只接受 `x.y.z` 三段纯数字。Inno 的 `VersionInfoVersion` 只吃数字,`v1.0.0-beta`
  会让 ISCC 直接失败。真要做预发布,得先把 `.iss` 里的 `VersionInfoVersion`
  拆成一个独立的数字 define,再放开 workflow 里的校验。

### 先手动跑一次

打 tag 之前,在 Actions 页选 **Build & Release → Run workflow**(分支 `master`)
先跑一遍。手动触发**只编译并上传 artifact,不建 Release** —— 编译或打包有问题
当场就能看到,不会留下一个发不出去的 tag。

### 流水线做了什么

| 步骤 | 内容 |
|---|---|
| 版本号 | 从 tag 或 `Cargo.toml` 取版本、校验格式、回写 `Cargo.toml` |
| 工具链 | `dtolnay/rust-toolchain` 装 stable MSVC;MSVC 环境由 `build.cmd` 自己用 vswhere + `vcvars64.bat` 拉起来 |
| 上游源码 | 跑 `setup.ps1` 浅克隆 wezterm;`.wezterm-src` 按"上游 tag + `patches/*.patch` 的哈希"缓存 |
| Inno Setup | `choco install innosetup`(当前 6.7.1),并在编译前卡 `>= 6.5`(中文 `.isl` 是 6.5 格式) |
| 编译打包 | `build.cmd installer` —— 与本地同一条命令,没有第二套构建逻辑 |
| 校验 | 确认 `setup.exe` 存在且大于 3 MB,生成 `SHA256SUMS.txt` |
| 发布 | `gh release create --generate-notes`;同一个 tag 重跑时改走 `gh release upload --clobber` |

只缓存 cargo registry 与 `.wezterm-src`,**不缓存 `target/`**:release + lto 的
产物好几个 GB,占满 10 GB 缓存额度不如冷编译一次干净。嫌慢可以自己加
`Swatinem/rust-cache`。

流水线**只构建、不跑单元测试**。打 tag 之前先在本地过一遍(64 个测试里有一部分
是渲染与动画时序,需要在真实桌面上跑才稳):

```bat
build.cmd test --release
cargo test -p vectorterm-settings --release
```

要在 CI 里也跑测试,加一个独立 job 就行,别塞进发布路径 —— 测试挂掉把发布卡死
比发出一个有问题的包更让人难受。

### 常见失败点

- **没有写 Release 的权限**。workflow 里已经声明 `permissions: contents: write`,
  但组织/仓库若把默认权限锁成只读,还要在 Settings → Actions → General →
  Workflow permissions 里放开,否则 `gh release create` 会 403。
- **Inno Setup 低于 6.5**。workflow 会在编译前报出版本,不会让你对着 ISCC 的
  `Unknown message name` 猜。
- **`.wezterm-src` 缓存过期**。缓存 key 含上游 tag 与补丁哈希,换版本或改补丁会
  自动重拉;真要强制重建,在 Actions 页删掉对应 cache。
- **tag 不存在**。`gh release create` 带 `--verify-tag`;手动触发那条路径本来就不发布。
- **安装包没有代码签名**。这不是流水线故障,但用户下载后首次运行会看到
  SmartScreen「Windows 已保护你的电脑」。要消掉得买 OV/EV 代码签名证书,在
  `.iss` 的 `[Setup]` 里配 `SignTool`,再把证书放进仓库 Secrets —— 一套下来
  不比这个流水线省事,没做也很正常。
- fork 里的 PR 不会触发(只监听 tag 与手动触发)。

### 本地复现 CI

出错时先本地跑一遍,比翻 Actions 日志快:

```bat
powershell -ExecutionPolicy Bypass -File .\setup.ps1
build.cmd installer
```

本地缺 Inno Setup 时:`choco install innosetup -y` 或
`winget install --id JRSoftware.InnoSetup -e`,`build.cmd` 会自己找到它
(装在非 C 盘也能找到,见上一节的查找顺序)。

## 换一台机器跑不起来?

把 exe + 配置拷到别的电脑上,最容易踩的两个坑,现在都会自动兜住:

- **shell 不存在**。默认配置要的是 `pwsh.exe`(PowerShell 7),没装的机器上 `CreateProcessW` 会直接报"系统找不到指定的文件",整个程序起不来。现在启动前会解析一遍:
  `配置里的 shell` → `pwsh.exe` → `powershell.exe` → `cmd.exe` → `%COMSPEC%`,谁在就用谁,并打印替换原因;换到另一档 shell 时参数按目标 shell 重建(`cmd` 不带 `-NoLogo`)。`pwsh.exe` 还会额外查 Store 别名目录和 `%ProgramFiles%\PowerShell\7`,避免"装完 PS7 但 PATH 还是旧的"被静默降级。
- **工作目录不存在**。配置里的 `cwd` 指向另一台机器的路径时,启动会失败。现在检测到不是目录就打印提示并退回默认目录。
- **GPU 不可用**。不再有任何"没有 DX12 适配器"就退出或黑屏的路径:三个后端都试完还不行,自动切 CPU 软渲染,并在 stderr 说明原因。

排查顺序:先看启动日志第一行 `渲染后端`,再看有没有 `找不到 <shell>` 的提示;要更详细就加 `VECTORTERM_GPU_LIST=1`。

## PowerShell 报 "history file ... is denied"?

启动后提示符旁出现:

```text
Error reading or writing history file 'C:\Users\<你>\AppData\Roaming\Microsoft\Windows\PowerShell\PSReadLine\ConsoleHost_history.txt':
Access to the path '...ConsoleHost_history.txt' is denied.
```

这不是终端的 bug,是**进程完整性级别**的问题。完整性级别沿父子进程**原样继承**——`CreateProcessW` 不带令牌参数(`portable-pty/src/win/psuedocon.rs`),子进程拿的就是调用者的令牌;而低完整性进程**不能写**中等完整性的文件(NTFS 的 no-write-up 规则)。PSReadLine 启动时*读*历史是成功的,失败的是保存那一步,所以文案才写成 "reading or writing"。

低完整性有**两个来源**,要分开查:

**① 启动链上的父进程被降权**(沙箱、AppContainer、"隔离运行"、安全软件的受控运行)。

**② exe 文件自身带 Low 标签** —— 自编译的版本更常撞上这个。Windows 有一条额外规则:**可执行文件的完整性标签低于调用者时,进程按文件的较低标签启动**。所以只要 `VectorTerm.exe` 自己是 Low,不管谁双击、父进程多正常,进程都会是 Low。一条命令判定:

```powershell
icacls target\release\VectorTerm.exe | findstr Mandatory
# Mandatory Label\Low Mandatory Level:(I)(NW)   ← 问题就在这
```

成因:这个项目曾经被**低完整性进程**构建过(沙箱化的工具链 / Agent 沙箱 / AppContainer)。Windows 会给低完整性进程新建的对象自动打上 Low 标签,并沿目录树一路继承,于是 `target\` 下所有产物——包括链接出来的 exe——都是 Low。

修复(把整棵树设回 Medium):

```powershell
icacls "C:\path\to\VectorTerm" /setintegritylevel (OI)(CI)Medium /T /C
```

如果某层目录报 **"拒绝访问"**,说明它的 DACL 里没有你自己的 FullControl(而改 IL 标签要写 SACL 里的 `Mandatory Label`,需要 `WRITE_OWNER`),这时改不动,只能删掉重建——新对象会继承父目录当前的标签:

```powershell
cargo clean
cargo build --release
```

> 注意 `/reset` 修不了这个:`icacls /reset` 只重置 DACL,mandatory label 在 **SACL** 里,不会跟着变。

启动日志第一行已经会打出来:

```text
[VectorTerm] 完整性级别: Medium (S-1-16-8192)  父进程: explorer.exe(33696)
```

- `Medium` = 正常(资源管理器双击就是这一档)。此时若仍报错,是别的原因(文件被别的进程占用、杀软拦截)
- `Low (S-1-16-4096)` = 本进程被降权了,要查的是**谁启动的它**(沙箱、AppContainer、"隔离运行"、部分安全软件的受控运行)
- 也可以随时自己查:`whoami /groups | findstr S-1-16`

治本:从资源管理器 / 开始菜单正常启动。**不要**用"以管理员身份运行"绕过——提权确实能写(HIGH 可以写 MEDIUM),但整棵进程树都变成高完整性,安全面反而更大。

## 为什么"以管理员身份运行"就不报错?

因为**提权后不受文件标签降级**。实测(同一个标成 `Low` 的 exe):

| 谁启动 | exe 文件标签 | 进程实际 IL | 结果 |
|---|---|---|---|
| 普通双击(Medium) | Low | **Low (0x1000)** | 写不了 → 报错 |
| 以管理员运行(High) | Low | **High (0x3000)** | 能写 → 不报错 |

所以提权看起来"治好了"这个问题,但它**只是跳过了降级**,不是修好了根因:整个终端连同 shell、连同你在里面敲的每一条命令都跑在高完整性上,任何一次误操作影响面都更大。而且它掩盖了真实原因——文件标签本来就是坏的。

正解还是把 exe 的标签修回 `Medium`(见上),修好之后普通双击就是 `Medium`,不需要任何提权。

必须在这种环境里用的话,把历史挪到可写位置(`$PROFILE`):

```powershell
Set-PSReadLineOption -HistorySavePath "$env:TEMP\ps_history.txt"
```

注意 `-NoProfile` 挡不住这个报错:PSReadLine 由 console host 自动加载,不经过 profile。

## 为什么不能直接 `cargo build`?

本机是 VS 2026(MSVC 14.51)+ 新版 `cc-rs`(1.11)。`cc-rs` 会清空子进程环境并按自己的探测结果重建,在这套组合下 `cl.exe` 加载前端 `c1.dll` 失败(报 `error D8050`)。`build.cmd` 先执行 `vcvars64.bat` 初始化 MSVC 环境,并设置 `CRATE_CC_NO_DEFAULTS=1` 让 `cc-rs` 沿用继承的 vcvars 环境,从而正常编译。此问题与本项目代码无关,将来 `cc-rs` 修复后可直接用 `cargo build`。

## 验证记录(本机)

本节提到的所有探针 / 审计脚本都放在 **`tools/diagnostics/`**(随仓库提交,不随发行版分发)。
以前它们散落在 `target/` 里,`cargo clean` 一跑就没了;现在统一从项目根目录执行
`python tools/diagnostics/<脚本>.py`,输出截图 / 日志仍然写到 `target/`。
`readme_shot.py` 是 README 头图的导帧脚本:用 `config/deepseek.lua` 主题 + 用户
真实的 PowerShell 7 profile 启动,导出「打开后的第一屏」(fastfetch 立绘与系统信息
+ oh-my-posh 提示符 + 主题状态栏),**不跑演示脚本、不模拟按键**。
`python tools/diagnostics/readme_shot.py` 会覆盖 `assets/screenshot.png`;临时
配置只写 `theme = "deepseek"`,窗口尺寸、shell、字体、状态栏全部继承主题
(1250×750),不覆盖任何布局项。仅排查时可用 `README_SHOT_SIZE` 临时改尺寸。
`cursor_probe.py` 是光标本体的实机回归:把 `colors.cursor_bg` 改成品红,依次
启动默认 / 竖条 / 半格 / 闪烁三种灭相 / 脉冲七组配置,按像素统计光标块的
数量与包围盒,输出 `target/_cursor_shot/grid.png`。判据见下表(字号 16 时
一格约 12×21px):

| 组 | 期望 |
|---|---|
| `default` | 一整格实心品红(12×21) |
| `bar` / `small` | 细竖条(约 3×21) / 半格居中(约 6×11) |
| `hidden` | 0 个品红像素 |
| `outline` | 外框尺寸不变、像素数明显少于实心 |
| `fade` | 尺寸不变但中心像素被背景混暗 |
| `pulse` | 包围盒明显大于一整格 |

- `rustc 1.98.1 (msvc)`,`cargo 1.98.1`
- VS 2026 Community(MSVC 14.51.36231),Windows SDK 10.0.26100(E: 盘)
- `wezterm-term` tag `20240203-110809-5046fc22` 浅克隆 + sparse-checkout 成功
- 392 个依赖 crate 全部编译通过
- debug 构建:`Finished dev profile`,vectorterm 0 error / 0 warning
- release 构建:`Finished release profile [optimized]`,产物 `target/release/VectorTerm.exe`(约 26 MB,含图标与版本资源,以及 13.8 MB 内嵌字体 —— JetBrains Mono 四个 face / Nerd Font Symbols / Noto Color Emoji,见 `assets/fonts/README.md`)
- 冒烟测试(debug 与 release 各一次):运行 8 秒,进程存活,stderr 为空 —— 窗口、字体加载、softbuffer 呈现、ConPTY、解析线程、渲染循环均正常
- 单元测试:`cargo test --release` **152 passed / 0 failed / 15 ignored** —— 覆盖尾迹动画状态机、**鼠标选区(多击计数 / 反向拖拽归一 / 选词与整行边界 / 宽字符与行尾空格 / 软折行拼接)**、**Lua `selection` 配置解析**、**scrollback 回绕后取快照不越界(上游补丁回归)**、**滚动动画(ease-in-out / 连发累加 / 回底不重启 / 亚行快照)**、Lua 键位、SGR 属性快照(斜体/闪烁/上划线/花下划线/删除线)、**256 色与真彩色解析**、**调色板覆盖(ansi/indexed)**、回看偏移、宽字符占格、**PxScale em 归一回归**、**字体索引/家族解析**、**Nerd Font PUA 图标回退**、**字形角色排版(powerline 不越格)**、**`render` 配置解析(auto/cpu/gpu)**、**GPU 与 CPU 逐像素一致**、**Lua 字体表/示例配置解析**、**shell 与参数解析**、**配置优先级(用户 > 软件目录默认)**、**DECCKM→SS3**、**LNM→CRLF**、修饰键 xterm 编码、括号粘贴
- 渲染后端冒烟(debug):默认配置启动打印 `[VectorTerm] 渲染后端:CPU 软渲染`;`render = "gpu"` 启动打印 `[VectorTerm] 渲染后端:GPU(... / Dx12) —— 拖动缩放窗口时旧帧会被 DXGI 拉伸...` —— 两条路径都能正常承载 shell 会话
- 性能基准:`powershell -ExecutionPolicy Bypass -File benchmark\run.ps1` 一键全套,或 `build.cmd test --release bench_ -- --ignored --nocapture`(渲染分阶段 / GPU 提交口径 / 解析吞吐 / 16 MiB 洪泛),**必须在 release 下运行**;真实窗口帧率用 `VECTORTERM_PROFILE_FRAMES=300` 采样。数据与复现步骤见 `benchmark/README.md`
- 滚动条 / 历史搜索的实机回归:`python tools/diagnostics/scrollbar_search_probe.py release`。它启动一个跑 160 行 `NEEDLE` 日志的 cmd,用洋红滑块 + 绿色匹配色做像素判据,依次验证「贴底滑块在最下 → 拖滑块回看 → 拖选到边缘自动上滚 → `Ctrl+Shift+F` 输入查询并高亮 → 拖回贴底后回车跳到匹配行」共 9 项,截图落在 `target/sbsearch_*.png`。探针里注入文本用 `SendInput` 的 `KEYEVENTF_UNICODE`(纯 VK 码在部分键盘布局下会变成 `Named(Process)`、不带 `text`),抓图前必须设 DPI 感知,否则 125% 缩放下位置会偏
- 命令面板 / 窗格大小调整的综合实机回归:`python tools/diagnostics/panel_resize_probe.py release`。用 `VECTORTERM_AUTO_SPLIT=v` 造左右分屏,把分割线配成纯红便于按像素量位置,依次核对「鼠标拖线 → `Ctrl+Shift+Alt+→` 右移一步 → `Ctrl+Shift+Alt+←` 左移一步 → `Ctrl+Shift+P` 命令面板 → 输入 `key` 回车出键位表 → `PgDn` 翻页看后半段 → `Ctrl+Shift+F` 后输 `/` 切回命令面板 → 输入 `files` 回车铺开文件管理器」共 9 项,截图落在 `target/panel_*.png`。**方向键必须用扫描码 + `KEYEVENTF_EXTENDEDKEY` 注入**:`keybd_event(VK_RIGHT, scan=0)` 在 NumLock 打开时会被 Windows 翻译成小键盘 6 并伪造 Shift up/down,keydown 直接丢失,日志里只会剩一条 `Released ... physical=Code(Numpad6)`。另外 `Ctrl+Alt+方向键` 是显卡驱动的旋转屏幕全局热键,keydown 到不了窗口,别拿它当备用组合
- 内置主题的实机回归:`python tools/diagnostics/theme_probe.py release`。先把 `config/deepseek.lua` 同步到 `target/release/config/`(和 `build.cmd` 第 5b 段同一个位置),再把探针配置放进临时目录并让子进程 `cwd` 也指向那里 —— 于是 `theme = "deepseek"` 只能靠 `<exe 目录>/config/deepseek.lua` 命中,顺带验证主题确实随发布版本分发。用 `VECTORTERM_DUMP_FRAME` 各导一帧:A 帧核对夜空蓝画布 `#0B1220`、顶栏实心品牌蓝活动标签 `#4D6BFE`、品牌蓝光标、没有退回默认底色 `#0c0c0c` 或上一版近黑底色 `#0E0E10`、右侧无滚动条;B 帧在用户配置里显式把滚动条开成洋红常显,确认右侧真的出现滚动条(反过来证明 A 帧是主题关掉的)。判据有两个坑:旧画布 `#0E0E10` 与默认底色 `#0c0c0c` 只差 2~4,颜色容差必须收紧到 1(新的 `#0B1220` 蓝通道差得多,能明确区分);活动标签要**只数窗口顶部 60px 一带**的蓝像素,否则光标那一小块蓝会蒙混过关;滚动条带 `opacity`(滑块 0.85 / 轨道再乘 0.45),`#ff00ff` 会被混成 `(205,3,205)` / `(106,9,107)`,要用「红蓝高、绿低」判色调而不是精确取色。截图落在 `target/theme_*.png`
- 内嵌主题的实机回归:`python tools/diagnostics/embed_probe.py release`。验证"`config\` 目录整个不存在也能用 `theme = \"chatgpt\"`":把 `VectorTerm.exe` **单独**拷进一个空目录、子进程 `cwd` 再指向另一个空目录,两处都没有 `config/`,配置只写一行 `return { theme = "chatgpt" }`。判据:画布命中 `#212121` > 1000 像素、顶栏 60px 内白底活动标签 `#ECECEC` > 200、默认底色 `#0C0C0C` 残留 < 50 —— 任一条不满足就是回退失败(退回了内置默认配置)。另外记得把 `VECTORTERM_DUMP_AT` 设到 2.5s:状态栏靠定时器派发 `update-right-status`,第一帧拍出来是空的,早拍会误判成"主题脚本没执行"。截图落在 `target/embed_*.bmp`
- 另有开发辅助:`cargo test dump_frame -- --ignored --nocapture` 可把一帧画面(中英混排/256色/真彩色)导出为 `target/vectorterm-frame.bmp` 供目测核对;`dump_scroll_frame` 导出亚行滚动画面(`target/vectorterm-scroll.bmp`)核对接缝;`dump_gpu_frame` 用 GPU 后端离屏渲染同一帧(`target/vectorterm-gpu.bmp`),`dump_omp_frame` 复现 Oh My Posh 风格的 powerline/图标密集行(`target/vectorterm-omp.bmp`)
- 状态栏排版量测:`python tools/diagnostics/status_probe.py "☾ 晚上好|晚上好|97%"`。把 `config/deepseek.lua` 复制一份、在 `update-right-status` 开头塞一段固定测试文字(用 `|` 分段),导一帧后按列扫标签栏的墨迹,打印每段的包围盒并另存一张放大图。状态栏是终端里**唯一不按网格排**的文字(按 shaper 自然字距推进),字高、基线、空格收缩差几像素肉眼看不出来,只能这样量 —— 「晚上好只显示晚上」「电池比 97% 高一截」这两个都是用它定位的
- 浮层进出场动画的逐帧连拍:`python tools/diagnostics/overlay_probe.py search`(或 `commands`)。靠两个诊断钩子驱动: `VECTORTERM_AUTO_PANEL=search|commands[@毫秒]` 到点打开、`VECTORTERM_AUTO_PANEL_CLOSE=毫秒` 到点关闭;`VECTORTERM_DUMP_AT` 给一串时刻(支持 `auto+28` / `close+40` 这种相对写法,不用手算绝对毫秒),程序在那些毫秒各导一帧,脚本裁到浮层所在区域横向拼成连拍图。判据:进场 190ms 里框宽单调增大(实测 620 → 684)、y 单调上移(684 → 657),退场**每一帧都要比上一帧更小** —— 早先退场用 ease-in 时前三分之二几乎不动、最后一帧"啪"地消失,正是这张图看出来的。另外这两个钩子的到期时刻会被 `about_to_wait` 提前唤醒,否则默认 400ms 的空闲轮询会让动画晚 0~400ms 才开始,时间轴全对不上
- 文件面板(侧边栏)横向滑入 / 滑出的逐帧连拍:`python tools/diagnostics/files_slide_probe.py side --close-at 2000`(`full` 是铺满 pane 的文件管理器)。钩子:`VECTORTERM_AUTO_FILES=side|full[@毫秒]` 到点打开、`VECTORTERM_AUTO_FILES_CLOSE=毫秒` 到点收起,同样走提前唤醒。判据:落定帧里面板右边那条竖线在 x=343(`files_gui.side_cols * cell_w`),进场每帧右边缘**单调右移**(实测 31 → 106 → 207 → 279 → 340),退场**单调左移**到 0。量测必须认那条竖线的固定色 (50,50,54),不能量"墨迹最右列" —— 面板底色和终端底色同色,退场时壁纸一露,最亮列全变,数会乱跳。采点间隔别小于 20ms:一次重绘约 25ms,更密的时刻会落在同一帧上,导出来的图一模一样,看着像动画卡住了
- 中文输入法的实机回归:`python tools/diagnostics/ime_shortcut_probe.py release`。`VECTORTERM_IME_OPEN=1` 让程序自己切到 0804 并打开输入法(自动化没法去点任务栏的语言栏),然后全程按真实窗口送键,四项判据:① `Ctrl+Shift+E` 之后日志里要有「VK_PROCESSKEY 还原 vk=0x45」+ `logical=Character("E")`,同时右侧出现文件面板(墨迹 > 200);② 开搜索框后终端光标让位(提示符一带亮像素峰值,前后差 = 一整格);③ `n` `i` 空格上屏的「你」和随后输入的「，」都走 `ime commit ... -> search` 而不是 `-> pty`;④ `Esc` 关掉之后光标回来。**注入必须用 `KEYEVENTF_SCANCODE`**:用 `wVk` 注入的 `lParam` 扫描码是 0,而复现这个 bug 的关键恰恰是「wParam 被换成 0xE5、扫描码还在」——用 VK 注入会得到一条完全正常的消息,什么都测不出来。`VECTORTERM_IME_OPEN` 要反复生效(窗口拿焦点时 winit 会重建输入上下文,把打开状态清掉),所以它在 `about_to_wait` 里每次都读一遍真实状态。截图落在 `target/ime_probe/ime_*.png`
- 关窗口确认的实机回归:`python tools/diagnostics/window_close_probe.py release`。三个场景:① 跑着 `ping -t` 时发 `WM_CLOSE`(等价点系统 ✕ / `Alt+F4`)→ 应弹框且窗口不关;② 只剩一个标签时走 `VECTORTERM_AUTO_TAB=close@3000`(等价点标签栏那个 ×)→ 同样弹框,文案是 "this **window** and all contained **tabs**" 那句;③ 空 shell → `WM_CLOSE` 直接关、不弹。第 ②③ 两条正是"跑了 exe,关窗口却一声不响"的两个入口:`WindowButton::Close` 以前直接 `request_quit()`,而单标签时 `ask_close_tab` 一上来就按 `background.len() <= 1` 放行。判据同上用 `mid_ink`(实测有框 1186、空框 0);帧的路径要注意——`VECTORTERM_DUMP_AT` 只给**一个**时刻时不加 `-<毫秒>` 后缀,给一串才加,脚本两种都得试
- 关标签确认的实机回归:`python tools/diagnostics/confirm_probe.py release`。用 `VECTORTERM_AUTO_TAB=new@800,close@3000,no@3500,close@4000,yes@4500` 造出「问 → 答否 → 再问 → 答是」四步,`VECTORTERM_DUMP_AT` 各拍一帧,截图落在 `target/confirm_probe/`。判据用 `mid_ink`(内容区 y 35%~66%、x 4%~96% 里的非底色像素数):确认框**居中**,墨迹几百~上千;终端输出靠顶,这一段接近 0 —— 早先用"纯色占比"完全不灵,终端本来就 96% 是底色。核对 3500ms 有框且文案是 `Really kill this tab...`、4600ms 框消失且标签与 `ping -t` 都还在、按 yes 后只剩一个标签。**截图别用 `PrintWindow`**:窗口被别的窗口压着时拍到的是遮挡物,只能靠 `VECTORTERM_DUMP_FRAME` 让程序自导出帧
- 内嵌字体与彩色 emoji 的实机回归:`python tools/diagnostics/font_probe.py release`。故意用一份**不写 `font` 表**的配置,让默认值完全生效;`VECTORTERM_AUTOTYPE` 先 `chcp 65001` 再打印 ASCII / 中文 / 五个 emoji,`VECTORTERM_DUMP_FRAME` 自导出一帧。判据:内容区墨迹 > 500(主字体与中文回退都加载成功)、非灰彩色像素 > 60(说明 CBDT 取出的 PNG 真的进了显示列表,而不是只解码了没画)。截图落在 `target/font_probe/`。注意 CBDT 字体没有 glyf 轮廓,不能塞进普通文本回退链——`src/emoji.rs` 单独按 CBLC 索引取 PNG,缩到当前字号后走 `Frame::image`,CPU/GPU 两端共用同一张 RGBA 位图
- 关窗格(`Ctrl+Shift+X`)的实机回归:`python tools/diagnostics/pane_close_probe.py release`。四个场景:① 空 shell 分屏后关窗格 → 不弹、直接关;② 分屏后在**新窗格**里跑 `ping -t` 再关 → 弹 `● Really kill this pane?`,按 `n` 后窗格和分隔线都还在;③ 同上按 `y` → 窗格消失、窗口还在;④ 只有一个窗格时关 → 等同关标签,单标签时走"关窗口"那句文案。判据:分隔线默认色 `#323236`(竖线近全高,实测 296 个采样点),关掉后归零;确认框**会清屏**,弹框那一帧分隔线也被盖掉,所以"线还在不在"要看按 `n` 之后那一帧。场景走 `VECTORTERM_AUTO_TAB=closepane@…,yes/no@…`,和 `Ctrl+Shift+X` 是同一个内部入口(`close_pane_or_ask`)
- `--cwd` / 右键菜单的实机回归:`python tools/diagnostics/cwd_arg_probe.py`。资源管理器右键注册的命令就是 `VectorTerm.exe --cwd "<文件夹>"`,探针按同样命令行拉窗口,核对三种情况:① 带空格 + 中文的路径;② 盘根 `C:\`(命令行里会被解析成 `C:"`,靠 `tidy_cwd_arg` 补回来);③ 不存在的目录(应回落用户目录且窗口照常起来)。判据**不用** `VECTORTERM_AUTOTYPE` —— 那个钩子 2.5s 后才敲字节、且依赖窗口事件循环,无人值守时可能压根不触发(排查时一度被误判成"`--cwd` 不生效")。改成让 cmd 用 `shell_args = { "/k", "cd> <文件>" }` 在启动瞬间自己把 cwd 写进文件,与 UI 时序无关;证明文件放在**无空格**的临时目录里,因为 `/k` 后面那串是 cmd 自己重新解析的,路径带空格就要嵌套引号、极易解析错。另外往 Lua 配置里拼路径时反斜杠必须成对(`\U` 这类非法转义会让整份配置静默失效,窗口回落到用户默认配置)
- 配置文档一致性审计:`python tools/diagnostics/audit_config_docs.py`。把 `settings-gui/src/schema.rs` 的 155 个字段逐一对照手册 [§19 全量模板](config-manual.md#19-全量配置模板) / [§22 默认值总表](config-manual.md#22-默认值总表)与 `config.example.lua`,再抽查 `src/config.rs` 真正解析的键、已移除功能留下的过时说法;任一处漏项就退出 1。加新配置项时先跑它,再跑 `python tools/diagnostics/check_md_links.py`(手册锚点)与 `python tools/diagnostics/check_tables.py`(表格列数),避免代码 / GUI / 手册三边漂移(这些脚本都随仓库提交在 `tools/diagnostics/`,不随发行版分发)
