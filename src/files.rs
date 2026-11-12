//! 文件浏览器:列表模型 + 面板绘制。
//!
//! 命令面板里输入 `/files`(或 `/` 后选 files)是全屏列表,
//! `Ctrl+Shift+E` 是从左侧分出一条窄栏。
//! 两种模式共用同一份模型,只是绘制的矩形和可见行数不同:
//!
//! - **全屏**:铺满宿主 pane(分屏时只盖当前窗格),底部一行按键提示;
//! - **侧边栏**:只占左边 `side_cols` 列,列表项只显示名字(宽度不够时连
//!   尺寸列一起省掉),不画页脚。
//!
//! 模型刻意不依赖 winit / 渲染管线:目录排序、上下移动、进入目录、回上级
//! 都是纯逻辑,单测里用临时目录就能覆盖。绘制部分只吃 [`crate::render::Renderer`]
//! 的文字接口和 [`crate::frame::Frame`] 的矩形接口。
//!
//! 路径同步:打开时把 shell 报上来的当前目录(OSC 7 / 进程工作目录)传进来;
//! 之后两边是**双向同步**的 —— shell 里 `cd` 后面板跟着换目录
//! (见 `Browser::follow_dir`),面板里进目录 / 回上级则由上层把一行 `cd` 写回
//! shell。这一层只管"怎么换目录",不管往哪儿写 —— 那是 `main` 的事。

use crate::frame::Frame;
use crate::render::{Renderer, Rgb};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 两个路径指向同一个目录吗。
///
/// Windows 上大小写不敏感、正反斜杠混用都不算差别,末尾分隔符也一样
/// (`C:\` vs `C:`)。面板和终端各报一个路径,不做这层归一化会来回打架。
pub fn same_dir(a: &Path, b: &Path) -> bool {
    fn norm(p: &Path) -> String {
        let s = p.to_string_lossy().replace('/', "\\");
        let s = s.trim_end_matches('\\');
        if s.len() == 2 && s.as_bytes()[1] == b':' {
            // 盘符根目录:`C:` 补回 `C:\`
            format!("{s}\\")
        } else {
            s.to_string()
        }
    }
    norm(a).eq_ignore_ascii_case(&norm(b))
}

/// 侧边栏默认占多少列。30 列在 12~14px 字号下大约 240px,够放两层缩进的长名字。
pub const DEFAULT_SIDE_COLS: usize = 30;

/// 列表里的一项。
#[derive(Clone, Debug)]
pub struct Entry {
    /// 显示名。`..` 是回到上级的伪条目。
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    /// 文件字节数(目录为 0)
    pub size: u64,
    /// 修改时间;拿不到就是 None
    pub modified: Option<SystemTime>,
    /// true = `..` 伪条目
    pub up: bool,
}

/// 面板形态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// 铺满宿主 pane(命令面板 `/files` 进入)
    Full,
    /// 左侧窄栏(快捷键进入)
    Side,
}

/// 上下移动的语义。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
}

/// `activate()` 的结果:进了目录 or 选中了一个要打开的文件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Open {
    /// 已经进入该目录(模型已刷新)
    Dir(PathBuf),
    /// 选中文件,调用方决定用什么打开
    File(PathBuf),
}

/// 文件浏览器状态。一个标签页最多一份。
pub struct Browser {
    pub cwd: PathBuf,
    pub entries: Vec<Entry>,
    /// 选中项在 `entries` 里的下标
    pub selected: usize,
    /// 列表第一行对应 `entries` 的哪个下标(滚动位置)
    pub scroll: usize,
    pub mode: Mode,
    /// 键盘输入是否归这个面板。侧边栏和终端共存时,只有它为 true 才吃方向键。
    pub focused: bool,
    pub show_hidden: bool,
    pub side_cols: usize,
    /// 上一次刷新/进入目录时的错误(权限不足、目录被删等)
    pub err: Option<String>,
    /// 贴在面板底部的一行提示(目前主要是"编辑器没装")。None = 不占高度。
    ///
    /// 用英文写:这类提示多半要拿去搜报错、对文档,和终端里的工具输出保持一致。
    pub notice: Option<String>,
    /// 最近一次绘制里列表能放几行。翻页、滚动都按它算,和绘制共用一份口径。
    pub view_rows: usize,
}

