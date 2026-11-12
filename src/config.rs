//! Lua 配置(mlua + Lua 5.4 vendored)

use mlua::{Lua, Value};
use std::path::{Path, PathBuf};

/// 光标形状覆盖。`Auto` 跟随终端的 DECSCUSR(`Steady*`/`Blinking*`),
/// 其余三个会无视应用发来的序列强制形状。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShapeOverride {
    Auto,
    Block,
    Bar,
    Underline,
}

impl CursorShapeOverride {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "default" => Some(Self::Auto),
            "block" => Some(Self::Block),
            "bar" | "beam" | "ibeam" => Some(Self::Bar),
            "underline" | "line" => Some(Self::Underline),
            _ => None,
        }
    }
}

/// 光标闪烁开关。`Auto` = 只有终端声明 `Blinking*` 形状时才闪。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorBlinkMode {
    Auto,
    Always,
    Never,
}

impl CursorBlinkMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "true" | "on" | "always" => Some(Self::Always),
            "false" | "off" | "never" => Some(Self::Never),
            _ => None,
        }
    }
}

/// 闪烁「灭」的那一相怎么表现。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorBlinkStyle {
    /// 整块消失(传统终端行为)
    Hidden,
    /// 淡出到接近透明,再淡回来
    Fade,
    /// 只留一圈描边,像空心块
    Outline,
}

impl CursorBlinkStyle {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "hidden" | "hide" => Some(Self::Hidden),
            "fade" | "dim" => Some(Self::Fade),
            "outline" | "hollow" => Some(Self::Outline),
            _ => None,
        }
    }
}

/// 光标本体的常驻动画(不影响位置,和尾迹叠加)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorAnimation {
    None,
    /// 尺寸脉冲:宽度/高度按正弦在 1±amount 之间缩放
    Pulse,
    /// 呼吸:不透明度按正弦在 1 与 1-amount 之间起伏
    Breathe,
}

impl CursorAnimation {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "off" | "false" => Some(Self::None),
            "pulse" | "scale" => Some(Self::Pulse),
            "breathe" | "breath" | "fade" => Some(Self::Breathe),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CursorConfig {
    /// 光标静止多少毫秒后尾迹才开始跟随(kitty `cursor_trail`;0 = 关闭)
    pub trail_dwell_ms: f32,
    /// 每轴最小位移(格):两轴都小于阈值时直接吸附,无尾迹(kitty `cursor_trail_start_threshold`)
    pub trail_start_x: i64,
    pub trail_start_y: i64,
    /// 最快衰减时间(秒,kitty `cursor_trail_decay` 的第一个值)
    pub trail_decay_fast: f32,
    /// 最慢衰减时间(秒,kitty `cursor_trail_decay` 的第二个值)
    pub trail_decay_slow: f32,
    /// 单段尾迹动画的硬上限(毫秒;0 = 不限制)。
    ///
    /// 连续输入时不排队:目标始终只有一个(最新位置),动画到点直接吸附,
    /// 保证再密的输入也不会攒出一串要逐个"播完"的旧动画。
    pub trail_max_ms: f32,
    /// 尾迹颜色;None = 跟随光标颜色(kitty `cursor_trail_color` = none)
    pub trail_color: Option<String>,

    // ── 光标本体 ────────────────────────────────────────────────
    /// 形状覆盖;Auto = 跟随终端 DECSCUSR
    pub shape: CursorShapeOverride,
    /// 宽 / 高,格子的倍数(1.0 = 正好一格)
    pub width: f32,
    pub height: f32,
    /// 缩小或放大后在格子里的锚点,0 = 左/上,0.5 = 居中,1 = 右/下。
    /// None = 按形状的默认位置(block/bar 左上,underline 贴底)。
    pub align_x: Option<f32>,
    pub align_y: Option<f32>,
    /// bar 的宽 / underline 的高(物理像素);0 = 按格子尺寸自动算
    pub thickness: f32,
    /// 光标本体不透明度(0..1)
    pub alpha: f32,
    /// 闪烁开关
    pub blink: CursorBlinkMode,
    /// 一个完整闪烁周期的毫秒数
    pub blink_rate_ms: f32,
    /// 亮相占周期的比例
    pub blink_duty: f32,
    /// 灭相的表现
    pub blink_style: CursorBlinkStyle,
    /// 常驻动画
    pub anim: CursorAnimation,
    /// 动画周期(毫秒)
    pub anim_ms: f32,
    /// 动画幅度(0..1)
    pub anim_amount: f32,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            trail_dwell_ms: 20.0,
            trail_start_x: 0,
            trail_start_y: 0,
            trail_decay_fast: 0.1,
            trail_decay_slow: 0.4,
            trail_max_ms: 220.0,
            trail_color: None,
            // 以下全部保持「旧版行为」:整格方块、不闪、不动。
            shape: CursorShapeOverride::Auto,
            width: 1.0,
            height: 1.0,
            align_x: None,
            align_y: None,
            thickness: 0.0,
            alpha: 1.0,
            blink: CursorBlinkMode::Auto,
            blink_rate_ms: 1060.0,
            blink_duty: 0.5,
            blink_style: CursorBlinkStyle::Hidden,
            anim: CursorAnimation::None,
            anim_ms: 1200.0,
            anim_amount: 0.25,
        }
    }
}

/// 回看滚动动画
#[derive(Debug, Clone)]
pub struct ScrollConfig {
    /// 一个滚轮档位的平滑动画时长(毫秒);0 = 关闭动画(逐行瞬移)
    pub anim_ms: f32,
    /// 单段滚动动画的硬上限(毫秒):连续滚轮再密也不会攒出很长的动画
    pub max_ms: f32,
    /// 按键/粘贴后回到底部的时长(毫秒)
    pub back_ms: f32,
    /// 长按连续输入(回车连发)时回到底部的时长(毫秒):
    /// 明显短于 `back_ms`,让视口赶在下一批输出之前就位,剩下的交给光标尾迹
    pub back_burst_ms: f32,
}

impl Default for ScrollConfig {
    fn default() -> Self {
        Self {
            anim_ms: 90.0,
            max_ms: 200.0,
            back_ms: 90.0,
            back_burst_ms: 40.0,
        }
    }
}

/// 滚动条什么时候出现。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollbarShow {
    /// 一直画轨道和滑块
    Always,
    /// 贴底且鼠标不在轨道上时不画(默认)
    Auto,
    /// 完全不画
    Never,
}

impl ScrollbarShow {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "always" | "show" | "true" => Some(Self::Always),
            "auto" | "hover" => Some(Self::Auto),
            "never" | "hide" | "off" | "false" => Some(Self::Never),
            _ => None,
        }
    }
}

/// 滚动条停靠在 pane 哪一侧。分屏时把左 pane 的条挪到左边,就不会贴着分割线。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollbarSide {
    Right,
    Left,
}

impl ScrollbarSide {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "left" | "l" => Some(Self::Left),
            "right" | "r" => Some(Self::Right),
            _ => None,
        }
    }
}

/// 右侧可视化滚动条。轨道 / 滑块都画在 pane 内容区边缘内侧,
/// 不改终端网格(不占列),所以不影响 PTY 尺寸与文字折行。
#[derive(Debug, Clone, PartialEq)]
pub struct ScrollbarConfig {
    /// 是否启用(`false` 等价于 `show = "never"`)
    pub enabled: bool,
    /// 显示策略。`auto_hide` 是它的向后兼容别名
    pub show: ScrollbarShow,
    /// 停靠侧
    pub side: ScrollbarSide,
    /// 是否画轨道。关掉后只剩滑块,分割线旁边不会再有一条长灰带
    pub show_track: bool,
    /// 轨道宽度(px)
    pub width: f64,
    /// 滑块最小高度(px):历史很长时不至于细到抓不住
    pub min_thumb: f64,
    /// 贴底且鼠标不在轨道上时不画(拖动/回看时才出现)
    pub auto_hide: bool,
    /// 整体不透明度
    pub opacity: f32,
    /// 轨道颜色。None = 由前景/背景色自动混合
    pub track_color: Option<String>,
    /// 滑块颜色。None = 由前景/背景色自动混合
    pub thumb_color: Option<String>,
    /// 鼠标悬停 / 拖动滑块时的颜色。None = 沿用 `thumb_color`
    pub hover_color: Option<String>,
}

impl Default for ScrollbarConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            // 默认自动隐藏:贴底时那条常显轨道会贴着分割线画出一条 ~9px 的
            // 灰带,视觉上又把「文字到分割线的距离」拉大。回看 / 悬停时照常显示。
            show: ScrollbarShow::Auto,
            side: ScrollbarSide::Right,
            show_track: true,
            width: 9.0,
            min_thumb: 28.0,
            auto_hide: true,
            opacity: 0.85,
            track_color: None,
            thumb_color: None,
            hover_color: None,
        }
    }
}

/// 历史搜索(`Ctrl+Shift+F`)。
#[derive(Debug, Clone, PartialEq)]
pub struct SearchConfig {
    /// 是否启用搜索栏
    pub enabled: bool,
    /// 默认区分大小写(搜索栏里按 Tab 可临时切换)
    pub case_sensitive: bool,
    /// 匹配高亮色
    pub highlight_color: String,
    /// 当前匹配的高亮色
    pub current_color: String,
    /// 一次搜索最多收集多少处匹配(防止洪泛日志撑爆内存)
    pub max_matches: usize,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            case_sensitive: false,
            highlight_color: "#f0c674".to_string(),
            current_color: "#ff8f3f".to_string(),
            max_matches: 5000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ColorsConfig {
    pub foreground: String,
    pub background: String,
    pub cursor_fg: String,
    pub cursor_bg: String,
    /// 覆盖 16 个基础 ANSI 色(顺序:black red green yellow blue magenta cyan white,
    /// 再紧跟各自的亮色版本)。少于 16 项时只覆盖前面的。
    pub ansi: Vec<String>,
    /// 覆盖任意 0-255 索引色(16-231 是 216 色立方,232-255 是灰阶)。
    /// SGR 38;5;n / 48;5;n 用的就是这张表。
    pub indexed: Vec<(u8, String)>,
}

/// 背景图的缩放/填充方式。
///
/// wezterm 只有"等比铺满并裁切"一种行为;这里额外给几个常见选择,
/// 因为背景图的分辨率和窗口比例往往对不上。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageFill {
    /// 等比放大到完全覆盖窗口,超出部分居中裁掉(默认,等同 wezterm)
    #[default]
    Cover,
    /// 等比缩到整张图都可见,四周露出底色
    Contain,
    /// 拉伸铺满,不保持宽高比
    Stretch,
    /// 按原始像素尺寸平铺
    Tile,
    /// 原始像素尺寸居中,不缩放也不平铺
    Center,
}

impl ImageFill {
    pub fn as_str(self) -> &'static str {
        match self {
            ImageFill::Cover => "cover",
            ImageFill::Contain => "contain",
            ImageFill::Stretch => "stretch",
            ImageFill::Tile => "tile",
            ImageFill::Center => "center",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "cover" | "fill" => Some(ImageFill::Cover),
            "contain" | "fit" => Some(ImageFill::Contain),
            "stretch" => Some(ImageFill::Stretch),
            "tile" | "repeat" => Some(ImageFill::Tile),
            "center" | "none" => Some(ImageFill::Center),
            _ => None,
        }
    }
}

/// Win11 的系统背板材质(毛玻璃)。
///
/// 由 DWM 在窗口后方绘制,所以要求窗口本身输出半透明像素,否则会被
/// 我们画的不透明背景盖住。CPU 后端不支持(softbuffer 走 GDI BitBlt,
/// 没有 alpha 通道),配置了会自动降级并提示。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backdrop {
    /// 不启用,背景完全由自己绘制
    #[default]
    None,
    /// Mica:对桌面壁纸采样做模糊,最省电,但只反映壁纸不反映窗口后方
    Mica,
    /// Acrylic:亚克力,实时模糊窗口后方的内容
    Acrylic,
    /// Tabbed:Win11 22H2 的标签页材质
    Tabbed,
}

impl Backdrop {
    pub fn as_str(self) -> &'static str {
        match self {
            Backdrop::None => "none",
            Backdrop::Mica => "mica",
            Backdrop::Acrylic => "acrylic",
            Backdrop::Tabbed => "tabbed",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "disable" | "off" | "" => Some(Backdrop::None),
            "mica" | "mainwindow" => Some(Backdrop::Mica),
            "acrylic" | "transientwindow" | "blur" => Some(Backdrop::Acrylic),
            "tabbed" => Some(Backdrop::Tabbed),
            // wezterm 的 "Auto" 在 Win11 上是 Mica,老系统上无效果
            "auto" => Some(Backdrop::Mica),
            _ => None,
        }
    }
}

/// 背景渐变。图片存在时渐变画在图片下面(图片半透明时才会透出来)。
#[derive(Debug, Clone, PartialEq)]
pub struct Gradient {
    pub start: String,
    pub end: String,
    /// true = 从上到下,false = 从左到右
    pub vertical: bool,
}

/// 分割线渐变的方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DividerGradientDir {
    /// 沿分割线本身:竖线从上到下,横线从左到右(默认)
    Auto,
    Vertical,
    Horizontal,
}

impl DividerGradientDir {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "along" | "" => Some(Self::Auto),
            "vertical" | "v" => Some(Self::Vertical),
            "horizontal" | "h" => Some(Self::Horizontal),
            _ => None,
        }
    }

    /// 在一条 `vertical` 走向的分割线上,这次渐变是否沿 Y 轴推进。
    pub fn along_y(self, line_vertical: bool) -> bool {
        match self {
            Self::Auto => line_vertical,
            Self::Vertical => true,
            Self::Horizontal => false,
        }
    }
}

/// 分割线渐变:任意多个颜色停靠点,沿线均匀插值。
#[derive(Debug, Clone, PartialEq)]
pub struct DividerGradient {
    pub colors: Vec<String>,
    pub orientation: DividerGradientDir,
}

/// 分屏中间那根分割线的外观。
///
/// 布局上两个 pane 之间永远留 1 个字符宽的缝(终端格子要对齐),这里只决定
/// **这根线画成什么样**:在缝里居中画多粗、两端各自留多长空白、什么颜色。
/// 不配置时保持旧行为 —— 整条缝填满深灰色。
#[derive(Debug, Clone, PartialEq)]
pub struct SplitDividerConfig {
    /// false = 完全不画线(缝仍在,只是露出背景)
    pub enabled: bool,
    /// 线宽(px)。`None` = 填满整格缝;`Some(0)` = 不画线;大于格宽会盖住两边文字
    pub thickness: Option<f64>,
    /// 起点端到内容区边界的距离(px):竖线是上边,横线是左边
    pub inset_start: f64,
    /// 终点端到内容区边界的距离(px):竖线是下边,横线是右边
    pub inset_end: f64,
    /// 单色(HEX / wezterm 颜色名)。配了 `gradient` 时此项只作为解析失败的兜底
    pub color: String,
    /// 渐变色。存在且颜色合法时覆盖 `color`
    pub gradient: Option<DividerGradient>,
    /// 两个 pane 文字区之间的缝宽(px)。`None` = Auto(按字符宽高自适应)。
    pub gap: Option<f64>,
    /// 文字 / 光标到分割线**边缘**的距离(px)。设置后覆盖 `gap`:
    /// `gap = 2 * text_gap + 线宽`。比 `gap` 更符合直觉。
    pub text_gap: Option<f64>,
    /// 不透明度 0..=1
    pub opacity: f32,
    /// 允许用鼠标拖动分割线调整两侧窗格大小
    pub draggable: bool,
    /// 分割线鼠标热区的半宽(px)。线本身只有几像素,热区要宽一点才好抓
    pub hit_width: f64,
    /// 单侧窗格最小占比(0..0.45),拖动和快捷键都夹在这个范围内
    pub min_ratio: f64,
    /// 快捷键每按一次移动多少占比
    pub step: f64,
}

impl Default for SplitDividerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            thickness: None,
            inset_start: 0.0,
            inset_end: 0.0,
            color: "#323236".to_string(),
            gradient: None,
            gap: None,
            text_gap: None,
            opacity: 1.0,
            draggable: true,
            hit_width: 8.0,
            min_ratio: 0.08,
            step: 0.05,
        }
    }
}

impl SplitDividerConfig {
    /// 两个留白是否把线压没了(只用于提前退出,不产生负尺寸矩形)。
    pub fn is_visible(&self) -> bool {
        self.enabled
            && self.opacity > 0.0
            && !matches!(self.thickness, Some(t) if t <= 0.0)
    }
}

/// 文件浏览器里回车打开文件时,编辑器跑在哪里。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOpenMode {
    /// 新开一个 VectorTerm 窗口跑编辑器(默认)。
    ///
    /// 新窗口是独立进程、独立 PTY:不和当前窗口抢文字区,关了它也不影响
    /// 原来的终端;分屏 / 全屏列表都照旧可以继续挑下一个文件。
    Window,
    /// 在宿主 pane 里向右分出一个新 pane 跑编辑器。
    Split,
    /// 把编辑器命令写进宿主 pane 的 shell,和你自己敲命令一样。
    Current,
}

impl FileOpenMode {
    /// 配置文件里写的名字(写回 / 提示用)。
    pub fn as_str(self) -> &'static str {
        match self {
            FileOpenMode::Window => "window",
            FileOpenMode::Split => "split",
            FileOpenMode::Current => "current",
        }
    }

    /// 解析配置里的字符串;认不出来时返回 `None`(调用方保留默认值)。
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        Some(match s.as_str() {
            // 新窗口
            "window" | "new-window" | "new_window" | "newwindow" | "new" | "窗口" => {
                FileOpenMode::Window
            }
            // 分屏
            "split" | "pane" | "new-pane" | "new_pane" | "splits" | "分屏" => FileOpenMode::Split,
            // 当前终端
            "current" | "same" | "inline" | "shell" | "terminal" | "当前" => FileOpenMode::Current,
            _ => return None,
        })
    }
}

/// 文件浏览器(命令面板 `/files` 铺满宿主 pane / `Ctrl+Shift+E` 侧边栏)。
///
/// 面板本身是 VectorTerm 自绘的,不依赖外部程序;`editor` 只决定"回车打开文件"
/// 时用谁去编辑,`open_mode` 决定编辑器跑在哪儿(默认新开一个窗口)。
#[derive(Debug, Clone, PartialEq)]
pub struct FilesGuiConfig {
    /// 编辑器命令:第一个是程序,其余是固定参数(文件名会追加在末尾)。
    /// 空 = 自动探测(`nvim` → `vim` → 记事本)。
    pub editor: Vec<String>,
    /// 编辑器跑在哪儿:新窗口(默认)/ 分屏 / 当前终端的 shell。
    pub open_mode: FileOpenMode,
    /// 侧边栏宽度(列)。太窄放不下尺寸列,只显示名字。
    pub side_cols: usize,
    /// 面板打开时是否显示隐藏文件(点开头 / 带 Windows 隐藏属性)。
    pub show_hidden: bool,
    /// 面板跟着终端走:shell 里 `cd` 之后面板自动切到那个目录
    /// (PowerShell 靠注入的 prompt 钩子上报 OSC 7,cmd 靠读进程 PEB)。
    pub follow_cwd: bool,
    /// 反过来:面板里进目录 / 回上级时,把 `cd` 写进所属终端的 shell,
    /// 让终端也跟着换目录。
    pub push_cwd: bool,
}

impl Default for FilesGuiConfig {
    fn default() -> Self {
        Self {
            // 空 = 自动:装了 nvim 用 nvim,只有 vim 用 vim,都没有就用记事本
            editor: Vec::new(),
            // 默认单独开一个窗口:不动当前窗口的布局,也不污染它的 shell
            open_mode: FileOpenMode::Window,
            side_cols: crate::files::DEFAULT_SIDE_COLS,
            show_hidden: false,
            // 双向同步默认开着:面板和终端"看的是同一个目录"才符合直觉
            follow_cwd: true,
            push_cwd: true,
        }
    }
}

/// 切换焦点时,飞行中的输入光标最终落在哪里。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaretLanding {
    /// 落到新 pane 真实的输入光标位置(默认):动画结束时光标不跳变
    Caret,
    /// 落到新 pane 的几何中心:只看"飞进这个窗口"的效果
    Center,
}

/// 分屏焦点:哪个 pane 显示输入光标,以及切换焦点时光标怎么过去。
///
/// 焦点不靠描边表达:**只有聚焦的 pane 画输入光标**,其余 pane 的 caret
/// 一律隐藏;切换时这唯一的输入光标从旧 pane 的位置飞到新 pane。
#[derive(Debug, Clone, PartialEq)]
pub struct PaneFocusConfig {
    /// 左键点进某个 pane 时把焦点切过去(同时这次点击仍照常开始选区)
    pub click_to_focus: bool,
    /// 非聚焦 pane 隐藏输入光标(及其 kitty 光标尾迹)
    pub hide_inactive_caret: bool,
    /// 切换焦点时播放输入光标的飞行动画
    pub trail: bool,
    /// 飞行动画的**速度曲线、尾迹颜色、不透明度**全部沿用普通光标尾迹
    /// (`cursor.trail_decay_*` / `cursor.trail_color`)。
    ///
    /// 打开时 `trail_color` 被忽略 —— 跨窗格飞行与窗格内 kitty 尾迹是同一份
    /// 参数;关掉才用 `pane_focus.trail_color` 单独指定。
    pub sync_trail: bool,
    /// 飞行动画时长(ms),0 = 关闭
    pub trail_ms: f64,
    /// 飞行光标/拖影颜色。`None` = 跟随终端光标色
    pub trail_color: Option<String>,
    /// 飞行终点
    pub landing: CaretLanding,
    /// `Alt+1..9` / `Ctrl+Shift+Alt+1..9` 直接聚焦第 N 个 pane
    pub number_shortcuts: bool,
}

impl Default for PaneFocusConfig {
    fn default() -> Self {
        Self {
            click_to_focus: true,
            hide_inactive_caret: true,
            trail: true,
            sync_trail: true,
            trail_ms: 320.0,
            trail_color: None,
            landing: CaretLanding::Caret,
            number_shortcuts: true,
        }
    }
}

/// 自定义背景。
///
/// 字段名尽量对齐 wezterm,方便直接搬配置:
/// `window_background_image` / `window_background_image_hsb` /
/// `window_background_opacity` / `win32_system_backdrop`。
#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundConfig {
    /// 纯色背景,覆盖 `colors.background`
    pub color: Option<String>,
    /// 背景图路径。相对路径按配置文件所在目录解析
    pub image: Option<PathBuf>,
    /// 亮度乘数(1.0 = 原样,0.5 = 压暗一半)。对应 hsb.brightness
    pub brightness: f32,
    /// 色相乘数/偏移(1.0 = 不变)。对应 hsb.hue
    pub hue: f32,
    /// 饱和度乘数(1.0 = 原样,0 = 灰度)。对应 hsb.saturation
    pub saturation: f32,
    pub fill: ImageFill,
    /// 模糊半径(相对图片原始像素)。0 = 不模糊;拖到 8~24 是毛玻璃感
    pub blur: f32,
    /// 背景不透明度 0..=1。`None` = 未设置(不透明;开背板时默认全透明)
    pub opacity: Option<f32>,
    pub backdrop: Backdrop,
    pub gradient: Option<Gradient>,
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            color: None,
            image: None,
            brightness: 1.0,
            hue: 1.0,
            saturation: 1.0,
            fill: ImageFill::Cover,
            blur: 0.0,
            opacity: None,
            backdrop: Backdrop::None,
            gradient: None,
        }
    }
}

