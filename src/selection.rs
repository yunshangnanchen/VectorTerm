//! 鼠标文本选区:坐标模型、点击计数、文本抽取。
//!
//! 坐标用**绝对物理行号**(wezterm-term `Screen::with_phys_lines` 的口径),
//! 而不是"当前视口第几行"。这样滚动、回看、拖到屏幕边缘时选区还锚定在
//! 原来的文字上;显示时再由 main 换算回视口行号。
//!
//! 文本抽取复用 `render::snapshot` 的归一化结果(`SnapCell.span` 已经处理了
//! 宽字符续格),避免自己去猜 `visible_cells()` 对双宽字符的迭代语义。

use crate::render::SnapCell;
#[cfg(test)]
use crate::render::ScreenSnapshot;
use std::time::{Duration, Instant};
use wezterm_term::Terminal;
#[cfg(test)]
use wezterm_term::Underline;

/// (列, 绝对物理行号)
pub type Pos = (usize, usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// 单击拖拽:任意单元格范围
    Cell,
    /// 双击:选词
    Word,
    /// 三击:整行
    Line,
}

#[derive(Clone, Debug)]
pub struct Selection {
    /// 按下时的锚点(拖动方向可能是往左上)
    pub anchor: Pos,
    /// 当前鼠标所在位置
    pub head: Pos,
    pub mode: Mode,
    pub dragging: bool,
}

impl Selection {
    pub fn new(anchor: Pos, mode: Mode) -> Self {
        Self {
            anchor,
            head: anchor,
            mode,
            dragging: true,
        }
    }

    /// (起点, 终点),先按行再按列排序
    pub fn normalized(&self) -> (Pos, Pos) {
        if (self.head.1, self.head.0) < (self.anchor.1, self.anchor.0) {
            (self.head, self.anchor)
        } else {
            (self.anchor, self.head)
        }
    }

    /// 单击未拖动:不算选区,松开时清掉
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

/// 选区状态 + 多击检测。winit 0.30 不给 click count,自己按时间/距离判定。
#[derive(Default)]
pub struct SelectionState {
    pub current: Option<Selection>,
    last_click: Option<(Instant, (f64, f64))>,
    clicks: u8,
}

impl SelectionState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.current = None;
    }

    /// 记录一次左键按下,返回这次该用的选择粒度
    pub fn press(&mut self, x: f64, y: f64, now: Instant) -> Mode {
        let close = self.last_click.as_ref().is_some_and(|(t, (px, py))| {
            now.duration_since(*t) < Duration::from_millis(450)
                && (px - x).abs() < 8.0
                && (py - y).abs() < 8.0
        });
        self.clicks = if close { (self.clicks + 1).min(3) } else { 1 };
        self.last_click = Some((now, (x, y)));
        match self.clicks {
            2 => Mode::Word,
            3 => Mode::Line,
            _ => Mode::Cell,
        }
    }
}

/// 词的分界:空白是一类,常见路径/括号分隔符是一类,其余连续字符是一类。
const DELIMS: &str = " \t()[]{}<>'\"`,;:|=&";

fn cell_kind(cell: &SnapCell) -> u8 {
    if cell.span == 0 {
        return 0; // 宽字符续格
    }
    let t = cell.text.as_str();
    if t.is_empty() || t.chars().all(char::is_whitespace) {
        0
    } else if t.chars().all(|c| !DELIMS.contains(c)) {
        1
    } else {
        2
    }
}

/// 双击选词:以 col 为中心向两侧扩到同类字符的边界。
/// 点在空白上时不选任何词(返回该格本身)。
pub fn word_bounds(row: &[SnapCell], col: usize) -> (usize, usize) {
    if row.is_empty() {
        return (0, 0);
    }
    let c = col.min(row.len() - 1);
    let k = cell_kind(&row[c]);
    if k == 0 {
        return (c, c);
    }
    let mut a = c;
    while a > 0 && cell_kind(&row[a - 1]) == k {
        a -= 1;
    }
    let mut b = c;
    while b + 1 < row.len() && cell_kind(&row[b + 1]) == k {
        b += 1;
    }
    (a, b)
}

/// 三击选行:从行首到最后一个非空白格(行尾空格不进剪贴板)。
pub fn line_bounds(row: &[SnapCell]) -> (usize, usize) {
    let mut last = row.len();
    while last > 0 && cell_kind(&row[last - 1]) == 0 {
        last -= 1;
    }
    (0, last.saturating_sub(1))
}

