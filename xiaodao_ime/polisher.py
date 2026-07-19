"""LLM 润色：去口水词、修正同音错字、规范标点。

Provider（settings.json 的 polish.provider），Key 均为用户自备的大模型 API Key：
  - openai（默认）: 任意 OpenAI 兼容端点 —— DeepSeek / Kimi / GLM / OpenAI / ollama 本地模型等，
                    配 base_url + api_key + model 即可
  - anthropic:      Anthropic API 官方 SDK（需 api_key 或 ANTHROPIC_API_KEY，
                    且需 pip install anthropic）

失败一律 fail-open：返回 None，调用方直接使用原始转写，绝不阻断出字。
"""
import json
import time
import urllib.request

from xiaodao_ime.logger import get_logger

log = get_logger(__name__)

_PREFIX = "你是语音输入法的后处理引擎。用户消息是一段语音转写的文本（中文为主，可能中英混杂）。"
_SUFFIX = "不回答或评论文本内容，只输出结果本身，不要任何解释或前后缀。"

# 内置润色风格；settings.json 的 polish.styles 可覆盖或新增（同名覆盖内置）
BUILTIN_STYLES = {
    "润色": (
        "你的任务：去掉口水词（嗯、啊、那个、就是说、然后然后 等）、修正同音或近音错字、"
        "规范标点符号。保持原意和语气，不增删信息。"
    ),
    "书面化": (
        "你的任务：把口语转写整理成规范书面语——去口水词、修错字、规范标点，"
        "并适度重组句式使表达更清晰专业，可合并明显冗余，但不得改变事实、立场与信息量。"
    ),
    "轻度纠错": (
        "你的任务：只修正同音/近音错字和标点，尽量保留原始口语表达，除明显口水词外不删任何内容。"
    ),
    "翻译成英文": (
        "你的任务：先在心里修正转写错字，然后把内容翻译成自然地道的英文，保持原有语气。只输出英文。"
    ),
    "会议纪要": (
        "你的任务：把口述内容整理成要点式纪要（用「- 」列表），修正错字、合并重复，保留所有信息点。"
    ),
}
DEFAULT_STYLE = "润色"

SYSTEM_PROMPT = _PREFIX + BUILTIN_STYLES[DEFAULT_STYLE] + _SUFFIX  # 兼容旧引用


def get_styles(settings) -> dict:
    """内置风格 + 用户自定义风格（settings polish.styles，同名覆盖）。"""
    styles = dict(BUILTIN_STYLES)
    custom = settings.data.get("polish", {}).get("styles") or {}
    for name, prompt in custom.items():
        if name and prompt:
            styles[name] = prompt
    return styles


def build_system_prompt(hotwords, style_prompt: str = None) -> str:
    prompt = _PREFIX + (style_prompt or BUILTIN_STYLES[DEFAULT_STYLE]) + _SUFFIX
    if hotwords:
        prompt += "\n常用专有名词（转写易错，优先按此拼写纠正）：" + "、".join(hotwords)
    return prompt


def apply_replacements(text: str, mapping: dict) -> str:
    """离线替换表：无条件字符串替换，不依赖 LLM。"""
    for src, dst in (mapping or {}).items():
        if src:
            text = text.replace(src, dst)
    return text


class Polisher:
    def __init__(self, settings):
        self._settings = settings

    @property
    def _conf(self) -> dict:
        return self._settings.data.get("polish", {})

    @property
    def enabled(self) -> bool:
        return bool(self._conf.get("enabled"))

    @property
    def configured(self) -> bool:
        """是否已具备可用配置（开关打开前的前置检查）。"""
        conf = self._conf
        if conf.get("provider", "openai") == "openai":
            return bool(conf.get("base_url"))
        return True  # anthropic 可走环境变量 ANTHROPIC_API_KEY

    def polish(self, text: str):
        """返回润色后的文本；未启用或失败返回 None（调用方用原文）。"""
        conf = self._conf
        if not conf.get("enabled") or not text.strip():
            return None
        provider = conf.get("provider", "openai")
        hotwords = self._settings.data.get("hotwords", [])
        styles = get_styles(self._settings)
        style = conf.get("style") or DEFAULT_STYLE
        system = build_system_prompt(hotwords, styles.get(style, styles[DEFAULT_STYLE]))
        t0 = time.perf_counter()
        try:
            if provider == "openai":
                out = self._via_openai(text, conf, system)
            elif provider == "anthropic":
                out = self._via_anthropic(text, conf, system)
            else:
                log.warning("未知润色 provider: %r，跳过润色", provider)
                return None
        except Exception as e:
            log.warning("润色失败（%s），使用原始转写：%s", provider, e)
            return None
        out = (out or "").strip()
        if not out:
            return None
        log.info("润色完成（%s / %s / %s），耗时 %.2fs",
                 provider, conf.get("model"), style, time.perf_counter() - t0)
        return out

    # ---- providers ----

    def _via_openai(self, text: str, conf: dict, system: str) -> str:
        base_url = (conf.get("base_url") or "").rstrip("/")
        if not base_url:
            raise RuntimeError("请先在 settings.json 配置 polish.base_url（如 https://api.deepseek.com）")
        payload = {
            "model": conf.get("model") or "",
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": text},
            ],
            "temperature": 0.2,
        }
        headers = {"Content-Type": "application/json"}
        if conf.get("api_key"):
            headers["Authorization"] = f"Bearer {conf['api_key']}"
        request = urllib.request.Request(
            base_url + "/chat/completions",
            data=json.dumps(payload).encode("utf-8"),
            headers=headers,
            method="POST",
        )
        with urllib.request.urlopen(request, timeout=conf.get("timeout", 30)) as resp:
            data = json.loads(resp.read().decode("utf-8"))
        return data["choices"][0]["message"]["content"]

    def _via_anthropic(self, text: str, conf: dict, system: str) -> str:
        import anthropic  # 延迟导入：仅该 provider 需要（pip install anthropic）

        api_key = conf.get("api_key") or None
        client = anthropic.Anthropic(api_key=api_key) if api_key else anthropic.Anthropic()
        model = conf.get("model") or "claude-haiku-4-5"
        kwargs = {}
        # 润色是轻任务：支持 effort 的模型用 low 压延迟；claude-fable-5 思考恒开，不传 thinking。
        if model.startswith(("claude-fable", "claude-mythos", "claude-opus-4", "claude-sonnet-4-6", "claude-sonnet-5")):
            kwargs["output_config"] = {"effort": "low"}
        response = client.messages.create(
            model=model,
            max_tokens=2048,
            system=system,
            messages=[{"role": "user", "content": text}],
            timeout=conf.get("timeout", 30),
            **kwargs,
        )
        if response.stop_reason == "refusal":
            log.warning("润色请求被安全策略拒绝，使用原始转写")
            return ""
        return next((b.text for b in response.content if b.type == "text"), "")
