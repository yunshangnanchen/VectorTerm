//! 进程树查询:判断某个标签里"除了 shell 自己,还有没有别的程序在跑"。
//!
//! 判据照抄 wezterm 的 `LocalPane::can_close_without_prompting`:
//!
//! 1. 从 shell 的 pid 出发,把**整棵进程树**(含 shell 自己)的 exe 名收起来;
//! 2. 减去 `skip_close_confirmation_for_processes_named` 那张白名单;
//! 3. 还剩东西 → "有状态",关之前问一句;一个不剩 → 直接关。
//!
//! 口径是**白名单**,不是"看有没有子进程"。两者在这几种情况下分道扬镳:
//! - `pwsh` 里敲 `cmd` 又退出后还挂着 `cmd.exe`:子进程判据会拦,白名单不会 ——
//!   `cmd.exe` 本来就在名单里,关掉它没有任何损失。
//! - `shell = "wsl.exe"` 这种名单里没有的 shell:白名单会一直问。wezterm 也这样,
//!   想安静就把它自己加进名单。
//!
//! 拿不到进程表时返回 `None`,调用方按"有状态"处理 —— 和 wezterm 一致,
//! 判不准的时候宁可多问一句。

/// 进程树里一个节点
#[derive(Debug, Clone)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
}

/// 快照当前所有进程。失败返回 `None`(而不是空表)—— 调用方要能分清
/// "真的一个进程都没有"和"根本没查到"。
#[cfg(windows)]
pub fn snapshot() -> Option<Vec<Proc>> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut entry);
        while ok != 0 {
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            out.push(Proc {
                pid: entry.th32ProcessID,
                ppid: entry.th32ParentProcessID,
                name: String::from_utf16_lossy(&entry.szExeFile[..len]),
            });
            ok = Process32NextW(snap, &mut entry);
        }
        CloseHandle(snap);
    }
    Some(out)
}

#[cfg(not(windows))]
pub fn snapshot() -> Option<Vec<Proc>> {
    None
}

/// ConPTY 宿主、以及一些"总是挂在那儿"的壳进程:它们不代表用户在跑程序。
///
/// `conhost.exe` / `OpenConsole.exe` 是控制台宿主;`WmiPrvSE.exe` 之类不在
/// 这里排,因为那真的是用户跑起来的。只排宿主,避免误报。
fn is_console_host(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "conhost.exe" | "openconsole.exe" | "conpty.exe"
    )
}

/// 从 `root` 出发的整棵子树(不含 root 自己),按层展开。
fn descendants(all: &[Proc], root: u32) -> Vec<Proc> {
    let mut out = Vec::new();
    let mut frontier = vec![root];
    // 进程树深度有限,但异常情况下可能有环(pid 复用),加个上限兜住
    for _ in 0..16 {
        let mut next = Vec::new();
        for p in all {
            if frontier.contains(&p.ppid)
                && p.pid != root
                && !out.iter().any(|q: &Proc| q.pid == p.pid)
            {
                next.push(p.clone());
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next.iter().map(|p| p.pid).collect();
        out.extend(next);
    }
    out
}

/// 整棵树(含 root 自己)里的 exe 名,大小写不敏感去重、按发现顺序。
///
/// 含 root 是 wezterm 的口径:它拿 `flatten_to_exe_names()` 的整个集合去对比
/// 白名单,shell 自己也在集合里 —— 所以 `shell = "wsl.exe"` 而名单里没有
/// `wsl.exe` 时,关窗口会一直问。
fn tree_names(all: &[Proc], root: u32) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |name: &str| {
        if !out.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            out.push(name.to_string());
        }
    };
    if let Some(p) = all.iter().find(|p| p.pid == root) {
        push(&p.name);
    }
    for p in descendants(all, root) {
        push(&p.name);
    }
    out
}

fn stateful_in(all: &[Proc], roots: &[u32], skip: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for &root in roots {
        for name in tree_names(all, root) {
            if skip.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
                continue;
            }
            if !out.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
                out.push(name);
            }
        }
    }
    out
}

/// wezterm 口径的判定:`roots` 每棵树的进程名里,凡是不在 `skip` 里的都算
/// "有状态"。返回 `Some(空表)` = 干净、可以直接关;`None` = 进程表拿不到,
/// 调用方应当按"有状态"处理。
///
/// `roots` 为空(没有活着的会话、mux 客户端拿不到 pid)时返回 `Some(空表)`:
/// 没有东西可查的时候不拦人 —— 对应 wezterm 里 RemotePane 用 trait 默认实现
/// 直接返回"可以关"。
pub fn stateful_programs(roots: &[u32], skip: &[String]) -> Option<Vec<String>> {
    let roots: Vec<u32> = roots.iter().copied().filter(|&p| p != 0).collect();
    if roots.is_empty() {
        return Some(Vec::new());
    }
    let all = snapshot()?;
    Some(stateful_in(&all, &roots, skip))
}

