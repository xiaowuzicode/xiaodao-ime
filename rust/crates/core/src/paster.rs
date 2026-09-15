//! 粘贴与选区抓取（平台无关流程；剪贴板/按键原语来自 [`Platform`] 后端）。
//!
//! 1:1 移植 `xiaodao_ime/paster.py`（含 `SelectionCapture` 事务式 API）。
//!
//! - 粘贴：保存原剪贴板 → 写入转写文字 → 模拟粘贴快捷键 → **后台线程延迟恢复**原剪贴板；
//! - 抓选区：写入哨兵值 → 模拟复制快捷键 → 读回；仍是哨兵说明没有选区。
//!
//! 所有入口都收 `&SharedPlatform`（`Arc<dyn Platform>`）：延迟恢复要把后端搬进线程，
//! 拿不到 `Arc` 就没法做；统一签名免得调用方在 `&dyn` / `Arc` 之间来回转。

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::platform::SharedPlatform;

/// 粘贴后隔多久把原剪贴板还回去（同 Python `config.CLIPBOARD_RESTORE_DELAY`）。
///
/// TODO(agent A)：`paths.rs` / `settings.rs` 就位后改为从那里读，这里先留本地常量。
pub const CLIPBOARD_RESTORE_DELAY: Duration = Duration::from_millis(400);

/// 抓选区用的哨兵：前后各一个零宽空格，正常文本几乎不可能撞上。
pub const SENTINEL: &str = "\u{200b}__xiaodao_sentinel__\u{200b}";

/// 流程里的各段等待时长。生产用 [`Default`]，测试注入 0 或很小的值以免慢/flaky。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PasterTimings {
    /// 写完剪贴板到发粘贴键之间的缓冲（给系统时间让写入生效）。
    pub before_paste: Duration,
    /// 粘贴后隔多久恢复原剪贴板。
    pub restore_delay: Duration,
    /// 写完哨兵到发复制键之间的缓冲。
    pub sentinel_settle: Duration,
    /// 发完复制键到读回剪贴板之间的缓冲。
    pub copy_settle: Duration,
}

impl Default for PasterTimings {
    fn default() -> Self {
        Self {
            before_paste: Duration::from_millis(30),
            restore_delay: CLIPBOARD_RESTORE_DELAY,
            sentinel_settle: Duration::from_millis(50),
            copy_settle: Duration::from_millis(250),
        }
    }
}

/// 立即恢复剪贴板（改写流程中止时用）。`None` 表示原本就是空的 → 清空。
pub fn restore_clipboard(platform: &SharedPlatform, original: Option<&str>) {
    let result = match original {
        Some(text) => platform.write_clipboard(text),
        None => platform.clear_clipboard(),
    };
    if let Err(e) = result {
        tracing::warn!("恢复剪贴板失败：{e}");
    }
}

/// 仅复制到剪贴板（历史菜单用），不触发粘贴、不恢复原内容。
pub fn copy_to_clipboard(platform: &SharedPlatform, text: &str) -> bool {
    match platform.write_clipboard(text) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("复制到剪贴板失败：{e}");
            false
        }
    }
}

/// 把 `text` 粘贴到当前焦点输入框，粘贴后延迟恢复**当前**剪贴板内容。成功返回 `true`。
pub fn paste_text(platform: &SharedPlatform, text: &str) -> bool {
    paste_text_restoring(platform, text, None)
}

/// 同 [`paste_text`]，但可显式指定粘贴后要恢复成什么。
///
/// `restore_to`：
/// - `None` —— 未指定，读当前剪贴板作为「原内容」（对应 Python 的 `_UNSET`）；
/// - `Some(Some(s))` —— 恢复成 `s`（改写流程用：此刻剪贴板里是抓选区留下的内容，
///   不是用户原本的内容）；
/// - `Some(None)` —— 用户原本剪贴板就是空的，恢复时清空。
pub fn paste_text_restoring(
    platform: &SharedPlatform,
    text: &str,
    restore_to: Option<Option<String>>,
) -> bool {
    paste_text_with(platform, text, restore_to, PasterTimings::default())
}

