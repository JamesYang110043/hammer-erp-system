import { expect, test } from '@playwright/test';
import type { Locator, Page } from '@playwright/test';

import type { Record as DataRecord } from '../src/bindings';
import {
  PURCHASE_ALLOWANCE_ID,
  PURCHASE_INVOICE_ID,
  PURCHASE_INVOICING_ENTITIES,
  purchaseInvoicingEntityDetail,
} from '../src/test/invoicingFixtures';
import {
  CURRENCIES,
  RECEIPT_ID,
  SUPPLIER_ID,
  SUPPLIER_TITLE,
  TAX_RATES,
  purchasingRecord,
} from '../src/test/purchasingFixtures';
import { openNavSection } from './nav';
import { mockTauri } from './tauri-mock';
import type { E2eCommandHandler } from './tauri-mock';

/**
 * 進項憑證與進貨折讓（docs/architecture.md §8.11）：進貨 → 登錄憑證（選廠商帶入賣方統編、勾選進貨單、
 * 依台幣總計帶入金額）→ 開立進貨折讓（可折讓金額）；進貨單顯示關聯的進項憑證。伺服器以 mock 模擬。
 */

const ADMIN = {
  id: '0196f5a0-0000-7000-8000-000000000001',
  username: 'admin',
  display_name: '王管理',
  role: 'admin',
  permissions: ['record.post', 'record.read', 'record.write'],
  must_change_password: false,
};

const RECEIPT_TITLE = 'GR202611-0001 美商五金';
const INVOICE_TITLE = 'CD12345678 美商五金';
const DOC = {
  posted_at: null,
  posted_by: null,
  voided_at: null,
  voided_by: null,
  closed_at: null,
  closed_by: null,
};

const page = <T>(items: T[]) => ({ items, total: items.length, page: 1, page_size: 50 });
const rel = (id: string, title: string) => ({ id, title, deleted: false, inactive: false });

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

/** 已過帳的進貨單（台幣，總計 1,050）。 */
const RECEIPT = purchasingRecord({
  id: RECEIPT_ID,
  record_no: 'GR202611-0001',
  title: RECEIPT_TITLE,
  data: {
    supplier_id: SUPPLIER_ID,
    receipt_date: '2026-11-10',
    currency_code: 'TWD',
    exchange_rate: '1.000000',
    tax_type: 'taxable',
    status: 'posted',
    subtotal: '1000.0000',
    tax_amount: '50.0000',
    total: '1050.0000',
    total_twd: '1050.00',
  },
  relations: { supplier_id: rel(SUPPLIER_ID, SUPPLIER_TITLE) },
  document: { ...DOC, posted_at: '2026-11-10T03:00:00Z' },
  lines: { goods_receipt_line: [] },
});

function baseHandlers(): Record<string, E2eCommandHandler> {
  return {
    get_server_url: () => 'http://localhost:8080',
    health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
    setup_status: () => ({ needs_setup: false }),
    current_user: () => ADMIN,
    list_entities: () => PURCHASE_INVOICING_ENTITIES,
    get_entity: ({ key }) => purchaseInvoicingEntityDetail(key as string),
    list_currencies: () => CURRENCIES,
    list_tax_rates: () => TAX_RATES,
    lookup_records: ({ key }) =>
      key === 'supplier'
        ? [{ id: SUPPLIER_ID, title: SUPPLIER_TITLE }]
        : key === 'goods_receipt'
          ? [{ id: RECEIPT_ID, title: RECEIPT_TITLE }]
          : key === 'purchase_invoice'
            ? [{ id: PURCHASE_INVOICE_ID, title: INVOICE_TITLE }]
            : [],
    list_records: () => page([]),
    list_invoice_links: () => [],
    list_inventory_transactions: () => page([]),
  };
}

