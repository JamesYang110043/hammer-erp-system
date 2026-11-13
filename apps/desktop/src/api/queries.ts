import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';

import { sessionKey } from '../auth/session';
import { commands } from '../bindings';
import { unwrap } from './client';

/** 狀態列多久檢查一次伺服器連線。 */
const HEALTH_POLL_MS = 10_000;

const keys = {
  health: ['health'] as const,
  serverUrl: ['serverUrl'] as const,
};

/** 伺服器連線狀態（定期輪詢）。 */
export function useHealth() {
  return useQuery({
    queryKey: keys.health,
    queryFn: () => unwrap(commands.healthCheck(null)),
    refetchInterval: HEALTH_POLL_MS,
    refetchIntervalInBackground: true,
  });
}

/** 已儲存的伺服器網址。 */
export function useServerUrl() {
  return useQuery({
    queryKey: keys.serverUrl,
    queryFn: () => commands.getServerUrl(),
    staleTime: Infinity,
  });
}

/** 測試指定網址的連線（不儲存）。 */
export function useTestConnection() {
  return useMutation({
    mutationFn: (serverUrl: string) => unwrap(commands.healthCheck(serverUrl)),
  });
}

/** 儲存伺服器網址，並重新檢查連線與登入狀態（token 以伺服器網址區分）。 */
export function useSaveServerUrl() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (serverUrl: string) => unwrap(commands.setServerUrl(serverUrl)),
    onSuccess: async (saved) => {
      queryClient.setQueryData(keys.serverUrl, saved);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: keys.health }),
        queryClient.invalidateQueries({ queryKey: sessionKey }),
      ]);
    },
  });
}
