//! 键盘事件 → 终端输入
//!
//! 编码工作交给 wezterm-term(内部走 termwiz 的 `KeyCode::encode`),
//! 因为几乎所有"按键序列差异"本质上都是 VT 模式差异:
//! - DECCKM(`?1h`)应用光标模式 → 方向键从 `CSI A` 变 `SS3 A`
//! - LNM(`20h`)换行模式 → Enter 发 `CR LF`
//! - modifyOtherKeys(`?x;2h`)/ CSI-u → 带修饰键的字符编码
//! - F1-F4 用 SS3、F5+ 用 `CSI 15~`,以及 keypad 等
//!
//! 本地只保留三件"非 VT"的事:纯文本直写(鼠标/IME/布局相关)、
//! 粘贴快捷键、Lua 键位匹配。

use wezterm_term::{KeyCode, KeyModifiers};
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, KeyCode as WinitKeyCode, ModifiersState, NamedKey, PhysicalKey};

pub enum KeyAction {
    /// 交给 `Terminal::key_down` 按当前模式编码
    Key(KeyCode, KeyModifiers),
    /// 原样写入 PTY 的字节(普通文本、IME 提交)
    Bytes(Vec<u8>),
    Paste,
    None,
}

pub fn translate(ev: &KeyEvent, m: ModifiersState) -> KeyAction {
    if ev.state != ElementState::Pressed {
        return KeyAction::None;
    }
    let mods = key_mods(m);
    let ctrl = m.control_key();
    let alt = m.alt_key();

    // Super(Win)组合键归操作系统;程序不去抢
    if m.super_key() {
        return KeyAction::None;
    }

    match &ev.logical_key {
        Key::Named(n) => {
            // 单独按修饰键不产生任何字节
            if matches!(
                n,
                NamedKey::Shift
                    | NamedKey::Control
                    | NamedKey::Alt
                    | NamedKey::Super
                    | NamedKey::Meta
                    | NamedKey::CapsLock
                    | NamedKey::NumLock
                    | NamedKey::ScrollLock
            ) {
                return KeyAction::None;
            }
            match named_key_code(n) {
                Some(code) => KeyAction::Key(code, mods),
                None => KeyAction::None,
            }
        }
        Key::Character(s) => {
            let Some(ch) = s.chars().next() else {
                return KeyAction::None;
            };
            // Ctrl+V:本程序自己处理粘贴(Windows 习惯)。
            // Ctrl+Shift+V 交给 main 统一裁决:默认粘贴,也可在 Lua 里把
            // selection.ctrl_shift_v 设成 "copy" 让它变成复制选区。
            if ctrl && !alt && !m.shift_key() && (ch == 'v' || ch == 'V') {
                return KeyAction::Paste;
            }

            // AltGr 在 Windows 上是 Ctrl+Alt,同时会给出可打印字符。
            // 这类组合应当作文本直写,而不是拆成 ESC+控制字节。
            let altgr = ctrl && alt && !ch.is_ascii_alphabetic();
            let plain = mods.is_empty() || mods == KeyModifiers::SHIFT;
            if plain || altgr {
                if let Some(text) = ev.text.as_deref() {
                    if !text.is_empty() && !text.chars().all(char::is_control) {
                        return KeyAction::Bytes(text.as_bytes().to_vec());
                    }
                }
            }
            KeyAction::Key(KeyCode::Char(ch), mods)
        }
        _ => KeyAction::None,
    }
}

fn key_mods(m: ModifiersState) -> KeyModifiers {
    let mut km = KeyModifiers::NONE;
    if m.shift_key() {
        km |= KeyModifiers::SHIFT;
    }
    if m.alt_key() {
        km |= KeyModifiers::ALT;
    }
    if m.control_key() {
        km |= KeyModifiers::CTRL;
    }
    if m.super_key() {
        km |= KeyModifiers::SUPER;
    }
    km
}

