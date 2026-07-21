"""粘贴：保存原剪贴板 → 写入转写文字 → Quartz 模拟 Cmd+V → 延迟恢复原剪贴板。"""
import threading
import time

from AppKit import NSPasteboard, NSPasteboardTypeString
import Quartz

from xiaodao_ime.config import CLIPBOARD_RESTORE_DELAY
from xiaodao_ime.logger import get_logger

log = get_logger(__name__)

_V_KEYCODE = 9  # macOS 虚拟键码：字母 V
_C_KEYCODE = 8  # macOS 虚拟键码：字母 C


def _read_clipboard_text() -> str | None:
    pb = NSPasteboard.generalPasteboard()
    return pb.stringForType_(NSPasteboardTypeString)


def _write_clipboard_text(text: str) -> None:
    pb = NSPasteboard.generalPasteboard()
    pb.clearContents()
    pb.setString_forType_(text, NSPasteboardTypeString)


def _send_cmd_key(keycode: int) -> None:
    """通过 Quartz CGEvent 模拟一次 Command+<key>。"""
    src = Quartz.CGEventSourceCreate(Quartz.kCGEventSourceStateHIDSystemState)
    key_down = Quartz.CGEventCreateKeyboardEvent(src, keycode, True)
    Quartz.CGEventSetFlags(key_down, Quartz.kCGEventFlagMaskCommand)
    key_up = Quartz.CGEventCreateKeyboardEvent(src, keycode, False)
    Quartz.CGEventSetFlags(key_up, Quartz.kCGEventFlagMaskCommand)
    Quartz.CGEventPost(Quartz.kCGHIDEventTap, key_down)
    Quartz.CGEventPost(Quartz.kCGHIDEventTap, key_up)


def _send_cmd_v() -> None:
    _send_cmd_key(_V_KEYCODE)


def grab_selection():
    """抓取当前焦点 App 中选中的文本。

    返回 (选中文本或 None, 原剪贴板内容)。原剪贴板内容由调用方负责最终恢复。
    实现：记录原剪贴板 → 写入哨兵值 → 模拟 Cmd+C → 读回；仍是哨兵说明没有选区。
    """
    original = _read_clipboard_text()
    sentinel = "​__xiaodao_sentinel__​"
    try:
        _write_clipboard_text(sentinel)
        time.sleep(0.05)
        _send_cmd_key(_C_KEYCODE)
        time.sleep(0.25)
        copied = _read_clipboard_text()
        if not copied or copied == sentinel:
            return None, original
        return copied, original
    except Exception as e:
        log.warning("抓取选区失败：%s", e)
        return None, original


def restore_clipboard(original) -> None:
    """立即恢复剪贴板（改写流程中止时用）。"""
    try:
        if original is not None:
            _write_clipboard_text(original)
        else:
            NSPasteboard.generalPasteboard().clearContents()
    except Exception as e:
        log.warning("恢复剪贴板失败：%s", e)


def copy_to_clipboard(text: str) -> bool:
    """仅复制到剪贴板（历史菜单用），不触发粘贴、不恢复原内容。"""
    try:
        _write_clipboard_text(text)
        return True
    except Exception as e:
        log.warning("复制到剪贴板失败：%s", e)
        return False


_UNSET = object()


def paste_text(text: str, restore_delay: float = CLIPBOARD_RESTORE_DELAY,
               restore_to=_UNSET) -> bool:
    """把 text 粘贴到当前焦点输入框。成功返回 True。

    流程：保存原剪贴板 → 写入 text → Cmd+V → 后台线程延迟恢复原剪贴板。
    restore_to：显式指定粘贴后要恢复的剪贴板内容（改写流程用，因为此时
    剪贴板里是 Cmd+C 抓来的选区，不是用户原本的内容）；缺省读取当前剪贴板。
    """
    if not text or not text.strip():
        log.info("粘贴跳过：文本为空")
        return False
    try:
        original = _read_clipboard_text() if restore_to is _UNSET else restore_to
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
