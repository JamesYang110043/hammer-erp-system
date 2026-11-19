import { keepPreviousData, useQuery } from '@tanstack/react-query';

import { commands } from '../bindings';
import type { Record as DataRecord } from '../bindings';
import {
  SALES_FIELDS,
  SALES_ORDER,
  SALES_ORDER_LINE,
  SHIPMENT,
  unshippedQty,
} from '../records/sales';
import type { PreviewInput } from '../records/sales';
import { unwrap } from './client';
import { inventoryKey } from './inventory';
import { MAX_PAGE_SIZE, recordKey } from './records';

/**
 * 出貨的建議批號（docs/architecture.md §8.9「建議批號 API」，需要 `record.read`）。
 * 只試算，不鎖定、不寫入；`inputs` 為空時不查詢。放在庫存的快取底下：單據過帳、作廢後重新取得。
 */
export function useAllocationPreview(inputs: readonly PreviewInput[]) {
  const lines = inputs.map(({ product_id, qty, lot_no }) => ({ product_id, qty, lot_no }));
  return useQuery({
    queryKey: [...inventoryKey, 'allocation-preview', lines],
    queryFn: () => unwrap(commands.allocationPreview({ lines })),
    enabled: lines.length > 0,
    placeholderData: keepPreviousData,
    retry: false,
  });
}

/** 訂單的查詢設定（出貨單帶入明細、檢查未出貨數量用，`useQuery` 與 `fetchQuery` 共用）。 */
export function salesOrderQuery(id: string) {
  return {
    queryKey: recordKey(SALES_ORDER, id),
    queryFn: () => unwrap(commands.getRecord(SALES_ORDER, id)),
  };
}

/** 訂單每一列的未出貨數量（鍵為訂單明細 id）。 */
export function unshippedByLine(order: Pick<DataRecord, 'lines'> | undefined): Map<string, string> {
  const result = new Map<string, string>();
  for (const line of order?.lines?.[SALES_ORDER_LINE] ?? []) {
    const qty = unshippedQty(line.data[SALES_FIELDS.qty], line.data[SALES_FIELDS.shippedQty]);
    if (qty) {
      result.set(line.id, qty);
    }
  }
  return result;
}

/** 出貨單草稿指向的訂單；沒有訂單時不查詢。 */
export function useSalesOrder(id: string | undefined) {
  return useQuery({ ...salesOrderQuery(id ?? ''), enabled: Boolean(id), staleTime: 30_000 });
}

/**
 * 出貨單實際扣用的批號：此出貨單的出貨異動（`txn_type = shipment`），過帳後查詢。
 */
export function useShipmentTransactions(shipmentId: string | undefined) {
  return useQuery({
    queryKey: [...inventoryKey, 'transactions', 'shipment', shipmentId ?? ''],
    queryFn: () =>
      unwrap(
        commands.listInventoryTransactions({
          ref_doc_type: SHIPMENT,
          ref_doc_id: shipmentId!,
          txn_type: 'shipment',
          page: 1,
          page_size: MAX_PAGE_SIZE,
        }),
      ),
    enabled: Boolean(shipmentId),
  });
}
