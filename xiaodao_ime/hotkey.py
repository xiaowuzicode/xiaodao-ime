"""全局热键与录音状态机。

两种录音方式：
  1. 按住说话：按住热键（默认左 Option，可在设置菜单切换）录音，松开即转写、（可选）润色、粘贴；
  2. 双击锁定：0.35s 内连击两下热键进入「锁定录音」，可长段口述无需一直按着，
     再按一下热键结束并转写。

防误触规则：
  1. 录音期间（按住或锁定）若有任何其他按键按下（说明在用组合快捷键/开始打字），立即取消本次录音；
  2. 单次按住时长 < 0.4s 的录音直接丢弃（但会被记为双击的第一击）；
  3. 转写结果为空/纯空白时不粘贴。

pynput 的 on_press/on_release 在同一个监听线程内串行回调，因此状态无需加锁；
耗时的转写+润色+粘贴放到独立 worker 线程，避免阻塞监听线程。
"""
import threading
import time
from typing import Callable, Optional

from pynput import keyboard

from xiaodao_ime.config import DOUBLE_TAP_WINDOW, MIN_HOLD_SECONDS
from xiaodao_ime.feedback import play
from xiaodao_ime.logger import get_logger
from xiaodao_ime.paster import paste_text
from xiaodao_ime.polisher import apply_replacements
from xiaodao_ime.recorder import Recorder
from xiaodao_ime.transcriber import Transcriber

log = get_logger(__name__)

# 可选热键：settings.json 的 hotkey 字段 -> (菜单显示名, pynput 键)
HOTKEY_CHOICES = {
    "alt_l": ("左 Option", keyboard.Key.alt_l),
    "alt_r": ("右 Option", keyboard.Key.alt_r),
    "cmd_r": ("右 Command", keyboard.Key.cmd_r),
    "f19": ("F19", keyboard.Key.f19),
}
DEFAULT_HOTKEY = "alt_l"


