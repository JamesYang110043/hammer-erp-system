import { expect, test } from '@playwright/test';

import { CommandRejection, mockTauri } from './tauri-mock';

const SAVED_URL = 'http://localhost:8080';
const OTHER_URL = 'http://192.168.1.10:8080';

test('main screen shows connection status and test connection works', async ({ page }) => {
  await mockTauri(page, {
    get_server_url: () => SAVED_URL,
    // 已登入（鑰匙圈有有效的 token）
    setup_status: () => ({ needs_setup: false }),
    current_user: () => ({
      id: '0196f5a0-0000-7000-8000-000000000001',
      username: 'admin',
      display_name: '王管理',
      role: 'admin',
      permissions: ['user.manage'],
      must_change_password: false,
    }),
    health_check: ({ serverUrl }) => {
      // 狀態列輪詢（serverUrl 為 null）使用已儲存的網址
      if (serverUrl === null || serverUrl === OTHER_URL) {
        return { status: 'ok', version: '0.1.0', database: 'ok' };
      }
      throw new CommandRejection({ kind: 'network', message: '無法連線到伺服器' });
    },
  });

  // 主畫面
  await page.goto('/');
  await expect(page.getByText('Hammer ERP')).toBeVisible();
  await expect(page.getByText('已連線', { exact: true })).toBeVisible();
  await expect(page.getByText('伺服器 v0.1.0')).toBeVisible();

  // 設定頁：測試另一個網址
  await page.getByRole('button', { name: '連線設定' }).click();
  const input = page.getByLabel('伺服器網址');
  await expect(input).toHaveValue(SAVED_URL);
  await input.fill(OTHER_URL);
  await page.getByRole('button', { name: '測試連線' }).click();
  await expect(page.getByText('連線成功，伺服器版本 v0.1.0')).toBeVisible();

  // 連不到的網址顯示錯誤訊息
  await input.fill('http://10.0.0.99:8080');
  await page.getByRole('button', { name: '測試連線' }).click();
  await expect(page.getByRole('alert')).toContainText('無法連線到伺服器');
});
