import type {
  DataType,
  FieldDefinition,
  FilterOp,
  RecordFilter,
  RecordListQuery_Deserialize as RecordListQuery,
  RecordSort,
} from '../bindings';
import { SUPPORTED_DATA_TYPES } from '../schema/fieldSettings';
import type { LayoutConfig } from './columns';

/**
 * 篩選畫面使用的運算子。`empty` 拆成「沒有值」與「有值」兩個選項，
 * 送出時都轉成 `empty`（值為 `true` / `false`）。
 */
export type UiFilterOp = Exclude<FilterOp, 'empty'> | 'empty' | 'not_empty';

export const FILTER_OP_LABELS: Record<UiFilterOp, string> = {
  eq: '等於',
  contains: '包含',
  gt: '大於',
  gte: '大於等於',
  lt: '小於',
  lte: '小於等於',
  any: '包含任一',
  empty: '沒有值',
  not_empty: '有值',
};

/** 篩選畫面的一個條件。`any` 的值為選項陣列，其他為字串。 */
export interface FilterRow {
  fieldKey: string;
  op: UiFilterOp;
  value: string | string[] | null;
}

/**
 * 可以篩選、排序的欄位：有效、存在 JSON、目前支援的型態。
 *
 * 與伺服器 `hammer_core::records::queryable_field` 的規則相同；以伺服器檢查為準。
 */
export function isQueryable(field: FieldDefinition): boolean {
  return (
    field.is_active && field.storage === 'json' && SUPPORTED_DATA_TYPES.includes(field.data_type)
  );
}

/** 欄位型態可用的篩選運算子，第一個為預設（與伺服器 `op_supported` 相同）。 */
export function filterOps(dataType: DataType): UiFilterOp[] {
  const empty: UiFilterOp[] = ['empty', 'not_empty'];
  switch (dataType) {
    case 'text':
    case 'long_text':
      return ['contains', 'eq', ...empty];
    case 'integer':
    case 'decimal':
    case 'money':
    case 'date':
    case 'datetime':
      return ['eq', 'gte', 'lte', 'gt', 'lt', ...empty];
    case 'boolean':
    case 'select':
      return ['eq', ...empty];
    case 'multi_select':
      return ['any', ...empty];
    default:
      return [];
  }
}

/** 運算子是否不需要輸入值（「沒有值」、「有值」）。 */
export function isValueless(op: UiFilterOp): op is 'empty' | 'not_empty' {
  return op === 'empty' || op === 'not_empty';
}

/**
 * 篩選畫面的條件 → 版面與 API 的篩選條件。
 *
 * - 還沒選欄位、還沒填值的條件略過（文字值會去掉前後空白）
 * - 「沒有值」→ `empty=true`，「有值」→ `empty=false`
 * - `any` 的每個選項各一個條件（`filter.tags.any=a&filter.tags.any=b`）
 */
export function toRecordFilters(rows: readonly FilterRow[]): RecordFilter[] {
  const filters: RecordFilter[] = [];
  for (const row of rows) {
    if (!row.fieldKey) {
      continue;
    }
    if (isValueless(row.op)) {
      filters.push({
        field_key: row.fieldKey,
        op: 'empty',
        value: row.op === 'empty' ? 'true' : 'false',
      });
      continue;
    }
    const values = (Array.isArray(row.value) ? row.value : [row.value ?? ''])
      .map((v) => v.trim())
      .filter((v) => v !== '');
    for (const value of values) {
      filters.push({ field_key: row.fieldKey, op: row.op, value });
    }
  }
  return filters;
}

/** 版面與 API 的篩選條件 → 篩選畫面的條件。同一個欄位的 `any` 合併成一列。 */
export function fromRecordFilters(filters: readonly RecordFilter[]): FilterRow[] {
  const rows: FilterRow[] = [];
  for (const f of filters) {
    if (f.op === 'empty') {
      rows.push({
        fieldKey: f.field_key,
        op: f.value === 'false' ? 'not_empty' : 'empty',
        value: null,
      });
      continue;
    }
    if (f.op === 'any') {
      const existing = rows.find((r) => r.fieldKey === f.field_key && r.op === 'any');
      if (existing && Array.isArray(existing.value)) {
        existing.value.push(f.value);
      } else {
        rows.push({ fieldKey: f.field_key, op: 'any', value: [f.value] });
      }
      continue;
    }
    rows.push({ fieldKey: f.field_key, op: f.op, value: f.value });
  }
  return rows;
}

/** 版面設定與分頁 → `list_records` 的參數。 */
export function buildRecordListQuery(
  config: LayoutConfig,
  page: number,
  pageSize: number,
): RecordListQuery {
  return {
    page,
    page_size: pageSize,
    sort: config.sort,
    filters: config.filters,
  };
}

/** 表格欄位的排序狀態（antd Table 的 `sortOrder`）。 */
export function columnSortOrder(
  sort: RecordSort | null,
  fieldKey: string,
): 'ascend' | 'descend' | null {
  if (sort?.field_key !== fieldKey) {
    return null;
  }
  return sort.descending ? 'descend' : 'ascend';
}

/** antd Table 的排序變更 → 版面的排序；取消排序時為 `null`（依建立順序由新到舊）。 */
export function sortFromTable(
  fieldKey: string | undefined,
  order: 'ascend' | 'descend' | null | undefined,
): RecordSort | null {
  if (!fieldKey || !order) {
    return null;
  }
  return { field_key: fieldKey, descending: order === 'descend' };
}
