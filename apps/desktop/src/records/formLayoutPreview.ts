import type { FieldDefinition } from '../bindings';
import { valueField } from './display';
import { usesSplitView } from './masterSplit';

/**
 * 表單版面設計器的預覽（docs/architecture.md §7.7）。
 */

/** 預覽的呈現方式：可編輯的表單，或分割檢視右側的唯讀預覽。 */
export type PreviewKind = 'form' | 'readonly';

/** 預覽寬度：整頁，或分割檢視右側。 */
export type PreviewWidth = 'full' | 'split';

export { SPLIT_DETAIL_PREVIEW_WIDTH } from './splitPreviewWidth';

/** 這份版面套用的畫面（設計器上方的說明）。 */
export function layoutAppliesTo(entityKey: string): string[] {
  const screens = ['新增、編輯與檢視的表單'];
  if (usesSplitView(entityKey)) {
    screens.push('分割檢視右側的唯讀預覽', '分割檢視右側的編輯與新增');
  }
  return screens;
}

/** 可以切換「整頁／分割檢視右側」寬度（使用分割檢視的資料類別）。 */
export function hasSplitPreview(entityKey: string): boolean {
  return usesSplitView(entityKey);
}

/**
 * 唯讀預覽的範例值（依欄位型態，公式欄位依結果型態），以 `FieldValue` 格式化後顯示。
 * 關聯、附件沒有範例資料，回傳 `undefined`（由呼叫端顯示佔位文字）。
 */
export function sampleValue(field: FieldDefinition): unknown {
  const f = valueField(field);
  switch (f.data_type) {
    case 'text':
    case 'long_text':
      return `${field.label}範例`;
    case 'integer':
      return 1234;
    case 'decimal':
    case 'money':
      return '12345.6789';
    case 'date':
      return '2026-01-15';
    case 'datetime':
      return '2026-01-15T01:30:00Z';
    case 'boolean':
      return true;
    case 'select':
      return f.options?.[0]?.value ?? null;
    case 'multi_select':
      return f.options?.[0] ? [f.options[0].value] : [];
    default:
      return undefined;
  }
}
