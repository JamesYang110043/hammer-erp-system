import { expect, test } from '@playwright/test';
import type { Locator, Page } from '@playwright/test';

import type { Record as DataRecord } from '../src/bindings';
import { PRODUCT_ID, lot } from '../src/test/documentFixtures';
import {
  CURRENCIES,
  ORDER_ID,
  PURCHASING_ENTITIES,
  RECEIPT_ID,
  SUPPLIER_ID,
  SUPPLIER_TITLE,
  TAX_RATES,
  purchasingEntityDetail,
  purchasingRecord,
} from '../src/test/purchasingFixtures';
import { openNavSection } from './nav';
import { mockTauri } from './tauri-mock';
import type { E2eCommandHandler } from './tauri-mock';

/**
 * 進貨模組（docs/architecture.md §8.8）：外幣採購單 → 確認 → 進貨單從採購單帶入 → 過帳 → 庫存查詢。
 * 伺服器以 mock 模擬，存檔的金額使用 §8.8 範例二的結果（與畫面預覽比對）。
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
const ORDER_LINE_1 = '0196f5a0-0000-7000-8000-000000000b01';
const ORDER_LINE_2 = '0196f5a0-0000-7000-8000-000000000b02';

const page = <T>(items: T[]) => ({ items, total: items.length, page: 1, page_size: 50 });

/** 表格載入完成、淡入動畫結束（截圖前）。 */
async function settled(p: Page) {
  await expect(p.locator('.ant-spin-spinning, .ant-spin-blur')).toHaveCount(0);
}

/** 共用的 mock：登入、資料類別、幣別、稅別、匯率（2026-11-15 起 USD 32.15）、選擇器。 */
function baseHandlers(): Record<string, E2eCommandHandler> {
  return {
    get_server_url: () => 'http://localhost:8080',
    health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
    setup_status: () => ({ needs_setup: false }),
    current_user: () => ADMIN,
    list_entities: () => PURCHASING_ENTITIES,
    get_entity: ({ key }) => purchasingEntityDetail(key as string),
    list_currencies: () => CURRENCIES,
    list_tax_rates: () => TAX_RATES,
    effective_exchange_rate: ({ query }) => {
      const { currency_code, date } = query as { currency_code: string; date: string };
      if (currency_code === 'TWD') {
        return { currency_code, date, rate: '1.000000', rate_date: null };
      }
      return date >= '2026-11-15'
        ? { currency_code, date, rate: '32.150000', rate_date: '2026-11-15' }
        : { currency_code, date, rate: null, rate_date: null };
    },
    lookup_records: ({ key }) =>
      key === 'supplier'
        ? [{ id: SUPPLIER_ID, title: SUPPLIER_TITLE }]
        : key === 'purchase_order'
          ? [{ id: ORDER_ID, title: 'PO202611-0001 美商五金' }]
          : [
              { id: PRODUCT_ID, title: PRODUCT_TITLE },
              { id: PRODUCT2_ID, title: PRODUCT2_TITLE },
            ],
  };
}

function supplierRecord() {
  return purchasingRecord({
    id: SUPPLIER_ID,
    title: SUPPLIER_TITLE,
    data: { code: 'S001', name: '美商五金', default_currency_code: 'USD' },
  });
}

function productRecord(id: string) {
  const title = id === PRODUCT_ID ? PRODUCT_TITLE : PRODUCT2_TITLE;
  return purchasingRecord({ id, title, data: { code: title.split(' ')[0] } });
}

const rel = (id: string, title: string) => ({ id, title, deleted: false });

/** 採購明細（§8.8 範例二）。 */
function orderLine(id: string, productId: string, qty: string, price: string, amount: string) {
  return purchasingRecord({
    id,
    title: productId === PRODUCT_ID ? PRODUCT_TITLE : PRODUCT2_TITLE,
    data: { product_id: productId, qty, unit_price: price, amount, received_qty: '0.0000' },
    relations: {
      product_id: rel(productId, productId === PRODUCT_ID ? PRODUCT_TITLE : PRODUCT2_TITLE),
    },
  });
}

