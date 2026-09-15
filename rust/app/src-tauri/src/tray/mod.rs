//! 菜单栏/托盘图标与菜单。菜单结构 1:1 对照 Python 版 `app.py`。
//!
//! Tauri 的菜单是静态的，没有 rumps 的「打开菜单时重算」钩子，所以这里起一个 1s 定时线程
//! （对应 Python 的 `rumps.Timer`）：历史 `version` 变了就重建「最近历史」子菜单与统计行，
//! 录音期间在图标旁显示已录秒数。菜单条目的读写在 Tauri 内部会自动派发回主线程，可跨线程调用。

mod actions;
mod menu;

use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tauri::image::Image;
use tauri::menu::{CheckMenuItem, IsMenuItem, MenuItem, Submenu};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Manager, Wry};
use xiaodao_core::polisher::style_names;
use xiaodao_core::settings::Settings;
use xiaodao_core::types::Status;

use crate::bootstrap;

// 菜单栏模板图标（黑 + alpha，系统自动适配深浅色）。编进二进制，免打包时再拷资源。
const ICON_IDLE: &[u8] = include_bytes!("../../icons/menubar/idle.png");
const ICON_RECORDING: &[u8] = include_bytes!("../../icons/menubar/recording.png");
const ICON_TRANSCRIBING: &[u8] = include_bytes!("../../icons/menubar/transcribing.png");
const ICON_POLISHING: &[u8] = include_bytes!("../../icons/menubar/polishing.png");
const ICON_PAUSED: &[u8] = include_bytes!("../../icons/menubar/paused.png");

/// 历史菜单展示条数与单条标题字符上限（Python `app.py::_refresh_history`）。
const HISTORY_ITEMS: usize = 10;
const HISTORY_LABEL_CHARS: usize = 24;
/// 统计口径：打字 60 字/分、说话 180 字/分，省下的时间约等于 字数 / 90 分钟。
const SAVED_CHARS_PER_MINUTE: u64 = 90;
/// 子菜单清空时的兜底上限，避免 remove_at 异常时死循环。
const MAX_SUBMENU_ITEMS: usize = 64;

/// 托盘运行期句柄。放进 Tauri 全局 state，供状态回调改图标与文字。
pub struct TrayState {
    tray: TrayIcon<Wry>,
    status_item: MenuItem<Wry>,
    stats_item: MenuItem<Wry>,
    pause_item: CheckMenuItem<Wry>,
    polish_item: CheckMenuItem<Wry>,
    preview_item: CheckMenuItem<Wry>,
    sounds_item: CheckMenuItem<Wry>,
    history_menu: Submenu<Wry>,
    style_menu: Submenu<Wry>,
    /// 单选组条目：id 形如 `hotkey:alt_l`，同前缀者互斥（风格子菜单会整体重建）。
    checks: Mutex<Vec<(String, CheckMenuItem<Wry>)>>,
    /// 历史菜单第 i 项对应的完整文本（菜单标题会截断，复制要用全文）。
    history_texts: Mutex<Vec<String>>,
    /// 已渲染的历史版本号，与 `History::version()` 比对决定是否重绘。
    history_version: Mutex<Option<u64>>,
    /// 录音开始时刻（图标旁显示已录秒数）。
    recording_since: Mutex<Option<Instant>>,
    /// 当前图标旁文字，避免每秒重复设置。
    title_shown: Mutex<Option<String>>,
}

