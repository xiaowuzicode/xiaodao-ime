//! 菜单栏/托盘图标与菜单。菜单结构 1:1 对照 Python 版 `app.py`。
//!
//! 本阶段（应用壳）菜单事件只打日志 + 改状态行文字；接上核心逻辑后
//! 由各 handler 调 `xiaodao-core` 的对应能力。

use std::sync::Mutex;

use tauri::image::Image;
use tauri::menu::{
    CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu,
};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Manager, Wry};
use xiaodao_core::keys::{HotkeyId, RecordMode};
use xiaodao_core::types::Status;

use crate::commands;

// 菜单栏模板图标（黑 + alpha，系统自动适配深浅色）。编进二进制，免打包时再拷资源。
const ICON_IDLE: &[u8] = include_bytes!("../icons/menubar/idle.png");
const ICON_RECORDING: &[u8] = include_bytes!("../icons/menubar/recording.png");
const ICON_TRANSCRIBING: &[u8] = include_bytes!("../icons/menubar/transcribing.png");
const ICON_POLISHING: &[u8] = include_bytes!("../icons/menubar/polishing.png");
const ICON_PAUSED: &[u8] = include_bytes!("../icons/menubar/paused.png");

/// 内置 5 个润色风格（对齐 Python `polisher.BUILTIN_STYLES` 的键顺序）。
const BUILTIN_STYLES: [&str; 5] = ["润色", "书面化", "轻度纠错", "翻译成英文", "会议纪要"];

/// 托盘运行期句柄。放进 Tauri 全局 state，供状态回调改图标与文字。
pub struct TrayState {
    tray: TrayIcon<Wry>,
    status_item: MenuItem<Wry>,
    stats_item: MenuItem<Wry>,
    pause_item: CheckMenuItem<Wry>,
    /// 所有单选组条目：id 形如 `hotkey:alt_l`，前缀相同者互斥。
    checks: Vec<(String, CheckMenuItem<Wry>)>,
    paused: Mutex<bool>,
}

fn image(bytes: &[u8]) -> Option<Image<'static>> {
    match Image::from_bytes(bytes) {
        Ok(img) => Some(img.to_owned()),
        Err(e) => {
            tracing::error!("托盘图标解码失败：{e}");
            None
        }
    }
}

