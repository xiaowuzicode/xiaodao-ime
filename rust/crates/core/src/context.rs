//! 场景感知润色：前台应用 → 润色风格匹配（1:1 移植 Python `xiaodao_ime/context.py`）。
//!
//! settings.json 的 `app_styles` 示例（macOS 键为 bundle id 或应用名；
//! Windows 键为进程 exe 名，如 `"WeChat"`、`"Code"`）：
//!
//! ```json
//! "app_styles": {
//!   "com.tencent.xinWeChat": "轻度纠错",
//!   "Mail": "书面化",
//!   "Terminal": "关闭"
//! }
//! ```
//!
//! 取前台应用本身由 [`crate::platform::Platform::frontmost_app`] 负责，本模块只做纯匹配。

use std::collections::BTreeMap;

/// 表示「该 App 不润色」的风格名。
pub const STYLE_OFF: [&str; 2] = ["关闭", "off"];

/// 风格名是否等于「关闭」。与 Python 的 `style in STYLE_OFF` 一致（精确匹配）。
pub fn is_style_off(style: &str) -> bool {
    STYLE_OFF.contains(&style)
}

/// 按 `app_styles` 匹配风格：应用标识（bundle id / exe 名）精确优先，其次应用名（大小写不敏感）。
///
/// 返回风格名（可能是「关闭」），无匹配返回 `None`（走全局默认风格）。
pub fn pick_style(
    app_name: &str,
    app_id: &str,
    mapping: &BTreeMap<String, String>,
) -> Option<String> {
    if mapping.is_empty() {
        return None;
    }
    if !app_id.is_empty() {
        if let Some(style) = mapping.get(app_id) {
            return Some(style.clone());
        }
    }
    if app_name.is_empty() {
        return None;
    }
    let lowered = app_name.to_lowercase();
    mapping
        .iter()
        .find(|(key, _)| !key.is_empty() && key.to_lowercase() == lowered)
        .map(|(_, value)| value.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping() -> BTreeMap<String, String> {
        [
            ("com.tencent.xinWeChat", "轻度纠错"),
            ("Mail", "书面化"),
            ("Terminal", "关闭"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    /// 移植自 Python `test_polish.py::test_pick_style`。
    #[test]
    fn pick_style_ported() {
        let m = mapping();
        // 应用标识优先
        assert_eq!(
            pick_style("微信", "com.tencent.xinWeChat", &m).as_deref(),
            Some("轻度纠错")
        );
        // 应用名忽略大小写
        assert_eq!(
            pick_style("mail", "com.apple.mail", &m).as_deref(),
            Some("书面化")
        );
        assert_eq!(pick_style("Terminal", "", &m).as_deref(), Some("关闭"));
        // 无匹配走默认
        assert_eq!(pick_style("Safari", "com.apple.Safari", &m), None);
        assert_eq!(pick_style("Mail", "x", &BTreeMap::new()), None);
    }

    #[test]
    fn style_off() {
        assert!(is_style_off("关闭"));
        assert!(is_style_off("off"));
        assert!(!is_style_off("润色"));
        assert!(!is_style_off(""));
    }
}
