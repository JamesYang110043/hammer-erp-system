import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { App } from '../../App';
import type {
  Currency,
  DefaultViewLayout_Serialize as DefaultViewLayout,
  EntityDefinition,
  FieldDefinition,
  Page,
  Record as DataRecord,
} from '../../bindings';
import { ADMIN, startupCommands } from '../../test/fixtures';
import { commandError, mockCommands } from '../../test/ipc';
import type { CommandHandler } from '../../test/ipc';
import { navSectionItems, openNavSection } from '../../test/nav';
import { renderWithProviders } from '../../test/render';
import { REPAIR, fieldDef } from '../../test/schemaFixtures';

const PAGE_LOAD_TIMEOUT = 5000;

/** 系統資料類別（docs/architecture.md §8.6）。 */
function systemEntity(
  key: string,
  label: string,
  sortOrder: number,
  patch: Partial<EntityDefinition> = {},
): EntityDefinition {
  return {
    ...REPAIR,
    id: `entity-${key}`,
    key,
    label,
    icon: null,
    is_system: true,
    storage: 'table',
    sort_order: sortOrder,
    ...patch,
  };
}

const PRODUCT = systemEntity('product', '商品', 10);
const CATEGORY = systemEntity('product_category', '商品分類', 20);
const UNIT = systemEntity('unit', '單位', 30);
const CUSTOMER = systemEntity('customer', '客戶', 40);
const CONTACT = systemEntity('customer_contact', '客戶聯絡人', 0, {
  kind: 'detail',
  parent_entity_id: CUSTOMER.id,
});
const SUPPLIER = systemEntity('supplier', '廠商', 50);
const ENTITIES = [SUPPLIER, CUSTOMER, CONTACT, UNIT, CATEGORY, PRODUCT, REPAIR];

/** 系統欄位。 */
function sys(entity: EntityDefinition, patch: Partial<FieldDefinition>): FieldDefinition {
  return fieldDef({ entity_id: entity.id, is_system: true, storage: 'column', ...patch });
}

const FIELDS: Record<string, FieldDefinition[]> = {
  product: [
    sys(PRODUCT, { field_key: 'code', label: '料號', is_required: true, sort_order: 10 }),
    sys(PRODUCT, { field_key: 'name', label: '品名', is_required: true, sort_order: 20 }),
    sys(PRODUCT, {
      field_key: 'category_id',
      label: '分類',
      data_type: 'relation',
      relation_entity_id: CATEGORY.id,
      sort_order: 30,
    }),
    sys(PRODUCT, {
      field_key: 'unit_id',
      label: '單位',
      data_type: 'relation',
      relation_entity_id: UNIT.id,
      is_required: true,
      sort_order: 40,
    }),
  ],
  product_category: [
    sys(CATEGORY, { field_key: 'name', label: '名稱', is_required: true, sort_order: 10 }),
  ],
  unit: [
    sys(UNIT, { field_key: 'code', label: '代碼', is_required: true, sort_order: 10 }),
    sys(UNIT, { field_key: 'name', label: '名稱', is_required: true, sort_order: 20 }),
  ],
  customer: [
    sys(CUSTOMER, { field_key: 'code', label: '客戶編號', is_required: true, sort_order: 10 }),
    sys(CUSTOMER, { field_key: 'name', label: '公司名稱', is_required: true, sort_order: 20 }),
  ],
  customer_contact: [
    sys(CONTACT, { field_key: 'name', label: '姓名', is_required: true, sort_order: 10 }),
    sys(CONTACT, {
      field_key: 'is_primary',
      label: '主要聯絡人',
      data_type: 'boolean',
      is_required: true,
      default_value: false,
      sort_order: 60,
    }),
  ],
  supplier: [
    sys(SUPPLIER, { field_key: 'code', label: '廠商編號', is_required: true, sort_order: 10 }),
    sys(SUPPLIER, { field_key: 'name', label: '公司名稱', is_required: true, sort_order: 20 }),
    sys(SUPPLIER, {
      field_key: 'default_currency_code',
      label: '預設幣別',
      is_required: true,
      default_value: 'TWD',
      ui_widget: 'currency',
      sort_order: 130,
    }),
  ],
};

const ID = '0196f5a0-0000-7000-8000-000000000201';
const UNIT_PCS = '0196f5a0-0000-7000-8000-000000000301';
const CAT_ELEC = '0196f5a0-0000-7000-8000-000000000401';
const CAT_PHONE = '0196f5a0-0000-7000-8000-000000000402';
const CAT_FOOD = '0196f5a0-0000-7000-8000-000000000403';
const CONTACT_A = '0196f5a0-0000-7000-8000-000000000501';
const CONTACT_B = '0196f5a0-0000-7000-8000-000000000502';

