import {
  DeleteOutlined,
  PlusOutlined,
  ReloadOutlined,
  SaveOutlined,
  SettingOutlined,
} from '@ant-design/icons';
import { Alert, App, Button, Card, Empty, Popconfirm, Select, Space, Table, Tag } from 'antd';
import type { TableColumnsType, TableProps } from 'antd';
import { useState } from 'react';
import { useNavigate, useParams } from 'react-router';

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
import { newRecordPath } from '../../modules';
import type {
  DefaultViewLayout_Serialize as DefaultViewLayout,
  EntityDefinition,
  FieldDefinition,
  Record as DataRecord,
} from '../../bindings';
import { generatedConfig, resolveColumns } from '../../records/columns';
import type { LayoutColumn, LayoutConfig } from '../../records/columns';
import { isNumericType } from '../../records/display';
import { GENERATED_LAYOUT_ID, canEditLayout, sameConfig } from '../../records/layouts';
import type { ViewLayout } from '../../records/layouts';
import {
  buildRecordListQuery,
  columnSortOrder,
  isQueryable,
  sortFromTable,
} from '../../records/query';
import { entityIcon } from '../schema/entityIcons';
import { ColumnSettingsModal } from './ColumnSettingsModal';
import { FieldValue } from './FieldValue';
import { FilterPanel } from './FilterPanel';
import { SaveLayoutModal } from './SaveLayoutModal';

const DEFAULT_PAGE_SIZE = 50;

function layoutLabel(layout: ViewLayout): string {
  return layout.user_id === null ? `${layout.name}（全域）` : layout.name;
}

/** 一個資料類別的列表。`initial` 為開啟時套用的預設版面。 */
function RecordList({
  entity,
  fields,
  initial,
}: {
  entity: EntityDefinition;
  fields: FieldDefinition[];
  initial: DefaultViewLayout;
}) {
  const entityKey = entity.key;
  const navigate = useNavigate();
  const { message } = App.useApp();
  const user = useCurrentUser();
  const canWrite = usePermission(PERMISSIONS.RECORD_WRITE);
  const canManageSchema = usePermission(PERMISSIONS.SCHEMA_MANAGE);

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
      ellipsis: field.data_type === 'long_text',
      sorter: isQueryable(field),
      sortOrder: columnSortOrder(config.sort, field.field_key),
      render: (value: unknown) => <FieldValue field={field} value={value} />,
    }),
  );
  if (canWrite) {
    columns.push({
      title: '操作',
      key: '__actions',
      width: 80,
      fixed: 'right',
      render: (_, record) => (
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
    <Card
      title={
        <Space>
          {entityIcon(entity.icon)}
          {entity.label}
        </Space>
      }
      extra={
        <Space>
          <Button
            icon={<ReloadOutlined />}
            onClick={() => records.refetch()}
            loading={records.isFetching}
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
        </Space>
      }
    >
      <Space wrap style={{ marginBottom: 16 }}>
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
        {dirty && <Tag color="orange">未儲存的變更</Tag>}
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
        fields={fields.filter(isQueryable).sort((a, b) => a.sort_order - b.sort_order)}
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
    </Card>
  );
}

/**
 * 自訂資料類別的列表（需要 `record.read`），路徑 `/entities/:key`。
 *
 * 欄位依版面設定（沒有版面時依欄位定義）產生；分頁、排序與篩選在伺服器端處理。
 */
export function RecordsPage() {
  const { key = '' } = useParams();
  const entity = useEntity(key);
  const defaultLayout = useDefaultLayout(key);

  const error = entity.error ?? defaultLayout.error;
  if (error) {
    return (
      <Card title="資料列表">
        <Alert showIcon type="error" message={errorMessage(error)} />
      </Card>
    );
  }
  if (!entity.data || !defaultLayout.data) {
    return <Card title="資料列表" loading />;
  }
  const detail = entity.data;
  if (detail.entity.storage !== 'custom_records' || !detail.entity.is_active) {
    return (
      <Card title={detail.entity.label}>
        <Empty description="此資料類別尚未開放列表" />
      </Card>
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
