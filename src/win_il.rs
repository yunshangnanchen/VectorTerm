//! 进程自身的完整性级别与父进程探测(仅启动日志用)。
//!
//! 存在的唯一理由:排查"终端里的 shell 写不了 %APPDATA%"。
//! 完整性级别是**继承**的——`CreateProcessW` 不带令牌参数,子进程原样拿父进程的令牌。
//! 所以终端自己被低完整性(沙箱、受控运行、AppContainer)拉起时,它启动的 pwsh 也是
//! 低完整性;而低完整性进程写不了中等完整性的 `ConsoleHost_history.txt`,PowerShell
//! 就会报 "Error reading or writing history file ... is denied"。
//! 启动时把这两条打到 stderr,一眼能分清是终端的问题还是启动方式的问题。
//!
//! 参考:`portable-pty-0.9.0/src/win/psuedocon.rs` 里的 `CreateProcessW` 调用。

/// 一个进程的完整性级别。`rid` 是 SID 的最后一段(`S-1-16-<rid>`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Integrity {
    pub rid: u32,
    pub name: &'static str,
}

impl Integrity {
    /// 低完整性及以下都写不了中等完整性的用户目录
    pub fn is_low(&self) -> bool {
        self.rid <= 0x1000
    }
}

impl std::fmt::Display for Integrity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (S-1-16-{})", self.name, self.rid)
    }
}

#[cfg(windows)]
mod imp {
    use super::Integrity;
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenIntegrityLevel, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    fn name_of(rid: u32) -> &'static str {
        match rid {
            0x0000 => "Untrusted",
            0x1000 => "Low",
            0x2000 => "Medium",
            0x3000 => "High",
            0x4000 => "System",
            _ => "Unknown",
        }
    }

    pub fn current() -> Option<Integrity> {
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return None;
            }
            let mut len = 0u32;
            GetTokenInformation(token, TokenIntegrityLevel, std::ptr::null_mut(), 0, &mut len);
            if len == 0 {
                CloseHandle(token);
                return None;
            }
            // 用 usize 数组做缓冲区:TOKEN_MANDATORY_LABEL 里有指针,需要按机器字对齐
            let mut buf = vec![0usize; (len as usize).div_ceil(std::mem::size_of::<usize>())];
            let ok = GetTokenInformation(
                token,
                TokenIntegrityLevel,
                buf.as_mut_ptr().cast(),
                len,
                &mut len,
            );
            CloseHandle(token);
            if ok == 0 {
                return None;
            }
            let label = &*buf.as_ptr().cast::<TOKEN_MANDATORY_LABEL>();
            let mut sid_text: *mut u16 = std::ptr::null_mut();
            if ConvertSidToStringSidW(label.Label.Sid, &mut sid_text) == 0 {
                return None;
            }
            let mut n = 0usize;
            while *sid_text.add(n) != 0 {
                n += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(sid_text, n));
            LocalFree(sid_text.cast());
            // "S-1-16-8192" → 8192
            let rid = text.rsplit('-').next()?.parse::<u32>().ok()?;
            Some(Integrity {
                rid,
                name: name_of(rid),
            })
        }
    }

    /// 父进程 (pid, 可执行文件名)。父进程可能已经退出,拿不到就返回 None。
    pub fn parent() -> Option<(u32, String)> {
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return None;
            }
            let me = std::process::id();
            let mut entries = Vec::new();
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut ok = Process32FirstW(snap, &mut entry);
            while ok != 0 {
                let name_len = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                entries.push((
                    entry.th32ProcessID,
                    entry.th32ParentProcessID,
                    String::from_utf16_lossy(&entry.szExeFile[..name_len]),
                ));
                ok = Process32NextW(snap, &mut entry);
            }
            CloseHandle(snap);

            let ppid = entries.iter().find(|e| e.0 == me).map(|e| e.1)?;
            let name = entries
                .iter()
                .find(|e| e.0 == ppid)
                .map(|e| e.2.clone())
                .unwrap_or_else(|| "?".to_string());
            Some((ppid, name))
        }
    }

    /// 一行摘要:`完整性级别: Medium (S-1-16-8192)  父进程: explorer.exe(33696)`
    pub fn note() -> Option<String> {
        let il = current()?;
        let mut line = format!("完整性级别: {il}");
        if let Some((pid, name)) = parent() {
            // 父进程可能在我们查快照之前就退出了,名字拿不到时只报 pid
            if name == "?" {
                line.push_str(&format!("  父进程: pid {pid}"));
            } else {
                line.push_str(&format!("  父进程: {name}({pid})"));
            }
        }
        Some(line)
    }
}

#[cfg(windows)]
pub use imp::{current, note};

#[cfg(not(windows))]
mod imp {
    use super::Integrity;
    pub fn current() -> Option<Integrity> {
        None
    }
    pub fn parent() -> Option<(u32, String)> {
        None
    }
    pub fn note() -> Option<String> {
        None
    }
}

#[cfg(not(windows))]
pub use imp::{current, note};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_own_integrity_level() {
        let il = current().expect("应能读到自身完整性级别");
        // 只在这几个已知档位之间取值
        assert!(
            matches!(il.rid, 0x0000..=0x4000),
            "意外的 RID: 0x{:X}",
            il.rid
        );
        assert_eq!(il.is_low(), il.rid <= 0x1000);
    }

    #[test]
    fn note_mentions_both_sides() {
        let line = note().expect("应能生成摘要");
        assert!(line.contains("完整性级别: "), "{line}");
        assert!(line.contains("S-1-16-"), "{line}");
    }

    #[test]
    fn parent_looks_like_a_process() {
        let (pid, name) = super::imp::parent().expect("应能读到父进程");
        assert!(pid > 0, "父 pid 应非零");
        assert!(!name.is_empty(), "父进程名不应为空");
    }
}
