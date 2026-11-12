//! Lua 脚本层(mlua):键位映射 + 运行时动作 + 状态栏事件
//!
//! `config.lua` 中的 `keys` 表把"键位组合"映射到内置动作字符串或 Lua 函数:
//!
//! ```lua
//! return {
//!   keys = {
//!     ["ctrl+shift+r"] = "reload",
//!     ["f12"] = "send:dir\r",
//!     ["ctrl+alt+t"] = function() VectorTerm.notify("hi") end,
//!   },
//! }
//! ```
//!
//! Lua 回调里可用的全局表 `vectorterm` 提供一组动作函数(见 `register_api`)。
//! 所有动作都先进入队列,由主线程在 Lua 调用返回后统一执行,避免对 `GuiState`
//! 的可变借用重入。
//!
//! 另一半是**状态栏事件**:`wezterm.on('update-status' / 'update-right-status')`
//! 注册的回调由 [`ScriptEngine::update_status`] 定期派发,回调里调用
//! `window:set_right_status(...)` 写入的文字会被取回来交给标签栏绘制。
//! `wezterm.on('format-tab-title')` 同理,用来逐标签定制标题与配色。

use mlua::{Function, Lua, Table, Value};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use crate::render::Rgb;
use crate::status::Span;

/// 脚本请求的动作(主线程统一执行)
#[derive(Debug, Clone)]
pub enum ScriptAction {
    SetCursorColor(String),
    SetTrailDwell(f32),
    SetTrailThreshold(i64, i64),
    SetTrailDecay(f32, f32),
    SetTrailMax(f32),
    SetTrailColor(Option<String>),
    SendText(String),
    Paste,
    Copy,
    Reload,
    Quit,
    Notify(String),
    // ---- 标签页(wezterm 的 action 映射到这里)----
    NewTab,
    CloseTab,
    /// 关掉当前 pane(最后一个 pane 则连带关标签)
    ClosePane,
    NextTab,
    PrevTab,
    /// 0-based 标签下标(wezterm 的 ActivateTab(n))
    ActivateTab(usize),
    ActivateLastTab,
    ToggleFullScreen,
    /// 文件浏览器侧边栏:开 / 关 / 在列表与终端之间换焦点
    FilesGui,
}

enum Binding {
    Builtin(ScriptAction),
    Lua(Function),
}

pub struct ScriptEngine {
    lua: Lua,
    bindings: HashMap<String, Binding>,
    actions: Arc<Mutex<Vec<ScriptAction>>>,
    /// `format-tab-title` 已经报过一次错就不再刷屏(每帧都会调用)
    tab_title_error_reported: Cell<bool>,
}

impl ScriptEngine {
    pub fn load(path: &Path, actions: Arc<Mutex<Vec<ScriptAction>>>) -> anyhow::Result<Self> {
        let src = std::fs::read_to_string(path)?;
        Self::from_source_with_keys(&src, path.parent(), actions)
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
    }

    /// 兼容旧调用(单测):等价于 `from_source_with_keys(src, None, actions)`,
    /// 即不解析 `keys_file` 外部引用,只认主配置内联的 `keys`。
    pub fn from_source(src: &str, actions: Arc<Mutex<Vec<ScriptAction>>>) -> anyhow::Result<Self> {
        Self::from_source_with_keys(src, None, actions)
    }

    /// 加载主配置并解析键位。
    ///
    /// 键位分两层,后者覆盖前者,都没列出的组合键照常退回终端内置默认键位:
    /// 1. **基础层**:`keys_file` / `keys_files` 指向的外部 Lua 文件里的 `keys` 表;
    /// 2. **覆盖层**:主配置自己内联的 `keys` 表(优先级最高)。
    ///
    /// 外部文件里的 `keys` 与主配置里的 `keys` 走同一套解析(字典写法
    /// `["ctrl+shift+r"] = "reload"`、wezterm 数组写法、以及指向 Lua 函数),
    /// 所以只改某一个组合键、其余保留默认这件事天然成立。
    ///
    /// `base_dir` 是主配置所在目录,用来把相对路径 / 裸名字解析成真实文件。
    pub fn from_source_with_keys(
        src: &str,
        base_dir: Option<&Path>,
        actions: Arc<Mutex<Vec<ScriptAction>>>,
    ) -> anyhow::Result<Self> {
        let lua = crate::config::new_lua();
        // wezterm 配置的第一行通常是 `local wezterm = require 'wezterm'`,
        // 兼容层必须在 eval 之前注册。
        crate::wezterm_compat::install(&lua).map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let tbl: Table = lua
            .load(src)
            .eval()
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        register_api(&lua, &actions).map_err(|e| anyhow::anyhow!(e.to_string()))?;

        // 主题文件里可能有**代码**而不只是配色 —— 内置 DeepSeek 主题的
        // `wezterm.on('update-right-status', …)` 状态栏就是这么写的。而
        // ScriptEngine 平时只加载主配置,不补执行一遍主题,那其中的回调永远
        // 不会注册(表现就是"主题里写了状态栏,终端里什么都没有")。
        // 配色等数据由 config.rs 深合并,这里只取它的副作用。
        // 顺序:主配置先、主题后 —— 所以主题里的 `window:set_right_status`
        // 会盖掉主配置里写的同一条,想换成自己的就改主题副本。
        // 磁盘上没有这份主题时落到编译期内嵌的那一份(见 config::BUILTIN_THEMES)。
        if let Some(base) = base_dir {
            if let Ok(Some(name)) = tbl.get::<Option<String>>("theme") {
                match crate::config::theme_source(base, &name) {
                    Some((tsrc, origin)) => {
                        if let Err(e) = lua.load(tsrc.as_str()).eval::<Table>() {
                            crate::wlog!("[VectorTerm] 主题脚本执行失败({origin}):{e}");
                        }
                    }
                    None => crate::wlog!("[VectorTerm] 找不到主题脚本:{name}"),
                }
            }
        }

        let mut bindings = HashMap::new();

        // 第 1 层:外部键位文件的 keys 作为基础层
        if let Some(base) = base_dir {
            for spec in collect_keys_file_specs(&tbl) {
                if let Some(p) = resolve_keys_ref(base, &spec) {
                    if let Ok(ksrc) = std::fs::read_to_string(&p) {
                        if let Ok(ktbl) = lua.load(&ksrc).eval::<Table>() {
                            if let Some(kkeys) = ktbl.get("keys").ok().flatten() {
                                collect_keys(&kkeys, &mut bindings);
                            }
                        } else {
                            crate::wlog!("[VectorTerm] 键位文件解析失败:{p:?}");
                        }
                    } else {
                        crate::wlog!("[VectorTerm] 找不到键位文件:{p:?}");
                    }
                }
            }
        }

        // 第 2 层:主配置内联的 keys 覆盖上去
        if let Some(keys) = tbl.get("keys").ok().flatten() {
            collect_keys(&keys, &mut bindings);
        }

        Ok(Self {
            lua,
            bindings,
            actions,
            tab_title_error_reported: Cell::new(false),
        })
    }

