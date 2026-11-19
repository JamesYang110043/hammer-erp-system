import { expect, test } from '@playwright/test';
import type { Locator, Page } from '@playwright/test';

import type { Record as DataRecord } from '../src/bindings';
import { PRODUCT_ID, lot } from '../src/test/documentFixtures';
import { CURRENCIES, TAX_RATES, purchasingRecord } from '../src/test/purchasingFixtures';
import {
  CUSTOMER_ID,
  CUSTOMER_TITLE,
  SALES_ENTITIES,
  SALES_ORDER_ID,
  SHIPMENT_ID,
  salesEntityDetail,
} from '../src/test/salesFixtures';
import { openNavSection } from './nav';
import { CommandRejection, mockTauri } from './tauri-mock';
import type { E2eCommandHandler } from './tauri-mock';

/**
 * 銷貨模組（docs/architecture.md §8.9）：訂單（選商品帶入售價、金額預覽、確認）→
 * 出貨單從訂單帶入 → 建議批號、指定批號 → 存檔前的數量提示 → 過帳（庫存不足標示列）→ 實際扣用的批號 → 庫存查詢。
 * 伺服器以 mock 模擬，訂單的金額使用 §8.9 範例一的結果（與畫面預覽比對）。
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
const ORDER_TITLE = 'SO202611-0001 大同五金行';
const ORDER_LINE_1 = '0196f5a0-0000-7000-8000-000000000e01';
const ORDER_LINE_2 = '0196f5a0-0000-7000-8000-000000000e02';
const ADDRESS = '台北市大同區民生西路 100 號';

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

/** 商品的批號（先進先出順序）：A001 有 L-01（剩 4）、L-02（剩 10）；B002 只有 N-01（剩 2）。 */
const LOTS: Record<
  string,
  { id: string; lot_no: string; date: string; qty: number; cost: string }[]
> = {
  [PRODUCT_ID]: [
    { id: 'lot-1', lot_no: 'L-01', date: '2026-10-02', qty: 4, cost: '80.0000' },
    { id: 'lot-2', lot_no: 'L-02', date: '2026-11-01', qty: 10, cost: '82.5000' },
  ],
  [PRODUCT2_ID]: [{ id: 'lot-3', lot_no: 'N-01', date: '2026-11-05', qty: 2, cost: '20.0000' }],
};

/** 建議批號 API 的 mock：先扣指定批號的列，再依順序以先進先出扣其他列（與伺服器相同）。 */
function allocationPreview({ req }: Record<string, unknown>) {
  const { lines } = req as { lines: { product_id: string; qty: string; lot_no: string | null }[] };
  const left = new Map(Object.values(LOTS).flatMap((lots) => lots.map((l) => [l.id, l.qty])));
  const result = lines.map((line, index) => ({
    index,
    product_id: line.product_id,
    product_type: 'goods',
    qty: Number(line.qty).toFixed(4),
    allocations: [] as unknown[],
    shortage: '0.0000',
  }));
  const take = (i: number, lots: (typeof LOTS)[string]) => {
    let need = Number(lines[i]!.qty);
    for (const l of lots) {
      const n = Math.min(need, left.get(l.id)!);
      if (n > 0) {
        left.set(l.id, left.get(l.id)! - n);
        need -= n;
        result[i]!.allocations.push({
          lot_id: l.id,
          lot_no: l.lot_no,
          received_date: l.date,
          qty: n.toFixed(4),
          unit_cost_twd: l.cost,
        });
      }
    }
    result[i]!.shortage = need.toFixed(4);
  };
  lines.forEach((line, i) => {
    if (line.lot_no) {
      take(
        i,
        LOTS[line.product_id]!.filter((l) => l.lot_no.toLowerCase() === line.lot_no!.toLowerCase()),
      );
    }
  });
  lines.forEach((line, i) => {
    if (!line.lot_no) {
      take(i, LOTS[line.product_id]!);
    }
  });
  return { lines: result };
}

function productRecord(id: string) {
  return purchasingRecord({
    id,
    title: titleOf(id),
    data: {
      code: titleOf(id).split(' ')[0],
      default_sale_price: id === PRODUCT_ID ? '120.0000' : '33.3333',
      tax_type: 'taxable',
    },
  });
}

function customerRecord() {
  return purchasingRecord({
    id: CUSTOMER_ID,
    title: CUSTOMER_TITLE,
    data: { code: 'C001', name: '大同五金行', address: ADDRESS },
  });
}

