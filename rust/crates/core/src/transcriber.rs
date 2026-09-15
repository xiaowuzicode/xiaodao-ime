//! transcribe.cpp + SenseVoice 常驻转写器，实现 [`crate::types::Transcribe`]。
//!
//! 移植自 Python 版 `xiaodao_ime/transcriber.py`：模型进程内加载一次并常驻，
//! 每次松手复用同一个 session 跑一次 run，命中「松手到出字 1 秒内」的目标。
//!
//! 线程安全：`transcribe.cpp` 同一时刻只允许一个 run，这里用 `Mutex<Session>` 串行化；
//! 预览（partial=true）与收尾转写共用这把锁，先到先得。
//!
//! 原生日志：ggml / transcribe.cpp 默认往 stderr 刷日志，会污染终端。默认调用
//! `disable_logging()` 静音；设 `XIAODAO_NATIVE_LOG=1` 时改成 `init_logging()`，
//! 原生日志走 `log` facade，由 tracing-subscriber 的 log 桥接落进正常日志文件。

use std::path::Path;
use std::sync::Once;
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use parking_lot::Mutex;
use tracing::{debug, info, warn};
use transcribe_cpp::{Model, RunOptions, Session};

use crate::types::Transcribe;

static NATIVE_LOG_INIT: Once = Once::new();

/// 安装原生日志开关（进程内只能设一次，必须在加载模型之前）。
fn init_native_logging() {
    NATIVE_LOG_INIT.call_once(|| {
        let verbose = std::env::var("XIAODAO_NATIVE_LOG")
            .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes"))
            .unwrap_or(false);
        if verbose {
            transcribe_cpp::init_logging();
            debug!("原生日志已路由到 log/tracing");
        } else {
            transcribe_cpp::disable_logging();
        }
    });
}

/// 常驻内存的转写器。
pub struct Transcriber {
    session: Mutex<Session>,
    language: Option<String>,
    load_seconds: f64,
}

impl std::fmt::Debug for Transcriber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transcriber")
            .field("language", &self.language)
            .field("load_seconds", &self.load_seconds)
            .finish()
    }
}

impl Transcriber {
    /// 加载并常驻模型。`language` 为 `None` 时让模型自动判定语种。
    pub fn load(model_path: &Path, language: Option<String>) -> Result<Self> {
        if !model_path.is_file() {
            return Err(anyhow!("模型文件不存在：{}", model_path.display()));
        }
        init_native_logging();

        let started = Instant::now();
        info!(
            "开始加载模型：{}（引擎 transcribe.cpp {}）",
            model_path.display(),
            transcribe_cpp::version()
        );
        let model = Model::load(model_path)
            .map_err(|e| anyhow!("加载模型失败（{}）：{e}", model_path.display()))?;
        let session = model.session().context("创建转写 session 失败")?;
        let load_seconds = started.elapsed().as_secs_f64();

        let device = match model.device() {
            Ok(d) => format!("{}（{}，{}）", d.name, d.description, d.kind),
            Err(e) => {
                warn!("读取推理设备信息失败：{e}");
                "未知".to_string()
            }
        };
        info!(
            "模型加载完成，耗时 {load_seconds:.3}s，后端={}，设备={device}，语种={}",
            model.backend(),
            language.as_deref().unwrap_or("自动")
        );
        // model 可以就地 drop：session 内部持有 Arc，模型会活到最后一个引用释放
        Ok(Self {
            session: Mutex::new(session),
            language,
            load_seconds,
        })
    }

    /// 模型加载耗时（秒）。
    pub fn load_seconds(&self) -> f64 {
        self.load_seconds
    }

    /// 生效的语种提示（`None` = 自动判定）。
    pub fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }
}

impl Transcribe for Transcriber {
    fn transcribe(&self, pcm: &[f32], partial: bool) -> Result<String> {
        if pcm.is_empty() {
            return Ok(String::new());
        }
        let options = RunOptions {
            language: self.language.clone(),
            ..Default::default()
        };
        let started = Instant::now();
        let transcript = {
            // 锁只包住原生 run，保证同一时刻只有一个 run 在跑
            let mut session = self.session.lock();
            session
                .run(pcm, &options)
                .map_err(|e| anyhow!("转写失败：{e}"))?
        };
        let elapsed = started.elapsed().as_secs_f64();
        let text = transcript.text.trim().to_string();
        if partial {
            debug!("预览转写：耗时 {elapsed:.3}s，样本数={}", pcm.len());
        } else {
            info!(
                "转写完成，耗时 {elapsed:.3}s，样本数={}，文本={text:?}",
                pcm.len()
            );
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 模型路径不存在时给出中文错误，而不是 panic。
    #[test]
    fn load_missing_model_reports_error() {
        let err = Transcriber::load(Path::new("/definitely/not/here.gguf"), None).unwrap_err();
        assert!(err.to_string().contains("模型文件不存在"), "{err}");
    }

    /// 引擎版本可读（同时确认动态库链接正常）。
    #[test]
    fn engine_version_is_readable() {
        assert!(!transcribe_cpp::version().is_empty());
    }
}
