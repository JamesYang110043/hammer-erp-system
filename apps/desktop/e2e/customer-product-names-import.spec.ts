import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

import type { RecordListQuery_Deserialize as RecordListQuery } from '../src/bindings';
import {
  CUSTOMER,
  CUSTOMER_CONTACT,
  CUSTOMER_CONTACT_FIELDS,
  CUSTOMER_FIELDS,
  CUSTOMER_PRODUCT_NAME,
  CUSTOMER_PRODUCT_NAME_FIELDS,
  PRODUCT,
  PRODUCT_FIELDS,
  PRODUCT_P002,
  PRODUCT_TITLES,
  customerDetail,
  customerId,
  fakeListRecords,
  manyCustomers,
  partyRecord,
} from '../src/test/masterSplitFixtures';
import { CommandRejection, mockTauri } from './tauri-mock';

/** Excel 匯入客戶品名（#284，docs/architecture.md §8.7「Excel 匯入客戶品名」）。 */

const ADMIN = {
  id: '0196f5a0-0000-7000-8000-000000000001',
  username: 'admin',
  display_name: '王管理',
  role: 'admin',
  permissions: ['audit.read', 'record.post', 'record.read', 'record.write', 'schema.manage'],
  must_change_password: false,
};
const VIEWER = { ...ADMIN, role: 'viewer', permissions: ['record.read'] };

const ENTITIES = [PRODUCT, CUSTOMER, CUSTOMER_CONTACT, CUSTOMER_PRODUCT_NAME];
const FIELDS: Record<string, unknown> = {
  customer: CUSTOMER_FIELDS,
  product: PRODUCT_FIELDS,
  customer_contact: CUSTOMER_CONTACT_FIELDS,
  customer_product_name: CUSTOMER_PRODUCT_NAME_FIELDS,
};
const LAYOUT = {
  id: null,
  version: null,
  is_default: true,
  config: {
    sections: [
      {
        key: 'basic',
        title: '基本資料',
        columns: 2,
        collapsible: false,
        collapsed: false,
        items: [
          { type: 'field', field: 'code', span: 1 },
          { type: 'field', field: 'name', span: 1 },
        ],
      },
      {
        key: '_blocks',
        title: '',
        columns: 1,
        collapsible: false,
        collapsed: false,
        items: [{ type: 'block', block: 'lines:customer_product_name' }],
      },
    ],
  },
};

/** 有錯誤的檔案：第 3 列料號不存在。 */
const BAD_FILE = {
  file_name: '客戶品名.xlsx',
  rows: [
    { row_no: 2, customer_code: 'C001', product_code: 'P001', name: 'X 型扳手', notes: null },
    { row_no: 3, customer_code: 'C001', product_code: 'P404', name: '不存在', notes: null },
  ],
};
/** 修正後的檔案。 */
const GOOD_FILE = {
  file_name: '客戶品名（修正）.xlsx',
  rows: [
    { row_no: 2, customer_code: 'C001', product_code: 'P001', name: 'X 型扳手', notes: null },
    {
      row_no: 3,
      customer_code: 'C001',
      product_code: 'P002',
      name: '工具組零件 03',
      notes: '備註',
    },
  ],
};

interface BatchRow {
  customer_code: string;
  product_code: string;
  name: string;
  notes?: string | null;
}

/** 批次寫入 API 的模擬：P404 不存在；P001 已有相同的品名（不變）；其他新增。 */
function batchResult(dryRun: boolean, rows: BatchRow[]) {
  const results = rows.map((r, index) => {
    const missing = r.product_code === 'P404';
    return {
      index,
      action: missing ? null : r.product_code === 'P001' ? 'unchanged' : 'create',
      customer_id: customerId(1),
      product_id: missing ? null : PRODUCT_P002,
      errors: missing
        ? [{ field: 'product_code', message: `找不到料號 ${r.product_code}`, duplicate_of: null }]
        : [],
    };
  });
  const count = (a: string | null) => results.filter((r) => r.action === a).length;
  return {
    dry_run: dryRun,
    summary: {
      create: count('create'),
      update: count('update'),
      unchanged: count('unchanged'),
      error: count(null),
    },
    rows: results,
  };
}

