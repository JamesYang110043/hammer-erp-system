import { useQuery } from '@tanstack/react-query';

import { commands } from '../bindings';
import type { Record as DataRecord } from '../bindings';
import { documentStatus } from '../records/documents';
import {
  GOODS_RECEIPT,
  PURCHASE_RETURN,
  PURCHASING_FIELDS,
  returnableQty,
} from '../records/purchasing';
import { unwrap } from './client';
import { MAX_PAGE_SIZE, recordsKey } from './records';

/** 進貨單與其每一列的可退數量。 */
export interface ReturnableReceipt {
  receipt: DataRecord;
  /** 鍵為進貨明細 id。 */
  returnable: Map<string, string>;
}

/**
 * 進貨單的可退數量查詢設定（`useQuery` 與 `queryClient.fetchQuery` 共用）：讀取進貨單，
 * 以及指向它、已過帳的進貨退回（含明細），依 `returnableQty` 計算（docs/architecture.md §8.8「進貨退回」）。
 *
 * 放在進貨退回的 {@link recordsKey} 底下：退回單存檔、過帳、作廢後一起重新取得。
 */
export function returnableQuery(goodsReceiptId: string) {
  return {
    queryKey: [...recordsKey(PURCHASE_RETURN), 'returnable', goodsReceiptId],
    queryFn: async (): Promise<ReturnableReceipt> => {
      const receipt = await unwrap(commands.getRecord(GOODS_RECEIPT, goodsReceiptId));
      const posted =
        documentStatus(receipt) === 'posted'
          ? await unwrap(
              commands.listRecords(PURCHASE_RETURN, {
                page: 1,
                page_size: MAX_PAGE_SIZE,
                sort: null,
                filters: [
                  { field_key: PURCHASING_FIELDS.goodsReceipt, op: 'eq', value: goodsReceiptId },
                  { field_key: 'status', op: 'eq', value: 'posted' },
                ],
              }),
            )
          : { items: [] };
      // 列表不含明細，逐筆讀取
      const returns = await Promise.all(
        posted.items.map((r) => unwrap(commands.getRecord(PURCHASE_RETURN, r.id))),
      );
      return { receipt, returnable: returnableQty(receipt, returns) };
    },
    staleTime: 30_000,
  };
}

/** 進貨單的可退數量；`goodsReceiptId` 沒有值時不查詢。 */
export function useReturnable(goodsReceiptId: string | undefined) {
  return useQuery({
    ...returnableQuery(goodsReceiptId ?? ''),
    enabled: Boolean(goodsReceiptId),
  });
}
