//! 输入历史（1:1 移植 Python `xiaodao_ime/history.py`）。
//!
//! 本地 `data/history.jsonl` 追加存储，托盘菜单展示最近 N 条并可点击复制。
//! 线程模型：`append` 来自转写 worker 线程，`recent` / `version` 由 UI 线程读取，
//! 内部用 Mutex 保护内存列表；`version` 自增供 UI 判断是否需要重绘。

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::settings::SettingsStore;

/// 一条历史记录。`final` 是 JSON 键名（Rust 关键字，故字段名为 `final_text`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// 本地时间 `%Y-%m-%d %H:%M:%S`。
    pub ts: String,
    /// 原始转写文本。
    #[serde(default)]
    pub raw: String,
    /// 实际出字文本（替换表 + 润色之后）。
    #[serde(rename = "final", default)]
    pub final_text: String,
}

#[derive(Default)]
struct State {
    items: VecDeque<HistoryEntry>,
    version: u64,
    /// 累计出字次数（全量，含已滚出内存的旧记录）
    total_count: u64,
    /// 累计出字字数
    total_chars: u64,
}

pub struct History {
    path: PathBuf,
    store: SettingsStore,
    state: Mutex<State>,
}

impl History {
    /// 读取 jsonl（不存在则空），统计全量计数、内存只保留最近 `history.max_items` 条。
    pub fn load(path: impl Into<PathBuf>, store: SettingsStore) -> Self {
        let history = Self {
            path: path.into(),
            store,
            state: Mutex::new(State::default()),
        };
        history.load_from_disk();
        history
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `history.enabled`
    pub fn enabled(&self) -> bool {
        self.store.get().history.enabled
    }

    fn max_items(&self) -> usize {
        self.store.get().history.max_items
    }

    fn load_from_disk(&self) {
        if !self.path.is_file() {
            return;
        }
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) => {
                warn!("加载历史失败：{e}");
                return;
            }
        };
        let max_items = self.max_items();
        let mut state = self.state.lock();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            match serde_json::from_str::<HistoryEntry>(line) {
                Ok(entry) => {
                    state.total_count += 1;
                    state.total_chars += entry.final_text.chars().count() as u64;
                    state.items.push_back(entry);
                    while state.items.len() > max_items {
                        state.items.pop_front();
                    }
                }
                Err(_) => continue, // 坏行跳过，不影响其它记录
            }
        }
        state.version += 1;
        info!(
            "已加载历史 {} 条（累计 {} 次 / {} 字）",
            state.items.len(),
            state.total_count,
            state.total_chars
        );
    }

    /// 追加一条；`history.enabled` 关闭或出字为空时忽略。
    pub fn append(&self, raw: &str, final_text: &str) {
        if !self.enabled() || final_text.trim().is_empty() {
            return;
        }
        let entry = HistoryEntry {
            ts: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            raw: raw.to_string(),
            final_text: final_text.to_string(),
        };
        let max_items = self.max_items();
        {
            let mut state = self.state.lock();
            state.items.push_back(entry.clone());
            while state.items.len() > max_items {
                state.items.pop_front();
            }
            state.total_count += 1;
            state.total_chars += final_text.chars().count() as u64;
            state.version += 1;
        }
        if let Err(e) = self.write_line(&entry) {
            warn!("写入历史失败：{e:#}");
        }
    }

    fn write_line(&self, entry: &HistoryEntry) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{}", serde_json::to_string(entry)?)?;
        Ok(())
    }

    /// 最近 n 条，新 → 旧。
    pub fn recent(&self, n: usize) -> Vec<HistoryEntry> {
        let state = self.state.lock();
        state.items.iter().rev().take(n).cloned().collect()
    }

    /// 累计出字次数（含已滚出内存的旧记录）。
    pub fn total_count(&self) -> u64 {
        self.state.lock().total_count
    }

    /// 累计出字字数（按 Unicode 字符计，与 Python `len(str)` 一致）。
    pub fn total_chars(&self) -> u64 {
        self.state.lock().total_chars
    }

    /// 自增版本号：UI 据此判断是否需要重绘菜单。
    pub fn version(&self) -> u64 {
        self.state.lock().version
    }
}

impl crate::hotkey::HistorySink for History {
    fn append(&self, raw: &str, final_text: &str) {
        History::append(self, raw, final_text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    fn store() -> SettingsStore {
        SettingsStore::from_settings("/dev/null/settings.json", Settings::default())
    }

    /// 移植自 Python `test_polish.py::test_history`。
    #[test]
    fn history_ported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let h = History::load(&path, store());
        let v0 = h.version();
        h.append("原始 文本", "润色后的文本");
        assert_eq!(h.version(), v0 + 1);
        assert_eq!(h.recent(5)[0].final_text, "润色后的文本");
        assert_eq!(h.recent(5)[0].raw, "原始 文本");

        // 重新加载能读回
        let h2 = History::load(&path, store());
        assert_eq!(h2.recent(5)[0].final_text, "润色后的文本");

        // 空文本不入库
        h.append("x", "   ");
        assert_eq!(h.recent(10).len(), 1);

        // 累计统计（重载后仍保留全量计数）
        assert_eq!(h.total_count(), 1);
        assert_eq!(h.total_chars(), "润色后的文本".chars().count() as u64);
        let h3 = History::load(&path, store());
        assert_eq!(h3.total_count(), 1);
        assert_eq!(h3.total_chars(), "润色后的文本".chars().count() as u64);
    }

    #[test]
    fn disabled_skips_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let store = store();
        store.update(|s| s.history.enabled = false);
        let h = History::load(&path, store);
        h.append("raw", "final");
        assert!(h.recent(5).is_empty());
        assert!(!path.is_file());
    }

    #[test]
    fn rolls_at_max_items_but_keeps_totals() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let store = store();
        store.update(|s| s.history.max_items = 3);
        let h = History::load(&path, store.clone());
        for i in 0..5 {
            h.append("raw", &format!("第{i}条"));
        }
        let recent = h.recent(10);
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].final_text, "第4条"); // 新 → 旧
        assert_eq!(recent[2].final_text, "第2条");
        assert_eq!(h.total_count(), 5);
        // jsonl 全量落盘，重载后计数不丢
        let h2 = History::load(&path, store);
        assert_eq!(h2.total_count(), 5);
        assert_eq!(h2.recent(10).len(), 3);
    }

    #[test]
    fn bad_lines_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        std::fs::write(
            &path,
            "{\"ts\":\"2026-01-01 00:00:00\",\"raw\":\"a\",\"final\":\"好\"}\n坏行\n\n",
        )
        .unwrap();
        let h = History::load(&path, store());
        assert_eq!(h.total_count(), 1);
        assert_eq!(h.total_chars(), 1);
        assert_eq!(h.recent(5)[0].final_text, "好");
    }

    /// worker 线程 append / UI 线程 recent，必须 Send + Sync。
    #[test]
    fn history_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<History>();
    }

    #[test]
    fn json_key_is_final() {
        let entry = HistoryEntry {
            ts: "2026-01-01 00:00:00".into(),
            raw: "r".into(),
            final_text: "f".into(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"final\":\"f\""), "{json}");
        assert!(!json.contains("final_text"));
    }
}
