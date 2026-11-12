//! 标签栏的纯几何布局(不依赖窗口/渲染器,便于单测)
//!
//! 宽度分配跟 wezterm 一致:每个标签先按 `tab_max_width` 封顶,剩余宽度
//! 均分;标签多到放不下时继续压缩(wezterm 也是挤压而不是横向滚动)。
//! 新建按钮固定在标签后面,不参与压缩。

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

/// 标题栏按钮。只在**没有系统标题栏且启用了 `INTEGRATED_BUTTONS`** 时出现,
/// 具体有哪几枚、按什么顺序、靠哪边全由 wezterm 的配置决定。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowButton {
    Minimize,
    Maximize,
    Close,
}

/// `integrated_title_button_alignment`
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ButtonAlign {
    Left,
    #[default]
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    None,
    Tab(usize),
    /// 标签右侧的关闭按钮
    Close(usize),
    NewTab,
    /// 窗口控制按钮
    Window(WindowButton),
    /// 标题栏空白处:无边框时按住可拖动窗口,双击切换最大化
    Drag,
}

pub struct Layout {
    /// 整条栏的占位:包含靠窗口外框那侧的安全留白。背景条铺满它,
    /// DWM 负责把四角裁圆;CSD 模式下整块都是可拖动区。
    pub bar: Rect,
    /// 真正放标签 / 按钮的内容带。顶部栏时贴着内容区(在 bar 下半),
    /// 底部栏时同样贴着内容区(在 bar 上半);留白永远在靠窗口边框那侧。
    pub content: Rect,
    pub tabs: Vec<Rect>,
    /// 每个标签的关闭按钮;标签太窄时为 None(仍可用中键 / Ctrl+Shift+W 关)
    pub closes: Vec<Option<Rect>>,
    pub new_tab: Option<Rect>,
    /// 左侧状态条(`window:set_left_status`),没有就是 None
    pub left_status: Option<Rect>,
    /// 右侧状态条(`window:set_right_status`),没有就是 None
    pub right_status: Option<Rect>,
    /// 右侧的窗口按钮(顺序即从左到右);空 = 一枚都不画
    pub window_buttons: Vec<(WindowButton, Rect)>,
    /// 是否处于无系统标题栏模式(决定空白处能不能拖动窗口)
    pub csd: bool,
}

impl Layout {
    pub fn hit(&self, x: f64, y: f64) -> Hit {
        if !self.bar.contains(x, y) {
            return Hit::None;
        }
        // 窗口按钮压在标签右边,先判它们
        for (kind, r) in &self.window_buttons {
            if r.contains(x, y) {
                return Hit::Window(*kind);
            }
        }
        for (i, r) in self.tabs.iter().enumerate() {
            // 关闭按钮压在标签上面,先判它
            if self
                .closes
                .get(i)
                .copied()
                .flatten()
                .is_some_and(|c| c.contains(x, y))
            {
                return Hit::Close(i);
            }
            if r.contains(x, y) {
                return Hit::Tab(i);
            }
        }
        if self.new_tab.is_some_and(|r| r.contains(x, y)) {
            return Hit::NewTab;
        }
        // 剩下的是标题栏空白:无边框时用来拖动窗口
        if self.csd {
            Hit::Drag
        } else {
            Hit::None
        }
    }
}

/// 标签栏的尺寸参数。高度取 1.32 个行高(和 wezterm 的观感接近),
/// 行高很小时至少留 4px 余量,免得文字顶到边框。
#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    pub cell_w: f64,
    pub bar_h: f64,
}

impl Metrics {
    pub fn new(cell_w: f64, cell_h: f64) -> Self {
        let bar_h = (cell_h * 1.32).round().max(cell_h + 4.0);
        Self {
            cell_w: cell_w.max(1.0),
            bar_h,
        }
    }
}

