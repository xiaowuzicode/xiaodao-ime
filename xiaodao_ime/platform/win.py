"""Windows 后端：Win32 剪贴板（ctypes）/ pynput 按键注入 / tkinter 悬浮窗 / winsound。

接口约定见 xiaodao_ime/platform/__init__.py。
Windows 没有 macOS 式 TCC 权限体系：全局键盘钩子与按键注入开箱即用，
仅麦克风受「设置 → 隐私 → 麦克风」控制（拿不到音频时提示用户即可）。
"""
import ctypes
import os
import queue
import threading
from ctypes import wintypes

from pynput.keyboard import Controller, Key

from xiaodao_ime.logger import get_logger

log = get_logger(__name__)

_u32 = ctypes.windll.user32
_k32 = ctypes.windll.kernel32

# 64 位下句柄/指针默认按 c_int 截断，必须显式声明（经典 ctypes 坑）
_u32.GetClipboardData.restype = wintypes.HANDLE
_u32.GetClipboardData.argtypes = [wintypes.UINT]
_u32.SetClipboardData.restype = wintypes.HANDLE
_u32.SetClipboardData.argtypes = [wintypes.UINT, wintypes.HANDLE]
_u32.OpenClipboard.argtypes = [wintypes.HWND]
_k32.GlobalAlloc.restype = wintypes.HGLOBAL
_k32.GlobalAlloc.argtypes = [wintypes.UINT, ctypes.c_size_t]
_k32.GlobalLock.restype = wintypes.LPVOID
_k32.GlobalLock.argtypes = [wintypes.HGLOBAL]
_k32.GlobalUnlock.argtypes = [wintypes.HGLOBAL]
_k32.OpenProcess.restype = wintypes.HANDLE
_k32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]

_CF_UNICODETEXT = 13
_GMEM_MOVEABLE = 0x0002


# ---- 剪贴板 ----

def _open_clipboard(retries: int = 10) -> bool:
    """其他进程占用剪贴板时 OpenClipboard 会失败，短重试。"""
    import time
    for _ in range(retries):
        if _u32.OpenClipboard(None):
            return True
        time.sleep(0.01)
    return False


def read_clipboard():
    if not _open_clipboard():
        return None
    try:
        handle = _u32.GetClipboardData(_CF_UNICODETEXT)
        if not handle:
            return None
        ptr = _k32.GlobalLock(handle)
        if not ptr:
            return None
        try:
            return ctypes.wstring_at(ptr)
        finally:
            _k32.GlobalUnlock(handle)
    except Exception as e:
        log.warning("读取剪贴板失败：%s", e)
        return None
    finally:
        _u32.CloseClipboard()


def write_clipboard(text: str) -> None:
    if not _open_clipboard():
        return
    try:
        _u32.EmptyClipboard()
        buf = ctypes.create_unicode_buffer(text)
        size = ctypes.sizeof(buf)
        handle = _k32.GlobalAlloc(_GMEM_MOVEABLE, size)
        if not handle:
            return
        ptr = _k32.GlobalLock(handle)
        ctypes.memmove(ptr, buf, size)
        _k32.GlobalUnlock(handle)
        # SetClipboardData 成功后内存归系统所有，不能再 GlobalFree
        _u32.SetClipboardData(_CF_UNICODETEXT, handle)
    except Exception as e:
        log.warning("写入剪贴板失败：%s", e)
    finally:
        _u32.CloseClipboard()


def clear_clipboard() -> None:
    if not _open_clipboard():
        return
    try:
        _u32.EmptyClipboard()
    finally:
        _u32.CloseClipboard()


# ---- 按键注入（Ctrl+C / Ctrl+V）----

_kb = Controller()


def send_copy() -> None:
    with _kb.pressed(Key.ctrl):
        _kb.press("c")
        _kb.release("c")


def send_paste() -> None:
    with _kb.pressed(Key.ctrl):
        _kb.press("v")
        _kb.release("v")


# ---- 提示音 ----

_ALIASES = {
    "start": "SystemAsterisk",
    "stop": "SystemDefault",
    "cancel": "SystemHand",
}


