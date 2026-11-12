//! 历史搜索:把整个 scrollback 拍平成可检索的文本行,再映射回列坐标。
//!
//! 难点只在**坐标**:终端的一格里可能是一个宽字符、一个组合串,甚至图片,
//! 而搜索命中的是「第几个字符」。所以每行除了文本,还存一张 `cols` 表:
//! 第 i 个字符落在哪一列,末位多一个哨兵表示行尾列。这样高亮矩形、跳转
//! 后的横向定位都能直接换算,不会因为 CJK / emoji 偏掉。
//!
//! `wezterm-term` 的行是绝对物理行号(0..scrollback_rows),和 `selection.rs`
//! 用的是同一套口径 —— 视口滚动不会让匹配跟着跑。

use wezterm_term::Terminal;

/// 一处匹配。`row` 是绝对物理行号,`col..col+cols` 是它在那一行的列范围。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    pub row: usize,
    pub col: usize,
    pub cols: usize,
}

/// 一行的可检索文本 + 字符到列的映射。
/// `cols.len() == text.chars().count() + 1`,最后一项是行尾列(哨兵)。
#[derive(Clone, Debug)]
pub struct Line {
    pub text: String,
    pub cols: Vec<usize>,
}

impl Line {
    /// 单测/调试用:等宽 ASCII 行,每个字符占一列。
    #[cfg(test)]
    pub fn plain(s: &str) -> Self {
        let n = s.chars().count();
        Line {
            text: s.to_string(),
            cols: (0..=n).collect(),
        }
    }
}

/// 把终端的全部物理行拍平成可检索文本。
/// 宽字符按 `width()` 推进列,续格(0 宽)不会重复计数;空白格落成一个空格,
/// 这样「搜两个词中间的空」也能命中,和 `selection::selected_text` 一致。
pub fn collect_lines(term: &Terminal, cols: usize) -> Vec<Line> {
    let screen = term.screen();
    let sb = screen.scrollback_rows();
    let mut out: Vec<Line> = Vec::with_capacity(sb);
    screen.with_phys_lines(0..sb, |lines| {
        for line in lines {
            let mut text = String::new();
            let mut cmap: Vec<usize> = Vec::new();
            let mut col = 0usize;
            for c in line.visible_cells() {
                if col >= cols {
                    break;
                }
                let w = (c.width() as usize).max(1);
                let raw = c.str();
                if raw.is_empty() || raw == " " {
                    text.push(' ');
                    cmap.push(col);
                } else {
                    for ch in raw.chars() {
                        text.push(ch);
                        cmap.push(col);
                    }
                }
                col += w;
            }
            // 行尾哨兵:即使整行没内容也要给一个终点列
            cmap.push(col.max(1));
            out.push(Line { text, cols: cmap });
        }
    });
    out
}

fn same_char(a: char, b: char, case_sensitive: bool) -> bool {
    if case_sensitive {
        a == b
    } else {
        // 用迭代器比较而不是 to_ascii_lowercase:非 ASCII(日文/西里尔)也能忽略大小写
        a.to_lowercase().eq(b.to_lowercase())
    }
}

/// 在单行里找**不重叠**的所有匹配。命中会追加到 `out`,到 `max` 就收工,
/// 免得 `cat 一个巨大的日志` 时一次搜索分配上百万个对象。
pub fn find_in_line(
    line: &Line,
    query: &str,
    case_sensitive: bool,
    row: usize,
    max: usize,
    out: &mut Vec<Match>,
) {
    if query.is_empty() || out.len() >= max {
        return;
    }
    let q: Vec<char> = query.chars().collect();
    let chars: Vec<char> = line.text.chars().collect();
    if q.is_empty() || q.len() > chars.len() || line.cols.len() < chars.len() + 1 {
        return;
    }
    let end = line.cols[chars.len()];
    let mut i = 0usize;
    while i + q.len() <= chars.len() {
        let mut ok = true;
        for j in 0..q.len() {
            if !same_char(chars[i + j], q[j], case_sensitive) {
                ok = false;
                break;
            }
        }
        if ok {
            let c0 = line.cols[i];
            let c1 = if i + q.len() < line.cols.len() {
                line.cols[i + q.len()]
            } else {
                end
            }
            .max(c0 + 1);
            out.push(Match {
                row,
                col: c0,
                cols: c1 - c0,
            });
            if out.len() >= max {
                return;
            }
            i += q.len(); // 不重叠
        } else {
            i += 1;
        }
    }
}

pub fn find_matches(lines: &[Line], query: &str, case_sensitive: bool, max: usize) -> Vec<Match> {
    let mut out = Vec::new();
    if query.is_empty() || max == 0 {
        return out;
    }
    for (row, line) in lines.iter().enumerate() {
        find_in_line(line, query, case_sensitive, row, max, &mut out);
        if out.len() >= max {
            break;
        }
    }
    out
}

/// 跳到某处匹配时视口该摆在哪:让匹配行尽量落在视口中间。
/// `view_top = sb - rows - scroll`,所以 `scroll = sb - rows - (row - rows/2)`。
pub fn scroll_for_match(m: Match, sb: usize, rows: usize, max_scroll: f64) -> f64 {
    let half = (rows / 2) as isize;
    let top = ((m.row as isize) - half).max(0) as usize;
    (sb.saturating_sub(rows + top) as f64).clamp(0.0, max_scroll.max(0.0))
}

