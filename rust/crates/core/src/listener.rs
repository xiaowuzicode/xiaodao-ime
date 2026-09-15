//! rdev 全局键盘钩子 → [`KeyEvent`] 映射（平台原始键码只在本文件出现）。
//!
//! # rdev 0.5.3 键映射核对结论（读 crate 源码确认，非猜测）
//!
//! macOS（`src/macos/keycodes.rs::key_from_code` + `common.rs::convert`）：
//! - 修饰键在 macOS 上走 `CGEventType::FlagsChanged` 而不是 KeyDown/KeyUp，rdev 已在
//!   `convert()` 里按 `CGEventFlags` 的增减把它翻译成 `KeyPress` / `KeyRelease`，
//!   所以 Option 这类纯修饰键热键可以正常收到按下/松开两个事件；
//! - `ALT = 58` → `Key::Alt`，即**左 Option**（macOS 虚拟键码 kVK_Option = 58）；
//! - `ALT_GR = 61` → `Key::AltGr`，即**右 Option**（kVK_RightOption = 61）——
//!   rdev 没有 `Key::AltRight`，右 Option 一律上报 `AltGr`；
//! - `META_RIGHT = 54` → `Key::MetaRight`（右 Command，kVK_RightCommand = 54）；
//! - F19 的虚拟键码是 80（0x50），**不在** rdev 的映射表里，落到兜底分支
//!   `code => Key::Unknown(code)`，因此只能按 `Key::Unknown(80)` 识别。
//!
//! Windows（`src/windows/keycodes.rs`，值即 Win32 VK 码）：
//! - `ControlRight = 163`（VK_RCONTROL）、`F8 = 119`、`F9 = 120`；
//! - `Alt = 164`（VK_LMENU，**左** Alt）、`AltGr = 165`（VK_RMENU）。Windows 上右 Alt
//!   的 VK 就是 VK_RMENU，所以「右 Alt / AltGr」在 rdev 里是同一个 `Key::AltGr`，
//!   一条分支即可；左 Alt 会聚焦窗口菜单栏，Python 版也不提供，这里归为 `Other`。

use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tracing::{error, info, warn};

use crate::hotkey::HotkeyController;
use crate::keys::{HotkeyId, KeyEvent, KeyKind};

/// macOS F19 虚拟键码（kVK_F19 = 0x50）；rdev 映射表里没有，上报为 `Key::Unknown(80)`。
#[cfg(target_os = "macos")]
const MAC_F19_KEYCODE: u32 = 80;

/// 钩子启动后等多久确认「没有立刻失败」；超时即认为事件 tap 建立成功。
const LISTEN_PROBE: Duration = Duration::from_millis(300);

/// 键盘监听线程句柄。rdev 的 `listen` 没有停止接口，线程随进程退出；
/// 句柄只用于「等线程结束」与保持所有权语义清晰。
pub struct ListenerHandle {
    handle: Option<JoinHandle<()>>,
}

