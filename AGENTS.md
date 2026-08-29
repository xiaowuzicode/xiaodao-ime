# AGENTS.md — 给 AI 编码助手的操作说明（Codex / Claude Code / Cursor …）

这是一个本地离线语音输入工具：按热键说话、松手出字，语音识别全在本机跑。
用户把你带到这个仓库，多半是想让你**帮他装上并跑起来**，或帮他改代码。

## 帮用户安装（最常见任务）

### macOS

一条命令完成 clone → venv → 依赖 → 模型 → 打包 .app → 装入 /Applications 并启动：

```bash
curl -fsSL https://raw.githubusercontent.com/xiaowuzicode/xiaodao-ime/main/install.sh | bash
```

已 clone 的话在仓库根目录 `bash install.sh` 即可（就地安装，幂等，可重复执行）。

**装完必须提醒用户手动做一步（你无法代劳）**：
系统设置 → 隐私与安全性，为「小岛AI输入法」开启 **输入监听 / 辅助功能 / 麦克风**，
然后退出重开 App。菜单栏出现声浪图标 ·ıllı· 即就绪：单击左 Option 说话，再击出字。

常见问题：

- 模型下载慢/失败：`HF_ENDPOINT=https://hf-mirror.com bash install.sh`（install.sh 失败时也会自动切镜像重试一次）。
- 热键没反应：九成是权限没授或授给了错误宿主（终端启动时权限挂在终端上，App 方式挂在 App 上）。App 内有权限自检，看日志 `logs/xiaodao-ime.log`。
- 重新打包后权限失效：TCC 认签名不认路径，重打包须在系统设置里先「−」移除旧条目再重新添加。**改 Python 代码不需要重打包**——.app 只是启动器，实时读项目目录，重开 App 即生效。

### Windows（beta）

无需系统授权。一条命令（PowerShell）：

```powershell
irm https://raw.githubusercontent.com/xiaowuzicode/xiaodao-ime/main/install.ps1 | iex
```

自动下载最新 Release 的 `XiaodaoIME-windows-x64.zip` 装到 `%LOCALAPPDATA%\Programs\XiaodaoIME` 并启动。备选：Releases 页手动下载解压，或 `pip install -r requirements.txt` 后跑 `scripts\build_app_windows.bat` 本地构建。

默认热键：右 Ctrl 听写、F8 改写。首次启动自动下载模型（241MB）。SmartScreen 提示属正常（未签名 exe）。

## 运行与测试

```bash
./start.sh                  # 终端直跑（调试用；注意权限会挂在终端上）
python test_hotkey.py       # 热键状态机（无需权限，CI 双平台跑）
python test_paster.py       # 选区/粘贴/HUD 流程
python test_polish.py       # 润色（无 key 时走 mock）
python test_transcribe.py   # say 合成语音端到端（仅 Mac，需模型）
```

## 改代码的纪律

- **分层铁律**：`xiaodao_ime/` 核心层平台无关，禁止直接 import AppKit/Quartz/ctypes/winsound 等平台库；所有系统 API 调用只能进 `xiaodao_ime/platform/mac.py` 或 `win.py`（接口约定见 `platform/__init__.py`）。改核心逻辑时两个平台都要能跑。
- 用户配置在项目根 `settings.json`（gitignore 内），示例见 `settings.example.json`；改配置结构要同步两处。
- 润色链路必须 **fail-open**：LLM 超时/报错一律回退原始转写文本，绝不能吞掉用户说的话。
- 提交信息用中文，风格参考 `git log`（`feat:` / `fix:` / `docs:` / `build:` 前缀）。
