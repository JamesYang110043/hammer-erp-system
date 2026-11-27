import {
  DeleteOutlined,
  PlusOutlined,
  ReloadOutlined,
  SaveOutlined,
  SettingOutlined,
} from '@ant-design/icons';
import { Alert, App, Button, Popconfirm, Select, Space, Table } from 'antd';
import type { TableColumnsType, TableProps } from 'antd';
import { useState } from 'react';
import type { ReactNode } from 'react';
import { useNavigate, useParams, useSearchParams } from 'react-router';

import { StatusTag } from '../../components/StatusTag';
import { EmptyState } from '../../components/EmptyState';
import { Page } from '../../components/PageHeader';
import { errorMessage } from '../../api/client';
import { mutationErrorMessage } from '../../api/errors';
import {
  useDefaultLayout,
  useDeleteLayout,
  useDeleteRecord,
  useLayouts,
  useRecords,
  useUpdateLayout,
} from '../../api/records';
import { useEntity } from '../../api/schema';
import { useCurrentUser, usePermission } from '../../auth/permission';
import { PERMISSIONS } from '../../bindings';
import { newRecordPath, recordPath } from '../../modules';
import type {
  DefaultViewLayout_Serialize as DefaultViewLayout,
  EntityDefinition,
  FieldDefinition,
  Record as DataRecord,
  RecordFilter,
} from '../../bindings';
import { generatedConfig, resolveColumns } from '../../records/columns';
import type { LayoutColumn, LayoutConfig } from '../../records/columns';
import { isNumericType, valueField } from '../../records/display';
import {
  DOCUMENT_STATUS_LABELS,
  STATUS_FIELD,
  documentStatus,
  documentStatuses,
  isEditableDocument,
  isSystemDocument,
} from '../../records/documents';
import { RECORD_NO, showsRecordNo } from '../../records/lines';
import { GENERATED_LAYOUT_ID, canEditLayout, sameConfig } from '../../records/layouts';
import type { ViewLayout } from '../../records/layouts';
import {
  buildRecordListQuery,
  columnSortOrder,
  isQueryable,
  isSortable,
  sortFromTable,
} from '../../records/query';
import { entityIcon } from '../schema/entityIcons';
import {
  MODE_PARAM,
  SELECTED_PARAM,
  loadRecordView,
  saveRecordView,
  usesSplitView,
} from '../../records/masterSplit';
import type { RecordView, SplitMode } from '../../records/masterSplit';
import { ColumnSettingsModal } from './ColumnSettingsModal';
import { FieldValue } from './FieldValue';
import { FilterPanel } from './FilterPanel';
import { MasterSplitView } from './MasterSplitView';
import { CustomerProductNamesImport } from './CustomerProductNamesImport';
import { RecordViewToggle } from './RecordViewToggle';
import { DocumentStatusTag } from './DocumentStatusTag';
import { SaveLayoutModal } from './SaveLayoutModal';

const DEFAULT_PAGE_SIZE = 50;

/** 狀態快速篩選的「全部」。 */
const ALL_STATUSES = '';

/** 篩選條件中的狀態（`status` 等於某值）。 */
function isStatusFilter(f: RecordFilter): boolean {
  return f.field_key === STATUS_FIELD && f.op === 'eq';
}

function layoutLabel(layout: ViewLayout): string {
  return layout.user_id === null ? `${layout.name}（全域）` : layout.name;
}