/// 互斥锁中毒时沿用内部值：菜单状态不是一致性关键数据，不值得让进程崩溃。
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
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
    let parts = menu::build_menu(app)?;
    let mut builder = TrayIconBuilder::with_id("main")
        .menu(&parts.menu)
        .show_menu_on_left_click(true)
        .on_menu_event(actions::on_menu_event);
    if let Some(icon) = image(ICON_IDLE) {
        builder = builder.icon(icon).icon_as_template(true);
    }
    let tray = builder.build(app)?;

    app.manage(TrayState {
        tray,
        status_item: parts.status_item,
        stats_item: parts.stats_item,
        pause_item: parts.pause_item,
        polish_item: parts.polish_item,
        preview_item: parts.preview_item,
        sounds_item: parts.sounds_item,
        history_menu: parts.history_menu,
        style_menu: parts.style_menu,
        checks: Mutex::new(parts.checks),
        history_texts: Mutex::new(Vec::new()),
        history_version: Mutex::new(None),
        recording_since: Mutex::new(None),
        title_shown: Mutex::new(None),
    });
    sync_menu_state(app);
    refresh(app);
    spawn_refresh(app.clone());
    Ok(())
}

/// 1s 定时线程：历史/统计重绘 + 录音计时（对应 Python 的两个 `rumps.Timer`）。
fn spawn_refresh(app: AppHandle) {
    let spawned = std::thread::Builder::new()
        .name("xiaodao-tray".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            refresh(&app);
        });
    if let Err(e) = spawned {
        tracing::error!("托盘刷新线程创建失败：{e}");
    }
}

fn refresh(app: &AppHandle) {
    let (Some(tray), Some(state)) = (app.try_state::<TrayState>(), bootstrap::state(app)) else {
        return;
    };
    let version = state.history.version();
    let stale = *lock(&tray.history_version) != Some(version);
    if stale {
        *lock(&tray.history_version) = Some(version);
        refresh_stats(&tray, &state);
        rebuild_history(app, &tray, &state);
    }
    refresh_recording_title(&tray);
}

fn refresh_stats(tray: &TrayState, state: &bootstrap::AppState) {
    let chars = state.history.total_chars();
    let saved = chars / SAVED_CHARS_PER_MINUTE;
    let text = format!(
        "统计：{} 段 · {chars} 字 · 约省 {saved} 分钟",
        state.history.total_count()
    );
    let _ = tray.stats_item.set_text(text);
}

/// 重建「最近历史」子菜单：点击即复制该条全文。
fn rebuild_history(app: &AppHandle, tray: &TrayState, state: &bootstrap::AppState) {
    clear_submenu(&tray.history_menu);
    let entries = state.history.recent(HISTORY_ITEMS);
    if entries.is_empty() {
        if let Ok(item) =
            MenuItem::with_id(app, "history:none", "（暂无记录）", false, None::<&str>)
        {
            let _ = tray.history_menu.append(&item);
        }
        lock(&tray.history_texts).clear();
        return;
    }
    let mut texts = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let label = truncate(&entry.final_text, HISTORY_LABEL_CHARS);
        match MenuItem::with_id(app, format!("history:{index}"), label, true, None::<&str>) {
            Ok(item) => {
                let _ = tray.history_menu.append(&item);
                texts.push(entry.final_text.clone());
            }
            Err(e) => tracing::warn!("历史菜单条目创建失败：{e}"),
        }
    }
    *lock(&tray.history_texts) = texts;
}

/// 录音中在图标旁显示已录秒数（尤其锁定录音时的「还在录」确认）。
///
/// **绝不能持锁调 `set_title`**：托盘 API 会把调用派发回主线程并阻塞等待，
/// 若此时主线程正在菜单回调里等这把锁就死锁了。先改完内存状态再放锁、最后才动 UI。
fn refresh_recording_title(tray: &TrayState) {
    let desired =
        lock(&tray.recording_since).map(|since| format!("{}s", since.elapsed().as_secs()));
    {
        let mut shown = lock(&tray.title_shown);
        if *shown == desired {
            return;
        }
        shown.clone_from(&desired);
    }
    let _ = tray.tray.set_title(desired.as_deref());
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut out: String = text.chars().take(limit).collect();
    out.push('…');
    out
}

fn clear_submenu(submenu: &Submenu<Wry>) {
    for _ in 0..MAX_SUBMENU_ITEMS {
        match submenu.remove_at(0) {
            Ok(Some(_)) => continue,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!("清空子菜单失败：{e}");
                return;
            }
        }
    }
}

