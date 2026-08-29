# 小岛AI输入法 Windows 一键安装（beta）
#
# 用法（PowerShell 直接粘贴）：
#   irm https://raw.githubusercontent.com/xiaowuzicode/xiaodao-ime/main/install.ps1 | iex
#
# 做的事：下载最新 Release 的 Windows 包 → 装到 %LOCALAPPDATA%\Programs\XiaodaoIME
#         → 创建开始菜单快捷方式 → 启动。首次启动自动下载语音模型（241MB）。
$ErrorActionPreference = "Stop"

$repo = "xiaowuzicode/xiaodao-ime"
$dir = "$env:LOCALAPPDATA\Programs\XiaodaoIME"

Write-Host "==> 获取最新版本信息…"
$release = Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest"
$asset = $release.assets | Where-Object { $_.name -eq "XiaodaoIME-windows-x64.zip" } | Select-Object -First 1
if (-not $asset) {
    Write-Error "最新 Release（$($release.tag_name)）里没有 Windows 包，请到 https://github.com/$repo/releases 手动查看"
}

$zip = Join-Path $env:TEMP "XiaodaoIME-windows-x64.zip"
Write-Host "==> 下载 $($release.tag_name)（约 $([math]::Round($asset.size / 1MB)) MB）…"
Invoke-WebRequest $asset.browser_download_url -OutFile $zip

Write-Host "==> 安装到 $dir"
Get-Process XiaodaoIME -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
if (Test-Path $dir) { Remove-Item $dir -Recurse -Force }
Expand-Archive $zip -DestinationPath $dir -Force
Remove-Item $zip

# 开始菜单快捷方式
$ws = New-Object -ComObject WScript.Shell
$lnk = $ws.CreateShortcut("$env:APPDATA\Microsoft\Windows\Start Menu\Programs\小岛AI输入法.lnk")
$lnk.TargetPath = Join-Path $dir "XiaodaoIME.exe"
$lnk.WorkingDirectory = $dir
$lnk.Save()

Write-Host "==> 启动"
Start-Process (Join-Path $dir "XiaodaoIME.exe")

Write-Host ""
Write-Host "✅ 安装完成：托盘出现声浪图标即就绪" -ForegroundColor Green
Write-Host "   单击右 Ctrl 说话，再击一次出字；F8 语音改写（先选中文字）"
Write-Host "   首次启动会自动下载语音模型（241MB），托盘图标会提示进度"
Write-Host "   未签名 exe 被 SmartScreen 拦截属正常：点「更多信息 → 仍要运行」"
