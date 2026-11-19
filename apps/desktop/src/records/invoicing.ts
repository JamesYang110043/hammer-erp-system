import dayjs from 'dayjs';

import type { EntityDefinition, Record as DataRecord } from '../bindings';
import { add, div, format, mul, parse, round, sub, ZERO } from './decimal';
import type { Dec } from './decimal';
import { groupThousands } from './display';
import type { DocumentAction } from './documents';
import type { RecordData } from './form';
import { salesLineAmount, salesTotals } from './sales';
import type { SalesTotals } from './sales';

/**
 * 發票模組的畫面規則（docs/architecture.md §8.11）：銷項發票、銷貨折讓、進項憑證、進貨折讓。
 *
 * 發票在別處開立，系統只登錄號碼與金額；金額計算與伺服器的 `sales::amounts` 相同
 * （三聯式、折讓單價未稅，二聯式單價含稅），再套用手動修正的稅額。
 */

export const SALES_INVOICE = 'sales_invoice';
export const SALES_INVOICE_LINE = 'sales_invoice_line';
export const SALES_INVOICE_SHIPMENT = 'sales_invoice_shipment';
export const SALES_ALLOWANCE = 'sales_allowance';
export const SALES_ALLOWANCE_LINE = 'sales_allowance_line';
export const PURCHASE_INVOICE = 'purchase_invoice';
export const PURCHASE_INVOICE_RECEIPT = 'purchase_invoice_receipt';
export const PURCHASE_ALLOWANCE = 'purchase_allowance';

/** 左側選單「發票」分組列出的資料類別（依序）。 */
export const INVOICING_ENTITY_KEYS: readonly string[] = [
  SALES_INVOICE,
  SALES_ALLOWANCE,
  PURCHASE_INVOICE,
  PURCHASE_ALLOWANCE,
];

/** 選單「發票」分組的標題。 */
export const INVOICING_GROUP_LABEL = '發票';

/** 「發票」分組：{@link INVOICING_ENTITY_KEYS} 中有效的系統資料類別。 */
export function invoicingEntities(entities: readonly EntityDefinition[]): EntityDefinition[] {
  return INVOICING_ENTITY_KEYS.flatMap((key) => {
    const entity = entities.find((e) => e.key === key && e.is_active && e.is_system);
    return entity ? [entity] : [];
  });
}

/** 是否為發票模組的單據（作廢需要日期與原因）。 */
export function isInvoicingDocument(entityKey: string): boolean {
  return INVOICING_ENTITY_KEYS.includes(entityKey);
}

/** 欄位代碼。 */
export const INVOICING_FIELDS = {
  invoiceType: 'invoice_type',
  invoiceNo: 'invoice_no',
  invoiceDate: 'invoice_date',
  taxPeriod: 'tax_period',
  customer: 'customer_id',
  buyerTaxId: 'buyer_tax_id',
  buyerName: 'buyer_name',
  buyerAddress: 'buyer_address',
  taxType: 'tax_type',
  taxManual: 'tax_amount_manual',
  subtotal: 'subtotal',
  taxAmount: 'tax_amount',
  total: 'total',
  shipment: 'shipment_id',
  salesInvoice: 'sales_invoice_id',
  salesReturn: 'sales_return_id',
  supplier: 'supplier_id',
  sellerTaxId: 'seller_tax_id',
  docType: 'doc_type',
  goodsReceipt: 'goods_receipt_id',
  purchaseInvoice: 'purchase_invoice_id',
  purchaseReturn: 'purchase_return_id',
  allowanceDate: 'allowance_date',
  voidDate: 'void_date',
  product: 'product_id',
  description: 'description',
  qty: 'qty',
  unitPrice: 'unit_price',
  amount: 'amount',
} as const;

const F = INVOICING_FIELDS;

/** 海關代徵營業稅繳納證。 */
export const CUSTOMS = 'customs';

/** 二聯式（單價含稅）。 */
export const DUPLICATE = 'duplicate';

/** 有明細金額的發票模組單據 → 明細子表。 */
const PRICED: Record<string, string> = {
  [SALES_INVOICE]: SALES_INVOICE_LINE,
  [SALES_ALLOWANCE]: SALES_ALLOWANCE_LINE,
};

/** 有明細金額時的明細子表代碼。 */
export function invoicingLineKey(entityKey: string): string | undefined {
  return PRICED[entityKey];
}

/** 已過帳（`posted`）的顯示名稱：發票、折讓為「已開立」，進項憑證為「已登錄」。 */
export function postedLabel(entityKey: string): string | undefined {
  if (entityKey === PURCHASE_INVOICE) {
    return '已登錄';
  }
  return isInvoicingDocument(entityKey) ? '已開立' : undefined;
}

/** 動作按鈕的名稱：發票模組的過帳為「開立」（進項憑證為「登錄」）。 */
export function invoicingActionLabel(
  entityKey: string,
  action: DocumentAction,
): string | undefined {
  if (!isInvoicingDocument(entityKey) || action !== 'post') {
    return undefined;
  }
  return entityKey === PURCHASE_INVOICE ? '登錄' : '開立';
}

