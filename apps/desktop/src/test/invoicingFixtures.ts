import type { EntityDefinition, FieldDefinition } from '../bindings';
import { PRODUCT, STOCK_ADJUSTMENT } from './documentFixtures';
import {
  GOODS_RECEIPT,
  PURCHASE_RETURN,
  PURCHASING_ENTITIES,
  SUPPLIER,
  purchasingEntityDetail,
} from './purchasingFixtures';
import {
  CUSTOMER,
  SALES_ENTITIES,
  SALES_RETURN,
  SHIPMENT,
  salesEntityDetail,
} from './salesFixtures';
import { fieldDef } from './schemaFixtures';

/**
 * 發票模組的資料類別與欄位定義（crates/db/migrations/0014_invoicing.sql；id 為測試用）。
 */

const documentEntity = (patch: Partial<EntityDefinition>): EntityDefinition => ({
  ...STOCK_ADJUSTMENT,
  ...patch,
});

export const SALES_INVOICE: EntityDefinition = documentEntity({
  id: '01a2a0c0-4c50-7000-8000-000000000001',
  key: 'sales_invoice',
  label: '銷項發票',
  numbering_pattern: 'SI{yyyyMM}-{0000}',
  title_template: '{invoice_no} {buyer_name}',
  sort_order: 300,
});

export const SALES_INVOICE_LINE: EntityDefinition = documentEntity({
  id: '01a2a0c0-4c50-7001-8000-000000000002',
  key: 'sales_invoice_line',
  label: '發票明細',
  kind: 'detail',
  parent_entity_id: SALES_INVOICE.id,
  numbering_pattern: null,
  title_template: '{description}',
  sort_order: 0,
});

export const SALES_INVOICE_SHIPMENT: EntityDefinition = documentEntity({
  id: '01a2a0c0-4c50-7002-8000-000000000003',
  key: 'sales_invoice_shipment',
  label: '發票出貨單',
  kind: 'detail',
  parent_entity_id: SALES_INVOICE.id,
  numbering_pattern: null,
  title_template: '{shipment_id.record_no}',
  sort_order: 0,
});

export const SALES_ALLOWANCE: EntityDefinition = documentEntity({
  id: '01a2a0c0-4c50-7003-8000-000000000004',
  key: 'sales_allowance',
  label: '銷貨折讓',
  numbering_pattern: 'SA{yyyyMM}-{0000}',
  title_template: '{record_no} {customer_id.name}',
  sort_order: 310,
});

export const SALES_ALLOWANCE_LINE: EntityDefinition = documentEntity({
  id: '01a2a0c0-4c50-7004-8000-000000000005',
  key: 'sales_allowance_line',
  label: '折讓明細',
  kind: 'detail',
  parent_entity_id: SALES_ALLOWANCE.id,
  numbering_pattern: null,
  title_template: '{description}',
  sort_order: 0,
});

const TAX_OPTIONS = [
  { value: 'taxable', label: '應稅', color: null },
  { value: 'zero_rated', label: '零稅率', color: null },
  { value: 'exempt', label: '免稅', color: null },
];
const MIN_ZERO = { min: '0', max: null, max_length: null, pattern: null };

let sort = 0;
function sys(entity: EntityDefinition, patch: Partial<FieldDefinition>): FieldDefinition {
  sort += 10;
  return fieldDef({
    entity_id: entity.id,
    is_system: true,
    storage: 'column',
    id: `${entity.key}-${patch.field_key}`,
    sort_order: sort,
    ...patch,
  });
}

const readonlyMoney = (entity: EntityDefinition, key: string, label: string) =>
  sys(entity, {
    field_key: key,
    label,
    data_type: 'money',
    is_readonly: true,
    decimal_places: 4,
  });

const status = (entity: EntityDefinition) =>
  sys(entity, {
    field_key: 'status',
    label: '狀態',
    data_type: 'select',
    is_readonly: true,
    options: [
      { value: 'draft', label: '草稿', color: null },
      { value: 'posted', label: '已開立', color: null },
      { value: 'void', label: '已作廢', color: null },
    ],
  });

const voidFields = (entity: EntityDefinition) => [
  sys(entity, { field_key: 'void_date', label: '作廢日期', data_type: 'date', is_readonly: true }),
  sys(entity, { field_key: 'void_reason', label: '作廢原因', is_readonly: true }),
];

const notes = (entity: EntityDefinition) =>
  sys(entity, { field_key: 'notes', label: '備註', data_type: 'long_text' });