impl BackgroundConfig {
    /// 实际生效的背景不透明度。
    ///
    /// 开了系统背板却没显式设 `opacity` 时返回 0.0 —— 否则自己画的不透明
    /// 背景会把 DWM 的毛玻璃整个盖住,配了等于没配。
    pub fn effective_opacity(&self) -> f32 {
        self.opacity.unwrap_or(1.0).clamp(0.0, 1.0)
    }

    /// 是否需要把整个窗口做半透明(层叠窗口统一 alpha)
    pub fn needs_transparent_window(&self) -> bool {
        self.effective_opacity() < 0.999
    }

    /// 什么都不配:纯不透明底色,走原来的快速路径
    pub fn is_plain(&self) -> bool {
        self.image.is_none()
            && self.gradient.is_none()
            && self.color.is_none()
            && self.backdrop == Backdrop::None
            && self.effective_opacity() >= 0.999
    }
}

/// 渲染后端
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    /// 优先 GPU,失败自动退回 CPU
    Auto,
    /// 纯软件光栅化(softbuffer)。**默认值**。
    ///
    /// 之所以默认选它:GPU 路径在拖动缩放窗口时,新尺寸的交换链还没重建完,
    /// DXGI 会按 `DXGI_SCALING_STRETCH` 把旧尺寸的后缓冲缩放铺满新客户区,
    /// 肉眼看到的就是文字被横向拉长一下再弹回。CPU 路径用 GDI `BitBlt`
    /// 做 1:1 像素拷贝,没有"缩放"这个环节,所以不会出现该现象。
    /// 想看 GPU 加速可以显式写 `render = "gpu"`,代价是拖动时会有上述抖动。
    /// 原因与两条路径的完整机制见 `docs/rendering-backends.md`。
    Cpu,
    /// wgpu / DX12。速度快,但拖动缩放时旧帧会被 DXGI 拉伸。
    Gpu,
}

impl RenderMode {
    pub fn as_str(self) -> &'static str {
        match self {
            RenderMode::Auto => "auto",
            RenderMode::Cpu => "cpu",
            RenderMode::Gpu => "gpu",
        }
    }
}

/// 拖动窗口边框(live resize)时,终端行列数怎么跟着窗口变。
///
/// wezterm 的 `WindowEvent::Resized { live_resizing }` 一进来就
/// `apply_dimensions()`,行列数是实时的,文字位置跟着窗口实时调整;
/// 这里默认与它一致。`freeze` / `debounce` 是 VectorTerm 扩展,给
/// ConPTY + PSReadLine 组合下重绘太跳的机器留的退路。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LiveResizeMode {
    /// 每次尺寸变化立即同步 PTY(wezterm 行为,文字实时重排)
    #[default]
    Realtime,
    /// 拖动中按 `resize_debounce_ms` 防抖,停下才同步
    Debounce,
    /// 拖动中完全冻结行列数,松手后一次性同步
    Freeze,
}

impl LiveResizeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            LiveResizeMode::Realtime => "realtime",
            LiveResizeMode::Debounce => "debounce",
            LiveResizeMode::Freeze => "freeze",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "realtime" | "live" | "immediate" => Some(Self::Realtime),
            "debounce" | "settle" => Some(Self::Debounce),
            "freeze" | "defer" => Some(Self::Freeze),
            _ => None,
        }
    }
}

/// 双显卡机器上选哪块 GPU。终端是常驻窗口,默认核显(省电、发热低);
/// 独显更适合 4K/高刷屏,写 `gpu_power = "high"` 切换。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GpuPower {
    #[default]
    Low,
    High,
}

impl GpuPower {
    pub fn as_str(self) -> &'static str {
        match self {
            GpuPower::Low => "low",
            GpuPower::High => "high",
        }
    }
}

/// Ctrl+Shift+V 的语义。默认按 Windows 终端惯例 = 粘贴;
/// 设成 "copy" 则与 Ctrl+Shift+C 对调(V 复制 / C 粘贴)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VKeyAction {
    Paste,
    Copy,
}

/// 鼠标选区与复制
#[derive(Debug, Clone)]
pub struct SelectionConfig {
    /// 松开鼠标即把选区写进系统剪贴板(PuTTY / Windows Terminal 风格)
    pub copy_on_select: bool,
    /// 拖选到窗口上/下边缘时自动滚动回看缓冲并继续扩选
    pub edge_scroll: bool,
    /// 选区底色(#RGB / #RRGGBB)
    pub background: String,
    /// 高亮不透明度 0..1
    pub opacity: f32,
    /// Ctrl+Shift+V 是粘贴还是复制
    pub ctrl_shift_v: VKeyAction,
}

impl Default for SelectionConfig {
    fn default() -> Self {
        Self {
            copy_on_select: true,
            edge_scroll: true,
            background: "#3a6ea5".to_string(),
            opacity: 0.45,
            ctrl_shift_v: VKeyAction::Paste,
        }
    }
}

/// 单个标签的配色(wezterm 的 `TabColor` 子集)
#[derive(Debug, Clone)]
pub struct TabColor {
    pub bg: String,
    pub fg: String,
}

impl TabColor {
    fn new(bg: &str, fg: &str) -> Self {
        Self {
            bg: bg.to_string(),
            fg: fg.to_string(),
        }
    }
}

/// 图形协议(iTerm2 / Kitty / sixel)。
///
/// 三条协议都由 wezterm-term 解析成"挂在单元格上的图",渲染侧一视同仁。
/// 只要 `enable = false`,Kitty 图形协议就不回响应(应用会走降级路径),
/// iTerm2 / sixel 的图也不再绘制。
#[derive(Debug, Clone)]
pub struct ImageConfig {
    pub enabled: bool,
    /// 解码后位图的内存预算(MB),超出按最久未用淘汰
    pub max_memory_mb: usize,
}

impl Default for ImageConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_memory_mb: 256,
        }
    }
}

/// 标签栏配色。字段名与 wezterm 的 `colors.tab_bar` 一致:
/// `background` / `active_tab` / `inactive_tab` / `inactive_tab_hover` /
/// `new_tab` / `new_tab_hover` / `inactive_tab_edge`。
#[derive(Debug, Clone)]
pub struct TabBarColors {
    pub background: String,
    pub active: TabColor,
    pub inactive: TabColor,
    pub inactive_hover: TabColor,
    pub new_tab: TabColor,
    pub new_tab_hover: TabColor,
    /// 非活动标签之间的分隔线;空字符串 = 不画(wezterm 的 `inactive_tab_edge`)
    pub edge: String,
}

impl Default for TabBarColors {
    fn default() -> Self {
        Self {
            background: "#0c0c0c".to_string(),
            active: TabColor::new("#2f2f2f", "#ffffff"),
            inactive: TabColor::new("#161616", "#a0a0a0"),
            inactive_hover: TabColor::new("#262626", "#e6e6e6"),
            new_tab: TabColor::new("#0c0c0c", "#a0a0a0"),
            new_tab_hover: TabColor::new("#2f2f2f", "#ffffff"),
            edge: "#2a2a2a".to_string(),
        }
    }
}

/// 标签栏。既接受 VectorTerm 的 `tab_bar = { ... }`,也接受 wezterm 的顶层写法:
/// `enable_tab_bar` / `tab_bar_at_bottom` / `use_fancy_tab_bar` /
/// `hide_tab_bar_if_only_one_tab` / `tab_max_width` /
/// `show_tab_index_in_tab_bar` / `show_new_tab_button_in_tab_bar`。
#[derive(Debug, Clone)]
pub struct TabBarConfig {
    pub enabled: bool,
    pub at_bottom: bool,
    /// true = 圆角/悬停强调(wezterm 的 fancy tab bar);false = 纯扁平色块
    pub fancy: bool,
    pub hide_if_one: bool,
    /// 单个标签的最大宽度(单位:单元格)。默认 16,与 wezterm 一致
    pub max_width_cells: usize,
    pub show_index: bool,
    pub show_new_tab_button: bool,
    /// 每个标签右侧的关闭按钮(wezterm 没有,VectorTerm 自加)
    pub show_close_button: bool,
    /// 标签圆角半径(像素)。None = 按标签栏高度自动;0 = 直角
    pub corner_radius: Option<f64>,
    /// 标签栏内容左右各让出的安全距离(逻辑像素)。None = 自动跟随窗口圆角
    pub edge_padding: Option<f64>,
    /// 标签栏到窗口上边框(底部栏时到下边框)的距离(逻辑像素)。
    /// None = 自动跟随窗口圆角;0 = 贴边
    pub top_padding: Option<f64>,
    pub colors: TabBarColors,
}

impl Default for TabBarConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            at_bottom: false,
            fancy: true,
            hide_if_one: false,
            max_width_cells: 16,
            show_index: true,
            show_new_tab_button: true,
            show_close_button: true,
            corner_radius: None,
            edge_padding: None,
            top_padding: None,
            colors: TabBarColors::default(),
        }
    }
}

/// 标签栏右侧状态栏的外观与内容(第 8.7 节)。
///
/// 状态栏里"写什么"完全交给配置里的脚本(`update-right-status` /
/// `window:set_right_status`),这里只管"长什么样":
///
/// - `font_scale` 文字大小(相对终端字号);
/// - `height` 整条栏的最小高度;
/// - `padding` 左右留白;
/// - `item_spacing` 相邻两段文字之间的额外间距(物理像素)。
///
/// 间距按**渲染出来的片段**算,也就是 `wezterm.format` 里每换一次颜色的那一段,
/// 所以无论状态栏是脚本拼的、还是 `text` 里写的,调它都立刻有反应。
#[derive(Debug, Clone)]
pub struct StatusBarConfig {
    /// false = 右侧状态栏整段不画(脚本结果一起忽略)
    pub enabled: bool,
    /// 脚本自己 `set_right_status` 的结果是否保留
    pub keep_script: bool,
    /// 状态栏文字相对终端字号的倍数(0.5 ~ 2.0)
    pub font_scale: f64,
    /// 整条栏(标签栏 + 状态栏)的最小高度,物理像素。None = 跟随字号
    pub height: Option<f64>,
    /// 状态栏左右留白,物理像素
    pub padding: f64,
    /// 相邻两段文字之间的额外间距,物理像素(按状态栏自己的字号绘制)
    pub item_spacing: f64,
    /// 状态栏里一个空格占几个字宽(0.2 ~ 2.0)。
    ///
    /// 脚本里那些"分隔带"其实是空格:`'   ·   '` 就是点两侧各三个字宽,
    /// 所以看着松。这一项按倍数缩放空格的推进量 —— 调到 0.5 就变成各 1.5 字宽,
    /// 调到 0 空格完全不占位置。字形本身不动,只有空格变窄,所以字不会被压扁。
    pub space_width: f64,
    /// 固定文字,排在脚本内容前面(想纯手写一小段时用)
    pub text: String,
}

impl Default for StatusBarConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            keep_script: true,
            font_scale: 1.0,
            height: None,
            padding: 8.0,
            item_spacing: 0.0,
            space_width: 1.0,
            text: String::new(),
        }
    }
}

impl StatusBarConfig {
    /// 有没有写固定文字。没有时完全沿用脚本的输出。
    pub fn has_text(&self) -> bool {
        !self.text.trim().is_empty()
    }
}

/// wezterm `integrated_title_buttons` 里的一枚按钮。
/// wezterm 沿用了 macOS 的叫法,把"最小化"写作 `Hide`;Windows 上就是最小化。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleButton {
    Hide,
    Maximize,
    Close,
}

impl TitleButton {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "hide" | "minimize" | "min" => Some(Self::Hide),
            "maximize" | "max" => Some(Self::Maximize),
            "close" => Some(Self::Close),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonAlignment {
    Left,
    #[default]
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonStyle {
    #[default]
    Windows,
    /// Adwaita 风格:圆形按钮底 + 图标
    Gnome,
}

/// 窗口装饰。字段名与 wezterm 的 `window_decorations` 标志一一对应,
/// 按钮部分对应 `integrated_title_buttons` / `_alignment` / `_color` / `_style`。
///
/// 这里是与旧实现最关键的差别:**按钮和标题栏是两件事**。wezterm 的
/// `"RESIZE"` 只去掉系统标题栏,标签栏里并不画按钮;只有
/// `"INTEGRATED_BUTTONS|RESIZE"` 才把按钮搬进标签栏;而
/// `integrated_title_buttons = {}` 又能把按钮全关掉,顶部那条标签栏仍在,
/// 照样能拖动窗口。旧实现把 "关标题栏" 和 "画三枚按钮" 绑死了。
#[derive(Debug, Clone)]
pub struct WindowDecorations {
    /// `TITLE`:Windows 系统标题栏(原生最小化 / 最大化 / 关闭都在上面)
    pub title: bool,
    /// `RESIZE`:允许缩放窗口。有 `TITLE` 时由系统边框负责;没有 `TITLE` 时
    /// winit 无装饰窗口的非客户区被压成 0,改由程序自绘四边/四角热区
    pub resize: bool,
    /// `INTEGRATED_BUTTONS`:把窗口按钮画进标签栏
    pub integrated_buttons: bool,
    /// `integrated_title_buttons`:按钮顺序与集合,空 = 三枚全隐藏
    pub buttons: Vec<TitleButton>,
    /// `integrated_title_button_alignment`
    pub alignment: ButtonAlignment,
    /// `integrated_title_button_color`:None = `"Auto"`(跟随标签栏前景色)
    pub color: Option<String>,
    /// `integrated_title_button_style`
    pub style: ButtonStyle,
    /// Win11 无边框窗口的圆角。`window_decorations` 去掉 TITLE 后 winit 把
    /// 非客户区压成 0,DWM 默认画方角;默认通过 `DWMWCP_ROUND` 要回圆角。
    /// 这是 VectorTerm 自己的开关(wezterm 没有同名项),`false` = 强制方角。
    pub rounded_corners: bool,
}

impl Default for WindowDecorations {
    fn default() -> Self {
        Self {
            // 默认用 Windows 自己的标题栏:系统阴影、贴边吸附(Aero Snap)、
            // 双击标题栏最大化、右键系统菜单都在,不用自己模拟。
            // 想要无边框 + 标签栏集成按钮写 "INTEGRATED_BUTTONS|RESIZE"。
            title: true,
            resize: true,
            integrated_buttons: false,
            buttons: vec![TitleButton::Hide, TitleButton::Maximize, TitleButton::Close],
            alignment: ButtonAlignment::Right,
            color: None,
            style: ButtonStyle::Windows,
            rounded_corners: true,
        }
    }
}

impl WindowDecorations {
    /// 没有系统标题栏时,标签栏要兼任标题栏(空白处可拖动窗口)
    pub fn csd(&self) -> bool {
        !self.title
    }

    /// 标签栏里要不要画窗口按钮。系统标题栏存在时按钮由系统提供,不重复画。
    pub fn show_buttons(&self) -> bool {
        self.csd() && self.integrated_buttons && !self.buttons.is_empty()
    }

    /// `RESIZE` 且没有系统标题栏时,由程序自绘四边/四角缩放热区。
    ///
    /// winit 的无装饰窗口是在 `WM_NCCALCSIZE` 里把非客户区压成 0 来实现的,
    /// 系统不会真的画出缩放边框;所以改 `WS_THICKFRAME` 没用,得自己接管。
    /// `NONE` 两种边框都不给,和 wezterm 文档里"NONE 无法缩放"一致。
    pub fn own_resize_border(&self) -> bool {
        !self.title && self.resize
    }

    /// 解析 `window_decorations` 的 flag set,返回 (TITLE, RESIZE, INTEGRATED_BUTTONS)。
    /// 未知标志(macOS 专有、拼错)只提示不报错,避免一条配置挡住启动。
    pub fn parse_flags(spec: &str) -> (bool, bool, bool) {
        let mut title = false;
        let mut resize = false;
        let mut integrated = false;
        for part in spec.split('|') {
            match part.trim().to_ascii_uppercase().as_str() {
                "NONE" | "" => {}
                "TITLE" => title = true,
                "RESIZE" => resize = true,
                "INTEGRATED_BUTTONS" => integrated = true,
                other if other.starts_with("MACOS_") => {
                    // macOS 专有标志在 Windows 上无意义
                }
                other => crate::wlog!("配置:未知的 window_decorations 标志 {other:?},已忽略"),
            }
        }
        // wezterm 的定义是"用标签栏按钮代替标题栏",两者互斥
        if integrated {
            title = false;
        }
        (title, resize, integrated)
    }
}

/// Nerd Font 图标字形的来源
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NerdFontMode {
    /// 不加载图标字体
    Off,
    /// 自动在系统里找 Nerd Font(Symbols / Mono 变体优先)
    Auto,
    /// 使用指定家族
    Family(String),
}

#[derive(Debug, Clone)]
pub struct FontsConfig {
    /// 主字体:家族名(如 "Consolas" / "MesloLGL Nerd Font Mono")或字体文件路径
    pub family: String,
    /// 逻辑字号(px @96dpi,运行时再乘窗口 DPI 缩放)
    pub size: f32,
    /// 行高倍数(wezterm 的 `line_height`)。None = 自动:
    /// 用字体自身 asc-desc,但限制在 1.0em~1.6em 之间
    /// (个别字体的 hhea 会到 2em,不限制行距会离谱)
    pub line_height: Option<f64>,
    /// 格宽倍数(wezterm 的 `cell_width`)
    pub cell_width: f64,
    /// 文本回退字体链(家族名或路径),按顺序尝试
    pub fallback: Vec<String>,
    /// 四个样式槽,缺省时用主家族里对应的 face
    pub bold: Option<String>,
    pub italic: Option<String>,
    pub bold_italic: Option<String>,
    /// PUA 图标字形(Nerd Font 的 󰀀 / 󰁔 一类字符)的来源
    pub nerd_font: NerdFontMode,
    /// 额外的字体搜索目录
    pub dirs: Vec<PathBuf>,
}

impl Default for FontsConfig {
    fn default() -> Self {
        Self {
            // 和 wezterm 一样:主字体 + 两款内置符号/emoji 字体都编进 exe,
            // 不依赖系统装没装。中文还需系统字体(体积太大,不适合打包)。
            family: crate::builtin_fonts::DEFAULT_FAMILY.to_string(),
            size: 18.0,
            line_height: None,
            cell_width: 1.0,
            fallback: crate::builtin_fonts::DEFAULT_FALLBACK
                .iter()
                .map(|s| s.to_string())
                .collect(),
            bold: None,
            italic: None,
            bold_italic: None,
            nerd_font: NerdFontMode::Auto,
            dirs: Vec::new(),
        }
    }
}

/// 会话(PTY 里的程序)退出后怎么处理,对应 wezterm 的 `exit_behavior`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExitBehavior {
    /// 关掉这个标签;如果它是最后一个标签,窗口一起退出。wezterm 的默认值。
    #[default]
    Close,
    /// 留在原地,末尾显示一行"[会话结束]"。对应 wezterm 的 "Hold"。
    Hold,
}

impl ExitBehavior {
    /// 从配置字符串解析,大小写不敏感(wezterm 写 "Close" / "Hold")。
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "close" => Some(Self::Close),
            "hold" => Some(Self::Hold),
            _ => None,
        }
    }
}

/// 默认 shell:PowerShell 7 的应用执行别名。
///
/// 这台机器上没装 PS7 时不会起不来 —— [`crate::pty`] 会在启动前依次回退到
/// Windows 自带的 `powershell.exe`、再回退到 `cmd.exe`(见 `pty::FALLBACKS`)。
pub const DEFAULT_SHELL: &str = "pwsh.exe";

/// 默认 shell 的参数。`-NoLogo` 关掉启动横幅:终端里那段版权信息既没用,
/// 又占掉宝贵的滚动缓冲,还会让"新开标签"看起来像是慢了一拍。
pub const DEFAULT_SHELL_ARGS: &[&str] = &["-NoLogo"];

