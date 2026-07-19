#!/usr/bin/env python3
"""小岛AI输入法 —— 菜单栏常驻入口。

按住热键（默认左 Option，可在设置中切换）说话，松手把转写文字粘贴进当前输入框。

rumps 必须运行在主线程；pynput 监听、录音回调、转写/润色均在子线程执行。
"""
import os
import subprocess
import sys

# 保证以「项目根目录」为导入根，使 `xiaodao_ime` 绝对导入可用
_ROOT = os.path.dirname(os.path.abspath(__file__))
if _ROOT not in sys.path:
    sys.path.insert(0, _ROOT)

import rumps  # noqa: E402

from xiaodao_ime.config import (  # noqa: E402
    ICON_IDLE,
    ICON_POLISHING,
    ICON_RECORDING,
    ICON_TRANSCRIBING,
    LOG_FILE,
    MODEL_PATH,
)
from xiaodao_ime.hotkey import HOTKEY_CHOICES, HotkeyController  # noqa: E402
from xiaodao_ime.logger import get_logger  # noqa: E402
from xiaodao_ime.polisher import Polisher  # noqa: E402
from xiaodao_ime.recorder import Recorder  # noqa: E402
from xiaodao_ime.settings import Settings  # noqa: E402
from xiaodao_ime.transcriber import Transcriber  # noqa: E402

log = get_logger("xiaodao_ime.app")

_STATUS_ICON = {
    "idle": ICON_IDLE,
    "recording": ICON_RECORDING,
    "transcribing": ICON_TRANSCRIBING,
    "polishing": ICON_POLISHING,
}
_STATUS_LABEL = {
    "idle": "状态：待机",
    "recording": "状态：录音中",
    "transcribing": "状态：转写中",
    "polishing": "状态：润色中",
}


