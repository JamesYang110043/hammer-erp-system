import { describe, expect, it } from 'vitest';

import { CommandError } from '../api/client';
import type { Record as DataRecord } from '../bindings';
import { REPAIR, fieldDef } from '../test/schemaFixtures';
import {
  LINES_FORM_KEY,
  buildCreateLines,
  buildUpdateLines,
  detailEntitiesOf,
  initialLinesValues,
  lineErrorPath,
  lineErrorText,
  linesBaseline,
  linesChanged,
  recordLineErrors,
  showsRecordNo,
} from './lines';
import type { DetailTable } from './lines';

const PARTS = {
  ...REPAIR,
  id: 'e-parts',
  key: 'parts',
  label: '零件',
  kind: 'detail' as const,
  parent_entity_id: REPAIR.id,
};
const QTY = fieldDef({ field_key: 'qty', label: '數量', data_type: 'integer', default_value: 1 });
const PRICE = fieldDef({ field_key: 'price', label: '單價', data_type: 'money', sort_order: 1 });
const TABLES: DetailTable[] = [{ entity: PARTS, fields: [QTY, PRICE] }];

function line(id: string, data: Record<string, unknown>): DataRecord {
  return {
    id,
    title: id,
    data,
    created_at: '2026-11-13T01:00:00Z',
    updated_at: '2026-11-13T01:00:00Z',
    version: 1,
  };
}

const RECORD: DataRecord = {
  ...line('r1', {}),
  lines: { parts: [line('a', { qty: 2, price: '10.00' }), line('b', { qty: 1 })] },
};

describe('detailEntitiesOf', () => {
  it('returns_active_details_of_master_sorted', () => {
    const other = { ...PARTS, id: 'e-x', key: 'x', parent_entity_id: 'other' };
    const inactive = { ...PARTS, id: 'e-y', key: 'y', is_active: false };
    const second = { ...PARTS, id: 'e-z', key: 'z', sort_order: -1 };
    expect(
      detailEntitiesOf([REPAIR, PARTS, other, inactive, second], REPAIR).map((e) => e.key),
    ).toEqual(['z', 'parts']);
  });
});

describe('showsRecordNo', () => {
  it('depends_on_pattern_or_existing_no', () => {
    expect(showsRecordNo(REPAIR)).toBe(false);
    expect(showsRecordNo({ ...REPAIR, numbering_pattern: 'R{####}' })).toBe(true);
    expect(showsRecordNo(REPAIR, { ...RECORD, record_no: 'R0001' })).toBe(true);
  });
});

describe('lines payload', () => {
  const baseline = linesBaseline(TABLES, RECORD);

  it('unchanged_lines_are_omitted', () => {
    const values = initialLinesValues(TABLES, RECORD);
    expect(linesChanged(TABLES, values, baseline)).toBe(false);
    expect(buildUpdateLines(TABLES, values, baseline)).toBeUndefined();
  });

  it('reorder_counts_as_change', () => {
    const values = initialLinesValues(TABLES, RECORD);
    values.parts!.reverse();
    expect(buildUpdateLines(TABLES, values, baseline)).toEqual({
      parts: [
        { id: 'b', data: {} },
        { id: 'a', data: {} },
      ],
    });
  });

  it('decimal_trailing_zeros_are_not_a_change', () => {
    const values = initialLinesValues(TABLES, RECORD);
    values.parts![0]!.price = '10';
    expect(linesChanged(TABLES, values, baseline)).toBe(false);
  });

  it('create_sends_all_editable_fields', () => {
    expect(buildCreateLines(TABLES, { parts: [{ qty: '3' }] })).toEqual({
      parts: [{ data: { qty: 3, price: null } }],
    });
    expect(buildCreateLines(TABLES, undefined)).toEqual({ parts: [] });
    expect(buildCreateLines([], undefined)).toBeUndefined();
  });
});

describe('line errors', () => {
  const err = new CommandError({
    kind: 'api',
    code: 'VALIDATION',
    message: '資料有誤',
    details: {
      fields: [],
      lines: [
        { detail_key: 'parts', index: 0, field_key: 'qty', message: '必須是整數' },
        { detail_key: 'parts', index: null, field_key: null, message: '明細子表不存在' },
        { detail_key: 'parts', index: 'x', field_key: null, message: '格式錯' },
      ],
    },
  });

  it('parses_valid_entries', () => {
    expect(recordLineErrors(err)).toHaveLength(2);
    expect(recordLineErrors(new Error('x'))).toBeUndefined();
  });

  it('maps_to_cell_path_or_text', () => {
    const [cellErr, tableErr] = recordLineErrors(err)!;
    expect(lineErrorPath(cellErr!, TABLES, { parts: 2 })).toEqual([
      LINES_FORM_KEY,
      'parts',
      0,
      'qty',
    ]);
    // 列已不存在（例如送出後被刪除）時不標示在儲存格
    expect(lineErrorPath(cellErr!, TABLES, { parts: 0 })).toBeUndefined();
    expect(lineErrorPath(tableErr!, TABLES, { parts: 2 })).toBeUndefined();
    expect(lineErrorText(cellErr!, TABLES)).toBe('零件 第 1 列 數量：必須是整數');
    expect(lineErrorText(tableErr!, TABLES)).toBe('零件：明細子表不存在');
  });
});
