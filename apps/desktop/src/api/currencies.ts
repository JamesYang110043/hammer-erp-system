import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';

import { commands } from '../bindings';
import type { CreateCurrencyRequest, UpdateCurrencyRequest } from '../bindings';
import { auditLogsKey } from './auditLogs';
import { unwrap } from './client';

/** 幣別（需要登入）。幣別很少變動，選單共用同一份快取。 */
export const currenciesKey = ['currencies'] as const;

/** 啟用中的幣別（幣別選單用），本位幣優先、再依代碼排序。 */
export function useActiveCurrencies() {
  return useQuery({
    queryKey: [...currenciesKey, 'active'],
    queryFn: () => unwrap(commands.listCurrencies(false)),
    staleTime: 5 * 60_000,
  });
}

/** 所有幣別的查詢設定（`useQuery` 與 `queryClient.fetchQuery` 共用）。 */
export const allCurrenciesQuery = {
  queryKey: [...currenciesKey, 'all'],
  queryFn: () => unwrap(commands.listCurrencies(true)),
};

/** 所有幣別，含已停用的（幣別管理、匯率列表、單據的金額小數位數用）。 */
export function useAllCurrencies(enabled = true) {
  return useQuery({ ...allCurrenciesQuery, enabled });
}

/** 修改幣別後重新取得所有幣別列表（含選單）與操作紀錄。 */
function useCurrencyMutation<TVars, TData>(mutationFn: (vars: TVars) => Promise<TData>) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn,
    onSuccess: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: currenciesKey }),
        queryClient.invalidateQueries({ queryKey: auditLogsKey }),
      ]),
  });
}

/** 新增幣別（需要 `settings.manage`，伺服器檢查）。 */
export function useCreateCurrency() {
  return useCurrencyMutation((req: CreateCurrencyRequest) => unwrap(commands.createCurrency(req)));
}

/** 修改、停用、重新啟用幣別（需要 `settings.manage`，伺服器檢查）。 */
export function useUpdateCurrency() {
  return useCurrencyMutation(({ code, req }: { code: string; req: UpdateCurrencyRequest }) =>
    unwrap(commands.updateCurrency(code, req)),
  );
}

/** 稅別（需要登入）：由 migration 寫入，不提供修改。 */
export function useTaxRates(enabled = true) {
  return useQuery({
    queryKey: ['taxRates'],
    queryFn: () => unwrap(commands.listTaxRates()),
    staleTime: Infinity,
    enabled,
  });
}
