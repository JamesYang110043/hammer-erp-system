import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

import type {
  EntityDefinition,
  FormLayoutConfig_Serialize as FormLayoutConfig,
} from '../src/bindings';
import { REPAIR, fieldDef } from '../src/test/schemaFixtures';
import { mockTauri } from './tauri-mock';

// 表單依版面渲染（#222，docs/architecture.md §7.7）

const ADMIN = {
  id: '0196f5a0-0000-7000-8000-000000000001',
  username: 'admin',
  display_name: '王管理',
  role: 'admin',
  permissions: ['record.read', 'record.write', 'schema.manage'],
  must_change_password: false,
};

const ORDER: EntityDefinition = {
  ...REPAIR,
  kind: 'document',
  numbering_pattern: 'R{YYYYMM}-{####}',
};
const PARTS: EntityDefinition = {
  ...REPAIR,
  id: '0196f5a0-0000-7000-8000-000000000110',
  key: 'parts',
  label: '零件',
  kind: 'detail',
  parent_entity_id: REPAIR.id,
  sort_order: 1,
};

const FIELDS = [
  fieldDef({ field_key: 'name', label: '客戶名稱', is_required: true, sort_order: 0 }),
  fieldDef({ field_key: 'phone', label: '電話', sort_order: 1 }),
  fieldDef({ field_key: 'contact', label: '聯絡人', sort_order: 2 }),
  fieldDef({ field_key: 'address', label: '地址', sort_order: 3 }),
  fieldDef({ field_key: 'repair_date', label: '報修日期', data_type: 'date', sort_order: 4 }),
  fieldDef({ field_key: 'note', label: '備註', data_type: 'long_text', sort_order: 5 }),
  fieldDef({ field_key: 'inspector', label: '檢修人員', sort_order: 6 }),
];
const PART_FIELDS = [
  fieldDef({ entity_id: PARTS.id, field_key: 'part', label: '品名', sort_order: 0 }),
  fieldDef({
    entity_id: PARTS.id,
    field_key: 'qty',
    label: '數量',
    data_type: 'integer',
    sort_order: 1,
  }),
];

const LAYOUT: FormLayoutConfig = {
  sections: [
    {
      key: 'basic',
      title: '基本資料',
      columns: 3,
      collapsible: false,
      collapsed: false,
      items: [
        { type: 'block', block: 'record_no' },
        { type: 'field', field: 'repair_date', span: 1 },
        { type: 'spacer', span: 1 },
        { type: 'field', field: 'name', span: 2 },
        { type: 'field', field: 'contact', span: 1 },
        { type: 'field', field: 'phone', span: 1 },
        { type: 'field', field: 'address', span: 2 },
      ],
    },
    {
      key: 'lines',
      title: '',
      columns: 1,
      collapsible: false,
      collapsed: false,
      items: [{ type: 'block', block: 'lines:parts' }],
    },
    {
      key: 'extra',
      title: '檢修紀錄',
      columns: 2,
      collapsible: true,
      collapsed: true,
      items: [
        { type: 'field', field: 'inspector', span: 1 },
        { type: 'field', field: 'note', span: 2 },
      ],
    },
  ],
};

async function mockServer(page: Page) {
  await mockTauri(page, {
    get_server_url: () => 'http://localhost:8080',
    health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
    setup_status: () => ({ needs_setup: false }),
    current_user: () => ADMIN,
    list_entities: () => [ORDER, PARTS],
    get_entity: ({ key }) =>
      key === 'parts' ? { entity: PARTS, fields: PART_FIELDS } : { entity: ORDER, fields: FIELDS },
    get_form_layout: () => ({
      id: '0196f5a0-0000-7000-8000-000000000301',
      version: 1,
      is_default: false,
      config: LAYOUT,
    }),
  });
}

/** 欄位（`.ant-form-item`）的位置。 */
async function itemBox(page: Page, label: string) {
  const item = page
    .locator('.ant-form-item')
    .filter({ has: page.locator('label').getByText(label, { exact: true }) });
  const box = await item.boundingBox();
  expect(box, label).not.toBeNull();
  return box!;
}

async function expectSameRow(page: Page, a: string, b: string) {
  const [boxA, boxB] = [await itemBox(page, a), await itemBox(page, b)];
  expect(Math.abs(boxA.y - boxB.y), `${a} / ${b} 同一列`).toBeLessThanOrEqual(1);
  expect(boxA.x + boxA.width, `${a} 在 ${b} 左邊`).toBeLessThanOrEqual(boxB.x);
}

async function expectBelow(page: Page, a: string, b: string) {
  const [boxA, boxB] = [await itemBox(page, a), await itemBox(page, b)];
  expect(boxB.y, `${b} 在 ${a} 下方`).toBeGreaterThanOrEqual(boxA.y + boxA.height - 1);
}

async function expectNoHorizontalScroll(page: Page) {
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  );
  expect(overflow, '不出現水平捲軸').toBeLessThanOrEqual(0);
}

test('form follows the layout: columns, spans, spacer, blocks, collapsed section', async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await mockServer(page);
  await page.goto('/#/entities/repair/new');
  await expect(page.getByLabel('客戶名稱')).toBeVisible();

  // 單號｜報修日期｜（空白）
  await expectSameRow(page, '單號', '報修日期');
  // 客戶名稱（2 欄）｜聯絡人
  await expectBelow(page, '單號', '客戶名稱');
  await expectSameRow(page, '客戶名稱', '聯絡人');
  const [name, contact] = [await itemBox(page, '客戶名稱'), await itemBox(page, '聯絡人')];
  expect(name.width, '客戶名稱佔 2 欄').toBeGreaterThan(contact.width * 1.8);
  // 電話｜地址（2 欄）
  await expectSameRow(page, '電話', '地址');

  // 明細表在基本資料之後
  const lines = page.getByRole('region', { name: '零件' });
  await expect(lines).toBeVisible();
  const linesBox = (await lines.boundingBox())!;
  expect(linesBox.y).toBeGreaterThan((await itemBox(page, '地址')).y);

  // 收折的區塊：預設收折，點開後兩欄
  await expect(page.getByLabel('檢修人員')).toBeHidden();
  await page.getByRole('button', { name: /檢修紀錄/ }).click();
  await expect(page.getByLabel('檢修人員')).toBeVisible();
  await expectBelow(page, '檢修人員', '備註');

  // Tab 鍵：左到右、上到下
  await page.getByLabel('客戶名稱').focus();
  await page.keyboard.press('Tab');
  await expect(page.getByLabel('聯絡人')).toBeFocused();
  await page.keyboard.press('Tab');
  await expect(page.getByLabel('電話')).toBeFocused();
  await expect(page.getByLabel('檢修人員')).toBeVisible();

  await expectNoHorizontalScroll(page);
});

test('narrow window falls back to a single column without horizontal scroll', async ({ page }) => {
  await page.setViewportSize({ width: 900, height: 900 });
  await mockServer(page);
  await page.goto('/#/entities/repair/new');
  await expect(page.getByLabel('客戶名稱')).toBeVisible();

  // 3 欄的區塊寬度不足時退回單欄：依版面順序由上往下
  await expectBelow(page, '單號', '報修日期');
  await expectBelow(page, '報修日期', '客戶名稱');
  await expectBelow(page, '客戶名稱', '聯絡人');
  const [name, contact] = [await itemBox(page, '客戶名稱'), await itemBox(page, '聯絡人')];
  expect(Math.abs(name.x - contact.x)).toBeLessThanOrEqual(1);

  await expectNoHorizontalScroll(page);
});
