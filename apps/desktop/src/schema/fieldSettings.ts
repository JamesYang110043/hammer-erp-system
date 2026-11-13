import type { DataType, FieldDefinition } from '../bindings';

/** 資料型態的顯示名稱（docs/architecture.md §7.3）。 */
export const DATA_TYPE_LABELS: Record<DataType, string> = {
  text: '單行文字',
  long_text: '多行文字',
  integer: '整數',
  decimal: '小數',
  money: '金額',
  date: '日期',
  datetime: '日期時間',
  boolean: '勾選',
  select: '下拉單選',
  multi_select: '下拉多選',
  relation: '關聯',
  file: '附件',
  formula: '公式',
};

/**
 * 目前可以建立、可以轉換成的型態。
 *
 * `relation`、`file`（M2b）與 `formula`（M2c）尚未支援，與 `DataType::is_value_supported` 相同。
 */
export const SUPPORTED_DATA_TYPES: readonly DataType[] = [
  'text',
  'long_text',
  'integer',
  'decimal',
  'money',
  'date',
  'datetime',
  'boolean',
  'select',
  'multi_select',
];

/**
 * 小數位數上限，與 `hammer_core::schema::rules::MAX_DECIMAL_PLACES` 相同。
 * 前端只做即時提示，以伺服器檢查為準。
 */
export const MAX_DECIMAL_PLACES = 6;

/** 依資料型態才出現的設定。 */
export type TypeSetting =
  /** 小數位數：`decimal`、`money` */
  | 'decimal_places'
  /** 選項：`select`、`multi_select` */
  | 'options'
  /** 最小值、最大值：`integer`、`decimal`、`money` */
  | 'min_max'
  /** 字元數上限：`text`、`long_text` */
  | 'max_length'
  /** 格式規則（正規表示式）：`text` */
  | 'pattern';

/** 資料型態使用哪些設定（與 `hammer_core::schema::rules::validate_field_config` 的規則相同）。 */
export function typeSettings(dataType: DataType): TypeSetting[] {
  switch (dataType) {
    case 'text':
      return ['max_length', 'pattern'];
    case 'long_text':
      return ['max_length'];
    case 'integer':
      return ['min_max'];
    case 'decimal':
    case 'money':
      return ['decimal_places', 'min_max'];
    case 'select':
    case 'multi_select':
      return ['options'];
    default:
      return [];
  }
}

/** 欄位表單中可能出現的屬性。`type_settings` 代表 {@link typeSettings} 的所有設定。 */
export type FieldProp =
  | 'field_key'
  | 'data_type'
  | 'label'
  | 'is_required'
  | 'is_visible'
  | 'is_readonly'
  | 'is_searchable'
  | 'is_unique'
  | 'default_value'
  | 'type_settings'
  | 'column_width'
  | 'help_text';

/** 系統欄位可以修改的屬性（§7.2）。必填只能開啟，不能取消（見 {@link isPropEditable}）。 */
const SYSTEM_EDITABLE: ReadonlySet<FieldProp> = new Set<FieldProp>([
  'label',
  'is_visible',
  'is_required',
  'default_value',
  'column_width',
  'help_text',
]);

/**
 * 表單中的屬性是否可以修改。`field` 為 `undefined` 代表新增欄位。
 *
 * - 新增：全部可以設定
 * - 編輯：欄位代碼建立後不可修改；資料型態只能透過「變更型態」修改
 * - 系統欄位（§7.2）：只能修改顯示名稱、顯示或隱藏、欄寬、說明文字、預設值，
 *   必填只能開啟（原本必填時不可取消）
 */
export function isPropEditable(field: FieldDefinition | undefined, prop: FieldProp): boolean {
  if (!field) {
    return true;
  }
  if (prop === 'field_key' || prop === 'data_type') {
    return false;
  }
  if (!field.is_system) {
    return true;
  }
  if (prop === 'is_required') {
    return !field.is_required;
  }
  return SYSTEM_EDITABLE.has(prop);
}

/**
 * 系統欄位的表單是否顯示這個屬性：可以修改的屬性，加上不可修改但需要辨識欄位的代碼與型態。
 * 自訂欄位全部顯示。
 */
export function isPropVisible(field: FieldDefinition | undefined, prop: FieldProp): boolean {
  if (!field?.is_system) {
    return true;
  }
  return prop === 'field_key' || prop === 'data_type' || SYSTEM_EDITABLE.has(prop);
}

/** 欄位可以變更資料型態（自訂、啟用中、型態已支援）。 */
export function canChangeType(field: FieldDefinition): boolean {
  return !field.is_system && field.is_active && SUPPORTED_DATA_TYPES.includes(field.data_type);
}

/** 欄位可以停用（自訂、啟用中）。系統欄位不可停用（§7.2）。 */
export function canDeactivate(field: FieldDefinition): boolean {
  return !field.is_system && field.is_active;
}
