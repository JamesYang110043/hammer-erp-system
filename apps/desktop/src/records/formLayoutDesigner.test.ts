import { describe, expect, it } from 'vitest';

import type { FormLayoutConfig } from './formLayout';
import {
  designerReducer,
  placedKeys,
  sectionKeyOfId,
  toConfig,
  toDesign,
} from './formLayoutDesigner';
import type { DesignerAction, DesignerState } from './formLayoutDesigner';

const CONFIG: FormLayoutConfig = {
  sections: [
    {
      key: 'basic',
      title: '基本資料',
      columns: 2,
      collapsible: false,
      collapsed: false,
      items: [
        { type: 'block', block: 'record_no' },
        { type: 'field', field: 'name', span: 2 },
        { type: 'spacer', span: 1 },
        { type: 'field', field: 'phone', span: 1 },
      ],
    },
    {
      key: 'lines',
      title: '',
      columns: 1,
      collapsible: false,
      collapsed: false,
      items: [{ type: 'block', block: 'lines:parts' }],
    },
  ],
};

function run(...actions: DesignerAction[]): DesignerState {
  return actions.reduce(designerReducer, toDesign(CONFIG));
}

/** 各區塊的代碼與項目 id。 */
function outline(state: DesignerState) {
  return state.sections.map((s) => [s.key, s.items.map((i) => i.id)]);
}

describe('formLayoutDesigner', () => {
  it('round_trips_the_config', () => {
    const state = toDesign(CONFIG);
    expect(outline(state)).toEqual([
      ['basic', ['block:record_no', 'field:name', 'spacer:0', 'field:phone']],
      ['lines', ['block:lines:parts']],
    ]);
    expect(toConfig(state)).toEqual(CONFIG);
    expect(sectionKeyOfId('section:basic')).toBe('basic');
    expect(sectionKeyOfId('drop:lines')).toBe('lines');
    expect(sectionKeyOfId('field:name')).toBeUndefined();
  });

  it('adds_renames_and_configures_sections', () => {
    const state = run(
      { type: 'addSection' },
      { type: 'addSection' },
      { type: 'updateSection', key: 'section_1', patch: { title: '聯絡資訊', columns: 3 } },
      { type: 'updateSection', key: 'section_1', patch: { collapsible: true, collapsed: true } },
    );
    expect(state.sections.map((s) => s.key)).toEqual(['basic', 'lines', 'section_1', 'section_2']);
    expect(state.sections[2]).toMatchObject({
      title: '聯絡資訊',
      columns: 3,
      collapsible: true,
      collapsed: true,
    });
    // 取消可收折時一併取消預設收折；欄數限制在 1–4
    const off = designerReducer(state, {
      type: 'updateSection',
      key: 'section_1',
      patch: { collapsible: false, columns: 9 },
    });
    expect(off.sections[2]).toMatchObject({ collapsible: false, collapsed: false, columns: 4 });
  });

  it('reducing_columns_clamps_spans', () => {
    const state = run({ type: 'updateSection', key: 'basic', patch: { columns: 1 } });
    expect(toConfig(state).sections[0]!.items[1]).toEqual({
      type: 'field',
      field: 'name',
      span: 1,
    });
  });

  it('moves_items_within_and_across_sections', () => {
    const within = run({ type: 'moveItem', id: 'field:phone', section: 'basic', index: 0 });
    expect(outline(within)[0]![1]).toEqual([
      'field:phone',
      'block:record_no',
      'field:name',
      'spacer:0',
    ]);

    const across = run(
      { type: 'addSection' },
      { type: 'moveItem', id: 'field:name', section: 'section_1', index: 0 },
      { type: 'moveItem', id: 'block:lines:parts', section: 'section_1', index: 5 },
    );
    expect(outline(across)).toEqual([
      ['basic', ['block:record_no', 'spacer:0', 'field:phone']],
      ['lines', []],
      ['section_1', ['field:name', 'block:lines:parts']],
    ]);
    // 移到 2 欄的新區塊：span 2 維持
    expect(across.sections[2]!.items[0]).toMatchObject({ span: 2 });
    // 移到單欄區塊：span 調整為 1
    const narrow = designerReducer(across, {
      type: 'moveItem',
      id: 'field:name',
      section: 'lines',
      index: 0,
    });
    expect(narrow.sections[1]!.items[0]).toMatchObject({ id: 'field:name', span: 1 });
  });

  it('moves_sections', () => {
    const state = run({ type: 'addSection' }, { type: 'moveSection', key: 'section_1', to: 0 });
    expect(state.sections.map((s) => s.key)).toEqual(['section_1', 'basic', 'lines']);
  });

  it('sets_span_and_adds_spacers', () => {
    const state = run(
      { type: 'setSpan', id: 'field:phone', span: 2 },
      { type: 'setSpan', id: 'field:name', span: 5 },
      { type: 'addSpacer', section: 'basic' },
      { type: 'addSpacer', section: 'basic' },
    );
    const items = state.sections[0]!.items;
    expect(items.find((i) => i.id === 'field:phone')).toMatchObject({ span: 2 });
    expect(items.find((i) => i.id === 'field:name')).toMatchObject({ span: 2 });
    expect(items.slice(-2).map((i) => i.id)).toEqual(['spacer:1', 'spacer:2']);
  });

  it('removes_fields_and_spacers_but_not_blocks', () => {
    const state = run(
      { type: 'removeItem', id: 'field:name' },
      { type: 'removeItem', id: 'spacer:0' },
      { type: 'removeItem', id: 'block:record_no' },
    );
    expect(outline(state)[0]![1]).toEqual(['block:record_no', 'field:phone']);
    expect(placedKeys(state).fields).toEqual(new Set(['phone']));

    // 再加回來：放在區塊最後；重複加入不會出現兩次
    const back = designerReducer(
      designerReducer(state, {
        type: 'addItem',
        section: 'lines',
        item: { type: 'field', field: 'name', span: 1 },
      }),
      { type: 'addItem', section: 'basic', item: { type: 'field', field: 'name', span: 1 } },
    );
    expect(outline(back)).toEqual([
      ['basic', ['block:record_no', 'field:phone']],
      ['lines', ['block:lines:parts', 'field:name']],
    ]);
  });

  it('removing_a_section_keeps_its_blocks', () => {
    const state = run(
      { type: 'removeSection', key: 'lines' },
      { type: 'removeSection', key: 'basic' },
    );
    // 最後一個區塊不能刪除
    expect(outline(state)).toEqual([
      ['basic', ['block:record_no', 'field:name', 'spacer:0', 'field:phone', 'block:lines:parts']],
    ]);
    const moved = run(
      { type: 'moveItem', id: 'field:name', section: 'lines', index: 0 },
      { type: 'removeSection', key: 'lines' },
    );
    // 欄位回到尚未放進版面，系統區塊移到第一個區塊
    expect(placedKeys(moved).fields.has('name')).toBe(false);
    expect(placedKeys(moved).blocks).toEqual(new Set(['record_no', 'lines:parts']));
  });

  it('reset_replaces_the_state', () => {
    const state = designerReducer(run({ type: 'addSection' }), { type: 'reset', config: CONFIG });
    expect(toConfig(state)).toEqual(CONFIG);
  });
});
