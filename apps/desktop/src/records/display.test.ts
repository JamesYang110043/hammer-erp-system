import { describe, expect, it } from 'vitest';

import { fieldDef } from '../test/schemaFixtures';
import { formatDecimal, formatFieldText, groupThousands } from './display';

describe('groupThousands', () => {
  it.each([
    ['0', '0'],
    ['999', '999'],
    ['1000', '1,000'],
    ['-1234567', '-1,234,567'],
    ['1234567.891', '1,234,567.891'],
    ['abc', 'abc'],
  ])('%s → %s', (raw, expected) => {
    expect(groupThousands(raw)).toBe(expected);
  });
});

describe('formatDecimal', () => {
  it('pads_to_decimal_places_without_float_conversion', () => {
    expect(formatDecimal('1234.5', 2)).toBe('1,234.50');
    expect(formatDecimal('12', 2)).toBe('12.00');
    // 超過 Number 精度的值也不會失真
    expect(formatDecimal('12345678901234567.89', 2)).toBe('12,345,678,901,234,567.89');
    // 小數位數比設定多時不四捨五入
    expect(formatDecimal('0.125', 2)).toBe('0.125');
    expect(formatDecimal('7', null)).toBe('7');
  });
});

describe('formatFieldText', () => {
  it('formats_each_type', () => {
    const money = fieldDef({ data_type: 'money', decimal_places: 2 });
    expect(formatFieldText(money, '1234567.5')).toBe('1,234,567.50');
    expect(formatFieldText(fieldDef({ data_type: 'integer' }), 12000)).toBe('12,000');
    expect(formatFieldText(fieldDef({ data_type: 'date' }), '2026-11-13')).toBe('2026-11-13');
    // 日期時間以台北時間顯示
    expect(formatFieldText(fieldDef({ data_type: 'datetime' }), '2026-11-13T01:05:00Z')).toBe(
      '2026-11-13 09:05',
    );
    const select = fieldDef({
      data_type: 'select',
      options: [{ value: 'a', label: '甲', color: 'blue' }],
    });
    expect(formatFieldText(select, 'a')).toBe('甲');
    expect(formatFieldText(select, 'removed')).toBe('removed');
    expect(formatFieldText(fieldDef(), null)).toBeUndefined();
    expect(formatFieldText(fieldDef(), '')).toBeUndefined();
  });
});
