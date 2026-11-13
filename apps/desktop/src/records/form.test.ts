import dayjs from 'dayjs';
import { describe, expect, it } from 'vitest';

import { CommandError } from '../api/client';
import { fieldDef } from '../test/schemaFixtures';
import {
  baselineData,
  buildCreateData,
  buildUpdateData,
  compareDecimal,
  decimalScale,
  formFields,
  initialFormValues,
  recordFieldErrors,
  toFormValue,
  toJsonValue,
  validateFieldValue,
} from './form';

const PRICE = fieldDef({ field_key: 'price', data_type: 'money', decimal_places: 2 });
const RATE = fieldDef({ field_key: 'rate', data_type: 'decimal', decimal_places: 4 });
const QTY = fieldDef({ field_key: 'qty', data_type: 'integer' });
const AT = fieldDef({ field_key: 'at', data_type: 'datetime' });

describe('compareDecimal', () => {
  it('compares_by_digits_without_floats', () => {
    expect(compareDecimal('0.1', '0.10')).toBe(0);
    expect(compareDecimal('10', '9.99')).toBe(1);
    expect(compareDecimal('-1', '0')).toBe(-1);
    expect(compareDecimal('-1.5', '-1.25')).toBe(-1);
    expect(compareDecimal('-0', '0')).toBe(0);
    // 超過浮點數精度的值仍能正確比較
    expect(compareDecimal('9007199254740993', '9007199254740992')).toBe(1);
    expect(compareDecimal('0.30000000000000001', '0.3')).toBe(1);
    expect(compareDecimal('abc', '1')).toBeUndefined();
  });

  it('decimal_scale_ignores_trailing_zeros', () => {
    expect(decimalScale('12.3400')).toBe(2);
    expect(decimalScale('12')).toBe(0);
  });
});

describe('formFields', () => {
  it('keeps_active_visible_supported_fields_by_sort_order', () => {
    const fields = [
      fieldDef({ field_key: 'b', sort_order: 2 }),
      fieldDef({ field_key: 'a', sort_order: 1 }),
      fieldDef({ field_key: 'hidden', is_visible: false }),
      fieldDef({ field_key: 'inactive', is_active: false }),
      fieldDef({ field_key: 'rel', data_type: 'relation' }),
      fieldDef({ field_key: 'sys', storage: 'column' }),
    ];
    expect(formFields(fields).map((f) => f.field_key)).toEqual(['a', 'b']);
  });
});

describe('值的轉換', () => {
  it('decimal_values_stay_strings', () => {
    // 0.1 + 0.2 用浮點數會變成 0.30000000000000004；字串原樣送出
    expect(toJsonValue(PRICE, '0.1')).toBe('0.1');
    expect(toJsonValue(PRICE, ' 0.2 ')).toBe('0.2');
    expect(toJsonValue(RATE, '+12.3400')).toBe('12.3400');
    expect(toFormValue(PRICE, '12.30')).toBe('12.30');
    expect(toJsonValue(PRICE, '')).toBeNull();
  });

  it('integer_is_sent_as_number', () => {
    expect(toJsonValue(QTY, '42')).toBe(42);
    expect(toFormValue(QTY, 42)).toBe('42');
    expect(toJsonValue(QTY, ' ')).toBeNull();
  });

  it('datetime_uses_taipei_time', () => {
    const value = toFormValue(AT, '2026-11-13T01:05:00Z') as dayjs.Dayjs;
    expect(value.format('YYYY-MM-DD HH:mm')).toBe('2026-11-13 09:05');
    expect(toJsonValue(AT, value)).toBe('2026-11-13T09:05:00+08:00');
  });

  it('create_applies_defaults_and_sends_editable_fields', () => {
    const fields = [
      fieldDef({ field_key: 'name', default_value: '新案件' }),
      fieldDef({ field_key: 'done', data_type: 'boolean' }),
      fieldDef({ field_key: 'code', is_readonly: true, default_value: 'X' }),
      PRICE,
    ];
    const values = initialFormValues(fields);
    expect(values).toEqual({ name: '新案件', done: false, code: 'X', price: undefined });
    expect(buildCreateData(fields, { ...values, price: '0.1' })).toEqual({
      name: '新案件',
      done: false,
      price: '0.1',
    });
  });

  it('update_sends_only_changed_fields', () => {
    const fields = [fieldDef({ field_key: 'name' }), PRICE, AT, QTY];
    const data = { name: '舊', price: '12.30', at: '2026-11-13T01:05:00Z', qty: 3 };
    const baseline = baselineData(fields, data);
    const values = initialFormValues(fields, data);
    expect(buildUpdateData(fields, values, baseline)).toEqual({});
    // 小數補零、時區不同但時間相同，都不算修改
    expect(buildUpdateData(fields, { ...values, price: '12.3' }, baseline)).toEqual({});
    expect(buildUpdateData(fields, { ...values, name: '新', qty: '' }, baseline)).toEqual({
      name: '新',
      qty: null,
    });
  });
});

describe('validateFieldValue', () => {
  it('required', () => {
    const f = fieldDef({ is_required: true });
    expect(validateFieldValue(f, '')).toBe('此欄位為必填');
    expect(validateFieldValue(f, 'a')).toBeUndefined();
    expect(validateFieldValue({ ...f, is_readonly: true }, '')).toBeUndefined();
    const tags = fieldDef({ data_type: 'multi_select', is_required: true });
    expect(validateFieldValue(tags, [])).toBe('此欄位為必填');
  });

  it('integer_format_range_and_min_max', () => {
    const f = { ...QTY, validation: { min: '0', max: '100', max_length: null, pattern: null } };
    expect(validateFieldValue(f, '1.5')).toBe('必須是整數');
    expect(validateFieldValue(f, '-1')).toBe('不可小於 0');
    expect(validateFieldValue(f, '101')).toBe('不可大於 100');
    expect(validateFieldValue(f, '100')).toBeUndefined();
    expect(validateFieldValue(QTY, '9007199254740992')).toBe(
      '必須是整數，且不可超過 ±9007199254740991',
    );
  });

  it('decimal_format_places_and_min_max', () => {
    const f = { ...PRICE, validation: { min: '0.01', max: null, max_length: null, pattern: null } };
    expect(validateFieldValue(f, '1,000')).toBe('數字格式不正確');
    expect(validateFieldValue(f, '1.234')).toBe('小數位數不可超過 2 位');
    expect(validateFieldValue(f, '1.2300')).toBeUndefined();
    expect(validateFieldValue(f, '0')).toBe('不可小於 0.01');
  });

  it('text_length_and_pattern', () => {
    const f = fieldDef({
      validation: { min: null, max: null, max_length: 3, pattern: '^\\d+$' },
    });
    expect(validateFieldValue(f, '1234')).toBe('不可超過 3 個字元');
    expect(validateFieldValue(f, '12a')).toBe('格式不符合規則');
    expect(validateFieldValue(f, '123')).toBeUndefined();
    // 以字元數計算（不是 UTF-16 長度）
    const g = fieldDef({ validation: { min: null, max: null, max_length: 2, pattern: null } });
    expect(validateFieldValue(g, '😀😀')).toBeUndefined();
  });
});

describe('recordFieldErrors', () => {
  it('reads_details_fields', () => {
    const err = new CommandError({
      kind: 'api',
      code: 'VALIDATION',
      message: '資料有誤',
      details: { fields: [{ field_key: 'qty', message: '不可大於 100' }] },
    });
    expect(recordFieldErrors(err)).toEqual([{ field_key: 'qty', message: '不可大於 100' }]);
    expect(recordFieldErrors(new Error('x'))).toBeUndefined();
  });
});
