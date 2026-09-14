//! macOS 后端：afplay 提示音 / NSWorkspace 前台应用 / TCC 权限。（由 agent D 填写；下面是让 crate 先能编译的占位）
use super::Platform;
use crate::types::{FrontmostApp, Permissions, PrivacySection, SoundEvent};

pub struct MacPlatform;
impl MacPlatform {
    pub fn new() -> Self { MacPlatform }
}
impl Platform for MacPlatform {
    fn read_clipboard(&self) -> Option<String> { todo!() }
    fn write_clipboard(&self, _: &str) -> anyhow::Result<()> { todo!() }
    fn clear_clipboard(&self) -> anyhow::Result<()> { todo!() }
    fn send_copy(&self) -> anyhow::Result<()> { todo!() }
    fn send_paste(&self) -> anyhow::Result<()> { todo!() }
    fn play_sound(&self, _: SoundEvent) {}
    fn frontmost_app(&self) -> FrontmostApp { FrontmostApp::default() }
    fn check_permissions(&self, _: bool) -> Permissions { Permissions { input_monitoring: true, accessibility: true } }
    fn open_privacy_settings(&self, _: PrivacySection) {}
}
