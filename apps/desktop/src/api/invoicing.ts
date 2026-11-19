import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';

import { commands } from '../bindings';
import type { Record as DataRecord } from '../bindings';
import { INVOICING_FIELDS as F, SALES_ALLOWANCE } from '../records/invoicing';
import { auditLogsKey } from './auditLogs';
import { unwrap } from './client';
import { inventoryKey } from './inventory';
import { MAX_PAGE_SIZE, recordKey, recordsKey } from './records';

/** 發票模組查詢 API 的快取鍵（單據存檔、開立、作廢後一起重新取得）。 */
export const invoicingKey = ['invoicing'] as const;

/**
 * 尚未開立發票的已過帳出貨單（docs/architecture.md §8.11「查詢 API」）；沒有客戶時不查詢。
 * 最多一頁 {@link MAX_PAGE_SIZE} 筆。
 */
export function useUninvoicedShipments(customerId: string | undefined, enabled = true) {
  return useQuery({
    queryKey: [...invoicingKey, 'uninvoiced-shipments', customerId ?? ''],
    queryFn: () =>
      unwrap(
        commands.listUninvoicedShipments({
          customer_id: customerId ?? null,
          date_from: null,
          date_to: null,
          page: 1,
          page_size: MAX_PAGE_SIZE,
        }),
      ),
    enabled: enabled && Boolean(customerId),
  });
}

/** 關聯單據（出貨單、進貨單、退回單）的發票、憑證或折讓；沒有 id 時不查詢。 */
export function useInvoiceLinks(entityKey: string, id: string | undefined, enabled = true) {
  return useQuery({
    queryKey: [...invoicingKey, 'links', entityKey, id ?? ''],
    queryFn: () => unwrap(commands.listInvoiceLinks({ entity_key: entityKey, record_id: id! })),
    enabled: enabled && Boolean(id),
  });
}

/** 發票已開立的銷貨折讓（計算可折讓金額）；沒有發票時不查詢。 */
export function usePostedAllowances(invoiceId: string | undefined) {
  return useQuery({
    queryKey: [...recordsKey(SALES_ALLOWANCE), 'posted-of', invoiceId ?? ''],
    queryFn: () =>
      unwrap(
        commands.listRecords(SALES_ALLOWANCE, {
          page: 1,
          page_size: MAX_PAGE_SIZE,
          sort: null,
          filters: [
            { field_key: F.salesInvoice, op: 'eq', value: invoiceId! },
            { field_key: 'status', op: 'eq', value: 'posted' },
          ],
        }),
      ),
    enabled: Boolean(invoiceId),
  });
}

/** 讀取一筆資料（帶入用，`fetchQuery` 與 `useQuery` 共用）。 */
export function recordQuery(entityKey: string, id: string) {
  return {
    queryKey: recordKey(entityKey, id),
    queryFn: () => unwrap(commands.getRecord(entityKey, id)),
  };
}

/** 一筆資料；沒有 id 時不查詢。 */
export function useRecordById(entityKey: string, id: string | undefined) {
  return useQuery({ ...recordQuery(entityKey, id ?? ''), enabled: Boolean(id), staleTime: 30_000 });
}

/** 作廢的輸入。 */
export interface VoidInput {
  record: Pick<DataRecord, 'id' | 'version'>;
  /** `YYYY-MM-DD`；`null` 為作廢當天。 */
  voidDate: string | null;
  voidReason: string;
}

/**
 * 發票模組單據的作廢（帶作廢日期與原因，§8.11「開立與作廢」）。成功後重新取得單據、列表、
 * 查詢 API（出貨單重新出現在「尚未開立發票」）與操作紀錄。
 */
export function useVoidInvoice(entityKey: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ record, voidDate, voidReason }: VoidInput) =>
      unwrap(
        commands.voidInvoiceRecord(entityKey, record.id, record.version, voidDate, voidReason),
      ),
    onSettled: () =>
      Promise.all(
        [invoicingKey, inventoryKey, ['records'], ['record'], auditLogsKey].map((queryKey) =>
          queryClient.invalidateQueries({ queryKey }),
        ),
      ),
  });
}
