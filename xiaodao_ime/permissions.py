"""系统权限自检（委托平台后端）。

macOS：TCC 输入监听 + 辅助功能（详见 platform/mac.py 注释）；
Windows：无对应权限体系，恒返回通过。
"""
from xiaodao_ime import platform as _platform


def check_permissions(prompt: bool = False) -> dict:
    """返回 {"input_monitoring": bool, "accessibility": bool}；
    prompt=True 时对缺失项触发系统授权弹窗（如果平台支持）。"""
    return _platform.backend.check_permissions(prompt=prompt)
