import type { EntityDefinition, FieldDefinition, Record as DataRecord } from '../bindings';
import { PRODUCT, STOCK_ADJUSTMENT } from './documentFixtures';
import { fieldDef } from './schemaFixtures';

/**
 * 進貨模組的資料類別與欄位定義（crates/db/migrations/0012_purchasing.sql）。
 */

const documentEntity = (patch: Partial<EntityDefinition>): EntityDefinition => ({
  ...STOCK_ADJUSTMENT,
  ...patch,
});

/** 廠商（系統主檔）。 */
export const SUPPLIER: EntityDefinition = documentEntity({
  id: '01a1e516-3406-71fc-8cf2-9ce47caf0e68',
  key: 'supplier',
  label: '廠商',
  kind: 'master',
  numbering_pattern: null,
  title_template: '{code} {name}',
  sort_order: 50,
});

export const PURCHASE_ORDER: EntityDefinition = documentEntity({
  id: '01a1f4c0-5e10-7000-961c-f268733a2a58',
  key: 'purchase_order',
  label: '採購單',
  numbering_pattern: 'PO{yyyyMM}-{0000}',
  title_template: '{record_no} {supplier_id.name}',
  sort_order: 110,
});

export const PURCHASE_ORDER_LINE: EntityDefinition = documentEntity({
  id: '01a1f4c0-5e10-7001-a354-dccb85b65acf',
  key: 'purchase_order_line',
  label: '採購明細',
  kind: 'detail',
  parent_entity_id: PURCHASE_ORDER.id,
  numbering_pattern: null,
  title_template: '{product_id.code} {product_id.name}',
  sort_order: 0,
});

export const GOODS_RECEIPT: EntityDefinition = documentEntity({
  id: '01a1f4c0-5e10-7002-a3f6-25702f618e4d',
  key: 'goods_receipt',
  label: '進貨單',
  numbering_pattern: 'GR{yyyyMM}-{0000}',
  title_template: '{record_no} {supplier_id.name}',
  sort_order: 120,
});

export const GOODS_RECEIPT_LINE: EntityDefinition = documentEntity({
  id: '01a1f4c0-5e10-7003-b2b2-bf5f574e1236',
  key: 'goods_receipt_line',
  label: '進貨明細',
  kind: 'detail',
  parent_entity_id: GOODS_RECEIPT.id,
  numbering_pattern: null,
  title_template: '{product_id.code} {product_id.name} {lot_no}',
  sort_order: 0,
});

const TAX_OPTIONS = [
  { value: 'taxable', label: '應稅', color: null },
  { value: 'zero_rated', label: '零稅率', color: null },
  { value: 'exempt', label: '免稅', color: null },
];

const MIN_ZERO = { min: '0', max: null, max_length: null, pattern: null };

/** 系統欄位。 */
function sys(entity: EntityDefinition, patch: Partial<FieldDefinition>): FieldDefinition {
  return fieldDef({
    entity_id: entity.id,
    is_system: true,
    storage: 'column',
    id: `${entity.key}-${patch.field_key}`,
    ...patch,
  });
}

/** 唯讀的金額欄位。 */
function money(entity: EntityDefinition, key: string, label: string, sort: number, dp = 4) {
  return sys(entity, {
    field_key: key,
    label,
    data_type: 'money',
    is_readonly: true,
    decimal_places: dp,
    sort_order: sort,
  });
}

function pricedHeader(
  entity: EntityDefinition,
  dateKey: string,
  dateLabel: string,
  statuses: { value: string; label: string }[],
  extra: FieldDefinition[],
): FieldDefinition[] {
  return [
    sys(entity, {
      field_key: 'supplier_id',
      label: '廠商',
      data_type: 'relation',
      is_required: true,
      relation_entity_id: SUPPLIER.id,
      sort_order: 10,
    }),
    ...extra,
    sys(entity, {
      field_key: dateKey,
      label: dateLabel,
      data_type: 'date',
      is_required: true,
      sort_order: 30,
    }),
    sys(entity, {
      field_key: 'currency_code',
      label: '幣別',
      is_required: true,
      ui_widget: 'currency',
      sort_order: 40,
    }),
    sys(entity, {
      field_key: 'exchange_rate',
      label: '匯率',
      data_type: 'decimal',
      decimal_places: 6,
      validation: MIN_ZERO,
      sort_order: 50,
    }),
    sys(entity, {
      field_key: 'tax_type',
      label: '稅別',
      data_type: 'select',
      is_required: true,
      options: TAX_OPTIONS,
      sort_order: 60,
    }),
    sys(entity, {
      field_key: 'status',
      label: '狀態',
      data_type: 'select',
      is_readonly: true,
      options: statuses.map((s) => ({ ...s, color: null })),
      sort_order: 70,
    }),
    money(entity, 'subtotal', '未稅金額', 80),
    money(entity, 'tax_amount', '稅額', 90),
    money(entity, 'total', '總計', 100),
    money(entity, 'total_twd', '台幣總計', 110, 2),
    sys(entity, { field_key: 'notes', label: '備註', data_type: 'long_text', sort_order: 120 }),
  ];
}

export const PURCHASE_ORDER_FIELDS = pricedHeader(
  PURCHASE_ORDER,
  'order_date',
  '採購日期',
  [
    { value: 'draft', label: '草稿' },
    { value: 'confirmed', label: '已確認' },
    { value: 'partially_received', label: '部分進貨' },
    { value: 'closed', label: '已結案' },
    { value: 'void', label: '已作廢' },
  ],
  [],
);

