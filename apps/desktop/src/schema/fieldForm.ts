import dayjs from 'dayjs';
import type { Dayjs } from 'dayjs';

import type {
  ChangeFieldTypeRequest,
  CreateFieldRequest,
  DataType,
  FieldDefinition,
  SelectOption,
  UpdateFieldRequest,
  Validation,
} from '../bindings';
import { TAIPEI_OFFSET_MINUTES } from '../utils/time';
import { isPropEditable, typeSettings } from './fieldSettings';

/** 選項的表單值。 */
export interface OptionFormValue {
  value?: string;
  label?: string;
  color?: string | null;
}

/**
 * 欄位表單的值。
 *
 * 驗證規則攤平成 `min`、`max`、`max_length`、`pattern`；`min`、`max` 是字串（D9）。
 * `default_value`：`date`、`datetime` 是 `Dayjs`，其他型態與 JSON 存值格式相同（§7.3）。
 */
export interface FieldFormValues {
  field_key: string;
  label: string;
  data_type: DataType;
  is_required: boolean;
  is_visible: boolean;
  is_readonly: boolean;
  is_searchable: boolean;
  is_unique: boolean;
  decimal_places?: number | null;
  options?: OptionFormValue[];
  min?: string | null;
  max?: string | null;
  max_length?: number | null;
  pattern?: string | null;
  default_value?: unknown;
  column_width?: number | null;
  help_text?: string | null;
}

/** 依型態設定送出的部分（新增、修改、變更型態共用）。 */
export interface TypeConfig {
  decimal_places: number | null;
  options: SelectOption[] | null;
  validation: Validation | null;
}

/** 新增欄位表單的初始值。 */
export const NEW_FIELD_VALUES: FieldFormValues = {
  field_key: '',
  label: '',
  data_type: 'text',
  is_required: false,
  is_visible: true,
  is_readonly: false,
  is_searchable: false,
  is_unique: false,
};

/** 欄位定義 → 表單值。 */
export function toFormValues(field: FieldDefinition): FieldFormValues {
  return {
    field_key: field.field_key,
    label: field.label,
    data_type: field.data_type,
    is_required: field.is_required,
    is_visible: field.is_visible,
    is_readonly: field.is_readonly,
    is_searchable: field.is_searchable,
    is_unique: field.is_unique,
    decimal_places: field.decimal_places ?? null,
    options: field.options?.map((o) => ({ ...o })) ?? [],
    min: field.validation?.min ?? null,
    max: field.validation?.max ?? null,
    max_length: field.validation?.max_length ?? null,
    pattern: field.validation?.pattern ?? null,
    default_value: defaultToForm(field.data_type, field.default_value),
    column_width: field.column_width ?? null,
    help_text: field.help_text ?? null,
  };
}

/** 表單值 → 型態設定。不屬於該型態的設定一律送 `null`（伺服器不接受多餘的設定）。 */
export function toTypeConfig(dataType: DataType, values: Partial<FieldFormValues>): TypeConfig {
  const settings = typeSettings(dataType);
  const validation: Validation = {};
  if (settings.includes('min_max')) {
    const min = blankToNull(values.min);
    const max = blankToNull(values.max);
    if (min !== null) validation.min = min;
    if (max !== null) validation.max = max;
  }
  if (settings.includes('max_length') && values.max_length != null) {
    validation.max_length = values.max_length;
  }
  if (settings.includes('pattern')) {
    const pattern = blankToNull(values.pattern);
    if (pattern !== null) validation.pattern = pattern;
  }
  return {
    decimal_places: settings.includes('decimal_places') ? (values.decimal_places ?? null) : null,
    options: settings.includes('options')
      ? (values.options ?? []).map((o) => ({
          value: (o.value ?? '').trim(),
          label: (o.label ?? '').trim(),
          color: blankToNull(o.color),
        }))
      : null,
    validation: Object.keys(validation).length > 0 ? validation : null,
  };
}

