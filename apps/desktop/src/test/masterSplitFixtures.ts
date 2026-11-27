import type {
  EntityDefinition,
  FormLayout_Serialize as FormLayout,
  FieldDefinition,
  Page,
  Record as DataRecord,
  RecordListQuery_Deserialize as RecordListQuery,
} from '../bindings';
import { REPAIR, fieldDef } from './schemaFixtures';

/** 分割檢視（§8.7）測試用的客戶、廠商資料類別與資料。單元測試與 E2E 共用。 */

function systemEntity(key: string, label: string, sortOrder: number): EntityDefinition {
  return {
    ...REPAIR,
    id: `entity-${key}`,
    key,
    label,
    icon: null,
    is_system: true,
    storage: 'table',
    title_template: '{code} {name}',
    sort_order: sortOrder,
  };
}

export const CUSTOMER = systemEntity('customer', '客戶', 40);
export const SUPPLIER = systemEntity('supplier', '廠商', 50);
export const PRODUCT = systemEntity('product', '商品', 10);
/** 客戶聯絡人（明細子表）。 */
export const CUSTOMER_CONTACT: EntityDefinition = {
  ...systemEntity('customer_contact', '客戶聯絡人', 0),
  kind: 'detail',
  parent_entity_id: CUSTOMER.id,
  title_template: null,
};

/** 客戶品名（明細子表，M11）：排在聯絡人之後（sort_order 10）。 */
export const CUSTOMER_PRODUCT_NAME: EntityDefinition = {
  ...systemEntity('customer_product_name', '客戶品名', 10),
  kind: 'detail',
  parent_entity_id: CUSTOMER.id,
  title_template: '{product_id.code} {name}',
};

function sys(entity: EntityDefinition, patch: Partial<FieldDefinition>): FieldDefinition {
  return fieldDef({
    id: `f-${entity.key}-${patch.field_key}`,
    entity_id: entity.id,
    is_system: true,
    storage: 'column',
    ...patch,
  });
}

/** 客戶、廠商共同的欄位（§8.7，只列分割檢視用到的）。 */
function partyFields(entity: EntityDefinition, codeLabel: string): FieldDefinition[] {
  return [
    sys(entity, { field_key: 'code', label: codeLabel, is_required: true, sort_order: 10 }),
    sys(entity, { field_key: 'name', label: '公司名稱', is_required: true, sort_order: 20 }),
    sys(entity, { field_key: 'short_name', label: '簡稱', sort_order: 30 }),
    sys(entity, { field_key: 'tax_id', label: '統一編號', sort_order: 40 }),
    sys(entity, { field_key: 'phone', label: '電話', sort_order: 50 }),
    sys(entity, {
      field_key: 'is_active',
      label: '啟用',
      data_type: 'boolean',
      is_required: true,
      default_value: true,
      sort_order: 130,
    }),
  ];
}

export const CUSTOMER_FIELDS = partyFields(CUSTOMER, '客戶編號');
export const SUPPLIER_FIELDS = partyFields(SUPPLIER, '廠商編號');
export const CUSTOMER_CONTACT_FIELDS: FieldDefinition[] = [
  sys(CUSTOMER_CONTACT, { field_key: 'name', label: '姓名', is_required: true, sort_order: 10 }),
  sys(CUSTOMER_CONTACT, { field_key: 'title', label: '職稱', sort_order: 20 }),
  sys(CUSTOMER_CONTACT, { field_key: 'phone', label: '電話', sort_order: 30 }),
  sys(CUSTOMER_CONTACT, {
    field_key: 'is_primary',
    label: '主要聯絡人',
    data_type: 'boolean',
    is_required: true,
    default_value: false,
    sort_order: 60,
  }),
];

