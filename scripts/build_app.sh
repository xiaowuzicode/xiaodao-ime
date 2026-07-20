#!/bin/zsh
# 用 PyInstaller 打真正自包含的「小岛AI输入法.app」（发布版）。
#
# 与 scripts/make_app.sh（开发版壳，依赖项目目录和 venv）不同：
# 本脚本产出的 App 自带 Python 运行时与全部依赖，可拷给任何 Apple Silicon Mac；
# TCC 权限干净地归属 App 自身；用户数据在 ~/Library/Application Support/xiaodao-ime；
# 首次启动自动下载模型。
#
# 用法：scripts/build_app.sh  → 产物 dist/小岛AI输入法.app
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
PY="$ROOT/.venv/bin"
APP_NAME="小岛AI输入法"

"$PY/pip" show pyinstaller >/dev/null 2>&1 || "$PY/pip" install pyinstaller

rm -rf build "dist/XiaodaoIME" "dist/XiaodaoIME.app" "dist/$APP_NAME.app"

"$PY/pyinstaller" --noconfirm --clean --windowed \
  --name XiaodaoIME \
  --icon "$ROOT/resources/icon.icns" \
  --osx-bundle-identifier ai.xiaodao.ime \
  --collect-all transcribe_cpp \
  --collect-all transcribe_cpp_native \
  app.py

APP="dist/XiaodaoIME.app"
PLIST="$APP/Contents/Info.plist"
pb() { /usr/libexec/PlistBuddy -c "$1" "$PLIST" 2>/dev/null || true; }
pb "Add :LSUIElement bool true";            pb "Set :LSUIElement true"
pb "Add :NSMicrophoneUsageDescription string 占位"
pb "Set :NSMicrophoneUsageDescription 小岛AI输入法需要麦克风进行本地语音转写（语音不会上传）。"
pb "Set :CFBundleName $APP_NAME"
pb "Add :CFBundleDisplayName string $APP_NAME"; pb "Set :CFBundleDisplayName $APP_NAME"

mv "$APP" "dist/$APP_NAME.app"
codesign --force --deep -s - "dist/$APP_NAME.app"

echo ""
echo "✅ 发布版已生成：dist/$APP_NAME.app（自包含，可直接拖进「应用程序」）"
echo "   首次启动会自动下载模型（241MB）；权限授予对象就是「$APP_NAME」本身。"
echo "   ⚠️ 重新构建后签名会变：系统设置里旧授权条目需先移除再重新勾选。"
