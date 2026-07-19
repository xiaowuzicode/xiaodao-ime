"""提示音反馈：录音开始/结束/取消播放系统音效（afplay，异步不阻塞）。"""
import os
import subprocess

from xiaodao_ime.logger import get_logger

log = get_logger(__name__)

_SOUNDS = {
    "start": "/System/Library/Sounds/Tink.aiff",
    "stop": "/System/Library/Sounds/Pop.aiff",
    "cancel": "/System/Library/Sounds/Bottle.aiff",
}


def play(event: str, settings=None) -> None:
    """播放提示音；settings.sounds 为 False 时静音。任何失败静默忽略。"""
    if settings is not None and not settings.data.get("sounds", True):
        return
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