test('purchase invoice from goods receipts: seller, amounts, register, allowance', async ({
  page: p,
}) => {
  let invoice: DataRecord | null = null;
  let allowance: DataRecord | null = null;
  const created: Record<string, unknown> = {};
  let customsQuery: string | undefined;

  await mockTauri(p, {
    ...baseHandlers(),
    list_uninvoiced_receipts: ({ query }) => {
      const q = query as { supplier_id: string; doc_class: string };
      expect(q.supplier_id).toBe(SUPPLIER_ID);
      customsQuery = q.doc_class;
      return page([
        {
          id: RECEIPT_ID,
          record_no: 'GR202611-0001',
          receipt_date: '2026-11-10',
          supplier_id: SUPPLIER_ID,
          supplier_code: 'S001',
          supplier_name: '美商五金',
          currency_code: 'TWD',
          exchange_rate: '1.000000',
          tax_type: 'taxable',
          total: '1050.0000',
          total_twd: '1050.00',
          returned_total: '0.0000',
          net_total: '1050.0000',
        },
      ]);
    },
    get_record: ({ key }) =>
      key === 'supplier'
        ? purchasingRecord({
            id: SUPPLIER_ID,
            title: SUPPLIER_TITLE,
            data: { code: 'S001', name: '美商五金', tax_id: '87654321' },
          })
        : key === 'goods_receipt'
          ? RECEIPT
          : key === 'purchase_allowance'
            ? allowance
            : invoice,
    create_record: ({ key, req }) => {
      created[key as string] = req;
      const { data, lines } = req as {
        data: DataRecord['data'];
        lines?: Record<string, { data: DataRecord['data'] }[]>;
      };
      if (key === 'purchase_invoice') {
        invoice = purchasingRecord({
          id: PURCHASE_INVOICE_ID,
          record_no: 'PI202611-0001',
          title: INVOICE_TITLE,
          data: {
            ...data,
            invoice_no: String(data.invoice_no).toUpperCase(),
            tax_period: '202611',
            status: 'draft',
            total: '1050.0000',
          },
          relations: { supplier_id: rel(SUPPLIER_ID, SUPPLIER_TITLE) },
          document: DOC,
          lines: {
            purchase_invoice_receipt: (lines?.purchase_invoice_receipt ?? []).map((l) =>
              purchasingRecord({
                id: '0196f5a0-0000-7000-8000-00000000e401',
                title: 'GR202611-0001',
                data: l.data,
                relations: { goods_receipt_id: rel(RECEIPT_ID, RECEIPT_TITLE) },
              }),
            ),
          },
        });
        return invoice;
      }
      allowance = purchasingRecord({
        id: PURCHASE_ALLOWANCE_ID,
        record_no: 'PA202611-0001',
        title: 'PA202611-0001 美商五金',
        data: {
          ...data,
          supplier_id: SUPPLIER_ID,
          tax_type: 'taxable',
          tax_period: '202611',
          status: 'draft',
          total: '105.0000',
        },
        relations: {
          supplier_id: rel(SUPPLIER_ID, SUPPLIER_TITLE),
          purchase_invoice_id: rel(PURCHASE_INVOICE_ID, INVOICE_TITLE),
        },
        document: DOC,
        lines: {},
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
      if (key === 'purchase_invoice') {
        invoice = posted(invoice!);
        return invoice;
      }
      allowance = posted(allowance!);
      return allowance;
    },
  });

  await p.goto('/');
  await openNavSection(p, '發票');
  await expect(p.getByRole('menuitem', { name: /進項憑證/ })).toBeVisible();
  await expect(p.getByRole('menuitem', { name: /進貨折讓/ })).toBeVisible();
  await p.goto('/#/entities/purchase_invoice/new');
  await expect(textbox(p, '憑證日期')).toHaveValue(/^\d{4}-\d{2}-\d{2}$/);
  await expect(select(p, '憑證種類')).toContainText('三聯式發票');
  await expect(select(p, '稅別')).toContainText('應稅');

  // 選擇廠商：帶入賣方統一編號
  await textbox(p, '憑證號碼').fill('cd12345678');
  await select(p, '廠商').click();
  await option(p, SUPPLIER_TITLE).click();
  await expect(textbox(p, '賣方統一編號')).toHaveValue('87654321');

  // 勾選進貨單：加入關聯，依台幣總計帶入金額（1,050 → 未稅 1,000、稅 50）
  await p.getByRole('button', { name: /選擇進貨單/ }).click();
  const dialog = p.getByRole('dialog', { name: '選擇進貨單' });
  await expect(dialog.getByRole('table', { name: '尚未取得憑證的進貨單' })).toContainText(
    'GR202611-0001',
  );
  expect(customsQuery).toBe('invoice');
  await dialog.getByRole('checkbox').nth(1).check();
  await expect(dialog.getByRole('checkbox').nth(1)).toBeChecked();
  await settled(p);
  await p.waitForTimeout(300); // 等對話框的開啟動畫結束再截圖
  await p.screenshot({ path: test.info().outputPath('pick-receipts.png') });
  await dialog.getByRole('button', { name: /帶\s?入/ }).click();
  await expect(dialog).toBeHidden();
  await expect(textbox(p, '未稅金額')).toHaveValue('1000');
  await expect(textbox(p, '稅額')).toHaveValue('50');
  await expect(textbox(p, '總計')).toHaveValue('1050.0000');
  const links = p.getByRole('region', { name: '憑證進貨單' });
  await expect(links.locator('tbody tr.ant-table-row')).toHaveCount(1);
  await expect(select(links, '進貨單')).toContainText(RECEIPT_TITLE);
  await settled(p);
  await links.scrollIntoViewIfNeeded();
  await p.screenshot({ path: test.info().outputPath('purchase-invoice-draft.png') });

  await p.getByRole('button', { name: /儲\s?存/ }).click();
  await expect(p.getByLabel('單號')).toHaveValue('PI202611-0001');
  expect(created.purchase_invoice).toMatchObject({
    data: {
      doc_type: 'triplicate',
      invoice_no: 'cd12345678',
      supplier_id: SUPPLIER_ID,
      seller_tax_id: '87654321',
      subtotal: '1000',
      tax_amount: '50',
      deductible: true,
    },
    lines: { purchase_invoice_receipt: [{ data: { goods_receipt_id: RECEIPT_ID } }] },
  });

  // 登錄
  const action = (name: RegExp) => p.locator('.page-header-actions').getByRole('button', { name });
  await action(/登\s?錄/).click();
  await p
    .getByRole('dialog')
    .getByRole('button', { name: /登\s?錄/ })
    .click();
  await expect(p.locator('.page-header .ant-tag', { hasText: '已登錄' })).toBeVisible();
  await expect(p.getByText(/此單據已登錄，不能修改。登錄時間：/)).toBeVisible();
  await expect(p.getByRole('dialog')).toHaveCount(0);
  await settled(p);
  await p.screenshot({ path: test.info().outputPath('purchase-invoice-registered.png') });

  // 開立進貨折讓：帶入原憑證、廠商、稅別，顯示可折讓金額
  await action(/開立折讓/).click();
  await expect(p).toHaveURL(/purchase_allowance\/new\?invoice=/);
  await expect(select(p, '原憑證')).toContainText(INVOICE_TITLE);
  await expect(select(p, '廠商')).toContainText(SUPPLIER_TITLE);
  await expect(p.getByText('可折讓金額 1,050')).toBeVisible();
  await expect(p.getByText('憑證 CD12345678 總計 1,050，已折讓 0')).toBeVisible();
  await textbox(p, '折讓未稅金額').fill('100');
  await textbox(p, '折讓稅額').fill('5');
  await expect(textbox(p, '折讓總計')).toHaveValue('105.0000');
  await textbox(p, '折讓原因').fill('數量短少');
  await settled(p);
  await p.screenshot({ path: test.info().outputPath('purchase-allowance-draft.png') });
  await p.getByRole('button', { name: /儲\s?存/ }).click();
  await expect(p.getByLabel('單號')).toHaveValue('PA202611-0001');
  expect(created.purchase_allowance).toMatchObject({
    data: {
      purchase_invoice_id: PURCHASE_INVOICE_ID,
      subtotal: '100',
      tax_amount: '5',
      reason: '數量短少',
    },
  });
  await action(/開\s?立$/).click();
  await p
    .getByRole('dialog')
    .getByRole('button', { name: /開\s?立/ })
    .click();
  await expect(p.locator('.page-header .ant-tag', { hasText: '已開立' })).toBeVisible();
  await expect(p.getByRole('dialog')).toHaveCount(0);
  await settled(p);
  await p.screenshot({ path: test.info().outputPath('purchase-allowance-issued.png') });
});

test('customs certificate lists receipts separately and cannot be allowed', async ({ page: p }) => {
  let docClass: string | undefined;
  await mockTauri(p, {
    ...baseHandlers(),
    list_uninvoiced_receipts: ({ query }) => {
      docClass = (query as { doc_class: string }).doc_class;
      return page([]);
    },
    get_record: ({ key }) =>
      key === 'supplier'
        ? purchasingRecord({ id: SUPPLIER_ID, title: SUPPLIER_TITLE, data: { tax_id: null } })
        : purchasingRecord({
            id: PURCHASE_INVOICE_ID,
            record_no: 'PI202611-0002',
            title: 'AA123 美商五金',
            data: {
              doc_type: 'customs',
              invoice_no: 'AA123',
              invoice_date: '2026-11-12',
              tax_period: '202611',
              supplier_id: SUPPLIER_ID,
              tax_type: 'taxable',
              subtotal: '3215.0000',
              tax_amount: '161.0000',
              total: '3376.0000',
              deductible: true,
              status: 'posted',
            },
            relations: { supplier_id: rel(SUPPLIER_ID, SUPPLIER_TITLE) },
            document: { ...DOC, posted_at: '2026-11-12T03:00:00Z' },
            lines: { purchase_invoice_receipt: [] },
          }),
  });

  await p.goto('/#/entities/purchase_invoice/new');
  await select(p, '憑證種類').click();
  await option(p, '海關代徵營業稅繳納證').click();
  await select(p, '廠商').click();
  await option(p, SUPPLIER_TITLE).click();
  await expect(p.getByText('勾選尚未取得海關繳納證的進貨單')).toBeVisible();
  await p.getByRole('button', { name: /選擇進貨單/ }).click();
  const dialog = p.getByRole('dialog', { name: '選擇進貨單' });
  await expect(dialog).toContainText('這個廠商沒有尚未取得憑證的進貨單');
  expect(docClass).toBe('customs');
  await dialog.getByRole('button', { name: /取\s?消/ }).click();

  // 已登錄的海關繳納證沒有「開立折讓」
  await p.goto(`/#/entities/purchase_invoice/${PURCHASE_INVOICE_ID}`);
  await expect(p.locator('.page-header .ant-tag', { hasText: '已登錄' })).toBeVisible();
  await expect(
    p.locator('.page-header-actions').getByRole('button', { name: /作\s?廢/ }),
  ).toBeVisible();
  await expect(
    p.locator('.page-header-actions').getByRole('button', { name: /開立折讓/ }),
  ).toHaveCount(0);
});

test('goods receipt shows its purchase invoices', async ({ page: p }) => {
  await mockTauri(p, {
    ...baseHandlers(),
    get_record: () => RECEIPT,
    list_invoice_links: ({ query }) => {
      expect(query).toMatchObject({ entity_key: 'goods_receipt', record_id: RECEIPT_ID });
      return [
        {
          entity_key: 'purchase_invoice',
          record_id: PURCHASE_INVOICE_ID,
          record_no: 'PI202611-0001',
          invoice_no: 'CD12345678',
          invoice_date: '2026-11-19',
          status: 'posted',
        },
        {
          entity_key: 'purchase_invoice',
          record_id: '0196f5a0-0000-7000-8000-000000000e22',
          record_no: 'PI202611-0002',
          invoice_no: 'AA123',
          invoice_date: '2026-11-12',
          status: 'posted',
        },
      ];
    },
  });

  await p.goto(`/#/entities/goods_receipt/${RECEIPT_ID}`);
  await expect(p.getByText('進項憑證：')).toBeVisible();
  await expect(p.getByRole('link', { name: 'CD12345678（已登錄）' })).toBeVisible();
  await expect(p.getByRole('link', { name: 'AA123（已登錄）' })).toBeVisible();
  await settled(p);
  await p.screenshot({ path: test.info().outputPath('receipt-invoices.png') });
});