impl Browser {
    /// 打开某个目录。目录读不出来时不报错退出,而是留一张空列表 + `err`,
    /// 面板照样能显示(还能用 `..` 往回走)。
    pub fn new(cwd: impl Into<PathBuf>, mode: Mode, show_hidden: bool, side_cols: usize) -> Self {
        let mut b = Self {
            cwd: cwd.into(),
            entries: Vec::new(),
            selected: 0,
            scroll: 0,
            mode,
            focused: true,
            show_hidden,
            side_cols: side_cols.clamp(8, 120),
            err: None,
            notice: None,
            view_rows: 10,
        };
        // 相对路径(比如 shell 报了个 ".")先解析成绝对路径,否则 `..` 会滚到
        // 进程自己的工作目录上去
        if b.cwd.is_relative() {
            if let Ok(abs) = std::fs::canonicalize(&b.cwd) {
                b.cwd = abs;
            }
        }
        b.refresh();
        b
    }

    /// 重新读目录,尽量保住当前选中的名字(刷新后还停在同一个文件上)。
    pub fn refresh(&mut self) {
        let keep = self.entries.get(self.selected).map(|e| e.name.clone());
        self.refresh_keeping(keep);
    }

    fn refresh_keeping(&mut self, keep: Option<String>) {
        let mut entries = Vec::new();
        let mut err = None;

        // 上级入口:盘符根目录没有 parent,就不给这一项
        if let Some(parent) = self.cwd.parent() {
            entries.push(Entry {
                name: "..".to_string(),
                path: parent.to_path_buf(),
                is_dir: true,
                size: 0,
                modified: None,
                up: true,
            });
        }

        let mut dirs: Vec<Entry> = Vec::new();
        let mut files: Vec<Entry> = Vec::new();
        match std::fs::read_dir(&self.cwd) {
            Ok(rd) => {
                for item in rd.flatten() {
                    let name = item.file_name().to_string_lossy().to_string();
                    // `.` / `..` 由上面那条伪条目代表,read_dir 通常也不会给
                    if name == "." || name == ".." || name.is_empty() {
                        continue;
                    }
                    // metadata 会跟随符号链接:目录链接也能进
                    let Ok(md) = item.metadata() else { continue };
                    if !self.show_hidden && is_hidden(&name, &md) {
                        continue;
                    }
                    let e = Entry {
                        name,
                        path: item.path(),
                        is_dir: md.is_dir(),
                        size: if md.is_dir() { 0 } else { md.len() },
                        modified: md.modified().ok(),
                        up: false,
                    };
                    if e.is_dir {
                        dirs.push(e);
                    } else {
                        files.push(e);
                    }
                }
            }
            Err(e) => err = Some(format!("{}: {e}", self.cwd.display())),
        }

        // 目录在前、文件在后,各自按名字排(大小写不敏感,和资源管理器一致)
        let by_name = |a: &Entry, b: &Entry| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.name.cmp(&b.name))
        };
        dirs.sort_by(by_name);
        files.sort_by(by_name);
        entries.extend(dirs);
        entries.extend(files);

        self.entries = entries;
        self.err = err;
        // 尽量停在原来那一项;找不到(被删/被隐藏)就退回列表顶部
        self.selected = match keep.and_then(|k| self.entries.iter().position(|e| e.name == k)) {
            Some(i) => i,
            None => 0,
        };
        self.clamp();
    }

    /// 当前选中的条目。
    pub fn selected_entry(&self) -> Option<&Entry> {
        self.entries.get(self.selected)
    }

    /// 移动选择。`view_rows` 是绘制时写回来的可见行数,翻页按它走。
    pub fn nav(&mut self, n: Nav) {
        let rows = self.view_rows.max(1);
        let last = self.entries.len().saturating_sub(1);
        match n {
            Nav::Up => self.selected = self.selected.saturating_sub(1),
            Nav::Down => self.selected = (self.selected + 1).min(last),
            Nav::PageUp => self.selected = self.selected.saturating_sub(rows),
            Nav::PageDown => self.selected = (self.selected + rows).min(last),
            Nav::Home => self.selected = 0,
            Nav::End => self.selected = last,
        }
        self.clamp();
    }

    /// 鼠标点到第 `row` 行(列表内的行号,0 = 第一可见行)。
    /// 返回是否落在有效行上。
    pub fn click_row(&mut self, row: usize) -> bool {
        let idx = self.scroll + row;
        if idx < self.entries.len() {
            self.selected = idx;
            self.clamp();
            true
        } else {
            false
        }
    }

    /// 回车:目录就进去,文件就交给调用方打开。
    pub fn activate(&mut self) -> Option<Open> {
        let e = self.entries.get(self.selected)?.clone();
        if e.up {
            self.go_parent();
            return None;
        }
        if e.is_dir {
            self.cwd = e.path;
            self.selected = 0;
            self.scroll = 0;
            self.refresh_keeping(None);
            // 有 `..` 时选中第一个真实条目,连按两下回车才不会原地打转
            if self.entries.len() > 1 {
                self.selected = 1;
            }
            self.clamp();
            Some(Open::Dir(self.cwd.clone()))
        } else {
            Some(Open::File(e.path))
        }
    }

    /// 回到上级目录,并把刚才待着的那一层选中(和资源管理器一致)。
    pub fn go_parent(&mut self) {
        let Some(parent) = self.cwd.parent().map(|p| p.to_path_buf()) else {
            return;
        };
        let leave = self.cwd.file_name().map(|s| s.to_string_lossy().to_string());
        self.cwd = parent;
        self.selected = 0;
        self.scroll = 0;
        self.refresh_keeping(leave);
    }

    /// 跟着终端换目录:shell 里 `cd` 到哪,面板就显示哪。
    ///
    /// 和 `go_parent` / `activate` 不同,这条路径是被动的 —— 不保留"回去时选中
    /// 刚才那一层",而是尽量保住**当前选中的那个名字**,用户正在列表里挑文件时
    /// 终端恰好换目录,选中项不会莫名其妙跳走。
    ///
    /// 返回 `true` 表示目录真的变了(调用方据此决定要不要重绘)。
    pub fn follow_dir(&mut self, dir: impl Into<PathBuf>) -> bool {
        let dir = dir.into();
        if same_dir(&self.cwd, &dir) {
            return false;
        }
        let keep = self.entries.get(self.selected).map(|e| e.name.clone());
        self.cwd = dir;
        self.selected = 0;
        self.scroll = 0;
        self.refresh_keeping(keep);
        true
    }

    /// `.` 键:在"显示隐藏文件"和"只看普通文件"之间切换。
    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.refresh();
    }

    /// 列表滚动一屏(滚轮用)。`delta` 为正 = 往下。
    pub fn scroll_by(&mut self, delta: isize) {
        let last = self.entries.len().saturating_sub(1) as isize;
        self.selected = (self.selected as isize + delta).clamp(0, last.max(0)) as usize;
        self.clamp();
    }

    /// 保证选中项在可视区内。`view_rows` 由绘制写入。
    pub fn clamp(&mut self) {
        if self.entries.is_empty() {
            self.selected = 0;
            self.scroll = 0;
            return;
        }
        self.selected = self.selected.min(self.entries.len() - 1);
        let rows = self.view_rows.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + rows {
            self.scroll = self.selected + 1 - rows;
        }
        // 列表末尾留白不该超过实际内容
        let max_scroll = self.entries.len().saturating_sub(rows);
        self.scroll = self.scroll.min(max_scroll);
    }

    /// 文件名(面板标题与打开命令用)
    pub fn dir_name(&self) -> String {
        match self.cwd.file_name() {
            Some(n) => n.to_string_lossy().to_string(),
            // 盘符根目录没有 file_name,退化成 "C:/"
            None => self.cwd.display().to_string(),
        }
    }

    /// 状态行:目录名 + 条目数(全屏模式页脚用)
    pub fn status(&self) -> String {
        let dirs = self.entries.iter().filter(|e| e.is_dir && !e.up).count();
        let files = self.entries.iter().filter(|e| !e.is_dir).count();
        format!("{dirs} 个文件夹 · {files} 个文件")
    }

    /// 挂一条底部提示(会占掉一行列表高度)。
    pub fn set_notice(&mut self, msg: impl Into<String>) {
        self.notice = Some(msg.into());
    }

    pub fn clear_notice(&mut self) {
        self.notice = None;
    }

    /// 提示文本按面板宽度折好的行。窄的侧边栏放不下一整句,折两行比截断
    /// 成 "nvim not installed or not i…" 有用。
    pub fn notice_lines(&self, cols: usize) -> Vec<String> {
        match self.notice.as_deref() {
            Some(msg) => wrap_cells(msg, cols.saturating_sub(2).max(1)),
            None => Vec::new(),
        }
    }

    /// 提示行占几行。绘制和鼠标命中都读它,避免"画在这里、点在那里"。
    pub fn notice_rows(&self, cols: usize) -> usize {
        self.notice_lines(cols).len()
    }
}

