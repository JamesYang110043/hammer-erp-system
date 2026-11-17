import { ArrowLeftOutlined, PlusOutlined, ReloadOutlined } from '@ant-design/icons';
import type { DragEndEvent } from '@dnd-kit/core';
import { arrayMove } from '@dnd-kit/sortable';
import { Alert, App, Button, Card, Popconfirm, Space, Table, Tag, Typography } from 'antd';
import type { TableColumnsType } from 'antd';
import { useMemo, useState } from 'react';
import { useNavigate, useParams } from 'react-router';

import { errorMessage } from '../../api/client';
import { mutationErrorMessage } from '../../api/errors';
import {
  useAllEntities,
  useDeactivateField,
  useEntity,
  useReorderFields,
  useUpdateField,
} from '../../api/schema';
import type { FieldDefinition } from '../../bindings';
import { toReactivateRequest } from '../../schema/fieldForm';
import { DATA_TYPE_LABELS, canChangeType, canDeactivate } from '../../schema/fieldSettings';
import { duplicateValues, formulaReferences } from '../../schema/errorDetails';
import { ChangeTypeModal } from './ChangeTypeModal';
import { DetailEntitiesCard } from './DetailEntitiesCard';
import { FieldRefList } from './EntityActions';
import { FieldFormModal } from './FieldFormModal';
import type { FieldFormTarget } from './FieldFormModal';
import { DragHandle, SortableRow, SortableTableContext } from './sortableRows';

/** 欄位的屬性標籤。 */
function FieldTags({ field }: { field: FieldDefinition }) {
  return (
    <Space size={[0, 4]} wrap>
      {!field.is_active && <Tag color="default">已停用</Tag>}
      {field.is_system && <Tag color="purple">系統</Tag>}
      {field.is_required && <Tag color="red">必填</Tag>}
      {field.is_unique && <Tag color="orange">不可重複</Tag>}
      {field.is_searchable && <Tag color="blue">可搜尋</Tag>}
      {field.is_readonly && <Tag>唯讀</Tag>}
      {!field.is_visible && <Tag>隱藏</Tag>}
    </Space>
  );
}

/**
 * 欄位定義頁（需要 `schema.manage`）：欄位列表、拖曳排序、新增、編輯、停用、變更型態。
 *
 * 自訂的主檔與單據、有明細子表的系統主檔另外顯示明細子表（{@link DetailEntitiesCard}）；
 * 明細子表的「返回」回到所屬的主表。
 */
