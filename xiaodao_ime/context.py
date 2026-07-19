"""前台应用感知：场景化润色（不同 App 自动用不同润色风格）。

settings.json 的 app_styles 示例：
    "app_styles": {
        "com.tencent.xinWeChat": "轻度纠错",   // 键可以是 bundle id
        "Mail": "书面化",                      // 也可以是应用名（大小写不敏感）
        "Terminal": "关闭"                     // "关闭"/"off" = 该 App 不润色
    }
每个 App 的 bundle id 会打在日志里（转写一次即可看到）。
"""
from AppKit import NSWorkspace

from xiaodao_ime.logger import get_logger

log = get_logger(__name__)

STYLE_OFF = ("关闭", "off")


def frontmost_app():
    """返回 (应用名, bundle id)；失败返回空串。"""
    try:
        app = NSWorkspace.sharedWorkspace().frontmostApplication()
        if app is None:
            return "", ""
        return str(app.localizedName() or ""), str(app.bundleIdentifier() or "")
    except Exception as e:
        log.debug("获取前台应用失败：%s", e)
        return "", ""


def pick_style(app_name: str, bundle_id: str, mapping: dict):
    """按 app_styles 匹配风格：bundle id 精确优先，其次应用名（大小写不敏感）。

    返回风格名（含 "关闭"），无匹配返回 None（走全局默认风格）。
    """
    if not mapping:
        return None
    if bundle_id and bundle_id in mapping:
        return mapping[bundle_id]
    if app_name:
        lowered = {k.lower(): v for k, v in mapping.items() if k}
        return lowered.get(app_name.lower())
    return None
