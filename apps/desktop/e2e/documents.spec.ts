import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

import type { Record as DataRecord } from '../src/bindings';
import {
  ADJUSTMENT_ID,
  STOCK_ADJUSTMENT,
  STOCK_ADJUSTMENT_FIELDS,
  adjustment,
} from '../src/test/documentFixtures';
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

const actions = (page: Page) => page.locator('.ant-card-extra');

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
  await expect(page.locator('.ant-card-head .ant-tag', { hasText: '草稿' })).toBeVisible();
  await expect(actions(page).getByRole('button', { name: VOID_BUTTON })).toHaveCount(0);

  await actions(page).getByRole('button', { name: POST_BUTTON }).click();
  const postDialog = page.getByRole('dialog');
  await expect(postDialog).toContainText('確定要過帳這張盤點調整單？');
  await postDialog.getByRole('button', { name: POST_BUTTON }).click();

  // 已過帳：唯讀，沒有儲存，可以作廢
  await expect(page.locator('.ant-card-head .ant-tag', { hasText: '已過帳' })).toBeVisible();
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

  await expect(page.locator('.ant-card-head .ant-tag', { hasText: '已作廢' })).toBeVisible();
  await expect(actions(page).getByRole('button', { name: VOID_BUTTON })).toHaveCount(0);

  // 列表：單號、狀態標籤；已作廢的單據沒有刪除按鈕
  await page.getByRole('button', { name: /返回列表/ }).click();
  const row = page.getByRole('row', { name: /ADJ202611-0001/ });
  await expect(row.locator('.ant-tag-red', { hasText: '已作廢' })).toBeVisible();
  await expect(row.getByRole('button', { name: /刪\s?除/ })).toHaveCount(0);
  await expect(page.getByRole('combobox', { name: '狀態篩選' })).toBeVisible();
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
  await expect(page.locator('.ant-card-head .ant-tag', { hasText: '草稿' })).toBeVisible();
});

test('stock adjustment: viewer cannot post or void', async ({ page }) => {
  const state: { current: DataRecord | null } = { current: adjustment('posted') };
  await mockDocument(page, VIEWER, state);

  await page.goto(`/#/entities/stock_adjustment/${ADJUSTMENT_ID}`);
  await expect(page.locator('.ant-card-head .ant-tag', { hasText: '已過帳' })).toBeVisible();
  await expect(actions(page).getByRole('button', { name: VOID_BUTTON })).toHaveCount(0);
  await expect(actions(page).getByRole('button', { name: POST_BUTTON })).toHaveCount(0);
  await expect(page.getByLabel('備註')).toBeDisabled();
});