impl ListenerHandle {
    /// 阻塞等待监听线程结束（正常情况下直到进程退出都不会返回）。
    pub fn join(mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// 把 rdev 原始按键映射成状态机认识的 [`KeyKind`]。
///
/// 只区分「是不是当前平台可选的热键」，其余（字母、Esc、其它修饰键）一律 `Other`——
/// 状态机靠 `Other` 实现「录音中按任意键取消」和「组合键防误触」。
pub fn map_key(key: rdev::Key) -> KeyKind {
    #[cfg(target_os = "macos")]
    {
        match key {
            rdev::Key::Alt => KeyKind::Hotkey(HotkeyId::AltL),
            rdev::Key::AltGr => KeyKind::Hotkey(HotkeyId::AltR),
            rdev::Key::MetaRight => KeyKind::Hotkey(HotkeyId::CmdR),
            rdev::Key::Unknown(MAC_F19_KEYCODE) => KeyKind::Hotkey(HotkeyId::F19),
            _ => KeyKind::Other,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        match key {
            rdev::Key::ControlRight => KeyKind::Hotkey(HotkeyId::CtrlR),
            rdev::Key::F8 => KeyKind::Hotkey(HotkeyId::F8),
            rdev::Key::F9 => KeyKind::Hotkey(HotkeyId::F9),
            // VK_RMENU：右 Alt 与 AltGr 是同一个键
            rdev::Key::AltGr => KeyKind::Hotkey(HotkeyId::AltR),
            _ => KeyKind::Other,
        }
    }
}

/// 把 rdev 事件翻译成 [`KeyEvent`]；鼠标/滚轮等非键盘事件返回 `None`。
fn map_event(event: &rdev::Event) -> Option<KeyEvent> {
    match event.event_type {
        rdev::EventType::KeyPress(key) => Some(KeyEvent {
            key: map_key(key),
            pressed: true,
        }),
        rdev::EventType::KeyRelease(key) => Some(KeyEvent {
            key: map_key(key),
            pressed: false,
        }),
        _ => None,
    }
}

/// 起一个后台线程跑 `rdev::listen`，把按键事件喂给状态机。
///
/// macOS 上 `rdev::listen` 需要「输入监听」权限，缺权限时创建事件 tap 会直接失败；
/// 这里等 [`LISTEN_PROBE`] 确认没有立刻报错再返回 `Ok`，让调用方能在启动阶段就提示授权。
/// rdev 内部自己建 CFRunLoop 并 `CFRunLoopRun()`，**不要求跑在主线程**。
pub fn spawn(controller: Arc<HotkeyController>) -> anyhow::Result<ListenerHandle> {
    let (tx, rx) = crossbeam_channel::bounded::<String>(1);
    let handle = thread::Builder::new()
        .name("xiaodao-listener".into())
        .spawn(move || {
            // 钩子回调里 panic 会穿过 C 调用边界（UB / abort），必须在边界内兜住。
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                rdev::listen(move |event| {
                    let Some(key_event) = map_event(&event) else {
                        return;
                    };
                    let hit = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        controller.on_key(key_event)
                    }));
                    if hit.is_err() {
                        error!("热键状态机处理按键时 panic，已隔离，本次事件丢弃");
                    }
                })
            }));
            let message = match result {
                Ok(Ok(())) => "rdev::listen 意外返回（钩子已失效）".to_string(),
                Ok(Err(e)) => format!("{e:?}"),
                Err(_) => "监听线程 panic".to_string(),
            };
            error!("全局键盘监听已停止：{}", message);
            let _ = tx.send(message);
        })?;

    // 立刻失败（多半是 macOS 缺「输入监听」权限）在这里被捕获并上报给调用方。
    if let Ok(message) = rx.recv_timeout(LISTEN_PROBE) {
        error!("全局键盘监听启动失败：{}。macOS 请在「系统设置 → 隐私与安全性 → 输入监听」里勾选本应用后重启。", message);
        let _ = handle.join();
        anyhow::bail!("全局键盘监听启动失败：{message}");
    }
    if handle.is_finished() {
        warn!("监听线程已退出但未上报原因");
    }
    info!("全局热键监听已启动");
    Ok(ListenerHandle {
        handle: Some(handle),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn map_key_macos() {
        // 移植自 Python `test_hotkey.py::test_macos_alt_matches`：
        // 左 Option 在系统层上报为「Alt」，必须命中 alt_l 配置，且不能撞上右 Option。
        assert_eq!(map_key(rdev::Key::Alt), KeyKind::Hotkey(HotkeyId::AltL));
        assert_ne!(map_key(rdev::Key::AltGr), KeyKind::Hotkey(HotkeyId::AltL));
        assert_eq!(map_key(rdev::Key::AltGr), KeyKind::Hotkey(HotkeyId::AltR));
        assert_eq!(
            map_key(rdev::Key::MetaRight),
            KeyKind::Hotkey(HotkeyId::CmdR)
        );
        assert_eq!(
            map_key(rdev::Key::Unknown(MAC_F19_KEYCODE)),
            KeyKind::Hotkey(HotkeyId::F19)
        );
        // 左 Command / 左 Ctrl / 普通字母都不是热键
        assert_eq!(map_key(rdev::Key::MetaLeft), KeyKind::Other);
        assert_eq!(map_key(rdev::Key::ControlLeft), KeyKind::Other);
        assert_eq!(map_key(rdev::Key::KeyC), KeyKind::Other);
        assert_eq!(map_key(rdev::Key::Escape), KeyKind::Other);
        // F19 之外的未知键码不该被当成热键
        assert_eq!(map_key(rdev::Key::Unknown(81)), KeyKind::Other);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn map_key_windows() {
        assert_eq!(
            map_key(rdev::Key::ControlRight),
            KeyKind::Hotkey(HotkeyId::CtrlR)
        );
        assert_eq!(map_key(rdev::Key::F8), KeyKind::Hotkey(HotkeyId::F8));
        assert_eq!(map_key(rdev::Key::F9), KeyKind::Hotkey(HotkeyId::F9));
        // VK_RMENU（右 Alt / AltGr）
        assert_eq!(map_key(rdev::Key::AltGr), KeyKind::Hotkey(HotkeyId::AltR));
        // VK_LMENU（左 Alt）不做热键
        assert_eq!(map_key(rdev::Key::Alt), KeyKind::Other);
        assert_eq!(map_key(rdev::Key::ControlLeft), KeyKind::Other);
        assert_eq!(map_key(rdev::Key::KeyC), KeyKind::Other);
    }

    #[test]
    fn map_event_ignores_mouse() {
        let mouse = rdev::Event {
            event_type: rdev::EventType::MouseMove { x: 1.0, y: 2.0 },
            time: std::time::SystemTime::now(),
            name: None,
        };
        assert!(map_event(&mouse).is_none());

        let press = rdev::Event {
            event_type: rdev::EventType::KeyPress(rdev::Key::KeyA),
            time: std::time::SystemTime::now(),
            name: None,
        };
        let mapped = map_event(&press).expect("按键事件应被映射");
        assert!(mapped.pressed);
        assert_eq!(mapped.key, KeyKind::Other);

        let release = rdev::Event {
            event_type: rdev::EventType::KeyRelease(rdev::Key::KeyA),
            time: std::time::SystemTime::now(),
            name: None,
        };
        assert!(!map_event(&release).expect("按键事件应被映射").pressed);
    }
}
