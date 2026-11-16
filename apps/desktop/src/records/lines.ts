import { CommandError } from '../api/client';
import type {
  EntityDefinition,
  FieldDefinition,
  LineInput_Deserialize as LineInput,
  Record as DataRecord,
} from '../bindings';
import {
  baselineData,
  buildCreateData,
  buildUpdateData,
  changedFieldKeys,
  initialFormValues,
} from './form';
import type { RecordData, RecordFormValues } from './form';

/**
 * 明細子表的表單（docs/architecture.md §8.5「明細子表」、§10「主表與明細」）。
 *
 * 明細放在主表表單值的 {@link LINES_FORM_KEY} 底下：
 * `{ __lines: { parts: [{ __id: "…", qty: "2" }, { qty: "1" }] } }`。
 * 欄位代碼必須以英文字母開頭，`__lines`、`__id` 不會與欄位衝突。
 */

/** 表單值中放明細的鍵。 */
export const LINES_FORM_KEY = '__lines';

/** 明細列表單值中放既有明細 id 的鍵；新的明細沒有。 */
export const LINE_ID_KEY = '__id';

/** 一列明細的表單值。 */
export type LineFormValues = RecordFormValues & { [LINE_ID_KEY]?: string };

/** 一個明細子表與其欄位（表單顯示的欄位，見 `formFields`）。 */
export interface DetailTable {
  entity: EntityDefinition;
  fields: FieldDefinition[];
  /** 顯示的公式欄位（唯讀，見 `formulaFields`）；沒有時省略。 */
  formulas?: FieldDefinition[];
}

/** 主表的明細子表：有效、`kind = detail`、`parent_entity_id` 為主表，依選單排序。 */
export function detailEntitiesOf(
  entities: readonly EntityDefinition[],
  master: EntityDefinition,
): EntityDefinition[] {
  return entities
    .filter((e) => e.kind === 'detail' && e.is_active && e.parent_entity_id === master.id)
    .sort((a, b) => a.sort_order - b.sort_order);
}

/** 單號的保留代碼（`hammer_shared::record::RECORD_NO`），列表排序與篩選使用。 */
export const RECORD_NO = 'record_no';

/** 資料是否顯示單號：資料類別有單號規則，或資料已經有單號。 */
export function showsRecordNo(entity: EntityDefinition, record?: DataRecord): boolean {
  return Boolean(entity.numbering_pattern) || Boolean(record?.record_no);
}

/** 一列明細的初始表單值。沒有 `line` 時為新的明細（套用欄位預設值）。 */
export function lineFormValues(
  fields: readonly FieldDefinition[],
  line?: DataRecord,
): LineFormValues {
  const values: LineFormValues = initialFormValues(fields, line?.data);
  if (line) {
    values[LINE_ID_KEY] = line.id;
  }
  return values;
}

/** 所有明細子表的初始表單值（鍵為明細子表代碼）。 */
export function initialLinesValues(
  tables: readonly DetailTable[],
  record?: DataRecord,
): Record<string, LineFormValues[]> {
  const result: Record<string, LineFormValues[]> = {};
  for (const { entity, fields } of tables) {
    result[entity.key] = (record?.lines?.[entity.key] ?? []).map((line) =>
      lineFormValues(fields, line),
    );
  }
  return result;
}

/** 既有明細的比較基準，鍵為明細 id。 */
export type LineBaselines = Map<string, RecordData>;

/** 每個明細子表的既有明細：順序（id）與比較基準。 */
export interface LinesBaseline {
  ids: Record<string, string[]>;
  data: Record<string, LineBaselines>;
}

export function linesBaseline(tables: readonly DetailTable[], record?: DataRecord): LinesBaseline {
  const ids: Record<string, string[]> = {};
  const data: Record<string, LineBaselines> = {};
  for (const { entity, fields } of tables) {
    const lines = record?.lines?.[entity.key] ?? [];
    ids[entity.key] = lines.map((l) => l.id);
    data[entity.key] = new Map(lines.map((l) => [l.id, baselineData(fields, l.data)]));
  }
  return { ids, data };
}

function rowsOf(values: Record<string, LineFormValues[]> | undefined, key: string) {
  return values?.[key] ?? [];
}

/** 明細子表是否有變動：新增、刪除、順序、任一列的值。 */
export function detailChanged(
  table: DetailTable,
  rows: readonly LineFormValues[],
  baseline: LinesBaseline,
): boolean {
  const key = table.entity.key;
  const ids = baseline.ids[key] ?? [];
  if (rows.length !== ids.length) {
    return true;
  }
  return rows.some((row, i) => {
    const id = row[LINE_ID_KEY];
    if (id === undefined || id !== ids[i]) {
      return true;
    }
    const base = baseline.data[key]?.get(id) ?? {};
    return changedFieldKeys(table.fields, row, base).length > 0;
  });
}

