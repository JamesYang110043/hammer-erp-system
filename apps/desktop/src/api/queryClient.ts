import { MutationCache, QueryCache, QueryClient } from '@tanstack/react-query';

import { handleAuthError } from '../auth/session';

/** 建立 QueryClient（main.tsx 與測試共用同一份設定）。 */
export function createQueryClient() {
  const queryClient: QueryClient = new QueryClient({
    // 任何 command 收到登入逾時、必須改密碼時，統一切換畫面
    queryCache: new QueryCache({ onError: (err) => handleAuthError(queryClient, err) }),
    mutationCache: new MutationCache({ onError: (err) => handleAuthError(queryClient, err) }),
    defaultOptions: {
      queries: {
        // 錯誤由各畫面自行顯示；連線失敗時不自動重試，避免畫面卡在載入中
        retry: false,
      },
      mutations: { retry: false },
    },
  });
  return queryClient;
}
