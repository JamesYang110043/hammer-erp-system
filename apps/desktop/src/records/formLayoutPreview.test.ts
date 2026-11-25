import { describe, expect, it } from 'vitest';

import { fieldDef } from '../test/schemaFixtures';
import {
  SPLIT_DETAIL_PREVIEW_WIDTH,
  displayedColumns,
  hasSplitPreview,
  layoutAppliesTo,
  resetDescription,
  sampleValue,
  splitDetailWidth,
} from './formLayoutPreview';

describe('表單版面設計器的預覽', () => {
  it('split_width_only_for_split_view_entities', () => {
    expect(hasSplitPreview('customer')).toBe(true);
    expect(hasSplitPreview('supplier')).toBe(true);
    expect(hasSplitPreview('product')).toBe(false);
    expect(hasSplitPreview('repair')).toBe(false);
  });

  it('applies_to_lists_split_view_screens', () => {
    expect(layoutAppliesTo('product')).toEqual(['新增、編輯與檢視的表單']);
    // 客戶、廠商只在分割檢視右側新增、編輯與檢視（#260）
    expect(layoutAppliesTo('customer')).toEqual([
      '分割檢視右側的唯讀預覽',
      '分割檢視右側的編輯與新增',
    ]);
  });

  it('split_detail_width_follows_the_page_width', () => {
    // 1440px 視窗：內容區 1440 − 216 − 48 = 1176
    expect(splitDetailWidth(1176)).toBe(SPLIT_DETAIL_PREVIEW_WIDTH);
    // 1280px 視窗：1280 − 216 − 48 = 1016
    expect(splitDetailWidth(1016)).toBe(680);
    expect(splitDetailWidth(100)).toBe(0);
  });

  it('displayed_columns_match_the_container_queries', () => {
    // 1440px 視窗的右側表單內容寬度 840 − 48 = 792
    expect(displayedColumns(4, 792)).toBe(2);
    expect(displayedColumns(3, 792)).toBe(3);
    expect(displayedColumns(2, 792)).toBe(2);
    // 1280px：680 − 48 = 632
    expect(displayedColumns(3, 632)).toBe(2);
    expect(displayedColumns(4, 961)).toBe(4);
    expect(displayedColumns(4, 960)).toBe(2);
    expect(displayedColumns(3, 720)).toBe(2);
    expect(displayedColumns(3, 721)).toBe(3);
    expect(displayedColumns(2, 480)).toBe(1);
    expect(displayedColumns(4, 400)).toBe(1);
    expect(displayedColumns(1, 300)).toBe(1);
  });

  it('reset_description_mentions_the_grouped_default', () => {
    expect(resetDescription(true)).not.toContain('單欄');
    expect(resetDescription(true)).toContain('分組');
    expect(resetDescription(false)).toContain('單欄');
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
