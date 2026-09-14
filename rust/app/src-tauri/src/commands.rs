//! 前端可调用的命令，以及路径/外部打开等小工具。
//!
//! 本阶段 settings.json 用 `serde_json::Value` 直读直写（与 Python 版同 schema、同位置），
//! 接上核心后换成 `xiaodao_core::settings::SettingsStore`。

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Map, Value};
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;
use xiaodao_core::keys::HotkeyId;
use xiaodao_core::types::Channel;

use crate::hud::TauriHud;

/// 数据目录名（与 Python 版一致，老用户配置可直接继承）。
const APP_DIR_NAME: &str = "xiaodao-ime";

/// 数据目录：`XIAODAO_HOME` 优先；macOS `~/Library/Application Support/xiaodao-ime`，
/// Windows `%APPDATA%\xiaodao-ime`。
pub fn data_dir() -> PathBuf {
    if let Ok(home) = std::env::var("XIAODAO_HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home);
        }
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(home)
            .join("Library/Application Support")
            .join(APP_DIR_NAME)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let base = std::env::var("APPDATA")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_default();
        PathBuf::from(base).join(APP_DIR_NAME)
    }
}

pub fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

pub fn log_file() -> PathBuf {
    data_dir().join("logs").join("xiaodao-ime.log")
}

/// 用系统默认程序打开文件/目录（打开日志用）。
pub fn open_path(app: &AppHandle, path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if !path.exists() {
        let _ = std::fs::write(path, b"");
    }
    if let Err(e) = app
        .opener()
        .open_path(path.to_string_lossy().to_string(), None::<&str>)
    {
        tracing::warn!("打开 {} 失败：{e}", path.display());
    }
}

/// settings.json 默认值（对齐 Python `xiaodao_ime/settings.py` 的 DEFAULTS）。
fn defaults() -> Value {
    json!({
        "hotkey": HotkeyId::default_dictate().as_str(),
        "rewrite_hotkey": HotkeyId::default_rewrite().as_str(),
        "record_mode": "toggle",
        "live_preview": true,
        "polish": {
            "enabled": false,
            "provider": "openai",
            "model": "deepseek-chat",
            "api_key": "",
            "base_url": "https://api.deepseek.com",
            "timeout": 30,
            "style": "润色",
            "styles": {},
        },
        "app_styles": {},
        "sounds": true,
        "history": {"enabled": true, "max_items": 50},
        "hotwords": [],
        "replacements": {},
    })
}

/// 字段级深合并：用户只写想改的键，其余取默认（与 Python `_merge` 同语义）。
fn merge(base: &Value, override_with: &Value) -> Value {
    let (Some(base_map), Some(over_map)) = (base.as_object(), override_with.as_object()) else {
        return override_with.clone();
    };
    let mut out: Map<String, Value> = base_map.clone();
    for (key, value) in over_map {
        let merged = match base_map.get(key) {
            Some(existing) if existing.is_object() && value.is_object() => merge(existing, value),
            _ => value.clone(),
        };
        out.insert(key.clone(), merged);
    }
    Value::Object(out)
}

/// 读 settings.json（缺失或解析失败一律回退默认值，不让设置页打不开）。
#[tauri::command]
pub fn get_settings() -> Value {
    let path = settings_path();
    match std::fs::read_to_string(&path) {
        Ok(raw) => match serde_json::from_str::<Value>(&raw) {
            Ok(value) => merge(&defaults(), &value),
            Err(e) => {
                tracing::error!("settings.json 解析失败，使用默认设置：{e}");
                defaults()
            }
        },
        Err(_) => {
            tracing::info!("settings.json 不存在，使用默认设置：{}", path.display());
            defaults()
        }
    }
}

/// 整体写回 settings.json（前端已保留未知键）。
#[tauri::command]
pub fn save_settings(settings: Value) -> Result<(), String> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建数据目录失败：{e}"))?;
    }
    let mut text = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    text.push('\n');
    std::fs::write(&path, text).map_err(|e| format!("写入设置失败：{e}"))?;
    tracing::info!("设置已保存：{}", path.display());
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct HotkeyChoice {
    pub id: &'static str,
    pub label: &'static str,
}

/// 当前平台可选热键（顺序即设置页下拉顺序）。
#[tauri::command]
pub fn get_hotkey_choices() -> Vec<HotkeyChoice> {
    HotkeyId::choices()
        .iter()
        .map(|k| HotkeyChoice {
            id: k.as_str(),
            label: k.display(),
        })
        .collect()
}

/// 记录一次权限面板跳转请求（接核心后调 platform 的 open_privacy_settings）。
pub fn log_privacy_section(section: &str) {
    tracing::info!("请求打开系统隐私设置：{section}（应用壳阶段仅打日志）");
}

/// 跳转系统隐私设置面板。TODO(接核心)：调 `xiaodao_core::platform` 的实现。
#[tauri::command]
pub fn open_privacy_settings(section: String) {
    log_privacy_section(&section);
}

/// 调试用：播一串假的 HUD 事件（begin → level/partial → status → hide）。
#[tauri::command]
pub fn hud_demo(app: AppHandle) {
    std::thread::spawn(move || run_demo(&TauriHud::new(app)));
}

fn run_demo(hud: &TauriHud) {
    use std::thread::sleep;
    use std::time::Duration;
    use xiaodao_core::types::Hud as _;

    const DEMO_TEXT: &str = "这是小岛AI输入法的悬浮窗预览，按热键说话文字就会出现在光标处。";
    // 等前端把 listen("hud") 挂好再发首个事件（启动即播的场景下 webview 尚未加载完）
    sleep(Duration::from_millis(800));
    hud.begin(Channel::Dictate, "聆听中…", "再按热键出字 · 按 Esc 取消");
    let chars: Vec<char> = DEMO_TEXT.chars().collect();
    for tick in 0..48u32 {
        // 无需引入随机数：用两个不同周期的正弦叠加出像样的说话包络
        let t = tick as f32;
        let level = (0.45 + 0.35 * (t * 0.7).sin() + 0.2 * (t * 1.9).sin()).clamp(0.05, 1.0);
        hud.set_level(level);
        if tick % 6 == 0 {
            let take = ((tick as usize / 6 + 1) * 5).min(chars.len());
            let text: String = chars[..take].iter().collect();
            hud.set_partial(f64::from(tick) / 12.0, &text);
        }
        sleep(Duration::from_millis(80));
    }
    hud.set_status("转写中…", DEMO_TEXT);
    sleep(Duration::from_millis(900));
    hud.set_status("润色中…", DEMO_TEXT);
    sleep(Duration::from_millis(1200));
    hud.hide();
}
