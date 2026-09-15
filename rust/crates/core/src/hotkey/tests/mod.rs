//! 状态机回归测试：逐条移植 Python `test_hotkey.py`，另补 worker / 预览链路的用例。
//!
//! 全部用假录音器驱动按键序列，不依赖真实键盘/麦克风/模型；双击窗口用可控时钟，
//! 不做 350ms 级别的真 sleep（只有预览用例真等一个刷新周期）。
//!
//! 本文件 = 测试脚手架 + `test_hotkey.py` 的逐条移植；
//! [`extra`] = Python 版没覆盖、但 Rust 版必须守住的 worker / 预览 / 静音等链路。

mod extra;

use std::sync::Arc;

use super::fakes::*;
use super::{Deps, HotkeyController, MIN_HOLD};
use crate::keys::{HotkeyId, KeyEvent, KeyKind, RecordMode};
use crate::types::{Channel, SharedPolisher, SoundEvent, Status};

struct Harness {
    ctl: Arc<HotkeyController>,
    rec: FakeRecorder,
    events: Arc<SpyEvents>,
    hud: Arc<SpyHud>,
    platform: Arc<FakePlatform>,
    transcriber: Arc<FakeTranscriber>,
    history: Arc<SpyHistory>,
    clock: Arc<TestClock>,
}

struct Builder {
    mode: RecordMode,
    text: String,
    polisher: Option<SharedPolisher>,
    settings: FakeSettings,
    selection: Option<String>,
}

impl Builder {
    fn new(mode: RecordMode) -> Self {
        Builder {
            mode,
            text: String::new(), // 与 Python FakeTranscriber 一致：空结果 => 不粘贴
            polisher: None,
            settings: FakeSettings::new(),
            selection: None,
        }
    }
    fn text(mut self, text: &str) -> Self {
        self.text = text.to_string();
        self
    }
    fn polisher(mut self, polisher: SharedPolisher) -> Self {
        self.polisher = Some(polisher);
        self
    }
    fn selection(mut self, selection: &str) -> Self {
        self.selection = Some(selection.to_string());
        self
    }
    fn settings(mut self, edit: impl FnOnce(&mut FakeSettings)) -> Self {
        edit(&mut self.settings);
        self
    }
    fn build(self) -> Harness {
        let rec = FakeRecorder::new();
        let events = Arc::new(SpyEvents::default());
        let hud = Arc::new(SpyHud::default());
        let platform = FakePlatform::new();
        *platform.selection.lock() = self.selection;
        let transcriber = FakeTranscriber::new(&self.text);
        let history = Arc::new(SpyHistory::default());
        let clock = TestClock::new();
        let clock_for_deps = clock.clone();
        let deps = Deps {
            recorder: Box::new(rec.clone()),
            transcriber: transcriber.clone(),
            polisher: self.polisher,
            hud: hud.clone(),
            events: events.clone(),
            platform: platform.clone(),
            settings: Arc::new(self.settings),
            history: Some(history.clone()),
            clock: Arc::new(move || clock_for_deps.now()),
        };
        let ctl = HotkeyController::new(
            deps,
            HotkeyId::default_dictate(),
            HotkeyId::default_rewrite(),
            self.mode,
            MIN_HOLD,
        );
        Harness {
            ctl,
            rec,
            events,
            hud,
            platform,
            transcriber,
            history,
            clock,
        }
    }
}

impl Harness {
    fn press(&self, id: HotkeyId) {
        self.ctl.on_key(KeyEvent {
            key: KeyKind::Hotkey(id),
            pressed: true,
        });
    }
    fn release(&self, id: HotkeyId) {
        self.ctl.on_key(KeyEvent {
            key: KeyKind::Hotkey(id),
            pressed: false,
        });
    }
    fn press_other(&self) {
        self.ctl.on_key(KeyEvent {
            key: KeyKind::Other,
            pressed: true,
        });
    }
    fn channel(&self) -> Channel {
        self.ctl.state.lock().channel
    }
    fn locked(&self) -> bool {
        self.ctl.state.lock().locked
    }
    fn cancelled(&self) -> bool {
        self.ctl.state.lock().cancelled
    }
}

fn dictate() -> HotkeyId {
    HotkeyId::default_dictate()
}
fn rewrite() -> HotkeyId {
    HotkeyId::default_rewrite()
}

// ---- 移植自 test_hotkey.py ----

/// `test_toggle_basic`：单击开始 / 再击结束。
#[test]
fn toggle_basic() {
    let h = Builder::new(RecordMode::Toggle).build();
    // 单击（按下+松开）=> 开始录音
    h.press(dictate());
    h.release(dictate());
    assert!(h.rec.is_recording() && h.ctl.is_recording());
    // 再击 => 结束并处理（按下即停止，松开被忽略）
    h.press(dictate());
    assert!(!h.rec.is_recording() && !h.ctl.is_recording());
    h.release(dictate());
    assert!(!h.ctl.is_recording());

    h.events.wait_idle(1);
    assert_eq!(
        h.events.statuses(),
        vec![Status::Recording, Status::Transcribing, Status::Idle]
    );
    assert_eq!(
        h.platform.sounds(),
        vec![SoundEvent::Start, SoundEvent::Stop]
    );
    // 转写为空 => 不粘贴、不记历史
    assert!(h.platform.pasted().is_empty());
    assert!(h.history.entries().is_empty());
}

