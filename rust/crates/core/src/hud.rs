//! HUD 组合逻辑辅助：纯函数，无状态、无 IO、无平台依赖。
//!
//! 悬浮窗本体（计时 + canvas 声浪 + 文本两行）由 Tauri 前端绘制，核心层只通过
//! [`crate::types::Hud`] trait 把「已录秒数 / 电平 / 伪流式文本 / 状态文案」推过去。
//! 本模块负责那些**跨 UI 实现都一样**的文本组合规则，对应 Python `xiaodao_ime/hud.py`
//! 里的 `_tail` / `HotkeyController._hint_text` / `_start_preview` 的 placeholder。

use crate::keys::RecordMode;
use crate::types::Channel;

/// 主行伪流式识别文本的尾部字符上限（Python `hud.py::_MAX_TAIL`）。
pub const MAX_TAIL: usize = 24;
/// 状态副行（如润色期间的已转写全文）尾部字符上限（Python `hud.py::_MAX_DETAIL`）。
pub const MAX_DETAIL: usize = 40;

/// 取文本尾部 `limit` 个字符，超长时前置省略号。
///
/// 与 Python `_tail` 逐行对应：空串原样返回；长度（**字符数**，非字节数）不超过
/// 上限时原样返回；否则返回 `…` + 末尾 `limit` 个字符。
pub fn tail(text: &str, limit: usize) -> String {
    if text.is_empty() {
        return String::new();
    }
    let total = text.chars().count();
    if total <= limit {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 3);
    out.push('…');
    out.extend(text.chars().skip(total - limit));
    out
}

/// 取文本开头 `limit` 个字符（改写指令回显用，对应 Python `instruction[:24]`）。
pub fn head(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// HUD 副行操作提示：告诉用户怎么结束、怎么取消（交互可发现性）。
///
/// 对应 Python `HotkeyController._hint_text`：toggle 方式或已进入锁定录音时都是
/// 「再按一下热键」结束，只有 hold 方式的普通按住才是「松开出字」。
pub fn hint_text(mode: RecordMode, locked: bool) -> &'static str {
    if mode == RecordMode::Toggle || locked {
        "再按热键出字 · 按 Esc 取消"
    } else {
        "松开出字 · 快速双击可锁定 · 按 Esc 取消"
    }
}

/// 录音刚开始、还没有任何识别结果时主行显示的占位文案。
pub fn placeholder(channel: Channel) -> &'static str {
    match channel {
        Channel::Rewrite => "说出改写指令…",
        Channel::Dictate => "聆听中…",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_keeps_short_text() {
        assert_eq!(tail("", 5), "");
        assert_eq!(tail("小岛", 5), "小岛");
        assert_eq!(tail("12345", 5), "12345");
    }

    #[test]
    fn tail_truncates_by_chars_not_bytes() {
        // 中文按字符计数：6 字取尾 3 字 => 「…四五六」
        assert_eq!(tail("一二三四五六", 3), "…四五六");
        assert_eq!(tail("abcdef", 3), "…def");
    }

    #[test]
    fn head_takes_prefix_by_chars() {
        assert_eq!(head("改成英文再润色一下", 4), "改成英文");
        assert_eq!(head("短", 24), "短");
        assert_eq!(head("", 3), "");
    }

    #[test]
    fn hint_follows_mode_and_lock() {
        assert_eq!(
            hint_text(RecordMode::Toggle, false),
            "再按热键出字 · 按 Esc 取消"
        );
        assert_eq!(
            hint_text(RecordMode::Toggle, true),
            "再按热键出字 · 按 Esc 取消"
        );
        assert_eq!(
            hint_text(RecordMode::Hold, true),
            "再按热键出字 · 按 Esc 取消"
        );
        assert_eq!(
            hint_text(RecordMode::Hold, false),
            "松开出字 · 快速双击可锁定 · 按 Esc 取消"
        );
    }

    #[test]
    fn placeholder_differs_per_channel() {
        assert_eq!(placeholder(Channel::Dictate), "聆听中…");
        assert_eq!(placeholder(Channel::Rewrite), "说出改写指令…");
    }
}
