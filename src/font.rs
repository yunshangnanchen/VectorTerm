//! 系统字体定位与匹配(对标 wezterm 的 font locator)。
//!
//! wezterm 在 Windows 上通过 GDI 枚举字体族,再按 `family + weight + style`
//! 选具体字体;本模块用更轻的办法:扫描字体目录,只读取 sfnt 头 + `name` /
//! `OS/2` / `head` / `post` 这几张小表,得到一个"家族名 → 字体文件"的索引。
//!
//! 之所以不把整个字体文件读进来:`C:\Windows\Fonts` + 用户字体目录合计约
//! 800MB,全量解析会让启动明显变慢。这里每个字体只做 3~4 次小范围 seek。
//!
//! 支持:
//! - `.ttf` / `.otf` / `.ttc` / `.otc`(TTC 会展开成多个 face)
//! - 字体族名、全名、PostScript 名匹配(大小写/空格/连字符不敏感)
//! - 同一家族内按 weight + italic 挑 Regular / Bold / Italic / BoldItalic
//! - Nerd Font 识别(供 PUA 图标字形做优先回退)

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

const MAX_FACES_PER_FILE: u32 = 32;
const MAX_TABLES: usize = 512;
const MAX_NAME_TABLE: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct FaceInfo {
    pub path: PathBuf,
    /// TTC 里的 face 下标;普通字体为 0
    pub index: u32,
    /// 排版家族名(name ID 16),没有则用 name ID 1
    pub family: String,
    pub full_name: String,
    pub postscript: String,
    /// OS/2 usWeightClass(400 = Regular,700 = Bold)
    pub weight: u16,
    pub italic: bool,
    pub monospaced: bool,
}

/// 一个字体家族的四个常见样式槽位
#[derive(Clone, Debug)]
pub struct FamilyFaces {
    pub name: String,
    pub regular: FaceInfo,
    pub bold: Option<FaceInfo>,
    pub italic: Option<FaceInfo>,
    pub bold_italic: Option<FaceInfo>,
}

#[derive(Default)]
pub struct FontIndex {
    faces: Vec<FaceInfo>,
}

fn be16(b: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(off)?, *b.get(off + 1)?]))
}

fn be32(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *b.get(off)?,
        *b.get(off + 1)?,
        *b.get(off + 2)?,
        *b.get(off + 3)?,
    ]))
}

fn read_at(f: &mut File, off: u64, len: usize) -> Option<Vec<u8>> {
    if len == 0 {
        return Some(Vec::new());
    }
    let file_len = f.metadata().ok()?.len();
    if off.checked_add(len as u64)? > file_len {
        return None;
    }
    f.seek(SeekFrom::Start(off)).ok()?;
    let mut buf = vec![0u8; len];
    f.read_exact(&mut buf).ok()?;
    Some(buf)
}

/// 把字体名归一化后比较:大小写、空格、连字符、下划线都不敏感。
/// "MesloLGL Nerd Font Mono" == "meslolglnf"? 不,只去掉分隔符,不改写缩写。
pub fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Nerd Font 的字形几乎都在 Unicode 私用区(PUA):
/// - BMP: U+E000..=U+F8FF
/// - 15/16 平面: U+F0000..=U+FFFFD, U+100000..=U+10FFFD
pub fn is_private_use(c: char) -> bool {
    matches!(
        c as u32,
        0xE000..=0xF8FF | 0xF0000..=0xFFFFD | 0x100000..=0x10FFFD
    )
}

/// Windows 上 wezterm 的 GDI locator 会枚举到的两个目录
pub fn system_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let windir = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:/Windows"));
    dirs.push(windir.join("Fonts"));
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("Microsoft/Windows/Fonts"));
    }
    dirs
}