function textLines(entity: EntityDefinition): FieldDefinition[] {
  sort = 0;
  return [
    sys(entity, {
      field_key: 'product_id',
      label: '商品',
      data_type: 'relation',
      relation_entity_id: PRODUCT.id,
    }),
    sys(entity, { field_key: 'description', label: '品名', is_required: true }),
    sys(entity, {
      field_key: 'qty',
      label: '數量',
      data_type: 'decimal',
      is_required: true,
      decimal_places: 4,
      validation: MIN_ZERO,
    }),
    sys(entity, {
      field_key: 'unit_price',
      label: '單價',
      data_type: 'money',
      is_required: true,
      decimal_places: 4,
      validation: MIN_ZERO,
    }),
    readonlyMoney(entity, 'amount', '金額'),
    notes(entity),
  ];
}

function salesInvoiceFields(): FieldDefinition[] {
  sort = 0;
  const e = SALES_INVOICE;
  return [
    sys(e, {
      field_key: 'invoice_type',
      label: '發票種類',
      data_type: 'select',
      is_required: true,
      default_value: 'triplicate',
      options: [
        { value: 'triplicate', label: '三聯式', color: null },
        { value: 'duplicate', label: '二聯式', color: null },
      ],
    }),
    sys(e, { field_key: 'invoice_no', label: '發票號碼', is_required: true }),
    sys(e, { field_key: 'invoice_date', label: '發票日期', data_type: 'date', is_required: true }),
    sys(e, { field_key: 'tax_period', label: '期別', is_readonly: true }),
    sys(e, {
      field_key: 'customer_id',
      label: '客戶',
      data_type: 'relation',
      is_required: true,
      relation_entity_id: CUSTOMER.id,
    }),
    sys(e, { field_key: 'buyer_tax_id', label: '買受人統一編號' }),
    sys(e, { field_key: 'buyer_name', label: '買受人' }),
    sys(e, { field_key: 'buyer_address', label: '買受人地址' }),
    sys(e, {
      field_key: 'tax_type',
      label: '稅別',
      data_type: 'select',
      is_required: true,
      options: TAX_OPTIONS,
    }),
    sys(e, {
      field_key: 'tax_amount_manual',
      label: '稅額（手動）',
      data_type: 'money',
      decimal_places: 4,
      validation: MIN_ZERO,
    }),
    status(e),
    readonlyMoney(e, 'subtotal', '未稅金額'),
    readonlyMoney(e, 'tax_amount', '稅額'),
    readonlyMoney(e, 'total', '總計'),
    ...voidFields(e),
    notes(e),
  ];
}

function salesAllowanceFields(): FieldDefinition[] {
  sort = 0;
  const e = SALES_ALLOWANCE;
  return [
    sys(e, {
      field_key: 'sales_invoice_id',
      label: '原發票',
      data_type: 'relation',
      is_required: true,
      relation_entity_id: SALES_INVOICE.id,
    }),
    sys(e, {
      field_key: 'customer_id',
      label: '客戶',
      data_type: 'relation',
      is_readonly: true,
      relation_entity_id: CUSTOMER.id,
    }),
    sys(e, {
      field_key: 'allowance_date',
      label: '折讓日期',
      data_type: 'date',
      is_required: true,
    }),
    sys(e, { field_key: 'tax_period', label: '期別', is_readonly: true }),
    sys(e, {
      field_key: 'tax_type',
      label: '稅別',
      data_type: 'select',
      is_readonly: true,
      options: TAX_OPTIONS,
    }),
    sys(e, {
      field_key: 'sales_return_id',
      label: '銷貨退回',
      data_type: 'relation',
      relation_entity_id: SALES_RETURN.id,
    }),
    sys(e, {
      field_key: 'tax_amount_manual',
      label: '稅額（手動）',
      data_type: 'money',
      decimal_places: 4,
      validation: MIN_ZERO,
    }),
    status(e),
    readonlyMoney(e, 'subtotal', '折讓未稅金額'),
    readonlyMoney(e, 'tax_amount', '折讓稅額'),
    readonlyMoney(e, 'total', '折讓總計'),
    sys(e, { field_key: 'reason', label: '折讓原因' }),
    ...voidFields(e),
    notes(e),
  ];
}

function shipmentLinkFields(): FieldDefinition[] {
  sort = 0;
  return [
    sys(SALES_INVOICE_SHIPMENT, {
      field_key: 'shipment_id',
      label: '出貨單',
      data_type: 'relation',
      is_required: true,
      relation_entity_id: SHIPMENT.id,
    }),
  ];
}

