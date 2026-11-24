import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

import type { RecordListQuery_Deserialize as RecordListQuery } from '../src/bindings';
import {
  CUSTOMER,
  CUSTOMER_CONTACT,
  CUSTOMER_CONTACT_FIELDS,
  CUSTOMER_FIELDS,
  CUSTOMER_FORM_LAYOUT,
  PRODUCT,
  PRODUCT_FIELDS,
  SUPPLIER,
  SUPPLIER_FIELDS,
  customerDetail,
  customerId,
  fakeListRecords,
  manyCustomers,
  partyRecord,
} from '../src/test/masterSplitFixtures';
import { SPLIT_DETAIL_PREVIEW_WIDTH } from '../src/records/splitPreviewWidth';
import { CommandRejection, mockTauri } from './tauri-mock';

const ADMIN = {
  id: '0196f5a0-0000-7000-8000-000000000001',
  username: 'admin',
  display_name: '王管理',
  role: 'admin',
  permissions: ['audit.read', 'record.post', 'record.read', 'record.write', 'schema.manage'],
  must_change_password: false,
};

const NEW_ID = '0196f5a0-0000-7000-8000-00000000beef';
const ENTITIES = [PRODUCT, CUSTOMER, CUSTOMER_CONTACT, SUPPLIER];
const FIELDS: Record<string, unknown> = {
  customer: CUSTOMER_FIELDS,
  supplier: SUPPLIER_FIELDS,
  product: PRODUCT_FIELDS,
  customer_contact: CUSTOMER_CONTACT_FIELDS,
};

const VIEWER = {
  ...ADMIN,
  id: '0196f5a0-0000-7000-8000-000000000002',
  username: 'viewer',
  display_name: '李唯讀',
  role: 'viewer',
  permissions: ['record.read'],
};

