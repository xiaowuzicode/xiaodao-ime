//! 状态机主体：键盘事件 → 录音生命周期（逐行对照 Python `hotkey.py` 的
//! `_on_press` / `_on_release` / `_press_*` / `_release_*` / `_start_recording` /
//! `_abort_recording` / `_finish_and_process` / `_preview_loop` / `_level_loop`）。

use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, TryRecvError};
use tracing::{debug, info, warn};

use super::{
    HotkeyController, State, DOUBLE_TAP_WINDOW, LEVEL_INTERVAL, PREVIEW_COST_FACTOR,
    PREVIEW_MIN_INTERVAL, PREVIEW_MIN_SAMPLES,
};
use crate::hud;
use crate::keys::{KeyEvent, KeyKind, RecordMode};
use crate::types::{Channel, SoundEvent, Status};

/// 等待 `timeout`；返回 true 表示收到停止信号（Sender 被丢弃），应退出循环。
/// 对应 Python `stop_event.wait(interval)` 的返回语义。
fn wait_or_stop(rx: &Receiver<()>, timeout: Duration) -> bool {
    !matches!(rx.recv_timeout(timeout), Err(RecvTimeoutError::Timeout))
}

/// 非阻塞查询停止信号（对应 Python `stop_event.is_set()`）。
fn stopped(rx: &Receiver<()>) -> bool {
    matches!(rx.try_recv(), Err(TryRecvError::Disconnected) | Ok(()))
}

impl HotkeyController {
    // ---- 键盘事件入口（由 listener 钩子线程调用） ----

    pub fn on_key(&self, ev: KeyEvent) {
        if ev.pressed {
            self.on_press(ev.key);
        } else {
            self.on_release(ev.key);
        }
    }

    fn on_press(&self, key: KeyKind) {
        // 权限探针：首个键盘事件说明输入监听权限已生效。回调放到锁外，
        // 避免 UI 层实现回调时反过来调状态机造成重入死锁。
        let first = {
            let mut st = self.state.lock();
            let first = !st.saw_event;
            if first {
                st.saw_event = true;
            }
            first
        };
        if first {
            info!("✅ 已收到键盘事件，输入监听权限正常（首个按键：{:?}）", key);
            self.events.first_key_event();
        }

        let mut st = self.state.lock();
        if st.paused {
            return;
        }
        match Self::match_channel(&st, key) {
            Some(channel) => {
                if st.mode == RecordMode::Toggle {
                    self.press_toggle(&mut st, channel);
                } else {
                    self.press_hold(&mut st, channel);
                }
            }
            None => self.press_other(&mut st),
        }
    }

    fn on_release(&self, key: KeyKind) {
        let mut st = self.state.lock();
        if st.paused {
            return;
        }
        let Some(channel) = Self::match_channel(&st, key) else {
            return;
        };
        if st.ignore_next_release {
            st.ignore_next_release = false;
            return;
        }
        if st.mode == RecordMode::Toggle {
            self.release_toggle(&mut st, channel);
        } else {
            self.release_hold(&mut st, channel);
        }
    }

    /// 按键属于哪个通道；非热键返回 None（含配置外的其它热键）。
    fn match_channel(st: &State, key: KeyKind) -> Option<Channel> {
        let KeyKind::Hotkey(id) = key else {
            return None;
        };
        if id == st.trigger {
            Some(Channel::Dictate)
        } else if id == st.rewrite_trigger {
            Some(Channel::Rewrite)
        } else {
            None
        }
    }

    /// 录音期间其他按键 => 取消；toggle 按住期间其他按键 => 组合键，本次单击作废。
    fn press_other(&self, st: &mut State) {
        if st.recording && !st.cancelled {
            if st.mode == RecordMode::Toggle || st.locked {
                self.abort_recording(st, "录音中按下其他键");
            } else if st.held {
                // hold 方式按住期间的组合键：静默丢弃（不放取消音，避免打断打字）
                st.cancelled = true;
                st.recording = false;
                Self::stop_preview(st);
                self.recorder.lock().abort();
                self.hud.hide();
                info!("检测到组合键，取消本次录音（防误触）");
                self.status(Status::Idle);
            }
        } else if st.held {
            st.suppressed = true;
        }
    }

