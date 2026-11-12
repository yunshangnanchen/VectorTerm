//! 极简 Lua 表解析器(带字节偏移)。
//!
//! 目的很窄:把配置文件里 `return { ... }` 那张表拆成一棵**带字节偏移**的节点树,
//! 这样改某个键时可以对原文本做定点替换 —— 用户写的注释、空行、对齐都不会被动。
//! 不做求值,不建完整 AST;遇到函数体、`and/or` 表达式这类认不出来的东西就整段
//! 记成 raw 原样留着。
//!
//! 只支持 vectorterm 配置会出现的语法子集:
//!   return { key = <值>, [表达式] = <值>, <值>, ... }
//!
//! 所有 start/end 都是 **UTF-8 字节偏移**(和 &str 切片一致),保证中文注释安全。

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Node {
    pub kind: NodeKind,
    pub start: usize,
    pub end: usize,
    pub value: Option<Value>,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Table,
    Scalar,
    Expr,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub key: Option<Key>,
    pub key_start: isize,
    pub key_end: isize,
    pub node: Node,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Key {
    Ident(String),
    Computed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Num(f64),
    Int(i64),
    Bool(bool),
    Nil,
    Expr(String),
    Table(Table),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Table {
    Array(Vec<Value>),
    Map(HashMap<String, Value>),
}

impl Default for Table {
    fn default() -> Self {
        Table::Map(HashMap::new())
    }
}

#[derive(Debug)]
pub struct LuaError(pub String);

impl std::fmt::Display for LuaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub struct Document {
    pub text: String,
    pub root: Option<Node>,
}

impl Document {
    pub fn find(&self, path: &[&str]) -> Option<&Entry> {
        let mut node = self.root.as_ref()?;
        if node.kind != NodeKind::Table {
            return None;
        }
        for (depth, key) in path.iter().enumerate() {
            let mut found = None;
            for e in &node.entries {
                if let Some(Key::Ident(k)) = &e.key {
                    if k == key {
                        found = Some(e);
                        break;
                    }
                }
            }
            let found = found?;
            if depth < path.len() - 1 {
                node = &found.node;
                if node.kind != NodeKind::Table {
                    return None;
                }
            } else {
                return Some(found);
            }
        }
        None
    }

    pub fn get(&self, path: &[&str]) -> Option<&Value> {
        self.find(path).and_then(|e| e.node.value.as_ref())
    }
}

// ---------------------------------------------------------------------
// 字符工具(全部基于字节偏移)
// ---------------------------------------------------------------------

/// 当前位置的字符(安全)。
#[inline]
fn ch_at(s: &str, i: usize) -> Option<char> {
    s[i..].chars().next()
}

/// 当前位置是否以某字符串开头。
#[inline]
fn starts_with(s: &str, i: usize, pat: &str) -> bool {
    s[i..].starts_with(pat)
}

/// 跳过空白、行注释、块注释。返回字节偏移。
/// 解析长括号开头(`[`、`[==[` …),返回 `(等号个数, 内容起始偏移)`。
///
/// Lua 的长括号允许任意多个等号(`[[`、`[=[`、`[==[` …),开闭必须成对且
/// 等号个数相同。旧实现只认 0 / 1 个等号,于是 `--[====[ … ]====]` 这类注释
/// 会被当成"未闭合"一路吞到文件尾,整个文件解析不出根表。
fn long_bracket_open(s: &str, i: usize) -> Option<(usize, usize)> {
    if ch_at(s, i)? != '[' {
        return None;
    }
    let mut j = i + 1;
    let mut eq = 0usize;
    while j < s.len() && ch_at(s, j)? == '=' {
        eq += 1;
        j += 1;
    }
    if j < s.len() && ch_at(s, j)? == '[' {
        Some((eq, j + 1))
    } else {
        None
    }
}

/// 从 `from` 起找与 `eq` 个等号匹配的长括号结束符(`]==]`),返回其起始偏移。
fn long_bracket_close(s: &str, from: usize, eq: usize) -> Option<usize> {
    let mut pat = String::with_capacity(eq + 2);
    pat.push(']');
    for _ in 0..eq {
        pat.push('=');
    }
    pat.push(']');
    find_from(s, from, &pat)
}

fn skip(s: &str, mut i: usize) -> usize {
    let n = s.len();
    while i < n {
        let c = ch_at(s, i).unwrap();
        if c == ' ' || c == '\t' || c == '\r' || c == '\n' {
            i += c.len_utf8();
        } else if starts_with(s, i, "--") {
            // 长注释 `--[==[ … ]==]`(等号个数可变),否则按行注释处理
            if let Some((eq, body)) = long_bracket_open(s, i + 2) {
                match long_bracket_close(s, body, eq) {
                    Some(p) => i = p + eq + 2,
                    None => i = n,
                }
            } else {
                match s[i..].find('\n') {
                    Some(p) => i = i + p,
                    None => i = n,
                }
            }
        } else {
            break;
        }
    }
    i
}

/// 从 from 起找 pat 的字节偏移。
fn find_from(s: &str, from: usize, pat: &str) -> Option<usize> {
    s[from..].find(pat).map(|p| p + from)
}

/// 匹配一个标识符,返回 (word, 结束字节偏移)。
fn ident_at(s: &str, i: usize) -> Option<(String, usize)> {
    let c = ch_at(s, i)?;
    if !(c.is_ascii_alphabetic() || c == '_') {
        return None;
    }
    let mut j = i + 1;
    while j < s.len() {
        let c = ch_at(s, j).unwrap();
        if c.is_ascii_alphanumeric() || c == '_' {
            j += 1;
        } else {
            break;
        }
    }
    Some((s[i..j].to_owned(), j))
}

// ---------------------------------------------------------------------
// 解析
// ---------------------------------------------------------------------

/// 找**顶层**的 `return {` 的 `{` 位置。
///
/// 只找"第一个 return"是不够的:配置文件开头常带函数定义,例如
/// `local function slot(h) if h < 5 then return '夜深了' end … end`,
/// 老实现遇到函数体里的 `return`(后面跟字符串而不是表)就判定整份文件
/// 解析失败 —— 结果是"改了配置想保存,提示无法解析"。
///
/// 这里按 Lua 的块结构维护深度:`function` / `if` / `do` / `repeat` 开块,
/// `end` / `until` 闭块,只有深度 0 的 `return {` 才算文件的主返回表。
/// (`elseif … then` 属于同一个 if 块,所以 `then`、`elseif` 都不计入。)
fn top_level_return_table(s: &str) -> Option<usize> {
    let n = s.len();
    let mut i = 0usize;
    let mut depth: i32 = 0;
    while i < n {
        // 空白与注释:直接跳过,注释里的关键字不算数
        let j = skip(s, i);
        if j != i {
            i = j;
            continue;
        }
        let c = ch_at(s, i)?;
        // 字符串里的 `end` / `return` 同样不算数
        if c == '"' || c == '\'' {
            let (_, k) = string(s, i).ok()?;
            i = k;
            continue;
        }
        if long_bracket_open(s, i).is_some() {
            let (_, k) = long_string(s, i).ok()?;
            i = k;
            continue;
        }
        if let Some((w, next)) = ident_at(s, i) {
            match w.as_str() {
                "function" | "if" | "do" | "repeat" => depth += 1,
                "end" | "until" => depth -= 1,
                "return" if depth <= 0 => {
                    let k = skip(s, next);
                    if ch_at(s, k) == Some('{') {
                        return Some(k);
                    }
                    i = next;
                    continue;
                }
                _ => {}
            }
            i = next;
            continue;
        }
        i += c.len_utf8();
    }
    None
}

pub fn parse(text: &str) -> Document {
    let root = top_level_return_table(text).and_then(|j| table(text, j).ok());
    Document {
        text: text.to_owned(),
        root,
    }
}

fn table(s: &str, i: usize) -> Result<Node, LuaError> {
    let start = i;
    let mut idx = i + 1; // 吃掉 '{'
    let mut entries: Vec<Entry> = Vec::new();
    let n = s.len();
    loop {
        idx = skip(s, idx);
        if idx >= n {
            return Err(LuaError("表没有闭合(缺少 })".into()));
        }
        if ch_at(s, idx) == Some('}') {
            let end = idx + 1;
            let val = to_value(&entries);
            return Ok(Node {
                kind: NodeKind::Table,
                start,
                end,
                value: Some(val),
                entries,
            });
        }
        if ch_at(s, idx) == Some(',') || ch_at(s, idx) == Some(';') {
            idx += 1;
            continue;
        }

        let e_start = idx;
        let mut key: Option<Key> = None;
        let mut key_start: isize = -1;
        let mut key_end: isize = -1;

        if ch_at(s, idx) == Some('[') {
            let close = match_bracket(s, idx)?;
            let expr: String = s[idx + 1..close].trim().to_owned();
            let j = skip(s, close + 1);
            if j < n && ch_at(s, j) == Some('=') {
                key_start = idx as isize;
                key_end = (close + 1) as isize;
                key = Some(Key::Computed(expr));
                idx = skip(s, j + 1);
            }
        }
        if key.is_none() {
            if let Some((word, end)) = ident_at(s, idx) {
                let j = skip(s, end);
                if j < n && ch_at(s, j) == Some('=') && !starts_with(s, j, "==") {
                    key_start = idx as isize;
                    key_end = end as isize;
                    key = Some(Key::Ident(word));
                    idx = skip(s, j + 1);
                }
            }
        }

        let node = value(s, idx)?;
        idx = node.end;
        entries.push(Entry {
            key,
            key_start,
            key_end,
            node,
            start: e_start,
            end: idx,
        });
    }
}

fn match_bracket(s: &str, i: usize) -> Result<usize, LuaError> {
    let mut depth = 0;
    let mut idx = i;
    let n = s.len();
    while idx < n {
        let c = ch_at(s, idx).unwrap();
        if c == '"' || c == '\'' {
            let (_, j) = string(s, idx)?;
            idx = j;
            continue;
        }
        if c == '[' {
            depth += 1;
        } else if c == ']' {
            depth -= 1;
            if depth == 0 {
                return Ok(idx);
            }
        }
        idx += c.len_utf8();
    }
    Err(LuaError("`[` 没有配对的 `]`".into()))
}

fn value(s: &str, i: usize) -> Result<Node, LuaError> {
    let idx = skip(s, i);
    if idx >= s.len() {
        return Err(LuaError("表项缺少值".into()));
    }
    let c = ch_at(s, idx).unwrap();
    if c == '{' {
        return table(s, idx);
    }
    if c == '"' || c == '\'' {
        let (v, j) = string(s, idx)?;
        return Ok(Node {
            kind: NodeKind::Scalar,
            start: idx,
            end: j,
            value: Some(Value::Str(v)),
            entries: vec![],
        });
    }
    if long_bracket_open(s, idx).is_some() {
        let (v, j) = long_string(s, idx)?;
        return Ok(Node {
            kind: NodeKind::Scalar,
            start: idx,
            end: j,
            value: Some(Value::Str(v)),
            entries: vec![],
        });
    }
    if let Some((val, end)) = number_at(s, idx) {
        return Ok(Node {
            kind: NodeKind::Scalar,
            start: idx,
            end,
            value: Some(val),
            entries: vec![],
        });
    }
    if let Some((word, end)) = ident_at(s, idx) {
        match word.as_str() {
            "true" => {
                return Ok(Node {
                    kind: NodeKind::Scalar,
                    start: idx,
                    end,
                    value: Some(Value::Bool(true)),
                    entries: vec![],
                })
            }
            "false" => {
                return Ok(Node {
                    kind: NodeKind::Scalar,
                    start: idx,
                    end,
                    value: Some(Value::Bool(false)),
                    entries: vec![],
                })
            }
            "nil" => {
                return Ok(Node {
                    kind: NodeKind::Scalar,
                    start: idx,
                    end,
                    value: Some(Value::Nil),
                    entries: vec![],
                })
            }
            _ => {}
        }
    }
    expr(s, idx)
}

fn expr(s: &str, i: usize) -> Result<Node, LuaError> {
    let start = i;
    let mut idx = i;
    let n = s.len();
    let mut depth = 0;
    while idx < n {
        let c = ch_at(s, idx).unwrap();
        if c == '"' || c == '\'' {
            let (_, j) = string(s, idx)?;
            idx = j;
            continue;
        }
        if starts_with(s, idx, "--") {
            idx = skip(s, idx);
            continue;
        }
        if c == '(' || c == '[' || c == '{' {
            depth += 1;
        } else if c == ')' || c == ']' || c == '}' {
            if depth == 0 {
                break;
            }
            depth -= 1;
        } else if (c == ',' || c == ';') && depth == 0 {
            break;
        }
        idx += c.len_utf8();
    }
    let mut end = idx;
    while end > start {
        let c = ch_at(s, end - 1).unwrap();
        if c == ' ' || c == '\t' || c == '\r' || c == '\n' {
            end -= 1;
        } else {
            break;
        }
    }
    if end <= start {
        return Err(LuaError("表项缺少值".into()));
    }
    let text = s[start..end].to_owned();
    Ok(Node {
        kind: NodeKind::Expr,
        start,
        end,
        value: Some(Value::Expr(text)),
        entries: vec![],
    })
}

fn string(s: &str, i: usize) -> Result<(String, usize), LuaError> {
    let quote = ch_at(s, i).unwrap();
    let mut idx = i + 1;
    let mut out = String::new();
    let n = s.len();
    while idx < n {
        let c = ch_at(s, idx).unwrap();
        if c == '\\' {
            if idx + 1 >= n {
                break;
            }
            let nxt = ch_at(s, idx + 1).unwrap();
            let esc = match nxt {
                'n' => Some('\n'),
                't' => Some('\t'),
                'r' => Some('\r'),
                'a' => Some('\u{07}'),
                'b' => Some('\u{08}'),
                'f' => Some('\u{0c}'),
                'v' => Some('\u{0b}'),
                '\\' => Some('\\'),
                '"' => Some('"'),
                '\'' => Some('\''),
                '\n' => Some('\n'),
                _ => None,
            };
            if let Some(e) = esc {
                out.push(e);
                idx += 2;
                continue;
            }
            if nxt == 'x' && idx + 3 < n {
                let hex = &s[idx + 2..idx + 4];
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b as char);
                    idx += 4;
                    continue;
                }
                out.push(nxt);
                idx += 2;
                continue;
            }
            if nxt.is_ascii_digit() {
                let mut j = idx + 1;
                let mut d = 0u32;
                while j < n && j < idx + 4 && ch_at(s, j).unwrap().is_ascii_digit() {
                    d = d * 10 + ch_at(s, j).unwrap().to_digit(10).unwrap();
                    j += 1;
                }
                out.push((d & 0xFF) as u8 as char);
                idx = j;
                continue;
            }
            out.push(nxt);
            idx += 2;
            continue;
        }
        if c == quote {
            return Ok((out, idx + 1));
        }
        if c == '\n' {
            return Ok((out, idx));
        }
        out.push(c);
        idx += c.len_utf8();
    }
    Err(LuaError("字符串没有闭合".into()))
}

