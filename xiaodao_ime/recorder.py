"""录音：sounddevice，16kHz 单声道 float32，按住开始/松手停止。"""
import threading
import time
from typing import Optional

import numpy as np
import sounddevice as sd

from xiaodao_ime.config import CHANNELS, SAMPLE_RATE
from xiaodao_ime.logger import get_logger

log = get_logger(__name__)


class Recorder:
    """流式录音器：start() 打开输入流并持续收集帧，stop() 关闭并返回整段 PCM。"""

    def __init__(self, samplerate: int = SAMPLE_RATE, channels: int = CHANNELS):
        self._samplerate = samplerate
        self._channels = channels
        self._frames: list[np.ndarray] = []
        self._stream: Optional[sd.InputStream] = None
        self._lock = threading.Lock()
        self._started_at: float = 0.0
        self._recording = False

    @property
    def recording(self) -> bool:
        return self._recording

    def _callback(self, indata, frames, time_info, status) -> None:  # noqa: ANN001
        if status:
            log.debug("录音流状态：%s", status)
        with self._lock:
            self._frames.append(indata.copy())

    def start(self) -> None:
        """开始录音。若已在录音则忽略。"""
        if self._recording:
            return
        with self._lock:
            self._frames = []
        self._stream = sd.InputStream(
            samplerate=self._samplerate,
            channels=self._channels,
            dtype="float32",
            callback=self._callback,
        )
        self._stream.start()
        self._recording = True
        self._started_at = time.perf_counter()
        log.info("录音开始")

    def duration(self) -> float:
        """当前已录时长（秒）。"""
        if not self._started_at:
            return 0.0
        return time.perf_counter() - self._started_at

    def stop(self) -> tuple[np.ndarray, float]:
        """停止录音，返回 (PCM float32 一维数组, 录音时长秒)。未在录音则返回空。"""
        if not self._recording:
            return np.zeros(0, dtype=np.float32), 0.0
        dur = self.duration()
        self._recording = False
        try:
            self._stream.stop()
            self._stream.close()
        except Exception as e:
            log.warning("关闭录音流出错：%s", e)
        finally:
            self._stream = None
        with self._lock:
            frames = self._frames
            self._frames = []
        if not frames:
            log.info("录音停止：无音频数据，时长 %.3fs", dur)
            return np.zeros(0, dtype=np.float32), dur
        pcm = np.concatenate(frames, axis=0).reshape(-1).astype(np.float32)
        log.info("录音停止：时长 %.3fs，样本数 %d", dur, len(pcm))
        return pcm, dur

    def abort(self) -> float:
        """取消录音并丢弃数据，返回已录时长（秒）。"""
        if not self._recording:
            return 0.0
        dur = self.duration()
        self._recording = False
        try:
            if self._stream is not None:
                self._stream.stop()
                self._stream.close()
        except Exception as e:
            log.warning("取消录音关闭流出错：%s", e)
        finally:
            self._stream = None
        with self._lock:
            self._frames = []
        log.info("录音取消：已丢弃，时长 %.3fs", dur)
        return dur
