import type {
  AllocationPreview_Serialize as AllocationPreview,
  DocumentStatus,
  EntityDefinition,
  InventoryTransaction_Serialize as InventoryTransaction,
  Record as DataRecord,
} from '../bindings';
import { add, div, format, mul, parse, round, sub, ZERO } from './decimal';
import type { Dec } from './decimal';
import { SALES_ORDER } from './documents';
import type { RecordData } from './form';

/**
 * 銷貨模組的畫面規則（docs/architecture.md §8.9）：訂單、出貨單的金額預覽
 * （與伺服器 `hammer_core::sales::amounts` 相同的計算）、訂單轉出貨單、出貨的批號（建議批號、手動指定）。
 *
 * 銷貨以台幣交易，沒有幣別與匯率；金額一律以字串與 `bigint` 計算，不經過浮點數（D9）。
 */

export { SALES_ORDER };
export const SALES_ORDER_LINE = 'sales_order_line';
export const SHIPMENT = 'shipment' as const;
export const SHIPMENT_LINE = 'shipment_line';
export const SALES_RETURN = 'sales_return' as const;
export const SALES_RETURN_LINE = 'sales_return_line';

/** 「銷貨」選單分組列出的系統單據，依此順序。 */
export const SALES_ENTITY_KEYS: readonly string[] = [SALES_ORDER, SHIPMENT, SALES_RETURN];

/** 選單「銷貨」分組的標題。 */
export const SALES_GROUP_LABEL = '銷貨';

/**
 * 「銷貨」分組列出的資料類別：{@link SALES_ENTITY_KEYS} 中有效的系統資料類別
 * （自訂資料類別的代碼可能相同，例如自訂的 `sales_order`，仍放在自訂分組）。
 */
export function salesEntities(entities: readonly EntityDefinition[]): EntityDefinition[] {
  return SALES_ENTITY_KEYS.flatMap((key) => {
    const entity = entities.find((e) => e.key === key && e.is_active && e.is_system);
    return entity ? [entity] : [];
  });
}

/** 欄位代碼。 */
export const SALES_FIELDS = {
  customer: 'customer_id',
  salesOrder: 'sales_order_id',
  taxType: 'tax_type',
  taxIncluded: 'tax_included',
  subtotal: 'subtotal',
  taxAmount: 'tax_amount',
  total: 'total',
  shipAddress: 'ship_address',
  product: 'product_id',
  qty: 'qty',
  unitPrice: 'unit_price',
  discountRate: 'discount_rate',
  amount: 'amount',
  shippedQty: 'shipped_qty',
  lotNo: 'lot_no',
  costAmountTwd: 'cost_amount_twd',
  salesOrderLine: 'sales_order_line_id',
  shipment: 'shipment_id',
  shipmentLine: 'shipment_line_id',
} as const;

/** 有金額預覽的銷貨單據 → 明細子表代碼（訂單、出貨單、銷貨退回）。 */
const SALES_PRICED: Record<string, string> = {
  [SALES_ORDER]: SALES_ORDER_LINE,
  [SHIPMENT]: SHIPMENT_LINE,
  // 稅別、單價含稅、單價、折扣率由伺服器從出貨單帶入，畫面只預覽金額
  [SALES_RETURN]: SALES_RETURN_LINE,
};

/** 銷貨單據的金額預覽用的明細子表代碼；不是訂單、出貨單、銷貨退回時回傳 `undefined`。 */
export function salesLineKey(entityKey: string): string | undefined {
  return SALES_PRICED[entityKey];
}

/** 單據金額的小數位數（`NUMERIC(18,4)`）。 */
const AMOUNT_SCALE = 4;

/**
 * 明細金額：`round(qty × unit_price × (1 − discount_rate), d)`，4 位小數。
 * 數量或單價不是數字時回傳 `undefined`；折扣率留空視為 0。
 */
export function salesLineAmount(
  qty: unknown,
  unitPrice: unknown,
  discountRate: unknown,
  decimalPlaces: number,
): string | undefined {
  const q = parse(qty);
  const p = parse(unitPrice);
  if (!q || !p) {
    return undefined;
  }
  const discount = parse(discountRate) ?? ZERO;
  const factor = sub({ units: 1n, scale: 0 }, discount);
  return format(round(mul(mul(q, p), factor), decimalPlaces), AMOUNT_SCALE);
}

