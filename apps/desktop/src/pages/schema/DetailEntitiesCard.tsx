import { PlusOutlined } from '@ant-design/icons';
import { Alert, Button, Card, Table, Tag, Typography } from 'antd';
import type { TableColumnsType } from 'antd';
import { useState } from 'react';
import { useNavigate } from 'react-router';

import { errorMessage } from '../../api/client';
import { useAllEntities } from '../../api/schema';
import type { EntityDefinition } from '../../bindings';
import { detailEntitiesOf } from '../../schema/entities';
import { EntityActions } from './EntityActions';
import { EntityFormModal } from './EntityFormModal';
import type { EntityFormTarget } from './EntityFormModal';
import { entityIcon } from './entityIcons';

/**
 * 主表（主檔、單據）的明細子表：列表、新增、編輯、停用、刪除，點「欄位設定」進入明細子表的欄位定義頁。
 *
 * 明細子表不出現在左側選單，只能從這裡管理（docs/architecture.md §8.5「明細子表」）。
 */
export function DetailEntitiesCard({ parent }: { parent: EntityDefinition }) {
  const navigate = useNavigate();
  const entities = useAllEntities();
  const [formTarget, setFormTarget] = useState<EntityFormTarget>();
  const details = detailEntitiesOf(entities.data ?? [], parent.id);

  const open = (key: string) => navigate(`/schema/${key}`);

  const columns: TableColumnsType<EntityDefinition> = [
    {
      title: '名稱',
      dataIndex: 'label',
      render: (label: string, e) => (
        <Button type="link" icon={entityIcon(e.icon)} onClick={() => open(e.key)}>
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
      title: '狀態',
      dataIndex: 'is_active',
      render: (active: boolean) => (active ? <Tag color="green">啟用</Tag> : <Tag>已停用</Tag>),
    },
    {
      title: '操作',
      key: 'actions',
      render: (_, e) => (
        <EntityActions
          entity={e}
          onEdit={() => setFormTarget({ mode: 'edit', entity: e })}
          onOpenFields={() => open(e.key)}
        />
      ),
    },
  ];

  const nextSortOrder = details.reduce((max, e) => Math.max(max, e.sort_order + 1), 0);

  return (
    <Card
      title="明細子表"
      extra={
        <Button
          icon={<PlusOutlined />}
          onClick={() =>
            setFormTarget({ mode: 'create', parent, sortOrderFor: () => nextSortOrder })
          }
          disabled={!entities.data}
        >
          新增明細子表
        </Button>
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
        明細子表是單據中的多筆明細（例如訂單的商品明細），在「{parent.label}」的表單中編輯。
      </Typography.Paragraph>
      <Table<EntityDefinition>
        rowKey="id"
        size="small"
        columns={columns}
        dataSource={details}
        loading={entities.isPending}
        pagination={false}
        locale={{ emptyText: '還沒有明細子表' }}
      />
      <EntityFormModal
        target={formTarget}
        onClose={() => setFormTarget(undefined)}
        onCreated={(detail) => open(detail.entity.key)}
      />
    </Card>
  );
}
