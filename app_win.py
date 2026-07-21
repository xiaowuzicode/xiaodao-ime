#!/usr/bin/env python3
"""小岛AI输入法 —— Windows 托盘入口（pystray）。

与 macOS 入口 app.py 功能对等：托盘图标按状态换色，右键菜单提供
设置/历史/统计/暂停热键；核心层（热键状态机、录音、转写、润色、HUD
组合逻辑）与 macOS 完全共享。
"""
import os
import subprocess
import sys
import threading
import time

_ROOT = os.path.dirname(os.path.abspath(__file__))
if _ROOT not in sys.path:
    sys.path.insert(0, _ROOT)

import pystray  # noqa: E402
from PIL import Image, ImageDraw  # noqa: E402
from pystray import Menu, MenuItem as Item  # noqa: E402

from xiaodao_ime.config import (  # noqa: E402
    LOG_FILE,
    MODEL_FILENAME,
    MODEL_PATH,
    MODEL_REPO,
    MODELS_DIR,
)
from xiaodao_ime.history import History  # noqa: E402
from xiaodao_ime.hotkey import HOTKEY_CHOICES, RECORD_MODES, HotkeyController  # noqa: E402
from xiaodao_ime.hud import PreviewHUD  # noqa: E402
from xiaodao_ime.logger import get_logger  # noqa: E402
from xiaodao_ime.paster import copy_to_clipboard  # noqa: E402
from xiaodao_ime.permissions import check_permissions  # noqa: E402
from xiaodao_ime.platform import DEFAULT_HOTKEY, DEFAULT_REWRITE_HOTKEY  # noqa: E402
from xiaodao_ime.polisher import Polisher, get_styles  # noqa: E402
from xiaodao_ime.recorder import Recorder  # noqa: E402
from xiaodao_ime.settings import Settings  # noqa: E402
from xiaodao_ime.transcriber import Transcriber  # noqa: E402

log = get_logger("xiaodao_ime.app_win")

_STATE_COLORS = {
    "idle": "#2AA198",          # 岛屿青
    "recording": "#E5484D",     # 红
    "transcribing": "#E8A33D",  # 琥珀
    "polishing": "#3B82F6",     # 蓝
    "paused": "#777777",        # 灰
}
_STATE_LABELS = {
    "idle": "状态：待机",
    "recording": "状态：录音中",
    "transcribing": "状态：转写中",
    "polishing": "状态：润色中",
    "paused": "状态：已暂停",
}


