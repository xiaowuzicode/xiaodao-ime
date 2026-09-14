//! 端到端转写冒烟：macOS `say` 合成中文语音 → ffmpeg 转 16kHz 单声道 f32 → [`Transcriber`]。
//!
//! 依赖本机模型与 say/ffmpeg，默认不跑：
//! `cargo test -p xiaodao-core --test transcribe_e2e -- --ignored --nocapture`
//! 模型路径默认取 `~/xiaodao-ime/models/SenseVoiceSmall-Q8_0.gguf`，可用
//! `XIAODAO_MODEL_PATH` 覆盖。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use xiaodao_core::transcriber::Transcriber;
use xiaodao_core::types::Transcribe;

/// 合成的话术：断言转写结果里必须出现「输入法」。
const SPOKEN: &str = "欢迎使用小岛AI输入法，语音输入又快又准。";
const CHINESE_VOICES: [&str; 4] = ["Tingting", "Meijia", "Eddy", "Flo"];

fn model_path() -> PathBuf {
    if let Ok(p) = std::env::var("XIAODAO_MODEL_PATH") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").expect("读不到 HOME");
    PathBuf::from(home).join("xiaodao-ime/models/SenseVoiceSmall-Q8_0.gguf")
}

/// 用 say 合成 aiff，再用 ffmpeg 转成 16kHz 单声道 f32 裸流读进内存。
fn synth_pcm(dir: &Path) -> Vec<f32> {
    let aiff = dir.join("say.aiff");
    let raw = dir.join("say.f32le");

    let mut spoken = false;
    for voice in CHINESE_VOICES {
        let ok = Command::new("say")
            .args(["-v", voice, "-o"])
            .arg(&aiff)
            .arg(SPOKEN)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            println!("合成语音：voice={voice}");
            spoken = true;
            break;
        }
    }
    assert!(spoken, "say 合成失败（没有可用的中文语音？）");

    let status = Command::new("ffmpeg")
        .arg("-y")
        .arg("-i")
        .arg(&aiff)
        .args(["-ar", "16000", "-ac", "1", "-f", "f32le"])
        .arg(&raw)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("ffmpeg 未安装？");
    assert!(status.success(), "ffmpeg 转码失败");

    let bytes = std::fs::read(&raw).expect("读取裸流失败");
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

#[test]
#[ignore = "需要本机模型与 say/ffmpeg"]
fn transcribe_says_input_method() {
    let model = model_path();
    assert!(model.is_file(), "模型不存在：{}", model.display());

    let dir = tempfile::tempdir().expect("建临时目录失败");
    let pcm = synth_pcm(dir.path());
    let seconds = pcm.len() as f64 / 16_000.0;
    println!("音频：样本数={}，时长={seconds:.2}s", pcm.len());

    let transcriber = Transcriber::load(&model, Some("zh".to_string())).expect("加载模型失败");
    println!("模型加载耗时：{:.3}s", transcriber.load_seconds());

    // 第一次含预热，跑三次看稳态
    let mut text = String::new();
    for round in 1..=3 {
        let t0 = Instant::now();
        text = transcriber.transcribe(&pcm, false).expect("转写失败");
        println!(
            "第 {round} 次：耗时 {:.3}s，文本={text:?}",
            t0.elapsed().as_secs_f64()
        );
    }
    assert!(
        text.contains("输入法"),
        "转写结果里没有「输入法」：{text:?}"
    );

    // 空 PCM 返回空串
    assert_eq!(transcriber.transcribe(&[], false).unwrap(), "");
    // partial 预览路径同样能出字
    let partial = transcriber
        .transcribe(&pcm[..pcm.len() / 2], true)
        .expect("预览转写失败");
    println!("预览（前半段）：{partial:?}");
}