/// 布局开关(对应配置里的 `tab_bar` 各项)
#[derive(Clone, Debug)]
pub struct Options {
    /// 标签栏内容左右各让出的物理像素。无边框 + DWM 圆角时,贴边的标签块
    /// 会被窗口圆角裁掉一个角,留出这个安全区后标签/按钮就不会进圆角三角区。
    /// 背景条仍然铺满整宽(圆角由 DWM 裁),所以外观上只是多了一点内边距。
    pub edge_inset: f64,
    /// 靠窗口外框那一侧的留白:顶部栏是到上边框的距离,底部栏是到下边框
    /// 的距离。和 `edge_inset` 一起构成四周对称的安全区,同时让 fancy tab
    /// 背向内容区的两个圆角完整露出来,不会被 DWM 圆角啃掉。
    pub top_inset: f64,
    pub at_bottom: bool,
    /// 单个标签的最大宽度(单元格数)
    pub max_width_cells: usize,
    pub show_new_tab: bool,
    pub show_close: bool,
    /// client-side decoration:没有系统标题栏,空白处可拖动窗口
    pub csd: bool,
    /// 要画进标签栏的窗口按钮(顺序即从左到右)。空 = 不画任何按钮,
    /// 对应 wezterm 的 `"RESIZE"` 或 `integrated_title_buttons = {}`。
    pub window_buttons: Vec<WindowButton>,
    /// 按钮靠左还是靠右
    pub button_alignment: ButtonAlign,
    /// 左侧状态条(`window:set_left_status`)的宽度,物理像素。0 = 不占位
    pub left_status_w: f64,
    /// 右侧状态条(`window:set_right_status`)的宽度,物理像素。0 = 不占位
    pub right_status_w: f64,
}

/// 两条状态条最多吃掉多少宽度。剩下的必须留给标签 —— 状态栏再重要也不能
/// 把标签挤没(wezterm 那边是把标签压窄,这里给个硬上限,免得窗口一窄
/// 就只剩状态栏)。
const STATUS_SHARE: f64 = 0.6;

/// 两条状态条按比例让位:装得下就原样,装不下就等比压缩。
fn fit_status(left: f64, right: f64, avail: f64) -> (f64, f64) {
    let left = left.max(0.0);
    let right = right.max(0.0);
    let total = left + right;
    let cap = (avail * STATUS_SHARE).max(0.0);
    if total <= cap || total <= 0.0 {
        return (left, right);
    }
    let k = cap / total;
    (left * k, right * k)
}

/// 靠外框那侧的留白上限。窗口被拖到很矮时不能让留白把标签挤没,
/// `tab_bar_height()` 和 `layout()` 都用它,保证内容区裁剪与绘制一致。
pub fn clamp_top_inset(top: f64, window_h: f64, tab_h: f64) -> f64 {
    // 只在窗口真的很矮时收缩:留白最多占窗口高度的 1/4,且不超过标签带
    // 高度的两倍(再大就喧宾夺主)。用户显式设的值正常完全生效。
    top.max(0.0).min(window_h * 0.25).min(tab_h * 2.0)
}

/// 单枚窗口按钮的宽度。按标签栏高度推,DPI 缩放时自动跟着变大
/// (Windows 原生标题栏按钮是 46 逻辑像素,和这个量级接近)。
pub fn window_button_width(m: &Metrics) -> f64 {
    (m.bar_h * 1.9).round().clamp(28.0, 64.0)
}

/// 图标边长(最小化横线 / 最大化方框 / 关闭叉的包围盒)
pub fn window_icon_size(bar_h: f64) -> f64 {
    (bar_h * 0.34).round().clamp(7.0, 14.0)
}

/// 关闭按钮的边长(正方形),随标签栏高度走
pub fn close_size(bar_h: f64, cell_w: f64) -> f64 {
    (bar_h * 0.58).round().clamp(11.0, (cell_w * 1.4).max(11.0))
}

/// 标签右侧的关闭按钮。标题至少留 4 格,不够就干脆不画
/// (中键点标签、Ctrl+Shift+W 仍然能关)
fn close_button(r: Rect, m: &Metrics) -> Option<Rect> {
    let d = close_size(m.bar_h, m.cell_w);
    if r.w < d + m.cell_w * 4.0 {
        return None;
    }
    let margin = (m.cell_w * 0.35).round().max(2.0);
    let x = r.x + r.w - d - margin;
    let y = r.y + ((r.h - d) / 2.0).round();
    Some(Rect { x, y, w: d, h: d })
}

