//! 配置文档层:读入 Lua 文本 → 定点改写 → 输出新文本。
//!
//! 核心思路是**只替换该替换的那几个字符**,用户写的注释、空行、对齐都原样留着。
//! 只有"新增一个键"时才生成新行,样式跟着相邻行的缩进走。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::lua::{self, Document, Entry, Value};

#[derive(Debug, Clone)]
struct Edit {
    start: usize,
    end: usize,
    text: String,
}

// ---------------------------------------------------------------------
// 读写
// ---------------------------------------------------------------------

pub fn read_text(path: &Path) -> String {
    let raw = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return String::new(),
    };
    // 配置文件带中文注释,统一按 UTF-8;带 BOM 也照收。
    let text = if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(&raw[3..]).into_owned()
    } else {
        match String::from_utf8(raw.clone()) {
            Ok(s) => s,
            Err(_) => {
                // 退回 GBK
                let (s, _, _) = encoding_rs_decode(&raw);
                s
            }
        }
    };
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// 极简 GBK 解码回退(不引第三方编码库,覆盖常见中文注释场景)。
fn encoding_rs_decode(raw: &[u8]) -> (String, bool, bool) {
    // 先用 gbk 试,失败的字节用 '?' 替代。
    let mut out = String::new();
    let mut i = 0;
    while i < raw.len() {
        let b = raw[i];
        if b < 0x80 {
            out.push(b as char);
            i += 1;
        } else if i + 1 < raw.len() {
            let code = ((b as u16) << 8) | raw[i + 1] as u16;
            // GBK 双字节区大致范围
            if (0x8140..=0xFEFE).contains(&code) {
                // 用 Windows-936 码表太重,这里退化成直接按 UTF-8 再试一次已在上层处理;
                // 简单起见保留原字节的 Unicode 私用映射并标记为失败。
                out.push(char::from_u32(0xFFFD).unwrap());
                i += 2;
            } else {
                out.push(char::from_u32(0xFFFD).unwrap());
                i += 1;
            }
        } else {
            out.push(char::from_u32(0xFFFD).unwrap());
            i += 1;
        }
    }
    (out, false, false)
}

pub fn write_text(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut out = text.to_owned();
    // 原文件是 CRLF 就继续 CRLF。
    if let Ok(old) = std::fs::read(path) {
        if old.windows(2).any(|w| w == b"\r\n") {
            out = out.replace('\n', "\r\n");
        }
    }
    std::fs::write(path, out.as_bytes())
}

pub fn empty_config() -> String {
    "-- VectorTerm 配置文件\n\
     -- 由「VectorTerm 配置管理器」生成;也可以直接用编辑器改,两边都认。\n\
     -- 逐项说明见 docs/config-manual.md。\n\
     return {\n\
     }\n"
    .to_owned()
}

// ---------------------------------------------------------------------
// 缩进 / 位置工具
// ---------------------------------------------------------------------

fn line_indent(text: &str, pos: usize) -> String {
    let ls = text[..pos].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let mut j = ls;
    while j < text.len() {
        let c = text[j..].chars().next().unwrap();
        if c == ' ' || c == '\t' {
            j += 1;
        } else {
            break;
        }
    }
    text[ls..j].to_owned()
}

fn line_start(text: &str, pos: usize) -> usize {
    text[..pos].rfind('\n').map(|p| p + 1).unwrap_or(0)
}

// ---------------------------------------------------------------------
// 删除一个 entry(连同它行尾的注释、后面的逗号)
// ---------------------------------------------------------------------

fn delete_range(text: &str, entry: &Entry) -> (usize, usize) {
    let mut start = line_start(text, entry.start);
    if !text[start..entry.start].trim().is_empty() {
        start = entry.start;
    }
    let mut end = entry.end;
    let mut j = end;
    while j < text.len() {
        let c = text[j..].chars().next().unwrap();
        if c == ' ' || c == '\t' {
            j += 1;
        } else {
            break;
        }
    }
    if j < text.len() && text[j..].starts_with(',') {
        j += 1;
        while j < text.len() {
            let c = text[j..].chars().next().unwrap();
            if c == ' ' || c == '\t' {
                j += 1;
            } else {
                break;
            }
        }
    }
    if text[j..].starts_with("--") {
        let k = text[j..].find('\n').map(|p| p + j).unwrap_or(text.len());
        j = k;
    }
    if text[j..].starts_with('\n') {
        j += 1;
    }
    end = j;
    if start == end {
        return (entry.start, entry.end);
    }
    (start, end)
}

// ---------------------------------------------------------------------
// 渲染
// ---------------------------------------------------------------------

pub fn render(base: &str, changes: &HashMap<Vec<String>, Option<Value>>) -> String {
    if changes.is_empty() {
        return base.to_owned();
    }
    let doc = lua::parse(base);
    if doc.root.is_none() {
        return render_new(changes);
    }
    let mut edits: Vec<Edit> = Vec::new();
    let mut pending: HashMap<Vec<String>, Vec<(Vec<String>, Value)>> = HashMap::new();

    for (path, value) in changes {
        let path_ref: Vec<&str> = path.iter().map(|s| s.as_str()).collect();
        if let Some(entry) = doc.find(&path_ref) {
            match value {
                None => {
                    let (lo, hi) = delete_range(base, entry);
                    edits.push(Edit {
                        start: lo,
                        end: hi,
                        text: String::new(),
                    });
                }
                Some(v) => {
                    let level = std::cmp::max(1, path.len());
                    let new = lua::to_lua(v, 2, level);
                    edits.push(Edit {
                        start: entry.node.start,
                        end: entry.node.end,
                        text: new,
                    });
                }
            }
        } else if let Some(v) = value {
            // 找最深的已存在祖先表
            let mut prefix: Vec<String> = Vec::new();
            for i in 0..path.len().saturating_sub(1) {
                let cand: Vec<&str> = path[..=i].iter().map(|s| s.as_str()).collect();
                match doc.find(&cand) {
                    Some(e) if e.node.kind == lua::NodeKind::Table => prefix.push(path[i].clone()),
                    _ => break,
                }
            }
            let rest: Vec<String> = path[prefix.len()..].to_vec();
            pending.entry(prefix).or_default().push((rest, v.clone()));
        }
    }

    // 删除后变空的父表:整块摘掉
    for (path, value) in changes {
        if value.is_some() || path.len() < 2 {
            continue;
        }
        let parent_ref: Vec<&str> = path[..path.len() - 1].iter().map(|s| s.as_str()).collect();
        if let Some(parent) = doc.find(&parent_ref) {
            if parent.node.kind == lua::NodeKind::Table && parent.node.entries.len() == 1 {
                let (lo, hi) = delete_range(base, parent);
                edits.push(Edit {
                    start: lo,
                    end: hi,
                    text: String::new(),
                });
            }
        }
    }

    // 新增的键:插到所属表末尾
    for (prefix, items) in &pending {
        let node: &lua::Node = if prefix.is_empty() {
            doc.root.as_ref().unwrap()
        } else {
            let pref: Vec<&str> = prefix.iter().map(|s| s.as_str()).collect();
            match doc.find(&pref) {
                Some(e) if e.node.kind == lua::NodeKind::Table => &e.node,
                _ => continue,
            }
        };
        // 同一个新表下的多个键要合并成一行,否则会写出重复的父键
        // (例如 split_divider.thickness 与 split_divider.gradient.colors
        //  同时改,旧实现会生成两条 split_divider = { ... })。
        let merged = merge_pending(items);
        if let Some(edit) = insert_block(base, node, prefix.len(), &merged) {
            edits.push(edit);
        }
    }

    apply(base, edits)
}