/// Windows 的隐藏判定:点开头(兼容 Unix 习惯)或带 FILE_ATTRIBUTE_HIDDEN(0x2)。
fn is_hidden(name: &str, md: &std::fs::Metadata) -> bool {
    if name.starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if md.file_attributes() & 0x2 != 0 {
            return true;
        }
    }
    #[cfg(not(windows))]
    {
        let _ = md;
    }
    false
}

/// 面板矩形(像素质,坐标是整窗坐标,含标签栏高度)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// 面板配色。由调用方从终端调色板里挑,保证和当前主题一致。
#[derive(Clone, Copy)]
pub struct Theme {
    pub fg: Rgb,
    pub bg: Rgb,
    /// 次要信息(尺寸、页脚提示、`..`)
    pub dim: Rgb,
    /// 目录名 / 强调色
    pub accent: Rgb,
    pub sel_bg: Rgb,
    pub sel_fg: Rgb,
    /// 侧边栏右边那条竖线
    pub border: Rgb,
    /// 底部提示行的颜色(未装编辑器这类"能用但不对劲"的消息)
    pub warn: Rgb,
}

/// 绘制结果里跟交互有关的部分(鼠标命中用)。
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    /// 列表第一行的整窗 y
    pub list_y: f64,
    /// 每行高
    pub row_h: f64,
    /// 可见行数
    pub rows: usize,
}

