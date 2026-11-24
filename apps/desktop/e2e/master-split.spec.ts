import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

import type { RecordListQuery_Deserialize as RecordListQuery } from '../src/bindings';
import {
  CUSTOMER,
  CUSTOMER_FIELDS,
  PRODUCT,
  PRODUCT_FIELDS,
  SUPPLIER,
  SUPPLIER_FIELDS,
  customerId,
  fakeListRecords,
  manyCustomers,
} from '../src/test/masterSplitFixtures';
import { mockTauri } from './tauri-mock';

const ADMIN = {
  id: '0196f5a0-0000-7000-8000-000000000001',
  username: 'admin',
  display_name: '王管理',
  role: 'admin',
  permissions: ['audit.read', 'record.post', 'record.read', 'record.write', 'schema.manage'],
  must_change_password: false,
};

const ENTITIES = [PRODUCT, CUSTOMER, SUPPLIER];
const FIELDS: Record<string, unknown> = {
  customer: CUSTOMER_FIELDS,
  supplier: SUPPLIER_FIELDS,
  product: PRODUCT_FIELDS,
};

/** 4 筆固定的客戶＋80 筆一般客戶（啟用中 83 筆），超過一頁（50 筆）。 */
const CUSTOMERS = manyCustomers(80);

async function setup(page: Page) {
  await mockTauri(page, {
    get_server_url: () => 'http://localhost:8080',
    health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
    setup_status: () => ({ needs_setup: false }),
    current_user: () => ADMIN,
    list_entities: () => ENTITIES,
    get_entity: ({ key }) => ({
      entity: ENTITIES.find((e) => e.key === key),
      fields: FIELDS[key as string] ?? [],
    }),
    get_default_layout: ({ key }) => ({
      source: 'generated',
      layout: null,
      config: {
        columns: (key === 'product' ? ['code', 'name'] : []).map((k) => ({
          field_key: k,
          width: null,
        })),
        sort: null,
        filters: [],
      },
    }),
    list_layouts: () => [],
    list_records: ({ key, query }) =>
      fakeListRecords(key === 'customer' ? CUSTOMERS : [], query as RecordListQuery),
  });
}

const listbox = (page: Page) => page.getByRole('listbox', { name: '客戶列表' });

test('customer split view: picker, search, active filter, scroll loading', async ({ page }) => {
  await setup(page);
  await page.goto('/#/entities/customer');

  const list = listbox(page);
  await expect(list.getByRole('option').first()).toContainText('C001');
  await expect(page.getByTestId('picker-total')).toHaveText('共 83 筆');
  await expect(page.getByText('請從左側選擇客戶')).toBeVisible();
  // 第一頁 50 筆
  await expect(list.getByRole('option')).toHaveCount(50);

  // 左側固定寬度，右側填滿
  const picker = await page.locator('.master-split-picker').boundingBox();
  const detail = await page.locator('.master-split-detail').boundingBox();
  expect(picker!.width).toBe(320);
  expect(detail!.x).toBeGreaterThan(picker!.x + picker!.width);
  expect(detail!.height).toBeGreaterThan(400);

  // 捲動到底載入下一頁
  await list.evaluate((el) => el.scrollTo({ top: el.scrollHeight }));
  await expect(list.getByRole('option')).toHaveCount(83);

  // 全部：包含停用（灰色）
  await page.getByText('全部', { exact: true }).click();
  await expect(page.getByTestId('picker-total')).toHaveText('共 84 筆');
  const inactive = list.getByRole('option', { name: /C003/ });
  await expect(inactive).toContainText('停用');
  await expect(inactive).toHaveAttribute('data-inactive', 'true');

  // 搜尋統編
  await page.getByRole('textbox', { name: '搜尋客戶' }).fill('8765');
  await expect(list.getByRole('option')).toHaveCount(1);
  await expect(list.getByRole('option')).toContainText('C002光明電子有限公司');
  await expect(page.getByTestId('picker-total')).toHaveText('共 1 筆');

  // 搜尋簡稱
  await page.getByRole('textbox', { name: '搜尋客戶' }).fill('阿昇');
  await expect(list.getByRole('option')).toHaveCount(1);
  await expect(list.getByRole('option')).toContainText('C003東昇貿易');
});

test('customer split view: arrow keys and selection survives reload', async ({ page }) => {
  await setup(page);
  await page.goto('/#/entities/customer');
  const list = listbox(page);

  await list.getByRole('option', { name: /C001/ }).click();
  await expect(list.getByRole('option', { name: /C001/ })).toHaveAttribute('aria-selected', 'true');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowDown');
  await expect(list.getByRole('option', { name: /C004/ })).toHaveAttribute('aria-selected', 'true');
  await expect(page).toHaveURL(new RegExp(`selected=${customerId(4)}`));
  await expect(page.getByTestId('detail-title')).toHaveText('C004 Global Trading Co.');

  await page.reload();
  await expect(listbox(page).getByRole('option', { name: /C004/ })).toHaveAttribute(
    'aria-selected',
    'true',
  );
  await expect(page.getByTestId('detail-title')).toHaveText('C004 Global Trading Co.');
});

test('supplier uses split view, product keeps the table', async ({ page }) => {
  await setup(page);
  await page.goto('/#/entities/supplier');
  await expect(page.getByRole('listbox', { name: '廠商列表' })).toBeVisible();
  await expect(page.getByText('沒有資料')).toBeVisible();

  await page.goto('/#/entities/product');
  await expect(page.getByRole('columnheader', { name: '料號' })).toBeVisible();
  await expect(page.getByRole('listbox')).toHaveCount(0);
});
