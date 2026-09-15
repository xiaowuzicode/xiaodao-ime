//! 前端可调用的命令，以及打开文件等小工具。
//!
//! settings.json 的唯一读写口是核心层的 `SettingsStore`（与 Python 版同 schema、同位置）：
//! 设置页保存后立刻把新热键/录音方式推给状态机，无需重启（对应 `settings_window.py` 的 on_save）。

use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;
use xiaodao_core::keys::HotkeyId;
use xiaodao_core::settings::Settings;
use xiaodao_core::types::Channel;

use crate::bootstrap;
use crate::hud::TauriHud;

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

/// 用文本编辑器打开（macOS `open -t`：.json 的默认程序常被浏览器抢注）。
pub fn open_text_file(app: &AppHandle, path: &Path) {
    #[cfg(target_os = "macos")]
    {
        match std::process::Command::new("open")
            .arg("-t")
            .arg(path)
            .spawn()
        {
            Ok(_) => return,
            Err(e) => tracing::warn!("open -t 打开 {} 失败：{e}", path.display()),
        }
    }
    open_path(app, path);
}

/// 读 settings.json（应用未初始化时回退默认值，不让设置页打不开）。
#[tauri::command]
pub fn get_settings(app: AppHandle) -> Value {
    let settings = bootstrap::state(&app)
        .map(|state| state.store.get())
        .unwrap_or_default();
    serde_json::to_value(settings).unwrap_or_else(|e| {
        tracing::error!("设置序列化失败：{e}");
        Value::Object(serde_json::Map::new())
    })
}

/// 整体写回 settings.json 并立刻生效（前端已保留未知键）。
#[tauri::command]
pub fn save_settings(app: AppHandle, settings: Value) -> Result<(), String> {
    let state = bootstrap::state(&app).ok_or_else(|| "应用尚未初始化".to_string())?;
    let parsed: Settings =
        serde_json::from_value(settings).map_err(|e| format!("设置格式不正确：{e}"))?;
    state.store.update(|current| *current = parsed);
    state
        .store
        .save()
        .map_err(|e| format!("写入设置失败：{e:#}"))?;
    bootstrap::apply_settings(&app);
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

/// 跳转系统隐私设置面板。
#[tauri::command]
pub fn open_privacy_settings(app: AppHandle, section: String) {
    bootstrap::open_privacy(&app, &section);
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
