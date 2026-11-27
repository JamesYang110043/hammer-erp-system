import type {
  CustomerProductNameBatchAction,
  CustomerProductNameBatchResult_Serialize as CustomerProductNameBatchResult,
  CustomerProductNameBatchRow_Deserialize as CustomerProductNameBatchRow,
  ImportedCustomerProductName,
} from '../bindings';

/**
 * Excel 匯入客戶品名（docs/architecture.md §8.7「Excel 匯入客戶品名」）的畫面邏輯：解析結果轉成批次寫入
 * API 的列、預覽的文字與能否匯入。
 */

/** 動作的顯示文字。 */
export const IMPORT_ACTION_LABELS: Record<CustomerProductNameBatchAction, string> = {
  create: '新增',
  update: '修改',
  unchanged: '不變',
};

/** Excel 的列轉成批次寫入的列：以編號指定；沒有備註時不帶 `notes`（保留既有的備註）。 */
export function batchRowsOf(
  rows: readonly ImportedCustomerProductName[],
): CustomerProductNameBatchRow[] {
  return rows.map((r) => ({
    customer_code: r.customer_code,
    product_code: r.product_code,
    name: r.name,
    ...(r.notes === null ? {} : { notes: r.notes }),
  }));
}

/** 預覽上方的筆數：「新增 2、修改 1、不變 0、錯誤 1」。 */
export function importSummaryText(result: CustomerProductNameBatchResult): string {
  const { create, update, unchanged, error } = result.summary;
  return `新增 ${create}、修改 ${update}、不變 ${unchanged}、錯誤 ${error}`;
}

/** 不能匯入的原因；可以匯入時為 `undefined`。 */
export function importBlockedReason(result: CustomerProductNameBatchResult): string | undefined {
  if (result.summary.error > 0) {
    return '請修正 Excel 後重新選擇檔案';
  }
  if (result.summary.create + result.summary.update === 0) {
    return '沒有需要匯入的資料';
  }
  return undefined;
}

/** 匯入成功的訊息。 */
export function importSuccessText(result: CustomerProductNameBatchResult): string {
  return `已匯入客戶品名：新增 ${result.summary.create} 筆、修改 ${result.summary.update} 筆`;
}

/**
 * 寫入時的 `422 VALIDATION`：`details` 為與預覽相同格式的結果（預覽之後資料被別人改了），回傳它以更新
 * 預覽；其他錯誤為 `undefined`。
 */
export function rejectedBatchResult(
  error: { code: string; details?: unknown } | undefined,
): CustomerProductNameBatchResult | undefined {
  const details = error?.code === 'VALIDATION' ? error.details : undefined;
  if (
    details &&
    typeof details === 'object' &&
    'rows' in details &&
    'summary' in details &&
    Array.isArray((details as { rows: unknown }).rows)
  ) {
    return details as CustomerProductNameBatchResult;
  }
  return undefined;
}
