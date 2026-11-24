import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

import { CommandRejection, mockTauri } from './tauri-mock';
import type { E2eCommandHandler } from './tauri-mock';

const USER = {
  id: '0196f5a0-0000-7000-8000-000000000002',
  username: 'clerk',
  display_name: '陳小明',
  role: 'clerk',
  permissions: ['record.read'],
  must_change_password: true,
};

const base: Record<string, E2eCommandHandler> = {
  get_server_url: () => 'http://localhost:8080',
  health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
  setup_status: () => ({ needs_setup: false }),
  current_user: () => null,
};

/** 四種登入前的畫面：開啟後等卡片標題出現。 */
const SCREENS: { name: string; title: string; commands: Record<string, E2eCommandHandler> }[] = [
  { name: 'setup', title: '建立管理者', commands: { setup_status: () => ({ needs_setup: true }) } },
  { name: 'login', title: '登入', commands: {} },
  { name: 'change-password', title: '修改密碼', commands: { current_user: () => USER } },
  {
    name: 'connection-failed',
    title: '無法連線',
    commands: {
      setup_status: () => {
        throw new CommandRejection({ kind: 'network', message: '無法連線到伺服器' });
      },
    },
  },
];

async function open(page: Page, commands: Record<string, E2eCommandHandler>, title: string) {
  await mockTauri(page, { ...base, ...commands });
  await page.goto('/');
  await expect(page.locator('.ant-card-head-title', { hasText: title })).toBeVisible({
    timeout: 15_000,
  });
}

/** 畫面驗證用的額外截圖（E2E_SCREENSHOT=1 時）。 */
async function snapshot(page: Page, name: string) {
  if (process.env.E2E_SCREENSHOT) {
    await page.screenshot({ path: test.info().outputPath(`${name}.png`) });
  }
}

const hasHorizontalScroll = (page: Page) =>
  page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth);

for (const screen of SCREENS) {
  test(`${screen.name}: brand panel on wide window, card only on narrow window`, async ({
    page,
  }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await open(page, screen.commands, screen.title);

    // 寬視窗：左側品牌區塊（Logo、名稱、標語），右上角連線設定
    const brand = page.getByTestId('auth-brand');
    await expect(brand).toBeVisible();
    await expect(brand.getByRole('heading', { name: 'Hammer ERP', level: 1 })).toBeVisible();
    await expect(brand.getByText('進銷存與庫存管理系統')).toBeVisible();
    await expect(page.getByRole('button', { name: /連線設定/ })).toBeVisible();
    await expect(page.getByText('已連線').or(page.getByText('未連線'))).toBeVisible();

    // 最小視窗：兩欄都在，不出現水平捲軸
    await page.setViewportSize({ width: 1024, height: 640 });
    await expect(brand).toBeVisible();
    expect(await hasHorizontalScroll(page)).toBe(false);
    await snapshot(page, 'min-window');

    // 窄視窗：只顯示表單卡片，Logo 移到卡片上方
    await page.setViewportSize({ width: 720, height: 640 });
    await expect(brand).toBeHidden();
    await expect(page.locator('.auth-compact-logo')).toBeVisible();
    expect(await hasHorizontalScroll(page)).toBe(false);
    await snapshot(page, 'narrow-window');

    await page.setViewportSize({ width: 1280, height: 800 });
    await expect(brand).toBeVisible();
  });
}

test('connection settings open from the login screen', async ({ page }) => {
  await open(page, {}, '登入');
  await page.getByRole('button', { name: /連線設定/ }).click();
  await expect(page.getByRole('dialog').getByLabel('伺服器網址')).toBeVisible();
});
