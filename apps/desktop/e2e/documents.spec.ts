import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

import type { Record as DataRecord } from '../src/bindings';
import {
  ADJUSTMENT_ID,
  STOCK_ADJUSTMENT,
  STOCK_ADJUSTMENT_FIELDS,
  adjustment,
} from '../src/test/documentFixtures';
import { fieldDef } from '../src/test/schemaFixtures';
import { CommandRejection, mockTauri } from './tauri-mock';
import type { E2eCommandHandler } from './tauri-mock';

const ADMIN = {
  id: '0196f5a0-0000-7000-8000-000000000001',
  username: 'admin',
  display_name: '王管理',
  role: 'admin',
  permissions: ['record.post', 'record.read', 'record.write'],
  must_change_password: false,
};

const VIEWER = {
  ...ADMIN,
  id: '0196f5a0-0000-7000-8000-000000000002',
  username: 'viewer',
  display_name: '李唯讀',
  role: 'viewer',
  permissions: ['record.read'],
};

// antd 在兩個中文字的按鈕中間插入空白（「過 帳」）；有圖示時名稱前面會加上圖示名稱
const POST_BUTTON = /(^|\s)過\s?帳$/;
const VOID_BUTTON = /(^|\s)作\s?廢$/;
const SAVE_BUTTON = /儲\s?存/;

/** 單據畫面需要的 command；`current` 是伺服器上的單據（`null` 為還沒新增）。 */
async function mockDocument(
  page: Page,
  user: typeof ADMIN,
  state: { current: DataRecord | null },
  extra: Record<string, E2eCommandHandler> = {},
) {
  await mockTauri(page, {
    get_server_url: () => 'http://localhost:8080',
    health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
    setup_status: () => ({ needs_setup: false }),
    current_user: () => user,
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
    list_records: () => {
      const items = state.current ? [state.current] : [];
      return { items, total: items.length, page: 1, page_size: 50 };
    },
    get_record: () => state.current,
    create_record: ({ req }) => {
      const { data } = req as { data: DataRecord['data'] };
      state.current = adjustment('draft', { data: { ...data, status: 'draft' } });
      return state.current;
    },
    post_record: () => {
      state.current = adjustment('posted', { data: { ...state.current!.data, status: 'posted' } });
      return state.current;
    },
    void_record: () => {
      state.current = adjustment('void', { data: { ...state.current!.data, status: 'void' } });
      return state.current;
    },
    ...extra,
  });
}

const actions = (page: Page) => page.locator('.page-header-actions');

test('stock adjustment: create, post and void', async ({ page }) => {
  const state: { current: DataRecord | null } = { current: null };
  await mockDocument(page, ADMIN, state);

  await page.goto('/#/entities/stock_adjustment/new');
  await expect(page.getByLabel('單號')).toHaveAttribute('placeholder', '存檔後產生');
  // 調整日期預設今天（台北時間）
  await expect(page.getByLabel('調整日期')).toHaveValue(/^\d{4}-\d{2}-\d{2}$/);
  await page.locator('.ant-select', { has: page.getByRole('combobox', { name: '類型' }) }).click();
  await page.getByTitle('期初庫存').click();
  await page.getByLabel('備註').fill('期初庫存');
  await page.getByRole('button', { name: SAVE_BUTTON }).click();

  // 存檔後：草稿，有單號，可以過帳
  await expect(page.getByLabel('單號')).toHaveValue('ADJ202611-0001');
  await expect(page.locator('.page-header .ant-tag', { hasText: '草稿' })).toBeVisible();
  await expect(actions(page).getByRole('button', { name: VOID_BUTTON })).toHaveCount(0);

  await actions(page).getByRole('button', { name: POST_BUTTON }).click();
  const postDialog = page.getByRole('dialog');
  await expect(postDialog).toContainText('確定要過帳這張盤點調整單？');
  await postDialog.getByRole('button', { name: POST_BUTTON }).click();

  // 已過帳：唯讀，沒有儲存，可以作廢
  await expect(page.locator('.page-header .ant-tag', { hasText: '已過帳' })).toBeVisible();
  await expect(page.getByText('此單據已過帳，不能修改。')).toContainText(
    '過帳時間：2026-11-18 10:00:00',
  );
  await expect(page.getByLabel('備註')).toBeDisabled();
  await expect(page.getByRole('button', { name: SAVE_BUTTON })).toHaveCount(0);
  await expect(actions(page).getByRole('button', { name: POST_BUTTON })).toHaveCount(0);

  await actions(page).getByRole('button', { name: VOID_BUTTON }).click();
  const voidDialog = page.getByRole('dialog');
  await expect(voidDialog).toContainText('作廢後不能再恢復');
  await voidDialog.getByRole('button', { name: VOID_BUTTON }).click();

  await expect(page.locator('.page-header .ant-tag', { hasText: '已作廢' })).toBeVisible();
  await expect(actions(page).getByRole('button', { name: VOID_BUTTON })).toHaveCount(0);

  // 列表：單號、狀態標籤；已作廢的單據沒有刪除按鈕
  await page.getByRole('button', { name: /返回列表/ }).click();
  const row = page.getByRole('row', { name: /ADJ202611-0001/ });
  await expect(row.locator('.ant-tag[data-tone="danger"]', { hasText: '已作廢' })).toBeVisible();
  await expect(row.getByRole('button', { name: /刪\s?除/ })).toHaveCount(0);
  // 盤點調整單的狀態篩選只有草稿、已過帳、已作廢（採購單的狀態不列出）
  await page
    .locator('.ant-select', { has: page.getByRole('combobox', { name: '狀態篩選' }) })
    .click();
  const options = page.locator('.ant-select-dropdown:visible .ant-select-item-option');
  await expect(options).toHaveText(['全部狀態', '草稿', '已過帳', '已作廢']);
});

