//! wezterm 配置文件兼容层
//!
//! 目标:让 VectorTerm 直接加载 wezterm 的 `config.lua`,不必改写成两套配置。
//! 典型写法:
//!
//! ```lua
//! local wezterm = require 'wezterm'
//! local config = wezterm.config_builder()
//! config.font = wezterm.font_with_fallback({ 'MesloLGM Nerd Font', 'Cascadia Code' })
//! config.colors = { tab_bar = { ... } }
//! config.keys = {
//!   { key = 'T', mods = 'CTRL|SHIFT', action = wezterm.action.SpawnTab 'DefaultDomain' },
//! }
//! return config
//! ```
//!
//! 除了构造配置,这里还实现了状态栏 / 标签标题用的那半边 API:
//! `wezterm.on` 把回调登记进 Lua registry(由 [`crate::script`] 派发),
//! `wezterm.format` 产出 SGR 串,`wezterm.nerdfonts` 查真实字形表,
//! `wezterm.battery_info` 读本机电池。

use mlua::{Lua, Table, Value};

/// 事件回调在 Lua registry 里的键前缀,和 wezterm 一致:
/// `wezterm.on('update-right-status', f)` → `wezterm-event-update-right-status`
const EVENT_PREFIX: &str = "wezterm-event-";

/// 把 `wezterm` 全局表与 `package.preload['wezterm']` 装进 Lua 状态。
/// 必须在 `lua.load(src).eval()` 之前调用。
pub fn install(lua: &Lua) -> mlua::Result<()> {
    let wezterm = lua.create_table()?;

    // config_builder():wezterm 返回一个带默认值的配置对象;对我们来说
    // 普通表就够用,解析层会按缺省值补齐。
    wezterm.set(
        "config_builder",
        lua.create_function(|lua, ()| lua.create_table())?,
    )?;

    // font(spec [, opts]) -> { family = spec }
    wezterm.set(
        "font",
        lua.create_function(|lua, (spec, _opts): (String, Option<Table>)| {
            let t = lua.create_table()?;
            t.set("family", spec)?;
            Ok(t)
        })?,
    )?;

    // font_with_fallback({ '主字体', '回退1', ... }) —— 数组形式正好对应
    // VectorTerm 的 `font = { '主字体', '回退1' }` 写法。
    wezterm.set(
        "font_with_fallback",
        lua.create_function(|_, list: Table| Ok(list))?,
    )?;

    // nerdfonts:字形名 → 私用区字符。表由 `tools/gen_nerdfonts.py` 从 Nerd Font
    // 的 `post` + `cmap` 生成,键名与 wezterm 一致(`md_weather_night`)。
    // 用 `__index` 懒查而不是一次性铺满一万多项:不用状态栏的配置一分钱不花,
    // 未收录的名字返回空串(wezterm 给 nil,直接拼进字符串会报错)。
    let nerdfonts = lua.create_table()?;
    let nf_mt = lua.create_table()?;
    nf_mt.set(
        "__index",
        lua.create_function(|lua, (_t, key): (Table, String)| {
            match crate::nerdfonts::lookup(&key) {
                Some(ch) => Ok(lua.create_string(ch.encode_utf8(&mut [0u8; 4]))?),
                None => Ok(lua.create_string("")?),
            }
        })?,
    )?;
    nerdfonts.set_metatable(Some(nf_mt))?;
    wezterm.set("nerdfonts", nerdfonts)?;

    // on(event, callback):把回调存进 registry。同一个事件可以注册多个,
    // 派发时按注册顺序调用(wezterm 里排在前面的是"后注册"的,这里不区分 ——
    // 实际上没人依赖这个顺序)。
    wezterm.set("on", lua.create_function(register_event)?)?;

    // strftime:直接借 Lua 标准库的 os.date(本地时区、格式串一致)
    let os: Table = lua.globals().get("os")?;
    let date: mlua::Function = os.get("date")?;
    wezterm.set(
        "strftime",
        lua.create_function(move |_, fmt: String| date.call::<String>(fmt))?,
    )?;

    // battery_info():本机电池列表,台式机没有电池时是空表
    wezterm.set(
        "battery_info",
        lua.create_function(|lua, ()| {
            let list = lua.create_table()?;
            for (i, b) in crate::status::battery_info().iter().enumerate() {
                let t = lua.create_table()?;
                t.set("state_of_charge", b.state_of_charge)?;
                t.set("state", b.state.as_str())?;
                list.set(i + 1, t)?;
            }
            Ok(list)
        })?,
    )?;

    // format(items):把 { Text } / { Foreground } / { Background } / { Attribute }
    // 转成带 SGR 的字符串 —— 和 wezterm 一样,颜色是**转义序列**而不是结构体,
    // 这样 `window:set_right_status(wezterm.format(...))` 之后 VectorTerm 再解析回来。
    wezterm.set("format", lua.create_function(format_items)?)?;

    // action:wezterm 的 Action 常量既可能直接当值用(`action = wezterm.action.ReloadConfiguration`),
    // 也可能带参数调用(`wezterm.action.SpawnTab 'DefaultDomain'`)。
    // 用「带 __call 的表」同时满足两种写法。
    let action = lua.create_table()?;
    let action_mt = lua.create_table()?;
    action_mt.set(
        "__index",
        lua.create_function(|lua, (_t, name): (Table, String)| {
            let act = lua.create_table()?;
            act.set("__wezterm_action", name.clone())?;
            let mt = lua.create_table()?;
            mt.set(
                "__call",
                lua.create_function(|_, (this, arg): (Table, Value)| {
                    if !matches!(arg, Value::Nil) {
                        this.set("arg", arg)?;
                    }
                    Ok(this)
                })?,
            )?;
            act.set_metatable(Some(mt))?;
            Ok(act)
        })?,
    )?;
    action.set_metatable(Some(action_mt))?;
    wezterm.set("action", action)?;

    // 配置里常见的环境信息
    wezterm.set("home_dir", std::env::var("USERPROFILE").unwrap_or_default())?;
    wezterm.set("target_triple", "x86_64-pc-windows-msvc")?;
    wezterm.set("version", env!("CARGO_PKG_VERSION"))?;

    lua.globals().set("wezterm", wezterm.clone())?;
    // `local wezterm = require 'wezterm'` 也要能拿到同一个表
    let package: Table = lua.globals().get("package")?;
    let preload: Table = package.get("preload")?;
    preload.set(
        "wezterm",
        lua.create_function(move |_, _: Value| Ok(wezterm.clone()))?,
    )?;

    Ok(())
}

