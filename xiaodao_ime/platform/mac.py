"""macOS 后端：AppKit 剪贴板 / Quartz 按键注入与权限 / NSPanel 悬浮窗 / afplay 提示音。

接口约定见 xiaodao_ime/platform/__init__.py。
"""
import os
import subprocess

import Quartz
from AppKit import (
    NSBackingStoreBuffered,
    NSColor,
    NSFont,
    NSMakeRect,
    NSPanel,
    NSPasteboard,
    NSPasteboardTypeString,
    NSScreen,
    NSStatusWindowLevel,
    NSTextField,
    NSWindowCollectionBehaviorCanJoinAllSpaces,
    NSWindowStyleMaskBorderless,
    NSWindowStyleMaskNonactivatingPanel,
    NSWorkspace,
)
from PyObjCTools import AppHelper

from xiaodao_ime.logger import get_logger

log = get_logger(__name__)

_V_KEYCODE = 9  # macOS 虚拟键码：字母 V
_C_KEYCODE = 8  # macOS 虚拟键码：字母 C


# ---- 剪贴板 ----

def read_clipboard():
    pb = NSPasteboard.generalPasteboard()
    return pb.stringForType_(NSPasteboardTypeString)


def write_clipboard(text: str) -> None:
    pb = NSPasteboard.generalPasteboard()
    pb.clearContents()
    pb.setString_forType_(text, NSPasteboardTypeString)


def clear_clipboard() -> None:
    NSPasteboard.generalPasteboard().clearContents()


# ---- 按键注入（Cmd+C / Cmd+V）----

def _send_cmd_key(keycode: int) -> None:
    src = Quartz.CGEventSourceCreate(Quartz.kCGEventSourceStateHIDSystemState)
    key_down = Quartz.CGEventCreateKeyboardEvent(src, keycode, True)
    Quartz.CGEventSetFlags(key_down, Quartz.kCGEventFlagMaskCommand)
    key_up = Quartz.CGEventCreateKeyboardEvent(src, keycode, False)
    Quartz.CGEventSetFlags(key_up, Quartz.kCGEventFlagMaskCommand)
    Quartz.CGEventPost(Quartz.kCGHIDEventTap, key_down)
    Quartz.CGEventPost(Quartz.kCGHIDEventTap, key_up)


def send_copy() -> None:
    _send_cmd_key(_C_KEYCODE)


def send_paste() -> None:
    _send_cmd_key(_V_KEYCODE)


# ---- 提示音 ----

_SOUNDS = {
    "start": "/System/Library/Sounds/Tink.aiff",
    "stop": "/System/Library/Sounds/Pop.aiff",
    "cancel": "/System/Library/Sounds/Bottle.aiff",
}


