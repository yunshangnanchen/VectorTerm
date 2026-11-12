//! 软件自身的诊断输出 —— 统一送到「另一个调试终端」,绝不写进当前 shell 窗口。
//!
//! wezterm 的做法(本机实测:坏配置启动会同时出现 `wezterm Configuration Error`
//! 和 `cmd.exe` 两个独立窗口):配置和脚本的错误显示在专门的窗口 / Debug
//! Overlay 里,`wezterm.log_*` 也只进那里,**当前 pane 一个字节都不写**。
//! 这样 shell 的提示符、启动脚本输出、TUI 画面都不会被终端软件自己的日志污染。
//!
//! VectorTerm 对应的通道:
//!
//! | 启动方式 | 调试输出去哪 |
//! |---|---|
//! | 任何方式(含 cmd / PowerShell / `cargo run`) | 出错时弹出的独立控制台窗口「VectorTerm Debug Console」 |
//! | 任意方式 | 同时写 `~/.config/VectorTerm/VectorTerm.log`,配置错误另存 `config-error.log` |
//! | `VECTORTERM_DEBUG_CONSOLE=0` | 不弹窗,只留文件日志(纯后台/自动化场景) |
//!
//! 从控制台启动时进程本来挂在父控制台上,这时会先 `FreeConsole` 再另开窗口,
//! 免得软件日志混进启动它的那个 shell 的输出里。
//!
//! **平时不弹窗**:只有出现错误(`werr!`)或显式设置 `VECTORTERM_DEBUG_CONSOLE=1`
//! 时才创建调试控制台。创建时会把之前缓存在内存里的历史日志整段回放,
//! 所以「所有调试输出都进另一个终端」和「非错误情况不弹调试窗口」能同时成立。
//!
//! 调试控制台的关闭按钮被禁用、Ctrl 事件被忽略 —— Windows 的控制台关闭事件
//! 默认会终止宿主进程,不能让用户随手一个 X 就把终端连带 shell 一起关掉。

use parking_lot::Mutex;
use std::collections::VecDeque;
use std::io::Write;
use std::time::Instant;

/// 调试终端里能回放的历史行数。
const MAX_HISTORY: usize = 2000;
/// `VectorTerm.log` 超过这个大小就整个重来(只看最近一段)。
const MAX_LOG_BYTES: u64 = 256 * 1024;

struct Sink {
    history: VecDeque<String>,
    file: Option<std::fs::File>,
    /// 调试控制台的 `CONOUT$`。裸指针不是 `Send`,转成 usize 存才能放进 static。
    debug_conout: Option<usize>,
    revealed: bool,
    /// 启动时就带着控制台(从 cmd / PowerShell 启动,或 debug 构建)
    parent_console: bool,
    start: Option<Instant>,
    errors: usize,
    /// 上一次报过的错误内容,用于热重载去重
    last_error: Option<String>,
}

impl Sink {
    const fn new() -> Self {
        Self {
            history: VecDeque::new(),
            file: None,
            debug_conout: None,
            revealed: false,
            parent_console: false,
            start: None,
            errors: 0,
            last_error: None,
        }
    }
}

static SINK: Mutex<Sink> = Mutex::new(Sink::new());

/// 普通日志(info / warn)。`eprintln!` 的替代,见 `wlog!` 宏。
#[macro_export]
macro_rules! wlog {
    ($($arg:tt)*) => {
        $crate::debug_term::log(&format!($($arg)*))
    };
}

/// 错误日志:除了记录,还会按需要弹出独立的调试终端。
#[macro_export]
macro_rules! werr {
    ($($arg:tt)*) => {
        $crate::debug_term::error(&format!($($arg)*))
    };
}

