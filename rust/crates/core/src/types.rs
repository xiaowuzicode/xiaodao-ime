//! 核心层共享抽象：状态、事件、以及 UI/平台/引擎的 trait 边界。
//!
//! 这些 trait 是各模块之间唯一的耦合点，也是单测的注入点（假录音器、假平台、假 HUD）。
//! **改这里等于改架构**：需要新增方法时先在 docs/rust-migration.md 里登记再动。

use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// 托盘/菜单栏展示的整体状态（对应 Python `_on_status`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Idle,
    Recording,
    Transcribing,
    Polishing,
    Paused,
}

/// 提示音事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundEvent {
    Start,
    Stop,
    Cancel,
}

/// 录音通道：听写（转写→润色→粘贴）/ 语音改写（选区+指令→LLM→原地替换）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Dictate,
    Rewrite,
}

/// macOS TCC 权限自检结果；Windows 恒为 true。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Permissions {
    pub input_monitoring: bool,
    pub accessibility: bool,
}

impl Permissions {
    pub fn all_granted(&self) -> bool {
        self.input_monitoring && self.accessibility
    }
}

/// 系统隐私设置面板锚点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacySection {
    InputMonitoring,
    Accessibility,
    Microphone,
}

/// 前台应用（应用名, 应用标识）。macOS 标识 = bundle id；Windows 标识 = exe 名（不含扩展名）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FrontmostApp {
    pub name: String,
    pub id: String,
}

/// 录音器抽象（16kHz 单声道 f32）。实现：`audio::CpalRecorder`；测试用假实现。
///
/// 语义与 Python `Recorder` 一致：
/// - `start` 已在录音则忽略；`stop` 返回 (pcm, 时长秒)；`abort` 丢弃不返回；
/// - `snapshot` 返回当前累积 PCM 的拷贝（伪流式预览用）；
/// - `level` 返回最近 ~100ms 的 RMS 映射到 0~1（HUD 声浪用）。
pub trait Recorder: Send {
    fn start(&mut self) -> anyhow::Result<()>;
    fn stop(&mut self) -> (Vec<f32>, f64);
    fn abort(&mut self);
    fn snapshot(&self) -> Vec<f32>;
    fn duration(&self) -> f64;
    fn level(&self) -> f32;
    fn is_recording(&self) -> bool;
}

/// 转写引擎抽象。实现：`transcriber::Transcriber`（transcribe.cpp + SenseVoice）。
/// 内部必须串行（模型同一时刻只允许一个 run）；`partial=true` 为预览中间结果，只打 debug 日志。
pub trait Transcribe: Send + Sync {
    fn transcribe(&self, pcm: &[f32], partial: bool) -> anyhow::Result<String>;
}

/// LLM 润色/改写抽象。实现：`polisher::Polisher`。全部 fail-open：失败返回 `None`。
pub trait Polish: Send + Sync {
    /// polish.enabled 开关。
    fn enabled(&self) -> bool;
    /// provider 配置是否可用（改写通道的前置检查，不看 enabled）。
    fn configured(&self) -> bool;
    /// `style`：场景感知匹配到的风格名覆盖；`None` 用全局默认。
    fn polish(&self, text: &str, style: Option<&str>) -> Option<String>;
    fn rewrite(&self, selection: &str, instruction: &str) -> Option<String>;
}

/// 悬浮窗抽象（由 Tauri 层实现，内部把调用转成前端事件）。所有方法可从任意线程调用。
pub trait Hud: Send + Sync {
    /// 开始一次录音会话：重置并显示。`prefix` 如「改写」；`placeholder` 如「聆听中…」。
    fn begin(&self, channel: Channel, placeholder: &str, hint: &str);
    /// 推入音量电平 0~1，约 12Hz。
    fn set_level(&self, level: f32);
    /// 更新已录秒数与伪流式文本（text 为空则保留上次）。
    fn set_partial(&self, elapsed: f64, text: &str);
    /// 录音后阶段：主行状态文案，副行已转写全文（可空）。
    fn set_status(&self, status: &str, detail: &str);
    fn hide(&self);
}

/// 状态/通知回调（由 Tauri 层实现：托盘图标、系统通知、权限探针）。
pub trait Events: Send + Sync {
    fn status(&self, status: Status);
    fn notify(&self, title: &str, message: &str);
    /// 首次收到任何键盘事件（输入监听权限探针）。
    fn first_key_event(&self) {}
}

/// 空实现，测试与 headless 场景用。
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopHud;
impl Hud for NoopHud {
    fn begin(&self, _: Channel, _: &str, _: &str) {}
    fn set_level(&self, _: f32) {}
    fn set_partial(&self, _: f64, _: &str) {}
    fn set_status(&self, _: &str, _: &str) {}
    fn hide(&self) {}
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopEvents;
impl Events for NoopEvents {
    fn status(&self, _: Status) {}
    fn notify(&self, _: &str, _: &str) {}
}

pub type SharedHud = Arc<dyn Hud>;
pub type SharedEvents = Arc<dyn Events>;
pub type SharedTranscriber = Arc<dyn Transcribe>;
pub type SharedPolisher = Arc<dyn Polish>;