function record(data: Record<string, unknown>, patch: Partial<DataRecord> = {}): DataRecord {
  return {
    id: ID,
    record_no: null,
    title: String(data.name ?? ''),
    data,
    created_at: '2026-11-17T01:00:00Z',
    created_by: null,
    updated_at: '2026-11-17T01:00:00Z',
    updated_by: null,
    version: 3,
    ...patch,
  };
}

function page(items: DataRecord[]): Page<DataRecord> {
  return { items, total: items.length, page: 1, page_size: 50 };
}

const CATEGORIES = [
  record({ name: '手機', parent_id: CAT_ELEC, sort_order: 0 }, { id: CAT_PHONE }),
  record({ name: '電子', parent_id: null, sort_order: 0 }, { id: CAT_ELEC }),
  record({ name: '食品', parent_id: null, sort_order: 1 }, { id: CAT_FOOD }),
];

function currency(code: string, name: string, isActive = true): Currency {
  return {
    id: `currency-${code}`,
    code,
    name,
    symbol: null,
    decimal_places: 2,
    is_base: code === 'TWD',
    is_active: isActive,
    created_at: '2026-11-17T01:00:00Z',
    updated_at: '2026-11-17T01:00:00Z',
    version: 1,
  };
}

function generatedLayout(key: string): DefaultViewLayout {
  return {
    source: 'generated',
    layout: null,
    config: {
      columns: (FIELDS[key] ?? []).map((f) => ({ field_key: f.field_key, width: null })),
      sort: null,
      filters: [],
    },
  };
}

function renderAt(hash: string, extra: Record<string, CommandHandler> = {}) {
  window.location.hash = hash;
  const invoke = mockCommands({
    ...startupCommands(ADMIN),
    list_entities: () => ENTITIES,
    get_entity: ({ key }) => ({
      entity: ENTITIES.find((e) => e.key === key),
      fields: FIELDS[key as string] ?? [],
    }),
    get_default_layout: ({ key }) => generatedLayout(key as string),
    list_layouts: () => [],
    list_records: ({ key }) => page(key === 'product_category' ? CATEGORIES : []),
    lookup_records: () => [{ id: UNIT_PCS, title: '個' }],
    list_currencies: () => [currency('TWD', '新台幣'), currency('USD', '美元')],
    ...extra,
  });
  renderWithProviders(<App />);
  return invoke;
}

function calls(invoke: ReturnType<typeof mockCommands>, cmd: string) {
  return invoke.mock.calls.filter(([c]) => c === cmd).map(([, args]) => args);
}

/** antd Select 的輸入框（`aria-label` 也會出現在下拉清單上）。 */
function combobox(label: string): HTMLElement {
  return screen.getAllByLabelText(label).find((el) => el.getAttribute('role') === 'combobox')!;
}

/** 欄位的 Form.Item。 */
function formItem(label: string) {
  const el = document.querySelector<HTMLElement>(`label[title="${label}"]`);
  if (!el) {
    throw new Error(`找不到欄位「${label}」`);
  }
  return within(el.closest<HTMLElement>('.ant-form-item')!);
}

/** 打開中的下拉選單選項文字。 */
function optionTexts(): string[] {
  return Array.from(document.querySelectorAll('.ant-select-item-option-content')).map(
    (el) => el.textContent ?? '',
  );
}

function save(): HTMLElement {
  return screen.getByRole('button', { name: /儲\s*存/ });
}

describe('基本資料選單', () => {
  it('sidebar_lists_system_masters_under_master_data', async () => {
    const invoke = renderAt('#/connection');
    const user = userEvent.setup();
    const menu = await screen.findByRole(
      'navigation',
      { name: '主選單' },
      { timeout: PAGE_LOAD_TIMEOUT },
    );
    // 「基本資料」是選單的第一個區塊
    await within(menu).findByRole('menuitem', { name: '基本資料' });
    expect(within(menu).getAllByRole('menuitem')[0]).toHaveTextContent('基本資料');
    await openNavSection(user, '基本資料');

    const items = navSectionItems('基本資料');
    // 依選單排序；明細子表（聯絡人）不列出
    expect(items.slice(0, 5)).toEqual(['商品', '商品分類', '單位', '客戶', '廠商']);
    expect(items).not.toContain('客戶聯絡人');
    // 自訂資料類別仍在原本的分組
    expect(items).not.toContain('維修單');
    await openNavSection(user, '主檔');
    expect(navSectionItems('主檔')).toContain('維修單');

    await user.click(within(menu).getByRole('menuitem', { name: /商品$/ }));
    expect(window.location.hash).toBe('#/entities/product');
    await waitFor(() =>
      expect(calls(invoke, 'list_records')).toContainEqual(
        expect.objectContaining({ key: 'product' }),
      ),
    );
    expect(await screen.findByText('料號', { selector: 'thead th *' })).toBeInTheDocument();
  });
});

