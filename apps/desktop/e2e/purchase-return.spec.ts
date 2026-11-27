import { expect, test } from '@playwright/test';
import type { Locator, Page } from '@playwright/test';

import type { Record as DataRecord } from '../src/bindings';
import { PRODUCT_ID, lot } from '../src/test/documentFixtures';
import {
  CURRENCIES,
  PURCHASING_ENTITIES,
  RECEIPT_ID,
  RETURN_ID,
  SUPPLIER_ID,
  SUPPLIER_TITLE,
  TAX_RATES,
  purchasingEntityDetail,
  purchasingRecord,
} from '../src/test/purchasingFixtures';
import { openNavSection } from './nav';
import { mockTauri } from './tauri-mock';

/**
 * 進貨退回（docs/architecture.md §8.8「進貨退回」）：選擇已過帳的進貨單 → 帶入可退明細 →
 * 修改數量 → 存檔 → 過帳 → 庫存查詢 → 作廢。伺服器以 mock 模擬。
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
const RECEIPT_TITLE = 'GR202611-0001 美商五金';
const RECEIPT_LINE_1 = '0196f5a0-0000-7000-8000-000000000c01';
const RECEIPT_LINE_2 = '0196f5a0-0000-7000-8000-000000000c02';
const OLD_RETURN_ID = '0196f5a0-0000-7000-8000-000000000a30';
const RETURN_LINE_ID = '0196f5a0-0000-7000-8000-000000000d01';

const page = <T>(items: T[]) => ({ items, total: items.length, page: 1, page_size: 50 });
const rel = (id: string, title: string) => ({ id, title, deleted: false, inactive: false });
const doc = (posted: string | null, voided: string | null = null) => ({
  posted_at: posted,
  posted_by: null,
  voided_at: voided,
  voided_by: null,
  closed_at: null,
  closed_by: null,
});

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

/** 已過帳的進貨單：A001 6 個（批號 US-2611）、B002 3 個。 */
const receipt = purchasingRecord({
  id: RECEIPT_ID,
  record_no: 'GR202611-0001',
  title: RECEIPT_TITLE,
  data: {
    supplier_id: SUPPLIER_ID,
    receipt_date: '2026-11-18',
    currency_code: 'USD',
    exchange_rate: '32.150000',
    tax_type: 'zero_rated',
    status: 'posted',
  },
  document: doc('2026-11-18T03:00:00Z'),
  lines: {
    goods_receipt_line: [
      purchasingRecord({
        id: RECEIPT_LINE_1,
        data: {
          product_id: PRODUCT_ID,
          qty: '6.0000',
          unit_price: '12.3450',
          lot_no: 'US-2611',
          unit_cost_twd: '396.8918',
        },
      }),
      purchasingRecord({
        id: RECEIPT_LINE_2,
        data: {
          product_id: PRODUCT2_ID,
          qty: '3.0000',
          unit_price: '7.5000',
          lot_no: 'GR202611-0001-2',
          unit_cost_twd: '241.1250',
        },
      }),
    ],
  },
});

/** 之前已過帳的退回：A001 退了 2 個。 */
const oldReturn = purchasingRecord({
  id: OLD_RETURN_ID,
  record_no: 'PR202611-0001',
  data: { goods_receipt_id: RECEIPT_ID, status: 'posted' },
  lines: {
    purchase_return_line: [
      purchasingRecord({
        id: '0196f5a0-0000-7000-8000-000000000d00',
        data: { goods_receipt_line_id: RECEIPT_LINE_1, qty: '2.0000' },
      }),
    ],
  },
});

