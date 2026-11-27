import { expect, test } from '@playwright/test';
import type { Locator, Page } from '@playwright/test';

import type { RecordListQuery_Deserialize as RecordListQuery } from '../src/bindings';
import {
  CUSTOMER,
  CUSTOMER_CONTACT,
  CUSTOMER_CONTACT_FIELDS,
  CUSTOMER_FIELDS,
  CUSTOMER_PRODUCT_NAME,
  CUSTOMER_PRODUCT_NAME_A,
  CUSTOMER_PRODUCT_NAME_FIELDS,
  PRODUCT,
  PRODUCT_FIELDS,
  PRODUCT_P001,
  PRODUCT_P002,
  PRODUCT_TITLES,
  customerDetail,
  customerId,
  fakeListRecords,
  manyCustomers,
  partyRecord,
} from '../src/test/masterSplitFixtures';
import { CommandRejection, mockTauri } from './tauri-mock';

/** 客戶畫面維護客戶品名（#267，docs/architecture.md §8.7 `customer_product_names`）。 */

const ADMIN = {
  id: '0196f5a0-0000-7000-8000-000000000001',
  username: 'admin',
  display_name: '王管理',
  role: 'admin',
  permissions: ['audit.read', 'record.post', 'record.read', 'record.write', 'schema.manage'],
  must_change_password: false,
};

const ENTITIES = [PRODUCT, CUSTOMER, CUSTOMER_CONTACT, CUSTOMER_PRODUCT_NAME];
const FIELDS: Record<string, unknown> = {
  customer: CUSTOMER_FIELDS,
  product: PRODUCT_FIELDS,
  customer_contact: CUSTOMER_CONTACT_FIELDS,
  customer_product_name: CUSTOMER_PRODUCT_NAME_FIELDS,
};

/** 沒有設定版面：客戶的分組預設版面，明細表依序為聯絡人 → 客戶品名。 */
const DEFAULT_LAYOUT = {
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
          { type: 'field', field: 'short_name', span: 1 },
          { type: 'field', field: 'tax_id', span: 1 },
          { type: 'field', field: 'phone', span: 1 },
          { type: 'field', field: 'is_active', span: 1 },
        ],
      },
      {
        key: '_blocks',
        title: '',
        columns: 1,
        collapsible: false,
        collapsed: false,
        items: [
          { type: 'block', block: 'lines:customer_contact' },
          { type: 'block', block: 'lines:customer_product_name' },
        ],
      },
    ],
  },
};

interface Saved {
  version: number;
  data: Record<string, unknown>;
  lines?: Record<string, { id?: string; data: Record<string, unknown> }[]>;
}

/** 客戶品名的商品已停用（`relations.product_id.inactive`，§7.3）。 */
function withInactiveProduct(record: ReturnType<typeof customerDetail>) {
  if (!record?.lines) {
    return record;
  }
  return {
    ...record,
    lines: {
      ...record.lines,
      customer_product_name: record.lines.customer_product_name!.map((l) => ({
        ...l,
        relations: { product_id: { ...l.relations!.product_id!, inactive: true } },
      })),
    },
  };
}

/** 商品的客戶品名 API 的回應（#282）。 */
interface CustomerNameRow {
  id: string;
  customer_id: string;
  customer_title: string;
  customer_is_active: boolean;
  name: string;
  notes: string | null;
}

/** 商品表單的版面：料號、品名。 */
const PRODUCT_LAYOUT = {
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
    ],
  },
};

