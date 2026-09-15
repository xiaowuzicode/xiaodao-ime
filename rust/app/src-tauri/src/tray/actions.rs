//! 托盘菜单事件分派。每一项的行为对照 Python 版 `app.py` 的同名回调。

use tauri::menu::MenuEvent;
use tauri::{AppHandle, Manager};
use xiaodao_core::keys::{HotkeyId, RecordMode};
use xiaodao_core::paster;
use xiaodao_core::polisher::style_names;
use xiaodao_core::types::{Events, Polish, Status};

use crate::bootstrap;
use crate::commands;
use crate::events::TauriEvents;

use super::{apply_status, lock, open_settings, sync_menu_state, TrayState};

pub fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    let id = event.id().as_ref().to_string();
    match id.as_str() {
        "quit" => bootstrap::quit(app),
        "status" => bootstrap::open_missing_privacy(app),
        "settings" => open_settings(app),
        "edit_json" => open_settings_json(app),
        "reload" => reload(app),
        "log" => open_log(app),
        "pause" => toggle_pause(app),
        "preview" => toggle_preview(app),
        "polish" => toggle_polish(app),
        "sounds" => toggle_sounds(app),
        _ => on_prefixed_event(app, &id),
    }
}

fn on_prefixed_event(app: &AppHandle, id: &str) {
    let Some((prefix, value)) = id.split_once(':') else {
        tracing::debug!("菜单：未处理的条目 {id}");
        return;
    };
    match prefix {
        "hotkey" => set_hotkey(app, value, true),
        "rewrite" => set_hotkey(app, value, false),
        "mode" => set_mode(app, value),
        "style" => set_style(app, value),
        "history" => copy_history(app, value),
        "perm" => bootstrap::open_privacy(app, value),
        _ => tracing::debug!("菜单：未处理的条目 {id}"),
    }
}

// ---- 热键 / 录音方式 / 风格 ----

/// 切换听写（`dictate=true`）或改写热键；两者不能相同（与 Python 版同提示语）。
fn set_hotkey(app: &AppHandle, value: &str, dictate: bool) {
    let Some(id) = HotkeyId::parse(value) else {
        tracing::warn!("菜单：未知热键 {value}");
        return;
    };
    let Some(state) = bootstrap::state(app) else {
        return;
    };
    let current = state.store.get();
    let events = TauriEvents::new(app.clone());
    if dictate && id == current.rewrite_hotkey {
        events.notify("听写热键不能和改写热键相同", "请先把改写热键换成别的键。");
        return;
    }
    if !dictate && id == current.hotkey {
        events.notify("改写热键不能和听写热键相同", "请先把听写热键换成别的键。");
        return;
    }
    state.store.update_and_save(|s| {
        if dictate {
            s.hotkey = id;
        } else {
            s.rewrite_hotkey = id;
        }
    });
    if let Some(controller) = state.controller() {
        if dictate {
            controller.set_trigger(id);
        } else {
            controller.set_rewrite_trigger(id);
        }
    }
    // 切换日志由核心 `HotkeyController::set_trigger` 打印，这里不重复
    sync_menu_state(app);
}

fn set_mode(app: &AppHandle, value: &str) {
    let Some(mode) = RecordMode::parse(value) else {
        tracing::warn!("菜单：未知录音方式 {value}");
        return;
    };
    let Some(state) = bootstrap::state(app) else {
        return;
    };
    state.store.update_and_save(|s| s.record_mode = mode);
    if let Some(controller) = state.controller() {
        controller.set_mode(mode);
    }
    // 切换日志由核心 `HotkeyController::set_mode` 打印，这里不重复
    sync_menu_state(app);
}

fn set_style(app: &AppHandle, value: &str) {
    let Some(state) = bootstrap::state(app) else {
        return;
    };
    // 菜单 id 由风格名拼出，这里再校验一次，避免 settings 被外部改后写入不存在的风格
    if !style_names(&state.store.get()).iter().any(|n| n == value) {
        tracing::warn!("菜单：未知润色风格 {value}");
        return;
    }
    state
        .store
        .update_and_save(|s| s.polish.style = value.to_string());
    sync_menu_state(app);
    tracing::info!("润色风格已切换为：{value}");
}

