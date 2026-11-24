import { describe, expect, it } from 'vitest';

import { fieldDef } from '../test/schemaFixtures';
import { hasSplitPreview, layoutAppliesTo, sampleValue } from './formLayoutPreview';

describe('表單版面設計器的預覽', () => {
  it('split_width_only_for_split_view_entities', () => {
    expect(hasSplitPreview('customer')).toBe(true);
    expect(hasSplitPreview('supplier')).toBe(true);
    expect(hasSplitPreview('product')).toBe(false);
    expect(hasSplitPreview('repair')).toBe(false);
  });

  it('applies_to_lists_split_view_screens', () => {
    expect(layoutAppliesTo('product')).toEqual(['新增、編輯與檢視的表單']);
    expect(layoutAppliesTo('customer')).toEqual([
      '新增、編輯與檢視的表單',
      '分割檢視右側的唯讀預覽',
      '分割檢視右側的編輯與新增',
    ]);
  });

  it('sample_values_follow_data_type', () => {
    expect(sampleValue(fieldDef({ label: '公司名稱' }))).toBe('公司名稱範例');
    expect(sampleValue(fieldDef({ data_type: 'integer' }))).toBe(1234);
    expect(sampleValue(fieldDef({ data_type: 'money' }))).toBe('12345.6789');
    expect(sampleValue(fieldDef({ data_type: 'date' }))).toBe('2026-01-15');
    expect(sampleValue(fieldDef({ data_type: 'boolean' }))).toBe(true);
    const options = [
      { value: 'a', label: '甲', color: null },
      { value: 'b', label: '乙', color: null },
    ];
    expect(sampleValue(fieldDef({ data_type: 'select', options }))).toBe('a');
    expect(sampleValue(fieldDef({ data_type: 'multi_select', options }))).toEqual(['a']);
    expect(sampleValue(fieldDef({ data_type: 'select', options: null }))).toBeNull();
    // 公式欄位依結果型態
    expect(sampleValue(fieldDef({ data_type: 'formula', formula_result_type: 'integer' }))).toBe(
      1234,
    );
    // 關聯、附件沒有範例資料
    expect(sampleValue(fieldDef({ data_type: 'relation' }))).toBeUndefined();
    expect(sampleValue(fieldDef({ data_type: 'file' }))).toBeUndefined();
  });
});