/// 建托盘图标 + 菜单，并把句柄挂进全局 state。
pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let status_item = MenuItem::with_id(app, "status", "状态：待机", false, None::<&str>)?;
    let stats_item = MenuItem::with_id(app, "stats", "统计：0 段 · 0 字", false, None::<&str>)?;
    let mut checks: Vec<(String, CheckMenuItem<Wry>)> = Vec::new();

    // 最近历史：接上核心前先占位（点击复制的回调在 on_menu_event 里按 id 前缀分派）
    let history_empty =
        MenuItem::with_id(app, "history:none", "（暂无记录）", false, None::<&str>)?;
    let history_menu =
        Submenu::with_id_and_items(app, "history", "最近历史", true, &[&history_empty])?;

    let style_menu = check_submenu(
        app,
        &mut checks,
        "style",
        "润色风格",
        BUILTIN_STYLES
            .iter()
            .map(|s| (s.to_string(), s.to_string())),
        "润色",
    )?;
    let hotkey_menu = check_submenu(
        app,
        &mut checks,
        "hotkey",
        "听写热键",
        HotkeyId::choices()
            .iter()
            .map(|k| (k.as_str().to_string(), k.display().to_string())),
        HotkeyId::default_dictate().as_str(),
    )?;
    let rewrite_menu = check_submenu(
        app,
        &mut checks,
        "rewrite",
        "改写热键",
        HotkeyId::choices()
            .iter()
            .map(|k| (k.as_str().to_string(), k.display().to_string())),
        HotkeyId::default_rewrite().as_str(),
    )?;
    let mode_menu = check_submenu(
        app,
        &mut checks,
        "mode",
        "录音方式",
        [RecordMode::Toggle, RecordMode::Hold]
            .iter()
            .map(|m| (m.as_str().to_string(), m.display().to_string())),
        RecordMode::default().as_str(),
    )?;

    let pause_item = CheckMenuItem::with_id(app, "pause", "暂停热键", true, false, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let sep3 = PredefinedMenuItem::separator(app)?;
    let settings_item = MenuItem::with_id(app, "settings", "设置…", true, None::<&str>)?;
    let reload_item = MenuItem::with_id(app, "reload", "重新加载配置", true, None::<&str>)?;
    let log_item = MenuItem::with_id(app, "log", "打开日志", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;

    // 权限直达只在 macOS 有意义（Windows 无对应权限体系）
    let perm_input =
        MenuItem::with_id(app, "perm:input_monitoring", "输入监听", true, None::<&str>)?;
    let perm_ax = MenuItem::with_id(app, "perm:accessibility", "辅助功能", true, None::<&str>)?;
    let perm_mic = MenuItem::with_id(app, "perm:microphone", "麦克风", true, None::<&str>)?;
    let perm_menu = Submenu::with_id_and_items(
        app,
        "permissions",
        "权限",
        true,
        &[&perm_input, &perm_ax, &perm_mic],
    )?;

    let mut items: Vec<&dyn IsMenuItem<Wry>> = vec![
        &status_item,
        &stats_item,
        &sep1,
        &history_menu,
        &style_menu,
        &hotkey_menu,
        &rewrite_menu,
        &mode_menu,
        &pause_item,
        &sep2,
        &settings_item,
        &reload_item,
        &log_item,
    ];
    if cfg!(target_os = "macos") {
        items.push(&perm_menu);
    }
    items.push(&sep3);
    items.push(&quit_item);
    let menu = Menu::with_items(app, &items)?;

    let mut builder = TrayIconBuilder::with_id("main")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(on_menu_event);
    if let Some(icon) = image(ICON_IDLE) {
        builder = builder.icon(icon).icon_as_template(true);
    }
    let tray = builder.build(app)?;

    app.manage(TrayState {
        tray,
        status_item,
        stats_item,
        pause_item,
        checks,
        paused: Mutex::new(false),
    });
    Ok(())
}

/// 建一个单选子菜单（勾选项互斥），并把条目登记进 `checks`。
fn check_submenu(
    app: &AppHandle,
    checks: &mut Vec<(String, CheckMenuItem<Wry>)>,
    prefix: &str,
    title: &str,
    entries: impl Iterator<Item = (String, String)>,
    current: &str,
) -> tauri::Result<Submenu<Wry>> {
    let mut owned = Vec::new();
    for (value, label) in entries {
        let id = format!("{prefix}:{value}");
        let item = CheckMenuItem::with_id(app, &id, label, true, value == current, None::<&str>)?;
        checks.push((id, item.clone()));
        owned.push(item);
    }
    let refs: Vec<&dyn IsMenuItem<Wry>> = owned.iter().map(|i| i as &dyn IsMenuItem<Wry>).collect();
    Submenu::with_id_and_items(app, prefix, title, true, &refs)
}

/// 单选：把同前缀的兄弟条目全部取消勾选，只留被点的那个。
fn select_exclusive(app: &AppHandle, id: &str) {
    let Some(prefix) = id.split(':').next() else {
        return;
    };
    let Some(state) = app.try_state::<TrayState>() else {
        return;
    };
    for (item_id, item) in &state.checks {
        if item_id.starts_with(&format!("{prefix}:")) {
            let _ = item.set_checked(item_id == id);
        }
    }
}

fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    let id = event.id().as_ref().to_string();
    match id.as_str() {
        "quit" => {
            tracing::info!("菜单：退出");
            app.exit(0);
        }
        "settings" => open_settings(app),
        "reload" => {
            // TODO(接核心)：调 SettingsStore::load() 并把新值应用到热键控制器
            tracing::info!("菜单：重新加载配置（应用壳阶段仅打日志）");
            set_status_text(app, "状态：配置已重新加载");
        }
        "log" => {
            let path = commands::log_file();
            tracing::info!("菜单：打开日志 {}", path.display());
            commands::open_path(app, &path);
        }
        "pause" => toggle_pause(app),
        _ => on_prefixed_event(app, &id),
    }
}

