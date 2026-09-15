//! macOS 后端：afplay 提示音 / NSWorkspace 前台应用 / TCC 权限自检与授权跳转。
//!
//! 1:1 移植 `xiaodao_ime/platform/mac.py`（HUD 那部分不在这里：Rust 版 HUD 由 Tauri 负责）。
//! 剪贴板与按键注入走 [`super::common`]（按键注入用 CGEvent，原因见 common.rs 顶部注释）。

use std::path::Path;
use std::process::{Command, Stdio};

use objc2_app_kit::NSWorkspace;

use super::{common, Platform};
use crate::types::{FrontmostApp, Permissions, PrivacySection, SoundEvent};

// ---- 系统框架 FFI ----
//
// 全部照抄 Python 版用 ctypes 动态加载的那几个符号，改成静态 link。
// 返回值一律用 `u8`/`u32` 接：Objective-C 的 `Boolean` 是 `unsigned char`，
// 直接声明成 Rust `bool` 时若拿到 0/1 以外的值就是 UB。

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    /// `request`：1 = kIOHIDRequestTypeListenEvent；返回 0 = granted。
    fn IOHIDCheckAccess(request: u32) -> u32;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> u8;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGRequestListenEventAccess() -> u8;
    fn CGRequestPostEventAccess() -> u8;
}

/// kIOHIDRequestTypeListenEvent
const IOHID_REQUEST_LISTEN_EVENT: u32 = 1;
/// kIOHIDAccessTypeGranted
const IOHID_ACCESS_GRANTED: u32 = 0;

const SOUND_START: &str = "/System/Library/Sounds/Tink.aiff";
const SOUND_STOP: &str = "/System/Library/Sounds/Pop.aiff";
const SOUND_CANCEL: &str = "/System/Library/Sounds/Bottle.aiff";

/// 系统设置 → 隐私与安全性 的深链锚点（macOS 13+ 系统设置仍兼容此 scheme）。
fn privacy_anchor(section: PrivacySection) -> &'static str {
    match section {
        PrivacySection::InputMonitoring => "Privacy_ListenEvent",
        PrivacySection::Accessibility => "Privacy_Accessibility",
        PrivacySection::Microphone => "Privacy_Microphone",
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct MacPlatform;

impl MacPlatform {
    pub fn new() -> Self {
        MacPlatform
    }
}

impl Platform for MacPlatform {
    fn read_clipboard(&self) -> Option<String> {
        common::read_clipboard()
    }

    fn write_clipboard(&self, text: &str) -> anyhow::Result<()> {
        common::write_clipboard(text)
    }

    fn clear_clipboard(&self) -> anyhow::Result<()> {
        common::clear_clipboard()
    }

    fn send_copy(&self) -> anyhow::Result<()> {
        common::send_copy()
    }

    fn send_paste(&self) -> anyhow::Result<()> {
        common::send_paste()
    }

    fn play_sound(&self, event: SoundEvent) {
        let path = match event {
            SoundEvent::Start => SOUND_START,
            SoundEvent::Stop => SOUND_STOP,
            SoundEvent::Cancel => SOUND_CANCEL,
        };
        if !Path::new(path).exists() {
            return;
        }
        match Command::new("afplay")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            // 不等播放完（会阻塞热键线程），但要有人收尸，否则每次提示音留一个僵尸进程
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(e) => tracing::debug!("播放提示音失败：{e}"),
        }
    }

    fn frontmost_app(&self) -> FrontmostApp {
        // frontmostApplication 不要求主线程（NSWorkspace 该属性是线程安全的 KVO 属性），
        // 因此这里不需要 MainThreadMarker。
        let Some(app) = NSWorkspace::sharedWorkspace().frontmostApplication() else {
            tracing::debug!("获取前台应用失败：frontmostApplication 为空");
            return FrontmostApp::default();
        };
        FrontmostApp {
            name: app
                .localizedName()
                .map(|s| s.to_string())
                .unwrap_or_default(),
            id: app
                .bundleIdentifier()
                .map(|s| s.to_string())
                .unwrap_or_default(),
        }
    }

    /// macOS 的权限授予对象是「进程的签名身份」而非路径：
    /// - 终端启动 → 权限挂在终端 App 上；
    /// - .app 启动 → 挂在 App bundle 上；ad-hoc 签名每次重打包都会变，旧授权
    ///   静默失效（设置里开关看着还开着）——必须先移除旧条目再重新勾选。
    ///
    /// 只查不弹窗；`prompt=true` 时对缺失项触发系统弹窗，把当前宿主加进权限列表。
    fn check_permissions(&self, prompt: bool) -> Permissions {
        // 输入监听：IOHIDCheckAccess 比 CGPreflightListenEventAccess 可靠——
        // 后者在系统设置里明明已勾选时仍可能返回 false（实测误报）。
        let listen =
            unsafe { IOHIDCheckAccess(IOHID_REQUEST_LISTEN_EVENT) } == IOHID_ACCESS_GRANTED;
        // 辅助功能：AXIsProcessTrusted 是官方口径，CGPreflightPostEventAccess 有同款误报。
        let post = unsafe { AXIsProcessTrusted() } != 0;

        if prompt {
            if !listen {
                unsafe { CGRequestListenEventAccess() };
            }
            if !post {
                unsafe { CGRequestPostEventAccess() };
            }
        }

        tracing::info!(
            "权限自检：输入监听={}，辅助功能={}",
            if listen { "✅" } else { "❌ 未授权" },
            if post { "✅" } else { "❌ 未授权" }
        );
        if !listen {
            tracing::warn!(
                "【热键无响应的原因】输入监听未授权：系统设置 → 隐私与安全性 → 输入监听，\
                 勾选本程序的宿主（.app 启动就是「小岛AI输入法」，终端启动就是终端）。\
                 若列表里已有旧条目仍不生效：先用「-」移除，再重新添加勾选（重新打包后签名已变）。\
                 改完必须重启本程序。"
            );
        }
        if !post {
            tracing::warn!(
                "【出不了字的原因】辅助功能未授权：系统设置 → 隐私与安全性 → 辅助功能，\
                 同上勾选宿主并重启。"
            );
        }
        Permissions {
            input_monitoring: listen,
            accessibility: post,
        }
    }

    /// 跳转系统设置对应隐私面板，让用户手动勾选授权。
    fn open_privacy_settings(&self, section: PrivacySection) {
        let url = format!(
            "x-apple.systempreferences:com.apple.preference.security?{}",
            privacy_anchor(section)
        );
        match Command::new("open")
            .arg(&url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                tracing::info!("已打开系统设置隐私面板：{section:?}");
            }
            Err(e) => tracing::warn!("打开隐私设置失败：{e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privacy_anchor_matches_python() {
        assert_eq!(
            privacy_anchor(PrivacySection::InputMonitoring),
            "Privacy_ListenEvent"
        );
        assert_eq!(
            privacy_anchor(PrivacySection::Accessibility),
            "Privacy_Accessibility"
        );
        assert_eq!(
            privacy_anchor(PrivacySection::Microphone),
            "Privacy_Microphone"
        );
    }

    #[test]
    fn check_permissions_without_prompt_is_safe() {
        // 无论终端宿主当前是否已授权，只要不 panic 即可（CI/无 TCC 环境也能跑）
        let _ = MacPlatform::new().check_permissions(false);
    }
}
