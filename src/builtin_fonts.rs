//! 内嵌字体:与 wezterm 发行版内置的那几款完全一致。
//!
//! wezterm 把 JetBrains Mono、Nerd Font Symbols、Noto Color Emoji 一起打进
//! 安装包,所以没装任何字体也能正常显示等宽正文、Nerd 图标和彩色 emoji。
//! 这里照做:`include_bytes!` 把字体编进 VectorTerm.exe,启动时不再依赖
//! `C:\Windows\Fonts` 里有没有这些家族。
//!
//! 只有彩色 emoji 那份不进本模块维护的普通字体链:CBDT 位图字体没有 glyf
//! 轮廓,ab_glyph 读不到字形,由 `emoji` 模块按 CBLC/CBDT 直接取 PNG
//! (详见 `assets/fonts/README.md`)。
//!
//! 字体文件来自 wezterm 仓库 `assets/fonts/` 与 googlefonts/noto-emoji,
//! 许可证见同目录 `README.md`。
//!
//! ## 怎么接进现有的字体链
//!
//! `FaceInfo` 里存的是磁盘路径,加载时 `FontSlot::from_face` 用 `fs::read`。
//! 为了不动这个结构,内嵌字体用一个虚拟路径 `@builtin/<文件名>` 作标识:
//! `bytes()` 认这个前缀并从静态字节里取数据。`FontIndex::resolve` 在查系统
//! 字体**之前**先查本模块,所以 `family = "JetBrains Mono"` 命中的永远是
//! 程序自带的这一份,而不是用户机器上碰巧装的同名版本。
//!
//! 用户显式写字体文件路径时不走这里 —— 那是他自己的字体,优先级更高。

use crate::font::{build_family, normalize, FaceInfo, FamilyFaces};
use std::path::{Path, PathBuf};

/// 虚拟路径的目录段。Windows 下它是相对路径,不会与真实文件重名。
/// 虚拟路径的目录段。Windows 下它是相对路径,不会与真实文件重名。
pub const PREFIX: &str = "@builtin";

struct BuiltinFace {
    /// 文件名,同时是虚拟路径的最后一段
    key: &'static str,
    bytes: &'static [u8],
    weight: u16,
    italic: bool,
}

struct BuiltinGroup {
    family: &'static str,
    /// 除了家族名之外还可以怎么写(wezterm 文档里的叫法、文件名前缀等)
    aliases: &'static [&'static str],
    monospaced: bool,
    faces: &'static [BuiltinFace],
}

static JETBRAINS: &[BuiltinFace] = &[
    BuiltinFace {
        key: "JetBrainsMono-Regular.ttf",
        bytes: include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
        weight: 400,
        italic: false,
    },
    BuiltinFace {
        key: "JetBrainsMono-Bold.ttf",
        bytes: include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf"),
        weight: 700,
        italic: false,
    },
    BuiltinFace {
        key: "JetBrainsMono-Italic.ttf",
        bytes: include_bytes!("../assets/fonts/JetBrainsMono-Italic.ttf"),
        weight: 400,
        italic: true,
    },
    BuiltinFace {
        key: "JetBrainsMono-BoldItalic.ttf",
        bytes: include_bytes!("../assets/fonts/JetBrainsMono-BoldItalic.ttf"),
        weight: 700,
        italic: true,
    },
];

static NERD_SYMBOLS: &[BuiltinFace] = &[BuiltinFace {
    key: "SymbolsNerdFontMono-Regular.ttf",
    bytes: include_bytes!("../assets/fonts/SymbolsNerdFontMono-Regular.ttf"),
    weight: 400,
    italic: false,
}];

/// 彩色 emoji 专用字体(CBDT 位图版)。它**不进普通字体链**:ab_glyph
/// 只会栅格化 glyf 轮廓,拿这份字体加载不到任何字形,只会被 `emoji` 模块
/// 按 CBDT/CBLC 直接取 PNG。
pub const EMOJI_BYTES: &[u8] = include_bytes!("../assets/fonts/NotoColorEmoji.ttf");

static GROUPS: &[BuiltinGroup] = &[
    BuiltinGroup {
        family: "JetBrains Mono",
        aliases: &["JetBrainsMono", "JetBrains Mono Regular"],
        monospaced: true,
        faces: JETBRAINS,
    },
    BuiltinGroup {
        family: "Symbols Nerd Font Mono",
        // wezterm 文档里管它叫 "Nerd Font Symbols"
        aliases: &[
            "Nerd Font Symbols",
            "SymbolsNerdFontMono",
            "Symbols Nerd Font",
        ],
        monospaced: true,
        faces: NERD_SYMBOLS,
    },
];

