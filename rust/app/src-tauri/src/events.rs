//! 状态与通知：把核心层的 [`Events`] trait 落到托盘图标与系统通知。

use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;
use xiaodao_core::types::{Events, Status};

use crate::tray;

/// 系统通知标题（与 Python 版一致）。
const APP_NAME: &str = "小岛AI输入法";

pub struct TauriEvents {
    app: AppHandle,
}

impl TauriEvents {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl Events for TauriEvents {
    fn status(&self, status: Status) {
        tray::apply_status(&self.app, status);
    }

    fn notify(&self, title: &str, message: &str) {
        let body = if title.is_empty() {
            message.to_string()
        } else {
            format!("{title}\n{message}")
        };
        if let Err(e) = self
            .app
            .notification()
            .builder()
            .title(APP_NAME)
            .body(body)
            .show()
        {
            tracing::warn!("系统通知发送失败：{e}");
        }
    }

    fn first_key_event(&self) {
        // 功能性纠偏：真收到键盘事件 = 输入监听权限已通（对齐 Python `_on_first_key_event`）
        tracing::info!("已收到首个键盘事件，输入监听权限正常");
        tray::set_status_text(&self.app, "状态：待机");
    }
}