/// `wezterm.on(event, callback)`
fn register_event(lua: &Lua, (event, func): (String, mlua::Function)) -> mlua::Result<()> {
    let key = format!("{EVENT_PREFIX}{event}");
    let list: Table = match lua.named_registry_value::<Value>(&key)? {
        Value::Table(t) => t,
        _ => {
            let t = lua.create_table()?;
            lua.set_named_registry_value(&key, &t)?;
            t
        }
    };
    let n = list.raw_len();
    list.set(n + 1, func)?;
    Ok(())
}

/// 取某个事件登记过的全部回调(派发由 [`crate::script`] 负责)
pub fn event_callbacks(lua: &Lua, event: &str) -> Vec<mlua::Function> {
    let key = format!("{EVENT_PREFIX}{event}");
    match lua.named_registry_value::<Value>(&key) {
        Ok(Value::Table(t)) => t.sequence_values().flatten().collect(),
        _ => Vec::new(),
    }
}

/// `wezterm.format(items)` —— 逐项翻译成 SGR 序列。
///
/// 支持的写法(和 wezterm 的 `FormatItem` 一致):
/// * `{ Text = '...' }`
/// * `{ Foreground = { Color = '#rrggbb' } }` / `{ Foreground = { AnsiColor = 'Maroon' } }`
///   / `{ Foreground = '#rrggbb' }`(简写)
/// * `{ Background = ... }` 同上
/// * `{ Attribute = { Intensity = 'Bold' } }` / `{ Italic = true }` /
///   `{ Underline = 'Single' }` / `{ Strikethrough = true }`
/// * `{ ResetAttributes }`
fn format_items(_lua: &Lua, items: Table) -> mlua::Result<String> {
    let mut out = String::new();
    for item in items.sequence_values::<Table>().flatten() {
        if let Ok(Some(text)) = item.get::<Option<String>>("Text") {
            out.push_str(&text);
            continue;
        }
        if let Ok(Some(c)) = item.get::<Option<Table>>("Foreground") {
            out.push_str(&color_sgr(&c, true));
            continue;
        }
        if let Ok(Some(c)) = item.get::<Option<Table>>("Background") {
            out.push_str(&color_sgr(&c, false));
            continue;
        }
        if let Ok(Some(attr)) = item.get::<Option<Table>>("Attribute") {
            out.push_str(&attr_sgr(&attr));
            continue;
        }
        let reset: Value = item.get("ResetAttributes").unwrap_or(Value::Nil);
        if !matches!(reset, Value::Nil) {
            out.push_str("\u{1b}[0m");
        }
    }
    Ok(out)
}