export const CUSTOMER_PRODUCT_NAME_FIELDS: FieldDefinition[] = [
  sys(CUSTOMER_PRODUCT_NAME, {
    field_key: 'product_id',
    label: '商品',
    data_type: 'relation',
    relation_entity_id: PRODUCT.id,
    is_required: true,
    sort_order: 10,
  }),
  sys(CUSTOMER_PRODUCT_NAME, {
    field_key: 'name',
    label: '客戶品名',
    is_required: true,
    is_searchable: true,
    validation: { max_length: 200 },
    sort_order: 20,
  }),
  sys(CUSTOMER_PRODUCT_NAME, {
    field_key: 'notes',
    label: '備註',
    data_type: 'long_text',
    sort_order: 30,
  }),
];

/** 商品（客戶品名的選擇器使用）。 */
export const PRODUCT_P001 = '0196f5a0-0000-7000-8000-000000000b01';
export const PRODUCT_P002 = '0196f5a0-0000-7000-8000-000000000b02';
export const PRODUCT_TITLES: Record<string, string> = {
  [PRODUCT_P001]: 'P001 扳手 10mm',
  [PRODUCT_P002]: 'P002 鉗子',
};
export const CUSTOMER_PRODUCT_NAME_A = '0196f5a0-0000-7000-8000-000000000c01';

export const PRODUCT_FIELDS = [
  sys(PRODUCT, { field_key: 'code', label: '料號', is_required: true, sort_order: 10 }),
  sys(PRODUCT, { field_key: 'name', label: '品名', is_required: true, sort_order: 20 }),
];

/** 第 `n` 筆客戶的 id（`n` 從 1 開始）。 */
export const customerId = (n: number) =>
  `0196f5a0-0000-7000-8000-${String(700000000000 + n).padStart(12, '0')}`;

export function partyRecord(
  id: string,
  data: Record<string, unknown>,
  patch: Partial<DataRecord> = {},
): DataRecord {
  return {
    id,
    record_no: null,
    title: `${String(data.code ?? '')} ${String(data.name ?? '')}`.trim(),
    data: { is_active: true, ...data },
    created_at: '2026-11-24T01:00:00Z',
    created_by: null,
    updated_at: '2026-11-24T01:00:00Z',
    updated_by: null,
    version: 1,
    ...patch,
  };
}

/** 固定的幾筆客戶：含停用、只有簡稱或統編符合搜尋的資料。 */
export const NAMED_CUSTOMERS: DataRecord[] = [
  partyRecord(customerId(1), {
    code: 'C001',
    name: '大華實業股份有限公司',
    short_name: '大華',
    tax_id: '12345675',
  }),
  partyRecord(customerId(2), {
    code: 'C002',
    name: '光明電子有限公司',
    short_name: '光明',
    tax_id: '87654321',
  }),
  partyRecord(customerId(3), {
    code: 'C003',
    name: '東昇貿易',
    short_name: '阿昇',
    tax_id: null,
    is_active: false,
  }),
  partyRecord(customerId(4), {
    code: 'C004',
    name: 'Global Trading Co.',
    short_name: null,
    tax_id: null,
  }),
];

/** `NAMED_CUSTOMERS` 之後補上 `count` 筆一般客戶（編號 C101 起），用來測試捲動載入。 */
export function manyCustomers(count: number): DataRecord[] {
  const extra = Array.from({ length: count }, (_, i) =>
    partyRecord(customerId(100 + i), {
      code: `C${String(101 + i).padStart(3, '0')}`,
      name: `測試客戶 ${i + 1}`,
      short_name: null,
      tax_id: String(50000000 + i),
    }),
  );
  return [...NAMED_CUSTOMERS, ...extra];
}

/**
 * 模擬伺服器的 `list_records`：支援 `eq`、`contains` 篩選（AND）、依欄位排序與分頁。
 * 只處理分割檢視用到的條件。
 */
