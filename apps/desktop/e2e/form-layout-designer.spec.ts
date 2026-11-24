import { expect, test } from '@playwright/test';
import type { Locator, Page } from '@playwright/test';

import type {
  EntityDefinition,
  FormLayoutConfig_Serialize as FormLayoutConfig,
  FormLayout_Serialize as FormLayout,
} from '../src/bindings';
import { REPAIR, fieldDef } from '../src/test/schemaFixtures';
import { CommandRejection, mockTauri } from './tauri-mock';

// 表單版面設計器（#223，docs/architecture.md §7.7）

const PERMISSIONS = ['record.read', 'record.write', 'schema.manage'];

function user(role: 'admin' | 'editor') {
  return {
    id: '0196f5a0-0000-7000-8000-000000000001',
    username: role,
    display_name: role === 'admin' ? '王管理' : '陳編輯',
    role,
    permissions: role === 'admin' ? PERMISSIONS : ['record.read', 'record.write'],
    must_change_password: false,
  };
}

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
  fieldDef({ field_key: 'address', label: '地址', sort_order: 2 }),
  fieldDef({ field_key: 'note', label: '備註', data_type: 'long_text', sort_order: 3 }),
];
const PART_FIELDS = [fieldDef({ entity_id: PARTS.id, field_key: 'part', label: '品名' })];

/** 沒有設定版面時伺服器自動產生的版面。 */
const GENERATED: FormLayoutConfig = {
  sections: [
    {
      key: 'main',
      title: '',
      columns: 1,
      collapsible: false,
      collapsed: false,
      items: [
        { type: 'block', block: 'record_no' },
        ...FIELDS.map((f) => ({ type: 'field' as const, field: f.field_key, span: 1 })),
        { type: 'block', block: 'lines:parts' },
      ],
    },
  ],
};

/** 模擬伺服器：記住儲存的版面。`conflict` 時儲存回傳版本衝突。 */
async function mockServer(
  page: Page,
  { role = 'admin', conflict = false }: { role?: 'admin' | 'editor'; conflict?: boolean } = {},
) {
  let layout: FormLayout = { id: null, version: null, is_default: true, config: GENERATED };
  const saves: unknown[] = [];
  await mockTauri(page, {
    get_server_url: () => 'http://localhost:8080',
    health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
    setup_status: () => ({ needs_setup: false }),
    current_user: () => user(role),
    list_entities: () => [ORDER, PARTS],
    list_all_entities: () => [ORDER, PARTS],
    get_entity: ({ key }) =>
      key === 'parts' ? { entity: PARTS, fields: PART_FIELDS } : { entity: ORDER, fields: FIELDS },
    get_form_layout: () => layout,
    save_form_layout: ({ req }) => {
      saves.push(req);
      if (conflict) {
        throw new CommandRejection({
          kind: 'api',
          code: 'CONFLICT',
          message: '表單版面已被其他人修改，請重新載入',
          details: null,
        });
      }
      const { config } = req as { config: FormLayoutConfig };
      layout = {
        id: '0196f5a0-0000-7000-8000-000000000301',
        version: (layout.version ?? 0) + 1,
        is_default: false,
        config,
      };
      return layout;
    },
    reset_form_layout: () => {
      layout = { id: null, version: null, is_default: true, config: GENERATED };
      return layout;
    },
  });
  return { saves };
}

/** 設計器中某個區塊（`區塊：<標題>`）內的項目名稱，依順序。 */
async function itemsIn(section: Locator): Promise<string[]> {
  return section
    .locator('[data-design-item]')
    .evaluateAll((els) => els.map((el) => el.getAttribute('data-design-item') ?? ''));
}

/** 以滑鼠拖曳把手到目標的中央。 */
async function dragTo(page: Page, handle: Locator, target: Locator) {
  const from = (await handle.boundingBox())!;
  const to = (await target.boundingBox())!;
  await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
  await page.mouse.down();
  await page.mouse.move(from.x + from.width / 2 + 10, from.y + from.height / 2 + 10, { steps: 5 });
  await page.mouse.move(to.x + to.width / 2, to.y + to.height / 2, { steps: 15 });
  await page.mouse.move(to.x + to.width / 2 + 2, to.y + to.height / 2 + 2, { steps: 3 });
  await page.mouse.up();
}