/** 一個資料類別的列表。`initial` 為開啟時套用的預設版面。 */
function RecordList({
  entity,
  fields,
  initial,
  split,
}: {
  entity: EntityDefinition;
  fields: FieldDefinition[];
  initial: DefaultViewLayout;
  /** 可以切換到分割檢視的資料類別（§8.7）：點列、新增都在分割檢視中進行。 */
  split?: { open: (id: string) => void; create: () => void; switchToSplit: () => void };
}) {
  const entityKey = entity.key;
  const navigate = useNavigate();
  const { message } = App.useApp();
  const user = useCurrentUser();
  const canWrite = usePermission(PERMISSIONS.RECORD_WRITE);
  const canManageSchema = usePermission(PERMISSIONS.SCHEMA_MANAGE);
  const isDocument = isSystemDocument(entity);

  const [layoutId, setLayoutId] = useState(initial.layout?.id ?? GENERATED_LAYOUT_ID);
  const [config, setConfig] = useState<LayoutConfig>(initial.config);
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(DEFAULT_PAGE_SIZE);
  const [columnSettingsOpen, setColumnSettingsOpen] = useState(false);
  const [saveAsOpen, setSaveAsOpen] = useState(false);

  const layouts = useLayouts(entityKey);
  const records = useRecords(entityKey, buildRecordListQuery(config, page, pageSize));
  const updateLayout = useUpdateLayout(entityKey);
  const deleteLayout = useDeleteLayout(entityKey);
  const deleteRecord = useDeleteRecord(entityKey);

  const currentLayout =
    layouts.data?.find((l) => l.id === layoutId) ??
    (initial.layout?.id === layoutId ? initial.layout : undefined);
  const savedConfig = currentLayout?.config ?? generatedConfig(fields);
  const dirty = !sameConfig(config, savedConfig);
  const canEditCurrent = currentLayout
    ? canEditLayout(currentLayout, user?.id, canManageSchema)
    : false;

  const changeConfig = (patch: Partial<LayoutConfig>) => {
    setConfig((prev) => ({ ...prev, ...patch }));
    setPage(1);
  };

  const applyLayout = (id: string, layout?: ViewLayout) => {
    setLayoutId(id);
    setConfig(layout?.config ?? generatedConfig(fields));
    setPage(1);
  };

  const switchLayout = (id: string) =>
    applyLayout(
      id,
      layouts.data?.find((l) => l.id === id),
    );

  const saveCurrent = () => {
    if (!currentLayout) {
      return;
    }
    updateLayout.mutate(
      { id: currentLayout.id, req: { version: currentLayout.version, config } },
      {
        onSuccess: () => void message.success(`已儲存版面「${currentLayout.name}」`),
        onError: (err) => void message.error(mutationErrorMessage(err)),
      },
    );
  };

  const removeCurrent = () => {
    if (!currentLayout) {
      return Promise.resolve();
    }
    return deleteLayout.mutateAsync(currentLayout).then(
      () => {
        message.success(`已刪除版面「${currentLayout.name}」`);
        applyLayout(GENERATED_LAYOUT_ID);
      },
      (err: unknown) => void message.error(mutationErrorMessage(err)),
    );
  };

  const columns: TableColumnsType<DataRecord> = resolveColumns(fields, config).map(
    ({ field, width }) => ({
      key: field.field_key,
      title: field.label,
      dataIndex: ['data', field.field_key],
      width,
      align: isNumericType(field) ? 'right' : undefined,
      ellipsis: valueField(field).data_type === 'long_text',
      sorter: isSortable(field),
      sortOrder: columnSortOrder(config.sort, field.field_key),
      render: (value: unknown, record) => {
        // 系統單據的狀態以固定的顏色標籤顯示
        const status = isDocument && field.field_key === STATUS_FIELD && documentStatus(record);
        return status ? (
          <DocumentStatusTag status={status} entityKey={entityKey} />
        ) : (
          <FieldValue field={field} value={value} record={record} />
        );
      },
    }),
  );
  // 單號不是欄位，不在版面的顯示欄位中；有單號規則時固定顯示在第一欄，可以排序
  if (showsRecordNo(entity)) {
    columns.unshift({
      key: RECORD_NO,
      title: '單號',
      dataIndex: RECORD_NO,
      sorter: true,
      sortOrder: columnSortOrder(config.sort, RECORD_NO),
      render: (value: string | null) => value ?? '',
    });
  }
  if (canWrite) {
    columns.push({
      title: '操作',
      key: '__actions',
      width: 80,
      fixed: 'right',
      // 點列會開啟資料；操作欄（含確認視窗）的點擊不要傳到列
      onCell: () => ({ onClick: (e) => e.stopPropagation() }),
      // 系統單據只有草稿可以刪除
      render: (_, record) =>
        isEditableDocument(entity, record) && (
          <Popconfirm
            title="確定要刪除這筆資料？"
            okText="刪除"
            okButtonProps={{ danger: true }}
            cancelText="取消"
            onConfirm={() =>
              deleteRecord.mutateAsync(record).then(
                () => void message.success('已刪除'),
                (err: unknown) => void message.error(mutationErrorMessage(err)),
              )
            }
          >
            <Button type="link" size="small" danger>
              刪除
            </Button>
          </Popconfirm>
        ),
    });
  }

  const statusFilter = config.filters.find(isStatusFilter)?.value ?? ALL_STATUSES;
  const changeStatusFilter = (status: string) =>
    changeConfig({
      filters: [
        ...config.filters.filter((f) => !isStatusFilter(f)),
        ...(status === ALL_STATUSES
          ? []
          : [{ field_key: STATUS_FIELD, op: 'eq' as const, value: status }]),
      ],
    });

  const onTableChange: TableProps<DataRecord>['onChange'] = (
    _pagination,
    _filters,
    sorter,
    extra,
  ) => {
    if (extra.action !== 'sort') {
      return;
    }
    const s = Array.isArray(sorter) ? sorter[0] : sorter;
    changeConfig({ sort: sortFromTable(s?.columnKey as string | undefined, s?.order) });
  };

  return (
    <Page
      icon={entityIcon(entity.icon)}
      title={entity.label}
      actions={
        <Space>
          {split && <RecordViewToggle value="table" onChange={split.switchToSplit} />}
          <Button
            icon={<ReloadOutlined />}
            onClick={() => records.refetch()}
            loading={records.isFetching}
          >
            重新整理
          </Button>
          {canWrite && entityKey === 'customer' && <CustomerProductNamesImport />}
          {canWrite && (
            <Button
              type="primary"
              icon={<PlusOutlined />}
              onClick={() => (split ? split.create() : navigate(newRecordPath(entityKey)))}
            >
              新增
            </Button>
          )}
        </Space>
      }
    >
      <Space wrap style={{ marginBottom: 16 }}>
        {isDocument && (
          <Select
            aria-label="狀態篩選"
            style={{ width: 140 }}
            value={statusFilter}
            onChange={changeStatusFilter}
            options={[
              { value: ALL_STATUSES, label: '全部狀態' },
              ...documentStatuses(entityKey).map((st) => ({
                value: st,
                label: DOCUMENT_STATUS_LABELS[st],
              })),
            ]}
          />
        )}
        <Select
          aria-label="版面"
          style={{ width: 220 }}
          value={layoutId}
          onChange={switchLayout}
          loading={layouts.isPending}
          options={[
            { value: GENERATED_LAYOUT_ID, label: '依欄位定義' },
            ...(layouts.data ?? []).map((l) => ({ value: l.id, label: layoutLabel(l) })),
          ]}
        />
        {dirty && <StatusTag tone="warning">未儲存的變更</StatusTag>}
        {canEditCurrent && (
          <Button
            icon={<SaveOutlined />}
            disabled={!dirty}
            loading={updateLayout.isPending}
            onClick={saveCurrent}
          >
            儲存版面
          </Button>
        )}
        <Button onClick={() => setSaveAsOpen(true)}>另存新版面</Button>
        {canEditCurrent && (
          <Popconfirm
            title={`確定要刪除版面「${currentLayout?.name}」？`}
            okText="刪除"
            okButtonProps={{ danger: true }}
            cancelText="取消"
            onConfirm={removeCurrent}
          >
            <Button icon={<DeleteOutlined />}>刪除版面</Button>
          </Popconfirm>
        )}
        <Button icon={<SettingOutlined />} onClick={() => setColumnSettingsOpen(true)}>
          欄位設定
        </Button>
      </Space>
      <FilterPanel
        key={`${layoutId}:${JSON.stringify(config.filters)}`}
        fields={fields
          .filter(isQueryable)
          .map(valueField)
          .sort((a, b) => a.sort_order - b.sort_order)}
        filters={config.filters}
        onApply={(filters) => changeConfig({ filters })}
      />
      {records.isError && (
        <Alert
          showIcon
          type="error"
          message={errorMessage(records.error)}
          style={{ marginBottom: 16 }}
        />
      )}
      <Table<DataRecord>
        rowKey="id"
        size="small"
        columns={columns}
        dataSource={records.data?.items}
        loading={records.isFetching}
        scroll={{ x: 'max-content' }}
        onChange={onTableChange}
        onRow={(record) => ({
          onClick: () =>
            split ? split.open(record.id) : navigate(recordPath(entityKey, record.id)),
          style: { cursor: 'pointer' },
        })}
        pagination={{
          current: records.data?.page ?? page,
          pageSize: records.data?.page_size ?? pageSize,
          total: records.data?.total ?? 0,
          showSizeChanger: true,
          pageSizeOptions: [20, 50, 100, 200],
          showTotal: (total) => `共 ${total} 筆`,
          onChange: (nextPage, nextSize) => {
            setPage(nextSize !== pageSize ? 1 : nextPage);
            setPageSize(nextSize);
          },
        }}
      />
      <ColumnSettingsModal
        open={columnSettingsOpen}
        fields={fields}
        config={config}
        onClose={() => setColumnSettingsOpen(false)}
        onSubmit={(next: LayoutColumn[]) => setConfig((prev) => ({ ...prev, columns: next }))}
      />
      <SaveLayoutModal
        entityKey={entityKey}
        open={saveAsOpen}
        config={config}
        canSaveGlobal={canManageSchema}
        onClose={() => setSaveAsOpen(false)}
        onSaved={(layout) => {
          setLayoutId(layout.id);
          message.success(`已儲存版面「${layout.name}」`);
        }}
      />
    </Page>
  );
}

