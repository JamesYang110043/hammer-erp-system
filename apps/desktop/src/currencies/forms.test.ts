import dayjs from 'dayjs';
import { describe, expect, it } from 'vitest';

import type { Currency } from '../bindings';
import { taipeiToday } from '../utils/time';
import {
  buildCreateCurrency,
  buildExchangeRateQuery,
  buildUpdateCurrency,
  currencyLabel,
  rateCurrencyOptions,
  rateFilterOptions,
  sameRate,
  validateRateText,
} from './forms';

function currency(patch: Partial<Currency> = {}): Currency {
  return {
    id: '0190a000-0000-7000-8000-000000000002',
    code: 'USD',
    name: '美元',
    symbol: 'US$',
    decimal_places: 2,
    is_base: false,
    is_active: true,
    created_at: '2026-11-17T00:00:00Z',
    updated_at: '2026-11-17T00:00:00Z',
    version: 3,
    ...patch,
  };
}

describe('buildCreateCurrency', () => {
  it('trims_and_uppercases', () => {
    expect(
      buildCreateCurrency({
        code: ' gbp ',
        name: ' 英鎊 ',
        symbol: '  ',
        decimal_places: 2,
        is_active: true,
      }),
    ).toEqual({ code: 'GBP', name: '英鎊', symbol: null, decimal_places: 2 });
    expect(
      buildCreateCurrency({
        code: 'GBP',
        name: '英鎊',
        symbol: '£',
        decimal_places: 2,
        is_active: true,
      }).symbol,
    ).toBe('£');
  });
});

describe('buildUpdateCurrency', () => {
  const values = { name: '美元', symbol: 'US$', decimal_places: 2, is_active: true };

  it('returns_undefined_without_changes', () => {
    expect(buildUpdateCurrency(currency(), values)).toBeUndefined();
    // 只差前後空白也算沒有變更
    expect(buildUpdateCurrency(currency(), { ...values, name: ' 美元 ' })).toBeUndefined();
    expect(
      buildUpdateCurrency(currency({ symbol: null }), { ...values, symbol: '' }),
    ).toBeUndefined();
  });

  it('sends_only_changed_fields_with_version', () => {
    expect(
      buildUpdateCurrency(currency(), { ...values, decimal_places: 3, is_active: false }),
    ).toEqual({
      version: 3,
      name: null,
      symbol: null,
      decimal_places: 3,
      is_active: false,
    });
  });

  it('clearing_symbol_sends_empty_string', () => {
    expect(buildUpdateCurrency(currency(), { ...values, symbol: '' })?.symbol).toBe('');
    expect(buildUpdateCurrency(currency(), { ...values, symbol: undefined })?.symbol).toBe('');
  });
});

describe('currency options', () => {
  const list = [
    currency({ code: 'TWD', name: '新台幣', is_base: true }),
    currency({ code: 'USD' }),
    currency({ code: 'XXX', name: '停用幣', is_active: false }),
  ];

  it('label_marks_inactive', () => {
    expect(currencyLabel(list[1]!)).toBe('USD 美元');
    expect(currencyLabel(list[2]!)).toBe('XXX 停用幣（已停用）');
  });

  it('rate_input_excludes_base_and_inactive', () => {
    expect(rateCurrencyOptions(list)).toEqual([{ value: 'USD', label: 'USD 美元' }]);
  });

  it('rate_filter_excludes_base_only', () => {
    expect(rateFilterOptions(list).map((o) => o.value)).toEqual(['USD', 'XXX']);
  });
});

describe('validateRateText', () => {
  it('accepts_positive_decimals_up_to_six_places', () => {
    for (const ok of [
      '32.15',
      '1',
      '0.000001',
      ' 32.150000 ',
      '30.1234560',
      '999999999999.999999',
    ]) {
      expect(validateRateText(ok), ok).toBeUndefined();
    }
  });

  it('rejects_invalid_text', () => {
    expect(validateRateText('')).toBe('請輸入匯率');
    expect(validateRateText(undefined)).toBe('請輸入匯率');
    for (const bad of ['abc', '1e3', '-1', '+1', '.5', '1.', '1,000', '32.1.5']) {
      expect(validateRateText(bad), bad).toBe('請輸入數字，例如 32.15');
    }
    expect(validateRateText('0')).toBe('匯率必須大於 0');
    expect(validateRateText('0.000000')).toBe('匯率必須大於 0');
    // 不自動四捨五入
    expect(validateRateText('32.1234567')).toBe('匯率最多 6 位小數');
    expect(validateRateText('1000000000000')).toBe('匯率超過可輸入的範圍');
  });

  it('keeps_precision_beyond_float', () => {
    // 浮點數無法精確表示的值也能正確判斷
    expect(validateRateText('123456789012.123456')).toBeUndefined();
    expect(sameRate('123456789012.123456', '123456789012.123457')).toBe(false);
    expect(sameRate('32.15', '32.150000')).toBe(true);
  });
});

describe('buildExchangeRateQuery', () => {
  it('maps_filters_and_paging', () => {
    expect(buildExchangeRateQuery({}, 1, 50)).toEqual({
      currency_code: null,
      date_from: null,
      date_to: null,
      page: 1,
      page_size: 50,
    });
    expect(
      buildExchangeRateQuery(
        { currencyCode: 'USD', dateRange: [dayjs('2026-11-01'), dayjs('2026-11-17')] },
        2,
        20,
      ),
    ).toEqual({
      currency_code: 'USD',
      date_from: '2026-11-01',
      date_to: '2026-11-17',
      page: 2,
      page_size: 20,
    });
    expect(
      buildExchangeRateQuery({ dateRange: [null, dayjs('2026-11-17')] }, 1, 50).date_from,
    ).toBeNull();
  });
});

describe('taipeiToday', () => {
  it('uses_taipei_date', () => {
    // UTC 11/16 17:00 = 台北 11/17 01:00
    expect(taipeiToday(dayjs('2026-11-16T17:00:00Z'))).toBe('2026-11-17');
    expect(taipeiToday(dayjs('2026-11-16T15:59:59Z'))).toBe('2026-11-16');
  });
});
