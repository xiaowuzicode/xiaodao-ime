"""macOS 原生设置窗口（AppKit / pyobjc）。

只被 app.py（macOS 入口）使用；核心层不依赖本模块。
覆盖日常设置项（热键/录音方式/预览/提示音/AI 润色）；
热词、替换表、场景感知等低频高级项仍走「编辑完整配置」打开 JSON。
"""
import objc
from AppKit import (
    NSAlert,
    NSApp,
    NSBackingStoreBuffered,
    NSButton,
    NSFont,
    NSMakeRect,
    NSObject,
    NSPopUpButton,
    NSSecureTextField,
    NSTextField,
    NSWindow,
    NSWindowStyleMaskClosable,
    NSWindowStyleMaskTitled,
)

from xiaodao_ime.hotkey import HOTKEY_CHOICES, RECORD_MODES
from xiaodao_ime.logger import get_logger
from xiaodao_ime.polisher import get_styles

log = get_logger(__name__)

_W, _H = 480, 532
_LABEL_X, _LABEL_W = 20, 124
_CTRL_X, _CTRL_W = 152, 300
_NSButtonTypeSwitch = 3  # NSButtonTypeSwitch（数值稳定，免 pyobjc 版本差异）

_PROVIDERS = ["openai", "anthropic"]
_PROVIDER_LABELS = [
    "OpenAI 兼容（DeepSeek / Kimi / GLM / ollama…）",
    "Anthropic",
]


def _rect(top: float, h: float, x: float = _CTRL_X, w: float = _CTRL_W):
    """按「距窗口顶部 top、高 h」放置（AppKit 坐标原点在左下）。"""
    return NSMakeRect(x, _H - top - h, w, h)


