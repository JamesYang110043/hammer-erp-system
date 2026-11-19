import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

import type { Record as DataRecord } from '../src/bindings';
import {
  ADJUSTMENT_ID,
  LOT_ID,
  PRODUCT,
  PRODUCT_ID,
  STOCK_ADJUSTMENT,
  STOCK_ADJUSTMENT_FIELDS,
  STOCK_ADJUSTMENT_LINE,
  STOCK_ADJUSTMENT_LINE_FIELDS,
  adjustment,
  lot,
} from '../src/test/documentFixtures';
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

const POST_BUTTON = /(^|\s)過\s?帳$/;
const PRODUCT_TITLE = 'A001 不鏽鋼螺絲 M6';
/** 表格載入完成、淡入動畫結束（截圖前）。 */
async function settled(p: Page) {
  await expect(p.locator('.ant-spin-spinning, .ant-spin-blur')).toHaveCount(0);
  for (const container of await p.locator('.ant-spin-container').all()) {
    await expect(container).toHaveCSS('opacity', '1');
  }
}

const page = <T>(items: T[]) => ({ items, total: items.length, page: 1, page_size: 50 });

/**
 * 模擬伺服器：期初庫存過帳後產生批號與異動；庫存查詢依目前的狀態回應。
 */
test('opening stock: enter, post and check inventory', async ({ page: p }) => {
  let current: DataRecord | null = null;
  let posted = false;
  let created: unknown;
  const lineId = '0196f5a0-0000-7000-8000-000000000901';

  await mockTauri(p, {
    get_server_url: () => 'http://localhost:8080',
    health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
    setup_status: () => ({ needs_setup: false }),
    current_user: () => ADMIN,
    list_entities: () => [PRODUCT, STOCK_ADJUSTMENT, STOCK_ADJUSTMENT_LINE],
    get_entity: ({ key }) =>
      key === 'stock_adjustment_line'
        ? { entity: STOCK_ADJUSTMENT_LINE, fields: STOCK_ADJUSTMENT_LINE_FIELDS }
        : { entity: STOCK_ADJUSTMENT, fields: STOCK_ADJUSTMENT_FIELDS },
    lookup_records: () => [{ id: PRODUCT_ID, title: PRODUCT_TITLE }],
    get_record: ({ key }) =>
      key === 'product'
        ? {
            ...adjustment(),
            id: PRODUCT_ID,
            title: PRODUCT_TITLE,
            record_no: null,
            document: null,
            data: { code: 'A001', name: '不鏽鋼螺絲 M6' },
          }
        : current,
    create_record: ({ req }) => {
      created = req;
      const { data, lines } = req as {
        data: DataRecord['data'];
        lines: { stock_adjustment_line: { data: DataRecord['data'] }[] };
      };
      const line = lines.stock_adjustment_line[0]!;
      current = adjustment('draft', {
        data: { ...data, status: 'draft' },
        lines: {
          stock_adjustment_line: [
            {
              ...adjustment(),
              id: lineId,
              record_no: null,
              document: null,
              title: PRODUCT_TITLE,
              data: { ...line.data },
              relations: { product_id: { id: PRODUCT_ID, title: PRODUCT_TITLE, deleted: false } },
            },
          ],
        },
      });
      return current;
    },
    post_record: () => {
      posted = true;
      current = {
        ...current!,
        ...adjustment('posted'),
        data: { ...current!.data, status: 'posted' },
        lines: current!.lines,
      };
      return current;
    },
    list_stock_balances: () =>
      page([
        {
          product_id: PRODUCT_ID,
          code: 'A001',
          name: '不鏽鋼螺絲 M6',
          spec: '6mm',
          unit_name: '個',
          is_active: true,
          qty_on_hand: posted ? '30.0000' : '0.0000',
          stock_value_twd: posted ? '375.00' : '0.00',
          safety_stock: '50.0000',
          below_safety_stock: true,
        },
        {
          product_id: '0196f5a0-0000-7000-8000-000000000602',
          code: 'B002',
          name: '六角螺帽 M6',
          spec: null,
          unit_name: '個',
          is_active: true,
          qty_on_hand: '120.0000',
          stock_value_twd: '360.00',
          safety_stock: '20.0000',
          below_safety_stock: false,
        },
      ]),
    list_product_lots: () =>
      page(
        posted
          ? [
              lot({
                lot_no: 'OPEN-A001',
                qty_in: '30.0000',
                qty_remaining: '30.0000',
                received_date: '2026-11-18',
              }),
            ]
          : [],
      ),
    list_inventory_transactions: () =>
      page(
        posted
          ? [
              {
                id: '0196f5a0-0000-7000-8000-000000000801',
                txn_at: '2026-11-18T02:00:00Z',
                txn_date: '2026-11-18',
                txn_type: 'adjustment',
                product_id: PRODUCT_ID,
                product_code: 'A001',
                product_name: '不鏽鋼螺絲 M6',
                lot_id: LOT_ID,
                lot_no: 'OPEN-A001',
                qty_change: '30.0000',
                unit_cost_twd: '12.5000',
                ref_doc_type: 'stock_adjustment',
                ref_doc_id: ADJUSTMENT_ID,
                ref_record_no: 'ADJ202611-0001',
                ref_line_id: lineId,
                created_by: ADMIN.id,
                created_by_name: '王管理',
              },
            ]
          : [],
      ),
  });

  // 從選單「庫存」開啟盤點調整單，新增期初庫存
  await p.goto('/');
  await openNavSection(p, '庫存');
  await p.getByRole('menuitem', { name: /盤點調整單/ }).click();
  await p.goto('/#/entities/stock_adjustment/new');
  await p.locator('.ant-select', { has: p.getByRole('combobox', { name: '類型' }) }).click();
  await p.getByTitle('期初庫存').click();

  const lines = p.getByRole('region', { name: '盤點調整明細' });
  await lines.getByRole('button', { name: /新增盤點調整明細/ }).click();
  await lines.locator('.ant-select', { has: p.getByRole('combobox', { name: '商品' }) }).click();
  await p.getByTitle(PRODUCT_TITLE).click();
  await expect(lines.getByText('新批號', { exact: true })).toBeVisible();
  await lines.getByRole('textbox', { name: '批號' }).fill('OPEN-A001');
  await lines.getByLabel('數量').fill('30');
  await lines.getByLabel('單位成本').fill('12.5');
  await p.screenshot({ path: test.info().outputPath('adjustment-lines.png'), fullPage: true });
  await p.getByRole('button', { name: /儲\s?存/ }).click();

  await expect(p.getByLabel('單號')).toHaveValue('ADJ202611-0001');
  // 調整日期預設今天；明細依輸入送出（數量、成本為字串）
  expect(created).toMatchObject({
    data: {
      adjustment_type: 'opening',
      adjustment_date: expect.stringMatching(/^\d{4}-\d{2}-\d{2}$/),
    },
    lines: {
      stock_adjustment_line: [
        {
          data: {
            product_id: PRODUCT_ID,
            line_type: 'gain_new',
            lot_no: 'OPEN-A001',
            qty: '30',
            unit_cost_twd: '12.5',
          },
        },
      ],
    },
  });
  await p.locator('.ant-card-extra').getByRole('button', { name: POST_BUTTON }).click();
  await p.getByRole('dialog').getByRole('button', { name: POST_BUTTON }).click();
  await expect(p.locator('.ant-card-head .ant-tag', { hasText: '已過帳' })).toBeVisible();

  // 庫存查詢：低於安全庫存的商品有標示
  await p.getByRole('menuitem', { name: /庫存查詢/ }).click();
  const row = p.getByRole('row', { name: /A001/ });
  await expect(row).toContainText('30');
  await expect(row).toContainText('375.00');
  await expect(row.getByText('低於安全庫存')).toBeVisible();
  await expect(p.getByRole('row', { name: /B002/ }).getByText('低於安全庫存')).toHaveCount(0);
  await settled(p);
  await p.screenshot({ path: test.info().outputPath('stock-balances.png') });

  // 商品 → 批號與異動流水帳 → 回到來源單據
  await row.click();
  await expect(p.getByRole('table', { name: '批號' })).toContainText('OPEN-A001');
  const txns = p.getByRole('table', { name: '異動流水帳' });
  await expect(txns).toContainText('+30');
  await expect(txns).toContainText('盤點調整');
  await settled(p);
  await p.screenshot({ path: test.info().outputPath('product-stock.png') });
  await txns.getByRole('link', { name: 'ADJ202611-0001' }).click();
  await expect(p.getByLabel('單號')).toHaveValue('ADJ202611-0001');
  await expect(p.getByRole('menuitem', { name: /盤點調整單/ })).toHaveClass(
    /ant-menu-item-selected/,
  );
});
