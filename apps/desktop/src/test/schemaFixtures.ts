import type { EntityDefinition, EntityDetail, FieldDefinition } from '../bindings';

/** 欄位定義頁測試用的資料類別。 */
export const REPAIR: EntityDefinition = {
  id: '0196f5a0-0000-7000-8000-000000000100',
  key: 'repair',
  label: '維修單',
  icon: 'ToolOutlined',
  kind: 'master',
  is_system: false,
  storage: 'custom_records',
  parent_entity_id: null,
  numbering_pattern: null,
  title_template: null,
  sort_order: 0,
  is_active: true,
  created_at: '2026-11-13T01:00:00Z',
  updated_at: '2026-11-13T01:00:00Z',
  version: 1,
};

/** 一個欄位定義（預設為自訂、啟用中的單行文字）。 */
export function fieldDef(patch: Partial<FieldDefinition> = {}): FieldDefinition {
  return {
    id: `f-${patch.field_key ?? 'name'}`,
    entity_id: REPAIR.id,
    field_key: 'name',
    label: '名稱',
    data_type: 'text',
    is_system: false,
    storage: 'json',
    is_required: false,
    is_visible: true,
    is_readonly: false,
    is_searchable: false,
    is_unique: false,
    default_value: null,
    validation: null,
    options: null,
    decimal_places: null,
    relation_entity_id: null,
    formula: null,
    formula_result_type: null,
    ui_widget: null,
    column_width: null,
    help_text: null,
    sort_order: 0,
    is_active: true,
    created_at: '2026-11-13T01:00:00Z',
    updated_at: '2026-11-13T01:00:00Z',
    version: 1,
    ...patch,
  };
}

/** 系統欄位（§7.2）。 */
export const SYSTEM_FIELD = fieldDef({
  id: 'f-system',
  field_key: 'record_no',
  label: '單號',
  is_system: true,
  storage: 'column',
  is_required: true,
  is_unique: true,
  is_searchable: true,
  sort_order: 0,
  version: 2,
});

/** 自訂欄位。 */
export const CUSTOM_FIELD = fieldDef({
  id: 'f-custom',
  field_key: 'qty',
  label: '數量',
  data_type: 'integer',
  sort_order: 1,
  version: 4,
});

export function entityDetail(
  fields: FieldDefinition[] = [SYSTEM_FIELD, CUSTOM_FIELD],
): EntityDetail {
  return { entity: REPAIR, fields };
}