/** 銷貨單據的合計。 */
export interface SalesTotals {
  subtotal: string;
  tax_amount: string;
  total: string;
}

/**
 * 依明細金額計算合計（§8.9「金額與稅額」）：
 *
 * - 未稅：`subtotal = Σ amount`、`tax_amount = round(subtotal × 稅率, d)`、`total = subtotal + tax_amount`
 * - 含稅：`total = Σ amount`、`subtotal = round(total ÷ (1 + 稅率), d)`、`tax_amount = total − subtotal`
 */
export function salesTotals(
  amounts: readonly string[],
  taxRate: string,
  taxIncluded: boolean,
  decimalPlaces: number,
): SalesTotals {
  const sum = amounts.reduce<Dec>((acc, a) => add(acc, parse(a) ?? ZERO), ZERO);
  const rate = parse(taxRate) ?? ZERO;
  if (taxIncluded) {
    const subtotal = div(sum, add({ units: 1n, scale: 0 }, rate), decimalPlaces) ?? sum;
    return {
      subtotal: format(subtotal, AMOUNT_SCALE),
      tax_amount: format(sub(sum, subtotal), AMOUNT_SCALE),
      total: format(sum, AMOUNT_SCALE),
    };
  }
  const tax = round(mul(sum, rate), decimalPlaces);
  return {
    subtotal: format(sum, AMOUNT_SCALE),
    tax_amount: format(tax, AMOUNT_SCALE),
    total: format(add(sum, tax), AMOUNT_SCALE),
  };
}

/**
 * 選擇商品時帶入的單價（§8.9「金額與稅額」）：`default_sale_price` 是未稅價；
 * 單價含稅時帶入 `round(default_sale_price × (1 + 稅率), 4)`。沒有設定售價時回傳 `undefined`。
 */
export function salePrice(
  defaultSalePrice: unknown,
  taxIncluded: boolean,
  taxRate: string | undefined,
): string | undefined {
  const price = parse(defaultSalePrice);
  if (!price) {
    return undefined;
  }
  if (!taxIncluded) {
    return format(price, AMOUNT_SCALE);
  }
  const factor = add({ units: 1n, scale: 0 }, parse(taxRate) ?? ZERO);
  return format(round(mul(price, factor), AMOUNT_SCALE), AMOUNT_SCALE);
}

/** 數值是否大於 0。 */
function positive(raw: unknown): boolean {
  const d = parse(raw);
  return Boolean(d && d.units > 0n);
}

// ---- 訂單轉出貨單 ----

/** 訂單不能出貨的原因（§8.9「訂單轉出貨單」）；可以出貨時回傳 `undefined`。 */
export function orderNotShippable(status: DocumentStatus | undefined): string | undefined {
  switch (status) {
    case 'confirmed':
    case 'partially_shipped':
      return undefined;
    case 'closed':
      return '訂單已結案';
    case 'void':
      return '訂單已作廢';
    default:
      return '訂單尚未確認';
  }
}

/** 未出貨數量：`qty − shipped_qty`（4 位小數）；不是數字時為 `undefined`。 */
export function unshippedQty(qty: unknown, shippedQty: unknown): string | undefined {
  const q = parse(qty);
  if (!q) {
    return undefined;
  }
  return format(sub(q, parse(shippedQty) ?? ZERO), AMOUNT_SCALE);
}

/**
 * 訂單帶入出貨單的明細（JSON 存值）：未出貨數量大於 0 的每一列，帶入訂單明細、商品、單價、折扣率，
 * 數量為未出貨數量，批號留空（先進先出）。
 */
export function shipmentLinesFromOrder(order: Pick<DataRecord, 'lines'>): RecordData[] {
  return (order.lines?.[SALES_ORDER_LINE] ?? []).flatMap((line) => {
    const qty = unshippedQty(line.data[SALES_FIELDS.qty], line.data[SALES_FIELDS.shippedQty]);
    if (!positive(qty)) {
      return [];
    }
    return [
      {
        [SALES_FIELDS.salesOrderLine]: line.id,
        [SALES_FIELDS.product]: line.data[SALES_FIELDS.product] ?? null,
        [SALES_FIELDS.qty]: qty!,
        [SALES_FIELDS.unitPrice]: line.data[SALES_FIELDS.unitPrice] ?? null,
        [SALES_FIELDS.discountRate]: line.data[SALES_FIELDS.discountRate] ?? '0',
        [SALES_FIELDS.lotNo]: null,
      },
    ];
  });
}