// ---- 开关 ----

/// 临时停用/恢复热键（打游戏、热键冲突时用），不退出 App。
fn toggle_pause(app: &AppHandle) {
    let (Some(tray), Some(state)) = (app.try_state::<TrayState>(), bootstrap::state(app)) else {
        return;
    };
    let controller = state.controller();
    let paused = !controller.as_ref().map(|c| c.paused()).unwrap_or(false);
    if let Some(controller) = controller {
        controller.set_paused(paused);
    } else {
        tracing::warn!("热键尚未就绪，暂停开关只改菜单显示");
    }
    let _ = tray.pause_item.set_checked(paused);
    let _ = tray.pause_item.set_text(if paused {
        "恢复热键"
    } else {
        "暂停热键"
    });
    apply_status(app, if paused { Status::Paused } else { Status::Idle });
}

fn toggle_preview(app: &AppHandle) {
    let Some(state) = bootstrap::state(app) else {
        return;
    };
    let enabled = state.store.update_and_save(|s| {
        s.live_preview = !s.live_preview;
        s.live_preview
    });
    sync_menu_state(app);
    tracing::info!("实时预览已{}", if enabled { "开启" } else { "关闭" });
}

/// 录音开始/结束提示音总开关（对应 settings 的 `sounds`）。
fn toggle_sounds(app: &AppHandle) {
    let Some(state) = bootstrap::state(app) else {
        return;
    };
    let enabled = state.store.update_and_save(|s| {
        s.sounds = !s.sounds;
        s.sounds
    });
    sync_menu_state(app);
    tracing::info!("提示音已{}", if enabled { "开启" } else { "关闭" });
}

/// AI 润色总开关；配置不全时拒绝开启并引导去填配置（与 Python 版一致）。
fn toggle_polish(app: &AppHandle) {
    let Some(state) = bootstrap::state(app) else {
        return;
    };
    if !state.store.get().polish.enabled && !state.polisher.configured() {
        TauriEvents::new(app.clone()).notify(
            "无法开启 AI 润色",
            "请先点「设置 → 打开配置文件」，填写 polish 的 base_url / api_key / model。",
        );
        open_settings(app);
        sync_menu_state(app);
        return;
    }
    let enabled = state.store.update_and_save(|s| {
        s.polish.enabled = !s.polish.enabled;
        s.polish.enabled
    });
    sync_menu_state(app);
    tracing::info!("AI 润色已{}", if enabled { "开启" } else { "关闭" });
}

// ---- 配置 / 日志 / 历史 ----

/// 外部编辑 settings.json 后手动重载，无需重启进程。
fn reload(app: &AppHandle) {
    let Some(state) = bootstrap::state(app) else {
        return;
    };
    state.store.reload();
    bootstrap::apply_settings(app);
    tracing::info!("配置已重新加载");
}

fn open_settings_json(app: &AppHandle) {
    let Some(state) = bootstrap::state(app) else {
        return;
    };
    commands::open_text_file(app, &state.store.ensure_file());
}

fn open_log(app: &AppHandle) {
    let Some(state) = bootstrap::state(app) else {
        return;
    };
    let path = state.paths.log_file.clone();
    tracing::info!("菜单：打开日志 {}", path.display());
    commands::open_path(app, &path);
}

fn copy_history(app: &AppHandle, value: &str) {
    let Ok(index) = value.parse::<usize>() else {
        return; // "history:none" 占位项
    };
    let (Some(tray), Some(state)) = (app.try_state::<TrayState>(), bootstrap::state(app)) else {
        return;
    };
    let text = lock(&tray.history_texts).get(index).cloned();
    let Some(text) = text else {
        return;
    };
    if paster::copy_to_clipboard(&state.platform, &text) {
        tracing::info!("历史条目已复制（{} 字符）", text.chars().count());
    }
}
