import dayjs from 'dayjs';

import { CommandError } from '../api/client';
import type { FieldDefinition } from '../bindings';
import { TAIPEI_OFFSET_MINUTES } from '../utils/time';

/**
 * 資料表單：欄位值在「表單值」與「JSON 存值」之間的轉換，以及與伺服器一致的驗證規則
 * （`crates/core/src/record_data.rs`、docs/architecture.md §7.3、D9）。
 *
 * | data_type | 表單值 | JSON 存值 |
 * |---|---|---|
 * | `text`、`long_text` | string | string |
 * | `integer` | string（輸入框文字） | number |
 * | `decimal`、`money` | string | **string**（不轉成浮點數） |
 * | `date` | Dayjs | `YYYY-MM-DD` |
 * | `datetime` | Dayjs（台北時間） | RFC 3339（`+08:00`） |
 * | `boolean` | boolean | boolean |
 * | `select` | string | string |
 * | `multi_select` | string[] | string[] |
 *
 * 沒有值時表單值為 `undefined`（勾選為 `false`），JSON 存值為 `null`。
 */

/** 表單的值，鍵為 `field_key`。 */
export type RecordFormValues = Record<string, unknown>;

/** JSON 存值，鍵為 `field_key`。 */
export type RecordData = Record<string, unknown>;

/** `integer` 的絕對值上限（`hammer_shared::schema::value::MAX_SAFE_INTEGER`）。 */
export const MAX_SAFE_INTEGER_TEXT = '9007199254740991';

/** 日期時間送出的格式：選擇的時間視為台北時間（D13）。 */
const DATETIME_VALUE_FORMAT = 'YYYY-MM-DDTHH:mm:ss';

/** 表單可以輸入的型態（`relation`、`file`、`formula` 在 M2b、M2c 處理）。 */
export function isFormSupported(field: FieldDefinition): boolean {
  return !['relation', 'file', 'formula'].includes(field.data_type);
}

/** 表單顯示的欄位：有效、設定為顯示、存在 JSON 的欄位，依 `sort_order`。 */
export function formFields(fields: readonly FieldDefinition[]): FieldDefinition[] {
  return fields
    .filter((f) => f.is_active && f.is_visible && f.storage === 'json' && isFormSupported(f))
    .sort((a, b) => a.sort_order - b.sort_order);
}

/** 使用者可以修改的欄位（唯讀欄位只顯示）。 */
export function isEditable(field: FieldDefinition): boolean {
  return !field.is_readonly;
}

// ---- 數值字串 ----

const INTEGER_RE = /^[-+]?\d+$/;
const DECIMAL_RE = /^([-+]?)(\d*)(?:\.(\d*))?$/;

interface ParsedDecimal {
  negative: boolean;
  /** 整數部分，去掉開頭的 0（0 為空字串）。 */
  int: string;
  /** 小數部分，去掉結尾的 0。 */
  frac: string;
}

function parseDecimal(raw: string): ParsedDecimal | undefined {
  const m = DECIMAL_RE.exec(raw.trim());
  if (!m) {
    return undefined;
  }
  const [, sign = '', intPart = '', fracPart = ''] = m;
  if (intPart === '' && fracPart === '') {
    return undefined;
  }
  const int = intPart.replace(/^0+/, '');
  const frac = fracPart.replace(/0+$/, '');
  return { negative: sign === '-' && (int !== '' || frac !== ''), int, frac };
}

/** 數值字串是否為合法的小數（`12`、`-0.5`、`.5`）。 */
export function isDecimalText(raw: string): boolean {
  return parseDecimal(raw) !== undefined;
}

/** 數值字串是否為合法的整數。 */
export function isIntegerText(raw: string): boolean {
  return INTEGER_RE.test(raw.trim());
}

function compareMagnitude(a: ParsedDecimal, b: ParsedDecimal): number {
  if (a.int.length !== b.int.length) {
    return a.int.length < b.int.length ? -1 : 1;
  }
  if (a.int !== b.int) {
    return a.int < b.int ? -1 : 1;
  }
  const len = Math.max(a.frac.length, b.frac.length);
  const fa = a.frac.padEnd(len, '0');
  const fb = b.frac.padEnd(len, '0');
  return fa === fb ? 0 : fa < fb ? -1 : 1;
}

/**
 * 比較兩個數值字串，回傳 -1、0、1。以字串逐位比較，不轉成浮點數（D9）。
 *
 * 不是合法數值時回傳 `undefined`。
 */
export function compareDecimal(a: string, b: string): number | undefined {
  const pa = parseDecimal(a);
  const pb = parseDecimal(b);
  if (!pa || !pb) {
    return undefined;
  }
  if (pa.negative !== pb.negative) {
    return pa.negative ? -1 : 1;
  }
  const c = compareMagnitude(pa, pb);
  return pa.negative ? -c : c;
}

/** 小數位數（不含結尾的 0）：`"12.3400"` → 2。 */
export function decimalScale(raw: string): number {
  return parseDecimal(raw)?.frac.length ?? 0;
}