/// `w`/`h` 是窗口的物理像素尺寸;`at_bottom` 决定标签栏贴哪一边。
pub fn layout(w: f64, h: f64, count: usize, m: &Metrics, opts: &Options) -> Layout {
    let at_bottom = opts.at_bottom;
    let show_new_tab = opts.show_new_tab;
    let tab_h = m.bar_h.min(h);
    // 靠窗口外框那侧的空带。整条栏 = 空带 + 标签内容带,内容带始终
    // 贴着终端内容区,这样 fancy tab 朝内容区一侧仍能连成一片。
    let top_inset = clamp_top_inset(opts.top_inset, h, tab_h);
    let total_h = (tab_h + top_inset).min(h);
    let bar_y = if at_bottom {
        (h - total_h).max(0.0)
    } else {
        0.0
    };
    let bar = Rect {
        x: 0.0,
        y: bar_y,
        w,
        h: total_h,
    };
    let content_y = if at_bottom { bar_y } else { bar_y + top_inset };
    let content = Rect {
        x: 0.0,
        y: content_y,
        w,
        h: (total_h - top_inset).max(0.0).min(tab_h),
    };
    // 圆角安全区:所有可点击/可见的内容都限制在 [inset, w-inset] 内,
    // 背景条仍是整宽(由 DWM 圆角裁剪)。
    let inset = opts.edge_inset.max(0.0).min(w * 0.25);
    let inner_x = inset;
    let inner_w = (w - 2.0 * inset).max(0.0);

    // ---- 窗口按钮(只有 INTEGRATED_BUTTONS 才有) ----
    // wezterm 的 "RESIZE" 只去掉系统标题栏,并不画按钮;按钮可以一枚都不画,
    // 也可以只留 'Close',还能换顺序/靠左。窗口窄到放不下就全不画。
    let btn_w = window_button_width(m);
    let total = btn_w * opts.window_buttons.len() as f64;
    let aligned_left = opts.button_alignment == ButtonAlign::Left;
    let window_buttons: Vec<(WindowButton, Rect)> =
        if !opts.window_buttons.is_empty() && inner_w >= total {
            let base_x = if aligned_left {
                inner_x
            } else {
                inner_x + inner_w - total
            };
            opts.window_buttons
                .iter()
                .enumerate()
                .map(|(i, k)| {
                    (
                        *k,
                        Rect {
                            x: base_x + i as f64 * btn_w,
                            y: content_y,
                            w: btn_w,
                            h: content.h,
                        },
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
    // 标签与新建按钮只在按钮让出来的区域里活动;左对齐时整体右移
    let reserved = window_buttons.iter().map(|(_, r)| r.w).sum::<f64>();
    // 状态条夹在标签两侧,靠窗口外框那侧还得避开窗口按钮
    let mut content_left = inner_x;
    let mut content_right = inner_x + inner_w;
    if !window_buttons.is_empty() {
        if aligned_left {
            content_left = inner_x + reserved;
        } else {
            content_right = inner_x + inner_w - reserved;
        }
    }
    let (lw, rw) = fit_status(
        opts.left_status_w,
        opts.right_status_w,
        content_right - content_left,
    );
    let left_status = if lw >= 1.0 {
        Some(Rect {
            x: content_left,
            y: content_y,
            w: lw,
            h: content.h,
        })
    } else {
        None
    };
    let right_status = if rw >= 1.0 {
        Some(Rect {
            x: content_right - rw,
            y: content_y,
            w: rw,
            h: content.h,
        })
    } else {
        None
    };
    let area_x = content_left + lw;
    let area_w = (content_right - rw - area_x).max(0.0);

    // 新建按钮:2.6 格宽,至少 24px,但不超过可用宽度的 1/4
    let new_w = if show_new_tab {
        (m.cell_w * 2.6).round().max(24.0).min(area_w * 0.25)
    } else {
        0.0
    };

    let max_w = (m.cell_w * opts.max_width_cells.max(3) as f64).max(m.cell_w);
    let usable = (area_w - new_w).max(0.0);
    let tab_w = if count > 0 {
        (usable / count as f64).min(max_w).max(1.0)
    } else {
        0.0
    };

    let mut tabs = Vec::with_capacity(count);
    let mut closes = Vec::with_capacity(count);
    for i in 0..count {
        let x = area_x + i as f64 * tab_w;
        // 最后一个标签不要盖住新建按钮
        if x >= area_x + area_w {
            break;
        }
        let width = tab_w.min(area_x + area_w - x);
        let r = Rect {
            x,
            y: content_y,
            w: width,
            h: content.h,
        };
        closes.push(if opts.show_close {
            close_button(r, m)
        } else {
            None
        });
        tabs.push(r);
    }

    let new_x = area_x + (count as f64 * tab_w).min(area_w);
    let new_tab = if show_new_tab && new_x < area_x + area_w {
        Some(Rect {
            x: new_x,
            y: content_y,
            w: (area_x + area_w - new_x).min(new_w),
            h: content.h,
        })
    } else {
        None
    };

    Layout {
        bar,
        content,
        tabs,
        closes,
        new_tab,
        left_status,
        right_status,
        window_buttons,
        csd: opts.csd,
    }
}

/// 标签标题最多能放下几个单元格(等宽字体)。
/// 序号和 ":" 也算进去,末尾留一格给省略号。
pub fn title_capacity(rect_w: f64, cell_w: f64, show_index: bool) -> usize {
    let cells = (rect_w / cell_w.max(1.0)).floor() as usize;
    let reserved = 2 /* 左右内边距 */ + if show_index { 3 } else { 0 };
    cells.saturating_sub(reserved).max(1)
}

/// 粗略的显示宽度:东亚宽字符占 2 格,其余 1 格。
/// 终端本身没有 unicode-width 依赖,这里覆盖常见区间就够了。
pub fn char_width(ch: char) -> usize {
    let c = ch as u32;
    let wide = (0x1100..=0x115F).contains(&c)      // 韩文字母
        || (0x2E80..=0xA4CF).contains(&c)          // CJK 部首 / 汉字 / 假名
        || (0xAC00..=0xD7A3).contains(&c)          // 韩文音节
        || (0xF900..=0xFAFF).contains(&c)          // CJK 兼容汉字
        || (0xFE30..=0xFE4F).contains(&c)          // CJK 兼容标点
        || (0xFF00..=0xFF60).contains(&c)          // 全角形式
        || (0xFFE0..=0xFFE6).contains(&c)          // 全角符号
        || (0x1F300..=0x1FAFF).contains(&c)        // emoji
        || (0x20000..=0x3FFFD).contains(&c); // CJK 扩展
    if wide {
        2
    } else {
        1
    }
}

pub fn text_cells(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

/// 按**单元格宽度**裁剪标题:中文按两格算,超出补省略号
pub fn fit_title_cells(title: &str, max_cells: usize) -> String {
    if text_cells(title) <= max_cells {
        return title.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in title.chars() {
        let w = char_width(ch);
        if used + w + 1 > max_cells {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> Metrics {
        Metrics::new(10.0, 20.0)
    }

    fn opts() -> Options {
        Options {
            edge_inset: 0.0,
            top_inset: 0.0,
            at_bottom: false,
            max_width_cells: 16,
            show_new_tab: true,
            show_close: true,
            csd: false,
            window_buttons: Vec::new(),
            button_alignment: ButtonAlign::Right,
            left_status_w: 0.0,
            right_status_w: 0.0,
        }
    }

    fn all_buttons() -> Vec<WindowButton> {
        vec![
            WindowButton::Minimize,
            WindowButton::Maximize,
            WindowButton::Close,
        ]
    }

    #[test]
    fn tabs_share_width_up_to_max() {
        let m = metrics();
        let l = layout(1000.0, 600.0, 3, &m, &opts());
        assert_eq!(l.tabs.len(), 3);
        // 每个标签不超过 16 格(160px)
        assert!(l.tabs.iter().all(|t| t.w <= 160.0 + 0.01));
        assert_eq!(
            l.new_tab.unwrap().x,
            l.tabs.last().unwrap().x + l.tabs.last().unwrap().w
        );
    }

    #[test]
    fn many_tabs_are_squeezed() {
        let m = metrics();
        let l = layout(300.0, 600.0, 10, &m, &opts());
        assert_eq!(l.tabs.len(), 10);
        let total: f64 = l.tabs.iter().map(|t| t.w).sum();
        assert!(total <= 300.0 + 0.01, "标签总宽不能超过窗口:{total}");
    }

    #[test]
    fn bottom_bar_sits_at_bottom() {
        let m = metrics();
        let l = layout(
            800.0,
            600.0,
            1,
            &m,
            &Options {
                at_bottom: true,
                ..opts()
            },
        );
        assert!((l.bar.y + l.bar.h - 600.0).abs() < 0.01);
        assert!((l.bar.y - (600.0 - m.bar_h)).abs() < 0.01);
    }

    #[test]
    fn hit_tests_tabs_and_new_button() {
        let m = metrics();
        let l = layout(600.0, 400.0, 2, &m, &opts());
        assert_eq!(l.hit(5.0, 5.0), Hit::Tab(0));
        assert_eq!(l.hit(l.tabs[1].x + 1.0, 5.0), Hit::Tab(1));
        let nb = l.new_tab.unwrap();
        assert_eq!(l.hit(nb.x + 2.0, 5.0), Hit::NewTab);
        assert_eq!(l.hit(5.0, 300.0), Hit::None);
    }

    /// 默认(右对齐)的三枚按钮贴右边,标签区让出宽度,空白处可拖动
    #[test]
    fn csd_puts_window_buttons_on_the_right() {
        let m = metrics();
        let csd = Options {
            csd: true,
            window_buttons: all_buttons(),
            ..opts()
        };
        let l = layout(600.0, 400.0, 2, &m, &csd);
        assert_eq!(l.window_buttons.len(), 3, "应有最小化 / 最大化 / 关闭三枚");
        let kinds: Vec<_> = l.window_buttons.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                WindowButton::Minimize,
                WindowButton::Maximize,
                WindowButton::Close
            ]
        );
        let close = l.window_buttons.last().unwrap().1;
        assert!(
            (close.x + close.w - 600.0).abs() < 0.01,
            "关闭按钮要贴右边缘:{close:?}"
        );
        let btns_left = l.window_buttons[0].1.x;
        assert!(
            l.tabs.iter().all(|t| t.x + t.w <= btns_left + 0.01),
            "标签不能盖住窗口按钮"
        );
        assert_eq!(l.hit(btns_left - 1.0, 5.0), Hit::Drag, "空白处应可拖动窗口");
        let minr = l.window_buttons[0].1;
        assert_eq!(
            l.hit(minr.x + 2.0, minr.y + 2.0),
            Hit::Window(WindowButton::Minimize)
        );
    }

    /// wezterm 的 "RESIZE" / `integrated_title_buttons = {}`:一枚按钮都不画,
    /// 但顶部这条栏仍在,标签占满整宽、空白处照样能拖动窗口。
    #[test]
    fn buttons_can_be_hidden_without_removing_the_bar() {
        let m = metrics();
        let l = layout(
            600.0,
            400.0,
            2,
            &m,
            &Options {
                csd: true,
                ..opts()
            },
        );
        assert!(l.window_buttons.is_empty());
        assert_eq!(l.tabs.len(), 2);
        assert!((l.tabs.iter().map(|t| t.w).sum::<f64>() + l.new_tab.unwrap().w) <= 600.0);
        assert_eq!(l.hit(580.0, 5.0), Hit::Drag);
        assert_eq!(l.hit(5.0, 5.0), Hit::Tab(0));
    }

    /// `integrated_title_buttons = { 'Close' }`:只留一枚,标签不盖住它
    #[test]
    fn a_single_button_keeps_its_slot() {
        let m = metrics();
        let l = layout(
            600.0,
            400.0,
            2,
            &m,
            &Options {
                csd: true,
                window_buttons: vec![WindowButton::Close],
                ..opts()
            },
        );
        assert_eq!(l.window_buttons.len(), 1);
        let (kind, r) = l.window_buttons[0];
        assert_eq!(kind, WindowButton::Close);
        assert!((r.x + r.w - 600.0).abs() < 0.01);
        assert_eq!(l.hit(r.x + 2.0, 5.0), Hit::Window(WindowButton::Close));
        assert!(l.tabs.iter().all(|t| t.x + t.w <= r.x + 0.01));
    }

    /// 按钮可乱序、可靠左:靠左时标签区整体右移,不压在按钮上
    #[test]
    fn buttons_can_be_reordered_and_left_aligned() {
        let m = metrics();
        let l = layout(
            600.0,
            400.0,
            2,
            &m,
            &Options {
                csd: true,
                window_buttons: vec![
                    WindowButton::Close,
                    WindowButton::Maximize,
                    WindowButton::Minimize,
                ],
                button_alignment: ButtonAlign::Left,
                ..opts()
            },
        );
        let kinds: Vec<_> = l.window_buttons.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                WindowButton::Close,
                WindowButton::Maximize,
                WindowButton::Minimize
            ]
        );
        assert!((l.window_buttons[0].1.x - 0.0).abs() < 0.01);
        let reserved: f64 = l.window_buttons.iter().map(|(_, r)| r.w).sum();
        assert!(
            l.tabs.iter().all(|t| t.x >= reserved - 0.01),
            "标签区应该从按钮右边开始"
        );
        assert_eq!(
            l.hit(l.window_buttons[0].1.x + 2.0, 5.0),
            Hit::Window(WindowButton::Close)
        );
    }

    /// 有系统标题栏时不该出现窗口按钮,空白处也不是拖动区
    #[test]
    fn no_csd_keeps_old_behaviour() {
        let m = metrics();
        let l = layout(600.0, 400.0, 2, &m, &opts());
        assert!(l.window_buttons.is_empty());
        assert_eq!(
            l.hit(500.0, 5.0),
            Hit::None,
            "有原生标题栏时标签栏空白不拖窗口"
        );
    }

    /// 关闭按钮落在标签右端内部,并且命中优先级高于标签本身
    #[test]
    fn close_button_is_inside_tab_and_clickable() {
        let m = metrics();
        let l = layout(600.0, 400.0, 2, &m, &opts());
        let c = l.closes[0].expect("宽标签应该有关闭按钮");
        let t = l.tabs[0];
        assert!(
            c.x >= t.x && c.x + c.w <= t.x + t.w + 0.01,
            "按钮要落在标签内"
        );
        assert!(
            (c.y + c.h / 2.0 - (t.y + t.h / 2.0)).abs() <= 1.0,
            "按钮垂直居中"
        );
        assert_eq!(l.hit(c.x + c.w / 2.0, c.y + c.h / 2.0), Hit::Close(0));
        // 标签左侧仍然是切换
        assert_eq!(l.hit(t.x + 2.0, t.y + t.h / 2.0), Hit::Tab(0));
    }

    /// 标签被挤窄时宁可不画按钮,也不要把标题吃掉
    #[test]
    fn narrow_tabs_drop_close_button() {
        let m = metrics();
        let l = layout(300.0, 600.0, 10, &m, &opts());
        assert!(l.closes.iter().all(|c| c.is_none()));
        // 关掉按钮开关也一样
        let l2 = layout(
            600.0,
            600.0,
            2,
            &m,
            &Options {
                show_close: false,
                ..opts()
            },
        );
        assert!(l2.closes.iter().all(|c| c.is_none()));
    }

    /// 无边框 + DWM 圆角:内容左右各让出 inset,标签/按钮都不进圆角三角区,
    /// 但背景条仍铺满整宽(圆角由 DWM 裁)
    #[test]
    fn edge_inset_keeps_content_clear_of_rounded_corners() {
        let m = metrics();
        let inset = 12.0;
        let o = Options {
            edge_inset: inset,
            window_buttons: vec![
                WindowButton::Minimize,
                WindowButton::Maximize,
                WindowButton::Close,
            ],
            ..opts()
        };
        let w = 900.0;
        let l = layout(w, 600.0, 3, &m, &o);

        assert_eq!(l.bar.x, 0.0);
        assert_eq!(l.bar.w, w, "背景条仍铺满整宽");
        assert!(
            l.tabs.first().is_some_and(|t| t.x >= inset),
            "第一个标签不能贴着圆角"
        );
        let last_btn = l.window_buttons.last().unwrap().1;
        assert!(
            last_btn.x + last_btn.w <= w - inset + 0.001,
            "最右侧按钮不能贴着圆角"
        );
        if let Some(nt) = l.new_tab {
            assert!(nt.x + nt.w <= w - inset + 0.001);
        }
        // 左对齐时按钮同样从 inset 开始
        let left = Options {
            button_alignment: ButtonAlign::Left,
            ..o.clone()
        };
        let ll = layout(w, 600.0, 3, &m, &left);
        assert!(ll.window_buttons.first().unwrap().1.x >= inset);
        assert!(ll.tabs.first().unwrap().x >= inset);
    }

    /// 顶部栏:靠近窗口上边框的一侧留出和左右对称的空带,标签内容带
    /// 仍贴着终端内容区(fancy tab 朝内容区一侧保持直角)。
    #[test]
    fn top_inset_leaves_an_even_gap_above_the_tabs() {
        let m = metrics();
        let o = Options {
            edge_inset: 10.0,
            top_inset: 10.0,
            csd: true,
            window_buttons: all_buttons(),
            ..opts()
        };
        let l = layout(900.0, 600.0, 3, &m, &o);

        assert_eq!(l.bar.y, 0.0);
        assert!((l.bar.h - (m.bar_h + 10.0)).abs() < 0.01, "整条栏要长高");
        assert!((l.content.y - 10.0).abs() < 0.01, "标签到上边框的距离");
        assert!((l.content.h - m.bar_h).abs() < 0.01, "标签本身高度不变");
        assert!(
            (l.content.y + l.content.h - (l.bar.y + l.bar.h)).abs() < 0.01,
            "内容带另一侧必须接上终端内容区"
        );
        for r in l
            .tabs
            .iter()
            .chain(l.new_tab.iter())
            .chain(l.window_buttons.iter().map(|(_, r)| r))
        {
            assert!((r.y - l.content.y).abs() < 0.01, "所有控件都在内容带里");
            assert!((r.h - l.content.h).abs() < 0.01);
        }
        // 空带仍属于标题栏:无边框时照样能拖动窗口
        assert_eq!(l.hit(5.0, 2.0), Hit::Drag);
        assert_eq!(l.hit(l.tabs[0].x + 1.0, 11.0), Hit::Tab(0));
    }

    /// 底部栏:空带翻转到靠近窗口下边框的一侧,标签仍贴内容区上沿
    #[test]
    fn bottom_bar_inset_flips_to_the_window_edge() {
        let m = metrics();
        let o = Options {
            edge_inset: 10.0,
            top_inset: 10.0,
            at_bottom: true,
            ..opts()
        };
        let l = layout(900.0, 600.0, 2, &m, &o);
        assert!((l.bar.y + l.bar.h - 600.0).abs() < 0.01);
        assert!((l.content.y - l.bar.y).abs() < 0.01, "内容带贴内容区");
        assert!((l.content.y + l.content.h - 590.0).abs() < 0.01);
        assert!((l.tabs[0].y - l.content.y).abs() < 0.01);
    }

    /// 窗口被拖得很矮时,留白必须让位给标签本身
    #[test]
    fn tiny_window_clamps_the_top_inset() {
        let m = metrics();
        let capped = clamp_top_inset(100.0, 40.0, m.bar_h);
        assert!((capped - 10.0).abs() < 0.01);
        let l = layout(
            600.0,
            40.0,
            1,
            &m,
            &Options {
                top_inset: 100.0,
                ..opts()
            },
        );
        assert!((l.bar.h - (m.bar_h + capped)).abs() < 0.01);
        assert!((l.content.h - m.bar_h).abs() < 0.01);
    }

    /// 用户显式给的大留白要照数生效,不能被 clamp 悄悄砍掉
    #[test]
    fn explicit_large_top_inset_survives_clamp() {
        let m = metrics();
        let l = layout(
            900.0,
            600.0,
            1,
            &m,
            &Options {
                top_inset: 40.0,
                ..opts()
            },
        );
        assert!((l.content.y - 40.0).abs() < 0.01);
        assert!((l.bar.h - (m.bar_h + 40.0)).abs() < 0.01);
    }

    /// 左右状态栏各占一头,标签夹在中间;谁都不许压到别人身上
    #[test]
    fn status_areas_squeeze_the_tabs_from_both_ends() {
        let m = metrics();
        let l = layout(
            1000.0,
            600.0,
            3,
            &m,
            &Options {
                left_status_w: 100.0,
                right_status_w: 200.0,
                ..opts()
            },
        );
        let left = l.left_status.expect("应有左状态栏");
        let right = l.right_status.expect("应有右状态栏");
        assert!((left.x - 0.0).abs() < 0.01, "左状态栏贴左");
        assert!((left.w - 100.0).abs() < 0.01);
        assert!((right.x + right.w - 1000.0).abs() < 0.01, "右状态栏贴右");
        assert!((right.w - 200.0).abs() < 0.01);
        assert!(
            l.tabs
                .iter()
                .all(|t| t.x >= 100.0 - 0.01 && t.x + t.w <= 800.0 + 0.01),
            "标签必须落在两条状态栏之间"
        );
        // 状态栏所在的那一段不算标签区域
        assert_eq!(l.hit(left.x + 1.0, left.y + 1.0), Hit::None);
        assert_eq!(l.hit(right.x + 1.0, right.y + 1.0), Hit::None);
    }

    /// 窗口很窄时状态栏一起缩,而且缩得动 —— 不能让标签被挤成零宽
    #[test]
    fn narrow_window_shrinks_both_statuses() {
        let m = metrics();
        let l = layout(
            400.0,
            600.0,
            1,
            &m,
            &Options {
                left_status_w: 300.0,
                right_status_w: 300.0,
                ..opts()
            },
        );
        let left = l.left_status.unwrap();
        let right = l.right_status.unwrap();
        let total = left.w + right.w;
        assert!(
            total <= 400.0 * STATUS_SHARE + 0.01,
            "合计不超过 60%:{total}"
        );
        assert!((left.w - right.w).abs() < 0.01, "等比压缩");
        assert!(l.tabs[0].w > 0.0, "标签仍然要有宽度");
    }

    /// 有窗口按钮时,右状态栏要让开按钮,不能叠上去
    #[test]
    fn right_status_stops_before_window_buttons() {
        let m = metrics();
        let l = layout(
            1000.0,
            600.0,
            2,
            &m,
            &Options {
                csd: true,
                window_buttons: all_buttons(),
                right_status_w: 150.0,
                ..opts()
            },
        );
        let right = l.right_status.unwrap();
        let buttons_left = l.window_buttons[0].1.x;
        assert!(
            right.x + right.w <= buttons_left + 0.01,
            "右状态栏不能盖住窗口按钮:{right:?} / {buttons_left}"
        );
        // 按钮靠左时,左状态栏同样要让开
        let l2 = layout(
            1000.0,
            600.0,
            2,
            &m,
            &Options {
                csd: true,
                window_buttons: all_buttons(),
                button_alignment: ButtonAlign::Left,
                left_status_w: 150.0,
                ..opts()
            },
        );
        let left = l2.left_status.unwrap();
        let buttons_right = l2.window_buttons.last().unwrap().1;
        assert!(left.x >= buttons_right.x + buttons_right.w - 0.01);
    }

    #[test]
    fn titles_fit_with_ellipsis() {
        assert_eq!(fit_title_cells("hello", 10), "hello");
        assert_eq!(fit_title_cells("hello world", 8), "hello w…");
        assert_eq!(fit_title_cells("会话", 1), "…");
    }

    /// 标签栏是等宽网格:中文占两格,截断按格数而不是字符数
    #[test]
    fn wide_titles_are_measured_in_cells() {
        assert_eq!(char_width('中'), 2);
        assert_eq!(char_width('a'), 1);
        assert_eq!(text_cells("中文ab"), 6);
        assert_eq!(fit_title_cells("中文标题", 4), "中…");
        assert_eq!(fit_title_cells("abc", 4), "abc");
    }
}
