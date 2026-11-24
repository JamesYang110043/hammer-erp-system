import type {
  FieldDefinition,
  Record as DataRecord,
  RecordFilter,
  RecordListQuery_Deserialize as RecordListQuery,
} from '../bindings';
import { isQueryable } from './query';

/**
 * 使用分割檢視（左側列表選擇器、右側預覽）的資料類別（docs/architecture.md §8.7）。
 * 其他資料類別維持表格列表。
 */
export const SPLIT_VIEW_ENTITIES: readonly string[] = ['customer', 'supplier'];

export function usesSplitView(entityKey: string): boolean {
  return SPLIT_VIEW_ENTITIES.includes(entityKey);
}

/** 左側列表的編號、名稱、統編欄位。 */
export const CODE_FIELD = 'code';
export const NAME_FIELD = 'name';
export const TAX_ID_FIELD = 'tax_id';
/** 「啟用中」篩選使用的欄位。 */
export const ACTIVE_FIELD = 'is_active';

/** 搜尋框比對的欄位（任一個包含輸入的文字即符合）。 */
export const SEARCH_FIELD_KEYS: readonly string[] = [
  CODE_FIELD,
  NAME_FIELD,
  'short_name',
  TAX_ID_FIELD,
];

/** 網址中選取資料的參數：`?selected=<id>`。 */
export const SELECTED_PARAM = 'selected';

/** 網址中右側的模式參數：`?mode=edit`（編輯選取的資料）、`?mode=new`（新增）。 */
export const MODE_PARAM = 'mode';
export type SplitMode = 'edit' | 'new';

export function splitMode(value: string | null): SplitMode | undefined {
  return value === 'edit' || value === 'new' ? value : undefined;
}

/** 分割檢視的路徑：`/entities/<key>?selected=<id>&mode=<mode>`。 */
export function splitViewPath(
  entityKey: string,
  { selected, mode }: { selected?: string; mode?: SplitMode } = {},
): string {
  const params = new URLSearchParams();
  if (selected) {
    params.set(SELECTED_PARAM, selected);
  }
  if (mode) {
    params.set(MODE_PARAM, mode);
  }
  const query = params.toString();
  return `/entities/${entityKey}${query ? `?${query}` : ''}`;
}

/** 搜尋框停止輸入多久（毫秒）後查詢。 */
export const PICKER_SEARCH_DELAY_MS = 300;

/** 左側列表每次載入的筆數。 */
export const PICKER_PAGE_SIZE = 50;

/** 資料類別中實際存在、可以篩選的搜尋欄位。 */
export function searchFieldKeys(fields: readonly FieldDefinition[]): string[] {
  return SEARCH_FIELD_KEYS.filter((key) =>
    fields.some((f) => f.field_key === key && isQueryable(f)),
  );
}

/** 資料類別是否有「啟用」欄位（沒有時不顯示「啟用中／全部」篩選）。 */
export function hasActiveField(fields: readonly FieldDefinition[]): boolean {
  return fields.some((f) => f.field_key === ACTIVE_FIELD && isQueryable(f));
}

/** 「只顯示啟用中」的篩選條件。 */
export function activeFilters(activeOnly: boolean): RecordFilter[] {
  return activeOnly ? [{ field_key: ACTIVE_FIELD, op: 'eq', value: 'true' }] : [];
}

/** 左側列表依編號由小到大排序。 */
const CODE_SORT = { field_key: CODE_FIELD, descending: false };

/** 沒有搜尋文字時的分頁查詢。 */
export function pickerPageQuery(
  activeOnly: boolean,
  page: number,
  pageSize: number,
): RecordListQuery {
  return { page, page_size: pageSize, sort: CODE_SORT, filters: activeFilters(activeOnly) };
}

/**
 * 有搜尋文字時，每個搜尋欄位各一個查詢（`contains`）。伺服器的多個篩選條件是 AND，
 * 欄位之間的「或」由前端分別查詢後以 {@link mergeRecords} 合併。
 */
