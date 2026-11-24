import { PlusOutlined, ReloadOutlined } from '@ant-design/icons';
import type { DragEndEvent } from '@dnd-kit/core';
import { arrayMove } from '@dnd-kit/sortable';
import { Alert, App, Button, Space, Table, Tag, Typography } from 'antd';
import type { TableColumnsType } from 'antd';
import { useState } from 'react';
import { useNavigate } from 'react-router';

import { StatusTag } from '../../components/StatusTag';
import { SectionTitle } from '../../components/SectionTitle';
import { Page } from '../../components/PageHeader';
import { errorMessage } from '../../api/client';
import { mutationErrorMessage } from '../../api/errors';
import { useAllEntities, useReorderEntities } from '../../api/schema';
import type { EntityDefinition } from '../../bindings';
import { groupEntities, reorderItems } from '../../schema/entities';
import type { EntityGroup, MenuKind } from '../../schema/entities';
import { EntityActions } from './EntityActions';
import { EntityFormModal } from './EntityFormModal';
import type { EntityFormTarget } from './EntityFormModal';
import { entityIcon } from './entityIcons';
import { DragHandle, SortableRow, SortableTableContext } from './sortableRows';

/** 同一組（主檔或單據）的資料類別，可拖曳調整選單排序。 */
function EntityGroupTable({
  group,
  loading,
  onReorder,
  onEdit,
}: {
  group: EntityGroup;
  loading: boolean;
  onReorder: (ordered: EntityDefinition[]) => void;
  onEdit: (entity: EntityDefinition) => void;
}) {
  const navigate = useNavigate();
  const open = (key: string) => navigate(`/schema/${key}`);
  const { entities } = group;

  const onDragEnd = ({ active, over }: DragEndEvent) => {
    if (!over || active.id === over.id) {
      return;
    }
    const from = entities.findIndex((e) => e.id === active.id);
    const to = entities.findIndex((e) => e.id === over.id);
    onReorder(arrayMove(entities, from, to));
  };

  const columns: TableColumnsType<EntityDefinition> = [
    {
      key: 'sort',
      width: 48,
      align: 'center',
      render: (_, e) => <DragHandle label={e.label} />,
    },
    {
      title: '名稱',
      dataIndex: 'label',
      render: (label: string, e) => (
        <Button
          type="link"
          icon={entityIcon(e.icon)}
          onClick={() => open(e.key)}
          style={e.is_active ? undefined : { opacity: 0.6 }}
        >
          {label}
        </Button>
      ),
    },
    {
      title: '代碼',
      dataIndex: 'key',
      render: (k: string) => <Typography.Text code>{k}</Typography.Text>,
    },
    {
      title: '單號規則',
      dataIndex: 'numbering_pattern',
      render: (p: string | null) =>
        p ? (
          <Typography.Text code>{p}</Typography.Text>
        ) : (
          <Typography.Text type="secondary">—</Typography.Text>
        ),
    },
    {
      title: '狀態',
      key: 'status',
      render: (_, e) => (
        <Space size={[0, 4]} wrap>
          {e.is_system ? (
            <StatusTag tone="info">系統</StatusTag>
          ) : (
            <StatusTag tone="neutral">自訂</StatusTag>
          )}
          {!e.is_active && <Tag>已停用</Tag>}
        </Space>
      ),
    },
    {
      title: '操作',
      key: 'actions',
      render: (_, e) => (
        <EntityActions entity={e} onEdit={() => onEdit(e)} onOpenFields={() => open(e.key)} />
      ),
    },
  ];

  return (
    <section aria-label={group.label}>
      <SectionTitle>{group.label}</SectionTitle>
      <SortableTableContext ids={entities.map((e) => e.id)} onDragEnd={onDragEnd}>
        <Table<EntityDefinition>
          rowKey="id"
          size="small"
          components={{ body: { row: SortableRow } }}
          columns={columns}
          dataSource={entities}
          loading={loading}
          pagination={false}
          locale={{ emptyText: `還沒有${group.label}` }}
        />
      </SortableTableContext>
    </section>
  );
}

/**
 * 資料類別管理（需要 `schema.manage`，伺服器檢查）：依種類分組（主檔、單據），含已停用的資料類別。
 *
 * 拖曳調整選單排序；新增、編輯、停用、重新啟用、刪除；點名稱或「欄位設定」進入欄位定義頁，
 * 明細子表在所屬主表的欄位定義頁管理。
 */
export function EntitiesPage() {
  const navigate = useNavigate();
  const { message } = App.useApp();
  const entities = useAllEntities();
  const reorder = useReorderEntities();
  const [formTarget, setFormTarget] = useState<EntityFormTarget>();
  // 送出排序時先以新順序顯示，完成（重新取得列表）後清除
  const [pendingOrder, setPendingOrder] = useState<Partial<Record<MenuKind, string[]>>>({});

  const groups = groupEntities(entities.data ?? []).map((g) => {
    const order = pendingOrder[g.kind];
    if (!order) {
      return g;
    }
    const byId = new Map(g.entities.map((e) => [e.id, e]));
    const ordered = order.flatMap((id) => byId.get(id) ?? []);
    return ordered.length === g.entities.length ? { ...g, entities: ordered } : g;
  });

  const onReorder = (kind: MenuKind, ordered: EntityDefinition[]) => {
    const items = reorderItems(ordered);
    if (items.length === 0) {
      return;
    }
    setPendingOrder((p) => ({ ...p, [kind]: ordered.map((e) => e.id) }));
    reorder.mutate(
      { entities: items },
      {
        onError: (err) => void message.error(mutationErrorMessage(err)),
        onSettled: () => setPendingOrder((p) => ({ ...p, [kind]: undefined })),
      },
    );
  };

  const sortOrderFor = (kind: string) =>
    (entities.data ?? [])
      .filter((e) => e.kind === kind)
      .reduce((max, e) => Math.max(max, e.sort_order + 1), 0);

  return (
    <Page
      title="資料類別"
      actions={
        <Space>
          <Button
            icon={<ReloadOutlined />}
            onClick={() => entities.refetch()}
            loading={entities.isFetching}
          >
            重新整理
          </Button>
          <Button
            type="primary"
            icon={<PlusOutlined />}
            onClick={() => setFormTarget({ mode: 'create', sortOrderFor })}
          >
            新增資料類別
          </Button>
        </Space>
      }
    >
      {entities.isError && (
        <Alert
          showIcon
          type="error"
          message={errorMessage(entities.error)}
          style={{ marginBottom: 16 }}
        />
      )}
      <Typography.Paragraph type="secondary">
        拖曳左側把手調整選單順序；停用的資料類別不會出現在選單，資料會保留。明細子表在所屬主表的「欄位設定」中管理。
      </Typography.Paragraph>
      <Space direction="vertical" size="large" style={{ width: '100%' }}>
        {groups.map((g) => (
          <EntityGroupTable
            key={g.kind}
            group={g}
            loading={entities.isPending || reorder.isPending}
            onReorder={(ordered) => onReorder(g.kind, ordered)}
            onEdit={(entity) => setFormTarget({ mode: 'edit', entity })}
          />
        ))}
      </Space>
      <EntityFormModal
        target={formTarget}
        onClose={() => setFormTarget(undefined)}
        onCreated={(detail) => navigate(`/schema/${detail.entity.key}`)}
      />
    </Page>
  );
}
