"""平台抽象层：按运行平台选择后端实现。

后端是一个模块，约定暴露统一接口：
  剪贴板   read_clipboard() / write_clipboard(text) / clear_clipboard()
  按键注入 send_copy() / send_paste()
  提示音   play_sound(event)            # event: start / stop / cancel
  前台应用 frontmost_app() -> (应用名, 应用标识)
  权限     check_permissions(prompt) -> {"input_monitoring": bool, "accessibility": bool}
  悬浮窗   HUDWindow 类：show_lines(main, hint) / hide()，线程安全

核心层（hotkey/recorder/transcriber/polisher/…）只依赖本包，不直接 import
AppKit/Quartz/ctypes 等平台库。
"""
import sys

IS_MAC = sys.platform == "darwin"
IS_WIN = sys.platform.startswith("win")

# 平台默认热键（settings.py 与 hotkey.py 共用；键名须存在于 hotkey.HOTKEY_CHOICES）
if IS_MAC:
    DEFAULT_HOTKEY = "alt_l"          # 左 Option
    DEFAULT_REWRITE_HOTKEY = "alt_r"  # 右 Option
else:
    # Windows：右 Alt 在欧洲键盘布局是 AltGr（负责输入 @€# 等符号），不能当默认；
    # 左 Alt 单击会聚焦窗口菜单栏。右 Ctrl / F8 是最不冲突的选择。
    DEFAULT_HOTKEY = "ctrl_r"         # 右 Ctrl
    DEFAULT_REWRITE_HOTKEY = "f8"

if IS_MAC:
    from xiaodao_ime.platform import mac as backend  # noqa: F401
elif IS_WIN:
    from xiaodao_ime.platform import win as backend  # noqa: F401
else:
    raise RuntimeError(
        "小岛AI输入法目前支持 macOS 与 Windows；Linux 支持在路线图中"
    )