/** antd Select（以 combobox 的名稱找到外框，點擊打開選單）。 */
function select(scope: Page | Locator, name: string) {
  const root = 'page' in scope && typeof scope.page === 'function' ? scope.page() : (scope as Page);
  return scope.locator('.ant-select', { has: root.getByRole('combobox', { name, exact: true }) });
}

/** 最後打開的下拉選單中的選項（前一個選單可能還在關閉的動畫中）。 */
function option(p: Page, title: string) {
  return p
    .locator('.ant-select-dropdown:not(.ant-select-dropdown-hidden)')
    .last()
    .locator('.ant-select-item-option', { hasText: title });
}

test('foreign purchase order: supplier fills currency and rate, preview matches server, confirm', async ({
  page: p,
}) => {
  let current: DataRecord | null = null;
  let created: unknown;

  await mockTauri(p, {
    ...baseHandlers(),
    get_record: ({ key, id }) =>
      key === 'supplier'
        ? supplierRecord()
        : key === 'product'
          ? productRecord(id as string)
          : current,
    create_record: ({ req }) => {
      created = req;
      const { data } = req as { data: DataRecord['data'] };
      // 伺服器的計算結果（§8.8 範例二）
      current = purchasingRecord({
        id: ORDER_ID,
        record_no: 'PO202611-0001',
        title: 'PO202611-0001 美商五金',
        data: {
          ...data,
          exchange_rate: '32.150000',
          status: 'draft',
          subtotal: '145.9500',
          tax_amount: '0.0000',
          total: '145.9500',
          total_twd: '4692.29',
        },
        relations: { supplier_id: rel(SUPPLIER_ID, SUPPLIER_TITLE) },
        lines: {
          purchase_order_line: [
            orderLine(ORDER_LINE_1, PRODUCT_ID, '10.0000', '12.3450', '123.4500'),
            orderLine(ORDER_LINE_2, PRODUCT2_ID, '3.0000', '7.5000', '22.5000'),
          ],
        },
        document: {
          posted_at: null,
          posted_by: null,
          voided_at: null,
          voided_by: null,
          closed_at: null,
          closed_by: null,
        },
      });
      return current;
    },
    confirm_record: () => {
      current = {
        ...current!,
        version: 2,
        data: { ...current!.data, status: 'confirmed' },
        document: { ...current!.document!, posted_at: '2026-11-18T03:00:00Z' },
      };
      return current;
    },
  });

  // 從選單「進貨」開啟採購單
  await p.goto('/');
  await openNavSection(p, '進貨');
  await expect(p.getByRole('menuitem', { name: /採購單/ })).toBeVisible();
  await expect(p.getByRole('menuitem', { name: /進貨單/ })).toBeVisible();
  await p.goto('/#/entities/purchase_order/new');

  // 採購日期改成沒有匯率的日期：選廠商後帶入幣別，但找不到匯率
  const date = p.getByRole('textbox', { name: '採購日期', exact: true });
  await expect(date).toHaveValue(/^\d{4}-\d{2}-\d{2}$/);
  await date.click();
  await date.fill('2026-11-01');
  await date.press('Tab');
  await select(p, '廠商').click();
  await option(p, SUPPLIER_TITLE).click();
  await expect(select(p, '幣別')).toContainText('USD');
  await expect(select(p, '稅別')).toContainText('零稅率');
  const note = p.getByTestId('exchange-rate-note');
  await expect(note).toHaveText('找不到 USD 在 2026-11-01 以前的匯率，請輸入匯率');
  await settled(p);
  await p.screenshot({ path: test.info().outputPath('order-missing-rate.png'), fullPage: true });

  // 改成有匯率的日期：匯率原本是空的，直接帶入並顯示匯率日期
  await date.click();
  await date.fill('2026-11-18');
  await date.press('Tab');
  await expect(p.getByRole('textbox', { name: '匯率', exact: true })).toHaveValue('32.15');
  await expect(note).toHaveText('匯率日期：2026-11-15');

  // 明細：金額即時預覽（§8.8 範例二）
  const lines = p.getByRole('region', { name: '採購明細' });
  const add = lines.getByRole('button', { name: /新增採購明細/ });
  await add.click();
  await add.click();
  const rows = lines.locator('tbody tr.ant-table-row');
  await select(rows.nth(0), '商品').click();
  await option(p, PRODUCT_TITLE).click();
  await rows.nth(0).getByRole('textbox', { name: '數量', exact: true }).fill('10');
  await rows.nth(0).getByRole('textbox', { name: '單價', exact: true }).fill('12.345');
  await select(rows.nth(1), '商品').click();
  await option(p, PRODUCT2_TITLE).click();
  await rows.nth(1).getByRole('textbox', { name: '數量', exact: true }).fill('3');
  await rows.nth(1).getByRole('textbox', { name: '單價', exact: true }).fill('7.5');
  await expect(rows.nth(0).getByRole('textbox', { name: '金額', exact: true })).toHaveValue(
    '123.4500',
  );
  await expect(rows.nth(1).getByRole('textbox', { name: '金額', exact: true })).toHaveValue(
    '22.5000',
  );
  await expect(p.getByRole('textbox', { name: '未稅金額', exact: true })).toHaveValue('145.9500');
  await expect(p.getByRole('textbox', { name: '稅額', exact: true })).toHaveValue('0.0000');
  await expect(p.getByRole('textbox', { name: '總計', exact: true })).toHaveValue('145.9500');
  await expect(p.getByRole('textbox', { name: '台幣總計', exact: true })).toHaveValue('4692.29');
  await settled(p);
  await p.screenshot({ path: test.info().outputPath('order-preview.png'), fullPage: true });

  await p.getByRole('button', { name: /儲\s?存/ }).click();
  await expect(p.getByLabel('單號')).toHaveValue('PO202611-0001');
  expect(created).toMatchObject({
    data: {
      supplier_id: SUPPLIER_ID,
      order_date: '2026-11-18',
      currency_code: 'USD',
      exchange_rate: '32.15',
      tax_type: 'zero_rated',
    },
    lines: {
      purchase_order_line: [
        { data: { product_id: PRODUCT_ID, qty: '10', unit_price: '12.345' } },
        { data: { product_id: PRODUCT2_ID, qty: '3', unit_price: '7.5' } },
      ],
    },
  });
  // 存檔後的值（伺服器計算）與預覽相同
  await expect(p.getByRole('textbox', { name: '未稅金額', exact: true })).toHaveValue('145.9500');
  await expect(p.getByRole('textbox', { name: '台幣總計', exact: true })).toHaveValue('4692.29');

  // 確認 → 已確認，可以結案或作廢
  await p
    .locator('.page-header-actions')
    .getByRole('button', { name: /確\s?認/ })
    .click();
  await p
    .getByRole('dialog')
    .getByRole('button', { name: /確\s?認/ })
    .click();
  await expect(p.locator('.page-header .ant-tag', { hasText: '已確認' })).toBeVisible();
  await expect(
    p.locator('.page-header-actions').getByRole('button', { name: /結\s?案/ }),
  ).toBeVisible();
  await expect(
    p.locator('.page-header-actions').getByRole('button', { name: /作\s?廢/ }),
  ).toBeVisible();
  await expect(p.getByText(/此單據已確認，不能修改。確認時間：/)).toBeVisible();
  await expect(p.getByRole('dialog')).toHaveCount(0);
  await settled(p);
  await p.screenshot({ path: test.info().outputPath('order-confirmed.png'), fullPage: true });
});