/// 把面板画进 `rect`。`rect` 已经是整窗坐标。
pub fn paint(
    rd: &mut Renderer,
    frame: &mut Frame,
    b: &mut Browser,
    rect: Rect,
    th: &Theme,
) -> Layout {
    let cw = rd.cell_w;
    let ch = rd.cell_h;
    let cols = (rect.w / cw).floor().max(1.0) as usize;

    // 底板:盖住下面的终端(全屏模式下就是整个内容区)
    frame.rect(rect.x, rect.y, rect.w, rect.h, th.bg, 1.0);

    // 顶部一行:标题。侧边栏只放目录名,全屏放完整路径。
    let header_h = ch;
    let title = match b.mode {
        Mode::Full => b.cwd.display().to_string(),
        Mode::Side => format!("{}", b.dir_name()),
    };
    let title_cells = cols.saturating_sub(2);
    rd.draw_plain_text(
        frame,
        &fit_cells(&title, title_cells),
        rect.x + cw * 0.5,
        rect.y + rd.ascent,
        rect.x + rect.w,
        th.accent,
        true,
    );
    // 标题下的一条细线:把标题和列表分开(只画半格高,不占行)
    frame.rect(
        rect.x + cw * 0.25,
        (rect.y + header_h - 1.0).round(),
        rect.w - cw * 0.5,
        1.0,
        th.dim,
        0.35,
    );

    // 页脚:只在全屏模式给按键提示(侧边栏太窄,放不下)
    let footer_h = if b.mode == Mode::Full { ch } else { 0.0 };
    // 提示行贴着底部(全屏在按键提示之上,侧边栏就在最下面):它占掉的高度
    // 要从列表里扣掉,不然最后一行会被挡住。`browser_geom` 用的是同一口径,
    // 连折行都走 `notice_lines`。
    let notice_lines = b.notice_lines(cols);
    let notice_h = notice_lines.len() as f64 * ch;
    let list_y = rect.y + header_h;
    let list_h = (rect.h - header_h - footer_h - notice_h).max(0.0);
    let rows = (list_h / ch).floor().max(0.0) as usize;
    b.view_rows = rows.max(1);
    b.clamp();

    // 列表
    if b.entries.is_empty() {
        let msg = if b.err.is_some() {
            "目录读不出来"
        } else {
            "(空目录)"
        };
        rd.draw_plain_text(
            frame,
            msg,
            rect.x + cw,
            list_y + rd.ascent,
            rect.x + rect.w,
            th.dim,
            false,
        );
    }

    // 名字列宽度:窄栏不留尺寸列
    let show_size = cols >= 24;
    let size_cells = if show_size { 8 } else { 0 };
    let name_cells = cols.saturating_sub(2 + size_cells);

    for (i, e) in b.entries.iter().enumerate().skip(b.scroll).take(rows) {
        let row = i - b.scroll;
        let y = list_y + row as f64 * ch;
        let selected = i == b.selected;
        if selected {
            // 选中行:整行铺一块圆角底色,文字换成反差色
            frame.round_rect(
                rect.x + 2.0,
                y,
                (rect.w - 4.0).max(1.0),
                ch,
                (ch * 0.22).round().max(2.0),
                th.sel_bg,
                1.0,
            );
        }
        let (fg, dim) = if selected {
            (th.sel_fg, th.sel_fg)
        } else {
            (th.fg, th.dim)
        };
        // 图标:目录用实心三角、文件用点、`..` 不画图标(名字本身就是"上"的
        // 意思,再加个箭头只会挤成 "↑..")。都是半角,不依赖 Nerd Font。
        let (icon, color) = if e.up {
            ("", dim)
        } else if e.is_dir {
            ("▸", if selected { fg } else { th.accent })
        } else {
            ("·", dim)
        };
        rd.draw_plain_text(
            frame,
            icon,
            rect.x + cw * 0.6,
            y + rd.ascent,
            rect.x + rect.w,
            color,
            false,
        );
        let name = if e.is_dir && !e.up {
            format!("{}/", e.name)
        } else {
            e.name.clone()
        };
        let name_x = rect.x + cw * 1.6;
        let name_max = rect.x + rect.w - cw * (0.6 + size_cells as f64);
        rd.draw_plain_text(
            frame,
            &fit_cells(&name, name_cells),
            name_x,
            y + rd.ascent,
            name_max,
            fg,
            selected,
        );
        if show_size && !e.is_dir {
            let size = format_size(e.size);
            // 右对齐:尺寸列终点贴到列表右边
            let w = rd.text_width(&size);
            rd.draw_plain_text(
                frame,
                &size,
                rect.x + rect.w - w - cw * 0.4,
                y + rd.ascent,
                rect.x + rect.w,
                dim,
                false,
            );
        }
    }

    // 滚动条:内容超出一屏时在右边画一条 2px 的指示条
    if b.entries.len() > rows && rows > 0 {
        let track_h = rows as f64 * ch;
        let frac = rows as f64 / b.entries.len() as f64;
        let thumb_h = (track_h * frac).max(12.0).min(track_h);
        let max_scroll = (b.entries.len() - rows).max(1) as f64;
        let t = b.scroll as f64 / max_scroll;
        frame.round_rect(
            rect.x + rect.w - 3.0,
            list_y + (track_h - thumb_h) * t,
            3.0,
            thumb_h,
            1.5,
            th.dim,
            0.55,
        );
    }

    // 底部提示行(未装编辑器之类)。用警告色 + 实心字形,扫一眼就能注意到;
    // 侧边栏放不下整句,由 `notice_lines` 折成两行。
    if notice_h > 0.0 {
        let y = rect.y + rect.h - footer_h - notice_h;
        frame.rect(rect.x, y, rect.w, notice_h, th.bg, 0.96);
        frame.rect(rect.x + cw * 0.25, y, rect.w - cw * 0.5, 1.0, th.warn, 0.35);
        for (i, line) in notice_lines.iter().enumerate() {
            rd.draw_plain_text(
                frame,
                line,
                rect.x + cw * 0.6,
                y + i as f64 * ch + rd.ascent,
                rect.x + rect.w,
                th.warn,
                true,
            );
        }
    }

    // 页脚
    if footer_h > 0.0 {
        let hints = "↑↓ 移动 · Enter 打开 · ← 上级 · . 隐藏项 · r 刷新 · Esc 关闭";
        let y = rect.y + rect.h - footer_h;
        frame.rect(rect.x, y, rect.w, footer_h, th.bg, 0.92);
        // 右边先留给"24 个文件夹 · 2 个文件";放不下就不画它,提示文本独占整行。
        // 面板现在只占一个 pane 的宽度(分屏后可能只有半个窗口),不预留就会
        // 出现两条文字压在一起。
        let st = b.status();
        let st_w = rd.text_width(&st);
        let st_cells = if st_w + cw * 2.0 < rect.w {
            (st_w / cw).ceil() as usize + 1
        } else {
            0
        };
        rd.draw_plain_text(
            frame,
            &fit_cells(hints, cols.saturating_sub(1 + st_cells)),
            rect.x + cw * 0.6,
            y + rd.ascent,
            rect.x + rect.w,
            th.dim,
            false,
        );
        if st_cells > 0 {
            rd.draw_plain_text(
                frame,
                &st,
                rect.x + rect.w - st_w - cw * 0.6,
                y + rd.ascent,
                rect.x + rect.w,
                th.dim,
                false,
            );
        }
    }

    Layout {
        list_y,
        row_h: ch,
        rows,
    }
}