def play_sound(event: str) -> None:
    alias = _ALIASES.get(event)
    if not alias:
        return
    try:
        import winsound
        winsound.PlaySound(alias, winsound.SND_ALIAS | winsound.SND_ASYNC)
    except Exception as e:
        log.debug("播放提示音失败：%s", e)


# ---- 前台应用 ----

def frontmost_app():
    """返回 (应用名, 进程 exe 名)；app_styles 的键在 Windows 上写 exe 名即可，
    如 {"WeChat": "轻度纠错", "Code": "关闭"}。失败返回空串。"""
    try:
        hwnd = _u32.GetForegroundWindow()
        if not hwnd:
            return "", ""
        pid = wintypes.DWORD()
        _u32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
        exe = ""
        PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
        handle = _k32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid.value)
        if handle:
            try:
                size = wintypes.DWORD(1024)
                buf = ctypes.create_unicode_buffer(size.value)
                if _k32.QueryFullProcessImageNameW(handle, 0, buf, ctypes.byref(size)):
                    exe = os.path.splitext(os.path.basename(buf.value))[0]
            finally:
                _k32.CloseHandle(handle)
        return exe, exe
    except Exception as e:
        log.debug("获取前台应用失败：%s", e)
        return "", ""


# ---- 权限 ----

def check_permissions(prompt: bool = False) -> dict:
    """Windows 无 TCC：热键与粘贴无需授权。麦克风若被系统隐私设置拦截，
    录音会得到全零数据，在日志里给出指引即可。"""
    log.info("权限自检：Windows 平台无需输入监听/辅助功能授权；"
             "若录不到声音，请检查 设置 → 隐私和安全性 → 麦克风 → 允许桌面应用访问")
    return {"input_monitoring": True, "accessibility": True}


# ---- 悬浮窗 ----

_WIDTH = 560
_HEIGHT = 72
_MARGIN_BOTTOM = 96


class HUDWindow:
    """tkinter 无边框置顶悬浮窗，两行文本；独立线程跑 mainloop，
    show_lines/hide 可从任意线程调用（经队列转发）。"""

    def __init__(self):
        self._queue: queue.Queue = queue.Queue()
        self._started = False
        self._lock = threading.Lock()

    def _ensure_thread(self) -> None:
        with self._lock:
            if self._started:
                return
            self._started = True
            threading.Thread(target=self._run, daemon=True).start()

    def _run(self) -> None:
        try:
            import tkinter as tk
        except Exception as e:
            log.warning("tkinter 不可用，悬浮窗禁用：%s", e)
            return
        root = tk.Tk()
        root.withdraw()
        root.overrideredirect(True)
        root.attributes("-topmost", True)
        try:
            root.attributes("-alpha", 0.93)
        except Exception:
            pass
        bg = "#141414"
        root.configure(bg=bg)
        sw, sh = root.winfo_screenwidth(), root.winfo_screenheight()
        x = (sw - _WIDTH) // 2
        y = sh - _HEIGHT - _MARGIN_BOTTOM
        root.geometry(f"{_WIDTH}x{_HEIGHT}+{x}+{y}")
        main = tk.Label(root, fg="white", bg=bg, font=("Microsoft YaHei UI", 12),
                        anchor="w")
        main.place(x=18, y=10, width=_WIDTH - 36, height=26)
        hint = tk.Label(root, fg="#8f8f8f", bg=bg, font=("Microsoft YaHei UI", 9),
                        anchor="w")
        hint.place(x=18, y=42, width=_WIDTH - 36, height=18)

        visible = {"v": False}

        def poll():
            try:
                while True:
                    msg = self._queue.get_nowait()
                    if msg[0] == "show":
                        main.config(text=msg[1])
                        hint.config(text=msg[2])
                        if not visible["v"]:
                            root.deiconify()
                            root.attributes("-topmost", True)
                            visible["v"] = True
                    elif msg[0] == "hide":
                        if visible["v"]:
                            root.withdraw()
                            visible["v"] = False
            except queue.Empty:
                pass
            root.after(40, poll)

        root.after(40, poll)
        root.mainloop()

    def show_lines(self, main_text: str, hint_text: str = "") -> None:
        self._ensure_thread()
        self._queue.put(("show", main_text, hint_text))

    def hide(self) -> None:
        if not self._started:
            return
        self._queue.put(("hide",))
