import type { Dayjs } from 'dayjs';

import { AUDIT_ACTIONS } from '../../bindings';
import type { AuditLogQuery } from '../../bindings';
import { taipeiStartOfDay } from '../../utils/time';

export type AuditAction = (typeof AUDIT_ACTIONS)[number];

/** 動作代碼的顯示名稱（代碼由 Rust 的 `hammer_shared::audit::ALL` 產生）。 */
export const AUDIT_ACTION_LABELS: Record<AuditAction, string> = {
  create: '新增',
  update: '修改',
  delete: '刪除',
  post: '過帳',
  void: '作廢',
  login: '登入',
  login_failed: '登入失敗',
  logout: '登出',
  password_change: '變更密碼',
  export: '匯出',
};

/** 資料類別代碼的顯示名稱；沒有列出的直接顯示代碼。 */
const ENTITY_LABELS: Record<string, string> = {
  // `hammer_core::auth::login::USER_ENTITY_KEY`
  user: '帳號',
};

export function actionLabel(action: string): string {
  return AUDIT_ACTION_LABELS[action as AuditAction] ?? action;
}

export function entityLabel(entityKey: string): string {
  return ENTITY_LABELS[entityKey] ?? entityKey;
}

/** 篩選表單的值。 */
export interface AuditLogFilters {
  /** 日期區間（含開始、含結束日），以台北時間計算。 */
  dateRange?: [Dayjs | null, Dayjs | null] | null;
  userId?: string;
  action?: AuditAction;
}

export const DEFAULT_PAGE_SIZE = 50;

/**
 * 篩選條件與分頁 → `GET /api/v1/audit-logs` 的參數。
 *
 * 伺服器的時間區間是「含開始、不含結束」，所以結束日要加一天：
 * 選 11/1–11/3 → `from = 11/1 00:00+08:00`、`to = 11/4 00:00+08:00`。
 */
export function buildAuditLogQuery(
  filters: AuditLogFilters,
  page: number,
  pageSize: number,
): AuditLogQuery {
  const [start, end] = filters.dateRange ?? [null, null];
  return {
    from: start ? taipeiStartOfDay(start) : null,
    to: end ? taipeiStartOfDay(end.add(1, 'day')) : null,
    user_id: filters.userId ?? null,
    action: filters.action ?? null,
    entity_key: null,
    record_id: null,
    page,
    page_size: pageSize,
  };
}
