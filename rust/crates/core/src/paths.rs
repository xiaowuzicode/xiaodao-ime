//! 数据目录与全局常量（1:1 移植 Python `xiaodao_ime/config.py`）。
//!
//! 与 Python 版的差异只有一处：Python 源码运行时把项目目录当数据目录（开发便利），
//! Rust 版一律使用系统标准位置，另给 `XIAODAO_HOME` 环境变量做开发/测试覆盖。
//! 目录布局保持不变，因此老用户（PyInstaller 版）的 settings.json / models / history 可直接继承：
//!
//! ```text
//! <base>/settings.json
//! <base>/models/SenseVoiceSmall-Q8_0.gguf
//! <base>/logs/xiaodao-ime.log
//! <base>/data/history.jsonl
//! ```

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};

/// 模型下载源（首次运行自动下载）。
pub const MODEL_REPO: &str = "handy-computer/SenseVoiceSmall-gguf";

/// 默认转写模型：SenseVoice Small，Q8_0 量化。可由环境变量 `XIAODAO_MODEL` 覆盖。
pub const DEFAULT_MODEL_FILENAME: &str = "SenseVoiceSmall-Q8_0.gguf";

/// 录音参数：transcribe.cpp 要求 16kHz 单声道 float32。
pub const SAMPLE_RATE: u32 = 16_000;
pub const CHANNELS: u16 = 1;

/// 防误触：按住时长小于该秒数的录音直接丢弃。
pub const MIN_HOLD_SECONDS: f64 = 0.4;

/// 双击热键进入「锁定录音」的两次按下间隔上限。
pub const DOUBLE_TAP_WINDOW: Duration = Duration::from_millis(350);

/// 粘贴后恢复原剪贴板的延迟。
pub const CLIPBOARD_RESTORE_DELAY: Duration = Duration::from_millis(400);

/// 应用数据目录名（`~/Library/Application Support/xiaodao-ime` 等）。
const APP_DIR_NAME: &str = "xiaodao-ime";

/// 转写模型文件名：环境变量 `XIAODAO_MODEL` 覆盖，缺省 [`DEFAULT_MODEL_FILENAME`]。
pub fn model_filename() -> String {
    non_empty_env("XIAODAO_MODEL").unwrap_or_else(|| DEFAULT_MODEL_FILENAME.to_string())
}

/// 转写后端：`auto` 自动挑选最佳设备（Apple Silicon 上为 Metal）。环境变量 `XIAODAO_BACKEND`。
pub fn transcribe_backend() -> String {
    non_empty_env("XIAODAO_BACKEND").unwrap_or_else(|| "auto".to_string())
}

/// 转写语言：`None` = 自动检测（SenseVoice 支持 zh/yue/en/ja/ko）；
/// 环境变量 `XIAODAO_LANGUAGE` 设为 `"zh"` 可强制中文。空串视同未设置（与 Python 的 `or None` 一致）。
pub fn transcribe_language() -> Option<String> {
    non_empty_env("XIAODAO_LANGUAGE")
}

fn non_empty_env(key: &str) -> Option<String> {
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => None,
    }
}

/// 全部数据路径。构造后 `models/` `logs/` `data/` 三个目录一定已存在。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// 数据根目录。
    pub base_dir: PathBuf,
    pub models_dir: PathBuf,
    pub logs_dir: PathBuf,
    /// `logs/xiaodao-ime.log`
    pub log_file: PathBuf,
    pub data_dir: PathBuf,
    /// `data/history.jsonl`
    pub history_file: PathBuf,
    /// `settings.json`（与 Python 版同路径，老用户无缝继承）
    pub settings_file: PathBuf,
    /// `models/<model_filename()>`
    pub model_path: PathBuf,
}

impl Paths {
    /// 解析数据根目录并创建子目录。
    ///
    /// 优先级：`XIAODAO_HOME` 环境变量 > 系统标准位置
    /// （macOS `~/Library/Application Support/xiaodao-ime`，Windows `%APPDATA%\xiaodao-ime`）。
    pub fn resolve() -> Result<Self> {
        Self::with_base(default_base_dir()?)
    }

