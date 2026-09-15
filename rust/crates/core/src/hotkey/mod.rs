//! 全局热键与录音状态机（1:1 移植 Python `xiaodao_ime/hotkey.py`）。
//!
//! 两个热键通道：
//!   - 听写（macOS 默认左 Option）：录音 → 转写 →（可选）润色 → 粘贴到光标处；
//!   - 语音改写（macOS 默认右 Option）：先选中一段文字，按改写键说指令（如「改成英文」），
//!     松手后抓取选区 + 指令一起交给大模型，结果原地替换选区。
//!
//! 两种录音方式（settings.json 的 `record_mode`）：
//!   - `toggle`（默认）：单击热键开始录音，再单击一次结束；
//!   - `hold`：按住热键说话、松开出字；0.35s 内双击进入锁定录音，再按一下结束。
//!
//! 防误触规则（与 Python 版完全一致，测试见 `tests.rs`）：
//!   1. 录音期间按下任何其他键（含另一个热键），立即取消本次录音；
//!   2. toggle 方式下「热键+其他键」组合快捷键不会误触发开始录音；
//!   3. 录音时长 < 0.4s 直接丢弃；转写结果为空不粘贴。
//!
//! # 线程与锁
//!
//! Python 版靠 GIL 天然串行，Rust 必须自己管：
//!   - [`HotkeyController::state`] 只保护**小状态**（几个 bool + 通道 + 计时点），
//!     键盘钩子线程在一次事件内全程持有，保证事件处理是原子的；
//!   - 录音器单独一把锁（[`HotkeyController::recorder`]），预览/声浪线程按需短暂持有；
//!   - **转写 / LLM / 粘贴一律在 worker 线程且不持任何锁**，避免键盘钩子线程被 LLM 卡死；
//!   - 锁序固定为 `state -> recorder`，预览线程先读完 state 再去拿 recorder，不会反向。

mod machine;
mod worker;

#[cfg(test)]
mod fakes;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use parking_lot::Mutex;
use tracing::info;

use crate::keys::{HotkeyId, RecordMode};
use crate::platform::SharedPlatform;
use crate::types::{
    Channel, Recorder, SharedEvents, SharedHud, SharedPolisher, SharedTranscriber, SoundEvent,
    Status,
};

/// 防误触：按住时长小于该秒数的录音直接丢弃（Python `config.MIN_HOLD_SECONDS`）。
pub const MIN_HOLD: Duration = Duration::from_millis(400);
/// 双击热键进入「锁定录音」的两次短按间隔上限（Python `config.DOUBLE_TAP_WINDOW`）。
pub const DOUBLE_TAP_WINDOW: Duration = Duration::from_millis(350);
/// 累积音频不足该样本数（0.5s @16kHz）时预览只更新计时，不做转写。
pub(crate) const PREVIEW_MIN_SAMPLES: usize = 8000;
/// 伪流式预览的最小刷新间隔；实际间隔按单次转写耗时自适应放大。
pub(crate) const PREVIEW_MIN_INTERVAL: Duration = Duration::from_millis(700);
/// 预览间隔 = max(PREVIEW_MIN_INTERVAL, 上次转写耗时 × 该系数)。
pub(crate) const PREVIEW_COST_FACTOR: u32 = 25; // ×2.5，用整数避免浮点
/// HUD 声浪刷新间隔（~12Hz）。
pub(crate) const LEVEL_INTERVAL: Duration = Duration::from_millis(80);

/// 输入历史落盘口（由 `history::History` 实现）。
///
/// 单独抽 trait 而不是直接依赖 `history::History`，是为了让状态机可单测、
/// 也让历史功能可以整体关掉（`Deps::history = None`）。
pub trait HistorySink: Send + Sync {
    fn append(&self, raw: &str, final_text: &str);
}

/// 状态机需要的设置只读视图（由 `settings::SettingsStore` 实现）。
///
/// 只读、按需取值：设置窗口随时可能改 settings.json，状态机不缓存。
pub trait SettingsView: Send + Sync {
    /// 是否开启伪流式预览（HUD 实时出字）。
    fn live_preview(&self) -> bool;
    /// 是否播放提示音。
    fn sounds(&self) -> bool;
    /// 离线替换表（转写后无条件字符串替换）。
    fn replacements(&self) -> BTreeMap<String, String>;
    /// 场景感知：前台应用标识 / 应用名 → 润色风格（值为「关闭」时该应用不润色）。
    fn app_styles(&self) -> BTreeMap<String, String>;
}

/// 可注入时钟：双击窗口判定用，测试里换成可控假时钟（不真 sleep 350ms）。
pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// 真实时钟。
pub fn system_clock() -> Clock {
    Arc::new(Instant::now)
}

/// 状态机的全部外部依赖（构造时一次性注入，之后不可变）。
pub struct Deps {
    pub recorder: Box<dyn Recorder>,
    pub transcriber: SharedTranscriber,
    pub polisher: Option<SharedPolisher>,
    pub hud: SharedHud,
    pub events: SharedEvents,
    pub platform: SharedPlatform,
    pub settings: Arc<dyn SettingsView>,
    pub history: Option<Arc<dyn HistorySink>>,
    pub clock: Clock,
}