/// 待新增节点:要么是最终值,要么是一层子表。
enum PendingNode {
    Leaf(Value),
    Branch(HashMap<String, PendingNode>),
}

fn insert_pending(
    map: &mut HashMap<String, PendingNode>,
    path: &[String],
    value: &Value,
) {
    if path.is_empty() {
        return;
    }
    if path.len() == 1 {
        map.insert(path[0].clone(), PendingNode::Leaf(value.clone()));
        return;
    }
    let entry = map
        .entry(path[0].clone())
        .or_insert_with(|| PendingNode::Branch(HashMap::new()));
    if !matches!(entry, PendingNode::Branch(_)) {
        // 同一路径既当值又当表:以更具体的子键为准
        *entry = PendingNode::Branch(HashMap::new());
    }
    if let PendingNode::Branch(children) = entry {
        insert_pending(children, &path[1..], value);
    }
}

fn pending_to_value(node: PendingNode) -> Value {
    match node {
        PendingNode::Leaf(v) => v,
        PendingNode::Branch(children) => Value::Table(crate::lua::Table::Map(
            children
                .into_iter()
                .map(|(k, v)| (k, pending_to_value(v)))
                .collect(),
        )),
    }
}

/// 把 (剩余路径, 值) 列表按公共前缀合并,并按键名排序保证输出稳定。
fn merge_pending(items: &[(Vec<String>, Value)]) -> Vec<(Vec<String>, Value)> {
    let mut root: HashMap<String, PendingNode> = HashMap::new();
    for (rest, v) in items {
        insert_pending(&mut root, rest, v);
    }
    let mut out: Vec<(Vec<String>, Value)> = root
        .into_iter()
        .map(|(k, node)| (vec![k], pending_to_value(node)))
        .collect();
    out.sort_by(|a, b| a.0[0].cmp(&b.0[0]));
    out
}

fn insert_block(
    base: &str,
    node: &lua::Node,
    level: usize,
    items: &[(Vec<String>, Value)],
) -> Option<Edit> {
    if items.is_empty() {
        return None;
    }
    if !node.entries.is_empty() {
        let last = node.entries.last().unwrap();
        let indent = line_indent(base, last.start);
        let mut pos = last.end;
        let mut j = pos;
        while j < base.len() {
            let c = base[j..].chars().next().unwrap();
            if c == ' ' || c == '\t' {
                j += 1;
            } else {
                break;
            }
        }
        // 单行表 `{ a = 1 }` 的最后一项后面没有逗号。此时必须先补一个逗号,
        // 否则会写出 `{ a = 1\n  b = 2, }` —— 整份配置直接语法错误、加载失败。
        let mut sep = "";
        if j < base.len() && base[j..].starts_with(',') {
            pos = j + 1;
        } else {
            sep = ",";
        }
        let mut lines = Vec::new();
        for (rest, value) in items {
            lines.push(kv_text(rest, value, level + 1));
        }
        let body: String = format!(
            "{}{}",
            sep,
            lines
                .iter()
                .map(|l| format!("\n{}{}", indent, l))
                .collect::<String>()
        );
        return Some(Edit {
            start: pos,
            end: pos,
            text: body,
        });
    }
    // 空表 {}
    let inner_start = node.start + 1;
    let inner_end = node.end - 1;
    let outer = line_indent(base, node.start);
    let indent = format!("{}  ", outer);
    let lines: Vec<String> = items
        .iter()
        .map(|(r, v)| kv_text(r, v, level + 1))
        .collect();
    let mut body = String::new();
    for l in &lines {
        body.push('\n');
        body.push_str(&indent);
        body.push_str(l);
    }
    body.push('\n');
    body.push_str(&outer);
    Some(Edit {
        start: inner_start,
        end: inner_end,
        text: body,
    })
}

fn kv_text(rest: &[String], value: &Value, level: usize) -> String {
    let key = rest[0].clone();
    if rest.len() == 1 {
        return format!("{} = {},", lua_key(&key), lua::to_lua(value, 2, level));
    }
    let mut nested = value.clone();
    for k in rest[1..].iter().rev() {
        let mut m = HashMap::new();
        m.insert(k.clone(), nested);
        nested = Value::Table(lua::Table::Map(m));
    }
    format!("{} = {},", lua_key(&key), lua::to_lua(&nested, 2, level))
}

fn lua_key(k: &str) -> String {
    let mut cs = k.chars();
    match cs.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {
            if cs.all(|c| c.is_ascii_alphanumeric() || c == '_') {
                k.to_owned()
            } else {
                format!("[{}]", lua::quote(k))
            }
        }
        _ => format!("[{}]", lua::quote(k)),
    }
}

fn apply(base: &str, mut edits: Vec<Edit>) -> String {
    if edits.is_empty() {
        return base.to_owned();
    }
    edits.sort_by_key(|e| (e.start, std::cmp::Reverse(e.end - e.start)));
    let mut kept: Vec<Edit> = Vec::new();
    for e in edits {
        if let Some(last) = kept.last() {
            if e.start < last.end {
                continue;
            }
        }
        kept.push(e);
    }
    let mut out = base.to_owned();
    for e in kept.iter().rev() {
        out.replace_range(e.start..e.end, &e.text);
    }
    out
}

pub fn render_new(changes: &HashMap<Vec<String>, Option<Value>>) -> String {
    let mut tree: HashMap<String, Value> = HashMap::new();
    for (path, value) in changes {
        if let Some(v) = value {
            insert_into_tree(&mut tree, path, v.clone());
        }
    }
    format!(
        "-- VectorTerm 配置文件\n-- 由「VectorTerm 配置管理器」生成。\nreturn {}\n",
        lua::to_lua(&Value::Table(lua::Table::Map(tree)), 2, 0)
    )
}

