//! 窗口过程钩子:在 `WM_SIZING` 阶段提前把交换链配置成「即将变成」的尺寸。
//!
//! ## 为什么要抢在窗口变大之前
//!
//! DWM 合成的是「窗口客户区」和「应用最后 present 上去的那块表面」。拖边框
//! 时系统先把窗口矩形改大,再发 `WM_SIZE`;等应用画完新尺寸的帧,present 到
//! 交换链,DWM 才停止拉伸 —— 这段空档里 DWM 会把旧表面**按比例拉伸**铺满新
//! 客户区,于是文字被横向拉长再弹回。每跨一格就来一次,肉眼就是抖。
//!
//! wgpu 的 `Surface::configure`(DXGI `ResizeBuffers`)在这台机器上要 4~5ms,
//! 这段时间正好落在「窗口已经变大、新帧还没上屏」的空档里,所以单靠
//! `WM_SIZE` 里同步重绘压不下去。
//!
//! `WM_SIZING` 是系统在**真正改窗口矩形之前**发来的,带一个可修改的目标矩形。
//! winit 自己在这个消息里做整格吸附,改完之后才应用。我们在转发给 winit 之后
//! 读取那个「最终会被采用」的矩形,把交换链提前配好:窗口尺寸变化的那一刻,
//! 交换链已经是新尺寸,`WM_SIZE` 里只剩绘制 + present(亚毫秒),拉伸窗口被压缩
//! 到一帧以内,DWM 基本抓不到。
//!
//! 万一预测失准(系统最终给了别的尺寸),`Gpu::resize` 在 `WM_SIZE` 里发现尺寸
//! 对不上会再配一次 —— 退化成改动前的行为,不会画错。
//!
//! ## 为什么可以裸指针
//!
//! 钩子只在窗口线程上、只从消息派发里被调用,而绘制过程不泵消息,两者不会
//! 重入;`GpuRenderer` 由 `Box` 持有,地址在窗口整个生命周期内稳定。
//!
//! ## 顺带负责的第二件事:把输入法藏起来的虚拟键码捞回来
//!
//! 中文输入法打开时,字母/数字键的 `wParam` 会被换成 `VK_PROCESSKEY`(0xE5)。
//! winit 拿这个值去查布局,得到的 `logical_key` 是 `Unidentified` ——
//! 所有按字符匹配的快捷键(`Ctrl+Shift+E`、`Ctrl+Shift+T`、`Ctrl+C`…)全部
//! 失配,而这些组合在输入法状态下用户根本不会想到「要先切回英文」。
//!
//! 好在同一条消息的 `lParam` 里扫描码没有被改。用 `MapVirtualKeyW` 把扫描码
//! 反推成虚拟键码,再把 `wParam` 换回去转给 winit,应用看到的输入流就和
//! 「没开输入法」一模一样 —— 不用在每个快捷键里各写一套物理键兜底。
//!
//! 边界只有一条:**只对按住 Ctrl/Alt 的组合生效**。裸按字母必须原样留给
//! 输入法去组字,换掉 `wParam` 会让拼音的按键同时被当成 ASCII 灌进终端。

use std::sync::atomic::{AtomicI32, AtomicIsize, AtomicPtr, Ordering};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, MapVirtualKeyW, MAPVK_VSC_TO_VK_EX, VK_CONTROL, VK_MENU,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, DefWindowProcW, GetClientRect, GetWindowRect, SetWindowLongPtrW, GWLP_WNDPROC,
    WM_KEYDOWN, WM_KEYUP, WM_SIZING, WM_SYSKEYDOWN, WM_SYSKEYUP, WNDPROC,
};

use crate::gpu::GpuRenderer;

/// winit 原本的窗口过程。必须原样转发,否则整个窗口会失去输入、绘制、IME。
static ORIG_PROC: AtomicIsize = AtomicIsize::new(0);
/// 当前活着的 GPU 后端(Box 里的地址,生命周期内稳定)。
static HOOK_GPU: AtomicPtr<GpuRenderer> = AtomicPtr::new(std::ptr::null_mut());
/// 外框与客户区的差值(像素)。无边框窗口是 0,但带 RESIZE 样式时由系统决定,
/// 所以在安装时量一次,之后按常量用。
static DECO_W: AtomicI32 = AtomicI32::new(0);
static DECO_H: AtomicI32 = AtomicI32::new(0);