/** 任一個明細子表有變動。 */
export function linesChanged(
  tables: readonly DetailTable[],
  values: Record<string, LineFormValues[]> | undefined,
  baseline: LinesBaseline,
): boolean {
  return tables.some((t) => detailChanged(t, rowsOf(values, t.entity.key), baseline));
}

/** 新增主表時送出的 `lines`：所有明細子表的所有列；沒有明細子表時為 `undefined`。 */
export function buildCreateLines(
  tables: readonly DetailTable[],
  values: Record<string, LineFormValues[]> | undefined,
): Record<string, LineInput[]> | undefined {
  if (tables.length === 0) {
    return undefined;
  }
  const lines: Record<string, LineInput[]> = {};
  for (const { entity, fields } of tables) {
    lines[entity.key] = rowsOf(values, entity.key).map((row) => ({
      data: buildCreateData(fields, row),
    }));
  }
  return lines;
}

/**
 * 修改主表時送出的 `lines`：只有變動的明細子表；沒有變動時為 `undefined`。
 *
 * 有帶到的明細子表會「整組取代」，所以列出該明細子表的所有列：既有的明細帶 `id`，
 * `data` 只有修改過的欄位；新的明細不帶 `id`，`data` 為所有可修改的欄位。沒列出的明細由伺服器刪除。
 */
export function buildUpdateLines(
  tables: readonly DetailTable[],
  values: Record<string, LineFormValues[]> | undefined,
  baseline: LinesBaseline,
): Record<string, LineInput[]> | undefined {
  const lines: Record<string, LineInput[]> = {};
  for (const table of tables) {
    const key = table.entity.key;
    const rows = rowsOf(values, key);
    if (!detailChanged(table, rows, baseline)) {
      continue;
    }
    lines[key] = rows.map((row) => {
      const id = row[LINE_ID_KEY];
      if (id === undefined) {
        return { data: buildCreateData(table.fields, row) };
      }
      const base = baseline.data[key]?.get(id) ?? {};
      return { id, data: buildUpdateData(table.fields, row, base) };
    });
  }
  return Object.keys(lines).length > 0 ? lines : undefined;
}

// ---- 伺服器錯誤 ----

/** 伺服器驗證錯誤的一個明細欄位（`422 VALIDATION` 的 `details.lines`）。 */
export interface RecordLineError {
  detail_key: string;
  /** 該列在 request 陣列中的位置；不屬於某一列時為 `null`。 */
  index: number | null;
  /** 不屬於某個欄位（`id` 不存在或重複）時為 `null`。 */
  field_key: string | null;
  message: string;
}

/**
 * 新增、修改資料的 `422 VALIDATION` 中的明細錯誤（`details.lines`）。
 * 不是這種錯誤時回傳 `undefined`。
 */
export function recordLineErrors(err: unknown): RecordLineError[] | undefined {
  if (!(err instanceof CommandError) || err.error.kind !== 'api') {
    return undefined;
  }
  const { code, details } = err.error;
  if (code !== 'VALIDATION' || typeof details !== 'object' || details === null) {
    return undefined;
  }
  const lines = (details as Record<string, unknown>).lines;
  if (!Array.isArray(lines)) {
    return undefined;
  }
  return lines.filter((l): l is RecordLineError => {
    if (typeof l !== 'object' || l === null) {
      return false;
    }
    const e = l as Record<string, unknown>;
    return (
      typeof e.detail_key === 'string' &&
      typeof e.message === 'string' &&
      (e.index === null || typeof e.index === 'number') &&
      (e.field_key === null || typeof e.field_key === 'string')
    );
  });
}

/** 明細錯誤在表單中的欄位路徑；不屬於某個儲存格時為 `undefined`。 */
export function lineErrorPath(
  error: RecordLineError,
  tables: readonly DetailTable[],
  rowCounts: Record<string, number>,
): (string | number)[] | undefined {
  const table = tables.find((t) => t.entity.key === error.detail_key);
  if (
    !table ||
    error.index === null ||
    error.field_key === null ||
    error.index >= (rowCounts[error.detail_key] ?? 0) ||
    !table.fields.some((f) => f.field_key === error.field_key)
  ) {
    return undefined;
  }
  return [LINES_FORM_KEY, error.detail_key, error.index, error.field_key];
}

/** 不能標示在儲存格的明細錯誤的文字，例如「明細 第 2 列：id 不存在」。 */
export function lineErrorText(error: RecordLineError, tables: readonly DetailTable[]): string {
  const table = tables.find((t) => t.entity.key === error.detail_key);
  const parts = [table?.entity.label ?? error.detail_key];
  if (error.index !== null) {
    parts.push(` 第 ${error.index + 1} 列`);
  }
  if (error.field_key !== null) {
    const label = table?.fields.find((f) => f.field_key === error.field_key)?.label;
    parts.push(` ${label ?? error.field_key}`);
  }
  return `${parts.join('')}：${error.message}`;
}