/// winit 命名键 → termwiz KeyCode;未知键返回 None(不发送任何字节)
fn named_key_code(n: &NamedKey) -> Option<KeyCode> {
    Some(match n {
        NamedKey::Enter => KeyCode::Enter,
        NamedKey::Escape => KeyCode::Escape,
        NamedKey::Tab => KeyCode::Tab,
        NamedKey::Space => KeyCode::Char(' '),
        NamedKey::Backspace => KeyCode::Backspace,
        NamedKey::Delete => KeyCode::Delete,
        NamedKey::Insert => KeyCode::Insert,
        NamedKey::Home => KeyCode::Home,
        NamedKey::End => KeyCode::End,
        NamedKey::PageUp => KeyCode::PageUp,
        NamedKey::PageDown => KeyCode::PageDown,
        NamedKey::ArrowUp => KeyCode::UpArrow,
        NamedKey::ArrowDown => KeyCode::DownArrow,
        NamedKey::ArrowLeft => KeyCode::LeftArrow,
        NamedKey::ArrowRight => KeyCode::RightArrow,
        NamedKey::Pause => KeyCode::Pause,
        NamedKey::Print => KeyCode::Print,
        NamedKey::PrintScreen => KeyCode::PrintScreen,
        NamedKey::F1 => KeyCode::Function(1),
        NamedKey::F2 => KeyCode::Function(2),
        NamedKey::F3 => KeyCode::Function(3),
        NamedKey::F4 => KeyCode::Function(4),
        NamedKey::F5 => KeyCode::Function(5),
        NamedKey::F6 => KeyCode::Function(6),
        NamedKey::F7 => KeyCode::Function(7),
        NamedKey::F8 => KeyCode::Function(8),
        NamedKey::F9 => KeyCode::Function(9),
        NamedKey::F10 => KeyCode::Function(10),
        NamedKey::F11 => KeyCode::Function(11),
        NamedKey::F12 => KeyCode::Function(12),
        NamedKey::F13 => KeyCode::Function(13),
        NamedKey::F14 => KeyCode::Function(14),
        NamedKey::F15 => KeyCode::Function(15),
        NamedKey::F16 => KeyCode::Function(16),
        NamedKey::F17 => KeyCode::Function(17),
        NamedKey::F18 => KeyCode::Function(18),
        NamedKey::F19 => KeyCode::Function(19),
        NamedKey::F20 => KeyCode::Function(20),
        NamedKey::F21 => KeyCode::Function(21),
        NamedKey::F22 => KeyCode::Function(22),
        NamedKey::F23 => KeyCode::Function(23),
        NamedKey::F24 => KeyCode::Function(24),
        _ => return None,
    })
}

/// 生成规范化键位名(与 config.lua 的 `keys` 表对应),例如 `ctrl+shift+t`、`f12`、`alt+enter`。
/// 修饰键固定顺序:ctrl → alt → shift → super(与 `script::normalize_combo` 的排序一致)。
///
/// Windows 上有个坑:`Ctrl+Alt` 会被当成 AltGr,winit 查字符时不会剥掉 Ctrl,
/// `ToUnicode` 于是给出控制字符(如 `\x1c`)或直接 `Unidentified`。这样
/// `ctrl+shift+alt+"` 之类的键名根本生成不出来,`keys` 里写了也命中不了。
/// 所以 logical_key 不可用时改用物理键反推,布局无关且一定能得到键名。
pub fn key_combo(ev: &KeyEvent, m: ModifiersState) -> Option<String> {
    combo_name(&ev.logical_key, &ev.physical_key, m)
}

fn combo_name(logical: &Key, physical: &PhysicalKey, m: ModifiersState) -> Option<String> {
    let physical = || physical_key_name(physical, m.shift_key());
    let key = match logical {
        Key::Character(s) => match s.chars().next() {
            Some(c) if !c.is_control() => c.to_lowercase().to_string(),
            _ => physical()?,
        },
        Key::Named(n) => named_key_name(n)?.to_string(),
        // Unidentified(NativeKey) —— 典型场景就是 Ctrl+Alt+Shift 组合
        _ => physical()?,
    };
    let mut parts: Vec<&str> = Vec::with_capacity(5);
    if m.control_key() {
        parts.push("ctrl");
    }
    if m.alt_key() {
        parts.push("alt");
    }
    if m.shift_key() {
        parts.push("shift");
    }
    if m.super_key() {
        parts.push("super");
    }
    parts.push(key.as_str());
    Some(parts.join("+"))
}

/// 物理键 → 键名。`shift` 决定取上档字符(`'` → `"`、`5` → `%`)。
/// 只在 logical_key 靠不住时调用,正常输入仍走系统的字符映射。
fn physical_key_name(pk: &PhysicalKey, shift: bool) -> Option<String> {
    use WinitKeyCode as C;

    let PhysicalKey::Code(code) = pk else {
        return None;
    };
    // 字母键:`format!("{code:?}")` 得到 "KeyA"/"KeyZ",取末位即可
    let dbg = format!("{code:?}");
    if let Some(letter) = dbg.strip_prefix("Key") {
        if letter.len() == 1 {
            return Some(letter.to_ascii_lowercase());
        }
    }
    let (plain, shifted) = match code {
        C::Digit0 => ("0", ")"),
        C::Digit1 => ("1", "!"),
        C::Digit2 => ("2", "@"),
        C::Digit3 => ("3", "#"),
        C::Digit4 => ("4", "$"),
        C::Digit5 => ("5", "%"),
        C::Digit6 => ("6", "^"),
        C::Digit7 => ("7", "&"),
        C::Digit8 => ("8", "*"),
        C::Digit9 => ("9", "("),
        C::Quote => ("'", "\""),
        C::Backquote => ("`", "~"),
        C::Minus => ("-", "_"),
        C::Equal => ("=", "+"),
        C::BracketLeft => ("[", "{"),
        C::BracketRight => ("]", "}"),
        C::Backslash => ("\\", "|"),
        C::Semicolon => (";", ":"),
        C::Comma => (",", "<"),
        C::Period => (".", ">"),
        C::Slash => ("/", "?"),
        C::Space => ("space", "space"),
        C::Enter => ("enter", "enter"),
        C::Escape => ("escape", "escape"),
        C::Tab => ("tab", "tab"),
        C::Backspace => ("backspace", "backspace"),
        C::Delete => ("delete", "delete"),
        C::Insert => ("insert", "insert"),
        C::Home => ("home", "home"),
        C::End => ("end", "end"),
        C::PageUp => ("pageup", "pageup"),
        C::PageDown => ("pagedown", "pagedown"),
        C::ArrowUp => ("up", "up"),
        C::ArrowDown => ("down", "down"),
        C::ArrowLeft => ("left", "left"),
        C::ArrowRight => ("right", "right"),
        _ => return None,
    };
    Some(if shift { shifted } else { plain }.to_string())
}

