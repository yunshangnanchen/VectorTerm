//! 发行版把 stdout / stderr 接到「能用」的地方。
//!
//! 正式发行版编译成 **GUI 子系统**(见 `main.rs` 顶部的
//! `windows_subsystem = "windows"`),双击 `VectorTerm.exe` 才不会先弹出一个黑色
//! 控制台窗口。代价是进程默认没有控制台,stdout / stderr 都是失效句柄:
//! 写失效句柄会失败,而 `eprintln!` 失败就是 panic,panic 又要往 stderr 写 ——
//! 表现是"启动即闪退,什么都不留"。
//!
//! `main` 的第一件事是调 [`attach`],它只负责**让句柄永远有效**:
//!
//! 1. 已经被重定向(`VectorTerm.exe > out.txt`)就不动它。
//! 2. 其余情况把 std handles 指到 `NUL`。
//!
//! 这里**刻意不** `AttachConsole(ATTACH_PARENT_PROCESS)`:软件自身的日志现在一律
//! 由 [`crate::debug_term`] 送进独立的调试终端窗口。进程一旦挂过父控制台,
//! 之后再 `AllocConsole` 开独立窗口会直接 `ERROR_ACCESS_DENIED`;而且日志混进
//! 启动它的 cmd 输出里也违背"调试输出不进当前终端"的目标。
//!
//! 调试构建是控制台子系统,控制台现成的,什么都不用做。

/// 把失效的 std handles 压到 NUL。只对发行版(GUI 子系统)有意义。
#[cfg(all(windows, not(debug_assertions), not(test)))]
pub fn attach() {
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, SetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };

    // 已经有有效句柄(被重定向到管道/文件,或本来就带着控制台)就别动它 ——
    // 强行 SetStdHandle 会把 `VectorTerm.exe > out.txt` 的输出打回控制台去。
    unsafe {
        let cur = GetStdHandle(STD_ERROR_HANDLE);
        if !cur.is_null() && cur != INVALID_HANDLE_VALUE {
            return;
        }
    }

    // NUL 句柄要活到进程结束,不关。
    unsafe fn open_device(name: &str) -> Option<HANDLE> {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let h = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        (h != INVALID_HANDLE_VALUE).then_some(h)
    }

    if let Some(h) = unsafe { open_device("NUL") } {
        unsafe {
            SetStdHandle(STD_OUTPUT_HANDLE, h);
            SetStdHandle(STD_ERROR_HANDLE, h);
        }
    }
}

/// 调试构建 / 非 Windows:控制台是现成的,什么都不用做。
#[cfg(not(all(windows, not(debug_assertions), not(test))))]
pub fn attach() {}