/// 关窗前不算"有程序在跑"的进程名 —— wezterm 的
/// `skip_close_confirmation_for_processes_named` 的默认表。
///
/// 前九个和 wezterm 一字不差;后两个是 ConPTY 的宿主进程,wezterm 的名单里
/// 没有它们(那边的宿主挂在终端自己下面,不在 shell 的子树里),我们这边
/// 会挂进 shell 的子树,漏掉就会每次关窗都弹框。
pub const DEFAULT_SKIP_CLOSE_CONFIRMATION: &[&str] = &[
    "bash",
    "sh",
    "zsh",
    "fish",
    "tmux",
    "nu",
    "cmd.exe",
    "pwsh.exe",
    "powershell.exe",
    "conhost.exe",
    "openconsole.exe",
];

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub shell: String,
    /// 传给 shell 的额外参数
    pub shell_args: Vec<String>,
    /// 追加/覆盖的环境变量(按顺序)
    pub env: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
    /// 会话输出记录(原始字节,含转义序列);None = 不记录
    pub session_log: Option<PathBuf>,
    /// 关窗 / 关标签时的白名单:整棵进程树里只要有一个名字不在表里,就算
    /// "还有程序在跑",先问一句(wezterm 的
    /// `skip_close_confirmation_for_processes_named`)
    pub skip_close_confirmation_for_processes_named: Vec<String>,
    pub fonts: FontsConfig,
    pub padding: f64,
    pub scrollback_lines: usize,
    /// 拖动缩放时行列数怎么跟随窗口。默认 `realtime`,与 wezterm 的
    /// `apply_dimensions()` 一致:文字位置随窗口实时调整。
    /// `debounce` / `freeze` 是 VectorTerm 扩展,见 `LiveResizeMode`。
    pub live_resize_mode: LiveResizeMode,
    /// 非拖动场景(程序化改尺寸、最大化 / Snap、Win+方向键)连续 resize
    /// 事件停止多少毫秒后才把新行列数同步给 PTY。`0` = 立即同步。
    /// 拖动期间的语义由 `live_resize_mode` 决定,不受这一项影响。
    pub resize_debounce_ms: u64,
    /// wezterm 的 `use_resize_increments`:把窗口的缩放步长设为一个字符格
    /// (`cell_w × cell_h`),让 WM_SIZE 只在跨过整格时发生,而不是每个像素一次。
    /// 对应 wezterm-gui 的 `ResizeIncrementCalculator` +
    /// `window.set_resize_increments(...)`。`false` = 允许逐像素缩放。
    pub use_resize_increments: bool,
    pub colors: ColorsConfig,
    pub cursor: CursorConfig,
    pub scroll: ScrollConfig,
    /// 右侧可视化滚动条(显示视口在 scrollback 中的位置、可拖动)
    pub scrollbar: ScrollbarConfig,
    /// 历史搜索栏
    pub search: SearchConfig,
    /// 渲染后端:auto / cpu / gpu
    pub render: RenderMode,
    /// 双显卡机器上优先核显(low)还是独显(high)
    pub gpu_power: GpuPower,
    pub selection: SelectionConfig,
    pub tab_bar: TabBarConfig,
    /// 图形协议:iTerm2(OSC 1337 File)/ Kitty(APC G)/ sixel
    pub images: ImageConfig,
    /// wezterm 的 `initial_cols` / `initial_rows`:按单元格数定初始窗口大小
    pub initial_cols: Option<usize>,
    pub initial_rows: Option<usize>,
    /// 启动窗口的**像素**尺寸(逻辑像素,VectorTerm 扩展;wezterm 没有这两项)。
    ///
    /// 优先级高于 `initial_cols` / `initial_rows`:写了它们就按像素开窗,
    /// 之后再怎么调 `font.size` 都**不会**改变窗口尺寸 —— 因为窗口大小不再
    /// 由"几列几行"反推。想"字号变大、窗口不变",就把这两项写上。
    pub window_width: Option<f64>,
    pub window_height: Option<f64>,
    /// 窗口装饰:wezterm 的 `window_decorations` 一组标志 +
    /// `integrated_title_*` 按钮配置。见 [`WindowDecorations`]。
    pub window: WindowDecorations,
    /// 自定义背景:纯色 / 图片 / 渐变 / 透明度 / Win11 毛玻璃。见 [`BackgroundConfig`]。
    pub background: BackgroundConfig,
    /// 分屏中间那根分割线的粗细 / 间距 / 两端留白 / 单色或渐变。见 [`SplitDividerConfig`]。
    pub split_divider: SplitDividerConfig,
    /// 分屏焦点:哪个 pane 显示输入光标、以及切换时的飞行动画。见 [`PaneFocusConfig`]。
    pub pane_focus: PaneFocusConfig,
    /// 文件浏览器:编辑器命令、侧边栏宽度、是否显示隐藏文件。见 [`FilesGuiConfig`]。
    pub files_gui: FilesGuiConfig,
    /// 关闭窗口时,如果某个标签里还有程序在跑(不只是 shell 本身),
    /// 先弹一个确认框。默认开,对应 wezterm 的"关闭前确认"行为。
    pub quit_confirmation: bool,
    /// 标签里的程序退出(比如在 shell 里敲 exit)之后怎么办。
    pub exit_behavior: ExitBehavior,
    /// 会话保持(wezterm 式多路复用):开启后所有 shell 由后台守护进程
    /// (`vectorterm --mux-server`)托管,关掉窗口/标签后会话继续运行,
    /// 下次启动可 reattach 恢复现场。默认关。
    pub session_persistence: bool,
    /// 状态栏刷新间隔(毫秒),对应 wezterm 的 `status_update_interval`。
    /// `update-status` / `update-right-status` 就是按这个节奏派发的。
    pub status_update_interval: u64,
    /// 状态栏外观与自定义模块。见 [`StatusBarConfig`]。
    pub status_bar: StatusBarConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            shell: DEFAULT_SHELL.to_string(),
            shell_args: DEFAULT_SHELL_ARGS.iter().map(|s| s.to_string()).collect(),
            env: Vec::new(),
            cwd: None,
            session_log: None,
            skip_close_confirmation_for_processes_named: DEFAULT_SKIP_CLOSE_CONFIRMATION
                .iter()
                .map(|s| s.to_string())
                .collect(),
            fonts: FontsConfig::default(),
            padding: 8.0,
            scrollback_lines: 3500,
            live_resize_mode: LiveResizeMode::default(),
            resize_debounce_ms: 150,
            use_resize_increments: true,
            colors: ColorsConfig {
                foreground: "#cccccc".to_string(),
                background: "#0c0c0c".to_string(),
                cursor_fg: "#0c0c0c".to_string(),
                cursor_bg: "#ffffff".to_string(),
                ansi: Vec::new(),
                indexed: Vec::new(),
            },
            cursor: CursorConfig::default(),
            scroll: ScrollConfig::default(),
            scrollbar: ScrollbarConfig::default(),
            search: SearchConfig::default(),
            // 默认软件渲染:GPU 路径拖动缩放时旧帧会被 DXGI 拉伸,详见
            // `docs/rendering-backends.md`。要 GPU 加速就显式写 render = "gpu"。
            render: RenderMode::Cpu,
            gpu_power: GpuPower::Low,
            selection: SelectionConfig::default(),
            tab_bar: TabBarConfig::default(),
            images: ImageConfig::default(),
            initial_cols: None,
            initial_rows: None,
            window_width: None,
            window_height: None,
            window: WindowDecorations::default(),
            background: BackgroundConfig::default(),
            split_divider: SplitDividerConfig::default(),
            pane_focus: PaneFocusConfig::default(),
            files_gui: FilesGuiConfig::default(),
            quit_confirmation: true,
            exit_behavior: ExitBehavior::Close,
            session_persistence: false,
            status_update_interval: 1000,
            status_bar: StatusBarConfig::default(),
        }
    }
}

/// 加载配置:`--config` → `~/.config/VectorTerm/current/` 下唯一那份 Lua → 程序目录默认配置。
/// 返回 (配置, 提示信息)。配置读不到 / 解析失败 / 目录里有多个 lua 时回退默认值,
/// 保证可启动;提示信息由调用方写进日志与调试终端。
pub fn load(explicit: Option<&Path>) -> (AppConfig, Option<String>) {
    let (path, note) = resolve_path_ex(explicit);
    let join = |msg: String| match &note {
        Some(n) => format!("{n};{msg}"),
        None => msg,
    };
    let Some(path) = path else {
        return (AppConfig::default(), note);
    };
    match std::fs::read_to_string(&path) {
        Err(e) => (
            AppConfig::default(),
            Some(join(format!("读取 {} 失败:{e}", path.display()))),
        ),
        Ok(src) => match parse_with_theme(&src, &path) {
            Ok(mut c) => {
                resolve_background_path(&mut c, &path);
                (c, note)
            }
            Err(e) => (
                AppConfig::default(),
                Some(join(format!("解析 {} 失败:{e}", path.display()))),
            ),
        },
    }
}

/// 把一次配置加载错误追加到 `~/.config/VectorTerm/config-error.log`。
///
/// 发行版是 GUI 子系统,双击启动时没有控制台,stderr 写到哪里没人看得见;
/// 窗口本身照常打开(用的默认配置),用户很容易以为"我的配置没生效"是 bug。
/// 错误本身由 `debug_term` 送进独立的调试终端(绝不写进当前 shell 窗口),
/// 这里再留一份能事后翻的专用记录。超过 64KB 直接重来,只看最近几次。
pub fn log_error(err: &str) {
    let Some(dir) = user_config_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("config-error.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 64 * 1024) {
        let _ = std::fs::remove_file(&path);
    }
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    use std::io::Write;
    // 错误文本自己就带"解析 <配置路径> 失败:...",这里只补一条分隔线
    let _ = f.write_all(b"---\n");
    let _ = f.write_all(err.trim_end().as_bytes());
    let _ = f.write_all(b"\n");
}

/// 用户配置目录:`%USERPROFILE%\.config\VectorTerm\`
/// (跟着 Linux/macOS 的 XDG 习惯放,Windows 上同样用 `.config`)
pub fn user_config_dir() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    let base = PathBuf::from(home).join(".config");
    let new_dir = base.join("VectorTerm");
    let old_dir = base.join("wintty");
    // 更名迁移:首次启动把旧目录整块搬过来,配置 / 键位 / 背景图原样保留。
    // 搬不动(被占用、跨卷、权限不足)就继续用旧目录 —— 改名不能让人丢配置。
    if !new_dir.exists() && old_dir.is_dir() {
        match std::fs::rename(&old_dir, &new_dir) {
            Ok(()) => crate::wlog!(
                "[VectorTerm] 配置目录已迁移:{} → {}",
                old_dir.display(),
                new_dir.display()
            ),
            Err(e) => {
                // 并发启动时另一个窗口可能刚好搬完:新目录已经出现就跟着用新的,
                // 否则退回旧目录(它还在,说明这次没搬动)。
                if new_dir.exists() {
                    return Some(new_dir);
                }
                crate::wlog!(
                    "[VectorTerm] 迁移旧配置目录失败({e}),继续使用 {}",
                    old_dir.display()
                );
                return Some(old_dir);
            }
        }
    }
    Some(new_dir)
}

/// 「当前生效配置」目录:`~/.config/VectorTerm/current/`。
///
/// 配置管理器把用户选中的那份 Lua **复制**到这里;终端只认这个目录里的
/// **唯一一份** `.lua`,文件名随意(`mine.lua`、`deepseek.lua` 都行)。
/// 目录里出现多份就是配置冲突 —— 终端报错并退回程序自带默认配置,而不是
/// 猜一份来加载。这个目录独立于 `~/.config/VectorTerm/` 根目录,所以根目录里
/// 放多少份候选配置都不会互相干扰。
pub fn active_config_dir() -> Option<PathBuf> {
    user_config_dir().map(|d| d.join("current"))
}

/// 键位文件目录:`~/.config/VectorTerm/keys/`。
///
/// 键位 Lua 单独放一层:它们也是 `.lua`,不能和「当前配置」挤在同一个目录里
/// (否则会被当成"多份配置")。配置里的 `keys_file = "keys.lua"` 会先按配置
/// 所在目录找,找不到再到这个目录里找。
pub fn keys_dir() -> Option<PathBuf> {
    user_config_dir().map(|d| d.join("keys"))
}

pub fn resolve_path(explicit: Option<&Path>) -> Option<PathBuf> {
    resolve_path_ex(explicit).0
}

/// 解析出真正要加载的配置文件,同时带回"为什么退回了默认配置"的说明。
///
/// 优先级:
/// 1. `--config <路径>` —— 显式指定,存在与否都直接用它;
/// 2. `~/.config/VectorTerm/current/` 下唯一的 `.lua`(**用户选定的当前配置**);
/// 3. 程序自带默认配置:`<exe>/config/config.lua` → `<exe>/config.lua` → `<cwd>/config/config.lua`。
///
/// 第 2 步里如果目录下有 0 份 lua,就静默落到第 3 步(首次启动的正常状态);
/// 有 2 份及以上则视为配置冲突:返回第 3 步的路径 + 一条错误说明。
pub fn resolve_path_ex(explicit: Option<&Path>) -> (Option<PathBuf>, Option<String>) {
    if let Some(p) = explicit {
        return (Some(p.to_path_buf()), None);
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut note = None;
    if let Some(dir) = active_config_dir() {
        match pick_active(&dir) {
            Ok(Some(p)) => return (Some(p), None),
            Ok(None) => {}
            Err(e) => note = Some(e),
        }
    }
    (default_config_path(exe_dir.as_deref(), &cwd), note)
}

/// 读「当前配置」目录,返回里面唯一的 `.lua`。
///
/// - 目录不存在 / 没有 lua → `Ok(None)`,走程序默认配置;
/// - 恰好一份 → `Ok(Some(路径))`;
/// - 多份 → `Err(说明)`,调用方退回默认配置并报错。
pub fn pick_active(dir: &Path) -> Result<Option<PathBuf>, String> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Ok(None);
    };
    let mut files: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("lua"))
        })
        .collect();
    files.sort();
    match files.len() {
        0 => Ok(None),
        1 => Ok(files.pop()),
        n => {
            let names = files
                .iter()
                .map(|p| {
                    p.file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join("、");
            Err(format!(
                "当前配置目录 {} 下有 {n} 个 .lua({names}),只允许有一份。\
                 已退回程序自带默认配置;请在配置管理器里重新「设为当前配置」",
                dir.display()
            ))
        }
    }
}

/// 程序自带的默认配置(随包分发),依次尝试三种布局。
fn default_config_path(exe_dir: Option<&Path>, cwd: &Path) -> Option<PathBuf> {
    let bundled = |dir: &Path| dir.join("config").join("config.lua");
    [
        exe_dir.map(bundled),
        exe_dir.map(|d| d.join("config.lua")),
        Some(bundled(cwd)),
    ]
    .into_iter()
    .flatten()
    .find(|candidate| candidate.exists())
}

/// 建一个带内存上限的 Lua 状态。
///
/// 配置脚本是用户自己写的,但一个手滑的 `('x'):rep(1e9)` 就能把内存吃光 ——
/// Rust 里分配失败是 abort,窗口直接消失、什么都不留。给 Lua 堆设个上限,
/// 超了只是普通的 Lua 错误,会走"配置加载失败 → 默认配置"这条已有分支。
///
/// 注意这只挡内存,不挡死循环:配置文件里写 `while true do end` 仍然会卡住
/// 启动(在 README 的已知限制里写了)。
pub(crate) fn new_lua() -> Lua {
    let lua = Lua::new();
    let _ = lua.set_memory_limit(64 * 1024 * 1024);
    lua
}

/// 内置主题的名字会拼进文件路径,只允许安全字符 —— 挡住 `..` / 路径分隔符 /
/// 绝对路径,免得一份配置把 exe 目录外的任意 `.lua` 读进来。
fn valid_theme_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// 编译期嵌入的主题预设:四套配色随 exe 一起走,不依赖磁盘上的 `config/`。
///
/// 分发时只想给一个 `VectorTerm.exe` 也能写 `theme = "chatgpt"`;磁盘上有同名
/// 文件时仍然以磁盘为准(方便用户改色、覆盖),没有才落到这里。
pub const BUILTIN_THEMES: &[(&str, &str)] = &[
    ("deepseek", include_str!("../config/deepseek.lua")),
    ("chatgpt", include_str!("../config/chatgpt.lua")),
    ("claude", include_str!("../config/claude.lua")),
    ("gemini", include_str!("../config/gemini.lua")),
];

/// 按名字取内嵌主题源码。名字不合法 / 没这套 → `None`。
pub fn builtin_theme(name: &str) -> Option<&'static str> {
    if !valid_theme_name(name) {
        return None;
    }
    BUILTIN_THEMES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, src)| *src)
}

/// 取主题源码:磁盘优先(用户目录 / `<exe>/config/` / 仓库根),找不到再回退内嵌。
///
/// 返回值第二项是来源说明,只用于报错和日志(路径,或 `<内置>`),不参与解析。
///
/// `config_path` 既可以是主配置**文件**,也可以直接是它所在的**目录** ——
/// 解析链路里两种都有(config.rs 传文件、script.rs 传目录)。
pub fn theme_source(config_path: &Path, name: &str) -> Option<(String, String)> {
    if !valid_theme_name(name) {
        return None;
    }
    let dir = if config_path.is_dir() {
        config_path.to_path_buf()
    } else {
        config_path.parent().unwrap_or(config_path).to_path_buf()
    };
    if let Some(p) = theme_path(&dir, name) {
        if let Ok(src) = std::fs::read_to_string(&p) {
            return Some((src, p.display().to_string()));
        }
    }
    builtin_theme(name).map(|src| (src.to_owned(), "<内置>".to_owned()))
}

/// 主题文件查找顺序:用户配置旁边 → 随包分发的 `<exe>/config/` → 仓库根(开发时)。

/// 每处都同时认 `<name>.lua` 和 `themes/<name>.lua` 两种布局。
///
/// 按 `theme = "name"` 找到主题文件的真实路径。`dir` 是**主配置所在目录**。
///
/// 配置解析那边只需要主题里的**数据**(配色等);但主题还可能包含
/// `wezterm.on('update-right-status', …)` 这类代码(内置 DeepSeek 主题的状态栏
/// 就是这么写的),而代码得在 `ScriptEngine` 那个 Lua 实例里执行才会注册回调。
/// 所以把"找主题文件"这一步暴露出来,给 script.rs 复用同一套查找顺序。
pub fn theme_path(dir: &Path, name: &str) -> Option<PathBuf> {
    if !valid_theme_name(name) {
        return None;
    }
    // theme_candidates 要的是"配置文件路径",这里用同目录的占位名换算
    let anchor = dir.join("config.lua");
    theme_candidates(&anchor, name)
        .into_iter()
        .find(|p| p.is_file())
}

fn theme_candidates(config_path: &Path, name: &str) -> Vec<PathBuf> {
    let file = format!("{name}.lua");
    let mut out = Vec::new();
    if let Some(dir) = config_path.parent() {
        out.push(dir.join("themes").join(&file));
        out.push(dir.join(&file));
    }
    if let Some(exe) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        out.push(exe.join("config").join("themes").join(&file));
        out.push(exe.join("config").join(&file));
    }
    if let Ok(cwd) = std::env::current_dir() {
        out.push(cwd.join("config").join("themes").join(&file));
        out.push(cwd.join("config").join(&file));
    }
    out
}

/// 把 `over` 深合并进 `base`:同名的表递归合并,其它值直接覆盖。
/// 这样写完 `theme = "deepseek"` 后只改 `colors.foreground`,也能保留主题里
/// 的其它配色,不必把整份主题复制过来。
fn merge_theme(lua: &Lua, base: &mlua::Table, over: &mlua::Table) -> Result<(), String> {
    let merge: mlua::Function = lua
        .load(
            r#"
            local function merge(base, over)
                for k, v in pairs(over) do
                    if type(v) == "table" and type(base[k]) == "table" then
                        merge(base[k], v)
                    else
                        base[k] = v
                    end
                end
            end
            return merge
            "#,
        )
        .eval()
        .map_err(|e| e.to_string())?;
    merge
        .call::<()>((base.clone(), over.clone()))
        .map_err(|e| e.to_string())
}

/// 解析一份配置 Lua。
///
/// 现在只有**一份**配置生效(见 [`resolve_path_ex`]:`--config` → `current/` 里
/// 唯一那份 → 程序默认),它自己就写具体配置项,不再有 `config_files` 引用链。
/// 唯一的引用是 `theme = "name"`:外观 / 配色方案作底层,配置里的字段覆盖在
/// 它上面(兼容"主题 + 少量覆盖"的写法)。
///
/// 配置里没写到的键,全部落回代码默认值(`AppConfig::default()`)。
fn parse_with_theme(src: &str, config_path: &Path) -> Result<AppConfig, String> {
    let lua = new_lua();
    crate::wezterm_compat::install(&lua).map_err(|e| e.to_string())?;
    let user: mlua::Table = lua.load(src).eval().map_err(|e| e.to_string())?;

    // 外观 / 配色方案(最底层,可选)
    let mut base: Option<mlua::Table> = None;
    if let Some(name) = user.get::<Option<String>>("theme").ok().flatten() {
        if !valid_theme_name(&name) {
            return Err(format!("theme 名字不合法:{name:?}(只允许字母 / 数字 / - / _)"));
        }
        let (theme_src, origin) = theme_source(config_path, &name).ok_or_else(|| {
            let known: Vec<&str> = BUILTIN_THEMES.iter().map(|(n, _)| *n).collect();
            format!(
                "找不到内置主题 {name:?}:磁盘上没有 config/{name}.lua,也不在内置主题 [{}] 里",
                known.join(", ")
            )
        })?;
        let theme: mlua::Table = lua
            .load(theme_src.as_str())
            .eval()
            .map_err(|e| format!("解析主题 {name:?}({origin})失败:{e}"))?;
        base = Some(theme);
    }

    // 配置本身的字段最后覆盖。
    match base {
        Some(b) => {
            merge_theme(&lua, &b, &user)?;
            parse_table(&b)
        }
        None => parse_table(&user),
    }
}

fn parse(src: &str) -> Result<AppConfig, String> {
    let lua = new_lua();
    // 先装 wezterm 兼容层:`require 'wezterm'` / `wezterm.config_builder()` 等
    // 必须在 eval 之前就位,否则原样的 wezterm 配置第一行就会报错。
    crate::wezterm_compat::install(&lua).map_err(|e| e.to_string())?;
    let tbl: mlua::Table = lua.load(src).eval().map_err(|e| e.to_string())?;
    parse_table(&tbl)
}

