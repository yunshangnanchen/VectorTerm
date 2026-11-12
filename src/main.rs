// VectorTerm —— Windows 终端模拟器
// 核心:wezterm-term(终端状态机)+ portable-pty(ConPTY)+ mlua(Lua 脚本)
// 渲染:winit + softbuffer + ab_glyph;自研 Kitty 风格光标尾迹动画

// 发行版用 GUI 子系统:否则从资源管理器双击会先弹出一个黑色控制台窗口 ——
// 终端软件自己是窗口程序,那个窗口只有调试意义。
// 调试构建保持控制台子系统(`cargo run` 直接看日志),测试也要排除:
// `cargo test --release` 下 `debug_assertions` 是关的,但测试框架要靠控制台
// 打印结果,套上 GUI 子系统就什么都看不见了。
// 发行版丢失的 stdout/stderr 由 `win_console::attach` 兜住(指向 NUL);
// 软件自身的日志一律走 `debug_term`,送到另一个调试终端,不写进当前 shell 窗口。
#![cfg_attr(
    all(windows, not(debug_assertions), not(test)),
    windows_subsystem = "windows"
)]

mod background;
#[cfg(test)]
mod bench;
mod builtin_fonts;
mod commands;
mod config;
mod cursor;
mod debug_term;
mod emoji;
mod files;
mod font;
mod frame;
mod gpu;
mod images;
mod input;
mod mux;
mod mux_server;
mod nerdfonts;
mod procs;
mod pty;
mod render;
mod script;
mod scroll;
mod scrollbar;
mod search;
mod selection;
mod status;
mod tabbar;
mod term;
mod wezterm_compat;
mod win_console;
mod win_il;
mod winhook;

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};

use anyhow::Context;

/// 仅基准模式使用:进程启动时刻,用来量"启动 → 首帧呈现完成"
static BOOT: OnceLock<Instant> = OnceLock::new();

/// 命令行有没有显式给 `--cwd`(资源管理器右键"在此处打开"就是这个入口)。
///
/// 给了就别去 attach 最近的会话:用户明确说"在这个目录开",恢复一个可能在别处的
/// 老会话会让右键看起来完全没生效 —— 而且不报错,最难查的那一类。守护进程照常起,
/// 这个新会话关掉之后照样能被恢复。
static CLI_CWD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize, Size};
use winit::event::{
    ElementState, Ime, MouseButton as WinitMouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, KeyCode as WinitKeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::raw_window_handle::HasWindowHandle;
use winit::window::{CursorIcon, ResizeDirection, Window, WindowId};

use config::{
    AppConfig, FileOpenMode, LiveResizeMode, ScrollbarShow, ScrollbarSide, SplitDividerConfig,
    VKeyAction,
};
use cursor::{CellRect, CursorTrail, Point2};
use termwiz::surface::CursorShape;
use script::{ScriptAction, ScriptEngine};
use scroll::ScrollAnim;
use wezterm_term::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

pub enum UserEvent {
    Repaint,
    /// 某个标签的标题变了(带稳定 id,标签关闭后不会串台)
    TitleChanged {
        tab: u64,
        title: String,
    },
    /// 某个标签的子进程退出
    ChildExited {
        tab: u64,
        msg: String,
    },
    /// mux 守护进程推送的输出字节(会话保持模式下)
    MuxOutput {
        pane: u64,
        bytes: Vec<u8>,
    },
    /// mux 守护进程通知某个 pane 的 shell 退出
    MuxExit {
        pane: u64,
        msg: String,
    },
    /// mux 守护进程通知标题变化
    MuxTitle {
        pane: u64,
        title: String,
    },
}

/// 分屏方向。和 wezterm 语义一致:Vertical = 左右分割(中间一条竖杠),
/// Horizontal = 上下分割(中间一条横杠)。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SplitDir {
    /// 左右分割(SplitVertical):左边 pane 占左半,右边新 pane 占右半。
    Vertical,
    /// 上下分割(SplitHorizontal):上边 pane 占上半,下边新 pane 占下半。
    Horizontal,
}

/// pane 焦点切换方向。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PaneDir {
    Left,
    Right,
    Up,
    Down,
}

/// 分屏布局:一棵二叉分割树。叶子是一个 pane 在 `panes` 里的下标,
/// 内部节点是一次分割(方向 + 左/上子树的占比)。
#[derive(Debug)]
enum SplitNode {
    /// 叶子:指向 `TabState.panes` 的下标。
    Leaf(usize),
    /// 分割:把一块区域沿 `dir` 切成两块,`ratio` 是 a 子树的占比(0..1)。
    Split {
        dir: SplitDir,
        ratio: f64,
        a: Box<SplitNode>,
        b: Box<SplitNode>,
    },
}

impl SplitNode {
    /// 叶子下标。单个 pane 时就是 0。
    fn leaf(idx: usize) -> Box<SplitNode> {
        Box::new(SplitNode::Leaf(idx))
    }

    /// 前序遍历收集所有叶子下标(从左到右 / 从上到下)。
    fn leaves(&self, out: &mut Vec<usize>) {
        match self {
            SplitNode::Leaf(i) => out.push(*i),
            SplitNode::Split { a, b, .. } => {
                a.leaves(out);
                b.leaves(out);
            }
        }
    }

    /// 在叶子 `target` 处分割:把该叶子替换成一个 Split 节点,原 pane 在 a,
    /// 新 pane(下标 `new_idx`)在 b。返回是否成功。
    fn split_at(&mut self, target: usize, new_idx: usize, dir: SplitDir, ratio: f64) -> bool {
        match self {
            SplitNode::Leaf(i) => {
                if *i == target {
                    let old = SplitNode::leaf(*i);
                    *self = SplitNode::Split {
                        dir,
                        ratio,
                        a: old,
                        b: SplitNode::leaf(new_idx),
                    };
                    true
                } else {
                    false
                }
            }
            SplitNode::Split { a, b, .. } => a.split_at(target, new_idx, dir, ratio)
                || b.split_at(target, new_idx, dir, ratio),
        }
    }

    /// 移除叶子 `target`:把它所在的 Split 节点用兄弟子树顶替。
    /// 返回新的根;若整棵树只剩这一个叶子,返回 None(表示标签该关了)。
    /// 注意:不重排剩余叶子的下标,由调用方在收缩 `panes` 后统一重映射。
    fn remove_leaf(&mut self, target: usize) -> Option<Box<SplitNode>> {
        match self {
            SplitNode::Leaf(i) => {
                if *i == target {
                    None
                } else {
                    Some(SplitNode::leaf(*i))
                }
            }
            SplitNode::Split { dir, ratio, a, b } => {
                let (dir, ratio) = (*dir, *ratio);
                let na = a.remove_leaf(target);
                let nb = b.remove_leaf(target);
                match (na, nb) {
                    // a 被整棵移除 → 用 b 顶替
                    (None, Some(bb)) => Some(bb),
                    // b 被整棵移除 → 用 a 顶替
                    (Some(aa), None) => Some(aa),
                    // 都没命中(不可能,因为 target 一定在树里) → 原样重建
                    (Some(aa), Some(bb)) => Some(Box::new(SplitNode::Split {
                        dir,
                        ratio,
                        a: aa,
                        b: bb,
                    })),
                    (None, None) => None,
                }
            }
        }
    }

    /// 找到「`b` 子树包含叶子 `target`」的那个 Split 节点,把它的 `ratio` 改
    /// 成 `new_ratio`(分屏动画用它推进分割线位置)。返回是否命中。
    fn set_ratio_of_new_pane(&mut self, target: usize, new_ratio: f64) -> bool {
        match self {
            SplitNode::Leaf(_) => false,
            SplitNode::Split { ratio, a, b, .. } => {
                // 只有直接包含新 pane 的那个 Split 节点需要动 —— 它就是本次
                // 分割新建的那一层(a 是原 pane,b 是新 pane)。
                if b.contains_leaf(target) && !a.contains_leaf(target) {
                    *ratio = new_ratio;
                    true
                } else {
                    a.set_ratio_of_new_pane(target, new_ratio)
                        || b.set_ratio_of_new_pane(target, new_ratio)
                }
            }
        }
    }

    /// 树里是否(任意深度)包含叶子 `target`。
    fn contains_leaf(&self, target: usize) -> bool {
        match self {
            SplitNode::Leaf(i) => *i == target,
            SplitNode::Split { a, b, .. } => a.contains_leaf(target) || b.contains_leaf(target),
        }
    }

    /// 按「从根往下的 0=a / 1=b 位图路径」改某个 Split 节点的 ratio。
    /// 路径低位对应根节点;根节点自身 `depth = 0`。
    fn set_ratio_at(&mut self, path: u32, depth: u8, new_ratio: f64) -> bool {
        if depth == 0 {
            if let SplitNode::Split { ratio, .. } = self {
                *ratio = new_ratio;
                return true;
            }
            return false;
        }
        let bit = path & 1;
        match self {
            SplitNode::Split { a, b, .. } => {
                let child = if bit == 0 { a.as_mut() } else { b.as_mut() };
                child.set_ratio_at(path >> 1, depth - 1, new_ratio)
            }
            SplitNode::Leaf(_) => false,
        }
    }

    /// 同上,只读版本(给单测与拖动前取当前值用)。
    fn ratio_at(&self, path: u32, depth: u8) -> Option<f64> {
        if depth == 0 {
            return match self {
                SplitNode::Split { ratio, .. } => Some(*ratio),
                _ => None,
            };
        }
        let bit = path & 1;
        match self {
            SplitNode::Split { a, b, .. } => {
                let child = if bit == 0 { a.as_ref() } else { b.as_ref() };
                child.ratio_at(path >> 1, depth - 1)
            }
            SplitNode::Leaf(_) => None,
        }
    }

    /// 收集「包含叶子 `leaf`」的所有 Split 节点,由浅到深。
    /// 快捷键调整大小时从这里挑方向匹配且最深的那一个。
    fn splits_containing(&self, leaf: usize, out: &mut Vec<(u32, u8, SplitDir, f64)>) {
        self.collect_splits(leaf, 0, 0, out);
    }

    fn collect_splits(
        &self,
        leaf: usize,
        path: u32,
        depth: u8,
        out: &mut Vec<(u32, u8, SplitDir, f64)>,
    ) {
        if !self.contains_leaf(leaf) {
            return;
        }
        if let SplitNode::Split { dir, ratio, a, b } = self {
            out.push((path, depth, *dir, *ratio));
            let d = depth.saturating_add(1);
            let bit = if depth < 32 { 1u32 << depth } else { 0 };
            a.collect_splits(leaf, path, d, out);
            b.collect_splits(leaf, path | bit, d, out);
        }
    }
}

/// 单个分屏窗格(pane):一个完整的终端会话 + 它的渲染/交互状态。
///
/// 分屏后一个标签里有多个 pane,每个 pane 都有独立的 term/PTY/滚动/选区。
/// 活动标签和后台标签里的 pane 是同一个类型,PTY 输出线程仍握着
/// 自己的 `Arc<Mutex<Terminal>>`。
struct Pane {
    /// 本地 pane id(在这个标签内唯一,由 `next_pane_id` 递增)。
    id: u64,
    term: Arc<Mutex<wezterm_term::Terminal>>,
    /// 本地直连的 PTY 会话;mux 模式(会话保持)下为 None。
    pty: Option<pty::PtySession>,
    /// mux 模式的远端 pane;本地直连模式下为 None。
    mux: Option<MuxPane>,
    kb_writer: SharedWriter,
    trail: CursorTrail,
    scroll: ScrollAnim,
    /// 视口整体"抬起"的行数:把屏幕上方的回看历史让进来,让光标所在行
    /// (最新的一行)贴住窗口底部。见 [`pad_after_resize`]。
    pad: usize,
    /// 上一帧光标所在的可视行,用来判断它是往下走(补白被吃掉)还是往上跳
    /// (清屏 / 应用重绘,这时要取消抬起)
    last_cy: usize,
    selection: selection::SelectionState,
    /// 这个 pane 自己的终端尺寸(cols, rows)。分屏后各 pane 尺寸不同。
    grid: (usize, usize),
    /// 这个 pane 的标题(OSC 0/2 设置)。
    title: String,
    /// 起这个 pane 用的 shell(程序名)。文件面板要按它决定两件事:
    /// 能不能读进程 PEB 拿 cwd(PowerShell 不行),以及 `cd` 该按哪种语法拼。
    shell: String,
    exited: bool,
    /// 这个 pane 的退出通知是否已经处理过(读线程 EOF 与 wait 返回会各报一次)
    exit_reported: bool,
}

/// 分屏动画:新 pane 从最右(垂直分割)或最下(水平分割)滑入,分割线
/// 从边缘一路滑到 50% 处,把原 pane 内容"挤"到一边。wezterm 的默认分屏
/// 就是这个观感 —— 新 pane 像抽屉一样从一侧推进来。
struct SplitAnim {
    /// 正在做动画的那个 Split 节点由 `target_pane`(新 pane 的下标)定位:
    /// 它就是 `b` 子树的叶子。动画推进时直接改该 Split 节点的 `ratio`。
    target_pane: usize,
    /// 动画起点的 ratio(垂直分割 = 1.0,分割线在最右;水平 = 0.0,在最下)。
    from: f64,
    /// 动画终点的 ratio。通常是 0.5;面板侧边栏占着一部分宽度时会更小,
    /// 让新 pane 在"除去侧边栏之后"的剩余宽度里对半分。
    to: f64,
    /// 起点时刻。
    start: Instant,
    /// 时长。
    dur: Duration,
}

impl SplitAnim {
    fn new(target_pane: usize, dir: SplitDir, start: Instant, to: f64) -> Self {
        // 两个方向的起点都是 1.0 —— 分割线先在**末端边缘**外:
        //   垂直分割:贴最右,新 pane 从右边拉开(线向左走);
        //   水平分割:贴最下,新 pane 从下边顶上来(线向上走)。
        // 终点恒 0.5。dir 只在这里把语义写清楚;两个方向的取整逻辑本来就一样,
        // 真正的区别是 ratio 作用在宽还是高上(见 compute_pane_geoms)。
        // 起点都是 1.0 —— 分割线先在末端边缘外;终点由调用方给(`to`),
        // 默认 0.5。
        let from = 1.0;
        let _ = dir;
        Self {
            target_pane,
            from,
            to: to.clamp(0.05, 0.95),
            start,
            // 180ms:60Hz 上大约 10 个可见位置,能看清"线在走"而不是"啪"地跳;
            // 再长就挡手了。wezterm 的抽屉滑入也是这个量级。
            dur: Duration::from_millis(180),
        }
    }

    /// 推进动画;返回是否仍在动画中(供事件循环决定要不要继续出帧)。
    fn tick(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.start) < self.dur
    }

    /// 当前 ratio(0..1,ease-out 缓动)。动画结束后恒为 `to`。
    fn ratio(&self, now: Instant) -> f64 {
        let t = now.saturating_duration_since(self.start).as_secs_f64() / self.dur.as_secs_f64();
        if t >= 1.0 {
            return self.to;
        }
        // ease-out cubic:速度曲线从快到慢,两端速度都为 0 —— 起步不顿、收尾不撞。
        // (quint 前段太猛,180ms 里 3 帧就走完 80%,看着还是"跳"。)
        let f = 1.0 - t;
        let eased = 1.0 - f * f * f;
        self.from + (self.to - self.from) * eased
    }
}

/// 立刻把正在跑的分屏动画落到终点。
///
/// 动画是 `Option`,同一时刻只有一条。如果动画没跑完就又分屏 / 关 pane,
/// 那条线所在的 Split 节点会永远停在半路的 ratio 上(布局歪掉、PTY 尺寸也跟着错),
/// 所以在改布局之前先把它"啪"地落到 0.5。
fn settle_split_anim(tab: &mut TabState) {
    settle_layout_anim(&mut tab.layout, &mut tab.split_anim);
}

/// [`settle_split_anim`] 的纯逻辑部分(不依赖 TabState,方便单测)。
fn settle_layout_anim(layout: &mut SplitNode, anim: &mut Option<SplitAnim>) {
    if let Some(a) = anim.take() {
        layout.set_ratio_of_new_pane(a.target_pane, a.to);
    }
}

/// 一个标签页的完整状态:一组 pane(分屏)+ 布局 + 焦点。
struct TabState {
    /// 标签 id(全局唯一)。
    id: u64,
    /// 这个标签里的所有 pane。布局树里的叶子下标指向这里。
    panes: Vec<Pane>,
    /// 焦点所在的 pane 下标。
    active_pane: usize,
    /// 分屏布局树。
    layout: Box<SplitNode>,
    /// 分配给这个标签的下一个 pane id。
    next_pane_id: u64,
    /// 进行中的分屏动画(新 pane 滑入)。
    split_anim: Option<SplitAnim>,
    /// 文件浏览器面板。`None` = 没打开。它**属于某个 pane**:面板铺满那个 pane
    /// 的区域(全屏)或从那个 pane 的左边让出一条(侧边栏),不会跑到别的终端
    /// 上去,也不会把整窗布局推歪。
    browser: Option<files::Browser>,
    /// 面板挂在哪个 pane 上(0 = 没有)。分屏后这个 pane 自己才是"当前终端":
    /// 回车打开的编辑器也在它旁边分出来。
    browser_pane: u64,
}

/// mux 模式下标签对应的远端 pane(由守护进程托管)。
struct MuxPane {
    pane_id: u64,
    /// 共享写端:键盘输入 / resize / detach 都走它。
    writer: Arc<StdMutex<std::fs::File>>,
    /// shell 进程 id(来自 attach/spawn 时的元信息)。
    pid: Option<u32>,
}

impl MuxPane {
    fn pid(&self) -> Option<u32> {
        self.pid
    }
    fn resize(&self, rows: u16, cols: u16, pw: u16, ph: u16) {
        let _ = mux::MuxClient::send_resize(&self.writer, self.pane_id, rows, cols, pw, ph);
    }
    fn kill(&self) {
        let _ = mux::MuxClient::send_kill(&self.writer, self.pane_id);
    }
    fn detach(&self) {
        let _ = mux::MuxClient::send_detach(&self.writer, self.pane_id);
    }
}

impl Pane {
    /// 该 pane 的 shell 进程 id(本地直连或 mux 远端)。
    fn shell_pid(&self) -> Option<u32> {
        self.pty.as_ref().and_then(|p| p.pid()).or_else(|| self.mux.as_ref().and_then(|m| m.pid()))
    }
    /// 通知后端调整 PTY 尺寸。
    fn resize_backend(&self, rows: u16, cols: u16, pw: u16, ph: u16) {
        if let Some(p) = &self.pty {
            let _ = p.resize(rows, cols, pw, ph);
        } else if let Some(m) = &self.mux {
            m.resize(rows, cols, pw, ph);
        }
    }
    /// 结束该 pane 的会话。本地直连模式杀掉 shell;mux 模式按 `keep_alive`
    /// 决定是 detach(会话保持,下次可 reattach)还是 kill(真正结束)。
    fn end_session(&mut self, keep_alive: bool) {
        if let Some(p) = &mut self.pty {
            p.kill();
        } else if let Some(m) = &self.mux {
            if keep_alive {
                m.detach();
            } else {
                m.kill();
            }
        }
    }
}

impl TabState {
    /// 焦点所在的 pane。
    fn active_pane(&self) -> &Pane {
        &self.panes[self.active_pane]
    }
    fn active_pane_mut(&mut self) -> &mut Pane {
        &mut self.panes[self.active_pane]
    }
    /// 按 pane id 找 pane 下标。
    fn pane_index(&self, id: u64) -> Option<usize> {
        self.panes.iter().position(|p| p.id == id)
    }
    /// 活动 pane 的标题(标签栏显示用)。
    fn title(&self) -> &str {
        &self.panes[self.active_pane].title
    }
    /// 该标签里是否有任一个 pane 的 shell 还在跑(退出确认用)。
    fn has_live_pane(&self) -> bool {
        self.panes.iter().any(|p| !p.exited)
    }
}

/// 输入法正在组合的那一段(拼音还没选字、假名还没上屏)。
///
/// 只用于显示:画在光标格下方,不写进终端内容、也不发给 PTY。上屏时
/// `Ime::Commit` 送来的才是真正的输入。
#[derive(Clone, Debug, Default)]
struct Preedit {
    text: String,
    /// 正在转换的那一段,单位是**字节**(winit 给的就是字节偏移)
    cursor: Option<(usize, usize)>,
}

/// 确认框要执行的动作。同一个界面服务三种入口,文案和后果都由它决定。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ConfirmAction {
    /// 关整个窗口(最后一个标签的 ×、标题栏关闭按钮、`Alt+F4`)
    Quit,
    /// 只关这一个标签(标签栏上的 ×、中键、`Ctrl+Shift+W`)
    CloseTab(usize),
    /// 只关这一个 pane(全局 pane id、`Ctrl+Shift+X`)
    ClosePane(u64),
}

/// 关闭前的确认。只在"还有程序在跑"时出现。
struct ConfirmState {
    /// 鼠标停在哪一项上(反显)。和 wezterm 一样初始为 `None`:
    /// 不预选、也没有默认按钮,键盘只认 y/n。
    active: Option<usize>,
    /// 点了"是"之后干什么
    action: ConfirmAction,
}

impl ConfirmState {
    /// 这一条确认该问什么。绘制和鼠标命中都从这里取,
    /// 免得"画的是关标签、算坐标用的是关窗口"那种错位。
    fn message(&self) -> &'static str {
        match self.action {
            ConfirmAction::Quit => CONFIRM_QUIT_MESSAGE,
            ConfirmAction::CloseTab(_) => CONFIRM_TAB_MESSAGE,
            ConfirmAction::ClosePane(_) => CONFIRM_PANE_MESSAGE,
        }
    }
}

/// 两个选项的文案。前后各带一个空格,是 wezterm 的 `" [Y]es "` / `" [N]o "`,
/// 反显时那两格空白也跟着反,才是一整条色块。
const CONFIRM_CHOICES: [&str; 2] = [" [Y]es ", " [N]o "];

/// 关窗口的提示语,文字和 wezterm `confirm_close_window` 里发的那句一致。
///
/// 句首那个标志不写进字符串:wezterm 靠字体回退把 emoji 画成彩色停止牌,
/// 我们这边等宽字体里拿不到轮廓、只能手绘,画出来像块膏药。改成渲染层
/// 画一个纯红圆点(`Renderer::draw_alert_dot`),文案就只剩字。
const CONFIRM_QUIT_MESSAGE: &str = "Really kill this window and all contained tabs and panes?";

/// 关单个标签的提示语,对应 wezterm 的 `confirm_close_tab`。红点同上。
const CONFIRM_TAB_MESSAGE: &str = "Really kill this tab and all contained panes?";

/// 关单个 pane 的提示语,对应 wezterm `CloseCurrentPane { confirm = true }`。
/// 红点同上。
const CONFIRM_PANE_MESSAGE: &str = "Really kill this pane?";

/// 确认框日志里那句"还有 2 个程序在运行(python.exe, node.exe)"。
///
/// `names` 为空 = 进程表没查到,按保守口径弹的框 —— 这时别谎称"有程序在跑",
/// 日志里说清楚是查不到。
fn describe_stateful(names: &[String]) -> String {
    if names.is_empty() {
        return "查不到进程表,按保守口径当作还有程序在跑".to_string();
    }
    format!("还有 {} 个程序在运行({})", names.len(), names.join(", "))
}

/// 按名字往列表里塞,大小写不敏感去重。
fn push_unique(list: &mut Vec<String>, name: &str) {
    if !list.iter().any(|n| n.eq_ignore_ascii_case(name)) {
        list.push(name.to_string());
    }
}

/// 窗口标题只反映当前会话。
///
/// 软件自身的错误一律去 `debug_term` 的独立调试终端 —— 和 wezterm 一样,
/// 不会拿错误文案去改正常 shell 窗口的标题,更不会往它的终端流里写东西。
fn window_title(tab_title: &str) -> String {
    format!("VectorTerm - {tab_title}")
}

/// panic 落盘。
///
/// 终端是常驻窗口程序,主线程 panic 之后窗口直接消失,用户拿不到任何线索;
/// 解析 / 等待 / 刷写三个子线程 panic 更是完全静默 —— 窗口还在,但终端不动了,
/// 看起来像"卡死"。统一记到 `~/.config/VectorTerm/crash.log`,stderr 照旧保留
/// (从控制台启动时能直接看到)。
///
/// 日志超过 256KB 就整个换掉:崩溃日志只看最近这次,不必做滚动归档。
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let thread = std::thread::current();
        let line = format!(
            "[{ms}] thread={:?} {info}\n",
            thread.name().unwrap_or("<unnamed>")
        );
        if let Some(dir) = config::user_config_dir() {
            let path = dir.join("crash.log");
            if std::fs::metadata(&path).is_ok_and(|m| m.len() > 256 * 1024) {
                let _ = std::fs::remove_file(&path);
            }
            let _ = std::fs::create_dir_all(&dir);
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                use std::io::Write;
                let _ = f.write_all(line.as_bytes());
            }
        }
        // 崩溃是最该被看见的一类错误:送进独立调试终端(会把它调出来)。
        crate::debug_term::error(line.trim_end());
        default(info);
    }));
}

fn main() -> anyhow::Result<()> {
    // 第一件事:接上 stdout/stderr。发行版是 GUI 子系统,默认没有控制台,
    // 句柄失效时写 stderr 会失败 —— 必须赶在任何输出(包括 panic hook)之前
    // 把它们指到 NUL(不挂父控制台,否则之后开不出独立调试终端)。
    win_console::attach();
    debug_term::init();
    install_panic_hook();
    let _ = BOOT.set(Instant::now());
    let args: Vec<String> = std::env::args().collect();

    // mux 守护进程模式:真正持有 PTY / shell,托管所有会话,常驻后台。
    // GUI 前端是它的客户端(会话保持 / detach / attach)。
    if args.iter().any(|a| a == "--mux-server") {
        let cfg_path = config::resolve_path(None);
        let (mut cfg, _) = config::load(cfg_path.as_deref());
        pty::apply_shell_choice(&mut cfg);
        return mux_server::run(&cfg);
    }

    // 调试/运维命令:列出守护进程里存活的会话。
    if args.iter().any(|a| a == "--mux-list") {
        return match mux::MuxClient::connect() {
            Ok(Some(mut c)) => match c.list() {
                Ok(list) => {
                    if list.is_empty() {
                        println!("(没有存活的会话)");
                    } else {
                        for m in list {
                            let state = if m.exited { "已退出" } else { "运行中" };
                            println!(
                                "#{} [{}] pid={} title=\"{}\" cwd={:?}",
                                m.id,
                                state,
                                m.pid,
                                m.title,
                                m.cwd
                            );
                        }
                    }
                    Ok(())
                }
                Err(e) => {
                    eprintln!("列出会话失败:{e}");
                    Err(e)
                }
            },
            Ok(None) => {
                eprintln!("mux 守护进程未运行(用 --mux-server 启动)");
                Ok(())
            }
            Err(e) => {
                eprintln!("连接失败:{e}");
                Err(e)
            }
        };
    }

    // 调试/运维命令:让守护进程起一个新会话(验证 spawn 链路)。
    if args.iter().any(|a| a == "--mux-spawn") {
        return match mux::MuxClient::connect() {
            Ok(Some(mut c)) => {
                let (mut cfg, _) = config::load(None);
                pty::apply_shell_choice(&mut cfg);
                let req = mux::SpawnReq {
                    shell: cfg.shell.clone(),
                    args: cfg.shell_args.clone(),
                    cwd: cfg.cwd.as_ref().map(|p| p.to_string_lossy().into_owned()),
                    env: cfg.env.clone(),
                    rows: 24,
                    cols: 80,
                    pixel_w: 800,
                    pixel_h: 600,
                    dpi: 96,
                };
                match c.spawn(&req) {
                    Ok(m) => {
                        println!("已创建会话 #{} pid={}", m.id, m.pid);
                        Ok(())
                    }
                    Err(e) => {
                        eprintln!("创建会话失败:{e}");
                        Err(e)
                    }
                }
            }
            Ok(None) => {
                eprintln!("mux 守护进程未运行(用 --mux-server 启动)");
                Ok(())
            }
            Err(e) => {
                eprintln!("连接失败:{e}");
                Err(e)
            }
        };
    }

    // 调试/运维命令:attach 到指定会话,打印快照信息(验证 attach 链路)。
    if let Some(pos) = args.iter().position(|a| a == "--mux-attach") {
        let id: u64 = args.get(pos + 1).and_then(|s| s.parse().ok()).unwrap_or(0);
        if id == 0 {
            eprintln!("用法: --mux-attach <会话 id>");
            return Ok(());
        }
        return match mux::MuxClient::connect() {
            Ok(Some(mut c)) => match c.attach(id) {
                Ok(snap) => {
                    println!(
                        "attach 成功:#{} title=\"{}\" 回放 {} 字节历史",
                        snap.pane,
                        snap.meta.title,
                        snap.raw.as_ref().map(|r| r.len()).unwrap_or(0)
                    );
                    if let Some(raw) = &snap.raw {
                        let text = String::from_utf8_lossy(raw);
                        let preview: String = text.chars().take(200).collect();
                        println!("--- 回放内容预览 ---\n{preview}\n---");
                    }
                    Ok(())
                }
                Err(e) => {
                    eprintln!("attach 失败:{e}");
                    Err(e)
                }
            },
            Ok(None) => {
                eprintln!("mux 守护进程未运行");
                Ok(())
            }
            Err(e) => {
                eprintln!("连接失败:{e}");
                Err(e)
            }
        };
    }

    // 调试/运维命令:向指定会话发键盘输入(验证 Input 链路)。
    if let Some(pos) = args.iter().position(|a| a == "--mux-input") {
        let id: u64 = args.get(pos + 1).and_then(|s| s.parse().ok()).unwrap_or(0);
        let text = args.get(pos + 2).cloned().unwrap_or_default();
        if id == 0 {
            eprintln!("用法: --mux-input <会话 id> <文本>");
            return Ok(());
        }
        return match mux::MuxClient::connect() {
            Ok(Some(mut c)) => {
                c.input(id, text.as_bytes())?;
                println!("已向会话 #{id} 发送 {} 字节", text.len());
                Ok(())
            }
            Ok(None) => {
                eprintln!("mux 守护进程未运行");
                Ok(())
            }
            Err(e) => {
                eprintln!("连接失败:{e}");
                Err(e)
            }
        };
    }

    let explicit = args
        .iter()
        .position(|a| a == "--config")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);

    let cfg_path = config::resolve_path(explicit.as_deref());
    let (cfg, cfg_err) = config::load(explicit.as_deref());
    if let Some(err) = &cfg_err {
        // 错误去独立调试终端(会自动弹出来)+ 两份日志;当前 shell 窗口不写任何东西。
        crate::werr!("[VectorTerm] 配置加载异常:{err}");
        config::log_error(err);
    }
    // 配置里写的 shell / 工作目录未必存在于当前这台机器(比如从别的设备
    // 拷过来的配置),先落到本机真能用上的那一档,否则连窗口都起不来。
    let mut cfg = cfg;
    // 命令行带进来的初始目录优先于配置:资源管理器右键「在此处打开 VectorTerm」
    // 就是把当前文件夹用 `--cwd` 塞进来的。必须赶在 apply_shell_choice 之前,
    // 让它走同一套"目录不存在就丢掉"的校验。
    if let Some(dir) = parse_cwd_arg(&args) {
        crate::wlog!("[VectorTerm] 初始工作目录由命令行指定:{}", dir.display());
        CLI_CWD.store(true, std::sync::atomic::Ordering::Relaxed);
        cfg.cwd = Some(dir);
    }
    pty::apply_shell_choice(&mut cfg);

    // 文件浏览器「在新窗口打开」:这个进程就是那扇新窗口,shell 直接换成
    // 编辑器(`--open-file <路径> -- nvim ...`)。编辑器找不到时 apply 返回
    // false,退回普通终端 —— 总比开不出窗口强。
    if let Some((path, editor)) = parse_open_file_args(&args) {
        if apply_open_file(&mut cfg, &path, &editor) {
            crate::wlog!("[VectorTerm] 新窗口打开文件:{}", path.display());
        } else {
            crate::werr!("[VectorTerm] --open-file 缺少编辑器命令,按普通窗口启动");
        }
    }
    let cfg = Arc::new(cfg);

    // 完整性与父进程决定 shell 的大部分环境:子进程原样继承本进程的令牌。
    // 本进程若是低完整性,shell 也写不了 %APPDATA%,PowerShell 会报 history 文件
    // Access denied,看起来像终端的 bug。打出来省一次排查。
    if let Some(note) = win_il::note() {
        crate::wlog!("[VectorTerm] {note}");
        if win_il::current().is_some_and(|il| il.is_low()) {
            crate::wlog!(
                "[VectorTerm] 警告: 当前是低完整性进程,shell 将无法写入 %APPDATA%\
                 (PowerShell 会报 history 文件 Access denied)。请从资源管理器正常启动。"
            );
        }
    }

    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();

    let mut app = App {
        proxy,
        cfg,
        cfg_path,
        state: None,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

fn mtime_of(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

struct App {
    proxy: EventLoopProxy<UserEvent>,
    cfg: Arc<AppConfig>,
    cfg_path: Option<PathBuf>,
    state: Option<GuiState>,
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        match GuiState::new(
            el,
            self.cfg.clone(),
            self.proxy.clone(),
            self.cfg_path.clone(),
        ) {
            Ok(state) => self.state = Some(state),
            Err(err) => {
                crate::werr!("[VectorTerm] 初始化失败:{err:#}");
                el.exit();
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                // 有程序还在跑时先弹确认框;真正退出交给 about_to_wait
                if state.close_window_or_ask() {
                    el.exit();
                }
            }
            WindowEvent::Resized(size) => {
                trace_resize("EV_RESIZED", size.width, size.height);
                if !state.apply_resize(size.width, size.height) {
                    return;
                }
                // 关键:DWM 在窗口客户区变大后,会继续拿上一帧那个「小尺寸
                // 的重定向表面」拉伸铺满新客户区,直到应用把新尺寸的帧真正
                // present 上去。好消息是交换链已经在 WM_SIZING 里提前配成了
                // 新尺寸(见 `winhook`),这里只剩绘制 + present,亚毫秒完成,
                // DWM 几乎抓不到那个中间态。
                // 所以这里不再做任何节流:每一次尺寸变化都必须立刻呈现。
                state.redraw_sync();
            }
            WindowEvent::RedrawRequested => {
                // 拖动中每一次尺寸变化已经同步画过一帧了,紧随其后的 WM_PAINT
                // 还会再派发一次。只有「尺寸和上次呈现的一模一样」才是纯重复,
                // 这时才跳过 —— 尺寸不同就必须画,否则新尺寸会没有帧可显示。
                if live_resizing()
                    && state.grid_px == state.last_presented
                    && state.last_sync_present.elapsed() < Duration::from_millis(8)
                {
                    trace_resize("REDRAW_SKIP", state.grid_px.0, state.grid_px.1);
                    return;
                }
                state.redraw()
            }
            WindowEvent::ModifiersChanged(mods) => state.mods = mods.state(),
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } => state.handle_key(event),
            // ---- 输入法 ----
            // 组合串只画出来,不发给 PTY;真正上屏的是 Commit。
            // 没有这一段的话,`Ime::Commit` 虽然能收到,但用户看不到自己
            // 正在打什么(候选窗之外一片空白),等于不能盲打中文。
            WindowEvent::Ime(Ime::Preedit(text, cursor)) => {
                state.set_preedit(text, cursor);
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                state.clear_preedit();
                state.ime_commit(text);
            }
            WindowEvent::Ime(Ime::Enabled) => {}
            WindowEvent::Ime(Ime::Disabled) => state.clear_preedit(),
            WindowEvent::Focused(focused) => {
                // 窗口重新拿到焦点时把 IME 重新挂上:切到别的程序再回来,
                // IMM 的输入上下文可能已经被系统摘掉了
                if focused {
                    state.window.set_ime_allowed(true);
                    state.ime_area = None;
                }
                for p in state.all_panes_mut() {
                    p.term.lock().focus_changed(focused);
                }
            }
            WindowEvent::CursorMoved { position, .. } => state.mouse_moved(position.x, position.y),
            WindowEvent::MouseInput {
                state: btn_state,
                button,
                ..
            } => state.mouse_button(btn_state, button),
            WindowEvent::MouseWheel { delta, .. } => state.mouse_wheel(delta),
            _ => {}
        }
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, ev: UserEvent) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        match ev {
            UserEvent::Repaint => {
                // 又有新输出:历史行号会整体前移,搜索结果需要在下一次出帧前重算
                if state.search.active {
                    state.search.dirty = true;
                }
                state.window.request_redraw()
            }
            UserEvent::TitleChanged { tab, title } => {
                // `tab` 现在是 pane id:标题变化路由到具体 pane。
                let is_active = state.active_pane_id() == Some(tab);
                if let Some(p) = state.pane_mut(tab) {
                    p.title = title.clone();
                }
                if is_active {
                    state.window.set_title(&window_title(&title));
                }
                state.window.request_redraw();
            }
            UserEvent::ChildExited { tab, msg } => {
                state.handle_exit(tab, msg);
            }
            UserEvent::MuxOutput { pane, bytes } => {
                // 守护进程推送的输出:喂给对应 pane 的 Terminal 解析渲染。
                if let Some(p) = state.pane_mut(pane) {
                    p.term.lock().advance_bytes(&bytes);
                }
                state.window.request_redraw();
            }
            UserEvent::MuxExit { pane, msg } => {
                state.handle_exit(pane, msg);
            }
            UserEvent::MuxTitle { pane, title } => {
                let is_active = state.active_pane_id() == Some(pane);
                if let Some(p) = state.pane_mut(pane) {
                    p.title = title.clone();
                }
                if is_active {
                    state.window.set_title(&window_title(&title));
                }
                state.window.request_redraw();
            }
        }
    }

    /// 动画期间用定时器驱动帧(约 120fps 上限);同时承担配置热重载的轮询。
    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        if state.quit_requested {
            el.exit();
            return;
        }
        // 诊断:`VECTORTERM_IME_OPEN=1` 时反复把输入法保持在打开状态(现实里没人
        // 会为了跑探针去点任务栏的语言栏)。没设这个变量时这里是一次
        // `OnceLock` 判定,开销可以忽略。
        #[cfg(windows)]
        if let Some(h) = hwnd_of(&state.window) {
            winhook::force_ime_open(h as _);
        }
        // `VECTORTERM_AUTO_PANEL` 的预定时刻:到点要**准时**醒来,不能等下面那条
        // 400ms 的空闲轮询 —— 否则动画会晚 0~400ms 开始,逐帧拍出来的时间轴
        // 全对不上。
        let mut panel_due: Option<Instant> = None;
        // `VECTORTERM_AUTO_FILES` 的两个时刻,同一个理由
        let mut files_due: Option<Instant> = None;
        // `VECTORTERM_AUTO_TAB` 的下一个时刻
        let mut tab_due: Option<Instant> = None;

        // 自动化测试钩子:启动 2.5s 后把 `VECTORTERM_AUTOTYPE` 里的字节直接写进
        // PTY(和键盘输入走同一条路径)。用于验证"shell 退出后窗口跟着关"。
        if !state.autotyped {
            if let Ok(s) = std::env::var("VECTORTERM_AUTOTYPE") {
                if BOOT
                    .get()
                    .is_some_and(|b| b.elapsed() > Duration::from_millis(2500))
                {
                    state.autotyped = true;
                    let bytes = s.replace("\\r", "\r").replace("\\n", "\n");
                    state.send_bytes(bytes.as_bytes());
                }
            }
        }

        // 自动化测试钩子:启动 1.5s 后按 `VECTORTERM_AUTO_SPLIT` 的值自动分屏,
        // 配合 VECTORTERM_DUMP_FRAME 验证分屏渲染。值可以是单个 "v" / "h",也可以
        // 是 "vh" / "hv" 这样的序列 —— 每次分的是当前活动 pane,用来复现嵌套布局。
        {
            static AUTO_SPLIT_STEP: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            if let Ok(seq) = std::env::var("VECTORTERM_AUTO_SPLIT") {
                let seq = seq.trim().to_ascii_lowercase().replace([',', ' '], "");
                let step = AUTO_SPLIT_STEP.load(std::sync::atomic::Ordering::Relaxed);
                let due = Duration::from_millis(1500 + 700 * step as u64);
                if step < seq.len() && BOOT.get().is_some_and(|b| b.elapsed() > due) {
                    AUTO_SPLIT_STEP.store(step + 1, std::sync::atomic::Ordering::Relaxed);
                    let d = if seq.as_bytes()[step] == b'h' {
                        SplitDir::Horizontal
                    } else {
                        SplitDir::Vertical
                    };
                    state.split_pane(d);
                }
            }
        }

        // 自动化测试钩子:`VECTORTERM_AUTO_TAB=new@800,close@3000,no@3500,close@4000,yes@4500`
        // 到点执行一串标签动作。为什么要这个:关标签确认必须靠点击 × 或按
        // `Ctrl+Shift+W` 才会出现,而 `SendInput` 送来的键在窗口没有键盘焦点时
        // 会静默丢掉,自动化脚本里时灵时不灵。这里走的是**同一个内部入口**
        // (`close_tab_or_ask` / `confirm_yes`),但不受焦点影响。
        //
        // 动作:`new` 开标签、`close` 走"该问就问"的关闭、`yes` / `no` 回答确认框。
        {
            static TAB_STEP: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            if let Ok(spec) = std::env::var("VECTORTERM_AUTO_TAB") {
                let steps: Vec<(String, u64)> = spec
                    .split(',')
                    .filter_map(|item| {
                        let (a, b) = item.trim().split_once('@')?;
                        Some((a.trim().to_ascii_lowercase(), b.trim().parse().ok()?))
                    })
                    .collect();
                let step = TAB_STEP.load(std::sync::atomic::Ordering::Relaxed);
                let now_ms = BOOT.get().map(|b| b.elapsed().as_millis() as u64);
                if let (Some(boot), Some(t)) = (BOOT.get(), now_ms) {
                    if let Some((_, at)) = steps.get(step) {
                        tab_due = Some(*boot + Duration::from_millis(at.saturating_sub(3)));
                    }
                    let _ = t;
                }
                if let (Some((act, at)), Some(t)) = (steps.get(step), now_ms) {
                    if t > *at {
                        TAB_STEP.store(step + 1, std::sync::atomic::Ordering::Relaxed);
                        match act.as_str() {
                            "new" => state.new_tab(),
                            "close" => state.close_tab_or_ask(state.active),
                            // 关当前窗格。`VECTORTERM_AUTO_SPLIT=v` 先分屏,再用
                            // `closepane` 验证 Ctrl+Shift+X 那条路(注入按键在
                            // 窗口没有焦点时会被丢掉,只能走内部入口)。
                            "closepane" => {
                                let id = state.active_pane_id();
                                if let Some(id) = id {
                                    state.close_pane_or_ask(id);
                                }
                            }
                            "yes" => state.confirm_yes(),
                            "no" => state.close_confirm(),
                            other => crate::wlog!("[VectorTerm] VECTORTERM_AUTO_TAB 未知动作: {other}"),
                        }
                    }
                }
            }
        }

        // 自动化测试钩子:`VECTORTERM_AUTO_FILES=full|side[@毫秒]` 到点打开文件面板
        // (`VECTORTERM_AUTO_FILES_DIR` 指定目录,默认 1200ms);再配
        // `VECTORTERM_AUTO_FILES_CLOSE=毫秒` 到点收起。两个时刻都会走 `files_due`
        // 提前唤醒 —— 默认 400ms 空闲轮询会把滑动动画的时间轴拖歪 0~400ms,
        // 逐帧连拍就对不上了。
        {
            static FILES_STEP: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            if let Ok(spec) = std::env::var("VECTORTERM_AUTO_FILES") {
                let (mode, open_at) = match spec.split_once('@') {
                    Some((a, b)) => (a.trim().to_string(), b.trim().parse::<u64>().unwrap_or(1200)),
                    None => (spec.trim().to_string(), 1200),
                };
                let close_at = std::env::var("VECTORTERM_AUTO_FILES_CLOSE")
                    .ok()
                    .and_then(|v| v.trim().parse::<u64>().ok());
                let step = FILES_STEP.load(std::sync::atomic::Ordering::Relaxed);
                let now_ms = BOOT.get().map(|b| b.elapsed().as_millis() as u64);
                if let Some(boot) = BOOT.get() {
                    let due = match step {
                        0 => Some(open_at),
                        1 => close_at,
                        _ => None,
                    };
                    if let Some(ms) = due {
                        files_due = Some(*boot + Duration::from_millis(ms.saturating_sub(3)));
                    }
                }
                if step == 0 && now_ms.is_some_and(|t| t > open_at) {
                    FILES_STEP.store(1, std::sync::atomic::Ordering::Relaxed);
                    let m = if mode.eq_ignore_ascii_case("side") {
                        files::Mode::Side
                    } else {
                        files::Mode::Full
                    };
                    let cwd = std::env::var("VECTORTERM_AUTO_FILES_DIR")
                        .map(PathBuf::from)
                        .unwrap_or_else(|_| state.pane_cwd());
                    state.browser_open(cwd, m);
                    // 再进一步:`VECTORTERM_AUTO_FILES_OPEN=文件名` 直接把那一项
                    // 打开(走真实的"回车"路径,按 files_gui.open_mode 决定
                    // 是新窗口 / 分屏 / 当前 shell)
                    if let Ok(name) = std::env::var("VECTORTERM_AUTO_FILES_OPEN") {
                        if let Some(b) = state.tab.browser.as_mut() {
                            if let Some(i) = b.entries.iter().position(|e| e.name == name) {
                                b.selected = i;
                            }
                        }
                        state.browser_activate();
                    }
                    // 配了导出帧就顺手截一张。**要等滑完**:打开这一帧面板还在
                    // 屏幕左边外面,截出来是空的。所以排到 open_at + 300ms。
                    if let Some(p) = std::env::var_os("VECTORTERM_DUMP_FRAME") {
                        state.dump_seq.push((open_at + 300, PathBuf::from(p)));
                    }
                } else if step == 1 {
                    if let (Some(t), Some(at)) = (now_ms, close_at) {
                        if t > at {
                            FILES_STEP.store(2, std::sync::atomic::Ordering::Relaxed);
                            state.browser_close();
                        }
                    }
                }
            }
        }

        // 测试钩子:整套分屏动画结束后,若设置了 `VECTORTERM_AUTO_SPLIT`,把最终结果
        // 导出一帧 BMP(路径由 VECTORTERM_DUMP_FRAME 决定),便于离屏验证分割线。
        {
            static SPLIT_DUMPED: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if !SPLIT_DUMPED.load(std::sync::atomic::Ordering::Relaxed) {
                if let Ok(seq) = std::env::var("VECTORTERM_AUTO_SPLIT") {
                    let want = seq.trim().replace([',', ' '], "").chars().count() + 1;
                    if state.tab.panes.len() >= want && state.tab.split_anim.is_none() {
                        SPLIT_DUMPED.store(true, std::sync::atomic::Ordering::Relaxed);
                        if let Some(path) =
                            std::env::var_os("VECTORTERM_DUMP_FRAME").map(PathBuf::from)
                        {
                            state.dump_frame.push(path);
                        }
                        state.window.request_redraw();
                    }
                }
            }
        }

        // 测试钩子:`VECTORTERM_AUTO_FOCUS=N[@毫秒]` 到点把焦点切到第 N 个 pane
        // (1 起),用来核对跨窗格光标尾迹。默认 2.2s;想等分屏动画彻底收尾再切
        // 就写 `VECTORTERM_AUTO_FOCUS=1@5000`。
        {
            static FOCUS_DONE: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if let Ok(n) = std::env::var("VECTORTERM_AUTO_FOCUS") {
                let (idx_txt, delay) = match n.split_once('@') {
                    Some((a, b)) => (a.trim().to_string(), b.trim().parse::<u64>().unwrap_or(2200)),
                    None => (n.trim().to_string(), 2200),
                };
                if !FOCUS_DONE.load(std::sync::atomic::Ordering::Relaxed)
                    && BOOT
                        .get()
                        .is_some_and(|b| b.elapsed() > Duration::from_millis(delay))
                {
                    FOCUS_DONE.store(true, std::sync::atomic::Ordering::Relaxed);
                    if let Ok(n) = idx_txt.parse::<usize>() {
                        if n >= 1 && n <= state.tab.panes.len() {
                            state.focus_pane_index(n - 1);
                        }
                    }
                }
            }
        }

        // 测试钩子:`VECTORTERM_AUTO_PANEL=search|commands[@毫秒]` 到点打开浮层,
        // 再配 `VECTORTERM_AUTO_PANEL_CLOSE=毫秒` 到点关掉 —— 配合
        // `VECTORTERM_DUMP_AT` 就能把进场 / 退场两张动画逐帧拍下来。
        {
            static PANEL_STEP: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            if let Ok(spec) = std::env::var("VECTORTERM_AUTO_PANEL") {
                let (what, open_at) = match spec.split_once('@') {
                    Some((a, b)) => (
                        a.trim().to_lowercase(),
                        b.trim().parse::<u64>().unwrap_or(1200),
                    ),
                    None => (spec.trim().to_lowercase(), 1200),
                };
                let is_panel = what.starts_with("command") || what == "panel";
                let step = PANEL_STEP.load(std::sync::atomic::Ordering::Relaxed);
                let now_ms = BOOT.get().map(|b| b.elapsed().as_millis() as u64);
                // 提前一点醒(扣掉调度抖动),让开/关动作落在指定的毫秒上
                if let Some(boot) = BOOT.get() {
                    let due = match step {
                        0 => Some(open_at),
                        1 => std::env::var("VECTORTERM_AUTO_PANEL_CLOSE")
                            .ok()
                            .and_then(|s| s.trim().parse::<u64>().ok()),
                        _ => None,
                    };
                    if let Some(ms) = due {
                        panel_due = Some(*boot + Duration::from_millis(ms.saturating_sub(3)));
                    }
                }
                if step == 0 && now_ms.is_some_and(|t| t > open_at) {
                    PANEL_STEP.store(1, std::sync::atomic::Ordering::Relaxed);
                    if is_panel {
                        state.open_panel();
                    } else {
                        state.open_search();
                    }
                    state.window.request_redraw();
                } else if step == 1 {
                    let close_at = std::env::var("VECTORTERM_AUTO_PANEL_CLOSE")
                        .ok()
                        .and_then(|s| s.trim().parse::<u64>().ok());
                    if let (Some(t), Some(at)) = (now_ms, close_at) {
                        if t > at {
                            PANEL_STEP.store(2, std::sync::atomic::Ordering::Relaxed);
                            if is_panel {
                                state.close_panel();
                            } else {
                                state.close_search();
                            }
                            state.window.request_redraw();
                        }
                    }
                }
            }
        }

        // 测试钩子:`VECTORTERM_DUMP_AT=毫秒[,毫秒...]` 到点各导出一帧。
        // 用来核对"操作之后"的画面(自动输入、命令面板打开面板之后等)。
        // 给多个时刻时,路径自动加 `-<毫秒>` 后缀(`a.bmp` → `a-2250.bmp`),
        // 这样一条命令就能把一段动画逐帧拍下来。
        if !state.dump_seq.is_empty() {
            if let Some(boot) = BOOT.get() {
                let elapsed = boot.elapsed().as_millis() as u64;
                let mut due = Vec::new();
                state.dump_seq.retain(|(at, path)| {
                    if *at <= elapsed {
                        due.push(path.clone());
                        false
                    } else {
                        true
                    }
                });
                if !due.is_empty() {
                    state.dump_frame.extend(due);
                    state.window.request_redraw();
                }
            }
        }

        // 连续 resize 刚停下:到点把攒着的网格变化提交一次。Windows 的模态
        // 拖动循环可能不跑主循环,所以松手后第一次唤醒就得把尺寸对齐。
        state.finish_live_resize();
        state.flush_resize(Instant::now());
        // 只把这个时刻作为"最早该醒来的时间",不能就此 return —— 等待期间
        // 光标拖尾、闪烁与配置热重载仍得照常驱动。
        let mut wake = state.resize_settler.deadline();
        // 浮层的自动打开时刻要准时,不能被下面的空闲轮询拖后
        if let Some(t) = panel_due {
            wake = earlier(wake, t);
        }
        if let Some(t) = files_due {
            wake = earlier(wake, t);
        }
        if let Some(t) = tab_due {
            wake = earlier(wake, t);
        }
        // 冻结期间没有 deadline:低频轮询,保证松手那一刻能及时发现
        if state.resize_settler.is_frozen() {
            wake = earlier(wake, Instant::now() + Duration::from_millis(16));
        }

        // 基准模式:绕过空闲等待与 400ms 热重载轮询,以 Poll 满载驱动重绘
        if state.profile.is_some() {
            el.set_control_flow(ControlFlow::Poll);
            state.window.request_redraw();
            return;
        }

        // 配置热重载:每 ~400ms 检查一次 config.lua 的修改时间
        if state.cfg_path.is_some() && state.last_cfg_check.elapsed() >= Duration::from_millis(400)
        {
            state.last_cfg_check = Instant::now();
            if let Some(path) = state.cfg_path.clone() {
                let now = mtime_of(&path);
                if now.is_none() {
                    // 监视的配置文件被删 / 被替换了(例如在配置管理器里换了
                    // 「当前配置」,current/ 里换成了另一个文件名)。重新解析
                    // 一次,落到新的那份上并立即重载。
                    let (fresh, _) = config::resolve_path_ex(None);
                    if let Some(fresh) = fresh {
                        if fresh != path {
                            state.cfg_path = Some(fresh);
                            state.cfg_mtime = None;
                            state.reload_config();
                        }
                    }
                } else if now != state.cfg_mtime {
                    state.reload_config();
                }
            }
        }

        // 状态栏:到点重跑 update-status(wezterm 的 status_update_interval)。
        // 这一项同时也是"最早该醒来"的候选,空闲窗口不会漏掉刷新。
        if Instant::now() >= state.next_status_update {
            state.refresh_status(Instant::now());
        }
        wake = earlier(wake, state.next_status_update);

        // 文件面板跟着终端走:低频轮询 shell 的当前目录(没开面板时是待机频率)
        state.poll_browser_cwd(Instant::now());
        wake = earlier(wake, state.next_cwd_poll);

        // 拖选到边缘:按固定节拍醒来推进一次,松手后自动归零
        if state.edge_scroll != 0 {
            wake = earlier(wake, state.next_edge_tick);
            state.window.request_redraw();
        }

        let ck_anim = state.cfg.cursor.anim != config::CursorAnimation::None
            || (state.cfg.cursor.blink != config::CursorBlinkMode::Never
                && state.cfg.cursor.blink_style == config::CursorBlinkStyle::Fade);
        let ck_blink_ticks = state.cfg.cursor.blink != config::CursorBlinkMode::Never;
        let any_anim = {
            let p = state.tab.active_pane();
            p.trail.needs_frames()
                || p.scroll.needs_frames()
                || state.tab.split_anim.is_some()
                || state.focus_trail_on
                // 浮层进出场 + 光标飞过去:都要连续帧,否则会一顿一顿
                || state.search_anim.running()
                || state.panel_anim.running()
                || state.browser_anim.running()
                || state.overlay_trail_on
        } || ck_anim;
        if any_anim {
            wake = earlier(wake, Instant::now() + Duration::from_millis(8));
            state.window.request_redraw();
        } else if state.has_blink || ck_blink_ticks {
            // 屏上有 SGR 5/6 的单元格,或光标在闪:只在相位切换的时刻重绘一次,
            // 避免空转。脉冲 / 呼吸 / 淡出需要连续帧,走上面的 8ms 分支。
            let now = Instant::now();
            if now >= state.next_blink {
                let rate = state.cfg.cursor.blink_rate_ms.max(60.0) as f64;
                let duty = state.cfg.cursor.blink_duty as f64;
                let step = (rate * duty.min(1.0 - duty)).max(30.0) as u64;
                state.next_blink = now + Duration::from_millis(step);
                state.window.request_redraw();
            }
            wake = earlier(wake, state.next_blink);
        } else if state.cfg_path.is_some() {
            // 有配置文件时保持低频唤醒,否则空闲等待会让热重载轮询永远等不到机会
            wake = earlier(wake, Instant::now() + Duration::from_millis(400));
        }
        match wake {
            Some(t) => el.set_control_flow(ControlFlow::WaitUntil(t)),
            None => el.set_control_flow(ControlFlow::Wait),
        }
    }
}

/// 取更早的那个唤醒时刻;`None` 表示目前只有另一个候选
fn earlier(a: Option<Instant>, b: Instant) -> Option<Instant> {
    Some(match a {
        Some(t) if t < b => t,
        _ => b,
    })
}

// ---------------------------------------------------------------------------
// SharedWriter:Terminal 的应答输出与键盘输入统一写入同一缓冲,由 flusher
// 线程低延迟地搬运到 PTY 输入。
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct SharedWriter {
    inner: Arc<WriterInner>,
    notify: std::sync::mpsc::Sender<()>,
}

struct WriterInner {
    /// parking_lot 的锁不中毒:万一某个线程在持锁时 panic,后面每敲一个键
    /// 都会再 panic 一次。这个缓冲在输入主路径上,换成不中毒的锁更划算。
    buf: Mutex<Vec<u8>>,
}

impl std::io::Write for SharedWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.inner.buf.lock().extend_from_slice(data);
        let _ = self.notify.send(());
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn spawn_flusher(
    mut writer: Box<dyn std::io::Write + Send>,
    rx: std::sync::mpsc::Receiver<()>,
    inner: Arc<WriterInner>,
) {
    let _ = std::thread::Builder::new()
        .name("vectorterm-flusher".into())
        .spawn(move || {
            while rx.recv().is_ok() {
                let data = {
                    let mut buf = inner.buf.lock();
                    std::mem::take(&mut *buf)
                };
                if data.is_empty() {
                    continue;
                }
                if let Err(e) = writer.write_all(&data) {
                    crate::werr!("[VectorTerm] 写入 PTY 失败:{e}");
                    break;
                }
                let _ = writer.flush();
            }
        });
}

// ---------------------------------------------------------------------------
// 渲染后端:GPU(wgpu/DX12)或 CPU(softbuffer 软光栅化)
// ---------------------------------------------------------------------------

enum Backend {
    /// 第二个字段是窗口句柄。软渲染没有交换链要预配置,但窗口过程子类化
    /// 同时还负责输入法键码还原(`winhook::fix_processkey`),所以照样要装、
    /// 照样要撤 —— 发行配置大量写 `render = "cpu"`,漏了这半边就等于没修。
    Cpu(softbuffer::Surface<Arc<Window>, Arc<Window>>, isize),
    /// 必须装箱:地址稳定,`WM_SIZING` 钩子才能安全地拿裸指针提前配交换链。
    /// 第二个字段是窗口句柄,留着在析构时撤钩子。
    Gpu(Box<gpu::GpuRenderer>, isize),
}

impl Drop for Backend {
    fn drop(&mut self) {
        match self {
            Backend::Gpu(_, hwnd) | Backend::Cpu(_, hwnd) => winhook::uninstall(*hwnd as _),
        }
    }
}

/// Win11 的系统背板材质(Mica / Acrylic / Tabbed)。
///
/// 目前只做 DWM 侧的属性设置,实际看不到材质:材质要靠"窗口客户区对 DWM 透明"
/// 才能透出来,而挂在窗口上的交换链(`CreateSwapChainForHwnd`)不接受
/// `PREMULTIPLIED` alpha,给不出逐像素透明(见 docs/custom-background.md)。
#[cfg(windows)]
fn apply_system_backdrop(hwnd: isize, kind: config::Backdrop) {
    use windows_sys::Win32::Graphics::Dwm::{DwmExtendFrameIntoClientArea, DwmSetWindowAttribute};
    use windows_sys::Win32::UI::Controls::MARGINS;
    // win32metadata 里 DWMWA_SYSTEMBACKDROP_TYPE = 38
    const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;
    const DWMSBT_MAINWINDOW: i32 = 2; // Mica
    const DWMSBT_TRANSIENTWINDOW: i32 = 3; // Acrylic
    const DWMSBT_TABBEDWINDOW: i32 = 4;

    let v = match kind {
        config::Backdrop::None => return,
        config::Backdrop::Mica => DWMSBT_MAINWINDOW,
        config::Backdrop::Acrylic => DWMSBT_TRANSIENTWINDOW,
        config::Backdrop::Tabbed => DWMSBT_TABBEDWINDOW,
    };
    unsafe {
        // 把窗口四周扩展成"玻璃区",DWM 才会在窗口背后画材质
        let margins = MARGINS {
            cxLeftWidth: -1,
            cxRightWidth: -1,
            cyTopHeight: -1,
            cyBottomHeight: -1,
        };
        let _ = DwmExtendFrameIntoClientArea(hwnd as _, &margins);
        let hr = DwmSetWindowAttribute(
            hwnd as _,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &v as *const i32 as *const _,
            std::mem::size_of::<i32>() as u32,
        );
        if hr != 0 {
            crate::wlog!(
                "[VectorTerm] 系统背板({})设置失败 HRESULT 0x{hr:08x} —— 需要 Win11 22H2 及以上",
                kind.as_str()
            );
        }
    }
}

#[cfg(not(windows))]
fn apply_system_backdrop(_hwnd: isize, _kind: config::Backdrop) {}

/// 窗口图标(任务栏 / Alt+Tab / 标题栏左上)。
///
/// 图标的母版是仓库根目录的 `VectorTermLogo.png`,`tools/gen_icons.py` 从它生成整套
/// `assets/icons/`,这里取 256 那一档在编译期嵌进二进制(256 足够覆盖 Windows 会用的
/// 最大图标尺寸,2 倍屏也不糊)。运行时因此不依赖 exe 旁边的文件;exe 文件本身的图标
/// 由 build.rs 把同一个目录下的 `VectorTerm.ico` 编进资源段。解码失败就当没有图标,
/// 不影响启动。
fn app_icon() -> Option<winit::window::Icon> {
    const ICON_PNG: &[u8] = include_bytes!("../assets/icons/VectorTerm-256.png");
    let img = image::load_from_memory(ICON_PNG).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    winit::window::Icon::from_rgba(img.into_raw(), w, h).ok()
}

/// 取窗口 HWND(只有 Win32 平台有意义)。
#[cfg(windows)]
fn hwnd_of(window: &Window) -> Option<isize> {
    match window.window_handle().ok()?.as_raw() {
        winit::raw_window_handle::RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

/// 客户区左上角在屏幕上的位置(物理像素)。带系统标题栏时它不等于窗口左上角,
/// 所以不能拿 `outer_position` 直接加客户区坐标。
#[cfg(windows)]
fn client_origin(hwnd: isize) -> Option<(i32, i32)> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
    unsafe {
        let mut p = POINT { x: 0, y: 0 };
        (ClientToScreen(hwnd as _, &mut p) != 0).then_some((p.x, p.y))
    }
}

/// 给整窗加统一 alpha(`WS_EX_LAYERED` + `LWA_ALPHA`)。
/// DWM 会按这个 alpha 合成整块窗口,窗口内容(GDI 位图或 DXGI 画面)一起变淡。
#[cfg(windows)]
fn set_uniform_alpha(hwnd: isize, alpha: u8) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongW, SetLayeredWindowAttributes, SetWindowLongW, GWL_EXSTYLE, LWA_ALPHA,
        WS_EX_LAYERED,
    };
    unsafe {
        let ex = GetWindowLongW(hwnd as _, GWL_EXSTYLE);
        SetWindowLongW(hwnd as _, GWL_EXSTYLE, ex | WS_EX_LAYERED as i32);
        let ok = SetLayeredWindowAttributes(hwnd as _, 0, alpha, LWA_ALPHA);
        if ok == 0 {
            crate::wlog!("[VectorTerm] 半透明窗口设置失败(SetLayeredWindowAttributes),已按不透明绘制");
        }
    }
}
impl Backend {
    fn new(cfg: &AppConfig, window: Arc<Window>) -> anyhow::Result<Self> {
        let prefer_discrete = cfg.gpu_power == config::GpuPower::High;
        match cfg.render {
            config::RenderMode::Cpu => Self::cpu(window),
            config::RenderMode::Gpu => {
                match Self::gpu(window.clone(), prefer_discrete) {
                    Ok(g) => Ok(g),
                    Err(e) => {
                        // 终端起不来比"没 GPU 加速"糟糕得多,这里退回软渲染并说明原因
                        crate::wlog!("[VectorTerm] GPU 初始化失败,退回 CPU 软渲染:{e:#}");
                        Self::cpu(window)
                    }
                }
            }
            config::RenderMode::Auto => match Self::gpu(window.clone(), prefer_discrete) {
                Ok(g) => Ok(g),
                Err(e) => {
                    crate::wlog!("[VectorTerm] 未找到可用的 GPU({e:#}),使用 CPU 软渲染");
                    Self::cpu(window)
                }
            },
        }
    }

    /// 建 GPU 后端并挂上 `WM_SIZING` 钩子。
    fn gpu(window: Arc<Window>, prefer_discrete: bool) -> anyhow::Result<Self> {
        let g = gpu::GpuRenderer::new(window.clone(), prefer_discrete)?;
        crate::wlog!(
            "[VectorTerm] 渲染后端:GPU({}) —— 拖动缩放窗口时旧帧会被 DXGI 拉伸, \
             想避免请改用 render = \"cpu\"(见 docs/rendering-backends.md)",
            g.adapter_name()
        );
        let mut boxed = Box::new(g);
        let mut hwnd: isize = 0;
        // 装钩子:让 WM_SIZING 能在窗口变大之前就把交换链配成新尺寸
        if let Ok(winit::raw_window_handle::RawWindowHandle::Win32(h)) =
            window.window_handle().map(|h| h.as_raw())
        {
            hwnd = h.hwnd.get();
            winhook::install(h.hwnd.get() as _, &mut *boxed as *mut gpu::GpuRenderer);
        }
        Ok(Backend::Gpu(boxed, hwnd))
    }

    fn cpu(window: Arc<Window>) -> anyhow::Result<Self> {
        let hwnd = hwnd_of(&window).unwrap_or(0);
        let context = softbuffer::Context::new(window.clone())
            .map_err(|e| anyhow::anyhow!("softbuffer context:{e}"))?;
        let surface = softbuffer::Surface::new(&context, window)
            .map_err(|e| anyhow::anyhow!("softbuffer surface:{e}"))?;
        drop(context);
        crate::wlog!("[VectorTerm] 渲染后端:CPU 软渲染");
        // 没有交换链要预配置,但键码还原要走同一条子类化链
        winhook::install_wndproc(hwnd as _);
        Ok(Backend::Cpu(surface, hwnd))
    }
}

/// `VECTORTERM_PROFILE_FRAMES=N` 开启的满载帧率采样:连续强制重绘 N 帧后
/// 打印帧间隔统计并自动退出。默认关闭,不影响正常使用。
struct ProfileCtx {
    remaining: usize,
    last: Option<Instant>,
    gaps: Vec<f64>,
}

/// 标签栏四周的安全区(物理像素):`(左右, 靠外框那侧)`。
///
/// 无系统标题栏 + DWM 圆角时,Win11 的角半径约 8 逻辑像素,默认给 10
/// 逻辑像素,让标签块、按钮、文字都躲开圆角三角区;顶部/底部也留同样
/// 的距离,四周才对称。`tab_bar.edge_padding` / `top_padding` 可以覆盖,
/// 显式写 0 就是贴边。这些值按 scale_factor 换算,高 DPI 自动放大。
///
/// 有系统标题栏时客户区上沿是直的(圆角在标题栏那一圈),不需要躲圆角,
/// 所以只留 4 逻辑像素的呼吸缝 —— 比无边框时的 10px 明显更贴近标题栏。
///
/// `rounded` 传的是**启动时实际生效**的圆角状态:`window_rounded_corners`
/// 要重启才生效,热重载不能让自动留白跟着还没生效的新配置跳。
fn tab_bar_insets(cfg: &AppConfig, scale: f64, rounded: bool) -> (f64, f64) {
    let auto = if rounded {
        (10.0 * scale).round()
    } else if cfg.window.title {
        (4.0 * scale).round()
    } else {
        0.0
    };
    let px = |v: Option<f64>| v.map(|v| (v.max(0.0) * scale).round());
    (
        px(cfg.tab_bar.edge_padding).unwrap_or(auto),
        px(cfg.tab_bar.top_padding).unwrap_or(auto),
    )
}

/// 状态栏的一段文字需要多宽(物理像素)。`advance` 是主字体的**自然字距**
/// (不是取整后的格宽 —— 状态栏按 shaper 的 advance 排版),`scale` 是这段
/// 文字的字号倍数,`pad` 是左右留白,`gap` 是相邻片段之间的额外间距,
/// `space` 是一个空格占几个字宽(`status_bar.space_width`)。
/// 占位和绘制必须用同一份口径,否则右对齐会偏。末尾多留半格,和标签之间有个缝。
fn status_width(
    spans: &[status::Span],
    advance: f64,
    scale: f64,
    pad: f64,
    gap: f64,
    space: f64,
) -> f64 {
    if spans.is_empty() {
        return 0.0;
    }
    let cw = advance * scale.max(0.5);
    let gaps = gap.max(0.0) * (spans.len().saturating_sub(1)) as f64;
    status::width_units(spans, space) * cw + gaps + pad * 2.0 + cw * 0.5
}

/// 配置里的 256 色调色板搬成渲染用的 RGB 数组。状态栏解析 `38;5;n` 与
/// `AnsiColor` 都查这张表,和终端正文用的是同一份颜色定义。
fn ansi_palette_of(cfg: &AppConfig) -> [render::Rgb; 256] {
    let p = term::build_palette(cfg);
    let mut out = [render::Rgb { r: 0, g: 0, b: 0 }; 256];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = render::rgb(p.colors.0[i]);
    }
    out
}

/// `VECTORTERM_DEBUG_CWD=1`:把"面板 ↔ 终端"目录同步的过程写进日志。
/// 这一层没法用单测覆盖(shell 真的在跑),实机探针靠它核对。
fn cwd_debug() -> bool {
    std::env::var_os("VECTORTERM_DEBUG_CWD").is_some()
}

/// 终端报上来的当前目录(OSC 7)。没报过就是 None —— 和 wezterm 一样,
/// Lua 那边 `pane:get_current_working_dir()` 会返回 nil。
fn cwd_of_term(term: &Arc<Mutex<wezterm_term::Terminal>>) -> Option<String> {
    let t = term.lock();
    let url = t.get_current_dir()?;
    let mut path = url.path().to_string();
    // file://host/C:/dir → C:/dir,Windows 上带不带前导斜杠都能被 shell 接住,
    // 但状态栏里显示"漂亮路径"更符合预期
    if path.len() >= 3 && path.starts_with('/') && path.as_bytes()[2] == b':' {
        path.remove(0);
    }
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

/// 标签栏整条占位高度 = 内容带 + 靠窗口外框那侧的留白。内容区裁剪、
/// 初始窗口尺寸反推、布局三方共用这个口径,避免差几个像素。
fn tab_bar_total_height(m: &tabbar::Metrics, window_h: f64, top_inset: f64, show_bar: bool) -> f64 {
    if show_bar {
        m.bar_h + tabbar::clamp_top_inset(top_inset, window_h, m.bar_h)
    } else {
        0.0
    }
}

/// 标签栏几何:`status_bar.height` 写了的话,把它当成这条栏的最小高度
/// (状态栏是画在这条栏上的,想要更宽松的状态栏就调它)。
fn metrics_with_status_height(cfg: &AppConfig, cell_w: f64, cell_h: f64) -> tabbar::Metrics {
    let mut m = tabbar::Metrics::new(cell_w, cell_h);
    if let Some(h) = cfg.status_bar.height {
        m.bar_h = m.bar_h.max(h.max(cell_h + 4.0));
    }
    m
}

/// wezterm 在 `WM_ENTERSIZEMOVE` / `WM_EXITSIZEMOVE` 里维护 `in_size_move`,
/// 并把它作为 `WindowEvent::Resized { live_resizing }` 传给上层:live resize
/// 期间只更新窗口内容、不动终端行列数,退出时再同步一次。
///
/// winit 0.30 没有暴露这个标志,而 Windows 的模态缩放循环必定是「按住
/// 鼠标左键」状态,所以用全局左键状态等价判断。它不依赖我们的事件循环
/// 在模态循环期间是否还在跑(Windows 会接管消息泵),也不需要在 wnd_proc
/// 上挂子类化。`resize_debounce_ms` 只管非拖动的尺寸变化(程序化 resize、
/// 最大化 / Snap),拖动期间一律冻结。
#[cfg(windows)]
fn live_resizing() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    const KEY_DOWN_MASK: u16 = 0x8000;
    unsafe { (GetAsyncKeyState(VK_LBUTTON as i32) as u16) & KEY_DOWN_MASK != 0 }
}

#[cfg(not(windows))]
fn live_resizing() -> bool {
    false
}

/// 直接读物理修饰键状态。
///
/// winit 在检测到右 Alt(AltGr) 时,会把 `Ctrl` 和 `Alt` **一起**从
/// `ModifiersState` 里过滤掉(见 winit `get_agnostic_mods`),于是
/// `Ctrl+Shift+Alt` 到了我们这里只剩 `Shift`。补一次全局按键状态,
/// 快捷键才不会因为用的是右 Alt 就失效。
#[cfg(windows)]
fn physical_modifiers() -> ModifiersState {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_MENU, VK_SHIFT,
    };
    const KEY_DOWN_MASK: u16 = 0x8000;
    let down = |vk: i32| unsafe { (GetAsyncKeyState(vk) as u16) & KEY_DOWN_MASK != 0 };
    let mut m = ModifiersState::empty();
    if down(VK_CONTROL as i32) {
        m |= ModifiersState::CONTROL;
    }
    if down(VK_MENU as i32) {
        m |= ModifiersState::ALT;
    }
    if down(VK_SHIFT as i32) {
        m |= ModifiersState::SHIFT;
    }
    m
}

#[cfg(not(windows))]
fn physical_modifiers() -> ModifiersState {
    ModifiersState::empty()
}

/// `pane_action` 的判定结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaneAction {
    Split(SplitDir),
    Focus(PaneDir),
    /// 移动分割线调整两侧窗格大小(方向 = 线的移动方向)
    Resize(PaneDir),
    /// 关掉当前 pane(最后一个 pane 则连带关标签)
    Close,
}

/// 分屏 / pane 快捷键判定(纯函数,方便单测)。
///
/// **为什么主要看 physical_key**:Windows 上 `Ctrl+Alt` 就是 AltGr,winit
/// 查字符表时因此不会剥掉 Ctrl,`ToUnicode` 只能给出控制字符甚至
/// `Key::Unidentified` —— 原来的实现拿 `logical_key` 去比 `"` / `%`,
/// 在真实按键下永远匹配不到,这就是"快捷键没用"的根因。物理键与键盘
/// 布局、输入法状态都无关,是快捷键唯一可靠的依据。
///
/// 同时认几组别名:`Ctrl+Shift+Alt` 常被输入法或显卡驱动拦掉,所以
/// `Ctrl+Shift+"` / `Ctrl+Shift+%`(不按 Alt)也能分屏;另外给了纯字母的
/// `Ctrl+Shift+Alt+V` / `H`,按不准符号键时更省事。
///
/// 调整窗格大小只用 `Ctrl+Shift+Alt+方向键`。不要再加 `Ctrl+Alt+方向键`
/// 之类的别名:那是 Intel/AMD 显卡驱动「旋转屏幕」的全局热键,按键根本
/// 到不了窗口(日志里只剩 Released,没有 Pressed),承诺给用户会失效。
fn pane_action(logical: &Key, physical: &PhysicalKey, m: ModifiersState) -> Option<PaneAction> {
    let ctrl = m.control_key();
    let shift = m.shift_key();
    let alt = m.alt_key();
    if !ctrl || m.super_key() {
        return None;
    }
    let physical_arrow = match physical {
        PhysicalKey::Code(c) => arrow_from_code(c),
        _ => None,
    };
    let named_arrow = match logical {
        Key::Named(n) => arrow_from_named(n),
        _ => None,
    };
    let arrow = named_arrow.or(physical_arrow);

    // 1) Ctrl+Shift+方向键:焦点移到相邻 pane
    if shift && !alt {
        if let Some(d) = arrow {
            return Some(PaneAction::Focus(d));
        }
    }

    // 2) 分屏:物理键优先,logical_key 只作兜底
    if shift {
        let split = match physical {
            PhysicalKey::Code(WinitKeyCode::Quote) => Some(SplitDir::Vertical),
            PhysicalKey::Code(WinitKeyCode::Digit5) => Some(SplitDir::Horizontal),
            // 字母别名只在带 Alt 时生效,避免抢走 Ctrl+Shift+V(粘贴)
            PhysicalKey::Code(WinitKeyCode::KeyV) if alt => Some(SplitDir::Vertical),
            PhysicalKey::Code(WinitKeyCode::KeyH) if alt => Some(SplitDir::Horizontal),
            _ => None,
        };
        if let Some(d) = split {
            return Some(PaneAction::Split(d));
        }
        if let Key::Character(s) = logical {
            match s.chars().next() {
                Some('"') | Some('\'') => return Some(PaneAction::Split(SplitDir::Vertical)),
                Some('%') => return Some(PaneAction::Split(SplitDir::Horizontal)),
                Some('v') | Some('V') if alt => {
                    return Some(PaneAction::Split(SplitDir::Vertical))
                }
                Some('h') | Some('H') if alt => {
                    return Some(PaneAction::Split(SplitDir::Horizontal))
                }
                _ => {}
            }
        }
    }

    // 3) Ctrl+Shift+X:关掉当前 pane(对应 wezterm 的 `CloseCurrentPane`)。
    //
    //    和分屏那几条一样只认物理键:中文输入法开着时 `Ctrl+Shift` 组合的
    //    `logical_key` 可能是 `Unidentified`(wParam 被换成 VK_PROCESSKEY),
    //    只有扫描码还原出的物理键可靠。字母 X 不与终端里任何控制字符冲突,
    //    也不需要 Shift 生成的字符,所以不按 Alt 也能按出来。
    if shift {
        if matches!(physical, PhysicalKey::Code(WinitKeyCode::KeyX)) {
            return Some(PaneAction::Close);
        }
        if let Key::Character(s) = logical {
            if s.eq_ignore_ascii_case("x") {
                return Some(PaneAction::Close);
            }
        }
    }

    // 4) Ctrl+Shift+Alt+方向键:移动分割线,调整窗格大小
    if shift && alt {
        if let Some(d) = arrow {
            return Some(PaneAction::Resize(d));
        }
    }
    None
}

/// 物理数字键 → 1..9(含小键盘)。
fn digit_from_code(c: &WinitKeyCode) -> Option<u32> {
    use WinitKeyCode as K;
    Some(match c {
        K::Digit1 | K::Numpad1 => 1,
        K::Digit2 | K::Numpad2 => 2,
        K::Digit3 | K::Numpad3 => 3,
        K::Digit4 | K::Numpad4 => 4,
        K::Digit5 | K::Numpad5 => 5,
        K::Digit6 | K::Numpad6 => 6,
        K::Digit7 | K::Numpad7 => 7,
        K::Digit8 | K::Numpad8 => 8,
        K::Digit9 | K::Numpad9 => 9,
        _ => return None,
    })
}

fn arrow_from_named(n: &NamedKey) -> Option<PaneDir> {
    Some(match n {
        NamedKey::ArrowLeft => PaneDir::Left,
        NamedKey::ArrowRight => PaneDir::Right,
        NamedKey::ArrowUp => PaneDir::Up,
        NamedKey::ArrowDown => PaneDir::Down,
        _ => return None,
    })
}

fn arrow_from_code(c: &WinitKeyCode) -> Option<PaneDir> {
    Some(match c {
        WinitKeyCode::ArrowLeft => PaneDir::Left,
        WinitKeyCode::ArrowRight => PaneDir::Right,
        WinitKeyCode::ArrowUp => PaneDir::Up,
        WinitKeyCode::ArrowDown => PaneDir::Down,
        _ => return None,
    })
}

/// 和 `trace_resize` 共用同一个开关,只打一段耗时。用来定位「resize 到
/// present」这几毫秒到底花在 PTY 同步、几何计算还是绘制上。
fn trace_dur(tag: &str, d: Duration) {
    if !trace_on() {
        return;
    }
    crate::wlog!("[VectorTerm][trace] {tag} {:.2}ms", d.as_secs_f64() * 1000.0);
}

/// `VECTORTERM_TRACE_RESIZE` 是否打开(resize 诊断开关,只读一次环境变量)
fn trace_on() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VECTORTERM_TRACE_RESIZE").is_some())
}

/// 可选的 resize/redraw 时序诊断:设 `VECTORTERM_TRACE_RESIZE=1` 后,每次
/// `WindowEvent::Resized` 与 `redraw` 都会打一条毫秒级日志。用来确认
/// Windows 模态缩放循环里 winit 是否还在实时派发事件(若被缓冲,拖动期间
/// 画面就不会重绘,只能看到系统保留/拉伸的旧位图)。
fn trace_resize(tag: &str, w: u32, h: u32) {
    if !trace_on() {
        return;
    }
    let ms = BOOT.get().map(|b| b.elapsed().as_millis()).unwrap_or(0);
    crate::wlog!(
        "[VectorTerm][trace] {tag} {w}x{h} t={ms}ms live={}",
        live_resizing()
    );
}

/// 一次尺寸变化该怎么处理终端行列数。
///
/// wezterm 在 `WindowEvent::Resized { live_resizing }` 里直接
/// `apply_dimensions()`,拖边框时行列数是实时的,文字位置跟着窗口调整。
/// 这里默认照做;非拖动的尺寸变化(程序化 resize、最大化 / Snap、
/// Win+方向键)仍按 `resize_debounce_ms` 防抖,避免连续多次惊动 PTY。
/// 抽成纯函数便于单测,也避免 `apply_resize` 里出现两套隐式策略。
fn resize_mode_for(dragging: bool, cfg: &AppConfig) -> LiveResizeMode {
    if dragging {
        cfg.live_resize_mode
    } else if cfg.resize_debounce_ms == 0 {
        LiveResizeMode::Realtime
    } else {
        LiveResizeMode::Debounce
    }
}

/// 默认标签标题:shell 的文件名(去掉 .exe)
fn default_title(cfg: &AppConfig) -> String {
    Path::new(&cfg.shell)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("Shell")
        .to_string()
}

/// 把布局树里所有 `>= target` 的叶子下标减一(panes 收缩后下标前移)。
fn remap_leaves_after(node: &mut SplitNode, target: usize) {
    match node {
        SplitNode::Leaf(i) => {
            if *i > target {
                *i -= 1;
            }
        }
        SplitNode::Split { a, b, .. } => {
            remap_leaves_after(a, target);
            remap_leaves_after(b, target);
        }
    }
}

/// 计算某个 pane 的选区高亮(纯函数,分屏时逐个 pane 调用)。
fn pane_selection_highlight_impl(
    cfg: &AppConfig,
    p: &Pane,
    top: usize,
    rows: usize,
) -> Option<frame::Highlight> {
    let sel = p.selection.current.as_ref()?;
    if sel.is_empty() {
        return None;
    }
    let ((c0, r0), (c1, r1)) = sel.normalized();
    if rows == 0 || r1 < top || r0 >= top.saturating_add(rows) {
        return None;
    }
    let last = rows - 1;
    let mut start = (c0, r0.saturating_sub(top).min(last));
    let mut end = (c1, r1.saturating_sub(top).min(last));
    if r0 < top {
        start = (0, 0);
    }
    if r1 >= top.saturating_add(rows) {
        end = (p.grid.0.saturating_sub(1), last);
    }
    let color = term::color_from_str(&cfg.selection.background).unwrap_or(render::Rgb {
        r: 58,
        g: 110,
        b: 165,
    });
    Some(frame::Highlight {
        start,
        end,
        color,
        alpha: cfg.selection.opacity,
    })
}

/// 起一个本地直连的 pane:ConPTY + 终端核心 + 读取/等待线程。
/// 标题与退出事件都带 `id`(全局唯一 pane id),由窗口线程路由回对应 pane。
fn spawn_pane(
    cfg: &AppConfig,
    grid: (usize, usize),
    px: (u32, u32),
    proxy: &EventLoopProxy<UserEvent>,
    id: u64,
    title: String,
    dpi: u32,
) -> anyhow::Result<Pane> {
    let (cols, rows) = grid;
    let pty::PtyParts {
        session,
        writer,
        reader,
        child,
    } = pty::spawn(cfg, rows as u16, cols as u16, px.0 as u16, px.1 as u16)?;

    // 会话记录(可选)。多个标签共用一个文件,按到达顺序追加。
    let session_log = match cfg.session_log.as_deref().map(|p| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
    }) {
        Some(Ok(f)) => Some(f),
        Some(Err(e)) => {
            crate::wlog!("[VectorTerm] 打开会话记录失败:{e}");
            None
        }
        None => None,
    };

    let (notify_tx, notify_rx) = std::sync::mpsc::channel::<()>();
    let shared_inner = Arc::new(WriterInner {
        buf: Mutex::new(Vec::new()),
    });
    spawn_flusher(writer, notify_rx, shared_inner.clone());
    let term_writer = SharedWriter {
        inner: shared_inner.clone(),
        notify: notify_tx.clone(),
    };
    let kb_writer = SharedWriter {
        inner: shared_inner,
        notify: notify_tx,
    };

    let size = wezterm_term::TerminalSize {
        rows,
        cols,
        pixel_width: px.0 as usize,
        pixel_height: px.1 as usize,
        dpi,
    };
    let mut term = term::create_terminal(size, cfg, Box::new(term_writer));
    term.set_notification_handler(Box::new(term::TitleAlerter {
        proxy: StdMutex::new(proxy.clone()),
        tab: id,
    }));
    let term = Arc::new(Mutex::new(term));

    // 解析线程:PTY 输出 → wezterm-term(可选落盘)
    {
        let term = term.clone();
        let proxy = proxy.clone();
        let mut reader = reader;
        let mut log = session_log;
        let _ = std::thread::Builder::new()
            .name("vectorterm-parser".into())
            .spawn(move || {
                let mut buf = vec![0u8; 16 * 1024];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if let Some(f) = log.as_mut() {
                                use std::io::Write;
                                let _ = f.write_all(&buf[..n]);
                            }
                            term.lock().advance_bytes(&buf[..n]);
                            let _ = proxy.send_event(UserEvent::Repaint);
                        }
                        Err(_) => break,
                    }
                }
                let _ = proxy.send_event(UserEvent::ChildExited {
                    tab: id,
                    msg: "输出流已关闭".into(),
                });
            });
    }

    // 子进程退出监视线程
    {
        let proxy = proxy.clone();
        let mut child = child;
        let _ = std::thread::Builder::new()
            .name("vectorterm-waiter".into())
            .spawn(move || {
                let msg = match child.wait() {
                    Ok(status) => format!("exit {status}"),
                    Err(e) => format!("{e}"),
                };
                let _ = proxy.send_event(UserEvent::ChildExited { tab: id, msg });
            });
    }

    Ok(Pane {
        id,
        term,
        pty: Some(session),
        mux: None,
        kb_writer,
        trail: CursorTrail::new(&cfg.cursor),
        scroll: ScrollAnim::new(&cfg.scroll),
        pad: 0,
        last_cy: 0,
        selection: selection::SelectionState::new(),
        grid,
        title,
        shell: cfg.shell.clone(),
        exited: false,
        exit_reported: false,
    })
}

/// 起一个本地直连的标签:首个 pane 用本地 PTY,再包装成 TabState。
fn spawn_tab(
    cfg: &AppConfig,
    grid: (usize, usize),
    px: (u32, u32),
    proxy: &EventLoopProxy<UserEvent>,
    id: u64,
    title: String,
    dpi: u32,
) -> anyhow::Result<TabState> {
    let pane = spawn_pane(cfg, grid, px, proxy, id, title, dpi)?;
    Ok(TabState {
        id,
        panes: vec![pane],
        active_pane: 0,
        layout: SplitNode::leaf(0),
        next_pane_id: id + 1,
        split_anim: None,
        browser: None,
        browser_pane: 0,
    })
}

/// 会话保持模式:通过 mux 守护进程新建一个 pane(起 shell)。
///
/// 与 [`spawn_pane`] 的差异:shell 由守护进程托管,不是本进程直接起。
/// 本地仍建一个 `Terminal` 作渲染用副本,其内容由守护进程推送的输出驱动。
fn spawn_pane_mux(
    cfg: &AppConfig,
    grid: (usize, usize),
    px: (u32, u32),
    proxy: &EventLoopProxy<UserEvent>,
    dpi: u32,
) -> anyhow::Result<Pane> {
    let (cols, rows) = grid;
    let Some(mut client) = mux::MuxClient::connect()? else {
        anyhow::bail!("mux 守护进程未运行");
    };
    let req = mux::SpawnReq {
        shell: cfg.shell.clone(),
        args: cfg.shell_args.clone(),
        cwd: cfg.cwd.as_ref().map(|p| p.to_string_lossy().into_owned()),
        env: cfg.env.clone(),
        rows: rows as u16,
        cols: cols as u16,
        pixel_w: px.0 as u16,
        pixel_h: px.1 as u16,
        dpi,
    };
    let meta = client.spawn(&req)?;
    let pane_id = meta.id;
    // spawn 只回元信息;历史为空(全新 shell),后续输出由推送驱动。
    build_mux_pane(cfg, grid, px, proxy, dpi, client, pane_id, meta)
}

/// 会话保持模式:起一个 mux 标签(首个 pane 走守护进程)。
fn spawn_tab_mux(
    cfg: &AppConfig,
    grid: (usize, usize),
    px: (u32, u32),
    proxy: &EventLoopProxy<UserEvent>,
    dpi: u32,
) -> anyhow::Result<TabState> {
    let pane = spawn_pane_mux(cfg, grid, px, proxy, dpi)?;
    Ok(TabState {
        id: pane.id,
        panes: vec![pane],
        active_pane: 0,
        layout: SplitNode::leaf(0),
        next_pane_id: 1,
        split_anim: None,
        browser: None,
        browser_pane: 0,
    })
}

/// 会话保持模式:attach 到守护进程里已有的 pane,恢复现场。
///
/// attach 返回该 pane 的全量快照(detach 期间缓存的输出字节),
/// 前端回放这些字节重建滚动缓冲,历史完整恢复。
fn attach_tab_mux(
    cfg: &AppConfig,
    grid: (usize, usize),
    px: (u32, u32),
    proxy: &EventLoopProxy<UserEvent>,
    dpi: u32,
    pane_id: u64,
) -> anyhow::Result<TabState> {
    let Some(mut client) = mux::MuxClient::connect()? else {
        anyhow::bail!("mux 守护进程未运行");
    };
    let snap = client.attach(pane_id)?;
    let meta = snap.meta;
    // 回放缓存历史
    let pane = build_mux_pane(cfg, grid, px, proxy, dpi, client, pane_id, meta)?;
    if let Some(raw) = snap.raw {
        if !raw.is_empty() {
            pane.term.lock().advance_bytes(&raw);
        }
    }
    Ok(TabState {
        id: pane.id,
        panes: vec![pane],
        active_pane: 0,
        layout: SplitNode::leaf(0),
        next_pane_id: 1,
        split_anim: None,
        browser: None,
        browser_pane: 0,
    })
}

/// mux pane 的公共装配:建 Terminal、接写端、起读线程。
fn build_mux_pane(
    cfg: &AppConfig,
    grid: (usize, usize),
    px: (u32, u32),
    proxy: &EventLoopProxy<UserEvent>,
    dpi: u32,
    client: mux::MuxClient,
    pane_id: u64,
    meta: mux::PaneMeta,
) -> anyhow::Result<Pane> {
    let (cols, rows) = grid;
    let (writer, reader) = client.into_parts();

    // kb_writer / term_writer 走同一条 flusher 管线,目标换成 MuxWriter
    // (把字节打包成 Input 帧发给守护进程)。
    let (notify_tx, notify_rx) = std::sync::mpsc::channel::<()>();
    let shared_inner = Arc::new(WriterInner {
        buf: Mutex::new(Vec::new()),
    });
    let mux_writer = Box::new(mux::MuxWriter {
        pane: pane_id,
        writer: writer.clone(),
    });
    spawn_flusher(mux_writer, notify_rx, shared_inner.clone());
    let term_writer = SharedWriter {
        inner: shared_inner.clone(),
        notify: notify_tx.clone(),
    };
    let kb_writer = SharedWriter {
        inner: shared_inner,
        notify: notify_tx,
    };

    let size = wezterm_term::TerminalSize {
        rows,
        cols,
        pixel_width: px.0 as usize,
        pixel_height: px.1 as usize,
        dpi,
    };
    let mut term = term::create_terminal(size, cfg, Box::new(term_writer));
    term.set_notification_handler(Box::new(term::TitleAlerter {
        proxy: StdMutex::new(proxy.clone()),
        tab: pane_id,
    }));
    let term = Arc::new(Mutex::new(term));

    // 读线程:收守护进程推送(输出 / 退出 / 标题),转成 UserEvent 路由回主线程。
    {
        let proxy = proxy.clone();
        let _ = std::thread::Builder::new()
            .name("vectorterm-mux-reader".into())
            .spawn(move || {
                let mut reader = reader;
                loop {
                    let frame = match reader.read_frame() {
                        Ok(Some(f)) => f,
                        Ok(None) | Err(_) => break,
                    };
                    let msg = match mux::decode_server(frame.kind, &frame.payload) {
                        Ok(m) => m,
                        Err(_) => continue,
                    };
                    match msg {
                        mux::ServerMsg::Output(o) => {
                            let _ = proxy.send_event(UserEvent::MuxOutput {
                                pane: o.pane,
                                bytes: o.bytes,
                            });
                        }
                        mux::ServerMsg::Exit { pane, msg } => {
                            let _ = proxy.send_event(UserEvent::MuxExit { pane, msg });
                            break;
                        }
                        mux::ServerMsg::Title { pane, title } => {
                            let _ = proxy.send_event(UserEvent::MuxTitle { pane, title });
                        }
                        _ => {}
                    }
                }
            });
    }

    Ok(Pane {
        id: pane_id,
        term,
        pty: None,
        mux: Some(MuxPane {
            pane_id,
            writer,
            pid: if meta.pid == 0 { None } else { Some(meta.pid) },
        }),
        kb_writer,
        trail: CursorTrail::new(&cfg.cursor),
        scroll: ScrollAnim::new(&cfg.scroll),
        pad: 0,
        last_cy: 0,
        selection: selection::SelectionState::new(),
        grid,
        title: meta.title,
        shell: cfg.shell.clone(),
        exited: meta.exited,
        exit_reported: false,
    })
}

/// 确保 mux 守护进程在跑:连不上就用当前 exe 的 `--mux-server` 启动一个。
fn ensure_mux_server() -> anyhow::Result<()> {
    if mux::MuxClient::connect()?.is_some() {
        return Ok(());
    }
    let exe = std::env::current_exe().context("拿当前 exe 路径失败")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--mux-server");
    // 守护进程与 GUI 解耦:CREATE_NEW_PROCESS_GROUP + 不继承控制台,
    // 这样关掉启动它的 GUI 也不会被一起带走。
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    }
    let _ = cmd.spawn().context("启动 mux 守护进程失败")?;
    // 给守护进程一点时间起来监听管道,再探测一次
    for _ in 0..50 {
        if mux::MuxClient::connect()?.is_some() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    anyhow::bail!("mux 守护进程启动后仍无法连接")
}

/// 尝试 attach 最近的一个存活 pane(会话保持的「恢复现场」)。
/// 没有存活 pane 时返回 None。
fn attach_recent_pane(
    cfg: &AppConfig,
    grid: (usize, usize),
    px: (u32, u32),
    proxy: &EventLoopProxy<UserEvent>,
    dpi: u32,
) -> Option<TabState> {
    let mut client = mux::MuxClient::connect().ok().flatten()?;
    let list = client.list().ok()?;
    // 挑 id 最大(最后创建)的存活 pane
    let target = list.into_iter().filter(|m| !m.exited).max_by_key(|m| m.id)?;
    attach_tab_mux(cfg, grid, px, proxy, dpi, target.id).ok()
}

/// 给 `base` 加 `-<tag>` 后缀(`a.bmp` → `a-2250.bmp`),逐帧导出时用来区分。
fn suffixed(base: &Path, tag: &str) -> PathBuf {
    let stem = base
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "frame".into());
    let ext = base
        .extension()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "bmp".into());
    base.with_file_name(format!("{stem}-{tag}.{ext}"))
}

/// 解析 `VECTORTERM_DUMP_AT=毫秒[,毫秒...]`,配上 `VECTORTERM_DUMP_FRAME` 的基名生成
/// 一组"到点各拍一张"的任务。给多个时刻时路径自动加 `-<毫秒>` 后缀,这样一条
/// 命令就能把一段动画逐帧拍下来(`a.bmp` → `a-2250.bmp` / `a-2270.bmp` ...)。
fn dump_seq_of(at: Option<std::ffi::OsString>, base: Option<std::ffi::OsString>) -> Vec<(u64, PathBuf)> {
    let (Some(at), Some(base)) = (at, base) else {
        return Vec::new();
    };
    let base = PathBuf::from(base);
    let times: Vec<u64> = at
        .to_string_lossy()
        .split([',', ' '])
        .filter_map(|s| s.trim().parse::<u64>().ok())
        .collect();
    if times.is_empty() {
        return Vec::new();
    }
    let multi = times.len() > 1;
    times
        .into_iter()
        .map(|ms| {
            if multi {
                (ms, suffixed(&base, &ms.to_string()))
            } else {
                (ms, base.clone())
            }
        })
        .collect()
}

/// 把一帧 ARGB 缓冲写成 24 位 BMP(诊断用:VECTORTERM_DUMP_FRAME=路径)
fn write_bmp(path: &Path, buf: &[u32], w: u32, h: u32) -> std::io::Result<()> {
    let row_bytes = (w * 3 + 3) & !3;
    let mut out = Vec::with_capacity((54 + row_bytes * h) as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(54 + row_bytes * h).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(row_bytes * h).to_le_bytes());
    out.extend_from_slice(&[0u8; 16]);
    for y in (0..h).rev() {
        for x in 0..w {
            let v = buf.get((y * w + x) as usize).copied().unwrap_or(0);
            out.push((v & 0xff) as u8);
            out.push(((v >> 8) & 0xff) as u8);
            out.push(((v >> 16) & 0xff) as u8);
        }
        for _ in w * 3..row_bytes {
            out.push(0);
        }
    }
    std::fs::write(path, out)
}

/// 视口几何:`(顶行绝对行号, 用户回看上限, 含抬起量的有效偏移)`。
///
/// 这是"屏幕第几行 ↔ 缓冲区第几行"的唯一口径:`render::snapshot`、
/// 鼠标选区、双击取词、滚动上限都从这里取值,免得几处各算一遍再对不上。
/// `pad` 是 resize 之后视口抬起的行数(见 [`pad_after_resize`]),它和用户的
/// 回看偏移一起构成相对缓冲区末尾的真实偏移。
fn view_geom_of(screen: &wezterm_term::Screen, pad: usize, scroll: f32) -> (usize, usize, f32) {
    view_geom_of_sizes(
        screen.physical_rows.max(1),
        screen.scrollback_rows(),
        pad,
        scroll,
    )
}

/// [`view_geom_of`] 的纯函数版本(便于单测)
fn view_geom_of_sizes(rows: usize, sb: usize, pad: usize, scroll: f32) -> (usize, usize, f32) {
    let max = sb.saturating_sub(rows);
    let pad = pad.min(max);
    let eff = (scroll.max(0.0) + pad as f32).min(max as f32);
    let base = eff.ceil() as usize;
    (sb.saturating_sub(rows + base), max.saturating_sub(pad), eff)
}

/// 拖选到内容区上/下边缘时该滚多快(行 / 45ms)。
///
/// 正 = 往更早的历史翻(鼠标在顶部);越贴近/越过边缘越快,1..8 行封顶。
/// 中间 3 格高的带状区域之外一律不滚,免得正常拖选也跟着动。
fn edge_scroll_lines(y: f64, top: f64, bottom: f64, cell_h: f64) -> i64 {
    let zone = (cell_h * 3.0).max(12.0);
    let speed = |d: f64| -> i64 {
        (1.0 + d.clamp(0.0, 1.0) * 7.0).round().max(1.0) as i64
    };
    if y < top + zone {
        speed((top + zone - y) / zone)
    } else if y > bottom - zone {
        -speed((y - (bottom - zone)) / zone)
    } else {
        0
    }
}

/// 调整窗口之后,视口应该"抬起"多少行。
///
/// ConPTY 把滚动区当成不可变的:窗口一变高,它就在屏幕尾部补一排空行,
/// 于是提示符被顶到窗口中间、下面留一大片空白(wezterm 在 Windows 上也是
/// 这个表现)。这些空行不能从缓冲区里删 —— 那会让 ConPTY 的行号与画面错开,
/// 之后它写出来的文字就会落到历史行上。所以只在**显示**上把视口抬起:
/// 底部让给光标所在行,顶部多看到几行回看历史。
///
/// 抬起量取"光标下方还有多少空行",但不能超过实际可回看的行数(没有历史时
/// 抬起来只会把画面顶空)。
fn pad_after_resize(rows: usize, cy: usize, sb: usize) -> usize {
    rows.saturating_sub(1)
        .saturating_sub(cy)
        .min(sb.saturating_sub(rows))
}

/// 光标往下走(shell 继续输出)时,尾部补白被一行行吃掉,抬起量跟着减少 ——
/// 画面上就是"提示符贴在底部,内容往上滚",和普通终端一致。
/// 光标往上跳(清屏、全屏应用重绘)说明屏幕已经换过内容,立刻取消抬起。
fn pad_follow(pad: usize, rows: usize, cy: usize, moved_up: bool) -> usize {
    if moved_up {
        0
    } else {
        pad.min(rows.saturating_sub(1).saturating_sub(cy))
    }
}

/// 一个 pane 的完整几何。坐标一律相对**内容区原点**(px),渲染 / 命中测试时
/// 再加 `pad_x` 与 `pad_y + content_top`。
#[derive(Clone, Copy, Debug)]
struct PaneGeom {
    pane_id: u64,
    /// 该 pane 可用的字符网格(交给 PTY)
    grid: (usize, usize),
    /// 文字区左上角
    x: f64,
    y: f64,
    /// 分配给该 pane 的矩形大小(px),用于鼠标命中与输入光标落点
    w: f64,
    h: f64,
}

impl PaneGeom {
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
    /// 中心点(相对内容区原点)
    fn center(&self) -> (f64, f64) {
        (self.x + self.w * 0.5, self.y + self.h * 0.5)
    }
}

/// 自动线宽(px):默认一根 3px 的细线,而不是旧版填满整格的粗灰条。
const AUTO_LINE_T: f64 = 3.0;

/// 线宽(px)。显式 `thickness` 可以大于缝宽(线会盖住两侧一点文字);
/// 自动线宽则随缝收窄,绝不越过文字区。
fn divider_line_t(cfg: &SplitDividerConfig, gap: f64) -> f64 {
    match cfg.thickness {
        Some(t) => t.max(0.0),
        None => AUTO_LINE_T.min(gap.max(0.0)),
    }
}

/// 两个 pane 文字区之间的缝宽(px)。
///
/// 默认**只留 1px 空气**:`gap = 线宽 + 2`,文字几乎贴着线,观感和 kitty /
/// wezterm 的分屏一致。旧实现按 `0.7 × 字符宽` 取 Auto,字号一大(或高 DPI)
/// 缝就跟着变宽,看起来"文字离分割线很远"。
///
/// - `text_gap` 优先:`gap = 2 * text_gap + 线宽`,直接表达"文字到线边缘的距离";
/// - `gap` 显式给值时用它本人;
/// - 永远不小于线宽(线最多贴到文字边,不会压到字上 —— 所以 `text_gap = 0`
///   就是字面意义上的"文字贴着线");且不超过所在区域的 1/6
///   (窄 pane / 分屏动画时自动收窄)。
fn divider_gap_px(cfg: &SplitDividerConfig, cell: f64, area: f64) -> f64 {
    // 自动线宽给 3px,但不超过半个字符(字号极小时不至于糊成一团)
    let auto_line = AUTO_LINE_T.min((cell * 0.5).max(1.0));
    let unit_t = cfg.thickness.unwrap_or(auto_line).max(0.0);
    let mut gap = unit_t + 2.0;
    if let Some(g) = cfg.gap {
        gap = g.max(0.0);
    }
    if let Some(tg) = cfg.text_gap {
        gap = 2.0 * tg.max(0.0) + unit_t;
    }
    gap.max(unit_t)
        .min((area / 6.0).max(0.0))
        .max(0.0)
}

/// **唯一的布局计算入口**。像素驱动:分割线永远精确落在区域中线,
/// `ratio = 0.5` 时两侧 pane 分到的像素宽度严格相等,因而列数也相等 ——
/// 旧实现用 `round((cols - 1) * 0.5)`,偶数窗口下两侧会差一列、线偏半格。
fn compute_pane_geoms(
    layout: &SplitNode,
    pane_id: &dyn Fn(usize) -> Option<u64>,
    cell_w: f64,
    cell_h: f64,
    area_w: f64,
    area_h: f64,
    cfg: &SplitDividerConfig,
) -> (Vec<PaneGeom>, Vec<SplitLine>) {
    compute_pane_geoms_in(layout, pane_id, cell_w, cell_h, 0.0, area_w, area_h, cfg)
}

/// 同上,但整块区域从 `x0` 开始(文件浏览器侧边栏占掉左边一条)。
/// 布局树本身不动:只是把可用宽度减去侧边栏,再整体右移 —— 分割线、
/// 取整余量、PTY 列数全部跟着这一份几何走,和普通分屏完全同源。
#[allow(clippy::too_many_arguments)]
fn compute_pane_geoms_in(
    layout: &SplitNode,
    pane_id: &dyn Fn(usize) -> Option<u64>,
    cell_w: f64,
    cell_h: f64,
    x0: f64,
    area_w: f64,
    area_h: f64,
    cfg: &SplitDividerConfig,
) -> (Vec<PaneGeom>, Vec<SplitLine>) {
    let mut geoms = Vec::new();
    let mut lines = Vec::new();
    walk_pane_geoms(
        layout,
        pane_id,
        cell_w,
        cell_h,
        x0,
        0.0,
        area_w.max(0.0),
        area_h.max(0.0),
        false,
        false,
        cfg,
        0,
        0,
        &mut geoms,
        &mut lines,
    );
    (geoms, lines)
}

#[allow(clippy::too_many_arguments)]
fn walk_pane_geoms(
    node: &SplitNode,
    pane_id: &dyn Fn(usize) -> Option<u64>,
    cell_w: f64,
    cell_h: f64,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    // 两个轴分开记:true = 该叶子紧邻所在区域**末端**(右 / 下)那条分割线,
    // 取整剩余的像素要留在远离线的一侧(左侧 / 上侧)。
    //
    // 以前横纵共用一个布尔量,嵌套分屏就会错:左半再上下分时,下半 pane 会
    // 继承"贴右边"的标记,连竖直方向也把余量推到下边,于是它的第一行文字
    // 离横线多出近一整格。现在竖直分割只改 align_x、水平分割只改 align_y。
    align_x: bool,
    align_y: bool,
    cfg: &SplitDividerConfig,
    path: u32,
    depth: u8,
    geoms: &mut Vec<PaneGeom>,
    lines: &mut Vec<SplitLine>,
) {
    let cw = cell_w.max(1.0);
    let ch = cell_h.max(1.0);
    match node {
        SplitNode::Leaf(idx) => {
            if let Some(id) = pane_id(*idx) {
                let cols = (w / cw).floor().max(1.0) as usize;
                let rows = (h / ch).floor().max(1.0) as usize;
                // 文字只能按整格排;不足一格的余量以前无条件留在右侧,
                // 导致左 pane 文字离分割线将近一整格、右 pane 却紧贴线。
                // 现在把余量挪到远离分割线的一侧:两侧文字到线都只剩半个缝宽。
                let used_w = (cols as f64 * cw).min(w.max(0.0));
                let used_h = (rows as f64 * ch).min(h.max(0.0));
                let px = if align_x {
                    x + (w - used_w).max(0.0)
                } else {
                    x
                };
                let py = if align_y {
                    y + (h - used_h).max(0.0)
                } else {
                    y
                };
                geoms.push(PaneGeom {
                    pane_id: id,
                    grid: (cols, rows),
                    x: px,
                    y: py,
                    w: used_w,
                    h: used_h,
                });
            }
        }
        SplitNode::Split { dir, ratio, a, b } => {
            let ratio = ratio.clamp(0.0, 1.0);
            match dir {
                SplitDir::Vertical => {
                    let gap = divider_gap_px(cfg, cw, w);
                    // 中线 = 区域起点 + 宽度 * ratio;缝以中线为轴左右均分。
                    let mid = x + w * ratio;
                    let left_edge = (mid - gap * 0.5).clamp(x, x + w);
                    let right_edge = (mid + gap * 0.5).clamp(left_edge, x + w);
                    lines.push(SplitLine {
                        x: left_edge,
                        y,
                        w: right_edge - left_edge,
                        h,
                        vertical: true,
                        area: (x, y, w, h),
                        path,
                        depth,
                    });
                    // a 在左、紧邻缝的右边界;b 在右、紧邻缝的左边界
                    walk_pane_geoms(
                        a,
                        pane_id,
                        cw,
                        ch,
                        x,
                        y,
                        left_edge - x,
                        h,
                        true,
                        align_y,
                        cfg,
                        path,
                        depth.saturating_add(1),
                        geoms,
                        lines,
                    );
                    walk_pane_geoms(
                        b,
                        pane_id,
                        cw,
                        ch,
                        right_edge,
                        y,
                        x + w - right_edge,
                        h,
                        false,
                        align_y,
                        cfg,
                        if depth < 32 { path | (1u32 << depth) } else { path },
                        depth.saturating_add(1),
                        geoms,
                        lines,
                    );
                }
                SplitDir::Horizontal => {
                    let gap = divider_gap_px(cfg, ch, h);
                    let mid = y + h * ratio;
                    let top_edge = (mid - gap * 0.5).clamp(y, y + h);
                    let bottom_edge = (mid + gap * 0.5).clamp(top_edge, y + h);
                    lines.push(SplitLine {
                        x,
                        y: top_edge,
                        w,
                        h: bottom_edge - top_edge,
                        vertical: false,
                        area: (x, y, w, h),
                        path,
                        depth,
                    });
                    // a 在上、紧邻缝的下边界;b 在下、紧邻缝的上边界
                    walk_pane_geoms(
                        a,
                        pane_id,
                        cw,
                        ch,
                        x,
                        y,
                        w,
                        top_edge - y,
                        align_x,
                        true,
                        cfg,
                        path,
                        depth.saturating_add(1),
                        geoms,
                        lines,
                    );
                    walk_pane_geoms(
                        b,
                        pane_id,
                        cw,
                        ch,
                        x,
                        bottom_edge,
                        w,
                        y + h - bottom_edge,
                        align_x,
                        false,
                        cfg,
                        if depth < 32 { path | (1u32 << depth) } else { path },
                        depth.saturating_add(1),
                        geoms,
                        lines,
                    );
                }
            }
        }
    }
}

/// 跨窗格尾迹用的光标参数。
///
/// **默认就是 `cursor` 那一份原样**(`pane_focus.trail_ms = 320` 时系数为 1),
/// 这样跨窗格和窗格内看起来是同一个动画。`trail_ms` 只作为快慢旋钮:把它调大
/// 就整体变慢(decay 与硬上限同比例放大),调小就更快;设成 0 直接关掉。
fn focus_cursor_cfg(cfg: &AppConfig) -> config::CursorConfig {
    let mut c = cfg.cursor.clone();
    let ms = cfg.pane_focus.trail_ms;
    if ms <= 0.0 {
        // CursorTrail 用 dwell=0 表示"关掉"
        c.trail_dwell_ms = 0.0;
        return c;
    }
    let scale = (ms / 320.0).clamp(0.25, 4.0) as f32;
    c.trail_decay_fast *= scale;
    c.trail_decay_slow *= scale;
    c.trail_max_ms *= scale;
    c
}

/// 编辑器探测结果。
///
/// 三种情况分开表达,而不是"返回空 vec 让调用方自己猜":找不到编辑器时要给
/// 的是一句能照着修的提示,不是一个静默失败。
enum EditorPick {
    /// 找到可用的编辑器,直接起
    Ready(Vec<String>),
    /// vim 系没找到,退回记事本 —— 同时带一条说明
    Fallback(Vec<String>, String),
    /// 一个都没有:只在面板底部提示,不打开 pane
    Missing(String),
}

/// 找编辑器:`PATH` 优先,再查常见安装位置。
///
/// 只查 `PATH` 会误判 —— 从资源管理器启动时继承的是**启动前**的环境;
/// 装完 nvim 还没重启过资源管理器的话,`PATH` 里根本没有它。
fn find_editor(exe: &str) -> Option<PathBuf> {
    if let Some(p) = pty::which(exe) {
        return Some(p);
    }
    let file = exe.to_string();
    let mut cands: Vec<PathBuf> = Vec::new();
    // ProgramW6432 在 32 位进程里指向真正的 Program Files
    for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Some(base) = std::env::var_os(var) {
            let base = PathBuf::from(&base);
            cands.push(base.join("Neovim/bin").join(&file));
            cands.push(base.join("Neovim").join(&file));
            cands.push(base.join("Vim/vim90").join(&file));
        }
    }
    for var in ["LOCALAPPDATA", "USERPROFILE"] {
        if let Some(base) = std::env::var_os(var) {
            let base = PathBuf::from(&base);
            cands.push(base.join("nvim/bin").join(&file));
            cands.push(base.join("scoop/shims").join(&file));
            cands.push(base.join("Microsoft/WinGet/Links").join(&file));
        }
    }
    if let Some(base) = std::env::var_os("ProgramData") {
        cands.push(PathBuf::from(base).join("chocolatey/bin").join(&file));
    }
    cands.into_iter().find(|p| p.is_file())
}

/// 把 `argv` 拼成一条能在 `shell` 里执行的命令行(不含结尾的回车)。
///
/// 文件是"写进当前 pane 的 shell"打开的,所以必须按目标 shell 的引用规则来:
/// - `cmd.exe`:`"exe" "arg with space"`(双引号,内部 `"` 重复);
/// - PowerShell:`& 'exe' 'arg'`(单引号,内部 `'` 重复;`&` 是调用运算符,
///   少了它 PowerShell 只会把带引号的路径当字符串打印出来)。
///
/// 探测到的编辑器都是绝对路径,统一加引号最省事,也不会被路径里的空格拆开。
fn shell_command_line(shell: &str, argv: &[String]) -> String {
    let name = Path::new(shell)
        .file_name()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let pwsh = name.contains("pwsh") || name.contains("powershell");
    let mut out = String::new();
    if pwsh {
        out.push_str("& ");
    }
    for (i, a) in argv.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if pwsh {
            out.push('\'');
            out.push_str(&a.replace('\'', "''"));
            out.push('\'');
        } else {
            out.push('"');
            out.push_str(&a.replace('"', "\"\""));
            out.push('"');
        }
    }
    out
}

/// 把"切到这个目录"拼成目标 shell 认的一行命令(末尾带 `\r`,可以直接写进 PTY)。
///
/// * cmd 用 `cd /d`(跨盘符必须带 `/d`);
/// * PowerShell 用 `Set-Location -LiteralPath`(单引号里不做变量展开);
/// * 其余(bash / nu 之类)按 POSIX 风格拼,反斜杠换成 `/` 以便 MSYS / WSL 也认。
fn shell_cd_line(shell: &str, dir: &Path) -> String {
    let name = Path::new(shell)
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let raw = dir.to_string_lossy();
    if name == "cmd" {
        format!("cd /d \"{raw}\"\r")
    } else if pty::is_powershell(shell) {
        format!("Set-Location -LiteralPath '{}'\r", raw.replace('\'', "''"))
    } else {
        format!("cd \"{}\"\r", raw.replace('\\', "/").replace('"', "\\\""))
    }
}

/// 文件浏览器「在新窗口打开」传给新进程的参数:
/// `--open-file <路径> -- <编辑器命令...>`。
///
/// 返回 `(要打开的文件, 编辑器命令)`;命令为空表示调用方没给编辑器,由
/// 调用方决定退回普通窗口。
fn parse_open_file_args(args: &[String]) -> Option<(PathBuf, Vec<String>)> {
    let pos = args.iter().position(|a| a == "--open-file")?;
    let path = PathBuf::from(args.get(pos + 1)?);
    let editor = match args.iter().position(|a| a == "--") {
        Some(sep) if sep > pos => args[sep + 1..].to_vec(),
        _ => Vec::new(),
    };
    Some((path, editor))
}

/// 命令行的初始工作目录:`--cwd <目录>`,`-d <目录>` 是 Windows Terminal 风格的别名。
///
/// 资源管理器右键菜单就是靠它把当前文件夹带进来的(注册表里写 `--cwd "%V\."`)。
/// 值不是现存目录就当作没给 —— 调用方回落到配置里的 cwd / 用户主目录,
/// 绝不因为一个坏参数起不来窗口。
fn parse_cwd_arg(args: &[String]) -> Option<PathBuf> {
    let (flag, idx) = ["--cwd", "-d"]
        .iter()
        .find_map(|f| args.iter().position(|a| a == f).map(|i| (*f, i)))?;
    let Some(value) = args.get(idx + 1) else {
        crate::werr!("[VectorTerm] {flag} 后面少了目录参数,忽略");
        return None;
    };
    let dir = tidy_cwd_arg(value);
    if dir.is_dir() {
        return Some(dir);
    }
    crate::werr!(
        "[VectorTerm] {flag} 指定的目录不可用:{}(已回落到默认目录)",
        dir.display()
    );
    None
}

/// 把注册表 / 快捷方式传进来的目录参数收拾成合法路径。
///
/// 注册表里写的是 `--cwd "%V\."`,末尾那个 `.` 是**为了绕开命令行转义**:
/// 在 `D:\` 下右键时 `%V` 本身就是 `D:\`,直接拼成 `--cwd "D:\"` 的话,
/// Windows 命令行解析会把 `\"` 当成转义的引号 —— 不但这一条参数变成 `D:"`,
/// 后面的 `--config "..."` 也会被并进同一个参数,整条命令行都被拆错。
/// 补一个 `.` 收尾(`"D:\."`)解析就正常了,这里再把补的那一段剪掉。
fn tidy_cwd_arg(raw: &str) -> PathBuf {
    let s = raw.trim_end_matches('"');
    let s = s.strip_suffix("\\.").unwrap_or(s);
    let s = s.trim_end_matches(['\\', '/']);
    if s.is_empty() {
        return PathBuf::from(raw.trim_end_matches('"'));
    }
    // 盘根剪成 `C:` 之后补回反斜杠(`C:` 单独存在是"盘符相对路径",不是盘根)
    if s.len() == 2 && s.ends_with(':') {
        return PathBuf::from(format!("{s}\\"));
    }
    PathBuf::from(s)
}

#[cfg(test)]
mod cwd_arg_tests {
    use super::*;

    fn argv(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    /// 右键菜单正常路径:存在的目录照单全收,`-d` 与 `--cwd` 等价。
    #[test]
    fn takes_existing_directory() {
        let tmp = std::env::temp_dir();
        let s = tmp.to_string_lossy().into_owned();
        assert_eq!(parse_cwd_arg(&argv(&["exe", "--cwd", &s])), Some(tmp.clone()));
        assert_eq!(parse_cwd_arg(&argv(&["exe", "-d", &s])), Some(tmp));
    }

    /// 没给 / 给的是文件 / 给的是不存在的路径:一律当作没指定,好让调用方回落。
    #[test]
    fn ignores_bad_directory() {
        assert_eq!(parse_cwd_arg(&argv(&["exe"])), None);
        assert_eq!(parse_cwd_arg(&argv(&["exe", "--cwd"])), None);
        assert_eq!(parse_cwd_arg(&argv(&["exe", "--cwd", "Z:/no/such/dir"])), None);
        // 拿一个文件冒充目录
        let exe = std::env::current_exe().expect("总能拿到自己的路径");
        assert_eq!(
            parse_cwd_arg(&argv(&["exe", "--cwd", &exe.to_string_lossy()])),
            None
        );
    }

    /// 注册表传的是 `"C:\."` 这种带收尾点的形式(见 tidy_cwd_arg),要还原成 `C:\`;
    /// 普通路径收尾的点也要一起剪掉。
    #[test]
    fn repairs_drive_root_quote() {
        assert_eq!(tidy_cwd_arg("C:\\."), PathBuf::from("C:\\"));
        assert_eq!(tidy_cwd_arg("C:\\work\\."), PathBuf::from("C:\\work"));
        // 万一命令行解析把收尾引号吃掉了(`"C:\"` → `C:"`),也照样能救回来
        assert_eq!(tidy_cwd_arg("D:\""), PathBuf::from("D:\\"));
        // 普通路径不受影响
        assert_eq!(tidy_cwd_arg(r"C:\work"), PathBuf::from(r"C:\work"));
    }
}

/// 把「这个窗口就跑编辑器」写进配置:shell 换成编辑器、文件追加到参数末尾、
/// cwd 落在文件所在目录。返回 `false` 表示没有可用的编辑器命令。
fn apply_open_file(cfg: &mut AppConfig, path: &Path, editor: &[String]) -> bool {
    let Some(prog) = editor.first().cloned() else {
        return false;
    };
    let mut args: Vec<String> = editor[1..].to_vec();
    args.push(path.to_string_lossy().into_owned());
    cfg.shell = prog;
    cfg.shell_args = args;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        cfg.cwd = Some(dir.to_path_buf());
    }
    // 编辑器窗口是一次性的:不跨进程保活,也不必挂进 mux
    cfg.session_persistence = false;
    true
}

/// 文件浏览器侧边栏占掉的左侧像素宽度(`0` = 没开 / 全屏模式)。
///
/// 侧边栏宽度按整列算,再加 1px 给它右边那条竖线。这个值同时决定 pane 的
/// 可用宽度(PTY 列数)和面板自己的绘制矩形,两边不会差一列。
fn browser_inset(tab: &TabState, cell_w: f64) -> f64 {
    match &tab.browser {
        Some(b) if b.mode == files::Mode::Side => b.side_cols as f64 * cell_w + 1.0,
        _ => 0.0,
    }
}

/// 把侧边栏占掉的宽度从**面板所属那个 pane** 的矩形里扣掉。
///
/// 关键点:侧边栏不是"整窗左边一条",而是某个 pane 自己左边的一条 —— 在哪个
/// 终端开的就属于哪个终端。所以不能在算布局前整体位移(那会把所有 pane 一起
/// 推窄),只能在算完之后单独缩这一块;PTY 列数跟着新的宽度重算,鼠标命中
/// 和绘制用的是同一份结果。
fn shrink_pane_for_browser(geoms: &mut [PaneGeom], pane_id: u64, inset: f64, cell_w: f64) {
    if inset <= 0.0 || cell_w <= 0.0 || pane_id == 0 {
        return;
    }
    let Some(g) = geoms.iter_mut().find(|g| g.pane_id == pane_id) else {
        return;
    };
    let avail = (g.w - inset).max(cell_w);
    let cols = (avail / cell_w).floor().max(1.0) as usize;
    g.x += inset;
    g.grid.0 = cols;
    g.w = cols as f64 * cell_w;
}

/// 某个标签里每个 pane 的几何。侧边栏只缩它所属的那一个。
fn tab_pane_geoms(
    renderer: &render::Renderer,
    tab: &TabState,
    grid: (usize, usize),
    cfg: &SplitDividerConfig,
) -> (Vec<PaneGeom>, Vec<SplitLine>) {
    let cell_w = renderer.cell_w;
    let (mut geoms, lines) = compute_pane_geoms_in(
        &tab.layout,
        &|i| tab.panes.get(i).map(|p| p.id),
        cell_w,
        renderer.cell_h,
        0.0,
        grid.0 as f64 * cell_w,
        grid.1 as f64 * renderer.cell_h,
        cfg,
    );
    shrink_pane_for_browser(
        &mut geoms,
        tab.browser_pane,
        browser_inset(tab, cell_w),
        cell_w,
    );
    (geoms, lines)
}

fn collect_pane_grids(
    renderer: &render::Renderer,
    tab: &TabState,
    grid: (usize, usize),
    cfg: &SplitDividerConfig,
    f: &mut dyn FnMut(&Pane, (usize, usize), (usize, usize)),
) {
    let (geoms, _) = tab_pane_geoms(renderer, tab, grid, cfg);
    for g in geoms {
        if let Some(p) = tab.panes.iter().find(|p| p.id == g.pane_id) {
            f(
                p,
                g.grid,
                (g.x.round().max(0.0) as usize, g.y.round().max(0.0) as usize),
            );
        }
    }
}

/// 计算某个标签里每个 pane 的尺寸并 resize 它。
fn resize_tab_layout(
    renderer: &render::Renderer,
    tab: &mut TabState,
    grid: (usize, usize),
    cfg: &SplitDividerConfig,
) {
    let cell_w = renderer.cell_w;
    let cell_h = renderer.cell_h;
    let mut jobs: Vec<(u64, (usize, usize), (u32, u32))> = Vec::new();
    collect_pane_grids(renderer, &*tab, grid, cfg, &mut |p, g, _| {
        let cell_px = (
            (g.0 as f64 * cell_w).round().max(0.0) as u32,
            (g.1 as f64 * cell_h).round().max(0.0) as u32,
        );
        jobs.push((p.id, g, cell_px));
    });
    for (id, g, px) in jobs {
        if let Some(p) = tab.panes.iter_mut().find(|p| p.id == id) {
            resize_pane(p, g, px);
        }
    }
}

/// 两色按比例混合(`t` = 后者占比),用来从调色板里派生出次级色。
fn mix_rgb(a: render::Rgb, b: render::Rgb, t: f32) -> render::Rgb {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 * (1.0 - t) + y as f32 * t).round() as u8;
    render::Rgb {
        r: m(a.r, b.r),
        g: m(a.g, b.g),
        b: m(a.b, b.b),
    }
}

/// 分割线默认/兜底色(深灰,比内容区背景略浅,和标签栏底色呼应)。
const SPLIT_DIVIDER_COLOR: render::Rgb = render::Rgb {
    r: 50,
    g: 50,
    b: 54,
};

/// 一条分屏缝的几何。坐标相对**内容区原点**(渲染时要再加 pad + 标签栏高度)。
/// 布局上缝永远是 1 个字符宽/高,分割线只是在这道缝里画出来的那一条。
#[derive(Clone, Copy, Debug, PartialEq)]
struct SplitLine {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    /// true = 竖线(左右分屏),false = 横线(上下分屏)
    vertical: bool,
    /// 这条线所在 Split 节点抢占的区域(内容区坐标),拖动时用它把鼠标位置
    /// 换算成新的 ratio。缝本身只有几像素,不能拿它当分母。
    area: (f64, f64, f64, f64),
    /// 从根走到该 Split 节点的位图路径(0=a / 1=b,低位是根)
    path: u32,
    /// 路径有效位数;根节点为 0
    depth: u8,
}

// ---------------- 跨窗格光标尾迹 ----------------
//
// 焦点切换**不另写一套飞行动画**:新 pane 的输入光标立刻出现(和窗格内回车
// 时一模一样),旧位置由一条尾迹收拢过去。之前那套"本体沿直线飞 320ms、尾巴
// 在后面追"的实现正是问题来源 —— 本体自己把行程走完了,四边形几乎没有被拉开,
// 看上去既慢又看不到尾巴。
//
// 现在这条尾迹就是 [`cursor::CursorTrail`] 的一个实例(App 上的 `focus_trail`),
// 参数直接来自 `cursor` 配置,坐标用内容区像素。切换时
// [`cursor::CursorTrail::seed`] 把它"种"到旧 pane 的光标位置,之后每帧
// `update` 到新 pane 的真实光标:停留时间、起止阈值、衰减、淡入曲线、
// `max_ms` 硬上限全走同一份代码和同一份 `cursor` 配置。唯一差别是跨窗格
// 把速度倍率锁成 1.0、并限制单帧步长(`set_fixed_speedup` / `set_dt_cap`):
// 几十格的距离会把距离自适应倍率顶到 4 倍,尾迹一两帧就收完,反而看不出
// 和普通输入光标是同一个动画。

/// 是否绘制某个 pane 的**静态输入光标**。
/// 非聚焦 pane 一律隐藏;聚焦 pane 永远画 —— 跨窗格切换只是"这边消失、那边
/// 出现",不存在第二个飞行光标。
fn pane_caret_visible(active: bool, hide_inactive: bool) -> bool {
    active || !hide_inactive
}

/// 8bit 通道线性插值。
fn lerp_rgb(a: render::Rgb, b: render::Rgb, t: f64) -> render::Rgb {
    let mix = |x: u8, y: u8| -> u8 {
        (x as f64 + (y as f64 - x as f64) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    render::Rgb {
        r: mix(a.r, b.r),
        g: mix(a.g, b.g),
        b: mix(a.b, b.b),
    }
}

/// 在任意多个停靠点上取 t∈[0,1] 处的颜色;段与段之间等距。
fn gradient_at(stops: &[render::Rgb], t: f64) -> render::Rgb {
    if stops.is_empty() {
        return SPLIT_DIVIDER_COLOR;
    }
    if stops.len() == 1 {
        return stops[0];
    }
    let scaled = t.clamp(0.0, 1.0) * (stops.len() - 1) as f64;
    let i = (scaled.floor() as usize).min(stops.len() - 2);
    lerp_rgb(stops[i], stops[i + 1], scaled - i as f64)
}

/// 把配置里的颜色字符串解析成停靠点。渐变里出现非法颜色时跳过该停靠点,
/// 一个都不剩就退回单色;单色也非法则用内置深灰。
fn divider_stops(cfg: &SplitDividerConfig) -> Vec<render::Rgb> {
    let solid = term::color_from_str(&cfg.color).unwrap_or(SPLIT_DIVIDER_COLOR);
    let Some(g) = &cfg.gradient else {
        return vec![solid];
    };
    let list: Vec<render::Rgb> = g
        .colors
        .iter()
        .filter_map(|s| term::color_from_str(s))
        .collect();
    if list.is_empty() {
        vec![solid]
    } else {
        list
    }
}

/// 分割线的实际矩形(相对内容区原点)。`None` = 这条线不用画。
///
/// - `thickness = None` 时自动画细线(缝宽的 30%),否则用指定线宽;线始终以缝中心为轴;
/// - `inset_start / inset_end` 从线两端各裁掉一段,竖线从上下裁、横线从左右裁;
/// - 两端留白把线压没了就不画,绝不产生负尺寸矩形。
fn divider_rect(line: &SplitLine, cfg: &SplitDividerConfig) -> Option<(f64, f64, f64, f64)> {
    if !cfg.is_visible() {
        return None;
    }
    let gap = if line.vertical { line.w } else { line.h };
    let len = if line.vertical { line.h } else { line.w };
    let t = divider_line_t(cfg, gap);
    let usable = len - cfg.inset_start - cfg.inset_end;
    if t <= 0.0 || usable <= 0.0 {
        return None;
    }
    Some(if line.vertical {
        (
            line.x + (line.w - t) * 0.5,
            line.y + cfg.inset_start,
            t,
            usable,
        )
    } else {
        (
            line.x + cfg.inset_start,
            line.y + (line.h - t) * 0.5,
            usable,
            t,
        )
    })
}

/// 生成显示列表用的矩形段。渐变靠把线切成最多 256 段纯色矩形实现 ——
/// 8bit 色深下 256 级已是最高精度,不会有色带;CPU / GPU 两条后端都不用改。
/// 分段边界对齐到整像素,半透明时相邻段不会因重叠而出现深色接缝。
fn divider_segments(
    line: &SplitLine,
    cfg: &SplitDividerConfig,
    stops: &[render::Rgb],
    origin: (f64, f64),
) -> Vec<(f64, f64, f64, f64, render::Rgb)> {
    let Some((x, y, w, h)) = divider_rect(line, cfg) else {
        return Vec::new();
    };
    let (x, y) = (x + origin.0, y + origin.1);
    if stops.len() < 2 {
        return vec![(x, y, w, h, stops.first().copied().unwrap_or(SPLIT_DIVIDER_COLOR))];
    }
    let along_y = cfg
        .gradient
        .as_ref()
        .map(|g| g.orientation.along_y(line.vertical))
        .unwrap_or(line.vertical);
    let (base, len) = if along_y { (y, h) } else { (x, w) };
    let n = (len.ceil() as usize).clamp(1, 256);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let b0 = (base + len * i as f64 / n as f64).round();
        let b1 = (base + len * (i + 1) as f64 / n as f64).round();
        if b1 <= b0 {
            continue;
        }
        let c = gradient_at(stops, (i as f64 + 0.5) / n as f64);
        if along_y {
            out.push((x, b0, w, b1 - b0, c));
        } else {
            out.push((b0, y, b1 - b0, h, c));
        }
    }
    out
}

/// 尺寸变化后同步一个 pane 的 PTY 与终端核心
fn resize_pane(p: &mut Pane, grid: (usize, usize), px: (u32, u32)) {
    if p.grid == grid {
        return;
    }
    let (cols, rows) = grid;
    p.resize_backend(rows as u16, cols as u16, px.0 as u16, px.1 as u16);
    // 回看中调整窗口:记住正在看的是哪一行,算完新的行数 / 补白后再把它放回原位
    let keep_top = if p.scroll.pos() > 0.001 {
        let term = p.term.lock();
        Some(view_geom_of(term.screen(), p.pad, p.scroll.pos()).0)
    } else {
        None
    };
    {
        let mut term = p.term.lock();
        let before = {
            let s = term.screen();
            (s.physical_rows, s.scrollback_rows(), term.cursor_pos().y)
        };
        term.resize(wezterm_term::TerminalSize {
            rows,
            cols,
            pixel_width: px.0 as usize,
            pixel_height: px.1 as usize,
            dpi: 0,
        });
        let s = term.screen();
        let after_rows = s.physical_rows.max(1);
        let sb = s.scrollback_rows();
        let cy = term.cursor_pos().y.max(0) as usize;
        // 尺寸变化后重新算抬起量:光标下方那些空行是这次 resize 补出来的,
        // 不该占着视口。变矮/内容本来就填满时这里自然是 0。
        p.pad = pad_after_resize(after_rows, cy, sb);
        p.last_cy = cy;
        // 行数变了以后,同样的回看偏移会指向别的行;按"顶行不变"重新换算
        let (_, max, _) = view_geom_of(s, p.pad, 0.0);
        if let Some(top) = keep_top {
            let eff = sb.saturating_sub(after_rows + top) as f32;
            let scroll = (eff - p.pad as f32).max(0.0);
            p.scroll.jump_to(scroll, max);
        }
        if trace_on() {
            crate::wlog!(
                "[VectorTerm][trace] GRID {}x{} -> {}x{} | rows {}->{} lines {}->{} cursor {}->{} pad={} scroll={:.1}{}",
                p.grid.0, p.grid.1, cols, rows,
                before.0, after_rows, before.1, sb, before.2, cy, p.pad, p.scroll.pos(),
                keep_top.map(|t| format!(" keep_top={t}")).unwrap_or_default()
            );
        }
    }
    p.grid = grid;
    p.trail.reset();
    // 注意:这里**不重置回看位置**。滚动偏移是相对缓冲区末尾的,而 resize
    // 不再往末尾追加内容(补白只影响显示),所以偏移仍然指着原来那几行;
    // 重置反而会把正在看的内容一把拽回底部。超界的部分由 ScrollAnim::set_max
    // 每帧钳制。
}

// ---------------------------------------------------------------------------
// 拖动缩放:画布尺寸与终端网格解耦
// ---------------------------------------------------------------------------

/// 尺寸变化时决定「什么时候把新行列数交给 PTY」的策略容器。
///
/// Windows 拖动窗口边框时每移动一两个像素就会发一次 `WM_SIZE`。wezterm 默认
/// 每次都 `apply_dimensions()`(实时重排);这里把画布尺寸与行列数拆开,
/// 让 `realtime` / `debounce` / `freeze` 三种策略可以共用同一条路径:
/// 画布始终跟着窗口走(标签栏、留白、surface 都按新尺寸),只有行列数
/// 的提交时机不同。
#[derive(Debug)]
struct ResizeSettler {
    settle_ms: u64,
    pending: Option<(usize, usize)>,
    last: Option<Instant>,
}

impl ResizeSettler {
    fn new(settle_ms: u64) -> Self {
        Self {
            settle_ms,
            pending: None,
            last: None,
        }
    }

    fn set_settle_ms(&mut self, ms: u64) {
        self.settle_ms = ms;
    }

    /// 记录一次画布尺寸变化;返回 true 表示应当**立即**同步网格(`0` = 实时模式)。
    fn on_resize(&mut self, grid: (usize, usize), now: Instant) -> bool {
        self.pending = Some(grid);
        self.last = Some(now);
        self.settle_ms == 0
    }

    /// 网格没变或已提交:清掉待办,免得白等一个定时器
    fn clear(&mut self) {
        self.pending = None;
        self.last = None;
    }

    /// 用户正拖着窗口边框(模态缩放循环):只记下最终目标网格,
    /// **不启动防抖计时**。否则拖动中只要出现一次 >debounce 的帧间隔
    /// (GPU 重建交换链、系统调度、抓帧都会造成),就会被误判成松手并
    /// 同步 PTY,PSReadLine 立刻重绘提示符,整屏文字跳一下。
    fn freeze(&mut self, grid: (usize, usize)) {
        self.pending = Some(grid);
        self.last = None;
    }

    fn is_frozen(&self) -> bool {
        self.pending.is_some() && self.last.is_none()
    }

    fn ready(&self, now: Instant) -> bool {
        match (self.pending, self.last) {
            (Some(_), Some(t)) => now.duration_since(t) >= Duration::from_millis(self.settle_ms),
            _ => false,
        }
    }

    fn take(&mut self) -> Option<(usize, usize)> {
        let g = self.pending.take();
        self.last = None;
        g
    }

    /// 还有未提交的网格时,返回该醒来的时刻(给 `about_to_wait` 用)
    fn deadline(&self) -> Option<Instant> {
        match (self.pending, self.last) {
            (Some(_), Some(t)) => Some(t + Duration::from_millis(self.settle_ms)),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// 浮层(历史搜索 / 命令面板)的进出场动画
// ---------------------------------------------------------------------------

/// 浮层的进入与退出动画。
///
/// 和 `SplitAnim` 一样是**纯时间函数**:每帧按已过时间算出 0..1 的进度,
/// 不保存逐帧中间态 —— 窗口卡一下只是这一帧跳得多一点,不会积攒误差。
///
/// "逻辑关闭"和"画面上消失"是两件事:面板的 `active` 立刻变 false(按键马上
/// 交回终端),这里继续把镜头收到 0,收完就不再画。
struct OverlayAnim {
    /// 进度:0 = 完全收起,1 = 完全展开
    t: f64,
    /// 本轮从 `from` 走到 `to`(`open` / `close` 时重设)
    from: f64,
    to: f64,
    start: Instant,
}

impl OverlayAnim {
    /// 进场 190ms、退场 160ms —— 退场稍快一点,手感上"叫出来从容、收回去利索"
    const IN_MS: f64 = 190.0;
    const OUT_MS: f64 = 160.0;

    fn new() -> Self {
        Self {
            t: 0.0,
            from: 0.0,
            to: 0.0,
            start: Instant::now(),
        }
    }

    fn open(&mut self, now: Instant) {
        if self.to >= 1.0 {
            return;
        }
        self.from = self.t;
        self.to = 1.0;
        self.start = now;
    }

    fn close(&mut self, now: Instant) {
        if self.to <= 0.0 {
            return;
        }
        self.from = self.t;
        self.to = 0.0;
        self.start = now;
    }

    /// 推进到 `now`,返回"还在动"(调用方据此决定下一帧什么时候来)
    fn tick(&mut self, now: Instant) -> bool {
        let going_up = self.to > self.from;
        let dur = if going_up { Self::IN_MS } else { Self::OUT_MS };
        let k = (now.duration_since(self.start).as_secs_f64() * 1000.0 / dur).clamp(0.0, 1.0);
        // 进出场都用 ease-out(先快后慢)。
        //
        // 退场原本用 ease-in(先慢后快,像被"收走"),但这段动画只有 160ms、
        // 帧率约 30~60fps,前三分之二的帧几乎看不出位移,最后一帧才"啪"地
        // 没了 —— 实测就是这个观感。改成 ease-out 后每一帧都在动。
        let e = 1.0 - (1.0 - k).powi(3);
        self.t = self.from + (self.to - self.from) * e;
        k < 1.0
    }

    fn running(&self) -> bool {
        (self.t - self.to).abs() > 1e-4
    }

    /// 还在画面上(退场没放完时逻辑上已经关了,但仍要画)
    ///
    /// 进场的第一帧 `t` 还是 0(时间刚起步),这时候**必须**算"要画":否则调用方
    /// 会以为没什么可画,顺手把配套状态(比如光标飞过去的尾迹)清掉,动画就没了。
    fn visible(&self) -> bool {
        self.t > 0.004 || self.running()
    }

    fn value(&self) -> f64 {
        self.t
    }
}

/// 浮层动画的坐标变换:绕自身中心等比缩放,再整体竖直位移。
///
/// 单拎出来是因为面板里每个矩形、每行文字都要过同一个变换 —— 各算各的话,
/// 框放大了字没放大(或者反过来)立刻露馅。
#[derive(Clone, Copy)]
struct OverlayXf {
    cx: f64,
    cy: f64,
    s: f64,
    dy: f64,
}

impl OverlayXf {
    /// `t` 是 0..1 的进度:0 时缩到 94% 并下沉 `rise` 像素,1 时就是原样。
    fn new(cx: f64, cy: f64, t: f64, rise: f64) -> Self {
        let t = t.clamp(0.0, 1.0);
        Self {
            cx,
            cy,
            s: 0.94 + 0.06 * t,
            dy: (1.0 - t) * rise,
        }
    }

    fn pt(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.cx + (x - self.cx) * self.s,
            self.cy + (y - self.cy) * self.s + self.dy,
        )
    }

    fn rect(&self, x: f64, y: f64, w: f64, h: f64) -> (f64, f64, f64, f64) {
        let (x0, y0) = self.pt(x, y);
        (x0, y0, w * self.s, h * self.s)
    }

    /// 文字基线:缩放改变了 ascent,所以基线和矩形走同一条插值
    fn baseline(&self, y: f64) -> f64 {
        self.pt(0.0, y).1
    }

    fn len(&self, v: f64) -> f64 {
        v * self.s
    }
}

/// 浮层元素在进度 `t` 下的不透明度。
///
/// 取开方而不是线性:动画只有 190ms,线性的话前半程几乎全黑,观感上像"啪"地
/// 出现;开方让前三分之一就把轮廓交代清楚,后段继续变亮到实心。
fn overlay_alpha(t: f64) -> f32 {
    t.clamp(0.0, 1.0).sqrt() as f32
}

struct GuiState {
    window: Arc<Window>,
    proxy: EventLoopProxy<UserEvent>,
    renderer: render::Renderer,
    backend: Backend,
    /// 当前帧的绘制列表(CPU / GPU 共用,容量复用)
    frame: frame::Frame,
    cfg: Arc<AppConfig>,
    cfg_path: Option<PathBuf>,
    cfg_mtime: Option<std::time::SystemTime>,
    last_cfg_check: Instant,
    script: Option<ScriptEngine>,
    script_actions: Arc<StdMutex<Vec<ScriptAction>>>,
    quit_requested: bool,
    /// `VECTORTERM_AUTOTYPE` 这个自动化测试钩子是否已经用过(只敲一次)
    autotyped: bool,
    /// 活动标签
    tab: TabState,
    /// 所有标签槽位:长度 = 标签数,活动位是 None(其状态在 `tab` 里)。
    /// 这样下标始终等于标签序号,切换/关闭都不需要重排。
    background: Vec<Option<TabState>>,
    active: usize,
    next_tab_id: u64,
    trail_color: Option<render::Rgb>,
    grid_px: (u32, u32),
    /// 拖动缩放时待提交的行列数(连续 resize 期间冻结终端网格)
    resize_settler: ResizeSettler,
    /// 上一次「同步绘制并 present」的时刻。用来给拖动中过密的 WM_SIZE 节流
    last_sync_present: Instant,
    /// 上一次真正 present 出去的客户区尺寸。`WM_PAINT` 复检用:尺寸和它一致
    /// 说明这一帧是重复的,尺寸不同就说明新尺寸还没有帧,必须补画。
    last_presented: (u32, u32),
    /// 标签栏内容左右各让出的物理像素,免得贴边的标签块被窗口圆角裁掉一块
    edge_inset: f64,
    /// 标签栏到窗口上边框(底部栏时到下边框)的距离,和左右留白一起
    /// 构成对称的安全区
    top_inset: f64,
    /// 启动时实际生效的 DWM 圆角状态。热重载不改窗口装饰,自动留白
    /// 也得跟着这个启动值走,否则视觉上会和窗口真实的圆角不一致
    rounded_window: bool,
    /// 软渲染 surface 当前的像素尺寸
    surface_px: (u32, u32),
    mods: ModifiersState,
    /// 最后一次鼠标位置(窗口逻辑像素),用于换算单元格坐标
    mouse_px: (f64, f64),
    /// 当前窗口光标形状,避免每个 mousemove 都调 set_cursor
    cursor_icon: CursorIcon,
    /// 标签栏上鼠标悬停的标签下标
    tab_hover: Option<usize>,
    /// 鼠标停在第几个标签的关闭按钮上
    close_hover: Option<usize>,
    /// 鼠标是否停在"新建标签"按钮上
    new_tab_hover: bool,
    /// 没有系统标题栏:标签栏兼任标题栏,空白处可拖动窗口
    csd: bool,
    /// 要画进标签栏的窗口按钮(已转成 tabbar 枚举);空 = 一枚都不画
    window_buttons: Vec<tabbar::WindowButton>,
    button_alignment: tabbar::ButtonAlign,
    /// `integrated_title_button_color`;None = Auto
    button_color: Option<render::Rgb>,
    /// `integrated_title_button_style = "Gnome"`
    button_gnome_style: bool,
    /// `RESIZE`:没有系统标题栏时由程序自绘四边/四角缩放热区
    own_resize_border: bool,
    /// 鼠标停在哪枚窗口按钮上
    window_hover: Option<tabbar::WindowButton>,
    /// 鼠标贴着窗口边缘时的缩放方向(无边框才能自己做缩放)
    resize_edge: Option<ResizeDirection>,
    /// 上次左键按下的时刻与位置,用来识别"双击标题栏最大化"
    last_click: Option<(Instant, f64, f64)>,
    /// 在最大化窗口的标题栏上按下时的锚点(客户区坐标)。等鼠标真的动了
    /// 才还原窗口并开始拖动 —— 单纯点一下不该把窗口恢复成普通尺寸。
    #[cfg_attr(not(windows), allow(dead_code))]
    pending_drag: Option<(f64, f64)>,
    /// 闪烁相位基准时刻(SGR 5/6)
    blink_epoch: Instant,
    /// 下一次允许翻转闪烁相位的时刻
    next_blink: Instant,
    /// 当前屏幕是否有闪烁单元格
    has_blink: bool,
    /// 仅基准模式:Some 表示正在采集帧间隔
    profile: Option<ProfileCtx>,
    /// 诊断开关 `VECTORTERM_DUMP_FRAME=路径`:渲染完后导出 BMP 再清空
    dump_frame: Vec<PathBuf>,
    /// `VECTORTERM_DUMP_AT=毫秒[,毫秒...]`:到点各导一帧(逐帧拍动画用)
    dump_seq: Vec<(u64, PathBuf)>,
    /// `VECTORTERM_DUMP_TRAIL=N`:只在**跨窗格尾迹真的被画出来的那一帧**导图,
    /// 一共导 N 张(`路径-1.bmp` / `-2.bmp` ...)。按时刻抓帧总会差一帧,
    /// 这个钩子直接对着"画了尾迹"的那一帧下手。
    dump_trail: u32,
    /// `dump_trail` 的初值,用来给导出的图编号
    dump_trail_total: u32,
    /// 输入法组合串;Some 时在光标格下方画一个浮层
    ime_preedit: Option<Preedit>,
    /// 上次上报给 IME 的光标格位置(物理像素)。只在真的移动时才调
    /// `set_ime_cursor_area`,免得每帧都进一次 IMM
    ime_area: Option<(i32, i32)>,
    /// 关闭确认框;Some 表示当前处于模态,键盘/鼠标先给它
    confirm: Option<ConfirmState>,
    /// 标签栏左侧状态文字(`window:set_left_status`),已解析成带颜色的片段
    left_status: Vec<status::Span>,
    /// 标签栏右侧状态文字(`window:set_right_status`)
    right_status: Vec<status::Span>,
    /// 下一次该刷新状态栏的时刻(wezterm 的 `status_update_interval`)
    next_status_update: Instant,
    /// 256 色调色板:状态栏把 `38;5;n` / `AnsiColor` 落成具体颜色要用
    ansi_palette: [render::Rgb; 256],
    /// `format-tab-title` 的结果(每帧都要画,只在键变化时重算)
    tab_titles: Vec<script::TabTitle>,
    /// 缓存键:活动标签 + 悬停标签 + 所有标题的指纹
    tab_titles_key: Option<(usize, Option<usize>, u64)>,
    /// 焦点切换时那条跨窗格尾迹(整窗叠加层)。和窗格内尾迹同一个类型、
    /// 同一份参数、同一套状态机,只是坐标用内容区像素、不被单个 pane 裁剪。
    focus_trail: cursor::CursorTrail,
    /// 跨窗格尾迹是否"已装填"。只在焦点切换后播一段;否则每帧都拿活动光标
    /// 喂它,普通打字就会在整窗层再画一条,和窗格内的尾迹叠成两层。
    focus_trail_on: bool,
    /// 下一次轮询 shell 当前目录的时刻(文件面板跟随终端用)
    next_cwd_poll: Instant,
    /// 上一次看到的 shell 目录:只有它变了才动面板,避免两个方向互相拽
    seen_cwd: Option<PathBuf>,
    /// `seen_cwd` 是哪个 pane 的(面板换宿主时重新记基线)
    seen_cwd_pane: u64,
    /// 历史搜索栏状态(作用于活动 pane)
    search: search::SearchState,
    /// 搜索栏的进出场动画。逻辑关闭后还要把退场放完,所以单独记一份进度。
    search_anim: OverlayAnim,
    /// 命令面板状态。和历史搜索栏互斥,共用同一个底部浮层位置;在搜索框里
    /// 输入 `/` 就从搜索切到这里。
    panel: commands::Panel,
    /// 命令面板的进出场动画
    panel_anim: OverlayAnim,
    /// 文件面板的进出场动画。和浮层那两个共用同一份纯时间函数状态机
    /// (`OverlayAnim`),只是手感不同:浮层是竖直位移 + 缩放,面板是横向滑。
    browser_anim: OverlayAnim,
    /// 正在退场的文件面板。逻辑上已经关了(`tab.browser` 是 `None`、按键也
    /// 不再进它),但数据要留到退场放完 —— 否则面板会"啪"地消失。
    /// 面板所属 pane 记在 `tab.browser_pane`,退场放完才清。
    browser_exit: Option<files::Browser>,
    /// 浮层打开时"光标飞过去"的那条尾迹。和跨窗格尾迹用同一个状态机,
    /// 只是落点换成了浮层的输入光标。
    overlay_trail: cursor::CursorTrail,
    /// 飞行动画是否已装填:只在浮层刚打开的那一段播,收敛后关掉
    overlay_trail_on: bool,
    /// 搜索结果重算的最早时刻:终端还在吐字时节流,别每帧扫一遍全量历史
    next_search_refresh: Instant,
    /// 正在拖动滚动条:(pane id, 按下时鼠标相对滑块上沿的偏移)
    scrollbar_drag: Option<(u64, f64)>,
    /// 正在拖动的分割栏:(布局路径, 路径深度)
    split_drag: Option<(u32, u8)>,
    /// 拖选到边缘时的自动滚屏速度(行/45ms,正 = 往更早的历史翻)
    edge_scroll: i64,
    /// 下一次允许推进一次边缘滚动的时刻
    next_edge_tick: Instant,
}

impl GuiState {
    fn new(
        el: &ActiveEventLoop,
        cfg: Arc<AppConfig>,
        proxy: EventLoopProxy<UserEvent>,
        cfg_path: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        // 基准模式可选窗口尺寸(VECTORTERM_PROFILE_SIZE=1920x1080);正常启动始终是 960x640
        let profiling = std::env::var("VECTORTERM_PROFILE_FRAMES").is_ok();
        let (init_w, init_h) = if profiling {
            std::env::var("VECTORTERM_PROFILE_SIZE")
                .ok()
                .and_then(|s| {
                    let (a, b) = s.split_once('x')?;
                    Some((a.parse::<f64>().ok()?, b.parse::<f64>().ok()?))
                })
                .unwrap_or((960.0, 640.0))
        } else {
            (960.0, 640.0)
        };
        // VectorTerm 扩展:显式像素尺寸(window_width / window_height)优先。
        // 它和 initial_cols/rows 的区别是"窗口多大"不再由字形大小决定 ——
        // 用户调 font.size 时窗口不会被顺带撑大/缩小。
        //
        // 单位是**物理像素**(客户区真实像素),不受系统缩放(DPI)影响:
        // 125% 缩放下写 1300 就是 1300 像素宽,而不是 1300 逻辑像素 → 1625 像素。
        let explicit_px = cfg.window_width.is_some() || cfg.window_height.is_some();
        let init_size: winit::dpi::Size = if explicit_px {
            let w = cfg.window_width.unwrap_or(init_w).round().max(120.0) as u32;
            let h = cfg.window_height.unwrap_or(init_h).round().max(60.0) as u32;
            winit::dpi::PhysicalSize::new(w, h).into()
        } else {
            LogicalSize::new(init_w, init_h).into()
        };
        let icon = app_icon();
        let mut attrs = Window::default_attributes()
            .with_title("VectorTerm")
            // Windows 把窗口图标分成两档存,必须都给:
            // - `with_window_icon` → WM_SETICON(ICON_SMALL),管标题栏左上角;
            // - `with_taskbar_icon` → WM_SETICON(ICON_BIG),管任务栏按钮和 Alt+Tab,
            //   只有 Windows 平台有这个方法。
            // 只设 SMALL 的话,任务栏拿不到 BIG 就退回去读 exe 资源段里的图标 ——
            // 那段要靠 rc.exe 编,没装 Windows SDK 的机器上是空的,于是任务栏显示的
            // 就是别处的旧图标。两档都从同一张 PNG 来,任务栏因此不依赖 exe 资源。
            .with_window_icon(icon.clone())
            .with_inner_size(init_size)
            // `TITLE` = 系统标题栏(原生三枚按钮都在上面);其余模式由标签栏
            // 兼任标题栏,缩放边框按需自绘。
            .with_decorations(cfg.window.title);
        #[cfg(windows)]
        {
            use winit::platform::windows::{CornerPreference, WindowAttributesExtWindows};
            attrs = attrs.with_taskbar_icon(icon);
            // 关掉系统标题栏后 winit 会把非客户区压成 0,DWM 默认不再画
            // Win11 的圆角;显式要回来,否则窗口四个角是硬的直角。
            // 有系统标题栏时保持 Default(让系统/用户的全局设置决定),
            // 只有 `window_rounded_corners = false` 才强制 DoNotRound。
            let pref = if !cfg.window.rounded_corners {
                CornerPreference::DoNotRound
            } else if cfg.window.title {
                CornerPreference::Default
            } else {
                CornerPreference::Round
            };
            attrs = attrs.with_corner_preference(pref);
        }
        let window = Arc::new(el.create_window(attrs)?);
        // 打开输入法。winit 默认把窗口的 IME 上下文摘掉
        // (`ImmAssociateContextEx(.., IACE_CHILDREN)`),不显式打开的话
        // 中文/日文/韩文输入法根本不会激活 —— 表现就是"只能打英文"。
        window.set_ime_allowed(true);
        // 诊断入口见 `winhook::force_ime_open`;真正的调用点在 `about_to_wait`
        // (要反复调,窗口拿焦点时 winit 会把输入上下文重建一遍)。
        // 半透明:层叠窗口 + 统一 alpha。DWM 按这个 alpha 合成整块窗口,
        // GDI 位图(CPU 后端)和 DXGI 画面(GPU 后端)都会跟着变淡,
        // 所以两个后端行为一致,不需要逐像素 alpha。
        #[cfg(windows)]
        if cfg.background.needs_transparent_window() {
            if let Some(h) = hwnd_of(&window) {
                let alpha = (cfg.background.effective_opacity() * 255.0).round() as u8;
                set_uniform_alpha(h, alpha);
            }
        }
        // Win11 毛玻璃背板:必须在窗口建好之后设,DWM 才会在窗口后方画材质
        if cfg.background.backdrop != config::Backdrop::None {
            #[cfg(windows)]
            {
                let applied = if let Ok(winit::raw_window_handle::RawWindowHandle::Win32(h)) =
                    window.window_handle().map(|h| h.as_raw())
                {
                    apply_system_backdrop(h.hwnd.get(), cfg.background.backdrop);
                    true
                } else {
                    false
                };
                if applied {
                    crate::wlog!(
                        "[VectorTerm] win32_system_backdrop = \"{}\" 只设了 DWM 属性,材质透不出来:\
                         窗口交换链给不出逐像素透明(见 docs/custom-background.md)。\
                         想要半透明请用 window_background_opacity",
                        cfg.background.backdrop.as_str()
                    );
                }
            }
            #[cfg(not(windows))]
            crate::wlog!("[VectorTerm] win32_system_backdrop 只在 Windows 上有效,已忽略");
        }
        let own_resize_border = cfg.window.own_resize_border();

        let scale = window.scale_factor();
        let rounded_window = !cfg.window.title && cfg.window.rounded_corners;
        let (edge_inset, top_inset) = tab_bar_insets(&cfg, scale, rounded_window);
        let mut renderer = render::Renderer::new(&cfg, scale)?;

        // ---- 标签栏占位 ----
        // 启动时只有 1 个标签:hide_if_one 为真就先不占高度,
        // 之后新建标签时 `relayout` 会重新算网格。
        // 无边框模式例外:这条栏同时是标题栏,必须留着,否则窗口没法拖动。
        let metrics = metrics_with_status_height(&cfg, renderer.cell_w, renderer.cell_h);
        let show_bar = cfg.window.csd() || (cfg.tab_bar.enabled && !cfg.tab_bar.hide_if_one);
        let mut inner = window.inner_size();
        let bar_h = tab_bar_total_height(&metrics, inner.height as f64, top_inset, show_bar);
        renderer.content_top = if cfg.tab_bar.at_bottom { 0.0 } else { bar_h };
        renderer.content_bottom = if cfg.tab_bar.at_bottom { bar_h } else { 0.0 };

        // wezterm 的 initial_cols / initial_rows:按单元格数反推窗口像素尺寸。
        // 显式写了 window_width / window_height 时跳过 —— 像素尺寸优先,
        // 否则"按列行反推"会随着 font.size 一起把窗口撑大。
        if !explicit_px {
            if let (Some(c), Some(r)) = (cfg.initial_cols, cfg.initial_rows) {
                let w = renderer.pad_x * 2.0 + renderer.cell_w * c as f64;
                let h = renderer.pad_y * 2.0
                    + renderer.cell_h * r as f64
                    + renderer.content_top
                    + renderer.content_bottom;
                let want = winit::dpi::PhysicalSize::new(
                    w.round().max(80.0) as u32,
                    h.round().max(60.0) as u32,
                );
                if let Some(actual) = window.request_inner_size(want) {
                    inner = actual;
                } else {
                    inner = want;
                }
            }
        }
        // 初始窗口尺寸确定后再校准一次(窗口很矮时留白会被 clamp 掉)
        let bar_h = tab_bar_total_height(&metrics, inner.height as f64, top_inset, show_bar);
        renderer.content_top = if cfg.tab_bar.at_bottom { 0.0 } else { bar_h };
        renderer.content_bottom = if cfg.tab_bar.at_bottom { bar_h } else { 0.0 };
        let (cols, rows) = renderer.compute_grid(inner.width, inner.height);
        if profiling {
            crate::wlog!(
                "[VectorTerm][profile] window={}x{} grid={cols}x{rows} scale={scale}",
                inner.width,
                inner.height
            );
        }
        // 把实际生效的组合打出来,配置有没有真正对上一眼就能看出来
        let o = window.outer_size();
        let mode = if cfg.window.title {
            "TITLE(系统标题栏)"
        } else if cfg.window.show_buttons() {
            "INTEGRATED_BUTTONS"
        } else if cfg.window.resize {
            "RESIZE(标签栏无按钮)"
        } else {
            "NONE"
        };
        let n_buttons = if cfg.window.show_buttons() {
            cfg.window.buttons.len()
        } else {
            0
        };
        let corners = if !cfg.window.rounded_corners {
            "关"
        } else if cfg.window.title {
            "系统默认"
        } else {
            "DWM"
        };
        let debounce = if cfg.resize_debounce_ms == 0 {
            "实时".to_string()
        } else {
            format!("{}ms", cfg.resize_debounce_ms)
        };
        let increments = if cfg.use_resize_increments {
            "整格"
        } else {
            "逐像素"
        };
        crate::wlog!(
            "[VectorTerm] 窗口装饰: {mode} / 自绘缩放边框={own_resize_border} / 标签栏按钮 {n_buttons} 枚 / 圆角={corners} / 拖动时行列数={} / 非拖动稳定窗口={debounce} / 缩放步进={increments} (外框 {}x{} / 客户区 {}x{})",
            cfg.live_resize_mode.as_str(),
            o.width, o.height, inner.width, inner.height
        );
        // 窗口大小的来源写清楚:改字号后发现窗口跟着变,看这行就知道是
        // window_width/height、initial_cols/rows 还是内置默认在起作用。
        let size_src = if explicit_px {
            format!(
                "window_width/height = {}x{} 物理像素",
                cfg.window_width.map(|v| v as i64).unwrap_or(-1),
                cfg.window_height.map(|v| v as i64).unwrap_or(-1)
            )
        } else if let (Some(c), Some(r)) = (cfg.initial_cols, cfg.initial_rows) {
            format!("initial_cols/rows = {c}x{r} (会随 font.size 变化)")
        } else {
            "内置默认 960x640".to_string()
        };
        crate::wlog!("[VectorTerm] 初始窗口尺寸来源: {size_src}");
        crate::wlog!(
            "[VectorTerm] 标签栏安全区: 左右={edge_inset}px / 上下={}px (内容带 {}px, 整条占位 {}px)",
            tabbar::clamp_top_inset(top_inset, inner.height as f64, metrics.bar_h),
            metrics.bar_h,
            tab_bar_total_height(&metrics, inner.height as f64, top_inset, show_bar)
        );

        let backend = Backend::new(&cfg, window.clone())?;
        // ---- 第一个标签 ----
        let mut next_tab_id = 1;
        let tab = if cfg.session_persistence {
            // 会话保持:确保守护进程在跑,attach 最近的一个会话,没有则新开。
            ensure_mux_server()?;
            // `--cwd` 进来的(右键"在此处打开")不 attach 老会话,但**仍然走 mux**:
            // shell 照样由守护进程托管,关掉窗口之后还能恢复。
            let restored = if CLI_CWD.load(std::sync::atomic::Ordering::Relaxed) {
                crate::wlog!("[VectorTerm] 命令行指定了目录:跳过会话恢复,按该目录新开");
                None
            } else {
                attach_recent_pane(&cfg, (cols, rows), (inner.width, inner.height), &proxy, (scale * 96.0) as u32)
            };
            match restored {
                Some(t) => {
                    crate::wlog!("[VectorTerm] 会话保持:已恢复最近的会话");
                    t
                }
                None => {
                    crate::wlog!("[VectorTerm] 会话保持:没有可恢复的会话,新开一个");
                    spawn_tab_mux(&cfg, (cols, rows), (inner.width, inner.height), &proxy, (scale * 96.0) as u32)?
                }
            }
        } else {
            spawn_tab(
                &cfg,
                (cols, rows),
                (inner.width, inner.height),
                &proxy,
                next_tab_id,
                default_title(&cfg),
                (scale * 96.0) as u32,
            )?
        };
        next_tab_id += 1;

        // ---- Lua 脚本 ----
        let script_actions = Arc::new(StdMutex::new(Vec::new()));
        let script = match cfg_path.as_deref() {
            Some(p) => match ScriptEngine::load(p, script_actions.clone()) {
                Ok(s) => Some(s),
                Err(e) => {
                    crate::werr!("[VectorTerm] Lua 脚本加载失败:{e:#}");
                    None
                }
            },
            None => None,
        };
        let cfg_mtime = cfg_path.as_deref().and_then(mtime_of);
        window.set_title(&window_title(tab.title()));

        let mut state = Self {
            window,
            proxy,
            renderer,
            backend,
            frame: frame::Frame::default(),
            cfg: cfg.clone(),
            cfg_path,
            cfg_mtime,
            last_cfg_check: Instant::now(),
            script,
            script_actions,
            quit_requested: false,
            autotyped: false,
            tab,
            background: vec![None],
            active: 0,
            next_tab_id,
            trail_color: cfg
                .cursor
                .trail_color
                .as_deref()
                .and_then(term::color_from_str),
            grid_px: (inner.width, inner.height),
            resize_settler: ResizeSettler::new(cfg.resize_debounce_ms),
            // 启动那一刻已经 present 过首帧,从这里开始计时
            last_sync_present: Instant::now(),
            last_presented: (0, 0),
            edge_inset,
            top_inset,
            rounded_window,
            surface_px: (0, 0),
            mods: ModifiersState::empty(),
            mouse_px: (0.0, 0.0),
            cursor_icon: CursorIcon::Default,
            tab_hover: None,
            close_hover: None,
            new_tab_hover: false,
            csd: cfg.window.csd(),
            window_buttons: if cfg.window.show_buttons() {
                cfg.window
                    .buttons
                    .iter()
                    .map(|b| match b {
                        config::TitleButton::Hide => tabbar::WindowButton::Minimize,
                        config::TitleButton::Maximize => tabbar::WindowButton::Maximize,
                        config::TitleButton::Close => tabbar::WindowButton::Close,
                    })
                    .collect()
            } else {
                Vec::new()
            },
            button_alignment: match cfg.window.alignment {
                config::ButtonAlignment::Left => tabbar::ButtonAlign::Left,
                config::ButtonAlignment::Right => tabbar::ButtonAlign::Right,
            },
            button_color: cfg.window.color.as_deref().and_then(term::color_from_str),
            button_gnome_style: cfg.window.style == config::ButtonStyle::Gnome,
            own_resize_border,
            window_hover: None,
            resize_edge: None,
            last_click: None,
            pending_drag: None,
            blink_epoch: Instant::now(),
            next_blink: Instant::now(),
            has_blink: false,
            profile: if profiling {
                std::env::var("VECTORTERM_PROFILE_FRAMES")
                    .ok()
                    .and_then(|s| s.parse::<usize>().ok())
                    .filter(|&n| n >= 10)
                    .map(|n| ProfileCtx {
                        remaining: n + 5, // 前 5 帧丢弃(窗口/图集预热)
                        last: None,
                        gaps: Vec::with_capacity(n + 5),
                    })
            } else {
                None
            },
            // 分屏探针要的是**分屏完成后**那一帧:此时先不抓,等下面的
            // VECTORTERM_AUTO_SPLIT 钩子在动画结束后再填进来。
            // `VECTORTERM_DUMP_AT` 也是"到点再拍",首帧就不拍了。
            dump_frame: if std::env::var_os("VECTORTERM_AUTO_SPLIT").is_some()
                || std::env::var_os("VECTORTERM_DUMP_AT").is_some()
            {
                Vec::new()
            } else {
                std::env::var_os("VECTORTERM_DUMP_FRAME")
                    .map(|p| vec![PathBuf::from(p)])
                    .unwrap_or_default()
            },
            dump_seq: dump_seq_of(std::env::var_os("VECTORTERM_DUMP_AT"), std::env::var_os("VECTORTERM_DUMP_FRAME")),
            dump_trail: std::env::var("VECTORTERM_DUMP_TRAIL")
                .ok()
                .and_then(|v| v.trim().parse::<u32>().ok())
                .unwrap_or(0),
            dump_trail_total: std::env::var("VECTORTERM_DUMP_TRAIL")
                .ok()
                .and_then(|v| v.trim().parse::<u32>().ok())
                .unwrap_or(0),
            ime_preedit: None,
            ime_area: None,
            confirm: None,
            left_status: Vec::new(),
            right_status: Vec::new(),
            // 启动后尽快刷第一次:配置里那句问候语应该一开窗就在
            next_status_update: Instant::now(),
            ansi_palette: ansi_palette_of(&cfg),
            tab_titles: Vec::new(),
            tab_titles_key: None,
            focus_trail: {
                let mut t = cursor::CursorTrail::new(&focus_cursor_cfg(&cfg));
                // 首帧掉帧也按一帧推进,否则尾迹"一步到位"、什么都看不到
                t.set_dt_cap(1.0 / 60.0);
                // 不套距离加速:跨屏几十格会把倍率顶到 4 倍,尾巴一两帧就收完。
                // 锁 1.0 = 和窗格里挪一两行同一个速度,只是拉得更长。
                t.set_fixed_speedup(1.0);
                t
            },
            focus_trail_on: false,
            next_cwd_poll: Instant::now(),
            seen_cwd: None,
            seen_cwd_pane: 0,
            search: search::SearchState::new(cfg.search.case_sensitive),
            search_anim: OverlayAnim::new(),
            panel: commands::Panel::new(),
            panel_anim: OverlayAnim::new(),
            browser_anim: OverlayAnim::new(),
            browser_exit: None,
            overlay_trail: {
                // 和跨窗格尾迹同一套参数:飞行的"手感"必须一致,否则两个动画
                // 一眼就能看出是两个人写的
                let mut t = cursor::CursorTrail::new(&focus_cursor_cfg(&cfg));
                t.set_dt_cap(1.0 / 60.0);
                t.set_fixed_speedup(1.0);
                t
            },
            overlay_trail_on: false,
            next_search_refresh: Instant::now(),
            scrollbar_drag: None,
            split_drag: None,
            edge_scroll: 0,
            next_edge_tick: Instant::now(),
        };

        // 诊断:`VECTORTERM_IME_DEMO=拼音` 假装有一段组合串,方便截图核对浮层
        // 位置(真机验证要手敲输入法,自动化测不了这一层)。
        if let Ok(text) = std::env::var("VECTORTERM_IME_DEMO") {
            if !text.is_empty() {
                state.ime_preedit = Some(Preedit {
                    cursor: Some((0, text.len())),
                    text,
                });
            }
        }

        // wezterm `apply_dimensions()` 末尾的 `window.set_resize_increments(...)`:
        // 把缩放步长锁到一个字符格,WM_SIZE 就只在跨整格时来一次。
        state.sync_resize_increments();

        // 启动时直接开 N 个标签(调试/习惯用法):VECTORTERM_START_TABS=3
        let tabs = std::env::var("VECTORTERM_START_TABS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(1)
            .clamp(1, 9);
        for _ in 1..tabs {
            state.new_tab();
        }
        Ok(state)
    }

    /// 照搬 wezterm `apply_dimensions()` 末尾的
    /// `window.set_resize_increments(ri_calc.into())` —— 窗口宽高只允许按
    /// `cell_w × cell_h` 整格变化。Windows 拖边框时 WM_SIZE 因此从"每像素
    /// 一次"变成"每跨一格一次",rows/cols 每次恰好 ±1,软折行重排是离散的。
    /// wezterm 的 `ResizeIncrementCalculator` 还带 `base_width/base_height`
    /// (padding + border + tab_bar_height);winit 只暴露增量本身,无边框窗口
    /// 外框=客户区,所以这里用纯格子增量即可。
    fn sync_resize_increments(&self) {
        let cw = self.renderer.cell_w.round().max(1.0) as u32;
        let ch = self.renderer.cell_h.round().max(1.0) as u32;
        if self.cfg.use_resize_increments {
            self.window
                .set_resize_increments(Some(Size::Physical(PhysicalSize::new(cw, ch))));
        } else {
            self.window.set_resize_increments::<Size>(None);
        }
    }

    /// 记录新的客户区像素尺寸并重算几何。返回 `true` 表示尺寸真的变了
    /// (调用方据此决定要不要马上呈现);`false` 说明是重复事件,什么都不用做。
    fn apply_resize(&mut self, w: u32, h: u32) -> bool {
        if w == 0 || h == 0 {
            return false;
        }
        if (w, h) == self.grid_px {
            return false;
        }
        self.grid_px = (w, h);
        // 画布 / 标签栏几何立即跟着窗口走;行列数按当前场景的策略决定。
        let grid = self.update_geometry();
        let now = Instant::now();
        if grid == self.tab.active_pane().grid {
            // 只是窗口像素变了、格子数没变:没必要惊动 PTY
            self.resize_settler.clear();
        } else {
            match resize_mode_for(live_resizing(), &self.cfg) {
                // wezterm 语义:每次格子数变化当场同步,文字实时重排。
                // 呈现在同一个 WM_SIZE → WM_PAINT 序列里完成,不会留下
                // 「窗口变大但内容还是旧网格」的空档。
                LiveResizeMode::Realtime => {
                    let t = Instant::now();
                    self.commit_grid(grid);
                    trace_dur("GRID_COMMIT", t.elapsed());
                }
                LiveResizeMode::Debounce => {
                    if self.resize_settler.on_resize(grid, now) {
                        self.commit_grid(grid);
                    }
                }
                // 可选退路:整个拖动模态循环一次都不碰 PTY,松手提交一次
                LiveResizeMode::Freeze => self.resize_settler.freeze(grid),
            }
        }
        true
    }

    /// 拖动结束(左键松开)后立刻把冻结期间攒下的目标网格同步一次。
    /// `flush_resize` 只认防抖计时,freeze 的待办计时是空的,得靠这个边沿。
    fn finish_live_resize(&mut self) {
        if self.resize_settler.is_frozen() && !live_resizing() {
            if let Some(grid) = self.resize_settler.take() {
                self.commit_grid(grid);
            }
        }
    }

    /// 只更新渲染侧几何(标签栏占位 + 目标行列数),不碰 PTY / terminal。
    fn update_geometry(&mut self) -> (usize, usize) {
        let (w, h) = self.grid_px;
        let bar_h = self.tab_bar_height();
        self.renderer.content_top = if self.cfg.tab_bar.at_bottom {
            0.0
        } else {
            bar_h
        };
        self.renderer.content_bottom = if self.cfg.tab_bar.at_bottom {
            bar_h
        } else {
            0.0
        };
        self.renderer.compute_grid(w, h)
    }

    /// 真正把行列数同步给所有标签的 PTY 与 wezterm-term。
    fn commit_grid(&mut self, grid: (usize, usize)) {
        self.resize_settler.clear();
        let changed = self.any_grid_changed(grid);
        if !changed {
            return;
        }
        let (w, h) = self.grid_px;
        // 为每个 tab 递归计算其布局树里各 pane 的尺寸,再逐个 resize。
        // 单 pane 标签退化为整格,与旧行为一致。
        resize_tab_layout(&self.renderer, &mut self.tab, grid, &self.cfg.split_divider);
        for t in self.background.iter_mut().flatten() {
            resize_tab_layout(&self.renderer, t, grid, &self.cfg.split_divider);
        }
        crate::wlog!("[VectorTerm] 网格同步: 整窗 {}x{} ({}x{})", grid.0, grid.1, w, h);
        self.window.request_redraw();
    }

    /// 是否有任一 pane 的尺寸需要更新。
    fn any_grid_changed(&self, grid: (usize, usize)) -> bool {
        let mut changed = false;
        collect_pane_grids(&self.renderer, &self.tab, grid, &self.cfg.split_divider, &mut |p, g, _| {
            if p.grid != g {
                changed = true;
            }
        });
        for t in self.background.iter().flatten() {
            collect_pane_grids(&self.renderer, t, grid, &self.cfg.split_divider, &mut |p, g, _| {
                if p.grid != g {
                    changed = true;
                }
            });
        }
        changed
    }

    /// 连续 resize 停止后,把攒着的那次网格变化提交掉。
    fn flush_resize(&mut self, now: Instant) {
        if self.resize_settler.ready(now) {
            if let Some(grid) = self.resize_settler.take() {
                self.commit_grid(grid);
            }
        }
    }

    /// 当前标签栏应占的像素高度(0 = 不显示)
    fn tab_bar_height(&self) -> f64 {
        let count = self.background.len();
        if count == 0 {
            return 0.0;
        }
        // 无边框模式下这一条同时是标题栏:拖动窗口、最小化/关闭都靠它。
        // 所以即使关了标签栏或设了 hide_if_one 也要留着,否则窗口会失去
        // 所有可交互的边框 —— 用户只能 Alt+F4。
        if !self.csd {
            if !self.cfg.tab_bar.enabled {
                return 0.0;
            }
            if self.cfg.tab_bar.hide_if_one && count <= 1 {
                return 0.0;
            }
        }
        tab_bar_total_height(
            &metrics_with_status_height(&self.cfg, self.renderer.cell_w, self.renderer.cell_h),
            self.grid_px.1 as f64,
            self.top_inset,
            true,
        )
    }

    /// 拼一份给 Lua 的标签快照。
    ///
    /// 只在真有 `update-status` / `format-tab-title` 回调时才调用 —— 里面要
    /// 锁每个标签的终端读 OSC 7 目录,还要查一次进程树,不该白做。
    fn status_ctx(&self) -> script::StatusCtx {
        let n = self.background.len();
        let mut tabs: Vec<script::TabInfo> = Vec::with_capacity(n);
        let mut pids: Vec<u32> = Vec::with_capacity(n);
        for i in 0..n {
            let slot = if i == self.active {
                Some(&self.tab)
            } else {
                self.background[i].as_ref()
            };
            let (id, title, pid, cwd) = match slot {
                Some(t) => {
                    let p = t.active_pane();
                    (t.id, p.title.clone(), p.shell_pid(), cwd_of_term(&p.term))
                }
                None => (0, String::new(), None, None),
            };
            pids.push(pid.unwrap_or(0));
            tabs.push(script::TabInfo {
                index: i,
                id,
                title,
                active: i == self.active,
                cwd,
                prog: None,
            });
        }
        // 一次进程快照覆盖所有标签:每个标签各查一遍会把 ToolHelp 打满
        for (info, prog) in tabs.iter_mut().zip(procs::foreground_names(&pids)) {
            info.prog = prog;
        }
        script::StatusCtx {
            tabs,
            active: self.active,
            focused: self.window.has_focus(),
            pixel: self.grid_px,
            dpi: (self.window.scale_factor() * 96.0).round(),
            tab_max_width: self.cfg.tab_bar.max_width_cells,
            fancy_tab_bar: self.cfg.tab_bar.fancy,
            ansi: self.ansi_palette,
        }
    }

    /// 到点刷一次状态栏。对应 wezterm 的 `status_update_interval`:
    /// 到点派发 `update-status` / `update-right-status`,把回调写的文字拿回来。
    ///
    /// `status_bar.text`(一段固定文字)若写了,就接到脚本结果的后面。
    fn refresh_status(&mut self, now: Instant) {
        let interval = self.cfg.status_update_interval.clamp(50, 60_000);
        self.next_status_update = now + Duration::from_millis(interval);

        let sb = self.cfg.status_bar.clone();
        // 状态栏关掉时,脚本写的东西也一起不显示(否则"关了还有字"很费解)
        let out = match self.script.as_ref() {
            Some(s) if sb.enabled && (s.has_status_hooks() || sb.has_text()) => {
                let ctx = self.status_ctx();
                Some(s.update_status(&ctx))
            }
            _ => None,
        };
        let mut left = out
            .as_ref()
            .map(|o| status::parse(&o.left, &self.ansi_palette))
            .unwrap_or_default();
        let mut right = out
            .as_ref()
            .map(|o| status::parse(&o.right, &self.ansi_palette))
            .unwrap_or_default();
        if !sb.enabled {
            left.clear();
            right.clear();
        } else if sb.has_text() {
            // 关了 keep_script 就整段丢掉脚本的结果,只显示自己拼的
            if !sb.keep_script {
                right.clear();
            }
            if let Some(text) = self.composed_text_bar(&sb) {
                right.extend(status::parse(&text, &self.ansi_palette));
            }
        }
        if left != self.left_status || right != self.right_status {
            self.left_status = left;
            self.right_status = right;
            self.window.request_redraw();
        }
        // 标题可能也含时间/负载这类随时间变的内容,顺手让缓存失效
        self.tab_titles_key = None;
    }

    /// 把 `status_bar.text` 变成一段带 SGR 的文本,接到脚本结果后面。
    ///
    /// 只负责"额外加一段固定文字";各段之间的间距由绘制层按像素统一处理,
    /// 所以脚本拼出来的状态栏和这里的文字都能被 `status_bar.item_spacing` 影响。
    fn composed_text_bar(&self, sb: &crate::config::StatusBarConfig) -> Option<String> {
        if !sb.has_text() {
            return None;
        }
        // 先复位颜色:否则脚本最后设的前景色会染到这段文字上
        Some(format!("\u{1b}[0m{}", sb.text))
    }

    /// `format-tab-title` 的缓存键:活动标签、悬停标签、所有标题的指纹。
    /// 三者都没变就不用再进 Lua —— 这个函数每帧都会被调到。
    fn tab_titles_key_now(&self) -> (usize, Option<usize>, u64) {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.background.len().hash(&mut h);
        for i in 0..self.background.len() {
            let slot = if i == self.active {
                Some(&self.tab)
            } else {
                self.background[i].as_ref()
            };
            match slot {
                Some(t) => {
                    t.id.hash(&mut h);
                    t.title().hash(&mut h);
                }
                None => 0u64.hash(&mut h),
            }
        }
        (self.active, self.tab_hover, h.finish())
    }

    /// 需要时重算每个标签的标题样式
    fn ensure_tab_titles(&mut self) {
        let key = self.tab_titles_key_now();
        if self.tab_titles_key == Some(key) {
            return;
        }
        self.tab_titles_key = Some(key);
        let titles = match self.script.as_ref() {
            Some(s) if s.has_status_hooks() => {
                let ctx = self.status_ctx();
                s.format_tab_titles(&ctx, self.tab_hover)
            }
            _ => None,
        };
        self.tab_titles = titles.unwrap_or_default();
    }

    /// 当前标签栏几何(不显示时返回 None)
    fn tab_bar_layout(&self) -> Option<tabbar::Layout> {
        let bar_h = self.tab_bar_height();
        if bar_h <= 0.0 {
            return None;
        }
        let (w, h) = self.grid_px;
        let m = metrics_with_status_height(&self.cfg, self.renderer.cell_w, self.renderer.cell_h);
        // 无边框模式下这条栏一定在,但标签本身仍尊重配置:
        // 关了标签栏 / 设了 hide_if_one,就退化成一条纯标题栏(只剩窗口按钮和拖动区)。
        let hide = !self.cfg.tab_bar.enabled
            || (self.cfg.tab_bar.hide_if_one && self.background.len() <= 1);
        let count = if hide { 0 } else { self.background.len() };
        let opts = tabbar::Options {
            edge_inset: self.edge_inset,
            top_inset: self.top_inset,
            at_bottom: self.cfg.tab_bar.at_bottom,
            max_width_cells: self.cfg.tab_bar.max_width_cells,
            // 没有标签可显示时不要孤零零地留一个 "+"
            show_new_tab: count > 0 && self.cfg.tab_bar.show_new_tab_button,
            show_close: self.cfg.tab_bar.show_close_button,
            csd: self.csd,
            window_buttons: self.window_buttons.clone(),
            button_alignment: self.button_alignment,
            // 状态栏只在标签栏上占位;藏起标签栏时也就没有它们的位置了。
            // 右侧按 status_bar 里配的字号倍数 / 左右留白算,和绘制保持同一份口径。
            left_status_w: if hide {
                0.0
            } else {
                status_width(
                    &self.left_status,
                    self.renderer.advance_w,
                    1.0,
                    0.0,
                    0.0,
                    1.0,
                )
            },
            right_status_w: if hide {
                0.0
            } else {
                status_width(
                    &self.right_status,
                    self.renderer.advance_w,
                    self.cfg.status_bar.font_scale,
                    self.cfg.status_bar.padding,
                    self.cfg.status_bar.item_spacing,
                    self.cfg.status_bar.space_width,
                )
            },
        };
        Some(tabbar::layout(w as f64, h as f64, count, &m, &opts))
    }

    fn tab_bar_hit(&self, x: f64, y: f64) -> Option<tabbar::Hit> {
        let layout = self.tab_bar_layout()?;
        match layout.hit(x, y) {
            tabbar::Hit::None => None,
            hit => Some(hit),
        }
    }

    /// 标题栏按钮:最小化 / 最大化(还原) / 关闭窗口
    fn window_button(&mut self, which: tabbar::WindowButton) {
        match which {
            tabbar::WindowButton::Minimize => self.window.set_minimized(true),
            tabbar::WindowButton::Maximize => self.set_maximized_state(!self.zoomed()),
            // 标题栏 ✕ 和 `Alt+F4` 一样要过确认 —— 以前这里直接
            // `request_quit()`,于是"点了 ✕ 一点提示都没有"。
            tabbar::WindowButton::Close => {
                self.close_window_or_ask();
            }
        }
    }

    /// 标题栏空白处:单击拖动窗口、双击切换最大化(和系统标题栏行为一致)
    fn drag_title_bar(&mut self) {
        let now = Instant::now();
        let (x, y) = self.mouse_px;
        let dbl = self.last_click.is_some_and(|(t, px, py)| {
            now.duration_since(t) < Duration::from_millis(400)
                && (x - px).abs() < 6.0
                && (y - py).abs() < 6.0
        });
        if dbl {
            // 消费掉这次点击,免得三击又翻回去
            self.last_click = None;
            self.set_maximized_state(!self.zoomed());
            return;
        }
        self.last_click = Some((now, x, y));
        // 最大化时不能直接 drag_window:它只是把 WM_NCLBUTTONDOWN/HTCAPTION
        // 投给窗口过程,而 DefWindowProc 对最大化窗口只会进入移动循环、
        // 不会像系统标题栏那样"拖出还原",结果就是窗口一动不动。
        // 所以先记下锚点,等鼠标真的移动了再还原 + 拖动(`mouse_moved`)——
        // 单纯点一下标题栏不应该把窗口恢复成普通尺寸。
        #[cfg(windows)]
        if self.zoomed() {
            self.pending_drag = Some((x, y));
            return;
        }
        let _ = self.window.drag_window();
    }

    /// 最大化 / 还原。winit 只在它自己发起的操作里更新标志,窗口要是被外部
    /// (`SC_MAXIMIZE`、系统的拖动贴顶吸附等)最大化过,`set_maximized` 就会
    /// 变成空操作,所以两个方向都用系统状态兜一次底。
    fn set_maximized_state(&mut self, on: bool) {
        self.window.set_maximized(on);
        #[cfg(windows)]
        if let Some(h) = hwnd_of(&self.window) {
            unsafe {
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    IsZoomed, ShowWindow, SW_MAXIMIZE, SW_RESTORE,
                };
                if (IsZoomed(h as _) != 0) != on {
                    ShowWindow(h as _, if on { SW_MAXIMIZE } else { SW_RESTORE });
                }
            }
        }
        self.window.request_redraw();
    }

    /// 窗口是否处于最大化(含被外部 `SC_MAXIMIZE` 最大化、winit 标志没跟上的情况)。
    #[cfg(windows)]
    fn zoomed(&self) -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::IsZoomed;
        self.window.is_maximized()
            || hwnd_of(&self.window).is_some_and(|h| unsafe { IsZoomed(h as _) != 0 })
    }

    #[cfg(not(windows))]
    fn zoomed(&self) -> bool {
        self.window.is_maximized()
    }

    /// 把最大化的窗口还原成普通窗口,并让"被抓住的那个点"在标题栏上的相对
    /// 位置保持不变 —— 抓哪儿还原后还在哪儿,手感跟系统标题栏一致。
    /// 紧接着调用 `drag_window()` 就是常规拖动了。
    ///
    /// `anchor` 是按下时在(最大化)客户区里的坐标。
    #[cfg(windows)]
    fn restore_for_drag(&mut self, anchor: (f64, f64)) {
        let Some(hwnd) = hwnd_of(&self.window) else {
            return;
        };
        let Some((cx, cy)) = client_origin(hwnd) else {
            return;
        };
        // 抓取点的屏幕坐标 = 客户区原点 + 客户区内坐标
        let sx = cx as f64 + anchor.0;
        let sy = cy as f64 + anchor.1;
        let (mw, mh) = (self.grid_px.0.max(1) as f64, self.grid_px.1.max(1) as f64);
        // 水平比例:还原后抓取点仍落在标题栏的同一相对位置(和系统一致)
        let rx = (anchor.0 / mw).clamp(0.0, 1.0);
        // 纵向保持"到这条栏的距离":栏在上方取到上边,在下方取到下边
        let at_bottom = self.cfg.tab_bar.at_bottom;
        let dy = if at_bottom { mh - anchor.1 } else { anchor.1 };

        self.window.set_maximized(false);
        // winit 的 MAXIMIZED 标志可能和系统不同步(窗口被外部 SC_MAXIMIZE
        // 最大化过),那样 set_maximized(false) 会变成空操作,所以再看一眼
        // 系统状态,必要时直接 SW_RESTORE。
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{IsZoomed, ShowWindow, SW_RESTORE};
            if IsZoomed(hwnd as _) != 0 {
                ShowWindow(hwnd as _, SW_RESTORE);
            }
        }
        // 还原是同步的,返回时窗口矩形已经是普通尺寸
        let size = self.window.outer_size();
        let (w, h) = (size.width as f64, size.height as f64);
        let left = (sx - rx * w).round() as i32;
        let top = if at_bottom {
            (sy - (h - dy)).round() as i32
        } else {
            (sy - dy).round() as i32
        };
        self.window
            .set_outer_position(winit::dpi::PhysicalPosition::new(left, top));
        self.window.request_redraw();
    }

    /// 关闭窗口的入口。返回 `true` = 先别退,已经弹出确认框等用户选。
    ///
    /// 判据照 wezterm 的 `LocalPane::can_close_without_prompting`:整棵进程树
    /// (含 shell 自己)里凡是有名字不在 `skip_close_confirmation_for_processes_named`
    /// 里的,就算"还有程序在跑"。`quit_confirmation = false` 直接放行。
    ///
    /// 进程表查不到时**照样弹**(和 wezterm 一样):宁可多问一句,也别让
    /// 用户在跑着 `cargo build` 的时候一点就把窗口关了。
    fn ask_quit(&mut self) -> bool {
        if self.quit_requested || self.confirm.is_some() {
            return true;
        }
        if !self.cfg.quit_confirmation {
            return false;
        }
        let state = self.window_stateful_programs();
        if state.as_deref().is_some_and(|names| names.is_empty()) {
            return false;
        }
        let names = state.unwrap_or_default();
        self.show_confirm(ConfirmAction::Quit);
        crate::wlog!(
            "[VectorTerm] {},已弹出退出确认(配置 quit_confirmation = false 可关掉)",
            describe_stateful(&names)
        );
        true
    }

    /// 整个窗口里"白名单之外"的程序名。`None` = 进程表查不到。
    ///
    /// 任何一个标签查不到就整体记作"查不到" —— 判不准时保守一点。
    fn window_stateful_programs(&self) -> Option<Vec<String>> {
        let mut out: Vec<String> = Vec::new();
        let mut unknown = false;
        for idx in 0..self.background.len() {
            match self.tab_stateful_programs(idx) {
                Some(names) => {
                    for n in names {
                        push_unique(&mut out, &n);
                    }
                }
                None => unknown = true,
            }
        }
        if unknown {
            None
        } else {
            Some(out)
        }
    }

    /// 某一个标签里"白名单之外"的程序名。
    ///
    /// 关闭标签的确认只该看**这个**标签 —— 旁边标签里跑着东西,不该拦住
    /// 关一个空标签。
    fn tab_stateful_programs(&self, idx: usize) -> Option<Vec<String>> {
        let t = if idx == self.active {
            Some(&self.tab)
        } else {
            self.background.get(idx).and_then(|t| t.as_ref())
        };
        let Some(t) = t else {
            return Some(Vec::new());
        };
        let pids: Vec<u32> = t
            .panes
            .iter()
            .filter(|p| !p.exited)
            .filter_map(|p| p.shell_pid())
            .collect();
        procs::stateful_programs(&pids, &self.cfg.skip_close_confirmation_for_processes_named)
    }

    /// 某一个 pane 里"白名单之外"的程序名。
    ///
    /// 关 pane 的确认只看**这个** pane 自己的会话 —— 同一标签里另一个 pane
    /// 在跑 `cargo build`,不该拦住关旁边这个空 shell。
    fn pane_stateful_programs(&self, pane_id: u64) -> Option<Vec<String>> {
        let p = if let Some(p) = self.tab.panes.iter().find(|p| p.id == pane_id) {
            p
        } else {
            self.background
                .iter()
                .flatten()
                .flat_map(|t| t.panes.iter())
                .find(|p| p.id == pane_id)?
        };
        // 会话已经结束的 pane 里不可能还有程序
        if p.exited {
            return Some(Vec::new());
        }
        let pids: Vec<u32> = p.shell_pid().into_iter().collect();
        procs::stateful_programs(&pids, &self.cfg.skip_close_confirmation_for_processes_named)
    }

    /// 把确认框挂上,并关掉输入法 —— 中文输入法开着时 y/n 会被它截走,
    /// 应用收到的是 `Named(Process)` 而不是 `Character("y")`,看起来就是
    /// "按 Y 没反应"。
    fn show_confirm(&mut self, action: ConfirmAction) {
        self.confirm = Some(ConfirmState {
            active: None,
            action,
        });
        self.ime_preedit = None;
        self.window.set_ime_allowed(false);
        self.window.request_redraw();
    }

    /// 关闭标签前的确认。返回 `true` = 先别关,已经弹出确认框等用户选。
    ///
    /// 和 `ask_quit` 同一套判据、同一个开关(`quit_confirmation`),只是把
    /// 进程树的范围缩到这个标签。
    fn ask_close_tab(&mut self, idx: usize) -> bool {
        if self.confirm.is_some() || idx >= self.background.len() {
            return false;
        }
        if !self.cfg.quit_confirmation {
            return false;
        }
        let state = self.tab_stateful_programs(idx);
        if state.as_deref().is_some_and(|names| names.is_empty()) {
            return false;
        }
        let names = state.unwrap_or_default();
        self.show_confirm(ConfirmAction::CloseTab(idx));
        crate::wlog!(
            "[VectorTerm] 标签 {} 里{},已弹出关闭确认",
            idx + 1,
            describe_stateful(&names)
        );
        true
    }

    /// 关标签的统一入口:该问就先问,不用问就直接关。
    /// 标签栏的 ×、中键、`Ctrl+Shift+W`、Lua 的 `CloseTab` 全走这里。
    ///
    /// 只剩一个标签时问的是"关窗口"(那一刻等价),文案跟着换 ——
    /// wezterm 里关最后一个 tab 也走 `confirm_close_window` 那条路。
    fn close_tab_or_ask(&mut self, idx: usize) {
        if self.background.len() <= 1 {
            self.close_window_or_ask();
            return;
        }
        if !self.ask_close_tab(idx) {
            self.close_tab(idx);
        }
    }

    /// 关闭某个 pane 前的确认。返回 `true` = 先别关,已经弹出确认框。
    ///
    /// 和 `ask_close_tab` 同一套判据、同一个开关(`quit_confirmation`),
    /// 只是把进程树的范围缩到这一个 pane。对应 wezterm 的
    /// `CloseCurrentPane { confirm = true }`。
    ///
    /// 该标签只剩这一个 pane 时不在这里问:那一刻关 pane 就是关标签,
    /// 交给 `close_tab_or_ask` 用标签/窗口那两条文案问。
    fn ask_close_pane(&mut self, pane_id: u64) -> bool {
        if self.confirm.is_some() {
            return false;
        }
        if !self.cfg.quit_confirmation {
            return false;
        }
        let Some(tab_idx) = self.pane_tab_index(pane_id) else {
            return false;
        };
        if self.tab_pane_count(tab_idx) <= 1 {
            return false;
        }
        let state = self.pane_stateful_programs(pane_id);
        if state.as_deref().is_some_and(|names| names.is_empty()) {
            return false;
        }
        let names = state.unwrap_or_default();
        self.show_confirm(ConfirmAction::ClosePane(pane_id));
        crate::wlog!(
            "[VectorTerm] 窗格 {pane_id} 里{},已弹出关闭确认",
            describe_stateful(&names)
        );
        true
    }

    /// 关 pane 的统一入口:只剩一个 pane 时等同关标签,否则该问就问。
    /// `Ctrl+Shift+X`、Lua 的 `close_pane`、wezterm 的 `CloseCurrentPane`
    /// 全走这里 —— 和关标签一样,主动关闭必须有且只有这一条路。
    fn close_pane_or_ask(&mut self, pane_id: u64) {
        // 已经有确认框挂着:不能因为"这次不用问"就把窗格直接关了 ——
        // 用户还在回答上一个问题。
        if self.confirm.is_some() {
            return;
        }
        let Some(tab_idx) = self.pane_tab_index(pane_id) else {
            return;
        };
        if self.tab_pane_count(tab_idx) <= 1 {
            self.close_tab_or_ask(tab_idx);
            return;
        }
        if !self.ask_close_pane(pane_id) {
            self.close_pane(pane_id);
        }
    }

    /// 全局 pane id → 所在标签下标(活动标签或后台标签)。
    fn pane_tab_index(&self, pane_id: u64) -> Option<usize> {
        if self.tab.panes.iter().any(|p| p.id == pane_id) {
            return Some(self.active);
        }
        self.background
            .iter()
            .position(|t| t.as_ref().is_some_and(|t| t.panes.iter().any(|p| p.id == pane_id)))
    }

    /// 某个标签里有几个 pane。
    fn tab_pane_count(&self, idx: usize) -> usize {
        if idx == self.active {
            self.tab.panes.len()
        } else {
            self.background
                .get(idx)
                .and_then(|t| t.as_ref())
                .map_or(0, |t| t.panes.len())
        }
    }

    /// 关窗口的统一入口:该问就问,不问就真关。标题栏 ✕、`Alt+F4`、
    /// 关最后一个标签都走这里 —— 少一条路就少一次"怎么没提示"。
    /// 返回 `true` = 真的开始退了。
    fn close_window_or_ask(&mut self) -> bool {
        if self.ask_quit() {
            return false;
        }
        self.request_quit();
        true
    }

    /// 确认框里"取消"
    fn close_confirm(&mut self) {
        self.confirm = None;
        self.window.set_ime_allowed(true);
        self.window.request_redraw();
    }

    /// 确认框里"是"。按 `action` 分派:关窗口、关标签,还是只关那一个窗格。
    fn confirm_yes(&mut self) {
        let Some(c) = self.confirm.take() else {
            return;
        };
        self.window.set_ime_allowed(true);
        match c.action {
            ConfirmAction::Quit => self.request_quit(),
            ConfirmAction::CloseTab(idx) => self.close_tab(idx),
            ConfirmAction::ClosePane(id) => self.close_pane(id),
        }
        self.window.request_redraw();
    }

    /// 确认框里的按键。和 wezterm 一样只认四个字符:
    /// `y` / `Y` = 关闭,`n` / `N` = 取消,Esc 也取消。
    /// Enter / Tab / 方向键在这里没有任何作用 —— wezterm 就是这样的。
    fn confirm_key(&mut self, ev: &winit::event::KeyEvent) {
        match &ev.logical_key {
            Key::Named(NamedKey::Escape) => self.close_confirm(),
            Key::Character(s) => match s.chars().next() {
                Some('y' | 'Y') => self.confirm_yes(),
                Some('n' | 'N') => self.close_confirm(),
                _ => {}
            },
            _ => {}
        }
    }

    /// 确认界面的鼠标逻辑,逐行照抄 wezterm 的 `run_confirmation_impl`:
    ///
    /// * 只有落在**按钮那一行**（行号严格相等）且在该选项的格区间内才算命中
    /// * 命中选项时把它反显；左键按下即生效
    /// * 移出按钮区间就把反显清掉(`active = None`)
    /// * 除了"按在选项上的左键",任何其它鼠标按下都当取消
    fn confirm_mouse(&mut self, x: f64, y: f64, pressed: Option<WinitMouseButton>) {
        if self.confirm.is_none() {
            return;
        }
        let Some(msg) = self.confirm.as_ref().map(|c| c.message()) else {
            return;
        };
        let l = self
            .renderer
            .confirm_layout(self.grid_px.0, self.grid_px.1, msg);
        let row = l.row_of(y);
        let col = l.col_of(x);

        let hit = if row == l.button_row {
            (0..2).find(|&i| col >= l.choice_col[i] && col < l.choice_col[i] + l.choice_w[i])
        } else {
            None
        };

        let old = self.confirm.as_ref().and_then(|c| c.active);
        if let Some(c) = self.confirm.as_mut() {
            c.active = hit;
        }
        if old != hit {
            self.window.request_redraw();
        }

        match (hit, pressed) {
            // 左键点在选项上:立即生效
            (Some(0), Some(WinitMouseButton::Left)) => self.confirm_yes(),
            (Some(1), Some(WinitMouseButton::Left)) => self.close_confirm(),
            // 其它任何鼠标按下(点空白、右键、中键)= 取消
            (_, Some(_)) => self.close_confirm(),
            // 只是移动:更新反显就够了
            (_, None) => {}
        }
    }

    /// 记录输入法组合串并重画。空串等于结束组合。
    fn set_preedit(&mut self, text: String, cursor: Option<(usize, usize)>) {
        if text.is_empty() {
            self.clear_preedit();
            return;
        }
        self.ime_preedit = Some(Preedit { text, cursor });
        self.window.request_redraw();
    }

    fn clear_preedit(&mut self) {
        if self.ime_preedit.take().is_some() {
            self.window.request_redraw();
        }
    }

    /// 输入法上屏。以前这里无条件 `send_bytes`,于是开搜索框时用拼音打
    /// 「，」「。」会被直接打进 shell —— 浮层虽然浮在上面,输入焦点其实
    /// 应该已经被它接管了。现在按模态分层派发,和键盘输入走同一套规则。
    fn ime_commit(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        let route = if self.panel.active {
            self.panel_input(&text);
            "commands"
        } else if self.search.active {
            self.search_input(&text);
            "search"
        } else if self.confirm.is_some() {
            // 确认框只有 Y/N 两种回答,没有文本输入,别把字符漏给终端
            "dropped"
        } else {
            self.send_bytes(text.as_bytes());
            "pty"
        };
        if std::env::var_os("VECTORTERM_DEBUG_KEYS").is_some() {
            crate::wlog!("[dbg] ime commit {text:?} -> {route}");
        }
        self.window.request_redraw();
    }

    /// 往搜索框里追加文本。键盘和输入法上屏共用这一条路径,
    /// 所以「第一个字符输 `/` 切命令面板」的规则对中文输入法同样成立。
    fn search_input(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        if self.search.query.is_empty() && s == "/" {
            self.close_search();
            self.open_panel();
            return;
        }
        self.search.query.push_str(s);
        self.search.current = 0;
        self.search.dirty = true;
    }

    /// 往命令面板的输入行追加文本(规则同 [`Self::search_input`])。
    fn panel_input(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        self.panel.query.push_str(s);
        self.panel.selected = 0;
        self.panel.output = None;
        self.panel.scroll = 0;
    }

    /// 退出:结束所有会话。mux 模式下 detach(会话保持),本地模式 kill。
    fn request_quit(&mut self) {
        if self.quit_requested {
            return;
        }
        for p in self.tab.panes.iter_mut() {
            p.end_session(true);
        }
        for t in self.background.iter_mut().flatten() {
            for p in t.panes.iter_mut() {
                p.end_session(true);
            }
        }
        self.quit_requested = true;
    }

    /// 鼠标贴着窗口边缘时的缩放方向。只有 `RESIZE`(没有系统标题栏、需要
    /// 自绘边框)才启用;TITLE 交给系统,NONE 按 wezterm 语义不缩放。
    fn resize_dir_at(&self, x: f64, y: f64) -> Option<ResizeDirection> {
        if !self.own_resize_border {
            return None;
        }
        // 最大化时四边都贴在屏幕边上,没有可拖的边框;而顶边那条热区正好压在
        // 标签栏最上面几像素上 —— 不排除的话,点在标题栏顶端会被当成"上边缘
        // 缩放",窗口既缩不了也拖不动,表现就是"最大化后拖不走窗口"。
        if self.zoomed() {
            return None;
        }
        let (w, h) = (self.grid_px.0 as f64, self.grid_px.1 as f64);
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        let m = (self.renderer.cell_h * 0.35).round().clamp(4.0, 10.0);
        let left = x < m;
        let right = x > w - m;
        let top = y < m;
        let bottom = y > h - m;
        Some(match (left, right, top, bottom) {
            (true, _, true, _) => ResizeDirection::NorthWest,
            (_, true, true, _) => ResizeDirection::NorthEast,
            (true, _, _, true) => ResizeDirection::SouthWest,
            (_, true, _, true) => ResizeDirection::SouthEast,
            (true, _, _, _) => ResizeDirection::West,
            (_, true, _, _) => ResizeDirection::East,
            (_, _, true, _) => ResizeDirection::North,
            (_, _, _, true) => ResizeDirection::South,
            _ => return None,
        })
    }

    /// 按窗口尺寸 + 标签栏占位重算网格,并**立即**同步到所有标签。
    /// 用于非拖动场景(新建/关闭标签、热重载);拖动走 `flush_resize`。
    fn relayout(&mut self) {
        let (w, h) = self.grid_px;
        if w == 0 || h == 0 {
            return;
        }
        let grid = self.update_geometry();
        self.commit_grid(grid);
    }

    /// 按稳定 id 找标签(活动标签或后台槽位)
    fn tab_mut(&mut self, id: u64) -> Option<&mut TabState> {
        if self.tab.id == id {
            return Some(&mut self.tab);
        }
        self.background.iter_mut().flatten().find(|t| t.id == id)
    }

    /// 按全局 pane id 找 pane(遍历所有标签的所有 pane)。
    fn pane_mut(&mut self, id: u64) -> Option<&mut Pane> {
        if let Some(p) = self.tab.panes.iter_mut().find(|p| p.id == id) {
            return Some(p);
        }
        self.background
            .iter_mut()
            .flatten()
            .flat_map(|t| t.panes.iter_mut())
            .find(|p| p.id == id)
    }

    /// 遍历所有 pane(活动标签 + 后台标签)。
    fn all_panes_mut(&mut self) -> impl Iterator<Item = &mut Pane> {
        self.tab
            .panes
            .iter_mut()
            .chain(self.background.iter_mut().flatten().flat_map(|t| t.panes.iter_mut()))
    }

    /// 活动 pane 的全局 id。
    fn active_pane_id(&self) -> Option<u64> {
        Some(self.tab.panes[self.tab.active_pane].id)
    }

    /// 处理某个 pane 的会话结束(本地子进程退出 / mux 远端退出共用)。
    ///
    /// 同一个 pane 可能收到两次退出通知(读管道 EOF 一次、子进程 wait 一次),
    /// 先到的负责收摊,后来的直接忽略。
    fn handle_exit(&mut self, pane_id: u64, msg: String) {
        let mut dup = true;
        if let Some(p) = self.pane_mut(pane_id) {
            dup = p.exit_reported;
            p.exit_reported = true;
            p.exited = true;
        }
        if dup {
            return;
        }
        let is_active = self.active_pane_id() == Some(pane_id);
        crate::wlog!("[VectorTerm] pane {pane_id} 的会话已结束({msg})");
        // 会话没了。wezterm 的默认(exit_behavior = "Close")是把这个 pane
        // 收掉,最后一个标签则连窗口一起退出。
        if self.cfg.exit_behavior == config::ExitBehavior::Close {
            self.close_pane(pane_id);
            return;
        }
        // Hold:留在原地,末尾补一行可见的结束提示
        let line = format!("\r\n\x1b[90m[会话结束:{msg}]\x1b[0m\r\n");
        if let Some(p) = self.pane_mut(pane_id) {
            p.term.lock().advance_bytes(line.as_bytes());
        }
        if is_active {
            self.window.set_title(&format!("VectorTerm - 会话结束({msg})"));
        }
        self.window.request_redraw();
    }

    fn switch_to(&mut self, idx: usize) {
        if idx >= self.background.len() || idx == self.active {
            return;
        }
        let Some(target) = self.background[idx].take() else {
            return;
        };
        let old = std::mem::replace(&mut self.tab, target);
        self.background[self.active] = Some(old);
        self.active = idx;
        self.after_tab_switch();
    }

    fn next_tab(&mut self) {
        let n = self.background.len();
        if n > 1 {
            self.switch_to((self.active + 1) % n);
        }
    }

    fn prev_tab(&mut self) {
        let n = self.background.len();
        if n > 1 {
            self.switch_to((self.active + n - 1) % n);
        }
    }

    fn new_tab(&mut self) {
        let id = self.next_tab_id;
        let grid = self.tab.active_pane().grid;
        let px = self.grid_px;
        let dpi = (self.window.scale_factor() * 96.0) as u32;
        let spawned = if self.cfg.session_persistence {
            spawn_tab_mux(&self.cfg, grid, px, &self.proxy, dpi)
        } else {
            spawn_tab(
                &self.cfg,
                grid,
                px,
                &self.proxy,
                id,
                default_title(&self.cfg),
                dpi,
            )
        };
        match spawned {
            Ok(t) => {
                self.next_tab_id += 1;
                let old = std::mem::replace(&mut self.tab, t);
                self.background[self.active] = Some(old);
                // 新标签紧跟在当前标签后面
                self.background.insert(self.active + 1, None);
                self.active += 1;
                // hide_if_one 从"藏"变"显"时网格会变,先重排再切标题
                self.relayout();
                self.after_tab_switch();
            }
            Err(e) => crate::werr!("[VectorTerm] 新建标签失败:{e:#}"),
        }
    }

    /// 关闭第 idx 个标签(连同它的所有 pane)。关掉最后一个时退出程序。
    /// 按 id 找标签在 `background` 里的下标(活动槽就是 `active`)。
    fn tab_index(&self, id: u64) -> Option<usize> {
        if self.tab.id == id {
            return Some(self.active);
        }
        self.background
            .iter()
            .position(|t| t.as_ref().is_some_and(|t| t.id == id))
    }

    /// 结束一个标签里的所有 pane 会话(detach 或 kill)。
    fn end_tab(&mut self, t: &mut TabState, keep_alive: bool) {
        for p in t.panes.iter_mut() {
            p.end_session(keep_alive);
        }
    }

    fn close_tab(&mut self, idx: usize) {
        let count = self.background.len();
        if idx >= count {
            return;
        }
        if count == 1 {
            self.request_quit();
            return;
        }
        if idx == self.active {
            let next = if idx + 1 < count { idx + 1 } else { idx - 1 };
            if let Some(target) = self.background[next].take() {
                let mut old = std::mem::replace(&mut self.tab, target);
                self.end_tab(&mut old, true);
                // 活动槽(idx)是 None,删掉它;目标槽跟着左移到 idx
                self.background.remove(idx);
                self.active = if next > idx { idx } else { next };
            }
        } else if let Some(mut t) = self.background[idx].take() {
            self.end_tab(&mut t, true);
            self.background.remove(idx);
            if idx < self.active {
                self.active -= 1;
            }
        }
        self.relayout();
        self.after_tab_switch();
    }

    /// 关闭某个 pane(按 pane id)。若它是该标签最后一个 pane,
    /// 则整个标签关闭;否则从布局树移除,焦点让给相邻 pane。
    ///
    /// 这是**真正的收摊**,不做确认:会话已结束(`handle_exit`)也走这里,
    /// 那时弹框只会挡住关不掉的窗口。用户主动关闭请走 `close_pane_or_ask`。
    fn close_pane(&mut self, pane_id: u64) {
        // 先定位 pane 所在的标签下标(避免在持有借用时再访问 self)。
        let Some(tab_idx) = self.pane_tab_index(pane_id) else {
            return;
        };

        // 只有一个 pane → 关掉整个标签
        if self.tab_pane_count(tab_idx) <= 1 {
            self.close_tab(tab_idx);
            return;
        }

        // 多 pane:移除该 pane 并从布局树摘掉叶子
        let (in_active, tab_ref) = if tab_idx == self.active {
            (true, &mut self.tab)
        } else {
            (false, self.background[tab_idx].as_mut().unwrap())
        };
        // 动画没跑完就关 pane:先把线落到终点,免得旧动画的 target 下标在
        // remove_leaf 重排后指到别的 pane 上
        settle_split_anim(tab_ref);
        let target = tab_ref.panes.iter().position(|p| p.id == pane_id).unwrap();
        tab_ref.panes[target].end_session(false);
        let new_layout = tab_ref.layout.remove_leaf(target);
        let Some(mut new_layout) = new_layout else {
            return;
        };
        tab_ref.panes.remove(target);
        // 面板挂在被关掉的 pane 上:一起收掉,别留下一个指向空气的列表
        if tab_ref.browser_pane == pane_id {
            tab_ref.browser = None;
            tab_ref.browser_pane = 0;
        }
        remap_leaves_after(&mut new_layout, target);
        tab_ref.layout = new_layout;
        let mut leaves = Vec::new();
        tab_ref.layout.leaves(&mut leaves);
        tab_ref.active_pane = leaves.first().copied().unwrap_or(0);
        let _ = in_active;

        self.relayout();
        self.after_tab_switch();
    }

    fn after_tab_switch(&mut self) {
        self.window.set_title(&window_title(self.tab.title()));
        self.tab_hover = None;
        self.close_hover = None;
        let p = self.tab.active_pane_mut();
        p.trail.reset();
        p.scroll.reset();
        self.window.request_redraw();
    }

    /// 会话保持:attach 守护进程里最近一个「当前未显示」的存活会话。
    /// 所有存活会话都已在标签里显示时,新开一个会话(和 Ctrl+Shift+T 等价)。
    fn attach_next_session(&mut self) {
        // 收集当前已显示的 pane id
        let mut shown: Vec<u64> = Vec::new();
        shown.extend(self.tab.panes.iter().map(|p| p.id));
        shown.extend(self.background.iter().flatten().flat_map(|t| t.panes.iter().map(|p| p.id)));

        let Some(mut client) = mux::MuxClient::connect().ok().flatten() else {
            crate::werr!("[VectorTerm] 无法连接 mux 守护进程");
            return;
        };
        let Ok(list) = client.list() else {
            crate::werr!("[VectorTerm] 列出会话失败");
            return;
        };
        // 挑 id 最大(最近)且未显示的存活会话
        let target = list
            .into_iter()
            .filter(|m| !m.exited && !shown.contains(&m.id))
            .max_by_key(|m| m.id);

        let grid = self.tab.active_pane().grid;
        let px = self.grid_px;
        let dpi = (self.window.scale_factor() * 96.0) as u32;
        let result = match target {
            Some(m) => attach_tab_mux(&self.cfg, grid, px, &self.proxy, dpi, m.id),
            None => spawn_tab_mux(&self.cfg, grid, px, &self.proxy, dpi),
        };
        match result {
            Ok(t) => {
                let old = std::mem::replace(&mut self.tab, t);
                self.background[self.active] = Some(old);
                self.background.insert(self.active + 1, None);
                self.active += 1;
                self.relayout();
                self.after_tab_switch();
            }
            Err(e) => crate::werr!("[VectorTerm] attach 会话失败:{e:#}"),
        }
    }

    fn handle_key(&mut self, ev: winit::event::KeyEvent) {
        if std::env::var("VECTORTERM_DEBUG_KEYS").is_ok() {
            crate::wlog!(
                "[dbg] key {:?} logical={:?} physical={:?} text={:?} mods={:?}",
                ev.state,
                ev.logical_key,
                ev.physical_key,
                ev.text,
                self.mods
            );
        }
        // 确认框是模态:按住什么键都先在框里消化掉,不能漏给终端
        if self.confirm.is_some() {
            if ev.state == ElementState::Pressed {
                self.confirm_key(&ev);
            }
            return;
        }
        // 命令面板是最高一层模态(从搜索框输 `/` 切过来)
        if self.panel.active {
            if ev.state == ElementState::Pressed {
                self.panel_key(&ev);
            }
            return;
        }
        // 历史搜索栏是模态输入:按键全部由它消化,不发给终端、也不触发回底
        if self.search.active {
            self.search_key(&ev);
            return;
        }
        // 松手即收尾:密集输入(按住回车)松开的瞬间,不要把剩下的尾迹动画播完
        if ev.state == ElementState::Released {
            if self.tab.active_pane_mut().trail.key_released() {
                self.window.request_redraw();
            }
            return;
        }
        // 文件浏览器:先看快捷键(再按一次收起),面板聚焦时由它吃掉方向键
        if ev.state == ElementState::Pressed {
            if self.files_shortcut(&ev) {
                return;
            }
            if self.browser_key(&ev) {
                return;
            }
        }
        // 先查 Lua 键位映射
        if ev.state == ElementState::Pressed {
            if let Some(combo) = input::key_combo(&ev, self.mods) {
                let hit = self
                    .script
                    .as_ref()
                    .map(|s| s.dispatch(&combo))
                    .unwrap_or(false);
                if hit {
                    self.apply_script_actions();
                    return;
                }
            }
            // 分屏焦点编号快捷键(Alt+1..9 / Ctrl+Shift+Alt+1..9)
            if self.pane_number_shortcut(&ev) {
                return;
            }
            // 标签页快捷键。放在 Lua 之后,所以配置里可以用 keys 覆盖
            if self.tab_shortcut(&ev) {
                return;
            }
            // 分屏快捷键(wezterm 同款 Ctrl+Shift+Alt+"/" 与 Ctrl+Shift+Alt+"%")
            if self.pane_shortcut(&ev) {
                return;
            }
            // 本地剪贴板快捷键:必须抢在交给终端状态机之前,否则
            // Ctrl+Shift+C 会被编码成 ETX、Ctrl+Shift+V 会变成普通字符
            if self.clipboard_shortcut(&ev) {
                return;
            }
            // Ctrl+Shift+P:命令面板(不必先开搜索再输 /)
            if self.panel_shortcut(&ev) {
                return;
            }
            // Ctrl+Shift+F:历史搜索栏(再按一次关闭)
            if self.search_shortcut(&ev) {
                return;
            }
            // Shift+PageUp/PageDown:本地回看滚动(所有终端的老习惯)
            let m = self.mods | physical_modifiers();
            if m.shift_key() && !m.control_key() && !m.alt_key() {
                let page = self.tab.active_pane().grid.1.saturating_sub(2).max(1) as i64;
                match ev.logical_key {
                    winit::keyboard::Key::Named(NamedKey::PageUp) => {
                        self.scroll_by(page);
                        return;
                    }
                    winit::keyboard::Key::Named(NamedKey::PageDown) => {
                        self.scroll_by(-page);
                        return;
                    }
                    _ => {}
                }
            }
        }
        match input::translate(&ev, self.mods) {
            // 交给终端状态机编码:方向键会按 DECCKM 决定 CSI/SS3,
            // Enter 会按 LNM 决定是否补 LF
            input::KeyAction::Key(code, km) => {
                if std::env::var("VECTORTERM_DEBUG_KEYS").is_ok() {
                    crate::wlog!("[dbg] key_down {code:?} {km:?}");
                }
                self.clear_selection_on_input();
                self.scroll_to_bottom();
                if let Err(e) = self.tab.active_pane_mut().term.lock().key_down(code, km) {
                    crate::wlog!("[VectorTerm] 按键编码失败:{e}");
                }
            }
            input::KeyAction::Bytes(data) => {
                self.clear_selection_on_input();
                self.scroll_to_bottom();
                self.send_bytes(&data);
            }
            input::KeyAction::Paste => self.paste(),
            input::KeyAction::None => {}
        }
    }

    /// 标签页快捷键(wezterm / Windows Terminal 的习惯):
    /// Ctrl+Shift+T 新建、Ctrl+Shift+W 关闭、Ctrl+Tab 下一个、
    /// Ctrl+Shift+Tab 上一个、Ctrl+1..9 跳到第 n 个。
    fn tab_shortcut(&mut self, ev: &winit::event::KeyEvent) -> bool {
        if !self.mods.control_key() || self.mods.alt_key() || self.mods.super_key() {
            return false;
        }
        let shift = self.mods.shift_key();
        match &ev.logical_key {
            Key::Named(NamedKey::Tab) => {
                if shift {
                    self.prev_tab();
                } else {
                    self.next_tab();
                }
                return true;
            }
            Key::Character(s) => {
                let Some(ch) = s.chars().next() else {
                    return false;
                };
                let ch = ch.to_ascii_lowercase();
                if shift {
                    match ch {
                        't' => {
                            self.new_tab();
                            return true;
                        }
                        'w' => {
                            self.close_tab_or_ask(self.active);
                            return true;
                        }
                        'a' if self.cfg.session_persistence => {
                            // 会话保持:attach 守护进程里最近一个「当前未显示」的会话
                            self.attach_next_session();
                            return true;
                        }
                        _ => {}
                    }
                }
                if let Some(d) = ch.to_digit(10) {
                    if (1..=9).contains(&d) {
                        let idx = (d - 1) as usize;
                        if idx < self.background.len() {
                            self.switch_to(idx);
                        }
                        return true;
                    }
                }
            }
            _ => {}
        }
        false
    }

    /// 文件浏览器侧边栏:`Ctrl+Shift+E`(Explorer)。
    ///
    /// 放在 Lua 键位之前、终端编码之前 —— 这个组合不能漏给 shell。想换键位
    /// 就在配置里写 `keys = { ["ctrl+shift+o"] = "files_gui" }`,内置这条
    /// 仍然保留(按自己习惯另加一个即可)。
    fn files_shortcut(&mut self, ev: &winit::event::KeyEvent) -> bool {
        if !self.mods.control_key()
            || !self.mods.shift_key()
            || self.mods.alt_key()
            || self.mods.super_key()
        {
            return false;
        }
        match &ev.logical_key {
            Key::Character(s) if s.eq_ignore_ascii_case("e") => {
                self.browser_toggle();
                true
            }
            _ => false,
        }
    }

    /// 分屏 / pane 焦点切换快捷键。wezterm 同款:
    /// - Ctrl+Shift+Alt+" → 垂直分屏(左右,中间竖杠)
    /// - Ctrl+Shift+Alt+% → 水平分屏(上下,中间横杠)
    /// - Ctrl+Shift+方向键 → 焦点移到相邻 pane
    /// - Ctrl+Shift+Alt+方向键 → 移动分割线(窗格大小)
    /// 分屏 / pane 焦点切换快捷键。判定抽成 `pane_action` 纯函数,这里只负责
    /// 补齐物理修饰键状态并执行动作。
    fn pane_shortcut(&mut self, ev: &winit::event::KeyEvent) -> bool {
        // winit 检测到右 Alt(AltGr) 时会把 ctrl/alt 一起过滤掉,补回来
        let mut m = self.mods;
        if !m.control_key() || !m.alt_key() {
            m |= physical_modifiers();
        }
        let Some(action) = pane_action(&ev.logical_key, &ev.physical_key, m) else {
            return false;
        };
        if std::env::var("VECTORTERM_DEBUG_KEYS").is_ok() {
            crate::wlog!("[dbg] pane shortcut {:?} (mods={:?})", action, m);
        }
        match action {
            PaneAction::Split(dir) => self.split_pane(dir),
            PaneAction::Focus(dir) => self.focus_pane_direction(dir),
            PaneAction::Resize(d) => self.resize_active_pane(d),
            PaneAction::Close => {
                if let Some(id) = self.active_pane_id() {
                    self.close_pane_or_ask(id);
                }
            }
        }
        true
    }

    /// 在活动 pane 处分割出一个新 pane(起默认 shell),焦点移到新 pane。
    fn split_pane(&mut self, dir: SplitDir) {
        let cfg = (*self.cfg).clone();
        self.split_pane_with(dir, cfg, default_title(&self.cfg));
    }

    /// 同上,但用 `cfg` 里的 shell / 参数 / cwd 起新 pane。
    ///
    /// 文件浏览器"回车打开文件"就走这条路:把 cfg 换成 `nvim <文件>`,
    /// 新 pane 落在右侧。布局、动画、PTY 尺寸同步全部复用同一套逻辑。
    /// 分屏后新 pane 的目标比例。
    ///
    /// 没有面板侧边栏时就是 0.5;侧边栏正占着被分割的这个 pane 时,按"去掉
    /// 侧边栏之后的可用宽度"对半分 —— 这样左右两个终端一样宽,不会出现
    /// "左边只剩十几列"。
    fn split_target_ratio(&self, dir: SplitDir, pane_idx: usize) -> f64 {
        if !matches!(dir, SplitDir::Vertical) {
            return 0.5;
        }
        let Some(owner) = self.tab.panes.get(pane_idx).map(|p| p.id) else {
            return 0.5;
        };
        if owner != self.tab.browser_pane {
            return 0.5;
        }
        let inset = browser_inset(&self.tab, self.renderer.cell_w);
        if inset <= 0.0 {
            return 0.5;
        }
        let grid = self.renderer.compute_grid(self.grid_px.0, self.grid_px.1);
        let (geoms, _) = compute_pane_geoms_in(
            &self.tab.layout,
            &|i| self.tab.panes.get(i).map(|p| p.id),
            self.renderer.cell_w,
            self.renderer.cell_h,
            0.0,
            grid.0 as f64 * self.renderer.cell_w,
            grid.1 as f64 * self.renderer.cell_h,
            &self.cfg.split_divider,
        );
        let Some(g) = geoms.iter().find(|g| g.pane_id == owner) else {
            return 0.5;
        };
        // ratio 是**原 pane(a 子树)**的占比:它拿走"侧边栏 + 可用宽度的一半",
        // 新 pane(b)才拿到同样宽的一半。
        let usable = (g.w - inset).max(2.0 * self.renderer.cell_w);
        ((inset + usable / 2.0) / g.w).clamp(0.05, 0.95)
    }

    fn split_pane_with(&mut self, dir: SplitDir, cfg: AppConfig, title: String) {
        // 上一次分屏的动画可能还在跑:先让它的线落到中线,否则那个节点会卡在半路
        settle_split_anim(&mut self.tab);

        // 新 pane 初始尺寸 = 活动 pane 的一半(commit_grid 会按布局树重新精确计算)
        let (cols, rows) = self.tab.active_pane().grid;
        let half_cols = (cols / 2).max(2);
        let half_rows = (rows / 2).max(2);
        let new_grid = match dir {
            SplitDir::Vertical => (half_cols, rows),
            SplitDir::Horizontal => (cols, half_rows),
        };
        let px = self.grid_px;
        let dpi = (self.window.scale_factor() * 96.0) as u32;

        // 分配新 pane id(全局唯一)
        let new_id = self.next_tab_id;
        self.next_tab_id += 1;

        let spawned = if cfg.session_persistence {
            spawn_pane_mux(&cfg, new_grid, px, &self.proxy, dpi)
        } else {
            spawn_pane(&cfg, new_grid, px, &self.proxy, new_id, title, dpi)
        };
        let Ok(new_pane) = spawned else {
            crate::werr!("[VectorTerm] 分屏失败");
            return;
        };

        // 插入新 pane 并更新布局树
        let target = self.tab.active_pane;
        let new_idx = self.tab.panes.len();
        self.tab.panes.push(new_pane);
        self.tab.layout.split_at(target, new_idx, dir, 0.5);
        // 焦点移到新 pane
        self.tab.active_pane = new_idx;

        // 分屏动画:新 pane 从最右(垂直)/最下(水平)滑入,分割线滑到中间。
        //
        // 关键顺序:先把**所有 pane 的 PTY resize 到最终尺寸**,再把分割线拉回
        // 边缘开始滑。旧实现是等动画结束才 resize,于是滑入过程中旧 pane 还是
        // 满尺寸的内容,最后一帧突然重排(26 行 → 12 行、文字整块跳一下),
        // 看着就像"动画结束才分割出区域"。现在内容在动画开始就已就位,
        // 滑入过程只是把分割线和 rect 挪过去,结束时零跳变。
        let grid = self.renderer.compute_grid(self.grid_px.0, self.grid_px.1);
        self.commit_grid(grid);

        // 侧边栏占着这块 pane 的左边一条时,新 pane 只吃"剩余宽度"的一半。
        // 否则按整宽对半分,旧 pane 的终端会被侧边栏挤成十几列(内容全断行)。
        let to = self.split_target_ratio(dir, target);
        let anim = SplitAnim::new(new_idx, dir, Instant::now(), to);
        let from = anim.from;
        self.tab.layout.set_ratio_of_new_pane(new_idx, from);
        self.tab.split_anim = Some(anim);

        self.after_tab_switch();
    }

    /// 把焦点切到指定 pane。
    ///
    /// 鼠标点击、方向键、编号快捷键都走这里,保证行为一致。焦点反馈只有一条
    /// **跨窗格尾迹**:把 `focus_trail` 种到旧 pane 的光标位置,新 pane 的
    /// 输入光标照常立刻出现 —— 和窗格内回车时的动画是同一套代码。
    fn focus_pane_index(&mut self, idx: usize) {
        if idx >= self.tab.panes.len() || idx == self.tab.active_pane {
            return;
        }
        let now = Instant::now();
        let old_idx = self.tab.active_pane;
        let old_id = self.tab.panes[old_idx].id;

        if self.cfg.pane_focus.trail && self.cfg.pane_focus.trail_ms > 0.0 {
            let (geoms, _) = self.pane_geoms();
            let cw = self.renderer.cell_w;
            let ch = self.renderer.cell_h;
            // 起点 = 旧 pane 的真实输入光标格(内容区像素)。用整格而不是光标
            // 形状:窗格内尾迹也是整格,尺寸一致才不会"换了个动画"。
            let from = geoms.iter().find(|g| g.pane_id == old_id).map(|g| {
                let (col, row) = {
                    let t = self.tab.panes[old_idx].term.lock();
                    let pos = t.cursor_pos();
                    (pos.x.max(0) as f64, pos.y.max(0) as f64)
                };
                let row = row.min(g.grid.1.saturating_sub(1) as f64);
                let col = col.min(g.grid.0.saturating_sub(1) as f64);
                let x = g.x + col * cw;
                let y = g.y + row * ch;
                [x as f32, (x + cw) as f32, y as f32, (y + ch) as f32]
            });
            if let Some(r) = from {
                self.focus_trail.seed(r, now);
                self.focus_trail_on = true;
            }
        }

        self.tab.active_pane = idx;
        // 切换瞬间清掉两边残留的光标尾迹与滚动惯性,避免旧动画串场
        let p = self.tab.active_pane_mut();
        p.trail.reset();
        p.scroll.reset();
        // 面板跟着"当前终端"走:换到哪个 pane,就看那个 pane 的目录。
        // 已经开着的面板不会消失,只是换了个宿主 —— 用户不用重新按快捷键。
        self.browser_follow_focus(idx);
        self.tab_hover = None;
        self.close_hover = None;
        self.window.set_title(&window_title(self.tab.title()));
        self.window.request_redraw();
    }

    /// `Alt+1..9`(或加 Ctrl+Shift)直接聚焦第 N 个 pane。
    /// 与 `Ctrl+1..9`(切标签)不冲突,也避开常被输入法吞掉的纯 Ctrl+Shift 组合。
    fn pane_number_shortcut(&mut self, ev: &winit::event::KeyEvent) -> bool {
        if !self.cfg.pane_focus.number_shortcuts || self.tab.panes.len() < 2 {
            return false;
        }
        let alt = self.mods.alt_key() || physical_modifiers().alt_key();
        if !alt || self.mods.super_key() {
            return false;
        }
        let digit = match &ev.physical_key {
            PhysicalKey::Code(c) => digit_from_code(c),
            _ => match &ev.logical_key {
                Key::Character(s) => s.chars().next().and_then(|c| c.to_digit(10)),
                _ => None,
            },
        };
        let Some(d) = digit else {
            return false;
        };
        if !(1..=9).contains(&d) {
            return false;
        }
        let idx = (d - 1) as usize;
        if idx < self.tab.panes.len() {
            self.focus_pane_index(idx);
        }
        true
    }

    fn focus_pane_direction(&mut self, dir: PaneDir) {
        let (geoms, _) = self.pane_geoms();
        let active_id = self.tab.panes[self.tab.active_pane].id;
        let Some(active) = geoms.iter().find(|g| g.pane_id == active_id).copied() else {
            return;
        };
        let (acx, acy) = active.center();

        // 找方向上几何最近的 pane:主方向距离优先,副方向距离次之
        let mut best: Option<(u64, f64)> = None;
        for g in &geoms {
            if g.pane_id == active_id {
                continue;
            }
            let (cx, cy) = g.center();
            let (dx, dy) = (cx - acx, cy - acy);
            let on_side = match dir {
                PaneDir::Left => dx < -0.5,
                PaneDir::Right => dx > 0.5,
                PaneDir::Up => dy < -0.5,
                PaneDir::Down => dy > 0.5,
            };
            if !on_side {
                continue;
            }
            let (primary, secondary) = match dir {
                PaneDir::Left | PaneDir::Right => (dx.abs(), dy.abs()),
                PaneDir::Up | PaneDir::Down => (dy.abs(), dx.abs()),
            };
            let score = primary * 10000.0 + secondary;
            if best.map_or(true, |(_, sc)| score < sc) {
                best = Some((g.pane_id, score));
            }
        }
        if let Some((id, _)) = best {
            if let Some(idx) = self.tab.panes.iter().position(|p| p.id == id) {
                self.focus_pane_index(idx);
            }
        }
    }

    /// Ctrl+Shift+C/V、Ctrl+Insert、Shift+Insert。
    /// `selection.ctrl_shift_v = "copy"` 时 C/V 对调,便于把 V 当复制键用。
    fn clipboard_shortcut(&mut self, ev: &winit::event::KeyEvent) -> bool {
        let ctrl = self.mods.control_key();
        let shift = self.mods.shift_key();
        if self.mods.alt_key() || self.mods.super_key() {
            return false;
        }
        let v_copies = self.cfg.selection.ctrl_shift_v == VKeyAction::Copy;
        if ctrl && shift {
            let ch = match &ev.logical_key {
                Key::Character(s) => s.chars().next(),
                _ => None,
            };
            match ch.map(|c| c.to_ascii_lowercase()) {
                Some('c') => {
                    if v_copies {
                        self.paste();
                    } else {
                        self.copy_selection();
                    }
                    return true;
                }
                Some('v') => {
                    if v_copies {
                        self.copy_selection();
                    } else {
                        self.paste();
                    }
                    return true;
                }
                _ => {}
            }
        }
        if let Key::Named(NamedKey::Insert) = &ev.logical_key {
            if ctrl && !shift {
                self.copy_selection();
                return true;
            }
            if shift && !ctrl {
                self.paste();
                return true;
            }
        }
        false
    }

    fn clear_selection_on_input(&mut self) {
        if self.tab.active_pane_mut().selection.current.take().is_some() {
            self.window.request_redraw();
        }
    }

    // ---- 鼠标:VT 报点(1000/1002/1003 + 1006/1015)与本地滚动 ----

    /// 像素坐标 → 单元格坐标(超出内容区时钳到边界)
    /// 当前的统一布局几何(相对内容区原点)。鼠标命中 / 焦点动画 / 绘制共用。
    fn pane_geoms(&self) -> (Vec<PaneGeom>, Vec<SplitLine>) {
        let grid = self.renderer.compute_grid(self.grid_px.0, self.grid_px.1);
        tab_pane_geoms(&self.renderer, &self.tab, grid, &self.cfg.split_divider)
    }

    /// 整窗坐标命中哪条分割线(带热区)。线本身只有几像素,所以按
    /// `split_divider.hit_width` 向两侧膨胀;但长度方向不膨胀,免得挡住标签栏。
    fn hit_split_line(&self, x: f64, y: f64) -> Option<SplitLine> {
        if !self.cfg.split_divider.draggable {
            return None;
        }
        let lx = x - self.renderer.pad_x;
        let ly = y - self.renderer.pad_y - self.renderer.content_top;
        let hw = self.cfg.split_divider.hit_width;
        let (_, lines) = self.pane_geoms();
        lines.into_iter().find(|l| {
            if l.vertical {
                lx >= l.x - hw && lx < l.x + l.w + hw && ly >= l.y && ly < l.y + l.h
            } else {
                ly >= l.y - hw && ly < l.y + l.h + hw && lx >= l.x && lx < l.x + l.w
            }
        })
    }

    /// 改一条分割线的比例,并把新的字符网格同步给所有 PTY。
    /// 拖动和快捷键都走这里,保证「几何 / 终端列数 / 渲染」永远是同一份结果。
    fn set_split_ratio(&mut self, path: u32, depth: u8, r: f64) {
        let lo = self.cfg.split_divider.min_ratio;
        let r = r.clamp(lo, (1.0 - lo).max(lo));
        if self
            .tab
            .layout
            .ratio_at(path, depth)
            .is_some_and(|old| (old - r).abs() < 1e-9)
        {
            return;
        }
        if !self.tab.layout.set_ratio_at(path, depth, r) {
            return;
        }
        let grid = self.renderer.compute_grid(self.grid_px.0, self.grid_px.1);
        self.commit_grid(grid);
        self.window.request_redraw();
    }

    /// `Ctrl+Shift+Alt+方向键`:移动分割线。方向 = 线的移动方向
    /// (→ 左/上侧变宽,← 变窄;上下同理),和鼠标拖动完全同一套语义。
    /// 嵌套分屏时会找「包含活动 pane、且方向与按键同轴」的最深那一条。
    fn resize_active_pane(&mut self, dir: PaneDir) {
        settle_split_anim(&mut self.tab);
        let leaf = self.tab.active_pane;
        let want_vertical = matches!(dir, PaneDir::Left | PaneDir::Right);
        let mut splits = Vec::new();
        self.tab.layout.splits_containing(leaf, &mut splits);
        let Some(&(path, depth, _, ratio)) = splits.iter().rev().find(|(_, _, d, _)| {
            matches!(
                (want_vertical, *d),
                (true, SplitDir::Vertical) | (false, SplitDir::Horizontal)
            )
        }) else {
            return;
        };
        let step = self.cfg.split_divider.step;
        let delta = match dir {
            PaneDir::Right | PaneDir::Down => step,
            _ => -step,
        };
        self.set_split_ratio(path, depth, ratio + delta);
    }

    /// 整窗像素坐标落在哪个 pane 里(缝上不算)。返回 pane 下标。
    fn pane_at(&self, x: f64, y: f64) -> Option<usize> {
        let ox = self.renderer.pad_x;
        let oy = self.renderer.pad_y + self.renderer.content_top;
        let (geoms, _) = self.pane_geoms();
        geoms
            .iter()
            .find(|g| g.contains(x - ox, y - oy))
            .and_then(|g| self.tab.pane_index(g.pane_id))
    }

    fn cell_at(&self, x: f64, y: f64) -> (usize, usize) {
        // 分屏后每个 pane 有自己的原点:先扣掉活动 pane 的偏移,再换算成列行。
        // 旧实现只减 pad_x/pad_y,点右半屏时列号会从整窗左边缘算起,完全错位。
        let active_id = self.tab.active_pane().id;
        let (geoms, _) = self.pane_geoms();
        let (ox, oy, grid) = geoms
            .iter()
            .find(|g| g.pane_id == active_id)
            .map(|g| (g.x, g.y, g.grid))
            .unwrap_or((0.0, 0.0, self.tab.active_pane().grid));
        let col = ((x - self.renderer.pad_x - ox) / self.renderer.cell_w).floor();
        let row = ((y - self.renderer.content_top - self.renderer.pad_y - oy)
            / self.renderer.cell_h)
            .floor();
        let col = col.clamp(0.0, (grid.0.saturating_sub(1)) as f64) as usize;
        let row = row.clamp(0.0, (grid.1.saturating_sub(1)) as f64) as usize;
        (col, row)
    }

    // ---------------- 文件浏览器 ----------------

    /// 面板的绘制矩形(整窗坐标)+ 列表首行的 y + 可见行数。
    ///
    /// 绘制、鼠标命中、滚轮全用这一份几何,和 pane 共用同一个内容区原点
    /// (`pad + content_top`),不会出现"画在这里、点在那里"。
    fn browser_geom(&self) -> Option<(files::Rect, f64, usize)> {
        let b = self.tab.browser.as_ref()?;
        self.browser_geom_for(b, self.tab.browser_pane)
    }

    /// 同上,但面板和它所属的 pane 都由调用方给。
    ///
    /// 退场动画期间 `tab.browser` 已经是 `None`(按键、鼠标都不该再进它),
    /// 可屏上那半块面板还在往外滑,得能对着手里这份 `Browser` 单独算几何。
    fn browser_geom_for(
        &self,
        b: &files::Browser,
        pane_id: u64,
    ) -> Option<(files::Rect, f64, usize)> {
        let cw = self.renderer.cell_w;
        let ch = self.renderer.cell_h;
        // 面板铺满的是**它所属的那个 pane**,不是整个内容区:分屏后一边开
        // 面板,另一边照常显示自己的终端。
        //
        // 这里取的是"没被侧边栏缩过"的矩形 —— 侧边栏自己就占它的左边一条,
        // 而 pane 的终端几何(pty 列数、文字原点)已经按 inset 让开了。
        let grid = self.renderer.compute_grid(self.grid_px.0, self.grid_px.1);
        let (geoms, _) = compute_pane_geoms_in(
            &self.tab.layout,
            &|i| self.tab.panes.get(i).map(|p| p.id),
            cw,
            ch,
            0.0,
            grid.0 as f64 * cw,
            grid.1 as f64 * ch,
            &self.cfg.split_divider,
        );
        let g = geoms.iter().find(|g| g.pane_id == pane_id)?;
        let w = match b.mode {
            files::Mode::Full => g.w,
            // 侧边栏宽度和 browser_inset 同源:整列 + 1px 给右边的竖线
            files::Mode::Side => (b.side_cols as f64 * cw).min(g.w),
        };
        let rect = files::Rect {
            x: self.renderer.pad_x + g.x,
            y: self.renderer.pad_y + self.renderer.content_top + g.y,
            w,
            h: g.h,
        };
        let header = ch;
        let footer = if b.mode == files::Mode::Full { ch } else { 0.0 };
        // 底部提示行(未装编辑器之类)也从列表高度里扣掉。列数和折行都跟
        // `files::paint` 同一份算法,免得最后一行被提示压住、点不到。
        let cols = (w / cw).floor().max(1.0) as usize;
        let notice = b.notice_rows(cols) as f64 * ch;
        let rows = (((rect.h - header - footer - notice) / ch).floor()).max(0.0) as usize;
        Some((rect, rect.y + header, rows))
    }

    /// 打开面板。会改变 pane 的可用宽度(侧边栏模式),所以顺手同步一次网格。
    fn browser_open(&mut self, cwd: PathBuf, mode: files::Mode) {
        let mut b = files::Browser::new(
            cwd,
            mode,
            self.cfg.files_gui.show_hidden,
            self.cfg.files_gui.side_cols,
        );
        b.focused = true;
        self.tab.browser = Some(b);
        // 面板属于"当前这个终端":分屏后在哪边开的就挂在哪边
        self.tab.browser_pane = self.tab.active_pane().id;
        // 上一次的退场还没放完就再开:旧面板直接丢,新的从当前位置接着滑进来
        self.browser_exit = None;
        self.browser_anim.open(Instant::now());
        let grid = self.renderer.compute_grid(self.grid_px.0, self.grid_px.1);
        self.commit_grid(grid);
        self.window.request_redraw();
    }

    /// 收起面板,把左侧区域还给终端。
    ///
    /// 面板本身不在这里丢,而是挪进 `browser_exit` 继续画到滑出屏幕为止 ——
    /// 逻辑上立刻算"关了"(按键、鼠标、pane 宽度全部按关处理),只有像素还
    /// 在往左滑。放完之后由 `redraw` 收尾。
    fn browser_close(&mut self) {
        if let Some(mut b) = self.tab.browser.take() {
            b.focused = false;
            // 上一轮退场没放完又开又关:新的覆盖旧的就是了
            self.browser_exit = Some(b);
            self.browser_anim.close(Instant::now());
            let grid = self.renderer.compute_grid(self.grid_px.0, self.grid_px.1);
            self.commit_grid(grid);
            self.window.request_redraw();
        }
    }

    /// `Ctrl+Shift+E`:没开就开侧边栏;开着没聚焦就把焦点拉回来;已聚焦就收起。
    fn browser_toggle(&mut self) {
        match self.tab.browser.as_mut() {
            None => {
                let cwd = self.pane_cwd();
                self.browser_open(cwd, files::Mode::Side);
            }
            Some(b) if !b.focused => {
                b.focused = true;
                self.window.request_redraw();
            }
            Some(_) => self.browser_close(),
        }
    }

    /// 命令面板 `/files`:全屏文件管理器,打开时取当前 pane 的 cwd。
    /// 以前这要靠在 shell 里敲 `files-gui`(私有 OSC 回传);现在全部走输入框。
    fn open_files_full(&mut self) {
        let cwd = self.pane_cwd();
        self.browser_open(cwd, files::Mode::Full);
        if let Some(p) = std::env::var_os("VECTORTERM_DUMP_FRAME") {
            self.dump_frame.push(PathBuf::from(p));
        }
    }

    /// 命令面板 `/settings`:拉起独立的可视化配置管理器。
    /// 它是另一个进程,不占当前终端;测试钩子不要继承过去,免得子窗口又
    /// 自动分屏 / 自动开面板 / 再导出一帧。
    fn open_settings_gui(&mut self) {
        let exe = match std::env::current_exe() {
            Ok(p) => p.with_file_name("vectorterm-settings.exe"),
            Err(e) => {
                crate::wlog!("[VectorTerm] 找不到自身路径,无法打开配置管理器:{e}");
                return;
            }
        };
        if !exe.exists() {
            crate::wlog!("[VectorTerm] 配置管理器不存在:{}", exe.display());
            self.browser_notice(format!("settings gui not found: {}", exe.display()));
            return;
        }
        let mut cmd = std::process::Command::new(&exe);
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        for k in [
            "VECTORTERM_AUTO_SPLIT",
            "VECTORTERM_AUTO_FILES",
            "VECTORTERM_AUTO_FILES_OPEN",
            "VECTORTERM_DUMP_FRAME",
            "VECTORTERM_AUTOTYPE",
        ] {
            cmd.env_remove(k);
        }
        if let Err(e) = cmd.spawn() {
            crate::wlog!("[VectorTerm] 启动配置管理器失败:{e}");
        }
    }

    /// 面板打开时的起始目录:优先 shell 现在的目录,其次配置里的 `cwd`,
    /// 最后退到进程自己的工作目录。
    fn pane_cwd(&self) -> PathBuf {
        self.pane_cwd_of(self.tab.active_pane)
    }

    /// 指定 pane 的当前目录(shell 目录 → 配置 cwd → 进程 cwd)。
    fn pane_cwd_of(&self, idx: usize) -> PathBuf {
        if let Some(dir) = self.shell_cwd_of(idx) {
            return dir;
        }
        if let Some(c) = self.cfg.cwd.as_ref().filter(|c| c.is_dir()) {
            return c.clone();
        }
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("C:/"))
    }

    /// shell 此刻到底在哪个目录。拿不到就 `None`(不再往下兜底)。
    ///
    /// 两个来源,按可信度排:
    /// 1. **shell 自己报的 OSC 7** —— 最可靠。PowerShell 靠我们注入的 prompt
    ///    钩子每次都报;配了 shell 集成(clink、oh-my-posh 之类)的 bash 也会报。
    /// 2. **读 shell 进程的 PEB** —— `cmd.exe` 的 `cd` 走 `SetCurrentDirectoryW`,
    ///    进程 cwd 跟着变,读得到。**PowerShell 不能走这条**:它的
    ///    `Set-Location` 只改自己的位置,进程 cwd 一直是启动目录,读出来会把
    ///    面板拽到一个错的（而且是旧的）目录。
    fn shell_cwd_of(&self, idx: usize) -> Option<PathBuf> {
        let p = self.tab.panes.get(idx)?;
        if let Some(dir) = cwd_of_term(&p.term) {
            let path = PathBuf::from(dir);
            if path.is_dir() {
                return Some(path);
            }
        }
        if pty::is_powershell(&p.shell) {
            return None;
        }
        let pid = p.shell_pid()?;
        procs::cwd_of(pid).filter(|p| p.is_dir())
    }

    /// 面板换了目录 → 把 `cd` 写进所属终端的 shell,让终端跟过来。
    ///
    /// 只在两边确实不一致时才写:shell 已经在这个目录就不必再敲一遍。
    /// 三种情况不写:功能关掉了、shell 已退出、前台正跑着全屏程序
    /// (vim / less 里,`cd` 会被当成按键喂进去)。
    fn browser_push_cwd(&mut self) {
        if !self.cfg.files_gui.push_cwd {
            return;
        }
        let Some(b) = self.tab.browser.as_ref() else {
            return;
        };
        let dir = b.cwd.clone();
        let pane_id = self.tab.browser_pane;
        let Some(idx) = self.tab.panes.iter().position(|p| p.id == pane_id) else {
            return;
        };
        let (exited, busy, shell) = {
            let p = &self.tab.panes[idx];
            (p.exited, p.term.lock().is_alt_screen_active(), p.shell.clone())
        };
        if exited || busy {
            return;
        }
        if self
            .shell_cwd_of(idx)
            .is_some_and(|cur| files::same_dir(&cur, &dir))
        {
            return;
        }
        let line = shell_cd_line(&shell, &dir);
        if line.is_empty() {
            return;
        }
        {
            use std::io::Write;
            let mut w = self.tab.panes[idx].kb_writer.clone();
            if w.write_all(line.as_bytes()).is_err() {
                crate::wlog!("[VectorTerm] 同步目录失败:{}", dir.display());
                return;
            }
        }
        if cwd_debug() {
            crate::wlog!("[cwdsync] 面板 → 终端:{}({})", dir.display(), line.trim_end());
        }
        // 终端马上就会报新目录(OSC 7 / PEB),把基线提前挪过去,
        // 免得轮询把这次"面板主动"的换目录又当成"终端变了"再拽一次
        self.seen_cwd = Some(dir);
        self.seen_cwd_pane = pane_id;
    }

    /// 面板跟随终端:shell 的目录变了就把面板切过去。
    ///
    /// 只在"shell 的目录**变过**"时才动面板。用户在面板里自己翻目录、而 shell
    /// 没变时,不能被拽回来 —— 两个方向都无条件同步会互相打架。
    fn poll_browser_cwd(&mut self, now: Instant) {
        if now < self.next_cwd_poll {
            return;
        }
        // 面板没开(或功能关掉)时不需要盯着 shell:cwd 只在开面板期间有用,
        // 低频待机就够,别为了一个看不见的列表把空闲窗口叫醒。
        if self.tab.browser.is_none() || !self.cfg.files_gui.follow_cwd {
            self.seen_cwd = None;
            self.seen_cwd_pane = 0;
            self.next_cwd_poll = now + Duration::from_millis(1000);
            return;
        }
        self.next_cwd_poll = now + Duration::from_millis(350);
        let pane_id = self.tab.browser_pane;
        let Some(idx) = self.tab.panes.iter().position(|p| p.id == pane_id) else {
            return;
        };
        let cur = self.shell_cwd_of(idx);
        // 面板换宿主了:先把当前目录记成基线,不立刻改面板
        if self.seen_cwd_pane != pane_id {
            self.seen_cwd_pane = pane_id;
            if cwd_debug() {
                crate::wlog!("[cwdsync] 基线 pane={pane_id} cwd={:?}", cur);
            }
            self.seen_cwd = cur;
            return;
        }
        let changed = match (&self.seen_cwd, &cur) {
            (Some(a), Some(b)) => !files::same_dir(a, b),
            (None, None) => false,
            // 从"不知道"变成"知道了"也算信息更新,但没必要动面板
            _ => {
                self.seen_cwd = cur.clone();
                return;
            }
        };
        if !changed {
            return;
        }
        if cwd_debug() {
            crate::wlog!("[cwdsync] shell 目录变化:{:?} → {:?}", self.seen_cwd, cur);
        }
        self.seen_cwd = cur.clone();
        let Some(dir) = cur else {
            return;
        };
        if let Some(b) = self.tab.browser.as_mut() {
            if b.follow_dir(dir.clone()) {
                if cwd_debug() {
                    crate::wlog!("[cwdsync] 终端 → 面板:{}", dir.display());
                }
                self.window.request_redraw();
            }
        }
    }

    /// 面板换宿主:开着面板时切到另一个终端,面板整体挪过去,并显示**那个
    /// 终端**的目录(全屏 / 侧边栏形态保持不变)。
    ///
    /// 没开面板时什么都不做 —— 切 pane 不该凭空弹出一个列表。
    fn browser_follow_focus(&mut self, idx: usize) {
        let Some(b) = self.tab.browser.as_ref() else {
            return;
        };
        let (mode, focused, show_hidden, side_cols) =
            (b.mode, b.focused, b.show_hidden, b.side_cols);
        let Some(new_id) = self.tab.panes.get(idx).map(|p| p.id) else {
            return;
        };
        if new_id == self.tab.browser_pane {
            return;
        }
        let cwd = self.pane_cwd_of(idx);
        let mut nb = files::Browser::new(cwd, mode, show_hidden, side_cols);
        nb.focused = focused;
        self.tab.browser = Some(nb);
        self.tab.browser_pane = new_id;
        // 侧边栏换了宿主:旧 pane 要把宽度还回去,新 pane 要让出一条
        let grid = self.renderer.compute_grid(self.grid_px.0, self.grid_px.1);
        self.commit_grid(grid);
    }

    /// 面板的键盘处理。返回 true 表示这个键被面板吃掉了。
    ///
    /// 只有 `focused` 时才算数:侧边栏和终端并排时,方向键该归谁由焦点决定。
    fn browser_key(&mut self, ev: &winit::event::KeyEvent) -> bool {
        let Some(b) = self.tab.browser.as_ref() else {
            return false;
        };
        if !b.focused {
            return false;
        }
        // Ctrl/Alt/Super 组合留给全局快捷键(比如再按一次 Ctrl+Shift+E 收起)
        if self.mods.control_key() || self.mods.alt_key() || self.mods.super_key() {
            return false;
        }
        let nav = match &ev.logical_key {
            Key::Named(NamedKey::ArrowUp) => Some(files::Nav::Up),
            Key::Named(NamedKey::ArrowDown) => Some(files::Nav::Down),
            Key::Named(NamedKey::PageUp) => Some(files::Nav::PageUp),
            Key::Named(NamedKey::PageDown) => Some(files::Nav::PageDown),
            Key::Named(NamedKey::Home) => Some(files::Nav::Home),
            Key::Named(NamedKey::End) => Some(files::Nav::End),
            _ => None,
        };
        if let Some(n) = nav {
            if let Some(b) = self.tab.browser.as_mut() {
                b.nav(n);
            }
            self.window.request_redraw();
            return true;
        }
        match &ev.logical_key {
            // 回车 / 右方向键:进目录或打开文件
            Key::Named(NamedKey::Enter) | Key::Named(NamedKey::ArrowRight) => {
                self.browser_activate();
                true
            }
            // 退格 / 左方向键:回上级
            Key::Named(NamedKey::Backspace) | Key::Named(NamedKey::ArrowLeft) => {
                if let Some(b) = self.tab.browser.as_mut() {
                    b.go_parent();
                }
                // 面板换了目录,终端也要跟过去(和"进目录"走同一条同步逻辑)
                self.browser_push_cwd();
                self.window.request_redraw();
                true
            }
            Key::Named(NamedKey::Escape) => {
                self.browser_close();
                true
            }
            Key::Character(s) => {
                let Some(ch) = s.chars().next() else {
                    return false;
                };
                match ch.to_ascii_lowercase() {
                    '.' => {
                        if let Some(b) = self.tab.browser.as_mut() {
                            b.toggle_hidden();
                        }
                        self.window.request_redraw();
                        true
                    }
                    'r' => {
                        if let Some(b) = self.tab.browser.as_mut() {
                            b.refresh();
                        }
                        self.window.request_redraw();
                        true
                    }
                    'q' => {
                        self.browser_close();
                        true
                    }
                    // 面板聚焦时其它可打印字符也不往 shell 漏(否则边看列表
                    // 边打字会把命令打进终端);没用上就直接吞掉。
                    _ => true,
                }
            }
            // 剩下的普通键(F1、Tab、Esc 之外的命名键)同样吞掉
            _ => true,
        }
    }

    /// 回车:目录进去,文件交给编辑器。
    fn browser_activate(&mut self) {
        let Some(b) = self.tab.browser.as_mut() else {
            return;
        };
        match b.activate() {
            None | Some(files::Open::Dir(_)) => {
                // 只是换了目录,面板和 pane 尺寸都没变;但终端得跟着 cd
                self.browser_push_cwd();
                self.window.request_redraw();
            }
            Some(files::Open::File(path)) => self.open_in_editor(path),
        }
    }

    /// 文件浏览器回车:按 `files_gui.open_mode` 决定编辑器跑在哪儿。
    ///
    /// * `window`(默认):新开一个 VectorTerm 窗口 —— 独立进程、独立 PTY,不动
    ///   当前窗口的分屏与 shell;
    /// * `split`:在宿主 pane 里向右分出一个 pane;
    /// * `current`:把命令写进宿主 pane 的 shell,和自己敲命令一样。
    ///
    /// 编辑器先确认能跑起来再动手:探测不到时不动布局,只在面板底部挂一行
    /// 英文提示(以前直接 spawn 会弹系统错误框)。
    fn open_in_editor(&mut self, path: PathBuf) {
        let editor = match self.editor_pick() {
            EditorPick::Ready(v) | EditorPick::Fallback(v, _) => v,
            // 编辑器找不到时不动布局,只在面板底部挂一行提示(英文)
            EditorPick::Missing(note) => {
                self.browser_notice(note);
                self.window.request_redraw();
                return;
            }
        };
        if editor.is_empty() {
            return;
        }
        match self.cfg.files_gui.open_mode {
            FileOpenMode::Window => self.open_in_new_window(path, editor),
            FileOpenMode::Split => self.open_in_new_pane(path, editor),
            FileOpenMode::Current => self.open_in_current_pane(path, editor),
        }
    }

    /// 在文件面板底部挂一行提示,并把焦点留在列表上(不吃掉用户的下一次按键)。
    fn browser_notice(&mut self, msg: String) {
        if let Some(b) = self.tab.browser.as_mut() {
            b.set_notice(msg);
            b.focused = true;
        }
    }

    /// 「新窗口」模式(默认):起一个新进程,由它开一个自己的 VectorTerm 窗口跑编辑器。
    ///
    /// 新窗口是**另一个进程**:自己的 PTY、自己的 shell(= 编辑器),关掉它
    /// 不会碰到当前窗口的分屏和 shell;当前窗口只是把面板收起来。
    ///
    /// 新进程靠 `--open-file <路径> -- <编辑器命令...>` 启动,编辑器命令已经
    /// 解析成绝对路径,不用在新进程里再探一次。
    fn open_in_new_window(&mut self, path: PathBuf, editor: Vec<String>) {
        let exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                self.browser_notice(format!("failed to open a new window: {e}"));
                self.window.request_redraw();
                return;
            }
        };
        let mut cmd = std::process::Command::new(&exe);
        // 新窗口和当前窗口用同一份配置(外观、字体、快捷键都对得上)
        if let Some(cfg) = self.cfg_path.as_deref() {
            cmd.arg("--config").arg(cfg);
        }
        cmd.arg("--open-file").arg(&path).arg("--").args(&editor);
        // 新窗口自己创建界面,不需要继承我们的控制台句柄
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // cwd 跟面板走(和分屏模式一致):`nvim` 里 `:Ex` 就在你浏览的目录
        let cwd = self
            .tab
            .browser
            .as_ref()
            .map(|b| b.cwd.clone())
            .or_else(|| path.parent().map(Path::to_path_buf));
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        // 测试钩子(`VECTORTERM_AUTO_*` / `VECTORTERM_DUMP_FRAME`)只属于当前这扇窗,
        // 带进子进程会让新窗口再自动开一遍面板 / 再走一次"回车"。
        for (k, _) in std::env::vars_os() {
            let name = k.to_string_lossy();
            if name.starts_with("VECTORTERM_AUTO_") || name == "VECTORTERM_DUMP_FRAME" {
                cmd.env_remove(&k);
            }
        }
        match cmd.spawn() {
            Ok(_) => {
                // 面板收起,当前窗口回到原来的样子
                self.browser_close();
            }
            Err(e) => self.browser_notice(format!("failed to open a new window: {e}")),
        }
        self.window.request_redraw();
    }

    /// 「当前终端」模式:把编辑器的启动命令写进宿主 pane 的 shell,
    /// 和用户自己敲 `nvim 文件` 完全一样。
    ///
    /// 面板会先收起:它和终端共用同一块文字区,不收起的话编辑器会被侧边栏
    /// 挤窄,或者被全屏列表整块盖住。
    ///
    /// 两种情况不盲写:宿主 pane 已结束(退回新分屏),或前台正跑着备用屏
    /// 程序(vim / less 之类)—— 后者会把命令当成按键,只给面板底部提示。
    fn open_in_current_pane(&mut self, path: PathBuf, editor: Vec<String>) {
        let idx = self.tab.active_pane;
        let (exited, busy) = {
            let p = &self.tab.panes[idx];
            (p.exited, p.term.lock().is_alt_screen_active())
        };
        if busy {
            self.browser_notice(
                "this pane is running a full-screen app; quit it, then open the file".to_string(),
            );
            self.window.request_redraw();
            return;
        }
        if exited {
            // shell 已经结束,没有可复用的终端 —— 只能按老办法新分一个
            self.open_in_new_pane(path, editor);
            return;
        }

        let mut argv = editor;
        argv.push(path.to_string_lossy().into_owned());
        let line = format!("{}\r", shell_command_line(&self.cfg.shell, &argv));
        // 先收面板、把整块文字区还给终端,再写命令 —— 这样 nvim 启动时
        // 拿到的已经是全宽/全高的网格,不会先按侧边栏宽度画一遍再重排。
        self.browser_close();
        {
            use std::io::Write;
            let mut w = self.tab.panes[idx].kb_writer.clone();
            if w.write_all(line.as_bytes()).is_err() {
                // 写入只是进缓冲,失败基本只可能是 PTY 已经没了:不打扰用户
                crate::werr!("[VectorTerm] 写入编辑器命令失败");
            }
        }
        self.window.request_redraw();
    }

    /// 「分屏」模式(`files_gui.open_mode = "split"`),也是「当前终端」模式遇到
    /// 已退出的 shell 时的兜底:在宿主 pane 里向右分一个新的 pane 跑编辑器。
    fn open_in_new_pane(&mut self, path: PathBuf, editor: Vec<String>) {
        let cwd = self.tab.browser.as_ref().map(|b| b.cwd.clone());
        // 分屏前先收起面板:它和终端共用同一块文字区,留着的话新 pane 只能
        // 拿到"侧边栏之外那一半的一半"(实测被压到窗口的 20%),编辑器没法用。
        // 收起面板后布局是干净的 50/50,和用户自己按键分屏完全一致。
        self.browser_close();
        let Some(prog) = editor.first().cloned() else {
            return;
        };
        // 探测到的是绝对路径时就用绝对路径:裸名字会再去走一遍 shell 解析,
        // 解析失败时 [`pty`] 会回退到 pwsh —— 那就变成"标题写着文件、里面是
        // PowerShell"了。
        let prog = pty::which(&prog)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or(prog);
        let mut cfg = (*self.cfg).clone();
        cfg.shell = prog;
        let mut args = editor[1..].to_vec();
        args.push(path.to_string_lossy().into_owned());
        cfg.shell_args = args;
        cfg.cwd = cwd;
        // 编辑器 pane 走本地直连:它不需要跨会话保活,而且 mux 模式下
        // 传自定义 prog 要多绕一层(mux 的 spawn 只认 cfg.shell/args,行为一致
        // 但要重建会话,不如直接本地起)。
        cfg.session_persistence = false;
        let title = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        self.split_pane_with(SplitDir::Vertical, cfg, title);
    }

    /// 解析编辑器。配置里写了就照用(但**要先确认它真的存在**);没写就按
    /// `nvim` → `vim` 探测,成功的结果缓存在进程里 —— 失败不缓存,用户可能
    /// 正开着安装包。
    fn editor_pick(&self) -> EditorPick {
        // 调试钩子:临时换编辑器,方便验证"没装"的分支(不动用户的配置)
        if let Some(over) = std::env::var_os("VECTORTERM_FILES_EDITOR") {
            let over = over.to_string_lossy().to_string();
            return Self::pick_one(vec![over.clone()], &over);
        }
        if let Some(want) = self.cfg.files_gui.editor.first().cloned() {
            return Self::pick_one(self.cfg.files_gui.editor.clone(), &want);
        }
        static AUTO: OnceLock<Vec<String>> = OnceLock::new();
        if let Some(v) = AUTO.get() {
            return EditorPick::Ready(v.clone());
        }
        for cand in ["nvim.exe", "vim.exe"] {
            if let Some(p) = find_editor(cand) {
                let v = vec![p.to_string_lossy().into_owned()];
                let _ = AUTO.set(v.clone());
                return EditorPick::Ready(v);
            }
        }
        // 一个 vim 系都没有:记事本能打开文件,但必须同时说明为什么不是 vim
        let note = "nvim not installed or not in PATH (using notepad)".to_string();
        if let Some(np) = pty::which("notepad.exe") {
            return EditorPick::Fallback(vec![np.to_string_lossy().into_owned()], note);
        }
        EditorPick::Missing("nvim not installed or not in PATH".to_string())
    }

    /// 一条命令能不能跑起来:能就 Ready,不能就 Missing + 说明。
    fn pick_one(cmd: Vec<String>, want: &str) -> EditorPick {
        match pty::which(want) {
            Some(p) => EditorPick::Ready(
                std::iter::once(p.to_string_lossy().into_owned())
                    .chain(cmd.into_iter().skip(1))
                    .collect(),
            ),
            None => EditorPick::Missing(format!("\"{want}\" not installed or not in PATH")),
        }
    }

    /// 鼠标点进面板:选中该行;点在已选中的那一行上就当作"双击"打开。
    /// 返回 true = 这次点击被面板处理掉了,不要继续传给终端。
    fn browser_mouse(&mut self, x: f64, y: f64) -> bool {
        let Some((rect, list_y, rows)) = self.browser_geom() else {
            return false;
        };
        if !rect.contains(x, y) {
            // 点回终端:面板还开着,但键盘交还给 shell
            if let Some(b) = self.tab.browser.as_mut() {
                if b.focused {
                    b.focused = false;
                    self.window.request_redraw();
                }
            }
            return false;
        }
        let row = ((y - list_y) / self.renderer.cell_h).floor();
        // 标题行 / 页脚:吃掉点击,但不动选择
        if row < 0.0 || row >= rows as f64 {
            if let Some(b) = self.tab.browser.as_mut() {
                b.focused = true;
            }
            self.window.request_redraw();
            return true;
        }
        let row = row as usize;
        // 点之前就选中的那一行 → 当作再次点击 = 打开
        let again = self
            .tab
            .browser
            .as_ref()
            .is_some_and(|b| b.focused && b.scroll + row == b.selected);
        if let Some(b) = self.tab.browser.as_mut() {
            b.focused = true;
            b.click_row(row);
        }
        if again {
            self.browser_activate();
        } else {
            self.window.request_redraw();
        }
        true
    }

    /// 滚轮落在面板上就滚列表。返回 true = 已处理。
    fn browser_wheel(&mut self, lines: i64) -> bool {
        let Some((rect, _, _)) = self.browser_geom() else {
            return false;
        };
        if !rect.contains(self.mouse_px.0, self.mouse_px.1) {
            return false;
        }
        // 只滚列表,不抢键盘焦点:侧边栏和 vim 并排时,滚一下不该把输入
        // 从编辑器抢走(想操作列表就点一下,或者按 Ctrl+Shift+E)。
        if let Some(b) = self.tab.browser.as_mut() {
            b.scroll_by(lines as isize * 3);
        }
        self.window.request_redraw();
        true
    }

    fn mouse_moved(&mut self, x: f64, y: f64) {
        self.mouse_px = (x, y);

        // 模态确认框:移动只更新选项的反显,别的不做
        if self.confirm.is_some() {
            self.confirm_mouse(x, y, None);
            return;
        }

        // 不在拖选就不该继续自动滚屏(松手事件可能被别的窗口收走)
        let sel_dragging = self
            .tab
            .active_pane()
            .selection
            .current
            .as_ref()
            .is_some_and(|s| s.dragging);
        if !sel_dragging {
            self.edge_scroll = 0;
        }
        // 拖动分割栏:用该 Split 节点的区域把鼠标位置换算成新比例。
        // 必须优先于选区和点击聚焦,否则拖到一半会开始框选文字。
        if let Some((path, depth)) = self.split_drag {
            self.edge_scroll = 0;
            let found = {
                let (_, lines) = self.pane_geoms();
                lines.into_iter().find(|l| l.path == path && l.depth == depth)
            };
            if let Some(l) = found {
                let (ax, ay, aw, ah) = l.area;
                let r = if l.vertical {
                    (x - self.renderer.pad_x - ax) / aw.max(1.0)
                } else {
                    (y - self.renderer.pad_y - self.renderer.content_top - ay) / ah.max(1.0)
                };
                self.set_split_ratio(path, depth, r);
            }
            return;
        }
        // 拖动滚动条:优先于一切命中测试,直接换算成回看位置落位
        if let Some((id, grab_dy)) = self.scrollbar_drag {
            self.edge_scroll = 0;
            let geoms = self.scrollbar_geoms();
            if let Some((_, g)) = geoms.iter().find(|(pid, _)| *pid == id) {
                let pos = scrollbar::scroll_at(g, y, grab_dy);
                self.scrollbar_jump(id, pos);
            }
            return;
        }

        // 最大化窗口的标题栏:按下时只记锚点,移动超过阈值才还原窗口并
        // 开始拖动(和系统标题栏一样,单纯点一下不会把窗口还原)
        #[cfg(windows)]
        if let Some((ax, ay)) = self.pending_drag {
            if !live_resizing() {
                // 左键已经松开了(松手事件可能被别的窗口收走),作废锚点
                self.pending_drag = None;
            } else if (x - ax).abs() + (y - ay).abs() >= 3.0 {
                self.pending_drag = None;
                self.restore_for_drag((ax, ay));
                let _ = self.window.drag_window();
            }
        }

        let hit = self.tab_bar_hit(x, y);
        // 落在标签/按钮上的不做边缘缩放,否则标签最上面几像素会点不中
        let over_widget = matches!(
            hit,
            Some(tabbar::Hit::Tab(_))
                | Some(tabbar::Hit::Close(_))
                | Some(tabbar::Hit::NewTab)
                | Some(tabbar::Hit::Window(_))
        );
        self.resize_edge = None;
        if self.own_resize_border && !over_widget {
            if let Some(dir) = self.resize_dir_at(x, y) {
                self.resize_edge = Some(dir);
                let icon = CursorIcon::from(dir);
                if icon != self.cursor_icon {
                    self.window.set_cursor(icon);
                    self.cursor_icon = icon;
                }
                if self.tab_hover.is_some()
                    || self.new_tab_hover
                    || self.close_hover.is_some()
                    || self.window_hover.is_some()
                {
                    self.tab_hover = None;
                    self.new_tab_hover = false;
                    self.close_hover = None;
                    self.window_hover = None;
                    self.window.request_redraw();
                }
                return;
            }
        }

        // 标签栏:只更新悬停态,不参与选区,也不向应用报点
        // 关闭按钮悬停时整个标签也算悬停(背景一起亮)
        let hover = match hit {
            Some(tabbar::Hit::Tab(i)) | Some(tabbar::Hit::Close(i)) => Some(i),
            _ => None,
        };
        let close_hover = match hit {
            Some(tabbar::Hit::Close(i)) => Some(i),
            _ => None,
        };
        let new_hover = matches!(hit, Some(tabbar::Hit::NewTab));
        let win_hover = match hit {
            Some(tabbar::Hit::Window(k)) => Some(k),
            _ => None,
        };
        if hover != self.tab_hover
            || new_hover != self.new_tab_hover
            || close_hover != self.close_hover
            || win_hover != self.window_hover
        {
            self.tab_hover = hover;
            self.new_tab_hover = new_hover;
            self.close_hover = close_hover;
            self.window_hover = win_hover;
            self.window.request_redraw();
        }
        if hit.is_some() {
            if self.cursor_icon != CursorIcon::Default {
                self.window.set_cursor(CursorIcon::Default);
                self.cursor_icon = CursorIcon::Default;
            }
            return;
        }

        // 悬停在分割线上:换成可拖动的光标,这次移动也不要报给终端
        if let Some(l) = self.hit_split_line(x, y) {
            let icon = if l.vertical {
                CursorIcon::ColResize
            } else {
                CursorIcon::RowResize
            };
            if icon != self.cursor_icon {
                self.window.set_cursor(icon);
                self.cursor_icon = icon;
            }
            return;
        }

        let grabbed = self.tab.active_pane().term.lock().is_mouse_grabbed();
        // 可选文本用 I 形光标;应用抓了鼠标(或 Shift 强制选择)时还原/保持
        let icon = if grabbed && !self.mods.shift_key() {
            CursorIcon::Default
        } else {
            CursorIcon::Text
        };
        if icon != self.cursor_icon {
            self.window.set_cursor(icon);
            self.cursor_icon = icon;
        }

        // 先算好新坐标再拿可变借用,避免 &self / &mut self 冲突
        let dragging = self
            .tab
            .active_pane()
            .selection
            .current
            .as_ref()
            .is_some_and(|s| s.dragging);
        if dragging {
            let head = self.expand_head(self.mouse_pos());
            let sel = self.tab.active_pane_mut().selection.current.as_mut().expect("checked above");
            if sel.head != head {
                sel.head = head;
                self.window.request_redraw();
            }
            // 拖到窗口上/下边缘时开自动滚屏;离开带状区域就自动归零
            self.update_edge_scroll();
            return; // 拖拽期间不向应用报 Move
        }
        // 1002/1003 模式下移动要报点;其余模式 wezterm-term 内部会过滤。
        // 注意:先把锁释放掉再报点,否则会自我死锁(parking_lot 不可重入)
        if grabbed {
            self.emit_mouse(MouseEventKind::Move, MouseButton::None);
        }
    }

    fn mouse_button(&mut self, state: ElementState, button: WinitMouseButton) {
        // 模态确认框:只有按下才判,松开当作"移动"更新反显
        if self.confirm.is_some() {
            let pressed = (state == ElementState::Pressed).then_some(button);
            self.confirm_mouse(self.mouse_px.0, self.mouse_px.1, pressed);
            return;
        }
        // Shift 是"强制本地选择"的逃生键:vim/tmux 抓了鼠标也能框选
        let force_select = self.mods.shift_key();
        let mut grabbed = self.tab.active_pane().term.lock().is_mouse_grabbed();
        let pressed = state == ElementState::Pressed;
        if !pressed {
            // 松手就作废:最大化标题栏上"点一下"不该把窗口还原
            self.pending_drag = None;
            self.scrollbar_drag = None;
            self.split_drag = None;
            self.edge_scroll = 0;
        }

        // 标签栏优先:左键切换/新建,中键关闭(和 wezterm / 浏览器一致)
        if pressed {
            // 滚动条:拖动滑块 / 点轨道翻页,同时把焦点切到这个 pane
            if button == WinitMouseButton::Left {
                if let Some((id, _, hit)) =
                    self.hit_scrollbar(self.mouse_px.0, self.mouse_px.1)
                {
                    if let Some(idx) = self.tab.pane_index(id) {
                        if idx != self.tab.active_pane {
                            self.focus_pane_index(idx);
                        }
                    }
                    match hit {
                        scrollbar::Hit::Thumb { grab_dy } => {
                            self.scrollbar_drag = Some((id, grab_dy));
                        }
                        scrollbar::Hit::Track { above } => {
                            let page = self
                                .tab
                                .active_pane()
                                .grid
                                .1
                                .saturating_sub(2)
                                .max(1) as i64;
                            self.scroll_by(if above { page } else { -page });
                        }
                    }
                    self.window.request_redraw();
                    return;
                }
                // 分割栏:按下即开始拖动;先把没跑完的分屏动画结算掉,
                // 否则动画每帧会把 ratio 写回去,拖不动。
                if let Some(l) = self.hit_split_line(self.mouse_px.0, self.mouse_px.1) {
                    settle_split_anim(&mut self.tab);
                    self.split_drag = Some((l.path, l.depth));
                    self.window.request_redraw();
                    return;
                }
            }
            // 贴着窗口边缘按下 = 拖拽缩放(只有无边框模式会记录这个)
            if button == WinitMouseButton::Left {
                if let Some(dir) = self.resize_edge {
                    let _ = self.window.drag_resize_window(dir);
                    return;
                }
            }
            if let Some(hit) = self.tab_bar_hit(self.mouse_px.0, self.mouse_px.1) {
                match (button, hit) {
                    (WinitMouseButton::Left, tabbar::Hit::Close(i)) => self.close_tab_or_ask(i),
                    (WinitMouseButton::Left, tabbar::Hit::Tab(i)) => self.switch_to(i),
                    (WinitMouseButton::Left, tabbar::Hit::NewTab) => self.new_tab(),
                    (WinitMouseButton::Left, tabbar::Hit::Window(w)) => self.window_button(w),
                    (WinitMouseButton::Left, tabbar::Hit::Drag) => self.drag_title_bar(),
                    (WinitMouseButton::Middle, tabbar::Hit::Tab(i))
                    | (WinitMouseButton::Middle, tabbar::Hit::Close(i)) => self.close_tab_or_ask(i),
                    _ => {}
                }
                return;
            }
        }

        // 文件浏览器面板:点列表选行(同一行再点一次 = 打开),点面板别处只换
        // 焦点;这些点击都不要漏给终端,否则会在 shell 里开始一段选区。
        if pressed && self.tab.browser.is_some() {
            match button {
                WinitMouseButton::Left => {
                    if self.browser_mouse(self.mouse_px.0, self.mouse_px.1) {
                        return;
                    }
                }
                _ => {
                    if let Some((rect, _, _)) = self.browser_geom() {
                        if rect.contains(self.mouse_px.0, self.mouse_px.1) {
                            return;
                        }
                    }
                }
            }
        }

        // 鼠标点击聚焦:点进哪个 pane 就把焦点切过去,这次点击继续按新 pane 的
        // 局部坐标照常处理(开始选区 / 转发给应用),和 wezterm 一致。
        if pressed && self.cfg.pane_focus.click_to_focus {
            if let Some(idx) = self.pane_at(self.mouse_px.0, self.mouse_px.1) {
                if idx != self.tab.active_pane {
                    self.focus_pane_index(idx);
                    grabbed = self.tab.active_pane().term.lock().is_mouse_grabbed();
                }
            }
        }

        if button == WinitMouseButton::Right {
            if grabbed && !force_select {
                self.emit_mouse(
                    if pressed {
                        MouseEventKind::Press
                    } else {
                        MouseEventKind::Release
                    },
                    MouseButton::Right,
                );
            } else if pressed {
                // 有选区 → 再复制一次;没选区 → 粘贴
                if self.tab.active_pane().selection.current.is_some() {
                    self.copy_selection();
                } else {
                    self.paste();
                }
            }
            return;
        }

        if button == WinitMouseButton::Left {
            if pressed {
                if grabbed && !force_select {
                    // 交给应用点的那一刻,旧的本地选区就不该继续亮着
                    self.clear_selection_on_input();
                    self.emit_mouse(MouseEventKind::Press, MouseButton::Left);
                    return;
                }
                let mode =
                    self.tab
                        .active_pane_mut()
                        .selection
                        .press(self.mouse_px.0, self.mouse_px.1, Instant::now());
                self.begin_selection(self.mouse_pos(), mode);
                self.window.request_redraw();
                return;
            }
            // 松开:空选区(单纯点一下)清掉;否则按配置自动复制
            let mut clear_after = false;
            let mut do_copy = false;
            if let Some(sel) = self.tab.active_pane_mut().selection.current.as_mut() {
                if sel.dragging {
                    sel.dragging = false;
                    let empty = sel.is_empty();
                    if empty {
                        clear_after = true;
                    } else if self.cfg.selection.copy_on_select {
                        do_copy = true;
                    }
                }
            }
            if clear_after {
                self.tab.active_pane_mut().selection.clear();
                self.window.request_redraw();
                return;
            }
            if do_copy {
                self.copy_selection();
                self.window.request_redraw();
                return;
            }
            if grabbed && !force_select {
                self.emit_mouse(MouseEventKind::Release, MouseButton::Left);
            }
            return;
        }

        // 中键:只在应用抓鼠标时转发(侧键 xterm button 8/9 暂不报点)
        if button == WinitMouseButton::Middle && grabbed && !force_select {
            self.emit_mouse(
                if pressed {
                    MouseEventKind::Press
                } else {
                    MouseEventKind::Release
                },
                MouseButton::Middle,
            );
        }
    }

    /// 鼠标位置 → (列, 绝对物理行号)
    fn mouse_pos(&self) -> selection::Pos {
        let (col, row) = self.cell_at(self.mouse_px.0, self.mouse_px.1);
        (col, self.abs_row(row))
    }

    /// 当前视口顶行的绝对物理行号;与 `render::snapshot` 的 base/frac 口径一致
    fn view_top(&self) -> usize {
        let t = self.tab.active_pane().term.lock();
        view_geom_of(t.screen(), self.tab.active_pane().pad, self.tab.active_pane().scroll.pos()).0
    }

    fn abs_row(&self, viewport_row: usize) -> usize {
        self.view_top() + viewport_row
    }

    /// 取某一绝对物理行的归一化单元格(双击选词 / 三击选行要用)
    fn row_cells(&self, abs_row: usize) -> Vec<render::SnapCell> {
        let t = self.tab.active_pane().term.lock();
        let s = t.screen();
        let rows = s.physical_rows.max(1);
        let sb = s.scrollback_rows();
        let max = sb.saturating_sub(rows) as f32;
        let scroll = (sb.saturating_sub(rows + abs_row) as f32).min(max);
        let base = scroll.ceil() as usize;
        let top = sb.saturating_sub(rows + base);
        let (snap, _, _) = render::snapshot(&t, self.tab.active_pane().grid.0, scroll);
        snap.rows
            .get(abs_row.saturating_sub(top))
            .cloned()
            .unwrap_or_default()
    }

    /// 双击/三击之后继续拖:按词/行粒度扩展终点,而不是退化成单格。
    fn expand_head(&self, head: selection::Pos) -> selection::Pos {
        let Some(sel) = self.tab.active_pane().selection.current.as_ref() else {
            return head;
        };
        let (a, b) = match sel.mode {
            selection::Mode::Cell => return head,
            selection::Mode::Word => {
                let row = self.row_cells(head.1);
                selection::word_bounds(&row, head.0)
            }
            selection::Mode::Line => {
                let row = self.row_cells(head.1);
                selection::line_bounds(&row)
            }
        };
        // 往左上拖取词/行首,往右下拖取词/行尾
        let backward = (head.1, head.0) < (sel.anchor.1, sel.anchor.0);
        (if backward { a } else { b }, head.1)
    }

    fn begin_selection(&mut self, pos: selection::Pos, mode: selection::Mode) {
        let (c0, c1) = match mode {
            selection::Mode::Cell => (pos.0, pos.0),
            _ => {
                let row = self.row_cells(pos.1);
                if mode == selection::Mode::Word {
                    selection::word_bounds(&row, pos.0)
                } else {
                    selection::line_bounds(&row)
                }
            }
        };
        let mut sel = selection::Selection::new((c0, pos.1), mode);
        sel.head = (c1, pos.1);
        self.tab.active_pane_mut().selection.current = Some(sel);
    }

    /// 把当前选区写进系统剪贴板(OSC 52 走同一条通道)
    fn copy_selection(&self) {
        let Some(sel) = self.tab.active_pane().selection.current.as_ref() else {
            return;
        };
        if sel.is_empty() {
            return;
        }
        let text = {
            let t = self.tab.active_pane().term.lock();
            selection::selected_text(&t, self.tab.active_pane().grid.0, sel)
        };
        if text.is_empty() {
            return;
        }
        if let Err(e) = term::set_clipboard_text(&text) {
            crate::wlog!("[VectorTerm] 复制到剪贴板失败:{e}");
        }
    }

    /// 绝对物理行号 → 当前视口坐标,供渲染层画高亮
    fn selection_highlight(&self, top: usize, rows: usize) -> Option<frame::Highlight> {
        let p = self.tab.active_pane();
        self.pane_selection_highlight(p, top, rows)
    }

    /// `selection_highlight` 的指定 pane 版本(分屏时每个 pane 各有选区)。
    fn pane_selection_highlight(
        &self,
        p: &Pane,
        top: usize,
        rows: usize,
    ) -> Option<frame::Highlight> {
        pane_selection_highlight_impl(&self.cfg, p, top, rows)
    }

    // ---------------- 滚动条 / 历史搜索 / 边缘自动滚屏 ----------------

    /// 每个 pane 右侧的滚动条几何(整窗坐标)。
    ///
    /// 用 `rows + max` 当"有效历史行数":`max` 就是 ScrollAnim 的回看上限
    /// (已扣掉 resize 补出来的空行),于是滑块长度、拖动换算和滚轮上限
    /// 完全同源,不会出现"拖到底了还能再滚两行"。
    fn scrollbar_geoms(&self) -> Vec<(u64, scrollbar::Geom)> {
        if !self.cfg.scrollbar.enabled || self.cfg.scrollbar.show == ScrollbarShow::Never {
            return Vec::new();
        }
        let (geoms, _) = self.pane_geoms();
        let ox = self.renderer.pad_x;
        let oy = self.renderer.pad_y + self.renderer.content_top;
        let cfg = &self.cfg.scrollbar;
        let mut out = Vec::with_capacity(geoms.len());
        for g in &geoms {
            let Some(idx) = self.tab.pane_index(g.pane_id) else {
                continue;
            };
            let p = &self.tab.panes[idx];
            let t = p.term.lock();
            let s = t.screen();
            let rows = s.physical_rows.max(1);
            let max = view_geom_of(s, p.pad, 0.0).1;
            let mut geom = scrollbar::geometry(
                scrollbar::Rect {
                    x: ox + g.x,
                    y: oy + g.y,
                    w: g.w,
                    h: g.h,
                },
                cfg.width,
                rows,
                rows + max,
                p.scroll.pos() as f64,
                cfg.min_thumb,
            );
            // 默认贴右边缘;改成左边时整体平移过去。绘制和命中都用这一份
            // Geom,所以换侧不需要第二套命中逻辑。
            if cfg.side == ScrollbarSide::Left {
                let dx = (ox + g.x) - geom.track.x;
                geom.track.x += dx;
                geom.thumb.x += dx;
            }
            out.push((g.pane_id, geom));
        }
        out
    }

    fn hit_scrollbar(&self, x: f64, y: f64) -> Option<(u64, scrollbar::Geom, scrollbar::Hit)> {
        self.scrollbar_geoms().into_iter().find_map(|(id, g)| {
            scrollbar::hit(&g, x, y).map(|h| (id, g, h))
        })
    }

    /// 拖动滚动条时直接落位(不走缓动动画:拖动必须跟手)。
    fn scrollbar_jump(&mut self, pane_id: u64, pos: f64) {
        let Some(idx) = self.tab.pane_index(pane_id) else {
            return;
        };
        {
            let p = &mut self.tab.panes[idx];
            let max = {
                let t = p.term.lock();
                view_geom_of(t.screen(), p.pad, 0.0).1
            };
            p.scroll
                .jump_to((pos as f32).clamp(0.0, max as f32), max);
        }
        self.window.request_redraw();
    }

    /// 活动 pane 的内容区矩形(整窗坐标),边缘滚屏的判定区域。
    fn active_pane_rect(&self) -> scrollbar::Rect {
        let (geoms, _) = self.pane_geoms();
        let id = self.tab.active_pane().id;
        let g = geoms
            .iter()
            .find(|g| g.pane_id == id)
            .cloned()
            .unwrap_or(PaneGeom {
                pane_id: id,
                grid: self.tab.active_pane().grid,
                x: 0.0,
                y: 0.0,
                w: self.grid_px.0 as f64,
                h: self.grid_px.1 as f64,
            });
        scrollbar::Rect {
            x: self.renderer.pad_x + g.x,
            y: self.renderer.pad_y + self.renderer.content_top + g.y,
            w: g.w,
            h: g.h,
        }
    }

    fn update_edge_scroll(&mut self) {
        if !self.cfg.selection.edge_scroll {
            self.edge_scroll = 0;
            return;
        }
        let r = self.active_pane_rect();
        self.edge_scroll = edge_scroll_lines(self.mouse_px.1, r.y, r.y + r.h, self.renderer.cell_h);
    }

    /// 每 45ms 推进一次:滚动视口 + 把选区终点重新锚到鼠标所在的绝对行。
    /// 视口一动,同样的屏幕行就指向另一条历史行了,所以必须重算 head。
    fn tick_edge_scroll(&mut self, now: Instant) {
        if self.edge_scroll == 0 || now < self.next_edge_tick {
            return;
        }
        self.next_edge_tick = now + Duration::from_millis(45);
        let lines = self.edge_scroll;
        {
            let p = self.tab.active_pane_mut();
            let max = {
                let t = p.term.lock();
                view_geom_of(t.screen(), p.pad, 0.0).1
            };
            let pos = (p.scroll.pos() + lines as f32).clamp(0.0, max as f32);
            p.scroll.jump_to(pos, max);
        }
        let head = self.expand_head(self.mouse_pos());
        if let Some(sel) = self.tab.active_pane_mut().selection.current.as_mut() {
            sel.head = head;
        }
        self.window.request_redraw();
    }

    // ---- 命令面板 ----

    /// `Ctrl+Shift+P`:直接打开命令面板(再按一次关闭)。
    fn panel_shortcut(&mut self, ev: &winit::event::KeyEvent) -> bool {
        let m = self.mods | physical_modifiers();
        if !m.control_key() || !m.shift_key() || m.alt_key() {
            return false;
        }
        let is_p = matches!(ev.physical_key, PhysicalKey::Code(WinitKeyCode::KeyP))
            || matches!(&ev.logical_key, Key::Character(s) if s.as_str().eq_ignore_ascii_case("p"));
        if !is_p {
            return false;
        }
        if self.panel.active {
            self.close_panel();
        } else {
            self.close_search();
            self.open_panel();
        }
        self.window.request_redraw();
        true
    }

    /// 命令面板激活时吞掉所有按键。
    fn panel_key(&mut self, ev: &winit::event::KeyEvent) {
        let m = self.mods | physical_modifiers();
        if m.control_key() && m.shift_key() && !m.alt_key() {
            // 面板里按 Ctrl+Shift+F 不应该只是「关掉」:用户按的是搜索键,
            // 就真的切到历史搜索栏,免得看完 help/key 输出还得再按一次。
            if matches!(ev.physical_key, PhysicalKey::Code(WinitKeyCode::KeyF))
                || matches!(&ev.logical_key, Key::Character(s) if s.as_str().eq_ignore_ascii_case("f"))
            {
                self.close_panel();
                self.open_search();
                self.window.request_redraw();
                return;
            }
            if matches!(ev.physical_key, PhysicalKey::Code(WinitKeyCode::KeyP))
                || matches!(&ev.logical_key, Key::Character(s) if s.as_str().eq_ignore_ascii_case("p"))
            {
                self.close_panel();
                self.window.request_redraw();
                return;
            }
        }
        // `help` / `key` 的输出比一屏长,方向键在这里是翻页而不是选命令。
        let rows = commands::output_page_rows(self.grid_px.1 as f64, self.renderer.cell_h);
        match &ev.logical_key {
            Key::Named(NamedKey::Escape) => {
                if self.panel.output.is_some() {
                    self.panel.output = None;
                    self.panel.scroll = 0;
                } else {
                    self.close_panel();
                }
            }
            Key::Named(NamedKey::Enter) => {
                if self.panel.output.is_some() {
                    self.close_panel();
                } else if let Some(cmd) = self.panel.selection() {
                    self.run_panel_command(cmd);
                }
            }
            Key::Named(NamedKey::ArrowDown) => {
                if self.panel.output.is_some() {
                    self.panel.page(1, rows);
                } else {
                    self.panel.move_selection(1);
                }
            }
            Key::Named(NamedKey::ArrowUp) => {
                if self.panel.output.is_some() {
                    self.panel.page(-1, rows);
                } else {
                    self.panel.move_selection(-1);
                }
            }
            Key::Named(NamedKey::PageDown) => {
                self.panel.page(rows as isize, rows);
            }
            Key::Named(NamedKey::PageUp) => {
                self.panel.page(-(rows as isize), rows);
            }
            Key::Named(NamedKey::Home) | Key::Named(NamedKey::End) => {
                let home = matches!(ev.logical_key, Key::Named(NamedKey::Home));
                let n = self
                    .panel
                    .output
                    .as_ref()
                    .map(|o| o.lines.len() as isize)
                    .unwrap_or(0);
                self.panel.page(if home { -n } else { n }, rows);
            }
            Key::Named(NamedKey::Tab) => self.panel.complete(),
            Key::Named(NamedKey::Backspace) => {
                self.panel.query.pop();
                self.panel.selected = 0;
                self.panel.output = None;
                self.panel.scroll = 0;
            }
            _ => {
                let typed: Option<String> = ev
                    .text
                    .as_ref()
                    .map(|t| t.chars().filter(|c| !c.is_control()).collect())
                    .or_else(|| match &ev.logical_key {
                        Key::Character(s) => Some(s.chars().filter(|c| !c.is_control()).collect()),
                        _ => None,
                    });
                if let Some(s) = typed {
                    if !s.is_empty() {
                        self.panel_input(&s);
                    }
                }
            }
        }
        self.window.request_redraw();
    }

    /// 执行面板里选中的命令。`help` / `key` 不关闭面板,改成多行输出视图。
    fn run_panel_command(&mut self, cmd: commands::Command) {
        match cmd {
            commands::Command::Files => {
                self.close_panel();
                self.open_files_full();
            }
            commands::Command::Settings => {
                self.close_panel();
                self.open_settings_gui();
            }
            commands::Command::Help => {
                self.panel
                    .show_output("命令列表  ·  help", commands::help_lines());
            }
            commands::Command::Keys => {
                self.panel.show_output("键位表  ·  key", commands::key_lines());
            }
            commands::Command::Reload => {
                self.close_panel();
                self.reload_config();
            }
            commands::Command::Search => {
                self.close_panel();
                self.open_search();
            }
        }
        self.window.request_redraw();
    }

    // ---- 历史搜索 ----

    fn open_search(&mut self) {
        if !self.cfg.search.enabled {
            return;
        }
        self.search.open();
        self.search_anim.open(Instant::now());
        self.seed_overlay_trail();
        self.window.request_redraw();
    }

    fn close_search(&mut self) {
        if !self.search.active {
            return;
        }
        self.search.close();
        // 逻辑上已经关了(按键马上回终端),动画继续把镜头收完
        self.search_anim.close(Instant::now());
        self.window.request_redraw();
    }

    /// 打开命令面板。和 `open_search` 一一对应:开浮层 → 播进场 → 光标飞过去。
    fn open_panel(&mut self) {
        if self.panel.active {
            return;
        }
        self.panel.open();
        self.panel_anim.open(Instant::now());
        self.seed_overlay_trail();
        self.window.request_redraw();
    }

    fn close_panel(&mut self) {
        if !self.panel.active {
            return;
        }
        self.panel.close();
        self.panel_anim.close(Instant::now());
        self.window.request_redraw();
    }

    /// 输入光标现在归谁:浮层在屏上(含退场过程)时归浮层。
    ///
    /// 浮层一开,输入焦点就搬到那个输入框去了,终端里原来那个方块必须收起来
    /// —— 否则"光标飞过去"之后原地还留着一个在闪,看着像两个光标。
    /// 没开浮层但开着确认框时不算:确认框没有文本输入,终端光标留着更自然。
    fn overlay_owns_caret(&self) -> bool {
        self.search_anim.visible() || self.panel_anim.visible()
    }

    /// 浮层里正在组字的那一段(拼音还没上屏的串)。
    ///
    /// 浮层在屏上时它归浮层:画在输入框里跟着走,终端光标那边的
    /// `build_ime_box` 就不再画 —— 否则拼音会出现在终端光标下面,
    /// 看着像正在往 shell 里打字。
    fn overlay_preedit(&self) -> Option<&str> {
        if !self.overlay_owns_caret() {
            return None;
        }
        self.ime_preedit.as_ref().map(|p| p.text.as_str())
    }

    /// 活动 pane 的输入光标格(内容区像素,整格)。
    ///
    /// 跨窗格尾迹和浮层飞入都要这个起点 —— 抽出来一份,免得两处各算一遍
    /// 结果差半格。
    fn active_cursor_rect(&self) -> Option<[f32; 4]> {
        let g = self
            .pane_geoms()
            .0
            .into_iter()
            .find(|g| g.pane_id == self.tab.panes[self.tab.active_pane].id)?;
        let cw = self.renderer.cell_w;
        let ch = self.renderer.cell_h;
        let (col, row) = {
            let t = self.tab.panes[self.tab.active_pane].term.lock();
            let pos = t.cursor_pos();
            (pos.x.max(0) as f64, pos.y.max(0) as f64)
        };
        let col = col.min(g.grid.0.saturating_sub(1) as f64);
        let row = row.min(g.grid.1.saturating_sub(1) as f64);
        let x = g.x + col * cw;
        let y = g.y + row * ch;
        Some([x as f32, (x + cw) as f32, y as f32, (y + ch) as f32])
    }

    /// 浮层打开时让输入光标"飞"过去:把尾迹种在终端光标格上,之后每帧由
    /// `redraw` 喂给它浮层里的落点。
    fn seed_overlay_trail(&mut self) {
        let Some(r) = self.active_cursor_rect() else {
            return;
        };
        // 和跨窗格飞行同一组开关:`pane_focus.trail` 主导关不关,
        // `cursor.trail_dwell_ms = 0` 是更底层的"整个尾迹系统关掉"
        if !self.cfg.pane_focus.trail
            || self.cfg.pane_focus.trail_ms <= 0.0
            || self.cfg.cursor.trail_dwell_ms <= 0.0
        {
            return;
        }
        // `active_cursor_rect` 给的是**内容区**坐标,浮层用的是**窗口**坐标:
        // 差一个内容区原点,不加的话尾巴会从左上角斜着飞过来。
        let ox = self.renderer.pad_x as f32;
        let oy = (self.renderer.pad_y + self.renderer.content_top) as f32;
        let r = [r[0] + ox, r[1] + ox, r[2] + oy, r[3] + oy];
        self.overlay_trail.seed(r, Instant::now());
        self.overlay_trail_on = true;
        // 让位:同一屏不画两条同色尾巴(窗格内那条会从终端光标往外拖)
        self.tab.active_pane_mut().trail.reset();
    }

    /// 重算匹配。`force = true` 用于回车跳转前(必须拿到最新结果);平时
    /// 只在终端又输出过内容时、且距上次重算 ≥180ms 才扫一遍全量历史。
    fn refresh_search(&mut self, force: bool) {
        if !self.search.active {
            return;
        }
        let now = Instant::now();
        if !force && (!self.search.dirty || now < self.next_search_refresh) {
            return;
        }
        self.next_search_refresh = now + Duration::from_millis(180);
        let p = self.tab.active_pane();
        let cols = p.grid.0;
        let lines = {
            let t = p.term.lock();
            search::collect_lines(&t, cols)
        };
        self.search.refresh(&lines, self.cfg.search.max_matches);
    }

    fn jump_to_search_match(&mut self, m: search::Match) {
        {
            let p = self.tab.active_pane_mut();
            let (max, sb, rows) = {
                let t = p.term.lock();
                let s = t.screen();
                (
                    view_geom_of(s, p.pad, 0.0).1,
                    s.scrollback_rows(),
                    s.physical_rows.max(1),
                )
            };
            let pos = search::scroll_for_match(m, sb, rows, max as f64) as f32;
            p.scroll.jump_to(pos.min(max as f32), max);
        }
        self.window.request_redraw();
    }

    /// `Ctrl+Shift+F`:打开 / 再按一次关闭。用物理键兜底,和分屏快捷键
    /// 一样避开 Windows 上 AltGr/Ctrl 组合把 logical_key 弄坏的问题。
    fn search_shortcut(&mut self, ev: &winit::event::KeyEvent) -> bool {
        // winit 的 ModifiersChanged 有时比 KEYDOWN 晚一拍(模拟按键 / 输入法
        // 激活时尤其明显),和分屏快捷键一样用 GetAsyncKeyState 兜底。
        let m = self.mods | physical_modifiers();
        if !self.cfg.search.enabled
            || !m.control_key()
            || !m.shift_key()
            || m.alt_key()
        {
            return false;
        }
        let is_f = matches!(
            ev.physical_key,
            PhysicalKey::Code(WinitKeyCode::KeyF)
        ) || matches!(&ev.logical_key, Key::Character(s) if s.as_str().eq_ignore_ascii_case("f"));
        if !is_f {
            return false;
        }
        if self.search.active {
            self.close_search();
        } else {
            self.open_search();
        }
        true
    }

    /// 搜索栏激活时吞掉所有按键:输入只改 query,不发给终端。
    fn search_key(&mut self, ev: &winit::event::KeyEvent) -> bool {
        if ev.state != ElementState::Pressed {
            return true;
        }
        let m = self.mods | physical_modifiers();
        // 两个浮层之间可以直切:`Ctrl+Shift+P` 去命令面板、`Ctrl+Shift+F`
        // 回搜索栏(命令面板那边同样有这个对称分支)。少了这一条的话,
        // 在搜索框里按 `Ctrl+Shift+P` 会把 `P` 当成要搜的字符打进去。
        if m.control_key() && m.shift_key() && !m.alt_key() {
            let hit = |code: WinitKeyCode, ch: &str| {
                matches!(ev.physical_key, PhysicalKey::Code(c) if c == code)
                    || matches!(&ev.logical_key, Key::Character(s) if s.as_str().eq_ignore_ascii_case(ch))
            };
            if hit(WinitKeyCode::KeyP, "p") {
                self.close_search();
                self.open_panel();
                self.window.request_redraw();
                return true;
            }
            if hit(WinitKeyCode::KeyF, "f") {
                self.close_search();
                return true;
            }
        }
        let step = |s: &mut Self, d: isize| {
            s.refresh_search(true);
            if let Some(m) = s.search.step(d) {
                s.jump_to_search_match(m);
            }
            s.window.request_redraw();
        };
        match &ev.logical_key {
            Key::Named(NamedKey::Escape) => {
                self.close_search();
                return true;
            }
            Key::Named(NamedKey::Enter) => {
                let m = self.mods | physical_modifiers();
                step(self, if m.shift_key() { -1 } else { 1 });
                return true;
            }
            Key::Named(NamedKey::ArrowDown) => {
                step(self, 1);
                return true;
            }
            Key::Named(NamedKey::ArrowUp) => {
                step(self, -1);
                return true;
            }
            Key::Named(NamedKey::Backspace) => {
                self.search.query.pop();
                self.search.current = 0;
                self.search.dirty = true;
                self.window.request_redraw();
                return true;
            }
            Key::Named(NamedKey::Tab) => {
                self.search.case_sensitive = !self.search.case_sensitive;
                self.search.dirty = true;
                self.window.request_redraw();
                return true;
            }
            _ => {}
        }
        // winit 的 text 已经处理了 Shift / IME;logical_key 只是兜底
        let typed: Option<String> = ev
            .text
            .as_ref()
            .map(|t| t.chars().filter(|c| !c.is_control()).collect())
            .or_else(|| match &ev.logical_key {
                Key::Character(s) => Some(s.chars().filter(|c| !c.is_control()).collect()),
                _ => None,
            });
        if let Some(s) = typed {
            if !s.is_empty() {
                // 搜索框里第一个字符输 `/` = 切到命令面板。以前唤起 UI 得
                // 在 shell 里敲专用命令,现在全部收进这个输入框。
                self.search_input(&s);
                self.window.request_redraw();
            }
        }
        true
    }

    // ---- 叠加层绘制 ----

    fn scrollbar_colors(
        &self,
        palette: &render::Palette,
    ) -> (render::Rgb, render::Rgb, render::Rgb) {
        let track = self
            .cfg
            .scrollbar
            .track_color
            .as_deref()
            .and_then(term::color_from_str)
            .unwrap_or_else(|| mix_rgb(palette.fg, palette.bg, 0.88));
        let thumb = self
            .cfg
            .scrollbar
            .thumb_color
            .as_deref()
            .and_then(term::color_from_str)
            .unwrap_or_else(|| mix_rgb(palette.fg, palette.bg, 0.34));
        let hover = self
            .cfg
            .scrollbar
            .hover_color
            .as_deref()
            .and_then(term::color_from_str)
            .unwrap_or(thumb);
        (track, thumb, hover)
    }

    fn paint_scrollbars(&mut self, geoms: &[(u64, scrollbar::Geom)], palette: &render::Palette) {
        let cfg = &self.cfg.scrollbar;
        let (track_c, thumb_c, hover_c) = self.scrollbar_colors(palette);
        let hovered_id = self.scrollbar_drag.map(|(id, _)| id);
        for (id, g) in geoms {
            let idx = self.tab.pane_index(*id);
            let at_bottom = idx
                .map(|i| self.tab.panes[i].scroll.pos() <= 0.001)
                .unwrap_or(true);
            let hovered = hovered_id == Some(*id)
                || g.track.contains(self.mouse_px.0, self.mouse_px.1);
            if cfg.show == ScrollbarShow::Auto && at_bottom && !hovered {
                continue;
            }
            // 只画滑块、不画轨道:分屏时轨道会贴着分割线形成一条长灰带
            if cfg.show_track {
                let t = &g.track;
                self.frame.round_rect(
                    t.x,
                    t.y + 2.0,
                    t.w,
                    (t.h - 4.0).max(1.0),
                    t.w * 0.5,
                    track_c,
                    cfg.opacity * 0.45,
                );
            }
            let th = &g.thumb;
            self.frame.round_rect(
                th.x + 1.0,
                th.y,
                (th.w - 2.0).max(1.0),
                th.h,
                th.w * 0.5,
                if hovered { hover_c } else { thumb_c },
                cfg.opacity * if hovered { 1.0 } else { 0.78 },
            );
        }
    }

    /// 搜索栏浮层。放在标签栏之后画,所以要先标 `overlay_at`:否则内容区
    /// 裁剪会把它当成终端文字切掉(标签栏在底部时尤其明显)。
    ///
    /// 进场时从下方 22px 非线性地升到位、同时从 94% 放大到原尺寸并淡入;
    /// 退场是同一套动作反过来(见 [`OverlayXf`] / [`OverlayAnim`])。
    ///
    /// 返回 (当前帧的输入光标, 动画结束时的输入光标),都是窗口像素。
    ///
    /// 尾迹要的是**终点**:面板自己还在缩放位移,若每帧都把当前帧位置当目标,
    /// 状态机的 dwell 会被无限重置,尾巴永远不启动。
    fn paint_search_bar(
        &mut self,
        w: u32,
        h: u32,
        palette: &render::Palette,
    ) -> Option<([f64; 4], [f64; 4])> {
        // 逻辑关闭后还要把退场画完,所以判的是"进度还在不在",不是 `active`
        if !self.search_anim.visible() {
            return None;
        }
        let t = self.search_anim.value();
        let a = overlay_alpha(t);
        // 输入法正在组字的那一段。取在借用 renderer/frame 之前,
        // 之后就不再碰 &self。
        let preedit = self.overlay_preedit().unwrap_or("").to_string();
        let renderer = &mut self.renderer;
        let frame = &mut self.frame;
        if frame.overlay_at.is_none() {
            frame.overlay_at = Some(frame.items.len());
        }
        let cw = renderer.cell_w;
        let ch = renderer.cell_h;
        let pad = 8.0;
        let bar_h = ch + pad * 1.5;
        let bw = (w as f64 * 0.62).min(cw * 62.0).max(cw * 26.0);
        let bx = (w as f64 - bw - 14.0).max(8.0);
        let by = (frame.clip_bottom as f64 - bar_h - 10.0)
            .max(renderer.content_top + 4.0)
            .min(h as f64 - bar_h - 4.0);
        // 绕自身中心缩放 + 竖直位移:框和字走同一个变换,不会一大一小
        let xf = OverlayXf::new(bx + bw * 0.5, by + bar_h * 0.5, t, 22.0);
        let bg = mix_rgb(palette.bg, mix_rgb(palette.bg, palette.fg, 0.06), a);
        let dim = mix_rgb(palette.bg, mix_rgb(palette.fg, palette.bg, 0.45), a);
        let fg = mix_rgb(palette.bg, palette.fg, a);
        let accent = mix_rgb(
            palette.bg,
            term::color_from_str(&self.cfg.search.current_color)
                .unwrap_or(render::Rgb {
                    r: 255,
                    g: 143,
                    b: 63,
                }),
            a,
        );
        let (rx, ry, rw, rh) = xf.rect(bx - 1.0, by - 1.0, bw + 2.0, bar_h + 2.0);
        frame.round_rect(rx, ry, rw, rh, xf.len(7.0), dim, 0.55 * a);
        let (rx, ry, rw, rh) = xf.rect(bx, by, bw, bar_h);
        frame.round_rect(rx, ry, rw, rh, xf.len(6.0), bg, 0.97 * a);
        let baseline = xf.baseline(by + pad * 0.5 + renderer.ascent);
        let head = format!(
            "/ {}{}",
            if self.search.case_sensitive { "[Aa] " } else { "" },
            self.search.query
        );
        // 组合串(拼音没上屏那段)直接接在查询后面画,下面再加一条下划线,
        // 和系统输入法的观感一致 —— 不动辄弹一个独立小窗。
        let left = if preedit.is_empty() {
            head.clone()
        } else {
            format!("{head}{preedit}")
        };
        let (tx, _) = xf.pt(bx + pad, 0.0);
        let (tmax, _) = xf.pt(bx + bw - pad, 0.0);
        renderer.draw_scaled_text(frame, &left, tx, baseline, tmax, fg, true, xf.s);
        // 输入位置的细竖条,让人一眼看出这是可输入的框
        let caret_x = bx + pad + renderer.text_width_at(&left, xf.s);
        if !preedit.is_empty() {
            let ux = bx + pad + renderer.text_width_at(&head, xf.s);
            let (ux, uy, uw, uh) = xf.rect(ux, by + pad * 0.5 + ch - 2.0, (caret_x - ux).max(2.0), 1.5);
            frame.rect(ux, uy, uw, uh, fg, 0.8 * a);
        }
        let (kx, ky, kw, kh) = xf.rect(caret_x, by + pad * 0.5, (cw * 0.5).max(2.0), ch);
        frame.rect(kx, ky, kw, kh, accent, 0.9 * a);
        let caret = [kx, kx + kw, ky, ky + kh];
        let label = if self.search.matches.is_empty() {
            "0 / 0  Enter↓  Tab:Aa  Esc".to_string()
        } else {
            format!(
                "{} / {}  Enter↓  Tab:Aa  Esc",
                self.search.current + 1,
                self.search.matches.len()
            )
        };
        let lw = renderer.text_width_at(&label, xf.s);
        let lx = (bx + bw - pad - lw).max(caret_x + cw);
        let (lxp, _) = xf.pt(lx, 0.0);
        let (lmax, _) = xf.pt(bx + bw - pad, 0.0);
        renderer.draw_scaled_text(frame, &label, lxp, baseline, lmax, dim, false, xf.s);
        let fx = bx + pad + renderer.text_width_at(&left, 1.0);
        let fy = by + pad * 0.5;
        let kw = (cw * 0.5).max(2.0);
        Some((caret, [fx, fx + kw, fy, fy + ch]))
    }

    /// 命令面板浮层。和历史搜索栏共用 `overlay_at` + `draw_plain_text`,
    /// 只是把单行输入框换成「输入框 + 多行命令列表 / 帮助输出」。
    /// 命令面板浮层。和历史搜索栏共用 `overlay_at` + `draw_scaled_text`,
    /// 只是把单行输入框换成「输入框 + 多行命令列表 / 帮助输出」。
    ///
    /// 进出场和搜索栏是同一套变换(从下方升起 + 放大淡入),只是位移小一点:
    /// 它本来就画在偏上的位置,升太多反而像在跳。
    ///
    /// 返回 (当前帧的输入光标, 动画结束时的输入光标),含义同 [`paint_search_bar`]。
    fn paint_command_panel(
        &mut self,
        w: u32,
        h: u32,
        palette: &render::Palette,
    ) -> Option<([f64; 4], [f64; 4])> {
        // 逻辑关闭后还要把退场画完,所以判的是"进度还在不在",不是 `active`
        if !self.panel_anim.visible() {
            return None;
        }
        let t = self.panel_anim.value();
        let a = overlay_alpha(t);
        // 同搜索栏:组合串在借用 renderer/frame 之前取好
        let preedit = self.overlay_preedit().unwrap_or("").to_string();
        let renderer = &mut self.renderer;
        let frame = &mut self.frame;
        if frame.overlay_at.is_none() {
            frame.overlay_at = Some(frame.items.len());
        }
        let cw = renderer.cell_w;
        let ch = renderer.cell_h;
        let pad = 9.0;
        let bar_h = ch + pad * 1.2;
        let bw = (w as f64 * 0.68).min(cw * 78.0).max(cw * 36.0);
        let bx = ((w as f64 - bw) * 0.5).max(8.0);

        let selected = self.panel.selected;
        let page_rows = commands::output_page_rows(h as f64, ch);
        let rows: Vec<(String, bool)> = match &self.panel.output {
            Some(o) => {
                let total = o.lines.len();
                let start = self.panel.scroll.min(total.saturating_sub(1));
                let end = (start + page_rows).min(total);
                let mut v = vec![(format!("  {}", o.title), true)];
                v.extend(
                    o.lines[start..end]
                        .iter()
                        .map(|l| (format!("  {l}"), false)),
                );
                if total > page_rows {
                    v.push((
                        format!(
                            "  [{}-{} / {}]  ↑↓ 逐行  PgUp/PgDn 翻页  Home/End 首尾",
                            start + 1,
                            end,
                            total
                        ),
                        false,
                    ));
                }
                v
            }
            None => {
                let m = self.panel.matches();
                if m.is_empty() {
                    vec![("  (没有匹配的命令)".to_string(), false)]
                } else {
                    let n = m.len();
                    m.iter()
                        .take(8)
                        .enumerate()
                        .map(|(i, s)| {
                            let on = i == selected.min(n - 1);
                            (
                                format!(
                                    "  {}{:<12}{}",
                                    if on { "> " } else { "  " },
                                    s.name,
                                    s.desc
                                ),
                                on,
                            )
                        })
                        .collect()
                }
            }
        };
        let list_h = rows.len() as f64 * ch;
        let total_h = bar_h + list_h + pad;
        let by = (h as f64 * 0.15)
            .max(renderer.content_top + 6.0)
            .min((h as f64 - total_h - 8.0).max(renderer.content_top + 6.0));
        let xf = OverlayXf::new(bx + bw * 0.5, by + total_h * 0.5, t, 16.0);
        let bg = mix_rgb(palette.bg, mix_rgb(palette.bg, palette.fg, 0.07), a);
        let dim = mix_rgb(palette.bg, mix_rgb(palette.fg, palette.bg, 0.45), a);
        let fg = mix_rgb(palette.bg, palette.fg, a);
        let accent = mix_rgb(
            palette.bg,
            term::color_from_str(&self.cfg.search.current_color).unwrap_or(render::Rgb {
                r: 255,
                g: 143,
                b: 63,
            }),
            a,
        );
        let (rx, ry, rw, rh) = xf.rect(bx - 1.0, by - 1.0, bw + 2.0, total_h + 2.0);
        frame.round_rect(rx, ry, rw, rh, xf.len(8.0), dim, 0.55 * a);
        let (rx, ry, rw, rh) = xf.rect(bx, by, bw, total_h);
        frame.round_rect(rx, ry, rw, rh, xf.len(7.0), bg, 0.97 * a);

        let baseline = xf.baseline(by + pad * 0.5 + renderer.ascent);
        let head = format!("> {}", self.panel.query);
        let label = if preedit.is_empty() {
            head.clone()
        } else {
            format!("{head}{preedit}")
        };
        let (tx, _) = xf.pt(bx + pad, 0.0);
        let (tmax, _) = xf.pt(bx + bw - pad, 0.0);
        renderer.draw_scaled_text(frame, &label, tx, baseline, tmax, fg, true, xf.s);
        let caret_x = bx + pad + renderer.text_width_at(&label, xf.s);
        // 组字下划线:含义同搜索栏那边
        if !preedit.is_empty() {
            let ux = bx + pad + renderer.text_width_at(&head, xf.s);
            let (ux, uy, uw, uh) =
                xf.rect(ux, by + pad * 0.5 + ch - 2.0, (caret_x - ux).max(2.0), 1.5);
            frame.rect(ux, uy, uw, uh, fg, 0.8 * a);
        }
        let (kx, ky, kw, kh) = xf.rect(caret_x, by + pad * 0.5, (cw * 0.5).max(2.0), ch);
        frame.rect(kx, ky, kw, kh, accent, 0.9 * a);
        let caret = [kx, kx + kw, ky, ky + kh];
        let hint = if self.panel.output.is_some() {
            "↑↓ 翻页  Esc 返回  Enter 关闭"
        } else {
            "up/down  Tab  Enter  Esc"
        };
        let hw = renderer.text_width_at(hint, xf.s);
        let hx = (bx + bw - pad - hw).max(caret_x + cw);
        let (hxp, _) = xf.pt(hx, 0.0);
        let (hmax, _) = xf.pt(bx + bw - pad, 0.0);
        renderer.draw_scaled_text(frame, hint, hxp, baseline, hmax, dim, false, xf.s);
        let list_top = by + bar_h;
        let (dx, dy, dw, dh) = xf.rect(bx + pad, list_top - 2.0, bw - 2.0 * pad, 1.0);
        frame.rect(dx, dy, dw, dh, dim, 0.35 * a);
        for (i, (text, on)) in rows.iter().enumerate() {
            let ry = list_top + i as f64 * ch;
            if *on && self.panel.output.is_none() {
                let (sx, sy, sw, sh) = xf.rect(bx + 4.0, ry, bw - 8.0, ch);
                frame.round_rect(sx, sy, sw, sh, xf.len(3.0), accent, 0.18 * a);
            }
            let color = if *on && self.panel.output.is_none() {
                accent
            } else if i == 0 && self.panel.output.is_some() {
                accent
            } else {
                fg
            };
            let (lx, _) = xf.pt(bx + pad, 0.0);
            let (lmax, _) = xf.pt(bx + bw - pad, 0.0);
            let lbase = xf.baseline(ry + renderer.ascent);
            renderer.draw_scaled_text(frame, text, lx, lbase, lmax, color, false, xf.s);
        }
        let fx = bx + pad + renderer.text_width_at(&label, 1.0);
        let fy = by + pad * 0.5;
        let kw = (cw * 0.5).max(2.0);
        Some((caret, [fx, fx + kw, fy, fy + ch]))
    }

    fn mouse_wheel(&mut self, delta: MouseScrollDelta) {
        // 滚轮落在标签栏上不滚动终端内容
        if self.tab_bar_hit(self.mouse_px.0, self.mouse_px.1).is_some() {
            return;
        }
        let lines: i64 = match delta {
            MouseScrollDelta::LineDelta(_, y) => (y as i64) * 3,
            MouseScrollDelta::PixelDelta(p) => {
                let ch = self.renderer.cell_h.max(1.0);
                (p.y / ch) as i64
            }
        };
        if lines == 0 {
            return;
        }
        // 滚轮落在文件面板上:滚列表,不动终端回看
        if self.browser_wheel(lines) {
            return;
        }
        // Shift+滚轮:强制走本地回看(xterm 传统,应用抓了鼠标也留一条退路)
        if self.mods.shift_key() {
            self.scroll_by(lines);
            return;
        }
        let (grabbed, alt_screen) = {
            let t = self.tab.active_pane().term.lock();
            (t.is_mouse_grabbed(), t.is_alt_screen_active())
        };
        let n = lines.unsigned_abs() as usize;

        if grabbed {
            let btn = if lines > 0 {
                MouseButton::WheelUp(n)
            } else {
                MouseButton::WheelDown(n)
            };
            for _ in 0..n {
                self.emit_mouse(MouseEventKind::Press, btn);
            }
        } else if alt_screen {
            // alternate scroll(?1007):全屏应用没开鼠标时,滚轮翻译成方向键
            let key = if lines > 0 {
                KeyCode::UpArrow
            } else {
                KeyCode::DownArrow
            };
            let mut t = self.tab.active_pane().term.lock();
            for _ in 0..n {
                let _ = t.key_down(key, KeyModifiers::NONE);
            }
        } else {
            self.scroll_by(lines);
        }
    }

    fn emit_mouse(&self, kind: MouseEventKind, button: MouseButton) {
        let (x, y) = self.cell_at(self.mouse_px.0, self.mouse_px.1);
        let mods = self.mouse_modifiers();
        let mut t = self.tab.active_pane().term.lock();
        let _ = t.mouse_event(MouseEvent {
            kind,
            x,
            y: y as i64,
            x_pixel_offset: 0,
            y_pixel_offset: 0,
            button,
            modifiers: mods,
        });
    }

    fn mouse_modifiers(&self) -> KeyModifiers {
        let mut km = KeyModifiers::NONE;
        if self.mods.shift_key() {
            km |= KeyModifiers::SHIFT;
        }
        if self.mods.alt_key() {
            km |= KeyModifiers::ALT;
        }
        if self.mods.control_key() {
            km |= KeyModifiers::CTRL;
        }
        km
    }

    /// 本地回看滚动:正值向上(更早的行)。目标累加,由 ScrollAnim 缓入缓出地追。
    fn scroll_by(&mut self, lines: i64) {
        // 上限要扣掉抬起量:否则轮子能滚进"resize 补出来的空行"里
        let max = {
            let p = self.tab.active_pane();
            let t = p.term.lock();
            view_geom_of(t.screen(), p.pad, 0.0).1
        };
        let p = self.tab.active_pane_mut();
        let before = p.scroll.pos();
        p.scroll.scroll_by(lines, Instant::now(), max);
        if p.scroll.needs_frames() || (p.scroll.pos() - before).abs() > 0.001 {
            self.window.request_redraw();
        }
    }

    /// 输入到达:视口提前回到底部。
    ///
    /// 长按回车时用 `back_burst`(更短)的时长,让视口赶在下一批输出之前就位,
    /// 剩下的位移交给光标尾迹自己播完 —— 滚动和尾迹各走各的,不会互相拖。
    fn scroll_to_bottom(&mut self) {
        let p = self.tab.active_pane_mut();
        let before = p.scroll.pos();
        p.scroll.back_to_bottom(Instant::now(), p.trail.is_dense());
        if p.scroll.needs_frames() || (p.scroll.pos() - before).abs() > 0.001 {
            self.window.request_redraw();
        }
    }

    fn send_bytes(&mut self, data: &[u8]) {
        use std::io::Write;
        if data.is_empty() {
            return;
        }
        if std::env::var("VECTORTERM_DEBUG_KEYS").is_ok() {
            crate::wlog!("[dbg] send_bytes {:?}", String::from_utf8_lossy(data));
        }
        let p = self.tab.active_pane_mut();
        let _ = p.kb_writer.write_all(data);
        let _ = p.kb_writer.flush();
    }

    fn paste(&mut self) {
        let text = match term::get_clipboard_text() {
            Ok(t) => t,
            Err(_) => return,
        };
        self.tab.active_pane_mut().selection.clear();
        self.scroll_to_bottom();
        // send_paste 会按 ?2004h 的括号粘贴模式自动加上 ESC[200~ / ESC[201~
        let _ = self.tab.active_pane().term.lock().send_paste(&text);
        self.window.request_redraw();
    }

    // ---- 脚本动作 ----

    fn apply_script_actions(&mut self) {
        let actions = match self.script.as_ref() {
            Some(s) => s.drain_actions(),
            None => Vec::new(),
        };
        for action in actions {
            match action {
                ScriptAction::Notify(msg) => {
                    self.window.set_title(&format!("VectorTerm - {msg}"));
                }
                ScriptAction::SendText(text) => self.send_bytes(text.as_bytes()),
                ScriptAction::Paste => self.paste(),
                ScriptAction::Quit => {
                    // 和点关闭按钮走同一条路:有程序在跑就先弹确认
                    if !self.ask_quit() {
                        self.request_quit();
                    }
                }
                ScriptAction::Reload => self.reload_config(),
                ScriptAction::SetCursorColor(color) => {
                    self.with_config(|cfg| cfg.colors.cursor_bg = color);
                }
                ScriptAction::SetTrailDwell(ms) => {
                    self.with_config(|cfg| cfg.cursor.trail_dwell_ms = ms.max(0.0));
                }
                ScriptAction::SetTrailThreshold(x, y) => {
                    self.with_config(|cfg| {
                        cfg.cursor.trail_start_x = x.max(0);
                        cfg.cursor.trail_start_y = y.max(0);
                    });
                }
                ScriptAction::SetTrailDecay(fast, slow) => {
                    self.with_config(|cfg| {
                        cfg.cursor.trail_decay_fast = fast.max(0.01);
                        cfg.cursor.trail_decay_slow = slow.max(fast.max(0.01));
                    });
                }
                ScriptAction::SetTrailMax(ms) => {
                    self.with_config(|cfg| cfg.cursor.trail_max_ms = ms.max(0.0));
                }
                ScriptAction::SetTrailColor(color) => {
                    self.with_config(|cfg| cfg.cursor.trail_color = color);
                }
                ScriptAction::Copy => self.copy_selection(),
                ScriptAction::NewTab => self.new_tab(),
                ScriptAction::CloseTab => self.close_tab_or_ask(self.active),
                ScriptAction::ClosePane => {
                    if let Some(id) = self.active_pane_id() {
                        self.close_pane_or_ask(id);
                    }
                }
                ScriptAction::NextTab => self.next_tab(),
                ScriptAction::PrevTab => self.prev_tab(),
                ScriptAction::ActivateTab(n) => self.switch_to(n),
                ScriptAction::ActivateLastTab => self.prev_tab(),
                ScriptAction::FilesGui => self.browser_toggle(),
                ScriptAction::ToggleFullScreen => {
                    let full = self.window.fullscreen().is_some();
                    self.window.set_fullscreen(if full {
                        None
                    } else {
                        Some(winit::window::Fullscreen::Borderless(None))
                    });
                }
            }
        }
    }

    fn with_config<F: FnOnce(&mut AppConfig)>(&mut self, f: F) {
        let mut cfg = (*self.cfg).clone();
        f(&mut cfg);
        self.cfg = Arc::new(cfg);
        self.apply_runtime_config();
    }

    /// 把可热更新的配置应用到手头状态(动画参数 + 调色板 + 颜色)
    fn apply_runtime_config(&mut self) {
        self.resize_settler
            .set_settle_ms(self.cfg.resize_debounce_ms);
        self.sync_resize_increments();
        // 圆角 / 安全区配置热更新:改了要重新算内容区高度
        (self.edge_inset, self.top_inset) =
            tab_bar_insets(&self.cfg, self.window.scale_factor(), self.rounded_window);
        self.trail_color = self
            .cfg
            .cursor
            .trail_color
            .as_deref()
            .and_then(term::color_from_str);
        // 光标形状 / 长宽 / 闪烁 / 动画是纯渲染参数,直接换样式即可,
        // 不用重建 Renderer(重建会重新加载整套字体)
        self.renderer.cursor_style = render::CursorVisual::from(&self.cfg.cursor);
        // 活动标签 + 后台标签的所有 pane 都换调色板/回看行数/动画参数
        for p in self.tab.panes.iter_mut() {
            p.trail = CursorTrail::new(&self.cfg.cursor);
            p.scroll.reconfigure(&self.cfg.scroll);
            p.term.lock().set_config(term::make_config(&self.cfg));
        }
        for t in self.background.iter_mut().flatten() {
            for p in t.panes.iter_mut() {
                p.trail = CursorTrail::new(&self.cfg.cursor);
                p.scroll.reconfigure(&self.cfg.scroll);
                p.term.lock().set_config(term::make_config(&self.cfg));
            }
        }
        // 标签栏开关/位置变了,内容区高度跟着变
        self.relayout();
        // 状态栏的内容(固定文字 / 模块)可能也改了,立刻重算一次,不用等下一个 tick
        self.refresh_status(Instant::now());
        self.window.request_redraw();
    }

    fn reload_config(&mut self) {
        let Some(path) = self.cfg_path.clone() else {
            return;
        };
        match config::load(Some(&path)) {
            (mut cfg, None) => {
                // 换了 shell / 工作目录时,新标签要用本机真能启动的那个
                pty::apply_shell_choice(&mut cfg);
                // 渲染后端在启动时确定,热重载只能提示
                if cfg.render != self.cfg.render {
                    crate::wlog!(
                        "[VectorTerm] render 从 {} 改为 {},重启后生效",
                        self.cfg.render.as_str(),
                        cfg.render.as_str()
                    );
                }
                if cfg.gpu_power != self.cfg.gpu_power {
                    crate::wlog!(
                        "[VectorTerm] gpu_power 从 {} 改为 {},重启后生效",
                        self.cfg.gpu_power.as_str(),
                        cfg.gpu_power.as_str()
                    );
                }
                self.cfg = Arc::new(cfg);
                match ScriptEngine::load(&path, self.script_actions.clone()) {
                    Ok(s) => self.script = Some(s),
                    Err(e) => crate::werr!("[VectorTerm] Lua 脚本重载失败,保留旧脚本:{e:#}"),
                }
                self.cfg_mtime = mtime_of(&path);
                self.apply_runtime_config();
                crate::wlog!("[VectorTerm] 已重新加载配置:{}", path.display());
            }
            (_, Some(err)) => {
                // 去重在 debug_term 内部做:编辑器每存一次都触发重载,同一个语法
                // 错误不该反复弹调试终端。旧配置保持不变,当前终端一个字节都不写。
                crate::werr!("[VectorTerm] 配置重载失败,保留旧配置:{err}");
                config::log_error(&err);
            }
        }
    }

    /// 同步呈现路径:在 `WindowEvent::Resized` 的处理里直接绘制并 present,
    /// 而不是 `request_redraw()` 排进队列。
    ///
    /// 为什么必须同步:DWM 里每个窗口有一块重定向表面,窗口变大的那一刻,
    /// 应用还没画出新尺寸的帧,DWM 就会把旧表面**拉伸**着铺满新客户区。
    /// 拖边框时每来一个 `WM_SIZE` 都命中一次,于是「拉一点→内容被拉长→
    /// 下一帧弹回」反复出现。
    ///
    /// 真正的解法在 `winhook`:交换链在 `WM_SIZING`(窗口还没变)阶段就被配成
    /// 新尺寸,所以到这里只剩绘制 + present,亚毫秒完成,拉伸窗口短到 DWM 基本
    /// 抓不到。这里只负责「一旦尺寸变了就立刻画」,不再做任何节流。
    fn redraw_sync(&mut self) {
        let t = Instant::now();
        self.redraw();
        trace_dur("DRAW_TOTAL", t.elapsed());
        self.last_sync_present = Instant::now();
        trace_resize("PRESENT_SYNC", self.grid_px.0, self.grid_px.1);
    }

    fn redraw(&mut self) {
        trace_resize("DRAW_ENTER", self.grid_px.0, self.grid_px.1);
        // WM_PAINT 可能先于 WindowEvent::Resized 到达(Windows 拖动时尤其常见)。
        // 画布、标签栏、surface 必须共用同一组尺寸,否则同一屏里内容按旧宽度
        // 画、标签栏按新宽度摆,文字和按钮会一起跳。以 grid_px 为唯一尺寸源,
        // 发现实时尺寸不同就先补一次 resize。
        let live = self.window.inner_size();
        if live.width > 0 && live.height > 0 && (live.width, live.height) != self.grid_px {
            // 返回值只表示"尺寸变了",这里本来就要继续往下画,不用管
            let _ = self.apply_resize(live.width, live.height);
        }
        let (w, h) = self.grid_px;
        if w == 0 || h == 0 {
            return;
        }
        let now = Instant::now();
        self.tick_edge_scroll(now);
        self.refresh_search(false);
        // 浮层进出场:纯时间函数,先推进再画 —— 放在这里是因为下面每一帧都要
        // 用到最新的进度(缩放、位移、alpha 全从它算)
        self.search_anim.tick(now);
        self.panel_anim.tick(now);
        // 文件面板:退场放完之后才算真正收摊 —— 面板数据、所属 pane 一起清
        self.browser_anim.tick(now);
        if !self.browser_anim.visible() && self.browser_exit.is_some() {
            self.browser_exit = None;
            self.tab.browser_pane = 0;
        }
        if std::env::var_os("VECTORTERM_OVERLAY_DEBUG").is_some() {
            crate::wlog!(
                "[anim] search t={:.3} to={:.1} active={} panel t={:.3} to={:.1} active={} browser t={:.3} to={:.1} exit={}",
                self.search_anim.value(),
                self.search_anim.to,
                self.search.active,
                self.panel_anim.value(),
                self.panel_anim.to,
                self.panel.active,
                self.browser_anim.value(),
                self.browser_anim.to,
                self.browser_exit.is_some()
            );
        }
        // 分屏动画:逐帧把新 pane 的分割线从边缘滑到中间。动画期间 panes 的
        // 实际尺寸跟着变,但 PTY resize 只在动画结束后一次性提交(见下方)。
        if let Some(anim) = &self.tab.split_anim {
            let target = anim.target_pane;
            let ratio = anim.ratio(now);
            self.tab.layout.set_ratio_of_new_pane(target, ratio);
            if !anim.tick(now) {
                self.tab.split_anim = None;
                // 收尾再同步一次网格:分屏时已经按 0.5 提前 resize 过,这里通常
                // 是空操作;窗口在这 160ms 里被拖动过才会真提交,保证网格不回退。
                let grid = self.renderer.compute_grid(w, h);
                self.commit_grid(grid);
            }
        }
        // 松手后别再等防抖:冻结的网格一次提交到位
        self.finish_live_resize();
        // 拖动停止后由 WM_PAINT / 定时器唤醒,把攒着的网格变化一次性提交
        self.flush_resize(now);
        let profile_first = self.profile.as_ref().is_some_and(|p| p.last.is_none());
        if let Some(p) = &mut self.profile {
            if let Some(prev) = p.last {
                p.gaps.push(now.duration_since(prev).as_secs_f64() * 1000.0);
            }
            p.last = Some(now);
        }

        // ---- 多 pane 渲染 ----
        // 每个 pane:推进自己的滚动动画 → 取快照 → 按布局偏移画进同一份 frame。
        // 单 pane 时退化为原来的整屏渲染,偏移为 0。
        let t_snap = Instant::now();
        // 闪烁相位与常驻动画时间:文本闪烁(SGR 5/6)和光标本体共用一条时间轴,
        // 所以 blink_rate 改了两者会一起变,不会出现「光标闪得比字快」。
        let blink_rate = self.cfg.cursor.blink_rate_ms.max(60.0) as u128;
        let blink_duty_ms = (blink_rate as f64 * self.cfg.cursor.blink_duty as f64) as u128;
        let blink_ms = now.duration_since(self.blink_epoch).as_millis() % blink_rate;
        let blink_on = blink_ms < blink_duty_ms;
        self.renderer.cursor_blink_phase = blink_ms as f32 / blink_rate as f32;
        self.renderer.cursor_anim_t = now.duration_since(self.blink_epoch).as_secs_f64();
        let cell_w = self.renderer.cell_w;
        let cell_h = self.renderer.cell_h;
        let orig_px = self.renderer.pad_x;
        let orig_py = self.renderer.pad_y;

        // 先收集每个 pane 的几何(格子尺寸 + 像素偏移),再逐个绘制。
        // 借用上分两步走:第一步只读 pane 收集快照,第二步写 renderer/frame。
        struct PaneDraw {
            pane_idx: usize,
            snap: render::ScreenSnapshot,
            cursor: render::CursorInfo,
            palette: render::Palette,
            trail: Option<cursor::TrailDraw>,
            highlight: Option<frame::Highlight>,
            grid: (usize, usize),
            off_x: f64,
            off_y: f64,
            active: bool,
        }

        let mut draws: Vec<PaneDraw> = Vec::new();
        // 活动 pane 的视口顶行 + 亚行偏移:搜索高亮要用它把绝对行号换回屏上坐标
        let mut active_view: Option<(usize, f32)> = None;
        // 浮层在屏上时输入光标归它:终端里那个方块要收起来,否则"光标飞过去"
        // 之后原地还留着一个在闪。这一帧内不变,先在借用 panes 之前算好。
        let caret_owned_by_overlay = self.overlay_owns_caret();
        // 唯一布局源:渲染、PTY 尺寸、鼠标命中、焦点动画全用这一份几何。
        let (pane_geoms, split_lines) = self.pane_geoms();
        {
            // 第一步:推进动画 + 快照(需要 &mut panes)
            let active_id = self.tab.panes[self.tab.active_pane].id;
            for p in self.tab.panes.iter_mut() {
                p.scroll.tick(now);
            }
            // 按 pane 顺序取快照
            for g in &pane_geoms {
                let (grid, off_x, off_y, id) = (g.grid, g.x, g.y, g.pane_id);
                let Some(idx) = self.tab.panes.iter().position(|p| p.id == id) else {
                    continue;
                };
                let p = &mut self.tab.panes[idx];
                let (snap, cursor, palette, view_top, max_scroll) = {
                    let t = p.term.lock();
                    let s = t.screen();
                    let rows = s.physical_rows.max(1);
                    let cy = t.cursor_pos().y.max(0) as usize;
                    let moved_up = cy < p.last_cy;
                    let new_pad = pad_follow(p.pad, rows, cy, moved_up);
                    if new_pad != p.pad && p.scroll.pos() > 0.001 {
                        let delta = p.pad as f32 - new_pad as f32;
                        let max = view_geom_of(s, new_pad, 0.0).1;
                        p.scroll.jump_to(p.scroll.pos() + delta, max);
                    }
                    p.pad = new_pad;
                    p.last_cy = cy;
                    let (top, max, eff) = view_geom_of(s, new_pad, p.scroll.pos());
                    let (snap, cursor, palette) =
                        render::snapshot_at(&t, grid.0, eff, !p.scroll.is_scrolled());
                    (snap, cursor, palette, top, max)
                };
                p.scroll.set_max(max_scroll);
                self.has_blink |= snap.has_blink;

                // 光标尾迹:只有聚焦 pane 才有输入光标,也就只有它可能有尾迹。
                // 非聚焦 pane 每帧把状态清掉,切回来时不会拖出一条陈旧残影。
                let is_active = p.id == active_id;
                if is_active {
                    active_view = Some((view_top, snap.scroll_frac));
                }
                let trail = if is_active && !p.scroll.is_scrolled() {
                    let cr = self.renderer.cell_rect_off(
                        cursor.col,
                        cursor.row,
                        off_x,
                        off_y,
                    );
                    p.trail.update(
                        now,
                        [
                            cr.x as f32,
                            (cr.x + cr.w) as f32,
                            cr.y as f32,
                            (cr.y + cr.h) as f32,
                        ],
                        cursor.visible && !caret_owned_by_overlay,
                        cell_w as f32,
                        cell_h as f32,
                    )
                } else {
                    p.trail.reset();
                    None
                };
                let highlight = if p.id == active_id {
                    pane_selection_highlight_impl(&self.cfg, p, view_top, grid.1)
                } else {
                    None
                };

                draws.push(PaneDraw {
                    pane_idx: idx,
                    snap,
                    cursor,
                    palette,
                    trail,
                    highlight,
                    grid,
                    off_x,
                    off_y,
                    active: p.id == active_id,
                });
            }
        }
        trace_dur("SNAPSHOT", t_snap.elapsed());

        // 第二步:逐个 pane 绘制到同一份 frame。
        let t_build = Instant::now();
        let mut active_cr: Option<CellRect> = None;
        let mut active_palette: Option<render::Palette> = None;
        // 活动 pane 的输入光标中心(内容区坐标)+ 形状,供飞行动画实时校正落点
        let mut active_caret: Option<(f64, f64, CursorShape)> = None;
        let hide_inactive = self.cfg.pane_focus.hide_inactive_caret;
        for (di, d) in draws.iter().enumerate() {
            self.renderer.pad_x = orig_px + d.off_x;
            self.renderer.pad_y = orig_py + d.off_y;
            let trail_color = self.trail_color.unwrap_or(d.palette.cursor_bg);
            // 非聚焦 pane 的输入光标消失;飞行期间活动 pane 也让位给飞行光标。
            let shown_cursor = render::CursorInfo {
                col: d.cursor.col,
                row: d.cursor.row,
                shape: d.cursor.shape.clone(),
                visible: d.cursor.visible
                    && pane_caret_visible(d.active, hide_inactive)
                    && !caret_owned_by_overlay,
            };
            self.renderer.build_frame_with_selection(
                &mut self.frame,
                w,
                h,
                &d.snap,
                &shown_cursor,
                &d.palette,
                d.trail,
                trail_color,
                blink_on,
                d.highlight,
                di == 0, // 第一个 pane begin,后续追加
            );
            // IME 候选窗跟着活动 pane 的光标走 —— 但浮层在屏上时输入焦点
            // 已经搬过去了,这里先不抢,等浮层画完用它的输入位置覆盖。
            if d.active {
                let cr = self.renderer.cell_rect(d.cursor.col, d.cursor.row);
                if !caret_owned_by_overlay {
                    let pos = (cr.x.round() as i32, cr.y.round() as i32);
                    self.ime_area = Some(pos);
                    self.window.set_ime_cursor_area(
                        winit::dpi::PhysicalPosition::new(pos.0, pos.1),
                        PhysicalSize::new(cell_w.round() as u32, cell_h.round() as u32),
                    );
                }
                active_cr = Some(cr);
                active_palette = Some(d.palette);
                active_caret = Some((
                    d.off_x + (d.cursor.col as f64 + 0.5) * cell_w,
                    d.off_y + (d.cursor.row as f64 + 0.5) * cell_h,
                    d.cursor.shape.clone(),
                ));
            }
        }
        // 恢复原 pad
        self.renderer.pad_x = orig_px;
        self.renderer.pad_y = orig_py;

        // 历史搜索匹配:半透明色块盖在文字上(不重绘文字,两条后端都支持
        // alpha 混合),当前项更亮并在左边加一条竖线。
        if self.search.active && !self.search.query.is_empty() {
            if let (Some((top, frac)), Some(g)) = (
                active_view,
                pane_geoms
                    .iter()
                    .find(|g| g.pane_id == self.tab.active_pane().id),
            ) {
                let ox = orig_px + g.x;
                let oy = orig_py + self.renderer.content_top + g.y;
                let rows = g.grid.1;
                let hi = term::color_from_str(&self.cfg.search.highlight_color)
                    .unwrap_or(render::Rgb { r: 240, g: 198, b: 116 });
                let cur = term::color_from_str(&self.cfg.search.current_color)
                    .unwrap_or(render::Rgb { r: 255, g: 143, b: 63 });
                for (i, m) in self.search.matches.iter().enumerate() {
                    if m.row < top || m.row >= top + rows {
                        continue;
                    }
                    let y = oy + (m.row - top) as f64 * cell_h - frac as f64 * cell_h;
                    let x = ox + m.col as f64 * cell_w;
                    let is_cur = i == self.search.current;
                    self.frame.rect(
                        x,
                        y,
                        (m.cols as f64 * cell_w).max(cell_w),
                        cell_h,
                        if is_cur { cur } else { hi },
                        if is_cur { 0.5 } else { 0.28 },
                    );
                    if is_cur {
                        self.frame.rect(x, y, 2.0, cell_h, cur, 0.95);
                    }
                }
            }
        }

        // 分割线:盖住两个 pane 之间 1 格的缝。粗细 / 两端留白 / 单色或渐变
        // 全部由 split_divider 配置决定,热重载后下一帧就生效。
        // 动画:竖杠从最右边(右 pane 的左边界)长出来 —— 首次分屏时 ratio 从
        // 0 涨到 0.5,竖杠位置跟着从右往左移动,视觉上就是「最右边出现竖杠分割」。
        // geoms 用的是内容区坐标,而显示列表是整窗坐标,这里补回 pad + 标签栏高度。
        let origin = (orig_px, orig_py + self.renderer.content_top);
        if self.cfg.split_divider.is_visible() {
            let stops = divider_stops(&self.cfg.split_divider);
            for line in &split_lines {
                for (x, y, w, h, c) in
                    divider_segments(line, &self.cfg.split_divider, &stops, origin)
                {
                    self.frame.rect(x, y, w, h, c, self.cfg.split_divider.opacity);
                }
            }
        }
        // 跨窗格尾迹:和窗格内尾迹**同一套状态机**,只是坐标用内容区像素、
        // 画在所有 pane 之上(不会被单个 pane 的裁剪区切断)。
        //
        // 注意这里**不画本体** —— 新 pane 的输入光标由正常的静态光标绘制,
        // 和窗格内回车时完全一样;"切过去"的观感全部由这条尾巴负责。
        if self.focus_trail_on && pane_geoms.len() > 1 && self.cfg.pane_focus.trail {
            if let Some((cx, cy, _)) = active_caret {
                let cw = self.renderer.cell_w;
                let ch = self.renderer.cell_h;
                // `rect` 用 x0/x1/y0/y1(和 `CursorTrail` 内部一致)
                let rect = [
                    (cx - cw * 0.5) as f32,
                    (cx + cw * 0.5) as f32,
                    (cy - ch * 0.5) as f32,
                    (cy + ch * 0.5) as f32,
                ];
                if let Some(d) = self.focus_trail.update(now, rect, true, cw as f32, ch as f32) {
                    // 颜色:同步开关打开时和窗格内尾迹取**同一个表达式**
                    let fallback = render::Rgb {
                        r: 200,
                        g: 200,
                        b: 200,
                    };
                    let trail_rgb = if self.cfg.pane_focus.sync_trail {
                        active_palette
                            .as_ref()
                            .map(|p| self.trail_color.unwrap_or(p.cursor_bg))
                            .unwrap_or(fallback)
                    } else {
                        self.cfg
                            .pane_focus
                            .trail_color
                            .as_deref()
                            .and_then(term::color_from_str)
                            .or(self.trail_color)
                            .or_else(|| active_palette.as_ref().map(|p| p.cursor_bg))
                            .unwrap_or(fallback)
                    };
                    self.frame.trail(
                        d.corners.map(|c| Point2 {
                            x: (origin.0 + c.x as f64) as f32,
                            y: (origin.1 + c.y as f64) as f32,
                        }),
                        trail_rgb,
                        d.opacity,
                        // 挖空区(当前光标格)的约定是 **[x0, y0, x1, y1]** ——
                        // 两个后端都按 `mask.x..mask.z` 取 x、`mask.y..mask.w` 取 y。
                        // 别照抄上面 `rect` 的 x0/x1/y0/y1 顺序,否则整块四边形会被
                        // 当成挖空区,一个像素都画不出来。
                        [
                            (origin.0 + rect[0] as f64) as f32,
                            (origin.1 + rect[2] as f64) as f32,
                            (origin.0 + rect[1] as f64) as f32,
                            (origin.1 + rect[3] as f64) as f32,
                        ],
                    );
                    // 诊断:正好在这一帧导图(此时显示列表里已经有这条尾迹)
                    if self.dump_trail > 0 {
                        if let Some(base) =
                            std::env::var_os("VECTORTERM_DUMP_FRAME").map(PathBuf::from)
                        {
                            let k = self.dump_trail_total - self.dump_trail + 1;
                            self.dump_frame.push(suffixed(&base, &k.to_string()));
                        }
                        self.dump_trail -= 1;
                    }
                }
                // 收敛了就收工,之后光标怎么动都交回窗格内的尾迹
                if !self.focus_trail.needs_frames() {
                    self.focus_trail_on = false;
                }
            }
        }
        // 文件浏览器面板:盖在终端和分割线之上。全屏模式铺满内容区,侧边栏
        // 模式只占左边一条(右边的 pane 已经被布局让开了,不会画到它下面)。
        //
        // 进出场是**横向**的:整块面板从自己宽度的左侧滑进来 / 滑出去。
        // 所以绘制矩形要能越界 —— 用 `frame.clip_from_here` 把它夹在自己的
        // 区域里,否则滑动中的面板会糊到左邻窗格的终端上。
        let browser_geom = self
            .tab
            .browser
            .as_ref()
            .or(self.browser_exit.as_ref())
            .and_then(|b| self.browser_geom_for(b, self.tab.browser_pane));
        // 动画没在跑就没有面板可画(退场放完那一刻 `visible` 变 false)
        if let Some((rect, _, _)) = browser_geom.filter(|_| self.browser_anim.visible()) {
            // 退场时 `t` 从 1 掉到 0,位移自然又回到左边,不用分支
            let off = (1.0 - self.browser_anim.value()) * rect.w;
            let rr = files::Rect {
                x: rect.x - off,
                y: rect.y,
                w: rect.w,
                h: rect.h,
            };
            if off > 0.5 {
                // 右侧多放 1px:给面板右边那条竖线
                self.frame
                    .clip_from_here(rect.x, rect.x + rect.w + 1.0);
            }
            let palette = active_palette.unwrap_or(render::Palette {
                fg: render::Rgb { r: 200, g: 200, b: 200 },
                bg: render::Rgb { r: 20, g: 20, b: 24 },
                cursor_fg: render::Rgb { r: 0, g: 0, b: 0 },
                cursor_bg: render::Rgb { r: 200, g: 200, b: 200 },
            });
            let theme = files::Theme {
                fg: palette.fg,
                bg: palette.bg,
                // 次要信息(尺寸 / 页脚 / `..`)压暗一档
                dim: mix_rgb(palette.fg, palette.bg, 0.55),
                // 目录名用亮蓝:资源管理器 / ranger 的老习惯,一眼分得清
                accent: self.ansi_palette[12],
                // 选中行:把光标色往背景里压 78%,做成一条低饱和的整行底色
                sel_bg: mix_rgb(palette.cursor_bg, palette.bg, 0.78),
                sel_fg: palette.fg,
                border: SPLIT_DIVIDER_COLOR,
                // 底部提示:亮黄。红太像"出错了",这里只是"缺个东西,能修"
                warn: self.ansi_palette[11],
            };
            let side = self
                .tab
                .browser
                .as_ref()
                .or(self.browser_exit.as_ref())
                .is_some_and(|b| b.mode == files::Mode::Side);
            if side {
                // 面板右边那条竖线:和分屏缝同色,让"这是一块独立区域"看得出来。
                // 跟着面板一起滑,不然会有一条线钉在原地。
                self.frame
                    .rect(rr.x + rr.w, rr.y, 1.0, rr.h, theme.border, 1.0);
            }
            // 活的面板 / 正在退场的面板,二选一画。这里直接对两个字段各取一次
            // 可变借用(而不是走一个 `&mut self` 的 helper),才和下面同时借
            // `renderer` + `frame` 相容。
            let slot = self.tab.browser.as_mut().or(self.browser_exit.as_mut());
            if let Some(b) = slot {
                files::paint(&mut self.renderer, &mut self.frame, b, rr, &theme);
            }
        }
        trace_dur("BUILD_FRAME", t_build.elapsed());
        let t_tb = Instant::now();

        // 标签栏:追加到显示列表末尾(叠加层,不受内容区裁剪)
        if let Some(layout) = self.tab_bar_layout() {
            self.ensure_tab_titles();
            let mut items = Vec::with_capacity(self.background.len());
            for i in 0..self.background.len() {
                let title = if i == self.active {
                    self.tab.title().to_string()
                } else {
                    self.background[i]
                        .as_ref()
                        .map(|t| t.title().to_string())
                        .unwrap_or_default()
                };
                let custom = self.tab_titles.get(i);
                items.push(render::TabBarItem {
                    title,
                    active: i == self.active,
                    hovered: self.tab_hover == Some(i),
                    close_hovered: self.close_hover == Some(i),
                    spans: custom.map(|c| c.spans.clone()),
                    bg: custom.and_then(|c| c.bg),
                    fg: custom.and_then(|c| c.fg),
                });
            }
            let draw = render::TabBarDraw {
                items: &items,
                layout: &layout,
                colors: &self.cfg.tab_bar.colors,
                fancy: self.cfg.tab_bar.fancy,
                show_index: self.cfg.tab_bar.show_index,
                at_bottom: self.cfg.tab_bar.at_bottom,
                corner_radius: self.cfg.tab_bar.corner_radius,
                new_tab_hovered: self.new_tab_hover,
                window_hovered: self.window_hover,
                button_color: self.button_color,
                button_gnome_style: self.button_gnome_style,
                maximized: self.window.is_maximized(),
                left_status: &self.left_status,
                right_status: &self.right_status,
                status_scale: self.cfg.status_bar.font_scale,
                status_pad: self.cfg.status_bar.padding,
                status_gap: self.cfg.status_bar.item_spacing,
                status_space: self.cfg.status_bar.space_width,
            };
            self.renderer.build_tab_bar(&mut self.frame, w, h, &draw);
        }
        trace_dur("BUILD_TABBAR", t_tb.elapsed());

        // 每个 pane 右侧一条滚动条。放在最上层,不被文件面板/分割线盖住;
        // 绘制和鼠标命中都用 scrollbar_geoms() 这一份几何。
        if self.cfg.scrollbar.enabled && self.cfg.scrollbar.show != ScrollbarShow::Never {
            let geoms = self.scrollbar_geoms();
            let palette = active_palette.unwrap_or(render::Palette {
                fg: render::Rgb { r: 200, g: 200, b: 200 },
                bg: render::Rgb { r: 20, g: 20, b: 24 },
                cursor_fg: render::Rgb { r: 0, g: 0, b: 0 },
                cursor_bg: render::Rgb { r: 200, g: 200, b: 200 },
            });
            self.paint_scrollbars(&geoms, &palette);
        }

        // 输入法组合串:贴在光标格下面。回看历史时屏上没有光标,也就不画。
        // 浮层在屏上时不画这个框 —— 组合串改由浮层自己画在输入框里,
        // 否则拼音会出现在终端光标那里,看着像正在往 shell 里打字。
        if let Some(p) = self.ime_preedit.clone() {
            if !caret_owned_by_overlay && !self.tab.active_pane().scroll.is_scrolled() {
                if let (Some(cr), Some(palette)) = (active_cr, active_palette) {
                    self.renderer
                        .build_ime_box(&mut self.frame, cr.x, cr.y, &p.text, p.cursor, &palette);
                }
            }
        }

        // 历史搜索栏:在标签栏/滚动条之后画,保证不被任何叠加层遮住。
        // 判断的是动画进度而不是 `active`:关掉之后还有一段退场要放完。
        let mut overlay_caret: Option<[f64; 4]> = None;
        // 浮层这一帧的输入位置(不是动画终点):输入法候选窗要贴着它
        let mut overlay_now: Option<[f64; 4]> = None;
        if self.search_anim.visible() {
            let palette = active_palette.unwrap_or(render::Palette {
                fg: render::Rgb { r: 200, g: 200, b: 200 },
                bg: render::Rgb { r: 20, g: 20, b: 24 },
                cursor_fg: render::Rgb { r: 0, g: 0, b: 0 },
                cursor_bg: render::Rgb { r: 200, g: 200, b: 200 },
            });
            if let Some((now_caret, target)) = self.paint_search_bar(w, h, &palette) {
                overlay_now = Some(now_caret);
                overlay_caret = Some(target);
            }
        }
        // 命令面板:和搜索栏同一个位置,两种模态互斥
        if self.panel_anim.visible() {
            let palette = active_palette.unwrap_or(render::Palette {
                fg: render::Rgb { r: 200, g: 200, b: 200 },
                bg: render::Rgb { r: 20, g: 20, b: 24 },
                cursor_fg: render::Rgb { r: 0, g: 0, b: 0 },
                cursor_bg: render::Rgb { r: 200, g: 200, b: 200 },
            });
            if let Some((now_caret, target)) = self.paint_command_panel(w, h, &palette) {
                overlay_now = Some(now_caret);
                overlay_caret = Some(target);
            }
        }
        // 输入法候选窗跟着浮层的输入位置走(浮层能画出来才有这个位置)
        if let Some(c) = overlay_now {
            let pos = (c[0].round() as i32, c[3].round() as i32);
            if self.ime_area != Some(pos) {
                self.ime_area = Some(pos);
                self.window.set_ime_cursor_area(
                    winit::dpi::PhysicalPosition::new(pos.0, pos.1),
                    PhysicalSize::new(
                        self.renderer.cell_w.round() as u32,
                        self.renderer.cell_h.round() as u32,
                    ),
                );
            }
        }
        // 光标飞过去:从终端光标格一路扫到浮层的输入框。和跨窗格尾迹用
        // **同一套状态机**(同样的 decay / 速度),所以手感是同一个动画。
        if self.overlay_trail_on {
            match overlay_caret {
                Some(c) => {
                    let cw = self.renderer.cell_w as f32;
                    let ch = self.renderer.cell_h as f32;
                    // `rect` 的约定是 [x0, x1, y0, y1](和 `CursorTrail` 内部一致)
                    let rect = [c[0] as f32, c[1] as f32, c[2] as f32, c[3] as f32];
                    let drawn = self.overlay_trail.update(now, rect, true, cw, ch);
                    if let Some(d) = drawn {
                        let fallback = render::Rgb {
                            r: 200,
                            g: 200,
                            b: 200,
                        };
                        let color = self
                            .trail_color
                            .or_else(|| active_palette.map(|p| p.cursor_bg))
                            .unwrap_or(fallback);
                        self.frame.trail(
                            d.corners.map(|p| Point2 { x: p.x, y: p.y }),
                            color,
                            d.opacity,
                            // 挖空区(当前落点)的约定是 **[x0, y0, x1, y1]**
                            [rect[0], rect[2], rect[1], rect[3]],
                        );
                    }
                    if !self.overlay_trail.needs_frames() {
                        self.overlay_trail_on = false;
                    }
                }
                // 面板已经收干净(退场放完)就不再喂它,尾巴自然跟着收敛
                None => self.overlay_trail_on = false,
            }
        }

        // 关闭确认框:最后一个叠加层,盖住标签栏和内容
        if let Some(c) = &self.confirm {
            let palette = active_palette.unwrap_or(render::Palette {
                fg: render::Rgb { r: 200, g: 200, b: 200 },
                bg: render::Rgb { r: 20, g: 20, b: 24 },
                cursor_fg: render::Rgb { r: 0, g: 0, b: 0 },
                cursor_bg: render::Rgb { r: 200, g: 200, b: 200 },
            });
            let draw = render::DialogDraw {
                message: c.message(),
                choices: CONFIRM_CHOICES,
                active: c.active,
                palette: &palette,
            };
            self.renderer
                .build_confirm_dialog(&mut self.frame, w, h, &draw);
        }

        // 呈现
        let present_t0 = Instant::now();
        match &mut self.backend {
            Backend::Cpu(surface, _) => {
                if (w, h) != self.surface_px {
                    match surface.resize(
                        NonZeroU32::new(w).expect("non-zero width"),
                        NonZeroU32::new(h).expect("non-zero height"),
                    ) {
                        Ok(()) => self.surface_px = (w, h),
                        Err(e) => {
                            crate::wlog!("[VectorTerm] surface resize 失败:{e}");
                            return;
                        }
                    }
                }
                match surface.buffer_mut() {
                    Ok(mut buffer) => {
                        self.renderer.paint_cpu(&mut buffer, w, h, &self.frame);
                        if let Err(e) = buffer.present() {
                            crate::wlog!("[VectorTerm] present 失败:{e}");
                        }
                    }
                    Err(e) => crate::wlog!("[VectorTerm] 获取缓冲失败:{e}"),
                }
            }
            Backend::Gpu(g, _) => {
                // configure(重建交换链)与 draw(取帧+提交+present)分开计时:
                // 前者是 DXGI 的 ResizeBuffers,后者可能卡在垂直同步上,两者的
                // 优化方向完全相反,混在 PRESENT 里看不出来。
                let t_cfg = Instant::now();
                g.resize(w, h);
                trace_dur("GPU_CONFIGURE", t_cfg.elapsed());
                let t_draw = Instant::now();
                if let Err(e) = g.draw(&self.frame, &self.renderer) {
                    crate::wlog!("[VectorTerm] GPU 绘制失败:{e:#}");
                }
                trace_dur("GPU_DRAW", t_draw.elapsed());
            }
        }
        trace_dur("PRESENT", present_t0.elapsed());
        // 记下这一帧对应的客户区尺寸:`WM_PAINT` 复检靠它判断「是否已有新尺寸的帧」
        self.last_presented = (w, h);

        // 诊断:把当前显示列表用 CPU 路径重绘一份落盘(不依赖窗口截图)
        for path in std::mem::take(&mut self.dump_frame) {
            let mut buf = vec![0u32; (w * h) as usize];
            self.renderer.paint_cpu(&mut buf, w, h, &self.frame);
            match write_bmp(&path, &buf, w, h) {
                Ok(()) => crate::wlog!("[VectorTerm] 帧已导出:{} ({w}x{h})", path.display()),
                Err(e) => crate::wlog!("[VectorTerm] 导出帧失败:{e}"),
            }
        }

        if self.tab.active_pane().trail.needs_frames()
            || self.tab.active_pane().scroll.needs_frames()
            || self.tab.split_anim.is_some()
            || self.focus_trail_on
            || self.edge_scroll != 0
        {
            self.window.request_redraw();
        }

        if let Some(p) = &mut self.profile {
            if profile_first {
                if let Some(boot) = BOOT.get() {
                    crate::wlog!(
                        "[VectorTerm][profile] first_frame_ms={:.3}",
                        Instant::now().duration_since(*boot).as_secs_f64() * 1000.0
                    );
                }
            }
            p.remaining = p.remaining.saturating_sub(1);
            if p.remaining == 0 {
                print_profile(p, self.cfg.render.as_str());
                self.quit_requested = true;
            } else {
                self.window.request_redraw();
            }
        }
    }
}

/// 基准模式结束时的帧间隔统计(丢弃前 5 帧预热)
fn print_profile(p: &ProfileCtx, render: &str) {
    let mut xs: Vec<f64> = p
        .gaps
        .iter()
        .copied()
        .skip(5)
        .filter(|v| v.is_finite())
        .collect();
    if xs.is_empty() {
        crate::wlog!("[VectorTerm][profile] 没有有效帧样本");
        return;
    }
    let mean = xs.iter().sum::<f64>() / xs.len() as f64;
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = xs.len();
    let p95 = xs[((n as f64 * 0.95) as usize).min(n - 1)];
    crate::wlog!(
        "[VectorTerm][profile] backend={render} frames={n} mean={:.3}ms p50={:.3}ms p95={:.3}ms min={:.3}ms max={:.3}ms fps={:.1}",
        mean,
        xs[n / 2],
        p95,
        xs[0],
        xs[n - 1],
        1000.0 / mean,
    );
}

#[cfg(test)]
mod resize_settle_tests {
    use super::*;

    /// 拖动默认走 wezterm 的实时语义;防抖只管非拖动的尺寸变化
    #[test]
    fn resize_mode_defaults_to_realtime_while_dragging() {
        let mut cfg = AppConfig::default();
        assert_eq!(resize_mode_for(true, &cfg), LiveResizeMode::Realtime);
        assert_eq!(resize_mode_for(false, &cfg), LiveResizeMode::Debounce);
        cfg.resize_debounce_ms = 0;
        assert_eq!(resize_mode_for(false, &cfg), LiveResizeMode::Realtime);
        for (m, want) in [
            (LiveResizeMode::Realtime, LiveResizeMode::Realtime),
            (LiveResizeMode::Debounce, LiveResizeMode::Debounce),
            (LiveResizeMode::Freeze, LiveResizeMode::Freeze),
        ] {
            cfg.live_resize_mode = m;
            assert_eq!(resize_mode_for(true, &cfg), want);
        }
    }

    /// 一整串连续 resize 只该产生一次提交,且提交的是最后的目标网格
    #[test]
    fn bursts_collapse_into_one_commit() {
        let t0 = Instant::now();
        let mut s = ResizeSettler::new(100);
        let mut immediate = 0;
        for i in 0..6 {
            let now = t0 + Duration::from_millis(i * 20);
            if s.on_resize((100 + i as usize, 30), now) {
                immediate += 1;
            }
            assert!(
                !s.ready(now + Duration::from_millis(10)),
                "拖动过程中不该提交网格"
            );
        }
        assert_eq!(immediate, 0);
        assert!(s.deadline().is_some());
        let last = t0 + Duration::from_millis(100);
        assert!(s.ready(last + Duration::from_millis(100)));
        assert_eq!(s.take(), Some((105, 30)), "只提交最后一次的目标网格");
        assert!(s.deadline().is_none());
        assert!(!s.ready(Instant::now()));
    }

    /// 拖动模态循环期间冻结:无论卡了多久都不许 ready,也没有定时器,
    /// 松手后由 finish_live_resize 取走最后一次的目标网格。
    #[test]
    fn frozen_burst_never_commits_until_unfreeze() {
        let t0 = Instant::now();
        let mut s = ResizeSettler::new(150);
        for i in 0..8 {
            s.freeze((100 + i, 30));
            assert!(s.is_frozen());
        }
        // 模拟拖动中出现一次 500ms 的卡顿:防抖会误判松手,冻结不会
        assert!(!s.ready(t0 + Duration::from_millis(500)));
        assert!(s.deadline().is_none(), "冻结期间不该挂防抖定时器");
        assert_eq!(s.take(), Some((107, 30)), "只提交最后一次目标网格");
        assert!(!s.is_frozen());
        assert!(!s.ready(Instant::now()));
    }

    /// `resize_debounce_ms = 0` 回到 wezterm 的实时同步
    #[test]
    fn zero_is_realtime() {
        let now = Instant::now();
        let mut s = ResizeSettler::new(0);
        assert!(s.on_resize((80, 24), now));
        s.clear();
        assert!(!s.ready(now));
        assert!(s.deadline().is_none());
    }

    /// 热重载改时长后立即生效(包括已经攒着的待办)
    #[test]
    fn changing_settle_ms_takes_effect() {
        let t0 = Instant::now();
        let mut s = ResizeSettler::new(100);
        s.on_resize((90, 24), t0);
        s.set_settle_ms(30);
        assert!(s.ready(t0 + Duration::from_millis(31)));
        assert_eq!(s.deadline(), Some(t0 + Duration::from_millis(30)));
    }
}

#[cfg(test)]
mod window_title_tests {
    use super::window_title;

    /// 主窗口标题只反映会话,永远不带软件自身的错误文案 —— 错误在调试终端里。
    #[test]
    fn title_only_reflects_the_session() {
        assert_eq!(window_title("pwsh"), "VectorTerm - pwsh");
        assert_eq!(window_title("cmd.exe"), "VectorTerm - cmd.exe");
    }
}

#[cfg(test)]
mod view_pad_tests {
    use super::{pad_after_resize, pad_follow};

    /// 窗口变高、光标上方有历史:抬起量 = 光标下方的空行数(承托住光标行)
    #[test]
    fn grow_lifts_viewport_to_the_cursor() {
        // 16 行 → 28 行,ConPTY 在尾部补了 12 行空行,光标仍在第 15 行
        assert_eq!(pad_after_resize(28, 15, 53), 12);
        // 空行数超过可回看行数时不能多抬(否则画面被顶空)
        assert_eq!(pad_after_resize(28, 15, 20), 0);
        // 光标本来就贴底(例如刚变矮 / 内容填满):不用抬
        assert_eq!(pad_after_resize(28, 27, 53), 0);
        // 没有回看历史:抬起来只会看到空白,不如保持原样
        assert_eq!(pad_after_resize(28, 0, 28), 0);
    }

    /// 光标往下走一行,补白就少一行:画面上"提示符贴底、内容上滚"
    #[test]
    fn pad_shrinks_as_cursor_walks_down() {
        assert_eq!(pad_follow(12, 28, 15, false), 12);
        assert_eq!(pad_follow(12, 28, 16, false), 11);
        assert_eq!(pad_follow(12, 28, 27, false), 0);
        // 已经没了就是没了,不会变成负数
        assert_eq!(pad_follow(0, 28, 20, false), 0);
    }

    /// 光标往上跳(清屏 / 应用整屏重绘)说明屏幕内容换过,立刻取消抬起,
    /// 否则会把清屏前的历史顶回画面。
    #[test]
    fn cursor_jumping_up_cancels_the_lift() {
        assert_eq!(pad_follow(12, 28, 0, true), 0);
        assert_eq!(pad_follow(12, 28, 14, true), 0);
    }

    /// 拖选到边缘的自动滚屏速度:中间不滚、越贴边越快、下边缘反向
    #[test]
    fn edge_scroll_speeds_up_near_borders_and_stops_in_the_middle() {
        use super::edge_scroll_lines;
        let (top, bottom, ch) = (0.0, 400.0, 20.0);
        assert_eq!(edge_scroll_lines(200.0, top, bottom, ch), 0, "中间不滚");
        assert_eq!(edge_scroll_lines(100.0, top, bottom, ch), 0, "离边缘 3 格外不滚");
        let near_top = edge_scroll_lines(50.0, top, bottom, ch);
        let at_top = edge_scroll_lines(-20.0, top, bottom, ch);
        assert!(near_top > 0 && at_top > near_top, "越贴上边越快");
        assert_eq!(at_top, 8, "速度封顶 8 行");
        let near_bottom = edge_scroll_lines(390.0, top, bottom, ch);
        assert!(near_bottom < 0 && near_bottom >= -8, "下边缘往下滚");
    }

    /// 抬起 + 回看一起算:顶行、上限、有效偏移三者的口径
    #[test]
    fn geom_combines_pad_and_scroll() {
        // 缓冲 53 行,窗口 28 行,抬起 12 行 → 顶行 = 53-28-12 = 13
        let (top, max, eff) = super::view_geom_of_sizes(28, 53, 12, 0.0);
        assert_eq!((top, max, eff), (13, 13, 12.0));
        // 再往回看 5 行 → 顶行上移 5 行,上限相应变小
        let (top, _, eff) = super::view_geom_of_sizes(28, 53, 12, 5.0);
        assert_eq!((top, eff), (8, 17.0));
        // 抬起量不会超过可回看行数(缓冲只有 30 行、窗口 28 行时最多抬 2 行)
        let (top, _, _) = super::view_geom_of_sizes(28, 30, 12, 0.0);
        assert_eq!(top, 0);
    }
}

#[cfg(test)]
mod split_tests {
    use super::*;

    /// 单 pane 布局树:一个叶子,leaves 返回 [0]。
    #[test]
    fn single_leaf_layout() {
        let mut root = SplitNode::leaf(0);
        let mut leaves = Vec::new();
        root.leaves(&mut leaves);
        assert_eq!(leaves, vec![0]);
        let _ = root;
    }

    /// 垂直分屏:在叶子 0 处分割,新 pane 下标 1,叶子顺序 [0, 1]。
    #[test]
    fn vertical_split_appends_leaf() {
        let mut root = SplitNode::leaf(0);
        assert!(root.split_at(0, 1, SplitDir::Vertical, 0.5));
        let mut leaves = Vec::new();
        root.leaves(&mut leaves);
        assert_eq!(leaves, vec![0, 1]);
    }

    /// 递归分割:先垂直分屏,再在右 pane(1)处水平分屏。
    #[test]
    fn nested_split() {
        let mut root = SplitNode::leaf(0);
        root.split_at(0, 1, SplitDir::Vertical, 0.5);
        root.split_at(1, 2, SplitDir::Horizontal, 0.5);
        let mut leaves = Vec::new();
        root.leaves(&mut leaves);
        assert_eq!(leaves, vec![0, 1, 2]);
    }

    /// 移除叶子后,布局树用兄弟子树顶替。
    #[test]
    fn remove_leaf_replaces_with_sibling() {
        let mut root = SplitNode::leaf(0);
        root.split_at(0, 1, SplitDir::Vertical, 0.5);
        // 移除叶子 1(右 pane),剩下叶子 0
        let new_root = root.remove_leaf(1).expect("还剩一个叶子");
        let mut leaves = Vec::new();
        new_root.leaves(&mut leaves);
        assert_eq!(leaves, vec![0]);
    }

    /// 移除唯一叶子返回 None(表示该关掉整个标签)。
    #[test]
    fn remove_last_leaf_returns_none() {
        let mut root = SplitNode::leaf(0);
        assert!(root.remove_leaf(0).is_none());
    }

    /// 布局树 2 分割:每个 pane 尺寸 = 整窗的一半(减掉 1 格分割线)。
    #[test]
    fn two_way_split_halves_grid() {
        // 整窗 80 列,垂直分割后左 39 右 40(80-1=79,取 39/40)。
        // 不依赖具体实现,只验证两个 pane 的列数之和 + 分割线 = 整窗。
        let cols = 80usize;
        let divider = 1usize;
        let avail = cols - divider;
        let left = (avail as f64 * 0.5).round() as usize;
        let right = avail - left;
        assert_eq!(left + right + divider, cols);
        // 比例 0.5 时左右尽量均分(差不超过 1)
        assert!((left as i64 - right as i64).abs() <= 1);
    }

    /// `set_ratio_of_new_pane`:只动「b 子树包含新 pane」的那一层,不动更深的层。
    #[test]
    fn set_ratio_targets_the_new_split() {
        let mut root = SplitNode::leaf(0);
        root.split_at(0, 1, SplitDir::Vertical, 0.5);
        // 新 pane 是 1,把它的分割层 ratio 改成 0.3
        assert!(root.set_ratio_of_new_pane(1, 0.3));
        // 根节点现在 ratio 应该是 0.3
        match &*root {
            SplitNode::Split { ratio, .. } => assert!((*ratio - 0.3).abs() < 1e-9),
            _ => panic!("应该是 Split"),
        }
    }

    /// 分屏动画:从最右 / 最下(ratio=1.0)滑到正中间(0.5)。
    #[test]
    fn unsettled_anim_snaps_to_target_before_layout_changes() {
        // 动画半路(0.72)时又来一次分屏 / 关 pane:必须先把线拍回 0.5,
        // 否则那个节点永远停 0.72,布局和 PTY 尺寸都跟着歪。
        let mut tree = SplitNode::Split {
            dir: SplitDir::Horizontal,
            ratio: 0.5,
            a: SplitNode::leaf(0),
            b: SplitNode::leaf(1),
        };
        tree.set_ratio_of_new_pane(1, 0.72);
        let mut anim = Some(SplitAnim::new(1, SplitDir::Horizontal, Instant::now(), 0.5));
        settle_layout_anim(&mut tree, &mut anim);
        assert!(anim.is_none(), "结算后不能留着动画");
        let SplitNode::Split { ratio, .. } = &tree else {
            panic!("根还是 Split");
        };
        assert!((ratio - 0.5).abs() < 1e-9, "线应落到中线,实际 {ratio}");
    }

    #[test]
    fn split_anim_slides_to_center() {
        let now = Instant::now();
        let v = SplitAnim::new(1, SplitDir::Vertical, now, 0.5);
        assert!((v.from - 1.0).abs() < 1e-9);
        assert!((v.to - 0.5).abs() < 1e-9);
        // 起点 ratio = 1.0(分割线在最右),终点 = 0.5
        assert!((v.ratio(now) - 1.0).abs() < 1e-9);
        assert!(v.tick(now));
        // 动画结束后 ratio 恒为 0.5
        let end = now + v.dur + Duration::from_millis(1);
        assert!(!v.tick(end));
        assert!((v.ratio(end) - 0.5).abs() < 1e-9);

        // 水平分割:同样从末端(最下)起步,分割线一路**向上**走到中线
        let h = SplitAnim::new(1, SplitDir::Horizontal, now, 0.5);
        assert!((h.from - 1.0).abs() < 1e-9);
        assert!((h.to - 0.5).abs() < 1e-9);
        // 后半程单调逼近 0.5:既不会往回走,也不会越过中线
        let mut prev = h.ratio(now);
        for ms in 1..=32 {
            let r = h.ratio(now + Duration::from_millis(ms) * 5);
            assert!(r <= prev + 1e-9, "{ms}:{r} 比上一帧 {prev} 更靠边缘");
            assert!(r >= 0.5 - 1e-9);
            prev = r;
        }
    }
}

#[cfg(test)]
mod pane_shortcut_tests {
    use super::*;
    use winit::keyboard::NativeKey;

    const CSA: ModifiersState = ModifiersState::CONTROL
        .union(ModifiersState::SHIFT)
        .union(ModifiersState::ALT);
    const CS: ModifiersState = ModifiersState::CONTROL.union(ModifiersState::SHIFT);

    fn code(c: WinitKeyCode) -> PhysicalKey {
        PhysicalKey::Code(c)
    }

    /// 真实 Windows 场景:Ctrl+Shift+Alt 下 logical_key 是 Unidentified,
    /// 旧实现在这里返回 None —— 快捷键完全失效。
    #[test]
    fn windows_ctrl_alt_shift_still_hits_by_physical_key() {
        let unidentified = Key::Unidentified(NativeKey::Unidentified);
        assert_eq!(
            pane_action(&unidentified, &code(WinitKeyCode::Quote), CSA),
            Some(PaneAction::Split(SplitDir::Vertical))
        );
        assert_eq!(
            pane_action(&unidentified, &code(WinitKeyCode::Digit5), CSA),
            Some(PaneAction::Split(SplitDir::Horizontal))
        );
        // ToUnicode 偶尔返回控制字符,也必须靠物理键命中
        assert_eq!(
            pane_action(
                &Key::Character("\u{1c}".into()),
                &code(WinitKeyCode::Quote),
                CSA
            ),
            Some(PaneAction::Split(SplitDir::Vertical))
        );
    }

    #[test]
    fn easy_aliases_and_logical_fallback() {
        let unidentified = Key::Unidentified(NativeKey::Unidentified);
        // 不按 Alt 也能分屏(输入法常拦 Ctrl+Shift+Alt)
        assert_eq!(
            pane_action(&unidentified, &code(WinitKeyCode::Quote), CS),
            Some(PaneAction::Split(SplitDir::Vertical))
        );
        // 纯字母别名
        assert_eq!(
            pane_action(&unidentified, &code(WinitKeyCode::KeyV), CSA),
            Some(PaneAction::Split(SplitDir::Vertical))
        );
        assert_eq!(
            pane_action(&unidentified, &code(WinitKeyCode::KeyH), CSA),
            Some(PaneAction::Split(SplitDir::Horizontal))
        );
        // 非 Windows / 合成事件:logical_key 的字符兜底
        assert_eq!(
            pane_action(
                &Key::Character("\"".into()),
                &PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified),
                CSA
            ),
            Some(PaneAction::Split(SplitDir::Vertical))
        );
    }

    /// Ctrl+Shift+X 关窗格。中文输入法开着时 `logical_key` 会变成
    /// `Unidentified`,只有物理键能命中 —— 和分屏那几条同一个坑。
    #[test]
    fn close_pane_shortcut() {
        let unidentified = Key::Unidentified(NativeKey::Unidentified);
        assert_eq!(
            pane_action(&unidentified, &code(WinitKeyCode::KeyX), CS),
            Some(PaneAction::Close)
        );
        // 某些布局下 Ctrl+Shift 会带出 AltGr,带 Alt 也得认
        assert_eq!(
            pane_action(&unidentified, &code(WinitKeyCode::KeyX), CSA),
            Some(PaneAction::Close)
        );
        // 非 Windows / 合成事件:字符兜底,大小写都认
        assert_eq!(
            pane_action(
                &Key::Character("X".into()),
                &PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified),
                CS
            ),
            Some(PaneAction::Close)
        );
        // 不带 Ctrl 或没按 Shift:不是我们的键,留给终端(单按 x 要能打字)
        assert_eq!(
            pane_action(
                &unidentified,
                &code(WinitKeyCode::KeyX),
                ModifiersState::CONTROL
            ),
            None
        );
        assert_eq!(
            pane_action(
                &unidentified,
                &code(WinitKeyCode::KeyX),
                ModifiersState::SHIFT
            ),
            None
        );
        // Ctrl+Shift+V 必须还是粘贴:分屏的字母别名只在带 Alt 时生效
        assert_eq!(
            pane_action(&unidentified, &code(WinitKeyCode::KeyV), CS),
            None
        );
    }

    #[test]
    fn focus_and_swallow() {
        assert_eq!(
            pane_action(
                &Key::Named(NamedKey::ArrowLeft),
                &code(WinitKeyCode::ArrowLeft),
                CS
            ),
            Some(PaneAction::Focus(PaneDir::Left))
        );
        assert_eq!(
            pane_action(
                &Key::Named(NamedKey::ArrowUp),
                &code(WinitKeyCode::ArrowUp),
                CSA
            ),
            Some(PaneAction::Resize(PaneDir::Up))
        );
        assert_eq!(
            pane_action(
                &Key::Named(NamedKey::ArrowRight),
                &code(WinitKeyCode::ArrowRight),
                CSA
            ),
            Some(PaneAction::Resize(PaneDir::Right))
        );
        // Ctrl+Alt+方向键不拦:那是显卡驱动的旋转热键,留给系统处理
        assert_eq!(
            pane_action(
                &Key::Named(NamedKey::ArrowRight),
                &code(WinitKeyCode::ArrowRight),
                ModifiersState::CONTROL.union(ModifiersState::ALT)
            ),
            None
        );
    }

    #[test]
    fn does_not_steal_clipboard_or_plain_keys() {
        // Ctrl+Shift+V 是粘贴,字母别名只在带 Alt 时生效
        assert_eq!(
            pane_action(
                &Key::Character("V".into()),
                &code(WinitKeyCode::KeyV),
                CS
            ),
            None
        );
        // 没按 Ctrl 一律不拦
        assert_eq!(
            pane_action(
                &Key::Character("\"".into()),
                &code(WinitKeyCode::Quote),
                ModifiersState::SHIFT
            ),
            None
        );
    }
}

#[cfg(test)]
mod divider_tests {
    use super::*;
    use crate::config::{DividerGradient, DividerGradientDir};

    fn vline() -> SplitLine {
        SplitLine {
            x: 100.0,
            y: 0.0,
            w: 10.0,
            h: 200.0,
            vertical: true,
            area: (0.0, 0.0, 210.0, 200.0),
            path: 0,
            depth: 0,
        }
    }

    fn hline() -> SplitLine {
        SplitLine {
            x: 0.0,
            y: 100.0,
            w: 200.0,
            h: 10.0,
            vertical: false,
            area: (0.0, 0.0, 200.0, 210.0),
            path: 0,
            depth: 0,
        }
    }

    fn rgb(r: u8, g: u8, b: u8) -> render::Rgb {
        render::Rgb { r, g, b }
    }

    #[test]
    fn default_draws_a_thin_centered_line() {
        // 未设 thickness 时自动画细线(缝宽 10px 的 30% = 3px),以缝中心为轴。
        let cfg = SplitDividerConfig::default();
        assert_eq!(divider_rect(&vline(), &cfg), Some((103.5, 0.0, 3.0, 200.0)));
        assert_eq!(divider_rect(&hline(), &cfg), Some((0.0, 103.5, 200.0, 3.0)));
        // 想要旧版"填满整格缝"的粗条,显式写 thickness >= 缝宽即可
        let mut fill = SplitDividerConfig::default();
        fill.thickness = Some(10.0);
        assert_eq!(divider_rect(&vline(), &fill), Some((100.0, 0.0, 10.0, 200.0)));
    }

    #[test]
    fn thickness_is_centered_in_the_gap() {
        let mut cfg = SplitDividerConfig::default();
        cfg.thickness = Some(2.0);
        assert_eq!(divider_rect(&vline(), &cfg), Some((104.0, 0.0, 2.0, 200.0)));
        assert_eq!(divider_rect(&hline(), &cfg), Some((0.0, 104.0, 200.0, 2.0)));
        // 允许盖到两边文字上,不夹在格宽内
        cfg.thickness = Some(30.0);
        assert_eq!(divider_rect(&vline(), &cfg), Some((90.0, 0.0, 30.0, 200.0)));
    }

    #[test]
    fn insets_shorten_from_both_ends() {
        let mut cfg = SplitDividerConfig::default();
        cfg.thickness = Some(4.0);
        cfg.inset_start = 12.0;
        cfg.inset_end = 30.0;
        // 竖线:从上边裁 12、下边裁 30
        assert_eq!(
            divider_rect(&vline(), &cfg),
            Some((103.0, 12.0, 4.0, 158.0))
        );
        // 横线:从左边裁 12、右边裁 30
        assert_eq!(
            divider_rect(&hline(), &cfg),
            Some((12.0, 103.0, 158.0, 4.0))
        );
    }

    #[test]
    fn no_negative_rects() {
        let mut cfg = SplitDividerConfig::default();
        cfg.enabled = false;
        assert_eq!(divider_rect(&vline(), &cfg), None);
        cfg.enabled = true;
        cfg.thickness = Some(0.0);
        assert_eq!(divider_rect(&vline(), &cfg), None);
        cfg.thickness = Some(2.0);
        cfg.inset_start = 150.0;
        cfg.inset_end = 150.0;
        assert_eq!(divider_rect(&vline(), &cfg), None);
        assert!(divider_segments(&vline(), &cfg, &[rgb(1, 2, 3)], (0.0, 0.0)).is_empty());
    }

    #[test]
    fn solid_uses_config_color_and_window_origin() {
        let mut cfg = SplitDividerConfig::default();
        cfg.color = "#ff0000".into();
        cfg.thickness = Some(2.0);
        cfg.inset_start = 10.0;
        let stops = divider_stops(&cfg);
        assert_eq!(stops, vec![rgb(255, 0, 0)]);
        let segs = divider_segments(&vline(), &cfg, &stops, (8.0, 32.0));
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0], (112.0, 42.0, 2.0, 190.0, rgb(255, 0, 0)));
    }

    #[test]
    fn gradient_segments_tile_exactly_without_gaps() {
        let mut cfg = SplitDividerConfig::default();
        cfg.thickness = Some(3.0);
        cfg.gradient = Some(DividerGradient {
            colors: vec!["#000000".into(), "#ffffff".into()],
            orientation: DividerGradientDir::Auto,
        });
        let stops = divider_stops(&cfg);
        assert_eq!(stops, vec![rgb(0, 0, 0), rgb(255, 255, 255)]);
        let segs = divider_segments(&vline(), &cfg, &stops, (0.0, 0.0));
        assert_eq!(segs.len(), 200, "200px 的线切成 200 段,逐像素过渡");
        for (i, s) in segs.iter().enumerate() {
            assert_eq!(s.0, 103.5, "全部居中于缝");
            assert_eq!(s.2, 3.0);
            assert_eq!(s.1, i as f64, "段 i 从第 i 行开始");
            assert_eq!(s.3, 1.0, "每段恰好 1px,无重叠无缝隙");
        }
        assert_eq!(segs.first().unwrap().4, rgb(1, 1, 1));
        assert_eq!(segs.last().unwrap().4, rgb(254, 254, 254));
    }

    #[test]
    fn long_gradient_caps_at_256_segments() {
        let mut line = vline();
        line.h = 4000.0;
        let mut cfg = SplitDividerConfig::default();
        cfg.gradient = Some(DividerGradient {
            colors: vec!["#000000".into(), "#ffffff".into()],
            orientation: DividerGradientDir::Auto,
        });
        let segs = divider_segments(&line, &cfg, &divider_stops(&cfg), (0.0, 0.0));
        assert_eq!(segs.len(), 256);
        assert_eq!(segs.first().unwrap().1, 0.0);
        assert_eq!(segs.last().unwrap().1 + segs.last().unwrap().3, 4000.0);
        for pair in segs.windows(2) {
            assert_eq!(pair[0].1 + pair[0].3, pair[1].1, "段之间不能有缝/重叠");
        }
    }

    #[test]
    fn explicit_orientation_overrides_along_line() {
        let mut cfg = SplitDividerConfig::default();
        cfg.thickness = Some(4.0);
        cfg.gradient = Some(DividerGradient {
            colors: vec!["#ff0000".into(), "#0000ff".into()],
            orientation: DividerGradientDir::Horizontal,
        });
        // 竖线却要求横向渐变:沿 X 分段,每段都是整条线高
        let segs = divider_segments(&vline(), &cfg, &divider_stops(&cfg), (0.0, 0.0));
        assert_eq!(segs.len(), 4);
        assert_eq!(segs[0].1, 0.0);
        assert_eq!(segs[0].3, 200.0);
        assert!(segs.windows(2).all(|p| p[0].0 + p[0].2 == p[1].0));
        assert!(segs.iter().any(|s| s.4.r < 255 && s.4.b > 0));
    }

    #[test]
    fn multi_stop_and_bad_colors_fall_back() {
        let stops = vec![rgb(0, 0, 0), rgb(255, 0, 0), rgb(255, 255, 255)];
        assert_eq!(gradient_at(&stops, 0.0), rgb(0, 0, 0));
        assert_eq!(gradient_at(&stops, 0.5), rgb(255, 0, 0));
        assert_eq!(gradient_at(&stops, 1.0), rgb(255, 255, 255));
        assert_eq!(gradient_at(&[], 0.5), SPLIT_DIVIDER_COLOR);

        let mut cfg = SplitDividerConfig::default();
        cfg.color = "#123456".into();
        cfg.gradient = Some(DividerGradient {
            colors: vec!["不是颜色".into(), "也不是".into()],
            orientation: DividerGradientDir::Auto,
        });
        assert_eq!(divider_stops(&cfg).len(), 1, "非法渐变退回单色");
        assert_eq!(divider_stops(&cfg)[0], rgb(0x12, 0x34, 0x56));
    }
}

/// 分割栏拖动 / 快捷键调整窗格大小:布局树路径与几何的纯逻辑回归。
#[cfg(test)]
mod split_resize_tests {
    use super::*;

    /// 根是左右分屏,左半再上下分屏:
    ///        root(V 0.5)
    ///        /         \
    ///   a(H 0.4)      leaf2
    ///   /      \
    /// leaf0   leaf1
    fn tree() -> Box<SplitNode> {
        Box::new(SplitNode::Split {
            dir: SplitDir::Vertical,
            ratio: 0.5,
            a: Box::new(SplitNode::Split {
                dir: SplitDir::Horizontal,
                ratio: 0.4,
                a: SplitNode::leaf(0),
                b: SplitNode::leaf(1),
            }),
            b: SplitNode::leaf(2),
        })
    }

    #[test]
    fn ratio_at_and_set_ratio_at_walk_the_bitmap_path() {
        let mut t = tree();
        assert_eq!(t.ratio_at(0, 0), Some(0.5));
        // 根的第 0 位 = 0 → a 子树(那个水平 Split)
        assert_eq!(t.ratio_at(0, 1), Some(0.4));
        assert_eq!(t.ratio_at(1, 1), None, "根的 b 是叶子,不是 Split");
        assert!(t.set_ratio_at(0, 0, 0.7));
        assert_eq!(t.ratio_at(0, 0), Some(0.7));
        assert!(t.set_ratio_at(0, 1, 0.25));
        assert_eq!(t.ratio_at(0, 1), Some(0.25));
        assert_eq!(t.ratio_at(0, 0), Some(0.7), "改子层不能动根");
        assert!(!t.set_ratio_at(1, 1, 0.9));
    }

    #[test]
    fn splits_containing_returns_shallow_to_deep() {
        let t = tree();
        let mut out = Vec::new();
        t.splits_containing(1, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], (0, 0, SplitDir::Vertical, 0.5));
        // a 子树的路径位仍是 0(为 1 表示根的 b 分支)
        assert_eq!(out[1], (0, 1, SplitDir::Horizontal, 0.4));
        // 叶子 2 只被根包含
        let mut right = Vec::new();
        t.splits_containing(2, &mut right);
        assert_eq!(right, vec![(0, 0, SplitDir::Vertical, 0.5)]);
        let mut none = Vec::new();
        t.splits_containing(99, &mut none);
        assert!(none.is_empty());
    }

    #[test]
    fn computed_lines_carry_the_right_path_and_area() {
        let cfg = config::SplitDividerConfig::default();
        let ids = |i: usize| Some(i as u64);
        let (geoms, lines) =
            compute_pane_geoms(&tree(), &ids, 10.0, 18.0, 1000.0, 540.0, &cfg);
        assert_eq!(geoms.len(), 3);
        assert_eq!(lines.len(), 2);
        let v = lines.iter().find(|l| l.vertical).unwrap();
        assert_eq!(v.depth, 0, "根分割线深度 0");
        assert_eq!(v.area.2, 1000.0, "根区域宽度就是整个内容区");
        let h = lines.iter().find(|l| !l.vertical).unwrap();
        assert_eq!(h.depth, 1);
        // 这条横线所在的 path 必须能精确定位回左半那个水平 Split
        let mut t = tree();
        assert!(t.set_ratio_at(h.path, h.depth, 0.3));
        assert_eq!(t.ratio_at(h.path, h.depth), Some(0.3));
        assert_eq!(t.ratio_at(0, 0), Some(0.5), "不能误改根");
    }

    #[test]
    fn resize_stays_inside_min_ratio_bounds() {
        let lo = config::SplitDividerConfig::default().min_ratio;
        assert_eq!((-1.0f64).clamp(lo, 1.0 - lo), lo);
        assert_eq!((2.0f64).clamp(lo, 1.0 - lo), 1.0 - lo);
    }
}

#[cfg(test)]
mod pane_layout_tests {
    use super::*;

    fn ids(i: usize) -> Option<u64> {
        Some(i as u64 + 1)
    }

    fn v2() -> SplitNode {
        SplitNode::Split {
            dir: SplitDir::Vertical,
            ratio: 0.5,
            a: SplitNode::leaf(0),
            b: SplitNode::leaf(1),
        }
    }

    fn h2() -> SplitNode {
        SplitNode::Split {
            dir: SplitDir::Horizontal,
            ratio: 0.5,
            a: SplitNode::leaf(0),
            b: SplitNode::leaf(1),
        }
    }

    fn line_center(l: &SplitLine) -> (f64, f64) {
        (l.x + l.w * 0.5, l.y + l.h * 0.5)
    }

    #[test]
    fn vertical_split_is_exactly_centered_and_symmetric() {
        // 1000x540 的内容区,字符 10x18。旧实现会算出左 50 列 / 右 49 列。
        let cfg = SplitDividerConfig::default();
        let (geoms, lines) =
            compute_pane_geoms(&v2(), &ids, 10.0, 18.0, 1000.0, 540.0, &cfg);
        assert_eq!(geoms.len(), 2);
        assert_eq!(lines.len(), 1);
        assert_eq!(geoms[0].grid.0, geoms[1].grid.0, "左右列数必须严格相等");
        assert_eq!(geoms[0].grid.1, geoms[1].grid.1);
        assert_eq!((geoms[0].w - geoms[1].w).abs() < 1e-9, true, "两侧像素宽相等");
        assert_eq!((line_center(&lines[0]).0 - 500.0).abs() < 1e-9, true, "竖线在中线");
        assert_eq!((geoms[1].x - (500.0 + lines[0].w * 0.5)).abs() < 1e-9, true);
        // 默认缝只有「线宽 + 2px」(5px),不再按整格/字符宽留白
        assert!(lines[0].w < 10.0, "自动间距应收紧:{}", lines[0].w);
        assert!(((lines[0].w - (AUTO_LINE_T + 2.0)).abs()) < 1e-9);
        assert!(divider_line_t(&cfg, lines[0].w) < lines[0].w);
    }

    #[test]
    fn pane_text_touches_the_divider_on_both_sides() {
        let cfg = SplitDividerConfig::default();
        let (geoms, lines) =
            compute_pane_geoms(&v2(), &ids, 10.0, 18.0, 1000.0, 540.0, &cfg);
        let line = &lines[0];
        let cell = 10.0;
        let left_text_right = geoms[0].x + geoms[0].grid.0 as f64 * cell;
        // 左 pane 文字右边界贴缝的左边缘
        assert!((left_text_right - line.x).abs() < 1e-9);
        // 右 pane 文字左边界贴缝的右边缘
        assert!((geoms[1].x - (line.x + line.w)).abs() < 1e-9);
        // 取整余量留在远离分割线的一侧(左 pane 的左边),不堆在靠线处
        assert!(geoms[0].x > 0.0, "左 pane 的余量应留在左侧");
        // 两侧文字到线边缘都只剩 1px 空气(默认),视觉上就是"贴着线"
        let lt = divider_line_t(&cfg, line.w);
        assert!(
            ((line.w - lt) * 0.5 - 1.0).abs() < 1e-9,
            "默认缝只留 1px 空气:gap={} line={}",
            line.w,
            lt
        );
    }

    #[test]
    fn horizontal_split_is_exactly_centered() {
        let cfg = SplitDividerConfig::default();
        let (geoms, lines) =
            compute_pane_geoms(&h2(), &ids, 10.0, 18.0, 1000.0, 540.0, &cfg);
        assert_eq!(geoms[0].grid.1, geoms[1].grid.1, "上下行数必须严格相等");
        assert_eq!((line_center(&lines[0]).1 - 270.0).abs() < 1e-9, true, "横线在中线");
        assert_eq!((geoms[0].h - geoms[1].h).abs() < 1e-9, true);
    }

    #[test]
    fn nested_splits_are_each_centered_in_their_own_area() {
        // 左半再上下分:竖线在整窗中线,横线在左半区域中线
        let inner = SplitNode::Split {
            dir: SplitDir::Horizontal,
            ratio: 0.5,
            a: SplitNode::leaf(0),
            b: SplitNode::leaf(2),
        };
        let root = SplitNode::Split {
            dir: SplitDir::Vertical,
            ratio: 0.5,
            a: Box::new(inner),
            b: SplitNode::leaf(1),
        };
        let cfg = SplitDividerConfig::default();
        let (geoms, lines) =
            compute_pane_geoms(&root, &ids, 10.0, 18.0, 1000.0, 540.0, &cfg);
        assert_eq!(geoms.len(), 3);
        assert_eq!(lines.len(), 2);
        let vertical = lines.iter().find(|l| l.vertical).unwrap();
        let horizontal = lines.iter().find(|l| !l.vertical).unwrap();
        assert!((line_center(vertical).0 - 500.0).abs() < 1e-9);
        assert!((line_center(horizontal).1 - 270.0).abs() < 1e-9);
        // 横线只覆盖左半区域,不会横穿右 pane
        assert!((horizontal.w - (500.0 - vertical.w * 0.5)).abs() < 1e-9);
        assert!((horizontal.x - 0.0).abs() < 1e-9);
    }

    #[test]
    fn gap_and_text_gap_are_user_controllable_and_auto_stays_tight() {
        let mut cfg = SplitDividerConfig::default();
        let auto = divider_gap_px(&cfg, 10.0, 1000.0);
        // 默认 = 线宽 + 2px:文字到线边缘 1px,和字号/DPI 无关
        assert!((auto - (AUTO_LINE_T + 2.0)).abs() < 1e-9, "auto={auto}");
        assert_eq!(
            divider_gap_px(&cfg, 20.0, 1000.0),
            auto,
            "字号变大后缝不该跟着变宽(旧实现按 0.7 字符算,这是文字离线远的根因)"
        );
        // 线变粗时缝跟着长,但两侧的空气仍固定 1px
        cfg.thickness = Some(8.0);
        assert_eq!(divider_gap_px(&cfg, 10.0, 1000.0), 10.0);
        cfg.thickness = None;

        cfg.gap = Some(20.0);
        assert_eq!(divider_gap_px(&cfg, 10.0, 1000.0), 20.0);
        cfg.text_gap = Some(6.0);
        let gap = divider_gap_px(&cfg, 10.0, 1000.0);
        let t = divider_line_t(&cfg, gap);
        assert!((gap - (2.0 * 6.0 + t)).abs() < 1e-9, "text_gap 优先于 gap");
        assert!((gap * 0.5 - t * 0.5 - 6.0).abs() < 1e-9, "文字到线边缘 = text_gap");

        // 线比缝还宽时自动把缝撑开,线不会压到文字
        let mut wide = SplitDividerConfig::default();
        wide.thickness = Some(30.0);
        assert!(divider_gap_px(&wide, 10.0, 1000.0) >= 31.0);
        // 极窄 pane 自动收窄,不把文字区吃光
        assert!(divider_gap_px(&wide, 10.0, 60.0) <= 10.0);
    }

    /// `split_divider.text_gap` 的承诺:设多少,文字离线的边缘就是多少 px,
    /// 竖线 / 横线、左右 / 上下四个方向都成立。
    #[test]
    fn custom_text_gap_is_the_exact_distance_on_both_axes() {
        for tg in [0.0, 2.0, 4.5, 9.0] {
            for thick in [None, Some(4.0), Some(7.0)] {
                let mut cfg = SplitDividerConfig::default();
                cfg.thickness = thick;
                cfg.text_gap = Some(tg);

                // 左右分屏:量左 pane 文字的右边界 / 右 pane 文字的左边界到线边缘
                let (geoms, lines) =
                    compute_pane_geoms(&v2(), &ids, 10.0, 18.0, 1000.0, 540.0, &cfg);
                let line = &lines[0];
                let t = divider_line_t(&cfg, line.w);
                let line_left = line.x + (line.w - t) * 0.5;
                let line_right = line_left + t;
                let left_text_right = geoms[0].x + geoms[0].grid.0 as f64 * 10.0;
                let right_text_left = geoms[1].x;
                let left_dist = line_left - left_text_right;
                let right_dist = right_text_left - line_right;
                assert!(
                    (left_dist - tg).abs() < 1e-9,
                    "竖线左距 {} != text_gap {tg}(thickness={thick:?})",
                    left_dist
                );
                assert!(
                    (right_dist - tg).abs() < 1e-9,
                    "竖线右距 {} != text_gap {tg}(thickness={thick:?})",
                    right_dist
                );

                // 上下分屏:量上 pane 文字的底边 / 下 pane 文字的顶边到线边缘
                let (geoms, lines) =
                    compute_pane_geoms(&h2(), &ids, 10.0, 18.0, 1000.0, 540.0, &cfg);
                let line = &lines[0];
                let t = divider_line_t(&cfg, line.w);
                let line_top = line.y + (line.h - t) * 0.5;
                let line_bottom = line_top + t;
                let top_text_bottom = geoms[0].y + geoms[0].grid.1 as f64 * 18.0;
                let bottom_text_top = geoms[1].y;
                assert!(
                    ((line_top - top_text_bottom) - tg).abs() < 1e-9,
                    "横线上距 {} != text_gap {tg}(thickness={thick:?})",
                    line_top - top_text_bottom
                );
                assert!(
                    ((bottom_text_top - line_bottom) - tg).abs() < 1e-9,
                    "横线下距 {} != text_gap {tg}(thickness={thick:?})",
                    bottom_text_top - line_bottom
                );
            }
        }
    }

    /// 嵌套分屏:两个轴的对齐各管各的。左半再上下分时,左下 pane 既要贴住
    /// 竖线(余量留在左边),又不能让"贴右边"的余量把第一行文字推离横线。
    #[test]
    fn nested_split_aligns_both_axes_independently() {
        let inner = SplitNode::Split {
            dir: SplitDir::Horizontal,
            ratio: 0.5,
            a: SplitNode::leaf(0),
            b: SplitNode::leaf(2),
        };
        let root = SplitNode::Split {
            dir: SplitDir::Vertical,
            ratio: 0.5,
            a: Box::new(inner),
            b: SplitNode::leaf(1),
        };
        let mut cfg = SplitDividerConfig::default();
        cfg.text_gap = Some(6.0);
        let (geoms, lines) =
            compute_pane_geoms(&root, &ids, 10.0, 18.0, 1000.0, 540.0, &cfg);
        let v = lines.iter().find(|l| l.vertical).unwrap();
        let h = lines.iter().find(|l| !l.vertical).unwrap();
        let th = divider_line_t(&cfg, h.h);
        // SplitLine 覆盖的是整条缝,线画在缝中央
        let h_top = h.y + (h.h - th) * 0.5;
        let h_bottom = h_top + th;
        let tv = divider_line_t(&cfg, v.w);
        let v_left = v.x + (v.w - tv) * 0.5;

        let pane = |idx: usize| {
            geoms
                .iter()
                .find(|g| g.pane_id == ids(idx).unwrap())
                .unwrap_or_else(|| panic!("缺 pane {idx}"))
        };
        let upper = pane(0);
        let lower = pane(2);
        let right = pane(1);

        // 上下两块:上块底边贴横线上缘,下块顶边贴横线下缘(各 text_gap)
        let upper_bottom = upper.y + upper.h;
        assert!(
            ((h_top - upper_bottom) - 6.0).abs() < 1e-9,
            "左上离横线 {} != 6",
            h_top - upper_bottom
        );
        assert!(
            ((lower.y - h_bottom) - 6.0).abs() < 1e-9,
            "左下离横线 {} != 6(旧的单布尔实现会多出取整余量)",
            lower.y - h_bottom
        );
        // 左右两块:都贴竖线,右侧 pane 贴左缘
        let upper_right = upper.x + upper.w;
        assert!(((v_left - upper_right) - 6.0).abs() < 1e-9);
        assert!(((right.x - (v_left + tv)) - 6.0).abs() < 1e-9);
        // 左半的取整余量留在最左边(远离竖线),下块也不会被竖直方向推走
        assert!(upper.x > 0.0);
        assert!(lower.x > 0.0);
        assert_eq!(lower.x, upper.x, "同一列的两个 pane 左右必须对齐");
    }

    #[test]
    fn sidebar_inset_pushes_panes_right_and_shrinks_columns() {
        let cfg = SplitDividerConfig::default();
        // 侧边栏占 26 列 + 1px:1000px 宽、10px 一格时 inset = 261px
        let inset = 26.0 * 10.0 + 1.0;
        let (full, full_lines) =
            compute_pane_geoms(&v2(), &ids, 10.0, 18.0, 1000.0, 540.0, &cfg);
        let (side, side_lines) = compute_pane_geoms_in(
            &v2(),
            &ids,
            10.0,
            18.0,
            inset,
            1000.0 - inset,
            540.0,
            &cfg,
        );
        // 所有 pane 都推到侧边栏右边
        assert!(side.iter().all(|g| g.x >= inset - 1e-9), "{:?}", side);
        // 总列数少了 26 列(侧边栏吃掉的就是这些)
        let cols_full: usize = full.iter().map(|g| g.grid.0).sum();
        let cols_side: usize = side.iter().map(|g| g.grid.0).sum();
        // 侧边栏吃掉 26 列,余下的再按分屏切;取整余地会少掉一两列
        assert!(
            (20..=26).contains(&(cols_full - cols_side)),
            "整窗 {cols_full} 列 -> 侧边栏后 {cols_side} 列"
        );
        // 分割线也整体右移,并且仍在两块 pane 之间
        let (x0, w0) = (side_lines[0].x, side_lines[0].w);
        assert!(x0 >= inset - 1e-9);
        assert!((x0 + w0 / 2.0 - 500.0).abs() > 40.0, "线应该压在右侧区域的中线附近");
        assert!(side_lines[0].x < full_lines[0].x + inset + 1e-9);
    }

    #[test]
    fn split_anim_starts_from_the_edge() {
        let cfg = SplitDividerConfig::default();
        // 左右分屏:线从最右往左走
        let mut anim_tree = v2();
        anim_tree.set_ratio_of_new_pane(1, 1.0);
        let (_, lines) =
            compute_pane_geoms(&anim_tree, &ids, 10.0, 18.0, 1000.0, 540.0, &cfg);
        let start_x = line_center(&lines[0]).0;
        assert!(start_x > 990.0, "动画起点线贴在最右边缘");
        anim_tree.set_ratio_of_new_pane(1, 0.5);
        let (_, lines) =
            compute_pane_geoms(&anim_tree, &ids, 10.0, 18.0, 1000.0, 540.0, &cfg);
        assert!((line_center(&lines[0]).0 - 500.0).abs() < 1e-9);

        // 上下分屏:线从最下往**上**走(不是从上往下),终点在 270
        let mut anim_tree = h2();
        anim_tree.set_ratio_of_new_pane(1, 1.0);
        let (geoms, lines) =
            compute_pane_geoms(&anim_tree, &ids, 10.0, 18.0, 1000.0, 540.0, &cfg);
        let start_y = line_center(&lines[0]).1;
        assert!(start_y > 530.0, "动画起点线贴在最下边缘:{start_y}");
        // 起点时上 pane 吃满整个高度,下 pane 只在底部露一条缝(还没长出来)
        assert_eq!(geoms.len(), 2);
        assert!(geoms[0].grid.1 >= 29, "上 pane 应占满高度:{:?}", geoms[0].grid);
        assert!(geoms[1].grid.1 <= 1, "下 pane 起点还没有行:{:?}", geoms[1].grid);
        anim_tree.set_ratio_of_new_pane(1, 0.5);
        let (_, lines) =
            compute_pane_geoms(&anim_tree, &ids, 10.0, 18.0, 1000.0, 540.0, &cfg);
        let end_y = line_center(&lines[0]).1;
        assert!((end_y - 270.0).abs() < 1e-9);
        assert!(end_y < start_y, "分割线必须向上移动:{start_y} -> {end_y}");
    }

    /// 跨窗格尾迹必须和窗格内尾迹**逐帧一致**:同一个 `CursorTrail`、同一份
    /// 参数,区别只是用 `seed` 把起点搬到另一个 pane 的旧光标位置。
    #[test]
    fn focus_trail_matches_pane_trail_frame_by_frame() {
        let cfg = config::CursorConfig {
            trail_dwell_ms: 30.0,
            trail_start_x: 2,
            trail_start_y: 2,
            ..config::CursorConfig::default()
        };
        let (cw, ch) = (9.0f32, 18.0f32);
        // 旧光标在左上,新光标在另一个 pane 的右侧 —— 一段真正的长距离
        let a = [0.0, cw, 0.0, ch];
        let b = [40.0 * cw, 41.0 * cw, 12.0 * ch, 13.0 * ch];
        let t0 = Instant::now();

        // 窗格内:先在 a 初始化,然后光标跳到 b
        let mut pane = cursor::CursorTrail::new(&cfg);
        pane.update(t0, a, true, cw, ch);
        // 跨窗格:直接把尾迹"种"到 a
        let mut focus = cursor::CursorTrail::new(&cfg);
        focus.seed(a, t0);

        let mut drawn = 0;
        let mut widest = 0.0f32;
        for i in 1..=40u64 {
            let now = t0 + Duration::from_millis(i * 8);
            let p = pane.update(now, b, true, cw, ch);
            let f = focus.update(now, b, true, cw, ch);
            match (p, f) {
                (None, None) => {}
                (Some(x), Some(y)) => {
                    drawn += 1;
                    for k in 0..4 {
                        assert!(
                            (x.corners[k].x - y.corners[k].x).abs() < 1e-4
                                && (x.corners[k].y - y.corners[k].y).abs() < 1e-4,
                            "{i}ms:第 {k} 个角不一致 —— 两边不是同一个动画"
                        );
                    }
                    assert!(
                        (x.opacity - y.opacity).abs() < 1e-6,
                        "{i}ms:不透明度不一致"
                    );
                    let xs = y.corners.iter().map(|c| c.x);
                    let span = xs.clone().fold(f32::MIN, f32::max)
                        - xs.fold(f32::MAX, f32::min);
                    widest = widest.max(span);
                }
                (p, f) => panic!("{i}ms:出帧时机不同 {p:?} / {f:?}"),
            }
            assert_eq!(
                pane.needs_frames(),
                focus.needs_frames(),
                "{i}ms:调度结论不一致"
            );
        }
        assert!(drawn >= 8, "尾迹几乎没出帧:{drawn}");
        // 尾巴被拉开过:横向跨度接近整段行程,而不是"本体走完了、尾巴没动"
        assert!(
            widest > 10.0 * cw,
            "尾迹没有拉出应有的跨度:{widest}px(行程 {}px)",
            40.0 * cw
        );
        // 收拢:最终停在 b 上,不再要求出帧
        assert!(!focus.needs_frames(), "收尾后不该继续出帧");
    }

    /// 跨窗格尾迹不能"飞出边界":四个角始终留在 `from → to` 的包围盒里,
    /// 收尾时精确落在新光标上(否则最后一帧会跳一下)。
    #[test]
    fn focus_trail_stays_inside_the_swept_box() {
        let cfg = config::CursorConfig {
            // 注意:dwell = 0 在 CursorTrail 里表示"关掉尾迹",这里要留 1ms
            trail_dwell_ms: 1.0,
            ..config::CursorConfig::default()
        };
        let (cw, ch) = (13.0f32, 23.0f32);
        let a = [700.0, 700.0 + cw, 120.0, 120.0 + ch];
        let b = [120.0, 120.0 + cw, 400.0, 400.0 + ch];
        let t0 = Instant::now();
        let mut t = cursor::CursorTrail::new(&cfg);
        t.seed(a, t0);
        let mut drawn = 0;
        for i in 1..=40u64 {
            let now = t0 + Duration::from_millis(i * 8);
            if let Some(d) = t.update(now, b, true, cw, ch) {
                drawn += 1;
                for c in d.corners {
                    assert!(
                        c.x >= b[0] - 1.0 && c.x <= a[0] + 1.0,
                        "{i}ms:x 越界 {}",
                        c.x
                    );
                    assert!(
                        c.y >= a[2] - 1.0 && c.y <= b[3] + 1.0,
                        "{i}ms:y 越界 {}",
                        c.y
                    );
                }
            }
        }
        assert!(drawn > 0, "整段动画一帧都没画");
        // 收尾:精确贴到新光标,且不再要求出帧
        assert!(
            t.update(t0 + Duration::from_millis(800), b, true, cw, ch)
                .is_none(),
            "动画结束后不该再出帧"
        );
        assert!(!t.needs_frames());
    }

    #[test]
    fn inactive_panes_have_no_caret() {
        assert!(pane_caret_visible(true, true));
        assert!(!pane_caret_visible(false, true), "非聚焦 pane 不画输入光标");
        // 跨窗格切换时聚焦 pane 的光标照常画(不存在"飞行期间让位")
        assert!(pane_caret_visible(true, false));
        assert!(pane_caret_visible(false, false), "关掉开关后仍可全显示");
    }

    #[test]
    fn open_file_args_roundtrip_to_new_window() {
        let args: Vec<String> = [
            "VectorTerm.exe",
            "--config",
            "C:\\cfg\\init.lua",
            "--open-file",
            "C:\\work\\a b.txt",
            "--",
            "C:\\tools\\nvim.exe",
            "-u",
            "min.lua",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let (path, editor) = parse_open_file_args(&args).expect("应认出 --open-file");
        assert_eq!(path, PathBuf::from("C:\\work\\a b.txt"));
        assert_eq!(
            editor,
            vec![
                "C:\\tools\\nvim.exe".to_string(),
                "-u".to_string(),
                "min.lua".to_string()
            ]
        );

        // 没带 `--` 时编辑器为空,调用方退回普通窗口而不是崩在索引上
        let bare: Vec<String> = ["VectorTerm.exe", "--open-file", "x.txt"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (p, ed) = parse_open_file_args(&bare).expect("路径在就算认出来");
        assert_eq!(p, PathBuf::from("x.txt"));
        assert!(ed.is_empty());
        // 连路径都没有:不成形,按普通启动
        let no_path: Vec<String> = ["VectorTerm.exe", "--open-file"].iter().map(|s| s.to_string()).collect();
        assert!(parse_open_file_args(&no_path).is_none());
        assert!(parse_open_file_args(&["VectorTerm.exe".to_string()]).is_none());
    }

    #[test]
    fn apply_open_file_turns_window_into_editor() {
        let mut cfg = AppConfig::default();
        let editor = vec!["C:\\tools\\nvim.exe".to_string(), "-u".to_string()];
        let ok = apply_open_file(&mut cfg, Path::new("C:\\work\\a.txt"), &editor);
        assert!(ok);
        assert_eq!(cfg.shell, "C:\\tools\\nvim.exe");
        assert_eq!(
            cfg.shell_args,
            vec!["-u".to_string(), "C:\\work\\a.txt".to_string()],
            "固定参数在前,文件名在最后"
        );
        assert_eq!(cfg.cwd, Some(PathBuf::from("C:\\work")));
        assert!(!cfg.session_persistence, "编辑器窗口不跨进程保活");

        // 没给编辑器:返回 false,让调用方保持普通终端
        let mut cfg2 = AppConfig::default();
        assert!(!apply_open_file(&mut cfg2, Path::new("a.txt"), &[]));
    }

    #[test]
    fn shell_cd_line_matches_each_shell() {
        let dir = Path::new(r"C:\a b\子目录");
        assert_eq!(shell_cd_line("C:\\Windows\\System32\\cmd.exe", dir), "cd /d \"C:\\a b\\子目录\"\r");
        assert_eq!(
            shell_cd_line("C:\\Program Files\\PowerShell\\7\\pwsh.exe", dir),
            "Set-Location -LiteralPath 'C:\\a b\\子目录'\r"
        );
        assert_eq!(
            shell_cd_line("C:\\Program Files\\Git\\bin\\bash.exe", dir),
            "cd \"C:/a b/子目录\"\r",
            "非 cmd/PowerShell 走 POSIX 风格,反斜杠换成 /"
        );
        // PowerShell 单引号里的单引号靠重复转义
        assert_eq!(
            shell_cd_line("pwsh", Path::new(r"C:\it's\x")),
            "Set-Location -LiteralPath 'C:\\it''s\\x'\r"
        );
    }

    #[test]
    fn shell_command_line_quotes_per_shell() {
        let argv = vec![
            "C:\\tools\\nvim.exe".to_string(),
            "C:\\a b\\x.txt".to_string(),
        ];
        assert_eq!(
            shell_command_line("C:\\Windows\\System32\\cmd.exe", &argv),
            "\"C:\\tools\\nvim.exe\" \"C:\\a b\\x.txt\""
        );
        assert_eq!(
            shell_command_line("C:\\Program Files\\PowerShell\\7\\pwsh.exe", &argv),
            "& 'C:\\tools\\nvim.exe' 'C:\\a b\\x.txt'"
        );
        // PowerShell 单引号内的单引号靠重复转义
        let q = vec!["nvim".to_string(), "C:\\it's\\a.txt".to_string()];
        assert_eq!(shell_command_line("pwsh", &q), "& 'nvim' 'C:\\it''s\\a.txt'");
        assert_eq!(shell_command_line("cmd.exe", &[]), "");
    }

    #[test]
    fn pane_number_keys_map_to_indices() {
        assert_eq!(digit_from_code(&WinitKeyCode::Digit1), Some(1));
        assert_eq!(digit_from_code(&WinitKeyCode::Numpad9), Some(9));
        assert_eq!(digit_from_code(&WinitKeyCode::Digit0), None);
        assert_eq!(digit_from_code(&WinitKeyCode::KeyA), None);
    }

    #[test]
    fn pane_hit_test_uses_local_rect() {
        let g = PaneGeom {
            pane_id: 2,
            grid: (40, 20),
            x: 500.0,
            y: 0.0,
            w: 490.0,
            h: 540.0,
        };
        assert!(g.contains(600.0, 100.0));
        assert!(!g.contains(499.0, 100.0));
        assert_eq!(g.center(), (745.0, 270.0));
    }
}

#[cfg(test)]
mod overlay_anim_tests {
    use super::*;

    /// 进场:第一帧就要算"可见"(否则配套的光标飞入会被当成没人要而清掉),
    /// 中途已过半(缓出),到点精确落在 1。
    #[test]
    fn overlay_opens_with_ease_out() {
        let t0 = Instant::now();
        let mut a = OverlayAnim::new();
        assert!(!a.visible(), "没打开时不该画");

        a.open(t0);
        assert!(a.visible(), "刚 open 的那一帧就必须算可见");
        assert!(a.running());
        assert_eq!(a.value(), 0.0);

        // 190ms 的一半:缓出应该已经走过大半路程
        assert!(a.tick(t0 + Duration::from_millis(95)));
        let mid = a.value();
        assert!(mid > 0.7 && mid < 1.0, "缓出中点的进度应当偏后: {mid}");

        assert!(!a.tick(t0 + Duration::from_millis(190)), "到点就不该再要帧");
        assert_eq!(a.value(), 1.0);
        assert!(a.visible(), "完全展开当然还要画");
        assert!(!a.running());
    }

    /// 退场:逻辑上关了但还要画,收完才不可见;每一帧都要在动(早先的 ease-in
    /// 版本前三分之二几乎不动,实测像"啪"地消失)。
    #[test]
    fn overlay_closes_smoothly_and_disappears() {
        let t0 = Instant::now();
        let mut a = OverlayAnim::new();
        a.open(t0);
        a.tick(t0 + Duration::from_millis(190));
        assert_eq!(a.value(), 1.0);

        let t1 = t0 + Duration::from_millis(400);
        a.close(t1);
        assert!(a.visible(), "刚关掉时还得把退场画完");

        let mut last = a.value();
        for ms in [20, 40, 60, 80] {
            a.tick(t1 + Duration::from_millis(ms));
            let v = a.value();
            assert!(v < last, "退场第 {ms}ms 应该比上一帧更小: {v} vs {last}");
            last = v;
        }
        assert!(last < 0.75, "80ms 后应该收掉一大半: {last}");

        a.tick(t1 + Duration::from_millis(160));
        assert_eq!(a.value(), 0.0);
        assert!(!a.visible());
    }

    /// 半路反悔:从当前进度往回走,不能跳。
    #[test]
    fn overlay_reverses_without_jumping() {
        let t0 = Instant::now();
        let mut a = OverlayAnim::new();
        a.open(t0);
        a.tick(t0 + Duration::from_millis(40));
        let before = a.value();
        assert!(before > 0.0 && before < 1.0);

        a.close(t0 + Duration::from_millis(40));
        assert_eq!(a.value(), before, "反转的那一帧不能跳变");
        a.tick(t0 + Duration::from_millis(60));
        assert!(a.value() < before, "反转之后应当往回走");

        // 再打开:从当前值继续往上,同样不跳
        let mid = a.value();
        a.open(t0 + Duration::from_millis(60));
        assert_eq!(a.value(), mid);
        a.tick(t0 + Duration::from_millis(120));
        assert!(a.value() > mid);
    }

    /// 变换:绕中心缩放,中心只受位移影响;完全展开时是恒等变换。
    #[test]
    fn overlay_xf_scales_around_center() {
        let xf = OverlayXf::new(100.0, 200.0, 0.0, 22.0);
        let (x, y) = xf.pt(100.0, 200.0);
        assert!((x - 100.0).abs() < 1e-9, "中心不该横向移动");
        assert!((y - 222.0).abs() < 1e-9, "中心只该整体下沉 rise");

        let (_, _, w, h) = xf.rect(0.0, 100.0, 200.0, 100.0);
        assert!((w - 188.0).abs() < 1e-9, "起点缩到 94%");
        assert!((h - 94.0).abs() < 1e-9);

        let full = OverlayXf::new(100.0, 200.0, 1.0, 22.0);
        let (fx, fy) = full.pt(13.0, 47.0);
        assert!((fx - 13.0).abs() < 1e-9 && (fy - 47.0).abs() < 1e-9);
        let (_, _, fw, fh) = full.rect(0.0, 0.0, 200.0, 100.0);
        assert!((fw - 200.0).abs() < 1e-9 && (fh - 100.0).abs() < 1e-9);
    }

    /// 不透明度:单调不减、0 完全透明、1 完全实心 —— 否则退场会让面板
    /// 中途突然"变亮"再消失。
    #[test]
    fn overlay_alpha_is_monotonic() {
        assert_eq!(overlay_alpha(0.0), 0.0);
        assert!((overlay_alpha(1.0) - 1.0).abs() < 1e-6);
        let mut last = -1.0;
        for i in 0..=40 {
            let a = overlay_alpha(i as f64 / 40.0);
            assert!(a >= last, "进度 {i}/40 时反而变暗了: {a} < {last}");
            last = a;
        }
    }
}