fn face_info(group: &BuiltinGroup, face: &BuiltinFace) -> FaceInfo {
    let stem = face.key.trim_end_matches(".ttf");
    FaceInfo {
        path: PathBuf::from(PREFIX).join(face.key),
        index: 0,
        family: group.family.to_string(),
        full_name: stem.to_string(),
        postscript: stem.to_string(),
        weight: face.weight,
        italic: face.italic,
        monospaced: group.monospaced,
    }
}

fn find(spec: &str) -> Option<(&'static BuiltinGroup, Option<&'static BuiltinFace>)> {
    let norm = normalize(spec.trim());
    if norm.is_empty() {
        return None;
    }
    for group in GROUPS {
        let family_hit = normalize(group.family) == norm
            || group
                .aliases
                .iter()
                .any(|a| normalize(a) == norm)
            // `JetBrainsMono-Regular.ttf` 这种文件名写法也算命中
            || group.faces.iter().any(|f| {
                normalize(f.key) == norm || normalize(f.key.trim_end_matches(".ttf")) == norm
            });
        if family_hit {
            // 精确指到某个 face 时只给那个 face(样式覆盖用)
            let exact = group
                .faces
                .iter()
                .find(|f| normalize(f.key) == norm || normalize(f.key.trim_end_matches(".ttf")) == norm);
            return Some((group, exact));
        }
    }
    None
}

/// 按家族名/别名解析出四个样式槽。命中即返回,不再查系统字体。
pub fn resolve(spec: &str) -> Option<FamilyFaces> {
    let (group, exact) = find(spec)?;
    let faces: Vec<FaceInfo> = match exact {
        Some(f) => vec![face_info(group, f)],
        None => group.faces.iter().map(|f| face_info(group, f)).collect(),
    };
    build_family(&faces)
}

/// 单个 face(给 `bold = "JetBrains Mono Bold"` 这类样式覆盖用)。
pub fn resolve_face(spec: &str) -> Option<FaceInfo> {
    let (group, exact) = find(spec)?;
    let face = exact.unwrap_or(&group.faces[0]);
    Some(face_info(group, face))
}

/// 虚拟路径 → 内嵌字节。`FontSlot::from_face` 用它代替 `fs::read`。
pub fn bytes(path: &Path) -> Option<&'static [u8]> {
    if !path.starts_with(PREFIX) {
        return None;
    }
    let name = path.file_name()?.to_str()?;
    GROUPS
        .iter()
        .flat_map(|g| g.faces.iter())
        .find(|f| f.key.eq_ignore_ascii_case(name))
        .map(|f| f.bytes)
}

/// 默认配置用的家族名与回退链。
///
/// 和 wezterm 一样是"主字体 + Nerd Font Symbols + Noto Color Emoji",但
/// 彩色 emoji 那份由 `emoji` 模块单独取 PNG,不能塞进普通文本回退链
/// (CBDT 字体没有 glyf 轮廓)。中文仍靠系统字体,体积太大不适合打包。
pub const DEFAULT_FAMILY: &str = "JetBrains Mono";
pub const DEFAULT_FALLBACK: &[&str] = &["Nerd Font Symbols", "Microsoft YaHei"];

/// 彩色 emoji 回退字体的名字(文档/日志里用,不参与普通文本链)。
pub const EMOJI_FAMILY: &str = "Noto Color Emoji";

#[cfg(test)]
mod tests {
    use super::*;
    use ab_glyph::{Font, FontVec};

    #[test]
    fn every_builtin_font_parses() {
        for group in GROUPS {
            for face in group.faces {
                let font = FontVec::try_from_vec(face.bytes.to_vec())
                    .unwrap_or_else(|_| panic!("内嵌字体 {} 解析失败", face.key));
                assert!(font.units_per_em().unwrap_or(0.0) > 0.0, "{}", face.key);
            }
        }
    }

    #[test]
    fn resolves_family_and_virtual_paths() {
        let jb = resolve("JetBrains Mono").expect("主字体");
        assert_eq!(jb.name, "JetBrains Mono");
        assert!(jb.bold.is_some() && jb.italic.is_some() && jb.bold_italic.is_some());
        assert_eq!(resolve("Nerd Font Symbols").unwrap().name, "Symbols Nerd Font Mono");
        // 彩色 emoji 是 CBDT 位图字体,不进普通字体链(由 emoji 模块单独取 PNG)
        assert!(resolve("Noto Color Emoji").is_none());
        assert_eq!(bytes(&jb.regular.path).unwrap().len(), JETBRAINS[0].bytes.len());
        assert!(bytes(Path::new("C:/Windows/Fonts/consola.ttf")).is_none());
    }
}