/// [`paste_text_restoring`] 的可注入时长版本（测试用）。
pub fn paste_text_with(
    platform: &SharedPlatform,
    text: &str,
    restore_to: Option<Option<String>>,
    timings: PasterTimings,
) -> bool {
    if text.trim().is_empty() {
        tracing::info!("粘贴跳过：文本为空");
        return false;
    }
    let original = match restore_to {
        Some(explicit) => explicit,
        None => platform.read_clipboard(),
    };
    if let Err(e) = platform.write_clipboard(text) {
        tracing::error!("粘贴失败：写入剪贴板出错：{e}");
        return false;
    }
    // 给系统一点时间让剪贴板写入生效，再触发粘贴
    thread::sleep(timings.before_paste);
    if let Err(e) = platform.send_paste() {
        tracing::error!("粘贴失败：{e}");
        return false;
    }
    tracing::info!("已粘贴 {} 字符", text.chars().count());

    let deferred = Arc::clone(platform);
    let delay = timings.restore_delay;
    thread::spawn(move || {
        thread::sleep(delay);
        restore_clipboard(&deferred, original.as_deref());
        tracing::debug!("原剪贴板已恢复");
    });
    true
}

/// 一次选区替换的剪贴板事务。
///
/// 调用方只关心是否抓到文本（[`SelectionCapture::text`]），以及最后要替换成什么。
/// 哨兵、原剪贴板和延迟恢复都留在本 module 里，避免把所有权协议泄漏给改写流程。
#[derive(Debug)]
pub struct SelectionCapture {
    /// 抓到的选区文本；没有选区 / 复制失败为 `None`。
    pub text: Option<String>,
    original: Option<String>,
    settled: bool,
    timings: PasterTimings,
}

impl SelectionCapture {
    /// 替换选区，并把原剪贴板交给粘贴流程延迟恢复。
    pub fn replace(&mut self, platform: &SharedPlatform, text: &str) -> bool {
        if self.settled {
            tracing::warn!("选区事务已经结束，跳过重复替换");
            return false;
        }
        let pasted = paste_text_with(platform, text, Some(self.original.clone()), self.timings);
        self.settled = true;
        if !pasted {
            // 粘贴没成功就没人接管延迟恢复，这里立刻归还
            restore_clipboard(platform, self.original.as_deref());
        }
        pasted
    }

    /// 在未完成替换时立即归还原剪贴板，可安全重复调用（幂等）。
    pub fn restore(&mut self, platform: &SharedPlatform) {
        if self.settled {
            return;
        }
        self.settled = true;
        restore_clipboard(platform, self.original.as_deref());
    }
}

/// 抓取当前焦点 App 中选中的文本，返回负责清理的事务对象。
///
/// 即使没有选区或系统复制失败，调用方也只需在收尾时调用 [`SelectionCapture::restore`]，
/// 不需要再知道哨兵和原剪贴板的细节。
pub fn capture_selection(platform: &SharedPlatform) -> SelectionCapture {
    capture_selection_with(platform, PasterTimings::default())
}

