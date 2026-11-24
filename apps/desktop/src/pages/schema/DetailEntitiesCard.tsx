import { PlusOutlined } from '@ant-design/icons';
import { Alert, Button, Card, Table, Typography } from 'antd';
import type { TableColumnsType } from 'antd';
import { useState } from 'react';
import { useNavigate } from 'react-router';

import { StatusTag } from '../../components/StatusTag';
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
 * 系統主檔（客戶、廠商）的明細子表（聯絡人）由系統建立，不顯示新增、停用、刪除。
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
      render: (active: boolean) =>
        active ? (
          <StatusTag tone="success">啟用</StatusTag>
        ) : (
          <StatusTag tone="neutral">已停用</StatusTag>
        ),
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
        // 系統資料類別不可新增明細子表（§7.1）
        !parent.is_system && (
          <Button
            icon={<PlusOutlined />}
            onClick={() =>
              setFormTarget({ mode: 'create', parent, sortOrderFor: () => nextSortOrder })
            }
            disabled={!entities.data}
          >
            新增明細子表
          </Button>
        )
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
        {parent.is_system && '系統資料類別不可新增明細子表，可以在既有的明細子表新增自訂欄位。'}
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
