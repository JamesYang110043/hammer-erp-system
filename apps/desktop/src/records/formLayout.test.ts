import { describe, expect, it } from 'vitest';

import {
  BLOCKS_SECTION_KEY,
  OTHER_SECTION_KEY,
  generatedFormLayout,
  linesBlockDetail,
  resolveFormLayout,
  systemBlocks,
} from './formLayout';
import type { FormItem, FormLayoutConfig, FormSection } from './formLayout';

const f = (field: string, span = 1): FormItem => ({ type: 'field', field, span });
const b = (block: string): FormItem => ({ type: 'block', block });
const spacer = (span = 1): FormItem => ({ type: 'spacer', span });

function section(key: string, columns: number, items: FormItem[]): FormSection {
  return { key, title: '', columns, collapsible: false, collapsed: false, items };
}

/** 各區塊的代碼與項目（欄位代碼、系統區塊代碼或 `spacer`）。 */
function outline(config: FormLayoutConfig) {
  return config.sections.map((s) => [
    s.key,
    s.items.map((i) => (i.type === 'field' ? i.field : i.type === 'block' ? i.block : 'spacer')),
  ]);
}

const CONTENT = { entityKey: 'repair', fieldKeys: ['a', 'b', 'c'], detailKeys: ['parts'] };

describe('systemBlocks', () => {
  it('follows_the_server_order', () => {
    const codes = (key: string, details: string[]) => systemBlocks(key, details).map((x) => x.code);
    expect(codes('sales_allowance', ['sales_allowance_line'])).toEqual([
      'allowance_available',
      'record_no',
      'lines:sales_allowance_line',
    ]);
    expect(codes('sales_invoice', ['sales_invoice_line', 'sales_invoice_shipment'])).toEqual([
      'record_no',
      'shipment_picker',
      'lines:sales_invoice_line',
      'lines:sales_invoice_shipment',
    ]);
    expect(codes('purchase_invoice', [])).toEqual(['record_no', 'receipt_picker']);
    expect(codes('shipment', ['shipment_line'])).toEqual([
      'record_no',
      'lines:shipment_line',
      'used_lots',
    ]);
    expect(linesBlockDetail('lines:parts')).toBe('parts');
    expect(linesBlockDetail('record_no')).toBeUndefined();
  });
});

describe('generatedFormLayout', () => {
  it('matches_the_previous_single_column_form', () => {
    expect(outline(generatedFormLayout(CONTENT))).toEqual([
      ['main', ['record_no', 'a', 'b', 'c', 'lines:parts']],
    ]);
    expect(generatedFormLayout(CONTENT).sections[0]!.columns).toBe(1);
  });
});

describe('resolveFormLayout', () => {
  it('keeps_a_complete_layout', () => {
    const config = {
      sections: [
        section('basic', 2, [b('record_no'), f('a', 2), spacer(), f('b')]),
        section('more', 3, [f('c', 3), b('lines:parts')]),
      ],
    };
    expect(resolveFormLayout(config, CONTENT)).toEqual(config);
  });

  it('drops_fields_that_cannot_be_shown_and_unknown_blocks', () => {
    const config = {
      sections: [
        section('basic', 2, [
          f('hidden'),
          f('a', 4),
          f('a'),
          b('used_lots'),
          b('record_no'),
          f('b'),
          f('c'),
          b('lines:parts'),
        ]),
      ],
    };
    const resolved = resolveFormLayout(config, CONTENT);
    expect(outline(resolved)).toEqual([['basic', ['a', 'record_no', 'b', 'c', 'lines:parts']]]);
    // span 不超過欄數
    expect(resolved.sections[0]!.items[0]).toEqual(f('a', 2));
  });

  it('appends_missing_fields_and_blocks', () => {
    const resolved = resolveFormLayout(
      { sections: [section('basic', 2, [f('b')])] },
      { ...CONTENT, entityKey: 'sales_allowance' },
    );
    expect(outline(resolved)).toEqual([
      ['basic', ['allowance_available', 'record_no', 'b']],
      [OTHER_SECTION_KEY, ['a', 'c']],
      [BLOCKS_SECTION_KEY, ['lines:parts']],
    ]);
    expect(resolved.sections[1]!.title).toBe('其他欄位');
  });

  it('appends_to_an_existing_other_section', () => {
    const resolved = resolveFormLayout(
      {
        sections: [
          section('basic', 1, [b('record_no'), f('a'), b('lines:parts')]),
          section(OTHER_SECTION_KEY, 1, [f('b')]),
        ],
      },
      CONTENT,
    );
    expect(outline(resolved)).toEqual([
      ['basic', ['record_no', 'a', 'lines:parts']],
      [OTHER_SECTION_KEY, ['b', 'c']],
    ]);
  });

  it('clamps_columns_and_collapsed', () => {
    const odd = { ...section('x', 9, [f('a', 9)]), collapsed: true };
    const resolved = resolveFormLayout({ sections: [odd] }, CONTENT);
    expect(resolved.sections[0]!.columns).toBe(4);
    expect(resolved.sections[0]!.collapsed).toBe(false);
    expect(resolved.sections[0]!.items[1]).toEqual(f('a', 4));
  });
});