/** 出貨明細列中與未出貨數量檢查有關的值。 */
export interface ShipmentRowQty {
  salesOrderLineId?: unknown;
  qty?: unknown;
}

/**
 * 超過未出貨數量的列（§8.9「訂單轉出貨單」的「不允許超出」）：同一張出貨單中指向同一列訂單明細的
 * `qty` 合計 > 未出貨數量時，回傳該組第一列的位置 → 未出貨數量（4 位小數）。
 *
 * `unshipped` 鍵為訂單明細 id；不在其中的訂單明細不檢查（由伺服器檢查）。
 */
export function overShippedRows(
  rows: readonly ShipmentRowQty[],
  unshipped: ReadonlyMap<string, string>,
): Map<number, string> {
  const groups = new Map<string, { first: number; sum: Dec }>();
  rows.forEach((row, i) => {
    const id = row.salesOrderLineId;
    if (typeof id !== 'string' || id === '') {
      return;
    }
    const qty = parse(row.qty) ?? ZERO;
    const g = groups.get(id);
    if (g) {
      g.sum = add(g.sum, qty);
    } else {
      groups.set(id, { first: i, sum: qty });
    }
  });
  const result = new Map<number, string>();
  for (const [id, g] of groups) {
    const limit = unshipped.get(id);
    const max = parse(limit);
    if (limit && max && sub(g.sum, max).units > 0n) {
      result.set(g.first, limit);
    }
  }
  return result;
}

// ---- 出貨的批號 ----

/** 送到建議批號 API 的一列，`row` 為在表單明細中的位置。 */
export interface PreviewInput {
  row: number;
  product_id: string;
  qty: string;
  lot_no: string | null;
}

/** 明細列中送給建議批號 API 的值。 */
export interface ShipmentRowLot {
  productId?: unknown;
  qty?: unknown;
  lotNo?: unknown;
}

/** 批號去除前後空白；空字串視為留空（§8.9「出貨的批號」）。 */
export function normalizeLotNo(raw: unknown): string | null {
  return typeof raw === 'string' && raw.trim() !== '' ? raw.trim() : null;
}

/**
 * 建議批號 API 的輸入：有商品、數量大於 0 的列（其他列不送出，結果中沒有該列）。
 * 送出的順序與表單相同，伺服器依同樣的順序分配（與過帳相同）。
 */
export function previewInputs(rows: readonly ShipmentRowLot[]): PreviewInput[] {
  return rows.flatMap((row, i) => {
    const productId = row.productId;
    const qty = row.qty;
    if (typeof productId !== 'string' || productId === '' || typeof qty !== 'string') {
      return [];
    }
    if (!positive(qty)) {
      return [];
    }
    return [{ row: i, product_id: productId, qty: qty.trim(), lot_no: normalizeLotNo(row.lotNo) }];
  });
}

/** 一列的建議批號結果（顯示用）。 */
export interface RowAllocation {
  /** `goods` / `service`。 */
  productType: string;
  /** 是否手動指定批號。 */
  manual: boolean;
  allocations: AllocationPreview['lines'][number]['allocations'];
  /** 不足的數量（4 位小數）；足夠時為 `undefined`。 */
  shortage?: string;
}

/** 建議批號 API 的結果 → 表單明細位置 → 該列的結果。 */
export function rowAllocations(
  inputs: readonly PreviewInput[],
  preview: Pick<AllocationPreview, 'lines'>,
): Map<number, RowAllocation> {
  const result = new Map<number, RowAllocation>();
  for (const line of preview.lines) {
    const input = inputs[line.index];
    if (!input) {
      continue;
    }
    result.set(input.row, {
      productType: line.product_type,
      manual: input.lot_no !== null,
      allocations: line.allocations,
      shortage: positive(line.shortage) ? line.shortage : undefined,
    });
  }
  return result;
}

/** 建議批號 API 回傳的列錯誤（`details.lines`，`index` 為輸入的位置）→ 表單明細位置 → 欄位錯誤。 */
export interface PreviewRowError {
  row: number;
  field_key: string | null;
  message: string;
}