export const SALES_INVOICE_FIELDS = salesInvoiceFields();
export const SALES_INVOICE_LINE_FIELDS = textLines(SALES_INVOICE_LINE);
export const SALES_INVOICE_SHIPMENT_FIELDS = shipmentLinkFields();
export const SALES_ALLOWANCE_FIELDS = salesAllowanceFields();
export const SALES_ALLOWANCE_LINE_FIELDS = textLines(SALES_ALLOWANCE_LINE);

export const PURCHASE_INVOICE: EntityDefinition = documentEntity({
  id: '01a2a0c0-4c50-7005-8000-000000000006',
  key: 'purchase_invoice',
  label: '進項憑證',
  numbering_pattern: 'PI{yyyyMM}-{0000}',
  title_template: '{invoice_no} {supplier_id.name}',
  sort_order: 320,
});

export const PURCHASE_INVOICE_RECEIPT: EntityDefinition = documentEntity({
  id: '01a2a0c0-4c50-7006-8000-000000000007',
  key: 'purchase_invoice_receipt',
  label: '憑證進貨單',
  kind: 'detail',
  parent_entity_id: PURCHASE_INVOICE.id,
  numbering_pattern: null,
  title_template: '{goods_receipt_id.record_no}',
  sort_order: 0,
});

export const PURCHASE_ALLOWANCE: EntityDefinition = documentEntity({
  id: '01a2a0c0-4c50-7007-8000-000000000008',
  key: 'purchase_allowance',
  label: '進貨折讓',
  numbering_pattern: 'PA{yyyyMM}-{0000}',
  title_template: '{record_no} {supplier_id.name}',
  sort_order: 330,
});

const enteredMoney = (entity: EntityDefinition, key: string, label: string) =>
  sys(entity, {
    field_key: key,
    label,
    data_type: 'money',
    is_required: true,
    decimal_places: 4,
    validation: MIN_ZERO,
  });

function purchaseStatus(entity: EntityDefinition, posted: string) {
  return sys(entity, {
    field_key: 'status',
    label: '狀態',
    data_type: 'select',
    is_readonly: true,
    options: [
      { value: 'draft', label: '草稿', color: null },
      { value: 'posted', label: posted, color: null },
      { value: 'void', label: '已作廢', color: null },
    ],
  });
}

function purchaseInvoiceFields(): FieldDefinition[] {
  sort = 0;
  const e = PURCHASE_INVOICE;
  return [
    sys(e, {
      field_key: 'doc_type',
      label: '憑證種類',
      data_type: 'select',
      is_required: true,
      default_value: 'triplicate',
      options: [
        { value: 'triplicate', label: '三聯式發票', color: null },
        { value: 'electronic', label: '電子發票', color: null },
        { value: 'customs', label: '海關代徵營業稅繳納證', color: null },
      ],
    }),
    sys(e, { field_key: 'invoice_no', label: '憑證號碼', is_required: true }),
    sys(e, { field_key: 'invoice_date', label: '憑證日期', data_type: 'date', is_required: true }),
    sys(e, { field_key: 'tax_period', label: '期別', is_readonly: true }),
    sys(e, {
      field_key: 'supplier_id',
      label: '廠商',
      data_type: 'relation',
      is_required: true,
      relation_entity_id: SUPPLIER.id,
    }),
    sys(e, { field_key: 'seller_tax_id', label: '賣方統一編號' }),
    sys(e, {
      field_key: 'tax_type',
      label: '稅別',
      data_type: 'select',
      is_required: true,
      default_value: 'taxable',
      options: TAX_OPTIONS,
    }),
    enteredMoney(e, 'subtotal', '未稅金額'),
    enteredMoney(e, 'tax_amount', '稅額'),
    readonlyMoney(e, 'total', '總計'),
    sys(e, {
      field_key: 'deductible',
      label: '可扣抵',
      data_type: 'boolean',
      is_required: true,
      default_value: true,
    }),
    purchaseStatus(e, '已登錄'),
    ...voidFields(e),
    notes(e),
  ];
}

function receiptLinkFields(): FieldDefinition[] {
  sort = 0;
  return [
    sys(PURCHASE_INVOICE_RECEIPT, {
      field_key: 'goods_receipt_id',
      label: '進貨單',
      data_type: 'relation',
      is_required: true,
      relation_entity_id: GOODS_RECEIPT.id,
    }),
  ];
}

