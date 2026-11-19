import { expect, test } from '@playwright/test';

import { PRODUCT, STOCK_ADJUSTMENT, STOCK_ADJUSTMENT_FIELDS } from '../src/test/documentFixtures';
import { openNavSection } from './nav';
import { mockTauri } from './tauri-mock';

const ADMIN = {
  id: '0196f5a0-0000-7000-8000-000000000001',
  username: 'admin',
  display_name: '王管理',
  role: 'admin',
  permissions: ['record.post', 'record.read', 'record.write'],
  must_change_password: false,
};

test('menu sections collapse, follow the current page, and survive a restart', async ({ page }) => {
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
    list_records: () => ({ items: [], total: 0, page: 1, page_size: 50 }),
  });

  const nav = page.getByRole('navigation', { name: '主選單' });
  const section = (label: string) => nav.getByRole('menuitem', { name: label, exact: true });
  const item = (name: RegExp) => nav.getByRole('menuitem', { name });

  // 直接進入盤點調整單：「庫存」區塊展開且項目為選取狀態，「基本資料」收起
  await page.goto('/#/entities/stock_adjustment');
  await expect(section('庫存')).toHaveAttribute('aria-expanded', 'true');
  await expect(item(/盤點調整單/)).toHaveClass(/ant-menu-item-selected/);
  await expect(section('基本資料')).toHaveAttribute('aria-expanded', 'false');
  await expect(item(/商品/)).toBeHidden();

  // 展開「基本資料」，收起「庫存」：收起後只剩區塊標題
  await openNavSection(page, '基本資料');
  await expect(item(/商品/)).toBeVisible();
  await section('庫存').click();
  await expect(section('庫存')).toHaveAttribute('aria-expanded', 'false');
  await expect(item(/盤點調整單/)).toBeHidden();
  await expect(section('庫存')).toBeVisible();

  // 切換到其他區塊的頁面（從畫面上的連結或網址）→ 自動展開
  await page.goto('/#/stock');
  await expect(section('庫存')).toHaveAttribute('aria-expanded', 'true');
  await expect(item(/庫存查詢/)).toHaveClass(/ant-menu-item-selected/);

  // 收起「庫存」後重新開啟程式（不在任何區塊的頁面）：維持上次的展開狀態
  await page.goto('/#/connection');
  await section('庫存').click();
  await expect(section('庫存')).toHaveAttribute('aria-expanded', 'false');
  await page.reload();
  await expect(section('基本資料')).toHaveAttribute('aria-expanded', 'true');
  await expect(section('庫存')).toHaveAttribute('aria-expanded', 'false');
  await expect(item(/商品/)).toBeVisible();
  await expect(item(/盤點調整單/)).toBeHidden();
});