/** 等對話框、提示訊息的動畫與按鈕顏色的轉場結束（截圖才不會半透明）。 */
async function settled(page: Page) {
  await expect(
    page.locator('.ant-zoom-appear, .ant-zoom-enter, .ant-fade-appear, .ant-move-up-appear'),
  ).toHaveCount(0);
  // 按鈕停用 → 啟用的顏色轉場（antd motionDurationMid 0.2s）
  await page.waitForTimeout(300);
}

async function setup(
  page: Page,
  options: {
    user?: typeof ADMIN;
    pick?: () => unknown;
    onBatch?: (req: { dry_run: boolean; rows: BatchRow[] }) => void;
  } = {},
) {
  const customers = manyCustomers(3);
  let imported = false;
  await mockTauri(page, {
    get_server_url: () => 'http://localhost:8080',
    health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
    setup_status: () => ({ needs_setup: false }),
    current_user: () => options.user ?? ADMIN,
    list_entities: () => ENTITIES,
    list_all_entities: () => ENTITIES,
    get_entity: ({ key }) => ({
      entity: ENTITIES.find((e) => e.key === key),
      fields: FIELDS[key as string] ?? [],
    }),
    get_default_layout: ({ key }) => ({
      source: 'generated',
      layout: null,
      config: {
        columns: ((FIELDS[key as string] ?? []) as { field_key: string }[]).map((f) => ({
          field_key: f.field_key,
          width: null,
        })),
        sort: null,
        filters: [],
      },
    }),
    list_layouts: () => [],
    list_records: ({ key, query }) =>
      fakeListRecords(key === 'customer' ? customers : [], query as RecordListQuery),
    get_record: ({ id }) => {
      const record = customerDetail(customers, id as string);
      if (!record || !imported || id !== customerId(1)) {
        return record;
      }
      // 匯入後伺服器回傳新的客戶品名
      return {
        ...record,
        lines: {
          ...record.lines,
          customer_product_name: [
            ...record.lines!.customer_product_name!,
            {
              ...partyRecord('0196f5a0-0000-7000-8000-000000000c09', {
                product_id: PRODUCT_P002,
                name: '工具組零件 03',
                notes: '備註',
              }),
              title: 'P002 工具組零件 03',
              data: { product_id: PRODUCT_P002, name: '工具組零件 03', notes: '備註' },
              relations: {
                product_id: {
                  id: PRODUCT_P002,
                  title: PRODUCT_TITLES[PRODUCT_P002]!,
                  deleted: false,
                  inactive: false,
                },
              },
            },
          ],
        },
      };
    },
    get_form_layout: () => LAYOUT,
    list_currencies: () => [],
    pick_customer_product_names_xlsx: () => (options.pick ? options.pick() : GOOD_FILE),
    customer_product_names_batch: ({ req }) => {
      const r = req as { dry_run: boolean; rows: BatchRow[] };
      options.onBatch?.(r);
      if (!r.dry_run) {
        imported = true;
      }
      return batchResult(r.dry_run, r.rows);
    },
  });
}

