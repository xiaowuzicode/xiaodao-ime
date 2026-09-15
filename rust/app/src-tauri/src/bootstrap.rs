//! 启动编排：1:1 对齐 Python 版 `app.py` 的 `__init__` / `_boot` / `_finish_boot`。
//!
//! 时序：
//! 1. [`init`]（主线程，`setup` 最先调用）：解析数据目录 → 初始化日志 → 读 settings.json →
//!    平台后端 → 历史 → 润色器，全部塞进 [`AppState`] 交给 Tauri `manage`；
//! 2. [`start`]（主线程，托盘建好之后）：权限自检（macOS）+ 起后台启动线程；
//! 3. 后台线程：确保模型就位（首启自动下载，带进度）→ 加载常驻模型 →
//!    组装 [`Deps`] → [`HotkeyController`] → [`listener::spawn`] 挂全局键钩。
//!
//! 任一步失败都不让进程崩溃：状态行 + 系统通知给出明确指引（与 Python 版一致）。

use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use tauri::{AppHandle, Manager, State};
use xiaodao_core::audio::CpalRecorder;
use xiaodao_core::history::History;
use xiaodao_core::hotkey::{system_clock, Deps, HistorySink, HotkeyController, SettingsView};
use xiaodao_core::model_download::{self, Progress};
use xiaodao_core::paths::{self, Paths, MIN_HOLD_SECONDS, MODEL_REPO};
use xiaodao_core::platform::{self, SharedPlatform};
use xiaodao_core::polisher::Polisher;
use xiaodao_core::settings::SettingsStore;
use xiaodao_core::transcriber::Transcriber;
use xiaodao_core::types::{Events, Hud, PrivacySection, SharedPolisher, SharedTranscriber};
use xiaodao_core::{listener, logging};

use crate::events::TauriEvents;
use crate::hud::TauriHud;
use crate::tray;

/// 全局运行期状态。托盘菜单与前端命令都从这里取核心对象。
pub struct AppState {
    /// 数据目录与各文件路径（settings.json / 日志 / 模型 / 历史）。
    pub paths: Paths,
    pub store: SettingsStore,
    pub history: Arc<History>,
    pub platform: SharedPlatform,
    /// 与状态机共用同一个润色器，菜单里的「AI 润色」开关据此判断配置是否可用。
    pub polisher: Arc<Polisher>,
    /// 模型加载完成后才有；启动期间菜单调用要容忍 `None`。
    controller: OnceLock<Arc<HotkeyController>>,
    /// 启动自检发现缺失的权限（状态行点击时逐个直达系统设置）。
    perm_missing: Mutex<Vec<PrivacySection>>,
}

/// 互斥锁中毒时沿用内部值：菜单状态不是一致性关键数据，不值得让进程崩溃。
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl AppState {
    /// 热键控制器；模型还没加载完时返回 `None`。
    pub fn controller(&self) -> Option<Arc<HotkeyController>> {
        self.controller.get().cloned()
    }

    fn set_controller(&self, controller: Arc<HotkeyController>) {
        if self.controller.set(controller).is_err() {
            tracing::warn!("热键控制器重复初始化，已忽略");
        }
    }

    pub fn perm_missing(&self) -> Vec<PrivacySection> {
        lock(&self.perm_missing).clone()
    }
}

/// 取全局状态；`setup` 之前或初始化失败时返回 `None`。
pub fn state(app: &AppHandle) -> Option<State<'_, AppState>> {
    app.try_state::<AppState>()
}

/// 第一步：路径 / 日志 / 设置 / 平台 / 历史 / 润色器。
pub fn init(app: &AppHandle) -> anyhow::Result<()> {
    let paths = Paths::resolve()?;
    let verbose = matches!(
        std::env::var("XIAODAO_VERBOSE").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    );
    match logging::init(&paths, verbose) {
        Ok(guard) => {
            // 非阻塞写线程的守卫必须活到进程结束，否则日志丢尾；菜单栏应用无正常退出点，直接泄漏。
            Box::leak(Box::new(guard));
        }
        // 只在日志系统自身起不来时发生，此时还没有日志可写，只能打 stderr
        Err(e) => eprintln!("[启动错误] 日志初始化失败：{e:#}"),
    }
    tracing::info!("=== 小岛AI输入法启动 ===");
    tracing::info!("数据目录：{}", paths.base_dir.display());

    let store = SettingsStore::load(paths.settings_file.clone());
    let history = Arc::new(History::load(paths.history_file.clone(), store.clone()));
    let polisher = Arc::new(Polisher::new(store.clone()));
    app.manage(AppState {
        platform: platform::native(),
        paths,
        store,
        history,
        polisher,
        controller: OnceLock::new(),
        perm_missing: Mutex::new(Vec::new()),
    });
    Ok(())
}

