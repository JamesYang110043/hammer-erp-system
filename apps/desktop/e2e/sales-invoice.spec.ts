import { expect, test } from '@playwright/test';
import type { Locator, Page } from '@playwright/test';

import type { Record as DataRecord } from '../src/bindings';
import { PRODUCT_ID } from '../src/test/documentFixtures';
import {
  INVOICING_ENTITIES,
  SALES_ALLOWANCE_ID,
  SALES_INVOICE_ID,
  invoicingEntityDetail,
} from '../src/test/invoicingFixtures';
import { CURRENCIES, TAX_RATES, purchasingRecord } from '../src/test/purchasingFixtures';
import { CUSTOMER_ID, CUSTOMER_TITLE, SHIPMENT_ID } from '../src/test/salesFixtures';
import { openNavSection } from './nav';
import { CommandRejection, mockTauri } from './tauri-mock';
import type { E2eCommandHandler } from './tauri-mock';

/**
 * 銷項發票與銷貨折讓（docs/architecture.md §8.11）：出貨 → 開立發票（選客戶帶入買受人、勾選出貨單帶入
 * 明細、手動稅額）→ 開立折讓（可折讓金額）；跨期作廢的警告；出貨單顯示發票號碼、作廢失敗時的提示。
 * 伺服器以 mock 模擬。
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
const SHIPMENT_TITLE = 'SH202611-0001 大同五金行';
const INVOICE_TITLE = 'AB12345678 大同五金行股份有限公司';
const DOC = {
  posted_at: null,
  posted_by: null,
  voided_at: null,
  voided_by: null,
  closed_at: null,
  closed_by: null,
};

const page = <T>(items: T[]) => ({ items, total: items.length, page: 1, page_size: 50 });
const rel = (id: string, title: string) => ({ id, title, deleted: false });

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

const textbox = (p: Page, name: string) => p.getByRole('textbox', { name, exact: true });

/** 已過帳的出貨單：A001 10 × 120，折扣 15%，未稅。 */
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
    subtotal: '1020.0000',
    tax_amount: '51.0000',
    total: '1071.0000',
  },
  relations: { customer_id: rel(CUSTOMER_ID, CUSTOMER_TITLE) },
  document: { ...DOC, posted_at: '2026-11-18T03:00:00Z' },
  lines: {
    shipment_line: [
      purchasingRecord({
        id: '0196f5a0-0000-7000-8000-000000000f01',
        title: PRODUCT_TITLE,
        data: {
          product_id: PRODUCT_ID,
          qty: '10.0000',
          unit_price: '120.0000',
          discount_rate: '0.1500',
          amount: '1020.0000',
        },
        relations: { product_id: rel(PRODUCT_ID, PRODUCT_TITLE) },
      }),
    ],
  },
});

const CUSTOMER = purchasingRecord({
  id: CUSTOMER_ID,
  title: CUSTOMER_TITLE,
  data: {
    code: 'C001',
    name: '大同五金行',
    tax_id: '12345678',
    invoice_title: '大同五金行股份有限公司',
    address: '台北市中山區',
    invoice_address: '台北市中山區南京東路 100 號',
  },
});

/** 共通的 mock（各測試再補上單據的讀寫）。 */
function baseHandlers(): Record<string, E2eCommandHandler> {
  return {
    get_server_url: () => 'http://localhost:8080',
    health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
    setup_status: () => ({ needs_setup: false }),
    current_user: () => ADMIN,
    list_entities: () => INVOICING_ENTITIES,
    get_entity: ({ key }) => invoicingEntityDetail(key as string),
    list_currencies: () => CURRENCIES,
    list_tax_rates: () => TAX_RATES,
    lookup_records: ({ key }) =>
      key === 'customer'
        ? [{ id: CUSTOMER_ID, title: CUSTOMER_TITLE }]
        : key === 'shipment'
          ? [{ id: SHIPMENT_ID, title: SHIPMENT_TITLE }]
          : key === 'sales_invoice'
            ? [{ id: SALES_INVOICE_ID, title: INVOICE_TITLE }]
            : [],
    list_records: () => page([]),
    list_invoice_links: () => [],
  };
}