// ---- 表單值 ↔ JSON 存值 ----

/** 沒有值：`null`、`undefined`、空字串、空陣列。 */
export function isEmptyValue(value: unknown): boolean {
  return (
    value === null ||
    value === undefined ||
    value === '' ||
    (Array.isArray(value) && value.length === 0)
  );
}

/** JSON 存值 → 表單值。 */
export function toFormValue(field: FieldDefinition, json: unknown): unknown {
  if (field.data_type === 'boolean') {
    return json === true;
  }
  if (json === null || json === undefined) {
    return undefined;
  }
  switch (field.data_type) {
    case 'integer':
      return String(json);
    case 'date':
      return typeof json === 'string' ? dayjs(json) : undefined;
    case 'datetime':
      // 以台北時間的年月日時分顯示，與電腦本身的時區無關
      return typeof json === 'string'
        ? dayjs(dayjs(json).utcOffset(TAIPEI_OFFSET_MINUTES).format(DATETIME_VALUE_FORMAT))
        : undefined;
    case 'multi_select':
      return Array.isArray(json) ? json.map(String) : undefined;
    default:
      return typeof json === 'string' ? json : String(json);
  }
}

/**
 * 表單值 → JSON 存值。沒有值回傳 `null`。
 *
 * 呼叫前應已通過 {@link fieldRules} 的驗證；格式不正確的數值原樣以字串送出，由伺服器回傳錯誤。
 */
export function toJsonValue(field: FieldDefinition, value: unknown): unknown {
  if (field.data_type === 'boolean') {
    return value === true;
  }
  if (isEmptyValue(value) || (isNumeric(field) && String(value).trim() === '')) {
    return null;
  }
  switch (field.data_type) {
    case 'integer': {
      const text = String(value).trim();
      // 已驗證在 ±2^53 − 1 內，轉成 number 不會失去精度
      return isIntegerText(text) ? Number(text) : text;
    }
    case 'decimal':
    case 'money': {
      // 以字串送出，不轉成浮點數（D9）
      const text = String(value).trim();
      return text.startsWith('+') ? text.slice(1) : text;
    }
    case 'date':
      return dayjs.isDayjs(value) ? value.format('YYYY-MM-DD') : value;
    case 'datetime':
      return dayjs.isDayjs(value) ? `${value.format(DATETIME_VALUE_FORMAT)}+08:00` : value;
    default:
      return value;
  }
}

/** 兩個 JSON 存值是否相同（小數忽略結尾的 0，日期時間比較時間點）。 */
export function sameJsonValue(field: FieldDefinition, a: unknown, b: unknown): boolean {
  if (isEmptyValue(a) || isEmptyValue(b)) {
    return isEmptyValue(a) && isEmptyValue(b);
  }
  switch (field.data_type) {
    case 'decimal':
    case 'money':
      return compareDecimal(String(a), String(b)) === 0;
    case 'datetime':
      return dayjs(String(a)).valueOf() === dayjs(String(b)).valueOf();
    case 'multi_select':
      return JSON.stringify(a) === JSON.stringify(b);
    default:
      return a === b;
  }
}

/**
 * 表單的初始值。
 *
 * - 新增（沒有 `data`）：套用欄位的預設值
 * - 編輯：資料目前的值
 */
export function initialFormValues(
  fields: readonly FieldDefinition[],
  data?: RecordData,
): RecordFormValues {
  const values: RecordFormValues = {};
  for (const field of fields) {
    const json = data ? data[field.field_key] : field.default_value;
    values[field.field_key] = toFormValue(field, json);
  }
  return values;
}

/** 表單初始值對應的 JSON 存值（比較是否有修改用）。 */
export function baselineData(fields: readonly FieldDefinition[], data?: RecordData): RecordData {
  const result: RecordData = {};
  for (const field of fields) {
    const json = data ? data[field.field_key] : field.default_value;
    result[field.field_key] = field.data_type === 'boolean' ? json === true : (json ?? null);
  }
  return result;
}

/** 可修改且值與 `baseline` 不同的欄位代碼。 */
export function changedFieldKeys(
  fields: readonly FieldDefinition[],
  values: RecordFormValues,
  baseline: RecordData,
): string[] {
  return fields
    .filter(isEditable)
    .filter(
      (f) => !sameJsonValue(f, toJsonValue(f, values[f.field_key]), baseline[f.field_key] ?? null),
    )
    .map((f) => f.field_key);
}

/**
 * 新增資料送出的 `data`：所有可修改的欄位（包含沒有值的 `null`）。
 *
 * 預設值已經在表單中套用；使用者清空預設值時送出 `null`，伺服器不會再套用預設值。
 * 唯讀與不顯示的欄位不送出，由伺服器套用預設值。
 */
export function buildCreateData(
  fields: readonly FieldDefinition[],
  values: RecordFormValues,
): RecordData {
  const data: RecordData = {};
  for (const field of fields.filter(isEditable)) {
    data[field.field_key] = toJsonValue(field, values[field.field_key]);
  }
  return data;
}

