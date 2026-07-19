"""用户设置：settings.json 读写与默认值。

settings.json 位于项目根目录、不入 git（见 .gitignore），
缺失或解析失败时回退默认值，字段级深合并（用户只需写想改的键）。
"""
import copy
import json
import os

from xiaodao_ime.config import PROJECT_ROOT
from xiaodao_ime.logger import get_logger

log = get_logger(__name__)

SETTINGS_PATH = os.path.join(PROJECT_ROOT, "settings.json")

DEFAULTS = {
    # 按住说话的热键：alt_l 左Option / alt_r 右Option / cmd_r 右Command / f19
    "hotkey": "alt_l",
    # LLM 润色（可选）：去口水词、修正同音错字、规范标点。Key 为用户自备的大模型 API Key。
    "polish": {
        "enabled": False,
        # openai:    任意 OpenAI 兼容端点（默认）—— DeepSeek / Kimi / GLM / OpenAI / ollama 均可
        # anthropic: Anthropic API（需 api_key 或环境变量 ANTHROPIC_API_KEY）
        "provider": "openai",
        "model": "deepseek-chat",
        "api_key": "",
        "base_url": "https://api.deepseek.com",
        "timeout": 30,
    },
    # 热词（人名/产品名/术语）：注入润色提示词，辅助纠正同音错字
    "hotwords": [],
    # 离线替换表：转写后无条件字符串替换（不依赖 LLM），如 {"欧朵": "Ordo"}
    "replacements": {},
}


def _merge(base: dict, override: dict) -> dict:
    out = copy.deepcopy(base)
    for key, value in override.items():
        if isinstance(value, dict) and isinstance(out.get(key), dict):
            out[key] = _merge(out[key], value)
        else:
            out[key] = value
    return out


class Settings:
    """settings.json 的内存镜像。改 self.data 后调用 save() 落盘。"""

    def __init__(self, path: str = SETTINGS_PATH):
        self._path = path
        self.data: dict = copy.deepcopy(DEFAULTS)
        self.load()

    @property
    def path(self) -> str:
        return self._path

    def load(self) -> dict:
        if os.path.isfile(self._path):
            try:
                with open(self._path, encoding="utf-8") as f:
                    self.data = _merge(DEFAULTS, json.load(f))
                log.info("已加载设置：%s", self._path)
            except Exception as e:
                log.error("settings.json 解析失败，使用默认设置：%s", e)
        return self.data

    def save(self) -> None:
        try:
            with open(self._path, "w", encoding="utf-8") as f:
                json.dump(self.data, f, ensure_ascii=False, indent=2)
                f.write("\n")
            log.info("设置已保存：%s", self._path)
        except Exception as e:
            log.error("保存设置失败：%s", e)

    def ensure_file(self) -> str:
        """确保 settings.json 存在（用于「打开配置文件」），返回路径。"""
        if not os.path.isfile(self._path):
            self.save()
        return self._path
