import { expect, test } from '@playwright/test';
import type { Locator, Page } from '@playwright/test';

import type { Record as DataRecord } from '../src/bindings';
import { PRODUCT_ID, lot } from '../src/test/documentFixtures';
import { CURRENCIES, TAX_RATES, purchasingRecord } from '../src/test/purchasingFixtures';
import {
  CUSTOMER_ID,
  CUSTOMER_TITLE,
  SALES_ENTITIES,
  SALES_RETURN_ID,
  SHIPMENT_ID,
  salesEntityDetail,
} from '../src/test/salesFixtures';
import { openNavSection } from './nav';
import { mockTauri } from './tauri-mock';

/**
 * 銷貨退回（docs/architecture.md §8.9「銷貨退回」）：選擇已過帳的出貨單 → 帶入可退明細（扣除其他已過帳的退回）
 * → 修改數量、刪除不退的列 → 存檔 → 過帳 → 加回的批號 → 作廢 → 庫存查詢。伺服器以 mock 模擬。
 */

const ADMIN = {
  id: '0196f5a0-0000-7000-8000-000000000001',
  username: 'admin',
  display_name: '王管理',
  role: 'admin',
  permissions: ['record.post', 'record.read', 'record.write'],
  must_change_password: false,
};

const PRODUCT_TITLE = 'A001 不鏽鋼螺絲 M6';
const PRODUCT2_ID = '0196f5a0-0000-7000-8000-000000000602';
const PRODUCT2_TITLE = 'B002 六角螺帽 M6';
const SHIPMENT_TITLE = 'SH202611-0001 大同五金行';
const SHIP_LINE_1 = '0196f5a0-0000-7000-8000-000000000f01';
const SHIP_LINE_2 = '0196f5a0-0000-7000-8000-000000000f02';
const OTHER_RETURN_ID = '0196f5a0-0000-7000-8000-000000000d32';
const RETURN_LINE_1 = '0196f5a0-0000-7000-8000-000000000f11';

const page = <T>(items: T[]) => ({ items, total: items.length, page: 1, page_size: 50 });
const rel = (id: string, title: string) => ({ id, title, deleted: false });
const titleOf = (id: string) => (id === PRODUCT_ID ? PRODUCT_TITLE : PRODUCT2_TITLE);
const DOC = {
  posted_at: null,
  posted_by: null,
  voided_at: null,
  voided_by: null,
  closed_at: null,
  closed_by: null,
};

async function settled(p: Page) {
  await expect(p.locator('.ant-spin-spinning, .ant-spin-blur')).toHaveCount(0);
}

function select(scope: Page | Locator, name: string) {
  const root = 'page' in scope && typeof scope.page === 'function' ? scope.page() : (scope as Page);
  return scope.locator('.ant-select', { has: root.getByRole('combobox', { name, exact: true }) });
}

function option(p: Page, title: string) {
  return p
    .locator('.ant-select-dropdown:not(.ant-select-dropdown-hidden)')
    .last()
    .locator('.ant-select-item-option', { hasText: title });
}

function lineRecord(id: string, data: DataRecord['data']) {
  const productId = data.product_id as string;
  return purchasingRecord({
    id,
    title: titleOf(productId),
    data,
    relations: { product_id: rel(productId, titleOf(productId)) },
  });
}

/** 已過帳的出貨單：A001 出貨 10（L-01 扣 4、L-02 扣 6）、B002 出貨 2。 */
const SHIPMENT = purchasingRecord({
  id: SHIPMENT_ID,
  record_no: 'SH202611-0001',
  title: SHIPMENT_TITLE,
  data: {
    customer_id: CUSTOMER_ID,
    ship_date: '2026-11-18',
    tax_type: 'taxable',
    tax_included: false,
    status: 'posted',
  },
  relations: { customer_id: rel(CUSTOMER_ID, CUSTOMER_TITLE) },
  document: { ...DOC, posted_at: '2026-11-18T03:00:00Z' },
  lines: {
    shipment_line: [
      lineRecord(SHIP_LINE_1, {
        product_id: PRODUCT_ID,
        qty: '10.0000',
        unit_price: '120.0000',
        discount_rate: '0.1500',
        amount: '1020.0000',
      }),
      lineRecord(SHIP_LINE_2, {
        product_id: PRODUCT2_ID,
        qty: '2.0000',
        unit_price: '33.3333',
        discount_rate: '0.0000',
        amount: '67.0000',
      }),
    ],
  },
});

