import { describe, expect, it } from 'vitest';

import { fieldDef } from '../test/schemaFixtures';
import {
  buildRecordListQuery,
  columnSortOrder,
  filterOps,
  fromRecordFilters,
  isQueryable,
  sortFromTable,
  toRecordFilters,
} from './query';

describe('filterOps', () => {
  it('matches_server_operator_rules', () => {
    expect(filterOps('text')).toEqual(['contains', 'eq', 'empty', 'not_empty']);
    expect(filterOps('money')).toEqual(['eq', 'gte', 'lte', 'gt', 'lt', 'empty', 'not_empty']);
    expect(filterOps('boolean')).toEqual(['eq', 'empty', 'not_empty']);
    expect(filterOps('multi_select')).toEqual(['any', 'empty', 'not_empty']);
    expect(filterOps('relation')).toEqual([]);
  });
});

describe('isQueryable', () => {
  it('requires_active_json_supported_field', () => {
    expect(isQueryable(fieldDef())).toBe(true);
    expect(isQueryable(fieldDef({ is_active: false }))).toBe(false);
    expect(isQueryable(fieldDef({ storage: 'column' }))).toBe(false);
    expect(isQueryable(fieldDef({ data_type: 'file' }))).toBe(false);
  });
});

describe('篩選條件轉換', () => {
  it('converts_rows_to_api_filters', () => {
    expect(
      toRecordFilters([
        { fieldKey: 'name', op: 'contains', value: '  車  ' },
        { fieldKey: 'qty', op: 'gte', value: '10' },
        { fieldKey: 'tags', op: 'any', value: ['a', 'b'] },
        { fieldKey: 'memo', op: 'empty', value: null },
        { fieldKey: 'note', op: 'not_empty', value: null },
        // 沒有值、沒有欄位的條件略過
        { fieldKey: 'price', op: 'lt', value: ' ' },
        { fieldKey: '', op: 'eq', value: 'x' },
      ]),
    ).toEqual([
      { field_key: 'name', op: 'contains', value: '車' },
      { field_key: 'qty', op: 'gte', value: '10' },
      { field_key: 'tags', op: 'any', value: 'a' },
      { field_key: 'tags', op: 'any', value: 'b' },
      { field_key: 'memo', op: 'empty', value: 'true' },
      { field_key: 'note', op: 'empty', value: 'false' },
    ]);
  });

  it('converts_api_filters_back_to_rows', () => {
    expect(
      fromRecordFilters([
        { field_key: 'tags', op: 'any', value: 'a' },
        { field_key: 'qty', op: 'lte', value: '5' },
        { field_key: 'tags', op: 'any', value: 'b' },
        { field_key: 'memo', op: 'empty', value: 'false' },
      ]),
    ).toEqual([
      { fieldKey: 'tags', op: 'any', value: ['a', 'b'] },
      { fieldKey: 'qty', op: 'lte', value: '5' },
      { fieldKey: 'memo', op: 'not_empty', value: null },
    ]);
  });
});

describe('排序與分頁參數', () => {
  it('builds_list_query_from_layout_config', () => {
    const filters = [{ field_key: 'qty', op: 'gt' as const, value: '1' }];
    expect(
      buildRecordListQuery(
        { columns: [], sort: { field_key: 'qty', descending: true }, filters },
        3,
        20,
      ),
    ).toEqual({ page: 3, page_size: 20, sort: { field_key: 'qty', descending: true }, filters });
  });

  it('maps_table_sorter', () => {
    expect(sortFromTable('qty', 'ascend')).toEqual({ field_key: 'qty', descending: false });
    expect(sortFromTable('qty', 'descend')).toEqual({ field_key: 'qty', descending: true });
    expect(sortFromTable('qty', null)).toBeNull();
    expect(columnSortOrder({ field_key: 'qty', descending: true }, 'qty')).toBe('descend');
    expect(columnSortOrder({ field_key: 'qty', descending: true }, 'name')).toBeNull();
  });
});
