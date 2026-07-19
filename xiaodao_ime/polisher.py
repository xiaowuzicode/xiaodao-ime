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

SYSTEM_PROMPT = (
    "你是语音输入法的润色引擎。用户消息是一段语音转写的文本（中文为主，可能中英混杂）。"
    "你的任务：去掉口水词（嗯、啊、那个、就是说、然后然后 等）、修正同音或近音错字、"
    "规范标点符号。保持原意和语气，不增删信息，不回答或评论文本内容。"
    "只输出润色后的文本本身，不要任何解释或前后缀。"
)


def build_system_prompt(hotwords) -> str:
    if hotwords:
        return SYSTEM_PROMPT + "\n常用专有名词（转写易错，优先按此拼写纠正）：" + "、".join(hotwords)
    return SYSTEM_PROMPT


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
        t0 = time.perf_counter()
        try:
            if provider == "openai":
                out = self._via_openai(text, conf, hotwords)
            elif provider == "anthropic":
                out = self._via_anthropic(text, conf, hotwords)
            else:
                log.warning("未知润色 provider: %r，跳过润色", provider)
                return None
        except Exception as e:
            log.warning("润色失败（%s），使用原始转写：%s", provider, e)
            return None
        out = (out or "").strip()
        if not out:
            return None
        log.info("润色完成（%s / %s），耗时 %.2fs", provider, conf.get("model"), time.perf_counter() - t0)
        return out

    # ---- providers ----

    def _via_openai(self, text: str, conf: dict, hotwords) -> str:
        base_url = (conf.get("base_url") or "").rstrip("/")
        if not base_url:
            raise RuntimeError("请先在 settings.json 配置 polish.base_url（如 https://api.deepseek.com）")
        payload = {
            "model": conf.get("model") or "",
            "messages": [
                {"role": "system", "content": build_system_prompt(hotwords)},
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

    def _via_anthropic(self, text: str, conf: dict, hotwords) -> str:
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
            system=build_system_prompt(hotwords),
            messages=[{"role": "user", "content": text}],
            timeout=conf.get("timeout", 30),
            **kwargs,
        )
        if response.stop_reason == "refusal":
            log.warning("润色请求被安全策略拒绝，使用原始转写")
            return ""
        return next((b.text for b in response.content if b.type == "text"), "")