fn long_string(s: &str, i: usize) -> Result<(String, usize), LuaError> {
    // 支持任意等号个数的长括号:`[[x]]`、`[=[x]=]`、`[==[x]==]` …
    let Some((eq, body)) = long_bracket_open(s, i) else {
        return Err(LuaError("不是长括号字符串".into()));
    };
    match long_bracket_close(s, body, eq) {
        Some(j) => Ok((s[body..j].to_owned(), j + eq + 2)),
        None => Err(LuaError("长字符串没有闭合".into())),
    }
}

fn number_at(s: &str, i: usize) -> Option<(Value, usize)> {
    let mut j = i;
    let n = s.len();
    if j < n && (ch_at(s, j)? == '+' || ch_at(s, j)? == '-') {
        j += 1;
    }
    // 十六进制
    if j + 1 < n && ch_at(s, j)? == '0' && (ch_at(s, j + 1)? == 'x' || ch_at(s, j + 1)? == 'X') {
        j += 2;
        let hs = j;
        while j < n && ch_at(s, j)?.is_ascii_hexdigit() {
            j += 1;
        }
        if j == hs {
            return None;
        }
        let lit = &s[i..j];
        let hexpart = lit
            .trim_start_matches(['+', '-'])
            .trim_start_matches("0x")
            .trim_start_matches("0X");
        let v = i64::from_str_radix(hexpart, 16).ok();
        return v.map(|v| (Value::Int(v), j));
    }
    let mut has_dot = false;
    let mut has_digit = false;
    while j < n && ch_at(s, j)?.is_ascii_digit() {
        has_digit = true;
        j += 1;
    }
    if j < n && ch_at(s, j)? == '.' {
        has_dot = true;
        j += 1;
        while j < n && ch_at(s, j)?.is_ascii_digit() {
            has_digit = true;
            j += 1;
        }
    }
    if !has_digit {
        return None;
    }
    if j < n && (ch_at(s, j)? == 'e' || ch_at(s, j)? == 'E') {
        let mut k = j + 1;
        if k < n && (ch_at(s, k)? == '+' || ch_at(s, k)? == '-') {
            k += 1;
        }
        let mut has_exp = false;
        while k < n && ch_at(s, k)?.is_ascii_digit() {
            has_exp = true;
            k += 1;
        }
        if has_exp {
            j = k;
        }
    }
    if j < n && (ch_at(s, j)?.is_ascii_alphanumeric() || ch_at(s, j)? == '_') {
        return None;
    }
    let lit = &s[i..j];
    if has_dot || lit.contains('e') || lit.contains('E') {
        lit.parse::<f64>().ok().map(|v| (Value::Num(v), j))
    } else {
        lit.parse::<i64>().ok().map(|v| (Value::Int(v), j))
    }
}