    /// 命中键位则执行并返回 true
    pub fn dispatch(&self, combo: &str) -> bool {
        let combo = normalize_combo(combo);
        let Some(binding) = self.bindings.get(&combo) else {
            return false;
        };
        match binding {
            Binding::Builtin(action) => self.push(action.clone()),
            Binding::Lua(f) => {
                if let Err(e) = f.call::<()>(()) {
                    crate::wlog!("[VectorTerm] Lua 键位回调出错:{e}");
                }
            }
        }
        true
    }

    /// 取走待执行的动作
    pub fn drain_actions(&self) -> Vec<ScriptAction> {
        let mut out = Vec::new();
        if let Ok(mut q) = self.actions.lock() {
            out.append(&mut q);
        }
        out
    }

    fn push(&self, action: ScriptAction) {
        if let Ok(mut q) = self.actions.lock() {
            q.push(action);
        }
    }

    /// 保证 Lua 状态活到本引擎销毁(值本身也持引用,冗余但更稳)
    #[allow(dead_code)]
    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    /// 有没有注册过状态栏 / 标签标题回调。没有的话主循环连上下文都不用拼,
    /// 白白每秒查一次进程树、每帧问一次标题。
    pub fn has_status_hooks(&self) -> bool {
        ["update-status", "update-right-status", "format-tab-title"]
            .iter()
            .any(|e| !crate::wezterm_compat::event_callbacks(&self.lua, e).is_empty())
    }

    /// 派发 `update-status` 与 `update-right-status`,取回它们写下的状态文字。
    ///
    /// wezterm 两个事件都会发,前者给所有窗口、后者只给当前窗口;VectorTerm 只有
    /// 一个窗口,区别只剩"两个都发",顺序保持一致。
    pub fn update_status(&self, ctx: &StatusCtx) -> StatusText {
        let slots = Rc::new(RefCell::new(StatusText::default()));
        let events = ["update-status", "update-right-status"];
        for event in events {
            for func in crate::wezterm_compat::event_callbacks(&self.lua, event) {
                match self.invoke_status(&func, ctx, &slots) {
                    Ok(()) => {}
                    Err(e) => crate::wlog!("[VectorTerm] {event} 回调出错:{e}"),
                }
            }
        }
        let out = slots.borrow().clone();
        out
    }

    fn invoke_status(
        &self,
        func: &Function,
        ctx: &StatusCtx,
        slots: &Rc<RefCell<StatusText>>,
    ) -> mlua::Result<()> {
        let panes = self.make_panes(ctx)?;
        let tabs = self.make_tabs(ctx, &panes)?;
        let window = self.make_window(ctx, slots, &panes)?;
        let pane = panes.get(ctx.active).cloned().unwrap_or_else(|| {
            // 没标签时也给个空 pane,`pane:get_title()` 不会炸
            self.make_pane(&TabInfo::default())
                .unwrap_or_else(|_| self.lua.create_table().unwrap())
        });
        let _ = tabs;
        func.call::<()>((window, pane))
    }

    /// 逐标签跑 `format-tab-title`。没有任何回调时返回 `None`,调用方退回
    /// 内置的"序号: 标题"画法。
    pub fn format_tab_titles(
        &self,
        ctx: &StatusCtx,
        hover: Option<usize>,
    ) -> Option<Vec<TabTitle>> {
        let funcs = crate::wezterm_compat::event_callbacks(&self.lua, "format-tab-title");
        if funcs.is_empty() {
            return None;
        }
        let panes = self.make_panes(ctx).ok()?;
        let tabs = self.make_tabs(ctx, &panes).ok()?;
        let config = self.make_config_table(ctx).ok()?;

        let mut out = Vec::with_capacity(ctx.tabs.len());
        for (i, info) in ctx.tabs.iter().enumerate() {
            let Ok(Some(tab)) = tabs.get::<Option<Table>>(i + 1) else {
                return None;
            };
            let ret: Value = match funcs[0].call((
                tab,
                tabs.clone(),
                panes.clone(),
                config.clone(),
                hover == Some(i),
                ctx.tab_max_width,
            )) {
                Ok(v) => v,
                Err(e) => {
                    // 回调出错不该让整个标签栏消失:这一项退回默认标题,
                    // 其余标签照常。错误只报一次,免得每秒刷屏。
                    if !self.tab_title_error_reported.get() {
                        self.tab_title_error_reported.set(true);
                        crate::wlog!("[VectorTerm] format-tab-title 回调出错(后续不再重复):{e}");
                    }
                    out.push(TabTitle {
                        spans: vec![Span {
                            text: info.title.clone(),
                            ..Span::default()
                        }],
                        ..TabTitle::default()
                    });
                    continue;
                }
            };
            out.push(title_from_value(&ret, &ctx.ansi, &info.title));
        }
        Some(out)
    }

