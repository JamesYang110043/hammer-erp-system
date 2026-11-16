import { describe, expect, it } from 'vitest';

import type { EntityDefinition } from '../bindings';
import { REPAIR, fieldDef } from '../test/schemaFixtures';
import { formulaRefGroups, insertText, splitAtSpan } from './formula';

const LINES: EntityDefinition = {
  ...REPAIR,
  id: 'entity-lines',
  key: 'lines',
  label: '明細',
  kind: 'detail',
  parent_entity_id: REPAIR.id,
};

describe('formulaRefGroups', () => {
  it('groups_own_relation_and_detail_fields', () => {
    const groups = formulaRefGroups({
      fields: [
        fieldDef({ field_key: 'qty', label: '數量', data_type: 'integer' }),
        fieldDef({ field_key: 'total', label: '總價', data_type: 'formula' }),
        fieldDef({ field_key: 'customer', label: '客戶', data_type: 'relation' }),
        fieldDef({ field_key: 'tags', label: '標籤', data_type: 'multi_select' }),
        fieldDef({ field_key: 'photo', label: '照片', data_type: 'file' }),
        fieldDef({ field_key: 'old', label: '舊欄位', is_active: false }),
      ],
      selfKey: 'total',
      relationTargets: new Map([
        ['customer', [fieldDef({ field_key: 'rate', label: '折扣率', data_type: 'decimal' })]],
      ]),
      details: [
        {
          entity: LINES,
          fields: [
            fieldDef({ field_key: 'amount', label: '金額', data_type: 'money' }),
            fieldDef({
              field_key: 'sub',
              label: '小計',
              data_type: 'formula',
              formula_result_type: 'decimal',
            }),
            fieldDef({ field_key: 'item', label: '品項' }),
          ],
        },
      ],
    });

    expect(groups).toEqual([
      // 不含自己、停用、不能引用的型態
      { label: '本筆欄位', options: [{ label: '數量', value: '{qty}' }] },
      {
        label: '關聯：客戶',
        options: [{ label: '客戶．折扣率', value: '{customer.rate}' }],
      },
      {
        label: '明細：明細',
        options: [
          { label: '金額（合計）', value: 'SUM(lines.amount)' },
          { label: '小計（合計）', value: 'SUM(lines.sub)' },
          { label: '品項（筆數）', value: 'COUNT(lines.item)' },
        ],
      },
    ]);
  });

  it('skips_relations_without_loaded_target_and_empty_groups', () => {
    const groups = formulaRefGroups({
      fields: [fieldDef({ field_key: 'customer', label: '客戶', data_type: 'relation' })],
      relationTargets: new Map(),
      details: [],
    });
    expect(groups).toEqual([]);
  });
});

describe('insertText', () => {
  it('replaces_selection_and_moves_cursor', () => {
    expect(insertText('{a} + ', '{b}', 6, 6)).toEqual({ value: '{a} + {b}', cursor: 9 });
    expect(insertText('{a} + X', '{b}', 6, 7)).toEqual({ value: '{a} + {b}', cursor: 9 });
  });

  it('appends_when_position_unknown', () => {
    expect(insertText('1 + ', '{b}', undefined, undefined)).toEqual({
      value: '1 + {b}',
      cursor: 7,
    });
    expect(insertText('', '{b}', 99, 99)).toEqual({ value: '{b}', cursor: 3 });
  });
});

describe('splitAtSpan', () => {
  it('marks_range_by_characters', () => {
    // 位置以字元計算（與 Rust 端相同），emoji 也算一個字元
    expect(splitAtSpan('"😀" & @', 6, 7)).toEqual({ before: '"😀" & ', marked: '@', after: '' });
  });

  it('empty_range_marks_one_character_or_end', () => {
    expect(splitAtSpan('1 + ) 2', 4, 4)).toEqual({ before: '1 + ', marked: ')', after: ' 2' });
    expect(splitAtSpan('1 +', 3, 3)).toEqual({ before: '1 +', marked: '', after: '' });
  });
});
