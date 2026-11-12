//! wezterm-term 装配:终端配置实现、调色板构建、标题告警桥接

use crate::config::AppConfig;
use crate::UserEvent;
use std::io::Write;
use std::str::FromStr;
use std::sync::{Arc, Mutex as StdMutex};
use wezterm_term::color::{ColorPalette, SrgbaTuple};
use wezterm_term::{
    Alert, AlertHandler, Clipboard, ClipboardSelection, Terminal, TerminalConfiguration,
    TerminalSize,
};
use winit::event_loop::EventLoopProxy;

/// OSC 52(`ESC ] 52 ; c ; <base64> BEL`)写入系统剪贴板。
/// vim/tmux/ssh 里的 yank 就是靠它把内容送到本机剪贴板的。
#[derive(Debug, Default)]
pub struct SystemClipboard;

impl Clipboard for SystemClipboard {
    fn set_contents(
        &self,
        selection: ClipboardSelection,
        data: Option<String>,
    ) -> anyhow::Result<()> {
        // Windows 只有一个系统剪贴板;PRIMARY 选择也落到同一处
        let _ = selection;
        let Some(data) = data else {
            // OSC 52 的查询形式:让应用自己回退,不泄露剪贴板内容
            return Ok(());
        };
        set_clipboard_text(&data)
    }
}

/// 写入系统剪贴板(OSC 52 与鼠标选区复制共用)
pub fn set_clipboard_text(text: &str) -> anyhow::Result<()> {
    arboard::Clipboard::new()?.set_text(text.to_string())?;
    Ok(())
}

/// 读取系统剪贴板(粘贴快捷键与右键粘贴共用)
pub fn get_clipboard_text() -> anyhow::Result<String> {
    Ok(arboard::Clipboard::new()?.get_text()?)
}

/// wezterm-term 需要的配置注入点:调色板 + 回看行数 + 图形协议开关
#[derive(Debug)]
pub struct TermConfig {
    palette: ColorPalette,
    scrollback: usize,
    /// 是否响应 Kitty 图形协议(APC G ...)。关掉时应用会收到失败响应并自行降级。
    images: bool,
}

impl TerminalConfiguration for TermConfig {
    fn color_palette(&self) -> ColorPalette {
        self.palette.clone()
    }
    fn scrollback_size(&self) -> usize {
        self.scrollback
    }
    fn enable_kitty_graphics(&self) -> bool {
        self.images
    }
}

/// 用 Lua 配置覆盖默认调色板的对应字段。
///
/// 默认调色板本身已经是完整的 256 色表(16 基础色 + xterm 216 色立方 + 24 灰阶),
/// 真彩色(SGR 38;2;r;g;b)由 `ColorPalette::resolve_fg/bg` 直接透传,所以这里
/// 只需要处理"用户想改颜色"的部分。
pub fn build_palette(cfg: &AppConfig) -> ColorPalette {
    let mut p = ColorPalette::default();
    let set = |slot: &mut SrgbaTuple, s: &str| {
        if let Ok(c) = SrgbaTuple::from_str(s) {
            *slot = c;
        }
    };
    set(&mut p.foreground, &cfg.colors.foreground);
    set(&mut p.background, &cfg.colors.background);
    set(&mut p.cursor_fg, &cfg.colors.cursor_fg);
    set(&mut p.cursor_bg, &cfg.colors.cursor_bg);
    // colors.ansi:覆盖前 16 个调色板下标
    for (i, s) in cfg.colors.ansi.iter().enumerate().take(16) {
        set(&mut p.colors.0[i], s);
    }
    // colors.indexed:覆盖任意下标(包括 216 色立方与灰阶)
    for (i, s) in &cfg.colors.indexed {
        set(&mut p.colors.0[*i as usize], s);
    }
    p
}

/// 由配置构造 wezterm-term 所需的配置对象(热重载时可直接 set_config)
pub fn make_config(cfg: &AppConfig) -> Arc<dyn TerminalConfiguration + Send + Sync> {
    Arc::new(TermConfig {
        palette: build_palette(cfg),
        scrollback: cfg.scrollback_lines,
        images: cfg.images.enabled,
    })
}