/// 从已经求值好的 Lua 表里读出全部配置。`parse` / `parse_with_theme` 共用。
fn parse_table(tbl: &mlua::Table) -> Result<AppConfig, String> {
    let mut cfg = AppConfig::default();

    let mut shell_explicit = false;
    if let Ok(Some(v)) = tbl.get::<Option<String>>("shell") {
        if !v.trim().is_empty() {
            cfg.shell = v;
            // 显式换了 shell,默认的 `-NoLogo` 就不能跟过去 —— cmd.exe 不认它,
            // 会以"无效开关"直接退出。参数要么由下面的 shell_args 给,
            // 要么由 pty 层按目标 shell 补(见 pty::FALLBACKS)。
            cfg.shell_args.clear();
            shell_explicit = true;
        }
    }
    let args: Option<Vec<String>> = tbl.get("shell_args").ok();
    if let Some(a) = args {
        cfg.shell_args = a;
    }
    // wezterm 的 `default_prog = { 'pwsh.exe', '-NoLogo' }`:
    // 第一个元素是程序,其余是参数。显式的 `shell` 优先级更高。
    if !shell_explicit {
        if let Ok(Some(mut prog)) = tbl.get::<Option<Vec<String>>>("default_prog") {
            if !prog.is_empty() {
                cfg.shell = prog.remove(0);
                cfg.shell_args = prog; // 同样丢掉默认参数,只认 default_prog 自带的
            }
        }
    }
    let env_table: Option<mlua::Table> = tbl.get("env").ok().flatten();
    if let Some(t) = env_table {
        for (k, v) in t.pairs::<String, String>().flatten() {
            cfg.env.push((k, v));
        }
    }
    // wezterm 的 `set_environment_variables = { KEY = 'value' }`
    if let Some(t) = tbl
        .get::<Option<mlua::Table>>("set_environment_variables")
        .ok()
        .flatten()
    {
        for (k, v) in t.pairs::<String, String>().flatten() {
            // 同名以后出现的为准(与 wezterm 的覆盖语义一致)
            if let Some(slot) = cfg.env.iter_mut().find(|(key, _)| *key == k) {
                slot.1 = v;
            } else {
                cfg.env.push((k, v));
            }
        }
    }
    let session_log: Option<String> = tbl.get("session_log").ok();
    if let Some(s) = session_log {
        if !s.trim().is_empty() {
            cfg.session_log = Some(PathBuf::from(s));
        }
    }
    let cwd: Option<String> = tbl.get("cwd").ok();
    if let Some(c) = cwd {
        cfg.cwd = Some(PathBuf::from(c));
    }
    // wezterm 的 `window_padding`:VectorTerm 只有统一内边距,取上边距
    // (没有上边距时退回左边距,再没有就接受纯数字写法)
    match tbl.get::<Value>("window_padding").unwrap_or(Value::Nil) {
        Value::Integer(i) => cfg.padding = i as f64,
        Value::Number(n) => cfg.padding = n,
        Value::Table(t) => {
            let pick = |key: &str| -> Option<f64> { t.get::<Option<f64>>(key).ok().flatten() };
            if let Some(v) = pick("top").or_else(|| pick("left")) {
                cfg.padding = v;
            }
        }
        _ => {}
    }
    // wezterm 的 initial_cols / initial_rows:按单元格数决定初始窗口大小
    cfg.initial_cols = tbl
        .get::<Option<usize>>("initial_cols")
        .ok()
        .flatten()
        .filter(|n| *n >= 2);
    cfg.initial_rows = tbl
        .get::<Option<usize>>("initial_rows")
        .ok()
        .flatten()
        .filter(|n| *n >= 2);
    // VectorTerm 扩展:启动窗口的像素尺寸(逻辑像素)。优先级高于 initial_cols/rows,
    // 用来把"窗口多大"和"字形多大"彻底解耦 —— 调 font.size 不再牵动窗口尺寸。
    // 上限 20000 是防止手滑写出天文数字把窗口开到屏幕外。
    cfg.window_width = opt_f64_field(&tbl, "window_width").filter(|v| *v >= 120.0).map(|v| v.min(20000.0));
    cfg.window_height = opt_f64_field(&tbl, "window_height").filter(|v| *v >= 60.0).map(|v| v.min(20000.0));
    // ---- 窗口装饰(wezterm 的一组标志) ----
    // 先按 window_decorations 决定三个维度,再用 VectorTerm 的简写 window_titlebar
    // 覆盖"有没有系统标题栏"这一维。其余维度保持 flags 或默认值的结果,
    // 这样两种写法混用也不会互相打架。
    if let Ok(Some(v)) = tbl.get::<Option<String>>("window_decorations") {
        let (title, resize, integrated) = WindowDecorations::parse_flags(&v);
        cfg.window.title = title;
        cfg.window.resize = resize;
        cfg.window.integrated_buttons = integrated;
    }
    if let Ok(Some(v)) = tbl.get::<Option<bool>>("window_titlebar") {
        cfg.window.title = v;
        if v {
            // 有系统标题栏时,那三枚按钮由系统画,标签栏不重复
            cfg.window.integrated_buttons = false;
        }
    }
    // VectorTerm 扩展:无边框窗口要不要 Win11 圆角(默认 true)
    if let Ok(Some(v)) = tbl.get::<Option<bool>>("window_rounded_corners") {
        cfg.window.rounded_corners = v;
    }
    parse_integrated_buttons(&tbl, &mut cfg.window);
    parse_background(&tbl, &mut cfg.background);
    parse_split_divider(&tbl, &mut cfg.split_divider);
    parse_pane_focus(&tbl, &mut cfg.pane_focus);
    parse_files_gui(&tbl, &mut cfg.files_gui);
    parse_fonts(&tbl, &mut cfg.fonts);
    // ---- 会话结束行为 / 关窗确认 ----
    // exit_behavior = "Close"(默认) | "Hold",wezterm 同名项
    if let Ok(Some(v)) = tbl.get::<Option<String>>("exit_behavior") {
        match ExitBehavior::parse(&v) {
            Some(b) => cfg.exit_behavior = b,
            None => {
                crate::wlog!("配置:未知的 exit_behavior = {v:?}(可选 Close / Hold),按 Close 处理")
            }
        }
    }
    // quit_confirmation = false 直接关窗,不弹确认
    if let Ok(Some(v)) = tbl.get::<Option<bool>>("quit_confirmation") {
        cfg.quit_confirmation = v;
    }
    // skip_close_confirmation_for_processes_named = { "bash", "cmd.exe" }
    // 关窗前先减去这张白名单;写空表 = 连 shell 自己都算"有程序在跑"。
    if let Ok(Some(list)) = tbl.get::<Option<Vec<String>>>("skip_close_confirmation_for_processes_named")
    {
        cfg.skip_close_confirmation_for_processes_named = list;
    }
    // session_persistence = true 开启会话保持(wezterm 式多路复用)
    if let Ok(Some(v)) = tbl.get::<Option<bool>>("session_persistence") {
        cfg.session_persistence = v;
    }
    // render = "cpu" | "gpu" | "auto"(默认 cpu);也接受 { backend = "gpu" }
    let render: Option<String> = tbl.get("render").ok();
    let render_table: Option<mlua::Table> = tbl.get("render").ok().flatten();
    let render_spec =
        render.or_else(|| render_table.and_then(|t| str_field(&t, "backend", "").into()));
    if let Some(spec) = render_spec {
        cfg.render = match spec.trim().to_ascii_lowercase().as_str() {
            "" => cfg.render,
            "cpu" | "software" => RenderMode::Cpu,
            "gpu" | "wgpu" | "dx12" => RenderMode::Gpu,
            "auto" => RenderMode::Auto,
            other => {
                // 退回"当前默认",而不是写死 auto —— 默认已经是 CPU,
                // 写错一个词不该把用户悄悄切到会抖的 GPU 路径上。
                crate::wlog!(
                    "配置:未知的 render = {other:?}(可选 auto / cpu / gpu),保持默认 {}",
                    cfg.render.as_str()
                );
                cfg.render
            }
        };
    }
    // gpu_power = "low" | "high":双显卡机器上优先核显还是独显
    if let Ok(Some(spec)) = tbl.get::<Option<String>>("gpu_power") {
        cfg.gpu_power = match spec.trim().to_ascii_lowercase().as_str() {
            "" => cfg.gpu_power,
            "high" | "performance" | "discrete" => GpuPower::High,
            "low" | "power" | "integrated" => GpuPower::Low,
            other => {
                crate::wlog!("配置:未知的 gpu_power = {other:?}(可选 low / high),按 low 处理");
                GpuPower::Low
            }
        };
    }
    // selection = { copy_on_select, background, opacity, ctrl_shift_v }
    let selection: Option<mlua::Table> = tbl.get("selection").ok();
    let mut selection_bg_explicit = false;
    if let Some(sl) = selection {
        let copy_on_select: Option<bool> = sl.get("copy_on_select").ok();
        if let Some(b) = copy_on_select {
            cfg.selection.copy_on_select = b;
        }
        let edge_scroll: Option<bool> = sl.get("edge_scroll").ok();
        if let Some(b) = edge_scroll {
            cfg.selection.edge_scroll = b;
        }
        if sl
            .get::<Option<String>>("background")
            .ok()
            .flatten()
            .is_some()
        {
            selection_bg_explicit = true;
        }
        cfg.selection.background = str_field(&sl, "background", &cfg.selection.background);
        let opacity: Option<f64> = sl.get("opacity").ok();
        if let Some(o) = opacity {
            cfg.selection.opacity = (o as f32).clamp(0.05, 1.0);
        }
        let v_key: Option<String> = sl.get("ctrl_shift_v").ok();
        if let Some(v) = v_key {
            cfg.selection.ctrl_shift_v = match v.trim().to_ascii_lowercase().as_str() {
                "copy" => VKeyAction::Copy,
                "paste" => VKeyAction::Paste,
                other => {
                    crate::wlog!("配置:未知的 selection.ctrl_shift_v = {other:?}(可选 paste / copy),按 paste 处理");
                    VKeyAction::Paste
                }
            };
        }
    }
    cfg.padding = f64_field(&tbl, "padding", cfg.padding);
    cfg.scrollback_lines = usize_field(&tbl, "scrollback_lines", cfg.scrollback_lines);
    // 0 = 实时同步;上限 1000ms,防止手滑写成一个让窗口"卡住"的值
    cfg.resize_debounce_ms =
        usize_field(&tbl, "resize_debounce_ms", cfg.resize_debounce_ms as usize).min(1000) as u64;
    // 拖动期间的行列数策略。只认已知值,写错就保留默认(realtime),
    // 免得一个拼写错误让窗口"拖着不动"。支持热重载,读的是 self.cfg。
    if let Ok(Some(v)) = tbl.get::<Option<String>>("live_resize_mode") {
        if let Some(mode) = LiveResizeMode::parse(&v) {
            cfg.live_resize_mode = mode;
        } else {
            crate::wlog!(
                "[VectorTerm] 未知的 live_resize_mode: {v:?}(可用 realtime / debounce / freeze),沿用 {}",
                cfg.live_resize_mode.as_str()
            );
        }
    }
    if let Ok(Some(v)) = tbl.get::<Option<bool>>("use_resize_increments") {
        cfg.use_resize_increments = v;
    }

    let colors: Option<mlua::Table> = tbl.get("colors").ok();
    if let Some(c) = colors {
        cfg.colors.foreground = str_field(&c, "foreground", &cfg.colors.foreground);
        cfg.colors.background = str_field(&c, "background", &cfg.colors.background);
        cfg.colors.cursor_fg = str_field(&c, "cursor_fg", &cfg.colors.cursor_fg);
        cfg.colors.cursor_bg = str_field(&c, "cursor_bg", &cfg.colors.cursor_bg);

        // ansi = { 8 个基础色 } + brights = { 8 个亮色 } —— wezterm 分成两张表,
        // VectorTerm 内部用一张 16 色表,这里拼起来。
        let ansi: Option<Vec<String>> = c.get("ansi").ok();
        let brights: Option<Vec<String>> = c.get("brights").ok();
        if ansi.is_some() || brights.is_some() {
            let mut list = ansi.unwrap_or_default();
            if let Some(b) = brights {
                list.extend(b);
            }
            cfg.colors.ansi = list;
        }
        // indexed = { [196] = "#ff0000", ... } —— 覆盖 256 色表里的任意下标
        let indexed: Option<mlua::Table> = c.get("indexed").ok();
        if let Some(t) = indexed {
            for (k, v) in t.pairs::<i64, String>().flatten() {
                if (0..256).contains(&k) {
                    cfg.colors.indexed.push((k as u8, v));
                }
            }
        }
        // wezterm 的 selection_bg / selection_fg:VectorTerm 只画底色,
        // 用不透明色更接近原意(原生 selection 表优先,稍后不再覆盖)。
        if !selection_bg_explicit {
            if let Some(bg) = opt_str(&c, "selection_bg") {
                cfg.selection.background = bg;
                cfg.selection.opacity = 1.0;
            }
        }
        // colors.tab_bar —— 与 wezterm 同名同结构
        if let Some(tb) = c.get::<Option<mlua::Table>>("tab_bar").ok().flatten() {
            parse_tab_bar_colors(&tb, &mut cfg.tab_bar.colors);
        }
    }

    // wezterm 顶层标签栏开关(原生 `tab_bar = { ... }` 稍后覆盖)
    if let Ok(Some(v)) = tbl.get::<Option<bool>>("enable_tab_bar") {
        cfg.tab_bar.enabled = v;
    }
    if let Ok(Some(v)) = tbl.get::<Option<bool>>("tab_bar_at_bottom") {
        cfg.tab_bar.at_bottom = v;
    }
    if let Ok(Some(v)) = tbl.get::<Option<bool>>("use_fancy_tab_bar") {
        cfg.tab_bar.fancy = v;
    }
    if let Ok(Some(v)) = tbl.get::<Option<bool>>("hide_tab_bar_if_only_one_tab") {
        cfg.tab_bar.hide_if_one = v;
    }
    if let Ok(Some(v)) = tbl.get::<Option<usize>>("tab_max_width") {
        cfg.tab_bar.max_width_cells = v.clamp(3, 128);
    }
    if let Ok(Some(v)) = tbl.get::<Option<bool>>("show_tab_index_in_tab_bar") {
        cfg.tab_bar.show_index = v;
    }
    if let Ok(Some(v)) = tbl.get::<Option<bool>>("show_new_tab_button_in_tab_bar") {
        cfg.tab_bar.show_new_tab_button = v;
    }
    // wezterm 的 `show_close_tab_button_in_tabs`:与其对应的本地键是
    // `tab_bar.show_close_button`,这里补上 wezterm 那边的写法
    if let Ok(Some(v)) = tbl.get::<Option<bool>>("show_close_tab_button_in_tabs") {
        cfg.tab_bar.show_close_button = v;
    }
    // 状态栏刷新间隔(毫秒)。太小会让 Lua 回调把 CPU 吃掉,钳一个下限
    if let Ok(Some(v)) = tbl.get::<Option<u64>>("status_update_interval") {
        cfg.status_update_interval = v.clamp(50, 60_000);
    }
    // 状态栏外观 + 自定义模块(第 8.7 节)
    if let Some(t) = tbl.get::<Option<mlua::Table>>("status_bar").ok().flatten() {
        if let Ok(Some(v)) = t.get::<Option<bool>>("enable") {
            cfg.status_bar.enabled = v;
        }
        if let Ok(Some(v)) = t.get::<Option<bool>>("keep_script") {
            cfg.status_bar.keep_script = v;
        }
        if let Some(v) = opt_f64_field(&t, "font_scale") {
            cfg.status_bar.font_scale = v.clamp(0.5, 2.0);
        }
        // 小于 12px 的高度没有意义(连一行字都放不下),当作没写
        if let Some(v) = opt_f64_field(&t, "height").filter(|v| *v >= 12.0) {
            cfg.status_bar.height = Some(v);
        }
        if let Some(v) = opt_f64_field(&t, "padding") {
            cfg.status_bar.padding = v.clamp(0.0, 200.0);
        }
        if let Some(v) = opt_f64_field(&t, "item_spacing") {
            cfg.status_bar.item_spacing = v.clamp(0.0, 200.0);
        }
        // 空格宽度倍数:0 = 空格不占位置(分隔带收到最紧),1 = 一个字宽
        if let Some(v) = opt_f64_field(&t, "space_width") {
            cfg.status_bar.space_width = v.clamp(0.0, 2.0);
        }
        if let Ok(Some(v)) = t.get::<Option<String>>("text") {
            cfg.status_bar.text = v;
        }
    }
    // VectorTerm 扩展:标签栏四周的安全区。wezterm 没有这两个键,不影响
    // wezterm 配置的解析。None = 自动(无边框 + 圆角时按 DPI 留一圈),
    // 显式 0 = 贴边,正数 = 逻辑像素。
    if let Some(v) = opt_f64_field(&tbl, "tab_bar_edge_padding") {
        cfg.tab_bar.edge_padding = Some(v);
    }
    if let Some(v) = opt_f64_field(&tbl, "tab_bar_top_padding") {
        cfg.tab_bar.top_padding = Some(v);
    }
    // VectorTerm 原生写法:tab_bar = { enable=..., at_bottom=..., colors=... }
    if let Some(t) = tbl.get::<Option<mlua::Table>>("tab_bar").ok().flatten() {
        if let Ok(Some(v)) = t.get::<Option<bool>>("enable") {
            cfg.tab_bar.enabled = v;
        }
        if let Ok(Some(v)) = t.get::<Option<bool>>("at_bottom") {
            cfg.tab_bar.at_bottom = v;
        }
        if let Ok(Some(v)) = t.get::<Option<bool>>("fancy") {
            cfg.tab_bar.fancy = v;
        }
        if let Ok(Some(v)) = t.get::<Option<bool>>("hide_if_one") {
            cfg.tab_bar.hide_if_one = v;
        }
        if let Ok(Some(v)) = t.get::<Option<usize>>("max_width") {
            cfg.tab_bar.max_width_cells = v.clamp(3, 128);
        }
        if let Ok(Some(v)) = t.get::<Option<bool>>("show_index") {
            cfg.tab_bar.show_index = v;
        }
        if let Ok(Some(v)) = t.get::<Option<bool>>("show_new_tab_button") {
            cfg.tab_bar.show_new_tab_button = v;
        }
        if let Ok(Some(v)) = t.get::<Option<bool>>("show_close_button") {
            cfg.tab_bar.show_close_button = v;
        }
        // 原生表内写法优先于顶层别名
        if let Some(v) = opt_f64_field(&t, "edge_padding") {
            cfg.tab_bar.edge_padding = Some(v);
        }
        if let Some(v) = opt_f64_field(&t, "top_padding") {
            cfg.tab_bar.top_padding = Some(v);
        }
        // corner_radius:数字(像素);0 = 直角,负数/缺省 = 自动
        let cr: Value = t.get("corner_radius").unwrap_or(Value::Nil);
        match cr {
            Value::Integer(i) => cfg.tab_bar.corner_radius = Some((i as f64).max(0.0)),
            Value::Number(n) => cfg.tab_bar.corner_radius = Some(n.max(0.0)),
            _ => {}
        }
        if let Some(c) = t.get::<Option<mlua::Table>>("colors").ok().flatten() {
            parse_tab_bar_colors(&c, &mut cfg.tab_bar.colors);
        }
    }

    // 图形协议(iTerm2 / Kitty / sixel)
    if let Some(t) = tbl.get::<Option<mlua::Table>>("images").ok().flatten() {
        if let Ok(Some(v)) = t.get::<Option<bool>>("enable") {
            cfg.images.enabled = v;
        }
        if let Ok(Some(v)) = t.get::<Option<usize>>("max_memory_mb") {
            cfg.images.max_memory_mb = v.clamp(8, 4096);
        }
    }

    let cursor: Option<mlua::Table> = tbl.get("cursor").ok();
    if let Some(c) = cursor {
        // trail:毫秒数;兼容旧配置的 true/false
        let v: Value = c.get("trail").unwrap_or(Value::Nil);
        match v {
            Value::Boolean(b) => cfg.cursor.trail_dwell_ms = if b { 40.0 } else { 0.0 },
            Value::Integer(i) => cfg.cursor.trail_dwell_ms = i as f32,
            Value::Number(n) => cfg.cursor.trail_dwell_ms = n as f32,
            _ => {}
        }

        // trail_start_threshold:数字 / "x y" / {x, y}
        let v: Value = c.get("trail_start_threshold").unwrap_or(Value::Nil);
        let (mut sx, mut sy) = (cfg.cursor.trail_start_x, cfg.cursor.trail_start_y);
        match v {
            Value::Integer(i) => {
                sx = i;
                sy = i;
            }
            Value::String(s) => {
                let text = s.to_string_lossy().to_string();
                let mut it = text.split_whitespace();
                if let Some(x) = it.next().and_then(|x| x.parse::<i64>().ok()) {
                    sx = x;
                    sy = it.next().and_then(|y| y.parse::<i64>().ok()).unwrap_or(x);
                }
            }
            Value::Table(t) => {
                let x: Option<i64> = t.get(1).ok();
                let y: Option<i64> = t.get(2).ok();
                if let Some(x) = x {
                    sx = x;
                    sy = y.unwrap_or(x);
                }
            }
            _ => {}
        }
        cfg.cursor.trail_start_x = sx.max(0);
        cfg.cursor.trail_start_y = sy.max(0);

        // trail_decay:数字 / "fast slow" / {fast, slow}
        let v: Value = c.get("trail_decay").unwrap_or(Value::Nil);
        let (mut fast, mut slow) = (cfg.cursor.trail_decay_fast, cfg.cursor.trail_decay_slow);
        match v {
            Value::Number(n) => {
                fast = n as f32;
                slow = n as f32;
            }
            Value::Integer(i) => {
                fast = i as f32;
                slow = i as f32;
            }
            Value::String(s) => {
                let text = s.to_string_lossy().to_string();
                let mut it = text.split_whitespace();
                if let Some(f) = it.next().and_then(|x| x.parse::<f32>().ok()) {
                    fast = f;
                    slow = it.next().and_then(|x| x.parse::<f32>().ok()).unwrap_or(f);
                }
            }
            Value::Table(t) => {
                let f: Option<f32> = t.get(1).ok();
                let s: Option<f32> = t.get(2).ok();
                if let Some(f) = f {
                    fast = f;
                    slow = s.unwrap_or(f);
                }
            }
            _ => {}
        }
        cfg.cursor.trail_decay_fast = fast.max(0.01);
        cfg.cursor.trail_decay_slow = slow.max(cfg.cursor.trail_decay_fast);

        // trail_max:单段动画的硬上限(毫秒);0 = 不限制
        let v: Value = c.get("trail_max").unwrap_or(Value::Nil);
        match v {
            Value::Integer(i) => cfg.cursor.trail_max_ms = (i as f32).max(0.0),
            Value::Number(n) => cfg.cursor.trail_max_ms = (n as f32).max(0.0),
            _ => {}
        }

        // trail_color:"none" / "#rrggbb"
        let v: Value = c.get("trail_color").unwrap_or(Value::Nil);
        if let Value::String(s) = v {
            let text = s.to_string_lossy().to_string();
            if !text.eq_ignore_ascii_case("none") && !text.is_empty() {
                cfg.cursor.trail_color = Some(text);
            }
        }

        // ── 光标本体:形状 / 尺寸 / 闪烁 / 常驻动画 ──────────────
        // 没写的键保持默认,所以旧配置(只有 trail_*)行为一字不变。
        if let Some(s) = opt_str(&c, "shape") {
            if let Some(v) = CursorShapeOverride::parse(&s) {
                cfg.cursor.shape = v;
            }
        }
        let num = |key: &str, d: f32| f64_field(&c, key, d as f64) as f32;
        cfg.cursor.width = num("width", cfg.cursor.width).clamp(0.1, 4.0);
        cfg.cursor.height = num("height", cfg.cursor.height).clamp(0.1, 4.0);
        cfg.cursor.thickness = num("thickness", cfg.cursor.thickness).clamp(0.0, 64.0);
        cfg.cursor.alpha = num("alpha", cfg.cursor.alpha).clamp(0.05, 1.0);
        if let Some(a) = parse_cursor_align(c.get("align_x").unwrap_or(Value::Nil)) {
            cfg.cursor.align_x = Some(a);
        }
        if let Some(a) = parse_cursor_align(c.get("align_y").unwrap_or(Value::Nil)) {
            cfg.cursor.align_y = Some(a);
        }

        match c.get("blink").unwrap_or(Value::Nil) {
            Value::Boolean(b) => {
                cfg.cursor.blink = if b {
                    CursorBlinkMode::Always
                } else {
                    CursorBlinkMode::Never
                }
            }
            Value::String(s) => {
                if let Some(m) = CursorBlinkMode::parse(&s.to_string_lossy()) {
                    cfg.cursor.blink = m;
                }
            }
            _ => {}
        }
        cfg.cursor.blink_rate_ms = num("blink_rate", cfg.cursor.blink_rate_ms)
            .clamp(60.0, 10_000.0);
        cfg.cursor.blink_duty = num("blink_duty", cfg.cursor.blink_duty)
            .clamp(0.05, 0.95);
        if let Some(s) = opt_str(&c, "blink_style") {
            if let Some(v) = CursorBlinkStyle::parse(&s) {
                cfg.cursor.blink_style = v;
            }
        }

        match c.get("anim").unwrap_or(Value::Nil) {
            Value::Boolean(false) => cfg.cursor.anim = CursorAnimation::None,
            Value::String(s) => {
                if let Some(v) = CursorAnimation::parse(&s.to_string_lossy()) {
                    cfg.cursor.anim = v;
                }
            }
            _ => {}
        }
        cfg.cursor.anim_ms = num("anim_ms", cfg.cursor.anim_ms).clamp(60.0, 10_000.0);
        cfg.cursor.anim_amount = num("anim_amount", cfg.cursor.anim_amount).clamp(0.0, 1.0);
    }

    // scroll = { anim = 90, max = 200, back = 90, back_burst = 40 }
    // 也接受 scroll = false 直接关掉平滑滚动(退回逐行瞬移)
    match tbl.get::<Value>("scroll").unwrap_or(Value::Nil) {
        Value::Boolean(false) => cfg.scroll.anim_ms = 0.0,
        Value::Table(t) => {
            let get = |key: &str, default: f32| -> f32 {
                match t.get::<Value>(key).unwrap_or(Value::Nil) {
                    Value::Integer(i) => i as f32,
                    Value::Number(n) => n as f32,
                    _ => default,
                }
            };
            cfg.scroll.anim_ms = get("anim", cfg.scroll.anim_ms).max(0.0);
            cfg.scroll.max_ms = get("max", cfg.scroll.max_ms).max(1.0);
            cfg.scroll.back_ms = get("back", cfg.scroll.back_ms).max(0.0);
            cfg.scroll.back_burst_ms = get("back_burst", cfg.scroll.back_burst_ms).max(0.0);
        }
        _ => {}
    }
    parse_scrollbar(&tbl, &mut cfg.scrollbar);
    parse_search(&tbl, &mut cfg.search);

    Ok(cfg)
}

