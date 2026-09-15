#!/usr/bin/env python3
"""转写链路冒烟测试：不依赖热键与麦克风。

用 macOS 自带 `say` 生成一段中文语音，afconvert 转成 16kHz 单声道 wav，
再走完整「加载模型 → 转写 → 断言输出非空且大致包含所说内容」链路，
并分别报告模型加载耗时与单次转写耗时。
"""
import os
import subprocess
import sys
import tempfile
import wave

import numpy as np

_ROOT = os.path.dirname(os.path.abspath(__file__))
if _ROOT not in sys.path:
    sys.path.insert(0, _ROOT)

from xiaodao_ime.config import MODEL_PATH, SAMPLE_RATE  # noqa: E402
from xiaodao_ime.transcriber import Transcriber  # noqa: E402

# 说一句可断言的话；Tingting 为中文语音，缺失时回退英文
_ZH_SENTENCE = "今天天气很好我们一起去公园散步"
_ZH_KEYWORDS = ["公园", "天气", "散步"]
_EN_SENTENCE = "the quick brown fox jumps over the lazy dog"
_EN_KEYWORDS = ["fox", "dog", "quick"]


def _has_voice(name: str) -> bool:
    out = subprocess.run(["say", "-v", "?"], capture_output=True, text=True).stdout
    return name.lower() in out.lower()


def _synthesize() -> tuple[str, list[str], str]:
    """生成 16kHz 单声道 wav，返回 (wav 路径, 关键词, 所说文本)。"""
    tmpdir = tempfile.mkdtemp(prefix="xiaodao_test_")
    aiff = os.path.join(tmpdir, "speech.aiff")
    wav = os.path.join(tmpdir, "speech16k.wav")

    if _has_voice("Tingting"):
        sentence, keywords, voice = _ZH_SENTENCE, _ZH_KEYWORDS, "Tingting"
    else:
        sentence, keywords, voice = _EN_SENTENCE, _EN_KEYWORDS, None

    say_cmd = ["say", "-o", aiff]
    if voice:
        say_cmd += ["-v", voice]
    say_cmd += ["--", sentence]
    subprocess.run(say_cmd, check=True)

    # afconvert -> 16kHz 单声道 16-bit PCM wav
    subprocess.run(
        ["afconvert", "-f", "WAVE", "-d", f"LEI16@{SAMPLE_RATE}", "-c", "1", aiff, wav],
        check=True,
    )
    return wav, keywords, sentence


def _load_wav_float32(path: str) -> np.ndarray:
    with wave.open(path, "rb") as wf:
        assert wf.getframerate() == SAMPLE_RATE, f"采样率非 {SAMPLE_RATE}"
        assert wf.getnchannels() == 1, "非单声道"
        raw = wf.readframes(wf.getnframes())
    pcm_i16 = np.frombuffer(raw, dtype=np.int16)
    return (pcm_i16.astype(np.float32) / 32768.0).copy()


def main() -> int:
    if not os.path.isfile(MODEL_PATH):
        print(f"[FAIL] 模型文件缺失：{MODEL_PATH}")
        return 1

    print(f"[1/4] 合成语音（say + afconvert -> 16kHz mono wav）...")
    wav, keywords, sentence = _synthesize()
    pcm = _load_wav_float32(wav)
    print(f"      所说文本：{sentence}")
    print(f"      音频样本数：{len(pcm)}（约 {len(pcm)/SAMPLE_RATE:.2f}s）")

    print("[2/4] 加载模型（常驻内存）...")
    tr = Transcriber()
    load_s = tr.load()
    print(f"      模型加载耗时：{load_s*1000:.0f} ms")

    print("[3/4] 转写...")
    text, dt = tr.transcribe(pcm)
    print(f"      单次转写耗时：{dt*1000:.0f} ms")
    print(f"      转写结果：{text!r}")

    # 第二次转写，验证常驻复用后的稳态延迟
    text2, dt2 = tr.transcribe(pcm)
    print(f"      第二次转写耗时：{dt2*1000:.0f} ms")

    print("[4/4] 断言...")
    assert text and text.strip(), "转写结果为空"
    hit = [k for k in keywords if k in text]
    assert hit, f"转写结果未包含任何关键词 {keywords}，实际：{text!r}"
    print(f"      命中关键词：{hit}")

    tr.close()
    print("\n[PASS] 冒烟测试通过")
    print(f"  模型加载耗时 : {load_s*1000:.0f} ms")
    print(f"  首次转写耗时 : {dt*1000:.0f} ms")
    print(f"  稳态转写耗时 : {dt2*1000:.0f} ms")
    return 0


if __name__ == "__main__":
    sys.exit(main())