/// `{ Color = '#ff0000' }` 或 `{ AnsiColor = 'Maroon' }`
fn color_sgr(spec: &Table, fg: bool) -> String {
    let layer = if fg { 38 } else { 48 };
    if let Ok(Some(s)) = spec.get::<Option<String>>("Color") {
        if let Some(c) = parse_color(&s) {
            return format!("\u{1b}[{layer};2;{};{};{}m", c.r, c.g, c.b);
        }
    }
    if let Ok(Some(name)) = spec.get::<Option<String>>("AnsiColor") {
        if let Some(idx) = ansi_index(&name) {
            return format!("\u{1b}[{layer};5;{idx}m");
        }
    }
    String::new()
}

/// wezterm 的颜色字符串:`#rgb` / `#rrggbb` / `rgb:...` / `rgba:...` 等,
/// 复用终端自己那套解析,少一份特例。
pub fn parse_color(s: &str) -> Option<crate::render::Rgb> {
    crate::term::color_from_str(s)
}

/// `{ Color = '#ff0000' }` 或 `{ AnsiColor = 'Maroon' }` → 具体颜色。
/// `format` 那边故意保留"索引色"的写法好让渲染层查同一张调色板;这里要给
/// 每个片段定色(`format-tab-title`),所以直接落成 RGB。
pub fn color_from_spec(
    spec: &Table,
    ansi: &[crate::render::Rgb; 256],
) -> Option<crate::render::Rgb> {
    if let Ok(Some(s)) = spec.get::<Option<String>>("Color") {
        if let Some(c) = parse_color(&s) {
            return Some(c);
        }
    }
    if let Ok(Some(name)) = spec.get::<Option<String>>("AnsiColor") {
        if let Some(i) = ansi_index(&name) {
            return Some(ansi[i as usize]);
        }
    }
    None
}

/// `AnsiColor` 的 16 个名字,顺序就是 Windows 控制台 / ANSI 下标
fn ansi_index(name: &str) -> Option<u8> {
    const NAMES: [&str; 16] = [
        "black", "maroon", "green", "olive", "navy", "purple", "teal", "silver", "grey", "red",
        "lime", "yellow", "blue", "fuchsia", "aqua", "white",
    ];
    let key = name.trim().to_ascii_lowercase();
    NAMES.iter().position(|n| *n == key).map(|i| i as u8)
}

