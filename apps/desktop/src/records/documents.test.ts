import { describe, expect, it } from 'vitest';

import { CommandError } from '../api/client';
import type { ClientError } from '../bindings';
import { STOCK_ADJUSTMENT, adjustment } from '../test/documentFixtures';
import { REPAIR } from '../test/schemaFixtures';
import {
  availableAction,
  documentActionError,
  documentStatus,
  documentStatuses,
  formatQty,
  isEditableDocument,
  isSystemDocument,
} from './documents';

function apiError(code: string, details?: unknown, message = '伺服器訊息'): CommandError {
  return new CommandError({ kind: 'api', code, message, details } as ClientError);
}

describe('documents', () => {
  it('detects_system_documents', () => {
    expect(isSystemDocument(STOCK_ADJUSTMENT)).toBe(true);
    // 自訂的單據類資料類別沒有過帳、作廢
    expect(isSystemDocument({ kind: 'document', is_system: false })).toBe(false);
    expect(isSystemDocument({ kind: 'master', is_system: true })).toBe(false);
  });

  it('only_drafts_are_editable', () => {
    expect(isEditableDocument(STOCK_ADJUSTMENT, adjustment('draft'))).toBe(true);
    expect(isEditableDocument(STOCK_ADJUSTMENT, adjustment('posted'))).toBe(false);
    expect(isEditableDocument(STOCK_ADJUSTMENT, adjustment('void'))).toBe(false);
    // 新增中、或不是系統單據
    expect(isEditableDocument(STOCK_ADJUSTMENT, undefined)).toBe(true);
    expect(isEditableDocument(REPAIR, adjustment('posted'))).toBe(true);
  });

  it('status_and_available_action', () => {
    expect(documentStatus(adjustment('posted'))).toBe('posted');
    expect(documentStatus({ data: { status: 'closed' } })).toBe('closed');
    expect(documentStatus({ data: { status: 'partially_done' } })).toBeUndefined();
    expect(availableAction('draft')).toBe('post');
    expect(availableAction('posted')).toBe('void');
    expect(availableAction('void')).toBeUndefined();
    expect(availableAction(undefined)).toBeUndefined();
  });

  it('statuses_depend_on_document', () => {
    expect(documentStatuses('stock_adjustment')).toEqual(['draft', 'posted', 'void']);
    expect(documentStatuses('purchase_order')).toEqual([
      'draft',
      'confirmed',
      'partially_received',
      'closed',
      'void',
    ]);
  });

  it('formats_quantities', () => {
    expect(formatQty('10.0000')).toBe('10');
    expect(formatQty('1200.5000')).toBe('1,200.5');
    expect(formatQty('100')).toBe('100');
    expect(formatQty('0.0000')).toBe('0');
  });

  it('explains_stock_insufficient', () => {
    const err = apiError('STOCK_INSUFFICIENT', {
      items: [
        {
          product_id: 'p1',
          product_code: 'A001',
          product_name: '螺絲',
          lot_id: 'l1',
          lot_no: 'L-01',
          required: '10.0000',
          available: '4.0000',
          shortage: '6.0000',
          lines: [
            { detail_key: 'stock_adjustment_line', index: 0 },
            { detail_key: 'stock_adjustment_line', index: 2 },
          ],
        },
        {
          product_id: 'p2',
          product_code: 'B002',
          product_name: '螺帽',
          lot_id: null,
          lot_no: null,
          required: '1500.0000',
          available: '0.0000',
          shortage: '1500.0000',
          lines: [],
        },
      ],
    });
    expect(documentActionError(err, 'post')).toEqual({
      reload: false,
      title: '庫存不足，無法過帳',
      items: [
        'A001 螺絲（批號 L-01）：需要 10，目前可用 4，不足 6（明細第 1、3 列）',
        'B002 螺帽：需要 1,500，目前可用 0，不足 1,500',
      ],
    });
  });

  it('explains_lot_in_use', () => {
    const err = apiError('LOT_IN_USE', {
      lots: [
        {
          lot_id: 'l1',
          lot_no: 'L-01',
          product_id: 'p1',
          product_code: 'A001',
          documents: [
            { entity_key: 'stock_adjustment', record_id: 'r1', record_no: 'ADJ202611-0002' },
            { entity_key: 'stock_adjustment', record_id: 'r2', record_no: 'ADJ202611-0003' },
          ],
        },
      ],
    });
    expect(documentActionError(err, 'void')).toEqual({
      reload: false,
      title: '批號已被其他單據使用，無法作廢',
      items: ['A001 批號 L-01：ADJ202611-0002、ADJ202611-0003'],
    });
  });

  it('conflict_and_invalid_status_suggest_reload', () => {
    expect(documentActionError(apiError('CONFLICT'), 'post')).toEqual({
      reload: true,
      title: '資料已被其他人修改，請重新整理',
      items: [],
    });
    expect(documentActionError(apiError('INVALID_STATUS', { status: 'posted' }), 'post')).toEqual({
      reload: true,
      title: '單據目前為「已過帳」，無法過帳，請重新載入',
      items: [],
    });
    expect(documentActionError(apiError('INVALID_STATUS'), 'void').title).toBe(
      '單據狀態已變更，無法作廢，請重新載入',
    );
  });

  it('falls_back_to_server_message', () => {
    expect(documentActionError(apiError('FORBIDDEN', undefined, '沒有權限'), 'post')).toEqual({
      reload: false,
      title: '過帳失敗：沒有權限',
      items: [],
    });
    // details 格式不符時不當掉
    expect(
      documentActionError(apiError('STOCK_INSUFFICIENT', null, '庫存不足'), 'post').title,
    ).toBe('過帳失敗：庫存不足');
  });
});