impl FontIndex {
    /// 扫描字体目录(含系统目录),建立索引。
    pub fn load(extra_dirs: &[PathBuf]) -> Self {
        let mut dirs = system_dirs();
        dirs.extend(extra_dirs.iter().cloned());

        let mut index = FontIndex::default();
        let mut seen: HashSet<(PathBuf, u32)> = HashSet::new();
        for dir in dirs {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if !matches!(ext.as_str(), "ttf" | "otf" | "ttc" | "otc") {
                    continue;
                }
                for face in parse_font_file(&path) {
                    if seen.insert((face.path.clone(), face.index)) {
                        index.faces.push(face);
                    }
                }
            }
        }
        index
    }

    /// 默认目录的索引在进程内只建一次(测试/热重载会反复构造 Renderer)。
    pub fn system_cached() -> Arc<FontIndex> {
        static CACHE: OnceLock<Arc<FontIndex>> = OnceLock::new();
        CACHE
            .get_or_init(|| Arc::new(FontIndex::load(&[])))
            .clone()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.faces.len()
    }

    /// 直接从字体文件解析(不需要索引)。
    /// TTC 里可能装了多个不同家族(msyh.ttc 同时有 YaHei / YaHei UI),
    /// 这里以第一个 face 的家族为准,只取同家族的 face。
    pub fn resolve_file(path: &Path) -> Option<FamilyFaces> {
        let faces = parse_font_file(path);
        let name = faces.first()?.family.clone();
        let same: Vec<FaceInfo> = faces
            .into_iter()
            .filter(|f| f.family.eq_ignore_ascii_case(&name))
            .collect();
        build_family(&same)
    }

    /// 字体文件里的第一个 face(用于 `bold = "C:/.../consolab.ttf"` 这种写法)
    pub fn face_from_file(path: &Path) -> Option<FaceInfo> {
        parse_font_file(path).into_iter().next()
    }

    /// 按名字找**单个** face(不展开成家族),用于样式覆盖
    pub fn face_from_spec(&self, spec: &str) -> Option<FaceInfo> {
        if let Some(face) = crate::builtin_fonts::resolve_face(spec) {
            return Some(face);
        }
        let norm = normalize(spec.trim());
        if norm.is_empty() {
            return None;
        }
        self.best_face(&norm).cloned()
    }

    fn best_face(&self, norm: &str) -> Option<&FaceInfo> {
        let mut best: Option<(&FaceInfo, i32)> = None;
        for face in &self.faces {
            let Some(score) = match_score(face, norm) else {
                continue;
            };
            // 同分时优先等宽字体、优先 Regular(避免选到 Bold 作为家族代表)
            let tie = (face.monospaced as i32) * 10
                - (face.italic as i32) * 4
                - (face.weight as i32 - 400).abs() / 50;
            let total = score * 100 + tie;
            if best.map(|(_, s)| total > s).unwrap_or(true) {
                best = Some((face, total));
            }
        }
        best.map(|(f, _)| f)
    }

    /// 按用户给的字符串找字体:可以是字体文件路径,也可以是家族名/全名。
    pub fn resolve(&self, spec: &str) -> Option<FamilyFaces> {
        let spec = spec.trim();
        if spec.is_empty() {
            return None;
        }

        // 1) 明确给了文件路径
        let as_path = Path::new(spec);
        if as_path.is_file() {
            let faces = parse_font_file(as_path);
            if let Some(family) = build_family(&faces) {
                return Some(family);
            }
        }

        // 2) 内嵌字体优先:没装 JetBrains Mono 的机器也用得上,而且保证
        //    拿到的是随程序发布的那一份,不会被系统里的同名版本顶掉
        if let Some(family) = crate::builtin_fonts::resolve(spec) {
            return Some(family);
        }

        // 3) 家族名/全名匹配
        let norm = normalize(spec);
        if norm.is_empty() {
            return None;
        }
        let hit = self.best_face(&norm)?;
        let family_name = hit.family.clone();
        let faces: Vec<FaceInfo> = self
            .faces
            .iter()
            .filter(|f| f.family.eq_ignore_ascii_case(&family_name))
            .cloned()
            .collect();
        build_family(&faces)
    }

    /// 找出所有 Nerd Font 家族,按"适合画图标"的程度排序。
    ///
    /// 优先 `Symbols Nerd Font`(纯图标,不含拉丁字母)与 `... Mono` 变体;
    /// `Propo` / `Proportional` 是比例宽度版本,图标容易对不齐单元格,排在最后。
    pub fn nerd_families(&self) -> Vec<String> {
        let mut scores: HashMap<String, i32> = HashMap::new();
        for face in &self.faces {
            let n = normalize(&face.family);
            if !n.contains("nerdfont") {
                continue;
            }
            let mut score = 0;
            if n.contains("symbols") {
                score += 40;
            }
            if n.contains("mono") {
                score += 25;
            }
            if n.contains("propo") || n.contains("proportional") {
                score -= 30;
            }
            if face.monospaced {
                score += 10;
            }
            if !face.italic && (face.weight as i32 - 400).abs() < 250 {
                score += 5;
            }
            scores
                .entry(face.family.clone())
                .and_modify(|s| *s = (*s).max(score))
                .or_insert(score);
        }
        let mut list: Vec<(String, i32)> = scores.into_iter().collect();
        list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        list.into_iter().map(|(name, _)| name).collect()
    }

}

