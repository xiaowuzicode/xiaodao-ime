//! LLM 润色 / 语音改写（1:1 移植 Python `xiaodao_ime/polisher.py`）。
//!
//! Provider（settings.json 的 `polish.provider`），Key 均为用户自备的大模型 API Key：
//! - `openai`（默认）：任意 OpenAI 兼容端点 —— DeepSeek / Kimi / GLM / OpenAI / ollama 本地模型等，
//!   配 `base_url` + `api_key` + `model` 即可；
//! - `anthropic`：Anthropic Messages API（原生 HTTP，不引 SDK），
//!   `api_key` 为空时读环境变量 `ANTHROPIC_API_KEY`。
//!
//! 失败一律 **fail-open**：返回 `None`，调用方直接使用原始转写，绝不阻断出字。

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::settings::{PolishConfig, Settings, SettingsStore};
use crate::types::Polish;

const PREFIX: &str =
    "你是语音输入法的后处理引擎。用户消息是一段语音转写的文本（中文为主，可能中英混杂）。";
const SUFFIX: &str = "不回答或评论文本内容，只输出结果本身，不要任何解释或前后缀。";

/// 内置润色风格（顺序即菜单顺序）；settings.json 的 `polish.styles` 可覆盖或新增。
pub const BUILTIN_STYLES: [(&str, &str); 5] = [
    (
        "润色",
        "你的任务：去掉口水词（嗯、啊、那个、就是说、然后然后 等）、修正同音或近音错字、规范标点符号。保持原意和语气，不增删信息。",
    ),
    (
        "书面化",
        "你的任务：把口语转写整理成规范书面语——去口水词、修错字、规范标点，并适度重组句式使表达更清晰专业，可合并明显冗余，但不得改变事实、立场与信息量。",
    ),
    (
        "轻度纠错",
        "你的任务：只修正同音/近音错字和标点，尽量保留原始口语表达，除明显口水词外不删任何内容。",
    ),
    (
        "翻译成英文",
        "你的任务：先在心里修正转写错字，然后把内容翻译成自然地道的英文，保持原有语气。只输出英文。",
    ),
    (
        "会议纪要",
        "你的任务：把口述内容整理成要点式纪要（用「- 」列表），修正错字、合并重复，保留所有信息点。",
    ),
];

/// 默认风格名。
pub const DEFAULT_STYLE: &str = "润色";

/// Anthropic Messages API 默认端点；可用环境变量 `ANTHROPIC_BASE_URL` 覆盖（自建网关/测试用）。
const ANTHROPIC_BASE: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const ANTHROPIC_DEFAULT_MODEL: &str = "claude-haiku-4-5";

/// 内置风格的提示词。
pub fn builtin_style(name: &str) -> Option<&'static str> {
    BUILTIN_STYLES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, p)| *p)
}

/// 内置风格 + 用户自定义风格（`polish.styles`，同名覆盖）。
pub fn get_styles(settings: &Settings) -> BTreeMap<String, String> {
    let mut styles: BTreeMap<String, String> = BUILTIN_STYLES
        .iter()
        .map(|(n, p)| (n.to_string(), p.to_string()))
        .collect();
    for (name, prompt) in &settings.polish.styles {
        if !name.is_empty() && !prompt.is_empty() {
            styles.insert(name.clone(), prompt.clone());
        }
    }
    styles
}

/// 菜单展示顺序：内置 5 个在前（固定顺序），用户新增的按名排在后面。
pub fn style_names(settings: &Settings) -> Vec<String> {
    let mut names: Vec<String> = BUILTIN_STYLES.iter().map(|(n, _)| n.to_string()).collect();
    for name in get_styles(settings).keys() {
        if !names.iter().any(|n| n == name) {
            names.push(name.clone());
        }
    }
    names
}

/// 拼 system 提示词：前缀 + 风格 + 后缀（+ 热词）。
pub fn build_system_prompt(hotwords: &[String], style_prompt: Option<&str>) -> String {
    let style = style_prompt
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| builtin_style(DEFAULT_STYLE).unwrap());
    let mut prompt = format!("{PREFIX}{style}{SUFFIX}");
    let words: Vec<&str> = hotwords
        .iter()
        .map(|w| w.as_str())
        .filter(|w| !w.is_empty())
        .collect();
    if !words.is_empty() {
        prompt.push_str("\n常用专有名词（转写易错，优先按此拼写纠正）：");
        prompt.push_str(&words.join("、"));
    }
    prompt
}

