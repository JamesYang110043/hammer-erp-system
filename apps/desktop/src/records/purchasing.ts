import type {
  DocumentStatus,
  EffectiveRate_Serialize as EffectiveRate,
  EntityDefinition,
  FieldDefinition,
  Record as DataRecord,
} from '../bindings';
import { PURCHASE_ORDER } from './documents';
import { add, format, mul, parse, round, sub } from './decimal';
import type { Dec } from './decimal';
import { formFields } from './form';
import type { RecordData } from './form';

/**
 * 進貨模組的畫面規則（docs/architecture.md §8.8）：採購單、進貨單的幣別與匯率帶入、
 * 金額預覽（與伺服器 `hammer_core::purchasing::amounts` 相同的計算）、採購單轉進貨單。
 *
 * 金額一律以字串與 `bigint` 計算，不經過浮點數（D9）。
 */

export { PURCHASE_ORDER };
export const PURCHASE_ORDER_LINE = 'purchase_order_line';
export const GOODS_RECEIPT = 'goods_receipt';
export const GOODS_RECEIPT_LINE = 'goods_receipt_line';
export const PURCHASE_RETURN = 'purchase_return';
export const PURCHASE_RETURN_LINE = 'purchase_return_line';

/** 「進貨」選單分組列出的系統單據，依此順序。 */
export const PURCHASING_ENTITY_KEYS: readonly string[] = [
  PURCHASE_ORDER,
  GOODS_RECEIPT,
  PURCHASE_RETURN,
];

/** 選單「進貨」分組的標題。 */
export const PURCHASING_GROUP_LABEL = '進貨';

/** 「進貨」分組列出的資料類別：{@link PURCHASING_ENTITY_KEYS} 中有效的資料類別。 */
export function purchasingEntities(entities: readonly EntityDefinition[]): EntityDefinition[] {
  return PURCHASING_ENTITY_KEYS.flatMap((key) => {
    const entity = entities.find((e) => e.key === key && e.is_active);
    return entity ? [entity] : [];
  });
}

/** 欄位代碼。 */
export const PURCHASING_FIELDS = {
  supplier: 'supplier_id',
  purchaseOrder: 'purchase_order_id',
  currency: 'currency_code',
  rate: 'exchange_rate',
  taxType: 'tax_type',
  subtotal: 'subtotal',
  taxAmount: 'tax_amount',
  total: 'total',
  totalTwd: 'total_twd',
  product: 'product_id',
  qty: 'qty',
  unitPrice: 'unit_price',
  amount: 'amount',
  receivedQty: 'received_qty',
  lotNo: 'lot_no',
  unitCostTwd: 'unit_cost_twd',
  purchaseOrderLine: 'purchase_order_line_id',
  goodsReceipt: 'goods_receipt_id',
  goodsReceiptLine: 'goods_receipt_line_id',
} as const;

/** 有幣別、匯率與金額計算的單據（採購單、進貨單）。 */
export interface PricedDocument {
  /** 匯率的適用日期欄位。 */
  dateField: string;
  /** 明細子表代碼。 */
  lineKey: string;
}

const PRICED: Record<string, PricedDocument> = {
  [PURCHASE_ORDER]: { dateField: 'order_date', lineKey: PURCHASE_ORDER_LINE },
  [GOODS_RECEIPT]: { dateField: 'receipt_date', lineKey: GOODS_RECEIPT_LINE },
};

/** 單據是否有幣別、匯率與金額計算；不是時回傳 `undefined`。 */
export function pricedDocument(entityKey: string): PricedDocument | undefined {
  return PRICED[entityKey];
}

/**
 * 有金額預覽的單據的明細子表代碼：採購單、進貨單，以及進貨退回（幣別、匯率、稅別沿用進貨單，
 * 由伺服器帶入，畫面只預覽金額）。其他單據回傳 `undefined`。
 */
export function amountLineKey(entityKey: string): string | undefined {
  return entityKey === PURCHASE_RETURN ? PURCHASE_RETURN_LINE : PRICED[entityKey]?.lineKey;
}