export function previewRowErrors(
  inputs: readonly PreviewInput[],
  details: unknown,
): PreviewRowError[] {
  if (typeof details !== 'object' || details === null) {
    return [];
  }
  const lines = (details as Record<string, unknown>).lines;
  if (!Array.isArray(lines)) {
    return [];
  }
  return lines.flatMap((l: unknown) => {
    if (typeof l !== 'object' || l === null) {
      return [];
    }
    const e = l as Record<string, unknown>;
    const input = typeof e.index === 'number' ? inputs[e.index] : undefined;
    if (!input || typeof e.message !== 'string') {
      return [];
    }
    return [
      {
        row: input.row,
        field_key: typeof e.field_key === 'string' ? e.field_key : null,
        message: e.message,
      },
    ];
  });
}

/** 手動指定批號的一個批號與數量（「指定批號」對話框）。 */
export interface LotPick {
  lot_no: string;
  qty: string;
}

/**
 * 「指定批號」對話框的檢查：各批號的數量合計必須等於該列的數量，且每個批號不超過剩餘數量。
 * 沒有問題時回傳 `undefined`。`remaining` 鍵為批號（不分大小寫）。
 */
export function lotPicksError(
  picks: readonly LotPick[],
  lineQty: unknown,
  remaining: ReadonlyMap<string, string>,
): string | undefined {
  const used = picks.filter((p) => positive(p.qty));
  if (used.length === 0) {
    return '請至少輸入一個批號的數量';
  }
  for (const p of used) {
    const left = remaining.get(p.lot_no.toLowerCase());
    if (left !== undefined && sub(parse(p.qty)!, parse(left) ?? ZERO).units > 0n) {
      return `批號 ${p.lot_no} 只剩 ${trimQty(left)}`;
    }
  }
  const sum = used.reduce<Dec>((acc, p) => add(acc, parse(p.qty)!), ZERO);
  const need = parse(lineQty) ?? ZERO;
  const diff = sub(sum, need);
  if (diff.units !== 0n) {
    return `批號數量合計 ${trimQty(format(sum, AMOUNT_SCALE))}，與出貨數量 ${trimQty(
      format(need, AMOUNT_SCALE),
    )} 不符`;
  }
  return undefined;
}

/** 數量去掉小數尾端的 0（`"5.5000"` → `"5.5"`）。 */
function trimQty(raw: string): string {
  return raw.includes('.') ? raw.replace(/\.?0+$/, '') : raw;
}

/**
 * 指定批號後，把一列拆成每個批號一列（§8.9「出貨的批號」）：其他欄位（訂單明細、商品、單價、折扣率、備註）
 * 沿用原來的列，數量與批號依 `picks`（數量為 0 的批號略過）。
 */
export function splitByLots<T extends Record<string, unknown>>(row: T, picks: readonly LotPick[]) {
  return picks
    .filter((p) => positive(p.qty))
    .map((p, i) => {
      const next: Record<string, unknown> = {
        ...row,
        [SALES_FIELDS.qty]: p.qty,
        [SALES_FIELDS.lotNo]: p.lot_no,
      };
      if (i > 0) {
        // 拆出的新列沒有明細 id（存檔時新增）
        delete next.__id;
      }
      return next as T;
    });
}

/**
 * 手動指定批號的列中數量超過批號可用數量的列（存檔前提示用）：位置 → 提示文字。
 * 依建議批號 API 的結果（同一批號出現在多列時依序扣，與過帳相同）。
 */
export function manualLotShortages(
  allocations: ReadonlyMap<number, RowAllocation>,
  lotNos: ReadonlyMap<number, string>,
): Map<number, string> {
  const result = new Map<number, string>();
  for (const [row, a] of allocations) {
    if (a.manual && a.shortage) {
      result.set(row, `批號 ${lotNos.get(row) ?? ''} 的可用數量不足，還差 ${trimQty(a.shortage)}`);
    }
  }
  return result;
}

// ---- 過帳後的批號扣除 ----

/** 出貨單實際扣用的一個批號（過帳後顯示）。 */
export interface UsedLot {
  /** 明細在表單中的位置（找不到時為 `undefined`）。 */
  row?: number;
  lineId: string;
  productId: string;
  product: string;
  lotNo: string;
  qty: string;
  unitCostTwd: string;
  /** `round(qty × unit_cost_twd, 2)`。 */
  costTwd: string;
}