fn insert_into_tree(tree: &mut HashMap<String, Value>, path: &[String], value: Value) {
    if path.len() == 1 {
        tree.insert(path[0].clone(), value);
        return;
    }
    let first = path[0].clone();
    let entry = tree
        .entry(first)
        .or_insert_with(|| Value::Table(lua::Table::Map(HashMap::new())));
    if let Value::Table(lua::Table::Map(m)) = entry {
        insert_into_tree(m, &path[1..], value);
    }
}

// ---------------------------------------------------------------------
// 一个文件的会话状态
// ---------------------------------------------------------------------

pub struct ConfigFile {
    pub path: PathBuf,
    saved_text: String,
    changes: HashMap<Vec<String>, Option<Value>>,
    cache_text: Option<String>,
    doc: Document,
    pub error: Option<String>,
}

/// 读取结果状态
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FieldStatus {
    /// 没设置(用默认值)
    Default,
    /// 已显式设置
    Set,
    /// 原样保留的表达式
    Expr,
}

impl ConfigFile {
    pub fn open(path: PathBuf) -> Self {
        let saved = if path.exists() {
            read_text(&path)
        } else {
            empty_config()
        };
        let mut f = ConfigFile {
            path,
            saved_text: saved,
            changes: HashMap::new(),
            cache_text: None,
            doc: Document {
                text: String::new(),
                root: None,
            },
            error: None,
        };
        f.reparse();
        f
    }

    /// 从一段文本(而不是磁盘文件)构造会话。用于"主配置还不存在,先用内置
    /// 默认配置当内容、但保存时落盘到用户目录"这类场景。`path` 会被调用方
    /// 另行改写,决定最终写回哪里。
    pub fn from_text(text: String) -> Self {
        let mut f = ConfigFile {
            path: PathBuf::new(),
            saved_text: text,
            changes: HashMap::new(),
            cache_text: None,
            doc: Document {
                text: String::new(),
                root: None,
            },
            error: None,
        };
        f.reparse();
        f
    }

    fn reparse(&mut self) {
        let doc = lua::parse(&self.saved_text);
        self.doc = Document {
            text: self.saved_text.clone(),
            root: doc.root,
        };
        // 核心防护:解析不出根表 = 我们不认识这个文件。此时**绝不能**让
        // `render` 走 `render_new` 从零重建 —— 那会把主题文件 / 手写配置
        // 整份覆盖掉(历史上 `--[====[ ]====]` 长注释就触发过这个事故)。
        // 记下错误,`preview` 保留原文,`save` 直接拒绝。
        self.error = if self.doc.root.is_none() {
            Some("无法解析为 Lua 配置表(可能是长注释 / 语法超出解析器支持)。已禁用保存以免覆盖原文件,请用文本编辑器修改。".to_owned())
        } else {
            None
        };
    }

    pub fn set_text(&mut self, text: String) {
        self.saved_text = text;
        self.changes.clear();
        self.cache_text = None;
        self.reparse();
    }

    /// 读取某个路径的值和状态。
    pub fn get(&self, path: &[&str]) -> (Option<Value>, FieldStatus) {
        let key: Vec<String> = path.iter().map(|s| s.to_string()).collect();
        if let Some(v) = self.changes.get(&key) {
            return match v {
                None => (None, FieldStatus::Default),
                Some(v) => (Some(v.clone()), FieldStatus::Set),
            };
        }
        if self.doc.root.is_none() {
            return (None, FieldStatus::Default);
        }
        match self.doc.get(path) {
            None => (None, FieldStatus::Default),
            Some(Value::Expr(e)) => (Some(Value::Expr(e.clone())), FieldStatus::Expr),
            Some(v) => (Some(v.clone()), FieldStatus::Set),
        }
    }

    pub fn set(&mut self, path: &[&str], value: Option<Value>) {
        let key: Vec<String> = path.iter().map(|s| s.to_string()).collect();
        if value.is_none() && self.doc.find(path).is_none() {
            self.changes.remove(&key);
            self.cache_text = None;
            return;
        }
        self.changes.insert(key, value);
        self.cache_text = None;
    }

    #[allow(dead_code)]
    pub fn revert(&mut self, path: &[&str]) {
        let key: Vec<String> = path.iter().map(|s| s.to_string()).collect();
        self.changes.remove(&key);
        self.cache_text = None;
    }

    pub fn dirty(&self) -> bool {
        !self.changes.is_empty()
    }

    pub fn has_change(&self, path: &[String]) -> bool {
        self.changes.contains_key(path)
    }

    pub fn change_count(&self) -> usize {
        self.changes.values().filter(|v| v.is_some()).count()
    }

    pub fn preview(&mut self) -> String {
        if self.cache_text.is_none() {
            // 解析不出根表时原样返回,不重建(重建必然丢内容)。
            let t = if self.doc.root.is_none() {
                self.saved_text.clone()
            } else {
                render(&self.saved_text, &self.changes)
            };
            self.cache_text = Some(t);
        }
        self.cache_text.clone().unwrap()
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        // 不认识的文件一律不写:宁可保存失败,也不能把用户文件覆盖成空配置。
        if self.doc.root.is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "该文件无法解析,已拒绝保存(避免覆盖原内容)",
            ));
        }
        let text = self.preview();
        // 存之前真验一遍:写出去的文本必须还能被解析成配置表。以前这行
        // 只是调用 `lua::parse` 而丢掉结果,等于没验 —— 一旦拼接出的文本
        // 语法坏了,就会把一份加载不了的配置落盘。
        if lua::parse(&text).root.is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "生成的配置未通过语法校验,已放弃写入(磁盘上的原文件没有被修改)",
            ));
        }
        write_text(&self.path, &text)?;
        self.saved_text = text.replace("\r\n", "\n");
        self.changes.clear();
        self.cache_text = None;
        self.reparse();
        Ok(())
    }

    pub fn reload(&mut self) {
        self.saved_text = if self.path.exists() {
            read_text(&self.path)
        } else {
            empty_config()
        };
        self.changes.clear();
        self.cache_text = None;
        self.reparse();
    }
}

// ---------------------------------------------------------------------
// 扫描磁盘上的配置文件
// ---------------------------------------------------------------------

/// 用户配置目录 `~/.config/VectorTerm/`。
///
/// 与终端本体同一套逻辑:若新目录还不存在而更名前的旧目录在,先整块搬过来再
/// 返回新路径;搬不动就继续用旧目录,不让配置管理器看到一份空配置。
pub fn user_config_dir() -> PathBuf {
    let profile = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".to_owned());
    let base = PathBuf::from(profile).join(".config");
    let new_dir = base.join("VectorTerm");
    let old_dir = base.join("wintty");
    if !new_dir.exists() && old_dir.is_dir() {
        if std::fs::rename(&old_dir, &new_dir).is_err() && !new_dir.exists() {
            // 搬不动且新目录也没出现(不是并发启动时别人搬完的情况)→ 用旧目录
            return old_dir;
        }
    }
    new_dir
}

