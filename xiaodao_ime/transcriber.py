"""转写引擎：SenseVoice GGUF 常驻内存，通过 transcribe.cpp 的 Python 绑定转写。

模型在进程启动时加载一次并常驻；每次松手复用同一个 Model 创建 session 运行，
以命中「松手到出字 1 秒内」的目标。
"""
import os
import threading
import time
from typing import Optional

import numpy as np

import transcribe_cpp as tc
from xiaodao_ime.config import MODEL_PATH, TRANSCRIBE_BACKEND, TRANSCRIBE_LANGUAGE
from xiaodao_ime.logger import get_logger

log = get_logger(__name__)

_log_installed = False


def _install_native_log_callback() -> None:
    """把 ggml/transcribe.cpp 的原生日志路由到我们的 logger（debug 级），避免污染 stdout。

    必须在加载模型之前调用一次（0.x 契约）。
    """
    global _log_installed
    if _log_installed:
        return

    def _handler(level: int, message: str) -> None:
        try:
            msg = message.rstrip()
            if not msg:
                return
            if level >= 3:
                log.warning("[native] %s", msg)
            else:
                log.debug("[native] %s", msg)
        except Exception:
            pass

    try:
        tc.set_log_callback(_handler)
        _log_installed = True
    except Exception as e:  # pragma: no cover - 兜底，不应致命
        log.warning("安装原生日志回调失败：%s", e)


class Transcriber:
    """常驻内存的转写器。线程安全：转写串行执行（模型限制同一时刻只允许一个 run）。"""

    def __init__(self, model_path: str = MODEL_PATH,
                 backend: str = TRANSCRIBE_BACKEND,
                 language: Optional[str] = TRANSCRIBE_LANGUAGE):
        _install_native_log_callback()
        self._model_path = model_path
        self._backend = backend
        self._language = language
        self._lock = threading.Lock()
        self._model = None
        self.load_seconds = 0.0

    def load(self) -> float:
        """加载并常驻模型；返回加载耗时（秒）。"""
        if not os.path.isfile(self._model_path):
            raise FileNotFoundError(f"模型文件不存在：{self._model_path}")
        t0 = time.perf_counter()
        log.info("开始加载模型：%s (backend=%s)", self._model_path, self._backend)
        self._model = tc.Model(self._model_path, backend=self._backend)
        self.load_seconds = time.perf_counter() - t0
        try:
            dev = self._model.device
        except Exception:
            dev = "?"
        log.info("模型加载完成，耗时 %.3fs，设备=%s", self.load_seconds, dev)
        return self.load_seconds

    @property
    def loaded(self) -> bool:
        return self._model is not None

    def transcribe(self, pcm: np.ndarray) -> tuple[str, float]:
        """转写 16kHz 单声道 float32 PCM，返回 (文本, 耗时秒)。"""
        if self._model is None:
            raise RuntimeError("模型尚未加载，请先调用 load()")
        if pcm is None or len(pcm) == 0:
            return "", 0.0
        pcm = np.ascontiguousarray(pcm, dtype=np.float32).reshape(-1)
        with self._lock:
            t0 = time.perf_counter()
            session = self._model.session()
            try:
                result = session.run(pcm, language=self._language)
            finally:
                try:
                    session.close()
                except Exception:
                    pass
            dt = time.perf_counter() - t0
        text = (result.text or "").strip()
        log.info("转写完成，耗时 %.3fs，样本数=%d，文本=%r", dt, len(pcm), text)
        return text, dt

    def close(self) -> None:
        with self._lock:
            if self._model is not None:
                try:
                    self._model.close()
                except Exception:
                    pass
                self._model = None
