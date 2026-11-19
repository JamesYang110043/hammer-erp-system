import type { EntityDefinition, FieldDefinition } from '../bindings';
import { PRODUCT, STOCK_ADJUSTMENT } from './documentFixtures';
import { fieldDef } from './schemaFixtures';

/**
 * 銷貨模組的資料類別與欄位定義（crates/db/migrations/0013_sales.sql）。
 */

const documentEntity = (patch: Partial<EntityDefinition>): EntityDefinition => ({
  ...STOCK_ADJUSTMENT,
  ...patch,
});

/** 客戶（系統主檔）。 */
export const CUSTOMER: EntityDefinition = documentEntity({
  id: '01a1e516-3404-7153-bc28-b536c04db4db',
  key: 'customer',
  label: '客戶',
  kind: 'master',
  numbering_pattern: null,
  title_template: '{code} {name}',
  sort_order: 40,
});

export const SALES_ORDER: EntityDefinition = documentEntity({
  id: '01a1f9d2-7b30-7000-94a2-592b16d9d49a',
  key: 'sales_order',
  label: '訂單',
  numbering_pattern: 'SO{yyyyMM}-{0000}',
  title_template: '{record_no} {customer_id.name}',
  sort_order: 200,
});

export const SALES_ORDER_LINE: EntityDefinition = documentEntity({
  id: '01a1f9d2-7b30-7001-810e-1c4e6e7ccaee',
  key: 'sales_order_line',
  label: '訂單明細',
  kind: 'detail',
  parent_entity_id: SALES_ORDER.id,
  numbering_pattern: null,
  title_template: '{product_id.code} {product_id.name}',
  sort_order: 0,
});

export const SHIPMENT: EntityDefinition = documentEntity({
  id: '01a1f9d2-7b30-7002-bbf4-a37505aebf4d',
  key: 'shipment',
  label: '出貨單',
  numbering_pattern: 'SH{yyyyMM}-{0000}',
  title_template: '{record_no} {customer_id.name}',
  sort_order: 210,
});

export const SHIPMENT_LINE: EntityDefinition = documentEntity({
  id: '01a1f9d2-7b30-7003-a04b-d9224e8eaf33',
  key: 'shipment_line',
  label: '出貨明細',
  kind: 'detail',
  parent_entity_id: SHIPMENT.id,
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
const RATE = { min: '0', max: '1', max_length: null, pattern: null };

function sys(entity: EntityDefinition, patch: Partial<FieldDefinition>): FieldDefinition {
  return fieldDef({
    entity_id: entity.id,
    is_system: true,
    storage: 'column',
    id: `${entity.key}-${patch.field_key}`,
    ...patch,
  });
}

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

function status(entity: EntityDefinition, options: [string, string][]) {
  return sys(entity, {
    field_key: 'status',
    label: '狀態',
    data_type: 'select',
    is_readonly: true,
    options: options.map(([value, label]) => ({ value, label, color: null })),
    sort_order: 60,
  });
}

function header(
  entity: EntityDefinition,
  dateKey: string,
  dateLabel: string,
  extra: FieldDefinition[],
) {
  return [
    sys(entity, {
      field_key: 'customer_id',
      label: '客戶',
      data_type: 'relation',
      is_required: true,
      relation_entity_id: CUSTOMER.id,
      sort_order: 10,
    }),
    sys(entity, {
      field_key: dateKey,
      label: dateLabel,
      data_type: 'date',
      is_required: true,
      sort_order: 30,
    }),
    sys(entity, {
      field_key: 'tax_type',
      label: '稅別',
      data_type: 'select',
      is_required: true,
      options: TAX_OPTIONS,
      sort_order: 40,
    }),
    sys(entity, {
      field_key: 'tax_included',
      label: '單價含稅',
      data_type: 'boolean',
      is_required: true,
      default_value: false,
      sort_order: 50,
    }),
    money(entity, 'subtotal', '未稅金額', 70),
    money(entity, 'tax_amount', '稅額', 80),
    money(entity, 'total', '總計', 90),
    ...extra,
    sys(entity, { field_key: 'notes', label: '備註', data_type: 'long_text', sort_order: 120 }),
  ];
}

export const SALES_ORDER_FIELDS = header(SALES_ORDER, 'order_date', '訂單日期', [
  sys(SALES_ORDER, {
    field_key: 'delivery_date',
    label: '預計交貨日',
    data_type: 'date',
    sort_order: 35,
  }),
  status(SALES_ORDER, [
    ['draft', '草稿'],
    ['confirmed', '已確認'],
    ['partially_shipped', '部分出貨'],
    ['closed', '已結案'],
    ['void', '已作廢'],
  ]),
]);

export const SHIPMENT_FIELDS = header(SHIPMENT, 'ship_date', '出貨日期', [
  sys(SHIPMENT, {
    field_key: 'sales_order_id',
    label: '訂單',
    data_type: 'relation',
    relation_entity_id: SALES_ORDER.id,
    sort_order: 20,
  }),
  status(SHIPMENT, [
    ['draft', '草稿'],
    ['posted', '已過帳'],
    ['void', '已作廢'],
  ]),
  money(SHIPMENT, 'cost_total_twd', '銷貨成本', 100, 2),
  sys(SHIPMENT, { field_key: 'ship_address', label: '送貨地址', sort_order: 110 }),
]);

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
    sys(entity, {
      field_key: 'discount_rate',
      label: '折扣率',
      data_type: 'decimal',
      is_required: true,
      decimal_places: 4,
      default_value: '0',
      validation: RATE,
      sort_order: 50,
    }),
    money(entity, 'amount', '金額', 60),
  ];
}