/**
 * 明細中不顯示（`is_visible = false`）、但由桌面程式帶入並送出的欄位：
 * 進貨明細的採購明細（採購單轉進貨單時帶入）、進貨退回明細的進貨明細（選擇進貨單時帶入）、
 * 出貨明細的訂單明細（訂單轉出貨單時帶入）。
 */
const CARRIED_LINE_FIELDS: Record<string, readonly string[]> = {
  [GOODS_RECEIPT_LINE]: [PURCHASING_FIELDS.purchaseOrderLine],
  [PURCHASE_RETURN_LINE]: [PURCHASING_FIELDS.goodsReceiptLine],
  // 出貨明細的訂單明細（訂單轉出貨單時帶入，§8.9）
  shipment_line: ['sales_order_line_id'],
  // 銷貨退回明細的出貨明細（選擇出貨單時帶入）
  sales_return_line: ['shipment_line_id'],
};

/**
 * 明細子表在表單中的欄位：顯示的欄位（`formFields`），加上桌面程式帶入的隱藏欄位
 * （{@link CARRIED_LINE_FIELDS}，表格不顯示，`LinesTable` 依 `is_visible` 略過）。
 */
export function lineFormFields(
  detailKey: string,
  fields: readonly FieldDefinition[],
): FieldDefinition[] {
  const carried = CARRIED_LINE_FIELDS[detailKey] ?? [];
  const shown = formFields(fields);
  const hidden = fields.filter(
    (f) => f.is_active && !f.is_visible && carried.includes(f.field_key),
  );
  return [...shown, ...hidden];
}

/** 單據幣別金額的小數位數（`NUMERIC(18,4)`）。 */
export const AMOUNT_SCALE = 4;
/** 台幣合計的小數位數。 */
export const TWD_SCALE = 2;
/** 台幣單位成本的小數位數。 */
export const COST_SCALE = 4;
/** 匯率的小數位數。 */
export const RATE_SCALE = 6;

/**
 * 明細金額：`round(qty × unit_price, d)`，4 位小數。數量或單價不是數字時回傳 `undefined`。
 */
export function lineAmount(qty: unknown, unitPrice: unknown, decimalPlaces: number) {
  const q = parse(qty);
  const p = parse(unitPrice);
  if (!q || !p) {
    return undefined;
  }
  return format(round(mul(q, p), decimalPlaces), AMOUNT_SCALE);
}

/** 單據的合計（與伺服器存檔時的計算相同）。 */
export interface Totals {
  subtotal: string;
  tax_amount: string;
  total: string;
  /** 匯率留空時為 `null`。 */
  total_twd: string | null;
}

/**
 * 依明細金額（{@link lineAmount} 的結果）計算合計：`subtotal = Σ amount`、
 * `tax_amount = round(subtotal × 稅率, d)`、`total = subtotal + tax_amount`、
 * `total_twd = round(total × exchange_rate, 2)`。
 */
export function documentTotals(
  amounts: readonly string[],
  taxRate: string,
  decimalPlaces: number,
  exchangeRate: unknown,
): Totals {
  const zero: Dec = { units: 0n, scale: AMOUNT_SCALE };
  const subtotal = amounts.reduce<Dec>((sum, a) => add(sum, parse(a) ?? zero), zero);
  const rate = parse(taxRate) ?? zero;
  const tax = round(mul(subtotal, rate), decimalPlaces);
  const total = add(subtotal, tax);
  const fx = parse(exchangeRate);
  return {
    subtotal: format(subtotal, AMOUNT_SCALE),
    tax_amount: format(tax, AMOUNT_SCALE),
    total: format(total, AMOUNT_SCALE),
    total_twd: fx && fx.units > 0n ? format(round(mul(total, fx), TWD_SCALE), TWD_SCALE) : null,
  };
}