/** 選擇目前展開的下拉選單中的選項。 */
async function pickOption(page: Page, label: string) {
  await page
    .locator('.ant-select-dropdown:visible .ant-select-item-option', { hasText: label })
    .click();
  // 等下拉選單關閉，避免下一次選擇時點到還在關閉動畫中的選單
  await expect(page.locator('.ant-select-dropdown:visible')).toHaveCount(0);
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

test('admin designs a layout: add section, drag fields, columns and span, save, reset', async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  const { saves } = await mockServer(page);
  await page.goto('/#/schema/repair');
  await page.getByRole('button', { name: /表單版面/ }).click();
  await expect(page.getByText('表單版面：維修單')).toBeVisible();

  const main = page.getByRole('region', { name: '區塊：區塊 1' });
  expect(await itemsIn(main)).toEqual([
    'block:record_no',
    'field:name',
    'field:phone',
    'field:address',
    'field:note',
    'block:lines:parts',
  ]);
  // 系統區塊不能移除
  await expect(main.getByRole('button', { name: '移除「單號」' })).toHaveCount(0);

  // 新增區塊並命名
  await page.getByRole('button', { name: /新增區塊/ }).click();
  const added = page.getByRole('region', { name: '區塊：新區塊' });
  await added.getByLabel('區塊標題').fill('聯絡資訊');
  const contact = page.getByRole('region', { name: '區塊：聯絡資訊' });

  // 拖曳電話、地址到新區塊
  await dragTo(
    page,
    main.getByRole('button', { name: '拖曳「電話」' }),
    contact.getByText('把欄位拖曳到這裡'),
  );
  await expect.poll(() => itemsIn(contact)).toEqual(['field:phone']);
  await dragTo(
    page,
    main.getByRole('button', { name: '拖曳「地址」' }),
    contact.locator('[data-design-item="field:phone"]'),
  );
  await expect
    .poll(async () => (await itemsIn(contact)).sort())
    .toEqual(['field:address', 'field:phone']);

  // 鍵盤拖曳：備註往上移一格
  const noteHandle = main.getByRole('button', { name: '拖曳「備註」' });
  await noteHandle.focus();
  // dnd-kit 在拿起之後的下一個畫面才開始處理方向鍵，每一步之間稍等
  await page.keyboard.press('Space');
  await page.waitForTimeout(200);
  await page.keyboard.press('ArrowUp');
  await page.waitForTimeout(200);
  await page.keyboard.press('Space');
  await expect
    .poll(() => itemsIn(main))
    .toEqual(['block:record_no', 'field:note', 'field:name', 'block:lines:parts']);

  // 欄數 3、地址寬 2 欄
  await contact.getByRole('combobox', { name: '欄數' }).click({ force: true });
  await pickOption(page, '3 欄');
  await contact.getByRole('combobox', { name: '「地址」的寬度' }).click({ force: true });
  await pickOption(page, '2 欄');

  // 預覽
  await page.getByText('預覽', { exact: true }).click();
  const preview = page.getByRole('region', { name: '表單預覽' });
  await expect(preview.getByRole('region', { name: '聯絡資訊' })).toBeVisible();
  await page.screenshot({ path: test.info().outputPath('preview.png') });
  await page.getByText('設計', { exact: true }).click();

  await page.getByRole('button', { name: /儲存/ }).click();
  await expect(page.getByText('已儲存表單版面')).toBeVisible();
  expect(saves).toHaveLength(1);
  expect(saves[0]).toMatchObject({ version: null });
  await page.screenshot({ path: test.info().outputPath('designer.png') });

  // 重新開啟表單：依新版面顯示
  await page.goto('/#/entities/repair/new');
  const section = page.getByRole('region', { name: '聯絡資訊' });
  await expect(section.getByLabel('電話')).toBeVisible();
  const [phone, address] = [await itemBox(page, '電話'), await itemBox(page, '地址')];
  expect(Math.abs(phone.y - address.y)).toBeLessThanOrEqual(1);
  expect(address.width).toBeGreaterThan(phone.width * 1.8);
  await expect((await itemBox(page, '備註')).y).toBeLessThan((await itemBox(page, '客戶名稱')).y);
  await page.screenshot({ path: test.info().outputPath('form.png') });

  // 還原成預設：表單回到單欄
  await page.goto('/#/schema/repair/form-layout');
  await page.getByRole('button', { name: /還原成預設/ }).click();
  // antd 在兩個中文字的按鈕文字中間加空白
  await page
    .locator('.ant-popconfirm')
    .getByRole('button', { name: /^還\s*原$/ })
    .click();
  await expect(page.getByText('已還原成預設版面')).toBeVisible();
  await page.goto('/#/entities/repair/new');
  await expect(page.getByLabel('電話')).toBeVisible();
  await expect(page.getByRole('region', { name: '聯絡資訊' })).toHaveCount(0);
  const [name, phone2] = [await itemBox(page, '客戶名稱'), await itemBox(page, '電話')];
  expect(phone2.y).toBeGreaterThan(name.y);
  expect(Math.abs(phone2.x - name.x)).toBeLessThanOrEqual(1);
});

test('version conflict shows a reload prompt and keeps the edits', async ({ page }) => {
  await mockServer(page, { conflict: true });
  await page.goto('/#/schema/repair/form-layout');
  await page.getByRole('button', { name: /新增區塊/ }).click();
  await page.getByRole('button', { name: /儲存/ }).click();
  await expect(page.getByText('資料已被其他人修改')).toBeVisible();
  await expect(page.getByRole('button', { name: /重新載入/ })).toBeVisible();
  await expect(page.getByRole('region', { name: '區塊：新區塊' })).toBeVisible();
  await expect(page.getByText('未存檔', { exact: true })).toBeVisible();
});

test('editor cannot open the designer', async ({ page }) => {
  await mockServer(page, { role: 'editor' });
  await page.goto('/#/schema/repair/form-layout');
  await expect(page.getByText('沒有權限')).toBeVisible();
  await expect(page.getByRole('menuitem', { name: '資料類別' })).toHaveCount(0);
});

// 預覽的呈現方式（#252）：非分割檢視的資料類別沒有寬度切換
test('preview switches between form and read only, no width switch for other entities', async ({
  page,
}) => {
  await mockServer(page);
  await page.goto('/#/schema/repair/form-layout');
  await expect(page.getByTestId('layout-applies-to')).toHaveText(
    '這份版面套用在：新增、編輯與檢視的表單。',
  );
  await page.getByText('預覽', { exact: true }).click();
  const preview = page.getByRole('region', { name: '表單預覽' });
  await expect(preview.locator('[data-item="name"] input')).toBeDisabled();
  await expect(page.getByRole('radiogroup', { name: '預覽寬度' })).toHaveCount(0);
  await expect(page.getByText('分割檢視右側', { exact: true })).toHaveCount(0);

  await page.getByText('唯讀預覽', { exact: true }).click();
  await expect(preview.getByRole('textbox')).toHaveCount(0);
  await expect(preview.locator('[data-item="name"]')).toContainText('客戶名稱範例');
  await expect(preview.locator('[data-item="record_no"]')).toContainText('單號範例');
  await expect(preview.getByText('明細：零件')).toBeVisible();
});
