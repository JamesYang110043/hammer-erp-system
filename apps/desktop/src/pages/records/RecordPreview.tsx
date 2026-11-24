import { DeleteOutlined, EditOutlined } from '@ant-design/icons';
import { Alert, App, Button, Popconfirm, Space, Spin, Table, Typography, theme } from 'antd';
import type { TableColumnsType } from 'antd';
import { useMemo } from 'react';
import type { ReactNode } from 'react';

import { apiErrorCode, errorMessage } from '../../api/client';
import { mutationErrorMessage } from '../../api/errors';
import { useFormLayout } from '../../api/formLayouts';
import { useDeleteRecord, useRecord } from '../../api/records';
import { useEntities, useEntityDetails } from '../../api/schema';
import { usePermission } from '../../auth/permission';
import { PERMISSIONS } from '../../bindings';
import type { EntityDefinition, FieldDefinition, Record as DataRecord } from '../../bindings';
import { EmptyState } from '../../components/EmptyState';
import { SectionTitle } from '../../components/SectionTitle';
import { isNumericType } from '../../records/display';
import { generatedFormLayout, linesBlockDetail, resolveFormLayout } from '../../records/formLayout';
import { FORM_BLOCKS } from '../../records/formLayout';
import { detailEntitiesOf, showsRecordNo } from '../../records/lines';
import { previewFields } from '../../records/masterSplit';
import { FieldValue } from './FieldValue';
import { FormLayoutRenderer } from './FormLayoutRenderer';

/** 唯讀預覽的一個欄位：上方標籤、下方格式化後的值。 */
function PreviewItem({ label, children }: { label: string; children: ReactNode }) {
  const { token } = theme.useToken();
  return (
    <div className="preview-item" style={{ marginBottom: token.marginSM }}>
      <div style={{ color: token.colorTextSecondary, fontSize: token.fontSizeSM, marginBottom: 2 }}>
        {label}
      </div>
      <div style={{ minHeight: token.controlHeightSM, wordBreak: 'break-word' }}>{children}</div>
    </div>
  );
}

/** 明細子表（聯絡人等）的唯讀表格。 */
function PreviewLines({
  entity,
  fields,
  rows,
}: {
  entity: EntityDefinition;
  fields: FieldDefinition[];
  rows: DataRecord[];
}) {
  const columns: TableColumnsType<DataRecord> = [
    { title: '#', key: '__index', width: 48, align: 'right', render: (_, __, i) => i + 1 },
    ...previewFields(fields).map((field): TableColumnsType<DataRecord>[number] => ({
      key: field.field_key,
      title: field.label,
      width: field.column_width ?? undefined,
      align: isNumericType(field) ? 'right' : undefined,
      render: (_, row) => (
        <FieldValue field={field} value={row.data[field.field_key]} record={row} />
      ),
    })),
  ];
  return (
    <section aria-label={entity.label} style={{ marginBottom: 16 }}>
      <SectionTitle>{entity.label}</SectionTitle>
      <Table<DataRecord>
        rowKey="id"
        size="small"
        columns={columns}
        dataSource={rows}
        pagination={false}
        scroll={{ x: 'max-content' }}
        style={{ marginTop: 8 }}
      />
    </section>
  );
}

/**
 * 分割檢視右側的唯讀預覽（docs/architecture.md §8.7）。
 *
 * - 欄位依表單版面（§7.7）排列，值以 {@link FieldValue} 格式化；明細子表（聯絡人）以唯讀表格顯示
 * - 上方「編號＋名稱」標題，有 `record.write` 時顯示「編輯」「刪除」
 * - 資料不存在或已刪除時顯示「找不到資料」
 */
export function RecordPreview({
  entity,
  fields,
  id,
  onEdit,
  onDeleted,
}: {
  entity: EntityDefinition;
  fields: FieldDefinition[];
  id: string;
  onEdit: () => void;
  /** 刪除成功後（清除選取）。 */
  onDeleted: () => void;
}) {
  const entityKey = entity.key;
  const { message } = App.useApp();
  const canWrite = usePermission(PERMISSIONS.RECORD_WRITE);
  const record = useRecord(entityKey, id);
  const entities = useEntities();
  const detailEntities = useMemo(
    () => (entities.data ? detailEntitiesOf(entities.data, entity) : undefined),
    [entities.data, entity],
  );
  const detailDefs = useEntityDetails(detailEntities);
  const formLayout = useFormLayout(entityKey);
  const deleteRecord = useDeleteRecord(entityKey);

  const shown = useMemo(() => previewFields(fields), [fields]);
  const layout = useMemo(() => {
    if (!detailDefs.data || formLayout.isPending) {
      return undefined;
    }
    const content = {
      entityKey,
      fieldKeys: shown.map((f) => f.field_key),
      detailKeys: detailDefs.data.map((d) => d.entity.key),
    };
    // 讀取版面失敗時依欄位定義產生
    return resolveFormLayout(formLayout.data?.config ?? generatedFormLayout(content), content);
  }, [detailDefs.data, formLayout.isPending, formLayout.data, entityKey, shown]);

  if (record.error) {
    return apiErrorCode(record.error) === 'NOT_FOUND' ? (
      <EmptyState kind="notFound" description="這筆資料可能已被刪除，請從左側重新選擇" />
    ) : (
      <Alert showIcon type="error" message={errorMessage(record.error)} />
    );
  }
  const error = entities.error ?? detailDefs.error;
  if (error) {
    return <Alert showIcon type="error" message={errorMessage(error)} />;
  }
  const data = record.data;
  const defs = detailDefs.data;
  if (!data || !layout || !defs) {
    return (
      <div style={{ padding: 24, textAlign: 'center' }}>
        <Spin />
      </div>
    );
  }

  const byKey = new Map(shown.map((f) => [f.field_key, f]));
  const remove = () =>
    deleteRecord.mutateAsync(data).then(
      () => {
        message.success('已刪除');
        onDeleted();
      },
      (err: unknown) => void message.error(mutationErrorMessage(err)),
    );

  const renderBlock = (code: string): ReactNode => {
    const detailKey = linesBlockDetail(code);
    if (detailKey !== undefined) {
      const def = defs.find((d) => d.entity.key === detailKey);
      return def ? (
        <PreviewLines
          entity={def.entity}
          fields={def.fields}
          rows={data.lines?.[detailKey] ?? []}
        />
      ) : null;
    }
    return code === FORM_BLOCKS.recordNo && showsRecordNo(entity, data) ? (
      <PreviewItem label="單號">{data.record_no ?? ''}</PreviewItem>
    ) : null;
  };

  return (
    <div data-testid="record-preview">
      <div className="preview-header">
        <Typography.Title level={5} style={{ margin: 0 }} data-testid="preview-title">
          {data.title}
        </Typography.Title>
        {canWrite && (
          <Space>
            <Button icon={<EditOutlined />} onClick={onEdit}>
              編輯
            </Button>
            <Popconfirm
              title="確定要刪除這筆資料？"
              okText="刪除"
              okButtonProps={{ danger: true }}
              cancelText="取消"
              onConfirm={remove}
            >
              <Button danger icon={<DeleteOutlined />} loading={deleteRecord.isPending}>
                刪除
              </Button>
            </Popconfirm>
          </Space>
        )}
      </div>
      <FormLayoutRenderer
        config={layout}
        renderField={(fieldKey) => {
          const field = byKey.get(fieldKey);
          return field ? (
            <PreviewItem label={field.label}>
              <FieldValue field={field} value={data.data[fieldKey]} record={data} />
            </PreviewItem>
          ) : null;
        }}
        renderBlock={renderBlock}
      />
    </div>
  );
}
