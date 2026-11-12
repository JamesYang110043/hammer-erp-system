import { defineConfig, devices } from '@playwright/test';

/**
 * 前端 E2E（docs/testing.md §1 L3）：對 Vite dev server 執行，Tauri command 以 mock 回應（e2e/tauri-mock.ts）。
 * 真實桌面程式的 E2E（tauri-driver）M4 才導入。
 */

// 與 vite.config.ts 的 server.port 相同
const PORT = 1420;
const isCI = !!process.env.CI;

export default defineConfig({
  testDir: './e2e',
  fullyParallel: true,
  forbidOnly: isCI,
  retries: 0,
  // 截圖與 trace 存在 test-results/，CI 失敗時上傳為 artifact
  outputDir: './test-results',
  reporter: isCI
    ? [['github'], ['html', { open: 'never' }]]
    : [['list'], ['html', { open: 'never' }]],
  use: {
    baseURL: `http://localhost:${PORT}`,
    locale: 'zh-TW',
    timezoneId: 'Asia/Taipei',
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  webServer: {
    command: 'pnpm dev',
    url: `http://localhost:${PORT}`,
    // 本機已經開著 `pnpm dev` 時直接使用
    reuseExistingServer: !isCI,
    timeout: 60_000,
  },
});
