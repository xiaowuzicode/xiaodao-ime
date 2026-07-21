"""实时预览悬浮窗（HUD）。

录音期间在屏幕下方居中浮一个不抢焦点的深色圆角小窗，实时显示：
    🎙️ 已录秒数 ｜ 识别中的文本（尾部截断）
转写/润色阶段显示状态文案，粘贴完成后收起。

线程模型：show/update/hide 可从任意线程调用，内部经 AppHelper.callAfter
调度到 AppKit 主线程（rumps 的事件循环就是 PyObjCTools.AppHelper 跑的）。
"""
from AppKit import (
    NSBackingStoreBuffered,
    NSColor,
    NSFont,
    NSMakeRect,
    NSPanel,
    NSScreen,
    NSStatusWindowLevel,
    NSTextField,
    NSWindowCollectionBehaviorCanJoinAllSpaces,
    NSWindowStyleMaskBorderless,
    NSWindowStyleMaskNonactivatingPanel,
)
from PyObjCTools import AppHelper

from xiaodao_ime.logger import get_logger

log = get_logger(__name__)

_WIDTH = 520
_HEIGHT = 64
_MARGIN_BOTTOM = 96
_MAX_CHARS = 60  # 只显示识别文本的尾部这么多字


class PreviewHUD:
    def __init__(self):
        self._panel = None
        self._label = None
        self._visible = False

    # ---- 主线程内部实现 ----

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

        label = NSTextField.alloc().initWithFrame_(
            NSMakeRect(18, 12, _WIDTH - 36, _HEIGHT - 24)
        )
        label.setEditable_(False)
        label.setBordered_(False)
        label.setBezeled_(False)
        label.setDrawsBackground_(False)
        label.setTextColor_(NSColor.whiteColor())
        label.setFont_(NSFont.systemFontOfSize_(15))
        label.setLineBreakMode_(0)  # NSLineBreakByWordWrapping
        label.setMaximumNumberOfLines_(2)
        content.addSubview_(label)

        self._panel = panel
        self._label = label

    def _do_update(self, text: str) -> None:
        try:
            self._ensure_panel()
            self._label.setStringValue_(text)
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

    # ---- 任意线程可调用的公开接口 ----

    def update(self, text: str) -> None:
        AppHelper.callAfter(self._do_update, text)

    def update_partial(self, elapsed: int, partial: str, prefix: str = "🎙️") -> None:
        tail = partial[-_MAX_CHARS:] if partial else "（聆听中…）"
        self.update(f"{prefix} {elapsed}s ｜ {tail}")

    def hide(self) -> None:
        AppHelper.callAfter(self._do_hide)