/// 按"格"截断,超出补 `…`(和标签栏标题同一套做法,中文占两格)。
pub fn fit_cells(text: &str, cells: usize) -> String {
    if cells == 0 {
        return String::new();
    }
    let mut used = 0usize;
    let mut out = String::new();
    for ch in text.chars() {
        let w = crate::tabbar::char_width(ch);
        if used + w > cells {
            // 留一格放省略号
            while used + 1 > cells {
                if out.pop().is_none() {
                    break;
                }
                used -= 1;
            }
            out.push('…');
            return out;
        }
        used += w;
        out.push(ch);
    }
    out
}

/// 按"格"折行:优先在空格处断,断不开就硬断。提示语都是英文,按字符数近似
/// 就够;中文占两格,最多多折一行,不影响可读性。
fn wrap_cells(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for ch in text.chars() {
        let w = crate::tabbar::char_width(ch);
        if cur_w + w > width {
            if let Some(pos) = cur.rfind(' ') {
                let rest = cur[pos + 1..].to_string();
                let head = cur[..pos].trim_end().to_string();
                if !head.is_empty() {
                    lines.push(head);
                    cur_w = rest.chars().map(crate::tabbar::char_width).sum();
                    cur = rest;
                } else {
                    cur.clear();
                    cur_w = 0;
                }
            } else {
                lines.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
        }
        cur.push(ch);
        cur_w += w;
    }
    if !cur.trim().is_empty() {
        lines.push(cur.trim_end().to_string());
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// 文件尺寸的可读形式。目录调用方不会走到这里。
pub fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes}B")
    } else if b < KB * KB {
        format!("{:.1}K", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1}M", b / (KB * KB))
    } else {
        format!("{:.1}G", b / (KB * KB * KB))
    }
}

