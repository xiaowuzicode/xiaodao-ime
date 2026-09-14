//! 平台抽象层：系统 API 唯一出口。核心层其它模块只能通过 [`Platform`] trait 触达系统。
//!
//! 实现：`mac.rs`（AppKit/Quartz/IOKit/afplay）、`win.rs`（Win32）。
//! 剪贴板与按键注入两端都走 `arboard` / `enigo`，放在 `common.rs` 共用；
//! 平台文件只放真正不同的部分（提示音、前台应用、权限）。

use crate::types::{FrontmostApp, Permissions, PrivacySection, SoundEvent};
use std::sync::Arc;

pub trait Platform: Send + Sync {
    // ---- 剪贴板 ----
    /// 读文本剪贴板；无文本内容返回 `None`。
    fn read_clipboard(&self) -> Option<String>;
    fn write_clipboard(&self, text: &str) -> anyhow::Result<()>;
    fn clear_clipboard(&self) -> anyhow::Result<()>;

    // ---- 按键注入 ----
    /// 模拟「复制」快捷键（macOS Cmd+C / Windows Ctrl+C）。
    fn send_copy(&self) -> anyhow::Result<()>;
    /// 模拟「粘贴」快捷键（macOS Cmd+V / Windows Ctrl+V）。
    fn send_paste(&self) -> anyhow::Result<()>;

    // ---- 反馈 ----
    /// 播放系统提示音，异步不阻塞，失败静默。
    fn play_sound(&self, event: SoundEvent);

    // ---- 环境 ----
    fn frontmost_app(&self) -> FrontmostApp;
    /// `prompt=true` 时对缺失项触发系统授权弹窗（macOS）。
    fn check_permissions(&self, prompt: bool) -> Permissions;
    fn open_privacy_settings(&self, section: PrivacySection);
}

pub type SharedPlatform = Arc<dyn Platform>;

pub mod common;

#[cfg(target_os = "macos")]
pub mod mac;
#[cfg(target_os = "windows")]
pub mod win;

/// 当前平台的后端实例。
pub fn native() -> SharedPlatform {
    #[cfg(target_os = "macos")]
    {
        Arc::new(mac::MacPlatform::new())
    }
    #[cfg(target_os = "windows")]
    {
        Arc::new(win::WinPlatform::new())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        compile_error!("小岛AI输入法目前支持 macOS 与 Windows；Linux 支持在路线图中")
    }
}
