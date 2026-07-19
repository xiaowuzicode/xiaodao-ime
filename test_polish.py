#!/usr/bin/env python3
"""润色与设置模块的离线测试（不联网、不依赖热键/麦克风）。"""
import json
import os
import sys
import tempfile

_ROOT = os.path.dirname(os.path.abspath(__file__))
if _ROOT not in sys.path:
    sys.path.insert(0, _ROOT)

from xiaodao_ime.polisher import (  # noqa: E402
    BUILTIN_STYLES,
    Polisher,
    apply_replacements,
    build_system_prompt,
    get_styles,
)
from xiaodao_ime.settings import DEFAULTS, Settings  # noqa: E402


def test_apply_replacements():
    assert apply_replacements("欧朵的达人匹配", {"欧朵": "Ordo"}) == "Ordo的达人匹配"
    assert apply_replacements("无替换", {}) == "无替换"
    assert apply_replacements("无替换", None) == "无替换"
    print("PASS: apply_replacements")


def test_build_system_prompt():
    assert "小岛AI" in build_system_prompt(["小岛AI", "Ordo"])
    assert "专有名词" not in build_system_prompt([])
    print("PASS: build_system_prompt")


def test_settings_merge_and_save():
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "settings.json")
        # 用户只写部分键，其余回退默认
        with open(path, "w", encoding="utf-8") as f:
            json.dump({"hotkey": "f19", "polish": {"enabled": True}}, f)
        s = Settings(path)
        assert s.data["hotkey"] == "f19"
        assert s.data["polish"]["enabled"] is True
        assert s.data["polish"]["provider"] == DEFAULTS["polish"]["provider"]
        s.data["hotkey"] = "alt_r"
        s.save()
        assert Settings(path).data["hotkey"] == "alt_r"
    print("PASS: settings merge/save")


def test_polisher_fail_open():
    with tempfile.TemporaryDirectory() as d:
        s = Settings(os.path.join(d, "settings.json"))
        p = Polisher(s)
        # 未启用 -> None
        assert p.polish("测试文本") is None
        # 启用但 provider 未知 -> None（fail-open）
        s.data["polish"]["enabled"] = True
        s.data["polish"]["provider"] = "nope"
        assert p.polish("测试文本") is None
        # openai 但 base_url 为空 -> None（fail-open），且 configured 为 False
        s.data["polish"]["provider"] = "openai"
        s.data["polish"]["base_url"] = ""
        assert p.configured is False
        assert p.polish("测试文本") is None
        # 空文本 -> None
        s.data["polish"]["base_url"] = "https://example.invalid"
        assert p.polish("   ") is None
    print("PASS: polisher fail-open")


def test_styles():
    with tempfile.TemporaryDirectory() as d:
        s = Settings(os.path.join(d, "settings.json"))
        styles = get_styles(s)
        for name in ("润色", "书面化", "轻度纠错", "翻译成英文", "会议纪要"):
            assert name in styles
        # 自定义风格：新增 + 同名覆盖内置
        s.data["polish"]["styles"] = {"文言文": "翻译成文言文", "润色": "自定义润色规则"}
        styles = get_styles(s)
        assert styles["文言文"] == "翻译成文言文"
        assert styles["润色"] == "自定义润色规则"
        assert "英文" in build_system_prompt([], BUILTIN_STYLES["翻译成英文"])
    print("PASS: styles")


def test_history():
    import xiaodao_ime.history as history_mod
    with tempfile.TemporaryDirectory() as d:
        s = Settings(os.path.join(d, "settings.json"))
        h = history_mod.History(s, path=os.path.join(d, "history.jsonl"))
        v0 = h.version
        h.append("原始 文本", "润色后的文本")
        assert h.version == v0 + 1
        assert h.recent(5)[0]["final"] == "润色后的文本"
        # 重新加载能读回
        h2 = history_mod.History(s, path=os.path.join(d, "history.jsonl"))
        assert h2.recent(5)[0]["final"] == "润色后的文本"
        # 空文本不入库
        h.append("x", "   ")
        assert len(h.recent(10)) == 1
    print("PASS: history")


if __name__ == "__main__":
    test_apply_replacements()
    test_build_system_prompt()
    test_settings_merge_and_save()
    test_polisher_fail_open()
    test_styles()
    test_history()
    print("\n全部离线测试通过 ✅")