/** 執行動作前確認對話框的說明。 */
export function invoicingConfirmText(
  entityKey: string,
  action: DocumentAction,
): string | undefined {
  if (!isInvoicingDocument(entityKey)) {
    return undefined;
  }
  if (action === 'post') {
    return entityKey === SALES_INVOICE || entityKey === PURCHASE_INVOICE
      ? '開立後會檢查號碼不重複、鎖定關聯的單據，單據不能再修改或刪除。'
      : '開立後折讓金額計入原發票的累計折讓，單據不能再修改或刪除。';
  }
  return '作廢後單據不能再恢復；請輸入作廢原因。';
}

/** 期別（`YYYYMM`，該期第一個月為奇數月）；日期格式不正確時為 `undefined`。 */
export function taxPeriod(date: unknown): string | undefined {
  if (typeof date !== 'string') {
    return undefined;
  }
  const m = /^(\d{4})-(\d{2})-\d{2}$/.exec(date);
  if (!m) {
    return undefined;
  }
  const month = Number(m[2]);
  if (month < 1 || month > 12) {
    return undefined;
  }
  const first = month - ((month - 1) % 2);
  return `${m[1]}${String(first).padStart(2, '0')}`;
}

/** 期別的顯示文字：`202611` → `2026 年 11–12 月`。 */
export function taxPeriodLabel(period: unknown): string | undefined {
  if (typeof period !== 'string' || !/^\d{6}$/.test(period)) {
    return undefined;
  }
  const year = period.slice(0, 4);
  const first = Number(period.slice(4));
  return `${year} 年 ${first}–${first + 1} 月`;
}

/**
 * 跨期作廢的警告（不阻擋作廢，§8.11「開立與作廢」）：作廢日期的期別與單據的期別不同時回傳警告文字。
 */
export function crossPeriodWarning(voidDate: unknown, documentPeriod: unknown): string | undefined {
  const period = taxPeriod(voidDate);
  const label = taxPeriodLabel(documentPeriod);
  if (!period || !label || period === documentPeriod) {
    return undefined;
  }
  return `作廢日期與發票不同期（發票 ${label}），請確認是否需要另外處理申報`;
}

const MAX_TAX_DIFF: Dec = { units: 1n, scale: 0 };

/**
 * 依明細金額計算合計，再套用手動稅額（與伺服器 `invoicing::amounts::apply_manual_tax` 相同）：
 * 未稅（三聯式、折讓）的總計 = 未稅 + 稅額；含稅（二聯式）的總計不變、未稅 = 總計 − 稅額。
 * 手動稅額不是數字、稅率為 0、或與系統計算相差超過 1 元時不套用（存檔時由伺服器回報錯誤）。
 */
export function invoiceTotals(
  amounts: readonly string[],
  taxRate: string,
  taxIncluded: boolean,
  decimalPlaces: number,
  manual: unknown,
): SalesTotals {
  const computed = salesTotals(amounts, taxRate, taxIncluded, decimalPlaces);
  const m = typeof manual === 'string' && manual.trim() !== '' ? parse(manual) : undefined;
  const rate = parse(taxRate) ?? ZERO;
  if (!m || rate.units === 0n) {
    return computed;
  }
  const diff = sub(m, parse(computed.tax_amount) ?? ZERO);
  const abs = diff.units < 0n ? { ...diff, units: -diff.units } : diff;
  if (compare(abs, MAX_TAX_DIFF) > 0) {
    return computed;
  }
  if (taxIncluded) {
    return {
      subtotal: format(sub(parse(computed.total) ?? ZERO, m), 4),
      tax_amount: format(m, 4),
      total: computed.total,
    };
  }
  return {
    subtotal: computed.subtotal,
    tax_amount: format(m, 4),
    total: format(add(parse(computed.subtotal) ?? ZERO, m), 4),
  };
}

function compare(a: Dec, b: Dec): number {
  const d = sub(a, b);
  return d.units === 0n ? 0 : d.units > 0n ? 1 : -1;
}

/** 發票、折讓明細金額：`round(qty × unit_price, d)`（沒有折扣率）。 */
export function invoiceLineAmount(
  qty: unknown,
  unitPrice: unknown,
  decimalPlaces: number,
): string | undefined {
  return salesLineAmount(qty, unitPrice, '0', decimalPlaces);
}

/**
 * 出貨（退回）明細帶入發票（折讓）明細的單價（§8.11「關聯出貨單、進貨單」）：
 * `p = unit_price × (1 − discount_rate)`，再依發票與出貨單的含稅基準換算，4 位小數。
 */
export function convertedUnitPrice(
  unitPrice: unknown,
  discountRate: unknown,
  sourceTaxIncluded: boolean,
  targetTaxIncluded: boolean,
  taxRate: string,
): string | undefined {
  const price = parse(unitPrice);
  if (!price) {
    return undefined;
  }
  const discount = parse(discountRate) ?? ZERO;
  let p = mul(price, sub({ units: 1n, scale: 0 }, discount));
  const factor = add({ units: 1n, scale: 0 }, parse(taxRate) ?? ZERO);
  if (sourceTaxIncluded && !targetTaxIncluded) {
    p = div(p, factor, 4) ?? p;
  } else if (!sourceTaxIncluded && targetTaxIncluded) {
    p = mul(p, factor);
  }
  return format(round(p, 4), 4);
}

