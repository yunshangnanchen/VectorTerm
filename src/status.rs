//! 状态栏文本:把 `wezterm.format` 产出的 SGR 串解析成带颜色的片段。
//!
//! wezterm 的 `window:set_right_status()` 收的是一个**已经带转义序列的字符串** ——
//! `wezterm.format({ ... })` 返回的就是它。所以这里必须能读懂 SGR:不解析的话
//! `{ Foreground = { Color = '#8A93A8' } }` 只能退化成纯文本,颜色全丢。
//!
//! 只实现状态栏用得上的那一小撮 SGR:`m` 里的 0/1/3/4/22/23/24 与
//! 30–37 / 90–97 / 40–47 / 100–107 / 38;5;n / 48;5;n / 38;2;r;g;b / 48;2;r;g;b /
//! 39 / 49。其余 CSI 与 OSC 直接跳过,不会把文字吞掉。

use crate::render::Rgb;

/// 一段同样式文字
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Span {
    pub text: String,
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

impl Span {
    #[cfg(test)]
    fn is_plain(&self) -> bool {
        self.fg.is_none() && self.bg.is_none() && !self.bold && !self.italic && !self.underline
    }
}

/// 当前生效的 SGR 状态(片段之间靠它判断要不要断成两段)
#[derive(Clone, Debug, Default, PartialEq)]
struct Style {
    fg: Option<Rgb>,
    bg: Option<Rgb>,
    bold: bool,
    italic: bool,
    underline: bool,
}

/// 把带 SGR 的字符串切成片段。`ansi` 是 256 色调色板,只有 `38;5;n` 会用到它。
pub fn parse(text: &str, ansi: &[Rgb; 256]) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let mut style = Style::default();
    let mut buf = String::new();
    let mut chars = text.chars().peekable();

    // 样式变了就把攒下的文字按旧样式收尾
    macro_rules! flush {
        () => {
            if !buf.is_empty() {
                out.push(Span {
                    text: std::mem::take(&mut buf),
                    fg: style.fg,
                    bg: style.bg,
                    bold: style.bold,
                    italic: style.italic,
                    underline: style.underline,
                });
            }
        };
    }

    while let Some(ch) = chars.next() {
        match ch {
            '\u{1b}' => match chars.next() {
                // CSI:参数串 + 终止符
                Some('[') => {
                    let mut params = String::new();
                    let mut fin = None;
                    for c in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&c) {
                            fin = Some(c);
                            break;
                        }
                        params.push(c);
                    }
                    if fin == Some('m') {
                        flush!();
                        apply_sgr(&params, ansi, &mut style);
                    }
                }
                // OSC:一直吃到 BEL 或 ST,内容(标题、链接)状态栏用不上
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                // 其它两字节转义(ESC 7 / ESC 8 之类)
                _ => {}
            },
            _ => buf.push(ch),
        }
    }
    flush!();
    out
}

