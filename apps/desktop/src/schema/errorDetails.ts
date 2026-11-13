import { CommandError } from '../api/client';

/** 型態變更時無法轉換的一筆資料（`422 VALIDATION` 的 `details.records`）。 */
export interface ConversionFailure {
  id: string;
  /** 原本的值。 */
  value: unknown;
  /** 該筆資料已軟刪除。 */
  deleted: boolean;
  message: string;
}

/** 開啟「不可重複」時已重複的值（`422 VALIDATION` 的 `details.values`）。 */
export interface DuplicateValue {
  value: unknown;
  count: number;
  record_ids: string[];
}

function validationDetails(err: unknown): Record<string, unknown> | undefined {
  if (!(err instanceof CommandError) || err.error.kind !== 'api') {
    return undefined;
  }
  const { code, details } = err.error;
  if (code !== 'VALIDATION' || typeof details !== 'object' || details === null) {
    return undefined;
  }
  return details as Record<string, unknown>;
}

/**
 * 變更型態失敗的資料清單（`hammer_shared::schema::ChangeFieldTypeRequest`）：
 * `details` 為 `{ field: "data_type", total, records: [{ id, value, deleted, message }] }`。
 * 不是這種錯誤時回傳 `undefined`。
 */
export function conversionFailures(
  err: unknown,
): { total: number; records: ConversionFailure[] } | undefined {
  const details = validationDetails(err);
  if (details?.field !== 'data_type' || !Array.isArray(details.records)) {
    return undefined;
  }
  const records = details.records as ConversionFailure[];
  const total = typeof details.total === 'number' ? details.total : records.length;
  return { total, records };
}

/**
 * 無法開啟「不可重複」時重複的值（docs/architecture.md §7.5）：
 * `details` 為 `{ field: "is_unique", total, values: [{ value, count, record_ids }] }`。
 * 不是這種錯誤時回傳 `undefined`。
 */
export function duplicateValues(
  err: unknown,
): { total: number; values: DuplicateValue[] } | undefined {
  const details = validationDetails(err);
  if (details?.field !== 'is_unique' || !Array.isArray(details.values)) {
    return undefined;
  }
  const values = details.values as DuplicateValue[];
  const total = typeof details.total === 'number' ? details.total : values.length;
  return { total, values };
}

/** 顯示資料的值：字串原樣顯示，其他（數字、陣列、`null`）以 JSON 顯示。 */
export function formatValue(value: unknown): string {
  return typeof value === 'string' ? value : JSON.stringify(value);
}