/// `main` 里 `win_console::attach()` 之后立刻调用。
pub fn init() {
    let mut s = SINK.lock();
    s.start = Some(Instant::now());
    s.parent_console = has_console();
    if let Some(dir) = crate::config::user_config_dir() {
        let path = dir.join("VectorTerm.log");
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
            let _ = std::fs::remove_file(&path);
        }
        let _ = std::fs::create_dir_all(&dir);
        s.file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
        if let Some(f) = s.file.as_mut() {
            let _ = writeln!(f, "=== VectorTerm 启动 ===");
        }
    }
    drop(s);

    // `=1` 常开调试终端(平时也要盯着日志);`=0` 完全不弹窗,只写文件。
    if std::env::var("VECTORTERM_DEBUG_CONSOLE").as_deref() == Ok("1") {
        reveal();
    }
}

/// `VECTORTERM_DEBUG_CONSOLE=0` 时只保留文件日志和 stderr,不创建窗口。
fn debug_console_allowed() -> bool {
    std::env::var("VECTORTERM_DEBUG_CONSOLE").as_deref() != Ok("0")
}

/// 写一条日志。分发到 stderr(父控制台/重定向)、文件、历史缓冲、调试终端。
pub fn log(line: &str) {
    if cfg!(test) {
        // 单测要能在 `--nocapture` 里看到输出,且不往用户目录写文件
        eprintln!("{line}");
        return;
    }
    let elapsed = SINK
        .lock()
        .start
        .map(|t| t.elapsed().as_millis())
        .unwrap_or(0);
    let thread = std::thread::current();
    let who = thread.name().unwrap_or("<unnamed>");
    for part in line.split('\n') {
        let clean = sanitize(part);
        let stamped = format!("[{elapsed:>6}ms] [{who}] {clean}");
        emit(&stamped);
    }
}

/// 写一条错误日志,并把独立调试终端调出来(同一条错误不重复弹)。
pub fn error(line: &str) {
    if cfg!(test) {
        eprintln!("ERROR {line}");
        return;
    }
    log(&format!("ERROR {line}"));

    let mut s = SINK.lock();
    s.errors += 1;
    let is_new = is_new_error(s.last_error.as_deref(), line);
    s.last_error = Some(line.to_owned());
    let count = s.errors;
    let revealed = s.revealed;
    let need_reveal = is_new && !s.revealed && debug_console_allowed();
    drop(s);

    if need_reveal {
        reveal();
    } else if revealed {
        // 只改自己那个调试终端的标题。从 cmd 启动时没有独立窗口,
        // 这里绝不能去动父控制台的标题。
        set_console_title(&format!("VectorTerm Debug Console - {count} 个错误"));
    }
}

fn emit(stamped: &str) {
    let mut s = SINK.lock();
    // 1) stderr。父控制台场景它就是「另一个终端」;双击场景 win_console 把它
    //    指到 NUL,写失败也无害(一律忽略,绝不用 eprintln! —— 那会 panic)。
    let _ = std::io::stderr().write_all(format!("{stamped}\n").as_bytes());
    // 2) 常驻日志文件
    if let Some(f) = s.file.as_mut() {
        let _ = writeln!(f, "{stamped}");
    }
    // 3) 历史(调试终端晚开时回放)
    push_capped(&mut s.history, stamped.to_owned(), MAX_HISTORY);
    // 4) 已打开的调试终端
    if let Some(h) = s.debug_conout {
        let is_error = stamped.contains("ERROR ");
        write_console(h, &format!("{stamped}\r\n"), is_error);
    }
}

