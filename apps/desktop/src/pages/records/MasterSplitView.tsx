import { PlusOutlined, ReloadOutlined, SearchOutlined } from '@ant-design/icons';
import { Alert, Button, Card, Input, Segmented, Spin, Typography, theme } from 'antd';
import { useCallback, useEffect, useRef, useState } from 'react';
import type { KeyboardEvent } from 'react';
import { useNavigate, useSearchParams } from 'react-router';

import { errorMessage } from '../../api/client';
import { useMasterPicker } from '../../api/records';
import { usePermission } from '../../auth/permission';
import { PERMISSIONS } from '../../bindings';
import type { EntityDefinition, FieldDefinition, Record as DataRecord } from '../../bindings';
import { EmptyState } from '../../components/EmptyState';
import { PageHeader } from '../../components/PageHeader';
import { StatusTag } from '../../components/StatusTag';
import { newRecordPath } from '../../modules';
import {
  NAME_FIELD,
  PICKER_PAGE_SIZE,
  PICKER_SEARCH_DELAY_MS,
  SELECTED_PARAM,
  TAX_ID_FIELD,
  adjacentId,
  hasActiveField,
  isInactive,
  recordCode,
  searchFieldKeys,
} from '../../records/masterSplit';
import { useDebouncedValue } from '../../utils/useDebouncedValue';
import { entityIcon } from '../schema/entityIcons';

/** 左側列表的寬度。 */
export const PICKER_WIDTH = 320;

/** 捲動到距離底部多少 px 內時載入下一頁。 */
const LOAD_MORE_THRESHOLD = 48;

type ActiveFilter = 'active' | 'all';

const optionId = (id: string) => `master-picker-option-${id}`;

function text(record: DataRecord, key: string): string {
  const v = record.data[key];
  return typeof v === 'string' ? v : '';
}

/** 左側列表的一筆：第一行「編號＋名稱」，第二行統編；停用的資料為灰色。 */
function PickerOption({
  record,
  selected,
  onSelect,
}: {
  record: DataRecord;
  selected: boolean;
  onSelect: (id: string) => void;
}) {
  const { token } = theme.useToken();
  const inactive = isInactive(record);
  const taxId = text(record, TAX_ID_FIELD);
  return (
    <div
      id={optionId(record.id)}
      role="option"
      aria-selected={selected}
      data-inactive={inactive || undefined}
      onClick={() => onSelect(record.id)}
      className="master-picker-option"
      style={{
        padding: `${token.paddingXS}px ${token.paddingSM}px`,
        borderRadius: token.borderRadius,
        cursor: 'pointer',
        background: selected ? token.colorPrimaryBg : undefined,
        // 停用的資料以次要文字色（灰色）顯示，仍維持 4.5:1 對比
        color: inactive ? token.colorTextSecondary : token.colorText,
        // 選取的那筆左側加主色色條
        boxShadow: selected ? `inset 3px 0 0 ${token.colorPrimary}` : undefined,
      }}
    >
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, minWidth: 0 }}>
        <span style={{ flexShrink: 0, fontWeight: selected ? 600 : undefined }}>
          {recordCode(record)}
        </span>
        <span
          style={{
            overflow: 'hidden',
            textOverflow: 'ellipsis',
            whiteSpace: 'nowrap',
            fontWeight: selected ? 600 : undefined,
          }}
        >
          {text(record, NAME_FIELD)}
        </span>
        {inactive && (
          <StatusTag tone="neutral" style={{ marginInlineEnd: 0, flexShrink: 0 }}>
            停用
          </StatusTag>
        )}
      </div>
      <div
        style={{
          fontSize: token.fontSizeSM,
          color: token.colorTextSecondary,
          minHeight: token.fontSizeSM * 1.5,
        }}
      >
        {taxId && `統編 ${taxId}`}
      </div>
    </div>
  );
}

/**
 * 分割檢視（docs/architecture.md §8.7）：左側列表選擇器，右側選取資料的內容。
 *
 * 只用於 {@link usesSplitView} 的資料類別（客戶、廠商）；選取的資料存在網址 `?selected=<id>`。
 */
