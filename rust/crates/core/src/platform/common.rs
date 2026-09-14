//! 跨平台共用原语：arboard 剪贴板 + 按键注入（Cmd/Ctrl + C/V）。
//!
//! `mac.rs` / `win.rs` 的 [`Platform`](super::Platform) 实现直接转发到这里，
//! 平台文件只保留真正不同的部分（提示音、前台应用、权限）。
//!
//! ## 按键注入为什么两端不同后端
//!
//! - **Windows**：`enigo`（底层 `SendInput`），与 Python 版 `pynput` 等价。
//! - **macOS**：**不用 enigo**，改为直接 `CGEvent` + `CGEventPost(HID)`，
//!   与 Python 版 `platform/mac.py::_send_cmd_key` 逐行等价。原因见下。
//!
//! enigo 0.3 的 macOS 后端默认用 `CGEventSourceStateID::CombinedSessionState`
//! 建事件源（`Settings::independent_of_keyboard_state` 默认 false），
//! 该事件源会把**当前物理按键状态**合并进 posted event 的 flags。
//! 本程序的粘贴恰好发生在用户可能还按着热键修饰键（Option/Ctrl 等）的瞬间，
//! 合并后就变成 `Cmd+Option+V`——目标 App 收到的是另一个快捷键，粘贴静默失效。
//! Python 版是显式 `CGEventSetFlags(event, kCGEventFlagMaskCommand)`，
//! 只带 Command 一个 flag；这里照抄该做法（事件源用 `HIDSystemState`，同 Python）。

use anyhow::Result;

/// 读文本剪贴板；无文本内容（含空剪贴板）返回 `None`。
pub fn read_clipboard() -> Option<String> {
    let mut clipboard = match arboard::Clipboard::new() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("打开剪贴板失败：{e}");
            return None;
        }
    };
    match clipboard.get_text() {
        Ok(text) => Some(text),
        // 剪贴板为空或里面不是文本（图片/文件）——等同于「没有文本」，不是错误
        Err(arboard::Error::ContentNotAvailable) => None,
        Err(e) => {
            tracing::warn!("读取剪贴板失败：{e}");
            None
        }
    }
}

pub fn write_clipboard(text: &str) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new()?;
    clipboard.set_text(text.to_owned())?;
    Ok(())
}

pub fn clear_clipboard() -> Result<()> {
    let mut clipboard = arboard::Clipboard::new()?;
    clipboard.clear()?;
    Ok(())
}

/// 模拟「复制」快捷键（macOS Cmd+C / Windows Ctrl+C）。
pub fn send_copy() -> Result<()> {
    imp::send_copy()
}

/// 模拟「粘贴」快捷键（macOS Cmd+V / Windows Ctrl+V）。
pub fn send_paste() -> Result<()> {
    imp::send_paste()
}

#[cfg(target_os = "macos")]
mod imp {
    use anyhow::{anyhow, Result};
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGKeyCode};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    /// macOS 虚拟键码：字母 C（同 Python `_C_KEYCODE`）。
    const C_KEYCODE: CGKeyCode = 8;
    /// macOS 虚拟键码：字母 V（同 Python `_V_KEYCODE`）。
    const V_KEYCODE: CGKeyCode = 9;

    /// 发一次 Command + `keycode`（down/up 各一个事件，flags 只带 Command）。
    fn send_cmd_key(keycode: CGKeyCode) -> Result<()> {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|_| anyhow!("创建 CGEventSource 失败"))?;
        let key_down = CGEvent::new_keyboard_event(source.clone(), keycode, true)
            .map_err(|_| anyhow!("创建按下事件失败（辅助功能未授权？）"))?;
        let key_up = CGEvent::new_keyboard_event(source, keycode, false)
            .map_err(|_| anyhow!("创建抬起事件失败（辅助功能未授权？）"))?;
        // 关键：显式覆盖 flags，不继承用户此刻物理按着的 Option/Shift 等修饰键
        key_down.set_flags(CGEventFlags::CGEventFlagCommand);
        key_up.set_flags(CGEventFlags::CGEventFlagCommand);
        key_down.post(CGEventTapLocation::HID);
        key_up.post(CGEventTapLocation::HID);
        Ok(())
    }

    pub fn send_copy() -> Result<()> {
        send_cmd_key(C_KEYCODE)
    }

    pub fn send_paste() -> Result<()> {
        send_cmd_key(V_KEYCODE)
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use anyhow::{anyhow, Result};
    use enigo::{Direction, Enigo, Key, Keyboard, Settings};

    /// 发一次 Ctrl + `ch`。每次新建 Enigo：`Keyboard::key` 需要 `&mut self`，
    /// 且 Enigo 不是 `Sync`，复用实例反而要加锁，得不偿失。
    fn send_ctrl_key(ch: char) -> Result<()> {
        let mut enigo =
            Enigo::new(&Settings::default()).map_err(|e| anyhow!("初始化按键注入失败：{e}"))?;
        enigo
            .key(Key::Control, Direction::Press)
            .map_err(|e| anyhow!("按下 Ctrl 失败：{e}"))?;
        let clicked = enigo
            .key(Key::Unicode(ch), Direction::Click)
            .map_err(|e| anyhow!("按下 {ch} 失败：{e}"));
        // Ctrl 必须无论如何抬起，否则用户键盘会一直处于 Ctrl 按下态
        let released = enigo
            .key(Key::Control, Direction::Release)
            .map_err(|e| anyhow!("抬起 Ctrl 失败：{e}"));
        clicked?;
        released?;
        Ok(())
    }

    pub fn send_copy() -> Result<()> {
        send_ctrl_key('c')
    }

    pub fn send_paste() -> Result<()> {
        send_ctrl_key('v')
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod imp {
    use anyhow::{bail, Result};

    pub fn send_copy() -> Result<()> {
        bail!("当前平台暂不支持按键注入")
    }

    pub fn send_paste() -> Result<()> {
        bail!("当前平台暂不支持按键注入")
    }
}
