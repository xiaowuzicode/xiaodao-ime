//! 小岛AI输入法核心库（平台无关、无 UI 依赖）。
//!
//! 分层铁律（与 Python 版 AGENTS.md 一致）：
//! - 本 crate 只依赖 [`platform::Platform`] trait 与 [`types`] 里的抽象，
//!   系统 API 调用全部收进 `platform/mac.rs` / `platform/win.rs`；
//! - UI（托盘 / HUD / 设置窗口）由 `app/src-tauri` 实现 [`types::Hud`] 与 [`types::Events`]
//!   两个 trait 接进来；核心层不知道 Tauri 的存在。
//!
//! 线程模型：全部同步代码 + `std::thread`。键盘钩子线程 → [`hotkey::HotkeyController`]
//! 状态机 → 转写/润色 worker 线程 → 通过 trait 回调把状态推给 UI。

pub mod audio;
pub mod context;
pub mod history;
pub mod hotkey;
pub mod hud;
pub mod keys;
pub mod listener;
pub mod logging;
pub mod model_download;
pub mod paster;
pub mod paths;
pub mod platform;
pub mod polisher;
pub mod settings;
pub mod transcriber;
pub mod types;

pub use keys::{HotkeyId, RecordMode};
pub use settings::Settings;
pub use types::*;