test('purchase return: from posted receipt, returnable qty, post, stock, void', async ({
  page: p,
}) => {
  let current: DataRecord | null = null;
  let created: unknown;
  let state: 'none' | 'posted' | 'void' = 'none';

  await mockTauri(p, {
    get_server_url: () => 'http://localhost:8080',
    health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
    setup_status: () => ({ needs_setup: false }),
    current_user: () => ADMIN,
    list_entities: () => PURCHASING_ENTITIES,
    get_entity: ({ key }) => purchasingEntityDetail(key as string),
    list_currencies: () => CURRENCIES,
    list_tax_rates: () => TAX_RATES,
    lookup_records: () => [{ id: RECEIPT_ID, title: RECEIPT_TITLE }],
    list_records: ({ key, query }) => {
      expect(key).toBe('purchase_return');
      // 可退數量：指向此進貨單、已過帳的退回
      expect((query as { filters: unknown[] }).filters).toEqual([
        { field_key: 'goods_receipt_id', op: 'eq', value: RECEIPT_ID },
        { field_key: 'status', op: 'eq', value: 'posted' },
      ]);
      return page(state === 'posted' ? [oldReturn, current!] : [oldReturn]);
    },
    get_record: ({ key, id }) => {
      if (key === 'goods_receipt') return receipt;
      if (key === 'supplier') return purchasingRecord({ id: SUPPLIER_ID, title: SUPPLIER_TITLE });
      if (key === 'product')
        return purchasingRecord({
          id: id as string,
          title: id === PRODUCT_ID ? PRODUCT_TITLE : PRODUCT2_TITLE,
        });
      return id === OLD_RETURN_ID ? oldReturn : current;
    },
    create_record: ({ req }) => {
      created = req;
      const { data } = req as { data: DataRecord['data'] };
      // 伺服器從進貨單帶入表頭與明細，計算金額：2 × 12.345 = 24.69
      current = purchasingRecord({
        id: RETURN_ID,
        record_no: 'PR202611-0002',
        title: 'PR202611-0002 美商五金',
        data: {
          ...data,
          supplier_id: SUPPLIER_ID,
          currency_code: 'USD',
          exchange_rate: '32.150000',
          tax_type: 'zero_rated',
          status: 'draft',
          subtotal: '24.6900',
          tax_amount: '0.0000',
          total: '24.6900',
          total_twd: '793.78',
        },
        relations: {
          goods_receipt_id: rel(RECEIPT_ID, RECEIPT_TITLE),
          supplier_id: rel(SUPPLIER_ID, SUPPLIER_TITLE),
        },
        document: doc(null),
        lines: {
          purchase_return_line: [
            purchasingRecord({
              id: RETURN_LINE_ID,
              data: {
                goods_receipt_line_id: RECEIPT_LINE_1,
                product_id: PRODUCT_ID,
                lot_no: 'US-2611',
                qty: '2.0000',
                unit_price: '12.3450',
                amount: '24.6900',
                unit_cost_twd: null,
              },
              relations: { product_id: rel(PRODUCT_ID, PRODUCT_TITLE) },
            }),
          ],
        },
      });
      return current;
    },
    post_record: () => {
      state = 'posted';
      const line = current!.lines!.purchase_return_line![0]!;
      current = {
        ...current!,
        version: 2,
        data: { ...current!.data, status: 'posted' },
        document: doc('2026-11-18T05:00:00Z'),
        lines: {
          purchase_return_line: [{ ...line, data: { ...line.data, unit_cost_twd: '396.8918' } }],
        },
      };
      return current;
    },
    void_record: () => {
      state = 'void';
      current = {
        ...current!,
        version: 3,
        data: { ...current!.data, status: 'void' },
        document: doc('2026-11-18T05:00:00Z', '2026-11-18T06:00:00Z'),
      };
      return current;
    },
    list_product_lots: () =>
      page([
        lot({
          lot_no: 'US-2611',
          qty_in: '6.0000',
          qty_remaining: state === 'posted' ? '2.0000' : '4.0000',
          received_date: '2026-11-18',
          source_type: 'goods_receipt_line',
          source_doc_id: RECEIPT_ID,
          source_record_no: 'GR202611-0001',
          unit_cost_twd: '396.8918',
        }),
      ]),
    list_inventory_transactions: () =>
      page(
        state === 'posted'
          ? [
              {
                id: '0196f5a0-0000-7000-8000-000000000e01',
                txn_at: '2026-11-18T05:00:00Z',
                txn_date: '2026-11-18',
                txn_type: 'purchase_return',
                product_id: PRODUCT_ID,
                product_code: 'A001',
                product_name: '不鏽鋼螺絲 M6',
                lot_id: '0196f5a0-0000-7000-8000-000000000701',
                lot_no: 'US-2611',
                qty_change: '-2.0000',
                unit_cost_twd: '396.8918',
                ref_doc_type: 'purchase_return',
                ref_doc_id: RETURN_ID,
                ref_record_no: 'PR202611-0002',
                ref_line_id: RETURN_LINE_ID,
                created_by: ADMIN.id,
                created_by_name: '王管理',
              },
            ]
          : [],
      ),
  });

  // 選單「進貨」有進貨退回
  await p.goto('/');
  await openNavSection(p, '進貨');
  await expect(p.getByRole('menuitem', { name: /進貨退回/ })).toBeVisible();
  await p.goto('/#/entities/purchase_return/new');
  await expect(p.getByRole('textbox', { name: '退回日期', exact: true })).toHaveValue(
    /^\d{4}-\d{2}-\d{2}$/,
  );

  // 選擇進貨單：帶入可退明細（A001 已退 2 個，剩 4 個）
  await select(p, '進貨單').click();
  await option(p, RECEIPT_TITLE).click();
  await expect(p.getByText('已帶入 GR202611-0001 的 2 筆可退明細')).toBeVisible();
  await expect(select(p, '廠商')).toContainText(SUPPLIER_TITLE);
  await expect(select(p, '幣別')).toContainText('USD');
  await expect(p.getByRole('textbox', { name: '匯率', exact: true })).toHaveValue('32.15');

  const lines = p.getByRole('region', { name: '進貨退回明細' });
  const rows = lines.locator('tbody tr.ant-table-row');
  await expect(rows).toHaveCount(2);
  await expect(lines.getByRole('button', { name: /新增進貨退回明細/ })).toHaveCount(0);
  await expect(rows.nth(0)).toContainText('US-2611');
  await expect(rows.nth(0).getByRole('cell', { name: '4', exact: true })).toBeVisible();
  await expect(rows.nth(1).getByRole('cell', { name: '3', exact: true })).toBeVisible();
  await expect(rows.nth(0).getByRole('textbox', { name: '數量', exact: true })).toHaveValue(
    '4.0000',
  );
  // 4 × 12.345 = 49.38；3 × 7.5 = 22.50
  await expect(p.getByRole('textbox', { name: '未稅金額', exact: true })).toHaveValue('71.8800');
  await expect(p.getByRole('textbox', { name: '台幣總計', exact: true })).toHaveValue('2310.94');

  // 只退 A001 的 2 個
  await rows.nth(0).getByRole('textbox', { name: '數量', exact: true }).fill('2');
  await lines.getByRole('button', { name: '刪除第 2 列' }).click();
  await p.getByRole('textbox', { name: '退回原因', exact: true }).fill('瑕疵品');
  await expect(rows.nth(0).getByRole('textbox', { name: '金額', exact: true })).toHaveValue(
    '24.6900',
  );
  await expect(p.getByRole('textbox', { name: '未稅金額', exact: true })).toHaveValue('24.6900');
  await expect(p.getByRole('textbox', { name: '台幣總計', exact: true })).toHaveValue('793.78');
  await settled(p);
  await lines.scrollIntoViewIfNeeded();
  await p.screenshot({ path: test.info().outputPath('return-lines.png'), fullPage: true });

  await p.getByRole('button', { name: /儲\s?存/ }).click();
  await expect(p.getByLabel('單號')).toHaveValue('PR202611-0002');
  // 只送出可修改的欄位；商品、批號、單價由伺服器從進貨明細帶入
  expect(created).toEqual({
    data: {
      goods_receipt_id: RECEIPT_ID,
      return_date: expect.stringMatching(/^\d{4}-\d{2}-\d{2}$/),
      reason: '瑕疵品',
      notes: null,
    },
    lines: {
      purchase_return_line: [
        { data: { goods_receipt_line_id: RECEIPT_LINE_1, qty: '2', notes: null } },
      ],
    },
  });
  // 存檔後不可更換進貨單
  await expect(p.getByRole('combobox', { name: '進貨單', exact: true })).toBeDisabled();
  await expect(p.getByRole('textbox', { name: '台幣總計', exact: true })).toHaveValue('793.78');

  // 過帳
  await p
    .locator('.page-header-actions')
    .getByRole('button', { name: /過\s?帳/ })
    .click();
  await p
    .getByRole('dialog')
    .getByRole('button', { name: /過\s?帳/ })
    .click();
  await expect(p.locator('.page-header .ant-tag', { hasText: '已過帳' })).toBeVisible();
  await expect(rows.nth(0).getByRole('textbox', { name: '台幣單位成本', exact: true })).toHaveValue(
    '396.8918',
  );
  await expect(p.getByRole('dialog')).toHaveCount(0);
  await settled(p);
  await p.screenshot({ path: test.info().outputPath('return-posted.png'), fullPage: true });

  // 批號連到庫存查詢：剩餘數量扣除，異動流水帳有進貨退回
  await rows.nth(0).getByRole('link', { name: 'US-2611' }).click();
  await expect(p.getByRole('table', { name: '批號' })).toContainText('US-2611');
  const txns = p.getByRole('table', { name: '異動流水帳' });
  await expect(txns).toContainText('進貨退回');
  await expect(txns).toContainText('-2');
  await settled(p);
  await p.screenshot({ path: test.info().outputPath('return-stock.png') });

  // 回到退回單作廢
  await txns.getByRole('link', { name: 'PR202611-0002' }).click();
  await expect(p.getByLabel('單號')).toHaveValue('PR202611-0002');
  await p
    .locator('.page-header-actions')
    .getByRole('button', { name: /作\s?廢/ })
    .click();
  await p
    .getByRole('dialog')
    .getByRole('button', { name: /作\s?廢/ })
    .click();
  await expect(p.locator('.page-header .ant-tag', { hasText: '已作廢' })).toBeVisible();
  await expect(p.getByText(/此單據已作廢，不能修改。過帳時間：.*；作廢時間：/)).toBeVisible();
  await expect(p.getByRole('dialog')).toHaveCount(0);
  await settled(p);
  await p.screenshot({ path: test.info().outputPath('return-void.png'), fullPage: true });
});
