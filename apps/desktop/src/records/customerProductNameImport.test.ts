import { describe, expect, it } from 'vitest';

import {
  batchRowsOf,
  importBlockedReason,
  importSuccessText,
  importSummaryText,
  rejectedBatchResult,
} from './customerProductNameImport';

const result = (create: number, update: number, unchanged: number, error: number) => ({
  dry_run: true,
  summary: { create, update, unchanged, error },
  rows: [],
});

describe('Excel 匯入客戶品名 (§8.7)', () => {
  it('maps rows by code and omits blank notes', () => {
    expect(
      batchRowsOf([
        { row_no: 2, customer_code: 'C001', product_code: 'P001', name: 'X', notes: '圖面' },
        { row_no: 3, customer_code: 'C001', product_code: 'P002', name: 'Y', notes: null },
      ]),
    ).toEqual([
      { customer_code: 'C001', product_code: 'P001', name: 'X', notes: '圖面' },
      { customer_code: 'C001', product_code: 'P002', name: 'Y' },
    ]);
  });

  it('summary text and whether import is allowed', () => {
    expect(importSummaryText(result(2, 1, 0, 1))).toBe('新增 2、修改 1、不變 0、錯誤 1');
    expect(importBlockedReason(result(2, 1, 0, 1))).toBe('請修正 Excel 後重新選擇檔案');
    expect(importBlockedReason(result(0, 0, 3, 0))).toBe('沒有需要匯入的資料');
    expect(importBlockedReason(result(0, 1, 3, 0))).toBeUndefined();
    expect(importSuccessText(result(2, 1, 0, 0))).toBe('已匯入客戶品名：新增 2 筆、修改 1 筆');
  });

  it('reads the preview from a rejected write', () => {
    const details = { ...result(0, 0, 0, 1), dry_run: false };
    expect(rejectedBatchResult({ code: 'VALIDATION', details })).toEqual(details);
    expect(rejectedBatchResult({ code: 'VALIDATION', details: { field: 'rows' } })).toBeUndefined();
    expect(rejectedBatchResult({ code: 'CONFLICT', details })).toBeUndefined();
    expect(rejectedBatchResult(undefined)).toBeUndefined();
  });
});