/// 按选区范围从终端取文本。
///
/// - 宽字符的续格(span = 0)跳过,不产生多余空格
/// - 空白格转成空格,行尾空格裁掉
/// - 软折行(`last_cell_was_wrapped`)的两行之间不插换行,硬换行才插 `\n`
pub fn selected_text(term: &Terminal, cols: usize, sel: &Selection) -> String {
    if sel.is_empty() || cols == 0 {
        return String::new();
    }
    let ((c0, r0), (c1, r1)) = sel.normalized();
    let screen = term.screen();
    let rows = screen.physical_rows.max(1);
    let sb = screen.scrollback_rows();
    let max_scroll = sb.saturating_sub(rows);

    // 想让 snapshot 从第 r0 行开始 → base = sb - rows - r0;但 base 不能
    // 超过 max_scroll(视口最多贴到底部),所以 clamp 之后重新算出实际顶行。
    let desired = sb.saturating_sub(rows + r0);
    let scroll = (desired as f32).min(max_scroll as f32);
    let base = scroll.ceil() as usize;
    let snap_top = sb.saturating_sub(rows + base);
    let (snap, _, _) = crate::render::snapshot(term, cols, scroll);
    if snap.rows.is_empty() {
        return String::new();
    }
    let last = snap.rows.len() - 1;
    let lo = r0.saturating_sub(snap_top).min(last);
    let hi = r1.saturating_sub(snap_top).min(last);
    if lo > hi {
        return String::new();
    }

    let mut parts: Vec<String> = Vec::with_capacity(hi - lo + 1);
    for r in lo..=hi {
        let row = &snap.rows[r];
        if row.is_empty() {
            parts.push(String::new());
            continue;
        }
        let from = if r == lo {
            c0.min(row.len() - 1)
        } else {
            0
        };
        let to = if r == hi {
            c1.min(row.len() - 1)
        } else {
            row.len() - 1
        };
        let mut s = String::new();
        for cell in &row[from..=to] {
            if cell.span == 0 {
                continue;
            }
            if cell.text.is_empty() {
                s.push(' ');
            } else {
                s.push_str(&cell.text);
            }
        }
        parts.push(s.trim_end_matches(' ').to_string());
    }

    let mut out = String::new();
    for (i, p) in parts.iter().enumerate() {
        out.push_str(p);
        if i + 1 < parts.len() && !snap.wrapped.get(lo + i).copied().unwrap_or(false) {
            out.push('\n');
        }
    }
    out
}

/// 调试/测试用:构造一格
#[cfg(test)]
fn cell(text: &str) -> SnapCell {
    use crate::render::Rgb;
    let black = Rgb { r: 0, g: 0, b: 0 };
    SnapCell {
        text: text.to_string(),
        span: 1,
        fg: black,
        bg: black,
        underline: Underline::None,
        underline_color: black,
        strike: false,
        italic: false,
        bold: false,
        blink: false,
        overline: false,
        invisible: false,
        images: Vec::new(),
    }
}

#[cfg(test)]
fn row_of(chars: &[&str]) -> Vec<SnapCell> {
    chars.iter().map(|s| cell(s)).collect()
}

