import { expect, test } from '@playwright/test';

import {
  ADJUSTMENT_ID,
  STOCK_ADJUSTMENT,
  STOCK_ADJUSTMENT_FIELDS,
  adjustment,
} from '../src/test/documentFixtures';
import { mockTauri } from './tauri-mock';

/**
 * 主色方案比較（#229）：以 `?theme=` 切換三套方案，對登入、列表、單據表單截圖。
 * 截圖存在 test-results/，供使用者選定主色。
 */

const ADMIN = {
  id: '0196f5a0-0000-7000-8000-000000000001',
  username: 'admin',
  display_name: '王管理',
  role: 'admin',
  permissions: ['record.post', 'record.read', 'record.write', 'user.manage', 'settings.manage'],
  must_change_password: false,
};

// 自訂品牌色以紫紅示範 antd 色彩演算法推導的效果
const PRESETS = [
  { name: 'navy', primary: 'rgb(31, 78, 140)' },
  { name: 'teal', primary: 'rgb(15, 139, 141)' },
  { name: '8E3B8A', primary: 'rgb(142, 59, 138)' },
];

for (const preset of PRESETS) {
  test(`theme preset ${preset.name}`, async ({ page }, testInfo) => {
    const records = [
      adjustment('draft', {
        data: {
          adjustment_date: '2026-11-19',
          adjustment_type: 'opening',
          status: 'draft',
          notes: '期初庫存',
        },
      }),
      adjustment('posted', {
        id: '0196f5a0-0000-7000-8000-0000000000a2',
        record_no: 'ADJ202611-0002',
        title: 'ADJ202611-0002',
        data: {
          adjustment_type: 'count',
          adjustment_date: '2026-11-20',
          status: 'posted',
          notes: '盤盈',
        },
      }),
      adjustment('void', {
        id: '0196f5a0-0000-7000-8000-0000000000a3',
        record_no: 'ADJ202611-0003',
        title: 'ADJ202611-0003',
        data: {
          adjustment_type: 'count',
          adjustment_date: '2026-11-21',
          status: 'void',
          notes: '輸入錯誤',
        },
      }),
    ];
    let loggedIn = false;
    await mockTauri(page, {
      get_server_url: () => 'http://localhost:8080',
      health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
      setup_status: () => ({ needs_setup: false }),
      current_user: () => (loggedIn ? ADMIN : null),
      login: () => {
        loggedIn = true;
        return ADMIN;
      },
      list_entities: () => [STOCK_ADJUSTMENT],
      get_entity: () => ({ entity: STOCK_ADJUSTMENT, fields: STOCK_ADJUSTMENT_FIELDS }),
      get_default_layout: () => ({
        source: 'generated',
        layout: null,
        config: {
          columns: ['adjustment_date', 'adjustment_type', 'status', 'notes'].map((k) => ({
            field_key: k,
            width: null,
          })),
          sort: null,
          filters: [],
        },
      }),
      list_layouts: () => [],
      list_records: () => ({ items: records, total: records.length, page: 1, page_size: 50 }),
      get_record: () => records[0],
    });
    await page.setViewportSize({ width: 1280, height: 800 });
    const shot = async (name: string) =>
      page.screenshot({ path: testInfo.outputPath(`${preset.name}-${name}.png`) });

    // 登入畫面
    await page.goto(`/?theme=${preset.name}#/`);
    const loginButton = page.getByRole('button', { name: /(^|\s)登\s?入$/ });
    await expect(loginButton).toHaveCSS('background-color', preset.primary);
    await shot('1-login');

    // 列表
    await page.getByLabel('帳號').fill('admin');
    await page.getByLabel('密碼').fill('admin-password');
    await loginButton.click();
    await page.goto(`/?theme=${preset.name}#/entities/stock_adjustment`);
    await expect(page.getByRole('row', { name: /ADJ202611-0003/ })).toBeVisible();
    await expect(page.locator('.ant-menu-item-selected')).toHaveCSS(
      'background-color',
      preset.primary,
    );
    // 等選單區塊展開動畫結束
    await expect(page.locator('.ant-motion-collapse')).toHaveCount(0);
    await shot('2-list');

    // 單據表單
    await page.goto(`/?theme=${preset.name}#/entities/stock_adjustment/${ADJUSTMENT_ID}`);
    await expect(page.getByLabel('單號')).toHaveValue('ADJ202611-0001');
    await expect(page.locator('.ant-motion-collapse')).toHaveCount(0);
    await shot('3-form');
  });
}
