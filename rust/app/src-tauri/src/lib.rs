//! 小岛AI输入法 Tauri 应用壳：托盘菜单 / 透明 HUD / 设置窗口。
//!
//! 分层：本 crate 只做 UI 与系统集成，语音/热键/润色等逻辑全在 `xiaodao-core`；
//! 两者通过 `xiaodao_core::types` 的 [`Hud`](xiaodao_core::types::Hud) 与
//! [`Events`](xiaodao_core::types::Events) trait 对接（见 `hud.rs` / `events.rs`）。

pub mod bootstrap;
pub mod commands;
pub mod events;
pub mod hud;
pub mod tray;

use tauri::{Manager, PhysicalPosition, WebviewWindow, WindowEvent};

/// HUD 距屏幕可视区域底部的距离（pt），对齐 Python 版 `_MARGIN_BOTTOM`。
const HUD_MARGIN_BOTTOM: f64 = 84.0;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // 第二次启动：不再开新进程，把设置窗口拉到前台
            tracing::info!("已有实例在运行，唤出设置窗口");
            tray::open_settings(app);
        }))
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::get_hotkey_choices,
            commands::open_privacy_settings,
            commands::hud_demo,
        ])
        .setup(|app| {
            // 菜单栏常驻应用：不占 Dock、不抢焦点（HUD 透明窗的前提）
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // 顺序固定：先备好核心对象（日志 / 设置 / 历史），托盘建菜单时要读它们，
            // 最后才启动后台线程（模型 + 键钩），保证状态行已经能显示进度。
            bootstrap::init(app.handle()).map_err(|e| e.to_string())?;
            tray::build(app.handle())?;

            if let Some(hud) = app.get_webview_window(hud::HUD_WINDOW) {
                if let Err(e) = place_hud(&hud) {
                    tracing::warn!("HUD 定位失败：{e}");
                }
                // 点击穿透：HUD 不能挡住用户正在操作的窗口（对齐 Python 的 setIgnoresMouseEvents）
                let _ = hud.set_ignore_cursor_events(true);
            }
            if let Some(settings) = app.get_webview_window("settings") {
                // 关窗只隐藏：菜单栏应用不能因为关了设置窗口就退出
                let handle = settings.clone();
                settings.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = handle.hide();
                    }
                });
            }

            bootstrap::start(app.handle());

            // 开发期看 HUD 动效：XIAODAO_HUD_DEMO=1 cargo tauri dev
            if std::env::var("XIAODAO_HUD_DEMO").is_ok() {
                commands::hud_demo(app.handle().clone());
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Tauri 应用启动失败");
}

/// 把 HUD 放到主屏可视区域底部居中、上方留 84pt（与 Python 版位置一致）。
fn place_hud(window: &WebviewWindow) -> tauri::Result<()> {
    let Some(monitor) = window.primary_monitor()? else {
        return Ok(());
    };
    let scale = monitor.scale_factor();
    // work_area 已排除 Dock 与菜单栏，等价于 AppKit 的 visibleFrame
    let area = monitor.work_area();
    let size = window.outer_size()?;
    let margin = (HUD_MARGIN_BOTTOM * scale).round() as i32;
    let x = area.position.x + (area.size.width as i32 - size.width as i32) / 2;
    let y = area.position.y + area.size.height as i32 - size.height as i32 - margin;
    window.set_position(PhysicalPosition::new(x, y))
}
