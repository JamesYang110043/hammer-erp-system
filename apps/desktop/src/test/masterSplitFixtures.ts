import type {
  EntityDefinition,
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
