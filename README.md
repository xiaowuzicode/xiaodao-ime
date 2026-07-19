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
       ├─ logger.py       日志（logs/xiaodao-ime.log，带时间戳/文件名行号）
       ├─ transcriber.py  SenseVoice GGUF 常驻内存，封装 transcribe.cpp
       ├─ recorder.py     sounddevice 录音（16kHz 单声道 float32）
       ├─ paster.py       NSPasteboard 存/取 + Quartz CGEvent 模拟 Cmd+V
       └─ hotkey.py       pynput 全局监听 + 按住说话状态机（含防误触）
```

线程模型：`rumps` 跑主线程；`pynput` 监听、`sounddevice` 录音回调、转写各跑子线程，互不阻塞。

菜单栏图标状态：🏝️ 待机 / 🎙️ 录音中 / ✍️ 转写中。菜单项：「打开日志」「退出」。

### 交互与防误触规则
- 按住**左 Option**（`pynput.Key.alt_l`）开始录音，松开停止并转写、粘贴。
- 按住左 Option 期间若按下**任何其他键**（说明你在用 ⌥+x 系统快捷键）→ 立即取消本次录音，不转写不粘贴。
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

用系统 `say` 合成语音，跑完整「加载模型 → 转写 → 断言」链路：

```bash
.venv/bin/python test_transcribe.py
```

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