/** 台幣單位成本：`round(unit_price × exchange_rate, 4)`（過帳時寫入）。 */
export function unitCostTwd(unitPrice: unknown, exchangeRate: unknown): string | undefined {
  const p = parse(unitPrice);
  const r = parse(exchangeRate);
  return p && r ? format(round(mul(p, r), COST_SCALE), COST_SCALE) : undefined;
}

/** 新增時稅別的預設值：本位幣為應稅，外幣為零稅率（§8.8「金額與稅額」）。 */
export function defaultTaxType(isBaseCurrency: boolean): string {
  return isBaseCurrency ? 'taxable' : 'zero_rated';
}

/** 兩個數值字串是否相等（忽略小數尾端的 0）；任一個不是數字時為 `false`。 */
export function sameDecimal(a: unknown, b: unknown): boolean {
  const x = parse(a);
  const y = parse(b);
  return Boolean(x && y && sub(x, y).units === 0n);
}

/** 匯率的顯示：去掉小數尾端的 0（`"32.150000"` → `"32.15"`）。 */
export function trimRate(raw: string): string {
  return raw.includes('.') ? raw.replace(/\.?0+$/, '') : raw;
}

// ---- 匯率提示 ----

/** 匯率欄位下方的提示。 */
export interface RateNote {
  type: 'secondary' | 'warning';
  text: string;
}

/**
 * 匯率欄位的提示（§8.8「幣別與匯率」）：
 *
 * - 本位幣：匯率固定為 1
 * - 找到適用匯率：目前的值與適用匯率相同時顯示匯率日期；不同時顯示為手動輸入，並列出適用匯率
 * - 找不到：提示輸入匯率（草稿可以留空存檔，但確認、過帳時必須有匯率）
 */
export function rateNote(
  effective: Pick<EffectiveRate, 'currency_code' | 'date' | 'rate' | 'rate_date'> | undefined,
  value: unknown,
  isBaseCurrency: boolean,
): RateNote | undefined {
  if (isBaseCurrency) {
    return { type: 'secondary', text: '本位幣，匯率固定為 1' };
  }
  if (!effective) {
    return undefined;
  }
  if (!effective.rate || !effective.rate_date) {
    return {
      type: 'warning',
      text: `找不到 ${effective.currency_code} 在 ${effective.date} 以前的匯率，請輸入匯率`,
    };
  }
  const source = `${effective.rate_date} 的匯率 ${trimRate(effective.rate)}`;
  if (value === undefined || value === null || value === '') {
    return { type: 'warning', text: `尚未輸入匯率（適用匯率為 ${source}）` };
  }
  return sameDecimal(value, effective.rate)
    ? { type: 'secondary', text: `匯率日期：${effective.rate_date}` }
    : { type: 'secondary', text: `手動輸入；適用匯率為 ${source}` };
}

// ---- 採購單轉進貨單 ----

/** 採購單不能進貨的原因（§8.8「採購單轉進貨單」）；可以進貨時回傳 `undefined`。 */
export function orderNotReceivable(status: DocumentStatus | undefined): string | undefined {
  switch (status) {
    case 'confirmed':
    case 'partially_received':
      return undefined;
    case 'closed':
      return '採購單已結案';
    case 'void':
      return '採購單已作廢';
    default:
      return '採購單尚未確認';
  }
}

/** 未進貨數量：`qty − received_qty`（4 位小數）；不是數字時為 `undefined`。 */
export function unreceivedQty(qty: unknown, receivedQty: unknown): string | undefined {
  const q = parse(qty);
  if (!q) {
    return undefined;
  }
  const r = parse(receivedQty) ?? { units: 0n, scale: 0 };
  return format(sub(q, r), AMOUNT_SCALE);
}

/**
 * 採購單帶入進貨單的明細（JSON 存值）：未進貨數量大於 0 的每一列，帶入採購明細、商品、單價，
 * 數量為未進貨數量，批號留空。
 */