/// 模型层的单元测试:排序、导航、进退目录都不碰渲染,临时目录足够。
#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个临时目录树,返回根路径。名字带进程 id + 计数,避免并行测试撞车。
    fn temp_tree(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "vectorterm-files-{}-{tag}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("beta")).unwrap();
        std::fs::create_dir_all(root.join("Alpha")).unwrap();
        std::fs::write(root.join("z.txt"), b"12345").unwrap();
        std::fs::write(root.join("A.md"), b"x").unwrap();
        std::fs::write(root.join(".hidden"), b"h").unwrap();
        root
    }

    #[test]
    fn follow_dir_moves_and_keeps_pick() {
        let root = temp_tree("follow");
        // beta 里也放一个同名文件:换目录后应该继续选中它
        std::fs::write(root.join("beta").join("z.txt"), b"z").unwrap();
        let mut b = Browser::new(&root, Mode::Full, false, DEFAULT_SIDE_COLS);
        b.selected = b.entries.iter().position(|e| e.name == "z.txt").unwrap();

        assert!(b.follow_dir(root.join("beta")), "目录变了要返回 true");
        assert_eq!(b.cwd, root.join("beta"));
        assert_eq!(b.entries[b.selected].name, "z.txt", "同名项要保持选中");

        // 同一个目录(大小写 / 末尾分隔符不同)不算变
        assert!(!b.follow_dir(root.join("beta")));
        let with_sep = format!("{}\\", root.join("BETA").display());
        assert!(!b.follow_dir(PathBuf::from(with_sep)), "大小写与末尾反斜杠不算变化");

        // 回到 root:z.txt 还在,也还选着
        assert!(b.follow_dir(&root));
        assert_eq!(b.entries[b.selected].name, "z.txt");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn same_dir_ignores_case_and_separators() {
        assert!(same_dir(Path::new(r"C:\Windows"), Path::new("c:/windows/")));
        assert!(same_dir(Path::new(r"C:\"), Path::new("C:")));
        assert!(same_dir(Path::new(r"C:\"), Path::new(r"c:\")));
        assert!(!same_dir(Path::new(r"C:\Windows"), Path::new(r"C:\Windows\System32")));
        assert!(!same_dir(Path::new(r"C:\a"), Path::new(r"D:\a")));
    }

    #[test]
    fn dirs_first_then_files_case_insensitive() {
        let root = temp_tree("sort");
        let b = Browser::new(&root, Mode::Full, false, DEFAULT_SIDE_COLS);
        let names: Vec<&str> = b.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["..", "Alpha", "beta", "A.md", "z.txt"]);
        // 隐藏文件默认不出现
        assert!(!names.contains(&".hidden"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hidden_toggle_shows_dot_files() {
        let root = temp_tree("hidden");
        let mut b = Browser::new(&root, Mode::Full, false, DEFAULT_SIDE_COLS);
        b.toggle_hidden();
        assert!(b.entries.iter().any(|e| e.name == ".hidden"));
        b.toggle_hidden();
        assert!(!b.entries.iter().any(|e| e.name == ".hidden"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn nav_clamps_and_pages() {
        let root = temp_tree("nav");
        let mut b = Browser::new(&root, Mode::Full, false, DEFAULT_SIDE_COLS);
        b.view_rows = 2;
        b.nav(Nav::Up);
        assert_eq!(b.selected, 0, "已经在第一项,不能往下越界");
        b.nav(Nav::End);
        assert_eq!(b.selected, b.entries.len() - 1);
        b.nav(Nav::Down);
        assert_eq!(b.selected, b.entries.len() - 1, "最后一项不能再往下");
        b.nav(Nav::Home);
        assert_eq!(b.selected, 0);
        b.nav(Nav::PageDown);
        assert_eq!(b.selected, 2, "翻页 = 移动一屏(2 行)");
        // 滚动位置跟着选中项走
        assert!(b.scroll <= b.selected && b.selected < b.scroll + b.view_rows);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn enter_dir_and_back_keeps_selection() {
        let root = temp_tree("enter");
        let mut b = Browser::new(&root, Mode::Full, false, DEFAULT_SIDE_COLS);
        // 第一项是 `..`,第二项是第一个目录(Alpha)
        b.selected = 1;
        let name = b.selected_entry().unwrap().name.clone();
        let opened = b.activate();
        assert!(matches!(opened, Some(Open::Dir(_))));
        assert_eq!(b.cwd, root.join(&name));
        assert_eq!(b.dir_name(), name);
        // 空目录里只剩 `..`,回车回去
        assert_eq!(b.entries.len(), 1);
        b.activate();
        assert_eq!(b.cwd, root);
        assert_eq!(
            b.selected_entry().map(|e| e.name.clone()),
            Some(name),
            "回到上级后应该还停在刚才那个目录上"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn file_activation_reports_path() {
        let root = temp_tree("open");
        let mut b = Browser::new(&root, Mode::Full, false, DEFAULT_SIDE_COLS);
        let idx = b.entries.iter().position(|e| e.name == "z.txt").unwrap();
        b.selected = idx;
        match b.activate() {
            Some(Open::File(p)) => assert_eq!(p, root.join("z.txt")),
            other => panic!("应该是文件:{other:?}"),
        }
        // 文件不改变 cwd
        assert_eq!(b.cwd, root);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_dir_keeps_usable() {
        let base = temp_tree("gone");
        let root = base.join("nope");
        let mut b = Browser::new(&root, Mode::Full, false, DEFAULT_SIDE_COLS);
        assert!(b.err.is_some(), "读不出来要记住原因");
        // 只剩 `..` 这一条:还能往回走,不会把用户锁死在一个不存在的目录里
        assert_eq!(b.entries.len(), 1, "{:?}", b.entries);
        assert!(b.entries[0].up);
        // 不 panic,按什么都没反应
        b.nav(Nav::Down);
        assert!(b.activate().is_none());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn click_row_maps_through_scroll() {
        let root = temp_tree("click");
        let mut b = Browser::new(&root, Mode::Full, false, DEFAULT_SIDE_COLS);
        b.view_rows = 2;
        b.scroll = 2;
        assert!(b.click_row(1));
        assert_eq!(b.selected, 3);
        // 越过末尾不算命中
        assert!(!b.click_row(99));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn refresh_keeps_selected_name() {
        let root = temp_tree("refresh");
        let mut b = Browser::new(&root, Mode::Full, false, DEFAULT_SIDE_COLS);
        let idx = b.entries.iter().position(|e| e.name == "A.md").unwrap();
        b.selected = idx;
        b.refresh();
        assert_eq!(b.selected_entry().map(|e| e.name.clone()), Some("A.md".into()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn notice_wraps_and_clears() {
        let root = temp_tree("notice");
        let mut b = Browser::new(&root, Mode::Side, false, DEFAULT_SIDE_COLS);
        assert_eq!(b.notice_rows(30), 0, "没提示时不占行");
        b.set_notice("nvim not installed or not in PATH");
        // 30 列的面板:28 格可用,这条要折两行
        assert_eq!(b.notice_rows(30), 2, "{:?}", b.notice_lines(30));
        assert_eq!(b.notice_rows(90), 1, "全屏一行放得下");
        // 折行不丢字:拼回去和原文一致
        let joined = b.notice_lines(30).join(" ");
        assert_eq!(joined, "nvim not installed or not in PATH");
        b.clear_notice();
        assert_eq!(b.notice_rows(30), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn size_and_fit_helpers() {
        assert_eq!(format_size(999), "999B");
        assert_eq!(format_size(2048), "2.0K");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0M");
        assert_eq!(fit_cells("abcdef", 3), "ab…");
        // 中文占两格:4 格只能放 "中文" 两个字 + 省略号需要 5 格,所以截到 1 个字
        assert_eq!(fit_cells("中文字", 4), "中…");
        assert_eq!(fit_cells("abc", 10), "abc");
    }
}

