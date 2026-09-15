//! settings.json 读写（1:1 移植 Python `xiaodao_ime/settings.py`）。
//!
//! **schema 与 Python 版完全一致**（键名、默认值），老用户的 settings.json 直接可读。
//! Python 用「默认值深合并」实现「用户只写想改的键」，Rust 用 `#[serde(default = ...)]`
//! 逐字段实现等价语义；用户自定义的未知键由 `extra`（`#[serde(flatten)]`）原样保留，
//! 写回时不会丢失。
//!
//! 解析失败（JSON 语法错误、类型不匹配）整体回退默认值并 warn——与 Python 行为一致。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use tracing::{error, info, warn};

use crate::keys::{HotkeyId, RecordMode};

fn default_true() -> bool {
    true
}
fn default_provider() -> String {
    "openai".to_string()
}
fn default_model() -> String {
    "deepseek-chat".to_string()
}
fn default_base_url() -> String {
    "https://api.deepseek.com".to_string()
}
fn default_timeout() -> u64 {
    30
}
fn default_style() -> String {
    // 单一真相：默认风格名只在 polisher 里定义一次
    crate::polisher::DEFAULT_STYLE.to_string()
}
fn default_max_items() -> usize {
    50
}

/// LLM 润色配置（settings.json 的 `polish`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolishConfig {
    /// 听写润色总开关（不影响语音改写）。
    #[serde(default)]
    pub enabled: bool,
    /// `openai`（任意 OpenAI 兼容端点）/ `anthropic`。
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_base_url")]
    pub base_url: String,
    /// 单次请求超时（秒）。
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    /// 全局默认风格名。
    #[serde(default = "default_style")]
    pub style: String,
    /// 自定义风格：`{"风格名": "system 提示词"}`，同名覆盖内置。
    #[serde(default)]
    pub styles: BTreeMap<String, String>,
    /// 用户自定义的未知键，原样保留。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for PolishConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: default_provider(),
            model: default_model(),
            api_key: String::new(),
            base_url: default_base_url(),
            timeout: default_timeout(),
            style: default_style(),
            styles: BTreeMap::new(),
            extra: Map::new(),
        }
    }
}

/// 输入历史配置（settings.json 的 `history`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 内存中保留的最近条数（jsonl 全量落盘，不受此限）。
    #[serde(default = "default_max_items")]
    pub max_items: usize,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_items: default_max_items(),
            extra: Map::new(),
        }
    }
}

/// settings.json 的内存镜像。字段顺序即写回时的键顺序（与 Python DEFAULTS 一致）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// 听写热键。
    #[serde(default = "HotkeyId::default_dictate", deserialize_with = "de_dictate")]
    pub hotkey: HotkeyId,
    /// 语音改写热键（选中文字后按它说指令）。
    #[serde(default = "HotkeyId::default_rewrite", deserialize_with = "de_rewrite")]
    pub rewrite_hotkey: HotkeyId,
    /// 录音方式：toggle 单击开始再击结束 / hold 按住说话 + 双击锁定。
    #[serde(default, deserialize_with = "de_record_mode")]
    pub record_mode: RecordMode,
    /// 录音时悬浮窗实时预览识别文本。
    #[serde(default = "default_true")]
    pub live_preview: bool,
    #[serde(default)]
    pub polish: PolishConfig,
    /// 场景感知润色：前台 App 标识/应用名 → 风格名（或「关闭」）。
    #[serde(default)]
    pub app_styles: BTreeMap<String, String>,
    /// 录音开始/结束提示音。
    #[serde(default = "default_true")]
    pub sounds: bool,
    #[serde(default)]
    pub history: HistoryConfig,
    /// 热词（人名/产品名/术语）：注入润色提示词。
    #[serde(default)]
    pub hotwords: Vec<String>,
    /// 离线替换表：转写后无条件字符串替换，如 `{"欧朵": "Ordo"}`。
    #[serde(default)]
    pub replacements: BTreeMap<String, String>,
    /// 用户自定义的未知键，原样保留。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: HotkeyId::default_dictate(),
            rewrite_hotkey: HotkeyId::default_rewrite(),
            record_mode: RecordMode::default(),
            live_preview: true,
            polish: PolishConfig::default(),
            app_styles: BTreeMap::new(),
            sounds: true,
            history: HistoryConfig::default(),
            hotwords: Vec::new(),
            replacements: BTreeMap::new(),
            extra: Map::new(),
        }
    }
}

/// 热键值非法或不属于当前平台时回退默认（settings.json 跨平台拷贝时用）。
fn de_dictate<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<HotkeyId, D::Error> {
    Ok(de_hotkey(d, true))
}

fn de_rewrite<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<HotkeyId, D::Error> {
    Ok(de_hotkey(d, false))
}