/// `scrollbar = { ... }` / `scrollbar = false`。颜色写 `Auto` / `none` / 空串
/// 都表示由前后景色自动混合。
fn parse_scrollbar(tbl: &mlua::Table, b: &mut ScrollbarConfig) {
    let t = match tbl.get::<Value>("scrollbar").unwrap_or(Value::Nil) {
        Value::Boolean(false) => {
            b.enabled = false;
            return;
        }
        Value::Table(t) => t,
        _ => return,
    };
    if let Ok(Some(v)) = t.get::<Option<bool>>("enabled") {
        b.enabled = v;
    }
    if let Ok(Some(v)) = t.get::<Option<bool>>("auto_hide") {
        b.auto_hide = v;
        b.show = if v { ScrollbarShow::Auto } else { ScrollbarShow::Always };
    }
    // show 比 auto_hide 更细(always / auto / never),写了就以它为准
    if let Ok(Some(v)) = t.get::<Option<String>>("show") {
        if let Some(s) = ScrollbarShow::parse(&v) {
            b.show = s;
            b.auto_hide = s == ScrollbarShow::Auto;
        }
    }
    if let Ok(Some(v)) = t.get::<Option<String>>("side") {
        if let Some(s) = ScrollbarSide::parse(&v) {
            b.side = s;
        }
    }
    if let Ok(Some(v)) = t.get::<Option<bool>>("show_track") {
        b.show_track = v;
    }
    if let Ok(Some(v)) = t.get::<Option<f64>>("width") {
        b.width = v.clamp(2.0, 64.0);
    }
    if let Ok(Some(v)) = t.get::<Option<f64>>("min_thumb") {
        b.min_thumb = v.clamp(4.0, 400.0);
    }
    if let Ok(Some(v)) = t.get::<Option<f64>>("opacity") {
        b.opacity = v.clamp(0.0, 1.0) as f32;
    }
    let color = |key: &str| -> Option<Option<String>> {
        match t.get::<Option<String>>(key) {
            Ok(Some(v)) => {
                let v = v.trim().to_string();
                if v.is_empty() || v.eq_ignore_ascii_case("auto") || v.eq_ignore_ascii_case("none") {
                    Some(None)
                } else {
                    Some(Some(v))
                }
            }
            _ => None,
        }
    };
    if let Some(v) = color("track_color") {
        b.track_color = v;
    }
    if let Some(v) = color("thumb_color") {
        b.thumb_color = v;
    }
    if let Some(v) = color("hover_color") {
        b.hover_color = v;
    }
}

/// `search = { ... }` / `search = false`。
fn parse_search(tbl: &mlua::Table, s: &mut SearchConfig) {
    let t = match tbl.get::<Value>("search").unwrap_or(Value::Nil) {
        Value::Boolean(false) => {
            s.enabled = false;
            return;
        }
        Value::Table(t) => t,
        _ => return,
    };
    if let Ok(Some(v)) = t.get::<Option<bool>>("enabled") {
        s.enabled = v;
    }
    if let Ok(Some(v)) = t.get::<Option<bool>>("case_sensitive") {
        s.case_sensitive = v;
    }
    if let Ok(Some(v)) = t.get::<Option<f64>>("max_matches") {
        s.max_matches = (v.round().max(1.0) as usize).min(200_000);
    }
    if let Ok(Some(v)) = t.get::<Option<String>>("highlight_color") {
        if !v.trim().is_empty() {
            s.highlight_color = v.trim().to_string();
        }
    }
    if let Ok(Some(v)) = t.get::<Option<String>>("current_color") {
        if !v.trim().is_empty() {
            s.current_color = v.trim().to_string();
        }
    }
}

/// 读一个标签配色子表(wezterm 的 `active_tab = { bg_color, fg_color, ... }`),
/// 同时接受 VectorTerm 简写的 `bg` / `fg`。
fn read_tab_color(t: &mlua::Table, key: &str, default: &TabColor) -> TabColor {
    let Some(c) = t.get::<Option<mlua::Table>>(key).ok().flatten() else {
        return default.clone();
    };
    TabColor {
        bg: opt_str(&c, "bg_color")
            .or_else(|| opt_str(&c, "bg"))
            .unwrap_or_else(|| default.bg.clone()),
        fg: opt_str(&c, "fg_color")
            .or_else(|| opt_str(&c, "fg"))
            .unwrap_or_else(|| default.fg.clone()),
    }
}

fn parse_tab_bar_colors(t: &mlua::Table, out: &mut TabBarColors) {
    out.background = str_field(t, "background", &out.background);
    out.active = read_tab_color(t, "active_tab", &out.active);
    out.inactive = read_tab_color(t, "inactive_tab", &out.inactive);
    out.inactive_hover = read_tab_color(t, "inactive_tab_hover", &out.inactive_hover);
    out.new_tab = read_tab_color(t, "new_tab", &out.new_tab);
    out.new_tab_hover = read_tab_color(t, "new_tab_hover", &out.new_tab_hover);
    if let Some(v) = opt_str(t, "inactive_tab_edge") {
        out.edge = v;
    }
}

/// 解析字体配置。同时兼容三种写法:
///
/// ```lua
/// font = "C:/Windows/Fonts/consola.ttf"        -- 旧写法:文件路径
/// font = { family = "Consolas", size = 12 }    -- 推荐写法
/// font = { "JetBrainsMono Nerd Font", "Microsoft YaHei" } -- 第一项主字体,其余回退
/// ```
fn parse_fonts(tbl: &mlua::Table, cfg: &mut crate::config::FontsConfig) {
    let mut size_from_table = false;
    let mut fallback_from_table = false;

    match tbl.get::<Value>("font").unwrap_or(Value::Nil) {
        Value::String(s) => cfg.family = s.to_string_lossy().to_string(),
        Value::Table(t) => {
            // 数组写法:{ "主字体", "回退1", "回退2" }
            if let Ok(Some(first)) = t.get::<Option<String>>(1) {
                cfg.family = first;
                let rest: Vec<String> = str_list(&t).into_iter().skip(1).collect();
                if !rest.is_empty() {
                    cfg.fallback = rest;
                    fallback_from_table = true;
                }
            } else {
                cfg.family = str_field(&t, "family", &cfg.family);
                if let Ok(Some(list)) = t.get::<Option<Vec<String>>>("fallback") {
                    cfg.fallback = list;
                    fallback_from_table = true;
                }
            }

            if let Ok(Some(v)) = t.get::<Option<f64>>("size") {
                cfg.size = v as f32;
                size_from_table = true;
            } else if let Ok(Some(v)) = t.get::<Option<f64>>("font_size") {
                cfg.size = v as f32;
                size_from_table = true;
            }
            if let Ok(Some(v)) = t.get::<Option<f64>>("line_height") {
                cfg.line_height = Some(v);
            }
            if let Ok(Some(v)) = t.get::<Option<f64>>("cell_width") {
                cfg.cell_width = v;
            }
            if let Some(s) = opt_str(&t, "bold") {
                cfg.bold = Some(s);
            }
            if let Some(s) = opt_str(&t, "italic") {
                cfg.italic = Some(s);
            }
            if let Some(s) = opt_str(&t, "bold_italic") {
                cfg.bold_italic = Some(s);
            }
            if let Ok(Some(dirs)) = t.get::<Option<Vec<String>>>("dirs") {
                cfg.dirs = dirs.into_iter().map(PathBuf::from).collect();
            }

            match t.get::<Value>("nerd_font").unwrap_or(Value::Nil) {
                Value::Boolean(true) => cfg.nerd_font = crate::config::NerdFontMode::Auto,
                Value::Boolean(false) => cfg.nerd_font = crate::config::NerdFontMode::Off,
                Value::String(s) => {
                    let text = s.to_string_lossy().to_string();
                    let lower = text.trim().to_ascii_lowercase();
                    cfg.nerd_font = match lower.as_str() {
                        "" | "auto" => crate::config::NerdFontMode::Auto,
                        "off" | "none" | "false" => crate::config::NerdFontMode::Off,
                        _ => crate::config::NerdFontMode::Family(text.trim().to_string()),
                    };
                }
                _ => {}
            }
        }
        _ => {}
    }

    // 旧版 / wezterm 风格的顶层字段,优先级低于 font 表内的显式设置
    if let Ok(Some(list)) = tbl.get::<Option<Vec<String>>>("font_fallback") {
        if !fallback_from_table {
            cfg.fallback = list;
        }
    }
    if let Ok(Some(v)) = tbl.get::<Option<f64>>("font_size") {
        if !size_from_table {
            cfg.size = v as f32;
        }
    }
    if let Ok(Some(v)) = tbl.get::<Option<f64>>("line_height") {
        cfg.line_height = Some(v);
    }
    if let Ok(Some(v)) = tbl.get::<Option<f64>>("cell_width") {
        cfg.cell_width = v;
    }

    // 兜底:字号/行高/格宽落在合理区间,避免配置写错导致网格为 0
    cfg.size = cfg.size.clamp(6.0, 96.0);
    cfg.line_height = cfg.line_height.map(|v| v.clamp(0.6, 3.0));
    cfg.cell_width = cfg.cell_width.clamp(0.5, 3.0);
}

