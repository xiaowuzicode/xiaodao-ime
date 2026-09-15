//! `polisher` 的离线测试（移植自 Python `test_polish.py` + mockito 起本地 HTTP）。
//! 放在 tests/ 而非模块内，是为了守住「单文件 ≤ 500 行」。

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use mockito::Matcher;
use serde_json::json;
use xiaodao_core::polisher::{
    apply_replacements, build_system_prompt, builtin_style, get_styles, style_names, Polisher,
};
use xiaodao_core::settings::{Settings, SettingsStore};
use xiaodao_core::types::Polish;

/// 纯内存 store（不落盘）。
fn store(f: impl FnOnce(&mut Settings)) -> SettingsStore {
    let mut s = Settings::default();
    f(&mut s);
    SettingsStore::from_settings(std::path::PathBuf::from("/dev/null/settings.json"), s)
}

fn enabled_openai(base_url: &str) -> SettingsStore {
    let base_url = base_url.to_string();
    store(move |s| {
        s.polish.enabled = true;
        s.polish.provider = "openai".into();
        s.polish.api_key = "sk-test".into();
        s.polish.base_url = base_url;
        s.polish.timeout = 5;
    })
}

/// 移植自 Python `test_polish.py::test_apply_replacements`。
#[test]
fn apply_replacements_ported() {
    let map: BTreeMap<String, String> = [("欧朵".to_string(), "Ordo".to_string())]
        .into_iter()
        .collect();
    assert_eq!(apply_replacements("欧朵的达人匹配", &map), "Ordo的达人匹配");
    assert_eq!(apply_replacements("无替换", &BTreeMap::new()), "无替换");
}

/// 移植自 Python `test_polish.py::test_build_system_prompt`。
#[test]
fn build_system_prompt_ported() {
    let hotwords = vec!["小岛AI".to_string(), "Ordo".to_string()];
    assert!(build_system_prompt(&hotwords, None).contains("小岛AI"));
    assert!(build_system_prompt(&hotwords, None).contains("小岛AI、Ordo"));
    assert!(!build_system_prompt(&[], None).contains("专有名词"));
    assert!(build_system_prompt(&[], None).starts_with("你是语音输入法的后处理引擎"));
    assert!(build_system_prompt(&[], None).ends_with("不要任何解释或前后缀。"));
}

/// 移植自 Python `test_polish.py::test_styles`。
#[test]
fn styles_ported() {
    let s = Settings::default();
    let styles = get_styles(&s);
    for name in ["润色", "书面化", "轻度纠错", "翻译成英文", "会议纪要"] {
        assert!(styles.contains_key(name), "缺风格 {name}");
    }
    let mut s2 = Settings::default();
    s2.polish.styles = [
        ("文言文".to_string(), "翻译成文言文".to_string()),
        ("润色".to_string(), "自定义润色规则".to_string()),
    ]
    .into_iter()
    .collect();
    let styles = get_styles(&s2);
    assert_eq!(styles["文言文"], "翻译成文言文");
    assert_eq!(styles["润色"], "自定义润色规则");
    assert!(build_system_prompt(&[], builtin_style("翻译成英文")).contains("英文"));
    // 菜单顺序：内置在前、自定义在后
    assert_eq!(style_names(&s2)[0], "润色");
    assert_eq!(style_names(&s2).last().unwrap(), "文言文");
}

/// 移植自 Python `test_polish.py::test_polisher_fail_open`。
#[test]
fn fail_open_ported() {
    // 未启用 -> None
    let p = Polisher::new(store(|_| {}));
    assert!(p.polish("测试文本", None).is_none());
    // 启用但 provider 未知 -> None
    let p = Polisher::new(store(|s| {
        s.polish.enabled = true;
        s.polish.provider = "nope".into();
    }));
    assert!(p.polish("测试文本", None).is_none());
    // openai 但 base_url 为空 -> configured false + None
    let p = Polisher::new(store(|s| {
        s.polish.enabled = true;
        s.polish.base_url = String::new();
    }));
    assert!(!p.configured());
    assert!(p.polish("测试文本", None).is_none());
    // 空文本 -> None
    let p = Polisher::new(enabled_openai("https://example.invalid"));
    assert!(p.polish("   ", None).is_none());
    assert!(p.rewrite("  ", "改成英文").is_none());
    assert!(p.rewrite("原文", "  ").is_none());
    assert!(p.enabled());
    assert!(p.configured());
}

