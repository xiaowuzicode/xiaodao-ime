import { defineConfig } from "vite";

// Tauri 多页：index.html = HUD 悬浮窗（"/"），settings.html = 设置窗口。
// 全部资源本地打包，禁止任何 CDN / 远程字体。
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: "127.0.0.1",
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    // WKWebView(macOS 12) / WebView2 都支持的下限
    target: "safari15",
    emptyOutDir: true,
    rollupOptions: {
      // 相对 root（本目录）解析，避免在 config 里依赖 node 类型
      input: { index: "index.html", settings: "settings.html" },
    },
  },
});