    fn make_panes(&self, ctx: &StatusCtx) -> mlua::Result<Vec<Table>> {
        ctx.tabs.iter().map(|t| self.make_pane(t)).collect()
    }

    fn make_tabs(&self, ctx: &StatusCtx, panes: &[Table]) -> mlua::Result<Table> {
        let tabs = self.lua.create_table()?;
        for (i, info) in ctx.tabs.iter().enumerate() {
            let tab = self.lua.create_table()?;
            tab.set("tab_index", info.index)?;
            tab.set("tab_id", info.id)?;
            tab.set("is_active", info.active)?;
            tab.set("window_id", 1)?;
            if let Some(pane) = panes.get(i) {
                tab.set("active_pane", pane.clone())?;
            }
            tabs.set(i + 1, tab)?;
        }
        Ok(tabs)
    }

    /// `pane` 对象。字段(`title`)和方法(`get_title()`)都给,因为两种写法
    /// 在真实配置里都能见到:`tab.active_pane.title` 与 `pane:get_title()`。
    fn make_pane(&self, info: &TabInfo) -> mlua::Result<Table> {
        let pane = self.lua.create_table()?;
        let owners = Arc::new(Mutex::new(info.clone()));
        pane.set("pane_id", info.id)?;
        pane.set("title", info.title.clone())?;
        pane.set("is_active", info.active)?;

        let p = Arc::clone(&owners);
        pane.set(
            "get_title",
            self.lua.create_function(move |_, _: Table| {
                Ok(p.lock().map(|t| t.title.clone()).unwrap_or_default())
            })?,
        )?;
        let p = Arc::clone(&owners);
        pane.set(
            "get_foreground_process_name",
            self.lua.create_function(move |_, _: Table| {
                Ok(p.lock().ok().and_then(|t| t.prog.clone()))
            })?,
        )?;
        let p = Arc::clone(&owners);
        pane.set(
            "get_current_working_dir",
            self.lua.create_function(move |lua, _: Table| {
                let cwd = p.lock().ok().and_then(|t| t.cwd.clone());
                let Some(cwd) = cwd else {
                    return Ok(Value::Nil);
                };
                // wezterm 这里返回一个 Url 对象,配置里常见 `.file_path` 与 `.. ''`
                let u = lua.create_table()?;
                u.set("scheme", "file")?;
                u.set("host", "")?;
                u.set("path", cwd.clone())?;
                u.set("file_path", cwd.clone())?;
                let mt = lua.create_table()?;
                let s = cwd.clone();
                mt.set(
                    "__tostring",
                    lua.create_function(move |_, _: Table| Ok(s.clone()))?,
                )?;
                let s = cwd;
                mt.set(
                    "__concat",
                    lua.create_function(move |_, (_, other): (Value, Value)| {
                        Ok(format!("{}{}", s, value_to_string(&other)))
                    })?,
                )?;
                u.set_metatable(Some(mt))?;
                Ok(Value::Table(u))
            })?,
        )?;
        Ok(pane)
    }

    /// `window` 对象。状态栏配置真正用得上的就这几个方法,其余给到不报错
    /// 的桩实现(`is_focused` / `get_dimensions` 这类偶尔会被用到)。
    fn make_window(
        &self,
        ctx: &StatusCtx,
        slots: &Rc<RefCell<StatusText>>,
        panes: &[Table],
    ) -> mlua::Result<Table> {
        let window = self.lua.create_table()?;
        window.set("window_id", 1)?;

        let s = Rc::clone(slots);
        window.set(
            "set_right_status",
            self.lua
                .create_function(move |_, (_, text): (Table, String)| {
                    s.borrow_mut().right = text;
                    Ok(())
                })?,
        )?;
        let s = Rc::clone(slots);
        window.set(
            "set_left_status",
            self.lua
                .create_function(move |_, (_, text): (Table, String)| {
                    s.borrow_mut().left = text;
                    Ok(())
                })?,
        )?;
        let focused = ctx.focused;
        window.set(
            "is_focused",
            self.lua.create_function(move |_, _: Table| Ok(focused))?,
        )?;
        let (pw, ph) = ctx.pixel;
        let dpi = ctx.dpi;
        window.set(
            "get_dimensions",
            self.lua.create_function(move |lua, _: Table| {
                let t = lua.create_table()?;
                t.set("pixel_width", pw)?;
                t.set("pixel_height", ph)?;
                t.set("dpi", dpi)?;
                t.set("is_full_screen", false)?;
                Ok(t)
            })?,
        )?;
        let active = ctx.active;
        let pane = panes.get(active).cloned();
        window.set(
            "active_pane",
            self.lua
                .create_function(move |_, _: Table| Ok(pane.clone()))?,
        )?;
        Ok(window)
    }

    /// `format-tab-title` 的第 4 个参数
    fn make_config_table(&self, ctx: &StatusCtx) -> mlua::Result<Table> {
        let c = self.lua.create_table()?;
        c.set("tab_max_width", ctx.tab_max_width)?;
        c.set("use_fancy_tab_bar", ctx.fancy_tab_bar)?;
        c.set("enable_tab_bar", true)?;
        Ok(c)
    }
}

/// 一个标签喂给 Lua 的最小快照。
///
/// 只在状态栏 / 标签标题回调里用,所以不持有终端对象 —— 回调执行期间不能在
/// 主线程上再去锁 `Terminal`(会和外层已经持有的锁撞车)。
#[derive(Clone, Debug, Default)]
pub struct TabInfo {
    /// 0 基,和 wezterm 的 `tab.tab_index` 一致
    pub index: usize,
    pub id: u64,
    pub title: String,
    pub active: bool,
    /// OSC 7 报上来的当前目录;没报过就是 None
    pub cwd: Option<String>,
    /// 标签里正在跑的程序名(排掉 shell 自己之前的那层)
    pub prog: Option<String>,
}

