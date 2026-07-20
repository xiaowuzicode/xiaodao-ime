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
echo ""
echo "⚠️  重要（TCC 权限规则）："
echo "   1. 权限认「签名」不认路径。每次重新打包签名都会变，之前授过的权限会"
echo "      静默失效（设置里开关看着还开着）——必须先用「-」移除旧条目再重新添加。"
echo "   2. 改代码【不需要】重新打包：App 只是个启动器，代码实时读项目目录，"
echo "      改完代码退出并重开 App 即可。只有改 Info.plist/启动器时才重新打包。"
echo ""
echo "   首次启动请在「系统设置 → 隐私与安全性」为「$APP_NAME」授予："
echo "   输入监听 / 辅助功能 / 麦克风，然后重启 App。"
echo "   （App 内已有权限自检，缺什么会在日志和系统通知里明确指出）"