/// 第二步：权限自检 + 后台启动线程（模型下载 / 加载 / 挂键钩）。
pub fn start(app: &AppHandle) {
    check_permissions(app);
    let handle = app.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("xiaodao-boot".into())
        .spawn(move || boot(handle))
    {
        tracing::error!("启动后台线程创建失败：{e}");
    }
}

fn boot(app: AppHandle) {
    let Some(state) = state(&app) else {
        tracing::error!("全局状态缺失，启动流程中止");
        return;
    };
    let events = TauriEvents::new(app.clone());
    let hud = TauriHud::new(app.clone());
    if !ensure_model(&app, &state, &events, &hud) {
        return;
    }
    let Some(transcriber) = load_model(&app, &state, &events) else {
        return;
    };
    launch(&app, &state, transcriber);
}

/// 模型就位：已存在直接过；缺失则下载（进度同时推托盘状态行与 HUD）。
fn ensure_model(app: &AppHandle, state: &AppState, events: &TauriEvents, hud: &TauriHud) -> bool {
    let filename = paths::model_filename();
    let first_run = !state.paths.model_path.is_file();
    if first_run {
        tracing::info!("模型缺失，后台自动下载：{MODEL_REPO} / {filename}");
        tray::set_status_text(app, "状态：正在下载模型（241MB，仅首次）…");
        events.notify(
            "首次运行：正在下载语音模型（约 241MB）",
            "完成后会通知你。国内网络慢可设环境变量 HF_ENDPOINT=https://hf-mirror.com",
        );
        hud.set_status("下载语音模型…", "首次运行，约 241MB");
    }
    let report = |p: Progress| {
        let Some(percent) = p.percent() else {
            return;
        };
        tray::set_status_text(app, &format!("状态：下载模型 {percent:.0}%"));
        hud.set_status(&format!("下载模型 {percent:.0}%"), &p.source);
    };
    let result =
        model_download::ensure_model(&state.paths.model_path, MODEL_REPO, &filename, &report);
    match result {
        Ok(()) => {
            if first_run {
                tracing::info!("模型下载完成：{}", state.paths.model_path.display());
                events.notify("模型下载完成", "语音输入已就绪 🏝️");
                hud.hide();
            }
            true
        }
        Err(e) => {
            tracing::error!("模型下载失败：{e:#}");
            tray::set_status_text(app, "状态：模型下载失败，见日志");
            events.notify(
                "模型下载失败",
                "请检查网络。国内可设 HF_ENDPOINT=https://hf-mirror.com 后重启本程序。",
            );
            hud.hide();
            false
        }
    }
}

/// 加载常驻模型（进程内只加载一次）。
fn load_model(
    app: &AppHandle,
    state: &AppState,
    events: &TauriEvents,
) -> Option<SharedTranscriber> {
    tray::set_status_text(app, "状态：加载模型中…");
    match Transcriber::load(&state.paths.model_path, paths::transcribe_language()) {
        Ok(t) => {
            tracing::info!("模型常驻就绪，加载耗时 {:.2}s", t.load_seconds());
            Some(Arc::new(t))
        }
        Err(e) => {
            tracing::error!("模型加载失败：{e:#}");
            tray::set_status_text(app, "状态：模型加载失败");
            events.notify(
                "模型加载失败",
                "见日志。若模型文件损坏，删掉 models/ 下的 gguf 后重启会自动重新下载。",
            );
            None
        }
    }
}

/// 组装状态机依赖并挂上全局键钩。
fn launch(app: &AppHandle, state: &AppState, transcriber: SharedTranscriber) {
    let settings = state.store.get();
    let polisher: SharedPolisher = state.polisher.clone();
    let history: Arc<dyn HistorySink> = state.history.clone();
    let view: Arc<dyn SettingsView> = Arc::new(state.store.clone());
    let deps = Deps {
        recorder: Box::new(CpalRecorder::new()),
        transcriber,
        polisher: Some(polisher),
        hud: Arc::new(TauriHud::new(app.clone())),
        events: Arc::new(TauriEvents::new(app.clone())),
        platform: state.platform.clone(),
        settings: view,
        history: Some(history),
        clock: system_clock(),
    };
    let controller = HotkeyController::new(
        deps,
        settings.hotkey,
        settings.rewrite_hotkey,
        settings.record_mode,
        Duration::from_secs_f64(MIN_HOLD_SECONDS),
    );
    state.set_controller(controller.clone());

    match listener::spawn(controller) {
        Ok(handle) => {
            // rdev 没有停止接口，监听线程随进程退出；句柄丢弃即分离线程。
            drop(handle);
            tracing::info!(
                "小岛AI输入法已就绪：热键「{}」，方式「{}」",
                settings.hotkey.display(),
                settings.record_mode.display()
            );
            if state.perm_missing().is_empty() {
                tray::set_status_text(app, "状态：待机");
            }
        }
        Err(e) => {
            tracing::error!("热键监听启动失败：{e:#}（通常是缺少「输入监听/辅助功能」权限）");
            tray::set_status_text(app, "状态：热键监听启动失败（见日志）");
            TauriEvents::new(app.clone()).notify(
                "热键监听启动失败",
                "请在「系统设置 → 隐私与安全性 → 输入监听 / 辅助功能」中授权本应用后重启。",
            );
        }
    }
}

