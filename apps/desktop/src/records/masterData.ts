import type { Currency, FieldDefinition, Record as DataRecord } from '../bindings';

/**
 * 基本資料（系統資料類別，docs/architecture.md §8.6、§8.7）在動態表單中的特殊輸入方式。
 *
 * 表單仍依欄位定義產生（不手寫專屬版面），只依 `ui_widget`、關聯的資料類別、欄位代碼換成較合適的元件。
 */

/** `ui_widget = currency`：以幣別選單輸入（廠商的 `default_currency_code`）。 */
export const CURRENCY_WIDGET = 'currency';

/** 欄位是否以幣別選單輸入。 */
export function isCurrencyField(field: Pick<FieldDefinition, 'data_type' | 'ui_widget'>): boolean {
  return field.data_type === 'text' && field.ui_widget === CURRENCY_WIDGET;
}

export interface CurrencyOption {
  value: string;
  label: string;
}

/**
 * 幣別選單的選項：啟用中的幣別（伺服器已依本位幣優先、代碼排序），標示為「代碼 名稱」。
 *
 * 目前的值不在清單中（幣別已停用，既有資料可以保留原值，§7.2）時，放在最前面並標示「已停用」。
 */
export function currencyOptions(
  currencies: readonly Pick<Currency, 'code' | 'name' | 'is_active'>[],
  value: string | undefined,
): CurrencyOption[] {
  const options = currencies
    .filter((c) => c.is_active)
    .map((c) => ({ value: c.code, label: `${c.code} ${c.name}` }));
  if (value && !options.some((o) => o.value === value)) {
    options.unshift({ value, label: `${value}（已停用）` });
  }
  return options;
}

/** 商品分類的資料類別代碼；指向它的關聯欄位以階層路徑顯示。 */
export const CATEGORY_ENTITY_KEY = 'product_category';

/** 階層路徑的分隔符號，例如「電子 / 手機 / 配件」。 */
export const PATH_SEPARATOR = ' / ';

export interface CategoryNode {
  id: string;
  /** 從最上層到這個分類的名稱，以 {@link PATH_SEPARATOR} 連接。 */
  path: string;
  /** 第幾層，最上層為 0。 */
  depth: number;
}

function sortKey(record: DataRecord): number {
  const v = record.data.sort_order;
  return typeof v === 'number' ? v : Number(v ?? 0) || 0;
}

function nameOf(record: DataRecord): string {
  return typeof record.data.name === 'string' ? record.data.name : record.title;
}

/**
 * 商品分類（`name`、`parent_id`、`sort_order`）→ 依樹狀順序排列的節點與階層路徑。
 *
 * - 同一層依 `sort_order`、再依名稱排序；子分類緊接在上層之後
 * - 上層分類不在清單中（已刪除）時視為最上層
 * - 資料有循環時（伺服器會擋，這裡只是避免無限迴圈），循環中的分類以自己的名稱為路徑接在最後
 */
export function categoryTree(records: readonly DataRecord[]): CategoryNode[] {
  const byId = new Map(records.map((r) => [r.id, r]));
  const children = new Map<string | null, DataRecord[]>();
  for (const r of records) {
    const parent = typeof r.data.parent_id === 'string' ? r.data.parent_id : null;
    const key = parent !== null && byId.has(parent) && parent !== r.id ? parent : null;
    children.set(key, [...(children.get(key) ?? []), r]);
  }
  for (const list of children.values()) {
    list.sort((a, b) => sortKey(a) - sortKey(b) || nameOf(a).localeCompare(nameOf(b), 'zh-Hant'));
  }

  const result: CategoryNode[] = [];
  const visited = new Set<string>();
  const walk = (parent: string | null, prefix: string, depth: number) => {
    for (const r of children.get(parent) ?? []) {
      if (visited.has(r.id)) {
        continue;
      }
      visited.add(r.id);
      const path = prefix ? `${prefix}${PATH_SEPARATOR}${nameOf(r)}` : nameOf(r);
      result.push({ id: r.id, path, depth });
      walk(r.id, path, depth + 1);
    }
  };
  walk(null, '', 0);
  // 循環中的分類從最上層走不到
  for (const r of records) {
    if (!visited.has(r.id)) {
      visited.add(r.id);
      result.push({ id: r.id, path: nameOf(r), depth: 0 });
      walk(r.id, nameOf(r), 1);
    }
  }
  return result;
}

/** 聯絡人的「主要聯絡人」：同一個客戶（廠商）最多一位（§7.2），明細表格中以單選輸入。 */
export const PRIMARY_CONTACT_FIELD = 'is_primary';

/** 明細欄位是否「整組明細最多一列為 `true`」（以單選輸入）。 */
export function isExclusiveLineField(
  field: Pick<FieldDefinition, 'field_key' | 'data_type' | 'is_system'>,
): boolean {
  return (
    field.is_system && field.data_type === 'boolean' && field.field_key === PRIMARY_CONTACT_FIELD
  );
}

/**
 * 單選欄位選了第 `selected` 列：回傳其他目前為 `true`、要改成 `false` 的列。
 *
 * `values` 為各列目前的值（依列的位置）。
 */
export function rowsToClear(values: readonly unknown[], selected: number): number[] {
  return values.flatMap((v, i) => (i !== selected && v === true ? [i] : []));
}