/**
 * 資料類別的列表（自訂與系統資料類別共用，需要 `record.read`），路徑 `/entities/:key`。
 *
 * 欄位依版面設定（沒有版面時依欄位定義）產生；分頁、排序與篩選在伺服器端處理。
 * 客戶、廠商改用分割檢視（{@link MasterSplitView}）。
 */
export function RecordsPage() {
  const { key = '' } = useParams();
  const entity = useEntity(key);
  const defaultLayout = useDefaultLayout(key);

  const error = entity.error ?? defaultLayout.error;
  if (error) {
    return (
      <Page title="資料列表">
        <Alert showIcon type="error" message={errorMessage(error)} />
      </Page>
    );
  }
  if (!entity.data || !defaultLayout.data) {
    return <Page title="資料列表" loading />;
  }
  const detail = entity.data;
  if (!detail.entity.is_active) {
    return (
      <Page title={detail.entity.label}>
        <EmptyState kind="disabled" title="此資料類別已停用" description="請洽管理者重新啟用" />
      </Page>
    );
  }
  // 客戶、廠商預設使用分割檢視，可以切換回表格（§8.7）
  if (usesSplitView(key)) {
    return (
      <SplitOrTable
        key={key}
        entity={detail.entity}
        fields={detail.fields}
        initial={defaultLayout.data}
      />
    );
  }
  return (
    <RecordList
      key={key}
      entity={detail.entity}
      fields={detail.fields}
      initial={defaultLayout.data}
    />
  );
}