/**
 * 庫存異動（`txn_type = shipment`、來源為此出貨單；或 `sales_return`、來源為此退回單）→
 * 扣用（加回）的批號，依明細順序、處理順序（異動由新到舊排列，反轉後為處理的順序）。
 * `lineIds` 為明細 id，依表單順序；數量一律為正數。
 */
export function usedLots(
  txns: readonly InventoryTransaction[],
  lineIds: readonly string[],
  txnType: InventoryTransaction['txn_type'] = 'shipment',
): UsedLot[] {
  const order = new Map(lineIds.map((id, i) => [id, i]));
  return txns
    .filter((t) => t.txn_type === txnType)
    .slice()
    .reverse()
    .map((t) => {
      const qty = parse(t.qty_change) ?? ZERO;
      const abs = qty.units < 0n ? { ...qty, units: -qty.units } : qty;
      const cost = parse(t.unit_cost_twd) ?? ZERO;
      return {
        row: order.get(t.ref_line_id),
        lineId: t.ref_line_id,
        productId: t.product_id,
        product: `${t.product_code} ${t.product_name}`,
        lotNo: t.lot_no,
        qty: format(abs, AMOUNT_SCALE),
        unitCostTwd: t.unit_cost_twd,
        costTwd: format(round(mul(abs, cost), 2), 2),
      };
    })
    .sort((a, b) => (a.row ?? Number.MAX_SAFE_INTEGER) - (b.row ?? Number.MAX_SAFE_INTEGER));
}

// ---- 銷貨退回 ----

/** 出貨單不能退回的原因（§8.9「銷貨退回」）；可以退回時回傳 `undefined`。 */
export function shipmentNotReturnable(status: DocumentStatus | undefined): string | undefined {
  switch (status) {
    case 'posted':
      return undefined;
    case 'void':
      return '出貨單已作廢';
    default:
      return '出貨單尚未過帳';
  }
}

/**
 * 每一列出貨明細的可退數量（4 位小數），鍵為出貨明細 id：
 * `出貨明細的 qty − 該出貨明細在其他「已過帳」銷貨退回中的 qty 合計`。
 *
 * `postedReturns` 為指向此出貨單、已過帳的銷貨退回（含明細）；`excludeId` 的退回不計入。
 */
export function salesReturnableQty(
  shipment: Pick<DataRecord, 'lines'>,
  postedReturns: readonly Pick<DataRecord, 'id' | 'lines'>[],
  excludeId?: string,
): Map<string, string> {
  const returned = new Map<string, Dec>();
  for (const r of postedReturns) {
    if (r.id === excludeId) {
      continue;
    }
    for (const line of r.lines?.[SALES_RETURN_LINE] ?? []) {
      const id = line.data[SALES_FIELDS.shipmentLine];
      const qty = parse(line.data[SALES_FIELDS.qty]);
      if (typeof id === 'string' && qty) {
        returned.set(id, add(returned.get(id) ?? ZERO, qty));
      }
    }
  }
  const result = new Map<string, string>();
  for (const line of shipment.lines?.[SHIPMENT_LINE] ?? []) {
    const qty = parse(line.data[SALES_FIELDS.qty]);
    if (qty) {
      result.set(line.id, format(sub(qty, returned.get(line.id) ?? ZERO), AMOUNT_SCALE));
    }
  }
  return result;
}

/**
 * 出貨單帶入銷貨退回的明細（JSON 存值）：可退數量大於 0 的每一列，帶入出貨明細、商品、單價、折扣率
 * （商品、單價、折扣率為系統維護，只用於顯示與金額預覽，存檔時由伺服器帶入），數量為可退數量。
 */
export function returnLinesFromShipment(
  shipment: Pick<DataRecord, 'lines'>,
  returnable: ReadonlyMap<string, string>,
): RecordData[] {
  return (shipment.lines?.[SHIPMENT_LINE] ?? []).flatMap((line) => {
    const qty = returnable.get(line.id);
    if (!positive(qty)) {
      return [];
    }
    return [
      {
        [SALES_FIELDS.shipmentLine]: line.id,
        [SALES_FIELDS.product]: line.data[SALES_FIELDS.product] ?? null,
        [SALES_FIELDS.unitPrice]: line.data[SALES_FIELDS.unitPrice] ?? null,
        [SALES_FIELDS.discountRate]: line.data[SALES_FIELDS.discountRate] ?? '0',
        [SALES_FIELDS.qty]: qty!,
      },
    ];
  });
}