test('stock adjustment: stock insufficient error is explained', async ({ page }) => {
  const state: { current: DataRecord | null } = { current: adjustment('draft') };
  await mockDocument(page, ADMIN, state, {
    post_record: () => {
      throw new CommandRejection({
        kind: 'api',
        code: 'STOCK_INSUFFICIENT',
        message: '庫存不足',
        details: {
          items: [
            {
              product_id: '0196f5a0-0000-7000-8000-000000000601',
              product_code: 'A001',
              product_name: '不鏽鋼螺絲 M6',
              lot_id: '0196f5a0-0000-7000-8000-000000000701',
              lot_no: 'L-20261101',
              required: '1200.0000',
              available: '350.5000',
              shortage: '849.5000',
              lines: [{ detail_key: 'stock_adjustment_line', index: 0 }],
            },
          ],
        },
      });
    },
  });

  await page.goto(`/#/entities/stock_adjustment/${ADJUSTMENT_ID}`);
  await actions(page).getByRole('button', { name: POST_BUTTON }).click();
  await page.getByRole('dialog').getByRole('button', { name: POST_BUTTON }).click();

  await expect(page.getByText('庫存不足，無法過帳')).toBeVisible();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(
    page.getByText(
      'A001 不鏽鋼螺絲 M6（批號 L-20261101）：需要 1,200，目前可用 350.5，不足 849.5（明細第 1 列）',
    ),
  ).toBeVisible();
  await expect(page.locator('.page-header .ant-tag', { hasText: '草稿' })).toBeVisible();
});

test('stock adjustment: viewer cannot post or void', async ({ page }) => {
  const state: { current: DataRecord | null } = { current: adjustment('posted') };
  await mockDocument(page, VIEWER, state);

  await page.goto(`/#/entities/stock_adjustment/${ADJUSTMENT_ID}`);
  await expect(page.locator('.page-header .ant-tag', { hasText: '已過帳' })).toBeVisible();
  await expect(actions(page).getByRole('button', { name: VOID_BUTTON })).toHaveCount(0);
  await expect(actions(page).getByRole('button', { name: POST_BUTTON })).toHaveCount(0);
  await expect(page.getByLabel('備註')).toBeDisabled();
});

// ---------------------------------------------------------------------------
// 訂單（§8.9「訂單的狀態」）：部分出貨只能結案；狀態篩選列出訂單的狀態
// ---------------------------------------------------------------------------

const SALES_ORDER = {
  ...STOCK_ADJUSTMENT,
  id: '01a1f9d2-7b30-7000-94a2-592b16d9d49a',
  key: 'sales_order',
  label: '訂單',
  numbering_pattern: 'SO{yyyyMM}-{0000}',
  sort_order: 200,
};
const salesField = { entity_id: SALES_ORDER.id, is_system: true, storage: 'column' } as const;
const SALES_ORDER_FIELDS = [
  fieldDef({
    ...salesField,
    field_key: 'order_date',
    label: '訂單日期',
    data_type: 'date',
    is_required: true,
    sort_order: 20,
  }),
  fieldDef({
    ...salesField,
    field_key: 'status',
    label: '狀態',
    data_type: 'select',
    is_readonly: true,
    options: [
      { value: 'draft', label: '草稿', color: null },
      { value: 'confirmed', label: '已確認', color: null },
      { value: 'partially_shipped', label: '部分出貨', color: null },
      { value: 'closed', label: '已結案', color: null },
      { value: 'void', label: '已作廢', color: null },
    ],
    sort_order: 60,
  }),
  fieldDef({
    ...salesField,
    field_key: 'notes',
    label: '備註',
    data_type: 'long_text',
    sort_order: 100,
  }),
];
const CLOSE_BUTTON = /(^|\s)結\s?案$/;