class XiaodaoIME(rumps.App):
    def __init__(self):
        super().__init__(ICON_IDLE, quit_button=None)
        self._settings = Settings()
        self._polisher = Polisher(self._settings)

        self._status_item = rumps.MenuItem("状态：待机")  # 无 callback => 置灰显示
        self._hotkey_items = {
            name: rumps.MenuItem(label, callback=self._make_hotkey_cb(name))
            for name, (label, _) in HOTKEY_CHOICES.items()
        }
        hotkey_menu = rumps.MenuItem("热键")
        for item in self._hotkey_items.values():
            hotkey_menu.add(item)
        self._polish_item = rumps.MenuItem("AI 润色", callback=self.toggle_polish)
        settings_menu = rumps.MenuItem("设置")
        settings_menu.add(hotkey_menu)
        settings_menu.add(self._polish_item)
        settings_menu.add(rumps.MenuItem("打开配置文件", callback=self.open_settings))
        settings_menu.add(rumps.MenuItem("重新加载配置", callback=self.reload_settings))

        self.menu = [
            self._status_item,
            None,
            settings_menu,
            rumps.MenuItem("打开日志", callback=self.open_log),
            rumps.MenuItem("退出", callback=self.quit_app),
        ]
        self._sync_menu_state()

        self._recorder = Recorder()
        self._transcriber = Transcriber()
        self._hotkey = None
        self._boot()

    def _boot(self) -> None:
        """加载模型 + 启动热键监听；任一步失败都不让进程崩溃，给出明确指引。"""
        # 1. 加载常驻模型
        try:
            if not os.path.isfile(MODEL_PATH):
                msg = (f"模型文件缺失：{MODEL_PATH}\n"
                       "请先下载 SenseVoice GGUF 到 models/ 目录（见 README）。")
                log.error(msg)
                self._status_item.title = "状态：模型缺失"
                print("[启动错误] " + msg, file=sys.stderr)
            else:
                secs = self._transcriber.load()
                log.info("模型常驻就绪，加载耗时 %.2fs", secs)
        except Exception as e:
            log.error("模型加载失败：%s", e)
            self._status_item.title = "状态：模型加载失败"
            print(f"[启动错误] 模型加载失败：{e}", file=sys.stderr)

        # 2. 启动全局热键监听
        try:
            self._hotkey = HotkeyController(
                self._recorder, self._transcriber, on_status=self._set_status,
                polisher=self._polisher, settings=self._settings,
                hotkey=self._settings.data.get("hotkey", "alt_l"),
            )
            self._hotkey.start()
            log.info("小岛AI输入法已就绪：按住「%s」说话",
                     HOTKEY_CHOICES.get(self._settings.data.get("hotkey", "alt_l"),
                                        HOTKEY_CHOICES["alt_l"])[0])
        except Exception as e:
            log.error("热键监听启动失败：%s（通常是缺少「输入监听/辅助功能」权限）", e)
            print(
                "[启动错误] 热键监听启动失败，请在「系统设置 → 隐私与安全性 → "
                "输入监听 / 辅助功能」中授权启动本程序的终端或 Python：\n"
                f"  {e}",
                file=sys.stderr,
            )

    # ---- 设置菜单 ----

    def _sync_menu_state(self) -> None:
        """把 settings 当前值同步到菜单勾选状态。"""
        current = self._settings.data.get("hotkey", "alt_l")
        for name, item in self._hotkey_items.items():
            item.state = 1 if name == current else 0
        polish = self._settings.data.get("polish", {})
        self._polish_item.state = 1 if polish.get("enabled") else 0
        provider = polish.get("provider", "openai")
        model = polish.get("model") or "?"
        self._polish_item.title = f"AI 润色（{provider} / {model}）"

    def _make_hotkey_cb(self, name: str):
        def _cb(_):
            self._settings.data["hotkey"] = name
            self._settings.save()
            if self._hotkey:
                self._hotkey.set_trigger(name)
            self._sync_menu_state()
        return _cb

    def toggle_polish(self, _) -> None:
        polish = self._settings.data.setdefault("polish", {})
        if not polish.get("enabled") and not self._polisher.configured:
            rumps.notification(
                "小岛AI输入法", "无法开启 AI 润色",
                "请先点「设置 → 打开配置文件」，填写 polish 的 base_url / api_key / model。",
            )
            self.open_settings(None)
            return
        polish["enabled"] = not polish.get("enabled", False)
        self._settings.save()
        self._sync_menu_state()
        log.info("AI 润色已%s", "开启" if polish["enabled"] else "关闭")

    def reload_settings(self, _) -> None:
        """外部编辑 settings.json 后手动重载，无需重启进程。"""
        self._settings.load()
        if self._hotkey:
            self._hotkey.set_trigger(self._settings.data.get("hotkey", "alt_l"))
        self._sync_menu_state()
        log.info("配置已重新加载")

    def open_settings(self, _) -> None:
        try:
            subprocess.Popen(["open", self._settings.ensure_file()])
        except Exception as e:
            log.warning("打开配置文件失败：%s", e)

    # ---- 状态与通用菜单 ----

    def _set_status(self, state: str) -> None:
        """由子线程调用，更新菜单栏图标与菜单文字。"""
        try:
            self.title = _STATUS_ICON.get(state, ICON_IDLE)
            self._status_item.title = _STATUS_LABEL.get(state, "状态：待机")
        except Exception as e:
            log.debug("更新状态显示失败：%s", e)

    def open_log(self, _) -> None:
        try:
            subprocess.Popen(["open", LOG_FILE])
        except Exception as e:
            log.warning("打开日志失败：%s", e)

    def quit_app(self, _) -> None:
        log.info("用户点击退出")
        try:
            if self._hotkey:
                self._hotkey.stop()
            self._transcriber.close()
        finally:
            rumps.quit_application()


def main() -> None:
    log.info("=== 小岛AI输入法启动 ===")
    XiaodaoIME().run()


if __name__ == "__main__":
    main()
