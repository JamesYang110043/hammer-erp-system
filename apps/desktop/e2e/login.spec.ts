import { expect, test } from '@playwright/test';

import { CommandRejection, mockTauri } from './tauri-mock';

const ADMIN = {
  id: '0196f5a0-0000-7000-8000-000000000001',
  username: 'admin',
  display_name: '王管理',
  role: 'admin',
  permissions: ['audit.read', 'settings.manage', 'user.manage'],
  must_change_password: false,
};

// antd 會在兩個中文字的按鈕中間插入空白
const LOGIN_BUTTON = /^登\s?入$/;

test('setup admin, logout, login, and stay logged in after reload', async ({ page }) => {
  // 模擬伺服器與鑰匙圈：`loggedIn` 代表 Rust 端是否存有 token
  let hasAdmin = false;
  let loggedIn = false;
  await mockTauri(page, {
    get_server_url: () => 'http://localhost:8080',
    health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
    setup_status: () => ({ needs_setup: !hasAdmin }),
    setup_admin: () => {
      hasAdmin = true;
      loggedIn = true;
      return ADMIN;
    },
    current_user: () => (loggedIn ? ADMIN : null),
    logout: () => {
      loggedIn = false;
      return null;
    },
    login: ({ req }) => {
      const { password } = req as { password: string };
      if (password !== 'admin-password') {
        throw new CommandRejection({
          kind: 'api',
          code: 'UNAUTHORIZED',
          message: '帳號或密碼錯誤',
        });
      }
      loggedIn = true;
      return ADMIN;
    },
  });

  // 空資料庫 → 建立管理者
  await page.goto('/');
  await page.getByLabel('帳號').fill('admin');
  await page.getByLabel('顯示名稱').fill('王管理');
  await page.getByLabel('密碼', { exact: true }).fill('admin-password');
  await page.getByLabel('確認密碼').fill('admin-password');
  await page.getByRole('button', { name: '建立並登入' }).click();
  await expect(page.getByRole('menuitem', { name: /帳號管理/ })).toBeVisible();

  // 登出
  await page.getByRole('button', { name: /王管理/ }).click();
  await page.getByRole('menuitem', { name: /登出/ }).click();
  await expect(page.getByRole('button', { name: LOGIN_BUTTON })).toBeVisible();

  // 密碼錯誤 → 正確密碼登入
  await page.getByLabel('帳號').fill('admin');
  await page.getByLabel('密碼').fill('wrong-password');
  await page.getByRole('button', { name: LOGIN_BUTTON }).click();
  await expect(page.getByText('帳號或密碼錯誤')).toBeVisible();
  await page.getByLabel('密碼').fill('admin-password');
  await page.getByRole('button', { name: LOGIN_BUTTON }).click();
  await expect(page.getByRole('button', { name: /王管理/ })).toBeVisible();

  // 重新開啟（重新載入頁面）仍保持登入
  await page.reload();
  await expect(page.getByRole('button', { name: /王管理/ })).toBeVisible();
  await expect(page.getByRole('button', { name: LOGIN_BUTTON })).toHaveCount(0);
});
