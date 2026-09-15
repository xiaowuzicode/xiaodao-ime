# Rust + Tauri 2 重写方案（v1.0）

> 状态：进行中（分支 `feat/rust-tauri`）。目标：功能与 Python 版逐项对等后切为默认，Python 版移入 `legacy/`。

## 为什么能一把过

- 转写引擎零重写：transcribe.cpp 是纯 C ABI，官方 `transcribe-cpp` 0.2.3 crate 从源码 cmake 编译 ggml，
  本机 spike 已验证：现有 `models/SenseVoiceSmall-Q8_0.gguf` 直接加载，稳态 36ms / 3.6s 音频，与 Python 版一致。
- 其余全是成熟 crate：`rdev`（全局键钩，能分左右 Option/Ctrl）、`cpal`（录音）、`reqwest`（LLM）、
  `arboard`/`enigo`（剪贴板/按键注入）、Tauri 2（托盘 + 透明 HUD + 设置页）。

## 目录结构

```
rust/
├── Cargo.toml                 # workspace
├── crates/core/               # xiaodao-core：无 UI 依赖，可单测
│   └── src/
│       ├── types.rs           # Status/Channel/Permissions + Recorder/Transcribe/Polish/Hud/Events trait（架构边界）
│       ├── keys.rs            # HotkeyId / RecordMode / KeyEvent（settings 字符串 ↔ 类型）
│       ├── settings.rs        # settings.json（与 Python 版同 schema，深合并默认值）
│       ├── paths.rs           # 数据目录：mac ~/Library/Application Support/xiaodao-ime，win %APPDATA%\xiaodao-ime；XIAODAO_HOME 覆盖
│       ├── logging.rs         # tracing → logs/xiaodao-ime.log + stderr
│       ├── hotkey.rs          # 状态机（toggle/hold/双击锁定/防误触/预览调度）—— 1:1 移植 Python hotkey.py，测试移植 test_hotkey.py
│       ├── listener.rs        # rdev 钩子线程 → KeyEvent
│       ├── audio.rs           # CpalRecorder（16k mono f32，RMS level）
│       ├── transcriber.rs     # transcribe-cpp 封装，串行 run
│       ├── model_download.rs  # HF 直连 → hf-mirror 回退，进度回调
│       ├── polisher.rs        # OpenAI 兼容 / Anthropic Messages（原生 HTTP），fail-open
│       ├── context.rs         # app_styles 匹配
│       ├── history.rs         # history.jsonl + 统计
│       ├── paster.rs          # 粘贴 / 哨兵抓选区，泛化于 Platform trait，测试移植 test_paster.py
│       ├── hud.rs             # 纯函数辅助
│       └── platform/          # mod.rs Platform trait；common.rs arboard+enigo；mac.rs；win.rs
└── app/                       # Tauri 2 应用
    ├── package.json / vite.config.ts / index.html(hud) / settings.html
    ├── src/                   # 前端（vanilla TS，无框架）：hud.ts（计时+canvas 声浪+文本）、settings.ts
    └── src-tauri/
        ├── tauri.conf.json    # productName 小岛AI输入法, identifier ai.xiaodao.ime, 窗口 hud/settings
        ├── Info.plist         # NSMicrophoneUsageDescription, LSUIElement
        ├── icons/             # 由 resources/icon_1024.png 生成
        └── src/               # main.rs / tray.rs / hud.rs(实现 core Hud trait→emit) / commands.rs / bootstrap.rs
```

## 线程模型

```
rdev 钩子线程 ──KeyEvent──▶ HotkeyController(Mutex 状态机)
                               ├─ 预览线程：每 ~0.7s snapshot→transcribe(partial)→Hud::set_partial
                               ├─ 声浪线程：12Hz Recorder::level→Hud::set_level
                               └─ 收尾 worker：transcribe→replacements→(场景风格)→polish→paste→history
Tauri 主线程：托盘菜单 / 窗口；核心通过 Events/Hud trait 回调，Tauri 层 emit 事件给 webview。
```

## 行为对等清单（验收）

| 功能 | Python | Rust |
|---|---|---|
| toggle 单击开始/再击结束、组合键不误触 | ✅ | test_hotkey 移植 |
| hold 按住 + 0.35s 双击锁定、<0.4s 丢弃 | ✅ | test_hotkey 移植 |
| 录音中其他键取消（Esc）、另一热键取消 | ✅ | test_hotkey 移植 |
| 暂停热键总开关 | ✅ | |
| 伪流式预览（自适应间隔）+ 声浪 | ✅ | HUD webview |
| 转写 → replacements → app_styles → 润色（fail-open）→ 粘贴 → 历史 | ✅ | |
| 语音改写（哨兵抓选区 → LLM → 原地替换 → 恢复剪贴板） | ✅ | test_paster 移植 |
| 5 内置风格 + 自定义、hotwords | ✅ | |
| 托盘：状态图标 / 统计 / 最近历史复制 / 风格 / 热键 / 录音方式 / 暂停 / 设置 / 权限直达 / 日志 / 退出 | ✅ | |
| 设置窗口（热键 / 录音方式 / 润色 provider/key/model/风格） | mac only | 双平台（web） |
| 首启自动下模型（hf-mirror 回退） | ✅ | |
| 权限自检（IOHIDCheckAccess / AXIsProcessTrusted）+ 首键探针 | ✅ | |
| 打包：.app/.dmg（自签 XiaodaoIME Signing）、Windows NSIS | PyInstaller | tauri build |

## 关键决策

- **settings.json 同 schema、同位置**：老用户（PyInstaller 版）设置无缝继承。
- **HUD 必须不抢焦点**：macOS `set_activation_policy(Accessory)` + 窗口 `focusable:false` + `always_on_top` +
  `visible_on_all_workspaces` + `ignore_cursor_events`；实测若仍抢焦点则改用 `tauri-nspanel`（NonactivatingPanel）。
- **Windows 转写用 CPU 后端**（不引入 Vulkan SDK 依赖，安装零门槛）。
- **Anthropic provider 用原生 Messages API HTTP**，不引 SDK。
- **模型不入包**：首启下载到数据目录，与 Python 版同路径可直接复用已下载的模型。

## 阶段

1. core 各模块并行（A settings/paths/logging/polisher/context/history；B hotkey/listener/hud；C audio/transcriber/model_download；D platform/paster）
2. Tauri app：scaffold + 前端页面（与 1 并行）→ 接线 core（1 完成后）
3. 本机：`cargo test` 全绿 → `tauri build` 出 .app → 授权后真机 E2E；CI 双平台构建（macos-14 + windows-latest）
4. README/AGENTS/install 脚本切换；Python 版移 `legacy/`（2026-09-15 已完成，install.sh 仍装 legacy 版，待 Rust Release 后切换）