export function pickerSearchQueries(
  fieldKeys: readonly string[],
  q: string,
  activeOnly: boolean,
  page: number,
  pageSize: number,
): RecordListQuery[] {
  return fieldKeys.map((field_key) => ({
    page,
    page_size: pageSize,
    sort: CODE_SORT,
    filters: [...activeFilters(activeOnly), { field_key, op: 'contains', value: q }],
  }));
}

/** 資料的編號（沒有值時為空字串）。 */
export function recordCode(record: DataRecord): string {
  const v = record.data[CODE_FIELD];
  return typeof v === 'string' ? v : '';
}

/** 合併多個查詢結果：依 `id` 去除重複，依編號排序。 */
export function mergeRecords(lists: readonly (readonly DataRecord[])[]): DataRecord[] {
  const byId = new Map<string, DataRecord>();
  for (const list of lists) {
    for (const r of list) {
      if (!byId.has(r.id)) {
        byId.set(r.id, r);
      }
    }
  }
  return [...byId.values()].sort((a, b) => {
    const ca = recordCode(a);
    const cb = recordCode(b);
    return ca < cb ? -1 : ca > cb ? 1 : 0;
  });
}

/** 資料是否已停用（`is_active` 為 `false`）。 */
export function isInactive(record: DataRecord): boolean {
  return record.data[ACTIVE_FIELD] === false;
}

/**
 * 上下鍵切換選取：`delta` 為 `1`（↓）或 `-1`（↑）。沒有選取（或選取的不在列表中）時選第一筆；
 * 已經在第一筆或最後一筆時不變。列表是空的時回傳 `undefined`。
 */
export function adjacentId(
  items: readonly DataRecord[],
  selectedId: string | null,
  delta: 1 | -1,
): string | undefined {
  if (items.length === 0) {
    return undefined;
  }
  const index = selectedId === null ? -1 : items.findIndex((r) => r.id === selectedId);
  if (index === -1) {
    return items[0]!.id;
  }
  const next = Math.min(Math.max(index + delta, 0), items.length - 1);
  return items[next]!.id;
}

/**
 * 右側唯讀預覽顯示的欄位：有效、設定為顯示的欄位（含公式欄位），依 `sort_order`。
 * 位置依表單版面（§7.7）。
 */
export function previewFields(fields: readonly FieldDefinition[]): FieldDefinition[] {
  return fields
    .filter((f) => f.is_active && f.is_visible)
    .sort((a, b) => a.sort_order - b.sort_order);
}

// ---- 分割／表格檢視切換（#245） ----

export type RecordView = 'split' | 'table';

/** 每個資料類別上次選的檢視，存在 localStorage `hammer.recordView.<資料類別代碼>`。 */
export const recordViewStorageKey = (entityKey: string) => `hammer.recordView.${entityKey}`;

/** 上次選的檢視；沒有存過、格式錯誤或無法存取 localStorage 時為分割檢視。 */
export function loadRecordView(entityKey: string): RecordView {
  try {
    return window.localStorage.getItem(recordViewStorageKey(entityKey)) === 'table'
      ? 'table'
      : 'split';
  } catch {
    return 'split';
  }
}

/** 儲存檢視；無法寫入時忽略。 */
export function saveRecordView(entityKey: string, view: RecordView) {
  try {
    window.localStorage.setItem(recordViewStorageKey(entityKey), view);
  } catch {
    // 忽略
  }
}

/**
 * 從分割檢視開啟表單版面設計器（#253）時帶的 router state：設計器返回或存檔後回到這個位置
 *（保留選取的資料）。
 */
export interface DesignerReturnState {
  returnTo: string;
}

export function designerReturnState(entityKey: string, selectedId: string): DesignerReturnState {
  return { returnTo: splitViewPath(entityKey, { selected: selectedId }) };
}

/** router state 中的返回位置；只接受同一個資料類別的資料畫面，其他值忽略。 */
export function designerReturnPath(state: unknown, entityKey: string): string | undefined {
  const returnTo = (state as Partial<DesignerReturnState> | null | undefined)?.returnTo;
  if (typeof returnTo !== 'string') {
    return undefined;
  }
  const prefix = `/entities/${entityKey}`;
  return returnTo === prefix || returnTo.startsWith(`${prefix}?`) ? returnTo : undefined;
}