    // ---- toggle：单击开始 / 再击结束 ----

    fn press_toggle(&self, st: &mut State, channel: Channel) {
        if st.recording {
            if channel == st.channel {
                // 第二击：结束录音并处理（对应的松开事件要忽略掉）
                st.ignore_next_release = true;
                self.finish_and_process(st, true);
            } else {
                self.abort_recording(st, "录音中按下另一个热键");
            }
            return;
        }
        if st.held {
            return; // 长按自动重复
        }
        st.held = true;
        st.suppressed = false;
        st.pending_channel = channel;
    }

    fn release_toggle(&self, st: &mut State, _channel: Channel) {
        if !st.held {
            return;
        }
        st.held = false;
        if st.suppressed {
            st.suppressed = false;
            return; // 刚才是组合快捷键，不开始录音
        }
        let channel = st.pending_channel;
        self.start_recording(st, channel);
        info!(
            "开始录音（{}，单击模式，再按一次热键结束）",
            match channel {
                Channel::Rewrite => "语音改写",
                Channel::Dictate => "听写",
            }
        );
    }

    // ---- hold：按住说话 + 双击锁定 ----

    fn press_hold(&self, st: &mut State, channel: Channel) {
        if st.locked {
            if channel == st.channel {
                st.locked = false;
                st.ignore_next_release = true;
                self.finish_and_process(st, false);
            } else {
                self.abort_recording(st, "锁定录音中按下另一个热键");
            }
            return;
        }
        if st.recording && channel != st.channel {
            self.abort_recording(st, "录音中按下另一个热键");
            return;
        }
        if st.held {
            return;
        }
        st.held = true;
        self.start_recording(st, channel);
    }

    fn release_hold(&self, st: &mut State, _channel: Channel) {
        if !st.held {
            return;
        }
        st.held = false;
        if st.cancelled {
            st.cancelled = false;
            st.recording = false;
            return;
        }
        if !st.recording {
            return;
        }
        let duration = self.recorder.lock().duration();
        if secs(duration) >= self.min_hold {
            self.finish_and_process(st, true);
            return;
        }
        let now = (self.clock)();
        let double_tap = st
            .last_short_tap
            .is_some_and(|last| now.saturating_duration_since(last) < DOUBLE_TAP_WINDOW);
        if double_tap {
            // 双击：进入锁定录音（丢弃两次极短音频，重新开始录）
            st.last_short_tap = None;
            Self::stop_preview(st);
            {
                let mut rec = self.recorder.lock();
                rec.abort();
                if let Err(e) = rec.start() {
                    warn!("锁定录音启动失败：{}", e);
                }
            }
            st.locked = true;
            info!("双击热键，进入锁定录音（再按一下结束）");
            self.play(SoundEvent::Start);
            self.status(Status::Recording);
            self.start_preview(st);
            return;
        }
        st.last_short_tap = Some(now);
        st.recording = false;
        Self::stop_preview(st);
        self.recorder.lock().abort();
        self.hud.hide();
        info!(
            "按住时长 {:.3}s < {:.2}s，丢弃本次录音",
            duration,
            self.min_hold.as_secs_f64()
        );
        self.status(Status::Idle);
    }

    // ---- 录音生命周期 ----

    fn start_recording(&self, st: &mut State, channel: Channel) {
        st.channel = channel;
        st.cancelled = false;
        st.recording = true;
        if let Err(e) = self.recorder.lock().start() {
            // Python 版这里异常会冒泡到 _on_press 的兜底 except；Rust 降级为告警，
            // 后续 stop() 拿到空 PCM 会走「无音频数据，丢弃」分支。
            warn!("启动录音失败：{}", e);
        }
        self.play(SoundEvent::Start);
        self.status(Status::Recording);
        self.start_preview(st);
    }