export function FieldsPage() {
  const { key = '' } = useParams();
  const navigate = useNavigate();
  const { message, modal } = App.useApp();
  const entity = useEntity(key);
  const reorder = useReorderFields(key);
  const deactivate = useDeactivateField(key);
  const updateField = useUpdateField(key);
  const [formTarget, setFormTarget] = useState<FieldFormTarget>();
  const [changingType, setChangingType] = useState<FieldDefinition>();
  const allEntities = useAllEntities();
  const current = entity.data?.entity;
  const parent =
    current?.kind === 'detail'
      ? allEntities.data?.find((e) => e.id === current.parent_entity_id)
      : undefined;
  // 自訂的主檔、單據可以新增明細子表；系統主檔只列出既有的明細子表（例如聯絡人，§7.1）
  const hasDetails =
    current !== undefined &&
    current.kind !== 'detail' &&
    (!current.is_system ||
      (allEntities.data ?? []).some(
        (e) => e.kind === 'detail' && e.parent_entity_id === current.id,
      ));

  const fields = useMemo(() => entity.data?.fields ?? [], [entity.data]);

  const showError = (err: unknown) => message.error(mutationErrorMessage(err));

  const onDragEnd = ({ active, over }: DragEndEvent) => {
    if (!over || active.id === over.id) {
      return;
    }
    const from = fields.findIndex((f) => f.id === active.id);
    const to = fields.findIndex((f) => f.id === over.id);
    reorder.mutate(arrayMove(fields, from, to), { onError: showError });
  };

  const reactivate = (field: FieldDefinition) =>
    updateField.mutate(
      { fieldKey: field.field_key, req: toReactivateRequest(field) },
      {
        onSuccess: () => void message.success(`已啟用「${field.label}」`),
        onError: (err) => {
          const dup = duplicateValues(err);
          void message.error(
            dup
              ? `已有 ${dup.total} 個值重複，無法重新啟用不可重複的欄位`
              : mutationErrorMessage(err),
          );
        },
      },
    );

  const columns: TableColumnsType<FieldDefinition> = [
    {
      key: 'sort',
      width: 48,
      align: 'center',
      render: (_, f) => <DragHandle label={f.label} />,
    },
    {
      title: '顯示名稱',
      dataIndex: 'label',
      render: (label: string, f) => (
        <Typography.Text type={f.is_active ? undefined : 'secondary'}>{label}</Typography.Text>
      ),
    },
    {
      title: '欄位代碼',
      dataIndex: 'field_key',
      render: (k: string) => <Typography.Text code>{k}</Typography.Text>,
    },
    {
      title: '資料型態',
      dataIndex: 'data_type',
      render: (_, f) =>
        f.data_type === 'formula' && f.formula_result_type
          ? `${DATA_TYPE_LABELS.formula}（${DATA_TYPE_LABELS[f.formula_result_type]}）`
          : DATA_TYPE_LABELS[f.data_type],
    },
    { title: '屬性', key: 'flags', render: (_, f) => <FieldTags field={f} /> },
    {
      title: '操作',
      key: 'actions',
      render: (_, f) => (
        <Space size={0} wrap>
          <Button type="link" size="small" onClick={() => setFormTarget(f)}>
            編輯
          </Button>
          {canChangeType(f) && (
            <Button type="link" size="small" onClick={() => setChangingType(f)}>
              變更型態
            </Button>
          )}
          {canDeactivate(f) && (
            <Popconfirm
              title={`確定要停用欄位「${f.label}」？`}
              description="停用後不會出現在列表與表單，資料會保留，之後可以重新啟用。"
              okText="停用"
              okButtonProps={{ danger: true }}
              cancelText="取消"
              onConfirm={() =>
                deactivate.mutateAsync(f).then(
                  () => void message.success(`已停用「${f.label}」，資料已保留`),
                  (err: unknown) => {
                    const formulas = formulaReferences(err);
                    if (!formulas) {
                      return showError(err);
                    }
                    modal.error({
                      title: `無法停用「${f.label}」`,
                      content: (
                        <FieldRefList
                          intro={`以下 ${formulas.length} 個公式欄位引用了這個欄位，請先修改或停用這些公式：`}
                          fields={formulas}
                        />
                      ),
                      okText: '知道了',
                      width: 520,
                    });
                  },
                )
              }
            >
              <Button type="link" size="small" danger>
                停用
              </Button>
            </Popconfirm>
          )}
          {!f.is_active && (
            <Button type="link" size="small" onClick={() => reactivate(f)}>
              啟用
            </Button>
          )}
        </Space>
      ),
    },
  ];

  const nextSortOrder = fields.reduce((max, f) => Math.max(max, f.sort_order + 1), 0);

  return (
    <Space direction="vertical" size="middle" style={{ width: '100%' }}>
      <Card
        title={
          <Space>
            <Button
              type="text"
              icon={<ArrowLeftOutlined />}
              aria-label={parent ? `回到「${parent.label}」` : '回到資料類別列表'}
              onClick={() => navigate(parent ? `/schema/${parent.key}` : '/schema')}
            />
            {entity.data ? `欄位定義：${entity.data.entity.label}` : '欄位定義'}
          </Space>
        }
        extra={
          <Space>
            <Button
              icon={<ReloadOutlined />}
              onClick={() => entity.refetch()}
              loading={entity.isFetching}
            >
              重新整理
            </Button>
            <Button
              type="primary"
              icon={<PlusOutlined />}
              onClick={() => setFormTarget('new')}
              disabled={!entity.data}
            >
              新增欄位
            </Button>
          </Space>
        }
      >
        {entity.isError && (
          <Alert
            showIcon
            type="error"
            message={errorMessage(entity.error)}
            style={{ marginBottom: 16 }}
          />
        )}
        <Typography.Paragraph type="secondary">
          拖曳左側把手調整欄位順序；停用的欄位資料會保留。
        </Typography.Paragraph>
        <SortableTableContext ids={fields.map((f) => f.id)} onDragEnd={onDragEnd}>
          <Table<FieldDefinition>
            rowKey="id"
            size="small"
            components={{ body: { row: SortableRow } }}
            columns={columns}
            dataSource={fields}
            loading={entity.isPending || reorder.isPending}
            pagination={false}
          />
        </SortableTableContext>
        <FieldFormModal
          entityKey={key}
          target={formTarget}
          nextSortOrder={nextSortOrder}
          onClose={() => setFormTarget(undefined)}
        />
        <ChangeTypeModal
          entityKey={key}
          field={changingType}
          onClose={() => setChangingType(undefined)}
        />
      </Card>
      {hasDetails && current && <DetailEntitiesCard parent={current} />}
    </Space>
  );
}
