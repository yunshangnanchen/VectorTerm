//! 命令面板(`/` 或 `Ctrl+Shift+P`)。
//!
//! 以前想在终端里唤起 UI,得往 shell 敲 `files-gui` 这种专用命令:它靠 stdout
//! 打一条私有 OSC,终端再从 PTY 输出流里扫出来。那条链路已经整个删掉 ——
//! 现在所有「终端自身的动作」都收进这个输入框,和普通历史搜索共用同一个
//! 浮层位置,输入 `/` 就从搜索切到命令。
//!
//! 这里只放纯逻辑(命令表、过滤、补全、帮助/键位文本),渲染和动作执行在
//! `main.rs`,所以可以脱离窗口直接单测。

/// 面板能执行的动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// 打开可视化文件管理器(铺满当前 pane)
    Files,
    /// 打开可视化配置管理器
    Settings,
    /// 列出所有命令
    Help,
    /// 列出所有键位
    Keys,
    /// 重新加载配置
    Reload,
    /// 切回历史搜索栏
    Search,
}

#[derive(Clone, Copy, Debug)]
pub struct Spec {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub desc: &'static str,
    pub command: Command,
}

pub static COMMANDS: &[Spec] = &[
    Spec {
        name: "files",
        aliases: &["f", "explorer", "file"],
        desc: "打开可视化文件管理器(铺满当前窗格)",
        command: Command::Files,
    },
    Spec {
        name: "settings",
        aliases: &["s", "config", "preferences"],
        desc: "打开可视化配置管理器",
        command: Command::Settings,
    },
    Spec {
        name: "help",
        aliases: &["h", "?"],
        desc: "列出全部命令与用法",
        command: Command::Help,
    },
    Spec {
        name: "key",
        aliases: &["keys", "k", "bind"],
        desc: "查看全部键位与鼠标操作",
        command: Command::Keys,
    },
    Spec {
        name: "reload",
        aliases: &["r"],
        desc: "重新加载 config.lua",
        command: Command::Reload,
    },
    Spec {
        name: "search",
        aliases: &["find"],
        desc: "切回历史搜索(Ctrl+Shift+F)",
        command: Command::Search,
    },
];

/// `help` / `key` 的多行输出视图。
#[derive(Clone, Debug)]
pub struct Output {
    pub title: String,
    pub lines: Vec<String>,
}

/// 命令面板运行时状态。和历史搜索栏互斥,同一时刻只有一个是模态。
#[derive(Clone, Debug)]
pub struct Panel {
    pub active: bool,
    pub query: String,
    pub selected: usize,
    pub output: Option<Output>,
    /// 输出视图当前显示到的行号(`help` / `key` 比一屏长,要能翻页)。
    pub scroll: usize,
}

/// 输出视图一屏能放几行。随窗口高度变化,但夹在 8~32 行,免得小窗口把
/// 浮层撑出屏幕、大窗口又把底部提示顶掉。绘制和按键翻页共用这一份。
pub fn output_page_rows(window_h: f64, cell_h: f64) -> usize {
    (((window_h * 0.68) / cell_h.max(1.0)).floor() as usize).clamp(8, 32)
}

impl Default for Panel {
    fn default() -> Self {
        Self::new()
    }
}

impl Panel {
    pub fn new() -> Self {
        Self {
            active: false,
            query: String::new(),
            selected: 0,
            output: None,
            scroll: 0,
        }
    }

    pub fn open(&mut self) {
        self.active = true;
        self.query.clear();
        self.selected = 0;
        self.output = None;
    }

    pub fn close(&mut self) {
        *self = Self::new();
    }