test('goods receipt from purchase order: fill unreceived lines, post, lot and cost, stock', async ({
  page: p,
}) => {
  let receipt: DataRecord | null = null;
  let created: unknown;
  let posted = false;
  const receiptLines = [
    '0196f5a0-0000-7000-8000-000000000c01',
    '0196f5a0-0000-7000-8000-000000000c02',
  ];

  const order = purchasingRecord({
    id: ORDER_ID,
    record_no: 'PO202611-0001',
    title: 'PO202611-0001 美商五金',
    data: {
      supplier_id: SUPPLIER_ID,
      order_date: '2026-11-10',
      currency_code: 'USD',
      exchange_rate: '31.900000',
      tax_type: 'zero_rated',
      status: 'partially_received',
    },
    relations: { supplier_id: rel(SUPPLIER_ID, SUPPLIER_TITLE) },
    lines: {
      purchase_order_line: [
        {
          ...orderLine(ORDER_LINE_1, PRODUCT_ID, '10.0000', '12.3450', '123.4500'),
          data: {
            product_id: PRODUCT_ID,
            qty: '10.0000',
            unit_price: '12.3450',
            amount: '123.4500',
            received_qty: '4.0000',
          },
        },
        orderLine(ORDER_LINE_2, PRODUCT2_ID, '3.0000', '7.5000', '22.5000'),
      ],
    },
  });

  await mockTauri(p, {
    ...baseHandlers(),
    get_record: ({ key, id }) =>
      key === 'supplier'
        ? supplierRecord()
        : key === 'product'
          ? productRecord(id as string)
          : key === 'purchase_order'
            ? order
            : receipt,
    create_record: ({ req }) => {
      created = req;
      const { data, lines } = req as {
        data: DataRecord['data'];
        lines: { goods_receipt_line: { data: DataRecord['data'] }[] };
      };
      receipt = purchasingRecord({
        id: RECEIPT_ID,
        record_no: 'GR202611-0001',
        title: 'GR202611-0001 美商五金',
        data: {
          ...data,
          exchange_rate: '32.150000',
          status: 'draft',
          subtotal: '96.5700',
          tax_amount: '0.0000',
          total: '96.5700',
          total_twd: '3104.73',
        },
        relations: {
          supplier_id: rel(SUPPLIER_ID, SUPPLIER_TITLE),
          purchase_order_id: rel(ORDER_ID, 'PO202611-0001 美商五金'),
        },
        lines: {
          goods_receipt_line: lines.goods_receipt_line.map((line, i) =>
            purchasingRecord({
              id: receiptLines[i]!,
              data: {
                ...line.data,
                amount: i === 0 ? '74.0700' : '22.5000',
                unit_cost_twd: null,
              },
              relations: {
                product_id: rel(
                  line.data.product_id as string,
                  i === 0 ? PRODUCT_TITLE : PRODUCT2_TITLE,
                ),
              },
            }),
          ),
        },
      });
      return receipt;
    },
    post_record: () => {
      posted = true;
      const lines = receipt!.lines!.goods_receipt_line!;
      receipt = {
        ...receipt!,
        version: 2,
        data: { ...receipt!.data, status: 'posted' },
        document: {
          posted_at: '2026-11-18T03:00:00Z',
          posted_by: null,
          voided_at: null,
          voided_by: null,
          closed_at: null,
          closed_by: null,
        },
        lines: {
          goods_receipt_line: [
            { ...lines[0]!, data: { ...lines[0]!.data, unit_cost_twd: '396.8918' } },
            {
              ...lines[1]!,
              // 留空的批號由伺服器自動產生
              data: { ...lines[1]!.data, lot_no: 'GR202611-0001-2', unit_cost_twd: '241.1250' },
            },
          ],
        },
      };
      return receipt;
    },
    list_product_lots: () =>
      page(
        posted
          ? [
              lot({
                lot_no: 'US-2611',
                qty_in: '6.0000',
                qty_remaining: '6.0000',
                received_date: '2026-11-18',
                source_type: 'goods_receipt_line',
                source_doc_id: RECEIPT_ID,
                source_record_no: 'GR202611-0001',
                unit_cost_twd: '396.8918',
              }),
            ]
          : [],
      ),
    list_inventory_transactions: () => page([]),
  });

  await p.goto('/#/entities/goods_receipt/new');
  await expect(p.getByRole('textbox', { name: '進貨日期', exact: true })).toHaveValue(
    /^\d{4}-\d{2}-\d{2}$/,
  );
  const date = p.getByRole('textbox', { name: '進貨日期', exact: true });
  await date.click();
  await date.fill('2026-11-18');
  await date.press('Tab');

  // 選擇採購單：帶入廠商、幣別、稅別、進貨日期的匯率與未進貨明細
  await select(p, '採購單').click();
  await option(p, 'PO202611-0001 美商五金').click();
  await expect(p.getByText('已帶入 PO202611-0001 的 2 筆未進貨明細')).toBeVisible();
  await expect(select(p, '廠商')).toContainText(SUPPLIER_TITLE);
  await expect(select(p, '幣別')).toContainText('USD');
  await expect(select(p, '稅別')).toContainText('零稅率');
  await expect(p.getByRole('textbox', { name: '匯率', exact: true })).toHaveValue('32.15');
  await expect(p.getByTestId('exchange-rate-note')).toHaveText('匯率日期：2026-11-15');

  const lines = p.getByRole('region', { name: '進貨明細' });
  const rows = lines.locator('tbody tr.ant-table-row');
  await expect(rows).toHaveCount(2);
  await expect(rows.nth(0).getByRole('textbox', { name: '數量', exact: true })).toHaveValue(
    '6.0000',
  );
  await expect(rows.nth(1).getByRole('textbox', { name: '數量', exact: true })).toHaveValue(
    '3.0000',
  );
  await expect(rows.nth(0).getByRole('textbox', { name: '批號', exact: true })).toHaveAttribute(
    'placeholder',
    '留空則過帳時自動產生',
  );
  await rows.nth(0).getByRole('textbox', { name: '批號', exact: true }).fill('US-2611');
  // 6 × 12.345 = 74.07；3 × 7.5 = 22.50
  await expect(rows.nth(0).getByRole('textbox', { name: '金額', exact: true })).toHaveValue(
    '74.0700',
  );
  await expect(p.getByRole('textbox', { name: '總計', exact: true })).toHaveValue('96.5700');
  await expect(p.getByRole('textbox', { name: '台幣總計', exact: true })).toHaveValue('3104.73');
  await settled(p);
  await lines.scrollIntoViewIfNeeded();
  await p.screenshot({ path: test.info().outputPath('receipt-from-order.png'), fullPage: true });

  await p.getByRole('button', { name: /儲\s?存/ }).click();
  await expect(p.getByLabel('單號')).toHaveValue('GR202611-0001');
  expect(created).toMatchObject({
    data: {
      purchase_order_id: ORDER_ID,
      supplier_id: SUPPLIER_ID,
      currency_code: 'USD',
      exchange_rate: '32.15',
      tax_type: 'zero_rated',
    },
    lines: {
      goods_receipt_line: [
        {
          data: {
            purchase_order_line_id: ORDER_LINE_1,
            product_id: PRODUCT_ID,
            qty: '6.0000',
            unit_price: '12.3450',
            lot_no: 'US-2611',
          },
        },
        {
          data: {
            purchase_order_line_id: ORDER_LINE_2,
            product_id: PRODUCT2_ID,
            qty: '3.0000',
            lot_no: null,
          },
        },
      ],
    },
  });

  // 過帳：顯示批號（自動產生的也是）與台幣單位成本
  await p
    .locator('.page-header-actions')
    .getByRole('button', { name: /過\s?帳/ })
    .click();
  await p
    .getByRole('dialog')
    .getByRole('button', { name: /過\s?帳/ })
    .click();
  await expect(p.locator('.page-header .ant-tag', { hasText: '已過帳' })).toBeVisible();
  await expect(rows.nth(0).getByRole('link', { name: 'US-2611' })).toBeVisible();
  await expect(rows.nth(1).getByRole('link', { name: 'GR202611-0001-2' })).toBeVisible();
  await expect(rows.nth(0).getByRole('textbox', { name: '台幣單位成本', exact: true })).toHaveValue(
    '396.8918',
  );
  await expect(rows.nth(1).getByRole('textbox', { name: '台幣單位成本', exact: true })).toHaveValue(
    '241.1250',
  );
  await expect(p.getByRole('dialog')).toHaveCount(0);
  await settled(p);
  await lines.scrollIntoViewIfNeeded();
  await p.screenshot({ path: test.info().outputPath('receipt-posted.png'), fullPage: true });

  // 批號連到庫存查詢，看得到新批號
  await rows.nth(0).getByRole('link', { name: 'US-2611' }).click();
  await expect(p.getByRole('table', { name: '批號' })).toContainText('US-2611');
  await expect(p.getByRole('table', { name: '批號' })).toContainText('GR202611-0001');
  await settled(p);
  await p.screenshot({ path: test.info().outputPath('receipt-stock.png') });
});
