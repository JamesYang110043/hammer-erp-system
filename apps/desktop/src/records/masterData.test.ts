import { describe, expect, it } from 'vitest';

import type { Record as DataRecord } from '../bindings';
import { REPAIR, fieldDef } from '../test/schemaFixtures';
import { masterDataEntities } from '../modules';
import {
  categoryTree,
  currencyOptions,
  isCurrencyField,
  isExclusiveLineField,
  rowsToClear,
} from './masterData';

function category(id: string, name: string, parent: string | null, sortOrder = 0): DataRecord {
  return {
    id,
    record_no: null,
    title: name,
    data: { name, parent_id: parent, sort_order: sortOrder },
    created_at: '2026-11-17T01:00:00Z',
    created_by: null,
    updated_at: '2026-11-17T01:00:00Z',
    updated_by: null,
    version: 1,
  };
}

describe('currencyOptions', () => {
  const currencies = [
    { code: 'TWD', name: '新台幣', is_active: true },
    { code: 'USD', name: '美元', is_active: true },
    { code: 'JPY', name: '日圓', is_active: false },
  ];

  it('lists_active_currencies_in_server_order', () => {
    expect(currencyOptions(currencies, 'USD')).toEqual([
      { value: 'TWD', label: 'TWD 新台幣' },
      { value: 'USD', label: 'USD 美元' },
    ]);
  });

  it('keeps_inactive_current_value_marked', () => {
    expect(currencyOptions(currencies, 'JPY')[0]).toEqual({ value: 'JPY', label: 'JPY（已停用）' });
    expect(currencyOptions(currencies, undefined)).toHaveLength(2);
  });

  it('currency_widget_only_on_text_fields', () => {
    expect(isCurrencyField({ data_type: 'text', ui_widget: 'currency' })).toBe(true);
    expect(isCurrencyField({ data_type: 'text', ui_widget: null })).toBe(false);
    expect(isCurrencyField({ data_type: 'integer', ui_widget: 'currency' })).toBe(false);
  });
});

describe('categoryTree', () => {
  it('builds_paths_in_tree_order', () => {
    const nodes = categoryTree([
      category('phone', '手機', 'elec', 1),
      category('food', '食品', null, 2),
      category('elec', '電子', null, 1),
      category('acc', '配件', 'phone'),
      category('pc', '電腦', 'elec', 0),
    ]);
    expect(nodes).toEqual([
      { id: 'elec', path: '電子', depth: 0 },
      { id: 'pc', path: '電子 / 電腦', depth: 1 },
      { id: 'phone', path: '電子 / 手機', depth: 1 },
      { id: 'acc', path: '電子 / 手機 / 配件', depth: 2 },
      { id: 'food', path: '食品', depth: 0 },
    ]);
  });

  it('missing_parent_is_treated_as_top_level', () => {
    expect(categoryTree([category('a', '甲', 'deleted')])).toEqual([
      { id: 'a', path: '甲', depth: 0 },
    ]);
  });

  it('cycles_do_not_loop_forever', () => {
    const nodes = categoryTree([
      category('a', '甲', 'b'),
      category('b', '乙', 'a'),
      category('self', '自己', 'self'),
    ]);
    expect(nodes.map((n) => n.id).sort()).toEqual(['a', 'b', 'self']);
    expect(nodes.find((n) => n.id === 'self')?.path).toBe('自己');
  });
});

describe('主要聯絡人', () => {
  it('only_system_is_primary_boolean_is_exclusive', () => {
    const primary = fieldDef({ field_key: 'is_primary', data_type: 'boolean', is_system: true });
    expect(isExclusiveLineField(primary)).toBe(true);
    expect(isExclusiveLineField({ ...primary, is_system: false })).toBe(false);
    expect(isExclusiveLineField({ ...primary, field_key: 'is_active' })).toBe(false);
  });

  it('rows_to_clear_are_other_checked_rows', () => {
    expect(rowsToClear([true, false, true, undefined], 1)).toEqual([0, 2]);
    expect(rowsToClear([true, false], 0)).toEqual([]);
  });
});

describe('masterDataEntities', () => {
  it('lists_active_system_masters_by_sort_order', () => {
    const product = {
      ...REPAIR,
      id: 'p',
      key: 'product',
      label: '商品',
      is_system: true,
      sort_order: 10,
    };
    const unit = { ...product, id: 'u', key: 'unit', label: '單位', sort_order: 30 };
    const customer = { ...product, id: 'c', key: 'customer', label: '客戶', sort_order: 40 };
    const contact = { ...customer, id: 'cc', key: 'customer_contact', kind: 'detail' as const };
    const inactive = { ...product, id: 'x', key: 'x', is_active: false };
    expect(
      masterDataEntities([customer, REPAIR, contact, unit, product, inactive]).map((e) => e.key),
    ).toEqual(['product', 'unit', 'customer']);
  });
});
