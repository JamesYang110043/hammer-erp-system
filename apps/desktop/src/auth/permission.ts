import { PERMISSIONS } from '../bindings';
import type { CurrentUser } from '../bindings';
import { useSession } from './session';

/** 權限代碼（`PERMISSIONS` 由 Rust 的 `hammer_shared::permission` 產生）。 */
export type Permission = (typeof PERMISSIONS)[keyof typeof PERMISSIONS];

/** 目前登入的使用者；未登入時為 `undefined`。 */
export function useCurrentUser(): CurrentUser | undefined {
  const session = useSession();
  return session.data?.kind === 'authenticated' ? session.data.user : undefined;
}

/** 使用者是否有某個權限的判斷函式。 */
export function usePermissions(): (permission: Permission) => boolean {
  const user = useCurrentUser();
  return (permission) => user?.permissions.includes(permission) ?? false;
}

/**
 * 目前使用者是否有指定權限，用來顯示或隱藏介面。
 *
 * 只是輔助：權限一律由伺服器檢查（docs/architecture.md §6.2）。
 */
export function usePermission(permission: Permission): boolean {
  return usePermissions()(permission);
}
