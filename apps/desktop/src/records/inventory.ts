import type {
  EntityDefinition,
  FieldDefinition,
  InventoryLot_Serialize as InventoryLot,
  TxnType,
} from '../bindings';
import { formatDecimal, groupThousands } from './display';
import { formatQty } from './documents';

/**
 * 庫存畫面的共通規則（docs/architecture.md §8.10）：盤點調整單的明細輸入、庫存查詢。
 */

/** 庫存查詢頁（商品庫存列表）的路徑；商品的批號與異動為 `/stock/:productId`。 */
export const STOCK_PATH = 'stock';

/** 商品的批號與異動流水帳頁。 */
export function productStockPath(productId: string): string {
  return `/${STOCK_PATH}/${productId}`;
}

/** 盤點調整單（系統單據）與其明細子表的代碼。 */
export const STOCK_ADJUSTMENT = 'stock_adjustment';
export const STOCK_ADJUSTMENT_LINE = 'stock_adjustment_line';

/** 「庫存」選單分組列出的系統資料類別，依此順序。 */
export const INVENTORY_ENTITY_KEYS: readonly string[] = [STOCK_ADJUSTMENT];

/** 選單「庫存」分組的標題。 */
export const INVENTORY_GROUP_LABEL = '庫存';

/** 「庫存」分組列出的資料類別：{@link INVENTORY_ENTITY_KEYS} 中有效的資料類別。 */
export function inventoryEntities(entities: readonly EntityDefinition[]): EntityDefinition[] {
  return INVENTORY_ENTITY_KEYS.flatMap((key) => {
    const entity = entities.find((e) => e.key === key && e.is_active);
    return entity ? [entity] : [];
  });
}

/** 盤點調整明細的調整方式（`line_type`）。 */
export type LineType = 'gain_new' | 'gain_existing' | 'loss';

/** 明細欄位代碼。 */
export const LINE_FIELDS = {
  product: 'product_id',
  lineType: 'line_type',
  lotNo: 'lot_no',
  qty: 'qty',
  unitCost: 'unit_cost_twd',
} as const;

/**
 * 調整方式是否使用既有批號（`gain_existing` 加回、`loss` 扣除）：批號從該商品的批號中選擇，
 * 單位成本不可填（過帳時寫入批號的成本）。`gain_new` 為新批號：自行輸入批號（可留空）與成本。
 */
export function usesExistingLot(lineType: unknown): boolean {
  return lineType === 'gain_existing' || lineType === 'loss';
}

/** 選擇批號時是否列出已用完的批號：盤盈加回可以加回已用完的批號；盤虧只能扣有數量的批號。 */
export function lotsIncludeEmpty(lineType: unknown): boolean {
  return lineType === 'gain_existing';
}

/** 批號選項的文字：`L-01（剩 3，2026-11-17 入庫）`。 */
export function lotOptionLabel(
  lot: Pick<InventoryLot, 'lot_no' | 'qty_remaining' | 'received_date'>,
) {
  return `${lot.lot_no}（剩 ${formatQty(lot.qty_remaining)}，${lot.received_date} 入庫）`;
}

/** 依批號（不分大小寫，與伺服器比對方式相同）找出批號。 */
export function findLot<T extends Pick<InventoryLot, 'lot_no'>>(
  lots: readonly T[] | undefined,
  lotNo: unknown,
): T | undefined {
  if (typeof lotNo !== 'string' || lotNo.trim() === '') {
    return undefined;
  }
  const key = lotNo.trim().toLowerCase();
  return lots?.find((l) => l.lot_no.toLowerCase() === key);
}

/** 新增時預設今天的單據日期欄位（盤點調整單 §8.10、進貨單據 §8.8、銷貨單據 §8.9、發票模組 §8.11）。 */
const TODAY_DEFAULT_FIELDS: Record<string, string> = {
  [STOCK_ADJUSTMENT]: 'adjustment_date',
  purchase_order: 'order_date',
  goods_receipt: 'receipt_date',
  purchase_return: 'return_date',
  sales_order: 'order_date',
  shipment: 'ship_date',
  sales_return: 'return_date',
  // 發票模組（§8.11）
  sales_invoice: 'invoice_date',
  sales_allowance: 'allowance_date',
  purchase_invoice: 'invoice_date',
  purchase_allowance: 'allowance_date',
};

/** 新增時稅別預設應稅的銷貨單據（§8.9「金額與稅額」；出貨單選擇訂單後帶入訂單的稅別）。 */
const TAXABLE_DEFAULT: readonly string[] = ['sales_order', 'shipment', 'sales_invoice'];

/**
 * 新增資料時表單的預設值：系統單據的單據日期預設今天（台北時間）；訂單、出貨單的稅別預設應稅
 * （都只在欄位定義沒有設定預設值時）。
 * 其他資料類別原樣回傳。
 */
export function withFormDefaults(
  entityKey: string,
  fields: FieldDefinition[],
  today: string,
): FieldDefinition[] {
  const dateField = TODAY_DEFAULT_FIELDS[entityKey];
  if (!dateField) {
    return fields;
  }
  const taxable = TAXABLE_DEFAULT.includes(entityKey);
  return fields.map((f) => {
    if (f.default_value !== null) {
      return f;
    }
    if (f.field_key === dateField) {
      return { ...f, default_value: today };
    }
    if (taxable && f.field_key === 'tax_type') {
      return { ...f, default_value: 'taxable' };
    }
    return f;
  });
}

/** 異動類型的顯示名稱。 */
export const TXN_TYPE_LABELS: Record<TxnType, string> = {
  receipt: '進貨',
  shipment: '出貨',
  sales_return: '銷貨退回',
  purchase_return: '進貨退回',
  adjustment: '盤點調整',
  void_reversal: '作廢沖回',
};

/** 數量變動：入庫加上 `+`，`"-3.0000"` → `-3`、`"5.5000"` → `+5.5`。 */
export function formatQtyChange(raw: string): string {
  const text = formatQty(raw);
  return raw.trim().startsWith('-') ? text : `+${text}`;
}

/** 單位成本（4 位小數，去掉尾端的 0 但至少 2 位）：`"12.5000"` → `12.50`。 */
export function formatUnitCost(raw: string): string {
  const m = /^(-?\d+)(?:\.(\d*?))0*$/.exec(raw.trim());
  if (!m) {
    return raw;
  }
  return formatDecimal(`${m[1]}${m[2] ? `.${m[2]}` : ''}`, 2);
}

/** 庫存金額（伺服器已四捨五入到 2 位小數）：`"3450.00"` → `3,450.00`。 */
export function formatAmount(raw: string): string {
  return groupThousands(raw);
}

/**
 * 明細子表代碼 → 主表（單據）代碼，例如批號的 `source_type = stock_adjustment_line` → `stock_adjustment`。
 * 找不到時回傳 `undefined`（不顯示連結）。
 */
export function documentKeyOfLine(
  entities: readonly EntityDefinition[] | undefined,
  lineKey: string,
): string | undefined {
  const line = entities?.find((e) => e.key === lineKey);
  return line?.parent_entity_id
    ? entities?.find((e) => e.id === line.parent_entity_id)?.key
    : undefined;
}
