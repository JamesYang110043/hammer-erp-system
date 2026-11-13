import type { CurrentUser } from '../bindings';
import { PERMISSIONS } from '../bindings';
import type { CommandHandler } from './ipc';

/** 管理者（擁有全部權限）。 */
export const ADMIN: CurrentUser = {
  id: '0196f5a0-0000-7000-8000-000000000001',
  username: 'admin',
  display_name: '王管理',
  role: 'admin',
  permissions: Object.values(PERMISSIONS).sort(),
  must_change_password: false,
};

/** 唯讀者（docs/architecture.md §6.1）。 */
export const VIEWER: CurrentUser = {
  id: '0196f5a0-0000-7000-8000-000000000002',
  username: 'viewer',
  display_name: '李唯讀',
  role: 'viewer',
  permissions: [PERMISSIONS.DATA_EXPORT, PERMISSIONS.RECORD_READ],
  must_change_password: false,
};

/** 啟動流程與狀態列需要的 command。`user` 為 `null` 表示鑰匙圈沒有 token。 */
export function startupCommands(
  user: CurrentUser | null,
  { needsSetup = false }: { needsSetup?: boolean } = {},
): Record<string, CommandHandler> {
  return {
    setup_status: () => ({ needs_setup: needsSetup }),
    current_user: () => user,
    health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
    get_server_url: () => 'http://localhost:8080',
  };
}