pub struct FileItem {
    pub path: PathBuf,
    pub group: &'static str,
    pub exists: bool,
}

pub fn discover(project_dir: Option<&Path>) -> Vec<FileItem> {
    let mut out: Vec<FileItem> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    fn add(
        out: &mut Vec<FileItem>,
        seen: &mut std::collections::HashSet<String>,
        path: PathBuf,
        group: &'static str,
    ) {
        let key = path.to_string_lossy().to_lowercase();
        if seen.contains(&key) {
            return;
        }
        seen.insert(key);
        let exists = path.exists();
        out.push(FileItem {
            path,
            group,
            exists,
        });
    }

    if let Some(dir) = project_dir {
        if let Ok(rd) = std::fs::read_dir(dir) {
            let mut files: Vec<PathBuf> = rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().map(|e| e == "lua").unwrap_or(false))
                .collect();
            files.sort();
            for p in files {
                add(&mut out, &mut seen, p, "程序目录");
            }
        }
    }

    let ucd = user_config_dir();
    if ucd.exists() {
        if let Ok(rd) = std::fs::read_dir(&ucd) {
            let mut files: Vec<PathBuf> = rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().map(|e| e == "lua").unwrap_or(false))
                .collect();
            files.sort();
            for p in files {
                add(&mut out, &mut seen, p, "用户目录");
            }
        }
    }

    // 键位文件单独一层:~/.config/VectorTerm/keys/(不能和当前配置挤在 current/)
    let kd = keys_dir();
    if kd.exists() {
        if let Ok(rd) = std::fs::read_dir(&kd) {
            let mut files: Vec<PathBuf> = rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().map(|e| e == "lua").unwrap_or(false))
                .collect();
            files.sort();
            for p in files {
                add(&mut out, &mut seen, p, "键位文件");
            }
        }
    }

    if let Some(dir) = project_dir {
        let example = dir.parent().map(|p| p.join("config.example.lua"));
        if let Some(e) = example {
            if e.exists() {
                add(&mut out, &mut seen, e, "示例");
            }
        }
    }

    if out.is_empty() {
        if let Some(dir) = project_dir {
            add(&mut out, &mut seen, dir.join("config.lua"), "程序目录");
        }
    }
    out
}

// ---------------------------------------------------------------------
// 「主配置 → 引用 Lua 方案」的读写辅助
// ---------------------------------------------------------------------

// ---------------------------------------------------------------------
// 「主配置 → 引用外部键位 Lua 文件」的读写辅助
// ---------------------------------------------------------------------

