# 小岛AI输入法

本地离线语音输入工具（macOS / Apple Silicon）。**按住左 Option 键说话，松手把转写文字粘贴进当前光标所在的任意输入框。** 全程本地推理，不联网、不上传语音。

- 转写引擎：[transcribe.cpp](https://github.com/handy-computer/transcribe.cpp)（ggml，Metal 加速）
- 模型：SenseVoice Small（中文最强、自带标点、多语言 zh/yue/en/ja/ko），常驻内存
- 实测（Apple M4 Pro）：模型加载 ~0.2s；一段 3.5s 语音稳态转写 ~40ms，松手到出字远快于 1s 目标

---

## 架构

```
app.py（rumps 菜单栏，主线程）
  └─ xiaodao_ime/
       ├─ config.py       路径与常量
       ├─ settings.py     用户设置（settings.json 读写、默认值深合并）
       ├─ logger.py       日志（logs/xiaodao-ime.log，带时间戳/文件名行号）
       ├─ transcriber.py  SenseVoice GGUF 常驻内存，封装 transcribe.cpp
       ├─ recorder.py     sounddevice 录音（16kHz 单声道 float32）
       ├─ polisher.py     可选 LLM 润色（OpenAI 兼容 / Anthropic，fail-open）
       ├─ paster.py       NSPasteboard 存/取 + Quartz CGEvent 模拟 Cmd+V
       └─ hotkey.py       pynput 全局监听 + 按住说话状态机（含防误触）
```

线程模型：`rumps` 跑主线程；`pynput` 监听、`sounddevice` 录音回调、转写/润色各跑子线程，互不阻塞。

菜单栏图标状态：🏝️ 待机 / 🎙️ 录音中 / ✍️ 转写中 / 🪄 润色中。菜单项：「设置」（热键、AI 润色、配置文件）「打开日志」「退出」。

### 交互与防误触规则
- **按住说话**：按住热键（默认左 Option，可在「设置 → 热键」切换为右 Option / 右 Command / F19）录音，松开即转写、（可选）润色、粘贴。
- **双击锁定**（长段口述）：0.35s 内连击两下热键进入锁定录音，无需一直按着；再按一下热键结束并出字。
- 录音期间（按住或锁定）若按下**任何其他键**（说明你在用组合快捷键/开始打字）→ 立即取消本次录音，不转写不粘贴。
- 录音开始/结束/取消有系统提示音（`settings.json` 的 `sounds` 可关）。
- 按住时长 **< 0.4s** → 直接丢弃（防误碰）。
- 转写结果为空/纯空白 → 不粘贴。
- 粘贴方式：先保存当前剪贴板 → 写入转写文字 → Cmd+V → ~0.4s 后恢复原剪贴板，尽量不破坏你的复制内容。

---

## 安装

```bash
cd /Users/chengang/xiaodao-ime
python3 -m venv .venv
.venv/bin/pip install -r requirements.txt
```

### 下载模型

模型文件不入库（241MB），需单独下载到 `models/`：

```bash
# 直连 HuggingFace
.venv/bin/python -c "from huggingface_hub import hf_hub_download; \
hf_hub_download('handy-computer/SenseVoiceSmall-gguf','SenseVoiceSmall-Q8_0.gguf', local_dir='models')"

# 国内网络慢/失败时用镜像：
HF_ENDPOINT=https://hf-mirror.com .venv/bin/python -c "..."   # 同上
```

> 仓库提供多种量化：`Q4_K_M / Q5_K_M / Q6_K / Q8_0 / F16 / F32`。默认用 **Q8_0**（近无损、速度快）。想换其他量化，下载后设环境变量 `XIAODAO_MODEL=SenseVoiceSmall-F16.gguf` 即可。

---

## 使用

```bash
./start.sh          # 激活 venv 并启动，菜单栏出现 🏝️ 图标
```

启动后按住左 Option 说话、松手即出字。日志在 `logs/xiaodao-ime.log`，也可点菜单「打开日志」。

### 冒烟测试（不依赖热键/麦克风）

```bash
.venv/bin/python test_transcribe.py   # say 合成语音 → 加载模型 → 转写 → 断言
.venv/bin/python test_polish.py       # 设置/润色模块离线测试（不联网）
```

---

## 设置与 AI 润色

设置存在项目根目录 `settings.json`（首次点「设置 → 打开配置文件」自动生成，不入 git），完整示例见 `settings.example.json`。改完文件后点「设置 → 重新加载配置」即可生效，热键和润色开关也可直接在菜单里切。

### AI 润色（可选，默认关闭）

开启后，转写文本会过一遍大模型：去口水词（嗯/那个/就是说）、修正同音错字、规范标点。**Key 用你自己的大模型 API Key**，两种 provider：

| provider | 适用 | 配置 |
|---|---|---|
| `openai`（默认） | DeepSeek / Kimi / GLM / OpenAI / ollama 本地模型等一切 OpenAI 兼容端点 | `base_url` + `api_key` + `model` |
| `anthropic` | Anthropic API | `api_key`（或环境变量 `ANTHROPIC_API_KEY`），需 `pip install anthropic` |

常用 `base_url`（填到 `/chat/completions` 的上一级）：

- DeepSeek：`https://api.deepseek.com`（model `deepseek-chat`）
- Kimi：`https://api.moonshot.cn/v1`（model `moonshot-v1-8k` 等）
- 智谱 GLM：`https://open.bigmodel.cn/api/paas/v4`（model `glm-4-flash` 等）
- OpenAI：`https://api.openai.com/v1`
- ollama 本地：`http://localhost:11434/v1`（无需 key，全离线）

润色是 **fail-open** 的：API 超时/报错/被拒一律回退原始转写，绝不因为润色挂了导致不出字。

### 润色风格（设置 → 润色风格）

内置五种：**润色**（默认，去口水词+纠错）/ **书面化** / **轻度纠错** / **翻译成英文**（说中文出英文）/ **会议纪要**（口述转要点列表）。
在 `settings.json` 的 `polish.styles` 里可自定义新风格或覆盖内置，格式 `{"风格名": "system 提示词"}`，菜单会自动出现。

### 输入历史

每次成功出字都记录到本地 `data/history.jsonl`（原始转写 + 最终文本），菜单「历史」显示最近 10 条，点击即复制。`settings.json` 的 `history.enabled` 可关。

### 热词与离线替换

- `hotwords`：人名/产品名/术语列表，注入润色提示词，辅助大模型纠正同音错字（如"欧朵"→"Ordo"）；
- `replacements`：离线字符串替换表，转写后无条件执行、零延迟、不依赖 LLM，适合 100% 确定的固定纠错。

路线图与豆包输入法对标见 [ROADMAP.md](ROADMAP.md)。

---

## ⚠️ 必须手动授予的系统权限

macOS 会拦截全局按键监听、模拟按键、麦克风。请在**系统设置 → 隐私与安全性**中，为**启动本程序的那个 App**（用 `./start.sh` 从终端启动就是你的终端 App，如 Terminal / iTerm；用 launchd 启动则是 `.venv/bin/python`）授予：

| 权限项 | 位置 | 用途 |
|---|---|---|
| **输入监听 Input Monitoring** | 隐私与安全性 → 输入监听 | pynput 监听左 Option 全局热键 |
| **辅助功能 Accessibility** | 隐私与安全性 → 辅助功能 | Quartz 模拟 Cmd+V 粘贴 |
| **麦克风 Microphone** | 隐私与安全性 → 麦克风 | 录音 |

未授权时：程序**不会崩溃**，而是在启动输出和日志里给出明确授权指引；热键收不到事件或录音报错都会被捕获并记录。授权后请重启本程序（部分权限需重启进程生效）。

> 提示：若换了终端 App 或用 launchd，需要重新对新的宿主程序授权。

---

## 打包成独立 App（推荐）

```bash
scripts/make_app.sh    # 生成 dist/小岛AI输入法.app
```

生成后可拖进「应用程序」。**权限从此授予「小岛AI输入法」App 本身**（不再依赖终端），双击即用，也可加进「系统设置 → 通用 → 登录项」开机自启。首次启动按提示在隐私与安全性里授予三项权限即可。

> 注意：App 内部仍引用本项目目录的 venv 和代码，移动/删除项目目录后需重新打包。

---

## 开机自启（可选，launchd）

`launchd/ai.xiaodao.ime.plist` 已备好但**不会自动加载**。手动安装：

```bash
cp launchd/ai.xiaodao.ime.plist ~/Library/LaunchAgents/
launchctl load ~/Library/LaunchAgents/ai.xiaodao.ime.plist
# 卸载：
launchctl unload ~/Library/LaunchAgents/ai.xiaodao.ime.plist
```

注意：launchd 启动时宿主是 `.venv/bin/python`，需按上表给它单独授予三项权限。

---

## 技术选型说明

- **采用路线 A**：PyPI 的 `transcribe-cpp`（0.1.3）自带 Apple Silicon 原生 wheel（`transcribe_cpp_native-*-macosx_11_0_arm64.whl`），开箱即用、Metal 加速，无需自行 cmake 编译，故未走路线 B（源码编译 + ctypes）或路线 C（subprocess 兜底）。
- 模型常驻：进程启动时 `transcribe_cpp.Model(path)` 加载一次；每次转写复用同一 Model 建 session 运行（模型限制同一时刻仅一个 run，已用锁串行化）。