/** 另一張已過帳的退回：A001 已退 2。 */
const OTHER_RETURN = purchasingRecord({
  id: OTHER_RETURN_ID,
  record_no: 'SR202611-0001',
  data: { shipment_id: SHIPMENT_ID, status: 'posted' },
  lines: {
    sales_return_line: [
      lineRecord('0196f5a0-0000-7000-8000-000000000f21', {
        shipment_line_id: SHIP_LINE_1,
        product_id: PRODUCT_ID,
        qty: '2.0000',
      }),
    ],
  },
});

test('sales return from posted shipment: returnable qty, preview, post, lots added back, void, stock', async ({
  page: p,
}) => {
  let current: DataRecord | null = null;
  let created: unknown;

  await mockTauri(p, {
    get_server_url: () => 'http://localhost:8080',
    health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
    setup_status: () => ({ needs_setup: false }),
    current_user: () => ADMIN,
    list_entities: () => SALES_ENTITIES,
    get_entity: ({ key }) => salesEntityDetail(key as string),
    list_currencies: () => CURRENCIES,
    list_tax_rates: () => TAX_RATES,
    lookup_records: ({ key }) =>
      key === 'shipment' ? [{ id: SHIPMENT_ID, title: SHIPMENT_TITLE }] : [],
    list_records: ({ key }) =>
      key === 'sales_return' ? page([{ ...OTHER_RETURN, lines: undefined }]) : page([]),
    get_record: ({ key, id }) =>
      key === 'shipment'
        ? SHIPMENT
        : key === 'product'
          ? purchasingRecord({ id: id as string, title: titleOf(id as string), data: {} })
          : key === 'customer'
            ? purchasingRecord({ id: CUSTOMER_ID, title: CUSTOMER_TITLE, data: { code: 'C001' } })
            : id === OTHER_RETURN_ID
              ? OTHER_RETURN
              : current,
    create_record: ({ req }) => {
      created = req;
      const { data, lines } = req as {
        data: DataRecord['data'];
        lines: { sales_return_line: { data: DataRecord['data'] }[] };
      };
      // 伺服器從出貨單帶入客戶、稅別、商品、單價，計算金額（7 × 120 × 0.85 = 714；稅 36）
      current = purchasingRecord({
        id: SALES_RETURN_ID,
        record_no: 'SR202611-0002',
        title: 'SR202611-0002 大同五金行',
        data: {
          ...data,
          customer_id: CUSTOMER_ID,
          tax_type: 'taxable',
          tax_included: false,
          status: 'draft',
          subtotal: '714.0000',
          tax_amount: '36.0000',
          total: '750.0000',
          cost_total_twd: null,
        },
        relations: {
          customer_id: rel(CUSTOMER_ID, CUSTOMER_TITLE),
          shipment_id: rel(SHIPMENT_ID, SHIPMENT_TITLE),
        },
        document: DOC,
        lines: {
          sales_return_line: lines.sales_return_line.map((l) =>
            lineRecord(RETURN_LINE_1, {
              ...l.data,
              product_id: PRODUCT_ID,
              unit_price: '120.0000',
              discount_rate: '0.1500',
              amount: '714.0000',
              cost_amount_twd: null,
            }),
          ),
        },
      });
      return current;
    },
    post_record: () => {
      const line = current!.lines!.sales_return_line![0]!;
      current = {
        ...current!,
        version: 2,
        data: { ...current!.data, status: 'posted', cost_total_twd: '570.00' },
        document: { ...DOC, posted_at: '2026-11-19T03:00:00Z' },
        lines: {
          sales_return_line: [{ ...line, data: { ...line.data, cost_amount_twd: '570.00' } }],
        },
      };
      return current;
    },
    void_record: () => {
      current = {
        ...current!,
        version: 3,
        data: { ...current!.data, status: 'void' },
        document: { ...current!.document!, voided_at: '2026-11-19T04:00:00Z' },
      };
      return current;
    },
    // 加回原出貨扣除的批號：最後扣的先加回（L-02 剩 4 可加回，再加回 L-01 3）
    list_inventory_transactions: ({ query }) => {
      const q = query as { ref_doc_id?: string };
      if (q.ref_doc_id !== SALES_RETURN_ID) {
        return page([]);
      }
      const txn = (id: string, lotNo: string, qty: string, cost: string) => ({
        id,
        txn_at: '2026-11-19T03:00:00Z',
        txn_date: '2026-11-19',
        txn_type: 'sales_return',
        product_id: PRODUCT_ID,
        product_code: 'A001',
        product_name: '不鏽鋼螺絲 M6',
        lot_id: lotNo,
        lot_no: lotNo,
        qty_change: qty,
        unit_cost_twd: cost,
        ref_doc_type: 'sales_return',
        ref_doc_id: SALES_RETURN_ID,
        ref_record_no: 'SR202611-0002',
        ref_line_id: RETURN_LINE_1,
        created_by: null,
        created_by_name: null,
      });
      // 由新到舊
      return page([txn('t2', 'L-01', '3.0000', '80.0000'), txn('t1', 'L-02', '4.0000', '82.5000')]);
    },
    list_product_lots: () =>
      page([
        lot({
          id: 'lot-1',
          lot_no: 'L-01',
          qty_in: '4.0000',
          qty_remaining: '3.0000',
          unit_cost_twd: '80.0000',
        }),
        lot({
          id: 'lot-2',
          lot_no: 'L-02',
          qty_in: '10.0000',
          qty_remaining: '8.0000',
          unit_cost_twd: '82.5000',
        }),
      ]),
    list_stock_balances: () => page([]),
  });

  // 從選單「銷貨」開啟銷貨退回
  await p.goto('/');
  await openNavSection(p, '銷貨');
  await expect(p.getByRole('menuitem', { name: /銷貨退回/ })).toBeVisible();
  await p.goto('/#/entities/sales_return/new');
  await expect(p.getByRole('textbox', { name: '退回日期', exact: true })).toHaveValue(
    /^\d{4}-\d{2}-\d{2}$/,
  );

  // 選擇出貨單：帶入客戶、稅別與可退的明細（A001 已退 2 → 可退 8）
  await select(p, '出貨單').click();
  await option(p, SHIPMENT_TITLE).click();
  await expect(
    p.getByText('已帶入 SH202611-0001 的 2 筆可退明細，請修改退回數量或刪除不退的列'),
  ).toBeVisible();
  await expect(select(p, '客戶')).toContainText(CUSTOMER_TITLE);
  await expect(select(p, '稅別')).toContainText('應稅');

  const lines = p.getByRole('region', { name: '銷貨退回明細' });
  const rows = lines.locator('tbody tr.ant-table-row');
  const cell = (i: number, name: string) => rows.nth(i).getByRole('textbox', { name, exact: true });
  await expect(rows).toHaveCount(2);
  await expect(lines.getByRole('button', { name: /新增/ })).toHaveCount(0);
  await expect(lines.locator('thead')).toContainText('可退數量');
  await expect(rows.nth(0)).toContainText('8');
  await expect(cell(0, '數量')).toHaveValue('8.0000');
  await expect(cell(1, '數量')).toHaveValue('2.0000');
  await expect(select(rows.nth(0), '商品')).toContainText(PRODUCT_TITLE);
  // 8 × 120 × 0.85 = 816；2 × 33.3333 = 66.6666 → 67；稅 round(883 × 0.05) = 44
  await expect(cell(0, '金額')).toHaveValue('816.0000');
  await expect(cell(1, '金額')).toHaveValue('67.0000');
  await expect(p.getByRole('textbox', { name: '總計', exact: true })).toHaveValue('927.0000');
  await settled(p);
  await lines.scrollIntoViewIfNeeded();
  await p.screenshot({ path: test.info().outputPath('return-from-shipment.png'), fullPage: true });

  // 只退 A001 的 7：修改數量、刪除第 2 列
  await cell(0, '數量').fill('7');
  await rows.nth(1).getByRole('button', { name: '刪除第 2 列' }).click();
  await expect(rows).toHaveCount(1);
  await expect(cell(0, '金額')).toHaveValue('714.0000');
  await expect(p.getByRole('textbox', { name: '稅額', exact: true })).toHaveValue('36.0000');
  await expect(p.getByRole('textbox', { name: '總計', exact: true })).toHaveValue('750.0000');
  await p.getByRole('textbox', { name: '退回原因', exact: true }).fill('包裝破損');

  await p.getByRole('button', { name: /儲\s?存/ }).click();
  await expect(p.getByLabel('單號')).toHaveValue('SR202611-0002');
  expect(created).toMatchObject({
    data: { shipment_id: SHIPMENT_ID, reason: '包裝破損' },
    lines: { sales_return_line: [{ data: { shipment_line_id: SHIP_LINE_1, qty: '7' } }] },
  });
  // 存檔後不可更換出貨單
  await expect(p.getByRole('combobox', { name: '出貨單', exact: true })).toBeDisabled();

  // 過帳：顯示加回的批號與成本
  const action = (name: RegExp) => p.locator('.ant-card-extra').getByRole('button', { name });
  await action(/過\s?帳/).click();
  await p
    .getByRole('dialog')
    .getByRole('button', { name: /過\s?帳/ })
    .click();
  await expect(p.locator('.ant-card-head .ant-tag', { hasText: '已過帳' })).toBeVisible();
  await expect(p.getByRole('textbox', { name: '退回成本', exact: true })).toHaveValue('570.00');
  const added = p.getByRole('region', { name: '加回的批號' });
  const addedRows = added.locator('tbody tr.ant-table-row');
  await expect(addedRows).toHaveCount(2);
  await expect(addedRows.nth(0)).toContainText('第 1 列');
  await expect(addedRows.nth(0)).toContainText('L-02');
  await expect(addedRows.nth(0)).toContainText('330.00');
  await expect(addedRows.nth(1)).toContainText('L-01');
  await expect(addedRows.nth(1)).toContainText('240.00');
  await expect(p.getByRole('dialog')).toHaveCount(0);
  await settled(p);
  await added.scrollIntoViewIfNeeded();
  await p.screenshot({ path: test.info().outputPath('return-posted.png'), fullPage: true });

  // 作廢
  await action(/作\s?廢/).click();
  await expect(p.getByRole('dialog')).toContainText('作廢會反向處理這張單據的庫存異動');
  await p
    .getByRole('dialog')
    .getByRole('button', { name: /作\s?廢/ })
    .click();
  await expect(p.locator('.ant-card-head .ant-tag', { hasText: '已作廢' })).toBeVisible();
  await expect(p.getByText(/作廢時間：/)).toBeVisible();
  await expect(added).toBeVisible();
  await expect(p.getByRole('dialog')).toHaveCount(0);
  await settled(p);
  await p.screenshot({ path: test.info().outputPath('return-void.png'), fullPage: true });

  // 加回的批號連到庫存查詢
  await added.getByRole('link', { name: 'L-02' }).click();
  await expect(p.locator('.ant-card-head-title', { hasText: PRODUCT_TITLE })).toBeVisible();
  await expect(p.getByRole('table', { name: '批號' })).toContainText('L-02');
  await settled(p);
  await p.screenshot({ path: test.info().outputPath('return-stock.png') });
});