/// 搜索栏的运行时状态。作用于**活动 pane**,切换标签/分屏时由调用方重算。
#[derive(Debug, Clone)]
pub struct SearchState {
    pub active: bool,
    pub query: String,
    pub case_sensitive: bool,
    pub matches: Vec<Match>,
    pub current: usize,
    /// 终端又输出了新内容:下次用到结果前要重算一次
    pub dirty: bool,
}

impl SearchState {
    pub fn new(case_sensitive: bool) -> Self {
        Self {
            active: false,
            query: String::new(),
            case_sensitive,
            matches: Vec::new(),
            current: 0,
            dirty: false,
        }
    }

    pub fn open(&mut self) {
        self.active = true;
        self.query.clear();
        self.matches.clear();
        self.current = 0;
        self.dirty = true;
    }

    pub fn close(&mut self) {
        self.active = false;
        self.query.clear();
        self.matches.clear();
        self.current = 0;
        self.dirty = false;
    }

    pub fn set_query(&mut self, q: String) {
        self.query = q;
        self.current = 0;
        self.dirty = true;
    }

    /// 重算结果,并尽量让「当前项」停在原来那一行附近(终端又滚出新行时,
    /// 不至于按一下回车就跳到完全不相干的位置)。
    pub fn refresh(&mut self, lines: &[Line], max: usize) {
        let old = self.matches.get(self.current).copied();
        self.matches = find_matches(lines, &self.query, self.case_sensitive, max);
        self.current = old
            .map(|o| {
                self.matches
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, m)| (m.row as isize - o.row as isize).abs())
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        self.dirty = false;
    }

    pub fn current_match(&self) -> Option<Match> {
        self.matches.get(self.current).copied()
    }

    /// Enter / Shift+Enter。`delta` 为正 = 下一个(更早的行按搜索顺序在前面,
    /// 这里只按列表顺序循环)。
    pub fn step(&mut self, delta: isize) -> Option<Match> {
        if self.matches.is_empty() {
            return None;
        }
        let n = self.matches.len() as isize;
        let i = self.current as isize + delta;
        self.current = i.rem_euclid(n) as usize;
        self.current_match()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_all_non_overlapping_matches() {
        let line = Line::plain("error error ERROR");
        let mut out = Vec::new();
        find_in_line(&line, "error", false, 7, 100, &mut out);
        assert_eq!(
            out,
            vec![
                Match { row: 7, col: 0, cols: 5 },
                Match { row: 7, col: 6, cols: 5 },
                Match { row: 7, col: 12, cols: 5 },
            ]
        );
    }

    #[test]
    fn case_sensitive_switch_works() {
        let line = Line::plain("Error ERROR");
        let mut out = Vec::new();
        find_in_line(&line, "error", true, 0, 100, &mut out);
        assert!(out.is_empty());
        find_in_line(&line, "Error", true, 0, 100, &mut out);
        assert_eq!(out, vec![Match { row: 0, col: 0, cols: 5 }]);
    }

    #[test]
    fn wide_chars_map_to_terminal_columns() {
        // "中a文b":列 0(中,占2) / 2(a) / 3(文,占2) / 5(b)
        let line = Line {
            text: "中a文b".to_string(),
            cols: vec![0, 2, 3, 5, 6],
        };
        let mut out = Vec::new();
        find_in_line(&line, "文b", false, 3, 100, &mut out);
        assert_eq!(out, vec![Match { row: 3, col: 3, cols: 3 }]);
    }

    #[test]
    fn max_matches_is_a_hard_cap() {
        let lines: Vec<Line> = (0..10).map(|_| Line::plain("x x x x x")).collect();
        let got = find_matches(&lines, "x", false, 7);
        assert_eq!(got.len(), 7);
    }

    #[test]
    fn empty_query_has_no_matches() {
        assert!(find_matches(&[Line::plain("abc")], "", false, 100).is_empty());
    }

    #[test]
    fn scroll_centers_match_and_clamps() {
        let m = Match { row: 900, col: 0, cols: 1 };
        // sb=4000 rows=40 → 目标顶行 880 → scroll = 4000-40-880 = 3080
        assert_eq!(scroll_for_match(m, 4000, 40, 3960.0), 3080.0);
        assert_eq!(scroll_for_match(m, 4000, 40, 100.0), 100.0);
        let top = Match { row: 2, col: 0, cols: 1 };
        assert_eq!(scroll_for_match(top, 4000, 40, 3960.0), 3960.0);
    }

    #[test]
    fn step_wraps_both_ways() {
        let lines: Vec<Line> = (0..3).map(|_| Line::plain("hit")).collect();
        let mut s = SearchState::new(false);
        s.set_query("hit".to_string());
        s.refresh(&lines, 100);
        assert_eq!(s.matches.len(), 3);
        assert_eq!(s.step(1).unwrap().row, 1);
        assert_eq!(s.step(-1).unwrap().row, 0);
        assert_eq!(s.step(-1).unwrap().row, 2, "往前循环到最后一条");
        assert_eq!(s.step(1).unwrap().row, 0);
    }

    #[test]
    fn refresh_keeps_current_near_old_row() {
        let mut s = SearchState::new(false);
        s.set_query("x".to_string());
        let lines: Vec<Line> = (0..10)
            .map(|i| Line {
                text: format!("{i} x"),
                cols: vec![0, 1, 2, 3, 4],
            })
            .collect();
        s.refresh(&lines, 100);
        s.current = 5;
        s.dirty = true;
        s.refresh(&lines, 100);
        assert_eq!(s.current, 5);
        assert!(!s.dirty);
    }
}