/// 自定义背景。字段名对齐 wezterm,直接搬配置也能用。
/// 从 Lua 表里按顺序取出字符串数组(`{ "#111", "#222" }`)。
fn string_array(t: &mlua::Table) -> Vec<String> {
    t.sequence_values::<String>()
        .filter_map(|v| v.ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// `orientation` 既接受 "Auto" / "Vertical" / "Horizontal",
/// 也接受 wezterm 的 `{ Linear = { angle = 90.0 } }`(90° 视作竖向)。
fn parse_gradient_orientation(v: Value) -> Option<DividerGradientDir> {
    match v {
        Value::String(s) => {
            DividerGradientDir::parse(&s.to_str().map(|b| b.to_string()).unwrap_or_default())
        }
        Value::Table(t) => {
            let angle = t
                .get::<Option<mlua::Table>>("Linear")
                .ok()
                .flatten()
                .and_then(|l| l.get::<Option<f64>>("angle").ok().flatten())
                .unwrap_or(90.0);
            // wezterm 的 angle:0 = 从左到右,90 = 从上到下
            Some(if (angle - 90.0).abs() <= 45.0 {
                DividerGradientDir::Vertical
            } else {
                DividerGradientDir::Horizontal
            })
        }
        _ => None,
    }
}

/// 解析 `split_divider = { ... }`。
///
/// 三种颜色写法都认,优先级 渐变 > 单色:
///   color = "#323236"                         单色
///   color = { "#ff5f6d", "#ffc371" }          渐变(沿分割线)
///   gradient = { colors = {...}, orientation = "Vertical" }
///   gradient = { "#ff5f6d", "#ffc371" }       渐变简写
fn parse_split_divider(tbl: &mlua::Table, d: &mut SplitDividerConfig) {
    let Ok(Some(t)) = tbl.get::<Option<mlua::Table>>("split_divider") else {
        return;
    };

    if let Ok(Some(v)) = t.get::<Option<bool>>("enabled") {
        d.enabled = v;
    }
    // thickness 写成 nil / "auto" 都表示"填满整格缝"
    match t.get::<Option<Value>>("thickness") {
        Ok(Some(Value::Integer(i))) => d.thickness = Some((i as f64).clamp(0.0, 256.0)),
        Ok(Some(Value::Number(n))) => d.thickness = Some(n.clamp(0.0, 256.0)),
        Ok(Some(Value::Nil)) | Ok(None) => {}
        _ => d.thickness = None,
    }
    if let Ok(Some(v)) = t.get::<Option<f64>>("opacity") {
        d.opacity = v.clamp(0.0, 1.0) as f32;
    }
    // 分割栏交互:鼠标拖动 + 快捷键调整窗格大小
    if let Ok(Some(v)) = t.get::<Option<bool>>("draggable") {
        d.draggable = v;
    }
    if let Ok(Some(v)) = t.get::<Option<f64>>("hit_width") {
        d.hit_width = v.clamp(2.0, 32.0);
    }
    if let Ok(Some(v)) = t.get::<Option<f64>>("min_ratio") {
        d.min_ratio = v.clamp(0.02, 0.45);
    }
    if let Ok(Some(v)) = t.get::<Option<f64>>("step") {
        d.step = v.clamp(0.005, 0.3);
    }
    // inset = 12 是两端相同的简写;inset_start / inset_end 随后可分别覆盖
    if let Ok(Some(v)) = t.get::<Option<f64>>("inset") {
        let v = v.max(0.0);
        d.inset_start = v;
        d.inset_end = v;
    }
    // gap = 两侧文字之间的缝宽;text_gap = 文字到线边缘的距离(优先级更高)。
    // 两者都支持 "Auto" / nil 表示自适应,也可直接写 px 数值。
    for (key, slot) in [("gap", &mut d.gap), ("text_gap", &mut d.text_gap)] {
        match t.get::<Option<Value>>(key) {
            Ok(Some(Value::Integer(i))) => *slot = Some((i as f64).clamp(0.0, 256.0)),
            Ok(Some(Value::Number(n))) => *slot = Some(n.clamp(0.0, 256.0)),
            // "Auto" / "auto" / nil 都是自适应
            Ok(Some(Value::String(s))) => {
                let s = s.to_str().map(|b| b.to_string()).unwrap_or_default();
                if s.trim().eq_ignore_ascii_case("auto") {
                    *slot = None;
                } else if let Ok(n) = s.trim().parse::<f64>() {
                    *slot = Some(n.clamp(0.0, 256.0));
                }
            }
            Ok(Some(Value::Nil)) | Ok(None) => *slot = None,
            _ => {}
        }
    }
    for (key, slot) in [
        ("inset_start", &mut d.inset_start),
        ("inset_end", &mut d.inset_end),
    ] {
        if let Ok(Some(v)) = t.get::<Option<f64>>(key) {
            *slot = v.max(0.0);
        }
    }

    match t.get::<Option<Value>>("color") {
        Ok(Some(Value::String(s))) => {
            let s = s.to_str().map(|b| b.to_string()).unwrap_or_default();
            if !s.trim().is_empty() {
                d.color = s.trim().to_string();
            }
        }
        // color 直接给数组 = 沿线渐变
        Ok(Some(Value::Table(colors))) => {
            let list = string_array(&colors);
            if list.len() >= 2 {
                d.gradient = Some(DividerGradient {
                    colors: list,
                    orientation: DividerGradientDir::Auto,
                });
            }
        }
        _ => {}
    }

    if let Ok(Some(g)) = t.get::<Option<mlua::Table>>("gradient") {
        // 简写:gradient = { "#a", "#b" };完整:gradient = { colors = {...}, orientation = ... }
        let inline = string_array(&g);
        let colors = if inline.len() >= 2 {
            inline
        } else {
            g.get::<Option<mlua::Table>>("colors")
                .ok()
                .flatten()
                .map(|c| string_array(&c))
                .unwrap_or_default()
        };
        if colors.len() >= 2 {
            let orientation = g
                .get::<Option<Value>>("orientation")
                .ok()
                .flatten()
                .and_then(parse_gradient_orientation)
                .unwrap_or(DividerGradientDir::Auto);
            d.gradient = Some(DividerGradient { colors, orientation });
        }
    }
}

/// `files_gui = { ... }`:文件浏览器的编辑器与侧边栏参数。
///
/// 编辑器可以写字符串(`editor = "nvim"`)也可以写数组
/// (`editor = { "nvim", "-p" }`),两种都按"程序 + 固定参数"理解。
fn parse_files_gui(tbl: &mlua::Table, f: &mut FilesGuiConfig) {
    let Ok(Some(t)) = tbl.get::<Option<mlua::Table>>("files_gui") else {
        return;
    };
    match t.get::<Option<Value>>("editor") {
        Ok(Some(Value::String(s))) => {
            let s = s.to_string_lossy().trim().to_string();
            f.editor = if s.is_empty() { Vec::new() } else { vec![s] };
        }
        Ok(Some(Value::Table(arr))) => {
            f.editor = string_array(&arr)
                .into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
        _ => {}
    }
    if let Ok(Some(v)) = t.get::<Option<f64>>("side_cols") {
        f.side_cols = (v.round().max(0.0) as usize).clamp(8, 120);
    }
    if let Ok(Some(v)) = t.get::<Option<bool>>("show_hidden") {
        f.show_hidden = v;
    }
    for (key, slot) in [("follow_cwd", &mut f.follow_cwd), ("push_cwd", &mut f.push_cwd)] {
        if let Ok(Some(v)) = t.get::<Option<bool>>(key) {
            *slot = v;
        }
    }
    // open_mode 认不出来就保留默认(新窗口),不因为一个拼写错误改变行为
    if let Ok(Some(v)) = t.get::<Option<String>>("open_mode") {
        if let Some(m) = FileOpenMode::parse(&v) {
            f.open_mode = m;
        }
    }
}

/// `pane_focus = { ... }`:焦点交互与输入光标飞行动画。
fn parse_pane_focus(tbl: &mlua::Table, f: &mut PaneFocusConfig) {
    let Ok(Some(t)) = tbl.get::<Option<mlua::Table>>("pane_focus") else {
        return;
    };
    for (key, slot) in [
        ("click_to_focus", &mut f.click_to_focus),
        ("hide_inactive_caret", &mut f.hide_inactive_caret),
        ("trail", &mut f.trail),
        ("sync_trail", &mut f.sync_trail),
        ("number_shortcuts", &mut f.number_shortcuts),
    ] {
        if let Ok(Some(v)) = t.get::<Option<bool>>(key) {
            *slot = v;
        }
    }
    if let Ok(Some(v)) = t.get::<Option<f64>>("trail_ms") {
        f.trail_ms = v.clamp(0.0, 5000.0);
    }
    // 颜色写成 "none" / "Auto" / 空串 = 跟随终端光标色
    if let Ok(Some(v)) = t.get::<Option<String>>("trail_color") {
        let v = v.trim().to_string();
        f.trail_color = if v.is_empty()
            || v.eq_ignore_ascii_case("auto")
            || v.eq_ignore_ascii_case("none")
        {
            None
        } else {
            Some(v)
        };
    }
    // 落点:默认停在新 pane 的真实输入光标处,也可只飞到 pane 中心
    if let Ok(Some(v)) = t.get::<Option<String>>("landing") {
        let v = v.trim();
        if v.eq_ignore_ascii_case("center") {
            f.landing = CaretLanding::Center;
        } else if !v.is_empty() {
            f.landing = CaretLanding::Caret;
        }
    }
}

fn parse_background(tbl: &mlua::Table, bg: &mut BackgroundConfig) {
    if let Ok(Some(v)) = tbl.get::<Option<String>>("window_background_color") {
        if !v.trim().is_empty() {
            bg.color = Some(v);
        }
    }
    if let Ok(Some(v)) = tbl.get::<Option<String>>("window_background_image") {
        let v = v.trim();
        if !v.is_empty() && v != "nil" {
            bg.image = Some(PathBuf::from(v));
        }
    }
    // window_background_image_hsb = { brightness = 0.3, hue = 1.0, saturation = 1.0 }
    if let Ok(Some(hsb)) = tbl.get::<Option<mlua::Table>>("window_background_image_hsb") {
        if let Ok(Some(b)) = hsb.get::<Option<f64>>("brightness") {
            // 负亮度没有意义,但允许 0(纯黑)
            bg.brightness = b.max(0.0) as f32;
        }
        if let Ok(Some(h)) = hsb.get::<Option<f64>>("hue") {
            bg.hue = h as f32;
        }
        if let Ok(Some(s)) = hsb.get::<Option<f64>>("saturation") {
            bg.saturation = s.max(0.0) as f32;
        }
    }
    // 模糊半径:wezterm 没有这个项(它在 macOS 上是固定背景模糊),这里自己起名
    for key in ["window_background_blur", "window_background_image_blur"] {
        if let Ok(Some(v)) = tbl.get::<Option<f64>>(key) {
            bg.blur = v.clamp(0.0, 200.0) as f32;
        }
    }
    if let Ok(Some(v)) = tbl.get::<Option<String>>("window_background_fill") {
        match ImageFill::parse(&v) {
            Some(f) => bg.fill = f,
            None => crate::wlog!(
                "配置:未知的 window_background_fill = {v:?}\
                 (可选 cover / contain / stretch / tile / center),保持 cover"
            ),
        }
    }
    // window_background_opacity = 0.85
    if let Ok(Some(v)) = tbl.get::<Option<f64>>("window_background_opacity") {
        bg.opacity = Some(v.clamp(0.0, 1.0) as f32);
    }
    // win32_system_backdrop = "Acrylic" (wezterm 同名项)
    if let Ok(Some(v)) = tbl.get::<Option<String>>("win32_system_backdrop") {
        match Backdrop::parse(&v) {
            Some(b) => bg.backdrop = b,
            None => crate::wlog!(
                "配置:未知的 win32_system_backdrop = {v:?}\
                 (可选 none / mica / acrylic / tabbed / auto),按 none 处理"
            ),
        }
    }
    // window_background_gradient = { colors = { "#1a1a1a", "#333" }, orientation = "Vertical" }
    if let Ok(Some(g)) = tbl.get::<Option<mlua::Table>>("window_background_gradient") {
        let colors: Vec<String> = g
            .get::<Option<mlua::Table>>("colors")
            .ok()
            .flatten()
            .map(|t| {
                t.sequence_values::<String>()
                    .filter_map(|v| v.ok())
                    .collect()
            })
            .unwrap_or_default();
        if colors.len() >= 2 {
            let vertical = match g.get::<Option<Value>>("orientation") {
                Ok(Some(Value::String(s))) => {
                    let text = s.to_str().map(|b| b.to_string()).unwrap_or_default();
                    !text.eq_ignore_ascii_case("horizontal")
                }
                Ok(Some(Value::Table(t))) => {
                    // wezterm 的 { Linear = { angle = 90.0 } }:90° 视为竖向
                    let angle = t
                        .get::<Option<mlua::Table>>("Linear")
                        .ok()
                        .flatten()
                        .and_then(|l| l.get::<Option<f64>>("angle").ok().flatten())
                        .unwrap_or(90.0);
                    (angle - 90.0).abs() < 45.0
                }
                _ => true,
            };
            bg.gradient = Some(Gradient {
                start: colors[0].clone(),
                end: colors[1].clone(),
                vertical,
            });
        }
    }
}

/// 图片路径按配置文件所在目录解析,和 font 目录的处理一致。
fn resolve_background_path(cfg: &mut AppConfig, cfg_path: &Path) {
    let Some(img) = &cfg.background.image else {
        return;
    };
    if img.is_absolute() {
        return;
    }
    if let Some(dir) = cfg_path.parent() {
        cfg.background.image = Some(dir.join(img));
    }
}

/// wezterm 的 `integrated_title_*` 四件套。只在 `INTEGRATED_BUTTONS` 下影响画面,
/// 但无论窗口装饰怎么配都先解析好,改配置时不用重启两遍。
fn parse_integrated_buttons(tbl: &mlua::Table, w: &mut WindowDecorations) {
    // integrated_title_buttons = { 'Hide', 'Maximize', 'Close' }
    // 空表 / false = 三枚全隐藏;顺序即从左到右的绘制顺序
    match tbl
        .get::<Value>("integrated_title_buttons")
        .unwrap_or(Value::Nil)
    {
        Value::Boolean(false) => w.buttons.clear(),
        Value::Table(t) => {
            let mut list = Vec::new();
            for v in t.sequence_values::<String>() {
                let Ok(s) = v else { break };
                match TitleButton::parse(&s) {
                    Some(b) => {
                        if !list.contains(&b) {
                            list.push(b);
                        }
                    }
                    None => crate::wlog!("配置:未知的 integrated_title_buttons 项 {s:?},已忽略"),
                }
            }
            w.buttons = list;
        }
        _ => {}
    }

    if let Ok(Some(v)) = tbl.get::<Option<String>>("integrated_title_button_alignment") {
        w.alignment = match v.trim().to_ascii_lowercase().as_str() {
            "left" => ButtonAlignment::Left,
            "right" => ButtonAlignment::Right,
            other => {
                crate::wlog!("配置:未知的 integrated_title_button_alignment = {other:?}(可选 Left / Right),按 Right 处理");
                ButtonAlignment::Right
            }
        };
    }

    if let Ok(Some(v)) = tbl.get::<Option<String>>("integrated_title_button_style") {
        w.style = match v.trim().to_ascii_lowercase().as_str() {
            "gnome" | "adwaita" => ButtonStyle::Gnome,
            // MacOsNative 只在 macOS 有意义,Windows 上退回方形图标
            "windows" | "macosnative" => ButtonStyle::Windows,
            other => {
                crate::wlog!("配置:未知的 integrated_title_button_style = {other:?}(可选 Windows / Gnome),按 Windows 处理");
                ButtonStyle::Windows
            }
        };
    }

    if let Ok(Some(v)) = tbl.get::<Option<String>>("integrated_title_button_color") {
        let text = v.trim();
        w.color = if text.is_empty() || text.eq_ignore_ascii_case("auto") {
            None
        } else {
            Some(text.to_string())
        };
    }
}

fn str_list(t: &mlua::Table) -> Vec<String> {
    let mut out = Vec::new();
    for s in t.sequence_values::<String>().flatten() {
        if !s.trim().is_empty() {
            out.push(s);
        }
    }
    out
}

fn opt_str(t: &mlua::Table, key: &str) -> Option<String> {
    t.get::<Option<String>>(key)
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty())
}

/// 光标锚点:数字(0..1)或 left/center/right、top/center/bottom。
/// 没写 / 不认识的值返回 None,交给渲染层按形状取默认位置。
fn parse_cursor_align(v: Value) -> Option<f32> {
    match v {
        Value::Integer(i) => Some((i as f32).clamp(0.0, 1.0)),
        Value::Number(n) => Some((n as f32).clamp(0.0, 1.0)),
        Value::String(s) => match s.to_string_lossy().trim().to_ascii_lowercase().as_str() {
            "left" | "top" | "start" => Some(0.0),
            "center" | "middle" => Some(0.5),
            "right" | "bottom" | "end" => Some(1.0),
            // 配置管理器里「跟随形状」这一项写的是 default,显式认下来
            "default" | "auto" | "none" => None,
            _ => None,
        },
        _ => None,
    }
}

fn str_field(t: &mlua::Table, key: &str, default: &str) -> String {
    let v: Option<String> = t.get(key).ok();
    v.unwrap_or_else(|| default.to_string())
}

fn f64_field(t: &mlua::Table, key: &str, default: f64) -> f64 {
    let v: Option<f64> = t.get(key).ok();
    v.unwrap_or(default)
}

fn opt_f64_field(t: &mlua::Table, key: &str) -> Option<f64> {
    t.get::<Option<f64>>(key).ok().flatten().map(|v| v.max(0.0))
}

fn usize_field(t: &mlua::Table, key: &str, default: usize) -> usize {
    let v: Option<usize> = t.get(key).ok();
    v.unwrap_or(default)
}

// ---------------------------------------------------------------------------
// 单元测试:Lua 配置项解析(shell / 参数 / 环境变量 / 工作目录)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(src: &str) -> AppConfig {
        parse(src).expect("Lua 配置应能解析")
    }

    // --- 光标本体(形状 / 尺寸 / 闪烁 / 动画) ---------------------------
    /// 旧配置只写 trail_* 时,新字段必须全部保持「整格方块、不闪、不动」。
    #[test]
    fn cursor_visual_defaults_keep_legacy_behavior() {
        let c = parse_str("return { cursor = { trail = 10, trail_color = 'none' } }");
        let k = &c.cursor;
        assert_eq!(k.trail_dwell_ms, 10.0);
        assert_eq!(k.shape, CursorShapeOverride::Auto);
        assert_eq!(k.width, 1.0);
        assert_eq!(k.height, 1.0);
        assert_eq!(k.align_x, None);
        assert_eq!(k.align_y, None);
        assert_eq!(k.thickness, 0.0);
        assert_eq!(k.alpha, 1.0);
        assert_eq!(k.blink, CursorBlinkMode::Auto);
        assert_eq!(k.blink_rate_ms, 1060.0);
        assert_eq!(k.blink_duty, 0.5);
        assert_eq!(k.blink_style, CursorBlinkStyle::Hidden);
        assert_eq!(k.anim, CursorAnimation::None);
    }

    #[test]
    fn cursor_visual_config_parsed() {
        let c = parse_str(
            "return { cursor = {
               shape = 'bar', width = 0.5, height = 1.4,
               align_x = 'center', align_y = 'bottom', thickness = 3,
               alpha = 0.8,
               blink = true, blink_rate = 800, blink_duty = 0.3,
               blink_style = 'fade',
               anim = 'pulse', anim_ms = 900, anim_amount = 0.4,
             } }",
        );
        let k = &c.cursor;
        assert_eq!(k.shape, CursorShapeOverride::Bar);
        assert_eq!(k.width, 0.5);
        assert_eq!(k.height, 1.4);
        assert_eq!(k.align_x, Some(0.5));
        assert_eq!(k.align_y, Some(1.0));
        assert_eq!(k.thickness, 3.0);
        assert_eq!(k.alpha, 0.8);
        assert_eq!(k.blink, CursorBlinkMode::Always);
        assert_eq!(k.blink_rate_ms, 800.0);
        assert_eq!(k.blink_duty, 0.3);
        assert_eq!(k.blink_style, CursorBlinkStyle::Fade);
        assert_eq!(k.anim, CursorAnimation::Pulse);
        assert_eq!(k.anim_ms, 900.0);
        assert_eq!(k.anim_amount, 0.4);

        // 别名与布尔 / 数字锚点也要能吃
        let c2 = parse_str(
            "return { cursor = {
               shape = 'underline', align_x = 0.25, align_y = 'middle',
               blink = 'off', blink_style = 'outline', anim = 'breathe',
             } }",
        );
        let k2 = &c2.cursor;
        assert_eq!(k2.shape, CursorShapeOverride::Underline);
        assert_eq!(k2.align_x, Some(0.25));
        assert_eq!(k2.align_y, Some(0.5));
        assert_eq!(k2.blink, CursorBlinkMode::Never);
        assert_eq!(k2.blink_style, CursorBlinkStyle::Outline);
        assert_eq!(k2.anim, CursorAnimation::Breathe);
    }

    #[test]
    fn cursor_visual_config_clamps_out_of_range() {
        let c = parse_str(
            "return { cursor = {
               width = -2, height = 99, alpha = 5, thickness = 500,
               blink_rate = 1, blink_duty = 3, anim_ms = 0, anim_amount = 9,
               shape = '不认识', blink_style = '不认识', anim = '不认识',
             } }",
        );
        let k = &c.cursor;
        assert_eq!(k.width, 0.1);
        assert_eq!(k.height, 4.0);
        assert_eq!(k.alpha, 1.0);
        assert_eq!(k.thickness, 64.0);
        assert_eq!(k.blink_rate_ms, 60.0);
        assert_eq!(k.blink_duty, 0.95);
        assert_eq!(k.anim_ms, 60.0);
        assert_eq!(k.anim_amount, 1.0);
        assert_eq!(k.shape, CursorShapeOverride::Auto, "未知值忽略而不是报错");
        assert_eq!(k.blink_style, CursorBlinkStyle::Hidden);
        assert_eq!(k.anim, CursorAnimation::None);
    }

    // --- 内置主题 -------------------------------------------------------
    /// 随包分发的主题必须是合法 Lua,且确实把右侧滚动条关掉。这个文件由
    /// build.cmd / 安装包复制到 `<exe>/config/`,要是写错语法,得等用户敲了
    /// `theme = "deepseek"` 才发现 —— 所以在这里直接兜住。
    #[test]
    fn bundled_deepseek_theme_parses_and_hides_scrollbar() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("config")
            .join("deepseek.lua");
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("读不到 {}: {e}", path.display()));
        let c = parse(&src).unwrap_or_else(|e| panic!("主题解析失败: {e}"));

        // 色板逐项取自早期 wezterm 配置的 DARK 表,别让主题被改回官方近黑配色
        assert_eq!(c.colors.background, "#0B1220", "夜空蓝画布");
        assert_eq!(c.colors.foreground, "#E8EDF9");
        assert_eq!(c.colors.cursor_bg, "#4D6BFE");
        assert_eq!(c.colors.ansi.len(), 16, "8 基础 + 8 亮色");
        assert_eq!(c.colors.ansi[1], "#F0564A", "DARK.ansi 的 red");
        assert_eq!(c.colors.ansi[4], "#4D6BFE", "蓝色位就是品牌蓝");
        assert_eq!(c.colors.ansi[12], "#6D87FF", "DARK.brights 的 blue");
        assert_eq!(c.tab_bar.colors.active.bg, "#4D6BFE", "实心品牌蓝活动标签");
        assert_eq!(c.tab_bar.colors.inactive.fg, "#A9B4CC");
        assert_eq!(c.selection.background, "#1E2A4A");
        assert_eq!(c.selection.opacity, 1.0, "wezterm 选区色是不透明的");
        assert_eq!(c.split_divider.color, "#3B4A72");
        assert!(c.split_divider.gradient.is_some());
        assert!(!c.scrollbar.enabled, "主题要求去掉右侧滚动条");
        assert_eq!(c.scrollbar.show, ScrollbarShow::Never);
        // 主题只管外观:不该写 initial_cols/rows —— 否则用户调 font.size 时
        // 窗口会被"按列数反推"一起撑大/缩小。想固定窗口尺寸用 window_width/height。
        assert_eq!(c.initial_cols, None, "主题不应决定窗口尺寸");
        assert_eq!(c.initial_rows, None);
        // 状态栏画在标签栏上,所以主题里这条得是开着的
        assert!(c.tab_bar.enabled, "状态栏需要标签栏承载");
    }

    /// 启动窗口的像素尺寸:合法值照收,离谱的小值当没写(防止窗口开不出来)。
    #[test]
    fn window_size_px_and_its_clamp() {
        let c = parse_str(
            "return { window_width = 1200, window_height = 800, \
             initial_cols = 100, initial_rows = 30 }",
        );
        assert_eq!(c.window_width, Some(1200.0));
        assert_eq!(c.window_height, Some(800.0));
        // 像素尺寸不覆盖 initial_cols 的解析(优先级在 main.rs 里处理),
        // 这里只确认两项都读到了,启动时以像素为准。
        assert_eq!(c.initial_cols, Some(100));

        let c = parse_str("return { window_width = 10, window_height = 5 }");
        assert_eq!(c.window_width, None, "过小的宽度当没写");
        assert_eq!(c.window_height, None, "过小的高度当没写");
    }

    /// 手动验证用(默认不跑):加载真实的当前配置,把最终生效的关键项打出来。
    /// 排查"改了配置没反应"时执行:
    /// `cargo test -p vectorterm manually_dump_user_config -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn manually_dump_user_config() {
        let (path, note) = resolve_path_ex(None);
        let Some(p) = path else {
            eprintln!("没有找到任何配置(既没有 current/,程序目录也没有默认配置)");
            return;
        };
        eprintln!("当前配置目录:{:?}", active_config_dir());
        if let Some(n) = &note {
            eprintln!("注意: {n}");
        }
        let (cfg, err) = load(None);
        eprintln!("实际加载:{}", p.display());
        eprintln!("错误: {err:?}");
        eprintln!(
            "tab_bar.enabled={} window={:?}x{:?} initial_cols={:?} font.size={} bg={} shell={}",
            cfg.tab_bar.enabled,
            cfg.window_width,
            cfg.window_height,
            cfg.initial_cols,
            cfg.fonts.size,
            cfg.colors.background,
            cfg.shell
        );
        // 主题里若写了状态栏,ScriptEngine 这边应该能看到回调
        let actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        match crate::script::ScriptEngine::load(&p, actions) {
            Ok(eng) => {
                eprintln!("状态栏回调已注册: {}", eng.has_status_hooks());
                let sb = &cfg.status_bar;
                eprintln!(
                    "status_bar: enable={} keep_script={} font_scale={} height={:?} padding={} spacing={} space_width={} text={:?}",
                    sb.enabled,
                    sb.keep_script,
                    sb.font_scale,
                    sb.height,
                    sb.padding,
                    sb.item_spacing,
                    sb.space_width,
                    sb.text
                );
            }
            Err(e) => eprintln!("ScriptEngine 加载失败: {e}"),
        }
    }

    /// `theme = "name"` 以主题为底、用户表深合并覆盖:只改一个颜色时,
    /// 其余配色和别的表(滚动条等)都要完整留着。
    #[test]
    fn theme_directive_deep_merges_user_overrides() {
        let dir = std::env::temp_dir().join(format!(
            "vectorterm-theme-merge-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("themes")).unwrap();
        let theme_path = dir.join("themes").join("unit.lua");
        let config_path = dir.join("config.lua");
        std::fs::write(
            &theme_path,
            r##"return {
              colors = {
                foreground = "#111111",
                background = "#222222",
                cursor_bg  = "#333333",
              },
              scrollbar = { enabled = false, show = "never" },
            }"##,
        )
        .unwrap();
        std::fs::write(
            &config_path,
            r##"return {
              theme = "unit",
              colors = { foreground = "#abcdef" },
              padding = 12,
            }"##,
        )
        .unwrap();

        let c = parse_with_theme(&std::fs::read_to_string(&config_path).unwrap(), &config_path)
            .expect("主题 + 用户配置应能解析");
        assert_eq!(c.colors.foreground, "#abcdef", "用户覆盖生效");
        assert_eq!(c.colors.background, "#222222", "主题里未覆盖的键要保留");
        assert_eq!(c.colors.cursor_bg, "#333333");
        assert!(!c.scrollbar.enabled, "深合并不能把主题的其它分组弄丢");
        assert_eq!(c.padding, 12.0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `config_files` 通用分片:按数组顺序深合并(后者覆盖前者),主配置内联字段
    /// 最后覆盖;三层都没写的键落回代码默认。这就是「config.lua 只做引用」的核心。
    #[test]
    fn single_config_file_is_the_whole_config() {
        let dir = std::env::temp_dir().join(format!(
            "vectorterm-single-config-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("我的配置.lua");
        std::fs::write(
            &config_path,
            r##"return {
              colors = { foreground = "#cccccc", background = "#bbbbbb" },
              padding = 20,
              window_width = 1000,
            }"##,
        )
        .unwrap();

        let c = parse_with_theme(&std::fs::read_to_string(&config_path).unwrap(), &config_path)
            .expect("单份配置应能解析");
        assert_eq!(c.colors.foreground, "#cccccc");
        assert_eq!(c.padding, 20.0);
        assert_eq!(c.window_width, Some(1000.0));

        // 旧写法里的 config_files 现在被忽略:不再读别的 lua,只认本文件。
        std::fs::write(
            &config_path,
            r##"return {
              config_files = { "shard.lua" },
              padding = 12,
            }"##,
        )
        .unwrap();
        std::fs::write(dir.join("shard.lua"), "return { padding = 99 }").unwrap();
        let c = parse_with_theme(&std::fs::read_to_string(&config_path).unwrap(), &config_path)
            .expect("残留的 config_files 不应让加载失败");
        assert_eq!(c.padding, 12.0, "config_files 已废弃,不应再生效");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 走真正的 `load()` 入口(和热重载 / 启动同一条路径),用随包分发的
    /// deepseek.lua 当主题底表,确认用户覆盖与「去掉滚动条」都生效。
    #[test]
    fn load_with_bundled_deepseek_theme() {
        let dir = std::env::temp_dir().join(format!(
            "vectorterm-theme-load-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("themes")).unwrap();
        let theme_src = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("config")
                .join("deepseek.lua"),
        )
        .unwrap();
        std::fs::write(dir.join("themes").join("deepseek.lua"), theme_src).unwrap();
        let config_path = dir.join("config.lua");
        std::fs::write(
            &config_path,
            "return { theme = \"deepseek\", padding = 14, colors = { foreground = \"#fafafa\" } }",
        )
        .unwrap();

        let (cfg, err) = load(Some(&config_path));
        assert!(err.is_none(), "load 不应报错:{err:?}");
        assert_eq!(cfg.colors.background, "#0B1220", "主题底色");
        assert_eq!(cfg.colors.cursor_bg, "#4D6BFE", "品牌蓝光标");
        assert_eq!(cfg.colors.foreground, "#fafafa", "用户覆盖");
        assert_eq!(cfg.padding, 14.0);
        assert!(!cfg.scrollbar.enabled, "主题关掉了右侧滚动条");
        assert_eq!(cfg.scrollbar.show, ScrollbarShow::Never);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn theme_name_rejects_path_traversal() {
        let p = Path::new("C:/tmp/VectorTerm/config.lua");
        for bad in ["../evil", "a/b", "..", "", "with space"] {
            let src = format!("return {{ theme = {bad:?} }}");
            let e = parse_with_theme(&src, p).unwrap_err();
            assert!(e.contains("名字不合法"), "{bad:?} → {e}");
        }
        assert!(valid_theme_name("deepseek"));
        assert!(valid_theme_name("DeepSeek-Dark_2"));
        assert!(!valid_theme_name("../evil"));
        assert!(!valid_theme_name("a/b"));
    }

    #[test]
    fn theme_without_file_reports_clear_error() {
        let p = Path::new("C:/tmp/VectorTerm/config.lua");
        let e = parse_with_theme("return { theme = \"no_such_theme_9f3a\" }", p).unwrap_err();
        assert!(e.contains("找不到内置主题"), "{e}");
    }

    // --- 分屏分割线配置 -------------------------------------------------
    #[test]
    fn split_divider_defaults_keep_legacy_look() {
        let d = parse_str("return {}").split_divider;
        assert!(d.enabled);
        assert_eq!(d.thickness, None, "未设置厚度时填满整格缝");
        assert_eq!((d.inset_start, d.inset_end), (0.0, 0.0));
        assert_eq!(d.color, "#323236");
        assert!(d.gradient.is_none());
        assert_eq!(d.opacity, 1.0);
        assert!(d.is_visible());
    }

    #[test]
    fn split_divider_solid_thickness_and_insets() {
        let d = parse_str(
            r##"return { split_divider = {
                thickness = 3, inset_start = 12, inset_end = 24,
                color = "#88aaff", opacity = 0.6, enabled = true
            } }"##,
        )
        .split_divider;
        assert_eq!(d.thickness, Some(3.0));
        assert_eq!((d.inset_start, d.inset_end), (12.0, 24.0));
        assert_eq!(d.color, "#88aaff");
        assert!((d.opacity - 0.6).abs() < 1e-6);
        assert!(d.gradient.is_none());
    }

    #[test]
    fn split_divider_inset_shorthand_can_be_overridden() {
        let d = parse_str(
            "return { split_divider = { inset = 10, inset_end = 30 } }",
        )
        .split_divider;
        assert_eq!((d.inset_start, d.inset_end), (10.0, 30.0));
    }

    #[test]
    fn split_divider_gradient_shorthands() {
        let a = parse_str(
            r##"return { split_divider = { color = { "#ff0000", "#00ff00", "#0000ff" } } }"##,
        )
        .split_divider
        .gradient
        .unwrap();
        assert_eq!(a.colors, ["#ff0000", "#00ff00", "#0000ff"]);
        assert_eq!(a.orientation, DividerGradientDir::Auto);

        let b = parse_str(
            "return { split_divider = { gradient = { \"#111111\", \"#eeeeee\" } } }",
        )
        .split_divider
        .gradient
        .unwrap();
        assert_eq!(b.colors.len(), 2);
        assert_eq!(b.orientation, DividerGradientDir::Auto);
    }

    #[test]
    fn split_divider_gradient_full_form_and_wezterm_angle() {
        let v = parse_str(
            r##"return { split_divider = { gradient = {
                colors = { "#111111", "#999999", "#ffffff" },
                orientation = "Horizontal"
            } } }"##,
        )
        .split_divider
        .gradient
        .unwrap();
        assert_eq!(v.colors.len(), 3);
        assert_eq!(v.orientation, DividerGradientDir::Horizontal);

        // wezterm 的 { Linear = { angle = 90 } } 视作竖向,0 视作横向
        let w = parse_str(
            "return { split_divider = { gradient = { colors = { '#000', '#fff' }, orientation = { Linear = { angle = 0 } } } } }",
        )
        .split_divider
        .gradient
        .unwrap();
        assert_eq!(w.orientation, DividerGradientDir::Horizontal);
    }

    #[test]
    fn split_divider_rejects_bad_values() {
        let d = parse_str(
            "return { split_divider = { thickness = 999, inset_start = -5, opacity = 3, enabled = false, gradient = { '#000000' } } }",
        )
        .split_divider;
        assert_eq!(d.thickness, Some(256.0));
        assert_eq!(d.inset_start, 0.0);
        assert_eq!(d.opacity, 1.0);
        assert!(d.gradient.is_none(), "单色不构成渐变");
        assert!(!d.enabled);
        assert!(!d.is_visible());

        let hidden = parse_str("return { split_divider = { thickness = 0 } }").split_divider;
        assert!(!hidden.is_visible());
    }

    // --- 文字间距 / 焦点配置 -------------------------------------------
    #[test]
    fn split_divider_gap_and_text_gap_parse() {
        let d = parse_str("return {}").split_divider;
        assert_eq!(d.gap, None, "默认自动");
        assert_eq!(d.text_gap, None, "默认自动");

        let d = parse_str(
            "return { split_divider = { gap = 14, text_gap = 4.5 } }",
        )
        .split_divider;
        assert_eq!(d.gap, Some(14.0));
        assert_eq!(d.text_gap, Some(4.5));

        // "Auto" / nil / 字符串数字都能接受
        let d = parse_str(
            r#"return { split_divider = { gap = "Auto", text_gap = "3" } }"#,
        )
        .split_divider;
        assert_eq!(d.gap, None);
        assert_eq!(d.text_gap, Some(3.0));

        let d = parse_str("return { split_divider = { gap = -9 } }").split_divider;
        assert_eq!(d.gap, Some(0.0), "负数归零而不是变成非法矩形");
    }

    #[test]
    fn files_gui_open_mode_defaults_to_new_window() {
        assert_eq!(parse_str("return {}").files_gui.open_mode, FileOpenMode::Window);
        assert_eq!(
            FilesGuiConfig::default().open_mode,
            FileOpenMode::Window,
            "默认必须单独开一个窗口"
        );

        let m = |s: &str| parse_str(&format!("return {{ files_gui = {{ open_mode = \"{s}\" }} }}")).files_gui.open_mode;
        assert_eq!(m("split"), FileOpenMode::Split);
        assert_eq!(m("SPLIT"), FileOpenMode::Split, "大小写不敏感");
        assert_eq!(m("new-pane"), FileOpenMode::Split);
        assert_eq!(m("current"), FileOpenMode::Current);
        assert_eq!(m(" inline "), FileOpenMode::Current, "两边空白会被忽略");
        assert_eq!(m("window"), FileOpenMode::Window);
        assert_eq!(m("bogus"), FileOpenMode::Window, "认不出来保留默认");

        // 写回配置管理器时用的名字要和解析对得上
        assert_eq!(FileOpenMode::parse(FileOpenMode::Window.as_str()), Some(FileOpenMode::Window));
        assert_eq!(FileOpenMode::parse(FileOpenMode::Split.as_str()), Some(FileOpenMode::Split));
        assert_eq!(FileOpenMode::parse(FileOpenMode::Current.as_str()), Some(FileOpenMode::Current));
    }

    #[test]
    fn files_gui_cwd_sync_defaults_and_overrides() {
        let f = parse_str("return {}").files_gui;
        assert!(f.follow_cwd, "默认跟终端走");
        assert!(f.push_cwd, "默认也把目录同步给终端");

        let f = parse_str(
            "return { files_gui = { follow_cwd = false, push_cwd = false } }",
        )
        .files_gui;
        assert!(!f.follow_cwd);
        assert!(!f.push_cwd);

        // 只写一半时另一半保持默认
        let f = parse_str("return { files_gui = { push_cwd = false } }").files_gui;
        assert!(f.follow_cwd);
        assert!(!f.push_cwd);
    }

    #[test]
    fn scrollbar_appearance_and_split_resize_parse() {
        let d = parse_str("return {}");
        assert_eq!(d.scrollbar.show, ScrollbarShow::Auto);
        assert_eq!(d.scrollbar.side, ScrollbarSide::Right);
        assert!(d.scrollbar.show_track);
        assert_eq!(d.scrollbar.hover_color, None);
        assert!(d.split_divider.draggable);
        assert_eq!(d.split_divider.hit_width, 8.0);
        assert_eq!(d.split_divider.min_ratio, 0.08);
        assert_eq!(d.split_divider.step, 0.05);

        let c = parse_str(
            r##"return {
                scrollbar = {
                    -- 同时写旧字段和 show,以 show 为准
                    auto_hide = false, show = "never", side = "left",
                    show_track = false, hover_color = "#aabbcc",
                },
                split_divider = {
                    draggable = false, hit_width = 20,
                    min_ratio = 0.2, step = 0.1,
                },
            }"##,
        );
        assert_eq!(c.scrollbar.show, ScrollbarShow::Never);
        assert_eq!(c.scrollbar.side, ScrollbarSide::Left);
        assert!(!c.scrollbar.show_track);
        assert_eq!(c.scrollbar.hover_color.as_deref(), Some("#aabbcc"));
        assert!(!c.split_divider.draggable);
        assert_eq!(c.split_divider.hit_width, 20.0);
        assert_eq!(c.split_divider.min_ratio, 0.2);
        assert_eq!(c.split_divider.step, 0.1);

        // 旧字段仍然独立生效
        let old = parse_str("return { scrollbar = { auto_hide = false } }");
        assert_eq!(old.scrollbar.show, ScrollbarShow::Always);
    }

    #[test]
    fn scrollbar_and_search_defaults_and_overrides() {
        let d = parse_str("return {}");
        assert!(d.scrollbar.enabled);
        assert_eq!(d.scrollbar.width, 9.0);
        assert_eq!(d.scrollbar.min_thumb, 28.0);
        assert!(d.scrollbar.auto_hide, "默认自动隐藏,避免轨道贴着分割线形成灰带");
        assert_eq!(d.scrollbar.opacity, 0.85);
        assert_eq!(d.scrollbar.track_color, None);
        assert!(d.search.enabled);
        assert!(!d.search.case_sensitive);
        assert_eq!(d.search.max_matches, 5000);
        assert!(d.selection.edge_scroll);

        let c = parse_str(
            r##"return {
                scrollbar = {
                    enabled = true, width = 14, min_thumb = 40, auto_hide = false,
                    opacity = 0.4, track_color = "Auto", thumb_color = "#ff0000"
                },
                search = {
                    enabled = true, case_sensitive = true, max_matches = 999999,
                    highlight_color = "#00ff00", current_color = "#0000ff"
                },
                selection = { edge_scroll = false },
            }"##,
        );
        assert_eq!(c.scrollbar.width, 14.0);
        assert_eq!(c.scrollbar.min_thumb, 40.0);
        assert!(!c.scrollbar.auto_hide, "仍然可以显式改回常显");
        assert_eq!(c.scrollbar.opacity, 0.4);
        assert_eq!(c.scrollbar.track_color, None, "Auto 视为自动配色");
        assert_eq!(c.scrollbar.thumb_color.as_deref(), Some("#ff0000"));
        assert!(c.search.case_sensitive);
        assert_eq!(c.search.highlight_color, "#00ff00");
        assert_eq!(c.search.current_color, "#0000ff");
        assert_eq!(c.search.max_matches, 200_000, "上限夹住");
        assert!(!c.selection.edge_scroll);

        let off = parse_str("return { scrollbar = false, search = false }");
        assert!(!off.scrollbar.enabled);
        assert!(!off.search.enabled);
        let clamp = parse_str("return { scrollbar = { width = 999, opacity = 9 } }");
        assert_eq!(clamp.scrollbar.width, 64.0);
        assert_eq!(clamp.scrollbar.opacity, 1.0);
    }

    #[test]
    fn pane_focus_defaults_and_overrides() {
        let f = parse_str("return {}").pane_focus;
        assert!(f.click_to_focus);
        assert!(f.hide_inactive_caret, "默认只让聚焦 pane 显示输入光标");
        assert!(f.trail);
        assert!(f.sync_trail, "默认与普通光标尾迹同步");
        assert_eq!(f.trail_ms, 320.0);
        assert_eq!(f.trail_color, None, "默认跟随光标色");
        assert_eq!(f.landing, CaretLanding::Caret, "默认落在真实输入光标处");
        assert!(f.number_shortcuts);

        let f = parse_str(
            r##"return { pane_focus = {
                click_to_focus = false, hide_inactive_caret = false,
                trail = false, sync_trail = false, trail_ms = 600,
                trail_color = "Auto",
                landing = "Center", number_shortcuts = false
            } }"##,
        )
        .pane_focus;
        assert!(!f.click_to_focus);
        assert!(!f.hide_inactive_caret);
        assert!(!f.trail);
        assert!(!f.sync_trail, "可以关掉同步,单独指定飞行尾迹颜色");
        assert_eq!(f.trail_ms, 600.0);
        assert_eq!(f.trail_color, None, "Auto 视为跟随光标色");
        assert_eq!(f.landing, CaretLanding::Center);
        assert!(!f.number_shortcuts);

        let f = parse_str(
            r##"return { pane_focus = { trail_color = "#abcdef", trail_ms = 99999, landing = "caret" } }"##,
        )
        .pane_focus;
        assert_eq!(f.trail_color.as_deref(), Some("#abcdef"));
        assert_eq!(f.trail_ms, 5000.0, "时长有上限");
        assert_eq!(f.landing, CaretLanding::Caret);

        // 旧配置里残留的描边键应被静默忽略,不再产生任何效果
        let f = parse_str(
            r##"return { pane_focus = { focus_border = true, focus_border_color = "#ff8800" } }"##,
        )
        .pane_focus;
        assert!(f.hide_inactive_caret);
        assert_eq!(f.landing, CaretLanding::Caret);
    }

    /// 一段真实的 wezterm 配置(tmpceshi.lua 的子集)应被完整理解:
    /// default_prog、font_with_fallback、ansi+brights、tab_bar、initial_cols 等
    #[test]
    fn wezterm_style_config_is_understood() {
        let cfg = parse_str(
            r##"
            local wezterm = require 'wezterm'
            local config = wezterm.config_builder()
            config.default_prog = { 'pwsh.exe', '-NoLogo' }
            config.font = wezterm.font_with_fallback({ 'MesloLGM Nerd Font', 'Cascadia Code' })
            config.font_size = 10.0
            config.line_height = 0.886
            config.window_padding = { left = 6, right = 6, top = 4, bottom = 4 }
            config.window_decorations = "RESIZE"
            config.colors = {
              background = '#0B1220',
              foreground = '#E8EDF9',
              selection_bg = '#1E2A4A',
              ansi = { '#000000', '#ff0000', '#00ff00', '#ffff00',
                       '#0000ff', '#ff00ff', '#00ffff', '#ffffff' },
              brights = { '#111111', '#ff5555', '#55ff55', '#ffff55',
                          '#5555ff', '#ff55ff', '#55ffff', '#eeeeee' },
              tab_bar = {
                background = '#0B1220',
                active_tab = { bg_color = '#4D6BFE', fg_color = '#FFFFFF' },
                inactive_tab = { bg_color = '#16223A', fg_color = '#A9B4CC' },
                new_tab = { bg_color = '#0B1220', fg_color = '#8A93A8' },
                inactive_tab_edge = '#22304A',
              },
            }
            config.enable_tab_bar = true
            config.tab_bar_at_bottom = true
            config.hide_tab_bar_if_only_one_tab = true
            config.tab_max_width = 24
            config.show_tab_index_in_tab_bar = false
            config.set_environment_variables = { DSH_TUI_IMAGE_PROTOCOL = 'kitty' }
            config.initial_cols = 117
            config.initial_rows = 32
            return config
            "##,
        );

        assert_eq!(cfg.shell, "pwsh.exe");
        assert_eq!(cfg.shell_args, vec!["-NoLogo"]);
        assert_eq!(cfg.fonts.family, "MesloLGM Nerd Font");
        assert_eq!(cfg.fonts.fallback, vec!["Cascadia Code"]);
        assert!((cfg.fonts.size - 10.0).abs() < 1e-6);
        assert_eq!(cfg.fonts.line_height, Some(0.886));
        assert_eq!(cfg.padding, 4.0);
        assert_eq!(cfg.colors.background, "#0B1220");
        assert_eq!(cfg.colors.ansi.len(), 16);
        assert_eq!(cfg.colors.ansi[8], "#111111", "brights 应接在 ansi 后面");
        assert_eq!(cfg.selection.background, "#1E2A4A");
        assert_eq!(cfg.selection.opacity, 1.0, "wezterm 的选区色是不透明的");
        assert_eq!(cfg.tab_bar.colors.active.bg, "#4D6BFE");
        assert_eq!(cfg.tab_bar.colors.inactive.fg, "#A9B4CC");
        assert_eq!(cfg.tab_bar.colors.edge, "#22304A");
        assert!(cfg.tab_bar.enabled);
        assert!(cfg.tab_bar.at_bottom);
        assert!(cfg.tab_bar.hide_if_one);
        assert_eq!(cfg.tab_bar.max_width_cells, 24);
        assert!(!cfg.tab_bar.show_index);
        assert!(!cfg.window.title, "RESIZE 表示不要系统标题栏");
        assert!(
            cfg.window.resize,
            "RESIZE 允许缩放(无 TITLE 时由自绘热区接管)"
        );
        assert!(
            !cfg.window.integrated_buttons,
            "wezterm 的 RESIZE 不画集成按钮,只有 INTEGRATED_BUTTONS 才画"
        );
        assert!(cfg
            .env
            .iter()
            .any(|(k, v)| k == "DSH_TUI_IMAGE_PROTOCOL" && v == "kitty"));
        assert_eq!(cfg.initial_cols, Some(117));
        assert_eq!(cfg.initial_rows, Some(32));
    }

    /// 窗口装饰的标志组合:默认值不要系统标题栏(用户要求),
    /// 但允许缩放并保留标签栏集成按钮
    #[test]
    fn window_decorations_defaults_and_flags() {
        let d = AppConfig::default().window;
        assert!(d.title, "默认用 Windows 系统标题栏");
        assert!(d.resize);
        assert!(!d.integrated_buttons, "按钮由系统画,标签栏不重复");
        assert!(!d.csd());
        assert!(!d.show_buttons());

        // wezterm 的四种标准写法
        let cases = [
            ("NONE", false, false, false),
            ("TITLE", true, false, false),
            ("RESIZE", false, true, false),
            ("TITLE | RESIZE", true, true, false),
            ("INTEGRATED_BUTTONS|RESIZE", false, true, true),
        ];
        for (spec, title, resize, integrated) in cases {
            let (t, r, i) = WindowDecorations::parse_flags(spec);
            assert_eq!((t, r, i), (title, resize, integrated), "{spec}");
        }
        // INTEGRATED_BUTTONS 与 TITLE 互斥(按钮代替标题栏)
        assert_eq!(
            WindowDecorations::parse_flags("TITLE|INTEGRATED_BUTTONS|RESIZE"),
            (false, true, true)
        );
        // macOS 专有标志不干扰 Windows
        assert_eq!(
            WindowDecorations::parse_flags("RESIZE|MACOS_FORCE_DISABLE_SHADOW"),
            (false, true, false)
        );
    }

    #[test]
    fn window_decorations_flags_are_read_from_lua() {
        let cfg = parse_str("return { window_decorations = 'TITLE|RESIZE' }");
        assert!(cfg.window.title);
        assert!(cfg.window.resize);
        assert!(
            !cfg.window.show_buttons(),
            "系统标题栏自带按钮,标签栏不重复画"
        );

        let cfg = parse_str("return { window_decorations = 'INTEGRATED_BUTTONS|RESIZE' }");
        assert!(!cfg.window.title);
        assert!(cfg.window.resize);
        assert!(cfg.window.show_buttons());

        let cfg = parse_str("return { window_decorations = 'NONE' }");
        assert!(!cfg.window.title);
        assert!(!cfg.window.resize);
        assert!(!cfg.window.show_buttons());
    }

    /// 按钮可以单独关掉 / 排序,关按钮不等于把顶部那条栏去掉
    #[test]
    fn integrated_title_buttons_are_independent_from_titlebar() {
        let cfg = parse_str(
            "return { window_decorations = 'INTEGRATED_BUTTONS|RESIZE', \
             integrated_title_buttons = {} }",
        );
        assert!(!cfg.window.show_buttons(), "空表 = 三枚按钮全隐藏");
        assert!(cfg.window.csd(), "顶部标签栏仍在,还能拖动窗口");

        let cfg = parse_str("return { integrated_title_buttons = false }");
        assert!(cfg.window.buttons.is_empty());

        let cfg =
            parse_str("return { integrated_title_buttons = { 'Close', 'Maximize', 'Hide' } }");
        assert_eq!(
            cfg.window.buttons,
            vec![TitleButton::Close, TitleButton::Maximize, TitleButton::Hide]
        );
        let cfg = parse_str("return { integrated_title_buttons = { 'Minimize', 'Close' } }");
        assert_eq!(
            cfg.window.buttons,
            vec![TitleButton::Hide, TitleButton::Close]
        );
    }

    #[test]
    fn integrated_title_button_options_are_read() {
        let cfg = parse_str(
            r#"
            return {
              integrated_title_button_alignment = 'Left',
              integrated_title_button_style = 'Gnome',
              integrated_title_button_color = '#E06C75',
            }
            "#,
        );
        assert_eq!(cfg.window.alignment, ButtonAlignment::Left);
        assert_eq!(cfg.window.style, ButtonStyle::Gnome);
        assert_eq!(cfg.window.color.as_deref(), Some("#E06C75"));

        let cfg = parse_str("return { integrated_title_button_color = 'Auto' }");
        assert_eq!(cfg.window.color, None);
    }

    /// `window_titlebar` 是简写,只覆盖"有没有系统标题栏"这一维
    #[test]
    fn window_titlebar_is_a_shortcut() {
        assert!(AppConfig::default().window.title);

        // 默认已经没有集成按钮了,想验证"关标题栏仍保留按钮"得两个维度都写
        let cfg = parse_str(
            "return { window_decorations = 'INTEGRATED_BUTTONS|RESIZE', window_titlebar = false }",
        );
        assert!(!cfg.window.title);
        assert!(cfg.window.show_buttons(), "简写关标题栏时保留集成按钮");

        let cfg = parse_str("return { window_titlebar = true }");
        assert!(cfg.window.title);
        assert!(!cfg.window.show_buttons());

        // 两种写法混用:简写决定标题栏,flags 决定边框与按钮
        let cfg = parse_str(
            "return { window_titlebar = true, window_decorations = 'INTEGRATED_BUTTONS|RESIZE' }",
        );
        assert!(cfg.window.title);
        assert!(cfg.window.resize);
        assert!(!cfg.window.show_buttons());

        let cfg =
            parse_str("return { window_titlebar = false, window_decorations = 'TITLE|RESIZE' }");
        assert!(!cfg.window.title);
        assert!(cfg.window.resize);
    }

    /// 无边框窗口的 Win11 圆角默认开,也能显式关掉
    #[test]
    fn window_rounded_corners_are_configurable() {
        assert!(AppConfig::default().window.rounded_corners);

        let cfg = parse_str("return { window_decorations = 'RESIZE' }");
        assert!(cfg.window.rounded_corners, "RESIZE 也应保留圆角");

        let cfg = parse_str("return { window_rounded_corners = false }");
        assert!(!cfg.window.rounded_corners);

        let cfg = parse_str("return { window_rounded_corners = true }");
        assert!(cfg.window.rounded_corners);
    }

    /// 拖动时默认与 wezterm 一致:实时调整文字位置;另两种是可选退路
    #[test]
    fn live_resize_mode_is_configurable() {
        assert_eq!(
            AppConfig::default().live_resize_mode,
            LiveResizeMode::Realtime
        );
        for (src, want) in [
            (
                "return { live_resize_mode = 'realtime' }",
                LiveResizeMode::Realtime,
            ),
            (
                "return { live_resize_mode = 'DEBOUNCE' }",
                LiveResizeMode::Debounce,
            ),
            (
                "return { live_resize_mode = 'freeze' }",
                LiveResizeMode::Freeze,
            ),
            (
                "return { live_resize_mode = 'typo' }",
                LiveResizeMode::Realtime,
            ),
        ] {
            assert_eq!(parse_str(src).live_resize_mode, want, "src={src}");
        }
    }

    /// 拖动缩放的稳定窗口:默认开启,0 = 实时同步,过大值要夹住
    #[test]
    fn resize_debounce_is_configurable() {
        assert_eq!(AppConfig::default().resize_debounce_ms, 150);

        let cfg = parse_str("return { resize_debounce_ms = 0 }");
        assert_eq!(cfg.resize_debounce_ms, 0, "0 应回到 wezterm 的实时同步");

        let cfg = parse_str("return { resize_debounce_ms = 250 }");
        assert_eq!(cfg.resize_debounce_ms, 250);

        let cfg = parse_str("return { resize_debounce_ms = 99999 }");
        assert_eq!(cfg.resize_debounce_ms, 1000, "上限 1000ms");
    }

    /// wezterm 的 use_resize_increments:默认按整格步进,可关掉
    #[test]
    fn resize_increments_are_configurable() {
        assert!(AppConfig::default().use_resize_increments);
        let cfg = parse_str("return { use_resize_increments = false }");
        assert!(!cfg.use_resize_increments);
    }

    /// VectorTerm 原生 tab_bar 表优先级高于 wezterm 顶层别名
    #[test]
    fn native_tab_bar_table_overrides_wezterm_aliases() {
        let cfg = parse_str(
            r##"
            return {
              enable_tab_bar = true,
              tab_bar_at_bottom = true,
              tab_max_width = 20,
              tab_bar = {
                enable = false,
                at_bottom = false,
                max_width = 8,
                show_close_button = false,
                corner_radius = 6,
                colors = { active_tab = { bg = '#123456', fg = '#ffffff' } },
              },
            }
            "##,
        );
        assert!(!cfg.tab_bar.enabled);
        assert!(!cfg.tab_bar.at_bottom);
        assert_eq!(cfg.tab_bar.max_width_cells, 8);
        assert_eq!(cfg.tab_bar.colors.active.bg, "#123456");
        assert!(!cfg.tab_bar.show_close_button);
        assert_eq!(cfg.tab_bar.corner_radius, Some(6.0));
    }

    /// 标签栏四周安全区:默认自动,可显式给数值,显式 0 表示贴边。
    /// wezterm 的 Lua 里没这些键,原样加载不会报错。
    #[test]
    fn tab_bar_padding_is_configurable() {
        let d = AppConfig::default().tab_bar;
        assert_eq!(d.edge_padding, None);
        assert_eq!(d.top_padding, None);

        let cfg = parse_str(
            r##"
            return {
              wezterm_compat = true,
              tab_bar_edge_padding = 12,
              tab_bar_top_padding = 6.5,
            }
            "##,
        );
        assert_eq!(cfg.tab_bar.edge_padding, Some(12.0));
        assert_eq!(cfg.tab_bar.top_padding, Some(6.5));

        // 原生 tab_bar 表覆盖顶层别名;0 是"贴边"而不是"没配"
        let cfg = parse_str(
            r##"
            return {
              tab_bar_top_padding = 8,
              tab_bar = { top_padding = 0, edge_padding = 4 },
            }
            "##,
        );
        assert_eq!(cfg.tab_bar.top_padding, Some(0.0));
        assert_eq!(cfg.tab_bar.edge_padding, Some(4.0));

        // 负数当 0 处理
        let cfg = parse_str("return { tab_bar = { top_padding = -5 } }");
        assert_eq!(cfg.tab_bar.top_padding, Some(0.0));
    }

    #[test]
    fn shell_and_args_come_from_lua() {
        let cfg = parse_str(
            r#"
            return {
              shell = "pwsh.exe",
              shell_args = { "-NoLogo", "-NoProfile" },
              env = { TERM = "xterm-256color", WT_SESSION = "1" },
              cwd = "D:/work",
            }
            "#,
        );
        assert_eq!(cfg.shell, "pwsh.exe");
        assert_eq!(cfg.shell_args, vec!["-NoLogo", "-NoProfile"]);
        assert_eq!(cfg.env.len(), 2);
        assert!(cfg.env.iter().any(|(k, v)| k == "WT_SESSION" && v == "1"));
        assert_eq!(cfg.cwd.as_deref(), Some(Path::new("D:/work")));
    }

    #[test]
    fn shell_defaults_when_absent() {
        let cfg = parse_str("return { padding = 4 }");
        assert_eq!(cfg.shell, "pwsh.exe", "默认应该是 PowerShell 7");
        assert_eq!(cfg.shell, DEFAULT_SHELL);
        assert_eq!(cfg.shell_args, vec!["-NoLogo"], "默认关掉启动横幅");
        assert_eq!(cfg.padding, 4.0);
    }

    /// 显式换了 shell 就不能把默认的 `-NoLogo` 带过去 —— cmd.exe 会以
    /// "无效开关"直接退出,那种 bug 只在用户改配置后才出现,很难查。
    #[test]
    fn explicit_shell_drops_default_args() {
        let cfg = parse_str(r#"return { shell = "cmd.exe" }"#);
        assert_eq!(cfg.shell, "cmd.exe");
        assert!(cfg.shell_args.is_empty(), "cmd 不认 -NoLogo");

        let cfg = parse_str(r#"return { default_prog = { "cmd.exe" } }"#);
        assert_eq!(cfg.shell, "cmd.exe");
        assert!(
            cfg.shell_args.is_empty(),
            "default_prog 只带一个元素时也不能有默认参数"
        );

        // 用户自己给的参数当然要保留
        let cfg = parse_str(r#"return { shell = "pwsh.exe", shell_args = { "-NoProfile" } }"#);
        assert_eq!(cfg.shell_args, vec!["-NoProfile"]);
    }

    #[test]
    fn selection_options_are_configurable() {
        let defaults = parse_str("return {}").selection;
        assert!(defaults.copy_on_select);
        assert_eq!(defaults.background, "#3a6ea5");
        assert_eq!(defaults.ctrl_shift_v, VKeyAction::Paste);

        let cfg = parse_str(
            r##"
            return {
              selection = {
                copy_on_select = false,
                background = "#804000",
                opacity = 0.7,
                ctrl_shift_v = "copy",
              },
            }
            "##,
        );
        assert!(!cfg.selection.copy_on_select);
        assert_eq!(cfg.selection.background, "#804000");
        assert!((cfg.selection.opacity - 0.7).abs() < 1e-5);
        assert_eq!(cfg.selection.ctrl_shift_v, VKeyAction::Copy);

        // 越界不透明度夹到合法区间;未知值按 paste 处理
        let cfg = parse_str("return { selection = { opacity = 9, ctrl_shift_v = 'weird' } }");
        assert_eq!(cfg.selection.opacity, 1.0);
        assert_eq!(cfg.selection.ctrl_shift_v, VKeyAction::Paste);
    }

    /// 自定义背景:纯色 / 图片 / HSB / 模糊 / 填充 / 透明度 / 背板 / 渐变
    #[test]
    fn background_is_configurable() {
        let c = parse_str(
            r##"
            return {
              window_background_image = "bg.jpg",
              window_background_image_hsb = { brightness = 0.4, saturation = 0.2 },
              window_background_blur = 12,
              window_background_fill = "contain",
              window_background_opacity = 0.85,
              win32_system_backdrop = "Acrylic",
              window_background_gradient = {
                colors = { "#111111", "#222222" },
                orientation = "Horizontal",
              },
            }
        "##,
        );
        assert_eq!(c.background.image, Some(PathBuf::from("bg.jpg")));
        assert!((c.background.brightness - 0.4).abs() < 1e-6);
        assert!((c.background.saturation - 0.2).abs() < 1e-6);
        assert_eq!(c.background.fill, ImageFill::Contain);
        assert!((c.background.blur - 12.0).abs() < 1e-6);
        assert_eq!(c.background.backdrop, Backdrop::Acrylic);
        assert!((c.background.effective_opacity() - 0.85).abs() < 1e-6);
        let g = c.background.gradient.as_ref().expect("应解析出渐变");
        assert!(!g.vertical, "orientation = Horizontal 应是横向");
        assert!(!c.background.is_plain());

        // 什么都不配:纯色不透明,走原来的快速路径
        let d = parse_str("return {}");
        assert!(d.background.is_plain());
        assert_eq!(d.background.fill, ImageFill::Cover);
        assert!(!d.background.needs_transparent_window());
    }

    /// 开了毛玻璃却没设 opacity 时,背景必须让位(否则材质被自己的不透明背景盖住)
    #[test]
    fn backdrop_is_parsed_without_forcing_transparency() {
        let c = parse_str(r#"return { win32_system_backdrop = "mica" }"#);
        assert_eq!(c.background.backdrop, Backdrop::Mica);
        // 没有合成表面就没有逐像素 alpha,这里不能把 opacity 归零(否则整窗不可见)
        assert_eq!(c.background.effective_opacity(), 1.0);
        assert!(!c.background.needs_transparent_window());

        // 显式设了 opacity 就尊重它
        let c = parse_str(
            r#"return { win32_system_backdrop = "acrylic", window_background_opacity = 0.6 }"#,
        );
        assert_eq!(c.background.backdrop, Backdrop::Acrylic);
        assert!((c.background.effective_opacity() - 0.6).abs() < 1e-6);
    }

    /// 图片路径按配置文件所在目录解析
    #[test]
    fn background_image_path_is_relative_to_config() {
        let mut cfg = AppConfig::default();
        cfg.background.image = Some(PathBuf::from("pics/bg.png"));
        resolve_background_path(&mut cfg, Path::new("C:/Users/me/.config/VectorTerm/config.lua"));
        assert_eq!(
            cfg.background.image,
            Some(PathBuf::from("C:/Users/me/.config/VectorTerm").join("pics/bg.png"))
        );

        // 绝对路径不动
        let mut cfg = AppConfig::default();
        cfg.background.image = Some(PathBuf::from("D:/wall/bg.png"));
        resolve_background_path(&mut cfg, Path::new("C:/x/config.lua"));
        assert_eq!(cfg.background.image, Some(PathBuf::from("D:/wall/bg.png")));
    }

    /// render 支持字符串与 { backend = ... } 两种写法
    #[test]
    fn render_backend_is_configurable() {
        // 默认软件渲染:GPU 路径拖动缩放时旧帧会被 DXGI 拉伸
        assert_eq!(parse_str("return {}").render, RenderMode::Cpu);
        assert_eq!(
            parse_str("return { render = 'cpu' }").render,
            RenderMode::Cpu
        );
        assert_eq!(
            parse_str("return { render = 'GPU' }").render,
            RenderMode::Gpu
        );
        assert_eq!(
            parse_str("return { render = 'dx12' }").render,
            RenderMode::Gpu
        );
        assert_eq!(
            parse_str("return { render = 'auto' }").render,
            RenderMode::Auto
        );
        assert_eq!(
            parse_str("return { render = { backend = 'cpu' } }").render,
            RenderMode::Cpu
        );
        // 写错就退回默认(CPU),不阻塞启动,也不会把用户悄悄切到 GPU
        assert_eq!(
            parse_str("return { render = 'vulkan' }").render,
            RenderMode::Cpu
        );
    }

    /// gpu_power 决定双显卡机器上先挑核显还是独显
    #[test]
    fn gpu_power_is_configurable() {
        assert_eq!(parse_str("return {}").gpu_power, GpuPower::Low);
        assert_eq!(
            parse_str("return { gpu_power = 'high' }").gpu_power,
            GpuPower::High
        );
        assert_eq!(
            parse_str("return { gpu_power = 'discrete' }").gpu_power,
            GpuPower::High
        );
        assert_eq!(
            parse_str("return { gpu_power = 'integrated' }").gpu_power,
            GpuPower::Low
        );
        // 未知值按默认处理
        assert_eq!(
            parse_str("return { gpu_power = 'turbo' }").gpu_power,
            GpuPower::Low
        );
    }

    /// 关窗白名单:默认带 shell 自己,写空表就是"连 shell 都算在跑"。
    #[test]
    fn skip_close_confirmation_list_is_configurable() {
        let def = parse_str("return {}");
        assert!(
            def.skip_close_confirmation_for_processes_named
                .iter()
                .any(|n| n == "pwsh.exe"),
            "默认名单里得有 shell,否则光开着一个 pwsh 就弹确认"
        );
        assert!(
            def.skip_close_confirmation_for_processes_named
                .iter()
                .any(|n| n == "conhost.exe"),
            "ConPTY 宿主也算在名单里"
        );

        // 自己给一份:整份覆盖,不是追加
        let cfg = parse_str(r#"return { skip_close_confirmation_for_processes_named = { "nu", "x.exe" } }"#);
        assert_eq!(
            cfg.skip_close_confirmation_for_processes_named,
            vec!["nu", "x.exe"]
        );

        // 空表 = 谁都拦
        let empty = parse_str("return { skip_close_confirmation_for_processes_named = {} }");
        assert!(empty.skip_close_confirmation_for_processes_named.is_empty());
    }

    /// scroll 表覆盖滚动动画参数;scroll = false 直接关掉平滑滚动
    #[test]
    fn scroll_animation_is_configurable() {
        let cfg =
            parse_str("return { scroll = { anim = 60, max = 120, back = 70, back_burst = 30 } }");
        assert_eq!(cfg.scroll.anim_ms, 60.0);
        assert_eq!(cfg.scroll.max_ms, 120.0);
        assert_eq!(cfg.scroll.back_ms, 70.0);
        assert_eq!(cfg.scroll.back_burst_ms, 30.0);

        let off = parse_str("return { scroll = false }");
        assert_eq!(off.scroll.anim_ms, 0.0, "scroll = false 应关闭平滑滚动");

        let d = AppConfig::default();
        assert_eq!(d.scroll.anim_ms, 90.0);
        assert_eq!(d.scroll.max_ms, 200.0);
    }

    /// 当前配置目录(current/)里唯一的 lua 压过软件目录默认配置;
    /// 目录里有多份 lua 视为冲突(报错,退回默认)。
    #[test]
    fn active_config_dir_wins_over_bundled_default() {
        // 用 target/ 下的临时目录(系统 %TEMP% 在部分沙箱里不可写)
        let root = Path::new("target").join(format!("cfg-priority-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let active = root.join("home/.config/VectorTerm/current");
        let exe_dir = root.join("app");
        let cwd = root.join("work");
        std::fs::create_dir_all(&active).unwrap();
        std::fs::create_dir_all(&exe_dir).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();

        let bundled = exe_dir.join("config").join("config.lua");
        std::fs::create_dir_all(bundled.parent().unwrap()).unwrap();
        std::fs::write(&bundled, "return { shell = 'bundled' }").unwrap();
        std::fs::create_dir_all(cwd.join("config")).unwrap();
        std::fs::write(cwd.join("config").join("config.lua"), "return {}").unwrap();

        // 目录空 / 不存在 → 用软件目录里 config/config.lua 的默认配置
        assert_eq!(pick_active(&active), Ok(None), "空目录不算冲突");
        assert_eq!(
            pick_active(&root.join("home/.config/VectorTerm/missing")),
            Ok(None)
        );
        assert_eq!(
            default_config_path(Some(&exe_dir), &cwd),
            Some(bundled.clone()),
            "没有当前配置时应回落到软件目录的默认配置"
        );

        // 恰好一份 lua → 无论叫什么名字都认
        let mine = active.join("我的配置.lua");
        std::fs::write(&mine, "return { shell = 'mine' }").unwrap();
        assert_eq!(pick_active(&active), Ok(Some(mine.clone())));
        // 非 lua 文件不参与
        std::fs::write(active.join("notes.txt"), "hi").unwrap();
        assert_eq!(pick_active(&active), Ok(Some(mine.clone())));

        // 多份 lua → 冲突,错误信息里点名两份文件
        let other = active.join("another.lua");
        std::fs::write(&other, "return { shell = 'other' }").unwrap();
        let err = pick_active(&active).expect_err("多份 lua 必须报错");
        assert!(err.contains("只允许有一份"), "{err}");
        assert!(err.contains("我的配置.lua") && err.contains("another.lua"), "{err}");
        std::fs::remove_file(&other).unwrap();

        // 旧布局(exe 旁的 config.lua)仍然认,但优先级低于 config/config.lua
        let legacy = exe_dir.join("config.lua");
        std::fs::remove_file(&bundled).unwrap();
        std::fs::write(&legacy, "return { shell = 'legacy' }").unwrap();
        assert_eq!(
            default_config_path(Some(&exe_dir), &cwd),
            Some(legacy.clone()),
            "没有 config/config.lua 时应兼容 exe 旁的旧配置"
        );

        // 软件目录里两种布局都没有 → 退回工作目录的 config/config.lua(开发时用)
        std::fs::remove_file(&legacy).unwrap();
        assert_eq!(
            default_config_path(Some(&exe_dir), &cwd),
            Some(cwd.join("config").join("config.lua")),
            "软件目录没有配置时应退回工作目录"
        );

        // 显式 --config 最高优先级(即使文件不存在也照用,交给 load 报错)
        let explicit = root.join("nope.lua");
        assert_eq!(resolve_path_ex(Some(&explicit)), (Some(explicit), None));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn active_config_dir_follows_xdg_layout() {
        let path = active_config_dir().expect("应能从 USERPROFILE/HOME 推导");
        assert!(
            path.ends_with("VectorTerm/current") || path.ends_with("VectorTerm\\current"),
            "当前配置目录应为 .config/VectorTerm/current:{}",
            path.display()
        );
        assert!(keys_dir().unwrap().ends_with("keys"));
        assert!(path.to_string_lossy().contains(".config"));
    }

    /// `status_bar` 的外观项要能读进来,数值要有上下限。
    #[test]
    fn status_bar_config_parsed() {
        let src = r##"return {
          status_bar = {
            enable = true,
            keep_script = false,
            font_scale = 0.85,
            height = 40,
            padding = 12,
            item_spacing = 6,
            space_width = 0.4,
            text = "Hi",
          },
        }"##;
        let dir = std::env::temp_dir().join(format!("vectorterm-sb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.lua");
        let c = parse_with_theme(src, &path).expect("status_bar 应能解析");

        assert!(c.status_bar.enabled);
        assert!(!c.status_bar.keep_script);
        assert!((c.status_bar.font_scale - 0.85).abs() < 1e-9);
        assert_eq!(c.status_bar.height, Some(40.0));
        assert!((c.status_bar.padding - 12.0).abs() < 1e-9);
        assert!((c.status_bar.item_spacing - 6.0).abs() < 1e-9);
        assert!((c.status_bar.space_width - 0.4).abs() < 1e-9);
        assert_eq!(c.status_bar.text, "Hi");
        assert!(c.status_bar.has_text());

        // 越界的字号 / 高度被夹住或忽略,不会让状态栏画到不可用
        let clamp = parse_with_theme(
            "return { status_bar = { font_scale = 9.0, height = 3, space_width = 9.0 } }",
            &path,
        )
        .unwrap();
        assert!((clamp.status_bar.font_scale - 2.0).abs() < 1e-9, "字号上限 2.0");
        assert!(
            (clamp.status_bar.space_width - 2.0).abs() < 1e-9,
            "空格宽度上限 2.0"
        );
        // 不写时是 1.0 —— 和 wezterm 一个空格一个字宽的表现完全一致
        let bare = parse_with_theme("return { status_bar = { } }", &path).unwrap();
        assert!((bare.status_bar.space_width - 1.0).abs() < 1e-9, "默认 1.0");
        assert_eq!(clamp.status_bar.height, None, "太矮的高度当没写");
        assert!(!clamp.status_bar.has_text(), "没写 text 时不算有文字");

        // 旧版配置里的 `separator` / `modules` 现在没有意义了,但不能因为
        // 它们还在文件里就报错 —— 直接忽略,其余项照常生效。
        let legacy = parse_with_theme(
            "return { status_bar = { font_scale = 0.9, separator = ' | ', modules = { { name = 'x', code = 'return 1' } } } }",
            &path,
        )
        .unwrap();
        assert!((legacy.status_bar.font_scale - 0.9).abs() < 1e-9);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 四套配色是**编译进 exe** 的(`BUILTIN_THEMES`):分发单个 exe 也能写
    /// `theme = "chatgpt"`。这里确认名字和 `config/` 目录一一对应,且每份内嵌
    /// 源码都能独立解析出完整配色 —— 否则打包时漏一个文件,用户那边就是
    /// "主题不存在"。
    #[test]
    fn builtin_themes_are_embedded_and_parse() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("config");
        let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
            .expect("应有 config 目录")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "lua"))
            .map(|p| p.file_stem().unwrap().to_string_lossy().to_string())
            .filter(|n| n != "config")
            .collect();
        on_disk.sort();
        let mut embedded: Vec<String> = BUILTIN_THEMES.iter().map(|(n, _)| n.to_string()).collect();
        embedded.sort();
        assert_eq!(
            embedded, on_disk,
            "config/ 里每套配色都要 include_str! 进二进制,两边名单必须一致"
        );

        let lua = new_lua();
        crate::wezterm_compat::install(&lua).unwrap();
        for (name, src) in BUILTIN_THEMES {
            let tbl: mlua::Table = lua
                .load(*src)
                .eval()
                .unwrap_or_else(|e| panic!("内嵌主题 {name} 解析失败:{e}"));
            let c = parse_table(&tbl).unwrap_or_else(|e| panic!("内嵌主题 {name} 数据不完整:{e}"));
            assert_eq!(c.colors.ansi.len(), 16, "{name}:16 色要齐");
            assert!(c.colors.background.starts_with('#'), "{name}:要有画布色");
        }

        // 非法名字直接拒绝,不去碰文件系统
        assert!(builtin_theme("../etc/passwd").is_none());
        assert!(builtin_theme("no_such_theme_9f3a").is_none());
    }

    /// 磁盘优先、内嵌兜底:同名的用户副本能覆盖内置预设,没有文件时才用内置。
    #[test]
    fn theme_source_prefers_disk_then_builtin() {
        let dir = std::env::temp_dir().join(format!("vectorterm-theme-src-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let anchor = dir.join("config.lua");
        std::fs::write(dir.join("mine.lua"), "return { colors = { background = \"#010203\" } }")
            .unwrap();

        let (_, origin) = theme_source(&anchor, "mine").expect("磁盘上的主题应被找到");
        assert!(origin.ends_with("mine.lua"), "磁盘命中时来源应是路径:{origin}");

        // config/ 目录和内置里都没有这个名字
        assert!(theme_source(&anchor, "no_such_theme_9f3a").is_none());
        // 路径穿越式名字不合法,直接 None
        assert!(theme_source(&anchor, "../passwd").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 随程序分发的 `config/*.lua` 全部要能解析 —— 语法错一个字符,用户下次
    /// 启动就整份掉回内置默认值(配色、窗口尺寸全没了),所以让测试把目录扫一遍。
    ///
    /// 四套"整套配置"(deepseek / chatgpt / claude / gemini)还要求**只差颜色**:
    /// 窗口尺寸、字号、留白、状态栏参数必须一致,否则切主题会顺带换掉布局。
    #[test]
    fn shipped_configs_all_parse() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("config");
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .expect("应有 config 目录")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "lua"))
            .collect();
        files.sort();
        assert!(
            files.len() >= 4,
            "至少应有 config.lua + 四套配色,实际:{files:?}"
        );

        // 1) 目录里每个 lua 都要能解析
        let mut parsed: std::collections::HashMap<String, AppConfig> = Default::default();
        for path in &files {
            let src = std::fs::read_to_string(path).unwrap();
            let cfg = parse_with_theme(&src, path)
                .unwrap_or_else(|e| panic!("{} 解析失败:{e}", path.display()));
            let name = path.file_stem().unwrap().to_string_lossy().to_string();
            parsed.insert(name, cfg);
        }

        // 2) 四套整套配置:布局项一致,配色各自不同
        let base = parsed.get("deepseek").expect("缺少 config/deepseek.lua").clone();
        let mut backgrounds: Vec<String> = Vec::new();
        let mut accents: Vec<String> = Vec::new();
        for name in ["deepseek", "chatgpt", "claude", "gemini"] {
            let cfg = parsed
                .get(name)
                .unwrap_or_else(|| panic!("缺少 config/{name}.lua"));
            assert_eq!(
                (cfg.window_width, cfg.window_height),
                (Some(1250.0), Some(750.0)),
                "{name}:窗口尺寸要跟用户调好的那份一致"
            );
            assert_eq!(cfg.fonts.size, 14.0, "{name}:字号应一致");
            assert_eq!(
                cfg.fonts.line_height,
                Some(1.0),
                "{name}:行高倍数应一致(1250x750 + 14 号字下实测行距 20px)"
            );
            assert_eq!(cfg.fonts.cell_width, 1.0, "{name}:格宽倍数应一致");
            assert_eq!(cfg.padding, base.padding, "{name}:留白应一致");
            assert_eq!(cfg.status_bar.item_spacing, 0.0, "{name}");
            assert!(
                (cfg.status_bar.font_scale - 0.9).abs() < 1e-6,
                "{name}:状态栏字号倍数应一致"
            );
            assert!(
                (cfg.status_bar.space_width - 0.2).abs() < 1e-6,
                "{name}:空格宽度应一致"
            );
            assert_eq!(cfg.tab_bar.fancy, base.tab_bar.fancy, "{name}");
            assert!(cfg.tab_bar.enabled, "{name}");
            assert_eq!(cfg.colors.ansi.len(), 16, "{name}:要写满 16 色");
            assert!(
                cfg.colors.background.starts_with('#') && cfg.colors.foreground.starts_with('#'),
                "{name}:底色/主文本必须是 #rrggbb"
            );
            assert!(
                cfg.tab_bar.colors.active.bg.starts_with('#'),
                "{name}:活动标签要有底色"
            );
            backgrounds.push(cfg.colors.background.clone());
            accents.push(cfg.colors.cursor_bg.clone());
        }
        backgrounds.sort();
        backgrounds.dedup();
        accents.sort();
        accents.dedup();
        assert_eq!(backgrounds.len(), 4, "四套主题的底色不能重复");
        assert_eq!(accents.len(), 4, "四套主题的强调色不能重复");
    }
}