function purchaseAllowanceFields(): FieldDefinition[] {
  sort = 0;
  const e = PURCHASE_ALLOWANCE;
  return [
    sys(e, {
      field_key: 'purchase_invoice_id',
      label: '原憑證',
      data_type: 'relation',
      is_required: true,
      relation_entity_id: PURCHASE_INVOICE.id,
    }),
    sys(e, {
      field_key: 'supplier_id',
      label: '廠商',
      data_type: 'relation',
      is_readonly: true,
      relation_entity_id: SUPPLIER.id,
    }),
    sys(e, {
      field_key: 'allowance_date',
      label: '折讓日期',
      data_type: 'date',
      is_required: true,
    }),
    sys(e, { field_key: 'tax_period', label: '期別', is_readonly: true }),
    sys(e, {
      field_key: 'tax_type',
      label: '稅別',
      data_type: 'select',
      is_readonly: true,
      options: TAX_OPTIONS,
    }),
    sys(e, {
      field_key: 'purchase_return_id',
      label: '進貨退回',
      data_type: 'relation',
      relation_entity_id: PURCHASE_RETURN.id,
    }),
    enteredMoney(e, 'subtotal', '折讓未稅金額'),
    enteredMoney(e, 'tax_amount', '折讓稅額'),
    readonlyMoney(e, 'total', '折讓總計'),
    purchaseStatus(e, '已開立'),
    sys(e, { field_key: 'reason', label: '折讓原因' }),
    ...voidFields(e),
    notes(e),
  ];
}

export const PURCHASE_INVOICE_FIELDS = purchaseInvoiceFields();
export const PURCHASE_INVOICE_RECEIPT_FIELDS = receiptLinkFields();
export const PURCHASE_ALLOWANCE_FIELDS = purchaseAllowanceFields();

/** 進貨與發票模組的資料類別（`list_entities`）。 */
export const PURCHASE_INVOICING_ENTITIES = [
  ...PURCHASING_ENTITIES,
  PURCHASE_INVOICE,
  PURCHASE_INVOICE_RECEIPT,
  PURCHASE_ALLOWANCE,
];

/** 進項憑證、進貨折讓的 `get_entity`（其他交給 {@link purchasingEntityDetail}）。 */
export function purchaseInvoicingEntityDetail(key: string) {
  const fields: Record<string, FieldDefinition[]> = {
    purchase_invoice: PURCHASE_INVOICE_FIELDS,
    purchase_invoice_receipt: PURCHASE_INVOICE_RECEIPT_FIELDS,
    purchase_allowance: PURCHASE_ALLOWANCE_FIELDS,
  };
  const entity = PURCHASE_INVOICING_ENTITIES.find((e) => e.key === key);
  if (!entity || !(key in fields)) {
    return purchasingEntityDetail(key);
  }
  return { entity, fields: fields[key] };
}

export const PURCHASE_INVOICE_ID = '0196f5a0-0000-7000-8000-000000000e21';
export const PURCHASE_ALLOWANCE_ID = '0196f5a0-0000-7000-8000-000000000e31';

/** 銷貨與發票模組的資料類別（`list_entities`）。 */
export const INVOICING_ENTITIES = [
  ...SALES_ENTITIES,
  SALES_INVOICE,
  SALES_INVOICE_LINE,
  SALES_INVOICE_SHIPMENT,
  SALES_ALLOWANCE,
  SALES_ALLOWANCE_LINE,
];

/** `get_entity` 的回應（銷貨模組以外的交給 {@link salesEntityDetail}）。 */
export function invoicingEntityDetail(key: string) {
  const fields: Record<string, FieldDefinition[]> = {
    sales_invoice: SALES_INVOICE_FIELDS,
    sales_invoice_line: SALES_INVOICE_LINE_FIELDS,
    sales_invoice_shipment: SALES_INVOICE_SHIPMENT_FIELDS,
    sales_allowance: SALES_ALLOWANCE_FIELDS,
    sales_allowance_line: SALES_ALLOWANCE_LINE_FIELDS,
  };
  const entity = INVOICING_ENTITIES.find((e) => e.key === key);
  if (!entity || !(key in fields)) {
    return salesEntityDetail(key);
  }
  return { entity, fields: fields[key] };
}

export const SALES_INVOICE_ID = '0196f5a0-0000-7000-8000-000000000e01';
export const SALES_ALLOWANCE_ID = '0196f5a0-0000-7000-8000-000000000e11';
