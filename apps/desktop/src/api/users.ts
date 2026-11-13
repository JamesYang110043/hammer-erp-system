import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';

import { commands } from '../bindings';
import type { CreateUserRequest, ResetPasswordRequest, UpdateUserRequest, User } from '../bindings';
import { auditLogsKey } from './auditLogs';
import { unwrap } from './client';

/** 帳號管理（需要 `user.manage`，伺服器檢查）。 */
export const usersKey = ['users'] as const;
const rolesKey = ['roles'] as const;

export function useUsers({ enabled = true }: { enabled?: boolean } = {}) {
  return useQuery({
    queryKey: usersKey,
    queryFn: () => unwrap(commands.listUsers()),
    enabled,
  });
}

export function useRoles() {
  return useQuery({
    queryKey: rolesKey,
    queryFn: () => unwrap(commands.listRoles()),
    staleTime: Infinity,
  });
}

/** 修改帳號後重新取得帳號列表與操作紀錄。 */
function useUserMutation<TVars, TData>(mutationFn: (vars: TVars) => Promise<TData>) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn,
    onSuccess: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: usersKey }),
        queryClient.invalidateQueries({ queryKey: auditLogsKey }),
      ]),
  });
}

export function useCreateUser() {
  return useUserMutation((req: CreateUserRequest) => unwrap(commands.createUser(req)));
}

export function useUpdateUser() {
  return useUserMutation(({ id, req }: { id: string; req: UpdateUserRequest }) =>
    unwrap(commands.updateUser(id, req)),
  );
}

export function useResetUserPassword() {
  return useUserMutation(({ id, req }: { id: string; req: ResetPasswordRequest }) =>
    unwrap(commands.resetUserPassword(id, req)),
  );
}

export function useUnlockUser() {
  return useUserMutation((user: User) => unwrap(commands.unlockUser(user.id)));
}

export function useDeleteUser() {
  return useUserMutation((user: User) => unwrap(commands.deleteUser(user.id, user.version)));
}