fn match_score(face: &FaceInfo, norm: &str) -> Option<i32> {
    let family = normalize(&face.family);
    let full = normalize(&face.full_name);
    let ps = normalize(&face.postscript);
    let keys = [&family, &full, &ps];
    if keys.iter().any(|k| k.as_str() == norm) {
        return Some(1000);
    }
    // 用户写的是家族名,而某个 face 的全名更长(如 ".. Mono" vs ".. Mono Bold")
    if keys
        .iter()
        .any(|k| !k.is_empty() && (k.starts_with(norm) || norm.starts_with(k.as_str())))
    {
        return Some(650);
    }
    if keys.iter().any(|k| !norm.is_empty() && k.contains(norm)) {
        return Some(450);
    }
    None
}

/// 从同家族的多个 face 里挑出四个样式槽
pub(crate) fn build_family(faces: &[FaceInfo]) -> Option<FamilyFaces> {
    let first = faces.first()?;
    let name = first.family.clone();

    let regular = pick(faces, false, 400)?;
    Some(FamilyFaces {
        name,
        bold: pick(faces, false, 700),
        italic: pick(faces, true, 400),
        bold_italic: pick(faces, true, 700),
        regular,
    })
}

fn pick(faces: &[FaceInfo], italic: bool, target_weight: u16) -> Option<FaceInfo> {
    faces
        .iter()
        .filter(|f| f.italic == italic)
        .min_by_key(|f| (f.weight as i32 - target_weight as i32).abs())
        .cloned()
}

// ---------------------------------------------------------------------------
// 最小 sfnt 解析:只读 name / OS/2 / head / post
// ---------------------------------------------------------------------------

fn parse_font_file(path: &Path) -> Vec<FaceInfo> {
    let mut out = Vec::new();
    let Ok(mut f) = File::open(path) else {
        return out;
    };
    let Some(header) = read_at(&mut f, 0, 12) else {
        return out;
    };

    let mut offsets: Vec<(u32, u64)> = Vec::new();
    if &header[0..4] == b"ttcf" {
        let num = be32(&header, 8).unwrap_or(0);
        let n = num.min(MAX_FACES_PER_FILE) as usize;
        let Some(raw) = read_at(&mut f, 12, n * 4) else {
            return out;
        };
        for i in 0..n {
            let Some(off) = be32(&raw, i * 4) else {
                continue;
            };
            offsets.push((i as u32, off as u64));
        }
    } else {
        offsets.push((0, 0));
    }

    for (index, off) in offsets {
        if let Some(face) = parse_face(&mut f, path, index, off) {
            out.push(face);
        }
    }
    out
}

fn parse_face(f: &mut File, path: &Path, index: u32, off: u64) -> Option<FaceInfo> {
    let head12 = read_at(f, off, 12)?;
    let num_tables = be16(&head12, 4)? as usize;
    if num_tables == 0 || num_tables > MAX_TABLES {
        return None;
    }
    let recs = read_at(f, off + 12, num_tables * 16)?;
    let mut tables: HashMap<[u8; 4], (u64, u64)> = HashMap::new();
    for i in 0..num_tables {
        let r = &recs[i * 16..i * 16 + 16];
        let tag = [r[0], r[1], r[2], r[3]];
        let t_off = be32(r, 8)? as u64;
        let t_len = be32(r, 12)? as u64;
        tables.insert(tag, (t_off, t_len));
    }

    let names = tables
        .get(b"name")
        .and_then(|&(o, l)| parse_names(f, o, l))?;
    let family = names.family?;

    // OS/2: usWeightClass @4, fsSelection @62
    let (mut weight, mut italic) = (400u16, false);
    if let Some(&(o, l)) = tables.get(b"OS/2") {
        if l >= 64 {
            if let Some(b) = read_at(f, o, 64) {
                weight = be16(&b, 4).unwrap_or(400);
                let sel = be16(&b, 62).unwrap_or(0);
                italic |= sel & 0x01 != 0;
                if sel & 0x20 != 0 {
                    weight = 700;
                }
            }
        }
    }
    // head.macStyle @44: bit0 bold, bit1 italic
    if let Some(&(o, _)) = tables.get(b"head") {
        if let Some(b) = read_at(f, o, 46) {
            let style = be16(&b, 44).unwrap_or(0);
            italic |= style & 0x02 != 0;
            if style & 0x01 != 0 && weight < 600 {
                weight = 700;
            }
        }
    }
    // post.isFixedPitch @12
    let mut monospaced = false;
    if let Some(&(o, l)) = tables.get(b"post") {
        if l >= 16 {
            if let Some(b) = read_at(f, o, 16) {
                monospaced = be32(&b, 12).unwrap_or(0) != 0;
            }
        }
    }
    let n = normalize(&family);
    if n.contains("mono") || n.contains("consol") || n.contains("courier") {
        monospaced = true;
    }

    let subfamily = names.subfamily.unwrap_or_default();
    let full_name = names
        .full_name
        .unwrap_or_else(|| format!("{family} {subfamily}").trim().to_string());
    let postscript = names.postscript.unwrap_or_default();

    Some(FaceInfo {
        path: path.to_path_buf(),
        index,
        family,
        full_name,
        postscript,
        weight,
        italic,
        monospaced,
    })
}

