//! portable-pty(Windows ConPTY)会话管理

use crate::config::AppConfig;
use anyhow::Context;
use portable_pty::{native_pty_system, Child, ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// shell 解析
//
// 默认配置里写的是 `pwsh.exe`(PowerShell 7 的应用执行别名),但不是每台 Windows
// 都装了 PS7,而 CreateProcessW 找不到可执行文件时会直接返回"系统找不到指定的
// 文件",整个程序起不来。终端不该因为少装一个 shell 就退出,所以启动前先解析:
//   PowerShell 7 → Windows PowerShell → cmd → %COMSPEC%
// 谁在就用谁,并把这个决定打印出来(默认 shell 就是第一项,所以正常情况下无提示)。
// ---------------------------------------------------------------------------

/// 最终要执行的 shell(程序 + 参数)以及替换原因(用于提示)。
pub struct ShellChoice {
    pub prog: String,
    pub args: Vec<String>,
    pub note: Option<String>,
}

/// 兜底顺序。每项自带默认参数:从 PowerShell 换到 cmd 时,
/// 原来的 `-NoLogo` 之类不能一起带过去(cmd 不认)。
///
/// 顺序是刻意排的:**PowerShell 7 优先**。它是当前受支持的版本,PSReadLine 的
/// 补全、UTF-8 默认编码、`&&` 链路都比 5.1 好;5.1 只作"没装 PS7"的兜底。
const FALLBACKS: &[(&str, &[&str])] = &[
    ("pwsh.exe", &["-NoLogo"]),
    ("powershell.exe", &["-NoLogo"]),
    ("cmd.exe", &[]),
];

/// 这个 shell 是不是 PowerShell 7。回退提示要靠它区分"降级"和"升级"。
fn is_pwsh(exe: &str) -> bool {
    std::path::Path::new(exe)
        .file_stem()
        .is_some_and(|s| s.eq_ignore_ascii_case("pwsh"))
}

/// `pwsh.exe` 除 PATH 之外的常见安装位置。
///
/// 从 Microsoft Store 装的话 `pwsh.exe` 是 `%LOCALAPPDATA%\Microsoft\WindowsApps`
/// 下的应用执行别名;MSI / winget / scoop 装的话在 `%ProgramFiles%\PowerShell\7`。
/// 刚装完 PS7 就启动(资源管理器还带着安装前的旧 PATH,子进程原样继承)时,
/// 只有这些路径能找到它 —— 否则会被静默降级到 5.1。
fn pwsh_well_known() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        let base = PathBuf::from(base);
        out.push(base.join("Microsoft/WindowsApps/pwsh.exe"));
        out.push(base.join("Microsoft/WindowsApps/PowerShell/pwsh.exe"));
    }
    // ProgramW6432 在 32 位进程里指向真正的 64 位 Program Files
    for var in ["ProgramFiles", "ProgramW6432"] {
        let Some(base) = std::env::var_os(var) else {
            continue;
        };
        let base = PathBuf::from(base);
        out.push(base.join("PowerShell/7/pwsh.exe"));
        out.push(base.join("PowerShell/7-preview/pwsh.exe"));
    }
    out
}

/// 找 shell:先按 PATH,再查 PowerShell 7 的常见安装位置。
fn find_shell(name: &str) -> Option<PathBuf> {
    if let Some(p) = which(name) {
        return Some(p);
    }
    if is_pwsh(name) {
        return pwsh_well_known().into_iter().find(|p| p.is_file());
    }
    None
}

/// 在 PATH 里找可执行文件:带路径分隔符的直接看文件是否存在,
/// 裸名字按 PATHEXT 逐个补后缀试。
pub fn which(exe: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH");
    let pathext = std::env::var_os("PATHEXT");
    which_in(exe, path.as_deref(), pathext.as_deref())
}

