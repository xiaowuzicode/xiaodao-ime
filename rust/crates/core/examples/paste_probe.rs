//! 真机自检小工具：`cargo run -p xiaodao-core --example paste_probe`
//!
//! 打印前台应用、TCC 权限自检结果、当前剪贴板内容；
//! 加 `--capture` 发一次 复制快捷键（无选区时是无操作，用来验证按键注入通路）；
//! 加 `--paste` 参数时会真的往当前焦点窗口粘贴一句话（都需要辅助功能权限）。
//!
//! 只读路径不需要任何授权，权限没给也只会打印「未授权」而不是崩溃。

use xiaodao_core::paster;
use xiaodao_core::platform::{self, SharedPlatform};
use xiaodao_core::types::SoundEvent;

fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_file(true)
        .with_line_number(true)
        .init();

    let platform: SharedPlatform = platform::native();

    println!("== 前台应用 ==");
    let app = platform.frontmost_app();
    println!("  name = {:?}", app.name);
    println!("  id   = {:?}", app.id);

    println!("== 权限自检（不弹窗）==");
    let perms = platform.check_permissions(false);
    println!("  输入监听 input_monitoring = {}", perms.input_monitoring);
    println!("  辅助功能 accessibility    = {}", perms.accessibility);
    println!("  全部就绪 = {}", perms.all_granted());

    println!("== 剪贴板 ==");
    match platform.read_clipboard() {
        Some(text) => {
            let preview: String = text.chars().take(40).collect();
            println!(
                "  当前内容（前 40 字）= {preview:?}（共 {} 字）",
                text.chars().count()
            );
        }
        None => println!("  当前无文本内容"),
    }

    println!("== 提示音（Tink）==");
    platform.play_sound(SoundEvent::Start);

    if std::env::args().any(|a| a == "--capture") {
        // 只发 Cmd+C：没有选区时对目标 App 是无操作，用来验证 CGEvent 注入这条路通不通。
        // 走的是完整事务，结束后原剪贴板会被还回去。
        println!("== 抓选区测试：3 秒后往当前焦点窗口发 复制快捷键 ==");
        std::thread::sleep(std::time::Duration::from_secs(3));
        let mut capture = paster::capture_selection(&platform);
        println!("  抓到的选区 = {:?}", capture.text);
        capture.restore(&platform);
        println!("  恢复后剪贴板 = {:?}", platform.read_clipboard().is_some());
    }

    if std::env::args().any(|a| a == "--paste") {
        println!("== 粘贴测试：3 秒后往当前焦点窗口粘贴 ==");
        std::thread::sleep(std::time::Duration::from_secs(3));
        let ok = paster::paste_text(&platform, "小岛AI输入法 Rust 版粘贴自检");
        println!("  paste_text -> {ok}");
        // 等延迟恢复线程跑完再退出，否则进程结束时原剪贴板还没还回去
        std::thread::sleep(paster::CLIPBOARD_RESTORE_DELAY + std::time::Duration::from_millis(200));
        println!("  恢复后剪贴板 = {:?}", platform.read_clipboard());
    } else {
        println!("（加 --capture / --paste 可做真实按键注入测试）");
    }
}