async function setup(
  page: Page,
  onSave: (req: Saved) => void,
  inactiveProduct = false,
  productNames: () => CustomerNameRow[] = () => [],
) {
  const customers = manyCustomers(3);
  const detail = (id: string) => {
    const record = customerDetail(customers, id);
    return inactiveProduct ? withInactiveProduct(record) : record;
  };
  await mockTauri(page, {
    get_server_url: () => 'http://localhost:8080',
    health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
    setup_status: () => ({ needs_setup: false }),
    current_user: () => ADMIN,
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
    get_record: ({ key, id }) =>
      key === 'product'
        ? partyRecord(id as string, {
            code: PRODUCT_TITLES[id as string]!.split(' ')[0],
            name: PRODUCT_TITLES[id as string]!.split(' ').slice(1).join(' '),
          })
        : detail(id as string),
    get_form_layout: ({ key }) => (key === 'product' ? PRODUCT_LAYOUT : DEFAULT_LAYOUT),
    product_customer_names: () => productNames(),
    lookup_records: ({ key }) =>
      key === 'product' ? Object.entries(PRODUCT_TITLES).map(([id, title]) => ({ id, title })) : [],
    update_record: ({ id, req }) => {
      onSave(req as Saved);
      return detail(id as string);
    },
    list_currencies: () => [],
  });
}

function select(scope: Locator, name: string) {
  return scope.locator('.ant-select', {
    has: scope.page().getByRole('combobox', { name, exact: true }),
  });
}

function option(p: Page, title: string) {
  return p
    .locator('.ant-select-dropdown:not(.ant-select-dropdown-hidden)')
    .last()
    .locator('.ant-select-item-option', { hasText: title });
}

test('customer preview shows customer product names after contacts', async ({ page }) => {
  await setup(page, () => undefined);
  await page.goto(`/#/entities/customer?selected=${customerId(1)}`);

  const preview = page.getByTestId('record-preview');
  await expect(preview.getByTestId('preview-title')).toHaveText('C001 大華實業股份有限公司');
  const contacts = preview.getByRole('region', { name: '客戶聯絡人' });
  const names = preview.getByRole('region', { name: '客戶品名' });
  await expect(names.locator('thead')).toContainText('商品');
  await expect(names.locator('thead')).toContainText('客戶品名');
  await expect(names.locator('thead')).toContainText('備註');
  const rows = names.locator('tbody tr.ant-table-row');
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toContainText('P001 扳手 10mm');
  await expect(rows.first()).toContainText('X 型扳手');
  await expect(rows.first()).toContainText('客戶圖面編號 W-10');

  // 聯絡人在上、客戶品名在下
  const contactsBox = await contacts.boundingBox();
  const namesBox = await names.boundingBox();
  expect(namesBox!.y).toBeGreaterThan(contactsBox!.y + contactsBox!.height - 1);
});

test('customer edit: add, change and remove customer product names', async ({ page }) => {
  let saved: Saved | undefined;
  await setup(page, (req) => {
    saved = req;
  });
  await page.goto(`/#/entities/customer?selected=${customerId(1)}&mode=edit`);

  const panel = page.getByTestId('record-form-panel');
  const names = panel.getByRole('region', { name: '客戶品名' });
  const rows = names.locator('tbody tr.ant-table-row');
  await expect(rows).toHaveCount(1);
  await expect(select(rows.nth(0), '商品')).toContainText('P001 扳手 10mm');
  await expect(rows.nth(0).getByRole('textbox', { name: '客戶品名' })).toHaveValue('X 型扳手');

  // 修改第一列、新增第二列（選商品、輸入客戶品名）
  await rows.nth(0).getByRole('textbox', { name: '客戶品名' }).fill('X 型扳手（新）');
  await names.getByRole('button', { name: /新增客戶品名/ }).click();
  await expect(rows).toHaveCount(2);
  await select(rows.nth(1), '商品').click();
  await option(page, 'P002 鉗子').click();
  await rows.nth(1).getByRole('textbox', { name: '客戶品名' }).fill('工具組零件 03');
  await page.screenshot({ path: test.info().outputPath('customer-product-names-edit.png') });

  await panel.getByRole('button', { name: /儲\s*存/ }).click();
  await expect.poll(() => saved).toBeDefined();
  expect(saved!.lines?.customer_product_name).toEqual([
    // 既有的列只送出有變動的欄位
    { id: CUSTOMER_PRODUCT_NAME_A, data: { name: 'X 型扳手（新）' } },
    { data: expect.objectContaining({ product_id: PRODUCT_P002, name: '工具組零件 03' }) },
  ]);

  // 刪除第一列：送出的明細不含它（伺服器軟刪除）
  saved = undefined;
  await page.goto(`/#/entities/customer?selected=${customerId(1)}&mode=edit`);
  await expect(rows).toHaveCount(1);
  await rows.nth(0).getByRole('button', { name: /刪除/ }).click();
  await expect(rows).toHaveCount(0);
  await panel.getByRole('button', { name: /儲\s*存/ }).click();
  await expect.poll(() => saved).toBeDefined();
  expect(saved!.lines?.customer_product_name).toEqual([]);
});

