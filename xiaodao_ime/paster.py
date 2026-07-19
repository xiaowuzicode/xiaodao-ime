"""粘贴：保存原剪贴板 → 写入转写文字 → Quartz 模拟 Cmd+V → 延迟恢复原剪贴板。"""
import threading
import time

from AppKit import NSPasteboard, NSPasteboardTypeString
import Quartz

from xiaodao_ime.config import CLIPBOARD_RESTORE_DELAY
from xiaodao_ime.logger import get_logger

log = get_logger(__name__)

_V_KEYCODE = 9  # macOS 虚拟键码：字母 V


def _read_clipboard_text() -> str | None:
    pb = NSPasteboard.generalPasteboard()
    return pb.stringForType_(NSPasteboardTypeString)


def _write_clipboard_text(text: str) -> None:
    pb = NSPasteboard.generalPasteboard()
    pb.clearContents()
    pb.setString_forType_(text, NSPasteboardTypeString)


def _send_cmd_v() -> None:
    """通过 Quartz CGEvent 模拟一次 Command+V。"""
    src = Quartz.CGEventSourceCreate(Quartz.kCGEventSourceStateHIDSystemState)
    key_down = Quartz.CGEventCreateKeyboardEvent(src, _V_KEYCODE, True)
    Quartz.CGEventSetFlags(key_down, Quartz.kCGEventFlagMaskCommand)
    key_up = Quartz.CGEventCreateKeyboardEvent(src, _V_KEYCODE, False)
    Quartz.CGEventSetFlags(key_up, Quartz.kCGEventFlagMaskCommand)
    Quartz.CGEventPost(Quartz.kCGHIDEventTap, key_down)
    Quartz.CGEventPost(Quartz.kCGHIDEventTap, key_up)


def paste_text(text: str, restore_delay: float = CLIPBOARD_RESTORE_DELAY) -> bool:
    """把 text 粘贴到当前焦点输入框。成功返回 True。

    流程：保存原剪贴板 → 写入 text → Cmd+V → 后台线程延迟恢复原剪贴板。
    """
    if not text or not text.strip():
        log.info("粘贴跳过：文本为空")
        return False
    try:
        original = _read_clipboard_text()
        _write_clipboard_text(text)
        # 给系统一点时间让剪贴板写入生效，再触发粘贴
        time.sleep(0.03)
        _send_cmd_v()
        log.info("已粘贴 %d 字符", len(text))

        def _restore() -> None:
            time.sleep(restore_delay)
            try:
                if original is not None:
                    _write_clipboard_text(original)
                else:
                    NSPasteboard.generalPasteboard().clearContents()
                log.debug("原剪贴板已恢复")
            except Exception as e:
                log.warning("恢复剪贴板失败：%s", e)

        threading.Thread(target=_restore, daemon=True).start()
        return True
    except Exception as e:
        log.error("粘贴失败：%s", e)
        return False
