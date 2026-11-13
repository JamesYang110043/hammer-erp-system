import { ReloadOutlined, SearchOutlined } from '@ant-design/icons';
import { Alert, Button, Card, DatePicker, Form, Select, Space, Table, Typography } from 'antd';
import type { TableColumnsType } from 'antd';
import { useState } from 'react';

import { useAuditLogs } from '../../api/auditLogs';
import { errorMessage } from '../../api/client';
import { useUsers } from '../../api/users';
import { usePermission } from '../../auth/permission';
import { AUDIT_ACTIONS, PERMISSIONS } from '../../bindings';
import type { AuditLogEntry } from '../../bindings';
import { formatTaipei } from '../../utils/time';
import {
  AUDIT_ACTION_LABELS,
  DEFAULT_PAGE_SIZE,
  actionLabel,
  buildAuditLogQuery,
  entityLabel,
} from './auditQuery';
import type { AuditLogFilters } from './auditQuery';

const columns: TableColumnsType<AuditLogEntry> = [
  {
    title: '時間',
    dataIndex: 'occurred_at',
    width: 180,
    render: (value: string) => formatTaipei(value),
  },
  {
    title: '使用者',
    key: 'user',
    width: 200,
    render: (_, row) =>
      row.username ? (
        `${row.user_display_name ?? ''}（${row.username}）`
      ) : (
        <Typography.Text type="secondary">—</Typography.Text>
      ),
  },
  {
    title: '動作',
    dataIndex: 'action',
    width: 110,
    render: (value: string) => actionLabel(value),
  },
  {
    title: '資料類別',
    dataIndex: 'entity_key',
    width: 110,
    render: (value: string | null | undefined) => (value ? entityLabel(value) : '—'),
  },
  {
    title: '用戶端',
    dataIndex: 'client_info',
    ellipsis: true,
    render: (value: string | null | undefined) => value ?? '—',
  },
];

function hasChanges(row: AuditLogEntry): boolean {
  return row.before_data != null || row.after_data != null;
}

function JsonBlock({ title, value }: { title: string; value: unknown }) {
  return (
    <div style={{ flex: 1, minWidth: 0 }}>
      <Typography.Text strong>{title}</Typography.Text>
      <pre style={{ margin: '4px 0 0', whiteSpace: 'pre-wrap', wordBreak: 'break-all' }}>
        {value == null ? '（無）' : JSON.stringify(value, null, 2)}
      </pre>
    </div>
  );
}

/** 操作紀錄頁（需要 `audit.read`）。 */
export function AuditLogsPage() {
  const [form] = Form.useForm<AuditLogFilters>();
  const [filters, setFilters] = useState<AuditLogFilters>({});
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(DEFAULT_PAGE_SIZE);
  const logs = useAuditLogs(buildAuditLogQuery(filters, page, pageSize));

  // 使用者篩選的選項來自帳號列表，需要 `user.manage`；沒有時不顯示此篩選
  const canListUsers = usePermission(PERMISSIONS.USER_MANAGE);
  const users = useUsers({ enabled: canListUsers });

  const search = (values: AuditLogFilters) => {
    setFilters(values);
    setPage(1);
  };

  return (
    <Card
      title="操作紀錄"
      extra={
        <Button icon={<ReloadOutlined />} onClick={() => logs.refetch()} loading={logs.isFetching}>
          重新整理
        </Button>
      }
    >
      <Form form={form} layout="inline" onFinish={search} style={{ marginBottom: 16, rowGap: 8 }}>
        <Form.Item label="日期" name="dateRange">
          <DatePicker.RangePicker allowEmpty={[true, true]} />
        </Form.Item>
        {canListUsers && (
          <Form.Item label="使用者" name="userId">
            <Select
              allowClear
              showSearch
              optionFilterProp="label"
              placeholder="全部"
              style={{ width: 200 }}
              loading={users.isPending}
              options={users.data?.map((u) => ({
                value: u.id,
                label: `${u.display_name}（${u.username}）`,
              }))}
            />
          </Form.Item>
        )}
        <Form.Item label="動作" name="action">
          <Select
            allowClear
            placeholder="全部"
            style={{ width: 140 }}
            options={AUDIT_ACTIONS.map((a) => ({ value: a, label: AUDIT_ACTION_LABELS[a] }))}
          />
        </Form.Item>
        <Form.Item>
          <Space>
            <Button type="primary" htmlType="submit" icon={<SearchOutlined />}>
              查詢
            </Button>
            <Button
              onClick={() => {
                form.resetFields();
                search({});
              }}
            >
              清除
            </Button>
          </Space>
        </Form.Item>
      </Form>
      {logs.isError && (
        <Alert
          showIcon
          type="error"
          message={errorMessage(logs.error)}
          style={{ marginBottom: 16 }}
        />
      )}
      <Table<AuditLogEntry>
        rowKey="id"
        size="small"
        columns={columns}
        dataSource={logs.data?.items}
        loading={logs.isFetching}
        expandable={{
          rowExpandable: hasChanges,
          expandedRowRender: (row) => (
            <div style={{ display: 'flex', gap: 24 }}>
              <JsonBlock title="變更前" value={row.before_data} />
              <JsonBlock title="變更後" value={row.after_data} />
            </div>
          ),
        }}
        pagination={{
          current: logs.data?.page ?? page,
          pageSize: logs.data?.page_size ?? pageSize,
          total: logs.data?.total ?? 0,
          showSizeChanger: true,
          pageSizeOptions: [20, 50, 100, 200],
          showTotal: (total) => `共 ${total} 筆`,
          onChange: (nextPage, nextSize) => {
            setPage(nextSize !== pageSize ? 1 : nextPage);
            setPageSize(nextSize);
          },
        }}
      />
    </Card>
  );
}