/// 一次状态刷新的输入
pub struct StatusCtx {
    pub tabs: Vec<TabInfo>,
    pub active: usize,
    pub focused: bool,
    pub pixel: (u32, u32),
    pub dpi: f64,
    /// `format-tab-title` 里的 `config` 参数(只放得进状态栏用得到的那几项)
    pub tab_max_width: usize,
    pub fancy_tab_bar: bool,
    /// 256 色调色板,用来把 `{ AnsiColor = 'Maroon' }` 落成具体颜色
    pub ansi: [Rgb; 256],
}

/// 回调写进来的两条状态文字(原始串,带 SGR)
#[derive(Clone, Debug, Default)]
pub struct StatusText {
    pub left: String,
    pub right: String,
}

/// `format-tab-title` 的返回:带颜色的标题 + 这个标签的底色/前景
#[derive(Clone, Debug, Default)]
pub struct TabTitle {
    pub spans: Vec<Span>,
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
}

impl TabTitle {
    #[cfg(test)]
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}

/// 规范化键位名:小写、去空格、按字母序排列各段(使 `shift+ctrl+t` 与 `ctrl+shift+t` 等价)
pub fn normalize_combo(combo: &str) -> String {
    let mut parts: Vec<String> = combo
        .split('+')
        .map(|p| p.trim().to_ascii_lowercase())
        .filter(|p| !p.is_empty())
        .collect();
    parts.sort();
    parts.join("+")
}

/// 从主配置表里取出 `keys_file`(字符串)与 `keys_files`(数组),合并成一份引用列表。
///
/// 空串 / 空白会被忽略。两个字段都不存在时返回空向量 —— 即不引用任何外部键位。
fn collect_keys_file_specs(tbl: &Table) -> Vec<String> {
    let mut specs: Vec<String> = Vec::new();
    if let Ok(Some(s)) = tbl.get::<Option<String>>("keys_file") {
        let s = s.trim().to_string();
        if !s.is_empty() {
            specs.push(s);
        }
    }
    if let Ok(Some(arr)) = tbl.get::<Option<Table>>("keys_files") {
        for v in arr.sequence_values::<String>().flatten() {
            let s = v.trim().to_string();
            if !s.is_empty() {
                specs.push(s);
            }
        }
    }
    specs
}

/// 把一个引用字符串解析成真实文件路径。
///
/// - 绝对路径:原样返回;
/// - 含路径分隔符(`/` `\`)或 `..` 或结尾是 `.lua`:当作相对主配置目录的路径;
/// - 其它(裸名字,例如 `"vim"`):补上 `.lua` 后缀,依次在
///   ① 配置所在目录 → ② `~/.config/VectorTerm/keys/`(配置管理器放键位文件的地方)
///   → ③ `<exe>/config/` → ④ `<cwd>/config/` 里找,取第一个存在的。
///
/// 这样配置被复制到 `current/` 之后,`keys_file = "keys.lua"` 仍然能找到
/// 用户目录 `keys/` 里的那份键位文件,不需要写绝对路径。
fn resolve_keys_ref(base: &Path, spec: &str) -> Option<PathBuf> {
    let s = spec.trim();
    if s.is_empty() {
        return None;
    }
    let p = Path::new(s);
    if p.is_absolute() {
        return Some(p.to_path_buf());
    }
    let has_sep = s.contains('/') || s.contains('\\') || s.contains("..");
    let leaf = if has_sep {
        s.to_string()
    } else if s.ends_with(".lua") {
        s.to_string()
    } else {
        format!("{s}.lua")
    };
    let primary = base.join(&leaf);
    if primary.is_file() {
        return Some(primary);
    }
    if !has_sep {
        let mut candidates = Vec::new();
        if let Some(dir) = crate::config::keys_dir() {
            candidates.push(dir.join(&leaf));
        }
        if let Some(exe) = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
        {
            candidates.push(exe.join("config").join(&leaf));
        }
        if let Ok(cwd) = std::env::current_dir() {
            candidates.push(cwd.join("config").join(&leaf));
        }
        if let Some(hit) = candidates.into_iter().find(|p| p.is_file()) {
            return Some(hit);
        }
    }
    Some(primary)
}

