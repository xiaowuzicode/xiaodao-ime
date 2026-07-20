"""全局热键与录音状态机。

两种录音方式（settings.json 的 record_mode，可在菜单「设置 → 录音方式」切换）：
  - toggle（默认）：单击热键开始录音，再单击一次结束并出字；
  - hold：按住热键说话、松开出字；0.35s 内双击进入锁定录音，再按一下结束。

防误触规则（两种模式通用）：
  1. 录音期间按下任何其他键（说明在用组合快捷键/开始打字），立即取消本次录音；
  2. toggle 模式下「热键+其他键」的组合快捷键不会误触发开始录音（松开热键时已被标记跳过）；
  3. 录音时长 < 0.4s 直接丢弃；转写结果为空不粘贴。

macOS 兼容：pynput 在 macOS 上把左 Option 上报为通用 Key.alt（而非 Key.alt_l）、
左 Command 上报为 Key.cmd，因此每个热键匹配一组按键而不是单个。

pynput 的 on_press/on_release 在同一监听线程内串行回调，状态无需加锁；
耗时的转写+润色+粘贴放到独立 worker 线程。
"""
import threading
import time
from typing import Callable, Optional

from pynput import keyboard

from xiaodao_ime.config import DOUBLE_TAP_WINDOW, MIN_HOLD_SECONDS
from xiaodao_ime.context import STYLE_OFF, frontmost_app, pick_style
from xiaodao_ime.feedback import play
from xiaodao_ime.logger import get_logger
from xiaodao_ime.paster import paste_text
from xiaodao_ime.polisher import apply_replacements
from xiaodao_ime.recorder import Recorder
from xiaodao_ime.transcriber import Transcriber

log = get_logger(__name__)


def _keyset(*names) -> frozenset:
    keys = set()
    for name in names:
        key = getattr(keyboard.Key, name, None)
        if key is not None:
            keys.add(key)
    return frozenset(keys)


# settings.json 的 hotkey 字段 -> (菜单显示名, 匹配按键集合)
# macOS 上左侧修饰键上报为无方向的 Key.alt / Key.cmd，一并匹配
HOTKEY_CHOICES = {
    "alt_l": ("左 Option", _keyset("alt_l", "alt")),
    "alt_r": ("右 Option", _keyset("alt_r")),
    "cmd_r": ("右 Command", _keyset("cmd_r")),
    "f19": ("F19", _keyset("f19")),
}
DEFAULT_HOTKEY = "alt_l"

RECORD_MODES = {
    "toggle": "单击开始 / 再击结束",
    "hold": "按住说话（双击锁定）",
}
DEFAULT_MODE = "toggle"


