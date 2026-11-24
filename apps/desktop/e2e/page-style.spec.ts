import { expect, test } from '@playwright/test';
import type { Locator } from '@playwright/test';

import {
  PRODUCT,
  PRODUCT_ID,
  STOCK_ADJUSTMENT,
  STOCK_ADJUSTMENT_FIELDS,
} from '../src/test/documentFixtures';
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

const page = <T>(items: T[]) => ({ items, total: items.length, page: 1, page_size: 50 });

/** 標題列使用 PageHeader 的頁面（issue #232 列出的頁面）。 */
const PAGES: { path: string; title: string | RegExp }[] = [
  { path: '/#/entities/stock_adjustment', title: '盤點調整單' },
  { path: '/#/currencies', title: '幣別' },
  { path: '/#/exchange-rates', title: '匯率' },
  { path: '/#/users', title: '帳號管理' },
  { path: '/#/audit-logs', title: '操作紀錄' },
  { path: '/#/schema', title: '資料類別' },
  { path: '/#/schema/stock_adjustment', title: '欄位定義：盤點調整單' },
  { path: '/#/stock', title: '庫存查詢' },
  { path: `/#/stock/${PRODUCT_ID}`, title: /不鏽鋼螺絲/ },
];

async function box(l: Locator) {
  const b = await l.boundingBox();
  expect(b).not.toBeNull();
  return b!;
}

test('every page uses the same header: title position, size and action bar', async ({
  page: p,
}) => {
  await mockTauri(p, {
    get_server_url: () => 'http://localhost:8080',
    health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
    setup_status: () => ({ needs_setup: false }),
    current_user: () => ADMIN,
    list_entities: () => [PRODUCT, STOCK_ADJUSTMENT],
    get_entity: ({ key }) =>
      key === 'product'
        ? { entity: PRODUCT, fields: [] }
        : { entity: STOCK_ADJUSTMENT, fields: STOCK_ADJUSTMENT_FIELDS },
    get_record: () => ({
      id: PRODUCT_ID,
      entity_key: 'product',
      title: 'A001 不鏽鋼螺絲 M6',
      data: { code: 'A001', name: '不鏽鋼螺絲 M6' },
      version: 1,
      created_at: '2026-11-18T02:00:00Z',
      updated_at: '2026-11-18T02:00:00Z',
    }),
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
    list_records: () => page([]),
    list_currencies: () => [],
    list_exchange_rates: () => page([]),
    list_users: () => [],
    list_audit_logs: () => page([]),
    list_inventory_lots: () => page([]),
    list_inventory_transactions: () => page([]),
    list_stock_balances: () =>
      page([
        {
          product_id: PRODUCT_ID,
          code: 'A001',
          name: '不鏽鋼螺絲 M6',
          spec: '6mm',
          unit_name: '個',
          is_active: true,
          qty_on_hand: '30.0000',
          // 負數（例如成本調整後）：錯誤色
          stock_value_twd: '-1234.50',
          safety_stock: null,
          below_safety_stock: false,
        },
      ]),
  });

  let reference: { x: number; y: number; height: number; fontSize: string } | undefined;
  for (const { path, title } of PAGES) {
    await p.goto(path);
    const header = p.getByTestId('page-header');
    await expect(header).toHaveCount(1);
    const heading = header.getByRole('heading', { level: 4 });
    await expect(heading).toHaveText(title);

    // 動作列在標題列右側，與標題同一列
    const actions = header.locator('.page-header-actions');
    const [h, a] = [await box(heading), await box(actions)];
    expect(a.x).toBeGreaterThan(h.x + h.width);
    expect(Math.abs(a.y + a.height / 2 - (h.y + h.height / 2))).toBeLessThan(12);

    // 每一頁的標題位置與字級相同（有返回按鈕的頁面，標題往右移，只比較上緣）
    const headerBox = await box(header);
    const fontSize = await heading.evaluate((el) => getComputedStyle(el).fontSize);
    if (!reference) {
      reference = { x: headerBox.x, y: headerBox.y, height: h.height, fontSize };
    }
    expect(headerBox.x).toBe(reference.x);
    expect(headerBox.y).toBe(reference.y);
    expect(h.height).toBe(reference.height);
    expect(fontSize).toBe(reference.fontSize);
  }

  // 庫存查詢：金額靠右、負數為錯誤色
  await p.goto('/#/stock');
  const cell = p.getByRole('cell', { name: '-1,234.50' });
  await expect(cell).toHaveCSS('text-align', 'right');
  await expect(cell.locator('.ant-typography-danger')).toBeVisible();
});