class SettingsWindowController(NSObject):
    """设置窗口：show() 弹出，「保存并应用」写回 settings 并回调 on_save。"""

    def initWithSettings_onSave_(self, settings, on_save):
        self = objc.super(SettingsWindowController, self).init()
        if self is None:
            return None
        self._settings = settings
        self._on_save = on_save
        self._window = None
        return self

    # ---- 构建 ----

    @objc.python_method
    def _label(self, text, top, bold=False, x=_LABEL_X, w=_LABEL_W, gray=False):
        lbl = NSTextField.labelWithString_(text)
        lbl.setFrame_(_rect(top, 20, x=x, w=w))
        size = 13
        lbl.setFont_(NSFont.boldSystemFontOfSize_(size) if bold
                     else NSFont.systemFontOfSize_(size))
        if gray:
            lbl.setTextColor_(lbl.textColor().colorWithAlphaComponent_(0.55))
        self._content.addSubview_(lbl)
        return lbl

    @objc.python_method
    def _popup(self, top, titles):
        pop = NSPopUpButton.alloc().initWithFrame_pullsDown_(_rect(top, 26), False)
        pop.addItemsWithTitles_(titles)
        self._content.addSubview_(pop)
        return pop

    @objc.python_method
    def _checkbox(self, top, title, w=_CTRL_W + 20):
        box = NSButton.alloc().initWithFrame_(_rect(top, 20, w=w))
        box.setButtonType_(_NSButtonTypeSwitch)
        box.setTitle_(title)
        box.setFont_(NSFont.systemFontOfSize_(13))
        self._content.addSubview_(box)
        return box

    @objc.python_method
    def _field(self, top, placeholder="", secure=False):
        cls = NSSecureTextField if secure else NSTextField
        field = cls.alloc().initWithFrame_(_rect(top, 24))
        field.setFont_(NSFont.systemFontOfSize_(13))
        if placeholder:
            field.setPlaceholderString_(placeholder)
        self._content.addSubview_(field)
        return field

    @objc.python_method
    def _button(self, title, x, w, action, default=False):
        btn = NSButton.alloc().initWithFrame_(NSMakeRect(x, 16, w, 30))
        btn.setBezelStyle_(1)  # NSBezelStyleRounded
        btn.setTitle_(title)
        btn.setTarget_(self)
        btn.setAction_(action)
        if default:
            btn.setKeyEquivalent_("\r")
        self._content.addSubview_(btn)
        return btn

    @objc.python_method
    def _build(self):
        mask = NSWindowStyleMaskTitled | NSWindowStyleMaskClosable
        win = NSWindow.alloc().initWithContentRect_styleMask_backing_defer_(
            NSMakeRect(0, 0, _W, _H), mask, NSBackingStoreBuffered, False)
        win.setTitle_("小岛AI输入法 设置")
        win.setReleasedWhenClosed_(False)
        win.center()
        self._window = win
        self._content = win.contentView()

        top = 18
        self._label("热键与录音", top, bold=True, w=200)
        top += 30
        self._label("听写热键", top)
        self._pop_hotkey = self._popup(top, [v[0] for v in HOTKEY_CHOICES.values()])
        top += 34
        self._label("改写热键", top)
        self._pop_rewrite = self._popup(top, [v[0] for v in HOTKEY_CHOICES.values()])
        top += 34
        self._label("录音方式", top)
        self._pop_mode = self._popup(top, list(RECORD_MODES.values()))
        top += 34
        self._chk_preview = self._checkbox(top, "实时预览悬浮窗（计时 / 声浪 / 边说边出字）")
        top += 28
        self._chk_sounds = self._checkbox(top, "录音开始 / 结束提示音")

        top += 40
        self._label("AI 润色（自备大模型 Key，可选）", top, bold=True, w=320)
        top += 30
        self._chk_polish = self._checkbox(top, "启用：去口水词、修同音错字、规范标点")
        top += 32
        self._label("服务商", top)
        self._pop_provider = self._popup(top, _PROVIDER_LABELS)
        top += 34
        self._label("Base URL", top)
        self._fld_base = self._field(top, "https://api.deepseek.com")
        top += 32
        self._label("API Key", top)
        self._fld_key = self._field(top, "sk-…（只存本机 settings.json）", secure=True)
        top += 32
        self._label("模型", top)
        self._fld_model = self._field(top, "deepseek-chat")
        top += 34
        self._label("润色风格", top)
        self._pop_style = self._popup(top, list(get_styles(self._settings)))

        top += 38
        self._label("热词、替换表、场景感知等高级项 → 编辑完整配置", top,
                    w=_W - 40, gray=True)

        self._button("编辑完整配置…", 20, 140, "editJSONClicked:")
        self._button("取消", _W - 216, 90, "cancelClicked:")
        self._button("保存并应用", _W - 122, 102, "saveClicked:", default=True)

    # ---- 数据同步 ----

    @objc.python_method
    def _load_values(self):
        s = self._settings.data
        names = list(HOTKEY_CHOICES.keys())
        self._pop_hotkey.selectItemAtIndex_(
            names.index(s.get("hotkey")) if s.get("hotkey") in names else 0)
        self._pop_rewrite.selectItemAtIndex_(
            names.index(s.get("rewrite_hotkey")) if s.get("rewrite_hotkey") in names else 1)
        modes = list(RECORD_MODES.keys())
        self._pop_mode.selectItemAtIndex_(
            modes.index(s.get("record_mode")) if s.get("record_mode") in modes else 0)
        self._chk_preview.setState_(1 if s.get("live_preview", True) else 0)
        self._chk_sounds.setState_(1 if s.get("sounds", True) else 0)

        polish = s.get("polish", {})
        self._chk_polish.setState_(1 if polish.get("enabled") else 0)
        provider = polish.get("provider", "openai")
        self._pop_provider.selectItemAtIndex_(
            _PROVIDERS.index(provider) if provider in _PROVIDERS else 0)
        self._fld_base.setStringValue_(polish.get("base_url", ""))
        self._fld_key.setStringValue_(polish.get("api_key", ""))
        self._fld_model.setStringValue_(polish.get("model", ""))
        # 风格列表可能因自定义 styles 变化，每次重建
        self._pop_style.removeAllItems()
        styles = list(get_styles(self._settings))
        self._pop_style.addItemsWithTitles_(styles)
        current = polish.get("style", "润色")
        if current in styles:
            self._pop_style.selectItemAtIndex_(styles.index(current))

    @objc.python_method
    def _alert(self, title, message):
        alert = NSAlert.alloc().init()
        alert.setMessageText_(title)
        alert.setInformativeText_(message)
        alert.runModal()

    # ---- 动作（selector 命名须带冒号后缀对应的下划线）----

    def saveClicked_(self, _sender):
        names = list(HOTKEY_CHOICES.keys())
        hotkey = names[self._pop_hotkey.indexOfSelectedItem()]
        rewrite = names[self._pop_rewrite.indexOfSelectedItem()]
        if hotkey == rewrite:
            self._alert("热键冲突", "听写热键和改写热键不能相同。")
            return
        s = self._settings.data
        s["hotkey"] = hotkey
        s["rewrite_hotkey"] = rewrite
        s["record_mode"] = list(RECORD_MODES.keys())[self._pop_mode.indexOfSelectedItem()]
        s["live_preview"] = bool(self._chk_preview.state())
        s["sounds"] = bool(self._chk_sounds.state())
        polish = s.setdefault("polish", {})
        polish["enabled"] = bool(self._chk_polish.state())
        polish["provider"] = _PROVIDERS[self._pop_provider.indexOfSelectedItem()]
        polish["base_url"] = str(self._fld_base.stringValue()).strip()
        polish["api_key"] = str(self._fld_key.stringValue()).strip()
        polish["model"] = str(self._fld_model.stringValue()).strip()
        styles = list(get_styles(self._settings))
        idx = self._pop_style.indexOfSelectedItem()
        if 0 <= idx < len(styles):
            polish["style"] = styles[idx]
        self._settings.save()
        log.info("设置窗口：已保存并应用")
        if self._on_save:
            self._on_save()
        self._window.orderOut_(None)

    def cancelClicked_(self, _sender):
        self._window.orderOut_(None)

    def editJSONClicked_(self, _sender):
        import subprocess
        # open -t：强制用文本编辑器打开（.json 的系统默认程序常是浏览器）
        subprocess.Popen(["open", "-t", self._settings.ensure_file()])

    # ---- 入口 ----

    @objc.python_method
    def show(self):
        if self._window is None:
            self._build()
        self._load_values()
        NSApp.activateIgnoringOtherApps_(True)
        self._window.makeKeyAndOrderFront_(None)