class HotkeyController:
    """管理按住/锁定说话的完整生命周期。"""

    def __init__(self, recorder: Recorder, transcriber: Transcriber,
                 on_status: Optional[Callable[[str], None]] = None,
                 min_hold: float = MIN_HOLD_SECONDS,
                 polisher=None, settings=None, history=None,
                 hotkey: str = DEFAULT_HOTKEY):
        self._recorder = recorder
        self._transcriber = transcriber
        self._on_status = on_status or (lambda s: None)
        self._min_hold = min_hold
        self._polisher = polisher
        self._settings = settings
        self._history = history
        self._trigger = HOTKEY_CHOICES.get(hotkey, HOTKEY_CHOICES[DEFAULT_HOTKEY])[1]

        self._held = False            # 热键当前是否被按住
        self._recording = False
        self._locked = False          # 锁定录音模式（双击进入）
        self._cancelled = False
        self._ignore_next_release = False
        self._last_short_tap = 0.0    # 上一次「短按」的时间（用于识别双击）
        self._listener: Optional[keyboard.Listener] = None

    def set_trigger(self, hotkey: str) -> None:
        """切换按住说话的热键（来自设置菜单），录音中切换则先取消本次录音。"""
        if hotkey not in HOTKEY_CHOICES:
            log.warning("未知热键 %r，忽略", hotkey)
            return
        if self._recording:
            self._abort_recording("切换热键")
        self._trigger = HOTKEY_CHOICES[hotkey][1]
        log.info("热键已切换为：%s", HOTKEY_CHOICES[hotkey][0])

    # ---- 状态回调 ----
    def _status(self, state: str) -> None:
        try:
            self._on_status(state)
        except Exception as e:
            log.debug("状态回调出错：%s", e)

    def _abort_recording(self, reason: str) -> None:
        self._recorder.abort()
        self._recording = False
        self._locked = False
        self._cancelled = False
        self._held = False
        log.info("录音取消（%s）", reason)
        play("cancel", self._settings)
        self._status("idle")

    # ---- 键盘事件 ----
    def _on_press(self, key) -> None:  # noqa: ANN001
        try:
            if key == self._trigger:
                if self._locked:
                    # 锁定模式下再按一下：结束录音并处理
                    self._locked = False
                    self._ignore_next_release = True
                    self._finish_and_process(check_min_hold=False)
                    return
                if self._held:
                    return  # 忽略按住时的自动重复
                self._held = True
                self._cancelled = False
                self._recording = True
                self._recorder.start()
                play("start", self._settings)
                self._status("recording")
            else:
                # 录音期间按下其他键 => 用户在用快捷键/打字，取消本次录音
                if self._recording and not self._cancelled:
                    if self._locked:
                        self._abort_recording("锁定录音中按下其他键")
                    elif self._held:
                        self._cancelled = True
                        self._recording = False
                        self._recorder.abort()
                        log.info("检测到组合键，取消本次录音（防误触）")
                        self._status("idle")
        except Exception as e:
            log.error("按键按下处理异常：%s", e)

    def _on_release(self, key) -> None:  # noqa: ANN001
        try:
            if key != self._trigger:
                return
            if self._ignore_next_release:
                self._ignore_next_release = False
                return
            if not self._held:
                return
            self._held = False
            if self._cancelled:
                self._cancelled = False
                self._recording = False
                return
            if not self._recording:
                return
            duration = self._recorder.duration()
            if duration < self._min_hold:
                now = time.monotonic()
                if now - self._last_short_tap < DOUBLE_TAP_WINDOW:
                    # 双击：进入锁定录音（丢弃两次极短音频，重新开始录）
                    self._last_short_tap = 0.0
                    self._recorder.abort()
                    self._recorder.start()
                    self._locked = True
                    log.info("双击热键，进入锁定录音（再按一下结束）")
                    play("start", self._settings)
                    self._status("recording")
                    return
                self._last_short_tap = now
                self._recording = False
                self._recorder.abort()
                log.info("按住时长 %.3fs < %.2fs，丢弃本次录音", duration, self._min_hold)
                self._status("idle")
                return
            self._finish_and_process()
        except Exception as e:
            log.error("按键松开处理异常：%s", e)
            self._status("idle")

    def _finish_and_process(self, check_min_hold: bool = True) -> None:
        """停止录音，把音频交给 worker 线程转写/润色/粘贴。"""
        self._recording = False
        pcm, duration = self._recorder.stop()
        if check_min_hold and duration < self._min_hold:
            log.info("录音时长 %.3fs 过短，丢弃", duration)
            self._status("idle")
            return
        if pcm is None or len(pcm) == 0:
            log.info("无音频数据，丢弃")
            self._status("idle")
            return
        play("stop", self._settings)
        self._status("transcribing")
        threading.Thread(
            target=self._transcribe_and_paste, args=(pcm,), daemon=True
        ).start()

    def _transcribe_and_paste(self, pcm) -> None:  # noqa: ANN001
        try:
            raw, _ = self._transcriber.transcribe(pcm)
            if not raw or not raw.strip():
                log.info("转写结果为空，不粘贴")
                return
            text = raw
            if self._settings is not None:
                text = apply_replacements(text, self._settings.data.get("replacements"))
            if self._polisher is not None and self._polisher.enabled:
                self._status("polishing")
                polished = self._polisher.polish(text)
                if polished:
                    text = polished  # 润色失败时 polish 返回 None，直接用原始转写
            paste_text(text)
            if self._history is not None:
                self._history.append(raw, text)
        except Exception as e:
            log.error("转写/粘贴流程异常：%s", e)
        finally:
            self._status("idle")

    # ---- 生命周期 ----
    def start(self) -> None:
        """启动全局键盘监听（非阻塞）。"""
        self._listener = keyboard.Listener(
            on_press=self._on_press, on_release=self._on_release
        )
        self._listener.start()
        log.info("全局热键监听已启动（按住说话；双击锁定录音）")

    def stop(self) -> None:
        if self._listener is not None:
            try:
                self._listener.stop()
            except Exception:
                pass
            self._listener = None
