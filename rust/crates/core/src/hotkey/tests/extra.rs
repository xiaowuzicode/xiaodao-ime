//! 状态机补充用例：Python `test_hotkey.py` 没有覆盖、但同样属于既有行为的部分
//! （听写/改写 worker 全链路、伪流式预览、双击窗口边界、提示音静音开关）。

use std::time::Duration;

use super::super::fakes::*;
use super::super::DOUBLE_TAP_WINDOW;
use super::{dictate, rewrite, Builder};
use crate::keys::{HotkeyId, RecordMode};
use crate::types::{FrontmostApp, SoundEvent, Status};

/// 两次短按间隔超过双击窗口 => 不进锁定，只是各自丢弃。
#[test]
fn hold_short_taps_outside_window_do_not_lock() {
    let h = Builder::new(RecordMode::Hold).build();
    h.rec.set_duration(0.1);
    h.press(dictate());
    h.release(dictate());
    assert!(!h.locked());
    // 关键：用可控时钟跨过双击窗口，不真 sleep
    h.clock
        .advance(DOUBLE_TAP_WINDOW + Duration::from_millis(10));
    h.press(dictate());
    h.release(dictate());
    assert!(!h.locked() && !h.rec.is_recording());
    assert!(h.platform.pasted().is_empty());
}

/// toggle 方式下时长不足 min_hold 的录音直接丢弃，不进 worker。
#[test]
fn toggle_short_recording_discarded() {
    let h = Builder::new(RecordMode::Toggle).text("不该出现").build();
    h.rec.set_duration(0.1);
    h.press(dictate());
    h.release(dictate());
    h.press(dictate());
    h.release(dictate());
    assert_eq!(h.events.statuses(), vec![Status::Recording, Status::Idle]);
    assert!(h.platform.pasted().is_empty());
    // 没有 stop 提示音：压根没进入转写
    assert_eq!(h.platform.sounds(), vec![SoundEvent::Start]);
}

/// 听写 worker 全链路：转写 → 替换表 → 场景风格 → 润色 → 粘贴 → 历史。
#[test]
fn dictate_worker_applies_replacements_polish_and_history() {
    let polisher = FakePolisher::with_results(true, Some("润色后的文本"), None);
    let h = Builder::new(RecordMode::Toggle)
        .text("原始 转写 结果")
        .polisher(polisher.clone())
        .settings(|s| {
            s.replacements
                .insert("转写".to_string(), "識別".to_string());
            s.app_styles
                .insert("com.apple.Notes".to_string(), "书面化".to_string());
        })
        .build();
    *h.platform.app.lock() = FrontmostApp {
        name: "备忘录".to_string(),
        id: "com.apple.Notes".to_string(),
    };

    h.press(dictate());
    h.release(dictate());
    h.press(dictate());
    h.release(dictate());
    h.events.wait_idle(1);

    // 替换表先于润色生效，且把匹配到的风格透传给 polisher
    assert_eq!(
        polisher.polish_calls.lock().clone(),
        vec![("原始 識別 结果".to_string(), Some("书面化".to_string()))]
    );
    assert_eq!(h.platform.pasted(), vec!["润色后的文本".to_string()]);
    // 历史记的是「原始转写」+「最终文本」
    assert_eq!(
        h.history.entries(),
        vec![("原始 转写 结果".to_string(), "润色后的文本".to_string())]
    );
    assert_eq!(
        h.events.statuses(),
        vec![
            Status::Recording,
            Status::Transcribing,
            Status::Polishing,
            Status::Idle
        ]
    );
    assert!(h.hud.status_titles().contains(&"润色中…".to_string()));
}

/// app_styles 配「关闭」时该应用直出转写，不调用 LLM。
#[test]
fn dictate_worker_respects_style_off() {
    let polisher = FakePolisher::with_results(true, Some("不该用到"), None);
    let h = Builder::new(RecordMode::Toggle)
        .text("直出文本")
        .polisher(polisher.clone())
        .settings(|s| {
            s.app_styles
                .insert("Terminal".to_string(), "关闭".to_string());
        })
        .build();
    *h.platform.app.lock() = FrontmostApp {
        name: "Terminal".to_string(),
        id: "com.apple.Terminal".to_string(),
    };

    h.press(dictate());
    h.release(dictate());
    h.press(dictate());
    h.release(dictate());
    h.events.wait_idle(1);

    assert!(polisher.polish_calls.lock().is_empty());
    assert_eq!(h.platform.pasted(), vec!["直出文本".to_string()]);
    // 没有 Polishing 状态
    assert!(!h.events.statuses().contains(&Status::Polishing));
}

/// 润色返回 None（LLM 失败）时 fail-open：粘贴原始转写。
#[test]
fn dictate_worker_fail_open_on_polish_failure() {
    let polisher = FakePolisher::with_results(true, None, None);
    let h = Builder::new(RecordMode::Toggle)
        .text("兜底文本")
        .polisher(polisher)
        .build();
    h.press(dictate());
    h.release(dictate());
    h.press(dictate());
    h.release(dictate());
    h.events.wait_idle(1);
    assert_eq!(h.platform.pasted(), vec!["兜底文本".to_string()]);
}