fn to_value(entries: &[Entry]) -> Value {
    if !entries.is_empty() && entries.iter().all(|e| e.key.is_none()) {
        Value::Table(Table::Array(
            entries
                .iter()
                .filter_map(|e| e.node.value.clone())
                .collect(),
        ))
    } else {
        let mut map = HashMap::new();
        for e in entries {
            if let Some(Key::Ident(k)) = &e.key {
                if let Some(v) = e.node.value.clone() {
                    map.insert(k.clone(), v);
                }
            }
        }
        Value::Table(Table::Map(map))
    }
}

// ---------------------------------------------------------------------
// 序列化
// ---------------------------------------------------------------------

const KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

fn is_ident(s: &str) -> bool {
    let mut cs = s.chars();
    match cs.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn num(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{}", (v * 1e6).round() / 1e6)
    }
}

pub fn to_lua(value: &Value, indent: usize, level: usize) -> String {
    match value {
        Value::Nil => "nil".into(),
        Value::Bool(b) => (if *b { "true" } else { "false" }).into(),
        Value::Int(i) => i.to_string(),
        Value::Num(f) => num(*f),
        Value::Str(s) => quote(s),
        Value::Expr(e) => e.clone(),
        Value::Table(t) => table_to_lua(t, indent, level),
    }
}

