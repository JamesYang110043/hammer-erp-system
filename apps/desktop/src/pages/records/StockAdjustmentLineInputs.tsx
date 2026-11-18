import { Form, Input, Select, Typography } from 'antd';
import type { NamePath } from 'antd/es/form/interface';
import { useEffect, useRef } from 'react';

import { useProductLots } from '../../api/inventory';
import { MAX_PAGE_SIZE } from '../../api/records';
import type { FieldDefinition } from '../../bindings';
import { formatQty } from '../../records/documents';
import {
  LINE_FIELDS,
  findLot,
  lotOptionLabel,
  lotsIncludeEmpty,
  usesExistingLot,
} from '../../records/inventory';
import { RecordFieldInput } from './RecordFieldInput';

/**
 * 盤點調整明細（`stock_adjustment_line`，docs/architecture.md §8.10）在明細表格中的專屬輸入：
 *
 * - 批號：盤盈加回、盤虧從該商品的批號中選擇（{@link LotNoInput}）；盤盈新批號（含期初）自行輸入，可留空
 * - 單位成本：只有新批號可以輸入，其他方式由過帳時帶入批號的成本（{@link UnitCostInput}）
 * - 目前數量：選擇的批號目前的數量（{@link CurrentQty}）
 *
 * 商品或調整方式改變時，清空已不適用的批號、成本。
 */

/** 明細列中欄位的完整路徑。 */
function fieldPath(row: NamePath, fieldKey: string): (string | number)[] {
  return [...(Array.isArray(row) ? row : [row]), fieldKey];
}

/** 監看同一列的商品與調整方式。 */
function useLineContext(row: (string | number)[]) {
  const form = Form.useFormInstance();
  const productId = Form.useWatch(fieldPath(row, LINE_FIELDS.product), form) as
    string | null | undefined;
  const lineType = Form.useWatch(fieldPath(row, LINE_FIELDS.lineType), form) as unknown;
  const existing = usesExistingLot(lineType);
  const lots = useProductLots(existing && productId ? productId : undefined, {
    include_empty: lotsIncludeEmpty(lineType) ? 'true' : null,
    page_size: MAX_PAGE_SIZE,
  });
  return { productId: productId ?? undefined, lineType, existing, lots };
}

/**
 * 使用者改了同一列的商品或調整方式時呼叫 `onChange`。
 *
 * `Form.useWatch` 第一次繪製時可能是 `undefined`（欄位還沒註冊），從 `undefined` 變成有值不算修改，
 * 避免開啟既有資料時清掉已存檔的值。
 */
function useOnLineChanged(
  productId: string | undefined,
  lineType: unknown,
  onChange: (changed: { product: boolean; lotMode: boolean }) => void,
) {
  const prev = useRef({ productId, lineType });
  useEffect(() => {
    const before = prev.current;
    prev.current = { productId, lineType };
    const product = before.productId !== undefined && before.productId !== productId;
    const lotMode =
      before.lineType !== undefined &&
      usesExistingLot(before.lineType) !== usesExistingLot(lineType);
    if (product || lotMode) {
      onChange({ product, lotMode });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [productId, lineType]);
}

/** 批號：使用既有批號時為下拉選單（顯示剩餘數量與入庫日期），否則為文字輸入。 */
export function LotNoInput({
  row,
  label,
  disabled,
  value,
  onChange,
  id,
}: {
  row: (string | number)[];
  label: string;
  disabled?: boolean;
  value?: unknown;
  onChange?: (value: string | null) => void;
  id?: string;
}) {
  const { productId, lineType, existing, lots } = useLineContext(row);
  const text = typeof value === 'string' ? value : '';

  // 換商品或在「新批號 / 既有批號」之間切換時，原本的批號不再適用
  useOnLineChanged(productId, lineType, () => {
    if (!disabled && text !== '') {
      onChange?.(null);
    }
  });

  if (!existing) {
    return (
      <Input
        id={id}
        aria-label={label}
        disabled={disabled}
        value={text}
        placeholder="留空則過帳時自動產生"
        onChange={(e) => onChange?.(e.target.value)}
      />
    );
  }

  const items = lots.data?.items ?? [];
  const options = items.map((lot) => ({ value: lot.lot_no, label: lotOptionLabel(lot) }));
  if (text && !findLot(items, text)) {
    // 已存檔的批號不在清單中（例如已用完）時仍顯示原值
    options.unshift({ value: text, label: text });
  }
  return (
    <Select
      id={id}
      aria-label={label}
      showSearch
      allowClear
      disabled={disabled || !productId}
      loading={lots.isFetching}
      style={{ minWidth: 240 }}
      placeholder={productId ? '選擇批號' : '請先選擇商品'}
      value={text || undefined}
      options={options}
      optionFilterProp="label"
      notFoundContent={lots.isFetching ? '載入中…' : '此商品沒有可用的批號'}
      onChange={(v?: string) => onChange?.(v ?? null)}
    />
  );
}

/** 單位成本：只有新批號可以輸入；改成使用既有批號時清空。 */
export function UnitCostInput({
  row,
  field,
  disabled,
  value,
  onChange,
  id,
}: {
  row: (string | number)[];
  field: FieldDefinition;
  disabled?: boolean;
  value?: unknown;
  onChange?: (value: unknown) => void;
  id?: string;
}) {
  const { productId, lineType, existing } = useLineContext(row);
  useOnLineChanged(productId, lineType, ({ lotMode }) => {
    if (!disabled && lotMode && existing && value !== null && value !== undefined && value !== '') {
      onChange?.(null);
    }
  });
  return (
    <RecordFieldInput
      id={id}
      field={field}
      disabled={disabled || existing || undefined}
      value={value}
      onChange={onChange}
    />
  );
}

/** 目前數量：使用既有批號時為該批號目前的數量；新批號顯示「新批號」。 */
export function CurrentQty({ row }: { row: (string | number)[] }) {
  const form = Form.useFormInstance();
  const lotNo = Form.useWatch(fieldPath(row, LINE_FIELDS.lotNo), form) as unknown;
  const { productId, lineType, existing, lots } = useLineContext(row);
  if (!productId || lineType === undefined || lineType === null) {
    return <Typography.Text type="secondary">—</Typography.Text>;
  }
  if (!existing) {
    return <Typography.Text type="secondary">新批號</Typography.Text>;
  }
  const lot = findLot(lots.data?.items, lotNo);
  return lot ? (
    <Typography.Text strong>{formatQty(lot.qty_remaining)}</Typography.Text>
  ) : (
    <Typography.Text type="secondary">—</Typography.Text>
  );
}