export function MasterSplitView({
  entity,
  fields,
}: {
  entity: EntityDefinition;
  fields: FieldDefinition[];
}) {
  const entityKey = entity.key;
  const navigate = useNavigate();
  const { token } = theme.useToken();
  const canWrite = usePermission(PERMISSIONS.RECORD_WRITE);
  const [searchParams, setSearchParams] = useSearchParams();
  const selectedId = searchParams.get(SELECTED_PARAM);
  // 連續按上下鍵時，網址還沒更新前就要以剛選的那筆為準
  const latestSelected = useRef(selectedId);
  useEffect(() => {
    latestSelected.current = selectedId;
  }, [selectedId]);

  const [search, setSearch] = useState('');
  const debouncedSearch = useDebouncedValue(search, PICKER_SEARCH_DELAY_MS);
  const showActiveFilter = hasActiveField(fields);
  const [activeFilter, setActiveFilter] = useState<ActiveFilter>('active');

  const picker = useMasterPicker(
    entityKey,
    searchFieldKeys(fields),
    debouncedSearch,
    showActiveFilter && activeFilter === 'active',
    PICKER_PAGE_SIZE,
  );
  const items = picker.data?.pages.flatMap((p) => p.items) ?? [];
  const total = picker.data?.pages[0]?.total ?? 0;

  const listRef = useRef<HTMLDivElement>(null);
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = picker;

  const select = useCallback(
    (id: string) => {
      latestSelected.current = id;
      setSearchParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          next.set(SELECTED_PARAM, id);
          return next;
        },
        { replace: true },
      );
    },
    [setSearchParams],
  );

  /** 捲動到接近底部（或列表還沒填滿可視範圍）時載入下一頁。 */
  const loadMoreIfNeeded = useCallback(() => {
    const el = listRef.current;
    if (!el || !hasNextPage || isFetchingNextPage || el.clientHeight === 0) {
      return;
    }
    if (el.scrollTop + el.clientHeight >= el.scrollHeight - LOAD_MORE_THRESHOLD) {
      void fetchNextPage();
    }
  }, [hasNextPage, isFetchingNextPage, fetchNextPage]);

  useEffect(loadMoreIfNeeded, [loadMoreIfNeeded, items.length]);

  // 上下鍵切換後，讓選取的那筆保持在可視範圍
  useEffect(() => {
    if (selectedId) {
      document.getElementById(optionId(selectedId))?.scrollIntoView?.({ block: 'nearest' });
    }
  }, [selectedId]);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') {
      return;
    }
    e.preventDefault();
    const current = latestSelected.current;
    const next = adjacentId(items, current, e.key === 'ArrowDown' ? 1 : -1);
    if (next && next !== current) {
      select(next);
    }
  };

  const selected = items.find((r) => r.id === selectedId);

  return (
    <div className="page master-split-page">
      <PageHeader
        icon={entityIcon(entity.icon)}
        title={entity.label}
        actions={
          <>
            <Button
              icon={<ReloadOutlined />}
              onClick={() => picker.refetch()}
              loading={picker.isFetching && !isFetchingNextPage}
            >
              重新整理
            </Button>
            {canWrite && (
              <Button
                type="primary"
                icon={<PlusOutlined />}
                onClick={() => navigate(newRecordPath(entityKey))}
              >
                新增
              </Button>
            )}
          </>
        }
      />
      <div className="master-split">
        <Card
          className="master-split-picker"
          style={{ width: PICKER_WIDTH }}
          styles={{ body: { padding: 0 } }}
        >
          <div
            style={{
              padding: token.paddingSM,
              display: 'flex',
              flexDirection: 'column',
              gap: token.paddingXS,
              borderBottom: `1px solid ${token.colorSplit}`,
            }}
          >
            <Input
              allowClear
              prefix={<SearchOutlined style={{ color: token.colorTextQuaternary }} />}
              placeholder="搜尋編號、名稱、簡稱、統編"
              aria-label={`搜尋${entity.label}`}
              value={search}
              onChange={(e) => setSearch(e.target.value)}
            />
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
              {showActiveFilter ? (
                <Segmented<ActiveFilter>
                  size="small"
                  aria-label="啟用篩選"
                  value={activeFilter}
                  onChange={setActiveFilter}
                  options={[
                    { value: 'active', label: '啟用中' },
                    { value: 'all', label: '全部' },
                  ]}
                />
              ) : (
                <span />
              )}
              <Typography.Text type="secondary" data-testid="picker-total">
                共 {total} 筆
              </Typography.Text>
            </div>
          </div>
          {picker.isError && (
            <Alert
              showIcon
              type="error"
              message={errorMessage(picker.error)}
              style={{ margin: token.paddingSM }}
            />
          )}
          <div
            ref={listRef}
            role="listbox"
            tabIndex={0}
            aria-label={`${entity.label}列表`}
            aria-activedescendant={selected ? optionId(selected.id) : undefined}
            onKeyDown={onKeyDown}
            onScroll={loadMoreIfNeeded}
            className="master-picker-list"
            style={{ padding: token.paddingXXS }}
          >
            {items.map((record) => (
              <PickerOption
                key={record.id}
                record={record}
                selected={record.id === selectedId}
                onSelect={select}
              />
            ))}
            {picker.isPending && (
              <div style={{ padding: token.padding, textAlign: 'center' }}>
                <Spin size="small" />
              </div>
            )}
            {picker.isSuccess && items.length === 0 && (
              <EmptyState
                compact
                kind={debouncedSearch.trim() ? 'notFound' : 'empty'}
                title={debouncedSearch.trim() ? '找不到符合的資料' : undefined}
              />
            )}
            {isFetchingNextPage && (
              <div style={{ padding: token.paddingXS, textAlign: 'center' }} aria-label="載入更多">
                <Spin size="small" />
              </div>
            )}
          </div>
        </Card>
        <Card className="master-split-detail">
          {selected ? (
            <Typography.Title level={5} style={{ margin: 0 }} data-testid="detail-title">
              {recordCode(selected)} {text(selected, NAME_FIELD)}
            </Typography.Title>
          ) : (
            <EmptyState kind="empty" title={`請從左側選擇${entity.label}`} />
          )}
        </Card>
      </div>
    </div>
  );
}
