//! 拖进窗口的文件 / 文件夹 → 一段可以直接敲进 shell 的路径文本。
//!
//! 这里只管「文本怎么拼」,不碰窗口、不碰 PTY:输入入口在 `main` 的
//! `drop_paths`,它拿到路径后调用 [`paths_text`] 再 `send_bytes` 写下去。
//! 分开写是为了能对引号规则做纯函数测试 —— 引号拼错的表现是"shell 报
//! 找不到文件",隔着窗口调试一次要开一次资源管理器,不值得。
//!
//! 三条规则:
//! 1. 一律用**绝对路径**(shell 的当前目录可能跟用户想的不一样);
//! 2. 含空格等特殊字符才加双引号 —— 不含就别加,裸路径在命令行里更好读;
//! 3. 结尾补一个空格,方便接着敲参数(和 Windows Terminal 一致)。

use std::path::{Path, PathBuf};

use crate::config::PathQuote;

/// 一批拖入的路径 → 写进 PTY 的一整段文本。
///
/// 多选用空格隔开(它们通常是要拼进同一条命令的参数),末尾再留一个空格
/// 方便接着敲;一次只拖一个文件时也走这里,结果就是"路径 + 一个空格"。
pub fn paths_text(paths: &[PathBuf], quote: PathQuote) -> String {
    let mut out = paths
        .iter()
        .map(|p| shell_text(&absolute(p), quote))
        .collect::<Vec<_>>()
        .join(" ");
    if !out.is_empty() {
        out.push(' ');
    }
    out
}

/// 单条路径 → 写进 PTY 的文本(不带尾部空格)。
pub fn shell_text(path: &Path, quote: PathQuote) -> String {
    let raw = path.to_string_lossy();
    let raw = raw.as_ref();
    let want_quotes = match quote {
        PathQuote::Always => true,
        PathQuote::Never => false,
        PathQuote::Auto => needs_quotes(raw),
    };
    // 以反斜杠结尾的路径只可能是盘符根(`C:\`)。这种路径一定不含空格,
    // 而 `"C:\"` 在 PowerShell / bash 里会被读成"转义了结尾的引号",
    // 所以宁可裸着写 —— 裸的 `C:\` 三个 shell 都认。
    if want_quotes && !raw.ends_with('\\') {
        format!("\"{raw}\"")
    } else {
        raw.to_string()
    }
}

/// 路径能不能裸着塞进命令行。
///
/// 白名单而不是黑名单:Windows 的路径里出现 `&`、`^`、`(`、`%`、`!` 的
/// 概率不低,列黑名单总会漏。多打一对引号没有副作用(除了上面那个盘符根),
/// 少打一对就报错。
fn needs_quotes(s: &str) -> bool {
    s.chars().any(|c| {
        if c.is_ascii_alphanumeric() {
            return false;
        }
        // 非 ASCII(中文 / emoji / 拉丁扩展):cmd、PowerShell、bash 都不需要
        // 引号,加了反而更难读
        if !c.is_ascii() {
            return false;
        }
        // 绝对安全的一小撮:分隔符、盘符冒号、以及文件名里最常见的几个符号
        !matches!(c, '.' | '_' | '-' | '+' | '~' | '/' | '\\' | ':' | '@')
    })
}

/// 拖进来的路径如果是相对的(理论上不会),按当前工作目录补成绝对路径。
///
/// 不用 `canonicalize`:它在 Windows 上会加 `\\?\` 前缀,写进 shell 既难看
/// 又有兼容性问题;而资源管理器给的就是干净的绝对路径,这里只是兜底。
pub fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    match std::env::current_dir() {
        Ok(dir) => dir.join(path),
        Err(_) => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auto(p: &str) -> String {
        shell_text(Path::new(p), PathQuote::Auto)
    }

    fn text(paths: &[&str], quote: PathQuote) -> String {
        let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        paths_text(&paths, quote)
    }

    #[test]
    fn plain_path_stays_bare() {
        // 没有空格、没有特殊字符:裸写最好读
        assert_eq!(
            auto(r"C:\Windows\System32\cmd.exe"),
            r"C:\Windows\System32\cmd.exe"
        );
        assert_eq!(
            auto(r"F:\Code\VectorTerm\src\main.rs"),
            r"F:\Code\VectorTerm\src\main.rs"
        );
    }

    #[test]
    fn spaces_get_quoted() {
        assert_eq!(
            auto(r"C:\Program Files\Git\bin\git.exe"),
            "\"C:\\Program Files\\Git\\bin\\git.exe\""
        );
        assert_eq!(auto(r"D:\我的 文档\a b.txt"), "\"D:\\我的 文档\\a b.txt\"");
    }

    #[test]
    fn folder_paths_are_handled_like_files() {
        // 文件夹在资源管理器里不会带结尾反斜杠,和文件走同一条路径
        assert_eq!(auto(r"C:\Program Files\Git"), "\"C:\\Program Files\\Git\"");
        assert_eq!(auto(r"F:\Code\VectorTerm"), r"F:\Code\VectorTerm");
    }

    #[test]
    fn command_metacharacters_get_quoted() {
        for p in [
            r"C:\a&b.txt",
            r"C:\a^b.txt",
            r"C:\a(b).txt",
            r"C:\100%.txt",
            r"C:\a;b.txt",
            r"C:\a'b.txt",
            r"C:\a=b.txt",
            r"C:\a,b.txt",
        ] {
            assert!(auto(p).starts_with('"'), "{p} 应该被加引号");
        }
    }

    #[test]
    fn quotes_keep_apostrophes_inside() {
        // 双引号包起来时,单引号原样保留(bash 里也安全)
        assert_eq!(auto(r"C:\it's here\x.txt"), "\"C:\\it's here\\x.txt\"");
    }

    #[test]
    fn drive_root_is_never_quoted() {
        // `"C:\"` 会被当成转义引号,盘符根必须裸写
        assert_eq!(auto(r"C:\"), "C:\\");
        assert_eq!(shell_text(Path::new(r"C:\"), PathQuote::Always), "C:\\");
    }

    #[test]
    fn quote_modes_are_respected() {
        assert_eq!(
            shell_text(Path::new(r"C:\a.txt"), PathQuote::Always),
            "\"C:\\a.txt\""
        );
        assert_eq!(
            shell_text(Path::new(r"C:\a b.txt"), PathQuote::Never),
            r"C:\a b.txt"
        );
    }

    #[test]
    fn unicode_without_spaces_stays_bare() {
        assert_eq!(auto(r"D:\项目\终端\说明.md"), r"D:\项目\终端\说明.md");
    }

    #[test]
    fn single_and_multiple_paths_end_with_one_space() {
        assert_eq!(text(&[r"C:\a.txt"], PathQuote::Auto), "C:\\a.txt ");
        assert_eq!(
            text(&[r"C:\a.txt", r"C:\b c.txt"], PathQuote::Auto),
            "C:\\a.txt \"C:\\b c.txt\" "
        );
    }

    #[test]
    fn empty_input_yields_empty_text() {
        assert_eq!(text(&[], PathQuote::Auto), "");
    }
}