/** 修改資料送出的 `data`：只有修改過的欄位，`null` 代表清空。 */
export function buildUpdateData(
  fields: readonly FieldDefinition[],
  values: RecordFormValues,
  baseline: RecordData,
): RecordData {
  const changed = new Set(changedFieldKeys(fields, values, baseline));
  const data: RecordData = {};
  for (const field of fields) {
    if (changed.has(field.field_key)) {
      data[field.field_key] = toJsonValue(field, values[field.field_key]);
    }
  }
  return data;
}

// ---- 驗證 ----

/**
 * 驗證一個欄位的表單值，回傳錯誤訊息；通過時回傳 `undefined`。
 *
 * 規則與訊息和伺服器一致（`crates/core/src/record_data.rs`）：
 *
 * - 必填：不可為空字串或空陣列（唯讀欄位不檢查）
 * - `integer`：整數，±9007199254740991 內；`min`、`max`（含）
 * - `decimal`、`money`：數字格式、小數位數不可超過 `decimal_places`；`min`、`max`（含）
 * - `text`、`long_text`：`max_length`（字元數）；`text`：`pattern`
 */
export function validateFieldValue(field: FieldDefinition, value: unknown): string | undefined {
  if (field.data_type === 'boolean') {
    return undefined;
  }
  if (isEmptyValue(value) || (isNumeric(field) && String(value).trim() === '')) {
    return field.is_required && !field.is_readonly ? '此欄位為必填' : undefined;
  }
  const validation = field.validation;
  switch (field.data_type) {
    case 'integer': {
      const text = String(value).trim();
      if (!isIntegerText(text)) {
        return '必須是整數';
      }
      const abs = text.replace(/^[-+]/, '');
      if (compareDecimal(abs, MAX_SAFE_INTEGER_TEXT)! > 0) {
        return '必須是整數，且不可超過 ±9007199254740991';
      }
      return checkRange(field, text);
    }
    case 'decimal':
    case 'money': {
      const text = String(value).trim();
      if (!isDecimalText(text)) {
        return '數字格式不正確';
      }
      const places = field.decimal_places;
      if (places !== null && places !== undefined && decimalScale(text) > places) {
        return `小數位數不可超過 ${places} 位`;
      }
      return checkRange(field, text);
    }
    case 'text':
    case 'long_text': {
      const text = String(value);
      const max = validation?.max_length;
      if (max !== null && max !== undefined && Array.from(text).length > max) {
        return `不可超過 ${max} 個字元`;
      }
      const pattern = validation?.pattern;
      if (field.data_type === 'text' && pattern) {
        const re = compilePattern(pattern);
        if (re && !re.test(text)) {
          return '格式不符合規則';
        }
      }
      return undefined;
    }
    default:
      return undefined;
  }
}

function isNumeric(field: FieldDefinition): boolean {
  return ['integer', 'decimal', 'money'].includes(field.data_type);
}

function checkRange(field: FieldDefinition, text: string): string | undefined {
  const min = field.validation?.min;
  const max = field.validation?.max;
  if (min !== null && min !== undefined && compareDecimal(text, min) === -1) {
    return `不可小於 ${min}`;
  }
  if (max !== null && max !== undefined && compareDecimal(text, max) === 1) {
    return `不可大於 ${max}`;
  }
  return undefined;
}

/**
 * 格式規則轉成 `RegExp`（與伺服器的 Rust `regex` 一樣是「部分符合」，需要整段符合時規則自己加 `^…$`）。
 *
 * 少數語法兩邊不相容；前端無法解析時不檢查，交給伺服器判斷。
 */
function compilePattern(pattern: string): RegExp | undefined {
  try {
    return new RegExp(pattern, 'u');
  } catch {
    return undefined;
  }
}

// ---- 伺服器錯誤 ----

/** 伺服器驗證錯誤的一個欄位（`422 VALIDATION` 的 `details.fields`）。 */
export interface RecordFieldError {
  field_key: string;
  message: string;
}

/**
 * 新增、修改資料的 `422 VALIDATION`：`details.fields` 為 `[{ field_key, message }]`。
 * 不是這種錯誤時回傳 `undefined`。
 */
export function recordFieldErrors(err: unknown): RecordFieldError[] | undefined {
  if (!(err instanceof CommandError) || err.error.kind !== 'api') {
    return undefined;
  }
  const { code, details } = err.error;
  if (code !== 'VALIDATION' || typeof details !== 'object' || details === null) {
    return undefined;
  }
  const fields = (details as Record<string, unknown>).fields;
  if (!Array.isArray(fields)) {
    return undefined;
  }
  return fields.filter(
    (f): f is RecordFieldError =>
      typeof f === 'object' &&
      f !== null &&
      typeof (f as RecordFieldError).field_key === 'string' &&
      typeof (f as RecordFieldError).message === 'string',
  );
}
