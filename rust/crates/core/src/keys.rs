//! 热键标识与录音方式（settings.json 字段值的类型化表示）。
//!
//! settings.json 里 `hotkey` / `rewrite_hotkey` 的取值与 Python 版完全一致：
//! macOS：`alt_l` 左 Option（默认听写）/ `alt_r` 右 Option（默认改写）/ `cmd_r` / `f19`
//! Windows：`ctrl_r` 右 Ctrl（默认听写）/ `f8`（默认改写）/ `f9` / `alt_r`
//!
//! rdev 键值到 [`HotkeyId`] 的映射在 `listener.rs`（平台相关键码只在那里出现）。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyId {
    AltL,
    AltR,
    CmdR,
    F19,
    CtrlR,
    F8,
    F9,
}

impl HotkeyId {
    /// settings.json 里的字符串值。
    pub fn as_str(&self) -> &'static str {
        match self {
            HotkeyId::AltL => "alt_l",
            HotkeyId::AltR => "alt_r",
            HotkeyId::CmdR => "cmd_r",
            HotkeyId::F19 => "f19",
            HotkeyId::CtrlR => "ctrl_r",
            HotkeyId::F8 => "f8",
            HotkeyId::F9 => "f9",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "alt_l" => HotkeyId::AltL,
            "alt_r" => HotkeyId::AltR,
            "cmd_r" => HotkeyId::CmdR,
            "f19" => HotkeyId::F19,
            "ctrl_r" => HotkeyId::CtrlR,
            "f8" => HotkeyId::F8,
            "f9" => HotkeyId::F9,
            _ => return None,
        })
    }

    /// 菜单/设置页显示名。
    pub fn display(&self) -> &'static str {
        match self {
            HotkeyId::AltL => "左 Option",
            HotkeyId::AltR => {
                if cfg!(target_os = "macos") {
                    "右 Option"
                } else {
                    "右 Alt（AltGr 键盘慎用）"
                }
            }
            HotkeyId::CmdR => "右 Command",
            HotkeyId::F19 => "F19",
            HotkeyId::CtrlR => "右 Ctrl",
            HotkeyId::F8 => "F8",
            HotkeyId::F9 => "F9",
        }
    }

    /// 当前平台可选的热键列表（顺序即菜单顺序）。
    pub fn choices() -> &'static [HotkeyId] {
        if cfg!(target_os = "macos") {
            &[HotkeyId::AltL, HotkeyId::AltR, HotkeyId::CmdR, HotkeyId::F19]
        } else {
            &[HotkeyId::CtrlR, HotkeyId::F8, HotkeyId::F9, HotkeyId::AltR]
        }
    }

    pub fn default_dictate() -> HotkeyId {
        if cfg!(target_os = "macos") {
            HotkeyId::AltL
        } else {
            HotkeyId::CtrlR
        }
    }

    pub fn default_rewrite() -> HotkeyId {
        if cfg!(target_os = "macos") {
            HotkeyId::AltR
        } else {
            HotkeyId::F8
        }
    }

    /// 不在当前平台选项里的值回退到默认（settings.json 跨平台拷贝时用）。
    pub fn valid_or_default_dictate(self) -> HotkeyId {
        if Self::choices().contains(&self) {
            self
        } else {
            Self::default_dictate()
        }
    }

    pub fn valid_or_default_rewrite(self) -> HotkeyId {
        if Self::choices().contains(&self) {
            self
        } else {
            Self::default_rewrite()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordMode {
    /// 单击开始 / 再击结束（默认）
    #[default]
    Toggle,
    /// 按住说话；0.35s 内双击进入锁定录音
    Hold,
}

impl RecordMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            RecordMode::Toggle => "toggle",
            RecordMode::Hold => "hold",
        }
    }

    pub fn display(&self) -> &'static str {
        match self {
            RecordMode::Toggle => "单击开始 / 再击结束",
            RecordMode::Hold => "按住说话（双击锁定）",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "toggle" => Some(RecordMode::Toggle),
            "hold" => Some(RecordMode::Hold),
            _ => None,
        }
    }
}

/// 键盘事件抽象：状态机只关心「是不是热键、按下还是松开」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    Hotkey(HotkeyId),
    /// 任何非热键按键（含 Esc、字母、其他修饰键）
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: KeyKind,
    pub pressed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_str() {
        for id in [
            HotkeyId::AltL,
            HotkeyId::AltR,
            HotkeyId::CmdR,
            HotkeyId::F19,
            HotkeyId::CtrlR,
            HotkeyId::F8,
            HotkeyId::F9,
        ] {
            assert_eq!(HotkeyId::parse(id.as_str()), Some(id));
            let json = serde_json::to_string(&id).unwrap();
            assert_eq!(json, format!("\"{}\"", id.as_str()));
        }
        assert_eq!(HotkeyId::parse("nope"), None);
    }

    #[test]
    fn defaults_are_valid_choices() {
        assert!(HotkeyId::choices().contains(&HotkeyId::default_dictate()));
        assert!(HotkeyId::choices().contains(&HotkeyId::default_rewrite()));
        assert_ne!(HotkeyId::default_dictate(), HotkeyId::default_rewrite());
    }
}
