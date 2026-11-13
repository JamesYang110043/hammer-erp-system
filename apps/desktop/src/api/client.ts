import type { ClientError, ErrorCode } from '../bindings';

/** Tauri command 回傳的錯誤，包成 `Error` 讓 TanStack Query 處理。 */
export class CommandError extends Error {
  readonly error: ClientError;

  constructor(error: ClientError) {
    super(error.message);
    this.name = 'CommandError';
    this.error = error;
  }
}

type CommandResult<T> = { status: 'ok'; data: T } | { status: 'error'; error: ClientError };

/** 把 tauri-specta 的結果轉成「成功回傳資料、失敗丟出 {@link CommandError}」。 */
export async function unwrap<T>(result: Promise<CommandResult<T>>): Promise<T> {
  const r = await result;
  if (r.status === 'error') {
    throw new CommandError(r.error);
  }
  return r.data;
}

/** 取得可以顯示給使用者的錯誤訊息。 */
export function errorMessage(err: unknown): string {
  if (err instanceof CommandError) {
    if (err.error.kind === 'api' && err.error.code === 'CLIENT_OUTDATED') {
      return '程式版本過舊，請更新後再使用';
    }
    return err.error.message;
  }
  if (err instanceof Error) {
    return err.message;
  }
  return String(err);
}

/** 登入已逾時（Rust 端已清除 token），應回到登入頁。 */
export function isSessionExpired(err: unknown): boolean {
  return err instanceof CommandError && err.error.kind === 'sessionExpired';
}

/** 伺服器回傳的錯誤碼；不是伺服器錯誤時回傳 `undefined`。 */
export function apiErrorCode(err: unknown): ErrorCode | undefined {
  return err instanceof CommandError && err.error.kind === 'api' ? err.error.code : undefined;
}
