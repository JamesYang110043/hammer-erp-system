import { Select } from 'antd';
import { useMemo } from 'react';
import type { CSSProperties } from 'react';

import { useAllRecords } from '../../api/records';
import type { RelationRef } from '../../bindings';
import { relationLabel } from '../../records/display';
import { CATEGORY_ENTITY_KEY, categoryTree } from '../../records/masterData';

/**
 * 指向商品分類的關聯欄位（商品的分類、分類的上層分類）：選項顯示階層路徑，例如「電子 / 手機」，
 * 依樹狀順序排列，輸入關鍵字比對整條路徑。
 *
 * 分類筆數不多，一次取得全部分類在前端組成路徑。值為分類 id；目前的值已刪除時顯示
 * `initial` 的標題並加上「（已刪除）」。不可指向自己或下層分類的限制由伺服器檢查（§7.2）。
 */
export function CategorySelect({
  value,
  onChange,
  initial,
  disabled,
  id,
  style,
  'aria-label': ariaLabel,
}: {
  value?: string;
  onChange?: (value: string | undefined) => void;
  /** 資料目前的關聯（`Record.relations`）。 */
  initial?: RelationRef;
  disabled?: boolean;
  id?: string;
  style?: CSSProperties;
  'aria-label'?: string;
}) {
  const categories = useAllRecords(CATEGORY_ENTITY_KEY);
  const tree = useMemo(
    () => categoryTree(categories.data ?? []).map((node) => ({ value: node.id, label: node.path })),
    [categories.data],
  );
  const options = [...tree];
  if (value && !tree.some((o) => o.value === value)) {
    // 分類還沒載入時先顯示資料回應的標題；載入後仍找不到代表已刪除
    options.unshift({
      value,
      label:
        initial?.id === value
          ? relationLabel(categories.data ? { ...initial, deleted: true } : initial)
          : categories.data
            ? relationLabel({ title: '', deleted: true })
            : '載入中…',
    });
  }

  return (
    <Select
      id={id}
      aria-label={ariaLabel}
      disabled={disabled}
      style={{ maxWidth: 480, width: '100%', ...style }}
      showSearch
      allowClear
      optionFilterProp="label"
      placeholder="搜尋分類"
      loading={categories.isPending}
      status={categories.isError ? 'error' : undefined}
      notFoundContent={categories.isError ? '無法載入分類' : '找不到分類'}
      value={value || undefined}
      options={options}
      onChange={(v: string | undefined) => onChange?.(v || undefined)}
    />
  );
}
