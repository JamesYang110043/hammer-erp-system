import { expect, test } from '@playwright/test';
import type { Locator } from '@playwright/test';

import type { EntityDefinition } from '../src/bindings';
import { PRODUCT, STOCK_ADJUSTMENT } from '../src/test/documentFixtures';
import { mockTauri } from './tauri-mock';

const ADMIN = {
  id: '0196f5a0-0000-7000-8000-000000000001',
  username: 'admin',
  display_name: '王管理',
  role: 'admin',
  permissions: [
    'audit.read',
    'record.post',
    'record.read',
    'record.write',
    'schema.manage',
    'settings.manage',
    'user.manage',
  ],
  must_change_password: false,
};

/** 自訂資料類別，讓選單長到超過視窗高度。 */
const CUSTOM: EntityDefinition[] = Array.from({ length: 8 }, (_, i) => ({
  ...PRODUCT,
  id: `0196f5a0-0000-7000-8000-0000000010${String(i).padStart(2, '0')}`,
  key: `custom_${i}`,
  label: `自訂類別 ${i + 1}`,
  is_system: false,
  storage: 'custom_records',
  sort_order: 100 + i,
}));

const scrollTop = (l: Locator) => l.evaluate((el) => el.scrollTop);

test('side menu scrolls independently of the content area', async ({ page }) => {
  await mockTauri(page, {
    get_server_url: () => 'http://localhost:8080',
    health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
    setup_status: () => ({ needs_setup: false }),
    current_user: () => ADMIN,
    list_entities: () => [PRODUCT, STOCK_ADJUSTMENT, ...CUSTOM],
  });
  // 矮視窗：選單放不下
  await page.setViewportSize({ width: 1280, height: 420 });
  await page.goto('/');

  const nav = page.getByTestId('app-nav');
  const content = page.getByTestId('app-content');
  const title = page.getByRole('heading', { name: 'Hammer ERP' });
  const header = page.getByRole('button', { name: /連線設定/ });
  const last = page.getByRole('menuitem', { name: /系統設定/ });
  await expect(last).toBeAttached();

  // 選單超過可視高度，需要捲動
  const overflow = await nav.evaluate((el) => el.scrollHeight - el.clientHeight);
  expect(overflow).toBeGreaterThan(0);

  // 右側內容也放一段很長的內容，確認兩邊互不連動
  await content.evaluate((el) => {
    const filler = document.createElement('div');
    filler.style.height = '3000px';
    el.appendChild(filler);
  });

  const titleBox = await title.boundingBox();
  const headerBox = await header.boundingBox();
  const statusBox = await page.locator('.ant-layout-footer').boundingBox();

  // 捲動選單（捲到底後繼續捲），內容區不動
  await nav.hover();
  for (let i = 0; i < 5; i++) {
    await page.mouse.wheel(0, 400);
  }
  await expect.poll(() => scrollTop(nav)).toBeGreaterThanOrEqual(overflow - 1);
  expect(await scrollTop(content)).toBe(0);

  // 最下方的項目可以點到
  await expect(last).toBeInViewport();
  await last.click();
  await expect(page).toHaveURL(/#\/system$/);

  // 標題、上方標題列、底部狀態列位置不變
  expect(await title.boundingBox()).toEqual(titleBox);
  expect(await header.boundingBox()).toEqual(headerBox);
  expect(await page.locator('.ant-layout-footer').boundingBox()).toEqual(statusBox);

  // 反過來：捲動內容區（捲到底後繼續捲），選單不動
  const navTop = await scrollTop(nav);
  await content.hover();
  for (let i = 0; i < 12; i++) {
    await page.mouse.wheel(0, 600);
  }
  await expect.poll(() => scrollTop(content)).toBeGreaterThan(0);
  expect(await scrollTop(nav)).toBe(navTop);
  expect(await title.boundingBox()).toEqual(titleBox);
  expect(await page.locator('.ant-layout-footer').boundingBox()).toEqual(statusBox);
});