test('duplicate product error is shown on the row', async ({ page }) => {
  await setup(page, () => {
    throw new CommandRejection({
      kind: 'api',
      code: 'VALIDATION',
      message: '資料驗證失敗',
      details: {
        fields: [],
        lines: [
          {
            detail_key: 'customer_product_name',
            index: 1,
            field_key: 'product_id',
            message: '此商品已設定客戶品名',
          },
        ],
      },
    });
  });
  await page.goto(`/#/entities/customer?selected=${customerId(1)}&mode=edit`);

  const panel = page.getByTestId('record-form-panel');
  const names = panel.getByRole('region', { name: '客戶品名' });
  const rows = names.locator('tbody tr.ant-table-row');
  await expect(rows).toHaveCount(1);
  await names.getByRole('button', { name: /新增客戶品名/ }).click();
  await select(rows.nth(1), '商品').click();
  await option(page, 'P001 扳手 10mm').click();
  await rows.nth(1).getByRole('textbox', { name: '客戶品名' }).fill('Y 型扳手');
  await panel.getByRole('button', { name: /儲\s*存/ }).click();

  // 錯誤標示在第 2 列的商品欄（等出現動畫結束）
  await expect(rows.nth(1).getByText('此商品已設定客戶品名')).toBeVisible();
  await expect
    .poll(() =>
      rows
        .nth(1)
        .locator('.ant-form-item-explain-error')
        .evaluate((el) => getComputedStyle(el).opacity),
    )
    .toBe('1');
  await expect(rows.nth(0)).not.toContainText('此商品已設定客戶品名');
  await page.screenshot({ path: test.info().outputPath('customer-product-names-error.png') });
});

test('inactive product is marked in preview and edit, and saving still works', async ({ page }) => {
  let saved: Saved | undefined;
  await setup(
    page,
    (req) => {
      saved = req;
    },
    true,
  );
  await page.goto(`/#/entities/customer?selected=${customerId(1)}`);

  // 預覽：商品欄顯示「已停用」標籤
  const preview = page.getByTestId('record-preview');
  const names = preview.getByRole('region', { name: '客戶品名' });
  const row = names.locator('tbody tr.ant-table-row').first();
  await expect(row).toContainText('P001 扳手 10mm');
  await expect(row.getByText('已停用', { exact: true })).toBeVisible();
  await names.screenshot({ path: test.info().outputPath('customer-product-names-inactive.png') });

  // 編輯：選擇器目前的值也顯示「已停用」；存檔照常送出
  await page.goto(`/#/entities/customer?selected=${customerId(1)}&mode=edit`);
  const panel = page.getByTestId('record-form-panel');
  const editRows = panel
    .getByRole('region', { name: '客戶品名' })
    .locator('tbody tr.ant-table-row');
  const product = select(editRows.nth(0), '商品');
  await expect(product).toContainText('P001 扳手 10mm');
  await expect(product.getByText('已停用', { exact: true })).toBeVisible();
  await panel
    .getByRole('region', { name: '客戶品名' })
    .screenshot({ path: test.info().outputPath('customer-product-names-inactive-edit.png') });
  await editRows.nth(0).getByRole('textbox', { name: '客戶品名' }).fill('X 型扳手（停用後）');
  await panel.getByRole('button', { name: /儲\s*存/ }).click();
  await expect.poll(() => saved).toBeDefined();
  expect(saved!.lines?.customer_product_name).toEqual([
    { id: CUSTOMER_PRODUCT_NAME_A, data: { name: 'X 型扳手（停用後）' } },
  ]);
});

