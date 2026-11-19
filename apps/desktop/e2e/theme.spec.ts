import { expect, test } from '@playwright/test';
import type { Locator } from '@playwright/test';

import {
  STOCK_ADJUSTMENT,
  STOCK_ADJUSTMENT_FIELDS,
  adjustment,
} from '../src/test/documentFixtures';
import { mockTauri } from './tauri-mock';

const ADMIN = {
  id: '0196f5a0-0000-7000-8000-000000000001',
  username: 'admin',
  display_name: '王管理',
  role: 'admin',
  permissions: ['record.post', 'record.read', 'record.write'],
  must_change_password: false,
};

/** 與 src/theme.ts 的 CONTROL_HEIGHT 相同（原本 antd 預設 32）。 */
const CONTROL_HEIGHT = 36;

async function box(l: Locator) {
  const b = await l.boundingBox();
  expect(b).not.toBeNull();
  return b!;
}

/** 高度為 {@link CONTROL_HEIGHT}，且與 `ref` 上緣對齊（同一列）。 */
async function expectAligned(l: Locator, ref: Locator) {
  const [a, r] = [await box(l), await box(ref)];
  expect(Math.round(a.height)).toBe(CONTROL_HEIGHT);
  expect(Math.abs(a.y - r.y)).toBeLessThan(1);
}

test('buttons and inputs share the taller control height', async ({ page }) => {
  const record = adjustment('draft', { data: { adjustment_date: '2026-11-19', status: 'draft' } });
  await mockTauri(page, {
    get_server_url: () => 'http://localhost:8080',
    health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
    setup_status: () => ({ needs_setup: false }),
    current_user: () => ADMIN,
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
    list_records: () => ({ items: [record], total: 1, page: 1, page_size: 50 }),
  });
  await page.goto('/#/entities/stock_adjustment');

  // 卡片右上角的按鈕
  const reload = page.getByRole('button', { name: /重新整理/ });
  await expectAligned(reload, reload);
  await expectAligned(page.getByRole('button', { name: /新\s?增$/ }), reload);

  // 工具列：下拉選單與按鈕同高、對齊
  const statusSelect = page.locator('.ant-select').filter({ has: page.getByLabel('狀態篩選') });
  await expectAligned(statusSelect, statusSelect);
  await expectAligned(
    page.locator('.ant-select').filter({ has: page.getByLabel('版面') }),
    statusSelect,
  );
  await expectAligned(page.getByRole('button', { name: '另存新版面' }), statusSelect);
  await expectAligned(page.getByRole('button', { name: /欄位設定/ }), statusSelect);

  // 表格：資料列正常顯示，分頁控制項沒有破版（同一列）
  await expect(page.getByRole('row').nth(1)).toBeVisible();
  const pager = page.locator('.ant-pagination').first();
  const items = pager.locator('> li');
  const first = await box(items.first());
  for (const li of await items.all()) {
    if (await li.isVisible()) {
      expect(Math.abs((await box(li)).y - first.y)).toBeLessThan(2);
    }
  }

  // Modal：輸入框與 footer 按鈕
  await page.getByRole('button', { name: '另存新版面' }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toBeVisible();
  // 等開啟動畫結束再量位置
  await expect(page.locator('.ant-zoom-appear, .ant-zoom-enter')).toHaveCount(0);
  const input = dialog.locator('input.ant-input').first();
  expect(Math.round((await box(input)).height)).toBe(CONTROL_HEIGHT);
  const footerButtons = dialog.locator('.ant-modal-footer button');
  await expect(footerButtons).toHaveCount(2);
  await expectAligned(footerButtons.nth(1), footerButtons.nth(0));
  await expectAligned(footerButtons.nth(0), footerButtons.nth(0));
});
