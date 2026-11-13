import { PlusOutlined, ReloadOutlined } from '@ant-design/icons';
import { Alert, Button, Card, Space, Table, Tag, Typography } from 'antd';
import type { TableColumnsType } from 'antd';
import { useState } from 'react';
import { useNavigate } from 'react-router';

import { errorMessage } from '../../api/client';
import { useEntities } from '../../api/schema';
import type { EntityDefinition, EntityKind } from '../../bindings';
import { CreateEntityModal } from './CreateEntityModal';
import { entityIcon } from './entityIcons';

const KIND_LABELS: Record<EntityKind, string> = {
  master: '主檔',
  document: '單據',
  detail: '明細',
};

/** 資料類別列表（需要 `schema.manage`）。點名稱進入欄位定義頁。 */
export function EntitiesPage() {
  const navigate = useNavigate();
  const entities = useEntities();
  const [creating, setCreating] = useState(false);

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
    { title: '類型', dataIndex: 'kind', render: (k: EntityKind) => KIND_LABELS[k] },
    {
      title: '來源',
      dataIndex: 'is_system',
      render: (system: boolean) =>
        system ? <Tag color="purple">系統</Tag> : <Tag color="blue">自訂</Tag>,
    },
    {
      title: '操作',
      key: 'actions',
      render: (_, e) => (
        <Button type="link" size="small" onClick={() => open(e.key)}>
          欄位設定
        </Button>
      ),
    },
  ];

  return (
    <Card
      title="資料類別"
      extra={
        <Space>
          <Button
            icon={<ReloadOutlined />}
            onClick={() => entities.refetch()}
            loading={entities.isFetching}
          >
            重新整理
          </Button>
          <Button type="primary" icon={<PlusOutlined />} onClick={() => setCreating(true)}>
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
      <Table<EntityDefinition>
        rowKey="id"
        size="small"
        columns={columns}
        dataSource={entities.data}
        loading={entities.isPending}
        pagination={false}
      />
      <CreateEntityModal
        open={creating}
        onClose={() => setCreating(false)}
        onCreated={(detail) => open(detail.entity.key)}
      />
    </Card>
  );
}
