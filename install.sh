#!/usr/bin/env bash
# 小岛AI输入法 一键安装（macOS）
#
# 用法（任选其一）：
#   curl -fsSL https://raw.githubusercontent.com/xiaowuzicode/xiaodao-ime/main/install.sh | bash
#   git clone 后在仓库根目录执行：bash install.sh
#
# 做的事：clone/更新代码 → venv 装依赖 → 预下载语音模型（失败不阻塞）
#         → 打包成独立 .app → 装入 /Applications 并启动。
# 装完只剩一步人工操作：到「系统设置 → 隐私与安全性」授三项权限。
set -euo pipefail

REPO="https://github.com/xiaowuzicode/xiaodao-ime.git"
DIR="${XIAODAO_DIR:-$HOME/xiaodao-ime}"
APP_NAME="小岛AI输入法"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "本脚本仅支持 macOS。Windows 请见 README「快速开始 → Windows」。" >&2
  exit 1
fi

command -v git >/dev/null 2>&1 || {
  echo "缺少 git，请先执行：xcode-select --install" >&2; exit 1; }
command -v python3 >/dev/null 2>&1 || {
  echo "缺少 python3（需 3.11+），可用 brew install python@3.12" >&2; exit 1; }
python3 -c 'import sys; sys.exit(0 if sys.version_info >= (3, 11) else 1)' || {
  echo "python3 版本过低（$(python3 -V 2>&1)），需 3.11+，可用 brew install python@3.12" >&2
  exit 1; }

# 若已在仓库根目录内执行（bash install.sh），就地安装；否则 clone 到 $DIR
if [[ -f "./install.sh" && -f "./requirements.txt" && -d "./xiaodao_ime" ]]; then
  DIR="$(pwd)"
  echo "==> 检测到当前目录即仓库，就地安装：$DIR"
elif [[ -d "$DIR/.git" ]]; then
  echo "==> 更新已有代码：$DIR"
  git -C "$DIR" pull --ff-only || echo "⚠️ 更新失败（本地有改动？），用现有代码继续"
else
  echo "==> 获取代码：$DIR"
  git clone --depth 1 "$REPO" "$DIR"
fi
cd "$DIR"

echo "==> 创建 venv 并安装依赖"
[[ -x .venv/bin/python ]] || python3 -m venv .venv
.venv/bin/pip install -q --upgrade pip
.venv/bin/pip install -q -r requirements.txt

echo "==> 预下载语音模型（241MB，已存在则跳过）"
download_model() {
  .venv/bin/python -c "
import os, sys
from xiaodao_ime.config import MODEL_PATH, MODEL_REPO, MODEL_FILENAME, MODELS_DIR
if os.path.isfile(MODEL_PATH):
    print('模型已存在，跳过'); sys.exit(0)
from huggingface_hub import hf_hub_download
hf_hub_download(MODEL_REPO, MODEL_FILENAME, local_dir=MODELS_DIR)
print('模型就绪')
"
}
if ! download_model; then
  echo "直连 HuggingFace 失败，改走国内镜像 hf-mirror.com 重试…"
  HF_ENDPOINT="https://hf-mirror.com" download_model || \
    echo "⚠️ 模型预下载失败，不阻塞安装——App 首次启动会自动下载（可先设 HF_ENDPOINT）"
fi

echo "==> 打包独立 App"
scripts/make_app.sh

echo "==> 装入 /Applications 并启动"
rm -rf "/Applications/$APP_NAME.app"
cp -R "dist/$APP_NAME.app" /Applications/
open "/Applications/$APP_NAME.app"

cat <<'DONE'

✅ 安装完成，「小岛AI输入法」已启动。

只剩最后一步（仅首次，需要人工点击）：
  系统设置 → 隐私与安全性，为「小岛AI输入法」开启这三项：
    · 输入监听    （监听全局热键）
    · 辅助功能    （模拟 Cmd+V 粘贴）
    · 麦克风      （录音，全本地处理不上传）
  授权后退出并重开 App。

菜单栏出现声浪图标 ·ıllı· 即就绪：单击左 Option 开始说话，再单击一次，文字落进光标处。
DONE