#[test]
fn openai_success() {
    let mut server = mockito::Server::new();
    let m = server
        .mock("POST", "/chat/completions")
        .match_header("authorization", "Bearer sk-test")
        .match_body(Matcher::PartialJson(
            json!({"model": "deepseek-chat", "temperature": 0.2}),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"choices":[{"message":{"content":" 润色后的文本 "}}]}"#)
        .create();
    let p = Polisher::new(enabled_openai(&server.url()));
    assert_eq!(p.polish("测试 文本", None).as_deref(), Some("润色后的文本"));
    m.assert();
}

#[test]
fn openai_rewrite_ignores_enabled_switch() {
    let mut server = mockito::Server::new();
    let m = server
        .mock("POST", "/chat/completions")
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex("指令：改成英文".to_string()),
            Matcher::Regex("待改写文本".to_string()),
            Matcher::Regex("你是文本改写引擎".to_string()),
        ]))
        .with_body(r#"{"choices":[{"message":{"content":"Hello"}}]}"#)
        .create();
    let url = server.url();
    let s = store(move |s| {
        s.polish.enabled = false; // 改写不看这个开关
        s.polish.base_url = url;
        s.polish.timeout = 5;
    });
    let p = Polisher::new(s);
    assert_eq!(p.rewrite("你好", "改成英文").as_deref(), Some("Hello"));
    m.assert();
}

#[test]
fn anthropic_success() {
    let mut server = mockito::Server::new();
    let m = server
        .mock("POST", "/v1/messages")
        .match_header("x-api-key", "sk-ant")
        .match_header("anthropic-version", "2023-06-01")
        .match_body(Matcher::PartialJson(json!({"max_tokens": 2048})))
        .with_body(
            r#"{"stop_reason":"end_turn","content":[{"type":"thinking","thinking":"x"},{"type":"text","text":"Anthropic 结果"}]}"#,
        )
        .create();
    let s = store(|s| {
        s.polish.enabled = true;
        s.polish.provider = "anthropic".into();
        s.polish.api_key = "sk-ant".into();
        s.polish.timeout = 5;
    });
    let p = Polisher::with_anthropic_base(s, server.url());
    assert_eq!(p.polish("测试", None).as_deref(), Some("Anthropic 结果"));
    m.assert();
}

#[test]
fn anthropic_refusal_fails_open() {
    let mut server = mockito::Server::new();
    let _m = server
        .mock("POST", "/v1/messages")
        .with_body(r#"{"stop_reason":"refusal","content":[{"type":"text","text":"不行"}]}"#)
        .create();
    let s = store(|s| {
        s.polish.enabled = true;
        s.polish.provider = "anthropic".into();
        s.polish.api_key = "sk-ant".into();
        s.polish.timeout = 5;
    });
    let p = Polisher::with_anthropic_base(s, server.url());
    assert!(p.polish("测试", None).is_none());
}

#[test]
fn http_error_and_empty_content_fail_open() {
    let mut server = mockito::Server::new();
    let _bad = server
        .mock("POST", "/chat/completions")
        .with_status(500)
        .with_body("boom")
        .expect_at_least(1)
        .create();
    let p = Polisher::new(enabled_openai(&server.url()));
    assert!(p.polish("测试", None).is_none());

    let mut server2 = mockito::Server::new();
    let _empty = server2
        .mock("POST", "/chat/completions")
        .with_body(r#"{"choices":[{"message":{"content":"   "}}]}"#)
        .create();
    let p2 = Polisher::new(enabled_openai(&server2.url()));
    assert!(p2.polish("测试", None).is_none());

    // 响应结构不对（缺 choices）也 fail-open
    let mut server3 = mockito::Server::new();
    let _weird = server3
        .mock("POST", "/chat/completions")
        .with_body(r#"{"error":"nope"}"#)
        .create();
    let p3 = Polisher::new(enabled_openai(&server3.url()));
    assert!(p3.polish("测试", None).is_none());
}

/// 超时 fail-open：服务端接受连接但永不回包。
#[test]
fn timeout_fails_open() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        while let Ok((sock, _)) = listener.accept() {
            held.push(sock); // 只 hold 住不回包
        }
    });
    let s = enabled_openai(&format!("http://{addr}"));
    s.update(|s| s.polish.timeout = 1);
    let p = Polisher::new(s);
    let t0 = Instant::now();
    assert!(p.polish("测试", None).is_none());
    assert!(t0.elapsed() < Duration::from_secs(10));
}

/// 核心层按 `Arc<dyn Polish>` 共享给各线程，这里钉住 trait object 可用 + Send/Sync。
#[test]
fn polisher_is_shareable() {
    let shared: xiaodao_core::types::SharedPolisher =
        std::sync::Arc::new(Polisher::new(store(|_| {})));
    let handle = std::thread::spawn(move || shared.enabled());
    assert!(!handle.join().unwrap());
}