export function receiptLinesFromOrder(order: Pick<DataRecord, 'lines'>): RecordData[] {
  const lines = order.lines?.[PURCHASE_ORDER_LINE] ?? [];
  return lines.flatMap((line) => {
    const qty = unreceivedQty(
      line.data[PURCHASING_FIELDS.qty],
      line.data[PURCHASING_FIELDS.receivedQty],
    );
    if (!qty || !(parse(qty)!.units > 0n)) {
      return [];
    }
    return [
      {
        [PURCHASING_FIELDS.purchaseOrderLine]: line.id,
        [PURCHASING_FIELDS.product]: line.data[PURCHASING_FIELDS.product] ?? null,
        [PURCHASING_FIELDS.qty]: qty,
        [PURCHASING_FIELDS.unitPrice]: line.data[PURCHASING_FIELDS.unitPrice] ?? null,
        [PURCHASING_FIELDS.lotNo]: null,
      },
    ];
  });
}

// ---- 進貨退回 ----

/** 進貨單不能退回的原因（§8.8「進貨退回」）；可以退回時回傳 `undefined`。 */
export function receiptNotReturnable(status: DocumentStatus | undefined): string | undefined {
  switch (status) {
    case 'posted':
      return undefined;
    case 'void':
      return '進貨單已作廢';
    default:
      return '進貨單尚未過帳';
  }
}

/**
 * 每一列進貨明細的可退數量（4 位小數），鍵為進貨明細 id：
 * `進貨明細的 qty − 該進貨明細在其他「已過帳」進貨退回中的 qty 合計`。
 *
 * `postedReturns` 為指向此進貨單、已過帳的進貨退回（含明細）；`excludeId` 的退回不計入。
 */
export function returnableQty(
  receipt: Pick<DataRecord, 'lines'>,
  postedReturns: readonly Pick<DataRecord, 'id' | 'lines'>[],
  excludeId?: string,
): Map<string, string> {
  const returned = new Map<string, Dec>();
  for (const r of postedReturns) {
    if (r.id === excludeId) {
      continue;
    }
    for (const line of r.lines?.[PURCHASE_RETURN_LINE] ?? []) {
      const id = line.data[PURCHASING_FIELDS.goodsReceiptLine];
      const qty = parse(line.data[PURCHASING_FIELDS.qty]);
      if (typeof id === 'string' && qty) {
        returned.set(id, add(returned.get(id) ?? { units: 0n, scale: 0 }, qty));
      }
    }
  }
  const result = new Map<string, string>();
  for (const line of receipt.lines?.[GOODS_RECEIPT_LINE] ?? []) {
    const qty = parse(line.data[PURCHASING_FIELDS.qty]);
    if (qty) {
      result.set(
        line.id,
        format(sub(qty, returned.get(line.id) ?? { units: 0n, scale: 0 }), AMOUNT_SCALE),
      );
    }
  }
  return result;
}

/**
 * 進貨單帶入進貨退回的明細（JSON 存值）：可退數量大於 0 的每一列，帶入進貨明細、商品、批號、單價
 * （商品、批號、單價為系統維護，只用於顯示與金額預覽，存檔時由伺服器帶入），數量為可退數量。
 */
export function returnLinesFromReceipt(
  receipt: Pick<DataRecord, 'lines'>,
  returnable: ReadonlyMap<string, string>,
): RecordData[] {
  return (receipt.lines?.[GOODS_RECEIPT_LINE] ?? []).flatMap((line) => {
    const qty = returnable.get(line.id);
    if (!qty || !(parse(qty)!.units > 0n)) {
      return [];
    }
    return [
      {
        [PURCHASING_FIELDS.goodsReceiptLine]: line.id,
        [PURCHASING_FIELDS.product]: line.data[PURCHASING_FIELDS.product] ?? null,
        [PURCHASING_FIELDS.lotNo]: line.data[PURCHASING_FIELDS.lotNo] ?? null,
        [PURCHASING_FIELDS.unitPrice]: line.data[PURCHASING_FIELDS.unitPrice] ?? null,
        [PURCHASING_FIELDS.qty]: qty,
      },
    ];
  });
}
