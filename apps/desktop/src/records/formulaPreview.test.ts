import dayjs from 'dayjs';
import { describe, expect, it } from 'vitest';

import type { Record as DataRecord } from '../bindings';
import { fieldDef } from '../test/schemaFixtures';
import { buildPreviewRequest, formulaCell, formulaFields, snapshotData } from './formulaPreview';
import type { FormulaPreview, PreviewTable } from './formulaPreview';
import { LINE_ID_KEY } from './lines';

const NAME = fieldDef({ field_key: 'name', sort_order: 0 });
const QTY = fieldDef({ field_key: 'qty', data_type: 'integer', sort_order: 1 });
const DUE = fieldDef({ field_key: 'due', data_type: 'date', sort_order: 2 });
const HIDDEN = fieldDef({
  field_key: 'hidden',
  is_visible: false,
  default_value: 'x',
  sort_order: 3,
});
const OFF = fieldDef({ field_key: 'off', is_active: false, sort_order: 4 });
const TOTAL = fieldDef({
  field_key: 'total',
  data_type: 'formula',
  formula: '{qty} * 2',
  formula_result_type: 'integer',
  sort_order: 5,
});

function line(id: string, data: Record<string, unknown>): DataRecord {
  return {
    id,
    record_no: null,
    title: '',
    data,
    lines: null,
    created_at: '2026-11-13T01:00:00Z',
    created_by: null,
    updated_at: '2026-11-13T01:00:00Z',
    updated_by: null,
    version: 1,
  };
}

describe('formulaFields', () => {
  it('active_visible_formulas_sorted', () => {
    const hidden = { ...TOTAL, field_key: 'h', is_visible: false };
    const first = { ...TOTAL, field_key: 'first', sort_order: -1 };
    expect(formulaFields([NAME, TOTAL, hidden, first]).map((f) => f.field_key)).toEqual([
      'first',
      'total',
    ]);
  });
});

describe('snapshotData', () => {
  const fields = [NAME, QTY, DUE, HIDDEN, OFF, TOTAL];
  const shown = [NAME, QTY, DUE];

  it('form_values_become_json_and_new_rows_use_defaults', () => {
    const values = { name: '冷氣', qty: '3', due: dayjs('2026-11-30'), total: 99 };
    expect(snapshotData(fields, shown, values)).toEqual({
      name: '冷氣',
      qty: 3,
      due: '2026-11-30',
      hidden: 'x',
    });
  });

  it('existing_rows_keep_saved_values_of_fields_not_on_form', () => {
    const saved = { name: '舊', hidden: 'saved', off: 'y', total: 10 };
    expect(snapshotData(fields, shown, { name: '', qty: '1' }, saved)).toEqual({
      qty: 1,
      hidden: 'saved',
    });
  });
});

describe('buildPreviewRequest', () => {
  const master: PreviewTable = { fields: [NAME, QTY], shown: [NAME, QTY] };
  const parts: PreviewTable = {
    fields: [QTY, HIDDEN, { ...TOTAL, entity_id: 'parts' }],
    shown: [QTY],
  };

  it('undefined_without_any_formula', () => {
    const plain = new Map([['parts', { fields: [QTY], shown: [QTY] }]]);
    expect(buildPreviewRequest(master, plain, {}, undefined)).toBeUndefined();
  });

  it('includes_lines_in_form_order_with_saved_values', () => {
    const record = line('m', { name: 'x' });
    record.lines = { parts: [line('l1', { qty: 1, hidden: 'h1' })] };
    const req = buildPreviewRequest(
      master,
      new Map([['parts', parts]]),
      { name: 'x', qty: '2' },
      { parts: [{ qty: '5' }, { [LINE_ID_KEY]: 'l1', qty: '4' }] },
      record,
    );
    expect(req).toEqual({
      fields: master.fields,
      details: [{ key: 'parts', fields: parts.fields }],
      data: { name: 'x', qty: 2 },
      lines: {
        parts: [
          { qty: 5, hidden: 'x' },
          { qty: 4, hidden: 'h1' },
        ],
      },
    });
  });
});

describe('formulaCell', () => {
  const preview: FormulaPreview = {
    master: { values: { total: 6, rel: null }, errors: { rel: '找不到欄位' } },
    lines: { parts: [{ values: { amount: '1.00' }, errors: {} }] },
    relation_dependent: [{ detail: null, field_key: 'rel' }],
  };
  const saved = { data: { total: 2, rel: '9.00' }, errors: { total: '舊錯誤' } };

  it('uses_preview_values_and_errors', () => {
    expect(formulaCell(preview, null, 0, 'total', saved)).toEqual({
      value: 6,
      error: undefined,
      pending: false,
    });
    expect(formulaCell(preview, 'parts', 0, 'amount')).toEqual({
      value: '1.00',
      error: undefined,
      pending: false,
    });
  });

  it('relation_dependent_uses_saved_value', () => {
    expect(formulaCell(preview, null, 0, 'rel', saved)).toEqual({
      value: '9.00',
      error: undefined,
      pending: true,
    });
  });

  it('without_preview_uses_saved_value_and_error', () => {
    expect(formulaCell(undefined, null, 0, 'total', saved)).toEqual({
      value: 2,
      error: '舊錯誤',
      pending: false,
    });
    // 預覽還沒有這一列（剛新增）
    expect(formulaCell(preview, 'parts', 3, 'amount')).toEqual({
      value: undefined,
      error: undefined,
      pending: false,
    });
  });
});