fn named_key_name(n: &NamedKey) -> Option<&'static str> {
    Some(match n {
        NamedKey::Enter => "enter",
        NamedKey::Escape => "escape",
        NamedKey::Tab => "tab",
        NamedKey::Space => "space",
        NamedKey::Backspace => "backspace",
        NamedKey::Delete => "delete",
        NamedKey::Insert => "insert",
        NamedKey::ArrowUp => "up",
        NamedKey::ArrowDown => "down",
        NamedKey::ArrowLeft => "left",
        NamedKey::ArrowRight => "right",
        NamedKey::Home => "home",
        NamedKey::End => "end",
        NamedKey::PageUp => "pageup",
        NamedKey::PageDown => "pagedown",
        NamedKey::F1 => "f1",
        NamedKey::F2 => "f2",
        NamedKey::F3 => "f3",
        NamedKey::F4 => "f4",
        NamedKey::F5 => "f5",
        NamedKey::F6 => "f6",
        NamedKey::F7 => "f7",
        NamedKey::F8 => "f8",
        NamedKey::F9 => "f9",
        NamedKey::F10 => "f10",
        NamedKey::F11 => "f11",
        NamedKey::F12 => "f12",
        NamedKey::F13 => "f13",
        NamedKey::F14 => "f14",
        NamedKey::F15 => "f15",
        NamedKey::F16 => "f16",
        NamedKey::F17 => "f17",
        NamedKey::F18 => "f18",
        NamedKey::F19 => "f19",
        NamedKey::F20 => "f20",
        NamedKey::F21 => "f21",
        NamedKey::F22 => "f22",
        NamedKey::F23 => "f23",
        NamedKey::F24 => "f24",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::NativeKey;

    fn pk(code: WinitKeyCode) -> PhysicalKey {
        PhysicalKey::Code(code)
    }

    #[test]
    fn combo_normal_uses_logical_key() {
        let m = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert_eq!(
            combo_name(&Key::Character("T".into()), &pk(WinitKeyCode::KeyT), m),
            Some("ctrl+shift+t".into())
        );
        assert_eq!(
            combo_name(&Key::Named(NamedKey::ArrowLeft), &pk(WinitKeyCode::ArrowLeft), m),
            Some("ctrl+shift+left".into())
        );
    }

    #[test]
    fn combo_ctrl_alt_shift_falls_back_to_physical_key() {
        // Windows 上 Ctrl+Alt 被当成 AltGr,logical_key 拿不到字符。
        // 这时必须靠物理键反推,否则 keys 里写的 ctrl+shift+alt+" 永远命中不了。
        let m = ModifiersState::CONTROL | ModifiersState::ALT | ModifiersState::SHIFT;
        let unidentified = Key::Unidentified(NativeKey::Unidentified);
        assert_eq!(
            combo_name(&unidentified, &pk(WinitKeyCode::Quote), m),
            Some("ctrl+alt+shift+\"".into())
        );
        assert_eq!(
            combo_name(&unidentified, &pk(WinitKeyCode::Digit5), m),
            Some("ctrl+alt+shift+%".into())
        );
        assert_eq!(
            combo_name(&unidentified, &pk(WinitKeyCode::KeyV), m),
            Some("ctrl+alt+shift+v".into())
        );
        // ToUnicode 有时给的是控制字符,同样要兜底
        assert_eq!(
            combo_name(&Key::Character("\u{1c}".into()), &pk(WinitKeyCode::Quote), m),
            Some("ctrl+alt+shift+\"".into())
        );
    }

    #[test]
    fn physical_shift_chars() {
        assert_eq!(physical_key_name(&pk(WinitKeyCode::Quote), true), Some("\"".into()));
        assert_eq!(physical_key_name(&pk(WinitKeyCode::Digit5), true), Some("%".into()));
        assert_eq!(physical_key_name(&pk(WinitKeyCode::Quote), false), Some("'".into()));
    }
}
