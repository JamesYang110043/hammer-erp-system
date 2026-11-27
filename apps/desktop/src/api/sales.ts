import { keepPreviousData, useQuery } from '@tanstack/react-query';

import { commands } from '../bindings';
import type { Record as DataRecord } from '../bindings';
import { documentStatus } from '../records/documents';
import {
  SALES_FIELDS,
  SALES_ORDER,
  SALES_ORDER_LINE,
  SALES_RETURN,
  SHIPMENT,
  salesReturnableQty,
  unshippedQty,
} from '../records/sales';
import type { PreviewInput } from '../records/sales';
import { unwrap } from './client';
import { inventoryKey } from './inventory';
import { MAX_PAGE_SIZE, productCustomerNamesKey, recordKey, recordsKey } from './records';

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

/** 出貨單與其每一列的可退數量。 */
export interface ReturnableShipment {
  shipment: DataRecord;
  /** 鍵為出貨明細 id。 */
  returnable: Map<string, string>;
}

/**
 * 出貨單的可退數量查詢設定（`useQuery` 與 `queryClient.fetchQuery` 共用）：讀取出貨單，
 * 以及指向它、已過帳的銷貨退回（含明細），依 `salesReturnableQty` 計算（docs/architecture.md §8.9「銷貨退回」）。
 *
 * 放在銷貨退回的 {@link recordsKey} 底下：退回單存檔、過帳、作廢後一起重新取得。
 */
export function shipmentReturnableQuery(shipmentId: string) {
  return {
    queryKey: [...recordsKey(SALES_RETURN), 'returnable', shipmentId],
    queryFn: async (): Promise<ReturnableShipment> => {
      const shipment = await unwrap(commands.getRecord(SHIPMENT, shipmentId));
      const posted =
        documentStatus(shipment) === 'posted'
          ? await unwrap(
              commands.listRecords(SALES_RETURN, {
                page: 1,
                page_size: MAX_PAGE_SIZE,
                sort: null,
                filters: [
                  { field_key: SALES_FIELDS.shipment, op: 'eq', value: shipmentId },
                  { field_key: 'status', op: 'eq', value: 'posted' },
                ],
              }),
            )
          : { items: [] };
      // 列表不含明細，逐筆讀取
      const returns = await Promise.all(
        posted.items.map((r) => unwrap(commands.getRecord(SALES_RETURN, r.id))),
      );
      return { shipment, returnable: salesReturnableQty(shipment, returns) };
    },
    staleTime: 30_000,
  };
}

/** 出貨單的可退數量；`shipmentId` 沒有值時不查詢。 */
export function useShipmentReturnable(shipmentId: string | undefined) {
  return useQuery({
    ...shipmentReturnableQuery(shipmentId ?? ''),
    enabled: Boolean(shipmentId),
  });
}

/**
 * 出貨單扣用、銷貨退回加回的批號：單據本身的庫存異動（`txn_type` 與 `ref_doc_type` 都是
 * `shipment` 或 `sales_return`），過帳後查詢。
 */
export function useDocumentLotTransactions(
  docType: typeof SHIPMENT | typeof SALES_RETURN,
  id: string | undefined,
) {
  return useQuery({
    queryKey: [...inventoryKey, 'transactions', docType, id ?? ''],
    queryFn: () =>
      unwrap(
        commands.listInventoryTransactions({
          ref_doc_type: docType,
          ref_doc_id: id!,
          txn_type: docType,
          page: 1,
          page_size: MAX_PAGE_SIZE,
        }),
      ),
    enabled: Boolean(id),
  });
}

/** 銷貨用商品選擇 API 的快取鍵（客戶品名、商品名稱改變時一併失效）。 */
export const salesProductLookupKey = ['sales-product-lookup'] as const;

/**
 * 銷貨用商品選擇的查詢設定（docs/architecture.md §8.9「銷貨用商品選擇 API」，需要 `record.read`）：
 * 比對商品標題與客戶 `customerId` 的客戶品名，回傳依帶入順序決定的品名。`useQuery` 與 `fetchQuery` 共用。
 */
export function salesProductLookupQuery(customerId: string | undefined, q: string, limit?: number) {
  return {
    queryKey: [...salesProductLookupKey, customerId ?? '', q, limit ?? null] as const,
    queryFn: () =>
      unwrap(
        commands.salesProductLookup({
          customer_id: customerId ?? null,
          q: q || null,
          limit: limit ?? null,
        }),
      ),
  };
}

/** 銷貨單據明細的商品選擇器使用；`enabled` 為 `false` 時不查詢（下拉選單還沒打開）。 */
export function useSalesProductLookup(customerId: string | undefined, q: string, enabled: boolean) {
  return useQuery({
    ...salesProductLookupQuery(customerId, q),
    enabled,
    placeholderData: keepPreviousData,
  });
}

/** 批次取得明細品名的快取鍵（客戶品名、商品名稱改變時一併失效）。 */
export const salesProductNamesKey = ['sales-product-names'] as const;

/**
 * 批次取得明細品名的查詢設定（docs/architecture.md §8.9「批次取得明細品名 API」，需要 `record.read`）：
 * 一次取得 `productIds`（1–200 個）對客戶 `customerId` 依帶入順序決定的品名。已刪除的商品不回傳。
 */
export function salesProductNamesQuery(customerId: string, productIds: readonly string[]) {
  return {
    queryKey: [...salesProductNamesKey, customerId, ...productIds] as const,
    queryFn: () => unwrap(commands.salesProductNames(customerId, [...productIds])),
  };
}

/**
 * 商品 `productId` 的客戶品名（docs/architecture.md §8.7「商品畫面的客戶品名」，需要 `record.read`）：
 * 各客戶的客戶品名，依客戶編號排序。客戶存檔後由 {@link productCustomerNamesKey} 清除快取。
 */
export function useProductCustomerNames(productId: string | undefined) {
  return useQuery({
    queryKey: [...productCustomerNamesKey, productId ?? ''] as const,
    queryFn: () => unwrap(commands.productCustomerNames(productId!)),
    enabled: Boolean(productId),
  });
}
