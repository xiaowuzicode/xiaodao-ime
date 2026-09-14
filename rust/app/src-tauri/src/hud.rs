//! HUD 悬浮窗：把核心层的 [`Hud`] trait 调用翻译成前端事件。
//!
//! 核心层（`xiaodao-core`）不知道 Tauri 的存在，只调 trait；本模块负责
//! `emit_to("hud", "hud", HudEvent)` 并顺带显示/隐藏窗口。
//! 事件在 `app/src/hud.ts` 里消费，两边的 JSON 形状必须同步修改。

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use xiaodao_core::types::{Channel, Hud};

/// HUD 窗口 label（与 tauri.conf.json 一致）。
pub const HUD_WINDOW: &str = "hud";

/// 发给前端的 HUD 事件：`{"kind": "...", ...}`。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HudEvent {
    /// 开始一次录音会话：重置并显示悬浮窗。
    Begin {
        channel: Channel,
        placeholder: String,
        hint: String,
    },
    /// 音量电平 0~1，约 12Hz。
    Level { level: f32 },
    /// 已录秒数 + 伪流式识别文本（text 为空表示沿用上次）。
    Partial { elapsed: f64, text: String },
    /// 录音后阶段：主行状态文案 + 副行已转写全文。
    Status { status: String, detail: String },
    /// 隐藏悬浮窗。
    Hide,
}

/// [`Hud`] 的 Tauri 实现。可从任意线程调用（`AppHandle` 是 Send + Sync）。
pub struct TauriHud {
    app: AppHandle,
}

impl TauriHud {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }

    pub fn emit(&self, event: HudEvent) {
        if let Err(e) = self.app.emit_to(HUD_WINDOW, "hud", event) {
            tracing::debug!("HUD 事件发送失败：{e}");
        }
    }

    fn set_visible(&self, visible: bool) {
        let Some(window) = self.app.get_webview_window(HUD_WINDOW) else {
            return;
        };
        let result = if visible {
            window.show()
        } else {
            window.hide()
        };
        if let Err(e) = result {
            tracing::debug!("HUD 窗口显示/隐藏失败：{e}");
        }
    }
}

impl Hud for TauriHud {
    fn begin(&self, channel: Channel, placeholder: &str, hint: &str) {
        self.emit(HudEvent::Begin {
            channel,
            placeholder: placeholder.to_string(),
            hint: hint.to_string(),
        });
        self.set_visible(true);
    }

    fn set_level(&self, level: f32) {
        self.emit(HudEvent::Level { level });
    }

    fn set_partial(&self, elapsed: f64, text: &str) {
        self.emit(HudEvent::Partial {
            elapsed,
            text: text.to_string(),
        });
    }

    fn set_status(&self, status: &str, detail: &str) {
        self.emit(HudEvent::Status {
            status: status.to_string(),
            detail: detail.to_string(),
        });
        self.set_visible(true);
    }

    fn hide(&self) {
        self.emit(HudEvent::Hide);
        self.set_visible(false);
    }
}
