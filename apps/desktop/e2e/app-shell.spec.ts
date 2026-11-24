import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

import { PRODUCT, STOCK_ADJUSTMENT, STOCK_ADJUSTMENT_FIELDS } from '../src/test/documentFixtures';
import { mockTauri } from './tauri-mock';

const ADMIN = {
  id: '0196f5a0-0000-7000-8000-000000000001',
  username: 'admin',
  display_name: '王管理',
  role: 'admin',
  permissions: ['record.post', 'record.read', 'record.write', 'schema.manage', 'user.manage'],
  must_change_password: false,
};

const emptyPage = { items: [], total: 0, page: 1, page_size: 50 };

async function mockServer(page: Page) {
  await mockTauri(page, {
    get_server_url: () => 'http://localhost:8080',
    health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
    setup_status: () => ({ needs_setup: false }),
    current_user: () => ADMIN,
    list_entities: () => [PRODUCT, STOCK_ADJUSTMENT],
    get_entity: () => ({ entity: STOCK_ADJUSTMENT, fields: STOCK_ADJUSTMENT_FIELDS }),
    get_default_layout: () => ({
      source: 'generated',
      layout: null,
      config: {
        columns: ['adjustment_date', 'status'].map((k) => ({ field_key: k, width: null })),
        sort: null,
        filters: [],
      },
    }),
    list_layouts: () => [],
    list_records: () => emptyPage,
    list_stock_balances: () => emptyPage,
    list_users: () => [],
  });
}

test('breadcrumb shows where the user is', async ({ page }) => {
  await mockServer(page);
  const breadcrumb = page.getByRole('navigation', { name: '目前位置' });

  // 主檔
  await page.goto('/#/entities/product');
  await expect(breadcrumb).toHaveText('基本資料/商品');

  // 單據與明細
  await page.goto('/#/entities/stock_adjustment');
  await expect(breadcrumb).toHaveText('庫存/盤點調整單');
  await page.goto('/#/entities/stock_adjustment/new');
  await expect(breadcrumb).toHaveText('庫存/盤點調整單/新增');
  await breadcrumb.getByRole('link', { name: '盤點調整單' }).click();
  await expect(page).toHaveURL(/#\/entities\/stock_adjustment$/);

  // 資料類別（欄位設定顯示資料類別名稱）
  await page.goto('/#/schema/stock_adjustment');
  await expect(breadcrumb).toHaveText('資料類別/盤點調整單');

  // 庫存查詢
  await page.goto('/#/stock');
  await expect(breadcrumb).toHaveText('庫存/庫存查詢');

  // 不在選單中的頁面
  await page.goto('/#/connection');
  await expect(breadcrumb).toHaveText('連線設定');

  // 使用者選單：Avatar 顯示名稱首字
  await expect(page.getByRole('button', { name: /王管理/ })).toContainText('王');
});

test('sidebar collapses to icons and remembers the state', async ({ page }) => {
  await mockServer(page);
  await page.goto('/#/entities/stock_adjustment');

  const logo = page.getByTestId('app-logo');
  const nav = page.getByRole('navigation', { name: '主選單' });
  await expect(logo.getByText('Hammer ERP')).toBeVisible();
  // 深色側邊欄：選中項目為主色底
  await expect(nav.getByRole('menuitem', { name: /盤點調整單/ })).toHaveCSS(
    'background-color',
    'rgb(31, 78, 140)',
  );
  const expandedWidth = (await nav.boundingBox())!.width;

  await page.getByRole('button', { name: '收合側邊欄' }).click();
  await expect(logo.getByText('Hammer ERP')).toHaveCount(0);
  await expect.poll(async () => (await nav.boundingBox())!.width).toBeLessThan(expandedWidth / 2);

  // 收合時滑過圖示顯示名稱
  await nav.getByRole('menuitem', { name: /帳號管理/ }).hover();
  await expect(page.getByRole('tooltip').filter({ hasText: '帳號管理' })).toBeVisible();

  // 重新開啟後維持收合
  await page.reload();
  await expect(page.getByRole('button', { name: '展開側邊欄' })).toBeVisible();
  await expect(logo.getByText('Hammer ERP')).toHaveCount(0);

  // 收合時區塊以浮出選單開啟
  await nav.locator('.nav-section').first().hover();
  await expect(page.getByRole('menuitem', { name: /商品/ })).toBeVisible();
  // 等浮出動畫結束再截圖
  await expect(page.locator('.ant-menu-submenu-popup')).toHaveCSS('opacity', '1');
  if (process.env.E2E_SCREENSHOT) {
    await page.screenshot({ path: test.info().outputPath('collapsed.png') });
  }
  await page.mouse.move(800, 400);

  // 展開回來，區塊的展開狀態不受影響
  await page.getByRole('button', { name: '展開側邊欄' }).click();
  await expect(logo.getByText('Hammer ERP')).toBeVisible();
  await expect(nav.getByRole('menuitem', { name: '庫存', exact: true })).toHaveAttribute(
    'aria-expanded',
    'true',
  );
  // 等展開動畫結束（截圖前）
  await expect.poll(async () => (await nav.boundingBox())!.width).toBe(expandedWidth);
  await expect(page.locator('.ant-menu-submenu-popup').filter({ visible: true })).toHaveCount(0);
});
