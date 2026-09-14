//! 单测替身：假录音器 / 假转写器 / 假润色器 / 间谍 HUD·事件·平台 / 可控时钟。
//!
//! 对应 Python `test_hotkey.py` 里的 `FakeRecorder` / `FakeTranscriber`，
//! 另外补上 Python 版没有的观察点（状态序列、提示音序列、粘贴文本、通知），
//! 让「行为等价」可以被断言而不只是被肉眼确认。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::{HistorySink, SettingsView};
use crate::platform::Platform;
use crate::types::{
    Channel, Events, FrontmostApp, Hud, Permissions, PrivacySection, Recorder, SoundEvent, Status,
    Transcribe,
};

// ---- 录音器 ----

#[derive(Default)]
pub(super) struct RecState {
    pub recording: bool,
    pub duration: f64,
    pub samples: usize,
    pub aborts: usize,
}

#[derive(Clone)]
pub(super) struct FakeRecorder(pub Arc<Mutex<RecState>>);

impl FakeRecorder {
    pub fn new() -> Self {
        FakeRecorder(Arc::new(Mutex::new(RecState {
            duration: 1.0,
            samples: 160,
            ..Default::default()
        })))
    }
    pub fn is_recording(&self) -> bool {
        self.0.lock().recording
    }
    pub fn set_duration(&self, seconds: f64) {
        self.0.lock().duration = seconds;
    }
    pub fn set_samples(&self, samples: usize) {
        self.0.lock().samples = samples;
    }
}

impl Recorder for FakeRecorder {
    fn start(&mut self) -> anyhow::Result<()> {
        let mut st = self.0.lock();
        st.recording = true;
        Ok(())
    }
    fn stop(&mut self) -> (Vec<f32>, f64) {
        let mut st = self.0.lock();
        st.recording = false;
        (vec![0.0; st.samples], st.duration)
    }
    fn abort(&mut self) {
        let mut st = self.0.lock();
        st.recording = false;
        st.aborts += 1;
    }
    fn snapshot(&self) -> Vec<f32> {
        vec![0.0; self.0.lock().samples]
    }
    fn duration(&self) -> f64 {
        self.0.lock().duration
    }
    fn level(&self) -> f32 {
        0.5
    }
    fn is_recording(&self) -> bool {
        self.0.lock().recording
    }
}

// ---- 转写器 ----

pub(super) struct FakeTranscriber {
    pub text: Mutex<String>,
    pub calls: Mutex<Vec<bool>>, // 记录每次调用的 partial 标志
}

impl FakeTranscriber {
    /// 默认返回空串 —— 与 Python `FakeTranscriber` 一致：worker 直接返回、不粘贴。
    pub fn new(text: &str) -> Arc<Self> {
        Arc::new(FakeTranscriber {
            text: Mutex::new(text.to_string()),
            calls: Mutex::new(Vec::new()),
        })
    }
    pub fn partial_calls(&self) -> usize {
        self.calls.lock().iter().filter(|p| **p).count()
    }
}

impl Transcribe for FakeTranscriber {
    fn transcribe(&self, _pcm: &[f32], partial: bool) -> anyhow::Result<String> {
        self.calls.lock().push(partial);
        Ok(self.text.lock().clone())
    }
}

// ---- 润色器 ----

pub(super) struct FakePolisher {
    pub enabled: bool,
    pub configured: bool,
    /// None 表示 LLM 返回空（fail-open 分支）
    pub polished: Option<String>,
    pub rewritten: Option<String>,
    pub polish_calls: Mutex<Vec<(String, Option<String>)>>,
    pub rewrite_calls: Mutex<Vec<(String, String)>>,
}

impl FakePolisher {
    pub fn with_results(
        enabled: bool,
        polished: Option<&str>,
        rewritten: Option<&str>,
    ) -> Arc<Self> {
        Arc::new(FakePolisher {
            enabled,
            configured: true,
            polished: polished.map(str::to_string),
            rewritten: rewritten.map(str::to_string),
            polish_calls: Mutex::new(Vec::new()),
            rewrite_calls: Mutex::new(Vec::new()),
        })
    }
}