def play_sound(event: str) -> None:
    path = _SOUNDS.get(event)
    if not path or not os.path.exists(path):
        return
    try:
        subprocess.Popen(
            ["afplay", path],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
    except Exception as e:
        log.debug("播放提示音失败：%s", e)


# ---- 前台应用 ----

def frontmost_app():
    """返回 (应用名, bundle id)；失败返回空串。"""
    try:
        app = NSWorkspace.sharedWorkspace().frontmostApplication()
        if app is None:
            return "", ""
        return str(app.localizedName() or ""), str(app.bundleIdentifier() or "")
    except Exception as e:
        log.debug("获取前台应用失败：%s", e)
        return "", ""


# ---- 权限（TCC）----
# macOS 的权限授予对象是「进程的签名身份」而非路径：
#   - 终端启动 -> 权限挂在终端 App 上；
#   - .app 启动 -> 挂在 App bundle 上；ad-hoc 签名每次重打包都会变，旧授权
#     静默失效（设置里开关看着还开着）——必须先移除旧条目再重新勾选。
# Preflight 只查不弹窗；Request 触发系统弹窗并把当前宿主加进权限列表。

def check_permissions(prompt: bool = False) -> dict:
    try:
        listen = bool(Quartz.CGPreflightListenEventAccess())   # 输入监听（全局热键）
        post = bool(Quartz.CGPreflightPostEventAccess())       # 辅助功能（模拟 Cmd+V）
    except Exception as e:
        log.warning("权限自检不可用：%s", e)
        return {"input_monitoring": True, "accessibility": True}  # 查不了就不拦

    if prompt:
        try:
            if not listen:
                Quartz.CGRequestListenEventAccess()
            if not post:
                Quartz.CGRequestPostEventAccess()
        except Exception as e:
            log.debug("触发权限弹窗失败：%s", e)

    log.info("权限自检：输入监听=%s，辅助功能=%s",
             "✅" if listen else "❌ 未授权", "✅" if post else "❌ 未授权")
    if not listen:
        log.warning("【热键无响应的原因】输入监听未授权：系统设置 → 隐私与安全性 → 输入监听，"
                    "勾选本程序的宿主（.app 启动就是「小岛AI输入法」，终端启动就是终端）。"
                    "若列表里已有旧条目仍不生效：先用「-」移除，再重新添加勾选（重新打包后签名已变）。"
                    "改完必须重启本程序。")
    if not post:
        log.warning("【出不了字的原因】辅助功能未授权：系统设置 → 隐私与安全性 → 辅助功能，"
                    "同上勾选宿主并重启。")
    return {"input_monitoring": listen, "accessibility": post}


# ---- 悬浮窗 ----

_WIDTH = 560
_HEIGHT = 72
_MARGIN_BOTTOM = 96


class HUDWindow:
    """NSPanel 两行悬浮窗：主行（识别文本/状态）+ 提示行（操作提示）。

    show_lines/hide 可从任意线程调用，内部经 AppHelper.callAfter 调度到
    AppKit 主线程（rumps 的事件循环就是 PyObjCTools.AppHelper 跑的）。
    """

    def __init__(self):
        self._panel = None
        self._main = None
        self._hint = None
        self._visible = False

    def _ensure_panel(self) -> None:
        if self._panel is not None:
            return
        screen = NSScreen.mainScreen()
        frame = screen.visibleFrame() if screen else NSMakeRect(0, 0, 1440, 900)
        x = frame.origin.x + (frame.size.width - _WIDTH) / 2
        y = frame.origin.y + _MARGIN_BOTTOM
        panel = NSPanel.alloc().initWithContentRect_styleMask_backing_defer_(
            NSMakeRect(x, y, _WIDTH, _HEIGHT),
            NSWindowStyleMaskBorderless | NSWindowStyleMaskNonactivatingPanel,
            NSBackingStoreBuffered,
            False,
        )
        panel.setLevel_(NSStatusWindowLevel)
        panel.setOpaque_(False)
        panel.setBackgroundColor_(NSColor.clearColor())
        panel.setIgnoresMouseEvents_(True)  # 点击穿透，不打断用户操作
        panel.setCollectionBehavior_(NSWindowCollectionBehaviorCanJoinAllSpaces)
        panel.setHidesOnDeactivate_(False)

        content = panel.contentView()
        content.setWantsLayer_(True)
        layer = content.layer()
        layer.setCornerRadius_(14.0)
        layer.setBackgroundColor_(
            NSColor.colorWithCalibratedWhite_alpha_(0.08, 0.92).CGColor()
        )

        def _label(y: float, h: float, size: float, color):
            field = NSTextField.alloc().initWithFrame_(
                NSMakeRect(18, y, _WIDTH - 36, h)
            )
            field.setEditable_(False)
            field.setBordered_(False)
            field.setBezeled_(False)
            field.setDrawsBackground_(False)
            field.setTextColor_(color)
            field.setFont_(NSFont.systemFontOfSize_(size))
            field.setMaximumNumberOfLines_(1)
            content.addSubview_(field)
            return field

        main = _label(32, 24, 15, NSColor.whiteColor())
        hint = _label(10, 16, 11,
                      NSColor.colorWithCalibratedWhite_alpha_(1.0, 0.5))

        self._panel = panel
        self._main = main
        self._hint = hint

    def _do_show(self, main_text: str, hint_text: str) -> None:
        try:
            self._ensure_panel()
            self._main.setStringValue_(main_text)
            self._hint.setStringValue_(hint_text)
            if not self._visible:
                self._panel.orderFrontRegardless()
                self._visible = True
        except Exception as e:
            log.debug("HUD 更新失败：%s", e)

    def _do_hide(self) -> None:
        try:
            if self._panel is not None and self._visible:
                self._panel.orderOut_(None)
            self._visible = False
        except Exception as e:
            log.debug("HUD 隐藏失败：%s", e)

    def show_lines(self, main_text: str, hint_text: str = "") -> None:
        AppHelper.callAfter(self._do_show, main_text, hint_text)

    def hide(self) -> None:
        AppHelper.callAfter(self._do_hide)
