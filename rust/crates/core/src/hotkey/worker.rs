//! 收尾 worker：转写 → 替换表 → 场景风格 → 润色 → 粘贴 → 历史（听写通道），
//! 以及 抓选区 → 识别指令 → LLM 改写 → 原地替换（改写通道）。
//!
//! 逐行对照 Python `hotkey.py` 的 `_transcribe_and_paste` / `_rewrite_and_replace`。
//! **本文件全部运行在 worker 线程，且不持有任何状态锁**——转写和 LLM 调用可能耗时
//! 数秒，持锁会把键盘钩子线程一起卡死（Python 版靠 GIL 释放规避，Rust 必须显式保证）。

use std::thread;

use tracing::{error, info};

use super::HotkeyController;
use crate::context::{is_style_off, pick_style};
use crate::hud;
use crate::paster::{self, SelectionCapture};
use crate::polisher::apply_replacements;
use crate::types::{Channel, SoundEvent, Status};

impl HotkeyController {
    /// 把音频交给后台线程处理；调用方（状态机）持锁，这里只做 spawn。
    pub(crate) fn spawn_worker(&self, channel: Channel, pcm: Vec<f32>) {
        let Some(me) = self.me.upgrade() else {
            return;
        };
        let spawned = thread::Builder::new()
            .name("xiaodao-worker".into())
            .spawn(move || match channel {
                Channel::Dictate => me.transcribe_and_paste(&pcm),
                Channel::Rewrite => me.rewrite_and_replace(&pcm),
            });
        if let Err(e) = spawned {
            error!("启动处理线程失败：{}", e);
            self.hud.hide();
            self.status(Status::Idle);
        }
    }

    // ---- 听写通道 ----

    fn transcribe_and_paste(&self, pcm: &[f32]) {
        if let Err(e) = self.run_dictate(pcm) {
            error!("转写/粘贴流程异常：{}", e);
        }
        // 对应 Python 的 finally
        self.hud.hide();
        self.status(Status::Idle);
    }

    fn run_dictate(&self, pcm: &[f32]) -> anyhow::Result<()> {
        let raw = self.transcriber.transcribe(pcm, false)?;
        if raw.trim().is_empty() {
            info!("转写结果为空，不粘贴");
            return Ok(());
        }
        let mut text = apply_replacements(&raw, &self.settings.replacements());

        if let Some(polisher) = self.polisher.as_ref() {
            if polisher.enabled() {
                // 场景感知：按前台 App 匹配润色风格（或对该 App 关闭润色）
                let app = self.platform.frontmost_app();
                let style = pick_style(&app.name, &app.id, &self.settings.app_styles());
                if !app.name.is_empty() || !app.id.is_empty() {
                    info!(
                        "前台应用：{}（{}）→ 风格 {}",
                        app.name,
                        app.id,
                        style.as_deref().unwrap_or("默认")
                    );
                }
                if style.as_deref().is_some_and(is_style_off) {
                    info!("该应用已配置关闭润色，直出转写");
                } else {
                    self.status(Status::Polishing);
                    // 等待不做黑盒：润色期间先把已转写全文亮出来
                    self.hud.set_status("润色中…", &text);
                    // 润色失败时 polish 返回 None，直接用原始转写（fail-open）
                    if let Some(polished) = polisher.polish(&text, style.as_deref()) {
                        text = polished;
                    }
                }
            }
        }

        paster::paste_text(&self.platform, &text);
        if let Some(history) = self.history.as_ref() {
            history.append(&raw, &text);
        }
        Ok(())
    }

    // ---- 改写通道 ----

    /// 语音改写：抓选区 → 识别指令 → LLM 改写 → 原地替换。全程 fail-open。
    fn rewrite_and_replace(&self, pcm: &[f32]) {
        let mut capture: Option<SelectionCapture> = None;
        if let Err(e) = self.run_rewrite(pcm, &mut capture) {
            error!("改写流程异常：{}", e);
        }
        // 对应 Python 的 finally：未完成替换时立即归还原剪贴板（可安全重复调用）
        if let Some(capture) = capture.as_mut() {
            capture.restore(&self.platform);
        }
        self.hud.hide();
        self.status(Status::Idle);
    }

    fn run_rewrite(&self, pcm: &[f32], slot: &mut Option<SelectionCapture>) -> anyhow::Result<()> {
        let configured = self.polisher.as_ref().is_some_and(|p| p.configured());
        if !configured {
            self.play(SoundEvent::Cancel);
            self.notify(
                "语音改写不可用",
                "请先在「设置 → 打开配置文件」配置 polish 的 base_url / api_key",
            );
            return Ok(());
        }
        let polisher = self
            .polisher
            .as_ref()
            .expect("configured 已保证 polisher 存在");

        let capture = slot.insert(paster::capture_selection(&self.platform));
        let selection = capture.text.clone().unwrap_or_default();
        if selection.trim().is_empty() {
            self.play(SoundEvent::Cancel);
            self.notify("未检测到选中文字", "先选中要改写的文本，再按改写热键说指令");
            return Ok(());
        }
        self.hud.set_status("识别指令中…", &selection);

        let instruction = self.transcriber.transcribe(pcm, false)?.trim().to_string();
        if instruction.is_empty() {
            self.play(SoundEvent::Cancel);
            self.notify("没听清指令", "再按改写热键说一次？");
            return Ok(());
        }
        info!(
            "改写指令：{:?}，选区 {} 字符",
            instruction,
            selection.chars().count()
        );
        self.hud.set_status(
            &format!("改写中：{}", hud::head(&instruction, 24)),
            &selection,
        );
        self.status(Status::Polishing);

        let Some(result) = polisher.rewrite(&selection, &instruction) else {
            self.play(SoundEvent::Cancel);
            self.notify("改写失败", "模型没有返回结果，原文未改动");
            return Ok(());
        };

        let replaced = slot
            .as_mut()
            .expect("上面刚写入选区事务")
            .replace(&self.platform, &result);
        if replaced {
            if let Some(history) = self.history.as_ref() {
                history.append(&format!("〔改写〕{instruction}"), &result);
            }
        }
        Ok(())
    }
}