test('product form lists customer product names and opens the customer', async ({ page }) => {
  let rows: CustomerNameRow[] = [
    {
      id: CUSTOMER_PRODUCT_NAME_A,
      customer_id: customerId(1),
      customer_title: 'C001 大華實業股份有限公司',
      customer_is_active: true,
      name: 'X 型扳手',
      notes: '客戶圖面編號 W-10',
    },
    {
      id: '0196f5a0-0000-7000-8000-000000000c02',
      customer_id: customerId(3),
      customer_title: 'C003 東昇貿易',
      customer_is_active: false,
      name: '東昇的扳手',
      notes: null,
    },
  ];
  await setup(
    page,
    () => {
      // 客戶存檔後伺服器回傳新的客戶品名
      rows = [{ ...rows[0]!, name: 'X 型扳手（新）' }, rows[1]!];
    },
    false,
    () => rows,
  );
  await page.goto(`/#/entities/product/${PRODUCT_P001}`);

  // 表單最後的唯讀區塊：客戶、客戶品名、備註
  const block = page.getByRole('region', { name: '客戶品名' });
  await expect(block.locator('thead')).toContainText('客戶');
  await expect(block.locator('thead')).toContainText('備註');
  const tableRows = block.locator('tbody tr.ant-table-row');
  await expect(tableRows).toHaveCount(2);
  await expect(tableRows.nth(0)).toContainText('C001 大華實業股份有限公司');
  await expect(tableRows.nth(0)).toContainText('X 型扳手');
  await expect(tableRows.nth(0)).toContainText('客戶圖面編號 W-10');
  await expect(tableRows.nth(1)).toContainText('C003 東昇貿易');
  await expect(tableRows.nth(1).getByText('停用', { exact: true })).toBeVisible();
  // 區塊在表單欄位之後、不能編輯
  const codeBox = await page.getByRole('textbox', { name: '料號' }).boundingBox();
  const blockBox = await block.boundingBox();
  expect(blockBox!.y).toBeGreaterThan(codeBox!.y);
  await expect(block.getByRole('textbox')).toHaveCount(0);
  await block.scrollIntoViewIfNeeded();
  await page.screenshot({ path: test.info().outputPath('product-customer-names.png') });

  // 點客戶 → 客戶畫面選取該客戶
  await block.getByRole('link', { name: 'C001 大華實業股份有限公司' }).click();
  await expect(page).toHaveURL(new RegExp(`selected=${customerId(1)}`));
  await expect(page.getByTestId('preview-title')).toHaveText('C001 大華實業股份有限公司');

  // 在客戶畫面修改客戶品名後回到商品：看到新資料
  await page.getByRole('button', { name: /編\s*輯/ }).click();
  const panel = page.getByTestId('record-form-panel');
  await panel
    .getByRole('region', { name: '客戶品名' })
    .getByRole('textbox', { name: '客戶品名' })
    .fill('X 型扳手（新）');
  await panel.getByRole('button', { name: /儲\s*存/ }).click();
  await expect(page.getByTestId('record-preview')).toBeVisible();
  await page.goBack();
  await expect(page).toHaveURL(new RegExp(`entities/product/${PRODUCT_P001}`));
  await expect(block.locator('tbody tr.ant-table-row').nth(0)).toContainText('X 型扳手（新）');
});

test('product form shows empty state when no customer product names', async ({ page }) => {
  await setup(page, () => undefined);
  await page.goto(`/#/entities/product/${PRODUCT_P002}`);
  const block = page.getByRole('region', { name: '客戶品名' });
  await expect(block).toContainText('尚未設定客戶品名');
  await block.scrollIntoViewIfNeeded();
  await block.screenshot({ path: test.info().outputPath('product-customer-names-empty.png') });

  // 新增商品時不顯示
  await page.goto('/#/entities/product/new');
  await expect(page.getByRole('textbox', { name: '料號' })).toBeVisible();
  await expect(page.getByRole('region', { name: '客戶品名' })).toHaveCount(0);
});