/// 打开(或唤醒)独立的调试终端。
///
/// 即使是从 cmd / PowerShell 启动(进程已经挂在父控制台上),也先 `FreeConsole`
/// 再另开一个窗口 —— 软件自身的日志不该混进启动它的那个 shell 的输出里,
/// 更不该混进 VectorTerm 自己的终端窗口。`VECTORTERM_DEBUG_CONSOLE=0` 可关掉弹窗,
/// 只保留文件日志和 stderr。
pub fn reveal() {
    if cfg!(test) || !debug_console_allowed() {
        return;
    }
    let (title, detach_parent, history) = {
        let s = SINK.lock();
        if s.revealed {
            return;
        }
        (
            if s.errors == 0 {
                "VectorTerm Debug Console".to_owned()
            } else {
                format!("VectorTerm Debug Console - {} 个错误", s.errors)
            },
            s.parent_console,
            s.history.iter().cloned().collect::<Vec<_>>(),
        )
    };

    // Windows API 调用放在锁外:AllocConsole 可能要拉起 conhost / Windows
    // Terminal,耗时不可控,不能让日志调用线程一起卡在临界区里。
    match open_console(&title, detach_parent) {
        Ok(h) => {
            {
                let mut s = SINK.lock();
                s.debug_conout = Some(h);
                s.revealed = true;
            }
            let banner = "VectorTerm 调试终端 —— 这里是软件自身的日志,不会写进终端窗口。\r\n\
                          关闭按钮已禁用;退出请关闭 VectorTerm 主窗口。\r\n\
                          ------------------------------------------------------------\r\n";
            write_console(h, banner, false);
            for line in &history {
                write_console(h, &format!("{line}\r\n"), line.contains("ERROR "));
            }
        }
        Err(why) => log(&format!(
            "[VectorTerm] 创建独立调试终端失败({why}),日志仅写入 VectorTerm.log"
        )),
    }
}

/// 纯函数:同一条错误反复出现(编辑器每存一次触发一次热重载)不该重复弹窗。
fn is_new_error(last: Option<&str>, now: &str) -> bool {
    last != Some(now)
}

fn push_capped(history: &mut VecDeque<String>, line: String, cap: usize) {
    if history.len() >= cap {
        history.pop_front();
    }
    history.push_back(line);
}

/// 日志里不该出现控制字符:ESC 会在控制台里被解释成别的动作,\r 会让行错位。
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim_end()
        .to_owned()
}

// ---------------------------------------------------------------------------
// Windows 平台部分
// ---------------------------------------------------------------------------

#[cfg(all(windows, not(test)))]
fn has_console() -> bool {
    use windows_sys::Win32::System::Console::GetConsoleWindow;
    unsafe { !GetConsoleWindow().is_null() }
}

#[cfg(not(all(windows, not(test))))]
fn has_console() -> bool {
    false
}

#[cfg(all(windows, not(debug_assertions), not(test)))]
unsafe extern "system" fn ignore_console_ctrl(ctrl: u32) -> i32 {
    use windows_sys::Win32::System::Console::{CTRL_C_EVENT, CTRL_CLOSE_EVENT};
    // 只吞 Ctrl+C 和关闭事件(用户点调试窗口的 X 不该把整个终端带走);
    // 注销 / 关机仍交回系统默认处理,免得卡住关机流程。
    if ctrl == CTRL_C_EVENT || ctrl == CTRL_CLOSE_EVENT {
        1
    } else {
        0
    }
}