fn which_in(exe: &str, path: Option<&OsStr>, pathext: Option<&OsStr>) -> Option<PathBuf> {
    let exe = exe.trim().trim_matches('"');
    if exe.is_empty() {
        return None;
    }
    let looks_pathed = exe.contains('\\') || exe.contains('/');
    if looks_pathed || Path::new(exe).is_absolute() {
        let p = PathBuf::from(exe);
        return p.is_file().then_some(p);
    }

    let exts: Vec<String> = pathext
        .map(|s| {
            s.to_string_lossy()
                .split(';')
                .filter(|e| !e.is_empty())
                .map(|e| {
                    let e = e.to_ascii_lowercase();
                    if e.starts_with('.') { e } else { format!(".{e}") }
                })
                .collect()
        })
        .filter(|v: &Vec<String>| !v.is_empty())
        .unwrap_or_else(|| vec![".exe".into(), ".cmd".into(), ".bat".into(), ".com".into()]);

    let dirs = path.map(std::env::split_paths).into_iter().flatten();
    for dir in dirs {
        // PATH 里偶尔会有带引号或空白的项
        let dir = dir.to_string_lossy();
        let dir = dir.trim().trim_matches('"');
        if dir.is_empty() {
            continue;
        }
        let dir = Path::new(dir);
        // 先按原样试一次(配置里可能已经写了扩展名)
        let direct = dir.join(exe);
        if direct.is_file() {
            return Some(direct);
        }
        let has_ext = Path::new(exe)
            .extension()
            .map(|e| !e.is_empty())
            .unwrap_or(false);
        if has_ext {
            continue;
        }
        for ext in &exts {
            let candidate = dir.join(format!("{exe}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// 解析出最终可执行的 shell。`find` 便于测试注入。
fn resolve_shell_with<F>(shell: &str, args: &[String], find: F) -> ShellChoice
where
    F: Fn(&str) -> Option<PathBuf>,
{
    if let Some(p) = find(shell) {
        return ShellChoice {
            prog: p.to_string_lossy().into_owned(),
            args: args.to_vec(),
            note: None,
        };
    }

    for (cand, cand_args) in FALLBACKS {
        if cand.eq_ignore_ascii_case(shell) {
            continue; // 刚试过
        }
        if let Some(p) = find(cand) {
            let hint = if is_pwsh(shell) && !is_pwsh(cand) {
                "装了 PowerShell 7 就会自动用回来"
            } else {
                "想固定的话把配置里的 shell 改成它"
            };
            return ShellChoice {
                prog: p.to_string_lossy().into_owned(),
                args: cand_args.iter().map(|s| s.to_string()).collect(),
                note: Some(format!("找不到 {shell},已自动改用 {cand}({hint})")),
            };
        }
    }

    // 连 cmd 都找不到时退回 %COMSPEC%,再不行就把用户写的原样丢出去
    for key in ["COMSPEC", "ComSpec"] {
        if let Some(v) = std::env::var_os(key) {
            let s = v.to_string_lossy().into_owned();
            if find(&s).is_some() {
                return ShellChoice {
                    prog: s,
                    args: Vec::new(),
                    note: Some(format!("找不到 {shell},已改用 %{key}%")),
                };
            }
        }
    }

    ShellChoice {
        prog: shell.to_string(),
        args: args.to_vec(),
        note: Some(format!(
            "找不到 {shell},也没找到任何备用 shell(已经试过 pwsh/powershell/cmd)"
        )),
    }
}

/// 启动前把配置修成这台机器真的能跑的样子:shell 存在、工作目录存在。
/// 只改内存里的配置,不动用户的文件。
pub fn apply_shell_choice(cfg: &mut AppConfig) {
    let choice = resolve_shell_with(&cfg.shell, &cfg.shell_args, find_shell);
    if let Some(note) = &choice.note {
        crate::wlog!("[VectorTerm] {note}");
    }
    cfg.shell = choice.prog;
    cfg.shell_args = choice.args;

    if let Some(cwd) = cfg.cwd.clone() {
        if !cwd.is_dir() {
            crate::wlog!(
                "[VectorTerm] 工作目录 {} 不存在(配置可能来自另一台机器),改用默认目录",
                cwd.display()
            );
            cfg.cwd = None;
        }
    }

    // PowerShell 的 `Set-Location` 不改进程 cwd,从外面读不到它现在在哪 ——
    // 给它注入一段 prompt 钩子上报 OSC 7,终端才能跟上 shell 的目录。
    if cfg.files_gui.follow_cwd {
        let extra = osc7_prompt_args(&cfg.shell, &cfg.shell_args);
        if !extra.is_empty() {
            crate::wlog!("[VectorTerm] 已给 {} 注入 OSC 7 prompt 钩子", cfg.shell);
            cfg.shell_args.extend(extra);
        }
    }
}

/// 这个程序是不是 PowerShell 系(`pwsh` / `powershell`)。
///
/// 用来决定"能不能靠读进程 PEB 拿 cwd":PowerShell 的 `Set-Location` 只改它
/// 自己的位置,进程 cwd 一直是启动时那个,读了会得到错的目录。
pub fn is_powershell(prog: &str) -> bool {
    Path::new(prog)
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|s| s == "pwsh" || s == "powershell")
}

/// 给 PowerShell 注入的 prompt 钩子脚本的路径(写在临时目录里,内容是固定的)。
fn osc7_prompt_script_path() -> PathBuf {
    std::env::temp_dir().join("vectorterm").join("osc7-prompt.ps1")
}

/// 钩子脚本内容。
///
/// 做法:把用户原来的 `prompt` 存起来,换成我们的一层包装 —— 先用 OSC 7
/// 把当前目录报给终端,再调用原来的 prompt。所以自定义提示符(oh-my-posh /
/// starship / 自己写的)照样生效,只是多了一串不可见的控制序列。
const OSC7_PROMPT_PS1: &str = r#"# VectorTerm 自动生成:每次画提示符前用 OSC 7 上报当前目录。
# PowerShell 的 Set-Location 不会改进程 cwd,终端只能靠这个知道 shell 在哪个目录。
# 删掉也没关系,下次启动会重新生成。
if (-not (Test-Path Variable:\global:__vectorterm_prev_prompt)) {
    $global:__vectorterm_prev_prompt = $function:prompt
}
function global:prompt {
    $dir = $null
    try {
        $dir = $ExecutionContext.SessionState.Path.CurrentFileSystemLocation.ProviderPath
    } catch { }
    if (-not $dir) {
        try { $dir = $ExecutionContext.SessionState.Path.CurrentLocation.Path } catch { }
    }
    if ($dir) {
        $esc = [char]27
        $path = $dir.Replace('\', '/')
        if (-not $path.StartsWith('/')) { $path = '/' + $path }
        Write-Host -NoNewline ($esc + ']7;file://' + $path + $esc + '\')
    }
    if ($global:__vectorterm_prev_prompt) {
        & $global:__vectorterm_prev_prompt
    } else {
        "PS $($ExecutionContext.SessionState.Path.CurrentLocation)> "
    }
}
"#;

/// 需要追加到 PowerShell 启动参数后面的那串 `-NoExit -File <脚本>`。
///
/// 返回空表示"不用 / 不能注入":不是 PowerShell,或者用户自己写了
/// `-Command` / `-File` 这类非交互用法(硬塞 `-NoExit` 会改写他的意图)。
pub fn osc7_prompt_args(prog: &str, args: &[String]) -> Vec<String> {
    if !is_powershell(prog) {
        return Vec::new();
    }
    for a in args {
        let a = a.trim_start_matches(['-', '/']).to_ascii_lowercase();
        // 交互式启动才谈得上 prompt;这些开关说明用户要的是"跑一段就退"
        if matches!(
            a.as_str(),
            "command" | "c" | "file" | "f" | "encodedcommand" | "ec" | "e" | "noninteractive"
        ) {
            return Vec::new();
        }
    }
    let path = osc7_prompt_script_path();
    if let Some(dir) = path.parent() {
        if std::fs::create_dir_all(dir).is_err() {
            return Vec::new();
        }
    }
    // 内容没变就不重写:避免每次开窗都碰磁盘时间戳
    let ok = match std::fs::read_to_string(&path) {
        Ok(s) => s == OSC7_PROMPT_PS1,
        Err(_) => std::fs::write(&path, OSC7_PROMPT_PS1).is_ok(),
    };
    if !ok {
        crate::wlog!("[VectorTerm] 写不了 OSC 7 钩子脚本 {}", path.display());
        return Vec::new();
    }
    vec![
        "-NoExit".to_string(),
        // Windows 客户端默认 ExecutionPolicy 是 Restricted,`-File` 会被直接拒掉;
        // 只对这一个进程放行,不改用户的机器设置
        "-ExecutionPolicy".to_string(),
        "Bypass".to_string(),
        "-File".to_string(),
        path.to_string_lossy().into_owned(),
    ]
}

pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// shell 自己的进程号。退出确认要靠它判断"这条会话里还有没有别的程序"
    pid: Option<u32>,
}

impl PtySession {
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }
    pub fn resize(&self, rows: u16, cols: u16, pixel_width: u16, pixel_height: u16) -> anyhow::Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width,
                pixel_height,
            })
            .context("resize ConPTY")
    }

    pub fn kill(&mut self) {
        let _ = self.killer.kill();
    }
}

pub struct PtyParts {
    pub session: PtySession,
    pub writer: Box<dyn Write + Send>,
    pub reader: Box<dyn Read + Send>,
    pub child: Box<dyn Child + Send + Sync>,
}

pub fn spawn(cfg: &AppConfig, rows: u16, cols: u16, pixel_width: u16, pixel_height: u16) -> anyhow::Result<PtyParts> {
    let cwd = cfg.cwd.as_deref().map(|p| p.to_string_lossy().into_owned());
    spawn_with(
        &cfg.shell,
        &cfg.shell_args,
        cwd.as_deref(),
        &cfg.env,
        rows,
        cols,
        pixel_width,
        pixel_height,
    )
}

/// 用显式参数起一个 PTY(守护进程端用,不经过 AppConfig)。
/// `cwd` / `env` 为显式字符串,与 `spawn` 的配置路径解耦。
pub fn spawn_with(
    shell: &str,
    args: &[String],
    cwd: Option<&str>,
    env: &[(String, String)],
    rows: u16,
    cols: u16,
    pixel_width: u16,
    pixel_height: u16,
) -> anyhow::Result<PtyParts> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width,
            pixel_height,
        })
        .context("打开 ConPTY 失败(需要 Windows 10 1809+)")?;

    let mut cmd = CommandBuilder::new(shell);
    for arg in args {
        cmd.arg(arg);
    }
    if let Some(cwd) = cwd {
        cmd.cwd(cwd);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    // 让子进程知道自己在 VectorTerm 里,方便用户配置 / 外部工具做能力探测
    // (用户配置里的 env 可以覆盖)。终端自身的动作不再依赖这个变量。
    cmd.env("VECTORTERM", "1");
    // 用户配置的环境变量最后应用,可覆盖上面的默认值
    for (k, v) in env {
        cmd.env(k, v);
    }

    let child = pair
        .slave
        .spawn_command(cmd)
        .with_context(|| format!("启动 shell 失败({shell})"))?;
    // 父进程侧必须释放 slave,否则关闭时机不对
    drop(pair.slave);

    let killer = child.clone_killer();
    let pid = child.process_id();
    let reader = pair
        .master
        .try_clone_reader()
        .context("clone PTY reader 失败")?;
    let writer = pair.master.take_writer().context("take PTY writer 失败")?;

    Ok(PtyParts {
        session: PtySession {
            master: pair.master,
            killer,
            pid,
        },
        writer,
        reader,
        child,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        // 用 target/ 下的临时目录(系统 %TEMP% 在部分沙箱里不可写)
        let p = Path::new("target").join(format!("pty-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn which_finds_exe_via_pathext() {
        let dir = tmpdir("which");
        let exe = dir.join("fakeshell.exe");
        std::fs::write(&exe, b"").unwrap();

        // 裸名字 + PATHEXT
        let got = which_in("fakeshell", Some(dir.as_os_str()), Some(OsStr::new(".EXE")));
        assert_eq!(got.as_deref(), Some(exe.as_path()));

        // 带扩展名
        let got = which_in("fakeshell.exe", Some(dir.as_os_str()), None);
        assert_eq!(got.as_deref(), Some(exe.as_path()));

        // 找不到就是 None,不会 panic
        assert!(which_in("nope", Some(dir.as_os_str()), None).is_none());

        // 直接给绝对路径
        let got = which_in(exe.to_str().unwrap(), None, None);
        assert_eq!(got.as_deref(), Some(exe.as_path()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn which_skips_empty_path_entries() {
        assert!(which_in("whatever", Some(OsStr::new(";;")), None).is_none());
    }

    #[test]
    fn missing_shell_falls_back_to_powershell() {
        // 模拟这台机器没装 pwsh,但有 Windows PowerShell
        let choice = resolve_shell_with("pwsh.exe", &["-NoLogo".to_string()], |name| {
            name.eq_ignore_ascii_case("powershell.exe").then(|| {
                PathBuf::from("C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe")
            })
        });
        assert!(choice.prog.ends_with("powershell.exe"));
        // -NoLogo 对 powershell.exe 同样有效,保留
        assert_eq!(choice.args, vec!["-NoLogo"]);
        assert!(choice.note.as_deref().unwrap().contains("pwsh.exe"));
    }

    #[test]
    fn falling_back_to_cmd_drops_powershell_args() {
        let choice = resolve_shell_with("pwsh.exe", &["-NoLogo".to_string()], |name| {
            name.eq_ignore_ascii_case("cmd.exe")
                .then(|| PathBuf::from("C:\\Windows\\System32\\cmd.exe"))
        });
        assert!(choice.prog.ends_with("cmd.exe"));
        // cmd 不认 -NoLogo,必须丢掉
        assert!(choice.args.is_empty(), "回退到 cmd 时不能带 PowerShell 参数");
    }

    /// 两个 PowerShell 都在时,必须选 7 而不是系统自带的 5.1。
    #[test]
    fn fallback_prefers_powershell7() {
        let choice = resolve_shell_with("nushell.exe", &[], |name| match name {
            n if n.eq_ignore_ascii_case("pwsh.exe") => {
                Some(PathBuf::from("C:\\Program Files\\PowerShell\\7\\pwsh.exe"))
            }
            n if n.eq_ignore_ascii_case("powershell.exe") => Some(PathBuf::from(
                "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
            )),
            _ => None,
        });
        assert!(choice.prog.ends_with("pwsh.exe"), "第一顺位是 PowerShell 7");
        assert_eq!(choice.args, vec!["-NoLogo"]);
    }

    /// 默认配置原样解析:PS7 在就用 PS7,参数只有 -NoLogo。
    #[test]
    fn default_shell_resolves_without_note() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.shell, "pwsh.exe");
        let choice = resolve_shell_with(&cfg.shell, &cfg.shell_args, |name| {
            name.eq_ignore_ascii_case("pwsh.exe")
                .then(|| PathBuf::from("C:\\Program Files\\PowerShell\\7\\pwsh.exe"))
        });
        assert!(choice.prog.ends_with("pwsh.exe"));
        assert_eq!(choice.args, vec!["-NoLogo"]);
        assert!(choice.note.is_none(), "默认 shell 存在时不该有替换提示");
    }

    /// 三档全试过:PS7 → 5.1 → cmd。
    #[test]
    fn fallback_chain_is_pwsh_then_windows_powershell_then_cmd() {
        let seen = std::cell::RefCell::new(Vec::new());
        let choice = resolve_shell_with("pwsh.exe", &["-NoLogo".to_string()], |name| {
            seen.borrow_mut().push(name.to_string());
            name.eq_ignore_ascii_case("cmd.exe")
                .then(|| PathBuf::from("C:\\Windows\\System32\\cmd.exe"))
        });
        assert!(choice.prog.ends_with("cmd.exe"));
        let seen = seen.borrow();
        let ps7 = seen
            .iter()
            .position(|n| n.eq_ignore_ascii_case("pwsh.exe"))
            .expect("应该先试 pwsh");
        let ps5 = seen
            .iter()
            .position(|n| n.eq_ignore_ascii_case("powershell.exe"))
            .expect("再试 Windows PowerShell");
        let cmd = seen
            .iter()
            .position(|n| n.eq_ignore_ascii_case("cmd.exe"))
            .expect("最后才是 cmd");
        assert!(ps7 < ps5 && ps5 < cmd, "顺序应为 pwsh → powershell → cmd");
    }

    /// 从 PS7 降级时的提示要告诉用户"装了 PS7 会自动用回来"。
    #[test]
    fn downgrade_note_mentions_powershell7() {
        let choice = resolve_shell_with("pwsh.exe", &[], |name| {
            name.eq_ignore_ascii_case("powershell.exe")
                .then(|| PathBuf::from("C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"))
        });
        let note = choice.note.unwrap();
        assert!(note.contains("PowerShell 7"), "实际提示:{note}");
    }

    #[test]
    fn existing_shell_is_kept_untouched() {
        let choice = resolve_shell_with("cmd.exe", &[], |name| {
            name.eq_ignore_ascii_case("cmd.exe")
                .then(|| PathBuf::from("C:\\Windows\\System32\\cmd.exe"))
        });
        assert!(choice.prog.ends_with("cmd.exe"));
        assert!(choice.note.is_none(), "能找到就不该提示替换");
    }

    #[test]
    fn nothing_found_keeps_original_and_explains() {
        let choice = resolve_shell_with("nushell.exe", &[], |_| None);
        assert_eq!(choice.prog, "nushell.exe");
        assert!(choice.note.unwrap().contains("nushell.exe"));
    }

    /// OSC 7 钩子只给 PowerShell 注入,而且遇到非交互用法要让路。
    #[test]
    fn osc7_hook_only_for_interactive_powershell() {
        assert!(osc7_prompt_args("C:\\Windows\\System32\\cmd.exe", &[]).is_empty());
        assert!(osc7_prompt_args("nvim.exe", &[]).is_empty(), "编辑器 pane 不该被注入");

        let args = osc7_prompt_args("C:\\Program Files\\PowerShell\\7\\pwsh.exe", &["-NoLogo".to_string()]);
        assert_eq!(args[0], "-NoExit");
        assert_eq!(args[1], "-ExecutionPolicy", "Restricted 策略下 -File 会被拒");
        assert_eq!(args[2], "Bypass");
        assert_eq!(args[3], "-File");
        assert!(args[4].ends_with("osc7-prompt.ps1"));
        assert!(Path::new(&args[4]).is_file(), "脚本内容要真的写出来");
        let body = std::fs::read_to_string(&args[4]).unwrap();
        assert!(body.contains("]7;file://"), "脚本里要发 OSC 7");

        // 用户自己写了非交互开关时不注入(硬塞 -NoExit 会改写他的意图)
        for a in ["-Command", "/c", "-File", "-NonInteractive", "-EncodedCommand"] {
            assert!(
                osc7_prompt_args("powershell.exe", &[a.to_string()]).is_empty(),
                "{a} 是交互以外的用法"
            );
        }
        // 已经注入过(参数里带 -File)就不会再叠一层
        assert!(osc7_prompt_args("pwsh.exe", &args).is_empty());
    }
}

