import type { FieldDefinition, SelectOption } from '../bindings';
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
 * | 其他 | 原樣 |
 */
export function formatFieldText(field: FieldDefinition, value: unknown): string | undefined {
  if (value === null || value === undefined || value === '') {
    return undefined;
  }
  switch (field.data_type) {
    case 'integer':
      return groupThousands(String(value));
    case 'decimal':
    case 'money':
      return formatDecimal(String(value), field.decimal_places);
    case 'datetime':
      return typeof value === 'string' ? formatTaipei(value, 'YYYY-MM-DD HH:mm') : String(value);
    case 'boolean':
      return value === true ? '是' : '否';
    case 'select':
      return typeof value === 'string' ? (findOption(field, value)?.label ?? value) : String(value);
    case 'multi_select':
      return Array.isArray(value)
        ? value.map((v) => findOption(field, String(v))?.label ?? String(v)).join('、')
        : String(value);
    default:
      return typeof value === 'string' ? value : JSON.stringify(value);
  }
}

/** 數值型態靠右對齊。 */
export function isNumericType(field: FieldDefinition): boolean {
  return (
    field.data_type === 'integer' || field.data_type === 'decimal' || field.data_type === 'money'
  );
}
