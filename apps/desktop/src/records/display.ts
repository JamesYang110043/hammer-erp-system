import type { FieldDefinition, RelationRef, SelectOption } from '../bindings';
import { formatTaipei } from '../utils/time';

/**
 * 數值字串加上千分位：`"-1234567.50"` → `"-1,234,567.50"`。
 *
 * 只處理字串，不轉成浮點數（D9）；不是數值格式時原樣回傳。
 */
export function groupThousands(raw: string): string {
  const m = /^([-+]?)(\d+)(\.\d+)?$/.exec(raw.trim());
  if (!m) {
    return raw;
  }
  const [, sign, int, frac] = m;
  return `${sign === '-' ? '-' : ''}${int!.replace(/\B(?=(\d{3})+(?!\d))/g, ',')}${frac ?? ''}`;
}

/**
 * 小數、金額的顯示：補足到欄位的小數位數，再加上千分位。`"1234.5"`（2 位）→ `"1,234.50"`。
 *
 * 小數位數比設定多時不四捨五入（存值已由伺服器依小數位數正規化）。
 */
export function formatDecimal(raw: string, decimalPlaces: number | null | undefined): string {
  const m = /^([-+]?\d+)(?:\.(\d*))?$/.exec(raw.trim());
  if (!m) {
    return raw;
  }
  const [, int, frac = ''] = m;
  const places = Math.max(decimalPlaces ?? 0, frac.length);
  const padded = places > 0 ? `${int}.${frac.padEnd(places, '0')}` : int!;
  return groupThousands(padded);
}

/** 選項值 → 選項定義；找不到（例如選項已移除）時回傳 `undefined`。 */
export function findOption(field: FieldDefinition, value: string): SelectOption | undefined {
  return field.options?.find((o) => o.value === value);
}

/**
 * 欄位值的文字表示（勾選、選項以外的型態）。沒有值時回傳 `undefined`。
 *
 * | 型態 | 顯示 |
 * |---|---|
 * | `integer` | 千分位 |
 * | `decimal`、`money` | 依小數位數補零、千分位 |
 * | `date` | `YYYY-MM-DD` |
 * | `datetime` | 台北時間 `YYYY-MM-DD HH:mm` |
 * | `formula` | 依 `formula_result_type` |
 * | 其他 | 原樣 |
 */
export function formatFieldText(field: FieldDefinition, value: unknown): string | undefined {
  if (value === null || value === undefined || value === '') {
    return undefined;
  }
  const f = valueField(field);
  switch (f.data_type) {
    case 'integer':
      return groupThousands(String(value));
    case 'decimal':
    case 'money':
      return formatDecimal(String(value), f.decimal_places);
    case 'datetime':
      return typeof value === 'string' ? formatTaipei(value, 'YYYY-MM-DD HH:mm') : String(value);
    case 'boolean':
      return value === true ? '是' : '否';
    case 'select':
      return typeof value === 'string' ? (findOption(f, value)?.label ?? value) : String(value);
    case 'multi_select':
      return Array.isArray(value)
        ? value.map((v) => findOption(f, String(v))?.label ?? String(v)).join('、')
        : String(value);
    default:
      return typeof value === 'string' ? value : JSON.stringify(value);
  }
}

/**
 * 顯示、篩選與排序時使用的欄位定義：公式欄位的值依 `formula_result_type` 處理（§7.3），
 * 所以把型態換成結果型態；其他欄位原樣回傳。沒有結果型態的公式欄位（資料不一致）視為文字。
 */
export function valueField(field: FieldDefinition): FieldDefinition {
  if (field.data_type !== 'formula') {
    return field;
  }
  return { ...field, data_type: field.formula_result_type ?? 'text' };
}

/** 數值型態（公式欄位依結果型態）靠右對齊。 */
export function isNumericType(field: FieldDefinition): boolean {
  const { data_type } = valueField(field);
  return data_type === 'integer' || data_type === 'decimal' || data_type === 'money';
}

/**
 * 顯示用的數值文字是否為負數（`"-1,234.50"`、`"-0.5"`）；`"-0"`、`"-0.00"` 不算。
 *
 * 只看字串，不轉成浮點數（D9）。
 */
export function isNegativeNumber(text: string): boolean {
  const m = /^-([\d,]*)(?:\.(\d*))?$/.exec(text.trim());
  if (!m) {
    return false;
  }
  return /[1-9]/.test(`${m[1]}${m[2] ?? ''}`);
}

/** 已刪除的關聯資料加在標題後面的文字。 */
export const DELETED_SUFFIX = '（已刪除）';

/** 關聯資料的顯示文字：標題；目標資料已刪除時加上「（已刪除）」。 */
export function relationLabel(ref: Pick<RelationRef, 'title' | 'deleted'>): string {
  return ref.deleted ? `${ref.title}${DELETED_SUFFIX}` : ref.title;
}

/** 公式欄位要存檔後才會更新的標示（關聯引用的值，預覽時沒有最新的關聯資料）。 */
export const FORMULA_PENDING_TEXT = '存檔後更新';

/** 公式錯誤圖示的無障礙標籤，例如「公式錯誤：除數不可為零」。 */
export function formulaErrorLabel(error: string): string {
  return `公式錯誤：${error}`;
}