fn de_hotkey<'de, D: Deserializer<'de>>(d: D, dictate: bool) -> HotkeyId {
    let raw = String::deserialize(d).unwrap_or_default();
    let parsed = HotkeyId::parse(&raw);
    if parsed.is_none() && !raw.is_empty() {
        warn!("未知热键 {:?}，回退默认", raw);
    }
    let id = parsed.unwrap_or_else(|| {
        if dictate {
            HotkeyId::default_dictate()
        } else {
            HotkeyId::default_rewrite()
        }
    });
    if dictate {
        id.valid_or_default_dictate()
    } else {
        id.valid_or_default_rewrite()
    }
}

fn de_record_mode<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<RecordMode, D::Error> {
    let raw = String::deserialize(d).unwrap_or_default();
    Ok(RecordMode::parse(&raw).unwrap_or_else(|| {
        if !raw.is_empty() {
            warn!("未知录音方式 {:?}，回退默认", raw);
        }
        RecordMode::default()
    }))
}

impl Settings {
    /// 解析 JSON 文本；失败返回 Err（调用方决定是否回退默认）。
    pub fn from_json(text: &str) -> Result<Self> {
        Ok(serde_json::from_str(text)?)
    }

    /// 序列化为写盘文本（2 空格缩进 + 末尾换行，非 ASCII 不转义，与 Python 一致）。
    pub fn to_json(&self) -> Result<String> {
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        Ok(text)
    }

    /// 两个热键相同时把改写热键让回默认（Python 版靠文档约定，这里做兜底）。
    pub fn hotkeys_conflict(&self) -> bool {
        self.hotkey == self.rewrite_hotkey
    }
}

/// `settings.json` 的共享句柄：内部 `Arc<RwLock<Settings>>`，可自由 clone 给各线程。
#[derive(Clone)]
pub struct SettingsStore {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    data: RwLock<Settings>,
}

impl SettingsStore {
    /// 读取 settings.json；文件不存在或解析失败一律用默认值（不报错，与 Python 一致）。
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let data = read_or_default(&path);
        Self {
            inner: Arc::new(Inner {
                path,
                data: RwLock::new(data),
            }),
        }
    }

    /// 纯内存实例（测试/无盘场景），`save` 仍会写到给定路径。
    pub fn from_settings(path: impl Into<PathBuf>, settings: Settings) -> Self {
        Self {
            inner: Arc::new(Inner {
                path: path.into(),
                data: RwLock::new(settings),
            }),
        }
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// 取当前设置的快照（克隆，读锁只在克隆期间持有）。
    pub fn get(&self) -> Settings {
        self.inner.data.read().clone()
    }

    /// 就地修改（不落盘，需要持久化时自行调用 [`SettingsStore::save`]）。
    pub fn update<R>(&self, f: impl FnOnce(&mut Settings) -> R) -> R {
        let mut guard = self.inner.data.write();
        f(&mut guard)
    }

    /// 修改并立刻落盘。
    pub fn update_and_save<R>(&self, f: impl FnOnce(&mut Settings) -> R) -> R {
        let out = self.update(f);
        if let Err(e) = self.save() {
            error!("保存设置失败：{e:#}");
        }
        out
    }

    /// 写回磁盘。
    pub fn save(&self) -> Result<()> {
        let text = self.inner.data.read().to_json()?;
        if let Some(parent) = self.inner.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建目录失败：{}", parent.display()))?;
        }
        std::fs::write(&self.inner.path, text)
            .with_context(|| format!("写入失败：{}", self.inner.path.display()))?;
        info!("设置已保存：{}", self.inner.path.display());
        Ok(())
    }

    /// 从磁盘重新加载（外部编辑 settings.json 后调用）。
    pub fn reload(&self) {
        let fresh = read_or_default(&self.inner.path);
        *self.inner.data.write() = fresh;
    }

    /// 确保 settings.json 存在（用于「打开配置文件」菜单），返回路径。
    pub fn ensure_file(&self) -> PathBuf {
        if !self.inner.path.is_file() {
            if let Err(e) = self.save() {
                error!("创建 settings.json 失败：{e:#}");
            }
        }
        self.inner.path.clone()
    }
}

impl std::fmt::Debug for SettingsStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsStore")
            .field("path", &self.inner.path)
            .finish()
    }
}

fn read_or_default(path: &Path) -> Settings {
    if !path.is_file() {
        return Settings::default();
    }
    match std::fs::read_to_string(path) {
        Ok(text) => match Settings::from_json(&text) {
            Ok(s) => {
                info!("已加载设置：{}", path.display());
                s
            }
            Err(e) => {
                error!("settings.json 解析失败，使用默认设置：{e}");
                Settings::default()
            }
        },
        Err(e) => {
            warn!("settings.json 读取失败，使用默认设置：{e}");
            Settings::default()
        }
    }
}

/// 热键状态机只读视图：每次调用读取当前快照，「重新加载配置」后立即生效。
impl crate::hotkey::SettingsView for SettingsStore {
    fn live_preview(&self) -> bool {
        self.get().live_preview
    }
    fn sounds(&self) -> bool {
        self.get().sounds
    }
    fn replacements(&self) -> std::collections::BTreeMap<String, String> {
        self.get().replacements
    }
    fn app_styles(&self) -> std::collections::BTreeMap<String, String> {
        self.get().app_styles
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_settings(json: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, json).unwrap();
        (dir, path)
    }

