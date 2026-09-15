"""提示音反馈：录音开始/结束/取消播放系统音效（平台后端实现，异步不阻塞）。"""
from xiaodao_ime import platform as _platform
from xiaodao_ime.logger import get_logger

log = get_logger(__name__)


def play(event: str, settings=None) -> None:
    """播放提示音；settings.sounds 为 False 时静音。任何失败静默忽略。"""
    if settings is not None and not settings.data.get("sounds", True):
        return
    try:
        _platform.backend.play_sound(event)
    except Exception as e:
        log.debug("播放提示音失败：%s", e)