/// 从目标窗口矩形算出客户区尺寸,并让 GPU 后端提前配好交换链。
fn preconfigure_for(rect: &RECT) {
    thread_local! {
        /// 防重入:configure 内部若泵消息又引来一条 WM_SIZING,直接跳过。
        static BUSY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    BUSY.with(|busy| {
        if busy.get() {
            return;
        }
        busy.set(true);
        let gpu = HOOK_GPU.load(Ordering::Acquire);
        if !gpu.is_null() {
            let w = (rect.right - rect.left - DECO_W.load(Ordering::Relaxed)).max(1) as u32;
            let h = (rect.bottom - rect.top - DECO_H.load(Ordering::Relaxed)).max(1) as u32;
            // SAFETY:见模块文档 —— 只在窗口线程的消息派发里使用,绘制不泵消息。
            unsafe { (*gpu).resize(w, h) };
        }
        busy.set(false);
    });
}

/// 输入法接管字母键时塞进 `wParam` 的占位值。
const VK_PROCESSKEY: u32 = 0xE5;

thread_local! {
    /// 最近一次被替换过的按下:(扫描码, 虚拟键码)。
    ///
    /// 松手时用户可能**先把 Ctrl 放了**(尤其是按 `Ctrl+Shift+E` 之后手指
    /// 松开顺序不固定),这时 `GetKeyState` 已经看不到修饰键,但同一次按键的
    /// keyup 仍要换成同一个键码,否则 down/up 在 winit 眼里是两颗不同的键。
    static LAST_SUB: std::cell::Cell<Option<(u32, u32)>> = const { std::cell::Cell::new(None) };
}

/// `lParam` 高 16 位里的扫描码(含扩展位)。`0` 表示这条消息没带扫描码。
fn scancode_of(lp: LPARAM) -> u32 {
    let lp = lp as usize;
    let scan = ((lp >> 16) & 0xFF) as u32;
    if scan == 0 {
        return 0;
    }
    let ext = ((lp >> 24) & 0x01) as u32;
    scan | (ext << 8)
}

/// 扫描码 → 虚拟键码。输入法只改 `wParam`,扫描码还是原来那颗键的。
fn vk_of_scancode(scan: u32) -> u32 {
    if scan == 0 {
        return 0;
    }
    // SAFETY:纯查询,不碰任何全局状态
    unsafe { MapVirtualKeyW(scan, MAPVK_VSC_TO_VK_EX) }
}

fn ctrl_or_alt_down() -> bool {
    // SAFETY:纯查询。GetKeyState 读的是本线程「最后派发的那条消息」时的状态,
    // 而我们正是在处理那条消息。
    unsafe { GetKeyState(VK_CONTROL as i32) < 0 || GetKeyState(VK_MENU as i32) < 0 }
}

/// 要不要把 `VK_PROCESSKEY` 换回真实键码?换的话返回新的 `wParam`。
fn fix_processkey(msg: u32, wp: WPARAM, lp: LPARAM, hwnd: HWND) -> WPARAM {
    let dbg = std::env::var_os("VECTORTERM_DEBUG_KEYS").is_some();
    if dbg && matches!(msg, WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP) {
        let (imc, open) = unsafe {
            let c = windows_sys::Win32::UI::Input::Ime::ImmGetContext(hwnd);
            let o = if c.is_null() {
                -1
            } else {
                windows_sys::Win32::UI::Input::Ime::ImmGetOpenStatus(c) as i32
            };
            (c, o)
        };
        unsafe {
            if !imc.is_null() {
                windows_sys::Win32::UI::Input::Ime::ImmReleaseContext(hwnd, imc);
            }
        }
        crate::wlog!(
            "[dbg] hook msg=0x{msg:04X} wp=0x{:02X} scan=0x{:02X} held={} ime_open={open}",
            wp as u32,
            scancode_of(lp),
            ctrl_or_alt_down() as u8
        );
    }
    if wp as u32 != VK_PROCESSKEY {
        return wp;
    }
    let down = matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN);
    let up = matches!(msg, WM_KEYUP | WM_SYSKEYUP);
    if !down && !up {
        return wp;
    }
    let scan = scancode_of(lp);
    let vk = vk_of_scancode(scan);
    if vk == 0 {
        return wp;
    }
    let held = ctrl_or_alt_down();
    if down {
        // 裸按字母留给输入法组字 —— 这一条是硬边界,见模块文档
        if !held {
            // 顺手把上一次的残留清掉:否则这颗键松手时会被误当成
            // 「刚才那个 Ctrl 组合」的 keyup
            LAST_SUB.with(|c| c.set(None));
            return wp;
        }
        LAST_SUB.with(|c| c.set(Some((scan, vk))));
    } else {
        let same_key = LAST_SUB.with(|c| matches!(c.get(), Some((s, _)) if s == scan));
        if !held && !same_key {
            return wp;
        }
        LAST_SUB.with(|c| c.set(None));
    }
    if std::env::var_os("VECTORTERM_DEBUG_KEYS").is_some() {
        crate::wlog!("[dbg] VK_PROCESSKEY 还原 vk=0x{vk:02X} scan=0x{scan:02X} {}", if down { "down" } else { "up" });
    }
    vk as WPARAM
}

