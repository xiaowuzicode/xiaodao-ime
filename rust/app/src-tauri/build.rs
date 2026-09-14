fn main() {
    link_clang_runtime();
    tauri_build::build()
}

/// macOS：显式链接 clang 的 compiler-rt。
///
/// ggml-metal 的 ObjC 代码里有 `@available(...)`，clang 会为它生成对
/// `___isPlatformVersionAtLeast` 的引用，该符号只在 `libclang_rt.osx.a` 里。
/// dev 档能链上，但 release 档开了 `lto = "thin"` 之后就会报
/// `Undefined symbols: ___isPlatformVersionAtLeast`，所以这里主动把运行时库加进链接搜索路径。
/// 找不到 clang 或运行时目录时静默跳过（非 macOS、或交叉编译环境）。
fn link_clang_runtime() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let output = std::process::Command::new("clang")
        .arg("-print-runtime-dir")
        .output();
    let Ok(output) = output else {
        println!("cargo:warning=未找到 clang，跳过 compiler-rt 链接");
        return;
    };
    let dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if dir.is_empty()
        || !std::path::Path::new(&dir)
            .join("libclang_rt.osx.a")
            .is_file()
    {
        println!("cargo:warning=未找到 libclang_rt.osx.a（{dir}），跳过 compiler-rt 链接");
        return;
    }
    println!("cargo:rustc-link-search=native={dir}");
    println!("cargo:rustc-link-lib=static=clang_rt.osx");
}
