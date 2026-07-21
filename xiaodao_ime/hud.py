"""实时预览悬浮窗（HUD）：平台无关的展示组合逻辑。

窗口本体由平台后端提供（HUDWindow：show_lines(main, hint) / hide()），
本模块负责把「计时 + 声浪 + 识别文本 + 操作提示」组合成两行文本：

    🎙️ 12s ▁▂▅▇▆▃▁▁▂▄  …识别中的文本尾部
    再按热键出字 · 按 Esc 取消

声浪是最近 N 次音量电平的滚动波形（块字符渲染），让用户一眼确认
「麦克风正在收到我的声音」；转写/润色阶段主行显示状态、副行显示已
转写全文尾部，等待不再是黑盒。
"""
from collections import deque

from xiaodao_ime import platform as _platform
from xiaodao_ime.logger import get_logger

log = get_logger(__name__)

_BLOCKS = "▁▂▃▄▅▆▇█"
_WAVE_SLOTS = 10       # 声浪显示最近多少个电平采样
_MAX_TAIL = 40         # 主行识别文本尾部最多字符数
_MAX_DETAIL = 56       # 状态副行（如润色中的已转写全文）尾部最多字符数


def _tail(text: str, limit: int) -> str:
    if not text:
        return ""
    return text if len(text) <= limit else "…" + text[-limit:]


class PreviewHUD:
    """录音会话期间的悬浮窗。方法均线程安全（后端负责线程调度）。"""

    def __init__(self, window=None):
        self._win = window if window is not None else _platform.backend.HUDWindow()
        self._levels = deque([0.0] * _WAVE_SLOTS, maxlen=_WAVE_SLOTS)
        self._elapsed = 0
        self._partial = ""
        self._placeholder = ""
        self._prefix = "🎙️"
        self._hint = ""
        self._recording = False

    # ---- 录音会话 ----

    def begin(self, prefix: str = "🎙️", placeholder: str = "",
              hint: str = "") -> None:
        """开始一次录音会话：重置状态并立即显示。"""
        self._levels = deque([0.0] * _WAVE_SLOTS, maxlen=_WAVE_SLOTS)
        self._elapsed = 0
        self._partial = ""
        self._placeholder = placeholder or "聆听中…"
        self._prefix = prefix
        self._hint = hint
        self._recording = True
        self._redraw()

    def set_level(self, level: float) -> None:
        """推入一个音量电平（0~1），刷新声浪。约 10Hz 调用。"""
        if not self._recording:
            return
        self._levels.append(max(0.0, min(1.0, float(level))))
        self._redraw()

    def set_partial(self, elapsed: float, text: str) -> None:
        """更新已录秒数与伪流式识别文本（文本为空则保留占位/上次结果）。"""
        if not self._recording:
            return
        self._elapsed = int(elapsed)
        if text:
            self._partial = text
        self._redraw()

    def set_hint(self, hint: str) -> None:
        if self._recording:
            self._hint = hint
            self._redraw()

    # ---- 录音后的状态展示 ----

    def set_status(self, status: str, detail: str = "") -> None:
        """转写/润色等阶段：主行状态文案，副行给已转写文本尾部（可空）。"""
        self._recording = False
        try:
            self._win.show_lines(status, _tail(detail, _MAX_DETAIL))
        except Exception as e:
            log.debug("HUD 状态更新失败：%s", e)

    def hide(self) -> None:
        self._recording = False
        try:
            self._win.hide()
        except Exception as e:
            log.debug("HUD 隐藏失败：%s", e)

    # ---- 渲染 ----

    def _wave(self) -> str:
        top = len(_BLOCKS) - 1
        return "".join(_BLOCKS[int(lv * top)] for lv in self._levels)

    def _redraw(self) -> None:
        text = _tail(self._partial, _MAX_TAIL) or self._placeholder
        main = f"{self._prefix} {self._elapsed}s {self._wave()}  {text}"
        try:
            self._win.show_lines(main, self._hint)
        except Exception as e:
            log.debug("HUD 更新失败：%s", e)