fn on_prefixed_event(app: &AppHandle, id: &str) {
    let Some((prefix, value)) = id.split_once(':') else {
        tracing::debug!("菜单：未处理的条目 {id}");
        return;
    };
    match prefix {
        "style" | "hotkey" | "rewrite" | "mode" => {
            // TODO(接核心)：写回 settings 并调 HotkeyController::set_*
            tracing::info!("菜单：{prefix} → {value}（应用壳阶段仅打日志）");
            select_exclusive(app, id);
        }
        "history" => {
            // TODO(接核心)：复制该条历史文本到剪贴板
            tracing::info!("菜单：复制历史条目 {value}（应用壳阶段仅打日志）");
        }
        "perm" => commands::log_privacy_section(value),
        _ => tracing::debug!("菜单：未处理的条目 {id}"),
    }
}

fn toggle_pause(app: &AppHandle) {
    let Some(state) = app.try_state::<TrayState>() else {
        return;
    };
    let paused = {
        let mut guard = state.paused.lock().expect("pause 状态锁");
        *guard = !*guard;
        *guard
    };
    // TODO(接核心)：调 HotkeyController::set_paused(paused)
    tracing::info!("菜单：暂停热键 → {paused}");
    let _ = state.pause_item.set_checked(paused);
    let _ = state.pause_item.set_text(if paused {
        "恢复热键"
    } else {
        "暂停热键"
    });
    apply_status(app, if paused { Status::Paused } else { Status::Idle });
}

/// 打开设置窗口（先让前端重读一遍 settings.json，避免显示陈旧值）。
pub fn open_settings(app: &AppHandle) {
    let Some(window) = app.get_webview_window("settings") else {
        tracing::error!("找不到 settings 窗口");
        return;
    };
    use tauri::Emitter;
    let _ = window.emit("settings:reload", ());
    let _ = window.show();
    let _ = window.set_focus();
}

/// 状态变化：换托盘图标 + 改状态行文字（对齐 Python `_set_status`）。
pub fn apply_status(app: &AppHandle, status: Status) {
    let Some(state) = app.try_state::<TrayState>() else {
        return;
    };
    let (bytes, label) = match status {
        Status::Idle => (ICON_IDLE, "状态：待机"),
        Status::Recording => (ICON_RECORDING, "状态：录音中"),
        Status::Transcribing => (ICON_TRANSCRIBING, "状态：转写中"),
        Status::Polishing => (ICON_POLISHING, "状态：润色中"),
        Status::Paused => (ICON_PAUSED, "状态：已暂停"),
    };
    if let Some(icon) = image(bytes) {
        let _ = state.tray.set_icon(Some(icon));
    }
    // 录音态是唯一的彩色图标（固定红），必须关掉模板渲染
    let _ = state.tray.set_icon_as_template(status != Status::Recording);
    let _ = state.status_item.set_text(label);
}

/// 直接改状态行文字（权限提示、配置重载等非 Status 枚举场景）。
pub fn set_status_text(app: &AppHandle, text: &str) {
    if let Some(state) = app.try_state::<TrayState>() {
        let _ = state.status_item.set_text(text);
    }
}

/// 改「输入统计」行（接上 history 模块后由其调用）。
pub fn set_stats_text(app: &AppHandle, text: &str) {
    if let Some(state) = app.try_state::<TrayState>() {
        let _ = state.stats_item.set_text(text);
    }
}