/** 商品的品名（帶入發票明細的 `description`）：關聯標題去掉料號。 */
export function productDescription(title: string | undefined): string {
  if (!title) {
    return '';
  }
  const space = title.indexOf(' ');
  return space > 0 ? title.slice(space + 1) : title;
}

/**
 * 出貨單（或銷貨退回）的明細轉成發票（折讓）明細的 `data`。`targetTaxIncluded` 為發票是否含稅
 * （二聯式），折讓一律未稅。
 */
export function linesFromSource(
  source: Pick<DataRecord, 'data' | 'lines'>,
  lineKey: string,
  targetTaxIncluded: boolean,
  taxRate: string,
): RecordData[] {
  const sourceIncluded = source.data.tax_included === true;
  return (source.lines?.[lineKey] ?? []).map((line) => ({
    [F.product]: line.data.product_id ?? null,
    [F.description]: productDescription(line.relations?.product_id?.title),
    [F.qty]: line.data.qty ?? null,
    [F.unitPrice]:
      convertedUnitPrice(
        line.data.unit_price,
        line.data.discount_rate,
        sourceIncluded,
        targetTaxIncluded,
        taxRate,
      ) ?? null,
  }));
}

/** 可折讓金額：發票總計 − 已開立折讓的總計合計（4 位小數，不小於 0）。 */
export function availableAllowance(
  invoiceTotal: unknown,
  postedTotals: readonly unknown[],
): string | undefined {
  const total = parse(invoiceTotal);
  if (!total) {
    return undefined;
  }
  const used = postedTotals.reduce<Dec>((acc, t) => add(acc, parse(t) ?? ZERO), ZERO);
  const available = sub(total, used);
  return format(available.units < 0n ? ZERO : available, 4);
}

/** 金額合計（4 位小數）；不是數字的值略過。 */
export function sumAmounts(values: readonly unknown[]): string {
  return format(
    values.reduce<Dec>((acc, v) => add(acc, parse(v) ?? ZERO), ZERO),
    4,
  );
}

/** 表單上的日期（`Dayjs`）或資料的日期字串 → `YYYY-MM-DD`；沒有值時為 `undefined`。 */
export function dateText(value: unknown): string | undefined {
  if (dayjs.isDayjs(value)) {
    return value.format('YYYY-MM-DD');
  }
  return typeof value === 'string' && value !== '' ? value : undefined;
}

/** 台幣金額的顯示：四捨五入到整數、千分位（`1070.0000` → `1,070`）；不是數字時原樣顯示。 */
export function moneyText(raw: unknown): string {
  const d = parse(raw);
  return d ? groupThousands(format(round(d, 0), 0)) : String(raw ?? '');
}

/**
 * 非草稿單據的說明文字（`documentStatusNote` 的結果）改用發票模組的用語：已過帳 → 已開立（進項憑證為
 * 已登錄）、過帳時間 → 開立時間（登錄時間）。不是發票模組的單據時原樣回傳。
 */
export function invoicingStatusNote(entityKey: string, note: string): string {
  const posted = postedLabel(entityKey);
  if (!posted) {
    return note;
  }
  const verb = posted.slice(1);
  return note.replace('已過帳', posted).replace('過帳時間', `${verb}時間`);
}

/** 沒有明細、由使用者輸入金額的單據（進項憑證、進貨折讓）：總計 = 未稅 + 稅額；不是數字時為 `undefined`。 */
export function enteredTotal(subtotal: unknown, taxAmount: unknown): string | undefined {
  const a = parse(subtotal);
  const b = parse(taxAmount);
  return a && b ? format(add(a, b), 4) : undefined;
}

/** 是否為由使用者輸入金額的單據。 */
export function hasEnteredAmounts(entityKey: string): boolean {
  return entityKey === PURCHASE_INVOICE || entityKey === PURCHASE_ALLOWANCE;
}

/**
 * 依勾選的進貨單建議進項憑證的金額：都是台幣時，以台幣總計合計反推未稅（`round(合計 ÷ (1 + 稅率), 0)`），
 * 稅額 = 合計 − 未稅；有外幣或沒有台幣總計時為 `undefined`（依憑證輸入）。
 */
export function suggestedInvoiceAmounts(
  receipts: readonly { currency_code: string; total_twd: string | null }[],
  taxRate: string,
): { subtotal: string; tax_amount: string } | undefined {
  if (receipts.length === 0 || receipts.some((r) => r.currency_code !== 'TWD' || !r.total_twd)) {
    return undefined;
  }
  const sum = receipts.reduce<Dec>((acc, r) => add(acc, parse(r.total_twd) ?? ZERO), ZERO);
  const subtotal = div(sum, add({ units: 1n, scale: 0 }, parse(taxRate) ?? ZERO), 0) ?? sum;
  return { subtotal: format(subtotal, 0), tax_amount: format(sub(sum, subtotal), 0) };
}