export function fakeListRecords(
  all: readonly DataRecord[],
  query: RecordListQuery,
): Page<DataRecord> {
  let items = all.filter((r) =>
    (query.filters ?? []).every((f) => {
      const v = r.data[f.field_key];
      if (f.op === 'eq') {
        return String(v) === f.value;
      }
      if (f.op === 'contains') {
        return typeof v === 'string' && v.toLowerCase().includes(f.value.toLowerCase());
      }
      return true;
    }),
  );
  const sort = query.sort;
  if (sort) {
    items = [...items].sort((a, b) => {
      const av = String(a.data[sort.field_key] ?? '');
      const bv = String(b.data[sort.field_key] ?? '');
      const c = av < bv ? -1 : av > bv ? 1 : 0;
      return sort.descending ? -c : c;
    });
  }
  const page = query.page ?? 1;
  const size = query.page_size ?? 50;
  return {
    items: items.slice((page - 1) * size, page * size),
    total: items.length,
    page,
    page_size: size,
  };
}

/**
 * 客戶的表單版面：「基本資料」2 欄（名稱在編號前面，統編佔 2 欄）、可收折的「聯絡資訊」，
 * 最後是聯絡人明細表。順序與欄位的 `sort_order` 不同，用來確認預覽依版面排列。
 */
export const CUSTOMER_FORM_LAYOUT: FormLayout = {
  id: '0196f5a0-0000-7000-8000-000000000901',
  version: 1,
  is_default: false,
  config: {
    sections: [
      {
        key: 'basic',
        title: '基本資料',
        columns: 2,
        collapsible: false,
        collapsed: false,
        items: [
          { type: 'field', field: 'name', span: 1 },
          { type: 'field', field: 'code', span: 1 },
          { type: 'field', field: 'tax_id', span: 2 },
          { type: 'field', field: 'short_name', span: 1 },
          { type: 'field', field: 'is_active', span: 1 },
        ],
      },
      {
        key: 'contact',
        title: '聯絡資訊',
        columns: 1,
        collapsible: true,
        collapsed: false,
        items: [{ type: 'field', field: 'phone', span: 1 }],
      },
      {
        key: '_blocks',
        title: '',
        columns: 1,
        collapsible: false,
        collapsed: false,
        items: [{ type: 'block', block: 'lines:customer_contact' }],
      },
    ],
  },
};

export const CONTACT_A = '0196f5a0-0000-7000-8000-000000000a01';
export const CONTACT_B = '0196f5a0-0000-7000-8000-000000000a02';

/** 單筆查詢回傳的客戶（含聯絡人明細）。沒有這筆時回傳 `undefined`。 */
export function customerDetail(all: readonly DataRecord[], id: string): DataRecord | undefined {
  const r = all.find((x) => x.id === id);
  if (!r) {
    return undefined;
  }
  return {
    ...r,
    data: { phone: '02-2345-6789', ...r.data },
    lines: {
      customer_contact:
        r.id === customerId(1)
          ? [
              partyRecord(CONTACT_A, {
                name: '王小明',
                title: '採購經理',
                phone: '0912-345-678',
                is_primary: true,
              }),
              partyRecord(CONTACT_B, {
                name: '李小華',
                title: null,
                phone: null,
                is_primary: false,
              }),
            ].map((c) => ({ ...c, title: '', data: { ...c.data, is_active: undefined } }))
          : [],
      customer_product_name:
        r.id === customerId(1)
          ? [
              {
                ...partyRecord(CUSTOMER_PRODUCT_NAME_A, {
                  product_id: PRODUCT_P001,
                  name: 'X 型扳手',
                  notes: '客戶圖面編號 W-10',
                }),
                title: 'P001 X 型扳手',
                relations: {
                  product_id: {
                    id: PRODUCT_P001,
                    title: PRODUCT_TITLES[PRODUCT_P001]!,
                    deleted: false,
                    inactive: false,
                  },
                },
              },
            ].map((c) => ({ ...c, data: { ...c.data, is_active: undefined } }))
          : [],
    },
  };
}
