import type { DataType, EntityDefinition, FieldDefinition } from '../bindings';

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
 * 可以轉換成的型態（「變更型態」的目標）。
 *
 * `relation`、`file` 不能由其他型態轉換而來（docs/architecture.md §7.4）；`formula`（M2c）尚未支援。
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
 * 可以在表單輸入、在列表篩選的型態：`formula`（M2c）以外的型態，
 * 與 `DataType::is_value_supported` 相同。
 */
export const VALUE_DATA_TYPES: readonly DataType[] = [...SUPPORTED_DATA_TYPES, 'relation', 'file'];

/** 新增欄位時可以選的型態：{@link VALUE_DATA_TYPES} 加上公式。 */
export const CREATABLE_DATA_TYPES: readonly DataType[] = [...VALUE_DATA_TYPES, 'formula'];

/**
 * 公式的結果型態，與 `hammer_core::schema::rules::FORMULA_RESULT_TYPES` 相同（§7.3）。
 * `decimal`、`money` 需要小數位數。
 */
export const FORMULA_RESULT_TYPES: readonly DataType[] = [
  'text',
  'long_text',
  'integer',
  'decimal',
  'money',
  'date',
  'datetime',
  'boolean',
];

/** 數字型態：`SUM`、`AVG`、`MIN`、`MAX` 只能用於這些型態（含結果為數字的公式欄位，§7.6）。 */
export function isNumericField(field: Pick<FieldDefinition, 'data_type' | 'formula_result_type'>) {
  const t = field.data_type === 'formula' ? field.formula_result_type : field.data_type;
  return t === 'integer' || t === 'decimal' || t === 'money';
}

/**
 * 可以在公式中引用的欄位型態（`hammer_formula::json_to_value`）：`multi_select`、`relation`、
 * `file` 不能直接引用（關聯欄位以 `{關聯欄位.目標欄位}` 引用目標資料的欄位）。
 */
export function isFormulaOperand(field: Pick<FieldDefinition, 'data_type'>): boolean {
  return !['multi_select', 'relation', 'file'].includes(field.data_type);
}

/** 可以設定預設值的型態：`relation`、`file` 不可設定（§7.3）。 */
export function hasDefaultValue(dataType: DataType): boolean {
  return dataType !== 'relation' && dataType !== 'file' && dataType !== 'formula';
}

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
  | 'pattern'
  /** 關聯的資料類別：`relation`（建立後不可修改） */
  | 'relation_entity';

/**
 * 資料型態使用哪些設定（與 `hammer_core::schema::rules::validate_field_config` 的規則相同）。
 *
 * `formula` 的小數位數依結果型態 `resultType`（`decimal`、`money` 才需要）；
 * 公式與結果型態本身另外顯示（`FormulaSettings`）。
 */
export function typeSettings(dataType: DataType, resultType?: DataType | null): TypeSetting[] {
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
    case 'relation':
      return ['relation_entity'];
    case 'formula':
      return resultType === 'decimal' || resultType === 'money' ? ['decimal_places'] : [];
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
 * 屬性不可修改的原因（顯示在表單上）；可以修改時為 `undefined`。`field` 為 `undefined` 代表新增欄位。
 *
 * 系統欄位的所有屬性都顯示，不可修改的屬性停用並說明原因（§7.2）。
 */
export function lockedReason(
  field: FieldDefinition | undefined,
  prop: FieldProp,
): string | undefined {
  if (isPropEditable(field, prop) || !field) {
    return undefined;
  }
  if (prop === 'field_key') {
    return '欄位代碼建立後不可修改';
  }
  if (!field.is_system) {
    return prop === 'data_type' ? '要變更型態請使用欄位列表的「變更型態」' : undefined;
  }
  switch (prop) {
    case 'data_type':
      return '系統欄位不可變更型態';
    case 'is_required':
      return '系統規定必填，不可取消';
    case 'type_settings':
      return '系統欄位的型態設定由系統規定，不可修改';
    default:
      return '系統欄位不可修改此屬性';
  }
}

/**
 * 屬性是否適用於這個型態：公式欄位由系統計算，不可設定「不可重複」、必填、預設值（§7.3）。
 */
export function isPropApplicable(dataType: DataType, prop: FieldProp): boolean {
  if (dataType === 'formula') {
    return prop !== 'is_unique' && prop !== 'is_required' && prop !== 'default_value';
  }
  return true;
}

/** 欄位可以變更資料型態（自訂、啟用中、型態已支援）。 */
export function canChangeType(field: FieldDefinition): boolean {
  return !field.is_system && field.is_active && SUPPORTED_DATA_TYPES.includes(field.data_type);
}

/** 欄位可以停用（自訂、啟用中）。系統欄位不可停用（§7.2）。 */
export function canDeactivate(field: FieldDefinition): boolean {
  return !field.is_system && field.is_active;
}

/**
 * 關聯欄位可以指向的資料類別：有效的主檔或單據（不含明細子表），自訂與系統資料類別都可以
 * （系統資料類別例如商品、客戶，§7.1），與伺服器 `hammer_core::schema::rules::validate_relation_target`
 * 相同；以伺服器檢查為準。
 */
export function isRelationTarget(entity: EntityDefinition): boolean {
  return entity.is_active && (entity.kind === 'master' || entity.kind === 'document');
}
