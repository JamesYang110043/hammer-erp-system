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

export {
  SPLIT_DETAIL_PADDING,
  SPLIT_DETAIL_PREVIEW_WIDTH,
  displayedColumns,
  splitDetailWidth,
} from './splitPreviewWidth';

/**
 * 這份版面套用的畫面（設計器上方的說明），只列實際存在的畫面：使用分割檢視的資料類別
 *（客戶、廠商）只在分割檢視右側新增、編輯與檢視（#260）。
 */
export function layoutAppliesTo(entityKey: string): string[] {
  return usesSplitView(entityKey)
    ? ['分割檢視右側的唯讀預覽', '分割檢視右側的編輯與新增']
    : ['新增、編輯與檢視的表單'];
}

/**
 * 預覽固定為分割檢視右側的寬度（使用分割檢視的資料類別，#260）；其他資料類別為整頁。
 */
export function hasSplitPreview(entityKey: string): boolean {
  return usesSplitView(entityKey);
}

/** 「還原成預設」的確認文字：客戶、廠商為分組的預設版面（#254）。 */
export function resetDescription(hasPreset: boolean): string {
  return hasPreset
    ? '表單會回到預設的分組版面（基本資料、發票資訊、交易條件、其他，每個區塊 2 欄）。'
    : '表單會回到依欄位定義排列的單欄畫面。';
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
