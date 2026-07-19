#!/usr/bin/env python3
"""小岛AI输入法 —— 菜单栏常驻入口。

按住左 Option 说话，松手把转写文字粘贴进当前输入框。

rumps 必须运行在主线程；pynput 监听、录音回调、转写均在子线程执行。
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
    ICON_RECORDING,
    ICON_TRANSCRIBING,
    LOG_FILE,
    MODEL_PATH,
)
from xiaodao_ime.hotkey import HotkeyController  # noqa: E402
from xiaodao_ime.logger import get_logger  # noqa: E402
from xiaodao_ime.recorder import Recorder  # noqa: E402
from xiaodao_ime.transcriber import Transcriber  # noqa: E402

log = get_logger("xiaodao_ime.app")

_STATUS_ICON = {
    "idle": ICON_IDLE,
    "recording": ICON_RECORDING,
    "transcribing": ICON_TRANSCRIBING,
}
_STATUS_LABEL = {
    "idle": "状态：待机",
    "recording": "状态：录音中",
    "transcribing": "状态：转写中",
}


class XiaodaoIME(rumps.App):
    def __init__(self):
        super().__init__(ICON_IDLE, quit_button=None)
        self._status_item = rumps.MenuItem("状态：待机")  # 无 callback => 置灰显示
        self.menu = [
            self._status_item,
            None,
            rumps.MenuItem("打开日志", callback=self.open_log),
            rumps.MenuItem("退出", callback=self.quit_app),
        ]
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
                self._recorder, self._transcriber, on_status=self._set_status
            )
            self._hotkey.start()
            log.info("小岛AI输入法已就绪：按住左 Option 说话")
        except Exception as e:
            log.error("热键监听启动失败：%s（通常是缺少「输入监听/辅助功能」权限）", e)
            print(
                "[启动错误] 热键监听启动失败，请在「系统设置 → 隐私与安全性 → "
                "输入监听 / 辅助功能」中授权启动本程序的终端或 Python：\n"
                f"  {e}",
                file=sys.stderr,
            )

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
