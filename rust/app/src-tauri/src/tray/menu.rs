//! 托盘菜单结构：条目与层级 1:1 对照 Python 版 `app.py` 的 `self.menu`。
//!
//! 只负责「建出来」，勾选状态由 [`crate::tray::sync_menu_state`] 按 settings 当前值刷。

use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Wry};
use xiaodao_core::keys::{HotkeyId, RecordMode};

/// 建好的菜单及需要运行期改动的条目句柄。
pub struct MenuParts {
    pub menu: Menu<Wry>,
    pub status_item: MenuItem<Wry>,
    pub stats_item: MenuItem<Wry>,
    pub pause_item: CheckMenuItem<Wry>,
    pub polish_item: CheckMenuItem<Wry>,
    pub preview_item: CheckMenuItem<Wry>,
    pub history_menu: Submenu<Wry>,
    pub style_menu: Submenu<Wry>,
    /// 单选组条目：id 形如 `hotkey:alt_l`，同前缀者互斥。
    pub checks: Vec<(String, CheckMenuItem<Wry>)>,
}

/// 建单选子菜单（勾选互斥），条目登记进 `checks`。
pub fn check_submenu(
    app: &AppHandle,
    checks: &mut Vec<(String, CheckMenuItem<Wry>)>,
    prefix: &str,
    title: &str,
    entries: impl Iterator<Item = (String, String)>,
) -> tauri::Result<Submenu<Wry>> {
    let mut owned = Vec::new();
    for (value, label) in entries {
        let id = format!("{prefix}:{value}");
        let item = CheckMenuItem::with_id(app, &id, label, true, false, None::<&str>)?;
        checks.push((id, item.clone()));
        owned.push(item);
    }
    let refs: Vec<&dyn IsMenuItem<Wry>> = owned.iter().map(|i| i as &dyn IsMenuItem<Wry>).collect();
    Submenu::with_id_and_items(app, prefix, title, true, &refs)
}

/// 建整个托盘菜单。
pub fn build_menu(app: &AppHandle) -> tauri::Result<MenuParts> {
    // 状态行默认置灰；缺权限时由 set_status_clickable 打开点击（直达系统设置）
    let status_item = MenuItem::with_id(app, "status", "状态：待机", false, None::<&str>)?;
    let stats_item = MenuItem::with_id(app, "stats", "统计：0 段 · 0 字", false, None::<&str>)?;
    let mut checks: Vec<(String, CheckMenuItem<Wry>)> = Vec::new();

    let history_empty =
        MenuItem::with_id(app, "history:none", "（暂无记录）", false, None::<&str>)?;
    let history_menu =
        Submenu::with_id_and_items(app, "history", "最近历史", true, &[&history_empty])?;
    // 风格列表含用户自定义，启动后由 rebuild_style_menu 填充
    let style_menu = Submenu::with_id_and_items(app, "style", "润色风格", true, &[])?;
    let hotkey_menu = check_submenu(
        app,
        &mut checks,
        "hotkey",
        "听写热键",
        HotkeyId::choices()
            .iter()
            .map(|k| (k.as_str().to_string(), k.display().to_string())),
    )?;
    let rewrite_menu = check_submenu(
        app,
        &mut checks,
        "rewrite",
        "改写热键",
        HotkeyId::choices()
            .iter()
            .map(|k| (k.as_str().to_string(), k.display().to_string())),
    )?;
    let mode_menu = check_submenu(
        app,
        &mut checks,
        "mode",
        "录音方式",
        [RecordMode::Toggle, RecordMode::Hold]
            .iter()
            .map(|m| (m.as_str().to_string(), m.display().to_string())),
    )?;

    let pause_item = CheckMenuItem::with_id(app, "pause", "暂停热键", true, false, None::<&str>)?;
    let polish_item = CheckMenuItem::with_id(app, "polish", "AI 润色", true, false, None::<&str>)?;
    let preview_item =
        CheckMenuItem::with_id(app, "preview", "实时预览悬浮窗", true, true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let sep3 = PredefinedMenuItem::separator(app)?;
    let settings_item = MenuItem::with_id(app, "settings", "设置…", true, None::<&str>)?;
    let json_item = MenuItem::with_id(app, "edit_json", "编辑完整配置(JSON)", true, None::<&str>)?;
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
        &preview_item,
        &polish_item,
        &pause_item,
        &sep2,
        &settings_item,
        &json_item,
        &reload_item,
        &log_item,
    ];
    if cfg!(target_os = "macos") {
        items.push(&perm_menu);
    }
    items.push(&sep3);
    items.push(&quit_item);
    let menu = Menu::with_items(app, &items)?;

    Ok(MenuParts {
        menu,
        status_item,
        stats_item,
        pause_item,
        polish_item,
        preview_item,
        history_menu,
        style_menu,
        checks,
    })
}