    pub(crate) fn abort_recording(&self, st: &mut State, reason: &str) {
        Self::stop_preview(st);
        self.recorder.lock().abort();
        st.recording = false;
        st.locked = false;
        st.cancelled = false;
        st.held = false;
        st.suppressed = false;
        info!("录音取消（{}）", reason);
        self.play(SoundEvent::Cancel);
        self.hud.hide();
        self.status(Status::Idle);
    }

    /// 停止录音，把音频交给 worker 线程处理（听写或改写）。
    fn finish_and_process(&self, st: &mut State, check_min_hold: bool) {
        st.recording = false;
        Self::stop_preview(st);
        let (pcm, duration) = self.recorder.lock().stop();
        if check_min_hold && secs(duration) < self.min_hold {
            info!("录音时长 {:.3}s 过短，丢弃", duration);
            self.hud.hide();
            self.status(Status::Idle);
            return;
        }
        if pcm.is_empty() {
            info!("无音频数据，丢弃");
            self.hud.hide();
            self.status(Status::Idle);
            return;
        }
        self.play(SoundEvent::Stop);
        self.status(Status::Transcribing);
        self.hud.set_status("转写中…", "");
        self.spawn_worker(st.channel, pcm);
    }

    // ---- 实时预览（悬浮窗伪流式） ----

    fn start_preview(&self, st: &mut State) {
        if !self.settings.live_preview() {
            return;
        }
        let Some(me) = self.me.upgrade() else {
            return;
        };
        let channel = st.channel;
        self.hud.begin(
            channel,
            hud::placeholder(channel),
            hud::hint_text(st.mode, st.locked),
        );
        // 丢弃 Sender 即广播停止；rendezvous channel 不占内存。
        let (tx, rx) = crossbeam_channel::bounded::<()>(0);
        st.preview_stop = Some(tx);
        let preview_rx = rx.clone();
        let preview_me = me.clone();
        let _ = thread::Builder::new()
            .name("xiaodao-preview".into())
            .spawn(move || preview_me.preview_loop(preview_rx));
        let _ = thread::Builder::new()
            .name("xiaodao-level".into())
            .spawn(move || me.level_loop(rx));
    }

    pub(crate) fn stop_preview(st: &mut State) {
        st.preview_stop = None;
    }

    /// ~12Hz 刷新 HUD 声浪：给用户「麦克风正在收到声音」的即时确认。
    fn level_loop(&self, rx: Receiver<()>) {
        while !wait_or_stop(&rx, LEVEL_INTERVAL) {
            if !self.is_recording() {
                break;
            }
            let (level, duration) = {
                let rec = self.recorder.lock();
                (rec.level(), rec.duration())
            };
            self.hud.set_level(level);
            self.hud.set_partial(duration, "");
        }
    }

    /// 伪流式预览：每 ~0.7s 把累积音频全量重转一遍，间隔随转写耗时自适应放大。
    fn preview_loop(&self, rx: Receiver<()>) {
        let mut interval = PREVIEW_MIN_INTERVAL;
        while !wait_or_stop(&rx, interval) {
            if !self.is_recording() {
                break;
            }
            let pcm = self.recorder.lock().snapshot();
            if pcm.len() < PREVIEW_MIN_SAMPLES {
                continue; // 不足 0.5s 音频只更新计时（由 level_loop 负责）
            }
            let t0 = Instant::now();
            let text = match self.transcriber.transcribe(&pcm, true) {
                Ok(text) => text,
                Err(e) => {
                    debug!("预览转写失败，停止预览：{}", e);
                    break;
                }
            };
            // 音频变长转写变慢时自动放缓刷新：interval = max(0.7s, cost × 2.5)
            interval = PREVIEW_MIN_INTERVAL.max(t0.elapsed() * PREVIEW_COST_FACTOR / 10);
            if stopped(&rx) || !self.is_recording() {
                break;
            }
            let duration = self.recorder.lock().duration();
            self.hud.set_partial(duration, &text);
        }
    }
}

/// 录音时长（秒，可能为负/NaN 时按 0 处理）转 Duration，用于与 min_hold 比较。
fn secs(duration: f64) -> Duration {
    Duration::from_secs_f64(if duration.is_finite() && duration > 0.0 {
        duration
    } else {
        0.0
    })
}
