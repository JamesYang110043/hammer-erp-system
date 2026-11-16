import type {
  FieldDefinition,
  FormulaPreviewRequest_Deserialize as FormulaPreviewRequest,
  FormulaPreview_Serialize as FormulaPreview,
  Record as DataRecord,
} from '../bindings';
import { toJsonValue } from './form';
import type { RecordData, RecordFormValues } from './form';
import { LINE_ID_KEY } from './lines';
import type { LineFormValues } from './lines';

/**
 * 表單中公式欄位的即時預覽（docs/architecture.md §3.3、§7.6）。
 *
 * 表單值轉成 JSON 存值後交給 Tauri command `preview_formulas`（與伺服器存檔時使用同一套計算）。
 * 預覽不讀取關聯資料：直接或間接使用關聯引用的公式欄位（`relation_dependent`）顯示上一次存檔的
 * 值，並標示「存檔後更新」。存檔時一律以伺服器計算的結果為準，預覽的錯誤不阻擋存檔。
 */

export type { FormulaPreview, FormulaPreviewRequest };

/** 表單與明細顯示的公式欄位：有效、設定為顯示、存在 JSON，依 `sort_order`。 */
export function formulaFields(fields: readonly FieldDefinition[]): FieldDefinition[] {
  return fields
    .filter((f) => f.is_active && f.is_visible && f.storage === 'json' && f.data_type === 'formula')
    .sort((a, b) => a.sort_order - b.sort_order);
}

/** 是否有任何有效的公式欄位（沒有時不需要預覽）。 */
export function hasFormulas(fields: readonly FieldDefinition[]): boolean {
  return fields.some((f) => f.is_active && f.data_type === 'formula');
}

/**
 * 一列資料（主表或一列明細）目前的 JSON 存值。
 *
 * - 表單上的欄位（`shown`）：表單值轉成 JSON 存值
 * - 不在表單上的欄位：既有資料的值（`saved`）；新的資料為欄位的預設值（與伺服器新增時相同）
 * - 公式欄位不放入（由預覽計算）
 */
export function snapshotData(
  fields: readonly FieldDefinition[],
  shown: readonly FieldDefinition[],
  values: RecordFormValues,
  saved?: RecordData,
): RecordData {
  const shownKeys = new Set(shown.map((f) => f.field_key));
  const data: RecordData = {};
  for (const field of fields) {
    if (!field.is_active || field.storage !== 'json' || field.data_type === 'formula') {
      continue;
    }
    const key = field.field_key;
    const json = shownKeys.has(key)
      ? toJsonValue(field, values[key])
      : saved
        ? saved[key]
        : field.default_value;
    if (json !== null && json !== undefined) {
      data[key] = json;
    }
  }
  return data;
}

/** 主表或一個明細子表：完整的欄位定義與表單上的欄位。 */
export interface PreviewTable {
  /** 所有欄位定義（含不顯示、已停用的欄位）。 */
  fields: FieldDefinition[];
  /** 表單上可以輸入的欄位（`formFields`）。 */
  shown: FieldDefinition[];
}

/**
 * 預覽的輸入。沒有任何公式欄位時回傳 `undefined`（不需要預覽）。
 *
 * `details` 為有效的明細子表（代碼 → 欄位），`lines` 為表單中各明細子表的列。
 */
export function buildPreviewRequest(
  master: PreviewTable,
  details: ReadonlyMap<string, PreviewTable>,
  values: RecordFormValues,
  lines: Record<string, LineFormValues[]> | undefined,
  record?: DataRecord,
): FormulaPreviewRequest | undefined {
  if (!hasFormulas(master.fields) && ![...details.values()].some((d) => hasFormulas(d.fields))) {
    return undefined;
  }
  const requestLines: Record<string, RecordData[]> = {};
  for (const [key, table] of details) {
    const saved = new Map((record?.lines?.[key] ?? []).map((l) => [l.id, l.data]));
    requestLines[key] = (lines?.[key] ?? []).map((row) => {
      const id = row[LINE_ID_KEY];
      return snapshotData(table.fields, table.shown, row, id ? saved.get(id) : undefined);
    });
  }
  return {
    fields: master.fields,
    details: [...details].map(([key, table]) => ({ key, fields: table.fields })),
    data: snapshotData(master.fields, master.shown, values, record?.data),
    lines: requestLines,
  };
}

/** 一個公式欄位要顯示的內容。 */
export interface FormulaCell {
  value: unknown;
  error: string | undefined;
  /** 使用關聯引用：顯示的是上一次存檔的值，存檔後才會更新。 */
  pending: boolean;
}

/**
 * 公式欄位要顯示的值。
 *
 * - 使用關聯引用的欄位，或還沒有預覽結果時：上一次存檔的值與錯誤（`saved`、`savedErrors`）
 * - 其他：預覽的值與錯誤
 *
 * `detail` 為明細子表代碼，主表為 `null`；`index` 為明細在表單中的位置。
 */
export function formulaCell(
  preview: FormulaPreview | undefined,
  detail: string | null,
  index: number,
  fieldKey: string,
  saved: { data?: RecordData; errors?: Partial<Record<string, string>> | null } = {},
): FormulaCell {
  const pending =
    preview?.relation_dependent.some((f) => f.detail === detail && f.field_key === fieldKey) ??
    false;
  const row = detail === null ? preview?.master : preview?.lines[detail]?.[index];
  if (pending || !row) {
    return {
      value: saved.data?.[fieldKey],
      error: saved.errors?.[fieldKey] ?? undefined,
      pending,
    };
  }
  return { value: row.values[fieldKey], error: row.errors[fieldKey] ?? undefined, pending };
}