/// [`capture_selection`] 的可注入时长版本（测试用）。
pub fn capture_selection_with(
    platform: &SharedPlatform,
    timings: PasterTimings,
) -> SelectionCapture {
    let original = platform.read_clipboard();
    let copied = (|| -> anyhow::Result<Option<String>> {
        platform.write_clipboard(SENTINEL)?;
        thread::sleep(timings.sentinel_settle);
        platform.send_copy()?;
        thread::sleep(timings.copy_settle);
        Ok(platform.read_clipboard())
    })();
    let text = match copied {
        // 剪贴板仍是哨兵（或空）=> 当前焦点里没有选区
        Ok(Some(s)) if !s.is_empty() && s != SENTINEL => Some(s),
        Ok(_) => None,
        Err(e) => {
            tracing::warn!("抓取选区失败：{e}");
            None
        }
    };
    SelectionCapture {
        text,
        original,
        settled: false,
        timings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Platform;
    use crate::types::{FrontmostApp, Permissions, PrivacySection, SoundEvent};
    use parking_lot::Mutex;
    use std::time::Instant;

    /// 假剪贴板/按键后端。`copy_effect` 控制模拟 复制快捷键 后剪贴板变成什么
    /// （`None` = 复制不改变剪贴板，即「没有选区」）。
    #[derive(Default)]
    struct FakePlatform {
        state: Mutex<FakeState>,
    }

    #[derive(Default)]
    struct FakeState {
        clipboard: Option<String>,
        copy_effect: Option<String>,
        /// 剪贴板写入序列（含 clear，用 None 表示）
        writes: Vec<Option<String>>,
        copies: usize,
        pastes: usize,
        /// 置 true 后 send_paste 返回错误，用来验证 replace 失败路径
        fail_paste: bool,
    }

    impl FakePlatform {
        fn new(initial: Option<&str>, copy_effect: Option<&str>) -> Arc<Self> {
            Arc::new(Self {
                state: Mutex::new(FakeState {
                    clipboard: initial.map(str::to_owned),
                    copy_effect: copy_effect.map(str::to_owned),
                    ..Default::default()
                }),
            })
        }

        fn clipboard(&self) -> Option<String> {
            self.state.lock().clipboard.clone()
        }

        fn pastes(&self) -> usize {
            self.state.lock().pastes
        }

        fn copies(&self) -> usize {
            self.state.lock().copies
        }

        fn writes(&self) -> Vec<Option<String>> {
            self.state.lock().writes.clone()
        }

        fn fail_paste(&self) {
            self.state.lock().fail_paste = true;
        }
    }

    impl Platform for FakePlatform {
        fn read_clipboard(&self) -> Option<String> {
            self.state.lock().clipboard.clone()
        }
        fn write_clipboard(&self, text: &str) -> anyhow::Result<()> {
            let mut s = self.state.lock();
            s.clipboard = Some(text.to_owned());
            s.writes.push(Some(text.to_owned()));
            Ok(())
        }
        fn clear_clipboard(&self) -> anyhow::Result<()> {
            let mut s = self.state.lock();
            s.clipboard = None;
            s.writes.push(None);
            Ok(())
        }
        fn send_copy(&self) -> anyhow::Result<()> {
            let mut s = self.state.lock();
            s.copies += 1;
            if let Some(effect) = s.copy_effect.clone() {
                s.clipboard = Some(effect);
            }
            Ok(())
        }
        fn send_paste(&self) -> anyhow::Result<()> {
            let mut s = self.state.lock();
            if s.fail_paste {
                anyhow::bail!("模拟粘贴失败");
            }
            s.pastes += 1;
            Ok(())
        }
        fn play_sound(&self, _: SoundEvent) {}
        fn frontmost_app(&self) -> FrontmostApp {
            FrontmostApp::default()
        }
        fn check_permissions(&self, _: bool) -> Permissions {
            Permissions {
                input_monitoring: true,
                accessibility: true,
            }
        }
        fn open_privacy_settings(&self, _: PrivacySection) {}
    }

    /// 后台恢复线程是异步的，轮询等待条件成立（上限 2s，够慢机器用，正常几毫秒内返回）。
    fn wait_until(mut cond: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if cond() {
                return true;
            }
            thread::sleep(Duration::from_millis(2));
        }
        cond()
    }

    fn fast() -> PasterTimings {
        PasterTimings {
            before_paste: Duration::ZERO,
            restore_delay: Duration::ZERO,
            sentinel_settle: Duration::ZERO,
            copy_settle: Duration::ZERO,
        }
    }

    #[test]
    fn paste_restores_clipboard_after_delay() {
        let fake = FakePlatform::new(Some("老板的转账账号"), None);
        let platform: SharedPlatform = fake.clone();
        let timings = PasterTimings {
            restore_delay: Duration::from_millis(80),
            ..fast()
        };

        assert!(paste_text_with(&platform, "转写结果", None, timings));
        assert_eq!(fake.pastes(), 1);
        // 粘贴瞬间剪贴板是转写文本
        assert_eq!(fake.clipboard().as_deref(), Some("转写结果"));
        // 延迟后恢复原内容
        assert!(wait_until(
            || fake.clipboard().as_deref() == Some("老板的转账账号")
        ));

        // 空文本/纯空白不粘贴
        assert!(!paste_text_with(&platform, "   ", None, timings));
        assert!(!paste_text_with(&platform, "", None, timings));
        assert_eq!(fake.pastes(), 1);
    }

    #[test]
    fn restore_clears_when_original_empty() {
        let fake = FakePlatform::new(None, None);
        let platform: SharedPlatform = fake.clone();

        assert!(paste_text_with(&platform, "转写结果", None, fast()));
        assert!(wait_until(|| fake.clipboard().is_none()));
        // 写入序列：先写转写文本，再 clear（None）
        assert_eq!(
            fake.writes(),
            vec![Some("转写结果".to_owned()), None],
            "空剪贴板必须用 clear 恢复，而不是写空串"
        );
    }

    #[test]
    fn capture_without_selection_restores_clipboard() {
        // copy_effect = None：模拟 Cmd+C 后剪贴板没变（焦点里没有选中文字）
        let fake = FakePlatform::new(Some("用户原有内容"), None);
        let platform: SharedPlatform = fake.clone();

        let mut capture = capture_selection_with(&platform, fast());
        assert_eq!(fake.copies(), 1);
        assert!(capture.text.is_none(), "剪贴板仍是哨兵 => 没有选区");
        capture.restore(&platform);
        assert_eq!(fake.clipboard().as_deref(), Some("用户原有内容"));
    }

    #[test]
    fn empty_copy_result_means_no_selection() {
        let fake = FakePlatform::new(Some("用户原有内容"), Some(""));
        let platform: SharedPlatform = fake.clone();

        let mut capture = capture_selection_with(&platform, fast());
        assert!(capture.text.is_none());
        capture.restore(&platform);
        assert_eq!(fake.clipboard().as_deref(), Some("用户原有内容"));
    }

    #[test]
    fn capture_replace_then_restores_clipboard() {
        let fake = FakePlatform::new(Some("旧内容"), Some("选中的文字"));
        let platform: SharedPlatform = fake.clone();
        let timings = PasterTimings {
            restore_delay: Duration::from_millis(120),
            ..fast()
        };

        let mut capture = capture_selection_with(&platform, timings);
        assert_eq!(capture.text.as_deref(), Some("选中的文字"));
        assert!(capture.replace(&platform, "改写结果"));
        assert_eq!(fake.pastes(), 1);
        assert_eq!(fake.clipboard().as_deref(), Some("改写结果"));

        // replace 已接管延迟恢复，重复清理不能提前覆盖粘贴文本
        capture.restore(&platform);
        assert_eq!(fake.clipboard().as_deref(), Some("改写结果"));

        assert!(wait_until(|| fake.clipboard().as_deref() == Some("旧内容")));
    }

    #[test]
    fn repeated_replace_is_rejected() {
        let fake = FakePlatform::new(Some("旧内容"), Some("选中的文字"));
        let platform: SharedPlatform = fake.clone();

        let mut capture = capture_selection_with(&platform, fast());
        assert!(capture.replace(&platform, "改写结果"));
        assert!(!capture.replace(&platform, "又一次改写"), "事务已结束");
        assert_eq!(fake.pastes(), 1);
    }

    #[test]
    fn failed_replace_restores_immediately() {
        let fake = FakePlatform::new(Some("旧内容"), Some("选中的文字"));
        let platform: SharedPlatform = fake.clone();
        let mut capture = capture_selection_with(&platform, fast());
        assert_eq!(capture.text.as_deref(), Some("选中的文字"));

        // 让 send_paste 开始失败
        fake.fail_paste();
        assert!(!capture.replace(&platform, "改写结果"));
        assert_eq!(fake.pastes(), 0);
        assert_eq!(
            fake.clipboard().as_deref(),
            Some("旧内容"),
            "粘贴失败必须立刻归还，不能等延迟线程"
        );
    }

    #[test]
    fn restore_is_idempotent() {
        let fake = FakePlatform::new(Some("旧内容"), None);
        let platform: SharedPlatform = fake.clone();
        let mut capture = capture_selection_with(&platform, fast());

        capture.restore(&platform);
        let after_first = fake.writes().len();
        capture.restore(&platform);
        capture.restore(&platform);
        assert_eq!(
            fake.writes().len(),
            after_first,
            "重复 restore 不应再写剪贴板"
        );
        assert_eq!(fake.clipboard().as_deref(), Some("旧内容"));
    }

    #[test]
    fn copy_to_clipboard_writes_only() {
        let fake = FakePlatform::new(Some("旧内容"), None);
        let platform: SharedPlatform = fake.clone();

        assert!(copy_to_clipboard(&platform, "历史记录一条"));
        assert_eq!(fake.clipboard().as_deref(), Some("历史记录一条"));
        assert_eq!(fake.pastes(), 0);
        // 不做延迟恢复
        thread::sleep(Duration::from_millis(50));
        assert_eq!(fake.clipboard().as_deref(), Some("历史记录一条"));
    }
}
