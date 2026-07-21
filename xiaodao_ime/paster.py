"""粘贴与选区抓取（平台无关流程；剪贴板/按键原语来自平台后端）。

粘贴：保存原剪贴板 → 写入转写文字 → 模拟 粘贴快捷键 → 延迟恢复原剪贴板。
抓选区：写入哨兵值 → 模拟 复制快捷键 → 读回；仍是哨兵说明没有选区。
"""
import threading
import time

from xiaodao_ime import platform as _platform
from xiaodao_ime.config import CLIPBOARD_RESTORE_DELAY
from xiaodao_ime.logger import get_logger

log = get_logger(__name__)


def grab_selection():
    """抓取当前焦点 App 中选中的文本。

    返回 (选中文本或 None, 原剪贴板内容)。原剪贴板内容由调用方负责最终恢复。
    """
    backend = _platform.backend
    original = backend.read_clipboard()
    sentinel = "​__xiaodao_sentinel__​"
    try:
        backend.write_clipboard(sentinel)
        time.sleep(0.05)
        backend.send_copy()
        time.sleep(0.25)
        copied = backend.read_clipboard()
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
            _platform.backend.write_clipboard(original)
        else:
            _platform.backend.clear_clipboard()
    except Exception as e:
        log.warning("恢复剪贴板失败：%s", e)


def copy_to_clipboard(text: str) -> bool:
    """仅复制到剪贴板（历史菜单用），不触发粘贴、不恢复原内容。"""
    try:
        _platform.backend.write_clipboard(text)
        return True
    except Exception as e:
        log.warning("复制到剪贴板失败：%s", e)
        return False


_UNSET = object()


def paste_text(text: str, restore_delay: float = CLIPBOARD_RESTORE_DELAY,
               restore_to=_UNSET) -> bool:
    """把 text 粘贴到当前焦点输入框。成功返回 True。

    流程：保存原剪贴板 → 写入 text → 粘贴 → 后台线程延迟恢复原剪贴板。
    restore_to：显式指定粘贴后要恢复的剪贴板内容（改写流程用，因为此时
    剪贴板里是抓选区留下的内容，不是用户原本的内容）；缺省读取当前剪贴板。
    """
    if not text or not text.strip():
        log.info("粘贴跳过：文本为空")
        return False
    backend = _platform.backend
    try:
        original = backend.read_clipboard() if restore_to is _UNSET else restore_to
        backend.write_clipboard(text)
        # 给系统一点时间让剪贴板写入生效，再触发粘贴
        time.sleep(0.03)
        backend.send_paste()
        log.info("已粘贴 %d 字符", len(text))

        def _restore() -> None:
            time.sleep(restore_delay)
            try:
                if original is not None:
                    backend.write_clipboard(original)
                else:
                    backend.clear_clipboard()
                log.debug("原剪贴板已恢复")
            except Exception as e:
                log.warning("恢复剪贴板失败：%s", e)

        threading.Thread(target=_restore, daemon=True).start()
        return True
    except Exception as e:
        log.error("粘贴失败：%s", e)
        return False
