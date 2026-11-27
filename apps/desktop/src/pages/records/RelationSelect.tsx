import { Select, Spin } from 'antd';
import { useState } from 'react';
import type { CSSProperties } from 'react';

import { useLookupRecords, useRecord } from '../../api/records';
import { useEntities } from '../../api/schema';
import type { FieldDefinition, RelationRef } from '../../bindings';
import { relationLabel } from '../../records/display';
import { useDebouncedValue } from '../../utils/useDebouncedValue';
import { RelationTitle } from './RelationTitle';

/** 輸入關鍵字後等待多久才查詢（毫秒）。 */
export const LOOKUP_DEBOUNCE_MS = 300;

/**
 * 關聯欄位的選擇器：可搜尋的下拉選單，選項來自目標資料類別的 `lookup`（依標題比對）。
 *
 * - 打開選單時列出最新的資料；輸入關鍵字停頓 {@link LOOKUP_DEBOUNCE_MS} 毫秒後查詢
 * - 值為資料 id。目前的值顯示 `initial`（資料回應的 `relations`）的標題，已刪除時加上「（已刪除）」；
 *   沒有 `initial` 的值（例如版面儲存的篩選條件）以單筆查詢取得標題
 */
export function RelationSelect({
  field,
  value,
  onChange,
  initial,
  disabled,
  id,
  style,
  'aria-label': ariaLabel,
}: {
  field: FieldDefinition;
  value?: string;
  onChange?: (value: string | undefined) => void;
  /** 資料目前的關聯（`Record.relations`）。 */
  initial?: RelationRef;
  disabled?: boolean;
  id?: string;
  style?: CSSProperties;
  'aria-label'?: string;
}) {
  const entities = useEntities();
  const target = entities.data?.find((e) => e.id === field.relation_entity_id);
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState('');
  const q = useDebouncedValue(search.trim(), LOOKUP_DEBOUNCE_MS);
  const lookup = useLookupRecords(target?.key, q, open);
  // 選過的選項的標題，選單關閉、換了關鍵字後仍可顯示
  const [picked, setPicked] = useState<Map<string, string>>(() => new Map());

  const known = (id: string): string | undefined => {
    if (initial?.id === id) {
      return relationLabel(initial);
    }
    return picked.get(id) ?? lookup.data?.find((i) => i.id === id)?.title;
  };
  const currentLabel = value ? known(value) : undefined;
  // 不知道標題的值：查詢那一筆資料（找不到時視為已刪除）
  const resolved = useRecord(
    target?.key ?? '',
    value && !currentLabel && target ? value : undefined,
  );

  const valueLabel =
    currentLabel ??
    (resolved.data
      ? resolved.data.title
      : resolved.isError
        ? relationLabel({ title: '', deleted: true })
        : '載入中…');

  const options = (lookup.data ?? []).map((item) => ({ value: item.id, label: item.title }));
  if (value && !options.some((o) => o.value === value)) {
    options.unshift({ value, label: valueLabel });
  }

  if (entities.data && !target) {
    return (
      <Select
        id={id}
        aria-label={ariaLabel}
        disabled
        style={{ maxWidth: 320, ...style }}
        placeholder="找不到關聯的資料類別"
      />
    );
  }

  return (
    <Select
      id={id}
      aria-label={ariaLabel}
      disabled={disabled}
      style={{ maxWidth: 320, width: '100%', ...style }}
      showSearch
      allowClear
      placeholder={target ? `搜尋${target.label}` : undefined}
      filterOption={false}
      searchValue={search}
      onSearch={setSearch}
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) {
          setSearch('');
        }
      }}
      loading={lookup.isFetching}
      notFoundContent={lookup.isFetching ? <Spin size="small" /> : '找不到資料'}
      value={value || undefined}
      options={options}
      labelRender={(props) =>
        initial?.inactive && props.value === initial.id ? (
          <RelationTitle relation={initial} />
        ) : (
          props.label
        )
      }
      onChange={(next: string | undefined, option) => {
        if (next && option && !Array.isArray(option)) {
          const label = String(option.label ?? '');
          setPicked((prev) => new Map(prev).set(next, label));
        }
        onChange?.(next || undefined);
      }}
    />
  );
}