describe('商品表單', () => {
  it('creates_product_with_unit_lookup_and_category_path', async () => {
    const invoke = renderAt('#/entities/product/new', {
      create_record: () => record({ name: '手機殼' }, { version: 1 }),
    });
    const user = userEvent.setup();
    await user.type(
      await screen.findByRole('textbox', { name: '料號' }, { timeout: PAGE_LOAD_TIMEOUT }),
      'A001',
    );
    await user.type(screen.getByRole('textbox', { name: '品名' }), '手機殼');

    // 分類：顯示階層路徑，依樹狀順序
    await user.click(combobox('分類'));
    await screen.findByTitle('電子 / 手機');
    expect(optionTexts()).toEqual(['電子', '電子 / 手機', '食品']);
    await user.type(combobox('分類'), '手機');
    await user.click(await screen.findByTitle('電子 / 手機'));
    expect(formItem('分類').getByTitle('電子 / 手機')).toHaveClass('ant-select-selection-item');
    expect(calls(invoke, 'list_records')).toContainEqual({
      key: 'product_category',
      query: { page: 1, page_size: 200, sort: null, filters: [] },
    });

    // 單位：關聯選擇器
    await user.click(combobox('單位'));
    await user.click(await screen.findByTitle('個'));
    await waitFor(() =>
      expect(calls(invoke, 'lookup_records')).toContainEqual({
        key: 'unit',
        query: { q: null, limit: 20 },
      }),
    );

    await user.click(save());
    await waitFor(() => expect(calls(invoke, 'create_record')).toHaveLength(1));
    expect(calls(invoke, 'create_record')[0]).toEqual({
      key: 'product',
      req: {
        data: { code: 'A001', name: '手機殼', category_id: CAT_PHONE, unit_id: UNIT_PCS },
      },
    });
  });

  it('shows_current_category_path_when_editing', async () => {
    renderAt(`#/entities/product/${ID}`, {
      get_record: () =>
        record(
          { code: 'A001', name: '手機殼', category_id: CAT_PHONE, unit_id: UNIT_PCS },
          {
            relations: {
              category_id: { id: CAT_PHONE, title: '手機', deleted: false },
              unit_id: { id: UNIT_PCS, title: '個', deleted: false },
            },
          },
        ),
    });
    await screen.findByRole('textbox', { name: '料號' }, { timeout: PAGE_LOAD_TIMEOUT });
    expect(await formItem('分類').findByTitle('電子 / 手機')).toHaveClass(
      'ant-select-selection-item',
    );
    expect(formItem('單位').getByTitle('個')).toHaveClass('ant-select-selection-item');
  });
});

describe('刪除基本資料', () => {
  it('unit_in_use_shows_server_message', async () => {
    const invoke = renderAt('#/entities/unit', {
      list_records: () => page([record({ code: 'pcs', name: '個' })]),
      delete_record: () =>
        commandError({
          kind: 'api',
          code: 'VALIDATION',
          message: '此單位還有 3 筆商品使用中，無法刪除',
          details: { fields: [], in_use_count: 3 },
        }),
    });
    const user = userEvent.setup();
    const cell = await screen.findByText('pcs', { selector: 'td' }, { timeout: PAGE_LOAD_TIMEOUT });
    await user.click(within(cell.closest('tr')!).getByRole('button', { name: '刪除' }));
    await screen.findByText('確定要刪除這筆資料？');
    const popup = document.querySelector<HTMLElement>('.ant-popconfirm')!;
    await user.click(within(popup).getByRole('button', { name: /刪\s*除/ }));

    expect(await screen.findByText('此單位還有 3 筆商品使用中，無法刪除')).toBeInTheDocument();
    expect(calls(invoke, 'delete_record')).toEqual([{ key: 'unit', id: ID, version: 3 }]);
  });
});