    /// 指定根目录（测试与 `XIAODAO_HOME` 共用），同样会创建子目录。
    pub fn with_base(base_dir: impl Into<PathBuf>) -> Result<Self> {
        let base_dir = base_dir.into();
        let paths = Self::layout(base_dir);
        for dir in [&paths.models_dir, &paths.logs_dir, &paths.data_dir] {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("创建目录失败：{}", dir.display()))?;
        }
        Ok(paths)
    }

    /// 只算路径、不碰磁盘（日志/诊断用）。
    pub fn layout(base_dir: impl Into<PathBuf>) -> Self {
        let base_dir = base_dir.into();
        let models_dir = base_dir.join("models");
        let logs_dir = base_dir.join("logs");
        let data_dir = base_dir.join("data");
        Self {
            log_file: logs_dir.join("xiaodao-ime.log"),
            history_file: data_dir.join("history.jsonl"),
            settings_file: base_dir.join("settings.json"),
            model_path: models_dir.join(model_filename()),
            models_dir,
            logs_dir,
            data_dir,
            base_dir,
        }
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }
}

/// 系统标准数据目录（不创建）。
fn default_base_dir() -> Result<PathBuf> {
    if let Some(home) = non_empty_env("XIAODAO_HOME") {
        return Ok(PathBuf::from(home));
    }
    // macOS: ~/Library/Application Support；Windows: %APPDATA%（Roaming）。
    if let Some(dirs) = directories::BaseDirs::new() {
        return Ok(dirs.data_dir().join(APP_DIR_NAME));
    }
    // 兜底：directories 拿不到 HOME 时退回环境变量。
    #[cfg(target_os = "windows")]
    let fallback = std::env::var("APPDATA").ok().map(PathBuf::from);
    #[cfg(not(target_os = "windows"))]
    let fallback = std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join("Library").join("Application Support"));
    fallback
        .map(|p| p.join(APP_DIR_NAME))
        .context("无法确定用户数据目录，请设置 XIAODAO_HOME 环境变量")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_python_config() {
        let p = Paths::layout("/tmp/xd-base");
        assert_eq!(p.models_dir, PathBuf::from("/tmp/xd-base/models"));
        assert_eq!(p.logs_dir, PathBuf::from("/tmp/xd-base/logs"));
        assert_eq!(p.data_dir, PathBuf::from("/tmp/xd-base/data"));
        assert_eq!(p.log_file, PathBuf::from("/tmp/xd-base/logs/xiaodao-ime.log"));
        assert_eq!(
            p.history_file,
            PathBuf::from("/tmp/xd-base/data/history.jsonl")
        );
        assert_eq!(
            p.settings_file,
            PathBuf::from("/tmp/xd-base/settings.json")
        );
        assert!(p.model_path.starts_with("/tmp/xd-base/models"));
    }

    #[test]
    fn with_base_creates_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Paths::with_base(tmp.path().join("nested")).unwrap();
        assert!(p.models_dir.is_dir());
        assert!(p.logs_dir.is_dir());
        assert!(p.data_dir.is_dir());
        // 幂等：再来一次不报错
        assert!(Paths::with_base(tmp.path().join("nested")).is_ok());
    }

    #[test]
    fn constants_match_python() {
        assert_eq!(SAMPLE_RATE, 16_000);
        assert_eq!(CHANNELS, 1);
        assert!((MIN_HOLD_SECONDS - 0.4).abs() < f64::EPSILON);
        assert_eq!(DOUBLE_TAP_WINDOW.as_millis(), 350);
        assert_eq!(CLIPBOARD_RESTORE_DELAY.as_millis(), 400);
        assert_eq!(MODEL_REPO, "handy-computer/SenseVoiceSmall-gguf");
    }
}