test('sales invoice from shipments: buyer, lines, manual tax, issue, allowance', async ({
  page: p,
}) => {
  let invoice: DataRecord | null = null;
  let allowance: DataRecord | null = null;
  const created: Record<string, unknown> = {};

  await mockTauri(p, {
    ...baseHandlers(),
    list_uninvoiced_shipments: ({ query }) => {
      expect((query as { customer_id: string }).customer_id).toBe(CUSTOMER_ID);
      return page([
        {
          id: SHIPMENT_ID,
          record_no: 'SH202611-0001',
          ship_date: '2026-11-18',
          customer_id: CUSTOMER_ID,
          customer_code: 'C001',
          customer_name: '大同五金行',
          tax_type: 'taxable',
          tax_included: false,
          subtotal: '1020.0000',
          tax_amount: '51.0000',
          total: '1071.0000',
          returned_total: '0.0000',
          net_total: '1071.0000',
        },
      ]);
    },
    get_record: ({ key, id }) =>
      key === 'shipment'
        ? SHIPMENT
        : key === 'customer'
          ? CUSTOMER
          : key === 'product'
            ? purchasingRecord({ id: id as string, title: PRODUCT_TITLE, data: {} })
            : key === 'sales_allowance'
              ? allowance
              : invoice,
    create_record: ({ key, req }) => {
      created[key as string] = req;
      const { data, lines } = req as {
        data: DataRecord['data'];
        lines: Record<string, { data: DataRecord['data'] }[]>;
      };
      if (key === 'sales_invoice') {
        invoice = purchasingRecord({
          id: SALES_INVOICE_ID,
          record_no: 'SI202611-0001',
          title: INVOICE_TITLE,
          data: {
            ...data,
            // 伺服器把號碼轉成大寫
            invoice_no: String(data.invoice_no).toUpperCase(),
            tax_period: '202611',
            status: 'draft',
            subtotal: '1020.0000',
            tax_amount: '50.0000',
            total: '1070.0000',
          },
          relations: { customer_id: rel(CUSTOMER_ID, CUSTOMER_TITLE) },
          document: DOC,
          lines: {
            sales_invoice_line: (lines.sales_invoice_line ?? []).map((l, i) =>
              purchasingRecord({
                id: `0196f5a0-0000-7000-8000-00000000e1${i}0`,
                title: String(l.data.description),
                data: { ...l.data, amount: '1020.0000' },
                relations: { product_id: rel(PRODUCT_ID, PRODUCT_TITLE) },
              }),
            ),
            sales_invoice_shipment: (lines.sales_invoice_shipment ?? []).map((l, i) =>
              purchasingRecord({
                id: `0196f5a0-0000-7000-8000-00000000e2${i}0`,
                title: 'SH202611-0001',
                data: l.data,
                relations: { shipment_id: rel(SHIPMENT_ID, SHIPMENT_TITLE) },
              }),
            ),
          },
        });
        return invoice;
      }
      allowance = purchasingRecord({
        id: SALES_ALLOWANCE_ID,
        record_no: 'SA202611-0001',
        title: 'SA202611-0001 大同五金行',
        data: {
          ...data,
          customer_id: CUSTOMER_ID,
          tax_type: 'taxable',
          tax_period: '202611',
          status: 'draft',
          subtotal: '100.0000',
          tax_amount: '5.0000',
          total: '105.0000',
        },
        relations: {
          customer_id: rel(CUSTOMER_ID, CUSTOMER_TITLE),
          sales_invoice_id: rel(SALES_INVOICE_ID, INVOICE_TITLE),
        },
        document: DOC,
        lines: {
          sales_allowance_line: (lines.sales_allowance_line ?? []).map((l) =>
            purchasingRecord({
              id: '0196f5a0-0000-7000-8000-00000000e301',
              title: String(l.data.description),
              data: { ...l.data, amount: '100.0000' },
            }),
          ),
        },
      });
      return allowance;
    },
    post_record: ({ key }) => {
      const posted = (r: DataRecord) => ({
        ...r,
        version: 2,
        data: { ...r.data, status: 'posted' },
        document: { ...DOC, posted_at: '2026-11-19T03:00:00Z' },
      });
      if (key === 'sales_invoice') {
        invoice = posted(invoice!);
        return invoice;
      }
      allowance = posted(allowance!);
      return allowance;
    },
  });

  // 左側選單的「發票」分組
  await p.goto('/');
  await openNavSection(p, '發票');
  await expect(p.getByRole('menuitem', { name: /銷項發票/ })).toBeVisible();
  await expect(p.getByRole('menuitem', { name: /銷貨折讓/ })).toBeVisible();
  await p.goto('/#/entities/sales_invoice/new');
  await expect(textbox(p, '發票日期')).toHaveValue(/^\d{4}-\d{2}-\d{2}$/);
  await expect(p.getByText(/^期別：\d{4} 年 \d{1,2}–\d{1,2} 月$/)).toBeVisible();
  await expect(select(p, '稅別')).toContainText('應稅');
  await expect(select(p, '發票種類')).toContainText('三聯式');

  // 選擇客戶：帶入買受人資訊（發票抬頭、發票地址優先）
  await textbox(p, '發票號碼').fill('ab12345678');
  await select(p, '客戶').click();
  await option(p, CUSTOMER_TITLE).click();
  await expect(textbox(p, '買受人統一編號')).toHaveValue('12345678');
  await expect(textbox(p, '買受人')).toHaveValue('大同五金行股份有限公司');
  await expect(textbox(p, '買受人地址')).toHaveValue('台北市中山區南京東路 100 號');

  // 勾選出貨單：加入關聯並帶入明細（120 × 0.85 = 102 未稅）
  await p.getByRole('button', { name: /選擇出貨單/ }).click();
  const dialog = p.getByRole('dialog', { name: '選擇出貨單' });
  await expect(dialog.getByRole('table', { name: '尚未開立發票的出貨單' })).toContainText(
    'SH202611-0001',
  );
  await expect(dialog).toContainText('1,071');
  await dialog.getByRole('checkbox').nth(1).check();
  await settled(p);
  await p.screenshot({ path: test.info().outputPath('invoice-pick-shipments.png') });
  await dialog.getByRole('button', { name: /帶\s?入/ }).click();
  await expect(dialog).toBeHidden();

  const lines = p.getByRole('region', { name: '發票明細' });
  const rows = lines.locator('tbody tr.ant-table-row');
  const cell = (i: number, name: string) => rows.nth(i).getByRole('textbox', { name, exact: true });
  await expect(rows).toHaveCount(1);
  await expect(cell(0, '品名')).toHaveValue('不鏽鋼螺絲 M6');
  await expect(cell(0, '數量')).toHaveValue('10.0000');
  await expect(cell(0, '單價')).toHaveValue('102.0000');
  await expect(cell(0, '金額')).toHaveValue('1020.0000');
  const links = p.getByRole('region', { name: '發票出貨單' });
  await expect(links.locator('tbody tr.ant-table-row')).toHaveCount(1);
  await expect(textbox(p, '稅額')).toHaveValue('51.0000');
  await expect(textbox(p, '總計')).toHaveValue('1071.0000');

  // 手動修正稅額為紙本上的值（±1 元），總計跟著變
  await textbox(p, '稅額（手動）').fill('50');
  await expect(textbox(p, '稅額')).toHaveValue('50.0000');
  await expect(textbox(p, '總計')).toHaveValue('1070.0000');
  await settled(p);
  await lines.scrollIntoViewIfNeeded();
  await p.screenshot({ path: test.info().outputPath('invoice-draft.png'), fullPage: true });

  await p.getByRole('button', { name: /儲\s?存/ }).click();
  await expect(p.getByLabel('單號')).toHaveValue('SI202611-0001');
  expect(created.sales_invoice).toMatchObject({
    data: {
      invoice_no: 'ab12345678',
      customer_id: CUSTOMER_ID,
      buyer_tax_id: '12345678',
      buyer_name: '大同五金行股份有限公司',
      tax_amount_manual: '50',
      tax_type: 'taxable',
    },
    lines: {
      sales_invoice_line: [
        {
          data: {
            product_id: PRODUCT_ID,
            description: '不鏽鋼螺絲 M6',
            qty: '10.0000',
            unit_price: '102.0000',
          },
        },
      ],
      sales_invoice_shipment: [{ data: { shipment_id: SHIPMENT_ID } }],
    },
  });

  // 開立
  const action = (name: RegExp) => p.locator('.ant-card-extra').getByRole('button', { name });
  await action(/開\s?立$/).click();
  await expect(p.getByRole('dialog')).toContainText('開立後會檢查號碼不重複');
  await p
    .getByRole('dialog')
    .getByRole('button', { name: /開\s?立/ })
    .click();
  await expect(p.locator('.ant-card-head .ant-tag', { hasText: '已開立' })).toBeVisible();
  await expect(p.getByRole('dialog')).toHaveCount(0);
  await settled(p);
  await p.screenshot({ path: test.info().outputPath('invoice-issued.png'), fullPage: true });

  // 從發票開立折讓：帶入原發票、顯示可折讓金額
  await action(/開立折讓/).click();
  await expect(p).toHaveURL(/sales_allowance\/new\?invoice=/);
  await expect(select(p, '原發票')).toContainText(INVOICE_TITLE);
  await expect(p.getByText('可折讓金額 1,070')).toBeVisible();
  await expect(p.getByText('發票 AB12345678 總計 1,070，已折讓 0')).toBeVisible();
  const allowanceLines = p.getByRole('region', { name: '折讓明細' });
  await allowanceLines.getByRole('button', { name: /新增/ }).click();
  const arow = allowanceLines.locator('tbody tr.ant-table-row').first();
  await arow.getByRole('textbox', { name: '品名', exact: true }).fill('不鏽鋼螺絲 M6 降價');
  await arow.getByRole('textbox', { name: '數量', exact: true }).fill('10');
  await arow.getByRole('textbox', { name: '單價', exact: true }).fill('10');
  await expect(arow.getByRole('textbox', { name: '金額', exact: true })).toHaveValue('100.0000');
  // 原發票的客戶、稅別先帶入：金額預覽計算稅額
  await expect(select(p, '客戶')).toContainText(CUSTOMER_TITLE);
  await expect(textbox(p, '折讓稅額')).toHaveValue('5.0000');
  await expect(textbox(p, '折讓總計')).toHaveValue('105.0000');
  await textbox(p, '折讓原因').fill('品質瑕疵降價');
  await settled(p);
  await p.screenshot({ path: test.info().outputPath('allowance-draft.png'), fullPage: true });
  await p.getByRole('button', { name: /儲\s?存/ }).click();
  await expect(p.getByLabel('單號')).toHaveValue('SA202611-0001');
  expect(created.sales_allowance).toMatchObject({
    data: { sales_invoice_id: SALES_INVOICE_ID, reason: '品質瑕疵降價' },
  });
  await action(/開\s?立$/).click();
  await p
    .getByRole('dialog')
    .getByRole('button', { name: /開\s?立/ })
    .click();
  await expect(p.locator('.ant-card-head .ant-tag', { hasText: '已開立' })).toBeVisible();
  await expect(p.getByRole('dialog')).toHaveCount(0);
  await settled(p);
  await p.screenshot({ path: test.info().outputPath('allowance-issued.png'), fullPage: true });
});

