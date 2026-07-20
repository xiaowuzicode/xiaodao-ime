#!/usr/bin/env python3
"""热键状态机离线测试：用假 Recorder 模拟按键序列，不依赖真实键盘/麦克风。"""
import os
import sys
import tempfile
import time

_ROOT = os.path.dirname(os.path.abspath(__file__))
if _ROOT not in sys.path:
    sys.path.insert(0, _ROOT)

from pynput import keyboard  # noqa: E402

from xiaodao_ime.hotkey import HOTKEY_CHOICES, HotkeyController  # noqa: E402
from xiaodao_ime.settings import Settings  # noqa: E402

ALT = keyboard.Key.alt        # macOS 上左 Option 的实际上报值
OTHER = keyboard.KeyCode.from_char("c")


class FakeRecorder:
    def __init__(self):
        self.recording = False
        self.dur = 1.0  # stop/duration 返回的时长

    def start(self):
        self.recording = True

    def abort(self):
        self.recording = False
        return self.dur

    def stop(self):
        self.recording = False
        import numpy as np
        return np.zeros(160, dtype="float32"), self.dur

    def duration(self):
        return self.dur


class FakeTranscriber:
    def transcribe(self, pcm):
        return "", 0.0  # 空结果 => worker 不粘贴


def make_controller(tmp, mode):
    settings = Settings(os.path.join(tmp, "settings.json"))
    settings.data["sounds"] = False  # 测试静音
    rec = FakeRecorder()
    ctl = HotkeyController(rec, FakeTranscriber(), settings=settings, mode=mode)
    return ctl, rec


def test_macos_alt_matches():
    # macOS 左 Option 上报 Key.alt，必须能命中 alt_l 配置
    assert ALT in HOTKEY_CHOICES["alt_l"][1]
    assert keyboard.Key.alt_r not in HOTKEY_CHOICES["alt_l"][1]
    print("PASS: macOS Key.alt 兼容")


def test_toggle_basic():
    with tempfile.TemporaryDirectory() as d:
        ctl, rec = make_controller(d, "toggle")
        # 单击（按下+松开）=> 开始录音
        ctl._on_press(ALT)
        ctl._on_release(ALT)
        assert rec.recording and ctl._recording
        # 再击 => 结束并处理（按下即停止，松开被忽略）
        ctl._on_press(ALT)
        assert not rec.recording and not ctl._recording
        ctl._on_release(ALT)
        assert not ctl._recording
    print("PASS: toggle 单击开始/再击结束")


def test_toggle_combo_no_false_start():
    with tempfile.TemporaryDirectory() as d:
        ctl, rec = make_controller(d, "toggle")
        # ⌥+C 组合键：按住 Option 时按下其他键，松开 Option 不应开始录音
        ctl._on_press(ALT)
        ctl._on_press(OTHER)
        ctl._on_release(ALT)
        assert not rec.recording and not ctl._recording
    print("PASS: toggle 组合键不误触发")


def test_toggle_other_key_cancels():
    with tempfile.TemporaryDirectory() as d:
        ctl, rec = make_controller(d, "toggle")
        ctl._on_press(ALT)
        ctl._on_release(ALT)
        assert rec.recording
        # 录音中开始打字 => 取消
        ctl._on_press(OTHER)
        assert not rec.recording and not ctl._recording
        # 取消后还能正常开启下一次
        ctl._on_press(ALT)
        ctl._on_release(ALT)
        assert rec.recording
    print("PASS: toggle 录音中按其他键取消，且可恢复")


def test_hold_basic_and_lock():
    with tempfile.TemporaryDirectory() as d:
        ctl, rec = make_controller(d, "hold")
        # 按住 >= min_hold 后松开 => 处理
        ctl._on_press(ALT)
        assert rec.recording
        rec.dur = 1.0
        ctl._on_release(ALT)
        assert not rec.recording and not ctl._recording
        # 双击（两次短按）=> 锁定录音；再按一下 => 结束
        rec.dur = 0.1
        ctl._on_press(ALT)
        ctl._on_release(ALT)          # 第一次短按：丢弃并记时间
        assert not rec.recording
        ctl._on_press(ALT)
        ctl._on_release(ALT)          # 第二次短按：进入锁定
        assert rec.recording and ctl._locked
        rec.dur = 2.0
        ctl._on_press(ALT)            # 第三次按下：结束
        assert not rec.recording and not ctl._locked
        ctl._on_release(ALT)
    print("PASS: hold 按住说话 + 双击锁定")


def test_hold_combo_cancels():
    with tempfile.TemporaryDirectory() as d:
        ctl, rec = make_controller(d, "hold")
        ctl._on_press(ALT)
        assert rec.recording
        ctl._on_press(OTHER)          # ⌥+C
        assert not rec.recording
        ctl._on_release(ALT)
        assert not ctl._recording and not ctl._cancelled
    print("PASS: hold 组合键取消")


if __name__ == "__main__":
    test_macos_alt_matches()
    test_toggle_basic()
    test_toggle_combo_no_false_start()
    test_toggle_other_key_cancels()
    test_hold_basic_and_lock()
    test_hold_combo_cancels()
    time.sleep(0.2)  # 等 worker 线程退出
    print("\n热键状态机测试全部通过 ✅")
