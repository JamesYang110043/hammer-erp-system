import { useQuery } from '@tanstack/react-query';

import { commands } from '../bindings';
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