/// 离线替换表：无条件字符串替换，不依赖 LLM。
pub fn apply_replacements(text: &str, mapping: &BTreeMap<String, String>) -> String {
    let mut out = text.to_string();
    for (src, dst) in mapping {
        if !src.is_empty() {
            out = out.replace(src.as_str(), dst);
        }
    }
    out
}

/// LLM 客户端。实现 [`crate::types::Polish`]，全部 fail-open。
pub struct Polisher {
    store: SettingsStore,
    client: reqwest::blocking::Client,
    anthropic_base: String,
}

impl Polisher {
    pub fn new(store: SettingsStore) -> Self {
        let anthropic_base = std::env::var("ANTHROPIC_BASE_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| ANTHROPIC_BASE.to_string());
        Self::with_anthropic_base(store, anthropic_base)
    }

    /// 指定 Anthropic 端点（自建网关 / 单测 mock 用）。
    pub fn with_anthropic_base(store: SettingsStore, anthropic_base: impl Into<String>) -> Self {
        Self {
            store,
            client: reqwest::blocking::Client::new(),
            anthropic_base: anthropic_base.into().trim_end_matches('/').to_string(),
        }
    }

    fn call(
        &self,
        provider: &str,
        text: &str,
        conf: &PolishConfig,
        system: &str,
    ) -> Option<String> {
        let started = Instant::now();
        let result = match provider {
            "openai" => self.via_openai(text, conf, system),
            "anthropic" => self.via_anthropic(text, conf, system),
            other => {
                warn!("未知润色 provider: {other:?}，跳过");
                return None;
            }
        };
        match result {
            Ok(out) => {
                let out = out.trim().to_string();
                if out.is_empty() {
                    None
                } else {
                    info!(
                        "LLM 返回（{provider} / {}），耗时 {:.2}s",
                        conf.model,
                        started.elapsed().as_secs_f64()
                    );
                    Some(out)
                }
            }
            Err(e) => {
                warn!("LLM 调用失败（{provider}），fail-open 使用原文：{e:#}");
                None
            }
        }
    }

    /// OpenAI 兼容端点：`POST {base_url}/chat/completions`。
    fn via_openai(&self, text: &str, conf: &PolishConfig, system: &str) -> Result<String> {
        let base_url = conf.base_url.trim().trim_end_matches('/');
        if base_url.is_empty() {
            bail!("请先在 settings.json 配置 polish.base_url（如 https://api.deepseek.com）");
        }
        let body = json!({
            "model": conf.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": text},
            ],
            "temperature": 0.2,
        });
        let mut req = self
            .client
            .post(format!("{base_url}/chat/completions"))
            .timeout(Duration::from_secs(conf.timeout.max(1)))
            .json(&body);
        if !conf.api_key.is_empty() {
            req = req.bearer_auth(&conf.api_key);
        }
        let data = send_json(req)?;
        data["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("响应缺少 choices[0].message.content：{data}"))
    }

    /// Anthropic Messages API：`POST {base}/v1/messages`（原生 HTTP，不引 SDK）。
    fn via_anthropic(&self, text: &str, conf: &PolishConfig, system: &str) -> Result<String> {
        let api_key = if conf.api_key.is_empty() {
            std::env::var("ANTHROPIC_API_KEY").unwrap_or_default()
        } else {
            conf.api_key.clone()
        };
        if api_key.trim().is_empty() {
            bail!("缺少 Anthropic API Key（settings.polish.api_key 或环境变量 ANTHROPIC_API_KEY）");
        }
        let model = if conf.model.is_empty() {
            ANTHROPIC_DEFAULT_MODEL
        } else {
            conf.model.as_str()
        };
        let mut body = json!({
            "model": model,
            "max_tokens": 2048,
            "system": system,
            "messages": [{"role": "user", "content": text}],
        });
        // 润色是轻任务：支持 effort 的模型用 low 压延迟（与 Python 版一致）。
        const LOW_EFFORT_PREFIXES: [&str; 5] = [
            "claude-fable",
            "claude-mythos",
            "claude-opus-4",
            "claude-sonnet-4-6",
            "claude-sonnet-5",
        ];
        if LOW_EFFORT_PREFIXES.iter().any(|p| model.starts_with(p)) {
            body["output_config"] = json!({"effort": "low"});
        }
        let req = self
            .client
            .post(format!("{}/v1/messages", self.anthropic_base))
            .timeout(Duration::from_secs(conf.timeout.max(1)))
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body);
        let data = send_json(req)?;
        if data["stop_reason"].as_str() == Some("refusal") {
            warn!("请求被安全策略拒绝，使用原文");
            return Ok(String::new());
        }
        let text = data["content"]
            .as_array()
            .and_then(|blocks| {
                blocks
                    .iter()
                    .find(|b| b["type"].as_str() == Some("text"))
                    .and_then(|b| b["text"].as_str())
            })
            .unwrap_or_default();
        Ok(text.to_string())
    }
}