fn apply_sgr(params: &str, ansi: &[Rgb; 256], style: &mut Style) {
    // 空参数等价于 0;`38;2;r;g;b` 这类要按顺序吃掉后续参数。
    // 冒号写法(`38:2::r:g:b`)不常见,标记成不认识,交给后面跳过。
    let mut nums: Vec<Option<u32>> = params
        .split(';')
        .map(|s| {
            if s.is_empty() {
                Some(0)
            } else if s.contains(':') {
                None
            } else {
                s.parse().ok()
            }
        })
        .collect();
    if nums.is_empty() {
        nums.push(Some(0));
    }

    let mut i = 0;
    while i < nums.len() {
        let Some(n) = nums[i] else {
            i += 1;
            continue;
        };
        match n {
            0 => *style = Style::default(),
            1 => style.bold = true,
            3 => style.italic = true,
            4 => style.underline = true,
            22 => style.bold = false,
            23 => style.italic = false,
            24 => style.underline = false,
            30..=37 => style.fg = Some(ansi[(n - 30) as usize]),
            39 => style.fg = None,
            40..=47 => style.bg = Some(ansi[(n - 40) as usize]),
            49 => style.bg = None,
            90..=97 => style.fg = Some(ansi[(n - 90 + 8) as usize]),
            100..=107 => style.bg = Some(ansi[(n - 100 + 8) as usize]),
            38 | 48 => {
                let target = if n == 38 { 0 } else { 1 };
                let color = match nums.get(i + 1).copied().flatten() {
                    // 38;5;n
                    Some(5) => {
                        let idx = nums.get(i + 2).copied().flatten().unwrap_or(0) as usize;
                        i += 2;
                        Some(ansi[idx.min(255)])
                    }
                    // 38;2;r;g;b
                    Some(2) => {
                        let r = nums.get(i + 2).copied().flatten().unwrap_or(0) as u8;
                        let g = nums.get(i + 3).copied().flatten().unwrap_or(0) as u8;
                        let b = nums.get(i + 4).copied().flatten().unwrap_or(0) as u8;
                        i += 4;
                        Some(Rgb { r, g, b })
                    }
                    _ => None,
                };
                if let Some(c) = color {
                    if target == 0 {
                        style.fg = Some(c);
                    } else {
                        style.bg = Some(c);
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
}

/// 这些片段一共占几格(等宽网格,中文按两格算)
pub fn cells(spans: &[Span]) -> usize {
    spans
        .iter()
        .map(|s| crate::tabbar::text_cells(&s.text))
        .sum()
}

/// 这些片段一共占多宽,单位是"默认字宽"(`space` = 一个空格算几个字宽)。
///
/// 状态栏里连续空格常被拿来做分隔带(比如 `'   ·   '`),想把这些空隙收窄
/// 又不想改脚本,就让 `status_bar.space_width` 乘一下空格的推进量:
/// `space = 1.0` 时和 [`cells`] 完全一致,`0.5` 时空格只占半格,`0` = 空格不占位置。
pub fn width_units(spans: &[Span], space: f64) -> f64 {
    let space = space.max(0.0);
    let mut w = 0.0;
    for span in spans {
        for ch in span.text.chars() {
            if ch == ' ' {
                w += space;
            } else {
                w += crate::tabbar::char_width(ch) as f64;
            }
        }
    }
    w
}

/// 从左边截到 `max` 格,超出的丢弃。状态栏文字宁可截断也不换行 ——
/// wezterm 的做法是把标签区挤窄,状态本身不折行。
pub fn head_to_cells(spans: &[Span], max: usize) -> Vec<Span> {
    let mut out = Vec::new();
    let mut used = 0usize;
    for span in spans {
        let mut text = String::new();
        for ch in span.text.chars() {
            let w = crate::tabbar::char_width(ch);
            if used + w > max {
                if !text.is_empty() {
                    out.push(Span {
                        text,
                        ..span.clone()
                    });
                }
                return out;
            }
            text.push(ch);
            used += w;
        }
        if !text.is_empty() {
            out.push(Span {
                text,
                ..span.clone()
            });
        }
    }
    out
}

/// 从右边保留 `max` 格。右侧状态栏是右对齐的,地方不够时该丢的是**开头**
/// (`2026/11/07 周四` 被挤掉一半,留时间比留年份有用)。
pub fn tail_to_cells(spans: &[Span], max: usize) -> Vec<Span> {
    let total = cells(spans);
    if total <= max {
        return spans.to_vec();
    }
    let drop = total - max;
    let mut out: Vec<Span> = Vec::new();
    let mut seen = 0usize;
    for span in spans {
        let mut text = String::new();
        for ch in span.text.chars() {
            let w = crate::tabbar::char_width(ch);
            if seen + w <= drop {
                seen += w;
                continue;
            }
            text.push(ch);
        }
        if !text.is_empty() {
            out.push(Span {
                text,
                ..span.clone()
            });
        }
    }
    out
}

/// 从右边保留 `max` 个字宽的宽度(空格按 `space` 折算)。
///
/// 和 [`tail_to_cells`] 同一个思路,只是宽度口径带上了空格缩放 ——
/// 收窄空格之后能塞下更多字,截断点也得跟着放宽。
pub fn tail_to_units(spans: &[Span], max: f64, space: f64) -> Vec<Span> {
    let space = space.max(0.0);
    let total = width_units(spans, space);
    if total <= max {
        return spans.to_vec();
    }
    let mut drop = total - max;
    let mut out: Vec<Span> = Vec::new();
    for span in spans {
        let mut text = String::new();
        for ch in span.text.chars() {
            let w = if ch == ' ' {
                space
            } else {
                crate::tabbar::char_width(ch) as f64
            };
            if drop > 0.0 {
                drop -= w;
                continue;
            }
            text.push(ch);
        }
        if !text.is_empty() {
            out.push(Span {
                text,
                ..span.clone()
            });
        }
    }
    out
}

// ---------------------------------------------------------------- 电量

/// 一块电池的当前状态,对应 `wezterm.battery_info()` 的一项
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Battery {
    /// 0.0 ~ 1.0
    pub state_of_charge: f32,
    pub state: BatteryState,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BatteryState {
    Charging,
    Discharging,
    Full,
    Unknown,
}

impl BatteryState {
    /// wezterm 用的就是这几个字符串(Lua 侧拿去做比较)
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Charging => "Charging",
            Self::Discharging => "Discharging",
            Self::Full => "Full",
            Self::Unknown => "Unknown",
        }
    }
}

/// 读本机电池。台式机没有电池时返回空列表 —— 和 wezterm 一致,
/// 示例里那句 `for _, b in ipairs(wezterm.battery_info())` 自然不会进循环。
#[cfg(windows)]
pub fn battery_info() -> Vec<Battery> {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

    let mut st = SYSTEM_POWER_STATUS {
        ACLineStatus: 255,
        BatteryFlag: 128,
        BatteryLifePercent: 255,
        SystemStatusFlag: 0,
        BatteryLifeTime: u32::MAX,
        BatteryFullLifeTime: u32::MAX,
    };
    // SAFETY: 结构体按 Win32 定义原样传指针,函数只写这块内存
    let ok = unsafe { GetSystemPowerStatus(&mut st) };
    if ok == 0 || st.BatteryLifePercent == 255 {
        return Vec::new();
    }
    const UNKNOWN: u8 = 4;
    const NO_BATTERY: u8 = 128;
    const CHARGING: u8 = 8;
    let flag = st.BatteryFlag;
    if flag & NO_BATTERY != 0 || flag & UNKNOWN != 0 {
        return Vec::new();
    }
    let state = if flag & CHARGING != 0 {
        BatteryState::Charging
    } else if st.ACLineStatus == 255 {
        // 系统自己也不知道接没接市电(某些虚拟机上会这样)
        BatteryState::Unknown
    } else if st.ACLineStatus == 1 && st.BatteryLifePercent >= 100 {
        BatteryState::Full
    } else {
        BatteryState::Discharging
    };
    vec![Battery {
        state_of_charge: st.BatteryLifePercent as f32 / 100.0,
        state,
    }]
}

#[cfg(not(windows))]
pub fn battery_info() -> Vec<Battery> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> [Rgb; 256] {
        let mut p = [Rgb { r: 0, g: 0, b: 0 }; 256];
        for (i, slot) in p.iter_mut().enumerate() {
            *slot = Rgb {
                r: i as u8,
                g: 0,
                b: 0,
            };
        }
        p
    }

    #[test]
    fn plain_text_is_one_span() {
        let s = parse("hello", &palette());
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].text, "hello");
        assert!(s[0].is_plain());
    }

    #[test]
    fn truecolor_foreground_splits_spans() {
        // wezterm.format({ {Foreground={Color='#ff0000'}}, {Text='a'}, {Text='b'} })
        let s = parse("\u{1b}[38;2;255;0;0ma\u{1b}[0mb", &palette());
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].text, "a");
        assert_eq!(s[0].fg, Some(Rgb { r: 255, g: 0, b: 0 }));
        assert_eq!(s[1].text, "b");
        assert_eq!(s[1].fg, None, "SGR 0 要能把颜色清回去");
    }

    #[test]
    fn indexed_and_basic_colors_resolve_through_palette() {
        let p = palette();
        let s = parse("\u{1b}[38;5;9mx\u{1b}[0m\u{1b}[32my", &p);
        assert_eq!(s[0].fg, Some(p[9]));
        assert_eq!(s[1].fg, Some(p[2]));
    }

    #[test]
    fn attributes_are_kept() {
        let s = parse("\u{1b}[1;3;4mbold\u{1b}[22mplain", &palette());
        assert!(s[0].bold && s[0].italic && s[0].underline);
        assert!(!s[1].bold, "22 只关粗体");
        assert!(s[1].italic, "斜体不受 22 影响");
    }

    #[test]
    fn unknown_csi_is_skipped_without_eating_text() {
        let s = parse("\u{1b}[2Jhello\u{1b}[?25l!", &palette());
        assert_eq!(s[0].text, "hello!");
    }

    #[test]
    fn cell_counts_use_display_width() {
        let s = parse("ab中文", &palette());
        assert_eq!(cells(&s), 2 + 4);
    }

    /// 空格宽度倍数只缩放空格,不动其它字符的格数;截断也按同一口径算。
    #[test]
    fn space_width_scales_only_spaces() {
        let s = parse("a   b", &palette());
        assert_eq!(width_units(&s, 1.0), 5.0, "3 个空格各占一格");
        assert!((width_units(&s, 0.5) - 3.5).abs() < 1e-9, "3 个空格各半格");
        assert!((width_units(&s, 0.0) - 2.0).abs() < 1e-9, "0 = 空格不占位置");

        // 汉字仍是两格,只有空格跟着缩
        let cjk = parse("中  文", &palette());
        let spaces = cjk
            .iter()
            .map(|x| x.text.chars().filter(|c| *c == ' ').count())
            .sum::<usize>() as f64;
        let full = width_units(&cjk, 1.0);
        assert!((full - (4.0 + spaces)).abs() < 1e-9, "两个汉字占四格");
        assert!((width_units(&cjk, 0.5) - (full - spaces * 0.5)).abs() < 1e-9);

        // 收窄空格后能塞下更多字:4 格的预算里,1.0 要把开头的 `a` 丢掉,
        // 0.5 时整段都放得下
        let tight: String = tail_to_units(&s, 4.0, 1.0)
            .iter()
            .map(|x| x.text.as_str())
            .collect();
        let loose: String = tail_to_units(&s, 4.0, 0.5)
            .iter()
            .map(|x| x.text.as_str())
            .collect();
        assert_eq!(tight, "   b", "1.0 只留得下四格");
        assert_eq!(loose, "a   b", "空格减半后整段都放得下");
    }

    #[test]
    fn head_and_tail_truncation() {
        let s = parse("\u{1b}[31mab\u{1b}[0mcd", &palette());
        let head = head_to_cells(&s, 3);
        assert_eq!(
            head.iter().map(|x| x.text.as_str()).collect::<String>(),
            "abc"
        );
        assert_eq!(head[0].fg, Some(palette()[1]));

        let tail = tail_to_cells(&s, 2);
        assert_eq!(
            tail.iter().map(|x| x.text.as_str()).collect::<String>(),
            "cd"
        );
        assert!(
            tail.iter().all(|x| x.fg.is_none()),
            "留下的这段本来就没颜色"
        );

        // 放得下就原样返回
        assert_eq!(tail_to_cells(&s, 4).len(), s.len());
    }

    #[test]
    fn wide_chars_are_not_split_across_the_cut() {
        let s = parse("中文", &palette());
        assert_eq!(head_to_cells(&s, 3)[0].text, "中", "剩一格放不下第二个汉字");
        assert_eq!(tail_to_cells(&s, 1)[0].text, "文");
    }
}
