#!/usr/bin/env python3
"""平台无关层离线测试：粘贴/选区流程 + HUD 组合逻辑，全部用假后端，不碰真剪贴板。"""
import os
import sys
import time

_ROOT = os.path.dirname(os.path.abspath(__file__))
if _ROOT not in sys.path:
    sys.path.insert(0, _ROOT)

from xiaodao_ime import platform as _platform  # noqa: E402
from xiaodao_ime import paster  # noqa: E402
from xiaodao_ime.hud import PreviewHUD  # noqa: E402


class FakeBackend:
    """假剪贴板/按键后端。copy_effect 控制模拟 Cmd+C 后剪贴板变成什么。"""

    def __init__(self, initial=None, copy_effect=None):
        self.clipboard = initial
        self.copy_effect = copy_effect  # None = 复制不改变剪贴板（无选区）
        self.pastes = 0

    def read_clipboard(self):
        return self.clipboard

    def write_clipboard(self, text):
        self.clipboard = text

    def clear_clipboard(self):
        self.clipboard = None

    def send_copy(self):
        if self.copy_effect is not None:
            self.clipboard = self.copy_effect

    def send_paste(self):
        self.pastes += 1


class FakeWindow:
    def __init__(self):
        self.lines = None
        self.hidden = False

    def show_lines(self, main, hint=""):
        self.lines = (main, hint)
        self.hidden = False

    def hide(self):
        self.hidden = True


def _with_backend(backend):
    old = _platform.backend
    _platform.backend = backend
    return old


def test_grab_selection_none():
    old = _with_backend(FakeBackend(initial="用户原有内容"))
    try:
        selection, original = paster.grab_selection()
        assert selection is None            # 剪贴板仍是哨兵 => 没有选区
        assert original == "用户原有内容"    # 原剪贴板被完整带回
    finally:
        _platform.backend = old
    print("PASS: 无选区时哨兵检测")


def test_grab_selection_found():
    old = _with_backend(FakeBackend(initial="旧内容", copy_effect="选中的文字"))
    try:
        selection, original = paster.grab_selection()
        assert selection == "选中的文字"
        assert original == "旧内容"
    finally:
        _platform.backend = old
    print("PASS: 有选区时正确抓取")


def test_paste_restores_clipboard():
    fake = FakeBackend(initial="老板的转账账号")
    old = _with_backend(fake)
    try:
        assert paster.paste_text("转写结果", restore_delay=0.05)
        assert fake.pastes == 1
        assert fake.clipboard == "转写结果"   # 粘贴瞬间剪贴板是转写文本
        time.sleep(0.3)
        assert fake.clipboard == "老板的转账账号"  # 延迟后恢复原内容
        # 空文本不粘贴
        assert not paster.paste_text("   ")
        assert fake.pastes == 1
    finally:
        _platform.backend = old
    print("PASS: 粘贴后延迟恢复原剪贴板")


def test_hud_compose():
    win = FakeWindow()
    hud = PreviewHUD(window=win)
    hud.begin(prefix="🎙️", placeholder="聆听中…", hint="再按热键出字 · 按 Esc 取消")
    main, hint = win.lines
    assert main.startswith("🎙️ 0s ")
    assert "聆听中…" in main
    assert "Esc" in hint
    # 声浪：高电平应出现高块字符
    for _ in range(10):
        hud.set_level(1.0)
    main, _ = win.lines
    assert "█" in main
    # 识别文本进来替换占位；长文本只留尾部
    hud.set_partial(12, "这是一段识别出来的文本")
    main, _ = win.lines
    assert "12s" in main and "识别出来的文本" in main
    hud.set_partial(13, "长" * 100)
    main, _ = win.lines
    assert "…" in main and len(main) < 120
    # 状态阶段：录音态刷新全部失效，副行显示全文尾部
    hud.set_status("🪄 润色中…", "已转写的全文内容")
    main, hint = win.lines
    assert main == "🪄 润色中…" and "全文内容" in hint
    hud.set_level(0.5)
    assert win.lines[0] == "🪄 润色中…"  # set_status 后声浪不再打扰
    hud.hide()
    assert win.hidden
    print("PASS: HUD 组合渲染（声浪/尾部截断/状态两行）")


if __name__ == "__main__":
    test_grab_selection_none()
    test_grab_selection_found()
    test_paste_restores_clipboard()
    test_hud_compose()
    print("\n平台无关层测试全部通过 ✅")
