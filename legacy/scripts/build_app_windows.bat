@echo off
REM 用 PyInstaller 打 Windows 发布版（自包含，无需 Python 环境）。
REM 用法：在仓库根目录执行 scripts\build_app_windows.bat
REM 产物：dist\XiaodaoIME\XiaodaoIME.exe（整个目录拷走即可运行）
REM
REM 说明：
REM   - 未签名的 exe 可能被 SmartScreen/杀软提示，属正常现象（点「仍要运行」）；
REM   - 首次启动自动下载模型（241MB）到 %%APPDATA%%\xiaodao-ime；
REM   - Windows 无 macOS 式权限授权，开箱即用。

pip show pyinstaller >nul 2>&1 || pip install pyinstaller

pyinstaller --noconfirm --clean --windowed ^
  --name XiaodaoIME ^
  --collect-all transcribe_cpp ^
  --collect-all transcribe_cpp_native ^
  main.py

echo.
echo 完成：dist\XiaodaoIME\XiaodaoIME.exe
