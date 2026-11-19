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
  confirmed: '已確認',
  partially_received: '部分進貨',
  partially_shipped: '部分出貨',
  closed: '已結案',
  void: '已作廢',
};

/** 狀態標籤的顏色（antd `Tag` 的 `color`）。 */
export const DOCUMENT_STATUS_COLORS: Record<DocumentStatus, string> = {
  draft: 'default',
  posted: 'green',
  confirmed: 'blue',
  partially_received: 'gold',
  partially_shipped: 'gold',
  closed: 'green',
  void: 'red',
};

/** 所有單據狀態。 */
export const DOCUMENT_STATUSES: readonly DocumentStatus[] = [
  'draft',
  'posted',
  'confirmed',
  'partially_received',
  'partially_shipped',
  'closed',
  'void',
];

/** 採購單的資料類別代碼。 */
export const PURCHASE_ORDER = 'purchase_order';

/** 訂單的資料類別代碼。 */
export const SALES_ORDER = 'sales_order';

/** 是否為有確認、結案的訂單類單據（採購單、訂單）。 */
function isOrder(entityKey: string): boolean {
  return entityKey === PURCHASE_ORDER || entityKey === SALES_ORDER;
}

/**
 * 資料類別會出現的狀態（列表的狀態篩選）：採購單為草稿、已確認、部分進貨、已結案、已作廢
 * （docs/architecture.md §8.8「採購單的狀態」），訂單為草稿、已確認、部分出貨、已結案、已作廢
 * （§8.9「訂單的狀態」），其他單據為草稿、已過帳、已作廢。
 */
export function documentStatuses(entityKey: string): readonly DocumentStatus[] {
  switch (entityKey) {
    case PURCHASE_ORDER:
      return ['draft', 'confirmed', 'partially_received', 'closed', 'void'];
    case SALES_ORDER:
      return ['draft', 'confirmed', 'partially_shipped', 'closed', 'void'];
    default:
      return ['draft', 'posted', 'void'];
  }
}

/** 單據動作（`hammer_shared::document::DocumentAction`）。 */
export type DocumentAction = 'post' | 'void' | 'confirm' | 'close';

export const DOCUMENT_ACTION_LABELS: Record<DocumentAction, string> = {
  post: '過帳',
  void: '作廢',
  confirm: '確認',
  close: '結案',
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

/**
 * 依目前狀態可以執行的動作（伺服器同樣檢查）：
 *
 * - 採購單、訂單（§8.8「採購單的狀態」、§8.9「訂單的狀態」）：草稿 → 確認；已確認 → 結案、作廢；
 *   部分進貨、部分出貨 → 結案
 * - 其他單據：草稿 → 過帳；已過帳 → 作廢
 */
export function availableActions(
  entityKey: string,
  status: DocumentStatus | undefined,
): DocumentAction[] {
  if (isOrder(entityKey)) {
    switch (status) {
      case 'draft':
        return ['confirm'];
      case 'confirmed':
        return ['close', 'void'];
      case 'partially_received':
      case 'partially_shipped':
        return ['close'];
      default:
        return [];
    }
  }
  switch (status) {
    case 'draft':
      return ['post'];
    case 'posted':
      return ['void'];
    default:
      return [];
  }
}

/** 執行動作前確認對話框的說明（docs/architecture.md §8.2、§8.8、§8.9）。 */
export function actionConfirmText(entityKey: string, action: DocumentAction): string {
  if (entityKey === SALES_ORDER) {
    switch (action) {
      case 'confirm':
        return '確認後金額即鎖定，訂單不能再修改或刪除，之後可以據以出貨。';
      case 'close':
        return '結案後不能再依這張訂單出貨，尚未出貨的數量視為取消。';
      case 'void':
        return '作廢後這張訂單不能再使用，作廢後不能再恢復。';
      default:
        break;
    }
  }
  if (entityKey === PURCHASE_ORDER) {
    switch (action) {
      case 'confirm':
        return '確認後匯率與金額即鎖定，採購單不能再修改或刪除，之後可以據以進貨。';
      case 'close':
        return '結案後不能再依這張採購單進貨，尚未進貨的數量視為取消。';
      case 'void':
        return '作廢後這張採購單不能再使用，作廢後不能再恢復。';
      default:
        break;
    }
  }
  switch (action) {
    case 'post':
      return '過帳後會依明細異動庫存，單據不能再修改或刪除。';
    case 'void':
      return '作廢會反向處理這張單據的庫存異動，作廢後不能再恢復。';
    default:
      return '';
  }
}

/**
 * 非草稿單據的說明文字：目前狀態與確認（過帳）、結案、作廢的時間。
 * 採購單、訂單的 `posted_at` 是確認時間（§8.8、§8.9）。
 */
export function documentStatusNote(
  entityKey: string,
  status: DocumentStatus,
  document: DataRecord['document'] | undefined,
  format: (iso: string) => string,
): string {
  const parts = [`此單據${DOCUMENT_STATUS_LABELS[status]}，不能修改。`];
  const times: string[] = [];
  if (document?.posted_at) {
    times.push(`${isOrder(entityKey) ? '確認' : '過帳'}時間：${format(document.posted_at)}`);
  }
  if (document?.closed_at) {
    times.push(`結案時間：${format(document.closed_at)}`);
  }
  if (document?.voided_at) {
    times.push(`作廢時間：${format(document.voided_at)}`);
  }
  return parts.join('') + times.join('；');
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
 * - `422 VALIDATION` → 伺服器的訊息，`details.fields` 逐項列出（`labels` 為欄位代碼 → 顯示名稱）
 * - 其他 → 伺服器的訊息
 *
 * `actionLabel` 為動作的名稱（發票模組的過帳顯示為「開立」）。
 */
export function documentActionError(
  err: unknown,
  action: DocumentAction,
  labels: Record<string, string> = {},
  actionLabel: string = DOCUMENT_ACTION_LABELS[action],
): DocumentActionError {
  const label = actionLabel;
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
  if (code === 'VALIDATION' && isObject(d) && Array.isArray(d.fields)) {
    const items = d.fields.flatMap((f: unknown) =>
      isObject(f) && typeof f.field_key === 'string' && typeof f.message === 'string'
        ? [`${labels[f.field_key] ?? f.field_key}：${f.message}`]
        : [],
    );
    return { reload: false, title: `${label}失敗：${errorMessage(err)}`, items };
  }
  return { reload: false, title: `${label}失敗：${errorMessage(err)}`, items: [] };
}