/// 创建独立控制台窗口,返回它的 `CONOUT$` 句柄(转 usize)。
#[cfg(all(windows, not(debug_assertions), not(test)))]
fn open_console(title: &str, detach_parent: bool) -> Result<usize, String> {
    use windows_sys::Win32::Foundation::{GetLastError, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        AllocConsole, FreeConsole, GetConsoleMode, GetConsoleWindow, SetConsoleCtrlHandler,
        SetConsoleMode, SetConsoleTitleW, SetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DeleteMenu, GetSystemMenu, SetWindowPos, MF_BYCOMMAND, SC_CLOSE,
    };

    unsafe fn open_device(name: &str) -> HANDLE {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        CreateFileW(
            wide.as_ptr(),
            FILE_GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    }

    unsafe {
        let mut note = String::new();
        if detach_parent && FreeConsole() == 0 {
            note = format!(" FreeConsole_err={}", GetLastError());
        }
        if AllocConsole() == 0 {
            return Err(format!("AllocConsole_err={}{note}", GetLastError()));
        }
        // AllocConsole 会重设进程的 std handles,把它们压回 NUL:日志由
        // write_console 明确写一次,不能让 stderr 再往同一个控制台写第二遍。
        let nul = open_device("NUL");
        if nul != INVALID_HANDLE_VALUE {
            SetStdHandle(STD_OUTPUT_HANDLE, nul);
            SetStdHandle(STD_ERROR_HANDLE, nul);
        }

        let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        SetConsoleTitleW(wide.as_ptr());

        let hwnd = GetConsoleWindow();
        if !hwnd.is_null() {
            let menu = GetSystemMenu(hwnd, 0);
            if !menu.is_null() {
                DeleteMenu(menu, SC_CLOSE, MF_BYCOMMAND);
            }
            // 放到左上偏内一点,别正好压住主窗口的标题栏
            SetWindowPos(hwnd, std::ptr::null_mut(), 80, 60, 900, 520, 0x4 | 0x40);
        }
        SetConsoleCtrlHandler(Some(ignore_console_ctrl), 1);

        let out = open_device("CONOUT$");
        if out == INVALID_HANDLE_VALUE {
            return Err(format!("CONOUT$_err={}", GetLastError()));
        }
        // 关掉快速编辑:控制台进入"标记"状态时 WriteConsoleW 会阻塞,
        // 而日志是在调用线程上同步写的,不能让主窗口跟着卡住。
        // 0x40 = ENABLE_QUICK_EDIT_MODE, 0x20 = ENABLE_INSERT_MODE
        let mut mode = 0u32;
        if GetConsoleMode(out, &mut mode) != 0 {
            SetConsoleMode(out, mode & !0x60);
        }
        Ok(out as usize)
    }
}

#[cfg(not(all(windows, not(debug_assertions), not(test))))]
fn open_console(_title: &str, _detach_parent: bool) -> Result<usize, String> {
    Err("非发行版构建".to_owned())
}

#[cfg(all(windows, not(debug_assertions), not(test)))]
fn write_console(h: usize, text: &str, is_error: bool) {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::Console::{SetConsoleTextAttribute, WriteConsoleW};
    let wide: Vec<u16> = text.encode_utf16().collect();
    if wide.is_empty() {
        return;
    }
    unsafe {
        let handle = h as HANDLE;
        if is_error {
            SetConsoleTextAttribute(handle, 12); // 亮红
        }
        let mut written = 0u32;
        WriteConsoleW(
            handle,
            wide.as_ptr(),
            wide.len() as u32,
            &mut written,
            std::ptr::null(),
        );
        if is_error {
            SetConsoleTextAttribute(handle, 7); // 恢复默认灰白
        }
    }
}

#[cfg(not(all(windows, not(debug_assertions), not(test))))]
fn write_console(_h: usize, _text: &str, _is_error: bool) {}

#[cfg(all(windows, not(debug_assertions), not(test)))]
fn set_console_title(title: &str) {
    use windows_sys::Win32::System::Console::SetConsoleTitleW;
    let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        SetConsoleTitleW(wide.as_ptr());
    }
}

#[cfg(not(all(windows, not(debug_assertions), not(test))))]
fn set_console_title(_title: &str) {}

#[cfg(test)]
mod tests {
    use super::{is_new_error, push_capped, sanitize};
    use std::collections::VecDeque;

    #[test]
    fn same_error_is_not_reported_twice() {
        assert!(is_new_error(None, "boom"));
        assert!(!is_new_error(Some("boom"), "boom"));
        assert!(is_new_error(Some("boom"), "boom\nmore detail"));
    }

    #[test]
    fn history_is_bounded() {
        let mut q = VecDeque::new();
        for i in 0..10 {
            push_capped(&mut q, i.to_string(), 3);
        }
        assert_eq!(q.len(), 3);
        assert_eq!(q.iter().map(String::as_str).collect::<Vec<_>>(), ["7", "8", "9"]);
    }

    #[test]
    fn control_chars_never_reach_the_debug_terminal() {
        // 错误文本来自 Lua / 文件路径,夹一个 ESC[2J 不应该把日志窗口清屏
        assert_eq!(sanitize("坏配置 \x1b[2J 试试"), "坏配置  [2J 试试");
        assert_eq!(sanitize("a\rb\n"), "a b"); // \n 由 log() 拆行处理
        assert_eq!(sanitize("  末尾空格  "), "  末尾空格");
    }
}
