"""全局热键与录音状态机。

按住左 Option（Key.alt_l）开始录音，松开停止并转写粘贴。
防误触规则：
  1. 按住左 Option 期间若有任何其他按键按下（说明在用 ⌥+x 快捷键），立即取消本次录音；
  2. 按住时长 < MIN_HOLD_SECONDS 的录音直接丢弃；
  3. 转写结果为空/纯空白时不粘贴。

pynput 的 on_press/on_release 在同一个监听线程内串行回调，因此状态无需加锁；
耗时的转写+粘贴放到独立 worker 线程，避免阻塞监听线程。
"""
import threading
from typing import Callable, Optional

from pynput import keyboard

from xiaodao_ime.config import MIN_HOLD_SECONDS
from xiaodao_ime.logger import get_logger
from xiaodao_ime.paster import paste_text
from xiaodao_ime.recorder import Recorder
from xiaodao_ime.transcriber import Transcriber

log = get_logger(__name__)

_TRIGGER_KEY = keyboard.Key.alt_l  # 左 Option 键


class HotkeyController:
    """管理按住说话的完整生命周期。"""

    def __init__(self, recorder: Recorder, transcriber: Transcriber,
                 on_status: Optional[Callable[[str], None]] = None,
                 min_hold: float = MIN_HOLD_SECONDS):
        self._recorder = recorder
        self._transcriber = transcriber
        self._on_status = on_status or (lambda s: None)
        self._min_hold = min_hold

        self._alt_down = False
        self._recording = False
        self._cancelled = False
        self._listener: Optional[keyboard.Listener] = None

    # ---- 状态回调 ----
    def _status(self, state: str) -> None:
        try:
            self._on_status(state)
        except Exception as e:
            log.debug("状态回调出错：%s", e)

    # ---- 键盘事件 ----
    def _on_press(self, key) -> None:  # noqa: ANN001
        try:
            if key == _TRIGGER_KEY:
                if self._alt_down:
                    return  # 忽略按住时的自动重复
                self._alt_down = True
                self._cancelled = False
                self._recording = True
                self._recorder.start()
                self._status("recording")
            else:
                # 按住左 Option 期间的其他按键 => 用户在用快捷键，取消本次录音
                if self._alt_down and self._recording and not self._cancelled:
                    self._cancelled = True
                    self._recording = False
                    self._recorder.abort()
                    log.info("检测到组合键，取消本次录音（防误触）")
                    self._status("idle")
        except Exception as e:
            log.error("按键按下处理异常：%s", e)

    def _on_release(self, key) -> None:  # noqa: ANN001
        try:
            if key != _TRIGGER_KEY:
                return
            if not self._alt_down:
                return
            self._alt_down = False
            if self._cancelled:
                # 已在按下其他键时取消，无需处理
                self._cancelled = False
                self._recording = False
                return
            if not self._recording:
                return
            self._recording = False
            pcm, duration = self._recorder.stop()
            if duration < self._min_hold:
                log.info("按住时长 %.3fs < %.2fs，丢弃本次录音", duration, self._min_hold)
                self._status("idle")
                return
            if pcm is None or len(pcm) == 0:
                log.info("无音频数据，丢弃")
                self._status("idle")
                return
            # 转写 + 粘贴放到 worker 线程
            self._status("transcribing")
            threading.Thread(
                target=self._transcribe_and_paste, args=(pcm,), daemon=True
            ).start()
        except Exception as e:
            log.error("按键松开处理异常：%s", e)
            self._status("idle")

    def _transcribe_and_paste(self, pcm) -> None:  # noqa: ANN001
        try:
            text, _ = self._transcriber.transcribe(pcm)
            if not text or not text.strip():
                log.info("转写结果为空，不粘贴")
                return
            paste_text(text)
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
        log.info("全局热键监听已启动（按住左 Option 说话）")

    def stop(self) -> None:
        if self._listener is not None:
            try:
                self._listener.stop()
            except Exception:
                pass
            self._listener = None