test('import: preview with errors blocks import, fixed file imports', async ({ page }) => {
  const files = [BAD_FILE, GOOD_FILE];
  const calls: { dry_run: boolean; rows: BatchRow[] }[] = [];
  await setup(page, { pick: () => files.shift() ?? null, onBatch: (r) => calls.push(r) });
  await page.goto(`/#/entities/customer?selected=${customerId(1)}`);
  await expect(page.getByTestId('preview-title')).toHaveText('C001 大華實業股份有限公司');

  await page.getByRole('button', { name: /匯入客戶品名/ }).click();
  const dialog = page.getByRole('dialog', { name: /匯入客戶品名/ });
  await expect(dialog).toContainText('客戶品名.xlsx');
  await expect(dialog.getByLabel('匯入筆數')).toHaveText('新增 0、修改 0、不變 1、錯誤 1');
  const rows = dialog.locator('tbody tr.ant-table-row');
  await expect(rows).toHaveCount(2);
  await expect(rows.nth(0)).toContainText('2');
  await expect(rows.nth(0)).toContainText('不變');
  await expect(rows.nth(1)).toContainText('P404');
  await expect(rows.nth(1)).toContainText('找不到料號 P404');
  await expect(dialog).toContainText('請修正 Excel 後重新選擇檔案');
  await expect(dialog.getByRole('button', { name: /^匯\s?入$/ })).toBeDisabled();
  await settled(page);
  await page.screenshot({ path: test.info().outputPath('import-preview-errors.png') });

  // 重新選擇修正後的檔案：可以匯入
  await dialog.getByRole('button', { name: /重新選擇檔案/ }).click();
  await expect(dialog).toContainText('客戶品名（修正）.xlsx');
  await expect(dialog.getByLabel('匯入筆數')).toHaveText('新增 1、修改 0、不變 1、錯誤 0');
  await expect(rows.nth(1)).toContainText('新增');
  await expect(dialog.getByRole('button', { name: /^匯\s?入$/ })).toBeEnabled();
  await expect(dialog).not.toContainText('請修正 Excel');
  await settled(page);
  await page.screenshot({ path: test.info().outputPath('import-preview-ok.png') });
  await dialog.getByRole('button', { name: /^匯\s?入$/ }).click();
  await expect(page.getByText('已匯入客戶品名：新增 1 筆、修改 0 筆')).toBeVisible();
  await expect(dialog).toBeHidden();

  // 送出的內容：dry_run 兩次（每次選檔），之後寫入一次；沒有備註的列不帶 notes
  expect(calls.map((c) => c.dry_run)).toEqual([true, true, false]);
  expect(calls[2]!.rows).toEqual([
    { customer_code: 'C001', product_code: 'P001', name: 'X 型扳手' },
    { customer_code: 'C001', product_code: 'P002', name: '工具組零件 03', notes: '備註' },
  ]);

  // 匯入後客戶畫面看到新的客戶品名
  const names = page.getByTestId('record-preview').getByRole('region', { name: '客戶品名' });
  await expect(names.locator('tbody tr.ant-table-row')).toHaveCount(2);
  await expect(names).toContainText('工具組零件 03');
  await page.screenshot({ path: test.info().outputPath('import-done.png') });
});

test('import: file errors are shown and nothing is sent', async ({ page }) => {
  let batches = 0;
  await setup(page, {
    pick: () => {
      throw new CommandRejection({ kind: 'invalidInput', message: '缺少標題：客戶編號、料號' });
    },
    onBatch: () => {
      batches += 1;
    },
  });
  await page.goto('/#/entities/customer');
  await page.getByRole('button', { name: /匯入客戶品名/ }).click();
  const toast = page.getByText('缺少標題：客戶編號、料號');
  await expect(toast).toBeVisible();
  await settled(page);
  await page.screenshot({ path: test.info().outputPath('import-missing-headers.png') });
  await expect(page.getByRole('dialog', { name: /匯入客戶品名/ })).toHaveCount(0);
  expect(batches).toBe(0);
});

test('import: cancelling the file dialog does nothing', async ({ page }) => {
  let batches = 0;
  await setup(page, { pick: () => null, onBatch: () => (batches += 1) });
  await page.goto('/#/entities/customer');
  await page.getByRole('button', { name: /匯入客戶品名/ }).click();
  await expect(page.getByRole('button', { name: /匯入客戶品名/ })).toBeEnabled();
  await expect(page.getByRole('dialog', { name: /匯入客戶品名/ })).toHaveCount(0);
  expect(batches).toBe(0);
});

test('import button needs record.write', async ({ page }) => {
  await setup(page, { user: VIEWER });
  await page.goto('/#/entities/customer');
  await expect(
    page.getByTestId('record-preview').or(page.getByText('請從左側選擇客戶')),
  ).toBeVisible();
  await expect(page.getByRole('button', { name: /匯入客戶品名/ })).toHaveCount(0);
});