test('void across periods shows a warning but can still be voided', async ({ page: p }) => {
  let invoice = purchasingRecord({
    id: SALES_INVOICE_ID,
    record_no: 'SI202609-0003',
    title: INVOICE_TITLE,
    data: {
      invoice_type: 'triplicate',
      invoice_no: 'AB12345678',
      invoice_date: '2026-09-15',
      tax_period: '202609',
      customer_id: CUSTOMER_ID,
      buyer_tax_id: '12345678',
      buyer_name: '大同五金行股份有限公司',
      tax_type: 'taxable',
      status: 'posted',
      subtotal: '1000.0000',
      tax_amount: '50.0000',
      total: '1050.0000',
    },
    relations: { customer_id: rel(CUSTOMER_ID, CUSTOMER_TITLE) },
    document: { ...DOC, posted_at: '2026-09-15T03:00:00Z' },
    lines: { sales_invoice_line: [], sales_invoice_shipment: [] },
  });
  let voided: Record<string, unknown> | undefined;
  await mockTauri(p, {
    ...baseHandlers(),
    get_record: () => invoice,
    void_invoice_record: (args) => {
      voided = args;
      invoice = {
        ...invoice,
        version: 3,
        data: {
          ...invoice.data,
          status: 'void',
          void_date: args.voidDate as string,
          void_reason: args.voidReason as string,
        },
        document: { ...invoice.document!, voided_at: '2026-11-19T04:00:00Z' },
      };
      return invoice;
    },
  });

  await p.goto(`/#/entities/sales_invoice/${SALES_INVOICE_ID}`);
  await expect(p.locator('.ant-card-head .ant-tag', { hasText: '已開立' })).toBeVisible();
  await expect(p.getByText('期別：2026 年 9–10 月')).toBeVisible();
  await p
    .locator('.ant-card-extra')
    .getByRole('button', { name: /作\s?廢/ })
    .click();
  const dialog = p.getByRole('dialog', { name: '作廢這張銷項發票' });
  await expect(dialog).toBeVisible();
  // 預設今天（不同期）：顯示警告
  await expect(dialog).toContainText(
    '作廢日期與發票不同期（發票 2026 年 9–10 月），請確認是否需要另外處理申報',
  );
  // 原因必填
  await dialog.getByRole('button', { name: /作\s?廢/ }).click();
  await expect(dialog).toContainText('請輸入作廢原因');
  await dialog.getByRole('textbox', { name: '作廢原因' }).fill('買方統編錯誤');
  await expect(dialog).not.toContainText('請輸入作廢原因');
  await settled(p);
  await p.screenshot({ path: test.info().outputPath('void-cross-period.png') });

  // 改成同一期的日期：警告消失
  const date = dialog.getByRole('textbox', { name: '作廢日期' });
  await date.click();
  await date.fill('2026-10-20');
  await date.press('Enter');
  await expect(dialog).not.toContainText('作廢日期與發票不同期');

  await dialog.getByRole('button', { name: /作\s?廢/ }).click();
  await expect(p.locator('.ant-card-head .ant-tag', { hasText: '已作廢' })).toBeVisible();
  expect(voided).toMatchObject({
    key: 'sales_invoice',
    id: SALES_INVOICE_ID,
    voidDate: '2026-10-20',
    voidReason: '買方統編錯誤',
  });
  await expect(textbox(p, '作廢原因')).toHaveValue('買方統編錯誤');
  await settled(p);
  await p.screenshot({ path: test.info().outputPath('invoice-void.png'), fullPage: true });
});