function baseHandlers(): Record<string, E2eCommandHandler> {
  return {
    get_server_url: () => 'http://localhost:8080',
    health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
    setup_status: () => ({ needs_setup: false }),
    current_user: () => ADMIN,
    list_entities: () => SALES_ENTITIES,
    get_entity: ({ key }) => salesEntityDetail(key as string),
    list_currencies: () => CURRENCIES,
    list_tax_rates: () => TAX_RATES,
    lookup_records: ({ key }) =>
      key === 'customer'
        ? [{ id: CUSTOMER_ID, title: CUSTOMER_TITLE }]
        : key === 'sales_order'
          ? [{ id: SALES_ORDER_ID, title: ORDER_TITLE }]
          : [
              { id: PRODUCT_ID, title: PRODUCT_TITLE },
              { id: PRODUCT2_ID, title: PRODUCT2_TITLE },
            ],
    allocation_preview: allocationPreview,
  };
}

/** 明細列的記錄（伺服器回應）。 */
function lineRecord(id: string, data: DataRecord['data']) {
  const productId = data.product_id as string;
  return purchasingRecord({
    id,
    title: titleOf(productId),
    data,
    relations: { product_id: rel(productId, titleOf(productId)) },
  });
}

test('sales order: product fills sale price, preview matches server, confirm', async ({
  page: p,
}) => {
  let current: DataRecord | null = null;
  let created: unknown;

  await mockTauri(p, {
    ...baseHandlers(),
    get_record: ({ key, id }) =>
      key === 'customer'
        ? customerRecord()
        : key === 'product'
          ? productRecord(id as string)
          : current,
    create_record: ({ req }) => {
      created = req;
      const { data, lines } = req as {
        data: DataRecord['data'];
        lines: { sales_order_line: { data: DataRecord['data'] }[] };
      };
      // 伺服器的計算結果（§8.9 範例一）
      current = purchasingRecord({
        id: SALES_ORDER_ID,
        record_no: 'SO202611-0001',
        title: ORDER_TITLE,
        data: {
          ...data,
          status: 'draft',
          subtotal: '1120.0000',
          tax_amount: '56.0000',
          total: '1176.0000',
        },
        relations: { customer_id: rel(CUSTOMER_ID, CUSTOMER_TITLE) },
        lines: {
          sales_order_line: lines.sales_order_line.map((l, i) =>
            lineRecord(i === 0 ? ORDER_LINE_1 : ORDER_LINE_2, {
              ...l.data,
              amount: i === 0 ? '1020.0000' : '100.0000',
              shipped_qty: '0.0000',
            }),
          ),
        },
        document: DOC,
      });
      return current;
    },
    confirm_record: () => {
      current = {
        ...current!,
        version: 2,
        data: { ...current!.data, status: 'confirmed' },
        document: { ...DOC, posted_at: '2026-11-19T03:00:00Z' },
      };
      return current;
    },
  });

  // 從選單「銷貨」開啟訂單
  await p.goto('/');
  await openNavSection(p, '銷貨');
  await expect(p.getByText('銷貨', { exact: true })).toBeVisible();
  await expect(p.getByRole('menuitem', { name: /訂單/ })).toBeVisible();
  await expect(p.getByRole('menuitem', { name: /出貨單/ })).toBeVisible();
  await expect(p.getByRole('menuitem', { name: /銷貨退回/ })).toBeVisible();
  await p.goto('/#/entities/sales_order/new');

  await expect(p.getByRole('textbox', { name: '訂單日期', exact: true })).toHaveValue(
    /^\d{4}-\d{2}-\d{2}$/,
  );
  await expect(select(p, '稅別')).toContainText('應稅');
  await select(p, '客戶').click();
  await option(p, CUSTOMER_TITLE).click();

  // 明細：選商品帶入預設售價；金額即時預覽（§8.9 範例一）
  const lines = p.getByRole('region', { name: '訂單明細' });
  const add = lines.getByRole('button', { name: /新增訂單明細/ });
  await add.click();
  await add.click();
  const rows = lines.locator('tbody tr.ant-table-row');
  const cell = (i: number, name: string) => rows.nth(i).getByRole('textbox', { name, exact: true });
  await select(rows.nth(0), '商品').click();
  await option(p, PRODUCT_TITLE).click();
  await expect(cell(0, '單價')).toHaveValue('120.0000');
  await cell(0, '數量').fill('10');
  await cell(0, '折扣率').fill('0.15');
  await select(rows.nth(1), '商品').click();
  await option(p, PRODUCT2_TITLE).click();
  await expect(cell(1, '單價')).toHaveValue('33.3333');
  await cell(1, '數量').fill('3');
  await expect(cell(0, '金額')).toHaveValue('1020.0000');
  await expect(cell(1, '金額')).toHaveValue('100.0000');
  await expect(p.getByRole('textbox', { name: '未稅金額', exact: true })).toHaveValue('1120.0000');
  await expect(p.getByRole('textbox', { name: '稅額', exact: true })).toHaveValue('56.0000');
  await expect(p.getByRole('textbox', { name: '總計', exact: true })).toHaveValue('1176.0000');
  await settled(p);
  await p.screenshot({ path: test.info().outputPath('order-preview.png'), fullPage: true });

  await p.getByRole('button', { name: /儲\s?存/ }).click();
  await expect(p.getByLabel('單號')).toHaveValue('SO202611-0001');
  expect(created).toMatchObject({
    data: { customer_id: CUSTOMER_ID, tax_type: 'taxable', tax_included: false },
    lines: {
      sales_order_line: [
        {
          data: {
            product_id: PRODUCT_ID,
            qty: '10',
            unit_price: '120.0000',
            discount_rate: '0.15',
          },
        },
        { data: { product_id: PRODUCT2_ID, qty: '3', unit_price: '33.3333' } },
      ],
    },
  });
  await expect(p.getByRole('textbox', { name: '總計', exact: true })).toHaveValue('1176.0000');

  // 確認 → 已確認，可以結案或作廢
  await p
    .locator('.ant-card-extra')
    .getByRole('button', { name: /確\s?認/ })
    .click();
  await expect(p.getByRole('dialog')).toContainText('之後可以據以出貨');
  await p
    .getByRole('dialog')
    .getByRole('button', { name: /確\s?認/ })
    .click();
  await expect(p.locator('.ant-card-head .ant-tag', { hasText: '已確認' })).toBeVisible();
  await expect(p.locator('.ant-card-extra').getByRole('button', { name: /結\s?案/ })).toBeVisible();
  await expect(p.locator('.ant-card-extra').getByRole('button', { name: /作\s?廢/ })).toBeVisible();
  await expect(p.getByRole('dialog')).toHaveCount(0);
  await settled(p);
  await p.screenshot({ path: test.info().outputPath('order-confirmed.png'), fullPage: true });
});

