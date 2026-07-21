"""全局配置与路径常量。"""
import os
import sys

# 项目根目录（本文件位于 <root>/xiaodao_ime/config.py）
PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# PyInstaller 打包运行时（sys.frozen），用户数据放系统标准位置；
# 源码运行时沿用项目目录，便于开发调试。
IS_FROZEN = bool(getattr(sys, "frozen", False))
if IS_FROZEN:
    if sys.platform == "darwin":
        BASE_DIR = os.path.expanduser("~/Library/Application Support/xiaodao-ime")
    elif sys.platform.startswith("win"):
        BASE_DIR = os.path.join(
            os.environ.get("APPDATA", os.path.expanduser("~")), "xiaodao-ime")
    else:
        BASE_DIR = os.path.expanduser("~/.local/share/xiaodao-ime")
else:
    BASE_DIR = PROJECT_ROOT

MODELS_DIR = os.path.join(BASE_DIR, "models")
LOGS_DIR = os.path.join(BASE_DIR, "logs")
LOG_FILE = os.path.join(LOGS_DIR, "xiaodao-ime.log")
DATA_DIR = os.path.join(BASE_DIR, "data")
HISTORY_FILE = os.path.join(DATA_DIR, "history.jsonl")

for _d in (MODELS_DIR, LOGS_DIR, DATA_DIR):
    os.makedirs(_d, exist_ok=True)

# 模型下载源（首次运行自动下载）
MODEL_REPO = "handy-computer/SenseVoiceSmall-gguf"

# 转写模型：SenseVoice Small，Q8_0 量化（中文最强、自带标点、速度极快）
MODEL_FILENAME = os.environ.get("XIAODAO_MODEL", "SenseVoiceSmall-Q8_0.gguf")
MODEL_PATH = os.path.join(MODELS_DIR, MODEL_FILENAME)

# 转写后端：auto 自动挑选最佳设备（Apple Silicon 上为 Metal）
TRANSCRIBE_BACKEND = os.environ.get("XIAODAO_BACKEND", "auto")
# 语言：None = 自动检测（SenseVoice 支持 zh/yue/en/ja/ko）；可设为 "zh" 强制中文
TRANSCRIBE_LANGUAGE = os.environ.get("XIAODAO_LANGUAGE") or None

# 录音参数：transcribe.cpp 要求 16kHz 单声道 float32
SAMPLE_RATE = 16000
CHANNELS = 1

# 防误触：按住时长小于该秒数的录音直接丢弃
MIN_HOLD_SECONDS = 0.4

# 双击热键进入「锁定录音」的两次按下间隔上限（秒）
DOUBLE_TAP_WINDOW = 0.35

# 粘贴后恢复原剪贴板的延迟（秒）
CLIPBOARD_RESTORE_DELAY = 0.4

# 菜单栏图标状态
ICON_IDLE = "🏝️"
ICON_RECORDING = "🎙️"
ICON_TRANSCRIBING = "✍️"
ICON_POLISHING = "🪄"