impl crate::types::Polish for FakePolisher {
    fn enabled(&self) -> bool {
        self.enabled
    }
    fn configured(&self) -> bool {
        self.configured
    }
    fn polish(&self, text: &str, style: Option<&str>) -> Option<String> {
        self.polish_calls
            .lock()
            .push((text.to_string(), style.map(str::to_string)));
        self.polished.clone()
    }
    fn rewrite(&self, selection: &str, instruction: &str) -> Option<String> {
        self.rewrite_calls
            .lock()
            .push((selection.to_string(), instruction.to_string()));
        self.rewritten.clone()
    }
}

// ---- HUD ----

#[derive(Default)]
pub(super) struct SpyHud {
    pub begins: Mutex<Vec<(Channel, String, String)>>,
    pub partials: Mutex<Vec<(f64, String)>>,
    pub statuses: Mutex<Vec<(String, String)>>,
    pub hides: Mutex<usize>,
}

impl SpyHud {
    pub fn partial_texts(&self) -> Vec<String> {
        self.partials
            .lock()
            .iter()
            .map(|(_, t)| t.clone())
            .collect()
    }
    pub fn status_titles(&self) -> Vec<String> {
        self.statuses
            .lock()
            .iter()
            .map(|(s, _)| s.clone())
            .collect()
    }
}

impl Hud for SpyHud {
    fn begin(&self, channel: Channel, placeholder: &str, hint: &str) {
        self.begins
            .lock()
            .push((channel, placeholder.to_string(), hint.to_string()));
    }
    fn set_level(&self, _level: f32) {}
    fn set_partial(&self, elapsed: f64, text: &str) {
        self.partials.lock().push((elapsed, text.to_string()));
    }
    fn set_status(&self, status: &str, detail: &str) {
        self.statuses
            .lock()
            .push((status.to_string(), detail.to_string()));
    }
    fn hide(&self) {
        *self.hides.lock() += 1;
    }
}

// ---- 事件（托盘状态 + 通知 + 权限探针） ----

#[derive(Default)]
pub(super) struct SpyEvents {
    pub statuses: Mutex<Vec<Status>>,
    pub notifications: Mutex<Vec<(String, String)>>,
    pub first_key: Mutex<usize>,
}

