import { expect, test } from '@playwright/test';
import type { Locator, Page } from '@playwright/test';

import { ROLES, managedUser } from '../src/test/fixtures';
import { REPAIR } from '../src/test/schemaFixtures';
import { mockTauri } from './tauri-mock';

const ADMIN = {
  id: '0196f5a0-0000-7000-8000-000000000001',
  username: 'admin',
  display_name: '王管理',
  role: 'admin',
  permissions: ['record.read', 'record.write', 'schema.manage', 'user.manage'],
  must_change_password: false,
};

async function mockServer(page: Page) {
  await mockTauri(page, {
    get_server_url: () => 'http://localhost:8080',
    health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
    setup_status: () => ({ needs_setup: false }),
    current_user: () => ADMIN,
    list_entities: () => [REPAIR],
    list_all_entities: () => [REPAIR],
    get_entity: () => ({ entity: REPAIR, fields: [] }),
    list_users: () => [managedUser()],
    list_roles: () => ROLES,
  });
}

/** Modal 開啟動畫結束（量位置前）。 */
async function dialogReady(page: Page) {
  const dialog = page.getByRole('dialog');
  await expect(dialog).toBeVisible();
  await expect(page.locator('.ant-zoom-appear, .ant-zoom-enter')).toHaveCount(0);
  return dialog;
}

/** 欄位（`.ant-form-item`）的位置。 */
async function itemBox(dialog: Locator, label: string) {
  const item = dialog
    .locator('.ant-form-item')
    .filter({ has: dialog.page().locator('label').getByText(label, { exact: true }) });
  const box = await item.boundingBox();
  expect(box, label).not.toBeNull();
  return box!;
}

/** `a`、`b` 在同一列，`a` 在左邊。 */
async function expectSameRow(dialog: Locator, a: string, b: string) {
  const [boxA, boxB] = [await itemBox(dialog, a), await itemBox(dialog, b)];
  expect(Math.abs(boxA.y - boxB.y), `${a} / ${b} 同一列`).toBeLessThanOrEqual(1);
  expect(boxA.x + boxA.width, `${a} 在 ${b} 左邊`).toBeLessThanOrEqual(boxB.x);
}

/** `b` 在 `a` 的下一列。 */
async function expectBelow(dialog: Locator, a: string, b: string) {
  const [boxA, boxB] = [await itemBox(dialog, a), await itemBox(dialog, b)];
  expect(boxB.y, `${b} 在 ${a} 下方`).toBeGreaterThanOrEqual(boxA.y + boxA.height - 1);
}

test('create user modal: two columns, tab order, validation messages', async ({ page }) => {
  await mockServer(page);
  await page.goto('/#/users');
  await page.getByRole('button', { name: /新增帳號/ }).click();
  const dialog = await dialogReady(page);

  // 帳號｜顯示名稱、角色｜臨時密碼
  await expectSameRow(dialog, '帳號', '顯示名稱');
  await expectSameRow(dialog, '角色', '臨時密碼');
  await expectBelow(dialog, '帳號', '角色');

  // Tab 鍵：左到右、上到下
  await dialog.getByLabel('帳號').focus();
  await page.keyboard.press('Tab');
  await expect(dialog.getByLabel('顯示名稱')).toBeFocused();
  await page.keyboard.press('Tab');
  await expect(dialog.getByLabel('角色')).toBeFocused();

  // 驗證訊息：顯示在各自的欄位下方，同一列的欄位位置不變、不重疊
  const before = await itemBox(dialog, '顯示名稱');
  await dialog.getByLabel('顯示名稱').fill('陳小明');
  await page.getByRole('button', { name: /建\s?立/ }).click();
  await expect(dialog.getByText('請輸入帳號')).toBeVisible();
  await expect(dialog.getByText('請選擇角色')).toBeVisible();
  const after = await itemBox(dialog, '顯示名稱');
  expect(after.x).toBe(before.x);
  expect(after.width).toBe(before.width);
  await expectSameRow(dialog, '帳號', '顯示名稱');
  await expectBelow(dialog, '帳號', '角色');
  const error = await dialog.getByText('請輸入帳號').boundingBox();
  expect(error!.x + error!.width).toBeLessThanOrEqual(after.x);
});

test('entity modal: long fields span the full row', async ({ page }) => {
  await mockServer(page);
  await page.goto('/#/schema');
  await page.getByRole('button', { name: /新增資料類別/ }).click();
  const dialog = await dialogReady(page);

  await expectSameRow(dialog, '代碼', '名稱');
  await expectSameRow(dialog, '圖示', '種類');
  // 標題範本（說明很長）佔滿整列
  const code = await itemBox(dialog, '代碼');
  const name = await itemBox(dialog, '名稱');
  const template = await itemBox(dialog, '標題範本');
  expect(template.x).toBe(code.x);
  expect(Math.round(template.x + template.width)).toBe(Math.round(name.x + name.width));
});

test('field modal: text area spans the full row', async ({ page }) => {
  await mockServer(page);
  await page.goto('/#/schema/repair');
  await page.getByRole('button', { name: /新增欄位/ }).click();
  const dialog = await dialogReady(page);

  await expectSameRow(dialog, '欄位代碼', '顯示名稱');
  const key = await itemBox(dialog, '欄位代碼');
  const help = await itemBox(dialog, '說明文字');
  expect(help.x).toBe(key.x);
  expect(help.width).toBeGreaterThan(key.width * 2);
});

test('narrow window falls back to one column without horizontal scroll', async ({ page }) => {
  await mockServer(page);
  await page.setViewportSize({ width: 480, height: 800 });
  await page.goto('/#/users');
  await page.getByRole('button', { name: /新增帳號/ }).click();
  const dialog = await dialogReady(page);

  await expectBelow(dialog, '帳號', '顯示名稱');
  await expectBelow(dialog, '顯示名稱', '角色');
  expect((await itemBox(dialog, '顯示名稱')).x).toBe((await itemBox(dialog, '帳號')).x);

  const overflow = await page.evaluate(() => {
    const body = document.querySelector('.ant-modal-body')!;
    return {
      page: document.documentElement.scrollWidth - document.documentElement.clientWidth,
      modal: body.scrollWidth - body.clientWidth,
    };
  });
  expect(overflow).toEqual({ page: 0, modal: 0 });
});