    /// 按输入过滤命令:名字/别名前缀命中,或者说明里含这段文本。
    pub fn matches(&self) -> Vec<&'static Spec> {
        let q = self.query.trim().to_ascii_lowercase();
        COMMANDS
            .iter()
            .filter(|s| {
                q.is_empty()
                    || s.name.starts_with(&q)
                    || s.aliases.iter().any(|a| a.starts_with(&q))
                    || s.desc.to_ascii_lowercase().contains(&q)
            })
            .collect()
    }

    pub fn selection(&self) -> Option<Command> {
        let m = self.matches();
        if m.is_empty() {
            return None;
        }
        Some(m[self.selected.min(m.len() - 1)].command)
    }

    pub fn move_selection(&mut self, d: isize) {
        let n = self.matches().len();
        if n == 0 {
            self.selected = 0;
            return;
        }
        let cur = self.selected.min(n - 1) as isize;
        self.selected = (cur + d).rem_euclid(n as isize) as usize;
    }

    /// Tab 补全:把当前选中命令的名字填进输入框。
    pub fn complete(&mut self) {
        if let Some(s) = self.matches().get(self.selected).copied() {
            self.query = s.name.to_string();
            self.selected = 0;
        }
    }

    pub fn show_output(&mut self, title: impl Into<String>, lines: Vec<String>) {
        self.output = Some(Output {
            title: title.into(),
            lines,
        });
        self.scroll = 0;
    }

    /// 输出视图翻页。`d` 是行数增量(负数往上),`rows` 是一屏行数。
    /// 返回是否真的移动了;到顶/到底返回 false。
    pub fn page(&mut self, d: isize, rows: usize) -> bool {
        let Some(o) = &self.output else {
            return false;
        };
        let max = o.lines.len().saturating_sub(rows.max(1));
        let next = (self.scroll as isize + d).clamp(0, max as isize) as usize;
        if next == self.scroll {
            return false;
        }
        self.scroll = next;
        true
    }
}

/// 精确/别名匹配,也接受唯一前缀(方便直接敲 `set` 回车)。
pub fn resolve(input: &str) -> Option<Command> {
    let q = input.trim().to_ascii_lowercase();
    if q.is_empty() {
        return None;
    }
    for s in COMMANDS {
        if s.name == q || s.aliases.iter().any(|a| *a == q) {
            return Some(s.command);
        }
    }
    let prefix: Vec<_> = COMMANDS
        .iter()
        .filter(|s| s.name.starts_with(&q) || s.aliases.iter().any(|a| a.starts_with(&q)))
        .collect();
    (prefix.len() == 1).then(|| prefix[0].command)
}

pub fn help_lines() -> Vec<String> {
    let mut v = vec![
        "在搜索框里输入 / 进入命令模式;↑↓ 选择、Tab 补全、Enter 执行、Esc 退出。".to_string(),
        "也可以直接按 Ctrl+Shift+P 打开命令面板。".to_string(),
        "help / key 的输出视图:↑↓ 逐行、PgUp/PgDn 翻页、Home/End 到首尾,Esc 返回。".to_string(),
        String::new(),
        format!("{:<12}{:<22}{}", "命令", "别名", "说明"),
    ];
    for s in COMMANDS {
        v.push(format!(
            "{:<12}{:<22}{}",
            s.name,
            s.aliases.join(", "),
            s.desc
        ));
    }
    v.push(String::new());
    v.push("提示:输入 help 看这份列表,输入 key 看全部键位。".to_string());
    v
}

