import type { CurrentUser, Role, User } from '../bindings';
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

/** 帳號管理頁的角色列表（`list_roles`）。 */
export const ROLES: Role[] = [
  { id: '0196f5a0-0000-7000-8000-0000000000a1', code: 'admin', name: '管理者', permissions: [] },
  { id: '0196f5a0-0000-7000-8000-0000000000a2', code: 'editor', name: '可編輯者', permissions: [] },
  { id: '0196f5a0-0000-7000-8000-0000000000a3', code: 'viewer', name: '唯讀者', permissions: [] },
];

/** 帳號管理頁的一筆帳號（`list_users`）。 */
export function managedUser(patch: Partial<User> = {}): User {
  return {
    id: '0196f5a0-0000-7000-8000-000000000010',
    username: 'editor1',
    display_name: '陳編輯',
    role: 'editor',
    role_name: '可編輯者',
    is_active: true,
    is_locked: false,
    locked_until: null,
    must_change_password: false,
    // 台北時間 2026-11-13 09:05:30
    last_login_at: '2026-11-13T01:05:30Z',
    created_at: '2026-11-12T08:00:00Z',
    updated_at: '2026-11-12T08:00:00Z',
    version: 3,
    ...patch,
  };
}