/// 创建终端核心;Windows ConPTY 需要启用 quirk(抑制初始标题变更、
/// resize 保持滚动区不可变等,与 wezterm 一致)
pub fn create_terminal(size: TerminalSize, cfg: &AppConfig, writer: Box<dyn Write + Send>) -> Terminal {
    let config = make_config(cfg);
    let mut t = Terminal::new(size, config, "VectorTerm", env!("CARGO_PKG_VERSION"), writer);
    t.enable_conpty_quirks();
    let clipboard: Arc<dyn Clipboard> = Arc::new(SystemClipboard);
    t.set_clipboard(&clipboard);
    t
}

/// 把 "#rrggbb" 之类的颜色字符串解析为渲染用 RGB(供 trail_color 等使用)
pub fn color_from_str(s: &str) -> Option<crate::render::Rgb> {
    SrgbaTuple::from_str(s).ok().map(crate::render::rgb)
}

/// OSC 标题变化 → winit 窗口标题
pub struct TitleAlerter {
    pub proxy: StdMutex<EventLoopProxy<UserEvent>>,
    /// 标签的稳定 id:窗口线程用它把标题更新落到正确的标签上
    pub tab: u64,
}

impl AlertHandler for TitleAlerter {
    fn alert(&mut self, alert: Alert) {
        let title = match alert {
            Alert::WindowTitleChanged(t) => Some(t),
            Alert::IconTitleChanged(Some(t)) => Some(t),
            _ => None,
        };
        if let Some(t) = title {
            if let Ok(p) = self.proxy.lock() {
                let _ = p.send_event(UserEvent::TitleChanged {
                    tab: self.tab,
                    title: t,
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 单元测试:确认按键编码确实跟随 VT 模式(DECCKM / LNM)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use wezterm_term::{KeyCode, KeyModifiers};

    struct Capture(Arc<StdMutex<Vec<u8>>>);

    impl Write for Capture {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn make() -> (Terminal, Arc<StdMutex<Vec<u8>>>) {
        let buf = Arc::new(StdMutex::new(Vec::new()));
        let size = TerminalSize {
            rows: 5,
            cols: 20,
            pixel_width: 200,
            pixel_height: 100,
            dpi: 96,
        };
        let t = create_terminal(size, &AppConfig::default(), Box::new(Capture(buf.clone())));
        (t, buf)
    }

    /// 终端写入经过一条内部线程,这里轮询等待字节到齐
    fn take(buf: &Arc<StdMutex<Vec<u8>>>) -> String {
        let mut out = String::new();
        for _ in 0..200 {
            {
                let mut b = buf.lock().unwrap();
                if !b.is_empty() {
                    out = String::from_utf8_lossy(&b).to_string();
                    b.clear();
                }
            }
            if !out.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        out
    }

    #[test]
    fn decckm_switches_arrows_to_ss3() {
        let (mut t, buf) = make();
        t.key_down(KeyCode::UpArrow, KeyModifiers::NONE).unwrap();
        assert_eq!(take(&buf), "\x1b[A", "默认应发 CSI A");

        t.advance_bytes(b"\x1b[?1h"); // DECCKM 应用光标模式
        t.key_down(KeyCode::UpArrow, KeyModifiers::NONE).unwrap();
        assert_eq!(take(&buf), "\x1bOA", "DECCKM 打开后应发 SS3 A");

        t.advance_bytes(b"\x1b[?1l");
        t.key_down(KeyCode::Home, KeyModifiers::NONE).unwrap();
        assert_eq!(take(&buf), "\x1b[H");
    }

    #[test]
    fn lnm_makes_enter_send_crlf() {
        let (mut t, buf) = make();
        t.key_down(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert_eq!(take(&buf), "\r");

        t.advance_bytes(b"\x1b[20h"); // LNM 换行模式
        t.key_down(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert_eq!(take(&buf), "\r\n");
    }

    #[test]
    fn modified_keys_use_xterm_encoding() {
        let (mut t, buf) = make();
        t.key_down(KeyCode::UpArrow, KeyModifiers::CTRL).unwrap();
        assert_eq!(take(&buf), "\x1b[1;5A");
        t.key_down(KeyCode::Function(5), KeyModifiers::SHIFT).unwrap();
        assert_eq!(take(&buf), "\x1b[15;2~");
    }

    #[test]
    fn bracketed_paste_wraps_text() {
        let (mut t, buf) = make();
        t.advance_bytes(b"\x1b[?2004h");
        t.send_paste("hi\nthere").unwrap();
        assert_eq!(take(&buf), "\x1b[200~hi\nthere\x1b[201~");
    }
}
