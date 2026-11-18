import { CommandError, apiErrorCode, errorMessage } from '../api/client';
import { CONFLICT_MESSAGE } from '../api/errors';
import type {
  DocumentStatus,
  EntityDefinition,
  InvalidStatusDetails,
  LotInUseDetails,
  Record as DataRecord,
  StockInsufficientDetails_Serialize as StockInsufficientDetails,
} from '../bindings';
import { groupThousands } from './display';

/**
 * 系統單據的畫面共通規則（docs/architecture.md §7.1「系統單據」、§8.2、§10「單據動作」）。
 *
 * 系統單據（例如盤點調整單）沿用動態列表與表單；狀態 `data.status` 由伺服器維護：
 * 只有草稿可以修改、刪除、過帳，已過帳可以作廢，已作廢不能再做任何操作。
 */

/** 單據狀態的系統欄位代碼（`hammer_shared::document::STATUS_FIELD`）。 */
export const STATUS_FIELD = 'status';

/** 單據狀態的顯示名稱（`DocumentStatus::label`）。 */
export const DOCUMENT_STATUS_LABELS: Record<DocumentStatus, string> = {
  draft: '草稿',
  posted: '已過帳',
  void: '已作廢',
};

/** 狀態標籤的顏色（antd `Tag` 的 `color`）。 */
export const DOCUMENT_STATUS_COLORS: Record<DocumentStatus, string> = {
  draft: 'default',
  posted: 'green',
  void: 'red',
};

export const DOCUMENT_STATUSES: readonly DocumentStatus[] = ['draft', 'posted', 'void'];

/** 單據動作。 */
export type DocumentAction = 'post' | 'void';

export const DOCUMENT_ACTION_LABELS: Record<DocumentAction, string> = {
  post: '過帳',
  void: '作廢',
};

/** 是否為有過帳、作廢的系統單據（`kind = document` 的系統資料類別）。 */
export function isSystemDocument(entity: Pick<EntityDefinition, 'kind' | 'is_system'>): boolean {
  return entity.kind === 'document' && entity.is_system;
}

/** 單據目前的狀態；不認得的值回傳 `undefined`。 */
export function documentStatus(
  record: Pick<DataRecord, 'data'> | undefined,
): DocumentStatus | undefined {
  const value = record?.data[STATUS_FIELD];
  return typeof value === 'string' && (DOCUMENT_STATUSES as readonly string[]).includes(value)
    ? (value as DocumentStatus)
    : undefined;
}

/**
 * 系統單據是否可以修改、刪除：只有草稿可以（伺服器對其他狀態回傳 `422 INVALID_STATUS`）。
 * 不是系統單據時一律可以。
 */
export function isEditableDocument(entity: EntityDefinition, record?: DataRecord): boolean {
  if (!isSystemDocument(entity) || !record) {
    return true;
  }
  return documentStatus(record) === 'draft';
}

/** 依目前狀態可以執行的動作：草稿 → 過帳，已過帳 → 作廢，已作廢 → 無。 */
export function availableAction(status: DocumentStatus | undefined): DocumentAction | undefined {
  switch (status) {
    case 'draft':
      return 'post';
    case 'posted':
      return 'void';
    default:
      return undefined;
  }
}

/** 數量顯示：去掉小數尾端的 0，加上千分位（`"1200.5000"` → `"1,200.5"`）。 */
export function formatQty(raw: string): string {
  const trimmed = raw.includes('.') ? raw.replace(/\.?0+$/, '') : raw;
  return groupThousands(trimmed);
}

function details(err: unknown): unknown {
  return err instanceof CommandError && err.error.kind === 'api' ? err.error.details : undefined;
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

/** 明細位置的文字：`[1, 3]`（從 0 開始）→ `第 2、4 列`。 */
function lineText(lines: readonly { index: number }[]): string {
  return lines.length > 0 ? `（明細第 ${lines.map((l) => l.index + 1).join('、')} 列）` : '';
}

/** 單據動作失敗時要顯示的內容：標題與逐項說明。 */
export interface DocumentActionError {
  /** 畫面上的資料已經過時（版本衝突、狀態已變更），應提示重新載入。 */
  reload: boolean;
  title: string;
  items: string[];
}

/**
 * 把過帳、作廢的錯誤轉成易懂的訊息（docs/architecture.md §10「單據動作」）。
 *
 * - `409 CONFLICT` → 提示重新載入
 * - `422 STOCK_INSUFFICIENT` → 逐項列出不足的商品（批號）與數量
 * - `422 LOT_IN_USE` → 逐項列出被其他單據使用的批號與單號
 * - `422 INVALID_STATUS` → 說明單據目前的狀態，提示重新載入
 * - 其他 → 伺服器的訊息（`422 VALIDATION` 的明細錯誤由表單另外處理）
 */
export function documentActionError(err: unknown, action: DocumentAction): DocumentActionError {
  const label = DOCUMENT_ACTION_LABELS[action];
  const code = apiErrorCode(err);
  const d = details(err);
  if (code === 'CONFLICT') {
    return { reload: true, title: CONFLICT_MESSAGE, items: [] };
  }
  if (code === 'STOCK_INSUFFICIENT' && isObject(d) && Array.isArray(d.items)) {
    const { items } = d as StockInsufficientDetails;
    return {
      reload: false,
      title: `庫存不足，無法${label}`,
      items: items.map((item) => {
        const name = `${item.product_code} ${item.product_name}`;
        const lot = item.lot_no ? `（批號 ${item.lot_no}）` : '';
        return (
          `${name}${lot}：需要 ${formatQty(item.required)}，目前可用 ${formatQty(item.available)}，` +
          `不足 ${formatQty(item.shortage)}${lineText(item.lines)}`
        );
      }),
    };
  }
  if (code === 'LOT_IN_USE' && isObject(d) && Array.isArray(d.lots)) {
    const { lots } = d as LotInUseDetails;
    return {
      reload: false,
      title: `批號已被其他單據使用，無法${label}`,
      items: lots.map(
        (lot) =>
          `${lot.product_code} 批號 ${lot.lot_no}：${lot.documents.map((doc) => doc.record_no).join('、')}`,
      ),
    };
  }
  if (code === 'INVALID_STATUS') {
    const status = isObject(d) ? (d as Partial<InvalidStatusDetails>).status : undefined;
    const current = status ? DOCUMENT_STATUS_LABELS[status] : undefined;
    return {
      reload: true,
      title: current
        ? `單據目前為「${current}」，無法${label}，請重新載入`
        : `單據狀態已變更，無法${label}，請重新載入`,
      items: [],
    };
  }
  return { reload: false, title: `${label}失敗：${errorMessage(err)}`, items: [] };
}
