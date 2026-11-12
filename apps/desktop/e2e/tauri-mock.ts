import type { Page } from '@playwright/test';

import type { ClientError } from '../src/bindings';

/** 單一 Tauri command 的 mock，在 Node 端（測試程式）執行。 */
export type E2eCommandHandler = (args: Record<string, unknown>) => unknown;

/** 從 handler 丟出，模擬 Rust command 回傳 `Err(ClientError)`。 */
export class CommandRejection {
  readonly error: ClientError;

  constructor(error: ClientError) {
    this.error = error;
  }
}

type InvokeOutcome =
  { kind: 'ok'; value: unknown } | { kind: 'error'; error: ClientError } | { kind: 'missing' };

const BRIDGE = '__hammerE2eInvoke';

/**
 * 在瀏覽器裡模擬 Tauri IPC：前端的 `invoke()` 轉到 Node 端的 handler。
 *
 * 等同單元測試用的 `mockIPC`（src/test/ipc.ts），差別是 Playwright 的瀏覽器
 * 沒辦法 import 測試程式，所以用 `exposeFunction` 把呼叫轉回 Node 端。
 * 必須在 `page.goto()` 之前呼叫。
 */
export async function mockTauri(page: Page, handlers: Record<string, E2eCommandHandler>) {
  await page.exposeFunction(
    BRIDGE,
    async (cmd: string, args: Record<string, unknown> | undefined): Promise<InvokeOutcome> => {
      const handler = handlers[cmd];
      if (!handler) {
        return { kind: 'missing' };
      }
      try {
        return { kind: 'ok', value: await handler(args ?? {}) };
      } catch (e) {
        if (e instanceof CommandRejection) {
          return { kind: 'error', error: e.error };
        }
        throw e;
      }
    },
  );

  await page.addInitScript((bridge: string) => {
    const w = window as unknown as Record<string, unknown>;
    w.__TAURI_INTERNALS__ = {
      invoke: async (cmd: string, args?: Record<string, unknown>) => {
        const call = w[bridge] as (cmd: string, args?: unknown) => Promise<InvokeOutcome>;
        const outcome = await call(cmd, args);
        if (outcome.kind === 'ok') {
          return outcome.value;
        }
        if (outcome.kind === 'error') {
          // 與 Tauri 相同：錯誤以一般物件 reject
          throw outcome.error;
        }
        throw new Error(`E2E 沒有 mock 這個 Tauri command：${cmd}`);
      },
    };
  }, BRIDGE);
}
