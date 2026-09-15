"""输入历史：本地 data/history.jsonl 追加存储，菜单展示最近 N 条并可点击复制。

线程模型：append 来自转写 worker 线程；recent/version 由主线程（rumps Timer）读取，
用锁保护内存列表；version 自增供 UI 判断是否需要重绘。
"""
import json
import os
import threading
import time

from xiaodao_ime.config import HISTORY_FILE
from xiaodao_ime.logger import get_logger

log = get_logger(__name__)


class History:
    def __init__(self, settings, path: str = HISTORY_FILE):
        self._settings = settings
        self._path = path
        self._lock = threading.Lock()
        self._items: list = []
        self.version = 0
        self.total_count = 0   # 累计出字次数（全量，含已滚出内存的旧记录）
        self.total_chars = 0   # 累计出字字数
        self._load()

    @property
    def _conf(self) -> dict:
        return self._settings.data.get("history", {})

    @property
    def enabled(self) -> bool:
        return bool(self._conf.get("enabled", True))

    def _max_items(self) -> int:
        return int(self._conf.get("max_items", 50))

    def _load(self) -> None:
        if not os.path.isfile(self._path):
            return
        try:
            with open(self._path, encoding="utf-8") as f:
                lines = [line for line in f if line.strip()]
            for line in lines:
                try:
                    entry = json.loads(line)
                    self.total_count += 1
                    self.total_chars += len(entry.get("final", ""))
                except Exception:
                    continue
            self._items = [json.loads(line) for line in lines[-self._max_items():]]
            self.version += 1
            log.info("已加载历史 %d 条（累计 %d 次 / %d 字）",
                     len(self._items), self.total_count, self.total_chars)
        except Exception as e:
            log.warning("加载历史失败：%s", e)

    def append(self, raw: str, final: str) -> None:
        if not self.enabled or not final.strip():
            return
        entry = {"ts": time.strftime("%Y-%m-%d %H:%M:%S"), "raw": raw, "final": final}
        with self._lock:
            self._items.append(entry)
            self._items = self._items[-self._max_items():]
            self.total_count += 1
            self.total_chars += len(final)
            self.version += 1
        try:
            os.makedirs(os.path.dirname(self._path), exist_ok=True)
            with open(self._path, "a", encoding="utf-8") as f:
                f.write(json.dumps(entry, ensure_ascii=False) + "\n")
        except Exception as e:
            log.warning("写入历史失败：%s", e)

    def recent(self, n: int = 10) -> list:
        with self._lock:
            return list(reversed(self._items[-n:]))
