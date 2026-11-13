import { describe, expect, it } from 'vitest';

import type { DataType } from '../bindings';
import { CUSTOM_FIELD, SYSTEM_FIELD, fieldDef } from '../test/schemaFixtures';
import {
  SUPPORTED_DATA_TYPES,
  canChangeType,
  canDeactivate,
  isPropEditable,
  isPropVisible,
  typeSettings,
} from './fieldSettings';
import type { FieldProp } from './fieldSettings';

const ALL_PROPS: FieldProp[] = [
  'field_key',
  'data_type',
  'label',
  'is_required',
  'is_visible',
  'is_readonly',
  'is_searchable',
  'is_unique',
  'default_value',
  'type_settings',
  'column_width',
  'help_text',
];

describe('typeSettings', () => {
  it('settings_per_data_type', () => {
    const expected: Record<DataType, string[]> = {
      text: ['max_length', 'pattern'],
      long_text: ['max_length'],
      integer: ['min_max'],
      decimal: ['decimal_places', 'min_max'],
      money: ['decimal_places', 'min_max'],
      date: [],
      datetime: [],
      boolean: [],
      select: ['options'],
      multi_select: ['options'],
      relation: [],
      file: [],
      formula: [],
    };
    for (const [type, settings] of Object.entries(expected)) {
      expect(typeSettings(type as DataType), type).toEqual(settings);
    }
  });

  it('supported_types_exclude_m2b_m2c', () => {
    expect(SUPPORTED_DATA_TYPES).not.toContain('relation');
    expect(SUPPORTED_DATA_TYPES).not.toContain('file');
    expect(SUPPORTED_DATA_TYPES).not.toContain('formula');
    expect(SUPPORTED_DATA_TYPES).toHaveLength(10);
  });
});

describe('isPropEditable', () => {
  it('new_field_can_set_everything', () => {
    for (const prop of ALL_PROPS) {
      expect(isPropEditable(undefined, prop), prop).toBe(true);
    }
  });

  it('existing_field_key_and_type_are_locked', () => {
    expect(isPropEditable(CUSTOM_FIELD, 'field_key')).toBe(false);
    expect(isPropEditable(CUSTOM_FIELD, 'data_type')).toBe(false);
    for (const prop of ALL_PROPS.filter((p) => p !== 'field_key' && p !== 'data_type')) {
      expect(isPropEditable(CUSTOM_FIELD, prop), prop).toBe(true);
    }
  });

  it('system_field_only_allows_section_7_2', () => {
    const editable = ALL_PROPS.filter((p) => isPropEditable(SYSTEM_FIELD, p));
    // SYSTEM_FIELD 原本必填：不可取消
    expect(editable).toEqual(['label', 'is_visible', 'default_value', 'column_width', 'help_text']);
    const optional = fieldDef({ is_system: true, is_required: false });
    expect(isPropEditable(optional, 'is_required')).toBe(true);
  });

  it('system_field_hides_locked_props', () => {
    const visible = ALL_PROPS.filter((p) => isPropVisible(SYSTEM_FIELD, p));
    expect(visible).toEqual([
      'field_key',
      'data_type',
      'label',
      'is_required',
      'is_visible',
      'default_value',
      'column_width',
      'help_text',
    ]);
    expect(ALL_PROPS.every((p) => isPropVisible(CUSTOM_FIELD, p))).toBe(true);
  });
});

describe('field actions', () => {
  it('system_fields_cannot_change_type_or_deactivate', () => {
    expect(canChangeType(SYSTEM_FIELD)).toBe(false);
    expect(canDeactivate(SYSTEM_FIELD)).toBe(false);
    expect(canChangeType(CUSTOM_FIELD)).toBe(true);
    expect(canDeactivate(CUSTOM_FIELD)).toBe(true);
  });

  it('inactive_or_unsupported_fields', () => {
    const inactive = fieldDef({ is_active: false });
    expect(canChangeType(inactive)).toBe(false);
    expect(canDeactivate(inactive)).toBe(false);
    expect(canChangeType(fieldDef({ data_type: 'formula' }))).toBe(false);
  });
});
