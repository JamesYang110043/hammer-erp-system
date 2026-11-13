import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import type { QueryClient } from '@tanstack/react-query';

import { apiErrorCode, isSessionExpired, unwrap } from '../api/client';
import { commands } from '../bindings';
import type {
  ChangePasswordRequest,
  CreateAdminRequest,
  CurrentUser,
  LoginRequest,
} from '../bindings';

/**
 * 登入狀態（docs/architecture.md §3.3、§6.2）。
 *
 * - `setup`：伺服器還沒有任何帳號，顯示「建立管理者」
 * - `login`：未登入；`notice` 是登入頁上方的提示（例如登入逾時）
 * - `authenticated`：已登入；`user.must_change_password` 為 true 時先顯示強制改密碼頁
 *
 * token 只存在 Rust 端（作業系統鑰匙圈），前端看不到。
 */
export type Session =
  | { kind: 'setup' }
  | { kind: 'login'; notice?: string }
  | { kind: 'authenticated'; user: CurrentUser };

export const SESSION_EXPIRED_NOTICE = '登入已逾時，請重新登入';

export const sessionKey = ['session'] as const;

/** 啟動流程：先看伺服器是否需要建立管理者，再看鑰匙圈的 token 是否有效。 */
async function loadSession(): Promise<Session> {
  const status = await unwrap(commands.setupStatus());
  if (status.needs_setup) {
    return { kind: 'setup' };
  }
  try {
    const user = await unwrap(commands.currentUser());
    return user ? { kind: 'authenticated', user } : { kind: 'login' };
  } catch (err) {
    if (isSessionExpired(err)) {
      return { kind: 'login', notice: SESSION_EXPIRED_NOTICE };
    }
    throw err;
  }
}

/** 目前的登入狀態。只在啟動、切換伺服器、登入登出時改變，不自動重新取得。 */
export function useSession() {
  return useQuery({
    queryKey: sessionKey,
    queryFn: loadSession,
    staleTime: Infinity,
  });
}

function setSession(queryClient: QueryClient, session: Session) {
  queryClient.setQueryData<Session>(sessionKey, session);
}

function setAuthenticated(queryClient: QueryClient, user: CurrentUser) {
  setSession(queryClient, { kind: 'authenticated', user });
}

function updateUser(queryClient: QueryClient, patch: Partial<CurrentUser>) {
  queryClient.setQueryData<Session>(sessionKey, (prev) =>
    prev?.kind === 'authenticated' ? { ...prev, user: { ...prev.user, ...patch } } : prev,
  );
}

/**
 * 所有 command 錯誤的共同處理（掛在 QueryCache / MutationCache 的 `onError`）。
 *
 * - 登入逾時（伺服器回 401，Rust 端已清除 token）→ 回到登入頁並提示
 * - `PASSWORD_CHANGE_REQUIRED` → 顯示強制改密碼頁
 */
export function handleAuthError(queryClient: QueryClient, err: unknown) {
  if (isSessionExpired(err)) {
    setSession(queryClient, { kind: 'login', notice: SESSION_EXPIRED_NOTICE });
  } else if (apiErrorCode(err) === 'PASSWORD_CHANGE_REQUIRED') {
    updateUser(queryClient, { must_change_password: true });
  }
}

export function useLogin() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (req: LoginRequest) => unwrap(commands.login(req)),
    onSuccess: (user) => setAuthenticated(queryClient, user),
  });
}

export function useSetupAdmin() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (req: CreateAdminRequest) => unwrap(commands.setupAdmin(req)),
    onSuccess: (user) => setAuthenticated(queryClient, user),
  });
}

export function useLogout() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => unwrap(commands.logout()),
    onSuccess: () => setSession(queryClient, { kind: 'login' }),
  });
}

export function useChangePassword() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (req: ChangePasswordRequest) => unwrap(commands.changePassword(req)),
    onSuccess: () => updateUser(queryClient, { must_change_password: false }),
  });
}