// ---- 勾选状态 ----

/// 把 settings 当前值同步到菜单勾选状态（对应 Python `_sync_menu_state`）。
pub fn sync_menu_state(app: &AppHandle) {
    let (Some(tray), Some(state)) = (app.try_state::<TrayState>(), bootstrap::state(app)) else {
        return;
    };
    let settings = state.store.get();
    select_exclusive(&tray, &format!("hotkey:{}", settings.hotkey.as_str()));
    select_exclusive(
        &tray,
        &format!("rewrite:{}", settings.rewrite_hotkey.as_str()),
    );
    select_exclusive(&tray, &format!("mode:{}", settings.record_mode.as_str()));
    let _ = tray.preview_item.set_checked(settings.live_preview);
    let _ = tray.polish_item.set_checked(settings.polish.enabled);
    let _ = tray.sounds_item.set_checked(settings.sounds);
    let model = if settings.polish.model.trim().is_empty() {
        "?"
    } else {
        settings.polish.model.as_str()
    };
    let _ = tray
        .polish_item
        .set_text(format!("AI 润色（{} / {model}）", settings.polish.provider));
    rebuild_style_menu(app, &tray, &settings);
}

/// 按当前（含自定义）风格列表重建「润色风格」子菜单（对应 Python `_rebuild_style_menu`）。
fn rebuild_style_menu(app: &AppHandle, tray: &TrayState, settings: &Settings) {
    clear_submenu(&tray.style_menu);
    lock(&tray.checks).retain(|(id, _)| !id.starts_with("style:"));
    let current = &settings.polish.style;
    for name in style_names(settings) {
        let id = format!("style:{name}");
        let checked = &name == current;
        match CheckMenuItem::with_id(app, &id, &name, true, checked, None::<&str>) {
            Ok(item) => {
                let _ = tray.style_menu.append(&item as &dyn IsMenuItem<Wry>);
                lock(&tray.checks).push((id, item));
            }
            Err(e) => tracing::warn!("风格菜单条目创建失败：{e}"),
        }
    }
}

/// 单选：把同前缀的兄弟条目全部取消勾选，只留被点的那个。
fn select_exclusive(tray: &TrayState, id: &str) {
    let Some(prefix) = id.split(':').next() else {
        return;
    };
    let prefix = format!("{prefix}:");
    // 同 refresh_recording_title：set_checked 会派发回主线程并阻塞，
    // 先把要改的条目从锁里取出来再放锁，不在持锁期间碰 UI。
    let targets: Vec<(String, CheckMenuItem<Wry>)> = lock(&tray.checks)
        .iter()
        .filter(|(item_id, _)| item_id.starts_with(&prefix))
        .cloned()
        .collect();
    for (item_id, item) in targets {
        let _ = item.set_checked(item_id == id);
    }
}

// ---- 状态展示 ----

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

/// 状态变化：换托盘图标 + 改状态行文字（对应 Python `_set_status`）。
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
    *lock(&state.recording_since) = match status {
        Status::Recording => Some(Instant::now()),
        _ => None,
    };
    refresh_recording_title(&state);
    if let Some(icon) = image(bytes) {
        let _ = state.tray.set_icon(Some(icon));
    }
    // 录音态是唯一的彩色图标（固定红），必须关掉模板渲染
    let _ = state.tray.set_icon_as_template(status != Status::Recording);
    let _ = state.status_item.set_text(label);
}

/// 直接改状态行文字（权限提示、模型下载等非 Status 枚举场景）。
pub fn set_status_text(app: &AppHandle, text: &str) {
    if let Some(state) = app.try_state::<TrayState>() {
        let _ = state.status_item.set_text(text);
    }
}

/// 状态行可否点击：缺权限时打开，点击直达系统设置（Python 用 `set_callback` 实现）。
pub fn set_status_clickable(app: &AppHandle, clickable: bool) {
    if let Some(state) = app.try_state::<TrayState>() {
        let _ = state.status_item.set_enabled(clickable);
    }
}