/// 受锁保护的可变状态。字段与 Python 版 `HotkeyController.__init__` 一一对应。
pub(crate) struct State {
    /// 暂停热键（菜单总开关），true 时忽略一切按键
    pub(crate) paused: bool,
    /// 当前录音属于哪个通道
    pub(crate) channel: Channel,
    /// 热键当前是否被按住
    pub(crate) held: bool,
    /// toggle：按住热键期间出现组合键，本次单击作废
    pub(crate) suppressed: bool,
    pub(crate) recording: bool,
    /// hold：锁定录音（双击进入）
    pub(crate) locked: bool,
    pub(crate) cancelled: bool,
    pub(crate) ignore_next_release: bool,
    /// hold：上一次短按时间（识别双击）
    pub(crate) last_short_tap: Option<Instant>,
    /// 是否收到过键盘事件（输入监听权限探针）
    pub(crate) saw_event: bool,
    /// toggle：按下时记下通道，松开时才真正开始录音
    pub(crate) pending_channel: Channel,
    pub(crate) trigger: HotkeyId,
    pub(crate) rewrite_trigger: HotkeyId,
    pub(crate) mode: RecordMode,
    /// 预览/声浪线程的停止信号：持有 Sender，丢弃即通知全部接收端退出
    pub(crate) preview_stop: Option<Sender<()>>,
}

pub struct HotkeyController {
    /// 自引用：worker / 预览线程需要 `Arc<Self>`，用 Weak 避免循环引用
    pub(crate) me: Weak<HotkeyController>,
    pub(crate) state: Mutex<State>,
    /// 录音器单独一把锁：预览线程只需 snapshot/level/duration，不该被状态锁挡住
    pub(crate) recorder: Mutex<Box<dyn Recorder>>,
    pub(crate) transcriber: SharedTranscriber,
    pub(crate) polisher: Option<SharedPolisher>,
    pub(crate) hud: SharedHud,
    pub(crate) events: SharedEvents,
    pub(crate) platform: SharedPlatform,
    pub(crate) settings: Arc<dyn SettingsView>,
    pub(crate) history: Option<Arc<dyn HistorySink>>,
    pub(crate) clock: Clock,
    pub(crate) min_hold: Duration,
}

impl HotkeyController {
    pub fn new(
        deps: Deps,
        hotkey: HotkeyId,
        rewrite_hotkey: HotkeyId,
        mode: RecordMode,
        min_hold: Duration,
    ) -> Arc<Self> {
        let Deps {
            recorder,
            transcriber,
            polisher,
            hud,
            events,
            platform,
            settings,
            history,
            clock,
        } = deps;
        Arc::new_cyclic(|me| HotkeyController {
            me: me.clone(),
            state: Mutex::new(State {
                paused: false,
                channel: Channel::Dictate,
                held: false,
                suppressed: false,
                recording: false,
                locked: false,
                cancelled: false,
                ignore_next_release: false,
                last_short_tap: None,
                saw_event: false,
                pending_channel: Channel::Dictate,
                trigger: hotkey,
                rewrite_trigger: rewrite_hotkey,
                mode,
                preview_stop: None,
            }),
            recorder: Mutex::new(recorder),
            transcriber,
            polisher,
            hud,
            events,
            platform,
            settings,
            history,
            clock,
            min_hold,
        })
    }

    // ---- 外部配置 ----

    /// 切换听写热键；录音中切换会先取消本次录音（与 Python 版一致）。
    pub fn set_trigger(&self, hotkey: HotkeyId) {
        let mut st = self.state.lock();
        if st.recording {
            self.abort_recording(&mut st, "切换热键");
        }
        st.trigger = hotkey;
        info!("听写热键已切换为：{}", hotkey.display());
    }

    /// 切换改写热键；录音中切换会先取消本次录音。
    pub fn set_rewrite_trigger(&self, hotkey: HotkeyId) {
        let mut st = self.state.lock();
        if st.recording {
            self.abort_recording(&mut st, "切换改写热键");
        }
        st.rewrite_trigger = hotkey;
        info!("改写热键已切换为：{}", hotkey.display());
    }

    /// 切换录音方式；录音中切换会先取消本次录音。
    pub fn set_mode(&self, mode: RecordMode) {
        let mut st = self.state.lock();
        if st.recording {
            self.abort_recording(&mut st, "切换录音方式");
        }
        st.mode = mode;
        info!("录音方式已切换为：{}", mode.display());
    }

    pub fn mode(&self) -> RecordMode {
        self.state.lock().mode
    }

    pub fn paused(&self) -> bool {
        self.state.lock().paused
    }

    /// 暂停/恢复热键监听（不销毁 listener，只忽略事件）。
    pub fn set_paused(&self, paused: bool) {
        let mut st = self.state.lock();
        if paused && st.recording {
            self.abort_recording(&mut st, "暂停热键");
        }
        st.paused = paused;
        st.held = false;
        st.suppressed = false;
        info!("热键已{}", if paused { "暂停" } else { "恢复" });
    }

    pub fn is_recording(&self) -> bool {
        self.state.lock().recording
    }

    /// 关停预览/声浪线程（退出前调用；键盘监听由 `listener::ListenerHandle` 负责）。
    pub fn stop(&self) {
        let mut st = self.state.lock();
        Self::stop_preview(&mut st);
    }

    // ---- 内部工具 ----

    pub(crate) fn status(&self, status: Status) {
        self.events.status(status);
    }

    pub(crate) fn notify(&self, title: &str, message: &str) {
        info!("通知：{} —— {}", title, message);
        self.events.notify(title, message);
    }

    /// 播放提示音；`settings.sounds` 为 false 时静音（Python `feedback.play`）。
    pub(crate) fn play(&self, event: SoundEvent) {
        if !self.settings.sounds() {
            return;
        }
        self.platform.play_sound(event);
    }
}