def _make_image(color: str) -> Image.Image:
    img = Image.new("RGBA", (64, 64), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    draw.ellipse((8, 8, 56, 56), fill=color)
    return img


class WinApp:
    def __init__(self):
        self._settings = Settings()
        self._polisher = Polisher(self._settings)
        self._history = History(self._settings)
        self._recorder = Recorder()
        self._transcriber = Transcriber()
        self._hud = PreviewHUD()
        self._hotkey = None
        self._state = "idle"
        self._boot_msg = ""
        self._rec_started = 0.0
        self._images = {k: _make_image(v) for k, v in _STATE_COLORS.items()}
        self._icon = pystray.Icon(
            "xiaodao-ime", self._images["idle"], "小岛AI输入法", menu=self._build_menu()
        )

    # ---- 菜单 ----

    def _build_menu(self) -> Menu:
        s = self._settings

        def status_text(_):
            return self._boot_msg or _STATE_LABELS.get(self._state, "状态：待机")

        def hotkey_items(field, other_field, setter):
            def make(name, label):
                def act(icon, item):
                    if name == s.data.get(other_field):
                        self._notify("热键冲突", "听写热键和改写热键不能相同")
                        return
                    s.data[field] = name
                    s.save()
                    if self._hotkey:
                        setter(name)
                return Item(label, act, radio=True,
                            checked=lambda item, n=name: s.data.get(field) == n)
            return [make(name, label) for name, (label, _) in HOTKEY_CHOICES.items()]

        def mode_items():
            def make(name, label):
                def act(icon, item):
                    s.data["record_mode"] = name
                    s.save()
                    if self._hotkey:
                        self._hotkey.set_mode(name)
                return Item(label, act, radio=True,
                            checked=lambda item, n=name: s.data.get("record_mode") == n)
            return [make(name, label) for name, label in RECORD_MODES.items()]

        def style_items():
            current = lambda: s.data.get("polish", {}).get("style", "润色")  # noqa: E731

            def make(name):
                def act(icon, item):
                    s.data.setdefault("polish", {})["style"] = name
                    s.save()
                return Item(name, act, radio=True,
                            checked=lambda item, n=name: current() == n)
            return [make(name) for name in get_styles(s)]

        def polish_text(_):
            polish = s.data.get("polish", {})
            return f"AI 润色（{polish.get('provider', 'openai')} / {polish.get('model') or '?'}）"

        def history_items():
            entries = self._history.recent(10)
            if not entries:
                return [Item("（暂无记录）", None, enabled=False)]
            items = []
            for entry in entries:
                text = entry.get("final", "")
                label = text if len(text) <= 24 else text[:24] + "…"
                items.append(Item(label, self._make_copy_cb(text)))
            return items

        def stats_text(_):
            chars = self._history.total_chars
            return (f"统计：{self._history.total_count} 段 · {chars} 字 · "
                    f"约省 {int(chars / 90)} 分钟")

        settings_menu = Menu(
            Item("听写热键", Menu(lambda: hotkey_items(
                "hotkey", "rewrite_hotkey",
                lambda n: self._hotkey.set_trigger(n)))),
            Item("改写热键", Menu(lambda: hotkey_items(
                "rewrite_hotkey", "hotkey",
                lambda n: self._hotkey.set_rewrite_trigger(n)))),
            Item("录音方式", Menu(lambda: mode_items())),
            Item("实时预览悬浮窗", self._toggle_preview,
                 checked=lambda item: bool(s.data.get("live_preview", True))),
            Item(polish_text, self._toggle_polish,
                 checked=lambda item: bool(s.data.get("polish", {}).get("enabled"))),
            Item("润色风格", Menu(lambda: style_items())),
            Item("打开配置文件", self._open_settings),
            Item("重新加载配置", self._reload_settings),
        )
        return Menu(
            Item(status_text, None, enabled=False),
            Item("暂停热键", self._toggle_pause,
                 checked=lambda item: bool(self._hotkey and self._hotkey.paused)),
            Menu.SEPARATOR,
            Item("设置", settings_menu),
            Item("历史", Menu(lambda: history_items())),
            Item(stats_text, None, enabled=False),
            Item("打开日志", self._open_log),
            Item("退出", self._quit),
        )

    # ---- 菜单动作 ----

    def _make_copy_cb(self, text: str):
        def _cb(icon, item):
            if copy_to_clipboard(text):
                log.info("历史条目已复制（%d 字符）", len(text))
        return _cb

    def _toggle_pause(self, icon, item) -> None:
        if not self._hotkey:
            return
        paused = not self._hotkey.paused
        self._hotkey.set_paused(paused)
        self._apply_state("paused" if paused else "idle")

    def _toggle_preview(self, icon, item) -> None:
        current = bool(self._settings.data.get("live_preview", True))
        self._settings.data["live_preview"] = not current
        self._settings.save()

    def _toggle_polish(self, icon, item) -> None:
        polish = self._settings.data.setdefault("polish", {})
        if not polish.get("enabled") and not self._polisher.configured:
            self._notify("无法开启 AI 润色",
                         "请先点「设置 → 打开配置文件」，填写 polish 的 base_url / api_key / model")
            self._open_settings(icon, item)
            return
        polish["enabled"] = not polish.get("enabled", False)
        self._settings.save()

    def _reload_settings(self, icon, item) -> None:
        self._settings.load()
        if self._hotkey:
            self._hotkey.set_trigger(self._settings.data.get("hotkey", DEFAULT_HOTKEY))
            self._hotkey.set_rewrite_trigger(
                self._settings.data.get("rewrite_hotkey", DEFAULT_REWRITE_HOTKEY))
            self._hotkey.set_mode(self._settings.data.get("record_mode", "toggle"))
        log.info("配置已重新加载")

    def _open_settings(self, icon, item) -> None:
        try:
            os.startfile(self._settings.ensure_file())  # noqa: S606
        except Exception as e:
            log.warning("打开配置文件失败：%s", e)

    def _open_log(self, icon, item) -> None:
        try:
            os.startfile(LOG_FILE)  # noqa: S606
        except Exception as e:
            log.warning("打开日志失败：%s", e)

    def _quit(self, icon, item) -> None:
        log.info("用户点击退出")
        try:
            if self._hotkey:
                self._hotkey.stop()
            self._transcriber.close()
        finally:
            self._icon.stop()

    # ---- 状态与通知 ----

    def _notify(self, title: str, message: str) -> None:
        try:
            self._icon.notify(message, title)
        except Exception as e:
            log.debug("发送通知失败：%s", e)

    def _apply_state(self, state: str) -> None:
        self._state = state
        if state == "recording":
            self._rec_started = time.time()
        try:
            self._icon.icon = self._images.get(state, self._images["idle"])
            self._icon.title = f"小岛AI输入法 · {_STATE_LABELS.get(state, '待机')}"
        except Exception as e:
            log.debug("更新托盘状态失败：%s", e)

    def _set_status(self, state: str) -> None:
        """HotkeyController 子线程回调。"""
        if self._hotkey and self._hotkey.paused:
            return
        self._apply_state(state)

    def _tick_loop(self) -> None:
        while True:
            time.sleep(1)
            if self._state == "recording" and self._rec_started:
                secs = int(time.time() - self._rec_started)
                try:
                    self._icon.title = f"小岛AI输入法 · 录音中 {secs}s"
                except Exception:
                    pass

    # ---- 启动 ----

    def _boot(self) -> None:
        check_permissions(prompt=True)
        if os.path.isfile(MODEL_PATH):
            self._finish_boot()
        else:
            log.info("模型缺失，后台自动下载：%s / %s", MODEL_REPO, MODEL_FILENAME)
            self._boot_msg = "状态：正在下载模型（241MB，仅首次）…"
            self._notify("首次运行：正在下载语音模型（约 241MB）",
                         "完成后会通知你。国内网络慢可设 HF_ENDPOINT=https://hf-mirror.com")
            threading.Thread(target=self._download_model, daemon=True).start()

    def _download_model(self) -> None:
        try:
            from huggingface_hub import hf_hub_download
            hf_hub_download(MODEL_REPO, MODEL_FILENAME, local_dir=MODELS_DIR)
            log.info("模型下载完成：%s", MODEL_PATH)
            self._notify("模型下载完成", "语音输入已就绪 🏝️")
            self._finish_boot()
        except Exception as e:
            log.error("模型下载失败：%s", e)
            self._boot_msg = "状态：模型下载失败（见日志）"
            self._notify("模型下载失败",
                         "请检查网络。国内可设 HF_ENDPOINT=https://hf-mirror.com 后重启本程序")

    def _finish_boot(self) -> None:
        self._boot_msg = ""
        try:
            secs = self._transcriber.load()
            log.info("模型常驻就绪，加载耗时 %.2fs", secs)
        except Exception as e:
            log.error("模型加载失败：%s", e)
            self._boot_msg = "状态：模型加载失败（见日志）"
            return
        try:
            self._hotkey = HotkeyController(
                self._recorder, self._transcriber, on_status=self._set_status,
                polisher=self._polisher, settings=self._settings,
                history=self._history, hud=self._hud,
                notifier=self._notify,
                hotkey=self._settings.data.get("hotkey", DEFAULT_HOTKEY),
                rewrite_hotkey=self._settings.data.get(
                    "rewrite_hotkey", DEFAULT_REWRITE_HOTKEY),
                mode=self._settings.data.get("record_mode", "toggle"),
            )
            self._hotkey.start()
            label = HOTKEY_CHOICES.get(
                self._settings.data.get("hotkey", DEFAULT_HOTKEY),
                HOTKEY_CHOICES[DEFAULT_HOTKEY])[0]
            log.info("小岛AI输入法已就绪：热键「%s」", label)
        except Exception as e:
            log.error("热键监听启动失败：%s", e)

    def run(self) -> None:
        threading.Thread(target=self._tick_loop, daemon=True).start()

        def setup(icon):
            icon.visible = True
            self._boot()

        self._icon.run(setup=setup)


def main() -> None:
    log.info("=== 小岛AI输入法启动（Windows）===")
    WinApp().run()


if __name__ == "__main__":
    main()