/// `test_toggle_combo_no_false_start`：⌥+C 组合键不误触发录音。
#[test]
fn toggle_combo_no_false_start() {
    let h = Builder::new(RecordMode::Toggle).build();
    h.press(dictate());
    h.press_other();
    h.release(dictate());
    assert!(!h.rec.is_recording() && !h.ctl.is_recording());
    assert!(h.events.statuses().is_empty(), "组合键不该产生任何状态变化");
    assert!(h.platform.sounds().is_empty());
}

/// `test_toggle_other_key_cancels`：录音中打字取消，取消后还能正常再开。
#[test]
fn toggle_other_key_cancels() {
    let h = Builder::new(RecordMode::Toggle).build();
    h.press(dictate());
    h.release(dictate());
    assert!(h.rec.is_recording());
    h.press_other();
    assert!(!h.rec.is_recording() && !h.ctl.is_recording());
    // 取消后还能正常开启下一次
    h.press(dictate());
    h.release(dictate());
    assert!(h.rec.is_recording());
    assert_eq!(
        h.platform.sounds(),
        vec![SoundEvent::Start, SoundEvent::Cancel, SoundEvent::Start]
    );
    h.ctl.stop();
}

/// `test_hold_basic_and_lock`：按住说话 + 双击锁定 + 再按结束。
#[test]
fn hold_basic_and_lock() {
    let h = Builder::new(RecordMode::Hold).build();
    // 按住 >= min_hold 后松开 => 处理
    h.press(dictate());
    assert!(h.rec.is_recording());
    h.rec.set_duration(1.0);
    h.release(dictate());
    assert!(!h.rec.is_recording() && !h.ctl.is_recording());
    h.events.wait_idle(1);

    // 双击（两次短按）=> 锁定录音
    h.rec.set_duration(0.1);
    h.press(dictate());
    h.release(dictate()); // 第一次短按：丢弃并记时间
    assert!(!h.rec.is_recording());
    assert!(!h.locked());
    h.press(dictate());
    h.release(dictate()); // 第二次短按：进入锁定
    assert!(h.rec.is_recording() && h.locked());

    // 第三次按下：结束（锁定录音不再检查 min_hold）
    h.rec.set_duration(2.0);
    h.press(dictate());
    assert!(!h.rec.is_recording() && !h.locked());
    h.release(dictate());
    h.events.wait_idle(3);
}

/// `test_hold_combo_cancels`：hold 方式下 ⌥+C 组合键取消且不残留 cancelled 标志。
#[test]
fn hold_combo_cancels() {
    let h = Builder::new(RecordMode::Hold).build();
    h.press(dictate());
    assert!(h.rec.is_recording());
    h.press_other(); // ⌥+C
    assert!(!h.rec.is_recording());
    h.release(dictate());
    assert!(!h.ctl.is_recording() && !h.cancelled());
    // hold 组合键是静默丢弃：不放取消音（与 Python `_press_other` 的 elif 分支一致）
    assert_eq!(h.platform.sounds(), vec![SoundEvent::Start]);
}

/// `test_rewrite_channel`：右 Option 单击开始改写通道、再击结束。
#[test]
fn rewrite_channel() {
    let h = Builder::new(RecordMode::Toggle).build();
    h.press(rewrite());
    h.release(rewrite());
    assert!(h.rec.is_recording());
    assert_eq!(h.channel(), Channel::Rewrite);
    h.press(rewrite());
    assert!(!h.rec.is_recording());
    h.release(rewrite());
    // polisher=None => 走「语音改写不可用」分支，不粘贴
    h.events.wait_idle(1);
    assert_eq!(
        h.events.notifications().first().map(|(t, _)| t.clone()),
        Some("语音改写不可用".to_string())
    );
    assert!(h.platform.pasted().is_empty());
}

/// `test_cross_hotkey_cancels`：两个热键互斥取消。
#[test]
fn cross_hotkey_cancels() {
    let h = Builder::new(RecordMode::Toggle).build();
    h.press(dictate());
    h.release(dictate());
    assert!(h.rec.is_recording());
    assert_eq!(h.channel(), Channel::Dictate);
    // 听写录音中按下改写键 => 取消，不触发改写
    h.press(rewrite());
    assert!(!h.rec.is_recording() && !h.ctl.is_recording());
    h.release(rewrite());
    // 取消后改写键还能正常开启
    h.press(rewrite());
    h.release(rewrite());
    assert!(h.rec.is_recording());
    assert_eq!(h.channel(), Channel::Rewrite);
    h.ctl.stop();
}

/// `test_pause_resume`：暂停热键总开关。
#[test]
fn pause_resume() {
    let h = Builder::new(RecordMode::Toggle).build();
    h.ctl.set_paused(true);
    assert!(h.ctl.paused());
    h.press(dictate());
    h.release(dictate());
    assert!(!h.rec.is_recording() && !h.ctl.is_recording());

    h.ctl.set_paused(false);
    assert!(!h.ctl.paused());
    h.press(dictate());
    h.release(dictate());
    assert!(h.rec.is_recording());

    // 录音中暂停 => 立即中止
    h.ctl.set_paused(true);
    assert!(!h.rec.is_recording() && !h.ctl.is_recording());
}
