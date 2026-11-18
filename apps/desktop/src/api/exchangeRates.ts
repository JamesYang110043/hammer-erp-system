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

/** 適用匯率的查詢鍵（放在 {@link exchangeRatesKey} 底下：修改匯率後一起重新取得）。 */
export const effectiveRateKey = (currencyCode: string, date: string) =>
  [...exchangeRatesKey, 'effective', currencyCode, date] as const;

/** 適用匯率的查詢設定（`useQuery` 與 `queryClient.fetchQuery` 共用）。 */
export function effectiveRateQuery(currencyCode: string, date: string) {
  return {
    queryKey: effectiveRateKey(currencyCode, date),
    queryFn: () => unwrap(commands.effectiveExchangeRate({ currency_code: currencyCode, date })),
    staleTime: 60_000,
  };
}

/**
 * 幣別在單據日期（`YYYY-MM-DD`）的適用匯率（docs/architecture.md §8.8「幣別與匯率」）。
 * 幣別或日期沒有值時不查詢。
 */
export function useEffectiveRate(currencyCode: string | undefined, date: string | undefined) {
  return useQuery({
    ...effectiveRateQuery(currencyCode ?? '', date ?? ''),
    enabled: Boolean(currencyCode && date),
  });
}
