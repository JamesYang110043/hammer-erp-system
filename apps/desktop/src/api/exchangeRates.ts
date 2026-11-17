import { keepPreviousData, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';

import { commands } from '../bindings';
import type {
  CreateExchangeRateRequest,
  ExchangeRate,
  ExchangeRateQuery,
  UpdateExchangeRateRequest,
} from '../bindings';
import { auditLogsKey } from './auditLogs';
import { unwrap } from './client';

/** 匯率（查看需要 `record.read`，新增、修改、刪除需要 `record.write`，伺服器檢查）。 */
export const exchangeRatesKey = ['exchangeRates'] as const;

export function useExchangeRates(query: ExchangeRateQuery) {
  return useQuery({
    queryKey: [...exchangeRatesKey, query],
    queryFn: () => unwrap(commands.listExchangeRates(query)),
    // 換頁時保留上一頁資料，避免表格閃爍
    placeholderData: keepPreviousData,
  });
}

/** 修改匯率後重新取得匯率列表與操作紀錄。 */
function useRateMutation<TVars, TData>(mutationFn: (vars: TVars) => Promise<TData>) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn,
    onSuccess: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: exchangeRatesKey }),
        queryClient.invalidateQueries({ queryKey: auditLogsKey }),
      ]),
  });
}

export function useCreateExchangeRate() {
  return useRateMutation((req: CreateExchangeRateRequest) =>
    unwrap(commands.createExchangeRate(req)),
  );
}

export function useUpdateExchangeRate() {
  return useRateMutation(({ id, req }: { id: string; req: UpdateExchangeRateRequest }) =>
    unwrap(commands.updateExchangeRate(id, req)),
  );
}

export function useDeleteExchangeRate() {
  return useRateMutation((rate: ExchangeRate) =>
    unwrap(commands.deleteExchangeRate(rate.id, rate.version)),
  );
}
