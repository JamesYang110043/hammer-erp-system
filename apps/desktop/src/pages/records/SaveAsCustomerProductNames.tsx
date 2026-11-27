import { useQueryClient } from '@tanstack/react-query';
import { App } from 'antd';

import { errorMessage, unwrap } from '../../api/client';
import { invalidateCustomerProductNames } from '../../api/sales';
import { usePermission } from '../../auth/permission';
import { PERMISSIONS, commands } from '../../bindings';
import type { Record as DataRecord } from '../../bindings';
import {
  SALES_FIELDS,
  SAVE_AS_CUSTOMER_NAMES_TITLE,
  SHIPMENT,
  changedCustomerNames,
  namedLineCandidates,
  salesLineKey,
} from '../../records/sales';
import type { CustomerNameSuggestion } from '../../records/sales';
import { SALES_ORDER } from '../../records/documents';
import { CustomerNameSuggestionTable } from './CustomerNameSuggestionTable';

/**
 * 訂單、出貨單存檔成功後，詢問是否把手動修改的品名存成客戶品名（docs/architecture.md §8.9
 * 「明細的品名」的「存檔後詢問是否存成客戶品名」）。回傳的函式在存檔成功後呼叫。
 *
 * - 沒有 `record.write`、不是訂單或出貨單、沒有不同的列、查詢目前帶入的品名失敗 → 不詢問
 * - 「設為客戶品名」→ 以 id 呼叫客戶品名批次寫入 API（只送勾選的列），成功後清除客戶相關的快取
 * - 「略過」→ 不寫入。不論結果，已存檔的單據都不受影響
 *
 * 對話框由 `App.useApp().modal` 顯示：新增存檔後表單會導向新的網址（元件卸載），對話框仍會保留。
 */
export function useOfferCustomerProductNames() {
  const { modal, message } = App.useApp();
  const queryClient = useQueryClient();
  const canWrite = usePermission(PERMISSIONS.RECORD_WRITE);

  return async (entityKey: string, saved: DataRecord) => {
    if (!canWrite || (entityKey !== SALES_ORDER && entityKey !== SHIPMENT)) {
      return;
    }
    const lineKey = salesLineKey(entityKey);
    const customer = saved.data[SALES_FIELDS.customer];
    if (!lineKey || typeof customer !== 'string' || !customer) {
      return;
    }
    const candidates = namedLineCandidates(saved.lines?.[lineKey] ?? []);
    if (candidates.length === 0) {
      return;
    }
    let rows: CustomerNameSuggestion[];
    try {
      const names = await unwrap(
        commands.salesProductNames(
          customer,
          candidates.map((c) => c.product_id),
        ),
      );
      rows = changedCustomerNames(candidates, names);
    } catch {
      // 單據已存檔：查不到目前的品名就不詢問
      return;
    }
    if (rows.length === 0) {
      return;
    }

    let selected = rows.map((r) => r.product_id);
    const instance = modal.confirm({
      title: SAVE_AS_CUSTOMER_NAMES_TITLE,
      width: 720,
      icon: null,
      content: (
        <CustomerNameSuggestionTable
          rows={rows}
          onChange={(next) => {
            selected = next;
            instance.update({ okButtonProps: { disabled: next.length === 0 } });
          }}
        />
      ),
      okText: '設為客戶品名',
      cancelText: '略過',
      onOk: async () => {
        const picked = rows.filter((r) => selected.includes(r.product_id));
        try {
          const result = await unwrap(
            commands.customerProductNamesBatch({
              dry_run: false,
              rows: picked.map((r) => ({
                customer_id: customer,
                product_id: r.product_id,
                name: r.document_name,
              })),
            }),
          );
          await invalidateCustomerProductNames(queryClient);
          message.success(`已設定 ${result.summary.create + result.summary.update} 筆客戶品名`);
        } catch (err) {
          message.error(`設定客戶品名失敗：${errorMessage(err)}`);
        }
      },
    });
  };
}
