"""全局热键与录音状态机。

两个热键通道：
  - 听写（默认左 Option）：录音 → 转写 →（可选）润色 → 粘贴到光标处；
  - 语音改写（默认右 Option）：先选中一段文字，按改写键说指令（如「改成英文」），
    松手后抓取选区 + 指令一起交给大模型，结果原地替换选区。

两种录音方式（settings.json 的 record_mode，可在菜单「设置 → 录音方式」切换）：
  - toggle（默认）：单击热键开始录音，再单击一次结束；
  - hold：按住热键说话、松开出字；0.35s 内双击进入锁定录音，再按一下结束。

实时预览：录音期间每 ~0.7s 把累积音频全量重转一遍（SenseVoice 足够快），
结果推送到悬浮窗 HUD；间隔随单次转写耗时自适应放大。

防误触规则：
  1. 录音期间按下任何其他键（含另一个热键），立即取消本次录音；
  2. toggle 模式下「热键+其他键」组合快捷键不会误触发开始录音；
  3. 录音时长 < 0.4s 直接丢弃；转写结果为空不粘贴。

macOS 兼容：pynput 把左 Option 上报为 Key.alt（非 Key.alt_l）、左 Command 上报为
Key.cmd，因此每个热键匹配一组按键。
"""
import threading
import time
from typing import Callable, Optional

from pynput import keyboard

from xiaodao_ime.config import DOUBLE_TAP_WINDOW, MIN_HOLD_SECONDS
from xiaodao_ime.context import STYLE_OFF, frontmost_app, pick_style
from xiaodao_ime.platform import DEFAULT_HOTKEY, DEFAULT_REWRITE_HOTKEY, IS_MAC
from xiaodao_ime.feedback import play
from xiaodao_ime.logger import get_logger
from xiaodao_ime.paster import grab_selection, paste_text, restore_clipboard
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


# settings.json 的 hotkey / rewrite_hotkey 字段 -> (菜单显示名, 匹配按键集合)
if IS_MAC:
    HOTKEY_CHOICES = {
        "alt_l": ("左 Option", _keyset("alt_l", "alt")),
        "alt_r": ("右 Option", _keyset("alt_r")),
        "cmd_r": ("右 Command", _keyset("cmd_r")),
        "f19": ("F19", _keyset("f19")),
    }
else:
    # Windows：右 Alt 在欧洲布局是 AltGr（pynput 可能上报 alt_gr），不做默认；
    # 左 Alt 会聚焦窗口菜单栏，不提供。
    HOTKEY_CHOICES = {
        "ctrl_r": ("右 Ctrl", _keyset("ctrl_r")),
        "f8": ("F8", _keyset("f8")),
        "f9": ("F9", _keyset("f9")),
        "alt_r": ("右 Alt（AltGr 键盘慎用）", _keyset("alt_r", "alt_gr")),
    }

RECORD_MODES = {
    "toggle": "单击开始 / 再击结束",
    "hold": "按住说话（双击锁定）",
}
DEFAULT_MODE = "toggle"

_PREVIEW_MIN_SAMPLES = 8000  # 累积不足 0.5s 音频时预览只显示计时