describe('客戶聯絡人', () => {
  function contact(id: string, name: string, isPrimary: boolean): DataRecord {
    return record({ name, is_primary: isPrimary }, { id, version: 1 });
  }

  function customer(): DataRecord {
    return record(
      { code: 'C001', name: '大華公司' },
      {
        lines: {
          customer_contact: [
            contact(CONTACT_A, '王小明', true),
            contact(CONTACT_B, '李小華', false),
          ],
        },
      },
    );
  }

  /** 聯絡人表格的資料列。 */
  function rows(): HTMLElement[] {
    const section = screen.getByRole('region', { name: '客戶聯絡人' });
    return Array.from(section.querySelectorAll<HTMLElement>('tbody tr.ant-table-row'));
  }

  function primary(row: number): HTMLElement {
    return within(rows()[row]!).getByRole('radio');
  }

  it('primary_contact_is_single_choice_and_saves_with_customer', async () => {
    const invoke = renderAt(`#/entities/customer/${ID}`, {
      get_record: customer,
      update_record: () => customer(),
    });
    const user = userEvent.setup();
    await screen.findByRole('textbox', { name: '公司名稱' }, { timeout: PAGE_LOAD_TIMEOUT });

    expect(primary(0)).toBeChecked();
    expect(primary(1)).not.toBeChecked();

    // 選第 2 位，第 1 位自動取消
    await user.click(primary(1));
    expect(primary(0)).not.toBeChecked();
    expect(primary(1)).toBeChecked();

    // 同時新增一位聯絡人、修改客戶名稱
    await user.click(screen.getByRole('button', { name: /新增客戶聯絡人/ }));
    await user.type(within(rows()[2]!).getByLabelText('姓名'), '陳大文');
    expect(primary(2)).not.toBeChecked();
    const name = screen.getByRole('textbox', { name: '公司名稱' });
    await user.clear(name);
    await user.type(name, '大華實業');
    await user.click(save());

    await waitFor(() => expect(calls(invoke, 'update_record')).toHaveLength(1));
    expect(calls(invoke, 'update_record')[0]).toEqual({
      key: 'customer',
      id: ID,
      req: {
        version: 3,
        data: { name: '大華實業' },
        lines: {
          customer_contact: [
            { id: CONTACT_A, data: { is_primary: false } },
            { id: CONTACT_B, data: { is_primary: true } },
            { data: { name: '陳大文', is_primary: false } },
          ],
        },
      },
    });
  });

  it('clicking_checked_primary_clears_it', async () => {
    renderAt(`#/entities/customer/${ID}`, { get_record: customer });
    const user = userEvent.setup();
    await screen.findByRole('textbox', { name: '公司名稱' }, { timeout: PAGE_LOAD_TIMEOUT });

    await user.click(primary(0));
    expect(primary(0)).not.toBeChecked();
    expect(primary(1)).not.toBeChecked();
    expect(screen.getByText('未存檔')).toBeInTheDocument();
  });
});

describe('廠商預設幣別', () => {
  it('currency_select_lists_active_currencies_only', async () => {
    const invoke = renderAt(`#/entities/supplier/${ID}`, {
      get_record: () => record({ code: 'S001', name: '東京商事', default_currency_code: 'JPY' }),
      update_record: () => record({ code: 'S001', name: '東京商事' }, { version: 4 }),
    });
    const user = userEvent.setup();
    await screen.findByRole('textbox', { name: '公司名稱' }, { timeout: PAGE_LOAD_TIMEOUT });

    // 已停用的幣別仍顯示原值
    expect(await formItem('預設幣別').findByTitle('JPY（已停用）')).toHaveClass(
      'ant-select-selection-item',
    );
    expect(calls(invoke, 'list_currencies')).toEqual([{ includeInactive: false }]);

    await user.click(combobox('預設幣別'));
    await screen.findByTitle('USD 美元');
    expect(optionTexts()).toEqual(['JPY（已停用）', 'TWD 新台幣', 'USD 美元']);
    await user.click(screen.getByTitle('USD 美元'));
    await user.click(save());

    await waitFor(() => expect(calls(invoke, 'update_record')).toHaveLength(1));
    expect(calls(invoke, 'update_record')[0]).toEqual({
      key: 'supplier',
      id: ID,
      req: { version: 3, data: { default_currency_code: 'USD' } },
    });
  });

  it('new_supplier_defaults_to_twd', async () => {
    renderAt('#/entities/supplier/new');
    await screen.findByRole('textbox', { name: '公司名稱' }, { timeout: PAGE_LOAD_TIMEOUT });
    expect(await formItem('預設幣別').findByTitle('TWD 新台幣')).toHaveClass(
      'ant-select-selection-item',
    );
  });
});