/** 新增欄位的請求。`sort_order` 排在現有欄位之後。 */
export function toCreateRequest(values: FieldFormValues, sortOrder: number): CreateFieldRequest {
  return {
    field_key: values.field_key,
    label: values.label.trim(),
    data_type: values.data_type,
    is_required: values.is_required,
    is_visible: values.is_visible,
    is_readonly: values.is_readonly,
    is_searchable: values.is_searchable,
    is_unique: values.is_unique,
    default_value: defaultToJson(values.data_type, values.default_value),
    ...toTypeConfig(values.data_type, values),
    ui_widget: null,
    column_width: values.column_width ?? null,
    help_text: blankToNull(values.help_text),
    sort_order: sortOrder,
  };
}

/**
 * 修改欄位的請求（整筆取代）。
 *
 * 不可修改的屬性（代碼、型態、系統欄位鎖定的屬性）一律送出原值，
 * 只有 {@link isPropEditable} 的屬性採用表單值。
 */
export function toUpdateRequest(
  field: FieldDefinition,
  values: FieldFormValues,
): UpdateFieldRequest {
  const can = (prop: Parameters<typeof isPropEditable>[1]) => isPropEditable(field, prop);
  const config: TypeConfig = can('type_settings')
    ? toTypeConfig(field.data_type, values)
    : {
        decimal_places: field.decimal_places ?? null,
        options: field.options ?? null,
        validation: field.validation ?? null,
      };
  return {
    version: field.version,
    label: values.label.trim(),
    data_type: field.data_type,
    // 必填：系統欄位原本必填時不可取消，表單停用該開關，仍取表單值（與原值相同）
    is_required: values.is_required ?? field.is_required,
    is_visible: can('is_visible') ? values.is_visible : field.is_visible,
    is_readonly: can('is_readonly') ? values.is_readonly : field.is_readonly,
    is_searchable: can('is_searchable') ? values.is_searchable : field.is_searchable,
    is_unique: can('is_unique') ? values.is_unique : field.is_unique,
    default_value: can('default_value')
      ? defaultToJson(field.data_type, values.default_value)
      : (field.default_value ?? null),
    ...config,
    ui_widget: field.ui_widget ?? null,
    column_width: can('column_width')
      ? (values.column_width ?? null)
      : (field.column_width ?? null),
    help_text: can('help_text') ? blankToNull(values.help_text) : (field.help_text ?? null),
    sort_order: field.sort_order,
    is_active: field.is_active,
  };
}

/** 重新啟用欄位的請求：其他屬性維持原值。 */
export function toReactivateRequest(field: FieldDefinition): UpdateFieldRequest {
  return { ...toUpdateRequest(field, toFormValues(field)), is_active: true };
}

/** 變更型態的請求。 */
export function toChangeTypeRequest(
  field: FieldDefinition,
  dataType: DataType,
  values: Partial<FieldFormValues>,
): ChangeFieldTypeRequest {
  return { version: field.version, data_type: dataType, ...toTypeConfig(dataType, values) };
}

/** 預設值：JSON 存值 → 表單值。 */
export function defaultToForm(dataType: DataType, value: unknown): unknown {
  if (value === null || value === undefined) {
    return undefined;
  }
  if (dataType === 'date' && typeof value === 'string') {
    return dayjs(value);
  }
  if (dataType === 'datetime' && typeof value === 'string') {
    return dayjs(value).utcOffset(TAIPEI_OFFSET_MINUTES);
  }
  return value;
}

/**
 * 預設值：表單值 → JSON 存值（§7.3）。空字串、空陣列、未設定 → `null`。
 *
 * 日期時間以畫面上的時間當作台北時間（D13），例如 `2026-11-13T09:00:00+08:00`。
 */
export function defaultToJson(dataType: DataType, value: unknown): unknown {
  if (value === undefined || value === null || value === '') {
    return null;
  }
  if (Array.isArray(value) && value.length === 0) {
    return null;
  }
  if (dayjs.isDayjs(value)) {
    const d = value as Dayjs;
    return dataType === 'date'
      ? d.format('YYYY-MM-DD')
      : `${d.format('YYYY-MM-DD')}T${d.format('HH:mm:ss')}+08:00`;
  }
  return value;
}

function blankToNull(value: string | null | undefined): string | null {
  const trimmed = value?.trim();
  return trimmed ? trimmed : null;
}