/// 每个会话"当前最前面"的程序名:进程树里往下走得最深的那个。
///
/// 状态栏的 `pane:get_foreground_process_name()` 要的就是它 —— shell 自己算
/// 兜底,`vim` / `python` 这类真正在跑的程序盖在它上面。整批一起算,只做一次
/// 进程快照(状态栏每秒刷一次,每个标签各查一遍纯属浪费)。
pub fn foreground_names(roots: &[u32]) -> Vec<Option<String>> {
    foreground_names_in(&snapshot().unwrap_or_default(), roots)
}

fn foreground_names_in(all: &[Proc], roots: &[u32]) -> Vec<Option<String>> {
    roots
        .iter()
        .map(|&root| {
            let mut best = all.iter().find(|p| p.pid == root).map(|p| p.name.clone());
            let mut cursor = root;
            // 一层层下潜:有子进程就继续走,走到叶子为止
            for _ in 0..32 {
                let Some(child) = all
                    .iter()
                    .find(|p| p.ppid == cursor && p.pid != cursor && !is_console_host(&p.name))
                else {
                    break;
                };
                cursor = child.pid;
                best = Some(child.name.clone());
            }
            best
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 进程的当前工作目录
// ---------------------------------------------------------------------------

/// 删掉路径末尾多余的分隔符(`F:\a\` → `F:\a`),但盘符根目录保留(`C:\`)。
#[cfg_attr(not(windows), allow(dead_code))]
fn trim_dir_sep(mut s: String) -> String {
    while s.len() > 1 && (s.ends_with('\\') || s.ends_with('/')) {
        // `C:\` / `C:/` 这种盘符根目录不能砍成 `C:`
        let bytes = s.as_bytes();
        if bytes.len() == 3 && bytes[1] == b':' {
            break;
        }
        s.pop();
    }
    s
}

/// 某个进程自己的工作目录(Windows:PEB → `RTL_USER_PROCESS_PARAMETERS.CurrentDirectory`)。
///
/// 这是"shell 自己 `cd` 过之后能读到的东西",对 `cmd.exe` 完全可靠
/// (`cd` / `cd /d` 都会走 `SetCurrentDirectoryW`,PEB 跟着更新)。
///
/// **PowerShell 不行**:`Set-Location` 只改 PowerShell 自己的位置,不动进程 cwd,
/// 读出来永远是启动时的那个目录 —— 所以 PowerShell 走 prompt 钩子上报 OSC 7,
/// 调用方要按 shell 名字决定用不用这个函数(见 `main::pane_cwd_of`)。
///
/// 读不到(权限、位数不匹配、进程已退出)一律返回 `None`,调用方自己兜底。
#[cfg(windows)]
pub fn cwd_of(pid: u32) -> Option<std::path::PathBuf> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows_sys::Win32::System::Threading::OpenProcess;

    // PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ
    const ACCESS: u32 = 0x1000 | 0x0010;
    // ProcessBasicInformation
    const PROCESS_BASIC_INFORMATION: u32 = 0;

    #[repr(C)]
    struct ProcessBasicInfo {
        reserved1: *mut core::ffi::c_void,
        peb: *mut core::ffi::c_void,
        reserved2: [*mut core::ffi::c_void; 2],
        unique_pid: usize,
        reserved3: *mut core::ffi::c_void,
    }

    type NtQueryInfo = unsafe extern "system" fn(
        *mut core::ffi::c_void,
        u32,
        *mut core::ffi::c_void,
        u32,
        *mut u32,
    ) -> i32;

    unsafe fn read(handle: *mut core::ffi::c_void, addr: *const u8, buf: &mut [u8]) -> bool {
        use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
        let mut got: usize = 0;
        ReadProcessMemory(
            handle,
            addr as *const core::ffi::c_void,
            buf.as_mut_ptr() as *mut core::ffi::c_void,
            buf.len(),
            &mut got,
        ) != 0
            && got == buf.len()
    }

    if pid == 0 {
        return None;
    }
    unsafe {
        let handle = OpenProcess(ACCESS, 0, pid);
        if handle.is_null() {
            return None;
        }
        let out = (|| -> Option<std::path::PathBuf> {
            // ntdll 里的 NtQueryInformationProcess:动态取地址,不额外链 ntdll.lib
            let name: Vec<u16> = "ntdll.dll\0".encode_utf16().collect();
            let ntdll = GetModuleHandleW(name.as_ptr());
            if ntdll.is_null() {
                return None;
            }
            let sym = b"NtQueryInformationProcess\0";
            let proc = GetProcAddress(ntdll, sym.as_ptr())?;
            let query: NtQueryInfo = std::mem::transmute(proc);

            let mut pbi: ProcessBasicInfo = std::mem::zeroed();
            let mut ret: u32 = 0;
            let st = query(
                handle,
                PROCESS_BASIC_INFORMATION,
                &mut pbi as *mut ProcessBasicInfo as *mut core::ffi::c_void,
                std::mem::size_of::<ProcessBasicInfo>() as u32,
                &mut ret,
            );
            if st != 0 || pbi.peb.is_null() {
                return None;
            }

            // x64 与 x86 的 PEB / RTL_USER_PROCESS_PARAMETERS 布局不同:
            // ProcessParameters 在 Peb+0x20(x64)/+0x10(x86),
            // CurrentDirectory 在 ProcessParameters+0x38(x64)/+0x24(x86)。
            // 位数不一致(32 位进程读 64 位目标)时这里读不到,直接返回 None。
            let (peb_off, curdir_off) = if cfg!(target_pointer_width = "64") {
                (0x20usize, 0x38usize)
            } else {
                (0x10usize, 0x24usize)
            };
            let ptr_len = core::mem::size_of::<usize>();
            let mut raw = [0u8; 8];
            if !read(handle, (pbi.peb as usize + peb_off) as *const u8, &mut raw[..ptr_len]) {
                return None;
            }
            let params = usize::from_le_bytes(raw[..ptr_len].try_into().ok()?);
            if params == 0 {
                return None;
            }

            // CURDIR { UNICODE_STRING DosPath; HANDLE Handle; }
            // UNICODE_STRING = { u16 Length; u16 MaximumLength; pad; *mut u16 Buffer }
            let mut cur = [0u8; 16];
            if !read(
                handle,
                (params + curdir_off) as *const u8,
                &mut cur[..ptr_len + 8],
            ) {
                return None;
            }
            let len = u16::from_le_bytes([cur[0], cur[1]]) as usize;
            let buf_at = usize::from_le_bytes(cur[8..8 + ptr_len].try_into().ok()?);
            if len == 0 || len > 4096 || buf_at == 0 {
                return None;
            }
            let mut utf16 = vec![0u8; len];
            if !read(handle, buf_at as *const u8, &mut utf16) {
                return None;
            }
            let units: Vec<u16> = utf16
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            let path = String::from_utf16_lossy(&units);
            let path = trim_dir_sep(path.trim().to_string());
            if path.is_empty() {
                None
            } else {
                Some(std::path::PathBuf::from(path))
            }
        })();
        CloseHandle(handle);
        out
    }
}

#[cfg(not(windows))]
pub fn cwd_of(_pid: u32) -> Option<std::path::PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_dir_sep_keeps_drive_root() {
        assert_eq!(trim_dir_sep(r"C:\Program Files\".to_string()), r"C:\Program Files");
        assert_eq!(trim_dir_sep(r"C:\".to_string()), r"C:\");
        assert_eq!(trim_dir_sep("C:/".to_string()), "C:/");
        assert_eq!(trim_dir_sep("C:/x//".to_string()), "C:/x");
    }

    #[test]
    fn cwd_of_reports_own_directory_for_child() {
        // 起一个真的 cmd.exe 并让它停住,再读它的 cwd —— 应该就是我们给的目录
        #[cfg(windows)]
        {
            let dir = std::env::temp_dir();
            let mut child = std::process::Command::new("cmd.exe")
                .args(["/c", "ping -n 6 127.0.0.1 >nul"])
                .current_dir(&dir)
                .spawn()
                .expect("起 cmd.exe");
            let mut got = None;
            for _ in 0..40 {
                std::thread::sleep(std::time::Duration::from_millis(50));
                if let Some(p) = cwd_of(child.id()) {
                    got = Some(p);
                    break;
                }
            }
            let _ = child.kill();
            let _ = child.wait();
            let got = got.expect("应该读到 cmd.exe 的 cwd");
            let want = std::fs::canonicalize(&dir).unwrap_or(dir);
            let got_norm = std::fs::canonicalize(&got).unwrap_or(got);
            assert_eq!(
                got_norm.to_string_lossy().to_lowercase(),
                want.to_string_lossy().to_lowercase()
            );
        }
    }

    fn tree() -> Vec<Proc> {
        vec![
            Proc {
                pid: 10,
                ppid: 1,
                name: "pwsh.exe".into(),
            },
            Proc {
                pid: 11,
                ppid: 10,
                name: "python.exe".into(),
            },
            Proc {
                pid: 12,
                ppid: 11,
                name: "conhost.exe".into(),
            },
            Proc {
                pid: 13,
                ppid: 10,
                name: "conhost.exe".into(),
            },
            Proc {
                pid: 20,
                ppid: 1,
                name: "notepad.exe".into(),
            },
        ]
    }

    #[test]
    fn descendants_walk_the_whole_subtree() {
        let found: Vec<u32> = descendants(&tree(), 10).iter().map(|p| p.pid).collect();
        assert!(found.contains(&11));
        assert!(found.contains(&12), "孙进程也要算进来");
        assert!(found.contains(&13));
        assert!(!found.contains(&10), "不含自己");
        assert!(!found.contains(&20), "不属于这棵树");
    }

    #[test]
    fn console_host_does_not_count_as_a_program() {
        // 只有宿主进程 = 没有用户程序
        let t = vec![
            Proc {
                pid: 10,
                ppid: 1,
                name: "pwsh.exe".into(),
            },
            Proc {
                pid: 11,
                ppid: 10,
                name: "conhost.exe".into(),
            },
        ];
        let real: Vec<String> = descendants(&t, 10)
            .into_iter()
            .filter(|p| !is_console_host(&p.name))
            .map(|p| p.name)
            .collect();
        assert!(real.is_empty());
    }

    #[test]
    fn cycles_do_not_hang() {
        // pid 复用造成的假环:11 的父进程指向 12,12 的父进程指向 11
        let t = vec![
            Proc {
                pid: 10,
                ppid: 1,
                name: "pwsh.exe".into(),
            },
            Proc {
                pid: 11,
                ppid: 10,
                name: "a.exe".into(),
            },
            Proc {
                pid: 12,
                ppid: 11,
                name: "b.exe".into(),
            },
            Proc {
                pid: 11,
                ppid: 12,
                name: "a.exe".into(),
            },
        ];
        let found = descendants(&t, 10);
        assert!(found.len() <= 4);
    }

    fn skip_list() -> Vec<String> {
        crate::config::DEFAULT_SKIP_CLOSE_CONFIRMATION
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    /// wezterm 的白名单口径:只剩 shell 自己 → 可以直接关;多出个 python → 要问。
    #[test]
    fn whitelist_decides_whether_to_prompt() {
        let skip = skip_list();
        // 光一个 pwsh,名单里有,不拦
        let only_shell = vec![Proc {
            pid: 10,
            ppid: 1,
            name: "pwsh.exe".into(),
        }];
        assert!(stateful_in(&only_shell, &[10], &skip).is_empty());

        // 里面跑着 python.exe —— 名字不在名单里,拦
        assert_eq!(
            stateful_in(&tree(), &[10], &skip),
            vec!["python.exe".to_string()]
        );
    }

    /// 名单里的程序不拦:在 pwsh 里敲 cmd(或嵌套一个 powershell)不该弹框。
    #[test]
    fn whitelisted_children_do_not_count() {
        let skip = skip_list();
        let t = vec![
            Proc {
                pid: 10,
                ppid: 1,
                name: "pwsh.exe".into(),
            },
            Proc {
                pid: 11,
                ppid: 10,
                name: "cmd.exe".into(),
            },
            Proc {
                pid: 12,
                ppid: 11,
                name: "PING.EXE".into(),
            },
        ];
        // 名字比较不分大小写,`PING.EXE` 照样要拦
        assert_eq!(stateful_in(&t, &[10], &skip), vec!["PING.EXE".to_string()]);
        // 把 ping 也加进名单,就安静了
        let mut wider = skip.clone();
        wider.push("ping.exe".into());
        assert!(stateful_in(&t, &[10], &wider).is_empty());
    }

    /// 名单为空 = 连 shell 自己都算"有状态"(wezterm 里也一样,只是没人这么配)
    #[test]
    fn empty_shell_means_always_prompt() {
        let t = vec![Proc {
            pid: 10,
            ppid: 1,
            name: "pwsh.exe".into(),
        }];
        assert_eq!(stateful_in(&t, &[10], &[]), vec!["pwsh.exe".to_string()]);
    }

    /// 最深的那个才是"前台":pwsh → node → conhost
    #[test]
    fn deepest_child_wins() {
        let t = vec![
            Proc {
                pid: 10,
                ppid: 1,
                name: "pwsh.exe".into(),
            },
            Proc {
                pid: 11,
                ppid: 10,
                name: "node.exe".into(),
            },
            Proc {
                pid: 12,
                ppid: 11,
                name: "conhost.exe".into(),
            },
        ];
        assert_eq!(
            foreground_names_in(&t, &[10])[0].as_deref(),
            Some("node.exe"),
            "宿主进程不算前台"
        );
        // 没有子进程时就是 shell 自己
        assert_eq!(
            foreground_names_in(&t, &[20])[0],
            None,
            "不在快照里的 pid 返回 None"
        );
    }
}
