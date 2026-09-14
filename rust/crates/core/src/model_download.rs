//! 首次运行自动下载 SenseVoice gguf：HF 直连 → hf-mirror 回退，带进度回调。
//!
//! 移植自 Python 版 `app.py::_download_model`（huggingface_hub）与 `install.sh` 的镜像策略：
//! 直连 huggingface.co 失败就换 hf-mirror.com；设了 `HF_ENDPOINT` 则该站点优先。
//! 下载先落 `*.part`，成功后原子 rename，中途失败清理临时文件，不会留半截模型。

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use tracing::{info, warn};

/// 认为「模型已下载好」的最小体积；比这小多半是半截文件或错误页。
const MIN_MODEL_BYTES: u64 = 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// reqwest blocking 的 `timeout` 作用在「每次 read」上而不是整个请求，
/// 所以 241MB 的大文件可以安全地拿它当「卡住 60s 就判失败」的护栏。
const STALL_TIMEOUT: Duration = Duration::from_secs(60);
const READ_BUF_BYTES: usize = 64 * 1024;
/// 进度回调节流：满 1MB 或满 1% 才回调一次。
const PROGRESS_MIN_BYTES: u64 = 1024 * 1024;

pub const DEFAULT_REPO: &str = "handy-computer/SenseVoiceSmall-gguf";
pub const DEFAULT_FILENAME: &str = "SenseVoiceSmall-Q8_0.gguf";
const HF_OFFICIAL: &str = "https://huggingface.co";
const HF_MIRROR: &str = "https://hf-mirror.com";

/// 下载进度。`total` 为 `None` 表示服务端没给 Content-Length。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub downloaded: u64,
    pub total: Option<u64>,
    pub source: String,
}

impl Progress {
    /// 已下载百分比（无总长时为 `None`）。
    pub fn percent(&self) -> Option<f64> {
        self.total
            .filter(|t| *t > 0)
            .map(|t| self.downloaded as f64 * 100.0 / t as f64)
    }
}

/// 确保模型就位：已存在且体积正常直接返回；否则按候选站点顺序下载。
pub fn ensure_model(
    model_path: &Path,
    repo: &str,
    filename: &str,
    progress: &dyn Fn(Progress),
) -> Result<()> {
    if let Ok(meta) = fs::metadata(model_path) {
        if meta.is_file() && meta.len() > MIN_MODEL_BYTES {
            info!(
                "模型已存在，跳过下载：{}（{:.1}MB）",
                model_path.display(),
                meta.len() as f64 / 1024.0 / 1024.0
            );
            return Ok(());
        }
        warn!(
            "模型文件不完整（{} 字节），将重新下载：{}",
            meta.len(),
            model_path.display()
        );
    }
    let urls: Vec<String> = endpoints(std::env::var("HF_ENDPOINT").ok().as_deref())
        .iter()
        .map(|base| {
            format!(
                "{}/{repo}/resolve/main/{filename}",
                base.trim_end_matches('/')
            )
        })
        .collect();
    download_with_fallback(model_path, &urls, progress)
}

/// 候选站点：`HF_ENDPOINT`（若设置）优先，然后官方直连，最后国内镜像。
fn endpoints(hf_endpoint: Option<&str>) -> Vec<String> {
    let mut list: Vec<String> = Vec::new();
    if let Some(custom) = hf_endpoint.map(str::trim).filter(|v| !v.is_empty()) {
        list.push(custom.to_string());
    }
    for default in [HF_OFFICIAL, HF_MIRROR] {
        let dup = list
            .iter()
            .any(|x| x.trim_end_matches('/') == default.trim_end_matches('/'));
        if !dup {
            list.push(default.to_string());
        }
    }
    list
}

/// 依次尝试各源，单源失败 warn 后换下一个；全部失败返回最后一个错误。
fn download_with_fallback(dest: &Path, urls: &[String], progress: &dyn Fn(Progress)) -> Result<()> {
    let mut last_err: Option<anyhow::Error> = None;
    for url in urls {
        info!("开始下载模型：{url}");
        match download_one(url, dest, progress) {
            Ok(bytes) => {
                info!(
                    "模型下载完成：{}（{:.1}MB，来源 {url}）",
                    dest.display(),
                    bytes as f64 / 1024.0 / 1024.0
                );
                return Ok(());
            }
            Err(e) => {
                warn!("下载失败（{url}）：{e:#}");
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("没有可用的下载源")))
}

/// 单源下载：流式写 `*.part`，成功后 rename；失败清理临时文件。
fn download_one(url: &str, dest: &Path, progress: &dyn Fn(Progress)) -> Result<u64> {
    if let Some(dir) = dest.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).with_context(|| format!("创建目录失败：{}", dir.display()))?;
    }
    let part = part_path(dest);
    let result = stream_to_file(url, &part, progress);
    if result.is_err() {
        let _ = fs::remove_file(&part);
    }
    let downloaded = result?;
    fs::rename(&part, dest)
        .with_context(|| format!("重命名失败：{} → {}", part.display(), dest.display()))?;
    Ok(downloaded)
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = OsString::from(dest.as_os_str());
    name.push(".part");
    PathBuf::from(name)
}