test('shipment from order: suggested lots, pick lots, quantity checks, post, used lots, stock', async ({
  page: p,
}) => {
  let shipment: DataRecord | null = null;
  let created: unknown;
  let failPost = true;
  let lineSeq = 0;
  const lineId = () => `0196f5a0-0000-7000-8000-00000000f${String(++lineSeq).padStart(3, '0')}`;

  const order = purchasingRecord({
    id: SALES_ORDER_ID,
    record_no: 'SO202611-0001',
    title: ORDER_TITLE,
    data: {
      customer_id: CUSTOMER_ID,
      order_date: '2026-11-18',
      tax_type: 'taxable',
      tax_included: false,
      status: 'confirmed',
    },
    relations: { customer_id: rel(CUSTOMER_ID, CUSTOMER_TITLE) },
    lines: {
      sales_order_line: [
        lineRecord(ORDER_LINE_1, {
          product_id: PRODUCT_ID,
          qty: '10.0000',
          unit_price: '120.0000',
          discount_rate: '0.1500',
          amount: '1020.0000',
          shipped_qty: '0.0000',
        }),
        lineRecord(ORDER_LINE_2, {
          product_id: PRODUCT2_ID,
          qty: '3.0000',
          unit_price: '33.3333',
          discount_rate: '0.0000',
          amount: '100.0000',
          shipped_qty: '0.0000',
        }),
      ],
    },
  });

  /** 依送出的明細建立伺服器的出貨單（金額不重新計算，畫面上以預覽比對）。 */
  const save = (data: DataRecord['data'], lines: { id?: string; data: DataRecord['data'] }[]) => {
    const before = new Map((shipment?.lines?.shipment_line ?? []).map((l) => [l.id, l]));
    shipment = purchasingRecord({
      id: SHIPMENT_ID,
      record_no: 'SH202611-0001',
      title: 'SH202611-0001 大同五金行',
      version: (shipment?.version ?? 0) + 1,
      data: { ...(shipment?.data ?? {}), ...data, status: 'draft' },
      relations: {
        customer_id: rel(CUSTOMER_ID, CUSTOMER_TITLE),
        sales_order_id: rel(SALES_ORDER_ID, ORDER_TITLE),
      },
      lines: {
        shipment_line: lines.map((l) => {
          const prev = l.id ? before.get(l.id) : undefined;
          const data = { ...(prev?.data ?? {}), ...l.data };
          // 伺服器計算的金額（d = 0）
          const amount = Math.round(
            Number(data.qty) * Number(data.unit_price) * (1 - Number(data.discount_rate ?? 0)),
          ).toFixed(4);
          return lineRecord(l.id ?? lineId(), { ...data, amount, cost_amount_twd: null });
        }),
      },
      document: DOC,
    });
    return shipment;
  };

  await mockTauri(p, {
    ...baseHandlers(),
    get_record: ({ key, id }) =>
      key === 'customer'
        ? customerRecord()
        : key === 'product'
          ? productRecord(id as string)
          : key === 'sales_order'
            ? order
            : shipment,
    create_record: ({ req }) => {
      created = req;
      const { data, lines } = req as {
        data: DataRecord['data'];
        lines: { shipment_line: { data: DataRecord['data'] }[] };
      };
      return save(data, lines.shipment_line);
    },
    update_record: ({ req }) => {
      const { data, lines } = req as {
        data: DataRecord['data'];
        lines?: { shipment_line: { id?: string; data: DataRecord['data'] }[] };
      };
      return save(
        data,
        lines?.shipment_line ??
          shipment!.lines!.shipment_line!.map((l) => ({ id: l.id, data: {} })),
      );
    },
    post_record: () => {
      if (failPost) {
        throw new CommandRejection({
          kind: 'api',
          code: 'STOCK_INSUFFICIENT',
          message: '庫存不足',
          details: {
            items: [
              {
                product_id: PRODUCT2_ID,
                product_code: 'B002',
                product_name: '六角螺帽 M6',
                lot_id: null,
                lot_no: null,
                required: '3.0000',
                available: '2.0000',
                shortage: '1.0000',
                lines: [{ detail_key: 'shipment_line', index: 2 }],
              },
            ],
          },
        });
      }
      const lines = shipment!.lines!.shipment_line!;
      const costs = ['320.00', '495.00', '40.00'];
      shipment = {
        ...shipment!,
        version: shipment!.version + 1,
        data: { ...shipment!.data, status: 'posted', cost_total_twd: '855.00' },
        document: { ...DOC, posted_at: '2026-11-19T03:00:00Z' },
        lines: {
          shipment_line: lines.map((l, i) => ({
            ...l,
            data: { ...l.data, cost_amount_twd: costs[i] ?? null },
          })),
        },
      };
      return shipment;
    },
    list_product_lots: ({ productId }) =>
      page(
        (LOTS[productId as string] ?? []).map((l) =>
          lot({
            id: l.id,
            lot_no: l.lot_no,
            received_date: l.date,
            qty_in: `${l.qty}.0000`,
            qty_remaining: `${l.qty}.0000`,
            unit_cost_twd: l.cost,
          }),
        ),
      ),
    list_inventory_transactions: ({ query }) => {
      const q = query as { ref_doc_id?: string };
      if (q.ref_doc_id !== SHIPMENT_ID || !shipment) {
        return page([]);
      }
      const [l1, l2, l3] = shipment.lines!.shipment_line!.map((l) => l.id);
      const txn = (
        id: string,
        line: string,
        productId: string,
        lotNo: string,
        qty: string,
        cost: string,
      ) => ({
        id,
        txn_at: '2026-11-19T03:00:00Z',
        txn_date: '2026-11-19',
        txn_type: 'shipment',
        product_id: productId,
        product_code: titleOf(productId).split(' ')[0],
        product_name: titleOf(productId).split(' ').slice(1).join(' '),
        lot_id: lotNo,
        lot_no: lotNo,
        qty_change: qty,
        unit_cost_twd: cost,
        ref_doc_type: 'shipment',
        ref_doc_id: SHIPMENT_ID,
        ref_record_no: 'SH202611-0001',
        ref_line_id: line,
        created_by: null,
        created_by_name: null,
      });
      // 由新到舊
      return page([
        txn('t3', l3!, PRODUCT2_ID, 'N-01', '-2.0000', '20.0000'),
        txn('t2', l2!, PRODUCT_ID, 'L-02', '-6.0000', '82.5000'),
        txn('t1', l1!, PRODUCT_ID, 'L-01', '-4.0000', '80.0000'),
      ]);
    },
    list_stock_balances: () => page([]),
  });

  await p.goto('/#/entities/shipment/new');
  await expect(p.getByRole('textbox', { name: '出貨日期', exact: true })).toHaveValue(
    /^\d{4}-\d{2}-\d{2}$/,
  );

  // 選擇訂單：帶入客戶、稅別、送貨地址與未出貨明細
  await select(p, '訂單').click();
  await option(p, ORDER_TITLE).click();
  await expect(p.getByText('已帶入 SO202611-0001 的 2 筆未出貨明細')).toBeVisible();
  await expect(select(p, '客戶')).toContainText(CUSTOMER_TITLE);
  await expect(p.getByRole('textbox', { name: '送貨地址', exact: true })).toHaveValue(ADDRESS);

  const lines = p.getByRole('region', { name: '出貨明細' });
  const rows = lines.locator('tbody tr.ant-table-row');
  const cell = (i: number, name: string) => rows.nth(i).getByRole('textbox', { name, exact: true });
  await expect(rows).toHaveCount(2);
  await expect(cell(0, '數量')).toHaveValue('10.0000');
  await expect(cell(0, '單價')).toHaveValue('120.0000');
  await expect(cell(0, '批號')).toHaveAttribute('placeholder', '留空則先進先出');
  await expect(cell(0, '金額')).toHaveValue('1020.0000');
  await expect(p.getByRole('textbox', { name: '總計', exact: true })).toHaveValue('1176.0000');

  // 建議批號（先進先出）：第 1 列跨兩個批號，第 2 列不足
  await expect(p.getByTestId('allocation-0')).toContainText('先進先出');
  await expect(p.getByTestId('allocation-0')).toContainText('L-01 × 4、L-02 × 6');
  await expect(p.getByTestId('allocation-1')).toContainText('N-01 × 2');
  await expect(p.getByTestId('allocation-1')).toContainText('不足 1');
  await settled(p);
  await p.getByTestId('allocation-1').scrollIntoViewIfNeeded();
  await p.screenshot({ path: test.info().outputPath('shipment-suggested.png'), fullPage: true });

  // 指定批號：預先填入建議；合計與出貨數量不符時不能套用
  await p.getByTestId('allocation-0').getByRole('button', { name: '指定批號' }).click();
  const dialog = p.getByRole('dialog', { name: /指定批號/ });
  const pick = (lotNo: string) => dialog.getByRole('textbox', { name: `批號 ${lotNo} 出貨數量` });
  await expect(pick('L-01')).toHaveValue('4');
  await expect(pick('L-02')).toHaveValue('6');
  await pick('L-02').fill('5');
  await expect(dialog).toContainText('批號數量合計 9，與出貨數量 10 不符');
  await expect(dialog.getByRole('button', { name: /套\s?用/ })).toBeDisabled();
  await p.screenshot({ path: test.info().outputPath('shipment-pick-mismatch.png') });
  await pick('L-02').fill('6');
  await dialog.getByRole('button', { name: /套\s?用/ }).click();
  await expect(dialog).toHaveCount(0);

  // 拆成每個批號一列（同一列訂單明細）
  await expect(rows).toHaveCount(3);
  await expect(cell(0, '批號')).toHaveValue('L-01');
  await expect(cell(0, '數量')).toHaveValue('4');
  await expect(cell(1, '批號')).toHaveValue('L-02');
  await expect(cell(1, '數量')).toHaveValue('6');
  await expect(p.getByTestId('allocation-0')).toContainText('手動');

  // 存檔前的提示一：同一列訂單明細合計超過未出貨數量
  await cell(1, '數量').fill('7');
  await p.getByRole('button', { name: /儲\s?存/ }).click();
  await expect(rows.nth(0)).toContainText('超過未出貨數量 10');
  expect(created).toBeUndefined();

  // 存檔前的提示二：手動批號的數量超過批號的可用數量
  await cell(0, '數量').fill('5');
  await cell(1, '數量').fill('5');
  await expect(p.getByTestId('allocation-0')).toContainText('不足 1');
  await p.getByRole('button', { name: /儲\s?存/ }).click();
  const warn = p.getByRole('dialog', { name: '手動指定的批號數量不足' });
  await expect(warn).toContainText('第 1 列：批號 L-01 的可用數量不足，還差 1');
  await p.getByTestId('allocation-1').scrollIntoViewIfNeeded();
  await p.screenshot({
    path: test.info().outputPath('shipment-manual-shortage.png'),
    fullPage: true,
  });
  await warn.getByRole('button', { name: '返回修改' }).click();
  await expect(rows.nth(0)).toContainText('批號 L-01 的可用數量不足，還差 1');
  expect(created).toBeUndefined();

  await cell(0, '數量').fill('4');
  await cell(1, '數量').fill('6');
  await expect(p.getByTestId('allocation-0')).not.toContainText('不足');
  await p.getByRole('button', { name: /儲\s?存/ }).click();
  await expect(p.getByLabel('單號')).toHaveValue('SH202611-0001');
  expect(created).toMatchObject({
    data: {
      customer_id: CUSTOMER_ID,
      sales_order_id: SALES_ORDER_ID,
      tax_type: 'taxable',
      ship_address: ADDRESS,
    },
    lines: {
      shipment_line: [
        {
          data: {
            sales_order_line_id: ORDER_LINE_1,
            product_id: PRODUCT_ID,
            qty: '4',
            lot_no: 'L-01',
          },
        },
        {
          data: {
            sales_order_line_id: ORDER_LINE_1,
            product_id: PRODUCT_ID,
            qty: '6',
            lot_no: 'L-02',
          },
        },
        {
          data: {
            sales_order_line_id: ORDER_LINE_2,
            product_id: PRODUCT2_ID,
            qty: '3.0000',
            lot_no: null,
          },
        },
      ],
    },
  });

  // 過帳失敗（庫存不足）：列出不足的商品並標示是哪一列
  const postButton = p.locator('.ant-card-extra').getByRole('button', { name: /過\s?帳/ });
  await postButton.click();
  await p
    .getByRole('dialog')
    .getByRole('button', { name: /過\s?帳/ })
    .click();
  await expect(p.getByText('庫存不足，無法過帳')).toBeVisible();
  await expect(
    p.getByText('B002 六角螺帽 M6：需要 3，目前可用 2，不足 1（明細第 3 列）'),
  ).toBeVisible();
  await expect(rows.nth(2)).toContainText('庫存不足，還差 1');
  await expect(p.getByRole('dialog')).toHaveCount(0);
  await settled(p);
  await rows.nth(2).getByRole('textbox', { name: '數量', exact: true }).scrollIntoViewIfNeeded();
  await p.screenshot({
    path: test.info().outputPath('shipment-post-shortage.png'),
    fullPage: true,
  });

  // 改數量後存檔、過帳
  failPost = false;
  await cell(2, '數量').fill('2');
  await p.getByRole('button', { name: /儲\s?存/ }).click();
  await expect(p.getByText('已儲存')).toBeVisible();
  await postButton.click();
  await p
    .getByRole('dialog')
    .getByRole('button', { name: /過\s?帳/ })
    .click();
  await expect(p.locator('.ant-card-head .ant-tag', { hasText: '已過帳' })).toBeVisible();
  await expect(p.getByRole('textbox', { name: '銷貨成本', exact: true })).toHaveValue('855.00');

  // 實際扣用的批號與成本
  const used = p.getByRole('region', { name: '實際扣用的批號' });
  const usedRows = used.locator('tbody tr.ant-table-row');
  await expect(usedRows).toHaveCount(3);
  await expect(usedRows.nth(0)).toContainText('第 1 列');
  await expect(usedRows.nth(0)).toContainText('L-01');
  await expect(usedRows.nth(0)).toContainText('80.00');
  await expect(usedRows.nth(0)).toContainText('320.00');
  await expect(usedRows.nth(1)).toContainText('L-02');
  await expect(usedRows.nth(1)).toContainText('495.00');
  await expect(usedRows.nth(2)).toContainText('第 3 列');
  await expect(usedRows.nth(2)).toContainText('N-01');
  await expect(usedRows.nth(2)).toContainText('40.00');
  await expect(rows.nth(0).getByRole('link', { name: 'L-01' })).toBeVisible();
  await expect(p.getByRole('dialog')).toHaveCount(0);
  await settled(p);
  await used.scrollIntoViewIfNeeded();
  await p.screenshot({ path: test.info().outputPath('shipment-posted.png'), fullPage: true });

  // 批號連到庫存查詢
  await used.getByRole('link', { name: 'L-02' }).click();
  await expect(p.getByRole('table', { name: '批號' })).toContainText('L-02');
  await settled(p);
  await p.screenshot({ path: test.info().outputPath('shipment-stock.png') });
});
