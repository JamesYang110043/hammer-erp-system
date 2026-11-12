import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';

// Tauri 開發時使用的設定，見 https://v2.tauri.app/start/frontend/vite/
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [react()],
  // 避免 Vite 清掉 Rust 的編譯錯誤訊息
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host ?? false,
    hmr: host ? { protocol: 'ws', host, port: 1421 } : undefined,
    watch: {
      // Rust 端由 Tauri 監看，Vite 不需要
      ignored: ['**/src-tauri/**'],
    },
  },
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  build: {
    // Tauri 在 Windows 用 WebView2（Chromium），macOS 用 WebKit
    target: process.env.TAURI_ENV_PLATFORM === 'windows' ? 'chrome105' : 'safari13',
    minify: process.env.TAURI_ENV_DEBUG ? false : 'oxc',
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
  // 前端單元測試（docs/testing.md §1）；Playwright E2E 在 e2e/，不由 Vitest 執行
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.{ts,tsx}'],
    setupFiles: ['src/test/setup.ts'],
    restoreMocks: true,
    coverage: {
      provider: 'v8',
      include: ['src/**/*.{ts,tsx}'],
      // bindings.ts 為自動產生；main.tsx 只負責掛載；測試輔助程式不計入
      exclude: ['src/bindings.ts', 'src/main.tsx', 'src/test/**', 'src/**/*.test.{ts,tsx}'],
      // 前端覆蓋率只回報數字，不設門檻（docs/testing.md §1.2）
      reporter: ['text', 'text-summary'],
    },
  },
});
