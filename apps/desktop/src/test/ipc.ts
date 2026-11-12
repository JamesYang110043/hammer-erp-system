import { mockIPC } from '@tauri-apps/api/mocks';
import { vi } from 'vitest';

import type { ClientError } from '../bindings';

/** 單一 Tauri command 的 mock：收到參數，回傳結果（可以是 Promise）。 */
export type CommandHandler = (args: Record<string, unknown>) => unknown;

/**
 * 以 `mockIPC` 模擬 Tauri command（docs/testing.md §1）。
 *
 * - key 是 Rust 端的 command 名稱（snake_case），例如 `health_check`
 * - 呼叫到沒有 mock 的 command 會丟出錯誤，讓測試直接失敗
 * - 模擬 command 回傳錯誤時，handler 回傳 {@link commandError}
 *
 * 回傳的 spy 記錄每次呼叫的 `(command, args)`，可用來檢查呼叫參數。
 *
 * ```ts
 * const invoke = mockCommands({
 *   health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
 * });
 * expect(invoke).toHaveBeenCalledWith('health_check', { serverUrl: null });
 * ```
 */
export function mockCommands(handlers: Record<string, CommandHandler>) {
  const invoke = vi.fn((cmd: string, args: Record<string, unknown>) => {
    const handler = handlers[cmd];
    if (!handler) {
      throw new Error(`測試沒有 mock 這個 Tauri command：${cmd}`);
    }
    return handler(args);
  });
  mockIPC((cmd, args) => invoke(cmd, (args ?? {}) as Record<string, unknown>));
  return invoke;
}

/**
 * 模擬 Rust command 回傳 `Err(ClientError)`。
 *
 * Tauri 會把錯誤以一般物件（不是 `Error`）reject，tauri-specta 產生的程式碼
 * 再把它轉成 `{ status: 'error', error }`。
 */
export function commandError(error: ClientError): Promise<never> {
  return Promise.reject(error);
}