export const GOODS_RECEIPT_FIELDS = pricedHeader(
  GOODS_RECEIPT,
  'receipt_date',
  '進貨日期',
  [
    { value: 'draft', label: '草稿' },
    { value: 'posted', label: '已過帳' },
    { value: 'void', label: '已作廢' },
  ],
  [
    sys(GOODS_RECEIPT, {
      field_key: 'purchase_order_id',
      label: '採購單',
      data_type: 'relation',
      relation_entity_id: PURCHASE_ORDER.id,
      sort_order: 20,
    }),
  ],
);

function pricedLine(entity: EntityDefinition) {
  return [
    sys(entity, {
      field_key: 'product_id',
      label: '商品',
      data_type: 'relation',
      is_required: true,
      relation_entity_id: PRODUCT.id,
      sort_order: 20,
    }),
    sys(entity, {
      field_key: 'qty',
      label: '數量',
      data_type: 'decimal',
      is_required: true,
      decimal_places: 4,
      validation: MIN_ZERO,
      sort_order: 30,
    }),
    sys(entity, {
      field_key: 'unit_price',
      label: '單價',
      data_type: 'money',
      is_required: true,
      decimal_places: 4,
      validation: MIN_ZERO,
      sort_order: 40,
    }),
    money(entity, 'amount', '金額', 50),
  ];
}

export const PURCHASE_ORDER_LINE_FIELDS = [
  ...pricedLine(PURCHASE_ORDER_LINE),
  sys(PURCHASE_ORDER_LINE, {
    field_key: 'received_qty',
    label: '已進貨數量',
    data_type: 'decimal',
    is_readonly: true,
    decimal_places: 4,
    sort_order: 55,
  }),
  sys(PURCHASE_ORDER_LINE, {
    field_key: 'notes',
    label: '備註',
    data_type: 'long_text',
    sort_order: 60,
  }),
];

export const GOODS_RECEIPT_LINE_FIELDS = [
  sys(GOODS_RECEIPT_LINE, {
    field_key: 'purchase_order_line_id',
    label: '採購明細',
    data_type: 'relation',
    is_visible: false,
    relation_entity_id: PURCHASE_ORDER_LINE.id,
    sort_order: 10,
  }),
  ...pricedLine(GOODS_RECEIPT_LINE),
  sys(GOODS_RECEIPT_LINE, {
    field_key: 'lot_no',
    label: '批號',
    validation: { min: null, max: null, max_length: 50, pattern: null },
    sort_order: 60,
  }),
  money(GOODS_RECEIPT_LINE, 'unit_cost_twd', '台幣單位成本', 70),
  sys(GOODS_RECEIPT_LINE, {
    field_key: 'notes',
    label: '備註',
    data_type: 'long_text',
    sort_order: 80,
  }),
];

/** 所有進貨相關的資料類別（`list_entities`）。 */
export const PURCHASING_ENTITIES = [
  PRODUCT,
  SUPPLIER,
  PURCHASE_ORDER,
  PURCHASE_ORDER_LINE,
  GOODS_RECEIPT,
  GOODS_RECEIPT_LINE,
];

/** `get_entity` 的回應。 */
export function purchasingEntityDetail(key: string) {
  const fields: Record<string, FieldDefinition[]> = {
    purchase_order: PURCHASE_ORDER_FIELDS,
    purchase_order_line: PURCHASE_ORDER_LINE_FIELDS,
    goods_receipt: GOODS_RECEIPT_FIELDS,
    goods_receipt_line: GOODS_RECEIPT_LINE_FIELDS,
    supplier: [],
    product: [],
  };
  const entity = PURCHASING_ENTITIES.find((e) => e.key === key)!;
  return { entity, fields: fields[key] ?? [] };
}

export const SUPPLIER_ID = '0196f5a0-0000-7000-8000-000000000a01';
export const SUPPLIER_TITLE = 'S001 美商五金';
export const ORDER_ID = '0196f5a0-0000-7000-8000-000000000a11';
export const RECEIPT_ID = '0196f5a0-0000-7000-8000-000000000a21';

/** 幣別（`list_currencies`）。 */
export const CURRENCIES = [
  {
    id: '0190a000-0000-7000-8000-000000000001',
    code: 'TWD',
    name: '新台幣',
    symbol: 'NT$',
    decimal_places: 0,
    is_base: true,
    is_active: true,
    created_at: '2026-11-17T00:00:00Z',
    updated_at: '2026-11-17T00:00:00Z',
    version: 1,
  },
  {
    id: '0190a000-0000-7000-8000-000000000002',
    code: 'USD',
    name: '美元',
    symbol: 'US$',
    decimal_places: 2,
    is_base: false,
    is_active: true,
    created_at: '2026-11-17T00:00:00Z',
    updated_at: '2026-11-17T00:00:00Z',
    version: 1,
  },
];

export const TAX_RATES = [
  { code: 'taxable', name: '應稅', rate: '0.0500' },
  { code: 'zero_rated', name: '零稅率', rate: '0.0000' },
  { code: 'exempt', name: '免稅', rate: '0.0000' },
];

/** 一筆單據或明細（`get_record` 的回應）。 */
export function purchasingRecord(patch: Partial<DataRecord> & Pick<DataRecord, 'id'>): DataRecord {
  return {
    title: '',
    record_no: null,
    data: {},
    formula_errors: {},
    relations: {},
    attachments: {},
    document: null,
    created_at: '2026-11-18T01:00:00Z',
    created_by: null,
    updated_at: '2026-11-18T01:00:00Z',
    updated_by: null,
    version: 1,
    ...patch,
  } as DataRecord;
}