/// 读取一段配置文本里的 `keys_file`(外部键位文件引用);没有 / 不是字符串 /
/// 是空串都返回 None。这与终端 `script.rs` 的 `keys_file` 解析对应。
pub fn read_keys_file(text: &str) -> Option<String> {
    let doc = lua::parse(text);
    match doc.get(&["keys_file"]) {
        Some(Value::Str(s)) if !s.trim().is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// 在配置文本里设定 / 清除 `keys_file`,返回改写后的新文本(保留注释与格式)。
///
/// - `ref_str = Some("vim.lua")` → 写 `keys_file = "vim.lua"`,主配置会去加载
///   这份外部键位文件,只覆盖它列出的组合键,其余退回内置默认键位;
/// - `ref_str = None` → 清掉 `keys_file`,回到只认主配置内联 `keys`(或纯默认)。
pub fn set_keys_file(text: &str, ref_str: Option<&str>) -> String {
    let mut cf = ConfigFile::from_text(text.to_owned());
    match ref_str {
        Some(r) => cf.set(&["keys_file"], Some(Value::Str(r.to_owned()))),
        None => cf.set(&["keys_file"], None),
    }
    cf.preview()
}

/// 计算「把键位文件 `keymap` 接到配置里」要写的引用字符串。
///
/// - 键位文件在 `~/.config/VectorTerm/keys/` 里 → 只写裸文件名(`keys.lua`),
///   终端会优先在配置目录找、再到 keys/ 目录找,配置被复制到别处也不会失效;
/// - 与配置同目录 → 也只写文件名;
/// - 否则写相对路径;相对化失败(不同盘符等)就绝对路径兜底。
pub fn keys_ref_for(main_config: &Path, keymap: &Path) -> String {
    if keymap.parent() == Some(keys_dir().as_path()) {
        if let Some(name) = keymap.file_name() {
            return name.to_string_lossy().to_string();
        }
    }
    let base = main_config.parent().unwrap_or_else(|| Path::new("."));
    // 同目录:只写文件名(bare 名字,终端会自动补 .lua)。
    if keymap.parent() == Some(base) {
        return keymap
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| keymap.display().to_string());
    }
    // 否则写相对路径;相对化失败(不同盘符等)就绝对路径兜底。
    // 统一用正斜杠,Windows 与类 Unix 都能认,也避免把反斜杠写进 Lua 字符串时
    // 被当成转义(终端 resolve_keys_ref 也同时接受两种分隔符)。
    relative_path(base, keymap)
        .map(|p| p.to_string_lossy().to_string().replace('\\', "/"))
        .unwrap_or_else(|| keymap.display().to_string())
}

/// 极简相对路径计算(标准库没有现成的):从 `base` 目录到 `target` 的相对路径。
/// 没有公共前缀(不同盘符 / 根)时返回 None。
fn relative_path(base: &Path, target: &Path) -> Option<PathBuf> {
    use std::path::Component;
    let b: Vec<Component> = base.components().collect();
    let t: Vec<Component> = target.components().collect();
    let mut i = 0;
    while i < b.len() && i < t.len() && b[i] == t[i] {
        i += 1;
    }
    if i == 0 {
        return None;
    }
    let mut out = PathBuf::new();
    for _ in i..b.len() {
        out.push("..");
    }
    for c in &t[i..] {
        out.push(c.as_os_str());
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

/// 把主配置里写的引用字符串还原成真实文件路径(与终端 `resolve_keys_ref` 一致)。
///
/// - 绝对路径:原样;
/// - 含 `/` `\` `..` 或以 `.lua` 结尾:相对主配置目录;
/// - 其它(裸名字):补 `.lua` 后缀,当主配置目录下的同名文件。
pub fn resolve_keys_ref_str(main_config: &Path, ref_str: &str) -> Option<PathBuf> {
    let s = ref_str.trim();
    if s.is_empty() {
        return None;
    }
    let base = main_config.parent().unwrap_or_else(|| Path::new("."));
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
    Some(base.join(leaf))
}

/// 从一段配置文本里提取 `keys` 表,返回 `组合键 → 动作` 列表(已按组合键排序)。
///
/// 用于配置管理器的「键位映射」面板:展示当前生效(已应用未保存改动)的键位,
/// 以及支持删除 / 新增。动作可能是字符串(`"reload"`)或 Lua 函数(`"function"`)。
///
/// 直接遍历 `keys` 节点下的条目(而不是依赖 `get` 把整张表还原成 `Value`),
/// 这样 `["ctrl+shift+k"]` 这类方括号字符串键也能正确取回。
pub fn keys_entries(text: &str) -> Vec<(String, String)> {
    let doc = lua::parse(text);
    let mut out: Vec<(String, String)> = Vec::new();
    if let Some(entry) = doc.find(&["keys"]) {
        if entry.node.kind == lua::NodeKind::Table {
            for e in &entry.node.entries {
                // 组合键:标识符(`keys`)或方括号字符串(`["ctrl+shift+k"]`)
                let combo = match &e.key {
                    Some(lua::Key::Ident(s)) => s.clone(),
                    Some(lua::Key::Computed(s)) => strip_quotes(s),
                    None => continue,
                };
                if combo.is_empty() {
                    continue;
                }
                // 动作:字符串或保留原样表达式(Lua 函数等)
                let action = match &e.node.value {
                    Some(Value::Str(s)) => s.clone(),
                    Some(Value::Expr(s)) => s.clone(),
                    _ => continue,
                };
                out.push((combo, action));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// 去掉字符串两端的引号(单 / 双),用于还原 `["ctrl+shift+k"]` 里的键名。
fn strip_quotes(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2 {
        let first = t.chars().next().unwrap();
        let last = t.chars().last().unwrap();
        if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
            return t[first.len_utf8()..t.len() - last.len_utf8()].to_owned();
        }
    }
    t.to_owned()
}

/// 新建键位文件时写入的模板:只放一个空的 `keys` 表,外加一份动作名与默认键位
/// 速查注释,降低手写门槛。
pub fn keymap_template(name: &str) -> String {
    format!(
        "-- VectorTerm 键位方案:{name}\n\
         -- 只写你想覆盖 / 新增的组合键;没写的组合键一律退回终端内置默认键位。\n\
         -- 组合键写法(小写、修饰键顺序无关):\n\
         --   [\"ctrl+shift+k\"] = \"reload\"        -- 内置动作(见下方清单)\n\
         --   [\"ctrl+alt+t\"] = function() VectorTerm.notify(\"hi\") end  -- 或写 Lua 回调\n\
         --   [\"ctrl+shift+y\"] = \"send:ls\\r\"      -- 把一段文本直接发进终端\n\
         --\n\
         -- 可用的内置动作(reload / quit / copy / paste / new_tab / close_tab /\n\
         -- next_tab / prev_tab / files_gui),其余交给 send: 与 Lua。\n\
         -- 终端内置默认键位见 docs/keyboard-shortcuts.md(分屏、焦点、标签、搜索等\n\
         -- 硬快捷键不在这个表里,但可以用同名组合键在这里覆盖掉)。\n\
         return {{\n  keys = {{\n    -- 例如:\n    -- [\"ctrl+shift+k\"] = \"reload\",\n  }},\n}}\n"
    )
}

// ---------------------------------------------------------------------
// 「当前配置」:current/ 目录里只放一份 lua
// ---------------------------------------------------------------------

/// 当前生效配置所在目录:`~/.config/VectorTerm/current/`。
///
/// 用户在界面里「设为当前配置」时,选中的 lua 会被**复制**到这里,并把目录里
/// 其它 lua 清掉 —— 终端只认这个目录里唯一的一份,所以配置名随意、也不会
/// 再出现"改了 A 却加载了 B"的层级叠加问题。
pub fn active_config_dir() -> PathBuf {
    user_config_dir().join("current")
}

/// 键位文件目录:`~/.config/VectorTerm/keys/`。
///
/// 键位 lua 不能放在 `current/`(那会被当成"多份配置"报错),所以单独一层;
/// 配置里的 `keys_file = "keys.lua"` 会自动到这个目录里找。
pub fn keys_dir() -> PathBuf {
    user_config_dir().join("keys")
}

/// `current/` 的扫描结果。
#[derive(Debug, Default, Clone)]
pub struct ActiveConfig {
    /// 目录里唯一的那份 lua。`None` = 目录为空 / 不存在(用程序默认配置)
    pub path: Option<PathBuf>,
    /// 目录里有多个 lua 时的错误说明(此时终端会退回程序默认配置)
    pub error: Option<String>,
}

/// 扫描 `current/`,规则与终端 `config::pick_active` 完全一致:
/// 0 份 → None;1 份 → 用它;多份 → error。
pub fn active_config() -> ActiveConfig {
    pick_active_in(&active_config_dir())
}

/// 扫描指定目录(抽出来便于单测):0 份 → None;1 份 → 用它;多份 → error。
pub fn pick_active_in(dir: &Path) -> ActiveConfig {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return ActiveConfig::default();
    };
    let mut files: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .map(|e| e.eq_ignore_ascii_case("lua"))
                    .unwrap_or(false)
        })
        .collect();
    files.sort();
    match files.len() {
        0 => ActiveConfig::default(),
        1 => ActiveConfig {
            path: files.pop(),
            error: None,
        },
        n => ActiveConfig {
            path: None,
            error: Some(format!(
                "当前配置目录下有 {n} 个 .lua,只能有一份 —— 终端会退回程序默认配置。\
                 请删掉多余的,或重新「设为当前配置」。"
            )),
        },
    }
}

/// 把 `src` 设为当前配置:复制到 `current/`,并清掉目录里其它 lua。
///
/// 返回复制后的路径。`src` 已经是当前配置时是幂等的(只做清理)。
pub fn set_active_config(src: &Path) -> Result<PathBuf, String> {
    set_active_in(&active_config_dir(), src)
}

/// [`set_active_config`] 的实现,目录可指定(便于单测)。
pub fn set_active_in(dir: &Path, src: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("创建目录 {} 失败:{e}", dir.display()))?;
    let name = src
        .file_name()
        .ok_or_else(|| format!("{} 不是有效的文件名", src.display()))?
        .to_owned();
    let dest = dir.join(&name);

    // 先清掉其它 lua(含同名旧版本),保证目录里只剩这一份。
    if let Ok(rd) = std::fs::read_dir(dir) {
        for p in rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .map(|e| e.eq_ignore_ascii_case("lua"))
                        .unwrap_or(false)
            })
        {
            if p != dest {
                std::fs::remove_file(&p)
                    .map_err(|e| format!("清理旧配置 {} 失败:{e}", p.display()))?;
            }
        }
    }

    if src != dest {
        std::fs::copy(src, &dest)
            .map_err(|e| format!("复制 {} → {} 失败:{e}", src.display(), dest.display()))?;
    }
    Ok(dest)
}