/// 把一份 Lua 的 `keys` 表解析进 bindings(字典写法 + wezterm 数组写法 + Lua 函数)。
///
/// 已是幂等的:后调用者覆盖先调用者同名的组合键,所以"外部文件 → 主配置覆盖"
/// 的优先级靠调用顺序就能保证。
fn collect_keys(keys: &Table, bindings: &mut HashMap<String, Binding>) {
    for pair in keys.pairs::<mlua::Value, mlua::Value>() {
        let Ok((key, value)) = pair else { continue };
        match (key, value) {
            // 数组写法:wezterm 的 { key=, mods=, action= }
            (mlua::Value::Integer(_), mlua::Value::Table(spec)) => {
                if let Some((combo, action)) = parse_wezterm_binding(&spec) {
                    bindings.insert(normalize_combo(&combo), Binding::Builtin(action));
                }
            }
            // 字典写法:["ctrl+shift+r"] = "reload" / function
            (mlua::Value::String(combo), value) => {
                let combo = normalize_combo(&combo.to_string_lossy());
                if combo.is_empty() {
                    continue;
                }
                match value {
                    mlua::Value::String(s) => {
                        let text = s.to_string_lossy().to_string();
                        match parse_builtin(&text) {
                            Some(action) => {
                                bindings.insert(combo, Binding::Builtin(action));
                            }
                            None => crate::wlog!("[VectorTerm] 未知的键位动作:{text}"),
                        }
                    }
                    mlua::Value::Function(f) => {
                        bindings.insert(combo, Binding::Lua(f));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// `status_bar.modules` 里一段代码的返回值 → 要显示的文字。
///
/// 表交给 `wezterm.format` 处理(和 `set_right_status(wezterm.format{...})`
/// 同一套写法),其它类型按字符串化处理。解析失败就当这块是空的。
/// `format-tab-title` 的返回值 → 带颜色的标题。
///
/// 允许三种返回:格式项列表(wezterm 的正规写法)、纯字符串、以及什么都不返回
/// (退回标签自己的标题)。
fn title_from_value(value: &Value, ansi: &[Rgb; 256], fallback: &str) -> TabTitle {
    let mut out = TabTitle::default();
    let items = match value {
        Value::Table(t) => t.clone(),
        Value::String(s) => {
            out.spans.push(Span {
                text: s.to_string_lossy().to_string(),
                ..Span::default()
            });
            return out;
        }
        _ => {
            out.spans.push(Span {
                text: fallback.to_string(),
                ..Span::default()
            });
            return out;
        }
    };

    // 逐项累积当前样式,遇到文字就吐一段
    let mut style = Span::default();
    let mut buf = String::new();
    // 第一个 `Foreground` 兼作这个标签的默认前景(文字片段之后可能还会换色)
    let mut base_fg = None;
    macro_rules! flush {
        () => {
            if !buf.is_empty() {
                out.spans.push(Span {
                    text: std::mem::take(&mut buf),
                    ..style.clone()
                });
            }
        };
    }
    for item in items.sequence_values::<Table>().flatten() {
        if let Ok(Some(text)) = item.get::<Option<String>>("Text") {
            buf.push_str(&text);
            continue;
        }
        if let Ok(Some(spec)) = item.get::<Option<Table>>("Foreground") {
            flush!();
            style.fg = crate::wezterm_compat::color_from_spec(&spec, ansi);
            if base_fg.is_none() {
                base_fg = style.fg;
            }
            continue;
        }
        if let Ok(Some(spec)) = item.get::<Option<Table>>("Background") {
            // 标签底色只有一个,以最后一次为准
            out.bg = crate::wezterm_compat::color_from_spec(&spec, ansi);
            continue;
        }
        if let Ok(Some(attr)) = item.get::<Option<Table>>("Attribute") {
            flush!();
            apply_attr(&attr, &mut style);
            continue;
        }
        let reset: Value = item.get("ResetAttributes").unwrap_or(Value::Nil);
        if !matches!(reset, Value::Nil) {
            flush!();
            style = Span::default();
        }
    }
    flush!();
    out.fg = base_fg.or_else(|| out.spans.iter().find_map(|s| s.fg));
    out
}

fn apply_attr(attr: &Table, style: &mut Span) {
    if let Ok(Some(v)) = attr.get::<Option<String>>("Intensity") {
        style.bold = v.eq_ignore_ascii_case("bold");
    }
    if attr.get::<Option<bool>>("Italic").ok().flatten() == Some(true) {
        style.italic = true;
    }
    if attr.get::<Option<bool>>("Strikethrough").ok().flatten() == Some(true) {
        style.underline = true; // 状态栏里不区分删除线,统一画条线
    }
    match attr.get::<Value>("Underline") {
        Ok(Value::Boolean(true)) => style.underline = true,
        Ok(Value::String(s)) if !s.to_string_lossy().is_empty() => style.underline = true,
        _ => {}
    }
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.to_string_lossy().to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Boolean(b) => b.to_string(),
        _ => String::new(),
    }
}

fn parse_builtin(action: &str) -> Option<ScriptAction> {
    match action {
        "paste" => Some(ScriptAction::Paste),
        "copy" => Some(ScriptAction::Copy),
        "reload" => Some(ScriptAction::Reload),
        "quit" => Some(ScriptAction::Quit),
        "new_tab" | "spawn_tab" => Some(ScriptAction::NewTab),
        "close_tab" => Some(ScriptAction::CloseTab),
        // 关当前窗格。内置快捷键是 Ctrl+Shift+X,这里给 Lua 一个同名动作,
        // 想换键位写 `keys = { ["ctrl+alt+w"] = "close_pane" }` 即可。
        "close_pane" | "close_pane_current" => Some(ScriptAction::ClosePane),
        "next_tab" => Some(ScriptAction::NextTab),
        "prev_tab" => Some(ScriptAction::PrevTab),
        // 文件浏览器侧边栏。Lua 里写 `keys = { ["ctrl+shift+e"] = "files_gui" }`
        // 或 wezterm 风格的 `{ key = 'E', mods = 'CTRL|SHIFT', action = 'files_gui' }`
        "files_gui" | "files-gui" | "toggle_files_gui" => Some(ScriptAction::FilesGui),
        _ => action
            .strip_prefix("send:")
            .map(|text| ScriptAction::SendText(text.to_string())),
    }
}

/// wezterm 的 `{ key = 'T', mods = 'CTRL|SHIFT', action = ... }`
fn parse_wezterm_binding(spec: &Table) -> Option<(String, ScriptAction)> {
    let key: String = spec.get("key").ok()?;
    let mods: String = spec
        .get::<Option<String>>("mods")
        .ok()
        .flatten()
        .unwrap_or_default();
    let combo = wezterm_combo(&mods, &key);
    let action: mlua::Value = spec.get("action").ok()?;
    let action = match action {
        mlua::Value::Table(t) => parse_wezterm_action(&t)?,
        mlua::Value::String(s) => parse_builtin(&s.to_string_lossy())?,
        _ => return None,
    };
    Some((combo, action))
}

/// wezterm 的修饰键写法(CTRL|SHIFT|ALT|SUPER)与键名 → VectorTerm 的 combo
fn wezterm_combo(mods: &str, key: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for m in mods.split('|').map(|s| s.trim().to_ascii_lowercase()) {
        let part = match m.as_str() {
            "ctrl" | "control" => "ctrl",
            "shift" => "shift",
            "alt" | "meta" => "alt",
            "super" | "cmd" | "win" => "super",
            _ => continue,
        };
        parts.push(part.to_string());
    }
    parts.push(normalize_wezterm_key(key));
    parts.join("+")
}

fn normalize_wezterm_key(key: &str) -> String {
    let k = key.trim().to_ascii_lowercase();
    match k.as_str() {
        "leftarrow" => "left".to_string(),
        "rightarrow" => "right".to_string(),
        "uparrow" => "up".to_string(),
        "downarrow" => "down".to_string(),
        "pageup" => "pageup".to_string(),
        "pagedown" => "pagedown".to_string(),
        _ => k,
    }
}

/// `wezterm.action.*` 构造出来的表 → 本地动作
fn parse_wezterm_action(t: &Table) -> Option<ScriptAction> {
    let name: String = t.get("__wezterm_action").ok()?;
    let arg_i = || t.get::<Option<i64>>("arg").ok().flatten();
    match name.as_str() {
        "ReloadConfiguration" => Some(ScriptAction::Reload),
        "SpawnTab" => Some(ScriptAction::NewTab),
        "CloseCurrentTab" => Some(ScriptAction::CloseTab),
        "CloseCurrentPane" => Some(ScriptAction::ClosePane),
        "ActivateTabRelative" => {
            let n = arg_i().unwrap_or(1);
            Some(if n < 0 {
                ScriptAction::PrevTab
            } else {
                ScriptAction::NextTab
            })
        }
        "ActivateTab" => Some(ScriptAction::ActivateTab(
            arg_i().unwrap_or(0).max(0) as usize
        )),
        "ActivateLastTab" => Some(ScriptAction::ActivateLastTab),
        "CopyTo" => Some(ScriptAction::Copy),
        "PasteFrom" => Some(ScriptAction::Paste),
        "ToggleFullScreen" => Some(ScriptAction::ToggleFullScreen),
        "SendString" => t
            .get::<Option<String>>("arg")
            .ok()
            .flatten()
            .map(ScriptAction::SendText),
        "Nop" => None,
        other => {
            crate::wlog!("[VectorTerm] 暂不支持的 wezterm 动作:{other}");
            None
        }
    }
}

fn register_api(lua: &Lua, actions: &Arc<Mutex<Vec<ScriptAction>>>) -> mlua::Result<()> {
    let vectorterm = lua.create_table()?;
    let queue = Arc::clone(actions);
    let push = move |action: ScriptAction| -> mlua::Result<()> {
        if let Ok(mut q) = queue.lock() {
            q.push(action);
        }
        Ok(())
    };

    let p = push.clone();
    vectorterm.set(
        "notify",
        lua.create_function(move |_, msg: String| p(ScriptAction::Notify(msg)))?,
    )?;
    let p = push.clone();
    vectorterm.set(
        "send",
        lua.create_function(move |_, text: String| p(ScriptAction::SendText(text)))?,
    )?;
    let p = push.clone();
    vectorterm.set(
        "paste",
        lua.create_function(move |_, ()| p(ScriptAction::Paste))?,
    )?;
    let p = push.clone();
    vectorterm.set(
        "reload",
        lua.create_function(move |_, ()| p(ScriptAction::Reload))?,
    )?;
    let p = push.clone();
    vectorterm.set(
        "quit",
        lua.create_function(move |_, ()| p(ScriptAction::Quit))?,
    )?;
    let p = push.clone();
    vectorterm.set(
        "cursor_color",
        lua.create_function(move |_, color: String| p(ScriptAction::SetCursorColor(color)))?,
    )?;
    let p = push.clone();
    vectorterm.set(
        "trail_dwell",
        lua.create_function(move |_, ms: f32| p(ScriptAction::SetTrailDwell(ms)))?,
    )?;
    let p = push.clone();
    vectorterm.set(
        "trail_threshold",
        lua.create_function(move |_, (x, y): (i64, Option<i64>)| {
            p(ScriptAction::SetTrailThreshold(x, y.unwrap_or(x)))
        })?,
    )?;
    let p = push.clone();
    vectorterm.set(
        "trail_decay",
        lua.create_function(move |_, (fast, slow): (f32, Option<f32>)| {
            p(ScriptAction::SetTrailDecay(fast, slow.unwrap_or(fast)))
        })?,
    )?;
    let p = push.clone();
    vectorterm.set(
        "trail_max",
        lua.create_function(move |_, ms: f32| p(ScriptAction::SetTrailMax(ms)))?,
    )?;
    let p = push.clone();
    vectorterm.set(
        "trail_color",
        lua.create_function(move |_, color: Option<String>| p(ScriptAction::SetTrailColor(color)))?,
    )?;

    // 对外名字是 VectorTerm
    lua.globals().set("VectorTerm", vectorterm.clone())?;
    // 兼容旧配置:更名前写的 `wintty.set(...)` / `wintty.notify(...)` 仍然可用,
    // 两者指向同一张表,新老写法混用也不会出现两份状态。
    lua.globals().set("wintty", vectorterm)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_orders_modifiers() {
        assert_eq!(normalize_combo("Shift+Ctrl+T"), "ctrl+shift+t");
        assert_eq!(normalize_combo("ctrl + alt + enter"), "alt+ctrl+enter");
        assert_eq!(normalize_combo("F12"), "f12");
        assert_eq!(normalize_combo(""), "");
    }

    #[test]
    fn builtin_and_lua_bindings_dispatch() {
        let src = r#"
            return {
                keys = {
                    ["ctrl+shift+r"] = "reload",
                    ["ctrl+shift+x"] = "send:hello",
                    ["ctrl+alt+t"] = function() VectorTerm.notify("hi") end,
                },
            }
        "#;
        let actions = Arc::new(Mutex::new(Vec::new()));
        let eng = ScriptEngine::from_source(src, actions.clone()).expect("脚本应能加载");

        // 修饰键顺序无关
        assert!(eng.dispatch("shift+ctrl+r"));
        assert!(eng.dispatch("ctrl+shift+x"));
        assert!(!eng.dispatch("ctrl+q"), "未绑定的键不应命中");

        let got = eng.drain_actions();
        assert!(matches!(got[0], ScriptAction::Reload));
        assert!(matches!(&got[1], ScriptAction::SendText(t) if t == "hello"));

        // Lua 回调通过 VectorTerm.notify 入队
        assert!(eng.dispatch("ctrl+alt+t"));
        let got = eng.drain_actions();
        assert!(matches!(&got[0], ScriptAction::Notify(m) if m == "hi"));
        assert!(eng.drain_actions().is_empty());
    }

    /// 更名后的兼容:旧配置里的 `wintty.*` 与新名字指向同一张表。
    #[test]
    fn legacy_wintty_namespace_still_works() {
        let src = r#"
            return {
                keys = {
                    ["ctrl+alt+1"] = function() VectorTerm.notify("new") end,
                    ["ctrl+alt+2"] = function() wintty.notify("old") end,
                },
            }
        "#;
        let actions = Arc::new(Mutex::new(Vec::new()));
        let eng = ScriptEngine::from_source(src, actions).expect("脚本应能加载");
        assert!(eng.dispatch("ctrl+alt+1"));
        assert!(eng.dispatch("ctrl+alt+2"));
        let got = eng.drain_actions();
        let notes: Vec<_> = got
            .iter()
            .filter_map(|a| match a {
                ScriptAction::Notify(m) => Some(m.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(notes, vec!["new", "old"], "新旧命名空间都应能触发 notify");
    }

    /// wezterm 的 keys 数组写法要能映射到本地动作
    #[test]
    fn wezterm_key_array_is_parsed() {
        let src = r#"
            local wezterm = require 'wezterm'
            local config = wezterm.config_builder()
            config.keys = {
              { key = 'T', mods = 'CTRL|SHIFT', action = wezterm.action.SpawnTab 'DefaultDomain' },
              { key = 'W', mods = 'CTRL|SHIFT', action = wezterm.action.CloseCurrentTab },
              { key = 'X', mods = 'CTRL|SHIFT', action = wezterm.action.CloseCurrentPane { confirm = true } },
              { key = 'Tab', mods = 'CTRL', action = wezterm.action.ActivateTabRelative(1) },
              { key = '1', mods = 'CTRL', action = wezterm.action.ActivateTab(0) },
            }
            return config
        "#;
        let actions = Arc::new(Mutex::new(Vec::new()));
        let eng = ScriptEngine::from_source(src, actions).expect("脚本应能加载");
        assert!(eng.dispatch("ctrl+shift+t"));
        assert!(eng.dispatch("ctrl+shift+w"));
        assert!(eng.dispatch("ctrl+shift+x"));
        assert!(eng.dispatch("ctrl+tab"));
        assert!(eng.dispatch("ctrl+1"));
        let got = eng.drain_actions();
        assert!(matches!(got[0], ScriptAction::NewTab));
        assert!(matches!(got[1], ScriptAction::CloseTab));
        assert!(matches!(got[2], ScriptAction::ClosePane));
        assert!(matches!(got[3], ScriptAction::NextTab));
        assert!(matches!(got[4], ScriptAction::ActivateTab(0)));
    }

    #[test]
    fn unknown_action_is_ignored() {
        let src = r#" return { keys = { ["f1"] = "no_such_action" } } "#;
        let actions = Arc::new(Mutex::new(Vec::new()));
        let eng = ScriptEngine::from_source(src, actions).expect("脚本应能加载");
        assert!(!eng.dispatch("f1"));
    }

    /// `keys_file` 指向的外部 Lua 文件会被当作基础层加载;主配置里内联的 keys
    /// 再覆盖它;没列出的组合键退回默认(这里用"未命中"验证)。
    #[test]
    fn keys_file_is_loaded_as_base_layer() {
        let tmp = std::env::temp_dir().join(format!("vectorterm_keysfile_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        // 外部键位文件:只定义了 ctrl+shift+k。
        let _ = std::fs::write(
            tmp.join("mykeys.lua"),
            r#" return { keys = { ["ctrl+shift+k"] = "reload" } } "#,
        );
        let main = format!(
            "return {{ keys_file = \"{}\", keys = {{ [\"ctrl+shift+k\"] = \"copy\" }} }}",
            tmp.join("mykeys.lua").display().to_string().replace('\\', "/")
        );
        let actions = Arc::new(Mutex::new(Vec::new()));
        let eng =
            ScriptEngine::from_source_with_keys(&main, Some(&tmp), actions).expect("应能加载");

        // 文件里的组合键在,且被主配置内联 keys 覆盖成 copy。
        assert!(eng.dispatch("ctrl+shift+k"));
        let got = eng.drain_actions();
        assert!(matches!(&got[0], ScriptAction::Copy), "内联 keys 应覆盖文件: {got:?}");

        // 文件与主配置都没列的组合键:不命中(退回内置默认)。
        assert!(!eng.dispatch("ctrl+shift+z"));
    }

    /// 主题文件里的**代码**要被执行,不能只当数据表读。
    ///
    /// 内置 DeepSeek 主题的状态栏就是 `wezterm.on('update-right-status', …)`;
    /// 如果 ScriptEngine 只加载主配置,这个回调永远注册不上,状态栏一片空白。
    #[test]
    fn theme_script_callbacks_get_registered() {
        let tmp = std::env::temp_dir().join(format!("vectorterm_theme_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let _ = std::fs::write(
            tmp.join("mytheme.lua"),
            r#"
wezterm.on('update-right-status', function(window, pane)
  window:set_right_status('hi')
end)
return { colors = { background = '#000000' } }
"#,
        );
        let _ = std::fs::write(tmp.join("config.lua"), r#" return { theme = "mytheme" } "#);
        assert!(
            crate::config::theme_path(&tmp, "mytheme").is_some(),
            "theme_path 应能找到 {}",
            tmp.join("mytheme.lua").display()
        );
        let actions = Arc::new(Mutex::new(Vec::new()));
        let eng = ScriptEngine::load(&tmp.join("config.lua"), actions).expect("应能加载");
        assert_eq!(
            crate::wezterm_compat::event_callbacks(&eng.lua, "update-right-status").len(),
            1,
            "主题里的 update-right-status 回调应已注册"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 裸名字(无路径、无后缀)按"主配置同目录下的 <name>.lua"解析。
    #[test]
    fn bare_keys_file_name_resolves_beside_main_config() {
        let tmp = std::env::temp_dir().join(format!("vectorterm_keysfile2_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let _ = std::fs::write(
            tmp.join("vim.lua"),
            r#" return { keys = { ["ctrl+shift+v"] = "paste" } } "#,
        );
        let main = r#" return { keys_file = "vim" } "#;
        let actions = Arc::new(Mutex::new(Vec::new()));
        let eng = ScriptEngine::from_source_with_keys(main, Some(&tmp), actions).expect("应能加载");
        assert!(eng.dispatch("ctrl+shift+v"));
        assert!(matches!(&eng.drain_actions()[0], ScriptAction::Paste));
    }

    // ---- 状态栏 / 标签标题事件 ----

    fn ctx() -> StatusCtx {
        StatusCtx {
            tabs: vec![
                TabInfo {
                    index: 0,
                    id: 7,
                    title: "pwsh".into(),
                    active: true,
                    cwd: Some("C:\\Users\\me".into()),
                    prog: Some("pwsh.exe".into()),
                },
                TabInfo {
                    index: 1,
                    id: 8,
                    title: "vim".into(),
                    active: false,
                    cwd: None,
                    prog: None,
                },
            ],
            active: 0,
            focused: true,
            pixel: (960, 640),
            dpi: 96.0,
            tab_max_width: 16,
            fancy_tab_bar: true,
            ansi: [Rgb { r: 0, g: 0, b: 0 }; 256],
        }
    }

    fn engine(src: &str) -> ScriptEngine {
        ScriptEngine::from_source(src, Arc::new(Mutex::new(Vec::new()))).expect("脚本应能加载")
    }

    /// `update-status` 写左、`update-right-status` 写右,两边都要收回来,
    /// 而且能拿到 pane 的标题与当前目录
    #[test]
    fn update_status_collects_left_and_right() {
        let eng = engine(
            r#"
            local wezterm = require 'wezterm'
            wezterm.on('update-status', function(window, pane)
                window:set_left_status('L:' .. pane:get_title())
            end)
            wezterm.on('update-right-status', function(window, pane)
                local cwd = pane:get_current_working_dir()
                window:set_right_status(wezterm.format({
                    { Foreground = { Color = '#ff0000' } }, { Text = 'R' },
                    { Text = ' ' .. tostring(cwd) },
                }))
            end)
            return {}
        "#,
        );
        assert!(eng.has_status_hooks());
        let out = eng.update_status(&ctx());
        assert_eq!(out.left, "L:pwsh");
        assert!(out.right.contains("38;2;255;0;0"), "颜色要留在串里:{out:?}");
        assert!(out.right.ends_with("R C:\\Users\\me"), "{out:?}");
    }

    /// 没有回调时不该白拼上下文
    #[test]
    fn no_hooks_means_no_work() {
        let eng = engine("return {}");
        assert!(!eng.has_status_hooks());
        assert_eq!(eng.update_status(&ctx()).left, "");
        assert!(eng.format_tab_titles(&ctx(), None).is_none());
    }

    /// `format-tab-title` 逐标签给文字与配色,`tab_index` 是 0 基
    #[test]
    fn format_tab_title_returns_spans_and_colors() {
        let eng = engine(
            r#"
            local wezterm = require 'wezterm'
            wezterm.on('format-tab-title', function(tab, tabs, panes, config, hover, max_width)
                local bg = tab.is_active and '#112233' or '#445566'
                return {
                    { Background = { Color = bg } },
                    { Text = tab.tab_index .. ':' .. tab.active_pane.title },
                    { Foreground = { Color = '#00ff00' } },
                    { Text = hover and '*' or '' },
                }
            end)
            return {}
        "#,
        );
        let titles = eng.format_tab_titles(&ctx(), Some(1)).expect("应有返回");
        assert_eq!(titles.len(), 2);
        assert_eq!(titles[0].text(), "0:pwsh");
        assert_eq!(
            titles[0].bg,
            Some(Rgb {
                r: 0x11,
                g: 0x22,
                b: 0x33
            })
        );
        assert_eq!(titles[0].fg, Some(Rgb { r: 0, g: 255, b: 0 }));
        assert_eq!(titles[1].text(), "1:vim*");
        assert_eq!(
            titles[1].bg,
            Some(Rgb {
                r: 0x44,
                g: 0x55,
                b: 0x66
            })
        );
    }

    /// 回调抛异常时退回默认标题,其余标签照常
    #[test]
    fn format_tab_title_errors_fall_back() {
        let eng = engine(
            r#"
            local wezterm = require 'wezterm'
            wezterm.on('format-tab-title', function() error('boom') end)
            return {}
        "#,
        );
        let titles = eng.format_tab_titles(&ctx(), None).expect("应有返回");
        assert_eq!(titles.len(), 2);
        assert_eq!(titles[0].text(), "pwsh");
        assert_eq!(titles[1].text(), "vim");
    }
}
