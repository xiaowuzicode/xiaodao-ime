"""系统权限自检（TCC）：输入监听 + 辅助功能。

macOS 的权限授予对象是「进程的签名身份」而非路径：
  - 终端启动 -> 权限挂在终端 App 上；
  - .app 启动 -> 挂在 App bundle 上；但 ad-hoc 签名每次重新打包都会变，
    旧授权会静默失效（设置里开关看着还开着）——必须先移除旧条目再重新勾选。

用 Quartz 的 Preflight/Request 接口：Preflight 只查不弹窗；Request 会触发系统弹窗
并把当前宿主自动加进「系统设置 -> 隐私与安全性」对应列表（用户只需打勾）。
"""
import Quartz

from xiaodao_ime.logger import get_logger

log = get_logger(__name__)


def check_permissions(prompt: bool = False) -> dict:
    """返回 {"input_monitoring": bool, "accessibility": bool}；prompt=True 时对缺失项触发系统弹窗。"""
    try:
        listen = bool(Quartz.CGPreflightListenEventAccess())   # 输入监听（全局热键）
        post = bool(Quartz.CGPreflightPostEventAccess())       # 辅助功能（模拟 Cmd+V）
    except Exception as e:
        log.warning("权限自检不可用：%s", e)
        return {"input_monitoring": True, "accessibility": True}  # 查不了就不拦

    if prompt:
        try:
            if not listen:
                Quartz.CGRequestListenEventAccess()
            if not post:
                Quartz.CGRequestPostEventAccess()
        except Exception as e:
            log.debug("触发权限弹窗失败：%s", e)

    log.info("权限自检：输入监听=%s，辅助功能=%s",
             "✅" if listen else "❌ 未授权", "✅" if post else "❌ 未授权")
    if not listen:
        log.warning("【热键无响应的原因】输入监听未授权：系统设置 → 隐私与安全性 → 输入监听，"
                    "勾选本程序的宿主（.app 启动就是「小岛AI输入法」，终端启动就是终端）。"
                    "若列表里已有旧条目仍不生效：先用「-」移除，再重新添加勾选（重新打包后签名已变）。"
                    "改完必须重启本程序。")
    if not post:
        log.warning("【出不了字的原因】辅助功能未授权：系统设置 → 隐私与安全性 → 辅助功能，"
                    "同上勾选宿主并重启。")
    return {"input_monitoring": listen, "accessibility": post}