/// 清空当前配置目录(终端回到程序自带默认配置)。
pub fn clear_active_config() -> Result<(), String> {
    let dir = active_config_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Ok(());
    };
    for p in rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
    {
        std::fs::remove_file(&p).map_err(|e| format!("删除 {} 失败:{e}", p.display()))?;
    }
    Ok(())
}

/// 判断一个路径是不是键位文件(在 `keys/` 目录里,或命名像 keys.lua / keys1.lua)。
pub fn is_keymap_path(path: &Path) -> bool {
    if path.parent() == Some(keys_dir().as_path()) {
        return true;
    }
    path.file_name()
        .map(|n| is_keymap_filename(&n.to_string_lossy()))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------
// 键位文件命名
// ---------------------------------------------------------------------

pub fn is_keymap_filename(name: &str) -> bool {
    let stem = name
        .strip_suffix(".lua")
        .or_else(|| name.strip_suffix(".LUA"))
        .unwrap_or(name);
    let low = stem.to_ascii_lowercase();
    if !low.starts_with("keys") {
        return false;
    }
    let rest = &low[4..];
    rest.is_empty() || rest.bytes().all(|b| b.is_ascii_digit())
}

/// 在 `dir` 里挑下一个可用的键位文件名:`keys.lua` → `keys1.lua` → `keys2.lua` …
/// 全占满时退化为带进程号的名字(几乎不会发生)。
pub fn next_keymap_filename(dir: &Path) -> String {
    if !dir.join("keys.lua").exists() {
        return "keys.lua".to_owned();
    }
    let mut n = 1u32;
    loop {
        let name = format!("keys{n}.lua");
        if !dir.join(&name).exists() {
            return name;
        }
        n += 1;
        if n > 9999 {
            return format!("keys{}-{}.lua", n, std::process::id());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lua::Value;

    #[test]
    fn replace_preserves_comments() {
        let src = "-- 顶部注释\nreturn {\n  -- 这是 shell 的说明\n  shell = \"pwsh.exe\",   -- 行尾注释\n  size = 18,\n}";
        let mut changes = HashMap::new();
        changes.insert(
            vec!["shell".to_string()],
            Some(Value::Str("powershell.exe".into())),
        );
        let out = render(src, &changes);
        assert!(out.contains("-- 这是 shell 的说明"), "注释应保留");
        assert!(out.contains("powershell.exe"));
        assert!(!out.contains("pwsh.exe"));
        assert!(out.contains("-- 行尾注释"), "行尾注释应保留");
    }

    #[test]
    fn delete_removes_line_not_group_comment() {
        let src = "-- 分组注释(应保留)\nreturn {\n  gpu_power = \"low\",\n  other = 1,\n}";
        let mut changes = HashMap::new();
        changes.insert(vec!["gpu_power".to_string()], None);
        let out = render(src, &changes);
        assert!(!out.contains("gpu_power"));
        assert!(out.contains("-- 分组注释(应保留)"));
    }

    #[test]
    fn insert_new_key() {
        let src = "return {\n  shell = \"pwsh.exe\",\n}";
        let mut changes = HashMap::new();
        changes.insert(vec!["size".to_string()], Some(Value::Int(20)));
        let out = render(src, &changes);
        assert!(out.contains("size = 20"));
        assert!(out.contains("shell = \"pwsh.exe\""));
    }

    /// 同一个新表下同时改多个键(分割线这一组全是新键)不能写出重复的父键。
    #[test]
    fn insert_multiple_nested_keys_merges_parent() {
        let src = "return {\n  shell = \"pwsh.exe\",\n}";
        let mut changes = HashMap::new();
        changes.insert(
            vec!["split_divider".into(), "thickness".into()],
            Some(Value::Int(2)),
        );
        changes.insert(
            vec!["split_divider".into(), "inset_start".into()],
            Some(Value::Int(12)),
        );
        changes.insert(
            vec!["split_divider".into(), "gradient".into(), "colors".into()],
            Some(Value::Table(crate::lua::Table::Array(vec![
                Value::Str("#ff0000".into()),
                Value::Str("#00ff00".into()),
            ]))),
        );
        changes.insert(
            vec![
                "split_divider".into(),
                "gradient".into(),
                "orientation".into(),
            ],
            Some(Value::Str("Vertical".into())),
        );
        let out = render(src, &changes);
        assert_eq!(
            out.matches("split_divider =").count(),
            1,
            "父表只能生成一次:\n{out}"
        );
        assert_eq!(out.matches("gradient =").count(), 1, ":\n{out}");
        assert!(out.contains("thickness = 2"));
        assert!(out.contains("inset_start = 12"));

        // 写回结果必须能被自己的解析器读回来
        let doc = crate::lua::parse(&out);
        assert!(matches!(
            doc.get(&["split_divider", "thickness"]),
            Some(Value::Int(2))
        ));
        assert!(matches!(
            doc.get(&["split_divider", "gradient", "orientation"]),
            Some(Value::Str(s)) if s == "Vertical"
        ));
        match doc.get(&["split_divider", "gradient", "colors"]) {
            Some(Value::Table(crate::lua::Table::Array(a))) => assert_eq!(a.len(), 2),
            other => panic!("渐变颜色没写对: {other:?}\n{out}"),
        }
    }

    /// 往「单行表」里追加键时必须补逗号。
    /// 事故现场:先保存 `status_bar = { font_scale = 1 },`,再保存
    /// `item_spacing`,旧实现直接插在 `1` 后面 —— 写出
    /// `{ font_scale = 1\n  item_spacing = 5, }`,整份配置语法错误、加载失败。
    #[test]
    fn insert_into_single_line_table_keeps_comma() {
        let src = "return {\n  padding = 5,\n  status_bar = { font_scale = 1 },\n}";
        let mut changes = HashMap::new();
        changes.insert(
            vec!["status_bar".into(), "item_spacing".into()],
            Some(Value::Int(5)),
        );
        let out = render(src, &changes);
        assert!(out.contains("font_scale = 1,"), "缺逗号:\n{out}");
        let doc = crate::lua::parse(&out);
        assert!(doc.root.is_some(), "写出的配置必须能解析:\n{out}");
        assert!(doc.get(&["status_bar", "font_scale"]).is_some(), "原键丢了:\n{out}");
        assert!(matches!(
            doc.get(&["status_bar", "item_spacing"]),
            Some(Value::Int(5))
        ));
    }

    /// 在一个**还没有** `status_bar` 表的配置里改「空格宽度」(要新建这张表),
    /// 必须产出能解析的 Lua,原键也不能丢。
    #[test]
    fn insert_space_width_creates_status_table() {
        let src = "return {\n  padding = 5,\n}";
        let mut changes = HashMap::new();
        changes.insert(
            vec!["status_bar".into(), "space_width".into()],
            Some(Value::Num(0.5)),
        );
        let out = render(src, &changes);
        let doc = crate::lua::parse(&out);
        assert!(doc.root.is_some(), "写出的配置必须能解析:\n{out}");
        assert!(
            matches!(
                doc.get(&["status_bar", "space_width"]),
                Some(Value::Num(v)) if (*v - 0.5).abs() < 1e-9
            ),
            "值没写进去:\n{out}"
        );
        assert!(
            matches!(doc.get(&["padding"]), Some(Value::Int(5))),
            "原键丢了:\n{out}"
        );
    }

    /// 真实配置文件上的两次连续保存(先 font_scale、再 item_spacing)，
    /// 必须始终产出可解析的 Lua。手动跑:
    /// `cargo test -p vectorterm-settings real_config_two_step -- --ignored`
    #[test]
    #[ignore]
    fn real_config_two_step_saves_stay_valid() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("config")
            .join("deepseek.lua");
        let Ok(src) = std::fs::read_to_string(&p) else {
            return;
        };
        let mut c = ConfigFile::from_text(src);
        c.set(&["status_bar", "font_scale"], Some(Value::Num(1.0)));
        let first = c.preview();
        assert!(crate::lua::parse(&first).root.is_some(), "第一次保存就坏了");
        c.set_text(first);
        c.set(&["status_bar", "item_spacing"], Some(Value::Int(5)));
        let second = c.preview();
        let doc = crate::lua::parse(&second);
        assert!(doc.root.is_some(), "第二次保存坏了:\n{second}");
        assert!(doc.get(&["status_bar", "item_spacing"]).is_some());
        assert!(doc.get(&["window_width"]).is_some(), "原有键不能丢");
    }

    // ---- 键位文件引用 ----
    #[test]
    fn read_keys_file_picks_string() {
        assert_eq!(
            read_keys_file("return { keys_file = \"vim.lua\" }"),
            Some("vim.lua".into())
        );
        assert_eq!(read_keys_file("return { shell = \"pwsh.exe\" }"), None);
        assert_eq!(read_keys_file("return { keys_file = \"\" }"), None);
    }

    #[test]
    fn set_keys_file_inserts_and_clears() {
        let src = "return {\n  shell = \"pwsh.exe\",\n}";
        let out = set_keys_file(src, Some("vim.lua"));
        assert!(out.contains("keys_file = \"vim.lua\""));
        assert!(out.contains("shell = \"pwsh.exe\""), "原键保留");

        let cleared = set_keys_file(&out, None);
        assert!(!cleared.contains("keys_file ="), "清除后无 keys_file 行:\n{cleared}");
        assert!(cleared.contains("shell = \"pwsh.exe\""));

        let twice = set_keys_file(&out, Some("emacs.lua"));
        assert_eq!(twice.matches("keys_file =").count(), 1, "keys_file 只能有一条:\n{twice}");
        assert!(twice.contains("keys_file = \"emacs.lua\""));
    }

    #[test]
    fn set_keys_file_preserves_comments() {
        let src = "-- 顶部说明\nreturn {\n  -- shell 说明\n  shell = \"pwsh.exe\",\n}";
        let out = set_keys_file(src, Some("vim.lua"));
        assert!(out.contains("-- 顶部说明"));
        assert!(out.contains("-- shell 说明"));
    }

    #[test]
    fn resolve_keys_ref_matches_terminal_rules() {
        let cfg = Path::new("C:/Users/me/.config/VectorTerm/config.lua");
        // 裸名字 → 同目录补 .lua
        assert_eq!(
            resolve_keys_ref_str(cfg, "vim"),
            Some(PathBuf::from("C:/Users/me/.config/VectorTerm/vim.lua"))
        );
        // 带 .lua 后缀 → 同目录
        assert_eq!(
            resolve_keys_ref_str(cfg, "vim.lua"),
            Some(PathBuf::from("C:/Users/me/.config/VectorTerm/vim.lua"))
        );
        // 含分隔符 → 相对目录
        assert_eq!(
            resolve_keys_ref_str(cfg, "keymaps/vim.lua"),
            Some(PathBuf::from("C:/Users/me/.config/VectorTerm/keymaps/vim.lua"))
        );
        // 绝对路径原样
        assert_eq!(
            resolve_keys_ref_str(cfg, "D:/x/mykeys.lua"),
            Some(PathBuf::from("D:/x/mykeys.lua"))
        );
        assert_eq!(resolve_keys_ref_str(cfg, ""), None);
    }

    #[test]
    fn keys_ref_for_prefers_same_dir_filename() {
        let cfg = Path::new("C:/Users/me/.config/VectorTerm/config.lua");
        let km = Path::new("C:/Users/me/.config/VectorTerm/vim.lua");
        assert_eq!(keys_ref_for(cfg, km), "vim.lua");
        // 不同目录 → 相对路径
        let km2 = Path::new("C:/Users/me/.config/keymaps/vim.lua");
        assert_eq!(keys_ref_for(cfg, km2), "../keymaps/vim.lua");
    }

    #[test]
    fn keys_entries_extracts_combo_action() {
        let src = "return {\n  keys = {\n    [\"ctrl+shift+k\"] = \"reload\",\n    [\"ctrl+shift+y\"] = \"close_tab\",\n  },\n}";
        let mut got = keys_entries(src);
        got.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], ("ctrl+shift+k".to_string(), "reload".to_string()));
        assert_eq!(got[1], ("ctrl+shift+y".to_string(), "close_tab".to_string()));
        // 方括号字符串键 + 普通标识符键都能取到
        let src2 = "return { keys = { up = \"paste\", [\"ctrl+x\"] = \"copy\" } }";
        let got2 = keys_entries(src2);
        assert_eq!(got2.len(), 2);
        assert!(got2.iter().any(|(c, a)| c == "up" && a == "paste"));
        assert!(got2.iter().any(|(c, a)| c == "ctrl+x" && a == "copy"));
        assert!(keys_entries("return {}").is_empty());
    }

    #[test]
    fn keymap_template_is_valid_lua() {
        let t = keymap_template("vim");
        assert!(t.contains("keys ="));
        // 模板必须能被自己的解析器读回来
        let doc = crate::lua::parse(&t);
        assert!(doc.root.is_some(), "模板应是合法 Lua:\n{t}");
        assert!(matches!(doc.get(&["keys"]), Some(Value::Table(_))));
    }

    #[test]
    fn active_config_dir_is_a_subdir_of_user_config() {
        assert_eq!(active_config_dir(), user_config_dir().join("current"));
        assert_eq!(keys_dir(), user_config_dir().join("keys"));
    }

    #[test]
    fn keymap_filename_rules() {
        for ok in ["keys.lua", "keys1.lua", "keys12.lua", "Keys.lua", "KEYS3.LUA"] {
            assert!(is_keymap_filename(ok), "{ok} 应识别为键位文件");
        }
        for no in ["config.lua", "keysx.lua", "settings.lua", "mykeys.lua", "keys_1.lua"] {
            assert!(!is_keymap_filename(no), "{no} 不应识别为键位文件");
        }
    }

    #[test]
    fn next_keymap_filename_increments() {
        let dir = std::env::temp_dir().join(format!("vectorterm-keyname-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(next_keymap_filename(&dir), "keys.lua");
        std::fs::write(dir.join("keys.lua"), "").unwrap();
        assert_eq!(next_keymap_filename(&dir), "keys1.lua");
        std::fs::write(dir.join("keys1.lua"), "").unwrap();
        assert_eq!(next_keymap_filename(&dir), "keys2.lua");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 带 `--[====[ ]====]` 长注释头的主题文件,编辑单个字段后必须"定点改写":
    /// 原注释与其它键都要留着,绝不能被 `render_new` 整份重建。
    #[test]
    fn long_comment_theme_file_survives_single_edit() {
        let src = "--[====[\n  VectorTerm 内置主题:DeepSeek Dark\n]====]\n\
                   return {\n  -- 夜空蓝底\n  colors = {\n    background = \"#0B1220\",\n    foreground = \"#E8EDF9\",\n  },\n  font = { size = 12 },\n}\n";
        let mut cf = ConfigFile::from_text(src.to_owned());
        assert!(cf.error.is_none(), "长注释头应能解析: {:?}", cf.error);
        cf.set(&["font", "size"], Some(Value::Int(10)));
        let out = cf.preview();
        assert!(out.contains("VectorTerm 内置主题"), "文件头注释要保留:\n{out}");
        assert!(out.contains("-- 夜空蓝底"), "行内注释要保留:\n{out}");
        assert!(out.contains("#0B1220"), "其它配色不能丢:\n{out}");
        assert!(out.contains("size = 10"), "改动要生效:\n{out}");
    }

    /// 随包分发的内置主题必须都能被配置管理器解析 —— 否则一保存就会把它重建掉。
    ///
    /// 四套整套配置(deepseek / chatgpt / claude / gemini)结构相同,管理器
    /// 必须都能打开;而且改一项之后写出来的文本还得能再解析一遍。
    #[test]
    fn bundled_themes_are_parseable() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("config");
        for name in ["deepseek", "chatgpt", "claude", "gemini"] {
            let p = dir.join(format!("{name}.lua"));
            let text = std::fs::read_to_string(&p)
                .unwrap_or_else(|e| panic!("内置主题 {} 应存在:{e}", p.display()));
            let mut cf = ConfigFile::from_text(text);
            assert!(cf.error.is_none(), "{name}.lua 应可解析,实际:{:?}", cf.error);
            assert!(
                cf.preview().contains("colors"),
                "{name}.lua 应能读到配色表"
            );
            // 改一项状态栏参数,预览必须仍然合法(写回后终端才读得动)
            cf.set(&["status_bar", "font_scale"], Some(Value::Num(0.95)));
            let out = cf.preview();
            assert!(
                crate::lua::parse(&out).root.is_some(),
                "{name}.lua 改完写出的文本必须还能解析:\n{out}"
            );
            assert!(out.contains("VectorTerm 内置主题"), "{name}.lua 文件头注释要保留");
        }
    }

    /// 解析不出根表时**必须拒绝保存** —— 以前会走 render_new 把整份文件覆盖成
    /// 空配置,主题文件就是这么被清掉的。
    #[test]
    fn unparseable_file_refuses_save() {
        let dir = std::env::temp_dir().join(format!("vectorterm-badsave-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("broken.lua");
        let original = "return { a = 1,\n"; // 括号没闭合
        std::fs::write(&path, original).unwrap();

        let mut cf = ConfigFile::open(path.clone());
        assert!(cf.error.is_some(), "解析失败应记错误");
        cf.set(&["font", "size"], Some(Value::Int(10)));
        assert!(cf.save().is_err(), "解析失败时保存必须失败");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            original,
            "磁盘内容不能被改动"
        );
        assert_eq!(cf.preview(), original, "预览要原样返回,不重建");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 「设为当前配置」:复制进 current/ 后目录里只剩这一份;换一份时旧的被清掉。
    #[test]
    fn set_active_config_keeps_exactly_one_lua() {
        let root = std::env::temp_dir().join(format!("vectorterm-active-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cur = root.join("current");
        let src_dir = root.join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&cur).unwrap();

        let a = src_dir.join("a.lua");
        let b = src_dir.join("b.lua");
        std::fs::write(&a, "return { shell = 'a' }").unwrap();
        std::fs::write(&b, "return { shell = 'b' }").unwrap();

        let dest = set_active_in(&cur, &a).unwrap();
        assert_eq!(dest, cur.join("a.lua"));
        assert_eq!(pick_active_in(&cur).path.as_deref(), Some(dest.as_path()));
        assert_eq!(
            std::fs::read_to_string(&dest).unwrap(),
            "return { shell = 'a' }"
        );

        // 换一份:旧的那份必须被清掉,否则终端会报"多个 lua"
        let dest_b = set_active_in(&cur, &b).unwrap();
        assert_eq!(dest_b, cur.join("b.lua"));
        assert!(!cur.join("a.lua").exists(), "旧配置要清掉");
        assert_eq!(pick_active_in(&cur).path.as_deref(), Some(dest_b.as_path()));

        // 重复设同一份是幂等的
        let again = set_active_in(&cur, &b).unwrap();
        assert_eq!(again, dest_b);
        assert!(pick_active_in(&cur).error.is_none());

        // 目录里出现第二份 lua → 冲突(模拟用户手动拷进去)
        std::fs::write(cur.join("extra.lua"), "return {}").unwrap();
        let got = pick_active_in(&cur);
        assert!(got.path.is_none());
        assert!(got.error.unwrap().contains("只能有一份"));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 键位文件放在 keys/ 里时,引用只写裸文件名(配置被复制到 current/ 也不失效)。
    #[test]
    fn keys_ref_for_uses_bare_name_for_keys_dir() {
        let km = keys_dir().join("keys.lua");
        let cfg = active_config_dir().join("mine.lua");
        assert_eq!(keys_ref_for(&cfg, &km), "keys.lua");
    }
}