// ---- 权限 ----

/// 测试钩子：`XIAODAO_FAKE_NO_PERM=1` 伪造「缺权限」，用于真机验证状态行/通知分支
/// 而不必真去撤销 TCC 授权（撤销会牵连整个终端宿主）。只影响自检结果的展示分支，
/// 不改动任何真实权限状态；未设置该环境变量时完全不生效。
fn fake_no_perm() -> bool {
    matches!(
        std::env::var("XIAODAO_FAKE_NO_PERM").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

/// 启动自检：缺失时触发系统授权弹窗（把本应用自动加进权限列表）并通知。
pub fn check_permissions(app: &AppHandle) {
    let Some(state) = state(app) else {
        return;
    };
    let mut perms = state.platform.check_permissions(true);
    if fake_no_perm() {
        tracing::debug!("测试钩子 XIAODAO_FAKE_NO_PERM=1 生效：伪造缺少输入监听 + 辅助功能");
        perms.input_monitoring = false;
        perms.accessibility = false;
    }
    if perms.all_granted() {
        return;
    }
    let mut missing = Vec::new();
    let mut names = Vec::new();
    if !perms.input_monitoring {
        missing.push(PrivacySection::InputMonitoring);
        names.push("输入监听");
    }
    if !perms.accessibility {
        missing.push(PrivacySection::Accessibility);
        names.push("辅助功能");
    }
    *lock(&state.perm_missing) = missing;
    let joined = names.join("、");
    tracing::warn!("权限自检未通过：缺少 {joined}");
    tray::set_status_text(app, &format!("状态：缺权限（{joined}）→ 点我去授权"));
    tray::set_status_clickable(app, true);
    TauriEvents::new(app.clone()).notify(
        &format!("缺少权限：{joined}"),
        "点菜单栏「状态」一行可直达系统设置对应页面；\
         若列表里已有旧条目仍无效，先移除再重新添加，然后重启本程序。",
    );
}

/// 功能性纠偏：真收到键盘事件 = 输入监听已通（自检 API 存在误报）。
pub fn on_first_key_event(app: &AppHandle) {
    let Some(state) = state(app) else {
        return;
    };
    let remaining = {
        let mut guard = lock(&state.perm_missing);
        if guard.is_empty() {
            return;
        }
        guard.retain(|s| *s != PrivacySection::InputMonitoring);
        guard.clone()
    };
    if remaining.is_empty() {
        tray::set_status_clickable(app, false);
        tray::set_status_text(app, "状态：待机");
        tracing::info!("权限状态已纠偏：实际事件已到，清除「缺权限」提示");
    } else {
        tray::set_status_text(app, "状态：缺权限（辅助功能）→ 点我去授权");
    }
}

/// 状态行点击：逐个打开缺失权限对应的系统设置面板（停在最后一个）。
pub fn open_missing_privacy(app: &AppHandle) {
    let Some(state) = state(app) else {
        return;
    };
    let mut missing = state.perm_missing();
    if missing.is_empty() {
        missing.push(PrivacySection::InputMonitoring);
    }
    for section in missing {
        state.platform.open_privacy_settings(section);
    }
}

/// 打开指定隐私设置面板（托盘「权限」子菜单与设置页共用）。
pub fn open_privacy(app: &AppHandle, section: &str) {
    let Some(target) = parse_section(section) else {
        tracing::warn!("未知权限面板：{section}");
        return;
    };
    let Some(state) = state(app) else {
        return;
    };
    tracing::info!("打开系统隐私设置：{section}");
    state.platform.open_privacy_settings(target);
}

fn parse_section(section: &str) -> Option<PrivacySection> {
    Some(match section {
        "input_monitoring" => PrivacySection::InputMonitoring,
        "accessibility" => PrivacySection::Accessibility,
        "microphone" => PrivacySection::Microphone,
        _ => return None,
    })
}

// ---- 配置生效 ----

/// 把 settings 当前值推给热键控制器并刷新菜单勾选（设置页保存 / 重新加载配置共用）。
pub fn apply_settings(app: &AppHandle) {
    let Some(state) = state(app) else {
        return;
    };
    let settings = state.store.get();
    if let Some(controller) = state.controller() {
        controller.set_trigger(settings.hotkey);
        controller.set_rewrite_trigger(settings.rewrite_hotkey);
        controller.set_mode(settings.record_mode);
    }
    tray::sync_menu_state(app);
}

/// 菜单「退出」：先停预览/声浪线程，再退出应用。
pub fn quit(app: &AppHandle) {
    tracing::info!("用户点击退出");
    if let Some(state) = state(app) {
        if let Some(controller) = state.controller() {
            controller.stop();
        }
    }
    app.exit(0);
}
