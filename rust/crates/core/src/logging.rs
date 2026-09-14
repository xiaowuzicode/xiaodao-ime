//! 日志初始化（对应 Python `xiaodao_ime/logger.py`）。
//!
//! 双输出：`logs/xiaodao-ime.log`（追加，不滚动）+ stderr；
//! 格式与 Python 版对齐：`2026-09-14 10:30:00 [INFO] settings.rs:120 - 已加载设置：…`。
//!
//! 过滤级别：`RUST_LOG` 环境变量优先（标准 env-filter 语法），否则 `verbose` 决定 debug/info。
//! 返回的 guard 必须由 main 持有到进程结束，否则非阻塞写线程会提前退出、日志丢尾。

use std::fmt;

use anyhow::{Context, Result};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// 本地时间 `%Y-%m-%d %H:%M:%S`（与 Python logging 的 datefmt 一致）。
struct LocalTime;

impl FormatTime for LocalTime {
    fn format_time(&self, w: &mut Writer<'_>) -> fmt::Result {
        write!(w, "{}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"))
    }
}

/// 日志文件名（放在 `paths.logs_dir` 下）。
const LOG_FILENAME: &str = "xiaodao-ime.log";

/// 初始化全局 tracing subscriber。整个进程只应调用一次；重复调用返回 Err。
pub fn init(paths: &crate::paths::Paths, verbose: bool) -> Result<WorkerGuard> {
    std::fs::create_dir_all(&paths.logs_dir)
        .with_context(|| format!("创建日志目录失败：{}", paths.logs_dir.display()))?;

    // never = 不滚动、追加写；滚动交给外部（日志体量很小）。
    let appender = tracing_appender::rolling::never(&paths.logs_dir, LOG_FILENAME);
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let default_level = if verbose { "debug" } else { "info" };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(format!("xiaodao_core={default_level},{default_level}"))
    });

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(writer)
        .with_ansi(false)
        .with_target(false)
        .with_file(true)
        .with_line_number(true)
        .with_timer(LocalTime);

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_target(false)
        .with_file(true)
        .with_line_number(true)
        .with_timer(LocalTime);

    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .try_init()
        .map_err(|e| anyhow::anyhow!("日志初始化失败（可能已初始化过）：{e}"))?;

    tracing::info!("日志文件：{}", paths.log_file.display());
    Ok(guard)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只验证「不 panic + 日志文件被创建」；全局 subscriber 只能装一次，
    /// 所以这里允许 init 因为其它测试先装过而失败。
    #[test]
    fn init_creates_log_file() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::with_base(tmp.path()).unwrap();
        if let Ok(guard) = init(&paths, true) {
            tracing::info!("测试日志");
            drop(guard); // flush
            assert!(paths.log_file.is_file());
            let text = std::fs::read_to_string(&paths.log_file).unwrap();
            assert!(text.contains("测试日志"));
            assert!(text.contains("logging.rs:"), "应带文件名行号：{text}");
        }
    }
}