struct Names {
    family: Option<String>,
    subfamily: Option<String>,
    full_name: Option<String>,
    postscript: Option<String>,
}

fn parse_names(f: &mut File, off: u64, len: u64) -> Option<Names> {
    if len < 6 || len > MAX_NAME_TABLE {
        return None;
    }
    let data = read_at(f, off, len as usize)?;
    let count = be16(&data, 2)? as usize;
    let string_off = be16(&data, 4)? as usize;
    if count == 0 || 6 + count * 12 > data.len() {
        return None;
    }

    // name_id -> (优先级, 文本);优先 Windows 英文,其次其它 Unicode
    let mut best: HashMap<u16, (i32, String)> = HashMap::new();
    for i in 0..count {
        let r = &data[6 + i * 12..6 + i * 12 + 12];
        let (Some(pid), Some(eid), Some(lid), Some(nid), Some(nlen), Some(noff)) = (
            be16(r, 0),
            be16(r, 2),
            be16(r, 4),
            be16(r, 6),
            be16(r, 8),
            be16(r, 10),
        ) else {
            continue;
        };
        if !matches!(nid, 1 | 2 | 4 | 6 | 16 | 17) {
            continue;
        }
        let start = string_off + noff as usize;
        let end = start + nlen as usize;
        let Some(raw) = data.get(start..end) else {
            continue;
        };
        let Some(text) = decode_name(pid, eid, raw) else {
            continue;
        };
        let text = text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        let prio = match (pid, lid) {
            (3, 0x409) => 0,
            (3, _) => 1,
            (0, _) => 2,
            (1, _) => 3,
            _ => continue,
        };
        best.entry(nid)
            .and_modify(|e| {
                if prio < e.0 {
                    *e = (prio, text.clone());
                }
            })
            .or_insert((prio, text));
    }

    let get = |id: u16| best.get(&id).map(|(_, s)| s.clone());
    Some(Names {
        family: get(16).or_else(|| get(1)),
        subfamily: get(17).or_else(|| get(2)),
        full_name: get(4),
        postscript: get(6),
    })
}

fn decode_name(pid: u16, eid: u16, raw: &[u8]) -> Option<String> {
    match (pid, eid) {
        // Unicode / Windows Unicode(BMP 或 full)
        (0, _) | (3, 0 | 1 | 10) => {
            if raw.len() % 2 != 0 {
                return None;
            }
            let units: Vec<u16> = raw
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16(&units).ok()
        }
        // Mac Roman:按 latin1 近似(字体家族名基本都是 ASCII)
        (1, 0) => Some(raw.iter().map(|&b| b as char).collect()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_ignores_case_and_separators() {
        assert_eq!(normalize("MesloLGL Nerd Font-Mono"), "meslolglnerdfontmono");
        assert_eq!(normalize("mesloLGLNerdFontMono"), "meslolglnerdfontmono");
    }

    #[test]
    fn detects_private_use_areas() {
        assert!(is_private_use('\u{e0b0}'));
        assert!(is_private_use('\u{f015}'));
        assert!(is_private_use('\u{f0001}'));
        assert!(!is_private_use('中'));
        assert!(!is_private_use('A'));
    }

    #[test]
    fn index_finds_installed_fonts() {
        let idx = FontIndex::load(&[]);
        if idx.len() == 0 {
            crate::wlog!("跳过:本机字体目录为空");
            return;
        }
        // Consolas 是 Windows 自带字体,必须能按家族名找到
        let family = idx.resolve("Consolas").expect("应能按家族名解析 Consolas");
        assert!(family.name.eq_ignore_ascii_case("Consolas"));
        assert!(family.bold.is_some(), "Consolas 应有 Bold");
        assert!(family.italic.is_some(), "Consolas 应有 Italic");
    }

    #[test]
    fn nerd_font_is_detected_when_installed() {
        let idx = FontIndex::load(&[]);
        let nerd = idx.nerd_families();
        crate::wlog!("nerd families: {nerd:?}");
        if nerd.is_empty() {
            crate::wlog!("跳过:本机未安装 Nerd Font");
            return;
        }
        // 排序:纯图标 / Mono 变体应排在 Propo 之前
        let first = &nerd[0];
        assert!(
            !normalize(first).contains("propo"),
            "Nerd Font 排序应优先 Mono/Symbols,实际第一名 {first}"
        );
    }
}