class HotkeyController:
    """管理听写/改写两通道、单击/按住两方式的录音生命周期。"""

    def __init__(self, recorder: Recorder, transcriber: Transcriber,
                 on_status: Optional[Callable[[str], None]] = None,
                 min_hold: float = MIN_HOLD_SECONDS,
                 polisher=None, settings=None, history=None,
                 hud=None, notifier: Optional[Callable[[str, str], None]] = None,
                 hotkey: str = DEFAULT_HOTKEY,
                 rewrite_hotkey: str = DEFAULT_REWRITE_HOTKEY,
                 mode: str = DEFAULT_MODE):
        self._recorder = recorder
        self._transcriber = transcriber
        self._on_status = on_status or (lambda s: None)
        self._min_hold = min_hold
        self._polisher = polisher
        self._settings = settings
        self._history = history
        self._hud = hud
        self._notifier = notifier
        self._trigger = HOTKEY_CHOICES.get(hotkey, HOTKEY_CHOICES[DEFAULT_HOTKEY])[1]
        self._rewrite_trigger = HOTKEY_CHOICES.get(
            rewrite_hotkey, HOTKEY_CHOICES[DEFAULT_REWRITE_HOTKEY])[1]
        self._mode = mode if mode in RECORD_MODES else DEFAULT_MODE

        self._paused = False            # 暂停热键（菜单总开关），True 时忽略一切按键
        self._channel = "dictate"       # 当前录音属于哪个通道：dictate / rewrite
        self._held = False              # 热键当前是否被按住
        self._suppressed = False        # toggle：按住热键期间出现组合键，本次单击作废
        self._recording = False
        self._locked = False            # hold：锁定录音（双击进入）
        self._cancelled = False
        self._ignore_next_release = False
        self._last_short_tap = 0.0      # hold：上一次短按时间（识别双击）
        self._saw_event = False         # 是否收到过键盘事件（输入监听权限探针）
        self._preview_stop: Optional[threading.Event] = None
        self._listener: Optional[keyboard.Listener] = None

    # ---- 外部配置 ----

    def set_trigger(self, hotkey: str) -> None:
        if hotkey not in HOTKEY_CHOICES:
            log.warning("未知热键 %r，忽略", hotkey)
            return
        if self._recording:
            self._abort_recording("切换热键")
        self._trigger = HOTKEY_CHOICES[hotkey][1]
        log.info("听写热键已切换为：%s", HOTKEY_CHOICES[hotkey][0])

    def set_rewrite_trigger(self, hotkey: str) -> None:
        if hotkey not in HOTKEY_CHOICES:
            log.warning("未知改写热键 %r，忽略", hotkey)
            return
        if self._recording:
            self._abort_recording("切换改写热键")
        self._rewrite_trigger = HOTKEY_CHOICES[hotkey][1]
        log.info("改写热键已切换为：%s", HOTKEY_CHOICES[hotkey][0])

    def set_mode(self, mode: str) -> None:
        if mode not in RECORD_MODES:
            log.warning("未知录音方式 %r，忽略", mode)
            return
        if self._recording:
            self._abort_recording("切换录音方式")
        self._mode = mode
        log.info("录音方式已切换为：%s", RECORD_MODES[mode])

    @property
    def paused(self) -> bool:
        return self._paused

    def set_paused(self, paused: bool) -> None:
        """暂停/恢复热键监听（不销毁 listener，只忽略事件）。"""
        if paused and self._recording:
            self._abort_recording("暂停热键")
        self._paused = bool(paused)
        self._held = False
        self._suppressed = False
        log.info("热键已%s", "暂停" if paused else "恢复")

    # ---- 内部工具 ----

    def _status(self, state: str) -> None:
        try:
            self._on_status(state)
        except Exception as e:
            log.debug("状态回调出错：%s", e)

    def _notify(self, title: str, message: str) -> None:
        log.info("通知：%s —— %s", title, message)
        if self._notifier is not None:
            try:
                self._notifier(title, message)
            except Exception as e:
                log.debug("发送通知失败：%s", e)

    def _start_recording(self, channel: str) -> None:
        self._channel = channel
        self._cancelled = False
        self._recording = True
        self._recorder.start()
        play("start", self._settings)
        self._status("recording")
        self._start_preview()

    def _abort_recording(self, reason: str) -> None:
        self._stop_preview()
        self._recorder.abort()
        self._recording = False
        self._locked = False
        self._cancelled = False
        self._held = False
        self._suppressed = False
        log.info("录音取消（%s）", reason)
        play("cancel", self._settings)
        if self._hud is not None:
            self._hud.hide()
        self._status("idle")

    # ---- 实时预览（悬浮窗伪流式） ----

    def _preview_enabled(self) -> bool:
        return (self._hud is not None and self._settings is not None
                and bool(self._settings.data.get("live_preview", True)))

    def _hint_text(self) -> str:
        """HUD 副行操作提示（交互可发现性：告诉用户怎么结束/取消）。"""
        if self._mode == "toggle" or self._locked:
            return "再按热键出字 · 按 Esc 取消"
        return "松开出字 · 快速双击可锁定 · 按 Esc 取消"

    def _start_preview(self) -> None:
        if not self._preview_enabled():
            return
        rewrite = self._channel == "rewrite"
        self._hud.begin(
            prefix="🪄" if rewrite else "🎙️",
            placeholder="说出改写指令…" if rewrite else "聆听中…",
            hint=self._hint_text(),
        )
        self._preview_stop = threading.Event()
        threading.Thread(
            target=self._preview_loop, args=(self._preview_stop,), daemon=True
        ).start()
        threading.Thread(
            target=self._level_loop, args=(self._preview_stop,), daemon=True
        ).start()

    def _stop_preview(self) -> None:
        if self._preview_stop is not None:
            self._preview_stop.set()
            self._preview_stop = None

    def _level_loop(self, stop_event: threading.Event) -> None:
        """~12Hz 刷新 HUD 声浪：给用户「麦克风正在收到声音」的即时确认。"""
        while not stop_event.wait(0.08):
            if not self._recording:
                break
            try:
                self._hud.set_level(self._recorder.level())
                self._hud.set_partial(self._recorder.duration(), "")
            except Exception as e:
                log.debug("声浪刷新失败，停止：%s", e)
                break

    def _preview_loop(self, stop_event: threading.Event) -> None:
        interval = 0.7
        while not stop_event.wait(interval):
            if not self._recording:
                break
            pcm = self._recorder.snapshot()
            if len(pcm) < _PREVIEW_MIN_SAMPLES:
                continue
            try:
                t0 = time.perf_counter()
                text, _ = self._transcriber.transcribe(pcm, partial=True)
                cost = time.perf_counter() - t0
                interval = max(0.7, cost * 2.5)  # 音频变长转写变慢时自动放缓刷新
            except Exception as e:
                log.debug("预览转写失败，停止预览：%s", e)
                break
            if stop_event.is_set() or not self._recording:
                break
            self._hud.set_partial(self._recorder.duration(), text)

    # ---- 键盘事件 ----

    def _match_channel(self, key) -> Optional[str]:
        if key in self._trigger:
            return "dictate"
        if key in self._rewrite_trigger:
            return "rewrite"
        return None

    def _on_press(self, key) -> None:  # noqa: ANN001
        try:
            if not self._saw_event:
                self._saw_event = True
                log.info("✅ 已收到键盘事件，输入监听权限正常（首个按键：%s）", key)
            if self._paused:
                return
            channel = self._match_channel(key)
            if channel is not None:
                if self._mode == "toggle":
                    self._press_toggle(channel)
                else:
                    self._press_hold(channel)
            else:
                self._press_other()
        except Exception as e:
            log.error("按键按下处理异常：%s", e)

    def _on_release(self, key) -> None:  # noqa: ANN001
        try:
            if self._paused:
                return
            channel = self._match_channel(key)
            if channel is None:
                return
            if self._ignore_next_release:
                self._ignore_next_release = False
                return
            if self._mode == "toggle":
                self._release_toggle(channel)
            else:
                self._release_hold(channel)
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
                self._stop_preview()
                self._recorder.abort()
                if self._hud is not None:
                    self._hud.hide()
                log.info("检测到组合键，取消本次录音（防误触）")
                self._status("idle")
        elif self._held:
            self._suppressed = True

    # ---- toggle：单击开始 / 再击结束 ----

    def _press_toggle(self, channel: str) -> None:
        if self._recording:
            if channel == self._channel:
                # 第二击：结束录音并处理
                self._ignore_next_release = True
                self._finish_and_process()
            else:
                self._abort_recording("录音中按下另一个热键")
            return
        if self._held:
            return  # 长按自动重复
        self._held = True
        self._suppressed = False
        self._pending_channel = channel

    def _release_toggle(self, channel: str) -> None:
        if not self._held:
            return
        self._held = False
        if self._suppressed:
            self._suppressed = False
            return  # 刚才是组合快捷键，不开始录音
        self._start_recording(getattr(self, "_pending_channel", channel))
        log.info("开始录音（%s，单击模式，再按一次热键结束）",
                 "语音改写" if self._channel == "rewrite" else "听写")

    # ---- hold：按住说话 + 双击锁定 ----

    def _press_hold(self, channel: str) -> None:
        if self._locked:
            if channel == self._channel:
                self._locked = False
                self._ignore_next_release = True
                self._finish_and_process(check_min_hold=False)
            else:
                self._abort_recording("锁定录音中按下另一个热键")
            return
        if self._recording and channel != self._channel:
            self._abort_recording("录音中按下另一个热键")
            return
        if self._held:
            return
        self._held = True
        self._start_recording(channel)

    def _release_hold(self, channel: str) -> None:
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
                self._stop_preview()
                self._recorder.abort()
                self._recorder.start()
                self._locked = True
                log.info("双击热键，进入锁定录音（再按一下结束）")
                play("start", self._settings)
                self._status("recording")
                self._start_preview()
                return
            self._last_short_tap = now
            self._recording = False
            self._stop_preview()
            self._recorder.abort()
            if self._hud is not None:
                self._hud.hide()
            log.info("按住时长 %.3fs < %.2fs，丢弃本次录音", duration, self._min_hold)
            self._status("idle")
            return
        self._finish_and_process()

    # ---- 收尾与后处理 ----

    def _finish_and_process(self, check_min_hold: bool = True) -> None:
        """停止录音，把音频交给 worker 线程处理（听写或改写）。"""
        self._recording = False
        self._stop_preview()
        pcm, duration = self._recorder.stop()
        if check_min_hold and duration < self._min_hold:
            log.info("录音时长 %.3fs 过短，丢弃", duration)
            if self._hud is not None:
                self._hud.hide()
            self._status("idle")
            return
        if pcm is None or len(pcm) == 0:
            log.info("无音频数据，丢弃")
            if self._hud is not None:
                self._hud.hide()
            self._status("idle")
            return
        play("stop", self._settings)
        self._status("transcribing")
        if self._hud is not None:
            self._hud.set_status("✍️ 转写中…")
        target = (self._rewrite_and_replace if self._channel == "rewrite"
                  else self._transcribe_and_paste)
        threading.Thread(target=target, args=(pcm,), daemon=True).start()

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
                    if self._hud is not None:
                        # 等待不做黑盒：润色期间先把已转写全文亮出来
                        self._hud.set_status("🪄 润色中…", text)
                    polished = self._polisher.polish(text, style=style)
                    if polished:
                        text = polished  # 润色失败时 polish 返回 None，直接用原始转写
            paste_text(text)
            if self._history is not None:
                self._history.append(raw, text)
        except Exception as e:
            log.error("转写/粘贴流程异常：%s", e)
        finally:
            if self._hud is not None:
                self._hud.hide()
            self._status("idle")

    def _rewrite_and_replace(self, pcm) -> None:  # noqa: ANN001
        """语音改写：抓选区 → 识别指令 → LLM 改写 → 原地替换。全程 fail-open。"""
        original = None
        original_owned = True
        try:
            if self._polisher is None or not self._polisher.configured:
                play("cancel", self._settings)
                self._notify("语音改写不可用",
                             "请先在「设置 → 打开配置文件」配置 polish 的 base_url / api_key")
                return
            selection, original = grab_selection()
            if not selection or not selection.strip():
                play("cancel", self._settings)
                self._notify("未检测到选中文字",
                             "先选中要改写的文本，再按改写热键说指令")
                return
            if self._hud is not None:
                self._hud.set_status("✍️ 识别指令中…", selection)
            instruction, _ = self._transcriber.transcribe(pcm)
            instruction = (instruction or "").strip()
            if not instruction:
                play("cancel", self._settings)
                self._notify("没听清指令", "再按改写热键说一次？")
                return
            log.info("改写指令：%r，选区 %d 字符", instruction, len(selection))
            if self._hud is not None:
                self._hud.set_status(f"🪄 改写中：{instruction[:24]}", selection)
            self._status("polishing")
            result = self._polisher.rewrite(selection, instruction)
            if not result:
                play("cancel", self._settings)
                self._notify("改写失败", "模型没有返回结果，原文未改动")
                return
            paste_text(result, restore_to=original)
            original_owned = False  # 剪贴板恢复交给 paste_text
            if self._history is not None:
                self._history.append(f"〔改写〕{instruction}", result)
        except Exception as e:
            log.error("改写流程异常：%s", e)
        finally:
            if original_owned and original is not None:
                restore_clipboard(original)
            if self._hud is not None:
                self._hud.hide()
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
        self._stop_preview()
        if self._listener is not None:
            try:
                self._listener.stop()
            except Exception:
                pass
            self._listener = None
