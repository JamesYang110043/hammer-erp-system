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
    // E2E_SCREENSHOT=1 時每個測試結束都截圖（畫面驗證，docs/issue-workflow.md §6）
    screenshot: process.env.E2E_SCREENSHOT ? 'on' : 'only-on-failure',
    trace: 'retain-on-failure',
  },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
    // 深色模式（#234）：系統為深色、外觀為預設的「跟隨系統」，主要畫面再跑一次並截圖
    {
      name: 'chromium-dark',
      use: { ...devices['Desktop Chrome'], colorScheme: 'dark' },
      testMatch: [
        'auth-layout.spec.ts',
        'documents.spec.ts',
        'form-layout.spec.ts',
        'form-layout-designer.spec.ts',
        'master-split.spec.ts',
        'page-style.spec.ts',
      ],
    },
  ],
  webServer: {
    command: 'pnpm dev',
    url: `http://localhost:${PORT}`,
    // 本機已經開著 `pnpm dev` 時直接使用
    reuseExistingServer: !isCI,
    timeout: 60_000,
  },
});