/// 改写通道全链路：抓选区 → 识别指令 → LLM → 原地替换 → 历史。
#[test]
fn rewrite_worker_replaces_selection() {
    let polisher = FakePolisher::with_results(false, None, Some("Rewritten text"));
    let h = Builder::new(RecordMode::Toggle)
        .text("  改成英文  ")
        .polisher(polisher.clone())
        .selection("选中的文字")
        .build();

    h.press(rewrite());
    h.release(rewrite());
    h.press(rewrite());
    h.release(rewrite());
    h.events.wait_idle(1);

    assert_eq!(
        polisher.rewrite_calls.lock().clone(),
        vec![("选中的文字".to_string(), "改成英文".to_string())]
    );
    assert_eq!(h.platform.pasted(), vec!["Rewritten text".to_string()]);
    assert_eq!(
        h.history.entries(),
        vec![("〔改写〕改成英文".to_string(), "Rewritten text".to_string())]
    );
    assert!(h.hud.status_titles().contains(&"识别指令中…".to_string()));
}

/// 改写但没有选区 => 提示 + 取消音，不动原文。
#[test]
fn rewrite_worker_without_selection_notifies() {
    let polisher = FakePolisher::with_results(false, None, Some("不该用到"));
    let h = Builder::new(RecordMode::Toggle)
        .text("改成英文")
        .polisher(polisher.clone())
        .build(); // selection = None

    h.press(rewrite());
    h.release(rewrite());
    h.press(rewrite());
    h.release(rewrite());
    h.events.wait_idle(1);

    assert!(polisher.rewrite_calls.lock().is_empty());
    assert!(h.platform.pasted().is_empty());
    assert_eq!(
        h.events.notifications().first().map(|(t, _)| t.clone()),
        Some("未检测到选中文字".to_string())
    );
    assert!(h.platform.sounds().contains(&SoundEvent::Cancel));
}

/// 改写但没听清指令（转写为空）=> 提示，不调 LLM。
#[test]
fn rewrite_worker_without_instruction_notifies() {
    let polisher = FakePolisher::with_results(false, None, Some("不该用到"));
    let h = Builder::new(RecordMode::Toggle)
        .text("   ")
        .polisher(polisher.clone())
        .selection("选中的文字")
        .build();

    h.press(rewrite());
    h.release(rewrite());
    h.press(rewrite());
    h.release(rewrite());
    h.events.wait_idle(1);

    assert!(polisher.rewrite_calls.lock().is_empty());
    assert_eq!(
        h.events.notifications().first().map(|(t, _)| t.clone()),
        Some("没听清指令".to_string())
    );
}

/// 首个键盘事件触发权限探针回调，且只触发一次。
#[test]
fn first_key_event_probe_fires_once() {
    let h = Builder::new(RecordMode::Toggle).build();
    h.ctl.set_paused(true); // 暂停也要能收到探针
    h.press_other();
    h.press_other();
    assert_eq!(*h.events.first_key.lock(), 1);
}

/// 切换录音方式 / 热键时若正在录音，先取消。
#[test]
fn switching_config_aborts_recording() {
    let h = Builder::new(RecordMode::Toggle).build();
    h.press(dictate());
    h.release(dictate());
    assert!(h.rec.is_recording());
    h.ctl.set_mode(RecordMode::Hold);
    assert!(!h.rec.is_recording() && !h.ctl.is_recording());
    assert_eq!(h.ctl.mode(), RecordMode::Hold);

    h.press(dictate());
    assert!(h.rec.is_recording());
    h.ctl.set_trigger(HotkeyId::F19);
    assert!(!h.rec.is_recording());
    h.ctl.set_rewrite_trigger(HotkeyId::F19);
}

/// 伪流式预览：录音期间把累积音频重转并推给 HUD（真 sleep 一个预览周期）。
#[test]
fn preview_loop_pushes_partial_text() {
    let h = Builder::new(RecordMode::Toggle)
        .text("预览文本")
        .settings(|s| s.live_preview = true)
        .build();
    h.rec.set_samples(16_000); // ≥ PREVIEW_MIN_SAMPLES，预览才会真去转写
    h.press(dictate());
    h.release(dictate());
    assert_eq!(h.hud.begins.lock().len(), 1);

    std::thread::sleep(Duration::from_millis(900));
    assert!(
        h.transcriber.partial_calls() >= 1,
        "预览应调用 partial 转写"
    );
    assert!(
        h.hud.partial_texts().contains(&"预览文本".to_string()),
        "HUD 应收到伪流式文本，实际：{:?}",
        h.hud.partial_texts()
    );

    // 结束录音后预览线程必须退出，不再产生 partial 调用
    h.press(dictate());
    h.release(dictate());
    h.events.wait_idle(1);
    let calls = h.transcriber.partial_calls();
    std::thread::sleep(Duration::from_millis(250));
    assert_eq!(h.transcriber.partial_calls(), calls, "预览线程没有被停掉");
}

/// 音频不足 0.5s 时预览只更新计时，不调用转写。
#[test]
fn preview_skips_transcribe_for_tiny_audio() {
    let h = Builder::new(RecordMode::Toggle)
        .settings(|s| s.live_preview = true)
        .build();
    h.rec.set_samples(160); // 远小于 PREVIEW_MIN_SAMPLES
    h.press(dictate());
    h.release(dictate());
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(h.transcriber.partial_calls(), 0);
    // 声浪线程仍在刷新计时
    assert!(!h.hud.partials.lock().is_empty());
    h.ctl.stop();
}

/// `sounds=false` 时不播放任何提示音。
#[test]
fn sounds_can_be_muted() {
    let h = Builder::new(RecordMode::Toggle)
        .settings(|s| s.sounds = false)
        .build();
    h.press(dictate());
    h.release(dictate());
    h.press(dictate());
    h.release(dictate());
    h.events.wait_idle(1);
    assert!(h.platform.sounds().is_empty());
}
