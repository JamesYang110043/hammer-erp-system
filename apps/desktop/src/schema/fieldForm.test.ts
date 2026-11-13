import dayjs from 'dayjs';
import { describe, expect, it } from 'vitest';

import { CUSTOM_FIELD, SYSTEM_FIELD, fieldDef } from '../test/schemaFixtures';
import {
  NEW_FIELD_VALUES,
  defaultToForm,
  defaultToJson,
  toChangeTypeRequest,
  toCreateRequest,
  toFormValues,
  toReactivateRequest,
  toTypeConfig,
  toUpdateRequest,
} from './fieldForm';

describe('toTypeConfig', () => {
  it('only_sends_settings_of_the_type', () => {
    const values = {
      decimal_places: 2,
      options: [{ value: ' a ', label: ' A ', color: '' }],
      min: '0',
      max: ' ',
      max_length: 20,
      pattern: '^\\d+$',
    };
    expect(toTypeConfig('decimal', values)).toEqual({
      decimal_places: 2,
      options: null,
      validation: { min: '0' },
    });
    expect(toTypeConfig('select', values)).toEqual({
      decimal_places: null,
      options: [{ value: 'a', label: 'A', color: null }],
      validation: null,
    });
    expect(toTypeConfig('text', values)).toEqual({
      decimal_places: null,
      options: null,
      validation: { max_length: 20, pattern: '^\\d+$' },
    });
    expect(toTypeConfig('long_text', values).validation).toEqual({ max_length: 20 });
    expect(toTypeConfig('boolean', values)).toEqual({
      decimal_places: null,
      options: null,
      validation: null,
    });
  });
});

describe('default value', () => {
  it('converts_dates_to_json_format', () => {
    const d = dayjs('2026-11-13T09:30:00');
    expect(defaultToJson('date', d)).toBe('2026-11-13');
    expect(defaultToJson('datetime', d)).toBe('2026-11-13T09:30:00+08:00');
    expect(defaultToForm('date', '2026-11-13')).toSatisfy(dayjs.isDayjs);
    // 台北時間顯示
    const dt = defaultToForm('datetime', '2026-11-13T01:30:00Z') as dayjs.Dayjs;
    expect(dt.format('YYYY-MM-DD HH:mm')).toBe('2026-11-13 09:30');
  });

  it('empty_values_become_null', () => {
    expect(defaultToJson('text', '')).toBeNull();
    expect(defaultToJson('multi_select', [])).toBeNull();
    expect(defaultToJson('integer', undefined)).toBeNull();
    expect(defaultToJson('boolean', false)).toBe(false);
    expect(defaultToJson('integer', 0)).toBe(0);
    expect(defaultToForm('text', null)).toBeUndefined();
  });
});

describe('requests', () => {
  it('create_request', () => {
    const req = toCreateRequest(
      {
        ...NEW_FIELD_VALUES,
        field_key: 'amount',
        label: ' 金額 ',
        data_type: 'money',
        decimal_places: 2,
        default_value: '10.5',
        help_text: '  ',
      },
      3,
    );
    expect(req).toEqual({
      field_key: 'amount',
      label: '金額',
      data_type: 'money',
      is_required: false,
      is_visible: true,
      is_readonly: false,
      is_searchable: false,
      is_unique: false,
      default_value: '10.5',
      decimal_places: 2,
      options: null,
      validation: null,
      ui_widget: null,
      column_width: null,
      help_text: null,
      sort_order: 3,
    });
  });

  it('update_request_round_trips_unchanged_field', () => {
    const field = fieldDef({
      data_type: 'select',
      options: [{ value: 'a', label: 'A', color: 'blue' }],
      default_value: 'a',
      column_width: 120,
      ui_widget: 'radio',
      version: 7,
      sort_order: 5,
    });
    const req = toUpdateRequest(field, toFormValues(field));
    expect(req).toEqual({
      version: 7,
      label: field.label,
      data_type: 'select',
      is_required: false,
      is_visible: true,
      is_readonly: false,
      is_searchable: false,
      is_unique: false,
      default_value: 'a',
      decimal_places: null,
      options: [{ value: 'a', label: 'A', color: 'blue' }],
      validation: null,
      ui_widget: 'radio',
      column_width: 120,
      help_text: null,
      sort_order: 5,
      is_active: true,
    });
  });

  it('system_field_update_keeps_locked_props', () => {
    const values = {
      ...toFormValues(SYSTEM_FIELD),
      label: '編號',
      // 表單不會顯示這些，即使值不同也送原值
      is_unique: false,
      is_searchable: false,
      is_readonly: true,
      max_length: 5,
    };
    const req = toUpdateRequest(SYSTEM_FIELD, values);
    expect(req.label).toBe('編號');
    expect(req.data_type).toBe(SYSTEM_FIELD.data_type);
    expect(req.is_unique).toBe(true);
    expect(req.is_searchable).toBe(true);
    expect(req.is_readonly).toBe(false);
    expect(req.validation).toBeNull();
  });

  it('reactivate_request', () => {
    const req = toReactivateRequest({ ...CUSTOM_FIELD, is_active: false });
    expect(req.is_active).toBe(true);
    expect(req.version).toBe(CUSTOM_FIELD.version);
    expect(req.data_type).toBe('integer');
  });

  it('change_type_request', () => {
    expect(
      toChangeTypeRequest(CUSTOM_FIELD, 'decimal', { decimal_places: 2, min: '0', max: null }),
    ).toEqual({
      version: CUSTOM_FIELD.version,
      data_type: 'decimal',
      decimal_places: 2,
      options: null,
      validation: { min: '0' },
    });
  });
});
