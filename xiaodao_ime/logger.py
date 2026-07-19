"""日志配置：写入 logs/xiaodao-ime.log，带时间戳与文件名行号。"""
import logging
import os
from logging.handlers import RotatingFileHandler

from xiaodao_ime.config import LOG_FILE, LOGS_DIR

_configured = False


def get_logger(name: str = "xiaodao_ime") -> logging.Logger:
    """返回配置好的 logger（进程内只配置一次）。"""
    global _configured
    logger = logging.getLogger("xiaodao_ime")
    if not _configured:
        os.makedirs(LOGS_DIR, exist_ok=True)
        logger.setLevel(logging.INFO)
        fmt = logging.Formatter(
            "%(asctime)s [%(levelname)s] %(filename)s:%(lineno)d - %(message)s",
            datefmt="%Y-%m-%d %H:%M:%S",
        )
        file_handler = RotatingFileHandler(
            LOG_FILE, maxBytes=2 * 1024 * 1024, backupCount=3, encoding="utf-8"
        )
        file_handler.setFormatter(fmt)
        logger.addHandler(file_handler)

        stream_handler = logging.StreamHandler()
        stream_handler.setFormatter(fmt)
        logger.addHandler(stream_handler)

        logger.propagate = False
        _configured = True
    return logging.getLogger(name if name.startswith("xiaodao_ime") else "xiaodao_ime")