fn stream_to_file(url: &str, part: &Path, progress: &dyn Fn(Progress)) -> Result<u64> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(STALL_TIMEOUT)
        .user_agent("xiaodao-ime")
        .build()
        .context("创建 HTTP 客户端失败")?;
    let mut response = client
        .get(url)
        .send()
        .with_context(|| format!("请求失败：{url}"))?
        .error_for_status()
        .with_context(|| format!("服务端返回错误状态：{url}"))?;
    let total = response.content_length();

    let mut file =
        File::create(part).with_context(|| format!("创建临时文件失败：{}", part.display()))?;
    let mut buf = vec![0u8; READ_BUF_BYTES];
    let (mut downloaded, mut reported) = (0u64, 0u64);
    loop {
        let n = response
            .read(&mut buf)
            .with_context(|| format!("读取响应失败（已下载 {downloaded} 字节）：{url}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).context("写入临时文件失败")?;
        downloaded += n as u64;
        if should_report(downloaded, reported, total) {
            reported = downloaded;
            progress(Progress {
                downloaded,
                total,
                source: url.to_string(),
            });
        }
    }
    file.flush().context("刷新临时文件失败")?;
    drop(file);

    if downloaded < MIN_MODEL_BYTES {
        return Err(anyhow!(
            "下载内容过小（{downloaded} 字节），疑似错误页而非模型文件"
        ));
    }
    if let Some(expected) = total.filter(|t| *t != downloaded) {
        return Err(anyhow!("下载不完整：{downloaded}/{expected} 字节"));
    }
    if reported != downloaded {
        progress(Progress {
            downloaded,
            total,
            source: url.to_string(),
        });
    }
    Ok(downloaded)
}

/// 满 1MB 或满 1% 就回调一次。
fn should_report(downloaded: u64, reported: u64, total: Option<u64>) -> bool {
    let delta = downloaded - reported;
    delta >= PROGRESS_MIN_BYTES || total.is_some_and(|t| t > 0 && delta >= t / 100)
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;

    fn body(bytes: usize) -> Vec<u8> {
        (0..bytes).map(|i| (i % 251) as u8).collect()
    }

    fn collector() -> Mutex<Vec<Progress>> {
        Mutex::new(Vec::new())
    }

    /// 候选站点顺序：HF_ENDPOINT 优先且不与默认源重复。
    #[test]
    fn endpoints_order() {
        assert_eq!(endpoints(None), vec![HF_OFFICIAL, HF_MIRROR]);
        assert_eq!(endpoints(Some("  ")), vec![HF_OFFICIAL, HF_MIRROR]);
        assert_eq!(
            endpoints(Some("https://hf.internal")),
            vec!["https://hf.internal", HF_OFFICIAL, HF_MIRROR]
        );
        // 与默认源相同（带不带尾斜杠都算）时不重复排队
        assert_eq!(
            endpoints(Some("https://hf-mirror.com/")),
            vec!["https://hf-mirror.com/", HF_OFFICIAL]
        );
    }

    #[test]
    fn report_throttle() {
        assert!(!should_report(100, 0, Some(1_000_000_000)));
        assert!(should_report(PROGRESS_MIN_BYTES, 0, None));
        assert!(should_report(10, 0, Some(1_000)), "满 1% 也要回调");
    }

    /// 已存在且体积正常 → 直接返回，不发请求、不回调。
    #[test]
    fn skips_existing_model() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.gguf");
        fs::write(&dest, body(2 * 1024 * 1024)).unwrap();
        let seen = collector();
        ensure_model(&dest, "repo/name", "model.gguf", &|p| seen.lock().push(p)).unwrap();
        assert!(seen.lock().is_empty(), "不该触发下载进度回调");
    }

    /// 首源 500 → 换第二源成功；`.part` 不残留，进度回调带来源。
    #[test]
    fn falls_back_to_second_source() {
        let payload = body(2 * 1024 * 1024);
        let mut bad = mockito::Server::new();
        let bad_mock = bad.mock("GET", "/model.gguf").with_status(500).create();
        let mut good = mockito::Server::new();
        let good_mock = good
            .mock("GET", "/model.gguf")
            .with_status(200)
            .with_body(payload.clone())
            .create();

        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("sub").join("model.gguf");
        let seen = collector();
        let urls = vec![
            format!("{}/model.gguf", bad.url()),
            format!("{}/model.gguf", good.url()),
        ];
        download_with_fallback(&dest, &urls, &|p| seen.lock().push(p)).unwrap();

        bad_mock.assert();
        good_mock.assert();
        assert_eq!(fs::read(&dest).unwrap(), payload);
        assert!(!part_path(&dest).exists(), ".part 应已 rename 掉");
        let seen = seen.lock();
        let last = seen.last().expect("至少回调一次");
        assert_eq!(last.downloaded, payload.len() as u64);
        assert_eq!(last.total, Some(payload.len() as u64));
        assert!(last.source.starts_with(&good.url()));
        assert_eq!(last.percent(), Some(100.0));
    }

    /// 全部源失败 → 返回 Err，不留 `.part`，也不留半截目标文件。
    #[test]
    fn all_sources_fail() {
        let mut server = mockito::Server::new();
        let _m = server.mock("GET", "/model.gguf").with_status(404).create();
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.gguf");
        let urls = vec![format!("{}/model.gguf", server.url())];
        let err = download_with_fallback(&dest, &urls, &|_| {}).unwrap_err();
        assert!(err.to_string().contains("服务端返回错误状态"), "{err:#}");
        assert!(!dest.exists() && !part_path(&dest).exists());
    }

    /// 返回内容过小（错误页）→ 判失败并清理。
    #[test]
    fn rejects_tiny_body() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/model.gguf")
            .with_status(200)
            .with_body("<html>404</html>")
            .create();
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.gguf");
        let urls = vec![format!("{}/model.gguf", server.url())];
        let err = download_with_fallback(&dest, &urls, &|_| {}).unwrap_err();
        assert!(err.to_string().contains("下载内容过小"), "{err:#}");
        assert!(!dest.exists() && !part_path(&dest).exists());
    }
}