/// 键位表。和 `pane_action` / `handle_key` / 鼠标处理保持同步;
/// `key` 命令、`docs/config-manual.md`、`README.md` 都以这份为准。
pub fn key_lines() -> Vec<String> {
    vec![
        "【命令 / 搜索】".to_string(),
        "  Ctrl+Shift+P              打开命令面板".to_string(),
        "  Ctrl+Shift+F              历史搜索(框内输入 / 切到命令面板)".to_string(),
        "  搜索框: Enter/↑↓ 下一个,Shift+Enter 上一个,Tab 大小写,Esc 关闭".to_string(),
        "  命令框: ↑↓ 选择,Tab 补全,Enter 执行,Esc 关闭".to_string(),
        "  help/key 输出: ↑↓ 逐行,PgUp/PgDn 翻页,Home/End 首尾,Esc 返回".to_string(),
        String::new(),
        "【分屏与窗格】".to_string(),
        "  Ctrl+Shift+Alt+\"          左右分屏(Ctrl+Shift+\" 亦可)".to_string(),
        "  Ctrl+Shift+Alt+%          上下分屏(Ctrl+Shift+% 亦可)".to_string(),
        "  Ctrl+Shift+Alt+V / H      左右 / 上下分屏(字母别名)".to_string(),
        "  Ctrl+Shift+方向键         焦点移到相邻窗格".to_string(),
        "  Ctrl+Shift+X              关闭当前窗格(只剩一个则关标签)".to_string(),
        "  Ctrl+Shift+Alt+方向键     调整分割线位置(窗格大小)".to_string(),
        "  Alt+1..9                  聚焦第 N 个窗格".to_string(),
        "  鼠标拖动分割线            自由调整两侧窗格大小".to_string(),
        "  鼠标点击窗格              聚焦该窗格".to_string(),
        String::new(),
        "【文件与配置】".to_string(),
        "  Ctrl+Shift+E              文件浏览器侧边栏开关".to_string(),
        "  /files                    文件管理器(铺满当前窗格)  ·  /settings 配置管理器".to_string(),
        "  /reload                   重新加载配置".to_string(),
        String::new(),
        "【标签 / 剪贴板 / 回看】".to_string(),
        "  Ctrl+Shift+T / W          新建 / 关闭标签".to_string(),
        "  Ctrl+Tab / Ctrl+Shift+Tab 下一个 / 上一个标签".to_string(),
        "  Ctrl+1..9                 跳到第 N 个标签".to_string(),
        "  Ctrl+Shift+C / V          复制 / 粘贴(另支持 Ctrl+Insert / Shift+Insert)".to_string(),
        "  Shift+PageUp / PageDown   回看历史".to_string(),
        "  鼠标滚轮 / 拖滚动条       回看;点轨道翻页;拖选到边缘自动滚屏".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_exact_alias_and_unique_prefix() {
        assert_eq!(resolve("files"), Some(Command::Files));
        assert_eq!(resolve("  CONFIG "), Some(Command::Settings));
        assert_eq!(resolve("k"), Some(Command::Keys));
        assert_eq!(resolve("set"), Some(Command::Settings));
        assert_eq!(resolve(""), None);
        assert_eq!(resolve("nope"), None);
        // "s" 同时是 settings 的别名和 search 的前缀(还有别名 s),不是唯一
        assert_eq!(resolve("s"), Some(Command::Settings));
    }

    #[test]
    fn panel_filters_and_selects() {
        let mut p = Panel::new();
        assert_eq!(p.matches().len(), COMMANDS.len());
        assert_eq!(p.selection(), Some(Command::Files));
        p.move_selection(-1);
        assert_eq!(
            p.selection(),
            Some(COMMANDS[COMMANDS.len() - 1].command),
            "向上应循环到最后一项"
        );
        p = Panel::new();
        p.query = "key".into();
        assert_eq!(p.matches().len(), 1);
        assert_eq!(p.selection(), Some(Command::Keys));
    }

    #[test]
    fn tab_completion_fills_the_name() {
        let mut p = Panel::new();
        p.query = "conf".into();
        p.complete();
        assert_eq!(p.query, "settings");
        assert_eq!(p.selection(), Some(Command::Settings));
    }

    #[test]
    fn empty_query_never_crashes_selection() {
        let mut p = Panel::new();
        p.query = "zzzz".into();
        assert_eq!(p.selection(), None);
        p.move_selection(3);
        assert_eq!(p.selected, 0);
    }

    #[test]
    fn output_is_separate_from_query() {
        let mut p = Panel::new();
        p.open();
        p.show_output("KEYS", key_lines());
        assert!(p.output.is_some());
        assert!(p.output.as_ref().unwrap().lines.len() > 15);
        p.close();
        assert!(!p.active);
        assert!(p.output.is_none());
    }

    #[test]
    fn output_view_can_page_through_every_line() {
        let mut p = Panel::new();
        p.open();
        let lines: Vec<String> = (0..28).map(|i| format!("line {i}")).collect();
        p.show_output("KEYS", lines.clone());
        assert_eq!(p.scroll, 0);
        let rows = output_page_rows(746.0, 25.0);
        assert!(rows >= 8 && rows < lines.len(), "一屏应装不下全部行");
        assert!(p.page(rows as isize, rows));
        assert_eq!(p.scroll, lines.len() - rows, "28 行两屏就能翻到底");
        assert!(!p.page(rows as isize, rows), "到底后不能再往下");
        assert_eq!(p.scroll, lines.len() - rows, "到底后不能越界");
        assert!(p.page(-(lines.len() as isize), rows));
        assert_eq!(p.scroll, 0);
        assert!(!p.page(-1, rows));
        // 重新显示输出要把滚动位置归零,不能停在上一份文本的中段
        p.show_output("HELP", vec!["only".into()]);
        assert_eq!(p.scroll, 0);
        assert!(!p.page(-1, rows));
        p.close();
        assert_eq!(p.scroll, 0);
    }

    #[test]
    fn help_lists_every_command() {
        let h = help_lines().join("\n");
        for s in COMMANDS {
            assert!(h.contains(s.name), "help 里漏了 {}", s.name);
        }
        assert!(h.contains("/"));
    }
}