fn table_to_lua(t: &Table, indent: usize, level: usize) -> String {
    let parts: Vec<String> = match t {
        Table::Array(v) => v.iter().map(|x| to_lua(x, indent, level + 1)).collect(),
        Table::Map(m) => m
            .iter()
            .map(|(k, v)| {
                let key = if is_ident(k) && !KEYWORDS.contains(&k.as_str()) {
                    k.clone()
                } else {
                    format!("[{}]", quote(k))
                };
                format!("{} = {}", key, to_lua(v, indent, level + 1))
            })
            .collect(),
    };
    inline_or_block(parts, indent, level)
}

fn inline_or_block(parts: Vec<String>, indent: usize, level: usize) -> String {
    if parts.is_empty() {
        return "{}".into();
    }
    let one = format!("{{ {} }}", parts.join(", "));
    if one.chars().count() <= 72 && !one.contains('\n') {
        return one;
    }
    let pad = " ".repeat(indent * (level + 1));
    let close = " ".repeat(indent * level);
    let mut body = String::new();
    for p in &parts {
        body.push('\n');
        body.push_str(&pad);
        body.push_str(p);
        body.push(',');
    }
    format!("{{{}\n{}}}", body, close)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple() {
        let doc = parse("return {\n  shell = \"pwsh.exe\",\n  size = 18,\n  flag = true,\n}");
        assert!(doc.root.is_some());
        assert_eq!(doc.get(&["shell"]).unwrap(), &Value::Str("pwsh.exe".into()));
        assert_eq!(doc.get(&["size"]).unwrap(), &Value::Int(18));
        assert_eq!(doc.get(&["flag"]).unwrap(), &Value::Bool(true));
    }

    #[test]
    fn parse_nested_and_comments() {
        let src = "return {\n  -- 字体相关\n  font = {\n    size = 18,   -- 字号\n    family = \"C:/Windows/Fonts/consola.ttf\",\n  },\n  colors = { background = \"#0c0c0c\" },\n}";
        let doc = parse(src);
        assert!(doc.root.is_some());
        assert_eq!(doc.get(&["font", "size"]).unwrap(), &Value::Int(18));
        assert_eq!(
            doc.get(&["font", "family"]).unwrap(),
            &Value::Str("C:/Windows/Fonts/consola.ttf".into())
        );
        assert_eq!(
            doc.get(&["colors", "background"]).unwrap(),
            &Value::Str("#0c0c0c".into())
        );
    }

    #[test]
    fn parse_array() {
        let doc = parse("return { shell_args = { \"-NoLogo\" } }");
        assert_eq!(
            doc.get(&["shell_args"]).unwrap(),
            &Value::Table(Table::Array(vec![Value::Str("-NoLogo".into())]))
        );
    }

    /// 主题文件常用 `--[====[ … ]====]` 长注释当文件头。旧解析器只认 0/1 个
    /// 等号,会把整份文件当注释吞掉(root=None),保存时再整份重建 —— 主题就
    /// 被清空了。这里守住"任意等号个数都能跳过"。
    #[test]
    fn parse_long_bracket_comments_with_any_equals() {
        let src = "--[====[\n  主题:DeepSeek Dark\n  ] 里面还有 ]=] 这种干扰\n]====]\n\
                   return {\n  colors = { background = \"#0B1220\" },\n}\n";
        let doc = parse(src);
        assert!(doc.root.is_some(), "长注释后应能解析出根表");
        assert_eq!(
            doc.get(&["colors", "background"]).unwrap(),
            &Value::Str("#0B1220".into())
        );

        // 0 / 1 / 2 个等号都要支持
        for (open, close) in [("--[[", "]]"), ("--[=[", "]=]"), ("--[==[", "]==]")] {
            let s = format!("{open} 注释 {close}\nreturn {{ x = 1 }}");
            let d = parse(&s);
            assert!(d.root.is_some(), "{open} 应被正确跳过");
            assert_eq!(d.get(&["x"]).unwrap(), &Value::Int(1));
        }
    }

    /// 长字符串同样支持任意等号个数。
    #[test]
    fn parse_long_string_with_any_equals() {
        let doc = parse("return { a = [==[ x ]] y ]==] }");
        assert_eq!(doc.get(&["a"]).unwrap(), &Value::Str(" x ]] y ".into()));
    }

    /// 顶层 `return {` 之前可以有函数定义 —— 函数体里的 `return '…'`
    /// 不能被当成文件的返回值(否则整份文件会被判成"解析失败")。
    #[test]
    fn parse_ignores_return_inside_functions() {
        let src = r#"
local function slot(h)
  if h < 5 then
    return '夜深了'
  elseif h < 9 then
    return '早安'
  end
  return '晚安'
end

wezterm.on('update-right-status', function(window, pane)
  local x = slot(3)
  return x
end)

return {
  font = { size = 15 },
}
"#;
        let doc = parse(src);
        let root = doc.root.as_ref().expect("应能解析出顶层 return 表");
        assert!(matches!(doc.get(&["font"]), Some(Value::Table(_))));
        assert_eq!(root.entries.len(), 1);
    }

    /// 字符串里的 `end` / `return` 也不能影响块深度。
    #[test]
    fn parse_ignores_keywords_inside_strings() {
        let src = "local s = 'end return'\nreturn { a = 1 }\n";
        let doc = parse(src);
        assert_eq!(doc.get(&["a"]).unwrap(), &Value::Int(1));
    }

    #[test]
    fn to_lua_roundtrip() {
        assert_eq!(to_lua(&Value::Str("hi".into()), 2, 0), "\"hi\"");
        assert_eq!(to_lua(&Value::Int(18), 2, 0), "18");
        assert_eq!(to_lua(&Value::Bool(true), 2, 0), "true");
        assert_eq!(to_lua(&Value::Nil, 2, 0), "nil");
        assert_eq!(
            to_lua(
                &Value::Table(Table::Array(vec![
                    Value::Str("a".into()),
                    Value::Str("b".into())
                ])),
                2,
                0
            ),
            "{ \"a\", \"b\" }"
        );
    }
}
