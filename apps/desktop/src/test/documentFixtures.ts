import type { EntityDefinition, Record as DataRecord } from '../bindings';
import { fieldDef } from './schemaFixtures';

/** 系統單據：盤點調整單（crates/db/migrations/0011_inventory.sql）。 */
export const STOCK_ADJUSTMENT: EntityDefinition = {
  id: '01a1ea12-f322-7000-8bce-42d87e7290fb',
  key: 'stock_adjustment',
  label: '盤點調整單',
  icon: null,
  kind: 'document',
  is_system: true,
  storage: 'table',
  parent_entity_id: null,
  numbering_pattern: 'ADJ{yyyyMM}-{0000}',
  title_template: '{record_no}',
  sort_order: 100,
  is_active: true,
  created_at: '2026-11-13T01:00:00Z',
  updated_at: '2026-11-13T01:00:00Z',
  version: 1,
};

const system = { entity_id: STOCK_ADJUSTMENT.id, is_system: true, storage: 'column' } as const;

export const STOCK_ADJUSTMENT_FIELDS = [
  fieldDef({
    ...system,
    field_key: 'adjustment_date',
    label: '調整日期',
    data_type: 'date',
    is_required: true,
    sort_order: 10,
  }),
  fieldDef({
    ...system,
    field_key: 'adjustment_type',
    label: '類型',
    data_type: 'select',
    is_required: true,
    default_value: 'count',
    options: [
      { value: 'opening', label: '期初庫存', color: null },
      { value: 'count', label: '盤點', color: null },
      { value: 'adjustment', label: '一般調整', color: null },
    ],
    sort_order: 20,
  }),
  fieldDef({
    ...system,
    field_key: 'status',
    label: '狀態',
    data_type: 'select',
    is_readonly: true,
    options: [
      { value: 'draft', label: '草稿', color: null },
      { value: 'posted', label: '已過帳', color: null },
      { value: 'void', label: '已作廢', color: null },
    ],
    sort_order: 30,
  }),
  fieldDef({
    ...system,
    field_key: 'notes',
    label: '備註',
    data_type: 'long_text',
    sort_order: 40,
  }),
];

export const ADJUSTMENT_ID = '0196f5a0-0000-7000-8000-000000000501';

/** 一張盤點調整單（預設為草稿）。 */
export function adjustment(
  status: 'draft' | 'posted' | 'void' = 'draft',
  patch: Partial<DataRecord> = {},
): DataRecord {
  return {
    id: ADJUSTMENT_ID,
    title: 'ADJ202611-0001',
    record_no: 'ADJ202611-0001',
    data: {
      adjustment_date: '2026-11-18',
      adjustment_type: 'opening',
      status,
      notes: '期初庫存',
    },
    document: {
      posted_at: status === 'draft' ? null : '2026-11-18T02:00:00Z',
      posted_by: null,
      voided_at: status === 'void' ? '2026-11-18T03:00:00Z' : null,
      voided_by: null,
    },
    created_at: '2026-11-18T01:00:00Z',
    created_by: null,
    updated_at: '2026-11-18T01:00:00Z',
    updated_by: null,
    version: status === 'draft' ? 1 : status === 'posted' ? 2 : 3,
    ...patch,
  };
}

/** 商品（系統主檔，盤點調整明細的 `product_id` 指向它）。 */
export const PRODUCT: EntityDefinition = {
  ...STOCK_ADJUSTMENT,
  id: '01a1e516-3401-7432-acb3-9cc3ac6a7db9',
  key: 'product',
  label: '商品',
  kind: 'master',
  numbering_pattern: null,
  title_template: '{code} {name}',
  sort_order: 10,
};

/** 盤點調整明細（明細子表）。 */
export const STOCK_ADJUSTMENT_LINE: EntityDefinition = {
  ...STOCK_ADJUSTMENT,
  id: '01a1ea12-f322-7001-a093-e320da8dffef',
  key: 'stock_adjustment_line',
  label: '盤點調整明細',
  kind: 'detail',
  parent_entity_id: STOCK_ADJUSTMENT.id,
  numbering_pattern: null,
  title_template: '{product_id.code} {product_id.name} {lot_no}',
  sort_order: 0,
};

const lineField = {
  entity_id: STOCK_ADJUSTMENT_LINE.id,
  is_system: true,
  storage: 'column',
} as const;

export const STOCK_ADJUSTMENT_LINE_FIELDS = [
  fieldDef({
    ...lineField,
    field_key: 'product_id',
    label: '商品',
    data_type: 'relation',
    is_required: true,
    relation_entity_id: PRODUCT.id,
    sort_order: 10,
  }),
  fieldDef({
    ...lineField,
    field_key: 'line_type',
    label: '調整方式',
    data_type: 'select',
    is_required: true,
    default_value: 'gain_new',
    options: [
      { value: 'gain_new', label: '盤盈：新批號', color: null },
      { value: 'gain_existing', label: '盤盈：加回既有批號', color: null },
      { value: 'loss', label: '盤虧：扣指定批號', color: null },
    ],
    sort_order: 20,
  }),
  fieldDef({
    ...lineField,
    field_key: 'lot_no',
    label: '批號',
    validation: { min: null, max: null, max_length: 50, pattern: null },
    sort_order: 30,
  }),
  fieldDef({
    ...lineField,
    field_key: 'qty',
    label: '數量',
    data_type: 'decimal',
    is_required: true,
    decimal_places: 4,
    sort_order: 40,
  }),
  fieldDef({
    ...lineField,
    field_key: 'unit_cost_twd',
    label: '單位成本',
    data_type: 'money',
    decimal_places: 4,
    sort_order: 50,
  }),
  fieldDef({ ...lineField, field_key: 'reason', label: '原因', sort_order: 60 }),
];

export const PRODUCT_ID = '0196f5a0-0000-7000-8000-000000000601';
export const LOT_ID = '0196f5a0-0000-7000-8000-000000000701';

/** 一個批號（`list_product_lots` 的項目）。 */
export function lot(patch: Record<string, unknown> = {}) {
  return {
    id: LOT_ID,
    lot_no: 'L-20261101',
    received_date: '2026-11-01',
    source_type: 'stock_adjustment_line',
    source_doc_id: ADJUSTMENT_ID,
    source_record_no: 'ADJ202611-0001',
    qty_in: '100.0000',
    qty_remaining: '35.5000',
    unit_cost_twd: '12.5000',
    ...patch,
  };
}