    #[test]
    fn missing_file_uses_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::load(dir.path().join("settings.json"));
        assert_eq!(store.get(), Settings::default());
        assert_eq!(store.get().polish.provider, "openai");
        assert_eq!(store.get().polish.timeout, 30);
        assert!(store.get().history.enabled);
        assert_eq!(store.get().history.max_items, 50);
        assert!(store.get().live_preview);
        assert!(store.get().sounds);
    }

    #[test]
    fn empty_or_broken_file_falls_back_to_defaults() {
        for bad in ["", "   ", "{", "null", "[1,2]"] {
            let (_d, path) = tmp_settings(bad);
            assert_eq!(
                SettingsStore::load(&path).get(),
                Settings::default(),
                "bad json: {bad:?}"
            );
        }
    }

    /// 移植自 Python `test_polish.py::test_settings_merge_and_save`。
    #[test]
    fn partial_json_merges_defaults_and_saves() {
        let (_d, path) = tmp_settings(r#"{"hotkey": "f19", "polish": {"enabled": true}}"#);
        let store = SettingsStore::load(&path);
        let s = store.get();
        // f19 只在 macOS 选项里；其它平台回退默认（跨平台拷贝语义）
        assert_eq!(s.hotkey, HotkeyId::F19.valid_or_default_dictate());
        assert!(s.polish.enabled);
        assert_eq!(s.polish.provider, PolishConfig::default().provider);
        assert_eq!(s.polish.model, PolishConfig::default().model);

        store.update(|s| s.hotkey = HotkeyId::default_rewrite().valid_or_default_dictate());
        store.save().unwrap();
        assert_eq!(
            SettingsStore::load(&path).get().hotkey,
            HotkeyId::default_rewrite().valid_or_default_dictate()
        );
    }

    /// 老用户的 settings.example.json 能直接读，且未知键写回不丢。
    #[test]
    fn legacy_example_json_and_unknown_keys() {
        let json = r#"{
  "hotkey": "alt_l",
  "polish": {
    "enabled": false,
    "provider": "openai",
    "model": "deepseek-chat",
    "api_key": "sk-你的key",
    "base_url": "https://api.deepseek.com",
    "timeout": 30,
    "my_future_polish_key": 1
  },
  "app_styles": {
    "com.tencent.xinWeChat": "轻度纠错",
    "Mail": "书面化",
    "Terminal": "关闭"
  },
  "hotwords": ["Ordo", "小岛AI", "SenseVoice"],
  "replacements": {"欧朵": "Ordo"},
  "my_custom_key": {"a": 1}
}"#;
        let (_d, path) = tmp_settings(json);
        let store = SettingsStore::load(&path);
        let s = store.get();
        assert_eq!(s.hotkey, HotkeyId::AltL.valid_or_default_dictate());
        assert_eq!(s.polish.api_key, "sk-你的key");
        assert_eq!(s.polish.base_url, "https://api.deepseek.com");
        assert_eq!(s.polish.timeout, 30);
        // example.json 里没有的键取默认
        assert_eq!(s.polish.style, "润色");
        assert_eq!(s.record_mode, RecordMode::Toggle);
        assert_eq!(s.hotwords, vec!["Ordo", "小岛AI", "SenseVoice"]);
        assert_eq!(s.replacements.get("欧朵").unwrap(), "Ordo");
        assert_eq!(s.app_styles.get("Terminal").unwrap(), "关闭");
        // 未知键保留
        assert!(s.extra.contains_key("my_custom_key"));
        assert!(s.polish.extra.contains_key("my_future_polish_key"));

        // save 后重读一致，且未知键仍在
        store.save().unwrap();
        let reread = SettingsStore::load(&path).get();
        assert_eq!(reread, s);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("my_custom_key"));
        assert!(text.contains("my_future_polish_key"));
        assert!(text.contains("小岛AI"), "非 ASCII 不应被转义");
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn unknown_hotkey_and_mode_fall_back() {
        let (_d, path) =
            tmp_settings(r#"{"hotkey": "nope", "rewrite_hotkey": "nah", "record_mode": "weird"}"#);
        let s = SettingsStore::load(&path).get();
        assert_eq!(s.hotkey, HotkeyId::default_dictate());
        assert_eq!(s.rewrite_hotkey, HotkeyId::default_rewrite());
        assert_eq!(s.record_mode, RecordMode::Toggle);
    }

    #[test]
    fn ensure_file_and_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("settings.json");
        let store = SettingsStore::load(&path);
        assert!(!path.is_file());
        assert_eq!(store.ensure_file(), path);
        assert!(path.is_file());

        std::fs::write(&path, r#"{"sounds": false}"#).unwrap();
        assert!(store.get().sounds, "reload 前仍是内存旧值");
        store.reload();
        assert!(!store.get().sounds);
    }
}