async function setup(
  page: Page,
  user: typeof ADMIN = ADMIN,
  formLayout: typeof CUSTOMER_FORM_LAYOUT = CUSTOMER_FORM_LAYOUT,
) {
  // 4 筆固定的客戶＋80 筆一般客戶（啟用中 83 筆），超過一頁（50 筆）；刪除後從清單移除
  let customers = manyCustomers(80);
  await mockTauri(page, {
    get_server_url: () => 'http://localhost:8080',
    health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
    setup_status: () => ({ needs_setup: false }),
    current_user: () => user,
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
        // 與「依欄位定義」相同，不會顯示「未儲存的變更」
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
      const r = customerDetail(customers, id as string);
      if (!r) {
        throw new CommandRejection({
          kind: 'api',
          code: 'NOT_FOUND',
          message: '找不到資料',
          details: null,
        });
      }
      return r;
    },
    get_form_layout: () => formLayout,
    delete_record: ({ id }) => {
      customers = customers.filter((c) => c.id !== id);
      return null;
    },
    update_record: ({ id, req }) => {
      const { data } = req as { data: Record<string, unknown> };
      customers = customers.map((c) => {
        if (c.id !== id) {
          return c;
        }
        const merged = { ...c.data, ...data };
        return {
          ...c,
          data: merged,
          title: `${String(merged.code)} ${String(merged.name)}`,
          version: c.version + 1,
        };
      });
      return customerDetail(customers, id as string);
    },
    create_record: ({ req }) => {
      const { data } = req as { data: Record<string, unknown> };
      customers = [...customers, partyRecord(NEW_ID, data)];
      return customerDetail(customers, NEW_ID);
    },
    list_currencies: () => [],
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
  await expect(page.getByTestId('preview-title')).toHaveText('C004 Global Trading Co.');

  await page.reload();
  await expect(listbox(page).getByRole('option', { name: /C004/ })).toHaveAttribute(
    'aria-selected',
    'true',
  );
  await expect(page.getByTestId('preview-title')).toHaveText('C004 Global Trading Co.');
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

test('customer preview: form layout, contacts, delete', async ({ page }) => {
  await setup(page);
  await page.goto(`/#/entities/customer?selected=${customerId(1)}`);

  const preview = page.getByTestId('record-preview');
  await expect(preview.getByTestId('preview-title')).toHaveText('C001 大華實業股份有限公司');
  await expect(preview.getByRole('region', { name: '基本資料' })).toBeVisible();
  // 2 欄：名稱在左、編號在右（同一列）
  const name = await preview.locator('[data-item="name"]').boundingBox();
  const code = await preview.locator('[data-item="code"]').boundingBox();
  expect(code!.x).toBeGreaterThan(name!.x);
  expect(Math.abs(code!.y - name!.y)).toBeLessThan(2);
  const contacts = preview.getByRole('region', { name: '客戶聯絡人' });
  await expect(contacts.locator('tbody tr.ant-table-row')).toHaveCount(2);
  await expect(contacts).toContainText('王小明');
  await expect(preview.getByRole('button', { name: /編\s*輯/ })).toBeVisible();

  // 刪除另一筆：選取清除、列表更新
  await page
    .getByRole('listbox', { name: '客戶列表' })
    .getByRole('option', { name: /C002/ })
    .click();
  await expect(preview.getByTestId('preview-title')).toHaveText('C002 光明電子有限公司');
  await preview.getByRole('button', { name: /刪\s*除/ }).click();
  await page
    .locator('.ant-popconfirm')
    .getByRole('button', { name: /刪\s*除/ })
    .click();
  await expect(page.getByText('請從左側選擇客戶')).toBeVisible();
  await expect(page).not.toHaveURL(/selected=/);
  await expect(page.getByTestId('picker-total')).toHaveText('共 82 筆');
  await expect(listbox(page).getByRole('option', { name: /C002/ })).toHaveCount(0);
});

test('customer preview: viewer is read only, missing record shows not found', async ({ page }) => {
  await setup(page, VIEWER);
  await page.goto(`/#/entities/customer?selected=${customerId(1)}`);
  const preview = page.getByTestId('record-preview');
  await expect(preview.getByTestId('preview-title')).toHaveText('C001 大華實業股份有限公司');
  await expect(preview.getByRole('button', { name: /編\s*輯/ })).toHaveCount(0);
  await expect(preview.getByRole('button', { name: /刪\s*除/ })).toHaveCount(0);
  await expect(page.getByRole('button', { name: /新\s*增/ })).toHaveCount(0);

  await page.goto('/#/entities/customer?selected=0196f5a0-0000-7000-8000-00000000dead');
  await expect(page.getByText('找不到資料')).toBeVisible();
});

test('customer edit and new on the right side', async ({ page }) => {
  await setup(page);
  await page.goto(`/#/entities/customer?selected=${customerId(1)}`);
  const list = listbox(page);
  const preview = page.getByTestId('record-preview');
  await preview.getByRole('button', { name: /編\s*輯/ }).click();

  const panel = page.getByTestId('record-form-panel');
  await expect(panel.getByText('編輯：C001 大華實業股份有限公司')).toBeVisible();
  // 編輯時左側列表仍然看得到
  await expect(list).toBeVisible();
  const name = panel.getByRole('textbox', { name: '公司名稱' });
  await name.fill('大華科技股份有限公司');
  await expect(panel.getByText('未存檔')).toBeVisible();

  // 未存檔時點其他資料：留下
  await list.getByRole('option', { name: /C002/ }).click();
  const confirm = page.getByRole('dialog').filter({ hasText: '有未存檔的修改' });
  await expect(confirm).toBeVisible();
  await confirm.getByRole('button', { name: /留\s*下/ }).click();
  await expect(name).toHaveValue('大華科技股份有限公司');

  // 換頁（左側選單）：留下
  await page
    .getByRole('navigation', { name: '主選單' })
    .getByRole('menuitem', { name: /商品/ })
    .click();
  await expect(confirm).toBeVisible();
  await confirm.getByRole('button', { name: /留\s*下/ }).click();
  await expect(page).toHaveURL(/entities\/customer/);

  await panel.getByRole('button', { name: /儲\s*存/ }).click();
  await expect(preview.getByTestId('preview-title')).toHaveText('C001 大華科技股份有限公司');
  await expect(list.getByRole('option', { name: /C001/ })).toContainText('大華科技股份有限公司');

  // 新增
  await page
    .getByTestId('page-header')
    .getByRole('button', { name: /新\s*增/ })
    .click();
  await expect(panel.getByRole('heading', { name: '新增客戶' })).toBeVisible();
  await panel.getByRole('textbox', { name: '客戶編號' }).fill('C000');
  await panel.getByRole('textbox', { name: '公司名稱' }).fill('新開發客戶');
  await panel.getByRole('button', { name: /儲\s*存/ }).click();
  await expect(preview.getByTestId('preview-title')).toHaveText('C000 新開發客戶');
  await expect(page).toHaveURL(new RegExp(`selected=${NEW_ID}`));
  await expect(list.getByRole('option').first()).toContainText('C000新開發客戶');
  await expect(list.getByRole('option').first()).toHaveAttribute('aria-selected', 'true');
});

test('customer edit form screenshot with contacts', async ({ page }) => {
  await setup(page);
  await page.goto(`/#/entities/customer?selected=${customerId(1)}&mode=edit`);
  const panel = page.getByTestId('record-form-panel');
  await expect(panel.getByRole('textbox', { name: '公司名稱' })).toHaveValue(
    '大華實業股份有限公司',
  );
  await expect(panel.getByRole('region', { name: '客戶聯絡人' })).toBeVisible();
});

test('old customer urls redirect to the split view', async ({ page }) => {
  await setup(page);
  await page.goto(`/#/entities/customer/${customerId(2)}`);
  await expect(page.getByTestId('preview-title')).toHaveText('C002 光明電子有限公司');
  await expect(page).toHaveURL(new RegExp(`#/entities/customer\\?selected=${customerId(2)}$`));

  await page.goto('/#/entities/customer/new');
  await expect(
    page.getByTestId('record-form-panel').getByRole('heading', { name: '新增客戶' }),
  ).toBeVisible();
  await expect(listbox(page)).toBeVisible();
});

test('customer view toggle: table is remembered, row click opens split', async ({ page }) => {
  await setup(page);
  await page.goto('/#/entities/customer');
  await expect(listbox(page).getByRole('option').first()).toContainText('C001');

  await page.getByRole('radiogroup', { name: '檢視方式' }).getByText('表格').click();
  await expect(page.getByRole('columnheader', { name: '客戶編號' })).toBeVisible();
  await expect(page.getByRole('button', { name: /欄位設定/ })).toBeVisible();
  await expect(page.getByRole('listbox')).toHaveCount(0);

  // 重新開啟頁面：記得表格
  await page.reload();
  await expect(page.getByRole('columnheader', { name: '客戶編號' })).toBeVisible();
  await expect(page.getByRole('cell', { name: 'C002' })).toBeVisible();

  // 點一列：切回分割檢視並選取
  await page.getByRole('cell', { name: 'C002' }).click();
  await expect(page.getByTestId('preview-title')).toHaveText('C002 光明電子有限公司');
  await expect(listbox(page).getByRole('option', { name: /C002/ })).toHaveAttribute(
    'aria-selected',
    'true',
  );
  await page.reload();
  await expect(listbox(page)).toBeVisible();
});

test('customer table view keeps the toggle in the header', async ({ page }) => {
  await setup(page);
  await page.addInitScript(() =>
    window.localStorage.setItem('hammer.recordView.customer', 'table'),
  );
  await page.goto('/#/entities/customer');
  await expect(page.getByRole('cell', { name: 'C001' })).toBeVisible();
  const toggle = page.getByTestId('page-header').getByRole('radiogroup', { name: '檢視方式' });
  await expect(toggle).toBeVisible();
  await expect(toggle.getByRole('radio', { name: /表格/ })).toBeChecked();
});

// 分割檢視右側寬度約 630–790px：4 欄、3 欄的區塊逐步減少欄數，不直接退回單欄（#251）
const WIDE_CUSTOMER_LAYOUT: typeof CUSTOMER_FORM_LAYOUT = {
  ...CUSTOMER_FORM_LAYOUT,
  config: {
    sections: [
      {
        key: 'four',
        title: '四欄區塊',
        columns: 4,
        collapsible: false,
        collapsed: false,
        items: [
          { type: 'field', field: 'code', span: 1 },
          { type: 'field', field: 'name', span: 3 },
          { type: 'field', field: 'short_name', span: 1 },
          { type: 'spacer', span: 1 },
          { type: 'field', field: 'tax_id', span: 1 },
        ],
      },
      {
        key: 'three',
        title: '三欄區塊',
        columns: 3,
        collapsible: false,
        collapsed: false,
        items: [
          { type: 'field', field: 'phone', span: 1 },
          { type: 'spacer', span: 1 },
          { type: 'field', field: 'is_active', span: 1 },
        ],
      },
      CUSTOMER_FORM_LAYOUT.config.sections[2]!,
    ],
  },
};

for (const { viewport, three } of [
  { viewport: 1440, three: 3 },
  { viewport: 1280, three: 2 },
]) {
  test(`split view right side reduces columns at ${viewport}px`, async ({ page }) => {
    await page.setViewportSize({ width: viewport, height: 900 });
    await setup(page, ADMIN, WIDE_CUSTOMER_LAYOUT);

    for (const mode of ['preview', 'edit'] as const) {
      await page.goto(
        `/#/entities/customer?selected=${customerId(1)}${mode === 'edit' ? '&mode=edit' : ''}`,
      );
      const side = page.getByTestId(mode === 'edit' ? 'record-form-panel' : 'record-preview');
      await expect(side.locator('[data-item="name"]')).toBeVisible();
      const columns = (key: string) =>
        side
          .locator(`[data-section="${key}"] .form-layout-grid`)
          .evaluate((el) => getComputedStyle(el).gridTemplateColumns.split(' ').length);
      expect(await columns('four'), `${mode}：4 欄區塊`).toBe(2);
      expect(await columns('three'), `${mode}：3 欄區塊`).toBe(three);

      // 4 欄 → 2 欄：編號｜公司名稱（3 欄改為佔滿整列，換到下一列）；簡稱｜統一編號（空白格不顯示）
      const box = async (field: string) =>
        (await side.locator(`[data-item="${field}"]`).boundingBox())!;
      const [code, name, short, tax] = [
        await box('code'),
        await box('name'),
        await box('short_name'),
        await box('tax_id'),
      ];
      expect(name.y, `${mode}：公司名稱在編號下方`).toBeGreaterThan(code.y + code.height - 1);
      expect(name.width, `${mode}：公司名稱佔滿整列`).toBeGreaterThan(code.width * 2);
      expect(Math.abs(short.y - tax.y), `${mode}：簡稱｜統一編號`).toBeLessThan(2);
      expect(tax.x).toBeGreaterThan(short.x);
    }
  });
}

// 設計器預覽：唯讀預覽與分割檢視右側寬度，與實際的右側預覽一致（#252）
test('form layout designer previews the split view right side', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await setup(page);

  // 實際的右側預覽
  await page.goto(`/#/entities/customer?selected=${customerId(1)}`);
  const preview = page.getByTestId('record-preview');
  await expect(preview.locator('[data-item="name"]')).toBeVisible();
  const detail = (await page.locator('.master-split-detail').boundingBox())!;
  expect(Math.round(detail.width), '右側寬度常數').toBe(SPLIT_DETAIL_PREVIEW_WIDTH);
  const realGrid = (await preview.locator('.form-layout-container').first().boundingBox())!;
  const realName = (await preview.locator('[data-item="name"]').boundingBox())!;
  const realTax = (await preview.locator('[data-item="tax_id"]').boundingBox())!;

  await page.goto('/#/schema/customer/form-layout');
  await expect(page.getByTestId('layout-applies-to')).toHaveText(
    '這份版面套用在：新增、編輯與檢視的表單、分割檢視右側的唯讀預覽、分割檢視右側的編輯與新增。',
  );
  await page.getByText('預覽', { exact: true }).click();
  const designerPreview = page.getByRole('region', { name: '表單預覽' });
  // 預設：編輯表單、整頁
  await expect(designerPreview.locator('[data-item="name"] input')).toBeDisabled();
  await expect(page.getByTestId('split-preview-frame')).toHaveCount(0);

  // 唯讀預覽、分割檢視右側：寬度與欄位位置與實際的右側預覽相同
  await page.getByText('唯讀預覽', { exact: true }).click();
  await page.getByText('分割檢視右側', { exact: true }).click();
  const frame = page.getByTestId('split-preview-frame');
  await expect(frame).toBeVisible();
  expect(Math.round((await frame.boundingBox())!.width)).toBe(SPLIT_DETAIL_PREVIEW_WIDTH);
  await expect(designerPreview.getByRole('textbox')).toHaveCount(0);
  await expect(frame.locator('[data-item="name"]')).toContainText('公司名稱範例');
  const grid = (await frame.locator('.form-layout-container').first().boundingBox())!;
  const name = (await frame.locator('[data-item="name"]').boundingBox())!;
  const tax = (await frame.locator('[data-item="tax_id"]').boundingBox())!;
  expect(Math.abs(grid.width - realGrid.width)).toBeLessThanOrEqual(1);
  expect(Math.abs(name.width - realName.width)).toBeLessThanOrEqual(1);
  expect(Math.abs(tax.width - realTax.width)).toBeLessThanOrEqual(1);
  expect(Math.abs(name.height - realName.height)).toBeLessThanOrEqual(1);
  await page.screenshot({ path: test.info().outputPath('readonly-split.png') });

  // 編輯表單、分割檢視右側
  await page.getByText('編輯表單', { exact: true }).click();
  await expect(frame.locator('[data-item="name"] input')).toBeDisabled();
  await page.screenshot({ path: test.info().outputPath('form-split.png') });

  // 整頁：不再限制寬度
  await page.getByText('整頁', { exact: true }).click();
  await expect(page.getByTestId('split-preview-frame')).toHaveCount(0);
  expect((await designerPreview.boundingBox())!.width).toBeGreaterThan(SPLIT_DETAIL_PREVIEW_WIDTH);
});
