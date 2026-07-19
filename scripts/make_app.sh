#!/bin/zsh
# 把小岛AI输入法打包成独立 .app（dist/小岛AI输入法.app）。
#
# 意义：macOS 的权限（麦克风/辅助功能/输入监听）授予的是「启动进程的宿主 App」。
# 终端启动时权限挂在终端上；打包成 .app 后权限授予输入法本身，
# 双击即用、可放进「登录项」开机自启，不再依赖终端。
#
# 用法：scripts/make_app.sh   （构建后把 dist/小岛AI输入法.app 拖进「应用程序」即可）
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_NAME="小岛AI输入法"
APP_DIR="$ROOT/dist/$APP_NAME.app"
PYTHON="$ROOT/.venv/bin/python"

if [[ ! -x "$PYTHON" ]]; then
  echo "错误：找不到 $PYTHON，请先按 README 创建 venv 并安装依赖" >&2
  exit 1
fi

rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS"

cat > "$APP_DIR/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key><string>ai.xiaodao.ime</string>
    <key>CFBundleName</key><string>$APP_NAME</string>
    <key>CFBundleDisplayName</key><string>$APP_NAME</string>
    <key>CFBundleExecutable</key><string>xiaodao-ime</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>0.2.0</string>
    <key>LSUIElement</key><true/>
    <key>NSMicrophoneUsageDescription</key>
    <string>小岛AI输入法需要麦克风来进行本地语音转写（语音不会上传）。</string>
</dict>
</plist>
PLIST

cat > "$APP_DIR/Contents/MacOS/xiaodao-ime" <<LAUNCHER
#!/bin/zsh
cd "$ROOT"
exec "$PYTHON" "$ROOT/app.py"
LAUNCHER
chmod +x "$APP_DIR/Contents/MacOS/xiaodao-ime"

# ad-hoc 签名，让 TCC 权限稳定挂在这个 bundle 上
codesign --force --deep -s - "$APP_DIR"

echo "✅ 已生成 $APP_DIR"
echo "   首次启动后请在「系统设置 → 隐私与安全性」为「$APP_NAME」授予："
echo "   输入监听 / 辅助功能 / 麦克风（之前授给终端的权限不再需要）"
