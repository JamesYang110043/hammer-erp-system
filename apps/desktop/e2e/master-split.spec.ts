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
  formLayout: typeof CUSTOMER_FORM_LAYOUT | null = CUSTOMER_FORM_LAYOUT,
  fieldOverrides: Record<string, unknown> = {},
) {
  const fieldsOf = (key: string) => fieldOverrides[key] ?? FIELDS[key] ?? [];
  // 4 筆固定的客戶＋80 筆一般客戶（啟用中 83 筆），超過一頁（50 筆）；刪除後從清單移除
  let customers = manyCustomers(80);
  let layout = formLayout;
  await mockTauri(page, {
    get_server_url: () => 'http://localhost:8080',
    health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
    setup_status: () => ({ needs_setup: false }),
    current_user: () => user,
    list_entities: () => ENTITIES,
    list_all_entities: () => ENTITIES,
    get_entity: ({ key }) => ({
      entity: ENTITIES.find((e) => e.key === key),
      fields: fieldsOf(key as string),
    }),
    get_default_layout: ({ key }) => ({
      source: 'generated',
      layout: null,
      config: {
        // 與「依欄位定義」相同，不會顯示「未儲存的變更」
        columns: (fieldsOf(key as string) as { field_key: string }[]).map((f) => ({
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
    get_form_layout: () => {
      if (!layout) {
        throw new CommandRejection({ kind: 'network', message: '無法連線' });
      }
      return layout;
    },
    save_form_layout: ({ req }) => {
      const { config } = req as { config: typeof CUSTOMER_FORM_LAYOUT.config };
      layout = { ...CUSTOMER_FORM_LAYOUT, ...layout, version: (layout?.version ?? 0) + 1, config };
      return layout;
    },
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

// 從分割檢視右側調整表單版面（#253）：返回與存檔後回到原本選取的資料
test('adjust form layout from the split view and come back', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await setup(page);
  await page.goto(`/#/entities/customer?selected=${customerId(2)}`);
  const preview = page.getByTestId('record-preview');
  await expect(preview.getByTestId('preview-title')).toHaveText('C002 光明電子有限公司');
  await page.screenshot({ path: test.info().outputPath('preview-header.png') });

  // 返回：選取不變
  await preview.getByRole('button', { name: /調整表單版面/ }).click();
  await expect(page.getByText('表單版面：客戶')).toBeVisible();
  await page.getByRole('button', { name: '回到「客戶」' }).click();
  await expect(preview.getByTestId('preview-title')).toHaveText('C002 光明電子有限公司');
  await expect(page).toHaveURL(new RegExp(`selected=${customerId(2)}`));

  // 改成 1 欄並存檔：回到分割檢視，預覽依新的版面
  await preview.getByRole('button', { name: /調整表單版面/ }).click();
  const basic = page.getByRole('region', { name: '區塊：基本資料' });
  await basic.getByRole('combobox', { name: '欄數' }).click({ force: true });
  await page.locator('.ant-select-dropdown:visible').getByText('1 欄', { exact: true }).click();
  await page.getByRole('button', { name: /儲\s*存/ }).click();
  await expect(preview.getByTestId('preview-title')).toHaveText('C002 光明電子有限公司');
  await expect(page.getByText('有未存檔的修改')).toHaveCount(0);
  const name = (await preview.locator('[data-item="name"]').boundingBox())!;
  const code = (await preview.locator('[data-item="code"]').boundingBox())!;
  expect(code.y, '1 欄：編號在名稱下方').toBeGreaterThan(name.y + name.height - 1);
});

test('viewer does not see adjust form layout', async ({ page }) => {
  await setup(page, VIEWER);
  await page.goto(`/#/entities/customer?selected=${customerId(1)}`);
  const preview = page.getByTestId('record-preview');
  await expect(preview.getByTestId('preview-title')).toBeVisible();
  await expect(preview.getByRole('button', { name: /調整表單版面/ })).toHaveCount(0);
});

// 客戶的分組預設版面（#254）：讀取版面失敗時由前端產生，與伺服器相同
const FULL_CUSTOMER_FIELDS = [
  ...CUSTOMER_FIELDS.filter((f) => f.field_key !== 'is_active'),
  ...(
    [
      ['fax', '傳真', 60],
      ['email', 'Email', 70],
      ['address', '聯絡地址', 80],
      ['invoice_title', '發票抬頭', 90],
      ['invoice_address', '發票地址', 100],
      ['payment_terms_days', '付款天數', 110],
      ['credit_limit', '信用額度', 120],
    ] as const
  ).map(([field_key, label, sort_order]) => ({
    ...CUSTOMER_FIELDS[0]!,
    id: `f-customer-${field_key}`,
    field_key,
    label,
    sort_order,
    is_required: false,
    data_type:
      field_key === 'payment_terms_days'
        ? 'integer'
        : field_key === 'credit_limit'
          ? 'money'
          : 'text',
    decimal_places: field_key === 'credit_limit' ? 2 : null,
  })),
  CUSTOMER_FIELDS.find((f) => f.field_key === 'is_active')!,
  {
    ...CUSTOMER_FIELDS[0]!,
    id: 'f-customer-notes',
    field_key: 'notes',
    label: '備註',
    data_type: 'long_text' as const,
    is_required: false,
    sort_order: 140,
  },
  {
    ...CUSTOMER_FIELDS[0]!,
    id: 'f-customer-region',
    field_key: 'region',
    label: '業務區域',
    is_system: false,
    is_required: false,
    sort_order: 200,
  },
];

for (const viewport of [1440, 1280]) {
  test(`customer default layout is grouped in two columns at ${viewport}px`, async ({ page }) => {
    await page.setViewportSize({ width: viewport, height: 1000 });
    await setup(page, ADMIN, null, { customer: FULL_CUSTOMER_FIELDS });
    await page.goto(`/#/entities/customer?selected=${customerId(1)}`);
    const preview = page.getByTestId('record-preview');
    await expect(preview.getByTestId('preview-title')).toHaveText('C001 大華實業股份有限公司');

    const titles = await preview
      .locator('section[data-section]')
      .evaluateAll((els) => els.map((el) => el.getAttribute('aria-label')));
    // 最後是沒有標題的系統區塊（聯絡人明細）
    expect(titles).toEqual(['基本資料', '發票資訊', '交易條件', '其他', '其他欄位', null]);
    await expect(
      preview
        .locator('section[data-section="_blocks"]')
        .getByRole('region', { name: '客戶聯絡人' }),
    ).toBeVisible();
    const box = async (field: string) =>
      (await preview.locator(`[data-item="${field}"]`).boundingBox())!;
    // 2 欄：編號｜名稱 同一列；備註佔滿整列；自訂欄位在「其他欄位」
    const [code, name, notes] = [await box('code'), await box('name'), await box('notes')];
    expect(Math.abs(code.y - name.y)).toBeLessThan(2);
    expect(name.x).toBeGreaterThan(code.x);
    expect(notes.width).toBeGreaterThan(code.width * 2);
    await expect(
      preview.getByRole('region', { name: '其他欄位' }).locator('[data-item="region"]'),
    ).toBeVisible();
  });
}

// 設計預覽與實際顯示相同（#259）：主檔沒有單號區塊；設計器「分割檢視右側」預覽（唯讀、編輯表單）
// 與實際的右側（唯讀、編輯、新增）逐項比對區塊、列、欄與寬度

/** 一個項目的位置：所在區塊（依顯示順序）、區塊內的列、相對於區塊格線的 x 與寬度。 */
interface ItemPlace {
  item: string;
  section: string;
  row: number;
  x: number;
  width: number;
}

async function layoutPlaces(root: ReturnType<Page['locator']>): Promise<ItemPlace[]> {
  return root.locator('section[data-section]').evaluateAll((sections) =>
    sections.flatMap((section) => {
      const title = section.getAttribute('aria-label') ?? section.getAttribute('data-section')!;
      const grid = section.querySelector('.form-layout-grid') ?? section;
      const left = grid.getBoundingClientRect().left;
      const items = Array.from(section.querySelectorAll<HTMLElement>('[data-item]'))
        .map((el) => ({ el, box: el.getBoundingClientRect() }))
        .filter(({ box }) => box.width > 0 && box.height > 0);
      const rows: number[] = [];
      for (const { box } of items) {
        if (!rows.some((y) => Math.abs(y - box.top) < 2)) {
          rows.push(box.top);
        }
      }
      rows.sort((a, b) => a - b);
      return items.map(({ el, box }) => ({
        item: el.getAttribute('data-item')!,
        section: title,
        row: rows.findIndex((y) => Math.abs(y - box.top) < 2),
        x: box.left - left,
        width: box.width,
      }));
    }),
  );
}

function expectSamePlaces(actual: ItemPlace[], expected: ItemPlace[], label: string) {
  const outline = (places: ItemPlace[]) => places.map((p) => [p.section, p.item, p.row]);
  expect(outline(actual), `${label}：區塊、項目與列`).toEqual(outline(expected));
  actual.forEach((p, i) => {
    const e = expected[i]!;
    expect(Math.abs(p.x - e.x), `${label}：${p.item} 的欄位置`).toBeLessThanOrEqual(1);
    expect(Math.abs(p.width - e.width), `${label}：${p.item} 的寬度`).toBeLessThanOrEqual(1);
  });
}

const FULL_SUPPLIER_FIELDS = FULL_CUSTOMER_FIELDS.filter((f) => f.field_key !== 'region').map(
  (f) => ({ ...f, id: f.id.replace('customer', 'supplier'), entity_id: SUPPLIER.id }),
);

/** 伺服器回傳的分組預設版面（§7.7；主檔沒有單號區塊）。 */
function defaultLayout(entityKey: string, fields: { field_key: string }[], details: string[]) {
  const keys = fields.map((f) => f.field_key);
  const supplier = entityKey === 'supplier';
  const groups: [string, string, string[]][] = [
    [
      'basic',
      '基本資料',
      [
        ...['code', 'name', 'short_name', 'tax_id', 'phone', 'fax', 'email', 'address'],
        ...(supplier ? ['country'] : []),
        'is_active',
      ],
    ],
    ['invoice', '發票資訊', ['invoice_title', 'invoice_address']],
    [
      'terms',
      '交易條件',
      [
        'payment_terms_days',
        'credit_limit',
        ...(supplier ? ['default_currency_code', 'bank_info'] : []),
      ],
    ],
    ['other', '其他', ['notes']],
  ];
  const listed = new Set(groups.flatMap(([, , g]) => g));
  const plain = { collapsible: false, collapsed: false };
  const sections = [
    ...groups
      .map(([key, title, g]) => ({
        key,
        title,
        columns: 2,
        ...plain,
        items: g
          .filter((f) => keys.includes(f))
          .map((field) => ({ type: 'field', field, span: field === 'notes' ? 2 : 1 })),
      }))
      .filter((s) => s.items.length > 0),
    {
      key: '_other',
      title: '其他欄位',
      columns: 1,
      ...plain,
      items: keys.filter((k) => !listed.has(k)).map((field) => ({ type: 'field', field, span: 1 })),
    },
    {
      key: '_blocks',
      title: '',
      columns: 1,
      ...plain,
      items: details.map((d) => ({ type: 'block', block: `lines:${d}` })),
    },
  ].filter((s) => s.items.length > 0);
  return {
    id: null,
    version: null,
    is_default: true,
    config: { sections },
  } as unknown as typeof CUSTOMER_FORM_LAYOUT;
}

for (const { entity, label, fields, details } of [
  {
    entity: 'customer',
    label: '客戶',
    fields: FULL_CUSTOMER_FIELDS,
    details: ['customer_contact'],
  },
  { entity: 'supplier', label: '廠商', fields: FULL_SUPPLIER_FIELDS, details: [] },
]) {
  for (const preset of [true, false]) {
    const name = `${label}${preset ? '預設' : '自訂'}版面`;
    test(`designer split preview matches the right side: ${name}`, async ({ page }) => {
      await page.setViewportSize({ width: 1440, height: 1000 });
      const layout = preset ? defaultLayout(entity, fields, details) : CUSTOMER_FORM_LAYOUT;
      await setup(page, ADMIN, layout, { [entity]: fields });

      // 實際的分割檢視右側：唯讀、編輯、新增
      await page.goto(`/#/entities/${entity}?selected=${customerId(1)}`);
      const preview = page.getByTestId('record-preview');
      await expect(preview.locator('[data-item="name"]')).toBeVisible();
      await expect(preview.getByText('單號')).toHaveCount(0);
      const readonly = await layoutPlaces(preview);
      await page.goto(`/#/entities/${entity}?selected=${customerId(1)}&mode=edit`);
      const panel = page.getByTestId('record-form-panel');
      await expect(panel.locator('[data-item="name"] input')).toBeVisible();
      const edit = await layoutPlaces(panel);
      await page
        .getByTestId('page-header')
        .getByRole('button', { name: /新\s*增/ })
        .click();
      await expect(panel.getByRole('heading', { name: `新增${label}` })).toBeVisible();
      const create = await layoutPlaces(panel);
      await expect(panel.getByText('單號')).toHaveCount(0);
      expectSamePlaces(create, edit, '新增與編輯');

      if (preset) {
        // 基本資料第 1 列：編號｜公司名稱；第 2 列：簡稱｜統一編號
        const basic = readonly.filter((p) => p.section === '基本資料');
        expect(basic.slice(0, 4).map((p) => [p.item, p.row])).toEqual([
          ['code', 0],
          ['name', 0],
          ['short_name', 1],
          ['tax_id', 1],
        ]);
      }

      // 設計器：區塊與「尚未放進版面」都沒有單號
      await page.goto(`/#/schema/${entity}/form-layout`);
      await expect(page.getByText(`表單版面：${label}`)).toBeVisible();
      await expect(page.getByText('單號', { exact: true })).toHaveCount(0);
      await expect(page.getByRole('button', { name: '單號' })).toHaveCount(0);

      await page.getByText('預覽', { exact: true }).click();
      await page.getByText('分割檢視右側', { exact: true }).click();
      const frame = page.getByTestId('split-preview-frame');
      await page.getByText('唯讀預覽', { exact: true }).click();
      await expect(frame.locator('[data-item="name"]')).toContainText('公司名稱範例');
      await expect(frame.getByText('單號')).toHaveCount(0);
      expectSamePlaces(await layoutPlaces(frame), readonly, '唯讀預覽');
      await page.screenshot({ path: test.info().outputPath(`${entity}-${preset}-readonly.png`) });

      await page.getByText('編輯表單', { exact: true }).click();
      await expect(frame.locator('[data-item="name"] input')).toBeDisabled();
      await expect(frame.getByText('單號')).toHaveCount(0);
      expectSamePlaces(await layoutPlaces(frame), edit, '編輯表單');
      await page.screenshot({ path: test.info().outputPath(`${entity}-${preset}-form.png`) });
    });
  }
}