#[cfg(test)]
fn test_term(rows: usize, cols: usize) -> Terminal {
    use crate::config::AppConfig;
    use wezterm_term::TerminalSize;
    let size = TerminalSize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
        dpi: 96,
    };
    crate::term::create_terminal(size, &AppConfig::default(), Box::new(std::io::sink()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_count_reaches_triple_and_resets_on_distance() {
        let mut st = SelectionState::new();
        let t = Instant::now();
        assert_eq!(st.press(10.0, 10.0, t), Mode::Cell);
        assert_eq!(
            st.press(11.0, 10.0, t + Duration::from_millis(100)),
            Mode::Word
        );
        assert_eq!(
            st.press(10.0, 11.0, t + Duration::from_millis(200)),
            Mode::Line
        );
        // 第 4 击封顶为 3;换位置/超时后重新从 1 开始
        assert_eq!(
            st.press(10.0, 11.0, t + Duration::from_millis(300)),
            Mode::Line
        );
        assert_eq!(st.press(200.0, 10.0, t + Duration::from_millis(400)), Mode::Cell);
        assert_eq!(
            st.press(10.0, 10.0, t + Duration::from_secs(2)),
            Mode::Cell
        );
    }

    #[test]
    fn reverse_drag_normalizes() {
        let s = Selection {
            anchor: (12, 40),
            head: (3, 38),
            mode: Mode::Cell,
            dragging: false,
        };
        assert_eq!(s.normalized(), ((3, 38), (12, 40)));
        assert!(!s.is_empty());
        let click = Selection::new((3, 7), Mode::Cell);
        assert!(click.is_empty());
    }

    #[test]
    fn word_and_line_bounds() {
        let row = row_of(&["f", "o", "o", "-", "b", "a", "r", " ", "(", "x", ")", " "]);
        assert_eq!(word_bounds(&row, 1), (0, 6)); // foo-bar 连成一词
        assert_eq!(word_bounds(&row, 7), (7, 7)); // 点在空白上
        assert_eq!(word_bounds(&row, 9), (9, 9)); // 标点自成一格
        assert_eq!(line_bounds(&row), (0, 10)); // 行尾空格不进选区
        let blank = row_of(&[" ", " ", " "]);
        assert_eq!(line_bounds(&blank), (0, 0));
    }

    #[test]
    fn selected_text_handles_wide_chars_and_trailing_blanks() {
        let mut t = test_term(6, 20);
        t.advance_bytes("ab cd\r\n你好 X\r\n".as_bytes());
        let top = {
            let s = t.screen();
            s.scrollback_rows().saturating_sub(s.physical_rows)
        };
        let sel = Selection {
            anchor: (0, top),
            head: (19, top + 1),
            mode: Mode::Cell,
            dragging: false,
        };
        assert_eq!(selected_text(&t, 20, &sel), "ab cd\n你好 X");
    }

    #[test]
    fn selected_text_joins_soft_wrapped_lines() {
        let mut t = test_term(4, 10);
        t.advance_bytes(b"abcdefghijklmnopq\r\n");
        let top = {
            let s = t.screen();
            s.scrollback_rows().saturating_sub(s.physical_rows)
        };
        let sel = Selection {
            anchor: (0, top),
            head: (9, top + 1),
            mode: Mode::Cell,
            dragging: false,
        };
        let text = selected_text(&t, 10, &sel);
        assert_eq!(text, "abcdefghijklmnopq", "折行不应插入换行: {text:?}");
    }

    #[test]
    fn selected_text_reads_scrollback_rows() {
        let mut t = test_term(4, 20);
        t.advance_bytes(
            b"first-line\r\nsecond-line\r\nthird\r\nfourth\r\nfifth\r\nsixth\r\nseventh\r\n",
        );
        // 第 0 行早已滚出视口,仍能按绝对行号取回原文
        let sel = Selection {
            anchor: (0, 0),
            head: (19, 0),
            mode: Mode::Cell,
            dragging: false,
        };
        assert_eq!(selected_text(&t, 20, &sel), "first-line");
    }

    #[test]
    fn highlight_is_drawn_behind_glyphs() {
        use crate::config::AppConfig;
        use crate::frame::{Frame, Highlight, Item};
        use crate::render::{Rgb, Renderer};
        let mut t = test_term(4, 12);
        t.advance_bytes(b"hello world\r\n");
        let mut r = Renderer::new(&AppConfig::default(), 1.0).unwrap();
        let (snap, cur, pal) = crate::render::snapshot(&t, 12, 0.0);
        let w = (12.0 * r.cell_w + 2.0 * r.pad_x) as u32;
        let h = (4.0 * r.cell_h + 2.0 * r.pad_y) as u32;
        let mut frame = Frame::default();
        let hl = Highlight {
            start: (1, 0),
            end: (4, 0),
            color: Rgb { r: 58, g: 110, b: 165 },
            alpha: 0.45,
        };
        r.build_frame_with_selection(
            &mut frame,
            w,
            h,
            &snap,
            &cur,
            &pal,
            None,
            pal.cursor_bg,
            true,
            Some(hl),
            true,
        );
        let first_glyph = frame
            .items
            .iter()
            .position(|it| matches!(it, Item::Glyph { .. }))
            .expect("这一帧应该有字形");
        let (idx, item) = frame
            .items
            .iter()
            .enumerate()
            .find(|(_, it)| matches!(it, Item::Rect { alpha, .. } if (*alpha - 0.45).abs() < 1e-6))
            .expect("缺少选区高亮矩形");
        assert!(idx < first_glyph, "高亮必须画在字形之前,否则会染字");
        if let Item::Rect { x, w: rw, .. } = item {
            assert!((*x - (r.pad_x + r.cell_w) as f32).abs() < 0.01);
            assert!((*rw - (4.0 * r.cell_w) as f32).abs() < 0.01);
        } else {
            unreachable!();
        }
    }

    #[test]
    fn snapshot_records_wrap_flags() {
        let mut t = test_term(4, 10);
        t.advance_bytes(b"abcdefghijklmnopq\r\n");
        let (snap, _, _) = crate::render::snapshot(&t, 10, 0.0);
        let _: &ScreenSnapshot = &snap;
        assert_eq!(snap.wrapped.len(), snap.rows.len());
        assert!(snap.wrapped.iter().any(|&w| w));
    }
}