export const SALES_ORDER_LINE_FIELDS = [
  ...pricedLine(SALES_ORDER_LINE),
  sys(SALES_ORDER_LINE, {
    field_key: 'shipped_qty',
    label: '已出貨數量',
    data_type: 'decimal',
    is_readonly: true,
    decimal_places: 4,
    sort_order: 65,
  }),
  sys(SALES_ORDER_LINE, {
    field_key: 'notes',
    label: '備註',
    data_type: 'long_text',
    sort_order: 70,
  }),
];

export const SHIPMENT_LINE_FIELDS = [
  sys(SHIPMENT_LINE, {
    field_key: 'sales_order_line_id',
    label: '訂單明細',
    data_type: 'relation',
    is_visible: false,
    relation_entity_id: SALES_ORDER_LINE.id,
    sort_order: 10,
  }),
  ...pricedLine(SHIPMENT_LINE),
  sys(SHIPMENT_LINE, {
    field_key: 'lot_no',
    label: '批號',
    validation: { min: null, max: null, max_length: 50, pattern: null },
    sort_order: 70,
  }),
  money(SHIPMENT_LINE, 'cost_amount_twd', '成本', 80, 2),
  sys(SHIPMENT_LINE, { field_key: 'notes', label: '備註', data_type: 'long_text', sort_order: 90 }),
];

/** 銷貨相關的資料類別（`list_entities`）。 */
export const SALES_ENTITIES = [
  PRODUCT,
  CUSTOMER,
  SALES_ORDER,
  SALES_ORDER_LINE,
  SHIPMENT,
  SHIPMENT_LINE,
];

/** `get_entity` 的回應。 */
export function salesEntityDetail(key: string) {
  const fields: Record<string, FieldDefinition[]> = {
    sales_order: SALES_ORDER_FIELDS,
    sales_order_line: SALES_ORDER_LINE_FIELDS,
    shipment: SHIPMENT_FIELDS,
    shipment_line: SHIPMENT_LINE_FIELDS,
  };
  const entity = SALES_ENTITIES.find((e) => e.key === key)!;
  return { entity, fields: fields[key] ?? [] };
}

export const CUSTOMER_ID = '0196f5a0-0000-7000-8000-000000000d01';
export const CUSTOMER_TITLE = 'C001 大同五金行';
export const SALES_ORDER_ID = '0196f5a0-0000-7000-8000-000000000d11';
export const SHIPMENT_ID = '0196f5a0-0000-7000-8000-000000000d21';