test('shipment shows its invoice numbers and why it cannot be voided', async ({ page: p }) => {
  await mockTauri(p, {
    ...baseHandlers(),
    get_record: ({ key, id }) =>
      key === 'shipment'
        ? SHIPMENT
        : purchasingRecord({ id: id as string, title: PRODUCT_TITLE, data: {} }),
    list_invoice_links: ({ query }) => {
      expect(query).toMatchObject({ entity_key: 'shipment', record_id: SHIPMENT_ID });
      return [
        {
          entity_key: 'sales_invoice',
          record_id: '0196f5a0-0000-7000-8000-000000000e02',
          record_no: 'SI202611-0001',
          invoice_no: 'AB12345677',
          invoice_date: '2026-11-18',
          status: 'void',
        },
        {
          entity_key: 'sales_invoice',
          record_id: SALES_INVOICE_ID,
          record_no: 'SI202611-0002',
          invoice_no: 'AB12345678',
          invoice_date: '2026-11-19',
          status: 'posted',
        },
      ];
    },
    list_inventory_transactions: () => page([]),
    void_record: () => {
      throw new CommandRejection({
        kind: 'api',
        code: 'VALIDATION',
        message: '此出貨單已開立發票 AB12345678，請先作廢發票',
        details: {
          fields: [],
          documents: [
            {
              entity_key: 'sales_invoice',
              record_id: SALES_INVOICE_ID,
              record_no: 'SI202611-0002',
            },
          ],
        },
      });
    },
  });

  await p.goto(`/#/entities/shipment/${SHIPMENT_ID}`);
  await expect(p.getByText('發票：')).toBeVisible();
  await expect(p.getByRole('link', { name: 'AB12345678（已開立）' })).toBeVisible();
  await expect(p.getByRole('link', { name: 'AB12345677（已作廢）' })).toBeVisible();
  await p
    .locator('.ant-card-extra')
    .getByRole('button', { name: /作\s?廢/ })
    .click();
  await p
    .getByRole('dialog')
    .getByRole('button', { name: /作\s?廢/ })
    .click();
  await expect(p.getByText('作廢失敗：此出貨單已開立發票 AB12345678，請先作廢發票')).toBeVisible();
  await expect(p.getByRole('dialog')).toHaveCount(0);
  await settled(p);
  await p.screenshot({ path: test.info().outputPath('shipment-invoices.png'), fullPage: true });
});
