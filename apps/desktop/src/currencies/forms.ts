import type { Dayjs } from 'dayjs';

import type {
  CreateCurrencyRequest,
  Currency,
  ExchangeRateQuery,
  UpdateCurrencyRequest,
} from '../bindings';
import { compareDecimal } from '../records/form';

/**
 * 幣別、匯率管理畫面的表單邏輯（docs/architecture.md §8.6 `currencies`、`exchange_rates`）。
 *
 * 前端檢查只是輔助，規則仍以伺服器為準；匯率一律以字串處理，不轉成浮點數（D9）。
 */

// ---- 幣別 ----

/** 幣別代碼：3 個英文字母（小寫由伺服器轉成大寫）。 */
export const CURRENCY_CODE_RE = /^[A-Za-z]{3}$/;

/** 小數位數的上限（`hammer_shared::currency::MAX_DECIMAL_PLACES`）。 */
export const MAX_CURRENCY_DECIMAL_PLACES = 4;

/** 幣別表單的值。 */
export interface CurrencyFormValues {
  code: string;
  name: string;
  symbol?: string | null;
  decimal_places: number;
  is_active: boolean;
}

export function buildCreateCurrency(values: CurrencyFormValues): CreateCurrencyRequest {
  const symbol = values.symbol?.trim() ?? '';
  return {
    code: values.code.trim().toUpperCase(),
    name: values.name.trim(),
    symbol: symbol === '' ? null : symbol,
    decimal_places: values.decimal_places,
  };
}

/**
 * 修改幣別的請求：只送出有變更的欄位，並帶上讀取時的版本號。
 *
 * 符號清空時送出空字串（伺服器視為清除）。沒有任何變更時回傳 `undefined`。
 */
export function buildUpdateCurrency(
  currency: Currency,
  values: Omit<CurrencyFormValues, 'code'>,
): UpdateCurrencyRequest | undefined {
  const name = values.name.trim();
  const symbol = values.symbol?.trim() ?? '';
  const req: UpdateCurrencyRequest = {
    version: currency.version,
    name: name !== currency.name ? name : null,
    symbol: symbol !== (currency.symbol ?? '') ? symbol : null,
    decimal_places:
      values.decimal_places !== currency.decimal_places ? values.decimal_places : null,
    is_active: values.is_active !== currency.is_active ? values.is_active : null,
  };
  const changed = [req.name, req.symbol, req.decimal_places, req.is_active].some((v) => v !== null);
  return changed ? req : undefined;
}

/** 選單選項的標示：「代碼 名稱」，已停用的加上「（已停用）」。 */
export function currencyLabel(currency: Pick<Currency, 'code' | 'name' | 'is_active'>): string {
  return `${currency.code} ${currency.name}${currency.is_active ? '' : '（已停用）'}`;
}

/** 可以設定匯率的幣別（新增匯率的選單）：啟用中、不是本位幣。 */
export function rateCurrencyOptions(currencies: readonly Currency[]) {
  return currencies
    .filter((c) => c.is_active && !c.is_base)
    .map((c) => ({ value: c.code, label: currencyLabel(c) }));
}

/** 匯率列表的幣別篩選：所有外幣（含已停用，已停用的匯率仍可查看）。 */
export function rateFilterOptions(currencies: readonly Currency[]) {
  return currencies
    .filter((c) => !c.is_base)
    .map((c) => ({ value: c.code, label: currencyLabel(c) }));
}

// ---- 匯率 ----

/** 匯率最多 6 位小數（`hammer_shared::exchange_rate::RATE_SCALE`）。 */
export const RATE_SCALE = 6;

/** `NUMERIC(18,6)` 的整數部分最多 12 位數。 */
export const RATE_MAX_INTEGER_DIGITS = 12;

const RATE_RE = /^(\d+)(?:\.(\d+))?$/;

/**
 * 檢查匯率字串，回傳錯誤訊息；正確時回傳 `undefined`。
 *
 * 只接受一般的十進位寫法（例如 `32.15`），不接受正負號、科學記號；
 * 大於 0、最多 {@link RATE_SCALE} 位小數（不自動四捨五入）、整數部分不超過 12 位。
 */
export function validateRateText(raw: string | undefined | null): string | undefined {
  const text = raw?.trim() ?? '';
  if (text === '') {
    return '請輸入匯率';
  }
  const m = RATE_RE.exec(text);
  if (!m) {
    return '請輸入數字，例如 32.15';
  }
  const [, intPart = '', fracPart = ''] = m;
  if (compareDecimal(text, '0')! <= 0) {
    return '匯率必須大於 0';
  }
  if (fracPart.replace(/0+$/, '').length > RATE_SCALE) {
    return `匯率最多 ${RATE_SCALE} 位小數`;
  }
  if (intPart.replace(/^0+/, '').length > RATE_MAX_INTEGER_DIGITS) {
    return '匯率超過可輸入的範圍';
  }
  return undefined;
}

/** 兩個匯率字串是否相同（`32.15` 與 `32.150000` 相同），以字串比較，不轉成浮點數。 */
export function sameRate(a: string, b: string): boolean {
  return compareDecimal(a, b) === 0;
}

/** 匯率列表的篩選條件。 */
export interface ExchangeRateFilters {
  currencyCode?: string;
  /** 日期區間（含開始、含結束）。 */
  dateRange?: [Dayjs | null, Dayjs | null] | null;
}

/** 伺服器的預設每頁筆數。 */
export const DEFAULT_RATE_PAGE_SIZE = 50;

/** 篩選條件與分頁 → `GET /api/v1/exchange-rates` 的參數（日期區間含開始、含結束）。 */
export function buildExchangeRateQuery(
  filters: ExchangeRateFilters,
  page: number,
  pageSize: number,
): ExchangeRateQuery {
  const [start, end] = filters.dateRange ?? [null, null];
  return {
    currency_code: filters.currencyCode ?? null,
    date_from: start ? start.format('YYYY-MM-DD') : null,
    date_to: end ? end.format('YYYY-MM-DD') : null,
    page,
    page_size: pageSize,
  };
}