fn attr_sgr(attr: &Table) -> String {
    let mut out = String::new();
    if let Ok(Some(v)) = attr.get::<Option<String>>("Intensity") {
        match v.trim().to_ascii_lowercase().as_str() {
            "bold" => out.push_str("\u{1b}[1m"),
            "half" => out.push_str("\u{1b}[2m"),
            _ => {}
        }
    }
    if attr.get::<Option<bool>>("Italic").ok().flatten() == Some(true) {
        out.push_str("\u{1b}[3m");
    }
    if attr.get::<Option<bool>>("Strikethrough").ok().flatten() == Some(true) {
        out.push_str("\u{1b}[9m");
    }
    // Underline 既可能是 bool,也可能是 'Single' / 'Double' 这类枚举名
    match attr.get::<Value>("Underline") {
        Ok(Value::Boolean(true)) => out.push_str("\u{1b}[4m"),
        Ok(Value::String(s)) if !s.to_string_lossy().is_empty() => out.push_str("\u{1b}[4m"),
        _ => {}
    }
    if let Ok(Some(v)) = attr.get::<Option<bool>>("Blink") {
        if v {
            out.push_str("\u{1b}[5m");
        }
    }
    if attr.get::<Option<bool>>("Reverse").ok().flatten() == Some(true) {
        out.push_str("\u{1b}[7m");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一段典型的 wezterm 配置能跑通,并解析出字体与 action
    #[test]
    fn wezterm_style_config_evaluates() {
        let lua = Lua::new();
        install(&lua).unwrap();
        let tbl: Table = lua
            .load(
                r#"
                local wezterm = require 'wezterm'
                local config = wezterm.config_builder()
                config.font = wezterm.font_with_fallback({ 'MesloLGM Nerd Font', 'Cascadia Code' })
                config.font_size = 10.0
                config.keys = {
                  { key = 'T', mods = 'CTRL|SHIFT', action = wezterm.action.SpawnTab 'DefaultDomain' },
                  { key = 'R', mods = 'CTRL|SHIFT', action = wezterm.action.ReloadConfiguration },
                }
                return config
            "#,
            )
            .eval()
            .expect("wezterm 风格配置应能执行");

        let font: Table = tbl.get("font").unwrap();
        assert_eq!(font.get::<String>(1).unwrap(), "MesloLGM Nerd Font");
        assert_eq!(font.get::<String>(2).unwrap(), "Cascadia Code");

        let keys: Table = tbl.get("keys").unwrap();
        let first: Table = keys.get(1).unwrap();
        let action: Table = first.get("action").unwrap();
        assert_eq!(
            action.get::<String>("__wezterm_action").unwrap(),
            "SpawnTab"
        );
        assert_eq!(action.get::<String>("arg").unwrap(), "DefaultDomain");
    }

    /// `wezterm.format` 产出的串要能被 `status::parse` 原样读回颜色 ——
    /// 这是状态栏那条链路的端到端契约。
    #[test]
    fn format_emits_parseable_sgr() {
        let lua = Lua::new();
        install(&lua).unwrap();
        let text: String = lua
            .load(
                r#"
                local wezterm = require 'wezterm'
                return wezterm.format({
                  { Text = 'a' },
                  { Foreground = { Color = '#ff0000' } },
                  { Text = 'b' },
                  { Attribute = { Intensity = 'Bold' } },
                  { Background = { AnsiColor = 'Navy' } },
                  { Text = 'c' },
                  { ResetAttributes },
                })
            "#,
            )
            .eval()
            .unwrap();

        let palette = [crate::render::Rgb { r: 0, g: 0, b: 0 }; 256];
        let spans = crate::status::parse(&text, &palette);
        assert_eq!(spans.len(), 3, "三段不同样式:{text}");
        assert_eq!(spans[0].text, "a");
        assert!(spans[0].fg.is_none());
        assert_eq!(spans[1].text, "b");
        assert_eq!(spans[1].fg, Some(crate::render::Rgb { r: 255, g: 0, b: 0 }));
        assert!(spans[2].text.starts_with('c'), "c 后面还跟着 SGR 0");
        assert!(spans[2].bold, "Bold 要能带过去");
        assert_eq!(spans[2].bg, Some(palette[4]), "Navy 是 4 号色");

        let year: String = lua
            .load("return require 'wezterm'.strftime('%Y')")
            .eval()
            .unwrap();
        assert_eq!(year.len(), 4);
    }

    /// `wezterm.nerdfonts` 查的是真表:示例里那几个图标都得有字
    #[test]
    fn nerdfonts_resolve_to_real_glyphs() {
        let lua = Lua::new();
        install(&lua).unwrap();
        let (a, missing): (String, String) = lua
            .load(
                r#"
                local wezterm = require 'wezterm'
                local nf = wezterm.nerdfonts
                return nf.md_weather_night, nf.no_such_icon
            "#,
            )
            .eval()
            .unwrap();
        assert_eq!(a, "\u{f0594}");
        assert_eq!(missing, "", "没收录的名字给空串,拼字符串不会炸");
    }

    /// `wezterm.on` 只登记不执行,派发由 script 层做
    #[test]
    fn on_registers_callbacks() {
        let lua = Lua::new();
        install(&lua).unwrap();
        lua.load(
            r#"
            local wezterm = require 'wezterm'
            wezterm.on('update-right-status', function() end)
            wezterm.on('update-right-status', function() end)
            wezterm.on('format-tab-title', function() end)
        "#,
        )
        .exec()
        .unwrap();
        assert_eq!(event_callbacks(&lua, "update-right-status").len(), 2);
        assert_eq!(event_callbacks(&lua, "format-tab-title").len(), 1);
        assert!(event_callbacks(&lua, "nope").is_empty());
    }
}