impl SpyEvents {
    pub fn statuses(&self) -> Vec<Status> {
        self.statuses.lock().clone()
    }
    pub fn notifications(&self) -> Vec<(String, String)> {
        self.notifications.lock().clone()
    }
    /// 轮询等待条件成立（worker 是后台线程，断言前必须等它收尾）。
    pub fn wait_for(&self, what: &str, pred: impl Fn(&[Status]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if pred(&self.statuses.lock()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("等待「{}」超时，状态序列：{:?}", what, self.statuses());
    }
    /// 等到出现第 n 次 Idle（每条链路收尾都会回到 Idle）。
    pub fn wait_idle(&self, n: usize) {
        self.wait_for(&format!("第 {n} 次回到 idle"), |s| {
            s.iter().filter(|x| **x == Status::Idle).count() >= n
        });
    }
}

impl Events for SpyEvents {
    fn status(&self, status: Status) {
        self.statuses.lock().push(status);
    }
    fn notify(&self, title: &str, message: &str) {
        self.notifications
            .lock()
            .push((title.to_string(), message.to_string()));
    }
    fn first_key_event(&self) {
        *self.first_key.lock() += 1;
    }
}

// ---- 平台 ----

/// 假平台：剪贴板是内存里的 `Option<String>`，`send_copy` 把「当前选区」写进剪贴板，
/// `send_paste` 记录下粘贴到光标处的文本——足以让真实 `paster` 的哨兵事务跑通。
#[derive(Default)]
pub(super) struct FakePlatform {
    pub clipboard: Mutex<Option<String>>,
    /// 模拟前台 App 里当前选中的文字；None = 没有选区（复制不改剪贴板）
    pub selection: Mutex<Option<String>>,
    pub pasted: Mutex<Vec<String>>,
    pub sounds: Mutex<Vec<SoundEvent>>,
    pub app: Mutex<FrontmostApp>,
}

impl FakePlatform {
    pub fn new() -> Arc<Self> {
        Arc::new(FakePlatform::default())
    }
    pub fn sounds(&self) -> Vec<SoundEvent> {
        self.sounds.lock().clone()
    }
    pub fn pasted(&self) -> Vec<String> {
        self.pasted.lock().clone()
    }
}

impl Platform for FakePlatform {
    fn read_clipboard(&self) -> Option<String> {
        self.clipboard.lock().clone()
    }
    fn write_clipboard(&self, text: &str) -> anyhow::Result<()> {
        *self.clipboard.lock() = Some(text.to_string());
        Ok(())
    }
    fn clear_clipboard(&self) -> anyhow::Result<()> {
        *self.clipboard.lock() = None;
        Ok(())
    }
    fn send_copy(&self) -> anyhow::Result<()> {
        if let Some(selection) = self.selection.lock().clone() {
            *self.clipboard.lock() = Some(selection);
        }
        Ok(())
    }
    fn send_paste(&self) -> anyhow::Result<()> {
        let text = self.clipboard.lock().clone().unwrap_or_default();
        self.pasted.lock().push(text);
        Ok(())
    }
    fn play_sound(&self, event: SoundEvent) {
        self.sounds.lock().push(event);
    }
    fn frontmost_app(&self) -> FrontmostApp {
        self.app.lock().clone()
    }
    fn check_permissions(&self, _prompt: bool) -> Permissions {
        Permissions {
            input_monitoring: true,
            accessibility: true,
        }
    }
    fn open_privacy_settings(&self, _section: PrivacySection) {}
}

// ---- 设置 ----

pub(super) struct FakeSettings {
    pub live_preview: bool,
    pub sounds: bool,
    pub replacements: BTreeMap<String, String>,
    pub app_styles: BTreeMap<String, String>,
}

impl FakeSettings {
    pub fn new() -> Self {
        FakeSettings {
            live_preview: false, // 与 Python 测试一致：不起预览线程
            sounds: true,        // 假平台不会真出声，打开以便断言提示音序列
            replacements: BTreeMap::new(),
            app_styles: BTreeMap::new(),
        }
    }
}

impl SettingsView for FakeSettings {
    fn live_preview(&self) -> bool {
        self.live_preview
    }
    fn sounds(&self) -> bool {
        self.sounds
    }
    fn replacements(&self) -> BTreeMap<String, String> {
        self.replacements.clone()
    }
    fn app_styles(&self) -> BTreeMap<String, String> {
        self.app_styles.clone()
    }
}

// ---- 历史 ----

#[derive(Default)]
pub(super) struct SpyHistory(pub Mutex<Vec<(String, String)>>);

impl SpyHistory {
    pub fn entries(&self) -> Vec<(String, String)> {
        self.0.lock().clone()
    }
}

impl HistorySink for SpyHistory {
    fn append(&self, raw: &str, final_text: &str) {
        self.0
            .lock()
            .push((raw.to_string(), final_text.to_string()));
    }
}

// ---- 可控时钟（双击窗口测试不真 sleep 350ms） ----

pub(super) struct TestClock(Mutex<Instant>);

impl TestClock {
    pub fn new() -> Arc<Self> {
        Arc::new(TestClock(Mutex::new(Instant::now())))
    }
    pub fn advance(&self, delta: Duration) {
        let mut now = self.0.lock();
        *now += delta;
    }
    pub fn now(&self) -> Instant {
        *self.0.lock()
    }
}
