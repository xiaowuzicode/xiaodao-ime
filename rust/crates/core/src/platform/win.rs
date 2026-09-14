//! Windows 后端：winmm 提示音 / Win32 前台进程 / 权限恒通过 / ms-settings 深链。
//!
//! 1:1 移植 `xiaodao_ime/platform/win.py`（HUD 那部分不在这里：Rust 版 HUD 由 Tauri 负责）。
//! 剪贴板与按键注入走 [`super::common`]（arboard + enigo，替代 Python 版的 ctypes + pynput）。
//!
//! Windows 没有 macOS 式 TCC 权限体系：全局键盘钩子与按键注入开箱即用，
//! 仅麦克风受「设置 → 隐私 → 麦克风」控制（拿不到音频时提示用户即可）。

use std::os::windows::ffi::OsStrExt;

use windows::core::PCWSTR;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, SW_SHOWNORMAL,
};

use super::{common, Platform};
use crate::types::{FrontmostApp, Permissions, PrivacySection, SoundEvent};

/// 系统提示音别名（同 Python `_ALIASES`）。
const ALIAS_START: &str = "SystemAsterisk";
const ALIAS_STOP: &str = "SystemDefault";
const ALIAS_CANCEL: &str = "SystemHand";

/// Rust `&str` → 以 NUL 结尾的 UTF-16 缓冲（Win32 宽字符 API 都要这个）。
fn wide(text: &str) -> Vec<u16> {
    std::ffi::OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[derive(Debug, Default, Clone, Copy)]
pub struct WinPlatform;

impl WinPlatform {
    pub fn new() -> Self {
        WinPlatform
    }
}

impl Platform for WinPlatform {
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
        let alias = match event {
            SoundEvent::Start => ALIAS_START,
            SoundEvent::Stop => ALIAS_STOP,
            SoundEvent::Cancel => ALIAS_CANCEL,
        };
        let buf = wide(alias);
        // SND_ALIAS：按系统声音别名播放；SND_ASYNC：立即返回不阻塞
        let ok = unsafe { PlaySoundW(PCWSTR(buf.as_ptr()), None, SND_ALIAS | SND_ASYNC) };
        if !ok.as_bool() {
            tracing::debug!("播放提示音失败：{alias}");
        }
    }

    /// 返回 (应用名, 进程 exe 名)；app_styles 的键在 Windows 上写 exe 名即可，
    /// 如 `{"WeChat": "轻度纠错", "Code": "关闭"}`。失败返回空串。
    fn frontmost_app(&self) -> FrontmostApp {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.is_invalid() {
                return FrontmostApp::default();
            }
            let mut pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == 0 {
                return FrontmostApp::default();
            }
            let handle = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                Ok(h) => h,
                Err(e) => {
                    tracing::debug!("获取前台应用失败：OpenProcess {e}");
                    return FrontmostApp::default();
                }
            };
            let mut buf = [0u16; 1024];
            let mut size = buf.len() as u32;
            let query = QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                windows::core::PWSTR(buf.as_mut_ptr()),
                &mut size,
            );
            let _ = CloseHandle(handle);
            if query.is_err() {
                tracing::debug!("获取前台应用失败：QueryFullProcessImageNameW");
                return FrontmostApp::default();
            }
            let full = String::from_utf16_lossy(&buf[..size as usize]);
            // 取文件名并去掉扩展名（同 Python os.path.splitext）：C:\...\WeChat.exe -> WeChat
            let file_name = full.rsplit(['\\', '/']).next().unwrap_or("");
            let exe = match file_name.rfind('.') {
                Some(dot) if dot > 0 => file_name[..dot].to_owned(),
                _ => file_name.to_owned(),
            };
            FrontmostApp {
                name: exe.clone(),
                id: exe,
            }
        }
    }

    /// Windows 无 TCC：热键与粘贴无需授权。麦克风若被系统隐私设置拦截，
    /// 录音会得到全零数据，在日志里给出指引即可。
    fn check_permissions(&self, _prompt: bool) -> Permissions {
        tracing::info!(
            "权限自检：Windows 平台无需输入监听/辅助功能授权；\
             若录不到声音，请检查 设置 → 隐私和安全性 → 麦克风 → 允许桌面应用访问"
        );
        Permissions {
            input_monitoring: true,
            accessibility: true,
        }
    }

    /// Windows 仅麦克风受隐私设置控制；其余 section 落到隐私设置首页。
    fn open_privacy_settings(&self, section: PrivacySection) {
        let uri = match section {
            PrivacySection::Microphone => "ms-settings:privacy-microphone",
            _ => "ms-settings:privacy",
        };
        let verb = wide("open");
        let file = wide(uri);
        // ShellExecuteW 返回值 <= 32 表示失败（等价 Python 的 os.startfile 抛异常）
        let result = unsafe {
            ShellExecuteW(
                None,
                PCWSTR(verb.as_ptr()),
                PCWSTR(file.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        if result.0 as usize <= 32 {
            tracing::warn!("打开隐私设置失败：{uri}");
        } else {
            tracing::info!("已打开 Windows 隐私设置：{section:?}");
        }
    }
}
