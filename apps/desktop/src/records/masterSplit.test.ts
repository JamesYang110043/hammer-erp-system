import { describe, expect, it, vi } from 'vitest';

import type { Record as DataRecord } from '../bindings';
import { fieldDef } from '../test/schemaFixtures';
import {
  activeFilters,
  adjacentId,
  hasActiveField,
  isInactive,
  loadRecordView,
  saveRecordView,
  designerReturnPath,
  designerReturnState,
  splitViewPath,
  mergeRecords,
  pickerPageQuery,
  pickerSearchQueries,
  previewFields,
  searchFieldKeys,
  usesSplitView,
} from './masterSplit';

function rec(id: string, code: string, patch: Record<string, unknown> = {}): DataRecord {
  return {
    id,
    record_no: null,
    title: code,
    data: { code, ...patch },
    created_at: '2026-11-24T01:00:00Z',
    created_by: null,
    updated_at: '2026-11-24T01:00:00Z',
    updated_by: null,
    version: 1,
  };
}

describe('usesSplitView', () => {
  it('only_customer_and_supplier', () => {
    expect(usesSplitView('customer')).toBe(true);
    expect(usesSplitView('supplier')).toBe(true);
    expect(usesSplitView('product')).toBe(false);
    expect(usesSplitView('repair_order')).toBe(false);
  });
});

describe('搜尋欄位', () => {
  it('keeps_existing_queryable_fields_in_order', () => {
    const fields = [
      fieldDef({ field_key: 'tax_id' }),
      fieldDef({ field_key: 'name' }),
      fieldDef({ field_key: 'short_name', is_active: false }),
      fieldDef({ field_key: 'code' }),
      fieldDef({ field_key: 'phone' }),
    ];
    expect(searchFieldKeys(fields)).toEqual(['code', 'name', 'tax_id']);
  });

  it('active_field_detected', () => {
    expect(hasActiveField([fieldDef({ field_key: 'is_active', data_type: 'boolean' })])).toBe(true);
    expect(hasActiveField([fieldDef({ field_key: 'name' })])).toBe(false);
  });
});

describe('查詢條件', () => {
  it('page_query_sorts_by_code_and_filters_active', () => {
    expect(pickerPageQuery(true, 2, 50)).toEqual({
      page: 2,
      page_size: 50,
      sort: { field_key: 'code', descending: false },
      filters: [{ field_key: 'is_active', op: 'eq', value: 'true' }],
    });
    expect(pickerPageQuery(false, 1, 50).filters).toEqual([]);
    expect(activeFilters(false)).toEqual([]);
  });

  it('search_queries_one_contains_per_field', () => {
    const queries = pickerSearchQueries(['code', 'tax_id'], '123', true, 1, 200);
    expect(queries.map((q) => q.filters)).toEqual([
      [
        { field_key: 'is_active', op: 'eq', value: 'true' },
        { field_key: 'code', op: 'contains', value: '123' },
      ],
      [
        { field_key: 'is_active', op: 'eq', value: 'true' },
        { field_key: 'tax_id', op: 'contains', value: '123' },
      ],
    ]);
    expect(queries.every((q) => q.page_size === 200 && q.sort?.field_key === 'code')).toBe(true);
  });
});

describe('mergeRecords', () => {
  it('dedupes_by_id_and_sorts_by_code', () => {
    const a = rec('1', 'C002');
    const b = rec('2', 'C001');
    const c = rec('3', 'C003');
    expect(mergeRecords([[a, c], [b, a], []]).map((r) => r.id)).toEqual(['2', '1', '3']);
  });
});

describe('isInactive', () => {
  it('only_explicit_false', () => {
    expect(isInactive(rec('1', 'A', { is_active: false }))).toBe(true);
    expect(isInactive(rec('1', 'A', { is_active: true }))).toBe(false);
    expect(isInactive(rec('1', 'A'))).toBe(false);
  });
});

describe('adjacentId', () => {
  const items = [rec('1', 'A'), rec('2', 'B'), rec('3', 'C')];

  it('moves_and_stops_at_edges', () => {
    expect(adjacentId(items, '1', 1)).toBe('2');
    expect(adjacentId(items, '2', -1)).toBe('1');
    expect(adjacentId(items, '1', -1)).toBe('1');
    expect(adjacentId(items, '3', 1)).toBe('3');
  });

  it('selects_first_when_nothing_selected', () => {
    expect(adjacentId(items, null, 1)).toBe('1');
    expect(adjacentId(items, 'missing', -1)).toBe('1');
    expect(adjacentId([], null, 1)).toBeUndefined();
  });
});

describe('previewFields', () => {
  it('active_visible_fields_including_formula_sorted', () => {
    const fields = [
      fieldDef({ field_key: 'b', sort_order: 2 }),
      fieldDef({ field_key: 'hidden', is_visible: false, sort_order: 0 }),
      fieldDef({ field_key: 'off', is_active: false, sort_order: 0 }),
      fieldDef({ field_key: 'f', data_type: 'formula', sort_order: 3 }),
      fieldDef({ field_key: 'a', sort_order: 1 }),
    ];
    expect(previewFields(fields).map((f) => f.field_key)).toEqual(['a', 'b', 'f']);
  });
});

describe('檢視偏好', () => {
  it('saves_per_entity_and_defaults_to_split', () => {
    expect(loadRecordView('customer')).toBe('split');
    saveRecordView('customer', 'table');
    expect(loadRecordView('customer')).toBe('table');
    expect(loadRecordView('supplier')).toBe('split');
    window.localStorage.setItem('hammer.recordView.supplier', 'garbage');
    expect(loadRecordView('supplier')).toBe('split');
  });

  it('ignores_storage_errors', () => {
    const get = vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new Error('blocked');
    });
    const set = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('blocked');
    });
    try {
      expect(loadRecordView('customer')).toBe('split');
      expect(() => saveRecordView('customer', 'table')).not.toThrow();
    } finally {
      get.mockRestore();
      set.mockRestore();
    }
  });
});

describe('splitViewPath', () => {
  it('builds_query', () => {
    expect(splitViewPath('customer')).toBe('/entities/customer');
    expect(splitViewPath('customer', { selected: 'x' })).toBe('/entities/customer?selected=x');
    expect(splitViewPath('supplier', { mode: 'new' })).toBe('/entities/supplier?mode=new');
  });
});

describe('designerReturnPath', () => {
  it('returns_to_the_split_view_with_selection', () => {
    const state = designerReturnState('customer', 'abc');
    expect(state).toEqual({ returnTo: '/entities/customer?selected=abc' });
    expect(designerReturnPath(state, 'customer')).toBe('/entities/customer?selected=abc');
  });

  it('ignores_missing_or_other_paths', () => {
    expect(designerReturnPath(null, 'customer')).toBeUndefined();
    expect(designerReturnPath({ returnTo: 1 }, 'customer')).toBeUndefined();
    expect(designerReturnPath({ returnTo: '/entities/supplier?selected=a' }, 'customer')).toBe(
      undefined,
    );
    expect(designerReturnPath({ returnTo: '/entities/customerx' }, 'customer')).toBeUndefined();
    expect(designerReturnPath({ returnTo: 'https://example.com' }, 'customer')).toBeUndefined();
  });
});
