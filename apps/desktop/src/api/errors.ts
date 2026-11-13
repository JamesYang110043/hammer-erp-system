import { CommandError, apiErrorCode, errorMessage } from './client';

/** 樂觀鎖定衝突（409）時顯示的訊息（docs/architecture.md §9.4）。 */
export const CONFLICT_MESSAGE = '資料已被其他人修改，請重新整理';

/**
 * 修改資料失敗時顯示的訊息。
 *
 * - `409 CONFLICT` → {@link CONFLICT_MESSAGE}
 * - 其他（含 `422 VALIDATION`、最後一個管理者保護）→ 伺服器的訊息
 */
export function mutationErrorMessage(err: unknown): string {
  return apiErrorCode(err) === 'CONFLICT' ? CONFLICT_MESSAGE : errorMessage(err);
}

/** `422 VALIDATION` 且 `details.field` 有值時，回傳欄位名稱與訊息（可顯示在表單欄位下方）。 */
export function validationFieldError(err: unknown): { field: string; message: string } | undefined {
  if (!(err instanceof CommandError) || err.error.kind !== 'api') {
    return undefined;
  }
  const { code, message, details } = err.error;
  if (code !== 'VALIDATION' || typeof details !== 'object' || details === null) {
    return undefined;
  }
  const field = (details as Record<string, unknown>).field;
  return typeof field === 'string' ? { field, message } : undefined;
}
