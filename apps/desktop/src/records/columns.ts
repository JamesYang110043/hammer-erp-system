import type {
  FieldDefinition,
  LayoutColumn_Serialize as LayoutColumn,
  LayoutConfig_Serialize as LayoutConfig,
} from '../bindings';

export type { LayoutColumn, LayoutConfig };

/** 列表實際顯示的一個欄位。 */
export interface ListColumn {
  field: FieldDefinition;
  /** 欄寬（px）；版面沒有設定時使用欄位定義的 `column_width`，都沒有時由表格決定。 */
  width: number | undefined;
}

/** 有效（未停用）的欄位，依欄位排序。 */
export function activeFields(fields: readonly FieldDefinition[]): FieldDefinition[] {
  return fields.filter((f) => f.is_active).sort((a, b) => a.sort_order - b.sort_order);
}

/**
 * 沒有版面時依欄位定義產生的設定：有效且顯示的欄位，依欄位排序；不排序、不篩選。
 *
 * 與伺服器 `LayoutSource::Generated` 的規則相同（`hammer_shared::layout`）。
 */
export function generatedConfig(fields: readonly FieldDefinition[]): LayoutConfig {
  return {
    columns: activeFields(fields)
      .filter((f) => f.is_visible)
      .map((f) => ({ field_key: f.field_key, width: null })),
    sort: null,
    filters: [],
  };
}

/**
 * 依版面設定決定顯示的欄位與欄寬。
 *
 * 略過已停用、不存在或重複的欄位（伺服器讀取時已略過，這裡再保險一次）；
 * 版面沒有任何可顯示的欄位時，改用欄位定義產生的欄位。
 */
export function resolveColumns(
  fields: readonly FieldDefinition[],
  config: LayoutConfig,
): ListColumn[] {
  const byKey = new Map(activeFields(fields).map((f) => [f.field_key, f]));
  const seen = new Set<string>();
  const columns: ListColumn[] = [];
  for (const c of config.columns) {
    const field = byKey.get(c.field_key);
    if (!field || seen.has(c.field_key)) {
      continue;
    }
    seen.add(c.field_key);
    columns.push({ field, width: c.width ?? field.column_width ?? undefined });
  }
  return columns.length > 0 ? columns : fallbackColumns(fields);
}

function fallbackColumns(fields: readonly FieldDefinition[]): ListColumn[] {
  const byKey = new Map(fields.map((f) => [f.field_key, f]));
  return generatedConfig(fields).columns.map((c) => {
    const field = byKey.get(c.field_key)!;
    return { field, width: field.column_width ?? undefined };
  });
}

/** 欄位設定畫面的一列：所有有效欄位，可勾選是否顯示、調整欄寬與順序。 */
export interface ColumnSetting {
  fieldKey: string;
  label: string;
  visible: boolean;
  /** 版面的欄寬；`null` 代表使用欄位定義的欄寬。 */
  width: number | null;
}

/** 目前顯示的欄位依顯示順序在前，其餘有效欄位依欄位排序接在後面。 */
export function columnSettings(
  fields: readonly FieldDefinition[],
  config: LayoutConfig,
): ColumnSetting[] {
  const shown = resolveColumns(fields, config);
  const widths = new Map(config.columns.map((c) => [c.field_key, c.width]));
  const shownKeys = new Set(shown.map((c) => c.field.field_key));
  return [
    ...shown.map(({ field }) => ({
      fieldKey: field.field_key,
      label: field.label,
      visible: true,
      width: widths.get(field.field_key) ?? null,
    })),
    ...activeFields(fields)
      .filter((f) => !shownKeys.has(f.field_key))
      .map((f) => ({ fieldKey: f.field_key, label: f.label, visible: false, width: null })),
  ];
}

/** 欄位設定 → 版面的 `columns`（只保留顯示的欄位）。 */
export function settingsToColumns(settings: readonly ColumnSetting[]): LayoutColumn[] {
  return settings.filter((s) => s.visible).map((s) => ({ field_key: s.fieldKey, width: s.width }));
}

/** 把第 `index` 列往上（`-1`）或往下（`1`）移一格；超出範圍時不變。 */
export function moveSetting<T>(items: readonly T[], index: number, delta: -1 | 1): T[] {
  const target = index + delta;
  if (index < 0 || index >= items.length || target < 0 || target >= items.length) {
    return [...items];
  }
  const next = [...items];
  [next[index], next[target]] = [next[target]!, next[index]!];
  return next;
}
