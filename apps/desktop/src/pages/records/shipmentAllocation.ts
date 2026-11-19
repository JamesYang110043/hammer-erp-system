import { createContext, useMemo } from 'react';

import { CommandError } from '../../api/client';
import { useAllocationPreview } from '../../api/sales';
import { LINES_FORM_KEY } from '../../records/lines';
import type { LineFormValues } from '../../records/lines';
import {
  SALES_FIELDS as F,
  SHIPMENT_LINE,
  previewInputs,
  previewRowErrors,
  rowAllocations,
} from '../../records/sales';
import type { PreviewRowError, RowAllocation } from '../../records/sales';
import { useDebouncedValue } from '../../utils/useDebouncedValue';

type Rows = LineFormValues[];

/** 出貨明細的建議批號（{@link useShipmentAllocation} 的結果）。 */
export interface ShipmentAllocation {
  /** 表單明細位置 → 建議結果。 */
  allocations: Map<number, RowAllocation>;
  /** 建議批號 API 回傳的列錯誤（例如找不到批號）。 */
  errors: PreviewRowError[];
  /** 其他錯誤（連線失敗等）。 */
  failed?: string;
  loading: boolean;
  /** 試算的輸入與表單目前的值相同（沒有等待中的修改）。 */
  current: boolean;
}

const EMPTY: ShipmentAllocation = {
  allocations: new Map(),
  errors: [],
  loading: false,
  current: true,
};

export const ShipmentAllocationContext = createContext<ShipmentAllocation>(EMPTY);

/**
 * 出貨單草稿的建議批號：依表單目前的明細（商品、數量、批號）呼叫建議批號 API（輸入停止 300ms 後）。
 * `enabled` 為 `false` 時不查詢。
 */
export function useShipmentAllocation(
  values: Record<string, unknown>,
  enabled: boolean,
): ShipmentAllocation {
  const rows = enabled
    ? (((values[LINES_FORM_KEY] as Record<string, Rows> | undefined)?.[SHIPMENT_LINE] ??
        []) as Rows)
    : [];
  const inputs = previewInputs(
    rows.map((r) => ({ productId: r[F.product], qty: r[F.qty], lotNo: r[F.lotNo] })),
  );
  const key = JSON.stringify(inputs);
  const debouncedKey = useDebouncedValue(key, 300);
  const debounced = useMemo(() => JSON.parse(debouncedKey) as typeof inputs, [debouncedKey]);
  const preview = useAllocationPreview(debounced);
  return useMemo(() => {
    if (!enabled || debounced.length === 0) {
      return { ...EMPTY, current: key === debouncedKey };
    }
    const err = preview.error;
    const apiDetails =
      err instanceof CommandError && err.error.kind === 'api' ? err.error.details : undefined;
    const errors = err ? previewRowErrors(debounced, apiDetails) : [];
    return {
      allocations: preview.data && !err ? rowAllocations(debounced, preview.data) : new Map(),
      errors,
      failed: err && errors.length === 0 ? '無法取得建議批號' : undefined,
      loading: preview.isFetching,
      current: key === debouncedKey && !preview.isFetching,
    };
  }, [enabled, debounced, preview.data, preview.error, preview.isFetching, key, debouncedKey]);
}