/**
 * 分割檢視與表格列表的切換（§8.7）。每個資料類別記住上次選的檢視（localStorage）；
 * 網址帶有選取或模式（`?selected=`、`?mode=`，例如從舊網址導向）時一律顯示分割檢視。
 */
function SplitOrTable({
  entity,
  fields,
  initial,
}: {
  entity: EntityDefinition;
  fields: FieldDefinition[];
  initial: DefaultViewLayout;
}): ReactNode {
  const entityKey = entity.key;
  const [view, setView] = useState<RecordView>(() => loadRecordView(entityKey));
  const [searchParams, setSearchParams] = useSearchParams();
  const inSplit = searchParams.has(SELECTED_PARAM) || searchParams.has(MODE_PARAM);

  const change = (next: RecordView) => {
    saveRecordView(entityKey, next);
    setView(next);
  };

  /** 切到分割檢視，並設定選取或模式。 */
  const openSplit = (params: { selected?: string; mode?: SplitMode }) => {
    change('split');
    const next = new URLSearchParams();
    if (params.selected) {
      next.set(SELECTED_PARAM, params.selected);
    }
    if (params.mode) {
      next.set(MODE_PARAM, params.mode);
    }
    setSearchParams(next, { replace: true });
  };

  if (view === 'split' || inSplit) {
    return (
      <MasterSplitView
        entity={entity}
        fields={fields}
        onSwitchToTable={() => {
          change('table');
          // 表格不使用選取與模式
          setSearchParams(new URLSearchParams(), { replace: true });
        }}
      />
    );
  }
  return (
    <RecordList
      entity={entity}
      fields={fields}
      initial={initial}
      split={{
        open: (id) => openSplit({ selected: id }),
        create: () => openSplit({ mode: 'new' }),
        switchToSplit: () => openSplit({}),
      }}
    />
  );
}