unsafe extern "system" fn hook_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // 转发之前先把输入法抹掉的键码补上 —— winit 会拿它算 logical_key
    let wp = fix_processkey(msg, wp, lp, hwnd);
    let orig: isize = ORIG_PROC.load(Ordering::Acquire);
    let res = if orig == 0 {
        DefWindowProcW(hwnd, msg, wp, lp)
    } else {
        // SAFETY:orig 是 winit 装进 GWLP_WNDPROC 的原始函数指针
        let f: WNDPROC = std::mem::transmute::<isize, WNDPROC>(orig);
        CallWindowProcW(f, hwnd, msg, wp, lp)
    };

    // 先转发再读:winit 的 WM_SIZING 处理会就地改 RECT 做整格吸附,改完的才
    // 是系统真正会采用的尺寸。
    if msg == WM_SIZING && lp != 0 {
        preconfigure_for(&*(lp as *const RECT));
    }
    res
}

/// 只装「窗口过程子类化」这一层,不登记 GPU 状态。
///
/// **两个渲染后端都要装**:子类化本身不只服务交换链预配置,输入法键码还原
/// (`fix_processkey`)也挂在这条链上 —— 而发行配置大量使用 `render = "cpu"`,
/// 只在 GPU 后端装的话,软渲染用户会完全没有这个修复。
pub fn install_wndproc(hwnd: HWND) {
    if hwnd.is_null() {
        return;
    }
    unsafe {
        if ORIG_PROC.load(Ordering::Acquire) != 0 {
            return;
        }
        let prev = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, hook_proc as *const () as isize);
        ORIG_PROC.store(prev, Ordering::Release);
    }
}

/// 安装钩子并把 GPU 后端地址登记进去。`gpu` 必须是 `Box` 里的值。
pub fn install(hwnd: HWND, gpu: *mut GpuRenderer) {
    install_wndproc(hwnd);
    // 逃生开关:设了就不登记 GPU,退回「WM_SIZE 里才配交换链」的老行为,
    // 方便 A/B 对比时量化抖动到底改善了没有。子类化仍然保留 ——
    // 它同时还负责输入法键码还原。
    if std::env::var_os("VECTORTERM_NO_SIZING_HOOK").is_some() {
        crate::wlog!("[VectorTerm] 已跳过 WM_SIZING 预配置(VECTORTERM_NO_SIZING_HOOK)");
        return;
    }
    unsafe {
        let mut wr: RECT = std::mem::zeroed();
        let mut cr: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd, &mut wr) != 0 && GetClientRect(hwnd, &mut cr) != 0 {
            DECO_W.store(
                (wr.right - wr.left) - (cr.right - cr.left),
                Ordering::Relaxed,
            );
            DECO_H.store(
                (wr.bottom - wr.top) - (cr.bottom - cr.top),
                Ordering::Relaxed,
            );
        }
        HOOK_GPU.store(gpu, Ordering::Release);
    }
}