class HotkeyController:
    """管理单击/按住两种录音方式的完整生命周期。"""

    def __init__(self, recorder: Recorder, transcriber: Transcriber,
                 on_status: Optional[Callable[[str], None]] = None,
                 min_hold: float = MIN_HOLD_SECONDS,
                 polisher=None, settings=None, history=None,
                 hotkey: str = DEFAULT_HOTKEY, mode: str = DEFAULT_MODE):
        self._recorder = recorder
        self._transcriber = transcriber
        self._on_status = on_status or (lambda s: None)
        self._min_hold = min_hold
        self._polisher = polisher
        self._settings = settings
        self._history = history
        self._trigger = HOTKEY_CHOICES.get(hotkey, HOTKEY_CHOICES[DEFAULT_HOTKEY])[1]
        self._mode = mode if mode in RECORD_MODES else DEFAULT_MODE

        self._held = False              # 热键当前是否被按住
        self._suppressed = False        # toggle：按住热键期间出现组合键，本次单击作废
        self._recording = False
        self._locked = False            # hold：锁定录音（双击进入）
        self._cancelled = False
        self._ignore_next_release = False
        self._last_short_tap = 0.0      # hold：上一次短按时间（识别双击）
        self._saw_event = False         # 是否收到过键盘事件（输入监听权限探针）
        self._listener: Optional[keyboard.Listener] = None

    # ---- 外部配置 ----

    def set_trigger(self, hotkey: str) -> None:
        if hotkey not in HOTKEY_CHOICES:
            log.warning("未知热键 %r，忽略", hotkey)
            return
        if self._recording:
            self._abort_recording("切换热键")
        self._trigger = HOTKEY_CHOICES[hotkey][1]
        log.info("热键已切换为：%s", HOTKEY_CHOICES[hotkey][0])

    def set_mode(self, mode: str) -> None:
        if mode not in RECORD_MODES:
            log.warning("未知录音方式 %r，忽略", mode)
            return
        if self._recording:
            self._abort_recording("切换录音方式")
        self._mode = mode
        log.info("录音方式已切换为：%s", RECORD_MODES[mode])

    # ---- 内部工具 ----

    def _status(self, state: str) -> None:
        try:
            self._on_status(state)
        except Exception as e:
            log.debug("状态回调出错：%s", e)

    def _start_recording(self) -> None:
        self._cancelled = False
        self._recording = True
        self._recorder.start()
        play("start", self._settings)
        self._status("recording")

    def _abort_recording(self, reason: str) -> None:
        self._recorder.abort()
        self._recording = False
        self._locked = False
        self._cancelled = False
        self._held = False
        self._suppressed = False
        log.info("录音取消（%s）", reason)
        play("cancel", self._settings)
        self._status("idle")

    # ---- 键盘事件 ----

    def _on_press(self, key) -> None:  # noqa: ANN001
        try:
            if not self._saw_event:
                self._saw_event = True
                log.info("✅ 已收到键盘事件，输入监听权限正常（首个按键：%s）", key)
            if key in self._trigger:
                if self._mode == "toggle":
                    self._press_toggle()
                else:
                    self._press_hold()
            else:
                self._press_other()
        except Exception as e:
            log.error("按键按下处理异常：%s", e)

    def _on_release(self, key) -> None:  # noqa: ANN001
        try:
            if key not in self._trigger:
                return
            if self._ignore_next_release:
                self._ignore_next_release = False
                return
            if self._mode == "toggle":
                self._release_toggle()
            else:
                self._release_hold()
        except Exception as e:
            log.error("按键松开处理异常：%s", e)
            self._status("idle")

    def _press_other(self) -> None:
        """录音期间其他按键 => 取消；toggle 按住期间其他按键 => 组合键，本次单击作废。"""
        if self._recording and not self._cancelled:
            if self._mode == "toggle" or self._locked:
                self._abort_recording("录音中按下其他键")
            elif self._held:
                self._cancelled = True
                self._recording = False
                self._recorder.abort()
                log.info("检测到组合键，取消本次录音（防误触）")
                self._status("idle")
        elif self._held:
            self._suppressed = True

    # ---- toggle：单击开始 / 再击结束 ----

    def _press_toggle(self) -> None:
        if self._recording:
            # 第二击：结束录音并处理
            self._ignore_next_release = True
            self._finish_and_process()
            return
        if self._held:
            return  # 长按自动重复
        self._held = True
        self._suppressed = False

    def _release_toggle(self) -> None:
        if not self._held:
            return
        self._held = False
        if self._suppressed:
            self._suppressed = False
            return  # 刚才是组合快捷键，不开始录音
        self._start_recording()
        log.info("开始录音（单击模式，再按一次热键结束）")

    # ---- hold：按住说话 + 双击锁定 ----

    def _press_hold(self) -> None:
        if self._locked:
            self._locked = False
            self._ignore_next_release = True
            self._finish_and_process(check_min_hold=False)
            return
        if self._held:
            return
        self._held = True
        self._start_recording()

    def _release_hold(self) -> None:
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

    # ---- 收尾与后处理 ----

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
                # 场景感知：按前台 App 匹配润色风格（或对该 App 关闭润色）
                app_name, bundle_id = frontmost_app()
                style = None
                if self._settings is not None:
                    style = pick_style(app_name, bundle_id,
                                       self._settings.data.get("app_styles"))
                if app_name or bundle_id:
                    log.info("前台应用：%s（%s）→ 风格 %s", app_name, bundle_id, style or "默认")
                if style in STYLE_OFF:
                    log.info("该应用已配置关闭润色，直出转写")
                else:
                    self._status("polishing")
                    polished = self._polisher.polish(text, style=style)
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
        log.info("全局热键监听已启动（方式：%s）", RECORD_MODES[self._mode])

    def stop(self) -> None:
        if self._listener is not None:
            try:
                self._listener.stop()
            except Exception:
                pass
            self._listener = None