function salesOrder(status: string, closed = false): DataRecord {
  return {
    id: ADJUSTMENT_ID,
    title: 'SO202611-0001 大華五金',
    record_no: 'SO202611-0001',
    data: { order_date: '2026-11-18', status, notes: '第一批已出貨' },
    document: {
      posted_at: '2026-11-18T02:00:00Z',
      posted_by: null,
      voided_at: null,
      voided_by: null,
      closed_at: closed ? '2026-11-18T05:00:00Z' : null,
      closed_by: null,
    },
    created_at: '2026-11-18T01:00:00Z',
    created_by: null,
    updated_at: '2026-11-18T01:00:00Z',
    updated_by: null,
    version: closed ? 4 : 3,
  };
}

test('sales order: partially shipped can only be closed', async ({ page }) => {
  let current = salesOrder('partially_shipped');
  await mockTauri(page, {
    get_server_url: () => 'http://localhost:8080',
    health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
    setup_status: () => ({ needs_setup: false }),
    current_user: () => ADMIN,
    list_entities: () => [SALES_ORDER],
    get_entity: () => ({ entity: SALES_ORDER, fields: SALES_ORDER_FIELDS }),
    get_default_layout: () => ({
      source: 'generated',
      layout: null,
      config: {
        columns: ['order_date', 'status', 'notes'].map((k) => ({ field_key: k, width: null })),
        sort: null,
        filters: [],
      },
    }),
    list_layouts: () => [],
    list_records: () => ({ items: [current], total: 1, page: 1, page_size: 50 }),
    get_record: () => current,
    close_record: () => {
      current = salesOrder('closed', true);
      return current;
    },
  });

  // 列表：部分出貨的標籤；狀態篩選是訂單的狀態
  await page.goto('/#/entities/sales_order');
  const row = page.getByRole('row', { name: /SO202611-0001/ });
  await expect(row.locator('.ant-tag[data-tone="warning"]', { hasText: '部分出貨' })).toBeVisible();
  await page
    .locator('.ant-select', { has: page.getByRole('combobox', { name: '狀態篩選' }) })
    .click();
  const options = page.locator('.ant-select-dropdown:visible .ant-select-item-option');
  await expect(options).toHaveText(['全部狀態', '草稿', '已確認', '部分出貨', '已結案', '已作廢']);
  await page.keyboard.press('Escape');

  // 單據：部分出貨不可作廢、不可過帳，只能結案
  await page.goto(`/#/entities/sales_order/${ADJUSTMENT_ID}`);
  await expect(page.locator('.page-header .ant-tag', { hasText: '部分出貨' })).toBeVisible();
  await expect(page.getByText('此單據部分出貨，不能修改。')).toContainText(
    '確認時間：2026-11-18 10:00:00',
  );
  await expect(actions(page).getByRole('button', { name: VOID_BUTTON })).toHaveCount(0);
  await expect(actions(page).getByRole('button', { name: POST_BUTTON })).toHaveCount(0);

  await actions(page).getByRole('button', { name: CLOSE_BUTTON }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toContainText('結案後不能再依這張訂單出貨');
  await dialog.getByRole('button', { name: CLOSE_BUTTON }).click();

  await expect(page.locator('.page-header .ant-tag', { hasText: '已結案' })).toBeVisible();
  await expect(page.getByText('此單據已結案，不能修改。')).toContainText(
    '結案時間：2026-11-18 13:00:00',
  );
  await expect(actions(page).getByRole('button', { name: CLOSE_BUTTON })).toHaveCount(0);
  await expect(page.getByRole('dialog')).toHaveCount(0);
});

test('status tags look the same in the list and in the form', async ({ page }) => {
  const state: { current: DataRecord | null } = { current: null };
  await mockDocument(page, ADMIN, state);
  for (const [status, label] of [
    ['draft', '草稿'],
    ['posted', '已過帳'],
    ['void', '已作廢'],
  ] as const) {
    state.current = adjustment(status);
    const colors = (tag: ReturnType<Page['locator']>) =>
      tag.evaluate((el) => {
        const s = getComputedStyle(el);
        return [s.color, s.backgroundColor, s.borderTopColor];
      });

    // 重新載入，不沿用上一輪的查詢快取
    await page.goto('/#/entities/stock_adjustment');
    await page.reload();
    const inList = page.getByRole('row').locator('.ant-tag', { hasText: label });
    await expect(inList).toBeVisible();
    const listColors = await colors(inList);

    await page.goto(`/#/entities/stock_adjustment/${ADJUSTMENT_ID}`);
    const inForm = page.locator('.page-header .ant-tag', { hasText: label });
    await expect(inForm).toBeVisible();
    expect(await colors(inForm)).toEqual(listColors);
  }
});