fn send_json(req: reqwest::blocking::RequestBuilder) -> Result<Value> {
    let resp = req.send()?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        bail!("HTTP {status}：{}", truncate(&text, 200));
    }
    Ok(serde_json::from_str(&text)?)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "…"
}

impl Polish for Polisher {
    fn enabled(&self) -> bool {
        self.store.get().polish.enabled
    }

    /// 是否已具备可用配置（开关打开前的前置检查）。
    fn configured(&self) -> bool {
        let conf = self.store.get().polish;
        if conf.provider == "openai" {
            !conf.base_url.trim().is_empty()
        } else {
            true // anthropic 可走环境变量 ANTHROPIC_API_KEY
        }
    }

    /// 返回润色后的文本；未启用或失败返回 `None`（调用方用原文）。
    ///
    /// `style` 为场景感知的风格覆盖（app_styles 匹配结果），`None` 时用全局默认风格。
    fn polish(&self, text: &str, style: Option<&str>) -> Option<String> {
        let settings = self.store.get();
        let conf = &settings.polish;
        if !conf.enabled || text.trim().is_empty() {
            return None;
        }
        let styles = get_styles(&settings);
        let name = style
            .filter(|s| styles.contains_key(*s))
            .map(str::to_string)
            .unwrap_or_else(|| {
                if conf.style.is_empty() {
                    DEFAULT_STYLE.to_string()
                } else {
                    conf.style.clone()
                }
            });
        let prompt = styles
            .get(&name)
            .or_else(|| styles.get(DEFAULT_STYLE))
            .cloned()
            .unwrap_or_default();
        let system = build_system_prompt(&settings.hotwords, Some(&prompt));
        let out = self.call(&conf.provider, text, conf, &system)?;
        info!("润色完成（{} / {} / {name}）", conf.provider, conf.model);
        Some(out)
    }

    /// 语音指令改写：按口头指令改写选中文本。失败返回 `None`（调用方不动原文）。
    ///
    /// 不受 `polish.enabled` 影响（那是听写润色的开关），只要 provider 配置可用即可。
    fn rewrite(&self, selection: &str, instruction: &str) -> Option<String> {
        if selection.trim().is_empty() || instruction.trim().is_empty() {
            return None;
        }
        let settings = self.store.get();
        let conf = &settings.polish;
        let mut system = String::from(
            "你是文本改写引擎。按照用户的口头指令改写给定文本，保持改写结果可以直接原地替换原文（不加解释、不加前后缀、不加引号）。指令若与文本无关或无法执行，原样输出原文。",
        );
        let words: Vec<&str> = settings
            .hotwords
            .iter()
            .map(|w| w.as_str())
            .filter(|w| !w.is_empty())
            .collect();
        if !words.is_empty() {
            system.push_str("\n常用专有名词：");
            system.push_str(&words.join("、"));
        }
        let user_msg = format!("指令：{instruction}\n\n待改写文本：\n{selection}");
        let out = self.call(&conf.provider, &user_msg, conf, &system)?;
        info!(
            "改写完成（{} / {}），指令={instruction:?}",
            conf.provider, conf.model
        );
        Some(out)
    }
}
