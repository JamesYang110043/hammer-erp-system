import { keepPreviousData, useQuery } from '@tanstack/react-query';

import { commands } from '../bindings';
import type {
  LotQuery_Deserialize as LotQuery,
  StockBalanceQuery_Deserialize as StockBalanceQuery,
  TransactionQuery_Deserialize as TransactionQuery,
} from '../bindings';
import { unwrap } from './client';

/**
 * 庫存查詢（docs/architecture.md §8.10「庫存查詢 API」，需要 `record.read`，伺服器檢查）。
 *
 * 單據過帳、作廢後重新取得（`useDocumentAction`）。
 */
export const inventoryKey = ['inventory'] as const;

/** 商品庫存。 */
export function useStockBalances(query: StockBalanceQuery) {
  return useQuery({
    queryKey: [...inventoryKey, 'balances', query],
    queryFn: () => unwrap(commands.listStockBalances(query)),
    placeholderData: keepPreviousData,
  });
}

/** 商品的批號（先進先出順序）。`productId` 為 `undefined` 時不查詢。 */
export function useProductLots(productId: string | undefined, query: LotQuery) {
  return useQuery({
    queryKey: [...inventoryKey, 'lots', productId ?? '', query],
    queryFn: () => unwrap(commands.listProductLots(productId!, query)),
    enabled: productId !== undefined && productId !== '',
    placeholderData: keepPreviousData,
  });
}

/** 庫存異動流水帳（新到舊）。 */
export function useInventoryTransactions(query: TransactionQuery, enabled = true) {
  return useQuery({
    queryKey: [...inventoryKey, 'transactions', query],
    queryFn: () => unwrap(commands.listInventoryTransactions(query)),
    enabled,
    placeholderData: keepPreviousData,
  });
}