/// 诊断用:`VECTORTERM_IME_OPEN=1` 时把本线程切到简体中文(0804)并打开输入法。
///
/// 只有「输入法开着时快捷键还能不能用」这类端到端验证需要它 ——
/// 自动化探针没法真的去点任务栏的语言栏切换输入法。
///
/// 要**反复**调用:窗口拿到焦点时 winit 会重新 `set_ime_allowed(true)`,
/// 输入上下文一重建,打开状态就回到关闭,只设一次会被它悄悄清掉。
/// 所以这里每次都读一遍真实状态,关着才开。
pub fn force_ime_open(hwnd: HWND) {
    static ARMED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    static ANNOUNCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !*ARMED.get_or_init(|| std::env::var_os("VECTORTERM_IME_OPEN").is_some()) || hwnd.is_null() {
        return;
    }
    unsafe {
        use windows_sys::Win32::UI::Input::Ime::{
            ImmGetContext, ImmGetOpenStatus, ImmReleaseContext, ImmSetOpenStatus,
        };
        let himc = ImmGetContext(hwnd);
        if himc.is_null() {
            return; // 输入上下文还没建好,下一轮再试
        }
        let was_open = ImmGetOpenStatus(himc) != 0;
        if !was_open {
            ImmSetOpenStatus(himc, 1);
        }
        let open = ImmGetOpenStatus(himc) != 0;
        ImmReleaseContext(hwnd, himc);
        if !open {
            return;
        }
        if !ANNOUNCED.swap(true, Ordering::Relaxed) {
            // 0x0804 = 简体中文。`LoadKeyboardLayoutW` 只是"把它装上手",
            // 要让本线程真的用它还得 `ActivateKeyboardLayout` —— 只 Load
            // 不 Activate 的话,`ImmGetOpenStatus` 会告诉你"已打开",但按键
            // 根本不会经过输入法(探针里踩过这个坑)。
            let id: Vec<u16> = "00000804\0".encode_utf16().collect();
            let hkl = windows_sys::Win32::UI::Input::KeyboardAndMouse::LoadKeyboardLayoutW(
                id.as_ptr(),
                windows_sys::Win32::UI::Input::KeyboardAndMouse::KLF_ACTIVATE,
            );
            windows_sys::Win32::UI::Input::KeyboardAndMouse::ActivateKeyboardLayout(hkl, 0);
            crate::wlog!(
                "[VectorTerm] 已强制打开输入法(0804),当前布局 0x{:08X}",
                windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout(0) as usize
            );
        }
    }
}

/// 撤钩子。后端析构后必须调用,否则窗口销毁期间会调到悬垂指针。
pub fn uninstall(hwnd: HWND) {
    HOOK_GPU.store(std::ptr::null_mut(), Ordering::Release);
    let orig = ORIG_PROC.swap(0, Ordering::AcqRel);
    if orig != 0 && !hwnd.is_null() {
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_WNDPROC, orig);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 拼一个 `WM_KEYDOWN` 的 lParam:重复次数 1、扫描码在 16..23 位、
    /// 扩展位在 24 位。
    fn lparam(scan: u32, extended: bool) -> LPARAM {
        (1 | ((scan as usize) << 16) | ((extended as usize) << 24)) as LPARAM
    }

    #[test]
    fn scancode_round_trips_through_lparam() {
        assert_eq!(scancode_of(lparam(0x12, false)), 0x12);
        assert_eq!(scancode_of(lparam(0x1C, true)), 0x11C, "扩展键要带上 0x100");
        assert_eq!(scancode_of(1), 0, "扫描码 0 表示这条消息没带");
    }

    #[test]
    fn recovers_letter_virtual_keys() {
        // E=0x12 → VK 'E'=0x45,1=0x02 → VK '1'=0x31。
        // 这两个正是「中文输入法开着时 Ctrl+Shift+E / Ctrl+1 失效」的两颗键。
        assert_eq!(vk_of_scancode(0x12), 0x45);
        assert_eq!(vk_of_scancode(0x02), 0x31);
        assert_eq!(vk_of_scancode(0), 0, "没有扫描码就不要瞎猜");
    }

    #[test]
    fn leaves_ordinary_keys_alone() {
        // 不带 VK_PROCESSKEY 的消息一律原样透传(哪怕 Ctrl 按着)
        let wp = 0x45usize;
        assert_eq!(fix_processkey(WM_KEYDOWN, wp, lparam(0x12, false), std::ptr::null_mut()), wp);
        assert_eq!(fix_processkey(WM_KEYUP, wp, lparam(0x12, false), std::ptr::null_mut()), wp);
    }

    #[test]
    fn bare_letter_stays_with_ime() {
        // 没按 Ctrl/Alt 时不碰 —— 裸字母是输入法组字用的,换掉 wParam
        // 会让拼音的按键同时以 ASCII 灌进终端。
        let wp = VK_PROCESSKEY as WPARAM;
        assert_eq!(fix_processkey(WM_KEYDOWN, wp, lparam(0x12, false), std::ptr::null_mut()), wp);
        assert_eq!(fix_processkey(WM_SYSKEYDOWN, wp, lparam(0x12, false), std::ptr::null_mut()), wp);
        // 非按键消息不该被牵扯进来
        assert_eq!(fix_processkey(WM_SIZING, wp, lparam(0x12, false), std::ptr::null_mut()), wp);
    }

    #[test]
    fn scan_zero_is_not_guessed() {
        // 合成输入 / 某些媒体键的 lParam 扫描码是 0,这时宁可维持原样
        let wp = VK_PROCESSKEY as WPARAM;
        assert_eq!(fix_processkey(WM_KEYDOWN, wp, lparam(0, false), std::ptr::null_mut()), wp);
    }
}
