import { describe, expect, it } from 'vitest';

import { fieldDef } from '../test/schemaFixtures';
import {
  columnSettings,
  generatedConfig,
  moveSetting,
  resolveColumns,
  settingsToColumns,
} from './columns';

const NAME = fieldDef({ field_key: 'name', label: '名稱', sort_order: 0, column_width: 200 });
const QTY = fieldDef({ field_key: 'qty', label: '數量', data_type: 'integer', sort_order: 2 });
const HIDDEN = fieldDef({ field_key: 'memo', label: '備註', is_visible: false, sort_order: 1 });
const INACTIVE = fieldDef({ field_key: 'old', label: '舊欄位', is_active: false, sort_order: 3 });
const FIELDS = [QTY, INACTIVE, HIDDEN, NAME];

const keys = (cols: { field: { field_key: string } }[]) => cols.map((c) => c.field.field_key);

describe('generatedConfig', () => {
  it('uses_active_visible_fields_in_sort_order', () => {
    expect(generatedConfig(FIELDS)).toEqual({
      columns: [
        { field_key: 'name', width: null },
        { field_key: 'qty', width: null },
      ],
      sort: null,
      filters: [],
    });
  });
});

describe('resolveColumns', () => {
  it('follows_layout_order_and_width', () => {
    const cols = resolveColumns(FIELDS, {
      columns: [
        { field_key: 'qty', width: 90 },
        { field_key: 'memo', width: null },
        { field_key: 'name', width: null },
      ],
      sort: null,
      filters: [],
    });
    expect(keys(cols)).toEqual(['qty', 'memo', 'name']);
    // 版面的欄寬優先，沒有時使用欄位定義的欄寬
    expect(cols.map((c) => c.width)).toEqual([90, undefined, 200]);
  });

  it('skips_inactive_unknown_and_duplicate_fields', () => {
    const cols = resolveColumns(FIELDS, {
      columns: [
        { field_key: 'old', width: null },
        { field_key: 'gone', width: null },
        { field_key: 'qty', width: null },
        { field_key: 'qty', width: 50 },
      ],
      sort: null,
      filters: [],
    });
    expect(keys(cols)).toEqual(['qty']);
  });

  it('falls_back_to_field_definitions_when_nothing_to_show', () => {
    const cols = resolveColumns(FIELDS, {
      columns: [{ field_key: 'old', width: null }],
      sort: null,
      filters: [],
    });
    expect(keys(cols)).toEqual(['name', 'qty']);
    expect(keys(resolveColumns(FIELDS, { columns: [], sort: null, filters: [] }))).toEqual([
      'name',
      'qty',
    ]);
  });
});

describe('欄位設定', () => {
  const config = {
    columns: [
      { field_key: 'qty', width: 80 },
      { field_key: 'name', width: null },
    ],
    sort: null,
    filters: [],
  };

  it('lists_shown_columns_first_then_other_active_fields', () => {
    expect(columnSettings(FIELDS, config)).toEqual([
      { fieldKey: 'qty', label: '數量', visible: true, width: 80 },
      { fieldKey: 'name', label: '名稱', visible: true, width: null },
      { fieldKey: 'memo', label: '備註', visible: false, width: null },
    ]);
  });

  it('round_trips_to_layout_columns', () => {
    const settings = columnSettings(FIELDS, config);
    settings[2]!.visible = true;
    settings[2]!.width = 300;
    settings[1]!.visible = false;
    expect(settingsToColumns(moveSetting(settings, 2, -1))).toEqual([
      { field_key: 'qty', width: 80 },
      { field_key: 'memo', width: 300 },
    ]);
  });

  it('move_out_of_range_keeps_order', () => {
    expect(moveSetting(['a', 'b'], 0, -1)).toEqual(['a', 'b']);
    expect(moveSetting(['a', 'b'], 1, 1)).toEqual(['a', 'b']);
    expect(moveSetting(['a', 'b', 'c'], 0, 1)).toEqual(['b', 'a', 'c']);
  });
});
